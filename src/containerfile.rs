use crate::config::{Config, Desktop};
use crate::seam;
use anyhow::{Context, Result};
use std::collections::BTreeSet;
use std::path::Path;

/// The curated niri desktop: compositor, greeter, launcher, bar,
/// notifications, terminal, audio, portals, fonts.
const NIRI_PACKAGES: &[&str] = &[
    "flatpak",
    "niri",
    "xwayland-satellite",
    "greetd",
    "tuigreet",
    // The shell. One process for the bar, notifications, wallpaper,
    // OSDs, idle, lock, control centre and night light, which is why
    // waybar, mako, swaybg, swayidle, swaylock, wob and wlsunset are all
    // gone from this list. In Fedora proper, so this costs kuma no new
    // trust root and nothing to package.
    "noctalia",
    "kitty",
    "pipewire",
    "pipewire-pulseaudio",
    "wireplumber",
    "xdg-desktop-portal-gtk",
    "xdg-desktop-portal-gnome",
    "dconf",
    "gnome-keyring",
    // the PAM module that unlocks the login keyring is a subpackage,
    // and nothing depends on it: without this every keyring-using app
    // prompts on launch, silently, because the greeter's PAM lines are
    // '-' prefixed and skip a missing module without a word
    "gnome-keyring-pam",
    "polkit",
    "mesa-dri-drivers",
    // OpenGL alone strands Vulkan apps on lavapipe software rendering
    "mesa-vulkan-drivers",
    "vulkan-loader",
    "default-fonts-core-sans",
    "default-fonts-core-mono",
    // Both desktop faces, via the metapackage rather than by naming
    // them: waybar needs Free for most glyphs and Brands for the
    // bluetooth one, and the per-face packages carry the major version
    // in their names (fontawesome-6-* became fontawesome-7-* in Fedora
    // 45), so naming the faces breaks the build on every Font Awesome
    // major. The metapackage's name is version-free, it requires
    // exactly those two packages, and it owns no files itself.
    // Kept after the bar that needed it left: the shell bundles its own
    // icon font, but flatpaks and the odd GTK app still reach for these
    // glyphs and render tofu without them.
    "fontawesome-fonts-all",
    // base ships glibc-minimal-langpack only; without real locale data
    // en_US.UTF-8 fails to resolve and anything formatting a date or a
    // number falls back to C
    "glibc-langpack-en",
    // hardware enablement — the minimal base targets servers
    "NetworkManager-wifi",
    // The icons kuma's desktop entries name, by file. GTK drags this in
    // anyway, and that is exactly the kind of luck the seam should not
    // run on: an entry whose icon does not resolve draws a blank square
    // and reports nothing. Named here so it is kuma's dependency and not
    // somebody else's.
    "adwaita-icon-theme",
    // The GTK3 half of "the desktop follows the palette". Stock Adwaita
    // GTK3 ignores the colour names a palette can set: overriding
    // theme_bg_color from the user stylesheet moves nothing, measured on
    // gtk3-3.24.52. adw-gtk3 is libadwaita's look ported back to GTK3 and
    // it does read them, so thunar, pavucontrol, nm-connection-editor and
    // blueman follow the same palette as the shell. In Fedora proper, 175
    // KiB, no new trust root.
    "adw-gtk3-theme",
    // desktop-file-validate: the build checks the entries it generates
    "desktop-file-utils",
    // The shell's control centre owns wifi now, so this is no longer the
    // desktop's answer for it. It stays because it is the TTY answer:
    // when a session refuses to start, this is the only way left to get
    // the network up and fix the machine.
    "NetworkManager-tui",
    "wpa_supplicant",
    "brightnessctl",
    "power-profiles-daemon",
    // device-level settings: the config file covers system definition,
    // but wifi picking, pairing, mixers, and mounts are machine state
    "pavucontrol",
    "nm-connection-editor",
    "bluez",
    "blueman",
    "thunar",
    "thunar-archive-plugin",
    "file-roller",
    "gvfs",
    "gvfs-mtp",
    "wf-recorder",
    // base ships zram-generator but not the defaults that activate it:
    // without this the desktop has zero swap and the OOM killer eats
    // windows under memory pressure
    "zram-generator-defaults",
    // avahi is named, not assumed: the desktop enables avahi-daemon, and
    // "the base ships it" was only ever true of fedora-bootc by luck —
    // kuma's composed base doesn't. nss-mdns makes .local names and
    // driverless printer discovery actually resolve
    "avahi",
    "nss-mdns",
    // (nwg-look would fit here for GTK theme tweaks, but it's COPR-only)
    "libnotify",
    "cups",
    "system-config-printer",
    // session essentials
    "wl-clipboard",
    "xsettingsd",
    "spice-vdagent",
    "xdg-user-dirs",
    "default-fonts-core-emoji",
    "mate-polkit",
    "firewalld",
    // niri's built-in screenshot UI covers the Print keys; grim+slurp are
    // the wlr-screencopy tools everything scriptable builds on
    "grim",
    "slurp",
    // Mod+Print: annotate before sharing (satty is COPR-only)
    "swappy",
    // the XF86Audio sed that makes room for kuma-osd also drops niri's
    // stock playerctl binds; kuma re-adds them, and nothing else pulls
    // playerctl in now that waybar has left the set
    "playerctl",
    // plug-in automount: thunar only mounts on click, and thunar-volman
    // needs the thunar daemon plus xfconf toggles to do its job
    "udiskie",
    // file-roller alone can't open 7z/rar downloads
    "7zip",
    "unar",
    // default-fonts-core-sans is latin-only: CJK pages render as tofu
    "google-noto-sans-cjk-vf-fonts",
    // quiet identity: baked and configured, never run at shell startup
    "fastfetch",
];

/// niri Recommends these, so they arrive whether or not `NIRI_PACKAGES`
/// names them. Every one is a program kuma deliberately does not ship.
///
/// All four were found the same way and none of them by a test: build
/// the image, then ask it whether the thing you removed is still there.
/// Removing a package from `NIRI_PACKAGES` does nothing when something
/// else recommends it, and the symptom is an image quietly carrying a
/// bar, a lock screen and a launcher that nothing starts.
const NIRI_EXCLUDES: &[&str] = &["alacritty", "waybar", "swaylock", "fuzzel"];

/// The curated COSMIC desktop. Unlike niri's hand-assembled set, COSMIC
/// curates itself: cosmic-session hard-requires the whole coherent
/// desktop (compositor, panel, applets, settings, files, terminal,
/// notifications, OSD, screenshot, portal, fonts), so this list is the
/// session plus the hardware enablement a desktop lives on. pipewire is
/// explicit because nothing in the session requires the daemon, only
/// its client library. cosmic-store is absent, though no longer because
/// convergence would fight it: a store is a user-facing app, so which
/// one a machine gets is the declaration's call, not the desktop set's.
const COSMIC_PACKAGES: &[&str] = &[
    "cosmic-session",
    // the session requires files, term, and settings but not the text
    // editor — which the default dock pins, so ship it or the pin is dead
    "cosmic-edit",
    "flatpak",
    "fastfetch",
    // unlocked with the login password via PAM, same as the niri set,
    // and the -pam subpackage is the module that does the unlocking
    "gnome-keyring",
    "gnome-keyring-pam",
    "pipewire",
    "pipewire-pulseaudio",
    "wireplumber",
    "mesa-dri-drivers",
    "mesa-vulkan-drivers",
    "vulkan-loader",
    "glibc-langpack-en",
    "NetworkManager-wifi",
    "wpa_supplicant",
    "power-profiles-daemon",
    "bluez",
    "firewalld",
    "zram-generator-defaults",
    // same story as the niri set: the desktop enables avahi-daemon, so
    // the desktop ships it
    "avahi",
    "nss-mdns",
    "cups",
    "system-config-printer",
    "spice-vdagent",
    // cosmic-files mounts removable media through udisks2 directly
    "udisks2",
    "default-fonts-core-emoji",
    "google-noto-sans-cjk-vf-fonts",
    // the seam's dependencies, named rather than assumed: kuma's desktop
    // entries draw Adwaita's symbolic icons and the build validates them
    // with desktop-file-validate. Both ride in today on cosmic-session's
    // closure, and a seam that silently loses its icons because an
    // unrelated package stopped depending on a theme is not a seam
    "adwaita-icon-theme",
    "desktop-file-utils",
];

/// COSMIC's packaged dock pins the Firefox flatpak and cosmic-store —
/// neither of which a kuma image guarantees: the browser is the
/// declaration's choice, and the store is deliberately absent. The
/// baked default pins only what the image ships; anything declared is
/// one right-click from a pin.
const COSMIC_FAVORITES: &str = r#"[
    "com.system76.CosmicFiles",
    "com.system76.CosmicEdit",
    "com.system76.CosmicTerm",
    "com.system76.CosmicSettings",
]
"#;

/// The packaged default, pointed at the Kuma wallpaper. filter_by_theme
/// must go false: left on, COSMIC swaps the wallpaper back out for its
/// own theme-matched set.
const COSMIC_BACKGROUND: &str = r#"(
    output: "all",
    source: Path("/usr/share/backgrounds/kuma/kuma-wallpaper.jpg"),
    filter_by_theme: false,
    rotation_frequency: 3600,
    filter_method: Lanczos,
    scaling_mode: Zoom,
    sampling_method: Alphanumeric,
)
"#;

/// kuma's own signing key, baked into the binary so that any build has
/// it. A build runs from the binary and not from a checkout, so reading
/// the repository's copy at build time would work only for people who
/// have the repository.
pub const COSIGN_PUB: &str = include_str!("../cosign.pub");

/// Where the key lands in the image. Under /etc/pki/containers because
/// that is where a policy can name it and where an administrator would
/// look for it.
pub const COSIGN_PUB_PATH: &str = "/etc/pki/containers/kuma.pub";

/// Require a signature for kuma's own published images, and nothing else.
///
/// The narrow rule is the point. This file is shared by podman and bootc,
/// so a blanket requirement would refuse Fedora's base image on the next
/// `kuma update` and refuse the machine's own locally built images on the
/// next `kuma switch`. What it buys, scoped this way, is that the one
/// registry kuma tells strangers to install from cannot serve them
/// something kuma did not sign.
///
/// `matchRepository`, not `matchRepoDigestOrExact`, and this was measured
/// rather than chosen: cosign records the identity as the bare repository
/// (`ghcr.io/letdown2491/kuma`, no tag), so an exact match can never
/// succeed and the policy would reject every image kuma has ever
/// published. Verified against the live registry in both directions, with
/// the real key and with a wrong one.
pub(crate) fn signature_policy() -> String {
    let repo = crate::published_repo();
    format!(
        r#"{{
  "default": [{{"type": "insecureAcceptAnything"}}],
  "transports": {{
    "docker": {{
      "{repo}": [
        {{
          "type": "sigstoreSigned",
          "keyPath": "{COSIGN_PUB_PATH}",
          "signedIdentity": {{"type": "matchRepository"}}
        }}
      ]
    }},
    "containers-storage": {{"": [{{"type": "insecureAcceptAnything"}}]}},
    "docker-daemon": {{"": [{{"type": "insecureAcceptAnything"}}]}},
    "dir": {{"": [{{"type": "insecureAcceptAnything"}}]}},
    "oci": {{"": [{{"type": "insecureAcceptAnything"}}]}}
  }}
}}
"#
    )
}

/// Without this the policy above cannot find anything to verify: cosign
/// stores a signature as a separate `sha256-<digest>.sig` tag beside the
/// image, and containers/image only looks there when told to.
pub(crate) fn registries_d() -> String {
    let repo = crate::published_repo();
    format!("docker:\n  {repo}:\n    use-sigstore-attachments: true\n")
}

/// Fedora's mesa VA-API driver ships with H.264/H.265/VC-1 decode
/// stripped (patents), so video silently falls back to CPU. RPM
/// Fusion's freeworld build restores it; --allowerasing swaps out
/// the gutted driver if a dependency dragged it in.
fn mesa_freeworld() -> String {
    dnf_layer(
        "dnf -y install --setopt=keepcache=1 \"https://mirrors.rpmfusion.org/free/fedora/rpmfusion-free-release-$(rpm -E %fedora).noarch.rpm\" \\\n    && dnf -y install --setopt=keepcache=1 --allowerasing mesa-va-drivers-freeworld",
    )
}

/// The mount that turns dnf's cache from dead weight in the image into a
/// cache that survives across builds.
///
/// Every dnf layer used to end in `dnf clean all`, which threw away the
/// repo metadata (~150 MB) and the downloaded RPMs (hundreds of MB for
/// the desktop set) so they would not bloat the image. The cost was that
/// a cold build, and every `kuma update` (a base bump invalidates every
/// layer), re-downloaded all of it. A build cache mount keeps that data
/// on the host instead of in the layer: it never lands in the image, so
/// there is nothing to clean, and the next build reuses it. `keepcache=1`
/// is what makes dnf leave the RPMs there rather than only the metadata,
/// so an update re-downloads only the packages that actually changed.
const DNF_CACHE: &str = "/var/cache/libdnf5";

/// One dnf RUN layer with the package cache mounted rather than baked.
/// `body` is the shell after `RUN `; callers that install a plain list go
/// through `dnf_install`, and the two-step mesa case builds its own.
fn dnf_layer(body: &str) -> String {
    format!("RUN --mount=type=cache,target={DNF_CACHE} \\\n    {body}\n")
}

/// The common case: install a package list, cached, no clean.
fn dnf_install(packages: &str) -> String {
    dnf_layer(&format!("dnf -y install --setopt=keepcache=1 {packages}"))
}

/// Unlock the keyring with the login password, or every keyring-using
/// app nags for it on launch (browsers most visibly, but far from only
/// them). Autologin skips this: no password is typed.
///
/// Fedora's greeter PAM stacks already call pam_gnome_keyring, so
/// there is nothing to append; but every one of those lines carries a
/// '-' prefix, which tells PAM to skip a missing module in total
/// silence. The module ships in gnome-keyring-pam, a subpackage of
/// gnome-keyring that nothing pulls in, so the desktop lists name it
/// explicitly and this asserts both halves: the module exists on disk,
/// and the stack that actually authenticates this desktop still calls
/// it. Without the assert the failure mode is invisible: login
/// succeeds, nothing is logged, and the keyring is simply never
/// unlocked (which is exactly how it shipped until 2026-08-07).
///
/// `service` is the /etc/pam.d file the greeter authenticates against:
/// greetd's own for niri, cosmic-greeter's for COSMIC. They are not
/// interchangeable; asserting the wrong one proves nothing.
fn keyring_pam(service: &str) -> String {
    format!(
        "RUN test -f /usr/lib64/security/pam_gnome_keyring.so \\\n    && grep -q pam_gnome_keyring /etc/pam.d/{service}\n"
    )
}

/// Two lint warnings' worth of build litter, swept before the verdict.
///
/// Installing packages leaves behind runtime state that a booted machine
/// makes for itself anyway: /run/cups, /run/dnf, /run/mdadm, /run/samba
/// and selinux-policy's scratch files, plus dnf5's own log in /var/log.
/// bootc's nonempty-run-tmp and var-log lints flag both, and both are
/// right. /run and /tmp are tmpfs on a running machine, so anything
/// baked into them is dead weight nothing will ever read, and a shipped
/// logfile is a record of the build host rather than of the machine.
///
/// rm's exit status cannot be the verdict here. podman holds two paths
/// inside /run open for the whole build: /run/secrets, and (because the
/// base's /etc/resolv.conf is a symlink into it)
/// /run/systemd/resolve/stub-resolv.conf. Deleting a live bind mount
/// fails busy no matter what the image wants. Neither survives as
/// content: what lands in the layer is an empty file, which is exactly
/// what "nonempty" means the lint does not count.
///
/// /var/log is the half kuma controls completely, so it is asserted
/// rather than hoped for: a logfile that outlives the sweep fails the
/// build instead of shipping.
const SWEEP: &str = r#"
RUN find /run /tmp -mindepth 1 -delete; \
    find /var/log -type f -delete; \
    ! find /var/log -type f | grep -q .
"#;

/// bootc's own build-time check that the image is a valid bootable
/// container. It runs last, and its verdict is the build's verdict.
///
/// The wrapper exists for one upstream bug. bootc 1.16.7's var-tmpfiles
/// lint walks /usr/lib/tmpfiles.d and opens what it finds, and
/// tpm2-tss-fapi.conf points at securityfs:
///
///   z- /sys/kernel/security/ima/binary_runtime_measurements 0440 root tss - -
///
/// Inside a build that path is unreadable, so the lint aborts the whole
/// run with "Unexpected runtime error running lint var-tmpfiles" and no
/// image is produced. (Note the '-' on 'z-': that is tmpfiles' own
/// ignore-failures modifier, so systemd tolerates the very path the lint
/// will not. Same bailout as bootc-dev/bootc#1481, which hits it through
/// qemu-user instead of securityfs.) It reproduces on the unmodified
/// fedora-bootc:44 base, so nothing kuma builds can avoid it.
///
/// The insult on top: var-tmpfiles is a warning-level lint, so even
/// when it runs it cannot fail a build. Only its crash can.
///
/// Rather than pass --skip var-tmpfiles forever, which would quietly
/// retire the check the day upstream fixes this, tolerate exactly this
/// crash and nothing else: any other lint failure still fails the build
/// (verified against a fatal var-run), warnings still reach the log
/// either way, and once bootc stops crashing the first run simply
/// passes and the fallback goes cold on its own.
///
/// The output is held in a variable rather than a scratch file. A file
/// under /tmp was the obvious way to look at the lint's own stderr twice,
/// and it worked, but the lint reads /tmp while it runs: once the rest of
/// the litter was swept, the only thing left in the image was
/// /tmp/lint.err, and the check spent its last warning reporting itself.
/// Nothing on disk means nothing to notice and nothing to clean up.
const LINT: &str = r#"
RUN said=$(bootc container lint 2>&1); rc=$?; \
    printf '%s\n' "$said"; \
    if [ $rc -ne 0 ] && printf '%s' "$said" | grep -q 'var-tmpfiles: I/O error'; then \
        rc=0; bootc container lint --skip var-tmpfiles || rc=$?; \
    fi; \
    exit $rc
"#;

const GREETD_CONFIG: &str = r#"[terminal]
vt = 1

[default_session]
command = "tuigreet --time --remember --greeting 'Welcome to Kuma' --cmd niri-session"
user = "greetd"
"#;

/// What starts a session, and where each greeter reads it from.
///
/// Shared with `liveiso`, which autologins its own throwaway account the
/// same way. Two copies of these strings means a session command can
/// change and break autologin in exactly one of the two places, and the
/// live one is the copy no CI boots.
pub(crate) const NIRI_SESSION: &str = "niri-session";
pub(crate) const COSMIC_SESSION: &str = "start-cosmic";
pub(crate) const GREETD_CONF: &str = "/etc/greetd/config.toml";
/// cosmic-greeter authenticates against its own file. greetd's exists in
/// a COSMIC image too, pulled in as a dependency, so writing there would
/// look healthy and prove nothing.
pub(crate) const COSMIC_GREETER_CONF: &str = "/etc/greetd/cosmic-greeter.toml";

/// greetd's initial_session is exactly autologin semantics: straight
/// into the desktop at boot, greeter on logout.
pub(crate) fn initial_session(command: &str, user: &str) -> String {
    format!("\n[initial_session]\ncommand = \"{command}\"\nuser = \"{user}\"\n")
}

fn greetd_config(config: &Config) -> String {
    let mut out = GREETD_CONFIG.to_string();
    if let Some(user) = &config.user {
        if user.autologin {
            out.push_str(&initial_session(NIRI_SESSION, &user.name));
        }
    }
    out
}

/// Desktop kernel args. The minimal base ships no auditd, so kernel audit
/// records spray onto the console; `quiet` keeps the console clean without
/// disabling auditing (records still reach the journal).
const DESKTOP_KARGS: &str = "kargs = [\"quiet\"]\n";

/// Declared flatpaks are baked into the image as a list; this oneshot
/// converges the machine to it on boot. The declaration is atomic image
/// content — only the app installs are runtime state.
///
/// The retries cover the timer's Persistent=true catch-up, which fires on
/// resume before Wi-Fi is back — network-online.target only orders boot.
///
/// The low CPU/IO weights matter most on first boot, when every declared
/// app downloads at once while the user logs in for the first time: that
/// storm once starved cosmic-panel into a session with no panel at all.
/// Convergence is background work; the session it converges for is not.
/// Permissions converge at boot and on an explicit `kuma sync`, and
/// deliberately not on the daily timer that carries installs. An install
/// arriving at a random hour is additive and idempotent; a permission
/// reverting at a random hour changes what a running app can reach, and
/// a Flatseal toggle silently flipping back the next afternoon is
/// indistinguishable from a bug. Boot-only buys a rule that fits in a
/// sentence: declared permissions are restored when you boot, and the
/// session in between is yours to experiment in, with `kuma diff` to
/// show the edit and `kuma capture` to keep it.
///
/// Ordered after the installer so an app that arrives this boot has its
/// permissions before it is first launched.
const FLATPAK_OVERRIDES_SERVICE: &str = "\
[Unit]
Description=Converge Flatpak permission overrides to the declaration
After=kuma-flatpak-sync.service

[Service]
Type=oneshot
ExecStart=/usr/bin/kuma flatpak-overrides --scope system

[Install]
WantedBy=multi-user.target
";

/// The user store's half. It runs as the account that owns the files
/// rather than as root reaching into a home, which is both rude and a
/// race against a running Flatseal.
const FLATPAK_OVERRIDES_USER_SERVICE: &str = "\
[Unit]
Description=Converge this account's Flatpak permission overrides

[Service]
Type=oneshot
ExecStart=/usr/bin/kuma flatpak-overrides --scope user

[Install]
WantedBy=default.target
";

const FLATPAK_SYNC_SERVICE: &str = r#"[Unit]
Description=Converge Flatpak applications to the declared list
Wants=network-online.target
After=network-online.target
StartLimitIntervalSec=1h
StartLimitBurst=6

[Service]
Type=oneshot
ExecStart=/usr/libexec/kuma-flatpak-sync
Restart=on-failure
RestartSec=2min
CPUWeight=25
IOWeight=25

[Install]
WantedBy=multi-user.target
"#;

/// Boot-only convergence goes stale on machines that stay up: a
/// two-week uptime means a two-week-old browser. Daily, with catch-up
/// for machines that were asleep at the appointed hour.
const FLATPAK_SYNC_TIMER: &str = r#"[Unit]
Description=Daily Flatpak convergence

[Timer]
OnCalendar=daily
Persistent=true
RandomizedDelaySec=1h

[Install]
WantedBy=timers.target
"#;

/// Read-only btrfs snapshots of the declared subvolume, pruned to the
/// declared retention. The parameters are baked in by `snapshot_script`.
///
/// Two guards, both of which exit clean rather than fail: a target that
/// isn't btrfs, and a target that is btrfs but not a subvolume. The
/// declaration is one file across many machines, and a laptop laid out
/// with ext4 must not turn into a unit that fails on every timer tick.
///
/// `.snapshots` lives inside the target on purpose. It is a directory
/// holding nested subvolumes, and btrfs does not recurse into those when
/// it snapshots the parent, so the snapshots never contain each other.
///
/// It is traversable (0755) so that recovering a file is a file manager
/// and a copy rather than a root shell. That exposes nothing: a snapshot
/// preserves the permissions of everything inside it, so a home directory
/// that was 0700 in the live tree is still 0700 in every snapshot of it.
/// Only the timestamps in the listing become public.
const SNAPSHOT_SCRIPT: &str = r#"#!/usr/bin/bash
set -euo pipefail
target='{target}'
keep_recent={keep_recent}
keep_daily={keep_daily}
store="$target/.snapshots"

# -T, so the question is "what filesystem holds this path" rather than
# "what is mounted exactly here". A btrfs subvolume does not have to be a
# mount point: on a machine kuma installed, /var/home is a subvolume
# nested inside the deployment's /var, and the bare form prints nothing
# and sent this script home. Machines whose installer gave /var/home its
# own fstab entry answered either way, which is the only reason this ever
# looked like it worked. `kuma doctor` has always asked with -T; these
# two have to ask the same question or one of them is lying.
[ "$(findmnt -no FSTYPE -T "$target" 2>/dev/null || true)" = btrfs ] || exit 0
btrfs subvolume show "$target" >/dev/null 2>&1 || exit 0

install -d -m 0755 "$store"
btrfs subvolume snapshot -r "$target" "$store/$(date +%Y-%m-%dT%H%M%S)" >/dev/null

# Newest first. Keep keep_recent whatever their age, then the newest
# survivor of each of keep_daily days the recent tier did not already
# cover, and delete the rest. Days the recent tier spans are marked seen
# without spending a daily slot, so keep_daily always buys that many
# *further* days back rather than being eaten by a busy afternoon. Only
# names this script writes are ever considered, let alone deleted.
mapfile -t all < <(ls -1 "$store" 2>/dev/null | grep -E '^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{6}$' | sort -r)
declare -A day_seen=()
recent=0
days=0
for snap in "${all[@]}"; do
    day="${snap%%T*}"
    if [ "$recent" -lt "$keep_recent" ]; then
        recent=$((recent + 1))
        day_seen[$day]=1
        continue
    fi
    if [ -z "${day_seen[$day]:-}" ] && [ "$days" -lt "$keep_daily" ]; then
        day_seen[$day]=1
        days=$((days + 1))
        continue
    fi
    btrfs subvolume delete "$store/$snap" >/dev/null
done
"#;

/// Copy the newest local snapshot to the declared repository.
///
/// **Why it mounts the snapshot over the live path.** restic 0.19.1 has
/// no `--set-path` (checked against the package Fedora ships, not
/// assumed), and its default `--group-by` is `host,paths`. A snapshot
/// directory is named for the minute it was taken, so backing one up
/// directly would hand restic a different source path every night: no
/// parent snapshot would ever match, and every run would re-read every
/// byte of a 93 GB home to discover it already had it. Content
/// addressing means that costs no storage, only the whole disk read
/// nightly, which is the kind of waste that gets a backup turned off.
///
/// Mounting the snapshot over `target` in this unit's own mount
/// namespace fixes both halves at once. restic sees a stable path, so
/// parents match and the run is incremental; and the path it records is
/// the one the files actually live at, so a restore lands where it
/// belongs rather than under some staging directory. `PrivateMounts=yes`
/// keeps the bind inside the unit, so nothing on the running system sees
/// its home replaced by a read-only copy for the duration.
///
/// **Three guards that exit 0 rather than failing**, because each is a
/// machine that is not ready rather than a machine that is broken, and
/// one declaration describes many machines: no credential loaded yet, no
/// snapshot taken yet, and no repository at the far end. The last one is
/// deliberate rather than helpful: seeding 93 GB is `kuma backup --init`,
/// a thing somebody does on purpose while plugged in, not something a
/// timer decides to start on a train. Freshness is what escalates a
/// machine that stays in one of these states, which is why the stamp is
/// only written by a run that actually copied something.
const BACKUP_SCRIPT: &str = r#"#!/usr/bin/bash
set -euo pipefail
target='{target}'
store="$target/.snapshots"
stamp=/var/lib/kuma/backup-last

export RESTIC_REPOSITORY='{repo}'


if [ -z "${RESTIC_PASSWORD:-}${RESTIC_PASSWORD_FILE:-}" ]; then
    echo "no credential loaded: the declaration names one and this machine has not been given it"
    exit 0
fi

# `|| true` is load-bearing under `set -o pipefail`: with no snapshot
# yet, grep exits 1, the assignment inherits it, and `set -e` would kill
# the script one line above the guard written for exactly that case. The
# guard would have been unreachable on every machine it was for. `head`
# closing the pipe early can end `sort` the same way.
newest=$(ls -1 "$store" 2>/dev/null \
    | grep -E '^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{6}$' | sort -r | head -1 || true)
if [ -z "$newest" ]; then
    echo "no snapshot in $store yet; nothing to copy"
    exit 0
fi

# restic keeps a cache of the repository's index and metadata, and treats
# being unable to open one as fatal rather than as a reason to go slower.
# A systemd system service has no HOME and no XDG_CACHE_HOME, so without
# this every run dies before it reaches the repository, saying "neither
# $XDG_CACHE_HOME nor $HOME are defined" from somewhere that reads like a
# network failure.
#
# /var/cache rather than a temporary directory, because the cache is why
# a second backup is quick: discarding it nightly means re-fetching the
# index to discover nothing changed. Below the guards above, because
# those need no repository and this needs root.
export RESTIC_CACHE_DIR=/var/cache/restic
install -d -m 0700 /var/cache/restic

# Bounded, because restic treats a missing bucket as a transient error
# and retries it with exponential backoff. Asking "is there a repository"
# of a machine that has not been seeded therefore takes minutes to answer
# "no", every night, and restic offers no flag to limit backend retries
# (--retry-lock is about locks; --stuck-request-timeout defaults to 5m).
#
# The two failures are told apart by what restic said rather than by an
# exit status, because a timeout looks identical either way. Both exit 0:
# one is a machine nobody has seeded and the other is a machine that
# cannot reach its repository right now, and neither is broken. Freshness
# is what escalates a machine that stays in either state.
probe=$(timeout 30 restic cat config 2>&1) || {
    if printf '%s' "$probe" | grep -qiE 'does not exist|is there a repository'; then
        echo "no repository at $RESTIC_REPOSITORY yet; seed it once with 'kuma backup --init'"
    else
        echo "cannot reach $RESTIC_REPOSITORY within 30s; nothing copied this run"
    fi
    exit 0
}

# Read-only already, being a btrfs snapshot; the bind is for the path,
# not for the permissions.
# Undone on the way out, and that matters outside this unit rather than
# inside it. Under PrivateMounts=yes the namespace dies with the service
# and takes the bind with it; run by hand, which is the obvious thing to
# try after "systemctl start kuma-snapshot.service" works, an untrapped
# bind leaves the live /var/home replaced by a read-only snapshot until
# the next reboot.
mount --bind "$store/$newest" "$target"
trap 'umount "$target" 2>/dev/null || true' EXIT

restic backup "$target"{extra_paths} \
    --tag kuma \
    --skip-if-unchanged \
    --exclude "$target/.snapshots" \
{excludes}
# Forget every run, prune weekly, and they are different costs. Forgetting
# drops snapshot references and is nearly free. Pruning walks the index and
# repacks pack files that expiry left mostly empty, which on a home
# directory over somebody's uplink can move gigabytes, and restic's own
# advice is to do it from time to time rather than after every backup.
# Retention still takes effect immediately either way; only the reclaiming
# waits.
#
# Timed off a stamp rather than a weekday, because a machine that is never
# on for the chosen day would never prune at all, and that is exactly the
# laptop this is for.
restic forget --tag kuma \
    --keep-daily {keep_daily} --keep-weekly {keep_weekly} --keep-monthly {keep_monthly}
# Before anything writes into it. The stamp below made the directory and
# the prune stamp above it did not, so a machine where /var/lib/kuma did
# not already exist would fail the unit after a backup that worked, which
# is the most expensive place to discover an ordering.
install -d -m 0755 /var/lib/kuma

pruned=/var/lib/kuma/backup-pruned
if [ ! -f "$pruned" ] || [ "$(( $(date -u +%s) - $(cat "$pruned" 2>/dev/null || echo 0) ))" -ge 604800 ]; then
    restic prune
    date -u +%s > "$pruned"
fi

# Epoch first because doctor parses it and this repo carries no date
# library; the readable form beside it is for whoever cats the file.
printf '%s %s\n' "$(date -u +%s)" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" > "$stamp"
"#;

/// The curated excludes, which are additive rather than a default the
/// declaration replaces.
///
/// Every one is a tree this same file already rebuilds, so storing it
/// would be paying to keep a copy of something kuma can recreate from
/// six lines. `/home` is a symlink to `var/home`, which puts Homebrew's
/// entire prefix inside the snapshot target: `packages.brew` reconverges
/// it, so a naive copy would ship the largest rebuildable tree on the
/// machine offsite every night.
/// The one unrecoverable thing that lives outside the snapshot target.
/// Named here rather than spelled twice, since doctor reports on the
/// same path.
pub const NETWORK_CONNECTIONS: &str = "/etc/NetworkManager/system-connections";

const CURATED_EXCLUDES: &[&str] = &["/linuxbrew", "/*/.cache", "/*/.local/share/containers"];

/// Restart because a timer that fires on resume finds the network still
/// coming up, and `network-online.target` only orders boot. The start
/// limit is what keeps that from becoming an infinite retry against a
/// repository that is simply gone: six tries an hour, then stop and let
/// doctor's freshness line be the thing that says so.
fn backup_service(config: &Config) -> String {
    format!(
        "[Unit]\nDescription=Copy the newest snapshot to the declared repository\n\
         Wants=network-online.target\nAfter=network-online.target\n\
         After=kuma-snapshot.service\n\
         StartLimitIntervalSec=1h\nStartLimitBurst=6\n\n\
         [Service]\nType=oneshot\nExecStart=/usr/libexec/kuma-backup\n\
         EnvironmentFile=-/var/lib/kuma/secrets/{secret}.env\n\
         PrivateMounts=yes\n\
         Restart=on-failure\nRestartSec=2min\nCPUWeight=25\nIOWeight=25\n\n\
         [Install]\nWantedBy=multi-user.target\n",
        secret = config.backup.secret,
    )
}

fn backup_timer(interval: &str) -> String {
    format!(
        "[Unit]\nDescription=Scheduled offsite backup\n\n[Timer]\nOnCalendar={interval}\nPersistent=true\nRandomizedDelaySec=1h\n\n[Install]\nWantedBy=timers.target\n"
    )
}

/// The script with this declaration's repository, retention and excludes
/// baked in. Validation has already restricted every substitution to a
/// conservative alphabet, and refused a repository with a password in it.
///
/// A declared `~/` means "in every home", which is the only reading that
/// works when the target holds more than one.
fn backup_script(config: &Config) -> String {
    let target = &config.snapshots.target;
    let mut excludes = String::new();
    // `$target` rather than the substituted path, so the script has one
    // definition of where it is looking and the line beside this one
    // (`--exclude "$target/.snapshots"`) is spelled the same way.
    for suffix in CURATED_EXCLUDES {
        excludes.push_str(&format!("    --exclude \"$target{suffix}\" \\\n"));
    }
    for path in &config.backup.exclude {
        let path = match path.strip_prefix("~/") {
            Some(rest) => format!("$target/*/{rest}"),
            None => path.clone(),
        };
        excludes.push_str(&format!("    --exclude \"{path}\" \\\n"));
    }
    // The last continuation has to go, or the blank line after it eats
    // the next command.
    let excludes = excludes.trim_end().trim_end_matches('\\').trim_end().to_string();
    // A second source path, so the one unrecoverable thing outside home
    // rides along when it is asked for. It changes restic's path group,
    // so the first run after switching this on has no parent and rescans
    // once; every run after that is incremental again.
    let extra_paths = if config.backup.network_connections {
        format!(" {NETWORK_CONNECTIONS}")
    } else {
        String::new()
    };
    BACKUP_SCRIPT
        .replace("{extra_paths}", &extra_paths)
        .replace("{target}", target)
        .replace("{repo}", &config.backup.repo)
        .replace("{excludes}", &excludes)
        .replace("{keep_daily}", &config.backup.keep_daily.to_string())
        .replace("{keep_weekly}", &config.backup.keep_weekly.to_string())
        .replace("{keep_monthly}", &config.backup.keep_monthly.to_string())
}

/// Put a home directory back on a machine that has just been installed.
///
/// **Why this is a first-boot unit and not part of `kuma install`.**
/// `/var/home` does not exist at install time. The image ships none;
/// `rpm-ostree-0-integration.conf` has tmpfiles create it every boot,
/// and `kuma-home-subvol` turns it into a btrfs subvolume with the right
/// SELinux label while it is still empty. Restoring during the install
/// would leave an ordinary directory with files in it, which is exactly
/// the state `kuma-home-subvol` steps back from, so the machine would
/// come up with no subvolume, no snapshots and nothing saying why.
///
/// So the install writes the request and the credential onto the target
/// the same way it writes the account, and this runs once, after the
/// subvolume exists and after the account it will own does.
///
/// **The request survives a failed restore, and is cleared only by one
/// that worked.** The first version cleared it first, reasoning that a
/// request outliving its own failure means a unit that re-runs forever.
/// That was wrong twice over. There is no `Restart=` here, so the loop
/// it guarded against cannot happen: what survival actually buys is one
/// attempt per boot. And the failure it caused is the worse one, because
/// a repository briefly unreachable at first boot would discard the
/// restore permanently and bring the machine up empty, with the data
/// still sitting safe somewhere nobody would think to look again.
///
/// A missing credential still clears it, because that is not a bad day,
/// it is a request nothing can ever satisfy.
const RESTORE_SCRIPT: &str = r#"#!/usr/bin/bash
set -euo pipefail
request=/var/lib/kuma/restore-request
secret=/var/lib/kuma/secrets/restore.env

[ -f "$request" ] || exit 0
if [ ! -r "$secret" ]; then
    echo "a restore was requested and $secret is not there" >&2
    rm -f "$request"
    exit 1
fi

# Nothing is sourced here. The values arrive through the unit's
# EnvironmentFile=, and what this checks is that they arrived: an
# unparseable file leaves the variable unset, and `set -u` would
# otherwise abort with a message about a shell variable rather than
# about the file somebody wrote.
if [ -z "${RESTIC_REPOSITORY:-}" ]; then
    echo "$secret set no RESTIC_REPOSITORY that systemd could parse" >&2
    rm -f "$request"
    exit 1
fi

# Same reason as the converger: a unit has no HOME, and restic will not
# start without somewhere to cache.
export RESTIC_CACHE_DIR=/var/cache/restic
install -d -m 0700 /var/cache/restic

# Both paths the converger stores, not just home. Backing up the network
# connections and then not restoring them defeats the only reason that
# knob exists, and it defeats it silently: the machine comes up, the
# files are all there, and the one thing nothing else can recreate is
# missing. Naming a path the snapshot does not hold restores nothing and
# is not an error, so this is safe when the knob was off.
#
# --tag kuma so a repository somebody also uses by hand cannot hand this
# machine a snapshot kuma never made.
echo "restoring /var/home from $RESTIC_REPOSITORY"
restic restore latest --tag kuma --target / \
    --include /var/home \
    --include /etc/NetworkManager/system-connections

# Only now. Everything above can fail on a bad day (a repository that is
# briefly unreachable, a link that drops), and a bad day must cost a
# retry at the next boot rather than the data.
rm -f "$request"
echo "restore finished"
"#;

/// Ordered behind both of the units that have to have run first, and
/// conditioned on the request file so that every other boot skips it
/// without a unit that failed.
const RESTORE_SERVICE: &str = r#"[Unit]
Description=Restore this machine's home directory from the declared repository
ConditionPathExists=/var/lib/kuma/restore-request
Wants=network-online.target
After=network-online.target
After=kuma-home-subvol.service kuma-user-sync.service
Before=greetd.service

[Service]
Type=oneshot
# systemd PARSES this; it never executes it. The script used to source
# the same file, which ran whatever was on a right-hand side as root on
# first boot, on a file concepts.md tells people to carry on the stick
# beside the ISO. The backup timer beside this one already read it this
# way, so the two halves of one file now agree.
#
# `-` so a missing file is not a unit failure: the script checks for it
# and says so in words, which is the better message.
EnvironmentFile=-/var/lib/kuma/secrets/restore.env
ExecStart=/usr/libexec/kuma-restore
TimeoutStartSec=infinity

[Install]
WantedBy=multi-user.target
"#;

const SNAPSHOT_SERVICE: &str = r#"[Unit]
Description=Snapshot the declared btrfs subvolume

[Service]
Type=oneshot
ExecStart=/usr/libexec/kuma-snapshot
CPUWeight=25
IOWeight=25

[Install]
WantedBy=multi-user.target
"#;

/// Persistent so a laptop that was asleep at the appointed hour still
/// gets its snapshot, and jittered so a fleet doesn't stampede a shared
/// disk at the top of the hour.
fn snapshot_timer(interval: &str) -> String {
    format!(
        "[Unit]\nDescription=Scheduled btrfs snapshots\n\n[Timer]\nOnCalendar={interval}\nPersistent=true\nRandomizedDelaySec=5m\n\n[Install]\nWantedBy=timers.target\n"
    )
}

/// The script with this declaration's retention baked in. Validation has
/// already restricted every substitution to a conservative alphabet.
fn snapshot_script(config: &Config) -> String {
    SNAPSHOT_SCRIPT
        .replace("{target}", &config.snapshots.target)
        .replace("{keep_recent}", &config.snapshots.keep_recent.to_string())
        .replace("{keep_daily}", &config.snapshots.keep_daily.to_string())
}

/// Toggle screen recording: wf-recorder to ~/Videos, notifications on
/// both edges. SIGINT lets wf-recorder finalize the file properly.
const RECORD_SCRIPT: &str = r#"#!/usr/bin/bash
set -u
if pgrep -x wf-recorder >/dev/null; then
    pkill -INT -x wf-recorder
    notify-send "Recording stopped" "Saved in ~/Videos"
else
    dir="$HOME/Videos"
    mkdir -p "$dir"
    f="$dir/recording-$(date +%Y%m%d-%H%M%S).mp4"
    notify-send "Recording started" "Mod+Alt+R stops it."
    exec wf-recorder -f "$f"
fi
"#;

/// Convergence, not just installation: an app the declaration installed
/// and no longer names is removed, so deleting a line in kuma.toml has
/// the same authority as adding one. That authority covers applications;
/// the unused-runtime prune at the end is the one exception and says so
/// where it is argued below.
///
/// Authority is tracked explicitly, in a state file of what the
/// declaration installed, exactly as the brew sync does — and for the
/// same reason, now that scope can no longer stand in for it. This once
/// removed every undeclared *system* app, reading `--system` as "kuma
/// put it here" and `--user` as "the owner did". That proxy held only
/// while nothing else installed system-wide, and a Flatpak store breaks
/// it: Bazaar's flatpak build talks to the system installation through
/// SystemHelper, so apps a person deliberately installed looked exactly
/// like drift and were silently uninstalled hours later. Convergence now
/// takes back only what it gave. Everything else on the machine is the
/// owner's, and `kuma capture` offers to write it down.
///
/// The uninstall tolerates failure because the state file can name an
/// app the owner already removed by hand, which is not an error.
///
/// **The prune at the end reaches past the declaration, and this is the
/// sentence saying so.** `flatpak uninstall --unused` removes any runtime
/// no installed app needs, including one somebody installed on purpose.
/// It is kept because a runtime nothing references is dead weight of the
/// kind an image-based system exists to avoid, and because a runtime is
/// infrastructure rather than a choice: reinstalling one is a download,
/// not a decision. The brew converger makes the same trade one line from
/// its own end and already admits it; this did not, while the docstring
/// above claimed convergence takes back only what it gave. The claim is
/// true of applications, which is what it was written about.
///
/// Membership and currency are different questions, and only the first
/// belongs to the declaration. Convergence decides what exists; the
/// update decides how old it is, and it is deliberately unscoped. An app
/// the owner installed through the store is theirs to keep, but leaving
/// it to rot is not respect, it is an unpatched browser. The same call
/// covers runtimes, which the declared install reaches only when a
/// declared app happens to demand a newer one.
///
/// Ordering is load-bearing. The state file is written before the
/// update, so a failed update (a flaky network, one broken remote)
/// cannot leave authority tracking behind and silently strand a removal
/// until the next successful run. Pruning comes last so the runtimes the
/// update orphans go in the same pass.
///
/// Both download paths retry without static deltas, because a remote can
/// serve a delta this machine refuses. ostree caps how large a
/// decompressed delta part may be, the cap is computed per machine, and
/// a delta over it fails byte-identically on every retry: Flathub's
/// Firefox did exactly this and the unit failed six times before systemd
/// stopped trying. Retrying is what the whole download is for. It costs
/// bandwidth on the path that already failed and nothing anywhere else,
/// since the second pass sees only what is still pending.
///
/// The retry belongs on the install too, not just the update. `xargs`
/// exits 123 when the command it ran failed, which is the status that
/// failure wore, and `--or-update` means the declared-install pass is
/// where an app already present gets its new version. Fixing only the
/// update line would have left the failure exactly where it was.
const FLATPAK_SYNC_SCRIPT: &str = r#"#!/usr/bin/bash
set -euo pipefail
declared=/usr/lib/kuma/flatpaks
state=/var/lib/kuma/flatpaks-installed
mkdir -p /var/lib/kuma
[ -f "$state" ] || : > "$state"
install_declared() {
    xargs -r -a "$declared" flatpak install --system --assumeyes --noninteractive --or-update "$@" flathub
}
install_declared || install_declared --no-static-deltas
while read -r app; do
    grep -qxF "$app" "$declared" \
        || flatpak uninstall --system --assumeyes --noninteractive "$app" || true
done < "$state"
cp "$declared" "$state"
flatpak update --system --assumeyes --noninteractive \
    || flatpak update --system --assumeyes --noninteractive --no-static-deltas
flatpak uninstall --system --unused --assumeyes --noninteractive
"#;

/// Doctor prints a `remote-add` for the same address, so a move here
/// must not leave it sending people somewhere the image disagrees with.
pub const FLATHUB_URL: &str = "https://dl.flathub.org/repo/flathub.flatpakrepo";

/// `kuma vm` passes the host's timezone over qemu's fw_cfg channel
/// (bootc-image-builder silently ignores [customizations.timezone] for
/// qcow2 builds). This service adopts it at boot; on real hardware the
/// fw_cfg key doesn't exist and the service is a no-op.
const VM_TZ_SERVICE: &str = r#"[Unit]
Description=Adopt the host timezone passed by kuma vm
Before=systemd-user-sessions.service

[Service]
Type=oneshot
ExecStart=/usr/libexec/kuma-vm-timezone

[Install]
WantedBy=multi-user.target
"#;

const VM_TZ_SCRIPT: &str = r#"#!/usr/bin/bash
set -euo pipefail
modprobe -q qemu_fw_cfg 2>/dev/null || exit 0
raw=/sys/firmware/qemu_fw_cfg/by_name/opt/org.kuma.tz/raw
[ -r "$raw" ] || exit 0
tz=$(tr -d '\0' < "$raw")
[ -e "/usr/share/zoneinfo/$tz" ] || exit 0
ln -sfn "../usr/share/zoneinfo/$tz" /etc/localtime
"#;

/// Makes `/var/home` a btrfs subvolume while it is still empty, which is
/// the only window there is.
///
/// `[snapshots]` takes a btrfs snapshot, and a snapshot is of a
/// subvolume. On a machine kuma installed, `/var/home` was an ordinary
/// directory inside the deployment's `/var`: the snapshot script exits 0
/// on a target it cannot snapshot, so the unit succeeded, the timer
/// stayed active, and the machine reported itself healthy while taking
/// nothing, forever. Anaconda-installed machines were never affected,
/// which is how it went unnoticed.
///
/// At boot rather than at install, for two reasons that only showed up
/// by trying the install first. The image ships no `/var/home`, since
/// `rpm-ostree-0-integration.conf` has tmpfiles create it on every boot,
/// so at install time there is nothing to replace. And the directory
/// tmpfiles creates already carries the right SELinux label, which a
/// subvolume made by the installer would have to name for itself.
///
/// Empty is the whole of the safety. Moving home directories is not
/// something a boot-time unit may do, so a machine that already has one
/// is left exactly as it is and `kuma doctor` reports it instead.
const HOME_SUBVOL_SERVICE: &str = r#"[Unit]
Description=Give /var/home its own btrfs subvolume while it is empty
ConditionPathExists=/run/ostree-booted
# The same slot systemd-tmpfiles-setup runs in, one step later, and the
# reason is a race that took two intermittent failures to see as one.
#
# Making /var/home a subvolume means `rmdir` then `btrfs subvolume
# create`, and between those two commands the directory does not exist.
# Twenty five units on a desktop image carry ProtectHome, which makes
# systemd mount something over /var/home before the service starts, so
# the window has two losers. If this converger wins it, firewalld starts
# into a missing directory and dies with 226/NAMESPACE. If the sandboxed
# unit wins it, the mount pins the directory, `rmdir` fails with EBUSY,
# this unit dies under set -e, and /var/home stays an ordinary directory
# forever, which costs [snapshots] everything and says nothing.
#
# Listing those units in Before= was the first attempt and it was the
# wrong shape: they do not write to /var/home, systemd merely binds it
# for them, and there are twenty five of them and counting. Running
# before sysinit.target closes the window against all of them at once,
# including systemd-resolved, -timesyncd and -userdbd, which start too
# early for any multi-user.target ordering to matter.
DefaultDependencies=no
RequiresMountsFor=/var
After=systemd-tmpfiles-setup.service
Before=sysinit.target
Conflicts=shutdown.target
Before=shutdown.target

[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=/usr/libexec/kuma-home-subvol

[Install]
WantedBy=sysinit.target
"#;

const HOME_SUBVOL_SCRIPT: &str = r#"#!/usr/bin/bash
# Generated by kuma. Takes the target as an argument so the branch that
# does the work is reachable in a test: on a converged machine it
# returns at the second line forever.
set -euo pipefail
target=${1:-/var/home}

# Every exit says why. Declining is the common case and the right one on
# every boot after the first, but it is also what a missed first boot
# looks like, and those were indistinguishable while both were a silent
# exit 0: the unit succeeds, the machine boots healthy, and the only
# evidence is an inode nobody looks at.
say() { echo "kuma-home-subvol: $*"; }

# Which command died, not just that one did. This unit fails on a small
# fraction of first boots, leaving /var/home an ordinary directory
# forever, and until this trap existed the only trace was `systemctl
# is-failed` saying "failed": the work below is four commands and
# `set -e` names none of them. Costs nothing on the boots that succeed.
trap 'say "FAILED at line $LINENO running: $BASH_COMMAND"' ERR

[ -d "$target" ] || { say "$target does not exist yet; nothing to do"; exit 0; }

# head -1 because findmnt prints a line per mount when something is
# stacked at or under the path, and a two-line answer never equals
# "btrfs", which would decline on a machine that is in fact btrfs.
fstype=$(findmnt -no FSTYPE -T "$target" 2>/dev/null | head -1 || true)
[ "$fstype" = btrfs ] \
    || { say "$target is on ${fstype:-an unknown filesystem}, not btrfs; nothing to do"; exit 0; }

# Inode 256 is every btrfs subvolume root, and asking this way needs no
# privilege and no btrfs command. Already one: nothing to do, on every
# boot after the first.
[ "$(stat -c %i "$target")" = 256 ] && { say "$target is already a subvolume"; exit 0; }

# Only while nothing lives there. This runs before the account converger
# and before any login, so on a first boot it is empty; on any later one
# it holds somebody's home directory and this must leave it alone.
if [ -n "$(ls -A "$target")" ]; then
    say "$target already holds $(ls -A "$target" | tr '\n' ' ')"
    say "leaving it alone: making it a subvolume now would mean moving what is in it"
    exit 0
fi

# Carried across rather than assumed: tmpfiles made this directory with
# the mode and label the system expects, and the subvolume replacing it
# should be indistinguishable.
mode=$(stat -c %a "$target")
label=$(stat -c %C "$target" 2>/dev/null || echo '?')
rmdir "$target"
btrfs subvolume create "$target" >/dev/null
chmod "$mode" "$target"
if [ "$label" != '?' ]; then chcon "$label" "$target"; fi
"#;

/// Runs before any login (console, greeter, ssh) so the declared account
/// exists the first time a login prompt appears.
const USER_SYNC_SERVICE: &str = r#"[Unit]
Description=Converge the declared user account
Before=systemd-user-sessions.service greetd.service

[Service]
Type=oneshot
ExecStart=/usr/libexec/kuma-user-sync

[Install]
WantedBy=multi-user.target
"#;

/// Creation happens at boot, not image build: /home is machine state
/// (/var/home), so an image-built home directory would exist only on
/// disk-image installs and be missing after a `bootc switch`. The
/// password hash applies only at creation — passwords are machine state.
/// Groups converge additively so imperative grants (docker, libvirt)
/// made on the machine survive.
/// Two sources, and which one wins is the whole point.
///
/// /usr is the image and /etc is machine state, the line kuma draws
/// everywhere else, and a user belongs on both sides of it depending on
/// who the image is for. On a personal image the account is declaration:
/// it is baked, and every machine built from that image is meant to have
/// it. On a *published* image it cannot be, because the image is shared
/// and the person is not — so a published image declares no [user] at
/// all, and a machine installed from one would have no account, no root
/// password, and no way in.
///
/// So an installer writes /var/lib/kuma/user on the target and this prefers
/// it. Same fields, same converger, machine state instead of image
/// content. It also gives a machine that rebased onto kuma a way to
/// declare an account without rebuilding anything.
const USER_SYNC_SCRIPT: &str = r#"#!/usr/bin/bash
set -euo pipefail
# shellcheck disable=SC1091  # both sources are written by kuma, not present here
# Machine state wins over image content. Neither is an error: this unit
# ships in every image now, including ones that declare no account,
# because a published image is exactly that and still needs the converger
# present for whatever an installer writes later.
#
# /var, not /etc, and the difference is not cosmetic. bootc populates /var
# from the image once at install and never touches it again, which is
# exactly what install-time state needs. /etc is three-way merged on every
# update: a file the installer shipped as image content sits in that
# deployment's /usr/etc unmodified, and the published image it updates
# from has no such file, so the merge deletes it. The account would
# survive (it is created at first boot) while the thing describing it
# vanished, and the machine would quietly stop matching what was written
# down.
if [ -f /usr/lib/kuma/user ]; then
    . /usr/lib/kuma/user
fi
if [ -f /var/lib/kuma/user ]; then
    # The installer answers for the person; the image still answers for
    # itself. Clearing the account keys first means an image that
    # declared a user cannot lend its name, password or groups to the
    # account somebody typed at install time. KUMA_SHELL is the one key
    # deliberately left to carry over: [system].shell describes the
    # image, not a person, so an image that installs fish says so once
    # and an installer that was told nothing about shells inherits it.
    unset KUMA_USER KUMA_PASSWORD_HASH KUMA_GROUPS
    . /var/lib/kuma/user
fi

# Written by `kuma install`, for the same reason and with one extra step.
# /etc/hostname IS image content (every kuma image bakes one), so writing
# it here makes it a local modification, which is what survives the merge.
# Shipping it in the installer's layer instead would revert to the
# published image's hostname on the first update.
if [ -f /var/lib/kuma/hostname ]; then
    want=$(cat /var/lib/kuma/hostname)
    if [ -n "$want" ] && [ "$(cat /etc/hostname 2>/dev/null)" != "$want" ]; then
        echo "$want" > /etc/hostname
        hostnamectl set-hostname "$want" 2>/dev/null || true
    fi
fi

[ -n "${KUMA_USER:-}" ] || exit 0
if ! id -u "$KUMA_USER" &>/dev/null; then
    args=(-m)
    [ -n "${KUMA_SHELL:-}" ] && args+=(-s "$KUMA_SHELL")
    useradd "${args[@]}" "$KUMA_USER"
    if [ -n "${KUMA_PASSWORD_HASH:-}" ]; then
        echo "$KUMA_USER:$KUMA_PASSWORD_HASH" | chpasswd -e
    fi
fi
[ -n "${KUMA_SHELL:-}" ] && usermod -s "$KUMA_SHELL" "$KUMA_USER"
for group in ${KUMA_GROUPS:-}; do
    usermod -aG "$group" "$KUMA_USER"
done
"#;

/// Declared keys live in root-owned image content, consulted alongside —
/// never instead of — the user's own ~/.ssh/authorized_keys.
const SSHD_KUMA_KEYS: &str = r#"# Kuma-declared keys, alongside the user's own.
AuthorizedKeysFile .ssh/authorized_keys /etc/kuma/keys/%u
"#;

/// The counter/fallback half of boot health is BOOTLOADER config —
/// written once at install time and never rewritten by bootupd — so a
/// machine installed before greenboot entered the image has a grub.cfg
/// that never decrements boot_counter. greenboot userspace relies on
/// grub for the countdown, and without it a failing update reboots
/// forever instead of falling back (observed empirically: the counter
/// sat at 3 across 40+ consecutive boots). grub sources
/// $prefix/custom.cfg at the end of bootupd's static config — after
/// blscfg registers the deployments, before the menu shows — which is
/// the sanctioned hook for exactly this. Converged, not just written:
/// if grub.cfg ever gains native boot_counter handling (fresh installs
/// have it; bootupd may learn to refresh), the block is removed so the
/// counter is never decremented twice per attempt.
const BOOT_HEALTH_SYNC_SCRIPT: &str = r#"#!/usr/bin/bash
set -euo pipefail
cfg=/boot/grub2/grub.cfg
custom=/boot/grub2/custom.cfg
begin='# >>> kuma boot-health >>>'
end='# <<< kuma boot-health <<<'
[ -f "$cfg" ] || exit 0

restore=""
if findmnt -n -o OPTIONS /boot 2>/dev/null | grep -qw ro; then
    mount -o remount,rw /boot
    restore="ro"
fi
finish() { [ "$restore" = ro ] && mount -o remount,ro /boot || true; }
trap finish EXIT

if grep -q boot_counter "$cfg"; then
    # The bootloader counts natively; drop our block if we ever wrote one.
    if [ -f "$custom" ] && grep -qF "$begin" "$custom"; then
        sed -i "\|^$begin|,\|^$end|d" "$custom"
        [ -s "$custom" ] || rm -f "$custom"
    fi
    exit 0
fi
if [ -f "$custom" ] && grep -qF "$begin" "$custom"; then
    exit 0
fi
cat >> "$custom" <<'EOF'
# >>> kuma boot-health >>>
# Managed by kuma-boot-health-sync; do not edit between these markers.
# Boot-counter fallback for bootloaders installed before greenboot
# entered the image: decrement boot_counter each attempt, and boot the
# previous deployment (menu entry 1) when it runs out. Same logic
# greenboot ships for fresh installs via bootupd's static config.
insmod increment
# Check if boot_counter exists and boot_success=0 to activate this behavior.
if [ -n "${boot_counter}" -a "${boot_success}" = "0" ]; then
  # if countdown has ended, choose to boot rollback deployment,
  # i.e. default=1 on OSTree-based systems.
  if  [ "${boot_counter}" = "0" -o "${boot_counter}" = "-1" ]; then
    set default=1
    set boot_counter=-1
  # otherwise decrement boot_counter
  else
    decrement boot_counter
  fi
  save_env boot_counter
fi

# Reset boot_success for current boot
set boot_success=0
save_env boot_success
# <<< kuma boot-health <<<
EOF
"#;

/// Anaconda writes a `/` line into /etc/fstab describing the root as the
/// filesystem it installed onto (btrfs here). On a bootc machine the root
/// is a composefs overlay, so systemd-remount-fs reads that line, tries to
/// remount `/` with those options, and the kernel refuses:
///
///   mount: /: fsconfig() failed: overlay: No changes allowed in reconfigure.
///
/// Nothing downstream depends on it, which is why this was filed as
/// cosmetic and carried a known-benign downgrade in doctor for months. The
/// reason it earns a converger anyway: nothing else ever rewrites that
/// file. It is machine state written once by the installer, so the failure
/// is permanent on every machine kuma installs, and the only cure anyone
/// had was editing the file by hand on each one. Same category as the
/// bootloader's boot counter above, and fixed the same way.
///
/// Deliberately one-directional, unlike the boot-counter block. Leaving
/// the line commented is safe on any bootc machine: the root is already
/// mounted by the initrd, and that line drives nothing but this remount.
/// Uncommenting it again would not be safe, because the only reason to do
/// so would be a detection mistake, and the cost of that mistake is a
/// machine that fails a mount at boot. So the marker records what was done
/// and a human can reverse it; the script never will.
/// Takes the file as an argument, defaulting to the real one, for the same
/// reason inspect.rs's scan_etc takes its roots: on a machine that is
/// already converged the editing branch never runs, so without a way to
/// point it at a fixture this would ship having never once executed the
/// only lines that matter. The unit passes no argument.
// r##: the awk below prints lines starting `"#`, which closes an r#" literal.
const FSTAB_SYNC_SCRIPT: &str = r##"#!/usr/bin/bash
set -euo pipefail
fstab=${1:-/etc/fstab}
[ -f "$fstab" ] || exit 0
# The whole justification for the edit is that the mounted root is not
# what the line claims. Ask the kernel rather than assuming: on anything
# that is not a composefs overlay this exits having done nothing.
[ "$(findmnt -n -o FSTYPE / 2>/dev/null)" = overlay ] || exit 0

# awk, not sed: the target is "the line whose second field is /", which is
# a field test, and writing it as a regex over the whole line is how you
# end up commenting out /boot or a subvolume that merely mentions root.
# Already-commented lines cannot match, so this is idempotent without
# needing to look for its own marker.
new=$(mktemp)
awk '
$1 !~ /^#/ && $2 == "/" {
    print "# Commented out by kuma-fstab-sync: this machine boots a composefs"
    print "# overlay, and systemd-remount-fs fails every boot trying to remount"
    print "# / with the options below. Uncomment only if / stops being an overlay."
    print "#" $0
    next
}
{ print }
' "$fstab" > "$new"

if cmp -s "$fstab" "$new"; then
    rm -f "$new"
    exit 0
fi
# cat into the existing file rather than mv over it: this keeps the inode,
# the mode, and the SELinux label that a rename from /tmp would replace.
cat "$new" > "$fstab"
rm -f "$new"
# The generated mount units come from this file, so tell systemd it moved.
# Nothing is remounted here: the failing remount is the thing being
# prevented, and this boot has already had it. Skipped when running
# against a fixture, which has no business reloading the host's systemd.
[ -n "${1:-}" ] || systemctl daemon-reload
"##;

/// multi-user, not early boot, and the cost is understood: systemd-remount-fs
/// runs in sysinit, so the machine that installs today still fails this unit
/// once and comes up clean on every boot after. Ordering this before it
/// instead would need DefaultDependencies=no and a hand-built ordering into
/// the middle of early boot, where a cycle does not produce a failed unit,
/// it produces a machine that does not boot. That is a bad trade against a
/// failure whose entire consequence is a red line in `systemctl --failed`.
/// What the policy has to be told, or a swapfile is unusable for the one
/// thing it was made for.
///
/// Measured on a real machine rather than reasoned about. A swapfile at
/// `/var/swap/swapfile` gets `var_t` from the policy's own default, and
/// `systemd-sleep` cannot read a `var_t` file:
///
/// ```text
/// avc: denied { read } for comm="systemd-sleep" name="swapfile"
///      scontext=systemd_sleep_t tcontext=var_t tclass=file
/// systemd-sleep: Failed to find location to hibernate to: Permission denied
/// ```
///
/// `systemd-logind` is denied the same way, which is why `CanHibernate`
/// answers "Access denied" on such a machine rather than yes or no. The
/// machine has a correct swapfile, correct kernel arguments, and cannot
/// hibernate.
///
/// So `restorecon` alone is not the fix: run against the stock policy it
/// produces exactly the `var_t` that fails. The path has to be declared
/// a swapfile first, which is what this does, and then `restorecon`
/// gives `swapfile_t` and hibernation works. Written as policy rather
/// than as a `chcon` somewhere because a rule survives a full relabel
/// and a hand-applied label does not.
const SWAP_FCONTEXT: &str = "\
# Added by kuma. Without this line the policy's default for a file at
# this path is var_t, and systemd-sleep cannot read a var_t file, so the
# machine gets a swapfile it can never hibernate into.
/var/swap/swapfile\t--\tsystem_u:object_r:swapfile_t:s0
";

/// Applying the rule above on the machine that has the file.
///
/// A unit rather than something the installer does, because the
/// installer cannot. It writes the swapfile from whatever host is
/// running it, and setting an SELinux label means writing a
/// `security.selinux` xattr, which a host with SELinux disabled cannot
/// do at all: CI installs from an Ubuntu runner. The label therefore has
/// to be applied by the machine, where the policy lives.
///
/// `restorecon` rather than a tmpfiles `z` line, which was tried first
/// and cannot do it: tmpfiles runs in a domain with no `relabelto` for
/// `swapfile_t` and fails with `Unable to fix SELinux security context
/// of /var/swap/swapfile: Operation not permitted`.
///
/// **Recursive, from the mount point, and that is the whole of a bug this
/// shipped with.** The first version relabelled the file alone. That
/// worked, and the machine still could not hibernate, because the
/// subvolume's own root inode carries no label at all when the installer
/// makes it, and `systemd-sleep` has to *search* the directory before it
/// can read anything inside:
///
/// ```text
/// avc: denied { search } for comm="systemd-sleep" name="swap"
///      scontext=systemd_sleep_t tcontext=unlabeled_t tclass=dir
/// ```
///
/// So there are two labels to fix, not one: `var_t` on the directory,
/// which is the policy's default and only needs applying, and
/// `swapfile_t` on the file, which needs the rule above first.
///
/// Conditioned on both the file and SELinux, so it is silently skipped
/// on the machines that have no swapfile, which is most of them. The
/// condition names the file rather than the directory on purpose: an
/// empty `/var/swap` is not a machine that wanted to hibernate.
const SWAP_LABEL_SERVICE: &str = r#"[Unit]
Description=Give the hibernate swapfile the SELinux label systemd-sleep needs
Documentation=man:restorecon(8)
ConditionSecurity=selinux
ConditionPathExists=/var/swap/swapfile
After=var-swap.mount

[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=/usr/sbin/restorecon -RF /var/swap

[Install]
WantedBy=multi-user.target
"#;

const FSTAB_SYNC_SERVICE: &str = r#"[Unit]
Description=Converge Anaconda's fstab root line for a composefs root
ConditionPathExists=/run/ostree-booted

[Service]
Type=oneshot
ExecStart=/usr/libexec/kuma-fstab-sync

[Install]
WantedBy=multi-user.target
"#;

/// The boot menu's titles, converged at the only two moments they can be
/// made right.
///
/// `Before=ostree-finalize-staged.service` is the whole design, and it
/// reads backwards. systemd stops units in the reverse of the order it
/// starts them, and the rotation this exists to follow happens in
/// ostree's own ExecStop, at shutdown: the deployment symlinks move onto
/// the new order and the entry files, whose titles ostree does not
/// compare, are left as they were. Ordering *before* finalize-staged at
/// start is therefore what puts this *after* it at stop, with the
/// deployments moved and the titles not yet. ostree wires its own hold
/// unit into this slot the same way; `After=` on that hold unit is what
/// keeps this pass inside the window where /boot is still mounted.
///
/// Doing it when kuma stages an image instead would write titles for an
/// arrangement that has not happened yet: `kuma switch` only stages, and
/// a staged deployment becomes the default at shutdown, or never does.
///
/// ExecStart is the same idempotent pass at boot. It costs a few file
/// reads and it covers the machine that lost power before its shutdown
/// finished, the first boot on an image that predates this unit, and
/// `bootc rollback` rotating the deployments mid-session.
///
/// DefaultDependencies=no, with Conflicts= and Before= on final.target
/// spelled out, because this has to stop in the late slot
/// finalize-staged stops in. The ordinary slot carries an implicit
/// Before=shutdown.target, which contradicts stopping after a unit that
/// stops at final.target, and systemd resolves a contradiction like that
/// by dropping one of the two rules rather than by saying anything.
const BOOT_TITLES_SERVICE: &str = r#"[Unit]
Description=Name the boot menu's entries after the deployments they boot
ConditionPathExists=/run/ostree-booted
DefaultDependencies=no
RequiresMountsFor=/boot /sysroot
After=local-fs.target
After=ostree-finalize-staged-hold.service
Before=ostree-finalize-staged.service
Before=basic.target final.target
Conflicts=final.target

[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=/usr/bin/kuma boot-titles
ExecStop=/usr/bin/kuma boot-titles
TimeoutStartSec=30s
TimeoutStopSec=2m

[Install]
WantedBy=basic.target
"#;

/// Before the health check only for tidiness — the hook matters at the
/// NEXT grub run, so any point in this boot converges in time.
const BOOT_HEALTH_SYNC_SERVICE: &str = r#"[Unit]
Description=Converge the bootloader's boot-counter fallback hook
# The same gate its sibling kuma-fstab-sync carries, and the one
# liveiso.rs already credited this unit with having. It did not: it
# skipped on live media only because the grub config it looks for is not
# there, which is an accident rather than a decision, and an accident
# that a future change to the script could remove without anyone noticing.
ConditionPathExists=/run/ostree-booted
RequiresMountsFor=/boot
Before=greenboot-healthcheck.service

[Service]
Type=oneshot
ExecStart=/usr/libexec/kuma-boot-health-sync

[Install]
WantedBy=multi-user.target
"#;

/// greenboot required check for desktop images. Runs from greenboot's
/// health-check oneshot, which blocks multi-user.target completion — so
/// it must poll display-manager.service directly and must NEVER wait on
/// graphical.target: graphical waits for multi-user, which waits for
/// this very check. That deadlock is one comment away, hence the shout.
const GREETER_CHECK: &str = r#"#!/usr/bin/bash
# A desktop boot is healthy when the greeter is on screen, not when
# multi-user is reached: a broken compositor or greeter update boots
# "fine" into a black screen, and that is exactly the regression this
# check exists to roll back. display-manager.service is the alias both
# greetd (niri) and cosmic-greeter carry; it starts in parallel with
# this check and its Restart= may be mid-retry, so only the deadline
# decides. DO NOT wait on graphical.target here: it waits for
# multi-user.target, which waits for this check: instant deadlock.
#
# Active once is not the same as running. greetd's Restart= makes a
# crash loop look healthy to a single sample: a machine whose greeter
# died five times in four seconds passed this check green, because the
# poll happened to land inside one of the retries. So the greeter has to
# be up, and still up a moment later, and a unit that has given up
# entirely is a failure now rather than in two minutes.
set -u
deadline=$(( SECONDS + 120 ))
settle=5
while true; do
    if systemctl --quiet is-failed display-manager.service; then
        echo "display-manager.service failed to start"
        exit 1
    fi
    if systemctl --quiet is-active display-manager.service; then
        sleep "$settle"
        if systemctl --quiet is-active display-manager.service; then
            exit 0
        fi
        echo "display-manager.service did not stay up; still waiting"
    fi
    if (( SECONDS >= deadline )); then
        echo "display-manager.service not running after 120s"
        exit 1
    fi
    sleep 3
done
"#;

/// Appended to niri's full default config (copied from the package) so the
/// stock keybindings survive; niri configs replace defaults entirely.
const NIRI_EXTRAS: &str = r##"

// adw-gtk3-dark is a real directory theme, so the plain name works
// here. (Stock Adwaita needed GTK_THEME=Adwaita:dark, because the
// settings-layer name "Adwaita-dark" loads as a nonexistent directory
// theme and falls back to light.) This variable outranks gsettings, so
// it has to move with the other three or GTK3 apps keep the old theme
// whatever dconf says.
environment {
    GTK_THEME "adw-gtk3-dark"
    XCURSOR_THEME "Adwaita"
    XCURSOR_SIZE "24"
    // Where the shell reads kuma's config from. Undocumented in
    // `noctalia --help` and found by grepping the binary: it redirects
    // config-home wholesale, and it is the only thing that does.
    // /etc/xdg and XDG_CONFIG_DIRS are both ignored, measured. Without
    // this kuma cannot bake the desktop's look at all, so the build
    // asserts the config survives to `config export merged`.
    //
    // This is HALF of the answer, and it stopped being the half that
    // matters in 0.17. It reaches what niri spawns: the `noctalia msg`
    // keybinds below, and a terminal where you ask the shell what it is
    // running. The shell itself runs from kuma-shell.service now, and a
    // unit inherits nothing from here, so [`SHELL_SERVICE`] states the
    // same variable and a test holds the two together.
    NOCTALIA_CONFIG_HOME "/usr/lib/kuma"
}

// Kuma session services
spawn-at-startup "/usr/libexec/polkit-mate-authentication-agent-1"
spawn-at-startup "/usr/libexec/kuma-clipboard-bridge"
spawn-at-startup "/usr/libexec/kuma-xsettings"
spawn-at-startup "blueman-applet"
// Automount removable media at the session level.
spawn-at-startup "udiskie"
// The shell is NOT spawned here. It runs as kuma-shell.service, a
// supervised user unit, because a `spawn-at-startup` lands in a
// transient scope and a scope cannot carry Restart=. Every lock this
// machine has goes through that one process: idle, the keybind, and
// lock-before-suspend. When it died, nothing restarted it and nothing
// said so, and the next lid close suspended the machine with an
// unlocked session inside it.
spawn-at-startup "/usr/libexec/kuma-battery-watch"

// Kuma look: rounded windows, quiet neutral focus ring. Window rules are
// additive, so this themes every window without touching the stock layout.
window-rule {
    geometry-corner-radius 8
    clip-to-geometry true
    focus-ring {
        active-color "#b8bec8"
        inactive-color "#333940"
    }
}

// The machine's own deltas, and the LAST thing in this file on purpose:
// niri merges includes positionally, so whatever is here wins over
// everything above it. Without this a user who wants one setting changed
// must copy the whole config into ~/.config/niri/config.kdl, which then
// shadows /etc forever and goes stale the moment an image update rewrites
// the config it was copied from. A few lines in local.kdl cost nothing and
// never expire. optional=true is what lets the build's `niri validate` (and
// every machine that never writes one) pass with the file absent; niri logs
// a reload warning in that case and carries on. Both optional includes and
// ~ expansion landed in niri 26.04, so this needs that or newer.
include optional=true "~/.config/niri/local.kdl"
"##;

/// The shell, supervised.
///
/// niri's `spawn-at-startup` hands the process to systemd as a transient
/// SCOPE, and a scope cannot carry `Restart=`: measured on a booted
/// machine, where the shell sat in `app-niri-noctalia-1441.scope` with
/// nothing watching it. That matters more here than for an ordinary
/// panel, because all three of this desktop's lock paths run through
/// that one process. A crash took the bar with it, which you notice, and
/// the idle lock and the sleep inhibitor, which you do not.
///
/// `PartOf=` and `WantedBy=graphical-session.target` because that target
/// is real in this session (measured active) and niri imports
/// WAYLAND_DISPLAY and NIRI_SOCKET into the user manager's environment
/// before it, so the unit starts with what it needs.
///
/// `Restart=always` rather than `on-failure`: a shell that exits zero has
/// still taken the lock screen with it.
///
/// `Environment=` because a unit inherits nothing from niri's
/// `environment` block, and 0.17 shipped without it: the first boot of
/// the supervised shell came up as stock noctalia, welcome screen and
/// all, because the one variable that points it at kuma's config was
/// stated only in a file that no longer applied to it. The check that
/// should have caught it read the niri config, which still said the
/// right thing about a process it no longer started.
const SHELL_SERVICE: &str = r#"[Unit]
Description=Noctalia, the kuma desktop shell
PartOf=graphical-session.target
After=graphical-session.target

[Service]
Type=simple
# The variable that makes this kuma's desktop rather than noctalia's.
# niri's `environment` block reaches the processes NIRI spawns, and the
# shell stopped being one of them the moment it moved into this unit, so
# the same variable has to be stated here or nothing states it: /etc/xdg
# and XDG_CONFIG_DIRS are both ignored by the shell, measured. Without
# it the desktop comes up on stock defaults, which is a wider bar, no
# wallpaper-derived palette, and the welcome screen kuma turns off.
# Measured on a booted 0.17 machine, where the running shell's environ
# held no NOCTALIA_ variable at all.
Environment=NOCTALIA_CONFIG_HOME=/usr/lib/kuma
# Out of the same block and lost the same way. The shell draws its own
# surfaces, so the cursor over the bar and the lock screen is themed by
# these or by nothing.
Environment=XCURSOR_THEME=Adwaita
Environment=XCURSOR_SIZE=24
ExecStart=/usr/bin/noctalia
Restart=always
RestartSec=1
Slice=session.slice

[Install]
WantedBy=graphical-session.target
"#;

/// Do not sleep into an unlocked session.
///
/// The residual case after supervision: the shell is gone at the moment
/// the lid closes, so its logind sleep inhibitor is gone too and the
/// machine suspends with the desktop on screen. Supervision makes that
/// rare; this makes it loud instead of silent.
///
/// Terminating the session is the honest move rather than a harsh one:
/// with the shell dead that session has no bar, no lock screen and no
/// idle handling, so it is already unusable. What changes is that the
/// machine sleeps showing a greeter instead of sleeping showing your
/// work.
///
/// Ordered `Before=sleep.target` and pulled in by it, so it runs on the
/// way down and on every path into sleep rather than only on the lid.
const SLEEP_GUARD_SERVICE: &str = r#"[Unit]
Description=Refuse to suspend a kuma desktop into an unlocked session
Before=sleep.target
StopWhenUnneeded=yes

[Service]
Type=oneshot
ExecStart=/usr/libexec/kuma-sleep-guard

[Install]
WantedBy=sleep.target
"#;

const SLEEP_GUARD: &str = r#"#!/usr/bin/bash
set -euo pipefail

# Only on a machine kuma gave a shell to. A server has no session to
# protect and no shell to miss.
[ -f /etc/niri/config.kdl ] || exit 0

# A graphical session on the seat, and the account that owns it. Anything
# else (no session, a TTY login, a machine at the greeter) is nothing to
# do here.
while read -r id _rest; do
    # Only the session id is read positionally, and everything else is
    # ASKED. `loginctl list-sessions` has nine columns today and has
    # gained columns before; a guard that reads the wrong one skips
    # silently, and a security guard that skips silently is the exact
    # property this exists to prevent.
    seat=$(loginctl show-session "$id" -p Seat --value 2>/dev/null || true)
    [ "$seat" = "seat0" ] || continue
    type=$(loginctl show-session "$id" -p Type --value 2>/dev/null || true)
    [ "$type" = "wayland" ] || continue
    user=$(loginctl show-session "$id" -p Name --value 2>/dev/null || true)
    [ -n "$user" ] || continue
    # The shell owns every lock path on this desktop. If it is running,
    # its own inhibitor already held sleep long enough to lock.
    if pgrep -u "$user" -x noctalia >/dev/null 2>&1; then
        exit 0
    fi
    logger -t kuma-sleep-guard         "the desktop shell is not running in session $id; ending it rather than suspending an unlocked session"
    loginctl terminate-session "$id" || true
done < <(loginctl list-sessions --no-legend 2>/dev/null || true)
"#;

/// Dark by default. Apps learn the preference from the settings portal,
/// which reads org.gnome.desktop.interface from dconf; without it every
/// CSD titlebar and GTK app falls back to light. color-scheme covers
/// GTK4/libadwaita/portal clients, gtk-theme covers GTK3 apps that
/// predate it. A system db sets the default; user settings still win.
const DCONF_PROFILE: &str = "user-db:user\nsystem-db:local\n";
const DCONF_DARK: &str = r#"[org/gnome/desktop/interface]
color-scheme='prefer-dark'
gtk-theme='adw-gtk3-dark'
"#;

/// One bluetooth indicator, not two.
///
/// blueman-applet earns its place as the Bluetooth agent: it answers
/// pairing requests and reconnects devices, and nothing else here does.
/// What it also does is spawn `blueman-tray`, which puts a second
/// bluetooth indicator in the bar's tray beside the bar's own bluetooth
/// widget. The bar then shows one state twice, in two styles, because a
/// tray renders the application's own coloured icon while every other
/// widget is a font glyph, and a tray cannot recolour what it is given.
/// The widget already opens blueman-manager on click, so the tray icon
/// was carrying no capability of its own.
///
/// Written against waybar and still true of the shell that replaced it,
/// which carries `tray` and `bluetooth` in the same group: verified on
/// the booted image, where blueman-applet runs, blueman-tray does not,
/// and one bluetooth glyph reaches the bar.
///
/// Disabling StatusIcon alone does nothing, which is the part worth
/// writing down: ShowConnected declares `__depends__ = ["StatusIcon"]`,
/// and the plugin manager loads a dependency whether or not it was
/// disabled, so the icon comes straight back. ShowConnected exists only
/// to decorate that icon ("adds an indication on the status icon...
/// shows the connections in the tooltip"), so it goes with it and takes
/// nothing else. Both names verified against a running session, after
/// the one-name version was tried and silently did nothing.
const DCONF_BLUEMAN: &str = r#"[org/blueman/general]
plugin-list=['!StatusIcon', '!ShowConnected']
"#;

/// One launch path per session service, and it is kuma's.
///
/// `xdg-desktop-autostart.target` is active in this session, so systemd's
/// xdg-autostart-generator turns every `/etc/xdg/autostart/*.desktop`
/// into a unit. Fedora ships one for blueman and one for the mate polkit
/// agent, and niri-extras.kdl *also* spawns both. They are single
/// instance, so one launch wins and the other quietly loses, and which
/// one wins is a race: measured on one boot, blueman came up under
/// `app-blueman@autostart.service` while the polkit agent came up under
/// niri's own scope. Nothing breaks, but a unit reads `dead` while its
/// program is running, the loser can log noise, and which cgroup owns a
/// process changes from boot to boot.
///
/// kuma's spawn is the one kept, because it is the one this image
/// declares: the alternative depends on the session reaching
/// `xdg-desktop-autostart.target`, and losing the polkit agent that way
/// is invisible until somebody needs an authentication prompt.
///
/// `Hidden=true` rather than masking the generated unit: the generator
/// names the polkit unit
/// `app-polkit\x2dmate\x2dauthentication\x2dagent\x2d1@autostart.service`,
/// and reproducing that escaping in a symlink is a worse thing to depend
/// on than the spec's own "ignore this entry" key. Verified against the
/// running generator, which reports the unit `not-found` with this in
/// place.
fn autostart_off(name: &str) -> String {
    format!("[Desktop Entry]\nType=Application\nName={name}\nHidden=true\n")
}

/// Runs a kuma verb in a terminal window, for the desktop entries in
/// [`crate::seam`].
///
/// **`Terminal=true` is the obvious way and does not work here.** It
/// hands the job to whatever the launcher believes a terminal is, which
/// differs per launcher, is configured separately in several, and is
/// nothing at all in some. kuma knows which terminal it put in the
/// image, so it opens that one, and the entries stay identical on niri
/// and COSMIC because the difference lives here instead of in them.
///
/// The window is held open after the verb exits. A terminal launched as
/// `kitty -e <command>` closes the instant the command returns; every
/// one of these prints something worth reading, and the ones that ask
/// for a password are exactly the ones whose window would otherwise
/// vanish as the password is finished.
pub(crate) const KUMA_LAUNCH: &str = r#"#!/usr/bin/bash
set -euo pipefail

if [ "$#" -eq 0 ]; then
    printf 'usage: kuma-launch <verb> [args...]\n' >&2
    exit 2
fi

# Not a fallback chain into xterm: an image either has the terminal its
# desktop set installed or it has no graphical session to launch from.
terminal=
for candidate in kitty cosmic-term; do
    if command -v "$candidate" >/dev/null 2>&1; then
        terminal=$candidate
        break
    fi
done
if [ -z "$terminal" ]; then
    printf 'kuma-launch: this image ships no terminal to run `kuma %s` in\n' "$1" >&2
    exit 1
fi

# The verb and its arguments are passed to the held script as arguments
# rather than pasted into it. Assembling a command line into a string is
# how an argument with a space in it becomes two arguments, and nothing
# here has to be a string.
# A LOGIN shell. Without `-l` nothing sources /etc/profile.d, so a
# desktop entry runs with a smaller PATH than the same person's terminal
# — on a machine with brew packages, `kuma edit` fell back to vi while
# the nano they installed sat unfound. A launcher that opens a terminal
# should hand over the environment that terminal would have had.
exec "$terminal" -e /usr/bin/bash -lc '
"$@"
status=$?
printf "\n"
[ "$status" -eq 0 ] || printf "exited %s\n" "$status"
printf "[kuma] press enter to close "
read -r _
' kuma-launch kuma "$@"
"#;

/// The media keys, bound in place of niri's stock wpctl binds.
///
/// It used to feed the resulting level to a wob overlay; the shell draws
/// its own OSD from the change now (`[osd.kinds]` covers volume and
/// brightness), so this only makes the adjustment. Still a script rather
/// than the stock binds, because mute has to re-read the level and
/// brightness is `brightnessctl` rather than `wpctl`.
const OSD_SCRIPT: &str = r#"#!/usr/bin/bash
set -euo pipefail
case "$1" in
    volume-up)       wpctl set-volume -l 1.0 @DEFAULT_AUDIO_SINK@ 5%+ ;;
    volume-down)     wpctl set-volume @DEFAULT_AUDIO_SINK@ 5%- ;;
    mute)            wpctl set-mute @DEFAULT_AUDIO_SINK@ toggle ;;
    brightness-up)   brightnessctl -q set +5% ;;
    brightness-down) brightnessctl -q set 5%- ;;
esac
"#;

/// kuma's noctalia configuration, baked into the image.
///
/// Read through `NOCTALIA_CONFIG_HOME` (set in [`NIRI_EXTRAS`]), because
/// noctalia ignores `/etc/xdg` and `XDG_CONFIG_DIRS` entirely. This is
/// the authored layer; `~/.local/state/noctalia/settings.toml` is the
/// person's and overrides it, which is the same shape as everything else
/// kuma bakes: the image states the default and the machine may differ.
///
/// **Two of these are corrections, not taste.** Every
/// `[idle.behavior.*]` ships `enabled = false`, so a stock noctalia
/// never locks on idle at all — a regression against the swayidle line
/// it replaces, and a sharper one since 0.15 gave machines somewhere to
/// hibernate to. `[nightlight]` ships disabled likewise, where kuma ran
/// wlsunset from 07:00 to 20:00.
const KUMA_NOCTALIA: &str = r#"# Generated by kuma. Edit kuma.toml instead.
#
# This is the image's copy. Changing the desktop from its own settings
# writes ~/.local/state/noctalia/settings.toml, which wins over this
# file and which kuma cannot see: nothing reads it, so `kuma diff` will
# still say the machine matches its declaration. `noctalia config export
# merged` is what answers "which of these lines is actually in effect".

# Colours derived from the wallpaper rather than a fixed palette, so a
# person who changes the wallpaper gets a desktop that follows it. The
# built-in palettes are noctalia's taste; this way the one visible
# decision stays kuma's wallpaper.
[theme]
mode = "dark"
source = "wallpaper"

# And the desktop follows that palette, not the shell alone. Each
# template renders the live palette into one application's own config,
# on every palette change and again at startup, so a machine nobody
# retunes still logs in themed (measured by deleting the rendered files
# and restarting the shell). None of it is tied to the wallpaper: point
# [theme] source at a built-in or community palette instead and the same
# files re-render from that.
#
#   - kitty takes every colour it shows from the palette, the sixteen
#     ANSI slots included, through noctalia's own template. That has a
#     cost and it was chosen with the cost on screen: Material You maps
#     every ANSI slot into the palette's hue family, so on a
#     wallpaper-derived palette red renders #ffb4ab, green #afc8ee and
#     blue #e3b9e2, and a diff's + and - become two tints of the same
#     colour. A palette picked by name keeps real hues, Gruvbox's green
#     being #b8bb26. The alternative was a terminal carrying sixteen
#     colours from a palette the machine no longer has, which is what
#     the old fixed set had become the moment the shell started
#     following the wallpaper.
#   - gtk3 is what adw-gtk3 is in the package list for. The template
#     writes ~/.config/gtk-3.0/noctalia.css and an @import into gtk.css,
#     and adw-gtk3 reads those colour names where stock Adwaita ignores
#     them: measured on a booted machine, thunar's background moved to
#     the palette's own surface colour.
#   - gtk4 is here for one reason, and it is not its output. The two
#     share an apply.sh that refuses to touch the GTK theme unless both
#     files exist, so enabling gtk3 alone leaves the hook failing on
#     every palette change. What it renders changes nothing today:
#     libadwaita 1.9 ignores a user stylesheet that redefines its
#     palette, by @define-color, by :root, and by the settings portal's
#     accent colour, all three measured against GNOME 50 flatpaks.
#     Direct CSS rules do land there, so theming libadwaita means kuma
#     authoring rules rather than colours, which is its own change.
#
# The seam to know about: that apply.sh also writes gtk-theme into the
# user's dconf. The value matches what the image sets today, but it is
# now a user setting, and a later image that changes the theme will not
# move a machine that has one.
#
# niri and qt stay out. niri's template has apply.sh CREATE
# ~/.config/niri/config.kdl to hold its include line, and niri takes the
# user's file INSTEAD of /etc/niri/config.kdl rather than merging it: a
# two-line file would shadow every bind, the layout and the startup
# list. qt has no reader here, since neither qt5ct nor qt6ct is in the
# image.
[theme.templates]
builtin_ids = [ "kitty", "gtk3", "gtk4" ]
# Nothing here enables a community template, and the shell still asks
# api.noctalia.dev for their catalog at every startup (two failed
# requests in the log of a machine with none configured). A desktop that
# works offline should not call a vendor to render nothing.
enable_community_templates = false

[shell]
font_family = "Noto Sans"

# No "Welcome to Noctalia" on a kuma machine's first login. kuma already
# decided the things that wizard asks about, and a second vendor's
# onboarding on the first screen is the same incoherence the shell was
# adopted to end. Verified honored from config-home, which is not a
# given here: [wallpaper.default] validates and is ignored from the same
# file.
setup_wizard_enabled = false

[bar.default]
position = "top"
thickness = 32
radius = 12
margin_ends = 10
# The left group is where you are and what you can start; the wallpaper
# picker is neither, and it sat between the two things a person touches
# most. It keeps its panel, bound below.
start = [ "launcher", "workspaces" ]
center = [ "clock" ]
# Notifications, then state, then the control centre, which is where the
# rest of it lives. Three widgets left the bar in 0.16, each a glyph that
# reported nothing: brightness, a control rather than a state and already
# on the media keys; the clipboard, which Mod+Ctrl+V opens; and the
# session buttons, which are one click into the control centre, whose
# header carries the same power glyph.
end = [
    "tray",
    "notifications",
    "network",
    "bluetooth",
    "volume",
    "battery",
    "control-center"
]

[wallpaper]
directory = "/usr/share/backgrounds/kuma"
fill_mode = "crop"

# There is deliberately no [wallpaper.default] here. The key exists and
# `config validate` accepts it, but the shell drops it from config-home
# and keeps its own: measured on a booted VM with a fresh home, where
# `wallpaper-get` answered with noctalia's asset. The image replaces that
# asset instead, see the COPY in generate().

# Icons, no text. The bar is 32px and an SSID or a percentage beside
# every glyph is what turns a bar into a status line.
[widget.network]
show_label = false

[widget.volume]
show_label = false

[widget.battery]
show_label = false

# Replaces wlsunset, which ran on the same schedule and temperatures.
[nightlight]
enabled = true
temperature_day = 6500
temperature_night = 4000

# Replaces swayidle: lock at 15 minutes, screen off a minute later.
# Both ship disabled, so leaving this out is a machine that never locks.
[idle.behavior.lock]
enabled = true
timeout = 900.0

[idle.behavior.screen-off]
enabled = true
timeout = 960.0

# The third clause of the swayidle line that left: `before-sleep`. The
# shell does this by default, so this line changes nothing today and is
# here anyway — every other security-shaped setting in this file is
# pinned because its default was wrong, and a beta that flips this one
# would unlock every kuma machine that suspends, silently. Pinned, and
# asserted.
[lockscreen]
lock_before_suspend = true
"#;

/// System-wide default apps: without associations, opening a PDF or a
/// link from Thunar is app-picker roulette. Flatpak-exported desktop
/// ids for the declared apps, native ids for the in-image tools.
///
/// A `.desktop` file's MimeType= line says an app *can* open a type;
/// this list says which one *wins*. So the entries worth having are the
/// contested types — a type with one claimant resolves to it unaided.
/// Firefox is why most of this list exists: it claims application/pdf,
/// six image types, and four audio/video types, every one of which it
/// would otherwise be free to take from Papers, Loupe, or Celluloid.
/// The in-image contest is inode/directory, which kitty-open.desktop
/// claims alongside thunar.
///
/// text/plain has no entry on purpose: nothing in the image claims it,
/// so a declared editor wins unopposed, and an entry would only pin an
/// app that a declaration is free not to install.
const MIMEAPPS: &str = r#"[Default Applications]
x-scheme-handler/http=org.mozilla.firefox.desktop
x-scheme-handler/https=org.mozilla.firefox.desktop
text/html=org.mozilla.firefox.desktop
application/pdf=org.gnome.Papers.desktop
inode/directory=thunar.desktop
image/png=org.gnome.Loupe.desktop
image/jpeg=org.gnome.Loupe.desktop
image/webp=org.gnome.Loupe.desktop
image/gif=org.gnome.Loupe.desktop
image/avif=org.gnome.Loupe.desktop
image/svg+xml=org.gnome.Loupe.desktop
image/tiff=org.gnome.Loupe.desktop
video/mp4=io.github.celluloid_player.Celluloid.desktop
video/webm=io.github.celluloid_player.Celluloid.desktop
video/ogg=io.github.celluloid_player.Celluloid.desktop
video/x-matroska=io.github.celluloid_player.Celluloid.desktop
audio/mpeg=io.github.celluloid_player.Celluloid.desktop
audio/flac=io.github.celluloid_player.Celluloid.desktop
audio/ogg=io.github.celluloid_player.Celluloid.desktop
audio/webm=io.github.celluloid_player.Celluloid.desktop
application/zip=org.gnome.FileRoller.desktop
"#;

/// Battery warnings, sent to whatever owns
/// `org.freedesktop.Notifications`, which on this desktop is the shell.
/// Polls sysfs: upower-notifier tools (poweralertd) aren't in Fedora's
/// repos. No battery (desktops, VMs) means the loop just idles cheaply.
///
/// **This overlaps the shell and the overlap is not settled.** noctalia
/// ships `[battery] warning_threshold = 10` and carries its own
/// low-and-critical notifications, so a discharging laptop gets warned
/// at 15 here, at 10 by the shell, and at 5 here again: one state
/// announced by two programs in two styles, which is the shape
/// `DCONF_BLUEMAN` above exists to undo. Removing this in favour of the
/// shell's own threshold is the obvious move and needs a battery to
/// prove, because nothing in a VM ever discharges.
const BATTERY_WATCH: &str = r#"#!/usr/bin/bash
set -u
warned=""
while sleep 60; do
    bat=$(ls /sys/class/power_supply 2>/dev/null | grep -m1 "^BAT" || true)
    [ -n "$bat" ] || continue
    cap=$(cat "/sys/class/power_supply/$bat/capacity" 2>/dev/null || echo 100)
    status=$(cat "/sys/class/power_supply/$bat/status" 2>/dev/null || echo Unknown)
    if [ "$status" != "Discharging" ]; then warned=""; continue; fi
    if [ "$cap" -le 5 ] && [ "$warned" != "critical" ]; then
        warned="critical"
        notify-send -u critical "Battery critical: ${cap}%" "Plug in now."
    elif [ "$cap" -le 15 ] && [ -z "$warned" ]; then
        warned="low"
        notify-send "Battery low: ${cap}%"
    fi
done
"#;

/// What `Mod+D` becomes.
///
/// Stock niri binds it to plain fuzzel, and the image no longer ships
/// fuzzel: the shell brought a launcher and fuzzel left with the menu
/// that was the only other thing using it. The key the hand already
/// goes to should open the thing that lists applications.
///
/// Substituted into the stock config rather than added beside it: niri
/// takes the last bind for a key, so a second `Mod+D` would leave the
/// original in the file, working or not depending on merge order.
const NIRI_MENU_BIND: &str = r#"Mod+D hotkey-overlay-title="Applications" { spawn "noctalia" "msg" "panel-toggle" "launcher"; }"#;

/// The stock line it replaces. Grepped for before the rewrite, so a niri
/// release that renames it fails the build instead of shipping media
/// whose main key does nothing.
///
/// Rewritten rather than left alone even though kuma once put its own
/// menu here: the stock line spawns a program this image does not have,
/// so leaving it is a dead key on the most-used bind there is.
const NIRI_STOCK_LAUNCHER: &str =
    r#"Mod+D hotkey-overlay-title="Run an Application: fuzzel" { spawn "fuzzel"; }"#;

/// niri's stock screen-reader toggle. kuma has never shipped orca, so
/// this has been a key that does nothing since the first niri image —
/// hidden from the overlay by its own `=null`, which is why it survived
/// this long. A dead accessibility key is worse than an absent one: it
/// tells a screen-reader user the machine has a screen reader.
const NIRI_STOCK_ORCA: &str = r#"Super+Alt+S allow-when-locked=true hotkey-overlay-title=null { spawn-sh "pkill orca || exec orca"; }"#;

/// niri's stock lock bind, and what kuma puts in its place.
///
/// This one is advertised on the Important Hotkeys overlay that opens on
/// every first login, so a dead key here is the first thing a new
/// machine shows a person. It was live until the shell replaced
/// swaylock, and swaylock is now excluded from the image outright, which
/// is exactly the shape of change that leaves a bind pointing at nothing.
const NIRI_STOCK_LOCK: &str =
    r#"Super+Alt+L hotkey-overlay-title="Lock the Screen: swaylock" { spawn "swaylock"; }"#;
const NIRI_LOCK_BIND: &str = r#"Super+Alt+L hotkey-overlay-title="Lock the Screen" { spawn "noctalia" "msg" "session" "lock"; }"#;

/// Titles are not decoration here. niri shows EVERY bind on the
/// Important Hotkeys overlay and generates the label from the action, so
/// an untitled `spawn` advertises itself as its own command line: the
/// clipboard bind read as its whole `sh -c` pipeline
/// on the first screen of a new machine. The four worth naming are
/// named, and the media keys are hidden outright — they are printed on
/// the keyboard, and ten of them crowd out everything worth reading.
///
/// Media-key binds routed through kuma-osd, spliced INTO the stock
/// `binds {}` section during the merge (niri rejects a second binds
/// node) while the stock wpctl/brightnessctl lines are sed-stripped.
const NIRI_MEDIA_BINDS: &str = r#"    XF86AudioRaiseVolume allow-when-locked=true hotkey-overlay-title=null { spawn "/usr/libexec/kuma-osd" "volume-up"; }
    XF86AudioLowerVolume allow-when-locked=true hotkey-overlay-title=null { spawn "/usr/libexec/kuma-osd" "volume-down"; }
    XF86AudioMute allow-when-locked=true hotkey-overlay-title=null { spawn "/usr/libexec/kuma-osd" "mute"; }
    XF86AudioMicMute allow-when-locked=true hotkey-overlay-title=null { spawn "wpctl" "set-mute" "@DEFAULT_AUDIO_SOURCE@" "toggle"; }
    XF86MonBrightnessUp allow-when-locked=true hotkey-overlay-title=null { spawn "/usr/libexec/kuma-osd" "brightness-up"; }
    XF86MonBrightnessDown allow-when-locked=true hotkey-overlay-title=null { spawn "/usr/libexec/kuma-osd" "brightness-down"; }
    XF86AudioPlay allow-when-locked=true hotkey-overlay-title=null { spawn "playerctl" "play-pause"; }
    XF86AudioStop allow-when-locked=true hotkey-overlay-title=null { spawn "playerctl" "stop"; }
    XF86AudioNext allow-when-locked=true hotkey-overlay-title=null { spawn "playerctl" "next"; }
    XF86AudioPrev allow-when-locked=true hotkey-overlay-title=null { spawn "playerctl" "previous"; }
    Mod+Ctrl+V hotkey-overlay-title="Clipboard History" { spawn "noctalia" "msg" "panel-toggle" "clipboard"; }
    Mod+Ctrl+W hotkey-overlay-title="Wallpaper" { spawn "noctalia" "msg" "panel-toggle" "wallpaper"; }
    Mod+Alt+R hotkey-overlay-title="Record the Screen" { spawn "/usr/libexec/kuma-record"; }
    Mod+Print hotkey-overlay-title="Screenshot a Region, then Annotate" { spawn "sh" "-c" "grim -g \"$(slurp)\" - | swappy -f -"; }
"#;

/// GTK theme settings travel two roads: Wayland-native apps read
/// gsettings (the dconf defaults cover those), but X11/XWayland GTK apps
/// only listen to an XSettings daemon — without one they render stock
/// light Adwaita. xsettingsd broadcasts the same dark values there.
const XSETTINGSD_CONF: &str = r#"Net/ThemeName "adw-gtk3-dark"
Net/IconThemeName "Adwaita"
Gtk/ApplicationPreferDarkTheme 1
Gtk/CursorThemeName "Adwaita"
Xft/DPI 98304
"#;

/// GTK's base-layer config, read by every GTK app on every backend when
/// higher-priority sources (XSettings, gsettings) don't reach it. The
/// value is 96 dpi << 10 above: without a broadcast DPI, X11 clients
/// compute one from XWayland's bogus virtual-monitor physical size and
/// render enormous.
const GTK3_SETTINGS_INI: &str = r#"[Settings]
gtk-theme-name = adw-gtk3-dark
gtk-application-prefer-dark-theme = true
gtk-icon-theme-name = Adwaita
"#;

const GTK4_SETTINGS_INI: &str = r#"[Settings]
gtk-application-prefer-dark-theme = true
"#;

/// The two X11 helpers below both start before XWayland does.
///
/// `xwayland-satellite` publishes `DISPLAY` into the session environment
/// when it comes up, which is after `spawn-at-startup` has already run
/// both of these. Waiting is the whole difference between a helper that
/// works and one that exits instantly on every login, so it is written
/// once: two copies of a 30-second timeout is two chances to fix half a
/// bug. Exit 0 rather than fail after the wait, because a session with
/// no XWayland at all is a session with nothing for either to do.
const WAIT_FOR_DISPLAY: &str = r#"for _ in $(seq 60); do
    [ -n "${DISPLAY:-}" ] && break
    DISPLAY=$(systemctl --user show-environment 2>/dev/null | sed -n 's/^DISPLAY=//p')
    [ -n "$DISPLAY" ] && export DISPLAY && break
    sleep 0.5
done
[ -n "${DISPLAY:-}" ] || exit 0
"#;

fn xsettings_launcher() -> String {
    format!(
        "#!/usr/bin/bash\nset -euo pipefail\n{WAIT_FOR_DISPLAY}\
         exec xsettingsd -c /usr/lib/kuma/xsettingsd.conf\n"
    )
}

/// Session half of host<->guest clipboard in `kuma vm`. spice-vdagent's
/// clipboard side is X11, so under niri it rides the xwayland-satellite
/// bridge — wait briefly for DISPLAY to appear in the session
/// environment. No vdagent port (real hardware) means exit quietly.
fn clipboard_bridge() -> String {
    format!(
        "#!/usr/bin/bash\nset -euo pipefail\n\
         [ -e /dev/virtio-ports/com.redhat.spice.0 ] || exit 0\n\
         {WAIT_FOR_DISPLAY}\
         exec spice-vdagent -x\n"
    )
}

/// Joan G. Stark's classic ASCII bear (her "jgs" signature moved here so
/// it doesn't render on every run), wallpaper-bear warm brown ($1) with
/// the wordmark in kuma green ($2). Identity you invoke: fastfetch is
/// baked but nothing runs it at shell startup.
const FASTFETCH_LOGO: &str = r#"$1 .--.              .--.
$1: (\ ". _......_ ." /) :
$1 '.    `        `    .'
$1  /'   _        _   `\
$1 /     0}      {0     \
$1|       /      \       |
$1|     /'        `\     |
$1 \   | .  .==.  . |   /
$1  '._ \.' \__/ './ _.'
$1  /  ``'._-''-_.'``  \
$1          `--`
$2     k   u   m   a
"#;

/// System-wide default via XDG_CONFIG_DIRS; a user config in
/// ~/.config/fastfetch still wins, same as every other config here.
const FASTFETCH_CONFIG: &str = r#"{
    "logo": {
        "type": "file",
        "source": "/usr/lib/kuma/fastfetch-logo.txt",
        "color": { "1": "38;2;226;190;146", "2": "38;2;126;224;168" },
        "padding": { "top": 1, "right": 3 }
    },
    "modules": [
        "title",
        "separator",
        "os",
        "kernel",
        "uptime",
        "packages",
        "shell",
        "wm",
        "terminal",
        "cpu",
        "gpu",
        "memory",
        "disk",
        "break",
        "colors"
    ]
}
"#;

/// Theme files for the curated desktop. The colours here are a fallback
/// now rather than the theme: the shell renders the live palette into
/// `~/.config/kitty/themes/noctalia.conf` and kitty loads that on top of
/// this file, so what is written here is what a terminal shows when the
/// shell has not rendered anything yet. Everything else in the file
/// (font, padding, decorations, opacity) is still the only copy.
/// All system-wide (never /etc/skel): skel only reaches homes created after
/// the image ships, so it strands existing users on stale copies — image
/// updates must retheme every account. User dotfiles still win everywhere:
/// The shell reads its own config-home, and kitty merges
/// /etc/xdg beneath the user's file (so a one-key override keeps the rest
/// of this theme).
const WALLPAPER: &[u8] = include_bytes!("../assets/kuma-wallpaper.jpg");
const KITTY_CONFIG: &str = include_str!("../assets/kitty.conf");

/// Rebrand the OS identity: Kuma, not Fedora. ID_LIKE=fedora keeps tools
/// that sniff os-release (toolbox, distrobox, dnf COPR, …) working. Runs
/// last so every dnf layer before it still sees stock Fedora metadata.
///
/// Two independent axes, and keeping them apart is the point:
///
/// - **The number is kuma's**, taken from the binary that built the image,
///   so PRETTY_NAME says which kuma made this machine. The number alone,
///   deliberately: `io.kuma.builder` already carries the full
///   `<version> (<sha> <date>)` stamp for the "is this the binary with my
///   change in it" question (see build.rs), and a GRUB menu entry is the
///   wrong place to answer it. Between releases every build from main
///   therefore shows the same number, which is the same approximation
///   `--version` makes and is why the sha exists beside it.
/// - **The bear names the Fedora base**, keyed by VERSION_ID below. Two
///   machines reading (Callisto) share a base whatever their numbers say,
///   and a bear you did not expect means the base moved under you.
///
/// **VERSION_ID stays Fedora's** and must: it is machine-readable, and
/// toolbox, distrobox and COPR resolve against it. Only the display
/// fields carry kuma's version. `/usr/lib/fedora-release` likewise keeps
/// Fedora's number, because it exists to be parsed as one.
///
/// Bear names go alphabetically where a letter has a name worth using;
/// the pool is bears real, extinct, mythical and fictional. D and F are
/// skipped deliberately (Deninger and Fozzie were the only candidates and
/// neither earns a place). Planned: E Ephraim, G Grizzly, H Helarctos,
/// I Iorek, J Jambavan, K Kodiak.
///
/// An unlisted base falls back to no bear, keeping "Kuma <version>" so a
/// machine still says what built it.
const BRANDING: &str = r#"
RUN . /usr/lib/os-release \
    && case "${VERSION_ID}" in \
        44) CODENAME="Beorn" ;; \
        45) CODENAME="Callisto" ;; \
        *) CODENAME="" ;; \
    esac \
    && sed -i \
        -e 's|^NAME=.*|NAME="Kuma"|' \
        -e "s|^PRETTY_NAME=.*|PRETTY_NAME=\"Kuma @KUMAVERSION@${CODENAME:+ ($CODENAME)}\"|" \
        -e "s|^VERSION=.*|VERSION=\"@KUMAVERSION@${CODENAME:+ ($CODENAME)}\"|" \
        -e 's|^ID=.*|ID=kuma|' \
        -e 's|^DEFAULT_HOSTNAME=.*|DEFAULT_HOSTNAME="{default_hostname}"|' \
        -e 's|^ANSI_COLOR=.*|ANSI_COLOR="0;38;2;126;224;168"|' \
        /usr/lib/os-release \
    && if [ -n "$CODENAME" ]; then sed -i \
        -e "s|^VERSION_CODENAME=.*|VERSION_CODENAME=$(printf %s "$CODENAME" | tr '[:upper:]' '[:lower:]')|" \
        /usr/lib/os-release; fi \
    && { grep -q '^ID_LIKE=' /usr/lib/os-release || echo 'ID_LIKE="fedora"' >> /usr/lib/os-release; } \
    && { [ ! -f /usr/lib/fedora-release ] || echo "Kuma release ${VERSION_ID}${CODENAME:+ ($CODENAME)}" > /usr/lib/fedora-release; }
"#;

/// BRANDING with the building binary's version substituted in. Not a
/// const because the version is only knowable at compile time, and not a
/// `format!` because the block is dense with `${...}` the shell needs and
/// `format!` would demand every brace be doubled.
fn branding() -> String {
    BRANDING
        .replace("@KUMAVERSION@", env!("CARGO_PKG_VERSION"))
        .replace("{default_hostname}", crate::install::DEFAULT_HOSTNAME)
}

/// Homebrew lives in /home/linuxbrew — machine-local mutable state, so it
/// can't be image content. First boot installs it; the tarball is the
/// official "untar anywhere" method. Prefix owned by uid 1000, brew's
/// single-user model (same choice Bluefin makes).
///
/// The in-script guard duplicates the unit's ConditionPathExists on
/// purpose: PID 1 evaluates conditions as init_t, and the script runs
/// unconfined, so the two can disagree (they did, for months — see the
/// service comment). Whichever check runs, an installed brew is left alone.
const BREW_SETUP_SCRIPT: &str = r#"#!/usr/bin/bash
set -euo pipefail
prefix=/home/linuxbrew/.linuxbrew

# This runs as root inside a tree the line at the bottom hands to uid
# 1000, so on the second boot every path here is one that uid can have
# replaced. Both guards (the -x below and the unit's own
# ConditionPathExists) live inside it too, which means that uid decides
# whether root runs this again.
#
# The concrete move is to delete the prefix and leave a symlink in its
# place: `mkdir -p` accepts an existing symlink-to-directory and `tar -C`
# chdirs through it, so root would extract Homebrew's tree at a path
# somebody else chose. That is a root-privileged write of fixed content
# rather than code execution, and on a machine whose account is in wheel
# (which every example declares) the attacker already has root, so no
# boundary is crossed there. It matters for a declaration that keeps its
# user out of wheel, and it is the one place kuma has root operating on
# paths a non-root uid owns.
#
# So: refuse rather than repair. A prefix that exists and is not root's
# is somebody's business, not this unit's, and saying so beats silently
# extracting into it.
for dir in /home/linuxbrew "$prefix" "$prefix/Homebrew" "$prefix/bin"; do
    [ -e "$dir" ] || continue
    if [ -L "$dir" ] || [ ! -d "$dir" ]; then
        echo "kuma: $dir is not a directory; refusing to set up Homebrew here" >&2
        exit 1
    fi
    if [ "$(stat -c %u "$dir")" != 0 ]; then
        echo "kuma: $dir is not root-owned; refusing to write into it as root" >&2
        exit 1
    fi
done

if [ -x "$prefix/bin/brew" ]; then exit 0; fi
mkdir -p "$prefix/Homebrew" "$prefix/bin"
curl -fsSL https://github.com/Homebrew/brew/tarball/HEAD \
    | tar -xz --strip-components=1 -C "$prefix/Homebrew"
ln -sf ../Homebrew/bin/brew "$prefix/bin/brew"
chown -R 1000:1000 /home/linuxbrew
"#;

const BREW_SETUP_SERVICE: &str = r#"[Unit]
Description=Install Homebrew
Wants=network-online.target
After=network-online.target
# The condition must not traverse a symlink: PID 1 checks it as init_t,
# which SELinux lets search home dirs and stat home files but not read
# home symlinks (lnk_file read), so any path through /home (itself a
# symlink to var/home) or ending at bin/brew (a symlink into Homebrew/)
# resolves as "missing" and the condition fails open — this service ran,
# and re-downloaded all of Homebrew, on every single boot. The real
# ruby entry point, named via /var/home, is symlink-free the whole way.
ConditionPathExists=!/var/home/linuxbrew/.linuxbrew/Homebrew/bin/brew

[Service]
Type=oneshot
ExecStart=/usr/libexec/kuma-brew-setup
CPUWeight=25
IOWeight=25

[Install]
WantedBy=multi-user.target
"#;

/// Converge installed formulae to the declared list. Brew is
/// single-prefix, with no scope split that could stand in for
/// authority, so a state file remembers what the declaration installed
/// and only ever-declared formulae are removal candidates. Ad-hoc `brew
/// install` is untouched. The flatpak sync learned the same trick after
/// its scope proxy turned out to be one a store could break.
///
/// The upgrade at the end is unscoped on purpose, and it is the one
/// thing here that reaches past the declaration. Ad-hoc formulae stay
/// the owner's to keep or remove; they were simply never getting
/// updated, because the upgrade used to name the declared list. Bare
/// `brew upgrade` also covers casks, which nothing else in kuma can even
/// see. It runs after the state file is written so a failure cannot
/// strand authority tracking, and needs no `brew update` first because
/// brew auto-updates its taps before upgrading.
const BREW_SYNC_SCRIPT: &str = r#"#!/usr/bin/bash
set -euo pipefail
brew=/home/linuxbrew/.linuxbrew/bin/brew
[ -x "$brew" ] || exit 0
declared=/usr/lib/kuma/brews
state=/home/linuxbrew/.linuxbrew/.kuma-brews
[ -f "$state" ] || : > "$state"
if [ -s "$declared" ]; then
    xargs -a "$declared" "$brew" install
fi
while read -r formula; do
    grep -qxF "$formula" "$declared" && continue
    "$brew" uninstall "$formula" || true
done < "$state"
"$brew" autoremove
cp "$declared" "$state"
"$brew" upgrade
"#;

/// brew refuses to run as root; uid 1000 owns the prefix (brew's
/// single-user model). Numeric User= needs no passwd entry, so this
/// works on first boot even before kuma-user-sync creates the account.
/// HOME points inside the prefix so brew's cache lands somewhere the
/// uid can write regardless of which human account exists.
const BREW_SYNC_SERVICE: &str = r#"[Unit]
Description=Converge Homebrew formulae to the declared list
Wants=network-online.target
After=network-online.target kuma-brew-setup.service
StartLimitIntervalSec=1h
StartLimitBurst=6

[Service]
Type=oneshot
User=1000
Environment=HOME=/home/linuxbrew
ExecStart=/usr/libexec/kuma-brew-sync
Restart=on-failure
RestartSec=2min
CPUWeight=25
IOWeight=25

[Install]
WantedBy=multi-user.target
"#;

/// Same rationale as the flatpak timer: boot-only convergence goes
/// stale on machines that stay up.
const BREW_SYNC_TIMER: &str = r#"[Unit]
Description=Daily Homebrew convergence

[Timer]
OnCalendar=daily
Persistent=true
RandomizedDelaySec=1h

[Install]
WantedBy=timers.target
"#;

const BREW_PROFILE_SH: &str = r#"[ -x /home/linuxbrew/.linuxbrew/bin/brew ] \
    && eval "$(/home/linuxbrew/.linuxbrew/bin/brew shellenv)"
"#;

const BREW_PROFILE_FISH: &str = r#"if test -x /home/linuxbrew/.linuxbrew/bin/brew
    /home/linuxbrew/.linuxbrew/bin/brew shellenv | source
end
"#;

/// Every /etc path this image owns the *contents* of.
///
/// Derived from the Containerfile the config compiles to rather than
/// listed by hand, so a new write into /etc is covered the day it lands
/// and can't be forgotten by whoever adds it.
///
/// A build writes into /etc three ways, all unambiguous: a COPY whose
/// destination is under /etc, a shell redirect (`>`, `>>`) into one, and
/// a symlink made there with `ln -s`. Reading an /etc file is not owning
/// it, and the difference matters: the keyring assert greps
/// /etc/pam.d/greetd and `niri validate` reads /etc/niri/config.kdl, but
/// only one of those two files is kuma's to have an opinion about.
/// Redirects separate them for free, since a read has none.
///
/// The third way is here because it was missing, and a declared
/// `system.timezone` is what it cost: the timezone lands as
/// `ln -sfn /usr/share/zoneinfo/<zone> /etc/localtime`, which is neither
/// a COPY nor a redirect, so the one file the declaration explicitly
/// claims was owned by the image and watched by nobody. On a machine
/// installed by Anaconda, which writes its own /etc/localtime, that is
/// precisely where the ostree merge makes a declared value never apply.
pub fn etc_paths(config: &Config) -> Vec<String> {
    let mut paths = etc_writes(&generate(config));
    // Hostname is machine state: unpinned, the baked file only seeds the
    // ostree merge default and an admin's `hostnamectl` rename is the
    // sanctioned interface, not drift. A *declared* hostname is an
    // opinion the machine should match, so then it stays owned.
    if config.system.hostname.is_none() {
        paths.retain(|p| p != "/etc/hostname");
    }
    paths
}

fn etc_writes(containerfile: &str) -> Vec<String> {
    let mut paths: BTreeSet<&str> = BTreeSet::new();
    for line in containerfile.lines() {
        if let Some(dest) =
            line.strip_prefix("COPY ").and_then(|rest| rest.split_whitespace().last())
        {
            if dest.starts_with("/etc/") {
                paths.insert(dest);
            }
        }
        // Split on the shell's separators so a line running several
        // commands is read as several commands, and the destination of
        // each `ln -s` is its own last word. Asked of the whole line
        // first, so the splitting only happens on the lines that could
        // possibly answer yes.
        for segment in line
            .contains("ln -s")
            .then(|| line.split("&&").flat_map(|part| part.split(';')))
            .into_iter()
            .flatten()
        {
            if !segment.contains("ln -s") {
                continue;
            }
            if let Some(dest) = segment.split_whitespace().last() {
                if dest.starts_with("/etc/") {
                    paths.insert(dest);
                }
            }
        }
        let mut rest = line;
        while let Some(at) = rest.find('>') {
            rest = rest[at + 1..].trim_start_matches('>').trim_start();
            let end = rest
                .find(|c: char| c.is_whitespace() || c == '"' || c == '\'')
                .unwrap_or(rest.len());
            if rest[..end].starts_with("/etc/") {
                paths.insert(&rest[..end]);
            }
        }
    }
    paths.into_iter().map(str::to_string).collect()
}

/// Compile a kuma config into a Containerfile for a bootc image build.
pub fn generate(config: &Config) -> String {
    let mut out = String::new();
    out.push_str("# Generated by kuma. Edit kuma.toml instead.\n");
    out.push_str(&format!("FROM {}\n", config.base_ref()));

    // Desktop layer first: it is large and changes rarely, so keeping it
    // before the user's packages preserves the build cache across edits.
    if config.system.desktop == Desktop::Niri {
        out.push('\n');
        // niri's weak deps, which ride in past the package list unless
        // they are named here. alacritty because kuma's terminal is
        // kitty; waybar and swaylock because the shell replaced them and
        // dropping them from NIRI_PACKAGES is not enough to remove them.
        // Measured: an image built without these excludes still had a bar
        // and a lock screen it never starts.
        out.push_str(&dnf_install(&format!(
            "{} {}",
            NIRI_EXCLUDES.iter().map(|p| format!("--exclude={p}")).collect::<Vec<_>>().join(" "),
            NIRI_PACKAGES.join(" ")
        )));
        out.push_str(&mesa_freeworld());
        // A theme named in four places and present in none is a desktop
        // that silently falls back to light Adwaita, so prove the
        // package put the directory where the four names point.
        out.push_str("RUN test -d /usr/share/themes/adw-gtk3-dark\n");
        out.push_str("COPY greetd-config.toml /etc/greetd/config.toml\n");
        out.push_str("COPY kargs-desktop.toml /usr/lib/bootc/kargs.d/10-kuma-desktop.toml\n");
        out.push_str("COPY niri-extras.kdl /usr/lib/kuma/niri-extras.kdl\n");
        out.push_str("COPY kuma-wallpaper.jpg /usr/share/backgrounds/kuma/kuma-wallpaper.jpg\n");
        out.push_str("COPY noctalia-config.toml /usr/lib/kuma/noctalia/config.toml\n");
        // The wallpaper the shell falls back to when nobody has chosen
        // one, which on a new machine is always.
        //
        // Not settable from the config: `[wallpaper.default] path` is a
        // real key that `config validate` accepts and the shell ignores
        // outside its own state, so the only way to change what a first
        // boot shows is to change the file it defaults to. kuma owns the
        // image, so it changes the file. A person who picks another
        // wallpaper still wins — that goes to state, which outranks this.
        //
        // A JPEG under a .png name on purpose: the path is noctalia's and
        // the decoder sniffs the content rather than trusting the suffix,
        // verified by setting one and reading it back.
        out.push_str("RUN test -f /usr/share/noctalia/assets/noctalia-wallpaper.png\n");
        out.push_str("COPY kuma-wallpaper.jpg /usr/share/noctalia/assets/noctalia-wallpaper.png\n");
        // Prove the baked config is actually reachable, in the build.
        //
        // `noctalia config validate` is not enough: it accepts
        // `source = "bogus"` happily, so it checks TOML syntax and key
        // names and not values. And `NOCTALIA_CONFIG_HOME` is
        // undocumented in `--help`, so an upstream rename would silently
        // drop the desktop back to noctalia's own palette with nothing
        // failing anywhere. This asks the binary what it merged and
        // greps for two things kuma put there.
        out.push_str(
            "RUN out=$(HOME=/tmp NOCTALIA_CONFIG_HOME=/usr/lib/kuma noctalia config export merged); \\\n                 printf '%s\\n' \"$out\"; \\\n                 printf '%s' \"$out\" | grep -q '/usr/share/backgrounds/kuma' \\\n                 && printf '%s' \"$out\" | grep -q 'timeout = 900' \\\n                 && printf '%s' \"$out\" | grep -q 'builtin_ids = \\[ \"kitty\"'\n",
        );
        out.push_str("COPY kitty.conf /etc/xdg/kitty/kitty.conf\n");
        // kitty skips settings it doesn't recognise and starts anyway, so a
        // renamed key ships a silently unthemed terminal — which is exactly
        // how foot 1.27 voided this palette before kuma switched. Parse the
        // file with kitty's own loader at build time, and treat BOTH of its
        // complaints as fatal: accumulate_bad_lines catches malformed lines
        // but NOT unknown keys, which are only ever logged to stderr (that
        // asymmetry was verified by sabotage, so don't collapse this into
        // the exit code alone). Grepping kitty's own log keeps the check
        // free of an option allowlist to maintain.
        out.push_str(
            "RUN rc=0; kitty +runpy \"import sys; from kitty.config import load_config; bad = []; load_config('/etc/xdg/kitty/kitty.conf', accumulate_bad_lines=bad); sys.exit('malformed kitty.conf lines: %s' % bad if bad else 0)\" 2>/tmp/kitty.err || rc=$?; \\\n    cat /tmp/kitty.err >&2; \\\n    if grep -q 'unknown config key' /tmp/kitty.err; then rc=1; fi; \\\n    rm -f /tmp/kitty.err; exit $rc\n",
        );
        // And prove the template the shell will render actually renders,
        // with the same engine it uses. It catches a placeholder noctalia
        // stopped filling in, which would ship a kitty theme full of
        // literal {{colors...}}, and an upstream template that stopped
        // carrying the ANSI sixteen, which would leave the terminal half
        // on the palette and half on the image's fallback colours.
        out.push_str(
            "RUN HOME=/tmp NOCTALIA_CONFIG_HOME=/usr/lib/kuma noctalia theme \\\n      /usr/share/backgrounds/kuma/kuma-wallpaper.jpg --dark \\\n      -r /usr/share/noctalia/assets/templates/kitty/kitty.conf:/tmp/kitty-rendered.conf \\\n    && cat /tmp/kitty-rendered.conf \\\n    && grep -Eq '^background +#[0-9a-fA-F]{6}$' /tmp/kitty-rendered.conf \\\n    && grep -qE '^color0 +#[0-9a-fA-F]{6}$' /tmp/kitty-rendered.conf \\\n    && ! grep -q '{{' /tmp/kitty-rendered.conf \\\n    && rm -f /tmp/kitty-rendered.conf\n",
        );
        out.push_str("COPY --chmod=755 kuma-clipboard-bridge /usr/libexec/kuma-clipboard-bridge\n");
        out.push_str("COPY fastfetch-config.jsonc /etc/xdg/fastfetch/config.jsonc\n");
        out.push_str("COPY fastfetch-logo.txt /usr/lib/kuma/fastfetch-logo.txt\n");
        out.push_str("COPY --chmod=755 kuma-xsettings /usr/libexec/kuma-xsettings\n");
        out.push_str("COPY xsettingsd.conf /usr/lib/kuma/xsettingsd.conf\n");
        out.push_str("COPY niri-binds.kdl /usr/lib/kuma/niri-binds.kdl\n");
        out.push_str("COPY --chmod=755 kuma-record /usr/libexec/kuma-record\n");
        out.push_str("COPY --chmod=755 kuma-battery-watch /usr/libexec/kuma-battery-watch\n");
        // The shell as a supervised unit, and the guard that refuses to
        // sleep without it. `--global` because the shell is a user unit
        // and every account on this image should get it; the sleep guard
        // is system-wide because sleep is.
        out.push_str("COPY kuma-shell.service /usr/lib/systemd/user/kuma-shell.service\n");
        out.push_str(
            "COPY kuma-sleep-guard.service /usr/lib/systemd/system/kuma-sleep-guard.service\n",
        );
        out.push_str("COPY --chmod=755 kuma-sleep-guard /usr/libexec/kuma-sleep-guard\n");
        out.push_str(
            "RUN systemctl --global enable kuma-shell.service \\\n    \
             && systemctl enable kuma-sleep-guard.service\n",
        );
        out.push_str("COPY --chmod=755 kuma-osd /usr/libexec/kuma-osd\n");
        out.push_str("COPY gtk3-settings.ini /etc/gtk-3.0/settings.ini\n");
        out.push_str("COPY gtk4-settings.ini /etc/gtk-4.0/settings.ini\n");
        out.push_str("COPY mimeapps.list /etc/xdg/mimeapps.list\n");
        out.push_str("COPY dconf-profile /etc/dconf/profile/user\n");
        out.push_str(&keyring_pam("greetd"));
        out.push_str("COPY dconf-kuma-dark /etc/dconf/db/local.d/10-kuma-dark\n");
        out.push_str("COPY dconf-kuma-blueman /etc/dconf/db/local.d/10-kuma-blueman\n");
        out.push_str("RUN dconf update\n");
        out.push_str("COPY autostart-blueman /etc/xdg/autostart/blueman.desktop\n");
        out.push_str(
            "COPY autostart-polkit-mate \
             /etc/xdg/autostart/polkit-mate-authentication-agent-1.desktop\n",
        );
        // The packaged default config is complete (all keybindings); Kuma's
        // config is that plus our session extras, validated at build time.
        // Fedora's default config already spawns waybar — drop that line (and
        // its comment) or the bar starts twice; Kuma's extras spawn it.
        // Upstream's terminal is alacritty; Kuma ships kitty, so rewrite the
        // spawn (and its hotkey-overlay title). grep first: if a niri update
        // stops naming alacritty, fail the build instead of silently
        // shipping a Mod+T that spawns a terminal the image doesn't have.
        out.push_str(
            &format!("RUN grep -q '\"alacritty\"' /usr/share/doc/niri/default-config.kdl \\\n    && grep -qF '{NIRI_STOCK_LAUNCHER}' /usr/share/doc/niri/default-config.kdl \\\n    && grep -qF '{NIRI_STOCK_LOCK}' /usr/share/doc/niri/default-config.kdl \\\n    && grep -qF '{NIRI_STOCK_ORCA}' /usr/share/doc/niri/default-config.kdl \\\n    && mkdir -p /etc/niri \\\n    && sed -e 's/alacritty/kitty/g' -e '/starts waybar/d' -e '/^spawn-at-startup \"waybar\"$/d' -e '/XF86Audio/d' -e '/XF86MonBrightness/d' -e 's|{NIRI_STOCK_LAUNCHER}|{NIRI_MENU_BIND}|' -e 's|{NIRI_STOCK_LOCK}|{NIRI_LOCK_BIND}|' -e '/pkill orca/d' -e '/^binds {{/r /usr/lib/kuma/niri-binds.kdl' /usr/share/doc/niri/default-config.kdl > /etc/niri/config.kdl \\\n    && cat /usr/lib/kuma/niri-extras.kdl >> /etc/niri/config.kdl \\\n    && niri validate --config /etc/niri/config.kdl\n"),
        );
        // Every "attach a file" button in every app did nothing, silently.
        //
        // niri's own portals.conf prefers the GNOME backend for everything
        // it does not name, and that backend does not implement FileChooser:
        // it delegates to org.gnome.Nautilus. kuma ships Thunar, so the name
        // is not activatable and the request dies inside the backend. The
        // only trace anywhere is "Delegated FileChooser call failed: The
        // name is not activatable" in the *user* journal, which is why this
        // survived two desktops and a bare-metal install unnoticed.
        //
        // The `default=gnome;gtk;` fallback cannot save it. Fallback is
        // resolved from the .portal files, gnome.portal advertises
        // FileChooser, and the failure only happens later, one level down,
        // where the router cannot see it. So the interface has to be
        // pointed at gtk by name. gtk implements it for real, is already
        // installed for Access and Notification, and is already running.
        //
        // Derived from niri's file rather than copied beside it: the
        // highest-precedence file wins outright instead of merging, so a
        // hand-copy would silently freeze whatever niri's defaults were the
        // day it was written. Both greps fail the build instead of shipping
        // a dead picker: the first if niri stops shipping the file this is
        // derived from, the second if the gtk backend stops implementing
        // the one interface being routed to it.
        out.push_str(
            "RUN grep -q '^\\[preferred\\]' /usr/share/xdg-desktop-portal/niri-portals.conf \\\n    && grep -q 'org.freedesktop.impl.portal.FileChooser' /usr/share/xdg-desktop-portal/portals/gtk.portal \\\n    && mkdir -p /etc/xdg-desktop-portal \\\n    && { cat /usr/share/xdg-desktop-portal/niri-portals.conf; echo 'org.freedesktop.impl.portal.FileChooser=gtk;'; } > /etc/xdg-desktop-portal/niri-portals.conf\n",
        );
        // Upstream niri-session imports the ENTIRE greeter environment into
        // the systemd user manager — deprecated (warns in the journal every
        // login) and indiscriminate. Scope it: the XDG_* trio is how
        // niri.service finds the logind session; PATH carries the login
        // shell's profile.d additions (brew) into everything niri spawns.
        // grep first so the build fails if a niri update rewords the script.
        out.push_str(
            "RUN grep -qx '    systemctl --user import-environment' /usr/bin/niri-session \\\n    && sed -i 's/^    systemctl --user import-environment$/    systemctl --user import-environment PATH XDG_SESSION_ID XDG_SEAT XDG_VTNR/' /usr/bin/niri-session\n",
        );
        out.push_str(
            "RUN systemctl set-default graphical.target && systemctl enable greetd.service firewalld.service power-profiles-daemon.service bluetooth.service cups.service avahi-daemon.service chronyd.service\n",
        );
    }

    if config.system.desktop == Desktop::Cosmic {
        out.push('\n');
        out.push_str(&dnf_install(&COSMIC_PACKAGES.join(" ")));
        out.push_str(&mesa_freeworld());
        // Fedora ships cosmic-greeter.service (preset-enabled, aliased as
        // display-manager.service) running `greetd --config
        // /etc/greetd/cosmic-greeter.toml` — kuma writes no greeter config
        // of its own here, unlike the niri arm.
        if let Some(user) = &config.user {
            if user.autologin {
                // same greetd initial_session semantics as the niri arm
                // (straight into the desktop at boot, greeter on logout),
                // appended to the config cosmic-greeter.service reads.
                // test -f first so a moved config fails the build instead
                // of autologin silently not happening.
                out.push_str(&format!(
                    "RUN test -f /etc/greetd/cosmic-greeter.toml && printf '\\n[initial_session]\\ncommand = \"start-cosmic\"\\nuser = \"{}\"\\n' >> /etc/greetd/cosmic-greeter.toml\n",
                    user.name
                ));
            }
        }
        // kuma declares the user and its look is settings, not a wizard —
        // the first-boot setup must not fire. Plain rm so the build fails
        // if COSMIC ever moves the autostart file, instead of the wizard
        // silently resurfacing.
        out.push_str("RUN rm /etc/xdg/autostart/com.system76.CosmicInitialSetup.desktop\n");
        // cosmic-comp promotes buffers straight to scanout, and on AMD
        // GFX10+ (seen on Rembrandt/680M) a promoted buffer can carry a
        // DCC-compressed modifier the scanout path reads as raw pixels —
        // intermittent bands of static across the panel. Disabling only
        // overlay scanout was not enough: the band recurred with that var
        // verifiably active, because fullscreen direct scanout takes the
        // same DCC path. Both knobs together are the verified cure on
        // bare metal; the cost is scanout-bypass power savings only,
        // composed output is unaffected. pam_env applies /etc/environment
        // to the greetd-spawned session, which is where cosmic-comp lives.
        // Upstream: pop-os/cosmic-comp#1039, #2152.
        out.push_str(
            "RUN printf 'COSMIC_DISABLE_OVERLAY_SCANOUT=1\\nCOSMIC_DISABLE_DIRECT_SCANOUT=1\\n' >> /etc/environment\n",
        );
        // cosmic-greeter authenticates against its own PAM service, not
        // greetd's: asserting /etc/pam.d/greetd here would pass while
        // the stack COSMIC logs in through went unchecked.
        out.push_str(&keyring_pam("cosmic-greeter"));
        out.push_str("COPY kargs-desktop.toml /usr/lib/bootc/kargs.d/10-kuma-desktop.toml\n");
        out.push_str("COPY fastfetch-config.jsonc /etc/xdg/fastfetch/config.jsonc\n");
        out.push_str("COPY fastfetch-logo.txt /usr/lib/kuma/fastfetch-logo.txt\n");
        out.push_str("COPY kuma-wallpaper.jpg /usr/share/backgrounds/kuma/kuma-wallpaper.jpg\n");
        // Overwrite COSMIC's packaged defaults in place, guarded so the
        // build fails if an update moves them — an override at a path
        // nothing reads would silently ship the stock look.
        out.push_str(
            "RUN test -f /usr/share/cosmic/com.system76.CosmicAppList/v1/favorites \\\n    && test -f /usr/share/cosmic/com.system76.CosmicBackground/v1/all\n",
        );
        out.push_str(
            "COPY cosmic-favorites /usr/share/cosmic/com.system76.CosmicAppList/v1/favorites\n",
        );
        out.push_str(
            "COPY cosmic-background /usr/share/cosmic/com.system76.CosmicBackground/v1/all\n",
        );
        // cosmic-greeter.service, not greetd.service: it already owns the
        // display-manager alias via preset — enabling greetd would fight
        // it (and did, failing the first prototype build). Explicit enable
        // is idempotent with the preset and keeps intent visible.
        out.push_str(
            "RUN systemctl set-default graphical.target && systemctl enable cosmic-greeter.service firewalld.service power-profiles-daemon.service bluetooth.service cups.service avahi-daemon.service chronyd.service\n",
        );
    }

    let wants_flatpak =
        config.system.desktop != Desktop::None || !config.packages.flatpak.is_empty();
    if wants_flatpak {
        if config.system.desktop == Desktop::None {
            out.push('\n');
            out.push_str(&dnf_install("flatpak"));
        }
        // Preconfigured-remote mechanism: flatpak reads /etc/flatpak/remotes.d,
        // so Flathub (with its GPG key) ships as image content. Flathub is
        // Kuma's only app source — mask the unit that injects Fedora's
        // registry remote at boot, or non-interactive installs become
        // ambiguous between the two.
        out.push_str(&format!(
            "RUN curl --fail -Lo /etc/flatpak/remotes.d/flathub.flatpakrepo {FLATHUB_URL} \\\n    && systemctl mask flatpak-add-fedora-repos.service\n"
        ));
    }
    if wants_flatpak {
        // Ship the declaration and sync even when the list is empty:
        // convergence means an emptied list removes the apps too.
        out.push_str("COPY flatpaks /usr/lib/kuma/flatpaks\n");
        out.push_str("COPY --chmod=755 kuma-flatpak-sync /usr/libexec/kuma-flatpak-sync\n");
        out.push_str(
            "COPY kuma-flatpak-sync.service /usr/lib/systemd/system/kuma-flatpak-sync.service\n",
        );
        out.push_str(
            "COPY kuma-flatpak-sync.timer /usr/lib/systemd/system/kuma-flatpak-sync.timer\n",
        );
        out.push_str("RUN systemctl enable kuma-flatpak-sync.service kuma-flatpak-sync.timer\n");
        // Overrides ride the same gate rather than their own. An
        // emptied [overrides] table has keys to take back, and gating
        // on "are any declared" would delete the converger in the same
        // build that gives it its last job.
        out.push_str("COPY overrides /usr/lib/kuma/overrides\n");
        out.push_str(
            "COPY kuma-flatpak-overrides.service /usr/lib/systemd/system/kuma-flatpak-overrides.service\n",
        );
        out.push_str(
            "COPY kuma-flatpak-overrides-user.service /usr/lib/systemd/user/kuma-flatpak-overrides.service\n",
        );
        // --global enables it for every account that logs in, which is
        // the only way a unit reaches a home directory without root
        // writing into one.
        out.push_str(
            "RUN systemctl enable kuma-flatpak-overrides.service \\\n    && systemctl --global enable kuma-flatpak-overrides.service\n",
        );
    }

    if config.system.brew || !config.packages.brew.is_empty() {
        // git-core: brew needs git at runtime to update itself.
        // tar: the setup script unpacks brew's tarball with it. fedora-bootc
        // happened to ship both; a base composed from Fedora's minimal
        // manifest ships neither, so this layer pays for its own tools.
        out.push('\n');
        out.push_str(&dnf_install("git-core tar"));
        out.push_str("COPY --chmod=755 kuma-brew-setup /usr/libexec/kuma-brew-setup\n");
        out.push_str(
            "COPY kuma-brew-setup.service /usr/lib/systemd/system/kuma-brew-setup.service\n",
        );
        out.push_str("COPY brew-profile.sh /etc/profile.d/kuma-brew.sh\n");
        out.push_str("COPY brew-profile.fish /etc/fish/conf.d/kuma-brew.fish\n");
        // Declaration and sync ship even when the list is empty, same as
        // flatpaks: an emptied list must still remove what it installed.
        out.push_str("COPY brews /usr/lib/kuma/brews\n");
        out.push_str("COPY --chmod=755 kuma-brew-sync /usr/libexec/kuma-brew-sync\n");
        out.push_str(
            "COPY kuma-brew-sync.service /usr/lib/systemd/system/kuma-brew-sync.service\n",
        );
        out.push_str("COPY kuma-brew-sync.timer /usr/lib/systemd/system/kuma-brew-sync.timer\n");
        out.push_str(
            "RUN systemctl enable kuma-brew-setup.service kuma-brew-sync.service kuma-brew-sync.timer\n",
        );
    }

    if !config.packages.rpm.is_empty() {
        out.push('\n');
        out.push_str(&dnf_install(&config.packages.rpm.join(" ")));
    }

    // Named rather than inherited. openssh-server is in the composed base
    // and Fedora's RPM preset already enables it, so every kuma image has
    // run a listening network service that nothing here chose. This
    // changes no behaviour; it makes the choice kuma's, and testable. A
    // preset flip upstream would otherwise silently change what a kuma
    // machine exposes, in either direction: a base that stopped enabling
    // sshd would take `kuma vm` and the boot smoke stage with it, since
    // both reach the guest over ssh.
    //
    // Placed before the declaration's own [services] block on purpose.
    // That boundary is what separates a curated default from kuma's
    // floor: a desktop's units are enabled above it and an owner's
    // `disable` can override them, while greenboot, fwupd, and the
    // timezone adoption come after and cannot be turned off. sshd is a
    // default, not a floor.
    out.push_str("\nRUN systemctl enable sshd.service\n");

    let services: Vec<String> = config
        .services
        .enable
        .iter()
        .map(|s| format!("systemctl enable {s}"))
        .chain(config.services.disable.iter().map(|s| format!("systemctl disable {s}")))
        .collect();
    if !services.is_empty() {
        out.push_str(&format!("\nRUN {}\n", services.join(" && ")));
    }

    // FUSE 2, in every image, so an AppImage runs by being executable.
    //
    // An AppImage is a squashfs the runtime mounts over FUSE before it
    // starts, and Fedora ships only FUSE 3: `fuse3-libs` and
    // `fusermount3`. The runtime asks for `libfuse.so.2` by name, so a
    // downloaded AppImage on a stock kuma machine died at `dlopen():
    // error loading libfuse.so.2` before any of its own code ran.
    //
    // Both packages, because neither implies the other and each one
    // alone gets a different failure. `fuse` requires only fuse-common
    // and `which`, so on its own the dlopen still fails; `fuse-libs`
    // alone loads the library and then dies at `failed to exec
    // fusermount`, since libfuse.so.2 mounts by exec'ing the setuid
    // helper that only `fuse` ships. Measured against a real AppImage,
    // all three ways.
    //
    // Not gated on a desktop. AppImages are how a lot of software is
    // shipped to Linux at all, the two packages are well under a
    // megabyte, and the failure they prevent is one a person hits by
    // double-clicking a file they downloaded, which is the worst place
    // to learn that a declaration needed another line. Coexists with
    // FUSE 3: separate libraries, separate helpers, shared fuse-common.
    out.push('\n');
    out.push_str(&dnf_install("fuse fuse-libs"));

    // Boot health, in every image: greenboot arms a GRUB boot counter on
    // the first boot of each new deployment; a boot that never reaches
    // the health check leaves the counter counting down, GRUB falls back
    // to the previous deployment when it hits zero, and greenboot makes
    // that permanent with `bootc rollback`. A bad update costs reboots,
    // not the machine. Rollback triggers only for freshly-updated-into
    // deployments (ConditionNeedsUpdate arms the trigger), so a
    // previously-good deployment that starts failing demands a human
    // instead of rolling back pointlessly. Core package only:
    // greenboot-default-health-checks ships a *required* DNS probe that
    // assumes an always-networked IoT box — it would roll back a laptop
    // that happens to boot offline.
    out.push('\n');
    out.push_str(&dnf_install("greenboot"));
    if config.system.desktop != Desktop::None {
        out.push_str(
            "COPY --chmod=755 kuma-greeter-check /usr/lib/greenboot/check/required.d/50-kuma-greeter.sh\n",
        );

        // kuma's own verbs, in whatever launcher the session shipped.
        // On both desktops, deliberately: the seam is the thing being
        // tested, and it is only a seam if it is not niri's.
        out.push_str("COPY --chmod=755 kuma-launch /usr/libexec/kuma-launch\n");
        for entry in seam::ENTRIES {
            out.push_str(&format!("COPY {}.desktop {}\n", entry.id, seam::path(entry)));
        }
        // The build validates what it generated rather than leaving it
        // to the smoke stage. A malformed entry does not fail anything
        // at runtime: it is silently skipped, so the verb simply is not
        // in the launcher and nothing anywhere says why.
        out.push_str(&format!(
            "RUN desktop-file-validate {}\n",
            seam::ENTRIES.iter().map(seam::path).collect::<Vec<String>>().join(" ")
        ));
        // And that what the entries name is in the image.
        // desktop-file-validate reads the syntax of the `Exec` line and
        // never whether the program on it exists, which is the half that
        // breaks when a package moves. The icon is checked for the same
        // reason and by file: an entry whose icon does not resolve draws
        // a blank square, which is not an error anywhere.
        //
        // `/usr/bin/kuma` is deliberately not checked here — it is copied
        // much later in the file and proved runnable there, and a guard
        // before its COPY proves nothing.
        out.push_str("RUN test -x /usr/libexec/kuma-launch \\\n");
        // Every icon, not the first one. The deleted icon_theme() step
        // failed per icon and searched for the file; checking one name
        // and calling it "the icons are checked" is how the other seven
        // ship as blank squares when Adwaita moves a name.
        for (i, entry) in seam::ENTRIES.iter().enumerate() {
            let last = i + 1 == seam::ENTRIES.len();
            out.push_str(&format!(
                "    && find /usr/share/icons/Adwaita -name {}.svg | grep -q .{}\n",
                entry.icon,
                if last { "" } else { " \\" }
            ));
        }
    }
    out.push_str("COPY --chmod=755 kuma-boot-health-sync /usr/libexec/kuma-boot-health-sync\n");
    out.push_str(
        "COPY kuma-boot-health-sync.service /usr/lib/systemd/system/kuma-boot-health-sync.service\n",
    );
    out.push_str(
        "COPY kuma-swap-fcontext /etc/selinux/targeted/contexts/files/file_contexts.local\n",
    );
    out.push_str("COPY kuma-swap-label.service /usr/lib/systemd/system/kuma-swap-label.service\n");
    out.push_str("COPY --chmod=755 kuma-fstab-sync /usr/libexec/kuma-fstab-sync\n");
    out.push_str("COPY kuma-fstab-sync.service /usr/lib/systemd/system/kuma-fstab-sync.service\n");
    out.push_str(
        "COPY kuma-boot-titles.service /usr/lib/systemd/system/kuma-boot-titles.service\n",
    );
    out.push_str(
        "RUN systemctl enable greenboot-healthcheck.service greenboot-set-rollback-trigger.service greenboot-success.target kuma-boot-health-sync.service kuma-fstab-sync.service kuma-boot-titles.service kuma-swap-label.service\n",
    );

    // What the machine will and will not accept from a registry. On every
    // image rather than only on published ones: the machine that needs
    // this is the one that installed from the published image and then
    // updates from it, and that machine's /etc comes from whatever image
    // it was installed from.
    out.push_str(&format!("\nCOPY cosign.pub {COSIGN_PUB_PATH}\n"));
    out.push_str("COPY containers-policy.json /etc/containers/policy.json\n");
    out.push_str("COPY kuma-sigstore.yaml /etc/containers/registries.d/kuma-sigstore.yaml\n");

    // Refreshes LVFS metadata only; it never applies a firmware update on
    // its own. Applying stays a deliberate act — `fwupdmgr update`, or the
    // org.gnome.Firmware flatpak the examples declare, which drives this
    // same daemon over the system bus.
    out.push_str("RUN systemctl enable fwupd-refresh.timer\n");

    if config.snapshots.enable {
        // btrfs-progs is named rather than assumed: it happens to ride in
        // today, and a snapshot timer that dies on a missing binary would
        // be a backup that silently isn't one.
        out.push('\n');
        out.push_str(&dnf_install("btrfs-progs"));
        out.push_str("COPY --chmod=755 kuma-snapshot /usr/libexec/kuma-snapshot\n");
        out.push_str("COPY kuma-snapshot.service /usr/lib/systemd/system/kuma-snapshot.service\n");
        out.push_str("COPY kuma-snapshot.timer /usr/lib/systemd/system/kuma-snapshot.timer\n");
        out.push_str("RUN systemctl enable kuma-snapshot.timer\n");
    }

    // Inside the snapshots gate would read as tidier and would be wrong:
    // validation already refuses backup.enable without snapshots.enable,
    // so nesting it would hide that dependency behind an `if` instead of
    // stating it where somebody reading the Containerfile can see it.
    if config.backup.enable {
        // restic is named for the same reason btrfs-progs is above: a
        // timer that dies on a missing binary is a backup that silently
        // is not one. Fedora packages it, so nothing is vendored.
        out.push('\n');
        out.push_str(&dnf_install("restic"));
        out.push_str("COPY --chmod=755 kuma-backup /usr/libexec/kuma-backup\n");
        out.push_str("COPY kuma-backup.service /usr/lib/systemd/system/kuma-backup.service\n");
        out.push_str("COPY kuma-backup.timer /usr/lib/systemd/system/kuma-backup.timer\n");
        out.push_str("RUN systemctl enable kuma-backup.timer\n");
        // The other end of the promise. Enabled always and gated on a
        // request file, because the machine that needs it has been
        // installed exactly once and there is nobody to start it.
        out.push_str("COPY --chmod=755 kuma-restore /usr/libexec/kuma-restore\n");
        out.push_str("COPY kuma-restore.service /usr/lib/systemd/system/kuma-restore.service\n");
        out.push_str("RUN systemctl enable kuma-restore.service\n");
    }

    // Every image can adopt a kuma vm host timezone; no-op on hardware.
    out.push_str("\nCOPY --chmod=755 kuma-vm-timezone /usr/libexec/kuma-vm-timezone\n");
    out.push_str(
        "COPY kuma-vm-timezone.service /usr/lib/systemd/system/kuma-vm-timezone.service\n",
    );
    out.push_str("RUN systemctl enable kuma-vm-timezone.service\n");

    if let Some(tz) = &config.system.timezone {
        // test -e first so a typo'd zone fails the build instead of
        // silently producing a dangling /etc/localtime symlink.
        out.push_str(&format!(
            "\nRUN test -e /usr/share/zoneinfo/{tz} && ln -sfn /usr/share/zoneinfo/{tz} /etc/localtime\n"
        ));
    }
    // The converger ships in EVERY image, including ones that declare no
    // account. A published image declares none by design, and a machine
    // installed from it has no account and no root password, so something
    // has to write /var/lib/kuma/user on the target and something has to act
    // on it at first boot. Shipping the unit only when the image already
    // knows the answer is what made that impossible.
    //
    // It is a no-op with neither file present, so a desktop image built
    // from a userless declaration gains one oneshot that exits 0.
    // Before the account converger it is ordered against, and in every
    // image for the same reason that one is: the machine an image gets
    // installed onto is where this matters, and the image cannot know
    // whether that will happen.
    out.push_str("\nCOPY --chmod=755 kuma-home-subvol /usr/libexec/kuma-home-subvol\n");
    out.push_str(
        "COPY kuma-home-subvol.service /usr/lib/systemd/system/kuma-home-subvol.service\n",
    );
    out.push_str("RUN systemctl enable kuma-home-subvol.service\n");

    out.push_str("\nCOPY --chmod=755 kuma-user-sync /usr/libexec/kuma-user-sync\n");
    out.push_str("COPY kuma-user-sync.service /usr/lib/systemd/system/kuma-user-sync.service\n");
    out.push_str("RUN systemctl enable kuma-user-sync.service\n");
    if let Some(user) = &config.user {
        // 600: only the root-run sync service reads this, and it can carry
        // the password hash — no reason to hand that to every local user.
        out.push_str("COPY --chmod=600 kuma-user /usr/lib/kuma/user\n");
        if let Some(shell) = &user.shell {
            // after the rpm layer, so a shell the config forgot to install
            // fails the build instead of locking the account out at login
            out.push_str(&format!("RUN test -x /usr/bin/{shell}\n"));
        }
        if !user.ssh_keys.is_empty() {
            out.push_str(&format!("COPY kuma-user-keys /etc/kuma/keys/{}\n", user.name));
            out.push_str("COPY kuma-sshd-keys.conf /etc/ssh/sshd_config.d/40-kuma-keys.conf\n");
        }
    }

    // A declared system shell gets the same build-time guard a declared
    // user's does, and needs it more: nothing on a published image will
    // notice it is wrong until an installer creates an account with it,
    // by which point a disk has been written. Same placement reasoning,
    // after the rpm layer that would install it.
    if let Some(shell) = &config.system.shell {
        out.push_str(&format!("RUN test -x /usr/bin/{shell}\n"));
    }

    // /etc/hostname ships in every image because DEFAULT_HOSTNAME can't
    // win: the initrd's dracut-built os-release still says "fedora", its
    // systemd sets the kernel hostname first, and the real root won't
    // override a hostname that's already set. Image /etc is only the
    // ostree merge default, so a machine whose admin set a hostname
    // keeps it. COPY, never a RUN redirect: buildah bind-mounts
    // /etc/hostname (like /etc/hosts) into every RUN container, so a
    // redirect writes the runtime mount and never reaches the layer.
    out.push_str("\nCOPY hostname /etc/hostname\n");
    if let Some(locale) = &config.system.locale {
        // The langpack makes the locale actually exist; without it glibc
        // silently falls back and every app renders C.UTF-8.
        if let Some(lang) = langpack(locale) {
            out.push('\n');
            out.push_str(&dnf_install(&format!("glibc-langpack-{lang}")));
        }
        out.push_str(&format!("RUN echo 'LANG={locale}' > /etc/locale.conf\n"));
    }

    // Anchors before branding only because everything after this point
    // is cosmetic; what matters is that they land before anything that
    // might need to trust them, and that update-ca-trust runs in the
    // same layer that adds them rather than being left for a boot.
    if !config.system.ca_certificates.is_empty() {
        out.push('\n');
        for name in config.system.ca_certificates.keys() {
            out.push_str(&format!(
                "COPY ca-{name}.crt /etc/pki/ca-trust/source/anchors/{name}.crt\n"
            ));
        }
        out.push_str("RUN update-ca-trust\n");
    }

    out.push_str(&branding());

    // The machine gets the kuma that built it. Everything else needed to
    // run kuma on a machine already shipped — the baked declaration below,
    // the convergence units, thirteen helpers in /usr/libexec — but not
    // the binary that drives them, so an ISO-installed machine could not
    // run the `kuma update --yes` docs/agents.md promises it, and the
    // fallback-to-baked-declaration path had nothing to execute it.
    //
    // current_exe rather than a download: no network in the build, and no
    // version skew between the kuma that wrote this image and the kuma
    // that ships in it. The cost is that the binary is the build host's,
    // so a musl host, a different arch, or a glibc newer than the base's
    // produces one this image cannot execute.
    //
    // Which is what the RUN is for. Without it the COPY succeeds, the
    // image ships, and the failure surfaces at first boot as a machine
    // whose kuma is an ELF it cannot run — the same class of far-end
    // failure as a shell that was never installed, and guarded the same
    // way (`RUN test -x /usr/bin/{shell}` above).
    //
    // Late in the file, beside the declaration, because both layers
    // change on every edit.
    out.push_str("\nCOPY --chmod=755 kuma /usr/bin/kuma\n");
    out.push_str("RUN /usr/bin/kuma --version\n");

    // The image carries the declaration it was built from, verbatim: the
    // machine stays self-describing when the original file is gone, and
    // `kuma init` seeds a working copy from it. No new secret exposure —
    // password_hash already ships in the kuma-user declaration. Late in
    // the file because this layer changes on every edit.
    out.push_str("\nCOPY kuma.toml /usr/lib/kuma/kuma.toml\n");

    // What `kuma build` prunes by: each rebuild strands the previous
    // image as a dangling <none>, and only kuma's own should be reclaimed.
    out.push_str("\nLABEL io.kuma.image=\"1\"\n");

    // Which kuma generated this. The declaration alone cannot answer "is
    // this image the one that has my last change in it": an unchanged
    // declaration built by an older binary produces a perfectly current
    // looking image, so the probe reports in-sync and is right by its own
    // definition. Same question VERSION's own doc comment exists for, one
    // level up, and the same cost when it goes unanswered.
    //
    // A label rather than reading /usr/bin/kuma out of the image: the
    // probe behind bare `kuma` is meant to stay cheap, and `podman image
    // inspect` already runs there for the id and the timestamp. Running a
    // container to ask the binary its version would not.
    out.push_str(&format!("LABEL io.kuma.builder=\"{}\"\n", crate::VERSION));

    out.push_str(SWEEP);
    out.push_str(LINT);
    out
}

/// Write the full build context: the Containerfile plus any files it COPYs.
/// "de_DE.UTF-8" → "de": the glibc langpack that provides the locale.
/// Locales without a territory part (C, POSIX, C.UTF-8) need none.
fn langpack(locale: &str) -> Option<&str> {
    let lang = locale.split('_').next()?;
    (locale.contains('_')
        && (2..=3).contains(&lang.len())
        && lang.chars().all(|c| c.is_ascii_lowercase()))
    .then_some(lang)
}

/// `config_text` is the declaration verbatim — comments and formatting
/// intact — because it gets baked into the image at /usr/lib/kuma/kuma.toml.
///
/// `kuma_binary` is the running kuma, passed rather than resolved here so
/// the tests can stage a stub instead of copying a 42 MB test harness into
/// a temp directory fourteen times — the same reason `loop_mounts_in` takes
/// its input and `scan_etc` takes its roots.
pub fn write_context(
    config: &Config,
    config_text: &str,
    kuma_binary: &Path,
    dir: &Path,
) -> Result<()> {
    std::fs::write(dir.join("kuma.toml"), config_text)?;
    std::fs::copy(kuma_binary, dir.join("kuma"))
        .with_context(|| format!("staging {} into the build context", kuma_binary.display()))?;
    std::fs::write(dir.join("Containerfile"), generate(config))?;
    // The same default `kuma install` falls back to, whose own doc
    // comment claims it "matches what every kuma image bakes" — an
    // invariant that was asserted and not shared.
    let hostname = config.system.hostname.as_deref().unwrap_or(crate::install::DEFAULT_HOSTNAME);
    std::fs::write(dir.join("hostname"), format!("{hostname}\n"))?;
    std::fs::write(dir.join("kuma-vm-timezone"), VM_TZ_SCRIPT)?;
    std::fs::write(dir.join("kuma-vm-timezone.service"), VM_TZ_SERVICE)?;
    std::fs::write(dir.join("kuma-boot-health-sync"), BOOT_HEALTH_SYNC_SCRIPT)?;
    std::fs::write(dir.join("kuma-boot-health-sync.service"), BOOT_HEALTH_SYNC_SERVICE)?;
    std::fs::write(dir.join("kuma-swap-fcontext"), SWAP_FCONTEXT)?;
    std::fs::write(dir.join("kuma-swap-label.service"), SWAP_LABEL_SERVICE)?;
    std::fs::write(dir.join("kuma-fstab-sync"), FSTAB_SYNC_SCRIPT)?;
    std::fs::write(dir.join("kuma-fstab-sync.service"), FSTAB_SYNC_SERVICE)?;
    std::fs::write(dir.join("kuma-boot-titles.service"), BOOT_TITLES_SERVICE)?;
    std::fs::write(dir.join("containers-policy.json"), signature_policy())?;
    std::fs::write(dir.join("kuma-sigstore.yaml"), registries_d())?;
    std::fs::write(dir.join("cosign.pub"), COSIGN_PUB)?;
    // Identity, wallpaper, and kargs ship with every desktop; the rest
    // of the niri block is glue COSMIC provides natively.
    if config.system.desktop != Desktop::None {
        std::fs::write(dir.join("kargs-desktop.toml"), DESKTOP_KARGS)?;
        std::fs::write(dir.join("fastfetch-config.jsonc"), FASTFETCH_CONFIG)?;
        std::fs::write(dir.join("fastfetch-logo.txt"), FASTFETCH_LOGO)?;
        std::fs::write(dir.join("kuma-wallpaper.jpg"), WALLPAPER)?;
        std::fs::write(dir.join("kuma-greeter-check"), GREETER_CHECK)?;
        std::fs::write(dir.join("kuma-launch"), KUMA_LAUNCH)?;
        for entry in seam::ENTRIES {
            std::fs::write(dir.join(format!("{}.desktop", entry.id)), seam::render(entry))?;
        }
    }
    if config.system.desktop == Desktop::Cosmic {
        std::fs::write(dir.join("cosmic-favorites"), COSMIC_FAVORITES)?;
        std::fs::write(dir.join("cosmic-background"), COSMIC_BACKGROUND)?;
    }
    if config.system.desktop == Desktop::Niri {
        std::fs::write(dir.join("greetd-config.toml"), greetd_config(config))?;
        std::fs::write(dir.join("niri-extras.kdl"), NIRI_EXTRAS)?;
        std::fs::write(dir.join("noctalia-config.toml"), KUMA_NOCTALIA)?;
        std::fs::write(dir.join("kitty.conf"), KITTY_CONFIG)?;
        std::fs::write(dir.join("kuma-clipboard-bridge"), clipboard_bridge())?;
        std::fs::write(dir.join("kuma-xsettings"), xsettings_launcher())?;
        std::fs::write(dir.join("xsettingsd.conf"), XSETTINGSD_CONF)?;
        std::fs::write(dir.join("niri-binds.kdl"), NIRI_MEDIA_BINDS)?;
        std::fs::write(dir.join("mimeapps.list"), MIMEAPPS)?;
        std::fs::write(dir.join("kuma-record"), RECORD_SCRIPT)?;
        std::fs::write(dir.join("kuma-battery-watch"), BATTERY_WATCH)?;
        std::fs::write(dir.join("kuma-shell.service"), SHELL_SERVICE)?;
        std::fs::write(dir.join("kuma-sleep-guard.service"), SLEEP_GUARD_SERVICE)?;
        std::fs::write(dir.join("kuma-sleep-guard"), SLEEP_GUARD)?;
        std::fs::write(dir.join("kuma-osd"), OSD_SCRIPT)?;
        std::fs::write(dir.join("gtk3-settings.ini"), GTK3_SETTINGS_INI)?;
        std::fs::write(dir.join("gtk4-settings.ini"), GTK4_SETTINGS_INI)?;
        std::fs::write(dir.join("dconf-profile"), DCONF_PROFILE)?;
        std::fs::write(dir.join("dconf-kuma-dark"), DCONF_DARK)?;
        std::fs::write(dir.join("dconf-kuma-blueman"), DCONF_BLUEMAN)?;
        std::fs::write(dir.join("autostart-blueman"), autostart_off("Blueman Applet"))?;
        std::fs::write(
            dir.join("autostart-polkit-mate"),
            autostart_off("PolicyKit Authentication Agent"),
        )?;
    }
    if config.system.desktop != Desktop::None || !config.packages.flatpak.is_empty() {
        let mut list = config.packages.flatpak.join("\n");
        if !list.is_empty() {
            list.push('\n');
        }
        std::fs::write(dir.join("flatpaks"), list)?;
        std::fs::write(dir.join("kuma-flatpak-sync"), FLATPAK_SYNC_SCRIPT)?;
        std::fs::write(dir.join("kuma-flatpak-sync.service"), FLATPAK_SYNC_SERVICE)?;
        std::fs::write(dir.join("kuma-flatpak-sync.timer"), FLATPAK_SYNC_TIMER)?;
        for scope in [crate::config::Scope::System, crate::config::Scope::User] {
            let scoped = dir.join("overrides").join(scope.as_str());
            std::fs::create_dir_all(&scoped)?;
            for (app, over) in &config.overrides {
                if over.scope == scope {
                    std::fs::write(scoped.join(app), crate::overrides::render(over))?;
                }
            }
        }
        std::fs::write(dir.join("kuma-flatpak-overrides.service"), FLATPAK_OVERRIDES_SERVICE)?;
        std::fs::write(
            dir.join("kuma-flatpak-overrides-user.service"),
            FLATPAK_OVERRIDES_USER_SERVICE,
        )?;
    }
    // Outside the flatpak gate: trust has nothing to do with apps.
    for (name, pem) in &config.system.ca_certificates {
        std::fs::write(dir.join(format!("ca-{name}.crt")), pem)?;
    }
    if config.snapshots.enable {
        std::fs::write(dir.join("kuma-snapshot"), snapshot_script(config))?;
        std::fs::write(dir.join("kuma-snapshot.service"), SNAPSHOT_SERVICE)?;
        std::fs::write(
            dir.join("kuma-snapshot.timer"),
            snapshot_timer(&config.snapshots.interval),
        )?;
    }
    if config.backup.enable {
        std::fs::write(dir.join("kuma-backup"), backup_script(config))?;
        std::fs::write(dir.join("kuma-backup.service"), backup_service(config))?;
        std::fs::write(dir.join("kuma-backup.timer"), backup_timer(&config.backup.interval))?;
        std::fs::write(dir.join("kuma-restore"), RESTORE_SCRIPT)?;
        std::fs::write(dir.join("kuma-restore.service"), RESTORE_SERVICE)?;
    }
    // Unconditional, like the Containerfile lines that copy them: the
    // converger has to be present in an image that declares no account,
    // because that is the image an installer writes /var/lib/kuma/user onto.
    std::fs::write(dir.join("kuma-user-sync"), USER_SYNC_SCRIPT)?;
    std::fs::write(dir.join("kuma-user-sync.service"), USER_SYNC_SERVICE)?;
    std::fs::write(dir.join("kuma-home-subvol"), HOME_SUBVOL_SCRIPT)?;
    std::fs::write(dir.join("kuma-home-subvol.service"), HOME_SUBVOL_SERVICE)?;
    if let Some(user) = &config.user {
        let mut decl = format!("KUMA_USER='{}'\n", user.name);
        if let Some(shell) = &user.shell {
            decl.push_str(&format!("KUMA_SHELL='/usr/bin/{shell}'\n"));
        }
        if !user.groups.is_empty() {
            decl.push_str(&format!("KUMA_GROUPS='{}'\n", user.groups.join(" ")));
        }
        if let Some(hash) = &user.password_hash {
            decl.push_str(&format!("KUMA_PASSWORD_HASH='{hash}'\n"));
        }
        std::fs::write(dir.join("kuma-user"), decl)?;
        if !user.ssh_keys.is_empty() {
            let mut keys = user.ssh_keys.join("\n");
            keys.push('\n');
            std::fs::write(dir.join("kuma-user-keys"), keys)?;
            std::fs::write(dir.join("kuma-sshd-keys.conf"), SSHD_KUMA_KEYS)?;
        }
    }
    if config.system.brew || !config.packages.brew.is_empty() {
        std::fs::write(dir.join("kuma-brew-setup"), BREW_SETUP_SCRIPT)?;
        std::fs::write(dir.join("kuma-brew-setup.service"), BREW_SETUP_SERVICE)?;
        std::fs::write(dir.join("brew-profile.sh"), BREW_PROFILE_SH)?;
        std::fs::write(dir.join("brew-profile.fish"), BREW_PROFILE_FISH)?;
        let mut list = config.packages.brew.join("\n");
        if !list.is_empty() {
            list.push('\n');
        }
        std::fs::write(dir.join("brews"), list)?;
        std::fs::write(dir.join("kuma-brew-sync"), BREW_SYNC_SCRIPT)?;
        std::fs::write(dir.join("kuma-brew-sync.service"), BREW_SYNC_SERVICE)?;
        std::fs::write(dir.join("kuma-brew-sync.timer"), BREW_SYNC_TIMER)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(toml: &str) -> Config {
        let config: Config = toml::from_str(toml).unwrap();
        config.validate().unwrap();
        config
    }

    /// A stand-in for the kuma binary: write_context only copies it, and
    /// the real one here would be the 42 MB test harness. Dot-prefixed so
    /// it cannot collide with a name the context actually uses.
    fn context(toml: &str, dir: &Path) {
        let stub = dir.join(".stub-kuma");
        std::fs::write(&stub, b"not really a binary\n").unwrap();
        write_context(&config(toml), toml, &stub, dir).unwrap();
    }

    /// The declaration, the units, and the helpers all shipped while the
    /// binary that drives them did not, so an installed machine had no way
    /// to run the `kuma update` docs/agents.md promises it. Every image,
    /// not just VM disks: an ISO install has the same hole.
    #[test]
    fn every_image_ships_the_kuma_that_built_it() {
        let out = generate(&config("schema_version = 1"));
        assert!(out.contains("COPY --chmod=755 kuma /usr/bin/kuma"));
        let dir = tempfile::tempdir().unwrap();
        context("schema_version = 1", dir.path());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("kuma")).unwrap(),
            "not really a binary\n"
        );
    }

    /// An AppImage is a squashfs its runtime mounts over FUSE 2 before
    /// any of its own code runs, and Fedora ships only FUSE 3.
    ///
    /// Both packages named exactly, because each one alone fails and
    /// they fail differently: without `fuse-libs` the runtime cannot
    /// load libfuse.so.2, and without `fuse` the loaded library cannot
    /// exec the setuid `fusermount` it mounts with. Half of this fix
    /// looks like the whole of it right up until somebody runs an
    /// AppImage.
    #[test]
    fn appimages_run_on_every_image() {
        for toml in ["schema_version = 1", "schema_version = 1\n[system]\ndesktop = \"niri\""] {
            assert!(
                generate(&config(toml)).contains(&dnf_install("fuse fuse-libs")),
                "every image needs both halves of FUSE 2"
            );
        }
    }

    /// Every file the Containerfile copies is a file the build context
    /// actually holds.
    ///
    /// The two halves live hundreds of lines apart: a `COPY` is pushed
    /// where the feature is assembled and the file is written where the
    /// context is staged, so adding one and forgetting the other is a
    /// build that dies at `podman build` on somebody's machine, minutes
    /// in, with a message about a file nobody named. Asserted over the
    /// whole file rather than per feature, so the next one is covered
    /// without anybody remembering to cover it.
    #[test]
    fn every_copied_file_is_staged() {
        // EVERYTHING_ON is the point. Six features decide separately, in
        // `generate` and again in `write_context`, whether they ship,
        // and five of the six default to off: this test claimed to cover
        // the next feature automatically while running only declarations
        // that entered neither branch. A COPY added under a gate with no
        // matching stage passed here and died at `podman build`.
        for toml in [
            "schema_version = 1",
            "schema_version = 1\n[system]\ndesktop = \"niri\"",
            EVERYTHING_ON,
        ] {
            let dir = tempfile::tempdir().unwrap();
            context(toml, dir.path());
            let containerfile = std::fs::read_to_string(dir.path().join("Containerfile")).unwrap();
            let sources: Vec<String> = containerfile
                .lines()
                .filter_map(|line| {
                    let rest = line.strip_prefix("COPY ")?;
                    // the source is the first word that is not a --flag
                    rest.split_whitespace().find(|word| !word.starts_with("--")).map(String::from)
                })
                .collect();
            for source in &sources {
                assert!(
                    dir.path().join(source).exists(),
                    "the Containerfile copies {source}, which nothing stages"
                );
            }

            // And the other direction, which was untested and is the
            // silent one: a file staged and never copied is a unit that
            // simply is not in the image, with nothing to say so.
            for entry in std::fs::read_dir(dir.path()).unwrap().flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                // `kuma` is the binary the build copies by a path this
                // parse does not see, Containerfile is the recipe
                // itself, and `.stub-kuma` is this harness standing in
                // for the 42 MB binary (dot-prefixed for exactly that
                // reason, see `context`).
                if name == "kuma" || name == "Containerfile" || name.starts_with('.') {
                    continue;
                }
                assert!(
                    sources.contains(&name),
                    "{name} is staged into the build context and nothing copies it"
                );
            }
        }
    }

    /// The staged binary is the build host's, so a musl host or a foreign
    /// arch produces one the image cannot execute. Running it during the
    /// build is what makes that a build failure rather than a machine that
    /// boots with a kuma it cannot run, so the order is the whole point:
    /// a guard before its COPY proves nothing.
    #[test]
    fn the_staged_binary_is_proved_runnable_before_the_image_ships() {
        let out = generate(&config("schema_version = 1"));
        let copied = out.find("COPY --chmod=755 kuma /usr/bin/kuma").unwrap();
        let proved = out.find("RUN /usr/bin/kuma --version").unwrap();
        assert!(copied < proved, "the guard must run after the binary lands");
    }

    /// The image records which kuma generated it, so the probe can answer
    /// "is this image the one that has my last change in it". A label
    /// rather than the binary inside the image, because bare `kuma` is a
    /// cheap probe and already runs `podman image inspect`.
    #[test]
    fn the_image_records_which_kuma_built_it() {
        let out = generate(&config("schema_version = 1"));
        assert!(
            out.contains(&format!("LABEL io.kuma.builder=\"{}\"", crate::VERSION)),
            "no builder label in:\n{out}"
        );
    }

    #[test]
    fn image_carries_its_declaration_verbatim() {
        let toml = "schema_version = 1\n# a comment worth keeping\n";
        let out = generate(&config(toml));
        assert!(out.contains("COPY kuma.toml /usr/lib/kuma/kuma.toml"));
        let dir = tempfile::tempdir().unwrap();
        context(toml, dir.path());
        assert_eq!(std::fs::read_to_string(dir.path().join("kuma.toml")).unwrap(), toml);
    }

    #[test]
    fn minimal_config_is_base_boot_health_and_lint() {
        let out = generate(&config("schema_version = 1"));
        // The default base is kuma's own composed one, content-addressed
        // so the FROM is deterministic before any compose has run.
        assert!(out.contains("FROM localhost/kuma-base:m"));
        // Two dnf layers even in a minimal image, and both are promises
        // rather than features: greenboot's never-worse-than-before
        // rollback, and the FUSE 2 pair that lets a downloaded AppImage
        // run without a declaration naming it. Everything else is
        // opt-in, and the count is what keeps it that way.
        assert_eq!(out.matches("dnf -y install").count(), 2);
        assert!(out.contains(&dnf_install("greenboot")));
        assert!(out.contains(&dnf_install("fuse fuse-libs")));
        assert!(out.contains("bootc container lint"));
        // The lint runs unqualified first: the --skip is a fallback for
        // one upstream crash, never the path a healthy build takes, and
        // it must not swallow any other lint failure. Pin the shape so a
        // future edit can't turn it into a blanket skip.
        assert!(out.contains("RUN said=$(bootc container lint 2>&1); rc=$?;"));
        assert!(out.contains("grep -q 'var-tmpfiles: I/O error'"));
        assert!(out.contains("bootc container lint --skip var-tmpfiles || rc=$?"));
        // the build's exit status is still the lint's
        assert!(out.contains("exit $rc"));
        // Nothing on disk: a scratch file here is litter the lint itself
        // reads back and reports, since /tmp is one of the directories
        // it checks.
        assert!(!out.contains("lint.err"));
    }

    /// Every image was shipping dnf's log and a /run full of state that a
    /// booted machine creates for itself. Two bootc lint warnings said so
    /// on every build, and warnings cannot fail a build, so they were
    /// noise nobody had to act on.
    #[test]
    fn build_litter_is_swept_before_the_image_is_judged() {
        let out = generate(&config("schema_version = 1"));
        assert!(out.contains("find /run /tmp -mindepth 1 -delete"));
        assert!(out.contains("find /var/log -type f -delete"));

        // The sweep has to precede the lint, or the lint passes judgement
        // on an image that is not the one being shipped.
        let sweep = out.find("find /run /tmp -mindepth 1 -delete").unwrap();
        let lint = out.find("RUN said=$(bootc container lint").unwrap();
        assert!(sweep < lint);

        // The logs are asserted rather than assumed. /run cannot be: the
        // build holds two paths inside it open, so nothing there can be
        // checked for emptiness from within the build itself.
        assert!(out.contains("! find /var/log -type f | grep -q ."));
    }

    #[test]
    fn full_config_generates_all_sections() {
        let out = generate(&config(
            r#"
            schema_version = 1
            [system]
            base = "ghcr.io/example/kuma-gnome:latest"
            [packages]
            rpm = ["fish", "tailscale"]
            [services]
            enable = ["tailscaled.service"]
            disable = ["cups.service"]
            "#,
        ));
        assert!(out.contains("FROM ghcr.io/example/kuma-gnome:latest"));
        assert!(out.contains(&dnf_install("fish tailscale")));
        assert!(out.contains("systemctl enable tailscaled.service"));
        assert!(out.contains("systemctl disable cups.service"));
    }

    #[test]
    fn niri_desktop_generates_curated_layer() {
        let out = generate(&config("schema_version = 1\n[system]\ndesktop = \"niri\"\n"));
        assert!(out.contains("niri"));
        assert!(out.contains("greetd"));
        assert!(out.contains("NetworkManager-wifi"));
        // full GPU acceleration: RADV for Vulkan, freeworld VA-API for
        // the codecs Fedora's build strips
        assert!(out.contains("mesa-vulkan-drivers"));
        assert!(out.contains("rpmfusion-free-release"));
        assert!(out.contains("--allowerasing mesa-va-drivers-freeworld"));
        assert!(out.contains("COPY greetd-config.toml /etc/greetd/config.toml"));
        assert!(out.contains("niri validate --config /etc/niri/config.kdl"));
        assert!(out.contains("systemctl set-default graphical.target"));
        assert!(out.contains("greetd.service firewalld.service power-profiles-daemon.service"));
        assert!(out.contains("mask flatpak-add-fedora-repos.service"));
        // bare import-environment is deprecated; the patched call must keep
        // the session vars niri.service needs to claim the logind seat
        assert!(out.contains("import-environment PATH XDG_SESSION_ID XDG_SEAT XDG_VTNR"));
    }

    #[test]
    fn branding_always_applied() {
        let out = generate(&config("schema_version = 1"));
        assert!(out.contains("NAME=\"Kuma\""));
        assert!(out.contains("ID=kuma"));
        assert!(out.contains("ID_LIKE=\\\"fedora\\\"") || out.contains("ID_LIKE=\"fedora\""));
        // branding must come after every dnf layer
        let brand_at = out.find("NAME=\"Kuma\"").unwrap();
        assert!(out.rfind("dnf -y install").is_none_or(|dnf_at| dnf_at < brand_at));
    }

    #[test]
    fn niri_desktop_ships_theme_and_wallpaper() {
        let out = generate(&config("schema_version = 1\n[system]\ndesktop = \"niri\"\n"));
        assert!(
            out.contains("COPY kuma-wallpaper.jpg /usr/share/backgrounds/kuma/kuma-wallpaper.jpg")
        );
        // The shell's config, in place of waybar's two files and mako's
        // one. Its reachability is checked in the build itself, see
        // the_baked_shell_config_is_proved_reachable.
        assert!(out.contains("COPY noctalia-config.toml /usr/lib/kuma/noctalia/config.toml"));
        // system-wide, never /etc/skel — skel strands existing homes on
        // stale copies (the fuzzel-DPI lesson)
        assert!(!out.contains("/etc/skel"));
        // systemd user sessions activate via SystemdService, not Exec —
        // without the drop-in the wrapper never runs where it matters
        assert!(out.contains("COPY kitty.conf /etc/xdg/kitty/kitty.conf"));
        // an unparseable theme must fail the build, not ship unthemed —
        // and unknown keys only ever reach stderr, so both halves matter
        assert!(out.contains("kitty +runpy"));
        assert!(out.contains("accumulate_bad_lines=bad"));
        assert!(out.contains("grep -q 'unknown config key' /tmp/kitty.err"));
        // The palette owns every colour the terminal shows, all sixteen
        // ANSI slots included, so the build renders the template it will
        // actually use and insists on both halves being there. The
        // image's own colours stay as the fallback for a terminal opened
        // before the shell has rendered anything.
        assert!(KITTY_CONFIG.contains("color0"));
        assert!(out.contains("grep -qE '^color0 +#[0-9a-fA-F]{6}$' /tmp/kitty-rendered.conf"));
        // and the config that turns the templates on at all
        assert!(KUMA_NOCTALIA.contains("builtin_ids = [ \"kitty\", \"gtk3\", \"gtk4\" ]"));
        // the GTK3 half only themes anything with adw-gtk3 present, and
        // GTK_THEME outranks gsettings, so all four names move together
        assert!(NIRI_PACKAGES.contains(&"adw-gtk3-theme"));
        assert!(NIRI_EXTRAS.contains("GTK_THEME \"adw-gtk3-dark\""));
        assert!(DCONF_DARK.contains("gtk-theme='adw-gtk3-dark'"));
        assert!(XSETTINGSD_CONF.contains("Net/ThemeName \"adw-gtk3-dark\""));
        assert!(GTK3_SETTINGS_INI.contains("gtk-theme-name = adw-gtk3-dark"));
        assert!(out.contains("RUN test -d /usr/share/themes/adw-gtk3-dark"));
        // niri's template would have apply.sh create ~/.config/niri/config.kdl,
        // which niri takes instead of /etc/niri/config.kdl rather than merging
        assert!(!KUMA_NOCTALIA.contains("\"niri\""));
        // upstream niri spawns alacritty; the image ships kitty, so the sed
        // must rewrite the bind, and the grep guard must keep it honest
        assert!(out.contains("grep -q '\"alacritty\"' /usr/share/doc/niri/default-config.kdl"));
        assert!(out.contains("sed -e 's/alacritty/kitty/g'"));
        // niri Recommends alacritty; without the exclude it ships anyway
        for excluded in NIRI_EXCLUDES {
            assert!(out.contains(&format!("--exclude={excluded}")), "{excluded} rides in");
            assert!(!NIRI_PACKAGES.contains(excluded), "{excluded} is both excluded and asked for");
        }
        assert!(out.contains("COPY dconf-profile /etc/dconf/profile/user"));
        assert!(out.contains("COPY dconf-kuma-dark /etc/dconf/db/local.d/10-kuma-dark"));
        assert!(out.contains("COPY dconf-kuma-blueman /etc/dconf/db/local.d/10-kuma-blueman"));
        assert!(out.contains("RUN dconf update"));
        // Both names, because disabling only StatusIcon is a no-op:
        // ShowConnected depends on it and the plugin manager loads a
        // dependency regardless of the disable flag. Dropping either name
        // brings the second bluetooth icon back, and nothing about the
        // bar would fail a test to say so.
        assert!(DCONF_BLUEMAN.contains("'!StatusIcon'"));
        assert!(DCONF_BLUEMAN.contains("'!ShowConnected'"));

        // Exactly one launch path for the two session services this image
        // both spawns and ships an autostart entry for. The pairing is
        // the point: an override here without the matching
        // spawn-at-startup would leave the machine with no bluetooth
        // agent and no polkit agent, and the second of those is silent
        // until somebody needs a password prompt.
        assert!(out.contains("COPY autostart-blueman /etc/xdg/autostart/blueman.desktop"));
        assert!(out.contains("/etc/xdg/autostart/polkit-mate-authentication-agent-1.desktop"));
        assert!(NIRI_EXTRAS.contains("spawn-at-startup \"blueman-applet\""));
        assert!(NIRI_EXTRAS.contains("polkit-mate-authentication-agent-1"));
        assert!(autostart_off("x").contains("Hidden=true"));
    }

    #[test]
    fn desktop_defaults_to_dark_and_bare_terminal() {
        assert!(DCONF_DARK.contains("color-scheme='prefer-dark'"));
        // a titlebar in a tiling compositor renders light Adwaita chrome
        assert!(KITTY_CONFIG.contains("hide_window_decorations yes"));
    }

    #[test]
    fn vm_timezone_adoption_ships_in_every_image() {
        let out = generate(&config("schema_version = 1"));
        assert!(out.contains("RUN systemctl enable kuma-vm-timezone.service"));
        assert!(VM_TZ_SCRIPT.contains("qemu_fw_cfg/by_name/opt/org.kuma.tz"));
        // guard against a garbage or hostile fw_cfg value
        assert!(VM_TZ_SCRIPT.contains("[ -e \"/usr/share/zoneinfo/$tz\" ] || exit 0"));
    }

    /// sshd was enabled on every kuma machine by Fedora's RPM preset
    /// rather than by anything here, which made a listening network
    /// service the one part of the image kuma had never decided. Naming
    /// it changed no behaviour and pins it against a preset flip
    /// upstream, in either direction: a base that stopped enabling sshd
    /// would otherwise take `kuma vm` and the boot smoke stage with it.
    #[test]
    fn sshd_is_enabled_by_name_in_every_image() {
        for declaration in [
            "schema_version = 1",
            "schema_version = 1\n[system]\ndesktop = \"niri\"\n",
            "schema_version = 1\n[system]\ndesktop = \"cosmic\"\n",
        ] {
            let out = generate(&config(declaration));
            assert!(
                out.contains("RUN systemctl enable sshd.service"),
                "sshd not named for: {declaration}"
            );
        }
    }

    /// A default, not a floor. The declaration's own [services] block is
    /// emitted after kuma's curated enables and before the units that
    /// cannot be turned off, so an owner who disables sshd gets a machine
    /// without it. Move the enable below that block and the disable
    /// becomes a silent no-op, which is the shape of bug that made the
    /// example's `disable` line fight the desktop.
    #[test]
    fn a_declared_disable_beats_the_sshd_default() {
        let out =
            generate(&config("schema_version = 1\n[services]\ndisable = [\"sshd.service\"]\n"));
        let enable = out.find("systemctl enable sshd.service").expect("sshd is enabled by name");
        let disable = out.find("systemctl disable sshd.service").expect("the declared disable");
        assert!(enable < disable, "kuma's default must not outrank the declaration");

        // The floor stays the floor: these are emitted after [services]
        // precisely so a declaration cannot switch them off.
        for floor in ["greenboot-healthcheck.service", "fwupd-refresh.timer"] {
            let at = out.find(floor).expect(floor);
            assert!(at > disable, "{floor} drifted above the declaration's [services]");
        }
    }

    #[test]
    fn boot_health_ships_in_every_image() {
        let out = generate(&config("schema_version = 1"));
        assert!(out.contains(&dnf_install("greenboot")));
        assert!(out.contains(
            "RUN systemctl enable greenboot-healthcheck.service greenboot-set-rollback-trigger.service greenboot-success.target kuma-boot-health-sync.service"
        ));
        assert!(out
            .contains("COPY --chmod=755 kuma-boot-health-sync /usr/libexec/kuma-boot-health-sync"));
        // the IoT subpackage's *required* DNS probe would roll back a
        // laptop that boots offline
        assert!(!out.contains("greenboot-default-health-checks"));
        // no desktop, no greeter to check
        assert!(!out.contains("kuma-greeter-check"));
    }

    /// Every image, because every image's menu goes stale the same way:
    /// the titles drift on any deploy that reuses the kernel, which is
    /// every kuma deploy that does not move the base.
    #[test]
    fn boot_titles_ship_in_every_image() {
        let out = generate(&config("schema_version = 1"));
        assert!(out.contains(
            "COPY kuma-boot-titles.service /usr/lib/systemd/system/kuma-boot-titles.service"
        ));
        // The enable line itself, not a bare `contains`: the COPY above
        // ends on exactly that file name, so `contains` was satisfied
        // by the line before it and dropping the unit from the enable
        // would have left this test green.
        assert!(
            out.lines().any(|line| {
                line.starts_with("RUN systemctl enable")
                    && line.contains("kuma-boot-titles.service")
            }),
            "and is enabled"
        );
        // The unit calls the binary the image ships, so the image has to
        // ship one.
        assert!(out.contains("COPY --chmod=755 kuma /usr/bin/kuma"));
    }

    #[test]
    fn boot_health_sync_converges_the_grub_hook() {
        // The heredoc block must carry the exact markers the script
        // greps and strips by — a drifted marker means the sync writes
        // a block it can never find again (double-append forever).
        let begin = "# >>> kuma boot-health >>>";
        let end = "# <<< kuma boot-health <<<";
        assert!(BOOT_HEALTH_SYNC_SCRIPT.contains(&format!("begin='{begin}'")));
        assert!(BOOT_HEALTH_SYNC_SCRIPT.contains(&format!("end='{end}'")));
        assert_eq!(BOOT_HEALTH_SYNC_SCRIPT.matches(begin).count(), 2);
        assert_eq!(BOOT_HEALTH_SYNC_SCRIPT.matches(end).count(), 2);
        // the fallback needs grub's increment module and must remove
        // itself when grub.cfg counts natively (no double decrement)
        assert!(BOOT_HEALTH_SYNC_SCRIPT.contains("insmod increment"));
        assert!(BOOT_HEALTH_SYNC_SCRIPT.contains("grep -q boot_counter \"$cfg\""));
        let dir = tempfile::tempdir().unwrap();
        context("schema_version = 1\n", dir.path());
        let script = std::fs::read_to_string(dir.path().join("kuma-boot-health-sync")).unwrap();
        assert!(script.starts_with("#!/usr/bin/bash"));
    }

    /// The cure for the fstab wart has to travel in the image. It was
    /// fixed by hand on one laptop in August 2026 and that fixed exactly
    /// one machine: nothing in a kuma image touched /etc/fstab, the bootc
    /// rpm ships no unit for it, and the ISO kickstart never mentions it,
    /// so every fresh install went on failing systemd-remount-fs forever.
    #[test]
    fn every_image_carries_the_fstab_converger() {
        let out = generate(&config("schema_version = 1"));
        assert!(out.contains("COPY --chmod=755 kuma-fstab-sync /usr/libexec/kuma-fstab-sync"));
        assert!(out.contains("kuma-fstab-sync.service"));

        let dir = tempfile::tempdir().unwrap();
        context("schema_version = 1\n", dir.path());
        let script = std::fs::read_to_string(dir.path().join("kuma-fstab-sync")).unwrap();
        assert!(script.starts_with("#!/usr/bin/bash"));

        // Three properties the edit is only safe because of, each of which
        // reads like a detail and is the whole guard:
        //
        // it asks the kernel what / actually is instead of assuming any
        // bootc machine is composefs,
        assert!(script.contains("findmnt -n -o FSTYPE /"));
        // it matches the mount point as a FIELD, because "root" appears in
        // subvolume names and /var/roothome is a real mount,
        assert!(script.contains(r#"$1 !~ /^#/ && $2 == "/""#));
        // and it writes back through the existing inode, so the file keeps
        // its mode and SELinux label instead of inheriting a temp file's.
        assert!(script.contains(r#"cat "$new" > "$fstab""#));

        // The unit is inert anywhere that is not an ostree deployment,
        // since on a package system that fstab line is load-bearing.
        assert!(FSTAB_SYNC_SERVICE.contains("ConditionPathExists=/run/ostree-booted"));
    }

    /// The one line that decides whether the boot-menu titles are ever
    /// right, stated as a test because it reads backwards and is
    /// therefore exactly the kind of thing a later reader "fixes".
    ///
    /// The rotation happens in ostree's ExecStop at shutdown. systemd
    /// stops in reverse start order, so this unit runs after that
    /// rotation only while it is ordered BEFORE finalize-staged.
    /// Flipping it to After= makes the unit start later, stop earlier,
    /// and write the titles of an arrangement that is about to change:
    /// green, quiet, and useless.
    #[test]
    fn boot_titles_runs_after_the_rotation_it_follows() {
        assert!(BOOT_TITLES_SERVICE.contains("Before=ostree-finalize-staged.service"));
        assert!(
            !BOOT_TITLES_SERVICE.contains("After=ostree-finalize-staged.service"),
            "After= would order this before the rotation, not after it"
        );
        // Inside the hold unit's window, so /boot is still mounted when
        // the ExecStop pass writes to it.
        assert!(BOOT_TITLES_SERVICE.contains("After=ostree-finalize-staged-hold.service"));
        assert!(BOOT_TITLES_SERVICE.contains("RequiresMountsFor=/boot"));
        // The late shutdown slot. Without DefaultDependencies=no the
        // implicit Before=shutdown.target contradicts stopping after a
        // unit that stops at final.target, and systemd drops one of the
        // two rules silently.
        assert!(BOOT_TITLES_SERVICE.contains("DefaultDependencies=no"));
        assert!(BOOT_TITLES_SERVICE.contains("Conflicts=final.target"));
        // Both passes: the shutdown one is the point, the boot one
        // covers a machine that lost power before finishing a shutdown.
        assert!(BOOT_TITLES_SERVICE.contains("ExecStop=/usr/bin/kuma boot-titles"));
        assert!(BOOT_TITLES_SERVICE.contains("ExecStart=/usr/bin/kuma boot-titles"));
        assert!(BOOT_TITLES_SERVICE.contains("ConditionPathExists=/run/ostree-booted"));
    }

    /// The window for making /var/home a subvolume is one boot wide, and
    /// the guards are what keep it from being a way to lose a home
    /// directory. Run rather than only read: the branch that does the
    /// work needs btrfs, but every branch that refuses to can be
    /// exercised here, and those are the ones that matter.
    #[test]
    fn the_home_subvolume_converger_refuses_everything_it_should() {
        let out = generate(&config("schema_version = 1"));
        assert!(out.contains("COPY --chmod=755 kuma-home-subvol /usr/libexec/kuma-home-subvol"));
        assert!(out.contains("RUN systemctl enable kuma-home-subvol.service"));
        // Before anything can create a home directory in it, which is
        // what makes "only while empty" a check that ever passes.
        assert!(HOME_SUBVOL_SERVICE.contains("ConditionPathExists=/run/ostree-booted"));
        // The early slot, which is what closes the window rather than
        // enumerating who might fall into it. Every one of these lines is
        // load-bearing: without DefaultDependencies=no the unit cannot be
        // ordered before sysinit.target at all, without the tmpfiles
        // ordering /var/home may not exist yet, and without
        // RequiresMountsFor the target may not be on the filesystem this
        // is about to convert.
        for line in [
            "DefaultDependencies=no",
            "RequiresMountsFor=/var",
            "After=systemd-tmpfiles-setup.service",
            "Before=sysinit.target",
            "WantedBy=sysinit.target",
        ] {
            assert!(
                HOME_SUBVOL_SERVICE.contains(line),
                "{line} is what keeps a sandboxed unit from racing the converger"
            );
        }
        // Ordering against individual writers was the wrong shape and is
        // not to be reintroduced: the units that lose this race do not
        // write to /var/home, systemd binds it for them, and there are
        // twenty five of them on a desktop image.
        assert!(!HOME_SUBVOL_SERVICE.contains("Before=kuma-user-sync.service"));

        let dir = tempfile::tempdir().unwrap();
        context("schema_version = 1\n", dir.path());
        let script = dir.path().join("kuma-home-subvol");
        std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();

        let run = |arg: &std::path::Path| {
            std::process::Command::new("bash")
                .arg(&script)
                .arg(arg)
                .output()
                .expect("run the converger")
        };

        // A path that does not exist is not a fault: tmpfiles makes
        // /var/home at boot, and this can be ordered before it.
        let missing = dir.path().join("nothing-here");
        assert!(run(&missing).status.success());

        // A target that exists on a filesystem that is not btrfs. This is
        // the branch every test machine takes, and the one that must not
        // rmdir anything: the tempdir here is whatever the test host runs.
        let plain = dir.path().join("home");
        std::fs::create_dir(&plain).unwrap();
        std::fs::write(plain.join("someone-lives-here"), "notes\n").unwrap();
        let out = run(&plain);
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        assert!(plain.join("someone-lives-here").exists(), "it must never delete a home");
        assert!(plain.is_dir());
        // Declining has to say so. A silent exit 0 here is
        // indistinguishable from the first boot where this was supposed
        // to act and did not, which is exactly the case that went
        // unexplained: the unit succeeds either way and the only
        // difference is an inode nobody reads.
        assert!(
            String::from_utf8_lossy(&out.stdout).contains("kuma-home-subvol:"),
            "declining must log a reason, got: {:?}",
            String::from_utf8_lossy(&out.stdout)
        );

        // And the order of the guards: emptiness is checked before
        // anything is removed, on the same line of reasoning.
        let body = std::fs::read_to_string(&script).unwrap();
        let empty_at = body.find(r#"[ -n "$(ls -A "$target")" ]"#).unwrap();
        let rmdir_at = body.find(r#"rmdir "$target""#).unwrap();
        assert!(empty_at < rmdir_at);
    }

    #[test]
    fn greeter_check_guards_every_desktop() {
        for desktop in ["niri", "cosmic"] {
            let out = generate(&config(&format!(
                "schema_version = 1\n[system]\ndesktop = \"{desktop}\"\n"
            )));
            assert!(out.contains(
                "COPY --chmod=755 kuma-greeter-check /usr/lib/greenboot/check/required.d/50-kuma-greeter.sh"
            ));
        }
        // the check must poll the greeter unit, never graphical.target —
        // graphical waits for multi-user, which waits for this check
        assert!(GREETER_CHECK.contains("is-active display-manager.service"));
        assert!(!GREETER_CHECK.contains("is-active graphical.target"));
        let dir = tempfile::tempdir().unwrap();
        context("schema_version = 1\n[system]\ndesktop = \"cosmic\"\n", dir.path());
        let script = std::fs::read_to_string(dir.path().join("kuma-greeter-check")).unwrap();
        assert!(script.starts_with("#!/usr/bin/bash"));
    }

    /// `kuma-launch` is valid bash. Every script kuma embeds has shipped
    /// unchecked at least once, and CI only shellchecks `scripts/`.
    /// **No root-run script sources a credential file, and this is the
    /// grep rather than the fix.**
    ///
    /// The restore unit ran `. /var/lib/kuma/secrets/restore.env` as
    /// root on first boot, so `RESTIC_PASSWORD=$(curl ...|sh)` executed
    /// with everything. That file is not the operator's own private
    /// state either: concepts.md tells people to put it on the stick
    /// beside the ISO, so it is designed to travel between machines and
    /// hands.
    ///
    /// Measured rather than argued, because the difference is the whole
    /// finding: reading `A=$(id -u)` through a read-and-export loop
    /// yields the literal `$(id -u)`, and sourcing the same line yields
    /// `1000`. systemd's EnvironmentFile= does the first.
    #[test]
    fn no_baked_script_sources_a_credential_file() {
        for (name, script) in [
            ("kuma-restore", RESTORE_SCRIPT),
            ("kuma-backup", BACKUP_SCRIPT),
            ("kuma-snapshot", SNAPSHOT_SCRIPT),
            ("kuma-user-sync", USER_SYNC_SCRIPT),
        ] {
            for line in script.lines().map(str::trim) {
                let sources = line.starts_with(". ") || line.starts_with("source ");
                assert!(
                    !(sources && line.contains("secret")),
                    "{name} sources a credential file: {line}"
                );
            }
        }
    }

    /// The sleep guard fires, which is the half a guard usually fails.
    ///
    /// Both paths were run against a booted machine before this was
    /// written: with the shell running it exits 0 silently, and with the
    /// process name changed to one that does not exist it names the
    /// session and reaches the terminate. A guard that cannot fire is
    /// the failure this project has already shipped once (the
    /// mounted-image check in install.rs), so this one was proved
    /// firing rather than assumed.
    #[test]
    fn the_sleep_guard_parses_and_asks_the_right_questions() {
        let out = std::process::Command::new("bash")
            .args(["-n", "/dev/stdin"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                use std::io::Write;
                child.stdin.take().unwrap().write_all(SLEEP_GUARD.as_bytes())?;
                child.wait_with_output()
            })
            .expect("bash -n");
        assert!(out.status.success(), "sleep guard is not valid shell");
        // Only where kuma put a shell, and only for a real session.
        assert!(SLEEP_GUARD.contains("/etc/niri/config.kdl"), "{SLEEP_GUARD}");
        assert!(SLEEP_GUARD.contains("seat0"), "{SLEEP_GUARD}");
        // The property: no shell means the session ends rather than the
        // machine sleeping with the desktop on screen.
        assert!(SLEEP_GUARD.contains("pgrep -u \"$user\" -x noctalia"), "{SLEEP_GUARD}");
        assert!(SLEEP_GUARD.contains("loginctl terminate-session"), "{SLEEP_GUARD}");
        // And it runs on the way down, on every path into sleep.
        assert!(SLEEP_GUARD_SERVICE.contains("Before=sleep.target"), "{SLEEP_GUARD_SERVICE}");
        assert!(SLEEP_GUARD_SERVICE.contains("WantedBy=sleep.target"), "{SLEEP_GUARD_SERVICE}");
    }

    /// The unit carries what the spawn used to hand it.
    ///
    /// 0.17 moved the shell into kuma-shell.service and left
    /// NOCTALIA_CONFIG_HOME behind in niri's `environment` block, which
    /// a unit does not read. The machine booted, the shell ran, the
    /// service was active and every check was green, and the desktop
    /// was stock noctalia: a wider bar, no wallpaper-derived palette,
    /// and the welcome screen. Every variable the shell needs is now
    /// asserted in the unit, and asserted to say what the niri block
    /// says, because two places holding one value drift silently.
    #[test]
    fn the_shell_unit_carries_the_shells_environment() {
        for var in
            ["NOCTALIA_CONFIG_HOME=/usr/lib/kuma", "XCURSOR_THEME=Adwaita", "XCURSOR_SIZE=24"]
        {
            assert!(
                SHELL_SERVICE.contains(&format!("Environment={var}")),
                "the shell unit does not set {var}, so the session will not:\n{SHELL_SERVICE}"
            );
            // Same value on both sides of the seam. The niri block
            // states them as `NAME "value"`.
            let (name, value) = var.split_once('=').unwrap();
            assert!(
                NIRI_EXTRAS.contains(&format!("{name} \"{value}\"")),
                "{name} disagrees between the unit and niri's environment block"
            );
        }
    }

    #[test]
    fn kuma_launch_parses_as_shell() {
        use std::io::Write;
        let mut child = std::process::Command::new("bash")
            .args(["-n", "/dev/stdin"])
            .stdin(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(KUMA_LAUNCH.as_bytes()).unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    }

    /// The verb reaches the terminal as separate arguments.
    ///
    /// This is the whole reason the held script takes `"$@"` instead of a
    /// command pasted into a string: `kuma snapshot restore /home/a b`
    /// assembled by concatenation is a different command than the one
    /// asked for, and the failure is silent for every path without a
    /// space in it. A fake terminal that prints its own argv is the only
    /// way to see the boundaries.
    #[test]
    fn kuma_launch_passes_the_verb_as_arguments() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        // A fake kitty that prints each argument on its own line, so the
        // boundaries between them are visible in the output.
        std::fs::write(
            bin.join("kitty"),
            "#!/usr/bin/bash
for a in \"$@\"; do printf '%s\\n' \"$a\"; done
",
        )
        .unwrap();
        let launch = dir.path().join("kuma-launch");
        std::fs::write(&launch, KUMA_LAUNCH).unwrap();
        for path in [bin.join("kitty"), launch.clone()] {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let out = std::process::Command::new(&launch)
            .args(["snapshot", "restore", "/home/a b"])
            .env("PATH", bin.to_str().unwrap())
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let lines: Vec<&str> = std::str::from_utf8(&out.stdout).unwrap().lines().collect();
        assert_eq!(&lines[..3], ["-e", "/usr/bin/bash", "-lc"], "{lines:?}");
        // The held script is one argument spanning several printed lines;
        // $0 and the command follow it, and it is their boundaries this
        // test is about.
        assert_eq!(
            &lines[lines.len() - 5..],
            ["kuma-launch", "kuma", "snapshot", "restore", "/home/a b"],
            "{lines:?}"
        );
    }

    /// No terminal in the image is an error that says so, not a launcher
    /// entry that does nothing when clicked.
    #[test]
    fn kuma_launch_says_when_there_is_no_terminal() {
        let dir = tempfile::tempdir().unwrap();
        let empty = dir.path().join("bin");
        std::fs::create_dir(&empty).unwrap();
        let launch = dir.path().join("kuma-launch");
        std::fs::write(&launch, KUMA_LAUNCH).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&launch, std::fs::Permissions::from_mode(0o755)).unwrap();
        let out = std::process::Command::new(&launch)
            .arg("doctor")
            .env("PATH", empty.to_str().unwrap())
            .output()
            .unwrap();
        assert!(!out.status.success());
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("no terminal"),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Called with nothing, it says how to call it rather than opening a
    /// terminal that runs a bare `kuma`.
    #[test]
    fn kuma_launch_refuses_an_empty_call() {
        let dir = tempfile::tempdir().unwrap();
        let launch = dir.path().join("kuma-launch");
        std::fs::write(&launch, KUMA_LAUNCH).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&launch, std::fs::Permissions::from_mode(0o755)).unwrap();
        let out = std::process::Command::new(&launch).output().unwrap();
        assert_eq!(out.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&out.stderr).contains("usage:"));
    }

    /// The entries and the wrapper reach both desktops, which is the seam
    /// being tested rather than asserted: `kuma menu` was niri-only
    /// because fuzzel was, and a replacement that inherits that is not a
    /// replacement.
    #[test]
    fn the_seam_is_on_every_desktop() {
        for desktop in ["niri", "cosmic"] {
            let out = generate(&config(&format!(
                "schema_version = 1\n[system]\ndesktop = \"{desktop}\""
            )));
            assert!(
                out.contains("COPY --chmod=755 kuma-launch /usr/libexec/kuma-launch"),
                "{desktop} has no wrapper"
            );
            for entry in seam::ENTRIES {
                assert!(
                    out.contains(&format!("COPY {}.desktop {}", entry.id, seam::path(entry))),
                    "{desktop} is missing {}",
                    entry.id
                );
            }
            assert!(out.contains("RUN desktop-file-validate "), "{desktop} does not validate");
        }
    }

    /// And not on a machine with no desktop, which has no launcher to put
    /// them in.
    #[test]
    fn the_seam_is_absent_without_a_desktop() {
        let out = generate(&config("schema_version = 1"));
        assert!(!out.contains("kuma-launch"), "a headless image has no launcher");
    }

    /// A greeter that starts, dies, and is restarted is not a greeter.
    ///
    /// This was found on a real installed machine: greenboot reached its
    /// success target at 15.8s while greetd was on restart 3 of 5, and
    /// gave up at 19.5s. Nobody could log in and boot health said green,
    /// which is the one outcome this check exists to prevent.
    #[test]
    fn the_greeter_check_fails_a_crash_looping_greeter() {
        // Sampled twice with a gap, so one lucky retry cannot pass it.
        assert_eq!(GREETER_CHECK.matches("is-active display-manager.service").count(), 2);
        assert!(GREETER_CHECK.contains("sleep \"$settle\""));
        // And a unit that has exhausted its restarts fails now rather
        // than after two minutes of polling something already dead.
        assert!(GREETER_CHECK.contains("is-failed display-manager.service"));

        // Run it against stub systemctls, since a check that has never
        // executed is a check nobody has tested.
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let script = dir.path().join("check");
        std::fs::write(
            &script,
            GREETER_CHECK.replace("SECONDS + 120", "SECONDS + 4").replace("settle=5", "settle=1"),
        )
        .unwrap();
        for (name, body, want) in [
            ("healthy", "case \"$1\" in is-failed) exit 3 ;; *) exit 0 ;; esac", true),
            // is-failed answers yes: the unit gave up.
            ("dead", "case \"$1\" in is-failed) exit 0 ;; *) exit 3 ;; esac", false),
            // Never active, never failed: the deadline decides.
            ("absent", "exit 3", false),
        ] {
            std::fs::write(bin.join("systemctl"), format!("#!/usr/bin/bash\nshift\n{body}\n"))
                .unwrap();
            let mut perms = std::fs::metadata(bin.join("systemctl")).unwrap().permissions();
            std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
            std::fs::set_permissions(bin.join("systemctl"), perms).unwrap();
            let out = std::process::Command::new("bash")
                .arg(&script)
                .env("PATH", format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()))
                .output()
                .expect("bash");
            assert_eq!(out.status.success(), want, "{name}: {:?}", out);
        }
    }

    #[test]
    fn timezone_links_localtime() {
        let out =
            generate(&config("schema_version = 1\n[system]\ntimezone = \"America/Denver\"\n"));
        assert!(out.contains(
            "test -e /usr/share/zoneinfo/America/Denver && ln -sfn /usr/share/zoneinfo/America/Denver /etc/localtime"
        ));
        // unset means UTC: no localtime layer at all
        let out = generate(&config("schema_version = 1"));
        assert!(!out.contains("/etc/localtime"));
    }

    #[test]
    fn stock_waybar_spawn_is_deduped() {
        let out = generate(&config("schema_version = 1\n[system]\ndesktop = \"niri\"\n"));
        // Fedora's default config spawns waybar and kuma no longer ships
        // one, so the sed matters more than it used to rather than less:
        // without it the session starts a bar that is not in the image
        // and the shell's own bar comes up beside a black rectangle.
        assert!(out.contains("-e '/^spawn-at-startup \"waybar\"$/d'"));
        assert_eq!(NIRI_EXTRAS.matches("spawn-at-startup \"waybar\"").count(), 0);
        assert!(!NIRI_PACKAGES.contains(&"waybar"), "nothing left to spawn");
    }

    /// The local-override include is worthless anywhere but last: niri
    /// merges includes positionally, so anything appended after it would
    /// silently outrank the machine's own settings. The extras are catted
    /// onto the end of the config, so last-in-extras is last-in-config.
    #[test]
    fn the_local_override_include_has_the_last_word() {
        let include = "include optional=true \"~/.config/niri/local.kdl\"";
        assert_eq!(
            NIRI_EXTRAS.trim_end().lines().next_back(),
            Some(include),
            "the local override must be the final line of the niri config"
        );
        // required-and-absent would fail `niri validate` in the build, and
        // every machine that never writes the file
        assert!(!NIRI_EXTRAS.contains("include \"~/.config/niri/local.kdl\""));
        // the build appends extras after the merged upstream config, which
        // is the only reason last-in-extras means last-in-config
        let out = generate(&config("schema_version = 1\n[system]\ndesktop = \"niri\"\n"));
        assert!(out.contains("cat /usr/lib/kuma/niri-extras.kdl >> /etc/niri/config.kdl"));
    }

    #[test]
    fn context_includes_theme_files_for_niri() {
        let dir = tempfile::tempdir().unwrap();
        context("schema_version = 1\n[system]\ndesktop = \"niri\"\n", dir.path());
        let wallpaper = std::fs::read(dir.path().join("kuma-wallpaper.jpg")).unwrap();
        assert!(!wallpaper.is_empty());
        let extras = std::fs::read_to_string(dir.path().join("niri-extras.kdl")).unwrap();
        // The wallpaper is still the image's, but the shell draws it from
        // its own config rather than a swaybg argument in here.
        assert!(KUMA_NOCTALIA.contains("/usr/share/backgrounds/kuma"));
        // The shell is a supervised unit now, not a spawn: a niri
        // spawn lands in a transient scope, and a scope cannot restart.
        assert!(!extras.contains("spawn-at-startup \"noctalia\""), "{extras}");
        assert!(dir.path().join("kuma-shell.service").exists());
        assert!(SHELL_SERVICE.contains("Restart=always"), "{SHELL_SERVICE}");
        assert!(extras.contains("kuma-clipboard-bridge"));
        assert!(dir.path().join("kuma-clipboard-bridge").exists());
        let greetd = std::fs::read_to_string(dir.path().join("greetd-config.toml")).unwrap();
        assert!(greetd.contains("Welcome to Kuma"));
        assert!(dir.path().join("noctalia-config.toml").exists());
        assert!(dir.path().join("kitty.conf").exists());
        let ff = std::fs::read_to_string(dir.path().join("fastfetch-config.jsonc")).unwrap();
        assert!(ff.contains("/usr/lib/kuma/fastfetch-logo.txt"));
        assert!(dir.path().join("fastfetch-logo.txt").exists());
    }

    #[test]
    fn branding_names_the_release() {
        let out = generate(&config("schema_version = 1\n"));
        // One bear per Fedora base, and an unlisted base must still name
        // the kuma version rather than degrading to a bare "Kuma".
        assert!(out.contains(r#"44) CODENAME="Beorn""#));
        assert!(out.contains(r#"45) CODENAME="Callisto""#));
        assert!(out.contains(r#"*) CODENAME="""#));

        // The number is kuma's own and comes from the building binary, so
        // an image says which kuma made it. The placeholder must be gone:
        // shipping "@KUMAVERSION@" as a machine's PRETTY_NAME is the whole
        // failure this asserts against.
        let version = env!("CARGO_PKG_VERSION");
        assert!(
            !out.contains("@KUMAVERSION@"),
            "the version placeholder reached the Containerfile"
        );
        assert!(
            out.contains(&format!(r#"PRETTY_NAME=\"Kuma {version}${{CODENAME:+ ($CODENAME)}}\""#))
        );
        // VERSION is rewritten unconditionally, not only when a bear
        // matched: left alone it keeps Fedora's own string, which on a
        // branched base reads "45 (Rawhide Prerelease)" inside an OS that
        // otherwise calls itself Kuma.
        assert!(out.contains(&format!(r#"VERSION=\"{version}${{CODENAME:+ ($CODENAME)}}\""#)));

        // VERSION_ID stays Fedora's: toolbox, distrobox and COPR resolve
        // against it, so kuma's version must never be written there.
        assert!(!out.contains(&format!("VERSION_ID={version}")));
        // fedora-release is a compatibility file and keeps Fedora's number.
        assert!(out.contains(r#"echo "Kuma release ${VERSION_ID}${CODENAME:+ ($CODENAME)}""#));
    }

    #[test]
    fn no_desktop_means_no_desktop_layer() {
        let out = generate(&config("schema_version = 1"));
        assert!(!out.contains("greetd"));
        assert!(!out.contains("graphical.target"));
    }

    /// The policy is the one thing here that fails closed in production
    /// and nowhere else: get it wrong and every `bootc upgrade` from the
    /// published image is refused, on machines belonging to people who
    /// did not build it. So the shipped bytes are pinned rather than
    /// described.
    ///
    /// `matchRepository` is the field that was wrong first and is the
    /// reason this test is exact. cosign records the signed identity as
    /// the bare repository with no tag, so `matchRepoDigestOrExact` —
    /// the obvious choice, and the one initially written — rejects every
    /// image kuma has ever published. Verified against the live registry
    /// with the real key (accepted) and a wrong key (refused).
    #[test]
    fn the_signature_policy_is_the_one_that_was_verified() {
        let policy = signature_policy();
        let parsed: serde_json::Value =
            serde_json::from_str(&policy).expect("policy.json must be valid JSON");

        let repo = "ghcr.io/letdown2491/kuma";
        let rule = &parsed["transports"]["docker"][repo][0];
        assert_eq!(rule["type"], "sigstoreSigned");
        assert_eq!(rule["keyPath"], COSIGN_PUB_PATH);
        assert_eq!(
            rule["signedIdentity"]["type"], "matchRepository",
            "cosign signs the bare repository; any stricter identity rejects every published image"
        );

        // Everything else stays permissive on purpose. This file is
        // shared by podman and bootc, so a blanket requirement refuses
        // Fedora's base on the next update and the machine's own local
        // build on the next switch.
        assert_eq!(parsed["default"][0]["type"], "insecureAcceptAnything");
        for transport in ["containers-storage", "docker-daemon", "dir", "oci"] {
            assert_eq!(
                parsed["transports"][transport][""][0]["type"], "insecureAcceptAnything",
                "{transport} must stay permissive or local images stop working"
            );
        }
        assert!(
            parsed["transports"]["docker"].get("").is_none(),
            "a catch-all docker rule would require signatures from every registry"
        );

        // The other half of the pair: without this the policy has nothing
        // to look at, because cosign stores signatures as a separate tag.
        let rd = registries_d();
        assert!(rd.contains("use-sigstore-attachments: true"));
        assert!(rd.contains(repo));
    }

    /// The key has to be in the binary, because a build runs from the
    /// binary and not from a checkout.
    #[test]
    fn the_signing_key_ships_in_every_image() {
        assert!(COSIGN_PUB.contains("BEGIN PUBLIC KEY"));
        let out = generate(&config("schema_version = 1"));
        assert!(out.contains(&format!("COPY cosign.pub {COSIGN_PUB_PATH}")));
        assert!(out.contains("COPY containers-policy.json /etc/containers/policy.json"));
        assert!(out.contains("registries.d/kuma-sigstore.yaml"));

        let dir = tempfile::tempdir().unwrap();
        context("schema_version = 1\n", dir.path());
        let shipped = std::fs::read_to_string(dir.path().join("cosign.pub")).unwrap();
        assert_eq!(shipped, COSIGN_PUB, "the image must carry the key the policy names");
        let policy = std::fs::read_to_string(dir.path().join("containers-policy.json")).unwrap();
        assert_eq!(policy, signature_policy());
    }

    /// SECURITY.md tells the reader that declaring no desktop reaches no
    /// package source beyond Fedora's. Choosing a desktop is what pulls in
    /// RPM Fusion, so "which declarations touch a third-party repo" is a
    /// promise the trust-boundary section makes and this is what keeps it
    /// true. A regression here would be silent: the image still builds and
    /// still works, it just quietly trusts one more party than the
    /// document says it does.
    #[test]
    fn only_a_desktop_reaches_a_third_party_repo() {
        let minimal = generate(&config("schema_version = 1"));
        assert!(!minimal.contains("rpmfusion"), "a desktopless image reached RPM Fusion");
        assert!(!minimal.contains("freeworld"));
        for desktop in ["niri", "cosmic"] {
            let out = generate(&config(&format!(
                "schema_version = 1\n[system]\ndesktop = \"{desktop}\"\n"
            )));
            assert!(
                out.contains("rpmfusion-free-release"),
                "{desktop} lost the freeworld codecs; SECURITY.md still names RPM Fusion"
            );
        }
    }

    #[test]
    fn desktop_layer_precedes_user_packages() {
        let out = generate(&config(
            "schema_version = 1\n[system]\ndesktop = \"niri\"\n[packages]\nrpm = [\"htop\"]\n",
        ));
        let desktop_at = out.find("niri").unwrap();
        let user_at = out.find("htop").unwrap();
        assert!(desktop_at < user_at);
    }

    #[test]
    fn flatpaks_generate_remote_list_and_sync_service() {
        let out = generate(&config(
            "schema_version = 1\n[packages]\nflatpak = [\"org.mozilla.firefox\"]\n",
        ));
        assert!(out.contains(&dnf_install("flatpak")));
        assert!(out.contains("/etc/flatpak/remotes.d/flathub.flatpakrepo"));
        assert!(out.contains("COPY flatpaks /usr/lib/kuma/flatpaks"));
        assert!(out.contains("systemctl enable kuma-flatpak-sync.service"));
    }

    #[test]
    fn niri_ships_sync_even_without_declared_apps() {
        let out = generate(&config("schema_version = 1\n[system]\ndesktop = \"niri\"\n"));
        assert!(out.contains("flathub.flatpakrepo"));
        // flatpak comes from the desktop set; no second install layer
        assert!(!out.contains(&dnf_install("flatpak")));
        // convergence: the empty declaration still syncs, removing strays
        assert!(out.contains("systemctl enable kuma-flatpak-sync.service"));
    }

    /// Convergence removes what the declaration installed and dropped,
    /// and nothing else. The removal loop must read the state file: the
    /// moment it reads `flatpak list` instead, every app a store put
    /// here system-wide is swept, which is the bug that kept kuma from
    /// being able to ship a store at all.
    /// Off unless declared, and when declared it bakes its own tools:
    /// a snapshot timer that dies on a missing btrfs binary would be a
    /// backup that silently isn't one.
    #[test]
    fn snapshots_are_opt_in_and_bring_their_own_tools() {
        let out = generate(&config("schema_version = 1\n"));
        assert!(!out.contains("kuma-snapshot"));
        assert!(!out.contains(&dnf_install("btrfs-progs")));

        let out = generate(&config("schema_version = 1\n[snapshots]\nenable = true\n"));
        assert!(out.contains(&dnf_install("btrfs-progs")));
        assert!(out.contains("RUN systemctl enable kuma-snapshot.timer"));
    }

    /// The retention and the target are the declaration's, so they have
    /// to reach the machine; and a target that isn't btrfs has to end the
    /// run cleanly rather than fail a unit on every tick, because one
    /// declaration describes machines laid out differently.
    #[test]
    fn snapshot_script_carries_the_declared_policy() {
        let declared = config(
            "schema_version = 1\n[snapshots]\nenable = true\ntarget = \"/var/data\"\nkeep_recent = 3\nkeep_daily = 90\n",
        );
        let script = snapshot_script(&declared);
        assert!(script.contains("target='/var/data'"));
        assert!(script.contains("keep_recent=3"));
        assert!(script.contains("keep_daily=90"));
        for placeholder in ["{target}", "{keep_recent}", "{keep_daily}"] {
            assert!(!script.contains(placeholder), "{placeholder} was never substituted");
        }
        assert!(script.contains("= btrfs ] || exit 0"), "a non-btrfs target exits clean");
        assert!(script.contains("btrfs subvolume show"), "a non-subvolume exits clean");

        // -T, and the same question doctor asks. Without it findmnt wants
        // a mount point, and a btrfs subvolume need not be one: a
        // /var/home nested inside the deployment's /var answered nothing,
        // so the script exited 0 having taken no snapshot, on every
        // machine kuma installs. The script said the target was not
        // btrfs; doctor, asking with -T, said it was.
        assert!(
            script.contains(r#"findmnt -no FSTYPE -T "$target""#),
            "the filesystem question has to be about the path, not about a mount point"
        );

        let dir = tempfile::tempdir().unwrap();
        context(
            "schema_version = 1\n[snapshots]\nenable = true\ninterval = \"daily\"\n",
            dir.path(),
        );
        let timer = std::fs::read_to_string(dir.path().join("kuma-snapshot.timer")).unwrap();
        assert!(timer.contains("OnCalendar=daily"));
        assert!(timer.contains("Persistent=true"), "a laptop asleep at the hour still snapshots");
    }

    /// The declaration's policy has to survive into the script, and the
    /// script has to be shell. The second half is not pedantry: every
    /// substitution here lands inside a `--exclude` argument list built
    /// by string concatenation, and a stray continuation would eat the
    /// command on the next line, which bash reports at run time on a
    /// machine nobody is watching.
    #[test]
    fn backup_script_carries_the_declared_policy() {
        let declared = config(
            "schema_version = 1\n[snapshots]\nenable = true\ntarget = \"/var/home\"\n\
             [backup]\nenable = true\nrepo = \"s3:https://minio.example:9000/kuma\"\n\
             keep_daily = 3\nkeep_weekly = 2\nkeep_monthly = 1\n\
             exclude = [\"~/Videos\", \"/var/home/shared/scratch\"]\n",
        );
        let script = backup_script(&declared);

        assert!(script.contains("RESTIC_REPOSITORY='s3:https://minio.example:9000/kuma'"));
        assert!(script.contains("--keep-daily 3 --keep-weekly 2 --keep-monthly 1"));
        // Forget is nearly free and prune repacks; doing both nightly
        // moves gigabytes over somebody's uplink to reclaim what one
        // expired snapshot left behind.
        assert!(
            !script.contains("forget --tag kuma --prune"),
            "pruning belongs on its own schedule: {script}"
        );
        assert!(script.contains("restic prune"), "it still prunes, just not every run");
        assert!(script.contains("604800"), "weekly, measured from a stamp rather than a weekday");
        // Both stamps live in /var/lib/kuma, so it has to exist before
        // either is written rather than before only the second.
        let made = script.find("install -d -m 0755 /var/lib/kuma").unwrap();
        for writes in ["$pruned", "$stamp"] {
            let at = script.rfind(&format!("> \"{writes}\"")).unwrap();
            assert!(made < at, "{writes} is written before its directory exists");
        }
        for placeholder in
            ["{target}", "{repo}", "{excludes}", "{keep_daily}", "{keep_weekly}", "{keep_monthly}"]
        {
            assert!(!script.contains(placeholder), "{placeholder} was never substituted");
        }

        // Curated first and always. These are trees this same file
        // rebuilds, so keeping them offsite is paying to store what kuma
        // can recreate.
        for curated in ["$target/linuxbrew", "$target/*/.cache"] {
            assert!(script.contains(&format!("--exclude \"{curated}\"")), "missing {curated}");
        }
        // A declared `~/` means every home, which is the only reading
        // that works when the target holds more than one.
        assert!(script.contains("--exclude \"$target/*/Videos\""), "{script}");
        assert!(script.contains("--exclude \"/var/home/shared/scratch\""));
        // The snapshot store is inside the target, and after the bind
        // mount it is the snapshot's own copy of it.
        assert!(script.contains("--exclude \"$target/.snapshots\""));

        // The bind is the whole reason this is incremental: restic 0.19
        // has no --set-path and groups by host+paths, so a source path
        // named for the minute it was taken never matches a parent.
        assert!(script.contains(r#"mount --bind "$store/$newest" "$target""#), "{script}");
        // Run by hand rather than by the unit, an untrapped bind leaves
        // the live /var/home replaced by a read-only snapshot until the
        // machine reboots.
        assert!(script.contains(r##"trap 'umount "$target""##), "the bind is undone: {script}");

        // Three states that are "not ready yet" rather than "broken",
        // each exiting clean so one declaration can describe machines at
        // different stages of being set up.
        assert!(script.contains("no credential loaded"));
        assert!(script.contains("no snapshot in $store yet"));
        assert!(script.contains("seed it once with 'kuma backup --init'"));
        // Bounded, and it has to be. restic retries a missing bucket
        // with backoff, so an unbounded probe takes minutes to answer
        // "no" on every machine nobody has seeded, every night.
        assert!(script.contains("timeout 30 restic cat config"), "{script}");
        // A unit has no HOME, and restic treats an unopenable cache as
        // fatal, so without this every run dies before it reaches the
        // repository with an error that reads like a network failure.
        assert!(script.contains("export RESTIC_CACHE_DIR=/var/cache/restic"), "{script}");
        assert!(script.contains("cannot reach"), "unreachable reads differently from absent");

        // The stamp is what doctor grades, so it must only be written by
        // a run that copied something. Every early exit is above it.
        let stamp_at = script.find("date -u").expect("the run stamps its success");
        for guard in ["no credential loaded", "no snapshot in $store yet", "no repository at"] {
            assert!(script.find(guard).unwrap() < stamp_at, "{guard} must precede the stamp");
        }

        let out = std::process::Command::new("bash")
            .args(["-n", "/dev/stdin"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                use std::io::Write;
                child.stdin.take().unwrap().write_all(script.as_bytes())?;
                child.wait_with_output()
            })
            .expect("bash");
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    }

    /// The guards are only worth having if they are reachable, and the
    /// first draft's were not: `newest=$(... | grep ...)` under
    /// `set -o pipefail` exits non-zero when there is no snapshot, so
    /// `set -e` killed the script one line above the message written for
    /// that machine. Reading the rendered script found it; nothing else
    /// would have until a real machine sat there failing quietly.
    ///
    /// So this runs the script rather than reading it, on a target with
    /// no snapshots in it, and asserts it exits clean and says why.
    #[test]
    fn a_machine_with_nothing_to_copy_yet_exits_clean() {
        let home = tempfile::tempdir().unwrap();
        let target = home.path().display();
        let declared = config(&format!(
            "schema_version = 1\n[snapshots]\nenable = true\ntarget = \"{target}\"\n\
             [backup]\nenable = true\nrepo = \"b2:kuma\"\n"
        ));
        let script = backup_script(&declared);

        let run = |env: &[(&str, &str)]| {
            let mut cmd = std::process::Command::new("bash");
            cmd.args(["-s"])
                .env_clear()
                .env("PATH", "/usr/bin:/bin")
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped());
            for (k, v) in env {
                cmd.env(k, v);
            }
            cmd.spawn()
                .and_then(|mut child| {
                    use std::io::Write;
                    child.stdin.take().unwrap().write_all(script.as_bytes())?;
                    child.wait_with_output()
                })
                .expect("bash")
        };

        // No credential: the machine is un-provisioned, not broken.
        let out = run(&[]);
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        assert!(
            String::from_utf8_lossy(&out.stdout).contains("no credential loaded"),
            "{}",
            String::from_utf8_lossy(&out.stdout)
        );

        // Credential present, no snapshot taken yet. This is the one the
        // pipefail bug made unreachable.
        let out = run(&[("RESTIC_PASSWORD", "x")]);
        assert!(
            out.status.success(),
            "an empty snapshot store must not fail the unit: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            String::from_utf8_lossy(&out.stdout).contains("no snapshot in"),
            "{}",
            String::from_utf8_lossy(&out.stdout)
        );

        // Nothing may have been stamped: freshness must reflect a run
        // that actually copied something.
        assert!(!std::path::Path::new("/var/lib/kuma/backup-last-test").exists());
    }

    /// The unit has to name the secret the declaration named, tolerate
    /// it being absent, and get its own mount namespace. Without the
    /// last one the bind above would replace the running system's home
    /// with a read-only snapshot for the length of the backup.
    #[test]
    fn the_backup_unit_reads_the_named_secret_and_mounts_privately() {
        let dir = tempfile::tempdir().unwrap();
        context(
            "schema_version = 1\n[snapshots]\nenable = true\n\
             [backup]\nenable = true\nrepo = \"b2:kuma\"\nsecret = \"start9\"\n\
             interval = \"03:00\"\n",
            dir.path(),
        );
        let service = std::fs::read_to_string(dir.path().join("kuma-backup.service")).unwrap();
        assert!(
            service.contains("EnvironmentFile=-/var/lib/kuma/secrets/start9.env"),
            "the leading - is what lets an un-provisioned machine boot: {service}"
        );
        assert!(service.contains("PrivateMounts=yes"), "{service}");
        assert!(service.contains("After=kuma-snapshot.service"), "copy after taking: {service}");
        // A timer firing on resume finds the network still coming up,
        // and network-online.target only orders boot.
        assert!(service.contains("Restart=on-failure"));
        assert!(service.contains("StartLimitBurst=6"), "retry, but not forever: {service}");

        let timer = std::fs::read_to_string(dir.path().join("kuma-backup.timer")).unwrap();
        assert!(timer.contains("OnCalendar=03:00"));
        assert!(timer.contains("Persistent=true"), "a laptop asleep at the hour still backs up");
    }

    /// The one unrecoverable thing outside home rides along only when
    /// asked, and the default is that it does not. Doctor is what makes
    /// that default honest rather than a trap, so this only asserts the
    /// converger obeys the knob.
    #[test]
    fn network_connections_ride_along_only_when_asked() {
        let head = "schema_version = 1\n[snapshots]\nenable = true\n\
                    [backup]\nenable = true\nrepo = \"b2:kuma\"\n";
        let off = backup_script(&config(head));
        assert!(
            !off.contains(NETWORK_CONNECTIONS),
            "off by default, because those are passphrases in the clear"
        );

        let on = backup_script(&config(&format!("{head}network_connections = true\n")));
        assert!(
            on.contains(&format!(r#"restic backup "$target" {NETWORK_CONNECTIONS} \"#)),
            "a second source path, not an exclude: {on}"
        );
    }

    /// The scripts are shell and the verb is Rust, so the paths they
    /// share cannot be one constant. They can still be one answer, and
    /// this is what makes them one: doctor grading a stamp the converger
    /// does not write, or a unit loading a credential `kuma backup` does
    /// not name, are both silent failures that look like a healthy
    /// A declaration with every optional feature turned on.
    ///
    /// Most of them default to off, so a fixture that leaves them there
    /// exercises none of the branches that copy files into the image or
    /// stage them into the build context. Two tests were checking those
    /// branches against declarations that never entered them.
    const EVERYTHING_ON: &str = "schema_version = 1\n\
         [system]\ndesktop = \"niri\"\nbrew = true\nhostname = \"probe\"\n\
         timezone = \"Pacific/Auckland\"\n\
         [packages]\nflatpak = [\"org.mozilla.firefox\"]\nbrew = [\"ripgrep\"]\n\
         [services]\nenable = [\"sshd.service\"]\n\
         [snapshots]\nenable = true\n\
         [backup]\nenable = true\nrepo = \"b2:kuma\"\nnetwork_connections = true\n\
         [overrides.\"org.mozilla.firefox\"]\nsockets = [\"wayland\"]\n\
         [user]\nname = \"probe\"\nssh_keys = [\"ssh-ed25519 AAAAC3Nz probe@example\"]\n";

    /// machine. This release has already produced that shape three
    /// times.
    #[test]
    fn the_shell_and_the_verb_agree_on_where_things_live() {
        // The baked lists: written by a COPY here, read as a literal by
        // the generated shell, and read again by four Rust callers that
        // all treat absence as "nothing to do". A drift makes a machine
        // that looks converged and is not.
        use crate::state::{BAKED_BREWS, BAKED_FLATPAKS, BAKED_OVERRIDES, FLATPAK_STATE};
        let niri = generate(&config(EVERYTHING_ON));
        for (path, script) in
            [(BAKED_FLATPAKS, FLATPAK_SYNC_SCRIPT), (BAKED_BREWS, BREW_SYNC_SCRIPT)]
        {
            assert!(
                script.contains(&format!("declared={path}")),
                "the converger reads a different path than the one Rust decides from: {path}"
            );
            assert!(niri.contains(&format!(" {path}\n")), "nothing copies {path} into the image");
        }
        assert!(
            FLATPAK_SYNC_SCRIPT.contains(&format!("state={FLATPAK_STATE}")),
            "the converger tracks authority somewhere doctor does not read"
        );
        assert!(niri.contains(BAKED_OVERRIDES), "nothing copies {BAKED_OVERRIDES} into the image");

        // The session constants exist so a session command cannot change
        // in one of two places, and their own doc comment says exactly
        // that, while the greeter config beside them spelled the command
        // out again. A const cannot interpolate into a const, so the
        // agreement is asserted instead of deduplicated.
        assert!(
            GREETD_CONFIG.contains(NIRI_SESSION),
            "the greeter starts a session the constants do not name"
        );
        assert!(
            niri.contains(GREETD_CONF),
            "the greeter config is copied somewhere greetd does not read"
        );

        assert!(
            BACKUP_SCRIPT.contains(&format!("stamp={}", crate::backup::STAMP)),
            "the converger stamps somewhere doctor does not read"
        );
        let dir = crate::backup::SECRETS_DIR;
        assert!(
            RESTORE_SCRIPT.contains(&format!("secret={dir}/restore.env")),
            "the restore unit reads a credential the install does not write"
        );
        let service = backup_service(&config(
            "schema_version = 1\n[snapshots]\nenable = true\n\
             [backup]\nenable = true\nrepo = \"b2:kuma\"\nsecret = \"named\"\n",
        ));
        assert!(
            service.contains(&format!("EnvironmentFile=-{dir}/named.env")),
            "the unit loads a credential from somewhere else entirely: {service}"
        );
    }

    /// Root extracting a tarball into a tree uid 1000 owns is the one
    /// place kuma does that, and the guard deciding whether it runs
    /// again lives inside the same tree.
    #[test]
    fn brew_setup_refuses_a_prefix_it_does_not_own() {
        // The check must come before anything that writes, or it is
        // decoration: mkdir -p follows a symlink and tar chdirs through
        // it, so both have to be downstream of the refusal.
        // Line starts, not substrings: the comment above the guard
        // describes the attack and names these same commands, and the
        // first version of this test matched the prose.
        let guard = BREW_SETUP_SCRIPT.find("refusing to write into it").unwrap();
        for writes in ["\nmkdir -p", "\n    | tar -xz", "\nln -sf"] {
            let at = BREW_SETUP_SCRIPT
                .find(writes)
                .unwrap_or_else(|| panic!("the setup no longer runs {writes:?}"));
            assert!(at > guard, "{writes:?} happens before the ownership check that protects it");
        }
        assert!(
            BREW_SETUP_SCRIPT.contains("[ -L \"$dir\" ]"),
            "a symlink in place of the prefix is the move this guards against"
        );

        let out = std::process::Command::new("bash")
            .args(["-n", "/dev/stdin"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                use std::io::Write;
                child.stdin.take().unwrap().write_all(BREW_SETUP_SCRIPT.as_bytes())?;
                child.wait_with_output()
            })
            .expect("bash");
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    }

    /// Every kuma unit an image enables has to be accounted for on live
    /// media: masked, or carrying a condition that makes it skip itself
    /// there.
    ///
    /// The mask list in liveiso.rs is hand-maintained and its written
    /// justification had already gone stale: it credits
    /// kuma-boot-health-sync with `ConditionPathExists=/run/ostree-booted`,
    /// which that unit does not carry. It self-skips today by accident,
    /// because the file it looks for is not there. A unit that starts
    /// converging inside a live session is converging something nobody
    /// installed, and the media is a newcomer's first impression.
    #[test]
    fn every_enabled_kuma_unit_is_accounted_for_on_live_media() {
        let dir = tempfile::tempdir().unwrap();
        context(EVERYTHING_ON, dir.path());
        let containerfile = std::fs::read_to_string(dir.path().join("Containerfile")).unwrap();

        let enabled: Vec<String> = containerfile
            .lines()
            .filter_map(|l| l.trim().strip_prefix("RUN systemctl enable "))
            .flat_map(|rest| rest.split_whitespace())
            .filter(|u| u.starts_with("kuma-"))
            .map(String::from)
            .collect();
        assert!(enabled.len() > 5, "expected the image to enable kuma's units: {enabled:?}");

        // Deliberately live-safe: enabled, unmasked, and correct there.
        // Named with the reason rather than left to be rediscovered,
        // the same way the walkthrough table records what nothing runs.
        const LIVE_SAFE: &[(&str, &str)] = &[(
            "kuma-vm-timezone.service",
            "adopts the host's timezone through qemu fw_cfg, which a live session inside \
             `kuma vm` wants, and exits 0 immediately when that channel is absent",
        )];

        for unit in &enabled {
            if crate::liveiso::LIVE_MASKED.contains(&unit.as_str()) {
                continue;
            }
            if let Some((_, why)) = LIVE_SAFE.iter().find(|(u, _)| u == unit) {
                assert!(!why.trim().is_empty(), "{unit} is called live-safe for no stated reason");
                continue;
            }
            // Not masked, so it must decline on its own. Either it is
            // gated on an ostree boot, or on a file a live session does
            // not have.
            let staged = dir.path().join(unit);
            let text = std::fs::read_to_string(&staged).unwrap_or_default();
            assert!(
                text.contains("ConditionPathExists="),
                "{unit} is enabled, is not masked on live media, and carries no condition \
                 that would make it skip there"
            );
        }
    }

    /// The unit that puts a home directory back on a machine that has
    /// just been installed, and the one ordering constraint that makes
    /// it possible at all.
    #[test]
    fn the_restore_runs_after_the_subvolume_exists() {
        // /var/home is not a subvolume until kuma-home-subvol has run,
        // and a restore that beat it would fill an ordinary directory,
        // which is the exact state that unit steps back from. The
        // machine would come up with no subvolume, no snapshots, and
        // nothing saying why.
        assert!(
            RESTORE_SERVICE.contains("After=kuma-home-subvol.service kuma-user-sync.service"),
            "{RESTORE_SERVICE}"
        );
        // Gated on the request rather than on being enabled, because
        // every boot after the first has to skip it without looking
        // like a unit that failed.
        assert!(RESTORE_SERVICE.contains("ConditionPathExists=/var/lib/kuma/restore-request"));
        // A home directory over a slow link outlasts any default.
        assert!(RESTORE_SERVICE.contains("TimeoutStartSec=infinity"));
        // The credential is PARSED by systemd, never executed by a
        // shell. See the sibling test below for what sourcing it did.
        assert!(
            RESTORE_SERVICE.contains("EnvironmentFile=-/var/lib/kuma/secrets/restore.env"),
            "{RESTORE_SERVICE}"
        );

        // Whatever the converger stores, this has to bring back. The
        // network connections were the entire reason for a knob, and
        // restoring only home would lose them silently: the machine
        // comes up looking complete with the one unrecreatable thing
        // missing.
        assert!(
            RESTORE_SCRIPT.contains("--include /var/home")
                && RESTORE_SCRIPT.contains(&format!("--include {NETWORK_CONNECTIONS}")),
            "the restore must cover both paths the backup stores: {RESTORE_SCRIPT}"
        );
        assert!(
            RESTORE_SCRIPT.contains("--tag kuma"),
            "a shared repository must not hand this machine a snapshot kuma never made"
        );

        // The request outlives a failed restore and is cleared only by
        // one that worked. There is no Restart= here, so surviving buys
        // one attempt per boot rather than a loop; clearing first would
        // mean a repository unreachable for one minute at first boot
        // discards the restore for good and the machine comes up empty.
        let restored = RESTORE_SCRIPT.find("restic restore").unwrap();
        let cleared = RESTORE_SCRIPT.rfind(r#"rm -f "$request""#).unwrap();
        assert!(restored < cleared, "a bad day must cost a retry, not the data");
        // Except the one request nothing can ever satisfy.
        let no_secret = RESTORE_SCRIPT.find("is not there").unwrap();
        assert!(
            RESTORE_SCRIPT[..no_secret].matches("restic restore").count() == 0,
            "a missing credential is cleared without attempting anything"
        );

        let out = std::process::Command::new("bash")
            .args(["-n", "/dev/stdin"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                use std::io::Write;
                child.stdin.take().unwrap().write_all(RESTORE_SCRIPT.as_bytes())?;
                child.wait_with_output()
            })
            .expect("bash");
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    }

    /// Declaring no backup must leave no trace of one, and declaring one
    /// must layer the binary it needs. A timer that dies on a missing
    /// restic is a backup that silently is not one.
    #[test]
    fn backup_is_absent_until_declared() {
        let without = generate(&config("schema_version = 1\n[snapshots]\nenable = true\n"));
        assert!(!without.contains("kuma-backup"));
        assert!(!without.contains("kuma-restore"), "no backup means nothing to restore from");
        assert!(!without.contains("restic"));

        let with = generate(&config(
            "schema_version = 1\n[snapshots]\nenable = true\n\
             [backup]\nenable = true\nrepo = \"b2:kuma\"\n",
        ));
        assert!(with.contains("RUN systemctl enable kuma-backup.timer"));
        assert!(with.contains("restic"), "the binary the unit calls has to be in the image");
    }

    /// A remote that serves a delta this machine refuses fails the same
    /// way forever, so every download the converger does has to be able
    /// to fall back to the whole file. The install pass is the one that
    /// caught fire in the field: `--or-update` means it, not the update
    /// line, is where an already-present app takes a new version.
    #[test]
    fn every_flatpak_download_can_retry_without_deltas() {
        for path in ["install_declared || install_declared --no-static-deltas", "flatpak update"] {
            assert!(FLATPAK_SYNC_SCRIPT.contains(path), "missing download path: {path}");
        }
        let update = FLATPAK_SYNC_SCRIPT
            .split("cp \"$declared\" \"$state\"")
            .nth(1)
            .expect("the update runs after the state file is written");
        assert!(
            update.contains(
                "|| flatpak update --system --assumeyes --noninteractive --no-static-deltas"
            ),
            "the update pass must retry without deltas"
        );
        // The retry is a fallback, not the default: deltas are why an
        // update is a few megabytes instead of a few hundred.
        assert_eq!(
            FLATPAK_SYNC_SCRIPT.matches("--no-static-deltas").count(),
            2,
            "one retry per download path, and no path defaulting to whole downloads"
        );
        assert!(
            !FLATPAK_SYNC_SCRIPT.contains("install_declared --no-static-deltas\ninstall"),
            "the fallback runs only after the delta attempt failed"
        );
    }

    /// The declared file reaches the image under its scope, holding
    /// kuma's keys and nothing else, and the two units that apply it are
    /// both installed. The user unit needs `--global`, which is what
    /// puts it in a home directory without root writing into one.
    #[test]
    fn overrides_bake_per_scope_and_enable_both_passes() {
        let toml = "schema_version = 1\n\
             [system]\ndesktop = \"niri\"\n\
             [overrides.\"org.mozilla.firefox\"]\n\
             sockets = [\"wayland\"]\n\
             [overrides.\"org.gnome.Loupe\"]\n\
             scope = \"user\"\n\
             filesystems = [\"xdg-pictures:ro\"]\n";
        let out = generate(&config(toml));
        assert!(out.contains("COPY overrides /usr/lib/kuma/overrides"));
        assert!(out.contains(
            "COPY kuma-flatpak-overrides-user.service /usr/lib/systemd/user/kuma-flatpak-overrides.service"
        ));
        assert!(out.contains("systemctl --global enable kuma-flatpak-overrides.service"));

        let dir = tempfile::tempdir().unwrap();
        context(toml, dir.path());
        let system = dir.path().join("overrides/system/org.mozilla.firefox");
        let user = dir.path().join("overrides/user/org.gnome.Loupe");
        assert_eq!(std::fs::read_to_string(&system).unwrap(), "[Context]\nsockets=wayland;\n");
        assert_eq!(
            std::fs::read_to_string(&user).unwrap(),
            "[Context]\nfilesystems=xdg-pictures:ro;\n"
        );
        // scope decides the directory, and nothing lands in both
        assert!(!dir.path().join("overrides/user/org.mozilla.firefox").exists());
        assert!(!dir.path().join("overrides/system/org.gnome.Loupe").exists());
    }

    /// An image with no overrides declared still ships the converger,
    /// because the declaration that just dropped its last override is
    /// exactly the one with keys to take back. Gating on "any declared"
    /// would delete the converger in the same build that gives it its
    /// last job.
    #[test]
    fn the_override_converger_ships_even_with_nothing_declared() {
        let out = generate(&config("schema_version = 1\n[system]\ndesktop = \"niri\"\n"));
        assert!(out.contains("systemctl enable kuma-flatpak-overrides.service"));
        let dir = tempfile::tempdir().unwrap();
        context("schema_version = 1\n[system]\ndesktop = \"niri\"\n", dir.path());
        assert!(dir.path().join("overrides/system").is_dir());
        assert!(dir.path().join("kuma-flatpak-overrides.service").exists());
    }

    /// Permissions are not on the daily timer, on purpose: an install
    /// arriving at a random hour is harmless, a permission reverting at
    /// a random hour is a Flatseal toggle flipping back tomorrow
    /// afternoon for no reason a person can see.
    #[test]
    fn overrides_converge_at_boot_and_never_on_the_timer() {
        assert!(FLATPAK_OVERRIDES_SERVICE.contains("WantedBy=multi-user.target"));
        assert!(FLATPAK_OVERRIDES_SERVICE.contains("After=kuma-flatpak-sync.service"));
        assert!(
            !FLATPAK_OVERRIDES_SERVICE.contains("OnCalendar")
                && !FLATPAK_SYNC_TIMER.contains("overrides"),
            "nothing may put permission convergence on a clock"
        );
        assert!(FLATPAK_OVERRIDES_USER_SERVICE.contains("WantedBy=default.target"));
        for unit in [FLATPAK_OVERRIDES_SERVICE, FLATPAK_OVERRIDES_USER_SERVICE] {
            assert!(unit.contains("Type=oneshot"));
            assert!(unit.contains("/usr/bin/kuma flatpak-overrides --scope"));
        }
    }

    #[test]
    fn flatpak_sync_removes_only_what_it_installed() {
        assert!(FLATPAK_SYNC_SCRIPT.contains("flatpak uninstall --system"));
        assert!(FLATPAK_SYNC_SCRIPT.contains(r#"done < "$state""#));
        assert!(
            !FLATPAK_SYNC_SCRIPT.contains("flatpak list"),
            "removal candidates must come from the state file, never from what is installed"
        );
        // user-level installs are personal state — never touched
        assert!(!FLATPAK_SYNC_SCRIPT.contains("--user"));
        let dir = tempfile::tempdir().unwrap();
        context("schema_version = 1\n[system]\ndesktop = \"niri\"\n", dir.path());
        // an empty declaration is real content: take back everything
        // convergence ever installed, leaving the owner's apps alone
        assert_eq!(std::fs::read_to_string(dir.path().join("flatpaks")).unwrap(), "");
        assert!(dir.path().join("kuma-flatpak-sync").exists());
    }

    /// Both syncs keep software current without claiming it. Scoping the
    /// upgrade to the declared list is what left 13 outdated formulae,
    /// every store-installed app, and a stale runtime on a real machine
    /// whose convergence had been running daily the whole time. So the
    /// upgrade must name nothing: the moment it takes an argument it has
    /// a scope again, and everything outside that scope rots.
    ///
    /// The ordering assertions are the ones worth keeping. Both scripts
    /// run under `set -euo pipefail`, so an upgrade placed before the
    /// state file is written would, on any flaky-network run, abort the
    /// script with authority tracking still describing the previous
    /// declaration, stranding a removal until some later run happened to
    /// have working DNS.
    #[test]
    fn both_syncs_update_what_they_do_not_own() {
        // Match whole lines, not offsets into them. Searching for the
        // command and slicing from the hit hides everything to its left,
        // so a re-scoped `xargs -a "$declared" flatpak update ...` reads
        // as unscoped: the assertion holds exactly when the thing it
        // guards has been undone.
        let line_of = |script: &'static str, needle: &str| -> (usize, &'static str) {
            let mut at = 0;
            for line in script.lines() {
                if line.contains(needle) {
                    return (at, line);
                }
                at += line.len() + 1;
            }
            panic!("{needle} is gone from the sync script");
        };

        let (flatpak_update, line) = line_of(FLATPAK_SYNC_SCRIPT, "flatpak update");
        assert!(
            !line.contains("declared") && !line.contains("xargs"),
            "the update must be unscoped, or undeclared apps and runtimes never move: {line}"
        );
        let (flatpak_state, _) = line_of(FLATPAK_SYNC_SCRIPT, r#"cp "$declared" "$state""#);
        assert!(
            flatpak_state < flatpak_update,
            "authority must be recorded before the update, so a failed update cannot strand it"
        );
        // The prune runs last so runtimes the update orphans go with it.
        let (flatpak_prune, _) = line_of(FLATPAK_SYNC_SCRIPT, "--unused");
        assert!(
            flatpak_prune > flatpak_update,
            "unused runtimes are pruned after the update, not before"
        );

        let (brew_upgrade, line) = line_of(BREW_SYNC_SCRIPT, r#""$brew" upgrade"#);
        assert_eq!(
            line.trim(),
            r#""$brew" upgrade"#,
            "bare upgrade takes formulae and casks alike; an argument list takes neither"
        );
        let (brew_state, _) = line_of(BREW_SYNC_SCRIPT, r#"cp "$declared" "$state""#);
        assert!(brew_state < brew_upgrade, "same ordering rule as the flatpak sync");

        // Membership is still the declaration's alone. Widening the
        // upgrade must not widen the install: only the declared list is
        // ever installed, in both scripts.
        assert!(BREW_SYNC_SCRIPT.contains(r#"xargs -a "$declared" "$brew" install"#));
        assert!(FLATPAK_SYNC_SCRIPT.contains(r#"xargs -r -a "$declared" flatpak install"#));
    }

    #[test]
    fn user_generates_boot_sync_not_build_time_useradd() {
        let out = generate(&config(
            "schema_version = 1\n[user]\nname = \"mira\"\nshell = \"fish\"\nssh_keys = [\"ssh-ed25519 AAAA m@kuma\"]\n[packages]\nrpm = [\"fish\"]\n",
        ));
        // 600: the declaration can carry a password hash — root-only
        assert!(out.contains("COPY --chmod=600 kuma-user /usr/lib/kuma/user"));
        assert!(out.contains("RUN systemctl enable kuma-user-sync.service"));
        assert!(out.contains("COPY kuma-user-keys /etc/kuma/keys/mira"));
        assert!(out.contains("sshd_config.d/40-kuma-keys.conf"));
        // /home is machine state — the account must be created at boot
        assert!(!out.contains("useradd"));
        // the shell check comes after the rpm layer that installs it
        let rpm_at = out.find("keepcache=1 fish").unwrap();
        let check_at = out.find("RUN test -x /usr/bin/fish").unwrap();
        assert!(rpm_at < check_at);

        // A declaration with no [user] bakes no ACCOUNT, but it does ship
        // the converger. That distinction is the whole point: a published
        // image declares no account on purpose, and a machine installed
        // from one has none and no root password either, so an installer
        // writes /var/lib/kuma/user on the target and something has to act on
        // it at first boot. Shipping the unit only when the image already
        // knew the answer is what made that impossible.
        let out = generate(&config("schema_version = 1"));
        assert!(!out.contains("/usr/lib/kuma/user"), "no account data without a declared one");
        assert!(!out.contains("kuma-user-keys"));
        assert!(out.contains("RUN systemctl enable kuma-user-sync.service"));
        // ... and it is a no-op with neither file present, rather than a
        // unit that fails on every boot of a userless image.
        assert!(USER_SYNC_SCRIPT.contains("if [ -f /usr/lib/kuma/user ]"));
        assert!(USER_SYNC_SCRIPT.contains(r#"[ -n "${KUMA_USER:-}" ] || exit 0"#));
    }

    /// A shell the image does not install locks every account made on
    /// that machine out of logging in, and on a published image nothing
    /// notices until an installer has already written a disk. Same
    /// guard the declared user's shell has always had, applied to the
    /// field shareable media can actually carry.
    #[test]
    fn a_declared_system_shell_is_checked_at_build_time() {
        let out = generate(&config(
            "schema_version = 1\n[system]\nshell = \"fish\"\n[packages]\nrpm = [\"fish\"]\n",
        ));
        assert!(out.contains("RUN test -x /usr/bin/fish"));
        // After the layer that would install it, or the guard fails on
        // an image that was going to be fine.
        let rpm_at = out.find("keepcache=1 fish").unwrap();
        assert!(rpm_at < out.find("RUN test -x /usr/bin/fish").unwrap());
        // And nothing is emitted when nothing is declared.
        assert!(!generate(&config("schema_version = 1")).contains("test -x /usr/bin/"));
    }

    /// Machine state beats image content, the same way it does for
    /// hostname and timezone. On a personal image the account is
    /// declaration and gets baked; on a shared one it cannot be, because
    /// the image is shared and the person is not.
    ///
    /// /var rather than /etc, and that is the load-bearing half. bootc
    /// fills /var from the image once at install and never again, while
    /// /etc is three-way merged on every update: a file an installer
    /// shipped as image content is not a local modification, so merging
    /// against a published image that has no such file DELETES it. The
    /// account would outlive the file describing it, and the converger
    /// would quietly stop maintaining groups and shell.
    #[test]
    fn a_written_user_file_outranks_the_baked_one_and_lives_where_updates_cannot_reach() {
        // Both are sourced, machine state second, so the later
        // assignments win key by key. Whole-file precedence was wrong in
        // one direction that mattered: an image declares [system].shell
        // and no person, an installer writes a person and was told
        // nothing about shells, and skipping the image's file entirely
        // made a machine whose shell nobody had asked for.
        let usr = USER_SYNC_SCRIPT.find(". /usr/lib/kuma/user").unwrap();
        let var = USER_SYNC_SCRIPT.find(". /var/lib/kuma/user").unwrap();
        assert!(usr < var, "the machine's own file has to be sourced last");
        // The keys that describe a person do not carry over, so an image
        // that named one cannot lend its password to somebody else's
        // account.
        let unset = USER_SYNC_SCRIPT.find("unset KUMA_USER").unwrap();
        assert!(usr < unset && unset < var);
        assert!(!USER_SYNC_SCRIPT.contains("unset KUMA_SHELL"), "[system].shell is the image's");
        assert!(
            !USER_SYNC_SCRIPT.contains("/etc/kuma/user"),
            "/etc is merged on update; the installer's file would be deleted"
        );
    }

    /// /etc/hostname is image content, so writing it at boot is what
    /// makes it a local modification and therefore what survives the
    /// merge. Shipping it in the installer's layer instead reverts to the
    /// published image's hostname on the first update.
    #[test]
    fn a_written_hostname_is_applied_at_boot_not_shipped_as_image_content() {
        assert!(USER_SYNC_SCRIPT.contains("/var/lib/kuma/hostname"));
        assert!(USER_SYNC_SCRIPT.contains("> /etc/hostname"));
        // Ahead of the account guard, so a machine that named itself but
        // declared no account still gets its name.
        let host = USER_SYNC_SCRIPT.find("/var/lib/kuma/hostname").unwrap();
        let guard = USER_SYNC_SCRIPT.find(r#"[ -n "${KUMA_USER:-}" ] || exit 0"#).unwrap();
        assert!(host < guard);
    }

    #[test]
    fn autologin_adds_initial_session() {
        let with = greetd_config(&config(
            "schema_version = 1\n[user]\nname = \"mira\"\nautologin = true\n",
        ));
        assert!(with.contains("[initial_session]"));
        assert!(with.contains("user = \"mira\""));
        // greeter remains the fallback for logout
        assert!(with.contains("[default_session]"));

        let without = greetd_config(&config("schema_version = 1\n[user]\nname = \"mira\"\n"));
        assert!(!without.contains("[initial_session]"));
    }

    /// Mod+D opens the menu, and the stock line it replaces is grepped
    /// for first. A niri release that renames that line must fail the
    /// build rather than ship media whose main key does nothing.
    #[test]
    fn the_launcher_key_opens_the_menu_and_fails_loudly_if_it_cannot() {
        let out = generate(&config("schema_version = 1\n[system]\ndesktop = \"niri\"\n"));
        assert!(
            out.contains(&format!("grep -qF '{NIRI_STOCK_LAUNCHER}'")),
            "the stock bind is a gate"
        );
        assert!(
            out.contains(&format!("s|{NIRI_STOCK_LAUNCHER}|{NIRI_MENU_BIND}|")),
            "and is replaced"
        );
        assert!(NIRI_MENU_BIND.contains("Mod+D"), "the key is the one the hand already goes to");
        assert!(!NIRI_MENU_BIND.contains("fuzzel"), "plain fuzzel is no longer what it opens");
        assert!(
            !NIRI_MEDIA_BINDS.contains("kuma\" \"menu"),
            "one key for the menu, not a second chord beside it"
        );
        // The stock line is what niri actually ships, not a guess: this
        // is the pairing that makes the grep meaningful.
        assert!(NIRI_STOCK_LAUNCHER.contains(r#"spawn "fuzzel";"#));
    }

    /// Nothing greets a person on kuma's behalf but kuma.
    ///
    /// The shell ships a first-run wizard, and a kuma machine has
    /// already answered what it asks. Asserted because it is a
    /// first-impression setting: it is invisible on every boot after the
    /// first, so losing it would be noticed by strangers and by nobody
    /// testing.
    /// Suspending locks, which was a clause of the swayidle line that
    /// left and is not covered by the two idle behaviors that replaced
    /// it. The shell defaults to it; kuma pins it, because a beta that
    /// flips this default unlocks every machine that suspends and says
    /// nothing.
    #[test]
    fn suspending_locks_the_screen() {
        assert!(KUMA_NOCTALIA.contains("lock_before_suspend = true"));
    }

    /// Every icon an entry names is checked in the build, not the first.
    #[test]
    fn the_build_checks_every_icon_the_entries_name() {
        let out = generate(&config("schema_version = 1\n[system]\ndesktop = \"niri\"\n"));
        for entry in seam::ENTRIES {
            assert!(
                out.contains(&format!("-name {}.svg", entry.icon)),
                "{} names {}, which the build never looks for",
                entry.id,
                entry.icon
            );
        }
    }

    #[test]
    fn no_other_vendor_greets_the_person_on_first_login() {
        assert!(KUMA_NOCTALIA.contains("setup_wizard_enabled = false"));
    }

    /// The shell's fallback wallpaper is kuma's.
    ///
    /// `[wallpaper.default] path` in kuma's config is not the mechanism
    /// and cannot be: the shell accepts the key and ignores it outside
    /// its own state, so a first boot showed noctalia's asset with kuma's
    /// config loaded and validating clean. Replacing the file is the only
    /// lever, and the `test -f` in front of it means an upstream rename
    /// fails the build rather than quietly restoring their wallpaper.
    #[test]
    fn the_shells_default_wallpaper_is_kumas() {
        let out = generate(&config("schema_version = 1\n[system]\ndesktop = \"niri\"\n"));
        let guard = out.find("RUN test -f /usr/share/noctalia/assets/noctalia-wallpaper.png");
        let copy = out.find("COPY kuma-wallpaper.jpg /usr/share/noctalia/assets/");
        assert!(guard.is_some() && copy.is_some(), "the asset is not replaced");
        assert!(guard < copy, "the guard must run before the file is overwritten");
        // The table header, not the string: the config explains in a
        // comment why the key is absent, and a substring check reads its
        // own explanation as the thing it forbids.
        assert!(
            !KUMA_NOCTALIA.lines().any(|line| line.trim() == "[wallpaper.default]"),
            "that key reads as if it works; it does not"
        );
    }

    /// No bind advertises a program the image does not have.
    ///
    /// The lock bind is the one that matters most: niri puts it on the
    /// Important Hotkeys overlay that opens at first login, so a dead key
    /// there is the first thing a new machine tells a person to press.
    /// It went dead the moment swaylock left the set, silently, and it
    /// took looking at a booted VM to notice.
    #[test]
    fn no_bind_names_a_program_the_image_excludes() {
        let out = generate(&config("schema_version = 1\n[system]\ndesktop = \"niri\"\n"));
        // grepped for before the rewrite, so a niri release that rewords
        // the line fails the build instead of shipping the dead key
        assert!(out.contains(&format!("grep -qF '{NIRI_STOCK_LOCK}'")), "unguarded rewrite");
        assert!(out.contains(&format!("s|{NIRI_STOCK_LOCK}|{NIRI_LOCK_BIND}|")), "not rewritten");
        // Spawn lines only. The prose in NIRI_EXTRAS names every program
        // the shell replaced, which is the point of it.
        let spawns = [NIRI_MENU_BIND, NIRI_LOCK_BIND, NIRI_MEDIA_BINDS, NIRI_EXTRAS]
            .iter()
            .flat_map(|block| block.lines())
            .map(str::trim)
            .filter(|line| !line.starts_with("//") && line.contains("spawn"));
        for line in spawns {
            for excluded in NIRI_EXCLUDES {
                assert!(
                    !line.contains(excluded),
                    "this spawns {excluded}, which the image excludes: {line}"
                );
            }
        }
    }

    #[test]
    fn every_kuma_verb_a_keybinding_spawns_is_a_real_verb() {
        use clap::CommandFactory;
        let cli = crate::Cli::command();
        let verbs: Vec<String> =
            cli.get_subcommands().map(|sub| sub.get_name().to_string()).collect();
        let mut found = 0;
        for bind in [NIRI_MEDIA_BINDS, NIRI_EXTRAS, NIRI_MENU_BIND] {
            for (index, _) in bind.match_indices(r#"spawn "kuma""#) {
                let rest = &bind[index..];
                let verb =
                    rest.split('"').nth(3).expect("a spawn line names a verb after the program");
                assert!(
                    verbs.contains(&verb.to_string()),
                    "a keybinding spawns `kuma {verb}`, which is not a verb"
                );
                found += 1;
            }
        }
        // No floor on `found` any more. kuma's verbs left the keybindings
        // when Mod+D became the shell's launcher; they are desktop
        // entries now, and `seam::tests::every_entry_names_a_real_verb`
        // is what keeps THOSE from naming a verb that does not exist.
        // This still runs because a bind naming a kuma verb may come
        // back, and the dead-key bug it was written for is cheap to
        // re-introduce and invisible when it happens.
        let _ = found;
    }

    #[test]
    fn session_polish_ships_osd_and_battery_watch() {
        // Locking and screen-off were swayidle arguments; they are the
        // shell's now, and they ship DISABLED, so kuma turning them on is
        // the difference between a machine that locks and one that does
        // not. Asserted on the config because that is where it lives.
        assert!(KUMA_NOCTALIA.contains("[idle.behavior.lock]"));
        assert!(KUMA_NOCTALIA.contains("[idle.behavior.screen-off]"));
        assert_eq!(
            KUMA_NOCTALIA.matches("enabled = true").count(),
            3,
            "lock, screen-off, nightlight"
        );
        assert!(NIRI_EXTRAS.contains("kuma-battery-watch"));
        assert!(NIRI_EXTRAS.contains("noctalia"));
        // Both X11 helpers wait for a DISPLAY that does not exist yet
        // when they are spawned. They share one copy of that wait, so
        // this asks the rendered scripts rather than the const: a
        // refactor that dropped it from one of them would leave a
        // helper that exits on every login and reports nothing.
        for script in [xsettings_launcher(), clipboard_bridge()] {
            assert!(script.contains("systemctl --user show-environment"), "{script}");
            assert!(script.trim_end().ends_with("-x") || script.contains("exec xsettingsd"));
        }
        // media keys route through the OSD helper, spliced into stock binds
        assert!(NIRI_MEDIA_BINDS.contains("kuma-osd"));
        let out = generate(&config("schema_version = 1\n[system]\ndesktop = \"niri\"\n"));
        assert!(out.contains("-e '/XF86Audio/d'"));
        assert!(out.contains("r /usr/lib/kuma/niri-binds.kdl"));
    }

    /// The baked defaults name apps the image actually has: flatpak ids
    /// for the ones the example declares, native ids for the ones the
    /// desktop set installs. A handler pointing at an app nobody installs
    /// is a link that opens nothing, which is how the default browser sat
    /// on Chromium while every real declaration shipped Firefox.
    ///
    /// Native desktop ids look exactly like flatpak ids, so the handlers
    /// the desktop set installs are named here rather than inferred.
    /// Everything else in the list has to come from the declaration —
    /// docs/desktops.md explains the desktop members whose presence is
    /// not self-evident, and every one of them is a failure someone had
    /// to diagnose: a silent keyring, a disabled clock, no swap, tofu.
    /// Dropping one from the set while the page still explains it turns
    /// the page into a lie about the image, and the reverse hides the
    /// only written record of why the package is there at all.
    ///
    /// Only these are pinned, deliberately. The full inventory's home is
    /// `kuma generate`, and asking a doc to track seventy packages would
    /// buy drift in exchange for churn.
    #[test]
    fn the_surprising_desktop_packages_are_the_ones_documented() {
        const EXPLAINED: &[&str] = &[
            "gnome-keyring-pam",
            "nss-mdns",
            "zram-generator-defaults",
            "glibc-langpack-en",
            "mesa-vulkan-drivers",
            "vulkan-loader",
            "avahi",
        ];
        // niri-only: COSMIC's set has no waybar to lose its glyphs, and
        // fonts follow the arm that renders with them.
        const EXPLAINED_NIRI: &[&str] = &["fontawesome-fonts-all", "google-noto-sans-cjk-vf-fonts"];

        let doc = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/desktops.md"))
            .unwrap();
        // Scoped to the section that does the explaining, not the whole
        // page. Every package is also named in the inventory tables, so
        // searching the file would assert only that the word appears
        // somewhere, which stays true precisely when the explanation is
        // the thing that got deleted.
        let why = doc
            .split_once("## Why these are here")
            .and_then(|(_, rest)| rest.split_once("\n## "))
            .map(|(section, _)| section)
            .expect("docs/desktops.md explains the non-obvious members");

        for pkg in EXPLAINED {
            assert!(NIRI_PACKAGES.contains(pkg), "{pkg} left the niri set");
            assert!(COSMIC_PACKAGES.contains(pkg), "{pkg} left the COSMIC set");
            assert!(why.contains(pkg), "docs/desktops.md stopped explaining {pkg}");
        }
        for pkg in EXPLAINED_NIRI {
            assert!(NIRI_PACKAGES.contains(pkg), "{pkg} left the niri set");
            assert!(why.contains(pkg), "docs/desktops.md stopped explaining {pkg}");
        }
    }

    /// and the declaration to check is the niri one, because this file
    /// is only baked on the niri arm.
    #[test]
    fn the_baked_defaults_name_apps_the_examples_install() {
        // shipped by NIRI_PACKAGES, never by a declaration
        const IN_IMAGE: &[&str] = &["thunar", "org.gnome.FileRoller"];
        let example =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/niri.toml"))
                .unwrap();
        let declared: Config = toml::from_str(&example).unwrap();
        let mut checked = 0;
        for line in MIMEAPPS.lines() {
            let Some((mime, handler)) = line.split_once('=') else {
                continue; // the [Default Applications] header
            };
            let app = handler.strip_suffix(".desktop").expect("a desktop id");
            if IN_IMAGE.contains(&app) {
                continue;
            }
            assert!(
                declared.packages.flatpak.iter().any(|a| a == app),
                "{app} handles {mime} but examples/niri.toml doesn't install it"
            );
            checked += 1;
        }
        assert!(checked > 0, "nothing was checked: has MIMEAPPS changed shape?");
        // the handler that actually went stale once, still named explicitly
        assert!(MIMEAPPS.contains("x-scheme-handler/https=org.mozilla.firefox.desktop"));
    }

    /// The example's `disable` line must not name a unit kuma's desktop
    /// deliberately enables, or the file argues with the image it
    /// compiles to. It read `avahi-daemon.service` when this was written,
    /// and the obvious replacement (`bluetooth.service`) was enabled by
    /// kuma too, which is how the mistake got made twice: the unit has to
    /// be chosen against kuma's curation, not against the base's defaults.
    #[test]
    fn the_disable_example_does_not_fight_the_desktop() {
        let out = generate(&config("schema_version = 1\n[system]\ndesktop = \"niri\"\n"));
        // EVERY enabling line, not the first one. There is more than one
        // now (the shell's user unit and the sleep guard have their own),
        // and picking the first made this test read a line it was not
        // written about and fail on a tree that was correct.
        let enabled: Vec<&str> = out
            .lines()
            .filter(|line| line.contains("systemctl enable "))
            .flat_map(|line| line.split_whitespace())
            .filter(|word| word.ends_with(".service"))
            .collect();
        assert!(!enabled.is_empty(), "the desktop arm enables units");
        assert!(enabled.contains(&"avahi-daemon.service"), "sanity: kuma enables avahi");

        let example =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/niri.toml"))
                .unwrap();
        for line in example.lines() {
            let line = line.trim_start().trim_start_matches('#').trim_start();
            let Some(units) = line.strip_prefix("disable = [") else { continue };
            for unit in units.split(['"', ',', ']']).filter(|u| u.ends_with(".service")) {
                assert!(
                    !enabled.contains(&unit),
                    "the example suggests disabling {unit}, which kuma's desktop enables"
                );
            }
        }
    }

    #[test]
    fn daily_driver_glue() {
        // The clipboard and wallpaper widgets both left the bar in 0.16,
        // so these two binds are the only routes to their panels left in
        // the image. Losing a bind here strands a panel.
        assert!(NIRI_MEDIA_BINDS.contains(r#"panel-toggle" "clipboard"#));
        assert!(NIRI_MEDIA_BINDS.contains(r#"panel-toggle" "wallpaper"#));
        assert!(KUMA_NOCTALIA.contains(r#"start = [ "launcher", "workspaces" ]"#));
        assert!(MIMEAPPS.contains("application/pdf=org.gnome.Papers.desktop"));
        assert!(MIMEAPPS.contains("inode/directory=thunar.desktop"));
        let out = generate(&config("schema_version = 1\n[system]\ndesktop = \"niri\"\n"));
        assert!(out.contains("COPY mimeapps.list /etc/xdg/mimeapps.list"));
        assert!(out.contains("pam_gnome_keyring"));
    }

    #[test]
    fn infra_round_swap_discovery_updates() {
        let out = generate(&config(
            "schema_version = 1\n[system]\ndesktop = \"niri\"\n[packages]\nflatpak = [\"org.gnome.Loupe\"]\n",
        ));
        assert!(out.contains("zram-generator-defaults"));
        assert!(out.contains("avahi-daemon.service chronyd.service"));
        assert!(out.contains("kuma-flatpak-sync.service kuma-flatpak-sync.timer"));
        assert!(FLATPAK_SYNC_TIMER.contains("Persistent=true"));
        // do-not-disturb left with mako; the control centre owns it.
        assert!(!NIRI_MEDIA_BINDS.contains("makoctl"), "no daemon behind that key");
        assert!(NIRI_MEDIA_BINDS.contains("kuma-record"));
        // the XF86Audio sed deletes niri's stock playerctl binds; these
        // re-additions ride the bind-splice file, immune to that sed
        assert!(NIRI_MEDIA_BINDS.contains("play-pause"));
        assert!(NIRI_MEDIA_BINDS.contains("swappy"));
        assert!(NIRI_EXTRAS.contains("spawn-at-startup \"udiskie\""));
    }

    #[test]
    fn context_writes_user_declaration() {
        let dir = tempfile::tempdir().unwrap();
        context("schema_version = 1\n[user]\nname = \"mira\"\nshell = \"fish\"\npassword_hash = \"$6$ab$cd\"\nssh_keys = [\"ssh-ed25519 AAAA m@kuma\"]\n", dir.path());
        let decl = std::fs::read_to_string(dir.path().join("kuma-user")).unwrap();
        assert_eq!(
            decl,
            "KUMA_USER='mira'\nKUMA_SHELL='/usr/bin/fish'\nKUMA_GROUPS='wheel'\nKUMA_PASSWORD_HASH='$6$ab$cd'\n"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("kuma-user-keys")).unwrap(),
            "ssh-ed25519 AAAA m@kuma\n"
        );
        let script = std::fs::read_to_string(dir.path().join("kuma-user-sync")).unwrap();
        assert!(script.contains("chpasswd -e"));
        assert!(script.contains("usermod -aG"));
    }

    /// `shell` is the one [user] field with two code paths, and only the
    /// declared one was covered. Undeclared has to mean *absent*, not
    /// empty: kuma-user-sync guards both its useradd -s and its usermod -s
    /// on `[ -n "${KUMA_SHELL:-}" ]`, so a `KUMA_SHELL=''` line would
    /// still take the guard's false branch, but `usermod -s ''` is what a
    /// later reader would "fix" it into. Pinning absence keeps the two
    /// halves honest about which one is load-bearing.
    #[test]
    fn a_user_with_no_declared_shell_pins_nothing_about_the_shell() {
        let dir = tempfile::tempdir().unwrap();
        context("schema_version = 1\n[user]\nname = \"mira\"\n", dir.path());
        let decl = std::fs::read_to_string(dir.path().join("kuma-user")).unwrap();
        assert_eq!(decl, "KUMA_USER='mira'\nKUMA_GROUPS='wheel'\n");

        // The build-time guard exists to fail a declaration naming a shell
        // the packages never install. With no shell named there is nothing
        // to install and nothing to check, and a stray `test -x /usr/bin/`
        // would fail every build.
        let out = generate(&config("schema_version = 1\n[user]\nname = \"mira\"\n"));
        assert!(!out.contains("test -x /usr/bin/"));
        // The account is still converged; only the shell claim is dropped.
        assert!(out.contains("RUN systemctl enable kuma-user-sync.service"));
    }

    /// The default is the whole point of the field: a declaration that
    /// names a user and stops must still produce an account that can
    /// administer the machine, because nothing else in the declaration
    /// grants sudo. Pinned here because "groups defaults to wheel" reads
    /// like a convenience and is the only path to root on a fresh install.
    #[test]
    fn a_user_who_declares_no_groups_still_lands_in_wheel() {
        let dir = tempfile::tempdir().unwrap();
        context("schema_version = 1\n[user]\nname = \"mira\"\n", dir.path());
        let decl = std::fs::read_to_string(dir.path().join("kuma-user")).unwrap();
        assert!(decl.contains("KUMA_GROUPS='wheel'\n"));

        // Asking for none is a different answer from asking for nothing,
        // and the sync script iterates `${KUMA_GROUPS:-}`, so the line has
        // to be gone rather than empty for the loop to run zero times.
        let dir = tempfile::tempdir().unwrap();
        context("schema_version = 1\n[user]\nname = \"mira\"\ngroups = []\n", dir.path());
        let decl = std::fs::read_to_string(dir.path().join("kuma-user")).unwrap();
        assert!(!decl.contains("KUMA_GROUPS"));
    }

    /// The hostname must ship as a COPY, never a RUN redirect: buildah
    /// bind-mounts /etc/hostname into RUN containers, so a redirect
    /// writes the runtime mount and the image ships no hostname at all —
    /// which is exactly what every image did until 2026-08.
    #[test]
    fn hostname_and_locale_pins() {
        let out = generate(&config(
            "schema_version = 1\n[system]\nhostname = \"kuma-laptop\"\nlocale = \"de_DE.UTF-8\"\n",
        ));
        assert!(out.contains("COPY hostname /etc/hostname"));
        assert!(!out.contains("> /etc/hostname"));
        let dir = tempfile::tempdir().unwrap();
        context("schema_version = 1\n[system]\nhostname = \"kuma-laptop\"\n", dir.path());
        assert_eq!(std::fs::read_to_string(dir.path().join("hostname")).unwrap(), "kuma-laptop\n");
        // undeclared, the default seeds the ostree merge default
        let dir = tempfile::tempdir().unwrap();
        context("schema_version = 1\n", dir.path());
        assert_eq!(std::fs::read_to_string(dir.path().join("hostname")).unwrap(), "kuma\n");
        assert!(out.contains(&dnf_install("glibc-langpack-de")));
        assert!(out.contains("RUN echo 'LANG=de_DE.UTF-8' > /etc/locale.conf"));
        // C.UTF-8 has no territory, so no langpack layer
        let out = generate(&config("schema_version = 1\n[system]\nlocale = \"C.UTF-8\"\n"));
        assert!(!out.contains("glibc-langpack"));
        assert!(out.contains("LANG=C.UTF-8"));
    }

    #[test]
    fn context_includes_flatpak_list() {
        let dir = tempfile::tempdir().unwrap();
        context("schema_version = 1\n[packages]\nflatpak = [\"org.mozilla.firefox\", \"org.gnome.Loupe\"]\n", dir.path());
        let list = std::fs::read_to_string(dir.path().join("flatpaks")).unwrap();
        assert_eq!(list, "org.mozilla.firefox\norg.gnome.Loupe\n");
        let script = std::fs::read_to_string(dir.path().join("kuma-flatpak-sync")).unwrap();
        // remote pinned: multiple remotes offering the same ref would make
        // non-interactive installs fail. Asserted as a property of the
        // install line rather than as adjacent words, which broke the
        // moment a flag was passed through to it: the remote is the last
        // word because xargs appends the app names after it.
        let install = script
            .lines()
            .find(|l| l.contains("flatpak install --system"))
            .expect("the declared list has to be installed by something");
        assert!(install.contains("--or-update"));
        assert!(
            install.trim_end().ends_with("flathub"),
            "the install must name exactly one remote: {install}"
        );
    }

    #[test]
    fn brew_generates_setup_service_and_shell_profiles() {
        let out = generate(&config("schema_version = 1\n[system]\nbrew = true\n"));
        assert!(out.contains("git-core"));
        assert!(out.contains("COPY --chmod=755 kuma-brew-setup /usr/libexec/kuma-brew-setup"));
        assert!(out.contains("systemctl enable kuma-brew-setup.service"));
        assert!(out.contains("/etc/profile.d/kuma-brew.sh"));
        assert!(out.contains("/etc/fish/conf.d/kuma-brew.fish"));

        let dir = tempfile::tempdir().unwrap();
        context("schema_version = 1\n[system]\nbrew = true\n", dir.path());
        let script = std::fs::read_to_string(dir.path().join("kuma-brew-setup")).unwrap();
        assert!(script.contains("/home/linuxbrew/.linuxbrew"));
        assert!(dir.path().join("kuma-brew-setup.service").exists());
    }

    #[test]
    fn declared_brews_imply_bootstrap_and_converge() {
        // no system.brew = true needed: the list alone pulls in the bootstrap
        let toml = "schema_version = 1\n[packages]\nbrew = [\"ripgrep\", \"node@22\"]\n";
        let out = generate(&config(toml));
        assert!(out.contains("kuma-brew-setup.service kuma-brew-sync.service kuma-brew-sync.timer"));
        assert!(out.contains("COPY brews /usr/lib/kuma/brews"));

        let dir = tempfile::tempdir().unwrap();
        context(toml, dir.path());
        let list = std::fs::read_to_string(dir.path().join("brews")).unwrap();
        assert_eq!(list, "ripgrep\nnode@22\n");
        let script = std::fs::read_to_string(dir.path().join("kuma-brew-sync")).unwrap();
        // removal authority is scoped to the state file, never `brew list`:
        // ad-hoc installs on the machine must survive convergence
        assert!(script.contains(".kuma-brews"));
        assert!(BREW_SYNC_SERVICE.contains("User=1000"));
        assert!(BREW_SYNC_TIMER.contains("Persistent=true"));
    }

    #[test]
    fn no_brew_by_default() {
        let out = generate(&config("schema_version = 1"));
        assert!(!out.contains("brew"));
    }

    #[test]
    fn context_includes_greetd_config_for_niri() {
        let dir = tempfile::tempdir().unwrap();
        context("schema_version = 1\n[system]\ndesktop = \"niri\"\n", dir.path());
        assert!(dir.path().join("Containerfile").exists());
        let greetd = std::fs::read_to_string(dir.path().join("greetd-config.toml")).unwrap();
        assert!(greetd.contains("niri-session"));
        let kargs = std::fs::read_to_string(dir.path().join("kargs-desktop.toml")).unwrap();
        assert!(kargs.contains("quiet"));
    }

    #[test]
    fn cosmic_desktop_composes_from_the_session() {
        let out = generate(&config("schema_version = 1\n[system]\ndesktop = \"cosmic\"\n"));
        assert!(out.contains("cosmic-session"));
        // Fedora's cosmic-greeter.service owns the display-manager alias;
        // enabling greetd.service alongside it fails the build
        assert!(out.contains("systemctl enable cosmic-greeter.service"));
        assert!(!out.contains("enable greetd.service"));
        // kuma declares the user — the first-boot wizard must not fire
        assert!(out.contains("rm /etc/xdg/autostart/com.system76.CosmicInitialSetup.desktop"));
        // the session pulls only the pipewire library; the daemon is on us
        assert!(out.contains("pipewire"));
        // the store would fight convergence — its installs get removed daily
        assert!(!out.contains("cosmic-store"));
        // the default dock pins the editor; the session alone doesn't pull it
        assert!(out.contains("cosmic-edit"));
        // wallpaper is identity, and the packaged dock/background defaults
        // are overwritten in place, guarded so a moved path fails the build
        assert!(
            out.contains("COPY kuma-wallpaper.jpg /usr/share/backgrounds/kuma/kuma-wallpaper.jpg")
        );
        assert!(out.contains("test -f /usr/share/cosmic/com.system76.CosmicAppList/v1/favorites"));
        assert!(out.contains(
            "COPY cosmic-favorites /usr/share/cosmic/com.system76.CosmicAppList/v1/favorites"
        ));
        assert!(out.contains(
            "COPY cosmic-background /usr/share/cosmic/com.system76.CosmicBackground/v1/all"
        ));
        // the baked dock pins only what the image ships: no store, and no
        // browser — that's the declaration's choice
        assert!(!COSMIC_FAVORITES.contains("Store"));
        assert!(!COSMIC_FAVORITES.contains("firefox"));
        // flathub remote ships in-image, same as the niri desktop
        assert!(out.contains("flathub.flatpakrepo"));
        assert!(out.contains("set-default graphical.target"));
        // codec restoration applies to every desktop, not just niri
        assert!(out.contains("mesa-va-drivers-freeworld"));
        // AMD DCC-on-scanout static bands: both knobs baked, not a
        // hand-edit on the installed machine — overlay alone recurred
        assert!(out.contains("COSMIC_DISABLE_OVERLAY_SCANOUT=1"));
        assert!(out.contains("COSMIC_DISABLE_DIRECT_SCANOUT=1"));
    }

    #[test]
    fn cosmic_autologin_appends_initial_session() {
        let with = generate(&config(
            "schema_version = 1\n[system]\ndesktop = \"cosmic\"\n[user]\nname = \"mira\"\nautologin = true\n",
        ));
        assert!(with.contains("[initial_session]"));
        assert!(with.contains("command = \"start-cosmic\""));
        assert!(with.contains("user = \"mira\""));
        // appended to the config cosmic-greeter.service actually reads
        assert!(with.contains(">> /etc/greetd/cosmic-greeter.toml"));
        let without = generate(&config(
            "schema_version = 1\n[system]\ndesktop = \"cosmic\"\n[user]\nname = \"mira\"\n",
        ));
        assert!(!without.contains("initial_session"));
    }

    /// The keyring failed silently on both desktops until 2026-08-07:
    /// gnome-keyring was installed but gnome-keyring-pam, which carries
    /// the module, was not, and the greeter's '-' prefixed PAM lines
    /// skip a missing module without logging a word. Nothing observable
    /// broke except that every keyring-using app prompted on launch, so
    /// the package and the assert are pinned here per desktop.
    #[test]
    fn keyring_unlocks_with_the_login_password_on_every_desktop() {
        let niri = generate(&config("schema_version = 1\n[system]\ndesktop = \"niri\"\n"));
        let cosmic = generate(&config("schema_version = 1\n[system]\ndesktop = \"cosmic\"\n"));
        for out in [&niri, &cosmic] {
            // the module is a subpackage; nothing else pulls it in
            assert!(out.contains("gnome-keyring-pam"));
            // and the build fails if a Fedora update stops shipping it
            assert!(out.contains("test -f /usr/lib64/security/pam_gnome_keyring.so"));
        }
        // each greeter authenticates against its own PAM service, and
        // asserting the other one would pass while proving nothing
        assert!(niri.contains("grep -q pam_gnome_keyring /etc/pam.d/greetd\n"));
        assert!(!niri.contains("/etc/pam.d/cosmic-greeter"));
        assert!(cosmic.contains("grep -q pam_gnome_keyring /etc/pam.d/cosmic-greeter\n"));
        // greetd's file exists in the COSMIC image too (cosmic-greeter
        // pulls greetd in), so a stale assert there would look healthy
        assert!(!cosmic.contains("pam_gnome_keyring /etc/pam.d/greetd\n"));
    }

    /// Ownership of an /etc file is "this build writes it", and the two
    /// ways to write one are a COPY destination and a shell redirect.
    /// Reading is not owning, which is the distinction the whole /etc
    /// drift check rests on: kuma greps Fedora's /etc/pam.d/greetd and
    /// validates its own /etc/niri/config.kdl, and only the second is
    /// kuma's to have an opinion about.
    #[test]
    fn etc_ownership_is_writes_not_reads() {
        let paths = etc_writes(
            "COPY greetd-config /etc/greetd/config.toml\n\
             COPY --chmod=600 kuma-user /usr/lib/kuma/user\n\
             RUN sed -e 's/x/y/' /usr/share/doc/niri/default.kdl > /etc/niri/config.kdl \\\n\
             RUN cat /usr/lib/kuma/extras.kdl >> /etc/niri/config.kdl\n\
             RUN printf 'A=1\\n' >> /etc/environment\n\
             RUN test -f /usr/lib64/security/pam_gnome_keyring.so \\\n\
             RUN grep -q pam_gnome_keyring /etc/pam.d/greetd\n\
             RUN niri validate --config /etc/niri/config.kdl\n\
             RUN something 2>/dev/null\n",
        );
        assert_eq!(paths, ["/etc/environment", "/etc/greetd/config.toml", "/etc/niri/config.kdl"]);
        // read-only mentions never become ownership, and a non-/etc
        // destination or redirect is not this check's business
        assert!(!paths.iter().any(|p| p.contains("pam.d")));
        assert!(!paths.iter().any(|p| p.contains("dev/null")));

        // The third way, which is how a declared timezone is written.
        // The `test -e` guard in front of it is why the destination has
        // to be the segment's last word rather than the line's.
        let linked = etc_writes(
            "RUN test -e /usr/share/zoneinfo/America/Denver && ln -sfn /usr/share/zoneinfo/America/Denver /etc/localtime\n\
             RUN ln -sfn /usr/lib/systemd/system/foo.service /usr/lib/systemd/system/bar.service\n",
        );
        assert_eq!(linked, ["/etc/localtime"]);
    }

    /// A trusted CA is state that survives every rebuild and that
    /// nothing could declare: the machine this was written on carries a
    /// hand-added anchor for its own infrastructure, and a reinstall
    /// would have lost it silently. Declaring it makes the image carry
    /// it, and because the anchor lands under /etc by a COPY, doctor
    /// watches it for free.
    #[test]
    fn a_declared_ca_anchor_is_baked_trusted_and_watched() {
        let pem = "-----BEGIN CERTIFICATE-----\nMIIBkTCB+wIJAKZ\n-----END CERTIFICATE-----\n";
        let toml = format!(
            "schema_version = 1\n[system.ca_certificates]\n\"my-root-ca\" = \"\"\"\n{pem}\"\"\"\n"
        );
        let declared = config(&toml);
        let out = generate(&declared);
        assert!(
            out.contains("COPY ca-my-root-ca.crt /etc/pki/ca-trust/source/anchors/my-root-ca.crt"),
            "{out}"
        );
        // extracted in the same layer that adds it: a trust store that
        // needs a boot to become true is one that is false in the image
        let copied = out.find("anchors/my-root-ca.crt").unwrap();
        let extracted = out.find("RUN update-ca-trust").unwrap();
        assert!(copied < extracted, "update-ca-trust runs before the anchor lands");

        let dir = tempfile::tempdir().unwrap();
        context(&toml, dir.path());
        assert_eq!(std::fs::read_to_string(dir.path().join("ca-my-root-ca.crt")).unwrap(), pem);

        // owned, therefore graded: no separate wiring needed
        assert!(etc_paths(&declared)
            .iter()
            .any(|p| p == "/etc/pki/ca-trust/source/anchors/my-root-ca.crt"));

        // and an image that declares none says nothing about trust
        let bare = config("schema_version = 1\n");
        assert!(!generate(&bare).contains("update-ca-trust"));
    }

    /// The declaration claims a timezone, so doctor has to be able to
    /// say whether the machine has it. It could not: the timezone is the
    /// one thing kuma writes into /etc with `ln -s`, the ownership scan
    /// read only COPY destinations and redirects, and so the single file
    /// `system.timezone` exists to produce was watched by nobody.
    ///
    /// The install path is where that bites. Anaconda writes its own
    /// /etc/localtime, so an installed machine has a local copy before
    /// kuma's ever arrives, and ostree's merge keeps a local file over
    /// every future image. A declared timezone would simply never take
    /// effect, silently, which is the exact failure the /etc check
    /// exists to end.
    ///
    /// What this pins is that the file is owned and therefore graded.
    /// Whether the merge behaves as described is ostree's business and
    /// needs a machine, not a test.
    #[test]
    fn a_declared_timezone_is_a_file_doctor_watches() {
        let declared = config(
            "schema_version = 1\n[system]\ndesktop = \"niri\"\n\
             timezone = \"America/Denver\"\nlocale = \"en_US.UTF-8\"\n",
        );
        let paths = etc_paths(&declared);
        assert!(paths.iter().any(|p| p == "/etc/localtime"), "{paths:?}");
        // locale rides the redirect path and always was owned
        assert!(paths.iter().any(|p| p == "/etc/locale.conf"), "{paths:?}");

        // Undeclared, the image writes neither, so there is nothing to
        // own and nothing to grade: timezone stays machine state.
        let bare = config("schema_version = 1\n[system]\ndesktop = \"niri\"\n");
        let paths = etc_paths(&bare);
        assert!(!paths.iter().any(|p| p == "/etc/localtime"), "{paths:?}");
        assert!(!paths.iter().any(|p| p == "/etc/locale.conf"), "{paths:?}");
    }

    /// Over the real generator, per desktop, because the value of this
    /// list is that nobody has to remember to update it.
    #[test]
    fn etc_paths_track_what_each_desktop_actually_writes() {
        let niri = etc_paths(&config("schema_version = 1\n[system]\ndesktop = \"niri\"\n"));
        assert!(niri.iter().any(|p| p == "/etc/niri/config.kdl"));
        assert!(niri.iter().any(|p| p == "/etc/greetd/config.toml"));

        // /etc/environment is where the COSMIC scanout vars live, and the
        // file whose hand-edited copy motivated the drift check.
        let cosmic = etc_paths(&config("schema_version = 1\n[system]\ndesktop = \"cosmic\"\n"));
        assert!(cosmic.iter().any(|p| p == "/etc/environment"));
        assert!(!cosmic.iter().any(|p| p == "/etc/niri/config.kdl"));

        // A headless image owns almost nothing in /etc, and must not
        // claim a desktop's files.
        let minimal = etc_paths(&config("schema_version = 1\n"));
        assert!(!minimal.iter().any(|p| p.contains("niri") || p.contains("greetd")));

        // Unpinned, the baked hostname is machine state (hostnamectl is
        // the sanctioned rename, not drift); declared, it's owned.
        assert!(!niri.iter().any(|p| p == "/etc/hostname"));
        let pinned = etc_paths(&config("schema_version = 1\n[system]\nhostname = \"workbench\"\n"));
        assert!(pinned.iter().any(|p| p == "/etc/hostname"));
    }

    /// A file picker that never appears, which is what niri shipped until
    /// this line existed. The GNOME portal backend advertises FileChooser
    /// and then delegates it to org.gnome.Nautilus, a package kuma does
    /// not install, so the interface has to be named to the gtk backend
    /// rather than left to `default=gnome;gtk;` to resolve.
    ///
    /// COSMIC is deliberately not given the same treatment: its own
    /// backend implements FileChooser, verified by reading cosmic.portal
    /// out of a built COSMIC image. Routing it to gtk there would replace
    /// a working native picker with a worse one.
    #[test]
    fn niri_routes_the_file_chooser_at_a_backend_that_implements_it() {
        let out = generate(&config("schema_version = 1\n[system]\ndesktop = \"niri\"\n"));
        let conf = "/etc/xdg-desktop-portal/niri-portals.conf";
        assert!(out.contains("org.freedesktop.impl.portal.FileChooser=gtk;"));
        assert!(out.contains(conf));

        // Derived from niri's own file, never a hand-copy: the winning
        // config file replaces rather than merges, so a copy would freeze
        // whatever the other defaults were the day it was written.
        assert!(out.contains("cat /usr/share/xdg-desktop-portal/niri-portals.conf"));

        // Both halves of the routing are guarded, because both can rot
        // without anything else failing: the file this derives from, and
        // the backend it hands the interface to.
        assert!(out.contains("grep -q '^\\[preferred\\]'"));
        assert!(out.contains(
            "grep -q 'org.freedesktop.impl.portal.FileChooser' \
             /usr/share/xdg-desktop-portal/portals/gtk.portal"
        ));

        // Written to /etc, so the drift check owns it like any other file
        // kuma has an opinion about.
        let owned = etc_paths(&config("schema_version = 1\n[system]\ndesktop = \"niri\"\n"));
        assert!(owned.iter().any(|p| p == conf));

        // COSMIC's own backend implements it; leave that alone.
        let cosmic = generate(&config("schema_version = 1\n[system]\ndesktop = \"cosmic\"\n"));
        assert!(!cosmic.contains("niri-portals.conf"));
        assert!(!cosmic.contains("FileChooser"));
    }

    /// The cheap tier of the smoke tests, and the only one that runs
    /// everywhere: every committed example must compile to an image that
    /// keeps the promises kuma makes about *all* images, plus the ones
    /// its own declaration asks for. scripts/smoke.sh takes it from here
    /// and actually builds and boots them; this catches the regressions
    /// that don't need a machine to find, on every `cargo test`.
    ///
    /// The point is coverage that grows by itself: a new example file is
    /// automatically held to the same floor, with nobody remembering to
    /// add a test for it.
    #[test]
    fn every_example_compiles_to_a_kuma_image() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/examples");
        let mut checked = 0;
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if !path.extension().is_some_and(|e| e == "toml")
                || crate::config::tests::is_local_declaration(&path)
            {
                continue;
            }
            let name = path.display().to_string();
            let text = std::fs::read_to_string(&path).unwrap();
            let parsed: Config = toml::from_str(&text).unwrap();
            let out = generate(&parsed);
            let at = |what: &str, ok: bool| assert!(ok, "{name}: {what}");

            // The floor, owed to every image no matter what it declares.
            at(
                "builds FROM the declared base",
                out.contains(&format!("FROM {}", parsed.base_ref())),
            );
            at("runs the bootc lint", out.contains("bootc container lint"));
            at("bakes greenboot", out.contains(&dnf_install("greenboot")));
            at("converges the boot counter", out.contains("kuma-boot-health-sync.service"));
            at("labels itself for GC", out.contains("LABEL io.kuma.image"));
            at("bakes its own declaration", out.contains("COPY kuma.toml /usr/lib/kuma/kuma.toml"));

            // What this particular declaration asked for.
            for pkg in &parsed.packages.rpm {
                at(&format!("installs declared rpm {pkg}"), out.contains(pkg));
            }
            for svc in &parsed.services.enable {
                at(&format!("enables declared service {svc}"), out.contains(svc));
            }
            if parsed.user.is_some() {
                at("converges the declared user", out.contains("kuma-user-sync.service"));
            }
            if !parsed.packages.flatpak.is_empty() {
                at("converges flatpaks", out.contains("kuma-flatpak-sync.service"));
            }

            // A desktop is a promise about what you see at boot, so the
            // greeter check and the keyring unlock ride with every one of
            // them and neither is per-desktop opt-in.
            if parsed.system.desktop != Desktop::None {
                at("guards the greeter in greenboot", out.contains("50-kuma-greeter.sh"));
                at("asserts the keyring PAM module", out.contains("pam_gnome_keyring.so"));
                at("ships identity", out.contains("fastfetch-logo.txt"));
            } else {
                at("stays headless", !out.contains("50-kuma-greeter.sh"));
            }
            checked += 1;
        }
        assert!(checked >= 3, "expected the committed examples, found {checked}");
    }

    #[test]
    fn cosmic_context_ships_identity_not_niri_glue() {
        let dir = tempfile::tempdir().unwrap();
        context("schema_version = 1\n[system]\ndesktop = \"cosmic\"\n", dir.path());
        // identity and kargs travel with every desktop
        assert!(dir.path().join("fastfetch-logo.txt").exists());
        assert!(dir.path().join("kargs-desktop.toml").exists());
        assert!(dir.path().join("kuma-wallpaper.jpg").exists());
        // the dock and background overrides are cosmic-only context
        assert!(dir.path().join("cosmic-favorites").exists());
        assert!(dir.path().join("cosmic-background").exists());
        // flatpak convergence ships even with an empty list
        assert!(dir.path().join("flatpaks").exists());
        // the niri glue stays home: COSMIC provides all of it natively
        assert!(!dir.path().join("greetd-config.toml").exists());
        assert!(!dir.path().join("kuma-osd").exists());
        assert!(!dir.path().join("mako.conf").exists());
        assert!(!dir.path().join("xsettingsd.conf").exists());
    }
}
