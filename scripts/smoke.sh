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

# One value out of the declaration, by dotted key, so every assertion
# below reads what the example actually asks for instead of a copy of it
# that can drift. Lists print space-separated; a true boolean prints
# "true" and a false or absent one prints nothing, so every caller can
# ask the same `[ -n ... ]` question of any key.
declared() {
    python3 -c '
import tomllib, sys
with open(sys.argv[1], "rb") as f:
    node = tomllib.load(f)
for key in sys.argv[2].split("."):
    node = node.get(key) if isinstance(node, dict) else None
if isinstance(node, bool):
    print("true" if node else "")
elif isinstance(node, list):
    print(" ".join(str(item) for item in node))
elif node is not None:
    print(node)
' "$1" "$2"
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
    rpms=$(declared "$file" packages.rpm)
    if [ -n "$rpms" ]; then
        # shellcheck disable=SC2086
        guest rpm -q $rpms >/dev/null || bad "declared rpms missing: $rpms"
        ok "declared packages are installed"
    fi

    # Everything above this line ran as the wrong account. `kuma vm` writes
    # a bib blueprint with a hardcoded `kuma` user (main.rs, vm_config), so
    # the account this stage logs in as is the disk builder's, created at
    # image-install time and owing nothing to the declaration. The declared
    # account is a different account, made at first boot by kuma-user-sync,
    # and until now nothing here ever looked at it: a declared shell or
    # group could have been wrong in every image kuma ever built and every
    # stage would still have passed.
    #
    # The blueprint account is what makes checking possible, though. None of
    # this needs to log in AS the declared user, so none of it needs a
    # password_hash in a committed example — it asks the machine about an
    # account from a shell it already has.
    local want_user
    want_user=$(declared "$file" user.name)
    if [ -z "$want_user" ]; then
        ok "no [user] declared, so nothing to converge"
    else
        guest getent passwd "$want_user" >/dev/null \
            || bad "kuma-user-sync never created $want_user"
        ok "declared user exists"

        local want_shell got_shell
        want_shell=$(declared "$file" user.shell)
        if [ -n "$want_shell" ]; then
            got_shell=$(guest getent passwd "$want_user" | cut -d: -f7)
            [ "$got_shell" = "/usr/bin/$want_shell" ] \
                || bad "declared shell /usr/bin/$want_shell, account has ${got_shell:-none}"
            ok "declared shell is the account's shell"
        fi

        # Groups are read from the image's own /usr/lib/kuma/user, not from
        # the declaration, because an absent `groups` key means the schema
        # default (wheel) while `groups = []` means none, and TOML gives
        # this script no way to tell those apart: both read as empty.
        # Re-deriving the default here would also put a second copy of it
        # in a file whose whole point is not holding copies. kuma-user is
        # what kuma-user-sync actually consumes, so this asks whether the
        # account matches what the machine was told, and leaves
        # declaration-to-kuma-user to the unit tests that own it.
        #
        # Parsed here rather than sourced over ssh: ssh joins its argv into
        # one string for a remote shell to re-parse, so quotes meant for
        # that shell are gone before it sees them. Every `guest` call in
        # this block keeps its metacharacters on the near side for that
        # reason.
        #
        # The file's existence is asserted first because `set -e` is off
        # for this whole stage (see bad()): an unreadable file would leave
        # the parse empty, the loop would run zero times, and this would
        # report success having checked nothing.
        guest test -f /usr/lib/kuma/user \
            || bad "no /usr/lib/kuma/user in the image for kuma-user-sync to read"
        local want_groups got_groups
        want_groups=$(guest cat /usr/lib/kuma/user | sed -n "s/^KUMA_GROUPS='\(.*\)'\$/\1/p")
        if [ -z "$want_groups" ]; then
            ok "no groups declared, so none to grant"
        else
            got_groups=$(guest id -nG "$want_user")
            for group in $want_groups; do
                case " $got_groups " in
                    *" $group "*) ;;
                    *) bad "$want_user is not in declared group $group (has: $got_groups)" ;;
                esac
            done
            ok "declared groups are granted"
        fi

        if [ -n "$(declared "$file" user.ssh_keys)" ]; then
            guest test -f "/etc/kuma/keys/$want_user" \
                || bad "declared ssh keys never reached /etc/kuma/keys/$want_user"
            ok "declared ssh keys are served"
        fi

        if [ -n "$(declared "$file" user.autologin)" ]; then
            # Two separate claims, and only the second one is the feature.
            # A greeter can be configured for autologin and still not
            # perform it: the COSMIC arm once wrote initial_session into a
            # file that greeter does not read, and asserting the config
            # alone would have called that a pass.
            #
            # Both greetd files are named because the arms write different
            # ones: niri generates config.toml wholesale, COSMIC appends to
            # the one cosmic-greeter.service reads. cat tolerates the
            # absent one.
            guest cat /etc/greetd/config.toml /etc/greetd/cosmic-greeter.toml \
                | grep -q "user = \"$want_user\"" \
                || bad "no greetd initial_session names $want_user"
            ok "greetd is configured to autologin $want_user"

            guest loginctl list-sessions --no-legend | awk '{print $3}' \
                | grep -qx "$want_user" \
                || bad "$want_user has no session, so autologin did not happen"
            ok "autologin put $want_user in a session"
        elif grep -q '^desktop' "$file"; then
            # Not silence: no committed example turns autologin on, so the
            # greetd path above is unexecuted rather than passing.
            ok "autologin not declared here, so that path is unchecked"
        fi
    fi

    # Every other check in this file drives kuma from the host, which is
    # how the image shipped for months with no kuma in it at all: the
    # declaration was baked, the units were enabled, the helpers were in
    # /usr/libexec, and nothing ever ran the binary from inside a machine.
    # `generate` is the cheapest verb that needs both halves — a runnable
    # binary and the baked-declaration fallback a machine with no working
    # copy depends on, which is what docs/agents.md promises.
    guest kuma --version >/dev/null || bad "the image ships no runnable kuma"
    guest kuma generate | grep -q '^FROM ' \
        || bad "kuma on the machine cannot read its baked declaration"
    ok "the machine can run its own kuma"

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

    # The boot stage builds from the example plus a [user] block, not from
    # the example as committed.
    #
    # Every committed example leaves [user] commented out, deliberately: a
    # declared account is a property of the image, so it rides into any
    # media built from that file, password hash included. That safety costs
    # the one thing the boot stage most needs to check. The account is made
    # at first boot by kuma-user-sync, so a wrong shell or a missing group
    # could ship in every image kuma builds and every assertion here would
    # print "no [user] declared, so nothing to converge" and pass.
    #
    # Appending rather than editing keeps this honest about what it tests:
    # the file is the example, plus exactly the block being exercised. It
    # costs no extra build, since --boot already runs the image stage.
    #
    # bash, not the example's own shell: /usr/bin/fish is only in the
    # desktop sets, and a declared shell emits a build-time `test -x`
    # guard that would fail the minimal image. No password_hash, because
    # nothing here logs in as this account; it asks the machine about it
    # from the shell `kuma vm` already provides.
    if [ $BOOT -eq 1 ]; then
        booted_file="vm-smoke/$name.toml"
        mkdir -p vm-smoke
        cp "$file" "$booted_file"
        cat >>"$booted_file" <<'EOF'

# Appended by scripts/smoke.sh so the boot stage has an account to check.
[user]
name = "smoketest"
shell = "bash"
groups = ["wheel"]
ssh_keys = ["ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIsmoketestnotarealkey smoke@kuma"]
EOF
        file=$booted_file
    fi

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
        [ $BOOT -eq 1 ] && rm -f "vm-smoke/$name.toml"
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
