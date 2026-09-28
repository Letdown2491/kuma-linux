//! `kuma-nostrd` — the nostr layer's daemon.
//!
//! A small executable whose surface an auditor can hold in their head:
//! it owns the vault, arms the bunker when the gate is open, and
//! answers the socket protocol. It runs as a user unit under the
//! graphical session (the hardened unit the declaration block bakes);
//! the relay set comes in as arguments until the declaration block
//! exists to carry it.
//!
//! The startup posture is the plan's gate made concrete: the keyring is
//! PAM-unlocked by the session, so the daemon auto-unlocks and comes up
//! answering — a reboot is invisible to a paired phone. A vault that
//! will not open says so on stderr and the daemon keeps running locked;
//! `kuma-nostr status` and the doctor both say so.

use std::sync::{Arc, Mutex};

use anyhow::Context;
use clap::Parser;
use kuma::nostr::protocol::Daemon;
use kuma::nostr::socket;
use kuma::nostr::vault::{KeyringStore, Vault};

#[derive(Parser)]
#[command(name = "kuma-nostrd", about = "The kumaOS nostr layer's daemon", verbatim_doc_comment)]
struct Args {
    /// The socket to answer on; the default is
    /// `$XDG_RUNTIME_DIR/kuma-nostr.sock`.
    #[arg(long)]
    socket: Option<std::path::PathBuf>,
    /// A relay to talk to, as many times as the set needs. Until the
    /// declaration block carries the relay list, this argument is the
    /// only way one exists.
    #[arg(long = "relay")]
    relays: Vec<String>,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let socket_path = match &args.socket {
        Some(path) => path.clone(),
        None => socket::default_socket_path()?,
    };

    // The runtime exists for oo7's keyring calls and the bunker's async
    // decisions and nothing else: the socket loop is blocking threads,
    // and each request bridges into this runtime for its duration.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("building the keyring runtime")?;

    let (daemon, inbound_rx) = Daemon::new(Vault::new(KeyringStore), args.relays.clone());
    let mut daemon = daemon;

    // The startup posture: come up answering. A vault that will not
    // open is a locked daemon, not a dead one.
    match runtime.block_on(daemon.startup_unlock()) {
        Ok(npub) => eprintln!("kuma-nostrd: vault unlocked, bunker live as {npub}"),
        Err(e) => {
            eprintln!("kuma-nostrd: starting locked: {e:#}");
        }
    }

    let daemon = Arc::new(Mutex::new(daemon));

    // The bunker worker: relay-delivered events in, answers published
    // out, under the same lock the socket verbs hold. It lives for the
    // process; a lock just makes its answers None until the next
    // unlock.
    let worker = daemon.clone();
    let runtime_handle = runtime.handle().clone();
    std::thread::spawn(move || loop {
        let Ok(event) = inbound_rx.recv() else {
            return;
        };
        let mut daemon = worker.lock().expect("the daemon lock");
        if let Some(response) = runtime_handle
            .block_on(daemon.process_bunker_event(event, &kuma::nostr::bunker::DenyAll))
        {
            if let Err(e) = daemon.publish(&response) {
                eprintln!("kuma-nostrd: the answer was not published: {e:#}");
            }
        }
    });

    eprintln!("kuma-nostrd: listening on {}", socket_path.display());
    let listener = socket::bind(&socket_path)?;
    socket::serve(listener, daemon, &runtime);
    // serve() only returns on an accept failure, which is fatal here.
    Ok(())
}
