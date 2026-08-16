//! Day-2 visibility: `kuma diff` shows drift between the declaration and
//! this machine, `kuma doctor` checks the machine itself. Both are
//! read-only about the machine — convergence stays with the boot services
//! and timers, so a diff is safe to run out of curiosity. The one thing
//! doctor writes is kuma's own deployed-image stamp, which it refreshes
//! from the truth it alone (having root) can see.

use crate::config::Config;
use crate::host::{host_output, host_output_any};
use crate::snapshot;
use crate::state::{action_json, print_actions, Action};
use anyhow::{bail, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

const BREW: &str = "/home/linuxbrew/.linuxbrew/bin/brew";
const BREW_CELLAR: &str = "/home/linuxbrew/.linuxbrew/Cellar";
const BREW_STATE: &str = "/home/linuxbrew/.linuxbrew/.kuma-brews";
/// Written by the flatpak sync: the apps the declaration installed, which
/// are the only system apps convergence considers its own to remove.
const FLATPAK_STATE: &str = "/var/lib/kuma/flatpaks-installed";
/// The declaration this image was built from, baked in at build time.
/// Doctor reads it to learn what the machine was meant to do, which is a
/// better question than what its unit files happen to say.
const BAKED_CONFIG: &str = "/usr/lib/kuma/kuma.toml";

/// One drift observation: what would change ("add"/"remove"/"mismatch"),
/// which item, and the note that carries its consequence or cure.
struct DiffEntry {
    change: &'static str,
    item: String,
    note: String,
}

/// A [packages]/[services] section of the diff. `skipped` is a section-
/// level caveat ("flatpak unavailable") — while one is present, "no
/// drift" would be a claim the observation can't back.
struct DiffSection {
    name: &'static str,
    entries: Vec<DiffEntry>,
    skipped: Option<String>,
}

/// What this machine actually has, gathered in one pass. `diff` renders
/// it as drift and `capture` filters it into declaration edits, and both
/// read this same observation on purpose: a diff that threatens to remove
/// something capture won't offer to keep is the worst bug this pair could
/// have, and sharing the source makes it unrepresentable rather than
/// merely unlikely.
///
/// `None` means "couldn't look" (tool absent, brew not bootstrapped),
/// which is not the same as "nothing there" and never reads as drift.
pub(crate) struct Machine {
    pub rpm: Option<BTreeSet<String>>,
    pub flatpak_system: Option<BTreeSet<String>>,
    /// `flatpak --user` installs: the documented imperative escape hatch.
    /// Convergence never touches these, and capture only takes one when
    /// it is named.
    pub flatpak_user: BTreeSet<String>,
    /// Apps the sync has ever installed (its state file): the only system
    /// apps convergence considers its own to remove. A store installs
    /// system-wide too, so scope alone can't tell whose an app is.
    pub flatpak_state: BTreeSet<String>,
    pub brew_installed: Option<BTreeSet<String>>,
    /// Explicit installs only. A dependency is baggage that arrived with
    /// a choice, not a choice.
    pub brew_leaves: BTreeSet<String>,
    /// Formulae the sync has ever installed (its state file): the only
    /// ones convergence considers its own to remove.
    pub brew_state: BTreeSet<String>,
}

/// Ask the machine what it has. Read-only, and every query is allowed to
/// fail: an answer nobody could observe must never turn into a claim.
pub(crate) fn observe(config: &Config) -> Machine {
    // One rpm -qa beats a spawn per declared package, and rpm being
    // absent reads as "nothing to check" rather than "everything is
    // missing". Nothing declares rpm, nothing to ask.
    let ask = |args: &[&str]| host_output(args).ok().map(|out| owned_set(&out));

    let rpm = (!config.packages.rpm.is_empty())
        .then(|| ask(&["rpm", "-qa", "--qf", "%{NAME}\n"]))
        .flatten();
    let flatpak_system = ask(&["flatpak", "list", "--system", "--app", "--columns=application"]);
    let flatpak_user =
        ask(&["flatpak", "list", "--user", "--app", "--columns=application"]).unwrap_or_default();

    let ask_brew = !config.packages.brew.is_empty() || Path::new(BREW).exists();
    let brew_installed = ask_brew.then(|| ask(&[BREW, "list", "--formula", "-1"])).flatten();
    // Nothing installed, nothing to classify.
    let brew_leaves = brew_installed
        .as_ref()
        .filter(|installed| !installed.is_empty())
        .and_then(|installed| leaves_from_receipts(installed).or_else(|| ask(&[BREW, "leaves"])))
        .unwrap_or_default();
    let brew_state = owned_set(&std::fs::read_to_string(BREW_STATE).unwrap_or_default());
    let flatpak_state = owned_set(&std::fs::read_to_string(FLATPAK_STATE).unwrap_or_default());

    Machine {
        rpm,
        flatpak_system,
        flatpak_user,
        flatpak_state,
        brew_installed,
        brew_leaves,
        brew_state,
    }
}

/// Leaves without asking brew: the installed formulae nothing else
/// depends on.
///
/// `brew leaves` resolves the dependency graph and costs about 1.2s,
/// more than every other query in `observe` put together. The same graph
/// is already on disk, one `runtime_dependencies` list per formula, and
/// reading all of them takes about 30ms. Same definition, same answer,
/// forty times faster. (`state.rs` already reads the Cellar rather than
/// paying for `brew list`, so this is the established trade here.)
///
/// None whenever the receipts can't be trusted (a formula with no
/// readable receipt, or a shape this doesn't recognise) and the caller
/// falls back to asking brew. That fallback is the price of reading a
/// format brew owns and could change.
fn leaves_from_receipts(installed: &BTreeSet<String>) -> Option<BTreeSet<String>> {
    let mut depended_on: BTreeSet<String> = BTreeSet::new();
    for name in installed {
        let mut read_one = false;
        for version in std::fs::read_dir(Path::new(BREW_CELLAR).join(name)).ok()? {
            let receipt = version.ok()?.path().join("INSTALL_RECEIPT.json");
            let Ok(text) = std::fs::read_to_string(&receipt) else { continue };
            let json: serde_json::Value = serde_json::from_str(&text).ok()?;
            for dep in json.get("runtime_dependencies")?.as_array()? {
                let full = dep.get("full_name")?.as_str()?;
                // A tap-qualified dependency (owner/tap/tool) has to match
                // the bare name `brew list` reports.
                depended_on.insert(full.rsplit('/').next().unwrap_or(full).to_string());
            }
            read_one = true;
        }
        if !read_one {
            return None;
        }
    }
    Some(installed.difference(&depended_on).cloned().collect())
}

/// Something this machine has that the declaration does not name.
pub(crate) struct Candidate {
    /// The [packages] list it would join. Only ever "flatpak" or "brew";
    /// capture.rs carries the reasoning for what can never be here.
    pub list: &'static str,
    pub item: String,
    /// Convergence removes this on its next run, so declaring it is the
    /// only way to keep it. The rest are merely unreproducible.
    pub doomed: bool,
    /// Declaring it changes what it *is* rather than just writing it
    /// down: a --user flatpak becomes a system one, installed for every
    /// account and owned by convergence from then on. Never in the
    /// default set.
    pub promotes: bool,
}

/// The undeclared half of the comparison, which `diff` reports and
/// `capture` offers to keep. Pure over the observation, so the rules for
/// what counts as a choice are testable without a machine to run on.
///
/// rpm is deliberately absent, and not because it is hard: on a bootc
/// machine you *cannot* imperatively install one, so [packages].rpm is
/// already declarative by construction and there is nothing to capture.
/// The mutable edge is exactly flatpak and brew, which is exactly this.
pub(crate) fn candidates(config: &Config, machine: &Machine) -> Vec<Candidate> {
    let flatpak: BTreeSet<&str> = config.packages.flatpak.iter().map(String::as_str).collect();
    let brew: BTreeSet<&str> = config.packages.brew.iter().map(String::as_str).collect();
    let mut out: Vec<Candidate> = Vec::new();

    if let Some(installed) = &machine.flatpak_system {
        for app in installed.iter().filter(|a| !flatpak.contains(a.as_str())) {
            // Same rule as brew: convergence takes back only what it
            // installed. An app a store put here system-wide is the
            // owner's, undeclared but in no danger.
            let doomed = machine.flatpak_state.contains(app);
            out.push(Candidate { list: "flatpak", item: app.clone(), doomed, promotes: false });
        }
    }

    if let Some(installed) = &machine.brew_installed {
        for f in installed.iter().filter(|f| !brew.contains(f.as_str())) {
            // Convergence takes back only what it installed; everything
            // else on the machine is the owner's, declared or not. A
            // dependency is neither, so it is never offered.
            let doomed = machine.brew_state.contains(f);
            if !doomed && !machine.brew_leaves.contains(f) {
                continue;
            }
            out.push(Candidate { list: "brew", item: f.clone(), doomed, promotes: false });
        }
    }

    for app in &machine.flatpak_user {
        if flatpak.contains(app.as_str())
            || machine.flatpak_system.as_ref().is_some_and(|s| s.contains(app))
        {
            continue;
        }
        out.push(Candidate { list: "flatpak", item: app.clone(), doomed: false, promotes: true });
    }

    // Urgent first (declare it or lose it), opt-in last, alphabetical
    // inside each band so two runs read the same.
    out.sort_by(|a, b| {
        (!a.doomed, a.promotes, a.list, &a.item).cmp(&(!b.doomed, b.promotes, b.list, &b.item))
    });
    out
}

/// Three-way, because that's how changes actually flow: kuma.toml is the
/// truth, the image carries a baked copy of the declaration, and the
/// machine converges to the IMAGE's copy — so config edits that were never
/// built show up here as "image declaration behind kuma.toml", not as
/// drift the next convergence run would fix.
pub fn diff(config: &Config, config_path: &Path, json: bool) -> Result<()> {
    let machine = observe(config);
    let found = candidates(config, &machine);
    let mut sections: Vec<DiffSection> = Vec::new();
    let mut stale_image = false;

    // rpm lives in the image itself; missing means the declaration was
    // never built (or the build was never switched to).
    let mut entries = Vec::new();
    if let Some(installed) = &machine.rpm {
        entries = config
            .packages
            .rpm
            .iter()
            .filter(|pkg| !installed.contains(pkg.as_str()))
            .map(|pkg| DiffEntry {
                change: "add",
                item: pkg.to_string(),
                note: "declared, missing from the running image".into(),
            })
            .collect();
    }
    sections.push(DiffSection { name: "packages.rpm", entries, skipped: None });

    let declared: BTreeSet<&str> = config.packages.flatpak.iter().map(String::as_str).collect();
    let mut entries = Vec::new();
    let mut skipped = None;
    match &machine.flatpak_system {
        Some(installed) => {
            for app in declared.iter().filter(|a| !installed.contains(**a)) {
                entries.push(DiffEntry {
                    change: "add",
                    item: app.to_string(),
                    note: "declared, not installed (convergence installs it)".into(),
                });
            }
            for c in found.iter().filter(|c| c.list == "flatpak" && c.doomed) {
                entries.push(DiffEntry {
                    change: "remove",
                    item: c.item.clone(),
                    note: "installed, not declared (convergence removes it)".into(),
                });
            }
        }
        None => skipped = Some("flatpak unavailable, skipped".to_string()),
    }
    stale_image |= image_list_stale("/usr/lib/kuma/flatpaks", &declared);
    sections.push(DiffSection { name: "packages.flatpak", entries, skipped });

    let declared: BTreeSet<&str> = config.packages.brew.iter().map(String::as_str).collect();
    let mut entries = Vec::new();
    let mut skipped = None;
    if !declared.is_empty() || Path::new(BREW).exists() {
        match &machine.brew_installed {
            Some(installed) => {
                for f in declared.iter().filter(|f| !installed.contains(**f)) {
                    entries.push(DiffEntry {
                        change: "add",
                        item: f.to_string(),
                        note: "declared, not installed (convergence installs it)".into(),
                    });
                }
                for c in found.iter().filter(|c| c.list == "brew" && c.doomed) {
                    entries.push(DiffEntry {
                        change: "remove",
                        item: c.item.clone(),
                        note: "no longer declared (convergence removes it)".into(),
                    });
                }
            }
            None => skipped = Some("brew not bootstrapped yet; first boot installs it".to_string()),
        }
    }
    stale_image |= image_list_stale("/usr/lib/kuma/brews", &declared);
    sections.push(DiffSection { name: "packages.brew", entries, skipped });

    // Ad-hoc installs are the non-doomed half of the same set:
    // convergence leaves them alone, so the only thing undeclared costs
    // them is that a rebuild elsewhere wouldn't reproduce them. Per-user
    // flatpaks are excluded because declaring one would change what it
    // is, and diff has never reported them as anything to reconcile.
    let adhoc_brews: Vec<String> =
        found.iter().filter(|c| c.list == "brew" && !c.doomed).map(|c| c.item.clone()).collect();
    let adhoc_flatpaks: Vec<String> = found
        .iter()
        .filter(|c| c.list == "flatpak" && !c.doomed && !c.promotes)
        .map(|c| c.item.clone())
        .collect();

    // Service state is machine state (an /etc overlay change survives image
    // updates), so the cure is systemctl, not a rebuild — name it when it
    // plainly applies.
    let mut entries = Vec::new();
    for svc in &config.services.enable {
        let state = unit_state(svc);
        if state != "enabled" && state != "alias" {
            let cure = if state == "disabled" {
                format!("; `sudo systemctl enable {svc}` reconciles")
            } else {
                String::new()
            };
            entries.push(DiffEntry {
                change: "mismatch",
                item: svc.clone(),
                note: format!("declared enable, currently {state}{cure}"),
            });
        }
    }
    for svc in &config.services.disable {
        if unit_state(svc) == "enabled" {
            entries.push(DiffEntry {
                change: "mismatch",
                item: svc.clone(),
                note: format!("declared disable, currently enabled; `sudo systemctl disable {svc}` reconciles"),
            });
        }
    }
    sections.push(DiffSection { name: "services", entries, skipped: None });

    let drift = sections.iter().any(|s| !s.entries.is_empty());
    let converge_hint = sections
        .iter()
        .any(|s| s.name != "packages.rpm" && s.name != "services" && !s.entries.is_empty());
    // Drift is a fork, not an error: everything undeclared can be kept by
    // writing it down as easily as it can be erased by converging. When
    // convergence is about to destroy something, the keeping edge goes
    // first, because that is the one with a deadline.
    let mut capture = (!found.iter().all(|c| c.promotes)).then(|| {
        Action::new("capture", "kuma capture", "keep them: declare what this machine already runs")
    });
    let mut actions: Vec<Action> = Vec::new();
    if found.iter().any(|c| c.doomed) {
        actions.extend(capture.take());
    }
    if stale_image {
        actions.push(Action::new(
            "build",
            "kuma build",
            "bake the edit; `kuma switch` and reboot carry it to the machine",
        ));
    } else if converge_hint {
        actions.push(Action::new(
            "sync",
            "kuma sync",
            "converge now; otherwise the boot/daily run picks this up",
        ));
    }
    actions.extend(capture);

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&diff_json(
                config_path,
                &sections,
                &adhoc_brews,
                &adhoc_flatpaks,
                stale_image,
                drift,
                &actions
            ))?
        );
        return Ok(());
    }

    let observed_all = sections.iter().all(|s| s.skipped.is_none());
    for section in &sections {
        if section.entries.is_empty() && section.skipped.is_none() {
            continue;
        }
        println!("{}", section.name);
        for e in &section.entries {
            let mark = match e.change {
                "add" => '+',
                "remove" => '-',
                _ => '!',
            };
            println!("  {mark} {}  {}", e.item, e.note);
        }
        if let Some(reason) = &section.skipped {
            println!("  ({reason})");
        }
    }
    if !adhoc_flatpaks.is_empty() {
        println!("Ad-hoc flatpaks, kept as yours: {}", adhoc_flatpaks.join(", "));
    }
    if !adhoc_brews.is_empty() {
        println!("Ad-hoc brews, kept as yours: {}", adhoc_brews.join(", "));
    }
    if stale_image {
        println!("\nThe image's baked declaration is behind {}.", config_path.display());
    } else if !actions.is_empty() {
        println!();
    }
    if !actions.is_empty() {
        print_actions(&actions);
    }
    if !drift && !stale_image && observed_all {
        println!("No drift: this machine matches {}.", config_path.display());
    }
    Ok(())
}

fn diff_json(
    config_path: &Path,
    sections: &[DiffSection],
    adhoc_brews: &[String],
    adhoc_flatpaks: &[String],
    stale_image: bool,
    drift: bool,
    actions: &[Action],
) -> serde_json::Value {
    serde_json::json!({
        "config": config_path.display().to_string(),
        "drift": drift,
        "image_declaration_stale": stale_image,
        "sections": sections.iter()
            .filter(|s| !s.entries.is_empty() || s.skipped.is_some())
            .map(|s| serde_json::json!({
                "name": s.name,
                "skipped": s.skipped,
                "entries": s.entries.iter().map(|e| serde_json::json!({
                    "change": e.change, "item": e.item, "note": e.note,
                })).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
        "adhoc_brews": adhoc_brews,
        "adhoc_flatpaks": adhoc_flatpaks,
        "actions": actions.iter().map(action_json).collect::<Vec<_>>(),
    })
}

pub(crate) fn to_set(text: &str) -> BTreeSet<&str> {
    text.lines().map(str::trim).filter(|l| !l.is_empty()).collect()
}

/// The same, owned: an observation outlives the command output it was
/// read from, because two verbs consume it.
fn owned_set(text: &str) -> BTreeSet<String> {
    to_set(text).into_iter().map(str::to_string).collect()
}

/// The baked copy at /usr/lib/kuma/<list> is what convergence follows;
/// absent (not a kuma image, list never declared) means nothing to lag.
fn image_list_stale(path: &str, declared: &BTreeSet<&str>) -> bool {
    match std::fs::read_to_string(path) {
        Ok(text) => to_set(&text) != *declared,
        Err(_) => false,
    }
}

/// Everything doctor asks about a unit, for every unit, in one systemctl
/// call. Each spawn costs about 140ms and there were one or two per unit.
///
/// Keyed by the unit rather than positional: `--value` is terser but
/// separates its answers with blank lines, so lining them up with the
/// units asked for means relying on a shape systemd never promised, and
/// getting it wrong pairs one unit's verdict with another's name. Asking
/// for Id as well makes every answer say who it belongs to, which makes
/// that mistake unrepresentable rather than merely unlikely.
#[derive(Default)]
struct UnitFacts {
    active: String,
    result: String,
}

fn unit_facts(units: &[&str]) -> BTreeMap<String, UnitFacts> {
    let mut facts: BTreeMap<String, UnitFacts> = BTreeMap::new();
    if units.is_empty() {
        return facts;
    }
    let mut args = vec!["systemctl", "show", "-p", "Id", "-p", "ActiveState", "-p", "Result"];
    args.extend(units);
    let text = host_output_any(&args).unwrap_or_default();
    let mut current = String::new();
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else { continue };
        match key {
            "Id" => current = value.to_string(),
            "ActiveState" => facts.entry(current.clone()).or_default().active = value.to_string(),
            "Result" => facts.entry(current.clone()).or_default().result = value.to_string(),
            _ => {}
        }
    }
    facts
}

fn unit_state(unit: &str) -> String {
    // is-enabled exits non-zero for disabled units but still names the
    // state on stdout, so read stdout regardless of exit status.
    let state = host_output_any(&["systemctl", "is-enabled", unit]).unwrap_or_default();
    if state.is_empty() {
        "not found".into()
    } else {
        state
    }
}

enum Grade {
    Ok,
    Warn,
    Fail,
}

/// One doctor check's verdict, held rather than printed so the same run
/// can render as text or as JSON for agents.
struct Finding {
    grade: Grade,
    name: String,
    detail: String,
    fix: Option<Action>,
}

/// Machine health, no config needed: the deployment, the convergence
/// machinery, and the hardware basics a desktop lives on. Read-only.
/// A finding that has a cure carries it as an action — a diagnosis
/// without its next command is a dead end.
/// Does this fstab still have an active `/` entry, the one kuma-fstab-sync
/// exists to comment out? The field test rather than a regex over the line:
/// "root" appears inside subvolume names and inside /var/roothome, and a
/// loose match there decides whether a real failure gets excused.
fn fstab_declares_root(text: &str) -> bool {
    text.lines().any(|line| {
        let mut fields = line.split_whitespace();
        match (fields.next(), fields.next()) {
            (Some(dev), Some(target)) => !dev.starts_with('#') && target == "/",
            _ => false,
        }
    })
}

/// Is this installer media rather than a machine?
///
/// A marker file, not an inference. The tempting test is "a kuma image
/// that is not ostree-booted", but that is also true of `podman run` on
/// the image and of an image being built, and a check that quietly
/// changes its mind about what it is looking at is worse than one that
/// asks. The live layer writes this; nothing else does.
fn live_media() -> bool {
    Path::new(crate::liveiso::LIVE_MARKER).exists()
}

/// A kuma machine that kuma actually converges: the image is kuma's AND
/// it was booted as a deployment. The second half is what separates a
/// running machine from live media or a container of the same image, and
/// it is the condition kuma's own boot units already use.
fn booted_kuma_machine() -> bool {
    Path::new("/usr/lib/kuma").is_dir() && Path::new("/run/ostree-booted").exists()
}

pub fn doctor(json: bool) -> Result<()> {
    let mut findings: Vec<Finding> = Vec::new();
    let mut report = |grade: Grade, name: &str, detail: String, fix: Option<Action>| {
        findings.push(Finding { grade, name: name.to_string(), detail, fix });
    };

    let live = live_media();
    // Live media has no deployment by design, so asking about one can
    // only produce a warning about a fact rather than a problem.
    if !live {
        check_deployment(&mut report);
    }

    match host_output_any(&["systemctl", "--failed", "--plain", "--no-legend"]) {
        Ok(out) if out.is_empty() => {
            report(Grade::Ok, "units", "no failed systemd units".into(), None)
        }
        Ok(out) => {
            let names: Vec<&str> =
                out.lines().filter_map(|l| l.split_whitespace().next()).collect();
            // Anaconda writes a `/` line into fstab that composefs can't
            // remount, so this one fails on every boot of an installed
            // system — real, but cosmetic; don't bury real failures in it.
            //
            // Narrowed to the cause rather than the unit name. kuma-fstab-sync
            // now comments that line out on first boot, so a machine can fail
            // this unit for the known reason (line still active, converger has
            // not run yet) or for an unknown one (line already gone, so
            // whatever broke is not this). Excusing the unit by name would
            // keep calling the second case benign forever, which is how a
            // workaround outlives its bug and starts hiding news.
            let root_line_active = std::fs::read_to_string("/etc/fstab")
                .map(|f| fstab_declares_root(&f))
                .unwrap_or(false);
            let (benign, real): (Vec<&str>, Vec<&str>) = names.iter().partition(|n| {
                **n == "systemd-remount-fs.service"
                    && Path::new("/run/ostree-booted").exists()
                    && root_line_active
            });
            if !real.is_empty() {
                let fix = Action::new(
                    "inspect",
                    format!("systemctl status {}", real[0]),
                    "see why it failed",
                );
                report(Grade::Fail, "units", format!("failed: {}", real.join(", ")), Some(fix));
            }
            if !benign.is_empty() {
                report(
                    Grade::Warn,
                    "units",
                    "systemd-remount-fs failed, known-benign: Anaconda's fstab `/` line can't be remounted over composefs".into(),
                    Some(Action::new(
                        "reboot",
                        "sudo systemctl reboot".to_string(),
                        "kuma-fstab-sync has commented the line out; the unit stops failing next boot",
                    )),
                );
            }
        }
        Err(_) => report(Grade::Warn, "units", "systemctl unavailable".into(), None),
    }

    // Three different places kuma can be asked how a machine is doing,
    // and only one of them has a machine.
    //
    // The checks below all grade a system kuma converges: timers that
    // should be running, a bootloader that should count boot attempts,
    // an /etc that should match the image. Live media runs none of that
    // on purpose and an unbooted image has not started yet, so reporting
    // those as failures states a deliberate design as a fault. That
    // matters most exactly where it looked worst: the first `kuma
    // doctor` a newcomer runs is the one inside the live session.
    if live {
        report(
            Grade::Ok,
            "live media",
            "running from installer media; nothing converges and nothing persists".into(),
            None,
        );
    } else if booted_kuma_machine() {
        check_convergence(&mut report);
        check_snapshots(&mut report);
        check_boot_health(&mut report);
        check_encryption(&mut report);
        check_etc_drift(&mut report);
    } else if Path::new("/usr/lib/kuma").is_dir() {
        report(
            Grade::Warn,
            "kuma",
            "a kuma image, but not booted as a deployment; convergence checks skipped".into(),
            None,
        );
    } else {
        report(
            Grade::Warn,
            "kuma",
            "not running a kuma image; convergence checks skipped".into(),
            None,
        );
    }

    check_gpu(&mut report);
    check_build_leftovers(&mut report);

    match host_output(&["df", "-h", "--output=pcent,avail", "/sysroot"])
        .or_else(|_| host_output(&["df", "-h", "--output=pcent,avail", "/"]))
    {
        Ok(out) => {
            let fields: Vec<&str> = out.lines().nth(1).unwrap_or("").split_whitespace().collect();
            let pcent: u32 =
                fields.first().and_then(|p| p.trim_end_matches('%').parse().ok()).unwrap_or(0);
            let detail = format!("{}% used, {} free", pcent, fields.get(1).unwrap_or(&"?"));
            if pcent >= 90 {
                let fix = Action::new("clean", "kuma clean", "reclaim build leftovers first");
                report(Grade::Warn, "disk", detail, Some(fix));
            } else {
                report(Grade::Ok, "disk", detail, None);
            }
        }
        Err(_) => report(Grade::Warn, "disk", "df unavailable".into(), None),
    }

    let fails = findings.iter().filter(|f| matches!(f.grade, Grade::Fail)).count();
    let warns = findings.iter().filter(|f| matches!(f.grade, Grade::Warn)).count();
    if json {
        println!("{}", serde_json::to_string_pretty(&doctor_json(&findings))?);
    } else {
        for f in &findings {
            let mark = match f.grade {
                Grade::Ok => "ok  ",
                Grade::Warn => "warn",
                Grade::Fail => "FAIL",
            };
            println!("{mark}  {}: {}", f.name, f.detail);
            if let Some(fix) = &f.fix {
                println!("      → {}   {}", fix.cmd, fix.why);
            }
        }
        println!();
        match (fails, warns) {
            (0, 0) => println!("All checks passed."),
            (0, _) => println!("{warns} warning(s)."),
            _ => {}
        }
    }
    // Non-zero exit on failed checks either way; the JSON stays on stdout
    // and the summary rides the error, so scripts get both signals.
    if fails > 0 {
        bail!("{fails} check(s) failed, {warns} warning(s)");
    }
    Ok(())
}

fn doctor_json(findings: &[Finding]) -> serde_json::Value {
    let grade = |g: &Grade| match g {
        Grade::Ok => "ok",
        Grade::Warn => "warn",
        Grade::Fail => "fail",
    };
    serde_json::json!({
        "checks": findings.iter().map(|f| serde_json::json!({
            "grade": grade(&f.grade),
            "name": f.name,
            "detail": f.detail,
            "fix": f.fix.as_ref().map(action_json),
        })).collect::<Vec<_>>(),
        "summary": {
            "fails": findings.iter().filter(|f| matches!(f.grade, Grade::Fail)).count(),
            "warns": findings.iter().filter(|f| matches!(f.grade, Grade::Warn)).count(),
        },
    })
}

/// When a booted image stops being merely old and starts being worth
/// saying out loud. Fedora moves the kernel every couple of weeks, so a
/// month means at least one missed and usually two. Nothing here applies
/// anything: an image update replaces the whole OS and needs a reboot,
/// which is a human's call. Knowing is what a machine can automate.
const STALE_IMAGE_DAYS: u64 = 30;

/// Epoch seconds from an RFC 3339 timestamp. Everything after the
/// minutes is ignored — seconds, fractional seconds, and the offset —
/// because bootc writes UTC and none of it can move an answer measured
/// in days. The inverse of lock.rs's formatter, which hand-rolls the
/// same civil-date arithmetic in the other direction, and hand-rolled
/// here for the same reason: one timestamp is not worth a date crate.
fn epoch_from_rfc3339(stamp: &str) -> Option<i64> {
    let (date, time) = stamp.split_once('T')?;
    let mut fields = date.split('-');
    let year = leading_number(fields.next()?)?;
    let month = leading_number(fields.next()?)?;
    let day = leading_number(fields.next()?)?;
    let mut clock = time.split(':');
    let hour = leading_number(clock.next()?)?;
    let minute = leading_number(clock.next()?)?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 {
        return None;
    }
    // Howard Hinnant's days_from_civil, the inverse of the era shift in
    // lock.rs: March-based years, so the leap day lands last.
    let shifted = if month <= 2 { year - 1 } else { year };
    let era = shifted.div_euclid(400);
    let yoe = shifted - era * 400;
    let mp = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + hour * 3_600 + minute * 60)
}

/// The digits a field starts with: `"56Z"` is a seconds field, `"34"` a
/// minutes one, and both have to parse the same way.
fn leading_number(field: &str) -> Option<i64> {
    let digits: String = field.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// Whole days between an RFC 3339 timestamp and now. A timestamp in the
/// future (clock skew, a machine that booted before its RTC was set)
/// reads as zero rather than as an enormous unsigned number.
fn days_since(stamp: &str) -> Option<u64> {
    let then = epoch_from_rfc3339(stamp)?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs() as i64;
    Some(now.saturating_sub(then).max(0) as u64 / 86_400)
}

/// bootc status needs root; a sudo prompt out of `kuma doctor` is the
/// price of seeing the deployment at all, same as `kuma switch` pays.
fn check_deployment(report: &mut impl FnMut(Grade, &str, String, Option<Action>)) {
    let status = match host_output(&["sudo", "bootc", "status", "--format", "json"]) {
        Ok(out) => out,
        Err(_) => {
            report(
                Grade::Warn,
                "deployment",
                "bootc status unavailable (not a bootc system, or sudo declined)".into(),
                None,
            );
            return;
        }
    };
    let json: serde_json::Value = match serde_json::from_str(&status) {
        Ok(json) => json,
        Err(_) => {
            report(Grade::Warn, "deployment", "cannot parse bootc status".into(), None);
            return;
        }
    };
    let slot = |name: &str| json.get("status").and_then(|s| s.get(name));
    let image_of = |slot: &serde_json::Value| {
        slot.pointer("/image/image/image").and_then(|v| v.as_str()).map(str::to_string)
    };
    let digest_of = |slot: &serde_json::Value| {
        slot.pointer("/image/imageDigest").and_then(|v| v.as_str()).map(str::to_string)
    };

    let booted = slot("booted").cloned().unwrap_or_default();
    let staged = slot("staged").filter(|v| !v.is_null()).cloned();
    let rollback = slot("rollback").filter(|v| !v.is_null()).cloned();
    let mut detail = match image_of(&booted) {
        Some(image) => format!("booted on {image}"),
        None => {
            report(Grade::Warn, "deployment", "no booted bootc deployment".into(), None);
            return;
        }
    };
    let mut fix = None;
    if staged.is_some() {
        detail.push_str("; a new deployment is staged");
        fix = Some(Action::new("reboot", "sudo systemctl reboot", "boot the staged deployment"));
    }
    if rollback.is_some() {
        detail.push_str("; rollback available (kuma rollback)");
    }
    report(Grade::Ok, "deployment", detail, fix);

    // How old the running bytes are. On a machine that only updates when
    // asked, nothing else on it will ever mention this: the flatpak and
    // brew timers converge their own layers daily and say so, while the
    // kernel underneath them waits for a human. bootc carries the image's
    // creation time, and an image that never recorded one is no answer
    // rather than a wrong one.
    if let Some(days) =
        booted.pointer("/image/timestamp").and_then(|v| v.as_str()).and_then(days_since)
    {
        // A staged deployment is a newer image already waiting and the
        // reboot that takes it is named above; a second alarm for a fire
        // already out is how a health check teaches people to skim it.
        let stale = days >= STALE_IMAGE_DAYS && staged.is_none();
        let detail = match days {
            0 => "booted image was built today".to_string(),
            1 => "booted image is 1 day old".to_string(),
            days => format!("booted image is {days} days old"),
        };
        let fix = stale.then(|| {
            Action::new(
                "update",
                "kuma update",
                "recompose against the repos' current packages and rebuild",
            )
        });
        report(if stale { Grade::Warn } else { Grade::Ok }, "deployment", detail, fix);
    }

    // A build that was never switched to is the easy thing to forget.
    // Two storages are in play: the rootless build, and root's copy that
    // bootc actually deploys. Manifest digests don't survive the
    // save/load sync between them — only image IDs (config digests) do —
    // so compare IDs across storages and digests only within root's.
    //
    // Compare against the tag the machine actually deploys, not a
    // hardcoded one: a deployment that intentionally tracks another tag
    // (an overridden build, a test image) must never be told the
    // unrelated default build is "newer" — following that suggestion
    // would switch the machine off the tag its admin chose.
    let compared_tag = booted
        .pointer("/image/image/transport")
        .and_then(|v| v.as_str())
        .filter(|transport| *transport == "containers-storage")
        .and_then(|_| image_of(&booted))
        .unwrap_or_else(|| crate::DEFAULT_TAG.to_string());
    if let Ok(local_id) = crate::image_id(&compared_tag) {
        let root = host_output(&[
            "sudo",
            "podman",
            "image",
            "inspect",
            "--format",
            "{{.Id}} {{.Digest}}",
            &compared_tag,
        ])
        .unwrap_or_default();
        let (root_id, root_digest) = root.split_once(' ').unwrap_or(("", ""));
        let deployed: Vec<String> =
            [Some(&booted), staged.as_ref()].into_iter().flatten().filter_map(digest_of).collect();
        let deployment_current =
            !root_digest.is_empty() && deployed.iter().any(|d| d == root_digest);
        if local_id != root_id || (!deployed.is_empty() && !deployment_current) {
            let switch_cmd = if compared_tag == crate::DEFAULT_TAG {
                "kuma switch".to_string()
            } else {
                format!("kuma switch --tag {compared_tag}")
            };
            report(
                Grade::Warn,
                "deployment",
                format!("{compared_tag} is newer than the deployment"),
                Some(Action::new("switch", switch_cmd, "stage the newer build")),
            );
        }
        // Doctor holds root and the truth, so refresh the stamp the
        // passwordless bare-`kuma` probe reads — this heals it after
        // out-of-band changes (`bootc rollback`, raw `bootc switch`).
        // Kuma's own metadata, not machine state, so doctor stays
        // read-only about the machine itself. When the deployment no
        // longer matches root storage's tag, the deployed ID is unknowable
        // here — drop the stamp so the probe skips the check.
        let heal = if deployment_current {
            format!(
                "mkdir -p /var/lib/kuma && printf '%s\\n' {root_id} > {}",
                crate::state::DEPLOYED_ID_FILE
            )
        } else {
            format!("rm -f {}", crate::state::DEPLOYED_ID_FILE)
        };
        let _ = host_output(&["sudo", "sh", "-c", &heal]);
    }
}

/// The oneshots record their last run in Result=; the timers are what
/// keeps long-uptime machines converged.
fn check_convergence(report: &mut impl FnMut(Grade, &str, String, Option<Action>)) {
    let mut targets: Vec<(&str, &str)> = Vec::new();
    if Path::new("/usr/lib/kuma/user").exists() {
        targets.push(("kuma-user-sync.service", "user"));
    }
    if Path::new("/usr/lib/kuma/flatpaks").exists() {
        targets.push(("kuma-flatpak-sync.service", "flatpak sync"));
        targets.push(("kuma-flatpak-sync.timer", "flatpak sync"));
    }
    if Path::new("/usr/lib/kuma/brews").exists() {
        targets.push(("kuma-brew-sync.service", "brew sync"));
        targets.push(("kuma-brew-sync.timer", "brew sync"));
    }
    let names: Vec<&str> = targets.iter().map(|(unit, _)| *unit).collect();
    let facts = unit_facts(&names);
    for (unit, name) in &targets {
        let Some(fact) = facts.get(*unit) else {
            report(Grade::Warn, name, format!("{unit} state unavailable"), None);
            continue;
        };
        if unit.ends_with(".timer") {
            if fact.active == "active" {
                report(Grade::Ok, name, format!("{unit} active"), None);
            } else {
                let fix = Action::new(
                    "start",
                    format!("sudo systemctl start {unit}"),
                    "restart the convergence timer",
                );
                report(Grade::Fail, name, format!("{unit} is not active"), Some(fix));
            }
        } else if fact.active == "active" || fact.active == "activating" {
            // Asked before the result, because systemd reports
            // Result=success for a unit that has not finished: a first
            // boot downloading a gigabyte of flatpaks was graded "last
            // run succeeded" while the run was still going, which is a
            // true field and a false sentence.
            report(Grade::Ok, name, format!("{unit} is running now"), None);
        } else if fact.result == "success" {
            report(Grade::Ok, name, format!("{unit} last run succeeded"), None);
        } else {
            let fix = Action::new("sync", "kuma sync", "re-run convergence now");
            report(Grade::Fail, name, format!("{unit} last run: {}", fact.result), Some(fix));
        }
    }
    if Path::new("/usr/lib/kuma/flatpaks").exists() {
        match host_output(&["flatpak", "remotes", "--system", "--columns=name"]) {
            Ok(out) if out.lines().any(|l| l.trim() == "flathub") => {
                report(Grade::Ok, "flathub", "remote configured".into(), None)
            }
            _ => {
                let fix = Action::new(
                    "add-remote",
                    "sudo flatpak remote-add --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo",
                    "restore the remote",
                );
                report(
                    Grade::Fail,
                    "flathub",
                    "remote missing; flatpak convergence cannot install".into(),
                    Some(fix),
                );
            }
        }
    }
}

/// Snapshots fail quietly by design, and correctly so: the script
/// degrades rather than erroring when the target isn't btrfs, and the
/// timer is Persistent with a jittered delay, so nothing complains on a
/// machine taking no snapshots at all. Both choices are right for the
/// machine and wrong for the person, who otherwise learns on the one day
/// they wanted a file back. This is the check that asks out loud.
fn check_snapshots(report: &mut impl FnMut(Grade, &str, String, Option<Action>)) {
    let config = match Config::load(Path::new(BAKED_CONFIG)) {
        Ok(config) => config,
        // The build validated this file before baking it in, so a copy
        // that won't load is worth naming even though this check can't
        // say more than that.
        Err(_) => {
            report(Grade::Warn, "snapshots", format!("cannot read {BAKED_CONFIG}"), None);
            return;
        }
    };
    // A declaration that never asked for snapshots is not unhealthy for
    // not having any, and saying so on every run would train people to
    // read past it.
    if !config.snapshots.enable {
        return;
    }

    // Ask the filesystem first: on a target that can't snapshot, the
    // timer being active is true and beside the point.
    let target = &config.snapshots.target;
    let fstype = host_output(&["findmnt", "-no", "FSTYPE", "-T", target]).unwrap_or_default();
    let fstype = fstype.trim();
    if !fstype.is_empty() && fstype != "btrfs" {
        report(
            Grade::Fail,
            "snapshots",
            format!("{target} is {fstype}, not btrfs; no snapshot can ever be taken"),
            None,
        );
        return;
    }

    let facts = unit_facts(&["kuma-snapshot.timer", "kuma-snapshot.service"]);
    match facts.get("kuma-snapshot.timer") {
        Some(fact) if fact.active == "active" => report(
            Grade::Ok,
            "snapshots",
            format!("kuma-snapshot.timer active ({})", config.snapshots.interval),
            None,
        ),
        Some(_) => {
            let fix = Action::new(
                "start",
                "sudo systemctl start kuma-snapshot.timer",
                "resume scheduled snapshots",
            );
            report(Grade::Fail, "snapshots", "kuma-snapshot.timer is not active".into(), Some(fix));
        }
        None => {
            report(Grade::Warn, "snapshots", "kuma-snapshot.timer state unavailable".into(), None)
        }
    }

    if let Some(fact) = facts.get("kuma-snapshot.service") {
        if !fact.result.is_empty() && fact.result != "success" {
            let fix = Action::new(
                "inspect",
                "systemctl status kuma-snapshot.service",
                "see why the snapshot failed",
            );
            report(
                Grade::Fail,
                "snapshots",
                format!("kuma-snapshot.service last run: {}", fact.result),
                Some(fix),
            );
        }
    }

    // The store is the only evidence the whole chain ran. A unit can be
    // active and successful and still have written nothing.
    let (store, count) = snapshot::store_state(&config);
    if count == 0 {
        // Warn, not fail: between switching to a declaration that asks
        // for snapshots and the timer's first tick, empty is the correct
        // state rather than a broken one.
        let fix = Action::new(
            "snapshot",
            "sudo systemctl start kuma-snapshot.service",
            "take the first one now instead of waiting for the timer",
        );
        report(
            Grade::Warn,
            "snapshots",
            format!("none taken yet in {}", store.display()),
            Some(fix),
        );
    } else {
        report(Grade::Ok, "snapshots", format!("{count} in {}", store.display()), None);
    }
}

/// What a machine has done to a file its own image ships.
#[derive(PartialEq, Debug)]
enum EtcState {
    /// Not in /usr/etc: this image doesn't ship it, so there is nothing
    /// to shadow.
    NotShipped,
    Matches,
    /// Edited locally. ostree carries the difference between /etc and
    /// /usr/etc forward onto every future deployment, so this copy wins
    /// over the image's, permanently and silently.
    Shadowed,
    /// Deleted locally, which carries forward the same way: the image's
    /// file stays gone across updates.
    Removed,
}

fn classify_etc(image: Option<&[u8]>, live: Option<&[u8]>) -> EtcState {
    match (image, live) {
        (None, _) => EtcState::NotShipped,
        (Some(_), None) => EtcState::Removed,
        (Some(from_image), Some(on_disk)) if from_image == on_disk => EtcState::Matches,
        _ => EtcState::Shadowed,
    }
}

struct EtcScan {
    /// Paths the image actually ships, which is what a percentage or a
    /// count should be taken over.
    owned: usize,
    shadowed: Vec<String>,
    removed: Vec<String>,
}

/// Compare each owned path under two roots. `paths` are absolute
/// (`/etc/greetd/config.toml`), so both roots get the same relative tail.
fn scan_etc(paths: &[String], image_root: &Path, live_root: &Path) -> EtcScan {
    let mut scan = EtcScan { owned: 0, shadowed: Vec::new(), removed: Vec::new() };
    for path in paths {
        let tail = path.trim_start_matches('/');
        let from_image = std::fs::read(image_root.join(tail)).ok();
        if from_image.is_none() {
            continue;
        }
        scan.owned += 1;
        let on_disk = std::fs::read(live_root.join(tail)).ok();
        match classify_etc(from_image.as_deref(), on_disk.as_deref()) {
            EtcState::Shadowed => scan.shadowed.push(path.clone()),
            EtcState::Removed => scan.removed.push(path.clone()),
            EtcState::Matches | EtcState::NotShipped => {}
        }
    }
    scan
}

/// /etc drift: is anything the image ships being overridden locally?
///
/// This is the general form of a trap that cost real time here. A var was
/// added to /etc/environment by hand to fix a display bug, later baked
/// into the image properly, and the hand-edited copy went on winning: the
/// declared version could never be tested, and nothing anywhere said so.
/// ostree's /etc merge is the mechanism, and it is working as designed;
/// what's missing is anyone telling you it happened.
///
/// Two design choices worth keeping. It compares /etc against /usr/etc
/// directly instead of shelling out to `ostree admin config-diff`, which
/// needs root: a health check that prompts for a password is a health
/// check people stop running, and /usr/etc is world-readable. And it only
/// looks at paths kuma's own image writes, computed from the machine's
/// baked declaration, because config-diff on a real system reports dozens
/// of legitimately-local files (machine-id, fstab, ssh host keys) and a
/// check that cries wolf teaches you to ignore it.
fn check_etc_drift(report: &mut impl FnMut(Grade, &str, String, Option<Action>)) {
    // The machine's own declaration says what its image owns.
    let Ok(config) = Config::load(Path::new(crate::state::BAKED_CONFIG)) else {
        return;
    };
    // "/usr" and "/" in production; parameterized so the states this
    // check exists to catch can be exercised against a temp tree instead
    // of only ever running the everything-is-fine branch.
    let scan =
        scan_etc(&crate::containerfile::etc_paths(&config), Path::new("/usr"), Path::new("/"));
    let EtcScan { owned, shadowed, removed } = scan;
    if owned == 0 {
        return;
    }
    // Restoring the image's copy is `cp`, not `rm`: a deletion is itself
    // a local modification and carries forward as one, so removing the
    // file would keep it removed rather than let the image's version back.
    let restore = |path: &str| {
        Action::new(
            "restore",
            format!("sudo cp /usr{path} {path}"),
            "put the image's copy back so future updates flow through",
        )
    };
    if !shadowed.is_empty() {
        report(
            Grade::Warn,
            "etc",
            // No capture edge here, deliberately, and the message says so
            // rather than leaving the asymmetry with `kuma diff` looking
            // like an oversight. Package drift is a fork because a package
            // is your choice; /etc content is kuma's curation, so an edit
            // worth keeping belongs in the image rather than in your
            // declaration. That is how the COSMIC scanout vars got fixed.
            format!(
                "local edits shadow the image: {}. These win over every future image, so the declared version never applies. An edit worth keeping belongs in the image, not the declaration",
                shadowed.join(", ")
            ),
            Some(restore(&shadowed[0])),
        );
    }
    if !removed.is_empty() {
        report(
            Grade::Warn,
            "etc",
            format!("deleted locally, and staying deleted across updates: {}", removed.join(", ")),
            Some(restore(&removed[0])),
        );
    }
    if shadowed.is_empty() && removed.is_empty() {
        report(
            Grade::Ok,
            "etc",
            format!("{owned} files this image owns in /etc, none shadowed locally"),
            None,
        );
    }
}

/// Boot health: is the greenboot auto-rollback machinery in the image,
/// did THIS boot pass its checks, and does the bootloader actually
/// consult the boot counter — the part that can silently be missing on
/// machines whose bootloader config predates greenboot in the image.
fn check_boot_health(report: &mut impl FnMut(Grade, &str, String, Option<Action>)) {
    if !Path::new("/usr/libexec/greenboot/greenboot").exists() {
        report(
            Grade::Warn,
            "boot health",
            "greenboot not in this image; automatic rollback of failed boots arrives with the next rebuild".into(),
            Some(Action::new("update", "kuma update", "rebuild on a kuma that bakes boot health")),
        );
        return;
    }
    // RemainAfterExit keeps the health-check oneshot `active` after
    // success, so is-active is this boot's green/red verdict.
    match host_output_any(&["systemctl", "is-active", "greenboot-healthcheck.service"]).as_deref() {
        Ok("active") => {
            report(Grade::Ok, "boot health", "this boot passed its health checks".into(), None)
        }
        Ok("failed") => report(
            Grade::Fail,
            "boot health",
            "this boot failed health checks; greenboot may reboot toward rollback".into(),
            Some(Action::new(
                "inspect",
                "systemctl status greenboot-healthcheck.service",
                "see which check failed",
            )),
        ),
        Ok(state) => report(
            Grade::Warn,
            "boot health",
            format!("health check is {state}: boot still settling, or the unit is not enabled"),
            None,
        ),
        Err(_) => report(Grade::Warn, "boot health", "systemctl unavailable".into(), None),
    }
    // The counter lives in GRUB: its config must decrement boot_counter
    // and fall back when it hits zero. Fresh installs carry it in
    // grub.cfg (bootupd assembles greenboot's snippet); machines
    // installed before greenboot get it converged into custom.cfg by
    // kuma-boot-health-sync — grep both, or converged machines warn
    // forever. /boot/grub2 is 0700 on Fedora, hence sudo; `true` keeps
    // no-match from reading as sudo-declined.
    let cfg = host_output(&[
        "sudo", "sh", "-c",
        "grep -h boot_counter /boot/grub2/grub.cfg /boot/grub2/custom.cfg /boot/efi/EFI/fedora/grub.cfg 2>/dev/null; true",
    ]);
    match cfg {
        Ok(out) if !out.trim().is_empty() => report(
            Grade::Ok,
            "boot health",
            "bootloader counts boot attempts; fallback armed".into(),
            None,
        ),
        Ok(_) => report(
            Grade::Warn,
            "boot health",
            "bootloader has no boot_counter fallback (config predates greenboot in the image); without it a failing update reboot-loops instead of rolling back".into(),
            Some(Action::new(
                "converge",
                "sudo /usr/libexec/kuma-boot-health-sync",
                "install the grub fallback hook now (also runs on every boot)",
            )),
        ),
        Err(_) => report(
            Grade::Warn,
            "boot health",
            "grub.cfg unreadable (sudo declined?)".into(),
            None,
        ),
    }
}

/// Build leftovers eat disk quietly: rebuilds strand dangling images
/// (~3.5 GB each), and interrupted builds abandon buildah working
/// containers that pin their layers while being invisible to
/// `podman images` — one was once found holding 68 GB.
fn check_build_leftovers(report: &mut impl FnMut(Grade, &str, String, Option<Action>)) {
    let dangling = host_output(&["podman", "images", "-f", "dangling=true", "-q"])
        .map(|out| out.lines().filter(|l| !l.trim().is_empty()).count());
    let abandoned = host_output_any(&[
        "podman",
        "ps",
        "-a",
        "--external",
        "--format",
        "{{.Names}} {{.Status}}",
    ])
    .map(|out| {
        out.lines().filter(|l| l.contains("-working-container") && l.ends_with(" Storage")).count()
    });
    match (dangling, abandoned) {
        (Err(_), Err(_)) => {} // no podman here — nothing to check
        (dangling, abandoned) => {
            let (dangling, abandoned) = (dangling.unwrap_or(0), abandoned.unwrap_or(0));
            if dangling == 0 && abandoned == 0 {
                report(Grade::Ok, "storage", "no build leftovers".into(), None);
            } else {
                // `kuma clean` reports every image object the prune cascade
                // deletes, so counting the same way needs a dry run podman
                // doesn't have — name the stranded builds instead.
                let mut parts = Vec::new();
                if dangling > 0 {
                    parts.push(format!(
                        "{dangling} stranded build image(s) plus their cached layers"
                    ));
                }
                if abandoned > 0 {
                    parts.push(format!("{abandoned} abandoned build container(s)"));
                }
                report(
                    Grade::Warn,
                    "storage",
                    parts.join(", "),
                    Some(Action::new("clean", "kuma clean", "reclaim them")),
                );
            }
        }
    }
}

/// Kernel-side only (driver bound, render node present) — userspace probes
/// need tools the image deliberately doesn't carry.
/// Whether the root filesystem sits inside a LUKS container.
///
/// Pure over what `findmnt` and `lsblk` answer, because the interesting
/// case is a machine this one is not: a check that can only run on an
/// encrypted machine would ship having never taken the other branch.
///
/// `findmnt` prints a btrfs subvolume as `/dev/mapper/luks-x[/root]`, so
/// the source has to be cut at the bracket before anything can be asked
/// about the device. `None` means the question could not be answered,
/// which is a reason to say nothing rather than to guess.
fn root_encrypted(findmnt_source: &str, lsblk_types: &str) -> Option<bool> {
    let source = root_device(findmnt_source);
    if source.is_empty() {
        return None;
    }
    let types: Vec<&str> = lsblk_types.split_whitespace().collect();
    if types.is_empty() {
        return None;
    }
    // lsblk names the whole stack under the device; a crypt layer
    // anywhere in it is the root sitting inside a container.
    Some(types.contains(&"crypt"))
}

/// The device out of what `findmnt` printed, which appends the btrfs
/// subvolume in brackets. Shared with the caller, which needs the same
/// answer to ask lsblk about it, and had its own copy of this until the
/// two could have disagreed.
fn root_device(findmnt_source: &str) -> &str {
    findmnt_source.trim().split('[').next().unwrap_or_default().trim()
}

fn check_encryption(report: &mut impl FnMut(Grade, &str, String, Option<Action>)) {
    // /sysroot on a booted ostree deployment, / everywhere else.
    let source = host_output(&["findmnt", "-no", "SOURCE", "/sysroot"])
        .or_else(|_| host_output(&["findmnt", "-no", "SOURCE", "/"]))
        .unwrap_or_default();
    let device = root_device(&source);
    let types = if device.is_empty() {
        String::new()
    } else {
        host_output(&["lsblk", "-no", "TYPE", device]).unwrap_or_default()
    };
    // Silent when unknowable. Encryption is a choice, so neither answer
    // is a fault and neither carries a fix: this line exists so that a
    // machine can say which choice was made, since nothing else on a
    // running system does.
    match root_encrypted(&source, &types) {
        Some(true) => {
            report(Grade::Ok, "encryption", "root is a LUKS volume, unlocked at boot".into(), None)
        }
        Some(false) => report(
            Grade::Ok,
            "encryption",
            "root is not encrypted; that was decided when this disk was partitioned".into(),
            None,
        ),
        None => {}
    }
}

fn check_gpu(report: &mut impl FnMut(Grade, &str, String, Option<Action>)) {
    let mut drivers: Vec<String> = Vec::new();
    if let Ok(cards) = std::fs::read_dir("/sys/class/drm") {
        for card in cards.flatten() {
            let name = card.file_name();
            let name = name.to_string_lossy();
            if !name.starts_with("card") || name.contains('-') {
                continue;
            }
            if let Ok(driver) = std::fs::read_link(card.path().join("device/driver")) {
                if let Some(driver) = driver.file_name() {
                    drivers.push(driver.to_string_lossy().into_owned());
                }
            }
        }
    }
    let render = std::fs::read_dir("/dev/dri").is_ok_and(|mut d| {
        d.any(|e| e.is_ok_and(|e| e.file_name().to_string_lossy().starts_with("renderD")))
    });
    match (drivers.is_empty(), render) {
        (false, true) => report(
            Grade::Ok,
            "gpu",
            format!("{} bound, render node present", drivers.join(", ")),
            None,
        ),
        (false, false) => report(
            Grade::Warn,
            "gpu",
            format!("{} bound, but no render node; software rendering likely", drivers.join(", ")),
            None,
        ),
        (true, _) => {
            report(Grade::Warn, "gpu", "no GPU driver bound (VM or headless?)".into(), None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    /// The epoch values come from `date -u -d … +%s`. A date crate would
    /// have brought its own tests; a hand-rolled civil-from-days needs
    /// these, and the leap-year cases are where that arithmetic breaks.
    #[test]
    fn rfc3339_parses_to_the_same_instant_date_reports() {
        assert_eq!(epoch_from_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(epoch_from_rfc3339("2026-08-13T00:00:00Z"), Some(1_786_579_200));
        assert_eq!(epoch_from_rfc3339("2026-07-04T13:45:00Z"), Some(1_783_172_700));
        // 2000 is a leap year (divisible by 400), 1900 was not.
        assert_eq!(epoch_from_rfc3339("2000-03-01T00:00:00Z"), Some(951_868_800));
        assert_eq!(epoch_from_rfc3339("2024-02-29T00:00:00Z"), Some(1_709_164_800));
    }

    /// bootc's timestamp is chrono-serialized, so the tail varies with
    /// what the image recorded. Everything past the minutes is noise to a
    /// number of days, and none of it may turn a real answer into None.
    #[test]
    fn the_tail_of_a_timestamp_is_ignored_not_fatal() {
        let plain = epoch_from_rfc3339("2026-08-13T00:00:00Z");
        assert_eq!(epoch_from_rfc3339("2026-08-13T00:00:00.123456789Z"), plain);
        assert_eq!(epoch_from_rfc3339("2026-08-13T00:00:00+00:00"), plain);
        assert_eq!(epoch_from_rfc3339("2026-08-13T00:00Z"), plain);
    }

    /// An unreadable timestamp has to read as "no answer". The failure
    /// this guards is a garbage field parsing to *some* number and a
    /// machine being told its image is fifty years old.
    #[test]
    fn a_timestamp_that_isnt_one_is_no_answer() {
        for bad in [
            "",
            "2026-08-13",
            "not-a-date",
            "2026-13-01T00:00:00Z", // month 13
            "2026-08-32T00:00:00Z", // day 32
            "2026-08-13T24:00:00Z", // hour 24
            "2026-08-13T00:60:00Z", // minute 60
            "2026-08-13Tmorning",
        ] {
            assert_eq!(epoch_from_rfc3339(bad), None, "{bad}");
        }
    }

    /// Clock skew is real on a machine whose RTC hasn't been set yet, and
    /// the subtraction is unsigned: without the floor, an image stamped
    /// one hour in the future reads as some hundred-million days old and
    /// doctor screams about it.
    #[test]
    fn a_future_timestamp_is_zero_days_old_not_an_enormous_number() {
        assert_eq!(days_since("2099-01-01T00:00:00Z"), Some(0));
        assert_eq!(days_since("1970-01-02T00:00:00Z").map(|d| d > 20_000), Some(true));
        assert_eq!(days_since("nonsense"), None);
    }

    /// This decides whether a failed systemd-remount-fs is excused, so a
    /// loose match here is how a real failure gets called known-benign
    /// forever. The near-misses are the point: `root` is a subvolume name
    /// on every Anaconda btrfs install, and /var/roothome is a real mount.
    #[test]
    fn only_an_active_root_entry_excuses_the_remount_failure() {
        let anaconda = "UUID=86bf4581 / btrfs subvol=root,compress=zstd:1,ro 0 0\n\
                        UUID=4de4aa74 /boot ext4 defaults 1 2\n";
        assert!(fstab_declares_root(anaconda));

        // What kuma-fstab-sync leaves behind: the same line, commented.
        let converged = "# Commented out by kuma-fstab-sync: this machine boots a composefs\n\
                         #UUID=86bf4581 / btrfs subvol=root,compress=zstd:1,ro 0 0\n\
                         UUID=4de4aa74 /boot ext4 defaults 1 2\n";
        assert!(!fstab_declares_root(converged));

        // A space after the comment marker shifts every field right, which
        // is exactly how a naive field test would still see a root mount.
        assert!(!fstab_declares_root("# UUID=86bf4581 / btrfs subvol=root 0 0\n"));

        // Neither of these is a root entry, and both contain "root".
        assert!(!fstab_declares_root("UUID=y /var/roothome btrfs subvol=root 0 0\n"));
        assert!(!fstab_declares_root("UUID=4de4aa74 /boot ext4 defaults 1 2\n"));

        assert!(!fstab_declares_root(""));
    }

    fn config(toml: &str) -> Config {
        toml::from_str(toml).expect("test config parses")
    }

    /// A machine with nothing on it, to be filled in per test. Every
    /// observation present means "looked, found this"; the Nones in the
    /// individual tests mean "couldn't look at all".
    fn machine() -> Machine {
        Machine {
            rpm: None,
            flatpak_system: Some(BTreeSet::new()),
            flatpak_user: BTreeSet::new(),
            flatpak_state: BTreeSet::new(),
            brew_installed: Some(BTreeSet::new()),
            brew_leaves: BTreeSet::new(),
            brew_state: BTreeSet::new(),
        }
    }

    fn items(found: &[Candidate]) -> Vec<&str> {
        found.iter().map(|c| c.item.as_str()).collect()
    }

    /// The whole point of the verb: what convergence is about to destroy
    /// is exactly what capture offers to keep. diff builds its "removes
    /// it" entries from this same set, so the two cannot disagree.
    #[test]
    fn capture_offers_what_convergence_would_destroy() {
        let config = config("schema_version = 1\n[packages]\nflatpak = [\"org.gnome.Loupe\"]\n");
        let mut m = machine();
        m.flatpak_system = Some(set(&["org.gnome.Loupe", "org.gnome.Boxes"]));
        // the sync installed Boxes back when it was declared, so it is
        // convergence's to take back
        m.flatpak_state = set(&["org.gnome.Loupe", "org.gnome.Boxes"]);
        let found = candidates(&config, &m);
        assert_eq!(items(&found), ["org.gnome.Boxes"]);
        assert!(found[0].doomed);
        assert_eq!(found[0].list, "flatpak");
        // and the declared one is never offered back to itself
        assert!(!items(&found).contains(&"org.gnome.Loupe"));
    }

    /// The reason a Flatpak store can exist on a kuma machine at all.
    /// Installing system-wide is no longer evidence that convergence put
    /// it there, so an app the owner installed is offered for capture
    /// and left alone until they decide — while one the sync installed
    /// and the declaration dropped is still removed.
    #[test]
    fn a_store_install_is_the_owners_not_convergences() {
        let config = config("schema_version = 1\n");
        let mut m = machine();
        m.flatpak_system = Some(set(&["io.github.kolunmi.Bazaar", "org.gnome.Boxes"]));
        m.flatpak_state = set(&["org.gnome.Boxes"]);
        let found = candidates(&config, &m);
        assert_eq!(items(&found), ["org.gnome.Boxes", "io.github.kolunmi.Bazaar"], "urgent first");
        assert!(found[0].doomed, "Boxes was convergence's and is now undeclared");
        assert!(!found[1].doomed, "the store put Bazaar there, so it is the owner's");
        assert!(!found[1].promotes, "it is already system-wide; declaring only writes it down");
    }

    /// You cannot imperatively install an rpm on a bootc machine, so
    /// [packages].rpm is already declarative and there is nothing to
    /// capture. An observation full of undeclared rpms changes nothing.
    #[test]
    fn rpm_is_never_a_capture_candidate() {
        let config = config("schema_version = 1\n[packages]\nrpm = [\"fish\"]\n");
        let mut m = machine();
        m.rpm = Some(set(&["fish", "vim", "gcc", "systemd"]));
        assert!(candidates(&config, &m).is_empty());
    }

    /// Leaves, not the full list: a dependency is baggage that arrived
    /// with a choice, and declaring it would pin someone else's
    /// implementation detail into your system definition.
    #[test]
    fn brew_dependencies_are_not_choices() {
        let config = config("schema_version = 1\n");
        let mut m = machine();
        m.brew_installed = Some(set(&["ripgrep", "pcre2"]));
        m.brew_leaves = set(&["ripgrep"]);
        let found = candidates(&config, &m);
        assert_eq!(items(&found), ["ripgrep"]);
        // ad-hoc, so convergence isn't coming for it: undeclared costs it
        // reproducibility, not survival
        assert!(!found[0].doomed);
        assert_eq!(found[0].list, "brew");
    }

    /// A formula the sync installed and the declaration no longer names
    /// is on convergence's removal list, so it is urgent even though the
    /// ad-hoc ones next to it are not.
    #[test]
    fn brew_convergence_owns_what_it_installed() {
        let config = config("schema_version = 1\n");
        let mut m = machine();
        m.brew_installed = Some(set(&["btop", "jq"]));
        m.brew_leaves = set(&["btop", "jq"]);
        m.brew_state = set(&["btop"]);
        let found = candidates(&config, &m);
        assert_eq!(items(&found), ["btop", "jq"], "urgent first");
        assert!(found[0].doomed, "btop was convergence's and is now undeclared");
        assert!(!found[1].doomed, "jq was always the owner's");
    }

    /// The escape hatch stays an escape hatch. Capturing a --user flatpak
    /// installs it system-wide and hands it to convergence, which is a
    /// change to what it is, not just to where it is written down.
    #[test]
    fn user_flatpaks_are_opt_in_only() {
        let config = config("schema_version = 1\n");
        let mut m = machine();
        m.flatpak_user = set(&["org.gnome.Boxes"]);
        let found = candidates(&config, &m);
        assert_eq!(items(&found), ["org.gnome.Boxes"]);
        assert!(found[0].promotes);
        assert!(!found[0].doomed, "convergence never touches --user installs");
    }

    /// The same app installed both ways, or already declared, must not
    /// show up twice or at all.
    #[test]
    fn user_flatpaks_already_covered_are_not_offered_again() {
        let config = config("schema_version = 1\n[packages]\nflatpak = [\"org.gnome.Papers\"]\n");
        let mut m = machine();
        m.flatpak_system = Some(set(&["org.gnome.Boxes"]));
        m.flatpak_user = set(&["org.gnome.Boxes", "org.gnome.Papers"]);
        let found = candidates(&config, &m);
        assert_eq!(items(&found), ["org.gnome.Boxes"], "system install already covers it");
        assert!(!found[0].promotes);
    }

    /// An observation nobody could make is not evidence of an empty
    /// machine: flatpak missing must never read as "everything you have
    /// is undeclared" (or, worse, propose declaring nothing at all).
    #[test]
    fn unobservable_lists_yield_no_candidates() {
        let config = config("schema_version = 1\n");
        let mut m = machine();
        m.flatpak_system = None;
        m.brew_installed = None;
        m.brew_leaves = set(&["ripgrep"]);
        assert!(candidates(&config, &m).is_empty());
    }

    /// The four states an image-owned /etc file can be in. "Not shipped"
    /// has to be distinct from "matches": a path the declaration implies
    /// but this image never wrote cannot be shadowed, and reporting it
    /// would be the false alarm that gets the whole check ignored.
    #[test]
    fn etc_states_separate_shadowing_from_absence() {
        let image = b"KUMA=1\n".as_slice();
        assert_eq!(classify_etc(Some(image), Some(image)), EtcState::Matches);
        assert_eq!(classify_etc(Some(image), Some(b"KUMA=0\n")), EtcState::Shadowed);
        assert_eq!(classify_etc(Some(image), None), EtcState::Removed);
        // no image copy: nothing to shadow, whatever is on disk
        assert_eq!(classify_etc(None, Some(b"local only\n")), EtcState::NotShipped);
        assert_eq!(classify_etc(None, None), EtcState::NotShipped);
        // byte comparison, so whitespace counts: an /etc file that differs
        // by a trailing newline still wins over the image's copy
        assert_eq!(classify_etc(Some(b"KUMA=1\n"), Some(b"KUMA=1")), EtcState::Shadowed);
    }

    /// The branch that fires when something is actually wrong, which on a
    /// healthy machine never runs. All four states in one tree: a file
    /// left alone, one edited (the /etc/environment trap), one deleted,
    /// and one the image never shipped.
    #[test]
    fn scanning_etc_finds_the_shadowed_and_the_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let (image, live) = (dir.path().join("usr"), dir.path().join("live"));
        let put = |root: &Path, rel: &str, body: &str| {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        };

        put(&image, "etc/greetd/config.toml", "greeter\n");
        put(&live, "etc/greetd/config.toml", "greeter\n");
        put(&image, "etc/environment", "COSMIC_DISABLE_OVERLAY_SCANOUT=1\n");
        put(&live, "etc/environment", "COSMIC_DISABLE_OVERLAY_SCANOUT=1\nEDITED=1\n");
        put(&image, "etc/xdg/mimeapps.list", "defaults\n");
        // no live copy of mimeapps.list: deleted by hand
        put(&live, "etc/local-only.conf", "not the image's\n");

        let paths: Vec<String> = [
            "/etc/greetd/config.toml",
            "/etc/environment",
            "/etc/xdg/mimeapps.list",
            "/etc/local-only.conf",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();

        let scan = scan_etc(&paths, &image, &live);
        assert_eq!(scan.owned, 3, "the local-only file is not the image's to own");
        assert_eq!(scan.shadowed, ["/etc/environment"]);
        assert_eq!(scan.removed, ["/etc/xdg/mimeapps.list"]);
    }

    /// Both answers, since the machine writing this check can only be
    /// one of them and the other would otherwise ship unexercised.
    #[test]
    fn an_encrypted_root_is_told_apart_from_a_plain_one() {
        // A LUKS root, as an ostree deployment reports it: the mapper
        // plus the subvolume in brackets, and the stack under it.
        assert_eq!(
            root_encrypted("/dev/mapper/luks-f5d1fc89[/root]", "crypt\n"),
            Some(true),
            "a crypt layer means the root is inside a container"
        );
        // lsblk prints the whole stack when asked about a partition.
        assert_eq!(root_encrypted("/dev/nvme0n1p3", "part\ncrypt\n"), Some(true));
        // And a plain install, which is the default and must not be
        // reported as anything else.
        assert_eq!(root_encrypted("/dev/sda3[/root]", "part\n"), Some(false));
        assert_eq!(root_encrypted("/dev/vda3", "part\n"), Some(false));
        // LVM is not encryption.
        assert_eq!(root_encrypted("/dev/mapper/fedora-root", "lvm\n"), Some(false));
        // Unanswerable rather than guessed: no findmnt, no lsblk, or a
        // source that is not a device at all.
        assert_eq!(root_encrypted("", "part\n"), None);
        assert_eq!(root_encrypted("/dev/sda3", ""), None);
        assert_eq!(root_encrypted("  ", "  "), None);
    }

    /// An image that ships none of the paths it declares means the check
    /// has nothing to say, not that everything is fine. Saying "0 files,
    /// all good" would be a green light nobody earned.
    #[test]
    fn an_image_owning_nothing_reports_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let scan = scan_etc(
            &["/etc/greetd/config.toml".to_string()],
            &dir.path().join("usr"),
            &dir.path().join("live"),
        );
        assert_eq!(scan.owned, 0);
        assert!(scan.shadowed.is_empty() && scan.removed.is_empty());
    }

    #[test]
    fn doctor_json_carries_findings_and_fixes() {
        let findings = vec![
            Finding {
                grade: Grade::Ok,
                name: "disk".into(),
                detail: "41% used, 280G free".into(),
                fix: None,
            },
            Finding {
                grade: Grade::Fail,
                name: "flathub".into(),
                detail: "remote missing".into(),
                fix: Some(Action::new(
                    "add-remote",
                    "sudo flatpak remote-add flathub",
                    "restore the remote",
                )),
            },
        ];
        let json = doctor_json(&findings);
        assert_eq!(json["checks"][0]["grade"], "ok");
        assert_eq!(json["checks"][0]["fix"], serde_json::Value::Null);
        assert_eq!(json["checks"][1]["grade"], "fail");
        assert_eq!(json["checks"][1]["fix"]["cmd"], "sudo flatpak remote-add flathub");
        assert!(json["checks"][1]["fix"]["why"].is_string());
        assert_eq!(json["summary"]["fails"], 1);
        assert_eq!(json["summary"]["warns"], 0);
    }

    #[test]
    fn diff_json_shape_is_stable_for_agents() {
        let sections = vec![
            DiffSection { name: "packages.rpm", entries: vec![], skipped: None },
            DiffSection {
                name: "packages.flatpak",
                entries: vec![DiffEntry {
                    change: "add",
                    item: "org.gnome.Loupe".into(),
                    note: "declared, not installed (convergence installs it)".into(),
                }],
                skipped: None,
            },
            DiffSection {
                name: "packages.brew",
                entries: vec![],
                skipped: Some("brew not bootstrapped yet; first boot installs it".into()),
            },
        ];
        let actions = [Action::new("sync", "kuma sync", "converge now")];
        let adhoc_brews = ["jq".to_string()];
        let adhoc_flatpaks = ["io.github.kolunmi.Bazaar".to_string()];
        let json = diff_json(
            Path::new("kuma.toml"),
            &sections,
            &adhoc_brews,
            &adhoc_flatpaks,
            false,
            true,
            &actions,
        );
        assert_eq!(json["config"], "kuma.toml");
        assert_eq!(json["drift"], true);
        // both ad-hoc lists are always present, so an agent can read
        // "nothing of mine is undeclared" from an empty array
        assert_eq!(json["adhoc_brews"][0], "jq");
        assert_eq!(json["adhoc_flatpaks"][0], "io.github.kolunmi.Bazaar");
        // the empty, unskipped section is elided; the skipped one is kept
        assert_eq!(json["sections"].as_array().unwrap().len(), 2);
        assert_eq!(json["sections"][0]["entries"][0]["change"], "add");
        assert_eq!(json["sections"][0]["entries"][0]["item"], "org.gnome.Loupe");
        assert!(json["sections"][1]["skipped"].is_string());
        assert_eq!(json["actions"][0]["cmd"], "kuma sync");
    }
}
