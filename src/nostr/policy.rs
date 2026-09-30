//! The policy engine: who may do what, who asks, and what was decided.
//!
//! The model is Opal's, borrowed whole because it is right, with one
//! deliberate divergence the plan recorded: a newly paired app defaults
//! to **Ask** for everything, not Basic — for a signer a person opts
//! into on purpose, the strict default is the honest one, and relaxing
//! to Basic is one toggle in the panel.
//!
//! * **Basic** — everyday methods sign unattended; sensitive ones ask.
//! * **Ask** — everything asks.
//! * **Trust** — everything signs unattended, sensitive included. This
//!   is what makes Trust the loudest thing in the layer, and why the
//!   doctor grades any app holding it Warn by name.
//!
//! Sensitive — the plan's list: profile and follow writes, relay-list
//! and mute-list writes, deletions, and the decrypt methods, which read
//! what was meant to be private. A sensitive ask may be remembered for
//! at most an hour; that is the longest standing grant the engine can
//! mint, and `approve --remember` is how.
//!
//! Every decision lands in the activity log with its reason, so the
//! panel and the doctor answer "what did this app do" from the same
//! record the decisions made. Privacy mode is the default: entries
//! carry the method, the event kind, and the verdict — never the
//! params, never the content.
//!
//! The kill switch is `lock`: it drops the keys, and a bunker with no
//! keys refuses everything by construction. The engine does not keep a
//! second switch, because two switches are two ways to believe the
//! bunker is dead when it is not.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Result};
use nostr::key::PublicKey;
use nostr::nips::nip46::NostrConnectMethod;
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use super::bunker::Decision;

/// The three levels a paired app can hold. The default for a newly
/// paired app is `Ask`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Basic,
    Ask,
    Trust,
}

/// Kinds whose writes always ask at Basic: profile, follows, deletions,
/// mute list, relay list — the writes that describe the identity or
/// reshape who sees it.
const SENSITIVE_KINDS: &[u16] = &[0, 3, 5, 10000, 10002];

/// Whether this method call reads a private payload or writes a
/// sensitive part of the identity. The decrypt methods always do; a
/// sign_event does when the unsigned event's kind does.
fn is_sensitive(method: &NostrConnectMethod, params: &[String]) -> bool {
    match method {
        NostrConnectMethod::Nip04Decrypt | NostrConnectMethod::Nip44Decrypt => true,
        NostrConnectMethod::SignEvent => params
            .first()
            .and_then(|json| nostr::event::UnsignedEvent::from_json(json).ok())
            .map(|event| u16::from(event.kind) as u32)
            .map(|kind| SENSITIVE_KINDS.contains(&(kind as u16)))
            .unwrap_or(false),
        _ => false,
    }
}

/// One paired app: identity, level, and when it was paired. The file
/// form is what persists; the daemon restarts to find its pairings
/// intact, because re-pairing every app after every reboot is how a
/// person stops trusting the feature.
///
/// `name` and `image` are the client metadata the connect may carry
/// (NIP-46's optional fourth param) — the app's own claim about
/// itself, unauthenticated by the protocol's own word, so the panel
/// renders it as a display hint and nothing authorizes by it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Paired {
    pub pubkey: String,
    pub level: Level,
    pub paired_at: u64,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub image: Option<String>,
}

/// What `prompts` shows: the ask, enough to decide on. `summary` is
/// the glance; `detail` is the exact event the approval renders before
/// the finger commits — the unsigned event JSON for a sign, the payload
/// shape otherwise, and nothing for the verbs that carry no content.
#[derive(Debug, Clone, Serialize)]
pub struct PromptView {
    pub id: String,
    pub app: String,
    pub method: String,
    pub summary: String,
    pub detail: Option<String>,
}

struct Prompt {
    app: PublicKey,
    method: NostrConnectMethod,
    params: Vec<String>,
    summary: String,
    responder: oneshot::Sender<Decision>,
}

/// One line of the activity log: what was asked, by whom, and why it
/// went the way it went. Privacy mode is structural — there is no field
/// a param could hide in.
#[derive(Debug, Clone, Serialize)]
pub struct LogEntry {
    pub at: u64,
    pub app: String,
    pub method: String,
    pub summary: String,
    pub verdict: String,
}

#[derive(Default)]
struct Inner {
    apps: Vec<Paired>,
    prompts: Vec<(String, Prompt)>,
    /// `{pubkey}:{method}` until unix-seconds — the remembered answers,
    /// an hour at most by construction of the verb.
    remembered: HashMap<String, u64>,
    log: Vec<LogEntry>,
    next_id: u64,
}

/// The engine: the [`super::bunker::Gate`] the bunker's worker asks,
/// and the state the socket verbs (`prompts`, `approve`, `deny`,
/// `apps`, `revoke`) drive. Cloneable on purpose — the handle is cheap
/// and the state is shared — because the verbs and the gate are two
/// roads into one decision record.
#[derive(Clone)]
pub struct Engine {
    inner: Arc<Mutex<Inner>>,
    state_path: Option<PathBuf>,
}

impl Engine {
    /// `state_dir` is where `apps.json` lives; `None` makes the engine
    /// memory-only, which is what the offline tests run against.
    pub fn new(state_dir: Option<PathBuf>) -> Self {
        let engine = Self { inner: Arc::new(Mutex::new(Inner::default())), state_path: state_dir };
        engine.load_apps();
        engine
    }

    /// The state file the pairings persist to, if this engine persists.
    fn apps_file(&self) -> Option<PathBuf> {
        self.state_path.as_ref().map(|dir| dir.join("apps.json"))
    }

    fn load_apps(&self) {
        let Some(file) = self.apps_file() else { return };
        let Ok(text) = std::fs::read_to_string(&file) else { return };
        let Ok(apps) = serde_json::from_str::<Vec<Paired>>(&text) else {
            eprintln!("kuma-nostrd: {file:?} did not parse; starting with no pairings");
            return;
        };
        self.inner.lock().expect("the policy lock").apps = apps;
    }

    fn persist_apps(&self, inner: &Inner) {
        let Some(file) = self.apps_file() else { return };
        if let Some(parent) = file.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                eprintln!("kuma-nostrd: cannot create {parent:?}: {e}");
                return;
            }
        }
        match serde_json::to_string_pretty(&inner.apps) {
            Ok(text) => {
                if let Err(e) = std::fs::write(&file, text) {
                    eprintln!("kuma-nostrd: cannot write {file:?}: {e}");
                }
            }
            Err(e) => eprintln!("kuma-nostrd: the pairing list did not serialize: {e}"),
        }
    }

    /// Pair an app, or find it already paired. The default level is
    /// Ask, and no path changes a level except the panel's toggle —
    /// pairing is not a decision the daemon makes for anyone. The
    /// metadata (the app's name and image, its own unverified claim)
    /// lands on the first pairing and fills in on reconnects: an app
    /// that ships a name later gets the better label.
    pub fn pair_with_metadata(&self, app: &PublicKey, name: Option<String>, image: Option<String>) {
        let mut inner = self.inner.lock().expect("the policy lock");
        let hex = app.to_string();
        if let Some(paired) = inner.apps.iter_mut().find(|p| p.pubkey == hex) {
            if paired.name.is_none() {
                paired.name = name;
            }
            if paired.image.is_none() {
                paired.image = image;
            }
            return;
        }
        inner.apps.push(Paired {
            pubkey: hex.clone(),
            level: Level::Ask,
            paired_at: unix_now(),
            name,
            image,
        });
        inner.log.push(LogEntry {
            at: unix_now(),
            app: hex,
            method: "connect".into(),
            summary: "paired".into(),
            verdict: "paired at ask".into(),
        });
        self.persist_apps(&inner);
    }

    fn pair(&self, app: &PublicKey) {
        self.pair_with_metadata(app, None, None);
    }

    /// The paired apps, for the `apps` verb.
    pub fn apps(&self) -> Vec<Paired> {
        self.inner.lock().expect("the policy lock").apps.clone()
    }

    /// Forget an app. Answers whether one was actually removed, so the
    /// verb can tell the caller "no such app" instead of nodding.
    pub fn revoke(&self, app: &str) -> bool {
        let mut inner = self.inner.lock().expect("the policy lock");
        let before = inner.apps.len();
        inner.apps.retain(|p| p.pubkey != app);
        inner.remembered.retain(|key, _| !key.starts_with(&format!("{app}:")));
        let removed = inner.apps.len() < before;
        if removed {
            self.persist_apps(&inner);
        }
        removed
    }

    /// The pending asks, for the `prompts` verb and the panel.
    pub fn prompts(&self) -> Vec<PromptView> {
        let inner = self.inner.lock().expect("the policy lock");
        inner
            .prompts
            .iter()
            .map(|(id, prompt)| PromptView {
                id: id.clone(),
                app: prompt.app.to_string(),
                method: format!("{:?}", prompt.method),
                summary: prompt.summary.clone(),
                detail: detail(&prompt.method, &prompt.params),
            })
            .collect()
    }

    /// Answer an ask with yes. `remember` grants the same method a
    /// standing answer for the given duration — the longest the engine
    /// can mint, and the only standing grant besides Trust.
    pub fn approve(&self, id: &str, remember: Option<Duration>) -> Result<()> {
        self.answer(id, Decision::Allow, remember)
    }

    /// Answer an ask with no.
    pub fn deny(&self, id: &str) -> Result<()> {
        self.answer(id, Decision::Deny("denied".into()), None)
    }

    fn answer(&self, id: &str, decision: Decision, remember: Option<Duration>) -> Result<()> {
        let mut inner = self.inner.lock().expect("the policy lock");
        let index = inner
            .prompts
            .iter()
            .position(|(prompt_id, _)| prompt_id == id)
            .ok_or_else(|| anyhow!("no prompt {id}"))?;
        let (_, prompt) = inner.prompts.remove(index);
        if let Some(duration) = remember {
            let key = remember_key(&prompt.app, &prompt.method);
            inner.remembered.insert(key, unix_now() + duration.as_secs());
        }
        inner.log.push(LogEntry {
            at: unix_now(),
            app: prompt.app.to_string(),
            method: format!("{:?}", prompt.method),
            summary: prompt.summary.clone(),
            verdict: match &decision {
                Decision::Allow => "allowed".into(),
                Decision::Deny(reason) => format!("denied: {reason}"),
            },
        });
        // A dropped send means the waiting bunker worker is gone — the
        // request's app already timed out — and the log still says the
        // decision happened, which is the part that must not be lost.
        let _ = prompt.responder.send(decision);
        Ok(())
    }

    /// The activity log, newest last.
    pub fn activity(&self) -> Vec<LogEntry> {
        self.inner.lock().expect("the policy lock").log.clone()
    }

    /// Set an app's level — the panel's one toggle, the thing that
    /// relaxes a new app's Ask to Basic or escalates to the standing
    /// grant the doctor will warn about. Unknown apps are refused:
    /// leveling an app that never paired is a typo, not a policy.
    pub fn set_level(&self, app: &str, level: Level) -> Result<()> {
        let mut inner = self.inner.lock().expect("the policy lock");
        let paired = inner
            .apps
            .iter_mut()
            .find(|p| p.pubkey == app)
            .ok_or_else(|| anyhow!("no paired app {app}"))?;
        paired.level = level;
        self.persist_apps(&inner);
        Ok(())
    }
}

impl super::bunker::Gate for Engine {
    async fn decide(
        &self,
        app: &PublicKey,
        method: &NostrConnectMethod,
        params: &[String],
    ) -> Decision {
        self.pair(app);
        let summary = summarize(method, params);
        let sensitive = is_sensitive(method, params);

        {
            let mut inner = self.inner.lock().expect("the policy lock");
            let level = inner
                .apps
                .iter()
                .find(|p| p.pubkey == app.to_string())
                .map(|p| p.level)
                .unwrap_or(Level::Ask);
            let trusted_everywhere = level == Level::Trust;
            let trusted_here = inner
                .remembered
                .get(&remember_key(app, method))
                .is_some_and(|until| *until > unix_now());
            let allowed_without_asking =
                trusted_everywhere || trusted_here || (level == Level::Basic && !sensitive);
            if allowed_without_asking {
                let verdict = if trusted_everywhere {
                    "allowed (trust)"
                } else if trusted_here {
                    "allowed (remembered)"
                } else {
                    "allowed (basic)"
                };
                inner.log.push(LogEntry {
                    at: unix_now(),
                    app: app.to_string(),
                    method: format!("{method:?}"),
                    summary: summary.clone(),
                    verdict: verdict.into(),
                });
                return Decision::Allow;
            }
        }

        // The ask: an id, a channel, and the lock released the moment
        // the prompt is registered — the answer comes back through the
        // oneshot whenever it comes.
        let (tx, rx) = oneshot::channel();
        let id = {
            let mut inner = self.inner.lock().expect("the policy lock");
            inner.next_id += 1;
            let id = format!("{}-{:04}", unix_now(), inner.next_id);
            inner.prompts.push((
                id.clone(),
                Prompt {
                    app: *app,
                    method: *method,
                    params: params.to_vec(),
                    summary: summary.clone(),
                    responder: tx,
                },
            ));
            id
        };
        eprintln!("kuma-nostrd: asking {id}: {summary}");
        match rx.await {
            Ok(decision) => decision,
            Err(_) => Decision::Deny("the prompt was dropped".into()),
        }
    }
}

/// The exact event an approval shows, privacy mode's one exception: a
/// signature cannot be judged blind, so the unsigned event rides in
/// full. The decrypt methods name their scope without their payload —
/// the payload is the secret, and the prompt is rendered on screens.
fn detail(method: &NostrConnectMethod, params: &[String]) -> Option<String> {
    match method {
        NostrConnectMethod::SignEvent => params.first().cloned(),
        NostrConnectMethod::Nip04Decrypt | NostrConnectMethod::Nip44Decrypt => {
            params.first().map(|pk| format!("decrypt for {pk}"))
        }
        _ => None,
    }
}

/// What a glance decides on: the method, and for a sign_event the kind
/// it writes. Never the params, never the content — the summary that
/// names nothing private is the one that goes in the log and the
/// prompt list alike.
fn summarize(method: &NostrConnectMethod, params: &[String]) -> String {
    match method {
        NostrConnectMethod::SignEvent => params
            .first()
            .and_then(|json| nostr::event::UnsignedEvent::from_json(json).ok())
            .map(|event| format!("sign a kind {} event", u16::from(event.kind)))
            .unwrap_or_else(|| "sign an unreadable event".into()),
        other => format!("{other:?}").to_lowercase(),
    }
}

fn remember_key(app: &PublicKey, method: &NostrConnectMethod) -> String {
    format!("{}:{method:?}", app)
}

pub(crate) fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

#[cfg(test)]
mod tests {
    use super::super::bunker::Gate;
    use super::*;

    fn engine() -> Engine {
        Engine::new(None)
    }

    fn app() -> PublicKey {
        nostr::key::Keys::generate().public_key()
    }

    fn everyday(method: &NostrConnectMethod) -> Vec<String> {
        match method {
            // A text note is as everyday as signing gets.
            NostrConnectMethod::SignEvent => {
                let unsigned = nostr::event::UnsignedEvent::new(
                    nostr::key::Keys::generate().public_key(),
                    nostr::types::Timestamp::now(),
                    nostr::event::Kind::TextNote,
                    [],
                    "hello",
                );
                vec![unsigned.as_json()]
            }
            _ => vec![],
        }
    }

    fn profile_write() -> Vec<String> {
        let unsigned = nostr::event::UnsignedEvent::new(
            nostr::key::Keys::generate().public_key(),
            nostr::types::Timestamp::now(),
            nostr::event::Kind::Metadata,
            [],
            "{}",
        );
        vec![unsigned.as_json()]
    }

    #[tokio::test]
    async fn a_newly_paired_app_defaults_to_ask_everything() {
        let engine = engine();
        let app = app();
        // The ask runs apart from the answer, the way the worker and
        // the socket verb do: an inline await here would wait on a
        // prompt nobody has answered yet.
        let engine_for_ask = engine.clone();
        let ask = tokio::spawn(async move {
            engine_for_ask.decide(&app, &NostrConnectMethod::GetPublicKey, &[]).await
        });
        tokio::task::yield_now().await;
        let prompts = engine.prompts();
        assert_eq!(prompts.len(), 1);
        assert_eq!(prompts[0].method, "GetPublicKey");
        engine.approve(&prompts[0].id, None).unwrap();
        assert!(matches!(ask.await.unwrap(), Decision::Allow));
        assert!(engine.prompts().is_empty(), "an answered prompt leaves the queue");
    }

    #[tokio::test]
    async fn ask_blocks_until_the_answer_arrives() {
        let engine = engine();
        let app = app();
        let engine_for_answer = engine.clone();
        let ask = tokio::spawn(async move {
            engine_for_answer.decide(&app, &NostrConnectMethod::GetPublicKey, &[]).await
        });
        tokio::task::yield_now().await;
        let prompt = engine.prompts().into_iter().next().expect("the ask is queued");
        engine.approve(&prompt.id, None).unwrap();
        assert!(matches!(ask.await.unwrap(), Decision::Allow));
    }

    #[tokio::test]
    async fn deny_denies_and_the_reason_travels() {
        let engine = engine();
        let app = app();
        let engine_for_answer = engine.clone();
        let ask = tokio::spawn(async move {
            engine_for_answer.decide(&app, &NostrConnectMethod::GetPublicKey, &[]).await
        });
        tokio::task::yield_now().await;
        engine.deny(&engine.prompts()[0].id).unwrap();
        match ask.await.unwrap() {
            Decision::Deny(reason) => assert!(reason.contains("denied")),
            Decision::Allow => panic!("a deny arrived as an allow"),
        }
        let log = engine.activity();
        assert!(log.last().unwrap().verdict.contains("denied"));
    }

    #[tokio::test]
    async fn remember_answers_the_next_one_without_asking() {
        let engine = engine();
        let app = app();
        let engine_for_ask = engine.clone();
        let ask = tokio::spawn(async move {
            engine_for_ask.decide(&app, &NostrConnectMethod::GetPublicKey, &[]).await
        });
        tokio::task::yield_now().await;
        engine.approve(&engine.prompts()[0].id, Some(Duration::from_secs(3600))).unwrap();
        assert!(matches!(ask.await.unwrap(), Decision::Allow));

        // The second ask of the same method is answered by the
        // remember grant: no prompt, allow, and the log says why.
        let second = engine.decide(&app, &NostrConnectMethod::GetPublicKey, &[]).await;
        assert!(matches!(second, Decision::Allow));
        assert!(engine.prompts().is_empty());
        assert!(engine.activity().iter().any(|e| e.verdict.contains("remembered")));
    }

    #[tokio::test]
    async fn sensitive_methods_ask_at_basic_and_the_kind_is_what_makes_them_so() {
        let engine = engine();
        let app = app();
        let engine_for_ask = engine.clone();
        let ask = tokio::spawn(async move {
            engine_for_ask.decide(&app, &NostrConnectMethod::SignEvent, &profile_write()).await
        });
        tokio::task::yield_now().await;
        // Basic would wave a text note through — but a profile write
        // is on the sensitive list, so there is a prompt to answer.
        engine.approve(&engine.prompts()[0].id, None).unwrap();
        assert!(matches!(ask.await.unwrap(), Decision::Allow));

        // The panel's toggle: the app relaxes to Basic, and now the
        // kind is what separates a wave-through from an ask.
        engine.set_level(&app.to_string(), Level::Basic).unwrap();

        // The everyday sign asks nothing.
        let everyday = engine
            .decide(&app, &NostrConnectMethod::SignEvent, &everyday(&NostrConnectMethod::SignEvent))
            .await;
        assert!(matches!(everyday, Decision::Allow));

        // The profile write still does — the spawned shape, because
        // the answer comes from the queue.
        let engine_for_ask = engine.clone();
        let ask = tokio::spawn(async move {
            engine_for_ask.decide(&app, &NostrConnectMethod::SignEvent, &profile_write()).await
        });
        tokio::task::yield_now().await;
        assert_eq!(engine.prompts().len(), 1, "a sensitive write asks at Basic");
        engine.approve(&engine.prompts()[0].id, None).unwrap();
        assert!(matches!(ask.await.unwrap(), Decision::Allow));
        assert!(engine.prompts().is_empty());
    }

    #[tokio::test]
    async fn trust_is_the_standing_grant_the_doctor_warns_about() {
        let engine = engine();
        let app = app();
        // Pair by asking once, then escalate to Trust the way the panel
        // will: through the engine's own state.
        let engine_for_ask = engine.clone();
        let ask = tokio::spawn(async move {
            engine_for_ask.decide(&app, &NostrConnectMethod::GetPublicKey, &[]).await
        });
        tokio::task::yield_now().await;
        engine.approve(&engine.prompts()[0].id, None).unwrap();
        ask.await.unwrap();

        {
            let mut inner = engine.inner.lock().unwrap();
            for paired in inner.apps.iter_mut() {
                paired.level = Level::Trust;
            }
        }
        let sensitive = engine
            .decide(&app, &NostrConnectMethod::Nip44Decrypt, &["pk".into(), "payload".into()])
            .await;
        assert!(matches!(sensitive, Decision::Allow), "trust is a standing grant");
        assert!(engine.prompts().is_empty());
    }

    #[tokio::test]
    async fn at_basic_nip44_encrypt_runs_unattended_and_nip04_encrypt_asks() {
        let engine = engine();
        let app = app();
        // Pair by asking once, then relax to Basic the way the panel will.
        let engine_for_ask = engine.clone();
        let ask = tokio::spawn(async move {
            engine_for_ask.decide(&app, &NostrConnectMethod::GetPublicKey, &[]).await
        });
        tokio::task::yield_now().await;
        engine.approve(&engine.prompts()[0].id, None).unwrap();
        ask.await.unwrap();
        engine.set_level(&app.to_string(), Level::Basic).unwrap();

        // NIP-44 is general-purpose encryption: it runs without a prompt.
        let allowed = engine
            .decide(
                &app,
                &NostrConnectMethod::Nip44Encrypt,
                &[app.to_string(), "text".into()],
            )
            .await;
        assert!(matches!(allowed, Decision::Allow));
        assert!(engine.prompts().is_empty());

        // NIP-04's job is private messages; encrypting one is writing one.
        let engine_for_ask = engine.clone();
        let ask = tokio::spawn(async move {
            engine_for_ask
                .decide(
                    &app,
                    &NostrConnectMethod::Nip04Encrypt,
                    &[app.to_string(), "text".into()],
                )
                .await
        });
        tokio::task::yield_now().await;
        assert_eq!(engine.prompts().len(), 1, "nip04_encrypt asks at Basic");
        engine.approve(&engine.prompts()[0].id, None).unwrap();
        assert!(matches!(ask.await.unwrap(), Decision::Allow));
    }

    #[tokio::test]
    async fn pairings_persist_through_a_reboot_of_the_engine() {
        let dir = tempfile::tempdir().unwrap();
        let app = app();
        {
            let engine = Engine::new(Some(dir.path().to_path_buf()));
            let engine_for_ask = engine.clone();
            let ask = tokio::spawn(async move {
                engine_for_ask.decide(&app, &NostrConnectMethod::GetPublicKey, &[]).await
            });
            tokio::task::yield_now().await;
            engine.approve(&engine.prompts()[0].id, None).unwrap();
            ask.await.unwrap();
        }
        // A fresh engine over the same state dir finds the pairing.
        let engine = Engine::new(Some(dir.path().to_path_buf()));
        let apps = engine.apps();
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].pubkey, app.to_string());
        assert_eq!(apps[0].level, Level::Ask);
    }

    #[test]
    fn revoke_forgets_the_app_and_its_standing_answers() {
        let engine = engine();
        let app = app();
        engine.pair(&app);
        assert_eq!(engine.apps().len(), 1);
        assert!(engine.revoke(&app.to_string()));
        assert!(engine.apps().is_empty());
        assert!(!engine.revoke(&app.to_string()), "revoking twice is not a success story");
    }
}
