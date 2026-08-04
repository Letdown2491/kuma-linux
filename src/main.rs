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
        /// Apply the built image to the RUNNING VM (bootc switch inside,
        /// then reboot) — /var (flatpaks, brew, homes) persists
        #[arg(long, conflicts_with_all = ["no_run", "rebuild"])]
        apply: bool,
    },
    /// Hash a password for the [user] section (prompts; prints the line to paste)
    Passwd,
    /// Show bootc status for this machine
    Status,
    /// Print shell completions (e.g. `kuma completions fish | source`)
    Completions {
        /// Shell to generate for
        shell: clap_complete::Shell,
    },
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
        Cmd::Vm { tag, output, no_run, rebuild, apply } => {
            vm(&tag, &output, no_run, rebuild, apply)
        }
        Cmd::Passwd => passwd(),
        Cmd::Status => run_host(&["bootc", "status"]),
        Cmd::Completions { shell } => {
            use clap::CommandFactory;
            clap_complete::generate(shell, &mut Cli::command(), "kuma", &mut std::io::stdout());
            Ok(())
        }
    }
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

fn hash_password(password: &str) -> Result<String> {
    let params = sha_crypt::Sha512Params::new(sha_crypt::ROUNDS_DEFAULT)
        .map_err(|e| anyhow::anyhow!("crypt params: {e:?}"))?;
    sha_crypt::sha512_simple(password, &params)
        .map_err(|e| anyhow::anyhow!("hashing failed: {e:?}"))
}


const STARTER: &str = r#"# Kuma system definition
schema_version = 1

[system]
base = "quay.io/fedora/fedora-bootc:44"
# Pin an IANA timezone across all machines built from this file. Usually
# leave unset — timezone is machine state (`timedatectl set-timezone`).
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
    let dir = tempfile::tempdir().context("cannot create build directory")?;
    containerfile::write_context(&config, dir.path())?;

    run_host(&[
        "podman",
        "build",
        "--tag",
        tag,
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
        let current =
            host_output(&["podman", "image", "inspect", "--format", "{{.Id}}", tag])
                .unwrap_or_default();
        if !current.is_empty() && stamped.trim() != current {
            println!(
                "WARNING: {tag} is newer than this disk — it will NOT have your latest changes. Re-run with --rebuild to pick them up."
            );
        }
    }

    if no_run {
        println!("Disk ready: {}", disk.display());
        println!("Boot it later with `kuma vm`, or import it into GNOME Boxes / virt-manager.");
        return Ok(());
    }
    boot_disk(&disk)
}

fn build_disk(tag: &str, output: &Path) -> Result<()> {
    // bootc-image-builder runs as root and reads root's containers-storage.
    // Sync by image ID, not tag existence: the root-side copy goes stale
    // every time the rootless image is rebuilt.
    let local_id = host_output(&["podman", "image", "inspect", "--format", "{{.Id}}", tag])
        .with_context(|| format!("{tag} not found — run `kuma build` first"))?;
    let root_id = host_output(&["sudo", "podman", "image", "inspect", "--format", "{{.Id}}", tag])
        .unwrap_or_default();
    if local_id != root_id {
        println!("Syncing {tag} into root podman storage (may take a minute)...");
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

    // Stamp which image this disk came from, so a later `kuma vm` can
    // warn when the image has moved on and the disk is silently stale.
    std::fs::write(output.join("image-id"), &local_id)?;
    Ok(())
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
    host_output(&["podman", "image", "inspect", "--format", "{{.Id}}", tag])
        .with_context(|| format!("{tag} not found — run `kuma build` first"))?;

    let mut probe = vec!["ssh", "-p", "2222", "-o", "ConnectTimeout=4"];
    probe.extend(VM_SSH_OPTS);
    probe.extend(["kuma@localhost", "true"]);
    host_output(&probe)
        .context("no running VM reachable on port 2222 — boot one with `kuma vm` first")?;

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
        &format!(
            "podman save {tag} | ssh -p 2222 {ssh_opts} kuma@localhost '{remote_load}'"
        ),
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
        "echo kuma | sudo -S sh -c 'bootc switch --transport containers-storage {tag} && bootc upgrade; podman rmi -f {tag} >/dev/null; bootc status | grep -qiE \"^  Staged|staged image\" || {{ echo \"kuma: nothing staged — the VM already runs this image\" >&2; exit 3; }}'"
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
fn bib_config_toml() -> String {
    // hostname first: bare keys must precede the sub-tables to stay under
    // [customizations]. Written to /etc/hostname at install time — the
    // os-release DEFAULT_HOSTNAME branding loses at boot because the
    // initrd (prebuilt by the Fedora base) sets the hostname before our
    // root is visible.
    let mut out = String::from(
        "[customizations]\nhostname = \"kuma\"\n\n[[customizations.user]]\nname = \"kuma\"\npassword = \"kuma\"\ngroups = [\"wheel\"]\n",
    );
    if let Some(key) = find_ssh_pubkey() {
        out.push_str(&format!("key = \"{}\"\n", key.trim()));
    }
    if let Some(tz) = host_timezone() {
        out.push_str(&format!("\n[customizations.timezone]\ntimezone = \"{tz}\"\n"));
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

fn boot_disk(disk: &Path) -> Result<()> {
    println!("Booting VM (user: kuma, password: kuma; ssh: `ssh -p 2222 kuma@localhost`)...");
    let drive = format!("file={},if=virtio", path_str(disk)?);
    let mut args: Vec<&str> = vec![
        // LIBGL_ALWAYS_SOFTWARE: render virgl on llvmpipe so guest GL work
        // never reaches the host GPU driver — a bad guest submission can
        // otherwise wedge the real GPU and take the host session down.
        "env",
        "LIBGL_ALWAYS_SOFTWARE=1",
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
        // virtio-vga-gl + gl=on (virgl): niri's GBM allocator needs a real
        // 3D-capable device; plain -vga virtio is display-only and leaves
        // the compositor with no output.
        "-device",
        "virtio-vga-gl",
        "-display",
        "gtk,gl=on",
        "-nic",
        "user,model=virtio-net-pci,hostfwd=tcp::2222-:22",
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

/// Run a host command and capture its trimmed stdout; Err on failure.
fn host_output<S: AsRef<str>>(args: &[S]) -> Result<String> {
    let out = host_command(args)?
        .stderr(std::process::Stdio::null())
        .output()
        .with_context(|| format!("failed to run {}", args[0].as_ref()))?;
    if !out.status.success() {
        let shown: Vec<&str> = args.iter().map(|s| s.as_ref()).collect();
        bail!("{} exited with {}", shown.join(" "), out.status);
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
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

#[cfg(test)]
mod tests {
    #[test]
    fn generated_hash_is_valid_config_material() {
        let hash = super::hash_password("kuma").unwrap();
        assert!(hash.starts_with("$6$"));
        // must survive the [user] password_hash validation round-trip
        let config: crate::config::Config = toml::from_str(&format!(
            "schema_version = 1\n[user]\nname = \"m\"\npassword_hash = '{hash}'\n"
        ))
        .unwrap();
        config.validate().unwrap();
    }
}
