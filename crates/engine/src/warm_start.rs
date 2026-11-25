//! Warm start: a new chat's agent started while its first message is still
//! being written, so Enter delivers that message into a runtime that has
//! already done its startup.
//!
//! The app asks for it when the person opens a new chat (⌘N, New chat) or
//! types into one, naming the id the chat will be created with. Nothing of
//! the chat exists yet: no row, no doc, no title, no transcript. Only a new
//! chat gets one: an id the workspace knows already, or one with a run, is
//! refused. The harness launches the runtime the chat's first run would
//! launch — its model, effort, folder, access and MCP servers, the Zeron
//! server with a token for this chat id alone — and parks it before any
//! prompt, so it asks no model anything ([`Harness::prewarm`]). The first run
//! of that chat id takes it ([`Harness::run_warm`]) when it launches
//! identically, else it is ended and the run starts as it always did. A run
//! that comes while the warm start is still launching waits for it.
//!
//! It lives only as long as the app keeps renewing it: [`OPEN_LEASE`] after
//! the open with nothing typed, then the engine's idle stop
//! ([`super::AGENT_IDLE_STOP`]) after each keystroke the app reports. The
//! app stops it sooner when the draft is left empty, the page is left, the
//! window closes or the app quits; a lease that runs out (an app that went
//! away too) stops it here. A warm start is never work in progress (the
//! keep-running setting never waits on one), the idle reaper never sees it
//! (it is no run), and it is never handed to any chat but its own.
//!
//! Like a run, it holds its CLI's execution lease for its life, so a CLI
//! update never replaces the binary under it; an update already accepted
//! refuses it, and one accepted while it waits ends it at its run. A run
//! started with another credential or binary than it was ends it too.

use std::sync::Arc;
use std::time::Duration;

use tokio::time::Instant;

use tokio::sync::watch;
use zeron_proto::{HarnessId, RunRequest};

use super::{Inner, SessionsEngine, lock};
use crate::EngineError;

/// How long a warm start lives after the open with nothing typed.
pub(crate) const OPEN_LEASE: Duration = Duration::from_secs(60);

/// Its lease: [`OPEN_LEASE`] until something is typed, then the idle stop
/// a parked session gets, from the last keystroke heard of.
/// `ZERON_WARM_OPEN_MS` and `ZERON_WARM_TYPED_MS` override them (the
/// measurement rig), and [`SessionsEngine::set_warm_leases`] (tests).
fn lease(inner: &Inner, typed: bool) -> Duration {
    if let Some((open, on_typed)) = *lock(&inner.warm_leases) {
        return if typed { on_typed } else { open };
    }
    let ms = |key: &str| {
        std::env::var(key)
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .map(Duration::from_millis)
    };
    if typed {
        ms("ZERON_WARM_TYPED_MS").unwrap_or(super::AGENT_IDLE_STOP)
    } else {
        ms("ZERON_WARM_OPEN_MS").unwrap_or(OPEN_LEASE)
    }
}

/// At most this many warm starts at once on one engine (one per window is
/// the app's rule; this bounds any client).
const MAX_WARM: usize = 4;

/// How long a run waits for its warm start still launching (Codex answers
/// `initialize` first) before it starts its own.
const LAUNCH_WAIT: Duration = Duration::from_secs(20);

/// Where a warm start's launch is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Launch {
    Launching,
    Ready,
    Failed,
}

/// A warm start, launching or parked.
pub(super) struct WarmStart {
    harness: HarnessId,
    /// What the app asked for (model, effort, folder, modes): the same ask
    /// renews it, another replaces it.
    asked: RunRequest,
    /// The harness's key for its runtime: one per ask, so a replaced ask's
    /// late launch never lands on (or ends) its successor's.
    key: String,
    generation: u64,
    launch: watch::Receiver<Launch>,
    /// What its run gets, filled in (under the lock) once it is parked.
    parked: Option<Parked>,
    renewed: Instant,
    /// How long it lives past `renewed` ([`lease`]).
    lease: Duration,
}

/// What a parked warm start hands its run.
pub(super) struct Parked {
    /// The runtime's MCP server and its token's grant (dead once dropped).
    pub(super) mcp: Option<zeron_proto::McpServer>,
    pub(super) grant: Option<zeron_mcp::http::Grant>,
    /// Held from before the launch: no CLI update under it.
    pub(super) execution_lease: Arc<tokio::sync::OwnedRwLockReadGuard<()>>,
    /// The credential and binary it launched with (see
    /// `SessionsEngine::credential_changed` and `binary_changed`).
    pub(super) credential: Option<String>,
    pub(super) binary: Option<String>,
    /// Whether it launched without its provider's own subagents (the
    /// setting may change before its run: `native_subagents_changed`).
    pub(super) side_chats_only: bool,
    /// Whether it launched with Code Mode enabled (the setting may change
    /// before its run: `code_mode_changed`).
    pub(super) code_mode: bool,
    /// Whether it launched with only Zeron's MCP servers (the setting, or
    /// whether any is set up, may change before its run).
    pub(super) zeron_mcp_only: bool,
    /// The projectless folder its launch made, removed if it ends unused
    /// and still empty.
    made_folder: Option<std::path::PathBuf>,
}

/// A warm start its run took.
pub(super) struct Taken {
    pub(super) key: String,
    pub(super) parked: Parked,
}

/// What a warm start request did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum WarmOutcome {
    /// A runtime is parked for the chat.
    Started,
    /// The one there already was renewed.
    Renewed,
    /// Nothing to do: the chat exists already, its harness can't park
    /// before a prompt, it runs in a worktree made at send time, its CLI is
    /// being updated, or the launch failed.
    Unsupported,
}

/// Whether two asks launch the same runtime (the prompt aside).
fn same_ask(a: &RunRequest, b: &RunRequest) -> bool {
    a.harness == b.harness
        && a.model == b.model
        && a.reasoning == b.reasoning
        && a.model_options == b.model_options
        && a.cwd == b.cwd
        && a.sandbox == b.sandbox
        && a.access == b.access
        && a.plan == b.plan
        && a.code_mode == b.code_mode
        && a.zeron_mcp_only == b.zeron_mcp_only
        && a.auto_approve == b.auto_approve
}

fn next_generation() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

impl SessionsEngine {
    /// Start (or renew) a warm start for the new chat the app will create as
    /// `chat_id`, with the run `request` describes (its prompt ignored).
    /// `typed`: the person has typed into its draft (the longer lease).
    pub async fn warm_start(
        &self,
        chat_id: &str,
        harness_id: HarnessId,
        mut request: RunRequest,
        typed: bool,
    ) -> Result<WarmOutcome, EngineError> {
        if chat_id.trim().is_empty() {
            return Err(EngineError::Other(
                "a warm start needs the chat's id".into(),
            ));
        }
        // Off on this device (`ZERON_WARM_START=0`), or shutting down.
        if std::env::var("ZERON_WARM_START").is_ok_and(|v| v == "0")
            || self
                .inner
                .shutting_down
                .load(std::sync::atomic::Ordering::Acquire)
        {
            return Ok(WarmOutcome::Unsupported);
        }
        // Only a new chat: none the workspace knows, none with a run (its
        // send came first), none whose run is made as it's sent (a worktree).
        if self.inner.chat_known(chat_id) || request.worktree.is_some() {
            self.warm_stop(chat_id).await;
            return Ok(WarmOutcome::Unsupported);
        }
        let harness = self.inner.registry.resolve(harness_id)?;
        if !harness.prewarms() || self.inner.registry.update_pending(harness_id) {
            self.warm_stop(chat_id).await;
            return Ok(WarmOutcome::Unsupported);
        }
        request.harness = Some(harness_id);
        request.code_mode = self.inner.registry.code_mode(harness_id);
        request.zeron_mcp_only = self.inner.zeron_mcp_only(harness_id);
        request.prompt.clear();
        request.attachments.clear();
        request.mcp = None;
        request.extra_mcp.clear();
        request.env.clear();
        request.resume = None;
        // The folder the run will have: a projectless chat's own scratch
        // folder, named for its id as `createChat` names it.
        if zeron_proto::projectless::is_home_cwd(&request.cwd) {
            request.cwd = zeron_proto::projectless::scratch_cwd(chat_id, chrono::Utc::now());
        }
        // The same ask again (launching or parked): renewed.
        {
            let mut warm = lock(&self.inner.warm);
            if let Some(w) = warm.get_mut(chat_id)
                && w.harness == harness_id
                && same_ask(&w.asked, &request)
                && *w.launch.borrow() != Launch::Failed
            {
                w.renewed = Instant::now();
                w.lease = w.lease.max(lease(&self.inner, typed));
                return Ok(WarmOutcome::Renewed);
            }
        }
        // Another ask (the person changed the model, the folder, …): the old
        // runtime goes (not waited on), the new one starts.
        if let Some(old) = lock(&self.inner.warm).remove(chat_id) {
            let inner = self.inner.clone();
            let chat = chat_id.to_owned();
            tokio::spawn(async move { end_warm(&inner, &chat, old).await });
        }
        // Registered before anything is awaited: a run from here on waits for
        // this launch rather than starting its own, and a second ask while it
        // launches is a renewal, never a second process.
        let generation = next_generation();
        let key = format!("{chat_id}#{generation}");
        let (launched, launch) = watch::channel(Launch::Launching);
        {
            let mut warm = lock(&self.inner.warm);
            if warm.len() >= MAX_WARM {
                return Ok(WarmOutcome::Unsupported);
            }
            warm.insert(
                chat_id.to_owned(),
                WarmStart {
                    harness: harness_id,
                    asked: request.clone(),
                    key: key.clone(),
                    generation,
                    launch,
                    parked: None,
                    renewed: Instant::now(),
                    lease: lease(&self.inner, typed),
                },
            );
        }
        // A send that registered its run just before this ask: that run
        // starts its own (`take_warm` runs after a run is registered, so it
        // sees this ask, or this sees its run).
        if lock(&self.inner.runs).contains_key(chat_id) {
            take_generation(&self.inner, chat_id, generation);
            let _ = launched.send(Launch::Failed);
            return Ok(WarmOutcome::Unsupported);
        }
        self.watch_lease(chat_id.to_owned(), generation);
        // Launched on a task of its own: an RPC dropped midway never leaves
        // a process behind that nothing would end.
        let engine = self.clone();
        let chat = chat_id.to_owned();
        let launch = tokio::spawn(async move {
            let outcome = engine
                .launch_warm(&chat, harness_id, &key, generation, request)
                .await;
            let state = match &outcome {
                Ok(true) => Launch::Ready,
                Ok(false) => Launch::Failed,
                Err(message) => {
                    tracing::info!(chat = %chat, %message, "warm start failed");
                    if let Some(warm) = take_generation(&engine.inner, &chat, generation) {
                        end_warm(&engine.inner, &chat, warm).await;
                    }
                    Launch::Failed
                }
            };
            let _ = launched.send(state);
            state
        });
        match launch.await {
            Ok(Launch::Ready) => {
                tracing::info!(chat = %chat_id, harness = ?harness_id, "warm start");
                Ok(WarmOutcome::Started)
            }
            _ => Ok(WarmOutcome::Unsupported),
        }
    }

    /// The launch of a registered warm start: true once parked, false when it
    /// was stopped meanwhile (what it made is ended again).
    async fn launch_warm(
        &self,
        chat_id: &str,
        harness_id: HarnessId,
        key: &str,
        generation: u64,
        mut request: RunRequest,
    ) -> Result<bool, String> {
        let harness = self
            .inner
            .registry
            .resolve(harness_id)
            .map_err(|e| e.to_string())?;
        // Its CLI's execution lease, as a run's: never during an update.
        let execution_lease = tokio::time::timeout(
            Duration::from_secs(2),
            self.inner.registry.execution_lease(harness_id),
        )
        .await
        .map_err(|_| "its CLI is being updated".to_owned())?;
        let scratch_root = self.scratch_root();
        let made_folder = zeron_proto::projectless::is_scratch_cwd(&request.cwd)
            .then(|| {
                let name = request.cwd.trim().rsplit('/').next().unwrap_or_default();
                scratch_root.join(name)
            })
            .filter(|folder| !folder.exists());
        let launched = async {
            request.cwd = crate::repos::resolve_chat_cwd(&request.cwd, &scratch_root)?;
            // What the chat's first run hands its harness (`drive_run`): the
            // Zeron server with a token for this chat alone, the plugins'
            // servers, the chat's origin and the host's credentials.
            let side_chats_only = self.inner.side_chats_only(chat_id, harness_id);
            let (mcp, grant) =
                match self
                    .inner
                    .zeron_mcp(chat_id, side_chats_only, harness.wakes_on_background())
                {
                    Some((server, grant)) => (Some(server), grant),
                    None => (None, None),
                };
            request.mcp = mcp.clone();
            self.inner.refresh_mcp_servers_for_run().await;
            request.extra_mcp = self.inner.extra_mcp_servers(grant.as_ref());
            request.env.insert(
                zeron_proto::ORIGIN_ENV.into(),
                zeron_proto::chat_origin(chat_id),
            );
            self.inner
                .registry
                .prepare_run(harness_id, Some(chat_id), &mut request)
                .await
                .map_err(|message| format!("not prepared: {message}"))?;
            let credential = self
                .inner
                .registry
                .run_credential(harness_id, Some(chat_id))
                .await;
            let binary = {
                let harness = harness.clone();
                tokio::task::spawn_blocking(move || super::binary_identity(harness.as_ref()))
                    .await
                    .ok()
                    .flatten()
            };
            // Stopped before it launched: nothing to end.
            if !self.is_current(chat_id, generation) {
                return Ok(None);
            }
            let code_mode = request.code_mode;
            let zeron_mcp_only = request.zeron_mcp_only;
            harness
                .prewarm(key, request)
                .await
                .map_err(|e| e.to_string())?;
            Ok::<_, String>(Some(Parked {
                mcp,
                grant,
                execution_lease: Arc::new(execution_lease),
                credential,
                binary,
                side_chats_only,
                code_mode,
                zeron_mcp_only,
                made_folder: made_folder.clone(),
            }))
        };
        let parked = match launched.await {
            Ok(Some(parked)) => parked,
            Ok(None) => {
                remove_if_empty(made_folder.as_deref());
                return Ok(false);
            }
            Err(message) => {
                remove_if_empty(made_folder.as_deref());
                return Err(message);
            }
        };
        // Stopped while it launched (the draft left, the window closed): the
        // process just made goes too.
        let unclaimed = {
            let mut warm = lock(&self.inner.warm);
            match warm.get_mut(chat_id).filter(|w| w.generation == generation) {
                Some(w) => {
                    w.parked = Some(parked);
                    None
                }
                None => Some(parked),
            }
        };
        match unclaimed {
            None => Ok(true),
            Some(parked) => {
                harness.discard_warm(key).await;
                remove_if_empty(parked.made_folder.as_deref());
                Ok(false)
            }
        }
    }

    fn is_current(&self, chat_id: &str, generation: u64) -> bool {
        lock(&self.inner.warm)
            .get(chat_id)
            .is_some_and(|w| w.generation == generation)
    }

    /// Stop the warm start for `chat_id`, if any (its runtime ends, its MCP
    /// token dies), returning once it has.
    pub async fn warm_stop(&self, chat_id: &str) {
        self.inner.stop_warm(chat_id).await;
    }

    /// Stop every warm start (the app quit, the engine shuts down),
    /// returning once they have.
    pub async fn warm_stop_all(&self) {
        let all: Vec<(String, WarmStart)> = lock(&self.inner.warm).drain().collect();
        futures::future::join_all(
            all.into_iter()
                .map(|(chat_id, warm)| async move { end_warm(&self.inner, &chat_id, warm).await }),
        )
        .await;
    }

    /// How long warm starts live unrenewed, opened and typed into (tests).
    pub fn set_warm_leases(&self, open: Duration, typed: Duration) {
        *lock(&self.inner.warm_leases) = Some((open, typed));
    }

    /// The chats with a warm start (tests, diagnostics).
    pub fn warm_chats(&self) -> Vec<String> {
        lock(&self.inner.warm).keys().cloned().collect()
    }

    fn scratch_root(&self) -> std::path::PathBuf {
        self.inner
            .scratch_root
            .get()
            .cloned()
            .unwrap_or_else(|| crate::repos::home_dir().join(".zeron").join("scratch"))
    }

    /// Stops the warm start once its lease runs out unrenewed.
    fn watch_lease(&self, chat_id: String, generation: u64) {
        let inner = Arc::downgrade(&self.inner);
        tokio::spawn(async move {
            let mut wait = Duration::ZERO;
            loop {
                tokio::time::sleep(wait).await;
                let Some(inner) = inner.upgrade() else { return };
                let (renewed, lease) = match lock(&inner.warm).get(&chat_id) {
                    Some(warm) if warm.generation == generation => (warm.renewed, warm.lease),
                    _ => return,
                };
                let left = lease.saturating_sub(renewed.elapsed());
                if !left.is_zero() {
                    wait = left;
                    continue;
                }
                if let Some(warm) = take_generation(&inner, &chat_id, generation) {
                    tracing::info!(chat = %chat_id, "warm start stopped: its lease ran out");
                    end_warm(&inner, &chat_id, warm).await;
                }
                return;
            }
        });
    }
}

impl Inner {
    /// Whether the workspace knows `chat_id` (made, deleted, or claimed by a
    /// run), or a run holds it: no new chat's warm start then.
    fn chat_known(&self, chat_id: &str) -> bool {
        lock(&self.runs).contains_key(chat_id)
            || self
                .workspace()
                .is_some_and(|ws| ws.chat_deleted(chat_id) || !matches!(ws.chat(chat_id), Ok(None)))
    }

    /// The warm start for `chat_id`, taken by its first run once parked (a
    /// run that comes during the launch waits for it, up to
    /// [`LAUNCH_WAIT`]). One made with another harness, or one whose launch
    /// failed, is ended and the run starts its own.
    pub(super) async fn take_warm(&self, chat_id: &str, harness_id: HarnessId) -> Option<Taken> {
        let (generation, mut launch) = {
            let warm = lock(&self.warm);
            let w = warm.get(chat_id)?;
            (w.generation, w.launch.clone())
        };
        let _ = tokio::time::timeout(
            LAUNCH_WAIT,
            launch.wait_for(|launch| *launch != Launch::Launching),
        )
        .await;
        let mut warm = take_generation(self, chat_id, generation)?;
        if warm.harness != harness_id || warm.parked.is_none() {
            tracing::info!(chat = %chat_id, "warm start not used: not parked, or another agent");
            end_warm(self, chat_id, warm).await;
            return None;
        }
        let mut parked = warm.parked.take()?;
        // (Its folder is the run's now.)
        parked.made_folder = None;
        Some(Taken {
            key: std::mem::take(&mut warm.key),
            parked,
        })
    }

    /// [`SessionsEngine::warm_stop`].
    pub(super) async fn stop_warm(&self, chat_id: &str) {
        let Some(warm) = lock(&self.warm).remove(chat_id) else {
            return;
        };
        end_warm(self, chat_id, warm).await;
    }

    /// End a warm start its run took but won't use.
    pub(super) async fn discard_taken(
        &self,
        harness: &dyn zeron_harness::Harness,
        chat_id: &str,
        taken: Taken,
        why: &str,
    ) {
        tracing::info!(chat = %chat_id, why, "warm start not used");
        harness.discard_warm(&taken.key).await;
        drop(taken);
    }
}

fn take_generation(inner: &Inner, chat_id: &str, generation: u64) -> Option<WarmStart> {
    let mut warm = lock(&inner.warm);
    if warm
        .get(chat_id)
        .is_some_and(|w| w.generation == generation)
    {
        warm.remove(chat_id)
    } else {
        None
    }
}

async fn end_warm(inner: &Inner, chat_id: &str, warm: WarmStart) {
    // (Still launching: its launch finds it gone and ends what it made.)
    if let Some(parked) = &warm.parked {
        if let Ok(harness) = inner.registry.resolve(warm.harness) {
            harness.discard_warm(&warm.key).await;
        }
        remove_if_empty(parked.made_folder.as_deref());
    }
    tracing::info!(chat = %chat_id, "warm start stopped");
    drop(warm);
}

/// A projectless folder a warm start made, gone again once nothing used it.
fn remove_if_empty(folder: Option<&std::path::Path>) {
    if let Some(folder) = folder {
        let _ = std::fs::remove_dir(folder);
    }
}
