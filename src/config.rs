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
    #[serde(default)]
    pub snapshots: Snapshots,
}

/// Local btrfs snapshots of the machine state a declaration cannot
/// reproduce. kuma.toml rebuilds a *system*; it does not rebuild
/// /var/home, and a tool that makes machines feel disposable owes that
/// part an answer.
///
/// Deliberately the cheap half of the problem: this survives a bad
/// update, an overwrite, or a deleted directory, and not a dead disk or
/// a stolen laptop. Offsite backup is a different feature with a
/// credential in it, and credentials do not belong in a file that gets
/// committed and baked world-readable into an image — the same boundary
/// capture.rs draws around [user].
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Snapshots {
    #[serde(default)]
    pub enable: bool,
    /// The btrfs subvolume to snapshot. Snapshots land in
    /// `<target>/.snapshots`, a nested subvolume path btrfs leaves out
    /// of the snapshots themselves, so they never nest.
    #[serde(default = "default_snapshot_target")]
    pub target: String,
    /// A systemd OnCalendar expression: "hourly", "daily", or something
    /// like "*-*-* 03:00:00".
    #[serde(default = "default_snapshot_interval")]
    pub interval: String,
    /// Keep this many of the newest snapshots, whatever their age.
    #[serde(default = "default_keep_recent")]
    pub keep_recent: u32,
    /// Then additionally keep the newest snapshot from each of this many
    /// further days, so the total ceiling is keep_recent + keep_daily.
    #[serde(default = "default_keep_daily")]
    pub keep_daily: u32,
}

fn default_snapshot_target() -> String {
    "/var/home".to_string()
}
fn default_snapshot_interval() -> String {
    "hourly".to_string()
}
fn default_keep_recent() -> u32 {
    24
}
fn default_keep_daily() -> u32 {
    7
}

impl Default for Snapshots {
    fn default() -> Self {
        Self {
            enable: false,
            target: default_snapshot_target(),
            interval: default_snapshot_interval(),
            keep_recent: default_keep_recent(),
            keep_daily: default_keep_daily(),
        }
    }
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

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct System {
    /// Unset — the default — means kuma composes its own base from
    /// Fedora's repos (see compose.rs). Set it to any bootc image
    /// reference to build FROM that instead: the escape hatch for
    /// debugging against fedora-bootc, a mirror, or a test tag.
    #[serde(default)]
    pub base: Option<String>,
    /// Trim the composed base's firmware to these packages (members of
    /// compose::FIRMWARE_PACKAGES). Unset ships the broad set: every
    /// vendor's GPU/wifi/audio blobs, so unknown hardware still boots
    /// with everything working. Only meaningful when the base is
    /// composed — an explicit `base` image rejects it.
    #[serde(default)]
    pub firmware: Option<Vec<String>>,
    #[serde(default)]
    pub desktop: Desktop,
    /// The login shell accounts made on this machine get, when nothing
    /// else says otherwise.
    ///
    /// Separate from `[user].shell` on purpose. That one describes a
    /// person, so shareable media cannot carry it: a published image
    /// declares no `[user]` at all, which left an image that installs
    /// fish with no way to say "and use it". This is a property of the
    /// image rather than of anybody, so it survives that rule, and
    /// `kuma install` reads it from the declaration baked into whatever
    /// it is installing.
    ///
    /// Not defaulted to a shell kuma ships, because kuma ships none:
    /// fish and the rest come from a declaration's package list, so a
    /// default would refuse to install any image that omitted it.
    #[serde(default)]
    pub shell: Option<String>,
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
    /// (`hostnamectl set-hostname` persists), and the image ships
    /// /etc/hostname "kuma" as the merge default when unset.
    #[serde(default)]
    pub hostname: Option<String>,
    /// System locale, e.g. "en_US.UTF-8". Installs the matching glibc
    /// langpack and sets /etc/locale.conf. Unset keeps the base default
    /// (C.UTF-8).
    #[serde(default)]
    pub locale: Option<String>,
}

impl Config {
    /// The image the Containerfile builds FROM: the declared base, or —
    /// the kuma default — the content-addressed tag its own composed
    /// base will carry. A pure function of the declaration, so builds,
    /// `kuma generate`, and tests all agree without touching podman.
    pub fn base_ref(&self) -> String {
        self.system.base.clone().unwrap_or_else(|| crate::compose::content_tag(self))
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
        if let Some(base) = &self.system.base {
            validate_name(base, "system.base", &['/', ':', '.', '-', '_', '@'])?;
        }
        if let Some(firmware) = &self.system.firmware {
            if self.system.base.is_some() {
                bail!(
                    "system.firmware trims kuma's composed base; it means nothing \
                     when system.base names an image — remove one of the two"
                );
            }
            for pkg in firmware {
                if !crate::compose::FIRMWARE_PACKAGES.contains(&pkg.as_str()) {
                    bail!(
                        "system.firmware entry {pkg:?} is not a firmware package kuma \
                         knows; the set is: {}",
                        crate::compose::FIRMWARE_PACKAGES.join(", ")
                    );
                }
            }
        }
        if let Some(tz) = &self.system.timezone {
            validate_name(tz, "system.timezone", &['/', '-', '_', '+'])?;
        }
        if let Some(hostname) = &self.system.hostname {
            validate_name(hostname, "system.hostname", &['-', '.'])?;
        }
        if let Some(locale) = &self.system.locale {
            validate_name(locale, "system.locale", &['_', '.', '-', '@'])?;
        }
        // Same character rules as user.shell: both end up as
        // /usr/bin/<name> in a shell fragment the boot converger sources.
        if let Some(shell) = &self.system.shell {
            validate_name(shell, "system.shell", &['-'])?;
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
                let looks_like_key = ["ssh-", "ecdsa-", "sk-"].iter().any(|p| key.starts_with(p));
                if !looks_like_key || key.contains('\n') {
                    bail!("user.ssh_keys entry doesn't look like a single-line OpenSSH public key");
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
        if self.snapshots.enable {
            let target = &self.snapshots.target;
            if !target.starts_with('/') || target.contains("..") {
                bail!("snapshots.target must be an absolute path with no `..` (got {target:?})");
            }
            validate_name(target, "snapshots.target", &['/', '.', '-', '_'])?;
            // Lands in a unit's OnCalendar=, so the systemd calendar
            // alphabet and nothing else.
            validate_name(
                &self.snapshots.interval,
                "snapshots.interval",
                &['*', '-', ':', ' ', ',', '.', '/', '~'],
            )?;
            // A retention that keeps nothing deletes each snapshot on the
            // run that takes it: busywork that looks like a backup.
            if self.snapshots.keep_recent == 0 && self.snapshots.keep_daily == 0 {
                bail!(
                    "snapshots keeps nothing: keep_recent and keep_daily are both 0, \
                     so every snapshot would be deleted by the run that took it"
                );
            }
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
    let well_formed =
        hash.starts_with('$') && fields.len() >= 3 && fields.iter().all(|f| !f.is_empty());
    if !well_formed {
        bail!(
            "user.password_hash {hash:?} is not a crypt(5) hash (expected `$id$salt$hash`, e.g. from `kuma passwd`); \
             a placeholder here builds fine and then fails on the machine at first boot"
        );
    }
    Ok(())
}

pub(crate) fn validate_name(value: &str, field: &str, extra: &[char]) -> Result<()> {
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

/// Commands as a doc page gives them: `$ ` prefixed, with the trailing
/// comment (which is prose, not argument) removed, in the order a reader
/// meets them and without repeats.
///
/// Shared with main.rs, which parses the same lines against the real CLI
/// definition. One reader for both, because two would drift and the
/// thing being guarded against here is drift.
#[cfg(test)]
pub(crate) fn documented_commands(doc: &str) -> Vec<String> {
    let text = std::fs::read_to_string(format!("{}/{doc}", env!("CARGO_MANIFEST_DIR"))).unwrap();
    let mut seen: Vec<String> = Vec::new();
    for line in text.lines() {
        let Some(rest) = line.trim().strip_prefix("$ ") else { continue };
        // No documented command contains a literal '#', so this splits
        // off the explanatory comment and nothing else.
        let cmd = rest.split('#').next().unwrap_or("").trim().to_string();
        if !cmd.is_empty() && !seen.contains(&cmd) {
            seen.push(cmd);
        }
    }
    seen
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The examples now end in `.toml` like any declaration, which makes
    /// them indistinguishable by extension from a real one someone keeps
    /// beside them. `.gitignore` reserves exactly two shapes in that
    /// directory for local declarations, `kuma.toml` and `*.kuma.toml`;
    /// the example walkers skip both, so a personal declaration can never
    /// be pulled into a test that asserts what a *committed* example says.
    pub(crate) fn is_local_declaration(path: &Path) -> bool {
        path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.ends_with("kuma.toml"))
    }

    #[test]
    fn committed_examples_stay_valid() {
        // The examples are documentation that can rot; this keeps every
        // one honest: schema-valid, no real identity in any committed face.
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/examples");
        let mut checked = 0;
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if !path.extension().is_some_and(|e| e == "toml") || is_local_declaration(&path) {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            let config: Config =
                toml::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
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
            let toml =
                format!("schema_version = 1\n[user]\nname = \"me\"\npassword_hash = '{h}'\n");
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

    /// The README tells people to download one filename and the release
    /// workflow publishes another, and nothing else connects the two. A
    /// rename on either side leaves an install command that 404s, which
    /// is invisible here and immediate for anyone following the front
    /// door. So the workflow's asset name is the assertion.
    ///
    /// The name is deliberately unversioned, which is what lets the
    /// README hold a `releases/latest/download/` URL at all; this pins
    /// that too, since putting the version back would break the link
    /// silently one release later.
    #[test]
    fn the_readme_downloads_what_the_release_workflow_publishes() {
        let workflow = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/.github/workflows/release.yml"
        ))
        .unwrap();
        let target = workflow
            .lines()
            .find_map(|l| l.trim().strip_prefix("TARGET: "))
            .expect("the workflow names a build target");
        let asset = format!("kuma-{target}");
        assert!(
            workflow.contains("echo \"name=kuma-${TARGET}\""),
            "the workflow builds its asset name from TARGET, unversioned"
        );

        let readme =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/README.md")).unwrap();
        assert!(
            readme.contains(&format!("releases/latest/download/{asset}")),
            "README should download {asset} from the latest release"
        );

        // SECURITY.md quotes the verify command against the same asset, and
        // a verify command naming a file nobody has is worse than none: it
        // reads as a check that passed.
        let security =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/SECURITY.md")).unwrap();
        assert!(
            security.contains(&format!("{asset}.bundle")) && security.contains(&asset),
            "SECURITY.md should verify {asset} against its bundle"
        );
    }

    /// SECURITY.md says every release asset is signed with Sigstore, and
    /// that sentence has already been false once: the ISO was wired up
    /// unsigned and it took a person reading the workflow to notice. A
    /// claim in the file people read before trusting you should not
    /// depend on somebody re-reading a workflow every time an asset is
    /// added, so the workflow answers for itself here.
    ///
    /// Every file handed to `gh release create` or `gh release upload`
    /// has to be something `cosign sign-blob` signed, or one of the two
    /// files that ride along with a signed one: its checksum and its
    /// bundle.
    #[test]
    fn every_release_asset_is_signed() {
        let workflow = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/.github/workflows/release.yml"
        ))
        .unwrap();

        // What the signing steps produce a bundle for. Quoted, which is
        // what separates a command the workflow runs from the one it
        // quotes at the reader in the release notes.
        let signed: Vec<&str> = workflow
            .lines()
            .filter_map(|l| l.trim().strip_prefix("--bundle '"))
            .filter_map(|r| r.split_once(".bundle'"))
            .map(|(base, _)| base)
            .collect();
        assert!(
            !signed.is_empty(),
            "no cosign sign-blob bundle found; has the signing step moved?"
        );

        let check = |asset: &str| {
            let base = asset.trim_end_matches(".sha256").trim_end_matches(".bundle");
            assert!(
                signed.contains(&base),
                "{base} is attached to a release but nothing signs it; SECURITY.md says every release asset is signed"
            );
        };

        for line in workflow.lines() {
            let t = line.trim();

            // Assets given on the command's own line. The first quoted
            // argument to either verb is the tag, not a file.
            let cmd =
                t.strip_prefix("gh release create").or_else(|| t.strip_prefix("gh release upload"));
            if let Some(rest) = cmd {
                for asset in rest.split('\'').skip(1).step_by(2).skip(1) {
                    check(asset);
                }
                continue;
            }

            // Assets on continuation lines, one per line, which is the
            // shape both sites use today. Read without tracking which
            // command they belong to: a quoted token alone on a line is
            // an asset list or a sign-blob subject, and the file has
            // nothing else shaped that way. The alternative is following
            // backslash continuations, which the release notes heredoc
            // breaks by containing lines that end without one.
            let t = t.trim_end_matches('\\').trim();
            if let Some(asset) = t.strip_prefix('\'').and_then(|r| r.strip_suffix('\'')) {
                check(asset);
            }
        }
    }

    /// The same trap, one asset over, and a worse one to fall into: the
    /// ISO is what a stranger downloads first, and it is 1.8 GB of it.
    /// The walkthrough leads with that URL, so a rename on either side
    /// makes step one of getting started a 404.
    ///
    /// Unversioned for the same reason the binary is, and pinned here for
    /// the same reason: putting the version back would break every
    /// `releases/latest/download/` link silently, one release later.
    #[test]
    fn the_docs_download_the_iso_the_release_workflow_publishes() {
        let workflow = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/.github/workflows/release.yml"
        ))
        .unwrap();
        let asset = workflow
            .lines()
            .find_map(|l| l.trim().strip_prefix("name=\"").map(|r| r.trim_end_matches('"')))
            .expect("the release workflow names its ISO asset");
        assert!(asset.ends_with(".iso"), "expected an ISO asset name, got {asset}");
        assert!(
            !asset.contains("${{"),
            "the ISO asset name is unversioned, so the docs can hold a latest/download URL: {asset}"
        );

        let url = format!("releases/latest/download/{asset}");
        for doc in ["README.md", "docs/getting-started.md"] {
            let text =
                std::fs::read_to_string(format!("{}/{doc}", env!("CARGO_MANIFEST_DIR"))).unwrap();
            assert!(text.contains(&url), "{doc} should download {asset} from the latest release");
        }

        // Same standard the binary is held to: a verify command naming a
        // file nobody has reads as a check that passed.
        let security =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/SECURITY.md")).unwrap();
        assert!(
            security.contains(&format!("{asset}.bundle")),
            "SECURITY.md should verify {asset} against its bundle"
        );
    }

    /// What executes a command the walkthrough hands somebody.
    enum Proof {
        /// A file that runs it, and the text in that file which proves
        /// it. The text is read and checked, so an entry here is
        /// evidence rather than a claim.
        Runs(&'static str, &'static str),
        /// Nothing runs it, recorded as a decision rather than left as
        /// an oversight. Shrinking this list is what turns "somebody
        /// walked the walkthrough once" into a gate.
        Unexecuted(&'static str),
    }

    /// Every command in the walkthrough, and what proves it.
    ///
    /// The walkthrough is the front door: it is what a stranger types,
    /// in order, having never seen kuma. A command in it that nothing
    /// executes is a claim, and a command that drifts from what kuma
    /// actually accepts is a 404 with extra steps.
    ///
    /// This table cannot make an unrun command run. What it does is stop
    /// the count being unknown: a new command in the docs fails this
    /// test until somebody decides which list it belongs in, and a
    /// `Runs` entry whose evidence disappears fails the moment CI stops
    /// running it.
    const WALKTHROUGH: &[(&str, Proof)] = &[
        (
            "curl -LO https://github.com/Letdown2491/kuma-linux/releases/latest/download/kuma-x86_64.iso",
            Proof::Unexecuted("the asset name is asserted by the sibling test; the download itself is not run"),
        ),
        (
            "sudo dd if=kuma-x86_64.iso of=/dev/sdX bs=4M status=progress",
            Proof::Unexecuted("writes a physical disk; CI boots the ISO file directly instead"),
        ),
        ("kuma install", Proof::Unexecuted("smoke.sh always passes --disk; the interactive live-media form is not run")),
        ("kuma", Proof::Unexecuted("the bare status line is not run anywhere")),
        ("kuma init", Proof::Unexecuted("nothing runs it; the starter declaration is only read as a fixture")),
        (
            "curl -LO https://github.com/Letdown2491/kuma-linux/releases/latest/download/kuma-x86_64-unknown-linux-musl",
            Proof::Unexecuted("the asset name is asserted by the sibling test; the download itself is not run"),
        ),
        ("chmod +x kuma-x86_64-unknown-linux-musl", Proof::Unexecuted("plain shell, nothing of kuma's to prove")),
        ("sudo mv kuma-x86_64-unknown-linux-musl /usr/local/bin/kuma", Proof::Unexecuted("plain shell, nothing of kuma's to prove")),
        ("kuma check", Proof::Runs("scripts/smoke.sh", "\"$KUMA\" --config \"$file\" check")),
        ("kuma build", Proof::Runs("scripts/smoke.sh", "\"$KUMA\" --config \"$file\" build --tag")),
        ("kuma switch --yes", Proof::Unexecuted("mutates a deployment; nothing runs it, and it is how a built image is taken")),
        // smoke.sh runs `vm --no-run --rebuild` and then boots the disk
        // with qemu itself, so kuma's own boot path is the half nothing
        // covers, and that half is what the walkthrough is selling.
        ("kuma vm", Proof::Unexecuted("smoke.sh runs vm --no-run to build a disk and boots it with qemu directly")),
        ("kuma vm --apply", Proof::Unexecuted("smoke.sh runs vm --no-run; nothing exercises --apply")),
        ("kuma iso --live", Proof::Runs("scripts/smoke.sh", "iso --live --tag")),
        (
            "kuma install --image ghcr.io/<owner>/kuma:<tag>",
            Proof::Runs("scripts/smoke.sh", "install --disk \"$raw\" --image"),
        ),
        ("kuma doctor", Proof::Runs("scripts/smoke.sh", "kuma doctor --json")),
        ("kuma update --check", Proof::Runs("scripts/smoke.sh", "update --check")),
        ("kuma update --yes", Proof::Unexecuted("builds and stages a deployment; nothing runs it")),
        ("kuma rollback --yes", Proof::Unexecuted("mutates the boot order; nothing runs it")),
    ];

    /// Commands as the walkthrough gives them: `$ ` prefixed, with the
    /// trailing comment (which is prose, not argument) removed.
    fn walkthrough_commands() -> Vec<String> {
        super::documented_commands("docs/getting-started.md")
    }

    /// The gate item of 0.12, and the honest version of "somebody walked
    /// the walkthrough and it worked".
    #[test]
    fn every_command_in_the_walkthrough_is_executed_or_named_as_unexecuted() {
        let documented = walkthrough_commands();
        assert!(
            documented.len() > 10,
            "the walkthrough should hand somebody more than a handful of commands"
        );

        for cmd in &documented {
            assert!(
                WALKTHROUGH.iter().any(|(known, _)| known == cmd),
                "the walkthrough gained a command nothing has decided about: {cmd}\n\
                 add it to WALKTHROUGH as Runs(file, evidence) or Unexecuted(reason)"
            );
        }
        for (known, _) in WALKTHROUGH {
            assert!(
                documented.iter().any(|cmd| cmd == known),
                "WALKTHROUGH names a command the docs no longer give: {known}"
            );
        }

        // Every claim of coverage is read back out of the file that
        // makes it, so this cannot decay into a list of good intentions.
        let mut executed = 0;
        for (cmd, proof) in WALKTHROUGH {
            match proof {
                Proof::Runs(file, evidence) => {
                    let text =
                        std::fs::read_to_string(format!("{}/{file}", env!("CARGO_MANIFEST_DIR")))
                            .unwrap_or_else(|_| {
                                panic!("{cmd} claims {file} runs it, and there is no such file")
                            });
                    assert!(
                        text.contains(evidence),
                        "{cmd} claims {file} runs it, but {file} no longer contains: {evidence}"
                    );
                    executed += 1;
                }
                Proof::Unexecuted(reason) => {
                    assert!(!reason.trim().is_empty(), "{cmd} is unexecuted for no stated reason");
                }
            }
        }
        assert!(executed > 0, "nothing in the walkthrough is executed by anything");
    }

    /// The release workflow pulls a tag's notes out of CHANGELOG.md and
    /// refuses to publish without them, which is what stops the file
    /// rotting. That check runs after the tag exists, and a tag is the one
    /// thing here that cannot be taken back, so the same question is asked
    /// where a version bump is still a working tree: the bump moves
    /// Cargo.toml, Cargo.lock, and the changelog together or it fails.
    #[test]
    fn the_changelog_has_a_section_for_this_version() {
        let changelog =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/CHANGELOG.md")).unwrap();
        let heading = format!("## v{}", env!("CARGO_PKG_VERSION"));
        assert!(
            changelog.lines().any(|l| l == heading || l.starts_with(&format!("{heading} "))),
            "CHANGELOG.md needs a `{heading}` section before this version can be released"
        );
        // Where the next release's notes accumulate. Without the heading
        // there is nowhere for an entry to land in the same push as its
        // change, and a section written at tag time is written from memory.
        assert!(
            changelog.lines().any(|l| l == "## Unreleased"),
            "CHANGELOG.md needs an `## Unreleased` section for the next release's notes"
        );
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

    /// Getting-started's example is the one a newcomer copies, which makes
    /// it the worst place for a declaration that does not build. The
    /// README's is read; this one is typed.
    #[test]
    fn the_getting_started_example_is_a_valid_declaration() {
        let doc = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/docs/getting-started.md"
        ))
        .unwrap();
        let block = doc
            .split_once("```toml\n")
            .and_then(|(_, rest)| rest.split_once("```"))
            .map(|(block, _)| block)
            .expect("getting-started has a ```toml example");
        let config: Config = toml::from_str(block).expect("getting-started example parses");
        config.validate().expect("getting-started example validates");

        // The walkthrough tells the reader to run `kuma passwd` and paste
        // the result in, so the example must not already carry one: a
        // published hash in the document everyone copies is the one place
        // it would spread furthest.
        assert!(
            config.user.as_ref().is_some_and(|u| u.password_hash.is_none()),
            "the walkthrough's example must declare a user and no password hash"
        );
    }

    /// niri.toml says it shows every field the schema accepts,
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
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/niri.toml"))
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
            assert!(documented, "examples/niri.toml never mentions `{field}`");
        }
    }

    /// A retention that keeps nothing would delete each snapshot on the
    /// run that took it: an expensive way to look protected.
    #[test]
    fn a_retention_that_keeps_nothing_is_rejected() {
        let keeps_nothing: Config = toml::from_str(
            "schema_version = 1\n[snapshots]\nenable = true\nkeep_recent = 0\nkeep_daily = 0\n",
        )
        .unwrap();
        assert!(keeps_nothing.validate().is_err());

        // one tier is enough; and the same policy disabled is nobody's
        // problem, so it must not fail a build that never snapshots
        let one_tier: Config = toml::from_str(
            "schema_version = 1\n[snapshots]\nenable = true\nkeep_recent = 0\nkeep_daily = 1\n",
        )
        .unwrap();
        one_tier.validate().unwrap();
        let disabled: Config =
            toml::from_str("schema_version = 1\n[snapshots]\nkeep_recent = 0\nkeep_daily = 0\n")
                .unwrap();
        disabled.validate().unwrap();
    }

    /// The target is baked into a root script that deletes subvolumes, so
    /// path traversal and relative paths are refused at `kuma check`.
    #[test]
    fn a_snapshot_target_must_be_an_absolute_path() {
        for bad in ["/var/home/../etc", "var/home", "/var/home; rm -rf /"] {
            let config: Config = toml::from_str(&format!(
                "schema_version = 1\n[snapshots]\nenable = true\ntarget = \"{bad}\"\n"
            ))
            .unwrap();
            assert!(config.validate().is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn minimal_config_parses_with_defaults() {
        let config: Config = toml::from_str("schema_version = 1").unwrap();
        config.validate().unwrap();
        // The default base is kuma's own composed one, not fedora-bootc.
        assert_eq!(config.system.base, None);
        assert!(config.base_ref().starts_with("localhost/kuma-base:m"));
        assert!(config.packages.rpm.is_empty());
    }

    #[test]
    fn firmware_trim_is_validated() {
        let unknown: Config =
            toml::from_str("schema_version = 1\n[system]\nfirmware = [\"warp-core-firmware\"]\n")
                .unwrap();
        assert!(unknown.validate().is_err());
        let with_image_base: Config = toml::from_str(
            "schema_version = 1\n[system]\nbase = \"quay.io/x/y:1\"\nfirmware = [\"amd-gpu-firmware\"]\n",
        )
        .unwrap();
        assert!(with_image_base.validate().is_err());
        let good: Config = toml::from_str(
            "schema_version = 1\n[system]\nfirmware = [\"amd-gpu-firmware\", \"mt7xxx-firmware\"]\n",
        )
        .unwrap();
        good.validate().unwrap();
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
        let config: Config =
            toml::from_str("schema_version = 1\n[packages]\nrpm = [\"fish; rm -rf /\"]\n").unwrap();
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
