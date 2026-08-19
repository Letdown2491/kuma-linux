//! `kuma menu`: the launcher is the settings surface.
//!
//! The desktop kuma assembles has a coherence problem that no amount of
//! curation fixes: every device-level setting belongs to somebody else's
//! control panel, so wifi looks like one product, pairing like another,
//! and the system that owns the machine has no face at all. A settings
//! *application* is the obvious answer and the wrong one — it is a
//! desktop environment's job, it never finishes, and a half-built one is
//! worse than the panels it replaces because it looks official.
//!
//! So this is not an application. It is a menu rendered by the launcher
//! the desktop already has, themed by the file kuma already ships
//! (`assets/fuzzel.ini`), driven by `fuzzel --dmenu`, which kuma already
//! uses for the clipboard picker. Nothing new is installed and nothing
//! new is drawn.
//!
//! **The rule that keeps it honest: this menu never writes the
//! declaration.** `capture.rs` is the one deliberate path from "the
//! machine has this" to "the file says this", and its safety is the
//! ceremony — dry run, review, confirm. A menu entry is one keystroke
//! with no diff and no pause, so adding it as a second writer would not
//! add convenience, it would remove the only thing that made the first
//! writer safe. Declaration entries open the file or run a verb that has
//! its own confirmation; machine state (a lock, a suspend, a
//! notification mode) is changed immediately, because that is the half a
//! launcher is genuinely better at. `no_leaf_writes_the_declaration`
//! enforces it.
//!
//! **A leaf appears only when its program is here.** The tree is a pure
//! function of what `Tools` observed, so a menu built on a machine
//! without `nmtui` offers the graphical editor instead, and one built
//! without either offers neither rather than a row that does nothing.
//! That also means adding a package to a desktop set is all it takes to
//! change what the menu offers.
//!
//! **Nothing here runs `sudo` to decide what to show.** Availability is
//! read from PATH and from files anybody can read, never from
//! `bootc status`, so opening the menu never prompts. Entries whose verb
//! needs root prompt inside their own terminal window, where the prompt
//! belongs.

use anyhow::{Context, Result};
use std::collections::BTreeSet;
use std::path::Path;

use crate::host::{host_output_stdin, spawn_detached};

/// What is on this machine, as far as the menu is concerned.
///
/// Every program the tree can name is probed once, up front, so building
/// the tree is a pure function and the tests can build one for a machine
/// that has nothing.
pub(crate) struct Tools {
    present: BTreeSet<String>,
    editor: String,
}

/// Every external program any leaf may name. The list is here rather
/// than scattered through the tree so that "what does the menu depend
/// on" is one thing to read, and so `Tools::none()` can be exhaustive.
const PROBED: &[&str] = &[
    "fuzzel",
    "kitty",
    "cosmic-term",
    "nmtui",
    "nm-connection-editor",
    "blueman-manager",
    "wiremix",
    "pavucontrol",
    "wdisplays",
    "swaylock",
    "makoctl",
    "niri",
];

impl Tools {
    pub(crate) fn observe() -> Self {
        let present = PROBED
            .iter()
            .filter(|program| on_path(program))
            .map(|program| (*program).to_string())
            .collect();
        // $EDITOR is the person's answer and outranks any default. The
        // fallbacks are ordered by what a kuma image actually has: the
        // base composes ncurses and Fedora's minimal core carries vi,
        // while nano is only ever present because somebody declared it.
        let editor = std::env::var("EDITOR")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .or_else(|| ["nano", "vim", "vi"].iter().find(|e| on_path(e)).map(|e| (*e).to_string()))
            .unwrap_or_else(|| "vi".to_string());
        Self { present, editor }
    }

    /// A machine with none of the probed programs. The empty case the
    /// availability tests are written against.
    #[cfg(test)]
    pub(crate) fn none() -> Self {
        Self { present: BTreeSet::new(), editor: "vi".to_string() }
    }

    #[cfg(test)]
    pub(crate) fn with(programs: &[&str]) -> Self {
        Self {
            present: programs.iter().map(|p| (*p).to_string()).collect(),
            editor: "vi".to_string(),
        }
    }

    fn has(&self, program: &str) -> bool {
        self.present.contains(program)
    }

    /// The first of `candidates` this machine has. What lets one entry
    /// prefer a terminal tool and fall back to a graphical one without
    /// the tree growing a branch per machine.
    fn first(&self, candidates: &[&str]) -> Option<String> {
        candidates.iter().find(|c| self.has(c)).map(|c| (*c).to_string())
    }

    /// The terminal a leaf that prints gets spawned in. Not a fallback
    /// chain into `xterm`: an image either has the terminal its desktop
    /// set installed or it has no graphical session to run a menu in.
    fn terminal(&self) -> Option<String> {
        self.first(&["kitty", "cosmic-term"])
    }
}

fn on_path(program: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| dir.join(program).is_file())
}

/// How a leaf is run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Run {
    /// Detached, silent, immediate: locking, suspending, toggling a
    /// notification mode. The menu is spawned from a keybinding, so
    /// there is no terminal to print to and these have nothing to print.
    Detached,
    /// In a terminal window of its own. Everything that produces output
    /// or asks for a password: the menu cannot show either.
    Terminal,
}

#[derive(Debug, Clone)]
pub(crate) enum Kind {
    Submenu(Vec<Entry>),
    Leaf { argv: Vec<String>, run: Run },
}

#[derive(Debug, Clone)]
pub(crate) struct Entry {
    pub(crate) label: String,
    pub(crate) kind: Kind,
}

/// The label that pops one level. Spelled out rather than relying on
/// cancel, because a dmenu offers no other way to say "up" and a person
/// who cancels means "away", not "back".
const BACK: &str = "..  back";

fn submenu(label: &str, entries: Vec<Entry>) -> Option<Entry> {
    // A group whose every entry was unavailable is not an empty group,
    // it is an absent one. Otherwise a machine without a single network
    // tool still offers "Connect" and answers with nothing.
    if entries.is_empty() {
        return None;
    }
    let mut with_back = entries;
    with_back.push(Entry { label: BACK.to_string(), kind: Kind::Submenu(Vec::new()) });
    Some(Entry { label: label.to_string(), kind: Kind::Submenu(with_back) })
}

fn leaf(label: &str, argv: &[&str], run: Run) -> Entry {
    Entry {
        label: label.to_string(),
        kind: Kind::Leaf { argv: argv.iter().map(|a| (*a).to_string()).collect(), run },
    }
}

/// A leaf that runs a program only if this machine has it.
fn tool(tools: &Tools, label: &str, argv: &[&str], run: Run) -> Option<Entry> {
    let program = argv.first().copied().unwrap_or_default();
    tools.has(program).then(|| leaf(label, argv, run))
}

/// The whole menu, as a pure function of what is installed.
pub(crate) fn tree(tools: &Tools) -> Vec<Entry> {
    let mut root = Vec::new();

    if let Some(entry) = tools.has("fuzzel").then(|| leaf("apps", &["fuzzel"], Run::Detached)) {
        root.push(entry);
    }

    let mut connect = Vec::new();
    // nmtui first: it is a terminal program, so it inherits the
    // terminal's own theme instead of arriving as a GTK window that
    // looks like it came from another system. It also works in a TTY,
    // which matters on the machine whose session will not start.
    if let Some(wifi) = tools.first(&["nmtui", "nm-connection-editor"]) {
        let run = if wifi == "nmtui" { Run::Terminal } else { Run::Detached };
        connect.push(leaf("network", &[&wifi], run));
    }
    if let Some(entry) = tool(tools, "bluetooth", &["blueman-manager"], Run::Detached) {
        connect.push(entry);
    }
    if let Some(audio) = tools.first(&["wiremix", "pavucontrol"]) {
        let run = if audio == "wiremix" { Run::Terminal } else { Run::Detached };
        connect.push(leaf("audio", &[&audio], run));
    }
    if let Some(entry) = tool(tools, "displays", &["wdisplays"], Run::Detached) {
        connect.push(entry);
    }
    root.extend(submenu("connect", connect));

    // Declaration: opens and shows, never writes. `capture` is the one
    // entry that can end in a write, and it does its own asking.
    let declaration = vec![
        leaf("edit the declaration", &["kuma", "edit"], Run::Terminal),
        leaf("show drift", &["kuma", "diff"], Run::Terminal),
        leaf("review proposals", &["kuma", "capture"], Run::Terminal),
    ];
    root.extend(submenu("declaration", declaration));

    let system = vec![
        leaf("health", &["kuma", "doctor"], Run::Terminal),
        leaf("check for updates", &["kuma", "update", "--check"], Run::Terminal),
        leaf("rebuild", &["kuma", "build"], Run::Terminal),
        leaf("roll back", &["kuma", "rollback"], Run::Terminal),
        leaf("snapshots", &["kuma", "snapshot"], Run::Terminal),
    ];
    root.extend(submenu("system", system));

    let mut notifications = Vec::new();
    if let Some(entry) =
        tool(tools, "do not disturb", &["makoctl", "mode", "-t", "do-not-disturb"], Run::Detached)
    {
        notifications.push(entry);
    }
    if let Some(entry) = tool(tools, "dismiss all", &["makoctl", "dismiss", "-a"], Run::Detached) {
        notifications.push(entry);
    }
    root.extend(submenu("notifications", notifications));

    // Power. Stock niri binds a lock and a quit and nothing else, so
    // suspend, reboot and power off have no key and no menu on a kuma
    // desktop today. systemctl reaches them without sudo: logind grants
    // them to the session that owns the seat.
    let mut power = Vec::new();
    if let Some(entry) = tool(tools, "lock", &["swaylock"], Run::Detached) {
        power.push(entry);
    }
    power.push(leaf("suspend", &["systemctl", "suspend"], Run::Detached));
    if let Some(entry) = tool(tools, "log out", &["niri", "msg", "action", "quit"], Run::Detached) {
        power.push(entry);
    }
    power.push(leaf("reboot", &["systemctl", "reboot"], Run::Detached));
    power.push(leaf("power off", &["systemctl", "poweroff"], Run::Detached));
    root.extend(submenu("power", power));

    root
}

/// Ask fuzzel to pick one of `labels`. `Ok(None)` is a cancel, which is
/// a person saying "away" and not an error.
fn pick(labels: &[String], prompt: &str) -> Result<Option<String>> {
    let input = labels.join("\n");
    let chosen = host_output_stdin(&["fuzzel", "--dmenu", "--prompt", prompt], &input)
        .context("cannot run fuzzel")?;
    Ok(chosen.map(|line| line.trim().to_string()).filter(|line| !line.is_empty()))
}

/// Run the menu: pick, descend, dispatch, exit.
pub fn menu(config_path: &Path) -> Result<()> {
    let tools = Tools::observe();
    if !tools.has("fuzzel") {
        anyhow::bail!("kuma menu needs fuzzel, which this image does not have");
    }
    let mut stack = vec![tree(&tools)];
    let mut trail: Vec<String> = Vec::new();

    loop {
        let level = stack.last().expect("the stack is never emptied without returning");
        let labels: Vec<String> = level.iter().map(|entry| entry.label.clone()).collect();
        let prompt = if trail.is_empty() {
            "kuma ".to_string()
        } else {
            format!("kuma {} ", trail.join(" "))
        };
        let Some(chosen) = pick(&labels, &prompt)? else {
            return Ok(());
        };
        if chosen == BACK {
            stack.pop();
            trail.pop();
            if stack.is_empty() {
                return Ok(());
            }
            continue;
        }
        let Some(entry) = level.iter().find(|entry| entry.label == chosen) else {
            // fuzzel in dmenu mode returns whatever was typed when it
            // matches nothing. Treated as a cancel rather than an error:
            // the person asked for something this menu does not have.
            return Ok(());
        };
        match &entry.kind {
            Kind::Submenu(entries) => {
                trail.push(entry.label.clone());
                stack.push(entries.clone());
            }
            Kind::Leaf { argv, run } => return dispatch(&tools, config_path, argv, *run),
        }
    }
}

fn dispatch(tools: &Tools, config_path: &Path, argv: &[String], run: Run) -> Result<()> {
    let argv: Vec<String> =
        argv.iter().map(|arg| if arg == "kuma" { kuma_program() } else { arg.clone() }).collect();
    match run {
        Run::Detached => spawn_detached(&argv),
        Run::Terminal => {
            let terminal = tools.terminal().context("no terminal in this image to run that in")?;
            let mut full = vec![terminal, "-e".to_string()];
            // `kuma edit` is not a verb; the declaration opens in the
            // person's own editor, which is the whole of what "edit"
            // means here. Expanded at dispatch rather than in the tree
            // so the tree stays a list of commands and the editor stays
            // one lookup.
            if argv.len() == 2 && argv[1] == "edit" {
                full.push(tools.editor.clone());
                full.push(config_path.to_string_lossy().to_string());
            } else {
                full.extend(argv);
            }
            spawn_detached(&full)
        }
    }
}

/// The kuma to run, which is this one. A menu entry that says `kuma`
/// must not find a different kuma on PATH than the one drawing the menu:
/// the image bakes a copy and a developer's `~/.cargo/bin` shadows it,
/// and a menu that silently drives the other binary is the kind of thing
/// nobody catches for a release.
fn kuma_program() -> String {
    std::env::current_exe()
        .ok()
        .filter(|path| path.is_file())
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_else(|| "kuma".to_string())
}

/// Whether `path` is the declaration this menu would open. Used by the
/// boundary test, and by nothing else.
#[cfg(test)]
fn opens_declaration(argv: &[String]) -> bool {
    argv.len() == 2 && argv[0] == "kuma" && argv[1] == "edit"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaves(entries: &[Entry], out: &mut Vec<(String, Vec<String>, Run)>) {
        for entry in entries {
            match &entry.kind {
                Kind::Submenu(children) => leaves(children, out),
                Kind::Leaf { argv, run } => out.push((entry.label.clone(), argv.clone(), *run)),
            }
        }
    }

    fn all_leaves(tools: &Tools) -> Vec<(String, Vec<String>, Run)> {
        let mut out = Vec::new();
        leaves(&tree(tools), &mut out);
        out
    }

    fn everything() -> Tools {
        Tools::with(super::PROBED)
    }

    /// The invariant that makes the menu safe to grow: a person cannot
    /// change the declaration from a launcher. `capture` is allowed
    /// because it is a dry run that asks before it writes, and `edit`
    /// because it opens the file in an editor the person then saves
    /// themselves.
    #[test]
    fn no_leaf_writes_the_declaration() {
        const WRITERS: &[&str] = &["add", "remove", "init", "sync", "switch", "install"];
        for (label, argv, _) in all_leaves(&everything()) {
            if argv[0] != "kuma" {
                continue;
            }
            let verb = argv[1].as_str();
            assert!(
                !WRITERS.contains(&verb),
                "{label} runs `kuma {verb}`, which writes; the menu may not"
            );
            assert!(
                !argv.iter().any(|arg| arg == "--yes"),
                "{label} passes --yes; every write from this menu must be confirmed by the verb itself"
            );
            assert!(
                verb != "capture" || !argv.iter().any(|arg| arg == "--yes"),
                "{label} would capture without asking"
            );
        }
    }

    /// The declaration is reachable, or the group is a lie.
    #[test]
    fn the_declaration_can_be_opened_and_only_opened() {
        let opens: Vec<_> = all_leaves(&everything())
            .into_iter()
            .filter(|(_, argv, _)| opens_declaration(argv))
            .collect();
        assert_eq!(opens.len(), 1, "exactly one entry opens kuma.toml");
        assert_eq!(opens[0].2, Run::Terminal, "an editor needs a terminal");
    }

    /// Every `kuma` leaf names a verb the CLI actually has. The menu is
    /// a second enumeration of what kuma can do, sitting outside the
    /// code that does it, so it rots exactly the way a docs page rots.
    /// Same shape as the walkthrough's coverage test, same reason.
    #[test]
    fn every_kuma_leaf_names_a_real_verb() {
        use clap::CommandFactory;
        let cli = crate::Cli::command();
        let verbs: BTreeSet<String> =
            cli.get_subcommands().map(|sub| sub.get_name().to_string()).collect();
        for (label, argv, _) in all_leaves(&everything()) {
            if argv[0] != "kuma" || opens_declaration(&argv) {
                continue;
            }
            assert!(
                verbs.contains(&argv[1]),
                "{label} runs `kuma {}`, which is not a verb",
                argv[1]
            );
        }
    }

    /// A machine with nothing installed offers nothing that would fail.
    /// The only leaves that survive are the ones whose program is kuma
    /// itself (it is running, so it is here) and systemctl (pid 1's own
    /// client, present in every image kuma can build).
    #[test]
    fn a_leaf_appears_only_when_its_program_does() {
        for (label, argv, _) in all_leaves(&Tools::none()) {
            assert!(
                argv[0] == "kuma" || argv[0] == "systemctl",
                "{label} runs {}, which nothing checked for",
                argv[0]
            );
        }
    }

    /// Every probed program earns its place, and every program a leaf
    /// names was probed. Without the second half a leaf can quietly stop
    /// being gated and offer a row that does nothing; without the first,
    /// PROBED grows entries nothing reads.
    ///
    /// Two probed programs are named by no leaf on any machine, and both
    /// are checked rather than excused: a terminal is what a talking
    /// leaf gets wrapped in, and a fallback only appears on a machine
    /// that lacks the tool it stands in for.
    #[test]
    fn probed_and_named_are_the_same_set() {
        fn programs(tools: &Tools) -> BTreeSet<String> {
            all_leaves(tools)
                .into_iter()
                .map(|(_, argv, _)| argv[0].clone())
                .filter(|program| program != "kuma" && program != "systemctl")
                .collect()
        }
        // The machine where every fallback wins, so the tools that only
        // ever appear as second choices are named too.
        let fallbacks: Vec<&str> =
            PROBED.iter().copied().filter(|p| *p != "nmtui" && *p != "wiremix").collect();
        let mut named = programs(&everything());
        named.extend(programs(&Tools::with(&fallbacks)));

        const TERMINALS: &[&str] = &["kitty", "cosmic-term"];
        for terminal in TERMINALS {
            assert_eq!(
                Tools::with(&[terminal]).terminal().as_deref(),
                Some(*terminal),
                "{terminal} is probed but is not a terminal the menu would use"
            );
            named.insert((*terminal).to_string());
        }

        let probed: BTreeSet<String> = PROBED.iter().map(|p| (*p).to_string()).collect();
        assert_eq!(named, probed, "PROBED and the programs leaves name have drifted");
    }

    /// A selection comes back as a string and is matched by label, so
    /// two entries sharing one at the same level would make the second
    /// unreachable.
    #[test]
    fn labels_are_unique_within_a_level() {
        fn check(entries: &[Entry], path: &str) {
            let mut seen = BTreeSet::new();
            for entry in entries {
                assert!(seen.insert(entry.label.clone()), "duplicate `{}` in {path}", entry.label);
                if let Kind::Submenu(children) = &entry.kind {
                    check(children, &entry.label);
                }
            }
        }
        check(&tree(&everything()), "root");
    }

    /// Back exists everywhere it is needed and nowhere it is not: the
    /// root has no level above it, and a cancel from there means away.
    #[test]
    fn every_submenu_can_be_left_and_the_root_cannot() {
        let root = tree(&everything());
        assert!(!root.iter().any(|entry| entry.label == BACK), "the root offers no way up");
        for entry in &root {
            if let Kind::Submenu(children) = &entry.kind {
                assert!(
                    children.iter().any(|child| child.label == BACK),
                    "{} cannot be left",
                    entry.label
                );
            }
        }
    }

    /// A group with nothing in it is absent, not empty. The machine that
    /// proves it is one with no network, audio or display tool at all.
    #[test]
    fn a_group_whose_entries_are_all_missing_does_not_appear() {
        let bare = tree(&Tools::with(&["fuzzel", "kitty"]));
        assert!(!bare.iter().any(|entry| entry.label == "connect"));
        assert!(!bare.iter().any(|entry| entry.label == "notifications"));
        // system and declaration are kuma's own and always available.
        assert!(bare.iter().any(|entry| entry.label == "system"));
        assert!(bare.iter().any(|entry| entry.label == "declaration"));
    }

    /// The terminal tools win where both are installed. This is the
    /// whole aesthetic argument in one assertion: a terminal program
    /// inherits the terminal's theme, a GTK panel brings its own.
    #[test]
    fn a_terminal_tool_is_preferred_to_a_graphical_one() {
        let both =
            Tools::with(&["fuzzel", "nmtui", "nm-connection-editor", "wiremix", "pavucontrol"]);
        let named: Vec<String> =
            all_leaves(&both).into_iter().map(|(_, argv, _)| argv[0].clone()).collect();
        assert!(named.contains(&"nmtui".to_string()));
        assert!(!named.contains(&"nm-connection-editor".to_string()));
        assert!(named.contains(&"wiremix".to_string()));
        assert!(!named.contains(&"pavucontrol".to_string()));
    }

    /// And the graphical one is offered when it is all there is, rather
    /// than the row vanishing.
    #[test]
    fn a_graphical_tool_is_offered_when_it_is_the_only_one() {
        let gtk = Tools::with(&["fuzzel", "nm-connection-editor", "pavucontrol"]);
        let named: Vec<String> =
            all_leaves(&gtk).into_iter().map(|(_, argv, _)| argv[0].clone()).collect();
        assert!(named.contains(&"nm-connection-editor".to_string()));
        assert!(named.contains(&"pavucontrol".to_string()));
    }

    /// Opening the menu must never prompt for a password, so nothing
    /// that builds it may run sudo.
    #[test]
    fn nothing_the_menu_runs_to_draw_itself_needs_root() {
        for (label, argv, _) in all_leaves(&everything()) {
            assert_ne!(argv[0], "sudo", "{label} would prompt from inside a launcher");
        }
    }

    /// Anything that prints or prompts gets a terminal; anything that
    /// does neither must not steal one, or locking the screen flashes a
    /// window.
    #[test]
    fn only_leaves_with_something_to_say_open_a_terminal() {
        for (label, argv, run) in all_leaves(&everything()) {
            let talks = argv[0] == "kuma" || argv[0] == "nmtui" || argv[0] == "wiremix";
            assert_eq!(
                run == Run::Terminal,
                talks,
                "{label} is run the wrong way for what it prints"
            );
        }
    }
}
