//! Cloud from a device with no engine (the phone).
//!
//! The same edge calls the desktop engine makes for Settings → Cloud
//! (`crates/engine/src/cloud_client.rs`): the account's status, its session
//! machines, GitHub through the vault, the vault's agent logins and API keys,
//! and billing. Every call acts for the signed-in user with their own bearer.
//!
//! A Cloud project is a registry row owned by the logical Cloud device, so
//! the phone writes it directly; a Cloud session is minted by the edge
//! (`POST /cloud/{org}/sessions`), then its chat row is written hosted on the
//! session's own machine — what the engine's `createChat {cloud}` does.
//!
//! Demo mode answers from fixtures so the screens can be driven offline.

use chrono::Utc;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use zeron_proto::{
    BillingLink, BillingStatus, Chat, CloudSession, CloudSessions, CloudState, CloudStatus,
    GithubConnectProgress, GithubDeviceFlow, GithubRepo, VaultAccount, VaultProvider, VaultStatus,
};

use crate::Client;
use crate::client::SessionTarget;
use crate::error::{ClientError, Result};

/// Where a session machine checks a repository out (the edge's choice; a
/// Cloud project's path matches it so the explorer reads alike).
const CLOUD_HOME: &str = "/home/user";

/// Why a request produced no value.
enum Failure {
    /// A read the edge can't route (404 / 501 / 503): "not offered here".
    Missing(ClientError),
    Error(ClientError),
}

impl Failure {
    fn into_error(self) -> ClientError {
        match self {
            Failure::Missing(error) | Failure::Error(error) => error,
        }
    }
}

/// What a Cloud project needs from its repository to start a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudRepoRef {
    pub full_name: String,
    pub clone_url: String,
    pub default_branch: String,
}

impl From<&GithubRepo> for CloudRepoRef {
    fn from(repo: &GithubRepo) -> Self {
        Self {
            full_name: repo.full_name.clone(),
            clone_url: repo.clone_url.clone(),
            default_branch: repo.default_branch.clone(),
        }
    }
}

impl Client {
    // ── account ─────────────────────────────────────────────────────────────

    /// `GET /cloud/{org}`. An edge without Cloud reads as `available: false`.
    pub async fn cloud_status(&self) -> Result<CloudStatus> {
        if self.inner.is_demo() {
            return Ok(demo::status());
        }
        match self
            .cloud_call::<CloudStatus>(reqwest::Method::GET, &["cloud"], &[], None, "Cloud status")
            .await
        {
            // The edge says whether this account is offered Cloud
            // (`CLOUD_ACCESS`): never assumed from a 200.
            Ok(status) => Ok(status),
            Err(Failure::Missing(_)) => Ok(CloudStatus::default()),
            Err(Failure::Error(error)) => Err(error),
        }
    }

    /// `POST /cloud/{org}/enable` — Cloud is part of every signed-in account;
    /// one not set up yet is set up (no machine starts until a chat runs).
    pub async fn cloud_enable(&self) -> Result<CloudStatus> {
        if self.inner.is_demo() {
            return Ok(demo::status());
        }
        self.cloud_call::<CloudStatus>(
            reqwest::Method::POST,
            &["cloud", "enable"],
            &[],
            Some(serde_json::json!({})),
            "Turn on Cloud",
        )
        .await
        .map_err(Failure::into_error)
    }

    /// The logical Cloud device (what Cloud projects belong to and logins
    /// are authorized for), setting Cloud up first when it's off.
    pub async fn cloud_device_id(&self) -> Result<String> {
        let mut status = self.cloud_status().await?;
        if !status.available {
            return Err(ClientError::Unsupported(
                "Cloud isn't available for this account.".into(),
            ));
        }
        if status.state == CloudState::Off || status.device_id.is_none() {
            status = self.cloud_enable().await?;
        }
        status
            .device_id
            .ok_or_else(|| ClientError::HostError("Cloud is still being set up.".into()))
    }

    /// `GET /cloud/{org}/sessions`.
    pub async fn cloud_sessions(&self) -> Result<CloudSessions> {
        if self.inner.is_demo() {
            return Ok(CloudSessions {
                sessions: Vec::new(),
                available: true,
            });
        }
        match self
            .cloud_call::<CloudSessions>(
                reqwest::Method::GET,
                &["cloud", "sessions"],
                &[],
                None,
                "Cloud sessions",
            )
            .await
        {
            Ok(sessions) => Ok(CloudSessions {
                available: true,
                ..sessions
            }),
            Err(Failure::Missing(_)) => Ok(CloudSessions::default()),
            Err(Failure::Error(error)) => Err(error),
        }
    }

    /// `POST /cloud/{org}/sessions/{chatId}/{wake|sleep}` — `chat_id` is the
    /// session's own chat (a side chat runs in its parent's machine).
    pub async fn cloud_session_action(&self, chat_id: &str, wake: bool) -> Result<CloudSession> {
        if self.inner.is_demo() {
            return Err(ClientError::Unsupported(
                "No Cloud machines in the demo.".into(),
            ));
        }
        let action = if wake { "wake" } else { "sleep" };
        self.cloud_call::<CloudSession>(
            reqwest::Method::POST,
            &["cloud", "sessions", chat_id, action],
            &[],
            Some(serde_json::json!({})),
            if wake {
                "Wake the Cloud agent"
            } else {
                "Put the Cloud agent to sleep"
            },
        )
        .await
        .map_err(Failure::into_error)
    }

    /// `DELETE /cloud/{org}/sessions/{chatId}` — the session's machine and
    /// anything uncommitted on it.
    pub async fn cloud_delete_session(&self, chat_id: &str) -> Result<CloudSession> {
        if self.inner.is_demo() {
            return Err(ClientError::Unsupported(
                "No Cloud machines in the demo.".into(),
            ));
        }
        self.cloud_call::<CloudSession>(
            reqwest::Method::DELETE,
            &["cloud", "sessions", chat_id],
            &[],
            None,
            "Delete the Cloud agent",
        )
        .await
        .map_err(Failure::into_error)
    }

    /// Deleted Cloud chats take their machines with them, in the background:
    /// the chats are already gone. A chat with no machine of its own (a side
    /// chat in its parent's) has no session, and the edge says so.
    pub(crate) fn delete_cloud_machines(&self, chat_ids: Vec<String>) {
        if chat_ids.is_empty() || self.inner.is_demo() {
            return;
        }
        let client = self.clone();
        crate::runtime::handle().spawn(async move {
            for chat_id in chat_ids {
                if let Err(err) = client.cloud_delete_session(&chat_id).await {
                    tracing::warn!(chat = %chat_id, error = %err, "Cloud machine not deleted");
                }
            }
        });
    }

    // ── GitHub (through the vault) ──────────────────────────────────────────

    /// `GET /vault/{org}/github/repos[?q=]` — the repositories Cloud reaches.
    /// Not connected reads as an empty list.
    pub async fn github_repos(&self, query: Option<&str>) -> Result<Vec<GithubRepo>> {
        if self.inner.is_demo() {
            return Ok(demo::repos(query));
        }
        #[derive(Deserialize)]
        struct Repos {
            #[serde(default)]
            repos: Vec<GithubRepo>,
        }
        let query: Vec<(&str, &str)> = query
            .map(str::trim)
            .filter(|q| !q.is_empty())
            .map(|q| ("q", q))
            .into_iter()
            .collect();
        match self
            .cloud_call::<Repos>(
                reqwest::Method::GET,
                &["vault", "github", "repos"],
                &query,
                None,
                "GitHub repositories",
            )
            .await
        {
            Ok(repos) => {
                let mut cache = crate::lock(&self.inner.cloud_repos);
                for repo in &repos.repos {
                    cache.insert(repo.full_name.to_ascii_lowercase(), CloudRepoRef::from(repo));
                }
                drop(cache);
                Ok(repos.repos)
            }
            Err(Failure::Missing(_)) => Ok(Vec::new()),
            Err(Failure::Error(error)) => Err(error),
        }
    }

    /// `GET /vault/{org}/github/branches?repo=owner/name`.
    pub async fn github_branches(&self, repo: &str) -> Result<Vec<String>> {
        if self.inner.is_demo() {
            return Ok(vec!["main".into(), "develop".into()]);
        }
        #[derive(Deserialize)]
        struct Branches {
            #[serde(default)]
            branches: Vec<String>,
        }
        self.cloud_call::<Branches>(
            reqwest::Method::GET,
            &["vault", "github", "branches"],
            &[("repo", repo)],
            None,
            "GitHub branches",
        )
        .await
        .map(|b| b.branches)
        .map_err(Failure::into_error)
    }

    /// `POST /vault/{org}/github/device` — GitHub's device flow: show the
    /// code, open the page, poll [`Self::github_connect_poll`].
    pub async fn github_connect_start(&self) -> Result<GithubDeviceFlow> {
        if self.inner.is_demo() {
            return Ok(GithubDeviceFlow {
                flow_id: "demo".into(),
                user_code: "ZRN-4821".into(),
                verification_uri: "https://github.com/login/device".into(),
                interval_secs: 5,
                expires_at: crate::now_ms() + 15 * 60 * 1000,
            });
        }
        let device = self.cloud_device_id().await?;
        self.cloud_call::<GithubDeviceFlow>(
            reqwest::Method::POST,
            &["vault", "github", "device"],
            &[],
            Some(serde_json::json!({ "authorizedDevices": [device] })),
            "Connect GitHub",
        )
        .await
        .map_err(Failure::into_error)
    }

    /// `POST /vault/{org}/github/device/{flowId}`.
    pub async fn github_connect_poll(&self, flow_id: &str) -> Result<GithubConnectProgress> {
        if self.inner.is_demo() {
            return Ok(GithubConnectProgress {
                state: zeron_proto::GithubConnectState::Connected,
                account: Some("octocat".into()),
                error: None,
            });
        }
        self.cloud_call::<GithubConnectProgress>(
            reqwest::Method::POST,
            &["vault", "github", "device", flow_id],
            &[],
            Some(serde_json::json!({})),
            "Connect GitHub",
        )
        .await
        .map_err(Failure::into_error)
    }

    // ── vault: agent logins and keys ────────────────────────────────────────

    /// `GET /vault/{org}` — which providers are connected (GitHub included).
    pub async fn vault_status(&self) -> Result<VaultStatus> {
        if self.inner.is_demo() {
            return Ok(demo::vault());
        }
        match self
            .cloud_call::<VaultStatus>(reqwest::Method::GET, &["vault"], &[], None, "Vault status")
            .await
        {
            Ok(status) => Ok(VaultStatus {
                available: true,
                ..status
            }),
            Err(Failure::Missing(_)) => Ok(VaultStatus::default()),
            Err(Failure::Error(error)) => Err(error),
        }
    }

    /// `GET /vault/{org}/accounts/{provider}` — every stored login.
    pub async fn vault_accounts(&self, provider: VaultProvider) -> Result<Vec<VaultAccount>> {
        if self.inner.is_demo() {
            return Ok(demo::accounts(provider));
        }
        self.account_list(
            reqwest::Method::GET,
            &["vault", "accounts", provider.as_str()],
            None,
            "Cloud accounts",
        )
        .await
    }

    /// `POST /vault/{org}/accounts/{provider}/active {slot}` — the login
    /// Cloud uses (`None`: none of this provider's).
    pub async fn vault_activate(
        &self,
        provider: VaultProvider,
        slot: Option<&str>,
    ) -> Result<Vec<VaultAccount>> {
        if self.inner.is_demo() {
            return Ok(demo::accounts(provider));
        }
        self.account_list(
            reqwest::Method::POST,
            &["vault", "accounts", provider.as_str(), "active"],
            Some(serde_json::json!({ "slot": slot })),
            "Switch the Cloud account",
        )
        .await
    }

    /// `GET /vault/{org}/accounts/{provider}/{slot}/usage` — a Cloud login's
    /// plan usage, read by the vault (it alone holds the tokens) and parsed
    /// as the desktop does: windows and the live plan. `None` when the reply
    /// carries no window.
    pub async fn vault_usage(
        &self,
        provider: VaultProvider,
        slot: &str,
    ) -> Result<Option<(Vec<zeron_proto::AgentUsageWindow>, Option<String>)>> {
        let harness = match provider {
            VaultProvider::Claude => zeron_proto::HarnessId::ClaudeCode,
            VaultProvider::Codex => zeron_proto::HarnessId::Codex,
            _ => return Ok(None),
        };
        if self.inner.is_demo() {
            let window = |label: &str, used: f32, hours: i64| zeron_proto::AgentUsageWindow {
                label: label.into(),
                used_fraction: used,
                resets_at: Some(Utc::now() + chrono::Duration::hours(hours)),
            };
            return Ok(Some((
                vec![window("Session", 0.52, 2), window("Week", 0.27, 80)],
                None,
            )));
        }
        #[derive(Deserialize)]
        struct Usage {
            body: serde_json::Value,
        }
        let usage = self
            .cloud_call::<Usage>(
                reqwest::Method::GET,
                &["vault", "accounts", provider.as_str(), slot, "usage"],
                &[],
                None,
                "Cloud account usage",
            )
            .await
            .map_err(Failure::into_error)?;
        Ok(zeron_proto::plan_usage::from_reply(harness, &usage.body))
    }

    /// `DELETE /vault/{org}/accounts/{provider}/{slot}`.
    pub async fn vault_forget(
        &self,
        provider: VaultProvider,
        slot: &str,
    ) -> Result<Vec<VaultAccount>> {
        if self.inner.is_demo() {
            return Ok(Vec::new());
        }
        self.account_list(
            reqwest::Method::DELETE,
            &["vault", "accounts", provider.as_str(), slot],
            None,
            "Remove the Cloud account",
        )
        .await
    }

    /// `PUT /vault/{org}/credentials/{anthropic-key|openai-key}` — the user's
    /// own API key, made the one Cloud uses (the subscription login steps
    /// aside, as on desktop).
    pub async fn vault_put_api_key(&self, provider: VaultProvider, key: &str) -> Result<()> {
        let login = match provider {
            VaultProvider::AnthropicKey => VaultProvider::Claude,
            VaultProvider::OpenaiKey => VaultProvider::Codex,
            other => {
                return Err(ClientError::InvalidArgument(format!(
                    "{} isn't an API key provider",
                    other.as_str()
                )));
            }
        };
        let key = key.trim();
        if key.is_empty() {
            return Err(ClientError::InvalidArgument(
                "Paste an API key first.".into(),
            ));
        }
        if key.chars().any(char::is_whitespace) {
            return Err(ClientError::InvalidArgument(
                "That doesn't look like an API key (it contains spaces).".into(),
            ));
        }
        if self.inner.is_demo() {
            return Ok(());
        }
        self.put_credential(provider, serde_json::json!({ "key": key }), None)
            .await?;
        self.vault_activate(login, None).await.map(drop)
    }

    /// `PUT /vault/{org}/credentials/{provider}` for the logical Cloud device:
    /// every session machine (its child) may draw grants from it.
    pub(crate) async fn put_credential(
        &self,
        provider: VaultProvider,
        material: serde_json::Value,
        profile: Option<zeron_proto::VaultAccountProfile>,
    ) -> Result<()> {
        let device = self.cloud_device_id().await?;
        let mut body = serde_json::json!({
            "material": material,
            "authorizedDevices": [device],
        });
        if let Some(profile) = profile {
            body["profile"] = serde_json::to_value(profile).unwrap_or_default();
        }
        self.cloud_call::<VaultStatus>(
            reqwest::Method::PUT,
            &["vault", "credentials", provider.as_str()],
            &[],
            Some(body),
            "Save the sign-in",
        )
        .await
        .map(drop)
        .map_err(Failure::into_error)
    }

    // ── billing ─────────────────────────────────────────────────────────────

    /// `GET /cloud/{org}/billing`.
    pub async fn billing_status(&self) -> Result<BillingStatus> {
        if self.inner.is_demo() {
            return Ok(demo::billing());
        }
        match self
            .cloud_call::<BillingStatus>(
                reqwest::Method::GET,
                &["cloud", "billing"],
                &[],
                None,
                "Billing",
            )
            .await
        {
            Ok(status) => Ok(status),
            Err(Failure::Missing(_)) => Ok(BillingStatus::default()),
            Err(Failure::Error(error)) => Err(error),
        }
    }

    /// A Stripe Checkout page: a plan (`kind = "subscription"`) or extra
    /// credits (`kind = "credits"`, `packs`).
    pub async fn billing_checkout(
        &self,
        kind: &str,
        plan: Option<&str>,
        packs: Option<u32>,
    ) -> Result<String> {
        let mut body = serde_json::json!({ "kind": kind });
        if let Some(plan) = plan {
            body["plan"] = plan.into();
        }
        if let Some(packs) = packs {
            body["packs"] = packs.into();
        }
        self.billing_link(&["cloud", "billing", "checkout"], body)
            .await
    }

    /// The Stripe customer portal (change or cancel a plan).
    pub async fn billing_portal(&self) -> Result<String> {
        self.billing_link(&["cloud", "billing", "portal"], serde_json::json!({}))
            .await
    }

    async fn billing_link(&self, segments: &[&str], body: serde_json::Value) -> Result<String> {
        if self.inner.is_demo() {
            return Err(ClientError::Unsupported("No billing in the demo.".into()));
        }
        self.cloud_call::<BillingLink>(reqwest::Method::POST, segments, &[], Some(body), "Billing")
            .await
            .map(|link| link.url)
            .map_err(Failure::into_error)
    }

    // ── Cloud projects and sessions ─────────────────────────────────────────

    /// Make `repo` a project that lives on Cloud alone (or return the one
    /// Cloud already has for it). The logical Cloud device has no engine to
    /// stamp the row, so it's written whole here.
    pub async fn create_cloud_project(&self, repo_full_name: &str) -> Result<String> {
        let device = if self.inner.is_demo() {
            demo::CLOUD_DEVICE.to_owned()
        } else {
            self.cloud_device_id().await?
        };
        let (state, _) = self.inner.workspace.state();
        if let Some(existing) = state.spaces.iter().find(|space| {
            space.device_id == device
                && space
                    .github_repo
                    .as_deref()
                    .is_some_and(|own| own.eq_ignore_ascii_case(repo_full_name))
        }) {
            return Ok(existing.id.clone());
        }
        let name = repo_full_name
            .rsplit('/')
            .next()
            .unwrap_or(repo_full_name)
            .to_owned();
        let space = zeron_proto::Space {
            id: crate::new_id(),
            device_id: device,
            path: format!("{CLOUD_HOME}/{name}"),
            name: None,
            git_detected: true,
            git_checked_at: Some(Utc::now()),
            checkout_id: None,
            // Groups with the clones of the repository on the user's devices.
            repository_id: Some(zeron_proto::view::cloud_repository_id(
                &state.spaces,
                repo_full_name,
            )),
            github_repo: Some(repo_full_name.to_owned()),
            created_at: Utc::now(),
        };
        let id = space.id.clone();
        self.inner.registry_write(|doc| doc.upsert_space(&space))?;
        Ok(id)
    }

    /// The repository a project's Cloud sessions clone, when Cloud reaches it.
    pub async fn cloud_repo_for_project(&self, space_id: &str) -> Result<Option<CloudRepoRef>> {
        let (state, _) = self.inner.workspace.state();
        let Some(name) = state
            .spaces
            .iter()
            .find(|s| s.id == space_id)
            .and_then(|s| s.github_repo.clone())
        else {
            return Ok(None);
        };
        // Already read (the new-session screen lists them): no round trip.
        if let Some(repo) = crate::lock(&self.inner.cloud_repos).get(&name.to_ascii_lowercase()) {
            return Ok(Some(repo.clone()));
        }
        Ok(self
            .github_repos(Some(&name))
            .await?
            .iter()
            .find(|repo| repo.full_name.eq_ignore_ascii_case(&name))
            .map(CloudRepoRef::from))
    }

    /// Start a session on its own Cloud machine in `space_id`'s repository:
    /// the edge mints the machine (idempotent per chat), then the chat row is
    /// written hosted there. Its first message wakes the machine like any
    /// other send. Returns the chat id.
    pub async fn create_cloud_session(
        &self,
        space_id: &str,
        config: Option<zeron_proto::ChatConfig>,
        branch: Option<String>,
        title: Option<String>,
    ) -> Result<String> {
        if self.inner.is_demo() {
            // The demo's machines are its own simulated host.
            return self.create_session(crate::NewSession {
                target: SessionTarget::Project {
                    space_id: space_id.to_owned(),
                },
                config,
                branch,
                cwd: None,
                title,
            });
        }
        let repo = self
            .cloud_repo_for_project(space_id)
            .await?
            .ok_or_else(|| {
                ClientError::Unsupported(
                "Cloud can't reach this project's repository — install the Zeron GitHub App on it."
                    .into(),
            )
            })?;
        let chat_id = crate::new_id();
        let mut body = serde_json::json!({
            "chatId": chat_id,
            "spaceId": space_id,
            "repo": {
                "fullName": repo.full_name,
                "cloneUrl": repo.clone_url,
                "defaultBranch": repo.default_branch,
            },
        });
        if let Some(branch) = &branch {
            body["branch"] = serde_json::Value::String(branch.clone());
        }
        let session = self
            .cloud_call::<CloudSession>(
                reqwest::Method::POST,
                &["cloud", "sessions"],
                &[],
                Some(body),
                "Start a Cloud session",
            )
            .await
            .map_err(Failure::into_error)?;
        let chat = Chat {
            id: chat_id.clone(),
            device_id: session.device_id,
            title: title.filter(|t| !t.trim().is_empty()),
            archived: false,
            cwd: session.path.or_else(|| {
                let name = repo.full_name.rsplit('/').next().unwrap_or("repo");
                Some(format!("{CLOUD_HOME}/{name}"))
            }),
            branch: branch.or(Some(repo.default_branch)),
            checkout_id: None,
            source_context: None,
            config,
            last_message_preview: None,
            last_message_at: None,
            created_at: Utc::now(),
            harness_session_id: None,
            harness_session_harness: None,
            harness_session_cwd: None,
            space_id: Some(space_id.to_owned()),
            last_seen_at: None,
            room_gen: Some(2),
            parent_chat_id: None,
        };
        self.inner.registry_write(|doc| doc.upsert_chat(&chat))?;
        Ok(chat_id)
    }

    /// Cloud's models for `harness` as the edge lists them (kept there by
    /// hand, so a new model needs no app release, and no computer or Cloud
    /// machine is asked); the last list read while the edge is out of reach;
    /// the curated static list before the first.
    pub async fn cloud_models(&self, harness: &str) -> Vec<crate::catalog::ModelInfo> {
        const CLOUD_CATALOG: &str = "cloud";
        #[derive(Deserialize)]
        struct Models {
            #[serde(default)]
            models: std::collections::HashMap<String, Vec<crate::catalog::ModelInfo>>,
        }
        let cache = crate::catalog::DiskCatalog::new(&self.inner.config.data_dir);
        if let Ok(reply) = self
            .cloud_call::<Models>(
                reqwest::Method::GET,
                &["cloud", "models"],
                &[],
                None,
                "Read Cloud's models",
            )
            .await
        {
            for (agent, list) in reply.models {
                if !list.is_empty() {
                    let list = crate::catalog::normalize_models(&agent, list);
                    cache.put_models(CLOUD_CATALOG, &agent, &list);
                }
            }
            // Rows and chips pick up newly learned model labels.
            self.inner.recompute_workspace();
        }
        match cache.models(CLOUD_CATALOG, harness) {
            Some(list) => {
                crate::catalog::learn_labels(harness, &list);
                list
            }
            None => crate::catalog::fallback_models(harness),
        }
    }

    // ── plumbing ────────────────────────────────────────────────────────────

    async fn account_list(
        &self,
        method: reqwest::Method,
        segments: &[&str],
        body: Option<serde_json::Value>,
        what: &str,
    ) -> Result<Vec<VaultAccount>> {
        #[derive(Deserialize)]
        struct AccountList {
            #[serde(default)]
            accounts: Vec<VaultAccount>,
        }
        self.cloud_call::<AccountList>(method, segments, &[], body, what)
            .await
            .map(|list| list.accounts)
            .map_err(Failure::into_error)
    }

    /// `{edge}/{root}/{org}/{rest…}` with the user's bearer.
    async fn cloud_call<T: DeserializeOwned>(
        &self,
        method: reqwest::Method,
        segments: &[&str],
        query: &[(&str, &str)],
        body: Option<serde_json::Value>,
        what: &str,
    ) -> std::result::Result<T, Failure> {
        let live = self.inner.live().ok_or_else(|| {
            Failure::Error(ClientError::Unsupported("Cloud needs an account.".into()))
        })?;
        let mut url = reqwest::Url::parse(&live.edge)
            .map_err(|e| Failure::Error(ClientError::Internal(format!("invalid edge url: {e}"))))?;
        {
            let mut path = url
                .path_segments_mut()
                .map_err(|_| Failure::Error(ClientError::Internal("invalid edge url".into())))?;
            path.pop_if_empty();
            let (root, rest) = segments.split_first().expect("a route root");
            path.push(root);
            path.push(self.inner.credentials.org_id());
            path.extend(rest);
        }
        if !query.is_empty() {
            url.query_pairs_mut().extend_pairs(query);
        }
        let is_read = method == reqwest::Method::GET;
        let token = self.inner.tokens.bearer().await.map_err(Failure::Error)?;
        let mut request = crate::auth::http().request(method, url).bearer_auth(token);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.map_err(|e| {
            Failure::Error(ClientError::Network(format!(
                "Couldn't reach Zeron's servers: {e}"
            )))
        })?;
        let status = response.status().as_u16();
        let bytes = response.bytes().await.map_err(|e| {
            Failure::Error(ClientError::Network(format!(
                "{what}: the reply was cut off: {e}"
            )))
        })?;
        if !(200..300).contains(&status) {
            let error = edge_error(status, &bytes, what);
            // A read the edge can't route is "not offered here". A 503 on
            // the status read itself is the edge failing for a moment (it
            // answers that read even unconfigured): an error, to retry.
            let status_read = segments == ["cloud"];
            return Err(if is_read && (matches!(status, 404 | 501) || (status == 503 && !status_read)) {
                Failure::Missing(error)
            } else {
                Failure::Error(error)
            });
        }
        serde_json::from_slice(&bytes).map_err(|e| {
            Failure::Error(ClientError::HostError(format!(
                "{what}: unexpected reply from Zeron's servers ({e})"
            )))
        })
    }
}

/// The edge's `{error, message}` body, as text a person can act on (the
/// desktop engine's wording, `cloud_client::edge_error`).
fn edge_error(status: u16, body: &[u8], what: &str) -> ClientError {
    #[derive(Deserialize)]
    struct EdgeErrorBody {
        #[serde(default)]
        error: Option<String>,
        #[serde(default)]
        message: Option<String>,
    }
    let parsed = serde_json::from_slice::<EdgeErrorBody>(body).ok();
    let message = parsed
        .as_ref()
        .and_then(|b| b.message.as_deref())
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .map(str::to_owned);
    let code = parsed.and_then(|b| b.error);
    let text = message.unwrap_or_else(|| match (code.as_deref(), status) {
        (Some("needs_reconnect"), _) => {
            "The provider rejected the stored sign-in — sign in again.".into()
        }
        (Some("out_of_credits"), _) => "You're out of Cloud credits.".into(),
        (Some("upstream"), _) => "The provider didn't answer — try again in a moment.".into(),
        (_, 400) => format!("{what}: the request was rejected."),
        (_, 401) => "Your Zeron session expired — sign in again.".into(),
        (_, 402) => "You're out of Cloud credits.".into(),
        (_, 403) => "This account can't manage Cloud for this workspace.".into(),
        (_, 404) => format!("{what}: not found."),
        (_, 409) => "Cloud is busy with another change — try again in a moment.".into(),
        (_, 429) => "Too many requests — try again in a moment.".into(),
        (_, 501 | 503) => "Cloud isn't available on this Zeron server yet.".into(),
        (_, status) if status >= 500 => {
            format!("{what} failed on Zeron's servers ({status}) — try again.")
        }
        (_, status) => format!("{what} failed ({status})."),
    });
    match status {
        401 => ClientError::Auth(text),
        _ => ClientError::HostError(text),
    }
}

/// Offline answers for the demo workspace.
mod demo {
    use zeron_proto::{
        BillingCredits, BillingPack, BillingPlan, BillingStatus, BillingSubscription, CloudState,
        CloudStatus, GithubRepo, VaultAccount, VaultConnection, VaultConnectionStatus,
        VaultProvider, VaultStatus,
    };

    pub(super) const CLOUD_DEVICE: &str = "cloud-demo";

    pub(super) fn status() -> CloudStatus {
        CloudStatus {
            state: CloudState::Ready,
            device_id: Some(CLOUD_DEVICE.into()),
            available: true,
            credits: Some(41_250.0),
            max_awake_sessions: 3,
            ..Default::default()
        }
    }

    pub(super) fn repos(query: Option<&str>) -> Vec<GithubRepo> {
        let all = [
            ("acme/web", "Marketing site and docs", false, 6),
            ("acme/api", "The public API", true, 5),
            ("acme/mobile", "iOS and Android apps", true, 4),
            ("acme/infra", "Terraform and deploys", true, 3),
            ("octocat/dotfiles", "", false, 2),
        ];
        let needle = query.unwrap_or("").trim().to_lowercase();
        all.into_iter()
            .filter(|(name, ..)| name.contains(&needle))
            .map(|(name, description, private, pushed)| GithubRepo {
                full_name: name.into(),
                clone_url: format!("https://github.com/{name}.git"),
                default_branch: "main".into(),
                private,
                description: (!description.is_empty()).then(|| description.into()),
                pushed_at: pushed,
            })
            .collect()
    }

    pub(super) fn vault() -> VaultStatus {
        let connection = |provider, account: &str| VaultConnection {
            provider,
            status: VaultConnectionStatus::Connected,
            authorized_devices: vec![CLOUD_DEVICE.into()],
            account: Some(account.into()),
            updated_at: 0,
            accounts: 1,
            has_active: true,
        };
        VaultStatus {
            connections: vec![
                connection(VaultProvider::Github, "octocat"),
                connection(VaultProvider::Claude, "you@example.com"),
            ],
            devices: Vec::new(),
            available: true,
            github_install_url: Some("https://github.com/apps/zeron/installations/new".into()),
        }
    }

    pub(super) fn accounts(provider: VaultProvider) -> Vec<VaultAccount> {
        match provider {
            VaultProvider::Claude => vec![VaultAccount {
                slot: "demo-claude".into(),
                status: VaultConnectionStatus::Connected,
                active: true,
                kind: "oauth".into(),
                account: Some("you@example.com".into()),
                email: Some("you@example.com".into()),
                display_name: Some("You".into()),
                organization: None,
                plan: Some("Max 20×".into()),
                authorized_devices: vec![CLOUD_DEVICE.into()],
                created_at: 0,
                updated_at: 0,
            }],
            _ => Vec::new(),
        }
    }

    pub(super) fn billing() -> BillingStatus {
        let plan = |id: &str, name: &str, price: u32, credits: f64| BillingPlan {
            id: id.into(),
            name: name.into(),
            price_cents: price,
            monthly_credits: credits,
        };
        BillingStatus {
            available: true,
            plan: Some(BillingSubscription {
                plan: plan("base", "Base", 2000, 45_000.0),
                status: "active".into(),
                period_end: crate::now_ms() + 12 * 24 * 3600 * 1000,
                cancel_at_period_end: false,
            }),
            credits: BillingCredits {
                monthly: 33_250.0,
                monthly_total: 45_000.0,
                monthly_ends_at: Some(crate::now_ms() + 12 * 24 * 3600 * 1000),
                extra: 8_000.0,
                total: 41_250.0,
            },
            plans: vec![
                plan("base", "Base", 2000, 45_000.0),
                plan("pro", "Pro", 5000, 112_500.0),
                plan("max", "Max", 20000, 450_000.0),
            ],
            pack: BillingPack {
                price_cents: 1000,
                credits: 12_000.0,
            },
            has_customer: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edge_errors_read_as_actions() {
        let body = br#"{"error":"out_of_credits"}"#;
        assert_eq!(
            edge_error(402, body, "Start a Cloud session"),
            ClientError::HostError("You're out of Cloud credits.".into())
        );
        let body = br#"{"message":"Install the app on acme/api."}"#;
        assert_eq!(
            edge_error(400, body, "x"),
            ClientError::HostError("Install the app on acme/api.".into())
        );
        assert!(matches!(edge_error(401, b"", "x"), ClientError::Auth(_)));
    }

    #[test]
    fn demo_repos_filter_by_name() {
        assert_eq!(demo::repos(Some("api")).len(), 1);
        assert_eq!(demo::repos(None).len(), 5);
    }
}
