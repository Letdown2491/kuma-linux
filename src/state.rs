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
use std::sync::OnceLock;

/// Written (as root) whenever `kuma switch`/`update` stage an image, and
/// refreshed by doctor, which holds root and the truth. It exists so the
/// passwordless probe can see the one state that otherwise needs root:
/// a build newer than the deployment. A hint, not an authority — raw
/// bootc paths (`bootc rollback`, `bootc switch`) go stale until the
/// next doctor run; `kuma rollback` drops the stamp instead, since the
/// rollback target's image ID is unknowable from podman storage.
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
        let cmd = apply_config_flag(cmd.into(), CONFIG_FLAG.get().and_then(Option::as_deref));
        Self { rel, cmd, why: why.into() }
    }
}

/// The `--config` this invocation carried, already rendered as the flag
/// to append to printed commands; None when the declaration was
/// discovered rather than named. Set once in main, before any affordance
/// exists to carry it.
///
/// Affordances are promises that a command is runnable, and a hint that
/// drops `--config` names a different file than the command that printed
/// it. From a directory with no kuma.toml that's a loud error; from one
/// with an unrelated kuma.toml it's a silent write into the wrong
/// declaration. Discovered paths deliberately stay off the printed
/// command: they'd resolve the same way for the reader, and every write
/// this suggests already names its target in the line above.
static CONFIG_FLAG: OnceLock<Option<String>> = OnceLock::new();

/// The verbs that resolve a declaration in `run`. Everything else takes
/// `--config` from clap (it's global) and ignores it — `init` always
/// writes ./kuma.toml, and switch/vm/sync/doctor/rollback never read one
/// — so appending it there would claim an influence it doesn't have.
const CONFIG_VERBS: &[&str] = &[
    "add", "build", "capture", "check", "clean", "diff", "generate", "iso", "remove", "snapshot",
    "update",
];

/// Called once from main with the `--config` path, or None when discovery
/// found the declaration on its own.
pub fn set_config_flag(explicit: Option<&Path>) {
    let _ = CONFIG_FLAG.set(explicit.map(|path| format!(" --config {}", shell_quote(path))));
}

/// The pure half of the flag logic, so tests can exercise it without
/// touching the process-wide cell (which, once set, would leak into every
/// other test's affordances).
fn apply_config_flag(cmd: String, flag: Option<&str>) -> String {
    let Some(flag) = flag else { return cmd };
    match cmd.strip_prefix("kuma ").and_then(|rest| rest.split_whitespace().next()) {
        Some(verb) if CONFIG_VERBS.contains(&verb) => format!("{cmd}{flag}"),
        _ => cmd,
    }
}

/// Single-quote anything that isn't a bare word, so a declaration under a
/// directory with a space in it survives the copy-paste it was printed
/// for.
fn shell_quote(path: &Path) -> String {
    let shown = path.display().to_string();
    let bare = !shown.is_empty()
        && shown.chars().all(|c| c.is_ascii_alphanumeric() || "._/-@+:,=".contains(c));
    if bare {
        shown
    } else {
        format!("'{}'", shown.replace('\'', r"'\''"))
    }
}

/// The one edge out of "staged" — shared by everything that stages
/// (switch, update, the bare-kuma classifier), so the promise it makes
/// about rollback can never fork.
pub fn reboot_action() -> Action {
    Action::new(
        "reboot",
        "sudo systemctl reboot",
        "boot the staged deployment; kuma rollback returns to the previous one",
    )
}

/// The JSON twin of print_actions: one shape for affordances everywhere
/// --json is spoken (bare kuma, doctor, diff).
pub fn action_json(action: &Action) -> serde_json::Value {
    serde_json::json!({ "rel": action.rel, "cmd": action.cmd, "why": action.why })
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
    Loaded {
        rpm: usize,
        flatpak: usize,
        brew: usize,
    },
    /// No working copy around; the machine's own baked declaration
    /// (BAKED_CONFIG) speaks for it.
    Baked {
        rpm: usize,
        flatpak: usize,
        brew: usize,
    },
}

struct ImageFact {
    id: String,
    age_secs: u64,
    /// kuma.toml was modified after this image was built.
    edited_after: bool,
    /// `io.kuma.builder`: which kuma generated this image. None for images
    /// built before the label existed, which is itself the answer — those
    /// were built by a kuma older than this one.
    builder: Option<String>,
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
    /// Running from installer media. A field rather than a probe inside
    /// `classify`, so the classifier stays a pure function of what was
    /// observed and this state is testable without an ISO.
    live: bool,
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
    println!("state: {} - {}", snapshot.state, snapshot.headline);
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
        "actions": snapshot.actions.iter().map(action_json).collect::<Vec<_>>(),
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

    // The builder label rides along with the id and the timestamp rather
    // than costing a second call. It is last because it is the only field
    // that can contain spaces, so a splitn leaves it whole.
    let image = host_output(&[
        "podman",
        "image",
        "inspect",
        "--format",
        "{{.Id}} {{.Created.Unix}} {{index .Config.Labels \"io.kuma.builder\"}}",
        crate::DEFAULT_TAG,
    ])
    .ok()
    .and_then(|out| {
        let mut parts = out.trim().splitn(3, ' ');
        let id = parts.next()?;
        let created = parts.next()?;
        // podman prints "<no value>" for a label the image does not carry.
        let builder = parts
            .next()
            .map(str::trim)
            .filter(|b| !b.is_empty() && *b != "<no value>")
            .map(str::to_string);
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
            builder,
        })
    });

    Observed {
        config_path: config_path.display().to_string(),
        config,
        image,
        machine: observe_machine(),
        live: Path::new(crate::liveiso::LIVE_MARKER).exists(),
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
            let installed: BTreeSet<String> =
                cellar.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
            let declared: BTreeSet<String> = to_set(&baked).iter().map(|f| short(f)).collect();
            let state = std::fs::read_to_string("/home/linuxbrew/.linuxbrew/.kuma-brews")
                .unwrap_or_default();
            let ever: BTreeSet<String> = to_set(&state).iter().map(|f| short(f)).collect();
            count(
                &mut drift,
                declared.difference(&installed).count(),
                "brew formula(e) to install",
            );
            let removals =
                installed.iter().filter(|f| ever.contains(*f) && !declared.contains(*f)).count();
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
    // Live media returns before anything else is considered, rather than
    // claiming first and letting the rest contribute edges like every
    // other state does.
    //
    // The reason is that the other branches read this session as a
    // developer's workstation and are each individually right about it:
    // there is no bootc deployment, `localhost/kuma:latest` is genuinely
    // not in podman storage, and the declaration genuinely has not been
    // built here. Together they produced `state: in-sync - nothing
    // pending` with a single `kuma init` edge, on the one medium where
    // the person reading is most likely to be a stranger and the answer
    // "nothing pending" is most wrong. Nothing else is worth saying here,
    // so nothing else is said.
    if obs.live {
        return Snapshot {
            state: "live",
            headline: "running from installer media; nothing here persists".into(),
            facts: live_facts(obs),
            // One edge, and it exists only because there is now a
            // published image to pull. While there was not, an empty
            // list was the honest answer: an affordance that fails is
            // worse than an absence that is stated.
            //
            // Bare `kuma install`, with no device path, because that is
            // the form a stranger can act on: it lists the disks it
            // found and asks. It is also a dry run, so following it
            // prints the plan rather than destroying anything.
            actions: vec![Action::new(
                "install",
                "kuma install",
                "write this system to a disk (it asks which, and shows the plan first)",
            )],
        };
    }
    let mut state: Option<(&'static str, String)> = None;
    let mut actions: Vec<Action> = Vec::new();
    let mut claim = |s: &'static str, headline: String| {
        state.get_or_insert((s, headline));
    };

    if let ConfigFact::Invalid(err) = &obs.config {
        claim(
            "config-invalid",
            format!("{} is invalid; nothing can build from it", obs.config_path),
        );
        actions.push(Action::new("edit", format!("$EDITOR {}", obs.config_path), err.clone()));
    }
    if let MachineFact::Kuma { staged: true, .. } = &obs.machine {
        claim("staged", "a new deployment is staged; reboot to apply".into());
        actions.push(reboot_action());
    }
    if let (ConfigFact::Loaded { .. }, None) = (&obs.config, &obs.image) {
        claim("not-built", format!("{} has never been built here", obs.config_path));
        actions.push(Action::new(
            "build",
            "kuma build",
            "build the system image from the declaration",
        ));
    }
    if matches!(&obs.config, ConfigFact::Loaded { .. })
        && obs.image.as_ref().is_some_and(|i| i.edited_after)
    {
        claim("edited", format!("{} changed after the last image build", obs.config_path));
        actions.push(Action::new("build", "kuma build", "rebuild the image to pick up the edit"));
    }
    // Below `edited` on purpose: when the declaration changed too, that is
    // the more familiar reason and both edges are the same `kuma build`.
    // The claim is "a different kuma", never "an older" one. Ordering two
    // of these strings would mean ranking a commit sha against another,
    // and running a deliberately older binary is a real thing to do.
    if obs.image.as_ref().is_some_and(|i| i.builder.as_deref() != Some(crate::VERSION)) {
        let built_by = match obs.image.as_ref().and_then(|i| i.builder.as_deref()) {
            Some(other) => format!("kuma {other}"),
            None => "a kuma older than the label".into(),
        };
        claim("stale-build", format!("the image was built by {built_by}, not the one running"));
        actions.push(Action::new("build", "kuma build", "rebuild with the kuma you are running"));
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
            claim("drifted", format!("machine drifted from its declaration: {}", drift.join(", ")));
            actions.push(Action::new(
                "sync",
                "kuma sync",
                "converge now (also runs at boot and daily)",
            ));
            actions.push(Action::new("diff", "kuma diff", "see the drift in detail"));
        }
    }
    if obs.image.is_some() && actions.is_empty() {
        match &obs.machine {
            MachineFact::BootcForeign => {
                claim("built", "image built; this bootc machine could switch to it".into());
                actions.push(Action::new(
                    "switch",
                    "kuma switch",
                    "adopt: point this machine at the kuma image (applies on reboot)",
                ));
                actions.push(Action::new(
                    "vm",
                    "kuma vm",
                    "boot the image in a disposable VM instead",
                ));
            }
            MachineFact::NotBootc => {
                claim("built", "image built; this machine can't run it directly".into());
                actions.push(Action::new("vm", "kuma vm", "boot the image in a QEMU VM"));
                actions.push(Action::new(
                    "iso",
                    "kuma iso",
                    "build an installer ISO for real hardware",
                ));
            }
            MachineFact::Kuma { .. } => {}
        }
    }
    if matches!(&obs.machine, MachineFact::Kuma { .. }) && actions.is_empty() {
        claim("in-sync", "machine matches its declaration; nothing pending".into());
        actions.push(Action::new(
            "update",
            "kuma update",
            "pull the latest base image and rebuild",
        ));
        actions.push(Action::new("doctor", "kuma doctor", "deeper machine health checks"));
    }
    match &obs.config {
        ConfigFact::Missing => {
            claim("no-config", format!("no {} here; nothing declared yet", obs.config_path));
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

    let (state, headline) = state.unwrap_or(("in-sync", "nothing pending".into()));
    Snapshot { state, headline, facts: facts_of(obs), actions }
}

/// The three facts that mean something on installer media.
///
/// The ordinary `machine` line would say "not a bootc machine
/// (build/test workspace)", which is true of the mechanism and wrong
/// about the situation.
fn live_facts(obs: &Observed) -> [(&'static str, String); 3] {
    let config = match &obs.config {
        ConfigFact::Loaded { rpm, flatpak, brew } | ConfigFact::Baked { rpm, flatpak, brew } => {
            format!("{rpm} rpm, {flatpak} flatpak, {brew} brew; what this media would install")
        }
        _ => "none carried".to_string(),
    };
    // The same three names the other states use, because `json_of` maps
    // them by position and docs/agents.md promises those keys. Only what
    // they say changes.
    [
        ("config", config),
        ("image", "this media's own filesystem, read-only; edits live in RAM".into()),
        ("machine", format!("none yet; `kuma install` writes one from {}", crate::PUBLISHED_IMAGE)),
    ]
}

fn facts_of(obs: &Observed) -> [(&'static str, String); 3] {
    let config = match &obs.config {
        ConfigFact::Missing => format!("none ({} not found)", obs.config_path),
        ConfigFact::Invalid(_) => format!("{} (invalid)", obs.config_path),
        ConfigFact::Loaded { rpm, flatpak, brew } => {
            format!("{}: {rpm} rpm, {flatpak} flatpak, {brew} brew declared", obs.config_path)
        }
        ConfigFact::Baked { rpm, flatpak, brew } => format!(
            "{BAKED_CONFIG} (this machine's baked declaration): {rpm} rpm, {flatpak} flatpak, {brew} brew"
        ),
    };
    let image = match &obs.image {
        None => format!("none built ({} not in podman storage)", crate::DEFAULT_TAG),
        Some(image) => {
            let edited = if image.edited_after { ", before the last config edit" } else { "" };
            // Named only when it is not this kuma. Stamping every line with
            // the version you are already running says nothing.
            let by = match image.builder.as_deref() {
                Some(b) if b == crate::VERSION => String::new(),
                Some(other) => format!(", by kuma {other}"),
                None => ", by an unrecorded kuma".into(),
            };
            format!("{}, built {} ago{edited}{by}", crate::DEFAULT_TAG, human_age(image.age_secs))
        }
    };
    let machine = match &obs.machine {
        MachineFact::NotBootc => "not a bootc machine (build/test workspace)".into(),
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
        Observed { config_path: "kuma.toml".into(), config, image, machine, live: false }
    }

    /// What a stranger sees. Every fact a live session presents to the
    /// ordinary classifier is individually true and collectively says
    /// "developer's laptop, nothing to do": no bootc deployment, no
    /// image in podman storage, a declaration never built here. That
    /// combination used to render as `in-sync - nothing pending` with a
    /// `kuma init` edge, on the one medium whose whole purpose is that
    /// something is pending.
    #[test]
    fn live_media_outranks_every_reading_of_a_workspace() {
        let mut obs = workspace(
            ConfigFact::Baked { rpm: 6, flatpak: 7, brew: 7 },
            None,
            MachineFact::NotBootc,
        );
        assert_eq!(classify(&obs).state, "in-sync");

        obs.live = true;
        let snap = classify(&obs);
        assert_eq!(snap.state, "live");
        assert!(snap.headline.contains("nothing here persists"));
        // The one move a live session has. It was deliberately absent
        // while kuma published nothing to install; now that it does, the
        // medium whose whole purpose is that something is pending has to
        // name it. Bare, because a stranger cannot be expected to know a
        // device path, and `kuma install --disk /dev/???` is not a move
        // anyone can take.
        assert_eq!(snap.actions.len(), 1);
        assert_eq!(snap.actions[0].cmd, "kuma install");
        assert!(!snap.facts.iter().any(|(_, d)| d.contains("build/test workspace")));
        // The keys docs/agents.md promises, whatever the state.
        assert_eq!(snap.facts.map(|(name, _)| name), ["config", "image", "machine"]);
    }

    /// The bug this exists to prevent: capture's dry run printed `kuma
    /// capture --yes` after being pointed at a declaration with --config,
    /// and running it either failed from a directory with no kuma.toml or
    /// wrote into an unrelated one.
    #[test]
    fn config_flag_rides_along_on_verbs_that_read_the_declaration() {
        let flag = Some(" --config /home/x/kuma.toml");
        assert_eq!(
            apply_config_flag("kuma capture --yes".into(), flag),
            "kuma capture --yes --config /home/x/kuma.toml"
        );
        assert_eq!(
            apply_config_flag("kuma build".into(), flag),
            "kuma build --config /home/x/kuma.toml"
        );
    }

    /// Verbs that never resolve a declaration must stay bare: `init`
    /// always writes ./kuma.toml, so a --config on it would be a lie.
    #[test]
    fn config_flag_stays_off_verbs_that_ignore_it() {
        let flag = Some(" --config /home/x/kuma.toml");
        for cmd in ["kuma init", "kuma switch", "kuma vm", "kuma sync", "kuma doctor"] {
            assert_eq!(apply_config_flag(cmd.into(), flag), cmd, "{cmd} should not carry --config");
        }
    }

    /// Not every affordance is a kuma command, and the ones that aren't
    /// take no such flag.
    #[test]
    fn config_flag_stays_off_foreign_commands() {
        let flag = Some(" --config /home/x/kuma.toml");
        for cmd in ["sudo systemctl reboot", "$EDITOR kuma.toml", "kumactl build"] {
            assert_eq!(apply_config_flag(cmd.into(), flag), cmd);
        }
    }

    /// A discovered declaration resolves the same way for whoever runs the
    /// hint, so it stays off the line.
    #[test]
    fn discovered_declaration_prints_no_flag() {
        assert_eq!(apply_config_flag("kuma build".into(), None), "kuma build");
    }

    #[test]
    fn awkward_paths_survive_the_copy_paste() {
        assert_eq!(shell_quote(Path::new("/home/x/kuma.toml")), "/home/x/kuma.toml");
        assert_eq!(shell_quote(Path::new("/home/my box/kuma.toml")), "'/home/my box/kuma.toml'");
        assert_eq!(shell_quote(Path::new("/tmp/it's.toml")), r"'/tmp/it'\''s.toml'");
    }
    fn loaded() -> ConfigFact {
        ConfigFact::Loaded { rpm: 2, flatpak: 1, brew: 0 }
    }
    /// Built by the running kuma unless a test says otherwise, so the
    /// existing cases keep testing what they were written to test.
    fn image(edited_after: bool) -> Option<ImageFact> {
        Some(ImageFact {
            id: "sha256:aaa".into(),
            age_secs: 60,
            edited_after,
            builder: Some(crate::VERSION.to_string()),
        })
    }

    fn image_built_by(builder: Option<&str>) -> Option<ImageFact> {
        Some(ImageFact {
            id: "sha256:aaa".into(),
            age_secs: 60,
            edited_after: false,
            builder: builder.map(str::to_string),
        })
    }
    fn kuma_machine(staged: bool, drift: Vec<String>, deployed_id: Option<&str>) -> MachineFact {
        MachineFact::Kuma { staged, drift, deployed_id: deployed_id.map(str::to_string) }
    }

    /// The gap this closes: an unchanged declaration built by a different
    /// binary produced an image the probe called in-sync, correctly by its
    /// own definition and uselessly in practice. Cost real time twice in
    /// one day before the image carried the answer.
    #[test]
    fn an_image_built_by_another_kuma_is_not_in_sync() {
        let snap = classify(&workspace(
            loaded(),
            image_built_by(Some("0.3.0 (deadbee 2026-08-01)")),
            kuma_machine(false, vec![], Some("sha256:aaa")),
        ));
        assert_eq!(snap.state, "stale-build");
        assert!(snap.headline.contains("0.3.0"), "name what built it: {}", snap.headline);
        assert!(snap.actions.iter().any(|a| a.cmd == "kuma build"));
    }

    /// Images predating the label are the common case on any machine that
    /// existed before it, and "unknown" is the answer, not an exemption:
    /// whatever built them was not this kuma.
    #[test]
    fn an_unlabelled_image_counts_as_built_by_another_kuma() {
        let snap = classify(&workspace(
            loaded(),
            image_built_by(None),
            kuma_machine(false, vec![], Some("sha256:aaa")),
        ));
        assert_eq!(snap.state, "stale-build");
        assert!(snap.actions.iter().any(|a| a.cmd == "kuma build"));
    }

    /// The half that matters more, since a check that always fires is one
    /// people learn to ignore.
    #[test]
    fn an_image_this_kuma_built_stays_quiet() {
        let snap = classify(&workspace(
            loaded(),
            image_built_by(Some(crate::VERSION)),
            kuma_machine(false, vec![], Some("sha256:aaa")),
        ));
        assert_eq!(snap.state, "in-sync");
        assert!(!snap.facts.iter().any(|(_, d)| d.contains("by kuma")), "{:?}", snap.facts);
    }

    /// An edit is the more familiar reason to rebuild and the edge is the
    /// same, so it names the state. Pinned because the ordering reads like
    /// an accident and is not one.
    #[test]
    fn an_edit_outranks_the_builder_check() {
        let mut img = image(true).unwrap();
        img.builder = Some("0.3.0 (deadbee 2026-08-01)".into());
        let snap = classify(&workspace(loaded(), Some(img), kuma_machine(false, vec![], None)));
        assert_eq!(snap.state, "edited");
    }

    #[test]
    fn staged_outranks_edits_and_both_edges_appear() {
        let snap = classify(&workspace(loaded(), image(true), kuma_machine(true, vec![], None)));
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
