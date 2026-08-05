use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::path::Path;

pub const CURRENT_SCHEMA: u32 = 1;
pub const DEFAULT_BASE: &str = "quay.io/fedora/fedora-bootc:44";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub schema_version: u32,
    #[serde(default)]
    pub system: System,
    #[serde(default)]
    pub user: Option<User>,
    #[serde(default)]
    pub packages: Packages,
    #[serde(default)]
    pub services: Services,
}

/// The primary account, created and converged by a boot service — not at
/// image build time, because /home is machine state (/var/home) and an
/// image-built home directory never materializes on `bootc switch`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct User {
    pub name: String,
    /// Login shell by name ("fish"); the binary must exist in the image,
    /// checked at build time.
    #[serde(default)]
    pub shell: Option<String>,
    /// Converged additively: declared groups are ensured, groups granted
    /// imperatively on the machine are left alone.
    #[serde(default = "default_groups")]
    pub groups: Vec<String>,
    /// crypt(5) hash (e.g. `openssl passwd -6`), applied only when the
    /// account is first created — after that the password is machine state.
    #[serde(default)]
    pub password_hash: Option<String>,
    /// OpenSSH public keys, served from /etc/kuma/keys/<name> alongside
    /// the user's own ~/.ssh/authorized_keys (never overwriting it).
    #[serde(default)]
    pub ssh_keys: Vec<String>,
    /// Log this user straight into the desktop at boot (greetd
    /// initial_session). The greeter still appears after logout.
    #[serde(default)]
    pub autologin: bool,
}

fn default_groups() -> Vec<String> {
    vec!["wheel".to_string()]
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct System {
    #[serde(default = "default_base")]
    pub base: String,
    #[serde(default)]
    pub desktop: Desktop,
    /// Homebrew in /home/linuxbrew: imperative CLI tools that survive image
    /// updates and need no reboot. Installed by a first-boot service.
    #[serde(default)]
    pub brew: bool,
    /// Pin an IANA timezone (e.g. "America/Denver") in the image. Usually
    /// unset: timezone is machine state, set once via timedatectl and kept
    /// across image updates; `kuma vm` mirrors the host.
    #[serde(default)]
    pub timezone: Option<String>,
    /// Pin /etc/hostname. Usually unset: hostname is machine state
    /// (`hostnamectl set-hostname` persists), and os-release branding
    /// already makes unset default to "kuma".
    #[serde(default)]
    pub hostname: Option<String>,
    /// System locale, e.g. "en_US.UTF-8". Installs the matching glibc
    /// langpack and sets /etc/locale.conf. Unset keeps the base default
    /// (C.UTF-8).
    #[serde(default)]
    pub locale: Option<String>,
}

impl Default for System {
    fn default() -> Self {
        Self {
            base: default_base(),
            desktop: Desktop::default(),
            brew: false,
            timezone: None,
            hostname: None,
            locale: None,
        }
    }
}

/// A desktop is a curated set (compositor, greeter, portals, audio, fonts),
/// not a package list — the curation is Kuma's job.
#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Desktop {
    #[default]
    None,
    Niri,
}

fn default_base() -> String {
    DEFAULT_BASE.to_string()
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Packages {
    #[serde(default)]
    pub rpm: Vec<String>,
    /// Flatpaks are runtime state, not image content. Accepted in the schema
    /// now so configs don't break later; applied by a future `kuma sync`.
    #[serde(default)]
    pub flatpak: Vec<String>,
    /// Homebrew formulae, converged like flatpaks: additions install,
    /// removals uninstall. Only formulae this list ever named are removal
    /// candidates — ad-hoc `brew install` on the machine stays yours.
    /// A non-empty list implies system.brew.
    #[serde(default)]
    pub brew: Vec<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Services {
    #[serde(default)]
    pub enable: Vec<String>,
    #[serde(default)]
    pub disable: Vec<String>,
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| {
            format!(
                "cannot read {} — run `kuma init` to start one here, or point --config at yours",
                path.display()
            )
        })?;
        let config: Config = toml::from_str(&text)
            .with_context(|| format!("invalid config in {}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != CURRENT_SCHEMA {
            bail!(
                "unsupported schema_version {} (this kuma understands {})",
                self.schema_version,
                CURRENT_SCHEMA
            );
        }
        validate_name(&self.system.base, "system.base", &['/', ':', '.', '-', '_', '@'])?;
        if let Some(tz) = &self.system.timezone {
            validate_name(tz, "system.timezone", &['/', '-', '_', '+'])?;
        }
        if let Some(hostname) = &self.system.hostname {
            validate_name(hostname, "system.hostname", &['-', '.'])?;
        }
        if let Some(locale) = &self.system.locale {
            validate_name(locale, "system.locale", &['_', '.', '-', '@'])?;
        }
        if let Some(user) = &self.user {
            validate_name(&user.name, "user.name", &['-', '_'])?;
            if let Some(shell) = &user.shell {
                validate_name(shell, "user.shell", &['-'])?;
            }
            for group in &user.groups {
                validate_name(group, "user.groups", &['-', '_'])?;
            }
            if let Some(hash) = &user.password_hash {
                // crypt(5) alphabet; also keeps the hash safe inside the
                // single-quoted declaration file the sync script sources
                validate_name(hash, "user.password_hash", &['$', '.', '/'])?;
            }
            for key in &user.ssh_keys {
                let looks_like_key = ["ssh-", "ecdsa-", "sk-"]
                    .iter()
                    .any(|p| key.starts_with(p));
                if !looks_like_key || key.contains('\n') {
                    bail!(
                        "user.ssh_keys entry doesn't look like a single-line OpenSSH public key"
                    );
                }
            }
        }
        for pkg in &self.packages.rpm {
            validate_name(pkg, "packages.rpm", &['.', '-', '_', '+', ':'])?;
        }
        for app in &self.packages.flatpak {
            validate_name(app, "packages.flatpak", &['.', '-', '_'])?;
        }
        for formula in &self.packages.brew {
            // '@' for versioned formulae (node@22), '/' for taps (owner/tap/tool)
            validate_name(formula, "packages.brew", &['.', '-', '_', '+', '@', '/'])?;
        }
        for svc in self.services.enable.iter().chain(&self.services.disable) {
            validate_name(svc, "services", &['.', '-', '_', '@'])?;
        }
        Ok(())
    }
}

/// Entries end up inside generated RUN instructions, so restrict them to a
/// conservative character set rather than trusting shell quoting.
fn validate_name(value: &str, field: &str, extra: &[char]) -> Result<()> {
    if value.is_empty() {
        bail!("{field} contains an empty entry");
    }
    for ch in value.chars() {
        if !ch.is_ascii_alphanumeric() && !extra.contains(&ch) {
            bail!("{field} entry {value:?} contains unsupported character {ch:?}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_config_parses_with_defaults() {
        let config: Config = toml::from_str("schema_version = 1").unwrap();
        config.validate().unwrap();
        assert_eq!(config.system.base, DEFAULT_BASE);
        assert!(config.packages.rpm.is_empty());
    }

    #[test]
    fn wrong_schema_version_rejected() {
        let config: Config = toml::from_str("schema_version = 2").unwrap();
        assert!(config.validate().is_err());
    }

    #[test]
    fn shell_metacharacters_rejected() {
        let config: Config = toml::from_str(
            "schema_version = 1\n[packages]\nrpm = [\"fish; rm -rf /\"]\n",
        )
        .unwrap();
        assert!(config.validate().is_err());
    }

    #[test]
    fn brew_formula_names_validated() {
        let config: Config = toml::from_str(
            "schema_version = 1\n[packages]\nbrew = [\"node@22\", \"oven-sh/bun/bun\"]\n",
        )
        .unwrap();
        config.validate().unwrap();

        let config: Config =
            toml::from_str("schema_version = 1\n[packages]\nbrew = [\"rg $(id)\"]\n").unwrap();
        assert!(config.validate().is_err());
    }

    #[test]
    fn unknown_fields_rejected() {
        let result: Result<Config, _> =
            toml::from_str("schema_version = 1\n[packages]\ndnf = [\"fish\"]\n");
        assert!(result.is_err());
    }

    #[test]
    fn user_defaults_and_validation() {
        let config: Config =
            toml::from_str("schema_version = 1\n[user]\nname = \"mira\"\n").unwrap();
        config.validate().unwrap();
        assert_eq!(config.user.unwrap().groups, vec!["wheel"]);

        // hash outside the crypt(5) alphabet (it gets single-quoted into a
        // sourced shell file) is rejected
        let config: Config = toml::from_str(
            "schema_version = 1\n[user]\nname = \"m\"\npassword_hash = \"$6$a b\"\n",
        )
        .unwrap();
        assert!(config.validate().is_err());

        let config: Config = toml::from_str(
            "schema_version = 1\n[user]\nname = \"m\"\nssh_keys = [\"not-a-key\"]\n",
        )
        .unwrap();
        assert!(config.validate().is_err());
    }
}
