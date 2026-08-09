#!/usr/bin/env bash
# kuma smoke tests: build every committed example, and optionally boot it.
#
# The promise under test is the one the stability plan opens with: a
# declaration that validates either becomes a running system matching it,
# or fails loudly. `cargo test` already checks what can be checked without
# a machine (every example compiles to an image that keeps kuma's floor);
# this is the part that needs real podman, real bootc, and a real boot.
#
# Three stages, cheapest first, each a superset of the last:
#
#   check   parse and validate the declaration            (no podman)
#   image   build it and inspect the built layers         (podman)
#   boot    make a disk, boot it headless, ask the        (podman + kvm + sudo)
#           machine whether the boot was healthy
#
# The boot stage's verdict is greenboot's own: the same check that decides
# whether this machine would roll an update back is the one that decides
# whether the test passed. On a desktop image that includes reaching the
# greeter, so "boots fine into a black screen" fails here rather than on
# your laptop.
#
# Usage:
#   scripts/smoke.sh                  # check + image, every example
#   scripts/smoke.sh --boot           # all three stages, every example
#   scripts/smoke.sh --boot minimal   # just one, by example name
#   scripts/smoke.sh --keep           # leave images and disks behind
#
# Env: KUMA (default target/debug/kuma), QEMU_DISPLAY (default egl-headless).
set -euo pipefail

cd "$(dirname "$0")/.."

KUMA=${KUMA:-target/debug/kuma}
# Headless but GL-capable: a compositor needs a DRM device with a working
# GBM allocator, so -display none is not enough for a desktop image.
# LIBGL_ALWAYS_SOFTWARE keeps guest GL work on llvmpipe, out of the host's
# GPU driver, where a bad guest submission could otherwise take the host
# session down with it.
QEMU_DISPLAY=${QEMU_DISPLAY:-egl-headless}
BOOT=0
KEEP=0
SELECTED=()

while [ $# -gt 0 ]; do
    case "$1" in
        --boot) BOOT=1 ;;
        --keep) KEEP=1 ;;
        -h|--help) sed -n '2,30p' "$0"; exit 0 ;;
        -*) echo "unknown flag: $1" >&2; exit 2 ;;
        *) SELECTED+=("$1") ;;
    esac
    shift
done

[ -x "$KUMA" ] || { echo "no kuma binary at $KUMA (cargo build first, or set KUMA)" >&2; exit 2; }

# A stale binary is the stale-disk trap wearing a different hat: every
# stage passes and none of it tested your change. `cargo test` and
# `cargo clippy` both leave target/debug/kuma untouched, so this is easy
# to hit. Refuse instead of warning, because a smoke test you have to
# remember to distrust is not a gate. (This build isn't run for you:
# on a bootc host the toolbox owns the compiler.)
stale=$(find src Cargo.toml Cargo.lock -newer "$KUMA" -print -quit 2>/dev/null || true)
[ -z "$stale" ] || {
    echo "$KUMA is older than $stale; rebuild it first (cargo build)" >&2
    exit 2
}

PASS=(); FAIL=()
note() { printf '\n\033[1m== %s\033[0m\n' "$*"; }
ok()   { printf '   ok   %s\n' "$*"; }
# exit, not return. Each example's stages run inside `if ( ... )`, and
# bash disables `set -e` for the whole dynamic extent of a command whose
# exit status is being tested, so a `|| bad` that only returned printed
# FAIL and then carried on to the next assertion and the summary. This
# harness reported "all good" over a failing unit exactly once, which is
# once more than a test harness gets to.
bad()  { printf '   FAIL %s\n' "$*"; exit 1; }

# Declared rpm names, read from the declaration itself so the assertion
# can't drift from what the example actually asks for.
declared_rpms() {
    python3 -c '
import tomllib, sys
with open(sys.argv[1], "rb") as f:
    print(" ".join(tomllib.load(f).get("packages", {}).get("rpm", [])))
' "$1"
}

# --- stage: image ------------------------------------------------------
# What a successful build already proves is not worth re-asserting (dnf
# resolved, the lint passed, every RUN test -f held). These are the things
# a build can succeed *without*.
smoke_image() {
    local file=$1 tag=$2

    "$KUMA" --config "$file" check >/dev/null || bad "check: $file"
    ok "declaration validates"

    "$KUMA" --config "$file" build --tag "$tag" >/dev/null || bad "build failed"
    ok "image builds"

    # Self-describing: the machine carries the declaration it was made
    # from, and `kuma init` on it must reproduce this file exactly.
    podman run --rm "$tag" cat /usr/lib/kuma/kuma.toml > /tmp/kuma-smoke-baked.toml
    diff -q "$file" /tmp/kuma-smoke-baked.toml >/dev/null \
        || bad "baked declaration differs from $file"
    rm -f /tmp/kuma-smoke-baked.toml
    ok "baked declaration is byte-identical"

    # The branding sed runs in the last layer over a file the base owns;
    # a silent no-op there is invisible until a machine says "Fedora".
    podman run --rm "$tag" sh -c '. /usr/lib/os-release && [ "$ID" = kuma ]' \
        || bad "os-release ID is not kuma"
    ok "identity is kuma's"

    podman run --rm "$tag" test -f /usr/libexec/greenboot/greenboot \
        || bad "greenboot missing: this image cannot roll back a bad update"
    ok "boot health present"

    # The lock is written by the build, and the pin is only real if the
    # next build actually resolves it. `generate` prints what a build
    # would do, so it proves the wiring without a second build.
    local lock="${file%.toml}.lock"
    [ -f "$lock" ] || bad "no lock written beside $file"
    grep -q '^digest = "sha256:' "$lock" || bad "lock records no base digest"
    if grep -q '^base *=' "$file"; then
        "$KUMA" --config "$file" generate | grep -qE '^FROM .+@sha256:' \
            || bad "builds would ignore the locked digest"
        ok "lock pins the base by digest"

        # The digest just recorded is the one this build used, so the registry
        # has to agree the base is current. A "moved" here means the lock is
        # recording a different KIND of digest than the tag resolves to (the
        # per-architecture manifest instead of the OCI index), which is a
        # permanent false alarm rather than news. That shipped once.
        "$KUMA" --config "$file" update --check | grep -q 'is current' \
            || bad "update --check disagrees with the lock this build just wrote"
        ok "check agrees with the fresh lock"
    else
        # Composed base: the pin is the content-addressed tag itself.
        # Builds FROM it (a localhost/ tag never touches a registry) and
        # the lock's reference must be that tag, so a manifest change
        # reads as a moved reference.
        "$KUMA" --config "$file" generate | grep -qE '^FROM localhost/kuma-base:m' \
            || bad "builds don't FROM the composed content tag"
        grep -q '^ref = "localhost/kuma-base:m' "$lock" \
            || bad "lock doesn't reference the composed base tag"
        ok "lock records the composed base"

        "$KUMA" --config "$file" update --check | grep -q 'composed' \
            || bad "update --check doesn't explain composed-base updates"
        ok "check explains recompose semantics"
    fi
}

# --- stage: boot -------------------------------------------------------
smoke_boot() {
    local file=$1 tag=$2 name=$3 port=$4
    local dir="vm-smoke/$name"
    local disk="$dir/qcow2/disk.qcow2"
    local log="$dir/console.log"

    echo "   .. building disk (bootc-image-builder, needs sudo)"
    "$KUMA" --config "$file" vm --tag "$tag" --output "$dir" --no-run --rebuild >/dev/null \
        || bad "disk build failed"
    ok "disk built"

    env LIBGL_ALWAYS_SOFTWARE=1 qemu-system-x86_64 \
        -enable-kvm -cpu host -smp 4 -m 4096 \
        -drive "file=$disk,if=virtio" \
        -device virtio-vga-gl -display "$QEMU_DISPLAY" \
        -nic "user,model=virtio-net-pci,hostfwd=tcp:127.0.0.1:$port-:22" \
        -serial "file:$log" &
    local qemu=$!
    # EXIT, not RETURN: a failed assertion exits this stage's subshell
    # rather than returning, and a RETURN trap would never fire, leaving
    # a headless VM running after a failure.
    # shellcheck disable=SC2064
    trap "kill $qemu 2>/dev/null || true" EXIT

    # BatchMode: this stage calls ssh dozens of times with stderr thrown
    # away, so an auth failure must return rather than stop on a password
    # prompt. Without it a host with no ssh key turns the whole stage
    # interactive and the deadline below never gets to run.
    local ssh_opts=(-p "$port" -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null
                    -o ConnectTimeout=5 -o LogLevel=ERROR -o BatchMode=yes)
    # `kuma vm` writes this only when the host had no key of its own.
    [ -f "$dir/ssh-key" ] && ssh_opts+=(-i "$dir/ssh-key")
    ssh_opts+=(kuma@127.0.0.1)
    # shellcheck disable=SC2029  # client-side expansion is the point: every
    # caller builds the command here and wants the guest to run it literally.
    guest() { ssh "${ssh_opts[@]}" "$@" 2>/dev/null; }

    echo "   .. waiting for ssh on $port"
    local deadline=$((SECONDS + 420))
    until guest true; do
        kill -0 $qemu 2>/dev/null || bad "qemu died; console at $log"
        [ $SECONDS -lt $deadline ] || bad "no ssh within 420s; console at $log"
        sleep 5
    done
    ok "booted and reachable"

    # Let boot finish before judging it: first boot creates the user,
    # converges flatpaks and brew, and greenboot runs after all of it.
    echo "   .. waiting for the boot to settle"
    deadline=$((SECONDS + 600))
    until [[ "$(guest systemctl is-system-running)" =~ ^(running|degraded)$ ]]; do
        [ $SECONDS -lt $deadline ] || bad "boot never settled; console at $log"
        sleep 10
    done

    # The verdict, from the machine's own health check rather than from
    # anything this script knows: on a desktop image a green greenboot
    # means the greeter came up, which is the regression class that boots
    # "fine" into a black screen.
    local verdict
    verdict=$(guest systemctl is-active greenboot-healthcheck.service || true)
    [ "$verdict" = active ] || bad "greenboot verdict: $verdict (console at $log)"
    ok "greenboot says this boot is healthy"

    # systemd-remount-fs fails on installed systems over composefs
    # (Anaconda's fstab '/' line); doctor calls it known-benign and so
    # does this, but nothing else gets a pass.
    local failed
    failed=$(guest systemctl --failed --plain --no-legend \
             | awk '{print $1}' | grep -v '^systemd-remount-fs.service$' || true)
    [ -z "$failed" ] || bad "failed units: $(echo "$failed" | tr '\n' ' ')"
    ok "no failed units"

    local rpms
    rpms=$(declared_rpms "$file")
    if [ -n "$rpms" ]; then
        # shellcheck disable=SC2086
        guest rpm -q $rpms >/dev/null || bad "declared rpms missing: $rpms"
        ok "declared packages are installed"
    fi

    if grep -q '^desktop' "$file"; then
        [ "$(guest systemctl is-active display-manager.service)" = active ] \
            || bad "greeter is not running"
        ok "greeter is up"
    fi

    # Shutting down is not what this test is about, and the guest's test
    # user is in wheel, which needs a password sudo can't ask for over an
    # ssh session with no tty. So: ask nicely, wait a little, then take
    # the disposable VM out. Waiting on a graceful poweroff that can never
    # arrive is how this hung the first time it ran.
    guest sudo -n systemctl poweroff >/dev/null 2>&1 || true
    local waited=0
    while kill -0 $qemu 2>/dev/null && [ $waited -lt 30 ]; do
        sleep 1
        waited=$((waited + 1))
    done
    kill $qemu 2>/dev/null || true
    wait $qemu 2>/dev/null || true
    trap - EXIT
    ok "shut down"
}

# --- run ---------------------------------------------------------------
port=2300
for file in examples/*.toml; do
    name=$(basename "$file" .toml)
    if [ ${#SELECTED[@]} -gt 0 ] && ! printf '%s\n' "${SELECTED[@]}" | grep -qx "$name"; then
        continue
    fi
    tag="localhost/kuma-smoke-$name:latest"
    port=$((port + 1))

    note "$name"
    if (smoke_image "$file" "$tag" && { [ $BOOT -eq 0 ] || smoke_boot "$file" "$tag" "$name" "$port"; }); then
        PASS+=("$name")
    else
        FAIL+=("$name")
    fi

    if [ $KEEP -eq 0 ]; then
        podman rmi -f "$tag" >/dev/null 2>&1 || true
        # The lock goes too, so a local run resolves the current base like
        # CI's fresh checkout does. A pin left lying here would quietly
        # freeze the smoke tests against a base the world has moved past,
        # which is the one thing they exist to notice.
        rm -f "${file%.toml}.lock"
        [ -d "vm-smoke/$name" ] && sudo rm -rf "vm-smoke/$name"
    fi
done

note "summary"
[ ${#PASS[@]} -gt 0 ] && printf '   pass: %s\n' "${PASS[*]}"
if [ ${#FAIL[@]} -gt 0 ]; then
    printf '   FAIL: %s\n' "${FAIL[*]}"
    exit 1
fi
[ ${#PASS[@]} -gt 0 ] || { echo "   no examples matched" >&2; exit 2; }
echo "   all good"
