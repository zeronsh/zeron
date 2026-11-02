//! Types shared between the proxy and the engine's credential source.

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use http::header::{AUTHORIZATION, InvalidHeaderValue};
use http::{HeaderName, HeaderValue};

use crate::provider::Provider;

/// The engine's id for a saved login (`AgentAccount.id`). Not a secret.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AccountId(Arc<str>);

impl AccountId {
    pub fn new(id: impl Into<Arc<str>>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for AccountId {
    fn from(id: &str) -> Self {
        Self::new(id)
    }
}

impl From<String> for AccountId {
    fn from(id: String) -> Self {
        Self::new(id)
    }
}

impl fmt::Debug for AccountId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&*self.0, f)
    }
}

impl fmt::Display for AccountId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// What a route serves: one harness process of one chat. Registered by the
/// engine when it spawns the process; handed back to the credential source on
/// every lease so it can apply the chat's sticky assignment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouteInfo {
    pub chat_id: String,
    /// Kebab-case harness id (`claude-code`, `codex`, ...).
    pub harness: String,
    pub provider: Provider,
}

/// An account-bound edit to a JSON request body, applied per attempt when the
/// body is uncompressed JSON. Paths are RFC 6901 pointers; an edit whose
/// target is missing is skipped.
#[derive(Clone, Debug, PartialEq)]
pub enum BodyEdit {
    /// Replace the value at `pointer`.
    Replace {
        pointer: String,
        value: serde_json::Value,
    },
    /// The value at `pointer` is a string holding a JSON object; set these
    /// keys inside it and keep the rest (Claude's `metadata.user_id`).
    EmbeddedJson {
        pointer: String,
        fields: Vec<(String, serde_json::Value)>,
    },
}

impl BodyEdit {
    pub fn pointer(&self) -> &str {
        match self {
            BodyEdit::Replace { pointer, .. } | BodyEdit::EmbeddedJson { pointer, .. } => pointer,
        }
    }

    /// Applies the edit; true when the body changed.
    pub(crate) fn apply(&self, body: &mut serde_json::Value) -> bool {
        match self {
            BodyEdit::Replace { pointer, value } => match body.pointer_mut(pointer) {
                Some(slot) if slot != value => {
                    *slot = value.clone();
                    true
                }
                _ => false,
            },
            BodyEdit::EmbeddedJson { pointer, fields } => {
                let Some(slot) = body.pointer_mut(pointer) else {
                    return false;
                };
                let Some(text) = slot.as_str() else {
                    return false;
                };
                let Ok(serde_json::Value::Object(mut inner)) = serde_json::from_str(text) else {
                    return false;
                };
                let mut changed = false;
                for (key, value) in fields {
                    if inner.get(key) != Some(value) {
                        inner.insert(key.clone(), value.clone());
                        changed = true;
                    }
                }
                if changed {
                    *slot = serde_json::Value::String(serde_json::Value::Object(inner).to_string());
                }
                changed
            }
        }
    }
}

/// An account handed out for one upstream attempt, with the credential
/// headers to put on the request. Header values are marked sensitive and the
/// `Debug` output never shows them.
#[derive(Clone)]
pub struct Lease {
    pub account: AccountId,
    pub(crate) headers: Vec<(HeaderName, HeaderValue)>,
    pub(crate) body_edits: Vec<BodyEdit>,
    /// Forward the CLI's own credential instead of swapping one in.
    pub(crate) passthrough: bool,
}

impl Lease {
    pub fn new(account: impl Into<AccountId>) -> Self {
        Self {
            account: account.into(),
            headers: Vec::new(),
            body_edits: Vec::new(),
            passthrough: false,
        }
    }

    /// The request goes out with the credential the CLI sent (its own
    /// login): for a route whose chat runs on the CLI's live account, or
    /// while rotation is off. `account` names that login for reports and
    /// exclusion (so a limit on it can still fail over to a swapped account).
    pub fn passthrough(account: impl Into<AccountId>) -> Self {
        Self {
            passthrough: true,
            ..Self::new(account)
        }
    }

    pub fn is_passthrough(&self) -> bool {
        self.passthrough
    }

    /// `Authorization: Bearer <token>`.
    pub fn bearer(self, token: &str) -> Result<Self, InvalidHeaderValue> {
        self.header(AUTHORIZATION, &format!("Bearer {token}"))
    }

    /// Sets (replaces) one upstream header. Any inbound header of the same
    /// name is dropped before forwarding.
    pub fn header(mut self, name: HeaderName, value: &str) -> Result<Self, InvalidHeaderValue> {
        let mut value = HeaderValue::from_str(value)?;
        value.set_sensitive(true);
        self.headers.retain(|(n, _)| *n != name);
        self.headers.push((name, value));
        Ok(self)
    }

    /// Adds an account-bound body edit.
    pub fn body_edit(mut self, edit: BodyEdit) -> Self {
        self.body_edits.push(edit);
        self
    }

    /// Claude Code puts the logged-in account's identity in
    /// `metadata.user_id`, a JSON object serialized into a string
    /// (`device_id`, `account_uuid`, `session_id`, ...). Rewrites the first
    /// two to the leased account's, keeping the session and any other keys.
    pub fn claude_identity(self, device_id: &str, account_uuid: &str) -> Self {
        self.body_edit(BodyEdit::EmbeddedJson {
            pointer: "/metadata/user_id".to_string(),
            fields: vec![
                ("device_id".to_string(), device_id.into()),
                ("account_uuid".to_string(), account_uuid.into()),
            ],
        })
    }

    pub fn header_names(&self) -> impl Iterator<Item = &HeaderName> {
        self.headers.iter().map(|(n, _)| n)
    }
}

impl fmt::Debug for Lease {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Lease")
            .field("account", &self.account)
            .field("passthrough", &self.passthrough)
            .field(
                "headers",
                &self
                    .headers
                    .iter()
                    .map(|(n, _)| n.as_str())
                    .collect::<Vec<_>>(),
            )
            .field(
                "body_edits",
                &self
                    .body_edits
                    .iter()
                    .map(BodyEdit::pointer)
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum LeaseError {
    /// No account is left to try (all excluded, signed out, or none saved).
    #[error("no account available")]
    NoAccount,
    /// The source failed (e.g. a token refresh error). The message must not
    /// contain credentials.
    #[error("credential source failed: {0}")]
    Failed(String),
}

/// One rate-limit window as reported by the provider's response headers.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RateLimitWindow {
    /// Provider's window name: `five_hour`, `seven_day`, `primary`, `secondary`...
    pub name: String,
    /// 0..=100.
    pub used_percent: Option<f64>,
    pub window_minutes: Option<u64>,
    pub resets_at: Option<DateTime<Utc>>,
    /// Provider status for this window (`allowed`, `allowed_warning`, `rejected`).
    pub status: Option<String>,
}

/// Usage information taken from one upstream response's headers.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RateLimitSnapshot {
    /// Overall status when the provider reports one (Anthropic
    /// `anthropic-ratelimit-unified-status`).
    pub status: Option<String>,
    pub windows: Vec<RateLimitWindow>,
    /// The raw rate-limit headers (names lowercased), for fields this crate
    /// does not interpret.
    pub headers: Vec<(String, String)>,
}

impl RateLimitSnapshot {
    pub fn is_empty(&self) -> bool {
        self.status.is_none() && self.windows.is_empty() && self.headers.is_empty()
    }
}

/// What one upstream attempt said about the account that served it.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// A successful (non-error) response.
    Ok { rate_limits: RateLimitSnapshot },
    /// The account's usage quota is exhausted. The source should stop handing
    /// the account out until `resets_at` (or until fresh usage says otherwise).
    LimitReached {
        resets_at: Option<DateTime<Utc>>,
        window: Option<String>,
        rate_limits: RateLimitSnapshot,
    },
    /// The provider refused the credential (401/403). The source may refresh
    /// the token or mark the login signed out.
    Rejected { status: u16 },
    /// The provider refused the conversation history this account was sent:
    /// thinking-block signatures (Claude) or encrypted reasoning (Codex) that
    /// another account produced. Typically follows a switch. The request is
    /// not retried; the engine decides (e.g. let the turn fail and start the
    /// next one with stripped reasoning, or move the chat back).
    IncompatibleHistory { status: u16 },
}

/// What to do with a request the provider refused because its history holds
/// reasoning another account produced ([`Outcome::IncompatibleHistory`]).
#[derive(Clone, Debug, PartialEq)]
pub enum HistoryRecovery {
    /// Pass the provider's error on (with an explanation).
    PassThrough,
    /// The chat went back to the account its history belongs to: lease
    /// again and send the request there.
    Retry,
    /// That account is out of usage: answer with the provider's own
    /// usage-limit response, so the CLI stops the way it does at a limit.
    LimitReached {
        resets_at: Option<DateTime<Utc>>,
        window: Option<String>,
    },
}

/// Supplies credentials per attempt. Implemented by the engine.
///
/// `lease` is called once per upstream attempt (every request, plus every
/// failover retry), so it should be cheap once tokens are cached. `inbound`
/// is the CLI's request as received (its own credential included), for a
/// source that checks which login the CLI holds. Accounts in `exclude`
/// already failed for this request and must not be returned.
#[async_trait]
pub trait CredentialSource: Send + Sync + 'static {
    async fn lease(
        &self,
        route: &RouteInfo,
        inbound: &http::HeaderMap,
        exclude: &[AccountId],
    ) -> Result<Lease, LeaseError>;

    /// Called once per upstream response, as soon as it is classified, and
    /// again if a streamed response later carries a quota or credential error
    /// in-band (that one cannot be retried). Must not block. A refused
    /// passthrough credential (the CLI's own login) is not reported: the CLI
    /// handles that itself.
    fn report(&self, route: &RouteInfo, account: &AccountId, outcome: Outcome);

    /// The provider refused `account`'s request for history another account
    /// produced (after [`Self::report`]). Asked at most once per request.
    async fn incompatible_history(
        &self,
        _route: &RouteInfo,
        _account: &AccountId,
    ) -> HistoryRecovery {
        HistoryRecovery::PassThrough
    }
}

/// Why a route moved to another account.
#[derive(Clone, Debug, PartialEq)]
pub enum SwitchReason {
    LimitReached {
        resets_at: Option<DateTime<Utc>>,
        window: Option<String>,
    },
    Rejected,
    /// The chat went back to the account its history belongs to.
    HistoryBound,
    /// The credential source handed out a different account on its own
    /// (e.g. the engine re-assigned the chat).
    Reassigned,
}

/// Events for the engine/UI.
#[derive(Clone, Debug, PartialEq)]
pub enum ProxyEvent {
    AccountSwitched {
        route: RouteInfo,
        from: Option<AccountId>,
        to: AccountId,
        reason: SwitchReason,
    },
    /// Every account was tried for one request; the last upstream error was
    /// passed through to the CLI.
    Exhausted {
        route: RouteInfo,
        tried: Vec<AccountId>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn embedded_json_edit_keeps_other_keys() {
        let mut body = json!({
            "metadata": {"user_id": r#"{"device_id":"d0","account_uuid":"u0","session_id":"s"}"#},
        });
        let lease = Lease::new("a").claude_identity("d1", "u1");
        assert!(lease.body_edits[0].apply(&mut body));
        let inner: serde_json::Value =
            serde_json::from_str(body["metadata"]["user_id"].as_str().unwrap()).unwrap();
        assert_eq!(
            inner,
            json!({"device_id":"d1","account_uuid":"u1","session_id":"s"})
        );
        // Already the leased identity: nothing to do.
        assert!(!lease.body_edits[0].apply(&mut body));
    }

    #[test]
    fn edits_skip_missing_or_foreign_shapes() {
        let lease = Lease::new("a").claude_identity("d1", "u1");
        let mut no_metadata = json!({"model": "m"});
        assert!(!lease.body_edits[0].apply(&mut no_metadata));
        let mut legacy = json!({"metadata": {"user_id": "user_abc_account_def_session_ghi"}});
        assert!(!lease.body_edits[0].apply(&mut legacy));
        assert_eq!(
            legacy["metadata"]["user_id"],
            "user_abc_account_def_session_ghi"
        );

        let replace = BodyEdit::Replace {
            pointer: "/project".into(),
            value: json!("p-1"),
        };
        let mut body = json!({"project": "p-0"});
        assert!(replace.apply(&mut body));
        assert_eq!(body["project"], "p-1");
    }

    #[test]
    fn lease_debug_hides_secrets() {
        let lease = Lease::new("acct")
            .bearer("sk-ant-oat-secret")
            .unwrap()
            .claude_identity("device-secret", "uuid");
        let shown = format!("{lease:?}");
        assert!(shown.contains("acct"));
        assert!(shown.contains("authorization"));
        assert!(!shown.contains("sk-ant-oat-secret"));
        assert!(!shown.contains("device-secret"));
        assert!(lease.headers[0].1.is_sensitive());
    }
}
