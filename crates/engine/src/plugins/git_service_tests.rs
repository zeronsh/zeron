//! Zeron's Git plugin (apps/zgx/plugins-builtin/git): its service, run by
//! this engine as any plugin's is, doing its git work in scratch
//! repositories: what changed (the work tree, a branch, a turn, a commit),
//! staging, discarding, committing, pushing and pulling, branches and
//! history, and failures said with a code the window reads.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::*;

struct Fixture {
    _dir: tempfile::TempDir,
    plugins: Plugins,
    scratch: tempfile::TempDir,
}

async fn call(plugins: &Plugins, method: &str, params: Value) -> Value {
    // Allowing and signing as Zeron's consent window does: with a review.
    let params = super::authority_tests::as_consent_window(plugins, method, params).await;
    match plugins.dispatch(method, params).await {
        Ok(RpcReply::Value(value)) => value,
        Ok(RpcReply::Stream(_)) => panic!("{method} streamed"),
        Err(error) => json!({ "rpc_error": error.to_string() }),
    }
}

/// The Git plugin installed from its folder, signed as yours and allowed.
async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let plugins = Plugins::new(dir.path(), "dev-1");
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../apps/zgx/plugins-builtin/git");
    let to = plugins.plugins_dir().join("git");
    std::fs::create_dir_all(to.join("locales")).unwrap();
    for file in ["zeron-plugin.json", "init.luau", "service.luau", "icon.svg"] {
        if src.join(file).exists() {
            std::fs::copy(src.join(file), to.join(file)).unwrap();
        }
    }
    if !to.join("init.luau").exists() {
        std::fs::write(to.join("init.luau"), "return nil").unwrap();
    }
    std::fs::write(to.join("zeron.luau"), scaffold::SDK).unwrap();
    let signed = call(&plugins, methods::SIGN_PLUGIN, json!({ "id": "git" })).await;
    assert!(signed.get("rpc_error").is_none(), "{signed}");
    let decided = call(&plugins, methods::DECIDE_PLUGIN, json!({ "id": "git", "allow": true, "hash": signed["hash"] })).await;
    assert_eq!(decided["status"], "on", "{decided}");
    // The scratch folder is this device's projects: the service works there alone.
    let scratch = tempfile::tempdir().unwrap();
    let known = scratch.path().display().to_string();
    plugins.set_folders(move || vec![known.clone()]);
    Fixture { _dir: dir, plugins, scratch }
}

impl Fixture {
    async fn ask(&self, method: &str, args: Value) -> Value {
        call(&self.plugins, methods::CALL_PLUGIN_SERVICE, json!({ "plugin": "git", "method": method, "args": args, "timeout": 120 })).await
    }

    /// The answer, or the test fails with the refusal.
    async fn ok(&self, method: &str, args: Value) -> Value {
        let answered = self.ask(method, args).await;
        assert_eq!(answered["ok"], true, "{method}: {answered}");
        answered["value"].clone()
    }

    /// A new repository in the scratch folder, named `name`.
    fn repo(&self, name: &str) -> PathBuf {
        let path = self.scratch.path().join(name);
        std::fs::create_dir_all(&path).unwrap();
        git(&path, &["init", "-q", "-b", "main"]);
        git(&path, &["config", "user.name", "Ada"]);
        git(&path, &["config", "user.email", "ada@example.com"]);
        git(&path, &["config", "commit.gpgsign", "false"]);
        path
    }
}

fn git(cwd: &Path, args: &[&str]) -> String {
    let ran = std::process::Command::new("git").args(args).current_dir(cwd).output().unwrap();
    assert!(ran.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&ran.stderr));
    String::from_utf8_lossy(&ran.stdout).into_owned()
}

fn write(cwd: &Path, path: &str, text: &str) {
    let full = cwd.join(path);
    std::fs::create_dir_all(full.parent().unwrap()).unwrap();
    std::fs::write(full, text).unwrap();
}

fn cwd(p: &Path) -> String {
    p.display().to_string()
}

/// `files` by path.
fn by_path(files: &Value) -> std::collections::HashMap<String, Value> {
    files.as_array().unwrap().iter().map(|f| (f["path"].as_str().unwrap().to_string(), f.clone())).collect()
}

#[tokio::test]
async fn the_work_tree_s_changes_staged_and_not() {
    let f = fixture().await;
    let repo = f.repo("work");
    write(&repo, "a.txt", "one\ntwo\n");
    write(&repo, "src/b.rs", "fn b() {}\n");
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "First"]);
    // A change staged, one not, a new file, a file gone.
    write(&repo, "a.txt", "one\ntwo\nthree\n");
    git(&repo, &["add", "a.txt"]);
    write(&repo, "a.txt", "one\n2\nthree\n");
    write(&repo, "src/c.rs", "fn c() {}\nfn d() {}\n");
    std::fs::remove_file(repo.join("src/b.rs")).unwrap();

    let status = f.ok("status", json!({ "cwd": cwd(&repo) })).await;
    assert_eq!(status["branch"], "main", "{status}");
    assert_eq!(status["changes"], 3, "{status}");
    assert!(status["tree"].as_str().unwrap().len() >= 40, "{status}");
    // The repository's own index is untouched: a.txt still half staged.
    assert_eq!(git(&repo, &["diff", "--cached", "--name-only"]), "a.txt\n");

    let changes = f.ok("changes", json!({ "cwd": cwd(&repo), "scope": "working" })).await;
    let files = by_path(&changes["files"]);
    assert_eq!(files["a.txt"]["stage"], "some", "{changes}");
    assert_eq!((files["a.txt"]["add"].as_i64(), files["a.txt"]["del"].as_i64()), (Some(2), Some(1)));
    assert_eq!(files["src/c.rs"]["status"], "added");
    assert_eq!(files["src/c.rs"]["stage"], "none");
    assert_eq!(files["src/c.rs"]["add"], 2, "a new file's lines are counted");
    assert_eq!(files["src/b.rs"]["status"], "deleted");

    // A file's patch, between the trees the list was read from.
    let diff = f
        .ok("diff", json!({ "cwd": cwd(&repo), "a": changes["a"], "b": changes["b"], "path": "src/c.rs" }))
        .await;
    assert!(diff["text"].as_str().unwrap().contains("+fn d() {}"), "{diff}");

    // The tree id moves as a file does, and stands still otherwise.
    let again = f.ok("status", json!({ "cwd": cwd(&repo) })).await;
    assert_eq!(again["tree"], status["tree"]);
    write(&repo, "src/c.rs", "fn c() {}\n");
    let moved = f.ok("status", json!({ "cwd": cwd(&repo) })).await;
    assert_ne!(moved["tree"], status["tree"]);
}

#[tokio::test]
async fn stage_unstage_discard_and_commit() {
    let f = fixture().await;
    let repo = f.repo("flow");
    write(&repo, "keep.txt", "keep\n");
    write(&repo, "edit.txt", "before\n");
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "First"]);
    write(&repo, "edit.txt", "after\n");
    write(&repo, "new.txt", "new\n");
    write(&repo, "junk.txt", "junk\n");
    let c = cwd(&repo);

    f.ok("stage", json!({ "cwd": c, "paths": ["edit.txt", "new.txt"] })).await;
    let files = by_path(&f.ok("changes", json!({ "cwd": c })).await["files"]);
    assert_eq!((files["edit.txt"]["stage"].as_str(), files["new.txt"]["stage"].as_str()), (Some("all"), Some("all")));
    assert_eq!(files["junk.txt"]["stage"], "none");

    f.ok("unstage", json!({ "cwd": c, "paths": ["new.txt"] })).await;
    let files = by_path(&f.ok("changes", json!({ "cwd": c })).await["files"]);
    assert_eq!(files["new.txt"]["stage"], "none");

    // Discarding: a new file goes, a changed one is as the commit has it.
    f.ok("discard", json!({ "cwd": c, "paths": ["junk.txt"] })).await;
    assert!(!repo.join("junk.txt").exists());
    write(&repo, "keep.txt", "changed\n");
    f.ok("discard", json!({ "cwd": c, "paths": ["keep.txt"] })).await;
    assert_eq!(std::fs::read_to_string(repo.join("keep.txt")).unwrap(), "keep\n");

    // A commit of what's staged, with its message.
    let made = f.ok("commit", json!({ "cwd": c, "message": "Edit the file\n\nWhy it changed." })).await;
    assert_eq!(made["sha"].as_str().unwrap().len(), 40);
    // (A commit-msg hook of this machine's may add a trailer after it.)
    assert!(git(&repo, &["log", "-1", "--format=%B"]).starts_with("Edit the file\n\nWhy it changed."));
    let files = by_path(&f.ok("changes", json!({ "cwd": c })).await["files"]);
    assert_eq!(files.keys().collect::<Vec<_>>(), vec!["new.txt"]);

    // Nothing staged: said so, by its code.
    let refused = f.ask("commit", json!({ "cwd": c, "message": "Again" })).await;
    assert_eq!(refused["reason"], "failed");
    assert!(refused["message"].as_str().unwrap().starts_with("nothing_staged:"), "{refused}");

    // Amending takes the last commit's place.
    f.ok("stage", json!({ "cwd": c, "all": true })).await;
    f.ok("commit", json!({ "cwd": c, "message": "Edit and add", "amend": true })).await;
    assert_eq!(git(&repo, &["rev-list", "--count", "HEAD"]).trim(), "2");
    assert_eq!(git(&repo, &["status", "--porcelain"]), "");
}

#[tokio::test]
async fn branch_turn_and_commit_scopes() {
    let f = fixture().await;
    let repo = f.repo("scopes");
    write(&repo, "a.txt", "a\n");
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "Base"]);
    let c = cwd(&repo);

    // A branch of its own, made here, a commit on it and a change after.
    f.ok("switch", json!({ "cwd": c, "branch": "feature/x", "create": true })).await;
    write(&repo, "b.txt", "b\n");
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "Add b"]);
    write(&repo, "a.txt", "a\nmore\n");

    let status = f.ok("status", json!({ "cwd": c })).await;
    assert_eq!((status["branch"].as_str(), status["base"].as_str()), (Some("feature/x"), Some("main")), "{status}");
    assert_eq!(status["base_ahead"], 1, "{status}");
    assert_eq!(status["pushed"], false);

    // Branch: everything since it left main, the uncommitted change too.
    let branch = f.ok("changes", json!({ "cwd": c, "scope": "branch" })).await;
    let files = by_path(&branch["files"]);
    assert!(files.contains_key("b.txt") && files.contains_key("a.txt"), "{branch}");
    assert_eq!(branch["base"], "main");

    // Last turn: only what changed since the turn started.
    let started = f.ok("turn_start", json!({ "cwd": c, "chat": "chat-1" })).await;
    assert!(started["tree"].is_string(), "{started}");
    write(&repo, "c.txt", "c\n");
    let turn = f.ok("changes", json!({ "cwd": c, "scope": "turn", "chat": "chat-1" })).await;
    assert_eq!(by_path(&turn["files"]).keys().collect::<Vec<_>>(), vec!["c.txt"], "{turn}");
    // A chat with no turn seen: said.
    let none = f.ask("changes", json!({ "cwd": c, "scope": "turn", "chat": "chat-2" })).await;
    assert!(none["message"].as_str().unwrap().starts_with("no_turn:"), "{none}");

    // A commit's own files, and its diff.
    let sha = git(&repo, &["rev-parse", "HEAD"]).trim().to_string();
    let commit = f.ok("changes", json!({ "cwd": c, "scope": "commit", "sha": sha })).await;
    assert_eq!(by_path(&commit["files"]).keys().collect::<Vec<_>>(), vec!["b.txt"]);
    let diff = f.ok("diff", json!({ "cwd": c, "a": commit["a"], "b": commit["b"], "path": "b.txt" })).await;
    assert!(diff["text"].as_str().unwrap().contains("+b"), "{diff}");

    // History: this branch's commits, newest first, with their parents
    // and refs; branches, the current one marked.
    let history = f.ok("history", json!({ "cwd": c })).await;
    let commits = history["commits"].as_array().unwrap();
    assert_eq!(commits.len(), 2, "{history}");
    assert_eq!(commits[0]["subject"], "Add b");
    assert_eq!(commits[0]["parents"][0], commits[1]["sha"]);
    assert!(commits[0]["refs"].to_string().contains("feature/x"), "{history}");
    let branches = f.ok("branches", json!({ "cwd": c })).await;
    let list = branches["branches"].as_array().unwrap();
    assert!(list.iter().any(|b| b["name"] == "feature/x" && b["current"] == true), "{branches}");
    assert!(list.iter().any(|b| b["name"] == "main" && b["current"] == false), "{branches}");

    // Switching with a change in the way: said as `dirty`.
    write(&repo, "b.txt", "changed\n");
    let refused = f.ask("switch", json!({ "cwd": c, "branch": "main" })).await;
    assert!(refused["message"].as_str().unwrap().starts_with("dirty:"), "{refused}");
    git(&repo, &["checkout", "--", "b.txt"]);
    git(&repo, &["checkout", "--", "a.txt"]);
    std::fs::remove_file(repo.join("c.txt")).unwrap();
    f.ok("switch", json!({ "cwd": c, "branch": "main" })).await;
    assert_eq!(git(&repo, &["branch", "--show-current"]).trim(), "main");
    let bad = f.ask("switch", json!({ "cwd": c, "branch": "no..good" })).await;
    assert!(bad["message"].as_str().unwrap().starts_with("bad_name:"), "{bad}");
}

#[tokio::test]
async fn push_publishes_then_a_rejected_push_pulls_and_goes() {
    let f = fixture().await;
    let origin = f.scratch.path().join("origin.git");
    std::fs::create_dir_all(&origin).unwrap();
    git(&origin, &["init", "-q", "--bare", "-b", "main"]);
    let repo = f.repo("mine");
    git(&repo, &["remote", "add", "origin", &cwd(&origin)]);
    write(&repo, "a.txt", "a\n");
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "First"]);
    let c = cwd(&repo);

    // No upstream yet: published, and tracked from then on.
    let before = f.ok("status", json!({ "cwd": c })).await;
    assert_eq!((before["upstream"].is_null(), before["remote"].as_str()), (true, Some("origin")), "{before}");
    f.ok("push", json!({ "cwd": c })).await;
    let after = f.ok("status", json!({ "cwd": c })).await;
    assert_eq!((after["upstream"].as_str(), after["pushed"].as_bool()), (Some("origin/main"), Some(true)), "{after}");

    // Someone else pushes first: ours is rejected, said as `rejected`.
    let theirs = f.scratch.path().join("theirs");
    git(f.scratch.path(), &["clone", "-q", &cwd(&origin), "theirs"]);
    git(&theirs, &["config", "user.name", "Grace"]);
    git(&theirs, &["config", "user.email", "grace@example.com"]);
    write(&theirs, "b.txt", "b\n");
    git(&theirs, &["add", "-A"]);
    git(&theirs, &["commit", "-q", "-m", "Theirs"]);
    git(&theirs, &["push", "-q"]);
    write(&repo, "c.txt", "c\n");
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "Ours"]);
    let ahead = f.ok("status", json!({ "cwd": c })).await;
    assert_eq!((ahead["ahead"].as_i64(), ahead["pushed"].as_bool()), (Some(1), Some(false)), "{ahead}");
    let refused = f.ask("push", json!({ "cwd": c })).await;
    assert!(refused["message"].as_str().unwrap().starts_with("rejected:"), "{refused}");

    // Pull (rebasing ours on theirs), then the push goes.
    f.ok("pull", json!({ "cwd": c })).await;
    f.ok("push", json!({ "cwd": c })).await;
    assert_eq!(git(&repo, &["log", "--format=%s", "-3"]).trim(), "Ours\nTheirs\nFirst");
    let done = f.ok("status", json!({ "cwd": c })).await;
    assert_eq!(done["pushed"], true, "{done}");

    // A pull that would stop on conflicts is undone, and said.
    write(&theirs, "a.txt", "theirs\n");
    git(&theirs, &["commit", "-q", "-am", "Theirs again"]);
    git(&theirs, &["pull", "-q", "--rebase"]);
    git(&theirs, &["push", "-q"]);
    write(&repo, "a.txt", "ours\n");
    git(&repo, &["commit", "-q", "-am", "Ours again"]);
    let conflicted = f.ask("pull", json!({ "cwd": c })).await;
    assert!(conflicted["message"].as_str().unwrap().starts_with("pull_conflicts:"), "{conflicted}");
    let after = f.ok("status", json!({ "cwd": c })).await;
    assert!(after["state"].is_null(), "nothing left half done: {after}");
    assert_eq!(std::fs::read_to_string(repo.join("a.txt")).unwrap(), "ours\n");
}

#[tokio::test]
async fn a_folder_that_isn_t_a_checkout_and_a_merge_in_conflict() {
    let f = fixture().await;
    let plain = f.scratch.path().join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    let refused = f.ask("status", json!({ "cwd": cwd(&plain) })).await;
    assert_eq!(refused["reason"], "failed");
    assert!(refused["message"].as_str().unwrap().starts_with("not_a_repo:"), "{refused}");
    let none = f.ask("status", json!({})).await;
    assert!(none["message"].as_str().unwrap().starts_with("no_checkout:"), "{none}");

    // A repository with no commit yet: its new files, and a first commit.
    let fresh = f.repo("fresh");
    write(&fresh, "first.txt", "hi\n");
    let status = f.ok("status", json!({ "cwd": cwd(&fresh) })).await;
    assert_eq!((status["branch"].as_str(), status["oid"].is_null()), (Some("main"), true), "{status}");
    let changes = f.ok("changes", json!({ "cwd": cwd(&fresh) })).await;
    assert_eq!(by_path(&changes["files"])["first.txt"]["add"], 1, "{changes}");
    f.ok("stage", json!({ "cwd": cwd(&fresh), "all": true })).await;
    f.ok("commit", json!({ "cwd": cwd(&fresh), "message": "First" })).await;

    // Two branches changing one line: a merge stops, in conflict.
    let repo = &fresh;
    git(repo, &["switch", "-q", "-c", "other"]);
    write(repo, "first.txt", "other\n");
    git(repo, &["commit", "-q", "-am", "Other"]);
    git(repo, &["switch", "-q", "main"]);
    write(repo, "first.txt", "main\n");
    git(repo, &["commit", "-q", "-am", "Main"]);
    let _ = std::process::Command::new("git").args(["merge", "-q", "other"]).current_dir(repo).output();
    let status = f.ok("status", json!({ "cwd": cwd(repo) })).await;
    assert_eq!((status["state"].as_str(), status["conflicts"].as_i64()), (Some("merge"), Some(1)), "{status}");
    let files = by_path(&f.ok("changes", json!({ "cwd": cwd(repo) })).await["files"]);
    assert_eq!(files["first.txt"]["status"], "conflict");
}

#[tokio::test]
async fn branch_totals_count_what_the_branch_adds_over_its_base() {
    let f = fixture().await;
    let repo = f.repo("totals");
    write(&repo, "a.txt", "a\n");
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "Base"]);
    let c = cwd(&repo);
    // On its base: nothing added.
    let none = f.ok("branch_totals", json!({ "cwd": c })).await;
    assert_eq!((none["adds"].as_i64(), none["dels"].as_i64()), (Some(0), Some(0)), "{none}");
    // A branch: a commit of its own, and a change not committed yet.
    git(&repo, &["switch", "-q", "-c", "feature"]);
    write(&repo, "b.txt", "one\ntwo\n");
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "Add b"]);
    write(&repo, "a.txt", "changed\n");
    let totals = f.ok("branch_totals", json!({ "cwd": c })).await;
    assert_eq!((totals["base"].as_str(), totals["adds"].as_i64(), totals["dels"].as_i64()), (Some("main"), Some(3), Some(1)), "{totals}");
    // No commit yet: no base to measure from.
    let fresh = f.repo("fresh-totals");
    let empty = f.ok("branch_totals", json!({ "cwd": cwd(&fresh) })).await;
    assert!(empty["adds"].is_null(), "{empty}");
}

#[tokio::test]
async fn history_searches_shows_every_branch_and_a_commit_whole() {
    let f = fixture().await;
    let repo = f.repo("whole");
    write(&repo, "a.txt", "a\n");
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "Base"]);
    git(&repo, &["switch", "-q", "-c", "side"]);
    write(&repo, "b.txt", "b\none\n");
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "Draw the lanes", "-m", "Why: rows need them.\nSecond line."]);
    git(&repo, &["switch", "-q", "main"]);
    let c = cwd(&repo);

    // This branch alone, then every branch.
    let mine = f.ok("history", json!({ "cwd": c })).await;
    assert_eq!(mine["commits"].as_array().unwrap().len(), 1, "{mine}");
    let all = f.ok("history", json!({ "cwd": c, "all": true })).await;
    assert_eq!(all["commits"].as_array().unwrap().len(), 2, "{all}");
    // A search through messages, in any case.
    let found = f.ok("history", json!({ "cwd": c, "all": true, "query": "LANES" })).await;
    let subjects: Vec<&str> = found["commits"].as_array().unwrap().iter().map(|c| c["subject"].as_str().unwrap()).collect();
    assert_eq!(subjects, vec!["Draw the lanes"], "{found}");

    // A commit whole: its body, who, its parents, what it adds.
    let sha = git(&repo, &["rev-parse", "side"]).trim().to_string();
    let info = f.ok("commit_info", json!({ "cwd": c, "sha": sha })).await;
    assert!(info["body"].as_str().unwrap().starts_with("Why: rows need them.\nSecond line."), "{info}");
    assert_eq!((info["author"].as_str(), info["files"].as_i64(), info["adds"].as_i64()), (Some("Ada"), Some(1), Some(2)), "{info}");
    assert_eq!(info["parents"].as_array().unwrap().len(), 1);

    // A branch started at a commit of its own choosing.
    f.ok("switch", json!({ "cwd": c, "branch": "from-side", "create": true, "start": sha })).await;
    assert_eq!(git(&repo, &["rev-parse", "HEAD"]).trim(), sha);
}

/// The Branch scope where real checkouts stand: a linked worktree on a
/// branch of a clone (origin/HEAD set), the default branch itself, a
/// clone whose default is neither main nor master, and a shallow clone.
/// Each answers, or fails with a code the window has words for.
#[tokio::test]
async fn branch_scope_in_worktrees_clones_and_on_the_default_branch() {
    let f = fixture().await;
    let origin = f.repo("origin");
    write(&origin, "a.txt", "a\n");
    git(&origin, &["add", "-A"]);
    git(&origin, &["commit", "-q", "-m", "Base"]);
    let scratch = f.scratch.path();
    git(scratch, &["clone", "-q", origin.to_str().unwrap(), "clone"]);
    let clone = scratch.join("clone");
    git(&clone, &["config", "user.name", "Ada"]);
    git(&clone, &["config", "user.email", "ada@example.com"]);
    git(&clone, &["config", "commit.gpgsign", "false"]);
    let message = |v: &Value| v["message"].as_str().unwrap_or_default().to_string();
    // (An empty list comes back as `{}`.)
    let count = |v: &Value| v.as_array().map(|a| a.len()).or_else(|| v.as_object().map(|o| o.len()));

    // On main itself (the clone's default): nothing ahead of itself.
    let on_main = f.ok("changes", json!({ "cwd": cwd(&clone), "scope": "branch" })).await;
    assert_eq!((on_main["base"].as_str(), count(&on_main["files"])), (Some("origin/main"), Some(0)), "{on_main}");
    // A linked worktree on a branch of its own, a commit on it.
    git(&clone, &["worktree", "add", "-q", "-b", "zeron/feature", scratch.join("wt").to_str().unwrap()]);
    let wt = scratch.join("wt");
    write(&wt, "b.txt", "b\n");
    git(&wt, &["add", "-A"]);
    git(&wt, &["commit", "-q", "-m", "Add b"]);
    let in_wt = f.ok("changes", json!({ "cwd": cwd(&wt), "scope": "branch" })).await;
    assert_eq!((in_wt["base"].as_str(), count(&in_wt["files"])), (Some("origin/main"), Some(1)), "{in_wt}");
    // A base asked for that isn't there: said so, by its name.
    let gone = f.ask("changes", json!({ "cwd": cwd(&wt), "scope": "branch", "base": "release/9" })).await;
    assert_eq!(message(&gone), "no_ref: release/9", "{gone}");
    // origin's head naming a branch that's gone (a default renamed and
    // pruned): the next that's there.
    git(&clone, &["symbolic-ref", "refs/remotes/origin/HEAD", "refs/remotes/origin/master"]);
    let stale = f.ok("changes", json!({ "cwd": cwd(&wt), "scope": "branch" })).await;
    assert_eq!(stale["base"], "origin/main", "{stale}");
    // git's own failure: its first line, cleaned, under the `git` code.
    let bad = f.ask("diff", json!({ "cwd": cwd(&wt), "a": "f".repeat(40), "b": "HEAD", "path": "a.txt" })).await;
    let said = message(&bad);
    assert!(!said.contains("fatal:") && !said.contains("git: fatal"), "{bad}");
    // A repository whose default is "trunk", no remote.
    let trunk = f.repo("trunk");
    git(&trunk, &["checkout", "-q", "-b", "trunk"]);
    write(&trunk, "a.txt", "a\n");
    git(&trunk, &["add", "-A"]);
    git(&trunk, &["commit", "-q", "-m", "Base"]);
    git(&trunk, &["checkout", "-q", "-b", "topic"]);
    let no_default = f.ask("changes", json!({ "cwd": cwd(&trunk), "scope": "branch" })).await;
    assert!(message(&no_default).starts_with("no_base:"), "{no_default}");
    // A repository with no commits yet.
    let empty = f.repo("empty");
    let unborn = f.ask("changes", json!({ "cwd": cwd(&empty), "scope": "branch" })).await;
    assert!(message(&unborn).starts_with("no_base:"), "{unborn}");
    f.ok("status", json!({ "cwd": cwd(&empty) })).await;
    f.ok("changes", json!({ "cwd": cwd(&empty), "scope": "working" })).await;
}

/// The "changed" events the service told this device's windows within
/// `wait`, each one's `cwds`.
async fn changed_within(channel: &mut (impl futures::Stream<Item = Value> + Unpin), wait: std::time::Duration) -> Vec<Value> {
    use futures::StreamExt;
    let mut out = Vec::new();
    let until = tokio::time::Instant::now() + wait;
    while let Ok(Some(event)) = tokio::time::timeout_at(until, channel.next()).await {
        if event["event"] == "changed" {
            out.push(event["data"]["cwds"].clone());
        }
    }
    out
}

#[tokio::test]
async fn a_watched_checkout_says_when_it_moves_and_never_for_its_own_reads() {
    use std::time::Duration;
    let f = fixture().await;
    let repo = f.repo("watched");
    write(&repo, ".gitignore", "target/\n");
    write(&repo, "src/a.rs", "fn a() {}\n");
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "First"]);
    let mut channel = match f.plugins.dispatch(methods::WATCH_PLUGIN_CHANNEL, json!({})).await {
        Ok(RpcReply::Stream(stream)) => stream,
        _ => panic!("no stream"),
    };
    let here = cwd(&repo);
    f.ok("status", json!({ "cwd": here, "watch": true })).await;
    // (As its watch goes live, once: what moved before it was unseen.)
    let first = changed_within(&mut channel, Duration::from_millis(1500)).await;
    assert!(first.len() <= 1, "{first:?}");

    // Its own asking (status, its snapshot's index and objects, changes,
    // history, a diff), again and again: nothing said.
    for _ in 0..4 {
        f.ok("status", json!({ "cwd": here, "watch": true })).await;
        let c = f.ok("changes", json!({ "cwd": here, "scope": "working" })).await;
        f.ok("history", json!({ "cwd": here })).await;
        let _ = f.ask("diff", json!({ "cwd": here, "a": c["a"], "b": c["b"], "path": "src/a.rs" })).await;
    }
    let quiet = changed_within(&mut channel, Duration::from_millis(1200)).await;
    assert!(quiet.is_empty(), "its own reads were said as changes: {quiet:?}");

    // A file written: said once, for this folder.
    write(&repo, "src/a.rs", "fn a() { 1 }\n");
    write(&repo, "src/b.rs", "fn b() {}\n");
    let said = changed_within(&mut channel, Duration::from_millis(1500)).await;
    assert_eq!(said, vec![json!([here])], "{said:?}");

    // A build's output git ignores: nothing (the second time without
    // asking git: its folder is known).
    for i in 0..2 {
        write(&repo, &format!("target/debug/deps/x{i}.o"), "obj");
        let said = changed_within(&mut channel, Duration::from_millis(1000)).await;
        assert!(said.is_empty(), "ignored output was said: {said:?}");
    }

    // A commit made elsewhere (an agent's git): HEAD, the index and the
    // branch moved: said.
    git(&repo, &["add", "-A"]);
    let said = changed_within(&mut channel, Duration::from_millis(1500)).await;
    assert_eq!(said.len(), 1, "staging: {said:?}");
    git(&repo, &["commit", "-q", "-m", "Second"]);
    let said = changed_within(&mut channel, Duration::from_millis(1500)).await;
    assert_eq!(said.len(), 1, "a commit: {said:?}");
    // And asked again after it: still nothing of its own.
    f.ok("status", json!({ "cwd": here, "watch": true })).await;
    f.ok("changes", json!({ "cwd": here, "scope": "working" })).await;
    let quiet = changed_within(&mut channel, Duration::from_millis(1200)).await;
    assert!(quiet.is_empty(), "{quiet:?}");
}

#[tokio::test]
async fn a_linked_worktree_is_told_of_its_own_branch_and_index() {
    use std::time::Duration;
    let f = fixture().await;
    let repo = f.repo("main-tree");
    write(&repo, "a.txt", "a\n");
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "First"]);
    let linked = f.scratch.path().join("linked");
    git(&repo, &["worktree", "add", "-q", "-b", "side", linked.to_str().unwrap()]);
    let mut channel = match f.plugins.dispatch(methods::WATCH_PLUGIN_CHANNEL, json!({})).await {
        Ok(RpcReply::Stream(stream)) => stream,
        _ => panic!("no stream"),
    };
    let here = cwd(&linked);
    f.ok("status", json!({ "cwd": here, "watch": true })).await;
    let _ = changed_within(&mut channel, Duration::from_millis(1500)).await;
    // Its index is in the repository's git folder (worktrees/linked): a
    // file staged there is said.
    write(&linked, "b.txt", "b\n");
    let said = changed_within(&mut channel, Duration::from_millis(1500)).await;
    assert_eq!(said, vec![json!([here])], "a new file: {said:?}");
    git(&linked, &["add", "b.txt"]);
    let said = changed_within(&mut channel, Duration::from_millis(1500)).await;
    assert_eq!(said.len(), 1, "staged in the linked tree: {said:?}");
    // The main tree's index moving isn't its.
    write(&repo, "c.txt", "c\n");
    git(&repo, &["add", "c.txt"]);
    let said = changed_within(&mut channel, Duration::from_millis(1500)).await;
    assert!(said.is_empty(), "another worktree's index: {said:?}");
    // Its branch moved by a commit made in it: said.
    git(&linked, &["commit", "-q", "-m", "Side"]);
    let said = changed_within(&mut channel, Duration::from_millis(1500)).await;
    assert_eq!(said.len(), 1, "{said:?}");
}

/// R20: a caller's values never reach git as options (`a` was put before
/// `--` as it came: `--output=` wrote any file), and git works only in this
/// device's chats' and projects' folders (a planted repository's config is
/// git's to run).
#[tokio::test]
async fn callers_values_never_become_git_options_or_folders() {
    let f = fixture().await;
    let repo = f.repo("work");
    write(&repo, "a.txt", "one\n");
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "First"]);
    let head = git(&repo, &["rev-parse", "HEAD"]).trim().to_owned();
    let outside = tempfile::tempdir().unwrap();
    let target = outside.path().join("authorized_keys");
    for (a, b) in [
        (format!("--output={}", target.display()), head.clone()),
        (head.clone(), format!("--output={}", target.display())),
        ("-p".to_string(), head.clone()),
        ("HEAD~1".to_string(), head.clone()),
    ] {
        let refused = f.ask("diff", json!({ "cwd": cwd(&repo), "a": a, "b": b, "path": "a.txt" })).await;
        assert_ne!(refused["ok"], true, "diff {a} {b}: {refused}");
    }
    assert!(!target.exists(), "git wrote outside the repository");
    for base in ["--output=/tmp/x", "-x", "a..b", "a b"] {
        let refused = f.ask("changes", json!({ "cwd": cwd(&repo), "scope": "branch", "base": base })).await;
        assert_ne!(refused["ok"], true, "base {base}: {refused}");
    }
    let refused = f.ask("commit_info", json!({ "cwd": cwd(&repo), "sha": "--output=/tmp/x" })).await;
    assert_ne!(refused["ok"], true, "{refused}");
    // The same file's diff, asked plainly, still answers.
    write(&repo, "a.txt", "two\n");
    let changes = f.ok("changes", json!({ "cwd": cwd(&repo), "scope": "working" })).await;
    let diff = f.ok("diff", json!({ "cwd": cwd(&repo), "a": changes["a"], "b": changes["b"], "path": "a.txt" })).await;
    assert!(diff["text"].as_str().unwrap().contains("+two"), "{diff}");

    // A repository outside every chat and project: refused before git runs
    // (its fsmonitor would have).
    let planted = outside.path().join("planted");
    std::fs::create_dir_all(&planted).unwrap();
    git(&planted, &["init", "-q"]);
    let ran = outside.path().join("ran");
    let config = planted.join(".git/config");
    let mut text = std::fs::read_to_string(&config).unwrap();
    text.push_str(&format!("[core]\n\tfsmonitor = \"touch {}; false\"\n", ran.display()));
    std::fs::write(&config, text).unwrap();
    for asked in [cwd(&planted), format!("{}/../{}", cwd(&repo), "../planted"), "/".to_string()] {
        let refused = f.ask("status", json!({ "cwd": asked })).await;
        assert_ne!(refused["ok"], true, "{asked}: {refused}");
    }
    assert!(!ran.exists(), "the planted repository's fsmonitor ran");
}

/// A commit made through Git, in a chat's or project's checkout, runs that
/// checkout's hooks as git does for you (a pre-commit that fails stops it);
/// reading (status, changes, diff) runs none, nor the filesystem monitor.
#[tokio::test]
#[cfg(unix)]
async fn a_commit_in_your_checkout_runs_its_hooks_and_reading_runs_none() {
    use std::os::unix::fs::PermissionsExt;
    let f = fixture().await;
    let repo = f.repo("hooked");
    write(&repo, "a.txt", "one\n");
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "First"]);
    let marks = tempfile::tempdir().unwrap();
    let (hook, fsmonitor) = (marks.path().join("pre-commit-ran"), marks.path().join("fsmonitor-ran"));
    let pre_commit = repo.join(".git/hooks/pre-commit");
    std::fs::write(&pre_commit, format!("#!/bin/sh\ntouch {}\n", hook.display())).unwrap();
    std::fs::set_permissions(&pre_commit, std::fs::Permissions::from_mode(0o755)).unwrap();
    git(&repo, &["config", "core.fsmonitor", &format!("touch {}; false", fsmonitor.display())]);

    write(&repo, "a.txt", "two\n");
    f.ok("status", json!({ "cwd": cwd(&repo) })).await;
    let changes = f.ok("changes", json!({ "cwd": cwd(&repo), "scope": "working" })).await;
    f.ok("diff", json!({ "cwd": cwd(&repo), "a": changes["a"], "b": changes["b"], "path": "a.txt" })).await;
    assert!(!hook.exists(), "reading ran a hook");
    f.ok("stage", json!({ "cwd": cwd(&repo), "all": true })).await;
    f.ok("commit", json!({ "cwd": cwd(&repo), "message": "Second" })).await;
    assert!(hook.exists(), "the checkout's pre-commit didn't run");
    assert!(!fsmonitor.exists(), "the filesystem monitor ran");

    // A pre-commit that says no stops the commit, as it would for you.
    std::fs::write(&pre_commit, "#!/bin/sh\necho 'lint failed' >&2\nexit 1\n").unwrap();
    write(&repo, "a.txt", "three\n");
    f.ok("stage", json!({ "cwd": cwd(&repo), "all": true })).await;
    let refused = f.ask("commit", json!({ "cwd": cwd(&repo), "message": "Third" })).await;
    assert_ne!(refused["ok"], true, "{refused}");
    assert_eq!(git(&repo, &["log", "-1", "--format=%s"]).trim(), "Second");
}
