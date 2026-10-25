//! Stopping one running tool call without ending the agent's turn: a command
//! chip, a subagent's spawn chip, or its row in the lanes card. The agent
//! keeps going and sees the call fail, like Ctrl+C on that one process.
//!
//! The request is a synced `StopTool` command, so it works for remote chats
//! the same as local ones; the host refuses it when no live run can take it.

use std::time::Duration;

use gpui::{
    App, ClickEvent, Div, ElementId, Entity, InteractiveElement as _, ParentElement as _,
    SharedString, Stateful, StatefulInteractiveElement as _, Styled as _, Window, div,
    prelude::FluentBuilder as _, px,
};
use zeron_proto::HarnessId;
use zeron_rpc::methods;

use crate::icons::{self, icon};
use crate::state::AppState;
use crate::theme::{Theme, ink};

/// How long a requested stop reads as "Stopping…" before the button returns:
/// long enough for the agent to report the call killed, short enough that a
/// stop the host could not carry out can be asked for again.
pub(crate) const STOP_PENDING: Duration = Duration::from_secs(10);

/// Harnesses that can end one command or subagent natively. The rest show no
/// stop button: stopping their call would mean ending the whole turn.
pub(crate) fn supports(harness: Option<HarnessId>) -> bool {
    matches!(harness, Some(HarnessId::ClaudeCode | HarnessId::Codex))
}

pub(crate) fn params(chat_id: &str, tool_call_id: &str) -> serde_json::Value {
    serde_json::json!({
        "chatId": chat_id,
        "command": { "kind": "stopTool", "toolCallId": tool_call_id },
    })
}

/// Ask the chat's host to stop `tool_call_id`, and mark the stop pending.
pub(crate) fn request(state: &Entity<AppState>, chat_id: &str, tool_call_id: &str, cx: &mut App) {
    let Some(engine) = state.read(cx).engine().cloned() else {
        return;
    };
    let began = state.update(cx, |state, cx| {
        let began = state.begin_tool_stop(tool_call_id);
        if began {
            cx.notify();
        }
        began
    });
    if !began {
        return;
    }
    let state = state.clone();
    let params = params(chat_id, tool_call_id);
    let tool_call_id = tool_call_id.to_owned();
    cx.spawn(async move |cx| {
        match engine.client().call(methods::QUEUE_COMMAND, params).await {
            Ok(_) => cx.background_executor().timer(STOP_PENDING).await,
            Err(err) => tracing::warn!(tool = %tool_call_id, "stop request failed: {err}"),
        }
        state.update(cx, |state, cx| {
            if state.end_tool_stop(&tool_call_id) {
                cx.notify();
            }
        });
    })
    .detach();
}

/// The small stop tile shared by chips and lane rows. While `pending` it is
/// dimmed and inert, reading "Stopping…". The click never reaches the row or
/// chip underneath, so stopping a call does not also open it.
pub(crate) fn stop_button(
    id: impl Into<SharedString>,
    tooltip: &'static str,
    pending: bool,
    theme: &Theme,
    on_stop: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let id: SharedString = id.into();
    let selector = id.clone();
    div()
        .id(ElementId::from(id))
        .debug_selector(move || selector.to_string())
        .size(px(18.0))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(5.0))
        .child(
            icon(icons::STOP)
                .size(px(11.0))
                .text_color(theme.text_muted),
        )
        .when(pending, |button| {
            button
                .opacity(0.4)
                .tooltip(crate::settings::widgets::text_tooltip("Stopping…"))
        })
        .when(!pending, |button| {
            button
                .cursor_pointer()
                .hover(|style| style.bg(ink(0.08)))
                .tooltip(crate::settings::widgets::text_tooltip(tooltip))
                .on_click(move |event, window, cx| {
                    cx.stop_propagation();
                    on_stop(event, window, cx);
                })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_claude_code_and_codex_stop_single_tools() {
        assert!(supports(Some(HarnessId::ClaudeCode)));
        assert!(supports(Some(HarnessId::Codex)));
        assert!(!supports(Some(HarnessId::Mock)));
        assert!(!supports(None));
    }

    #[test]
    fn the_command_names_the_chat_and_the_tool_part() {
        assert_eq!(
            params("chat-1", "toolu_9"),
            serde_json::json!({
                "chatId": "chat-1",
                "command": { "kind": "stopTool", "toolCallId": "toolu_9" },
            })
        );
        // It is exactly the payload the host decodes.
        let command: zeron_doc::SessionCommandPayload =
            serde_json::from_value(params("chat-1", "toolu_9")["command"].clone()).unwrap();
        assert_eq!(
            command,
            zeron_doc::SessionCommandPayload::StopTool {
                tool_call_id: "toolu_9".into()
            }
        );
    }

    #[test]
    fn a_pending_stop_is_shared_and_asked_once() {
        let mut state = AppState::new();
        assert!(!state.tool_stop_pending("t1"));
        let before = state.tool_stops_revision();
        assert!(state.begin_tool_stop("t1"));
        assert!(!state.begin_tool_stop("t1"));
        assert!(state.tool_stop_pending("t1"));
        assert_eq!(state.tool_stops_revision(), before + 1);
        assert!(state.end_tool_stop("t1"));
        assert!(!state.end_tool_stop("t1"));
        assert!(!state.tool_stop_pending("t1"));
    }
}
