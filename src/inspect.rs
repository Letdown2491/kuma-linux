//! Day-2 visibility: `kuma diff` shows drift between the declaration and
//! this machine, `kuma doctor` checks the machine itself. Both are
//! read-only about the machine — convergence stays with the boot services
//! and timers, so a diff is safe to run out of curiosity. The one thing
//! doctor writes is kuma's own deployed-image stamp, which it refreshes
//! from the truth it alone (having root) can see.

use crate::config::Config;
use crate::host::{host_output, host_output_any};
use crate::state::{action_json, print_actions, Action};
use anyhow::{bail, Result};
use std::collections::BTreeSet;
use std::path::Path;

const BREW: &str = "/home/linuxbrew/.linuxbrew/bin/brew";
const BREW_STATE: &str = "/home/linuxbrew/.linuxbrew/.kuma-brews";

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
    let flatpak_system =
        ask(&["flatpak", "list", "--system", "--app", "--columns=application"]);
    let flatpak_user =
        ask(&["flatpak", "list", "--user", "--app", "--columns=application"]).unwrap_or_default();

    let ask_brew = !config.packages.brew.is_empty() || Path::new(BREW).exists();
    let brew_installed =
        ask_brew.then(|| ask(&[BREW, "list", "--formula", "-1"])).flatten();
    // `brew leaves` walks the dependency graph and costs about 1.3s here,
    // more than everything else this function does put together. It is
    // only ever read to tell an ad-hoc formula from a dependency, so it is
    // worth nothing when there are no installed formulae to classify.
    let brew_leaves = brew_installed
        .as_ref()
        .and_then(|installed| (!installed.is_empty()).then(|| ask(&[BREW, "leaves"])).flatten())
        .unwrap_or_default();
    let brew_state = owned_set(&std::fs::read_to_string(BREW_STATE).unwrap_or_default());

    Machine { rpm, flatpak_system, flatpak_user, brew_installed, brew_leaves, brew_state }
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
            out.push(Candidate {
                list: "flatpak",
                item: app.clone(),
                doomed: true,
                promotes: false,
            });
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

    // Ad-hoc brews are the non-doomed half of the same set: convergence
    // leaves them alone, so the only thing undeclared costs them is that
    // a rebuild elsewhere wouldn't reproduce them.
    let adhoc: Vec<String> =
        found.iter().filter(|c| c.list == "brew" && !c.doomed).map(|c| c.item.clone()).collect();

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
                &adhoc,
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
    if !adhoc.is_empty() {
        println!("Ad-hoc brews, kept as yours: {}", adhoc.join(", "));
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
    adhoc: &[String],
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
        "adhoc_brews": adhoc,
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

fn unit_state(unit: &str) -> String {
    // is-enabled exits non-zero for disabled units but still names the
    // state on stdout, so read stdout regardless of exit status.
    let state = host_output_any(&["systemctl", "is-enabled", unit]).unwrap_or_default();
    if state.is_empty() { "not found".into() } else { state }
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
pub fn doctor(json: bool) -> Result<()> {
    let mut findings: Vec<Finding> = Vec::new();
    let mut report = |grade: Grade, name: &str, detail: String, fix: Option<Action>| {
        findings.push(Finding { grade, name: name.to_string(), detail, fix });
    };

    check_deployment(&mut report);

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
            let (benign, real): (Vec<&str>, Vec<&str>) = names.iter().partition(|n| {
                **n == "systemd-remount-fs.service" && Path::new("/run/ostree-booted").exists()
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
                    None,
                );
            }
        }
        Err(_) => report(Grade::Warn, "units", "systemctl unavailable".into(), None),
    }

    if Path::new("/usr/lib/kuma").is_dir() {
        check_convergence(&mut report);
        check_boot_health(&mut report);
        check_etc_drift(&mut report);
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
            let fields: Vec<&str> =
                out.lines().nth(1).unwrap_or("").split_whitespace().collect();
            let pcent: u32 = fields
                .first()
                .and_then(|p| p.trim_end_matches('%').parse().ok())
                .unwrap_or(0);
            let detail =
                format!("{}% used, {} free", pcent, fields.get(1).unwrap_or(&"?"));
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

/// bootc status needs root; a sudo prompt out of `kuma doctor` is the
/// price of seeing the deployment at all, same as `kuma switch` pays.
fn check_deployment(report: &mut impl FnMut(Grade, &str, String, Option<Action>)) {
    let status = match host_output(&["sudo", "bootc", "status", "--format", "json"]) {
        Ok(out) => out,
        Err(_) => {
            report(Grade::Warn, "deployment", "bootc status unavailable (not a bootc system, or sudo declined)".into(), None);
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
        slot.pointer("/image/image/image")
            .and_then(|v| v.as_str())
            .map(str::to_string)
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

    // A build that was never switched to is the easy thing to forget.
    // Two storages are in play: the rootless build, and root's copy that
    // bootc actually deploys. Manifest digests don't survive the
    // save/load sync between them — only image IDs (config digests) do —
    // so compare IDs across storages and digests only within root's.
    if let Ok(local_id) = crate::image_id(crate::DEFAULT_TAG) {
        let root = host_output(&[
            "sudo", "podman", "image", "inspect", "--format", "{{.Id}} {{.Digest}}",
            crate::DEFAULT_TAG,
        ])
        .unwrap_or_default();
        let (root_id, root_digest) = root.split_once(' ').unwrap_or(("", ""));
        let deployed: Vec<String> = [Some(&booted), staged.as_ref()]
            .into_iter()
            .flatten()
            .filter_map(digest_of)
            .collect();
        let deployment_current =
            !root_digest.is_empty() && deployed.iter().any(|d| d == root_digest);
        if local_id != root_id || (!deployed.is_empty() && !deployment_current) {
            report(
                Grade::Warn,
                "deployment",
                "localhost/kuma:latest is newer than the deployment".into(),
                Some(Action::new("switch", "kuma switch", "stage the newer build")),
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
    for (unit, name) in targets {
        if unit.ends_with(".timer") {
            match host_output_any(&["systemctl", "is-active", unit]).as_deref() {
                Ok("active") => report(Grade::Ok, name, format!("{unit} active"), None),
                _ => {
                    let fix = Action::new(
                        "start",
                        format!("sudo systemctl start {unit}"),
                        "restart the convergence timer",
                    );
                    report(Grade::Fail, name, format!("{unit} is not active"), Some(fix));
                }
            }
        } else {
            match host_output_any(&["systemctl", "show", "-p", "Result", "--value", unit]).as_deref() {
                Ok("success") => {
                    report(Grade::Ok, name, format!("{unit} last run succeeded"), None)
                }
                Ok(result) => {
                    let fix = Action::new("sync", "kuma sync", "re-run convergence now");
                    report(Grade::Fail, name, format!("{unit} last run: {result}"), Some(fix));
                }
                Err(_) => report(Grade::Warn, name, format!("{unit} state unavailable"), None),
            }
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
    let scan = scan_etc(
        &crate::containerfile::etc_paths(&config),
        Path::new("/usr"),
        Path::new("/"),
    );
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
            format!(
                "local edits shadow the image: {}. These win over every future image, so the declared version never applies",
                shadowed.join(", ")
            ),
            Some(restore(&shadowed[0])),
        );
    }
    if !removed.is_empty() {
        report(
            Grade::Warn,
            "etc",
            format!(
                "deleted locally, and staying deleted across updates: {}",
                removed.join(", ")
            ),
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
    match host_output_any(&["systemctl", "is-active", "greenboot-healthcheck.service"])
        .as_deref()
    {
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
        "podman", "ps", "-a", "--external", "--format", "{{.Names}} {{.Status}}",
    ])
    .map(|out| {
        out.lines()
            .filter(|l| l.contains("-working-container") && l.ends_with(" Storage"))
            .count()
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
                    parts.push(format!("{dangling} stranded build image(s) plus their cached layers"));
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
        d.any(|e| {
            e.is_ok_and(|e| e.file_name().to_string_lossy().starts_with("renderD"))
        })
    });
    match (drivers.is_empty(), render) {
        (false, true) => report(Grade::Ok, "gpu", format!("{} bound, render node present", drivers.join(", ")), None),
        (false, false) => report(Grade::Warn, "gpu", format!("{} bound, but no render node; software rendering likely", drivers.join(", ")), None),
        (true, _) => report(Grade::Warn, "gpu", "no GPU driver bound (VM or headless?)".into(), None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| s.to_string()).collect()
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
        let found = candidates(&config, &m);
        assert_eq!(items(&found), ["org.gnome.Boxes"]);
        assert!(found[0].doomed);
        assert_eq!(found[0].list, "flatpak");
        // and the declared one is never offered back to itself
        assert!(!items(&found).contains(&"org.gnome.Loupe"));
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
                fix: Some(Action::new("add-remote", "sudo flatpak remote-add flathub", "restore the remote")),
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
        let json = diff_json(Path::new("kuma.toml"), &sections, &[], false, true, &actions);
        assert_eq!(json["config"], "kuma.toml");
        assert_eq!(json["drift"], true);
        // the empty, unskipped section is elided; the skipped one is kept
        assert_eq!(json["sections"].as_array().unwrap().len(), 2);
        assert_eq!(json["sections"][0]["entries"][0]["change"], "add");
        assert_eq!(json["sections"][0]["entries"][0]["item"], "org.gnome.Loupe");
        assert!(json["sections"][1]["skipped"].is_string());
        assert_eq!(json["actions"][0]["cmd"], "kuma sync");
    }
}
