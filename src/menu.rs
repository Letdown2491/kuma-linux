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
    /// Freedesktop icon name, with a plainer fallback after the comma
    /// for themes that carry only the legacy name. Every item has one:
    /// fuzzel leaves a hole where an icon is missing, and one hole makes
    /// the whole list look broken.
    pub(crate) icon: &'static str,
    pub(crate) argv: Vec<String>,
    pub(crate) run: Run,
}

impl Item {
    /// The line fuzzel is handed. `\0icon\x1f<name>` is fuzzel's dmenu
    /// icon protocol; everything before the NUL is what is displayed and
    /// what the search matches against.
    fn line(&self) -> String {
        format!("{} · {}\u{0}icon\u{1f}{}", self.group, self.label, self.icon)
    }

    /// What the person reads and types against, without the protocol.
    #[cfg(test)]
    fn text(&self) -> String {
        format!("{} · {}", self.group, self.label)
    }
}

fn item(
    group: &'static str,
    label: &'static str,
    icon: &'static str,
    argv: &[&str],
    run: Run,
) -> Item {
    Item { group, label, icon, argv: argv.iter().map(|a| (*a).to_string()).collect(), run }
}

/// An item whose program this machine has, or nothing.
fn tool(
    tools: &Tools,
    group: &'static str,
    label: &'static str,
    icon: &'static str,
    argv: &[&str],
    run: Run,
) -> Option<Item> {
    let program = argv.first().copied().unwrap_or_default();
    tools.has(program).then(|| item(group, label, icon, argv, run))
}

/// The whole menu, as a pure function of what is installed.
pub(crate) fn items(tools: &Tools) -> Vec<Item> {
    let mut out = Vec::new();

    if tools.has("fuzzel") {
        out.push(item(
            "apps",
            "launch an application",
            "applications-system-symbolic,applications-system",
            &["fuzzel"],
            Run::Detached,
        ));
    }

    // nmtui before the graphical editor: a terminal program inherits the
    // terminal's theme instead of arriving as a window from another
    // system, and it works in a TTY, which is the only place left when a
    // session will not start.
    if let Some(wifi) = tools.first(&["nmtui", "nm-connection-editor"]) {
        let run = if wifi == "nmtui" { Run::Terminal } else { Run::Detached };
        out.push(item(
            "connect",
            "network",
            "network-wireless-symbolic,network-wireless",
            &[&wifi],
            run,
        ));
    }
    out.extend(tool(
        tools,
        "connect",
        "bluetooth",
        "bluetooth-symbolic,bluetooth",
        &["blueman-manager"],
        Run::Detached,
    ));
    if let Some(audio) = tools.first(&["wiremix", "pavucontrol"]) {
        let run = if audio == "wiremix" { Run::Terminal } else { Run::Detached };
        out.push(item(
            "connect",
            "audio",
            "audio-volume-high-symbolic,audio-volume-high",
            &[&audio],
            run,
        ));
    }
    out.extend(tool(
        tools,
        "connect",
        "displays",
        "video-display-symbolic,video-display",
        &["wdisplays"],
        Run::Detached,
    ));

    // Declaration: opens and shows, never writes. `capture` is the one
    // entry that can end in a write, and it does its own asking.
    out.push(item(
        "declaration",
        "edit",
        "text-editor-symbolic,text-editor",
        &["kuma", "edit"],
        Run::Terminal,
    ));
    out.push(item(
        "declaration",
        "show drift",
        "edit-find-symbolic,edit-find",
        &["kuma", "diff"],
        Run::Terminal,
    ));
    out.push(item(
        "declaration",
        "review proposals",
        "dialog-information-symbolic,dialog-information",
        &["kuma", "capture"],
        Run::Terminal,
    ));

    out.push(item(
        "system",
        "health",
        "emblem-system-symbolic,emblem-system",
        &["kuma", "doctor"],
        Run::Terminal,
    ));
    out.push(item(
        "system",
        "check for updates",
        "software-update-available-symbolic,software-update-available",
        &["kuma", "update", "--check"],
        Run::Terminal,
    ));
    out.push(item(
        "system",
        "rebuild",
        "view-refresh-symbolic,view-refresh",
        &["kuma", "build"],
        Run::Terminal,
    ));
    out.push(item(
        "system",
        "roll back",
        "go-previous-symbolic,go-previous",
        &["kuma", "rollback"],
        Run::Terminal,
    ));
    out.push(item(
        "system",
        "snapshots",
        "drive-harddisk-symbolic,drive-harddisk",
        &["kuma", "snapshot"],
        Run::Terminal,
    ));

    out.extend(tool(
        tools,
        "notifications",
        "do not disturb",
        "media-playback-pause-symbolic,media-playback-pause",
        &["makoctl", "mode", "-t", "do-not-disturb"],
        Run::Detached,
    ));
    out.extend(tool(
        tools,
        "notifications",
        "dismiss all",
        "user-trash-symbolic,user-trash",
        &["makoctl", "dismiss", "-a"],
        Run::Detached,
    ));

    // Power. Stock niri binds a lock and a quit and nothing else, so
    // suspend, reboot and power off have no key and no menu on a kuma
    // desktop today. systemctl reaches them without sudo: logind grants
    // them to the session that owns the seat.
    out.extend(tool(
        tools,
        "power",
        "lock",
        "system-lock-screen-symbolic,system-lock-screen",
        &["swaylock"],
        Run::Detached,
    ));
    // Adwaita has no system-suspend icon, symbolic or otherwise; the
    // night one is what every panel uses for the same idea.
    out.push(item(
        "power",
        "suspend",
        "weather-clear-night-symbolic,weather-clear-night",
        &["systemctl", "suspend"],
        Run::Detached,
    ));
    out.extend(tool(
        tools,
        "power",
        "log out",
        "system-log-out-symbolic,system-log-out",
        &["niri", "msg", "action", "quit"],
        Run::Detached,
    ));
    out.push(item(
        "power",
        "reboot",
        "system-reboot-symbolic,system-reboot",
        &["systemctl", "reboot"],
        Run::Detached,
    ));
    out.push(item(
        "power",
        "power off",
        "system-shutdown-symbolic,system-shutdown",
        &["systemctl", "poweroff"],
        Run::Detached,
    ));

    out
}

/// Ask fuzzel to pick one. `Ok(None)` is a cancel, which is a person
/// saying "away" and not an error.
///
/// `--index` rather than the chosen text: fuzzel in dmenu mode echoes
/// whatever was typed when it matches nothing, so matching the answer
/// back against labels would make a typo indistinguishable from a
/// choice, and would quietly require every line to be unique. An index
/// is unambiguous or it is out of range.
fn pick(items: &[Item]) -> Result<Option<usize>> {
    let input: Vec<String> = items.iter().map(Item::line).collect();
    let chosen = host_output_stdin(
        &["fuzzel", "--dmenu", "--index", "--prompt", "kuma  ", "--counter"],
        &input.join("\n"),
    )
    .context("cannot run fuzzel")?;
    Ok(chosen_index(chosen.as_deref(), items.len()))
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

/// Run the menu: pick, dispatch, exit.
pub fn menu(config_path: &Path) -> Result<()> {
    let tools = Tools::observe();
    if !tools.has("fuzzel") {
        anyhow::bail!("kuma menu needs fuzzel, which this image does not have");
    }
    let items = items(&tools);
    let Some(index) = pick(&items)? else {
        return Ok(());
    };
    let chosen = &items[index];
    dispatch(&tools, config_path, &chosen.argv, chosen.run)
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

    /// The search argument for flattening, as an assertion: typing a
    /// leaf's own word finds it without naming its group, and typing the
    /// group finds all of them. Both fail on a menu of submenus, because
    /// a launcher can only match the lines it was handed.
    #[test]
    fn a_row_is_found_by_its_own_word_and_by_its_group() {
        let rows: Vec<String> = items(&everything()).iter().map(Item::text).collect();
        let matching = |needle: &str| rows.iter().filter(|row| row.contains(needle)).count();
        assert_eq!(matching("reboot"), 1, "typing `reboot` should find exactly the reboot");
        assert_eq!(matching("power"), 5, "typing `power` should find the whole power group");
        assert!(matching("drift") == 1, "typing `drift` should find the drift row");
    }

    /// Every row carries an icon. fuzzel leaves a hole where one is
    /// missing, and a single hole makes the whole list look broken.
    #[test]
    fn every_row_has_an_icon_with_a_fallback() {
        for entry in items(&everything()) {
            assert!(!entry.icon.is_empty(), "{} has no icon", entry.label);
            assert!(
                entry.icon.contains(','),
                "{} names one icon with no fallback for a theme that lacks it",
                entry.label
            );
        }
    }

    /// The line handed to fuzzel is the displayed text, a NUL, and the
    /// icon protocol. Asserted on the bytes because a launcher that does
    /// not understand them shows the protocol to the person instead.
    #[test]
    fn a_row_is_encoded_the_way_fuzzel_reads_icons() {
        let row = item(
            "power",
            "reboot",
            "system-reboot-symbolic,system-reboot",
            &["true"],
            Run::Detached,
        );
        assert_eq!(row.line(), "power · reboot\u{0}icon\u{1f}system-reboot-symbolic,system-reboot");
        assert_eq!(row.line().split('\u{0}').next(), Some("power · reboot"));
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
