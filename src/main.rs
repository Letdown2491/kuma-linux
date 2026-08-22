mod backup;
mod bootentries;
mod capture;
mod compose;
mod config;
mod containerfile;
mod edit;
mod hibernate;
mod host;
mod inspect;
mod install;
mod liveiso;
mod lock;
mod overrides;
mod partition;
mod seam;
mod snapshot;
mod state;
mod updates;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use config::Config;
use host::{host_output, host_output_any, note, run_host, run_host_stdin};
use state::{action_json, print_actions, reboot_action, Action};
use std::path::{Path, PathBuf};

pub(crate) const DEFAULT_TAG: &str = "localhost/kuma:latest";

/// What `kuma install` writes when nobody names an image.
///
/// A required `--image` made the whole verb unusable from the place it
/// exists for: somebody on live media has no way to know a registry path,
/// and `kuma install` on its own answered with a clap error. Defaulting
/// costs the honesty of naming an image that may not be published yet,
/// which the dry run states rather than hides, and which fails at the
/// pull rather than silently.
///
/// Bound to what publish.yml actually pushes by a test, because the two
/// are written in different languages and nothing else compares them: get
/// the owner, the package name or the tag scheme out of step and the
/// installer's default points at nothing, which is only discovered by
/// somebody trying to install.
pub(crate) const PUBLISHED_IMAGE: &str = "ghcr.io/letdown2491/kuma:niri";

/// The same image without its tag, which is the name the signature
/// machinery works in: cosign records the signed identity as the bare
/// repository, the policy stanza is keyed by it, and the registries.d
/// entry that says where signatures live is keyed by it again.
///
/// Derived once rather than at each site. Three copies of one
/// `rsplit_once` is three chances for the policy to require a signature
/// for a name the grader does not look under, which is the shape of a bug
/// this feature has already produced once.
pub(crate) fn published_repo() -> &'static str {
    PUBLISHED_IMAGE.rsplit_once(':').map_or(PUBLISHED_IMAGE, |(repo, _)| repo)
}

/// The root filesystem bib puts in the disks it builds. Required at all
/// because fedora-bootc images declare no default and bib fails with
/// "missing required info: DefaultRootFs" without one.
///
/// ext4 rather than xfs, and the difference is load-bearing rather than
/// taste. osbuild pins filesystem UUIDs in its manifest so builds
/// reproduce, so every disk built from one declaration carries the same
/// UUID. XFS refuses outright to mount a UUID that is already mounted,
/// and a desktop automounter grabs each build's partitions as they
/// appear (see `automounted_loop_mounts`), so one automounted disk made
/// every later `kuma vm` die on "Filesystem has duplicate UUID ... -
/// can't mount", surfacing as a Python traceback out of osbuild. ext4
/// permits duplicates, so the collision can no longer fail a build.
const BIB_ROOTFS: &str = "ext4";
const BIB_IMAGE: &str = "quay.io/centos-bootc/bootc-image-builder:latest";

/// What `--version` prints. The number alone cannot answer "is this
/// binary the one that has my last change in it", which is the question
/// that has actually cost time here. See build.rs for where the stamp
/// comes from and why a dirty tree is called out.
pub(crate) const VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (",
    env!("KUMA_BUILD_SHA"),
    " ",
    env!("KUMA_BUILD_DATE"),
    ")"
);

#[derive(Parser)]
#[command(name = "kuma", version = VERSION, about = "Your system is one file.")]
struct Cli {
    /// Path to the kuma config file [default: ./kuma.toml, else
    /// ~/.config/kuma/kuma.toml when the current directory has none]
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    /// Emit JSON on the read surface: the state map (no command), doctor
    /// findings, or the diff
    #[arg(long)]
    json: bool,

    /// With no command, kuma reports where this machine is in its
    /// lifecycle and what the sensible next commands are.
    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Write a kuma.toml in the current directory (on a kuma machine: a
    /// copy of the machine's own baked declaration)
    Init {
        /// Overwrite an existing kuma.toml
        #[arg(long)]
        force: bool,
        /// Use the generic starter even on a kuma machine
        #[arg(long)]
        starter: bool,
    },
    /// Print the Containerfile compiled from kuma.toml
    Generate,
    /// Build the system image locally with podman
    Build {
        /// Image tag to build
        #[arg(long, default_value = DEFAULT_TAG)]
        tag: String,
        /// Report the result as JSON (progress moves to stderr)
        #[arg(long)]
        json: bool,
    },
    /// Point bootc at the built image (prints the command unless --yes)
    Switch {
        /// Image tag to switch to
        #[arg(long, default_value = DEFAULT_TAG)]
        tag: String,
        /// Actually run `bootc switch` (requires root; reboots take effect later)
        #[arg(long)]
        yes: bool,
        /// Report the result as JSON (progress moves to stderr)
        #[arg(long)]
        json: bool,
    },
    /// Build a bootable qcow2 disk from the image and boot it in QEMU
    Vm {
        /// Image tag to make a disk from
        #[arg(long, default_value = DEFAULT_TAG)]
        tag: String,
        /// Directory for the generated disk image
        #[arg(long, default_value = "vm")]
        output: PathBuf,
        /// Build the disk image but don't launch QEMU
        #[arg(long)]
        no_run: bool,
        /// Rebuild the disk image even if one already exists
        #[arg(long)]
        rebuild: bool,
        /// Apply the built image to the RUNNING VM (bootc switch inside,
        /// then reboot); /var (flatpaks, brew, homes) persists
        #[arg(long, conflicts_with_all = ["no_run", "rebuild"])]
        apply: bool,
    },
    /// Install kuma onto a disk, destroying everything on it
    Install {
        /// Disk to install onto. Everything on it is destroyed.
        /// Omit it and kuma lists what it found and asks.
        #[arg(long)]
        disk: Option<PathBuf>,
        /// Image to install, and to fetch updates from afterwards.
        /// Defaults to the image this installer media was built from when
        /// that can be pulled, and to kuma's published image otherwise.
        #[arg(long)]
        image: Option<String>,
        /// Where the installed machine fetches updates from, if that is
        /// not the image being installed. For installing a local build
        /// while tracking a published tag.
        #[arg(long)]
        update_from: Option<String>,
        /// Account to create on the installed machine's first boot
        #[arg(long)]
        user: Option<String>,
        /// Groups for that account
        #[arg(long, default_value = "wheel")]
        groups: String,
        /// Login shell for that account, e.g. fish. Must be a shell the
        /// image installs; without it the account gets the system default.
        #[arg(long)]
        shell: Option<String>,
        /// Hostname for the installed machine
        #[arg(long)]
        hostname: Option<String>,
        /// Encrypt the root partition. Asked for on a terminal when this
        /// is left off; off when it is left off and nobody is there to
        /// ask. The passphrase is read from stdin, before the account
        /// password, never from a flag.
        #[arg(long)]
        encrypt: bool,
        /// Create a swapfile of this size so the machine can hibernate,
        /// e.g. 16G. Asked for on a terminal when this is left off; no
        /// swapfile when it is left off and nobody is there to ask.
        /// `--swap none` declines without being asked.
        #[arg(long, value_name = "SIZE")]
        swap: Option<String>,
        /// Put this machine's home directory back from an offsite backup
        /// on its first boot. Takes a file setting RESTIC_REPOSITORY and
        /// the repository's credentials: the machine has no declaration
        /// of its own yet, so the address comes from the file.
        #[arg(long, value_name = "FILE")]
        restore: Option<PathBuf>,
        /// Do it. Without this, print the plan and change nothing.
        #[arg(long)]
        yes: bool,
        #[arg(long)]
        json: bool,
    },
    /// Build an installer ISO from the image (USB stick, GNOME Boxes)
    Iso {
        /// Image tag to build the installer from
        #[arg(long, default_value = DEFAULT_TAG)]
        tag: String,
        /// Directory for the generated ISO
        #[arg(long, default_value = "iso")]
        output: PathBuf,
        /// Build live media instead: the image is its own installer
        /// environment, so the ISO is roughly a gigabyte smaller and
        /// boots to a desktop you can try before installing. Installing
        /// from it pulls the image over the network.
        #[arg(long)]
        live: bool,
    },
    /// Pull the latest base image, rebuild, and stage the result
    Update {
        /// Image tag to build and stage
        #[arg(long, default_value = DEFAULT_TAG)]
        tag: String,
        /// Ask whether the locked base has moved, and change nothing.
        /// One round-trip to the registry, or to Fedora's repos for a
        /// composed base's kernel; no pull, no build.
        // --tag too: a check builds nothing, so there is no image for a
        // tag to name, and silently ignoring one is how a flag comes to
        // mean nothing.
        #[arg(long, conflicts_with_all = ["yes", "tag"])]
        check: bool,
        /// Actually stage the rebuilt image (requires root; applies on reboot)
        #[arg(long)]
        yes: bool,
        /// Report the result as JSON (progress moves to stderr)
        #[arg(long)]
        json: bool,
    },
    /// Swap the boot order back to the previous deployment (prints unless --yes)
    Rollback {
        /// Actually run `bootc rollback` (requires root; takes effect on next boot)
        #[arg(long)]
        yes: bool,
        /// Report the result as JSON (progress moves to stderr)
        #[arg(long)]
        json: bool,
    },
    /// Converge flatpaks and brew to the declaration and update everything
    /// installed, now rather than at next boot
    Sync {
        /// Report the result as JSON (progress moves to stderr)
        #[arg(long)]
        json: bool,
    },
    /// Reclaim build leftovers: dangling images, abandoned build containers, stale composed bases, live media images
    Clean {
        /// Report what was reclaimed as JSON (progress moves to stderr)
        #[arg(long)]
        json: bool,
    },
    /// Open kuma.toml in your editor ($EDITOR, else nano/vim/vi)
    // Not an editor kuma wrote: it resolves which declaration this
    // machine is actually using and hands that path to the person's own
    // editor. The resolution is the whole value — a local ./kuma.toml
    // outranks ~/.config/kuma/kuma.toml, and editing the wrong one of
    // those and wondering why nothing changed is the trap this closes.
    Edit {
        /// Print the path instead of opening it
        #[arg(long)]
        print: bool,
    },
    /// Declare packages in kuma.toml (pick the list: --rpm, --flatpak, --brew)
    // The list is required and exclusive, expressed to clap rather than
    // checked at runtime: `kuma add --help` now says so, and a call with
    // no list fails before anything reads the declaration.
    #[command(group(clap::ArgGroup::new("list").required(true).args(["rpm", "flatpak", "brew"])))]
    Add {
        /// Package names, Flathub app IDs, or brew formulae
        #[arg(required = true)]
        names: Vec<String>,
        /// Add to [packages].rpm (baked into the image)
        #[arg(long)]
        rpm: bool,
        /// Add to [packages].flatpak (Flathub system apps)
        #[arg(long)]
        flatpak: bool,
        /// Add to [packages].brew (Homebrew formulae)
        #[arg(long)]
        brew: bool,
        /// Report the result as JSON
        #[arg(long)]
        json: bool,
    },
    /// Declare what this machine already runs but kuma.toml doesn't name
    /// (prints the proposal unless --yes; never touches the machine)
    Capture {
        /// Capture only these (default: everything convergence would
        /// otherwise remove, plus undeclared ad-hoc brews)
        names: Vec<String>,
        /// Actually write them into kuma.toml
        #[arg(long)]
        yes: bool,
        /// Report the proposal or the result as JSON
        #[arg(long)]
        json: bool,
    },
    /// Drop declared packages from kuma.toml (searches every [packages] list)
    Remove {
        #[arg(required = true)]
        names: Vec<String>,
        /// Report the result as JSON
        #[arg(long)]
        json: bool,
    },
    /// Show drift between kuma.toml and this machine (read-only)
    Diff {
        /// Emit the drift as JSON
        #[arg(long)]
        json: bool,
    },
    /// Check this machine: deployment, boot health, convergence, GPU, storage, disk (read-only)
    Doctor {
        /// Emit the findings as JSON
        #[arg(long)]
        json: bool,
        /// Emit a support report: the findings plus which kuma, which
        /// image, and the declaration this machine was built from, with
        /// the password hash removed. What to attach to a bug report.
        #[arg(long)]
        report: bool,
    },
    /// Validate the declaration without building anything (read-only)
    Check {
        /// Emit the verdict as JSON
        #[arg(long)]
        json: bool,
    },
    /// List the snapshots this machine has taken, or restore a path from one
    Snapshot {
        /// Restore this path (absolute, inside the snapshot target)
        #[arg(long, value_name = "PATH")]
        restore: Option<String>,
        /// Take it from this snapshot rather than the newest one holding it
        #[arg(long, value_name = "ID", requires = "restore")]
        from: Option<String>,
        /// Actually write the restore (default is a dry run)
        #[arg(long, requires = "restore")]
        yes: bool,
        /// Emit as JSON
        #[arg(long)]
        json: bool,
    },
    /// List the offsite backups, seed the first one, or restore a path from one
    Backup {
        /// Create the repository and make the first copy, deliberately
        #[arg(long, conflicts_with_all = ["list", "restore"])]
        init: bool,
        /// Ask the repository what it is holding
        #[arg(long)]
        list: bool,
        /// Restore this path (absolute, as it lives on the machine)
        #[arg(long, value_name = "PATH")]
        restore: Option<String>,
        /// Take it from this backup rather than the newest one
        #[arg(long, value_name = "ID", requires = "restore")]
        from: Option<String>,
        /// Actually write the restore (default is a dry run)
        #[arg(long, requires = "restore")]
        yes: bool,
        /// Emit as JSON
        #[arg(long)]
        json: bool,
    },
    /// Set this machine up to hibernate: a swapfile, and the kernel
    /// arguments that resume from it
    ///
    /// The installer asks the same question. This is for a machine that
    /// is already running, and for repairing one whose swapfile and
    /// kernel arguments have fallen out of step.
    Hibernate {
        /// Size of the swapfile, e.g. 16G. Defaults to the size of this
        /// machine's memory, which is the most it can ever have to save.
        #[arg(long, value_name = "SIZE", conflicts_with = "off")]
        size: Option<String>,
        /// Take it away again: swap off, the file deleted, the kernel
        /// arguments removed
        #[arg(long)]
        off: bool,
        /// Do it. Without this, print what would happen and change nothing.
        #[arg(long)]
        yes: bool,
        #[arg(long)]
        json: bool,
    },
    /// Print the JSON Schema for kuma.toml, generated from the parser's own types
    Schema,
    /// Hash a password for the [user] section (prompts; prints the line to paste)
    Passwd,
    /// Print shell completions (e.g. `kuma completions fish | source`)
    Completions {
        /// Shell to generate for
        shell: clap_complete::Shell,
    },
    /// Rewrite the boot menu's titles to name the deployments they boot
    ///
    /// Hidden because nothing asks a person to run it:
    /// kuma-boot-titles.service runs it at boot and again after ostree
    /// rotates the deployments at shutdown. It is a verb rather than a
    /// libexec script so the logic that writes into /boot can be tested
    /// against a directory tree instead of asserted about a heredoc.
    #[command(hide = true)]
    BootTitles,
    /// Converge one store's Flatpak permission overrides to the declaration
    ///
    /// Hidden for the same reason as boot-titles: two units run it,
    /// nothing asks a person to. A verb rather than a shell script
    /// because the merge is per key inside a file kuma does not own, and
    /// that is worth testing against a directory tree.
    #[command(hide = true)]
    FlatpakOverrides {
        /// Which store to converge
        #[arg(long, value_enum)]
        scope: config::Scope,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let explicit = cli.config.is_some();
    let root_json = cli.json;
    let config_path = resolve_config(cli.config);
    // Before the first affordance is built: every command kuma goes on to
    // print has to address the declaration this run is actually using.
    state::set_config_flag(explicit.then_some(config_path.as_path()));
    let Some(command) = cli.command else {
        return state::root(&config_path, root_json);
    };
    // Mutating verbs in JSON mode promise exactly one JSON document on
    // stdout — success or failure. Progress and subprocess output move to
    // stderr (host::note / run_host handle the routing). The read verbs
    // (diff, doctor, check) manage their own JSON and stay out of this.
    let mutating_json = root_json
        || match &command {
            Cmd::Build { json, .. }
            | Cmd::Switch { json, .. }
            | Cmd::Update { json, .. }
            | Cmd::Rollback { json, .. }
            | Cmd::Sync { json }
            | Cmd::Clean { json }
            | Cmd::Add { json, .. }
            | Cmd::Capture { json, .. }
            | Cmd::Remove { json, .. }
            | Cmd::Hibernate { json, .. }
            | Cmd::Install { json, .. } => *json,
            _ => false,
        };
    let mutating = matches!(
        &command,
        Cmd::Build { .. }
            | Cmd::Switch { .. }
            | Cmd::Update { .. }
            | Cmd::Rollback { .. }
            | Cmd::Sync { .. }
            | Cmd::Clean { .. }
            | Cmd::Add { .. }
            | Cmd::Capture { .. }
            | Cmd::Remove { .. }
            | Cmd::Hibernate { .. }
            // Install was in the list above and not in this one, so
            // `json_mode` was false for the one verb that cannot be
            // undone: progress and subprocess output stayed on stdout in
            // the middle of the document, and a failed install emitted
            // no JSON at all. Three comments in this file already
            // described the behaviour this line is what provides.
            | Cmd::Install { .. }
    );
    let json_mode = mutating && mutating_json;
    if json_mode {
        host::set_json_output();
    }
    let result = run(command, &config_path, explicit, root_json, json_mode);
    if json_mode {
        if let Err(err) = &result {
            // even failure ends machine-readably; the Error: line still
            // rides stderr through main's Result
            println!("{}", serde_json::json!({ "ok": false, "error": format!("{err:#}") }));
        }
    }
    result
}

fn run(
    command: Cmd,
    config_path: &Path,
    explicit: bool,
    root_json: bool,
    json: bool,
) -> Result<()> {
    match command {
        Cmd::Init { force, starter } => init(force, starter),
        Cmd::Generate => {
            // quiet fallback: stdout is the artifact, keep it clean
            let path = read_config_path(config_path, explicit, false);
            let mut config = Config::load(&path)?;
            // Show what a build would actually do. Printing `FROM …:44`
            // while builds resolve `FROM …@sha256:…` would make this verb
            // a liar about the one thing the lock exists to control. A
            // composed base is never digest-rewritten (builds FROM its
            // content tag), so only a declared image applies its pin.
            if let Some(declared) = config.system.base.clone() {
                if let Some(pinned) = lock::for_config(&path).and_then(|l| l.pin_for(&declared)) {
                    config.system.base = Some(pinned);
                }
            }
            print!("{}", containerfile::generate(&config));
            Ok(())
        }
        Cmd::Build { tag, json: _ } => build(config_path, &tag, json),
        Cmd::Switch { tag, yes, json: _ } => switch(&tag, yes, json),
        Cmd::Vm { tag, output, no_run, rebuild, apply } => {
            vm(&tag, &output, no_run, rebuild, apply)
        }
        Cmd::Install {
            disk,
            image,
            update_from,
            user,
            groups,
            hostname,
            shell,
            encrypt,
            swap,
            restore,
            yes,
            json,
        } => {
            let groups = groups.split(',').filter(|g| !g.is_empty()).map(String::from).collect();
            // Resolved here rather than by clap, because the default is
            // not a constant: on installer media it is what the media was
            // built from. The note is why the resolution has to be
            // visible; see install::image_for_media.
            let (default_image, note_about_it) =
                install::image_for_media(inspect::live_source().as_deref());
            let image = match image {
                Some(named) => named,
                None => {
                    if let Some(why) = note_about_it {
                        note(&format!("\n{why}\n"));
                    }
                    default_image.clone()
                }
            };
            let request = install::Request {
                image,
                default_image,
                update_from,
                user,
                groups,
                hostname,
                shell,
                encrypt,
                swap,
                restore,
                yes,
                json,
            };
            install(disk.as_deref(), request)
        }
        Cmd::Iso { tag, output, live } => {
            if live {
                live_iso(config_path, &tag, &output)
            } else {
                iso(config_path, &tag, &output)
            }
        }
        Cmd::Update { tag, check, yes, json: _ } => {
            let path = read_config_path(config_path, explicit, !json);
            if check {
                update_check(&path, json)
            } else {
                update(&path, &tag, yes, json)
            }
        }
        Cmd::Rollback { yes, json: _ } => rollback(yes, json),
        Cmd::Sync { json: _ } => {
            // Resolved the same way diff resolves it: sync converges to
            // what the image baked, so whether the file being edited is
            // ahead of that is part of the answer, not a footnote.
            let path = read_config_path(config_path, explicit, false);
            sync(Config::load(&path).ok().as_ref(), json)
        }
        Cmd::Clean { json: _ } => {
            // Quiet fallback: which composed bases are live is decided by
            // the declaration, and a machine with only a baked one still
            // deserves a full clean.
            let path = read_config_path(config_path, explicit, false);
            clean(&path, json)
        }
        Cmd::Add { names, rpm, flatpak, brew, json: _ } => {
            // The ArgGroup on Cmd::Add makes clap reject "none" and "more
            // than one" before this runs, with a better message than any
            // written here. This arm is the exhaustiveness the compiler
            // wants, and a backstop if that group is ever loosened.
            let list = match (rpm, flatpak, brew) {
                (true, false, false) => "rpm",
                (false, true, false) => "flatpak",
                (false, false, true) => "brew",
                _ => bail!("pick exactly one of --rpm, --flatpak, --brew"),
            };
            edit::add(config_path, list, &names, json)
        }
        Cmd::Capture { names, yes, json: _ } => {
            // No baked fallback: capture writes, and a write path needs a
            // real file of yours to write into.
            let config = Config::load(config_path)?;
            capture::capture(config_path, &config, &names, yes, json)
        }
        Cmd::Remove { names, json: _ } => edit::remove(config_path, &names, json),
        // `config_path`, not `read_config_path`. The reader's fallback
        // resolves to the baked declaration in /usr, which on a bootc
        // machine is read-only: the launcher's Edit Declaration entry
        // opened an editor on a file that cannot be saved. Same reason
        // `add` and `remove` take this path.
        Cmd::Edit { print } => edit::open(config_path, print),
        Cmd::Diff { json } => {
            let json = json || root_json;
            // announce=false in JSON mode: stdout must stay pure JSON
            let path = read_config_path(config_path, explicit, !json);
            let config = Config::load(&path)?;
            inspect::diff(&config, &path, json)
        }
        Cmd::Doctor { json, report } => inspect::doctor(json || root_json, report),
        Cmd::Check { json } => {
            let json = json || root_json;
            check(&read_config_path(config_path, explicit, !json), json)
        }
        Cmd::Snapshot { restore, from, yes, json } => {
            let json = json || root_json;
            let path = read_config_path(config_path, explicit, !json);
            let config = Config::load(&path)?;
            snapshot::snapshot(&config, &path, restore.as_deref(), from.as_deref(), yes, json)
        }
        Cmd::Backup { init, list, restore, from, yes, json } => {
            let json = json || root_json;
            let path = read_config_path(config_path, explicit, !json);
            let config = Config::load(&path)?;
            backup::backup(
                &config,
                &path,
                backup::Request {
                    init,
                    list,
                    restore: restore.as_deref(),
                    from: from.as_deref(),
                    yes,
                    json,
                },
            )
        }
        Cmd::BootTitles => boot_titles(),
        Cmd::FlatpakOverrides { scope } => flatpak_overrides(scope),
        Cmd::Hibernate { size, off, yes, json: _ } => hibernate_cmd(size, off, yes, json),
        Cmd::Schema => schema(),
        Cmd::Passwd => passwd(),
        Cmd::Completions { shell } => {
            use clap::CommandFactory;
            clap_complete::generate(shell, &mut Cli::command(), "kuma", &mut std::io::stdout());
            Ok(())
        }
    }
}

/// Rootless podman's image ID for a tag — the question the probe, the
/// dry runs, the stale checks, and the root-storage sync all ask.
pub(crate) fn image_id(tag: &str) -> Result<String> {
    host_output(&["podman", "image", "inspect", "--format", "{{.Id}}", tag])
}

/// The GET for the write path: is this declaration one `kuma build` would
/// accept? Agents validate an edit before proposing it; humans get the
/// verdict with its next move.
fn check(config_path: &Path, json: bool) -> Result<()> {
    let shown = config_path.display().to_string();
    match Config::load(config_path) {
        Ok(config) => {
            // A valid declaration is not the end of anything, and this
            // said so only when it failed: the success branch printed a
            // verdict and stopped, while the failure branch named the
            // next move. `check` is one of the three commands the README
            // tells everyone to type, and the JSON did not even carry
            // the `actions` key, so a caller saw a different shape
            // depending on the answer.
            let next = [Action::new("build", "kuma build", "turn this declaration into an image")];
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "valid": true,
                        "config": shown,
                        "declares": {
                            "rpm": config.packages.rpm.len(),
                            "flatpak": config.packages.flatpak.len(),
                            "brew": config.packages.brew.len(),
                            "overrides": config.overrides.len(),
                        },
                        "actions": next.iter().map(action_json).collect::<Vec<_>>(),
                    }))?
                );
            } else {
                // Overrides are counted in apps rather than keys, and
                // only when there are any: a permanent "0 overrides" on
                // every check would be noise on the many declarations
                // that never name one.
                let permissions = match config.overrides.len() {
                    0 => String::new(),
                    1 => ", permissions for 1 app".to_string(),
                    n => format!(", permissions for {n} apps"),
                };
                println!(
                    "{shown} is a valid declaration: {} rpm, {} flatpak, {} brew{permissions}.",
                    config.packages.rpm.len(),
                    config.packages.flatpak.len(),
                    config.packages.brew.len()
                );
                print_actions(&next);
            }
            Ok(())
        }
        Err(err) => {
            let action = Action::new("edit", format!("$EDITOR {shown}"), format!("{err:#}"));
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "valid": false,
                        "config": shown,
                        "error": format!("{err:#}"),
                        "actions": [state::action_json(&action)],
                    }))?
                );
            } else {
                println!("{shown} is not a valid declaration.\n");
                print_actions(&[action]);
            }
            // non-zero exit either way; details are already on stdout
            bail!("declaration invalid")
        }
    }
}

/// The schema is generated from the same types that parse the file, so it
/// cannot drift — and the structs' doc comments ride along as field
/// descriptions. Quiet like `generate`: stdout is the artifact.
fn schema() -> Result<()> {
    let schema = schemars::schema_for!(Config);
    println!("{}", serde_json::to_string_pretty(&schema)?);
    Ok(())
}

/// The config wants a hash, not a password — kuma.toml is meant to live in
/// git. This is the ergonomic path to one; the hash applies only when the
/// account is first created.
fn passwd() -> Result<()> {
    use std::io::IsTerminal;
    let password = if std::io::stdin().is_terminal() {
        let password = rpassword::prompt_password("New password for [user]: ")?;
        let confirm = rpassword::prompt_password("Retype to confirm: ")?;
        if password != confirm {
            bail!("passwords don't match");
        }
        password
    } else {
        // piped stdin: read one line, no prompt — scripting-friendly
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        line.trim_end_matches(['\r', '\n']).to_string()
    };
    if password.is_empty() {
        bail!("empty password");
    }
    println!("\npassword_hash = '{}'", hash_password(&password)?);
    println!("\nPaste that into the [user] section of kuma.toml.");
    Ok(())
}

/// Raw salt bytes to generate. The hash stores the salt base64'd, and
/// sha512-crypt truncates that to 16 characters, so this is the largest
/// salt that survives a round trip: 12 bytes encode to exactly 16
/// characters, which is also all the entropy 16 characters can carry
/// (16 * 6 bits = 96). sha-crypt's own default is 16 *bytes*, which
/// encodes to 22 characters and does not survive: see the test.
const SALT_BYTES: usize = 12;

/// Split out so the test can exercise the real salt and format at rounds
/// it can afford, since the production cost is the whole point of the
/// number hash_password passes.
fn hash_with(password: &str, params: sha_crypt::Params) -> Result<String> {
    use sha_crypt::PasswordHasher;
    let salt = sha_crypt::password_hash::generate_salt();
    sha_crypt::ShaCrypt::new(sha_crypt::Algorithm::Sha512Crypt, params)
        .hash_password_with_salt(password.as_bytes(), &salt[..SALT_BYTES])
        .map(|hash| hash.to_string())
        .map_err(|e| anyhow::anyhow!("hashing failed: {e:?}"))
}

fn hash_password(password: &str) -> Result<String> {
    // The hash is world-readable on a kuma machine (baked kuma.toml) and
    // often committed to git, unlike /etc/shadow's mode-0 protection —
    // so the default 5000 rounds is not enough. 656k (passlib's sha512
    // calibration) makes offline guessing ~130x costlier; glibc reads the
    // rounds= prefix, and login-time cost stays well under a second.
    let params =
        sha_crypt::Params::new(656_000).map_err(|e| anyhow::anyhow!("crypt params: {e:?}"))?;
    hash_with(password, params)
}

const STARTER: &str = r#"# Kuma system definition
schema_version = 1

[system]
# Unset, and that is the default worth having: kuma composes its own base
# from Fedora's repositories, which is what every published image is
# built on. Naming a base here is the escape hatch, and a first
# declaration should not take it before anybody knows there is a choice.
# base = "quay.io/fedora/fedora-bootc:44"
# A desktop is a curated set kuma maintains: "niri" or "cosmic".
# desktop = "niri"
# Pin an IANA timezone across all machines built from this file. Usually
# leave unset: timezone is machine state (`timedatectl set-timezone`).
# timezone = "America/Denver"
# hostname = "kuma-laptop"
# locale = "en_US.UTF-8"

# Primary account, created on first boot and converged after. Get the
# hash from `kuma passwd`; it only applies at creation.
# [user]
# name = "me"
# shell = "fish"
# password_hash = '...'
# ssh_keys = ["ssh-ed25519 AAAA..."]

[packages]
rpm = []
# Flathub system apps, converged on boot: additions install, removals
# uninstall. `flatpak install --user` stays yours.
flatpak = []
# Homebrew CLI tools, converged the same way; good for fast-moving dev
# tools that shouldn't need an image rebuild. Ad-hoc `brew install` on
# the machine stays yours.
# brew = ["ripgrep", "fd", "jq"]

[services]
enable = []
disable = []
"#;

/// `--config` wins untouched. Otherwise ./kuma.toml, falling back to the
/// XDG config dir when the current directory has none — a home for
/// declarations that don't live in a project checkout. Never creates
/// anything; when neither exists, the local name is returned so error
/// messages point somewhere sensible.
fn resolve_config(explicit: Option<PathBuf>) -> PathBuf {
    if let Some(path) = explicit {
        return path;
    }
    let local = PathBuf::from("kuma.toml");
    if local.exists() {
        return local;
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")));
    if let Some(base) = base {
        let xdg = base.join("kuma/kuma.toml");
        if xdg.exists() {
            return xdg;
        }
    }
    local
}

/// Read-only consumers (generate, diff, update) can work straight from
/// the machine's own baked declaration when no working copy resolves —
/// "rebuild what this machine already is" needs no file of yours. Write
/// paths (init/add/remove/build-your-edits) still require a real file,
/// and an explicit --config that doesn't exist stays an error rather
/// than silently meaning something else.
fn read_config_path(resolved: &Path, explicit: bool, announce: bool) -> PathBuf {
    if !explicit && !resolved.exists() {
        let baked = Path::new(state::BAKED_CONFIG);
        if baked.exists() {
            if announce {
                println!(
                    "No local kuma.toml; using this machine's baked declaration ({}).\n",
                    baked.display()
                );
            }
            return baked.to_path_buf();
        }
    }
    resolved.to_path_buf()
}

fn init(force: bool, starter: bool) -> Result<()> {
    let path = PathBuf::from("kuma.toml");
    if path.exists() && !force {
        bail!("kuma.toml already exists (use --force to overwrite)");
    }
    // A kuma machine carries the declaration it was built from — a copy
    // of that beats a generic template, because it's true to this machine.
    let baked = (!starter).then(|| std::fs::read_to_string(state::BAKED_CONFIG).ok()).flatten();
    match baked {
        Some(text) => {
            std::fs::write(&path, text).context("cannot write kuma.toml")?;
            println!("Wrote kuma.toml, a copy of this machine's baked declaration.");
        }
        None => {
            std::fs::write(&path, STARTER).context("cannot write kuma.toml")?;
            println!("Wrote kuma.toml.");
        }
    }
    print_actions(&[Action::new(
        "build",
        "kuma build",
        "edit the file, then build it into a system image",
    )]);
    Ok(())
}

fn build(config_path: &Path, tag: &str, json: bool) -> Result<()> {
    build_image(config_path, tag)?;
    // The edges out of "built" depend on where we are: only a bootc
    // machine can switch to the image; anywhere else a VM is the way in.
    let mut actions = Vec::new();
    if Path::new("/run/ostree-booted").exists() {
        actions.push(Action::new(
            "switch",
            "kuma switch",
            "stage it onto this machine (applies on reboot)",
        ));
    }
    actions.push(Action::new("vm", "kuma vm", "boot it in a disposable VM"));
    if json {
        println!(
            "{}",
            serde_json::json!({
                "ok": true, "built": true, "tag": tag,
                "actions": actions.iter().map(action_json).collect::<Vec<_>>(),
            })
        );
    } else {
        println!("\nBuilt {tag}.");
        print_actions(&actions);
    }
    Ok(())
}

fn build_image(config_path: &Path, tag: &str) -> Result<()> {
    build_image_pinned(config_path, tag, Pin::Follow).map(|_| ())
}

/// Whether this build honors the lock's base digest or goes looking for
/// whatever the declared tag points at now. Only `kuma update` moves a
/// pin, which is what "moves pins deliberately" has to mean if the lock
/// is going to be worth anything.
#[derive(PartialEq)]
enum Pin {
    Follow,
    Refresh,
}

fn build_image_pinned(config_path: &Path, tag: &str, pin: Pin) -> Result<Option<lock::Lock>> {
    let mut config = Config::load(config_path)?;
    let config_text = std::fs::read_to_string(config_path)
        .with_context(|| format!("cannot read {}", config_path.display()))?;
    // For a declared image this is that reference; for kuma's own
    // composed base it is the content-addressed tag the manifest hashes
    // to. Either way it is what the lock's reference must equal for a
    // pin to mean anything.
    let declared_base = config.base_ref();

    // The declaration keeps saying `:44`; only the Containerfile gets the
    // digest. The baked copy is config_text, so the machine still carries
    // the declaration a human wrote, not a resolved artifact of it.
    let mut pinned_digest = lock::for_config(config_path)
        .filter(|_| pin == Pin::Follow)
        .filter(|lock| lock.base.reference == declared_base)
        .map(|lock| lock.base.digest);

    if config.system.base.is_none() {
        // The composed base. The Containerfile always FROMs the content
        // tag (a `localhost/` tag never touches a registry — the trap a
        // pruned digest fell into during the spike); "honoring the pin"
        // means making sure that tag still IS the locked image. When it
        // can't be — recomposed tag, pruned storage, a brand-new machine
        // — the honest move is to say so, compose fresh, and let the
        // lock record the move, not to fail a build that can succeed.
        let present = compose::image_exists(&declared_base);
        let matches_pin = |digest: &String| {
            lock::base_digest(&declared_base).is_ok_and(|current| current == *digest)
        };
        match (&pin, &pinned_digest, present) {
            (Pin::Refresh, _, _) => {
                compose::compose(&config, &declared_base)?;
                pinned_digest = None;
            }
            (Pin::Follow, Some(digest), true) if matches_pin(digest) => {
                note(&format!("Building from the locked composed base ({declared_base})."));
            }
            (Pin::Follow, Some(_), true) => {
                note(
                    "The composed base in storage no longer matches the lock; \
                     building from what's there — the lock will record the move.",
                );
                pinned_digest = None;
            }
            (Pin::Follow, Some(_), false) => {
                note(
                    "The locked composed base is gone from image storage; \
                     composing fresh — the lock will record the move.",
                );
                compose::compose(&config, &declared_base)?;
                pinned_digest = None;
            }
            (Pin::Follow, None, true) => {
                note(&format!("Reusing the composed base in storage ({declared_base})."));
            }
            (Pin::Follow, None, false) => {
                compose::compose(&config, &declared_base)?;
            }
        }
    } else if let Some(digest) = &pinned_digest {
        let pinned = lock::pinned_ref(&declared_base, digest);
        note(&format!("Building from the locked base ({pinned})."));
        config.system.base = Some(pinned);
    }

    let dir = tempfile::tempdir().context("cannot create build directory")?;
    // The image ships the kuma running this build, so a machine installed
    // from it can converge itself without acquiring one by hand.
    let self_exe = std::env::current_exe().context("cannot locate the running kuma binary")?;
    containerfile::write_context(&config, &config_text, &self_exe, dir.path())?;

    // What the tag pointed at before this build moved it. Asked now
    // because afterwards there is no way back to it.
    let previous = image_id(tag).ok();

    run_host(&[
        "podman",
        "build",
        "--tag",
        tag,
        dir.path().to_str().context("non-UTF-8 temp path")?,
    ])?;

    // The tag just moved, stranding the previous build as a dangling
    // <none> (~3.5 GB each — they once piled up to 150 GB).
    //
    // This used to be `podman image prune -f --filter label=io.kuma.image`,
    // which is a sweep of the whole store and MEASURED AT 4.6 TO 5.1
    // SECONDS ON EVERY BUILD, including a first build with nothing to
    // reclaim. A bare prune was 3.8s and listing by label alone 2.1s, so
    // the cost is podman walking storage rather than the filter or the
    // size of the pile. One targeted delete is milliseconds.
    //
    // Reclaiming what is already lying around is `kuma clean`'s job and
    // it is documented as such; what belongs here is only the stray this
    // build just made.
    //
    // Two guards, and both have a real machine behind them. A rebuild
    // that changes nothing leaves the tag on the SAME image, so deleting
    // "the previous one" unguarded deletes what was just built. And an
    // image that still carries another tag is not stranded: this machine
    // had `kuma:latest` and `kuma-motherbox:latest` side by side, and
    // removing by ID there would either fail or take a tag somebody
    // wanted.
    if let (Some(before), Ok(after)) = (previous, image_id(tag)) {
        if before != after
            && has_no_tags(&before)
            && host_output(&["podman", "rmi", &before]).is_ok()
        {
            note("Reclaimed the previous build.");
        }
    }
    // A build that followed a pin built from exactly that digest, so
    // there is nothing to resolve; one that refreshed asks the tag it
    // just pulled what it resolved to.
    let digest = match pinned_digest {
        Some(digest) => digest,
        None => match lock::base_digest(&declared_base) {
            Ok(digest) => digest,
            Err(err) => {
                eprintln!("cannot resolve the base digest ({err}); no lock written");
                return Ok(None);
            }
        },
    };
    // The record is taken from the image that just came out, so it says
    // what shipped rather than what was asked for.
    Ok(lock::record(config_path, &declared_base, digest, tag))
}

/// Where a disk image is mounted, if anything has it open through a loop
/// device.
///
/// `lsblk` refuses a file path outright, so this resolves the file to the
/// loop devices backing it and asks about those. Empty when nothing has
/// it attached, which is every ordinary case.
fn loop_backed_mountpoints(file: &str) -> String {
    let Ok(devices) = host_output_any(&["losetup", "-j", file, "-O", "NAME", "--noheadings"])
    else {
        return String::new();
    };
    devices
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter_map(|device| host_output_any(&["lsblk", "-no", "MOUNTPOINTS", device]).ok())
        .collect::<Vec<String>>()
        .join("\n")
}

/// Whether an image is stranded, which is the only state it is kuma's to
/// delete. An image somebody has tagged is somebody's.
fn has_no_tags(id: &str) -> bool {
    host_output(&["podman", "image", "inspect", "--format", "{{len .RepoTags}}", id])
        .map(|out| out.trim() == "0")
        .unwrap_or(false)
}

fn switch(tag: &str, yes: bool, json: bool) -> Result<()> {
    if !yes {
        // The dry run must tell the truth: with nothing built, the switch
        // it describes could only fail. Same passwordless check the bare
        // `kuma` probe uses; the --yes path gets its error from stage().
        let built = image_id(tag).is_ok();
        let actions = if built {
            vec![Action::new(
                "apply",
                "kuma switch --yes",
                "sync into root storage and stage via bootc (applies on reboot)",
            )]
        } else {
            vec![Action::new("build", "kuma build", "build the system image from the declaration")]
        };
        if json {
            println!(
                "{}",
                serde_json::json!({
                    "ok": true, "dry_run": true, "tag": tag, "image_built": built,
                    "actions": actions.iter().map(action_json).collect::<Vec<_>>(),
                })
            );
            return Ok(());
        }
        if !built {
            println!("{tag} is not built; there is nothing to switch to yet.\n");
            print_actions(&actions);
            return Ok(());
        }
        println!(
            "Would sync {tag} into root podman storage, then run (as root):\n\n  bootc switch --transport containers-storage {tag}\n"
        );
        println!("Re-run with --yes to apply. The change takes effect on next boot;");
        println!("the previous deployment stays available for `kuma rollback`.");
        return Ok(());
    }
    if !stage(tag)? {
        bail!("nothing staged; the system already runs this image (did `kuma build` succeed?)");
    }
    let reboot = reboot_action();
    if json {
        println!(
            "{}",
            serde_json::json!({
                "ok": true, "staged": true, "tag": tag,
                "actions": [action_json(&reboot)],
            })
        );
    } else {
        println!("\nStaged.");
        print_actions(&[reboot]);
    }
    Ok(())
}

/// Sync the image into root storage and stage it with bootc. False when
/// nothing new was staged — the system already runs this image.
fn stage(tag: &str) -> Result<bool> {
    // bootc runs as root and resolves containers-storage against ROOT's
    // storage; without this sync it would deploy whatever stale copy the
    // last `kuma vm` left there — silently.
    let scratch = tempfile::tempdir().context("cannot create scratch directory")?;
    let local_id = sync_image_to_root(tag, scratch.path())?;
    run_host(&["sudo", "bootc", "switch", "--transport", "containers-storage", tag])?;
    // switch is a no-op when the origin spec is unchanged (every switch
    // after the first!) — bootc upgrade is what re-pulls the origin and
    // stages new content. Then verify something IS staged: without the
    // check a no-op switch reboots into the same deployment looking like
    // success.
    run_host(&["sudo", "bootc", "upgrade"])?;
    // Root is warm — stamp which image the deployment now corresponds to
    // (staged here, or already booted when nothing staged), so the
    // passwordless bare-`kuma` probe can spot a future build outrunning
    // it. Best-effort: a missing stamp just skips that check, and doctor
    // rewrites it from the truth.
    let stamp = scratch.path().join("deployed-image-id");
    if std::fs::write(&stamp, format!("{local_id}\n")).is_ok() {
        if let Ok(stamp) = path_str(&stamp) {
            let _ = host_output(&[
                "sudo",
                "install",
                "-D",
                "-m",
                "0644",
                stamp,
                state::DEPLOYED_ID_FILE,
            ]);
        }
    }
    let status = host_output(&["sudo", "bootc", "status"])?;
    Ok(status.lines().any(|l| l.trim_start().to_lowercase().starts_with("staged")))
}

/// The full update loop. The pull is the point: `kuma build` alone reuses
/// the cached base, so a same-tag base (fedora-bootc:44) never moves
/// without it. Unlike `kuma switch`, an unchanged system is a normal
/// outcome here, not an error.
/// The cheap questions: has the base moved, and what would a rebuild
/// pick up?
///
/// Which of those can be answered depends on how the base is built, and
/// the split is not cosmetic:
///
/// - **A declared base** is a tag, so the question is whether the tag
///   moved: one registry round-trip. Its packages are not in play, since
///   kuma's Containerfile runs `dnf install` rather than `dnf upgrade`
///   and a rebuild leaves what the base already shipped exactly where it
///   is. Asking dnf what could upgrade would list hundreds of packages a
///   rebuild would not touch.
/// - **A composed base** has no tag, and every package in it is in play,
///   because `kuma update` recomposes the whole thing from the repos. So
///   the useful question is the wide one, and dnf answers it from repo
///   metadata in seconds: see updates.rs.
///
/// A prediction either way. The lock diff after the update is what
/// actually happened, and this never claims to be that.
///
/// Note this is the *builder's* check. A machine running a published
/// image asks bootc instead, and `bootc upgrade --check` already exists.
fn update_check(config_path: &Path, json: bool) -> Result<()> {
    let config = Config::load(config_path)?;
    let base = &config.base_ref();

    // Where the machine is, on either kind of base, in the same shape
    // `kuma update` reports a move in. One key means one thing: emitting
    // a bare string here and an object there would make `fedora_release`
    // something a caller has to type-check before reading.
    //
    // Never a prediction. For a composed base the target is only knowable
    // by composing, and for a declared one it would cost a pull of the
    // base this command deliberately does not pull.
    let release_now =
        lock::for_config(config_path).and_then(|l| fedora_release_of(&l.base.reference));

    if config.system.base.is_none() {
        // A composed base has no registry tag whose movement can be
        // checked; the repos it composes from move continuously. The
        // honest answer is what an update would do, not a fake "current".
        let lock = lock::for_config(config_path);
        let manifest_changed = lock.as_ref().is_some_and(|lock| &lock.base.reference != base);
        let source = update_source();
        note(&format!("Asking dnf what has moved in the repos ({})...", source.name()));
        let moved = updates::moved(&source);
        let update = Action::new(
            "update",
            "kuma update",
            "recompose and rebuild; the lock diff shows what moved",
        );
        // An update is worth offering when something would come of it, or
        // when nobody could establish that it wouldn't. A confident "you
        // are current" is the one case with nothing to suggest.
        let actions: Vec<Action> = match &moved {
            Ok(moved) if moved.is_empty() && !manifest_changed => Vec::new(),
            _ => vec![update],
        };
        if json {
            println!(
                "{}",
                serde_json::json!({
                    "ok": true, "composed": true, "locked": lock.is_some(),
                    "base": base, "manifest_changed": manifest_changed,
                    "fedora_release": release_move_json(None, &release_now),
                    "updates": match &moved {
                        Ok(moved) => serde_json::json!({
                            "checked": true, "source": source.name(),
                            "moved": updates::moves_json(moved),
                            "security": updates::security_count(moved),
                        }),
                        Err(err) => serde_json::json!({
                            "checked": false, "source": source.name(),
                            "error": err.to_string(),
                        }),
                    },
                    "actions": actions.iter().map(action_json).collect::<Vec<_>>(),
                })
            );
            return Ok(());
        }
        if manifest_changed {
            println!("The base manifest changed since the lock: the next build composes a new base ({base}).");
        } else {
            println!("The base is composed locally from Fedora's repos ({base}).");
        }
        print_release_now(&release_now);
        match &moved {
            Ok(moved) => print_moves(moved, &source),
            // Named rather than silent: a check that quietly drops half
            // its answer reads exactly like a clean bill.
            Err(err) => println!("What has moved could not be checked ({err})."),
        }
        if !actions.is_empty() {
            println!();
            print_actions(&actions);
        }
        return Ok(());
    }

    let Some(lock) = lock::for_config(config_path) else {
        // Both verbs record a lock, but only one of them works from here:
        // `build` is a write path and needs a real file, so on a machine
        // reading its own baked declaration it would fail. Naming an edge
        // that can't be taken is worse than naming a heavier one.
        let build = if config_path == Path::new(state::BAKED_CONFIG) {
            Action::new("update", "kuma update", "record what this declaration resolves to")
        } else {
            Action::new("build", "kuma build", "record what this declaration resolves to")
        };
        if json {
            println!(
                "{}",
                serde_json::json!({
                    "ok": true, "locked": false, "base": base,
                    "fedora_release": release_move_json(None, &release_now),
                    "actions": [action_json(&build)],
                })
            );
        } else {
            println!("Nothing pinned yet: {base} has no lock to have moved from.");
            print_actions(&[build]);
        }
        return Ok(());
    };

    let moved = lock::base_moved(base, &lock.base.digest)?;
    let update = Action::new("update", "kuma update", "move the pin and rebuild on the new base");
    let actions: Vec<Action> = if moved { vec![update] } else { Vec::new() };

    if json {
        println!(
            "{}",
            serde_json::json!({
                "ok": true, "locked": true, "base": base, "moved": moved,
                "digest": { "locked": lock.base.digest },
                "fedora_release": release_move_json(None, &release_now),
                "actions": actions.iter().map(action_json).collect::<Vec<_>>(),
            })
        );
        return Ok(());
    }
    // Both kinds of base say where the machine is. This line used to sit
    // in the composed branch only, so concepts.md promised every reader
    // something half of them could not see.
    print_release_now(&release_now);
    if moved {
        // The new digest isn't named: learning it would cost a second
        // tool, and `kuma update` prints the full before-and-after from
        // the lock anyway, one line per package.
        println!("{base} moved since {}.", short(&lock.base.digest));
        println!();
        print_actions(&actions);
    } else {
        println!("{base} is current ({}).", short(&lock.base.digest));
        // Saying "nothing to do" would overclaim: only the base is
        // pinned, so a rebuild can still resolve newer packages.
        println!("Only the base is pinned, so a rebuild can still move package versions.");
    }
    Ok(())
}

fn update(config_path: &Path, tag: &str, yes: bool, json: bool) -> Result<()> {
    let config = Config::load(config_path)?;
    // Both reads happen before the pull, and that is the whole trick.
    // Pulling can move a declared tag, and a recompose replaces the
    // content tag's image in place, so asking either of them afterwards
    // returns the new answer to the old question.
    let before = lock::for_config(config_path);
    let before_release = before.as_ref().and_then(|l| fedora_release_of(&l.base.reference));
    match &config.system.base {
        Some(base) => run_host(&["podman", "pull", base])?,
        // Composed base: the packages come from Fedora's repos at
        // compose time, so "pull the base" means "refresh the compose
        // environment" (repo definitions + Fedora's minimal manifest);
        // Pin::Refresh below forces the actual recompose.
        None => run_host(&["podman", "pull", compose::COMPOSE_ENV])?,
    }
    // The one command that moves the pin. Everything else builds from
    // whatever the lock already says, so an update is the only way the
    // base underneath you changes, and it says what changed.
    let after = build_image_pinned(config_path, tag, Pin::Refresh)?;
    let after_release = after.as_ref().and_then(|l| fedora_release_of(&l.base.reference));
    let release_move = release_move(&before_release, &after_release);
    let moved = match (&before, &after) {
        (Some(before), Some(after)) => Some(lock::diff(before, after)),
        _ => None,
    };
    if !json {
        print_lock_diff(moved.as_ref());
        print_release_move(release_move.as_ref());
    }
    if !yes {
        let stage_hint = Action::new(
            "stage",
            "kuma update --yes",
            "stage it: applies on reboot; the previous deployment stays for kuma rollback",
        );
        if json {
            println!(
                "{}",
                serde_json::json!({
                    "ok": true, "built": true, "staged": false, "tag": tag,
                    "changes": lock_diff_json(moved.as_ref()),
                    "fedora_release": release_move_json(release_move.as_ref(), &after_release),
                    "actions": [action_json(&stage_hint)],
                })
            );
        } else {
            println!("\nBuilt {tag}.");
            print_actions(&[stage_hint]);
        }
        return Ok(());
    }
    let staged = stage(tag)?;
    let reboot = reboot_action();
    if json {
        let actions: Vec<_> = if staged { vec![action_json(&reboot)] } else { vec![] };
        println!(
            "{}",
            serde_json::json!({
                "ok": true, "staged": staged, "up_to_date": !staged, "tag": tag,
                "changes": lock_diff_json(moved.as_ref()),
                "fedora_release": release_move_json(release_move.as_ref(), &after_release),
                "actions": actions,
            })
        );
    } else if staged {
        println!("\nStaged.");
        print_actions(&[reboot]);
    } else {
        println!("\nAlready up to date; the system runs this image.");
    }
    Ok(())
}

/// What an update actually moved. The base line is the one that matters
/// (it is the only pin), but the package churn underneath it is what
/// makes a broken update bisectable, so a bounded sample of it prints
/// too; the lock has the rest, and `git diff kuma.lock` is the full story.
/// The Fedora release an image is built on, read out of the image.
///
/// Not derived from the tag. `fedora-bootc:45` is not a promise that the
/// packages inside came from Fedora 45: a branched release still carries
/// rawhide's repo definitions for a while, so composing "from 45" can
/// produce a base that calls itself 46. The image is the only thing that
/// knows, so the image is asked.
///
/// `None` for an image that is not in local storage or does not say,
/// which is a reason to stay quiet rather than to guess.
fn fedora_release_of(image: &str) -> Option<String> {
    os_release_of_image(image, "$VERSION_ID").ok().filter(|out| !out.is_empty())
}

/// A shell expansion evaluated against an image's own os-release.
///
/// `--pull=never` is the load-bearing flag, and it is not an
/// optimisation. podman's default is `--pull=missing`, which turns
/// reading a local image into downloading whatever its tag points at
/// *now*, and that is wrong in two different ways here. `update --check`
/// says in its own comments that it does not pull, and a read-only check
/// that quietly fetches a base over a metered link breaks that promise.
/// Worse, `update` reads the release *before* its pull exactly so it can
/// say the release moved: on a machine without the base in local storage,
/// pulling here would answer with the new release, `release_move` would
/// see nothing between two identical numbers, and a Fedora major would
/// arrive unannounced. That is the whole feature, silently inverted.
///
/// An image that is not in local storage therefore reports nothing, which
/// is what the callers already treat as "stay quiet rather than guess".
fn os_release_of_image(image: &str, expr: &str) -> Result<String> {
    host_output(&os_release_argv(image, expr))
}

/// Split out from the call so the `--pull=never` above is a thing a test
/// can assert rather than a flag someone can drop while tidying.
fn os_release_argv(image: &str, expr: &str) -> Vec<String> {
    ["podman", "run", "--rm", "--pull=never", image, "sh", "-c"]
        .iter()
        .map(|s| s.to_string())
        .chain(std::iter::once(format!(". /usr/lib/os-release && printf %s \"{expr}\"")))
        .collect()
}

/// Whether the release actually moved, given what each side reported.
///
/// One unknown side means silence, not a change: an image that could not
/// be asked is not evidence of a move, and announcing "Fedora ? to 45"
/// would be worse than saying nothing.
fn release_move(before: &Option<String>, after: &Option<String>) -> Option<(String, String)> {
    match (before, after) {
        (Some(from), Some(to)) if from != to => Some((from.clone(), to.clone())),
        _ => None,
    }
}

/// The one line that stops a distro upgrade from arriving unannounced.
///
/// `kuma update` already reports what moved, but a Fedora major shows up
/// there as several hundred package lines and nothing that says which
/// release you are on. This is printed after the diff and before the
/// staging gate, so the answer to "did I just change Fedora version" is
/// visible while nothing has been staged yet and `--yes` is still
/// required.
fn print_release_move(moved: Option<&(String, String)>) {
    if let Some((from, to)) = moved {
        println!();
        println!("This is a Fedora release change: {from} to {to}.");
        println!(
            "Everything in the declaration is rebuilt against {to}'s packages. \
             Nothing is staged yet, and the current deployment stays for kuma rollback."
        );
    }
}

/// Where the machine is now, for `update --check`, which never predicts
/// where it is going. Silent when the release could not be read, because
/// "Currently on Fedora ?" is worse than saying nothing.
fn print_release_now(release: &Option<String>) {
    if let Some(release) = release {
        println!("Currently on Fedora {release}; `kuma update` says so before it stages a change.");
    }
}

fn release_move_json(
    moved: Option<&(String, String)>,
    current: &Option<String>,
) -> serde_json::Value {
    serde_json::json!({
        "current": current,
        "changed": moved.is_some(),
        "from": moved.map(|(from, _)| from.clone()),
        "to": moved.map(|(_, to)| to.clone()),
    })
}

/// Which rpmdb the repos get compared against.
///
/// A machine that runs kuma has the better answer and always has it: its
/// own rpmdb describes what is booted right now, needs no image in podman
/// storage, and so does not care whether kuma arrived by ISO, by `kuma
/// switch`, or by a rebase. A host that is not a kuma machine has to be
/// asked about the image it builds instead, because its rpmdb describes a
/// system this declaration does not govern.
///
/// The case this reads wrong is a kuma machine building for a *different*
/// machine, where the local rpmdb answers about the wrong system. Rare
/// enough to accept, and the output names which system answered.
fn update_source() -> updates::Source {
    if Path::new(state::BAKED_CONFIG).exists() {
        updates::Source::Machine
    } else {
        updates::Source::Image(DEFAULT_TAG.to_string())
    }
}

/// The lock diff's vocabulary, for a diff that hasn't happened yet.
fn print_moves(moved: &[updates::Move], source: &updates::Source) {
    let since = source.since();
    if moved.is_empty() {
        println!("Nothing has moved in the repos since {since}.");
        return;
    }
    println!("{} packages have moved in the repos since {since}.", moved.len());
    // Twice the lock diff's limit. That one summarizes a base bump, where
    // the count is the story; this one is read to decide whether to
    // update at all, and the package that decides it is often near the
    // bottom (a compositor, a shell tool) rather than in the CVEs.
    const SHOWN: usize = 20;
    for item in moved.iter().take(SHOWN) {
        let severity = match item.severity {
            Some(severity) => format!(" ({severity})"),
            None => String::new(),
        };
        println!("      {} {} -> {}{}", item.name, item.from, item.to, severity);
    }
    if moved.len() > SHOWN {
        println!("      ... and {} more", moved.len() - SHOWN);
    }
    let security = updates::security_count(moved);
    if security == 0 {
        println!("rpm   {} moved, none with a security advisory", moved.len());
        return;
    }
    let by_severity: Vec<String> = updates::by_severity(moved)
        .values()
        .map(|(severity, n)| format!("{n} {severity}"))
        .collect();
    println!(
        "rpm   {} moved, {security} with security advisories ({})",
        moved.len(),
        by_severity.join(", ")
    );
}

fn print_lock_diff(moved: Option<&lock::LockDiff>) {
    let Some(moved) = moved else { return };
    if moved.is_empty() {
        println!("\nNothing moved: same base digest, same packages.");
        return;
    }
    println!();
    if moved.base_from != moved.base_to {
        println!("base  {} -> {}", short(&moved.base_from), short(&moved.base_to));
    } else {
        println!("base  unchanged ({})", short(&moved.base_to));
    }
    const SHOWN: usize = 10;
    for (name, from, to) in moved.changed.iter().take(SHOWN) {
        println!("      {name} {from} -> {to}");
    }
    if moved.changed.len() > SHOWN {
        println!("      ... and {} more changed", moved.changed.len() - SHOWN);
    }
    let counts = [
        (moved.changed.len(), "changed"),
        (moved.added.len(), "added"),
        (moved.removed.len(), "removed"),
    ];
    let summary: Vec<String> =
        counts.iter().filter(|(n, _)| *n > 0).map(|(n, what)| format!("{n} {what}")).collect();
    if !summary.is_empty() {
        println!("rpm   {}", summary.join(", "));
    }
}

fn short(digest: &str) -> String {
    let hex = digest.strip_prefix("sha256:").unwrap_or(digest);
    format!("sha256:{}", &hex[..hex.len().min(12)])
}

fn lock_diff_json(moved: Option<&lock::LockDiff>) -> serde_json::Value {
    match moved {
        None => serde_json::Value::Null,
        Some(m) => serde_json::json!({
            "base": { "from": m.base_from, "to": m.base_to, "moved": m.base_from != m.base_to },
            "rpm": {
                "changed": m.changed.iter().map(|(name, from, to)| serde_json::json!({
                    "name": name, "from": from, "to": to,
                })).collect::<Vec<_>>(),
                "added": m.added,
                "removed": m.removed,
            },
        }),
    }
}

/// Run by `kuma-boot-titles.service`: once at boot, and again after
/// ostree rotates the deployments at shutdown, which is when the titles
/// actually go out of date.
///
/// Silent on a machine with nothing to fix. It runs on every boot of
/// every kuma machine, and a converger that narrates itself into the
/// journal on every boot is a converger whose one interesting line
/// nobody sees. Silent too where there is no menu to write to at all (a
/// container, a build, live media) rather than failing a unit that had
/// no work to do.
fn boot_titles() -> Result<()> {
    let entries = Path::new(bootentries::ENTRIES);
    if !entries.is_dir() {
        return Ok(());
    }
    let moved =
        bootentries::apply(entries, Path::new("/")).context("rewriting the boot menu's titles")?;
    for retitle in &moved {
        println!("{}: {} -> {}", retitle.name(), retitle.from, retitle.to);
    }
    Ok(())
}

/// Run by `kuma-flatpak-overrides.service` at boot, by its user-scope
/// twin at login, and by `kuma sync` on demand.
///
/// Silent when it changed nothing, like every other converger that runs
/// on every boot: the interesting line is the one nobody sees if each
/// boot prints three uninteresting ones.
fn flatpak_overrides(scope: config::Scope) -> Result<()> {
    // The user pass runs inside a systemd --user manager, which sets
    // HOME; the system pass never reads it. Neither is a place to guess:
    // an empty HOME would make the store a relative path and write one
    // person's permissions into whatever directory the command was run
    // from.
    let home = std::env::var("HOME").unwrap_or_default();
    if scope == config::Scope::User && home.is_empty() {
        bail!("HOME is unset, so there is no user store to converge");
    }
    let changed = overrides::converge_store(scope, Path::new("/"), Path::new(&home))
        .with_context(|| format!("converging {} flatpak overrides", scope.as_str()))?;
    for (app, what) in &changed {
        for id in &what.set {
            println!("{app}: set {}", id.replace('\t', " "));
        }
        for id in &what.removed {
            println!("{app}: removed {}", id.replace('\t', " "));
        }
    }
    Ok(())
}

/// The update's undo. bootc keeps the previous deployment around exactly
/// for this; the command is thin on purpose — verify there IS a rollback
/// target (so the failure is kuma-flavored, not bootc's), name what the
/// next boot lands on, and surface the one sharp edge: a staged-but-
/// never-booted deployment is discarded by the swap.
fn rollback(yes: bool, json: bool) -> Result<()> {
    if !yes {
        if json {
            let apply = Action::new(
                "apply",
                "kuma rollback --yes",
                "swap the boot order to the previous deployment (applies on reboot; discards any staged deployment)",
            );
            println!(
                "{}",
                serde_json::json!({
                    "ok": true, "dry_run": true, "would_run": "bootc rollback",
                    "actions": [action_json(&apply)],
                })
            );
            return Ok(());
        }
        println!("Would run (as root):\n\n  bootc rollback\n");
        println!("Re-run with --yes to apply. The boot order swaps to the previous");
        println!("deployment and takes effect on next boot; rolling back again before");
        println!("that reboot swaps the order back. A staged (never booted) deployment,");
        println!("if present, is discarded.");
        return Ok(());
    }
    let status = host_output(&["sudo", "bootc", "status", "--format", "json"])
        .context("cannot read bootc status (is this a bootc machine?)")?;
    let status_json: serde_json::Value =
        serde_json::from_str(&status).context("cannot parse bootc status")?;
    let Some((target, staged)) = rollback_facts(&status_json) else {
        bail!("no rollback deployment on this machine; nothing to roll back to");
    };
    if staged {
        note("note: discarding the staged (never booted) deployment.\n");
    }
    run_host(&["sudo", "bootc", "rollback"])?;
    // The deployment stamp names the image we just rolled back FROM, and
    // the rollback target's podman image ID is unknowable here — drop the
    // stamp so the passwordless probe skips its freshness check; the next
    // doctor run rewrites it from the truth.
    let _ = host_output(&["sudo", "rm", "-f", state::DEPLOYED_ID_FILE]);
    let reboot = Action::new(
        "reboot",
        "sudo systemctl reboot",
        "boot the previous deployment now; kuma rollback again undoes the swap",
    );
    if json {
        println!(
            "{}",
            serde_json::json!({
                "ok": true, "target": target, "staged_discarded": staged,
                "actions": [action_json(&reboot)],
            })
        );
    } else {
        println!("\nBoot order swapped; next boot lands on {target}.");
        print_actions(&[reboot]);
    }
    Ok(())
}

/// What a rollback would land on, from `bootc status --format json`: the
/// rollback slot's image (digest-pinned when possible — the tag alone is
/// ambiguous, since booted and rollback usually share it), plus whether a
/// staged deployment would be discarded. None when there is no rollback
/// deployment to land on.
fn rollback_facts(json: &serde_json::Value) -> Option<(String, bool)> {
    let slot = |name: &str| json.pointer(&format!("/status/{name}")).filter(|v| !v.is_null());
    let rollback = slot("rollback")?;
    let image = rollback
        .pointer("/image/image/image")
        .and_then(|v| v.as_str())
        .unwrap_or("the previous deployment");
    let digest = rollback.pointer("/image/imageDigest").and_then(|v| v.as_str()).unwrap_or("");
    let target = match digest.strip_prefix("sha256:") {
        Some(d) if d.len() >= 12 => format!("{image} ({})", &d[..12]),
        _ => image.to_string(),
    };
    Some((target, slot("staged").is_some()))
}

/// The systemctl calls a sync makes, in the order they must run, each
/// with whether the sync depends on it.
///
/// reset-failed comes first because a unit that spent its start limit
/// refuses `systemctl start` outright, and that is the exact state this
/// verb exists to leave. A converger that fails often enough burns
/// StartLimitBurst, doctor grades it Fail and prints `kuma sync` as the
/// fix, and without this the prescribed fix was refused by systemd
/// rather than run: the tool told a person to do something that could
/// not work until they knew to reset-failed by hand first.
fn convergence_calls(units: &[&str]) -> Vec<(Vec<String>, bool)> {
    [("reset-failed", false), ("start", true)]
        .iter()
        .map(|(verb, must_succeed)| {
            let mut call: Vec<String> =
                ["sudo", "systemctl", verb].iter().map(|s| s.to_string()).collect();
            call.extend(units.iter().map(|u| u.to_string()));
            (call, *must_succeed)
        })
        .collect()
}

/// On-demand convergence: start the same units boot and the daily timer
/// run, so there stays exactly one convergence path. systemctl blocks
/// until each oneshot finishes, so success here means converged.
fn sync(declared: Option<&Config>, json: bool) -> Result<()> {
    let mut units: Vec<&str> = Vec::new();
    if Path::new(state::BAKED_FLATPAKS).exists() {
        units.push("kuma-flatpak-sync.service");
    }
    if Path::new(state::BAKED_BREWS).exists() {
        units.push("kuma-brew-sync.service");
    }
    if Path::new(state::BAKED_OVERRIDES).exists() {
        units.push("kuma-flatpak-overrides.service");
    }
    if units.is_empty() {
        // Three different truths hide behind "nothing to start" — name the
        // one that holds here, with its next move.
        if Path::new("/usr/lib/kuma").is_dir() {
            // Nothing declared to converge is a terminal like in-sync: no
            // forward move, but the JSON still carries the actions key so
            // an agent sees the same shape every mutating verb promises.
            if json {
                println!("{}", serde_json::json!({ "ok": true, "converged": [], "actions": [] }));
            } else {
                println!("Nothing to converge: this image declares no flatpaks or brew formulae.");
            }
            return Ok(());
        }
        if Path::new("/run/ostree-booted").exists() {
            bail!("this bootc machine isn't running a kuma image; `kuma build` then `kuma switch` adopt one");
        }
        bail!("not a kuma machine; sync converges a machine booted into a kuma image (`kuma vm` boots one)");
    }
    for (call, must_succeed) in convergence_calls(&units) {
        let args: Vec<&str> = call.iter().map(String::as_str).collect();
        if must_succeed {
            run_host(&args)?;
        } else {
            let _ = run_host(&args);
        }
    }
    // The user store's converger cannot be started with sudo: it runs in
    // the caller's own systemd manager, which is the entire reason it is
    // a second unit rather than a second ExecStart. Best effort, because
    // a machine converging from a console with no session has no user
    // manager to talk to, and that is not a failed sync.
    let mut converged: Vec<String> = units.iter().map(|u| u.to_string()).collect();
    if Path::new("/usr/lib/systemd/user/kuma-flatpak-overrides.service").exists()
        && run_host(&["systemctl", "--user", "start", "kuma-flatpak-overrides.service"]).is_ok()
    {
        converged.push("kuma-flatpak-overrides.service (user)".to_string());
    }
    // What sync converges to is the declaration the *image* baked, and
    // for a long time it said "Converged" and then sent you to `kuma
    // diff` to confirm a match it could not produce: edit the
    // declaration, run sync, and it starts convergers that read
    // /usr/lib/kuma and never heard of the edit. Saying which
    // declaration was applied is the whole fix.
    let behind = declared.is_some_and(|c| crate::inspect::baked_is_behind(c, Path::new("/")));
    let mut actions = Vec::new();
    if behind {
        actions.push(Action::new(
            "build",
            "kuma build",
            "bake the edits this image does not have yet",
        ));
    }
    actions.push(Action::new(
        "diff",
        "kuma diff",
        if behind {
            "see what is still ahead of this image"
        } else {
            "confirm the machine now matches its declaration"
        },
    ));
    if json {
        println!(
            "{}",
            serde_json::json!({
                "ok": true,
                "converged": converged,
                "baked_declaration_behind": behind,
                "actions": actions.iter().map(action_json).collect::<Vec<_>>(),
            })
        );
    } else {
        println!("Converged: {}", converged.join(", "));
        if behind {
            println!(
                "\nConverged to the declaration this image baked, which is behind yours: \
                 the edits it does not have cannot reach a converger until a build of them boots."
            );
        }
        print_actions(&actions);
    }
    Ok(())
}

/// Two kinds of leftovers accumulate in podman storage. Every rebuild
/// Where the removed menu kept its launch counts.
///
/// Its own path under the cache dir, not a location anybody chose, which
/// is what makes it kuma's to delete rather than the person's to keep.
fn menu_cache_path() -> Option<PathBuf> {
    let base = match std::env::var_os("XDG_CACHE_HOME") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => PathBuf::from(std::env::var_os("HOME")?).join(".cache"),
    };
    Some(base.join("kuma").join("menu-apps"))
}

/// strands the previous image as a dangling <none>; worse, an interrupted
/// build abandons its buildah "working container", which pins its layers
/// while being invisible to `podman images` — one was found holding 68 GB.
/// `kuma build` self-cleans its own label; this reclaims everything,
/// including composed-base content tags the declaration no longer uses.
fn clean(config_path: &Path, json: bool) -> Result<()> {
    // Held rather than printed as it goes, so the same run can render as
    // text or as one JSON document. Progress chatter from host::note
    // already routes to stderr in JSON mode.
    let say = |line: String| {
        if !json {
            println!("{line}");
        }
    };
    // An in-flight build's working container looks identical to an
    // abandoned one — don't yank the layers out from under it. The [ ]
    // keeps the pattern from matching kuma's own pgrep invocation.
    if host_output(&["pgrep", "-f", "podman[ ].*build|^buildah"]).is_ok() {
        bail!("a build appears to be running; retry when it finishes");
    }
    let before = avail_bytes();

    let external = host_output_any(&[
        "podman",
        "ps",
        "-a",
        "--external",
        "--format",
        "{{.ID}} {{.Names}} {{.Status}}",
    ])
    .unwrap_or_default();
    let abandoned: Vec<&str> = external
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let (id, name, status) = (fields.next()?, fields.next()?, fields.next()?);
            (status == "Storage" && name.contains("-working-container")).then_some(id)
        })
        .collect();
    if !abandoned.is_empty() {
        let mut args = vec!["podman", "rm", "--force"];
        args.extend(&abandoned);
        host_output(&args)?; // capture the ID-per-line chatter
        say(format!("Removed {} abandoned build container(s).", abandoned.len()));
    }

    let pruned = prune_dangling(&["podman", "image", "prune", "-f"])?;
    if pruned > 0 {
        say(format!("Removed {pruned} dangling image(s)."));
    }

    // Composed bases are tagged, so dangling-pruning never reclaims
    // them, and every base manifest edit strands the previous content
    // tag (~1 GB each). Live means: the tag the current declaration
    // composes to, plus whatever its lock records. Without a readable
    // declaration there is no telling live from stale, so none go.
    let mut base_pruned = 0;
    if let Ok(config) = Config::load(config_path) {
        let mut keep = vec![compose::content_tag(&config)];
        if let Some(lock) = lock::for_config(config_path) {
            keep.push(lock.base.reference);
        }
        let listed = host_output_any(&[
            "podman",
            "images",
            "--format",
            "{{.Repository}}:{{.Tag}}",
            "--filter",
            "reference=localhost/kuma-base",
        ])
        .unwrap_or_default();
        for tag in stale_base_tags(&listed, &keep) {
            if host_output(&["podman", "rmi", &tag]).is_ok() {
                base_pruned += 1;
            }
        }
        if base_pruned > 0 {
            say(format!("Removed {base_pruned} stale composed base(s)."));
        }
    }

    // The live ISO's root filesystem is built under a tag of its own,
    // which `live_iso` calls disposable where it defines it: it exists
    // between two podman calls and is worth nothing once the ISO is
    // written. Nothing reclaimed it, so a 4 GB image sat in storage until
    // somebody went looking for space.
    //
    // Unconditional, unlike the composed bases. A base tag has to be told
    // from the live one the declaration still composes to; this one has
    // no live version to confuse it with, only a leftover.
    let live_pruned = host_output(&["podman", "rmi", LIVE_TAG]).is_ok();
    if live_pruned {
        say("Removed the live ISO build image.".to_string());
    }

    // `kuma switch` copies each image into root storage, where the same
    // stranding happens. Only on a bootc machine — elsewhere root storage
    // isn't part of kuma's flow. bootc's own image store is untouched.
    let mut root_pruned = 0;
    if Path::new("/run/ostree-booted").exists() {
        match prune_dangling(&["sudo", "podman", "image", "prune", "-f"]) {
            Ok(n) if n > 0 => {
                root_pruned = n;
                say(format!("Removed {n} dangling image(s) from root storage."));
            }
            Ok(_) => {}
            Err(_) => say("Root storage skipped (sudo declined).".to_string()),
        }
    }

    // The menu ranked applications by how often they were launched and
    // kept the counts here. The menu is gone; the file is not, because
    // it sits in every existing niri user's home and convergence does
    // not reach into home ([[kuma-cleanup-conventions]]: what kuma left
    // behind is kuma's to reclaim, what a person put there is not).
    // Kilobytes, and unread by anything that ships.
    let mut menu_cache_removed = false;
    if let Some(cache) = menu_cache_path() {
        if cache.exists() && std::fs::remove_file(&cache).is_ok() {
            menu_cache_removed = true;
            say(format!("Removed {} (the old menu's launch counts).", cache.display()));
        }
    }

    let nothing = abandoned.is_empty()
        && pruned == 0
        && base_pruned == 0
        && !live_pruned
        && root_pruned == 0
        && !menu_cache_removed;
    let freed = match (before, avail_bytes()) {
        (Some(before), Some(after)) if after > before => Some(after - before),
        _ => None,
    };
    if json {
        println!(
            "{}",
            serde_json::json!({
                "ok": true,
                "containers_removed": abandoned.len(),
                "images_pruned": pruned,
                "base_images_pruned": base_pruned,
                "live_image_pruned": live_pruned,
                "root_images_pruned": root_pruned,
                "menu_cache_removed": menu_cache_removed,
                "freed_bytes": freed,
                "actions": [],
            })
        );
        return Ok(());
    }
    if nothing {
        say("Nothing to reclaim.".to_string());
    } else if let Some(freed) = freed {
        say(format!("Freed {}.", human_size(freed)));
    }
    Ok(())
}

fn prune_dangling(cmd: &[&str]) -> Result<usize> {
    Ok(host_output(cmd)?.lines().filter(|l| !l.trim().is_empty()).count())
}

/// Which of podman's listed references are stale composed bases: shaped
/// like a tag kuma minted, and not in the live set. Pure set arithmetic
/// so the deletion policy is testable without a podman.
fn stale_base_tags(listed: &str, keep: &[String]) -> Vec<String> {
    listed
        .lines()
        .map(str::trim)
        .filter(|t| compose::is_content_tag(t) && !keep.iter().any(|k| k == t))
        .map(str::to_string)
        .collect()
}

/// Free space on the filesystem holding the user's podman storage.
fn avail_bytes() -> Option<u64> {
    let home = std::env::var("HOME").ok()?;
    let out = host_output(&["df", "--output=avail", "-B1", &home]).ok()?;
    out.lines().nth(1)?.trim().parse().ok()
}

fn human_size(bytes: u64) -> String {
    match bytes {
        b if b >= 1 << 30 => format!("{:.1} GiB", b as f64 / (1u64 << 30) as f64),
        b if b >= 1 << 20 => format!("{:.0} MiB", b as f64 / (1u64 << 20) as f64),
        b => format!("{b} bytes"),
    }
}

fn vm(tag: &str, output: &Path, no_run: bool, rebuild: bool, apply: bool) -> Result<()> {
    std::fs::create_dir_all(output)
        .with_context(|| format!("cannot create {}", output.display()))?;
    let output = std::fs::canonicalize(output)?;
    if apply {
        return vm_apply(tag);
    }
    let disk = output.join("qcow2/disk.qcow2");

    if !disk.exists() || rebuild {
        build_disk(tag, &output)?;
    } else {
        println!("Reusing existing disk {} (use --rebuild to regenerate).", disk.display());
        // A silently stale disk once cost an hour of "where's my theme":
        // the image had the changes, the reused disk predated them.
        let stamped = std::fs::read_to_string(output.join("image-id")).unwrap_or_default();
        let current = image_id(tag).unwrap_or_default();
        if !current.is_empty() && stamped.trim() != current {
            println!(
                "WARNING: {tag} is newer than this disk; it will NOT have your latest changes. Re-run with --rebuild to pick them up."
            );
        }
    }

    if no_run {
        println!("Disk ready: {}", disk.display());
        println!("Boot it later with `kuma vm`, or import it into GNOME Boxes / virt-manager.");
        return Ok(());
    }
    boot_disk(&disk, &output)
}

fn build_disk(tag: &str, output: &Path) -> Result<()> {
    let local_id = sync_image_to_root(tag, output)?;
    let bib_config = output.join("config.toml");
    std::fs::write(
        &bib_config,
        bib_config_toml(vm_ssh_key(output).as_deref(), image_shell(tag).as_deref()),
    )?;
    println!("Building qcow2 with bootc-image-builder (this takes a few minutes)...");
    run_bib(output, &bib_config, "qcow2", tag, &[])?;
    // Stamp which image this disk came from, so a later `kuma vm` can
    // warn when the image has moved on and the disk is silently stale.
    std::fs::write(output.join("image-id"), &local_id)?;
    Ok(())
}

/// Installer media: the same bib pipeline as the VM disk, different
/// output type. Unlike `kuma vm` nothing risky is preseeded — media
/// meant for hardware gets no baked-in test user and no disk-wiping
/// kickstart; Anaconda runs interactively.
fn iso(config_path: &Path, tag: &str, output: &Path) -> Result<()> {
    let config = Config::load(config_path)?;
    // Installer media outlives the machine it was meant for — surface
    // what identity it carries at the moment it's being baked in.
    if let Some(user) = &config.user {
        println!(
            "note: this installer bakes the declared user '{}' (account and password hash,\ncreated at first boot). For media you'll share, build from a declaration\nwithout [user]; Anaconda's create-a-user screen comes back automatically.\n",
            user.name
        );
    }
    std::fs::create_dir_all(output)
        .with_context(|| format!("cannot create {}", output.display()))?;
    let output = std::fs::canonicalize(output)?;
    let local_id = sync_image_to_root(tag, &output)?;
    let bib_config = output.join("iso-config.toml");
    std::fs::write(&bib_config, iso_config_toml(&config))?;

    // bib picks the Anaconda environment's package set from a def file
    // keyed by the image's os-release "ID-VERSION_ID" — kuma's branding
    // makes that "kuma-44", which bib has never heard of (it ships defs
    // for fedora, bluefin, bazzite, ...). Kuma's installer environment IS
    // Fedora's, so lift the newest fedora def out of the bib image and
    // mount it back in under kuma's name.
    let distro = os_release_of_image(tag, "$ID-$VERSION_ID")
        .context("cannot read os-release from the image")?;
    let mut def = host_output(&[
        "sudo",
        "podman",
        "run",
        "--rm",
        "--entrypoint",
        "/bin/sh",
        BIB_IMAGE,
        "-c",
        "cat \"$(ls /usr/share/bootc-image-builder/defs/fedora-*.yaml | sort -V | tail -1)\"",
    ])
    .context("cannot extract a fedora installer def from bootc-image-builder")?;
    def.push('\n');
    let def_path = output.join("installer-def.yaml");
    std::fs::write(&def_path, def)?;
    let def_mount =
        format!("{}:/usr/share/bootc-image-builder/defs/{distro}.yaml:ro", path_str(&def_path)?);

    println!("Building installer ISO with bootc-image-builder (this takes a while; it assembles a full Anaconda environment)...");
    run_bib(&output, &bib_config, "anaconda-iso", tag, &[def_mount])?;
    std::fs::write(output.join("image-id"), &local_id)?;
    let iso_path = output.join("bootiso/install.iso");
    println!("ISO ready: {}", iso_path.display());
    println!("Boot it in GNOME Boxes, or write it to a USB stick with e.g. `sudo dd if={} of=/dev/sdX bs=4M status=progress`.", iso_path.display());
    Ok(())
}

/// 0600, for a file that holds a credential.
///
/// Rust writes a new file 0644 minus the umask, which on an ordinary
/// machine means every local account can read it. That is the wrong
/// default for an account's password hash sitting in a temporary
/// directory, and the right one costs one syscall.
fn restrict_to_owner(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("cannot restrict permissions on {}", path.display()))
}

/// What the plan prints, gathered rather than passed as eight
/// positional arguments: this is a description of a disk about to be
/// destroyed, and telling two `bool`s apart by position is a poor way to
/// get one right.
struct PlanView<'a> {
    disk: &'a Path,
    to_file: bool,
    image: &'a str,
    local: bool,
    updates: &'a str,
    encrypt: bool,
    /// The swapfile, if one was asked for, and what is worth saying
    /// about it. Carried as the rendered lines rather than a size,
    /// because what the plan has to show is the size *and* the hazard,
    /// and the hazard depends on encryption rather than on the number.
    swap: Option<(u64, Vec<String>)>,
    layout: &'a [partition::Partition],
    disk_mib: u64,
}

/// The plan a person reads before agreeing to lose a disk. Separate from
/// `install` so that "print this only for a human" is one condition
/// rather than a `!json` on every line.
fn print_install_plan(view: &PlanView) {
    println!("Install plan");
    // "file", not "image": the line under it is the container image,
    // and two rows labelled the same thing in a plan somebody reads
    // before destroying something is a poor place to save a word.
    println!(
        "  {}     {}  (everything {} it is destroyed)",
        if view.to_file { "file" } else { "disk" },
        view.disk.display(),
        if view.to_file { "in" } else { "on" }
    );
    println!(
        "  image    {}  ({})",
        view.image,
        if view.local { "in local storage" } else { "pulled when you confirm" }
    );
    println!("  updates  fetched from {} afterwards", view.updates);
    // Shown in full because none of it can be changed afterwards, and
    // because btrfs is load-bearing rather than taste: `[snapshots]` is
    // btrfs-only, so a machine installed on anything else could never
    // use a feature kuma ships.
    println!(
        "  layout   GPT, btrfs root{} (snapshots need btrfs):",
        if view.encrypt { " inside LUKS" } else { "" }
    );
    for part in view.layout {
        println!(
            "             {:<10} {:>5}  {}",
            part.label,
            part.size_text(view.disk_mib),
            part.purpose
        );
    }
    // Under the layout because that is what it comes out of: the
    // swapfile is not a partition, it is a file on the root one, and
    // showing it as a fourth row would say the disk has a shape it does
    // not have.
    if let Some((mib, warnings)) = &view.swap {
        println!("  swap     {} swapfile on the root, for hibernate", hibernate::size_text(*mib));
        for warning in warnings {
            println!("           {warning}");
        }
    }
}

/// What this machine's kernel says about hibernating at all, as the
/// warning it deserves, or None when there is nothing to say.
///
/// Silent when `/sys/power/state` cannot be read, the same way the
/// encryption check is silent when it cannot tell: a question that could
/// not be asked is not an answer, and warning about a kernel this could
/// not reach would be inventing news.
fn lockdown_warning_here() -> Option<String> {
    let power_state = std::fs::read_to_string("/sys/power/state").ok()?;
    let lockdown = std::fs::read_to_string("/sys/kernel/security/lockdown").ok();
    hibernate::lockdown_warning(
        hibernate::kernel_allows_hibernation(&power_state),
        lockdown.as_deref().and_then(hibernate::active_lockdown).as_deref(),
    )
}

/// Set hibernate up on a machine that is already running, or take it
/// away again.
///
/// The installer asks the same question at the one moment it is cheapest
/// to answer, before anything is on the disk. This verb exists because
/// that moment passes: a machine installed before this shipped, or by
/// somebody who said no, would otherwise have reinstalling as its only
/// route to hibernate, and reinstalling is not a fix for a feature.
///
/// It is also the repair. A swapfile whose offset no longer matches the
/// kernel arguments is the failure this whole feature has to avoid, and
/// running this on a machine that already has a usable file changes only
/// the arguments: the file is left exactly where it is, because moving
/// it is what created the problem.
fn hibernate_cmd(size: Option<String>, off: bool, yes: bool, json: bool) -> Result<()> {
    let status = hibernate::probe();
    // /sysroot on a booted ostree deployment, / anywhere else, and the
    // mapper rather than the partition when the disk is encrypted: the
    // swapfile lives on the filesystem, which is inside the container.
    let source = host_output(&["findmnt", "-no", "SOURCE", "/sysroot"])
        .or_else(|_| host_output(&["findmnt", "-no", "SOURCE", "/"]))
        .unwrap_or_default();
    let device = inspect::root_device(&source).to_string();
    if device.is_empty() {
        bail!("cannot tell which device holds the root filesystem, so there is nowhere to put a swapfile");
    }
    let cmdline = std::fs::read_to_string("/proc/cmdline").unwrap_or_default();

    if off {
        return hibernate_off(&device, &cmdline, &status, yes, json);
    }

    // A file that is already there is never remade. Growing a swapfile
    // means allocating a different one, which moves the offset, and an
    // offset that moved is the one failure mode worth all of this. So
    // the size is only asked about when there is nothing there.
    let existing = status.file_offset;
    if existing.is_none() && std::path::Path::new(hibernate::FILE).exists() {
        bail!(
            "{} exists but the kernel would refuse it as swap.\n\n\
             Take it away with `kuma hibernate --off --yes` and make a new one; \n\
             kuma will not reuse a file it cannot vouch for.",
            hibernate::FILE
        );
    }

    // What the disk can spare, minus the room an update needs to stage a
    // whole second deployment before it discards the old one.
    let free_mib = host_output(&["findmnt", "-nbo", "AVAIL", "--target", "/var"])
        .ok()
        .and_then(|out| out.trim().parse::<u64>().ok())
        .map(|bytes| bytes / (1024 * 1024))
        .unwrap_or(0);
    let spare_mib = free_mib.saturating_sub(hibernate::RESERVE_MIB);

    let meminfo = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let size_mib = match (existing, &size) {
        // Repairing: the size is whatever the file already is, and a
        // --size naming something else would be describing a file this
        // verb has just refused to remake.
        (Some(_), Some(_)) => bail!(
            "there is already a {} swapfile; --size would not resize it.\n\n\
             `kuma hibernate --yes` puts the kernel arguments back in step with it, and\n\
             `kuma hibernate --off --yes` takes it away so a different size can be made.",
            hibernate::size_text(status.file_mib.unwrap_or(0))
        ),
        (Some(_), None) => status.file_mib.unwrap_or(0),
        (None, Some(text)) => {
            let Some(mib) = hibernate::parse_size(text)? else {
                bail!("--size none asks for no swapfile, which is what this machine already has")
            };
            if let Some(why) =
                hibernate::objections(mib, spare_mib, hibernate::SPARE_ON_DISK).into_iter().next()
            {
                bail!("{why}");
            }
            mib
        }
        (None, None) => {
            let mib = hibernate::default_size_mib(&meminfo)
                .context("cannot read this machine's memory, so pass --size")?;
            if let Some(why) =
                hibernate::objections(mib, spare_mib, hibernate::SPARE_ON_DISK).into_iter().next()
            {
                bail!("{why}\n\nPass --size with something smaller if that is a trade you want.");
            }
            mib
        }
    };
    let repairing = existing.is_some();
    let mut warnings = if repairing {
        Vec::new()
    } else {
        // Encryption is read the same way doctor reads it, from the
        // device the filesystem is actually on.
        let types = host_output(&["lsblk", "-no", "TYPE", &device]).unwrap_or_default();
        let encrypted = types.split_whitespace().any(|kind| kind == "crypt");
        hibernate::warnings(size_mib, hibernate::ram_mib(&meminfo), encrypted)
    };
    // Said even when repairing, because a machine whose kernel refuses
    // is one where the repair is correct and still changes nothing, and
    // that is worth hearing before the reboot rather than after it.
    warnings.extend(lockdown_warning_here());

    if !yes {
        let action = Action::new(
            "apply",
            match &size {
                Some(text) => format!("kuma hibernate --size {text} --yes"),
                None => "kuma hibernate --yes".to_string(),
            },
            if repairing {
                "set the kernel arguments to match the swapfile that is there"
            } else {
                "make the swapfile and set the kernel arguments (takes effect on reboot)"
            },
        );
        if json {
            println!(
                "{}",
                serde_json::json!({
                    "ok": true, "dry_run": true, "repairing": repairing,
                    "swap_mib": size_mib, "device": device,
                    "warnings": warnings,
                    "actions": [action_json(&action)],
                })
            );
            return Ok(());
        }
        if repairing {
            println!(
                "There is already a {} swapfile at {}.\n",
                hibernate::size_text(size_mib),
                hibernate::FILE
            );
            println!("Would leave the file exactly where it is and set:\n");
        } else {
            println!("Would make a {} swapfile:\n", hibernate::size_text(size_mib));
            println!("  {:<28} on its own btrfs subvolume, never snapshotted", hibernate::FILE);
            println!("  {:<28} mounts it at boot, below zram", "/etc/fstab");
            println!();
            println!("and set:\n");
        }
        for karg in hibernate::kargs("<the root filesystem's UUID>", "<the file's offset>") {
            println!("  {karg}");
        }
        for warning in &warnings {
            println!("\n  {warning}");
        }
        println!("\nNothing has been changed.");
        print_actions(&[action]);
        return Ok(());
    }

    if !repairing {
        note(&format!("Making a {} swapfile...", hibernate::size_text(size_mib)));
        let dir = tempfile::tempdir().context("cannot create a working directory")?;
        let script = dir.path().join("enable");
        std::fs::write(&script, hibernate::enable_script())?;
        run_host(&["sudo", "chmod", "a+rX", path_str(dir.path())?])?;
        let out = host_output(&[
            "sudo",
            "bash",
            path_str(&script)?,
            &device,
            &size_mib.to_string(),
            "/etc/fstab",
        ])
        .context("cannot create the swapfile")?;
        // The script prints the offset on its last line and nothing else
        // on stdout, so this is the offset the kernel has to be told.
        let printed = out.lines().next_back().unwrap_or_default().trim().to_string();
        if printed.parse::<u64>().is_err() {
            bail!("the swapfile was made but its offset came back as {printed:?}");
        }
    }
    // Both paths, because both can find the labels wrong. The create
    // path's script does this too and doing it twice is free; the repair
    // path has no script at all, and without this the fix doctor
    // prescribes for a mislabelled swapfile would not fix it.
    //
    // Recursive from the mount point: there are two labels, and the
    // directory's is the one a correct file hides.
    let _ = host_output(&["sudo", "restorecon", "-RF", hibernate::MOUNT]);

    // Re-read rather than reuse: on the repair path the offset came from
    // a probe, and on the create path it came from a script, and this is
    // the number a wrong value silently destroys a session with. Asking
    // the file once more, the same way doctor will, costs a second.
    let after = hibernate::probe();
    let offset = after
        .file_offset
        .context("the swapfile is there but will not report an offset; it is not usable as swap")?;
    let fs_uuid = host_output(&["findmnt", "-no", "UUID", "--target", "/var"])
        .context("cannot read the root filesystem's UUID")?
        .trim()
        .to_string();

    note("Setting the kernel arguments...");
    let args = hibernate::karg_arguments(&cmdline, &fs_uuid, &offset.to_string());
    let mut argv = vec!["sudo".to_string(), "rpm-ostree".to_string()];
    argv.extend(args);
    run_host(&argv).context(
        "cannot set the resume kernel arguments (rpm-ostree is what edits them on an ostree machine)",
    )?;

    let reboot = reboot_action();
    if json {
        println!(
            "{}",
            serde_json::json!({
                "ok": true, "enabled": true, "repairing": repairing,
                "swap_mib": size_mib, "resume_offset": offset, "resume": format!("UUID={fs_uuid}"),
                "warnings": warnings,
                "actions": [action_json(&reboot)],
            })
        );
        return Ok(());
    }
    println!("\nDone. resume=UUID={fs_uuid} resume_offset={offset}");
    for warning in &warnings {
        println!("\n  {warning}");
    }
    println!(
        "\nThe kernel arguments take effect on the next boot; until then this\n\
         machine has the swapfile but cannot resume from it."
    );
    print_actions(&[reboot]);
    Ok(())
}

/// Take hibernate away again.
///
/// Its own function because it is the reverse of the one above and
/// shares nothing with it but the device: everything here is a removal,
/// and the ordering that matters is the opposite one.
fn hibernate_off(
    device: &str,
    cmdline: &str,
    status: &hibernate::Status,
    yes: bool,
    json: bool,
) -> Result<()> {
    let fstab = std::fs::read_to_string("/etc/fstab").unwrap_or_default();
    let stripped = hibernate::strip_fstab(&fstab);
    let kargs = hibernate::karg_removal(cmdline);
    let file = std::path::Path::new(hibernate::FILE).exists();
    if !file && stripped == fstab && kargs.is_empty() {
        if json {
            println!(
                "{}",
                serde_json::json!({ "ok": true, "changed": false, "reason": "nothing set up" })
            );
        } else {
            println!("This machine has no kuma swapfile, no fstab entry for one, and no");
            println!("resume kernel arguments. There is nothing to take away.");
        }
        return Ok(());
    }
    if !yes {
        let action = Action::new("apply", "kuma hibernate --off --yes", "take it away");
        if json {
            println!(
                "{}",
                serde_json::json!({
                    "ok": true, "dry_run": true, "swapfile": file,
                    "fstab_lines": stripped != fstab, "kargs": !kargs.is_empty(),
                    "actions": [action_json(&action)],
                })
            );
            return Ok(());
        }
        println!("Would take hibernate away from this machine:\n");
        if file {
            println!(
                "  swap off, {} deleted ({})",
                hibernate::FILE,
                hibernate::size_text(status.file_mib.unwrap_or(0))
            );
        }
        if stripped != fstab {
            println!("  the two lines kuma added to /etc/fstab removed");
        }
        if !kargs.is_empty() {
            println!("  resume= and resume_offset= removed from the kernel arguments");
        }
        println!("\nNothing has been changed.");
        print_actions(&[action]);
        return Ok(());
    }
    let dir = tempfile::tempdir().context("cannot create a working directory")?;
    let script = dir.path().join("disable");
    std::fs::write(&script, hibernate::disable_script())?;
    // The stripped fstab is handed over as a file rather than generated
    // in shell, so that what gets written is exactly what `strip_fstab`
    // produced and is covered by its tests.
    let new_fstab = dir.path().join("fstab");
    std::fs::write(&new_fstab, &stripped)?;
    run_host(&["sudo", "chmod", "-R", "a+rX", path_str(dir.path())?])?;
    note("Turning swap off and removing the swapfile...");
    run_host(&["sudo", "bash", path_str(&script)?, device, "/etc/fstab", path_str(&new_fstab)?])
        .context("cannot remove the swapfile")?;
    if !kargs.is_empty() {
        note("Removing the kernel arguments...");
        let mut argv = vec!["sudo".to_string(), "rpm-ostree".to_string()];
        argv.extend(kargs);
        run_host(&argv).context("cannot remove the resume kernel arguments")?;
    }
    let reboot = reboot_action();
    if json {
        println!(
            "{}",
            serde_json::json!({
                "ok": true, "changed": true, "enabled": false,
                "actions": [action_json(&reboot)],
            })
        );
        return Ok(());
    }
    println!("\nDone. This machine no longer has a swapfile to hibernate into.");
    print_actions(&[reboot]);
    Ok(())
}

/// Install kuma onto a disk. See `install` for why the account is the
/// hard part and why it arrives as an image layer.
///
/// Dry run by default, like every kuma verb that changes something, and
/// unlike every other one this change cannot be undone: there is no
/// staged deployment to discard and no rollback slot to return to. So
/// the plan is printed in full, the objections are checked before
/// anything is built, and `--yes` is the only thing that writes.
fn install(disk: Option<&Path>, request: install::Request) -> Result<()> {
    let install::Request {
        image: image_owned,
        default_image,
        update_from,
        user,
        groups,
        hostname,
        shell,
        encrypt: encrypt_flag,
        swap: swap_flag,
        restore,
        yes,
        json,
    } = request;
    let image = image_owned.as_str();
    // What the machine fetches updates from, which is the image being
    // installed unless told otherwise.
    let updates_owned = update_from.unwrap_or_else(|| image_owned.clone());
    let updates = updates_owned.as_str();
    if let Some(why) = install::unreachable_update_source(updates) {
        bail!("refusing to install: {why}");
    }
    // No --disk means ask, and asking means listing. A verb whose only
    // entry point is a device path is one a person cannot walk through:
    // on live media they would have to know about lsblk, know which of
    // two names is theirs, and get it right first time. That also makes
    // the affordance nameable, which matters more than it sounds: kuma's
    // whole shape is that a response names the next move, and
    // `kuma install --disk /dev/???` is not a move anyone can take.
    let chosen;
    // The picker already asked lsblk what is mounted where. Carrying its
    // answer to the objection check rather than asking again is not about
    // the second process: it means there is one account of whether a disk
    // is in use, and the guard on the only irreversible verb here cannot
    // disagree with the list somebody chose from.
    let (disk, known_mounts): (&Path, Option<String>) = match disk {
        Some(disk) => (disk, None),
        None => {
            let listing =
                host_output_any(&["lsblk", "-J", "-o", "NAME,SIZE,MODEL,TYPE,MOUNTPOINTS"])
                    .context("cannot list disks: pass --disk")?;
            let picked = install::choose_disk(install::disks_from_lsblk(&listing)?)?;
            let mounts = picked.mounts.join("\n");
            chosen = PathBuf::from(picked.path);
            (&chosen, Some(mounts))
        }
    };
    let disk_str = path_str(disk)?;
    // A regular file target means a disk image, which bootc writes
    // through a loopback device. Worth supporting rather than working
    // around: producing a disk image is a real thing to want, and it is
    // also the only way to exercise this verb end to end on a machine
    // with no spare disk. On live media the image being installed has to
    // land in a RAM-backed overlay first, which caps the whole path at
    // roughly 14 GB of memory; installing to a file from an ordinary
    // machine puts podman's storage on a real disk and lifts that.
    let to_file = disk.is_file();

    // Objections first: no point asking for a password before saying the
    // disk cannot be used. The picker refuses an in-use disk too, but
    // --disk skips the picker entirely, so this is the check that counts.
    let mounts = std::fs::read_to_string("/proc/self/mounts").unwrap_or_default();
    // Sees through LUKS and LVM, which /proc/mounts cannot. Failure is
    // tolerated rather than fatal: an absent lsblk leaves the mount-table
    // check doing the work, and refusing to install because a tool is
    // missing would be its own kind of wrong.
    let lsblk = match known_mounts {
        Some(mounts) => mounts,
        // A FILE cannot be asked of lsblk: it answers "not a block
        // device" and the guard that was supposed to refuse a mounted
        // disk image received an empty string and refused nothing. The
        // comment in disk_objections claimed that check still ran, and
        // its test passed only because it fed lsblk output by hand,
        // which the caller could never produce.
        //
        // losetup takes a file and names the loop devices backing it,
        // needs no privilege, and answers empty for the ordinary case.
        // Then the mount question is asked of those devices, which is a
        // question lsblk can answer.
        None if to_file => loop_backed_mountpoints(disk_str),
        None => host_output_any(&["lsblk", "-no", "MOUNTPOINTS", disk_str]).unwrap_or_default(),
    };
    let objections = install::disk_objections(disk_str, &mounts, &lsblk, to_file);
    if !objections.is_empty() {
        bail!(
            "refusing to install to {}:\n  {}\n\nUnmount it, or pick another disk.",
            disk.display(),
            objections.join("\n  ")
        );
    }
    if !disk.exists() {
        bail!("no such device: {}", disk.display());
    }

    // Asked here, ahead of the plan, because the answer changes the plan:
    // the layout printed below says what the root partition will hold,
    // and a plan printed before the question would be describing a disk
    // nobody had decided on yet. Only under --yes, since a dry run
    // changes nothing and has no business prompting.
    // Cheap and local: podman answers from storage without touching a
    // registry. Known before the question below, because the question is
    // the first thing typed and this decides whether an install can
    // happen at all.
    let local = host_output(&["podman", "image", "exists", image]).is_ok();

    // Before the interview, not after it.
    //
    // Installing pulls this image, and podman only discovers it is not
    // there when the build reaches out, which is several minutes and one
    // typed password later. What that looks like is `error creating build
    // container: unable to copy from source` and exit 125, after being
    // asked to choose an account. Half a second of skopeo turns that into
    // a sentence, before anything is asked — including the encryption
    // question, which used to come first and be wasted.
    //
    // Only for a real install: a dry run reaches out to no network to
    // describe itself. Skipped when the image is already local, and
    // skipped rather than fatal when skopeo is missing, because a check
    // that cannot run is not a reason to refuse an install that would
    // have worked.
    if yes && !local {
        note(&format!("Checking {image} is reachable..."));
        // --command-timeout, because the comment above promises "half a
        // second of skopeo" and an unbounded call delivers an indefinite
        // stall instead. This runs on live media, at the moment a
        // captive portal is most likely to be in the way, which is the
        // exact failure the check was written to prevent, one layer down.
        if let Err(why) = host_output(&[
            "skopeo",
            "inspect",
            "--command-timeout",
            "20s",
            "--raw",
            &format!("docker://{image}"),
        ]) {
            if host_output_any(&["skopeo", "--version"]).is_ok() {
                bail!(
                    "cannot reach {image}\n\n{why}\n\n\
                     Installing pulls that image, so it has to exist and be readable\n\
                     from here. A 403 or 404 usually means it is not published yet, or\n\
                     is private. Pass --image to install a different one."
                );
            }
        }
    }

    let encrypt = if yes { install::ask_encrypt(encrypt_flag)? } else { encrypt_flag };

    // With the objections, for the same reason they are: this is the
    // verb that cannot be undone, so everything that would stop it stops
    // it before anything is typed. `sgdisk` taught this the expensive
    // way, wiping a table and then failing with exit 127 one password
    // later. cryptsetup joins the list only for an encrypted install,
    // because refusing a plain one for want of it would be a check
    // inventing a requirement.
    let required: Vec<(&str, &str)> = partition::REQUIRED_TOOLS
        .iter()
        .chain(if encrypt { partition::ENCRYPT_TOOLS } else { &[] })
        .copied()
        .collect();
    let missing = partition::missing_tools(&required, partition::TOOL_DIRS);
    if !missing.is_empty() {
        bail!(
            "cannot install from this machine: {} missing\n  {}\n\n\
             Installing partitions and formats the target, and these are what does it.",
            if missing.len() == 1 { "a tool is" } else { "tools are" },
            missing.join("\n  ")
        );
    }

    // The layout is decided here, ahead of the interview, because a disk
    // too small to hold a system is an objection like a mounted one and
    // belongs with the others. It also has to be printed: it cannot be
    // changed afterwards without reinstalling.
    let disk_bytes = if to_file {
        std::fs::metadata(disk).map(|meta| meta.len()).context("cannot size the target file")?
    } else {
        host_output(&["lsblk", "-bndo", "SIZE", disk_str])
            .ok()
            .and_then(|text| text.trim().parse::<u64>().ok())
            .with_context(|| format!("cannot read the size of {}", disk.display()))?
    };
    let disk_mib = disk_bytes / (1024 * 1024);
    let layout = partition::plan(disk_bytes, encrypt)
        .with_context(|| format!("cannot install to {}", disk.display()))?;

    // After the layout, because what a swapfile can take is what the
    // layout has left over, and after the encryption question, because
    // the answer decides whether hibernating writes memory to this disk
    // in the clear and that is the thing worth saying before it happens.
    //
    // Memory is read from the machine running the installer, which on
    // live media is the machine being installed. Installing to a *file*
    // is a different case: the disk image is for some other machine, and
    // this one's memory is a fact about the wrong computer, so it does
    // not get to propose a number.
    let meminfo = (!to_file)
        .then(|| std::fs::read_to_string("/proc/meminfo").ok())
        .flatten()
        .unwrap_or_default();
    let ram_mib = hibernate::ram_mib(&meminfo);
    let spare_mib = partition::spare_mib(disk_mib);
    // The flag is parsed on every run, dry or not, so that `--swap 16`
    // is refused while it still costs nothing. Asked only under --yes,
    // like encryption: a dry run changes nothing and has no business
    // prompting.
    let swap_mib = if yes {
        install::ask_swap(
            swap_flag.as_deref(),
            ram_mib,
            hibernate::default_size_mib(&meminfo),
            spare_mib,
        )?
    } else {
        match swap_flag.as_deref() {
            Some(text) => {
                let asked = hibernate::parse_size(text)?;
                // The dry run has to tell the truth. Without this it
                // prints a layout for a swapfile that does not fit and
                // then hands over a `--yes` command that refuses it,
                // which is the same failure `confirm` exists to avoid:
                // an affordance that does not work is worse than none.
                if let Some(mib) = asked {
                    if let Some(why) =
                        hibernate::objections(mib, spare_mib, hibernate::SPARE_AT_INSTALL)
                            .into_iter()
                            .next()
                    {
                        bail!("{why}");
                    }
                }
                asked
            }
            None => None,
        }
    };
    let swap_view = swap_mib.map(|mib| {
        let mut warnings = hibernate::warnings(mib, ram_mib, encrypt);
        // Only for a real disk, and for the same reason memory is only
        // read for one: installing to a file makes an image for some
        // other machine, and this one's firmware settings are a fact
        // about the wrong computer. On live media it is the right
        // computer, and the answer carries: the installer boots through
        // the same shim and the same firmware as the machine it writes,
        // so a kernel locked down here will be locked down there.
        if !to_file {
            warnings.extend(lockdown_warning_here());
        }
        (mib, warnings)
    });

    // The dry run is a resource like every other read: state, facts, and
    // the one legal move out of it. An agent that follows affordances
    // will be handed `kuma install` on live media once there is an image
    // to install, so it has to be able to read the answer.
    // Named once. It is the string somebody copies to destroy a disk, so
    // two spellings of it is two chances to get one wrong.
    let confirm = {
        let mut flags = format!("--disk {}", disk.display());
        // Against what a bare run resolves to *here*, not against the
        // published image. On installer media built from a registry
        // reference, the default is that reference, and naming it would
        // add a flag that changes nothing; on media built from a local
        // image, the default is the published one and a `--image` the
        // person passed has to survive into the command they copy.
        if image != default_image {
            flags.push_str(&format!(" --image {image}"));
        }
        // Carried because without it the command this prints is one the
        // next run refuses: a local image with nowhere to update from is
        // exactly the case --update-from exists for, and an affordance
        // that does not work is worse than none.
        if updates != image {
            flags.push_str(&format!(" --update-from {updates}"));
        }
        // Carried only when it was given. Adding it to the command a dry
        // run prints would be answering a question on somebody's behalf,
        // and `--yes` asks it anyway.
        if encrypt_flag {
            flags.push_str(" --encrypt");
        }
        // Same rule as --encrypt: carried only when it was given, since
        // putting it in the printed command would answer a question on
        // somebody's behalf, and `--yes` asks it anyway.
        if let Some(swap) = &swap_flag {
            flags.push_str(&format!(" --swap {swap}"));
        }
        format!("kuma install {flags} --yes")
    };
    if json && !yes {
        let action = Action::new(
            "install",
            confirm.clone(),
            "ask for an account and hostname, then write it",
        );
        // Built before the document rather than inside it: json! takes
        // values, not blocks, and the list is conditional twice over.
        let mut asks = Vec::new();
        if encrypt {
            asks.push("disk passphrase");
        }
        if swap_flag.is_none() {
            asks.push("whether to create a swapfile for hibernate");
        }
        asks.extend(["account name", "password", "hostname"]);
        println!(
            "{}",
            serde_json::json!({
                "ok": true, "installed": false, "dry_run": true,
                "disk": disk_str, "image": image, "image_local": local,
                // What `--encrypt` would make it, not what a person would
                // be asked: an agent is not a terminal, so the flag is the
                // whole of the answer it can give.
                "encrypted": encrypt,
                // The size a --swap would make, not one a person would
                // be asked for: an agent is not a terminal, so the flag
                // is the whole of the answer it can give.
                "swap_mib": swap_mib,
                "asks": asks,
                // The one decision here that cannot be revised later, so
                // an agent reading this resource can see it before
                // agreeing to it rather than only in the prose plan.
                "layout": layout.iter().map(|part| serde_json::json!({
                    "label": part.label,
                    "size": part.size_text(disk_mib),
                    "purpose": part.purpose,
                })).collect::<Vec<_>>(),
                "actions": [action_json(&action)],
            })
        );
        return Ok(());
    }
    // Prose only for a person. `--json --yes` promised exactly one
    // document on stdout and then printed a plan above it.
    if !json {
        print_install_plan(&PlanView {
            disk,
            to_file,
            image,
            local,
            updates,
            encrypt,
            swap: swap_view.clone(),
            layout: &layout,
            disk_mib,
        });
    }
    if !yes {
        // Describe, do not rehearse. The interview belongs behind --yes,
        // so a dry run that asked for a name and a password it then threw
        // away would be theatre: it would look like an install right up
        // until it silently was not one. Saying what --yes asks for costs
        // three lines and leaves nobody surprised by a prompt.
        println!("\nNothing has been changed. Re-run with --yes and kuma will ask for:");
        if encrypt {
            println!("\n  a disk passphrase              typed at every boot to unlock the");
            println!("                                 root, and not recoverable if lost");
        } else {
            println!("\n  whether to encrypt the disk    a passphrase typed at every boot;");
            println!("                                 off unless you say so, and not a");
            println!("                                 thing that can be added afterwards");
            println!("                                 without installing again");
        }
        if swap_flag.is_none() {
            println!("  whether to hibernate           a swapfile the size of memory, on the");
            println!("                                 root; off unless you say so, and it can");
            println!("                                 be added later with kuma hibernate");
        }
        println!("  an account name and password   created on the first boot of the");
        println!("                                 installed machine, since a published");
        println!("                                 image declares no account");
        println!("  a hostname                     defaults to {}", install::DEFAULT_HOSTNAME);
        println!(
            "\nand then destroy {}. This is the one kuma verb with no way back:\n\
             no staged deployment to discard, no rollback slot.\n",
            disk.display()
        );
        print_actions(&[Action::new("install", confirm, "ask those, then write it")]);
        return Ok(());
    }

    // `--shell` beats what the image declares, and saying nothing leaves
    // the image's own answer standing: the converger sources the baked
    // /usr/lib/kuma/user first and the installer's file second, so an
    // omitted KUMA_SHELL is [system].shell surviving rather than a
    // default being applied. Not read here to decide anything: reading
    // it would mean running the image, running it would mean pulling
    // it, and on live media a pull lands in a RAM-backed overlay, which
    // is the ceiling this whole install path exists to get out from
    // under. An image already in local storage is read further down,
    // where running it costs nothing and what it declares is worth
    // warning about.
    // Before the account, so that a pipe driving this reads in the same
    // order the questions are asked: the passphrase decides the shape of
    // the disk, the account only what is on it.
    let passphrase = if encrypt { Some(install::ask_passphrase()?) } else { None };
    let account = install::ask_account(user, groups, shell)?;
    let hostname = install::ask_hostname(hostname)?;
    let dir = tempfile::tempdir().context("cannot create install directory")?;
    let user_file = dir.path().join("kuma-user");
    std::fs::write(&user_file, install::user_file(&account))?;
    restrict_to_owner(&user_file)?;
    std::fs::write(dir.path().join("kuma-hostname"), format!("{hostname}\n"))?;
    // The restore file is read and checked here, before anything is
    // written to a disk. A restore that cannot work is worth finding out
    // about while the old machine's data is still the only copy.
    if let Some(path) = &restore {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("cannot read the restore file {}", path.display()))?;
        if let Err(why) = install::restore_file_is_usable(&text) {
            bail!("refusing to install: {why}");
        }
        let secret = dir.path().join("kuma-restore-secret");
        std::fs::write(&secret, &text)?;
        restrict_to_owner(&secret)?;
        std::fs::write(dir.path().join("kuma-restore-request"), "requested by kuma install\n")?;
    }
    std::fs::write(
        dir.path().join("Containerfile"),
        install::install_containerfile(image, &account, restore.is_some()),
    )?;

    // Said before anything is written, and only when it can be known
    // for free: reading the baked declaration means running the image,
    // and on live media a pull lands in RAM, which is the ceiling this
    // install path exists to get out from under. A local image is
    // already here, and a local image is exactly the case this warns
    // about, since a published one declares no user at all.
    // Both halves of "the image is already here", in the order they
    // matter: say what installing it carries before spending minutes
    // copying it anywhere.
    //
    // Reading the baked declaration runs the image. That is no new
    // trust: this is the image about to become the machine. It is
    // guarded on `local` because on live media the same read would mean
    // a pull into RAM.
    if local {
        if let Ok(baked) =
            host_output(&["podman", "run", "--rm", image, "cat", "/usr/lib/kuma/kuma.toml"])
        {
            if let Some(warning) = install::baked_user_warning(&baked, &account.name) {
                // note(), not println!(): in JSON mode this belongs on
                // stderr with the rest of the progress, not inside the
                // one document stdout promised.
                note(&format!("\n{warning}\n"));
            }
        }
        // The install script runs as root, and root's podman is a
        // different store than the one that built this image. Everything
        // else that hands an image to a root-side tool syncs it first
        // (switch for bootc, vm and iso for bootc-image-builder); this
        // did not. So a local image was found only when some earlier
        // `kuma vm` had left a copy in root's storage: where that copy
        // existed it got installed *instead* of the image just built,
        // and where it did not the build fell through to
        // `docker://localhost/...` and a refused connection.
        let scratch = tempfile::tempdir().context("cannot create scratch directory")?;
        sync_image_to_root(image, scratch.path())?;
    }
    let script = dir.path().join("install");
    std::fs::write(&script, partition::install_script(&layout, encrypt, swap_mib))?;

    // The script runs as root and reads this directory, and a tempdir is
    // 0700 for whoever created it.
    //
    // The credentials are SKIPPED rather than widened and put back.
    // This was `chmod -R a+rX` over everything followed by two `chmod
    // 600` calls, which made the account's password hash and the backup
    // repository password readable by every local account on the machine
    // for the two process spawns in between, and longer on a sudo
    // configuration that re-prompts. The comment that shipped with it
    // already named the right answer: root, which is what actually reads
    // these, needs no permission at all. So they never get any.
    //
    // `-prune` rather than a second pass, so there is no window at all
    // rather than a smaller one. The disk passphrase was never here: it
    // goes to the script on stdin, because `ps` shows argv.
    let mut credentials = vec![user_file.clone()];
    if restore.is_some() {
        credentials.push(dir.path().join("kuma-restore-secret"));
    }
    let mut widen = vec![
        "sudo".to_string(),
        "find".to_string(),
        path_str(dir.path())?.to_string(),
        "(".to_string(),
    ];
    for (i, credential) in credentials.iter().enumerate() {
        if i > 0 {
            widen.push("-o".to_string());
        }
        widen.push("-path".to_string());
        widen.push(path_str(credential)?.to_string());
    }
    widen.extend(
        [")", "-prune", "-o", "-exec", "chmod", "a+rX", "{}", "+"].iter().map(|s| s.to_string()),
    );
    run_host(&widen)?;

    // Read before, compared after. Installing to a file leaves a boot
    // entry in this machine's firmware naming a partition inside that
    // file, and the only way to know which entry is to know which ones
    // were there first. Readable without root, and skipped entirely when
    // the target is a real disk, where the entry is the point.
    let efi_before = if to_file { host_output(&["efibootmgr"]).ok() } else { None };

    note("Partitioning, formatting and installing (this destroys the target)...");
    let argv = ["sudo", "bash", path_str(&script)?, disk_str, path_str(dir.path())?, updates];
    match &passphrase {
        Some(passphrase) => run_host_stdin(&argv, &format!("{passphrase}\n"))?,
        None => run_host(&argv)?,
    }

    let reboot = Action::new(
        "reboot",
        "sudo systemctl reboot",
        "boot the installed machine (remove the install media first)",
    );
    if json {
        println!(
            "{}",
            serde_json::json!({
                "ok": true, "installed": true, "disk": disk_str, "image": image,
                "user": account.name, "hostname": hostname, "encrypted": encrypt,
                "actions": [action_json(&reboot)],
            })
        );
        return Ok(());
    }
    println!("\nInstalled {image} to {}.", disk.display());
    println!(
        "The account '{}' is created on first boot by kuma-user-sync, from\n\
         /var/lib/kuma/user written onto the target. The hostname is '{hostname}'.",
        account.name
    );
    if encrypt {
        println!(
            "\nThe root is a LUKS volume. It asks for that passphrase at every boot,\n\
             before the desktop and before anything can log in. Nothing here kept a\n\
             copy of it, and a lost one is a lost disk."
        );
    }
    if to_file {
        println!(
            "\nBoot it with UEFI firmware, which a disk image needs and a plain\n\
             qemu invocation does not supply, and with a 3D-capable device,\n\
             without which the desktop logs in to a black screen:\n\n{}",
            disk_image_boot_hint(disk)
        );
        // Named rather than removed. Deleting a firmware entry on
        // somebody's behalf is a larger liberty than leaving one they
        // can see, and the number is the whole of the difficulty.
        let added = match (efi_before, host_output(&["efibootmgr"]).ok()) {
            (Some(before), Some(after)) => install::new_efi_entries(&before, &after),
            _ => Vec::new(),
        };
        if !added.is_empty() {
            println!(
                "\nInstalling added {} to this machine's firmware, naming the ESP\n\
                 inside the image. It points at a partition no firmware can find,\n\
                 and it sorts ahead of the entries that can boot. Remove it with:\n",
                if added.len() == 1 { "a boot entry" } else { "boot entries" }
            );
            for number in &added {
                println!("  sudo efibootmgr -b {number} -B");
            }
        }
        return Ok(());
    }
    print_actions(&[reboot]);
    Ok(())
}

/// The container the ISO is assembled inside. Fedora rather than the
/// kuma image itself: mksquashfs and xorriso are build tools that no
/// kuma machine should carry, and installing them into a throwaway
/// container keeps them off both the host and the media.
const ISO_BUILD_IMAGE: &str = "registry.fedoraproject.org/fedora:44";

/// The tag the live root filesystem is built under. Distinct from the
/// image being shipped, and disposable: it exists between the two
/// podman calls below and is worth nothing afterwards.
const LIVE_TAG: &str = "localhost/kuma-live:latest";

/// Build live installer media: the image is its own installer
/// environment, so the ISO carries one root filesystem instead of two.
/// See `liveiso` for why this is assembled here rather than by
/// bootc-image-builder.
///
/// Unlike the Anaconda path this needs no root. bib runs as root and
/// reads root's containers-storage; everything here is podman doing
/// what podman does rootless, which is worth preserving: build media
/// that needs a password is one more reason not to build it.
fn live_iso(config_path: &Path, tag: &str, output: &Path) -> Result<()> {
    let config = Config::load(config_path)?;
    if host_output(&["podman", "image", "exists", tag]).is_err() {
        bail!("no image {tag}. Build it first:\n\n  kuma build\n");
    }
    std::fs::create_dir_all(output)
        .with_context(|| format!("cannot create {}", output.display()))?;
    let output = std::fs::canonicalize(output)?;

    let dir = tempfile::tempdir().context("cannot create ISO build directory")?;
    let containerfile = dir.path().join("Containerfile");
    std::fs::write(&containerfile, liveiso::live_containerfile(&config, tag))?;
    let script = dir.path().join("build-iso");
    std::fs::write(&script, liveiso::BUILD_ISO_SCRIPT)?;
    std::fs::write(dir.path().join("live-hostname"), format!("{}\n", liveiso::LIVE_HOSTNAME))?;
    std::fs::write(dir.path().join("live-storage.conf"), liveiso::LIVE_STORAGE_CONF)?;
    // The running binary, for the same reason `build` stages it: the
    // live layer writes a marker only a kuma new enough to read it can
    // act on, so the two have to travel together.
    let self_exe = std::env::current_exe().context("cannot locate the running kuma binary")?;
    std::fs::copy(&self_exe, dir.path().join("kuma"))
        .with_context(|| format!("staging {} into the build context", self_exe.display()))?;

    note("Building the live root filesystem (kuma plus a live boot's dracut modules)...");
    run_host(&[
        "podman",
        "build",
        "-t",
        LIVE_TAG,
        "-f",
        path_str(&containerfile)?,
        path_str(dir.path())?,
    ])?;

    // The live image is mounted rather than exported: podman assembles
    // the merged filesystem itself, so nothing here unpacks 3 GB into a
    // temp directory first.
    let script_mount = format!("{}:/src/build-iso:ro", path_str(&script)?);
    let rootfs_mount = format!("type=image,source={LIVE_TAG},dst=/rootfs");
    let out_mount = format!("{}:/output", path_str(&output)?);
    note("Assembling the ISO (squashing the root filesystem takes a while)...");
    run_host(&[
        "podman",
        "run",
        "--rm",
        "--security-opt",
        "label=disable",
        "-v",
        &script_mount,
        "--mount",
        &rootfs_mount,
        "-v",
        &out_mount,
        ISO_BUILD_IMAGE,
        "/usr/bin/bash",
        "/src/build-iso",
        liveiso::ISO_LABEL,
    ])?;

    let iso_path = output.join(format!("{}.iso", liveiso::ISO_LABEL));
    let size = std::fs::metadata(&iso_path).map(|m| m.len()).unwrap_or(0);
    println!("\nISO ready: {} ({:.2} GB)", iso_path.display(), size as f64 / 1e9);
    println!(
        "It boots to a live {} session as '{}', and `kuma install` from inside it\nwrites a machine to a disk. Installing pulls the published image over the\nnetwork rather than copying this media, so the ISO carries one system\nrather than two.",
        match config.system.desktop {
            config::Desktop::Cosmic => "COSMIC",
            config::Desktop::Niri => "niri",
            config::Desktop::None => "console",
        },
        liveiso::LIVE_USER
    );
    println!(
        "Write it to a USB stick with e.g. `sudo dd if={} of=/dev/sdX bs=4M status=progress`.",
        iso_path.display()
    );
    Ok(())
}

/// bootc-image-builder runs as root and reads root's containers-storage.
/// Sync by image ID, not tag existence: the root-side copy goes stale
/// every time the rootless image is rebuilt. Returns the image ID.
fn sync_image_to_root(tag: &str, scratch: &Path) -> Result<String> {
    let local_id =
        image_id(tag).with_context(|| format!("{tag} not found; run `kuma build` first"))?;
    let root_id = host_output(&["sudo", "podman", "image", "inspect", "--format", "{{.Id}}", tag])
        .unwrap_or_default();
    if local_id != root_id {
        note(&format!("Syncing {tag} into root podman storage (may take a minute)..."));
        // Piped rather than staged through a file, which `vm_apply`
        // already does for the same job over ssh.
        //
        // Measured on a 1.55 GB image: `podman save` alone streams for
        // 40 s, and `podman load` cannot start until it finishes, so the
        // two passes were strictly sequential. The archive also landed
        // in a tempdir, which is /tmp, which is tmpfs: 1.55 GB of RAM
        // held for the duration. On live media that is the RAM overlay,
        // the same ceiling the ISO work already had to get out from
        // under once.
        //
        // `set -o pipefail` is load-bearing: without it a failed save is
        // masked by a load that succeeded at reading nothing.
        run_host(&[
            "bash",
            "-c",
            &format!("set -o pipefail; podman save --format oci-archive {tag} | sudo podman load"),
        ])?;
        let _ = scratch;
    }
    Ok(local_id)
}

/// Mount points where a desktop automounter has grabbed a previous disk
/// build's partitions.
///
/// bib partitions a loop device, and udisks2 mounts anything with a
/// filesystem on it under `/run/media/<user>`. Those mounts then hold
/// the loop device open, so bib cannot detach it when it finishes and
/// the backing file is left deleted-but-attached.
///
/// That used to poison every later build. osbuild pins filesystem UUIDs
/// in its manifest so builds reproduce, which means the next disk from
/// the same declaration carries the same UUID, and XFS refuses outright
/// to mount a UUID that is already mounted: "Filesystem has duplicate
/// UUID ... - can't mount", forty lines into a Python traceback. One
/// automount was enough to make every subsequent `kuma vm` fail, and
/// each failure left another pair of mounts behind.
///
/// ext4 does not enforce that uniqueness, so the build survives now.
/// The mounts still accumulate and still pin loop devices, which is
/// worth saying out loud rather than leaving for whoever eventually
/// wonders where their loop devices went.
fn automounted_loop_mounts() -> Vec<String> {
    let Ok(mountinfo) = std::fs::read_to_string("/proc/self/mountinfo") else {
        return Vec::new();
    };
    loop_mounts_in(&mountinfo)
}

/// The parse, split from the read so it is testable. On a machine that
/// has never hit this the function above would otherwise ship having
/// never matched a line, which is the same reason `scan_etc` takes its
/// roots as parameters.
fn loop_mounts_in(mountinfo: &str) -> Vec<String> {
    mountinfo
        .lines()
        .filter_map(|line| {
            // "<id> <parent> <maj:min> <root> <target> <opts> ... - <fstype> <source> <superopts>"
            let (left, right) = line.split_once(" - ")?;
            let target = left.split_whitespace().nth(4)?;
            let source = right.split_whitespace().nth(1)?;
            (target.starts_with("/run/media/") && source.starts_with("/dev/loop"))
                .then(|| target.to_string())
        })
        .collect()
}

fn run_bib(
    output: &Path,
    bib_config: &Path,
    image_type: &str,
    tag: &str,
    extra_mounts: &[String],
) -> Result<()> {
    // A warning rather than a refusal: on ext4 the build works anyway,
    // and blocking a build that would have succeeded is the worse error.
    let stray = automounted_loop_mounts();
    if !stray.is_empty() {
        note(&format!(
            "WARNING: your desktop has automounted a previous disk build:\n\n  {}\n\n\
             Those mounts pin the loop devices they sit on. To clear them:\n\n  \
             sudo umount {}\n  sudo losetup -D\n",
            stray.join("\n  "),
            stray.join(" ")
        ));
    }
    let out_mount = format!("{}:/output", path_str(output)?);
    let config_mount = format!("{}:/config.toml:ro", path_str(bib_config)?);
    let mut args = vec![
        "sudo",
        "podman",
        "run",
        "--rm",
        "--privileged",
        "--security-opt",
        "label=type:unconfined_t",
        "-v",
        &out_mount,
        "-v",
        "/var/lib/containers/storage:/var/lib/containers/storage",
        "-v",
        &config_mount,
    ];
    for mount in extra_mounts {
        args.extend(["-v", mount.as_str()]);
    }
    args.extend([BIB_IMAGE, "--type", image_type, "--rootfs", BIB_ROOTFS, tag]);
    run_host(&args)?;
    // bib ran as root, so its output is root-owned; hand it back to the
    // user so QEMU (and cleanup) work without privileges.
    let user = std::env::var("USER").context("USER is not set")?;
    run_host(&["sudo", "chown", "-R", &format!("{user}:"), path_str(output)?])
}

const VM_SSH_OPTS: &[&str] = &[
    // the VM's host key changes with every rebuilt disk
    "-o",
    "StrictHostKeyChecking=no",
    "-o",
    "UserKnownHostsFile=/dev/null",
    "-o",
    "BatchMode=yes",
    "-o",
    "LogLevel=ERROR",
];

/// The dev-loop update: stream the built image into the running VM and
/// `bootc switch` inside it. Unlike a disk rebuild this keeps /var —
/// flatpaks, brew, homes — and exercises the real update path (staged
/// deployment, rollback) instead of the install path.
fn vm_apply(tag: &str) -> Result<()> {
    image_id(tag).with_context(|| format!("{tag} not found; run `kuma build` first"))?;

    let mut probe = vec!["ssh", "-p", "2222", "-o", "ConnectTimeout=4"];
    probe.extend(VM_SSH_OPTS);
    probe.extend(["kuma@localhost", "true"]);
    host_output(&probe)
        .context("no running VM reachable on port 2222; boot one with `kuma vm` first")?;

    // Stream straight into the guest's root podman storage: no archive
    // file on the guest and no untar temp copy — the 10G disk ran out of
    // space holding three copies of the image at once with oci-archive.
    println!("Streaming image into the VM...");
    let ssh_opts = VM_SSH_OPTS.join(" ");
    // The password must arrive out-of-band (askpass): `echo kuma | sudo -S`
    // would make the password pipe podman's stdin and starve it of the
    // image stream coming over ssh.
    let remote_load = r##"f=$(mktemp); printf "#!/bin/sh\necho kuma\n" > "$f"; chmod 700 "$f"; SUDO_ASKPASS="$f" sudo -A podman load; rc=$?; rm -f "$f"; exit $rc"##;
    run_host(&[
        "sh",
        "-c",
        &format!("podman save {tag} | ssh -p 2222 {ssh_opts} kuma@localhost '{remote_load}'"),
    ])?;

    println!("Switching the VM to the new image (staged; applies on reboot)...");
    let mut switch = vec!["ssh", "-p", "2222"];
    switch.extend(VM_SSH_OPTS);
    // switch is a no-op when the origin spec is unchanged (every apply
    // after the first!) — bootc upgrade is what re-pulls the origin and
    // stages new content. Then verify something IS staged: without the
    // check a no-op apply reboots into the same deployment looking like
    // success. rmi after: the ostree import is self-contained and the
    // podman copy is dead weight.
    let switch_cmd = format!(
        "echo kuma | sudo -S sh -c 'bootc switch --transport containers-storage {tag} && bootc upgrade; podman rmi -f {tag} >/dev/null; bootc status | grep -qiE \"^  Staged|staged image\" || {{ echo \"kuma: nothing staged; the VM already runs this image\" >&2; exit 3; }}'"
    );
    switch.extend(["kuma@localhost", &switch_cmd]);
    run_host(&switch)?;

    println!("Rebooting the VM into it...");
    let mut reboot = vec!["ssh", "-p", "2222"];
    reboot.extend(VM_SSH_OPTS);
    reboot.extend(["kuma@localhost", "echo kuma | sudo -S systemctl reboot"]);
    // the connection may drop as the VM goes down; that's success
    let _ = run_host(&reboot);
    println!("Done. /var (flatpaks, brew, homes) is untouched; `bootc rollback` inside the VM undoes this.");
    Ok(())
}

/// Disk-image config: a login user so the VM is actually reachable.
/// Password login on the console, plus the user's ssh key when one exists.
/// The VM mirrors the host's timezone — timezone is machine state, not
/// system definition, so it's detected here rather than put in kuma.toml.
/// bib config for the installer ISO. The install stays interactive —
/// language, keyboard, and destination disk are the machine owner's
/// call — but everything kuma already speaks for is preseeded away.
fn iso_config_toml(config: &Config) -> String {
    let mut ks = String::new();
    if config.system.hostname.is_none() {
        // Anaconda writes /etc/hostname; left empty, the initrd's
        // "localhost" beats os-release DEFAULT_HOSTNAME (same story the
        // VM disks hit) — stamp the brand default at install time.
        ks.push_str("network --hostname=kuma\n");
    }
    // kuma images ship no initial-setup; don't let installs wait on one
    ks.push_str("firstboot --disable\n");
    let mut out = format!("[customizations.installer.kickstart]\ncontents = \"\"\"\n{ks}\"\"\"\n");
    if config.user.is_some() {
        // kuma-user-sync creates the declared account on first boot, so
        // Anaconda's user screen would only mint a duplicate. Dropping
        // the module removes the screen and its create-a-user completion
        // requirement in one move.
        out.push_str(
            "\n[customizations.installer.modules]\ndisable = [\"org.fedoraproject.Anaconda.Modules.Users\"]\n",
        );
    }
    out
}

/// The shell the image says accounts on this machine should get.
///
/// Read from the image rather than from the declaration on disk, because
/// the disk is built from an image and `--tag` can name one this working
/// directory did not produce. Asked of a container rather than parsed
/// out of a file kuma might not have: `sync_image_to_root` has already
/// run podman against this tag by the time we get here, so it costs
/// nothing new.
///
/// Every failure is None. A tag that is not a kuma image, an image from
/// before this file existed, a declaration that declares no shell: all
/// of them mean "say nothing to bib", which is what kuma did before.
fn image_shell(tag: &str) -> Option<String> {
    let out = std::process::Command::new("podman")
        .args(["run", "--rm", "--entrypoint", "", tag, "cat", "/usr/lib/kuma/kuma.toml"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let parsed: Config = toml::from_str(&String::from_utf8(out.stdout).ok()?).ok()?;
    // Validated, because `Config::load` is what normally does that and
    // this does not go through it. `--tag` can name an image kuma did
    // not build, so a declaration read here has been through no build
    // of kuma's and no check of kuma's.
    parsed.validate().ok()?;
    parsed.system.shell
}

/// `pubkey` and `shell` are both optional and both omitted entirely when
/// absent, never emitted empty.
fn bib_config_toml(pubkey: Option<&str>, shell: Option<&str>) -> String {
    // Only what bib actually supports for qcow2. It rejects anything else
    // with "blueprint validation failed for image type qcow2: <key>: not
    // supported" and then builds the disk regardless, so an unsupported
    // key does nothing but print an alarming line into every VM build,
    // and it reports one key at a time, so they hid behind each other.
    //
    // Both keys that used to be here already had working replacements
    // elsewhere, which is why nobody noticed they were inert:
    //   hostname  the image writes /etc/hostname itself, which is what
    //             beats the initrd's early hostname (the ISO is a
    //             different path and keeps its kickstart `network
    //             --hostname=`, which Anaconda does honor).
    //   timezone  boot_disk passes it through -fw_cfg and
    //             kuma-vm-timezone adopts it at boot, which exists
    //             precisely because bib ignores the blueprint key.
    let mut out = String::from(
        "[customizations]\n\n[[customizations.user]]\nname = \"kuma\"\npassword = \"kuma\"\ngroups = [\"wheel\"]\n",
    );
    if let Some(key) = pubkey {
        // Escaped rather than interpolated: a public key's trailing
        // comment is free text from whenever it was generated, and one
        // quote in it would otherwise produce a blueprint bib cannot
        // parse.
        let value = toml::Value::String(key.trim().to_string());
        out.push_str(&format!("key = {value}\n"));
    }
    // What the image says accounts on it should get. `[system].shell`
    // exists precisely for the image that declares no [user] — which is
    // every published one and every committed example — so a disk built
    // from a declaration saying `shell = "fish"` handing back a bash
    // login is the one case the field was invented to cover, failing.
    //
    // `kuma install` already honors it, through the converger sourcing
    // the baked /usr/lib/kuma/user. The bib-made convenience account
    // never went near that path, so it needs telling directly.
    //
    // The name is validated by `image_shell`, which is where that has to
    // happen: `--tag` can name an image kuma did not build, so "the
    // build already ran `test -x /usr/bin/<shell>`" is true of kuma's
    // own images and of nothing else.
    if let Some(shell) = shell {
        let value = toml::Value::String(format!("/usr/bin/{shell}"));
        out.push_str(&format!("shell = {value}\n"));
    }
    // Headroom for `kuma vm --apply`: image updates transiently need a few
    // GB in the guest. Sparse qcow2, so the host pays nothing up front.
    out.push_str("\n[[customizations.filesystem]]\nmountpoint = \"/\"\nminsize = \"20 GiB\"\n");
    out
}

/// The host's IANA timezone, from the /etc/localtime symlink. None when
/// the link is absent (host on UTC) or oddly shaped — the guest then just
/// stays on UTC, which is also what a wrong guess would deserve.
fn host_timezone() -> Option<String> {
    let target = std::fs::read_link("/etc/localtime").ok()?;
    let tz = target.to_str()?.rsplit_once("zoneinfo/")?.1.to_string();
    let ok = |c: char| c.is_ascii_alphanumeric() || "/_+-".contains(c);
    (!tz.is_empty() && tz.chars().all(ok)).then_some(tz)
}

fn find_ssh_pubkey() -> Option<String> {
    let home = std::env::var_os("HOME")?;
    for name in ["id_ed25519.pub", "id_rsa.pub", "id_ecdsa.pub"] {
        let path = Path::new(&home).join(".ssh").join(name);
        if let Ok(key) = std::fs::read_to_string(&path) {
            return Some(key);
        }
    }
    None
}

/// The public key the VM should trust: the host's own when it has one,
/// otherwise a throwaway written beside the disk.
///
/// Without the fallback, a host with no key gets a VM reachable only by
/// password, and nothing says so. That is survivable when a human is
/// typing, and it is not survivable in `scripts/smoke.sh --boot`, which
/// calls ssh dozens of times with stderr discarded: every call stops on
/// a password prompt, so whether the boot stage is interactive depends
/// on whether the person running it happens to own an ssh key.
///
/// A key already in the output directory is reused rather than
/// regenerated. The disk beside it already trusts that one, and a fresh
/// pair would lock out every VM a previous run built here.
fn vm_ssh_key(output: &Path) -> Option<String> {
    if let Some(key) = find_ssh_pubkey() {
        return Some(key);
    }
    let private = output.join("ssh-key");
    let public = output.join("ssh-key.pub");
    if !private.exists() {
        note("No ssh key in ~/.ssh; generating a throwaway one for this VM...");
        // Captured, not run: ssh-keygen prints a fingerprint and a block
        // of randomart nobody asked for. A missing ssh-keygen is not
        // worth failing the build over, since the console password still
        // works; the launch message says which way in is available.
        host_output(&[
            "ssh-keygen",
            "-t",
            "ed25519",
            "-N",
            "",
            "-C",
            "kuma vm throwaway",
            "-f",
            path_str(&private).ok()?,
        ])
        .ok()?;
    }
    std::fs::read_to_string(&public).ok()
}

/// The ssh half of the launch message. Derived from what is on disk
/// rather than from what this run did, so a reused VM directory
/// describes itself as accurately as a freshly built one.
fn vm_ssh_hint(output: &Path) -> String {
    // Asked in the same order vm_ssh_key injects, or the two disagree:
    // a throwaway left by an earlier run would otherwise be advertised
    // for a disk that trusts the host key this run just used.
    let key = output.join("ssh-key");
    if find_ssh_pubkey().is_some() {
        "ssh: `ssh -p 2222 kuma@localhost`, using your ~/.ssh key".to_string()
    } else if key.exists() {
        format!("ssh: `ssh -p 2222 -i {} kuma@localhost`, throwaway key", key.display())
    } else {
        "ssh: console only, no key available".to_string()
    }
}

/// What a kuma desktop needs from qemu to render anything at all.
///
/// niri allocates through GBM and needs a 3D-capable device. Plain
/// `-vga std` or `-vga virtio` is display-only, and the failure is
/// quiet in the worst way: the greeter is text on a VT and comes up
/// fine, then the session that follows it renders nowhere. What that
/// looks like is a correct username and password followed by a black
/// screen, which reads like a broken install and is not one.
const VM_GPU_ARGS: [&str; 4] = ["-device", "virtio-vga-gl", "-display", "gtk,gl=on"];

/// Renders virgl on llvmpipe, so guest GL work never reaches the host
/// GPU driver. A bad guest submission can otherwise wedge the real GPU
/// and take the host session down with it.
const VM_GPU_ENV: &str = "LIBGL_ALWAYS_SOFTWARE=1";

/// How to boot a disk image kuma installed, for somebody who has one and
/// no VM manager in front of them.
///
/// Two things a plain qemu line does not supply and this one does: UEFI
/// firmware, without which the disk does not boot at all, and a
/// 3D-capable device, without which it boots to a black screen after the
/// login. Both were learned the same way, by booting one.
fn disk_image_boot_hint(disk: &Path) -> String {
    format!(
        "  cp /usr/share/edk2/ovmf/OVMF_VARS.fd /var/tmp/kuma-vars.fd\n  \
         env {VM_GPU_ENV} qemu-system-x86_64 -machine q35,accel=kvm -cpu host -m 4096 \\\n    \
         -drive if=pflash,format=raw,readonly=on,file=/usr/share/edk2/ovmf/OVMF_CODE.fd \\\n    \
         -drive if=pflash,format=raw,file=/var/tmp/kuma-vars.fd \\\n    \
         -drive file={},format=raw,if=virtio \\\n    \
         {}",
        disk.display(),
        VM_GPU_ARGS.join(" ")
    )
}

fn boot_disk(disk: &Path, output: &Path) -> Result<()> {
    println!("Booting VM (console: kuma/kuma; {})...", vm_ssh_hint(output));
    let drive = format!("file={},if=virtio", path_str(disk)?);
    let mut args: Vec<&str> = vec![
        // LIBGL_ALWAYS_SOFTWARE: render virgl on llvmpipe so guest GL work
        // never reaches the host GPU driver — a bad guest submission can
        // otherwise wedge the real GPU and take the host session down.
        "env",
        VM_GPU_ENV,
        "qemu-system-x86_64",
        "-enable-kvm",
        "-cpu",
        "host",
        "-smp",
        "4",
        "-m",
        "4096",
        "-drive",
        &drive,
        // 127.0.0.1 bind: without it qemu listens on every interface and
        // the whole LAN can ssh into the default-credential test user.
        "-nic",
        "user,model=virtio-net-pci,hostfwd=tcp:127.0.0.1:2222-:22",
        // Host<->guest clipboard: qemu speaks the spice vdagent protocol
        // itself (no SPICE server needed); the guest's spice-vdagent picks
        // it up on the virtio-serial port named com.redhat.spice.0.
        "-device",
        "virtio-serial-pci",
        "-chardev",
        "qemu-vdagent,id=vdagent,name=vdagent,clipboard=on",
        "-device",
        "virtserialport,chardev=vdagent,name=com.redhat.spice.0",
    ];
    // Named once, because `kuma install` has to tell somebody the same
    // thing in prose and a VM that renders nothing is indistinguishable
    // from one that did not boot.
    args.extend(VM_GPU_ARGS);
    // Mirror the host timezone into the guest (adopted at boot by
    // kuma-vm-timezone; bib ignores [customizations.timezone] for qcow2).
    let fw_cfg = host_timezone().map(|tz| format!("name=opt/org.kuma.tz,string={tz}"));
    if let Some(fw_cfg) = &fw_cfg {
        args.extend(["-fw_cfg", fw_cfg]);
    }
    run_host(&args)
}

fn path_str(path: &Path) -> Result<&str> {
    path.to_str().context("non-UTF-8 path")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Doctor prints `kuma sync` as the fix for a failed converger, and
    /// a converger fails often enough to spend StartLimitBurst, after
    /// which systemd refuses to start it at all. If the reset does not
    /// come first, the fix the tool prescribes cannot run in the state
    /// the tool prescribes it for.
    #[test]
    fn sync_clears_a_spent_start_limit_before_starting() {
        let calls = convergence_calls(&["kuma-flatpak-sync.service", "kuma-brew-sync.service"]);
        let verbs: Vec<&String> = calls.iter().map(|(call, _)| &call[2]).collect();
        assert_eq!(verbs, ["reset-failed", "start"], "reset-failed must precede start");
        for (call, must_succeed) in &calls {
            assert_eq!(call[0], "sudo");
            assert!(
                call.contains(&"kuma-flatpak-sync.service".to_string())
                    && call.contains(&"kuma-brew-sync.service".to_string()),
                "both calls name every unit being converged"
            );
            // A healthy unit has nothing to reset, so requiring the
            // reset to succeed would fail syncs that had no problem.
            assert_eq!(*must_succeed, call[2] == "start");
        }
    }

    /// A disk image that boots to a black screen after a correct
    /// password is indistinguishable from a broken install, and the
    /// only thing standing between somebody and that afternoon is this
    /// line of text. It has to carry both of the things a plain qemu
    /// invocation does not supply.
    #[test]
    fn the_boot_hint_supplies_firmware_and_a_gpu() {
        let hint = disk_image_boot_hint(Path::new("/var/tmp/kuma-target.raw"));
        assert!(hint.contains("OVMF_CODE.fd"), "no firmware, no boot at all");
        assert!(hint.contains("OVMF_VARS.fd"));
        assert!(hint.contains("/var/tmp/kuma-target.raw"));
        // The half that took a black screen to find.
        for arg in VM_GPU_ARGS {
            assert!(hint.contains(arg), "missing {arg}");
        }
        assert!(hint.contains(VM_GPU_ENV), "guest GL would reach the host GPU");
    }

    /// The deletion policy for composed bases, without a podman: only
    /// tags shaped like kuma's own content tags go, and never one the
    /// declaration or its lock still points at.
    #[test]
    fn stale_base_tags_spare_the_live_and_the_hand_named() {
        let listed = "localhost/kuma-base:maaaaaaaaaaaa\n\
                      localhost/kuma-base:mbbbbbbbbbbbb\n\
                      localhost/kuma-base:spike3\n\
                      localhost/kuma:latest\n";
        let keep = vec!["localhost/kuma-base:maaaaaaaaaaaa".to_string()];
        assert_eq!(
            super::stale_base_tags(listed, &keep),
            vec!["localhost/kuma-base:mbbbbbbbbbbbb".to_string()]
        );
        // an empty live set (no lock, fresh declaration) still only
        // touches content-shaped tags
        assert_eq!(super::stale_base_tags(listed, &[]).len(), 2);
    }

    /// clap's own verifier over the whole CLI: conflicts naming arguments
    /// that don't exist, groups over missing members, duplicate flags,
    /// and the rest. Cheap, and it covers every verb at once rather than
    /// the one somebody remembered to test.
    #[test]
    fn the_cli_definition_is_coherent() {
        use clap::CommandFactory;
        super::Cli::command().debug_assert();
    }

    /// Every command the docs hand somebody has to parse against the CLI
    /// that exists.
    ///
    /// Living with the machine proves the CLI works and cannot prove the
    /// docs still describe it: a renamed flag is noticed the moment it is
    /// typed, and the page saying the old name is a page its author never
    /// reads again. Both times this project shipped stale documentation
    /// it was found by somebody reading the file on purpose, which is not
    /// a mechanism.
    ///
    /// This asserts the words parse, not that the command works. What the
    /// command does is proven by CI where CI can reach it and by use
    /// where it cannot, and neither of those ever looks at the docs.
    #[test]
    fn every_documented_command_parses_against_this_cli() {
        use clap::Parser;
        let mut checked = 0;
        for doc in ["docs/getting-started.md", "README.md"] {
            for cmd in crate::config::documented_commands(doc) {
                let mut words = cmd.split_whitespace();
                // The docs also give curl, dd, chmod and cargo, which are
                // not kuma's to validate.
                if words.next() != Some("kuma") {
                    continue;
                }
                let argv: Vec<&str> = std::iter::once("kuma").chain(words).collect();
                if let Err(e) = super::Cli::try_parse_from(&argv) {
                    panic!("{doc} tells somebody to run `{cmd}`, which this kuma rejects:\n{e}");
                }
                checked += 1;
            }
        }
        assert!(checked > 15, "expected the docs to be full of kuma commands, found {checked}");
    }

    /// Every verb that speaks --json has to be in both lists or in
    /// neither. Install was in one, which made `--json` a flag that
    /// parsed and did nothing on the only verb that cannot be undone: a
    /// failed install emitted no document at all, while docs/agents.md
    /// promised exactly one.
    #[test]
    fn every_mutating_verb_that_takes_json_actually_enters_json_mode() {
        use clap::CommandFactory;
        // Read off the CLI rather than a hand-kept list, so a verb that
        // gains --json later cannot be forgotten here.
        let mutating_with_json = [
            "build", "switch", "update", "rollback", "sync", "clean", "add", "capture", "remove",
            "install",
        ];
        for verb in mutating_with_json {
            let sub = Cli::command()
                .get_subcommands()
                .find(|s| s.get_name() == verb)
                .unwrap_or_else(|| panic!("{verb} is not a verb any more; update this list"))
                .clone();
            assert!(
                sub.get_arguments().any(|a| a.get_id() == "json"),
                "{verb} is treated as a json-mode verb and does not take --json"
            );
        }
        // The pairing itself: both lists live in `main`, so this asserts
        // the source says so rather than re-deriving it.
        let src = include_str!("main.rs");
        let json_list = src.split("let mutating_json").nth(1).expect("mutating_json exists");
        let json_list = json_list.split("let mutating =").next().unwrap();
        let mutating = src.split("let mutating = matches!").nth(1).expect("mutating exists");
        let mutating = mutating.split("let json_mode").next().unwrap();
        for verb in [
            "Install", "Build", "Switch", "Update", "Rollback", "Sync", "Clean", "Add", "Capture",
            "Remove",
        ] {
            assert!(
                json_list.contains(&format!("Cmd::{verb}")),
                "{verb} is missing from mutating_json"
            );
            assert!(
                mutating.contains(&format!("Cmd::{verb}")),
                "{verb} takes --json and is missing from `mutating`, so the flag does nothing"
            );
        }
    }

    /// What `kuma init` writes is the first declaration anybody has, and
    /// nothing parsed it. It also pinned a base, which opts a newcomer
    /// out of the composed base every published image is built on,
    /// before they know that is a choice.
    #[test]
    fn the_starter_declaration_parses_and_composes_its_own_base() {
        let config: crate::config::Config =
            toml::from_str(STARTER).expect("the starter declaration parses");
        config.validate().expect("the starter declaration validates");
        assert!(
            config.system.base.is_none(),
            "the first declaration a stranger gets should compose a base, not name one"
        );
    }

    /// The installer's default image and the workflow that publishes it
    /// are the same string written in two languages, and nothing else
    /// compares them. Get the owner, the package name or the tag scheme
    /// out of step and `kuma install` defaults to a ref that does not
    /// exist, which is discovered by somebody trying to install rather
    /// than by anything here.
    /// Three places key work off the repository without its tag — the
    /// policy stanza, the registries.d scope and doctor's lookup — and
    /// they have to spell it the same way or the machine requires a
    /// signature under a name nothing grades. They share one derivation
    /// now; this pins what it derives.
    #[test]
    fn the_published_repo_is_the_published_image_without_its_tag() {
        let repo = super::published_repo();
        assert!(!repo.is_empty());
        assert!(super::PUBLISHED_IMAGE.starts_with(repo));
        assert!(
            super::PUBLISHED_IMAGE[repo.len()..].starts_with(':'),
            "published_repo left something other than a tag behind"
        );
        assert!(!repo.contains(':'), "a tag survived into the repository name");
    }

    /// Reading an image's os-release must never be able to fetch one.
    /// podman's default is `--pull=missing`, and with it `update --check`
    /// downloads a base it documents itself as not pulling, while
    /// `update` reads the *post-move* release before its own pull and so
    /// reports no move across a Fedora major. The flag is the whole fix,
    /// which makes it exactly the kind of thing a tidy-up deletes.
    #[test]
    fn reading_an_images_os_release_never_pulls_it() {
        let argv = super::os_release_argv("example.invalid/image:tag", "$VERSION_ID");
        assert!(
            argv.iter().any(|a| a == "--pull=never"),
            "os-release reads must not be able to pull: {argv:?}"
        );
        assert!(argv.iter().any(|a| a.contains("VERSION_ID")));
    }

    #[test]
    fn the_default_image_is_the_one_the_workflow_publishes() {
        let workflow = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/.github/workflows/publish.yml"
        ))
        .unwrap();
        // Assert the shape, not the spelling. The first version of this
        // matched one literal line of YAML and broke the moment that line
        // was rewritten to add a second tag, which is a test failing over
        // its own phrasing rather than over anything being wrong. What
        // has to hold is that the workflow builds `ghcr.io/<owner>/kuma`,
        // tags it with its `example` input, and can produce the tag the
        // installer defaults to.
        let (repo, tag) = super::PUBLISHED_IMAGE.rsplit_once(':').unwrap();
        let owner = repo.strip_prefix("ghcr.io/").unwrap().strip_suffix("/kuma").unwrap();
        assert!(
            workflow.contains(r#"repo="ghcr.io/$owner/kuma""#),
            "publish.yml no longer builds a ghcr.io/<owner>/kuma reference"
        );
        assert!(
            workflow.contains(r#"echo "remote=$repo:${{ inputs.example }}""#),
            "publish.yml no longer tags the image with its example input"
        );
        assert!(
            workflow.contains(&format!("options: [{tag}, "))
                || workflow.contains(&format!(", {tag}]")),
            "publish.yml cannot publish the tag `{tag}` that kuma install defaults to"
        );
        assert_eq!(owner, owner.to_lowercase(), "ghcr rejects an uppercase path");
        // The default tracks rather than pins: the pinned tag carries a
        // version, and pointing installs at one would freeze every new
        // machine on whatever was current when this line was written.
        assert!(!tag.contains(char::is_numeric), "the default should be the moving tag");
    }

    /// A Fedora major arriving unannounced is the failure this exists to
    /// prevent, so the quiet cases matter as much as the loud one: a
    /// release that did not move, and a release that could not be read,
    /// must both say nothing rather than invent a change.
    #[test]
    fn a_release_change_is_announced_and_a_non_change_is_not() {
        let f = |s: &str| Some(s.to_string());
        assert_eq!(super::release_move(&f("44"), &f("45")), Some(("44".into(), "45".into())));
        assert_eq!(super::release_move(&f("44"), &f("44")), None);
        assert_eq!(super::release_move(&None, &f("45")), None);
        assert_eq!(super::release_move(&f("44"), &None), None);
        assert_eq!(super::release_move(&None, &None), None);

        let moved = super::release_move(&f("44"), &f("45"));
        let json = super::release_move_json(moved.as_ref(), &f("45"));
        assert_eq!(json["changed"], true);
        assert_eq!(json["from"], "44");
        assert_eq!(json["to"], "45");
        assert_eq!(json["current"], "45");

        // The steady state still reports where the machine is, because
        // "which Fedora am I on" should not require a release change to
        // become answerable.
        let json = super::release_move_json(None, &f("44"));
        assert_eq!(json["changed"], false);
        assert_eq!(json["current"], "44");
        assert!(json["from"].is_null());
    }

    /// `kuma update --json` is how an agent learns what an update did to
    /// the machine, so the change set has to be in the document and not
    /// only in the human text. Null when there was no previous lock to
    /// compare against (the first build), which is different from an
    /// update that moved nothing.
    #[test]
    fn update_json_carries_what_moved() {
        assert!(super::lock_diff_json(None).is_null());

        let moved = crate::lock::LockDiff {
            base_from: "sha256:old".into(),
            base_to: "sha256:new".into(),
            changed: vec![("bootc".into(), "1.16.6".into(), "1.16.7".into())],
            added: vec!["newpkg".into()],
            removed: vec![],
        };
        let json = super::lock_diff_json(Some(&moved));
        assert_eq!(json["base"]["moved"], true);
        assert_eq!(json["base"]["from"], "sha256:old");
        assert_eq!(json["rpm"]["changed"][0]["name"], "bootc");
        assert_eq!(json["rpm"]["changed"][0]["to"], "1.16.7");
        assert_eq!(json["rpm"]["added"][0], "newpkg");
        assert!(json["rpm"]["removed"].as_array().unwrap().is_empty());
    }

    /// Digests are 64 hex characters and nobody reads them; the report is
    /// unreadable if two of them wrap the terminal.
    #[test]
    fn digests_are_shortened_for_humans() {
        assert_eq!(
            super::short("sha256:3e9f042245cf5be2c092b85b5091743b8e47fd57965c512cc4352ca1ac22daa7"),
            "sha256:3e9f042245cf"
        );
        // and something already short, or not a digest at all, survives
        assert_eq!(super::short("sha256:abc"), "sha256:abc");
    }

    /// bib rejects unsupported blueprint keys for qcow2 and then builds
    /// the disk anyway, so an inert key costs nothing but a "blueprint
    /// validation failed" line in every VM build. It reports one key at a
    /// time, which is how hostname and timezone hid behind each other
    /// until the smoke tests started reading the output. Both look
    /// obviously correct and would be re-added on sight, and both already
    /// have working replacements (/etc/hostname in the image; -fw_cfg and
    /// A disk built from an image that declares a shell logs you into it.
    ///
    /// `[system].shell` exists for the image with no `[user]`, which is
    /// every published image and every committed example. `kuma install`
    /// honored it and `kuma vm` did not, so `examples/niri.toml` says
    /// `shell = "fish"` and its VM handed back bash — the exact case the
    /// field was added for.
    #[test]
    fn a_vm_disk_honors_the_shell_its_image_declares() {
        let out = super::bib_config_toml(None, Some("fish"));
        assert!(out.contains("shell = \"/usr/bin/fish\""), "{out}");
        // Omitted, never empty: bib treats an empty shell as a shell.
        assert!(!super::bib_config_toml(None, None).contains("shell ="));
        // And it is the account's key, so it has to sit inside the user
        // table rather than after the filesystem block that follows it.
        let at = out.find("shell =").unwrap();
        assert!(at < out.find("[[customizations.filesystem]]").unwrap(), "{out}");
    }

    /// kuma-vm-timezone at boot), so the absence is pinned here.
    #[test]
    fn vm_config_asks_bib_for_nothing_it_refuses() {
        let out = super::bib_config_toml(None, None);
        assert!(!out.contains("hostname"), "bib rejects it for qcow2");
        assert!(!out.contains("timezone"), "bib rejects it for qcow2");
        // and still carries what bib does support
        assert!(out.contains("[[customizations.user]]"));
        assert!(out.contains("minsize = \"20 GiB\""));
        // no key to inject means no key line at all, not an empty one
        assert!(!out.contains("key ="));
    }

    /// XFS refuses a duplicate UUID and osbuild pins UUIDs, so the two
    /// together made one automounted disk poison every later build. The
    /// choice is pinned here because it reads like a preference and is
    /// not one; anything that reverts it brings the failure back.
    #[test]
    fn disks_are_built_on_a_filesystem_that_tolerates_duplicate_uuids() {
        assert_eq!(super::BIB_ROOTFS, "ext4");
    }

    /// The desktop automounts each disk build's partitions under
    /// /run/media/<user>, which pins the loop device they sit on. Only
    /// that combination counts: a loop device mounted somewhere else is
    /// someone's business, and a real disk under /run/media is a USB
    /// stick.
    #[test]
    fn only_loop_devices_under_run_media_are_reported() {
        let mountinfo = "\
25 30 0:22 / /proc rw,nosuid,nodev,noexec,relatime shared:12 - proc proc rw
99 33 7:4 / /run/media/mira/root ro,nosuid,nodev,relatime shared:1 - ext4 /dev/loop0p4 ro,seclabel
98 33 7:3 / /run/media/mira/boot ro,nosuid,nodev,relatime shared:2 - xfs /dev/loop0p3 ro,seclabel
97 33 8:17 / /run/media/mira/usb rw,nosuid,nodev,relatime shared:3 - vfat /dev/sdb1 rw
96 33 7:9 / /mnt/scratch rw,relatime shared:4 - ext4 /dev/loop9 rw,seclabel
";
        assert_eq!(
            super::loop_mounts_in(mountinfo),
            ["/run/media/mira/root", "/run/media/mira/boot"]
        );
    }

    /// A public key's trailing comment is free text fixed at the moment
    /// the key was generated, so it can hold anything a hostname or a
    /// `-C` once held, quotes included. Interpolating it into the
    /// blueprint would hand bib a file it cannot parse, and the failure
    /// would surface as a disk build dying rather than as a bad comment.
    #[test]
    fn a_pubkey_comment_cannot_break_the_blueprint() {
        let hostile = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5 a \"quoted\\name\"\n";
        let out = super::bib_config_toml(Some(hostile), None);
        let parsed: toml::Value = toml::from_str(&out).expect("blueprint stays valid TOML");
        assert_eq!(parsed["customizations"]["user"][0]["key"].as_str().unwrap(), hostile.trim());
    }

    #[test]
    fn iso_config_reflects_declaration() {
        let with_user: crate::config::Config =
            toml::from_str("schema_version = 1\n[user]\nname = \"m\"\n").unwrap();
        let out = super::iso_config_toml(&with_user);
        // must be valid TOML — the kickstart rides in a multiline string
        toml::from_str::<toml::Value>(&out).unwrap();
        assert!(out.contains("network --hostname=kuma"));
        assert!(out.contains("firstboot --disable"));
        // declared user → Anaconda's user screen is dropped
        assert!(out.contains("org.fedoraproject.Anaconda.Modules.Users"));

        let bare: crate::config::Config =
            toml::from_str("schema_version = 1\n[system]\nhostname = \"pine\"\n").unwrap();
        let out = super::iso_config_toml(&bare);
        toml::from_str::<toml::Value>(&out).unwrap();
        // image already pins /etc/hostname; no user declared → Anaconda
        // keeps its user screen so installs aren't left with no account
        assert!(!out.contains("--hostname"));
        assert!(!out.contains("Modules.Users"));
    }

    #[test]
    fn schema_reflects_the_parser_types() {
        let schema = serde_json::to_value(schemars::schema_for!(crate::config::Config)).unwrap();
        // unknown keys rejected at the root, same as serde does
        assert_eq!(schema["additionalProperties"], false);
        assert!(schema["properties"]["packages"].is_object());
        let text = serde_json::to_string(&schema).unwrap();
        // enum variants and field docs ride along for agents
        assert!(text.contains("\"niri\""));
        assert!(text.contains("password_hash"));
        assert!(text.contains("crypt(5)"));
    }

    #[test]
    fn rollback_facts_read_the_slots() {
        // no rollback slot (fresh install): nothing to land on
        let json = serde_json::json!({"status": {"booted": {"image": {}}, "rollback": null}});
        assert!(super::rollback_facts(&json).is_none());

        // rollback present, digest-pinned target; staged would be discarded
        let json = serde_json::json!({"status": {
            "staged": {"image": {}},
            "rollback": {"image": {
                "image": {"image": "localhost/kuma:latest"},
                "imageDigest": "sha256:0123456789abcdef0123456789abcdef",
            }},
        }});
        let (target, staged) = super::rollback_facts(&json).unwrap();
        assert_eq!(target, "localhost/kuma:latest (0123456789ab)");
        assert!(staged);

        // digest missing or odd: the tag alone still names the target
        let json = serde_json::json!({"status": {
            "rollback": {"image": {"image": {"image": "localhost/kuma:latest"}}},
        }});
        let (target, staged) = super::rollback_facts(&json).unwrap();
        assert_eq!(target, "localhost/kuma:latest");
        assert!(!staged);
    }

    #[test]
    fn generated_hash_is_valid_config_material() {
        // hash_password's 656k rounds take ~13s in a debug build — hash at
        // the spec minimum instead (still a real rounds= hash, so the '='
        // path is exercised) and validate the production shape statically.
        let real = super::hash_with("kuma", sha_crypt::Params::new(1_000).unwrap()).unwrap();
        assert!(real.starts_with("$6$"));

        // The salt has to survive crypt(3) unchanged. sha512-crypt
        // truncates it to 16 characters, and PAM authenticates by
        // string-comparing crypt(password, stored) against stored, so a
        // salt longer than that hashes correctly and still fails every
        // login: libcrypt echoes back a shorter string than the one we
        // wrote. sha-crypt's own salt generator produces 22 characters
        // and would do exactly this.
        let salt = real.split('$').nth(3).expect("$6$rounds=N$salt$hash");
        assert!(salt.len() <= 16, "salt {salt:?} is {} chars, crypt(3) keeps 16", salt.len());

        for hash in [real.as_str(), "$6$rounds=656000$0aQ8mNcQ$abc./XYZ"] {
            // must survive the [user] password_hash validation round-trip
            let config: crate::config::Config = toml::from_str(&format!(
                "schema_version = 1\n[user]\nname = \"m\"\npassword_hash = '{hash}'\n"
            ))
            .unwrap();
            config.validate().unwrap();
        }
    }
}
