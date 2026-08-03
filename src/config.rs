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
    pub packages: Packages,
    #[serde(default)]
    pub services: Services,
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
}

impl Default for System {
    fn default() -> Self {
        Self {
            base: default_base(),
            desktop: Desktop::default(),
            brew: false,
            timezone: None,
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
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("cannot read {}", path.display()))?;
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
        for pkg in &self.packages.rpm {
            validate_name(pkg, "packages.rpm", &['.', '-', '_', '+', ':'])?;
        }
        for app in &self.packages.flatpak {
            validate_name(app, "packages.flatpak", &['.', '-', '_'])?;
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
    fn unknown_fields_rejected() {
        let result: Result<Config, _> =
            toml::from_str("schema_version = 1\n[packages]\ndnf = [\"fish\"]\n");
        assert!(result.is_err());
    }
}
