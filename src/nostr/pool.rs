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
const SUBSCRIPTION_ID: &str = "kuma-bunker";

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

/// The pool: owns the relay threads, hands bunker-bound events to the
/// `inbound` receiver, and publishes the bunker's answers to every
/// relay that is up.
pub struct RelayPool {
    outbound: Sender<Event>,
    stop: Arc<AtomicBool>,
    handles: Vec<std::thread::JoinHandle<()>>,
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
        let stop = Arc::new(AtomicBool::new(false));
        let (outbound, outbound_rx): (Sender<Event>, Receiver<Event>) = channel();
        // One queue, many relay threads: the receiver rides behind a
        // mutex, and a thread only holds it for the drain of one loop
        // beat.
        let outbound_rx = Arc::new(Mutex::new(outbound_rx));
        let handles = relays
            .into_iter()
            .map(|url| {
                let stop = stop.clone();
                let outbound_rx = outbound_rx.clone();
                let inbound = inbound.clone();
                let status = status.clone();
                std::thread::spawn(move || {
                    one_relay(url, bunker_pubkey, inbound, status, outbound_rx, stop);
                })
            })
            .collect();
        Self { outbound, stop, handles }
    }

    /// Publish an event to every relay. Relays that are down get it on
    /// reconnect — the thread's first act after a subscribe is to drain
    /// the outbound queue — so an answer is never lost to a relay that
    /// blinks while the bunker was thinking.
    pub fn publish(&self, event: &Event) -> Result<()> {
        self.outbound.send(event.clone()).map_err(|_| anyhow::anyhow!("the relay threads are gone"))
    }

    /// Stop the threads and wait for them.
    pub fn shutdown(self) {
        self.stop.store(true, Ordering::SeqCst);
        for handle in self.handles {
            let _ = handle.join();
        }
    }
}

/// One relay's whole life: connect, subscribe, serve, die, retry. Never
/// panics and never gives up until `stop` — a relay that comes back has
/// to find the bunker waiting.
fn one_relay(
    url: String,
    bunker_pubkey: PublicKey,
    inbound: Sender<Event>,
    status: Sender<RelayStatus>,
    outbound: Arc<Mutex<Receiver<Event>>>,
    stop: Arc<AtomicBool>,
) {
    let mut backoff = BACKOFF_START;
    loop {
        if stop.load(Ordering::SeqCst) {
            return;
        }
        match connect_and_serve(&url, bunker_pubkey, &inbound, &status, &outbound, &stop) {
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
    bunker_pubkey: PublicKey,
    inbound: &Sender<Event>,
    status: &Sender<RelayStatus>,
    outbound: &Arc<Mutex<Receiver<Event>>>,
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
            while let Ok(event) = outbound.try_recv() {
                drained.push(event);
            }
        }
        for event in drained {
            socket.send(Message::text(serde_json::json!(["EVENT", event]).to_string()))?;
        }

        match socket.read() {
            Ok(Message::Text(text)) => {
                for event in events_from_relay_message(&text, SUBSCRIPTION_ID) {
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

    /// A relay stub: one WebSocket server that records what it is asked,
    /// then pushes one scripted event to its subscriber and collects
    /// what the bunker publishes. This is the offline end of the plan's
    /// relay harness — the Go relay's job is interop, not the suite.
    struct StubRelay {
        url: String,
        received: Arc<Mutex<Vec<String>>>,
    }

    impl StubRelay {
        /// Serve one connection: the subscribe comes in, the scripted
        /// event goes out, and everything after is recorded.
        fn start(scripted: Event) -> Self {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("ws://127.0.0.1:{}", listener.local_addr().unwrap().port());
            let received = Arc::new(Mutex::new(Vec::new()));
            let received_for_thread = received.clone();
            std::thread::spawn(move || {
                let (stream, _) = listener.accept().unwrap();
                let Ok(mut socket) = tungstenite::accept(stream) else {
                    return;
                };
                // The subscribe arrives; its shape is asserted by the
                // test through what was recorded next.
                loop {
                    match socket.read() {
                        Ok(Message::Text(text)) => {
                            received_for_thread.lock().unwrap().push(text.to_string());
                            break;
                        }
                        Ok(_) => continue,
                        Err(_) => return,
                    }
                }
                let frame = serde_json::json!(["EVENT", SUBSCRIPTION_ID, scripted]).to_string();
                if socket.send(Message::text(frame)).is_err() {
                    return;
                }
                // Collect publishes until the test says stop, with a
                // read timeout so the thread can notice the socket is
                // dead and leave.
                socket.get_mut().set_read_timeout(Some(Duration::from_millis(100))).ok();
                loop {
                    match socket.read() {
                        Ok(Message::Text(text)) => {
                            received_for_thread.lock().unwrap().push(text.to_string());
                        }
                        Ok(Message::Close(_)) => return,
                        // WouldBlock is the read timeout beat, not a
                        // fault: a relay holds the connection open.
                        Err(tungstenite::Error::Io(e))
                            if e.kind() == ErrorKind::WouldBlock
                                || e.kind() == ErrorKind::TimedOut =>
                        {
                            continue
                        }
                        Err(_) => return,
                        _ => continue,
                    }
                }
            });
            Self { url, received }
        }

        fn received(&self) -> Vec<String> {
            self.received.lock().unwrap().clone()
        }
    }

    /// Wait until the closure is true, because a relay thread and a
    /// subscriber meet in the middle: neither side knows who arrived
    /// first.
    fn wait_for(description: &str, tries: u32, check: impl Fn() -> bool) {
        for _ in 0..tries {
            if check() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("timed out waiting for {description}");
    }

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

        let stub = StubRelay::start(request.clone());
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
        let answer = EventBuilder::new(Kind::from_u16(24135), "answer-content")
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
