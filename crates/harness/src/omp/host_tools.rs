//! Zeron's MCP servers (its own and the plugins'), offered to omp as RPC
//! host tools (`set_host_tools`): the RPC's own way for a host to give the
//! agent tools of its own. omp has no per-run MCP flag (its MCP servers come
//! from config files), and a host tool runs in this process: the run speaks
//! MCP to each server over stdio and answers omp's `host_tool_call`s with
//! the server's `tools/call` results. Nothing in the user's config changes.
//!
//! Zeron's own server's tools keep the names the Pi driver gives them
//! (`zeron_<tool>`); a plugin server's are `<server>_<tool>`. A server that
//! cannot start is left out with a warning: it must not fail the turn.
use crate::{
    HarnessError,
    jsonrpc::{Incoming, RpcClient},
    pi::rpc::Client,
    process::{Command, Stdio, owned::Child},
};
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::task::{AbortHandle, JoinSet};
use zeron_proto::McpServer;

const START_TIMEOUT: Duration = Duration::from_secs(20);
/// `wait_for_turn` may legitimately wait several minutes.
const CALL_TIMEOUT: Duration = Duration::from_secs(660);

struct Server {
    client: RpcClient,
    _child: Child,
    _drain: tokio::task::JoinHandle<()>,
}

#[derive(Default)]
pub(super) struct HostTools {
    servers: Vec<Arc<Server>>,
    /// omp's name for a tool: its server (index), server name and MCP name.
    tools: HashMap<String, (usize, String, String)>,
    /// Calls in flight, by omp's host-call id.
    calls: HashMap<String, AbortHandle>,
    tasks: JoinSet<()>,
}

impl HostTools {
    /// Start every server and list its tools. Returns the tools and the
    /// `set_host_tools` definitions for omp (none when there are no servers).
    pub async fn start(
        mcp: Option<&McpServer>,
        extras: &[McpServer],
        env: &std::collections::BTreeMap<String, String>,
    ) -> (Self, Vec<Value>) {
        let mut tools = Self::default();
        let mut definitions = Vec::new();
        for config in mcp.into_iter().chain(extras) {
            match tokio::time::timeout(START_TIMEOUT, start(config, env)).await {
                Ok(Ok((server, listed))) => {
                    let index = tools.servers.len();
                    tools.servers.push(Arc::new(server));
                    for tool in listed {
                        let Some(mcp_name) = tool["name"].as_str().filter(|n| !n.is_empty()) else {
                            continue;
                        };
                        let name = host_name(&config.name, mcp_name);
                        if tools.tools.contains_key(&name) {
                            continue;
                        }
                        let mut parameters = tool["inputSchema"].clone();
                        if !parameters.is_object() {
                            parameters = json!({"type":"object","properties":{}});
                        }
                        definitions.push(json!({
                            "name": name,
                            "label": format!("Zeron: {mcp_name}"),
                            "description": tool["description"].as_str().unwrap_or(mcp_name),
                            "parameters": parameters,
                            "loadMode": "essential",
                        }));
                        tools
                            .tools
                            .insert(name, (index, config.name.clone(), mcp_name.to_owned()));
                    }
                }
                Ok(Err(error)) => tracing::warn!(
                    target: "zeron_harness::omp",
                    server = %config.name, %error,
                    "MCP server could not start; omp runs without its tools"
                ),
                Err(_) => tracing::warn!(
                    target: "zeron_harness::omp",
                    server = %config.name,
                    "MCP server did not start in time; omp runs without its tools"
                ),
            }
        }
        (tools, definitions)
    }

    /// omp's name for each tool, with the server and MCP tool it stands for.
    pub fn names(&self) -> HashMap<String, (String, String)> {
        self.tools
            .iter()
            .map(|(name, (_, server, tool))| (name.clone(), (server.clone(), tool.clone())))
            .collect()
    }

    /// Run a `host_tool_call` and answer it with `host_tool_result`.
    pub fn call(&mut self, omp: Client, frame: &Value) {
        while self.tasks.try_join_next().is_some() {}
        let Some(id) = frame["id"].as_str().map(str::to_owned) else {
            return;
        };
        let name = frame["toolName"].as_str().unwrap_or("");
        let Some((index, _, tool)) = self.tools.get(name).cloned() else {
            let _ = omp.send(failure(&id, &format!("unknown host tool: {name}")));
            return;
        };
        let server = Arc::clone(&self.servers[index]);
        let arguments = frame["arguments"].clone();
        let key = id.clone();
        let task = self.tasks.spawn(async move {
            let call = server
                .client
                .request("tools/call", json!({"name": tool, "arguments": arguments}));
            let reply = match tokio::time::timeout(CALL_TIMEOUT, call).await {
                Ok(Ok(result)) => result_frame(&key, &result),
                Ok(Err(error)) => failure(&key, &error.to_string()),
                Err(_) => failure(&key, &format!("Zeron MCP {tool} timed out")),
            };
            let _ = omp.send(reply);
        });
        self.calls.retain(|_, task| !task.is_finished());
        self.calls.insert(id, task);
    }

    /// omp gave up on a call (`host_tool_cancel`): stop waiting for it.
    pub fn cancel(&mut self, frame: &Value) {
        if let Some(task) = frame["targetId"]
            .as_str()
            .and_then(|id| self.calls.remove(id))
        {
            task.abort();
        }
    }
}

/// omp's name for an MCP tool: `zeron_<tool>` for Zeron's server (as the Pi
/// driver names them), `<server>_<tool>` otherwise, in the characters tool
/// names allow and at most 64 of them.
fn host_name(server: &str, tool: &str) -> String {
    let name: String = format!("{server}_{tool}")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    name.chars().take(64).collect()
}

fn failure(id: &str, message: &str) -> Value {
    json!({"type":"host_tool_result","id":id,"isError":true,
           "result":{"content":[{"type":"text","text":message}]}})
}

/// An MCP `tools/call` result as omp's tool result: text and images pass
/// through, other content as its JSON text; `isError` rejects the call.
fn result_frame(id: &str, result: &Value) -> Value {
    let content: Vec<Value> = result["content"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|item| match item["type"].as_str() {
            Some("text") => json!({"type":"text","text":item["text"].as_str().unwrap_or("")}),
            Some("image") if item["data"].is_string() => json!({
                "type":"image","data":item["data"],
                "mimeType":item["mimeType"].as_str().unwrap_or("image/png")}),
            _ => json!({"type":"text","text":item.to_string()}),
        })
        .collect();
    let mut frame = json!({"type":"host_tool_result","id":id,"result":{"content":content}});
    if let Some(details) = result.get("structuredContent").filter(|v| !v.is_null()) {
        frame["result"]["details"] = details.clone();
    }
    if result["isError"] == true {
        frame["isError"] = json!(true);
    }
    frame
}

async fn start(
    config: &McpServer,
    env: &std::collections::BTreeMap<String, String>,
) -> Result<(Server, Vec<Value>), HarnessError> {
    let mut cmd = Command::new(&config.command);
    crate::process::owned::configure(&mut cmd);
    crate::apply_run_env(&mut cmd, env);
    for (key, value) in &config.env {
        cmd.env(key, value);
    }
    cmd.args(&config.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let mut child = Child::new(cmd.spawn()?);
    let (client, mut incoming) = RpcClient::new(
        child.stdin.take().expect("piped stdin"),
        child.stdout.take().expect("piped stdout"),
    );
    // Server requests (sampling, roots) are refused; notifications dropped.
    let responder = client.clone();
    let drain = tokio::spawn(async move {
        while let Some(message) = incoming.recv().await {
            match message {
                Incoming::Request { id, .. } => {
                    responder.respond_error(&id, -32601, "not supported")
                }
                Incoming::Eof => break,
                Incoming::Notification { .. } => {}
            }
        }
    });
    client
        .request(
            "initialize",
            json!({"protocolVersion":"2024-11-05","capabilities":{},
                   "clientInfo":{"name":"zeron-omp","version":"1"}}),
        )
        .await?;
    client.notify("notifications/initialized", None);
    let listed = client.request("tools/list", json!({})).await?;
    let tools = listed["tools"].as_array().cloned().unwrap_or_default();
    Ok((
        Server {
            client,
            _child: child,
            _drain: drain,
        },
        tools,
    ))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn names_follow_the_pi_driver_and_stay_within_tool_name_rules() {
        assert_eq!(host_name("zeron", "send_message"), "zeron_send_message");
        assert_eq!(host_name("plugin-focus", "a.b c"), "plugin-focus_a_b_c");
        assert_eq!(host_name("s", &"x".repeat(100)).len(), 64);
    }

    #[test]
    fn results_keep_text_images_details_and_errors() {
        let frame = result_frame(
            "h1",
            &json!({
            "content":[{"type":"text","text":"ok"},{"type":"image","data":"AAAA","mimeType":"image/jpeg"},{"type":"resource","uri":"x"}],
            "structuredContent":{"n":1},"isError":true}),
        );
        assert_eq!(frame["id"], "h1");
        assert_eq!(frame["isError"], true);
        assert_eq!(
            frame["result"]["content"][0],
            json!({"type":"text","text":"ok"})
        );
        assert_eq!(frame["result"]["content"][1]["mimeType"], "image/jpeg");
        assert!(
            frame["result"]["content"][2]["text"]
                .as_str()
                .unwrap()
                .contains("resource")
        );
        assert_eq!(frame["result"]["details"], json!({"n":1}));
    }

    /// A stdio MCP server (a shell loop answering by request) proxied end to
    /// end: listed as host tools, called, and refused for an unknown name.
    #[tokio::test]
    async fn a_stdio_server_is_listed_and_called() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("server.py");
        std::fs::write(
            &script,
            r#"import json, sys
for line in sys.stdin:
    m = json.loads(line)
    if "id" not in m: continue
    r = {}
    if m["method"] == "initialize": r = {"protocolVersion": "2024-11-05", "capabilities": {"tools": {}}}
    elif m["method"] == "tools/list": r = {"tools": [{"name": "echo", "description": "Echo", "inputSchema": {"type": "object"}}]}
    elif m["method"] == "tools/call": r = {"content": [{"type": "text", "text": "echo:" + m["params"]["arguments"]["text"]}]}
    print(json.dumps({"jsonrpc": "2.0", "id": m["id"], "result": r}), flush=True)
"#,
        )
        .unwrap();
        let server = McpServer {
            name: "zeron".into(),
            command: "python3".into(),
            args: vec!["-I".into(), script.display().to_string()],
            env: Default::default(),
        };
        let broken = McpServer {
            name: "broken".into(),
            command: dir.path().join("missing").display().to_string(),
            args: vec![],
            env: Default::default(),
        };
        let (mut tools, definitions) = HostTools::start(
            Some(&server),
            std::slice::from_ref(&broken),
            &Default::default(),
        )
        .await;
        assert_eq!(definitions.len(), 1);
        assert_eq!(definitions[0]["name"], "zeron_echo");
        assert_eq!(definitions[0]["loadMode"], "essential");
        assert_eq!(
            tools.names()["zeron_echo"],
            ("zeron".to_owned(), "echo".to_owned())
        );
        let (writer, peer) = tokio::io::duplex(1 << 16);
        let (_peer_writer, reader) = tokio::io::duplex(1 << 16);
        let transport = crate::pi::rpc::Transport::named(writer, reader, "omp");
        tools.call(
            transport.client.clone(),
            &json!({"type":"host_tool_call","id":"h1","toolCallId":"t","toolName":"zeron_echo","arguments":{"text":"hi"}}),
        );
        tools.call(
            transport.client.clone(),
            &json!({"type":"host_tool_call","id":"h2","toolCallId":"u","toolName":"nope","arguments":{}}),
        );
        use tokio::io::AsyncBufReadExt;
        let mut lines = tokio::io::BufReader::new(peer).lines();
        let mut replies = HashMap::new();
        for _ in 0..2 {
            let line = tokio::time::timeout(Duration::from_secs(10), lines.next_line())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            let reply: Value = serde_json::from_str(&line).unwrap();
            replies.insert(reply["id"].as_str().unwrap().to_owned(), reply);
        }
        assert_eq!(replies["h1"]["result"]["content"][0]["text"], "echo:hi");
        assert!(replies["h1"].get("isError").is_none());
        assert_eq!(replies["h2"]["isError"], true);
    }
}
