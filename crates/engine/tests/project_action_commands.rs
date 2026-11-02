//! Exercise the actual manual/setup runner with commands at the supported limit.
#![cfg(unix)]

use std::time::Duration;
use zeron_engine::project_actions::{
    MAX_PROJECT_ACTION_COMMAND_BYTES, launch_project_action, launch_project_setup_action,
};
use zeron_engine::{ProjectActionsStore, Terminals};
use zeron_proto::{ProjectActionDraft, ProjectActionIcon};

#[tokio::test]
async fn manual_and_setup_actions_preserve_long_multiline_commands() {
    let root = tempfile::tempdir().unwrap();
    let checkout = root.path().join("checkout with spaces");
    std::fs::create_dir(&checkout).unwrap();
    let store = ProjectActionsStore::open(root.path()).unwrap();
    let terminals = Terminals::new();
    for setup in [false, true] {
        let prefix = "printf '%s' '";
        let suffix = "' > payload\nprintf '%s' \"$ZERON_PROJECT_ROOT\" > project-root\nprintf '%s' \"$ZERON_WORKTREE_PATH\" > worktree-path\nprintf '%s' 'quotes: \" $() ` ; é' > literal\n";
        let payload =
            "a".repeat(MAX_PROJECT_ACTION_COMMAND_BYTES - prefix.len() - suffix.trim_end().len());
        let command = format!("{prefix}{payload}{suffix}");
        let snapshot = store
            .upsert(
                "space",
                root.path(),
                None,
                ProjectActionDraft {
                    name: format!("Long command {setup}"),
                    command,
                    icon: ProjectActionIcon::Test,
                    run_on_worktree_create: setup,
                },
            )
            .unwrap();
        let action = snapshot.actions.last().unwrap();
        assert_eq!(action.command.len(), MAX_PROJECT_ACTION_COMMAND_BYTES);
        let run = if setup {
            launch_project_setup_action(&terminals, action, root.path(), &checkout, 80, 24)
        } else {
            launch_project_action(&terminals, action, root.path(), &checkout, 80, 24)
        }
        .unwrap();
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if std::fs::read_to_string(checkout.join("literal"))
                    .ok()
                    .as_deref()
                    == Some("quotes: \" $() ` ; é")
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("the complete command executes");
        assert_eq!(
            std::fs::read(checkout.join("payload")).unwrap(),
            payload.as_bytes()
        );
        assert_eq!(
            std::fs::read_to_string(checkout.join("project-root")).unwrap(),
            root.path().to_str().unwrap()
        );
        assert_eq!(
            std::fs::read_to_string(checkout.join("worktree-path")).unwrap(),
            checkout.to_str().unwrap()
        );
        terminals.close(&run.terminal.id).unwrap();
        for file in ["payload", "project-root", "worktree-path", "literal"] {
            std::fs::remove_file(checkout.join(file)).unwrap();
        }
    }
}

/// Everything an action (any engine terminal) starts carries the terminal's
/// origin, and the terminal's chat is remembered after it closes, so preview
/// discovery still lists a server it left running.
#[tokio::test]
async fn actions_mark_what_they_start_with_their_terminal() {
    let root = tempfile::tempdir().unwrap();
    let store = ProjectActionsStore::open(root.path()).unwrap();
    let terminals = Terminals::new();
    let snapshot = store
        .upsert(
            "space",
            root.path(),
            None,
            ProjectActionDraft {
                name: "Dev server".into(),
                command: "printf '%s' \"$ZERON_ORIGIN\" > origin".into(),
                icon: ProjectActionIcon::Test,
                run_on_worktree_create: false,
            },
        )
        .unwrap();
    let action = snapshot.actions.last().unwrap();
    let run = launch_project_action(&terminals, action, root.path(), root.path(), 80, 24).unwrap();
    terminals.associate_chat(&run.terminal.id, "chat").unwrap();
    let expected = zeron_proto::terminal_origin(&run.terminal.id);
    tokio::time::timeout(Duration::from_secs(10), async {
        while std::fs::read_to_string(root.path().join("origin")).ok() != Some(expected.clone()) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the action sees its terminal's origin");
    terminals.close(&run.terminal.id).unwrap();
    assert_eq!(
        terminals.chats().get(&run.terminal.id).map(String::as_str),
        Some("chat")
    );
}

/// Discovery and runs use the host's chat checkout; every click gets a new PTY.
#[tokio::test]
async fn package_scripts_validate_the_chat_and_open_fresh_terminals() {
    use std::sync::Arc;
    use zeron_engine::{EngineCore, HarnessRegistry};
    use zeron_proto::{PackageScript, ProjectActionRun};
    use zeron_rpc::methods;

    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("project with spaces");
    std::fs::create_dir(&project).unwrap();
    std::fs::write(
        project.join("package.json"),
        r#"{"scripts":{"dev":"echo package-script-test"}}"#,
    )
    .unwrap();
    let core = EngineCore::assemble(
        &root.path().join("engine"),
        Arc::new(HarnessRegistry::new()),
        zeron_proto::HarnessId::Mock,
        None,
    )
    .unwrap();
    core.workspace
        .create_space(
            "space",
            &core.device_id,
            project.to_str().unwrap(),
            None,
            true,
        )
        .unwrap();
    core.workspace
        .create_space(
            "other-space",
            &core.device_id,
            project.to_str().unwrap(),
            None,
            true,
        )
        .unwrap();
    core.workspace
        .create_chat("chat", Some("space"), None, None, None)
        .unwrap();
    core.workspace
        .create_chat_hosted(
            "foreign",
            Some("space"),
            None,
            Some("another-device"),
            None,
            None,
            None,
        )
        .unwrap();
    let client = zeron_rpc::memory_client(core.rpc_service());
    let params = serde_json::json!({"spaceId":"space", "chatId":"chat", "scriptName":"dev", "cols":80, "rows":24});
    let scripts: Vec<PackageScript> = client
        .call_as(methods::LIST_PACKAGE_SCRIPTS, params.clone())
        .await
        .unwrap();
    assert_eq!(scripts.len(), 1);
    assert_eq!(scripts[0].name, "dev");
    for invalid in [
        serde_json::json!({"spaceId":"space", "chatId":"foreign", "scriptName":"dev", "cols":80, "rows":24}),
        serde_json::json!({"spaceId":"other-space", "chatId":"chat", "scriptName":"dev", "cols":80, "rows":24}),
    ] {
        for method in [methods::LIST_PACKAGE_SCRIPTS, methods::RUN_PACKAGE_SCRIPT] {
            assert!(
                client.call(method, invalid.clone()).await.is_err(),
                "{method} must reject {invalid}"
            );
        }
    }
    let first: ProjectActionRun = client
        .call_as(methods::RUN_PACKAGE_SCRIPT, params.clone())
        .await
        .unwrap();
    let second: ProjectActionRun = client
        .call_as(methods::RUN_PACKAGE_SCRIPT, params.clone())
        .await
        .unwrap();
    assert_ne!(first.terminal.id, second.terminal.id);
    assert_eq!(
        std::path::Path::new(&first.terminal.cwd),
        std::fs::canonicalize(&project).unwrap()
    );
    assert_eq!(
        core.terminals
            .chats()
            .get(&first.terminal.id)
            .map(String::as_str),
        Some("chat")
    );
    for run in [first, second] {
        core.terminals.close(&run.terminal.id).unwrap();
    }
    std::fs::write(project.join("package.json"), "{}").unwrap();
    assert!(
        client
            .call(methods::RUN_PACKAGE_SCRIPT, params)
            .await
            .is_err()
    );
}
