mod config;
mod containerfile;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use config::Config;
use std::path::{Path, PathBuf};
use std::process::Command;

const DEFAULT_TAG: &str = "localhost/kuma:latest";
const BIB_IMAGE: &str = "quay.io/centos-bootc/bootc-image-builder:latest";

#[derive(Parser)]
#[command(name = "kuma", version, about = "Your system is one file.")]
struct Cli {
    /// Path to the kuma config file
    #[arg(long, global = true, default_value = "kuma.toml")]
    config: PathBuf,

    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Write a starter kuma.toml in the current directory
    Init {
        /// Overwrite an existing kuma.toml
        #[arg(long)]
        force: bool,
    },
    /// Print the Containerfile compiled from kuma.toml
    Generate,
    /// Build the system image locally with podman
    Build {
        /// Image tag to build
        #[arg(long, default_value = DEFAULT_TAG)]
        tag: String,
    },
    /// Point bootc at the built image (prints the command unless --yes)
    Switch {
        /// Image tag to switch to
        #[arg(long, default_value = DEFAULT_TAG)]
        tag: String,
        /// Actually run `bootc switch` (requires root; reboots take effect later)
        #[arg(long)]
        yes: bool,
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
    },
    /// Show bootc status for this machine
    Status,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Cmd::Init { force } => init(force),
        Cmd::Generate => {
            let config = Config::load(&cli.config)?;
            print!("{}", containerfile::generate(&config));
            Ok(())
        }
        Cmd::Build { tag } => build(&cli.config, &tag),
        Cmd::Switch { tag, yes } => switch(&tag, yes),
        Cmd::Vm { tag, output, no_run, rebuild } => vm(&tag, &output, no_run, rebuild),
        Cmd::Status => run_host(&["bootc", "status"]),
    }
}

const STARTER: &str = r#"# Kuma system definition
schema_version = 1

[system]
base = "quay.io/fedora/fedora-bootc:44"

[packages]
rpm = []
# Flatpaks are recorded here but applied at runtime (`kuma sync`, not yet implemented)
flatpak = []

[services]
enable = []
disable = []
"#;

fn init(force: bool) -> Result<()> {
    let path = PathBuf::from("kuma.toml");
    if path.exists() && !force {
        bail!("kuma.toml already exists (use --force to overwrite)");
    }
    std::fs::write(&path, STARTER).context("cannot write kuma.toml")?;
    println!("Wrote kuma.toml — edit it, then run `kuma build`.");
    Ok(())
}

fn build(config_path: &Path, tag: &str) -> Result<()> {
    let config = Config::load(config_path)?;
    if !config.packages.flatpak.is_empty() {
        eprintln!(
            "note: {} flatpak(s) declared; runtime apply is not implemented yet",
            config.packages.flatpak.len()
        );
    }
    let dir = tempfile::tempdir().context("cannot create build directory")?;
    let containerfile = dir.path().join("Containerfile");
    std::fs::write(&containerfile, containerfile::generate(&config))?;

    run_host(&[
        "podman",
        "build",
        "--tag",
        tag,
        "--file",
        containerfile.to_str().context("non-UTF-8 temp path")?,
        dir.path().to_str().context("non-UTF-8 temp path")?,
    ])?;
    println!("\nBuilt {tag}. Apply it with `kuma switch`, or boot it with `kuma vm`.");
    Ok(())
}

fn switch(tag: &str, yes: bool) -> Result<()> {
    let args = [
        "bootc",
        "switch",
        "--transport",
        "containers-storage",
        tag,
    ];
    if !yes {
        println!("Would run (as root):\n\n  {}\n", args.join(" "));
        println!("Re-run with --yes to apply. The change takes effect on next boot;");
        println!("the previous deployment stays available for rollback.");
        return Ok(());
    }
    let mut sudo_args = vec!["sudo"];
    sudo_args.extend(args);
    run_host(&sudo_args)
}

fn vm(tag: &str, output: &Path, no_run: bool, rebuild: bool) -> Result<()> {
    std::fs::create_dir_all(output)
        .with_context(|| format!("cannot create {}", output.display()))?;
    let output = std::fs::canonicalize(output)?;
    let disk = output.join("qcow2/disk.qcow2");

    if !disk.exists() || rebuild {
        build_disk(tag, &output)?;
    } else {
        println!("Reusing existing disk {} (use --rebuild to regenerate).", disk.display());
    }

    if no_run {
        println!("Disk ready: {}", disk.display());
        println!("Boot it later with `kuma vm`, or import it into GNOME Boxes / virt-manager.");
        return Ok(());
    }
    boot_disk(&disk)
}

fn build_disk(tag: &str, output: &Path) -> Result<()> {
    // bootc-image-builder runs as root and reads root's containers-storage,
    // so a rootless-built image has to be copied over first.
    if !host_ok(&["sudo", "podman", "image", "exists", tag]) {
        println!("Copying {tag} into root podman storage (one-time, may take a minute)...");
        let archive = output.join("kuma-image.tar");
        let archive_str = path_str(&archive)?;
        run_host(&["podman", "save", "--format", "oci-archive", "-o", archive_str, tag])?;
        run_host(&["sudo", "podman", "load", "-i", archive_str])?;
        let _ = std::fs::remove_file(&archive);
    }

    let bib_config = output.join("config.toml");
    std::fs::write(&bib_config, bib_config_toml())?;

    println!("Building qcow2 with bootc-image-builder (this takes a few minutes)...");
    run_host(&[
        "sudo",
        "podman",
        "run",
        "--rm",
        "--privileged",
        "--security-opt",
        "label=type:unconfined_t",
        "-v",
        &format!("{}:/output", path_str(output)?),
        "-v",
        "/var/lib/containers/storage:/var/lib/containers/storage",
        "-v",
        &format!("{}:/config.toml:ro", path_str(&bib_config)?),
        BIB_IMAGE,
        "--type",
        "qcow2",
        // fedora-bootc images declare no default root filesystem, so bib
        // fails with "missing required info: DefaultRootFs" without this.
        "--rootfs",
        "xfs",
        tag,
    ])?;

    // bib ran as root, so its output is root-owned; hand it back to the
    // user so QEMU (and cleanup) work without privileges.
    let user = std::env::var("USER").context("USER is not set")?;
    run_host(&["sudo", "chown", "-R", &format!("{user}:"), path_str(output)?])?;
    Ok(())
}

/// Disk-image config: a login user so the VM is actually reachable.
/// Password login on the console, plus the user's ssh key when one exists.
fn bib_config_toml() -> String {
    let mut out = String::from(
        "[[customizations.user]]\nname = \"kuma\"\npassword = \"kuma\"\ngroups = [\"wheel\"]\n",
    );
    if let Some(key) = find_ssh_pubkey() {
        out.push_str(&format!("key = \"{}\"\n", key.trim()));
    }
    out
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

fn boot_disk(disk: &Path) -> Result<()> {
    println!("Booting VM (user: kuma, password: kuma; ssh: `ssh -p 2222 kuma@localhost`)...");
    run_host(&[
        "qemu-system-x86_64",
        "-enable-kvm",
        "-cpu",
        "host",
        "-smp",
        "4",
        "-m",
        "4096",
        "-drive",
        &format!("file={},if=virtio", path_str(disk)?),
        "-nic",
        "user,model=virtio-net-pci,hostfwd=tcp::2222-:22",
    ])
}

fn path_str(path: &Path) -> Result<&str> {
    path.to_str().context("non-UTF-8 path")
}

/// Run a command on the host, escaping the container if kuma itself is
/// running inside one (e.g. a distrobox dev environment).
fn run_host<S: AsRef<str>>(args: &[S]) -> Result<()> {
    let status = host_command(args)?
        .status()
        .with_context(|| format!("failed to run {}", args[0].as_ref()))?;
    if !status.success() {
        let shown: Vec<&str> = args.iter().map(|s| s.as_ref()).collect();
        bail!("{} exited with {status}", shown.join(" "));
    }
    Ok(())
}

/// Like run_host, but a failed or missing command is just `false`.
fn host_ok<S: AsRef<str>>(args: &[S]) -> bool {
    host_command(args)
        .and_then(|mut cmd| {
            cmd.stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
            Ok(cmd.status()?)
        })
        .map(|s| s.success())
        .unwrap_or(false)
}

fn host_command<S: AsRef<str>>(args: &[S]) -> Result<Command> {
    let in_container = Path::new("/run/.containerenv").exists()
        || Path::new("/.dockerenv").exists();
    let mut full: Vec<&str> = Vec::new();
    if in_container {
        full.extend(["flatpak-spawn", "--host"]);
    }
    full.extend(args.iter().map(|s| s.as_ref()));

    let mut cmd = Command::new(full[0]);
    cmd.args(&full[1..]);
    Ok(cmd)
}
