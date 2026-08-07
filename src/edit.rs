//! Day-2 edits: `kuma add` and `kuma remove` rewrite kuma.toml in place.
//! The file is the interface — comments and layout are the owner's, so the
//! edits go through toml_edit (format-preserving) rather than a parse and
//! re-serialize that would flatten the whole document.

use crate::config::Config;
use crate::state::{action_json, print_actions, Action};
use anyhow::{Context, Result};
use std::path::Path;
use toml_edit::{Array, DocumentMut, Item, Table, Value};

const LISTS: &[&str] = &["rpm", "flatpak", "brew"];

pub fn add(path: &Path, list: &str, names: &[String], json: bool) -> Result<()> {
    let mut doc = load(path)?;
    let arr = list_array_mut(&mut doc, list)?;
    let mut added: Vec<&str> = Vec::new();
    let mut already: Vec<&str> = Vec::new();
    for name in names {
        if contains(arr, name) {
            already.push(name);
            continue;
        }
        push_matching_style(arr, name);
        added.push(name);
    }
    if !added.is_empty() {
        store(path, &doc)?;
    }
    let (actions, converge_note) = if added.is_empty() {
        (Vec::new(), None)
    } else {
        apply_edges(list == "rpm")
    };
    if json {
        println!(
            "{}",
            serde_json::json!({
                "ok": true, "list": list, "declared": added, "already_declared": already,
                "note": converge_note,
                "actions": actions.iter().map(action_json).collect::<Vec<_>>(),
            })
        );
        return Ok(());
    }
    for name in &already {
        println!("{name} is already declared in [packages].{list}");
    }
    if added.is_empty() {
        return Ok(());
    }
    println!("Declared in [packages].{list}: {}", added.join(", "));
    print_apply_hint(&actions, converge_note);
    Ok(())
}

/// All-or-nothing: every name must be found somewhere in [packages] before
/// anything is written, so a typo in one name can't half-apply the rest.
pub fn remove(path: &Path, names: &[String], json: bool) -> Result<()> {
    let mut doc = load(path)?;
    let mut removed: Vec<(&str, &str)> = Vec::new();
    for name in names {
        let list = LISTS
            .iter()
            .find(|list| {
                list_array(&doc, list).is_some_and(|arr| contains(arr, name))
            })
            .with_context(|| format!("{name} is not declared in any [packages] list"))?;
        let arr = list_array_mut(&mut doc, list)?;
        let idx = position(arr, name).expect("just found it");
        arr.remove(idx);
        removed.push((name, list));
    }
    store(path, &doc)?;
    let (actions, converge_note) =
        apply_edges(removed.iter().any(|(_, list)| *list == "rpm"));
    if json {
        println!(
            "{}",
            serde_json::json!({
                "ok": true,
                "removed": removed.iter().map(|(name, list)| serde_json::json!({
                    "item": name, "list": list,
                })).collect::<Vec<_>>(),
                "note": converge_note,
                "actions": actions.iter().map(action_json).collect::<Vec<_>>(),
            })
        );
        return Ok(());
    }
    for (name, list) in &removed {
        println!("Removed {name} from [packages].{list}");
    }
    print_apply_hint(&actions, converge_note);
    Ok(())
}

fn load(path: &Path) -> Result<DocumentMut> {
    let text = std::fs::read_to_string(path).with_context(|| {
        format!(
            "cannot read {}; run `kuma init` to start one here, or point --config at yours",
            path.display()
        )
    })?;
    text.parse()
        .with_context(|| format!("invalid config in {}", path.display()))
}

/// The edited document must still be a valid declaration — the same rules
/// `kuma build` enforces — before it replaces the file on disk.
fn store(path: &Path, doc: &DocumentMut) -> Result<()> {
    let text = doc.to_string();
    let config: Config =
        toml::from_str(&text).context("refusing to write: the edit breaks the config")?;
    config.validate().context("refusing to write: the edit breaks the config")?;
    std::fs::write(path, text).with_context(|| format!("cannot write {}", path.display()))
}

fn list_array<'a>(doc: &'a DocumentMut, list: &str) -> Option<&'a Array> {
    doc.get("packages")?.get(list)?.as_array()
}

fn list_array_mut<'a>(doc: &'a mut DocumentMut, list: &str) -> Result<&'a mut Array> {
    let packages = doc
        .entry("packages")
        .or_insert(Item::Table(Table::new()));
    let table = packages
        .as_table_mut()
        .context("[packages] is not a table")?;
    table
        .entry(list)
        .or_insert(Item::Value(Value::Array(Array::new())))
        .as_array_mut()
        .with_context(|| format!("packages.{list} is not an array"))
}

fn contains(arr: &Array, name: &str) -> bool {
    position(arr, name).is_some()
}

fn position(arr: &Array, name: &str) -> Option<usize> {
    arr.iter().position(|v| v.as_str() == Some(name))
}

/// Appending to a multi-line array with default decor would glue the new
/// entry onto the last line; give it its own indented line to match.
fn push_matching_style(arr: &mut Array, name: &str) {
    let multiline = arr
        .iter()
        .any(|v| v.decor().prefix().and_then(|p| p.as_str()).is_some_and(|p| p.contains('\n')));
    arr.push(name);
    if multiline {
        let last = arr.len() - 1;
        if let Some(v) = arr.get_mut(last) {
            v.decor_mut().set_prefix("\n    ");
        }
    }
}

/// Every list lives in the image (flatpak and brew declarations are baked
/// at /usr/lib/kuma), so the apply path is always a rebuild; the flatpak
/// and brew installs then converge on the machine after the switch. Where
/// the build goes next depends on the machine — same edges as build()'s.
fn apply_edges(rpm: bool) -> (Vec<Action>, Option<&'static str>) {
    let mut actions =
        vec![Action::new("build", "kuma build", "bake the edit into a new image")];
    if Path::new("/run/ostree-booted").exists() {
        actions.push(Action::new(
            "switch",
            "kuma switch",
            "stage it onto this machine (applies on reboot)",
        ));
    } else {
        actions.push(Action::new("vm", "kuma vm", "boot the result in a QEMU VM"));
    }
    let converge_note = (!rpm)
        .then_some("flatpak and brew changes converge on the machine at boot and daily");
    (actions, converge_note)
}

fn print_apply_hint(actions: &[Action], converge_note: Option<&str>) {
    println!();
    print_actions(actions);
    if converge_note.is_some() {
        println!("\nFlatpak and brew changes converge on the machine at boot and daily.");
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    fn write(dir: &Path, text: &str) -> std::path::PathBuf {
        let path = dir.join("kuma.toml");
        std::fs::write(&path, text).unwrap();
        path
    }

    const CONFIG: &str = "# my system\nschema_version = 1\n\n[packages]\n# tools\nrpm = [\"fish\"]\nflatpak = [\n    \"org.gnome.Loupe\",\n    \"org.gnome.Papers\",\n]\n";

    #[test]
    fn add_preserves_comments_and_layout() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), CONFIG);
        super::add(&path, "rpm", &["htop".into()], false).unwrap();
        let out = std::fs::read_to_string(&path).unwrap();
        assert!(out.contains("# my system"));
        assert!(out.contains("# tools"));
        assert!(out.contains("rpm = [\"fish\", \"htop\"]"));
    }

    #[test]
    fn add_matches_multiline_style() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), CONFIG);
        super::add(&path, "flatpak", &["org.gnome.Calculator".into()], false).unwrap();
        let out = std::fs::read_to_string(&path).unwrap();
        assert!(out.contains("\n    \"org.gnome.Calculator\""));
    }

    #[test]
    fn add_creates_missing_list() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "schema_version = 1\n");
        super::add(&path, "brew", &["ripgrep".into()], false).unwrap();
        let out = std::fs::read_to_string(&path).unwrap();
        assert!(out.contains("[packages]"));
        assert!(out.contains("brew = [\"ripgrep\"]"));
    }

    #[test]
    fn add_rejects_names_the_build_would_reject() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), CONFIG);
        assert!(super::add(&path, "rpm", &["fish; rm -rf /".into()], false).is_err());
        // and the file is untouched
        assert_eq!(std::fs::read_to_string(&path).unwrap(), CONFIG);
    }

    #[test]
    fn remove_finds_the_right_list() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), CONFIG);
        super::remove(&path, &["org.gnome.Papers".into(), "fish".into()], false).unwrap();
        let out = std::fs::read_to_string(&path).unwrap();
        assert!(!out.contains("org.gnome.Papers"));
        assert!(out.contains("rpm = []"));
        assert!(out.contains("org.gnome.Loupe"));
    }

    #[test]
    fn remove_unknown_name_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), CONFIG);
        assert!(super::remove(&path, &["fish".into(), "nope".into()], false).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), CONFIG);
    }
}
