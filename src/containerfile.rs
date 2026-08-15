use crate::config::{Config, Desktop};
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
    "fuzzel",
    "waybar",
    "mako",
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
    "fontawesome-6-free-fonts",
    // the bluetooth glyph lives in the Brands face, not Free
    "fontawesome-6-brands-fonts",
    // base ships glibc-minimal-langpack only; without real locale data
    // en_US.UTF-8 fails to resolve and waybar's clock disables itself
    "glibc-langpack-en",
    // hardware enablement — the minimal base targets servers
    "NetworkManager-wifi",
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
    "cliphist",
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
    "wob",
    "libnotify",
    "wlsunset",
    "cups",
    "system-config-printer",
    // session essentials
    "wl-clipboard",
    "xsettingsd",
    "spice-vdagent",
    "xdg-user-dirs",
    "default-fonts-core-emoji",
    "mate-polkit",
    "swaybg",
    "swayidle",
    "swaylock",
    "firewalld",
    // niri's built-in screenshot UI covers the Print keys; grim+slurp are
    // the wlr-screencopy tools everything scriptable builds on
    "grim",
    "slurp",
    // Mod+Print: annotate before sharing (satty is COPR-only)
    "swappy",
    // the XF86Audio sed that makes room for kuma-osd also drops niri's
    // stock playerctl binds; kuma re-adds them, and though waybar already
    // pulls playerctl in, naming it keeps the binds from ever dangling
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

[ "$(findmnt -no FSTYPE -- "$target" 2>/dev/null || true)" = btrfs ] || exit 0
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
/// the same authority as adding one.
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
const FLATPAK_SYNC_SCRIPT: &str = r#"#!/usr/bin/bash
set -euo pipefail
declared=/usr/lib/kuma/flatpaks
state=/var/lib/kuma/flatpaks-installed
mkdir -p /var/lib/kuma
[ -f "$state" ] || : > "$state"
xargs -r -a "$declared" flatpak install --system --assumeyes --noninteractive --or-update flathub
while read -r app; do
    grep -qxF "$app" "$declared" \
        || flatpak uninstall --system --assumeyes --noninteractive "$app" || true
done < "$state"
cp "$declared" "$state"
flatpak update --system --assumeyes --noninteractive
flatpak uninstall --system --unused --assumeyes --noninteractive
"#;

const FLATHUB_URL: &str = "https://dl.flathub.org/repo/flathub.flatpakrepo";

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
if [ -f /var/lib/kuma/user ]; then
    . /var/lib/kuma/user
elif [ -f /usr/lib/kuma/user ]; then
    . /usr/lib/kuma/user
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
const FSTAB_SYNC_SERVICE: &str = r#"[Unit]
Description=Converge Anaconda's fstab root line for a composefs root
ConditionPathExists=/run/ostree-booted

[Service]
Type=oneshot
ExecStart=/usr/libexec/kuma-fstab-sync

[Install]
WantedBy=multi-user.target
"#;

/// Before the health check only for tidiness — the hook matters at the
/// NEXT grub run, so any point in this boot converges in time.
const BOOT_HEALTH_SYNC_SERVICE: &str = r#"[Unit]
Description=Converge the bootloader's boot-counter fallback hook
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
set -u
deadline=$(( SECONDS + 120 ))
until systemctl --quiet is-active display-manager.service; do
    if (( SECONDS >= deadline )); then
        echo "display-manager.service not active after 120s"
        exit 1
    fi
    sleep 3
done
"#;

/// Appended to niri's full default config (copied from the package) so the
/// stock keybindings survive; niri configs replace defaults entirely.
const NIRI_EXTRAS: &str = r##"

// GTK_THEME=Adwaita:dark is the empirically reliable dark switch for
// GTK3: the settings-layer name "Adwaita-dark" loads as a (nonexistent)
// directory theme and silently falls back to light.
environment {
    GTK_THEME "Adwaita:dark"
    XCURSOR_THEME "Adwaita"
    XCURSOR_SIZE "24"
}

// Kuma session services
spawn-at-startup "/usr/libexec/polkit-mate-authentication-agent-1"
spawn-at-startup "/usr/libexec/kuma-clipboard-bridge"
spawn-at-startup "/usr/libexec/kuma-xsettings"
spawn-at-startup "/usr/libexec/kuma-wob"
spawn-at-startup "blueman-applet"
// Automount removable media at the session level; notifies via mako.
spawn-at-startup "udiskie"
// Time-based night light: no location needed, unlike solar mode.
spawn-at-startup "wlsunset" "-S" "07:00" "-s" "20:00"
spawn-at-startup "waybar"
spawn-at-startup "swaybg" "-i" "/usr/share/backgrounds/kuma/kuma-wallpaper.jpg" "-m" "fill"
// Lock at 15 min, screen off a minute later (any input wakes it).
spawn-at-startup "swayidle" "-w" "timeout" "900" "swaylock -f -i /usr/share/backgrounds/kuma/kuma-wallpaper.jpg -s fill" "timeout" "960" "niri msg action power-off-monitors" "before-sleep" "swaylock -f -i /usr/share/backgrounds/kuma/kuma-wallpaper.jpg -s fill"
spawn-at-startup "/usr/libexec/kuma-battery-watch"
// Wayland clipboards can die with their window; cliphist keeps history
// (paste picker on Mod+Ctrl+V, spliced into the stock binds).
spawn-at-startup "wl-paste" "--watch" "cliphist" "store"

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

/// Dark by default. Apps learn the preference from the settings portal,
/// which reads org.gnome.desktop.interface from dconf; without it every
/// CSD titlebar and GTK app falls back to light. color-scheme covers
/// GTK4/libadwaita/portal clients, gtk-theme covers GTK3 apps that
/// predate it. A system db sets the default; user settings still win.
const DCONF_PROFILE: &str = "user-db:user\nsystem-db:local\n";
const DCONF_DARK: &str = r#"[org/gnome/desktop/interface]
color-scheme='prefer-dark'
gtk-theme='Adwaita'
"#;

/// Volume/brightness OSD: wob draws an overlay bar from levels written
/// to a FIFO (swayosd would be nicer but is COPR-only). kuma-osd is
/// bound to the media keys in place of niri's stock wpctl binds — it
/// makes the same adjustment, then feeds the resulting level to the bar.
const WOB_INI: &str = r#"[default]
anchor = bottom
margin = 48
height = 28
width = 360
border_size = 1
border_color = 7ee0a8ff
background_color = 101a28e6
bar_color = 7ee0a8ff
"#;

const WOB_LAUNCHER: &str = r#"#!/usr/bin/bash
set -euo pipefail
fifo="${XDG_RUNTIME_DIR}/kuma-wob.fifo"
rm -f "$fifo"; mkfifo "$fifo"
# tail keeps the fifo open between writes so wob doesn't exit
exec sh -c "tail -f \"$fifo\" | wob -c /usr/lib/kuma/wob.ini"
"#;

const OSD_SCRIPT: &str = r#"#!/usr/bin/bash
set -euo pipefail
fifo="${XDG_RUNTIME_DIR}/kuma-wob.fifo"
feed() { [ -p "$fifo" ] && echo "$1" > "$fifo" || true; }
vol() {
    v=$(wpctl get-volume @DEFAULT_AUDIO_SINK@)
    case "$v" in
        *MUTED*) feed 0 ;;
        *) feed "$(awk '{print int($2*100)}' <<<"$v")" ;;
    esac
}
case "$1" in
    volume-up)       wpctl set-volume -l 1.0 @DEFAULT_AUDIO_SINK@ 5%+; vol ;;
    volume-down)     wpctl set-volume @DEFAULT_AUDIO_SINK@ 5%-; vol ;;
    mute)            wpctl set-mute @DEFAULT_AUDIO_SINK@ toggle; vol ;;
    brightness-up)   brightnessctl -q set +5%; feed "$(brightnessctl -m | cut -d, -f4 | tr -d %)" ;;
    brightness-down) brightnessctl -q set 5%-; feed "$(brightnessctl -m | cut -d, -f4 | tr -d %)" ;;
esac
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

/// Battery warnings through mako. Polls sysfs — upower-notifier tools
/// (poweralertd) aren't in Fedora's repos. No battery (desktops, VMs)
/// means the loop just idles cheaply.
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

/// Media-key binds routed through kuma-osd, spliced INTO the stock
/// `binds {}` section during the merge (niri rejects a second binds
/// node) while the stock wpctl/brightnessctl lines are sed-stripped.
const NIRI_MEDIA_BINDS: &str = r#"    XF86AudioRaiseVolume allow-when-locked=true { spawn "/usr/libexec/kuma-osd" "volume-up"; }
    XF86AudioLowerVolume allow-when-locked=true { spawn "/usr/libexec/kuma-osd" "volume-down"; }
    XF86AudioMute allow-when-locked=true { spawn "/usr/libexec/kuma-osd" "mute"; }
    XF86AudioMicMute allow-when-locked=true { spawn "wpctl" "set-mute" "@DEFAULT_AUDIO_SOURCE@" "toggle"; }
    XF86MonBrightnessUp allow-when-locked=true { spawn "/usr/libexec/kuma-osd" "brightness-up"; }
    XF86MonBrightnessDown allow-when-locked=true { spawn "/usr/libexec/kuma-osd" "brightness-down"; }
    XF86AudioPlay allow-when-locked=true { spawn "playerctl" "play-pause"; }
    XF86AudioStop allow-when-locked=true { spawn "playerctl" "stop"; }
    XF86AudioNext allow-when-locked=true { spawn "playerctl" "next"; }
    XF86AudioPrev allow-when-locked=true { spawn "playerctl" "previous"; }
    Mod+Ctrl+V { spawn "sh" "-c" "cliphist list | fuzzel --dmenu | cliphist decode | wl-copy"; }
    Mod+Shift+N { spawn "makoctl" "mode" "-t" "do-not-disturb"; }
    Mod+Alt+R { spawn "/usr/libexec/kuma-record"; }
    Mod+Print { spawn "sh" "-c" "grim -g \"$(slurp)\" - | swappy -f -"; }
"#;

/// GTK theme settings travel two roads: Wayland-native apps read
/// gsettings (the dconf defaults cover those), but X11/XWayland GTK apps
/// only listen to an XSettings daemon — without one they render stock
/// light Adwaita. xsettingsd broadcasts the same dark values there.
const XSETTINGSD_CONF: &str = r#"Net/ThemeName "Adwaita"
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
gtk-theme-name = Adwaita
gtk-application-prefer-dark-theme = true
gtk-icon-theme-name = Adwaita
"#;

const GTK4_SETTINGS_INI: &str = r#"[Settings]
gtk-application-prefer-dark-theme = true
"#;

const XSETTINGS_LAUNCHER: &str = r#"#!/usr/bin/bash
set -euo pipefail
for _ in $(seq 60); do
    [ -n "${DISPLAY:-}" ] && break
    DISPLAY=$(systemctl --user show-environment 2>/dev/null | sed -n 's/^DISPLAY=//p')
    [ -n "$DISPLAY" ] && export DISPLAY && break
    sleep 0.5
done
[ -n "${DISPLAY:-}" ] || exit 0
exec xsettingsd -c /usr/lib/kuma/xsettingsd.conf
"#;

/// Session half of host<->guest clipboard in `kuma vm`. spice-vdagent's
/// clipboard side is X11, so under niri it rides the xwayland-satellite
/// bridge — wait briefly for DISPLAY to appear in the session
/// environment. No vdagent port (real hardware) means exit quietly.
const CLIPBOARD_BRIDGE: &str = r#"#!/usr/bin/bash
set -euo pipefail
[ -e /dev/virtio-ports/com.redhat.spice.0 ] || exit 0
for _ in $(seq 60); do
    [ -n "${DISPLAY:-}" ] && break
    DISPLAY=$(systemctl --user show-environment 2>/dev/null | sed -n 's/^DISPLAY=//p')
    [ -n "$DISPLAY" ] && export DISPLAY && break
    sleep 0.5
done
[ -n "${DISPLAY:-}" ] || exit 0
exec spice-vdagent -x
"#;

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
/// ~/.config/fastfetch still wins, same as waybar and fuzzel.
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

/// Theme files for the curated desktop. The navy base is the wallpaper's own
/// darkest tones; the accent is picked to sit against it, not sampled from it,
/// so replacing the wallpaper does not oblige a retheme.
/// All system-wide (never /etc/skel): skel only reaches homes created after
/// the image ships, so it strands existing users on stale copies — image
/// updates must retheme every account. User dotfiles still win everywhere:
/// waybar and fuzzel search /etc/xdg after ~/.config, kitty merges
/// /etc/xdg beneath the user's file (so a one-key override keeps the rest
/// of this theme), and mako (no system path at all) goes through a
/// launcher that prefers the user's config.
const WALLPAPER: &[u8] = include_bytes!("../assets/kuma-wallpaper.jpg");
const WAYBAR_CONFIG: &str = include_str!("../assets/waybar.jsonc");
const WAYBAR_STYLE: &str = include_str!("../assets/waybar.css");
const FUZZEL_CONFIG: &str = include_str!("../assets/fuzzel.ini");
const MAKO_CONFIG: &str = include_str!("../assets/mako.conf");
const KITTY_CONFIG: &str = include_str!("../assets/kitty.conf");

/// mako is dbus-activated (org.freedesktop.Notifications), so this wrapper
/// is wired in via its dbus service file rather than spawn-at-startup.
/// The service file names SystemdService=mako.service, and under a systemd
/// user session THAT is what actually runs (Exec= is only the fallback) —
/// so the user unit gets a drop-in pointing at the wrapper too.
const MAKO_LAUNCHER: &str = r#"#!/usr/bin/bash
conf="${XDG_CONFIG_HOME:-$HOME/.config}/mako/config"
if [ -f "$conf" ]; then
    exec mako
fi
exec mako --config /usr/lib/kuma/mako.conf
"#;

/// Empty ExecStart= first: it clears the unit's own ExecStart, which a
/// drop-in otherwise appends to (two ExecStarts is a hard unit error).
const MAKO_DROPIN: &str = r#"[Service]
ExecStart=
ExecStart=/usr/libexec/kuma-mako
"#;

/// Rebrand the OS identity: Kuma, not Fedora. ID_LIKE=fedora keeps tools
/// that sniff os-release (toolbox, distrobox, dnf COPR, …) working. Runs
/// last so every dnf layer before it still sees stock Fedora metadata.
///
/// Releases carry bear names (species or fiction), one per Fedora base,
/// keyed by VERSION_ID below. PRETTY_NAME drops the number — kuma has no
/// version of its own, just a continuously rebuilt base — while VERSION_ID
/// stays Fedora's so toolbox/dnf/bib keep resolving. An unlisted base
/// falls back to plain "Kuma" and Fedora's own VERSION string.
///
const BRANDING: &str = r#"
RUN . /usr/lib/os-release \
    && case "${VERSION_ID}" in \
        44) CODENAME="Beorn" ;; \
        *) CODENAME="" ;; \
    esac \
    && sed -i \
        -e 's|^NAME=.*|NAME="Kuma"|' \
        -e "s|^PRETTY_NAME=.*|PRETTY_NAME=\"Kuma${CODENAME:+ ($CODENAME)}\"|" \
        -e 's|^ID=.*|ID=kuma|' \
        -e 's|^DEFAULT_HOSTNAME=.*|DEFAULT_HOSTNAME="kuma"|' \
        -e 's|^ANSI_COLOR=.*|ANSI_COLOR="0;38;2;126;224;168"|' \
        /usr/lib/os-release \
    && if [ -n "$CODENAME" ]; then sed -i \
        -e "s|^VERSION=.*|VERSION=\"${VERSION_ID} ($CODENAME)\"|" \
        -e "s|^VERSION_CODENAME=.*|VERSION_CODENAME=$(printf %s "$CODENAME" | tr '[:upper:]' '[:lower:]')|" \
        /usr/lib/os-release; fi \
    && { grep -q '^ID_LIKE=' /usr/lib/os-release || echo 'ID_LIKE="fedora"' >> /usr/lib/os-release; } \
    && { [ ! -f /usr/lib/fedora-release ] || echo "Kuma release ${VERSION_ID}${CODENAME:+ ($CODENAME)}" > /usr/lib/fedora-release; }
"#;

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
/// A build writes into /etc exactly two ways, and both are unambiguous:
/// a COPY whose destination is under /etc, and a shell redirect (`>`,
/// `>>`) into one. Reading an /etc file is not owning it, and the
/// difference matters: the keyring assert greps /etc/pam.d/greetd and
/// `niri validate` reads /etc/niri/config.kdl, but only one of those two
/// files is kuma's to have an opinion about. Redirects separate them for
/// free, since a read has none.
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
        // niri Recommends alacritty, which would ride in past the package
        // list as a weak dep; Kuma's terminal is kitty.
        out.push_str(&dnf_install(&format!("--exclude=alacritty {}", NIRI_PACKAGES.join(" "))));
        out.push_str(&mesa_freeworld());
        out.push_str("COPY greetd-config.toml /etc/greetd/config.toml\n");
        out.push_str("COPY kargs-desktop.toml /usr/lib/bootc/kargs.d/10-kuma-desktop.toml\n");
        out.push_str("COPY niri-extras.kdl /usr/lib/kuma/niri-extras.kdl\n");
        out.push_str("COPY kuma-wallpaper.jpg /usr/share/backgrounds/kuma/kuma-wallpaper.jpg\n");
        out.push_str("COPY waybar-config.jsonc /etc/xdg/waybar/config.jsonc\n");
        out.push_str("COPY waybar-style.css /etc/xdg/waybar/style.css\n");
        out.push_str("COPY fuzzel.ini /etc/xdg/fuzzel/fuzzel.ini\n");
        out.push_str("COPY mako.conf /usr/lib/kuma/mako.conf\n");
        out.push_str("COPY --chmod=755 kuma-mako /usr/libexec/kuma-mako\n");
        out.push_str("COPY mako-dropin.conf /usr/lib/systemd/user/mako.service.d/kuma.conf\n");
        // grep first: if a mako update moves or rewords the service file,
        // fail the build instead of silently shipping unthemed notifications
        out.push_str(
            "RUN grep -qx 'Exec=/usr/bin/mako' /usr/share/dbus-1/services/fr.emersion.mako.service \\\n    && sed -i 's|^Exec=/usr/bin/mako$|Exec=/usr/libexec/kuma-mako|' /usr/share/dbus-1/services/fr.emersion.mako.service\n",
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
        out.push_str("COPY --chmod=755 kuma-clipboard-bridge /usr/libexec/kuma-clipboard-bridge\n");
        out.push_str("COPY fastfetch-config.jsonc /etc/xdg/fastfetch/config.jsonc\n");
        out.push_str("COPY fastfetch-logo.txt /usr/lib/kuma/fastfetch-logo.txt\n");
        out.push_str("COPY --chmod=755 kuma-xsettings /usr/libexec/kuma-xsettings\n");
        out.push_str("COPY xsettingsd.conf /usr/lib/kuma/xsettingsd.conf\n");
        out.push_str("COPY niri-binds.kdl /usr/lib/kuma/niri-binds.kdl\n");
        out.push_str("COPY --chmod=755 kuma-record /usr/libexec/kuma-record\n");
        out.push_str("COPY --chmod=755 kuma-battery-watch /usr/libexec/kuma-battery-watch\n");
        out.push_str("COPY --chmod=755 kuma-wob /usr/libexec/kuma-wob\n");
        out.push_str("COPY --chmod=755 kuma-osd /usr/libexec/kuma-osd\n");
        out.push_str("COPY wob.ini /usr/lib/kuma/wob.ini\n");
        out.push_str("COPY gtk3-settings.ini /etc/gtk-3.0/settings.ini\n");
        out.push_str("COPY gtk4-settings.ini /etc/gtk-4.0/settings.ini\n");
        out.push_str("COPY mimeapps.list /etc/xdg/mimeapps.list\n");
        out.push_str("COPY dconf-profile /etc/dconf/profile/user\n");
        out.push_str(&keyring_pam("greetd"));
        out.push_str("COPY dconf-kuma-dark /etc/dconf/db/local.d/10-kuma-dark\n");
        out.push_str("RUN dconf update\n");
        // The packaged default config is complete (all keybindings); Kuma's
        // config is that plus our session extras, validated at build time.
        // Fedora's default config already spawns waybar — drop that line (and
        // its comment) or the bar starts twice; Kuma's extras spawn it.
        // Upstream's terminal is alacritty; Kuma ships kitty, so rewrite the
        // spawn (and its hotkey-overlay title). grep first: if a niri update
        // stops naming alacritty, fail the build instead of silently
        // shipping a Mod+T that spawns a terminal the image doesn't have.
        out.push_str(
            "RUN grep -q '\"alacritty\"' /usr/share/doc/niri/default-config.kdl \\\n    && mkdir -p /etc/niri \\\n    && sed -e 's/alacritty/kitty/g' -e '/starts waybar/d' -e '/^spawn-at-startup \"waybar\"$/d' -e '/XF86Audio/d' -e '/XF86MonBrightness/d' -e '/^binds {/r /usr/lib/kuma/niri-binds.kdl' /usr/share/doc/niri/default-config.kdl > /etc/niri/config.kdl \\\n    && cat /usr/lib/kuma/niri-extras.kdl >> /etc/niri/config.kdl \\\n    && niri validate --config /etc/niri/config.kdl\n",
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
    }
    out.push_str("COPY --chmod=755 kuma-boot-health-sync /usr/libexec/kuma-boot-health-sync\n");
    out.push_str(
        "COPY kuma-boot-health-sync.service /usr/lib/systemd/system/kuma-boot-health-sync.service\n",
    );
    out.push_str("COPY --chmod=755 kuma-fstab-sync /usr/libexec/kuma-fstab-sync\n");
    out.push_str("COPY kuma-fstab-sync.service /usr/lib/systemd/system/kuma-fstab-sync.service\n");
    out.push_str(
        "RUN systemctl enable greenboot-healthcheck.service greenboot-set-rollback-trigger.service greenboot-success.target kuma-boot-health-sync.service kuma-fstab-sync.service\n",
    );

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

    out.push_str(BRANDING);

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
    let hostname = config.system.hostname.as_deref().unwrap_or("kuma");
    std::fs::write(dir.join("hostname"), format!("{hostname}\n"))?;
    std::fs::write(dir.join("kuma-vm-timezone"), VM_TZ_SCRIPT)?;
    std::fs::write(dir.join("kuma-vm-timezone.service"), VM_TZ_SERVICE)?;
    std::fs::write(dir.join("kuma-boot-health-sync"), BOOT_HEALTH_SYNC_SCRIPT)?;
    std::fs::write(dir.join("kuma-boot-health-sync.service"), BOOT_HEALTH_SYNC_SERVICE)?;
    std::fs::write(dir.join("kuma-fstab-sync"), FSTAB_SYNC_SCRIPT)?;
    std::fs::write(dir.join("kuma-fstab-sync.service"), FSTAB_SYNC_SERVICE)?;
    // Identity, wallpaper, and kargs ship with every desktop; the rest
    // of the niri block is glue COSMIC provides natively.
    if config.system.desktop != Desktop::None {
        std::fs::write(dir.join("kargs-desktop.toml"), DESKTOP_KARGS)?;
        std::fs::write(dir.join("fastfetch-config.jsonc"), FASTFETCH_CONFIG)?;
        std::fs::write(dir.join("fastfetch-logo.txt"), FASTFETCH_LOGO)?;
        std::fs::write(dir.join("kuma-wallpaper.jpg"), WALLPAPER)?;
        std::fs::write(dir.join("kuma-greeter-check"), GREETER_CHECK)?;
    }
    if config.system.desktop == Desktop::Cosmic {
        std::fs::write(dir.join("cosmic-favorites"), COSMIC_FAVORITES)?;
        std::fs::write(dir.join("cosmic-background"), COSMIC_BACKGROUND)?;
    }
    if config.system.desktop == Desktop::Niri {
        std::fs::write(dir.join("greetd-config.toml"), greetd_config(config))?;
        std::fs::write(dir.join("niri-extras.kdl"), NIRI_EXTRAS)?;
        std::fs::write(dir.join("waybar-config.jsonc"), WAYBAR_CONFIG)?;
        std::fs::write(dir.join("waybar-style.css"), WAYBAR_STYLE)?;
        std::fs::write(dir.join("fuzzel.ini"), FUZZEL_CONFIG)?;
        std::fs::write(dir.join("mako.conf"), MAKO_CONFIG)?;
        std::fs::write(dir.join("kuma-mako"), MAKO_LAUNCHER)?;
        std::fs::write(dir.join("mako-dropin.conf"), MAKO_DROPIN)?;
        std::fs::write(dir.join("kitty.conf"), KITTY_CONFIG)?;
        std::fs::write(dir.join("kuma-clipboard-bridge"), CLIPBOARD_BRIDGE)?;
        std::fs::write(dir.join("kuma-xsettings"), XSETTINGS_LAUNCHER)?;
        std::fs::write(dir.join("xsettingsd.conf"), XSETTINGSD_CONF)?;
        std::fs::write(dir.join("niri-binds.kdl"), NIRI_MEDIA_BINDS)?;
        std::fs::write(dir.join("mimeapps.list"), MIMEAPPS)?;
        std::fs::write(dir.join("kuma-record"), RECORD_SCRIPT)?;
        std::fs::write(dir.join("kuma-battery-watch"), BATTERY_WATCH)?;
        std::fs::write(dir.join("kuma-wob"), WOB_LAUNCHER)?;
        std::fs::write(dir.join("kuma-osd"), OSD_SCRIPT)?;
        std::fs::write(dir.join("wob.ini"), WOB_INI)?;
        std::fs::write(dir.join("gtk3-settings.ini"), GTK3_SETTINGS_INI)?;
        std::fs::write(dir.join("gtk4-settings.ini"), GTK4_SETTINGS_INI)?;
        std::fs::write(dir.join("dconf-profile"), DCONF_PROFILE)?;
        std::fs::write(dir.join("dconf-kuma-dark"), DCONF_DARK)?;
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
    }
    if config.snapshots.enable {
        std::fs::write(dir.join("kuma-snapshot"), snapshot_script(config))?;
        std::fs::write(dir.join("kuma-snapshot.service"), SNAPSHOT_SERVICE)?;
        std::fs::write(
            dir.join("kuma-snapshot.timer"),
            snapshot_timer(&config.snapshots.interval),
        )?;
    }
    // Unconditional, like the Containerfile lines that copy them: the
    // converger has to be present in an image that declares no account,
    // because that is the image an installer writes /var/lib/kuma/user onto.
    std::fs::write(dir.join("kuma-user-sync"), USER_SYNC_SCRIPT)?;
    std::fs::write(dir.join("kuma-user-sync.service"), USER_SYNC_SERVICE)?;
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
        // Boot health is the one dnf layer even a minimal image carries:
        // the never-worse-than-before promise is not opt-in.
        assert_eq!(out.matches("dnf -y install").count(), 1);
        assert!(out.contains(&dnf_install("greenboot")));
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
        assert!(out.contains("COPY waybar-config.jsonc /etc/xdg/waybar/config.jsonc"));
        assert!(out.contains("COPY waybar-style.css /etc/xdg/waybar/style.css"));
        // system-wide, never /etc/skel — skel strands existing homes on
        // stale copies (the fuzzel-DPI lesson)
        assert!(!out.contains("/etc/skel"));
        assert!(out.contains("COPY fuzzel.ini /etc/xdg/fuzzel/fuzzel.ini"));
        assert!(out.contains("COPY mako.conf /usr/lib/kuma/mako.conf"));
        assert!(out.contains("Exec=/usr/libexec/kuma-mako"));
        // systemd user sessions activate via SystemdService, not Exec —
        // without the drop-in the wrapper never runs where it matters
        assert!(out.contains("/usr/lib/systemd/user/mako.service.d/kuma.conf"));
        assert!(out.contains("COPY kitty.conf /etc/xdg/kitty/kitty.conf"));
        // an unparseable theme must fail the build, not ship unthemed —
        // and unknown keys only ever reach stderr, so both halves matter
        assert!(out.contains("kitty +runpy"));
        assert!(out.contains("accumulate_bad_lines=bad"));
        assert!(out.contains("grep -q 'unknown config key' /tmp/kitty.err"));
        // upstream niri spawns alacritty; the image ships kitty, so the sed
        // must rewrite the bind, and the grep guard must keep it honest
        assert!(out.contains("grep -q '\"alacritty\"' /usr/share/doc/niri/default-config.kdl"));
        assert!(out.contains("sed -e 's/alacritty/kitty/g'"));
        // niri Recommends alacritty; without the exclude it ships anyway
        assert!(out.contains("--exclude=alacritty"));
        assert!(out.contains("COPY dconf-profile /etc/dconf/profile/user"));
        assert!(out.contains("COPY dconf-kuma-dark /etc/dconf/db/local.d/10-kuma-dark"));
        assert!(out.contains("RUN dconf update"));
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
        // Fedora's default config spawns waybar; the merge must drop it so
        // only the Kuma extras spawn remains (two spawns = two bars).
        assert!(out.contains("-e '/^spawn-at-startup \"waybar\"$/d'"));
        assert_eq!(NIRI_EXTRAS.matches("spawn-at-startup \"waybar\"").count(), 1);
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
        assert!(extras.contains("/usr/share/backgrounds/kuma/kuma-wallpaper.jpg"));
        assert!(extras.contains("spawn-at-startup \"waybar\""));
        assert!(extras.contains("kuma-clipboard-bridge"));
        assert!(dir.path().join("kuma-clipboard-bridge").exists());
        let greetd = std::fs::read_to_string(dir.path().join("greetd-config.toml")).unwrap();
        assert!(greetd.contains("Welcome to Kuma"));
        assert!(dir.path().join("waybar-config.jsonc").exists());
        assert!(dir.path().join("waybar-style.css").exists());
        assert!(dir.path().join("fuzzel.ini").exists());
        assert!(dir.path().join("mako.conf").exists());
        assert!(dir.path().join("kitty.conf").exists());
        let ff = std::fs::read_to_string(dir.path().join("fastfetch-config.jsonc")).unwrap();
        assert!(ff.contains("/usr/lib/kuma/fastfetch-logo.txt"));
        assert!(dir.path().join("fastfetch-logo.txt").exists());
    }

    #[test]
    fn branding_names_the_release() {
        let out = generate(&config("schema_version = 1\n"));
        // 44 is Beorn; an unlisted base must fall back to plain "Kuma".
        assert!(out.contains(r#"44) CODENAME="Beorn""#));
        assert!(out.contains(r#"*) CODENAME="""#));
        assert!(out.contains(r#"PRETTY_NAME=\"Kuma${CODENAME:+ ($CODENAME)}\""#));
    }

    #[test]
    fn no_desktop_means_no_desktop_layer() {
        let out = generate(&config("schema_version = 1"));
        assert!(!out.contains("greetd"));
        assert!(!out.contains("graphical.target"));
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

        let dir = tempfile::tempdir().unwrap();
        context(
            "schema_version = 1\n[snapshots]\nenable = true\ninterval = \"daily\"\n",
            dir.path(),
        );
        let timer = std::fs::read_to_string(dir.path().join("kuma-snapshot.timer")).unwrap();
        assert!(timer.contains("OnCalendar=daily"));
        assert!(timer.contains("Persistent=true"), "a laptop asleep at the hour still snapshots");
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
        assert!(USER_SYNC_SCRIPT.contains("elif [ -f /usr/lib/kuma/user ]"));
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
        let var = USER_SYNC_SCRIPT.find("/var/lib/kuma/user").unwrap();
        let usr = USER_SYNC_SCRIPT.find("/usr/lib/kuma/user").unwrap();
        assert!(var < usr, "the machine's own file has to be tried first");
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

    #[test]
    fn session_polish_ships_osd_and_battery_watch() {
        assert!(NIRI_EXTRAS.contains("power-off-monitors"));
        assert!(NIRI_EXTRAS.contains("kuma-battery-watch"));
        assert!(NIRI_EXTRAS.contains("kuma-wob"));
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
        const EXPLAINED_NIRI: &[&str] =
            &["fontawesome-6-brands-fonts", "google-noto-sans-cjk-vf-fonts"];

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

    /// fuzzel runs `Terminal=true` desktop entries through whatever
    /// `terminal=` names, so it has to be a binary the image ships. Nothing
    /// fails at build time if it isn't: the symptom is a launcher entry
    /// that silently does nothing. This read `foot` until kuma switched
    /// terminals, and the package list was the obvious edit to remember.
    #[test]
    fn fuzzels_terminal_is_one_the_desktop_installs() {
        let terminal = FUZZEL_CONFIG
            .lines()
            .find_map(|line| line.strip_prefix("terminal="))
            .expect("fuzzel names a terminal")
            .split_whitespace()
            .next()
            .expect("the terminal is not blank");
        assert!(
            NIRI_PACKAGES.contains(&terminal),
            "fuzzel launches terminal apps with {terminal}, which the niri image doesn't install"
        );
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
        let enabled: Vec<&str> = out
            .lines()
            .find(|line| line.contains("systemctl enable "))
            .expect("the desktop arm enables units")
            .split_whitespace()
            .filter(|word| word.ends_with(".service"))
            .collect();
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
        assert!(NIRI_MEDIA_BINDS.contains("cliphist list"));
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
        assert!(NIRI_MEDIA_BINDS.contains("do-not-disturb"));
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
        // non-interactive installs fail
        assert!(script.contains("--or-update flathub"));
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
