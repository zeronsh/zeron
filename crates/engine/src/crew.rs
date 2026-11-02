//! Crew control-plane integration on the upstream command ledger, not a shared CRDT.
use std::sync::Arc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::Mutex;
use tracing::Instrument;
use zeron_harness::{CancellationToken, InferenceRoute};
use zeron_proto::{AgentRoute, CollaborationScope, HarnessId, ScaffoldDatabaseEnvironment,
    ScaffoldEnvironmentControl as Control, ScaffoldEnvironmentControlResult as ControlResult,
    ScaffoldLifecycle, SessionEnvironment, SessionEnvironmentSource};
use zeron_rpc::RpcError;
use zeron_sync::DocsStore;
use crate::{EngineError, auth::Auth, inference_relay::InferenceRelay,
    scaffold::{ScaffoldError, ScaffoldRuntime}};

#[derive(Clone)]
pub struct CrewServices {
    auth: Auth,
    profile_key: String,
    runtime: ScaffoldRuntime,
    store: Arc<DocsStore>,
    relay: InferenceRelay,
    // ponytail: serialize control operations per controller; per-sandbox locks if needed.
    control_gate: Arc<Mutex<()>>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Command {
    principal_id: String,
    control: Control,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlOutcome {
    pub command_id: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<ControlResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HandoffRequest {
    pub command_id: String,
    pub source_session_id: String,
    pub native_session_id: String,
    pub cwd: String,
    pub prompt: String,
    pub agent_route: AgentRoute,
    pub database_environment: ScaffoldDatabaseEnvironment,
    #[serde(default)]
    pub session_only: bool,
    pub recover_sandbox_id: Option<String>,
    pub recover_chat_id: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HandoffReceipt {
    pub chat_id: String,
    pub sandbox_id: String,
    pub command_id: String,
    pub environment: SessionEnvironment,
    pub attachment: ControlResult,
    pub native_session_id: String,
    pub cwd: String,
}

impl CrewServices {
    pub fn new(auth: Auth, runtime: ScaffoldRuntime, store: Arc<DocsStore>) -> Result<Self, EngineError> {
        let profile_key = auth.verified_profile_key()
            .ok_or_else(|| EngineError::Other("Crew services require a verified profile".into()))?;
        let relay = InferenceRelay::start(runtime.client())?;
        Ok(Self { auth, profile_key, runtime, store, relay, control_gate: Arc::new(Mutex::new(())) })
    }

    pub fn runtime(&self) -> &ScaffoldRuntime { &self.runtime }

    pub fn session_metadata(&self, session_id: &str) -> Result<zeron_proto::CrewChatMetadata, EngineError> {
        if !zeron_proto::is_cloud_session_id(session_id) {
            return Err(EngineError::Other("Local history has no cloud session identity".into()));
        }
        let (owner_subject, mut scope) = self.identity().map_err(|error| EngineError::Other(error.to_string()))?;
        if scope.session_id.as_deref().is_some_and(|bound| bound != session_id) {
            return Err(EngineError::Other("Session differs from the verified device binding".into()));
        }
        scope.session_id = Some(session_id.to_owned());
        Ok(zeron_proto::CrewChatMetadata {
            owner_subject, scope, session_id: session_id.to_owned(), environment: None, agent_route: None,
            original_epoch_history: None,
        })
    }

    pub fn adopt_chat(&self, workspace: &crate::WorkspaceHost, chat_id: &str) -> Result<zeron_proto::Chat, EngineError> {
        let mut chat = workspace.chat(chat_id)?
            .ok_or_else(|| EngineError::Other("Crew session is not present in this profile".into()))?;
        if zeron_proto::is_local_history_chat(chat_id) {
            let (_, scope) = self.identity().map_err(|error| EngineError::Other(error.to_string()))?;
            if scope.session_id.is_some() || self.auth.device_mode() || !workspace.is_host(chat_id)
                || chat.crew.is_some()
                || chat.config.as_ref().is_none_or(|config| config.harness != HarnessId::Omp)
                || chat.harness_session_id.as_deref().is_none_or(|id| id.trim().is_empty())
                || chat.cwd.as_deref().is_none_or(|cwd| cwd.trim().is_empty())
            {
                return Err(EngineError::Other("Local history requires its imported native session on this host".into()));
            }
            return Ok(chat);
        }
        let expected = self.session_metadata(chat_id)?;
        if let Some(metadata) = &chat.crew {
            metadata.validate().map_err(|error| EngineError::Other(error.into()))?;
            if metadata.owner_subject != expected.owner_subject || metadata.scope != expected.scope {
                return Err(EngineError::Other("Crew session belongs to another owner or deployment".into()));
            }
        } else {
            chat.crew = Some(expected);
            workspace.import_chat_row(&chat)?;
        }
        Ok(chat)
    }

    pub fn ensure_identity(&self) -> Result<(), EngineError> {
        self.identity().map(|_| ()).map_err(|error| EngineError::Other(error.to_string()))
    }

    pub async fn generate_title(&self, chat_id: &str, prompt: &str) -> Result<String, EngineError> {
        if !zeron_proto::is_cloud_session_id(chat_id) {
            return Err(EngineError::Other("Local history cannot use cloud session titles".into()));
        }
        let (_, scope) = self.identity().map_err(|error| EngineError::Other(error.to_string()))?;
        if self.auth.device_mode() && scope.session_id.as_deref() != Some(chat_id) {
            return Err(EngineError::Other("Sandbox title request belongs to another session".into()));
        }
        self.runtime.client().generate_crew_title(chat_id, Some(self.runtime.deployment_id()), prompt)
            .await.map_err(|error| EngineError::Other(error.to_string()))
    }

    fn identity(&self) -> Result<(String, CollaborationScope), RpcError> {
        if self.auth.verified_profile_key().as_deref() != Some(self.profile_key.as_str()) {
            return Err(denied("Crew profile changed; reopen its isolated store"));
        }
        let principal = self.auth.user_id().ok_or_else(|| denied("Crew sign-in required"))?;
        let scope = self.auth.verified_scope().ok_or_else(|| denied("Verified Crew scope required"))?;
        Ok((principal, scope))
    }

    pub(crate) fn controller(&self) -> Result<(String, CollaborationScope), RpcError> {
        let identity = self.identity()?;
        if self.auth.device_mode() { return Err(denied("Sandbox hosts cannot control other environments or shared accounts")); }
        Ok(identity)
    }

    fn validate_scope(&self, scope: &CollaborationScope) -> Result<String, RpcError> {
        let (principal, verified) = self.controller()?;
        if scope.project_id != verified.project_id
            || scope.deployment_id.as_deref() != Some(self.runtime.deployment_id())
            || scope.deployment_id != verified.deployment_id
            || scope.session_id.as_deref().is_none_or(|id| id.trim().is_empty())
        { return Err(denied("Wrong Crew project, deployment, or session")); }
        Ok(principal)
    }

    fn validate_environment(&self, environment: &SessionEnvironment, scope: &CollaborationScope) -> Result<(), RpcError> {
        self.validate_scope(scope)?;
        let owner = self.auth.verified_owner_email().ok_or_else(|| denied("Verified sandbox owner unavailable"))?;
        if environment.scope != *scope || !environment.owner_principal.eq_ignore_ascii_case(&owner) {
            return Err(denied("Sandbox owner or scope changed"));
        }
        Ok(())
    }

    /// Host-only launch context. Unsupported managed routes fail rather than inheriting credentials.
    pub(crate) async fn prepare_inference(&self, session_id: &str, harness: HarnessId,
        model: Option<&str>, account_id: Option<&str>,
        admission: Option<crate::inference_relay::RunInferenceAdmission>) -> Result<InferenceRoute, EngineError> {
        self.identity().map_err(|error| EngineError::Other(error.to_string()))?;
        if self.auth.device_mode() {
            let (_, scope) = self.identity().map_err(|error| EngineError::Other(error.to_string()))?;
            if scope.session_id.as_deref() != Some(session_id) || !matches!(harness, HarnessId::Omp | HarnessId::Antigravity) {
                return Err(EngineError::Other("Sandbox inference is bound to its managed session".into()));
            }
        }
        self.relay.prepare_with_admission(session_id, harness, model, account_id, admission).await?
            .ok_or_else(|| EngineError::Other("This harness/model is unavailable in Crew-managed mode".into()))
    }

    pub fn retire_inference(&self, route: &InferenceRoute) { self.relay.remove(&route.token); }

    pub(crate) fn device_grant_authority(&self) -> Option<crate::auth::DeviceGrantAuthority> {
        self.auth.device_grant_authority()
    }

    pub async fn control(&self, command_id: &str, control: Control, cancellation: &CancellationToken)
        -> Result<ControlOutcome, RpcError> {
        validate_id(command_id)?;
        let principal_id = self.validate_scope(control_scope(&control))?;
        let command = Command { principal_id, control };
        let bytes = serde_json::to_vec(&command).map_err(failed)?;
        let _gate = self.control_gate.lock().await;
        if !self.store.claim_crew_command(command_id, &bytes).map_err(failed)? {
            return self.outcome(command_id);
        }
        let scope = control_scope(&command.control);
        let before = if !matches!(command.control, Control::Inspect { .. })
            && let Some(id) = sandbox_id(&command.control) {
            match self.runtime.client().inspect(id, scope, cancellation).await {
                Ok(environment) => {
                    if let Err(error) = self.validate_environment(&environment, scope) {
                        return self.finish(command_id, None, Some(error.to_string()));
                    }
                    if !matches!(command.control, Control::Stop { .. } | Control::Start { .. })
                        && self.retired(id, scope, environment_epoch(&environment))? {
                        return self.finish(command_id, None, Some("Sandbox epoch recovery was retired by Stop".into()));
                    }
                    if let Control::Start { expected_lifecycle_epoch, .. } = &command.control
                        && (!terminal_environment(&environment) || environment_epoch(&environment) != Some(*expected_lifecycle_epoch)) {
                        return self.finish(command_id, None, Some("Start requires the exact observed stopped or failed epoch; use Resume for a paused host".into()));
                    }
                    Some(environment)
                }
                Err(error) => return self.record_error(command_id, error),
            }
        } else { None };
        if let Some(environment) = &before {
            self.store.claim_crew_command(&format!("{command_id}:binding"),
                &serde_json::to_vec(environment).map_err(failed)?).map_err(failed)?;
        }
        if matches!(command.control, Control::Stop { .. }) {
            self.retire_pending(command_id, &command)?;
        }
        let result = match self.runtime.control(command.control.clone(), cancellation)
            .instrument(tracing::info_span!("crew_control", command_id, sandbox_id = sandbox_id(&command.control)))
            .await {
            Ok(mut result) => {
                self.validate_environment(&result.environment, scope)?;
                if let Some(before) = before.as_ref() {
                    validate_transition(before, &result.environment)?;
                }
                if matches!(command.control, Control::Start { .. }) {
                    validate_start_transition(before.as_ref().ok_or_else(|| denied("Start requires its prior epoch"))?, &result.environment)?;
                    if !result.environment.unknown.get("startTransition").and_then(|value| value.get("ownerSubject")).and_then(Value::as_str)
                        .is_some_and(|owner| owner.eq_ignore_ascii_case(&result.environment.owner_principal)) {
                        return Err(denied("Start transition belongs to another verified owner"));
                    }
                    self.store.claim_crew_command(&format!("{command_id}:start-response"),
                        &serde_json::to_vec(&result.environment).map_err(failed)?).map_err(failed)?;
                    result = match self.runtime.control(Control::Attach {
                        sandbox_id: environment_id(&result.environment).ok_or_else(|| denied("Start has no sandbox"))?.into(),
                        scope: scope.clone(),
                    }, cancellation).await {
                        Ok(attached) => attached,
                        Err(error) => return Ok(unknown(command_id, &format!("Start was accepted but native attachment is unconfirmed: {error}"))),
                    };
                    self.validate_environment(&result.environment, scope)?;
                    self.validate_started_epoch(command_id, &result.environment)?;
                }
                // A mutation response is not proof of readiness. Inspect the authoritative state.
                if !matches!(command.control, Control::Inspect { .. })
                    && let Some(id) = environment_id(&result.environment) {
                    match self.runtime.client().inspect(id, scope, cancellation).await {
                        Ok(observed) => {
                            self.validate_environment(&observed, scope)?;
                            validate_transition(&result.environment, &observed)?;
                            if matches!(command.control, Control::Start { .. }) { self.validate_started_epoch(command_id, &observed)?; }
                            result.environment = observed;
                        }
                        Err(error) => return self.record_error(command_id, error),
                    }
                }
                result
            }
            Err(error) => return self.record_error(command_id, error),
        };
        if matches!(command.control, Control::Attach { .. } | Control::Resume { .. } | Control::Start { .. }) {
            return match self.runtime.observe_attachment(result.environment, cancellation).await.map_err(failed)? {
                Some(mut observed) => {
                    observed.run_id = result.run_id;
                    self.finish(command_id, Some(observed), None)
                }
                None => Ok(unknown(command_id, "Native host authority is not yet confirmed for the observed lifecycle")),
            };
        }
        if !operation_observed(&command.control, &result.environment) {
            return Ok(unknown(command_id, "Platform accepted the operation; outcome is not yet confirmed"));
        }
        self.finish(command_id, Some(result), None)
    }

    fn finish(&self, id: &str, result: Option<ControlResult>, error: Option<String>) -> Result<ControlOutcome, RpcError> {
        let outcome = ControlOutcome { command_id: id.into(), status: if error.is_some() { "rejected" } else { "confirmed" }.into(), result, error };
        self.store.finish_crew_command(id, &serde_json::to_vec(&outcome).map_err(failed)?).map_err(failed)?;
        Ok(outcome)
    }

    fn record_error(&self, id: &str, error: ScaffoldError) -> Result<ControlOutcome, RpcError> {
        if let ScaffoldError::RepositoryImport { diagnostic, .. } = &error {
            persist_repository_diagnostic(&self.store, id, diagnostic)?;
        }
        if terminal_control_error(&error) {
            self.finish(id, None, Some(error.to_string()))
        } else { Ok(unknown(id, &error.to_string())) }
    }

    pub fn outcome(&self, command_id: &str) -> Result<ControlOutcome, RpcError> {
        let (principal, _) = self.controller()?;
        let (request, outcome) = self.store.crew_command(command_id).map_err(failed)?
            .ok_or_else(|| RpcError::BadParams("Unknown Crew command".into()))?;
        let command: Command = serde_json::from_slice(&request).map_err(failed)?;
        if command.principal_id != principal { return Err(denied("Command belongs to another owner")); }
        self.validate_scope(control_scope(&command.control))?;
        outcome.map(|bytes| serde_json::from_slice(&bytes).map_err(failed))
            .unwrap_or_else(|| Ok(unknown(command_id, "Command delivery is pending or its outcome is unknown; reconcile without replay")))
    }

    /// Reconcile only by observation. Never resubmit an unknown create, resume, attach, or exec.
    pub async fn reconcile(&self, command_id: &str, cancellation: &CancellationToken) -> Result<ControlOutcome, RpcError> {
        let _gate = self.control_gate.lock().await;
        let old = self.outcome(command_id)?;
        if old.status != "unknown" { return Ok(old); }
        let (request, _) = self.store.crew_command(command_id).map_err(failed)?.expect("outcome read command");
        let command: Command = serde_json::from_slice(&request).map_err(failed)?;
        let scope = control_scope(&command.control);
        let environment = if let Some(id) = sandbox_id(&command.control) {
            self.runtime.client().inspect(id, scope, cancellation).await.map_err(failed)?
        } else {
            let matches = self.runtime.client().list(scope, cancellation).await.map_err(failed)?;
            let mut matches = matches.into_iter().filter(|environment| environment.scope == *scope);
            let Some(environment) = matches.next() else { return Ok(old) };
            if matches.next().is_some() { return Err(denied("Multiple sandboxes match the command; explicit reconciliation required")); }
            environment
        };
        self.validate_environment(&environment, scope)?;
        if !matches!(command.control, Control::Stop { .. } | Control::Start { .. })
            && let Some(id) = environment_id(&environment)
            && self.retired(id, scope, environment_epoch(&environment))? {
            return self.finish(command_id, None, Some("Recovery retired by Stop".into()));
        }
        if matches!(command.control, Control::Start { .. }) {
            // Only an actual saved Start response can confirm the new epoch; observation alone cannot.
            if self.store.crew_command(&format!("{command_id}:start-response")).map_err(failed)?.is_none() { return Ok(old); }
            self.validate_started_epoch(command_id, &environment)?;
        }
        if matches!(command.control, Control::Create { .. } | Control::Attach { .. } | Control::Resume { .. } | Control::Start { .. })
            && let SessionEnvironmentSource::Scaffold { lifecycle, .. } = &environment.source
            && matches!(lifecycle, ScaffoldLifecycle::Failed | ScaffoldLifecycle::Stopped) {
            return self.finish(command_id, None, Some(format!(
                "Creation/attachment/resume recovery retired because the sandbox is {lifecycle:?}; no command was replayed."
            )));
        }
        if let Some((binding, _)) = self.store.crew_command(&format!("{command_id}:binding")).map_err(failed)? {
            let before: SessionEnvironment = serde_json::from_slice(&binding).map_err(failed)?;
            validate_transition(&before, &environment)?;
        }
        if matches!(command.control, Control::Attach { .. } | Control::Resume { .. } | Control::Start { .. }) {
            return match self.runtime.observe_attachment(environment, cancellation).await.map_err(failed)? {
                Some(result) => self.finish(command_id, Some(result), None),
                None => Ok(old),
            };
        }
        // A read-only inspect cannot prove an archive transfer. Route recovery
        // requires the authoritative route, not merely a running sandbox.
        if matches!(command.control, Control::HandoffOmpSession { .. })
            || matches!(&command.control, Control::UpdateAgentRoute { agent_route, .. }
                if environment.unknown.get("agentRoute")
                    .and_then(|value| serde_json::from_value::<AgentRoute>(value.clone()).ok()).as_ref() != Some(agent_route))
            || !operation_observed(&command.control, &environment) { return Ok(old); }
        let result = ControlResult { environment, preparation_generation: None, attached_device_id: None,
            run_id: None, room_projection: None, control_grant: None,
            handoff_native_session_id: None, handoff_cwd: None };
        self.finish(command_id, Some(result), None)
    }

    pub(crate) async fn reconcile_pending_batch(&self, after: &mut String) -> Result<Option<Vec<ControlResult>>, RpcError> {
        let (principal, _) = self.controller()?;
        let cancellation = CancellationToken::new();
        let commands = self.store.pending_crew_commands(after, 64).map_err(failed)?;
        if commands.is_empty() { return Ok(None); }
        let mut results = Vec::new();
        for (id, request) in commands {
            after.clone_from(&id);
            let Ok(command) = serde_json::from_slice::<Command>(&request) else { continue };
            if command.principal_id != principal { continue; }
            match tokio::time::timeout(std::time::Duration::from_secs(30), self.reconcile(&id, &cancellation)).await {
                Ok(Ok(outcome)) => if let Some(result) = outcome.result { results.push(result); },
                Ok(Err(error)) => tracing::warn!(command = %id, %error, "Crew recovery remains unconfirmed"),
                Err(_) => tracing::warn!(command = %id, "Crew observation timed out; no mutation replayed"),
            }
        }
        Ok(Some(results))
    }

    fn retired(&self, sandbox: &str, scope: &CollaborationScope, epoch: Option<u64>) -> Result<bool, RpcError> {
        let commands = self.store.crew_commands().map_err(failed)?;
        let explicitly_started = commands.iter().any(|(id, request, _)| {
            let Ok(command) = serde_json::from_slice::<Command>(request) else { return false };
            matches!(&command.control, Control::Start { sandbox_id, scope: started, .. } if sandbox_id == sandbox && started == scope)
                && self.store.crew_command(&format!("{id}:start-response")).ok().flatten()
                    .and_then(|(bytes, _)| serde_json::from_slice::<SessionEnvironment>(&bytes).ok())
                    .is_some_and(|environment| environment_epoch(&environment) == epoch && epoch.is_some())
        });
        for (id, request, outcome) in commands {
            let Ok(command) = serde_json::from_slice::<Command>(&request) else { continue };
            if matches!(&command.control, Control::Stop { sandbox_id, scope: stopped } if sandbox_id == sandbox && stopped == scope) {
                let rejected = outcome.as_deref().and_then(|bytes| serde_json::from_slice::<ControlOutcome>(bytes).ok())
                    .is_some_and(|outcome| outcome.status == "rejected");
                if rejected { continue; }
                let stopped_epoch = self.store.crew_command(&format!("{id}:binding")).map_err(failed)?
                    .and_then(|(bytes, _)| serde_json::from_slice::<SessionEnvironment>(&bytes).ok())
                    .and_then(|environment| environment_epoch(&environment));
                if !(explicitly_started && stopped_epoch.zip(epoch).is_some_and(|(old, new)| old < new)) { return Ok(true); }
            }
        }
        Ok(false)
    }

    fn validate_started_epoch(&self, command_id: &str, environment: &SessionEnvironment) -> Result<(), RpcError> {
        let (bytes, _) = self.store.crew_command(&format!("{command_id}:start-response")).map_err(failed)?
            .ok_or_else(|| denied("No verified Start response; a larger observed epoch is not authorization"))?;
        let started: SessionEnvironment = serde_json::from_slice(&bytes).map_err(failed)?;
        validate_transition(&started, environment)?;
        if environment_epoch(&started).is_none() || environment_epoch(&started) != environment_epoch(environment) {
            return Err(denied("Start response was superseded by another epoch"));
        }
        Ok(())
    }

    pub(crate) async fn start_publication(&self, command_id: &str, environment: &SessionEnvironment)
        -> Result<Option<tokio::sync::OwnedMutexGuard<()>>, RpcError> {
        let (request, _) = self.store.crew_command(command_id).map_err(failed)?.ok_or_else(|| denied("Unknown control command"))?;
        let command: Command = serde_json::from_slice(&request).map_err(failed)?;
        if !matches!(command.control, Control::Start { .. }) { return Ok(None); }
        let gate = self.control_gate.clone().lock_owned().await;
        let outcome = self.outcome(command_id)?;
        let result = outcome.result.filter(|_| outcome.status == "confirmed").ok_or_else(|| denied("Start is not confirmed"))?;
        if result.environment != *environment || result.attached_device_id.as_deref()
            != environment_id(environment).zip(environment_epoch(environment))
                .map(|(sandbox, epoch)| crate::scaffold::device_id_for(sandbox, epoch)).as_deref() {
            return Err(denied("Start publication differs from its verified native host"));
        }
        self.validate_started_epoch(command_id, environment)?;
        let sandbox = environment_id(environment).ok_or_else(|| denied("Start sandbox is missing"))?;
        let cancellation = CancellationToken::new();
        let current = self.runtime.client().inspect(sandbox, &environment.scope, &cancellation).await.map_err(failed)?;
        self.validate_environment(&current, &environment.scope)?;
        self.validate_started_epoch(command_id, &current)?;
        if self.retired(sandbox, &current.scope, environment_epoch(&current))?
            || self.runtime.observe_attachment(current, &cancellation).await.map_err(failed)?.is_none() {
            return Err(denied("Start no longer owns a live verified native host"));
        }
        Ok(Some(gate))
    }

    fn retire_pending(&self, current: &str, stopping: &Command) -> Result<(), RpcError> {
        for (id, request, outcome) in self.store.crew_commands().map_err(failed)? {
            if id == current || outcome.is_some() { continue; }
            let Ok(command) = serde_json::from_slice::<Command>(&request) else { continue };
            if control_scope(&command.control) == control_scope(&stopping.control)
                && (sandbox_id(&command.control).is_none() || sandbox_id(&command.control) == sandbox_id(&stopping.control)) {
                self.finish(&id, None, Some("Recovery retired by Stop".into()))?;
            }
        }
        Ok(())
    }

    pub async fn dispatch(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        let cancellation = CancellationToken::new();
        match method {
            "crew.status" => {
                let (principal, mut scope) = self.identity()?;
                if scope.deployment_id.is_none() { scope.deployment_id = Some(self.runtime.deployment_id().into()); }
                Ok(json!({"principalId":principal,"scope":scope,"managed":true,
                    "role":if self.auth.device_mode(){"scaffold_host"}else{"local_controller"},
                    "runtimeVersion":crate::scaffold::SCAFFOLD_COMET_RUNTIME_VERSION.trim()}))
            }
            "scaffold.refresh" => {
                let scope: CollaborationScope = serde_json::from_value(params).map_err(bad)?;
                let mut validation_scope = scope.clone();
                validation_scope.session_id.get_or_insert_with(|| "list".into());
                self.validate_scope(&validation_scope)?;
                let snapshot = self.runtime.refresh(&scope, &cancellation).await.map_err(failed)?;
                for environment in &snapshot.environments { self.validate_environment(environment, &environment.scope)?; }
                serde_json::to_value(snapshot).map_err(failed)
            }
            "scaffold.control" => {
                #[derive(Deserialize)] #[serde(rename_all="camelCase")]
                struct Input { command_id: String, control: Control }
                let input: Input = serde_json::from_value(params).map_err(bad)?;
                serde_json::to_value(self.control(&input.command_id,input.control,&cancellation).await?).map_err(failed)
            }
            "scaffold.outcome" | "scaffold.reconcile" => {
                let id = string(&params,"commandId")?;
                let outcome = if method.ends_with("reconcile") { self.reconcile(id,&cancellation).await? } else { self.outcome(id)? };
                serde_json::to_value(outcome).map_err(failed)
            }
            "crew.directory.get" => {
                self.controller()?;
                let deployment = params.get("deploymentId").and_then(Value::as_str).unwrap_or(self.runtime.deployment_id());
                if deployment != self.runtime.deployment_id() { return Err(denied("Wrong directory deployment")); }
                self.runtime.client().get_crew_session(string(&params,"sessionId")?,Some(deployment)).await.map_err(failed)
            }
            "crew.accounts.list" => { self.controller()?; Ok(json!({"accounts":self.runtime.client().list_agent_accounts().await.map_err(failed)?})) }
            "crew.accounts.revoke" => { self.controller()?; self.runtime.client().revoke_agent_account(string(&params,"accountId")?).await.map_err(failed)?; Ok(json!({})) }
            "crew.route.receipt" => { self.controller()?; serde_json::to_value(self.runtime.client().get_agent_route_receipt(string(&params,"sessionId")?,&cancellation).await.map_err(failed)?).map_err(failed) }
            _ => Err(RpcError::UnknownMethod(method.into())),
        }
    }

    /// Local IPC only; never register this on a peer/device RPC service.
    pub async fn import_local_account(&self, accounts: &crate::agent_accounts::AgentAccounts,
        harness: HarnessId, account_id: &str) -> Result<Value, RpcError> {
        self.controller()?;
        serde_json::to_value(accounts.import_local_shared(&self.runtime.client(),harness,account_id).await.map_err(failed)?).map_err(failed)
    }

    pub(crate) async fn recover_handoff_request(&self, chat: &str, sandbox: &str) -> Result<HandoffRequest, RpcError> {
        let (principal, mut scope) = self.controller()?;
        scope.session_id = Some(chat.into());
        let owner = self.auth.verified_owner_email().ok_or_else(|| denied("Verified sandbox owner unavailable"))?;
        let (request, environment) = recover_handoff_request(&self.store, &principal, &scope, &owner, chat, sandbox)?;
        let observed = self.runtime.client().inspect(sandbox, &scope, &CancellationToken::new()).await.map_err(failed)?;
        self.validate_environment(&observed, &scope)?;
        validate_handoff_epoch(&environment, &observed)?;
        if !matches!(&observed.source, SessionEnvironmentSource::Scaffold { lifecycle: ScaffoldLifecycle::Ready | ScaffoldLifecycle::AgentRunning, .. }) {
            return Err(denied("Handoff target is not running"));
        }
        if self.retired(sandbox, &scope, environment_epoch(&environment))? {
            return Err(denied("Stopped handoff cannot recover"));
        }
        Ok(request)
    }
    /// Explicit recovery prepares a separate child; the ambiguous legacy transfer is never replayed.
    pub(crate) async fn recover_handoff(&self, request: HandoffRequest, chat: &str, sandbox: &str,
        cancellation: &CancellationToken) -> Result<HandoffReceipt, RpcError> {
        let (principal, mut scope) = self.controller()?;
        scope.session_id = Some(chat.into());
        let owner = self.auth.verified_owner_email().ok_or_else(|| denied("Verified sandbox owner unavailable"))?;
        let (original, environment) = recover_handoff_request(&self.store, &principal, &scope, &owner, chat, sandbox)?;
        if request != original {
            return Err(denied("Recovery cannot change the immutable handoff request"));
        }
        let root = format!("crew-handoff:{}", original.command_id);
        if let Some((_, Some(bytes))) = self.store.crew_command(&root).map_err(failed)? {
            let receipt: HandoffReceipt = serde_json::from_slice(&bytes).map_err(failed)?;
            self.admit_handoff(&receipt, || Ok(())).await?;
            return Ok(receipt);
        }
        let child = format!("{root}:isolated-recovery");
        let immutable = serde_json::to_vec(&json!({"principal":principal,"root":root,"environment":environment,
            "sourceSelector":original.native_session_id,"sourceCwd":original.cwd,"sessionOnly":original.session_only})).map_err(failed)?;
        self.store.claim_crew_command(&child, &immutable).map_err(failed)?;
        let (_, attachment) = self.store.crew_command(&format!("{root}:attach")).map_err(failed)?
            .ok_or_else(|| denied("Confirmed original attachment missing"))?;
        let attachment = confirmed(serde_json::from_slice(&attachment.ok_or_else(|| denied("Original attachment is unknown"))?).map_err(failed)?)?;
        self.validate_recovery_target(&environment, cancellation).await?;
        let capsule_root = self.store.profile_root().map_err(failed)?.join("native-handoff-recovery");
        let prepared = match self.runtime.recover_native_handoff(&child, &environment, &original.native_session_id,
            &original.cwd, original.session_only, &capsule_root, cancellation).await {
            Ok(prepared) => prepared,
            Err(ScaffoldError::IsolatedRecovery { stage, diagnostic }) => {
                let evidence = persist_isolated_recovery_diagnostic(&self.store, &child, &diagnostic)?;
                return Err(failed(format!("{stage}; diagnostic={evidence}; sandbox={sandbox}; {}", diagnostic.stderr)));
            }
            Err(error) => return Err(failed(error)),
        };
        // Stop owns this same gate. A guest receipt alone never authorizes local publication.
        let _gate = self.control_gate.lock().await;
        self.validate_recovery_target(&environment, cancellation).await?;
        let receipt = HandoffReceipt { chat_id: chat.into(), sandbox_id: sandbox.into(), command_id: original.command_id,
            environment, attachment, native_session_id: prepared.native_session_id.clone(),
            cwd: prepared.cwd.clone() };
        self.store.finish_crew_command(&child, &serde_json::to_vec(&prepared).map_err(failed)?).map_err(failed)?;
        self.store.finish_crew_command(&root, &serde_json::to_vec(&receipt).map_err(failed)?).map_err(failed)?;
        Ok(receipt)
    }

    async fn validate_recovery_target(&self, before: &SessionEnvironment, cancellation: &CancellationToken) -> Result<(), RpcError> {
        let sandbox = environment_id(before).ok_or_else(|| denied("Handoff sandbox missing"))?;
        self.validate_environment(before, &before.scope)?;
        let observed = self.runtime.client().inspect(sandbox, &before.scope, cancellation).await.map_err(failed)?;
        self.validate_environment(&observed, &before.scope)?;
        validate_handoff_epoch(before, &observed)?;
        if !matches!(&observed.source, SessionEnvironmentSource::Scaffold { lifecycle: ScaffoldLifecycle::Ready | ScaffoldLifecycle::AgentRunning, .. })
            || self.retired(sandbox, &before.scope, environment_epoch(before))? {
            return Err(denied("Stopped or superseded handoff cannot recover"));
        }
        Ok(())
    }

    pub(crate) fn cleanup_handoff_recovery(&self, command: &str) -> Result<(), RpcError> {
        let root = self.store.profile_root().map_err(failed)?.join("native-handoff-recovery");
        self.runtime.cleanup_native_handoff_recovery(&format!("crew-handoff:{command}:isolated-recovery"), &root).map_err(failed)
    }


    /// Prepare a completed-turn native fork. Keep the source writer alive;
    /// publish the target and durably admit the original immutable command ID.
    pub async fn prepare_handoff(&self, request: HandoffRequest, cancellation: &CancellationToken) -> Result<HandoffReceipt, RpcError> {
        validate_id(&request.command_id)?;
        if request.prompt.trim().is_empty() || request.native_session_id.trim().is_empty() || request.cwd.trim().is_empty() {
            return Err(RpcError::BadParams("Native handoff requires prompt, native identity and cwd".into()));
        }
        let (principal, mut scope) = self.controller()?;
        let root_id = format!("crew-handoff:{}",request.command_id);
        let immutable = serde_json::to_vec(&json!({"principal":principal,"request":request})).map_err(failed)?;
        // Root claim contains every immutable input, including prompt and recovery IDs.
        self.store.claim_crew_command(&root_id,&immutable).map_err(failed)?;
        if let Some((_,Some(receipt))) = self.store.crew_command(&root_id).map_err(failed)? {
            let receipt: HandoffReceipt = serde_json::from_slice(&receipt).map_err(failed)?;
            if self.retired(&receipt.sandbox_id, &receipt.environment.scope, environment_epoch(&receipt.environment))? {
                return Err(denied("Stopped handoff cannot recover"));
            }
            self.validate_environment(&receipt.environment, &receipt.environment.scope)?;
            return Ok(receipt);
        }
        let chat_id = request.recover_chat_id.clone().unwrap_or_else(|| {
            use sha2::{Digest,Sha256};
            let digest = Sha256::digest(root_id.as_bytes());
            let mut bytes = [0u8;16]; bytes.copy_from_slice(&digest[..16]);
            bytes[6]=(bytes[6]&0x0f)|0x80; bytes[8]=(bytes[8]&0x3f)|0x80;
            uuid::Uuid::from_bytes(bytes).to_string()
        });
        scope.deployment_id=Some(self.runtime.deployment_id().into()); scope.session_id=Some(chat_id.clone());
        let sandbox = if let Some(id) = request.recover_sandbox_id.as_ref() {
            if request.recover_chat_id.is_none() { return Err(bad("Recovery requires both chat and sandbox IDs")); }
            let env = self.runtime.client().inspect(id,&scope,cancellation).await.map_err(failed)?;
            self.validate_environment(&env,&scope)?;
            if self.retired(id,&scope,environment_epoch(&env))? { return Err(denied("Stopped handoff cannot recover")); }
            id.clone()
        } else {
            let create_id = format!("{root_id}:create");
            let mut create = self.control(&create_id,Control::Create {
                scope:scope.clone(),name:Some("Crew handoff".into()),source_ref:None,region:None,
                database_environment:request.database_environment,agent_route:request.agent_route.clone(),
            },cancellation).await?;
            if create.status == "unknown" { create = self.reconcile(&create_id, cancellation).await?; }
            let created = confirmed(create)?;
            environment_id(&created.environment).ok_or_else(|| failed("Created environment has no sandbox"))?.into()
        };
        // Attach installs the verified runtime and releases its startup marker;
        // waiting for application readiness first would strand a fresh sandbox.
        let attach_id = format!("{root_id}:attach");
        let mut attachment = self.control(&attach_id,Control::Attach {
            sandbox_id:sandbox.clone(),scope:scope.clone(),
        },cancellation).await?;
        if attachment.status == "unknown" { attachment = self.reconcile(&attach_id, cancellation).await?; }
        let attachment = confirmed(attachment)?;
        let ready = async {
            loop {
                let environment = self.runtime.client().inspect(&sandbox, &scope, cancellation).await.map_err(failed)?;
                self.validate_environment(&environment, &scope)?;
                validate_handoff_epoch(&attachment.environment, &environment)?;
                if self.retired(&sandbox, &scope, environment_epoch(&environment))? {
                    return Err(denied("Stopped handoff cannot recover"));
                }
                match environment.source {
                    SessionEnvironmentSource::Scaffold { lifecycle: ScaffoldLifecycle::Ready | ScaffoldLifecycle::AgentRunning, .. } => return Ok::<(), RpcError>(()),
                    SessionEnvironmentSource::Scaffold { lifecycle: ScaffoldLifecycle::Stopped | ScaffoldLifecycle::Failed | ScaffoldLifecycle::Paused, .. } => return Err(failed("Handoff target is not running")),
                    _ => {}
                }
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
        };
        tokio::select! {
            _ = cancellation.cancelled() => return Err(failed("Handoff preparation cancelled")),
            result = tokio::time::timeout(std::time::Duration::from_secs(300), ready) =>
                result.map_err(|_| failed("Preserved sandbox is still starting; retry preparation with the same command ID"))??,
        }
        let transferred = confirmed(self.control(&format!("{root_id}:transfer"),Control::HandoffOmpSession {
            sandbox_id:sandbox.clone(),scope:scope.clone(),native_session_id:request.native_session_id.clone(),cwd:request.cwd,
            session_only: request.session_only,
        },cancellation).await?)?;
        let receipt = HandoffReceipt {chat_id,sandbox_id:sandbox,command_id:request.command_id,
            environment:attachment.environment.clone(),attachment,
            native_session_id:transferred.handoff_native_session_id.ok_or_else(|| failed("Missing transferred native identity"))?,
            cwd:transferred.handoff_cwd.ok_or_else(|| failed("Missing transferred worktree"))?};
        self.store.finish_crew_command(&root_id,&serde_json::to_vec(&receipt).map_err(failed)?).map_err(failed)?;
        Ok(receipt)
    }

    /// Serialize the final durable admission with Stop on this controller.
    /// Re-inspect the same sandbox/epoch; another controller may have stopped it.
    pub async fn admit_handoff<T, F>(&self, receipt: &HandoffReceipt, admit: F) -> Result<T, RpcError>
    where T: Send + 'static, F: FnOnce() -> Result<T, RpcError> + Send + 'static {
        let gate = self.control_gate.clone().lock_owned().await;
        self.validate_environment(&receipt.environment, &receipt.environment.scope)?;
        let environment = self.runtime.client().inspect(
            &receipt.sandbox_id, &receipt.environment.scope, &CancellationToken::new(),
        ).await.map_err(failed)?;
        self.validate_environment(&environment, &receipt.environment.scope)?;
        validate_handoff_epoch(&receipt.environment, &environment)?;
        if !matches!(&environment.source, SessionEnvironmentSource::Scaffold {
            lifecycle: ScaffoldLifecycle::Ready | ScaffoldLifecycle::AgentRunning, .. }) {
            return Err(denied("Handoff target is stopped or paused; do not replay its prompt"));
        }
        let services = self.clone();
        let receipt = receipt.clone();
        tokio::task::spawn_blocking(move || {
            let _gate = gate;
            services.validate_environment(&receipt.environment, &receipt.environment.scope)?;
            if services.retired(&receipt.sandbox_id, &receipt.environment.scope, environment_epoch(&receipt.environment))? {
                return Err(denied("Stopped handoff cannot admit a command"));
            }
            admit()
        }).await.map_err(failed)?
    }
}

fn persist_repository_diagnostic(store: &DocsStore, command: &str,
    diagnostic: &crate::scaffold::RepositoryImportDiagnostic) -> Result<String, RpcError> {
    use sha2::{Digest, Sha256};
    let bytes = serde_json::to_vec(diagnostic).map_err(failed)?;
    let id = format!("{command}:repository-diagnostic:{:x}", Sha256::digest(&bytes));
    // Evidence is immutable and separate from the transfer's still-unknown outcome.
    store.claim_crew_command(&id, &bytes).map_err(failed)?;
    Ok(id)
}

fn persist_isolated_recovery_diagnostic(store: &DocsStore, child: &str,
    diagnostic: &crate::scaffold::IsolatedRecoveryDiagnostic) -> Result<String, RpcError> {
    use sha2::{Digest, Sha256};
    let bytes = serde_json::to_vec(diagnostic).map_err(failed)?;
    let id = format!("{child}:diagnostic:{:x}", Sha256::digest(&bytes));
    store.claim_crew_command(&id, &bytes).map_err(failed)?;
    Ok(id)
}

/// Resolve allocation evidence only. This never marks preparation or transfer complete.
fn recover_handoff_request(store: &DocsStore, principal: &str, scope: &CollaborationScope,
    owner: &str, chat: &str, sandbox: &str) -> Result<(HandoffRequest, SessionEnvironment), RpcError> {
    #[derive(Deserialize)]
    struct Root { principal: String, request: HandoffRequest }
    let mut found = None;
    let mut after = String::new();
    loop {
        let ids = store.crew_handoff_command_ids(&after, 64).map_err(failed)?;
        let Some(last) = ids.last() else { break };
        after = last.clone();
        for id in ids {
        let Some((bytes, outcome)) = store.crew_command(&id).map_err(failed)? else { continue };
        let Ok(root) = serde_json::from_slice::<Root>(&bytes) else { continue };
        if id != format!("crew-handoff:{}", root.request.command_id) { return Err(denied("Handoff root identity mismatch")); }
        let completed = outcome.as_deref().map(serde_json::from_slice::<HandoffReceipt>).transpose().map_err(failed)?;
        let attachment = store.crew_command(&format!("{id}:attach")).map_err(failed)?;
        let attached = attachment.as_ref().and_then(|(_, bytes)| bytes.as_deref())
            .map(serde_json::from_slice::<ControlOutcome>).transpose().map_err(failed)?;
        let environment = completed.as_ref().map(|receipt| &receipt.environment)
            .or_else(|| attached.as_ref()?.result.as_ref().map(|result| &result.environment));
        let Some(environment) = environment else { continue };
        if environment.scope.session_id.as_deref() != Some(chat) || environment_id(environment) != Some(sandbox) { continue; }
        if found.is_some() { return Err(denied("Multiple handoffs bind these target IDs")); }
        if root.principal != principal || environment.scope != *scope || !environment.owner_principal.eq_ignore_ascii_case(owner)
            || environment.database_environment != Some(root.request.database_environment) {
            return Err(denied("Handoff owner, scope or database binding changed"));
        }
        if let Some(receipt) = completed {
            if receipt.chat_id != chat || receipt.sandbox_id != sandbox || receipt.command_id != root.request.command_id {
                return Err(denied("Handoff receipt identity mismatch"));
            }
            found = Some((root.request, receipt.environment));
            continue;
        }
        for suffix in ["create", "attach", "transfer"] {
            let child_id = format!("{id}:{suffix}");
            let (bytes, outcome) = store.crew_command(&child_id).map_err(failed)?
                .ok_or_else(|| denied("Partial handoff lacks confirmed allocation evidence"))?;
            let child: Command = serde_json::from_slice(&bytes).map_err(failed)?;
            if child.principal_id != principal || control_scope(&child.control) != scope {
                return Err(denied("Handoff child owner or scope changed"));
            }
            let matches = match (&child.control, suffix) {
                (Control::Create { database_environment, agent_route, .. }, "create") =>
                    *database_environment == root.request.database_environment && *agent_route == root.request.agent_route,
                (Control::Attach { sandbox_id, .. }, "attach") => sandbox_id == sandbox,
                (Control::HandoffOmpSession { sandbox_id, native_session_id, cwd, session_only, .. }, "transfer") =>
                    sandbox_id == sandbox && *native_session_id == root.request.native_session_id
                        && *cwd == root.request.cwd && *session_only == root.request.session_only,
                _ => false,
            };
            if !matches { return Err(denied("Handoff child input or target changed")); }
            if suffix != "transfer" {
                let outcome: ControlOutcome = serde_json::from_slice(&outcome.ok_or_else(|| denied("Allocation is not confirmed"))?).map_err(failed)?;
                if outcome.command_id != child_id || outcome.status != "confirmed" || outcome.error.is_some() {
                    return Err(denied("Allocation is not confirmed"));
                }
                let result = outcome.result.ok_or_else(|| denied("Allocation result missing"))?;
                validate_handoff_epoch(environment, &result.environment)?;
            }
            if suffix != "create" {
                let (binding, _) = store.crew_command(&format!("{child_id}:binding")).map_err(failed)?
                    .ok_or_else(|| denied("Handoff child binding missing"))?;
                validate_handoff_epoch(environment, &serde_json::from_slice(&binding).map_err(failed)?)?;
            }
        }
        found = Some((root.request, environment.clone()));
        }
    }
    found.ok_or_else(|| denied("No unique durable allocation matches those target IDs; never create a replacement"))
}

fn validate_handoff_epoch(before: &SessionEnvironment, after: &SessionEnvironment) -> Result<(), RpcError> {
    validate_transition(before, after)?;
    if environment_epoch(before).is_none() || environment_epoch(before) != environment_epoch(after)
        || before.database_environment != after.database_environment {
        return Err(denied("Handoff lifecycle epoch or database changed"));
    }
    Ok(())
}

fn control_scope(control:&Control)->&CollaborationScope {
    match control { Control::Inspect{scope,..}|Control::Create{scope,..}|Control::Pause{scope,..}|
        Control::Resume{scope,..}|Control::Start{scope,..}|Control::Stop{scope,..}|Control::Attach{scope,..}|
        Control::UpdateAgentRoute{scope,..}|Control::HandoffOmpSession{scope,..}=>scope }
}
fn sandbox_id(control:&Control)->Option<&str> {
    match control { Control::Create{..}=>None,Control::Inspect{sandbox_id,..}|Control::Pause{sandbox_id,..}|
        Control::Resume{sandbox_id,..}|Control::Start{sandbox_id,..}|Control::Stop{sandbox_id,..}|Control::Attach{sandbox_id,..}|
        Control::UpdateAgentRoute{sandbox_id,..}|Control::HandoffOmpSession{sandbox_id,..}=>Some(sandbox_id) }
}
fn environment_id(environment:&SessionEnvironment)->Option<&str> {
    match &environment.source { SessionEnvironmentSource::Scaffold{sandbox_id,..}=>Some(sandbox_id),_=>None }
}
fn environment_epoch(environment: &SessionEnvironment) -> Option<u64> {
    match &environment.source { SessionEnvironmentSource::Scaffold { lifecycle_epoch, .. } => *lifecycle_epoch, _ => None }
}
fn terminal_environment(environment: &SessionEnvironment) -> bool {
    matches!(&environment.source, SessionEnvironmentSource::Scaffold { lifecycle: ScaffoldLifecycle::Stopped | ScaffoldLifecycle::Failed, .. })
}
fn validate_start_transition(before: &SessionEnvironment, after: &SessionEnvironment) -> Result<(), RpcError> {
    validate_transition(before, after)?;
    if !terminal_environment(before) || !matches!(&after.source, SessionEnvironmentSource::Scaffold {
        lifecycle: ScaffoldLifecycle::Creating | ScaffoldLifecycle::RestoringSnapshot | ScaffoldLifecycle::Starting
            | ScaffoldLifecycle::Ready | ScaffoldLifecycle::AgentRunning, .. })
        || environment_epoch(before).is_none_or(|epoch| epoch == 0)
        || environment_epoch(before).and_then(|epoch| epoch.checked_add(1)) != environment_epoch(after)
        || environment_epoch(after).is_none() {
        return Err(denied("Explicit Start must create exactly the next nonterminal epoch for the same logical sandbox"));
    }
    Ok(())
}
fn validate_transition(before:&SessionEnvironment,after:&SessionEnvironment)->Result<(),RpcError> {
    if before.scope!=after.scope || before.owner_principal!=after.owner_principal || environment_id(before)!=environment_id(after) {
        return Err(denied("Sandbox identity changed during operation"));
    }
    if let (SessionEnvironmentSource::Scaffold{lifecycle_epoch:Some(old),..},SessionEnvironmentSource::Scaffold{lifecycle_epoch:Some(new),..})=(&before.source,&after.source) {
        if new<old { return Err(denied("Stale sandbox lifecycle completion")); }
    }
    Ok(())
}
fn operation_observed(control:&Control,environment:&SessionEnvironment)->bool {
    let SessionEnvironmentSource::Scaffold{lifecycle,..}=&environment.source else {return false};
    match control {
        Control::Pause{..}=>*lifecycle==ScaffoldLifecycle::Paused,
        Control::Stop{..}=>*lifecycle==ScaffoldLifecycle::Stopped,
        Control::Attach{..}|Control::Resume{..}|Control::Start{..}=>false, // Native authority, not application health, confirms a host.
        Control::Create{..}=>!matches!(lifecycle,ScaffoldLifecycle::Stopped|ScaffoldLifecycle::Failed),
        _=>true,
    }
}
fn confirmed(outcome:ControlOutcome)->Result<ControlResult,RpcError> {
    if outcome.status!="confirmed" {return Err(failed(outcome.error.unwrap_or_else(|| "Handoff outcome unknown; reconcile without repeating creation".into())));}
    outcome.result.ok_or_else(||failed("Confirmed command has no result"))
}
fn unknown(id:&str,error:&str)->ControlOutcome { ControlOutcome{command_id:id.into(),status:"unknown".into(),result:None,error:Some(error.into())} }
fn validate_id(id:&str)->Result<(),RpcError> { if id.trim().is_empty()||id.len()>512 {Err(bad("Invalid immutable command ID"))}else{Ok(())} }
fn string<'a>(value:&'a Value,key:&str)->Result<&'a str,RpcError>{value.get(key).and_then(Value::as_str).filter(|v|!v.trim().is_empty()).ok_or_else(||bad(format!("Missing {key}")))}
fn denied(message:impl ToString)->RpcError{RpcError::Capability(message.to_string())}
fn failed(message:impl ToString)->RpcError{RpcError::Failed(message.to_string())}
fn bad(message:impl ToString)->RpcError{RpcError::BadParams(message.to_string())}

/// Errors observation can never turn into success. Everything else, including
/// cancellation and malformed replies, stays unknown and is never replayed.
fn terminal_control_error(error: &ScaffoldError) -> bool {
    matches!(error, ScaffoldError::AttachRejected(_) | ScaffoldError::RuntimeContractMismatch)
        || matches!(error, ScaffoldError::Api(api) if (400..500).contains(&api.0.status) && api.0.status != 408 && api.0.status != 429)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn runtime_contract_mismatch_is_terminal_but_malformed_launch_stays_unknown() {
        assert!(terminal_control_error(&ScaffoldError::RuntimeContractMismatch));
        assert!(!terminal_control_error(&ScaffoldError::InvalidResponse(
            "sandbox exec returned no supervised native runId".into(),
        )));
        assert!(!terminal_control_error(&ScaffoldError::Cancelled));
    }
    #[test]
    fn repository_diagnostic_survives_reopen_without_completing_transfer() {
        let temp = tempfile::tempdir().unwrap();
        let store = DocsStore::open(temp.path()).unwrap();
        let transfer = "crew-handoff:original:transfer";
        store.claim_crew_command(transfer, b"immutable transfer").unwrap();
        let diagnostic = crate::scaffold::RepositoryImportDiagnostic {
            sandbox_id: "original-box".into(), exit_code: Some(45), stderr: "crew_repository:tree_entry_unsupported".into(),
            manifest_sha256: "a".repeat(64), base_sha: "b".repeat(40), archive_bytes: 10240,
            bundle_sha256: "c".repeat(64), bundle_bytes: 512, prerequisite_sha: None, shallow: Some(true),
        };
        let key = persist_repository_diagnostic(&store, transfer, &diagnostic).unwrap();
        assert_eq!(persist_repository_diagnostic(&store, transfer, &diagnostic).unwrap(), key);
        let child = "crew-handoff:original:isolated-recovery";
        store.claim_crew_command(child, b"immutable recovery").unwrap();
        let isolated = crate::scaffold::IsolatedRecoveryDiagnostic {
            operation_id: child.into(), sandbox_id: "original-box".into(), descriptor_sha256: Some("d".repeat(64)),
            exit_code: Some(60), stderr: "crew_repository:tree_entry_unsupported".into(),
            artifacts: json!({"repository":{"archive_sha256":"e".repeat(64),"bundle_sha256":"f".repeat(64)},"native":{"native_session_id":"saved-fork-A"}}),
        };
        let isolated_key = persist_isolated_recovery_diagnostic(&store, child, &isolated).unwrap();
        assert_eq!(persist_isolated_recovery_diagnostic(&store, child, &isolated).unwrap(), isolated_key);
        drop(store);
        let reopened = DocsStore::open(temp.path()).unwrap();
        assert!(reopened.crew_command(transfer).unwrap().unwrap().1.is_none());
        assert!(key.starts_with(&format!("{transfer}:repository-diagnostic:")));
        let (bytes, outcome) = reopened.crew_command(&key).unwrap().unwrap();
        assert!(outcome.is_none());
        let saved: crate::scaffold::RepositoryImportDiagnostic = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(saved.sandbox_id, diagnostic.sandbox_id);
        assert_eq!(saved.stderr, diagnostic.stderr);
        assert_eq!(saved.manifest_sha256, diagnostic.manifest_sha256);
        assert!(reopened.crew_command(child).unwrap().unwrap().1.is_none());
        let (bytes, outcome) = reopened.crew_command(&isolated_key).unwrap().unwrap();
        assert!(outcome.is_none());
        let saved: crate::scaffold::IsolatedRecoveryDiagnostic = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(saved.operation_id, child);
        assert_eq!(saved.stderr, isolated.stderr);
        assert_eq!(saved.artifacts, isolated.artifacts);
    }

    #[test]
    fn partial_handoff_recovery_survives_reopen_and_rejects_conflicting_evidence() {
        for scenario in ["valid", "completed", "owner", "scope", "sandbox", "mode", "binding_epoch", "unknown_create", "ambiguous", "paged", "paged_completed_ambiguity"] {
            let temp = tempfile::tempdir().unwrap();
            let store = DocsStore::open(temp.path()).unwrap();
            let scope = CollaborationScope { project_id: "p".into(), deployment_id: Some("d".into()), session_id: Some("chat".into()), unknown: Default::default() };
            let environment = SessionEnvironment {
                source: SessionEnvironmentSource::Scaffold { sandbox_id: "box".into(), region: None, lifecycle: ScaffoldLifecycle::Ready, lifecycle_epoch: Some(7), links: Default::default() },
                name: None, owner_principal: "owner@example.com".into(), scope: scope.clone(), source_ref: None,
                last_activity_at: None, database_environment: Some(ScaffoldDatabaseEnvironment::Local), unknown: Default::default(),
            };
            let request = HandoffRequest { command_id: "caller".into(), source_session_id: "source".into(), native_session_id: "native".into(), cwd: "/repo".into(), prompt: "original prompt".into(),
                agent_route: AgentRoute::from_omp_model("anthropic/claude-sonnet-4-5").unwrap(), database_environment: ScaffoldDatabaseEnvironment::Local,
                session_only: false, recover_sandbox_id: None, recover_chat_id: None };
            let root = "crew-handoff:caller";
            let save = |id: &str, value: Value, outcome: Option<Value>| {
                store.claim_crew_command(id, &serde_json::to_vec(&value).unwrap()).unwrap();
                if let Some(outcome) = outcome { store.finish_crew_command(id, &serde_json::to_vec(&outcome).unwrap()).unwrap(); }
            };
            save(root, json!({"principal":"principal", "request":request}), None);
            // Force discovery through multiple key pages without reading unrelated payloads.
            if scenario.starts_with("paged") {
                for index in 0..130 {
                    save(&format!("crew-handoff:aaa-{index:03}:child"), json!({"unrelated":true}), None);
                    save(&format!("crew-handoff:mmm-{index:03}:child"), json!({"unrelated":true}), None);
                }
            }
            let mut child_scope = scope.clone();
            if scenario == "scope" { child_scope.project_id = "other".into(); }
            let principal = if scenario == "owner" { "other" } else { "principal" };
            for (suffix, control) in [
                ("create", Control::Create { scope: child_scope.clone(), name: Some("Crew handoff".into()), source_ref: None, region: None, database_environment: request.database_environment, agent_route: request.agent_route.clone() }),
                ("attach", Control::Attach { sandbox_id: if scenario == "sandbox" { "other" } else { "box" }.into(), scope: child_scope.clone() }),
                ("transfer", Control::HandoffOmpSession { sandbox_id: "box".into(), scope: child_scope, native_session_id: request.native_session_id.clone(), cwd: request.cwd.clone(), session_only: scenario == "mode" }),
            ] {
                let id = format!("{root}:{suffix}");
                let result = ControlResult { environment: environment.clone(), preparation_generation: None, attached_device_id: Some("device".into()), run_id: None, room_projection: None, control_grant: None, handoff_native_session_id: None, handoff_cwd: None };
                let outcome = (suffix != "transfer" && !(scenario == "unknown_create" && suffix == "create"))
                    .then(|| json!(ControlOutcome { command_id: id.clone(), status: "confirmed".into(), result: Some(result), error: None }));
                save(&id, json!(Command { principal_id: principal.into(), control }), outcome);
                if suffix != "create" {
                    let mut binding = environment.clone();
                    if scenario == "binding_epoch" && suffix == "transfer" {
                        if let SessionEnvironmentSource::Scaffold { lifecycle_epoch, .. } = &mut binding.source { *lifecycle_epoch = Some(8); }
                    }
                    save(&format!("{id}:binding"), json!(binding), None);
                }
            }
            if scenario == "completed" {
                let (_, bytes) = store.crew_command(&format!("{root}:attach")).unwrap().unwrap();
                let attachment: ControlOutcome = serde_json::from_slice(&bytes.unwrap()).unwrap();
                save(root, json!({"principal":"principal", "request":request}), Some(json!(HandoffReceipt {
                    chat_id: "chat".into(), sandbox_id: "box".into(), command_id: request.command_id.clone(),
                    environment: environment.clone(), attachment: attachment.result.unwrap(), native_session_id: "native".into(), cwd: "/remote".into(),
                })));
            }
            if matches!(scenario, "ambiguous" | "paged_completed_ambiguity") {
                let mut other = request.clone(); other.command_id = "other".into();
                save("crew-handoff:other", json!({"principal":"principal", "request":other}), None);
                for suffix in ["create", "attach", "attach:binding", "transfer", "transfer:binding"] {
                    let (bytes, outcome) = store.crew_command(&format!("{root}:{suffix}")).unwrap().unwrap();
                    let mut outcome: Option<Value> = outcome.map(|bytes| serde_json::from_slice(&bytes).unwrap());
                    if let Some(outcome) = outcome.as_mut() { outcome["commandId"] = json!(format!("crew-handoff:other:{suffix}")); }
                    save(&format!("crew-handoff:other:{suffix}"), serde_json::from_slice(&bytes).unwrap(), outcome);
                }
                if scenario == "paged_completed_ambiguity" {
                    let (_, bytes) = store.crew_command(&format!("{root}:attach")).unwrap().unwrap();
                    let attachment: ControlOutcome = serde_json::from_slice(&bytes.unwrap()).unwrap();
                    save("crew-handoff:other", json!({"principal":"principal", "request":other}), Some(json!(HandoffReceipt {
                        chat_id: "chat".into(), sandbox_id: "box".into(), command_id: other.command_id.clone(),
                        environment: environment.clone(), attachment: attachment.result.unwrap(), native_session_id: "native".into(), cwd: "/remote".into(),
                    })));
                }
            }
            drop(store);
            let reopened = DocsStore::open(temp.path()).unwrap();
            let recovered = recover_handoff_request(&reopened, "principal", &scope, "owner@example.com", "chat", "box");
            if matches!(scenario, "valid" | "completed" | "paged") {
                assert_eq!(serde_json::to_value(recovered.unwrap().0).unwrap(), serde_json::to_value(request).unwrap());
                assert_eq!(reopened.crew_command(root).unwrap().unwrap().1.is_some(), scenario == "completed");
                assert!(reopened.crew_command(&format!("{root}:transfer")).unwrap().unwrap().1.is_none());
                assert!(!reopened.claim_crew_command(&format!("{root}:create"), &reopened.crew_command(&format!("{root}:create")).unwrap().unwrap().0).unwrap());
                assert!(!reopened.claim_crew_command(&format!("{root}:attach"), &reopened.crew_command(&format!("{root}:attach")).unwrap().unwrap().0).unwrap());
                assert!(recover_handoff_request(&reopened, "principal", &scope, "owner@example.com", "other", "box").is_err());
                assert!(recover_handoff_request(&reopened, "principal", &scope, "owner@example.com", "chat", "other").is_err());
            } else { assert!(recovered.is_err(), "{scenario}"); }
        }
    }

    #[test]
    fn handoff_recovery_rejects_changed_or_missing_epoch_before_transfer() {
        let before = SessionEnvironment {
            source: SessionEnvironmentSource::Scaffold { sandbox_id: "box".into(), region: None,
                lifecycle: ScaffoldLifecycle::Ready, lifecycle_epoch: Some(7), links: Default::default() },
            name: None, owner_principal: "owner@example.com".into(),
            scope: CollaborationScope { project_id: "p".into(), deployment_id: Some("d".into()), session_id: Some("s".into()), unknown: Default::default() },
            source_ref: None, last_activity_at: None, database_environment: Some(ScaffoldDatabaseEnvironment::Local), unknown: Default::default(),
        };
        assert!(validate_handoff_epoch(&before, &before).is_ok());
        for epoch in [None, Some(6), Some(8)] {
            let mut after = before.clone();
            if let SessionEnvironmentSource::Scaffold { lifecycle_epoch, .. } = &mut after.source { *lifecycle_epoch = epoch; }
            assert!(validate_handoff_epoch(&before, &after).is_err());
        }
        let mut changed = before.clone(); changed.database_environment = Some(ScaffoldDatabaseEnvironment::StagingSnapshot);
        assert!(validate_handoff_epoch(&before, &changed).is_err());
    }


    #[test]
    fn explicit_start_requires_exact_next_epoch_and_preserves_logical_scope() {
        let before = SessionEnvironment {
            source: SessionEnvironmentSource::Scaffold { sandbox_id: "box".into(), region: None,
                lifecycle: ScaffoldLifecycle::Stopped, lifecycle_epoch: Some(7), links: Default::default() },
            name: None, owner_principal: "owner@example.com".into(),
            scope: CollaborationScope { project_id: "p".into(), deployment_id: Some("d".into()), session_id: Some("s".into()), unknown: Default::default() },
            source_ref: None, last_activity_at: None, database_environment: None, unknown: Default::default(),
        };
        let mut after = before.clone();
        assert!(validate_start_transition(&before, &after).is_err());
        if let SessionEnvironmentSource::Scaffold { lifecycle, lifecycle_epoch, .. } = &mut after.source {
            *lifecycle = ScaffoldLifecycle::Ready; *lifecycle_epoch = Some(8);
        }
        assert!(validate_start_transition(&before, &after).is_ok());
        let start = Control::Start { sandbox_id: "box".into(), scope: before.scope.clone(), expected_lifecycle_epoch: 7 };
        assert!(!operation_observed(&start, &after), "a larger row epoch is not native authority");
        let wire = serde_json::to_value(&start).unwrap();
        assert_eq!(wire["operation"], "start");
        assert_eq!(wire["expectedLifecycleEpoch"], 7);
        after.scope.session_id = Some("different".into());
        assert!(validate_start_transition(&before, &after).is_err());
        after.scope = before.scope.clone();
        if let SessionEnvironmentSource::Scaffold { lifecycle_epoch, .. } = &mut after.source { *lifecycle_epoch = Some(9); }
        assert!(validate_start_transition(&before, &after).is_err());
    }
    #[test]
    fn accepted_startup_is_not_a_confirmed_resume() {
        let scope=CollaborationScope{project_id:"p".into(),deployment_id:Some("d".into()),session_id:Some("s".into()),unknown:Default::default()};
        let mut environment=SessionEnvironment{source:SessionEnvironmentSource::Scaffold{sandbox_id:"box".into(),region:None,lifecycle:ScaffoldLifecycle::Resuming,lifecycle_epoch:Some(2),links:Default::default()},name:None,owner_principal:"owner@example.com".into(),scope:scope.clone(),source_ref:None,last_activity_at:None,database_environment:None,unknown:Default::default()};
        assert!(!operation_observed(&Control::Resume{sandbox_id:"box".into(),scope:scope.clone()},&environment));
        if let SessionEnvironmentSource::Scaffold{lifecycle,..}=&mut environment.source{*lifecycle=ScaffoldLifecycle::Ready;}
        assert!(!operation_observed(&Control::Resume{sandbox_id:"box".into(),scope:scope.clone()},&environment));
        let previous=environment.clone();
        if let SessionEnvironmentSource::Scaffold{lifecycle,lifecycle_epoch,..}=&mut environment.source{*lifecycle=ScaffoldLifecycle::Ready;*lifecycle_epoch=Some(1);}
        assert!(validate_transition(&previous,&environment).is_err());
        environment.owner_principal="other@example.com".into();
        assert!(validate_transition(&previous,&environment).is_err());
    }
}
