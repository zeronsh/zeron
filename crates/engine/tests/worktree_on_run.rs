//! Host-side worktree materialization: a Run command carrying a
//! `WorktreeSpec` creates the isolated worktree on the HOST at drain time
//! (the durable replacement for the composer's old blocking CreateWorktree
//! relay RPC), runs there, and stamps the chat row's cwd + `zeron/<name>`
//! branch. A second spec-carrying Run for the same chat REUSES the checkout
//! instead of minting another.

use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;

use zeron_doc::{MessageRole, MessageStatus, SessionCommandPayload, SessionMessageEntry};
use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_harness::{Harness, HarnessError, RunControls};
use zeron_proto::{
    AgentEvent, DoneStatus, HarnessId, Model, ProjectActionDraft, ProjectActionIcon,
    ReasoningLevel, RunRequest, SandboxLevel, SteeringMode, WorktreeSpec,
};

const CHAT: &str = "chat-worktree-run";

/// Completes a one-line turn and records the cwd each run spawned with.
struct RecordingHarness {
    cwds: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl Harness for RecordingHarness {
    fn id(&self) -> HarnessId {
        HarnessId::Mock
    }
    fn display_name(&self) -> &str {
        "Recorder"
    }
    fn supports_steering(&self) -> bool {
        false
    }
    fn steering_mode(&self) -> SteeringMode {
        SteeringMode::TurnBoundary
    }
    fn reasoning_levels(&self) -> &[ReasoningLevel] {
        &[ReasoningLevel::Medium]
    }
    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        Ok(vec![])
    }
    async fn run(
        &self,
        request: RunRequest,
        _controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        self.cwds.lock().unwrap().push(request.cwd.clone());
        let events: Vec<Result<AgentEvent, HarnessError>> = vec![
            Ok(AgentEvent::SessionStarted {
                harness: HarnessId::Mock,
                model: "mock-1".into(),
                tools: vec![],
                cwd: request.cwd.clone(),
                session_id: "sess-wt".into(),
                assistant_message_id: "a-1".into(),
            }),
            Ok(AgentEvent::TextDelta {
                text: format!("ack: {}", request.prompt),
            }),
            Ok(AgentEvent::Done {
                status: DoneStatus::Completed,
                result: None,
                error: None,
                session_id: Some("sess-wt".into()),
            }),
        ];
        Ok(futures::stream::iter(events).boxed())
    }
}

async fn wait_for<F>(mut predicate: F, what: &str)
where
    F: FnMut() -> bool,
{
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while !predicate() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(15)).await;
    }
}

fn complete_assistant_count(core: &EngineCore) -> usize {
    let entries: Vec<SessionMessageEntry> = core
        .doc_host
        .open(CHAT)
        .ok()
        .and_then(|h| h.doc().read_entries().ok())
        .unwrap_or_default();
    entries
        .iter()
        .filter(|e| e.role == MessageRole::Assistant && e.status == Some(MessageStatus::Complete))
        .count()
}

fn run_payload(message_id: &str, repo_path: &str, space_id: Option<&str>) -> SessionCommandPayload {
    SessionCommandPayload::Run {
        request: RunRequest {
            mcp: None,
            prompt: "isolated please".into(),
            harness: None,
            model: None,
            reasoning: None,
            model_options: Default::default(),
            // Fallback for hosts that predate the spec: the repo's own folder.
            cwd: repo_path.into(),
            sandbox: SandboxLevel::WorkspaceWrite,
            auto_approve: true,
            attachments: Vec::new(),
            resume: None,
            worktree: Some(WorktreeSpec {
                repo_path: repo_path.into(),
                base: "main".into(),
                space_id: space_id.map(str::to_string),
            }),
        },
        message_id: message_id.into(),
    }
}

fn git(cwd: &std::path::Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn run_with_worktree_spec_materializes_on_host_and_reuses() {
    check_worktree_setup_and_reuse(false, true).await;
    #[cfg(unix)]
    check_worktree_setup_and_reuse(true, true).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn worktree_location_supports_native_path_aliases() {
    check_worktree_setup_and_reuse(false, false).await;
}

async fn check_worktree_setup_and_reuse(use_project_symlink: bool, test_setup: bool) {
    let tmp = tempfile::tempdir().unwrap();
    // Canonicalize: git records canonical paths in worktree gitdir links, and
    // macOS tempdirs live behind the /var → /private/var symlink.
    let tmp_path = tmp.path().canonicalize().unwrap();
    let worktrees_root = tmp_path.join("Worktrees con espacios 日本語");

    let repo_dir = tmp_path.join("repo");
    std::fs::create_dir_all(&repo_dir).unwrap();
    git(&repo_dir, &["init", "-b", "main"]);
    git(&repo_dir, &["config", "user.email", "t@example.com"]);
    git(&repo_dir, &["config", "user.name", "Test"]);
    std::fs::write(repo_dir.join("README.md"), "hello\n").unwrap();
    git(&repo_dir, &["add", "."]);
    git(&repo_dir, &["commit", "-m", "init"]);
    std::fs::write(repo_dir.join("local-only.txt"), "keep my original checkout").unwrap();
    let repo_path = repo_dir.to_string_lossy().to_string();
    #[cfg(unix)]
    let project_dir = if use_project_symlink {
        let project_dir = tmp_path.join("project-link");
        std::os::unix::fs::symlink(&repo_dir, &project_dir).unwrap();
        assert_ne!(project_dir, repo_dir);
        assert_eq!(project_dir.canonicalize().unwrap(), repo_dir);
        project_dir
    } else {
        repo_dir.clone()
    };
    #[cfg(not(unix))]
    let project_dir = {
        assert!(!use_project_symlink);
        repo_dir.clone()
    };

    let cwds: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let registry = HarnessRegistry::new();
    registry.register(Arc::new(RecordingHarness { cwds: cwds.clone() }));
    let core = EngineCore::assemble(
        &tmp_path.join("data"),
        Arc::new(registry),
        HarnessId::Mock,
        None,
    )
    .expect("engine core assembles");
    core.workspace
        .create_space(
            "space-worktree-run",
            &core.device_id,
            &project_dir.to_string_lossy(),
            Some("Repo".into()),
            true,
        )
        .expect("create project");
    // Save through the same RPC as the editor: the Space may use an alias
    // while the queued WorktreeSpec carries the canonical repository path.
    let client = zeron_rpc::memory_client(core.rpc_service());
    client
        .call(
            zeron_rpc::methods::SET_WORKTREE_SETTINGS,
            serde_json::json!({
                "useCustomDirectory": true,
                "customDirectory": worktrees_root,
            }),
        )
        .await
        .expect("choose initial worktree directory");
    if test_setup {
        client
        .call(
            zeron_rpc::methods::UPSERT_PROJECT_ACTION,
            serde_json::json!({
                "spaceId": "space-worktree-run",
                "action": ProjectActionDraft {
                    name: "Setup".into(),
                    command: "printf '%s' \"$ZERON_PROJECT_ROOT\" > setup-project-root; printf '%s' \"$ZERON_WORKTREE_PATH\" > setup-worktree-path; printf setup > setup-marker".into(),
                    icon: ProjectActionIcon::Configure,
                    run_on_worktree_create: true,
                }
            }),
        )
        .await
        .expect("save setup Action");
    }

    // Mirror the composer: createChat lands first (cwd-less; the engine
    // resolves the project folder), then the queued Run carries the spec.
    client
        .call(
            zeron_rpc::methods::MUTATE,
            serde_json::json!({
                "op": "createChat",
                "chatId": CHAT,
                "deviceId": core.device_id,
            }),
        )
        .await
        .expect("createChat");
    // Pre-title so the auto-titler's harness request stays out of the flow.
    core.workspace
        .rename_chat(CHAT, "Pre-titled")
        .expect("rename chat");

    let first_command = core
        .doc_host
        .queue_command(
            CHAT,
            run_payload("msg-wt-1", &repo_path, Some("space-worktree-run")),
        )
        .expect("queue run command");
    wait_for(|| complete_assistant_count(&core) == 1, "first turn").await;

    let first_cwd = cwds.lock().unwrap().first().cloned().expect("run recorded");
    assert_ne!(
        first_cwd, repo_path,
        "the run must execute in a fresh worktree, not the repo folder"
    );
    let first = PathBuf::from(&first_cwd);
    assert!(
        first.starts_with(&worktrees_root),
        "worktree lands under the worktrees root: {first_cwd}"
    );
    assert!(
        first.join(".git").is_file(),
        "a linked worktree has a .git FILE"
    );
    let setup = core
        .project_actions
        .take_setup_handoff(&first_command, CHAT)
        .expect("fresh worktree setup handoff");
    assert!(
        setup.setup_error.is_none(),
        "setup failed: {:?}",
        setup.setup_error
    );
    if test_setup {
        assert!(setup.setup_action.is_some());
        wait_for(|| first.join("setup-marker").is_file(), "setup Action").await;
        assert_eq!(
            std::fs::read_to_string(first.join("setup-project-root")).unwrap(),
            repo_path
        );
        assert_eq!(
            std::fs::read_to_string(first.join("setup-worktree-path")).unwrap(),
            first_cwd
        );
        // Reusing this checkout must not execute setup a second time.
        std::fs::remove_file(first.join("setup-marker")).unwrap();
    } else {
        assert!(setup.setup_action.is_none());
    }

    // The chat row follows: cwd repointed at the worktree, branch stamped
    // with the actual zeron/<name> (the composer only knew the base).
    let chat = core
        .workspace
        .chat(CHAT)
        .expect("read chat row")
        .expect("chat row exists");
    assert_eq!(chat.cwd.as_deref(), Some(first_cwd.as_str()));
    let branch = chat.branch.expect("branch stamped");
    assert!(
        branch.starts_with("zeron/"),
        "stamped branch is the worktree's own: {branch}"
    );

    // Changing the destination only affects future worktrees. A duplicate
    // spec-carrying Run (client retry) still REUSES the chat's old checkout.
    let next_root = tmp_path.join("new-worktree-location");
    client
        .call(
            zeron_rpc::methods::SET_WORKTREE_SETTINGS,
            serde_json::json!({
                "useCustomDirectory": true,
                "customDirectory": next_root,
            }),
        )
        .await
        .expect("change destination while the old chat exists");
    // Exercise identity across different spellings, including Windows verbatim
    // versus Git-style paths. Avoid lowercasing names on case-sensitive volumes.
    #[cfg(windows)]
    let retry_path = repo_path
        .strip_prefix(r"\\?\")
        .unwrap_or(&repo_path)
        .replace('\\', "/");
    #[cfg(not(windows))]
    let retry_path = format!("{repo_path}/.");
    let second_command = core
        .doc_host
        .queue_command(
            CHAT,
            run_payload("msg-wt-2", &retry_path, Some("space-worktree-run")),
        )
        .expect("queue second run");
    wait_for(|| complete_assistant_count(&core) == 2, "second turn").await;
    let second_cwd = cwds.lock().unwrap().get(1).cloned().expect("second run");
    assert_eq!(
        second_cwd, first_cwd,
        "a second Run with the spec must reuse the chat's worktree"
    );
    let minted = std::fs::read_dir(worktrees_root.join("repo"))
        .map(|entries| entries.count())
        .unwrap_or(0);
    assert_eq!(minted, 1, "exactly one worktree minted for the chat");
    let reused = core
        .project_actions
        .take_setup_handoff(&second_command, CHAT)
        .expect("reuse completion handoff");
    assert!(reused.setup_action.is_none(), "setup must not run on reuse");
    assert!(reused.setup_error.is_none());
    assert!(!first.join("setup-marker").exists());

    assert_eq!(std::fs::read_dir(&next_root).unwrap().count(), 0);
    // The browser must be able to descend from the canonical saved location.
    let nested =
        zeron_proto::device_paths::child_folder(&next_root.to_string_lossy(), "Carpeta 日本語");
    std::fs::create_dir(&nested).unwrap();
    assert!(std::path::Path::new(&nested).is_dir());
    let listing = core.repos.list_folders(Some(nested.clone())).await.unwrap();
    assert!(same_file::is_same_file(&listing.path, &nested).unwrap());
    let parent = zeron_proto::device_paths::parent_folder(&listing.path).unwrap();
    let listing = core.repos.list_folders(Some(parent)).await.unwrap();
    assert!(
        listing
            .entries
            .iter()
            .any(|entry| entry.name == "Carpeta 日本語" && entry.is_dir)
    );

    let next_chat = "chat-after-location-change";
    client
        .call(
            zeron_rpc::methods::MUTATE,
            serde_json::json!({
                "op": "createChat", "chatId": next_chat, "deviceId": core.device_id,
            }),
        )
        .await
        .unwrap();
    core.workspace
        .rename_chat(next_chat, "Another pre-titled chat")
        .unwrap();
    core.doc_host
        .queue_command(
            next_chat,
            run_payload("new-location-message", &repo_path, None),
        )
        .unwrap();
    wait_for(
        || cwds.lock().unwrap().len() == 3,
        "new chat in the new directory",
    )
    .await;
    let next_cwd = cwds.lock().unwrap()[2].clone();
    assert!(PathBuf::from(&next_cwd).starts_with(&next_root));
    assert_eq!(
        core.workspace.chat(CHAT).unwrap().unwrap().cwd.as_deref(),
        Some(first_cwd.as_str())
    );
    assert_eq!(
        core.workspace
            .chat(next_chat)
            .unwrap()
            .unwrap()
            .cwd
            .as_deref(),
        Some(next_cwd.as_str())
    );
    let refs = core.repos.refs(&repo_dir).await.unwrap();
    for cwd in [&first_cwd, &next_cwd] {
        assert!(
            refs.iter().any(|entry| entry
                .worktree_path
                .as_ref()
                .is_some_and(|path| same_file::is_same_file(path, cwd).unwrap_or(false))),
            "Git selector retains {cwd}"
        );
    }
    assert_eq!(
        std::fs::read_to_string(repo_dir.join("local-only.txt")).unwrap(),
        "keep my original checkout"
    );
    assert_eq!(core.repos.current_branch(&repo_dir).await.unwrap(), "main");
    assert_eq!(core.workspace.read_spaces().unwrap().len(), 1);

    core.shutdown().await;
}

/// Long file paths are supported inside a checkout whose working directory
/// fits Win32's process-start limit, regardless of global Git configuration.
#[cfg(windows)]
#[tokio::test]
async fn worktree_location_supports_long_windows_files() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let repo = root.join("repo");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-b", "main"]);
    git(&repo, &["config", "user.email", "t@example.com"]);
    git(&repo, &["config", "user.name", "Test"]);
    git(&repo, &["config", "core.longpaths", "true"]);
    let relative = std::iter::repeat_n("nested folder 日本語", 18)
        .collect::<PathBuf>()
        .join("file.txt");
    std::fs::create_dir_all(repo.join(&relative).parent().unwrap()).unwrap();
    std::fs::write(repo.join(&relative), "long file").unwrap();
    std::fs::write(repo.join("README.md"), "hello").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-m", "init"]);
    std::fs::write(
        repo.join(".git/hooks/post-checkout"),
        "#!/bin/sh\ntest -f README.md || exit 42\nprintf '%s\\n' \"$1\" \"$2\" \"$3\" >> hook-arguments\n",
    )
    .unwrap();
    git(&repo, &["config", "core.longpaths", "false"]);
    // Exercise a launchable checkout above Git's separate GIT_DIR limit.
    let destination = root.join("a".repeat(210 - root.to_string_lossy().encode_utf16().count()));
    let repos = zeron_engine::Repos::with_worktrees_root(
        &root.join("settings"),
        "test-device",
        root.join("default"),
    );
    repos
        .set_worktree_settings(zeron_proto::WorktreeSettings {
            use_custom_directory: true,
            custom_directory: Some(destination.to_string_lossy().into_owned()),
        })
        .await
        .unwrap();
    let worktree = repos.create_worktree(&repo, "main").await.unwrap();
    let long_file = PathBuf::from(&worktree.path).join(&relative);
    assert!(long_file.to_string_lossy().encode_utf16().count() > 300);
    assert_eq!(std::fs::read_to_string(long_file).unwrap(), "long file");
    let hook_arguments =
        std::fs::read_to_string(PathBuf::from(&worktree.path).join("hook-arguments")).unwrap();
    let head = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&repo)
        .output()
        .unwrap();
    assert!(head.status.success());
    let head = String::from_utf8(head.stdout).unwrap().trim().to_owned();
    assert_eq!(
        hook_arguments.lines().collect::<Vec<_>>(),
        ["0".repeat(head.len()), head, "1".into()]
    );
    assert!(!repo.join("hook-arguments").exists());
    assert_eq!(
        std::fs::read_to_string(PathBuf::from(&worktree.path).join("README.md")).unwrap(),
        "hello"
    );
    assert_eq!(
        repos
            .current_branch(std::path::Path::new(&worktree.path))
            .await
            .unwrap(),
        worktree.branch
    );
    assert!(repos.refs(&repo).await.unwrap().iter().any(|entry| {
        entry
            .worktree_path
            .as_ref()
            .is_some_and(|path| same_file::is_same_file(path, &worktree.path).unwrap_or(false))
    }));
    let config = Command::new("git")
        .args(["config", "--local", "core.longpaths"])
        .current_dir(&repo)
        .output()
        .unwrap();
    assert!(config.status.success());
    assert_eq!(String::from_utf8_lossy(&config.stdout).trim(), "false");
}

#[cfg(windows)]
#[tokio::test]
async fn worktree_location_rejects_unlaunchable_windows_destinations() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let default_root = root.join("default");
    let repos = zeron_engine::Repos::with_worktrees_root(
        &root.join("settings"),
        "test",
        default_root.clone(),
    );
    let mut destination = root.join("other disk");
    while destination.to_string_lossy().encode_utf16().count() < 300 {
        destination.push("long-folder-with-spaces 日本語");
    }
    let previous = repos.worktree_settings();
    let error = repos
        .set_worktree_settings(zeron_proto::WorktreeSettings {
            use_custom_directory: true,
            custom_directory: Some(destination.to_string_lossy().into_owned()),
        })
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("too long to start tools on Windows"),
        "{error}"
    );
    assert!(!destination.exists(), "reject before creating the folder");
    assert_eq!(repos.worktree_settings(), previous);

    // A saved root can fit while root/repository/generated-name exceeds the
    // limit. Validate the final checkout before creating any directory/branch.
    let repo = root.join("repo");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-b", "main"]);
    git(
        &repo,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "--allow-empty",
            "-m",
            "init",
        ],
    );
    let near_limit = root.join("a".repeat(250 - root.to_string_lossy().encode_utf16().count()));
    repos
        .set_worktree_settings(zeron_proto::WorktreeSettings {
            use_custom_directory: true,
            custom_directory: Some(near_limit.to_string_lossy().into_owned()),
        })
        .await
        .unwrap();
    let error = repos.create_worktree(&repo, "main").await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("too long to start tools on Windows"),
        "{error}"
    );
    assert_eq!(std::fs::read_dir(&near_limit).unwrap().count(), 0);
    assert!(
        repos
            .branches(&repo)
            .await
            .unwrap()
            .iter()
            .all(|branch| !branch.starts_with("zeron/"))
    );
}
