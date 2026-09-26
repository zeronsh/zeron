//! End-to-end coverage for the workspace file RPC surface over the real
//! in-memory transport.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_proto::{
    WorkspaceDirectoryPage, WorkspaceFileChanges, WorkspaceFileText, WriteWorkspaceFileOutcome,
};
use zeron_rpc::methods;

async fn git(cwd: &Path, args: &[&str]) {
    let output = tokio::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@test")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@test")
        .output()
        .await
        .expect("git spawns");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

async fn init_repo(path: &Path) {
    std::fs::create_dir_all(path.join("src/nested")).expect("repo tree");
    git(path, &["init", "-b", "main"]).await;
    std::fs::write(path.join("README.md"), "hello\n").expect("readme");
    std::fs::write(path.join("src/lib.rs"), "pub fn answer() -> u8 { 42 }\n").expect("source");
    std::fs::write(path.join("src/nested/mod.rs"), "pub mod child;\n").expect("nested source");
    git(path, &["add", "."]).await;
    git(path, &["commit", "-m", "initial"]).await;
}

fn assemble(data_dir: &Path, device_id: &str) -> EngineCore {
    std::fs::create_dir_all(data_dir).expect("data dir");
    std::fs::write(data_dir.join("device-id"), device_id).expect("device id");
    EngineCore::assemble(
        data_dir,
        Arc::new(HarnessRegistry::new()),
        zeron_proto::HarnessId::Mock,
        None,
    )
    .expect("engine assembles")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workspace_file_rpcs_list_search_read_write_and_watch() {
    let temp = tempfile::tempdir().expect("tempdir");
    let repo = temp.path().join("repo");
    init_repo(&repo).await;
    let core = assemble(&temp.path().join("data"), "device-files");
    core.workspace
        .create_space(
            "space-files",
            &core.device_id,
            &repo.to_string_lossy(),
            None,
            true,
        )
        .expect("space");
    core.workspace
        .create_chat("chat-files", Some("space-files"), None, None, None)
        .expect("chat");
    let client = zeron_rpc::memory_client(core.rpc_service());

    let root = client
        .call(
            methods::LIST_WORKSPACE_DIRECTORY,
            serde_json::json!({ "chatId": "chat-files" }),
        )
        .await
        .expect("list root");
    let root: WorkspaceDirectoryPage = serde_json::from_value(root).expect("typed root");
    assert_eq!(root.entries[0].path, "src");
    assert!(root.entries.iter().any(|entry| entry.path == "README.md"));
    assert!(!root.entries.iter().any(|entry| entry.path == ".git"));

    let nested = client
        .call(
            methods::LIST_WORKSPACE_DIRECTORY,
            serde_json::json!({ "chatId": "chat-files", "directory": "src" }),
        )
        .await
        .expect("list src");
    let nested: WorkspaceDirectoryPage = serde_json::from_value(nested).expect("typed src");
    assert!(
        nested
            .entries
            .iter()
            .any(|entry| entry.path == "src/lib.rs")
    );

    let matches = client
        .call(
            methods::SEARCH_WORKSPACE_FILES,
            serde_json::json!({ "chatId": "chat-files", "query": "lib" }),
        )
        .await
        .expect("search");
    assert_eq!(matches[0]["path"], "src/lib.rs");

    let read = client
        .call(
            methods::READ_WORKSPACE_FILE,
            serde_json::json!({ "chatId": "chat-files", "path": "README.md" }),
        )
        .await
        .expect("read");
    let read: WorkspaceFileText = serde_json::from_value(read).expect("typed read");
    assert_eq!(read.text.as_deref(), Some("hello\n"));
    let original_hash = read.content_hash.expect("content hash");
    let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20"><rect width="20" height="20"/></svg>"#;
    std::fs::write(repo.join("example.svg"), svg).unwrap();
    let image = client.call(methods::READ_WORKSPACE_IMAGE, serde_json::json!({ "chatId": "chat-files", "path": "example.svg", "expectedCheckoutId": read.checkout_id, "offset": 0 })).await.expect("workspace image");
    assert_eq!(image["mimeType"], "image/svg+xml");
    assert_eq!(image["done"], true);
    assert_eq!(image["size"], svg.len());
    for (path, checkout) in [
        ("example.svg", "wrong-checkout"),
        ("../example.svg", read.checkout_id.as_str()),
        (".git/config", read.checkout_id.as_str()),
    ] {
        assert!(client.call(methods::READ_WORKSPACE_IMAGE, serde_json::json!({ "chatId": "chat-files", "path": path, "expectedCheckoutId": checkout, "offset": 0 })).await.is_err());
    }

    let mut watch = client
        .subscribe(
            methods::WATCH_WORKSPACE_FILES,
            serde_json::json!({ "chatId": "chat-files" }),
        )
        .await
        .expect("watch subscribe");
    let baseline = tokio::time::timeout(Duration::from_secs(3), watch.recv())
        .await
        .expect("baseline timeout")
        .expect("watch alive");
    let baseline: WorkspaceFileChanges = serde_json::from_value(baseline).expect("baseline shape");
    assert!(baseline.resync_required);

    let written = client
        .call(
            methods::WRITE_WORKSPACE_FILE,
            serde_json::json!({
                "chatId": "chat-files",
                "path": "README.md",
                "text": "updated\n",
                "expectedCheckoutId": read.checkout_id,
                "expectedContentHash": original_hash,
                "encoding": "utf8",
                "lineEnding": "lf",
            }),
        )
        .await
        .expect("write");
    let written: WriteWorkspaceFileOutcome =
        serde_json::from_value(written).expect("written shape");
    let new_hash = match written {
        WriteWorkspaceFileOutcome::Written { file } => file.content_hash,
        outcome => panic!("unexpected write outcome: {outcome:?}"),
    };
    assert_eq!(
        std::fs::read_to_string(repo.join("README.md")).unwrap(),
        "updated\n"
    );

    let conflict = client
        .call(
            methods::WRITE_WORKSPACE_FILE,
            serde_json::json!({
                "chatId": "chat-files",
                "path": "README.md",
                "text": "stale overwrite\n",
                "expectedCheckoutId": read.checkout_id,
                "expectedContentHash": "stale",
                "encoding": "utf8",
                "lineEnding": "lf",
            }),
        )
        .await
        .expect("conflict response");
    assert_eq!(conflict["status"], "conflict");
    assert_eq!(conflict["currentContentHash"], new_hash);
    assert_eq!(
        std::fs::read_to_string(repo.join("README.md")).unwrap(),
        "updated\n"
    );

    let concurrent_read = client
        .call(
            methods::READ_WORKSPACE_FILE,
            serde_json::json!({ "chatId": "chat-files", "path": "README.md" }),
        )
        .await
        .expect("read before concurrent writers");
    let concurrent_hash = concurrent_read["contentHash"]
        .as_str()
        .expect("concurrent hash")
        .to_string();
    let first_write = client.call(
        methods::WRITE_WORKSPACE_FILE,
        serde_json::json!({
            "chatId": "chat-files",
            "path": "README.md",
            "text": "concurrent\n",
            "expectedCheckoutId": read.checkout_id,
                "expectedContentHash": concurrent_hash.clone(),
            "encoding": "utf8",
            "lineEnding": "lf",
        }),
    );
    let second_write = client.call(
        methods::WRITE_WORKSPACE_FILE,
        serde_json::json!({
            "chatId": "chat-files",
            "path": "README.md",
            "text": "concurrent\n",
            "expectedCheckoutId": read.checkout_id,
                "expectedContentHash": concurrent_hash,
            "encoding": "utf8",
            "lineEnding": "lf",
        }),
    );
    let (first_write, second_write) = tokio::join!(first_write, second_write);
    let statuses = [
        first_write.expect("first concurrent writer")["status"]
            .as_str()
            .expect("first status")
            .to_string(),
        second_write.expect("second concurrent writer")["status"]
            .as_str()
            .expect("second status")
            .to_string(),
    ];
    assert_eq!(
        statuses
            .iter()
            .filter(|status| status.as_str() == "written")
            .count(),
        1
    );
    assert_eq!(
        statuses
            .iter()
            .filter(|status| status.as_str() == "conflict")
            .count(),
        1
    );

    std::fs::write(repo.join("created.txt"), "created\n").expect("external create");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let item = tokio::time::timeout_at(deadline, watch.recv())
            .await
            .expect("watch change timeout")
            .expect("watch alive");
        let item: WorkspaceFileChanges = serde_json::from_value(item).expect("change shape");
        if item
            .changes
            .iter()
            .any(|change| change.path == "created.txt")
        {
            break;
        }
    }

    assert!(
        client
            .call(
                methods::READ_WORKSPACE_FILE,
                serde_json::json!({ "chatId": "chat-files", "path": "../outside" }),
            )
            .await
            .is_err()
    );
    drop(watch);
    core.shutdown().await;
    core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workspace_file_rpcs_preserve_plain_folder_search_support() {
    let temp = tempfile::tempdir().expect("tempdir");
    let folder = temp.path().join("plain-folder");
    std::fs::create_dir_all(&folder).expect("plain folder");
    std::fs::write(folder.join("notes.txt"), "plain\n").expect("plain file");
    let core = assemble(&temp.path().join("plain-data"), "device-plain");
    core.workspace
        .create_space(
            "space-plain",
            &core.device_id,
            &folder.to_string_lossy(),
            None,
            false,
        )
        .expect("plain space");
    let client = zeron_rpc::memory_client(core.rpc_service());

    let legacy = client
        .call(
            methods::SEARCH_FILES,
            serde_json::json!({ "spaceId": "space-plain", "query": "notes" }),
        )
        .await
        .expect("legacy SearchFiles on plain folder");
    assert_eq!(legacy[0]["path"], "notes.txt");
    let listing = client
        .call(
            methods::LIST_WORKSPACE_DIRECTORY,
            serde_json::json!({ "spaceId": "space-plain" }),
        )
        .await
        .expect("workspace listing on plain folder");
    assert!(
        listing["entries"]
            .as_array()
            .is_some_and(|entries| entries.iter().any(|entry| entry["path"] == "notes.txt"))
    );
    core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn projectless_files_use_the_chat_directory_without_a_space() {
    let temp = tempfile::tempdir().expect("tempdir");
    let folder = temp.path().join("home");
    std::fs::create_dir_all(folder.join("notes")).expect("chat directory");
    std::fs::write(folder.join("notes/todo.txt"), "first\n").expect("initial file");
    let core = assemble(&temp.path().join("data"), "device-projectless");
    core.workspace
        .create_chat(
            "chat-projectless",
            None,
            Some(&core.device_id),
            None,
            Some(folder.to_string_lossy().into_owned()),
        )
        .expect("projectless chat");
    core.workspace
        .create_chat(
            "chat-remote",
            None,
            Some("another-device"),
            None,
            Some(folder.to_string_lossy().into_owned()),
        )
        .expect("foreign chat");
    let client = zeron_rpc::memory_client(core.rpc_service());

    let root: WorkspaceDirectoryPage = serde_json::from_value(
        client
            .call(
                methods::LIST_WORKSPACE_DIRECTORY,
                serde_json::json!({ "chatId": "chat-projectless" }),
            )
            .await
            .expect("projectless root"),
    )
    .expect("typed root");
    assert!(root.entries.iter().any(|entry| entry.path == "notes"));

    let matches = client
        .call(
            methods::SEARCH_WORKSPACE_FILES,
            serde_json::json!({ "chatId": "chat-projectless", "query": "todo" }),
        )
        .await
        .expect("projectless search");
    assert_eq!(matches[0]["path"], "notes/todo.txt");

    let read: WorkspaceFileText = serde_json::from_value(
        client
            .call(
                methods::READ_WORKSPACE_FILE,
                serde_json::json!({ "chatId": "chat-projectless", "path": "notes/todo.txt" }),
            )
            .await
            .expect("projectless read"),
    )
    .expect("typed file");
    assert_eq!(read.text.as_deref(), Some("first\n"));
    let outcome = client
        .call(
            methods::WRITE_WORKSPACE_FILE,
            serde_json::json!({
                "chatId": "chat-projectless",
                "path": "notes/todo.txt",
                "text": "second\n",
                "expectedCheckoutId": read.checkout_id,
                "expectedContentHash": read.content_hash,
                "encoding": "utf8",
                "lineEnding": "lf",
            }),
        )
        .await
        .expect("projectless write");
    assert_eq!(outcome["status"], "written");
    assert_eq!(
        std::fs::read_to_string(folder.join("notes/todo.txt")).unwrap(),
        "second\n"
    );

    let mut watch = client
        .subscribe(
            methods::WATCH_WORKSPACE_FILES,
            serde_json::json!({ "chatId": "chat-projectless" }),
        )
        .await
        .expect("projectless watch");
    let baseline = tokio::time::timeout(Duration::from_secs(3), watch.recv())
        .await
        .expect("baseline timeout")
        .expect("watch alive");
    let baseline: WorkspaceFileChanges = serde_json::from_value(baseline).unwrap();
    assert!(baseline.resync_required);

    for params in [
        serde_json::json!({ "chatId": "chat-remote" }),
        serde_json::json!({ "chatId": "chat-projectless", "checkoutPath": "/" }),
    ] {
        assert!(
            client
                .call(methods::LIST_WORKSPACE_DIRECTORY, params)
                .await
                .is_err()
        );
    }
    assert!(
        client
            .call(
                methods::READ_WORKSPACE_FILE,
                serde_json::json!({ "chatId": "chat-projectless", "path": "../outside" }),
            )
            .await
            .is_err()
    );
    drop(watch);
    core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn projectless_files_stay_inside_a_chat_directory_within_a_git_repo() {
    let temp = tempfile::tempdir().expect("tempdir");
    let repo = temp.path().join("repo");
    init_repo(&repo).await;
    let core = assemble(&temp.path().join("data"), "device-projectless-git");
    core.workspace
        .create_chat(
            "chat-projectless",
            None,
            Some(&core.device_id),
            None,
            Some(repo.join("src").to_string_lossy().into_owned()),
        )
        .expect("projectless chat in a repo");
    let client = zeron_rpc::memory_client(core.rpc_service());

    let root: WorkspaceDirectoryPage = serde_json::from_value(
        client
            .call(
                methods::LIST_WORKSPACE_DIRECTORY,
                serde_json::json!({ "chatId": "chat-projectless" }),
            )
            .await
            .expect("list chat directory"),
    )
    .expect("typed root");
    assert!(root.entries.iter().any(|entry| entry.path == "lib.rs"));
    assert!(!root.entries.iter().any(|entry| entry.path == "README.md"));
    assert!(
        client
            .call(
                methods::READ_WORKSPACE_FILE,
                serde_json::json!({ "chatId": "chat-projectless", "path": "../README.md" }),
            )
            .await
            .is_err()
    );
    core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn projectless_home_is_required_for_files_catalogs_and_terminal() {
    const CHILD: &str = "ZERON_MISSING_HOME_FIXTURE";
    if std::env::var_os(CHILD).is_none() {
        let output = tokio::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "projectless_home_is_required_for_files_catalogs_and_terminal",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env_remove("HOME")
            .env_remove("USERPROFILE")
            .output()
            .await
            .expect("isolated missing-home fixture");
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }

    let temp = tempfile::tempdir().unwrap();
    let core = assemble(&temp.path().join("data"), "device-missing-home");
    core.workspace
        .create_chat("chat-missing-home", None, Some(&core.device_id), None, None)
        .unwrap();
    let explicit = temp.path().join("explicit-folder");
    std::fs::create_dir(&explicit).unwrap();
    core.workspace
        .create_chat(
            "chat-explicit-folder",
            None,
            Some(&core.device_id),
            None,
            Some(explicit.to_string_lossy().into_owned()),
        )
        .unwrap();
    let client = zeron_rpc::memory_client(core.rpc_service());
    for (method, params) in [
        (
            methods::LIST_WORKSPACE_DIRECTORY,
            serde_json::json!({ "chatId": "chat-missing-home" }),
        ),
        (methods::LIST_FOLDERS, serde_json::json!({})),
        (
            methods::LIST_COMMANDS,
            serde_json::json!({ "chatId": "chat-missing-home", "harness": "mock" }),
        ),
        (
            methods::OPEN_TERMINAL,
            serde_json::json!({ "chatId": "chat-missing-home", "cols": 80, "rows": 24 }),
        ),
    ] {
        let error = client.call(method, params).await.expect_err(method);
        assert!(
            error
                .to_string()
                .contains("User home directory unavailable"),
            "{method}: {error}"
        );
    }
    client
        .call(
            methods::LIST_WORKSPACE_DIRECTORY,
            serde_json::json!({ "chatId": "chat-explicit-folder" }),
        )
        .await
        .expect("an explicit folder remains usable without a home variable");
    core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn write_rejects_changed_checkout_even_when_contents_match() {
    let temp = tempfile::tempdir().expect("tempdir");
    let repo = temp.path().join("repo");
    let worktree = temp.path().join("worktree");
    init_repo(&repo).await;
    git(
        &repo,
        &["worktree", "add", "-b", "other", worktree.to_str().unwrap()],
    )
    .await;
    let core = assemble(&temp.path().join("data"), "device-files");
    core.workspace
        .create_space(
            "space-files",
            &core.device_id,
            repo.to_str().unwrap(),
            None,
            true,
        )
        .unwrap();
    core.workspace
        .create_chat("chat-files", Some("space-files"), None, None, None)
        .unwrap();
    let client = zeron_rpc::memory_client(core.rpc_service());
    let read = client
        .call(
            methods::READ_WORKSPACE_FILE,
            serde_json::json!({
                "chatId": "chat-files", "path": "README.md"
            }),
        )
        .await
        .unwrap();
    assert!(!read["checkoutId"].as_str().unwrap().is_empty());
    core.workspace
        .set_chat_cwd("chat-files", worktree.to_str().unwrap())
        .unwrap();
    let other = client
        .call(
            methods::READ_WORKSPACE_FILE,
            serde_json::json!({
                "chatId": "chat-files", "path": "README.md"
            }),
        )
        .await
        .unwrap();
    assert_eq!(read["contentHash"], other["contentHash"]);
    assert_ne!(read["checkoutId"], other["checkoutId"]);
    let mut request = serde_json::json!({
        "chatId": "chat-files", "path": "README.md", "text": "pending original edit\n",
        "expectedContentHash": read["contentHash"], "expectedCheckoutId": read["checkoutId"],
        "encoding": "utf8", "lineEnding": "lf"
    });
    let error = client
        .call(methods::WRITE_WORKSPACE_FILE, request.clone())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("Workspace changed"), "{error}");
    request
        .as_object_mut()
        .unwrap()
        .remove("expectedCheckoutId");
    assert!(
        client
            .call(methods::WRITE_WORKSPACE_FILE, request.clone())
            .await
            .is_err()
    );
    assert_eq!(
        std::fs::read_to_string(repo.join("README.md")).unwrap(),
        "hello\n"
    );
    assert_eq!(
        std::fs::read_to_string(worktree.join("README.md")).unwrap(),
        "hello\n"
    );
    // Returning to the original checkout allows the preserved edit to be saved.
    core.workspace
        .set_chat_cwd("chat-files", repo.to_str().unwrap())
        .unwrap();
    request["expectedCheckoutId"] = read["checkoutId"].clone();
    let written = client
        .call(methods::WRITE_WORKSPACE_FILE, request)
        .await
        .unwrap();
    assert_eq!(written["status"], "written");
    assert_eq!(
        std::fs::read_to_string(repo.join("README.md")).unwrap(),
        "pending original edit\n"
    );
    assert_eq!(
        std::fs::read_to_string(worktree.join("README.md")).unwrap(),
        "hello\n"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workspace_mutations_validate_revision_and_publish_semantic_events() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo).await;
    let core = assemble(&temp.path().join("data"), "device-mutations");
    core.workspace
        .create_space(
            "space",
            &core.device_id,
            &repo.to_string_lossy(),
            None,
            true,
        )
        .unwrap();
    core.workspace
        .create_chat("chat", Some("space"), None, None, None)
        .unwrap();
    let client = zeron_rpc::memory_client(core.rpc_service());
    let page: WorkspaceDirectoryPage = serde_json::from_value(
        client
            .call(
                methods::LIST_WORKSPACE_DIRECTORY,
                serde_json::json!({"chatId":"chat"}),
            )
            .await
            .unwrap(),
    )
    .unwrap();
    assert!(page.mutation_capabilities.unwrap().delete_entry);
    let revision = page
        .entries
        .iter()
        .find(|e| e.path == "README.md")
        .unwrap()
        .mutation_revision
        .clone()
        .unwrap();
    let checkout = page.checkout_id.unwrap();
    let mut watch = client
        .subscribe(
            methods::WATCH_WORKSPACE_FILES,
            serde_json::json!({"chatId":"chat"}),
        )
        .await
        .unwrap();
    watch.recv().await.unwrap();
    let mut request = serde_json::json!({"chatId":"chat", "operationId":"rename", "expectedCheckoutId":checkout, "sourcePath":"README.md", "destinationPath":"src/read me.md", "expectedSourceRevision":revision, "expectedKind":"file"});
    request["expectedCheckoutId"] = "wrong".into();
    let rejected = client
        .call(methods::MOVE_WORKSPACE_ENTRY, request.clone())
        .await
        .unwrap();
    assert_eq!(rejected["reason"], "workspaceChanged");
    request["expectedCheckoutId"] = checkout.clone().into();
    let moved = client
        .call(methods::MOVE_WORKSPACE_ENTRY, request)
        .await
        .unwrap();
    assert_eq!(moved["status"], "applied");
    assert!(!repo.join("README.md").exists());
    assert_eq!(
        std::fs::read_to_string(repo.join("src/read me.md")).unwrap(),
        "hello\n"
    );
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let frame: WorkspaceFileChanges =
                serde_json::from_value(watch.recv().await.unwrap()).unwrap();
            if frame.changes.iter().any(|c| {
                c.operation_id.as_deref() == Some("rename")
                    && c.old_path.as_deref() == Some("README.md")
            }) {
                break;
            }
        }
    })
    .await
    .unwrap();
    let deleted = client.call(methods::DELETE_WORKSPACE_ENTRY, serde_json::json!({"chatId":"chat", "operationId":"delete", "expectedCheckoutId":checkout, "path":"src/read me.md", "expectedSourceRevision":moved["entry"]["mutationRevision"], "expectedKind":"file", "recursive":false})).await.unwrap();
    assert_eq!(deleted["status"], "applied");
    assert!(!repo.join("src/read me.md").exists());
}
