//! The bunker's brain: NIP-46 request events in, response events out.
//!
//! What lives here is the part of NIP-46 that is protocol rather than
//! policy: decrypting a kind 24133 request, dispatching its method,
//! wrapping the answer as a kind 24135 response encrypted back to the
//! asking app. What does *not* live here is the decision of whether a
//! consequential method runs — that is the [`Gate`]'s job, and the
//! policy engine that implements it arrives in the next tracer. The
//! seam is drawn so that tracer changes decisions, not this file: the
//! round-trip tests prove the crypto choreography against a gate that
//! allows or denies, and the engine only ever feeds it different
//! answers.
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
pub trait Gate {
    fn decide(
        &self,
        app: &PublicKey,
        method: &NostrConnectMethod,
        params: &[String],
    ) -> impl std::future::Future<Output = Decision> + Send;
}

/// A gate that refuses everything consequential. What this tracer's
/// tests run against — and the honest default for a daemon whose
/// policy engine has not landed: a bunker that connects, pings, and
/// signs nothing.
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

/// The bunker: the signer keys and the apps that have connected.
pub struct Bunker {
    keys: Keys,
    sessions: HashMap<PublicKey, Session>,
}

impl Bunker {
    pub fn new(keys: Keys) -> Self {
        Self { keys, sessions: HashMap::new() }
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

    /// Process one event. Anything that is not a kind 24133 request
    /// addressed to this bunker answers `None` — relays are noisy and
    /// the pool will feed this everything it subscribes to, filtered
    /// but not promised. An event that cannot be decrypted is also
    /// `None`: there is no request id to answer, and a refusal that
    /// names nothing helps nobody.
    pub async fn process_event(&mut self, event: &Event, gate: &impl Gate) -> Option<Event> {
        if event.kind != Kind::NostrConnect {
            return None;
        }
        let self_pubkey = self.public_key();
        if !event.tags.public_keys().any(|p| p == self_pubkey) {
            return None;
        }
        let plaintext = self.keys.nip44_decrypt(&event.pubkey, &event.content).ok()?;
        let message = NostrConnectMessage::from_json(&plaintext).ok()?;
        let (id, method, params) = match message {
            NostrConnectMessage::Request { id, method, params } => (id, method, params),
            NostrConnectMessage::Response { .. } => return None,
        };
        let response = self.answer(event.pubkey, &method, &params, gate).await;
        self.response_event(event, &id, response)
    }

    /// Method dispatch. Connect and ping are answered as protocol; the
    /// rest goes to the gate and then to [`Bunker::run`], in that
    /// order, because an unpaired app holding policy weight is a
    /// decision the engine never made.
    async fn answer(
        &mut self,
        app: PublicKey,
        method: &NostrConnectMethod,
        params: &[String],
        gate: &impl Gate,
    ) -> NostrConnectResponse {
        match method {
            NostrConnectMethod::Connect => {
                // Params are [user_pubkey, secret?]; the secret rides
                // back as the result when it was sent (nostrconnect://
                // flow), and its absence is the bunker:// flow.
                let secret = params.get(1).cloned();
                self.sessions.entry(app).or_insert(Session { secret: secret.clone() });
                let result = secret.map_or(ResponseResult::Ack, ResponseResult::ConnectSecret);
                NostrConnectResponse::with_result(result)
            }
            NostrConnectMethod::Ping => NostrConnectResponse::with_result(ResponseResult::Pong),
            _ => {
                if !self.is_paired(&app) {
                    return NostrConnectResponse::with_error("this app is not paired");
                }
                match gate.decide(&app, method, params).await {
                    Decision::Allow => self.run(method, params),
                    Decision::Deny(reason) => NostrConnectResponse::with_error(reason),
                }
            }
        }
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

    /// Wrap an answer: kind 24135, encrypted back to the app that asked,
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
        EventBuilder::new(Kind::from_u16(24135), content)
            .tag(Tag::public_key(request.pubkey))
            .finalize(&self.keys)
            .ok()
    }
}

/// The `bunker://` URI a QR renders: the bunker pubkey and the relay
/// set, in the form a phone's nostr app parses. Percent-encoding the
/// relay URLs is the spec's own spelling.
pub fn bunker_uri(public_key: &PublicKey, relays: &[String]) -> String {
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

    /// A gate that allows everything: the choreography tests want the
    /// methods to run, and the deny path has its own test.
    struct AllowAll;
    impl Gate for AllowAll {
        async fn decide(&self, _: &PublicKey, _: &NostrConnectMethod, _: &[String]) -> Decision {
            Decision::Allow
        }
    }

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
        let mut bunker = Bunker::new(Keys::generate());
        let app = App::new();
        let bunker_pubkey = bunker.public_key();

        let request =
            app.request_event_with_id(&bunker_pubkey, "ping-id-1", NostrConnectMethod::Ping, &[]);
        let response = bunker.process_event(&request, &DenyAll).await.unwrap();

        assert_eq!(response.kind, Kind::from_u16(24135));
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
        let mut bunker = Bunker::new(Keys::generate());
        let app = App::new();
        let bunker_pubkey = bunker.public_key();

        bunker
            .process_event(
                &app.request_event(&bunker_pubkey, NostrConnectMethod::Connect, &[]),
                &DenyAll,
            )
            .await
            .unwrap();
        assert!(bunker.is_paired(&app.pubkey()));

        let response = bunker
            .process_event(
                &app.request_event(&bunker_pubkey, NostrConnectMethod::GetPublicKey, &[]),
                &AllowAll,
            )
            .await
            .unwrap();
        match app.decrypt_response(&response) {
            NostrConnectMessage::Response { result, error, .. } => {
                assert_eq!(error, None);
                assert_eq!(result.as_deref(), Some(bunker_pubkey.to_string().as_str()));
            }
            other => panic!("a response came back: {other:?}"),
        }
    }

    #[tokio::test]
    async fn an_unpaired_app_gets_refused() {
        let mut bunker = Bunker::new(Keys::generate());
        let app = App::new();
        let bunker_pubkey = bunker.public_key();

        let response = bunker
            .process_event(
                &app.request_event(&bunker_pubkey, NostrConnectMethod::GetPublicKey, &[]),
                &DenyAll,
            )
            .await
            .unwrap();
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
        let mut bunker = Bunker::new(Keys::generate());
        let app = App::new();
        let bunker_pubkey = bunker.public_key();

        bunker
            .process_event(
                &app.request_event(&bunker_pubkey, NostrConnectMethod::Connect, &[]),
                &DenyAll,
            )
            .await
            .unwrap();
        let response = bunker
            .process_event(
                &app.request_event(&bunker_pubkey, NostrConnectMethod::GetPublicKey, &[]),
                &DenyAll,
            )
            .await
            .unwrap();
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
        let mut bunker = Bunker::new(Keys::generate());
        let app = App::new();
        let bunker_pubkey = bunker.public_key();
        bunker
            .process_event(
                &app.request_event(&bunker_pubkey, NostrConnectMethod::Connect, &[]),
                &AllowAll,
            )
            .await
            .unwrap();

        let unsigned = UnsignedEvent::new(
            bunker_pubkey,
            Timestamp::now(),
            Kind::TextNote,
            [],
            "hello from the bunker",
        );
        let response = bunker
            .process_event(
                &app.request_event(
                    &bunker_pubkey,
                    NostrConnectMethod::SignEvent,
                    &[unsigned.as_json().as_str()],
                ),
                &AllowAll,
            )
            .await
            .unwrap();
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
        let mut bunker = Bunker::new(Keys::generate());
        let app = App::new();
        let bunker_pubkey = bunker.public_key();

        // Not a 24133: a text note mentioning the bunker pubkey in a tag.
        let note = EventBuilder::new(Kind::TextNote, "noise")
            .tag(Tag::public_key(bunker_pubkey))
            .finalize(&app.keys)
            .unwrap();
        assert!(bunker.process_event(&note, &DenyAll).await.is_none());

        // Addressed to the bunker but undecryptable — random content.
        let garbage = EventBuilder::new(Kind::NostrConnect, "not nip44")
            .tag(Tag::public_key(bunker_pubkey))
            .finalize(&app.keys)
            .unwrap();
        assert!(bunker.process_event(&garbage, &DenyAll).await.is_none());

        // Decryptable but not addressed to this bunker.
        let stranger = App::new();
        let misplaced = stranger.request_event(&stranger.pubkey(), NostrConnectMethod::Ping, &[]);
        assert!(bunker.process_event(&misplaced, &DenyAll).await.is_none());
    }

    #[test]
    fn the_bunker_uri_round_trips_through_its_own_parser() {
        let pubkey = Keys::generate().public_key();
        let relays =
            vec!["wss://relay.nip46.com".to_string(), "wss://machine.tailnet.ts.net".to_string()];
        let uri = bunker_uri(&pubkey, &relays);
        assert!(uri.starts_with(&format!("bunker://{pubkey}")));
        assert_eq!(parse_bunker_uri(&uri).unwrap(), (pubkey, relays));
        assert!(parse_bunker_uri("nostr://nope").is_err());
        assert!(parse_bunker_uri("bunker://not-a-pubkey").is_err());
    }
}
