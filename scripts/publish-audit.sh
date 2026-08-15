#!/usr/bin/env bash
# Fail if an image is not safe to publish.
#
# A published image is pulled by strangers, so it must carry no identity:
# not the account of whoever built it, not their hostname, not their ssh
# keys, and not the paths of their home directory. Most of that follows
# from building it from a declaration with no [user], but not all of it,
# which is the reason this is a check and not a rule in a document.
#
# Usage: scripts/publish-audit.sh [image]   (default localhost/kuma:latest)
#
# Mounting an image needs a user namespace, so the script re-execs itself
# under `podman unshare` when it is not already root. Reading the files
# from a *running* container would be easier and wrong: podman bind-mounts
# its own /etc/hostname over the image's, so the one file most likely to
# carry a machine's name is the one a `podman run` cannot show you.
set -euo pipefail

if [ -z "${KUMA_AUDIT_INNER:-}" ] && [ "$(id -u)" -ne 0 ]; then
    exec env KUMA_AUDIT_INNER=1 podman unshare "$0" "$@"
fi

image=${1:-localhost/kuma:latest}
failures=0

ok() { printf 'ok    %s\n' "$1"; }
bad() {
    printf 'FAIL  %s\n' "$1"
    [ $# -gt 1 ] && printf '      %s\n' "$2"
    failures=$((failures + 1))
}

mnt=$(podman image mount "$image")
cleanup() { podman image umount "$image" >/dev/null 2>&1 || true; }
trap cleanup EXIT

decl="$mnt/usr/lib/kuma/kuma.toml"

# The declaration is baked world-readable so that `kuma init` and the
# passwordless probe can read it. That is a deliberate tradeoff for a
# personal image and exactly why a published one must declare no account:
# publishing the declaration publishes the password hash with it.
if [ ! -f "$decl" ]; then
    bad "no baked declaration at /usr/lib/kuma/kuma.toml" "not a kuma image?"
else
    if grep -qE '^\s*\[user\]' "$decl"; then
        bad "the baked declaration has a [user] section" \
            "build from a declaration with no [user]; examples/niri.toml is one"
    else
        ok "baked declaration declares no [user]"
    fi
    if grep -qE '^\s*hostname\s*=' "$decl"; then
        bad "the baked declaration pins a hostname" \
            "a published image must not carry the builder's machine name"
    else
        ok "baked declaration pins no hostname"
    fi
fi

# Written only when [user] is declared, and 0600 rather than 0644, which
# makes it the easiest of these to forget: it does not show up in a
# world-readable file listing.
if [ -e "$mnt/usr/lib/kuma/user" ]; then
    bad "/usr/lib/kuma/user exists" "it carries KUMA_USER and KUMA_PASSWORD_HASH"
else
    ok "no baked user declaration"
fi

# kuma's default when the declaration pins nothing. Any other value came
# from a declaration and names somebody's machine.
host=$(cat "$mnt/etc/hostname" 2>/dev/null || echo "<missing>")
if [ "$host" = "kuma" ]; then
    ok "hostname is the default (kuma)"
else
    bad "hostname is '$host', not the default" "it names the machine that built this"
fi

# Autologin is a property of the image, so it rides into every machine
# installed from it, naming an account those machines will not have.
for greeter in etc/greetd/config.toml etc/greetd/cosmic-greeter.toml; do
    [ -f "$mnt/$greeter" ] || continue
    if grep -q 'initial_session' "$mnt/$greeter"; then
        bad "/$greeter has an initial_session" "a published image must not autologin"
    else
        ok "/$greeter has no autologin"
    fi
done

if [ -d "$mnt/etc/kuma/keys" ] && [ -n "$(ls -A "$mnt/etc/kuma/keys" 2>/dev/null)" ]; then
    bad "/etc/kuma/keys is not empty" "declared ssh public keys are baked in"
else
    ok "no baked ssh keys"
fi

# The one leak a sanitized declaration does not fix. Rust records source
# paths for panic messages, so a binary built in someone's home carries
# that path forever. `grep -a` rather than `strings`, which the image has
# no reason to ship.
#
# linuxbrew is excluded because it is not a person: kuma writes
# Homebrew's fixed install prefix into brew-profile.sh, the sync unit's
# ConditionPathExists, and the shell profiles, so those strings are the
# program's own content and identical on every machine. Without the
# exclusion this check fails on every kuma binary ever built, which is
# how a gate that cries wolf stops being read.
if [ -f "$mnt/usr/bin/kuma" ]; then
    if paths=$(grep -aoE '/(var/)?home/[a-z_][a-z0-9_-]*/' "$mnt/usr/bin/kuma" 2>/dev/null |
        grep -v linuxbrew | sort -u | head -3) && [ -n "$paths" ]; then
        bad "/usr/bin/kuma embeds build paths: $(echo "$paths" | tr '\n' ' ')" \
            "build it in CI, or set trim-paths in the release profile"
    else
        ok "/usr/bin/kuma embeds no build paths"
    fi
fi

echo
if [ "$failures" -gt 0 ]; then
    echo "$failures check(s) failed: do not publish $image"
    exit 1
fi
echo "$image carries no identity"
