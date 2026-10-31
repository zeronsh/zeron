//! Desktop banners, posted by the engine so they keep coming with the app's
//! window closed: a run finished, an agent is waiting on you, a run failed,
//! the connection went away, agent updates are ready.
//!
//! Only desktop engines post (the app's background engine and its
//! in-process fallback call [`enable`]); a server or Cloud machine never
//! does. The decision is the one the app used to make itself:
//!
//! - one baseline per session; a row's first appearance seeds it silently,
//!   so a boot or replay never rings;
//! - side chats keep their baselines current but never ring;
//! - preferences come from the app's `ui-settings.json` (read when a banner
//!   is due, so changes apply at once): `notificationsEnabled`,
//!   `notificationsBackgroundOnly`, `agentUpdateNotifications`;
//! - "background only" holds banners while any app window is focused, as
//!   viewports report through `SetViewportFocus` (the app's chime covers
//!   the foreground case).
//!
//! The app keeps the chimes; it uses [`SessionNotificationState`] and
//! [`ConnectivityNotificationState`] for them, so sound and banner always
//! agree on what happened.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use zeron_proto::view::{Indicator, effective_indicator};
use zeron_proto::{Chat, ConnectivityState, HarnessUpdatePhase, HarnessUpdateStatus, Session};

static ENABLED: AtomicBool = AtomicBool::new(false);

/// This process shows desktop banners: engines assembled from now on post
/// them (and advertise [`zeron_proto::capabilities::DESKTOP_NOTIFICATIONS_V1`]).
pub fn enable() {
    ENABLED.store(true, Ordering::SeqCst);
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::SeqCst)
}

/// What a transition calls for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ping {
    /// A run finished.
    Done,
    /// An agent is waiting on an answer.
    Request,
    /// A run failed, or the connection went away.
    Attention,
}

/// Notification baseline is separate from the visual activity indicator:
/// going idle can mean cancellation, expiry, or an internal handoff.
#[derive(Debug, Clone)]
pub struct SessionNotificationState {
    indicator: Indicator,
    last_completed_turn: Option<String>,
    fresh: bool,
}

impl SessionNotificationState {
    pub fn new(session: &Session, now: chrono::DateTime<chrono::Utc>) -> Self {
        Self {
            indicator: effective_indicator(Some(session), now),
            last_completed_turn: session.last_completed_turn.clone(),
            fresh: now
                .signed_duration_since(session.updated_at)
                .num_milliseconds()
                <= zeron_proto::view::SESSION_STALE_MS,
        }
    }

    /// Call after saving the new baseline, including when delivery is pending
    /// or outputs are disabled. Suppressed pings must never be replayed later.
    pub fn ping_since(&self, prev: &Self, send_pending: bool) -> Option<Ping> {
        if self.indicator == Indicator::Errored && prev.indicator != Indicator::Errored {
            return Some(Ping::Attention);
        }
        if self.indicator == Indicator::AwaitingInput && prev.indicator != Indicator::AwaitingInput
        {
            return Some(Ping::Request);
        }
        if !send_pending
            && self.fresh
            && self.last_completed_turn.is_some()
            && self.last_completed_turn != prev.last_completed_turn
        {
            return Some(Ping::Done);
        }
        None
    }
}

/// The engine already holds raw transport degradation for four seconds before
/// exposing `Offline` or `Reconnecting`. Notify once when that durable state is
/// first crossed; booting into an outage is seeded silently.
pub fn connectivity_ping_since(
    current: ConnectivityState,
    previous: ConnectivityState,
) -> Option<Ping> {
    let degraded = matches!(
        current,
        ConnectivityState::Offline | ConnectivityState::Reconnecting
    );
    let was_degraded = matches!(
        previous,
        ConnectivityState::Offline | ConnectivityState::Reconnecting
    );
    (degraded && !was_degraded).then_some(Ping::Attention)
}

/// A fresh observer can see `Connected` while the four-second degradation
/// grace is still measuring an outage that predates it. Arm alerts only
/// after a longer healthy observation, or after recovery from a boot-time
/// outage. Runtime replacement resets the observation flag.
#[derive(Debug, Default)]
pub struct ConnectivityNotificationState {
    previous: Option<ConnectivityState>,
    first_observed_at: Option<Instant>,
    armed: bool,
}

impl ConnectivityNotificationState {
    const STARTUP_QUIET: Duration = Duration::from_secs(5);

    pub fn update(
        &mut self,
        current: ConnectivityState,
        observed: bool,
        now: Instant,
    ) -> Option<Ping> {
        if !observed {
            *self = Self::default();
            return None;
        }
        let first = *self.first_observed_at.get_or_insert(now);
        let previous = self.previous.replace(current);
        if !self.armed {
            if now.duration_since(first) >= Self::STARTUP_QUIET {
                self.armed = true;
                return previous.and_then(|previous| connectivity_ping_since(current, previous));
            }
            return None;
        }
        previous.and_then(|previous| connectivity_ping_since(current, previous))
    }
}

/// Which app windows are focused, as their viewports report. A report goes
/// stale after [`Self::TTL`] — focused viewports repeat it — so a viewport
/// that died focused never holds banners back for good.
#[derive(Clone, Default)]
pub struct ViewportFocus(Arc<Mutex<HashMap<String, Instant>>>);

impl ViewportFocus {
    pub const TTL: Duration = Duration::from_secs(45);

    pub fn set(&self, viewport: &str, focused: bool) {
        self.set_at(viewport, focused, Instant::now());
    }

    fn set_at(&self, viewport: &str, focused: bool, now: Instant) {
        let mut map = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if focused {
            map.insert(viewport.to_owned(), now);
        } else {
            map.remove(viewport);
        }
    }

    /// Some app window is focused.
    pub fn any(&self) -> bool {
        self.any_at(Instant::now())
    }

    fn any_at(&self, now: Instant) -> bool {
        let mut map = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        map.retain(|_, at| now.saturating_duration_since(*at) < Self::TTL);
        !map.is_empty()
    }
}

/// The app's banner preferences, from `{data_dir}/ui-settings.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Prefs {
    enabled: bool,
    background_only: bool,
    agent_updates: bool,
}

impl Prefs {
    fn read(data_dir: &Path) -> Self {
        let value = std::fs::read(data_dir.join("ui-settings.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok());
        Self::from_value(value.as_ref())
    }

    fn from_value(value: Option<&serde_json::Value>) -> Self {
        let flag = |key: &str| {
            value
                .and_then(|v| v.get(key))
                .and_then(|v| v.as_bool())
                .unwrap_or(true)
        };
        Self {
            enabled: flag("notificationsEnabled"),
            background_only: flag("notificationsBackgroundOnly"),
            agent_updates: flag("agentUpdateNotifications"),
        }
    }
}

const VERSIONLESS: &str = "versionless";

fn versionless_key(device: &str, harness: zeron_proto::HarnessId) -> String {
    format!("{device}:{harness:?}:{VERSIONLESS}")
}

/// One banner per release: versioned releases deduplicate by version,
/// versionless availability until the agent reports current again.
fn update_key(device: &str, status: &HarnessUpdateStatus) -> Option<String> {
    if status.phase != HarnessUpdatePhase::Available {
        return None;
    }
    Some(match status.latest_version.as_deref() {
        Some(version) => format!("{device}:{:?}:{version}", status.harness),
        None => versionless_key(device, status.harness),
    })
}

type Post = Box<dyn Fn(&str, &str, Option<&str>) + Send>;

/// The decision state, separate from the watches so it can be driven by tests.
struct Notifier {
    data_dir: PathBuf,
    device: String,
    focus: ViewportFocus,
    post: Post,
    baselines: HashMap<String, SessionNotificationState>,
    connectivity: ConnectivityNotificationState,
    updates_seen: HashSet<String>,
}

impl Notifier {
    fn new(data_dir: PathBuf, device: String, focus: ViewportFocus, post: Post) -> Self {
        Self {
            data_dir,
            device,
            focus,
            post,
            baselines: HashMap::new(),
            connectivity: ConnectivityNotificationState::default(),
            updates_seen: HashSet::new(),
        }
    }

    /// Whether a banner may show now (and its preferences, read fresh).
    fn allowed(&self) -> Option<Prefs> {
        let prefs = Prefs::read(&self.data_dir);
        (prefs.enabled && !(prefs.background_only && self.focus.any())).then_some(prefs)
    }

    fn sessions(
        &mut self,
        sessions: &[Session],
        chats: &[Chat],
        now: chrono::DateTime<chrono::Utc>,
    ) {
        let mut due = Vec::new();
        for session in sessions {
            let status = SessionNotificationState::new(session, now);
            let chat = chats.iter().find(|chat| chat.id == session.chat_id);
            let prev = self
                .baselines
                .insert(session.chat_id.clone(), status.clone());
            // Side chats keep their baselines current but never ring.
            if !chat.is_some_and(Chat::is_top_level) {
                continue;
            }
            if let Some(prev) = prev
                && let Some(ping) = status.ping_since(&prev, false)
            {
                let title = chat
                    .and_then(|chat| chat.title.clone())
                    .unwrap_or_else(|| "New session".into());
                due.push((title, ping, session.chat_id.clone()));
            }
        }
        if due.is_empty() || self.allowed().is_none() {
            return;
        }
        for (title, ping, chat_id) in due {
            let body = match ping {
                Ping::Done => "Run finished",
                Ping::Request => "Waiting on your input",
                Ping::Attention => "Run failed",
            };
            tracing::info!(chat = %chat_id, body, "desktop banner");
            (self.post)(&title, body, Some(&chat_id));
        }
    }

    fn connectivity(&mut self, state: ConnectivityState, now: Instant) {
        if self.connectivity.update(state, true, now).is_none() || self.allowed().is_none() {
            return;
        }
        let body = match state {
            ConnectivityState::Offline => "Your device is offline",
            _ => "Zeron is trying to reconnect",
        };
        (self.post)("Connection unavailable", body, None);
    }

    /// New releases not bannered yet (a confirmed current/updated status
    /// re-arms versionless availability).
    fn unseen_updates(&mut self, statuses: &[HarnessUpdateStatus]) -> Vec<String> {
        for status in statuses {
            if matches!(
                status.phase,
                HarnessUpdatePhase::Current | HarnessUpdatePhase::Updated
            ) {
                self.updates_seen
                    .remove(&versionless_key(&self.device, status.harness));
            }
        }
        statuses
            .iter()
            .filter_map(|status| update_key(&self.device, status))
            .filter(|key| !self.updates_seen.contains(key))
            .collect()
    }

    /// Release discoveries finish independently, so the caller debounces and
    /// this delivers one aggregate banner for everything new.
    fn updates(&mut self, statuses: &[HarnessUpdateStatus]) {
        let new = self.unseen_updates(statuses);
        if new.is_empty() {
            return;
        }
        let count = new.len();
        self.updates_seen.extend(new);
        if !self.allowed().is_some_and(|prefs| prefs.agent_updates) {
            return;
        }
        let body = if count == 1 {
            "A coding agent update is ready"
        } else {
            "Coding agent updates are ready"
        };
        (self.post)(
            &format!(
                "{count} agent update{} available",
                if count == 1 { "" } else { "s" }
            ),
            body,
            Some(zeron_notify::AGENT_UPDATES_TARGET),
        );
    }
}

/// Watch this engine's sessions, connectivity and agent updates and post
/// banners for them until the runtime shuts down.
pub(crate) fn spawn(core: &crate::EngineCore, data_dir: PathBuf) -> tokio::task::JoinHandle<()> {
    let mut sessions = core
        .workspace
        .merged_sessions_watch(core.sessions.watch_sessions());
    let mut chats = core.workspace.watch_chats();
    let mut connectivity = core.doc_host.watch_connectivity();
    let mut updates = core.harness_updates.watch();
    let mut notifier = Notifier::new(
        data_dir,
        core.device_id.clone(),
        core.viewport_focus.clone(),
        Box::new(zeron_notify::post),
    );
    tokio::spawn(async move {
        const UPDATE_DEBOUNCE: Duration = Duration::from_secs(1);
        // Seed every baseline silently.
        notifier.sessions(
            &sessions.borrow_and_update(),
            &chats.borrow_and_update(),
            chrono::Utc::now(),
        );
        notifier.connectivity(connectivity.borrow_and_update().state, Instant::now());
        let mut updates_due: Option<tokio::time::Instant> =
            Some(tokio::time::Instant::now() + UPDATE_DEBOUNCE);
        loop {
            tokio::select! {
                changed = sessions.changed() => {
                    if changed.is_err() { break; }
                    let sessions = sessions.borrow_and_update().clone();
                    notifier.sessions(&sessions, &chats.borrow(), chrono::Utc::now());
                }
                changed = chats.changed() => {
                    if changed.is_err() { break; }
                    let chats = chats.borrow_and_update().clone();
                    notifier.sessions(&sessions.borrow(), &chats, chrono::Utc::now());
                }
                changed = connectivity.changed() => {
                    if changed.is_err() { break; }
                    let state = connectivity.borrow_and_update().state;
                    notifier.connectivity(state, Instant::now());
                }
                changed = updates.changed() => {
                    if changed.is_err() { break; }
                    updates.borrow_and_update();
                    updates_due.get_or_insert(tokio::time::Instant::now() + UPDATE_DEBOUNCE);
                }
                _ = async {
                    match updates_due {
                        Some(at) => tokio::time::sleep_until(at).await,
                        None => std::future::pending().await,
                    }
                } => {
                    updates_due = None;
                    let statuses = updates.borrow().clone();
                    notifier.updates(&statuses);
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn baseline(indicator: Indicator, turn: Option<&str>) -> SessionNotificationState {
        SessionNotificationState {
            indicator,
            last_completed_turn: turn.map(str::to_owned),
            fresh: true,
        }
    }

    #[test]
    fn interrupted_and_expired_activity_never_ping() {
        let working = baseline(Indicator::Working, Some("old"));
        let idle = baseline(Indicator::None, Some("old"));
        assert_eq!(idle.ping_since(&working, false), None);
        // An older host without explicit completion metadata is silent too.
        assert_eq!(
            baseline(Indicator::None, None).ping_since(&baseline(Indicator::Working, None), false),
            None
        );
    }

    #[test]
    fn a_run_error_pings_once_and_never_masquerades_as_completion() {
        let working = baseline(Indicator::Working, Some("old"));
        let errored = baseline(Indicator::Errored, Some("failed"));
        assert_eq!(errored.ping_since(&working, false), Some(Ping::Attention));
        assert_eq!(errored.ping_since(&errored, false), None);
    }

    #[test]
    fn durable_connectivity_degradation_pings_once_per_outage() {
        use zeron_proto::ConnectivityState as State;

        assert_eq!(
            connectivity_ping_since(State::Connected, State::Disabled),
            None
        );
        assert_eq!(
            connectivity_ping_since(State::Reconnecting, State::Connected),
            Some(Ping::Attention)
        );
        assert_eq!(
            connectivity_ping_since(State::Offline, State::Reconnecting),
            None
        );
        assert_eq!(
            connectivity_ping_since(State::Connected, State::Offline),
            None
        );
        assert_eq!(
            connectivity_ping_since(State::Offline, State::Connected),
            Some(Ping::Attention)
        );
    }

    #[test]
    fn ordinary_queue_completions_survive_coalesced_working_states() {
        let first = baseline(Indicator::Working, None);
        let second = baseline(Indicator::Working, Some("first"));
        assert_eq!(second.ping_since(&first, false), Some(Ping::Done));
        assert_eq!(second.ping_since(&second, false), None);
        let last = baseline(Indicator::None, Some("second"));
        assert_eq!(last.ping_since(&second, false), Some(Ping::Done));
        assert_eq!(last.ping_since(&last, false), None);
    }

    #[test]
    fn pending_send_consumes_completion_but_preserves_input_requests() {
        let before = baseline(Indicator::Working, None);
        let settled = baseline(Indicator::None, Some("first"));
        assert_eq!(settled.ping_since(&before, true), None);
        assert_eq!(settled.ping_since(&settled, false), None);
        let question = baseline(Indicator::AwaitingInput, Some("first"));
        assert_eq!(question.ping_since(&settled, true), Some(Ping::Request));
        assert_eq!(question.ping_since(&question, false), None);
    }

    #[test]
    fn stale_completion_is_consumed_without_replaying_on_a_heartbeat() {
        let before = baseline(Indicator::Working, None);
        let mut stale = baseline(Indicator::None, Some("old"));
        stale.fresh = false;
        assert_eq!(stale.ping_since(&before, false), None);
        let refreshed = baseline(Indicator::Working, Some("old"));
        assert_eq!(refreshed.ping_since(&stale, false), None);
    }

    #[test]
    fn connectivity_boot_outages_seed_silently_then_later_outages_alert() {
        use zeron_proto::ConnectivityState as State;
        let t0 = Instant::now();

        // Warm daemon: the first authoritative snapshot is already degraded.
        let mut warm = ConnectivityNotificationState::default();
        assert_eq!(warm.update(State::Disabled, false, t0), None);
        assert_eq!(warm.update(State::Offline, true, t0), None);
        assert_eq!(
            warm.update(State::Connected, true, t0 + Duration::from_secs(6)),
            None
        );
        assert_eq!(
            warm.update(State::Offline, true, t0 + Duration::from_secs(7)),
            Some(Ping::Attention)
        );

        // Cold daemon: the engine's grace initially masks the existing outage.
        let mut cold = ConnectivityNotificationState::default();
        assert_eq!(cold.update(State::Connected, true, t0), None);
        assert_eq!(
            cold.update(State::Offline, true, t0 + Duration::from_secs(4)),
            None
        );
        assert_eq!(
            cold.update(State::Connected, true, t0 + Duration::from_secs(5)),
            None
        );
        assert_eq!(
            cold.update(State::Reconnecting, true, t0 + Duration::from_secs(6)),
            Some(Ping::Attention)
        );

        // A quiet healthy boot may not publish another frame until the first
        // genuine outage; elapsed time arms that transition itself.
        let mut healthy = ConnectivityNotificationState::default();
        assert_eq!(healthy.update(State::Connected, true, t0), None);
        assert_eq!(
            healthy.update(State::Offline, true, t0 + Duration::from_secs(6)),
            Some(Ping::Attention)
        );

        // Replacing the runtime returns to bootstrap semantics even if the
        // prior runtime had already armed alerts.
        assert_eq!(
            healthy.update(State::Disabled, false, t0 + Duration::from_secs(7)),
            None
        );
        assert_eq!(
            healthy.update(State::Offline, true, t0 + Duration::from_secs(7)),
            None
        );
    }

    #[test]
    fn focus_reports_expire_and_clear() {
        let focus = ViewportFocus::default();
        let now = Instant::now();
        assert!(!focus.any_at(now));
        focus.set_at("window", true, now);
        assert!(focus.any_at(now + Duration::from_secs(10)));
        // A viewport that died focused stops holding banners back.
        assert!(!focus.any_at(now + ViewportFocus::TTL + Duration::from_secs(1)));
        focus.set_at("window", true, now);
        focus.set_at("window", false, now);
        assert!(!focus.any_at(now));
    }

    #[test]
    fn preferences_default_on_and_read_the_app_settings() {
        assert_eq!(
            Prefs::from_value(None),
            Prefs {
                enabled: true,
                background_only: true,
                agent_updates: true
            }
        );
        let value = serde_json::json!({
            "notificationsEnabled": false,
            "notificationsBackgroundOnly": false,
            "theme": "dark",
        });
        assert_eq!(
            Prefs::from_value(Some(&value)),
            Prefs {
                enabled: false,
                background_only: false,
                agent_updates: true
            }
        );
    }

    type Posted = Arc<Mutex<Vec<(String, String, Option<String>)>>>;

    fn notifier(dir: &Path) -> (Notifier, Posted, ViewportFocus) {
        let posted: Posted = Arc::default();
        let sink = posted.clone();
        let focus = ViewportFocus::default();
        let notifier = Notifier::new(
            dir.to_path_buf(),
            "mac".into(),
            focus.clone(),
            Box::new(move |title, body, target| {
                sink.lock()
                    .unwrap()
                    .push((title.into(), body.into(), target.map(str::to_owned)));
            }),
        );
        (notifier, posted, focus)
    }

    fn session(chat_id: &str, turn: Option<&str>, now: chrono::DateTime<chrono::Utc>) -> Session {
        serde_json::from_value(serde_json::json!({
            "chatId": chat_id,
            "deviceId": "mac",
            "status": "idle",
            "updatedAt": now,
            "lastCompletedTurn": turn,
        }))
        .expect("session fixture")
    }

    fn chat(id: &str, title: &str, parent: Option<&str>) -> Chat {
        let mut chat: Chat = serde_json::from_value(serde_json::json!({
            "id": id,
            "title": title,
            "deviceId": "mac",
            "archived": false,
            "cwd": null,
            "branch": null,
            "checkoutId": null,
            "config": null,
            "lastMessagePreview": null,
            "lastMessageAt": null,
            "createdAt": chrono::Utc::now(),
        }))
        .expect("chat fixture");
        chat.parent_chat_id = parent.map(str::to_owned);
        chat
    }

    #[test]
    fn finished_runs_banner_top_level_chats_unless_focused_or_off() {
        let dir = tempfile::tempdir().unwrap();
        let (mut notifier, posted, focus) = notifier(dir.path());
        let now = chrono::Utc::now();
        let chats = [
            chat("a", "Fix the build", None),
            chat("side", "Side", Some("a")),
        ];
        // First sight seeds silently.
        notifier.sessions(
            &[
                session("a", Some("t1"), now),
                session("side", Some("s1"), now),
            ],
            &chats,
            now,
        );
        assert!(posted.lock().unwrap().is_empty());
        // A finished run on the top-level chat banners; the side chat doesn't.
        notifier.sessions(
            &[
                session("a", Some("t2"), now),
                session("side", Some("s2"), now),
            ],
            &chats,
            now,
        );
        assert_eq!(
            *posted.lock().unwrap(),
            vec![(
                "Fix the build".into(),
                "Run finished".into(),
                Some("a".into())
            )]
        );
        // A focused window holds it back (background-only is the default).
        focus.set("window", true);
        notifier.sessions(&[session("a", Some("t3"), now)], &chats, now);
        assert_eq!(posted.lock().unwrap().len(), 1);
        // Banners off: nothing, focused or not.
        focus.set("window", false);
        std::fs::write(
            dir.path().join("ui-settings.json"),
            r#"{"notificationsEnabled": false}"#,
        )
        .unwrap();
        notifier.sessions(&[session("a", Some("t4"), now)], &chats, now);
        assert_eq!(posted.lock().unwrap().len(), 1);
    }

    #[test]
    fn agent_updates_banner_once_per_release() {
        let dir = tempfile::tempdir().unwrap();
        let (mut notifier, posted, _) = notifier(dir.path());
        let status = |phase: &str, version: Option<&str>| -> HarnessUpdateStatus {
            serde_json::from_value(serde_json::json!({
                "harness": "codex",
                "phase": phase,
                "latestVersion": version,
            }))
            .expect("update fixture")
        };
        notifier.updates(&[status("available", Some("1.2"))]);
        notifier.updates(&[status("available", Some("1.2"))]);
        assert_eq!(posted.lock().unwrap().len(), 1);
        assert_eq!(
            posted.lock().unwrap()[0].2.as_deref(),
            Some(zeron_notify::AGENT_UPDATES_TARGET)
        );
        // Versionless availability re-arms after the agent reports current.
        notifier.updates(&[status("available", None)]);
        notifier.updates(&[status("current", None)]);
        notifier.updates(&[status("available", None)]);
        assert_eq!(posted.lock().unwrap().len(), 3);
    }
}
