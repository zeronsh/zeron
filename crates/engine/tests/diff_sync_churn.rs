//! Regression coverage for the diff-sync reconcile runaway (idle checkout,
//! back-to-back `git diff` forever).
//!
//! Reconcile runs on every workspace chat row change — including the writes
//! `sync_entry` itself makes — and used to resolve every chat's checkout
//! identity with fresh `git rev-parse` spawns. One transient spawn failure made
//! the chat ungroupable, so its entry (watchers, checksum state, published
//! diff) was torn down and re-added on the next pass, and every re-add kicked a
//! full capture whose row writes triggered the next reconcile. These tests pin
//! the two dampers: memoized identity resolution and the orphan grace period.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use zeron_engine::{CheckoutDiffSync, EngineCore, HarnessRegistry};
use zeron_proto::CheckoutDiff;

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
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Init a repo at `dir` with one committed file and one dirty edit.
async fn init_dirty_repo(dir: &Path) {
    std::fs::create_dir_all(dir).expect("repo dir");
    git(dir, &["init", "-b", "main"]).await;
    std::fs::write(dir.join("a.txt"), "one\ntwo\n").expect("write a.txt");
    git(dir, &["add", "."]).await;
    git(dir, &["commit", "-m", "initial"]).await;
    std::fs::write(dir.join("a.txt"), "one\ntwo\nedited\n").expect("dirty tree");
}

fn assemble(dir: &Path) -> EngineCore {
    std::fs::create_dir_all(dir).expect("data dir");
    EngineCore::assemble(
        dir,
        Arc::new(HarnessRegistry::new()),
        zeron_proto::HarnessId::Mock,
        None,
    )
    .expect("engine assembles")
}

/// Poll `sync`'s watch until a diff for some checkout appears (or panic).
/// Generous deadline: these tests share the machine with real builds.
async fn wait_for_diff(sync: &CheckoutDiffSync) -> CheckoutDiff {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    loop {
        if let Some(diff) = sync.watch_diffs().borrow().first().cloned() {
            return diff;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "diff published before timeout"
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

fn current_diffs(sync: &CheckoutDiffSync) -> Vec<CheckoutDiff> {
    sync.watch_diffs().borrow().clone()
}

/// Block until the workspace chat watch reflects `chat_id`'s presence/absence —
/// row mutations land in the watch asynchronously, and the churn tests need
/// reconcile passes to run against a settled chat list to be deterministic.
async fn wait_chat_state(core: &EngineCore, chat_id: &str, present: bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        let found = core
            .workspace
            .watch_chats()
            .borrow()
            .iter()
            .any(|c| c.id == chat_id);
        if found == present {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "chat watch settled before timeout"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// A transient identity failure (fd exhaustion, EACCES, any failed git spawn)
/// must not tear down a live entry: the memo keeps the chat groupable during
/// chat-watch reconciles, and a failed fresh resolve keeps the memo while the
/// directory still exists. Pre-fix, the first reconcile during the outage
/// removed the entry and the next one re-added it, kicking a fresh capture —
/// the runaway loop.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn entry_survives_transient_identity_failure() {
    use std::os::unix::fs::PermissionsExt as _;

    let tmp = tempfile::tempdir().expect("tempdir");
    let repo_dir = tmp.path().join("repo");
    init_dirty_repo(&repo_dir).await;

    let core = assemble(&tmp.path().join("data"));
    core.workspace
        .create_space(
            "space-1",
            &core.device_id,
            &repo_dir.to_string_lossy(),
            None,
            true,
        )
        .expect("space row");
    core.workspace
        .create_chat("chat-1", Some("space-1"), None, None, None)
        .expect("chat row");
    wait_chat_state(&core, "chat-1", true).await;
    let _interest = core.diff_sync.retain_chat("chat-1");
    core.diff_sync.reconcile_now().await;
    let before = wait_for_diff(&core.diff_sync).await;

    // Simulate the outage: every git spawn against the checkout now fails
    // (chdir EACCES), exactly like fd exhaustion did in the incident logs.
    let live = std::fs::metadata(&repo_dir).expect("meta").permissions();
    let mut dead = live.clone();
    dead.set_mode(0o000);
    std::fs::set_permissions(&repo_dir, dead).expect("chmod 000");

    // Chat-watch reconciles during the outage: memo hit, no git needed.
    core.diff_sync.reconcile_now().await;
    core.diff_sync.reconcile_now().await;
    // Repair-tick reconcile during the outage: fresh resolve fails, but the
    // directory still exists, so the memo (and the entry) must survive.
    core.diff_sync.repair_now().await;

    std::fs::set_permissions(&repo_dir, live).expect("chmod back");
    core.diff_sync.reconcile_now().await;

    // Give any (wrongly) kicked capture time to land, then assert nothing
    // changed: same diff, never dropped, never re-published.
    tokio::time::sleep(Duration::from_secs(3)).await;
    let after = current_diffs(&core.diff_sync);
    assert_eq!(after.len(), 1, "diff must survive the outage");
    assert_eq!(after[0].checkout_id, before.checkout_id);
    assert_eq!(after[0].checksum, before.checksum);
    assert_eq!(
        after[0].updated_at, before.updated_at,
        "entry must not be torn down and re-captured"
    );
    core.shutdown().await;
}

/// A chat-watch emission that briefly misses a chat (row flap) must not tear
/// down the checkout's entry, and the chat coming back must not re-kick a
/// capture. Sustained absence past the grace period must still remove it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_flap_keeps_entry_and_sustained_absence_removes_it() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo_dir = tmp.path().join("repo");
    init_dirty_repo(&repo_dir).await;

    let core = assemble(&tmp.path().join("data"));
    // Standalone sync with a tiny grace so removal is testable; the core's own
    // diff_sync (default grace) also runs but assertions target this instance.
    let sync = CheckoutDiffSync::start_with_orphan_grace(
        core.repos.clone(),
        core.workspace.clone(),
        &core.device_id,
        None,
        Duration::from_millis(300),
    );
    core.workspace
        .create_space(
            "space-1",
            &core.device_id,
            &repo_dir.to_string_lossy(),
            None,
            true,
        )
        .expect("space row");
    core.workspace
        .create_chat("chat-1", Some("space-1"), None, None, None)
        .expect("chat row");
    wait_chat_state(&core, "chat-1", true).await;
    let _interest = sync.retain_chat("chat-1");
    sync.reconcile_now().await;
    let before = wait_for_diff(&sync).await;

    // Flap: the chat vanishes for one reconcile pass...
    core.workspace.delete_chat("chat-1").expect("delete chat");
    wait_chat_state(&core, "chat-1", false).await;
    sync.reconcile_now().await;
    let during = current_diffs(&sync);
    assert_eq!(
        during.len(),
        1,
        "one pass without the chat must only mark the entry, not remove it"
    );

    // ...and comes right back. The surviving entry already knows this chat id,
    // so no capture is kicked and nothing is re-published.
    core.workspace
        .create_chat("chat-1", Some("space-1"), None, None, None)
        .expect("chat row again");
    wait_chat_state(&core, "chat-1", true).await;
    sync.reconcile_now().await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    let after = current_diffs(&sync);
    assert_eq!(after.len(), 1);
    assert_eq!(
        after[0].updated_at, before.updated_at,
        "flap-back must not re-capture or re-publish"
    );

    // Genuine removal still works: absent past the grace on consecutive passes.
    core.workspace.delete_chat("chat-1").expect("delete chat");
    wait_chat_state(&core, "chat-1", false).await;
    sync.reconcile_now().await; // marks orphaned
    tokio::time::sleep(Duration::from_millis(400)).await; // > grace
    sync.reconcile_now().await; // removes
    assert!(
        current_diffs(&sync).is_empty(),
        "sustained absence must remove the entry and its diff"
    );
    core.shutdown().await;
}

/// A checkout whose directory is actually gone must still be evicted: the
/// fresh (repair) resolve drops the memo when the cwd no longer exists, and
/// the grace period then runs out. Guards against the dampers making removal
/// impossible.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deleted_checkout_is_evicted_after_grace() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo_dir = tmp.path().join("repo");
    init_dirty_repo(&repo_dir).await;

    let core = assemble(&tmp.path().join("data"));
    let sync = CheckoutDiffSync::start_with_orphan_grace(
        core.repos.clone(),
        core.workspace.clone(),
        &core.device_id,
        None,
        Duration::from_millis(300),
    );
    core.workspace
        .create_space(
            "space-1",
            &core.device_id,
            &repo_dir.to_string_lossy(),
            None,
            true,
        )
        .expect("space row");
    core.workspace
        .create_chat("chat-1", Some("space-1"), None, None, None)
        .expect("chat row");
    wait_chat_state(&core, "chat-1", true).await;
    let _interest = sync.retain_chat("chat-1");
    sync.reconcile_now().await;
    wait_for_diff(&sync).await;

    // The checkout vanishes while its chat row remains.
    std::fs::remove_dir_all(&repo_dir).expect("remove repo");

    // Chat-watch reconciles keep using the memo (they must not spawn git), so
    // eviction is the repair tick's job: fresh resolve fails + cwd is gone ⇒
    // memo dropped ⇒ chat ungroupable ⇒ orphaned ⇒ removed after grace.
    sync.repair_now().await; // marks orphaned
    tokio::time::sleep(Duration::from_millis(400)).await; // > grace
    sync.repair_now().await; // removes
    assert!(
        current_diffs(&sync).is_empty(),
        "vanished checkout must be evicted once absence outlasts the grace"
    );
    core.shutdown().await;
}

/// Block until the chat watch shows `chat_id` with the given archived flag.
async fn wait_chat_archived(core: &EngineCore, chat_id: &str, archived: bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        let settled = core
            .workspace
            .watch_chats()
            .borrow()
            .iter()
            .any(|c| c.id == chat_id && c.archived == archived);
        if settled {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "chat archive state settled before timeout"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Poll until the checkout under `root` is tracked with at least one live
/// fs watch (watches attach on the blocking pool after the entry is added).
async fn wait_watched(sync: &CheckoutDiffSync, root: &Path) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        if sync
            .tracked_checkouts()
            .iter()
            .any(|(tracked, watches)| tracked == root && *watches > 0)
        {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "checkout watched before timeout"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Old unarchived chats must cost nothing, including on repair passes. Chats
/// sharing a viewed checkout reuse one watcher; closing its last viewer drops
/// the checkout only after the switch-chat grace period.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unopened_chats_stay_idle_and_shared_checkout_lasts_until_last_viewer() {
    let tmp = tempfile::tempdir().unwrap();
    let core = assemble(&tmp.path().join("data"));
    let sync = CheckoutDiffSync::start_with_timings(
        core.repos.clone(),
        core.workspace.clone(),
        &core.device_id,
        None,
        Duration::from_secs(3600),
        Duration::from_millis(300),
    );
    for id in ["active", "old"] {
        let dir = tmp.path().join(id);
        init_dirty_repo(&dir).await;
        core.workspace
            .create_space(id, &core.device_id, &dir.to_string_lossy(), None, true)
            .unwrap();
        core.workspace
            .create_chat(id, Some(id), None, None, None)
            .unwrap();
        wait_chat_state(&core, id, true).await;
    }
    core.workspace
        .create_chat("second", Some("active"), None, None, None)
        .unwrap();
    wait_chat_state(&core, "second", true).await;
    sync.repair_now().await;
    assert!(
        sync.tracked_checkouts().is_empty(),
        "no viewers: no watchers or captures"
    );
    assert!(current_diffs(&sync).is_empty());
    assert_eq!(
        chat_row(&core, "old").checkout_id,
        None,
        "unopened checkout was never resolved"
    );

    let first = sync.retain_chat("active");
    let second = sync.retain_chat("second");
    let root = std::fs::canonicalize(tmp.path().join("active")).unwrap();
    wait_watched(&sync, &root).await;
    wait_for_diff(&sync).await;
    sync.repair_now().await;
    assert_eq!(
        sync.tracked_checkouts().len(),
        1,
        "only the viewed checkout is scanned"
    );
    assert_eq!(chat_row(&core, "old").checkout_id, None);

    drop(first);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        sync.tracked_checkouts().len(),
        1,
        "second chat still viewing this checkout"
    );
    drop(second);
    sync.reconcile_now().await;
    assert_eq!(
        sync.tracked_checkouts().len(),
        1,
        "brief switches retain the checkout"
    );
    eventually("last viewer releases watchers, diffs and statuses", || {
        sync.tracked_checkouts().is_empty()
            && current_diffs(&sync).is_empty()
            && sync.watch_git_statuses().borrow().is_empty()
    })
    .await;
    std::fs::write(root.join("a.txt"), "edited while closed\n").unwrap();
    sync.repair_now().await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(sync.tracked_checkouts().is_empty());
    assert!(
        current_diffs(&sync).is_empty(),
        "edits and repairs cannot reactivate a closed chat"
    );

    let _reopened = sync.retain_chat("active");
    assert!(
        wait_for_diff(&sync)
            .await
            .patch
            .contains("edited while closed")
    );
    sync.shutdown().await;
    core.shutdown().await;
}

fn chat_row(core: &EngineCore, chat_id: &str) -> zeron_proto::Chat {
    core.workspace
        .watch_chats()
        .borrow()
        .iter()
        .find(|c| c.id == chat_id)
        .cloned()
        .expect("chat row")
}

/// Poll until `check` holds or panic with `what`.
async fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    while !check() {
        assert!(tokio::time::Instant::now() < deadline, "{what}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// An archived chat someone has open (an engine-held interest guard — one
/// per transcript stream) keeps full functionality: its checkout is tracked
/// and watched, the live diff is published, and its `checkoutId` is stamped
/// even though it was archived before it ever was a checkout. Once the last
/// viewer leaves and the linger lapses, the checkout is released again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn open_archived_chat_is_tracked_until_interest_lapses() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path().join("folder");
    std::fs::create_dir_all(&dir).unwrap();

    let core = assemble(&tmp.path().join("data"));
    let sync = CheckoutDiffSync::start_with_timings(
        core.repos.clone(),
        core.workspace.clone(),
        &core.device_id,
        None,
        Duration::from_secs(3600),
        Duration::from_millis(300),
    );
    core.workspace
        .create_space(
            "space-1",
            &core.device_id,
            &dir.to_string_lossy(),
            None,
            true,
        )
        .expect("space row");
    core.workspace
        .create_chat("chat-1", Some("space-1"), None, None, None)
        .expect("chat row");
    core.workspace
        .set_chat_archived("chat-1", true)
        .expect("archive");
    wait_chat_archived(&core, "chat-1", true).await;
    // Only now does the folder become a checkout: the archived chat has never
    // been grouped, so nothing has stamped its checkoutId.
    init_dirty_repo(&dir).await;
    let root = std::fs::canonicalize(&dir).unwrap();
    sync.repair_now().await;
    assert!(
        sync.tracked_checkouts().is_empty(),
        "archived and unopened: free"
    );
    assert_eq!(chat_row(&core, "chat-1").checkout_id, None);

    let interest = sync.retain_chat("chat-1");
    wait_watched(&sync, &root).await;
    let diff = wait_for_diff(&sync).await;
    assert_eq!(
        Path::new(&diff.cwd),
        root,
        "the open chat's live diff is published"
    );
    eventually("checkoutId stamped for the open archived chat", || {
        chat_row(&core, "chat-1").checkout_id.as_deref() == Some(diff.checkout_id.as_str())
    })
    .await;

    // Edits while open still flow through the live watch.
    std::fs::write(dir.join("a.txt"), "changed while open\n").unwrap();
    eventually("live diff follows edits", || {
        current_diffs(&sync)
            .first()
            .is_some_and(|d| d.checksum != diff.checksum)
    })
    .await;

    drop(interest);
    eventually("released once the linger lapses", || {
        sync.tracked_checkouts().is_empty() && current_diffs(&sync).is_empty()
    })
    .await;
    core.shutdown().await;
}

/// Discard needs no entry: an archived chat's checkout (nobody watching) is
/// discarded directly, still guarded by the snapshot checksum.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn discard_works_for_an_untracked_checkout() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path().join("repo");
    init_dirty_repo(&dir).await;
    let core = assemble(&tmp.path().join("data"));
    let sync = CheckoutDiffSync::start_with_orphan_grace(
        core.repos.clone(),
        core.workspace.clone(),
        &core.device_id,
        None,
        Duration::from_secs(3600),
    );
    let identity = core.repos.checkout_identity(&dir).await.expect("identity");
    assert!(sync.tracked_checkouts().is_empty());

    let stale = sync
        .discard_working_tree(&identity, "not-the-checksum")
        .await;
    assert!(stale.is_err(), "a stale checksum is refused");
    assert_eq!(
        std::fs::read_to_string(dir.join("a.txt")).unwrap(),
        "one\ntwo\nedited\n"
    );

    let snapshot = zeron_engine::capture_diff(&core.repos, &identity.root)
        .await
        .expect("capture");
    sync.discard_working_tree(&identity, &snapshot.checksum)
        .await
        .expect("discard without an entry");
    assert_eq!(
        std::fs::read_to_string(dir.join("a.txt")).unwrap(),
        "one\ntwo\n"
    );
    core.shutdown().await;
}

/// While a checkout's chats are gone (orphan grace pending), neither repair
/// kicks nor fs churn re-capture it; if the chats come back, the skipped work
/// is caught up at once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn orphaned_entry_skips_captures_and_catches_up_on_return() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path().join("repo");
    init_dirty_repo(&dir).await;
    let core = assemble(&tmp.path().join("data"));
    let sync = CheckoutDiffSync::start_with_orphan_grace(
        core.repos.clone(),
        core.workspace.clone(),
        &core.device_id,
        None,
        Duration::from_secs(3600),
    );
    core.workspace
        .create_space(
            "space-1",
            &core.device_id,
            &dir.to_string_lossy(),
            None,
            true,
        )
        .expect("space row");
    core.workspace
        .create_chat("chat-1", Some("space-1"), None, None, None)
        .expect("chat row");
    wait_chat_state(&core, "chat-1", true).await;
    let _interest = sync.retain_chat("chat-1");
    sync.reconcile_now().await;
    let root = std::fs::canonicalize(&dir).unwrap();
    wait_watched(&sync, &root).await;
    let before = wait_for_diff(&sync).await;

    core.workspace.delete_chat("chat-1").expect("delete chat");
    wait_chat_state(&core, "chat-1", false).await;
    sync.reconcile_now().await; // marks orphaned
    std::fs::write(dir.join("a.txt"), "edited while orphaned\n").unwrap();
    sync.sync_all(); // a repair tick's kick
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(
        current_diffs(&sync)[0].checksum,
        before.checksum,
        "no capture for a chat-less checkout"
    );

    core.workspace
        .create_chat("chat-1", Some("space-1"), None, None, None)
        .expect("chat row again");
    wait_chat_state(&core, "chat-1", true).await;
    sync.reconcile_now().await;
    eventually("skipped capture caught up on return", || {
        current_diffs(&sync)
            .first()
            .is_some_and(|d| d.checksum != before.checksum)
    })
    .await;
    core.shutdown().await;
}
