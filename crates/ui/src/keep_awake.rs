//! Device-local sleep prevention, owned by the app rather than a window.
//! OS calls run on one dedicated thread: D-Bus must not block the UI, and
//! Windows execution-state assertions must be released on their owning thread.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::Duration;

use chrono::{DateTime, Utc};
use gpui::{App, AppContext, Context, Entity, Global, Subscription, Task};
use serde::{Deserialize, Serialize};
use zeron_proto::{
    Session,
    view::{Indicator, effective_indicator},
};

use crate::{settings, state::AppState};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum KeepAwakeMode {
    #[default]
    WhilePromptRunning,
    WhileAppOpen,
    Off,
}

impl KeepAwakeMode {
    pub const ALL: [Self; 3] = [Self::WhilePromptRunning, Self::WhileAppOpen, Self::Off];

    pub const fn label(self) -> &'static str {
        match self {
            Self::WhilePromptRunning => "While a prompt is running",
            Self::WhileAppOpen => "While Zeron is open",
            Self::Off => "Off",
        }
    }

    fn wants_awake(self, sessions: &[Session], now: DateTime<Utc>) -> bool {
        match self {
            Self::Off => false,
            Self::WhileAppOpen => true,
            Self::WhilePromptRunning => sessions.iter().any(|session| {
                matches!(
                    effective_indicator(Some(session), now),
                    Indicator::Working | Indicator::AwaitingInput
                )
            }),
        }
    }
}

struct AwakeController {
    state: Entity<AppState>,
    worker: Option<Sender<bool>>,
    desired: bool,
    _subscriptions: Vec<Subscription>,
    _timer: Task<()>,
}

struct AwakeService(Entity<AwakeController>);
impl Global for AwakeService {}

pub(crate) fn init(state: Entity<AppState>, cx: &mut App) {
    let (tx, rx) = mpsc::channel();
    let worker = std::thread::Builder::new()
        .name("zeron-keep-awake".into())
        .spawn(move || run_worker(rx, platform::acquire));
    if let Err(error) = worker {
        tracing::warn!(%error, "could not start sleep prevention worker");
        return;
    }
    let controller = cx.new(|cx| AwakeController::new(state, tx, cx));
    cx.set_global(AwakeService(controller));
}

pub(crate) fn shutdown(cx: &mut App) {
    if let Some(service) = cx.try_global::<AwakeService>() {
        let controller = service.0.clone();
        controller.update(cx, |this, _| {
            // Closing the channel releases the guard and stops the worker.
            this.worker.take();
        });
    }
}

impl AwakeController {
    fn new(state: Entity<AppState>, tx: Sender<bool>, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![
            cx.observe(&state, |this, _, cx| this.reconcile(cx)),
            cx.observe_global::<settings::SettingsStore>(|this, cx| this.reconcile(cx)),
        ];
        let timer = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(5)).await;
                if this.update(cx, |this, cx| this.reconcile(cx)).is_err() {
                    break;
                }
            }
        });
        let mut controller = AwakeController {
            state,
            worker: Some(tx),
            desired: false,
            _subscriptions: subscriptions,
            _timer: timer,
        };
        controller.reconcile(cx);
        controller
    }

    fn reconcile(&mut self, cx: &mut Context<Self>) {
        let desired = settings::current(cx)
            .keep_awake
            .wants_awake(&self.state.read(cx).sessions, Utc::now());
        if desired != self.desired {
            self.desired = desired;
            if let Some(worker) = &self.worker {
                let _ = worker.send(desired);
            }
        }
    }
}

fn run_worker<G>(rx: Receiver<bool>, mut acquire: impl FnMut() -> anyhow::Result<G>) {
    let mut guard = None;
    let mut desired = false;
    loop {
        let command = if desired && guard.is_none() {
            rx.recv_timeout(Duration::from_secs(30))
        } else {
            rx.recv().map_err(|_| RecvTimeoutError::Disconnected)
        };
        match command {
            Ok(value) => desired = value,
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        // A newer choice may have arrived while an OS call was in progress.
        for value in rx.try_iter() {
            desired = value;
        }
        if !desired {
            guard.take();
        } else if guard.is_none() {
            match acquire() {
                Ok(value) => guard = Some(value),
                Err(error) => {
                    tracing::warn!(%error, "sleep prevention unavailable; retrying in 30s")
                }
            }
        }
    }
}

mod platform {
    const REASON: &str = "Zeron is keeping this computer awake";

    #[cfg(not(target_os = "linux"))]
    pub fn acquire() -> anyhow::Result<keepawake::KeepAwake> {
        Ok(keepawake::Builder::default()
            .idle(true)
            .display(true)
            .app_name("Zeron")
            .app_reverse_domain("sh.zeron.desktop")
            .reason(REASON)
            .create()?)
    }

    // Prefer the desktop portal for both Wayland and X11. Fall back to
    // logind on desktops without an Inhibit portal; no shell tools required.
    #[cfg(target_os = "linux")]
    pub enum Guard {
        Portal(ashpd::desktop::Request<()>),
        Logind(keepawake::KeepAwake),
    }

    #[cfg(target_os = "linux")]
    pub fn acquire() -> anyhow::Result<Guard> {
        use ashpd::desktop::inhibit::{InhibitFlags, InhibitOptions, InhibitProxy};
        let portal = async_io::block_on(async {
            let proxy = InhibitProxy::new().await?;
            let request = proxy
                .inhibit(
                    None,
                    InhibitFlags::Idle.into(),
                    InhibitOptions::default().set_reason(REASON),
                )
                .await?;
            request.response()?;
            Ok::<_, ashpd::Error>(request)
        });
        match portal {
            Ok(request) => Ok(Guard::Portal(request)),
            Err(error) => {
                tracing::debug!(%error, "sleep prevention portal unavailable; trying logind");
                Ok(Guard::Logind(
                    keepawake::Builder::default()
                        .idle(true)
                        .app_name("Zeron")
                        .reason(REASON)
                        .create()?,
                ))
            }
        }
    }

    #[cfg(target_os = "linux")]
    impl Drop for Guard {
        fn drop(&mut self) {
            match self {
                Self::Portal(request) => {
                    if let Err(error) = async_io::block_on(request.close()) {
                        tracing::debug!(%error, "sleep prevention portal already closed");
                    }
                }
                Self::Logind(guard) => {
                    let _ = guard;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeron_proto::SessionStatus;

    fn session(status: SessionStatus, now: DateTime<Utc>) -> Session {
        Session {
            chat_id: "background-chat".into(),
            device_id: "device".into(),
            status,
            updated_at: now,
            started_at: Some(now),
            last_completed_turn: None,
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "queries native macOS power assertions"]
    fn native_macos_assertions_are_acquired_and_released() {
        let assertions = || {
            let output = std::process::Command::new("pmset")
                .args(["-g", "assertions"])
                .output()
                .unwrap();
            assert!(output.status.success());
            String::from_utf8(output.stdout)
                .unwrap()
                .lines()
                .filter(|line| line.contains("Zeron is keeping this computer awake"))
                .count()
        };
        let before = assertions();
        let guard = platform::acquire().unwrap();
        assert_eq!(assertions(), before + 2);
        drop(guard);
        assert_eq!(assertions(), before);
    }

    #[gpui::test]
    fn controller_observes_settings_and_background_sessions_without_a_window(
        cx: &mut gpui::TestAppContext,
    ) {
        let temp = tempfile::tempdir().unwrap();
        cx.update(|cx| settings::init(settings::UiSettings::default(), temp.path(), cx));
        let state = cx.new(|_| AppState::new());
        let (tx, rx) = mpsc::channel();
        let controller = cx.new(|cx| AwakeController::new(state.clone(), tx, cx));
        assert!(rx.try_recv().is_err());
        state.update(cx, |state, cx| {
            state.sessions = vec![session(SessionStatus::Working, Utc::now())];
            cx.notify();
        });
        cx.run_until_parked();
        assert_eq!(rx.try_recv().unwrap(), true);
        cx.update(|cx| {
            settings::update(settings::SavePolicy::Immediate, cx, |settings| {
                settings.keep_awake = KeepAwakeMode::Off
            });
        });
        cx.run_until_parked();
        assert_eq!(rx.try_recv().unwrap(), false);
        state.update(cx, |state, cx| {
            state.sessions.clear();
            cx.notify();
        });
        cx.run_until_parked();
        assert!(rx.try_recv().is_err());
        cx.update(|cx| {
            settings::update(settings::SavePolicy::Immediate, cx, |settings| {
                settings.keep_awake = KeepAwakeMode::WhileAppOpen
            });
        });
        cx.run_until_parked();
        assert_eq!(rx.try_recv().unwrap(), true);
        assert_eq!(
            settings::UiSettings::load(temp.path()).keep_awake,
            KeepAwakeMode::WhileAppOpen
        );
        cx.update(|cx| {
            cx.set_global(AwakeService(controller));
            shutdown(cx);
        });
        assert!(matches!(
            rx.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
    }

    #[test]
    fn old_settings_default_to_prompt_running_and_all_modes_round_trip() {
        assert_eq!(
            serde_json::from_str::<settings::UiSettings>("{}")
                .unwrap()
                .keep_awake,
            KeepAwakeMode::WhilePromptRunning
        );
        for mode in KeepAwakeMode::ALL {
            let mut original = settings::UiSettings::default();
            original.keep_awake = mode;
            let saved = serde_json::to_vec(&original).unwrap();
            assert_eq!(
                serde_json::from_slice::<settings::UiSettings>(&saved)
                    .unwrap()
                    .keep_awake,
                mode
            );
        }
    }

    #[test]
    fn prompt_policy_tracks_all_live_sessions_and_expires_crashed_runs() {
        let now = Utc::now();
        let mode = KeepAwakeMode::WhilePromptRunning;
        assert!(!mode.wants_awake(&[], now));
        for status in [SessionStatus::Working, SessionStatus::AwaitingInput] {
            let active = session(status, now);
            assert!(mode.wants_awake(&[session(SessionStatus::Idle, now), active.clone()], now));
            assert!(!mode.wants_awake(&[active], now + chrono::Duration::seconds(46)));
        }
        for status in [SessionStatus::Idle, SessionStatus::Errored] {
            assert!(!mode.wants_awake(&[session(status, now)], now));
        }
        assert!(KeepAwakeMode::WhileAppOpen.wants_awake(&[], now));
        assert!(!KeepAwakeMode::Off.wants_awake(&[session(SessionStatus::Working, now)], now));
    }

    #[test]
    fn worker_releases_and_reacquires_on_the_same_thread_and_on_shutdown() {
        struct Guard(Sender<(&'static str, std::thread::ThreadId)>);
        impl Drop for Guard {
            fn drop(&mut self) {
                self.0
                    .send(("released", std::thread::current().id()))
                    .unwrap();
            }
        }
        let (tx, rx) = mpsc::channel();
        let (events, results) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            run_worker(rx, || {
                events
                    .send(("acquired", std::thread::current().id()))
                    .unwrap();
                Ok(Guard(events.clone()))
            })
        });
        let next = || results.recv_timeout(Duration::from_secs(5)).unwrap();
        tx.send(true).unwrap();
        let (event, owner) = next();
        assert_eq!(event, "acquired");
        tx.send(true).unwrap(); // Does not create a second assertion.
        tx.send(false).unwrap();
        assert_eq!(next(), ("released", owner));
        tx.send(true).unwrap();
        assert_eq!(next(), ("acquired", owner));
        drop(tx);
        assert_eq!(next(), ("released", owner));
        thread.join().unwrap();
    }
}
