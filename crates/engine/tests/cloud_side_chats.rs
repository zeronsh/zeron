//! On a user's computer, a side chat of a Cloud chat runs on that chat's
//! machine, with or without a project. Written on the computer instead, it
//! would make the computer a delegate host its Cloud parent may dial.
//! Its own test binary: no Cloud session (this is a computer's engine).

use std::sync::Arc;

use zeron_engine::{EngineCore, HarnessId, default_registry};
use zeron_rpc::{memory_client, methods};

#[tokio::test]
async fn a_side_chat_of_a_cloud_chat_runs_on_its_machine_without_a_project() {
    let dir = tempfile::tempdir().unwrap();
    let core =
        EngineCore::assemble(dir.path(), Arc::new(default_registry()), HarnessId::Mock, None)
            .unwrap();
    core.workspace
        .create_chat_hosted(
            "main",
            None,
            Some("cloud-main"),
            Some("cloud-main"),
            None,
            Some("/home/user/app".into()),
            None,
        )
        .unwrap();
    let client = memory_client(core.rpc_service());
    // An agent here asks for a side chat naming this computer (the MCP tools
    // do when there's no project).
    let created: serde_json::Value = client
        .call_as(
            methods::MUTATE,
            serde_json::json!({
                "op": "createChat", "chatId": "side", "parentChatId": "main",
                "deviceId": core.workspace.device_id(),
            }),
        )
        .await
        .unwrap();
    assert_eq!(created["deviceId"], "cloud-main");
    let side = core.workspace.chat("side").unwrap().unwrap();
    assert_eq!(side.device_id, "cloud-main", "the side chat runs on its parent's machine");
    assert_eq!(side.parent_chat_id.as_deref(), Some("main"));
}

#[tokio::test]
async fn deleting_a_cloud_chat_deletes_its_cloud_children() {
    let dir = tempfile::tempdir().unwrap();
    let core =
        EngineCore::assemble(dir.path(), Arc::new(default_registry()), HarnessId::Mock, None)
            .unwrap();
    let chat = |id: &str, host: &str, parent: Option<&str>| {
        core.workspace
            .create_chat_hosted(
                id,
                None,
                Some(host),
                Some(host),
                None,
                Some("/home/user/app".into()),
                parent.map(str::to_owned),
            )
            .unwrap();
    };
    chat("main", "cloud-main", None);
    chat("delegate", "cloud-delegate", Some("main"));
    chat("side", "cloud-main", Some("main"));
    chat("other", "cloud-other", None);
    let client = memory_client(core.rpc_service());
    let _: serde_json::Value = client
        .call_as(
            methods::MUTATE,
            serde_json::json!({ "op": "deleteChat", "chatId": "main" }),
        )
        .await
        .unwrap();
    for gone in ["main", "delegate", "side"] {
        assert!(core.workspace.chat(gone).unwrap().is_none(), "{gone} survived");
    }
    assert!(core.workspace.chat("other").unwrap().is_some(), "another main chat is untouched");
}
