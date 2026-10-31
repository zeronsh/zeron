//! Composer prewarm: waking a hibernated chat's agent before the message
//! arrives. Focusing the composer, or starting to type in it, asks the chat's
//! host to resume the agent into a parked runtime (`PrewarmChat`), so the
//! first send finds it started. The engine decides whether there is anything
//! to resume; this side keeps the asks cheap: bursts collapse into one after
//! a short delay, a chat is asked at most once a minute, and nothing is asked
//! of a host that cannot prewarm.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use gpui::{Context, Focusable, Subscription, Task, Window};
use zeron_proto::{RuntimeState, SessionStatus};
use zeron_rpc::methods;

use crate::composer::Composer;

/// Triggers inside this window collapse into one ask, sent when it ends.
pub const PREWARM_DEBOUNCE: Duration = Duration::from_millis(300);
/// The least time between two asks for the same chat.
pub const PREWARM_MIN_INTERVAL: Duration = Duration::from_secs(60);

/// When each chat was last asked.
#[derive(Default)]
pub struct PrewarmGate {
    asked: HashMap<String, Instant>,
}

impl PrewarmGate {
    /// Whether `chat_id` may be asked at `now`; if so it counts as asked.
    pub fn allow(&mut self, chat_id: &str, now: Instant) -> bool {
        if self
            .asked
            .get(chat_id)
            .is_some_and(|at| now.saturating_duration_since(*at) < PREWARM_MIN_INTERVAL)
        {
            return false;
        }
        self.asked
            .retain(|_, at| now.saturating_duration_since(*at) < PREWARM_MIN_INTERVAL);
        self.asked.insert(chat_id.to_owned(), now);
        true
    }
}

#[derive(Default)]
pub struct PrewarmState {
    gate: PrewarmGate,
    /// The ask waiting out its debounce.
    pending: Option<Task<()>>,
    focus: Option<Subscription>,
    /// The chat the composer last had focus on, so opening another chat
    /// with the input already focused still asks.
    focused_key: Option<String>,
}

impl Composer {
    /// Watch the input's focus, and the chat it focuses on. Called from
    /// render, which is where the window is in hand.
    pub(crate) fn track_prewarm_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.prewarm.focus.is_none() {
            let focus = self.input.focus_handle(cx);
            self.prewarm.focus = Some(cx.on_focus_in(&focus, window, |this, _, cx| {
                this.prewarm.focused_key = Some(this.current_key.clone());
                this.schedule_prewarm(cx);
            }));
        }
        if self.input.focus_handle(cx).is_focused(window) {
            if self.prewarm.focused_key.as_deref() != Some(self.current_key.as_str()) {
                self.prewarm.focused_key = Some(self.current_key.clone());
                self.schedule_prewarm(cx);
            }
        } else {
            self.prewarm.focused_key = None;
        }
    }

    /// An edit of the draft. Only typing in the focused input counts: a draft
    /// restored by opening a chat is not the user reaching for it.
    pub(crate) fn prewarm_on_typing(&mut self, cx: &mut Context<Self>) {
        if self.prewarm.focused_key.is_some() && !self.input.read(cx).text().is_empty() {
            self.schedule_prewarm(cx);
        }
    }

    /// Ask for a prewarm of the chat the composer shows once the debounce
    /// passes. A second trigger while one waits joins it.
    pub(crate) fn schedule_prewarm(&mut self, cx: &mut Context<Self>) {
        if self.prewarm.pending.is_some() || self.prewarm_target(cx).is_none() {
            return;
        }
        self.prewarm.pending = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(PREWARM_DEBOUNCE).await;
            this.update(cx, |composer, cx| {
                composer.prewarm.pending = None;
                composer.send_prewarm(cx);
            })
            .ok();
        }));
    }

    /// The chat to prewarm and its host: an existing chat whose agent is
    /// not known to be running, on a host that can prewarm it.
    fn prewarm_target(&self, cx: &gpui::App) -> Option<(String, String)> {
        if self.current_key.is_empty() {
            return None;
        }
        let capability = zeron_proto::capabilities::PREWARM_V1;
        let state = self.state.read(cx);
        let host = state
            .chats
            .iter()
            .find(|chat| chat.id == self.current_key)?
            .device_id
            .clone();
        // A remote host's request goes through this window's own engine,
        // which forwards only the methods it knows.
        let supported = state.device_supports(&host, capability)
            && state.engine()?.engine_info().supports(capability);
        let running = matches!(
            state.runtime_state_for(&self.current_key),
            Some(RuntimeState::Live | RuntimeState::Parked | RuntimeState::Starting)
        ) || state
            .session_for(&self.current_key)
            .is_some_and(|session| session.status != SessionStatus::Idle);
        (supported && !running).then(|| (self.current_key.clone(), host))
    }

    fn send_prewarm(&mut self, cx: &mut Context<Self>) {
        let Some((chat_id, host)) = self.prewarm_target(cx) else {
            return;
        };
        if !self.prewarm.gate.allow(&chat_id, Instant::now()) {
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let params = serde_json::json!({ "chatId": chat_id, "targetDeviceId": host });
        cx.spawn(async move |_, _| {
            if let Err(error) = engine.client().call(methods::PREWARM_CHAT, params).await {
                tracing::debug!(%error, "prewarm request failed");
            }
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pull_request_test_support::{self as fixture, ScriptedRpc};
    use crate::state::{AppState, EngineHandle};
    use gpui::Entity;
    use std::sync::{Arc, Mutex};

    #[test]
    fn a_chat_is_asked_at_most_once_a_minute() {
        let mut gate = PrewarmGate::default();
        let start = Instant::now();
        assert!(gate.allow("a", start));
        assert!(!gate.allow("a", start + Duration::from_secs(59)));
        // Another chat has its own minute.
        assert!(gate.allow("b", start + Duration::from_secs(30)));
        assert!(!gate.allow("b", start + Duration::from_secs(89)));
        assert!(gate.allow("a", start + PREWARM_MIN_INTERVAL));
        assert!(gate.allow("b", start + Duration::from_secs(90)));
        // An ask refused by the limit does not push the next one back.
        assert!(!gate.allow("a", start + Duration::from_secs(119)));
        assert!(gate.allow("a", start + Duration::from_secs(120)));
    }

    type Calls = Arc<Mutex<Vec<serde_json::Value>>>;

    struct Rig {
        composer: Entity<Composer>,
        state: Entity<AppState>,
        rpc: Arc<ScriptedRpc>,
        calls: Calls,
        runtime: tokio::runtime::Runtime,
        _dir: tempfile::TempDir,
    }

    /// A composer on chat `c1` (hosted by the engine's own device), with
    /// `capabilities` on the engine, or the current set when `None`.
    fn rig<'a>(
        cx: &'a mut gpui::TestAppContext,
        capabilities: Option<&[&str]>,
    ) -> (Rig, &'a mut gpui::VisualTestContext) {
        let runtime = fixture::runtime();
        let _guard = runtime.enter();
        let dir = fixture::settings(cx, Default::default());
        cx.update(|cx| {
            gpui_base::init(cx);
            crate::app_menus::init(cx);
            crate::history::init(
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                cx,
            );
        });
        let calls: Calls = Default::default();
        let log = calls.clone();
        let rpc = ScriptedRpc::new(move |method, params| {
            let log = log.clone();
            async move {
                if method == methods::PREWARM_CHAT {
                    log.lock().unwrap().push(params);
                }
                zeron_rpc::RpcReply::value(&serde_json::json!({ "prewarming": true }))
            }
        });
        let mut engine = EngineHandle::from_test_client(rpc.client());
        if let Some(capabilities) = capabilities {
            engine = engine.with_test_capabilities(capabilities);
        }
        let (composer, cx) = cx.add_window_view(|_, cx| {
            let state = fixture::state(cx, None);
            state.update(cx, |state, _| {
                state.set_test_engine(engine);
                state.chats = ["c1", "c2"]
                    .into_iter()
                    .map(|id| {
                        serde_json::from_value(serde_json::json!({
                            "id": id, "title": id, "deviceId": "local", "archived": false,
                            "createdAt": chrono::Utc::now(),
                        }))
                        .unwrap()
                    })
                    .collect();
                state.selected_chat = Some("c1".into());
            });
            Composer::new(state, cx)
        });
        let state = composer.read_with(cx, |composer, _| composer.state.clone());
        (
            Rig {
                composer,
                state,
                rpc,
                calls,
                runtime,
                _dir: dir,
            },
            cx,
        )
    }

    impl Rig {
        fn focus(&self, cx: &mut gpui::VisualTestContext) {
            self.composer.update_in(cx, |composer, window, cx| {
                window.blur();
                let focus = composer.input.focus_handle(cx);
                window.focus(&focus, cx);
            });
            cx.update(|window, cx| window.draw(cx).clear());
        }

        /// Let the debounce pass and any request reach the engine. Real
        /// time passes too, for the transport behind the scripted engine.
        fn settle(&self, cx: &mut gpui::VisualTestContext) {
            cx.executor().advance_clock(PREWARM_DEBOUNCE * 2);
            for _ in 0..20 {
                cx.run_until_parked();
                self.runtime
                    .block_on(tokio::time::sleep(Duration::from_millis(5)));
            }
            cx.run_until_parked();
        }

        fn asked(&self) -> Vec<serde_json::Value> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[gpui::test]
    fn focusing_the_composer_asks_once_after_the_debounce(cx: &mut gpui::TestAppContext) {
        let (rig, cx) = rig(cx, None);
        let _guard = rig.runtime.enter();
        rig.focus(cx);
        // Nothing yet: the ask waits out the debounce.
        cx.run_until_parked();
        assert!(rig.asked().is_empty());
        rig.settle(cx);
        assert_eq!(rig.asked().len(), 1);
        assert_eq!(rig.asked()[0]["chatId"], "c1");
        assert_eq!(rig.asked()[0]["targetDeviceId"], "local");
    }

    #[gpui::test]
    fn a_burst_of_triggers_makes_one_ask_and_the_minute_holds_the_next(
        cx: &mut gpui::TestAppContext,
    ) {
        let (rig, cx) = rig(cx, None);
        let _guard = rig.runtime.enter();
        rig.focus(cx);
        // Typing joins the pending ask instead of making its own.
        for text in ["h", "he", "hel"] {
            rig.composer.update(cx, |composer, cx| {
                composer
                    .input
                    .update(cx, |input, cx| input.set_text(text, cx));
            });
        }
        rig.settle(cx);
        assert_eq!(rig.asked().len(), 1);
        // Refocusing and typing again inside the minute asks nothing.
        rig.focus(cx);
        rig.composer.update(cx, |composer, cx| {
            composer
                .input
                .update(cx, |input, cx| input.set_text("help", cx));
        });
        rig.settle(cx);
        assert_eq!(rig.asked().len(), 1, "once per chat per minute");
        // Another chat is its own ask.
        rig.state.update(cx, |state, cx| {
            state.selected_chat = Some("c2".into());
            cx.notify();
        });
        rig.focus(cx);
        rig.settle(cx);
        let asked = rig.asked();
        assert_eq!(asked.len(), 2);
        assert_eq!(asked[1]["chatId"], "c2");
    }

    #[gpui::test]
    fn typing_counts_only_in_the_focused_input(cx: &mut gpui::TestAppContext) {
        let (rig, cx) = rig(cx, None);
        let _guard = rig.runtime.enter();
        // A draft restored by opening the chat is not a reach for it.
        cx.update(|window, _| window.blur());
        rig.composer.update(cx, |composer, cx| {
            composer
                .input
                .update(cx, |input, cx| input.set_text("draft", cx));
        });
        rig.settle(cx);
        assert!(rig.asked().is_empty());
    }

    #[gpui::test]
    fn nothing_is_asked_of_a_running_agent_a_new_chat_or_an_old_engine(
        cx: &mut gpui::TestAppContext,
    ) {
        // A runtime the engine reports as running needs no wake-up.
        let (rig, cx) = rig(cx, None);
        let _guard = rig.runtime.enter();
        rig.state.update(cx, |state, _| {
            state.apply_runtime_states(
                [("c1".to_owned(), zeron_proto::RuntimeState::Parked)].into(),
            );
        });
        rig.focus(cx);
        rig.settle(cx);
        assert!(rig.asked().is_empty(), "parked");
        // Hibernated: now it is worth asking.
        rig.state.update(cx, |state, _| {
            state.apply_runtime_states(
                [("c1".to_owned(), zeron_proto::RuntimeState::Hibernated)].into(),
            );
        });
        rig.focus(cx);
        rig.settle(cx);
        assert_eq!(rig.asked().len(), 1);
    }

    #[gpui::test]
    fn the_new_chat_canvas_asks_nothing(cx: &mut gpui::TestAppContext) {
        let (rig, cx) = rig(cx, None);
        let _guard = rig.runtime.enter();
        rig.state.update(cx, |state, cx| {
            state.selected_chat = None;
            cx.notify();
        });
        rig.focus(cx);
        rig.composer.update(cx, |composer, cx| {
            composer
                .input
                .update(cx, |input, cx| input.set_text("hi", cx));
        });
        rig.settle(cx);
        assert!(rig.asked().is_empty());
    }

    #[gpui::test]
    fn an_engine_without_prewarm_is_never_asked(cx: &mut gpui::TestAppContext) {
        let (rig, cx) = rig(cx, Some(&[zeron_proto::capabilities::MCP_LAZY_V1]));
        let _guard = rig.runtime.enter();
        rig.focus(cx);
        rig.composer.update(cx, |composer, cx| {
            composer
                .input
                .update(cx, |input, cx| input.set_text("hi", cx));
        });
        rig.settle(cx);
        assert!(rig.asked().is_empty());
    }
}
