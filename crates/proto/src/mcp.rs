//! Managed MCP server wire types: presets, configured servers, OAuth flows,
//! and snapshot views shared across engine, UI, and RPC.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Transport used to reach an external MCP server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum McpTransport {
    #[default]
    Http,
    Sse,
    Stdio,
}

/// Authentication mechanism for an external MCP server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum McpAuthKind {
    None,
    #[default]
    #[serde(rename = "oauth", alias = "oAuth")]
    OAuth,
    Bearer,
    Headers,
}

/// Where an MCP server's configuration and credentials live.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum McpStorage {
    Vault,
    #[default]
    Local,
}

/// Live readiness / auth status of a configured MCP server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum McpServerStatus {
    Connected,
    #[default]
    NeedsAuth,
    Authorizing,
    ReauthRequired,
    Error,
    Disabled,
}

/// Progress state of an in-flight interactive MCP OAuth flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum McpAuthFlowStatus {
    #[default]
    Pending,
    Completed,
    Failed,
    Cancelled,
}

/// A built-in MCP server preset offered in the catalog.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct McpPreset {
    pub id: String,
    pub name: String,
    pub description: String,
    pub category: String,
    pub url: String,
    pub transport: McpTransport,
    pub auth_kind: McpAuthKind,
    pub docs_url: String,
    pub icon: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header_name: Option<String>,
}

/// Redacted view of a configured MCP server (never carries secret values).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct McpServerView {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    pub transport: McpTransport,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    pub args: Vec<String>,
    pub env_keys: Vec<String>,
    pub auth_kind: McpAuthKind,
    pub header_keys: Vec<String>,
    pub enabled: bool,
    pub storage: McpStorage,
    pub status: McpServerStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    pub scopes: Vec<String>,
    pub updated_at: i64,
}

/// Full MCP servers snapshot pushed by `WatchMcpServers` and returned by
/// `ListMcpServers` / mutations.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct McpServersSnapshot {
    pub storage: McpStorage,
    pub vault_available: bool,
    pub presets: Vec<McpPreset>,
    pub servers: Vec<McpServerView>,
}

/// Params for `UpsertMcpServer` (local IPC only; carries credentials).
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct UpsertMcpServerRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    pub transport: McpTransport,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub auth_kind: McpAuthKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bearer_token: Option<String>,
    pub headers: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    pub local_only: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oauth_client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oauth_client_secret: Option<String>,
    pub oauth_scopes: Vec<String>,
}

/// Hand-written so `{:?}` never logs bearer tokens, custom headers, env values,
/// or OAuth client secrets.
impl std::fmt::Debug for UpsertMcpServerRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UpsertMcpServerRequest")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("preset", &self.preset)
            .field("transport", &self.transport)
            .field("url", &self.url)
            .field("command", &self.command)
            .field("args", &self.args)
            .field("env_keys", &self.env.keys().collect::<Vec<_>>())
            .field("auth_kind", &self.auth_kind)
            .field(
                "bearer_token",
                &self.bearer_token.as_ref().map(|_| "<redacted>"),
            )
            .field("header_keys", &self.headers.keys().collect::<Vec<_>>())
            .field("enabled", &self.enabled)
            .field("local_only", &self.local_only)
            .field("oauth_client_id", &self.oauth_client_id)
            .field(
                "oauth_client_secret",
                &self.oauth_client_secret.as_ref().map(|_| "<redacted>"),
            )
            .field("oauth_scopes", &self.oauth_scopes)
            .finish()
    }
}

/// Params for `SetMcpServerEnabled`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SetMcpServerEnabledRequest {
    pub id: String,
    pub enabled: bool,
}

/// Params for `DeleteMcpServer`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DeleteMcpServerRequest {
    pub id: String,
}

/// Params for `StartMcpAuth`.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct StartMcpAuthRequest {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    pub scopes: Vec<String>,
}

impl std::fmt::Debug for StartMcpAuthRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StartMcpAuthRequest")
            .field("id", &self.id)
            .field("client_id", &self.client_id)
            .field(
                "client_secret",
                &self.client_secret.as_ref().map(|_| "<redacted>"),
            )
            .field("scopes", &self.scopes)
            .finish()
    }
}

/// Reply of `StartMcpAuth`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct StartMcpAuthResponse {
    pub flow_id: String,
    pub auth_url: String,
    pub callback_port: u16,
}

/// Params for `PollMcpAuth`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PollMcpAuthRequest {
    pub flow_id: String,
}

/// Reply of `PollMcpAuth`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PollMcpAuthResponse {
    pub status: McpAuthFlowStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Params for `CancelMcpAuth`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CancelMcpAuthRequest {
    pub flow_id: String,
}

/// Params for `DisconnectMcpAuth`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DisconnectMcpAuthRequest {
    pub id: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defaults_and_camel_case_wire_names() {
        assert_eq!(McpTransport::default(), McpTransport::Http);
        assert_eq!(McpAuthKind::default(), McpAuthKind::OAuth);
        assert_eq!(McpStorage::default(), McpStorage::Local);

        assert_eq!(serde_json::to_value(McpTransport::Http).unwrap(), json!("http"));
        assert_eq!(serde_json::to_value(McpTransport::Sse).unwrap(), json!("sse"));
        assert_eq!(serde_json::to_value(McpTransport::Stdio).unwrap(), json!("stdio"));

        assert_eq!(serde_json::to_value(McpAuthKind::None).unwrap(), json!("none"));
        assert_eq!(serde_json::to_value(McpAuthKind::OAuth).unwrap(), json!("oauth"));
        assert_eq!(serde_json::to_value(McpAuthKind::Bearer).unwrap(), json!("bearer"));
        assert_eq!(serde_json::to_value(McpAuthKind::Headers).unwrap(), json!("headers"));
        assert_eq!(
            serde_json::from_value::<McpAuthKind>(json!("oAuth")).unwrap(),
            McpAuthKind::OAuth
        );

        assert_eq!(serde_json::to_value(McpStorage::Vault).unwrap(), json!("vault"));
        assert_eq!(serde_json::to_value(McpStorage::Local).unwrap(), json!("local"));

        assert_eq!(
            serde_json::to_value(McpServerStatus::ReauthRequired).unwrap(),
            json!("reauthRequired")
        );
        assert_eq!(
            serde_json::to_value(McpAuthFlowStatus::Cancelled).unwrap(),
            json!("cancelled")
        );
    }

    #[test]
    fn empty_objects_deserialize_with_defaults_for_version_compat() {
        let snap: McpServersSnapshot = serde_json::from_value(json!({})).unwrap();
        assert_eq!(snap.storage, McpStorage::Local);
        assert!(!snap.vault_available);
        assert!(snap.presets.is_empty());
        assert!(snap.servers.is_empty());

        let view: McpServerView = serde_json::from_value(json!({})).unwrap();
        assert_eq!(view.transport, McpTransport::Http);
        assert_eq!(view.auth_kind, McpAuthKind::OAuth);
        assert_eq!(view.storage, McpStorage::Local);

        let upsert: UpsertMcpServerRequest = serde_json::from_value(json!({ "name": "linear" })).unwrap();
        assert_eq!(upsert.name, "linear");
        assert_eq!(upsert.transport, McpTransport::Http);
        assert_eq!(upsert.auth_kind, McpAuthKind::OAuth);
        assert!(!upsert.local_only);
    }

    #[test]
    fn debug_redacts_secrets() {
        let req = UpsertMcpServerRequest {
            name: "slack".into(),
            bearer_token: Some("xoxb-secret-token".into()),
            oauth_client_secret: Some("cs-secret".into()),
            headers: [("X-Api-Key".into(), "super-secret".into())].into(),
            env: [("TOKEN".into(), "env-secret".into())].into(),
            ..Default::default()
        };
        let printed = format!("{req:?}");
        assert!(!printed.contains("xoxb-secret-token"));
        assert!(!printed.contains("cs-secret"));
        assert!(!printed.contains("super-secret"));
        assert!(!printed.contains("env-secret"));
        assert!(printed.contains("X-Api-Key"));
        assert!(printed.contains("TOKEN"));

        let auth = StartMcpAuthRequest {
            id: "slack".into(),
            client_id: Some("cid".into()),
            client_secret: Some("very-secret".into()),
            scopes: vec![],
        };
        let printed = format!("{auth:?}");
        assert!(!printed.contains("very-secret"));
        assert!(printed.contains("cid"));
    }
}
