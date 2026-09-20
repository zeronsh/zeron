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
