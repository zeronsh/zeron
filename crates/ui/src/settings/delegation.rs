//! Per-agent native subagents inside Settings → Providers. The preference
//! lives on the target device's engine, where the agents launch, and is read
//! and written over `GetNativeSubagents` / `SetNativeSubagents`.

use super::HarnessesPage;
use crate::{
    popover::Loadable,
    settings::{
        search::{SearchAnchor, SettingId},
        widgets,
    },
    theme::Theme,
};
use gpui::{AnyElement, Context, div, prelude::*, px};
use zeron_engine::registry::NativeSubagentSettings;
use zeron_proto::HarnessId;
use zeron_rpc::methods;

/// The Delegation row's line while the device can switch it.
const NATIVE_SUBAGENTS_HINT: &str = "Off: delegate through side chats, from each chat's next turn.";

/// The Delegation row's line when the device can't answer; an engine that
/// predates the setting gets commit credit's wording.
fn native_subagents_error(error: zeron_rpc::RpcError) -> String {
    match error {
        zeron_rpc::RpcError::UnknownMethod(_) => {
            "Update Zeron on the selected device to change this.".to_string()
        }
        error => error.to_string(),
    }
}

impl HarnessesPage {
    /// Read the target device's native subagent preferences, or with `set`
    /// switch one agent's. Cloud has none: its machines use the defaults.
    pub(super) fn request_native_subagents(
        &mut self,
        set: Option<(HarnessId, bool)>,
        cx: &mut Context<Self>,
    ) {
        if self.targets_cloud() {
            self.native_subagents = Loadable::Idle;
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let (method, params) = match set {
            Some((harness, enabled)) => (
                methods::SET_NATIVE_SUBAGENTS,
                serde_json::json!({ "harness": harness, "enabled": enabled }),
            ),
            None => (methods::GET_NATIVE_SUBAGENTS, serde_json::json!({})),
        };
        let params = self.with_target(params);
        let target = self.target_device.clone();
        if set.is_none() && self.native_subagents.ready().is_none() {
            self.native_subagents = Loadable::Loading;
        }
        if set.is_some() {
            self.native_subagents_saving = true;
        }
        let task = cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(method, params)
                .await
                .map_err(native_subagents_error)
                .and_then(|value| {
                    serde_json::from_value::<NativeSubagentSettings>(value)
                        .map_err(|error| error.to_string())
                });
            this.update(cx, |page, cx| {
                if page.target_device != target {
                    return;
                }
                if set.is_some() {
                    page.native_subagents_saving = false;
                }
                match result {
                    Ok(settings) => page.native_subagents = Loadable::Ready(settings),
                    // The row says why the device can't be switched.
                    Err(error) if set.is_none() => page.native_subagents = Loadable::Error(error),
                    // A refused switch: say why and show the device's state.
                    Err(error) => {
                        page.error = Some(error);
                        page.request_native_subagents(None, cx);
                    }
                }
                cx.notify();
            })
            .ok();
        });
        if set.is_some() {
            self.native_subagents_toggle = Some(task);
        } else {
            self.native_subagents_task = Some(task);
        }
    }

    fn toggle_native_subagents(&mut self, harness: HarnessId, cx: &mut Context<Self>) {
        // One switch at a time, as commit credit does: the server answers
        // each request in its own task, so a second could apply first.
        if self.native_subagents_saving {
            return;
        }
        let Loadable::Ready(settings) = &mut self.native_subagents else {
            return;
        };
        let enabled = settings.off.contains(&harness);
        // The switch moves at once; the device's reply settles it.
        if enabled {
            settings.off.retain(|off| *off != harness);
        } else {
            settings.off.push(harness);
        }
        self.error = None;
        self.request_native_subagents(Some((harness, enabled)), cx);
        cx.notify();
    }

    /// What `harness`'s Delegation row shows: whether the switch reads on,
    /// whether it can be flipped, and its line. `None` hides the section:
    /// not loaded yet, Cloud, or an agent whose native subagents can't be
    /// switched ([`zeron_harness::switches_native_subagents`]).
    fn native_subagents_row(&self, harness: HarnessId) -> Option<(bool, bool, String)> {
        match &self.native_subagents {
            Loadable::Ready(settings) if settings.switchable.contains(&harness) => Some((
                !settings.off.contains(&harness),
                true,
                NATIVE_SUBAGENTS_HINT.to_string(),
            )),
            // The device didn't answer: the row says why, its switch inert
            // at the default.
            Loadable::Error(error) if zeron_harness::switches_native_subagents(harness) => {
                Some((true, false, error.clone()))
            }
            _ => None,
        }
    }

    /// The expanded provider's Delegation section, for an agent whose native
    /// subagents the device can turn off.
    pub(super) fn render_native_subagents_for(
        &self,
        harness: HarnessId,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let (enabled, interactive, line) = self.native_subagents_row(harness)?;
        let on_click = cx.listener(move |page, _, _, cx| page.toggle_native_subagents(harness, cx));
        let on_key = cx.listener(move |page, event: &gpui::KeyDownEvent, _, cx| {
            if !event.is_held && matches!(event.keystroke.key.as_str(), "enter" | "space") {
                page.toggle_native_subagents(harness, cx);
                cx.stop_propagation();
            }
        });
        let row = div()
            .id(format!("native-subagents-{harness:?}"))
            .min_h(px(52.0))
            .py(px(10.0))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(16.0))
            .role(gpui::Role::Switch)
            .aria_label(format!("{harness:?}: Native subagents"))
            .aria_toggled(if enabled {
                gpui::Toggled::True
            } else {
                gpui::Toggled::False
            })
            .when(interactive, |row| {
                row.tab_index(0)
                    .cursor_pointer()
                    .focus_visible(|s| s.border_2().border_color(theme.accent))
                    .on_click(on_click)
                    .on_key_down(on_key)
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(widgets::row_title(theme, "Native subagents"))
                    .child(widgets::meta_line(
                        theme,
                        vec![div().child(line).into_any_element()],
                    )),
            )
            .child(
                widgets::toggle_switch(
                    theme,
                    enabled,
                    format!("native-subagents-switch-{harness:?}"),
                )
                .when(!interactive, |switch| switch.opacity(0.5)),
            );
        Some(
            div()
                .flex()
                .flex_col()
                .child(widgets::details_label(theme, "Delegation"))
                .child(row.search_anchor(SettingId::NativeSubagents))
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod delegation_tests {
    use super::*;

    fn open_page(
        cx: &mut gpui::TestAppContext,
        dir: &std::path::Path,
    ) -> gpui::WindowHandle<HarnessesPage> {
        use gpui::AppContext;
        cx.update(|cx| {
            crate::settings::init(Default::default(), dir, cx);
            gpui_base::init(cx);
            cx.set_global(Theme::default());
        });
        cx.add_window(|_, cx| {
            let state = cx.new(|_| crate::state::AppState::new());
            HarnessesPage::new(state, cx)
        })
    }

    /// The section shows once the device answers, for an agent it can
    /// switch, and the switch moves before the device confirms.
    #[gpui::test]
    fn native_subagents_show_for_switchable_agents_and_switch_at_once(
        cx: &mut gpui::TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let window = open_page(cx, dir.path());
        window
            .update(cx, |page, _, cx| {
                let theme = Theme::default();
                let shown = |page: &HarnessesPage, harness, cx: &mut Context<HarnessesPage>| {
                    page.render_native_subagents_for(harness, &theme, cx)
                        .is_some()
                };
                assert!(!shown(page, HarnessId::ClaudeCode, cx), "not loaded yet");
                page.native_subagents = Loadable::Ready(NativeSubagentSettings {
                    switchable: vec![HarnessId::ClaudeCode],
                    off: Vec::new(),
                });
                assert!(shown(page, HarnessId::ClaudeCode, cx));
                assert!(!shown(page, HarnessId::Codex, cx));
                page.toggle_native_subagents(HarnessId::ClaudeCode, cx);
                assert_eq!(
                    page.native_subagents.ready().unwrap().off,
                    [HarnessId::ClaudeCode]
                );
                page.toggle_native_subagents(HarnessId::ClaudeCode, cx);
                assert!(page.native_subagents.ready().unwrap().off.is_empty());
                // While the device answers a switch, another flip waits.
                page.native_subagents_saving = true;
                page.toggle_native_subagents(HarnessId::ClaudeCode, cx);
                assert!(page.native_subagents.ready().unwrap().off.is_empty());
            })
            .unwrap();
    }

    /// A device that can't answer still shows the section for an agent that
    /// could switch: the row says why (an older engine: update it), and its
    /// switch stays put.
    #[gpui::test]
    fn native_subagents_say_why_a_device_cannot_switch(cx: &mut gpui::TestAppContext) {
        let update = "Update Zeron on the selected device to change this.";
        assert_eq!(
            native_subagents_error(zeron_rpc::RpcError::UnknownMethod(
                methods::GET_NATIVE_SUBAGENTS.into()
            )),
            update
        );
        assert_eq!(
            native_subagents_error(zeron_rpc::RpcError::Failed("device offline".into())),
            "device offline"
        );
        let dir = tempfile::tempdir().unwrap();
        let window = open_page(cx, dir.path());
        window
            .update(cx, |page, _, cx| {
                page.native_subagents = Loadable::Error(update.into());
                assert_eq!(
                    page.native_subagents_row(HarnessId::ClaudeCode),
                    Some((true, false, update.to_string()))
                );
                assert_eq!(page.native_subagents_row(HarnessId::Codex), None);
                assert!(
                    page.render_native_subagents_for(HarnessId::ClaudeCode, &Theme::default(), cx)
                        .is_some()
                );
                page.toggle_native_subagents(HarnessId::ClaudeCode, cx);
                assert!(
                    matches!(&page.native_subagents, Loadable::Error(error) if error == update),
                    "an inert switch sends nothing"
                );
            })
            .unwrap();
    }
}
