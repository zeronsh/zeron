//! Draft checkout conflicts select isolation without modifying the source tree.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use zeron_engine::{EngineCore, HarnessRegistry, Repos};
use zeron_proto::{HarnessId, SwitchRefOutcome};
use zeron_rpc::{RpcClient, methods};

async fn git(cwd: &Path, args: &[&str]) -> String {
    let output = tokio::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .output()
        .await
        .expect("git starts");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

async fn fixture() -> (tempfile::TempDir, PathBuf, EngineCore, RpcClient) {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-b", "main"]).await;
    git(&repo, &["config", "core.autocrlf", "false"]).await;
    std::fs::write(repo.join("tracked.txt"), "original\n").unwrap();
    git(&repo, &["add", "."]).await;
    git(&repo, &["commit", "-m", "Initial"]).await;
    git(&repo, &["checkout", "-b", "feature"]).await;
    std::fs::write(repo.join("tracked.txt"), "feature\n").unwrap();
    std::fs::write(repo.join("new.txt"), "feature file\n").unwrap();
    git(&repo, &["add", "."]).await;
    git(&repo, &["commit", "-m", "Feature"]).await;
    git(&repo, &["checkout", "main"]).await;
    let core = EngineCore::assemble(
        &temp.path().join("data"),
        Arc::new(HarnessRegistry::new()),
        HarnessId::Mock,
        None,
    )
    .unwrap();
    let client = zeron_rpc::memory_client(core.rpc_service());
    (temp, repo, core, client)
}

async fn switch(
    client: &RpcClient,
    repo: &Path,
    reference: &str,
    fallback: bool,
) -> Result<SwitchRefOutcome, zeron_rpc::RpcError> {
    let value = client
        .call(
            methods::SWITCH_REF,
            serde_json::json!({
                "repoPath": repo.to_string_lossy(), "refName": reference,
                "allowWorktreeFallback": fallback,
            }),
        )
        .await?;
    Ok(serde_json::from_value(value).unwrap())
}

#[tokio::test]
async fn switch_ref_worktree_preserves_staged_unstaged_and_untracked_conflicts() {
    for conflict in ["unstaged", "staged", "untracked", "remote-only"] {
        let (temp, repo, core, client) = fixture().await;
        let file = if conflict == "untracked" {
            "new.txt"
        } else {
            "tracked.txt"
        };
        std::fs::write(repo.join(file), "local work\n").unwrap();
        if conflict == "staged" {
            git(&repo, &["add", file]).await;
        }
        if conflict == "remote-only" {
            git(&repo, &["remote", "add", "origin", "."]).await;
            git(
                &repo,
                &["update-ref", "refs/remotes/origin/feature", "feature"],
            )
            .await;
            git(&repo, &["branch", "-D", "feature"]).await;
        }
        let before = git(&repo, &["status", "--porcelain"]).await;
        let index_before = git(&repo, &["ls-files", "--stage"]).await;
        let error = switch(&client, &repo, "feature", false).await.unwrap_err();
        assert!(
            error
                .to_string()
                .contains("would be overwritten by checkout"),
            "{conflict}: {error}"
        );
        let result = switch(&client, &repo, "feature", true).await.unwrap();
        assert!(result.worktree_required, "{conflict}");
        assert!(result.branch.is_none());
        assert_eq!(git(&repo, &["branch", "--show-current"]).await, "main");
        assert_eq!(git(&repo, &["status", "--porcelain"]).await, before);
        assert_eq!(git(&repo, &["ls-files", "--stage"]).await, index_before);
        assert_eq!(
            std::fs::read_to_string(repo.join(file)).unwrap(),
            "local work\n"
        );

        // The draft's NewWorktree plan can materialize the selected base,
        // including a remote-only ref left behind by a failed tracking checkout.
        let repos = Repos::with_worktrees_root(temp.path(), "test", temp.path().join("worktrees"));
        let worktree = repos.create_worktree(&repo, "feature").await.unwrap();
        assert_eq!(
            std::fs::read_to_string(Path::new(&worktree.path).join("tracked.txt")).unwrap(),
            "feature\n"
        );
        assert_eq!(git(&repo, &["status", "--porcelain"]).await, before);
        core.shutdown().await;
    }
}

#[tokio::test]
async fn switch_ref_worktree_preserves_an_unresolved_merge() {
    let (temp, repo, core, client) = fixture().await;
    std::fs::write(repo.join("tracked.txt"), "main change\n").unwrap();
    git(&repo, &["add", "."]).await;
    git(&repo, &["commit", "-m", "Main change"]).await;
    let merge = tokio::process::Command::new("git")
        .args(["merge", "feature"])
        .current_dir(&repo)
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .output()
        .await
        .unwrap();
    assert!(!merge.status.success());
    let before = git(&repo, &["ls-files", "--unmerged"]).await;
    assert!(!before.is_empty());
    let contents = std::fs::read(repo.join("tracked.txt")).unwrap();
    assert!(
        switch(&client, &repo, "feature", true)
            .await
            .unwrap()
            .worktree_required
    );
    let repos = Repos::with_worktrees_root(temp.path(), "test", temp.path().join("worktrees"));
    repos.create_worktree(&repo, "feature").await.unwrap();
    assert_eq!(git(&repo, &["ls-files", "--unmerged"]).await, before);
    assert_eq!(std::fs::read(repo.join("tracked.txt")).unwrap(), contents);
    assert!(repo.join(".git/MERGE_HEAD").exists());
    core.shutdown().await;
}

#[tokio::test]
async fn switch_ref_worktree_handles_a_branch_checked_out_elsewhere() {
    let (temp, repo, core, client) = fixture().await;
    let occupied = temp.path().join("occupied");
    git(
        &repo,
        &["worktree", "add", occupied.to_str().unwrap(), "feature"],
    )
    .await;
    std::fs::write(occupied.join("tracked.txt"), "other work\n").unwrap();
    assert!(
        switch(&client, &repo, "feature", true)
            .await
            .unwrap()
            .worktree_required
    );
    let repos = Repos::with_worktrees_root(temp.path(), "test", temp.path().join("worktrees"));
    repos.create_worktree(&repo, "feature").await.unwrap();
    assert_eq!(
        std::fs::read_to_string(occupied.join("tracked.txt")).unwrap(),
        "other work\n"
    );
    assert_eq!(git(&repo, &["branch", "--show-current"]).await, "main");
    core.shutdown().await;
}

#[tokio::test]
async fn switch_ref_worktree_keeps_normal_switches_and_unrelated_errors() {
    let (_temp, repo, core, client) = fixture().await;
    let error = switch(&client, &repo, "missing", true).await.unwrap_err();
    assert!(error.to_string().contains("pathspec"));
    std::fs::write(repo.join(".git/index.lock"), "locked").unwrap();
    let error = switch(&client, &repo, "feature", true).await.unwrap_err();
    assert!(error.to_string().contains("index.lock"));
    std::fs::remove_file(repo.join(".git/index.lock")).unwrap();
    assert_eq!(git(&repo, &["branch", "--show-current"]).await, "main");
    git(&repo, &["branch", "compatible"]).await;
    std::fs::write(repo.join("tracked.txt"), "safe local edit\n").unwrap();
    let result = switch(&client, &repo, "compatible", true).await.unwrap();
    assert!(!result.worktree_required);
    assert_eq!(result.branch.as_deref(), Some("compatible"));
    assert_eq!(
        std::fs::read_to_string(repo.join("tracked.txt")).unwrap(),
        "safe local edit\n"
    );
    // Legacy success responses still contain exactly the branch field.
    let value = client
        .call(
            methods::SWITCH_REF,
            serde_json::json!({
                "repoPath": repo.to_string_lossy(), "refName": "main",
            }),
        )
        .await
        .unwrap();
    assert_eq!(value, serde_json::json!({ "branch": "main" }));
    core.shutdown().await;
}
