//! The applications `kuma menu` lists.
//!
//! Reading `.desktop` files is not what a system tool wants to be doing,
//! and the menu delegated it to fuzzel until the delegation showed: an
//! Apps row that opened a second launcher on top of the first. Listing
//! them here puts everything on one surface, where typing `firefox` at
//! the top of the menu finds Firefox the same way `reboot` finds reboot.
//!
//! The cost is that the desktop entry spec has to actually be
//! implemented, and the parts people skip are the parts that bite. On
//! the machine this was written on, of 32 entries: **11 are
//! `NoDisplay=true`** and must never appear, and **12 carry field codes**
//! (`%U`, `%F`) that reach the program as literal arguments if nothing
//! strips them. `TryExec` gates four more on a binary existing.
//!
//! So the parse is the whole module, the IO is three functions at the
//! bottom, and the rules that decide what a person sees are pure and
//! tested against the shapes that actually ship.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// One launchable application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct App {
    /// The desktop file ID (`org.gnome.Calculator.desktop`, or
    /// `foo-bar.desktop` for a nested file). What the launch count is
    /// remembered against, and what makes an entry earlier in the search
    /// path shadow a later one, per the spec.
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) icon: String,
    pub(crate) argv: Vec<String>,
    /// `Terminal=true`: the program expects a terminal to run in.
    pub(crate) terminal: bool,
}

/// The `[Desktop Entry]` group's keys, before any of them are judged.
#[derive(Debug, Default)]
struct Entry {
    keys: HashMap<String, String>,
}

impl Entry {
    fn get(&self, key: &str) -> Option<&str> {
        self.keys.get(key).map(String::as_str)
    }

    fn flag(&self, key: &str) -> bool {
        self.get(key) == Some("true")
    }

    fn list(&self, key: &str) -> Vec<&str> {
        self.get(key)
            .map(|value| value.split(';').filter(|part| !part.is_empty()).collect())
            .unwrap_or_default()
    }
}

/// Parse the `[Desktop Entry]` group.
///
/// Only that group: an entry's `[Desktop Action ...]` groups are a
/// second menu per application and this list is flat enough already.
/// Locale-suffixed keys (`Name[de]`) are stored under their own
/// suffixed names and simply never looked up: the unsuffixed key is the
/// one the spec calls the default, and kuma has no locale negotiation to
/// do better with. Skipping them explicitly would be a branch that
/// changes nothing.
fn parse(text: &str) -> Entry {
    let mut entry = Entry::default();
    let mut inside = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            inside = line == "[Desktop Entry]";
            continue;
        }
        if !inside || line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        // First wins. A key repeated inside one group is malformed, and
        // the spec says nothing about it; picking an end and saying so
        // means two machines reading the same broken file agree.
        entry.keys.entry(key.to_string()).or_insert_with(|| value.trim().to_string());
    }
    entry
}

/// Whether this entry is something to show, given the desktop we are in.
///
/// `Hidden` means the entry is deleted, not merely quiet. `NoDisplay`
/// means it exists to handle a file type or to be launched by another
/// program, and eleven of this machine's thirty-two say so.
fn shown(entry: &Entry, desktop: &str) -> bool {
    if entry.get("Type") != Some("Application") {
        return false;
    }
    if entry.flag("Hidden") || entry.flag("NoDisplay") {
        return false;
    }
    let only = entry.list("OnlyShowIn");
    if !only.is_empty() && !only.iter().any(|name| name.eq_ignore_ascii_case(desktop)) {
        return false;
    }
    if entry.list("NotShowIn").iter().any(|name| name.eq_ignore_ascii_case(desktop)) {
        return false;
    }
    true
}

/// Whether `TryExec` is satisfied. Absent means yes.
fn runnable(entry: &Entry, on_path: &dyn Fn(&str) -> bool) -> bool {
    match entry.get("TryExec") {
        None => true,
        Some(program) if program.starts_with('/') => Path::new(program).exists(),
        Some(program) => on_path(program),
    }
}

/// Split an `Exec=` value into arguments, dropping the field codes.
///
/// The spec's quoting, because the alternative is splitting on spaces
/// and breaking every program in a path with one in it. Field codes are
/// dropped rather than expanded: kuma launches an application with no
/// files and no URLs to hand it, so `%U` has nothing to become, and
/// leaving it in passes a literal `%U` as an argument. `%%` is the one
/// that survives, as a percent sign.
///
/// Returns `None` for an `Exec` that has no program left in it, which is
/// a broken entry rather than an empty command.
pub(crate) fn exec_argv(exec: &str) -> Option<Vec<String>> {
    let mut args: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut started = false;
    let mut quoted = false;
    let mut chars = exec.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '"' => {
                quoted = !quoted;
                started = true;
            }
            '\\' if quoted => {
                // Inside quotes a backslash escapes the shell characters
                // the spec reserves; anything else keeps both.
                match chars.next() {
                    Some(next @ ('"' | '`' | '$' | '\\')) => current.push(next),
                    Some(next) => {
                        current.push('\\');
                        current.push(next);
                    }
                    None => current.push('\\'),
                }
            }
            '%' => match chars.next() {
                Some('%') => {
                    current.push('%');
                    started = true;
                }
                // Every other code names something kuma is not supplying.
                Some(_) => {}
                None => {}
            },
            c if c.is_whitespace() && !quoted => {
                if started {
                    args.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            c => {
                current.push(c);
                started = true;
            }
        }
    }
    if started {
        args.push(current);
    }
    args.retain(|arg| !arg.is_empty());
    (!args.is_empty()).then_some(args)
}

/// Turn one file's text into an app, or decide it is not one.
pub(crate) fn app_from(
    text: &str,
    id: &str,
    desktop: &str,
    on_path: &dyn Fn(&str) -> bool,
) -> Option<App> {
    let entry = parse(text);
    if !shown(&entry, desktop) || !runnable(&entry, on_path) {
        return None;
    }
    let name = entry.get("Name")?.to_string();
    let argv = exec_argv(entry.get("Exec")?)?;
    Some(App {
        id: id.to_string(),
        name,
        icon: entry.get("Icon").unwrap_or_default().to_string(),
        argv,
        terminal: entry.flag("Terminal"),
    })
}

/// The directories applications are found in, most specific first.
///
/// `XDG_DATA_DIRS` already carries the flatpak export directories on a
/// kuma machine, so there is nothing to special-case: an exported
/// flatpak is a desktop entry in a data directory like any other.
pub(crate) fn search_dirs() -> Vec<PathBuf> {
    let home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")));
    let dirs = std::env::var("XDG_DATA_DIRS").ok().filter(|value| !value.is_empty());
    search_dirs_from(home.as_deref(), dirs.as_deref())
}

/// The search path, as a function of the two variables that decide it,
/// so the ordering that makes shadowing mean anything can be asserted
/// without the test depending on the machine it runs on.
fn search_dirs_from(home: Option<&Path>, data_dirs: Option<&str>) -> Vec<PathBuf> {
    let dirs = data_dirs.unwrap_or("/usr/local/share:/usr/share");
    home.map(PathBuf::from)
        .into_iter()
        .chain(dirs.split(':').filter(|part| !part.is_empty()).map(PathBuf::from))
        .map(|dir| dir.join("applications"))
        .collect()
}

/// The desktop file ID for a file found under `root`: its path relative
/// to the applications directory, with separators turned into dashes.
fn desktop_id(root: &Path, file: &Path) -> String {
    file.strip_prefix(root)
        .unwrap_or(file)
        .to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "-")
}

/// Every application in `dirs`, first occurrence of an ID winning.
pub(crate) fn discover(
    dirs: &[PathBuf],
    desktop: &str,
    on_path: &dyn Fn(&str) -> bool,
) -> Vec<App> {
    let mut seen: Vec<String> = Vec::new();
    let mut apps: Vec<App> = Vec::new();
    for dir in dirs {
        for file in desktop_files(dir) {
            let id = desktop_id(dir, &file);
            // The spec's shadowing rule: an entry earlier in the search
            // path replaces a later one with the same ID, whether or not
            // the earlier one is displayable. A user who hides an app in
            // ~/.local/share must not have /usr/share put it back.
            if seen.contains(&id) {
                continue;
            }
            seen.push(id.clone());
            let Ok(text) = std::fs::read_to_string(&file) else {
                continue;
            };
            if let Some(app) = app_from(&text, &id, desktop, on_path) {
                apps.push(app);
            }
        }
    }
    apps
}

/// Every `.desktop` under `dir`, including one level of subdirectory,
/// which is where the spec puts vendor-prefixed entries.
fn desktop_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Ok(nested) = std::fs::read_dir(&path) {
                out.extend(
                    nested
                        .flatten()
                        .map(|entry| entry.path())
                        .filter(|path| path.extension().is_some_and(|ext| ext == "desktop")),
                );
            }
        } else if path.extension().is_some_and(|ext| ext == "desktop") {
            out.push(path);
        }
    }
    out.sort();
    out
}

/// Sort the most-launched first, then by name.
///
/// Without this the list is alphabetical forever, which is the one thing
/// a launcher must not be: fuzzel keeps exactly this count for the apps
/// it launches itself, and taking the list over means taking the
/// counting over too.
pub(crate) fn rank(apps: &mut [App], counts: &HashMap<String, u64>) {
    apps.sort_by(|a, b| {
        let by_count = counts.get(&b.id).unwrap_or(&0).cmp(counts.get(&a.id).unwrap_or(&0));
        by_count.then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
}

/// Where the launch counts live.
pub(crate) fn cache_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))?;
    Some(base.join("kuma/menu-apps"))
}

/// Parse `count id` lines. A malformed line is skipped rather than
/// fatal: this is a cache, and a person whose menu refuses to open
/// because a counter file got truncated is worse off than one whose
/// ordering resets.
pub(crate) fn parse_counts(text: &str) -> HashMap<String, u64> {
    text.lines()
        .filter_map(|line| {
            let (count, id) = line.trim().split_once(' ')?;
            Some((id.trim().to_string(), count.parse().ok()?))
        })
        .collect()
}

pub(crate) fn load_counts(path: &Path) -> HashMap<String, u64> {
    std::fs::read_to_string(path).map(|text| parse_counts(&text)).unwrap_or_default()
}

pub(crate) fn render_counts(counts: &HashMap<String, u64>) -> String {
    let mut lines: Vec<(u64, &String)> = counts.iter().map(|(id, count)| (*count, id)).collect();
    lines.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
    lines.iter().map(|(count, id)| format!("{count} {id}\n")).collect()
}

/// Remember that this app was launched. Best effort: a cache that cannot
/// be written is a menu that forgets, not a menu that fails.
pub(crate) fn record(id: &str) {
    let Some(path) = cache_path() else {
        return;
    };
    let mut counts = load_counts(&path);
    *counts.entry(id.to_string()).or_insert(0) += 1;
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&path, render_counts(&counts));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn always(_: &str) -> bool {
        true
    }
    fn never(_: &str) -> bool {
        false
    }

    /// Eleven of this machine's thirty-two entries are NoDisplay. They
    /// exist to open a file type or to be launched by another program,
    /// and a list that shows them is a list nobody trusts.
    #[test]
    fn an_entry_that_asks_not_to_be_shown_is_not_shown() {
        let hidden =
            "[Desktop Entry]\nType=Application\nName=Handler\nExec=handler\nNoDisplay=true\n";
        assert_eq!(app_from(hidden, "h.desktop", "niri", &always), None);
        let deleted = "[Desktop Entry]\nType=Application\nName=Gone\nExec=gone\nHidden=true\n";
        assert_eq!(app_from(deleted, "g.desktop", "niri", &always), None);
        let other = "[Desktop Entry]\nType=Link\nName=Bookmark\nURL=https://example.invalid\n";
        assert_eq!(app_from(other, "b.desktop", "niri", &always), None);
    }

    /// Twelve of them carry field codes. Passed through, the program
    /// receives a literal `%U` as its first argument.
    #[test]
    fn field_codes_are_dropped_rather_than_handed_to_the_program() {
        assert_eq!(exec_argv("file-roller %U"), Some(vec!["file-roller".to_string()]));
        assert_eq!(exec_argv("kitty +open %U"), Some(vec!["kitty".into(), "+open".into()]));
        assert_eq!(
            exec_argv("app %f %F %u %U %d %D %n %N %i %c %k %v %m"),
            Some(vec!["app".into()])
        );
        assert_eq!(
            exec_argv("app 100%% sure"),
            Some(vec!["app".into(), "100%".into(), "sure".into()])
        );
    }

    /// The spec's quoting, because splitting on spaces breaks the first
    /// program that lives in a path with one in it.
    #[test]
    fn a_quoted_argument_survives_being_split() {
        assert_eq!(
            exec_argv(r#""/opt/My App/run" --flag"#),
            Some(vec!["/opt/My App/run".to_string(), "--flag".to_string()])
        );
        assert_eq!(
            exec_argv(r#"sh -c "echo \"hi\"""#),
            Some(vec!["sh".into(), "-c".into(), r#"echo "hi""#.into()])
        );
        assert_eq!(exec_argv("   "), None, "an Exec with no program is not a command");
        assert_eq!(exec_argv("%U"), None, "nor is one that was only a field code");
    }

    /// TryExec names a binary whose absence means the entry is not
    /// installed, whatever else it claims.
    #[test]
    fn an_entry_whose_program_is_missing_is_not_offered() {
        let text = "[Desktop Entry]\nType=Application\nName=Thing\nExec=thing\nTryExec=thing\n";
        assert!(app_from(text, "t.desktop", "niri", &always).is_some());
        assert_eq!(app_from(text, "t.desktop", "niri", &never), None);
    }

    /// OnlyShowIn and NotShowIn scope an entry to a desktop. kuma runs
    /// niri, and a GNOME-only control panel in this list is a row that
    /// opens something confusing.
    #[test]
    fn an_entry_scoped_to_another_desktop_stays_there() {
        let gnome =
            "[Desktop Entry]\nType=Application\nName=Tweaks\nExec=tweaks\nOnlyShowIn=GNOME;\n";
        assert_eq!(app_from(gnome, "t.desktop", "niri", &always), None);
        assert!(app_from(gnome, "t.desktop", "GNOME", &always).is_some());
        let not_niri =
            "[Desktop Entry]\nType=Application\nName=Thing\nExec=thing\nNotShowIn=niri;\n";
        assert_eq!(app_from(not_niri, "t.desktop", "niri", &always), None);
    }

    /// Only the first group, and only the unsuffixed keys. A localised
    /// Name is somebody else's default, and the action groups are a
    /// second menu per app.
    #[test]
    fn only_the_desktop_entry_group_and_its_plain_keys_are_read() {
        let text = "[Desktop Entry]\nType=Application\nName=Files\nName[de]=Dateien\nExec=files\n\n[Desktop Action new]\nName=New Window\nExec=files --new\nIcon=folder\n";
        let app = app_from(text, "f.desktop", "niri", &always).expect("an application");
        assert_eq!(app.name, "Files", "the localised name is not the default one");
        assert_eq!(app.argv, vec!["files".to_string()], "the action's Exec is a different command");
        assert_eq!(app.icon, "", "a key that exists only in an action group is not this app's");

        // Malformed input gets a defined answer rather than a varying one.
        let twice = "[Desktop Entry]\nType=Application\nName=First\nName=Second\nExec=x\n";
        let app = app_from(twice, "t.desktop", "niri", &always).expect("an application");
        assert_eq!(app.name, "First", "a repeated key resolves the same way every time");
    }

    /// Most-launched first, then alphabetical, because a launcher that
    /// is alphabetical forever is the thing people notice immediately.
    #[test]
    fn the_most_launched_sort_first_and_ties_read_alphabetically() {
        let app = |id: &str, name: &str| App {
            id: id.into(),
            name: name.into(),
            icon: String::new(),
            argv: vec![name.to_lowercase()],
            terminal: false,
        };
        let mut apps = vec![app("c", "Cherry"), app("a", "apple"), app("b", "Banana")];
        let mut counts = HashMap::new();
        counts.insert("b".to_string(), 9);
        rank(&mut apps, &counts);
        let names: Vec<&str> = apps.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["Banana", "apple", "Cherry"], "count first, then case-insensitive name");
    }

    /// The count file survives a round trip, and a mangled one costs the
    /// ordering rather than the menu.
    #[test]
    fn counts_round_trip_and_a_broken_line_is_skipped() {
        let mut counts = HashMap::new();
        counts.insert("a.desktop".to_string(), 3);
        counts.insert("b.desktop".to_string(), 11);
        assert_eq!(parse_counts(&render_counts(&counts)), counts);
        let mangled = "11 b.desktop\nnonsense\n\n3 a.desktop\nx y\n";
        assert_eq!(parse_counts(mangled), counts, "only the readable lines count");
    }

    /// An ID is the path under the applications directory, so a vendor
    /// subdirectory does not collide with a top-level file.
    #[test]
    fn a_nested_entry_gets_the_id_the_spec_gives_it() {
        let root = Path::new("/usr/share/applications");
        assert_eq!(desktop_id(root, Path::new("/usr/share/applications/a.desktop")), "a.desktop");
        assert_eq!(
            desktop_id(root, Path::new("/usr/share/applications/kde4/b.desktop")),
            "kde4-b.desktop"
        );
    }

    /// The search path is most-specific first, which is what makes the
    /// shadowing rule mean anything.
    #[test]
    fn the_search_path_puts_the_home_directory_first() {
        let dirs = search_dirs_from(Path::new("/home/x/.local/share").into(), Some("/a:/b"));
        assert_eq!(
            dirs,
            [
                PathBuf::from("/home/x/.local/share/applications"),
                PathBuf::from("/a/applications"),
                PathBuf::from("/b/applications"),
            ],
            "most specific first, which is what makes shadowing mean anything"
        );
        assert_eq!(
            search_dirs_from(None, None),
            [
                PathBuf::from("/usr/local/share/applications"),
                PathBuf::from("/usr/share/applications")
            ],
            "the spec's defaults when nothing is set"
        );
        assert!(search_dirs().iter().all(|dir| dir.ends_with("applications")));
    }
}
