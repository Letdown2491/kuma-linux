//! The bunker's brain: NIP-46 request events in, response events out.
//!
//! What lives here is the part of NIP-46 that is protocol rather than
//! policy: decrypting a kind 24133 request, dispatching its method,
//! wrapping the answer as a kind 24133 response encrypted back to the
//! asking app. What does *not* live here is the decision of whether a
//! consequential method runs — that is the [`Gate`]'s job, and the
//! policy engine implements it.
//!
//! The seam is split in two on purpose, and the reason is a deadlock:
//! an Ask decision waits on a person, and the person answers through a
//! socket verb that needs the same lock the worker holds. So the bunker
//! offers [`Bunker::plan`] — decrypt and dispatch, no waiting — and
//! [`Bunker::execute`] — run the decided method. The worker plans
//! under the lock, awaits the decision with the lock released, and
//! re-locks only to execute; an approval arriving a minute later finds
//! a lock that was free the whole time.
//!
//! The key doing the signing is the dedicated remote-signer key the
//! vault holds — never the user's imported identity. Apps see the
//! bunker pubkey and learn nothing else until a method answer tells
//! them; that is the opinion the plan holds one layer down, and it is
//! why `get_public_key` answers with the signer key's public half.

use std::collections::HashMap;

use anyhow::{anyhow, Result};
use nostr::key::{Keys, PublicKey};
use nostr::nips::nip44::Nip44;
use nostr::nips::nip46::{
    NostrConnectMessage, NostrConnectMethod, NostrConnectResponse, ResponseResult,
};
use nostr::prelude::*;

/// The bunker session for one paired app: what `connect` opens and the
/// policy engine will decorate with policy, prompts, and an activity
/// log. Kept at identity-only in this tracer on purpose — inventing the
/// policy fields before the engine exists is how a schema gets designed
/// twice.
#[derive(Debug, Clone)]
pub struct Session {
    /// The secret a connect may carry. Per NIP-46 a `nostrconnect://`
    /// flow's secret is echoed back as the connect result, and the
    /// policy engine will compare it against what the pairing URI said.
    pub secret: Option<String>,
}

/// The client metadata a connect may carry (NIP-46's optional fourth
/// param): the app's own name and image, unauthenticated — the panel's
/// display hint, never an authorization input.
#[derive(Debug, Clone, PartialEq)]
pub struct ClientMeta {
    pub name: Option<String>,
    pub image: Option<String>,
}

impl ClientMeta {
    /// Lenient by design: metadata is a courtesy, and a malformed or
    /// absent blob pairs the same as an honest one.
    fn parse(raw: Option<&String>) -> Option<Self> {
        let raw = raw?;
        let value: serde_json::Value = serde_json::from_str(raw).ok()?;
        Some(Self {
            name: value["name"].as_str().map(str::to_string),
            image: value["image"].as_str().map(str::to_string),
        })
    }
}

/// The decision a gate hands back for a consequential method. The
/// reason is not decoration: it is what the response tells the app and
/// what the activity log records.
#[derive(Debug, Clone)]
pub enum Decision {
    Allow,
    Deny(String),
}

/// Whether a method a paired app asked for may run. `connect` and `ping`
/// never reach the gate — they are protocol, not policy — and every
/// other method does, paired or not: the gate sees the whole
/// consequential surface, which is where the activity log's complete
/// answer comes from.
///
/// The future is awaited by the bunker worker with the daemon's lock
/// released — that is the whole point of the plan/execute split: an Ask
/// may wait on a person, and the person's answer arrives through a
/// socket verb that needs the lock free.
pub trait Gate {
    fn decide(
        &self,
        app: &PublicKey,
        method: &NostrConnectMethod,
        params: &[String],
    ) -> impl std::future::Future<Output = Decision> + Send;
}

/// A gate that refuses everything consequential. What the tests run
/// against when the question is the choreography and not the policy,
/// and the honest posture for a daemon nobody has configured yet.
pub struct DenyAll;

impl Gate for DenyAll {
    async fn decide(
        &self,
        _app: &PublicKey,
        method: &NostrConnectMethod,
        _params: &[String],
    ) -> Decision {
        Decision::Deny(format!("the {method:?} method waits for the policy engine to land"))
    }
}

/// What planning produced: a finished response event, or a request
/// waiting on a gate. `Ignore` is the quiet path — noise, misdelivery,
/// undecryptable — and is answered with nothing, because a refusal
/// that names nothing helps nobody.
#[derive(Debug)]
pub enum Plan {
    Ignore,
    Answer(Event),
    /// A connect that verified: the ack rides in `answer`, and the
    /// daemon records the pairing (with the app's own metadata, when
    /// it claimed any) on its side of the wall.
    Paired {
        answer: Event,
        app: PublicKey,
        metadata: Option<ClientMeta>,
    },
    Ask {
        /// The full request event: `execute` re-reads the app's pubkey
        /// and the correlation id from it.
        request: Event,
        id: String,
        method: NostrConnectMethod,
        params: Vec<String>,
    },
}

/// The bunker: the signer keys, the apps that have connected, and the
/// pairing nonce the URI carries.
pub struct Bunker {
    keys: Keys,
    sessions: HashMap<PublicKey, Session>,
    /// The pairing nonce the bunker URI carries, when the vault has
    /// one. A connect that does not echo it is refused before it pairs:
    /// on a public relay, a pubkey in the clear is an invitation, and
    /// this is the door that invitation does not open. `None` only
    /// before the vault's first read, when there is no URI yet either.
    expected_secret: Option<String>,
}

impl Bunker {
    pub fn new(keys: Keys, expected_secret: Option<String>) -> Self {
        Self { keys, sessions: HashMap::new(), expected_secret }
    }

    /// The bunker's public identity, hex — what `get_public_key`
    /// answers and what a `bunker://` URI is built around.
    pub fn public_key(&self) -> PublicKey {
        self.keys.public_key()
    }

    /// Whether an app is paired. The policy engine replaces the storage
    /// with per-app policy and persisted pairing; the question stays.
    pub fn is_paired(&self, app: &PublicKey) -> bool {
        self.sessions.contains_key(app)
    }

    /// Plan one event: everything that is protocol answers immediately
    /// (as a finished response event), everything consequential becomes
    /// a [`Pending`] for the caller to decide outside any lock and hand
    /// back through [`Bunker::execute`]. Noise is [`Plan::Ignore`] —
    /// not a 24133, not addressed to this bunker, or undecryptable,
    /// where there is no request id to answer and a refusal that names
    /// nothing helps nobody.
    pub fn plan(&mut self, event: &Event) -> Plan {
        if event.kind != Kind::NostrConnect {
            return Plan::Ignore;
        }
        let self_pubkey = self.public_key();
        if !event.tags.public_keys().any(|p| p == self_pubkey) {
            return Plan::Ignore;
        }
        let Some(plaintext) = self.keys.nip44_decrypt(&event.pubkey, &event.content).ok() else {
            return Plan::Ignore;
        };
        let Ok(message) = NostrConnectMessage::from_json(&plaintext) else {
            return Plan::Ignore;
        };
        let (id, method, params) = match message {
            NostrConnectMessage::Request { id, method, params } => (id, method, params),
            NostrConnectMessage::Response { .. } => return Plan::Ignore,
        };
        let response = match method {
            NostrConnectMethod::Connect => {
                // Params are [user_pubkey, secret?]. When the URI
                // carries a pairing nonce, the connect must echo it —
                // constant-time, because a comparison that leaks its
                // own progress is a lock that shows its keys. A connect
                // without the echo is refused before it pairs, so a
                // scraped pubkey opens asks on nobody. With no nonce in
                // the vault there is nothing to verify against, and the
                // person's gate stays the door.
                let secret = params.get(1).cloned();
                let refused = match (&self.expected_secret, secret.as_deref()) {
                    (Some(expected), Some(provided)) => !constant_time_eq(provided, expected),
                    (Some(_), None) => true,
                    (None, _) => false,
                };
                eprintln!(
                    "kuma-nostrd: connect from {}: {}",
                    event.pubkey,
                    if refused {
                        "refused, the connect did not echo the pairing nonce"
                    } else {
                        "paired"
                    }
                );
                if refused {
                    return match self.response_event(
                        event,
                        &id,
                        NostrConnectResponse::with_error(
                            "the connect did not carry the secret the pairing URI carries",
                        ),
                    ) {
                        Some(answer) => Plan::Answer(answer),
                        None => Plan::Ignore,
                    };
                }
                self.sessions.entry(event.pubkey).or_insert(Session { secret: secret.clone() });
                // The answer is ack, whatever the app echoed: the nonce
                // was the bunker URI's own, the verify above is the
                // proof of readership, and the result's job is the
                // one word every client checks. (The echo-back-the-
                // secret shape belongs to the nostrconnect:// flow,
                // where the app minted the secret and the signer proves
                // it read that URI instead.)
                let answer = self.response_event(
                    event,
                    &id,
                    NostrConnectResponse::with_result(ResponseResult::Ack),
                );
                let metadata = ClientMeta::parse(params.get(3));
                return match answer {
                    Some(answer) => Plan::Paired { answer, app: event.pubkey, metadata },
                    None => Plan::Ignore,
                };
            }
            NostrConnectMethod::Ping => ResponseResult::Pong,
            method => {
                if !self.is_paired(&event.pubkey) {
                    return match self.response_event(
                        event,
                        &id,
                        NostrConnectResponse::with_error("this app is not paired"),
                    ) {
                        Some(answer) => Plan::Answer(answer),
                        None => Plan::Ignore,
                    };
                }
                return Plan::Ask { request: event.clone(), id, method, params };
            }
        };
        // The protocol answers and the refusal share one road out: a
        // finished response event when the wrap succeeds, Ignore when
        // it does not — an app whose channel broke sees silence and
        // retries, which is what the retry is for.
        match self.response_event(event, &id, NostrConnectResponse::with_result(response)) {
            Some(answer) => Plan::Answer(answer),
            None => Plan::Ignore,
        }
    }

    /// Run a planned method under its decided answer. The decision is
    /// the gate's; the signing is the bunker's; the response event is
    /// the app's half of the channel again.
    pub fn execute(
        &self,
        request: &Event,
        id: &str,
        method: &NostrConnectMethod,
        params: &[String],
        decision: Decision,
    ) -> Option<Event> {
        let response = match decision {
            Decision::Allow => self.run(method, params),
            Decision::Deny(reason) => NostrConnectResponse::with_error(reason),
        };
        self.response_event(request, id, response)
    }

    /// The methods the gate allowed. One arm per method family, sharing
    /// the response shape: a typed result the crate serializes, or an
    /// error the app sees.
    fn run(&self, method: &NostrConnectMethod, params: &[String]) -> NostrConnectResponse {
        match method {
            NostrConnectMethod::GetPublicKey => {
                NostrConnectResponse::with_result(ResponseResult::GetPublicKey(self.public_key()))
            }
            NostrConnectMethod::SignEvent => {
                let unsigned = match params.first() {
                    Some(json) => match UnsignedEvent::from_json(json) {
                        Ok(event) => event,
                        Err(e) => {
                            return NostrConnectResponse::with_error(format!(
                                "unreadable unsigned event: {e}"
                            ))
                        }
                    },
                    None => return NostrConnectResponse::with_error("sign_event wants an event"),
                };
                match self.keys.sign_event(unsigned) {
                    Ok(signed) => NostrConnectResponse::with_result(ResponseResult::SignEvent(
                        Box::new(signed),
                    )),
                    Err(e) => NostrConnectResponse::with_error(format!("signing failed: {e}")),
                }
            }
            other => {
                NostrConnectResponse::with_error(format!("the {other:?} method is not implemented"))
            }
        }
    }

    /// Wrap an answer: kind 24133, encrypted back to the app that asked,
    /// per NIP-46's own "Response Events `kind:24133`" — the request
    /// and the answer share the kind, and a response on 24135 (the old
    /// revision this crate's types grew up with) is a letter mailed to
    /// a box nobody checks: every current client listens on 24133.
    /// p-tagged to them. The e-tag back to the request event is not
    /// written: the id inside the payload is the correlation, and a
    /// second one invites disagreement about which one counts.
    fn response_event(
        &self,
        request: &Event,
        request_id: &str,
        response: NostrConnectResponse,
    ) -> Option<Event> {
        let message = NostrConnectMessage::response(request_id, response);
        let content = self.keys.nip44_encrypt(&request.pubkey, &message.as_json()).ok()?;
        EventBuilder::new(Kind::from_u16(24133), content)
            .tag(Tag::public_key(request.pubkey))
            .finalize(&self.keys)
            .ok()
    }
}

/// The `bunker://` URI a QR renders: the bunker pubkey and the relay
/// set, in the form a phone's nostr app parses. Percent-encoding the
/// relay URLs is the spec's own spelling.
/// Constant-time equality for the pairing nonce's echo. The comparison
/// leaks its length and nothing else: a nonce this short never leaves
/// room for a timing oracle to matter, and the discipline costs one
/// fold.
fn constant_time_eq(a: &str, b: &str) -> bool {
    use subtle::ConstantTimeEq;
    a.as_bytes().ct_eq(b.as_bytes()).into()
}

/// The bunker URI an app logs in with: the bunker's key, the relays it
/// answers on, and the pairing nonce the connect must echo.
pub fn bunker_uri(public_key: &PublicKey, relays: &[String], secret: Option<&str>) -> String {
    let mut uri = format!("bunker://{}", public_key);
    for relay in relays {
        let mut encoded = Vec::with_capacity(relay.len());
        for b in relay.bytes() {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                    encoded.push(b)
                }
                _ => encoded.extend_from_slice(format!("%{b:02X}").as_bytes()),
            }
        }
        let encoded = String::from_utf8(encoded).expect("percent-encoded ASCII");
        let sep = if uri.contains('?') { '&' } else { '?' };
        uri.push(sep);
        uri.push_str("relay=");
        uri.push_str(&encoded);
    }
    if let Some(secret) = secret {
        let mut encoded = Vec::with_capacity(secret.len());
        for b in secret.bytes() {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                    encoded.push(b)
                }
                _ => encoded.extend_from_slice(format!("%{b:02X}").as_bytes()),
            }
        }
        let encoded = String::from_utf8(encoded).expect("percent-encoded ASCII");
        uri.push_str("&secret=");
        uri.push_str(&encoded);
    }
    uri
}

/// Parse a `bunker://` URI back into its parts — what the CLI's
/// `bunker` verb renders and what nothing else reuses, but a URI that
/// cannot be parsed back is a URI that cannot be tested.
pub fn parse_bunker_uri(uri: &str) -> Result<(PublicKey, Vec<String>)> {
    let rest = uri.strip_prefix("bunker://").ok_or_else(|| anyhow!("not a bunker:// URI"))?;
    let (pubkey_str, query) = match rest.split_once('?') {
        Some((pk, q)) => (pk, Some(q)),
        None => (rest, None),
    };
    let public_key = PublicKey::parse(pubkey_str).map_err(|e| anyhow!("bad bunker pubkey: {e}"))?;
    let mut relays = Vec::new();
    if let Some(query) = query {
        for pair in query.split('&') {
            if let Some(value) = pair.strip_prefix("relay=") {
                relays.push(percent_decode(value)?);
            }
        }
    }
    Ok((public_key, relays))
}

fn percent_decode(value: &str) -> Result<String> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 3 > bytes.len() {
                return Err(anyhow!("bad percent-encoding"));
            }
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3])
                .map_err(|_| anyhow!("bad percent-encoding"))?;
            let byte = u8::from_str_radix(hex, 16).map_err(|_| anyhow!("bad percent-encoding"))?;
            out.push(byte);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| anyhow!("bad percent-encoding"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One keypair standing in for the paired app, with the pieces the
    /// tests need: a request event encrypted and signed as an app
    /// would, and the ability to decrypt what came back.
    struct App {
        keys: Keys,
    }

    impl App {
        fn new() -> Self {
            Self { keys: Keys::generate() }
        }

        fn pubkey(&self) -> PublicKey {
            self.keys.public_key()
        }

        fn request_event(
            &self,
            bunker: &PublicKey,
            method: NostrConnectMethod,
            params: &[&str],
        ) -> Event {
            self.request_event_with_id(bunker, "test-request-id", method, params)
        }

        fn request_event_with_id(
            &self,
            bunker: &PublicKey,
            id: &str,
            method: NostrConnectMethod,
            params: &[&str],
        ) -> Event {
            let message = NostrConnectMessage::Request {
                id: id.to_string(),
                method,
                params: params.iter().map(|s| s.to_string()).collect(),
            };
            self.request_event_raw(bunker, &message.as_json())
        }

        fn request_event_raw(&self, bunker: &PublicKey, plaintext: &str) -> Event {
            let content = self.keys.nip44_encrypt(bunker, plaintext).unwrap();
            EventBuilder::new(Kind::NostrConnect, content)
                .tag(Tag::public_key(*bunker))
                .finalize(&self.keys)
                .unwrap()
        }

        /// The response the bunker sent, decrypted with the app's own
        /// half of the channel.
        fn decrypt_response(&self, response: &Event) -> NostrConnectMessage {
            let plaintext = self.keys.nip44_decrypt(&response.pubkey, &response.content).unwrap();
            NostrConnectMessage::from_json(&plaintext).unwrap()
        }
    }

    #[tokio::test]
    async fn ping_round_trips_the_crypto_choreography() {
        let mut bunker = Bunker::new(Keys::generate(), None);
        let app = App::new();
        let bunker_pubkey = bunker.public_key();

        let request =
            app.request_event_with_id(&bunker_pubkey, "ping-id-1", NostrConnectMethod::Ping, &[]);
        let response = match bunker.plan(&request) {
            Plan::Answer(response) => response,
            other => panic!("a ping is protocol, answered in the plan: {other:?}"),
        };

        assert_eq!(response.kind, Kind::from_u16(24133));
        assert_eq!(response.pubkey, bunker_pubkey);
        assert_eq!(response.tags.public_keys().collect::<Vec<_>>(), vec![app.pubkey()]);
        match app.decrypt_response(&response) {
            NostrConnectMessage::Response { id, result, error } => {
                assert_eq!(id, "ping-id-1");
                assert_eq!(result.as_deref(), Some("pong"));
                assert_eq!(error, None);
            }
            other => panic!("a response came back: {other:?}"),
        }
    }

    #[tokio::test]
    async fn connect_pairs_and_get_public_key_answers_the_bunker_identity() {
        let mut bunker = Bunker::new(Keys::generate(), None);
        let app = App::new();
        let bunker_pubkey = bunker.public_key();

        match bunker.plan(&app.request_event(&bunker_pubkey, NostrConnectMethod::Connect, &[])) {
            Plan::Paired { .. } => {}
            other => panic!("connect is protocol: {other:?}"),
        }
        assert!(bunker.is_paired(&app.pubkey()));

        let ask_event = app.request_event(&bunker_pubkey, NostrConnectMethod::GetPublicKey, &[]);
        let response = match bunker.plan(&ask_event) {
            Plan::Ask { request, id, method, params } => bunker
                .execute(&request, &id, &method, &params, Decision::Allow)
                .expect("an allowed method answers"),
            other => panic!("get_public_key waits on the gate: {other:?}"),
        };
        match app.decrypt_response(&response) {
            NostrConnectMessage::Response { result, error, .. } => {
                assert_eq!(error, None);
                assert_eq!(result.as_deref(), Some(bunker_pubkey.to_string().as_str()));
            }
            other => panic!("a response came back: {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_connect_without_the_nonce_opens_nothing() {
        let mut bunker = Bunker::new(Keys::generate(), Some("the-nonce".into()));
        let app = App::new();
        let bunker_pubkey = bunker.public_key();

        // No echo at all: the app that never read the URI.
        let request = app.request_event(&bunker_pubkey, NostrConnectMethod::Connect, &[]);
        match bunker.plan(&request) {
            Plan::Answer(response) => match app.decrypt_response(&response) {
                NostrConnectMessage::Response { error: Some(e), .. } => {
                    assert!(e.contains("secret"), "the refusal names the nonce: {e}");
                }
                other => panic!("the missing echo is a refusal: {other:?}"),
            },
            other => panic!("a refusal is an answer, not a silence: {other:?}"),
        }
        assert!(!bunker.is_paired(&app.pubkey()), "a refused connect pairs nobody");

        // The wrong echo: a guess, or another URI's nonce. Connect's
        // params lead with the pubkey the app expects to control; the
        // nonce's echo rides behind it.
        let request = app.request_event(
            &bunker_pubkey,
            NostrConnectMethod::Connect,
            &[bunker_pubkey.to_string().as_str(), "another-uri-nonce"],
        );
        match bunker.plan(&request) {
            Plan::Answer(_) => {}
            other => panic!("the wrong echo is refused too: {other:?}"),
        }
        assert!(!bunker.is_paired(&app.pubkey()));

        // The right echo: the app read the URI, and the door opens.
        let request = app.request_event(
            &bunker_pubkey,
            NostrConnectMethod::Connect,
            &[bunker_pubkey.to_string().as_str(), "the-nonce"],
        );
        match bunker.plan(&request) {
            Plan::Paired { .. } => {}
            other => panic!("the nonce's echo pairs: {other:?}"),
        }
        assert!(bunker.is_paired(&app.pubkey()));
    }

    #[tokio::test]
    async fn a_bunker_without_a_nonce_still_pairs_by_the_human_gate() {
        // The pre-nonce shape: a vault that has not migrated yet arms a
        // bunker with nothing to verify against, and the person's gate
        // stays the only door — which is why the refusal above is an
        // answer rather than a silence: an app that reads the refusal
        // knows to read the fresh URI, and a person whose bunker asks
        // knows to look at the panel.
        let mut bunker = Bunker::new(Keys::generate(), None);
        let app = App::new();
        let bunker_pubkey = bunker.public_key();

        match bunker.plan(&app.request_event(&bunker_pubkey, NostrConnectMethod::Connect, &[])) {
            Plan::Paired { .. } => {}
            other => panic!("connect is protocol: {other:?}"),
        }
        assert!(bunker.is_paired(&app.pubkey()));
    }

    #[tokio::test]
    async fn an_unpaired_app_gets_refused() {
        let mut bunker = Bunker::new(Keys::generate(), None);
        let app = App::new();
        let bunker_pubkey = bunker.public_key();

        let ask_event = app.request_event(&bunker_pubkey, NostrConnectMethod::GetPublicKey, &[]);
        let response = match bunker.plan(&ask_event) {
            Plan::Answer(response) => response,
            other => panic!("an unpaired app is refused in the plan: {other:?}"),
        };
        match app.decrypt_response(&response) {
            NostrConnectMessage::Response { result, error, .. } => {
                assert_eq!(result, None);
                assert!(error.unwrap().contains("not paired"));
            }
            other => panic!("a response came back: {other:?}"),
        }
    }

    #[tokio::test]
    async fn the_gate_refusal_is_the_answer_the_app_sees() {
        let mut bunker = Bunker::new(Keys::generate(), None);
        let app = App::new();
        let bunker_pubkey = bunker.public_key();

        bunker.plan(&app.request_event(&bunker_pubkey, NostrConnectMethod::Connect, &[]));
        let ask_event = app.request_event(&bunker_pubkey, NostrConnectMethod::GetPublicKey, &[]);
        let response = match bunker.plan(&ask_event) {
            Plan::Ask { request, id, method, params } => bunker
                .execute(
                    &request,
                    &id,
                    &method,
                    &params,
                    DenyAll.decide(&app.pubkey(), &method, &params).await,
                )
                .expect("a denied method still answers"),
            other => panic!("a paired app's ask goes to the gate: {other:?}"),
        };
        match app.decrypt_response(&response) {
            NostrConnectMessage::Response { result, error, .. } => {
                assert_eq!(result, None);
                let error = error.unwrap();
                assert!(error.contains("policy engine"), "{error}");
            }
            other => panic!("a response came back: {other:?}"),
        }
    }

    #[tokio::test]
    async fn sign_event_signs_when_allowed_and_the_signature_verifies() {
        let mut bunker = Bunker::new(Keys::generate(), None);
        let app = App::new();
        let bunker_pubkey = bunker.public_key();
        bunker.plan(&app.request_event(&bunker_pubkey, NostrConnectMethod::Connect, &[]));

        let unsigned = UnsignedEvent::new(
            bunker_pubkey,
            Timestamp::now(),
            Kind::TextNote,
            [],
            "hello from the bunker",
        );
        let response = match bunker.plan(&app.request_event(
            &bunker_pubkey,
            NostrConnectMethod::SignEvent,
            &[unsigned.as_json().as_str()],
        )) {
            Plan::Ask { request, id, method, params } => bunker
                .execute(&request, &id, &method, &params, Decision::Allow)
                .expect("an allowed sign answers"),
            other => panic!("sign_event waits on the gate: {other:?}"),
        };
        match app.decrypt_response(&response) {
            NostrConnectMessage::Response { result, error, .. } => {
                assert_eq!(error, None, "{error:?}");
                let signed = Event::from_json(result.unwrap()).unwrap();
                signed.verify().unwrap();
                assert_eq!(signed.pubkey, bunker_pubkey);
            }
            other => panic!("a response came back: {other:?}"),
        }
    }

    #[tokio::test]
    async fn noise_is_none_and_never_a_response() {
        let mut bunker = Bunker::new(Keys::generate(), None);
        let app = App::new();
        let bunker_pubkey = bunker.public_key();

        // Not a 24133: a text note mentioning the bunker pubkey in a tag.
        let note = EventBuilder::new(Kind::TextNote, "noise")
            .tag(Tag::public_key(bunker_pubkey))
            .finalize(&app.keys)
            .unwrap();
        assert!(matches!(bunker.plan(&note), Plan::Ignore));

        // Addressed to the bunker but undecryptable — random content.
        let garbage = EventBuilder::new(Kind::NostrConnect, "not nip44")
            .tag(Tag::public_key(bunker_pubkey))
            .finalize(&app.keys)
            .unwrap();
        assert!(matches!(bunker.plan(&garbage), Plan::Ignore));

        // Decryptable but not addressed to this bunker.
        let stranger = App::new();
        let misplaced = stranger.request_event(&stranger.pubkey(), NostrConnectMethod::Ping, &[]);
        assert!(matches!(bunker.plan(&misplaced), Plan::Ignore));
    }

    #[test]
    fn the_bunker_uri_round_trips_through_its_own_parser() {
        let pubkey = Keys::generate().public_key();
        let relays =
            vec!["wss://relay.nip46.com".to_string(), "wss://machine.tailnet.ts.net".to_string()];
        let uri = bunker_uri(&pubkey, &relays, None);
        assert!(uri.starts_with(&format!("bunker://{pubkey}")));
        assert_eq!(parse_bunker_uri(&uri).unwrap(), (pubkey, relays));
        assert!(parse_bunker_uri("nostr://nope").is_err());
        assert!(parse_bunker_uri("bunker://not-a-pubkey").is_err());
    }
}
