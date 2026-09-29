//! The vault: where the daemon's nostr key lives when it is not in
//! memory.
//!
//! The design is the plan's gate-style lock (notes/44.4.0-plan.md, item
//! 1). The Secret Service's login collection is the wall — it is already
//! PAM-unlocked at greetd, so a fresh machine needs no second secret and
//! no new passphrase UX — and the vault is honest about that being a
//! gate: `lock` drops the key from memory and refuses to sign, `unlock`
//! re-reads it, and neither pretends to survive an attacker who is
//! already running as the user with the session unlocked. The upgrade to
//! an independent-passphrase vault later is a change of what fills the
//! same blob, not a schema change.
//!
//! What the keyring holds is not a bare secret key but a NIP-49
//! `ncryptsec` wrapped with a random passphrase carried beside it. Today
//! that wrap adds nothing the keyring does not already provide — the
//! wall and the wrap live in the same item — and that is the point: the
//! stored format is the format the future mode needs, so upgrading the
//! wall never migrates data.
//!
//! Storage itself sits behind [`SecretStore`] so every behavior the
//! daemon will get is testable offline against an in-memory store; the
//! real backend is the oo7 adapter at the bottom of this file, which is
//! compile-checked here and exercised by the smoke stage on a machine
//! that has a Secret Service.

use anyhow::{anyhow, bail, Context, Result};
use nostr::key::PublicKey;
use nostr::key::SecretKey;
use nostr::nips::nip19::ToBech32;
use serde::{Deserialize, Serialize};

use super::keys;

/// The label the Secret Service item carries, and the attributes it is
/// found by. One vault per login collection: the daemon has one key, the
/// thing a paired app is talking to is *the* bunker, and a second
/// concurrent vault is a way to sign with the wrong identity.
pub const VAULT_LABEL: &str = "kuma-nostr vault";

/// The attributes every store operation searches and writes by. These
/// are metadata the Secret Service holds in the clear; they name the
/// item, never its contents.
pub const VAULT_ATTRIBUTES: [(&str, &str); 2] = [("app", "kuma"), ("account", "nostr-vault")];

/// The bytes inside the keyring item. Versioned so the
/// independent-passphrase upgrade is a new version beside the old one
/// rather than a reinterpretation of the same bytes.
///
/// `wrap` is the random passphrase the `ncryptsec` was wrapped with —
/// kept here, beside it, because in gate mode the wall is the keyring
/// and a second item would be one more thing to lose. The independent
/// vault removes this field and asks a person instead; the `ncryptsec`
/// format does not change.
///
/// `pubkey` rides in the clear because it is the one value that is not
/// secret — the public half of the key — and because a locked daemon
/// still owes the surfaces an identity: `status` names the npub, and
/// the doctor grades the bunker without asking the gate to open.
#[derive(Serialize, Deserialize)]
struct VaultBlob {
    v: u8,
    wrap: String,
    ncryptsec: String,
    pubkey: String,
    /// The pairing nonce the bunker URI carries and a connecting app
    /// must echo. It is not a deep secret — it rides in the URI a
    /// person copies — but it is the difference between a scraped
    /// pubkey and a read pairing sheet: without it, anyone on a public
    /// relay can open asks on the daemon; with it, the connect is
    /// refused before the person ever sees a prompt. Version 2 added
    /// it; a version 1 blob migrates on first read, the secret minted
    /// and persisted beside the key it guards.
    #[serde(default)]
    secret: Option<String>,
}

const BLOB_VERSION: u8 = 2;
/// The version the secret joined at. A blob at this version or the one
/// before it loads; anything else is refused, because a future format
/// read as this one is a key silently misread.
const BLOB_VERSION_BEFORE_SECRET: u8 = 1;

impl VaultBlob {
    fn new(key: &SecretKey, wrap: String, secret: String) -> Result<Self> {
        let ncryptsec = keys::to_ncryptsec(key, &wrap)?.to_bech32()?;
        let pubkey = keys::public_key_hex(key);
        Ok(Self { v: BLOB_VERSION, wrap, ncryptsec, pubkey, secret: Some(secret) })
    }

    fn decode(bytes: &[u8]) -> Result<Self> {
        let blob: Self = serde_json::from_slice(bytes)?;
        if blob.v != BLOB_VERSION && blob.v != BLOB_VERSION_BEFORE_SECRET {
            bail!("vault blob is version {}, this binary reads {BLOB_VERSION}", blob.v);
        }
        Ok(blob)
    }
}

/// Where the vault's bytes live. Async because the real backend is the
/// Secret Service over D-Bus; every implementor is honest about failing
/// when the service is absent rather than pretending to have stored
/// something.
///
/// Not dyn-compatible on purpose: the vault is generic over its store,
/// and the daemon holds its backend as a concrete type. An object-safe
/// seam would invite runtime backends, and the choice of wall is not a
/// runtime decision. The `async fn in trait` form is kept over the
/// desugared `impl Future + Send` spelling for the same reason in the
/// other direction: these futures are awaited on the runtime that owns
/// the store and are never shipped across a thread, and a caller that
/// someday needs `Send` should be making that a reviewed signature
/// change, not inheriting a silent bound.
#[allow(async_fn_in_trait)]
pub trait SecretStore {
    /// The stored payload, or `None` when no vault exists here yet.
    async fn load(&self) -> Result<Option<Vec<u8>>>;
    /// Write the payload, replacing whatever was there.
    async fn save(&self, payload: &[u8]) -> Result<()>;
    /// Forget the vault entirely. The key it held is gone when this
    /// returns; that is what "delete the vault" means and the caller
    /// should have said so already.
    async fn remove(&self) -> Result<()>;
}

/// An in-memory store: the vault's behaviors without a Secret Service.
/// What the offline suite runs against, and what a future session-only
/// mode would want if one ever earns its keep.
#[derive(Default)]
pub struct MemoryStore(std::sync::Mutex<Option<Vec<u8>>>);

/// Shared ownership, so a test can hold the store a vault borrows and
/// read what the vault wrote — the migration proof's window.
impl SecretStore for std::sync::Arc<MemoryStore> {
    async fn load(&self) -> Result<Option<Vec<u8>>> {
        (**self).load().await
    }

    async fn save(&self, payload: &[u8]) -> Result<()> {
        (**self).save(payload).await
    }

    async fn remove(&self) -> Result<()> {
        (**self).remove().await
    }
}

impl SecretStore for MemoryStore {
    async fn load(&self) -> Result<Option<Vec<u8>>> {
        Ok(self.0.lock().expect("memory store lock").clone())
    }

    async fn save(&self, payload: &[u8]) -> Result<()> {
        *self.0.lock().expect("memory store lock") = Some(payload.to_vec());
        Ok(())
    }

    async fn remove(&self) -> Result<()> {
        *self.0.lock().expect("memory store lock") = None;
        Ok(())
    }
}

impl MemoryStore {
    /// The stored bytes, for the tests that prove what a migration
    /// wrote.
    #[cfg(test)]
    fn peek(&self) -> Option<Vec<u8>> {
        self.0.lock().expect("memory store lock").clone()
    }
}

/// The gate. Holds the store and — when unlocked — the key. Cloning the
/// key out is deliberately not offered: callers sign through the vault
/// so that a lock is a lock, and the daemon's request loop will borrow
/// for the duration of one signature.
pub struct Vault<S: SecretStore> {
    store: S,
    key: Option<SecretKey>,
    /// The pairing nonce from the blob, when one has been read or
    /// minted. Not a deep secret — it rides in the URI — but it is the
    /// daemon's to hand out and nobody else's to guess.
    secret: Option<String>,
}

impl<S: SecretStore> Vault<S> {
    pub fn new(store: S) -> Self {
        Self { store, key: None, secret: None }
    }

    pub fn is_unlocked(&self) -> bool {
        self.key.is_some()
    }

    /// The pairing nonce, when the blob has said it. The daemon builds
    /// the bunker URI from this and the connect path verifies against
    /// it; `None` before the blob's first read or on a store that has
    /// nothing yet.
    pub fn secret(&self) -> Option<&str> {
        self.secret.as_deref()
    }

    /// Whether a vault exists in the store, without opening it: what
    /// `status` reports and what distinguishes "locked" from "never set
    /// up" for every caller downstream.
    pub async fn stored(&self) -> Result<bool> {
        Ok(self.store.load().await?.is_some())
    }

    /// The bunker's public identity, read from the blob without
    /// unlocking: what `status` names while locked and what the doctor
    /// grades without asking the gate to open. `None` when no vault
    /// exists.
    pub async fn stored_pubkey(&self) -> Result<Option<PublicKey>> {
        match self.store.load().await? {
            Some(bytes) => {
                let blob = VaultBlob::decode(&bytes)?;
                let pubkey = PublicKey::parse(&blob.pubkey)
                    .map_err(|e| anyhow!("the stored pubkey is broken: {e}"))?;
                Ok(Some(pubkey))
            }
            None => Ok(None),
        }
    }

    /// The key, borrowed only while unlocked. A locked vault answers
    /// `None`, and every caller treats that as the refusal it is.
    pub fn key(&self) -> Option<&SecretKey> {
        self.key.as_ref()
    }

    /// First provisioning: wrap the key and store it, leaving the vault
    /// unlocked. Refuses to overwrite an existing vault — replacing a
    /// key is `destroy` followed by `setup`, spelled, because the
    /// mistake it prevents is signing under an identity nobody
    /// remembers choosing.
    pub async fn setup(&mut self, key: &SecretKey) -> Result<()> {
        if self.store.load().await?.is_some() {
            bail!("a vault already exists; destroy it first");
        }
        self.store_and_unlock(key).await
    }

    /// Re-read the key from storage. Idempotent on an already-unlocked
    /// vault — the CLI verb answers "already unlocked" the same way.
    /// A version 1 blob migrates here: the secret minted and persisted
    /// beside the key, because a blob that will carry a pairing nonce
    /// must do so durably, not per-boot.
    pub async fn unlock(&mut self) -> Result<()> {
        if self.key.is_some() {
            return Ok(());
        }
        let bytes =
            self.store.load().await?.ok_or_else(|| anyhow!("no vault exists in this store"))?;
        let mut blob = VaultBlob::decode(&bytes)?;
        let key = keys::decrypt_ncryptsec(&blob.ncryptsec, &blob.wrap)?;
        if blob.secret.is_none() {
            blob.secret = Some(generate_wrap()?);
            blob.v = BLOB_VERSION;
            self.store
                .save(&serde_json::to_vec(&blob).context("serializing the vault blob")?)
                .await?;
        }
        self.secret = blob.secret;
        self.key = Some(key);
        Ok(())
    }

    /// Drop the key from memory. The blob stays; `unlock` brings the
    /// same identity back.
    pub fn lock(&mut self) {
        self.key = None;
    }

    /// Mint a fresh pairing nonce and persist it. The key is untouched
    /// — rotation is a front-door surgery, not a re-provisioning — and
    /// every URI printed before this call points at a nonce the bunker
    /// no longer answers.
    pub async fn rotate_secret(&mut self) -> Result<String> {
        let bytes =
            self.store.load().await?.ok_or_else(|| anyhow!("no vault exists in this store"))?;
        let mut blob = VaultBlob::decode(&bytes)?;
        let fresh = generate_wrap()?;
        blob.secret = Some(fresh.clone());
        blob.v = BLOB_VERSION;
        self.store.save(&serde_json::to_vec(&blob).context("serializing the vault blob")?).await?;
        self.secret = Some(fresh.clone());
        Ok(fresh)
    }

    /// Forget the vault: the stored blob and the in-memory key both go.
    /// The key is unrecoverable afterwards, which is the contract; the
    /// pairing nonce goes with it, because a URI that outlived its
    /// vault would point at nothing.
    pub async fn destroy(&mut self) -> Result<()> {
        self.lock();
        self.secret = None;
        self.store.remove().await
    }

    /// The shared body of `setup` and a future import-with-replace:
    /// wrap, store, hold.
    async fn store_and_unlock(&mut self, key: &SecretKey) -> Result<()> {
        let wrap = generate_wrap()?;
        let secret = generate_wrap()?;
        let blob = VaultBlob::new(key, wrap, secret)?;
        self.store.save(&serde_json::to_vec(&blob).context("serializing the vault blob")?).await?;
        self.secret = blob.secret;
        self.key = Some(key.clone());
        Ok(())
    }
}

/// The random wrap passphrase: 32 bytes from the OS RNG, hex-encoded.
/// Hex rather than anything pronounceable — nobody is asked to type it,
/// and its job is to be long enough that NIP-49's passphrase stretching
/// is never the weak link.
fn generate_wrap() -> Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| anyhow!("OS randomness unavailable: {e}"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// The real store: the Secret Service's login collection through oo7.
/// A new connection per operation, deliberately — vault operations are
/// rare (setup, unlock, destroy), and a held D-Bus connection is one
/// more file descriptor for the hardened unit to justify.
pub struct KeyringStore;

impl SecretStore for KeyringStore {
    async fn load(&self) -> Result<Option<Vec<u8>>> {
        let keyring = oo7::Keyring::new()
            .await
            .map_err(|e| anyhow!("cannot reach the Secret Service: {e}"))?;
        let items = keyring
            .search_items(&VAULT_ATTRIBUTES)
            .await
            .map_err(|e| anyhow!("cannot search the login collection: {e}"))?;
        match items.first() {
            Some(item) => {
                let secret = item
                    .secret()
                    .await
                    .map_err(|e| anyhow!("the vault item would not open: {e}"))?;
                Ok(Some(secret.as_bytes().to_vec()))
            }
            None => Ok(None),
        }
    }

    async fn save(&self, payload: &[u8]) -> Result<()> {
        let keyring = oo7::Keyring::new()
            .await
            .map_err(|e| anyhow!("cannot reach the Secret Service: {e}"))?;
        keyring
            .create_item(VAULT_LABEL, &VAULT_ATTRIBUTES, oo7::Secret::blob(payload), true)
            .await
            .map_err(|e| anyhow!("the keyring refused the vault item: {e}"))?;
        Ok(())
    }

    async fn remove(&self) -> Result<()> {
        let keyring = oo7::Keyring::new()
            .await
            .map_err(|e| anyhow!("cannot reach the Secret Service: {e}"))?;
        keyring
            .delete(&VAULT_ATTRIBUTES)
            .await
            .map_err(|e| anyhow!("the keyring refused the delete: {e}"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn unlocked_vault(key: &SecretKey) -> Vault<MemoryStore> {
        let mut vault = Vault::new(MemoryStore::default());
        vault.setup(key).await.expect("setup on an empty store");
        vault
    }

    #[tokio::test]
    async fn the_gate_opens_and_closes_on_one_key() {
        let key = SecretKey::generate();
        let mut vault = unlocked_vault(&key).await;
        assert!(vault.is_unlocked());
        assert_eq!(vault.key(), Some(&key));

        vault.lock();
        assert!(!vault.is_unlocked());
        assert_eq!(vault.key(), None, "a locked vault holds no key");

        vault.unlock().await.unwrap();
        assert_eq!(vault.key(), Some(&key), "unlock re-reads the same identity");
    }

    #[tokio::test]
    async fn setup_refuses_to_overwrite_and_destroy_fulfils_itself() {
        let key = SecretKey::generate();
        let mut vault = unlocked_vault(&key).await;

        let second = SecretKey::generate();
        assert!(vault.setup(&second).await.is_err());
        assert_eq!(vault.key(), Some(&key), "the refused setup changed nothing");

        vault.destroy().await.unwrap();
        assert_eq!(vault.key(), None);
        assert!(vault.unlock().await.is_err(), "a destroyed vault does not come back");
        assert!(vault.store.load().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn unlock_without_a_vault_is_an_honest_error() {
        let mut vault = Vault::new(MemoryStore::default());
        assert!(vault.unlock().await.is_err());
    }

    #[tokio::test]
    async fn the_blob_survives_a_round_trip_through_bytes() {
        let key = SecretKey::generate();
        let blob = VaultBlob::new(&key, "wrap of substance".into(), "the nonce".into()).unwrap();
        let bytes = serde_json::to_vec(&blob).unwrap();
        let decoded = VaultBlob::decode(&bytes).unwrap();
        assert_eq!(decoded.wrap, "wrap of substance");
        assert_eq!(keys::decrypt_ncryptsec(&decoded.ncryptsec, &decoded.wrap).unwrap(), key);
    }

    #[tokio::test]
    async fn a_future_blob_version_is_refused_not_reinterpreted() {
        let key = SecretKey::generate();
        let blob = VaultBlob::new(&key, "wrap".into(), "the nonce".into()).unwrap();
        let bytes = serde_json::to_vec(&blob).unwrap();
        let mut mutated = serde_json::from_slice::<serde_json::Value>(&bytes).unwrap();
        mutated["v"] = serde_json::json!(99);
        assert!(VaultBlob::decode(mutated.to_string().as_bytes()).is_err());
    }

    #[tokio::test]
    async fn the_wrap_is_never_the_same_twice() {
        assert_ne!(generate_wrap().unwrap(), generate_wrap().unwrap());
        assert_eq!(generate_wrap().unwrap().len(), 64);
    }

    #[tokio::test]
    async fn a_version_one_blob_migrates_and_the_nonce_becomes_durable() {
        // A pre-nonce vault: the bytes a 44.4.0 keyring holds. The
        // unlock mints the nonce and writes it back, because a URI
        // minted per boot would be a URI every stored copy lies about.
        let key = SecretKey::generate();
        let wrap = generate_wrap().unwrap();
        let ncryptsec = keys::to_ncryptsec(&key, &wrap).unwrap().to_bech32().unwrap();
        let v1 = serde_json::json!({
            "v": 1,
            "wrap": wrap,
            "ncryptsec": ncryptsec,
            "pubkey": keys::public_key_hex(&key),
        });
        let store = std::sync::Arc::new(MemoryStore::default());
        store.save(&serde_json::to_vec(&v1).unwrap()).await.unwrap();

        let mut vault = Vault::new(store.clone());
        vault.unlock().await.unwrap();
        let minted = vault.secret().expect("the migration mints a nonce").to_string();
        assert_eq!(minted.len(), 64, "the nonce is the same 32-byte hex shape as the wrap");

        // Durability: what the store now holds is a version 2 blob
        // whose nonce is the one the vault hands out — a stored URI
        // keeps telling the truth across restarts.
        let blob: serde_json::Value =
            serde_json::from_slice(&store.peek().expect("the migration wrote a blob")).unwrap();
        assert_eq!(blob["v"], 2, "the migrated blob is the current version");
        assert_eq!(blob["secret"], minted.as_str(), "the stored nonce is the vault's own");
    }
}
