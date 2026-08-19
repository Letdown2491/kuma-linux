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

/// The groups, named once.
///
/// Each of these is written twice otherwise: once where the rows are
/// authored and once where the group's own icon is chosen. A typo in
/// either copy is a group that draws the fallback gear, which
/// `every_group_has_its_own_icon` catches, but a name the compiler
/// checks is better than a name a test catches.
pub(crate) const APPS: &str = "Apps";
const CONNECT: &str = "Connect";
const DECLARATION: &str = "Declaration";
const SYSTEM: &str = "System";
const NOTIFICATIONS: &str = "Notifications";
const POWER: &str = "Power";

/// The icon theme the build generates and the menu asks for.
pub(crate) const ICON_THEME: &str = "kuma";

/// The colour every generated icon is painted in.
///
/// Must be `assets/fuzzel.ini`'s `text=`, because the icons sit in the
/// same rows as that text and any drift makes them look like a different
/// product. `the_icon_fill_is_the_launchers_own_foreground` reads the
/// ini and asserts it, so the two files cannot part company quietly.
pub(crate) const ICON_FILL: &str = "#dce7f0";

/// What a row wears when nothing better is named. Named here so the
/// build generates it too; a fallback that is missing from the theme is
/// a hole in the list rather than a fallback.
pub(crate) const FALLBACK_ICON: &str = "preferences-system-symbolic";

/// The way back out of a group.
pub(crate) const BACK_ICON: &str = "go-previous-symbolic";

/// Every icon the menu can name, and therefore exactly what the build
/// generates into `/usr/share/icons/kuma`.
///
/// One list, read by `containerfile.rs` to write the build step and by
/// the menu to draw the rows, so a row cannot name an icon the image
/// does not carry. `every_icon_a_row_names_is_one_the_build_generates`
/// checks both directions.
pub(crate) const ICONS: &[&str] = &[
    "applications-system-symbolic",
    "audio-volume-high-symbolic",
    "bluetooth-symbolic",
    "dialog-information-symbolic",
    "drive-harddisk-symbolic",
    "edit-find-symbolic",
    "emblem-system-symbolic",
    "go-previous-symbolic",
    "media-playback-pause-symbolic",
    "network-wireless-symbolic",
    "preferences-system-symbolic",
    "software-update-available-symbolic",
    "system-lock-screen-symbolic",
    "system-log-out-symbolic",
    "system-reboot-symbolic",
    "system-shutdown-symbolic",
    "text-editor-symbolic",
    "user-trash-symbolic",
    "video-display-symbolic",
    "view-refresh-symbolic",
    "weather-clear-night-symbolic",
];

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
    std::env::split_paths(&path).any(|dir| is_executable(&dir.join(program)))
}

/// A file somebody can actually run. `is_file` alone would offer a row
/// for a program that is on PATH and not executable, which is a row
/// that draws and then does nothing. Metadata rather than symlink
/// metadata, because a PATH entry pointing at a real binary is the
/// normal shape of one.
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
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
    pub(crate) group: String,
    pub(crate) label: String,
    /// The icon this row wears, named in kuma's own theme.
    ///
    /// **Adwaita's own icons could not be used directly, and that was
    /// measured.** Every symbolic SVG it ships hardcodes a near-black
    /// fill (three different ones: `#2e3436`, `#474747`, `#222222`),
    /// fuzzel draws the file as it is, and kuma's launcher background is
    /// `#0e1626`. The icons rendered, invisibly. Font Awesome glyphs
    /// fixed the colour by being text, and could not be aligned for the
    /// same reason: a proportional face gives every glyph a different
    /// width, so the labels after them never line up.
    ///
    /// So the build generates `/usr/share/icons/kuma` from Adwaita's
    /// files, repainted in the launcher's own foreground. fuzzel's icon
    /// column is a fixed-width slot, so the rows align exactly, and the
    /// colour is right by construction because it is derived from the
    /// same palette the launcher is themed with.
    pub(crate) icon: String,
    /// The desktop file ID, for an application row. Empty for kuma's own
    /// rows, which have nothing to remember: their order is the browse
    /// order, and sorting them by use would move Power off under
    /// somebody's finger.
    pub(crate) id: String,
    pub(crate) argv: Vec<String>,
    pub(crate) run: Run,
}

impl Item {
    /// The line fuzzel is handed. Everything before the NUL is displayed
    /// and searched; `\0icon\x1f<name>` is fuzzel's dmenu icon protocol.
    ///
    /// `within` is the group being browsed, if any. A row inside its own
    /// group drops the prefix, because repeating `Connect ·` on every
    /// row of the Connect list says nothing. Rows from elsewhere keep it,
    /// which is what makes them legible when search pulls them up from
    /// below the fold.
    fn line(&self, within: Option<&str>) -> String {
        format!("{}\u{0}icon\u{1f}{}", self.text_within(within), self.icon)
    }

    fn text_within(&self, within: Option<&str>) -> String {
        if within == Some(self.group.as_str()) {
            self.label.to_string()
        } else {
            format!("{} · {}", self.group, self.label)
        }
    }

    /// What the row says at the top level: the part a person types
    /// against and the part the tests read.
    #[cfg(test)]
    fn text(&self) -> String {
        self.text_within(None)
    }
}

fn item(group: &str, label: &str, icon: &str, argv: &[&str], run: Run) -> Item {
    Item {
        group: group.to_string(),
        label: label.to_string(),
        icon: icon.to_string(),
        id: String::new(),
        argv: argv.iter().map(|a| (*a).to_string()).collect(),
        run,
    }
}

/// An item whose program this machine has, or nothing.
fn tool(
    tools: &Tools,
    group: &str,
    label: &str,
    icon: &str,
    argv: &[&str],
    run: Run,
) -> Option<Item> {
    let program = argv.first().copied().unwrap_or_default();
    tools.has(program).then(|| item(group, label, icon, argv, run))
}

/// The whole menu, as a pure function of what is installed.
pub(crate) fn items(tools: &Tools) -> Vec<Item> {
    let mut out = Vec::new();

    // nmtui before the graphical editor: a terminal program inherits the
    // terminal's theme instead of arriving as a window from another
    // system, and it works in a TTY, which is the only place left when a
    // session will not start.
    if let Some(wifi) = tools.first(&["nmtui", "nm-connection-editor"]) {
        let run = if wifi == "nmtui" { Run::Terminal } else { Run::Detached };
        out.push(item(CONNECT, "Network", "network-wireless-symbolic", &[&wifi], run));
    }
    out.extend(tool(
        tools,
        CONNECT,
        "Bluetooth",
        "bluetooth-symbolic",
        &["blueman-manager"],
        Run::Detached,
    ));
    if let Some(audio) = tools.first(&["wiremix", "pavucontrol"]) {
        let run = if audio == "wiremix" { Run::Terminal } else { Run::Detached };
        out.push(item(CONNECT, "Audio", "audio-volume-high-symbolic", &[&audio], run));
    }
    out.extend(tool(
        tools,
        CONNECT,
        "Displays",
        "video-display-symbolic",
        &["wdisplays"],
        Run::Detached,
    ));

    // Declaration: opens and shows, never writes. `capture` is the one
    // entry that can end in a write, and it does its own asking.
    out.push(item(DECLARATION, "Edit", "text-editor-symbolic", &["kuma", "edit"], Run::Terminal));
    out.push(item(
        DECLARATION,
        "Show drift",
        "edit-find-symbolic",
        &["kuma", "diff"],
        Run::Terminal,
    ));
    out.push(item(
        DECLARATION,
        "Review proposals",
        "dialog-information-symbolic",
        &["kuma", "capture"],
        Run::Terminal,
    ));

    out.push(item(SYSTEM, "Health", "emblem-system-symbolic", &["kuma", "doctor"], Run::Terminal));
    out.push(item(
        SYSTEM,
        "Check for updates",
        "software-update-available-symbolic",
        &["kuma", "update", "--check"],
        Run::Terminal,
    ));
    out.push(item(SYSTEM, "Rebuild", "view-refresh-symbolic", &["kuma", "build"], Run::Terminal));
    out.push(item(
        SYSTEM,
        "Roll back",
        "go-previous-symbolic",
        &["kuma", "rollback"],
        Run::Terminal,
    ));
    out.push(item(
        SYSTEM,
        "Snapshots",
        "drive-harddisk-symbolic",
        &["kuma", "snapshot"],
        Run::Terminal,
    ));

    out.extend(tool(
        tools,
        NOTIFICATIONS,
        "Do not disturb",
        "media-playback-pause-symbolic",
        &["makoctl", "mode", "-t", "do-not-disturb"],
        Run::Detached,
    ));
    out.extend(tool(
        tools,
        NOTIFICATIONS,
        "Dismiss all",
        "user-trash-symbolic",
        &["makoctl", "dismiss", "-a"],
        Run::Detached,
    ));

    // Power. Stock niri binds a lock and a quit and nothing else, so
    // suspend, reboot and power off have no key and no menu on a kuma
    // desktop today. systemctl reaches them without sudo: logind grants
    // them to the session that owns the seat.
    out.extend(tool(
        tools,
        POWER,
        "Lock",
        "system-lock-screen-symbolic",
        &["swaylock"],
        Run::Detached,
    ));
    out.push(item(
        POWER,
        "Suspend",
        "weather-clear-night-symbolic",
        &["systemctl", "suspend"],
        Run::Detached,
    ));
    out.extend(tool(
        tools,
        POWER,
        "Log out",
        "system-log-out-symbolic",
        &["niri", "msg", "action", "quit"],
        Run::Detached,
    ));
    out.push(item(
        POWER,
        "Reboot",
        "system-reboot-symbolic",
        &["systemctl", "reboot"],
        Run::Detached,
    ));
    out.push(item(
        POWER,
        "Power off",
        "system-shutdown-symbolic",
        &["systemctl", "poweroff"],
        Run::Detached,
    ));

    out
}

/// The application rows.
///
/// Separate from `items` because these are discovered rather than
/// authored: their labels are somebody else's `Name=` and their icons
/// are somebody else's `Icon=`, so the assertions that hold kuma's own
/// rows to kuma's own icon theme cannot apply to them.
pub(crate) fn app_items(apps: &[crate::apps::App]) -> Vec<Item> {
    apps.iter()
        .map(|app| Item {
            group: APPS.to_string(),
            label: app.name.clone(),
            icon: app.icon.clone(),
            id: app.id.clone(),
            argv: app.argv.clone(),
            // Terminal=true means the program has no window of its own.
            // It gets the same treatment as kuma's own printing rows,
            // including the wait, so one that exits immediately does not
            // take its output with it.
            run: if app.terminal { Run::Terminal } else { Run::Detached },
        })
        .collect()
}

/// Every row the menu holds: applications first, then the rows kuma
/// authors.
///
/// A function rather than two lines inside `menu`, because the order is
/// a decision: it puts Apps at the top of the group list, which is where
/// the menu opens. Sabotage swapped the two at the call site and every
/// test still passed, because the tests were building their own list.
pub(crate) fn all_items(tools: &Tools, apps: &[crate::apps::App]) -> Vec<Item> {
    let mut rows = app_items(apps);
    rows.extend(items(tools));
    rows
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
    /// An index into the level's group list.
    Group(usize),
    Item(usize),
}

fn group_icon(group: &str) -> Option<&'static str> {
    Some(match group {
        APPS => "applications-system-symbolic",
        CONNECT => "network-wireless-symbolic",
        DECLARATION => "text-editor-symbolic",
        SYSTEM => "preferences-system-symbolic",
        NOTIFICATIONS => "dialog-information-symbolic",
        POWER => "system-shutdown-symbolic",
        _ => return None,
    })
}

/// The groups present in `items`, in the order they first appear. Not
/// sorted: the authored order is the browse order.
fn groups(items: &[Item]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for entry in items {
        if !out.contains(&entry.group) {
            out.push(entry.group.clone());
        }
    }
    out
}

/// What a group's row reads. The chevron says it descends, and keeps a
/// group's row from reading identically to an item's.
fn group_line(group: &str) -> String {
    let icon = group_icon(group).unwrap_or(FALLBACK_ICON);
    format!("{group}   ›\u{0}icon\u{1f}{icon}")
}

/// The text of a row, without the icon protocol: what is displayed, what
/// is searched, and what the window's width is measured from.
fn row_text(row: Row, items: &[Item], groups: &[String], within: Option<&str>) -> String {
    match row {
        Row::Back => "Back".to_string(),
        Row::Group(index) => format!("{}   ›", groups[index]),
        Row::Item(index) => items[index].text_within(within),
    }
}

fn row_line(row: Row, items: &[Item], groups: &[String], within: Option<&str>) -> String {
    match row {
        Row::Back => format!("Back\u{0}icon\u{1f}{BACK_ICON}"),
        Row::Group(index) => group_line(&groups[index]),
        Row::Item(index) => items[index].line(within),
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
fn top_level(items: &[Item], groups: &[String]) -> (Vec<Row>, usize) {
    let mut rows: Vec<Row> = (0..groups.len()).map(Row::Group).collect();
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

/// What the menu would offer, as text.
///
/// The marker is the fold: rows above it are what opening the menu
/// shows, rows below it are what typing reaches. A listing that hid the
/// difference would misreport the thing it exists to explain.
fn listing(items: &[Item], groups: &[String]) -> String {
    let (rows, visible) = top_level(items, groups);
    rows.iter()
        .enumerate()
        .map(|(index, row)| {
            let marker = if index < visible { " " } else { "·" };
            format!("{marker} {}\n", row_text(*row, items, groups, None))
        })
        .collect()
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
fn pick(lines: &[String], texts: &[String], visible: usize) -> Result<Option<usize>> {
    let chosen = host_output_stdin(
        &[
            "fuzzel",
            "--dmenu",
            "--index",
            // Never return a custom entry: this is a menu, and a
            // hand-typed line that matches nothing is not one of its
            // rows. chosen_index still guards the index, because a
            // launcher's promise is not a bounds check.
            "--only-match",
            "--prompt",
            "kuma  ",
            "--counter",
            "--icon-theme",
            ICON_THEME,
            "--lines",
            &visible.to_string(),
            "--width",
            &width_for(texts).to_string(),
        ],
        &lines.join("\n"),
    )
    .context("cannot run fuzzel")?;
    Ok(chosen_index(chosen.as_deref(), lines.len()))
}

/// How wide the window should be, in characters.
///
/// Sized to the longest row rather than left at the launcher's own
/// width, which is chosen for application names and leaves this menu
/// with a hand's width of empty space down its right side. The padding
/// covers fuzzel's estimate being an estimate: `--width` is in
/// characters and the face is proportional, so a row measured exactly
/// would sometimes wrap.
fn width_for(texts: &[String]) -> usize {
    const PADDING: usize = 6;
    const NARROWEST: usize = 20;
    let longest = texts.iter().map(|text| text.chars().count()).max().unwrap_or(0);
    (longest + PADDING).max(NARROWEST)
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
pub fn menu(config_path: &Path, list: bool) -> Result<()> {
    let tools = Tools::observe();
    // --list is how the menu is looked at without a display: over ssh,
    // in a VM with no session, and by whoever is wondering why a row is
    // missing. It needs no launcher, so the refusal comes after it.
    if !list && !tools.has("fuzzel") {
        anyhow::bail!("kuma menu needs fuzzel, which this image does not have");
    }
    // Applications first, so Apps is the group the menu opens on.
    let mut apps = crate::apps::discover(
        &crate::apps::search_dirs(),
        &std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default(),
        &on_path,
    );
    crate::apps::rank(
        &mut apps,
        &crate::apps::cache_path().map(|path| crate::apps::load_counts(&path)).unwrap_or_default(),
    );
    let items = all_items(&tools, &apps);
    let groups = groups(&items);
    if list {
        // One write, not one per row: `kuma menu --list | head` closes
        // the pipe partway through, and a println! after that panics.
        print!("{}", listing(&items, &groups));
        return Ok(());
    }
    let mut inside: Option<String> = None;

    loop {
        let within = inside.as_deref();
        let (rows, visible) = match within {
            None => top_level(&items, &groups),
            Some(group) => group_level(&items, group),
        };
        let lines: Vec<String> =
            rows.iter().map(|row| row_line(*row, &items, &groups, within)).collect();
        let texts: Vec<String> =
            rows.iter().map(|row| row_text(*row, &items, &groups, within)).collect();
        let Some(index) = pick(&lines, &texts, visible)? else {
            return Ok(());
        };
        match rows[index] {
            Row::Back => inside = None,
            Row::Group(group) => inside = Some(groups[group].clone()),
            Row::Item(item) => {
                let chosen = &items[item];
                // An app row is remembered so the list stops being
                // alphabetical; kuma's own rows are not, because their
                // order is the browse order and shuffling it would move
                // Power off under somebody's finger.
                if chosen.group == APPS {
                    crate::apps::record(&chosen.id);
                }
                return dispatch(&tools, config_path, &chosen.argv, chosen.run);
            }
        }
    }
}

fn dispatch(tools: &Tools, config_path: &Path, argv: &[String], run: Run) -> Result<()> {
    let argv = with_this_kuma(argv);
    match run {
        Run::Detached => spawn_detached(&argv),
        Run::Terminal => {
            let terminal = tools.terminal().context("no terminal in this image to run that in")?;
            spawn_detached(&terminal_argv(&terminal, &tools.editor, config_path, argv))
        }
    }
}

/// The whole command line a terminal row is spawned as.
///
/// A function rather than four pushes at the call site, because what it
/// decides cannot be seen from outside otherwise: sabotage dropped the
/// hold from the spawn and every test still passed, which is the same
/// hole the window height had.
fn terminal_argv(
    terminal: &str,
    editor: &str,
    config_path: &Path,
    argv: Vec<String>,
) -> Vec<String> {
    // `kuma edit` is not a verb; the declaration opens in the person's
    // own editor, which is the whole of what "edit" means here. Expanded
    // here rather than in the list so the list stays a list of commands
    // and the editor stays one lookup.
    let command = if argv.len() == 2 && argv[1] == "edit" {
        vec![editor.to_string(), config_path.to_string_lossy().to_string()]
    } else {
        argv
    };
    vec![
        terminal.to_string(),
        "-e".to_string(),
        "sh".to_string(),
        "-c".to_string(),
        hold_script(&command),
    ]
}

/// Run a command in the terminal window and keep the window there
/// afterwards.
///
/// A terminal launched as `kitty -e <command>` closes the instant the
/// command exits, which for a row like Health means a sudo prompt
/// appears and the window vanishes as the password is finished. Every
/// row here prints something worth reading, and one that runs for a
/// second is exactly the one whose output is never seen.
///
/// A script rather than `kitty --hold`, because that flag is kitty's and
/// this dispatches into whatever terminal the desktop has. It also
/// reports a non-zero exit, which the terminal would otherwise swallow
/// along with the window.
fn hold_script(argv: &[String]) -> String {
    let command = argv.iter().map(|arg| shell_quote(arg)).collect::<Vec<String>>().join(" ");
    let wait = concat!(
        "status=$?; printf '\\n'; ",
        "[ \"$status\" -eq 0 ] || printf 'exited %s\\n' \"$status\"; ",
        "printf '[kuma] press enter to close '; read -r _"
    );
    format!("{command}; {wait}")
}

/// One shell word, whatever is in it.
///
/// The declaration's path arrives here from the caller and a home
/// directory can hold a space or a quote; a command assembled by
/// concatenation is how that becomes two words or an unterminated
/// string.
fn shell_quote(arg: &str) -> String {
    format!("'{}'", arg.replace('\'', r"'\''"))
}

/// The row's command, with `kuma` resolved to the binary drawing the
/// menu.
///
/// **The program only, never an argument that happens to read the
/// same.** An application row's argv is somebody else's `Exec=` line,
/// so a literal `kuma` further along one is a word being handed to
/// another program, and rewriting it to this binary's path would change
/// what that program was asked to do.
fn with_this_kuma(argv: &[String]) -> Vec<String> {
    let mut out = argv.to_vec();
    if out.first().is_some_and(|program| program == "kuma") {
        out[0] = kuma_program();
    }
    out
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

    /// One application, standing in for a machine's whole list. The Apps
    /// group only exists once something is in it, so the tests that hold
    /// the groups have to be given one.
    fn an_app() -> crate::apps::App {
        crate::apps::App {
            id: "org.example.Thing.desktop".to_string(),
            name: "Thing".to_string(),
            icon: "thing".to_string(),
            argv: vec!["thing".to_string()],
            terminal: false,
        }
    }

    /// The menu as it is actually built: applications first, then the
    /// rows kuma authors.
    fn everything_with_apps() -> Vec<Item> {
        all_items(&everything(), &[an_app()])
    }

    /// The invariant that makes the menu safe to grow: a person cannot
    /// change the declaration from a launcher. `capture` is allowed
    /// because it is a dry run that asks before it writes, and `edit`
    /// because it opens the file in an editor the person then saves
    /// themselves.
    ///
    /// **An allowlist, because a list of verbs to refuse cannot know
    /// about a verb that does not exist yet.** This was six names to
    /// refuse, which meant a writing verb added later would pass
    /// unnoticed, and the two rungs after this one are declaration
    /// completeness and migrations: exactly where such a verb comes
    /// from. Every verb the menu names is written here with the reason
    /// it is safe, so admitting one is a sentence somebody has to write.
    #[test]
    fn no_item_writes_the_declaration() {
        const ALLOWED: &[(&str, &str)] = &[
            ("edit", "opens the file in the person's own editor; they save it themselves"),
            ("diff", "read-only"),
            ("capture", "a dry run that shows a proposal and waits to be told yes"),
            ("doctor", "read-only"),
            ("update", "only ever `--check` here, which reads"),
            ("build", "builds an image and writes the lock, never the declaration"),
            ("rollback", "moves the boot order, and prints unless --yes"),
            ("snapshot", "lists; restoring takes an argument no row passes"),
        ];
        for entry in items(&everything()) {
            if entry.argv[0] != "kuma" {
                continue;
            }
            // `get` rather than `[1]`: a bare `kuma` row would panic
            // here, and a test that panics reports the wrong thing.
            let verb = entry.argv.get(1).map(String::as_str).unwrap_or_default();
            assert!(
                ALLOWED.iter().any(|(name, _)| *name == verb),
                "{} runs `kuma {verb}`, which is not in the menu's allowlist; \
                 add it there with the reason it cannot write the declaration",
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

        // fuzzel draws the menu rather than being launched by a row,
        // and it is probed because `menu()` refuses without it.
        assert!(!Tools::none().has("fuzzel"), "the probe is what that refusal reads");
        named.insert("fuzzel".to_string());

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
        let mut seen: Vec<String> = Vec::new();
        for entry in items(&everything()) {
            if seen.last() != Some(&entry.group) {
                assert!(
                    !seen.contains(&entry.group),
                    "{} is split into more than one run of rows",
                    entry.group
                );
                seen.push(entry.group.clone());
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

    /// The listing shows every row and says which are on screen at
    /// rest, because it exists to explain exactly that.
    #[test]
    fn the_listing_marks_the_fold() {
        let items = everything_with_apps();
        let groups = groups(&items);
        let text = listing(&items, &groups);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), groups.len() + items.len(), "every row is listed");
        assert!(
            lines[..groups.len()].iter().all(|line| line.starts_with("  ")),
            "the fold is below the groups"
        );
        assert!(
            lines[groups.len()..].iter().all(|line| line.starts_with('·')),
            "and everything else is under it"
        );
        assert!(text.ends_with('\n'), "a listing is lines, and the last one ends");
    }

    /// Applications come first, so the menu opens on Apps, and they
    /// carry the ID their launch count is remembered against. kuma's own
    /// rows carry none, because their order is the browse order.
    #[test]
    fn application_rows_come_first_and_are_the_only_ones_remembered() {
        let rows = everything_with_apps();
        assert_eq!(rows[0].group, APPS, "the menu opens on Apps");
        assert_eq!(groups(&rows).first().map(String::as_str), Some(APPS));
        for row in &rows {
            if row.group == APPS {
                assert!(!row.id.is_empty(), "{} has nothing to remember it by", row.label);
            } else {
                assert!(row.id.is_empty(), "{} is not an application", row.label);
            }
        }
    }

    /// An application that wants a terminal gets one, and one that does
    /// not must not have a window flash open behind it.
    #[test]
    fn an_application_is_run_the_way_its_entry_asks() {
        let mut console = an_app();
        console.terminal = true;
        assert_eq!(app_items(&[console])[0].run, Run::Terminal);
        assert_eq!(app_items(&[an_app()])[0].run, Run::Detached);
    }

    /// Every icon a row names is one the build generates, and every
    /// icon the build generates is one some row can name. Half of this
    /// stops a row drawing a hole; the other half stops the list growing
    /// entries nothing reads.
    #[test]
    fn every_icon_a_row_names_is_one_the_build_generates() {
        let generated: BTreeSet<&str> = ICONS.iter().copied().collect();
        let mut named: BTreeSet<&str> = BTreeSet::new();
        let items = items(&everything());
        for entry in &items {
            assert!(
                generated.contains(entry.icon.as_str()),
                "{} names {}, ungenerated",
                entry.label,
                entry.icon
            );
            named.insert(entry.icon.as_str());
        }
        // Application rows are exempt by nature: their icon is somebody
        // else's `Icon=`, resolved through the themes the launcher
        // already searches. Everything kuma authors is not, and the Apps
        // group's own icon only exists once something is in it.
        for group in groups(&everything_with_apps()) {
            let icon = group_icon(&group).expect("every group names its own icon");
            assert!(generated.contains(icon), "{group} names {icon}, ungenerated");
            named.insert(icon);
        }
        for icon in [BACK_ICON, FALLBACK_ICON] {
            assert!(generated.contains(icon), "{icon} is named but not generated");
            named.insert(icon);
        }
        assert_eq!(named, generated, "ICONS holds entries nothing draws");
    }

    /// A group with no icon falls back to a gear, which is how a new
    /// group ships looking like an afterthought. Asserted so that adding
    /// one to the list means naming it here too.
    #[test]
    fn every_group_has_its_own_icon() {
        for group in groups(&items(&everything())) {
            assert!(group_icon(&group).is_some(), "{group} has no icon of its own");
        }
    }

    /// The icons are painted in the launcher's own foreground. They sit
    /// in the same rows as that text, so a drift between these two files
    /// is a menu that looks like two products.
    #[test]
    fn the_icon_fill_is_the_launchers_own_foreground() {
        let ini = include_str!("../assets/fuzzel.ini");
        let text = ini
            .lines()
            .find_map(|line| line.trim().strip_prefix("text="))
            .expect("fuzzel.ini sets a text colour");
        assert_eq!(
            format!("#{}", &text[..6]),
            ICON_FILL,
            "the generated icons and the launcher's text have parted company"
        );
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
        for (index, row) in rows.iter().take(groups.len()).enumerate() {
            assert_eq!(*row, Row::Group(index), "the first rows are the groups, in order");
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
            let (rows, visible) = group_level(&items, &group);
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
            let (rows, visible) = group_level(&items, &group);
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
            let shown =
                group_line(&group).split('\u{0}').next().expect("a row has text").to_string();
            assert!(shown.ends_with('›'), "{group}'s row does not say it descends");
        }
        for entry in &items {
            assert!(!entry.line(None).contains('›'), "{} reads like a group", entry.label);
        }
    }

    /// The line handed to fuzzel is the row's words, a NUL, and the icon
    /// protocol. Asserted on the bytes because a launcher that does not
    /// understand them shows the protocol to the person instead.
    #[test]
    fn a_row_is_encoded_the_way_fuzzel_reads_icons() {
        let row = item("Power", "Reboot", "system-reboot-symbolic", &["true"], Run::Detached);
        assert_eq!(row.line(None), "Power · Reboot\u{0}icon\u{1f}system-reboot-symbolic");
        assert_eq!(row.line(None).split('\u{0}').next(), Some("Power · Reboot"));
    }

    /// Inside its own group a row drops the prefix, because repeating
    /// `Connect ·` down the Connect list says nothing. Everywhere else
    /// it keeps it, which is what makes a row legible when search pulls
    /// it up from another group.
    #[test]
    fn a_row_drops_its_group_only_inside_that_group() {
        let row = item("Connect", "Network", "network-wireless-symbolic", &["true"], Run::Detached);
        assert_eq!(row.text_within(Some("Connect")), "Network");
        assert_eq!(row.text_within(Some("Power")), "Connect · Network");
        assert_eq!(row.text_within(None), "Connect · Network");
    }

    /// The window is sized to what it holds, not left at the launcher's
    /// own width, which is chosen for application names.
    #[test]
    fn the_window_is_sized_to_its_longest_row() {
        let narrow = width_for(&["Back".to_string()]);
        let wide = width_for(&["System · Check for updates".to_string(), "Back".to_string()]);
        assert!(wide > narrow, "a longer row makes a wider window");
        assert!(wide > "System · Check for updates".len(), "the longest row fits with room over");
        assert_eq!(width_for(&[]), width_for(&["".to_string()]), "an empty menu still has a width");
    }

    /// A terminal row has to leave its output on the screen. `kitty -e`
    /// closes the window when the command exits, so Health would show a
    /// sudo prompt and then disappear as the password was finished.
    #[test]
    fn a_terminal_row_keeps_its_window_after_the_command_exits() {
        let script = hold_script(&["kuma".to_string(), "doctor".to_string()]);
        assert!(script.starts_with("'kuma' 'doctor';"), "the command runs first");
        assert!(script.contains("read -r _"), "and the window waits");
        assert!(script.contains("press enter"), "with something on screen saying so");
        assert!(script.contains("$status"), "a failure says so rather than vanishing");
        let command_at = script.find("'doctor'").expect("the command is in there");
        let wait_at = script.find("read -r _").expect("the wait is in there");
        assert!(command_at < wait_at, "the wait comes after the command, not before");
    }

    /// A terminal row is spawned as the terminal, a shell, and the held
    /// script. Asserted here because the spawn itself cannot be.
    #[test]
    fn a_terminal_row_is_spawned_through_a_shell_that_waits() {
        let argv = vec!["kuma".to_string(), "doctor".to_string()];
        let full = terminal_argv("kitty", "vi", Path::new("/etc/kuma.toml"), argv);
        assert_eq!(full[..4], ["kitty", "-e", "sh", "-c"]);
        assert_eq!(full.len(), 5, "the script is one argument, whatever is in it");
        assert!(full[4].contains("'kuma' 'doctor'"), "the row's command is what runs");
        assert!(full[4].contains("read -r _"), "and the window is held");
    }

    /// Edit is not a verb: it opens the resolved declaration in the
    /// person's own editor.
    #[test]
    fn the_edit_row_opens_this_machines_declaration() {
        let argv = vec!["kuma".to_string(), "edit".to_string()];
        let full =
            terminal_argv("kitty", "nano", Path::new("/home/x/.config/kuma/kuma.toml"), argv);
        assert!(full[4].starts_with("'nano' '/home/x/.config/kuma/kuma.toml'"));
        assert!(!full[4].contains("'kuma' 'edit'"), "there is no such verb to run");
    }

    /// The declaration's path is passed through here and a home
    /// directory can hold a space or a quote.
    #[test]
    fn an_argument_survives_being_put_in_a_shell() {
        let script = hold_script(&["vi".to_string(), "/home/a b/kuma.toml".to_string()]);
        assert!(script.contains("'/home/a b/kuma.toml'"), "a space stays one word");
        let awkward = hold_script(&["vi".to_string(), "it's.toml".to_string()]);
        assert!(awkward.contains(r"'it'\''s.toml'"), "a quote does not end the word early");
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

    /// `kuma` becomes the binary drawing the menu, and only where it is
    /// the program. The image bakes a copy and a developer's
    /// `~/.cargo/bin` shadows it, so a row that ran whichever one PATH
    /// found would be driving a different kuma than the one on screen.
    #[test]
    fn only_the_program_is_resolved_to_this_kuma() {
        let own = with_this_kuma(&["kuma".to_string(), "doctor".to_string()]);
        assert_eq!(own[0], kuma_program(), "the row runs the kuma that drew the menu");
        assert_eq!(own[1], "doctor", "and its arguments are untouched");

        // An application row's argv comes from a desktop file, which
        // can say anything at all.
        let foreign = vec!["grep".to_string(), "kuma".to_string(), "/etc/hosts".to_string()];
        assert_eq!(with_this_kuma(&foreign), foreign, "an argument reading `kuma` is a word");
    }

    /// A file on PATH that cannot be run is not a program, and a row
    /// offered for one draws and then does nothing.
    #[test]
    fn a_program_on_path_has_to_be_runnable() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("nmtui");
        std::fs::write(&file, "").unwrap();
        assert!(!is_executable(&file), "a file nobody can execute is not a program");
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(is_executable(&file), "and one they can, is");
        assert!(!is_executable(dir.path()), "nor is the directory it sits in");
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
