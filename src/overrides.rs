//! Flatpak permission overrides, converged one key at a time.
//!
//! An override file is a keyfile, and kuma is never its only author:
//! Flatseal writes the same files, and so does anyone who runs `flatpak
//! override` by hand. So the unit of ownership here is the **key**, not
//! the file and not the app. kuma sets the keys a declaration names,
//! removes the keys it set that the declaration stopped naming, and
//! copies every other line through untouched. That is `73771ab`'s rule
//! one level down: convergence takes back only what it gave.

use crate::config::{AppOverride, Scope};
use anyhow::{Context, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub const CONTEXT: &str = "Context";
pub const ENVIRONMENT: &str = "Environment";
pub const SESSION_BUS: &str = "Session Bus Policy";
pub const SYSTEM_BUS: &str = "System Bus Policy";

/// One key kuma has authority over, as `group\tkey`. The state file
/// stores these verbatim, one per line, so a group name with a space in
/// it ("Session Bus Policy") needs no quoting and no parser.
pub fn owned_id(group: &str, key: &str) -> String {
    format!("{group}\t{key}")
}

/// Every key a declaration asks for, in the order the file writes them:
/// group, key, value.
pub fn declared(app: &AppOverride) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    for (key, values) in app.context_lists() {
        if values.is_empty() {
            continue;
        }
        // flatpak writes these as semicolon-terminated lists, trailing
        // separator included; matching it exactly is what lets a
        // declared file compare equal to one flatpak wrote itself.
        let mut value = String::new();
        for v in values {
            value.push_str(v);
            value.push(';');
        }
        out.push((CONTEXT.to_string(), key.to_string(), value));
    }
    for (name, value) in &app.environment {
        out.push((ENVIRONMENT.to_string(), name.clone(), value.clone()));
    }
    for (bus, policy) in &app.session_bus {
        out.push((SESSION_BUS.to_string(), bus.clone(), policy.as_str().to_string()));
    }
    for (bus, policy) in &app.system_bus {
        out.push((SYSTEM_BUS.to_string(), bus.clone(), policy.as_str().to_string()));
    }
    out
}

/// The declared keys as a standalone keyfile. This is what the image
/// bakes: kuma's half of the answer, with nothing of the machine's in
/// it.
pub fn render(app: &AppOverride) -> String {
    let mut out = String::new();
    let mut current = "";
    for (group, key, value) in declared(app) {
        if group != current {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&format!("[{group}]\n"));
            current = match group.as_str() {
                CONTEXT => CONTEXT,
                ENVIRONMENT => ENVIRONMENT,
                SESSION_BUS => SESSION_BUS,
                SYSTEM_BUS => SYSTEM_BUS,
                _ => unreachable!("declared() only emits kuma's four groups"),
            };
        }
        out.push_str(&format!("{key}={value}\n"));
    }
    out
}

/// A parsed keyfile that remembers everything it did not understand.
/// Comments, blank lines, and groups kuma has never heard of all survive
/// a round trip, because the file belongs to the machine and kuma is
/// only editing part of it.
#[derive(Debug, Default)]
struct KeyFile {
    groups: Vec<Group>,
}

#[derive(Debug)]
struct Group {
    name: String,
    lines: Vec<Line>,
}

#[derive(Debug)]
enum Line {
    Pair(String, String),
    Raw(String),
}

impl KeyFile {
    fn parse(text: &str) -> Self {
        let mut file = KeyFile::default();
        // Anything before the first header (comments, usually) lives in
        // a nameless group so it keeps its place at the top.
        file.groups.push(Group { name: String::new(), lines: Vec::new() });
        for line in text.lines() {
            let trimmed = line.trim();
            if let Some(name) = trimmed.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                file.groups.push(Group { name: name.to_string(), lines: Vec::new() });
                continue;
            }
            let group = file.groups.last_mut().expect("a group always exists");
            match trimmed.split_once('=') {
                Some((key, value)) if !trimmed.starts_with('#') && !key.trim().is_empty() => {
                    group.lines.push(Line::Pair(key.trim().to_string(), value.to_string()));
                }
                _ => group.lines.push(Line::Raw(line.to_string())),
            }
        }
        file
    }

    fn set(&mut self, group: &str, key: &str, value: &str) {
        if let Some(g) = self.groups.iter_mut().find(|g| g.name == group) {
            for line in g.lines.iter_mut() {
                if let Line::Pair(k, v) = line {
                    if k == key {
                        *v = value.to_string();
                        return;
                    }
                }
            }
            g.lines.push(Line::Pair(key.to_string(), value.to_string()));
            return;
        }
        self.groups.push(Group {
            name: group.to_string(),
            lines: vec![Line::Pair(key.to_string(), value.to_string())],
        });
    }

    fn remove(&mut self, group: &str, key: &str) {
        if let Some(g) = self.groups.iter_mut().find(|g| g.name == group) {
            g.lines.retain(|l| !matches!(l, Line::Pair(k, _) if k == key));
        }
    }

    /// True when nothing but whitespace and comments is left. flatpak
    /// treats an empty override file and an absent one the same way, but
    /// a file kuma emptied is litter, and litter is what this whole rung
    /// is about.
    fn is_empty(&self) -> bool {
        !self.groups.iter().any(|g| g.lines.iter().any(|l| matches!(l, Line::Pair(_, _))))
    }

    fn render(&self) -> String {
        let mut out = String::new();
        for group in &self.groups {
            let has_pairs = group.lines.iter().any(|l| matches!(l, Line::Pair(_, _)));
            // A group kuma emptied goes with its header; one that was
            // always empty was somebody's choice and stays.
            if !group.name.is_empty() && has_pairs {
                out.push_str(&format!("[{}]\n", group.name));
            }
            if !group.name.is_empty() && !has_pairs {
                continue;
            }
            for line in &group.lines {
                match line {
                    Line::Pair(k, v) => out.push_str(&format!("{k}={v}\n")),
                    Line::Raw(raw) => out.push_str(&format!("{raw}\n")),
                }
            }
        }
        out
    }
}

/// What one app's convergence did, so the caller can say it out loud
/// rather than reporting that something happened.
#[derive(Debug, Default, PartialEq)]
pub struct Changed {
    pub set: Vec<String>,
    pub removed: Vec<String>,
}

impl Changed {
    pub fn is_empty(&self) -> bool {
        self.set.is_empty() && self.removed.is_empty()
    }
}

/// Merge a declaration into whatever the machine has.
///
/// `previously` is what kuma set last time, as `owned_id` strings. A key
/// in `previously` that the declaration no longer names is kuma's to
/// remove; a key in neither was never kuma's and is left exactly where
/// it is, even when it sits in the same group and contradicts what was
/// just declared. That is not a bug: declaring is how you win that
/// argument, and `kuma diff` is how you see you are having it.
pub fn converge(
    live: &str,
    wanted: &[(String, String, String)],
    previously: &[String],
) -> (String, Changed, Vec<String>) {
    let mut file = KeyFile::parse(live);
    let mut changed = Changed::default();
    let mut now_owned = Vec::new();

    for (group, key, value) in wanted {
        let id = owned_id(group, key);
        let existing = file.groups.iter().find(|g| &g.name == group).and_then(|g| {
            g.lines.iter().find_map(|l| match l {
                Line::Pair(k, v) if k == key => Some(v.clone()),
                _ => None,
            })
        });
        if existing.as_deref() != Some(value.as_str()) {
            changed.set.push(id.clone());
        }
        file.set(group, key, value);
        now_owned.push(id);
    }
    for id in previously {
        if now_owned.contains(id) {
            continue;
        }
        let Some((group, key)) = id.split_once('\t') else { continue };
        let present = file.groups.iter().any(|g| {
            g.name == group && g.lines.iter().any(|l| matches!(l, Line::Pair(k, _) if k == key))
        });
        if present {
            file.remove(group, key);
            changed.removed.push(id.clone());
        }
    }

    let rendered = if file.is_empty() { String::new() } else { file.render() };
    (rendered, changed, now_owned)
}

/// Where a scope's override files live. The system store is flatpak's
/// own; the user store is under the caller's data directory, which is
/// why the user pass runs as the user rather than root reaching into a
/// home.
///
/// Every path here takes a root, so the whole pass can be run against a
/// directory tree in a test rather than asserted about in prose.
pub fn store(scope: Scope, root: &Path, home: &Path) -> PathBuf {
    match scope {
        Scope::System => root.join("var/lib/flatpak/overrides"),
        Scope::User => home.join(".local/share/flatpak/overrides"),
    }
}

/// What the image baked: one keyfile per app, holding kuma's keys and
/// nothing else.
pub fn declared_dir(scope: Scope, root: &Path) -> PathBuf {
    root.join("usr/lib/kuma/overrides").join(scope.as_str())
}

/// What kuma set last time. Machine state, so `/var` for the system
/// pass; the user pass keeps its own beside the user's other state,
/// because root writing into a home is how you race a running Flatseal.
pub fn state_path(scope: Scope, root: &Path, home: &Path) -> PathBuf {
    match scope {
        Scope::System => root.join("var/lib/kuma/overrides-owned"),
        Scope::User => home.join(".local/state/kuma/overrides-owned"),
    }
}

/// Read a baked file back into the keys it asks for. The baked file is
/// the declaration at runtime, so this is the only reader that matters
/// on a machine; `declared()` exists for the build that wrote it.
pub fn parse_declared(text: &str) -> Vec<(String, String, String)> {
    let file = KeyFile::parse(text);
    let mut out = Vec::new();
    for group in &file.groups {
        if group.name.is_empty() {
            continue;
        }
        for line in &group.lines {
            if let Line::Pair(k, v) = line {
                out.push((group.name.clone(), k.clone(), v.clone()));
            }
        }
    }
    out
}

fn read_state(path: &Path) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let Ok(text) = std::fs::read_to_string(path) else { return out };
    for line in text.lines() {
        let mut parts = line.splitn(3, '\t');
        let (Some(app), Some(group), Some(key)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        out.entry(app.to_string()).or_default().push(owned_id(group, key));
    }
    out
}

fn write_state(path: &Path, owned: &BTreeMap<String, Vec<String>>) -> Result<()> {
    let mut text = String::new();
    for (app, ids) in owned {
        for id in ids {
            text.push_str(&format!("{app}\t{id}\n"));
        }
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))
}

/// Converge one scope's whole store, and report per app.
///
/// The app list is the union of what the image declares and what kuma
/// set last time, because an app that left the declaration entirely is
/// exactly the one with keys to take back, and reading only the declared
/// directory would never look at it.
pub fn converge_store(scope: Scope, root: &Path, home: &Path) -> Result<Vec<(String, Changed)>> {
    let declared = declared_dir(scope, root);
    let store = store(scope, root, home);
    let state = state_path(scope, root, home);
    let previous = read_state(&state);

    let mut apps: BTreeSet<String> = previous.keys().cloned().collect();
    if let Ok(entries) = std::fs::read_dir(&declared) {
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                apps.insert(name.to_string());
            }
        }
    }

    let mut owned: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut report = Vec::new();
    for app in apps {
        let wanted = std::fs::read_to_string(declared.join(&app))
            .map(|t| parse_declared(&t))
            .unwrap_or_default();
        let path = store.join(&app);
        let live = std::fs::read_to_string(&path).unwrap_or_default();
        let empty = Vec::new();
        let (out, changed, now) = converge(&live, &wanted, previous.get(&app).unwrap_or(&empty));
        if out.is_empty() {
            if path.exists() {
                std::fs::remove_file(&path)
                    .with_context(|| format!("removing {}", path.display()))?;
            }
        } else if out != live {
            std::fs::create_dir_all(&store)
                .with_context(|| format!("creating {}", store.display()))?;
            std::fs::write(&path, &out).with_context(|| format!("writing {}", path.display()))?;
        }
        if !now.is_empty() {
            owned.insert(app.clone(), now);
        }
        if !changed.is_empty() {
            report.push((app, changed));
        }
    }
    write_state(&state, &owned)?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::BusPolicy;

    fn app_of(toml: &str) -> AppOverride {
        toml::from_str(toml).unwrap()
    }

    /// The baked file is what flatpak would have written, separator for
    /// separator. If this drifts, a declared override and a hand-made
    /// one stop comparing equal and `kuma diff` reports drift forever.
    #[test]
    fn a_declaration_renders_as_flatpak_writes_it() {
        let a = app_of(
            "filesystems = [\"home\", \"!xdg-config/kitty\"]\n\
             sockets = [\"wayland\"]\n\
             [environment]\n\
             MOZ_ENABLE_WAYLAND = \"1\"\n\
             [session-bus]\n\
             \"org.freedesktop.Flatpak\" = \"talk\"\n",
        );
        assert_eq!(
            render(&a),
            "[Context]\n\
             filesystems=home;!xdg-config/kitty;\n\
             sockets=wayland;\n\
             \n\
             [Environment]\n\
             MOZ_ENABLE_WAYLAND=1\n\
             \n\
             [Session Bus Policy]\n\
             org.freedesktop.Flatpak=talk\n"
        );
    }

    /// The whole feature in one test: kuma's key is set, the key beside
    /// it in the same group is somebody else's and survives untouched.
    #[test]
    fn convergence_edits_its_own_key_and_no_other() {
        let live = "[Context]\nfilesystems=home;\nsockets=x11;\n";
        let a = app_of("filesystems = [\"host\"]\n");
        let (out, changed, owned) = converge(live, &declared(&a), &[]);
        assert_eq!(out, "[Context]\nfilesystems=host;\nsockets=x11;\n");
        assert_eq!(changed.set, vec![owned_id(CONTEXT, "filesystems")]);
        assert!(changed.removed.is_empty());
        assert_eq!(owned, vec![owned_id(CONTEXT, "filesystems")]);
    }

    /// Undeclaring removes only what kuma set. The same test proves the
    /// other half: `sockets` was never kuma's, so dropping every
    /// declared key does not make it kuma's to delete.
    #[test]
    fn undeclaring_a_key_removes_it_and_leaves_the_rest() {
        let live = "[Context]\nfilesystems=host;\nsockets=x11;\n";
        let previously = vec![owned_id(CONTEXT, "filesystems")];
        let (out, changed, owned) = converge(live, &[], &previously);
        assert_eq!(out, "[Context]\nsockets=x11;\n");
        assert_eq!(changed.removed, previously);
        assert!(owned.is_empty());
    }

    /// A file kuma emptied is deleted rather than left as an empty
    /// stanza, and the caller learns that by being handed "".
    #[test]
    fn the_last_key_leaving_empties_the_file() {
        let live = "[Context]\nfilesystems=host;\n";
        let (out, _, _) = converge(live, &[], &[owned_id(CONTEXT, "filesystems")]);
        assert!(out.is_empty(), "nothing left, so nothing to write: {out:?}");
    }

    /// Comments and groups kuma does not know about are the machine's,
    /// and a converger that eats them is one nobody will run twice.
    #[test]
    fn everything_kuma_does_not_understand_survives() {
        let live =
            "# set by hand, 2026\n[Context]\nsockets=x11;\n\n[Some Future Group]\nkey=value\n";
        let a = app_of("devices = [\"dri\"]\n");
        let (out, _, _) = converge(live, &declared(&a), &[]);
        assert!(out.starts_with("# set by hand, 2026\n"), "{out}");
        assert!(out.contains("[Some Future Group]\nkey=value\n"), "{out}");
        assert!(out.contains("devices=dri;"), "{out}");
        assert!(out.contains("sockets=x11;"), "{out}");
    }

    /// Converging twice with the same declaration must report nothing
    /// the second time. Without this the boot unit would announce
    /// changes on every boot and the report would mean nothing.
    #[test]
    fn a_second_pass_changes_nothing() {
        let a = app_of("filesystems = [\"home\"]\n");
        let (once, first, owned) = converge("", &declared(&a), &[]);
        assert!(!first.is_empty());
        let (twice, second, _) = converge(&once, &declared(&a), &owned);
        assert_eq!(once, twice);
        assert!(second.is_empty(), "second pass reported {second:?}");
    }

    /// A key kuma set, then a person changed by hand, is kuma's to set
    /// back: that is what declaring it means. The report says so, which
    /// is the difference between convergence and a silent fight.
    #[test]
    fn a_hand_edit_to_a_declared_key_is_taken_back_and_reported() {
        let a = app_of("sockets = [\"wayland\"]\n");
        let owned = vec![owned_id(CONTEXT, "sockets")];
        let (out, changed, _) = converge("[Context]\nsockets=x11;\n", &declared(&a), &owned);
        assert_eq!(out, "[Context]\nsockets=wayland;\n");
        assert_eq!(changed.set, owned);
    }

    /// The whole runtime pass against a directory tree: what the image
    /// baked reaches the store, a key that was already there and is not
    /// kuma's survives, running twice reports nothing, and undeclaring
    /// the app takes back kuma's keys and only kuma's.
    ///
    /// This is the test that would have to fail for the feature to be
    /// wrong on a real machine; everything above it is a unit of it.
    #[test]
    fn a_scope_converges_against_a_real_tree() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let home = root.join("home/mira");
        let app = "org.example.App";

        let declared = declared_dir(Scope::System, root);
        std::fs::create_dir_all(&declared).unwrap();
        let decl = app_of("sockets = [\"wayland\"]\n");
        std::fs::write(declared.join(app), render(&decl)).unwrap();

        // the machine already had an opinion about a different key
        let store_dir = store(Scope::System, root, &home);
        std::fs::create_dir_all(&store_dir).unwrap();
        std::fs::write(store_dir.join(app), "[Context]\nfilesystems=home;\n").unwrap();

        let report = converge_store(Scope::System, root, &home).unwrap();
        let live = std::fs::read_to_string(store_dir.join(app)).unwrap();
        assert!(live.contains("sockets=wayland;"), "{live}");
        assert!(live.contains("filesystems=home;"), "kuma ate a key that was not its own: {live}");
        assert_eq!(report.len(), 1);

        // idempotent: the boot after this one has nothing to say
        assert!(converge_store(Scope::System, root, &home).unwrap().is_empty());

        // undeclared: kuma's key goes, the machine's key stays
        std::fs::remove_file(declared.join(app)).unwrap();
        let report = converge_store(Scope::System, root, &home).unwrap();
        assert_eq!(report.len(), 1);
        let live = std::fs::read_to_string(store_dir.join(app)).unwrap();
        assert!(!live.contains("sockets"), "{live}");
        assert!(live.contains("filesystems=home;"), "{live}");

        // and the state file no longer claims a key kuma does not set
        let state = std::fs::read_to_string(state_path(Scope::System, root, &home)).unwrap();
        assert!(state.is_empty(), "state still claims {state:?}");
    }

    /// A file that only ever held kuma's keys is removed rather than
    /// left behind empty, on the same pass that stops declaring them.
    #[test]
    fn a_file_kuma_emptied_is_deleted_from_the_store() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let home = root.join("home/mira");
        let app = "org.example.Only";
        let declared = declared_dir(Scope::User, root);
        std::fs::create_dir_all(&declared).unwrap();
        std::fs::write(declared.join(app), render(&app_of("devices = [\"dri\"]\n"))).unwrap();
        converge_store(Scope::User, root, &home).unwrap();
        let path = store(Scope::User, root, &home).join(app);
        assert!(path.exists());

        std::fs::remove_file(declared.join(app)).unwrap();
        converge_store(Scope::User, root, &home).unwrap();
        assert!(!path.exists(), "an emptied override file was left behind");
    }

    /// Baked and read back must be the same keys. The build writes the
    /// file and the machine reads it, and nothing else connects the two,
    /// so a change to either side that is not a change to both is a
    /// machine that converges to something the declaration did not say.
    #[test]
    fn what_the_build_writes_is_what_the_machine_reads() {
        let decl = app_of(
            "filesystems = [\"home\"]\n\
             [environment]\n\
             GTK_THEME = \"Adwaita:dark\"\n\
             [system-bus]\n\
             \"org.freedesktop.UPower\" = \"talk\"\n",
        );
        assert_eq!(parse_declared(&render(&decl)), declared(&decl));
    }

    /// Bus policies are written as flatpak spells them, and the parser
    /// is what stops "Talk" or "yes" from reaching the file.
    #[test]
    fn bus_policies_take_only_flatpaks_four_words() {
        let a = app_of("[system-bus]\n\"org.freedesktop.UPower\" = \"see\"\n");
        assert_eq!(a.system_bus["org.freedesktop.UPower"], BusPolicy::See);
        assert!(toml::from_str::<AppOverride>("[system-bus]\nx = \"Talk\"\n").is_err());
    }
}
