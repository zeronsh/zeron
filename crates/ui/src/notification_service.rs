//! Application-wide alert detection. Window creation and navigation never
//! install another detector or replay a conversation's previous completion.

use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

use chrono::{DateTime, Utc};
use gpui::{App, AppContext, Context, Entity, Global, Subscription, Task};

use crate::{
    settings::UiSettings,
    sound::{AttentionSoundGate, ConnectivityNotificationState, SessionNotificationState, Sound},
    state::AppState,
};

struct Service(Entity<NotificationService>);
impl Global for Service {}

pub(crate) struct NotificationService {
    detector: Detector,
    harness_updates: HarnessUpdateDetector,
    harness_update_task: Option<Task<()>>,
    _subscription: Subscription,
    // A send must be acknowledged even if its last viewing window closes.
    // These leases end as soon as the engine transcript confirms the message.
    pending: HashMap<String, Entity<AppState>>,
    #[cfg(test)]
    emitted: Vec<Notice>,
    #[cfg(test)]
    emitted_harness_updates: Vec<(usize, bool)>,
}

#[derive(Default)]
struct HarnessUpdateDetector {
    seen: HashSet<String>,
}

impl HarnessUpdateDetector {
    fn versionless_key(device: &str, harness: zeron_proto::HarnessId) -> String {
        format!("{device}:{harness:?}:versionless")
    }

    fn key(device: &str, status: &zeron_proto::HarnessUpdateStatus) -> Option<String> {
        if status.phase != zeron_proto::HarnessUpdatePhase::Available {
            return None;
        }
        Some(match status.latest_version.as_deref() {
            Some(version) => format!("{device}:{:?}:{version}", status.harness),
            None => Self::versionless_key(device, status.harness),
        })
    }

    fn has_unseen(&mut self, state: &AppState) -> bool {
        let device = state.local_device_id.as_deref().unwrap_or("local");
        for status in &state.harness_updates {
            if matches!(
                status.phase,
                zeron_proto::HarnessUpdatePhase::Current | zeron_proto::HarnessUpdatePhase::Updated
            ) {
                self.seen
                    .remove(&Self::versionless_key(device, status.harness));
            }
        }
        state
            .harness_updates
            .iter()
            .any(|status| Self::key(device, status).is_some_and(|key| !self.seen.contains(&key)))
    }

    fn take_unseen(&mut self, state: &AppState) -> usize {
        let device = state.local_device_id.as_deref().unwrap_or("local");
        let new: HashSet<_> = state
            .harness_updates
            .iter()
            .filter_map(|status| Self::key(device, status))
            .filter(|key| !self.seen.contains(key))
            .collect();
        let count = new.len();
        self.seen.extend(new);
        count
    }
}

impl NotificationService {
    fn queue_harness_update_notice(&mut self, owner: &Entity<AppState>, cx: &mut Context<Self>) {
        let has_unseen = self.harness_updates.has_unseen(owner.read(cx));
        if self.harness_update_task.is_some() || !has_unseen {
            return;
        }
        let owner = owner.clone();
        self.harness_update_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            this.update(cx, |this, cx| {
                this.harness_update_task = None;
                let count = this.harness_updates.take_unseen(owner.read(cx));
                if count == 0 {
                    return;
                }
                let settings = crate::settings::current(cx);
                let banner = settings.notifications_enabled
                    && settings.agent_update_notifications
                    && !(settings.notifications_background_only && cx.active_window().is_some());
                #[cfg(not(test))]
                if banner {
                    let body = if count == 1 {
                        "A coding agent update is ready"
                    } else {
                        "Coding agent updates are ready"
                    };
                    crate::notify::post(
                        &format!(
                            "{count} agent update{} available",
                            if count == 1 { "" } else { "s" }
                        ),
                        body,
                        Some(crate::notify::AGENT_UPDATES_TARGET),
                    );
                }
                #[cfg(test)]
                this.emitted_harness_updates.push((count, banner));
            })
            .ok();
        }));
    }
}

#[derive(Default)]
struct Detector {
    sessions: HashMap<String, SessionNotificationState>,
    connectivity: ConnectivityNotificationState,
    attention: AttentionSoundGate,
    epoch: u64,
}

#[derive(Debug)]
struct Notice {
    chat_id: Option<String>,
    title: String,
    body: &'static str,
    sound: Sound,
    audio: bool,
    banner: bool,
}

impl Detector {
    fn collect(
        &mut self,
        state: &AppState,
        settings: &UiSettings,
        focused: bool,
        now: DateTime<Utc>,
        instant: Instant,
    ) -> Vec<Notice> {
        if self.epoch != state.runtime_epoch {
            *self = Self {
                epoch: state.runtime_epoch,
                ..Self::default()
            };
        }
        if !matches!(state.connection, crate::state::ConnectionStatus::Ready) {
            self.sessions.clear();
            self.connectivity = Default::default();
            return Vec::new();
        }
        let banner =
            settings.notifications_enabled && !(settings.notifications_background_only && focused);
        let mut notices = Vec::new();
        self.sessions.retain(|chat, _| {
            state
                .sessions
                .iter()
                .any(|session| &session.chat_id == chat)
        });
        for session in &state.sessions {
            let status = SessionNotificationState::new(session, now);
            let previous = self
                .sessions
                .insert(session.chat_id.clone(), status.clone());
            if let Some(sound) = previous.and_then(|previous| {
                status.sound_since(&previous, state.send_pending(&session.chat_id, now))
            }) {
                // Child chats and voice orchestrators have their own surfaces.
                // Keep their baselines without separate session notifications.
                let chat = state.chats.iter().find(|chat| chat.id == session.chat_id);
                if !chat.is_some_and(|chat| chat.is_top_level()) {
                    continue;
                }
                let audio = settings.session_sound_enabled(sound)
                    && (sound != Sound::Attention || self.attention.should_play(instant));
                notices.push(Notice {
                    chat_id: Some(session.chat_id.clone()),
                    title: chat
                        .and_then(|chat| chat.title.clone())
                        .unwrap_or_else(|| "New session".into()),
                    body: match sound {
                        Sound::Done => "Run finished",
                        Sound::Request => "Waiting on your input",
                        Sound::Attention => "Run failed",
                    },
                    sound,
                    audio,
                    banner,
                });
            }
        }
        if let Some(sound) = self.connectivity.update(
            state.connectivity.state,
            state.connectivity_observed,
            instant,
        ) {
            notices.push(Notice {
                chat_id: None,
                title: "Connection unavailable".into(),
                body: if state.connectivity.state == zeron_proto::ConnectivityState::Offline {
                    "Your device is offline"
                } else {
                    "Zeron is trying to reconnect"
                },
                sound,
                audio: settings.session_sound_enabled(sound) && self.attention.should_play(instant),
                banner,
            });
        }
        notices
    }
}

pub(crate) fn init(owner: Entity<AppState>, cx: &mut App) -> Entity<NotificationService> {
    if let Some(service) = cx.try_global::<Service>() {
        return service.0.clone();
    }
    let service = cx.new(|cx| NotificationService {
        detector: Default::default(),
        harness_updates: Default::default(),
        harness_update_task: None,
        pending: Default::default(),
        #[cfg(test)]
        emitted: Vec::new(),
        #[cfg(test)]
        emitted_harness_updates: Vec::new(),
        _subscription: cx.observe(&owner, |this: &mut NotificationService, owner, cx| {
            this.queue_harness_update_notice(&owner, cx);
            let (ids, engine) = {
                let state = owner.read(cx);
                (state.pending_chat_ids(), state.engine().cloned())
            };
            this.pending.retain(|id, _| ids.contains(id));
            for id in ids {
                if !this.pending.contains_key(&id) {
                    let source = crate::chat_store::acquire(id.clone(), engine.clone(), cx);
                    this.pending.insert(id, source);
                }
            }
            let settings = crate::settings::current(cx);
            let focused = cx.active_window().is_some();
            let notices = this.detector.collect(
                owner.read(cx),
                &settings,
                focused,
                Utc::now(),
                Instant::now(),
            );
            for notice in notices {
                #[cfg(not(test))]
                {
                    if notice.audio {
                        crate::sound::play(notice.sound);
                    }
                    if notice.banner {
                        crate::notify::post(&notice.title, notice.body, notice.chat_id.as_deref());
                    }
                }
                #[cfg(test)]
                this.emitted.push(notice);
            }
        }),
    });
    cx.set_global(Service(service.clone()));
    service
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> zeron_proto::Session {
        zeron_proto::Session {
            chat_id: "shared".into(),
            device_id: "local".into(),
            status: zeron_proto::SessionStatus::Working,
            started_at: Some(Utc::now()),
            updated_at: Utc::now(),
            last_completed_turn: None,
            running_subagents: 0,
        }
    }

    fn chat(parent_chat_id: Option<&str>) -> zeron_proto::Chat {
        zeron_proto::Chat {
            id: "shared".into(),
            device_id: "local".into(),
            title: None,
            archived: false,
            cwd: None,
            branch: None,
            checkout_id: None,
            source_context: None,
            config: None,
            last_message_preview: None,
            last_message_at: None,
            created_at: Utc::now(),
            harness_session_id: None,
            harness_session_cwd: None,
            space_id: None,
            last_seen_at: None,
            room_gen: None,
            parent_chat_id: parent_chat_id.map(str::to_owned),
        }
    }

    #[gpui::test]
    fn three_views_and_repeated_installation_emit_one_completion(cx: &mut gpui::TestAppContext) {
        let (owner, service, views) = cx.update(|cx| {
            let owner = cx.new(|_| AppState::new());
            let service = init(owner.clone(), cx);
            let views = (0..3)
                .map(|_| {
                    assert_eq!(init(owner.clone(), cx), service);
                    cx.new(|cx| AppState::for_window(owner.clone(), cx))
                })
                .collect::<Vec<_>>();
            owner.update(cx, |state, cx| {
                state.connection = crate::state::ConnectionStatus::Ready;
                state.chats = vec![chat(None)];
                state.apply_sessions(vec![session()]);
                cx.notify();
            });
            (owner, service, views)
        });
        cx.update(|cx| {
            assert!(service.read(cx).emitted.is_empty());
            owner.update(cx, |state, cx| {
                state.sessions[0].last_completed_turn = Some("turn-1".into());
                cx.notify();
            });
        });
        cx.update(|cx| {
            let emitted = &service.read(cx).emitted;
            assert_eq!(emitted.len(), 1);
            assert_eq!(emitted[0].sound, Sound::Done);
            assert!(emitted[0].audio && emitted[0].banner);
            assert_eq!(emitted[0].chat_id.as_deref(), Some("shared"));
            assert_eq!(emitted[0].body, "Run finished");
            assert_eq!(emitted[0].title, "New session");
            drop(views);
            owner.update(cx, |_, cx| cx.notify());
        });
        cx.update(|cx| assert_eq!(service.read(cx).emitted.len(), 1));
    }

    #[test]
    fn focus_preferences_and_profile_replacement_do_not_replay_alerts() {
        let mut state = AppState::new();
        state.connection = crate::state::ConnectionStatus::Ready;
        state.chats = vec![chat(None)];
        state.sessions = vec![session()];
        let mut detector = Detector::default();
        let settings = UiSettings::default();
        let now = Utc::now();
        let instant = Instant::now();
        assert!(
            detector
                .collect(&state, &settings, true, now, instant)
                .is_empty()
        );
        state.sessions[0].last_completed_turn = Some("first".into());
        let notices = detector.collect(&state, &settings, true, now, instant);
        assert_eq!(notices.len(), 1);
        assert!(notices[0].audio);
        assert!(!notices[0].banner);
        assert!(
            detector
                .collect(&state, &settings, false, now, instant)
                .is_empty()
        );
        state.runtime_epoch += 1;
        assert!(
            detector
                .collect(&state, &settings, false, now, instant)
                .is_empty()
        );
    }

    #[test]
    fn child_chat_completion_does_not_emit_a_separate_notification() {
        let mut state = AppState::new();
        state.connection = crate::state::ConnectionStatus::Ready;
        state.chats = vec![chat(Some("parent"))];
        state.sessions = vec![session()];
        let mut detector = Detector::default();
        let settings = UiSettings::default();
        let now = Utc::now();
        let instant = Instant::now();
        assert!(
            detector
                .collect(&state, &settings, false, now, instant)
                .is_empty()
        );
        state.sessions[0].last_completed_turn = Some("turn-1".into());
        assert!(
            detector
                .collect(&state, &settings, false, now, instant)
                .is_empty()
        );
        state.chats[0].parent_chat_id = None;
        assert!(
            detector
                .collect(&state, &settings, false, now, instant)
                .is_empty()
        );
    }

    #[test]
    fn voice_orchestrator_completion_does_not_emit_a_session_notification() {
        let mut state = AppState::new();
        state.connection = crate::state::ConnectionStatus::Ready;
        let id = format!("{}shared", zeron_proto::voice::ORCHESTRATOR_CHAT_PREFIX);
        let mut voice_chat = chat(None);
        voice_chat.id = id.clone();
        let mut voice_session = session();
        voice_session.chat_id = id.clone();
        state.chats = vec![voice_chat];
        state.sessions = vec![voice_session];
        let mut detector = Detector::default();
        let settings = UiSettings::default();
        let now = Utc::now();
        let instant = Instant::now();
        assert!(
            detector
                .collect(&state, &settings, false, now, instant)
                .is_empty()
        );
        state.sessions[0].last_completed_turn = Some("voice-turn-1".into());
        assert!(
            detector
                .collect(&state, &settings, false, now, instant)
                .is_empty()
        );
        assert!(detector.sessions.contains_key(&id));
    }

    #[gpui::test]
    fn agent_update_banner_is_debounced_once_across_windows(cx: &mut gpui::TestAppContext) {
        let (owner, service, views) = cx.update(|cx| {
            let owner = cx.new(|_| AppState::new());
            let service = init(owner.clone(), cx);
            let views = (0..3)
                .map(|_| cx.new(|cx| AppState::for_window(owner.clone(), cx)))
                .collect::<Vec<_>>();
            owner.update(cx, |state, cx| {
                state.connection = crate::state::ConnectionStatus::Ready;
                state.local_device_id = Some("local".into());
                state.harness_updates = vec![
                    serde_json::from_value(serde_json::json!({
                        "harness": "codex", "phase": "available", "latestVersion": "2.0.0",
                        "policy": "notify", "source": "unknown", "canApply": true
                    }))
                    .unwrap(),
                    serde_json::from_value(serde_json::json!({
                        "harness": "hermes", "phase": "available",
                        "policy": "notify", "source": "unknown", "canApply": true
                    }))
                    .unwrap(),
                ];
                cx.notify();
            });
            (owner, service, views)
        });
        cx.run_until_parked();
        cx.update(|cx| {
            assert!(
                views
                    .iter()
                    .all(|view| view.read(cx).harness_updates.len() == 2)
            );
        });
        cx.executor().advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
        cx.update(|cx| {
            assert_eq!(service.read(cx).emitted_harness_updates.len(), 1);
            assert_eq!(service.read(cx).emitted_harness_updates[0].0, 2);
            assert_eq!(service.read(cx).harness_updates.seen.len(), 2);
        });
        // A versionless release can notify again only after Current or Updated.
        for phase in [
            zeron_proto::HarnessUpdatePhase::Checking,
            zeron_proto::HarnessUpdatePhase::Available,
        ] {
            owner.update(cx, |state, cx| {
                state.harness_updates[1].phase = phase;
                cx.notify();
            });
        }
        cx.run_until_parked();
        cx.update(|cx| assert!(service.read(cx).harness_update_task.is_none()));
        owner.update(cx, |state, cx| {
            state.harness_updates[1].phase = zeron_proto::HarnessUpdatePhase::Current;
            cx.notify();
        });
        cx.update(|cx| assert_eq!(service.read(cx).harness_updates.seen.len(), 1));
        owner.update(cx, |state, cx| {
            state.harness_updates[1].phase = zeron_proto::HarnessUpdatePhase::Available;
            cx.notify();
        });
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
        cx.update(|cx| {
            assert_eq!(service.read(cx).emitted_harness_updates.len(), 2);
            assert_eq!(service.read(cx).emitted_harness_updates[1].0, 1);
        });
    }
}
