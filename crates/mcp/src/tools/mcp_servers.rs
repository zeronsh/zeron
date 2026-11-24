//! MCP server tools: what the person's MCP servers are (Settings → MCP),
//! and asking to change them.
//!
//! An agent reads the servers (no secret is in the list) and asks for a
//! change; the person answers in Zeron's window, which alone can (the
//! engine's `ResolveMcpRequest` is this device's app's, never an agent's:
//! see `crates/engine/src/mcp_servers/requests.rs`). A server that signs in
//! does so in the person's browser once they allow it.

use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{Value, json};
use zeron_rpc::{RpcError, methods};

use super::{ToolDef, Tools};

pub(super) const LIST_MCP_SERVERS: &str = "list_mcp_servers";
pub(super) const REQUEST_MCP_CHANGE: &str = "request_mcp_change";
pub(super) const NAMES: [&str; 2] = [LIST_MCP_SERVERS, REQUEST_MCP_CHANGE];

/// How long `request_mcp_change` may wait for the person, at most.
const MAX_WAIT: Duration = Duration::from_secs(600);
/// How often a waiting request is looked at again.
const POLL: Duration = Duration::from_secs(2);

pub(super) fn catalog() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: LIST_MCP_SERVERS,
            description: "The person's MCP servers in Zeron (Settings → MCP): each with its id, name, account (when a service is connected more than once), `agentName` (what agents see it as: their tools are mcp__<agentName>__…), status (connected, needsAuth, reauthRequired, error, disabled), whether it's on, and where it's kept (the vault syncs it to their devices and Cloud). `presets`: services that can be added by id (request_mcp_change). `requests`: changes asked for and how they were answered. No secret is listed.",
            input_schema: json!({ "type": "object", "properties": {} }),
        },
        ToolDef {
            name: REQUEST_MCP_CHANGE,
            description: concat!(
                "Ask to change the person's MCP servers: add one (a preset by id, a remote server by https URL that signs in with OAuth or needs nothing, or a command run on this device), remove one, or turn one on or off (by id or agentName, from list_mcp_servers). ",
                "Nothing changes until the person allows it in Zeron's window, which shows them what you asked and why: you can't allow it for them. ",
                "Once allowed, a server that signs in opens its sign-in in their browser. ",
                "A server that takes an API key can't be asked for (it would carry the key): ask the person to add it in Settings → MCP. ",
                "With wait_secs, waits up to that long for their answer (and, for a sign-in, for them to finish it) and says how it went; otherwise returns at once with the request's id: call again with the same change to see it, or list_mcp_servers. ",
                "New servers reach agents in chats started after they're connected."
            ),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["add", "remove", "enable", "disable"] },
                    "preset": { "type": "string", "description": "add: a preset's id (list_mcp_servers' presets), e.g. linear." },
                    "url": { "type": "string", "description": "add: a remote MCP server's https URL." },
                    "auth": { "type": "string", "enum": ["oauth", "none"], "description": "add with url: how it signs in (default oauth)." },
                    "command": { "type": "string", "description": "add: a command that runs a stdio MCP server on this device, e.g. npx." },
                    "args": { "type": "array", "items": { "type": "string" }, "description": "add with command: its arguments." },
                    "name": { "type": "string", "description": "add: its name (a preset's own when omitted)." },
                    "account": { "type": "string", "description": "add: a name for the account (Work, Personal), for a service connected more than once." },
                    "server": { "type": "string", "description": "remove / enable / disable: the server's id or agentName." },
                    "reason": { "type": "string", "description": "Why, in a sentence, shown to the person." },
                    "wait_secs": { "type": "integer", "minimum": 0, "maximum": 600, "description": "Wait up to this long for the person's answer (default 0: return at once)." }
                },
                "required": ["action"]
            }),
        },
    ]
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestArgs {
    action: String,
    #[serde(default)]
    preset: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    auth: Option<String>,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    account: Option<String>,
    #[serde(default)]
    server: Option<String>,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    wait_secs: u64,
}

/// The engine's `RequestMcpChange` params for `args`.
fn change_params(args: &RequestArgs) -> Result<Value, String> {
    let server = || {
        args.server
            .clone()
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| format!("{}: give the server (its id or agentName)", args.action))
    };
    let mut params = match args.action.as_str() {
        "add" => {
            let auth = match args.auth.as_deref() {
                None => None,
                Some("oauth") => Some("oauth"),
                Some("none") => Some("none"),
                Some(other) => return Err(format!("auth: oauth or none, not {other}")),
            };
            json!({
                "action": "add",
                "preset": args.preset,
                "url": args.url,
                "authKind": auth,
                "command": args.command,
                "args": args.args,
                "name": args.name,
                "accountLabel": args.account,
            })
        }
        "remove" => json!({ "action": "remove", "server": server()? }),
        "enable" | "disable" => json!({
            "action": "setEnabled",
            "server": server()?,
            "enabled": args.action == "enable",
        }),
        other => return Err(format!("action: add, remove, enable or disable, not {other}")),
    };
    // (Absent rather than null: the engine reads each as optional.)
    if let Some(map) = params.as_object_mut() {
        map.retain(|_, v| !v.is_null() && v.as_array().is_none_or(|a| !a.is_empty()));
        if let Some(reason) = &args.reason {
            map.insert("reason".into(), json!(reason));
        }
    }
    Ok(params)
}

fn plain(error: RpcError) -> String {
    match error {
        RpcError::Failed(message) => message,
        RpcError::UnknownMethod(_) => {
            "This Zeron can't manage MCP servers for agents yet; update Zeron to use it.".into()
        }
        other => other.to_string(),
    }
}

impl Tools {
    pub(super) async fn mcp_servers(&self, tool: &str, args: Value) -> Result<Value, String> {
        match tool {
            LIST_MCP_SERVERS => self.list_mcp_servers().await,
            _ => {
                let args: RequestArgs =
                    serde_json::from_value(if args.is_null() { json!({}) } else { args })
                        .map_err(|e| format!("invalid arguments: {e}"))?;
                self.request_mcp_change(args).await
            }
        }
    }

    async fn list_mcp_servers(&self) -> Result<Value, String> {
        let snap = self
            .zeron
            .call_rpc(methods::LIST_MCP_SERVERS, json!({}))
            .await
            .map_err(plain)?;
        let field = |v: &Value, key: &str| v.get(key).cloned().unwrap_or(Value::Null);
        let servers: Vec<Value> = snap["servers"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .map(|s| {
                json!({
                    "id": field(s, "id"),
                    "name": field(s, "name"),
                    "account": field(s, "accountLabel"),
                    "agentName": field(s, "agentName"),
                    "status": field(s, "status"),
                    "enabled": field(s, "enabled"),
                    "kept": if s["storage"] == "vault" { "vault (synced)" } else { "this device" },
                    "url": field(s, "url"),
                    "command": field(s, "command"),
                })
            })
            .collect();
        let presets: Vec<Value> = snap["presets"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .map(|p| json!({ "id": field(p, "id"), "name": field(p, "name"), "description": field(p, "description") }))
            .collect();
        Ok(json!({
            "servers": servers,
            "presets": presets,
            "requests": field(&snap, "requests"),
        }))
    }

    async fn request_mcp_change(&self, args: RequestArgs) -> Result<Value, String> {
        let mut params = change_params(&args)?;
        if let Some(chat) = &self.zeron.origin().chat_id {
            params["chatId"] = json!(chat);
        }
        let mut request = self
            .zeron
            .call_rpc(methods::REQUEST_MCP_CHANGE, params)
            .await
            .map_err(plain)?;
        let wait = Duration::from_secs(args.wait_secs).min(MAX_WAIT);
        let until = Instant::now() + wait;
        let id = request["id"].as_str().unwrap_or_default().to_owned();
        // Waiting: for the person's answer, then (a sign-in) for it to finish.
        while !wait.is_zero() && Instant::now() < until {
            let status = request["status"].as_str().unwrap_or_default();
            if status != "pending" && !(status == "approved" && self.signing_in(&request).await) {
                break;
            }
            tokio::time::sleep(POLL).await;
            request = self
                .zeron
                .call_rpc(methods::GET_MCP_REQUEST, json!({ "id": id }))
                .await
                .map_err(plain)?;
        }
        let server = self.server_of(&request).await;
        let said = match request["status"].as_str().unwrap_or_default() {
            "pending" => "Waiting for the person to allow it in Zeron's window (Settings → MCP).".to_owned(),
            "declined" => "The person declined it.".to_owned(),
            "failed" => format!(
                "Allowed, but it couldn't be done: {}",
                request["error"].as_str().unwrap_or("unknown error")
            ),
            _ => match server.as_ref().and_then(|s| s["status"].as_str()) {
                Some("connected") | None => "Allowed and done. Chats started from now on have it.".to_owned(),
                Some("needsAuth") | Some("authorizing") => {
                    "Allowed; the person is signing in to it in their browser.".to_owned()
                }
                Some(other) => format!("Allowed; the server is {other}."),
            },
        };
        let mut out = json!({ "request": request, "said": said });
        if let Some(server) = server {
            out["server"] = server;
        }
        Ok(out)
    }

    /// Whether an allowed request's server is still signing in.
    async fn signing_in(&self, request: &Value) -> bool {
        self.server_of(request)
            .await
            .and_then(|s| s["status"].as_str().map(str::to_owned))
            .is_some_and(|status| status == "needsAuth" || status == "authorizing")
    }

    /// The server an allowed request made or changed, as listed now.
    async fn server_of(&self, request: &Value) -> Option<Value> {
        let id = request["serverId"].as_str()?;
        let snap = self
            .zeron
            .call_rpc(methods::LIST_MCP_SERVERS, json!({}))
            .await
            .ok()?;
        snap["servers"]
            .as_array()?
            .iter()
            .find(|s| s["id"] == id)
            .map(|s| {
                json!({
                    "id": s["id"],
                    "name": s["name"],
                    "account": s["accountLabel"],
                    "agentName": s["agentName"],
                    "status": s["status"],
                })
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: Value) -> RequestArgs {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn requests_spell_the_engines_change() {
        assert_eq!(
            change_params(&args(json!({"action": "add", "preset": "linear", "account": "Work", "reason": "issues"}))).unwrap(),
            json!({"action": "add", "preset": "linear", "accountLabel": "Work", "reason": "issues"})
        );
        assert_eq!(
            change_params(&args(json!({"action": "add", "url": "https://x.test/mcp", "auth": "none", "name": "X"}))).unwrap(),
            json!({"action": "add", "url": "https://x.test/mcp", "authKind": "none", "name": "X"})
        );
        assert_eq!(
            change_params(&args(json!({"action": "disable", "server": "linear_work"}))).unwrap(),
            json!({"action": "setEnabled", "server": "linear_work", "enabled": false})
        );
        assert!(change_params(&args(json!({"action": "remove"}))).is_err());
        assert!(change_params(&args(json!({"action": "add", "url": "https://x", "auth": "bearer"}))).is_err());
        assert!(change_params(&args(json!({"action": "rename"}))).is_err());
        // A key is never something to pass.
        assert!(serde_json::from_value::<RequestArgs>(json!({"action": "add", "token": "sk"})).is_err());
    }

    #[test]
    fn catalog_names_match() {
        let defs = catalog();
        assert_eq!(defs.iter().map(|d| d.name).collect::<Vec<_>>(), NAMES);
    }
}
