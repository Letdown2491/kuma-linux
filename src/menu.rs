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
//! launcher is genuinely better at. `no_item_writes_the_declaration`
//! enforces it.
//!
//! **The menu is flat, and that is a decision rather than a shortcut.**
//! A launcher can only match against the lines it was handed, so a tree
//! of submenus searches terribly: typing `reboot` at the top of one
//! matches nothing, and the person who knew exactly what they wanted
//! navigates anyway. Flattening costs a word of prefix per row and makes
//! every entry reachable by typing any part of its name or its group.
//!
//! **A row appears only when its program is here.** The list is a pure
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

/// How an item is run.
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

/// One row.
///
/// **The menu is flat, and that is the whole design.** A tree of
/// submenus reads well and searches terribly: a launcher can only match
/// against the lines it was handed, so typing `reboot` at the top of a
/// nested menu matches nothing, and the person who knew exactly what
/// they wanted has to navigate to it anyway. Flattening costs one word
/// of prefix per row and makes every entry in the menu reachable by
/// typing any part of its name or its group.
#[derive(Debug, Clone)]
pub(crate) struct Item {
    /// The section this belongs to. Rendered as part of the line, so it
    /// is searchable: `connect` narrows to the network entries the same
    /// way `wifi` would.
    pub(crate) group: &'static str,
    pub(crate) label: &'static str,
    /// A Font Awesome glyph, as a character rather than an icon name.
    ///
    /// **Not fuzzel's dmenu icon protocol, and that was measured.**
    /// Every one of Adwaita's 587 symbolic SVGs hardcodes
    /// `fill="#2e3436"`, fuzzel renders the file as it is, and kuma's
    /// launcher background is `#0e1626`: the icons drew, in near-black
    /// on near-black, and no choice of icon name could have fixed it
    /// because the whole theme is that colour. A glyph is text, so it
    /// takes the row's own foreground colour and cannot go invisible.
    /// It is also what waybar already uses, so the menu and the bar
    /// speak the same alphabet.
    ///
    /// Referenced by codepoint, never by font name: face names carry the
    /// major version (`fontawesome-6-*` becomes `fontawesome-7-*` in
    /// Fedora 45) while codepoints do not, and fontconfig finds whoever
    /// provides the glyph. Same reasoning as the metapackage in the
    /// desktop set.
    pub(crate) glyph: char,
    pub(crate) argv: Vec<String>,
    pub(crate) run: Run,
}

impl Item {
    /// The line fuzzel is handed, which is also what it searches.
    fn line(&self) -> String {
        format!("{}  {} · {}", self.glyph, self.group, self.label)
    }

    /// What the row says, without its glyph: the part a person types
    /// against and the part the tests read.
    #[cfg(test)]
    fn text(&self) -> String {
        format!("{} · {}", self.group, self.label)
    }
}

fn item(group: &'static str, label: &'static str, glyph: char, argv: &[&str], run: Run) -> Item {
    Item { group, label, glyph, argv: argv.iter().map(|a| (*a).to_string()).collect(), run }
}

/// An item whose program this machine has, or nothing.
fn tool(
    tools: &Tools,
    group: &'static str,
    label: &'static str,
    glyph: char,
    argv: &[&str],
    run: Run,
) -> Option<Item> {
    let program = argv.first().copied().unwrap_or_default();
    tools.has(program).then(|| item(group, label, glyph, argv, run))
}

/// The whole menu, as a pure function of what is installed.
pub(crate) fn items(tools: &Tools) -> Vec<Item> {
    let mut out = Vec::new();

    if tools.has("fuzzel") {
        out.push(item("Apps", "Launch an application", '\u{f009}', &["fuzzel"], Run::Detached));
    }

    // nmtui before the graphical editor: a terminal program inherits the
    // terminal's theme instead of arriving as a window from another
    // system, and it works in a TTY, which is the only place left when a
    // session will not start.
    if let Some(wifi) = tools.first(&["nmtui", "nm-connection-editor"]) {
        let run = if wifi == "nmtui" { Run::Terminal } else { Run::Detached };
        out.push(item("Connect", "Network", '\u{f1eb}', &[&wifi], run));
    }
    out.extend(tool(
        tools,
        "Connect",
        "Bluetooth",
        '\u{f293}',
        &["blueman-manager"],
        Run::Detached,
    ));
    if let Some(audio) = tools.first(&["wiremix", "pavucontrol"]) {
        let run = if audio == "wiremix" { Run::Terminal } else { Run::Detached };
        out.push(item("Connect", "Audio", '\u{f028}', &[&audio], run));
    }
    out.extend(tool(tools, "Connect", "Displays", '\u{f108}', &["wdisplays"], Run::Detached));

    // Declaration: opens and shows, never writes. `capture` is the one
    // entry that can end in a write, and it does its own asking.
    out.push(item("Declaration", "Edit", '\u{f044}', &["kuma", "edit"], Run::Terminal));
    out.push(item("Declaration", "Show drift", '\u{f002}', &["kuma", "diff"], Run::Terminal));
    out.push(item(
        "Declaration",
        "Review proposals",
        '\u{f05a}',
        &["kuma", "capture"],
        Run::Terminal,
    ));

    out.push(item("System", "Health", '\u{f21e}', &["kuma", "doctor"], Run::Terminal));
    out.push(item(
        "System",
        "Check for updates",
        '\u{f021}',
        &["kuma", "update", "--check"],
        Run::Terminal,
    ));
    out.push(item("System", "Rebuild", '\u{f0ad}', &["kuma", "build"], Run::Terminal));
    out.push(item("System", "Roll back", '\u{f0e2}', &["kuma", "rollback"], Run::Terminal));
    out.push(item("System", "Snapshots", '\u{f0c7}', &["kuma", "snapshot"], Run::Terminal));

    out.extend(tool(
        tools,
        "Notifications",
        "Do not disturb",
        '\u{f1f6}',
        &["makoctl", "mode", "-t", "do-not-disturb"],
        Run::Detached,
    ));
    out.extend(tool(
        tools,
        "Notifications",
        "Dismiss all",
        '\u{f2ed}',
        &["makoctl", "dismiss", "-a"],
        Run::Detached,
    ));

    // Power. Stock niri binds a lock and a quit and nothing else, so
    // suspend, reboot and power off have no key and no menu on a kuma
    // desktop today. systemctl reaches them without sudo: logind grants
    // them to the session that owns the seat.
    out.extend(tool(tools, "Power", "Lock", '\u{f023}', &["swaylock"], Run::Detached));
    out.push(item("Power", "Suspend", '\u{f186}', &["systemctl", "suspend"], Run::Detached));
    out.extend(tool(
        tools,
        "Power",
        "Log out",
        '\u{f2f5}',
        &["niri", "msg", "action", "quit"],
        Run::Detached,
    ));
    out.push(item("Power", "Reboot", '\u{f01e}', &["systemctl", "reboot"], Run::Detached));
    out.push(item("Power", "Power off", '\u{f011}', &["systemctl", "poweroff"], Run::Detached));

    out
}

/// A row of whichever level is on screen.
///
/// `Item` carries an index into the one list of items rather than a
/// reference, so a level is a cheap plan that any test can build and
/// compare without cloning the menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Row {
    /// Up to the groups. Only ever below the top level, where there is
    /// nothing above to go to and a cancel means away.
    Back,
    Group(&'static str),
    Item(usize),
}

fn group_glyph(group: &str) -> Option<char> {
    Some(match group {
        "Apps" => '\u{f009}',
        "Connect" => '\u{f1eb}',
        "Declaration" => '\u{f044}',
        "System" => '\u{f013}',
        "Notifications" => '\u{f0f3}',
        "Power" => '\u{f011}',
        _ => return None,
    })
}

/// The groups present in `items`, in the order they first appear. Not
/// sorted: the authored order is the browse order.
fn groups(items: &[Item]) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for entry in items {
        if !out.contains(&entry.group) {
            out.push(entry.group);
        }
    }
    out
}

/// What a group's row reads. The chevron says it descends, and keeps a
/// group's row from reading identically to an item's.
fn group_line(group: &str) -> String {
    let glyph = group_glyph(group).unwrap_or('\u{f013}');
    format!("{glyph}  {group}   ›")
}

fn row_line(row: Row, items: &[Item]) -> String {
    match row {
        Row::Back => format!("{}  Back", '\u{f053}'),
        Row::Group(group) => group_line(group),
        Row::Item(index) => items[index].line(),
    }
}

/// The top level: the groups, then every item, with the window sized to
/// the groups.
///
/// The pair is one function because they are one decision. Handing
/// fuzzel a list and separately telling it a height is how the height
/// ends up being `items.len()` with nothing to notice: sabotage flipped
/// exactly that at the call site and every test still passed, because
/// the argument lived inside the shell-out where no test could see it.
fn top_level(items: &[Item], groups: &[&'static str]) -> (Vec<Row>, usize) {
    let mut rows: Vec<Row> = groups.iter().map(|group| Row::Group(group)).collect();
    let visible = rows.len();
    rows.extend((0..items.len()).map(Row::Item));
    (rows, visible)
}

/// Inside a group: its own rows, a way back, and then every other row
/// there is.
///
/// **The tail is the point.** Descending must not narrow what can be
/// found, only what is shown: a person who opens `Connect` and then
/// remembers they wanted to reboot should type `reboot` and get it,
/// rather than discovering that the menu quietly became a smaller menu.
/// Same trick as the top level, one level down: the window shows the
/// group, the list holds everything.
fn group_level(items: &[Item], group: &str) -> (Vec<Row>, usize) {
    let mine = |index: &usize| items[*index].group == group;
    let mut rows: Vec<Row> = (0..items.len()).filter(mine).map(Row::Item).collect();
    rows.push(Row::Back);
    let visible = rows.len();
    rows.extend((0..items.len()).filter(|index| !mine(index)).map(Row::Item));
    (rows, visible)
}

/// Ask fuzzel to pick one of `lines`, showing `visible` of them at rest.
/// `Ok(None)` is a cancel, which is a person saying "away" and not an
/// error.
///
/// `--index` rather than the chosen text: fuzzel in dmenu mode echoes
/// whatever was typed when it matches nothing, so matching the answer
/// back against labels would make a typo indistinguishable from a
/// choice, and would quietly require every line to be unique. An index
/// is unambiguous or it is out of range.
fn pick(lines: &[String], visible: usize) -> Result<Option<usize>> {
    let chosen = host_output_stdin(
        &[
            "fuzzel",
            "--dmenu",
            "--index",
            "--prompt",
            "kuma  ",
            "--counter",
            "--lines",
            &visible.to_string(),
        ],
        &lines.join("\n"),
    )
    .context("cannot run fuzzel")?;
    Ok(chosen_index(chosen.as_deref(), lines.len()))
}

/// What fuzzel's answer means, as a pure function so it can be tested
/// without a launcher.
///
/// Three ways to get nothing: cancelled (`None`), an index that is not a
/// number (fuzzel prints `-1` when the input was accepted but matched no
/// row), and an index past the end. The last cannot happen today and is
/// checked anyway, because the caller indexes a slice with the result
/// and the difference between a wrong answer and a panic is this line.
fn chosen_index(answer: Option<&str>, count: usize) -> Option<usize> {
    answer?.trim().parse::<usize>().ok().filter(|index| *index < count)
}

/// Run the menu: pick, descend or come back, dispatch, exit.
pub fn menu(config_path: &Path) -> Result<()> {
    let tools = Tools::observe();
    if !tools.has("fuzzel") {
        anyhow::bail!("kuma menu needs fuzzel, which this image does not have");
    }
    let items = items(&tools);
    let groups = groups(&items);
    let mut inside: Option<&'static str> = None;

    loop {
        let (rows, visible) = match inside {
            None => top_level(&items, &groups),
            Some(group) => group_level(&items, group),
        };
        let lines: Vec<String> = rows.iter().map(|row| row_line(*row, &items)).collect();
        let Some(index) = pick(&lines, visible)? else {
            return Ok(());
        };
        match rows[index] {
            Row::Back => inside = None,
            Row::Group(group) => inside = Some(group),
            Row::Item(item) => {
                let chosen = &items[item];
                return dispatch(&tools, config_path, &chosen.argv, chosen.run);
            }
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
            // means here. Expanded at dispatch rather than in the list
            // so the list stays a list of commands and the editor stays
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

/// Whether this is the entry that opens the declaration. Used by the
/// boundary test, and by nothing else.
#[cfg(test)]
fn opens_declaration(argv: &[String]) -> bool {
    argv.len() == 2 && argv[0] == "kuma" && argv[1] == "edit"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn everything() -> Tools {
        Tools::with(super::PROBED)
    }

    /// The invariant that makes the menu safe to grow: a person cannot
    /// change the declaration from a launcher. `capture` is allowed
    /// because it is a dry run that asks before it writes, and `edit`
    /// because it opens the file in an editor the person then saves
    /// themselves.
    #[test]
    fn no_item_writes_the_declaration() {
        const WRITERS: &[&str] = &["add", "remove", "init", "sync", "switch", "install"];
        for entry in items(&everything()) {
            if entry.argv[0] != "kuma" {
                continue;
            }
            let verb = entry.argv[1].as_str();
            assert!(
                !WRITERS.contains(&verb),
                "{} runs `kuma {verb}`, which writes; the menu may not",
                entry.label
            );
            assert!(
                !entry.argv.iter().any(|arg| arg == "--yes"),
                "{} passes --yes; every write from this menu must be confirmed by the verb itself",
                entry.label
            );
        }
    }

    /// The declaration is reachable, and reachable exactly one way.
    #[test]
    fn the_declaration_can_be_opened_and_only_opened() {
        let opens: Vec<Item> =
            items(&everything()).into_iter().filter(|i| opens_declaration(&i.argv)).collect();
        assert_eq!(opens.len(), 1, "exactly one entry opens kuma.toml");
        assert_eq!(opens[0].run, Run::Terminal, "an editor needs a terminal");
    }

    /// Every `kuma` item names a verb the CLI actually has. The menu is
    /// a second enumeration of what kuma can do, sitting outside the
    /// code that does it, so it rots exactly the way a docs page rots.
    /// Same shape as the walkthrough's coverage test, same reason.
    #[test]
    fn every_kuma_item_names_a_real_verb() {
        use clap::CommandFactory;
        let cli = crate::Cli::command();
        let verbs: BTreeSet<String> =
            cli.get_subcommands().map(|sub| sub.get_name().to_string()).collect();
        for entry in items(&everything()) {
            if entry.argv[0] != "kuma" || opens_declaration(&entry.argv) {
                continue;
            }
            assert!(
                verbs.contains(&entry.argv[1]),
                "{} runs `kuma {}`, which is not a verb",
                entry.label,
                entry.argv[1]
            );
        }
    }

    /// A machine with nothing installed offers nothing that would fail.
    /// The only items that survive name kuma itself (it is running, so
    /// it is here) and systemctl (pid 1's own client, in every image
    /// kuma can build).
    #[test]
    fn an_item_appears_only_when_its_program_does() {
        for entry in items(&Tools::none()) {
            assert!(
                entry.argv[0] == "kuma" || entry.argv[0] == "systemctl",
                "{} runs {}, which nothing checked for",
                entry.label,
                entry.argv[0]
            );
        }
    }

    /// Every probed program earns its place, and every program an item
    /// names was probed. Without the second half an item can quietly
    /// stop being gated and offer a row that does nothing; without the
    /// first, PROBED grows entries nothing reads.
    ///
    /// Two probed programs are named by no item on any machine, and both
    /// are checked rather than excused: a terminal is what a talking
    /// item gets wrapped in, and a fallback only appears on a machine
    /// that lacks the tool it stands in for.
    #[test]
    fn probed_and_named_are_the_same_set() {
        fn programs(tools: &Tools) -> BTreeSet<String> {
            items(tools)
                .into_iter()
                .map(|entry| entry.argv[0].clone())
                .filter(|program| program != "kuma" && program != "systemctl")
                .collect()
        }
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
        assert_eq!(named, probed, "PROBED and the programs items name have drifted");
    }

    /// Flat means every entry is typed for directly, so no two may read
    /// the same. Selection is by index and would not break, but a person
    /// looking at two identical rows has no way to tell them apart.
    #[test]
    fn no_two_rows_read_the_same() {
        let mut seen = BTreeSet::new();
        for entry in items(&everything()) {
            assert!(seen.insert(entry.text()), "two rows read `{}`", entry.text());
        }
    }

    /// A group's rows are contiguous, so the flat list still reads as
    /// sections when nothing has been typed. Sorting is not applied
    /// anywhere: the authored order is the browse order.
    #[test]
    fn rows_of_a_group_stay_together() {
        let mut seen: Vec<&str> = Vec::new();
        for entry in items(&everything()) {
            if seen.last() != Some(&entry.group) {
                assert!(
                    !seen.contains(&entry.group),
                    "{} is split into more than one run of rows",
                    entry.group
                );
                seen.push(entry.group);
            }
        }
        assert!(seen.len() > 1, "a menu of one group is not a menu");
    }

    /// The search argument, as an assertion: a row is found by its own
    /// word and by its group's. Both fail on a menu of submenus, because
    /// a launcher can only match the lines it was handed.
    ///
    /// Matched case-insensitively because that is how fuzzel matches,
    /// and because the rows are written for a person to read rather than
    /// for a person to type exactly.
    #[test]
    fn a_row_is_found_by_its_own_word_and_by_its_group() {
        let rows: Vec<String> =
            items(&everything()).iter().map(|entry| entry.text().to_lowercase()).collect();
        let matching = |needle: &str| rows.iter().filter(|row| row.contains(needle)).count();
        assert_eq!(matching("reboot"), 1, "typing `reboot` should find exactly the reboot");
        assert_eq!(matching("power"), 5, "typing `power` should find the whole power group");
        assert_eq!(matching("drift"), 1, "typing `drift` should find the drift row");
    }

    /// Every row carries a glyph, and every glyph is a real one. The
    /// Private Use Area check is what catches an ASCII placeholder
    /// standing in for a symbol nobody looked up.
    #[test]
    fn every_row_has_a_glyph_from_the_icon_font() {
        for entry in items(&everything()) {
            assert!(
                ('\u{e000}'..='\u{f8ff}').contains(&entry.glyph),
                "{} has {:?}, which is not an icon-font glyph",
                entry.label,
                entry.glyph
            );
        }
    }

    /// A group with no glyph falls back to a gear, which is how a new
    /// group ships looking like an afterthought. Asserted so that adding
    /// one to the list means adding it here too.
    #[test]
    fn every_group_has_its_own_glyph() {
        for group in groups(&items(&everything())) {
            assert!(group_glyph(group).is_some(), "{group} has no glyph of its own");
        }
    }

    /// Opening the menu shows the groups and nothing else: they come
    /// first and the window is sized to exactly their number. Typing
    /// still reaches every row, because they are all in the same list.
    #[test]
    fn the_menu_opens_on_its_groups_and_hides_nothing() {
        let items = items(&everything());
        let groups = groups(&items);
        let (rows, visible) = top_level(&items, &groups);
        assert_eq!(rows.len(), groups.len() + items.len(), "every row is in the list fuzzel sees");
        assert_eq!(visible, groups.len(), "the window at rest is exactly the groups");
        assert!(visible < rows.len(), "the rows below the fold are what typing reaches");
        for (index, group) in groups.iter().enumerate() {
            assert_eq!(rows[index], Row::Group(group), "the first rows are the groups, in order");
        }
        assert_eq!(rows[groups.len()], Row::Item(0), "then the items, in order");
    }

    /// There is nothing above the top level, so a cancel means away and
    /// a back row would be a lie.
    #[test]
    fn the_top_level_offers_no_way_back() {
        let items = items(&everything());
        let (rows, _) = top_level(&items, &groups(&items));
        assert!(!rows.contains(&Row::Back));
    }

    /// Descending shows a group and a way out of it.
    #[test]
    fn a_group_shows_its_own_rows_and_a_way_back() {
        let items = items(&everything());
        for group in groups(&items) {
            let (rows, visible) = group_level(&items, group);
            let mine = items.iter().filter(|entry| entry.group == group).count();
            assert_eq!(visible, mine + 1, "{group} shows its rows and the way back");
            for row in rows.iter().take(mine) {
                let Row::Item(index) = row else { panic!("{group} opens on something else") };
                assert_eq!(items[*index].group, group);
            }
            assert_eq!(rows[mine], Row::Back, "the way back is the last row in view");
        }
    }

    /// **Descending narrows what is shown, never what can be found.** A
    /// person who opens Connect and then remembers they wanted to reboot
    /// types `reboot` and gets it; without the tail the menu would have
    /// quietly become a smaller menu.
    #[test]
    fn every_row_is_still_reachable_from_inside_a_group() {
        let items = items(&everything());
        for group in groups(&items) {
            let (rows, visible) = group_level(&items, group);
            let reachable: BTreeSet<usize> = rows
                .iter()
                .filter_map(|row| match row {
                    Row::Item(index) => Some(*index),
                    _ => None,
                })
                .collect();
            assert_eq!(reachable.len(), items.len(), "{group} cannot reach every row");
            assert!(visible < rows.len(), "{group} shows everything it holds");
        }
    }

    /// Rows read like something a person wrote, not like an identifier.
    #[test]
    fn every_row_reads_as_written_english() {
        let items = items(&everything());
        for group in groups(&items) {
            assert!(
                group.starts_with(|c: char| c.is_ascii_uppercase()),
                "the group `{group}` is not capitalised"
            );
        }
        for entry in &items {
            assert!(
                entry.label.starts_with(|c: char| c.is_ascii_uppercase()),
                "the row `{}` is not capitalised",
                entry.label
            );
        }
    }

    /// A group's row is distinguishable from an item's. They sit in one
    /// list and both start with a glyph, so without the chevron the row
    /// that descends looks exactly like a row that acts.
    #[test]
    fn a_group_row_says_it_descends_and_an_item_row_does_not() {
        let items = items(&everything());
        for group in groups(&items) {
            assert!(group_line(group).ends_with('›'), "{group}'s row does not say it descends");
        }
        for entry in &items {
            assert!(!entry.line().contains('›'), "{} reads like a group", entry.label);
        }
    }

    /// The line handed to fuzzel is the glyph and the row's own words,
    /// so what is displayed is exactly what the search matches.
    #[test]
    fn a_row_reads_as_its_glyph_then_its_words() {
        let row = item("Power", "Reboot", '\u{f01e}', &["true"], Run::Detached);
        assert_eq!(row.line(), "\u{f01e}  Power · Reboot");
        assert!(row.line().ends_with(&row.text()));
    }

    /// Every way fuzzel can answer, including the two that must not
    /// reach a slice index.
    #[test]
    fn an_answer_is_a_row_or_it_is_nothing() {
        assert_eq!(chosen_index(Some("0"), 20), Some(0));
        assert_eq!(chosen_index(Some("19\n"), 20), Some(19));
        assert_eq!(chosen_index(None, 20), None, "a cancel is not a choice");
        assert_eq!(chosen_index(Some("-1"), 20), None, "fuzzel's no-match answer is not a choice");
        assert_eq!(chosen_index(Some(""), 20), None);
        assert_eq!(chosen_index(Some("power · reboot"), 20), None, "text is not an index");
        assert_eq!(chosen_index(Some("20"), 20), None, "one past the end is not a row");
        assert_eq!(chosen_index(Some("0"), 0), None, "an empty menu has no rows to choose");
    }

    /// The terminal tools win where both are installed. This is the
    /// whole aesthetic argument in one assertion: a terminal program
    /// inherits the terminal's theme, a GTK panel brings its own.
    #[test]
    fn a_terminal_tool_is_preferred_to_a_graphical_one() {
        let both =
            Tools::with(&["fuzzel", "nmtui", "nm-connection-editor", "wiremix", "pavucontrol"]);
        let named: Vec<String> =
            items(&both).into_iter().map(|entry| entry.argv[0].clone()).collect();
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
            items(&gtk).into_iter().map(|entry| entry.argv[0].clone()).collect();
        assert!(named.contains(&"nm-connection-editor".to_string()));
        assert!(named.contains(&"pavucontrol".to_string()));
    }

    /// Opening the menu must never prompt for a password, so nothing it
    /// runs may be sudo.
    #[test]
    fn nothing_the_menu_runs_to_draw_itself_needs_root() {
        for entry in items(&everything()) {
            assert_ne!(
                entry.argv[0], "sudo",
                "{} would prompt from inside a launcher",
                entry.label
            );
        }
    }

    /// Anything that prints or prompts gets a terminal; anything that
    /// does neither must not steal one, or locking the screen flashes a
    /// window.
    #[test]
    fn only_rows_with_something_to_say_open_a_terminal() {
        for entry in items(&everything()) {
            let talks =
                entry.argv[0] == "kuma" || entry.argv[0] == "nmtui" || entry.argv[0] == "wiremix";
            assert_eq!(
                entry.run == Run::Terminal,
                talks,
                "{} is run the wrong way for what it prints",
                entry.label
            );
        }
    }
}
