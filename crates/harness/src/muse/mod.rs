//! Meta's Muse Code agent, driven natively over the Muse Session Protocol
//! (MSP): `muse serve` speaking newline-delimited JSON-RPC 2.0 on stdio —
//! the protocol Muse's own SDK drives, with no adapter in between.
//!
//! - One host process per Zeron runtime, one session in it ([`session`]).
//!   The session is resumed by id on the next runtime; one the host no
//!   longer has starts fresh with a visible notice.
//! - Turns end on the host's `turn/completed`, including the turns Muse
//!   starts itself when background subagents finish, so the turn end is
//!   deterministic. Steers ride `turn/start` with `ifBusy: "steer"`, a stop
//!   is `turn/interrupt` and leaves the host (and its background agents)
//!   running.
//! - The model catalog and title generation run on a separate read-only
//!   host (`--disable-shell --disable-write --no-session-log`).
//! - Full access is `allowAll` with the `:unrestricted` profile outside the
//!   sandbox; supervised is `promptUnmatched` with `:ask-me` in it, each
//!   approval asked of the person. Either way every stage of a staged
//!   approval (`a && b` is two) is answered: Muse does not carry `allowAll`
//!   to workflow children, and each stage waits on its own decision.
//! - The host's environment drops an inherited `META_API_KEY` (so the
//!   user's own Muse login is used) and turns off Muse's self-update.
pub mod account;
mod catalog;
mod host;
mod normalize;
mod rpc;
mod session;

use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use futures::{StreamExt, stream::BoxStream};
use serde_json::json;
use tokio::sync::mpsc;
use zeron_proto::{
    AccessMode, AgentEvent, HarnessId, Model, ReasoningLevel, RunRequest, SteeringMode,
};

use crate::{Harness, HarnessError, RunControls};
use host::{Host, Mode};

/// The Muse Code harness. Construct with [`MuseHarness::new`]; tests point it
/// at a fake CLI with [`MuseHarness::with_executable`].
pub struct MuseHarness {
    executable: Option<PathBuf>,
    models_cache: crate::catalog::Catalog,
    interrupt_grace: Duration,
    kill_grace: Duration,
}

impl Default for MuseHarness {
    fn default() -> Self {
        Self {
            executable: None,
            models_cache: Default::default(),
            interrupt_grace: Duration::from_secs(5),
            kill_grace: Duration::from_secs(3),
        }
    }
}

impl MuseHarness {
    pub fn new() -> Self {
        Self::default()
    }

    /// Use a fixed CLI binary instead of PATH/known-location resolution.
    pub fn with_executable(mut self, path: impl Into<PathBuf>) -> Self {
        self.executable = Some(path.into());
        self
    }

    /// Tune how long a teardown waits for Muse to confirm the stop, and the
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
            .or_else(|| std::env::var_os("MUSE_EXECUTABLE").map(PathBuf::from))
        {
            return crate::executable::validate_native_override(&path);
        }
        let home = crate::executable::home_or_current_dir();
        crate::executable::find_on_paths(
            "muse",
            vec![
                home.join(".local/bin/muse"),
                PathBuf::from("/opt/homebrew/bin/muse"),
                PathBuf::from("/usr/local/bin/muse"),
            ],
        )
        .ok_or_else(|| {
            HarnessError::NotInstalled(
                "Muse Code CLI: install it (curl -fsSL https://dev.meta.ai/install.sh | sh) or \
                 set MUSE_EXECUTABLE"
                    .into(),
            )
        })
    }

    /// `model/list` from a short-lived read-only host.
    async fn discover_models(&self) -> Result<Vec<Model>, HarnessError> {
        let exe = self.resolve_executable()?;
        let cwd = std::env::current_dir()?;
        let mut host = Host::spawn(&exe, &cwd, Mode::ReadOnly, &Default::default())?;
        let grace = self.kill_grace;
        let result = tokio::time::timeout(Duration::from_secs(20), async {
            host.initialize(true).await?;
            host.call("model/list", json!({}), &mut Vec::new())
                .await?
                .map_err(|failure| host::failed("model/list", &failure))
        })
        .await
        .unwrap_or_else(|_| {
            Err(HarnessError::Protocol(
                "Muse model discovery timed out".into(),
            ))
        });
        host.shutdown(grace).await;
        let models = catalog::models(&result?);
        if models.is_empty() {
            return Err(HarnessError::Protocol(
                "Muse returned no models; run `muse` and sign in with /login".into(),
            ));
        }
        Ok(models)
    }

    fn start(
        &self,
        request: RunRequest,
        controls: RunControls,
        read_only: bool,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        let exe = self.resolve_executable()?;
        let mode = if read_only {
            Mode::ReadOnly
        } else {
            Mode::Session {
                sandbox: !full_access(&request)
                    && request.sandbox != zeron_proto::SandboxLevel::DangerFullAccess,
            }
        };
        let host = Host::spawn(&exe, Path::new(&request.cwd), mode, &request.env)?;
        let (tx, rx) = mpsc::channel(256);
        let kill_grace = self.kill_grace;
        let interrupt_grace = self.interrupt_grace;
        tokio::spawn(async move {
            let mut runner = session::Runner::new(host, tx, request, read_only);
            // The lease lives through shutdown even if the consumer drops its stream.
            let RunControls {
                realtime: _,
                execution_lease: _lease,
                request_input,
                mut steering,
                interrupt,
                turn,
            } = controls;
            runner.input = Some(std::sync::Arc::from(request_input));
            runner.turn = turn;
            let consumer = runner.tx.clone();
            let result = tokio::select! {
                result = runner.run(&mut steering) => result,
                _ = interrupt.cancelled() => { runner.interrupt(interrupt_grace).await; Ok(()) },
                _ = consumer.closed() => Ok(()),
            };
            if let Err(error) = result {
                // The first symptom of a crash is usually the pipe's ("Muse
                // stdout closed"), racing the exit itself: give the exit and
                // the stderr tail a moment so the message names the crash (as
                // the Pi driver does).
                let status =
                    tokio::time::timeout(Duration::from_millis(250), runner.host.child.wait())
                        .await
                        .ok()
                        .and_then(Result::ok);
                runner.error = Some(if status.is_some() {
                    runner.host.tail.wait_closed().await;
                    crate::crash_message("Muse", status, &runner.host.tail)
                } else {
                    error.to_string()
                });
            }
            runner.host.shutdown(kill_grace).await;
            let _ = runner.finish().await;
        });
        Ok(
            futures::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|v| (v, rx)) })
                .boxed(),
        )
    }
}

#[async_trait]
impl Harness for MuseHarness {
    fn id(&self) -> HarnessId {
        HarnessId::Muse
    }
    fn display_name(&self) -> &str {
        "Muse Code"
    }
    fn supports_steering(&self) -> bool {
        true
    }
    /// `turn/steer` lands at the running turn's next model step.
    fn steering_mode(&self) -> SteeringMode {
        SteeringMode::StepBoundary
    }
    /// Tiers are per model (`model/list` variants).
    fn reasoning_levels(&self) -> &[ReasoningLevel] {
        &[]
    }
    /// Supervised asks for what Muse's policy does not already allow; full
    /// access is `allowAll` outside the sandbox. Muse has no plan mode.
    fn access_modes(&self) -> &[AccessMode] {
        &[AccessMode::Supervised, AccessMode::FullAccess]
    }
    fn installed(&self) -> bool {
        self.resolve_executable().is_ok()
    }
    fn executable_path(&self) -> Option<PathBuf> {
        self.resolve_executable().ok()
    }
    fn deterministic_turn_end(&self) -> bool {
        true
    }
    fn stops_turn_in_place(&self) -> bool {
        true
    }
    /// The launch reads the sandbox and approval posture; the model moves
    /// with `session/setModel` and the effort rides each turn.
    fn same_runtime(&self, live: &RunRequest, next: &RunRequest) -> bool {
        live.model == next.model
            && live.reasoning == next.reasoning
            && crate::same_model_options(&live.model_options, &next.model_options)
            && launch_matches(live, next)
    }
    fn reconfigures_in_place(&self, live: &RunRequest, next: &RunRequest) -> bool {
        launch_matches(live, next)
    }
    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        Ok(self.model_catalog(false).await?.models)
    }
    fn model_context(&self) -> Result<Option<crate::ModelContext>, HarnessError> {
        crate::model_context::context(HarnessId::Muse, &self.resolve_executable()?, &[]).map(Some)
    }
    async fn model_catalog(&self, force: bool) -> Result<crate::ModelCatalog, HarnessError> {
        self.models_cache
            .get_with_timeout(
                force,
                Duration::from_secs(25),
                || Ok(self.model_context()?.expect("Muse model context").key()),
                || self.discover_models(),
            )
            .await
    }
    fn fallback_models(&self) -> Vec<Model> {
        vec![Model {
            id: "muse-spark-1.2".into(),
            label: "Muse Spark 1.2".into(),
            description: Some("Muse Code's default coding model".into()),
            reasoning_levels: vec![
                ReasoningLevel::Low,
                ReasoningLevel::Medium,
                ReasoningLevel::High,
                ReasoningLevel::XHigh,
                ReasoningLevel::Max,
            ],
            options: vec![],
        }]
    }
    /// A read-only session (no shell, no writes, approvals denied) asked
    /// for the title as quoted data.
    async fn run_title(
        &self,
        request: RunRequest,
        controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        self.start(request, controls, true)
    }
    async fn run(
        &self,
        request: RunRequest,
        controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        self.start(request, controls, false)
    }
}

/// Whether `request` runs without asking: allow everything, unsandboxed.
pub(super) fn full_access(request: &RunRequest) -> bool {
    request.auto_approve || request.access.is_full_access()
}

fn launch_matches(live: &RunRequest, next: &RunRequest) -> bool {
    live.cwd == next.cwd
        && full_access(live) == full_access(next)
        && live.sandbox == next.sandbox
        && live.env == next.env
        && live.mcp == next.mcp
}
