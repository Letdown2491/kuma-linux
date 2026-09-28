//! `kuma-nostrd` — the nostr layer's daemon.
//!
//! A small executable whose surface an auditor can hold in their head:
//! it owns the vault and answers the socket protocol, nothing else. It
//! runs as a user unit under the graphical session (the hardened unit
//! the declaration block bakes), takes at most one argument — the socket
//! path, for tests — and everything else is the protocol, which is where
//! the reading is.

use std::sync::{Arc, Mutex};

use anyhow::Context;
use kuma::nostr::protocol::Daemon;
use kuma::nostr::socket;
use kuma::nostr::vault::{KeyringStore, Vault};

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let socket_path = match args.next() {
        Some(path) => std::path::PathBuf::from(path),
        None => socket::default_socket_path()?,
    };
    if let Some(extra) = args.next() {
        anyhow::bail!("unexpected argument {extra:?}: the daemon takes at most a socket path");
    }

    // The runtime exists for oo7's keyring calls and nothing else: the
    // socket loop is blocking threads, and each request bridges into
    // this runtime for the duration of its store work.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("building the keyring runtime")?;

    let vault = Vault::new(KeyringStore);
    let daemon = Arc::new(Mutex::new(Daemon::new(vault)));

    eprintln!("kuma-nostrd: listening on {}", socket_path.display());
    let listener = socket::bind(&socket_path)?;
    socket::serve(listener, daemon, &runtime);
    // serve() only returns on an accept failure, which is fatal here.
    Ok(())
}
