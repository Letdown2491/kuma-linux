use crate::config::{Config, Desktop};
use anyhow::Result;
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
    "alacritty",
    "pipewire",
    "pipewire-pulseaudio",
    "wireplumber",
    "xdg-desktop-portal-gtk",
    "xdg-desktop-portal-gnome",
    "dconf",
    "gnome-keyring",
    "polkit",
    "mesa-dri-drivers",
    "default-fonts-core-sans",
    "default-fonts-core-mono",
    "fontawesome-6-free-fonts",
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
    "gvfs",
    "gvfs-mtp",
    // (nwg-look would fit here for GTK theme tweaks, but it's COPR-only)
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
];

const GREETD_CONFIG: &str = r#"[terminal]
vt = 1

[default_session]
command = "tuigreet --time --remember --greeting 'Welcome to Kuma' --cmd niri-session"
user = "greetd"
"#;

/// Desktop kernel args. The minimal base ships no auditd, so kernel audit
/// records spray onto the console; `quiet` keeps the console clean without
/// disabling auditing (records still reach the journal).
const DESKTOP_KARGS: &str = "kargs = [\"quiet\"]\n";

/// Declared flatpaks are baked into the image as a list; this oneshot
/// converges the machine to it on boot. The declaration is atomic image
/// content — only the app installs are runtime state.
const FLATPAK_SYNC_SERVICE: &str = r#"[Unit]
Description=Converge Flatpak applications to the declared list
Wants=network-online.target
After=network-online.target

[Service]
Type=oneshot
ExecStart=/usr/libexec/kuma-flatpak-sync

[Install]
WantedBy=multi-user.target
"#;

/// Convergence, not just installation: system apps missing from the
/// declaration are removed, so deleting a line in kuma.toml has the same
/// authority as adding one. User installs (`flatpak install --user`) are
/// personal machine state and are never touched.
const FLATPAK_SYNC_SCRIPT: &str = r#"#!/usr/bin/bash
set -euo pipefail
declared=/usr/lib/kuma/flatpaks
xargs -r -a "$declared" flatpak install --system --assumeyes --noninteractive --or-update flathub
flatpak list --system --app --columns=application | while read -r app; do
    grep -qxF "$app" "$declared" \
        || flatpak uninstall --system --assumeyes --noninteractive "$app"
done
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
const USER_SYNC_SCRIPT: &str = r#"#!/usr/bin/bash
set -euo pipefail
. /usr/lib/kuma/user
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

/// Appended to niri's full default config (copied from the package) so the
/// stock keybindings survive; niri configs replace defaults entirely.
const NIRI_EXTRAS: &str = r##"

// Kuma session services
spawn-at-startup "/usr/libexec/polkit-mate-authentication-agent-1"
spawn-at-startup "/usr/libexec/kuma-clipboard-bridge"
spawn-at-startup "/usr/libexec/kuma-xsettings"
spawn-at-startup "blueman-applet"
// Time-based night light: no location needed, unlike solar mode.
spawn-at-startup "wlsunset" "-S" "07:00" "-s" "20:00"
spawn-at-startup "waybar"
spawn-at-startup "swaybg" "-i" "/usr/share/backgrounds/kuma/kuma-wallpaper.png" "-m" "fill"
spawn-at-startup "swayidle" "-w" "timeout" "900" "swaylock -f -i /usr/share/backgrounds/kuma/kuma-wallpaper.png -s fill" "before-sleep" "swaylock -f -i /usr/share/backgrounds/kuma/kuma-wallpaper.png -s fill"

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
"##;

/// Dark by default. Apps learn the preference from the settings portal,
/// which reads org.gnome.desktop.interface from dconf; without it every
/// CSD titlebar and GTK app falls back to light. color-scheme covers
/// GTK4/libadwaita/portal clients, gtk-theme covers GTK3 apps that
/// predate it. A system db sets the default; user settings still win.
const DCONF_PROFILE: &str = "user-db:user\nsystem-db:local\n";
const DCONF_DARK: &str = r#"[org/gnome/desktop/interface]
color-scheme='prefer-dark'
gtk-theme='Adwaita-dark'
"#;

/// GTK theme settings travel two roads: Wayland-native apps read
/// gsettings (the dconf defaults cover those), but X11/XWayland GTK apps
/// only listen to an XSettings daemon — without one they render stock
/// light Adwaita. xsettingsd broadcasts the same dark values there.
const XSETTINGSD_CONF: &str = r#"Net/ThemeName "Adwaita-dark"
Net/IconThemeName "Adwaita"
Gtk/ApplicationPreferDarkTheme 1
Gtk/CursorThemeName "Adwaita"
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

/// Theme files for the curated desktop, drawn from the Kuma wallpaper palette.
/// Waybar reads /etc/xdg system-wide; the rest are seeded via /etc/skel so
/// users start themed but can override freely in their own dotfiles.
const WALLPAPER: &[u8] = include_bytes!("../assets/kuma-wallpaper.png");
const WAYBAR_CONFIG: &str = include_str!("../assets/waybar.jsonc");
const WAYBAR_STYLE: &str = include_str!("../assets/waybar.css");
const FUZZEL_CONFIG: &str = include_str!("../assets/fuzzel.ini");
const MAKO_CONFIG: &str = include_str!("../assets/mako.conf");
const ALACRITTY_CONFIG: &str = include_str!("../assets/alacritty.toml");

/// Rebrand the OS identity: Kuma, not Fedora. ID_LIKE=fedora keeps tools
/// that sniff os-release (toolbox, distrobox, dnf COPR, …) working. Runs
/// last so every dnf layer before it still sees stock Fedora metadata.
const BRANDING: &str = r#"
RUN . /usr/lib/os-release \
    && sed -i \
        -e 's|^NAME=.*|NAME="Kuma"|' \
        -e "s|^PRETTY_NAME=.*|PRETTY_NAME=\"Kuma ${VERSION_ID}\"|" \
        -e 's|^ID=.*|ID=kuma|' \
        -e 's|^DEFAULT_HOSTNAME=.*|DEFAULT_HOSTNAME="kuma"|' \
        -e 's|^ANSI_COLOR=.*|ANSI_COLOR="0;38;2;126;224;168"|' \
        /usr/lib/os-release \
    && { grep -q '^ID_LIKE=' /usr/lib/os-release || echo 'ID_LIKE="fedora"' >> /usr/lib/os-release; } \
    && { [ ! -f /usr/lib/fedora-release ] || echo "Kuma release ${VERSION_ID}" > /usr/lib/fedora-release; }
"#;

/// Homebrew lives in /home/linuxbrew — machine-local mutable state, so it
/// can't be image content. First boot installs it; the tarball is the
/// official "untar anywhere" method. Prefix owned by uid 1000, brew's
/// single-user model (same choice Bluefin makes).
const BREW_SETUP_SCRIPT: &str = r#"#!/usr/bin/bash
set -euo pipefail
prefix=/home/linuxbrew/.linuxbrew
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
ConditionPathExists=!/home/linuxbrew/.linuxbrew/bin/brew

[Service]
Type=oneshot
ExecStart=/usr/libexec/kuma-brew-setup

[Install]
WantedBy=multi-user.target
"#;

const BREW_PROFILE_SH: &str = r#"[ -x /home/linuxbrew/.linuxbrew/bin/brew ] \
    && eval "$(/home/linuxbrew/.linuxbrew/bin/brew shellenv)"
"#;

const BREW_PROFILE_FISH: &str = r#"if test -x /home/linuxbrew/.linuxbrew/bin/brew
    /home/linuxbrew/.linuxbrew/bin/brew shellenv | source
end
"#;

/// Compile a kuma config into a Containerfile for a bootc image build.
pub fn generate(config: &Config) -> String {
    let mut out = String::new();
    out.push_str("# Generated by kuma — edit kuma.toml instead.\n");
    out.push_str(&format!("FROM {}\n", config.system.base));

    // Desktop layer first: it is large and changes rarely, so keeping it
    // before the user's packages preserves the build cache across edits.
    if config.system.desktop == Desktop::Niri {
        out.push_str(&format!(
            "\nRUN dnf -y install {} && dnf clean all\n",
            NIRI_PACKAGES.join(" ")
        ));
        out.push_str("COPY greetd-config.toml /etc/greetd/config.toml\n");
        out.push_str("COPY kargs-desktop.toml /usr/lib/bootc/kargs.d/10-kuma-desktop.toml\n");
        out.push_str("COPY niri-extras.kdl /usr/lib/kuma/niri-extras.kdl\n");
        out.push_str("COPY kuma-wallpaper.png /usr/share/backgrounds/kuma/kuma-wallpaper.png\n");
        out.push_str("COPY waybar-config.jsonc /etc/xdg/waybar/config.jsonc\n");
        out.push_str("COPY waybar-style.css /etc/xdg/waybar/style.css\n");
        out.push_str("COPY fuzzel.ini /etc/skel/.config/fuzzel/fuzzel.ini\n");
        out.push_str("COPY mako.conf /etc/skel/.config/mako/config\n");
        out.push_str("COPY alacritty.toml /etc/skel/.config/alacritty/alacritty.toml\n");
        out.push_str("COPY --chmod=755 kuma-clipboard-bridge /usr/libexec/kuma-clipboard-bridge\n");
        out.push_str("COPY --chmod=755 kuma-xsettings /usr/libexec/kuma-xsettings\n");
        out.push_str("COPY xsettingsd.conf /usr/lib/kuma/xsettingsd.conf\n");
        out.push_str("COPY dconf-profile /etc/dconf/profile/user\n");
        out.push_str("COPY dconf-kuma-dark /etc/dconf/db/local.d/10-kuma-dark\n");
        out.push_str("RUN dconf update\n");
        // The packaged default config is complete (all keybindings); Kuma's
        // config is that plus our session extras, validated at build time.
        // Fedora's default config already spawns waybar — drop that line (and
        // its comment) or the bar starts twice; Kuma's extras spawn it.
        out.push_str(
            "RUN mkdir -p /etc/niri \\\n    && sed -e '/starts waybar/d' -e '/^spawn-at-startup \"waybar\"$/d' /usr/share/doc/niri/default-config.kdl > /etc/niri/config.kdl \\\n    && cat /usr/lib/kuma/niri-extras.kdl >> /etc/niri/config.kdl \\\n    && niri validate --config /etc/niri/config.kdl\n",
        );
        out.push_str(
            "RUN systemctl set-default graphical.target && systemctl enable greetd.service firewalld.service power-profiles-daemon.service bluetooth.service cups.service\n",
        );
    }

    let wants_flatpak = config.system.desktop == Desktop::Niri
        || !config.packages.flatpak.is_empty();
    if wants_flatpak {
        if config.system.desktop == Desktop::None {
            out.push_str("\nRUN dnf -y install flatpak && dnf clean all\n");
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
        out.push_str("RUN systemctl enable kuma-flatpak-sync.service\n");
    }

    if config.system.brew {
        // git-core: brew needs git at runtime to update itself
        out.push_str("\nRUN dnf -y install git-core && dnf clean all\n");
        out.push_str("COPY --chmod=755 kuma-brew-setup /usr/libexec/kuma-brew-setup\n");
        out.push_str(
            "COPY kuma-brew-setup.service /usr/lib/systemd/system/kuma-brew-setup.service\n",
        );
        out.push_str("COPY brew-profile.sh /etc/profile.d/kuma-brew.sh\n");
        out.push_str("COPY brew-profile.fish /etc/fish/conf.d/kuma-brew.fish\n");
        out.push_str("RUN systemctl enable kuma-brew-setup.service\n");
    }

    if !config.packages.rpm.is_empty() {
        out.push_str(&format!(
            "\nRUN dnf -y install {} && dnf clean all\n",
            config.packages.rpm.join(" ")
        ));
    }

    let services: Vec<String> = config
        .services
        .enable
        .iter()
        .map(|s| format!("systemctl enable {s}"))
        .chain(
            config
                .services
                .disable
                .iter()
                .map(|s| format!("systemctl disable {s}")),
        )
        .collect();
    if !services.is_empty() {
        out.push_str(&format!("\nRUN {}\n", services.join(" && ")));
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
    if let Some(user) = &config.user {
        out.push_str("\nCOPY kuma-user /usr/lib/kuma/user\n");
        out.push_str("COPY --chmod=755 kuma-user-sync /usr/libexec/kuma-user-sync\n");
        out.push_str(
            "COPY kuma-user-sync.service /usr/lib/systemd/system/kuma-user-sync.service\n",
        );
        if let Some(shell) = &user.shell {
            // after the rpm layer, so a shell the config forgot to install
            // fails the build instead of locking the account out at login
            out.push_str(&format!("RUN test -x /usr/bin/{shell}\n"));
        }
        out.push_str("RUN systemctl enable kuma-user-sync.service\n");
        if !user.ssh_keys.is_empty() {
            out.push_str(&format!("COPY kuma-user-keys /etc/kuma/keys/{}\n", user.name));
            out.push_str(
                "COPY kuma-sshd-keys.conf /etc/ssh/sshd_config.d/40-kuma-keys.conf\n",
            );
        }
    }

    if let Some(hostname) = &config.system.hostname {
        out.push_str(&format!("\nRUN echo '{hostname}' > /etc/hostname\n"));
    }
    if let Some(locale) = &config.system.locale {
        // The langpack makes the locale actually exist; without it glibc
        // silently falls back and every app renders C.UTF-8.
        if let Some(lang) = langpack(locale) {
            out.push_str(&format!(
                "\nRUN dnf -y install glibc-langpack-{lang} && dnf clean all\n"
            ));
        }
        out.push_str(&format!("RUN echo 'LANG={locale}' > /etc/locale.conf\n"));
    }

    out.push_str(BRANDING);

    out.push_str("\nRUN bootc container lint\n");
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

pub fn write_context(config: &Config, dir: &Path) -> Result<()> {
    std::fs::write(dir.join("Containerfile"), generate(config))?;
    std::fs::write(dir.join("kuma-vm-timezone"), VM_TZ_SCRIPT)?;
    std::fs::write(dir.join("kuma-vm-timezone.service"), VM_TZ_SERVICE)?;
    if config.system.desktop == Desktop::Niri {
        std::fs::write(dir.join("greetd-config.toml"), GREETD_CONFIG)?;
        std::fs::write(dir.join("kargs-desktop.toml"), DESKTOP_KARGS)?;
        std::fs::write(dir.join("niri-extras.kdl"), NIRI_EXTRAS)?;
        std::fs::write(dir.join("kuma-wallpaper.png"), WALLPAPER)?;
        std::fs::write(dir.join("waybar-config.jsonc"), WAYBAR_CONFIG)?;
        std::fs::write(dir.join("waybar-style.css"), WAYBAR_STYLE)?;
        std::fs::write(dir.join("fuzzel.ini"), FUZZEL_CONFIG)?;
        std::fs::write(dir.join("mako.conf"), MAKO_CONFIG)?;
        std::fs::write(dir.join("alacritty.toml"), ALACRITTY_CONFIG)?;
        std::fs::write(dir.join("kuma-clipboard-bridge"), CLIPBOARD_BRIDGE)?;
        std::fs::write(dir.join("kuma-xsettings"), XSETTINGS_LAUNCHER)?;
        std::fs::write(dir.join("xsettingsd.conf"), XSETTINGSD_CONF)?;
        std::fs::write(dir.join("dconf-profile"), DCONF_PROFILE)?;
        std::fs::write(dir.join("dconf-kuma-dark"), DCONF_DARK)?;
    }
    if config.system.desktop == Desktop::Niri || !config.packages.flatpak.is_empty() {
        let mut list = config.packages.flatpak.join("\n");
        if !list.is_empty() {
            list.push('\n');
        }
        std::fs::write(dir.join("flatpaks"), list)?;
        std::fs::write(dir.join("kuma-flatpak-sync"), FLATPAK_SYNC_SCRIPT)?;
        std::fs::write(dir.join("kuma-flatpak-sync.service"), FLATPAK_SYNC_SERVICE)?;
    }
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
        std::fs::write(dir.join("kuma-user-sync"), USER_SYNC_SCRIPT)?;
        std::fs::write(dir.join("kuma-user-sync.service"), USER_SYNC_SERVICE)?;
        if !user.ssh_keys.is_empty() {
            let mut keys = user.ssh_keys.join("\n");
            keys.push('\n');
            std::fs::write(dir.join("kuma-user-keys"), keys)?;
            std::fs::write(dir.join("kuma-sshd-keys.conf"), SSHD_KUMA_KEYS)?;
        }
    }
    if config.system.brew {
        std::fs::write(dir.join("kuma-brew-setup"), BREW_SETUP_SCRIPT)?;
        std::fs::write(dir.join("kuma-brew-setup.service"), BREW_SETUP_SERVICE)?;
        std::fs::write(dir.join("brew-profile.sh"), BREW_PROFILE_SH)?;
        std::fs::write(dir.join("brew-profile.fish"), BREW_PROFILE_FISH)?;
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

    #[test]
    fn minimal_config_is_just_base_and_lint() {
        let out = generate(&config("schema_version = 1"));
        assert!(out.contains("FROM quay.io/fedora/fedora-bootc:44"));
        assert!(!out.contains("dnf"));
        assert!(out.contains("bootc container lint"));
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
        assert!(out.contains("RUN dnf -y install fish tailscale && dnf clean all"));
        assert!(out.contains("systemctl enable tailscaled.service"));
        assert!(out.contains("systemctl disable cups.service"));
    }

    #[test]
    fn niri_desktop_generates_curated_layer() {
        let out = generate(&config(
            "schema_version = 1\n[system]\ndesktop = \"niri\"\n",
        ));
        assert!(out.contains("niri"));
        assert!(out.contains("greetd"));
        assert!(out.contains("NetworkManager-wifi"));
        assert!(out.contains("COPY greetd-config.toml /etc/greetd/config.toml"));
        assert!(out.contains("niri validate --config /etc/niri/config.kdl"));
        assert!(out.contains("systemctl set-default graphical.target"));
        assert!(out.contains("greetd.service firewalld.service power-profiles-daemon.service"));
        assert!(out.contains("mask flatpak-add-fedora-repos.service"));
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
        let out = generate(&config(
            "schema_version = 1\n[system]\ndesktop = \"niri\"\n",
        ));
        assert!(out.contains("COPY kuma-wallpaper.png /usr/share/backgrounds/kuma/kuma-wallpaper.png"));
        assert!(out.contains("COPY waybar-config.jsonc /etc/xdg/waybar/config.jsonc"));
        assert!(out.contains("COPY waybar-style.css /etc/xdg/waybar/style.css"));
        assert!(out.contains("COPY fuzzel.ini /etc/skel/.config/fuzzel/fuzzel.ini"));
        assert!(out.contains("COPY mako.conf /etc/skel/.config/mako/config"));
        assert!(out.contains("COPY alacritty.toml /etc/skel/.config/alacritty/alacritty.toml"));
        assert!(out.contains("COPY dconf-profile /etc/dconf/profile/user"));
        assert!(out.contains("COPY dconf-kuma-dark /etc/dconf/db/local.d/10-kuma-dark"));
        assert!(out.contains("RUN dconf update"));
    }

    #[test]
    fn desktop_defaults_to_dark_and_bare_terminal() {
        assert!(DCONF_DARK.contains("color-scheme='prefer-dark'"));
        // a titlebar in a tiling compositor renders light Adwaita chrome
        assert!(ALACRITTY_CONFIG.contains("decorations = \"None\""));
    }

    #[test]
    fn vm_timezone_adoption_ships_in_every_image() {
        let out = generate(&config("schema_version = 1"));
        assert!(out.contains("RUN systemctl enable kuma-vm-timezone.service"));
        assert!(VM_TZ_SCRIPT.contains("qemu_fw_cfg/by_name/opt/org.kuma.tz"));
        // guard against a garbage or hostile fw_cfg value
        assert!(VM_TZ_SCRIPT.contains("[ -e \"/usr/share/zoneinfo/$tz\" ] || exit 0"));
    }

    #[test]
    fn timezone_links_localtime() {
        let out = generate(&config(
            "schema_version = 1\n[system]\ntimezone = \"America/Denver\"\n",
        ));
        assert!(out.contains(
            "test -e /usr/share/zoneinfo/America/Denver && ln -sfn /usr/share/zoneinfo/America/Denver /etc/localtime"
        ));
        // unset means UTC: no localtime layer at all
        let out = generate(&config("schema_version = 1"));
        assert!(!out.contains("/etc/localtime"));
    }

    #[test]
    fn stock_waybar_spawn_is_deduped() {
        let out = generate(&config(
            "schema_version = 1\n[system]\ndesktop = \"niri\"\n",
        ));
        // Fedora's default config spawns waybar; the merge must drop it so
        // only the Kuma extras spawn remains (two spawns = two bars).
        assert!(out.contains("-e '/^spawn-at-startup \"waybar\"$/d'"));
        assert_eq!(NIRI_EXTRAS.matches("spawn-at-startup \"waybar\"").count(), 1);
    }

    #[test]
    fn context_includes_theme_files_for_niri() {
        let dir = tempfile::tempdir().unwrap();
        write_context(
            &config("schema_version = 1\n[system]\ndesktop = \"niri\"\n"),
            dir.path(),
        )
        .unwrap();
        let wallpaper = std::fs::read(dir.path().join("kuma-wallpaper.png")).unwrap();
        assert!(!wallpaper.is_empty());
        let extras = std::fs::read_to_string(dir.path().join("niri-extras.kdl")).unwrap();
        assert!(extras.contains("/usr/share/backgrounds/kuma/kuma-wallpaper.png"));
        assert!(extras.contains("spawn-at-startup \"waybar\""));
        assert!(extras.contains("kuma-clipboard-bridge"));
        assert!(dir.path().join("kuma-clipboard-bridge").exists());
        let greetd = std::fs::read_to_string(dir.path().join("greetd-config.toml")).unwrap();
        assert!(greetd.contains("Welcome to Kuma"));
        assert!(dir.path().join("waybar-config.jsonc").exists());
        assert!(dir.path().join("waybar-style.css").exists());
        assert!(dir.path().join("fuzzel.ini").exists());
        assert!(dir.path().join("mako.conf").exists());
        assert!(dir.path().join("alacritty.toml").exists());
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
        assert!(out.contains("dnf -y install flatpak"));
        assert!(out.contains("/etc/flatpak/remotes.d/flathub.flatpakrepo"));
        assert!(out.contains("COPY flatpaks /usr/lib/kuma/flatpaks"));
        assert!(out.contains("systemctl enable kuma-flatpak-sync.service"));
    }

    #[test]
    fn niri_ships_sync_even_without_declared_apps() {
        let out = generate(&config(
            "schema_version = 1\n[system]\ndesktop = \"niri\"\n",
        ));
        assert!(out.contains("flathub.flatpakrepo"));
        // flatpak comes from the desktop set; no second install layer
        assert!(!out.contains("\nRUN dnf -y install flatpak && dnf clean all"));
        // convergence: the empty declaration still syncs, removing strays
        assert!(out.contains("systemctl enable kuma-flatpak-sync.service"));
    }

    #[test]
    fn flatpak_sync_converges_removals() {
        assert!(FLATPAK_SYNC_SCRIPT.contains("flatpak uninstall --system"));
        // user-level installs are personal state — never touched
        assert!(!FLATPAK_SYNC_SCRIPT.contains("--user"));
        let dir = tempfile::tempdir().unwrap();
        write_context(
            &config("schema_version = 1\n[system]\ndesktop = \"niri\"\n"),
            dir.path(),
        )
        .unwrap();
        // empty declaration is real content: converge to "no system apps"
        assert_eq!(
            std::fs::read_to_string(dir.path().join("flatpaks")).unwrap(),
            ""
        );
        assert!(dir.path().join("kuma-flatpak-sync").exists());
    }

    #[test]
    fn user_generates_boot_sync_not_build_time_useradd() {
        let out = generate(&config(
            "schema_version = 1\n[user]\nname = \"mira\"\nshell = \"fish\"\nssh_keys = [\"ssh-ed25519 AAAA m@kuma\"]\n[packages]\nrpm = [\"fish\"]\n",
        ));
        assert!(out.contains("COPY kuma-user /usr/lib/kuma/user"));
        assert!(out.contains("RUN systemctl enable kuma-user-sync.service"));
        assert!(out.contains("COPY kuma-user-keys /etc/kuma/keys/mira"));
        assert!(out.contains("sshd_config.d/40-kuma-keys.conf"));
        // /home is machine state — the account must be created at boot
        assert!(!out.contains("useradd"));
        // the shell check comes after the rpm layer that installs it
        let rpm_at = out.find("dnf -y install fish").unwrap();
        let check_at = out.find("RUN test -x /usr/bin/fish").unwrap();
        assert!(rpm_at < check_at);

        let out = generate(&config("schema_version = 1"));
        assert!(!out.contains("kuma-user"));
    }

    #[test]
    fn context_writes_user_declaration() {
        let dir = tempfile::tempdir().unwrap();
        write_context(
            &config(
                "schema_version = 1\n[user]\nname = \"mira\"\nshell = \"fish\"\npassword_hash = \"$6$ab$cd\"\nssh_keys = [\"ssh-ed25519 AAAA m@kuma\"]\n",
            ),
            dir.path(),
        )
        .unwrap();
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

    #[test]
    fn hostname_and_locale_pins() {
        let out = generate(&config(
            "schema_version = 1\n[system]\nhostname = \"kuma-laptop\"\nlocale = \"de_DE.UTF-8\"\n",
        ));
        assert!(out.contains("RUN echo 'kuma-laptop' > /etc/hostname"));
        assert!(out.contains("dnf -y install glibc-langpack-de"));
        assert!(out.contains("RUN echo 'LANG=de_DE.UTF-8' > /etc/locale.conf"));
        // C.UTF-8 has no territory, so no langpack layer
        let out = generate(&config(
            "schema_version = 1\n[system]\nlocale = \"C.UTF-8\"\n",
        ));
        assert!(!out.contains("glibc-langpack"));
        assert!(out.contains("LANG=C.UTF-8"));
    }

    #[test]
    fn context_includes_flatpak_list() {
        let dir = tempfile::tempdir().unwrap();
        write_context(
            &config("schema_version = 1\n[packages]\nflatpak = [\"org.mozilla.firefox\", \"org.gnome.Loupe\"]\n"),
            dir.path(),
        )
        .unwrap();
        let list = std::fs::read_to_string(dir.path().join("flatpaks")).unwrap();
        assert_eq!(list, "org.mozilla.firefox\norg.gnome.Loupe\n");
        let script = std::fs::read_to_string(dir.path().join("kuma-flatpak-sync")).unwrap();
        // remote pinned: multiple remotes offering the same ref would make
        // non-interactive installs fail
        assert!(script.contains("--or-update flathub"));
    }

    #[test]
    fn brew_generates_setup_service_and_shell_profiles() {
        let out = generate(&config(
            "schema_version = 1\n[system]\nbrew = true\n",
        ));
        assert!(out.contains("git-core"));
        assert!(out.contains("COPY --chmod=755 kuma-brew-setup /usr/libexec/kuma-brew-setup"));
        assert!(out.contains("systemctl enable kuma-brew-setup.service"));
        assert!(out.contains("/etc/profile.d/kuma-brew.sh"));
        assert!(out.contains("/etc/fish/conf.d/kuma-brew.fish"));

        let dir = tempfile::tempdir().unwrap();
        write_context(&config("schema_version = 1\n[system]\nbrew = true\n"), dir.path())
            .unwrap();
        let script = std::fs::read_to_string(dir.path().join("kuma-brew-setup")).unwrap();
        assert!(script.contains("/home/linuxbrew/.linuxbrew"));
        assert!(dir.path().join("kuma-brew-setup.service").exists());
    }

    #[test]
    fn no_brew_by_default() {
        let out = generate(&config("schema_version = 1"));
        assert!(!out.contains("brew"));
    }

    #[test]
    fn context_includes_greetd_config_for_niri() {
        let dir = tempfile::tempdir().unwrap();
        write_context(
            &config("schema_version = 1\n[system]\ndesktop = \"niri\"\n"),
            dir.path(),
        )
        .unwrap();
        assert!(dir.path().join("Containerfile").exists());
        let greetd = std::fs::read_to_string(dir.path().join("greetd-config.toml")).unwrap();
        assert!(greetd.contains("niri-session"));
        let kargs = std::fs::read_to_string(dir.path().join("kargs-desktop.toml")).unwrap();
        assert!(kargs.contains("quiet"));
    }
}
