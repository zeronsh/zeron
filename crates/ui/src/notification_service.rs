//! Application-wide alert detection. Window creation and navigation never
//! install another detector or replay a conversation's previous completion.

use std::{collections::HashMap, time::Instant};

use chrono::{DateTime, Utc};
use gpui::{App, AppContext, Entity, Global, Subscription};

use crate::{
    settings::UiSettings,
    sound::{AttentionSoundGate, ConnectivityNotificationState, SessionNotificationState, Sound},
    state::AppState,
};

struct Service(Entity<NotificationService>);
impl Global for Service {}

pub(crate) struct NotificationService {
    detector: Detector,
    _subscription: Subscription,
    // A send must be acknowledged even if its last viewing window closes.
    // These leases end as soon as the engine transcript confirms the message.
    pending: HashMap<String, Entity<AppState>>,
    #[cfg(test)]
    emitted: Vec<Notice>,
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
                let audio = settings.session_sound_enabled(sound)
                    && (sound != Sound::Attention || self.attention.should_play(instant));
                notices.push(Notice {
                    chat_id: Some(session.chat_id.clone()),
                    title: state
                        .chats
                        .iter()
                        .find(|chat| chat.id == session.chat_id)
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
        pending: Default::default(),
        #[cfg(test)]
        emitted: Vec::new(),
        _subscription: cx.observe(&owner, |this: &mut NotificationService, owner, cx| {
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
}
