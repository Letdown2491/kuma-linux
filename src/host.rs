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

/// One line per subprocess, behind `KUMA_TRACE`.
///
/// Every measurement in the 2026-08-21 performance review was taken by
/// hand, and the ranking that came out of it did not survive contact
/// with a stopwatch: the item ranked first cost 1.1s and the one
/// mentioned in passing cost 4.6s on every build. That is not a failure
/// of judgement, it is what happens when the numbers have to be produced
/// by a person who already has a hypothesis.
///
/// So the numbers stop being something anybody has to produce. Set
/// `KUMA_TRACE=1` and every command kuma runs prints what it was and how
/// long it took, on stderr, where it does not disturb `--json` on stdout.
///
/// CANONICAL rather than the full argv: a digest, a tempdir and an image
/// tag change on every run, and a ledger nobody can diff is a ledger
/// nobody reads. `podman image inspect --format {{.Id}} localhost/kuma:latest`
/// prints as `podman image inspect`, which is stable, and two identical
/// lines in one verb's ledger is itself a finding.
fn canonical<S: AsRef<str>>(args: &[S]) -> String {
    args.iter()
        .map(|a| a.as_ref())
        .take_while(|word| !word.starts_with('-') && !word.contains('/') && !word.contains('='))
        .take(3)
        .collect::<Vec<&str>>()
        .join(" ")
}

fn trace<S: AsRef<str>>(args: &[S], started: std::time::Instant) {
    if std::env::var_os("KUMA_TRACE").is_none() {
        return;
    }
    eprintln!("[kuma-trace] {:7.3}s  {}", started.elapsed().as_secs_f64(), canonical(args));
}

/// Run a command on the host, escaping the container if kuma itself is
/// running inside one (e.g. a distrobox dev environment).
pub fn run_host<S: AsRef<str>>(args: &[S]) -> Result<()> {
    run_piped(args, None)
}

/// Run a command on the host, handing it `input` on standard input.
///
/// For the one thing kuma has that must not appear in argv: a disk
/// passphrase. `ps` shows any process's arguments to any user, so a
/// secret passed as one is readable by everybody on the machine for as
/// long as the command runs, and a file would leave it on a disk.
///
/// `sudo` still gets its own prompt: it reads a password from the
/// terminal rather than from stdin unless it is asked to do otherwise,
/// so a piped stdin does not swallow the one the person has to answer.
pub fn run_host_stdin<S: AsRef<str>>(args: &[S], input: &str) -> Result<()> {
    run_piped(args, Some(input))
}

/// The body both share, because they differ in one thing and the rest of
/// it (finding the host, moving stdout out of the way of JSON, naming
/// the command that failed) was worth writing once.
fn run_piped<S: AsRef<str>>(args: &[S], input: Option<&str>) -> Result<()> {
    use std::io::Write;
    let started = std::time::Instant::now();
    let mut cmd = host_command(args)?;
    if input.is_some() {
        cmd.stdin(std::process::Stdio::piped());
    }
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
    let mut child = cmd.spawn().with_context(|| format!("failed to run {}", args[0].as_ref()))?;
    // A write that fails is not the failure worth reporting. When sudo
    // refuses a password it exits before reading, and this write then
    // returns EPIPE: reporting that would replace "sudo: 3 incorrect
    // password attempts" with "cannot write to the child process", which
    // sends somebody looking at the wrong thing entirely. So the child's
    // own status is asked first, and the write is only news if the
    // command otherwise succeeded.
    let wrote = match (input, child.stdin.take()) {
        (Some(input), Some(mut stdin)) => stdin.write_all(input.as_bytes()),
        (Some(_), None) => Ok(()),
        (None, _) => Ok(()),
    };
    let status = child.wait().with_context(|| format!("failed to run {}", args[0].as_ref()))?;
    trace(args, started);
    if !status.success() {
        let shown: Vec<&str> = args.iter().map(|s| s.as_ref()).collect();
        bail!("{} exited with {status}", shown.join(" "));
    }
    wrote.context("cannot write to the child process")?;
    Ok(())
}

/// Run a host command and capture its trimmed stdout; Err on failure.
pub fn host_output<S: AsRef<str>>(args: &[S]) -> Result<String> {
    let started = std::time::Instant::now();
    let out = host_command(args)?
        // Captured rather than discarded, so a failure that reaches a
        // human carries the tool's own words. podman saying "Treating
        // single images as manifest lists is not implemented" is worth
        // rather more than "exited with exit status: 125". Probes that
        // expect to fail drop the Err anyway, so nothing gets noisier.
        .stderr(std::process::Stdio::piped())
        .output()
        .with_context(|| format!("failed to run {}", args[0].as_ref()))?;
    trace(args, started);
    if !out.status.success() {
        let shown: Vec<&str> = args.iter().map(|s| s.as_ref()).collect();
        bail!("{} exited with {}{}", shown.join(" "), out.status, reason(&out.stderr));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The tail of stderr, where tools put the reason. Bounded in both
/// directions: the last two lines, each clipped, because some of them
/// answer with a kilobyte of JSON and an error nobody can read is worth
/// no more than the exit code it replaced.
fn reason(stderr: &[u8]) -> String {
    const WIDTH: usize = 200;
    let text = String::from_utf8_lossy(stderr);
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let tail: Vec<String> = lines[lines.len().saturating_sub(2)..]
        .iter()
        // Clipped from the middle, not the end: tools put the reason at
        // either end and sometimes both. podman answers a bad manifest
        // request with a kilobyte of echoed JSON and then, last of all,
        // the sentence that explains it.
        .map(|line| {
            let chars: Vec<char> = line.chars().collect();
            if chars.len() <= WIDTH {
                return line.to_string();
            }
            let half = WIDTH / 2;
            let head: String = chars[..half].iter().collect();
            let tail: String = chars[chars.len() - half..].iter().collect();
            format!("{head} ... {tail}")
        })
        .collect();
    if tail.is_empty() {
        String::new()
    } else {
        format!(": {}", tail.join("; "))
    }
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
    let in_container =
        Path::new("/run/.containerenv").exists() || Path::new("/.dockerenv").exists();
    let mut full: Vec<&str> = Vec::new();
    if in_container {
        full.extend(["flatpak-spawn", "--host"]);
    }
    full.extend(args.iter().map(|s| s.as_ref()));

    let mut cmd = Command::new(full[0]);
    cmd.args(&full[1..]);
    Ok(cmd)
}
