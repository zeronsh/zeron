//! Sign-ins the engine drives itself for agents that keep one login per
//! model provider (OpenCode, Pi) — the same flows those agents run in their
//! own `/login`, with the result written in their own entry format.
//!
//! - **ChatGPT** (OpenCode `openai`, Pi `openai-codex`): OpenAI's PKCE
//!   authorization-code flow with the Codex CLI's public client, redirecting
//!   to `http://localhost:1455/auth/callback` — the port is FIXED by the
//!   client registration, so this flow, `codex login` and the agents' own
//!   logins can't overlap (a new one supersedes any of ours holding it).
//!   Each login is a fresh grant: a refresh token is never shared with the
//!   Codex slots (OpenAI rotates them, so sharing one logs the other out).
//!   The callback port is reported, so a remote login tunnels it.
//! - **Claude** (Pi `anthropic`): the Claude Pro/Max sign-in pi's `/login`
//!   runs — Claude Code's public client, PKCE against our own loopback on
//!   any free port, the code redeemed at Anthropic's token endpoint — saved
//!   as pi saves it (`{type: "oauth", refresh, access, expires}`, `expires`
//!   five minutes early). A fresh grant of its own: never Claude Code's
//!   refresh token (single-use: sharing one would log the other out). Pi
//!   refreshes it under its own lock, which every zeron swap takes too.
//!   (OpenCode has no Claude sign-in: none offered.)
//! - **GitHub Copilot** (`github-copilot`, OpenCode and Pi): GitHub's
//!   device flow (scope `read:user`) — the dialog shows the code; no
//!   loopback, so a remote login needs no tunnel. Each agent signs in with
//!   its own OAuth app and keeps its own entry shape:
//!   - OpenCode (its app): the GitHub token as both `access` and `refresh`,
//!     `expires: 0`.
//!   - Pi (the Copilot editor app pi's `/login` uses — Copilot's token
//!     endpoint only serves that one): the GitHub token is `refresh`, and
//!     `access` the Copilot session it's exchanged for at
//!     `copilot_internal/v2/token`, with its `expires` — pi re-exchanges it
//!     itself when it runs out. As pi's login does, the models the account
//!     may pick but hasn't enabled yet (`policy.state: "unconfigured"`) are
//!     enabled, and the ones it can use recorded as `availableModelIds`
//!     (pi lists those). Both best effort: pi fills them on its next
//!     refresh anyway. github.com only — an Enterprise host stays with pi.

use super::stores::{Upstream, openai_detected, upstream_of};
use super::*;

/// OpenAI's OAuth issuer (`/oauth/authorize`, `/oauth/token`).
pub(super) const OPENAI_AUTH: &str = "https://auth.openai.com";
/// The Codex CLI's public client — the one OpenCode and Pi sign in with.
const OPENAI_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
/// Fixed by the client's registered redirect.
pub(super) const OPENAI_LOOPBACK_PORT: u16 = 1455;
const OPENAI_SCOPES: &str = "openid profile email offline_access";
/// OpenCode's GitHub OAuth app (its Copilot device flow).
const OPENCODE_COPILOT_CLIENT_ID: &str = "Ov23li8tweQw6odWQebz";
/// The Copilot editor app pi's `/login` signs in with (pi-ai
/// `auth/oauth/github-copilot`): Copilot's token exchange serves its tokens.
const PI_COPILOT_CLIENT_ID: &str = "Iv1.b507a08c87ecfe98";
/// The headers pi's Copilot client sends (pi-ai `COPILOT_HEADERS`).
const PI_COPILOT_HEADERS: [(&str, &str); 4] = [
    ("User-Agent", "GitHubCopilotChat/0.35.0"),
    ("Editor-Version", "vscode/1.107.0"),
    ("Editor-Plugin-Version", "copilot-chat/0.35.0"),
    ("Copilot-Integration-Id", "vscode-chat"),
];
/// Pi's Claude Pro/Max sign-in (pi-ai `auth/oauth/anthropic`): Claude
/// Code's public client, claude.ai's authorize page, the scopes pi asks
/// for. Any loopback port is accepted by the client registration.
const PI_CLAUDE_AUTHORIZE_URL: &str = "https://claude.ai/oauth/authorize";
const PI_CLAUDE_SCOPES: &str = "org:create_api_key user:profile user:inference \
     user:sessions:claude_code user:mcp_servers user:file_upload";
/// Where an individual Copilot account's API is when its token names none.
const COPILOT_INDIVIDUAL_API: &str = "https://api.individual.githubcopilot.com";
/// How long a sign-in waits on the browser before giving up.
const LOGIN_WAIT: Duration = Duration::from_secs(10 * 60);

impl AgentAccounts {
    /// ChatGPT for OpenCode / Pi: PKCE against our own loopback on 1455.
    pub(super) async fn start_openai_login(
        &self,
        harness: HarnessId,
        store_key: &'static str,
    ) -> Result<AgentLoginStart, EngineError> {
        debug_assert_eq!(upstream_of(harness, store_key), Some(Upstream::OpenAi));
        self.reap_spawned_flows(harness);
        let wanted = self.inner.endpoints.openai_port;
        self.reap_port_flows(OPENAI_LOOPBACK_PORT);
        let listener = bind_loopback(wanted).await.map_err(|err| {
            EngineError::Other(if err.kind() == std::io::ErrorKind::AddrInUse {
                format!(
                    "Port {wanted} is in use — another ChatGPT sign-in (codex login, OpenCode or \
                     Pi) is running. Finish or cancel it, then try again."
                )
            } else {
                format!("Could not open the sign-in callback: {err}")
            })
        })?;
        let port = listener
            .local_addr()
            .map_err(|e| EngineError::Other(format!("sign-in callback has no port: {e}")))?
            .port();
        let login_id = new_id();
        let (verifier, challenge) = pkce_pair();
        let state = random_url_token();
        let redirect = format!("http://localhost:{port}/auth/callback");
        let originator = match harness {
            HarnessId::Pi => "pi",
            _ => "opencode",
        };
        let url = format!(
            "{}/oauth/authorize?response_type=code&client_id={OPENAI_CLIENT_ID}\
             &redirect_uri={}&scope={}&code_challenge={challenge}\
             &code_challenge_method=S256&id_token_add_organizations=true\
             &codex_cli_simplified_flow=true&state={state}&originator={originator}",
            self.inner.endpoints.openai_auth.trim_end_matches('/'),
            urlencode(&redirect),
            urlencode(OPENAI_SCOPES),
        );
        let task_state = Arc::new(Mutex::new(TaskLoginState {
            url: Some(url.clone()),
            ..Default::default()
        }));
        let this = self.clone();
        let outcome_state = task_state.clone();
        let handle = tokio::spawn(async move {
            let outcome = tokio::time::timeout(
                LOGIN_WAIT,
                this.finish_openai_login(
                    listener, harness, store_key, &state, &verifier, &redirect,
                ),
            )
            .await
            .unwrap_or_else(|_| Err(EngineError::Other("The sign-in timed out.".into())));
            lock(&outcome_state).outcome = Some(outcome.map_err(|e| e.to_string()));
        });
        lock(&self.inner.flows).insert(
            login_id.clone(),
            LoginFlow::Task {
                harness,
                started_at: Instant::now(),
                state: task_state,
                handle,
                home: None,
                port: Some(OPENAI_LOOPBACK_PORT),
            },
        );
        Ok(AgentLoginStart {
            login_id,
            url,
            mode: AgentLoginMode::Browser,
            callback_port: Some(port),
        })
    }

    async fn finish_openai_login(
        &self,
        listener: tokio::net::TcpListener,
        harness: HarnessId,
        store_key: &str,
        state: &str,
        verifier: &str,
        redirect: &str,
    ) -> Result<(), EngineError> {
        use tokio::io::AsyncWriteExt as _;
        let (callback, mut browser) =
            await_loopback_callback(&listener, "/auth/callback", state, "ChatGPT").await;
        drop(listener);
        let result = match callback {
            Ok(code) => {
                self.redeem_openai_code(harness, store_key, &code, verifier, redirect)
                    .await
            }
            Err(message) => Err(EngineError::Other(message)),
        };
        let (status, body) = match &result {
            Ok(()) => (
                "200 OK",
                "<!doctype html><title>Signed in</title><p>Signed in to ChatGPT.</p>\
                 <p>You can close this tab and return to Zeron.</p>"
                    .to_string(),
            ),
            Err(error) => (
                "400 Bad Request",
                format!(
                    "<!doctype html><title>Sign-in failed</title><p>{}</p>\
                     <p>Return to Zeron to try again.</p>",
                    html_escape(&error.to_string())
                ),
            ),
        };
        let response = http_response(
            status,
            &[("Content-Type", "text/html; charset=utf-8")],
            &body,
        );
        let _ = browser.write_all(response.as_bytes()).await;
        let _ = browser.shutdown().await;
        result
    }

    /// Redeem the code and save the login in the agent's entry format.
    async fn redeem_openai_code(
        &self,
        harness: HarnessId,
        store_key: &str,
        code: &str,
        verifier: &str,
        redirect: &str,
    ) -> Result<(), EngineError> {
        let response = self
            .inner
            .http
            .post(format!(
                "{}/oauth/token",
                self.inner.endpoints.openai_auth.trim_end_matches('/')
            ))
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", redirect),
                ("client_id", OPENAI_CLIENT_ID),
                ("code_verifier", verifier),
            ])
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .map_err(|e| EngineError::Other(format!("token exchange failed: {e}")))?;
        if !response.status().is_success() {
            let status = response.status();
            // Never echo the body: it can carry token material on odd errors.
            return Err(EngineError::Other(format!(
                "OpenAI rejected the sign-in ({status}) — try again."
            )));
        }
        let tokens: serde_json::Value = response
            .json()
            .await
            .map_err(|e| EngineError::Other(format!("token exchange returned junk: {e}")))?;
        let (Some(access), Some(refresh)) = (
            str_field(&tokens, "access_token"),
            str_field(&tokens, "refresh_token"),
        ) else {
            return Err(EngineError::Other(
                "OpenAI returned no usable tokens — try signing in again.".into(),
            ));
        };
        let id_token = str_field(&tokens, "id_token");
        let expires_in = tokens
            .get("expires_in")
            .and_then(|v| v.as_i64())
            .unwrap_or(3600);
        let mut entry = serde_json::json!({
            "type": "oauth",
            "access": access,
            "refresh": refresh,
            "expires": now_ms() + expires_in * 1000,
        });
        let account_id = [id_token.as_deref(), Some(access.as_str())]
            .into_iter()
            .flatten()
            .filter_map(jwt_claims)
            .find_map(|claims| {
                claims
                    .get("https://api.openai.com/auth")
                    .and_then(|auth| str_field(auth, "chatgpt_account_id"))
            });
        if let (Some(account_id), Some(map)) = (account_id, entry.as_object_mut()) {
            map.insert("accountId".into(), serde_json::json!(account_id));
        }
        let detected =
            openai_detected(store_key, &entry, id_token.as_deref()).ok_or_else(|| {
                EngineError::Other("Could not identify the signed-in ChatGPT account.".into())
            })?;
        self.save_new_login(harness, &detected).await
    }

    /// Claude for Pi: pi's own Claude Pro/Max sign-in, on our loopback.
    pub(super) async fn start_pi_claude_login(&self) -> Result<AgentLoginStart, EngineError> {
        self.reap_spawned_flows(HarnessId::Pi);
        let listener = bind_loopback(0).await.map_err(|err| {
            EngineError::Other(format!("Could not open the sign-in callback: {err}"))
        })?;
        let port = listener
            .local_addr()
            .map_err(|e| EngineError::Other(format!("sign-in callback has no port: {e}")))?
            .port();
        let login_id = new_id();
        let (verifier, challenge) = pkce_pair();
        let state = random_url_token();
        let redirect = format!("http://localhost:{port}/callback");
        let url = format!(
            "{PI_CLAUDE_AUTHORIZE_URL}?code=true&client_id={CLAUDE_CLIENT_ID}\
             &response_type=code&redirect_uri={}&scope={}&code_challenge={challenge}\
             &code_challenge_method=S256&state={state}",
            urlencode(&redirect),
            urlencode(PI_CLAUDE_SCOPES),
        );
        let task_state = Arc::new(Mutex::new(TaskLoginState {
            url: Some(url.clone()),
            ..Default::default()
        }));
        let this = self.clone();
        let outcome_state = task_state.clone();
        let handle = tokio::spawn(async move {
            let outcome = tokio::time::timeout(
                LOGIN_WAIT,
                this.finish_pi_claude_login(listener, &state, &verifier, &redirect),
            )
            .await
            .unwrap_or_else(|_| Err(EngineError::Other("The sign-in timed out.".into())));
            lock(&outcome_state).outcome = Some(outcome.map_err(|e| e.to_string()));
        });
        lock(&self.inner.flows).insert(
            login_id.clone(),
            LoginFlow::Task {
                harness: HarnessId::Pi,
                started_at: Instant::now(),
                state: task_state,
                handle,
                home: None,
                port: Some(port),
            },
        );
        Ok(AgentLoginStart {
            login_id,
            url,
            mode: AgentLoginMode::Browser,
            callback_port: Some(port),
        })
    }

    async fn finish_pi_claude_login(
        &self,
        listener: tokio::net::TcpListener,
        state: &str,
        verifier: &str,
        redirect: &str,
    ) -> Result<(), EngineError> {
        use tokio::io::AsyncWriteExt as _;
        let (callback, mut browser) = await_claude_callback(&listener, state).await;
        drop(listener);
        let result = match callback {
            Ok(code) => {
                self.redeem_pi_claude_code(&code, state, verifier, redirect)
                    .await
            }
            Err(message) => Err(EngineError::Other(message)),
        };
        let (status, body) = match &result {
            Ok(()) => (
                "200 OK",
                "<!doctype html><title>Signed in</title><p>Signed in to Claude for Pi.</p>\
                 <p>You can close this tab and return to Zeron.</p>"
                    .to_string(),
            ),
            Err(error) => (
                "400 Bad Request",
                format!(
                    "<!doctype html><title>Sign-in failed</title><p>{}</p>\
                     <p>Return to Zeron to try again.</p>",
                    html_escape(&error.to_string())
                ),
            ),
        };
        let response = http_response(
            status,
            &[("Content-Type", "text/html; charset=utf-8")],
            &body,
        );
        let _ = browser.write_all(response.as_bytes()).await;
        let _ = browser.shutdown().await;
        result
    }

    /// Redeem the code as pi does and save the login in pi's entry format.
    async fn redeem_pi_claude_code(
        &self,
        code: &str,
        state: &str,
        verifier: &str,
        redirect: &str,
    ) -> Result<(), EngineError> {
        let response = self
            .inner
            .http
            .post(&self.inner.endpoints.claude_loopback_token)
            .json(&serde_json::json!({
                "grant_type": "authorization_code",
                "client_id": CLAUDE_CLIENT_ID,
                "code": code,
                "state": state,
                "redirect_uri": redirect,
                "code_verifier": verifier,
            }))
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .map_err(|e| EngineError::Other(format!("token exchange failed: {e}")))?;
        if !response.status().is_success() {
            let status = response.status();
            // Never echo the body: it can carry token material on odd errors.
            return Err(EngineError::Other(format!(
                "Anthropic rejected the sign-in ({status}) — try again."
            )));
        }
        let tokens: serde_json::Value = response
            .json()
            .await
            .map_err(|e| EngineError::Other(format!("token exchange returned junk: {e}")))?;
        let (Some(access), Some(refresh)) = (
            str_field(&tokens, "access_token"),
            str_field(&tokens, "refresh_token"),
        ) else {
            return Err(EngineError::Other(
                "Anthropic returned no usable tokens — try signing in again.".into(),
            ));
        };
        let expires_in = tokens
            .get("expires_in")
            .and_then(|v| v.as_i64())
            .unwrap_or(3600);
        let entry = serde_json::json!({
            "type": "oauth",
            "refresh": refresh,
            "access": access,
            "expires": now_ms() + expires_in * 1000 - 5 * 60 * 1000,
        });
        // The profile names the plan; the token response already names the
        // account (as Claude Code's own sign-in reads it), so a profile call
        // that fails (rate limited, say) still signs in, plan unnamed.
        let (account_key, profile) = match self.anthropic_identity("anthropic", &access).await {
            Some(identity) => identity,
            None => token_identity(&tokens).ok_or_else(|| {
                EngineError::Other("Could not identify the signed-in Claude account.".into())
            })?,
        };
        let detected = Detected::known(account_key, profile, entry).keyed("anthropic");
        self.save_new_login(HarnessId::Pi, &detected).await
    }

    /// GitHub Copilot for OpenCode / Pi: GitHub's device flow, with the
    /// agent's own OAuth app. The start asks GitHub for the code (the dialog
    /// shows it), then a task polls until the user approves.
    pub(super) async fn start_copilot_login(
        &self,
        harness: HarnessId,
    ) -> Result<AgentLoginStart, EngineError> {
        self.reap_spawned_flows(harness);
        let login = self
            .inner
            .endpoints
            .github_login
            .trim_end_matches('/')
            .to_string();
        let device: serde_json::Value = self
            .inner
            .http
            .post(format!("{login}/login/device/code"))
            .header("Accept", "application/json")
            .header("User-Agent", copilot_user_agent(harness))
            .form(&[
                ("client_id", copilot_client_id(harness)),
                ("scope", "read:user"),
            ])
            .send()
            .await
            .map_err(|e| EngineError::Other(format!("Couldn't reach GitHub: {e}")))?
            .error_for_status()
            .map_err(|e| EngineError::Other(format!("GitHub refused the sign-in: {e}")))?
            .json()
            .await
            .map_err(|e| EngineError::Other(format!("GitHub returned junk: {e}")))?;
        let (Some(device_code), Some(user_code), Some(verification)) = (
            str_field(&device, "device_code"),
            str_field(&device, "user_code"),
            str_field(&device, "verification_uri"),
        ) else {
            return Err(EngineError::Other(
                "GitHub didn't return a sign-in code — try again.".into(),
            ));
        };
        let interval = device.get("interval").and_then(|v| v.as_u64()).unwrap_or(5);
        let login_id = new_id();
        let state = Arc::new(Mutex::new(TaskLoginState {
            url: Some(verification.clone()),
            message: Some(format!("Enter the code {user_code} on GitHub.")),
            ..Default::default()
        }));
        let this = self.clone();
        let task_state = state.clone();
        let handle = tokio::spawn(async move {
            let outcome = tokio::time::timeout(
                LOGIN_WAIT,
                this.finish_copilot_login(harness, &login, &device_code, interval),
            )
            .await
            .unwrap_or_else(|_| Err(EngineError::Other("The sign-in timed out.".into())));
            lock(&task_state).outcome = Some(outcome.map_err(|e| e.to_string()));
        });
        lock(&self.inner.flows).insert(
            login_id.clone(),
            LoginFlow::Task {
                harness,
                started_at: Instant::now(),
                state,
                handle,
                home: None,
                port: None,
            },
        );
        Ok(AgentLoginStart {
            login_id,
            url: verification,
            mode: AgentLoginMode::Browser,
            callback_port: None,
        })
    }

    async fn finish_copilot_login(
        &self,
        harness: HarnessId,
        login: &str,
        device_code: &str,
        interval: u64,
    ) -> Result<(), EngineError> {
        let mut interval = interval.max(1);
        let token = loop {
            tokio::time::sleep(Duration::from_secs(interval)).await;
            let reply: serde_json::Value = match self
                .inner
                .http
                .post(format!("{login}/login/oauth/access_token"))
                .header("Accept", "application/json")
                .header("User-Agent", copilot_user_agent(harness))
                .form(&[
                    ("client_id", copilot_client_id(harness)),
                    ("device_code", device_code),
                    ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ])
                .send()
                .await
            {
                Ok(response) => match response.json().await {
                    Ok(reply) => reply,
                    Err(_) => continue,
                },
                // A blip while the user types the code: keep polling.
                Err(_) => continue,
            };
            if let Some(token) = str_field(&reply, "access_token") {
                break token;
            }
            match str_field(&reply, "error").as_deref() {
                Some("authorization_pending") | None => {}
                Some("slow_down") => interval += 5,
                Some("expired_token") => {
                    return Err(EngineError::Other(
                        "The GitHub code expired — start again.".into(),
                    ));
                }
                Some("access_denied") => {
                    return Err(EngineError::Other(
                        "The sign-in was declined on GitHub.".into(),
                    ));
                }
                Some(other) => {
                    return Err(EngineError::Other(format!(
                        "GitHub sign-in failed: {other}"
                    )));
                }
            }
        };
        let entry = match harness {
            HarnessId::Pi => self.pi_copilot_entry(&token).await?,
            _ => serde_json::json!({
                "type": "oauth",
                "refresh": token,
                "access": token,
                "expires": 0,
            }),
        };
        let (account_key, profile) = self
            .github_identity("github-copilot", &token, &entry)
            .await
            .ok_or_else(|| {
                EngineError::Other("Could not identify the signed-in GitHub account.".into())
            })?;
        let detected = Detected::known(account_key, profile, entry).keyed("github-copilot");
        self.save_new_login(harness, &detected).await
    }

    /// Pi's Copilot entry for a fresh GitHub token, as pi's `/login` writes
    /// it: the token exchanged for a Copilot session (refused: the account
    /// has no Copilot, said so), then the models it may use.
    async fn pi_copilot_entry(&self, github_token: &str) -> Result<serde_json::Value, EngineError> {
        let api = self.inner.endpoints.github_api.trim_end_matches('/');
        let mut request = self
            .inner
            .http
            .get(format!("{api}/copilot_internal/v2/token"))
            .header("Accept", "application/json")
            .bearer_auth(github_token)
            .timeout(Duration::from_secs(15));
        for (name, value) in PI_COPILOT_HEADERS {
            request = request.header(name, value);
        }
        let response = request
            .send()
            .await
            .map_err(|e| EngineError::Other(format!("Couldn't reach GitHub Copilot: {e}")))?;
        let status = response.status();
        if matches!(status.as_u16(), 401 | 403 | 404) {
            return Err(EngineError::Other(
                "That GitHub account has no GitHub Copilot — sign in with one that does.".into(),
            ));
        }
        if !status.is_success() {
            // Never echo the body: it may carry token material.
            return Err(EngineError::Other(format!(
                "GitHub Copilot refused the sign-in ({status}) — try again."
            )));
        }
        let session: serde_json::Value = response
            .json()
            .await
            .map_err(|e| EngineError::Other(format!("GitHub Copilot returned junk: {e}")))?;
        let (Some(access), Some(expires_at)) = (
            str_field(&session, "token"),
            session.get("expires_at").and_then(|v| v.as_i64()),
        ) else {
            return Err(EngineError::Other(
                "GitHub Copilot returned no session — try signing in again.".into(),
            ));
        };
        let mut entry = serde_json::json!({
            "type": "oauth",
            "refresh": github_token,
            "access": access,
            // pi re-exchanges five minutes early, as its own login sets it.
            "expires": expires_at * 1000 - 5 * 60 * 1000,
        });
        if let Some(models) = self.pi_copilot_models(&access).await
            && let Some(map) = entry.as_object_mut()
        {
            map.insert("availableModelIds".into(), serde_json::json!(models));
        }
        Ok(entry)
    }

    /// The Copilot models a fresh Pi login may use (pi-ai
    /// `parseGitHubCopilotModelCatalog` + `enableGitHubCopilotModels`):
    /// those in the account's picker and not disabled by policy, plus the
    /// ones it may pick but never enabled, enabled now. `None` when the
    /// catalog can't be read (pi lists every model until it refreshes).
    async fn pi_copilot_models(&self, copilot_token: &str) -> Option<Vec<String>> {
        let base = match &self.inner.endpoints.copilot_api {
            Some(base) => base.trim_end_matches('/').to_string(),
            None => copilot_api_from_token(copilot_token)?,
        };
        let headers = |mut request: reqwest::RequestBuilder| {
            request = request
                .header("Accept", "application/json")
                .bearer_auth(copilot_token)
                .timeout(Duration::from_secs(10));
            for (name, value) in PI_COPILOT_HEADERS {
                request = request.header(name, value);
            }
            request
        };
        let catalog: serde_json::Value = headers(self.inner.http.get(format!("{base}/models")))
            .header("X-GitHub-Api-Version", "2026-06-01")
            .send()
            .await
            .ok()
            .filter(|res| res.status().is_success())?
            .json()
            .await
            .ok()?;
        let (mut available, to_enable) =
            copilot_model_choices(&catalog, base == COPILOT_INDIVIDUAL_API)?;
        for id in to_enable.into_iter().take(32) {
            let enabled = headers(
                self.inner
                    .http
                    .post(format!("{base}/models/{}/policy", urlencode(&id))),
            )
            .header("openai-intent", "chat-policy")
            .header("x-interaction-type", "chat-policy")
            .json(&serde_json::json!({ "state": "enabled" }))
            .send()
            .await;
            match enabled {
                Ok(res) if res.status().is_success() => {
                    if !available.contains(&id) {
                        available.push(id);
                    }
                }
                // Rate limited: the rest stay as they are (pi stops too).
                Ok(res) if res.status().as_u16() == 429 => break,
                _ => {}
            }
        }
        Some(available)
    }
}

/// The account a Claude token response names (`account.uuid`,
/// `account.email_address`, `organization.name`), keyed as
/// `anthropic_identity` keys it: the identity when the profile call fails.
/// The plan isn't in it.
fn token_identity(tokens: &serde_json::Value) -> Option<(String, SlotProfile)> {
    let account = tokens.get("account")?;
    let email = str_field(account, "email_address")?;
    let uuid = str_field(account, "uuid").unwrap_or_else(|| email.clone());
    let organization = tokens
        .get("organization")
        .and_then(|org| str_field(org, "name"));
    Some((
        format!("anthropic:{uuid}"),
        SlotProfile {
            email,
            display_name: None,
            organization,
            plan: Some("Claude".to_string()),
            auth_kind: AgentAuthKind::Oauth,
        },
    ))
}

fn copilot_client_id(harness: HarnessId) -> &'static str {
    match harness {
        HarnessId::Pi => PI_COPILOT_CLIENT_ID,
        _ => OPENCODE_COPILOT_CLIENT_ID,
    }
}

fn copilot_user_agent(harness: HarnessId) -> &'static str {
    match harness {
        HarnessId::Pi => PI_COPILOT_HEADERS[0].1,
        _ => "zeron",
    }
}

/// A Copilot session's API root, from its token's `proxy-ep` (`proxy.X` →
/// `https://api.X`) — only on GitHub Copilot's own hosts, or the individual
/// default when the token names none.
pub(super) fn copilot_api_from_token(token: &str) -> Option<String> {
    let Some(proxy) = token
        .split(';')
        .find_map(|part| part.trim().strip_prefix("proxy-ep="))
    else {
        return Some(COPILOT_INDIVIDUAL_API.to_string());
    };
    let host = proxy.trim().trim_start_matches("proxy.");
    let host = format!("api.{host}").to_ascii_lowercase();
    let valid = host
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
    (valid && super::stores::host_in(&host, &["githubcopilot.com"]))
        .then(|| format!("https://{host}"))
}

/// From a Copilot `/models` reply: the model ids the account can use now,
/// and those it may pick but must enable first. `individual` admits pi's
/// fallback for individual accounts whose picker flags are all off.
pub(super) fn copilot_model_choices(
    catalog: &serde_json::Value,
    individual: bool,
) -> Option<(Vec<String>, Vec<String>)> {
    struct Listed {
        id: String,
        picker: bool,
        policy: Option<String>,
    }
    let listed: Vec<Listed> = catalog
        .get("data")?
        .as_array()?
        .iter()
        .filter_map(|item| {
            let id = str_field(item, "id")?;
            let tools = item
                .pointer("/capabilities/supports/tool_calls")
                .and_then(|v| v.as_bool());
            (tools != Some(false)).then(|| Listed {
                id,
                picker: item.get("model_picker_enabled").and_then(|v| v.as_bool()) == Some(true),
                policy: item
                    .pointer("/policy/state")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
            })
        })
        .collect();
    let in_picker: Vec<String> = listed
        .iter()
        .filter(|m| m.picker && m.policy.as_deref() != Some("disabled"))
        .map(|m| m.id.clone())
        .collect();
    let fallback = individual && in_picker.is_empty();
    let available = if fallback {
        listed
            .iter()
            .filter(|m| m.policy.as_deref() == Some("enabled"))
            .map(|m| m.id.clone())
            .collect()
    } else {
        in_picker
    };
    let to_enable = listed
        .iter()
        .filter(|m| m.policy.as_deref() == Some("unconfigured") && (m.picker || fallback))
        .map(|m| m.id.clone())
        .collect();
    Some((available, to_enable))
}

/// A loopback listener on `port` (0 = any): `localhost` resolves to either
/// family, so bind IPv4 and fall back to IPv6.
async fn bind_loopback(port: u16) -> std::io::Result<tokio::net::TcpListener> {
    match tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await {
        Ok(listener) => Ok(listener),
        Err(err) if err.kind() == std::io::ErrorKind::AddrInUse => Err(err),
        Err(_) => tokio::net::TcpListener::bind((std::net::Ipv6Addr::LOCALHOST, port)).await,
    }
}
