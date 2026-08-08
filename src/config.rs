use anyhow::{bail, Context, Result};
use schemars::JsonSchema;
use serde::Deserialize;
use std::path::Path;

pub const CURRENT_SCHEMA: u32 = 1;
pub const DEFAULT_BASE: &str = "quay.io/fedora/fedora-bootc:44";

/// A kuma system declaration: the one file that describes a machine.
#[derive(Debug, Deserialize, JsonSchema)]
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

/// The primary account, created and converged by a boot service, not at
/// image build time, because /home is machine state (/var/home) and an
/// image-built home directory never materializes on `bootc switch`.
#[derive(Debug, Deserialize, JsonSchema)]
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
    /// account is first created; after that the password is machine state.
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

#[derive(Debug, Deserialize, JsonSchema)]
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
/// not a package list; the curation is Kuma's job. Niri is hand-assembled
/// (a compositor needs a desktop built around it); COSMIC curates itself
/// and kuma adds only hardware enablement and identity.
#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq, Default, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Desktop {
    #[default]
    None,
    Niri,
    Cosmic,
}

fn default_base() -> String {
    DEFAULT_BASE.to_string()
}

#[derive(Debug, Deserialize, Default, JsonSchema)]
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
    /// candidates; ad-hoc `brew install` on the machine stays yours.
    /// A non-empty list implies system.brew.
    #[serde(default)]
    pub brew: Vec<String>,
}

#[derive(Debug, Deserialize, Default, JsonSchema)]
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
                "cannot read {}; run `kuma init` to start one here, or point --config at yours",
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
                // crypt(5) alphabet plus '=' for rounds= parameters; all
                // inert inside the single-quoted declaration file the
                // sync script sources
                validate_name(hash, "user.password_hash", &['$', '.', '/', '='])?;
                validate_password_hash(hash)?;
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
/// A password hash that crypt can never accept has to fail here, at
/// `kuma check`, not eight minutes later on the machine.
///
/// Found by the smoke tests: every committed example shipped the
/// placeholder `password_hash = '...'`, which validated, built, and then
/// took down kuma-user-sync on first boot, because `chpasswd -e` rejects
/// it ("invalid password hash") and the sync script runs under
/// `set -euo pipefail`. The account ended up with no password and the
/// only evidence was a failed unit nobody was looking at. A declaration
/// that validates must either become a running system or fail at the
/// earliest stage, and this was neither.
///
/// Deliberately shape-only, not an allowlist of crypt ids: `$6$`, yescrypt
/// `$y$`, bcrypt `$2b$`, and whatever glibc adds next all have to keep
/// working, and rejecting a hash someone's machine already accepts would
/// be a worse bug than the one this fixes. Anything of the form
/// `$id$[params$]salt$hash` with non-empty fields passes.
fn validate_password_hash(hash: &str) -> Result<()> {
    let fields: Vec<&str> = hash.strip_prefix('$').unwrap_or("").split('$').collect();
    let well_formed = hash.starts_with('$')
        && fields.len() >= 3
        && fields.iter().all(|f| !f.is_empty());
    if !well_formed {
        bail!(
            "user.password_hash {hash:?} is not a crypt(5) hash (expected `$id$salt$hash`, e.g. from `kuma passwd`); \
             a placeholder here builds fine and then fails on the machine at first boot"
        );
    }
    Ok(())
}

fn validate_name(value: &str, field: &str, extra: &[char]) -> Result<()> {
    if value.is_empty() {
        bail!("{field} contains an empty entry");
    }
    // A leading dash turns a "name" into a flag for whatever command the
    // list feeds — dnf, systemctl, flatpak install (as root, every boot).
    // No real package, service, zone, or locale starts with one.
    if value.starts_with('-') {
        bail!("{field} entry {value:?} starts with '-': names cannot be options");
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
    fn committed_examples_stay_valid() {
        // The examples are documentation that can rot; this keeps every
        // one honest: schema-valid, no real identity in any committed face.
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/examples");
        let mut checked = 0;
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if !path.extension().is_some_and(|e| e == "example") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            let config: Config = toml::from_str(&text)
                .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            config.validate().unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            if let Some(user) = &config.user {
                assert_eq!(user.name, "me", "{}", path.display());
                // No hash in a committed example, in either direction: a
                // real one would be identity in git, and a placeholder is
                // worse than nothing since it validates here and then
                // fails on the machine (see validate_password_hash). The
                // examples show the line commented out instead.
                assert_eq!(user.password_hash, None, "{}", path.display());
            }
            checked += 1;
        }
        assert!(checked >= 2, "expected the committed examples, found {checked}");
    }

    /// The placeholder that started this: it passed every check kuma had,
    /// built an image, and then failed kuma-user-sync on first boot
    /// because chpasswd rejects it. Rejecting it at validate time is the
    /// promise ("fail at the earliest stage") being kept.
    ///
    /// The accept list matters more than the reject list here. Rejecting
    /// a hash that a machine already accepts would lock someone out of
    /// their own declaration, so anything crypt-shaped passes: every id,
    /// with or without parameters.
    #[test]
    fn password_hash_must_be_one_crypt_could_accept() {
        let hashed = |h: &str| {
            let toml = format!(
                "schema_version = 1\n[user]\nname = \"me\"\npassword_hash = '{h}'\n"
            );
            toml::from_str::<Config>(&toml).unwrap().validate()
        };

        // sha512crypt, as `openssl passwd -6` emits it
        assert!(hashed("$6$Xy1z$abcDEF.ghi/JKL0123456789").is_ok());
        // what `kuma passwd` emits: a rounds= parameter field
        assert!(hashed("$6$rounds=656000$Xy1z$abcDEF.ghi/JKL0123456789").is_ok());
        // yescrypt (Fedora's default) and bcrypt: no id allowlist here
        assert!(hashed("$y$j9T$Xy1z$abcDEF0123456789").is_ok());
        assert!(hashed("$2b$12$Xy1zabcDEF0123456789").is_ok());

        // the placeholder, and the shapes next to it
        assert!(hashed("...").is_err(), "the placeholder every example shipped");
        assert!(hashed("$6$").is_err(), "no salt, no hash");
        assert!(hashed("$6$$abcdef").is_err(), "empty salt field");
        assert!(hashed("hunter2").is_err(), "a plaintext password is not a hash");
    }

    /// The README's example declaration is the first thing anyone copies,
    /// so it is held to the same bar as the committed examples. It has
    /// been wrong before: it showed `password_hash = '...'` for months,
    /// which is a placeholder the parser now rejects precisely because it
    /// built an image that failed on first boot.
    #[test]
    fn the_readme_example_is_a_valid_declaration() {
        let readme =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/README.md")).unwrap();
        let block = readme
            .split_once("```toml\n")
            .and_then(|(_, rest)| rest.split_once("```"))
            .map(|(block, _)| block)
            .expect("README has a ```toml example");
        let config: Config = toml::from_str(block).expect("README example parses");
        config.validate().expect("README example validates");
        assert!(config.user.is_some(), "the example shows a declared [user]");
    }

    /// kuma.toml.example says it shows every field the schema accepts,
    /// which is a promise a reader can't check and a schema change can
    /// quietly break. `services.disable` and `system.brew` were both
    /// missing when this was written.
    ///
    /// Commented-out fields count: an optional field is documented by
    /// showing its shape, not by being switched on in the example.
    #[test]
    fn the_full_example_documents_every_field() {
        let schema = serde_json::to_value(schemars::schema_for!(Config)).unwrap();
        let example =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/kuma.toml.example"))
                .unwrap();

        // Field names from the schema itself, so a new one is covered the
        // day it lands rather than the day someone remembers.
        let mut fields: Vec<String> = Vec::new();
        let mut collect = |object: &serde_json::Value| {
            if let Some(props) = object["properties"].as_object() {
                fields.extend(props.keys().cloned());
            }
        };
        collect(&schema);
        if let Some(defs) = schema["$defs"].as_object() {
            for definition in defs.values() {
                collect(definition);
            }
        }
        assert!(fields.len() > 10, "schema walk found only {fields:?}");

        for field in &fields {
            // A key is documented by `<field> =`, a table by its `[field]`
            // header. Either may be commented out.
            let documented = example.lines().any(|line| {
                let line = line.trim_start().trim_start_matches('#').trim_start();
                line.starts_with(&format!("{field} "))
                    || line.starts_with(&format!("{field}="))
                    || line.starts_with(&format!("[{field}]"))
            });
            assert!(documented, "examples/kuma.toml.example never mentions `{field}`");
        }
    }

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
    fn leading_dash_option_injection_rejected() {
        // "--nogpgcheck" would ride into `dnf -y install` as a flag; the
        // flatpak list feeds root-run `flatpak install` on every boot
        for toml in [
            "schema_version = 1\n[packages]\nrpm = [\"--nogpgcheck\"]\n",
            "schema_version = 1\n[packages]\nflatpak = [\"--reinstall\"]\n",
            "schema_version = 1\n[services]\nenable = [\"--global\"]\n",
        ] {
            let config: Config = toml::from_str(toml).unwrap();
            assert!(config.validate().is_err(), "should reject: {toml}");
        }
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
