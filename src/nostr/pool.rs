//! The lean transport: one thread per relay, blocking WebSockets, and
//! the honest failure story the plan promised.
//!
//! What replaces nostr-sdk's pool is a few hundred lines, and this file
//! is most of them. Each relay gets a thread that connects, subscribes
//! once — requests addressed to the bunker pubkey, kind 24133 — and
//! then loops: read what arrives, drain what the bunker wants
//! published, and reconnect with a growing backoff when the relay
//! dies. The transport holds no secrets; the events it forwards go to
//! the bunker's brain, which is where every decision lives.
//!
//! The one scary failure — the bunker silently stops answering — is
//! watched elsewhere: the doctor's liveness probe, the bar widget's
//! offline state, and the smoke stage. What the transport owes them is
//! the truth about connection state, which is why every state change
//! is visible on the status channel rather than swallowed.

use std::collections::HashMap;
use std::io::ErrorKind;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use nostr::prelude::*;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::Message;

/// The subscription id every relay thread uses. One subscription per
/// relay; the id is what the relay echoes back, so it only needs to be
/// distinguishable in a log.
pub(crate) const SUBSCRIPTION_ID: &str = "kuma-bunker";

/// The backoff ceiling: a relay that has been down for this long keeps
/// trying on the minute, because the bunker being reachable when the
/// relay comes back is the whole job.
const BACKOFF_MAX: Duration = Duration::from_secs(60);
const BACKOFF_START: Duration = Duration::from_secs(1);

/// What a relay thread reports upward. Connection state is a fact the
/// surfaces render, not a log line to hope for.
#[derive(Debug, Clone, PartialEq)]
pub enum RelayState {
    Connected,
    Disconnected,
}

/// One relay's line in the status answer: its URL and where it stands.
#[derive(Debug, Clone, PartialEq)]
pub struct RelayStatus {
    pub url: String,
    pub state: RelayState,
}

/// Which roads an outbound event takes. The bunker answers apps, and
/// an answer encrypted to one app does not belong on every relay's
/// wire: a relay operator learns which app talks to which bunker from
/// the traffic alone, and an answer aimed at another app is not that
/// relay's business.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Fan {
    /// The declared set's roads — the bunker's answers on the relays
    /// the declaration chose.
    Own,
    /// One app's road: the declared set AND the app's own relays. A
    /// bunker:// app's road is the declared set alone; a
    /// nostrconnect app's adds the relays its URI named.
    App(PublicKey),
    /// Only one app's own relays — the nostrconnect handshake, whose
    /// road the NIP spells as the URI's relays and nothing else.
    OnlyApp(PublicKey),
}

/// One relay thread's identity and its own stop flag: a road. An app's
/// roads tear down individually on revocation — a shared flag would
/// stop every road to stop one.
struct Road {
    own: bool,
    app: Option<PublicKey>,
    stop: Arc<AtomicBool>,
    handle: std::thread::JoinHandle<()>,
}

/// The pool: owns the relay threads, hands bunker-bound events to the
/// `inbound` receiver, and publishes the bunker's answers down the
/// road each answer names.
pub struct RelayPool {
    outbound: Sender<(Event, Fan)>,
    /// The queue's receiver behind its mutex, cloned per thread —
    /// one queue, many roads, and a thread only holds it for the
    /// drain of one loop beat.
    outbound_rx: Arc<Mutex<Receiver<(Event, Fan)>>>,
    /// The declared set's roads.
    own: Vec<Road>,
    own_urls: Vec<String>,
    /// The nostrconnect apps' roads, by the client pubkey that
    /// presented its URI.
    apps: HashMap<PublicKey, Vec<Road>>,
    app_urls: HashMap<PublicKey, Vec<String>>,
}

impl RelayPool {
    /// Spawn one thread per relay. `inbound` receives every kind 24133
    /// event any relay hands over; `status` receives each relay's state
    /// changes, and starts empty — a relay that has said nothing yet
    /// has no status, which the surfaces render as "connecting".
    pub fn spawn(
        relays: Vec<String>,
        bunker_pubkey: PublicKey,
        inbound: Sender<Event>,
        status: Sender<RelayStatus>,
    ) -> Self {
        let (outbound, outbound_rx): (Sender<(Event, Fan)>, Receiver<(Event, Fan)>) = channel();
        // One queue, many relay threads: the receiver rides behind a
        // mutex, and a thread only holds it for the drain of one loop
        // beat.
        let outbound_rx = Arc::new(Mutex::new(outbound_rx));
        let mut pool = Self {
            outbound,
            outbound_rx,
            own: Vec::new(),
            own_urls: Vec::new(),
            apps: HashMap::new(),
            app_urls: HashMap::new(),
        };
        pool.spawn_own(relays, bunker_pubkey, &inbound, &status);
        pool
    }

    fn spawn_road(
        &self,
        url: String,
        own: bool,
        app: Option<PublicKey>,
        bunker_pubkey: PublicKey,
        inbound: &Sender<Event>,
        status: &Sender<RelayStatus>,
    ) -> Road {
        let stop = Arc::new(AtomicBool::new(false));
        let handle = std::thread::spawn({
            let stop = stop.clone();
            let outbound_rx = self.outbound_rx.clone();
            let inbound = inbound.clone();
            let status = status.clone();
            move || {
                one_relay(url, own, app, bunker_pubkey, inbound, status, outbound_rx, stop);
            }
        });
        Road { own, app, stop, handle }
    }

    fn spawn_own(
        &mut self,
        relays: Vec<String>,
        bunker_pubkey: PublicKey,
        inbound: &Sender<Event>,
        status: &Sender<RelayStatus>,
    ) {
        for url in relays {
            if self.own_urls.contains(&url) {
                continue;
            }
            self.own_urls.push(url.clone());
            self.own.push(self.spawn_road(url, true, None, bunker_pubkey, inbound, status));
        }
    }

    /// A nostrconnect app's own relays, as threads: the same
    /// subscription, the same channels, the app's road in. A URL the
    /// declared set already runs is skipped — one road, one thread —
    /// and so is one the app itself already has.
    pub fn subscribe_relays(
        &mut self,
        app: &PublicKey,
        relays: Vec<String>,
        bunker_pubkey: PublicKey,
        inbound: &Sender<Event>,
        status: &Sender<RelayStatus>,
    ) {
        let urls = self.app_urls.entry(*app).or_default();
        for url in relays {
            if self.own_urls.contains(&url) || urls.contains(&url) {
                continue;
            }
            urls.push(url.clone());
            self.apps.entry(*app).or_default().push(self.spawn_road(
                url,
                false,
                Some(*app),
                bunker_pubkey,
                inbound,
                status,
            ));
        }
    }

    /// Tear one app's roads down: the revocation's own half. The
    /// threads exit on their next beat; the declared set's roads are
    /// nobody else's to stop.
    pub fn drop_app(&mut self, app: &PublicKey) {
        if let Some(roads) = self.apps.remove(app) {
            for road in roads {
                road.stop.store(true, Ordering::SeqCst);
            }
        }
        self.app_urls.remove(app);
    }

    /// Whether an app's roads are still threaded — what the teardown
    /// test reads.
    #[cfg(test)]
    pub(crate) fn app_road_count(&self, app: &PublicKey) -> usize {
        self.apps.get(app).map_or(0, Vec::len)
    }

    /// Publish an event down the declared set's roads.
    pub fn publish(&self, event: &Event) -> Result<()> {
        self.send(event, Fan::Own)
    }

    /// Publish an answer down one app's road: the declared set, plus
    /// the app's own relays when it has any.
    pub fn publish_for(&self, event: &Event, app: &PublicKey) -> Result<()> {
        self.send(event, Fan::App(*app))
    }

    /// Publish the handshake down the client's relays and nothing
    /// else's — the NIP spells that road as the URI's, and a new
    /// pairing announced on the declared set is metadata the bunker's
    /// own relays do not need.
    pub fn publish_only_to(&self, event: &Event, app: &PublicKey) -> Result<()> {
        self.send(event, Fan::OnlyApp(*app))
    }

    fn send(&self, event: &Event, fan: Fan) -> Result<()> {
        self.outbound
            .send((event.clone(), fan))
            .map_err(|_| anyhow::anyhow!("the relay threads are gone"))
    }

    /// Stop the threads and wait for them.
    pub fn shutdown(self) {
        for road in self.own.iter().chain(self.apps.values().flatten()) {
            road.stop.store(true, Ordering::SeqCst);
        }
        for road in self.own.into_iter().chain(self.apps.into_values().flatten()) {
            let _ = road.handle.join();
        }
    }
}

/// One relay's whole life: connect, subscribe, serve, die, retry. Never
/// panics and never gives up until `stop` — a relay that comes back has
/// to find the bunker waiting.
fn one_relay(
    url: String,
    own: bool,
    app: Option<PublicKey>,
    bunker_pubkey: PublicKey,
    inbound: Sender<Event>,
    status: Sender<RelayStatus>,
    outbound: Arc<Mutex<Receiver<(Event, Fan)>>>,
    stop: Arc<AtomicBool>,
) {
    let mut backoff = BACKOFF_START;
    loop {
        if stop.load(Ordering::SeqCst) {
            return;
        }
        match connect_and_serve(&url, own, app, bunker_pubkey, &inbound, &status, &outbound, &stop)
        {
            Ok(()) => return,
            Err(e) => {
                let _ =
                    status.send(RelayStatus { url: url.clone(), state: RelayState::Disconnected });
                if stop.load(Ordering::SeqCst) {
                    return;
                }
                // The reason goes to stderr, where the unit's journal
                // holds it; the status channel is what surfaces read.
                eprintln!("kuma-nostrd: {url}: {e:#}");
                std::thread::sleep(backoff);
                backoff = (backoff * 2).min(BACKOFF_MAX);
            }
        }
    }
}

fn connect_and_serve(
    url: &str,
    own: bool,
    app: Option<PublicKey>,
    bunker_pubkey: PublicKey,
    inbound: &Sender<Event>,
    status: &Sender<RelayStatus>,
    outbound: &Arc<Mutex<Receiver<(Event, Fan)>>>,
    stop: &AtomicBool,
) -> Result<()> {
    let (mut socket, _response) =
        tungstenite::connect(url).with_context(|| format!("connecting to {url}"))?;
    set_read_timeout(&mut socket, Some(Duration::from_secs(1)))?;

    let filter = serde_json::json!({
        "kinds": [24133],
        "#p": [bunker_pubkey.to_string()],
    });
    socket.send(Message::text(serde_json::json!(["REQ", SUBSCRIPTION_ID, filter]).to_string()))?;
    let _ = status.send(RelayStatus { url: url.to_string(), state: RelayState::Connected });

    loop {
        if stop.load(Ordering::SeqCst) {
            let _ = socket.close(None);
            return Ok(());
        }

        // Outbound first, so an answer that arrived while the read was
        // blocking goes out before anything else is read. The lock is
        // held for the drain of this beat, never across the read.
        let mut drained = Vec::new();
        {
            let outbound = outbound.lock().expect("the outbound queue lock");
            while let Ok((event, fan)) = outbound.try_recv() {
                // The road decides: the declared set's threads carry
                // everything aimed at them and every app's road; an
                // app's threads carry only their own app's traffic.
                let mine = match &fan {
                    Fan::Own => own,
                    // An app's road is the declared set plus its own
                    // relays, so the declared threads carry it too.
                    Fan::App(who) => own || app.as_ref() == Some(who),
                    Fan::OnlyApp(who) => app.as_ref() == Some(who),
                };
                if mine {
                    drained.push(event);
                }
            }
        }
        for event in drained {
            socket.send(Message::text(serde_json::json!(["EVENT", event]).to_string()))?;
        }

        match socket.read() {
            Ok(Message::Text(text)) => {
                eprintln!("kuma-nostrd: frame: {}", &text.chars().take(120).collect::<String>());
                let events = events_from_relay_message(&text, SUBSCRIPTION_ID);
                if !events.is_empty() {
                    // One line per bunker-addressed event: when an app's
                    // ask never becomes a prompt, this is the line that
                    // says whether the relay ever spoke.
                    eprintln!("kuma-nostrd: relayed {} event(s) from the wire", events.len());
                }
                for event in events {
                    let _ = inbound.send(event);
                }
            }
            Ok(Message::Ping(payload)) => socket.send(Message::Pong(payload))?,
            Ok(Message::Pong(_)) => {}
            Ok(_) => {}
            Err(tungstenite::Error::Io(e))
                if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::TimedOut =>
            {
                // The read timeout: the loop's heartbeat, not a fault.
            }
            Err(tungstenite::Error::Protocol(
                tungstenite::error::ProtocolError::ResetWithoutClosingHandshake,
            )) => return Err(anyhow!("the relay reset the connection")),
            Err(e) => return Err(anyhow::anyhow!(e).context("reading from the relay")),
        }
    }
}

/// Parse a relay's `["EVENT", <sub>, <event>]` frames into events. The
/// subscription id is checked because a relay that ignores the filter
/// must not hand the bunker another app's traffic; a frame that parses
/// to nothing is skipped — relays are noisy, and the bunker's own
/// filter is the second line of defense, not the first.
fn events_from_relay_message(text: &str, expected_subscription: &str) -> Vec<Event> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return Vec::new();
    };
    let array = value.as_array().unwrap_or(&Vec::new()).clone();
    let kind_word = array.first().and_then(|v| v.as_str());
    if kind_word != Some("EVENT") {
        return Vec::new();
    }
    let subscription = array.get(1).and_then(|v| v.as_str());
    if subscription != Some(expected_subscription) {
        return Vec::new();
    }
    match array.get(2) {
        Some(event) => match serde_json::from_value::<Event>(event.clone()) {
            Ok(event) => vec![event],
            Err(_) => Vec::new(),
        },
        None => Vec::new(),
    }
}

/// The read timeout is the loop's heartbeat: outbound frames go out and
/// the stop flag is honored on its beat. TLS streams wrap the TcpStream;
/// the plain arm is what every test and every ws:// relay uses.
fn set_read_timeout(
    socket: &mut tungstenite::WebSocket<MaybeTlsStream<std::net::TcpStream>>,
    timeout: Option<Duration>,
) -> Result<()> {
    match socket.get_mut() {
        MaybeTlsStream::Plain(stream) => {
            stream.set_read_timeout(timeout).context("setting the relay read timeout")
        }
        MaybeTlsStream::Rustls(stream) => {
            stream.get_ref().set_read_timeout(timeout).context("setting the relay read timeout")
        }
        other => Err(anyhow!("unsupported relay transport: {other:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nostr::test_relay::{wait_for, StubRelay};

    #[test]
    fn a_request_event_flows_relay_to_pool_and_an_answer_flows_back() {
        let signer = Keys::generate();
        let app = Keys::generate();
        let bunker_pubkey = signer.public_key();

        // The app's request, encrypted and signed exactly as the bunker
        // tests build one — here it travels over a real socket instead.
        let message = NostrConnectMessage::request(
            &NostrConnectRequest::from_message(NostrConnectMethod::Ping, vec![]).unwrap(),
        );
        let content = app.nip44_encrypt(&bunker_pubkey, &message.as_json()).unwrap();
        let request = EventBuilder::new(Kind::NostrConnect, content)
            .tag(Tag::public_key(bunker_pubkey))
            .finalize(&app)
            .unwrap();

        let stub = StubRelay::start(vec![request.clone()]);
        let (inbound_tx, inbound_rx) = channel::<Event>();
        let (status_tx, status_rx) = channel::<RelayStatus>();
        let pool = RelayPool::spawn(vec![stub.url.clone()], bunker_pubkey, inbound_tx, status_tx);

        // The request arrives on the inbound channel; receiving waits
        // for it rather than polling, because a poll would eat the
        // event the assertion below wants.
        let delivered = inbound_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(delivered.id, request.id);
        assert_eq!(delivered.kind, Kind::NostrConnect);

        // The connection state was reported upward.
        wait_for("the connected status", 100, || {
            status_rx.try_iter().any(|s| s.state == RelayState::Connected)
        });

        // The bunker's answer goes out through the pool and lands in
        // the stub's received pile as an EVENT frame.
        let answer = EventBuilder::new(Kind::from_u16(24133), "answer-content")
            .tag(Tag::public_key(app.public_key()))
            .finalize(&signer)
            .unwrap();
        pool.publish(&answer).unwrap();
        wait_for("the answer to reach the relay", 100, || {
            stub.received().iter().any(|frame| frame.contains(&answer.id.to_string()))
        });
        let published = stub
            .received()
            .into_iter()
            .find(|frame| frame.contains("\"EVENT\""))
            .expect("the answer was published as an EVENT frame");

        // The frame shape is the publish protocol's own: an array whose
        // second member is the event.
        let parsed: serde_json::Value = serde_json::from_str(&published).unwrap();
        assert_eq!(parsed[0], "EVENT");
        assert_eq!(parsed[1]["id"], answer.id.to_string());
        pool.shutdown();
    }

    #[test]
    fn frames_from_another_subscription_are_not_forwarded() {
        let signer = Keys::generate();
        let app = Keys::generate();
        let bunker_pubkey = signer.public_key();

        // The same request event, but delivered by a stub whose
        // subscription id lies: the pool filters it the way it must
        // filter a relay that ignored the filter.
        let message = NostrConnectMessage::request(
            &NostrConnectRequest::from_message(NostrConnectMethod::Ping, vec![]).unwrap(),
        );
        let content = app.nip44_encrypt(&bunker_pubkey, &message.as_json()).unwrap();
        let request = EventBuilder::new(Kind::NostrConnect, content)
            .tag(Tag::public_key(bunker_pubkey))
            .finalize(&app)
            .unwrap();

        // The stub announces the event under a foreign subscription id.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("ws://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let foreign = serde_json::json!(["EVENT", "someone-else", request]).to_string();
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut socket = tungstenite::accept(stream).unwrap();
            loop {
                match socket.read().unwrap() {
                    Message::Text(_) => break,
                    _ => continue,
                }
            }
            socket.send(Message::text(foreign)).unwrap();
            std::thread::sleep(Duration::from_millis(500));
        });

        let (inbound_tx, inbound_rx) = channel::<Event>();
        let (status_tx, _status_rx) = channel::<RelayStatus>();
        let pool = RelayPool::spawn(vec![url], bunker_pubkey, inbound_tx, status_tx);
        std::thread::sleep(Duration::from_millis(300));
        assert!(
            inbound_rx.recv_timeout(Duration::from_millis(500)).is_err(),
            "a foreign subscription's event must not reach the bunker"
        );
        pool.shutdown();
    }

    // The parser's own table, for the frames a live relay actually
    // sends: ours, foreign, malformed, and the NOTICE chatter that is
    // not an event at all.
    #[test]
    fn the_frame_parser_distinguishes_the_four_kinds_of_chatter() {
        let event = EventBuilder::new(Kind::TextNote, "x").finalize(&Keys::generate()).unwrap();
        let ours = serde_json::json!(["EVENT", SUBSCRIPTION_ID, event]).to_string();
        assert_eq!(events_from_relay_message(&ours, SUBSCRIPTION_ID).len(), 1);

        let foreign = serde_json::json!(["EVENT", "other", event]).to_string();
        assert!(events_from_relay_message(&foreign, SUBSCRIPTION_ID).is_empty());

        assert!(events_from_relay_message("not json", SUBSCRIPTION_ID).is_empty());
        assert!(events_from_relay_message(r#"["NOTICE","hi"]"#, SUBSCRIPTION_ID).is_empty());
        assert!(events_from_relay_message(r#"["EVENT"]"#, SUBSCRIPTION_ID).is_empty());
    }
}
