//! Agent sign-ins from a device with no agent CLIs (the phone), straight
//! into Cloud's vault.
//!
//! The same flows the desktop engine drives itself
//! (`crates/engine/src/agent_accounts.rs`, `agent_accounts/oauth.rs`):
//!
//! - **Claude**: Claude Code's automatic login — PKCE against claude.com with
//!   a loopback redirect on a free port, redeemed at platform.claude.com, the
//!   account read from Anthropic's profile endpoint.
//! - **ChatGPT** (Codex): OpenAI's PKCE flow with the Codex CLI's public
//!   client, whose registered redirect is fixed at `localhost:1455`; the
//!   result is written in Codex's own `auth.json` shape.
//!
//! The browser lands on our loopback listener; we answer it with a redirect
//! to the platform's `done_url` (a custom scheme its auth sheet waits for, so
//! the sheet closes by itself), then redeem the code and store the login in
//! the vault for the logical Cloud device — the material the desktop uploads
//! after the same sign-in. No token stays on the phone.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD as BASE64, URL_SAFE_NO_PAD as BASE64_URL};
use sha2::{Digest as _, Sha256};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use zeron_proto::{VaultAccountProfile, VaultProvider};

use crate::Client;
use crate::error::{ClientError, Result};

/// Claude Code's public OAuth client (PKCE carries the proof; no secret).
const CLAUDE_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const CLAUDE_AUTHORIZE_URL: &str = "https://claude.com/cai/oauth/authorize";
const CLAUDE_TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
const CLAUDE_PROFILE_URL: &str = "https://api.anthropic.com/api/oauth/profile";
const CLAUDE_SCOPES: &str = "org:create_api_key user:profile user:inference \
     user:sessions:claude_code user:mcp_servers user:file_upload user:plugins";

/// The Codex CLI's public OpenAI client, and its fixed redirect port.
const OPENAI_AUTH: &str = "https://auth.openai.com";
const OPENAI_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const OPENAI_PORT: u16 = 1455;
const OPENAI_SCOPES: &str = "openid profile email offline_access";

/// How long a sign-in waits on the browser.
const LOGIN_WAIT: Duration = Duration::from_secs(10 * 60);

/// A subscription whose login Cloud keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginProvider {
    Claude,
    Codex,
}

impl LoginProvider {
    fn name(self) -> &'static str {
        match self {
            LoginProvider::Claude => "Claude",
            LoginProvider::Codex => "ChatGPT",
        }
    }
}

/// A sign-in under way: open `url` in an auth sheet waiting for the scheme
/// of the `done_url` it was started with, then await
/// [`Client::provider_login_finish`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderLoginStart {
    pub login_id: String,
    pub url: String,
}

struct Flow {
    result: Option<oneshot::Receiver<Result<()>>>,
    task: tokio::task::JoinHandle<()>,
}

fn flows() -> &'static Mutex<HashMap<String, Flow>> {
    static FLOWS: OnceLock<Mutex<HashMap<String, Flow>>> = OnceLock::new();
    FLOWS.get_or_init(Default::default)
}

impl Client {
    /// Start a sign-in. `done_url` is where the browser goes once the
    /// provider hands back its code (`zeron://signed-in` on iOS).
    pub async fn provider_login_start(
        &self,
        provider: LoginProvider,
        done_url: &str,
    ) -> Result<ProviderLoginStart> {
        if self.inner.is_demo() {
            return Err(ClientError::Unsupported(
                "Signing in isn't available in the demo.".into(),
            ));
        }
        // Fail before any browser opens: the login has nowhere to go.
        self.cloud_device_id().await?;
        let port = match provider {
            LoginProvider::Claude => 0,
            LoginProvider::Codex => {
                // One ChatGPT sign-in at a time holds 1455.
                cancel_all();
                OPENAI_PORT
            }
        };
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))
            .await
            .map_err(|e| {
                ClientError::Internal(if e.kind() == std::io::ErrorKind::AddrInUse {
                    "Another ChatGPT sign-in is still open — finish or cancel it first.".into()
                } else {
                    format!("Couldn't open the sign-in callback: {e}")
                })
            })?;
        let port = listener
            .local_addr()
            .map_err(|e| ClientError::Internal(format!("sign-in callback has no port: {e}")))?
            .port();
        let (verifier, challenge) = pkce_pair();
        let state = random_url_token();
        let (url, redirect, path) = match provider {
            LoginProvider::Claude => {
                let redirect = format!("http://localhost:{port}/callback");
                let url = format!(
                    "{CLAUDE_AUTHORIZE_URL}?code=true&client_id={CLAUDE_CLIENT_ID}\
                     &response_type=code&redirect_uri={}&scope={}&code_challenge={challenge}\
                     &code_challenge_method=S256&state={state}",
                    urlencode(&redirect),
                    urlencode(CLAUDE_SCOPES),
                );
                (url, redirect, "/callback")
            }
            LoginProvider::Codex => {
                let redirect = format!("http://localhost:{port}/auth/callback");
                let url = format!(
                    "{OPENAI_AUTH}/oauth/authorize?response_type=code&client_id={OPENAI_CLIENT_ID}\
                     &redirect_uri={}&scope={}&code_challenge={challenge}\
                     &code_challenge_method=S256&id_token_add_organizations=true\
                     &codex_cli_simplified_flow=true&state={state}&originator=codex_cli_rs",
                    urlencode(&redirect),
                    urlencode(OPENAI_SCOPES),
                );
                (url, redirect, "/auth/callback")
            }
        };
        let login_id = crate::new_id();
        let (tx, rx) = oneshot::channel();
        let client = self.clone();
        let done_url = done_url.to_owned();
        let task = crate::runtime::shared().spawn(async move {
            let outcome = tokio::time::timeout(LOGIN_WAIT, async {
                let (code, mut browser) = await_callback(&listener, path, &state, provider).await;
                drop(listener);
                // Back to the app at once; the redeem finishes behind the
                // "Signing in…" it shows.
                let _ = browser
                    .write_all(
                        http_response("302 Found", &[("Location", &done_url)], "").as_bytes(),
                    )
                    .await;
                let _ = browser.shutdown().await;
                let code = code.map_err(ClientError::HostError)?;
                match provider {
                    LoginProvider::Claude => {
                        client
                            .redeem_claude(&code, &state, &verifier, &redirect)
                            .await
                    }
                    LoginProvider::Codex => client.redeem_codex(&code, &verifier, &redirect).await,
                }
            })
            .await
            .unwrap_or_else(|_| Err(ClientError::HostError("The sign-in timed out.".into())));
            let _ = tx.send(outcome);
        });
        crate::lock(flows()).insert(
            login_id.clone(),
            Flow {
                result: Some(rx),
                task,
            },
        );
        Ok(ProviderLoginStart { login_id, url })
    }

    /// Wait for a started sign-in to land in the vault.
    pub async fn provider_login_finish(&self, login_id: &str) -> Result<()> {
        let receiver = crate::lock(flows())
            .get_mut(login_id)
            .and_then(|flow| flow.result.take())
            .ok_or_else(|| ClientError::NotFound("That sign-in expired — start again.".into()))?;
        let result = receiver
            .await
            .unwrap_or_else(|_| Err(ClientError::HostError("The sign-in was cancelled.".into())));
        crate::lock(flows()).remove(login_id);
        result
    }

    /// Give up on a sign-in (its sheet was dismissed).
    pub fn provider_login_cancel(&self, login_id: &str) {
        if let Some(flow) = crate::lock(flows()).remove(login_id) {
            flow.task.abort();
        }
    }

    /// Redeem Claude's code and store the login (Claude Code's credential
    /// blob, `claudeAiOauth`) with the account it belongs to.
    async fn redeem_claude(
        &self,
        code: &str,
        state: &str,
        verifier: &str,
        redirect: &str,
    ) -> Result<()> {
        let http = crate::auth::http();
        let response = http
            .post(CLAUDE_TOKEN_URL)
            .json(&serde_json::json!({
                "grant_type": "authorization_code",
                "code": code,
                "state": state,
                "client_id": CLAUDE_CLIENT_ID,
                "redirect_uri": redirect,
                "code_verifier": verifier,
            }))
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .map_err(|e| ClientError::Network(format!("Couldn't reach Anthropic: {e}")))?;
        if !response.status().is_success() {
            // Never echo the body: odd errors can carry token material.
            return Err(ClientError::HostError(format!(
                "Anthropic rejected the sign-in ({}) — try again.",
                response.status()
            )));
        }
        let token: serde_json::Value = response.json().await.map_err(|e| {
            ClientError::HostError(format!("Anthropic's reply was unreadable: {e}"))
        })?;
        let (Some(access), Some(refresh)) = (
            str_field(&token, "access_token"),
            str_field(&token, "refresh_token"),
        ) else {
            return Err(ClientError::HostError(
                "Anthropic returned no usable sign-in — try again.".into(),
            ));
        };
        let expires_in = token
            .get("expires_in")
            .and_then(|v| v.as_i64())
            .unwrap_or(3600);
        let profile: Option<serde_json::Value> = match http
            .get(CLAUDE_PROFILE_URL)
            .bearer_auth(&access)
            .header("anthropic-beta", "oauth-2025-04-20")
            .timeout(Duration::from_secs(15))
            .send()
            .await
        {
            Ok(res) if res.status().is_success() => res.json().await.ok(),
            _ => None,
        };
        let empty = serde_json::json!({});
        let p_account = profile
            .as_ref()
            .and_then(|p| p.get("account"))
            .unwrap_or(&empty);
        let p_org = profile
            .as_ref()
            .and_then(|p| p.get("organization"))
            .unwrap_or(&empty);
        let t_account = token.get("account").unwrap_or(&empty);
        let t_org = token.get("organization").unwrap_or(&empty);
        let email = str_field(p_account, "email_address")
            .or_else(|| str_field(t_account, "email_address"))
            .ok_or_else(|| {
                ClientError::HostError("Couldn't tell which account signed in.".into())
            })?;
        let org_type = str_field(p_org, "organization_type");
        let rate_tier = str_field(p_org, "rate_limit_tier");
        let scopes: Vec<String> = str_field(&token, "scope")
            .unwrap_or_else(|| CLAUDE_SCOPES.to_owned())
            .split(' ')
            .map(str::to_owned)
            .collect();
        let mut oauth = serde_json::json!({
            "accessToken": access,
            "refreshToken": refresh,
            "expiresAt": crate::now_ms() + expires_in * 1000,
            "scopes": scopes,
        });
        let subscription = match org_type.as_deref() {
            Some("claude_max") => Some("max"),
            Some("claude_pro") => Some("pro"),
            Some("claude_team") => Some("team"),
            Some("claude_enterprise") => Some("enterprise"),
            _ => None,
        };
        if let (Some(sub), Some(map)) = (subscription, oauth.as_object_mut()) {
            map.insert("subscriptionType".into(), serde_json::json!(sub));
        }
        let profile = VaultAccountProfile {
            email: Some(email),
            display_name: str_field(p_account, "display_name")
                .or_else(|| str_field(p_account, "full_name")),
            organization: str_field(p_org, "name").or_else(|| str_field(t_org, "name")),
            plan: claude_plan(org_type.as_deref(), rate_tier.as_deref()),
        };
        self.put_credential(
            VaultProvider::Claude,
            serde_json::json!({ "claudeAiOauth": oauth }),
            Some(profile),
        )
        .await
    }

    /// Redeem ChatGPT's code and store it as Codex's `auth.json` (the id
    /// token names the account).
    async fn redeem_codex(&self, code: &str, verifier: &str, redirect: &str) -> Result<()> {
        let response = crate::auth::http()
            .post(format!("{OPENAI_AUTH}/oauth/token"))
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
            .map_err(|e| ClientError::Network(format!("Couldn't reach OpenAI: {e}")))?;
        if !response.status().is_success() {
            return Err(ClientError::HostError(format!(
                "OpenAI rejected the sign-in ({}) — try again.",
                response.status()
            )));
        }
        let tokens: serde_json::Value = response
            .json()
            .await
            .map_err(|e| ClientError::HostError(format!("OpenAI's reply was unreadable: {e}")))?;
        let auth = codex_auth_json(&tokens, &chrono::Utc::now().to_rfc3339())?;
        self.put_credential(
            VaultProvider::Codex,
            serde_json::json!({ "authJson": auth }),
            None,
        )
        .await
    }
}

/// Codex's `auth.json` from a ChatGPT token reply.
fn codex_auth_json(tokens: &serde_json::Value, now: &str) -> Result<serde_json::Value> {
    let (Some(access), Some(refresh), Some(id_token)) = (
        str_field(tokens, "access_token"),
        str_field(tokens, "refresh_token"),
        str_field(tokens, "id_token"),
    ) else {
        return Err(ClientError::HostError(
            "OpenAI returned no usable sign-in — try again.".into(),
        ));
    };
    let account_id = [Some(id_token.as_str()), Some(access.as_str())]
        .into_iter()
        .flatten()
        .filter_map(jwt_claims)
        .find_map(|claims| {
            claims
                .get("https://api.openai.com/auth")
                .and_then(|auth| str_field(auth, "chatgpt_account_id"))
        });
    Ok(serde_json::json!({
        "OPENAI_API_KEY": null,
        "tokens": {
            "id_token": id_token,
            "access_token": access,
            "refresh_token": refresh,
            "account_id": account_id,
        },
        "last_refresh": now,
    }))
}

/// Serve `path` until a request carries our `state`.
async fn await_callback(
    listener: &TcpListener,
    path: &str,
    state: &str,
    provider: LoginProvider,
) -> (std::result::Result<String, String>, TcpStream) {
    loop {
        let Ok((mut socket, _)) = listener.accept().await else {
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue;
        };
        let Some(target) = read_request_target(&mut socket).await else {
            continue;
        };
        let url = reqwest::Url::parse(&format!("http://localhost{target}")).ok();
        let Some(url) = url.filter(|url| url.path() == path) else {
            let _ = socket
                .write_all(http_response("404 Not Found", &[], "").as_bytes())
                .await;
            continue;
        };
        let param = |name: &str| {
            url.query_pairs()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.into_owned())
        };
        if param("state").as_deref() != Some(state) {
            let _ = socket
                .write_all(http_response("400 Bad Request", &[], "Unknown sign-in.").as_bytes())
                .await;
            continue;
        }
        if let Some(code) = param("code").filter(|code| !code.is_empty()) {
            return (Ok(code), socket);
        }
        let reason = param("error_description")
            .or_else(|| param("error"))
            .unwrap_or_else(|| "no authorization code came back".into());
        return (
            Err(format!("{} sign-in failed: {reason}", provider.name())),
            socket,
        );
    }
}

/// The request target of a browser's `GET` (headers read and discarded).
pub(crate) async fn read_request_target(socket: &mut TcpStream) -> Option<String> {
    let mut head = Vec::new();
    let mut chunk = [0u8; 2048];
    let read = async {
        while !head.windows(4).any(|w| w == b"\r\n\r\n") && head.len() < 16 * 1024 {
            let n = socket.read(&mut chunk).await.ok()?;
            if n == 0 {
                break;
            }
            head.extend_from_slice(&chunk[..n]);
        }
        Some(())
    };
    tokio::time::timeout(Duration::from_secs(10), read)
        .await
        .ok()??;
    let line = String::from_utf8_lossy(&head);
    let mut parts = line.lines().next()?.split_whitespace();
    (parts.next()? == "GET").then_some(())?;
    parts
        .next()
        .filter(|target| target.starts_with('/'))
        .map(str::to_string)
}

pub(crate) fn http_response(status: &str, headers: &[(&str, &str)], body: &str) -> String {
    let mut response = format!("HTTP/1.1 {status}\r\n");
    for (name, value) in headers {
        response.push_str(&format!("{name}: {value}\r\n"));
    }
    response.push_str(&format!(
        "Content-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n{body}",
        body.len()
    ));
    response
}

/// Every sign-in still waiting (a new ChatGPT one needs port 1455 back).
fn cancel_all() {
    for (_, flow) in crate::lock(flows()).drain() {
        flow.task.abort();
    }
}

fn str_field(value: &serde_json::Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn jwt_claims(jwt: &str) -> Option<serde_json::Value> {
    let payload = jwt.split('.').nth(1)?;
    let bytes = BASE64_URL
        .decode(payload)
        .or_else(|_| BASE64.decode(payload))
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// 32 random bytes, URL-safe (Anthropic rejects a shorter `state`).
fn random_url_token() -> String {
    let raw: Vec<u8> = uuid::Uuid::new_v4()
        .as_bytes()
        .iter()
        .chain(uuid::Uuid::new_v4().as_bytes())
        .copied()
        .collect();
    BASE64_URL.encode(&raw)
}

/// PKCE: a verifier and its S256 challenge.
fn pkce_pair() -> (String, String) {
    let verifier = random_url_token();
    let challenge = BASE64_URL.encode(Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

/// Percent-encoding for OAuth query values (`encodeURIComponent`).
fn urlencode(input: &str) -> String {
    let mut out = String::with_capacity(input.len() * 3);
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// "Max 20×" from Claude's organization type and rate-limit tier.
fn claude_plan(org_type: Option<&str>, tier: Option<&str>) -> Option<String> {
    let base = match org_type {
        Some("claude_max") => "Max",
        Some("claude_pro") => "Pro",
        Some("claude_team") => "Team",
        Some("claude_enterprise") => "Enterprise",
        _ => return None,
    };
    let mult = tier.and_then(|t| {
        let stem = t.strip_suffix('x')?;
        let digits: String = stem
            .chars()
            .rev()
            .take_while(char::is_ascii_digit)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        let preceded = stem.len() > digits.len()
            && stem.as_bytes().get(stem.len() - digits.len() - 1) == Some(&b'_');
        (!digits.is_empty() && preceded).then_some(digits)
    });
    Some(match mult {
        Some(mult) => format!("{base} {mult}×"),
        None => base.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_auth_json_names_the_chatgpt_account() {
        let claims = BASE64_URL.encode(
            serde_json::json!({
                "https://api.openai.com/auth": { "chatgpt_account_id": "acct-1" }
            })
            .to_string(),
        );
        let id_token = format!("h.{claims}.s");
        let auth = codex_auth_json(
            &serde_json::json!({
                "access_token": "at", "refresh_token": "rt", "id_token": id_token
            }),
            "2026-10-19T00:00:00Z",
        )
        .unwrap();
        assert_eq!(auth["tokens"]["account_id"], "acct-1");
        assert_eq!(auth["tokens"]["refresh_token"], "rt");
        assert_eq!(auth["last_refresh"], "2026-10-19T00:00:00Z");
        assert!(codex_auth_json(&serde_json::json!({ "access_token": "at" }), "").is_err());
    }

    #[test]
    fn plans_and_pkce() {
        assert_eq!(
            claude_plan(Some("claude_max"), Some("default_claude_max_20x")).as_deref(),
            Some("Max 20×")
        );
        assert_eq!(
            claude_plan(Some("claude_pro"), None).as_deref(),
            Some("Pro")
        );
        let (verifier, challenge) = pkce_pair();
        assert_eq!(
            challenge,
            BASE64_URL.encode(Sha256::digest(verifier.as_bytes()))
        );
        assert_eq!(urlencode("a b/c"), "a%20b%2Fc");
    }

    #[tokio::test]
    async fn the_callback_hands_back_the_code_for_our_state_only() {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let browser = tokio::spawn(async move {
            for target in [
                "/favicon.ico",
                "/callback?code=x&state=other",
                "/callback?code=c0de&state=ours",
            ] {
                let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
                s.write_all(format!("GET {target} HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes())
                    .await
                    .unwrap();
                let mut reply = Vec::new();
                if target.contains("ours") {
                    // Ours is answered by the caller below.
                    return s;
                }
                let _ = s.read_to_end(&mut reply).await;
            }
            unreachable!()
        });
        let (code, mut socket) =
            await_callback(&listener, "/callback", "ours", LoginProvider::Claude).await;
        assert_eq!(code.unwrap(), "c0de");
        socket
            .write_all(
                http_response("302 Found", &[("Location", "zeron://signed-in")], "").as_bytes(),
            )
            .await
            .unwrap();
        socket.shutdown().await.unwrap();
        let mut browser = browser.await.unwrap();
        let mut reply = String::new();
        browser.read_to_string(&mut reply).await.unwrap();
        assert!(reply.starts_with("HTTP/1.1 302") && reply.contains("Location: zeron://signed-in"));
    }
}
