//! The hypermedia layer: kuma's outputs tell you which state this
//! machine-plus-workspace is in and which transitions are legal out of
//! it, as runnable commands. Bare `kuma` is the root resource; `--json`
//! serves the same map to scripts and agents. Affordances derive from
//! observed state through one classifier, so a hint that appears was
//! computed, not hand-written at some call site that later went stale.
//!
//! Everything here is passwordless on purpose — the root resource must
//! never cost a sudo prompt. A staged deployment shows in
//! /run/ostree/staged-deployment, brew installs in the Cellar directory;
//! the checks that genuinely need root stay in `kuma doctor`.

use crate::config::Config;
use crate::host::host_output;
use crate::inspect::to_set;
use anyhow::Result;
use std::collections::BTreeSet;
use std::path::Path;

/// Written (as root) whenever `kuma switch`/`update` stage an image, and
/// refreshed by doctor, which holds root and the truth. It exists so the
/// passwordless probe can see the one state that otherwise needs root:
/// a build newer than the deployment. A hint, not an authority — paths
/// that bypass kuma (`bootc rollback`, raw `bootc switch`) go stale
/// until the next doctor run.
pub const DEPLOYED_ID_FILE: &str = "/var/lib/kuma/deployed-image-id";

/// Every kuma image carries the declaration it was built from — the
/// machine is self-describing even when no working copy is around. The
/// probe falls back to it, and `kuma init` seeds from it.
pub const BAKED_CONFIG: &str = "/usr/lib/kuma/kuma.toml";

pub struct Action {
    pub rel: &'static str,
    pub cmd: String,
    pub why: String,
}

impl Action {
    pub fn new(rel: &'static str, cmd: impl Into<String>, why: impl Into<String>) -> Self {
        Self { rel, cmd: cmd.into(), why: why.into() }
    }
}

/// The one rendering every affordance uses: aligned `→ cmd   why` lines.
pub fn print_actions(actions: &[Action]) {
    let width = actions.iter().map(|a| a.cmd.chars().count()).max().unwrap_or(0);
    for action in actions {
        println!("  → {:<width$}   {}", action.cmd, action.why);
    }
}

enum ConfigFact {
    Missing,
    Invalid(String),
    Loaded { rpm: usize, flatpak: usize, brew: usize },
    /// No working copy around; the machine's own baked declaration
    /// (BAKED_CONFIG) speaks for it.
    Baked { rpm: usize, flatpak: usize, brew: usize },
}

struct ImageFact {
    id: String,
    age_secs: u64,
    /// kuma.toml was modified after this image was built.
    edited_after: bool,
}

enum MachineFact {
    /// No ostree/bootc underneath — a build/test workspace.
    NotBootc,
    /// A bootc machine that isn't running a kuma image (yet).
    BootcForeign,
    Kuma {
        staged: bool,
        drift: Vec<String>,
        /// Image ID from DEPLOYED_ID_FILE; None until the first `kuma
        /// switch` on this machine (ISO installs start without it).
        deployed_id: Option<String>,
    },
}

struct Observed {
    config_path: String,
    config: ConfigFact,
    image: Option<ImageFact>,
    machine: MachineFact,
}

struct Snapshot {
    state: &'static str,
    headline: String,
    facts: [(&'static str, String); 3],
    actions: Vec<Action>,
}

/// Entry point for bare `kuma`.
pub fn root(config_path: &Path, json: bool) -> Result<()> {
    let snapshot = classify(&observe(config_path));
    if json {
        println!("{}", serde_json::to_string_pretty(&json_of(&snapshot))?);
        return Ok(());
    }
    println!("state: {} — {}", snapshot.state, snapshot.headline);
    println!();
    for (name, detail) in &snapshot.facts {
        println!("{name:<8} {detail}");
    }
    if !snapshot.actions.is_empty() {
        println!();
        print_actions(&snapshot.actions);
    }
    println!("\n(`kuma --help` lists every command)");
    Ok(())
}

fn json_of(snapshot: &Snapshot) -> serde_json::Value {
    serde_json::json!({
        "state": snapshot.state,
        "headline": snapshot.headline,
        "facts": {
            "config": snapshot.facts[0].1,
            "image": snapshot.facts[1].1,
            "machine": snapshot.facts[2].1,
        },
        "actions": snapshot.actions.iter().map(|a| serde_json::json!({
            "rel": a.rel, "cmd": a.cmd, "why": a.why,
        })).collect::<Vec<_>>(),
    })
}

fn observe(config_path: &Path) -> Observed {
    let config = if config_path.exists() {
        match Config::load(config_path) {
            Ok(config) => ConfigFact::Loaded {
                rpm: config.packages.rpm.len(),
                flatpak: config.packages.flatpak.len(),
                brew: config.packages.brew.len(),
            },
            Err(e) => ConfigFact::Invalid(format!("{e:#}")),
        }
    } else if let Ok(config) = Config::load(Path::new(BAKED_CONFIG)) {
        ConfigFact::Baked {
            rpm: config.packages.rpm.len(),
            flatpak: config.packages.flatpak.len(),
            brew: config.packages.brew.len(),
        }
    } else {
        ConfigFact::Missing
    };

    let image = host_output(&[
        "podman", "image", "inspect", "--format", "{{.Id}} {{.Created.Unix}}", crate::DEFAULT_TAG,
    ])
    .ok()
    .and_then(|out| {
        let (id, created) = out.trim().split_once(' ')?;
        let created: i64 = created.parse().ok()?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(created);
        let edited = std::fs::metadata(config_path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64);
        Some(ImageFact {
            id: id.to_string(),
            age_secs: now.saturating_sub(created).max(0) as u64,
            edited_after: edited.is_some_and(|mtime| mtime > created),
        })
    });

    Observed {
        config_path: config_path.display().to_string(),
        config,
        image,
        machine: observe_machine(),
    }
}

fn observe_machine() -> MachineFact {
    if !Path::new("/run/ostree-booted").exists() {
        return MachineFact::NotBootc;
    }
    if !Path::new("/usr/lib/kuma").is_dir() {
        return MachineFact::BootcForeign;
    }
    let staged = Path::new("/run/ostree/staged-deployment").exists();
    let mut drift = Vec::new();

    // The baked lists are what convergence follows, so drift against them
    // is exactly what the next sync run would change.
    if let Ok(baked) = std::fs::read_to_string("/usr/lib/kuma/flatpaks") {
        if let Ok(installed) =
            host_output(&["flatpak", "list", "--system", "--app", "--columns=application"])
        {
            let baked = to_set(&baked);
            let installed = to_set(&installed);
            count(&mut drift, baked.difference(&installed).count(), "flatpak(s) to install");
            count(&mut drift, installed.difference(&baked).count(), "flatpak(s) to remove");
        }
    }
    // Installed brews are Cellar directory names — a filesystem read, not a
    // multi-second `brew list`. Only ever-declared formulae (the sync state
    // file) count as removals; ad-hoc installs are the owner's.
    if let Ok(baked) = std::fs::read_to_string("/usr/lib/kuma/brews") {
        if let Ok(cellar) = std::fs::read_dir("/home/linuxbrew/.linuxbrew/Cellar") {
            // tapped formulae ("owner/tap/tool") install under their last segment
            let short = |f: &str| f.rsplit('/').next().unwrap_or(f).to_string();
            let installed: BTreeSet<String> = cellar
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect();
            let declared: BTreeSet<String> = to_set(&baked).iter().map(|f| short(f)).collect();
            let state = std::fs::read_to_string("/home/linuxbrew/.linuxbrew/.kuma-brews")
                .unwrap_or_default();
            let ever: BTreeSet<String> = to_set(&state).iter().map(|f| short(f)).collect();
            count(&mut drift, declared.difference(&installed).count(), "brew formula(e) to install");
            let removals = installed
                .iter()
                .filter(|f| ever.contains(*f) && !declared.contains(*f))
                .count();
            count(&mut drift, removals, "brew formula(e) to remove");
        }
    }
    let deployed_id = std::fs::read_to_string(DEPLOYED_ID_FILE)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    MachineFact::Kuma { staged, drift, deployed_id }
}

fn count(drift: &mut Vec<String>, n: usize, what: &str) {
    if n > 0 {
        drift.push(format!("{n} {what}"));
    }
}

/// The state machine, in one place: conditions are checked in priority
/// order; the first that holds names the state, and every condition that
/// holds contributes its edges. Pure, so it's testable without a machine.
fn classify(obs: &Observed) -> Snapshot {
    let mut state: Option<(&'static str, String)> = None;
    let mut actions: Vec<Action> = Vec::new();
    let mut claim = |s: &'static str, headline: String| {
        state.get_or_insert((s, headline));
    };

    if let ConfigFact::Invalid(err) = &obs.config {
        claim("config-invalid", format!("{} is invalid — nothing can build from it", obs.config_path));
        actions.push(Action::new("edit", format!("$EDITOR {}", obs.config_path), err.clone()));
    }
    if let MachineFact::Kuma { staged: true, .. } = &obs.machine {
        claim("staged", "a new deployment is staged — reboot to apply".into());
        actions.push(Action::new(
            "reboot",
            "sudo systemctl reboot",
            "boot the staged deployment; the previous one stays for rollback",
        ));
    }
    if let (ConfigFact::Loaded { .. }, None) = (&obs.config, &obs.image) {
        claim("not-built", format!("{} has never been built here", obs.config_path));
        actions.push(Action::new("build", "kuma build", "build the system image from the declaration"));
    }
    if matches!(&obs.config, ConfigFact::Loaded { .. })
        && obs.image.as_ref().is_some_and(|i| i.edited_after)
    {
        claim("edited", format!("{} changed after the last image build", obs.config_path));
        actions.push(Action::new("build", "kuma build", "rebuild the image to pick up the edit"));
    }
    if let (Some(image), MachineFact::Kuma { deployed_id: Some(deployed), .. }) =
        (&obs.image, &obs.machine)
    {
        if *deployed != image.id {
            claim("built-ahead", "the built image is newer than the deployment".into());
            actions.push(Action::new(
                "switch",
                "kuma switch",
                "stage the newer build (applies on reboot)",
            ));
        }
    }
    if let MachineFact::Kuma { drift, .. } = &obs.machine {
        if !drift.is_empty() {
            claim("drifted", format!("machine drifted from its declaration — {}", drift.join(", ")));
            actions.push(Action::new("sync", "kuma sync", "converge now — also runs at boot and daily"));
            actions.push(Action::new("diff", "kuma diff", "see the drift in detail"));
        }
    }
    if obs.image.is_some() && actions.is_empty() {
        match &obs.machine {
            MachineFact::BootcForeign => {
                claim("built", "image built — this bootc machine could switch to it".into());
                actions.push(Action::new(
                    "switch",
                    "kuma switch",
                    "adopt: point this machine at the kuma image (applies on reboot)",
                ));
                actions.push(Action::new("vm", "kuma vm", "boot the image in a disposable VM instead"));
            }
            MachineFact::NotBootc => {
                claim("built", "image built — this machine can't run it directly".into());
                actions.push(Action::new("vm", "kuma vm", "boot the image in a QEMU VM"));
                actions.push(Action::new("iso", "kuma iso", "build an installer ISO for real hardware"));
            }
            MachineFact::Kuma { .. } => {}
        }
    }
    if matches!(&obs.machine, MachineFact::Kuma { .. }) && actions.is_empty() {
        claim("in-sync", "machine matches its declaration; nothing pending".into());
        actions.push(Action::new("update", "kuma update", "pull the newer base image and rebuild"));
        actions.push(Action::new("doctor", "kuma doctor", "deeper machine health checks"));
    }
    match &obs.config {
        ConfigFact::Missing => {
            claim("no-config", format!("no {} here — nothing declared yet", obs.config_path));
            actions.push(Action::new(
                "init",
                "kuma init",
                "start a system definition in this directory",
            ));
        }
        ConfigFact::Baked { .. } => {
            actions.push(Action::new(
                "init",
                "kuma init",
                "copy this machine's baked declaration here to edit it",
            ));
        }
        _ => {}
    }

    let (state, headline) =
        state.unwrap_or(("in-sync", "nothing pending".into()));
    Snapshot { state, headline, facts: facts_of(obs), actions }
}

fn facts_of(obs: &Observed) -> [(&'static str, String); 3] {
    let config = match &obs.config {
        ConfigFact::Missing => format!("none ({} not found)", obs.config_path),
        ConfigFact::Invalid(_) => format!("{} — invalid", obs.config_path),
        ConfigFact::Loaded { rpm, flatpak, brew } => {
            format!("{} — {rpm} rpm, {flatpak} flatpak, {brew} brew declared", obs.config_path)
        }
        ConfigFact::Baked { rpm, flatpak, brew } => format!(
            "{BAKED_CONFIG} (this machine's baked declaration) — {rpm} rpm, {flatpak} flatpak, {brew} brew"
        ),
    };
    let image = match &obs.image {
        None => format!("none built ({} not in podman storage)", crate::DEFAULT_TAG),
        Some(image) => {
            let edited = if image.edited_after { " — before the last config edit" } else { "" };
            format!("{} — built {} ago{edited}", crate::DEFAULT_TAG, human_age(image.age_secs))
        }
    };
    let machine = match &obs.machine {
        MachineFact::NotBootc => "not a bootc machine — build/test workspace".into(),
        MachineFact::BootcForeign => "bootc machine, not running a kuma image".into(),
        MachineFact::Kuma { staged, drift, .. } => {
            let mut parts = vec!["running a kuma image".to_string()];
            if *staged {
                parts.push("staged deployment pending reboot".into());
            }
            if drift.is_empty() {
                parts.push("converged".into());
            } else {
                parts.push(format!("drifted ({})", drift.join(", ")));
            }
            parts.join("; ")
        }
    };
    [("config", config), ("image", image), ("machine", machine)]
}

fn human_age(secs: u64) -> String {
    match secs {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m", s / 60),
        s if s < 86400 => format!("{}h", s / 3600),
        s => format!("{}d", s / 86400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace(config: ConfigFact, image: Option<ImageFact>, machine: MachineFact) -> Observed {
        Observed { config_path: "kuma.toml".into(), config, image, machine }
    }
    fn loaded() -> ConfigFact {
        ConfigFact::Loaded { rpm: 2, flatpak: 1, brew: 0 }
    }
    fn image(edited_after: bool) -> Option<ImageFact> {
        Some(ImageFact { id: "sha256:aaa".into(), age_secs: 60, edited_after })
    }
    fn kuma_machine(staged: bool, drift: Vec<String>, deployed_id: Option<&str>) -> MachineFact {
        MachineFact::Kuma { staged, drift, deployed_id: deployed_id.map(str::to_string) }
    }

    #[test]
    fn staged_outranks_edits_and_both_edges_appear() {
        let snap = classify(&workspace(
            loaded(),
            image(true),
            kuma_machine(true, vec![], None),
        ));
        assert_eq!(snap.state, "staged");
        assert_eq!(snap.actions[0].rel, "reboot");
        assert!(snap.actions.iter().any(|a| a.rel == "build"));
    }

    #[test]
    fn fresh_directory_offers_init() {
        let snap = classify(&workspace(ConfigFact::Missing, None, MachineFact::NotBootc));
        assert_eq!(snap.state, "no-config");
        assert_eq!(snap.actions[0].cmd, "kuma init");
    }

    #[test]
    fn drift_offers_sync_then_diff() {
        let snap = classify(&workspace(
            loaded(),
            image(false),
            kuma_machine(false, vec!["1 flatpak(s) to install".into()], None),
        ));
        assert_eq!(snap.state, "drifted");
        assert_eq!(snap.actions[0].rel, "sync");
        assert_eq!(snap.actions[1].rel, "diff");
    }

    #[test]
    fn converged_kuma_machine_is_in_sync() {
        let snap = classify(&workspace(
            loaded(),
            image(false),
            kuma_machine(false, vec![], Some("sha256:aaa")),
        ));
        assert_eq!(snap.state, "in-sync");
        assert!(snap.actions.iter().any(|a| a.rel == "update"));
    }

    #[test]
    fn newer_build_than_deployment_offers_switch() {
        let snap = classify(&workspace(
            loaded(),
            image(false),
            kuma_machine(false, vec![], Some("sha256:bbb")),
        ));
        assert_eq!(snap.state, "built-ahead");
        assert_eq!(snap.actions[0].rel, "switch");
    }

    #[test]
    fn missing_deployed_cache_skips_the_freshness_check() {
        let snap = classify(&workspace(loaded(), image(false), kuma_machine(false, vec![], None)));
        assert_eq!(snap.state, "in-sync");
    }

    #[test]
    fn baked_declaration_offers_seeded_init_not_build() {
        let snap = classify(&workspace(
            ConfigFact::Baked { rpm: 2, flatpak: 1, brew: 0 },
            image(false),
            kuma_machine(false, vec![], Some("sha256:aaa")),
        ));
        assert_eq!(snap.state, "in-sync");
        let init = snap.actions.iter().find(|a| a.rel == "init").unwrap();
        assert!(init.why.contains("baked"));
        assert!(!snap.actions.iter().any(|a| a.rel == "build"));
    }

    #[test]
    fn foreign_bootc_machine_gets_the_adoption_edge() {
        let snap = classify(&workspace(loaded(), image(false), MachineFact::BootcForeign));
        assert_eq!(snap.state, "built");
        assert_eq!(snap.actions[0].rel, "switch");
    }

    #[test]
    fn json_shape_is_stable_for_agents() {
        let snap = classify(&workspace(loaded(), None, MachineFact::NotBootc));
        let json = json_of(&snap);
        assert_eq!(json["state"], "not-built");
        assert!(json["facts"]["machine"].is_string());
        assert_eq!(json["actions"][0]["cmd"], "kuma build");
        assert!(json["actions"][0]["why"].is_string());
    }
}
