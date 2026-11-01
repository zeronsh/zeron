//! The chat menu's "Restart with all MCP servers": a one-shot launch that
//! skips the lazy MCP plan, for a session whose agent needs a server the
//! plan left asleep. The chat's host arms it and puts a parked runtime to
//! sleep; the next message starts the agent with every server eager.

use super::*;

impl Shell {
    /// Whether the chat menu offers the action for `chat_id`: its host runs
    /// an engine supporting this action, and so does the engine this window talks to
    /// (which forwards a remote host's request).
    pub(super) fn chat_can_restart_with_all_mcp(&self, chat_id: &str, cx: &App) -> bool {
        let capability = zeron_proto::capabilities::MCP_RUNTIME_CONTROLS_V1;
        let state = self.state.read(cx);
        state.chat_host_supports(chat_id, capability)
            && state
                .engine()
                .is_none_or(|engine| engine.engine_info().supports(capability))
    }

    pub(super) fn restart_chat_with_all_mcp(&mut self, chat_id: String, cx: &mut Context<Self>) {
        self.close_chat_menu(cx);
        let state = self.state.read(cx);
        let Some(engine) = state.engine().cloned() else {
            return;
        };
        let Some(host) = state
            .chats
            .iter()
            .find(|chat| chat.id == chat_id)
            .map(|chat| chat.device_id.clone())
        else {
            return;
        };
        let params = serde_json::json!({ "chatId": chat_id, "targetDeviceId": host });
        cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(zeron_rpc::methods::RESTART_CHAT_WITH_ALL_MCP, params)
                .await;
            let _ = this.update(cx, |shell, cx| match result {
                Ok(_) => shell.show_notice(
                    crate::toast::ToastKind::Info,
                    "Every MCP server starts with the next message",
                    cx,
                ),
                Err(err) => shell.show_notice(
                    crate::toast::ToastKind::Error,
                    format!("Couldn\u{2019}t restart with all MCP servers: {err}"),
                    cx,
                ),
            });
        })
        .detach();
    }
}
