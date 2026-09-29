//! `kuma-nostr` — the nostr layer's CLI.
//!
//! The human interface and the noctalia plugin's transport, in one
//! binary: every verb is one request line to the daemon's socket and one
//! answer rendered. The CLI reads no keys and holds no state — the vault
//! is the daemon's, and that separation is what lets the plugin shell
//! this binary without widening the trust boundary.
//!
//! The verbs the layer has so far: `setup`, `unlock`, `lock`, `status`,
//! `destroy`. Pairing, prompts and the bunker arrive with the policy
//! engine and ride the same socket.

use anyhow::{Context, Result};
use clap::Parser;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;

#[derive(Parser)]
#[command(name = "kuma-nostr", about = "The kumaOS nostr layer's CLI")]
struct Cli {
    #[command(subcommand)]
    command: Command,
    /// Talk to a socket somewhere other than the default, which is
    /// `$XDG_RUNTIME_DIR/kuma-nostr.sock`. Works before or after the
    /// subcommand.
    #[arg(long, global = true)]
    socket: Option<std::path::PathBuf>,
}

#[derive(clap::Subcommand)]
enum Command {
    /// Provision the vault: generate a key, or import one.
    Setup {
        /// An nsec, a hex secret key, or a NIP-06 mnemonic. Omit to
        /// generate.
        #[arg(long)]
        import: Option<String>,
    },
    /// Re-read the key from the keyring.
    Unlock,
    /// Drop the key from memory; the stored vault stays.
    Lock,
    /// What the daemon holds: whether a vault exists and is unlocked.
    Status,
    /// The `bunker://` URI a remote app pairs with — as text, and as a
    /// QR when asked. The URI carries the bunker pubkey and the relay
    /// set the daemon is running.
    Bunker {
        /// Render a QR beside the URI line.
        #[arg(long)]
        qr: bool,
    },
    /// Delete the vault. The key is unrecoverable afterwards.
    Destroy {
        /// Carry the flag; without it this is a dry run.
        #[arg(long)]
        yes: bool,
    },
    /// The asks waiting on a person, newest last.
    Prompts,
    /// Answer an ask with yes. `--remember 1` grants the same method a
    /// standing yes for an hour — the longest a remember can be.
    Approve {
        /// The ask's id, from `prompts`.
        id: String,
        /// Hours to remember, at most 1.
        #[arg(long)]
        remember: Option<u64>,
    },
    /// Answer an ask with no.
    Deny { id: String },
    /// The paired apps and their policy levels.
    Apps,
    /// Forget a paired app: it answers as unpaired from then on.
    Revoke { app: String },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let path = cli.socket.map(Ok).unwrap_or_else(kuma::nostr::socket::default_socket_path)?;

    let bunker_verb = matches!(cli.command, Command::Bunker { .. });
    let bunker_qr = matches!(cli.command, Command::Bunker { qr: true });

    let request = match &cli.command {
        Command::Setup { import } => match import {
            Some(secret) => {
                format!(
                    r#"{{"cmd":"setup","mode":{{"how":"import","secret":{}}}}}"#,
                    json_string(secret)
                )
            }
            None => r#"{"cmd":"setup","mode":{"how":"generate"}}"#.to_string(),
        },
        Command::Unlock => r#"{"cmd":"unlock"}"#.to_string(),
        Command::Lock => r#"{"cmd":"lock"}"#.to_string(),
        Command::Status => r#"{"cmd":"status"}"#.to_string(),
        Command::Bunker { .. } => r#"{"cmd":"status"}"#.to_string(),
        Command::Destroy { yes } => format!(r#"{{"cmd":"destroy","confirm":{yes}}}"#),
        Command::Prompts => r#"{"cmd":"prompts"}"#.to_string(),
        Command::Approve { id, remember } => {
            format!(
                r#"{{"cmd":"approve","id":{},"remember_hours":{}}}"#,
                json_string(id),
                remember.map_or("null".into(), |h| h.to_string())
            )
        }
        Command::Deny { id } => format!(r#"{{"cmd":"deny","id":{}}}"#, json_string(id)),
        Command::Apps => r#"{"cmd":"apps"}"#.to_string(),
        Command::Revoke { app } => format!(r#"{{"cmd":"revoke","app":{}}}"#, json_string(app)),
    };

    let mut stream = UnixStream::connect(&path)
        .with_context(|| format!("the daemon is not answering on {}", path.display()))?;
    stream.write_all(request.as_bytes())?;
    stream.write_all(b"\n")?;

    let mut answer = String::new();
    BufReader::new(stream).read_line(&mut answer)?;
    let value: serde_json::Value = serde_json::from_str(answer.trim())
        .context("the daemon's answer was not one JSON document")?;

    if bunker_verb {
        return render_bunker(&value, bunker_qr);
    }

    render(&value)
}

/// The one place the CLI formats a secret into a request line. Values
/// are JSON strings through and through — never pasted, never echoed.
fn json_string(value: &str) -> String {
    serde_json::to_string(value).expect("a string serializes")
}

fn render(value: &serde_json::Value) -> Result<()> {
    if value.get("ok").and_then(serde_json::Value::as_bool) != Some(true) {
        let error = value
            .get("error")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("the daemon refused without saying why");
        anyhow::bail!("{error}");
    }
    match value.get("verb").and_then(serde_json::Value::as_str) {
        Some("ping") => println!("daemon is answering"),
        Some("status") => {
            let vault = &value["vault"];
            let exists = vault["exists"].as_bool().unwrap_or(false);
            let unlocked = vault["unlocked"].as_bool().unwrap_or(false);
            let pubkey = vault["pubkey"].as_str();
            match (exists, unlocked, pubkey) {
                (false, _, _) => println!("no vault; run `kuma-nostr setup`"),
                (true, false, Some(npub)) => println!("vault exists, locked, identity {npub}"),
                (true, false, None) => println!("vault exists, locked"),
                (true, true, Some(npub)) => println!("vault exists, unlocked as {npub}"),
                (true, true, None) => println!("vault exists, unlocked"),
            }
        }
        Some("setup") => println!(
            "vault created, unlocked as {}",
            value["pubkey"].as_str().unwrap_or("(npub unreadable)")
        ),
        Some("unlock") => {
            println!("unlocked as {}", value["pubkey"].as_str().unwrap_or("(npub unreadable)"))
        }
        Some("lock") => println!("locked"),
        Some("destroy_dry_run") => println!(
            "dry run: {}",
            value["would"].as_str().unwrap_or("this would delete the vault")
        ),
        Some("destroy") => println!("vault destroyed"),
        Some("prompts") => {
            let prompts = value["prompts"].as_array().cloned().unwrap_or_default();
            if prompts.is_empty() {
                println!("nothing is waiting on you");
            }
            for prompt in prompts {
                println!(
                    "{}  {}  {}  {}",
                    prompt["id"].as_str().unwrap_or("?"),
                    prompt["app"].as_str().unwrap_or("?"),
                    prompt["method"].as_str().unwrap_or("?"),
                    prompt["summary"].as_str().unwrap_or("")
                );
                if let Some(detail) = prompt["detail"].as_str() {
                    println!("    {detail}");
                }
            }
        }
        Some("approve") => println!("approved"),
        Some("deny") => println!("denied"),
        Some("apps") => {
            let apps = value["apps"].as_array().cloned().unwrap_or_default();
            if apps.is_empty() {
                println!("no apps paired");
            }
            for app in apps {
                println!(
                    "{}  {:?}  paired at {}",
                    app["pubkey"].as_str().unwrap_or("?"),
                    app["level"],
                    app["paired_at"].as_u64().unwrap_or(0),
                );
            }
        }
        Some("revoke") => {
            if value["removed"].as_bool() == Some(true) {
                println!("revoked");
            } else {
                println!("no such app");
            }
        }
        _ => println!("{value}"),
    }
    Ok(())
}

/// The `bunker` verb: the pairing URI a phone's nostr app scans. The
/// URI is the copyable answer; the QR is the scannable one — both carry
/// the bunker pubkey and the relay set, because a QR that only renders
/// when the URI is not also printed is a URI nobody can paste into a
/// support question.
fn render_bunker(value: &serde_json::Value, qr: bool) -> Result<()> {
    if value.get("ok").and_then(serde_json::Value::as_bool) != Some(true) {
        anyhow::bail!(
            "the daemon refused: {}",
            value["error"].as_str().unwrap_or("no reason given")
        );
    }
    let vault = &value["vault"];
    let npub = vault["pubkey"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("the daemon has no identity yet; run `kuma-nostr setup`"))?;
    let pubkey =
        nostr::key::PublicKey::parse(npub).context("the daemon's identity did not parse")?;
    let relays: Vec<String> = vault["relays"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| r.as_str().map(str::to_string))
        .collect();
    let uri = kuma::nostr::bunker::bunker_uri(&pubkey, &relays);

    if vault["unlocked"].as_bool() != Some(true) {
        println!("the bunker is locked; the URI pairs but signs nothing until `kuma-nostr unlock`");
    }
    if qr {
        // One quiet-zone module on each side is the minimum a scanner
        // wants; the debug render is the matrix alone, so the padding
        // is printed here.
        let code = qrencode::QrCode::new(uri.as_bytes())?;
        println!();
        println!("{}", " ".repeat(code.width() + 8));
        for line in code.to_debug_str('#', ' ').lines() {
            println!("    {line}    ");
        }
        println!("{}", " ".repeat(code.width() + 8));
    }
    println!("{uri}");
    Ok(())
}
