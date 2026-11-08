//! A new session in a new Scaffold sandbox, started from a viewer device —
//! the phone's counterpart of the desktop composer's Scaffold destinations.
//!
//! The phone holds no execution credentials: one of the owner's desktop
//! controllers creates and attaches the sandbox and publishes its session;
//! the phone then sends the first message into it. Each mutation is
//! journaled before it is sent and sent once. An outcome that never came
//! back (a timeout, the app killed mid-launch) is reconciled by its command
//! id on the next launch, never replayed — so an interrupted launch resumes
//! its own sandbox instead of creating a second one.

use std::collections::HashSet;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use zeron_proto::{
    AgentRoute, AgentRoutingMode, ChatConfig, CollaborationScope, HarnessId,
    ScaffoldDatabaseEnvironment, ScaffoldEnvironmentControl, ScaffoldEnvironmentControlResult,
    SessionEnvironmentSource,
};

use crate::error::{ClientError, Result};
use crate::{Client, SendRequest, lock};

/// How long one launch watches an unsettled operation (the desktop's deadline).
const OBSERVE_DEADLINE: Duration = Duration::from_secs(10 * 60);
const RECONCILE_INTERVAL: Duration = Duration::from_secs(2);
/// How long an attached sandbox's host gets to come online (the desktop's).
const HOST_ONLINE_DEADLINE: Duration = Duration::from_secs(180);
const HOST_POLL: Duration = Duration::from_millis(500);
/// One `crew.status` probe per candidate controller.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
/// Answers of "Unknown Crew command" in a row before concluding the
/// controller never received the command (a reply racing its own dispatch
/// reads unknown for milliseconds, not seconds).
const UNADMITTED_ANSWERS: usize = 3;
/// Reconciles the controller refuses in a row (its registry still syncing,
/// say) before handing back with the command still owed (the desktop's).
const REFUSED_RECONCILES: usize = 5;
const JOURNAL_FILE: &str = "crew-launch.json";

/// Desktop destination labels (crates/ui `scaffold_destination_label`).
pub fn database_label(database: ScaffoldDatabaseEnvironment) -> &'static str {
    match database {
        ScaffoldDatabaseEnvironment::Local => "Scaffold (Local DB)",
        ScaffoldDatabaseEnvironment::StagingSnapshot => "Scaffold (Staging DB)",
        ScaffoldDatabaseEnvironment::ProductionSnapshot => "Scaffold (Production DB)",
    }
}

/// A model a Scaffold sandbox can run: managed OMP/Antigravity routes only,
/// the same filter the desktop applies to its Scaffold destinations.
pub fn is_managed_model(harness: HarnessId, model: &str) -> bool {
    matches!(harness, HarnessId::Omp | HarnessId::Antigravity)
        && AgentRoute::from_harness_model(harness, model).is_some()
}

/// The credential-free route for a picked harness/model, pinned to an Agent
/// Auth account when the picker chose one (`crewAccountId`).
pub fn launch_route(config: &ChatConfig) -> Result<AgentRoute> {
    let choose = || {
        ClientError::InvalidArgument(
            "Choose a managed Crew model before creating a sandbox.".into(),
        )
    };
    let model = config.model.as_deref().ok_or_else(choose)?;
    if !is_managed_model(config.harness, model) {
        return Err(choose());
    }
    let mut route = AgentRoute::from_harness_model(config.harness, model).ok_or_else(choose)?;
    if let Some(account) = config
        .model_options
        .get("crewAccountId")
        .and_then(Value::as_str)
        .filter(|account| !account.trim().is_empty())
    {
        route.routing_mode = AgentRoutingMode::Pinned;
        route.account_id = Some(account.to_owned());
    }
    route
        .validate()
        .map_err(|message| ClientError::InvalidArgument(message.into()))?;
    Ok(route)
}

/// Where a launch stands, for the platform's progress line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrewLaunchPhase {
    /// Reaching a desktop controller and creating the sandbox.
    Creating,
    /// The sandbox exists; its session host is being attached.
    Starting,
    /// Attached; waiting for the host to come online before sending.
    Connecting,
}

/// What the user submitted on the new-session page.
#[derive(Debug, Clone)]
pub struct CrewLaunch {
    pub database: ScaffoldDatabaseEnvironment,
    /// The picked harness, model, effort and options. The chat keeps them.
    pub config: ChatConfig,
    /// Sent once, after the sandbox's host is online.
    pub first: SendRequest,
}

/// The one launch whose outcome is still owed. Survives restarts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetainedLaunch {
    /// The desktop controller holding the command; only it can answer.
    pub controller: String,
    /// The new session's id (minted here, the sandbox's scope).
    pub session_id: String,
    pub database: ScaffoldDatabaseEnvironment,
    /// The command whose outcome is awaited: reconciled, never replayed.
    pub command_id: String,
    pub stage: LaunchStage,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum LaunchStage {
    Create,
    #[serde(rename_all = "camelCase")]
    Attach {
        sandbox_id: String,
    },
}

/// `scaffold.control` / `scaffold.reconcile` answers (crates/engine crew.rs).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Outcome {
    command_id: String,
    status: String,
    #[serde(default)]
    result: Option<ScaffoldEnvironmentControlResult>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, PartialEq)]
enum Step {
    /// A new control to send once; the record already names its command.
    Dispatch(ScaffoldEnvironmentControl),
    /// Not settled: reconcile the same command again.
    Observe,
    /// Attached to this session host.
    Attached(String),
    /// Settled without a usable session. `retire`: the command is final.
    Failed { message: String, retire: bool },
}

/// One answer for the record's command → the launch's next step. Advances
/// the record from Create to Attach (a distinct, newly minted command).
fn advance(record: &mut RetainedLaunch, outcome: &Outcome, mint: impl FnOnce() -> String) -> Step {
    if outcome.command_id != record.command_id {
        return Step::Failed {
            message: "Crew answered for a different operation. Your draft is kept; check Settings → Scaffold sandboxes before sending again.".into(),
            retire: false,
        };
    }
    match outcome.status.as_str() {
        "rejected" => {
            let reason = outcome
                .error
                .clone()
                .filter(|reason| !reason.trim().is_empty())
                .unwrap_or_else(|| "Crew rejected the sandbox operation.".into());
            let recovery = match record.stage {
                LaunchStage::Create => "Your draft is kept; no sandbox was created.",
                LaunchStage::Attach { .. } => {
                    "Your draft is kept. The sandbox stays listed in Settings → Scaffold sandboxes; no replacement was created."
                }
            };
            Step::Failed {
                message: format!("{reason} {recovery}"),
                retire: true,
            }
        }
        "confirmed" => {
            let Some(result) = &outcome.result else {
                return Step::Observe;
            };
            if result.environment.scope.session_id.as_deref() != Some(record.session_id.as_str()) {
                return Step::Failed {
                    message:
                        "Crew answered for another session. Your draft is kept; nothing was sent."
                            .into(),
                    retire: false,
                };
            }
            let SessionEnvironmentSource::Scaffold { sandbox_id, .. } = &result.environment.source
            else {
                return Step::Failed {
                    message: "Crew did not return a Scaffold sandbox. Your draft is kept; nothing was sent.".into(),
                    retire: false,
                };
            };
            match &record.stage {
                LaunchStage::Create => {
                    record.stage = LaunchStage::Attach {
                        sandbox_id: sandbox_id.clone(),
                    };
                    record.command_id = mint();
                    Step::Dispatch(ScaffoldEnvironmentControl::Attach {
                        sandbox_id: sandbox_id.clone(),
                        scope: result.environment.scope.clone(),
                    })
                }
                LaunchStage::Attach {
                    sandbox_id: expected,
                } if expected != sandbox_id => Step::Failed {
                    message:
                        "Crew attached a different sandbox. Your draft is kept; nothing was sent."
                            .into(),
                    retire: false,
                },
                LaunchStage::Attach { .. } => {
                    match (&result.control_grant, &result.attached_device_id) {
                        (Some(_), Some(host)) => Step::Attached(host.clone()),
                        // Native host authority not confirmed yet.
                        _ => Step::Observe,
                    }
                }
            }
        }
        _ => Step::Observe,
    }
}

/// The controller answered that it holds no such command.
fn unknown_command(error: &ClientError) -> bool {
    matches!(error, ClientError::HostError(message) if message.contains("Unknown Crew command"))
}

/// A failure that says nothing about the command (offline, timed out).
fn transient(error: &ClientError) -> bool {
    matches!(
        error,
        ClientError::HostUnavailable(_) | ClientError::Network(_)
    )
}

/// An error in its own words: no category, no relay method name.
fn reason(error: &ClientError) -> String {
    let text = match error {
        ClientError::NotFound(text)
        | ClientError::InvalidArgument(text)
        | ClientError::HostUnavailable(text)
        | ClientError::Unsupported(text)
        | ClientError::HostError(text)
        | ClientError::Network(text)
        | ClientError::Auth(text)
        | ClientError::Storage(text)
        | ClientError::NotImplemented(text)
        | ClientError::Internal(text) => text.as_str(),
        ClientError::Closed => return error.to_string(),
    };
    [
        "scaffold.control: ",
        "scaffold.reconcile: ",
        "crew.status: ",
    ]
    .iter()
    .find_map(|method| text.strip_prefix(method))
    .unwrap_or(text)
    .trim_end_matches('.')
    .to_owned()
}

/// `crew.status` names a desktop controller of this project and deployment.
fn is_controller(status: &Value, project: &str, deployment: &str) -> bool {
    status.get("role").and_then(Value::as_str) == Some("local_controller")
        && status.pointer("/scope/projectId").and_then(Value::as_str) == Some(project)
        && status
            .pointer("/scope/deploymentId")
            .and_then(Value::as_str)
            == Some(deployment)
}

/// The retained launch, on disk next to the profile's other state.
pub(crate) struct Journal {
    path: PathBuf,
}

impl Journal {
    pub(crate) fn at(dir: &Path) -> Self {
        Self {
            path: dir.join(JOURNAL_FILE),
        }
    }

    pub(crate) fn load(&self) -> Result<Option<RetainedLaunch>> {
        match std::fs::read(&self.path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(|error| {
                ClientError::Storage(format!(
                    "The pending sandbox launch can't be read ({error}); it was kept unchanged."
                ))
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(ClientError::Storage(error.to_string())),
        }
    }

    /// Durable before the command it names is sent.
    fn save(&self, record: &RetainedLaunch) -> Result<()> {
        let storage = |error: std::io::Error| {
            ClientError::Storage(format!("Couldn't record the sandbox launch: {error}"))
        };
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(storage)?;
        }
        let tmp = self.path.with_extension("json.tmp");
        let bytes =
            serde_json::to_vec(record).map_err(|error| ClientError::Internal(error.to_string()))?;
        let mut file = std::fs::File::create(&tmp).map_err(storage)?;
        file.write_all(&bytes).map_err(storage)?;
        file.sync_all().map_err(storage)?;
        std::fs::rename(&tmp, &self.path).map_err(storage)
    }

    pub(crate) fn clear(&self) -> Result<()> {
        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(ClientError::Storage(error.to_string())),
        }
    }
}

/// One launch at a time per profile: a second would contend for the journal.
struct Running(PathBuf);

impl Running {
    fn acquire(dir: &Path) -> Result<Self> {
        if !running().insert(dir.to_path_buf()) {
            return Err(ClientError::InvalidArgument(
                "Another sandbox is still starting. Your draft is kept; send it once that session opens.".into(),
            ));
        }
        Ok(Self(dir.to_path_buf()))
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        running().remove(&self.0);
    }
}

fn running() -> std::sync::MutexGuard<'static, HashSet<PathBuf>> {
    static RUNNING: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    lock(RUNNING.get_or_init(Default::default))
}

/// The world a launch drives: a desktop controller over the relay and the
/// session as this device sees it. `Client` is the real one; tests fake it.
#[async_trait::async_trait]
pub(crate) trait LaunchPort: Send + Sync {
    /// An online desktop controller of this project/deployment.
    async fn controller(&self) -> Result<String>;
    async fn control(
        &self,
        controller: &str,
        command_id: &str,
        control: ScaffoldEnvironmentControl,
    ) -> Result<Value>;
    async fn reconcile(&self, controller: &str, command_id: &str) -> Result<Value>;
    /// The session already carries a message (its launch finished before).
    fn consumed(&self, session_id: &str) -> bool;
    /// The published session runs on `host`, and `host` is online.
    fn ready(&self, session_id: &str, host: &str) -> bool;
    /// Apply the picked settings and send the first message.
    fn start(&self, session_id: &str, config: &ChatConfig, first: SendRequest) -> Result<()>;
}

pub(crate) async fn run<P: LaunchPort>(
    port: &P,
    journal: &Journal,
    scope: &CollaborationScope,
    launch: CrewLaunch,
    phase: &(dyn Fn(CrewLaunchPhase) + Send + Sync),
) -> Result<String> {
    let route = launch_route(&launch.config)?;
    // A retained launch whose message already went out is finished: forget
    // it and start this one fresh (at most once).
    for _ in 0..2 {
        phase(CrewLaunchPhase::Creating);
        let (mut record, mut pending) = match journal.load()? {
            Some(record) if record.database != launch.database => {
                return Err(ClientError::InvalidArgument(format!(
                    "A {} sandbox from an earlier send is still starting. Choose {} and send again to finish it first.",
                    database_label(record.database),
                    database_label(record.database),
                )));
            }
            Some(record) => (record, None),
            None => {
                let controller = port.controller().await?;
                let session_id = crate::new_id();
                let mut scope = scope.clone();
                scope.session_id = Some(session_id.clone());
                let record = RetainedLaunch {
                    controller,
                    session_id,
                    database: launch.database,
                    command_id: crate::new_id(),
                    stage: LaunchStage::Create,
                };
                let control = ScaffoldEnvironmentControl::Create {
                    scope,
                    name: None,
                    source_ref: None,
                    region: None,
                    database_environment: launch.database,
                    agent_route: route.clone(),
                };
                (record, Some(control))
            }
        };
        if pending.is_none() && port.consumed(&record.session_id) {
            journal.clear()?;
            continue;
        }
        let host = observe(port, journal, &mut record, &mut pending, phase).await?;
        phase(CrewLaunchPhase::Connecting);
        if port.consumed(&record.session_id) {
            journal.clear()?;
            continue;
        }
        let ready_by = tokio::time::Instant::now() + HOST_ONLINE_DEADLINE;
        while !port.ready(&record.session_id, &host) {
            if tokio::time::Instant::now() >= ready_by {
                return Err(ClientError::HostUnavailable(
                    "The sandbox is attached, but its host isn't online yet. Send again to keep waiting; your draft is kept and nothing will be created twice.".into(),
                ));
            }
            tokio::time::sleep(HOST_POLL).await;
        }
        port.start(&record.session_id, &launch.config, launch.first)?;
        if let Err(error) = journal.clear() {
            // The message is out; a stale entry is recognized as consumed.
            tracing::warn!(%error, "sandbox launch finished but its journal entry remains");
        }
        return Ok(record.session_id);
    }
    Err(ClientError::Internal(
        "sandbox launch journal did not settle".into(),
    ))
}

/// Send `pending` (when set) once, then reconcile the record's command until
/// it is attached, settles without a session, or the deadline passes.
async fn observe<P: LaunchPort>(
    port: &P,
    journal: &Journal,
    record: &mut RetainedLaunch,
    pending: &mut Option<ScaffoldEnvironmentControl>,
    phase: &(dyn Fn(CrewLaunchPhase) + Send + Sync),
) -> Result<String> {
    let deadline = tokio::time::Instant::now() + OBSERVE_DEADLINE;
    let mut unadmitted = 0;
    let mut refused = 0;
    // The controller answered the dispatch itself with an error: the command
    // may or may not have been admitted before it failed. Its reply is in,
    // so one reconcile tells which.
    let mut refusal: Option<ClientError> = None;
    loop {
        if matches!(record.stage, LaunchStage::Attach { .. }) {
            phase(CrewLaunchPhase::Starting);
        }
        let dispatched = pending.is_some();
        let answer = match pending.take() {
            Some(control) => {
                journal.save(record)?;
                port.control(&record.controller, &record.command_id, control)
                    .await
            }
            None => port.reconcile(&record.controller, &record.command_id).await,
        };
        let outcome = match answer {
            Ok(value) => match serde_json::from_value::<Outcome>(value) {
                Ok(outcome) => {
                    unadmitted = 0;
                    refused = 0;
                    refusal = None;
                    outcome
                }
                Err(error) => {
                    let error = ClientError::HostError(format!("Unreadable Crew outcome: {error}"));
                    wait(deadline, Some(&error)).await?;
                    continue;
                }
            },
            Err(error) if unknown_command(&error) => {
                unadmitted += 1;
                if let Some(refusal) = refusal.take() {
                    journal.clear()?;
                    return Err(ClientError::HostError(format!(
                        "{}. Nothing was created; your draft is kept.",
                        reason(&refusal)
                    )));
                }
                if unadmitted >= UNADMITTED_ANSWERS {
                    // The controller never received it: nothing to reconcile.
                    journal.clear()?;
                    return Err(ClientError::HostUnavailable(
                        "Your computer never received the sandbox request, so nothing was created. Your draft is kept; send again.".into(),
                    ));
                }
                wait(deadline, Some(&error)).await?;
                continue;
            }
            Err(error) if transient(&error) => {
                wait(deadline, Some(&error)).await?;
                continue;
            }
            // Checked before anything was sent.
            Err(error @ ClientError::InvalidArgument(_)) if dispatched => {
                journal.clear()?;
                return Err(error);
            }
            Err(error) if dispatched => {
                refusal = Some(error);
                continue;
            }
            // The controller can't account for the command right now (its
            // registry still syncing, signed out, ambiguous sandboxes): ask a
            // few more times; its outcome stays owed either way.
            Err(error) => {
                refused += 1;
                if refused >= REFUSED_RECONCILES {
                    return Err(error);
                }
                wait(deadline, Some(&error)).await?;
                continue;
            }
        };
        match advance(record, &outcome, crate::new_id) {
            Step::Dispatch(control) => *pending = Some(control),
            Step::Observe => {
                wait(deadline, outcome.error.map(ClientError::HostError).as_ref()).await?;
            }
            Step::Attached(host) => return Ok(host),
            Step::Failed { message, retire } => {
                if retire {
                    journal.clear()?;
                }
                return Err(ClientError::HostError(message));
            }
        }
    }
}

/// Pause before the next reconcile, or give up at the deadline with the
/// command still journaled for the next send.
async fn wait(deadline: tokio::time::Instant, last: Option<&ClientError>) -> Result<()> {
    if tokio::time::Instant::now() + RECONCILE_INTERVAL > deadline {
        let detail = last
            .map(|error| format!(" Last answer: {}.", reason(error)))
            .unwrap_or_default();
        return Err(ClientError::HostUnavailable(format!(
            "The sandbox is still starting.{detail} Send again to keep checking; your draft is kept and nothing will be created twice."
        )));
    }
    tokio::time::sleep(RECONCILE_INTERVAL).await;
    Ok(())
}

struct ClientPort<'a> {
    client: &'a Client,
    deployment: &'a str,
}

#[async_trait::async_trait]
impl LaunchPort for ClientPort<'_> {
    async fn controller(&self) -> Result<String> {
        self.client.crew_launch_controller(self.deployment).await
    }

    async fn control(
        &self,
        controller: &str,
        command_id: &str,
        control: ScaffoldEnvironmentControl,
    ) -> Result<Value> {
        self.client
            .crew_control(controller, command_id, control, self.deployment)
            .await
    }

    async fn reconcile(&self, controller: &str, command_id: &str) -> Result<Value> {
        self.client.crew_reconcile(controller, command_id).await
    }

    fn consumed(&self, session_id: &str) -> bool {
        let Some(chat) = self.client.inner.workspace.chat(session_id) else {
            return false;
        };
        if chat.last_message_at.is_some() || chat.last_message_preview.is_some() {
            return true;
        }
        self.client.open_session(session_id).is_ok_and(|session| {
            !session.snapshot().entries.is_empty() || !session.composer().pending_sends.is_empty()
        })
    }

    fn ready(&self, session_id: &str, host: &str) -> bool {
        self.client
            .inner
            .workspace
            .chat(session_id)
            .is_some_and(|chat| chat.device_id == host && chat.cwd.is_some())
            && self
                .client
                .workspace()
                .device(host)
                .is_some_and(|device| device.online)
    }

    fn start(&self, session_id: &str, config: &ChatConfig, first: SendRequest) -> Result<()> {
        let mut config = config.clone();
        // The controller chose the sandbox's own access; keep it.
        if let Some(existing) = self.client.session_config(session_id) {
            config.sandbox = existing.sandbox;
            config.access = existing.access;
            config.plan = existing.plan;
        }
        self.client.set_session_config(session_id, &config)?;
        self.client.open_session(session_id)?.send(first)?;
        Ok(())
    }
}

impl Client {
    /// One of the owner's online desktop controllers for this deployment —
    /// never a sandbox host, Cloud machine or phone. Macs first.
    pub async fn crew_launch_controller(&self, deployment: &str) -> Result<String> {
        self.crew_scope(self.org_id(), deployment)?;
        let live = self.inner.live().ok_or_else(|| {
            ClientError::Unsupported("Scaffold sandboxes need a signed-in Crew account".into())
        })?;
        let mut candidates: Vec<_> = self
            .workspace()
            .execution_devices()
            .into_iter()
            .filter(|device| {
                device.online
                    && !device.is_self
                    && zeron_proto::parse_scaffold_device_id(&device.id).is_none()
                    && !crate::workspace::is_cloud_device(&device.id)
            })
            .collect();
        candidates.sort_by_key(|device| {
            (
                device.platform != "macos",
                device.name.to_lowercase(),
                device.id.clone(),
            )
        });
        let answers = futures::future::join_all(candidates.iter().map(|device| {
            tokio::time::timeout(
                PROBE_TIMEOUT,
                live.relay.call(&device.id, "crew.status", json!({})),
            )
        }))
        .await;
        candidates
            .iter()
            .zip(answers)
            .find(|(_, answer)| {
                matches!(answer, Ok(Ok(status)) if is_controller(status, self.org_id(), deployment))
            })
            .map(|(device, _)| device.id.clone())
            .ok_or_else(|| {
                ClientError::HostUnavailable(
                    "Scaffold sandboxes start through Crew on your computer. Open Crew there and sign in, then try again; your draft is kept.".into(),
                )
            })
    }

    /// Create and attach a sandbox through a desktop controller, then send
    /// `launch.first` into its new session. Returns the session id.
    ///
    /// Errors leave the draft with the caller. While an operation's outcome
    /// is owed it stays journaled ([`Client::crew_launch_retained`]): the next
    /// launch reconciles it instead of creating another sandbox.
    pub async fn crew_launch(
        &self,
        launch: CrewLaunch,
        deployment: &str,
        phase: &(dyn Fn(CrewLaunchPhase) + Send + Sync),
    ) -> Result<String> {
        self.crew_scope(self.org_id(), deployment)?;
        let _running = Running::acquire(&self.inner.config.data_dir)?;
        let scope = CollaborationScope {
            project_id: self.org_id().to_owned(),
            deployment_id: Some(deployment.to_owned()),
            session_id: None,
            unknown: Default::default(),
        };
        let port = ClientPort {
            client: self,
            deployment,
        };
        run(
            &port,
            &Journal::at(&self.inner.config.data_dir),
            &scope,
            launch,
            phase,
        )
        .await
    }

    /// The launch whose outcome is still owed, if any.
    pub fn crew_launch_retained(&self) -> Result<Option<RetainedLaunch>> {
        Journal::at(&self.inner.config.data_dir).load()
    }

    /// Stop tracking the retained launch (the user's explicit choice). A
    /// sandbox it created stays listed with the controller's sandboxes.
    pub fn crew_launch_forget(&self) -> Result<()> {
        let _running = Running::acquire(&self.inner.config.data_dir)?;
        Journal::at(&self.inner.config.data_dir).clear()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use zeron_proto::{SandboxLevel, ScaffoldControlGrant, ScaffoldLifecycle, SessionEnvironment};

    fn config(harness: HarnessId, model: &str) -> ChatConfig {
        ChatConfig {
            harness,
            model: Some(model.into()),
            reasoning: None,
            model_options: Default::default(),
            sandbox: SandboxLevel::WorkspaceWrite,
            access: Default::default(),
            plan: false,
        }
    }

    fn scope(session: &str) -> CollaborationScope {
        CollaborationScope {
            project_id: "project".into(),
            deployment_id: Some("deployment".into()),
            session_id: Some(session.into()),
            unknown: Default::default(),
        }
    }

    fn result(session: &str, sandbox: &str, attached: Option<&str>) -> Value {
        serde_json::to_value(ScaffoldEnvironmentControlResult {
            environment: SessionEnvironment {
                source: SessionEnvironmentSource::Scaffold {
                    sandbox_id: sandbox.into(),
                    region: None,
                    lifecycle: ScaffoldLifecycle::Ready,
                    lifecycle_epoch: Some(1),
                    links: Default::default(),
                },
                name: None,
                owner_principal: "owner".into(),
                scope: scope(session),
                source_ref: None,
                last_activity_at: None,
                database_environment: None,
                unknown: Default::default(),
            },
            preparation_generation: None,
            attached_device_id: attached.map(Into::into),
            run_id: None,
            room_projection: None,
            control_grant: attached.map(|_| ScaffoldControlGrant {
                id: "grant".into(),
                expires_at: i64::MAX,
                capabilities: vec![],
            }),
            handoff_native_session_id: None,
            handoff_cwd: None,
        })
        .unwrap()
    }

    fn outcome(command: &str, status: &str, result: Option<Value>) -> Value {
        json!({"commandId": command, "status": status, "result": result, "error": null})
    }

    fn record(stage: LaunchStage) -> RetainedLaunch {
        RetainedLaunch {
            controller: "mac".into(),
            session_id: "session".into(),
            database: ScaffoldDatabaseEnvironment::Local,
            command_id: "one".into(),
            stage,
        }
    }

    fn parse(value: Value) -> Outcome {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn managed_models_and_routes() {
        assert!(is_managed_model(HarnessId::Omp, "openai-codex/gpt-5.5"));
        assert!(is_managed_model(HarnessId::Omp, "anthropic/claude-opus-5"));
        assert!(is_managed_model(HarnessId::Antigravity, "gemini-3-pro"));
        assert!(
            !is_managed_model(HarnessId::Omp, "gpt-5.5"),
            "provider-qualified only"
        );
        assert!(!is_managed_model(
            HarnessId::ClaudeCode,
            "anthropic/claude-opus-5"
        ));
        assert!(!is_managed_model(HarnessId::Antigravity, "google/gemini"));

        let route = launch_route(&config(HarnessId::Omp, "openai/gpt-5.5")).unwrap();
        assert_eq!(route.omp_model(), "openai-codex/gpt-5.5");
        assert_eq!(route.routing_mode, AgentRoutingMode::Automatic);
        let mut pinned = config(HarnessId::Omp, "anthropic/claude-opus-5");
        pinned
            .model_options
            .insert("crewAccountId".into(), json!("acct-1"));
        let route = launch_route(&pinned).unwrap();
        assert_eq!(route.routing_mode, AgentRoutingMode::Pinned);
        assert_eq!(route.account_id.as_deref(), Some("acct-1"));
        assert_eq!(
            launch_route(&config(HarnessId::Antigravity, "gemini-3-pro"))
                .unwrap()
                .harness(),
            HarnessId::Antigravity
        );

        let mut none = config(HarnessId::Omp, "x");
        none.model = None;
        assert!(matches!(
            launch_route(&none),
            Err(ClientError::InvalidArgument(_))
        ));
        assert!(matches!(
            launch_route(&config(HarnessId::Codex, "gpt-5.5")),
            Err(ClientError::InvalidArgument(_))
        ));
    }

    #[test]
    fn create_confirmation_mints_a_distinct_attach() {
        let mut rec = record(LaunchStage::Create);
        let step = advance(
            &mut rec,
            &parse(outcome(
                "one",
                "confirmed",
                Some(result("session", "sbx", None)),
            )),
            || "two".into(),
        );
        assert_eq!(
            step,
            Step::Dispatch(ScaffoldEnvironmentControl::Attach {
                sandbox_id: "sbx".into(),
                scope: scope("session")
            })
        );
        assert_eq!(rec.command_id, "two");
        assert_eq!(
            rec.stage,
            LaunchStage::Attach {
                sandbox_id: "sbx".into()
            }
        );
    }

    #[test]
    fn attach_needs_a_grant_and_host() {
        let mut rec = record(LaunchStage::Attach {
            sandbox_id: "sbx".into(),
        });
        let mint = || -> String { panic!("attach never mints") };
        assert_eq!(
            advance(
                &mut rec,
                &parse(outcome(
                    "one",
                    "confirmed",
                    Some(result("session", "sbx", None))
                )),
                mint
            ),
            Step::Observe
        );
        assert_eq!(
            advance(&mut rec, &parse(outcome("one", "unknown", None)), mint),
            Step::Observe
        );
        assert_eq!(
            advance(
                &mut rec,
                &parse(outcome(
                    "one",
                    "confirmed",
                    Some(result("session", "sbx", Some("host")))
                )),
                mint
            ),
            Step::Attached("host".into())
        );
        assert!(matches!(
            advance(
                &mut rec,
                &parse(outcome(
                    "one",
                    "confirmed",
                    Some(result("session", "other", Some("host")))
                )),
                mint
            ),
            Step::Failed { retire: false, .. }
        ));
    }

    #[test]
    fn foreign_answers_fail_without_retiring() {
        let mut rec = record(LaunchStage::Create);
        let mint = || -> String { panic!("no mint") };
        assert!(matches!(
            advance(&mut rec, &parse(outcome("other", "confirmed", None)), mint),
            Step::Failed { retire: false, .. }
        ));
        assert!(matches!(
            advance(
                &mut rec,
                &parse(outcome(
                    "one",
                    "confirmed",
                    Some(result("elsewhere", "sbx", None))
                )),
                mint
            ),
            Step::Failed { retire: false, .. }
        ));
        assert_eq!(rec, record(LaunchStage::Create));
    }

    #[test]
    fn rejection_is_final() {
        let mut rec = record(LaunchStage::Create);
        let mut rejected = parse(outcome("one", "rejected", None));
        rejected.error = Some("Quota exceeded.".into());
        let Step::Failed { message, retire } = advance(&mut rec, &rejected, || unreachable!())
        else {
            panic!()
        };
        assert!(retire);
        assert!(
            message.starts_with("Quota exceeded.") && message.contains("no sandbox was created"),
            "{message}"
        );
    }

    #[test]
    fn controller_status_must_match_scope() {
        let status = json!({"role": "local_controller", "scope": {"projectId": "project", "deploymentId": "deployment"}});
        assert!(is_controller(&status, "project", "deployment"));
        assert!(!is_controller(&status, "project", "other"));
        let host = json!({"role": "scaffold_host", "scope": {"projectId": "project", "deploymentId": "deployment"}});
        assert!(!is_controller(&host, "project", "deployment"));
    }

    #[test]
    fn journal_round_trips_and_keeps_unreadable_entries() {
        let dir = tempfile::tempdir().unwrap();
        let journal = Journal::at(dir.path());
        assert_eq!(journal.load().unwrap(), None);
        let rec = record(LaunchStage::Attach {
            sandbox_id: "sbx".into(),
        });
        journal.save(&rec).unwrap();
        assert_eq!(journal.load().unwrap(), Some(rec));
        journal.clear().unwrap();
        journal.clear().unwrap();
        std::fs::write(dir.path().join(JOURNAL_FILE), b"{").unwrap();
        assert!(matches!(journal.load(), Err(ClientError::Storage(_))));
        assert!(
            dir.path().join(JOURNAL_FILE).exists(),
            "never discarded on its own"
        );
    }

    #[test]
    fn one_launch_per_profile() {
        let dir = tempfile::tempdir().unwrap();
        let first = Running::acquire(dir.path()).unwrap();
        assert!(Running::acquire(dir.path()).is_err());
        drop(first);
        drop(Running::acquire(dir.path()).unwrap());
    }

    type Responder = Box<dyn FnMut(usize, &str, &RetainedLaunch) -> Result<Value> + Send>;

    /// A scripted controller. Every control/reconcile reads the journal first
    /// (a command is always journaled before it is sent) and answers through
    /// `respond(call index, "create" | "attach" | "reconcile", journaled)`.
    struct Fake {
        dir: PathBuf,
        respond: Mutex<Responder>,
        calls: Mutex<Vec<String>>,
        consumed: Mutex<HashSet<String>>,
        ready_after: Mutex<usize>,
        started: Mutex<Vec<(String, ChatConfig, String)>>,
        dispatched: Mutex<Vec<RetainedLaunch>>,
    }

    impl Fake {
        fn new(
            dir: &Path,
            respond: impl FnMut(usize, &str, &RetainedLaunch) -> Result<Value> + Send + 'static,
        ) -> Self {
            Self {
                dir: dir.to_path_buf(),
                respond: Mutex::new(Box::new(respond)),
                calls: Mutex::default(),
                consumed: Mutex::default(),
                ready_after: Mutex::default(),
                started: Mutex::default(),
                dispatched: Mutex::default(),
            }
        }

        fn calls(&self) -> Vec<String> {
            lock(&self.calls).clone()
        }

        fn answer(&self, kind: &str, command_id: &str) -> Result<Value> {
            let journaled = Journal::at(&self.dir)
                .load()
                .unwrap()
                .expect("journaled before it is sent");
            assert_eq!(
                journaled.command_id, command_id,
                "the journal names the command in flight"
            );
            let index = {
                let mut calls = lock(&self.calls);
                calls.push(format!("{kind}:{}:{command_id}", journaled.controller));
                calls.iter().filter(|call| *call != "controller").count() - 1
            };
            (lock(&self.respond))(index, kind, &journaled)
        }
    }

    #[async_trait::async_trait]
    impl LaunchPort for Fake {
        async fn controller(&self) -> Result<String> {
            lock(&self.calls).push("controller".into());
            Ok("mac".into())
        }
        async fn control(
            &self,
            controller: &str,
            command_id: &str,
            control: ScaffoldEnvironmentControl,
        ) -> Result<Value> {
            assert_eq!(controller, "mac");
            let journaled = Journal::at(&self.dir)
                .load()
                .unwrap()
                .expect("journaled before it is sent");
            lock(&self.dispatched).push(journaled.clone());
            let kind = match &control {
                ScaffoldEnvironmentControl::Create {
                    scope,
                    database_environment,
                    agent_route,
                    ..
                } => {
                    assert_eq!(
                        scope.session_id.as_deref(),
                        Some(journaled.session_id.as_str())
                    );
                    assert_eq!(*database_environment, journaled.database);
                    assert_eq!(agent_route.omp_model(), "openai-codex/gpt-5.5");
                    "create"
                }
                ScaffoldEnvironmentControl::Attach { sandbox_id, scope } => {
                    assert_eq!(
                        journaled.stage,
                        LaunchStage::Attach {
                            sandbox_id: sandbox_id.clone()
                        }
                    );
                    assert_eq!(
                        scope.session_id.as_deref(),
                        Some(journaled.session_id.as_str())
                    );
                    "attach"
                }
                other => panic!("unexpected control {other:?}"),
            };
            self.answer(kind, command_id)
        }
        async fn reconcile(&self, controller: &str, command_id: &str) -> Result<Value> {
            assert_eq!(controller, "mac");
            self.answer("reconcile", command_id)
        }
        fn consumed(&self, session_id: &str) -> bool {
            lock(&self.consumed).contains(session_id)
        }
        fn ready(&self, _: &str, host: &str) -> bool {
            assert_eq!(host, "host");
            let mut left = lock(&self.ready_after);
            if *left == 0 {
                return true;
            }
            *left -= 1;
            false
        }
        fn start(&self, session_id: &str, config: &ChatConfig, first: SendRequest) -> Result<()> {
            lock(&self.started).push((session_id.into(), config.clone(), first.text));
            Ok(())
        }
    }

    fn launch(database: ScaffoldDatabaseEnvironment) -> CrewLaunch {
        let mut config = config(HarnessId::Omp, "openai-codex/gpt-5.5");
        config.reasoning = Some(zeron_proto::ReasoningLevel::High);
        CrewLaunch {
            database,
            config,
            first: SendRequest::text("hello"),
        }
    }

    fn base_scope() -> CollaborationScope {
        let mut scope = scope("unused");
        scope.session_id = None;
        scope
    }

    type Phases = Arc<Mutex<Vec<CrewLaunchPhase>>>;

    fn phases() -> (Phases, impl Fn(CrewLaunchPhase) + Send + Sync) {
        let seen: Phases = Arc::default();
        let sink = seen.clone();
        (seen, move |phase| {
            let mut seen = lock(&sink);
            if seen.last() != Some(&phase) {
                seen.push(phase);
            }
        })
    }

    fn timed_out() -> Result<Value> {
        Err(ClientError::HostUnavailable(
            "scaffold.control outcome unknown; reconcile command id".into(),
        ))
    }

    fn created(journaled: &RetainedLaunch) -> Result<Value> {
        Ok(outcome(
            &journaled.command_id,
            "confirmed",
            Some(result(&journaled.session_id, "sbx", None)),
        ))
    }

    fn attached(journaled: &RetainedLaunch) -> Result<Value> {
        Ok(outcome(
            &journaled.command_id,
            "confirmed",
            Some(result(&journaled.session_id, "sbx", Some("host"))),
        ))
    }

    #[tokio::test(start_paused = true)]
    async fn launch_creates_attaches_waits_and_sends_once() {
        let dir = tempfile::tempdir().unwrap();
        let fake = Fake::new(dir.path(), |index, kind, journaled| match (index, kind) {
            // The create outlives the relay deadline; a reconcile blocks on it.
            (0, "create") | (1, "reconcile") => timed_out(),
            (2, "reconcile") => created(journaled),
            // Native host authority isn't confirmed on the first answer.
            (3, "attach") => Ok(outcome(&journaled.command_id, "unknown", None)),
            (4, "reconcile") => attached(journaled),
            other => panic!("unexpected call {other:?}"),
        });
        *lock(&fake.ready_after) = 3;
        let (seen, phase) = phases();
        let session = run(
            &fake,
            &Journal::at(dir.path()),
            &base_scope(),
            launch(ScaffoldDatabaseEnvironment::StagingSnapshot),
            &phase,
        )
        .await
        .unwrap();

        let calls = fake.calls();
        assert_eq!(calls.len(), 6, "{calls:?}");
        assert_eq!(calls[0], "controller");
        let dispatched = lock(&fake.dispatched).clone();
        assert_eq!(dispatched.len(), 2, "one create, one attach: {calls:?}");
        assert_eq!(dispatched[0].stage, LaunchStage::Create);
        assert_eq!(
            dispatched[0].database,
            ScaffoldDatabaseEnvironment::StagingSnapshot
        );
        assert_eq!(
            dispatched[1].stage,
            LaunchStage::Attach {
                sandbox_id: "sbx".into()
            }
        );
        assert_ne!(
            dispatched[0].command_id, dispatched[1].command_id,
            "attach is its own command"
        );
        assert_eq!(dispatched[0].session_id, session);
        // Unknown outcomes were only reconciled, under the command they owe.
        assert_eq!(
            calls[2],
            format!("reconcile:mac:{}", dispatched[0].command_id)
        );
        assert_eq!(
            calls[5],
            format!("reconcile:mac:{}", dispatched[1].command_id)
        );

        let started = lock(&fake.started).clone();
        assert_eq!(started.len(), 1);
        assert_eq!(started[0].0, session);
        assert_eq!(
            started[0].1.reasoning,
            Some(zeron_proto::ReasoningLevel::High),
            "the picked settings go with it"
        );
        assert_eq!(started[0].2, "hello");
        assert_eq!(
            Journal::at(dir.path()).load().unwrap(),
            None,
            "a finished launch is forgotten"
        );
        assert_eq!(
            *lock(&seen),
            [
                CrewLaunchPhase::Creating,
                CrewLaunchPhase::Starting,
                CrewLaunchPhase::Connecting
            ]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_retained_launch_is_reconciled_not_recreated() {
        let dir = tempfile::tempdir().unwrap();
        let journal = Journal::at(dir.path());
        let retained = record(LaunchStage::Create);
        journal.save(&retained).unwrap();
        let fake = Fake::new(dir.path(), |index, kind, journaled| match (index, kind) {
            (0, "reconcile") => created(journaled),
            (1, "attach") => attached(journaled),
            other => panic!("unexpected call {other:?}"),
        });
        let (_, phase) = phases();
        let session = run(
            &fake,
            &journal,
            &base_scope(),
            launch(ScaffoldDatabaseEnvironment::Local),
            &phase,
        )
        .await
        .unwrap();
        assert_eq!(
            session, retained.session_id,
            "the same session, not a new sandbox"
        );
        let calls = fake.calls();
        assert_eq!(calls[0], "reconcile:mac:one", "{calls:?}");
        assert!(
            !calls
                .iter()
                .any(|call| call == "controller" || call.starts_with("create")),
            "{calls:?}"
        );
        assert_eq!(lock(&fake.started).len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_retained_launch_for_another_database_blocks_a_new_one() {
        let dir = tempfile::tempdir().unwrap();
        let journal = Journal::at(dir.path());
        journal.save(&record(LaunchStage::Create)).unwrap();
        let fake = Fake::new(dir.path(), |_, kind, _| panic!("no {kind} expected"));
        let (_, phase) = phases();
        let error = run(
            &fake,
            &journal,
            &base_scope(),
            launch(ScaffoldDatabaseEnvironment::ProductionSnapshot),
            &phase,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("Scaffold (Local DB)"), "{error}");
        assert!(fake.calls().is_empty());
        assert!(journal.load().unwrap().is_some());
    }

    #[tokio::test(start_paused = true)]
    async fn a_consumed_retained_launch_starts_fresh() {
        let dir = tempfile::tempdir().unwrap();
        let journal = Journal::at(dir.path());
        let stale = record(LaunchStage::Attach {
            sandbox_id: "old".into(),
        });
        journal.save(&stale).unwrap();
        let fake = Fake::new(dir.path(), |index, kind, journaled| match (index, kind) {
            (0, "create") => created(journaled),
            (1, "attach") => attached(journaled),
            other => panic!("unexpected call {other:?}"),
        });
        // Its first message went out before the journal was cleared.
        lock(&fake.consumed).insert(stale.session_id.clone());
        let (_, phase) = phases();
        let fresh = run(
            &fake,
            &journal,
            &base_scope(),
            launch(ScaffoldDatabaseEnvironment::Local),
            &phase,
        )
        .await
        .unwrap();
        assert_ne!(fresh, stale.session_id);
        let calls = fake.calls();
        assert_eq!(calls[0], "controller", "{calls:?}");
        assert!(
            !calls.iter().any(|call| call.ends_with(":one")),
            "the finished launch is never reconciled: {calls:?}"
        );
        assert_eq!(
            lock(&fake.started)
                .iter()
                .map(|s| s.0.clone())
                .collect::<Vec<_>>(),
            [fresh]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_command_the_controller_never_saw_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let journal = Journal::at(dir.path());
        journal.save(&record(LaunchStage::Create)).unwrap();
        let fake = Fake::new(dir.path(), |_, kind, _| {
            assert_eq!(kind, "reconcile");
            Err(ClientError::HostError(
                "scaffold.reconcile: Unknown Crew command".into(),
            ))
        });
        let (_, phase) = phases();
        let error = run(
            &fake,
            &journal,
            &base_scope(),
            launch(ScaffoldDatabaseEnvironment::Local),
            &phase,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("nothing was created"), "{error}");
        assert_eq!(journal.load().unwrap(), None);
        assert_eq!(fake.calls().len(), UNADMITTED_ANSWERS);
    }

    #[tokio::test(start_paused = true)]
    async fn a_refused_dispatch_is_checked_once_then_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let journal = Journal::at(dir.path());
        let fake = Fake::new(dir.path(), |index, kind, _| match (index, kind) {
            (0, "create") => Err(ClientError::HostError(
                "scaffold.control: Wrong Crew project, deployment, or session".into(),
            )),
            (1, "reconcile") => Err(ClientError::HostError(
                "scaffold.reconcile: Unknown Crew command".into(),
            )),
            other => panic!("unexpected call {other:?}"),
        });
        let (_, phase) = phases();
        let error = run(
            &fake,
            &journal,
            &base_scope(),
            launch(ScaffoldDatabaseEnvironment::Local),
            &phase,
        )
        .await
        .unwrap_err();
        let ClientError::HostError(message) = error else {
            panic!("{error}")
        };
        assert_eq!(
            message,
            "Wrong Crew project, deployment, or session. Nothing was created; your draft is kept."
        );
        assert_eq!(journal.load().unwrap(), None);
    }

    #[tokio::test(start_paused = true)]
    async fn a_refused_dispatch_that_was_admitted_is_followed() {
        let dir = tempfile::tempdir().unwrap();
        let journal = Journal::at(dir.path());
        let fake = Fake::new(dir.path(), |index, kind, journaled| {
            match (index, kind) {
            (0, "create") => Err(ClientError::HostError("scaffold.control: Sandbox outcome is durable; its session reference awaits registry synchronization".into())),
            (1, "reconcile") => created(journaled),
            (2, "attach") => attached(journaled),
            other => panic!("unexpected call {other:?}"),
        }
        });
        let (_, phase) = phases();
        run(
            &fake,
            &journal,
            &base_scope(),
            launch(ScaffoldDatabaseEnvironment::Local),
            &phase,
        )
        .await
        .unwrap();
        assert_eq!(lock(&fake.dispatched).len(), 2);
        assert_eq!(lock(&fake.started).len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_controller_still_syncing_is_asked_again() {
        let dir = tempfile::tempdir().unwrap();
        let journal = Journal::at(dir.path());
        journal.save(&record(LaunchStage::Create)).unwrap();
        let fake = Fake::new(dir.path(), |index, kind, journaled| {
            match (index, kind) {
            (0 | 1, "reconcile") => Err(ClientError::HostError(
                "scaffold.reconcile: Sandbox outcome is durable; its session reference awaits registry synchronization".into(),
            )),
            (2, "reconcile") => created(journaled),
            (3, "attach") => attached(journaled),
            other => panic!("unexpected call {other:?}"),
        }
        });
        let (_, phase) = phases();
        run(
            &fake,
            &journal,
            &base_scope(),
            launch(ScaffoldDatabaseEnvironment::Local),
            &phase,
        )
        .await
        .unwrap();
        assert_eq!(lock(&fake.started).len(), 1);
        assert_eq!(journal.load().unwrap(), None);
    }

    #[tokio::test(start_paused = true)]
    async fn a_controller_that_keeps_refusing_hands_back_the_owed_command() {
        let dir = tempfile::tempdir().unwrap();
        let journal = Journal::at(dir.path());
        journal.save(&record(LaunchStage::Create)).unwrap();
        let fake = Fake::new(dir.path(), |_, kind, _| {
            assert_eq!(kind, "reconcile");
            Err(ClientError::HostError(
                "scaffold.reconcile: Multiple sandboxes match the command; explicit reconciliation required".into(),
            ))
        });
        let (_, phase) = phases();
        let error = run(
            &fake,
            &journal,
            &base_scope(),
            launch(ScaffoldDatabaseEnvironment::Local),
            &phase,
        )
        .await
        .unwrap_err();
        assert!(
            error.to_string().contains("Multiple sandboxes match"),
            "{error}"
        );
        assert_eq!(fake.calls().len(), REFUSED_RECONCILES);
        assert_eq!(
            journal.load().unwrap(),
            Some(record(LaunchStage::Create)),
            "never dropped while owed"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn an_unreachable_controller_keeps_the_command_past_the_deadline() {
        let dir = tempfile::tempdir().unwrap();
        let journal = Journal::at(dir.path());
        journal.save(&record(LaunchStage::Create)).unwrap();
        let fake = Fake::new(dir.path(), |_, kind, _| {
            assert_eq!(kind, "reconcile");
            Err(ClientError::HostUnavailable("mac: offline".into()))
        });
        let (_, phase) = phases();
        let error = run(
            &fake,
            &journal,
            &base_scope(),
            launch(ScaffoldDatabaseEnvironment::Local),
            &phase,
        )
        .await
        .unwrap_err();
        assert!(
            error.to_string().contains("nothing will be created twice"),
            "{error}"
        );
        assert_eq!(
            journal.load().unwrap(),
            Some(record(LaunchStage::Create)),
            "still owed"
        );
        let calls = fake.calls();
        assert!(
            calls.len() > 100 && calls.iter().all(|call| call == "reconcile:mac:one"),
            "{}",
            calls.len()
        );
    }

    #[tokio::test(start_paused = true)]
    async fn rejected_create_is_forgotten_and_never_attached() {
        let dir = tempfile::tempdir().unwrap();
        let journal = Journal::at(dir.path());
        journal.save(&record(LaunchStage::Create)).unwrap();
        let fake = Fake::new(dir.path(), |_, kind, journaled| {
            assert_eq!(kind, "reconcile");
            Ok(
                json!({"commandId": journaled.command_id, "status": "rejected", "error": "No capacity."}),
            )
        });
        let (_, phase) = phases();
        let error = run(
            &fake,
            &journal,
            &base_scope(),
            launch(ScaffoldDatabaseEnvironment::Local),
            &phase,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("No capacity."), "{error}");
        assert_eq!(journal.load().unwrap(), None);
        assert!(lock(&fake.started).is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn an_attached_host_that_never_comes_online_keeps_the_launch() {
        let dir = tempfile::tempdir().unwrap();
        let journal = Journal::at(dir.path());
        let retained = record(LaunchStage::Attach {
            sandbox_id: "sbx".into(),
        });
        journal.save(&retained).unwrap();
        let fake = Fake::new(dir.path(), |_, kind, journaled| {
            assert_eq!(kind, "reconcile");
            attached(journaled)
        });
        *lock(&fake.ready_after) = usize::MAX;
        let (seen, phase) = phases();
        let error = run(
            &fake,
            &journal,
            &base_scope(),
            launch(ScaffoldDatabaseEnvironment::Local),
            &phase,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("isn't online yet"), "{error}");
        assert_eq!(
            journal.load().unwrap(),
            Some(retained),
            "the next send resumes it"
        );
        assert!(lock(&fake.started).is_empty());
        assert_eq!(lock(&seen).last(), Some(&CrewLaunchPhase::Connecting));
    }

    #[tokio::test(start_paused = true)]
    async fn an_unmanaged_model_never_reaches_a_controller() {
        let dir = tempfile::tempdir().unwrap();
        let fake = Fake::new(dir.path(), |_, kind, _| panic!("no {kind} expected"));
        let mut unmanaged = launch(ScaffoldDatabaseEnvironment::Local);
        unmanaged.config = config(HarnessId::ClaudeCode, "claude-opus-5");
        let (_, phase) = phases();
        let error = run(
            &fake,
            &Journal::at(dir.path()),
            &base_scope(),
            unmanaged,
            &phase,
        )
        .await
        .unwrap_err();
        assert!(matches!(error, ClientError::InvalidArgument(_)), "{error}");
        assert!(fake.calls().is_empty());
    }
}
