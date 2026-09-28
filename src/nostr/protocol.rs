//! The socket protocol and the request handler behind it.
//!
//! The shape is the house response shape (docs/agents.md): every answer
//! carries `ok` first, a failure carries `error`, and a caller that reads
//! one document reads them all. Requests are newline-delimited JSON on a
//! unix socket; the verbs here are the vault's, and they will be joined —
//! not changed — by the policy and pairing verbs the policy engine
//! brings, because the CLI and the noctalia plugin both talk to this one
//! surface and a verb that changes meaning under a plugin is a bug that
//! ships twice.
//!
//! Everything here is offline-testable: [`Daemon`] is generic over the
//! vault's store, and the socket layer at the bottom of the stack is the
//! only thing that knows a network exists.

use anyhow::{anyhow, Result};
use nostr::key::SecretKey;
use serde::{Deserialize, Serialize};

use super::keys;
use super::vault::Vault;

/// A request, one line of JSON. `confirm` is the socket spelling of the
/// house `--yes`: destructive verbs answer a dry run without it.
#[derive(Debug, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    Ping,
    Status,
    /// Provision the vault: generated, or imported from an nsec, hex
    /// secret key, or NIP-06 mnemonic.
    Setup {
        mode: SetupMode,
    },
    Unlock,
    Lock,
    /// Deletes the vault. `confirm` defaults to false, so a bare
    /// destroy is the dry run — the cost is named before it is paid.
    Destroy {
        #[serde(default)]
        confirm: bool,
    },
}

#[derive(Debug, Deserialize)]
#[serde(tag = "how", rename_all = "snake_case")]
pub enum SetupMode {
    Generate,
    Import { secret: String },
}

/// An answer, one line of JSON, `ok` first.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum Response {
    Ok(OkResponse),
    Err(ErrResponse),
}

#[derive(Debug, Serialize)]
pub struct ErrResponse {
    pub ok: bool,
    pub error: String,
}

/// The `ok: true` variants. Untagged serialization keeps the wire shape
/// exactly as wide as the verb that answered, so `ping` carries nothing
/// and `status` carries the facts a caller renders.
#[derive(Debug, Serialize)]
#[serde(tag = "verb", rename_all = "snake_case")]
pub enum OkResponse {
    Ping { ok: bool },
    Status { ok: bool, vault: VaultFact },
    Setup { ok: bool, pubkey: String },
    Unlock { ok: bool },
    Lock { ok: bool },
    DestroyDryRun { ok: bool, would: String },
    Destroy { ok: bool },
}

/// What `status` says, and what `doctor` will grade through it later.
#[derive(Debug, Serialize)]
pub struct VaultFact {
    /// A vault exists in the store at all.
    pub exists: bool,
    /// The gate is open in this daemon.
    pub unlocked: bool,
    /// The bunker's public identity, when unlocked. An npub — the form
    /// everything downstream renders — never the raw key.
    pub pubkey: Option<String>,
}

/// One line in, one line out, over a newline.
pub fn decode(line: &str) -> Result<Request> {
    serde_json::from_str(line).map_err(|e| anyhow!("unparsable request: {e}"))
}

pub fn encode(response: &Response) -> String {
    let mut line = serde_json::to_string(response).expect("responses serialize");
    line.push('\n');
    line
}

/// The daemon's brain: a vault plus the answers to the verbs that drive
/// it. The socket layer calls one method per connection message; tests
/// call the same method with no socket involved.
pub struct Daemon<S: super::vault::SecretStore> {
    vault: Vault<S>,
}

impl<S: super::vault::SecretStore> Daemon<S> {
    pub fn new(vault: Vault<S>) -> Self {
        Self { vault }
    }

    pub async fn handle(&mut self, request: Request) -> Response {
        match request {
            Request::Ping => Response::Ok(OkResponse::Ping { ok: true }),
            Request::Status => {
                // A store failure is not "no vault" — saying so would turn
                // a dead keyring into advice to run setup, and a second
                // setup would then be refused on a vault nobody can see.
                // The failure is the answer.
                match self.vault.stored().await {
                    Ok(exists) => {
                        let unlocked = self.vault.is_unlocked();
                        let pubkey = self.vault.key().map(public_key_bech32);
                        Response::Ok(OkResponse::Status {
                            ok: true,
                            vault: VaultFact { exists, unlocked, pubkey },
                        })
                    }
                    Err(e) => err_response(anyhow!("cannot read the vault: {e}")),
                }
            }
            Request::Setup { mode } => self.setup(mode).await,
            Request::Unlock => self.unlock().await,
            Request::Lock => {
                self.vault.lock();
                Response::Ok(OkResponse::Lock { ok: true })
            }
            Request::Destroy { confirm } => {
                if !confirm {
                    return Response::Ok(OkResponse::DestroyDryRun {
                        ok: true,
                        would: "delete the vault from the keyring; the key is \
                                unrecoverable afterwards"
                            .into(),
                    });
                }
                match self.vault.destroy().await {
                    Ok(()) => Response::Ok(OkResponse::Destroy { ok: true }),
                    Err(e) => err_response(e),
                }
            }
        }
    }

    async fn setup(&mut self, mode: SetupMode) -> Response {
        let key = match mode {
            SetupMode::Generate => SecretKey::generate(),
            SetupMode::Import { secret } => match keys::import(&secret) {
                Ok(key) => key,
                Err(e) => return err_response(anyhow!("import failed: {e}")),
            },
        };
        match self.vault.setup(&key).await {
            Ok(()) => Response::Ok(OkResponse::Setup { ok: true, pubkey: public_key_bech32(&key) }),
            Err(e) => err_response(anyhow!("{e}")),
        }
    }

    async fn unlock(&mut self) -> Response {
        match self.vault.unlock().await {
            Ok(()) => Response::Ok(OkResponse::Unlock { ok: true }),
            Err(e) => err_response(anyhow!("{e}")),
        }
    }
}

/// The bunker's public identity in the form everything downstream
/// renders. Deriving it from the key rather than storing it means a
/// stored blob never has a second copy of a public value to disagree
/// with itself.
fn public_key_bech32(key: &SecretKey) -> String {
    use nostr::nips::nip19::ToBech32;
    nostr::key::Keys::new(key.clone()).public_key().to_bech32().expect("an npub encodes")
}

pub fn err_response(error: anyhow::Error) -> Response {
    Response::Err(ErrResponse { ok: false, error: error.to_string() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nostr::vault::{MemoryStore, SecretStore, Vault};

    async fn daemon() -> Daemon<MemoryStore> {
        Daemon::new(Vault::new(MemoryStore::default()))
    }

    async fn round_trip(request: &str) -> String {
        let mut daemon = daemon().await;
        let response = daemon.handle(decode(request).unwrap()).await;
        encode(&response)
    }

    #[tokio::test]
    async fn ping_answers_and_carries_nothing() {
        assert_eq!(round_trip(r#"{"cmd":"ping"}"#).await, "{\"verb\":\"ping\",\"ok\":true}\n");
    }

    #[tokio::test]
    async fn an_unparsable_line_is_an_error_not_a_crash() {
        // The verb is unknown, so the request never reaches the daemon:
        // decode refuses, and the socket layer renders that refusal in
        // the one shape a caller can read.
        assert!(decode(r#"{"cmd":"nope"}"#).is_err());
        let line = encode(&err_response(anyhow!("unparsable request")));
        assert!(line.contains("\"ok\":false"));
        assert!(line.contains("unparsable request"));
    }

    #[tokio::test]
    async fn the_status_walks_the_whole_life_cycle() {
        let mut daemon = daemon().await;

        let no_vault = daemon.handle(decode(r#"{"cmd":"status"}"#).unwrap()).await;
        assert!(!encode(&no_vault).contains("\"exists\":true"));

        let generated =
            daemon.handle(decode(r#"{"cmd":"setup","mode":{"how":"generate"}}"#).unwrap()).await;
        let line = encode(&generated);
        assert!(line.contains("\"pubkey\":\"npub1"), "setup answers with an npub: {line}");

        let locked = daemon.handle(decode(r#"{"cmd":"lock"}"#).unwrap()).await;
        assert!(encode(&locked).contains("\"ok\":true"));
        let status = daemon.handle(decode(r#"{"cmd":"status"}"#).unwrap()).await;
        let line = encode(&status);
        assert!(line.contains("\"exists\":true"));
        assert!(line.contains("\"unlocked\":false"));
        assert!(line.contains("\"pubkey\":null"), "a locked vault answers no pubkey: {line}");

        daemon.handle(decode(r#"{"cmd":"unlock"}"#).unwrap()).await;
        let status = daemon.handle(decode(r#"{"cmd":"status"}"#).unwrap()).await;
        assert!(encode(&status).contains("\"unlocked\":true"));
    }

    #[tokio::test]
    async fn status_never_mistakes_a_dead_store_for_an_empty_one() {
        // A store that fails is the one status answer that must not be
        // rendered as a fact: "no vault" invites a setup that would be
        // refused, and the refusal would name a vault that exists.
        struct FailingStore;
        impl SecretStore for FailingStore {
            async fn load(&self) -> Result<Option<Vec<u8>>> {
                Err(anyhow!("the keyring is not answering"))
            }
            async fn save(&self, _: &[u8]) -> Result<()> {
                Err(anyhow!("the keyring is not answering"))
            }
            async fn remove(&self) -> Result<()> {
                Err(anyhow!("the keyring is not answering"))
            }
        }
        let mut daemon = Daemon::new(Vault::new(FailingStore));
        let response = daemon.handle(decode(r#"{"cmd":"status"}"#).unwrap()).await;
        let line = encode(&response);
        assert!(line.contains("\"ok\":false"), "{line}");
        assert!(line.contains("cannot read the vault"), "{line}");
    }

    #[tokio::test]
    async fn setup_refuses_a_second_vault_and_import_rejects_junk() {
        let mut daemon = daemon().await;
        let request = r#"{"cmd":"setup","mode":{"how":"generate"}}"#;
        daemon.handle(decode(request).unwrap()).await;
        let second = daemon.handle(decode(request).unwrap()).await;
        assert!(encode(&second).contains("\"ok\":false"));

        let junk = daemon
            .handle(
                decode(r#"{"cmd":"setup","mode":{"how":"import","secret":"garbage"}}"#).unwrap(),
            )
            .await;
        assert!(encode(&junk).contains("\"ok\":false"));
    }

    #[tokio::test]
    async fn destroy_is_a_dry_run_until_confirmed() {
        let mut daemon = daemon().await;
        daemon.handle(decode(r#"{"cmd":"setup","mode":{"how":"generate"}}"#).unwrap()).await;

        let dry = daemon.handle(decode(r#"{"cmd":"destroy","confirm":false}"#).unwrap()).await;
        let line = encode(&dry);
        assert!(line.contains("unrecoverable"), "the dry run names the cost: {line}");
        let after_dry = daemon.handle(decode(r#"{"cmd":"status"}"#).unwrap()).await;
        assert!(encode(&after_dry).contains("\"exists\":true"), "a dry run destroys nothing");

        let gone = daemon.handle(decode(r#"{"cmd":"destroy","confirm":true}"#).unwrap()).await;
        assert!(encode(&gone).contains("\"ok\":true"));
        let status = daemon.handle(decode(r#"{"cmd":"status"}"#).unwrap()).await;
        assert!(encode(&status).contains("\"exists\":false"));
    }
}
