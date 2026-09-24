//! Inbound bearer verification for the remote engine listener.
//!
//! Upstream zeron routes remote clients through the edge Worker, which
//! verifies WorkOS AuthKit access-token JWTs against the WorkOS JWKS
//! (`edge/src/auth.ts`). The remote listener serves clients directly, so it
//! performs the same verification itself:
//!
//! - **WorkOS mode** (a WorkOS client id is configured): the bearer is an
//!   AuthKit access token, verified against the WorkOS JWKS with the issuer
//!   pinned to the client — the same checks the edge applies to relay
//!   clients. `WORKOS_ISSUER`/`WORKOS_JWKS_URL` override the endpoints,
//!   matching the edge's env.
//! - **Dev mode** (no client id): the bearer IS the user id — optionally
//!   `userId@orgId` to carry a fake org claim — mirroring the edge's dev
//!   verification so local development and tests run offline.
//!
//! The listener additionally serves the browser's sign-in surface on the
//! authorizer's word: `sign_in_config` reports the mode and builds the
//! AuthKit authorize URL (the same URL `auth.rs` builds for device
//! sign-in, redirecting to the edge's hosted paste-code callback), and
//! `exchange_code` proxies the secret-bearing code exchange to the edge —
//! the engine is a public client, so the browser never needs to reach the
//! edge itself.

use std::time::{Duration, Instant};

use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

/// A verified remote caller.
#[derive(Debug, Clone)]
pub struct RemoteIdentity {
    pub user_id: String,
    /// WorkOS `org_id` claim when the caller's session is org-scoped.
    pub org_id: Option<String>,
}

/// How long a fetched JWKS stays fresh before the verifier refetches it.
const JWKS_TTL: Duration = Duration::from_secs(3600);
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Deserialize)]
struct Claims {
    sub: String,
    #[serde(default)]
    org_id: Option<String>,
}

enum Mode {
    /// The bearer is the user id; `userId@orgId` carries a fake org claim.
    Dev,
    /// The bearer is a WorkOS AuthKit access token.
    Workos { issuer: String, jwks_url: String },
}

/// The pre-auth sign-in surface the listener reports to a browser:
/// `GET /auth/config`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SignInConfig {
    /// `"dev"` (the bearer is the user id) or `"workos"`.
    pub mode: &'static str,
    /// The AuthKit authorize URL when `mode` is `"workos"` — redirect the
    /// browser here; the callback is the edge's hosted paste-code page.
    /// `null` in dev mode.
    pub authorize_url: Option<String>,
}

/// The WorkOS endpoints the sign-in surface needs, captured at construction.
#[derive(Debug, Clone)]
struct SignInEndpoints {
    api_base: String,
    client_id: String,
    edge_url: String,
}

/// The outcome of a proxied code exchange, mapped by the listener to HTTP.
#[derive(Debug)]
pub enum CodeExchange {
    /// The edge's token payload (`user`, `accessToken`, `refreshToken`),
    /// passed through to the caller.
    Tokens(serde_json::Value),
    /// No WorkOS client id configured — dev mode never exchanges codes.
    NotConfigured,
    /// The edge rejected the code (refused or expired).
    Rejected,
    /// The edge could not be reached.
    Unreachable,
}

/// Verifies inbound remote-listener bearers. Shared as one `Arc` per engine.
pub struct RemoteAuthorizer {
    mode: Mode,
    http: reqwest::Client,
    jwks: RwLock<Option<(Instant, JwkSet)>>,
    sign_in: Option<SignInEndpoints>,
}

impl RemoteAuthorizer {
    /// Build from the engine's WorkOS client id and edge URL:
    /// `None` client id = dev mode; a missing edge URL disables the
    /// sign-in surface while WorkOS bearer verification still applies.
    pub fn new(workos_client_id: Option<&str>, edge_url: Option<&str>) -> Self {
        let http = reqwest::Client::builder()
            .timeout(HTTP_TIMEOUT)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        let mode = match workos_client_id {
            Some(client_id) => Mode::Workos {
                issuer: std::env::var("WORKOS_ISSUER").unwrap_or_else(|_| {
                    format!("https://api.workos.com/user_management/{client_id}")
                }),
                jwks_url: std::env::var("WORKOS_JWKS_URL")
                    .unwrap_or_else(|_| format!("https://api.workos.com/sso/jwks/{client_id}")),
            },
            None => Mode::Dev,
        };
        let sign_in = match (workos_client_id, edge_url) {
            (Some(client_id), Some(edge_url)) => Some(SignInEndpoints {
                api_base: std::env::var("WORKOS_API_BASE")
                    .unwrap_or_else(|_| "https://api.workos.com".into()),
                client_id: client_id.to_owned(),
                edge_url: edge_url.trim_end_matches('/').to_owned(),
            }),
            _ => None,
        };
        Self {
            mode,
            http,
            jwks: RwLock::new(None),
            sign_in,
        }
    }

    /// The sign-in mode and authorize URL the listener reports pre-auth.
    /// The state is a fresh nonce per call, matching device sign-in's CSRF
    /// discipline; the pasted `state.code` string comes back through
    /// [`Self::exchange_code`].
    pub fn sign_in_config(&self) -> SignInConfig {
        let Some(endpoints) = &self.sign_in else {
            return SignInConfig {
                mode: "dev",
                authorize_url: None,
            };
        };
        let redirect_uri = format!("{}/auth/cli/callback", endpoints.edge_url);
        let url = format!(
            "{}/user_management/authorize?response_type=code&client_id={}&redirect_uri={}&provider=authkit&state={}",
            endpoints.api_base,
            url_encode(&endpoints.client_id),
            url_encode(&redirect_uri),
            uuid::Uuid::new_v4()
        );
        SignInConfig {
            mode: "workos",
            authorize_url: Some(url),
        }
    }

    /// The edge base URL when the sign-in surface is configured.
    pub fn edge_url(&self) -> Option<&str> {
        self.sign_in
            .as_ref()
            .map(|endpoints| endpoints.edge_url.as_str())
    }

    /// Proxy a WorkOS authorization code to the edge's `/auth/exchange` and
    /// pass the tokens through. The API key never leaves the edge; the
    /// engine is a public client like the desktop's own sign-in.
    pub async fn exchange_code(&self, code: &str) -> CodeExchange {
        let Some(endpoints) = &self.sign_in else {
            return CodeExchange::NotConfigured;
        };
        let url = format!("{}/auth/exchange", endpoints.edge_url);
        let response = self
            .http
            .post(&url)
            .json(&serde_json::json!({ "code": code }))
            .send()
            .await;
        match response {
            Ok(response) if response.status().is_success() => {
                match response.json::<serde_json::Value>().await {
                    Ok(tokens) => CodeExchange::Tokens(tokens),
                    Err(error) => {
                        tracing::warn!(%error, "remote auth: edge exchange returned an invalid body");
                        CodeExchange::Unreachable
                    }
                }
            }
            Ok(response) => {
                tracing::warn!(status = %response.status(), "remote auth: edge exchange rejected the code");
                CodeExchange::Rejected
            }
            Err(error) => {
                tracing::warn!(%error, "remote auth: edge exchange failed");
                CodeExchange::Unreachable
            }
        }
    }

    /// `Some(identity)` when the credential is valid; `None` rejects.
    pub async fn verify(&self, credential: &str) -> Option<RemoteIdentity> {
        match &self.mode {
            Mode::Dev => {
                let (user_id, org_id) = match credential.split_once('@') {
                    Some((user_id, org_id)) => (user_id, Some(org_id)),
                    None => (credential, None),
                };
                (!user_id.is_empty()).then(|| RemoteIdentity {
                    user_id: user_id.to_owned(),
                    org_id: org_id.map(str::to_owned),
                })
            }
            Mode::Workos { issuer, jwks_url } => {
                self.verify_jwt(credential, issuer, jwks_url).await
            }
        }
    }

    async fn verify_jwt(
        &self,
        credential: &str,
        issuer: &str,
        jwks_url: &str,
    ) -> Option<RemoteIdentity> {
        let header = decode_header(credential).ok()?;
        let kid = header.kid.as_deref()?;
        // Cached JWKS, refetched once when the kid is unknown (key rotation).
        let key = match self.cached_key(kid).await {
            Some(key) => key,
            None => {
                self.refresh_jwks(jwks_url).await;
                self.cached_key(kid).await?
            }
        };
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_issuer(&[issuer]);
        let data = decode::<Claims>(credential, &key, &validation).ok()?;
        (!data.claims.sub.is_empty()).then(|| RemoteIdentity {
            user_id: data.claims.sub,
            org_id: data.claims.org_id,
        })
    }

    async fn cached_key(&self, kid: &str) -> Option<DecodingKey> {
        let cache = self.jwks.read().await;
        let Some((fetched_at, set)) = cache.as_ref() else {
            return None;
        };
        if fetched_at.elapsed() > JWKS_TTL {
            return None;
        }
        DecodingKey::from_jwk(set.find(kid)?).ok()
    }

    async fn refresh_jwks(&self, jwks_url: &str) {
        let fetched = match self.http.get(jwks_url).send().await {
            Ok(response) => match response.error_for_status() {
                Ok(response) => response.json::<JwkSet>().await.ok(),
                Err(error) => {
                    tracing::warn!(%error, "remote auth: JWKS fetch rejected");
                    None
                }
            },
            Err(error) => {
                tracing::warn!(%error, "remote auth: JWKS fetch failed");
                None
            }
        };
        if let Some(set) = fetched {
            *self.jwks.write().await = Some((Instant::now(), set));
        }
    }
}

/// Percent-encode a query component (RFC 3986 unreserved characters stay).
fn url_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(*byte as char);
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}
