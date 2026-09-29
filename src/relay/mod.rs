//! The NIP-46 relay's core: what the Go original does, in the language
//! the layer speaks.
//!
//! The relay is deliberately small — it is not a general Nostr relay
//! and does not pretend to be one. It carries only signing traffic
//! (kinds 24133 and 24135), keeps it in memory only, evicts it after
//! ten minutes, and rate-limits each connection. Everything here is
//! clock-injectable so the offline suite can test the behaviors the
//! plan names — kind filter, timestamp window, eviction, rate limits —
//! without waiting ten real minutes for anything; the binary at the
//! other end of the module tree is a thin server over these types.
//!
//! What a relay never does, and why the signer trusts it: it never
//! listens for anything but these two kinds, it never touches disk, it
//! decrypts nothing (the payloads are NIP-44 end to end), and it never
//! holds a key. The metadata it necessarily sees — which app asked
//! which bunker, how often — is the trust fact concepts.md discloses.

use std::collections::VecDeque;

use nostr::event::Event;
use nostr::key::PublicKey;

/// The kinds the relay carries: signing requests and answers. An event
/// of any other kind is refused at the door.
pub const CARRIED_KINDS: [u16; 2] = [24133, 24135];

/// How long an event stays in memory after arriving, in seconds. Ten
/// minutes is the plan's number: enough for a phone on bad network to
/// come back and read its answer, short enough that the relay holds
/// nothing worth subpoenaing.
pub const EVENT_TTL: u64 = 600;

/// How far an event's `created_at` may sit from the relay's now, in
/// seconds, before the door refuses it. A replayed request is stale by
/// definition; the window bounds how stale.
pub const TIMESTAMP_WINDOW: u64 = 600;

/// Why the door refused an event. `Duplicate` is not a refusal — it is
/// an acceptance that remembers — and the OK reply's boolean says so.
#[derive(Debug, PartialEq)]
pub enum Rejected {
    /// Not a kind this relay carries.
    Kind(u16),
    /// `created_at` outside the window in either direction.
    Timestamp,
    /// The event is already held; nothing was added.
    Duplicate,
}

/// One held event, with the relay's own clock at its arrival — the TTL
/// runs on arrival time, not the event's `created_at`, because the
/// sender's clock is the one thing this relay does not trust.
#[derive(Debug, Clone)]
pub struct Stored {
    pub event: Event,
    received_at: u64,
}

/// The subscription filter subset the bunker flow uses: kinds, the
/// `#p` addresses, and the time bounds. The full NIP-01 grammar is
/// deliberately out of scope — the daemon's subscription is the shape
/// this must serve, and a relay that implements everything is a relay
/// with a filter bug nobody can find.
#[derive(Debug, Clone, PartialEq)]
pub struct RelayFilter {
    pub kinds: Vec<u16>,
    pub p: Vec<PublicKey>,
    pub since: Option<u64>,
    pub until: Option<u64>,
    pub limit: Option<usize>,
}

impl RelayFilter {
    /// Parse the JSON object of a REQ frame. Unknown keys are ignored,
    /// because a filter that rejects its own superset would make the
    /// bunker miss requests the day the daemon learns a new key.
    pub fn from_json(value: &serde_json::Value) -> Self {
        let kinds = value
            .get("kinds")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_u64().map(|k| k as u16)).collect())
            .unwrap_or_default();
        let p = value
            .get("#p")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str())
                    .filter_map(|s| PublicKey::parse(s).ok())
                    .collect()
            })
            .unwrap_or_default();
        let since = value.get("since").and_then(|v| v.as_u64());
        let until = value.get("until").and_then(|v| v.as_u64());
        let limit = value.get("limit").and_then(|v| v.as_u64()).map(|l| l as usize);
        Self { kinds, p, since, until, limit }
    }

    /// Whether an event answers this subscription.
    pub fn matches(&self, event: &Event) -> bool {
        if !self.kinds.is_empty() && !self.kinds.contains(&event.kind.as_u16()) {
            return false;
        }
        if !self.p.is_empty()
            && !self.p.iter().any(|p| event.tags.public_keys().any(|tag| tag == *p))
        {
            return false;
        }
        let created = event.created_at.as_secs();
        if let Some(since) = self.since {
            if created < since {
                return false;
            }
        }
        if let Some(until) = self.until {
            if created > until {
                return false;
            }
        }
        true
    }
}

/// The in-memory event store: what the relay is, minus the sockets.
/// The lock is the caller's; this type is the memory and the rules.
#[derive(Default)]
pub struct EventStore {
    events: Vec<Stored>,
}

impl EventStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// The door: dedup, kind filter, timestamp window, then hold. The
    /// `now` is the relay's own clock, passed so the suite can wind it.
    pub fn insert(&mut self, event: Event, now: u64) -> Result<bool, Rejected> {
        if !CARRIED_KINDS.contains(&event.kind.as_u16()) {
            return Err(Rejected::Kind(event.kind.as_u16()));
        }
        let created = event.created_at.as_secs();
        if created.abs_diff(now) > TIMESTAMP_WINDOW {
            return Err(Rejected::Timestamp);
        }
        if self.events.iter().any(|s| s.event.id == event.id) {
            return Err(Rejected::Duplicate);
        }
        self.events.push(Stored { event, received_at: now });
        Ok(true) // fresh, held
    }

    /// Drop everything older than the TTL. Called on the eviction
    /// beat; also cheap enough to call before every query.
    pub fn evict(&mut self, now: u64) {
        self.events.retain(|stored| now.saturating_sub(stored.received_at) < EVENT_TTL);
    }

    /// The events matching a filter, oldest first, newest `limit`.
    pub fn query(&self, filter: &RelayFilter) -> Vec<Event> {
        let mut matched: Vec<Event> = self
            .events
            .iter()
            .filter(|stored| filter.matches(&stored.event))
            .map(|stored| stored.event.clone())
            .collect();
        if let Some(limit) = filter.limit {
            if matched.len() > limit {
                matched = matched.split_off(matched.len() - limit);
            }
        }
        matched
    }

    /// How many events are held — the eviction beat's reason to run.
    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}

/// A connection's admission budget: a sliding window of the last
/// minute. A bunker and its apps exchange single-digit requests per
/// minute; thirty is a ceiling a real client never touches and a flood
/// cannot sit under. `admit` answers whether this event may pass.
pub struct RateLimiter {
    budget: VecDeque<u64>,
    capacity: usize,
}

impl RateLimiter {
    /// Thirty events in a sliding minute: a ceiling a real bunker
    /// client never touches and a flood cannot sit under.
    pub fn new() -> Self {
        Self::with_capacity(30)
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self { budget: VecDeque::new(), capacity }
    }

    pub fn admit(&mut self, now: u64) -> bool {
        let window_start = now.saturating_sub(60);
        while self.budget.front().is_some_and(|at| *at < window_start) {
            self.budget.pop_front();
        }
        if self.budget.len() >= self.capacity {
            return false;
        }
        self.budget.push_back(now);
        true
    }
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::prelude::*;

    /// A kind 24133 request from a real keypair: the only shape the
    /// relay is in the business of holding.
    fn request_event(kind: u16, minutes_ago: u64) -> Event {
        let keys = Keys::generate();
        let created = Timestamp::from(Timestamp::now().as_secs() - minutes_ago * 60);
        EventBuilder::new(Kind::from_u16(kind), "payload")
            .tag(Tag::public_key(Keys::generate().public_key()))
            .custom_created_at(created)
            .finalize(&keys)
            .unwrap()
    }

    fn held(event: &Event, now: u64) -> Result<bool, Rejected> {
        let mut store = EventStore::new();
        store.insert(event.clone(), now)
    }

    #[test]
    fn the_door_carries_only_signing_traffic() {
        let now = Timestamp::now().as_secs();
        assert!(held(&request_event(24133, 0), now).is_ok());
        assert!(held(&request_event(24135, 0), now).is_ok());
        assert_eq!(
            held(&request_event(1, 0), now).unwrap_err(),
            Rejected::Kind(1),
            "a text note is not the relay's business"
        );
        assert_eq!(
            held(&request_event(0, 0), now).unwrap_err(),
            Rejected::Kind(0),
            "a profile write is not the relay's business"
        );
    }

    #[test]
    fn the_timestamp_window_refuses_stale_and_future_alike() {
        let now = Timestamp::now().as_secs();
        // Eleven minutes old: outside the window.
        assert_eq!(held(&request_event(24133, 11), now).unwrap_err(), Rejected::Timestamp);
        // Nine minutes old: inside it.
        assert!(held(&request_event(24133, 9), now).is_ok());
        // The future beyond the window is the same refusal — a replay
        // from a clock-lying sender.
        assert_eq!(
            held(&request_event(24133, 0), now + TIMESTAMP_WINDOW + 1).unwrap_err(),
            Rejected::Timestamp
        );
    }

    #[test]
    fn ttl_eviction_forgets_exactly_what_expired() {
        let mut store = EventStore::new();
        let now = Timestamp::now().as_secs();
        store.insert(request_event(24133, 0), now).unwrap();
        store.insert(request_event(24133, 0), now + 1).unwrap();

        // Nine minutes later both are still held; eleven and neither.
        store.evict(now + 9 * 60);
        assert_eq!(store.len(), 2);
        store.evict(now + 11 * 60);
        assert_eq!(store.len(), 0, "ten minutes is the whole memory of the relay");
    }

    #[test]
    fn a_duplicate_is_recognized_and_not_held_twice() {
        let mut store = EventStore::new();
        let event = request_event(24133, 0);
        let now = Timestamp::now().as_secs();
        assert!(store.insert(event.clone(), now).unwrap());
        assert_eq!(store.insert(event, now).unwrap_err(), Rejected::Duplicate);
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn query_matches_kinds_and_addresses_and_bounds() {
        let mut store = EventStore::new();
        let now = Timestamp::now().as_secs();
        let bunker = Keys::generate().public_key();
        let other = Keys::generate().public_key();

        let addressed = EventBuilder::new(Kind::from_u16(24133), "payload")
            .tag(Tag::public_key(bunker))
            .finalize(&Keys::generate())
            .unwrap();
        let stray = EventBuilder::new(Kind::from_u16(24133), "payload")
            .tag(Tag::public_key(other))
            .finalize(&Keys::generate())
            .unwrap();
        store.insert(addressed.clone(), now).unwrap();
        store.insert(stray, now).unwrap();

        // The daemon's subscription: the bunker's requests, addressed
        // to it.
        let filter = RelayFilter {
            kinds: vec![24133],
            p: vec![bunker],
            since: None,
            until: None,
            limit: None,
        };
        let matched = store.query(&filter);
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].id, addressed.id);
        assert!(matched[0].tags.public_keys().any(|tag| tag == bunker));
    }

    #[test]
    fn the_rate_limiter_admits_a_burst_and_refuses_a_flood() {
        let mut limiter = RateLimiter::new();
        let now = Timestamp::now().as_secs();
        for _ in 0..30 {
            assert!(limiter.admit(now), "the first thirty of a minute pass");
        }
        assert!(!limiter.admit(now), "the thirty-first in the same minute waits");

        // A minute later the window has slid: the budget is whole.
        assert!(limiter.admit(now + 61));
    }
}
