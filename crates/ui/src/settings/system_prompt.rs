//! Settings → General → System prompt: this device's own instructions for
//! its Claude Code and Codex runs. The engine stores and applies it (see
//! `GetSystemPrompt` / `SetSystemPrompt`); the Zeron MCP server edits the
//! same setting.

use gpui::{AppContext as _, Context, Entity, Subscription, Task, div, prelude::*, px};
use zeron_rpc::methods;

use crate::composer::{ComposerInput, ComposerInputEvent, TEXT_EDITOR_CONTEXT};
use crate::popover::Loadable;
use crate::settings::widgets;
use crate::state::AppState;
use crate::theme::Theme;

/// The field grows to this many lines, then scrolls.
const MAX_FIELD_HEIGHT: f32 = 220.0;

pub struct SystemPromptCard {
    state: Entity<AppState>,
    /// The prompt the device has, once loaded.
    saved: Loadable<Option<String>>,
    input: Entity<ComposerInput>,
    error: Option<String>,
    task: Option<Task<()>>,
    _input: Subscription,
    _state: Subscription,
}

impl SystemPromptCard {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| {
            ComposerInput::with_context(
                "For example: Answer in French. Prefer small, focused commits.",
                TEXT_EDITOR_CONTEXT,
                cx,
            )
            .with_text_metrics(13.0, 20.0)
            .with_viewport_height(MAX_FIELD_HEIGHT)
        });
        let input_sub = cx.subscribe(&input, |_, _, event, cx| {
            if matches!(event, ComposerInputEvent::Edited) {
                cx.notify();
            }
        });
        // Settings can open before the engine connects: load once it does.
        let state_sub = cx.observe(&state, |this: &mut Self, _, cx| {
            if matches!(this.saved, Loadable::Idle) {
                this.call(None, cx);
            }
        });
        let mut card = Self {
            state,
            saved: Loadable::Idle,
            input,
            error: None,
            task: None,
            _input: input_sub,
            _state: state_sub,
        };
        card.call(None, cx);
        card
    }

    fn draft(&self, cx: &gpui::App) -> String {
        self.input.read(cx).text().trim().to_string()
    }

    fn dirty(&self, cx: &gpui::App) -> bool {
        match &self.saved {
            Loadable::Ready(saved) => self.draft(cx) != saved.clone().unwrap_or_default(),
            _ => false,
        }
    }

    /// `GetSystemPrompt`, or `SetSystemPrompt` when saving (`Some("")`
    /// clears); either reply is the device's prompt.
    fn call(&mut self, save: Option<String>, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let (method, params) = match save {
            Some(text) => (
                methods::SET_SYSTEM_PROMPT,
                serde_json::json!({ "text": text }),
            ),
            None => (methods::GET_SYSTEM_PROMPT, serde_json::json!({})),
        };
        if matches!(self.saved, Loadable::Idle) {
            self.saved = Loadable::Loading;
        }
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(method, params)
                .await
                .map_err(|e| e.to_string())
                .map(|value| value["text"].as_str().map(str::to_owned));
            this.update(cx, |card, cx| {
                match result {
                    Ok(text) => {
                        let shown = text.clone().unwrap_or_default();
                        card.input.update(cx, |input, cx| input.set_text(shown, cx));
                        card.saved = Loadable::Ready(text);
                        card.error = None;
                    }
                    Err(error) if matches!(card.saved, Loadable::Ready(_)) => {
                        card.error = Some(error);
                    }
                    Err(error) => card.saved = Loadable::Error(error),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }
}

impl Render for SystemPromptCard {
    fn render(&mut self, _: &mut gpui::Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::toast::take_error(&mut self.error, cx);
        let theme = Theme::of(cx).for_settings_surface();
        let dirty = self.dirty(cx);
        let has_saved = matches!(&self.saved, Loadable::Ready(Some(_)));
        let header = widgets::card_row(&theme, true).child(
            div()
                .flex_1()
                .min_w_0()
                .child(widgets::row_title(&theme, "System prompt"))
                .child(widgets::meta_line(
                    &theme,
                    vec![
                        div()
                            .child(
                                "Added to the instructions of Claude Code and Codex on this \
                                 device. Applies from each chat's next message.",
                            )
                            .into_any_element(),
                    ],
                )),
        );
        let body = match &self.saved {
            Loadable::Ready(_) => div()
                .px(px(16.0))
                .pb(px(14.0))
                .flex()
                .flex_col()
                .gap(px(10.0))
                .child(
                    div()
                        .id("system-prompt-field")
                        .min_h(px(64.0))
                        .px(px(12.0))
                        .py(px(10.0))
                        .rounded(px(8.0))
                        .bg(theme.input_bg)
                        .border_1()
                        .border_color(theme.border)
                        .cursor_text()
                        .child(self.input.clone()),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .justify_end()
                        .gap(px(6.0))
                        .when(has_saved, |row| {
                            row.child(
                                widgets::text_action(&theme, widgets::ActionTone::Quiet, "Clear")
                                    .id("system-prompt-clear")
                                    .tab_index(0)
                                    .role(gpui::Role::Button)
                                    .aria_label("Clear the system prompt")
                                    .focus_visible(|s| s.border_2().border_color(theme.accent))
                                    .on_click(cx.listener(|card, _, _, cx| {
                                        card.call(Some(String::new()), cx)
                                    })),
                            )
                        })
                        .child(
                            widgets::text_action(&theme, widgets::ActionTone::Solid, "Save")
                                .id("system-prompt-save")
                                .tab_index(0)
                                .role(gpui::Role::Button)
                                .aria_label("Save the system prompt")
                                .focus_visible(|s| s.border_2().border_color(theme.accent))
                                .when(!dirty, |el| el.opacity(0.4))
                                .on_click(cx.listener(|card, _, _, cx| {
                                    if card.dirty(cx) {
                                        let text = card.draft(cx);
                                        card.call(Some(text), cx)
                                    }
                                })),
                        ),
                )
                .into_any_element(),
            Loadable::Error(error) => div()
                .px(px(16.0))
                .pb(px(14.0))
                .text_color(theme.text_muted)
                .child(error.clone())
                .into_any_element(),
            Loadable::Idle | Loadable::Loading => div()
                .px(px(16.0))
                .pb(px(14.0))
                .text_color(theme.text_muted)
                .child("Loading…")
                .into_any_element(),
        };
        widgets::section(
            &theme,
            "Agents",
            widgets::section_card(&theme)
                .mt_0()
                .child(header)
                .child(body),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use gpui::TestAppContext;

    use super::*;

    /// The engine side: one stored prompt, every call recorded.
    struct PromptRpc {
        text: Mutex<Option<String>>,
        calls: Arc<Mutex<Vec<(String, serde_json::Value)>>>,
    }

    #[async_trait::async_trait]
    impl zeron_rpc::RpcService for PromptRpc {
        async fn handle(
            &self,
            method: &str,
            params: serde_json::Value,
        ) -> Result<zeron_rpc::RpcReply, zeron_rpc::RpcError> {
            self.calls
                .lock()
                .unwrap()
                .push((method.to_string(), params.clone()));
            if method == methods::SET_SYSTEM_PROMPT {
                *self.text.lock().unwrap() = params["text"]
                    .as_str()
                    .filter(|text| !text.trim().is_empty())
                    .map(str::to_owned);
            }
            zeron_rpc::RpcReply::value(&serde_json::json!({ "text": *self.text.lock().unwrap() }))
        }
    }

    #[gpui::test]
    fn the_card_loads_saves_and_clears_the_devices_prompt(cx: &mut TestAppContext) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _guard = runtime.enter();
        cx.update(|cx| {
            cx.set_global(Theme::default());
            crate::composer::init(cx, Default::default());
        });
        let calls = Arc::new(Mutex::new(Vec::new()));
        let rpc = Arc::new(PromptRpc {
            text: Mutex::new(Some("Be brief.".into())),
            calls: calls.clone(),
        });
        let state = cx.new(|_| AppState::new());
        state.update(cx, |state, _| {
            state.set_test_engine(crate::state::EngineHandle::from_test_client(
                zeron_rpc::memory_client(rpc),
            ))
        });
        let (card, cx) = cx.add_window_view(|_, cx| SystemPromptCard::new(state, cx));
        let settle = |cx: &mut gpui::VisualTestContext| {
            for _ in 0..50 {
                cx.run_until_parked();
                runtime.block_on(async {
                    for _ in 0..8 {
                        tokio::task::yield_now().await;
                    }
                });
            }
        };
        settle(cx);
        card.read_with(cx, |card, cx| {
            assert_eq!(card.input.read(cx).text(), "Be brief.");
            assert!(!card.dirty(cx));
        });
        card.update(cx, |card, cx| {
            card.input
                .update(cx, |input, cx| input.set_text("Answer in French.", cx));
            assert!(card.dirty(cx));
            let text = card.draft(cx);
            card.call(Some(text), cx);
        });
        settle(cx);
        card.update(cx, |card, cx| {
            assert!(!card.dirty(cx), "the saved prompt is the draft");
            card.call(Some(String::new()), cx);
        });
        settle(cx);
        card.read_with(cx, |card, cx| {
            assert_eq!(card.input.read(cx).text(), "");
            assert!(matches!(card.saved, Loadable::Ready(None)));
        });
        let sets: Vec<_> = calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(method, _)| method == methods::SET_SYSTEM_PROMPT)
            .map(|(_, params)| params["text"].clone())
            .collect();
        assert_eq!(
            sets,
            vec![
                serde_json::json!("Answer in French."),
                serde_json::json!("")
            ]
        );
    }
}
