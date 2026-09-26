//! Edge auth — `/auth/exchange`, `/auth/orgs`, `/auth/refresh`
//! (edge/src/auth-routes.ts), plus the per-client bearer provider.
//!
//! Two live modes, mirroring the engine:
//! - WorkOS: paste-code exchange → access/refresh pair; a refresh scoped to an
//!   organization adds the `org_id` claim the registry room requires.
//! - Dev (`AUTH_MODE=dev` edge): the bearer IS `userId@orgId`.
//!
//! WorkOS refresh tokens are single-use: [`TokenProvider`] single-flights every
//! refresh (a cold launch's N room dials used to race N refreshes with the
//! same token — one won, the rest dialed with a dead token and sat in
//! backoff), and reports the rotated pair to the platform.

use std::sync::{Arc, Mutex, OnceLock};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::config::AuthTokens;
use crate::error::{ClientError, Result};
use crate::events::{ClientEvent, EventPump};
use crate::lock;

/// Refresh this long before `exp` (legacy AppConfig: 60s early margin).
pub const EARLY_REFRESH_SECS: i64 = 60;

/// Production endpoints (edge/wrangler.jsonc). Mobile always talks to prod —
/// a stale override once broke sign-in in the worst ghost way.
pub const PRODUCTION_EDGE_URL: &str = "https://edge.zeron.sh";
pub const WORKOS_CLIENT_ID: &str = "client_01KWD0EAKZKD50YCQJNYSRE4BY";
pub const WORKOS_API_BASE: &str = "https://api.workos.com";
/// OAuth redirect: `zeron://callback?code=…&state=…`.
pub const CALLBACK_SCHEME: &str = "zeron";

fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                match u8::from_str_radix(&value[i + 1..i + 3], 16) {
                    Ok(b) => {
                        out.push(b);
                        i += 3;
                        continue;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            b'+' => out.push(b' '),
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// WorkOS AuthKit authorization-code URL (the ASWebAuthenticationSession
/// start URL). `state` is the caller's CSRF nonce.
pub fn workos_authorize_url(state: &str) -> String {
    format!(
        "{WORKOS_API_BASE}/user_management/authorize?response_type=code&client_id={}&redirect_uri={}&provider=authkit&state={}",
        percent_encode(WORKOS_CLIENT_ID),
        percent_encode(&format!("{CALLBACK_SCHEME}://callback")),
        percent_encode(state),
    )
}

/// The `code` + `state` of an OAuth callback URL, or the provider's error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthCallback {
    Code { code: String, state: Option<String> },
    Error { error: String, description: Option<String> },
}

pub fn parse_auth_callback(url: &str) -> Option<AuthCallback> {
    let query = url.split_once('?')?.1;
    let query = query.split('#').next().unwrap_or(query);
    let mut params = std::collections::HashMap::new();
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        params.insert(percent_decode(key), percent_decode(value));
    }
    if let Some(error) = params.remove("error") {
        return Some(AuthCallback::Error {
            error,
            description: params.remove("error_description"),
        });
    }
    let code = params.remove("code").filter(|c| !c.is_empty())?;
    Some(AuthCallback::Code {
        code,
        state: params.remove("state"),
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthUser {
    pub id: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub first_name: Option<String>,
    #[serde(default)]
    pub last_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthOrg {
    pub id: String,
    pub organization_id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthExchange {
    pub user: AuthUser,
    pub tokens: AuthTokens,
}

pub(crate) fn http() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .expect("reqwest client")
    })
}

fn endpoint(edge_url: &str, path: &str) -> String {
    format!("{}/{path}", edge_url.trim_end_matches('/'))
}

async fn check(response: reqwest::Response) -> Result<reqwest::Response> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let body = response.text().await.unwrap_or_default();
    if status.is_client_error() {
        Err(ClientError::Auth(format!("{status}: {body}")))
    } else {
        Err(ClientError::Network(format!("{status}: {body}")))
    }
}

fn net(err: reqwest::Error) -> ClientError {
    ClientError::Network(err.to_string())
}

/// WorkOS paste-code exchange.
pub async fn exchange_code(edge_url: &str, code: &str) -> Result<AuthExchange> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Reply {
        user: AuthUser,
        access_token: String,
        refresh_token: String,
    }
    let url = endpoint(edge_url, "auth/exchange");
    let code = code.trim().to_owned();
    crate::runtime::run(async move {
        let response = http()
            .post(url)
            .json(&serde_json::json!({ "code": code }))
            .send()
            .await
            .map_err(net)?;
        let reply: Reply = check(response).await?.json().await.map_err(net)?;
        Ok(AuthExchange {
            user: reply.user,
            tokens: AuthTokens {
                access_token: reply.access_token,
                refresh_token: reply.refresh_token,
            },
        })
    })
    .await
}

/// Organizations the (unscoped) access token's user belongs to.
pub async fn list_orgs(edge_url: &str, access_token: &str) -> Result<Vec<AuthOrg>> {
    #[derive(Deserialize)]
    struct Reply {
        orgs: Vec<AuthOrg>,
    }
    let url = endpoint(edge_url, "auth/orgs");
    let token = access_token.to_owned();
    crate::runtime::run(async move {
        let response = http()
            .get(url)
            .bearer_auth(token)
            .send()
            .await
            .map_err(net)?;
        let reply: Reply = check(response).await?.json().await.map_err(net)?;
        Ok(reply.orgs)
    })
    .await
}

/// Rotate the pair; `organization_id` re-scopes the access token to an org
/// (the org picker's "select" step).
pub async fn refresh(
    edge_url: &str,
    refresh_token: &str,
    organization_id: Option<&str>,
) -> Result<AuthTokens> {
    let url = endpoint(edge_url, "auth/refresh");
    let mut body = serde_json::json!({ "refreshToken": refresh_token });
    if let Some(org) = organization_id {
        body["organizationId"] = serde_json::Value::String(org.to_owned());
    }
    crate::runtime::run(async move {
        let response = http().post(url).json(&body).send().await.map_err(net)?;
        check(response).await?.json().await.map_err(net)
    })
    .await
}

/// Decode a JWT's `exp` claim (seconds). `None` for anything unparseable.
pub fn jwt_expiry(jwt: &str) -> Option<i64> {
    let mut segments = jwt.split('.');
    let (_, payload, _) = (segments.next()?, segments.next()?, segments.next()?);
    let bytes = base64url_decode(payload)?;
    let claims: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    claims.get("exp")?.as_f64().map(|exp| exp as i64)
}

/// Expired (or inside the early-refresh margin). Unparseable tokens read as
/// NOT expired — the server is the arbiter.
pub fn jwt_expired(jwt: &str, now_secs: i64) -> bool {
    jwt_expiry(jwt).is_some_and(|exp| now_secs > exp - EARLY_REFRESH_SECS)
}

fn base64url_decode(input: &str) -> Option<Vec<u8>> {
    fn value(c: u8) -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => (c - b'A') as u32,
            b'a'..=b'z' => (c - b'a' + 26) as u32,
            b'0'..=b'9' => (c - b'0' + 52) as u32,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            _ => return None,
        })
    }
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let mut buffer = 0u32;
    let mut bits = 0u32;
    for &c in input.as_bytes() {
        if c == b'=' {
            break;
        }
        buffer = (buffer << 6) | value(c)?;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    Some(out)
}

enum Mode {
    Demo,
    Dev {
        bearer: String,
    },
    WorkOs {
        edge_url: String,
        org_id: String,
        tokens: Mutex<AuthTokens>,
        refresh_gate: tokio::sync::Mutex<()>,
    },
}

/// Per-client bearer source: dev bearer, or a WorkOS access token refreshed
/// (single-flight) when inside the early-refresh margin.
pub(crate) struct TokenProvider {
    mode: Mode,
    events: Arc<EventPump>,
    expired: AtomicBool,
}

impl TokenProvider {
    pub(crate) fn new(
        credentials: &crate::config::Credentials,
        edge_url: &str,
        events: Arc<EventPump>,
    ) -> Self {
        use crate::config::Credentials;
        let mode = match credentials {
            Credentials::Demo(_) => Mode::Demo,
            Credentials::Dev { user_id, org_id } => Mode::Dev {
                bearer: if org_id.is_empty() {
                    user_id.clone()
                } else {
                    format!("{user_id}@{org_id}")
                },
            },
            Credentials::WorkOs { org_id, tokens, .. } => Mode::WorkOs {
                edge_url: edge_url.to_owned(),
                org_id: org_id.clone(),
                tokens: Mutex::new(tokens.clone()),
                refresh_gate: tokio::sync::Mutex::new(()),
            },
        };
        Self {
            mode,
            events,
            expired: AtomicBool::new(false),
        }
    }

    /// Replace the pair (the platform re-signed-in or restored newer tokens).
    pub(crate) fn update_tokens(&self, next: AuthTokens) {
        if let Mode::WorkOs { tokens, .. } = &self.mode {
            *lock(tokens) = next;
            self.expired.store(false, Ordering::Release);
        }
    }

    /// Current bearer, refreshing first when the access token is (nearly)
    /// expired. Network failures fall back to the stale token — the server
    /// rejects and the caller's backoff retries through here.
    pub(crate) async fn bearer(&self) -> Result<String> {
        match &self.mode {
            Mode::Demo => Err(ClientError::Auth("demo mode has no edge credentials".into())),
            Mode::Dev { bearer } => Ok(bearer.clone()),
            Mode::WorkOs {
                edge_url,
                org_id,
                tokens,
                refresh_gate,
            } => {
                let now = chrono::Utc::now().timestamp();
                let current = lock(tokens).clone();
                if !jwt_expired(&current.access_token, now) {
                    return Ok(current.access_token);
                }
                if self.expired.load(Ordering::Acquire) {
                    return Err(ClientError::Auth("session expired".into()));
                }
                let _gate = refresh_gate.lock().await;
                // Joined an in-flight refresh: it already rotated the pair.
                let current = lock(tokens).clone();
                if !jwt_expired(&current.access_token, chrono::Utc::now().timestamp()) {
                    return Ok(current.access_token);
                }
                match refresh(edge_url, &current.refresh_token, Some(org_id)).await {
                    Ok(next) => {
                        *lock(tokens) = next.clone();
                        self.events.ordered(ClientEvent::AuthRefreshed(next.clone()));
                        Ok(next.access_token)
                    }
                    Err(ClientError::Auth(reason)) => {
                        if !self.expired.swap(true, Ordering::AcqRel) {
                            self.events
                                .ordered(ClientEvent::AuthExpired { reason: reason.clone() });
                        }
                        Err(ClientError::Auth(reason))
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, "token refresh failed; using the stale access token");
                        Ok(current.access_token)
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorize_url_and_callback_round_trip() {
        let url = workos_authorize_url("s t&1");
        assert!(url.starts_with("https://api.workos.com/user_management/authorize?response_type=code"));
        assert!(url.contains("redirect_uri=zeron%3A%2F%2Fcallback"));
        assert!(url.contains("state=s%20t%261"));
        assert_eq!(
            parse_auth_callback("zeron://callback?code=abc&state=s%20t%261"),
            Some(AuthCallback::Code {
                code: "abc".into(),
                state: Some("s t&1".into())
            })
        );
        assert!(matches!(
            parse_auth_callback("zeron://callback?error=access_denied"),
            Some(AuthCallback::Error { .. })
        ));
        assert_eq!(parse_auth_callback("zeron://callback"), None);
    }

    #[test]
    fn jwt_expiry_reads_the_exp_claim() {
        // {"alg":"none"}.{"exp":1000}.sig
        let jwt = "eyJhbGciOiJub25lIn0.eyJleHAiOjEwMDB9.c2ln";
        assert_eq!(jwt_expiry(jwt), Some(1000));
        assert!(jwt_expired(jwt, 1000 - EARLY_REFRESH_SECS + 1));
        assert!(!jwt_expired(jwt, 900));
        assert!(!jwt_expired("not-a-jwt", i64::MAX));
    }
}
