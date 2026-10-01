//! Settings → General → Default permission mode: the mode new chats on this
//! device start in (Bypass unless changed). Stored by the engine with the
//! device's harness preferences (`GetPolicySettings` / `SetPolicySettings`),
//! so agent-created chats and other clients start from the same default.

use gpui::{AnyElement, Context, Entity, Subscription, Task, div, prelude::*, px};
use zeron_proto::PermissionMode;
use zeron_rpc::methods;

use crate::popover::Loadable;
use crate::settings::widgets;
use crate::state::AppState;
use crate::theme::Theme;

pub struct DefaultModeCard {
    state: Entity<AppState>,
    mode: Loadable<PermissionMode>,
    select: widgets::SelectState,
    error: Option<String>,
    task: Option<Task<()>>,
    _state: Subscription,
}

impl DefaultModeCard {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        // Settings can open before the engine connects: load once it does.
        let state_sub = cx.observe(&state, |this: &mut Self, _, cx| {
            if matches!(this.mode, Loadable::Idle) {
                this.call(None, cx);
            }
        });
        let mut card = Self {
            state,
            mode: Loadable::Idle,
            select: widgets::SelectState::default(),
            error: None,
            task: None,
            _state: state_sub,
        };
        card.call(None, cx);
        card
    }

    /// Read the device's settings, or store a new default; either reply is
    /// the device's authoritative value, which composers then start from.
    fn call(&mut self, save: Option<PermissionMode>, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let (method, params) = match save {
            Some(mode) => (
                methods::SET_POLICY_SETTINGS,
                serde_json::json!({ "defaultMode": mode }),
            ),
            None => (methods::GET_POLICY_SETTINGS, serde_json::json!({})),
        };
        if matches!(self.mode, Loadable::Idle) {
            self.mode = Loadable::Loading;
        }
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(method, params)
                .await
                .map_err(|e| e.to_string())
                .and_then(|v| {
                    v.get("defaultMode")
                        .cloned()
                        .map(serde_json::from_value::<PermissionMode>)
                        .unwrap_or(Ok(PermissionMode::Bypass))
                        .map_err(|e| e.to_string())
                });
            this.update(cx, |card, cx| {
                match result {
                    Ok(mode) => {
                        crate::permission_mode::set_device_default(mode, cx);
                        card.mode = Loadable::Ready(mode);
                        card.error = None;
                    }
                    Err(error) if matches!(card.mode, Loadable::Ready(_)) => {
                        card.error = Some(error);
                    }
                    Err(error) => card.mode = Loadable::Error(error),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }
}

impl Render for DefaultModeCard {
    fn render(&mut self, _: &mut gpui::Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).for_settings_surface();
        let control: AnyElement = match &self.mode {
            Loadable::Ready(current) => {
                let current = *current;
                widgets::select(
                    "default-permission-mode",
                    "Default permission mode",
                    &theme,
                    |card: &mut Self| &mut card.select,
                )
                .options(
                    PermissionMode::ALL
                        .iter()
                        .map(|mode| widgets::SelectOption::new(mode.label())),
                    PermissionMode::ALL
                        .iter()
                        .position(|mode| *mode == current)
                        .unwrap_or_default(),
                )
                .width(184.0)
                .on_select(|card, ix, _, cx| {
                    if let Some(mode) = PermissionMode::ALL.get(ix) {
                        card.call(Some(*mode), cx);
                    }
                })
                .render(&self.select, cx)
                .into_any_element()
            }
            Loadable::Error(error) => div()
                .text_color(theme.text_muted)
                .child(error.clone())
                .into_any_element(),
            Loadable::Idle | Loadable::Loading => div()
                .text_color(theme.text_muted)
                .child("Loading…")
                .into_any_element(),
        };
        let description = self
            .mode
            .ready()
            .map(|mode| mode.description())
            .unwrap_or("The mode new chats start in.");
        widgets::section_card(&theme)
            .child(
                widgets::card_row(&theme, true)
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(160.0))
                            .child(widgets::row_title(&theme, "Default permission mode"))
                            .child(widgets::meta_line(
                                &theme,
                                vec![
                                    div()
                                        .child(format!(
                                            "New chats start here. {description}."
                                        ))
                                        .into_any_element(),
                                ],
                            )),
                    )
                    .child(control),
            )
            .when_some(self.error.clone(), |card, error| {
                card.child(widgets::card_row(&theme, false).child(widgets::error_strip(&theme, error)))
            })
    }
}
