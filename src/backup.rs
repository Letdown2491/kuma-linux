//! `kuma backup`: reach the offsite copies the timer makes.
//!
//! [snapshots] answers a mistake and this answers the disk, but the
//! shape of the two commands is deliberately the same: list what exists,
//! then bring one path back, dry run first. What differs is where the
//! copies live and that reaching them costs a credential, which is why
//! everything here goes through sudo and `kuma snapshot` does not.
//!
//! Seeding is a verb rather than something the timer does. The first
//! copy of a home directory is tens of gigabytes, and a background job
//! that decides to start sending those while somebody is tethered on a
//! train is a tool that gets uninstalled. Every later copy is the
//! difference since the last one, which is what a timer is for.

use crate::config::Config;
use crate::host::{host_output, run_host};
use crate::state::{action_json, print_actions, Action};
use anyhow::{bail, Result};
use std::path::Path;

/// Where the credential the declaration names lives on this machine.
///
/// The declaration names it and never holds it: a file written to be
/// committed, and baked world-readable into every image built from it,
/// is the wrong place for a secret. What the declaration does carry is
/// that a credential exists and what it is called, which is what makes
/// this path computable and therefore checkable.
pub fn secret_path(config: &Config) -> String {
    format!("/var/lib/kuma/secrets/{}.env", config.backup.secret)
}

/// Written only by a run that copied something, which is what lets
/// doctor tell a machine backing up nightly from one whose unit exits 0
/// every night having found nothing to do.
pub const STAMP: &str = "/var/lib/kuma/backup-last";

/// One door to restic, and it is `sudo sh -c` rather than a helper baked
/// into the image on purpose: `kuma install --restore` runs from live
/// media, where no image of the target machine exists yet, so a helper
/// in an image is precisely the thing that is absent when it matters
/// most. This is the same three lines wherever it runs.
///
/// Everything after `_` becomes `"$@"`, so a path with a space in it
/// stays one argument and nothing is ever handed back to a shell to
/// re-parse. The secret arrives as a path rather than as a value, so it
/// is never in argv, which `ps` shows to everybody on the machine.
fn restic_argv(secret: &str, repo: &str, args: &[&str]) -> Vec<String> {
    restic_argv_within(secret, repo, None, args)
}

/// The same door with a clock on it, for the one call that has to answer
/// rather than succeed.
///
/// restic treats a missing bucket as a transient error and retries it
/// with exponential backoff, so asking "is there a repository here" of a
/// machine nobody has seeded takes minutes to answer "no". There is no
/// flag for it: `--retry-lock` is about locks and
/// `--stuck-request-timeout` defaults to five minutes. Only the probe
/// gets a deadline; a backup or a restore is allowed to take as long as
/// it takes.
fn restic_argv_within(
    secret: &str,
    repo: &str,
    seconds: Option<u32>,
    args: &[&str],
) -> Vec<String> {
    // RESTIC_CACHE_DIR for the same reason the units set it: restic
    // treats an unopenable cache as fatal, and neither a systemd service
    // nor `sudo` can be relied on to leave a usable HOME behind. Naming
    // it also means the verb and the timer share one cache rather than
    // filling two.
    let open = format!(
        "set -a; . \"$1\"; set +a; export RESTIC_REPOSITORY=\"$2\"; \
         export RESTIC_CACHE_DIR=/var/cache/restic; install -d -m 0700 /var/cache/restic; \
         shift 2; exec {}restic \"$@\"",
        seconds.map(|s| format!("timeout {s} ")).unwrap_or_default()
    );
    let mut argv: Vec<String> =
        ["sudo", "sh", "-c", &open, "_", secret, repo].iter().map(|s| s.to_string()).collect();
    argv.extend(args.iter().map(|a| a.to_string()));
    argv
}

fn ready(config: &Config) -> Result<(String, String)> {
    if !config.backup.enable {
        bail!(
            "[backup] is not enabled in this declaration. Add it, then \
             `kuma build` and `kuma switch`."
        );
    }
    let secret = secret_path(config);
    if !Path::new(&secret).exists() {
        bail!(
            "no credential at {secret}. The declaration names it; this machine has \
             not been given it. Create it 0600 root with the repository's keys in \
             it (RESTIC_PASSWORD, and the backend's own variables), then try again."
        );
    }
    Ok((secret, config.backup.repo.clone()))
}

pub fn backup(
    config: &Config,
    init: bool,
    list: bool,
    restore: Option<&str>,
    from: Option<&str>,
    yes: bool,
    json: bool,
) -> Result<()> {
    if init {
        return seed(config);
    }
    if let Some(path) = restore {
        return restore_path(config, path, from, yes, json);
    }
    if list {
        return list_snapshots(config, json);
    }
    status(config, json)
}

/// Create the repository, then hand the first copy to the unit that
/// already knows how to make one.
///
/// Deliberately not a second implementation of the backup itself. The
/// converger knows the excludes, the snapshot to read from, the bind
/// mount that keeps restic incremental, and the stamp; a `--init` that
/// copied files itself would be a second answer to all four, free to
/// drift from the first.
fn seed(config: &Config) -> Result<()> {
    let (secret, repo) = ready(config)?;

    // No "is there one yet" probe first, deliberately. Asking costs a
    // round of restic's retry backoff on exactly the machine that has
    // never been seeded, and `restic init` gives the answer for free: it
    // creates the bucket when there is none and refuses when there is
    // already a repository, which are the only two answers the question
    // had.
    //
    // The first version did ask, and classified the reply by matching
    // restic's words in the error. It failed on a real machine, because
    // host_output keeps the last two lines of stderr and restic's last
    // two are its shutdown rather than its reason: an ordinary unseeded
    // repository read as unreachable and seeding refused itself.
    println!("Creating a repository at {repo}");
    if let Err(e) = run_host(&restic_argv(&secret, &repo, &["init"])) {
        let said = format!("{e:#}").to_lowercase();
        if said.contains("already exists") || said.contains("already initialized") {
            println!("{repo} already holds a repository; nothing to seed.");
            println!(
                "The timer copies the difference from here on. \
                 `kuma backup --list` shows what is in it."
            );
            return Ok(());
        }
        return Err(e);
    }
    println!();
    println!("Now making the first copy. This is the whole of your data rather than");
    println!("the difference since yesterday, so it takes as long as it takes.");
    run_host(&["sudo", "systemctl", "start", "kuma-backup.service"])?;
    println!();
    println!("Done. `kuma doctor` grades how fresh it stays from here.");
    Ok(())
}

/// What this machine knows without asking the network, which is the
/// same discipline doctor keeps: a status command that hangs on a train
/// is one people stop running.
fn status(config: &Config, json: bool) -> Result<()> {
    let secret = secret_path(config);
    let provisioned = Path::new(&secret).exists();
    let stamp = std::fs::read_to_string(STAMP).ok();
    let last = stamp.as_ref().and_then(|t| t.split_whitespace().nth(1).map(str::to_string));

    let mut actions: Vec<Action> = Vec::new();
    if !config.backup.enable {
        actions.push(Action::new(
            "edit",
            "$EDITOR ~/.config/kuma/kuma.toml",
            "add [backup] with a repo and a secret name, then kuma build and kuma switch",
        ));
    } else if !provisioned {
        actions.push(Action::new(
            "provision",
            format!("sudo install -m 0600 /dev/null {secret}"),
            "create the credential file the declaration names, then put the keys in it",
        ));
    } else if last.is_none() {
        actions.push(Action::new(
            "seed",
            "sudo kuma backup --init",
            "make the first copy, deliberately, while plugged in",
        ));
    } else {
        actions.push(Action::new(
            "list",
            "kuma backup --list",
            "ask the repository what it is holding",
        ));
    }

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "declared": config.backup.enable,
                "repo": config.backup.repo,
                "secret": config.backup.secret,
                "secret_path": secret,
                "provisioned": provisioned,
                "interval": config.backup.interval,
                "last_completed": last,
                "covers": config.snapshots.target,
                "network_connections": config.backup.network_connections,
                "actions": actions.iter().map(action_json).collect::<Vec<_>>(),
            }))?
        );
        return Ok(());
    }

    if !config.backup.enable {
        println!("[backup] is not enabled in this declaration.");
        print_actions(&actions);
        return Ok(());
    }
    println!("Repository  {}", config.backup.repo);
    println!("Credential  {secret}{}", if provisioned { "" } else { "   (absent)" });
    println!("Covers      {}", config.snapshots.target);
    println!(
        "            network connections {}",
        if config.backup.network_connections {
            "included"
        } else {
            "NOT included; a restore needs those passwords retyped"
        }
    );
    match &last {
        Some(when) => println!("Last copy   {when}"),
        None => println!("Last copy   never"),
    }
    print_actions(&actions);
    Ok(())
}

fn list_snapshots(config: &Config, json: bool) -> Result<()> {
    let (secret, repo) = ready(config)?;
    // Bounded, because a repository that is not there answers this the
    // slow way: restic treats a missing bucket as transient and retries
    // with backoff, so `--list` on an unseeded machine sits silent for
    // minutes. A minute is long enough for a slow link and short enough
    // to be an answer.
    let raw = host_output(&restic_argv_within(
        &secret,
        &repo,
        Some(60),
        &["snapshots", "--tag", "kuma", "--json"],
    ))?;
    if json {
        println!("{raw}");
        return Ok(());
    }
    let parsed: serde_json::Value = serde_json::from_str(&raw)?;
    let items = parsed.as_array().cloned().unwrap_or_default();
    if items.is_empty() {
        println!("{repo} holds no backups this declaration made.");
        return Ok(());
    }
    println!("{} backups in {repo}", items.len());
    for item in &items {
        let id = item["short_id"].as_str().unwrap_or("?");
        let time = item["time"].as_str().unwrap_or("");
        // restic's timestamp is RFC3339 with fractional seconds; the
        // date and the minute are what anybody reads.
        let when = time.get(..16).unwrap_or(time).replace('T', " ");
        println!("  {id}  {when}");
    }
    println!();
    print_actions(&[Action::new(
        "restore",
        "kuma backup --restore <path>",
        "bring a path back from the newest backup that has it",
    )]);
    Ok(())
}

/// File-level, and in place.
///
/// The converger mounts the snapshot over the live path before it
/// copies, so what the repository records is where the files actually
/// live. That is what makes an in-place restore possible at all: the
/// path you name is the path restic knows, and `--target /` puts it
/// back where it was rather than under a staging directory somebody then
/// has to move things out of.
///
/// Dry run by default, the same bargain `kuma snapshot --restore` makes.
/// Writing over a file that is there is the one outcome nobody can undo
/// from here, so it takes a second word.
fn restore_path(
    config: &Config,
    path: &str,
    from: Option<&str>,
    yes: bool,
    json: bool,
) -> Result<()> {
    // Before `ready`, deliberately. A path this command cannot vouch for
    // is wrong whether or not the machine has a credential, and saying
    // "provision a secret first" to somebody who typed a relative path
    // sends them to fix the wrong thing.
    if !Path::new(path).is_absolute() {
        bail!("{path} is not an absolute path; name the file as it lives on the machine");
    }
    if path.contains("..") {
        bail!("{path} contains `..`; name the path without it");
    }
    let (secret, repo) = ready(config)?;
    let snapshot = from.unwrap_or("latest");
    let live = Path::new(path).exists();

    let mut args = vec!["restore", snapshot, "--target", "/", "--include", path];
    if !yes {
        args.push("--dry-run");
    }

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "repo": repo,
                "snapshot": snapshot,
                "path": path,
                "replaces_live_copy": live,
                "dry_run": !yes,
            }))?
        );
    } else if live {
        println!(
            "{path} exists and would be replaced by the copy in {snapshot}{}",
            if yes { "" } else { " (dry run)" }
        );
    } else {
        println!("{path} would be restored from {snapshot}{}", if yes { "" } else { " (dry run)" });
    }

    run_host(&restic_argv(&secret, &repo, &args))?;
    if !yes && !json {
        println!();
        print_actions(&[Action::new(
            "write",
            format!("sudo kuma backup --restore {path} --yes"),
            "actually write it back",
        )]);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(toml: &str) -> Config {
        toml::from_str(toml).expect("valid declaration")
    }

    /// The secret is passed as a path and read inside the shell, never
    /// as a value in argv, because `ps` shows any process's arguments to
    /// every user on the machine for as long as it runs.
    #[test]
    fn the_credential_is_never_an_argument() {
        let argv = restic_argv("/var/lib/kuma/secrets/backup.env", "b2:kuma", &["snapshots"]);
        assert!(argv.iter().any(|a| a == "/var/lib/kuma/secrets/backup.env"));
        assert!(
            argv.iter().any(|a| a.contains("RESTIC_CACHE_DIR=/var/cache/restic")),
            "restic dies without somewhere to cache, and sudo's HOME is not it: {argv:?}"
        );
        assert!(
            argv.iter().all(|a| !a.contains("RESTIC_PASSWORD=")),
            "a value, not a path, would be readable by everybody: {argv:?}"
        );
        // Everything the caller asked for lands after the separator, so
        // a path with a space in it stays one argument.
        let with_space =
            restic_argv("/s.env", "b2:kuma", &["restore", "latest", "--include", "/var/home/a b"]);
        assert_eq!(with_space.last().unwrap(), "/var/home/a b");
    }

    /// Only a question gets a clock. A backup or a restore is allowed to
    /// take as long as it takes, and a deadline on those would kill a
    /// first copy of a home directory over a slow link.
    #[test]
    fn only_the_question_has_a_deadline() {
        let listing = restic_argv_within("/s.env", "b2:kuma", Some(60), &["snapshots"]);
        assert!(
            listing.iter().any(|a| a.contains("timeout 60 restic")),
            "listing an absent repository must not wait out the retry storm: {listing:?}"
        );
        let work = restic_argv("/s.env", "b2:kuma", &["backup", "/var/home"]);
        assert!(
            work.iter().all(|a| !a.contains("timeout")),
            "a copy must not be killed by a clock: {work:?}"
        );
    }

    /// A path outside the machine's own naming, or one that climbs, is
    /// refused rather than resolved: this feeds a restore that runs as
    /// root and writes at `/`.
    #[test]
    fn a_restore_refuses_a_path_it_cannot_vouch_for() {
        let declared = config(
            "schema_version = 1\n[snapshots]\nenable = true\n\
             [backup]\nenable = true\nrepo = \"b2:kuma\"\n",
        );
        for bad in ["notes.md", "/var/home/../etc/shadow"] {
            let err = restore_path(&declared, bad, None, false, false).unwrap_err().to_string();
            assert!(
                err.contains("absolute") || err.contains(".."),
                "{bad} should be refused before anything runs: {err}"
            );
        }
    }

    /// Every subcommand needs the same two things, and the message for
    /// each has to say what to do rather than what went wrong.
    #[test]
    fn an_unprovisioned_machine_is_told_what_is_missing() {
        let off = config("schema_version = 1\n");
        let err = ready(&off).unwrap_err().to_string();
        assert!(err.contains("[backup] is not enabled"), "{err}");

        let on = config(
            "schema_version = 1\n[snapshots]\nenable = true\n\
             [backup]\nenable = true\nrepo = \"b2:kuma\"\nsecret = \"nowhere-at-all\"\n",
        );
        let err = ready(&on).unwrap_err().to_string();
        assert!(err.contains("/var/lib/kuma/secrets/nowhere-at-all.env"), "{err}");
        assert!(err.contains("0600"), "the message says how to make it: {err}");
    }
}
