//! Day-2 visibility: `kuma diff` shows drift between the declaration and
//! this machine, `kuma doctor` checks the machine itself. Both are
//! read-only about the machine — convergence stays with the boot services
//! and timers, so a diff is safe to run out of curiosity. The one thing
//! doctor writes is kuma's own deployed-image stamp, which it refreshes
//! from the truth it alone (having root) can see.

use crate::config::Config;
use crate::host::{host_output, host_output_any};
use crate::state::{print_actions, Action};
use anyhow::{bail, Result};
use std::collections::BTreeSet;
use std::path::Path;

const BREW: &str = "/home/linuxbrew/.linuxbrew/bin/brew";
const BREW_STATE: &str = "/home/linuxbrew/.linuxbrew/.kuma-brews";

/// Three-way, because that's how changes actually flow: kuma.toml is the
/// truth, the image carries a baked copy of the declaration, and the
/// machine converges to the IMAGE's copy — so config edits that were never
/// built show up here as "image declaration behind kuma.toml", not as
/// drift the next convergence run would fix.
pub fn diff(config: &Config, config_path: &Path) -> Result<()> {
    let mut drift = false;
    let mut stale_image = false;
    let mut converge_hint = false;

    // rpm lives in the image itself; missing means the declaration was
    // never built (or the build was never switched to).
    let lines: Vec<String> = config
        .packages
        .rpm
        .iter()
        .filter(|pkg| host_output(&["rpm", "-q", pkg]).is_err())
        .map(|pkg| format!("  + {pkg}  declared, missing from the running image"))
        .collect();
    print_section("packages.rpm", &lines, &mut drift);

    let declared: BTreeSet<&str> = config.packages.flatpak.iter().map(String::as_str).collect();
    let mut lines = Vec::new();
    match host_output(&["flatpak", "list", "--system", "--app", "--columns=application"]) {
        Ok(out) => {
            let installed = to_set(&out);
            for app in declared.difference(&installed) {
                lines.push(format!("  + {app}  declared, not installed (convergence installs it)"));
            }
            for app in installed.difference(&declared) {
                lines.push(format!("  - {app}  installed, not declared (convergence removes it)"));
            }
            converge_hint |= !lines.is_empty();
        }
        Err(_) => lines.push("  (flatpak unavailable — skipped)".into()),
    }
    stale_image |= image_list_stale("/usr/lib/kuma/flatpaks", &declared);
    print_section("packages.flatpak", &lines, &mut drift);

    let declared: BTreeSet<&str> = config.packages.brew.iter().map(String::as_str).collect();
    let mut lines = Vec::new();
    let mut adhoc = String::new();
    if !declared.is_empty() || Path::new(BREW).exists() {
        match host_output(&[BREW, "list", "--formula", "-1"]) {
            Ok(out) => {
                let installed = to_set(&out);
                // Only ever-declared formulae are removal candidates (the
                // sync's state file); ad-hoc installs are the owner's.
                let state_text = std::fs::read_to_string(BREW_STATE).unwrap_or_default();
                let state = to_set(&state_text);
                for f in declared.difference(&installed) {
                    lines.push(format!("  + {f}  declared, not installed (convergence installs it)"));
                }
                for f in installed.difference(&declared) {
                    if state.contains(f) {
                        lines.push(format!("  - {f}  no longer declared (convergence removes it)"));
                    }
                }
                converge_hint |= !lines.is_empty();
                // Leaves, not the full list — dependencies aren't the
                // owner's installs, just baggage that came with them.
                let leaves_text = host_output(&[BREW, "leaves"]).unwrap_or_default();
                let yours: Vec<&str> = to_set(&leaves_text)
                    .difference(&declared)
                    .filter(|f| !state.contains(**f))
                    .copied()
                    .collect();
                if !yours.is_empty() {
                    adhoc = format!("Ad-hoc brews, kept as yours: {}", yours.join(", "));
                }
            }
            Err(_) => lines.push("  (brew not bootstrapped yet — first boot installs it)".into()),
        }
    }
    stale_image |= image_list_stale("/usr/lib/kuma/brews", &declared);
    print_section("packages.brew", &lines, &mut drift);

    let mut lines = Vec::new();
    for svc in &config.services.enable {
        let state = unit_state(svc);
        if state != "enabled" && state != "alias" {
            lines.push(format!("  ! {svc}  declared enable, currently {state}"));
        }
    }
    for svc in &config.services.disable {
        if unit_state(svc) == "enabled" {
            lines.push(format!("  ! {svc}  declared disable, currently enabled"));
        }
    }
    print_section("services", &lines, &mut drift);

    if !adhoc.is_empty() {
        println!("{adhoc}");
    }
    if stale_image {
        println!("\nThe image's baked declaration is behind {}.", config_path.display());
        print_actions(&[Action::new(
            "build",
            "kuma build",
            "bake the edit — then `kuma switch` and reboot carry it to the machine",
        )]);
    } else if converge_hint {
        println!();
        print_actions(&[Action::new(
            "sync",
            "kuma sync",
            "converge now — otherwise the boot/daily run picks this up",
        )]);
    }
    if !drift && !stale_image {
        println!("No drift — this machine matches {}.", config_path.display());
    }
    Ok(())
}

pub(crate) fn to_set(text: &str) -> BTreeSet<&str> {
    text.lines().map(str::trim).filter(|l| !l.is_empty()).collect()
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

fn print_section(title: &str, lines: &[String], drift: &mut bool) {
    if lines.is_empty() {
        return;
    }
    println!("{title}");
    for line in lines {
        println!("{line}");
    }
    *drift = true;
}

enum Grade {
    Ok,
    Warn,
    Fail,
}

/// Machine health, no config needed: the deployment, the convergence
/// machinery, and the hardware basics a desktop lives on. Read-only.
/// A finding that has a cure carries it as an action — a diagnosis
/// without its next command is a dead end.
pub fn doctor() -> Result<()> {
    let mut warns = 0u32;
    let mut fails = 0u32;
    let mut report = |grade: Grade, name: &str, detail: String, fix: Option<Action>| {
        let mark = match grade {
            Grade::Ok => "ok  ",
            Grade::Warn => {
                warns += 1;
                "warn"
            }
            Grade::Fail => {
                fails += 1;
                "FAIL"
            }
        };
        println!("{mark}  {name}: {detail}");
        if let Some(fix) = fix {
            println!("      → {}   {}", fix.cmd, fix.why);
        }
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
                    "systemd-remount-fs failed — known-benign: Anaconda's fstab `/` line can't be remounted over composefs".into(),
                    None,
                );
            }
        }
        Err(_) => report(Grade::Warn, "units", "systemctl unavailable".into(), None),
    }

    if Path::new("/usr/lib/kuma").is_dir() {
        check_convergence(&mut report);
    } else {
        report(
            Grade::Warn,
            "kuma",
            "not running a kuma image — convergence checks skipped".into(),
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

    println!();
    match (fails, warns) {
        (0, 0) => {
            println!("All checks passed.");
            Ok(())
        }
        (0, _) => {
            println!("{warns} warning(s).");
            Ok(())
        }
        _ => bail!("{fails} check(s) failed, {warns} warning(s)"),
    }
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
        detail.push_str("; rollback available");
    }
    report(Grade::Ok, "deployment", detail, fix);

    // A build that was never switched to is the easy thing to forget.
    // Two storages are in play: the rootless build, and root's copy that
    // bootc actually deploys. Manifest digests don't survive the
    // save/load sync between them — only image IDs (config digests) do —
    // so compare IDs across storages and digests only within root's.
    let local_id = host_output(&[
        "podman", "image", "inspect", "--format", "{{.Id}}", "localhost/kuma:latest",
    ]);
    if let Ok(local_id) = local_id {
        let root = host_output(&[
            "sudo", "podman", "image", "inspect", "--format", "{{.Id}} {{.Digest}}",
            "localhost/kuma:latest",
        ])
        .unwrap_or_default();
        let (root_id, root_digest) = root.split_once(' ').unwrap_or(("", ""));
        let deployed: Vec<String> = [Some(&booted), staged.as_ref()]
            .into_iter()
            .flatten()
            .filter_map(|s| digest_of(s))
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
                    "remote missing — flatpak convergence cannot install".into(),
                    Some(fix),
                );
            }
        }
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
        (false, false) => report(Grade::Warn, "gpu", format!("{} bound, but no render node — software rendering likely", drivers.join(", ")), None),
        (true, _) => report(Grade::Warn, "gpu", "no GPU driver bound (VM or headless?)".into(), None),
    }
}
