//! Settings → Providers → Codex: whether the target device's Codex sessions
//! identify as the Codex CLI. Codex forwards the handshake's client name to
//! the model provider, so endpoints that only admit the CLI reject Zeron's.

use super::HarnessesPage;
use crate::popover::Loadable;
use crate::{settings::widgets, theme::Theme};
use gpui::{AnyElement, Context, SharedString, div, prelude::*, px};
use zeron_rpc::methods;

const LABEL: &str = "Identify as Codex CLI";
const DESCRIPTION: &str = "For API endpoints that only accept the Codex CLI. Applies to new Codex \
                           threads; existing threads keep the identity they started with.";

impl HarnessesPage {
    /// `GetCodexCliIdentity`, or `SetCodexCliIdentity` when saving; either
    /// reply is the target device's stored value.
    pub(super) fn load_codex_identity(&mut self, save: Option<bool>, cx: &mut Context<Self>) {
        if self.codex_identity_pending.is_some() {
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let (method, params) = match save {
            Some(enabled) => (
                methods::SET_CODEX_CLI_IDENTITY,
                self.with_target(serde_json::json!({ "enabled": enabled })),
            ),
            None => (
                methods::GET_CODEX_CLI_IDENTITY,
                self.with_target(serde_json::json!({})),
            ),
        };
        if save.is_none() {
            self.codex_identity = Loadable::Loading;
        }
        self.codex_identity_pending = save;
        self.codex_identity_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(method, params)
                .await
                .map_err(|error| error.to_string())
                .and_then(|value| {
                    value
                        .get("enabled")
                        .and_then(serde_json::Value::as_bool)
                        .ok_or_else(|| "malformed Codex identity reply".to_string())
                });
            this.update(cx, |page, cx| {
                page.codex_identity_pending = None;
                match (result, save) {
                    (Ok(enabled), _) => page.codex_identity = Loadable::Ready(enabled),
                    // Failed validation leaves the confirmed value unchanged.
                    (Err(error), Some(attempted)) => {
                        page.codex_identity = Loadable::Ready(!attempted);
                        page.error = Some(error);
                    }
                    (Err(error), None) => page.codex_identity = Loadable::Error(error),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn toggle_codex_identity(&mut self, cx: &mut Context<Self>) {
        if self.codex_identity_pending.is_some() {
            return;
        }
        let Some(enabled) = self.codex_identity.ready().copied() else {
            return;
        };
        // Keep the last confirmed value until validation and saving succeed.
        self.error = None;
        self.load_codex_identity(Some(!enabled), cx);
    }

    pub(super) fn render_codex_identity(
        &self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let enabled = self.codex_identity.ready().copied();
        let saving = self.codex_identity_pending.is_some();
        let description: SharedString = match self.codex_identity_pending {
            Some(true) => "Checking Codex CLI version…".into(),
            Some(false) => "Saving…".into(),
            None => match &self.codex_identity {
                Loadable::Error(error) => error.clone().into(),
                _ => DESCRIPTION.into(),
            },
        };
        let row = div()
            .id("codex-cli-identity")
            .min_h(px(52.0))
            .py(px(10.0))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(16.0))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(widgets::row_title(theme, LABEL))
                    .child(widgets::meta_line(
                        theme,
                        vec![div().child(description).into_any_element()],
                    )),
            );
        let row = match enabled {
            Some(enabled) => row
                .role(gpui::Role::Switch)
                .aria_label(format!("Codex: {LABEL}"))
                .aria_toggled(if enabled {
                    gpui::Toggled::True
                } else {
                    gpui::Toggled::False
                })
                .when(saving, |row| row.opacity(0.5))
                .when(!saving, |row| {
                    row.tab_index(0)
                        .cursor_pointer()
                        .focus_visible(|s| s.border_2().border_color(theme.accent))
                        .on_click(cx.listener(|page, _, _, cx| page.toggle_codex_identity(cx)))
                        .on_key_down(cx.listener(|page, event: &gpui::KeyDownEvent, _, cx| {
                            if !event.is_held
                                && matches!(event.keystroke.key.as_str(), "enter" | "space")
                            {
                                page.toggle_codex_identity(cx);
                                cx.stop_propagation();
                            }
                        }))
                })
                .child(widgets::toggle_switch(
                    theme,
                    enabled,
                    "codex-cli-identity-switch",
                )),
            None => row.opacity(0.5),
        };
        div()
            .flex()
            .flex_col()
            .child(widgets::details_label(theme, "Identity"))
            .child(row)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn pending_identity_save_blocks_more_toggles_and_reloads(cx: &mut gpui::TestAppContext) {
        let state = cx.new(|_| crate::state::AppState::new());
        let page = cx.new(|cx| HarnessesPage::new(state, cx));
        page.update(cx, |page, cx| {
            page.codex_identity = Loadable::Ready(false);
            page.codex_identity_pending = Some(true);
            page.error = Some("keep until the pending request finishes".into());
            page.toggle_codex_identity(cx);
            page.load_codex_identity(None, cx);
            assert_eq!(page.codex_identity.ready(), Some(&false));
            assert_eq!(page.codex_identity_pending, Some(true));
            assert!(page.error.is_some(), "no second save was started");
        });
    }
}
