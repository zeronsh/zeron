//! Managed MCP servers, for Settings → MCP: the engine's presets, configured
//! servers, Vault vs local storage status, and interactive OAuth flows.
//!
//! What scripts see:
//!   `mcp.servers`  { loaded, storage = "vault" | "local", vaultAvailable,
//!                    presets = { Preset }, servers = { Server } }
//!   `mcp.auth`     { [serverId] = { phase = "authorizing" | "error" | "done",
//!                                   flowId?, error? } }
//!
//! Actions (async):
//!   `mcp.refresh`          RefreshMcpServers
//!   `mcp.upsert`           UpsertMcpServer
//!   `mcp.set_enabled`      SetMcpServerEnabled
//!   `mcp.delete`           DeleteMcpServer
//!   `mcp.disconnect_auth`  DisconnectMcpAuth
//!   `mcp.cancel_auth`      CancelMcpAuth
//!   `mcp.start_auth`       StartMcpAuth + open browser + poll PollMcpAuth

use std::{
    cell::RefCell,
    collections::BTreeMap,
    rc::Rc,
    time::{Duration, Instant},
};

use serde_json::{Value as Json, json};
use zgx_script::{ScriptHost, Shared, Value};

use crate::auth::{open_sign_in, text};
use crate::engine::{self, rpc::Watch};
use crate::plugins::{json_value, value_json};

const AUTH_POLL_INTERVAL: Duration = Duration::from_secs(1);
const AUTH_POLL_TIMEOUT: Duration = Duration::from_secs(5 * 60);

#[derive(Clone, Debug)]
struct AuthFlow {
    generation: u64,
    phase: &'static str,
    flow_id: Option<String>,
    error: Option<String>,
}

struct Mcp {
    servers_shared: RefCell<Vec<Shared>>,
    auth_shared: RefCell<Vec<Shared>>,
    snapshot: RefCell<Value>,
    auth: RefCell<BTreeMap<String, AuthFlow>>,
    auth_generation: RefCell<u64>,
    watch: RefCell<Option<Watch>>,
}

thread_local! {
    static MCP: Rc<Mcp> = Rc::new(Mcp::new());
}

fn initial_snapshot() -> Value {
    json_value(&json!({
        "loaded": false,
        "storage": "local",
        "vaultAvailable": false,
        "presets": [],
        "servers": [],
    }))
}

impl Mcp {
    fn new() -> Self {
        Self {
            servers_shared: RefCell::default(),
            auth_shared: RefCell::default(),
            snapshot: RefCell::new(initial_snapshot()),
            auth: RefCell::default(),
            auth_generation: RefCell::new(0),
            watch: RefCell::default(),
        }
    }

    fn next_generation(&self) -> u64 {
        let mut g = self.auth_generation.borrow_mut();
        *g += 1;
        *g
    }

    fn take_snapshot(&self, mut item: Json) {
        if let Json::Object(ref mut map) = item {
            map.insert("loaded".to_string(), Json::Bool(true));
        }
        let value = json_value(&item);
        *self.snapshot.borrow_mut() = value.clone();
        for shared in self.servers_shared.borrow().iter() {
            shared.set(value.clone());
        }
    }

    fn auth_value(&self) -> Value {
        let auth = self.auth.borrow();
        let entries: Vec<(String, Value)> = auth
            .iter()
            .map(|(id, flow)| {
                let record = Value::record([
                    ("phase", Value::from(flow.phase)),
                    ("flowId", flow.flow_id.as_deref().map_or(Value::Nil, Value::from)),
                    ("error", flow.error.as_deref().map_or(Value::Nil, Value::from)),
                ]);
                (id.clone(), record)
            })
            .collect();
        Value::record(entries.iter().map(|(k, v)| (k.as_str(), v.clone())))
    }

    fn publish_auth(&self) {
        let value = self.auth_value();
        for shared in self.auth_shared.borrow().iter() {
            shared.set(value.clone());
        }
    }

    fn set_auth(&self, id: &str, flow: AuthFlow) {
        self.auth.borrow_mut().insert(id.to_owned(), flow);
        self.publish_auth();
    }

    fn clear_auth(&self, id: &str) -> Option<String> {
        let prev = self.auth.borrow_mut().remove(id);
        self.publish_auth();
        prev.and_then(|f| f.flow_id)
    }

    fn is_current_generation(&self, id: &str, generation: u64) -> bool {
        self.auth
            .borrow()
            .get(id)
            .is_some_and(|flow| flow.generation == generation)
    }

    fn watch_servers(self: &Rc<Self>, rpc: &crate::engine::rpc::Rpc) {
        let heard = Rc::downgrade(self);
        let watch = rpc.watch("WatchMcpServers", json!({}), move |item| {
            if let Some(mcp) = heard.upgrade() {
                mcp.take_snapshot(item);
            }
        });
        *self.watch.borrow_mut() = Some(watch);
    }

    fn spawn_poll(self: &Rc<Self>, id: String, flow_id: String, generation: u64) {
        let mcp = self.clone();
        engine::get().spawn(async move {
            let started = Instant::now();
            loop {
                zgx::timer::sleep(AUTH_POLL_INTERVAL).await;
                if !mcp.is_current_generation(&id, generation) {
                    return;
                }
                if started.elapsed() >= AUTH_POLL_TIMEOUT {
                    let _ = engine::get()
                        .call("CancelMcpAuth", json!({ "flowId": flow_id }))
                        .await;
                    if mcp.is_current_generation(&id, generation) {
                        mcp.set_auth(
                            &id,
                            AuthFlow {
                                generation,
                                phase: "error",
                                flow_id: Some(flow_id),
                                error: Some("Authorization timed out.".to_string()),
                            },
                        );
                    }
                    return;
                }

                match engine::get()
                    .call("PollMcpAuth", json!({ "flowId": flow_id }))
                    .await
                {
                    Ok(reply) => {
                        if !mcp.is_current_generation(&id, generation) {
                            return;
                        }
                        match reply["status"].as_str().unwrap_or("pending") {
                            "pending" => continue,
                            "completed" => {
                                mcp.set_auth(
                                    &id,
                                    AuthFlow {
                                        generation,
                                        phase: "done",
                                        flow_id: Some(flow_id),
                                        error: None,
                                    },
                                );
                                if let Ok(snapshot) =
                                    engine::get().call("RefreshMcpServers", json!({})).await
                                    && snapshot.is_object()
                                {
                                    mcp.take_snapshot(snapshot);
                                }
                                return;
                            }
                            "cancelled" => {
                                mcp.clear_auth(&id);
                                return;
                            }
                            "failed" => {
                                let message = reply["error"]
                                    .as_str()
                                    .filter(|s| !s.trim().is_empty())
                                    .unwrap_or("Authorization failed.")
                                    .to_string();
                                mcp.set_auth(
                                    &id,
                                    AuthFlow {
                                        generation,
                                        phase: "error",
                                        flow_id: Some(flow_id),
                                        error: Some(message),
                                    },
                                );
                                return;
                            }
                            other => {
                                mcp.set_auth(
                                    &id,
                                    AuthFlow {
                                        generation,
                                        phase: "error",
                                        flow_id: Some(flow_id),
                                        error: Some(format!("Unexpected authorization status: {other}")),
                                    },
                                );
                                return;
                            }
                        }
                    }
                    Err(error) => {
                        if !mcp.is_current_generation(&id, generation) {
                            return;
                        }
                        // Keep polling across transient reconnects until timeout;
                        // surface explicit engine errors.
                        if error == "closed" || error == "offline" {
                            continue;
                        }
                        mcp.set_auth(
                            &id,
                            AuthFlow {
                                generation,
                                phase: "error",
                                flow_id: Some(flow_id),
                                error: Some(error),
                            },
                        );
                        return;
                    }
                }
            }
        });
    }
}

/// Validates an MCP OAuth URL before opening it in the system browser:
/// allows `https://` and loopback `http://127.0.0.1` / `http://localhost`
/// (with optional port/path), rejecting control characters and other schemes.
fn openable_auth_url(url: &str) -> bool {
    if url.is_empty() || url.chars().any(char::is_control) {
        return false;
    }
    if url.starts_with("https://") {
        return url.len() > "https://".len();
    }
    for prefix in ["http://127.0.0.1", "http://localhost"] {
        if let Some(rest) = url.strip_prefix(prefix) {
            return rest.is_empty()
                || rest.starts_with(':')
                || rest.starts_with('/')
                || rest.starts_with('?');
        }
    }
    false
}

/// Normalizes empty Luau tables (`{}`) that serialize ambiguously between
/// JSON arrays and objects so `UpsertMcpServerRequest` always deserializes
/// cleanly.
fn normalize_upsert_body(mut body: Json) -> Json {
    if let Json::Object(ref mut map) = body {
        for key in ["args", "oauthScopes"] {
            if let Some(val) = map.get_mut(key)
                && val.as_object().is_some_and(|o| o.is_empty())
            {
                *val = Json::Array(Vec::new());
            }
        }
        for key in ["env", "headers"] {
            if let Some(val) = map.get_mut(key)
                && val.as_array().is_some_and(|a| a.is_empty())
            {
                *val = Json::Object(serde_json::Map::new());
            }
        }
    }
    body
}

fn string_list_field(args: &Value, key: &str) -> Vec<String> {
    match args.field(key) {
        Some(Value::List(items)) => items
            .iter()
            .map(text)
            .filter(|s| !s.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

pub fn install(host: &ScriptHost) {
    let mcp = MCP.with(Rc::clone);
    mcp.servers_shared
        .borrow_mut()
        .push(host.shared("mcp.servers", mcp.snapshot.borrow().clone()));
    mcp.auth_shared
        .borrow_mut()
        .push(host.shared("mcp.auth", mcp.auth_value()));

    if mcp.servers_shared.borrow().len() == 1 {
        let weak = Rc::downgrade(&mcp);
        engine::get().on_connect(move |rpc| {
            if let Some(mcp) = weak.upgrade() {
                mcp.watch_servers(rpc);
            }
        });
    }

    let m = mcp.clone();
    host.async_action("mcp.refresh", move |_| {
        let m = m.clone();
        async move {
            let reply = engine::get().call("RefreshMcpServers", json!({})).await?;
            if reply.is_object() {
                m.take_snapshot(reply.clone());
            }
            Ok(json_value(&reply))
        }
    });

    let m = mcp.clone();
    host.async_action("mcp.upsert", move |args| {
        let m = m.clone();
        let params = normalize_upsert_body(value_json(&args));
        async move {
            crate::plugins::preview::refuse("mcp.upsert")?;
            let reply = engine::get().call("UpsertMcpServer", params).await?;
            if reply.is_object() {
                m.take_snapshot(reply.clone());
            }
            Ok(json_value(&reply))
        }
    });

    let m = mcp.clone();
    host.async_action("mcp.set_enabled", move |args| {
        let m = m.clone();
        let id = args.field("id").map(text).unwrap_or_default();
        let enabled = matches!(args.field("enabled"), Some(Value::Bool(true)));
        async move {
            crate::plugins::preview::refuse("mcp.set_enabled")?;
            let reply = engine::get()
                .call(
                    "SetMcpServerEnabled",
                    json!({ "id": id, "enabled": enabled }),
                )
                .await?;
            if reply.is_object() {
                m.take_snapshot(reply.clone());
            }
            Ok(json_value(&reply))
        }
    });

    let m = mcp.clone();
    host.async_action("mcp.delete", move |args| {
        let m = m.clone();
        let id = match args.field("id") {
            Some(v) => text(&v),
            None => text(&args),
        };
        async move {
            crate::plugins::preview::refuse("mcp.delete")?;
            let reply = engine::get()
                .call("DeleteMcpServer", json!({ "id": id }))
                .await?;
            m.clear_auth(&id);
            if reply.is_object() {
                m.take_snapshot(reply.clone());
            }
            Ok(json_value(&reply))
        }
    });

    let m = mcp.clone();
    host.async_action("mcp.disconnect_auth", move |args| {
        let m = m.clone();
        let id = match args.field("id") {
            Some(v) => text(&v),
            None => text(&args),
        };
        async move {
            crate::plugins::preview::refuse("mcp.disconnect_auth")?;
            let reply = engine::get()
                .call("DisconnectMcpAuth", json!({ "id": id }))
                .await?;
            m.clear_auth(&id);
            if reply.is_object() {
                m.take_snapshot(reply.clone());
            }
            Ok(json_value(&reply))
        }
    });

    let m = mcp.clone();
    host.async_action("mcp.cancel_auth", move |args| {
        let m = m.clone();
        let id = args.field("id").map(text).unwrap_or_default();
        let flow_id = args
            .field("flowId")
            .or_else(|| args.field("flow_id"))
            .map(text)
            .filter(|s| !s.is_empty());
        async move {
            crate::plugins::preview::refuse("mcp.cancel_auth")?;
            let stored_flow = if !id.is_empty() {
                m.clear_auth(&id)
            } else {
                None
            };
            let target_flow = flow_id.or(stored_flow);
            if let Some(flow_id) = target_flow {
                let _ = engine::get()
                    .call("CancelMcpAuth", json!({ "flowId": flow_id }))
                    .await;
            }
            Ok(Value::Nil)
        }
    });

    let m = mcp;
    host.async_action("mcp.start_auth", move |args| {
        let m = m.clone();
        let id = match args.field("id") {
            Some(v) => text(&v),
            None => text(&args),
        };
        let client_id = args
            .field("clientId")
            .or_else(|| args.field("client_id"))
            .map(text)
            .filter(|s| !s.is_empty());
        let client_secret = args
            .field("clientSecret")
            .or_else(|| args.field("client_secret"))
            .map(text)
            .filter(|s| !s.is_empty());
        let scopes = string_list_field(&args, "scopes");

        async move {
            crate::plugins::preview::refuse("mcp.start_auth")?;
            let mut params = serde_json::Map::new();
            params.insert("id".to_string(), json!(id));
            if let Some(cid) = client_id {
                params.insert("clientId".to_string(), json!(cid));
            }
            if let Some(csec) = client_secret {
                params.insert("clientSecret".to_string(), json!(csec));
            }
            if !scopes.is_empty() {
                params.insert("scopes".to_string(), json!(scopes));
            }

            let generation = m.next_generation();
            m.set_auth(
                &id,
                AuthFlow {
                    generation,
                    phase: "authorizing",
                    flow_id: None,
                    error: None,
                },
            );

            let reply = match engine::get()
                .call("StartMcpAuth", Json::Object(params))
                .await
            {
                Ok(reply) => reply,
                Err(error) => {
                    if m.is_current_generation(&id, generation) {
                        m.set_auth(
                            &id,
                            AuthFlow {
                                generation,
                                phase: "error",
                                flow_id: None,
                                error: Some(error.clone()),
                            },
                        );
                    }
                    return Err(error);
                }
            };

            let flow_id = reply["flowId"].as_str().unwrap_or_default().to_string();
            let auth_url = reply["authUrl"].as_str().unwrap_or_default().to_string();

            if !m.is_current_generation(&id, generation) {
                if !flow_id.is_empty() {
                    let _ = engine::get()
                        .call("CancelMcpAuth", json!({ "flowId": flow_id }))
                        .await;
                }
                return Err("cancelled".to_string());
            }

            if !openable_auth_url(&auth_url) {
                if !flow_id.is_empty() {
                    let _ = engine::get()
                        .call("CancelMcpAuth", json!({ "flowId": flow_id }))
                        .await;
                }
                let msg = "Refused to open an invalid authorization URL.".to_string();
                m.set_auth(
                    &id,
                    AuthFlow {
                        generation,
                        phase: "error",
                        flow_id: None,
                        error: Some(msg.clone()),
                    },
                );
                return Err(msg);
            }

            if let Err(error) = open_sign_in(&auth_url) {
                eprintln!("zeron-zgx: mcp: the browser didn't open: {error}");
                if !flow_id.is_empty() {
                    let _ = engine::get()
                        .call("CancelMcpAuth", json!({ "flowId": flow_id }))
                        .await;
                }
                let msg = format!("Could not open browser for sign-in: {error}");
                m.set_auth(
                    &id,
                    AuthFlow {
                        generation,
                        phase: "error",
                        flow_id: None,
                        error: Some(msg.clone()),
                    },
                );
                return Err(msg);
            }

            m.set_auth(
                &id,
                AuthFlow {
                    generation,
                    phase: "authorizing",
                    flow_id: Some(flow_id.clone()),
                    error: None,
                },
            );
            m.spawn_poll(id, flow_id, generation);
            Ok(json_value(&reply))
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_openable_auth_urls() {
        assert!(openable_auth_url("https://mcp.linear.app/authorize?client_id=abc"));
        assert!(openable_auth_url("http://127.0.0.1:4321/authorize"));
        assert!(openable_auth_url("http://localhost:8080/oauth"));
        assert!(openable_auth_url("http://127.0.0.1/oauth"));
        assert!(openable_auth_url("http://localhost/oauth"));

        assert!(!openable_auth_url(""));
        assert!(!openable_auth_url("https://"));
        assert!(!openable_auth_url("http://example.com/oauth"));
        assert!(!openable_auth_url("http://localhost.evil.com/oauth"));
        assert!(!openable_auth_url("http://127.0.0.1.evil.com/oauth"));
        assert!(!openable_auth_url("javascript:alert(1)"));
        assert!(!openable_auth_url("file:///etc/passwd"));
        assert!(!openable_auth_url("https://example.com/\nfoo"));
    }

    #[test]
    fn normalizes_empty_luau_tables_for_upsert() {
        let raw = json!({
            "name": "Demo",
            "transport": "stdio",
            "args": {},
            "oauthScopes": {},
            "env": [],
            "headers": [],
        });
        let normalized = normalize_upsert_body(raw);
        assert_eq!(normalized["args"], json!([]));
        assert_eq!(normalized["oauthScopes"], json!([]));
        assert_eq!(normalized["env"], json!({}));
        assert_eq!(normalized["headers"], json!({}));
    }

    #[test]
    fn settings_mcp_page_renders_servers_presets_and_custom_form() {
        use std::path::PathBuf;
        use zgx_script::testing::TestApp;

        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui/shell_fixture.luau");
        let snapshot = json!({
            "loaded": true,
            "storage": "vault",
            "vaultAvailable": true,
            "presets": [
                {
                    "id": "linear",
                    "name": "Linear",
                    "description": "Issues, projects, and cycles.",
                    "category": "Project Management",
                    "url": "https://mcp.linear.app/mcp",
                    "transport": "http",
                    "authKind": "oauth",
                    "docsUrl": "https://linear.app/docs/mcp",
                    "icon": "linear",
                },
                {
                    "id": "slack",
                    "name": "Slack",
                    "description": "Search channels and post messages.",
                    "category": "Communication",
                    "url": "https://mcp.slack.com/mcp",
                    "transport": "http",
                    "authKind": "oauth",
                    "docsUrl": "https://docs.slack.dev/ai/slack-mcp-server/",
                    "icon": "slack",
                    "clientRequired": true,
                },
                {
                    "id": "context7",
                    "name": "Context7",
                    "description": "Up-to-date library documentation.",
                    "category": "Docs",
                    "url": "https://mcp.context7.com/mcp",
                    "transport": "http",
                    "authKind": "none",
                    "docsUrl": "https://context7.com",
                    "icon": "context7",
                }
            ],
            "servers": [
                {
                    "id": "linear",
                    "name": "Linear",
                    "preset": "linear",
                    "transport": "http",
                    "url": "https://mcp.linear.app/mcp",
                    "args": [],
                    "envKeys": [],
                    "authKind": "oauth",
                    "headerKeys": [],
                    "enabled": true,
                    "storage": "vault",
                    "status": "connected",
                    "accountLabel": "ada@zeron.sh",
                    "scopes": ["read", "write"],
                    "updatedAt": 1700000000,
                },
                {
                    "id": "local-fs",
                    "name": "Local Files",
                    "transport": "stdio",
                    "command": "npx",
                    "args": ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"],
                    "envKeys": ["DEBUG"],
                    "authKind": "none",
                    "headerKeys": [],
                    "enabled": false,
                    "storage": "local",
                    "status": "disabled",
                    "scopes": [],
                    "updatedAt": 1700000001,
                }
            ],
            "clientRedirectUri": "http://localhost:38417/callback",
        });
        let snapshot_for_host = snapshot.clone();
        let mut app = TestApp::new(move |runtime| {
            let host = crate::plugins::lend_design_system(ScriptHost::new(runtime, path.clone()), &path);
            host.shared("zgx.window.size", zgx_script::window_size_value(1200., 800.));
            host.shared("mcp.servers", json_value(&snapshot_for_host));
            host.shared("mcp.auth", json_value(&json!({})));
            for name in [
                "mcp.refresh",
                "mcp.upsert",
                "mcp.set_enabled",
                "mcp.delete",
                "mcp.start_auth",
                "mcp.cancel_auth",
                "mcp.disconnect_auth",
            ] {
                host.async_action(name, |_| async { Ok(Value::Nil) });
            }
            host
        });
        assert_eq!(app.error(), None);
        app.ui.scene.borrow_mut().resize(1200., 800.);
        app.frames(20);
        app.host.run_command("shell.settings");
        app.frames(20);
        app.click("#settings-nav >> role:tab \"MCP\"");
        app.frames(20);
        assert_eq!(app.error(), None);

        let texts = app.texts();
        assert!(texts.iter().any(|t| t == "Your servers"), "missing servers section: {texts:?}");
        assert!(texts.iter().any(|t| t == "Linear"), "missing configured server Linear: {texts:?}");
        assert!(texts.iter().any(|t| t == "ada@zeron.sh"), "missing account label: {texts:?}");
        assert!(texts.iter().any(|t| t == "Local Files"), "missing stdio server: {texts:?}");
        assert!(texts.iter().any(|t| t == "Context7"), "missing preset Context7: {texts:?}");
        // No storage banner: the page's line under its title says it.
        assert!(!texts.iter().any(|t| t == "Zeron Vault"), "storage banner is back: {texts:?}");
        // Linear is added, so it's not offered again.
        assert_eq!(texts.iter().filter(|t| *t == "Linear").count(), 1, "{texts:?}");

        // Slack has no client registration: Connect asks for its app.
        app.click("#settings-page >> role:button \"Connect\"[1]");
        app.frames(20);
        assert_eq!(app.error(), None);
        let slack_texts = app.texts();
        assert!(slack_texts.iter().any(|t| t == "Connect Slack"), "missing Slack dialog: {slack_texts:?}");
        assert!(
            slack_texts.iter().any(|t| t == "http://localhost:38417/callback"),
            "missing redirect URL: {slack_texts:?}"
        );
        app.keys("Escape");
        app.frames(20);

        app.click("#settings-page >> role:button \"Custom server\"");
        app.frames(20);
        assert_eq!(app.error(), None);
        let form_texts = app.texts();
        assert!(form_texts.iter().any(|t| t == "Add server"), "missing custom server dialog: {form_texts:?}");
        assert!(form_texts.iter().any(|t| t == "Remote URL"), "missing server type choice: {form_texts:?}");
    }
}
