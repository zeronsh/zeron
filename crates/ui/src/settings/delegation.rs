//! Per-agent delegation inside Settings → Providers: native subagents and
//! Zeron's own tools. The preferences live on the target device's engine,
//! where the agents launch, and are read over `GetNativeSubagents` and
//! written over `SetNativeSubagents` / `SetZeronTools`.

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

/// The Native subagents row's line while the device can switch it.
const NATIVE_SUBAGENTS_HINT: &str = "Off: delegate through side chats, from each chat's next turn.";

/// The Native subagents row's line while Zeron's tools are off: side chats
/// ride them, so the agent keeps its own subagents whatever the switch says.
const NATIVE_SUBAGENTS_NEED_TOOLS: &str =
    "Side chats need Zeron tools: the agent keeps its own subagents.";

/// The Zeron tools row's line while the device can switch it.
const ZERON_TOOLS_HINT: &str = "Off: the agent runs without Zeron's MCP server (side chats, visuals), from each chat's next turn.";

/// The Delegation switch a request flips.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DelegationSwitch {
    NativeSubagents,
    ZeronTools,
}

impl DelegationSwitch {
    fn method(self) -> &'static str {
        match self {
            DelegationSwitch::NativeSubagents => methods::SET_NATIVE_SUBAGENTS,
            DelegationSwitch::ZeronTools => methods::SET_ZERON_TOOLS,
        }
    }

    fn title(self) -> &'static str {
        match self {
            DelegationSwitch::NativeSubagents => "Native subagents",
            DelegationSwitch::ZeronTools => "Zeron tools",
        }
    }

    fn element_id(self) -> &'static str {
        match self {
            DelegationSwitch::NativeSubagents => "native-subagents",
            DelegationSwitch::ZeronTools => "zeron-tools",
        }
    }

    fn setting(self) -> SettingId {
        match self {
            DelegationSwitch::NativeSubagents => SettingId::NativeSubagents,
            DelegationSwitch::ZeronTools => SettingId::ZeronTools,
        }
    }

    /// The harnesses this switch has turned off.
    fn off(self, settings: &mut NativeSubagentSettings) -> &mut Vec<HarnessId> {
        match self {
            DelegationSwitch::NativeSubagents => &mut settings.off,
            DelegationSwitch::ZeronTools => &mut settings.zeron_tools_off,
        }
    }
}

/// The Delegation rows' line when the device can't answer; an engine that
/// predates a setting gets commit credit's wording.
fn native_subagents_error(error: zeron_rpc::RpcError) -> String {
    match error {
        zeron_rpc::RpcError::UnknownMethod(_) => {
            "Update Zeron on the selected device to change this.".to_string()
        }
        error => error.to_string(),
    }
}

impl HarnessesPage {
    /// Read the target device's delegation preferences, or with `set` flip
    /// one agent's switch. Cloud has none: its machines use the defaults.
    pub(super) fn request_native_subagents(
        &mut self,
        set: Option<(DelegationSwitch, HarnessId, bool)>,
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
            Some((switch, harness, enabled)) => (
                switch.method(),
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
                    // The rows say why the device can't be switched.
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

    fn toggle_delegation(
        &mut self,
        switch: DelegationSwitch,
        harness: HarnessId,
        cx: &mut Context<Self>,
    ) {
        // One switch at a time, as commit credit does: the server answers
        // each request in its own task, so a second could apply first.
        if self.native_subagents_saving {
            return;
        }
        let interactive = match switch {
            DelegationSwitch::NativeSubagents => self.native_subagents_row(harness),
            DelegationSwitch::ZeronTools => self.zeron_tools_row(harness),
        }
        .is_some_and(|(_, interactive, _)| interactive);
        if !interactive {
            return;
        }
        let Loadable::Ready(settings) = &mut self.native_subagents else {
            return;
        };
        let off = switch.off(settings);
        let enabled = off.contains(&harness);
        // The switch moves at once; the device's reply settles it.
        if enabled {
            off.retain(|h| *h != harness);
        } else {
            off.push(harness);
        }
        self.error = None;
        self.request_native_subagents(Some((switch, harness, enabled)), cx);
        cx.notify();
    }

    /// What `harness`'s Native subagents row shows: whether the switch reads
    /// on, whether it can be flipped, and its line. `None` hides the row:
    /// not loaded yet, Cloud, or an agent whose native subagents can't be
    /// switched ([`zeron_harness::switches_native_subagents`]).
    fn native_subagents_row(&self, harness: HarnessId) -> Option<(bool, bool, String)> {
        match &self.native_subagents {
            // Without Zeron's tools there are no side chats to delegate
            // through: the agent keeps its own, and the switch waits.
            Loadable::Ready(settings)
                if settings.switchable.contains(&harness)
                    && settings.zeron_tools_off.contains(&harness) =>
            {
                Some((true, false, NATIVE_SUBAGENTS_NEED_TOOLS.to_string()))
            }
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

    /// What `harness`'s Zeron tools row shows, as [`Self::native_subagents_row`].
    /// Every local agent's runs carry Zeron's server, so every one can switch.
    fn zeron_tools_row(&self, harness: HarnessId) -> Option<(bool, bool, String)> {
        match &self.native_subagents {
            Loadable::Ready(settings) => Some((
                !settings.zeron_tools_off.contains(&harness),
                true,
                ZERON_TOOLS_HINT.to_string(),
            )),
            Loadable::Error(error) => Some((true, false, error.clone())),
            _ => None,
        }
    }

    fn render_delegation_switch(
        &self,
        switch: DelegationSwitch,
        harness: HarnessId,
        (enabled, interactive, line): (bool, bool, String),
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let on_click =
            cx.listener(move |page, _, _, cx| page.toggle_delegation(switch, harness, cx));
        let on_key = cx.listener(move |page, event: &gpui::KeyDownEvent, _, cx| {
            if !event.is_held && matches!(event.keystroke.key.as_str(), "enter" | "space") {
                page.toggle_delegation(switch, harness, cx);
                cx.stop_propagation();
            }
        });
        let id = switch.element_id();
        div()
            .id(format!("{id}-{harness:?}"))
            .min_h(px(52.0))
            .py(px(10.0))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(16.0))
            .role(gpui::Role::Switch)
            .aria_label(format!("{harness:?}: {}", switch.title()))
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
                    .child(widgets::row_title(theme, switch.title()))
                    .child(widgets::meta_line(
                        theme,
                        vec![div().child(line).into_any_element()],
                    )),
            )
            .child(
                widgets::toggle_switch(theme, enabled, format!("{id}-switch-{harness:?}"))
                    .when(!interactive, |toggle| toggle.opacity(0.5)),
            )
            .search_anchor(switch.setting())
            .into_any_element()
    }

    /// The expanded provider's Delegation section: its native subagents,
    /// where the device can turn them off, and Zeron's tools.
    pub(super) fn render_native_subagents_for(
        &self,
        harness: HarnessId,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let rows: Vec<AnyElement> = [
            DelegationSwitch::NativeSubagents,
            DelegationSwitch::ZeronTools,
        ]
        .into_iter()
        .filter_map(|switch| {
            let row = match switch {
                DelegationSwitch::NativeSubagents => self.native_subagents_row(harness),
                DelegationSwitch::ZeronTools => self.zeron_tools_row(harness),
            }?;
            Some(self.render_delegation_switch(switch, harness, row, theme, cx))
        })
        .collect();
        if rows.is_empty() {
            return None;
        }
        Some(
            div()
                .flex()
                .flex_col()
                .child(widgets::details_label(theme, "Delegation"))
                .children(rows)
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
                    ..Default::default()
                });
                assert!(shown(page, HarnessId::ClaudeCode, cx));
                assert!(page.native_subagents_row(HarnessId::ClaudeCode).is_some());
                assert_eq!(page.native_subagents_row(HarnessId::Codex), None);
                let native = DelegationSwitch::NativeSubagents;
                page.toggle_delegation(native, HarnessId::ClaudeCode, cx);
                assert_eq!(
                    page.native_subagents.ready().unwrap().off,
                    [HarnessId::ClaudeCode]
                );
                page.toggle_delegation(native, HarnessId::ClaudeCode, cx);
                assert!(page.native_subagents.ready().unwrap().off.is_empty());
                // While the device answers a switch, another flip waits.
                page.native_subagents_saving = true;
                page.toggle_delegation(native, HarnessId::ClaudeCode, cx);
                assert!(page.native_subagents.ready().unwrap().off.is_empty());
            })
            .unwrap();
    }

    /// Every agent can turn Zeron's tools off; with them off, its native
    /// subagents switch waits, reading on, since side chats ride the tools.
    #[gpui::test]
    fn zeron_tools_switch_for_every_agent_and_hold_native_subagents(cx: &mut gpui::TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let window = open_page(cx, dir.path());
        window
            .update(cx, |page, _, cx| {
                page.native_subagents = Loadable::Ready(NativeSubagentSettings {
                    switchable: vec![HarnessId::ClaudeCode],
                    off: vec![HarnessId::ClaudeCode],
                    ..Default::default()
                });
                // An agent without native subagents to switch still shows
                // the section, for its tools.
                assert!(
                    page.render_native_subagents_for(HarnessId::Codex, &Theme::default(), cx)
                        .is_some()
                );
                let tools = DelegationSwitch::ZeronTools;
                page.toggle_delegation(tools, HarnessId::ClaudeCode, cx);
                assert_eq!(
                    page.native_subagents.ready().unwrap().zeron_tools_off,
                    [HarnessId::ClaudeCode]
                );
                assert_eq!(
                    page.zeron_tools_row(HarnessId::ClaudeCode),
                    Some((false, true, ZERON_TOOLS_HINT.to_string()))
                );
                assert_eq!(
                    page.native_subagents_row(HarnessId::ClaudeCode),
                    Some((true, false, NATIVE_SUBAGENTS_NEED_TOOLS.to_string()))
                );
                page.native_subagents_saving = false;
                page.toggle_delegation(
                    DelegationSwitch::NativeSubagents,
                    HarnessId::ClaudeCode,
                    cx,
                );
                assert_eq!(
                    page.native_subagents.ready().unwrap().off,
                    [HarnessId::ClaudeCode],
                    "an inert switch sends nothing"
                );
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
                assert_eq!(
                    page.zeron_tools_row(HarnessId::Codex),
                    Some((true, false, update.to_string()))
                );
                assert!(
                    page.render_native_subagents_for(HarnessId::ClaudeCode, &Theme::default(), cx)
                        .is_some()
                );
                page.toggle_delegation(
                    DelegationSwitch::NativeSubagents,
                    HarnessId::ClaudeCode,
                    cx,
                );
                page.toggle_delegation(DelegationSwitch::ZeronTools, HarnessId::Codex, cx);
                assert!(
                    matches!(&page.native_subagents, Loadable::Error(error) if error == update),
                    "an inert switch sends nothing"
                );
            })
            .unwrap();
    }
}
