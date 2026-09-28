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
    /// Delete the vault. The key is unrecoverable afterwards.
    Destroy {
        /// Carry the flag; without it this is a dry run.
        #[arg(long)]
        yes: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let path = cli.socket.map(Ok).unwrap_or_else(kuma::nostr::socket::default_socket_path)?;

    let request = match cli.command {
        Command::Setup { import } => match import {
            Some(secret) => {
                format!(
                    r#"{{"cmd":"setup","mode":{{"how":"import","secret":{}}}}}"#,
                    json_string(&secret)
                )
            }
            None => r#"{"cmd":"setup","mode":{"how":"generate"}}"#.to_string(),
        },
        Command::Unlock => r#"{"cmd":"unlock"}"#.to_string(),
        Command::Lock => r#"{"cmd":"lock"}"#.to_string(),
        Command::Status => r#"{"cmd":"status"}"#.to_string(),
        Command::Destroy { yes } => format!(r#"{{"cmd":"destroy","confirm":{yes}}}"#),
    };

    let mut stream = UnixStream::connect(&path)
        .with_context(|| format!("the daemon is not answering on {}", path.display()))?;
    stream.write_all(request.as_bytes())?;
    stream.write_all(b"\n")?;

    let mut answer = String::new();
    BufReader::new(stream).read_line(&mut answer)?;
    let value: serde_json::Value = serde_json::from_str(answer.trim())
        .context("the daemon's answer was not one JSON document")?;

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
                (true, false, _) => println!("vault exists, locked"),
                (true, true, Some(npub)) => println!("vault exists, unlocked as {npub}"),
                (true, true, None) => println!("vault exists, unlocked"),
            }
        }
        Some("setup") => println!(
            "vault created, unlocked as {}",
            value["pubkey"].as_str().unwrap_or("(npub unreadable)")
        ),
        Some("unlock") => println!("unlocked"),
        Some("lock") => println!("locked"),
        Some("destroy_dry_run") => println!(
            "dry run: {}",
            value["would"].as_str().unwrap_or("this would delete the vault")
        ),
        Some("destroy") => println!("vault destroyed"),
        _ => println!("{value}"),
    }
    Ok(())
}
