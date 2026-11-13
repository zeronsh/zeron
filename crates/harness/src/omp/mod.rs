//! omp (oh-my-pi, <https://omp.sh>), driven natively over its RPC mode:
//! `omp --mode rpc-ui`, newline-delimited JSON on stdio — the surface omp
//! documents for a host that owns the process ("RPC mode is the
//! lowest-common-denominator way to embed omp"; ACP is for clients that
//! already speak ACP). `rpc-ui` is that same protocol for a host that
//! answers omp's dialogs: approvals and the ask tool's questions come to
//! Zeron as input requests. PROTOCOL.md is the contract this relies on.
//!
//! - One process per Zeron runtime, one session in it ([`session`]): new,
//!   or resumed from the session file omp wrote for its id.
//! - Turns end on omp's per-prompt `prompt_result`; steers ride `prompt`
//!   with `streamingBehavior: "steer"`; a stop aborts the run in place.
//! - Access modes are omp's approval modes for the launch
//!   (`--approval-mode always-ask | write | yolo`).
//! - Zeron's MCP servers become RPC host tools ([`host_tools`]).
//! - omp's RPC descends from Pi's: the line transport and image loading are
//!   the Pi driver's; the rest differs (ready handshake, protocol v2 chunks,
//!   prompt results, approvals) and lives here.
mod catalog;
mod dialogs;
mod host_tools;
mod normalize;
mod session;
mod sessions;
mod wire;

use crate::{Harness, HarnessError, RunControls};
use async_trait::async_trait;
use futures::{StreamExt, stream::BoxStream};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::sync::mpsc;
use wire::{Launch, Process};
use zeron_proto::{
    AccessMode, AgentEvent, HarnessId, Model, ReasoningLevel, RunRequest, SteeringMode,
};

/// The first omp with every RPC surface this driver uses (`prompt_result`
/// with `sessionSettled`, `set_event_filter` deltas, `set_ask_dialog`,
/// `abort_and_restore_queue`), by the published packages' sources.
const MIN_VERSION: semver::Version = semver::Version::new(18, 7, 0);
/// What omp prints, before `ready`, when it has no model it can use.
const NO_MODEL: &str = "No default model selected";
const PROBE_TIMEOUT: Duration = Duration::from_secs(60);

pub struct OmpHarness {
    executable: Option<PathBuf>,
    session_dir: Option<PathBuf>,
    models_cache: crate::catalog::Catalog,
    workspace_commands: crate::skills::CommandDiscovery,
    interrupt_grace: Duration,
    kill_grace: Duration,
}

impl Default for OmpHarness {
    fn default() -> Self {
        Self {
            executable: None,
            session_dir: None,
            models_cache: Default::default(),
            workspace_commands: Default::default(),
            interrupt_grace: Duration::from_secs(5),
            kill_grace: Duration::from_secs(3),
        }
    }
}

#[derive(Clone, Copy)]
enum Probe {
    Models,
    Commands,
}

impl OmpHarness {
    pub fn new() -> Self {
        Self::default()
    }

    /// Use a fixed CLI binary instead of PATH/known-location resolution.
    pub fn with_executable(mut self, path: impl Into<PathBuf>) -> Self {
        self.executable = Some(path.into());
        self
    }

    /// Store and look up sessions here (omp's `--session-dir`) instead of
    /// omp's own store: tests, so they never write the user's sessions.
    pub fn with_session_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.session_dir = Some(path.into());
        self
    }

    /// How long a teardown waits for omp to confirm the stop, and the
    /// SIGTERM→SIGKILL grace after it.
    pub fn with_graces(mut self, interrupt: Duration, kill: Duration) -> Self {
        self.interrupt_grace = interrupt;
        self.kill_grace = kill;
        self
    }

    pub fn resolve_executable(&self) -> Result<PathBuf, HarnessError> {
        if let Some(path) = self
            .executable
            .clone()
            .or_else(|| std::env::var_os("OMP_EXECUTABLE").map(PathBuf::from))
        {
            return crate::executable::validate_native_override(&path);
        }
        let home = crate::executable::home_or_current_dir();
        crate::executable::find_on_paths(
            "omp",
            vec![
                // omp's installer (`PI_INSTALL_DIR`, default ~/.local/bin),
                // then a bun global install.
                home.join(".local/bin/omp"),
                home.join(".bun/bin/omp"),
                PathBuf::from("/opt/homebrew/bin/omp"),
                PathBuf::from("/usr/local/bin/omp"),
            ],
        )
        .ok_or_else(|| {
            HarnessError::NotInstalled(
                "omp CLI: install it (curl -fsSL https://omp.sh/install | sh) or set OMP_EXECUTABLE"
                    .into(),
            )
        })
    }

    /// The executable, refused when it is older than [`MIN_VERSION`]. An
    /// explicit override is trusted (tests).
    fn executable(&self) -> Result<PathBuf, HarnessError> {
        let exe = self.resolve_executable()?;
        if self.executable.is_none()
            && let Some(version) = crate::executable::binary_version(&exe)
            && version < MIN_VERSION
        {
            return Err(HarnessError::Protocol(format!(
                "omp {version} is unsupported; update omp to {MIN_VERSION} or newer (`omp update`)"
            )));
        }
        Ok(exe)
    }

    /// A short-lived discovery process (`--mode rpc --no-session`). It is
    /// shut down even when the caller stops waiting.
    async fn probe(&self, cwd: &Path, probe: Probe) -> Result<Value, HarnessError> {
        let exe = self.executable()?;
        let mut process = Process::spawn(Launch {
            exe: &exe,
            cwd,
            mode: "rpc",
            args: &["--no-session".into()],
            env: &Default::default(),
        })?;
        let (mut tx, rx) = tokio::sync::oneshot::channel();
        let grace = self.kill_grace;
        tokio::spawn(async move {
            let query = async {
                let mut backlog = vec![];
                process.handshake(&mut backlog).await?;
                match probe {
                    Probe::Models => {
                        let available = process
                            .query(json!({"type":"get_available_models"}), &mut backlog)
                            .await?;
                        let state = process
                            .query(json!({"type":"get_state"}), &mut backlog)
                            .await?;
                        Ok(json!({"available": available, "state": state}))
                    }
                    Probe::Commands => {
                        process
                            .query(json!({"type":"get_available_commands"}), &mut backlog)
                            .await
                    }
                }
            };
            let result = tokio::select! {
                result = tokio::time::timeout(PROBE_TIMEOUT, query) => result
                    .unwrap_or_else(|_| Err(HarnessError::Protocol("omp discovery timed out".into()))),
                _ = tx.closed() => Err(HarnessError::Protocol("omp discovery cancelled".into())),
            };
            let result = match result {
                Err(error) => Err(startup_error(&mut process, error).await),
                ok => ok,
            };
            process.shutdown(grace).await;
            let _ = tx.send(result);
        });
        rx.await
            .map_err(|_| HarnessError::Protocol("omp discovery task failed".into()))?
    }

    fn launch_args(&self, request: &RunRequest, resume: Option<&Path>) -> Vec<String> {
        let mut args = vec!["--approval-mode".to_owned(), approval_mode(request).into()];
        if let Some(dir) = &self.session_dir {
            args.extend(["--session-dir".into(), dir.display().to_string()]);
        }
        if let Some(file) = resume {
            args.extend(["--resume".into(), file.display().to_string()]);
        }
        args
    }
}

/// omp's approval mode for `request`'s access: the highest tier that runs
/// without asking (reads always do).
fn approval_mode(request: &RunRequest) -> &'static str {
    if request.auto_approve {
        return "yolo";
    }
    match request.access {
        AccessMode::Supervised => "always-ask",
        AccessMode::AutoAcceptEdits => "write",
        AccessMode::FullAccess => "yolo",
    }
}

/// Whether a live runtime launched for `live` was launched as `next` would be.
fn launch_matches(live: &RunRequest, next: &RunRequest) -> bool {
    live.cwd == next.cwd
        && approval_mode(live) == approval_mode(next)
        && live.env == next.env
        && live.mcp == next.mcp
        && live.extra_mcp == next.extra_mcp
}

/// The error for a process that failed before its session was up: omp
/// exits before `ready` when it has no model to use (or cannot open the
/// session), saying why on stderr.
async fn startup_error(process: &mut Process, error: HarnessError) -> HarnessError {
    let status = tokio::time::timeout(Duration::from_millis(500), process.child.wait())
        .await
        .ok()
        .and_then(Result::ok);
    let Some(status) = status else {
        return error;
    };
    process.tail.wait_closed().await;
    if process
        .tail
        .snapshot()
        .is_some_and(|tail| tail.contains(NO_MODEL))
    {
        return HarnessError::NotSignedIn(
            "omp has no model it can use: run `omp` and sign in with /login, or set a provider \
             API key"
                .into(),
        );
    }
    HarnessError::Protocol(crate::crash_message("omp", Some(status), &process.tail))
}

#[async_trait]
impl Harness for OmpHarness {
    fn id(&self) -> HarnessId {
        HarnessId::Omp
    }
    fn display_name(&self) -> &str {
        "omp"
    }
    fn supports_steering(&self) -> bool {
        true
    }
    /// A steer joins the running run at its next step (omp's `steer`
    /// queue, interrupt mode `wait`: a running tool finishes first).
    fn steering_mode(&self) -> SteeringMode {
        SteeringMode::StepBoundary
    }
    /// Thinking levels are per model (`thinking.efforts`).
    fn reasoning_levels(&self) -> &[ReasoningLevel] {
        &[]
    }
    /// omp's approval modes: `always-ask` (reads only run unasked), `write`
    /// (edits too), `yolo`. Approvals come over RPC as dialogs.
    fn access_modes(&self) -> &[AccessMode] {
        &AccessMode::ALL
    }
    fn installed(&self) -> bool {
        self.resolve_executable().is_ok()
    }
    fn executable_path(&self) -> Option<PathBuf> {
        self.resolve_executable().ok()
    }
    /// Every prompt gets exactly one `prompt_result`, and a run omp starts
    /// itself ends at its terminal `agent_end`.
    fn deterministic_turn_end(&self) -> bool {
        true
    }
    /// An async job finishing wakes the agent with a turn of its own.
    fn wakes_on_background(&self) -> bool {
        true
    }
    /// A stop aborts the run; the process and its background jobs live on.
    fn stops_turn_in_place(&self) -> bool {
        true
    }
    /// The launch reads the approval mode, the session store and the MCP
    /// servers; the model and thinking level change in place.
    fn same_runtime(&self, live: &RunRequest, next: &RunRequest) -> bool {
        live.model == next.model
            && live.reasoning == next.reasoning
            && crate::same_model_options(&live.model_options, &next.model_options)
            && live.plan == next.plan
            && launch_matches(live, next)
    }
    /// `set_model` and `set_thinking_level` change the live session; going
    /// back to omp's own default cannot be said that way.
    fn reconfigures_in_place(&self, live: &RunRequest, next: &RunRequest) -> bool {
        let model = |r: &RunRequest| r.model.as_deref().filter(|m| *m != "default").is_some();
        launch_matches(live, next)
            && (model(next) || !model(live))
            && (catalog::thinking_selected(next) || !catalog::thinking_selected(live))
    }
    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        Ok(self.model_catalog(false).await?.models)
    }
    fn model_context(&self) -> Result<Option<crate::ModelContext>, HarnessError> {
        let root = sessions::agent_dir();
        crate::model_context::context(
            HarnessId::Omp,
            &self.resolve_executable()?,
            &[
                root.join("config.yml"),
                root.join("models.yml"),
                root.join("models.json"),
                // Logins: signing in to a provider changes the catalog.
                root.join("agent.db"),
            ],
        )
        .map(Some)
    }
    async fn model_catalog(&self, force: bool) -> Result<crate::ModelCatalog, HarnessError> {
        self.models_cache
            .get_with_timeout(
                force,
                Duration::from_secs(65),
                || Ok(self.model_context()?.expect("omp model context").key()),
                || async {
                    let found = self.probe(&std::env::current_dir()?, Probe::Models).await?;
                    let models = catalog::models(&found["available"], &found["state"]["model"]);
                    if models.is_empty() {
                        return Err(HarnessError::NotSignedIn(
                            "omp offers no models: run `omp` and sign in with /login, or set a \
                             provider API key"
                                .into(),
                        ));
                    }
                    Ok(models)
                },
            )
            .await
    }
    fn fallback_models(&self) -> Vec<Model> {
        vec![Model {
            default: false,
            id: "default".into(),
            label: "omp default".into(),
            description: Some("The model configured in omp".into()),
            reasoning_levels: vec![],
            options: vec![],
        }]
    }
    async fn commands(&self) -> Result<Vec<zeron_proto::SlashCommand>, HarnessError> {
        self.commands_for(&std::env::current_dir()?).await
    }
    async fn commands_for(
        &self,
        cwd: &Path,
    ) -> Result<Vec<zeron_proto::SlashCommand>, HarnessError> {
        self.workspace_commands
            .get(cwd, async {
                Ok(catalog::commands(&self.probe(cwd, Probe::Commands).await?))
            })
            .await
    }
    async fn skills(
        &self,
        cwd: &Path,
    ) -> Result<Option<Vec<zeron_proto::invocation::Skill>>, HarnessError> {
        let mut skills = crate::skills::discover(HarnessId::Omp, cwd).await?;
        let commands = self.commands_for(cwd).await?;
        crate::skills::attach_advertised_commands(HarnessId::Omp, &mut skills, &commands);
        Ok(Some(skills))
    }
    async fn run(
        &self,
        request: RunRequest,
        controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        let exe = self.executable()?;
        let mut request = request;
        // A session is resumed from its file; one omp no longer has (or
        // never wrote: a session saves once it has a message) starts fresh,
        // with a visible notice, like the other harnesses. Failing instead
        // would strand the chat: the engine resumes the same id forever.
        let mut lost_context = None;
        let resume = match request.resume.clone() {
            Some(id) => match sessions::find(
                &id,
                &sessions::roots(self.session_dir.as_deref(), &request.env),
            ) {
                Ok(file) => Some(file),
                Err(error) => {
                    lost_context = Some(format!(
                        "omp could not restore session {id}; starting a new session without the \
                         previous context: {error}"
                    ));
                    request.resume = None;
                    None
                }
            },
            None => None,
        };
        let args = self.launch_args(&request, resume.as_deref());
        let process = Process::spawn(Launch {
            exe: &exe,
            cwd: Path::new(&request.cwd),
            mode: "rpc-ui",
            args: &args,
            env: &request.env,
        })?;
        let (tx, rx) = mpsc::channel(256);
        let kill_grace = self.kill_grace;
        let interrupt_grace = self.interrupt_grace;
        tokio::spawn(async move {
            let mut runner = session::Runner::new(process, tx, request);
            runner.lost_context = lost_context;
            // The lease lives through shutdown even if the consumer drops its stream.
            let RunControls {
                realtime: _,
                execution_lease: _lease,
                request_input,
                mut steering,
                interrupt,
                turn,
            } = controls;
            runner.process.dialogs.input = Some(std::sync::Arc::from(request_input));
            runner.control = turn;
            let consumer = runner.tx.clone();
            let result = tokio::select! {
                result = runner.run(&mut steering) => result,
                _ = interrupt.cancelled() => { runner.interrupt(interrupt_grace).await; Ok(()) },
                _ = consumer.closed() => Ok(()),
            };
            if let Err(error) = result {
                runner.error = Some(match startup_error(&mut runner.process, error).await {
                    HarnessError::Protocol(message) => message,
                    other => other.to_string(),
                });
            }
            runner.process.shutdown(kill_grace).await;
            let _ = runner.finish().await;
        });
        Ok(
            futures::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|v| (v, rx)) })
                .boxed(),
        )
    }
}

#[cfg(test)]
pub(super) mod tests_support {
    use zeron_proto::{AccessMode, RunRequest, SandboxLevel};

    pub fn request(cwd: &str) -> RunRequest {
        RunRequest {
            prompt: "hi".into(),
            harness: None,
            model: None,
            reasoning: None,
            model_options: Default::default(),
            cwd: cwd.into(),
            sandbox: SandboxLevel::WorkspaceWrite,
            auto_approve: false,
            access: AccessMode::FullAccess,
            plan: false,
            resume: None,
            attachments: vec![],
            worktree: None,
            mcp: None,
            env: Default::default(),
            extra_mcp: vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_modes_are_approval_modes_and_reshape_the_launch() {
        let mut request = tests_support::request("/w");
        assert_eq!(approval_mode(&request), "yolo");
        request.access = AccessMode::AutoAcceptEdits;
        assert_eq!(approval_mode(&request), "write");
        request.access = AccessMode::Supervised;
        assert_eq!(approval_mode(&request), "always-ask");
        request.auto_approve = true;
        assert_eq!(approval_mode(&request), "yolo");
        let harness = OmpHarness::new().with_session_dir("/tmp/s");
        let live = tests_support::request("/w");
        let mut next = live.clone();
        next.model = Some("openai-codex/gpt-5.6-luna".into());
        assert!(!harness.same_runtime(&live, &next));
        assert!(harness.reconfigures_in_place(&live, &next));
        // Back to omp's default cannot be applied in place.
        assert!(!harness.reconfigures_in_place(&next, &live));
        next.access = AccessMode::Supervised;
        assert!(!harness.reconfigures_in_place(&live, &next));
        assert_eq!(
            harness.launch_args(&next, Some(Path::new("/s/x.jsonl"))),
            [
                "--approval-mode",
                "always-ask",
                "--session-dir",
                "/tmp/s",
                "--resume",
                "/s/x.jsonl"
            ]
        );
    }
}
