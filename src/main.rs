mod config;
mod containerfile;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use config::Config;
use std::path::PathBuf;
use std::process::Command;

const DEFAULT_TAG: &str = "localhost/kuma:latest";

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
        Cmd::Status => run_host(&["bootc", "status"]),
    }
}

const STARTER: &str = r#"# Kuma system definition — https://github.com/mira/kuma
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

fn build(config_path: &std::path::Path, tag: &str) -> Result<()> {
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
    println!("\nBuilt {tag}. Apply it with `kuma switch`.");
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

/// Run a command on the host, escaping the container if kuma itself is
/// running inside one (e.g. a distrobox dev environment).
fn run_host(args: &[&str]) -> Result<()> {
    let in_container = std::path::Path::new("/run/.containerenv").exists()
        || std::path::Path::new("/.dockerenv").exists();
    let mut full: Vec<&str> = Vec::new();
    if in_container {
        full.extend(["flatpak-spawn", "--host"]);
    }
    full.extend(args);

    let status = Command::new(full[0])
        .args(&full[1..])
        .status()
        .with_context(|| format!("failed to run {}", full[0]))?;
    if !status.success() {
        bail!("{} exited with {status}", args.join(" "));
    }
    Ok(())
}
