//! Settings → General → Usage limits: whether turns a usage limit stops on a
//! device pick themselves back up once the limit resets (that device's
//! `GetAutoResume` / `SetAutoResume`, picked like the Git section's device).
//! Each stop's error card can still turn it off for that stop alone.

use gpui::{AnyElement, Context, Entity, Render, Subscription, Task, Window, div, prelude::*, px};
use zeron_proto::AutoResumeSettings;
use zeron_rpc::methods;

use super::search::{SearchAnchor, SettingId};
use super::widgets;
use crate::popover::Loadable;
use crate::state::AppState;
use crate::theme::Theme;

pub(super) struct AutoResumeCard {
    state: Entity<AppState>,
    /// The device whose setting this is; `None` = this one.
    target: Option<String>,
    device_select: widgets::SelectState,
    settings: Loadable<AutoResumeSettings>,
    /// The switch flips at once; the device's reply settles it.
    saving: bool,
    task: Option<Task<()>>,
    _state: Subscription,
}

impl AutoResumeCard {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        // Settings can open before the engine connects: load once it does.
        let observe = cx.observe(&state, |card: &mut Self, _, cx| {
            if matches!(card.settings, Loadable::Idle) {
                card.request(None, cx);
            }
        });
        let mut card = Self {
            state,
            target: None,
            device_select: widgets::SelectState::default(),
            settings: Loadable::Idle,
            saving: false,
            task: None,
            _state: observe,
        };
        card.request(None, cx);
        card
    }

    fn load(&mut self, target: Option<String>, cx: &mut Context<Self>) {
        self.target = target;
        self.settings = Loadable::Idle;
        self.saving = false;
        self.task = None;
        self.request(None, cx);
    }

    /// `GetAutoResume`, or `SetAutoResume` when saving, on the picked
    /// device; either reply is its authoritative setting.
    fn request(&mut self, save: Option<bool>, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            if save.is_some() {
                self.settle(save, Err("Device is not connected".into()), cx);
            }
            return;
        };
        let (method, mut params) = match save {
            Some(enabled) => (
                methods::SET_AUTO_RESUME,
                serde_json::json!({ "enabled": enabled }),
            ),
            None => (methods::GET_AUTO_RESUME, serde_json::json!({})),
        };
        if let Some(target) = &self.target {
            params["targetDeviceId"] = serde_json::json!(target);
        }
        if matches!(self.settings, Loadable::Idle) {
            self.settings = Loadable::Loading;
        }
        self.saving = save.is_some();
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(method, params)
                .await
                .map_err(|error| match error {
                    zeron_rpc::RpcError::UnknownMethod(_) => {
                        "Update Zeron on the selected device to change this.".to_string()
                    }
                    error => error.to_string(),
                })
                .and_then(|value| {
                    serde_json::from_value::<AutoResumeSettings>(value)
                        .map_err(|error| error.to_string())
                });
            this.update(cx, |card, cx| {
                card.saving = false;
                card.settle(save, result, cx);
            })
            .ok();
        }));
        cx.notify();
    }

    fn settle(
        &mut self,
        save: Option<bool>,
        result: Result<AutoResumeSettings, String>,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(settings) => self.settings = Loadable::Ready(settings),
            // A failed save keeps the setting it had: the switch flips back.
            Err(error) if save.is_some() => {
                if let Loadable::Ready(settings) = &mut self.settings {
                    settings.enabled = !settings.enabled;
                }
                crate::toast::error(cx, error);
            }
            Err(error) => self.settings = Loadable::Error(error),
        }
        cx.notify();
    }

    fn toggle(&mut self, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        let Loadable::Ready(settings) = &mut self.settings else {
            return;
        };
        settings.enabled = !settings.enabled;
        let enabled = settings.enabled;
        self.request(Some(enabled), cx);
    }
}

impl Render for AutoResumeCard {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).for_settings_surface();
        // On until the device says otherwise: the setting's default.
        let enabled = match &self.settings {
            Loadable::Ready(settings) => settings.enabled,
            _ => AutoResumeSettings::default().enabled,
        };
        let ready = self.settings.ready().is_some();
        let interactive = ready && !self.saving;
        let meta = match &self.settings {
            Loadable::Error(error) => error.clone(),
            _ => "Chats on this device, and their subagents, continue once a usage limit \
                  resets, including chats already waiting."
                .into(),
        };
        let state = self.state.clone();
        let switcher = super::worktrees::device_switcher(
            "auto-resume-device-switcher",
            &state,
            self.target.clone(),
            &self.device_select,
            |card: &mut Self| &mut card.device_select,
            |card, target, cx| {
                if card.target != target {
                    card.load(target, cx);
                }
            },
            &theme,
            cx,
        );
        let row: AnyElement = widgets::card_row(&theme, true)
            .child(
                div()
                    .flex_1()
                    .min_w(px(160.0))
                    .child(widgets::row_title(&theme, "Auto-resume when limits reset"))
                    .child(widgets::meta_line(
                        &theme,
                        vec![div().child(meta).into_any_element()],
                    )),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(switcher)
                    .child(
                        widgets::toggle_switch(&theme, enabled, "auto-resume")
                            .id("auto-resume-toggle")
                            .debug_selector(|| "auto-resume-setting-toggle".into())
                            .flex_none()
                            .role(gpui::Role::Switch)
                            .aria_label("Auto-resume when limits reset")
                            .aria_toggled(if enabled {
                                gpui::Toggled::True
                            } else {
                                gpui::Toggled::False
                            })
                            .when(interactive, |el| {
                                el.cursor_pointer()
                                    .tab_index(0)
                                    .focus_visible(|s| s.border_2().border_color(theme.accent))
                                    .on_click(cx.listener(|card, _, _, cx| card.toggle(cx)))
                            })
                            // Loading reads as the default, not as a dimmed off.
                            .when(
                                self.saving || matches!(self.settings, Loadable::Error(_)),
                                |el| el.opacity(0.5),
                            ),
                    ),
            )
            .search_anchor(SettingId::AutoResume)
            .into_any_element();
        widgets::section(
            &theme,
            "Usage limits",
            widgets::section_card(&theme).mt_0().child(row),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    fn init(cx: &mut TestAppContext) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            crate::settings::init(Default::default(), dir.path(), cx);
            gpui_base::init(cx);
            cx.set_global(Theme::default());
        });
        dir
    }

    #[gpui::test]
    fn without_an_engine_the_switch_reads_on_and_a_flip_does_not_stick(cx: &mut TestAppContext) {
        let _dir = init(cx);
        let state = cx.new(|_| AppState::new());
        let (card, cx) = cx.add_window_view(|_, cx| AutoResumeCard::new(state, cx));
        cx.update(|window, cx| window.draw(cx).clear());
        // Nothing loaded yet: the default (on), and no flipping.
        card.update(cx, |card, cx| {
            assert!(matches!(card.settings, Loadable::Idle));
            card.toggle(cx);
            assert!(matches!(card.settings, Loadable::Idle));
            // Loaded once, then the engine is gone: the flip goes back.
            card.settle(None, Ok(AutoResumeSettings::default()), cx);
        });
        cx.update(|window, cx| window.draw(cx).clear());
        let toggle = cx
            .debug_bounds("auto-resume-setting-toggle")
            .expect("the switch renders");
        cx.simulate_click(toggle.center(), gpui::Modifiers::default());
        card.update(cx, |card, cx| {
            assert_eq!(card.settings.ready().map(|s| s.enabled), Some(true));
            assert_eq!(
                crate::toast::messages(cx),
                vec![gpui::SharedString::from("Device is not connected")]
            );
        });
    }

    #[gpui::test]
    fn the_switch_reads_and_sets_the_devices_setting(cx: &mut TestAppContext) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _guard = runtime.enter();
        let (out, mut requests) = tokio::sync::mpsc::channel(16);
        let (replies, inbound) = tokio::sync::mpsc::channel(16);
        let engine =
            crate::state::EngineHandle::from_test_client(zeron_rpc::RpcClient::new(out, inbound));
        let _dir = init(cx);
        let window = cx.add_window(|_, cx| {
            let state = cx.new(|_| {
                let mut state = AppState::new();
                state.set_test_engine(engine);
                state
            });
            AutoResumeCard::new(state, cx)
        });
        let mut answer = |cx: &mut TestAppContext, method: &str, reply: serde_json::Value| {
            cx.run_until_parked();
            let request: serde_json::Value =
                serde_json::from_str(&requests.try_recv().expect("a call to the device")).unwrap();
            assert_eq!(request["method"], method, "{request}");
            runtime.block_on(async {
                let reply = serde_json::json!({ "id": request["id"], "ok": reply });
                replies.send(reply.to_string()).await.unwrap();
                while replies.capacity() < replies.max_capacity() {
                    tokio::task::yield_now().await;
                }
            });
            cx.run_until_parked();
            request
        };
        answer(
            cx,
            methods::GET_AUTO_RESUME,
            serde_json::json!({ "enabled": false }),
        );
        window
            .update(cx, |card, _, cx| {
                assert_eq!(card.settings.ready().map(|s| s.enabled), Some(false));
                card.toggle(cx);
                assert!(card.saving);
            })
            .unwrap();
        let request = answer(
            cx,
            methods::SET_AUTO_RESUME,
            serde_json::json!({ "enabled": true }),
        );
        assert_eq!(request["params"]["enabled"], true);
        window
            .update(cx, |card, _, _| {
                assert!(!card.saving);
                assert_eq!(card.settings.ready().map(|s| s.enabled), Some(true));
            })
            .unwrap();
    }
}
