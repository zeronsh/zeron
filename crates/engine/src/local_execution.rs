//! Per-device "Disable local execution" policy (issue #730). The file lives in
//! the device data dir beside `harness-prefs.json`, not in a profile store, so
//! sign-out and profile switches keep it. The engine enforces it for every
//! client: [`crate::rpc`] rejects locally-handled execution methods, and the
//! sessions/terminals funnels refuse to start work that bypasses RPC (the
//! durable command plane, crash auto-resume, worktree setup Actions).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::watch;

use crate::EngineError;

pub const DISABLED_MESSAGE: &str = "Local execution is disabled on this device";

#[derive(serde::Serialize, serde::Deserialize)]
struct PolicyFile {
    #[serde(default)]
    disabled: bool,
}

/// Shared flag; clones observe the same state.
#[derive(Clone)]
pub struct LocalExecution {
    tx: Arc<watch::Sender<bool>>,
    path: Option<PathBuf>,
    /// Admission for async starts (see [`Self::until_disabled`]).
    starts: Arc<tokio::sync::RwLock<()>>,
    /// One policy change at a time, drain included; holds the ticket of the
    /// latest change that ran (see [`Self::transition`]).
    transitions: Arc<tokio::sync::Mutex<u64>>,
    /// Ticket counter: request order, taken before any change is scheduled.
    tickets: Arc<std::sync::atomic::AtomicU64>,
}

impl Default for LocalExecution {
    /// Enabled and unpersisted (bare tests).
    fn default() -> Self {
        Self {
            tx: Arc::new(watch::channel(false).0),
            path: None,
            starts: Arc::default(),
            transitions: Arc::default(),
            tickets: Arc::default(),
        }
    }
}

impl LocalExecution {
    /// A missing file means the default (local execution allowed). A file that
    /// exists but cannot be read or parsed fails closed.
    pub fn load(data_dir: &Path) -> Self {
        let path = data_dir.join("local-execution.json");
        let disabled = match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str::<PolicyFile>(&text)
                .map(|file| file.disabled)
                .unwrap_or_else(|err| {
                    tracing::warn!(error = %err, "local-execution.json unreadable; local execution disabled");
                    true
                }),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => false,
            Err(err) => {
                tracing::warn!(error = %err, "local-execution.json unreadable; local execution disabled");
                true
            }
        };
        // The harness crate's own background starts (login-shell snapshot,
        // managed adapter installs) are process-wide, like this policy.
        zeron_harness::set_local_execution_suspended(disabled);
        Self {
            tx: Arc::new(watch::channel(disabled).0),
            path: Some(path),
            starts: Arc::default(),
            transitions: Arc::default(),
            tickets: Arc::default(),
        }
    }

    pub fn disabled(&self) -> bool {
        *self.tx.borrow()
    }

    /// Cheap early refusal. Not atomic with starting work: process starts
    /// hold [`Self::admit`] instead.
    pub fn check(&self) -> Result<(), EngineError> {
        if self.disabled() {
            return Err(EngineError::Other(DISABLED_MESSAGE.into()));
        }
        Ok(())
    }

    /// Admission lease for starting local work. It holds the policy's read
    /// lock; [`Self::set`] publishes under the write lock, so a disable waits
    /// for every admitted start to spawn and register, and once it returns no
    /// start can be between "checked" and "spawned". Hold it only around the
    /// synchronous spawn + registration: it is `!Send` (the compiler rejects it
    /// across an `.await`), and calling `disabled`/`check` on the same policy
    /// while holding it can deadlock against a waiting disable.
    pub fn admit(&self) -> Result<watch::Ref<'_, bool>, EngineError> {
        let lease = self.tx.borrow();
        if *lease {
            return Err(EngineError::Other(DISABLED_MESSAGE.into()));
        }
        Ok(lease)
    }

    pub fn watch(&self) -> watch::Receiver<bool> {
        self.tx.subscribe()
    }

    /// Resolves once local execution is allowed (immediately by default).
    pub async fn wait_enabled(&self) {
        let _ = self.watch().wait_for(|disabled| !*disabled).await;
    }

    /// Resolves once local execution is disabled; never by default.
    pub async fn wait_disabled(&self) {
        let _ = self.watch().wait_for(|disabled| *disabled).await;
    }

    /// Spawn `command` under the admission lease: refused if disabled, and a
    /// disable cannot complete between this check and the spawn.
    pub fn spawn(
        &self,
        command: &mut tokio::process::Command,
    ) -> Result<tokio::process::Child, EngineError> {
        let _lease = self.admit()?;
        command
            .spawn()
            .map_err(|e| EngineError::Other(format!("spawn failed: {e}")))
    }

    /// Runs async local work whose process start sits behind an `.await`
    /// (agent CLI starts and probes, sign-ins): refused if disabled, dropped
    /// (which kills `kill_on_drop` children) the moment it is, and it holds
    /// the async admission lease meanwhile, so [`Self::quiesce`] returns only
    /// once no such work is left to spawn anything.
    pub async fn until_disabled<T>(
        &self,
        work: impl std::future::Future<Output = T>,
    ) -> Result<T, EngineError> {
        let _start = self.starts.read().await;
        self.check()?;
        tokio::select! {
            result = work => Ok(result),
            _ = self.wait_disabled() => Err(EngineError::Other(DISABLED_MESSAGE.into())),
        }
    }

    /// After a disable has been published: wait until every
    /// [`Self::until_disabled`] work has been dropped. They all race the
    /// policy, so this is prompt; nested leases cannot deadlock because each
    /// holder is already being cancelled.
    /// Request order for policy changes: taken when a request is accepted,
    /// before its change is scheduled, so that [`Self::transition`] can
    /// refuse a change that a later request has already overtaken.
    pub fn ticket(&self) -> u64 {
        self.tickets
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1
    }

    /// Held by a policy change from publish through its drain, so a
    /// re-enable cannot overtake a disable that is still stopping work.
    /// `None` when a later ticket has already run: the caller's change is
    /// stale and must not be applied (the last accepted request wins).
    pub async fn transition(&self, ticket: u64) -> Option<tokio::sync::MutexGuard<'_, u64>> {
        let mut latest = self.transitions.lock().await;
        if *latest >= ticket {
            return None;
        }
        *latest = ticket;
        Some(latest)
    }

    pub async fn quiesce(&self) {
        let _ = self.starts.write().await;
    }

    /// Persist first (atomic temp + rename), then publish: a failed write
    /// leaves the live policy unchanged. Both happen under the channel's
    /// write lock, so concurrent calls cannot leave the file and the live
    /// value disagreeing.
    pub fn set(&self, disabled: bool) -> Result<(), EngineError> {
        let mut result = Ok(());
        self.tx.send_if_modified(|current| {
            result = self.persist(disabled);
            if result.is_ok() {
                *current = disabled;
                if self.path.is_some() {
                    zeron_harness::set_local_execution_suspended(disabled);
                }
            }
            result.is_ok()
        });
        result
    }

    fn persist(&self, disabled: bool) -> Result<(), EngineError> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let json = serde_json::to_string_pretty(&PolicyFile { disabled })
            .map_err(|e| EngineError::Other(e.to_string()))?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json).and_then(|()| std::fs::rename(&tmp, path))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeron_doc::{SessionCommandEntry, SessionCommandPayload, SessionCommandStatus};
    use zeron_proto::{HarnessId, RunRequest, SandboxLevel};
    use zeron_rpc::{RpcError, RpcService, methods};

    fn assemble(dir: &Path) -> crate::EngineCore {
        let registry = crate::HarnessRegistry::new();
        registry.register(Arc::new(zeron_harness::mock::MockHarness {
            script: vec![],
        }));
        crate::EngineCore::assemble(dir, Arc::new(registry), HarnessId::Mock, None).unwrap()
    }

    fn is_disabled_error<T>(result: Result<T, RpcError>) -> bool {
        matches!(result, Err(RpcError::Failed(e)) if e == DISABLED_MESSAGE)
    }

    fn run_request() -> RunRequest {
        RunRequest {
            mcp: None,
            prompt: "hi".into(),
            harness: None,
            model: None,
            reasoning: None,
            model_options: Default::default(),
            cwd: std::env::temp_dir().to_string_lossy().into_owned(),
            sandbox: SandboxLevel::WorkspaceWrite,
            auto_approve: true,
            attachments: Vec::new(),
            resume: None,
            worktree: None,
        }
    }

    #[tokio::test]
    async fn a_change_overtaken_by_a_later_request_is_refused() {
        let policy = LocalExecution::default();
        let older = policy.ticket();
        let newer = policy.ticket();
        // The newer request reaches the transition first: it runs.
        assert!(policy.transition(newer).await.is_some());
        // The older one arrives late: stale, must not run.
        assert!(policy.transition(older).await.is_none());
        // Anything accepted after that still runs, in order.
        let next = policy.ticket();
        assert!(policy.transition(next).await.is_some());
    }

    #[tokio::test]
    async fn policy_gates_local_methods_forwards_remote_ones_and_survives_reassembly() {
        let dir = tempfile::tempdir().unwrap();
        let core = assemble(dir.path());
        let rpc = core.rpc_service();
        // Default off: behavior unchanged.
        assert!(!core.local_execution.disabled());
        rpc.handle(methods::LIST_REPOS, serde_json::json!({}))
            .await
            .unwrap();

        rpc.handle(
            methods::SET_LOCAL_EXECUTION,
            serde_json::json!({ "disabled": true }),
        )
        .await
        .unwrap();
        let cwd = dir.path().to_string_lossy();
        for (method, params) in [
            (methods::LIST_REPOS, serde_json::json!({})),
            (
                methods::OPEN_TERMINAL,
                serde_json::json!({ "chatId": "c", "cols": 80, "rows": 24, "cwd": cwd }),
            ),
            (methods::LIST_FOLDERS, serde_json::json!({ "path": cwd })),
            (
                methods::LIST_MODELS,
                serde_json::json!({ "harness": "mock" }),
            ),
        ] {
            assert!(
                is_disabled_error(rpc.handle(method, params.clone()).await),
                "{method} served locally"
            );
            // Addressed to another device it still routes (and fails only
            // because this test engine has no relay links).
            let mut remote = params;
            remote["targetDeviceId"] = "other-device".into();
            assert!(
                matches!(rpc.handle(method, remote).await, Err(RpcError::Failed(e)) if e.contains("cannot reach device other-device")),
                "{method} not forwarded"
            );
        }
        // Allow-listed methods keep working.
        rpc.handle(methods::LIST_HARNESSES, serde_json::json!({}))
            .await
            .unwrap();
        // The non-RPC funnels refuse too.
        assert!(core.terminals.open(&cwd, 80, 24).is_err());

        core.shutdown().await;
        drop(rpc);
        drop(core);
        let core = assemble(dir.path());
        assert!(core.local_execution.disabled(), "policy lost on restart");
        assert!(is_disabled_error(
            core.rpc_service()
                .handle(methods::LIST_REPOS, serde_json::json!({}))
                .await
        ));
        core.shutdown().await;
    }

    #[test]
    fn concurrent_sets_keep_the_file_and_the_live_value_in_agreement() {
        let dir = tempfile::tempdir().unwrap();
        let policy = LocalExecution::load(dir.path());
        std::thread::scope(|scope| {
            for disabled in [true, false] {
                let policy = policy.clone();
                scope.spawn(move || {
                    for _ in 0..200 {
                        policy.set(disabled).unwrap();
                    }
                });
            }
        });
        assert_eq!(
            LocalExecution::load(dir.path()).disabled(),
            policy.disabled()
        );
    }

    #[tokio::test]
    async fn relay_peers_cannot_change_the_policy() {
        let dir = tempfile::tempdir().unwrap();
        let core = assemble(dir.path());
        core.local_execution.set(true).unwrap();
        let relay = core.relay_rpc_service();
        assert!(
            relay
                .handle(
                    methods::SET_LOCAL_EXECUTION,
                    serde_json::json!({ "disabled": false }),
                )
                .await
                .is_err()
        );
        assert!(core.local_execution.disabled());
        // Peers may still read it.
        let Ok(zeron_rpc::RpcReply::Stream(mut watch)) = relay
            .handle(methods::WATCH_LOCAL_EXECUTION, serde_json::json!({}))
            .await
        else {
            panic!("expected a stream");
        };
        use futures::StreamExt;
        assert_eq!(watch.next().await, Some(serde_json::json!(true)));
        // Relay peers are also local-execution requests to this device.
        assert!(is_disabled_error(
            relay
                .handle(methods::LIST_REPOS, serde_json::json!({}))
                .await
        ));
        core.shutdown().await;
    }

    #[tokio::test]
    async fn queued_commands_for_local_chats_fail_visibly() {
        let dir = tempfile::tempdir().unwrap();
        let core = assemble(dir.path());
        core.local_execution.set(true).unwrap();
        let handle = core.doc_host.open("chat").unwrap();
        handle
            .doc()
            .queue_command(&SessionCommandEntry {
                id: "cmd-run".into(),
                payload: SessionCommandPayload::Run {
                    request: run_request(),
                    message_id: "msg-1".into(),
                },
                issued_by: "viewer-device".into(),
                issued_at: crate::now_ms(),
                based_on: None,
                expires_at: None,
                status: SessionCommandStatus::Pending,
                resolution: None,
            })
            .unwrap();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        let resolved = loop {
            let command = handle
                .doc()
                .read_commands()
                .unwrap()
                .into_iter()
                .find(|c| c.id == "cmd-run")
                .unwrap();
            if command.status != SessionCommandStatus::Pending {
                break command;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "command never resolved"
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        };
        assert_eq!(resolved.status, SessionCommandStatus::Rejected);
        assert_eq!(resolved.resolution.as_deref(), Some(DISABLED_MESSAGE));
        assert!(core.sessions.session_status("chat").is_none());
        // Queue drains and crash auto-resume dispatch through the same funnel.
        assert!(
            core.sessions
                .dispatch("chat", HarnessId::Mock, run_request(), None)
                .await
                .is_err()
        );
        core.shutdown().await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn enabling_reports_running_work_and_stops_it_only_when_asked() {
        let dir = tempfile::tempdir().unwrap();
        let core = assemble(dir.path());
        let rpc = core.rpc_service();
        core.terminals
            .open_with_shell(&dir.path().to_string_lossy(), 80, 24, Some("/bin/sh"))
            .unwrap();
        let reply = |value| match value {
            Ok(zeron_rpc::RpcReply::Value(value)) => value,
            _ => panic!("expected a value reply"),
        };
        let first = reply(
            rpc.handle(
                methods::SET_LOCAL_EXECUTION,
                serde_json::json!({ "disabled": true }),
            )
            .await,
        );
        assert_eq!(first["disabled"], false);
        assert_eq!(first["terminalsOpen"], true);
        assert!(core.terminals.any_open());
        let confirmed = reply(
            rpc.handle(
                methods::SET_LOCAL_EXECUTION,
                serde_json::json!({ "disabled": true, "interrupt": true }),
            )
            .await,
        );
        assert_eq!(confirmed["disabled"], true);
        assert!(!core.terminals.any_open());
        core.shutdown().await;
    }

    /// Runs `scenario` in a child test process whose PATH puts a logging
    /// `git` first (writing a pattern to `hold.on` parks the next matching
    /// git command until `hold.release` exists, which it then consumes) (git is spawned by name from many places; PATH is the one
    /// seam they share, and a child keeps it from leaking into other tests).
    #[cfg(unix)]
    async fn with_logging_git(test: &str, scenario: impl AsyncFnOnce(&Path)) {
        const CHILD: &str = "ZERON_LOCAL_EXECUTION_GIT_FIXTURE";
        if let Some(root) = std::env::var_os(CHILD) {
            return scenario(Path::new(&root)).await;
        }
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let bin = root.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let fake = bin.join("git");
        std::fs::write(
            &fake,
            "#!/bin/sh\n\
             echo \"$*\" >> \"$ZERON_GIT_LOG\"\n\
             if [ -e \"$ZERON_GIT_HOLD.on\" ]; then\n\
               case \"$*\" in *\"$(cat \"$ZERON_GIT_HOLD.on\")\"*)\n\
                 rm -f \"$ZERON_GIT_HOLD.on\"\n\
                 touch \"$ZERON_GIT_HOLD.started\"\n\
                 while [ ! -e \"$ZERON_GIT_HOLD.release\" ]; do sleep 0.02; done\n\
                 rm -f \"$ZERON_GIT_HOLD.release\" ;;\n\
               esac\n\
             fi\n\
             exec \"$ZERON_REAL_GIT\" \"$@\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let real_git = String::from_utf8(
            std::process::Command::new("sh")
                .args(["-c", "command -v git"])
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap();
        let path = std::env::join_paths(
            std::iter::once(bin).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
        )
        .unwrap();
        let output = tokio::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", test, "--nocapture"])
            .env(CHILD, root.path())
            .env("PATH", path)
            .env("ZERON_REAL_GIT", real_git.trim())
            .env("ZERON_GIT_LOG", root.path().join("git.log"))
            .env("ZERON_GIT_HOLD", root.path().join("hold"))
            .output()
            .await
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[cfg(unix)]
    fn git_log(root: &Path) -> String {
        std::fs::read_to_string(root.join("git.log")).unwrap_or_default()
    }

    #[cfg(unix)]
    fn init_repo(root: &Path) -> std::path::PathBuf {
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let real_git = std::env::var("ZERON_REAL_GIT").unwrap();
        for args in [
            &["init", "-q", "-b", "main"][..],
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "init",
            ],
        ] {
            assert!(
                std::process::Command::new(&real_git)
                    .args(args)
                    .current_dir(&repo)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        repo
    }

    async fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        while !done() {
            assert!(tokio::time::Instant::now() < deadline, "timed out: {what}");
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }

    /// Restart with the policy on, then flip it at runtime: with a local
    /// checkout (space + chat) the background git workers stay silent while
    /// disabled and resume when enabled again.
    #[cfg(unix)]
    #[tokio::test]
    async fn background_git_is_silent_while_disabled_and_resumes_after() {
        with_logging_git(
            "local_execution::tests::background_git_is_silent_while_disabled_and_resumes_after",
            async |root| {
                let repo = init_repo(root);
                let data = root.join("engine");
                std::fs::create_dir_all(&data).unwrap();
                std::fs::write(data.join("local-execution.json"), r#"{"disabled":true}"#).unwrap();
                let core = assemble(&data);
                assert!(core.local_execution.disabled());
                let repo_path = repo.to_string_lossy().into_owned();
                core.workspace
                    .create_space("space", &core.device_id, &repo_path, None, true)
                    .unwrap();
                core.workspace
                    .create_chat_with_parent("chat", Some("space"), None, None, None, None)
                    .unwrap();
                let settle = || async {
                    core.diff_sync.repair_now().await;
                    core.spaces_sync.reconcile_now().await;
                    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
                };
                settle().await;
                assert_eq!(git_log(root), "", "git ran on a restart with the policy on");

                core.local_execution.set(false).unwrap();
                wait_until("workers resume", || !git_log(root).is_empty()).await;

                core.local_execution.set(true).unwrap();
                // Let anything already spawned finish, then demand silence.
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                std::fs::write(root.join("git.log"), "").unwrap();
                settle().await;
                assert_eq!(git_log(root), "", "git ran after a runtime disable");
                assert!(core.diff_sync.watch_diffs().borrow().is_empty());
                core.shutdown().await;
            },
        )
        .await;
    }

    /// The durable worktree command racing a disable: the `git worktree add`
    /// already running finishes (the residual window), but nothing after it —
    /// no further git, no agent run — and the command fails visibly.
    #[cfg(unix)]
    #[tokio::test]
    async fn disable_racing_a_worktree_command_stops_after_the_inflight_git() {
        with_logging_git(
            "local_execution::tests::disable_racing_a_worktree_command_stops_after_the_inflight_git",
            async |root| {
                let repo = init_repo(root);
                std::fs::write(root.join("hold.on"), "worktree add").unwrap();
                let core = assemble(&root.join("engine"));
                let handle = core.doc_host.open("chat").unwrap();
                let mut request = run_request();
                request.worktree = Some(zeron_proto::WorktreeSpec {
                    repo_path: repo.to_string_lossy().into_owned(),
                    base: "main".into(),
                    space_id: None,
                });
                handle
                    .doc()
                    .queue_command(&SessionCommandEntry {
                        id: "cmd-wt".into(),
                        payload: SessionCommandPayload::Run {
                            request,
                            message_id: "msg-wt".into(),
                        },
                        issued_by: "viewer-device".into(),
                        issued_at: crate::now_ms(),
                        based_on: None,
                        expires_at: None,
                        status: SessionCommandStatus::Pending,
                        resolution: None,
                    })
                    .unwrap();
                wait_until("worktree add in flight", || {
                    root.join("hold.started").exists()
                })
                .await;
                core.local_execution.set(true).unwrap();
                std::fs::write(root.join("hold.release"), "").unwrap();
                let command = || {
                    handle
                        .doc()
                        .read_commands()
                        .unwrap()
                        .into_iter()
                        .find(|c| c.id == "cmd-wt")
                        .unwrap()
                };
                wait_until("command resolved", || {
                    command().status != SessionCommandStatus::Pending
                })
                .await;
                let resolved = command();
                assert_eq!(resolved.status, SessionCommandStatus::Rejected);
                assert_eq!(resolved.resolution.as_deref(), Some(DISABLED_MESSAGE));
                assert!(core.sessions.session_status("chat").is_none());
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                let log = git_log(root);
                assert!(
                    log.lines().last().unwrap().contains("worktree add"),
                    "git ran after the in-flight worktree add:\n{log}"
                );
                core.shutdown().await;
            },
        )
        .await;
    }

    /// A discard admitted before the disable stops at its next step: the
    /// git command already running (its own status scan, after the checksum
    /// capture passed) finishes, but nothing is restored, cleaned or removed.
    #[cfg(unix)]
    #[tokio::test]
    async fn disable_during_discard_stops_before_any_mutation() {
        with_logging_git(
            "local_execution::tests::disable_during_discard_stops_before_any_mutation",
            async |root| {
                let repo = init_repo(root);
                let real_git = std::env::var("ZERON_REAL_GIT").unwrap();
                std::fs::write(repo.join("tracked.txt"), "one\n").unwrap();
                for args in [
                    &["add", "tracked.txt"][..],
                    &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "add"],
                ] {
                    assert!(
                        std::process::Command::new(&real_git)
                            .args(args)
                            .current_dir(&repo)
                            .status()
                            .unwrap()
                            .success()
                    );
                }
                std::fs::write(repo.join("tracked.txt"), "edited\n").unwrap();
                std::fs::create_dir_all(repo.join("new")).unwrap();
                std::fs::write(repo.join("new/untracked.txt"), "x\n").unwrap();
                let core = assemble(&root.join("engine"));
                let checksum = crate::diff_sync::capture_diff(&core.repos, &repo)
                    .await
                    .unwrap()
                    .checksum;
                std::fs::write(root.join("git.log"), "").unwrap();
                // The capture's status scan comes first: let it through, then
                // park the discard's own scan (the last read before mutating).
                let pattern = "status --porcelain";
                std::fs::write(root.join("hold.on"), pattern).unwrap();
                let discard = tokio::spawn({
                    let repos = core.repos.clone();
                    let repo = repo.clone();
                    async move {
                        crate::diff_sync::discard_working_tree(&repos, &repo, &checksum).await
                    }
                });
                let started = root.join("hold.started");
                wait_until("capture status in flight", || started.exists()).await;
                std::fs::remove_file(&started).unwrap();
                std::fs::write(root.join("hold.on"), pattern).unwrap();
                std::fs::write(root.join("hold.release"), "").unwrap();
                wait_until("discard status in flight", || started.exists()).await;
                core.local_execution.set(true).unwrap();
                std::fs::write(root.join("hold.release"), "").unwrap();
                assert!(discard.await.unwrap().is_err());
                let log = git_log(root);
                assert!(
                    !log.contains("restore") && !log.contains("clean"),
                    "discard mutated after the disable:\n{log}"
                );
                assert_eq!(
                    std::fs::read_to_string(repo.join("tracked.txt")).unwrap(),
                    "edited\n"
                );
                assert!(repo.join("new/untracked.txt").exists());
                core.shutdown().await;
            },
        )
        .await;
    }

    /// An admitted blocked-class call does not outlive the disable: its
    /// reply is the disabled error even while its git command is still held.
    #[cfg(unix)]
    #[tokio::test]
    async fn disable_cancels_an_admitted_rpc() {
        with_logging_git(
            "local_execution::tests::disable_cancels_an_admitted_rpc",
            async |root| {
                let repo = init_repo(root);
                let core = assemble(&root.join("engine"));
                let rpc = core.rpc_service();
                std::fs::write(root.join("hold.on"), "branch").unwrap();
                let call = tokio::spawn({
                    let rpc = rpc.clone();
                    let repo = repo.to_string_lossy().into_owned();
                    async move {
                        rpc.handle(
                            methods::LIST_BRANCHES,
                            serde_json::json!({ "repoPath": repo }),
                        )
                        .await
                    }
                });
                wait_until("git in flight", || root.join("hold.started").exists()).await;
                core.local_execution.set(true).unwrap();
                let reply = tokio::time::timeout(std::time::Duration::from_secs(2), call)
                    .await
                    .expect("the admitted call outlived the disable")
                    .unwrap();
                assert!(is_disabled_error(reply));
                std::fs::write(root.join("hold.release"), "").unwrap();
                core.shutdown().await;
            },
        )
        .await;
    }

    /// A sign-in whose RPC future was dropped (client gone) still dies with
    /// the disable: the flow is registered, so the disable's drain reaches it.
    #[cfg(unix)]
    #[tokio::test]
    async fn disable_kills_a_login_whose_request_was_dropped() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let core = assemble(&dir.path().join("engine"));
        let pid_file = dir.path().join("pid");
        let cli = dir.path().join("grok");
        std::fs::write(
            &cli,
            format!(
                "#!/bin/sh\necho $$ > '{}'\nexec sleep 30\n",
                pid_file.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o755)).unwrap();
        core.agent_accounts.override_cli(HarnessId::Grok, &cli);
        let rpc = core.rpc_service();
        let request = tokio::spawn({
            let rpc = rpc.clone();
            async move {
                rpc.handle(
                    methods::START_AGENT_LOGIN,
                    serde_json::json!({ "harness": "grok" }),
                )
                .await
            }
        });
        wait_until("login child running", || {
            std::fs::read_to_string(&pid_file).is_ok_and(|pid| !pid.trim().is_empty())
        })
        .await;
        request.abort();
        let pid: i32 = std::fs::read_to_string(&pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        rpc.handle(
            methods::SET_LOCAL_EXECUTION,
            serde_json::json!({ "disabled": true }),
        )
        .await
        .unwrap();
        wait_until("login child killed", || unsafe { libc::kill(pid, 0) } != 0).await;
        core.shutdown().await;
    }

    /// The admission lease is atomic with a disable: a start holding it
    /// (between its check and its spawn) makes the disable wait, and once
    /// the disable returns every start is refused. Async starts are dropped
    /// and `quiesce` returns only after they are gone.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn disable_waits_for_admitted_starts() {
        let policy = LocalExecution::default();
        let lease = policy.admit().unwrap();
        let disable = std::thread::spawn({
            let policy = policy.clone();
            move || policy.set(true).unwrap()
        });
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(
            !disable.is_finished(),
            "disable returned while a start was admitted"
        );
        drop(lease);
        disable.join().unwrap();
        assert!(policy.admit().is_err());

        let policy = LocalExecution::default();
        let started = std::sync::Arc::new(tokio::sync::Notify::new());
        let work = tokio::spawn({
            let policy = policy.clone();
            let started = started.clone();
            async move {
                policy
                    .until_disabled(async {
                        started.notify_one();
                        std::future::pending::<()>().await
                    })
                    .await
            }
        });
        started.notified().await;
        policy.set(true).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), policy.quiesce())
            .await
            .unwrap();
        assert!(work.is_finished() || work.await.unwrap().is_err());
        assert!(policy.until_disabled(async {}).await.is_err());
    }

    /// A capture whose first git command is in flight when local execution is
    /// disabled stops there: no further git, and the capture fails.
    #[cfg(unix)]
    #[tokio::test]
    async fn disable_during_capture_spawns_no_further_git() {
        with_logging_git(
            "local_execution::tests::disable_during_capture_spawns_no_further_git",
            async |root| {
                let repo = init_repo(root);
                let core = assemble(&root.join("engine"));
                std::fs::write(root.join("git.log"), "").unwrap();
                std::fs::write(root.join("hold.on"), "rev-parse --verify HEAD").unwrap();
                let capture = tokio::spawn({
                    let repos = core.repos.clone();
                    let repo = repo.clone();
                    async move { crate::diff_sync::capture_diff(&repos, &repo).await }
                });
                wait_until("first capture git in flight", || {
                    root.join("hold.started").exists()
                })
                .await;
                core.local_execution.set(true).unwrap();
                std::fs::write(root.join("hold.release"), "").unwrap();
                assert!(capture.await.unwrap().is_err());
                let log = git_log(root);
                assert_eq!(log.lines().count(), 1, "git ran after the disable:\n{log}");
                core.shutdown().await;
            },
        )
        .await;
    }

    /// A restart with local execution disabled spawns no login shell (the
    /// PATH-snapshot prewarm and lazy captures are held), and enabling
    /// releases it.
    #[cfg(unix)]
    #[tokio::test]
    async fn restart_with_policy_on_spawns_no_login_shell() {
        const CHILD: &str = "ZERON_LOCAL_EXECUTION_SHELL_FIXTURE";
        if let Some(root) = std::env::var_os(CHILD) {
            let root = Path::new(&root);
            let data = root.join("engine");
            std::fs::create_dir_all(&data).unwrap();
            std::fs::write(data.join("local-execution.json"), r#"{"disabled":true}"#).unwrap();
            let policy = LocalExecution::load(&data);
            zeron_harness::shell_env::prewarm();
            assert_eq!(zeron_harness::shell_env::login_shell_path(), None);
            std::thread::sleep(std::time::Duration::from_millis(300));
            assert!(
                !root.join("shell-ran").exists(),
                "login shell spawned while disabled"
            );
            policy.set(false).unwrap();
            assert!(zeron_harness::shell_env::login_shell_path().is_some());
            assert!(root.join("shell-ran").exists());
            return;
        }
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let shell = root.path().join("fake-sh");
        std::fs::write(
            &shell,
            format!(
                "#!/bin/sh\ntouch '{}'\necho __ZERON_SHELL_ENV_BEGIN__\necho PATH=/fake/bin\necho __ZERON_SHELL_ENV_END__\n",
                root.path().join("shell-ran").display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o755)).unwrap();
        let output = tokio::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "local_execution::tests::restart_with_policy_on_spawns_no_login_shell",
                "--nocapture",
            ])
            .env(CHILD, root.path())
            .env("SHELL", &shell)
            .env_remove("ZERON_NO_LOGIN_SHELL")
            .output()
            .await
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// Preview discovery: a restart with the policy persisted never scans the
    /// local projects; `SetLocalExecution` resumes it and pauses it again.
    #[tokio::test]
    async fn preview_discovery_follows_the_policy() {
        use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("local-execution.json"),
            r#"{"disabled":true}"#,
        )
        .unwrap();
        let core = assemble(dir.path());
        let scans = Arc::new(AtomicUsize::new(0));
        let projects: zeron_preview::service::Projects = {
            let scans = scans.clone();
            Arc::new(move || {
                scans.fetch_add(1, SeqCst);
                Vec::new()
            })
        };
        core.start_previews(projects, None).await;
        tokio::time::sleep(std::time::Duration::from_millis(2500)).await;
        assert_eq!(scans.load(SeqCst), 0, "scanned with the policy persisted");

        let rpc = core.rpc_service();
        let set = |disabled: bool| {
            rpc.handle(
                methods::SET_LOCAL_EXECUTION,
                serde_json::json!({ "disabled": disabled }),
            )
        };
        set(false).await.unwrap();
        wait_until("discovery resumed", || scans.load(SeqCst) > 0).await;
        set(true).await.unwrap();
        let after = scans.load(SeqCst);
        tokio::time::sleep(std::time::Duration::from_millis(2500)).await;
        assert_eq!(
            scans.load(SeqCst),
            after,
            "scanned after a confirmed disable"
        );
        core.shutdown().await;
    }
}
