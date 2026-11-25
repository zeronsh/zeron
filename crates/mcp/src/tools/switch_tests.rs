//! While Orchestrators is off where the engine is, a chat's agent is offered
//! what it is offered without orchestration: main's tools, by name, and no
//! role instructions.

use std::sync::Arc;

use super::Tools;
use crate::zeron::{Origin, Zeron};

/// The tools upstream main (85f5221da) lists for an agent in a chat whose
/// runtime doesn't wake it on background work (`Tools::list`; its
/// `catalog()`, `plugins::catalog()`, `themes::catalog()`,
/// `commands::catalog()`), in order.
const MAIN_IN_CHAT: &[&str] = &[
    "whoami",
    "list_devices",
    "device_stats",
    "list_projects",
    "list_harnesses",
    "list_models",
    "list_chats",
    "get_chat",
    "create_chat",
    "read_chat",
    "send_message",
    "wait_for_turn",
    "interrupt_chat",
    "respond_to_input",
    "archive_chat",
    "create_chats",
    "send_messages",
    "show_visual",
    "show_image",
    "create_plugin",
    "list_plugins",
    "plugin_status",
    "install_plugin",
    "update_plugin",
    "export_plugin",
    "describe_elements",
    "list_templates",
    "create_template",
    "use_template",
    "template_status",
    "delete_template",
    "describe_app",
    "export_template",
    "import_template",
    "list_themes",
    "get_theme",
    "create_theme",
    "update_theme",
    "delete_theme",
    "use_theme",
    "run_command",
    "command_output",
    "stop_command",
    "list_commands",
    "list_mcp_servers",
    "request_mcp_change",
];

fn tools(orchestration: bool) -> Tools {
    let origin = Origin {
        chat_id: Some("chat-1".into()),
        device_id: Some("dev-1".into()),
        orchestration,
        ..Default::default()
    };
    // (Listing asks the engine nothing: a port nobody serves will do.)
    Tools::new(Arc::new(Zeron::new(1, None, origin)))
}

fn names(tools: &Tools) -> Vec<&'static str> {
    tools.list().into_iter().map(|t| t.name).collect()
}

#[tokio::test]
async fn off_a_chats_tools_are_mains_and_it_gets_no_role_instructions() {
    let off = tools(false);
    off.ensure_role().await;
    assert_eq!(names(&off), MAIN_IN_CHAT);
    assert_eq!(off.role_instructions(), None);
    assert!(!off.has("ask") && !off.has("progress") && !off.has("item_close"));
}

#[tokio::test]
async fn on_every_chat_is_offered_ask_and_progress() {
    let on = tools(true);
    let listed = names(&on);
    assert!(listed.contains(&"ask") && listed.contains(&"progress"), "{listed:?}");
    // The rest by role: a plain chat gets none of them.
    let extra: Vec<&str> = listed.iter().copied().filter(|n| !MAIN_IN_CHAT.contains(n)).collect();
    assert_eq!(extra, ["ask", "progress"]);
}
