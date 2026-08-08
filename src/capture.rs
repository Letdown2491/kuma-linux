//! `kuma capture`: the machine proposes, you dispose.
//!
//! Every declarative system treats drift as failure. The machine deviates,
//! the tool corrects it, the deviation is an error to be erased — which is
//! why the imperative escape hatches always feel like cheating, and why
//! things installed in a hurry never make it into the declaration.
//!
//! Capture is the second exit. What this machine has and the declaration
//! doesn't name is a *proposal* against kuma.toml: review it as a diff of
//! your declaration rather than a diff of your system, and either keep it
//! (capture) or drop it (sync). Experiment imperatively, promote
//! deliberately.
//!
//! The invariant that makes it safe: **capture never touches the
//! machine.** It reads the machine and writes the file. Convergence
//! authority stays exactly where it was, so this is as safe to run out of
//! curiosity as `kuma diff` is — dry run by default, `--yes` to write.

use crate::config::Config;
use crate::edit;
use crate::inspect::{candidates, observe, Candidate};
use crate::state::{action_json, print_actions, Action};
use anyhow::{Context, Result};
use std::path::Path;

/// What capture is allowed to write, ever.
///
/// rpm is absent because there is nothing to capture: a bootc machine
/// can't install one imperatively, so [packages].rpm is already
/// declarative by construction.
///
/// [user] is absent for a harder reason. That section holds a password
/// hash, and this repository's history was rewritten three times to get
/// one back out of git. A verb that automatically copies observed machine
/// state into a file that gets committed and baked world-readable into an
/// image is that same mistake on a schedule. Same for [system]: hostname
/// and timezone are machine state by an explicit design principle, and a
/// capture verb is exactly the pressure that would erode it. Neither is a
/// TODO. They are the boundary.
const CAPTURABLE: &[&str] = &["flatpak", "brew"];

pub fn capture(
    config_path: &Path,
    config: &Config,
    names: &[String],
    yes: bool,
    json: bool,
) -> Result<()> {
    let machine = observe(config);
    let found = candidates(config, &machine);
    debug_assert!(found.iter().all(|c| CAPTURABLE.contains(&c.list)));

    let selected: Vec<&Candidate> = if names.is_empty() {
        found.iter().filter(|c| !c.promotes).collect()
    } else {
        // All-or-nothing, like `remove`: one unknown name shouldn't
        // half-apply the rest.
        names
            .iter()
            .map(|name| {
                found.iter().find(|c| c.item == *name).with_context(|| {
                    format!(
                        "{name} is not undeclared on this machine; `kuma capture` lists what is"
                    )
                })
            })
            .collect::<Result<Vec<_>>>()?
    };

    // Named but not offered by default: capturing one changes what it is,
    // so it is worth saying they exist rather than silently omitting them.
    let opt_in: Vec<&str> = found.iter().filter(|c| c.promotes).map(|c| c.item.as_str()).collect();

    if selected.is_empty() {
        if json {
            println!(
                "{}",
                serde_json::json!({
                    "ok": true, "written": false, "captured": [],
                    "candidates": candidates_json(&found), "actions": [],
                })
            );
            return Ok(());
        }
        println!(
            "Nothing to capture: this machine runs nothing {} doesn't name.",
            config_path.display()
        );
        if !opt_in.is_empty() {
            print_opt_in(&opt_in);
        }
        return Ok(());
    }

    let items: Vec<(&str, &str)> = selected.iter().map(|c| (c.list, c.item.as_str())).collect();

    if !yes {
        if json {
            println!(
                "{}",
                serde_json::json!({
                    "ok": true, "written": false, "captured": [],
                    "candidates": candidates_json(&found),
                    "actions": [action_json(&yes_action(names))],
                })
            );
            return Ok(());
        }
        println!("Would declare in {}:", config_path.display());
        print_proposal(&selected);
        println!();
        print_actions(&[yes_action(names)]);
        if !opt_in.is_empty() {
            print_opt_in(&opt_in);
        }
        return Ok(());
    }

    edit::declare(config_path, &items)?;
    // Capture only ever writes flatpak and brew, never rpm, so the apply
    // path is always the convergent one.
    let (actions, converge_note) = edit::apply_edges(false);

    if json {
        println!(
            "{}",
            serde_json::json!({
                "ok": true,
                "written": true,
                "captured": items.iter().map(|(list, item)| serde_json::json!({
                    "list": list, "item": item,
                })).collect::<Vec<_>>(),
                "candidates": candidates_json(&found),
                "note": converge_note,
                "actions": actions.iter().map(action_json).collect::<Vec<_>>(),
            })
        );
        return Ok(());
    }

    println!("Declared in {}:", config_path.display());
    print_proposal(&selected);
    println!();
    print_actions(&actions);
    if converge_note.is_some() {
        println!("\nFlatpak and brew changes converge on the machine at boot and daily.");
    }
    Ok(())
}

/// The dry run's one edge, carrying the names it was narrowed to so the
/// printed command is the command that runs.
fn yes_action(names: &[String]) -> Action {
    let cmd = if names.is_empty() {
        "kuma capture --yes".to_string()
    } else {
        format!("kuma capture {} --yes", names.join(" "))
    };
    Action::new("capture", cmd, "write these into the declaration")
}

fn print_proposal(selected: &[&Candidate]) {
    let width = selected.iter().map(|c| c.item.chars().count()).max().unwrap_or(0);
    let list_width = selected.iter().map(|c| c.list.chars().count()).max().unwrap_or(0);
    for c in selected {
        let why = if c.doomed {
            "convergence would remove it"
        } else {
            "yours already; declaring it makes it reproducible"
        };
        let list = format!("[packages].{}", c.list);
        println!("  + {:<width$}  {:<w2$}  {why}", c.item, list, w2 = list_width + 11);
    }
}

/// Promoting a --user flatpak to a system one is a real change, not a
/// bookkeeping one, so it happens only when asked for by name.
fn print_opt_in(items: &[&str]) {
    println!("\nPer-user flatpaks stay yours and are captured only by name: {}", items.join(", "));
    println!("Declaring one installs it system-wide and hands it to convergence.");
}

fn candidates_json(found: &[Candidate]) -> Vec<serde_json::Value> {
    found
        .iter()
        .map(|c| {
            serde_json::json!({
                "list": c.list,
                "item": c.item,
                "doomed": c.doomed,
                "by_name_only": c.promotes,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_yes_edge_repeats_the_narrowing() {
        // A dry run narrowed to names must print the command that applies
        // *those* names; a bare --yes would take everything on offer.
        assert_eq!(yes_action(&[]).cmd, "kuma capture --yes");
        assert_eq!(
            yes_action(&["ghostty".to_string(), "jq".to_string()]).cmd,
            "kuma capture ghostty jq --yes"
        );
    }

    #[test]
    fn capture_writes_only_package_lists() {
        // The boundary is the point of the verb: [user] holds a password
        // hash and [system] holds machine state, and neither is ever a
        // capture target. Pin it so a later "just one more list" has to
        // argue with a test.
        assert_eq!(CAPTURABLE, &["flatpak", "brew"]);
    }
}
