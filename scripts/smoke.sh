#!/usr/bin/env bash
# kuma smoke tests: build every committed example, and optionally boot it.
#
# The promise under test is the one the stability plan opens with: a
# declaration that validates either becomes a running system matching it,
# or fails loudly. `cargo test` already checks what can be checked without
# a machine (every example compiles to an image that keeps kuma's floor);
# this is the part that needs real podman, real bootc, and a real boot.
#
# Five stages, cheapest first:
#
#   check     parse and validate the declaration          (no podman)
#   image     build it and inspect the built layers       (podman)
#   install   write an encrypted disk and verify it       (podman + sudo)
#   boot      make a disk, boot it headless, ask the      (podman + kvm + sudo)
#             machine whether the boot was healthy
#   published install an image kuma published and boot    (podman + kvm + sudo
#             the disk that install wrote                  + sshpass + network)
#
# The boot stage's verdict is greenboot's own: the same check that decides
# whether this machine would roll an update back is the one that decides
# whether the test passed. On a desktop image that includes reaching the
# greeter, so "boots fine into a black screen" fails here rather than on
# your laptop.
#
# Usage:
#   scripts/smoke.sh                   # check + image, every example
#   scripts/smoke.sh --boot            # check, image and boot, every example
#   scripts/smoke.sh --install         # check, image and install
#   scripts/smoke.sh --boot minimal    # just one, by example name
#   scripts/smoke.sh --keep            # leave images and disks behind
#   scripts/smoke.sh --published ghcr.io/letdown2491/kuma:niri
#
# --published builds nothing and reads no example: it installs what is on
# the registry and boots the result, so it is the only stage that can go
# red without anyone having committed anything.
#
# --install is separate from --boot rather than another step of it: it is
# the only stage that needs no KVM, and it answers a different question.
# Boot asks whether a machine works; install asks whether the disk it was
# written onto is the one that was described.
#
# Env: KUMA (default target/debug/kuma), QEMU_DISPLAY (default egl-headless),
#      QEMU_VGA (default virtio-vga-gl).
set -euo pipefail

cd "$(dirname "$0")/.."

KUMA=${KUMA:-target/debug/kuma}
# Headless but GL-capable: a compositor needs a DRM device with a working
# GBM allocator, so -display none is not enough for a desktop image.
# LIBGL_ALWAYS_SOFTWARE keeps guest GL work on llvmpipe, out of the host's
# GPU driver, where a bad guest submission could otherwise take the host
# session down with it.
#
# The device is a variable for one reason: egl-headless needs a DRM render
# node on the HOST, and a CI runner has no GPU. virtio-vga without -gl
# still gives the guest a virtio-gpu DRM device, so the question a machine
# without a host GPU has to answer is whether the guest can allocate
# through it on llvmpipe alone.
QEMU_DISPLAY=${QEMU_DISPLAY:-egl-headless}
QEMU_VGA=${QEMU_VGA:-virtio-vga-gl}
BOOT=0
INSTALL=0
PUBLISHED=""
UPGRADE_TO=""
KEEP=0
SELECTED=()

while [ $# -gt 0 ]; do
    case "$1" in
        --boot) BOOT=1 ;;
        --install) INSTALL=1 ;;
        --published) PUBLISHED=${2:?--published needs an image reference}; shift ;;
        --upgrade-to) UPGRADE_TO=${2:?--upgrade-to needs an image reference}; shift ;;
        --keep) KEEP=1 ;;
        -h|--help) sed -n '2,36p' "$0"; exit 0 ;;
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

    # A base already in local storage is the one podman builds on, however
    # old it is, and the lock then records that digest while `update
    # --check` below asks the registry. The two disagree the moment Fedora
    # pushes a new base, which is true news and not what that assertion is
    # about. CI never meets this because a fresh runner has nothing local
    # to be stale; a laptop that has been building for a month always does.
    if grep -q '^base *=' "$file"; then
        local base
        base=$(sed -n 's/^ *base *= *"\(.*\)".*/\1/p' "$file" | head -1)
        [ -n "$base" ] || bad "cannot read system.base out of $file"
        echo "   .. pulling $base so the build and the registry agree"
        podman pull -q "$base" >/dev/null || bad "cannot pull $base"
    fi

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
        # Reported with what it actually said. This failed once for a
        # reason the message could not express (a base that had genuinely
        # moved, before the pull above existed), and a failure that names
        # only its own assertion sends somebody looking in the wrong place.
        local checked
        checked=$("$KUMA" --config "$file" update --check 2>&1 || true)
        case "$checked" in
            *"is current"*) ;;
            *) bad "update --check disagrees with the lock this build just wrote: $checked" ;;
        esac
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

# --- stage: install ----------------------------------------------------
# What `kuma install` writes, checked on the disk rather than by booting
# it. Booting proves a machine works; this proves the one thing a boot
# cannot report, because a disk written wrongly is not recoverable by the
# person holding it: the container opens with the passphrase that was
# typed, and the bootloader asks the initramfs to unlock the container
# that is actually there. Get either wrong and the install succeeds, the
# machine is unbootable, and whatever was on that disk is gone.
#
# Encrypted only. The encrypted path is a superset: it writes the same
# table, the same filesystems and the same account file, plus a container
# and a karg. Running both would double the slowest stage here to check
# the same partition table twice.
smoke_install() {
    local file=$1 tag=$2 name=$3
    local dir="vm-smoke/$name-install"
    local raw="$dir/disk.raw"
    local pass="smoke-passphrase"
    local user="smoketest"

    mkdir -p "$dir"
    rm -f "$raw"
    # Sparse, so this costs no disk until the install fills it, and above
    # the 16G floor partition::plan refuses below.
    truncate -s 24G "$raw"

    # Two lines on stdin, in the order the interview asks: the disk
    # passphrase, then the account password. Neither is ever a flag.
    #
    # --update-from because the image being installed is a localhost tag,
    # which kuma refuses to record as an update source: on the installed
    # machine `localhost` means itself. The reference here is never
    # fetched by this test; it exists because the installed machine has to
    # record somewhere real to update from.
    echo "   .. installing to a disk image (needs sudo; this is the slow part)"
    printf '%s\n%s\n' "$pass" "$pass" \
        | "$KUMA" install --disk "$raw" --image "$tag" \
            --update-from ghcr.io/example/kuma:niri \
            --user "$user" --encrypt --yes >/dev/null \
        || bad "install failed"
    ok "installed"

    # Everything below reads the disk as root through a loop device, and
    # unwinds in the order it was set up. EXIT rather than RETURN for the
    # same reason the boot stage uses it: a failed assertion exits this
    # subshell and a RETURN trap would never fire, leaving a loop device
    # and an open mapper behind to confuse the next run.
    local loop mnt="$dir/mnt" mapper="kuma-smoke-$name"
    loop=$(sudo losetup -fP --show "$raw") || bad "cannot attach $raw"
    # shellcheck disable=SC2064
    trap "sudo umount -R '$mnt' 2>/dev/null || true
          sudo cryptsetup close '$mapper' 2>/dev/null || true
          sudo losetup -d '$loop' 2>/dev/null || true" EXIT
    mkdir -p "$mnt"

    sudo cryptsetup isLuks "${loop}p3" || bad "the root partition holds no LUKS container"
    printf '%s' "$pass" \
        | sudo cryptsetup luksOpen --test-passphrase --key-file - "${loop}p3" \
        || bad "the passphrase that was typed does not open the container"
    ok "the root partition is LUKS and opens with the passphrase"

    # The karg names the container's own UUID. The mapper reports the
    # UUID of the filesystem inside it, which is a real number that
    # unlocks nothing, and the difference is invisible until a machine
    # boots to an initramfs waiting for a device that will never appear.
    local luks_uuid
    luks_uuid=$(sudo blkid -s UUID -o value "${loop}p3")
    [ -n "$luks_uuid" ] || bad "no LUKS UUID on the root partition"
    sudo mount "${loop}p2" "$mnt" || bad "cannot mount /boot"
    sudo grep -rq "rd.luks.uuid=luks-$luks_uuid" "$mnt/loader/entries" \
        || bad "no boot entry unlocks luks-$luks_uuid"
    ok "the bootloader unlocks the container that is there"
    sudo umount "$mnt"

    # And the answers the installer was given, inside the container.
    printf '%s' "$pass" | sudo cryptsetup open --key-file - "${loop}p3" "$mapper" \
        || bad "cannot open the container"
    sudo mount -o subvol=root "/dev/mapper/$mapper" "$mnt" || bad "cannot mount the root"

    # Neither /var nor /etc is where it looks. This is an ostree
    # deployment: the subvolume holds /ostree/deploy/<stateroot>/var, and
    # the merged /etc lives inside the deployment directory under a
    # checksum nobody can predict. Naming them as if the subvolume were
    # the root is how this assertion read correctly and failed the first
    # time it ran.
    local user_file
    user_file=$(sudo find "$mnt/ostree/deploy" -maxdepth 5 -path '*/var/lib/kuma/user' -print -quit)
    [ -n "$user_file" ] || bad "no /var/lib/kuma/user on the installed root"
    sudo grep -q "KUMA_USER='$user'" "$user_file" \
        || bad "the installer's account file does not name $user"
    ok "the account to converge is written where kuma-user-sync reads it"

    local host_file
    host_file=$(sudo find "$mnt/ostree/deploy" -maxdepth 5 -path '*/var/lib/kuma/hostname' -print -quit)
    [ -n "$host_file" ] || bad "no /var/lib/kuma/hostname for first boot to apply"
    ok "the hostname to apply is written beside it"

    # No /var/home assertion here on purpose: the image ships none, and
    # tmpfiles creates it at first boot. Whether it is a subvolume is a
    # question for a booted machine, and smoke_boot asks it.

    # The greeter must not autologin an account this machine will not
    # have. A committed example declares no [user], so there is nothing to
    # strip here and the assertion is that nothing crept in.
    local greetd_conf
    greetd_conf=$(sudo find "$mnt/ostree/deploy" -maxdepth 6 \
                  -path '*/etc/greetd/config.toml' -print -quit)
    if [ -n "$greetd_conf" ]; then
        local autologin
        autologin=$(sudo sed -n 's/^user *= *"\(.*\)"/\1/p' "$greetd_conf" | tail -1)
        case "$autologin" in
            ""|greetd|"$user") ok "no greeter autologins an account this disk lacks" ;;
            *) bad "greetd autologins '$autologin', which this machine has no account for" ;;
        esac
    else
        # Out loud, not skipped. A headless image ships no greeter config,
        # so this says nothing about the case the check exists for, and a
        # silent pass would read as if it had.
        ok "no greeter on this image, so that path is unchecked here"
    fi

    sudo umount -R "$mnt"
    sudo cryptsetup close "$mapper"
    sudo losetup -d "$loop"
    trap - EXIT
    [ $KEEP -eq 1 ] || rm -f "$raw"
    ok "disk verified"
}

# --- stage: published --------------------------------------------------
#
# Installs an image kuma published, then boots the disk that install
# wrote. Every other stage here builds its own image from the tree, so
# this is the only one that asks whether what is on the registry works,
# and the only one that can fail because of something nobody committed.
# It lives behind its own flag and its own workflow for that reason.
#
# It also reaches a branch nothing else does. `smoke_boot` already asks
# whether /var/home is its own btrfs subvolume, and on a
# bootc-image-builder disk it never can be: those roots are ext4 (see
# BIB_ROOTFS), so the check correctly says the question does not arise
# and has therefore never once run in anger. **Only `kuma install` writes
# btrfs**, so booting an installed disk is the only way that assertion
# executes, and `kuma-home-subvol` is the only thing standing between
# `[snapshots]` and an hourly timer that snapshots nothing.
#
# Deliberately NOT encrypted, and this is a constraint rather than an
# omission: an encrypted root stops at boot for a passphrase, and nothing
# is there to type it. The encrypted path is covered by `smoke_install`,
# which reads the disk through a loop device instead of booting it.
smoke_published() {
    local image=$1 name=$2 port=$3
    local dir="vm-smoke/$name"
    local raw="$dir/disk.raw"
    local log="$dir/console.log"
    local user="smoketest"
    local pass="smoke-account-password"

    mkdir -p "$dir"
    rm -f "$raw"
    truncate -s 24G "$raw"

    # --update-from only when the machine is meant to move somewhere else
    # later. It is the flag that says "install this, but track that", and
    # until now it was exercised only with a reference nothing ever
    # fetched, so this is the first time it points at something real.
    local update_from=()
    [ -n "$UPGRADE_TO" ] && update_from=(--update-from "$UPGRADE_TO")

    # One line on stdin, not two: without --encrypt the interview never
    # asks for a disk passphrase, and only the account password is left.
    echo "   .. installing $image (needs sudo; this is the slow part)"
    printf '%s\n' "$pass" \
        | "$KUMA" install --disk "$raw" --image "$image" \
            "${update_from[@]}" \
            --user "$user" --hostname smoketest --yes >/dev/null \
        || bad "installing $image failed"
    ok "installed $image"

    # UEFI firmware, and this is not optional. `kuma install` writes a GPT
    # with an ESP, which is a UEFI layout; qemu defaults to SeaBIOS, which
    # finds nothing bootable and says so on the VGA console that
    # `-display none` throws away. The failure is therefore completely
    # silent: the install succeeds, the guest never boots, ssh times out
    # after seven minutes and the serial log is zero bytes. Nothing needed
    # this before because the boot stage's disks come from
    # bootc-image-builder and boot under BIOS.
    #
    # VARS is copied because pflash wants it writable, and a per-run copy
    # means EFI boot entries cannot leak from one run into the next.
    # The _4M names are Ubuntu's and are listed first because that is what
    # CI runs; the unsuffixed pair is Fedora's. VARS is derived from CODE
    # by name rather than searched for separately, because the two have to
    # be the same build: a 4M vars file against 2M code does not boot.
    local ovmf_code="" ovmf_vars="" candidate
    for candidate in /usr/share/OVMF/OVMF_CODE_4M.fd /usr/share/OVMF/OVMF_CODE.fd \
                     /usr/share/edk2/ovmf/OVMF_CODE.fd /usr/share/qemu/OVMF_CODE.fd; do
        [ -f "$candidate" ] && ovmf_code=$candidate && break
    done
    if [ -z "$ovmf_code" ]; then
        echo "   .. looked for OVMF in:" >&2
        ls -1 /usr/share/OVMF /usr/share/edk2/ovmf /usr/share/qemu 2>/dev/null >&2 || true
        bad "no OVMF firmware; an installed disk is UEFI and will not boot on SeaBIOS"
    fi
    ovmf_vars=${ovmf_code//CODE/VARS}
    [ -f "$ovmf_vars" ] || bad "found $ovmf_code but no $ovmf_vars beside it"
    cp "$ovmf_vars" "$dir/OVMF_VARS.fd"

    qemu-system-x86_64 \
        -enable-kvm -cpu host -smp 4 -m 4096 \
        -machine q35 \
        -drive "if=pflash,format=raw,readonly=on,file=$ovmf_code" \
        -drive "if=pflash,format=raw,file=$dir/OVMF_VARS.fd" \
        -drive "file=$raw,if=virtio,format=raw" \
        -device "$QEMU_VGA" -display "$QEMU_DISPLAY" \
        -nic "user,model=virtio-net-pci,hostfwd=tcp:127.0.0.1:$port-:22" \
        -serial "file:$log" &
    local qemu=$!
    # EXIT rather than RETURN, for the reason smoke_boot gives: a failed
    # assertion leaves this subshell without ever returning.
    # shellcheck disable=SC2064
    trap "kill $qemu 2>/dev/null || true" EXIT

    # Password auth, because `kuma install` has no way to plant a key:
    # the account it creates exists only on the installed machine and
    # nothing has ever logged into it. PubkeyAuthentication=no keeps a
    # runner's own agent from being offered first and eating the attempt.
    local ssh_opts=(-p "$port" -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null
                    -o ConnectTimeout=5 -o LogLevel=ERROR
                    -o PubkeyAuthentication=no -o PreferredAuthentications=password
                    "$user@127.0.0.1")
    # shellcheck disable=SC2029  # client-side expansion is the point.
    guest() { sshpass -p "$pass" ssh "${ssh_opts[@]}" "$@" 2>/dev/null; }

    # sudo over ssh has no terminal to ask on, and this account is in
    # wheel rather than NOPASSWD, so the password goes in on stdin. Same
    # shape `kuma vm --apply` already uses against its own guests. `-p ''`
    # drops the prompt, which would otherwise land in the output being
    # parsed. The remote side re-parses one string, so the pipeline is
    # built here as one argument rather than passed as words.
    gsudo() { guest "echo '$pass' | sudo -S -p '' $*"; }

    # Parsed on this side, never in the guest: anything with quotes in it
    # loses them crossing ssh, and a python one-liner is all quotes.
    booted_digest() {
        gsudo bootc status --format json \
            | python3 -c 'import sys,json; print(json.load(sys.stdin)["status"]["booted"]["image"]["imageDigest"])' \
            2>/dev/null || true
    }

    echo "   .. waiting for ssh on $port"
    local deadline=$((SECONDS + 420))
    until guest true; do
        kill -0 $qemu 2>/dev/null || bad "qemu died; console at $log"
        [ $SECONDS -lt $deadline ] || bad "no ssh within 420s; console at $log"
        sleep 5
    done
    ok "installed machine booted and is reachable"

    echo "   .. waiting for the boot to settle"
    deadline=$((SECONDS + 600))
    until [[ "$(guest systemctl is-system-running)" =~ ^(running|degraded)$ ]]; do
        [ $SECONDS -lt $deadline ] || bad "boot never settled; console at $log"
        sleep 10
    done

    local verdict
    verdict=$(guest systemctl is-active greenboot-healthcheck.service || true)
    [ "$verdict" = active ] || bad "greenboot verdict: $verdict (console at $log)"
    ok "greenboot says this boot is healthy"

    # The reason this stage exists. A `kuma install` root is btrfs, so
    # "not btrfs" is a failure rather than a question that does not arise.
    local home_fs home_inode home_was_subvol=no
    home_fs=$(guest findmnt -no FSTYPE -T /var/home)
    [ "$home_fs" = btrfs ] || bad "/var/home is $home_fs on an installed disk; expected btrfs"
    home_inode=$(guest stat -c %i /var/home)
    [ "$home_inode" = 256 ] && home_was_subvol=yes

    # Strict only when the image under test is meant to have the
    # converger. In the cross-version path the point is to install a
    # version from BEFORE a fix and see what upgrading does about it, so
    # its absence is the premise rather than the failure. Reported either
    # way, because a silent skip here would read as a pass.
    # A converger that ran and correctly declined is a different fault
    # from one that died, and `systemctl is-system-running` calls both
    # "degraded", which this stage accepts as settled. So ask about this
    # unit by name rather than letting a failure hide in the aggregate.
    [ "$(guest systemctl is-failed kuma-home-subvol.service)" != failed ] \
        || bad "kuma-home-subvol.service failed; console at $log"

    if [ -z "$UPGRADE_TO" ]; then
        if [ "$home_was_subvol" != yes ]; then
            # Say why, not just that. This has come back intermittently on
            # the same published image (two subvolumes and one directory
            # across three boots), and the guest's kernel log never
            # reaches the serial console because the installed image sets
            # no console= karg, so the evidence has to be collected here
            # while the machine is still up.
            echo "   .. kuma-home-subvol.service says:" >&2
            guest systemctl --no-pager -l status kuma-home-subvol.service >&2 2>&1 || true
            guest journalctl --no-pager -b -u kuma-home-subvol.service >&2 2>&1 || true
            echo "   .. /var/home contains:" >&2
            guest ls -A /var/home >&2 2>&1 || true
            echo "   .. units that finished before it:" >&2
            guest systemd-analyze critical-chain kuma-home-subvol.service >&2 2>&1 || true
            bad "/var/home is not a subvolume (inode $home_inode); snapshots would take nothing"
        fi
        ok "/var/home is its own subvolume"
    elif [ "$home_was_subvol" = yes ]; then
        ok "/var/home is its own subvolume before upgrading"
    else
        ok "/var/home is NOT a subvolume on $image (inode $home_inode), which is what upgrading is being asked about"
    fi

    # The account the installer was told to make, on the machine it made
    # it on. Nothing before this stage has booted a disk whose user came
    # from the install interview rather than from a declaration.
    guest id "$user" >/dev/null || bad "$user does not exist on the installed machine"
    guest id -nG "$user" | tr ' ' '\n' | grep -qx wheel || bad "$user is not in wheel"
    [ "$(guest hostnamectl hostname)" = smoketest ] || bad "hostname did not converge"
    ok "the installed account and hostname converged"

    # --- the cross-version half ----------------------------------------
    #
    # Nothing has ever checked that a machine installed at one version can
    # reach a later one. Every other stage builds and boots a single
    # image, so "does an existing machine survive moving forward" has been
    # a promise rather than a result, and it is the promise 1.0 rests on.
    #
    # bootc, not `kuma update`: kuma's own update_check says so in as many
    # words. `kuma update` is the builder's verb, which pulls a base and
    # rebuilds; a machine running a published image asks bootc to re-pull
    # its origin, and --update-from above is what pointed that origin at
    # something newer than what was installed.
    if [ -n "$UPGRADE_TO" ]; then
        local before after
        before=$(booted_digest)
        [ -n "$before" ] || bad "could not read the booted digest before upgrading"

        echo "   .. upgrading to $UPGRADE_TO (pulling inside the guest)"
        gsudo bootc upgrade >/dev/null 2>&1 \
            || bad "bootc upgrade failed; console at $log"

        # Staged, or there is nothing to reboot into and a green reboot
        # below would mean nothing at all.
        gsudo bootc status | grep -qiE '^  Staged|staged image' \
            || bad "nothing staged after bootc upgrade; the origin may not have moved"
        ok "the newer image staged"

        echo "   .. rebooting into it"
        gsudo systemctl reboot >/dev/null 2>&1 || true
        # Let it actually go down before waiting for it to come back, or
        # the first successful ssh is to the machine that is still
        # shutting down and every assertion runs against the old boot.
        local gone=0
        while guest true 2>/dev/null && [ $gone -lt 60 ]; do sleep 2; gone=$((gone + 2)); done

        deadline=$((SECONDS + 420))
        until guest true; do
            kill -0 $qemu 2>/dev/null || bad "qemu died during reboot; console at $log"
            [ $SECONDS -lt $deadline ] || bad "no ssh within 420s after upgrade; console at $log"
            sleep 5
        done

        deadline=$((SECONDS + 600))
        until [[ "$(guest systemctl is-system-running)" =~ ^(running|degraded)$ ]]; do
            [ $SECONDS -lt $deadline ] || bad "boot never settled after upgrade; console at $log"
            sleep 10
        done

        verdict=$(guest systemctl is-active greenboot-healthcheck.service || true)
        [ "$verdict" = active ] || bad "greenboot verdict after upgrade: $verdict (console at $log)"
        ok "the upgraded machine boots and greenboot says it is healthy"

        after=$(booted_digest)
        [ -n "$after" ] || bad "could not read the booted digest after upgrading"
        [ "$before" != "$after" ] \
            || bad "still booted on $before after the upgrade; nothing actually moved"
        ok "moved from ${before:0:19} to ${after:0:19}"

        # The half that a reboot alone would not catch. /var is shared
        # across deployments by design, so an image can move forward and
        # leave the machine's own state behind.
        guest id "$user" >/dev/null || bad "$user did not survive the upgrade"
        ok "the account survived the version jump"

        # Whether a fix that ships in the newer image reaches a machine
        # that was installed before it. `kuma-home-subvol` only acts while
        # /var/home is empty, and after a first boot it never is, so an
        # older machine cannot acquire the subvolume by upgrading. Never a
        # silent pass in either direction: losing it is a regression, and
        # not gaining it is the finding this job exists to produce.
        local home_now=no
        [ "$(guest stat -c %i /var/home)" = 256 ] && home_now=yes
        if [ "$home_was_subvol" = yes ]; then
            [ "$home_now" = yes ] || bad "/var/home stopped being a subvolume across the upgrade"
            ok "/var/home is still its own subvolume"
        elif [ "$home_now" = yes ]; then
            ok "upgrading turned /var/home into a subvolume"
        else
            ok "/var/home is still not a subvolume after upgrading, so [snapshots] on this machine would take nothing"
        fi

        # And now ask the machine to grade itself, rather than trusting
        # this script's reading of an inode. doctor already owns this
        # knowledge: check_snapshots fails a machine whose target is a
        # directory, with "the timer runs and takes nothing". Checking
        # that its verdict agrees with the filesystem turns that comment
        # into a result, and would catch doctor going quiet about a
        # machine that is still broken.
        local snapshots_grade
        snapshots_grade=$(gsudo kuma doctor --json \
            | python3 -c 'import sys,json; print(next((c["grade"] for c in json.load(sys.stdin)["checks"] if c["name"]=="snapshots"), "absent"))' \
            2>/dev/null || true)
        case "$home_now:$snapshots_grade" in
            yes:ok)   ok "doctor agrees the snapshot target is usable" ;;
            no:fail)  ok "doctor fails this machine's snapshot check, which is the correct verdict" ;;
            *:absent) bad "doctor reported no snapshots check; the declaration should have enabled it" ;;
            *)        bad "doctor says snapshots is '$snapshots_grade' while /var/home is-a-subvolume=$home_now" ;;
        esac
    fi

    echo "   .. powering off"
    gsudo systemctl poweroff >/dev/null 2>&1 || true
    local waited=0
    while kill -0 $qemu 2>/dev/null && [ $waited -lt 30 ]; do sleep 1; waited=$((waited + 1)); done
    kill $qemu 2>/dev/null || true
    wait $qemu 2>/dev/null || true
    trap - EXIT
    [ $KEEP -eq 1 ] || sudo rm -rf "$dir"
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
        -device "$QEMU_VGA" -display "$QEMU_DISPLAY" \
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

    # systemd-remount-fs fails on a machine whose fstab still declares the
    # root Anaconda wrote, because composefs cannot remount it. Excused by
    # that cause and not by the unit's name, which is the same narrowing
    # doctor got: kuma-fstab-sync comments the line out on first boot, and
    # a machine kuma installed never had one, so after that a failure of
    # this unit is news like anything else. Excusing it by name meant this
    # stage could never report it again.
    #
    # The fstab is read here rather than parsed in the guest, so the awk
    # program is not going through ssh's word splitting. Matching the
    # mount point as a field is deliberate, and the same rule the
    # converger follows: `root` is a subvolume name on every Anaconda
    # btrfs install, so a line regex matches things that are not the root.
    local failed
    failed=$(guest systemctl --failed --plain --no-legend | awk '{print $1}')
    if guest cat /etc/fstab | awk '$1 !~ /^#/ && $2 == "/" { found = 1 }
                                   END { exit !found }'; then
        failed=$(printf '%s\n' "$failed" | grep -v '^systemd-remount-fs.service$' || true)
    fi
    [ -z "$failed" ] || bad "failed units: $(echo "$failed" | tr '\n' ' ')"
    ok "no failed units"

    # /var/home has to be its own btrfs subvolume, or `[snapshots]` is a
    # timer that runs hourly and takes nothing: a snapshot is of a
    # subvolume, the script exits 0 on a target that is not one, and the
    # machine reports itself healthy throughout. kuma-home-subvol makes
    # it one on the first boot, while it is still empty, so a booted
    # machine is the only place the answer exists.
    #
    # Conditional, and the condition is not a hedge: these disks are
    # built by bootc-image-builder with an ext4 root (see BIB_ROOTFS),
    # where there is no subvolume to make and the converger is right to
    # do nothing. Said out loud rather than skipped, because a silent
    # pass here would read as if the btrfs case had been checked, and on
    # this harness it never is: only `kuma install` writes btrfs.
    local home_fs home_inode
    home_fs=$(guest findmnt -no FSTYPE -T /var/home)
    if [ "$home_fs" = btrfs ]; then
        home_inode=$(guest stat -c %i /var/home)
        [ "$home_inode" = 256 ] \
            || bad "/var/home is not a subvolume (inode $home_inode); snapshots would take nothing"
        ok "/var/home is its own subvolume"
    else
        ok "/var/home is on $home_fs, so the subvolume question does not arise here"
    fi

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

# The published stage answers a question about the registry, not about
# the examples, so it runs on its own and returns rather than joining the
# loop below. Nothing here builds an image.
if [ -n "$PUBLISHED" ]; then
    note "published: $PUBLISHED"
    if (smoke_published "$PUBLISHED" published "$port"); then
        PASS+=("published")
    else
        FAIL+=("published")
    fi
    note "summary"
    [ ${#PASS[@]} -gt 0 ] && printf '   pass: %s\n' "${PASS[*]}"
    if [ ${#FAIL[@]} -gt 0 ]; then
        printf '   FAIL: %s\n' "${FAIL[*]}"
        exit 1
    fi
    printf '\n   all good\n'
    exit 0
fi

for file in examples/*.toml; do
    name=$(basename "$file" .toml)
    if [ ${#SELECTED[@]} -gt 0 ] && ! printf '%s\n' "${SELECTED[@]}" | grep -qx "$name"; then
        continue
    fi
    tag="localhost/kuma-smoke-$name:latest"
    port=$((port + 1))
    example_file=$file

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
    # The committed example, never the boot stage's copy: the account an
    # install creates is the installer's answer, and building from a
    # declaration that already names one would test the case where the two
    # agree, which is the case that was never broken.
    if (smoke_image "$file" "$tag" \
        && { [ $INSTALL -eq 0 ] || smoke_install "$example_file" "$tag" "$name"; } \
        && { [ $BOOT -eq 0 ] || smoke_boot "$file" "$tag" "$name" "$port"; }); then
        PASS+=("$name")
    else
        FAIL+=("$name")
    fi

    if [ $KEEP -eq 0 ]; then
        podman rmi -f "$tag" >/dev/null 2>&1 || true
        # And root's copy, which is a different store with the same tag.
        # `kuma vm` syncs the image there for bootc-image-builder and
        # nothing ever took it away, so tags from old runs sat in root
        # storage for days. That is not only 7GB of nobody's business: an
        # install resolves this tag against root's store, so a stale copy
        # there means a stage can pass having installed an image from
        # last week. It did, before this line existed.
        sudo podman rmi -f "$tag" >/dev/null 2>&1 || true
        # The lock goes too, so a local run resolves the current base like
        # CI's fresh checkout does. A pin left lying here would quietly
        # freeze the smoke tests against a base the world has moved past,
        # which is the one thing they exist to notice.
        rm -f "${file%.toml}.lock"
        [ -d "vm-smoke/$name" ] && sudo rm -rf "vm-smoke/$name"
        [ -d "vm-smoke/$name-install" ] && sudo rm -rf "vm-smoke/$name-install"
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
