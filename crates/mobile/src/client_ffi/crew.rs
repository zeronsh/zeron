//! Crew uses the same resource-bound OAuth implementation as the host.
use std::sync::Arc;
use zeron_client::crew_launch as launch;
use zeron_proto::ScaffoldDatabaseEnvironment;
use zeron_sync::scaffold_auth::{self, ScaffoldAuth, ScaffoldConfig, ScaffoldSession};
use super::{ChatConfig, CoreClient, CoreError, CoreResult, Credentials, SendRequest};

/// The database a new Scaffold sandbox starts with (the desktop's
/// Scaffold destinations).
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum CrewDatabase {
    Local,
    StagingSnapshot,
    ProductionSnapshot,
}

impl From<CrewDatabase> for ScaffoldDatabaseEnvironment {
    fn from(database: CrewDatabase) -> Self {
        match database {
            CrewDatabase::Local => Self::Local,
            CrewDatabase::StagingSnapshot => Self::StagingSnapshot,
            CrewDatabase::ProductionSnapshot => Self::ProductionSnapshot,
        }
    }
}

impl From<ScaffoldDatabaseEnvironment> for CrewDatabase {
    fn from(database: ScaffoldDatabaseEnvironment) -> Self {
        match database {
            ScaffoldDatabaseEnvironment::Local => Self::Local,
            ScaffoldDatabaseEnvironment::StagingSnapshot => Self::StagingSnapshot,
            ScaffoldDatabaseEnvironment::ProductionSnapshot => Self::ProductionSnapshot,
        }
    }
}

/// Where a sandbox launch stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum CrewLaunchPhase {
    /// Reaching a desktop controller and creating the sandbox.
    Creating,
    /// The sandbox exists; its session host is being attached.
    Starting,
    /// Attached; waiting for the host to come online before sending.
    Connecting,
}

impl From<launch::CrewLaunchPhase> for CrewLaunchPhase {
    fn from(phase: launch::CrewLaunchPhase) -> Self {
        match phase {
            launch::CrewLaunchPhase::Creating => Self::Creating,
            launch::CrewLaunchPhase::Starting => Self::Starting,
            launch::CrewLaunchPhase::Connecting => Self::Connecting,
        }
    }
}

/// Launch progress. Called on a client thread: hop to the main thread.
#[uniffi::export(with_foreign)]
pub trait CrewLaunchProgress: Send + Sync {
    fn on_phase(&self, phase: CrewLaunchPhase);
}

/// A launch whose outcome is still owed (it resumes on the next launch).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct CrewRetainedLaunch {
    /// The desktop controller holding its command.
    pub controller_id: String,
    pub session_id: String,
    pub database: CrewDatabase,
}

/// "Scaffold (Local DB)" etc., as the desktop's device picker labels them.
#[uniffi::export]
pub fn crew_database_label(database: CrewDatabase) -> String {
    launch::database_label(database.into()).to_owned()
}

/// A model a Scaffold sandbox can run (managed OMP/Antigravity routes).
#[uniffi::export]
pub fn crew_managed_model(harness: String, model: String) -> bool {
    serde_json::from_value::<zeron_proto::HarnessId>(serde_json::Value::String(harness))
        .is_ok_and(|harness| launch::is_managed_model(harness, &model))
}

#[derive(uniffi::Record)]
pub struct CrewAccount {
    pub credentials: Credentials,
    pub profile_key: String,
    pub name: Option<String>,
    pub email: Option<String>,
    pub project_scope: String,
}

#[derive(uniffi::Object)]
pub struct CrewLogin {
    auth: Arc<ScaffoldAuth>,
    control_origin: String,
}

fn error(e: scaffold_auth::AuthError) -> CoreError {
    zeron_client::auth::scaffold_error(e).into()
}

impl CrewLogin {
    fn account(&self, session: ScaffoldSession) -> CoreResult<CrewAccount> {
        let profile_key = scaffold_auth::profile_key(&session);
        let session_json = serde_json::to_string(&session)
            .map_err(|e| CoreError::Internal { message: e.to_string() })?;
        Ok(CrewAccount {
            credentials: Credentials::Scaffold {
                user_id: session.identity.user_id.clone(),
                org_id: session.identity.project_scope.clone(),
                control_origin: self.control_origin.clone(),
                session_json,
            },
            profile_key,
            name: session.identity.name,
            email: Some(session.identity.email),
            project_scope: session.identity.project_scope,
        })
    }
}

#[uniffi::export]
impl CrewLogin {
    #[uniffi::constructor]
    pub fn new(control_origin: String, edge_origin: String, project_scope: String, deployment_id: String) -> CoreResult<Arc<Self>> {
        let mut config = ScaffoldConfig::new(control_origin.clone(), edge_origin, project_scope);
        config.deployment_id = Some(deployment_id);
        let auth = ScaffoldAuth::new(config).map_err(error)?;
        Ok(Arc::new(Self { auth: Arc::new(auth), control_origin }))
    }

    pub async fn begin(&self, redirect_uri: String) -> CoreResult<String> {
        let auth = self.auth.clone();
        super::on_runtime(async move { auth.begin_sign_in(&redirect_uri).await.map_err(zeron_client::auth::scaffold_error) }).await
    }

    pub async fn complete(&self, callback: String) -> CoreResult<CrewAccount> {
        let auth = self.auth.clone();
        let session = super::on_runtime(async move { auth.complete_sign_in(&callback).await.map_err(zeron_client::auth::scaffold_error) }).await?;
        self.account(session)
    }

    pub async fn restore(&self, session_json: String) -> CoreResult<CrewAccount> {
        let session: ScaffoldSession = serde_json::from_str(&session_json)
            .map_err(|_| CoreError::Auth { message: "Invalid saved Crew session".into() })?;
        let auth = self.auth.clone();
        let session = super::on_runtime(async move { auth.verify(&session).await.map_err(zeron_client::auth::scaffold_error) }).await?;
        self.account(session)
    }

    pub fn cancel(&self) { self.auth.cancel_sign_in(); }
}

/// Convert the stopped legacy phone cache before constructing a live client.
#[uniffi::export]
pub async fn crew_prepare_legacy_backup(
    config: super::CoreConfig, credentials: Credentials, source_path: String,
) -> CoreResult<String> {
    super::on_runtime(async move {
        zeron_client::Client::crew_import_legacy_backup(config.into(), credentials.into(), source_path.into()).await
    }).await
}

#[uniffi::export]
pub fn crew_is_session_link(url: String) -> bool {
    zeron_client::crew::parse_session_link(&url).is_some()
}

#[uniffi::export]
impl CoreClient {
    pub fn crew_cached_link(&self, url: String, deployment: String) -> CoreResult<Option<String>> {
        self.client.crew_cached_link(&url, &deployment).map_err(Into::into)
    }

    pub async fn crew_resolve_link(&self, device_id: String, url: String, deployment: String) -> CoreResult<String> {
        let client = self.client.clone();
        super::on_runtime(async move { client.crew_resolve_link(&device_id, &url, &deployment).await }).await
    }

    pub async fn crew_control(&self, device_id: String, command_id: String, control_json: String, deployment: String) -> CoreResult<String> {
        let control = serde_json::from_str(&control_json)
            .map_err(|e| CoreError::InvalidArgument { message: format!("Invalid Scaffold operation: {e}") })?;
        let client = self.client.clone();
        let value = super::on_runtime(async move { client.crew_control(&device_id, &command_id, control, &deployment).await }).await?;
        Ok(value.to_string())
    }

    pub async fn crew_reconcile(&self, device_id: String, command_id: String) -> CoreResult<String> {
        let client = self.client.clone();
        let value = super::on_runtime(async move { client.crew_reconcile(&device_id, &command_id).await }).await?;
        Ok(value.to_string())
    }

    pub async fn crew_refresh(&self, device_id: String, deployment: String) -> CoreResult<String> {
        let client = self.client.clone();
        let value = super::on_runtime(async move { client.crew_refresh(&device_id, &deployment).await }).await?;
        Ok(value.to_string())
    }

    /// An online desktop controller that can start Scaffold sandboxes (its
    /// device id; the model catalog comes from it).
    pub async fn crew_launch_controller(&self, deployment: String) -> CoreResult<String> {
        let client = self.client.clone();
        super::on_runtime(async move { client.crew_launch_controller(&deployment).await }).await
    }

    /// Start a new session in a new Scaffold sandbox and send `first` into
    /// it once its host is online. Returns the session id. A launch whose
    /// outcome is owed resumes here instead of creating another sandbox.
    pub async fn crew_launch(
        &self,
        database: CrewDatabase,
        config: ChatConfig,
        first: SendRequest,
        deployment: String,
        progress: Arc<dyn CrewLaunchProgress>,
    ) -> CoreResult<String> {
        let request = launch::CrewLaunch {
            database: database.into(),
            config: config.try_into()?,
            first: first.into(),
        };
        let client = self.client.clone();
        super::on_runtime(async move {
            let phase = move |phase: launch::CrewLaunchPhase| progress.on_phase(phase.into());
            client.crew_launch(request, &deployment, &phase).await
        })
        .await
    }

    pub fn crew_launch_retained(&self) -> CoreResult<Option<CrewRetainedLaunch>> {
        Ok(self.client.crew_launch_retained()?.map(|retained| CrewRetainedLaunch {
            controller_id: retained.controller,
            session_id: retained.session_id,
            database: retained.database.into(),
        }))
    }

    /// Stop waiting on the retained launch (the user's explicit choice). A
    /// sandbox it created stays listed under Scaffold sandboxes.
    pub fn crew_launch_forget(&self) -> CoreResult<()> {
        Ok(self.client.crew_launch_forget()?)
    }
}
