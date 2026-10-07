//! UniFFI facade over `zeron-client`: the account-scoped [`CoreClient`], the
//! per-chat [`SessionHandle`], the foreign [`ClientListener`], and the static
//! helpers the platform needs before a client exists (sign-in, catalogs,
//! formatting).
//!
//! Threading: every method is safe from any thread and never blocks on the
//! network; `async` methods run on the client's own tokio runtime and can be
//! awaited from Swift concurrency / Kotlin coroutines directly.
//!
//! Rust consumers inside this crate (the layout engine) reach the transcript
//! through [`CoreClient::session_handle`] → [`zeron_client::SessionHandle`]
//! (`snapshot()` / `subscribe()`), never over FFI.

mod session;
mod types;

#[cfg(test)]
mod scope_tests {
    use super::*;

    #[test]
    fn mobile_scope_facade_uses_the_shared_projection_before_limit() {
        let dir = std::env::temp_dir().join(format!("zeron-mobile-scope-{}", std::process::id()));
        let mut config = zc::ClientConfig::new("https://edge.invalid", &dir);
        config.device_id = "mobile-scope-test".into();
        let client = zc::Client::new(
            config,
            zc::Credentials::Demo(zc::DemoOptions {
                fixture: zc::DemoFixture::ProjectFilter,
                ..Default::default()
            }),
            Arc::new(zc::events::NullListener),
        )
        .unwrap();
        let core = CoreClient { client };
        let scope = SessionScope::Project {
            project_id: "space-zeron".into(),
        };
        let list = core.session_list(scope.clone());
        assert_eq!(list.scope, scope);
        assert_eq!(list.front.pinned.len(), 2);
        assert_eq!(list.front.sections[0].sessions.len(), 1);
        assert_eq!(list.archived.len(), 1);
        assert!(list.groups.is_empty(), "default options stay flat");
        let grouped = core.session_list_with_options(
            scope.clone(),
            SessionViewOptions {
                organization: SessionOrganization::ByProject,
                sort: SessionSort::Created,
            },
        );
        assert_eq!(grouped.groups.len(), 1);
        assert_eq!(grouped.groups[0].id, "project:space-zeron");
        assert_eq!(grouped.groups[0].sessions.len(), grouped.front.recent.len());
        let summary = core.project_deletion_summary("space-zeron".into()).unwrap();
        assert_eq!(summary.session_count, 6);
        assert_eq!(summary.archived_count, 1);
        assert!(core.project_deletion_summary("missing".into()).is_none());
        let hits = core.search_scoped("needle".into(), scope, false, 60);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].session.id, "chat-scoped-needle");
        assert_eq!(
            core.search_projects("studio archive/zeron".into())[0].id,
            "space-duplicate"
        );
        core.shutdown();
        let _ = std::fs::remove_dir_all(dir);
    }
}

use std::sync::Arc;

use zeron_client as zc;

pub use session::*;
pub use types::*;

/// Receives coalesced change events (at most one burst per display frame).
/// Called on a client thread: hop to the main thread and pull there.
#[uniffi::export(with_foreign)]
pub trait ClientListener: Send + Sync {
    fn on_event(&self, event: ClientEvent);
}

struct ListenerBridge(Arc<dyn ClientListener>);

impl zc::ClientListener for ListenerBridge {
    fn on_event(&self, event: zc::ClientEvent) {
        self.0.on_event(event.into());
    }
}

/// Upload progress: fraction of the file's bytes the host has committed.
#[uniffi::export(with_foreign)]
pub trait UploadProgress: Send + Sync {
    fn on_progress(&self, fraction: f64);
}

async fn on_runtime<T, F>(fut: F) -> CoreResult<T>
where
    F: std::future::Future<Output = Result<T, zc::ClientError>> + Send + 'static,
    T: Send + 'static,
{
    zc::runtime::run(fut).await.map_err(Into::into)
}

/// One signed-in account (or Demo mode). Create one per sign-in; call
/// `shutdown()` on sign-out.
#[derive(uniffi::Object)]
pub struct CoreClient {
    pub(crate) client: zc::Client,
}

#[allow(dead_code)] // consumed in Rust by the layout engine
impl CoreClient {
    /// The Rust client (crate-internal consumers).
    pub fn client(&self) -> &zc::Client {
        &self.client
    }

    /// The Rust session handle for `chat_id` — opening it if needed — for the
    /// layout engine: `handle.snapshot()` (an `Arc<SessionSnapshot>`) and
    /// `handle.subscribe()` (a `watch::Receiver`). `None` for an unknown chat.
    pub fn session_handle(&self, chat_id: &str) -> Option<zc::SessionHandle> {
        self.client.open_session(chat_id).ok()
    }
}

#[uniffi::export]
impl CoreClient {
    /// Build and start. Never blocks on the network: live mode hydrates from
    /// `data_dir` and connects in the background; Demo seeds its dataset.
    #[uniffi::constructor]
    pub fn new(
        config: CoreConfig,
        credentials: Credentials,
        listener: Arc<dyn ClientListener>,
    ) -> CoreResult<Arc<Self>> {
        let client = zc::Client::new(
            config.into(),
            credentials.into(),
            Arc::new(ListenerBridge(listener)),
        )?;
        Ok(Arc::new(Self { client }))
    }

    pub fn is_demo(&self) -> bool {
        self.client.is_demo()
    }

    pub fn device_id(&self) -> String {
        self.client.device_id().to_owned()
    }

    pub fn user_id(&self) -> String {
        self.client.user_id().to_owned()
    }

    pub fn org_id(&self) -> String {
        self.client.org_id().to_owned()
    }

    /// Stop all background work (sign-out). Snapshots stay readable.
    pub fn shutdown(&self) {
        self.client.shutdown();
    }

    /// The platform restored/re-signed a newer WorkOS pair.
    pub fn update_tokens(&self, tokens: AuthTokens) {
        self.client.update_tokens(tokens.into());
    }

    // ── workspace reads ────────────────────────────────────────────────────

    /// Current workspace revision (cheap; compare before pulling).
    pub fn workspace_revision(&self) -> u64 {
        self.client.workspace().revision
    }

    /// The whole derived workspace.
    pub fn workspace(&self) -> WorkspaceSnapshot {
        (&*self.client.workspace()).into()
    }

    /// Threads page: pinned, sections, recent.
    pub fn front_page(&self) -> FrontPage {
        (&self.client.workspace().front).into()
    }

    pub fn session_list(&self, scope: SessionScope) -> SessionList {
        self.session_list_with_options(scope, SessionViewOptions::default())
    }

    /// Scope-aware list with native grouping/sort options. `session_list`
    /// keeps the default (flat, last-updated) projection.
    pub fn session_list_with_options(
        &self,
        scope: SessionScope,
        options: SessionViewOptions,
    ) -> SessionList {
        self.client
            .workspace()
            .session_list_with_options(&scope.into(), options.into())
            .into()
    }

    /// Delete-confirmation facts for a project, computed over every chat
    /// (archived and child rows included). `None` when the project is gone.
    pub fn project_deletion_summary(&self, space_id: String) -> Option<ProjectDeletionSummary> {
        self.client
            .workspace()
            .project_deletion_summary(&space_id)
            .map(Into::into)
    }

    pub fn search_projects(&self, query: String) -> Vec<ProjectView> {
        self.client
            .workspace()
            .search_projects(&query)
            .iter()
            .map(Into::into)
            .collect()
    }

    pub fn search_scoped(
        &self,
        query: String,
        scope: SessionScope,
        include_archived: bool,
        limit: u32,
    ) -> Vec<SearchHit> {
        self.client
            .workspace()
            .search_scoped(&query, &scope.into(), include_archived, limit as usize)
            .iter()
            .map(Into::into)
            .collect()
    }

    pub fn projects(&self) -> Vec<ProjectView> {
        self.client
            .workspace()
            .projects
            .iter()
            .map(Into::into)
            .collect()
    }

    pub fn project(&self, space_id: String) -> Option<ProjectView> {
        self.client.workspace().project(&space_id).map(Into::into)
    }

    /// Active sessions without a project.
    pub fn projectless_sessions(&self) -> Vec<SessionRow> {
        rows(&self.client.workspace().projectless)
    }

    pub fn pull_requests(&self) -> PullRequestGroups {
        (&self.client.workspace().pull_requests).into()
    }

    pub fn archived_sessions(&self) -> Vec<SessionRow> {
        rows(&self.client.workspace().archived)
    }

    pub fn devices(&self) -> Vec<DeviceView> {
        self.client
            .workspace()
            .devices
            .iter()
            .map(Into::into)
            .collect()
    }

    /// Devices that can run sessions (new-session / new-project pickers).
    pub fn execution_devices(&self) -> Vec<DeviceView> {
        self.client
            .workspace()
            .execution_devices()
            .iter()
            .map(Into::into)
            .collect()
    }

    /// Any chat row (archived and child chats included).
    pub fn session_row(&self, chat_id: String) -> Option<SessionRow> {
        self.client
            .workspace()
            .session(&chat_id)
            .map(|r| (&**r).into())
    }

    /// Side chats / subagent chats of `parent_id`, recency order.
    pub fn child_sessions(&self, parent_id: String) -> Vec<SessionRow> {
        rows(self.client.workspace().children(&parent_id))
    }

    pub fn session_config(&self, chat_id: String) -> Option<ChatConfig> {
        self.client
            .session_config(&chat_id)
            .as_ref()
            .map(Into::into)
    }

    pub fn search(&self, query: String, limit: u32) -> Vec<SearchHit> {
        self.client
            .search(&query, limit as usize)
            .iter()
            .map(Into::into)
            .collect()
    }

    pub fn connectivity(&self) -> Connectivity {
        self.client.connectivity().into()
    }

    // ── workspace writes ───────────────────────────────────────────────────

    /// Mint a chat row (born on chat2). Returns the new chat id.
    pub fn create_session(&self, new_session: NewSession) -> CoreResult<String> {
        Ok(self.client.create_session(new_session.try_into()?)?)
    }

    pub fn archive_session(&self, chat_id: String) -> CoreResult<()> {
        Ok(self.client.archive_session(&chat_id)?)
    }

    pub fn unarchive_session(&self, chat_id: String) -> CoreResult<()> {
        Ok(self.client.unarchive_session(&chat_id)?)
    }

    pub fn rename_session(&self, chat_id: String, title: String) -> CoreResult<()> {
        Ok(self.client.rename_session(&chat_id, &title)?)
    }

    pub fn mark_seen(&self, chat_id: String) {
        self.client.mark_seen(&chat_id);
    }

    pub fn set_session_config(&self, chat_id: String, config: ChatConfig) -> CoreResult<()> {
        let config: zc::ChatConfig = config.try_into()?;
        Ok(self.client.set_session_config(&chat_id, &config)?)
    }

    pub fn delete_session(&self, chat_id: String) -> CoreResult<()> {
        Ok(self.client.delete_session(&chat_id)?)
    }

    pub fn pin_session(&self, chat_id: String) -> CoreResult<()> {
        Ok(self.client.pin_session(&chat_id)?)
    }

    pub fn unpin_session(&self, chat_id: String) -> CoreResult<()> {
        Ok(self.client.unpin_session(&chat_id)?)
    }

    /// Reorder a pin between neighbours (`None` at either end).
    pub fn move_pin(
        &self,
        chat_id: String,
        after: Option<String>,
        before: Option<String>,
    ) -> CoreResult<()> {
        Ok(self.client.move_pin(&chat_id, after, before)?)
    }

    /// Returns the new section id.
    pub fn create_section(&self, name: String) -> CoreResult<String> {
        Ok(self.client.create_section(&name)?)
    }

    pub fn rename_section(&self, section_id: String, name: String) -> CoreResult<()> {
        Ok(self.client.rename_section(&section_id, &name)?)
    }

    pub fn delete_section(&self, section_id: String) -> CoreResult<()> {
        Ok(self.client.delete_section(&section_id)?)
    }

    pub fn set_section_collapsed(&self, section_id: String, collapsed: bool) -> CoreResult<()> {
        Ok(self.client.set_section_collapsed(&section_id, collapsed)?)
    }

    /// Move a session into a section (`None` = back to recent).
    pub fn assign_section(&self, chat_id: String, section_id: Option<String>) -> CoreResult<()> {
        Ok(self.client.assign_section(&chat_id, section_id)?)
    }

    /// Create (or find) the project for `path` on `device_id`. Returns its id.
    pub async fn create_project(
        &self,
        device_id: String,
        path: String,
        git_detected: bool,
    ) -> CoreResult<String> {
        let client = self.client.clone();
        on_runtime(async move { client.create_project(&device_id, &path, git_detected).await })
            .await
    }

    pub fn rename_project(&self, space_id: String, name: Option<String>) -> CoreResult<()> {
        Ok(self.client.rename_project(&space_id, name.as_deref())?)
    }

    pub fn delete_project(&self, space_id: String) -> CoreResult<()> {
        Ok(self.client.delete_project(&space_id)?)
    }

    /// Mid-session ref switch (worktree retarget or `git checkout`).
    pub async fn switch_session_ref(&self, chat_id: String, reference: RepoRef) -> CoreResult<()> {
        let client = self.client.clone();
        on_runtime(async move { client.switch_session_ref(&chat_id, reference.into()).await }).await
    }

    // ── sessions ───────────────────────────────────────────────────────────

    /// Open (or return the open) session — instant from the local snapshot.
    pub fn open_session(&self, chat_id: String) -> CoreResult<Arc<SessionHandle>> {
        Ok(SessionHandle::new(self.client.open_session(&chat_id)?))
    }

    /// An already-open session.
    pub fn session(&self, chat_id: String) -> Option<Arc<SessionHandle>> {
        self.client.session(&chat_id).map(SessionHandle::new)
    }

    /// The view closed; the session stays warm until evicted.
    pub fn close_session(&self, chat_id: String) {
        self.client.close_session(&chat_id);
    }

    /// Warm the most relevant sessions (capped).
    pub fn preload_sessions(&self) {
        self.client.preload_sessions();
    }

    // ── host RPC ───────────────────────────────────────────────────────────

    /// Harness catalog of an execution device (static fallback when it's
    /// unreachable).
    pub async fn list_harnesses(&self, device_id: String) -> Vec<HarnessInfo> {
        let client = self.client.clone();
        on_runtime(async move { Ok(client.list_harnesses(&device_id).await) })
            .await
            .unwrap_or_else(|_| zc::catalog::fallback_harnesses())
            .into_iter()
            .map(Into::into)
            .collect()
    }

    /// Model catalog for `harness` on `device_id` (normalized; static fallback).
    pub async fn list_models(&self, device_id: String, harness: String) -> Vec<ModelInfo> {
        let client = self.client.clone();
        let fallback = harness.clone();
        on_runtime(async move { Ok(client.list_models(&device_id, &harness).await) })
            .await
            .unwrap_or_else(|_| zc::catalog::fallback_models(&fallback))
            .into_iter()
            .map(Into::into)
            .collect()
    }

    /// Ask for session notifications on this device (APNs token as hex,
    /// "production" | "sandbox", and which kinds).
    pub async fn register_push_target(
        &self,
        token: String,
        environment: String,
        prefs: PushPrefs,
    ) -> CoreResult<()> {
        let client = self.client.clone();
        on_runtime(async move {
            client
                .register_push_target(
                    &token,
                    &environment,
                    zc::PushPrefs {
                        done: prefs.done,
                        input: prefs.input,
                        failed: prefs.failed,
                    },
                )
                .await
        })
        .await
    }

    /// Stop session notifications to this device.
    pub async fn unregister_push_target(&self) -> CoreResult<()> {
        let client = self.client.clone();
        on_runtime(async move { client.unregister_push_target().await }).await
    }

    pub async fn list_refs(
        &self,
        device_id: String,
        repo_path: String,
    ) -> CoreResult<Vec<RepoRef>> {
        let client = self.client.clone();
        let refs =
            on_runtime(async move { client.list_refs(&device_id, &repo_path).await }).await?;
        Ok(refs.into_iter().map(Into::into).collect())
    }

    /// Files matching `query` in the chat's (or project's) workspace, for
    /// composer `@` mentions.
    pub async fn search_files(
        &self,
        device_id: String,
        chat_id: Option<String>,
        space_id: Option<String>,
        query: String,
    ) -> CoreResult<Vec<FileMatch>> {
        let client = self.client.clone();
        let files = on_runtime(async move {
            client
                .search_files(&device_id, chat_id, space_id, &query)
                .await
        })
        .await?;
        Ok(files
            .into_iter()
            .map(|f| FileMatch {
                path: f.path,
                is_dir: f.is_dir,
            })
            .collect())
    }

    /// Browse folders on a device (`None` = its home folder).
    pub async fn list_folders(
        &self,
        device_id: String,
        path: Option<String>,
    ) -> CoreResult<FolderListing> {
        let client = self.client.clone();
        Ok(
            on_runtime(async move { client.list_folders(&device_id, path).await })
                .await?
                .into(),
        )
    }

    /// `git checkout <ref>` in `repo_path` on the device.
    pub async fn switch_ref(
        &self,
        device_id: String,
        repo_path: String,
        ref_name: String,
    ) -> CoreResult<()> {
        let client = self.client.clone();
        on_runtime(async move { client.switch_ref(&device_id, &repo_path, &ref_name).await }).await
    }

    /// Create a worktree off `base`; returns its path.
    pub async fn create_worktree(
        &self,
        device_id: String,
        space_id: String,
        repo_path: String,
        base: String,
    ) -> CoreResult<String> {
        let client = self.client.clone();
        on_runtime(async move {
            client
                .create_worktree(&device_id, &space_id, &repo_path, &base)
                .await
        })
        .await
    }

    /// Chunked upload of one file; returns its durable path on the host.
    pub async fn upload_attachment(
        &self,
        device_id: String,
        name: String,
        data: Vec<u8>,
        progress: Option<Arc<dyn UploadProgress>>,
    ) -> CoreResult<String> {
        let client = self.client.clone();
        let progress: Option<zc::rpc::ProgressFn> = progress
            .map(|p| Arc::new(move |fraction: f64| p.on_progress(fraction)) as zc::rpc::ProgressFn);
        on_runtime(async move {
            client
                .upload_attachment(&device_id, &name, data, progress)
                .await
        })
        .await
    }

    /// Attachment bytes (LRU-cached): host paths and own `pending://` refs.
    pub async fn read_attachment(&self, device_id: String, path: String) -> CoreResult<Vec<u8>> {
        let client = self.client.clone();
        let bytes =
            on_runtime(async move { client.read_attachment(&device_id, &path).await }).await?;
        Ok(bytes.as_ref().clone())
    }

    // ── lifecycle ──────────────────────────────────────────────────────────

    /// OS network path: `false` only for a definitive "unsatisfied".
    pub fn set_network_online(&self, online: bool) {
        self.client.set_network_online(online);
    }

    /// App returned to the foreground: resume, redial/probe rooms.
    pub fn on_foreground(&self) {
        self.client.on_foreground();
    }

    /// App is backgrounding: persist now; pause time-driven work.
    pub fn on_background(&self) {
        self.client.on_background();
    }
}

// ── static helpers ─────────────────────────────────────────────────────────

/// Production edge base URL.
#[uniffi::export]
pub fn auth_production_edge_url() -> String {
    zc::auth::PRODUCTION_EDGE_URL.to_owned()
}

/// OAuth callback scheme (`zeron`).
#[uniffi::export]
pub fn auth_callback_scheme() -> String {
    zc::auth::CALLBACK_SCHEME.to_owned()
}

/// WorkOS AuthKit authorize URL (start of the web-auth session).
#[uniffi::export]
pub fn workos_authorize_url(state: String) -> String {
    zc::auth::workos_authorize_url(&state)
}

/// `code`/`state` (or the provider error) of a `zeron://callback?…` URL.
#[uniffi::export]
pub fn parse_auth_callback(url: String) -> Option<AuthCallback> {
    zc::auth::parse_auth_callback(&url).map(Into::into)
}

/// Exchange an authorization code for an (unscoped) token pair.
#[uniffi::export]
pub async fn auth_exchange_code(edge_url: String, code: String) -> CoreResult<AuthExchange> {
    let exchange = zc::auth::exchange_code(&edge_url, &code).await?;
    Ok(AuthExchange {
        user: exchange.user.into(),
        tokens: exchange.tokens.into(),
    })
}

/// Organizations the (unscoped) access token's user belongs to.
#[uniffi::export]
pub async fn auth_list_orgs(edge_url: String, access_token: String) -> CoreResult<Vec<AuthOrg>> {
    Ok(zc::auth::list_orgs(&edge_url, &access_token)
        .await?
        .into_iter()
        .map(Into::into)
        .collect())
}

/// Rotate the pair; `organization_id` scopes the access token to an org.
#[uniffi::export]
pub async fn auth_refresh(
    edge_url: String,
    refresh_token: String,
    organization_id: Option<String>,
) -> CoreResult<AuthTokens> {
    Ok(
        zc::auth::refresh(&edge_url, &refresh_token, organization_id.as_deref())
            .await?
            .into(),
    )
}

/// A JWT's `exp` claim (epoch seconds).
#[uniffi::export]
pub fn jwt_expiry(jwt: String) -> Option<i64> {
    zc::auth::jwt_expiry(&jwt)
}

/// Static harness catalog (the device-unreachable fallback).
#[uniffi::export]
pub fn fallback_harnesses() -> Vec<HarnessInfo> {
    zc::catalog::fallback_harnesses()
        .into_iter()
        .map(Into::into)
        .collect()
}

/// Curated models for a harness (first = default).
#[uniffi::export]
pub fn fallback_models(harness: String) -> Vec<ModelInfo> {
    zc::catalog::fallback_models(&harness)
        .into_iter()
        .map(Into::into)
        .collect()
}

#[uniffi::export]
pub fn harness_label(harness: String) -> String {
    zc::catalog::harness_label(&harness)
}

#[uniffi::export]
pub fn model_label(harness: String, model: String) -> String {
    zc::catalog::model_label(&harness, &model)
}

#[uniffi::export]
pub fn reasoning_label(level: String) -> String {
    zc::catalog::reasoning_label(&level)
}

/// `now` / `34m` / `4h` / `2d`.
#[uniffi::export]
pub fn relative_time_label(at_ms: i64, now_ms: i64) -> String {
    zc::relative_time_label(at_ms, now_ms)
}

/// Stable palette slot for a project path (`"home"` without a project).
#[uniffi::export]
pub fn project_color_index(space_path: String) -> u32 {
    zc::project_color_index(&space_path)
}

/// Size of the project palette `project_color_index` indexes.
#[uniffi::export]
pub fn project_color_count() -> u32 {
    zc::workspace::PROJECT_COLOR_COUNT
}

/// Split a user prompt into visible text + attachment refs.
#[uniffi::export]
pub fn parse_user_message(content: String) -> ParsedUserMessage {
    zc::attachments::parse_user_message(&content).into()
}

/// Largest attachment the composer may pick (bytes).
#[uniffi::export]
pub fn max_attachment_bytes() -> u64 {
    zc::attachments::MAX_ATTACHMENT_BYTES as u64
}
