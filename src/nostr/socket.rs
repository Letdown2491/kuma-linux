//! The unix socket the daemon answers on, and the framing both ends of
//! the layer agree on.
//!
//! One request line in, one response line out. Reaching the socket
//! grants nothing by itself: the socket lives under `XDG_RUNTIME_DIR`
//! (0700, user-owned), is chmod 0600, and a peer whose uid is not the
//! daemon's own is dropped before their first byte is read. The last
//! check is what "the paired socket is the local channel" leans on —
//! a same-uid process is inside the trust boundary already, and
//! anything else is not getting past the kernel.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Context, Result};
use tokio::runtime::Handle;

use super::protocol::{self, Daemon};
use super::vault::SecretStore;

/// The socket path under the session's runtime directory. The same name
/// the unit will own, the CLI will look for, and the doctor's socket
/// check will probe.
pub const SOCKET_NAME: &str = "kuma-nostr.sock";

/// Where the socket lives: the session's runtime directory, which is the
/// one place a per-user, session-scoped, root-owned-by-nobody file
/// belongs. Refuses to guess when the session did not say.
pub fn default_socket_path() -> Result<PathBuf> {
    let dir = std::env::var("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .map_err(|_| anyhow!("XDG_RUNTIME_DIR is not set: run the daemon inside a user session"))?;
    Ok(dir.join(SOCKET_NAME))
}

/// Bind the listening socket: unlink a stale path first (a daemon that
/// died without cleanup must not wedge the next one), bind, chmod 0600.
pub fn bind(path: &Path) -> Result<UnixListener> {
    if path.exists() {
        std::fs::remove_file(path)
            .with_context(|| format!("removing the stale socket at {}", path.display()))?;
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let listener =
        UnixListener::bind(path).with_context(|| format!("binding {}", path.display()))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("chmod 0600 on {}", path.display()))?;
    Ok(listener)
}

/// Serve connections until the process is signalled. Each connection is
/// one thread; the daemon behind them is a mutex, because the verbs are
/// short and the vault's answer must be the vault's truth.
pub fn serve<S: SecretStore + Send + 'static>(
    listener: UnixListener,
    daemon: Arc<Mutex<Daemon<S>>>,
    runtime: &tokio::runtime::Runtime,
) {
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let daemon = daemon.clone();
                let runtime = runtime.handle().clone();
                if !peer_is_self(&stream) {
                    eprintln!("kuma-nostrd: refused a peer that is not this user");
                    continue;
                }
                std::thread::spawn(move || {
                    if let Err(e) = one_connection(stream, daemon, &runtime) {
                        eprintln!("kuma-nostrd: connection ended: {e}");
                    }
                });
            }
            Err(e) => {
                eprintln!("kuma-nostrd: accepting on the socket failed: {e}");
                return;
            }
        }
    }
}

/// One connection: lines until the peer hangs up. A read error ends the
/// connection; a request error is *answered* — the caller is told their
/// line was refused, and the connection lives.
fn one_connection<S: SecretStore>(
    stream: UnixStream,
    daemon: Arc<Mutex<Daemon<S>>>,
    runtime: &Handle,
) -> Result<()> {
    let mut writer = stream.try_clone().context("cloning the socket for writing")?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    loop {
        line.clear();
        let read = reader.read_line(&mut line)?;
        if read == 0 {
            return Ok(());
        }
        let answer = match protocol::decode(line.trim_end()) {
            Ok(request) => {
                let mut daemon = daemon.lock().expect("daemon lock");
                runtime.block_on(daemon.handle(request))
            }
            Err(e) => protocol::err_response(e),
        };
        writer.write_all(protocol::encode(&answer).as_bytes())?;
        writer.flush()?;
    }
}

/// Whether the process on the other end runs as this daemon's own uid.
/// The socket's 0600 in a 0700 directory is the first wall; this is the
/// second, for filesystems where one of those facts was weaker than the
/// declaration assumed.
fn peer_is_self(stream: &UnixStream) -> bool {
    use std::os::fd::AsRawFd;
    let mut cred = libc::ucred { pid: 0, uid: 0, gid: 0 };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: SO_PEERCRED with a ucred-sized buffer is the documented
    // form; the kernel fills exactly len bytes.
    let ok = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut cred as *mut libc::ucred).cast(),
            &mut len,
        )
    };
    ok == 0 && cred.uid == unsafe { libc::getuid() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nostr::vault::{MemoryStore, Vault};

    /// The socket round-trip: a real unix socket in a temp directory, a
    /// daemon over an in-memory store, a client that is a plain thread.
    /// This is the one test that proves framing, refusal, and the answer
    /// shape survive the actual kernel.
    #[test]
    fn a_client_drives_the_whole_life_cycle_over_a_real_socket() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(SOCKET_NAME);
        let listener = bind(&path).unwrap();

        // The file the bind left behind is mode 0600, because anything
        // wider quietly re-draws the trust boundary the uid check draws.
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);

        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let daemon = Arc::new(Mutex::new(Daemon::new(Vault::new(MemoryStore::default()))));
        std::thread::spawn(move || {
            serve(listener, daemon, &runtime);
        });

        let mut client = UnixStream::connect(&path).unwrap();
        let ask = |mut client: &UnixStream, line: &str| -> String {
            client.write_all(line.as_bytes()).unwrap();
            client.write_all(b"\n").unwrap();
            let mut answer = String::new();
            BufReader::new(client.try_clone().unwrap()).read_line(&mut answer).unwrap();
            answer
        };

        // Two setup verbs, one refused: the second vault answer comes
        // back as a failure document, and the connection lives on.
        let first = ask(&mut client, r#"{"cmd":"setup","mode":{"how":"generate"}}"#);
        assert!(first.contains("\"pubkey\":\"npub1"), "{first}");
        let second = ask(&mut client, r#"{"cmd":"setup","mode":{"how":"generate"}}"#);
        assert!(second.contains("\"ok\":false"), "{second}");
        let ping = ask(&mut client, r#"{"cmd":"ping"}"#);
        assert!(ping.contains("\"ok\":true"));

        // A garbage line is answered as a refusal, and the client still
        // gets an answer to the next line on the same connection.
        let junk = ask(&mut client, "this is not json");
        assert!(junk.contains("\"ok\":false"));
        let still_alive = ask(&mut client, r#"{"cmd":"ping"}"#);
        assert!(still_alive.contains("\"ok\":true"));

        // Destroy without confirm is a dry run that destroys nothing.
        let dry = ask(&mut client, r#"{"cmd":"destroy"}"#);
        assert!(dry.contains("unrecoverable"));
        let status = ask(&mut client, r#"{"cmd":"status"}"#);
        assert!(status.contains("\"exists\":true"));
    }
}
