use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};

/// Set when the verb's stdout is a JSON document: everything else —
/// progress notes, subprocess chatter — must land on stderr instead, so
/// an agent can parse stdout without sifting.
static JSON_OUTPUT: AtomicBool = AtomicBool::new(false);

pub fn set_json_output() {
    JSON_OUTPUT.store(true, Ordering::Relaxed);
}

/// A progress note: stdout for humans, stderr when stdout carries JSON.
pub fn note(msg: &str) {
    if JSON_OUTPUT.load(Ordering::Relaxed) {
        eprintln!("{msg}");
    } else {
        println!("{msg}");
    }
}

/// Run a command on the host, escaping the container if kuma itself is
/// running inside one (e.g. a distrobox dev environment).
pub fn run_host<S: AsRef<str>>(args: &[S]) -> Result<()> {
    let mut cmd = host_command(args)?;
    if JSON_OUTPUT.load(Ordering::Relaxed) {
        // The child's stream (podman build output, bootc progress) stays
        // visible — just not where the JSON goes.
        use std::os::fd::AsFd;
        let stderr = std::io::stderr()
            .as_fd()
            .try_clone_to_owned()
            .context("cannot redirect subprocess output to stderr")?;
        cmd.stdout(std::process::Stdio::from(stderr));
    }
    let status = cmd
        .status()
        .with_context(|| format!("failed to run {}", args[0].as_ref()))?;
    if !status.success() {
        let shown: Vec<&str> = args.iter().map(|s| s.as_ref()).collect();
        bail!("{} exited with {status}", shown.join(" "));
    }
    Ok(())
}

/// Run a host command and capture its trimmed stdout; Err on failure.
pub fn host_output<S: AsRef<str>>(args: &[S]) -> Result<String> {
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

/// Capture stdout even when the command exits non-zero — for tools like
/// `systemctl is-enabled` that report state through stdout AND exit status.
/// Err only when the command cannot run at all.
pub fn host_output_any<S: AsRef<str>>(args: &[S]) -> Result<String> {
    let out = host_command(args)?
        .stderr(std::process::Stdio::null())
        .output()
        .with_context(|| format!("failed to run {}", args[0].as_ref()))?;
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
