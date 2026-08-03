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
    // session essentials
    "wl-clipboard",
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
Description=Sync declared Flatpak applications
Wants=network-online.target
After=network-online.target

[Service]
Type=oneshot
ExecStart=/usr/bin/bash -c 'xargs -r -a /usr/lib/kuma/flatpaks flatpak install --system --assumeyes --noninteractive --or-update flathub'

[Install]
WantedBy=multi-user.target
"#;

const FLATHUB_URL: &str = "https://dl.flathub.org/repo/flathub.flatpakrepo";

/// Appended to niri's full default config (copied from the package) so the
/// stock keybindings survive; niri configs replace defaults entirely.
const NIRI_EXTRAS: &str = r##"

// Kuma session services
spawn-at-startup "/usr/libexec/polkit-mate-authentication-agent-1"
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
            "RUN systemctl set-default graphical.target && systemctl enable greetd.service firewalld.service power-profiles-daemon.service\n",
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
    if !config.packages.flatpak.is_empty() {
        out.push_str("COPY flatpaks /usr/lib/kuma/flatpaks\n");
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

    if let Some(tz) = &config.system.timezone {
        // test -e first so a typo'd zone fails the build instead of
        // silently producing a dangling /etc/localtime symlink.
        out.push_str(&format!(
            "\nRUN test -e /usr/share/zoneinfo/{tz} && ln -sfn /usr/share/zoneinfo/{tz} /etc/localtime\n"
        ));
    }

    out.push_str(BRANDING);

    out.push_str("\nRUN bootc container lint\n");
    out
}

/// Write the full build context: the Containerfile plus any files it COPYs.
pub fn write_context(config: &Config, dir: &Path) -> Result<()> {
    std::fs::write(dir.join("Containerfile"), generate(config))?;
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
        std::fs::write(dir.join("dconf-profile"), DCONF_PROFILE)?;
        std::fs::write(dir.join("dconf-kuma-dark"), DCONF_DARK)?;
    }
    if !config.packages.flatpak.is_empty() {
        let mut list = config.packages.flatpak.join("\n");
        list.push('\n');
        std::fs::write(dir.join("flatpaks"), list)?;
        std::fs::write(dir.join("kuma-flatpak-sync.service"), FLATPAK_SYNC_SERVICE)?;
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
    fn niri_includes_flathub_but_no_sync_without_declared_apps() {
        let out = generate(&config(
            "schema_version = 1\n[system]\ndesktop = \"niri\"\n",
        ));
        assert!(out.contains("flathub.flatpakrepo"));
        // flatpak comes from the desktop set; no second install layer
        assert!(!out.contains("\nRUN dnf -y install flatpak && dnf clean all"));
        assert!(!out.contains("kuma-flatpak-sync"));
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
        let service =
            std::fs::read_to_string(dir.path().join("kuma-flatpak-sync.service")).unwrap();
        // remote pinned: multiple remotes offering the same ref would make
        // non-interactive installs fail
        assert!(service.contains("--or-update flathub"));
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
