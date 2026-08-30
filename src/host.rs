use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
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
    let words: Vec<&str> = args.iter().map(|a| a.as_ref()).collect();
    let mut out: Vec<&str> = words
        .iter()
        .copied()
        .take_while(|word| !word.starts_with('-') && !word.contains('/') && !word.contains('='))
        .take(3)
        .collect();
    // `sh -c <script>` would otherwise print as `sh`, and the first two
    // lines of doctor's own ledger were two indistinguishable `sudo sh`
    // entries at 2.2s and 1.9s. An instrument that cannot tell its most
    // expensive entries apart is not one. The script's first word is
    // what it actually ran.
    if let Some(index) = words.iter().position(|word| *word == "-c") {
        if let Some(script) = words.get(index + 1) {
            if let Some(first) = script.split_whitespace().next() {
                out.push(first);
            }
        }
    }
    out.join(" ")
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

/// A private working directory whose contents are handed to a
/// root-run script.
///
/// Every privileged verb stages the same way: write a script and the
/// files it reads into a fresh tempdir, then run the script as root.
/// What kept going wrong was the step in between. A tempdir is 0700
/// for whoever created it, and root reads through that without help,
/// so widening it is defense in depth rather than a necessity — but
/// the widen itself was the hazard: `chmod -R a+rX` over everything
/// followed by `chmod 600` put-backs made an account's password hash
/// and a backup repository password readable by every local account
/// for the two process spawns in between, and longer on a sudo
/// configuration that re-prompts.
///
/// So the discipline lives here, once. A credential is written 0600
/// from the moment it exists — created with the mode rather than
/// narrowed after — and the widen before a run skips them entirely
/// rather than restoring them afterwards: a skip has no window at all,
/// and a smaller window is not the same as none.
pub struct ForRoot {
    /// Held rather than borrowed so the directory lives exactly as
    /// long as the staging does and is removed when the verb ends.
    dir: tempfile::TempDir,
    /// The files the widen must never touch, in the order they were
    /// staged.
    credentials: Vec<PathBuf>,
    widened: bool,
}

/// Stage a working directory for a root-run script.
pub fn for_root() -> Result<ForRoot> {
    Ok(ForRoot {
        dir: tempfile::tempdir().context("cannot create a working directory")?,
        credentials: Vec::new(),
        widened: false,
    })
}

impl ForRoot {
    /// The directory itself, for the one argument a staged script
    /// takes that is neither a disk nor a size: its own context.
    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    /// Write a file the script will read, and return where it landed.
    /// `name` is a basename; the directory is the adapter's to place.
    pub fn file(&self, name: &str, contents: &str) -> Result<PathBuf> {
        let path = self.dir.path().join(name);
        std::fs::write(&path, contents).with_context(|| format!("cannot stage {name}"))?;
        Ok(path)
    }

    /// Write a credential: 0600 from the moment it exists, and never
    /// widened. Tracked rather than merely marked, because the widen
    /// is one command that must skip them all.
    pub fn credential(&mut self, name: &str, contents: &str) -> Result<PathBuf> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let path = self.dir.path().join(name);
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)
            .with_context(|| format!("cannot stage {name}"))?;
        file.write_all(contents.as_bytes()).with_context(|| format!("cannot stage {name}"))?;
        self.credentials.push(path.clone());
        Ok(path)
    }

    /// Run a staged script as root, discarding its output.
    pub fn run<S: AsRef<str>>(&mut self, script: &Path, args: &[S]) -> Result<()> {
        self.widen_once()?;
        run_host(&sudo_bash(script, args)?)
    }

    /// Run a staged script as root, feeding it `input` on stdin: the
    /// one channel for what must not appear in argv, which is why a
    /// disk passphrase travels this way.
    pub fn run_stdin<S: AsRef<str>>(
        &mut self,
        script: &Path,
        args: &[S],
        input: &str,
    ) -> Result<()> {
        self.widen_once()?;
        run_host_stdin(&sudo_bash(script, args)?, input)
    }

    /// Run a staged script as root and capture its stdout, for the
    /// scripts that answer with a fact rather than an exit code.
    pub fn output<S: AsRef<str>>(&mut self, script: &Path, args: &[S]) -> Result<String> {
        self.widen_once()?;
        host_output(&sudo_bash(script, args)?)
    }

    /// Widen before the first run and never again. A credential staged
    /// after the widen is already 0600, so no order of calls leaves
    /// one exposed: the mode is what protects it, and the prune is
    /// what keeps the widen from unprotecting it.
    fn widen_once(&mut self) -> Result<()> {
        if self.widened {
            return Ok(());
        }
        let dir = self.dir.path().to_str().context("non-UTF-8 working directory")?;
        let credentials = self
            .credentials
            .iter()
            .map(|c| c.to_str().map(str::to_string))
            .collect::<Option<Vec<_>>>()
            .context("non-UTF-8 credential path")?;
        run_host(&widen_argv(dir, &credentials))
            .context("cannot widen the working directory for root")?;
        self.widened = true;
        Ok(())
    }
}

/// The command that widens a staged directory for root without ever
/// touching a credential: each is pruned, so it holds no permission it
/// did not arrive with. With nothing to protect, this is a plain
/// recursive widen, which is the shape every caller got before the
/// credentials made it dangerous.
fn widen_argv(dir: &str, credentials: &[String]) -> Vec<String> {
    let mut out = vec!["sudo".to_string(), "find".to_string(), dir.to_string()];
    if !credentials.is_empty() {
        out.push("(".to_string());
        for (i, credential) in credentials.iter().enumerate() {
            if i > 0 {
                out.push("-o".to_string());
            }
            out.push("-path".to_string());
            out.push(credential.clone());
        }
        out.push(")".to_string());
        out.push("-prune".to_string());
        out.push("-o".to_string());
    }
    out.extend(["-exec", "chmod", "a+rX", "{}", "+"].iter().map(|s| s.to_string()));
    out
}

/// `sudo bash <script> <args…>`, gathered because that is the one
/// shape a staged script is run in and no other.
fn sudo_bash<S: AsRef<str>>(script: &Path, args: &[S]) -> Result<Vec<String>> {
    let mut argv = vec![
        "sudo".to_string(),
        "bash".to_string(),
        script.to_str().context("non-UTF-8 script path")?.to_string(),
    ];
    argv.extend(args.iter().map(|a| a.as_ref().to_string()));
    Ok(argv)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Nothing to protect, no prune clause: a plain recursive widen,
    /// which is what the hibernate verbs' hand-rolled chmods did, minus
    /// the three dialects they did it in.
    #[test]
    fn widen_without_credentials_prunes_nothing() {
        let argv = widen_argv("/tmp/xyz", &[]);
        assert_eq!(
            argv.iter().map(String::as_str).collect::<Vec<_>>(),
            vec!["sudo", "find", "/tmp/xyz", "-exec", "chmod", "a+rX", "{}", "+"],
        );
    }

    /// Every credential is a `-path` operand of the find, joined with
    /// `-o`, inside the parens that the prune applies to. One clause
    /// per credential is the whole promise: a file the widen skips is
    /// a file it cannot loosen.
    #[test]
    fn widen_prunes_every_credential() {
        let argv = widen_argv(
            "/tmp/xyz",
            &["/tmp/xyz/kuma-user".to_string(), "/tmp/xyz/kuma-restore-secret".to_string()],
        );
        assert!(argv.contains(&"-path".to_string()), "no -path operand: {}", argv.join(" "));
        assert!(
            argv.windows(2).any(|w| w == ["-path".to_string(), "/tmp/xyz/kuma-user".to_string()]),
            "the account file is not pruned: {}",
            argv.join(" ")
        );
        assert!(
            argv.windows(2)
                .any(|w| w == ["-path".to_string(), "/tmp/xyz/kuma-restore-secret".to_string()]),
            "the restore credential is not pruned: {}",
            argv.join(" ")
        );
    }

    /// The chmod is the tail, always: everything a credential could
    /// possibly interact with sits before the `-prune`, and the
    /// `-exec` names only `{}` — never a file of its own.
    #[test]
    fn the_widen_always_ends_in_the_chmod() {
        for credentials in [
            vec![],
            vec!["/tmp/xyz/kuma-user".to_string()],
            vec!["/tmp/xyz/kuma-user".to_string(), "/tmp/xyz/kuma-restore-secret".to_string()],
        ] {
            let argv = widen_argv("/tmp/xyz", &credentials);
            let tail = argv.len() - 5;
            assert_eq!(argv[tail..].join(" "), "-exec chmod a+rX {} +");
        }
    }

    /// The staging half, testable without root because only the run
    /// half needs it: a plain file round-trips, and a credential is
    /// 0600 from the moment it exists rather than a write followed by
    /// a narrowing that a crash in between would undo.
    #[test]
    fn files_stage_and_credentials_start_private() {
        use std::os::unix::fs::PermissionsExt;
        let mut root = for_root().expect("a tempdir is all this needs");
        let plain = root.file("Containerfile", "FROM scratch\n").expect("staged");
        assert_eq!(std::fs::read_to_string(&plain).unwrap(), "FROM scratch\n");
        let secret = root.credential("kuma-user", "KUMA_USER='root'\n").expect("staged");
        let mode = std::fs::metadata(&secret).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        assert!(root.path().join("kuma-user").exists());
    }
}
