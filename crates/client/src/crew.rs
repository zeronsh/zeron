//! Crew route hints and controller operations over upstream's authenticated relay.
//! Opening a link never attaches, resumes, or creates execution.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use zeron_proto::{CollaborationScope, CrewSessionLink, ScaffoldSessionLink, ScaffoldEnvironmentControl};
use crate::{Client, ClientError, config::Credentials, error::Result};

const SCHEMES: &[&str] = &["crew-upstream-canary", "comet", "comet-staging", "crew", "crew-staging"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionLink {
    pub project_id: String,
    pub session_id: String,
    pub deployment_id: Option<String>,
    pub sandbox_id: Option<String>,
}

pub fn parse_session_link(url: &str) -> Option<SessionLink> {
    for scheme in SCHEMES {
        if let Some(link) = CrewSessionLink::parse_deep_link(url, scheme) {
            return Some(SessionLink { project_id: link.project_id, session_id: link.session_id,
                deployment_id: link.deployment_id, sandbox_id: None });
        }
        if let Some(link) = ScaffoldSessionLink::parse_deep_link(url, scheme) {
            return Some(SessionLink { project_id: link.scope.project_id,
                session_id: uuid::Uuid::parse_str(link.scope.session_id.as_deref()?).ok()?.to_string(),
                deployment_id: link.scope.deployment_id, sandbox_id: Some(link.sandbox_id) });
        }
    }
    None
}

impl Client {
    pub(crate) fn crew_scope(&self, project: &str, deployment: &str) -> Result<()> {
        let Credentials::Scaffold { session_json, .. } = &self.inner.credentials else {
            return Err(ClientError::Auth("Sign in to Crew before accessing its scope".into()));
        };
        let session: zeron_sync::scaffold_auth::ScaffoldSession = serde_json::from_str(session_json)
            .map_err(|_| ClientError::Auth("Invalid Crew session".into()))?;
        if project != self.org_id() || deployment.trim().is_empty()
            || session.device.is_some() || session.identity.deployment_id.as_deref() != Some(deployment) {
            return Err(ClientError::Auth("Open this session in its authenticated Crew project and deployment".into()));
        }
        Ok(())
    }

    /// Returns an existing authorized registry row only, without requiring its host online.
    pub fn crew_cached_link(&self, url: &str, deployment: &str) -> Result<Option<String>> {
        let link = parse_session_link(url).ok_or_else(|| ClientError::InvalidArgument("Invalid Crew session link".into()))?;
        self.crew_scope(&link.project_id, deployment)?;
        if link.deployment_id.as_deref().is_some_and(|id| id != deployment) {
            return Err(ClientError::Auth("This link belongs to another deployment".into()));
        }
        let Some(chat) = self.inner.workspace.chat(&link.session_id) else { return Ok(None) };
        let metadata = chat.crew.as_ref().ok_or_else(|| ClientError::Auth("Session has no verified Crew scope".into()))?;
        if metadata.owner_subject != self.user_id() || metadata.scope.project_id != link.project_id
            || metadata.scope.deployment_id.as_deref() != Some(deployment)
            || metadata.session_id != link.session_id
            || metadata.scope.session_id.as_deref() != Some(link.session_id.as_str()) {
            return Err(ClientError::Auth("Session scope conflicts with this link".into()));
        }
        if let Some(sandbox) = link.sandbox_id {
            if !metadata.environment.as_ref().is_some_and(|env| env.scope == metadata.scope
                && env.owner_principal == self.user_id()
                && matches!(&env.source, zeron_proto::SessionEnvironmentSource::Scaffold { sandbox_id, .. } if sandbox_id == &sandbox)) {
                return Err(ClientError::Auth("Sandbox identity conflicts with this link".into()));
            }
        }
        Ok(Some(chat.id))
    }

    async fn crew_controller(&self, device: &str) -> Result<()> {
        let live = self.inner.live().ok_or_else(|| ClientError::Unsupported("Crew requires an authenticated host".into()))?;
        let status = live.relay.call(device, "crew.status", json!({})).await?;
        if status.get("role").and_then(Value::as_str) != Some("local_controller") {
            return Err(ClientError::Auth("Select a desktop Crew controller, not a sandbox or phone".into()));
        }
        Ok(())
    }

    pub async fn crew_resolve_link(&self, device: &str, url: &str, deployment: &str) -> Result<String> {
        if let Some(id) = self.crew_cached_link(url, deployment)? { return Ok(id); }
        self.crew_controller(device).await?;
        let live = self.inner.live().ok_or(ClientError::Closed)?;
        let reply = live.relay.call(device, "ResolveCrewSessionLink", json!({"url": url})).await?;
        let id = reply.get("chatId").and_then(Value::as_str)
            .ok_or_else(|| ClientError::HostError("Controller returned no session reference".into()))?;
        if Some(id) != parse_session_link(url).as_ref().map(|link| link.session_id.as_str()) {
            return Err(ClientError::Auth("Controller resolved a different session".into()));
        }
        // The controller publishes the canonical row. The caller waits for normal registry sync.
        Ok(id.to_owned())
    }

    /// Mutations are issued once. An ambiguous response is reconciled by command id, never replayed.
    pub async fn crew_control(&self, device: &str, command_id: &str, control: ScaffoldEnvironmentControl, deployment: &str) -> Result<Value> {
        let scope: &CollaborationScope = match &control {
            ScaffoldEnvironmentControl::Inspect { scope, .. } | ScaffoldEnvironmentControl::Create { scope, .. }
            | ScaffoldEnvironmentControl::Pause { scope, .. } | ScaffoldEnvironmentControl::Resume { scope, .. }
            | ScaffoldEnvironmentControl::Start { scope, .. }
            | ScaffoldEnvironmentControl::Stop { scope, .. } | ScaffoldEnvironmentControl::Attach { scope, .. }
            | ScaffoldEnvironmentControl::UpdateAgentRoute { scope, .. } => scope,
            ScaffoldEnvironmentControl::HandoffOmpSession { .. } => {
                return Err(ClientError::Unsupported("Native worktree handoff must be initiated on its owning desktop".into()));
            }
        };
        self.crew_scope(&scope.project_id, deployment)?;
        if scope.deployment_id.as_deref() != Some(deployment) || uuid::Uuid::parse_str(command_id).is_err()
            || scope.session_id.as_deref().and_then(|id| uuid::Uuid::parse_str(id).ok()).is_none() {
            return Err(ClientError::InvalidArgument("Invalid scoped Crew command".into()));
        }
        match &control {
            ScaffoldEnvironmentControl::Create { agent_route, .. }
            | ScaffoldEnvironmentControl::UpdateAgentRoute { agent_route, .. } =>
                agent_route.validate().map_err(|message| ClientError::InvalidArgument(message.into()))?,
            _ => {}
        }
        let expected_scope = scope.clone();
        self.crew_controller(device).await?;
        let reply = self.inner.live().ok_or(ClientError::Closed)?.relay.call_once(device, "scaffold.control",
            json!({"commandId":command_id,"control":control})).await?;
        if reply.get("commandId").and_then(Value::as_str) != Some(command_id) {
            return Err(ClientError::HostError("Controller returned a different command outcome".into()));
        }
        if let Some(result) = reply.get("result").filter(|value| !value.is_null()) {
            let result: zeron_proto::ScaffoldEnvironmentControlResult = serde_json::from_value(result.clone())
                .map_err(|_| ClientError::HostError("Invalid scoped controller outcome".into()))?;
            if result.environment.scope != expected_scope || result.environment.owner_principal != self.user_id() {
                return Err(ClientError::Auth("Controller outcome belongs to another session scope".into()));
            }
        }
        Ok(reply)
    }

    pub async fn crew_reconcile(&self, device: &str, command_id: &str) -> Result<Value> {
        self.crew_controller(device).await?;
        self.inner.live().ok_or(ClientError::Closed)?.relay.call(device, "scaffold.reconcile", json!({"commandId":command_id})).await
    }

    pub async fn crew_refresh(&self, device: &str, deployment: &str) -> Result<Value> {
        self.crew_scope(self.org_id(), deployment)?;
        self.crew_controller(device).await?;
        self.inner.live().ok_or(ClientError::Closed)?.relay.call(device, "scaffold.refresh",
            json!({"projectId":self.org_id(),"deploymentId":deployment})).await
    }

    /// Must complete before constructing a client for this profile. Legacy
    /// credentials are never inputs; the current Scaffold token is reverified.
    pub async fn crew_import_legacy_backup(config: crate::config::ClientConfig,
        credentials: Credentials, source: std::path::PathBuf) -> Result<String> {
        use zeron_sync::scaffold_auth::{ScaffoldAuth, ScaffoldConfig, ScaffoldSession};
        let Credentials::Scaffold { user_id, org_id, control_origin, session_json } = credentials else {
            return Err(ClientError::Auth("Legacy hydration requires current Scaffold sign-in".into()));
        };
        let session: ScaffoldSession = serde_json::from_str(&session_json)
            .map_err(|_| ClientError::Auth("Invalid current Scaffold credential".into()))?;
        let mut auth_config = ScaffoldConfig::new(control_origin, config.edge_url.clone(), org_id.clone());
        auth_config.deployment_id = session.identity.deployment_id.clone();
        let auth = ScaffoldAuth::new(auth_config).map_err(|error| ClientError::Auth(error.to_string()))?;
        let verified = auth.verify(&session).await.map_err(|error| ClientError::Auth(error.to_string()))?;
        if verified.device.is_some() || verified.identity.user_id != user_id || verified.identity.project_scope != org_id
            || verified.identity.deployment_id.is_none() {
            return Err(ClientError::Auth("Verified Scaffold owner/project/deployment differs from this profile".into()));
        }
        let principal = zeron_proto::CollaborationPrincipal {
            subject: verified.identity.user_id, email: Some(verified.identity.email),
            project_id: verified.identity.project_scope, deployment_id: verified.identity.deployment_id,
            session_id: None, capabilities: verified.identity.capabilities, unknown: Default::default(),
        };
        crate::runtime::shared().spawn_blocking(move || {
            let report = crate::legacy::hydrate_backup(&source, &config.data_dir, &principal, &verified.edge_origin, &config.device_id)?;
            serde_json::to_string(&report).map_err(|error| ClientError::Storage(error.to_string()))
        }).await.map_err(|error| ClientError::Storage(error.to_string()))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn existing_routes_are_hints_not_credentials() {
        let id = "8b3ca06e-1fe0-4b72-bbe3-7bc7ec760baf";
        for scheme in SCHEMES {
            let directory = parse_session_link(&format!("{scheme}://session/project/{id}?deploymentId=staging")).unwrap();
            let scaffold = parse_session_link(&format!("{scheme}://scaffold/project/staging/{id}/sandbox")).unwrap();
            assert_eq!(directory.session_id, scaffold.session_id);
            assert_eq!(directory.deployment_id.as_deref(), Some("staging"));
            assert_eq!(scaffold.sandbox_id.as_deref(), Some("sandbox"));
            assert!(parse_session_link(&format!("{scheme}://session/project/{id}?token=secret")).is_none());
            assert!(parse_session_link(&format!("{scheme}://scaffold/project/staging/not-a-uuid/sandbox")).is_none());
        }
        assert!(parse_session_link(&format!("https://session/project/{id}")).is_none());
    }
}
