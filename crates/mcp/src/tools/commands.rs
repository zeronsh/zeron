//! `run_command`: one shell command on this device or any other of the
//! person's online devices, run to its end like an agent's own shell tool
//! (the engine's `RunCommand`, sent to another device by `targetDeviceId`).
//! Nothing is kept here between calls: the device that runs it caps the
//! output and kills what the command started when it times out
//! (crates/engine/src/run_command.rs). The engine allows it only from a chat
//! with full access and no sandbox (rpc/agent_gate.rs).

use serde::Deserialize;
use serde_json::{Value, json};
use zeron_rpc::{RpcError, methods};

use super::{ToolDef, Tools};

pub(super) const RUN_COMMAND: &str = "run_command";
const DEFAULT_TIMEOUT_SECS: u64 = 120;
const MAX_TIMEOUT_SECS: u64 = 600;

pub(super) fn tool() -> ToolDef {
    ToolDef {
        name: RUN_COMMAND,
        description: "Run a shell command on this device, or on any other of the person's online devices (see list_devices; a Cloud machine must be awake), and get its output and exit code, as your own shell tool does here. It runs in the user's login shell (PowerShell on Windows) with no input and no terminal, in `cwd` (default: your chat's folder when it runs on your chat's device, else home). Stdout and stderr come back together; long output keeps its start and end. A command still running at `timeout_secs` is killed with everything it started (timedOut). Each call is a fresh shell: `cd` and variables don't carry over, so chain steps with && or pass cwd. Something left in the background with `nohup … &` keeps running. Only a chat with full access may use this.",
        input_schema: json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "The command, as you'd type it in a terminal there." },
                "device": { "type": "string", "description": "Device id or exact name. Omit for this device." },
                "cwd": { "type": "string", "description": "Folder on that device to run it in." },
                "timeout_secs": { "type": "integer", "minimum": 1, "maximum": MAX_TIMEOUT_SECS, "default": DEFAULT_TIMEOUT_SECS }
            },
            "required": ["command"]
        }),
    }
}

#[derive(Deserialize)]
pub(super) struct RunArgs {
    command: String,
    device: Option<String>,
    cwd: Option<String>,
    timeout_secs: Option<u64>,
}

impl Tools {
    pub(super) async fn run_command(&self, args: RunArgs) -> anyhow::Result<Value> {
        anyhow::ensure!(!args.command.trim().is_empty(), "command is empty");
        let (devices, local) = tokio::try_join!(self.zeron.devices(), self.zeron.local_device_id())?;
        let device = self.zeron.resolve_device_id(args.device.as_deref()).await?;
        let timeout = args.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS).clamp(1, MAX_TIMEOUT_SECS);
        let mut params = json!({ "command": args.command, "timeoutMs": timeout * 1000 });
        if let Some(cwd) = args.cwd.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
            params["cwd"] = json!(cwd);
        }
        if let Some(chat) = &self.zeron.origin().chat_id {
            params["chatId"] = json!(chat);
        }
        if device != local {
            params["targetDeviceId"] = json!(device);
        }
        let mut outcome = match self.zeron.call_rpc(methods::RUN_COMMAND, params).await {
            Ok(outcome) => outcome,
            Err(RpcError::UnknownMethod(_)) => anyhow::bail!(
                "That device's Zeron is too old to run commands: update Zeron there and restart it"
            ),
            Err(RpcError::Failed(message)) => anyhow::bail!(message),
            Err(error) => anyhow::bail!("{error}"),
        };
        let name = devices.iter().find(|d| d.id == device).map(|d| d.name.clone());
        outcome["device"] = json!({ "id": device, "name": name, "local": device == local });
        Ok(outcome)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::zeron::{Origin, Zeron};

    /// Two devices; `RunCommand` answers with what it was asked.
    struct Engine {
        calls: std::sync::Mutex<Vec<Value>>,
        old_remote: bool,
    }

    #[async_trait::async_trait]
    impl zeron_rpc::RpcService for Engine {
        async fn handle(&self, method: &str, params: Value) -> Result<zeron_rpc::RpcReply, RpcError> {
            use futures::StreamExt;
            use zeron_rpc::RpcReply;
            Ok(match method {
                methods::LOCAL_DEVICE => RpcReply::Value(json!({ "deviceId": "dev-local" })),
                methods::WATCH_DEVICES => RpcReply::Stream(
                    futures::stream::iter(vec![json!([
                        { "id": "dev-local", "name": "Laptop", "platform": "linux", "lastSeenAt": null },
                        { "id": "dev-remote", "name": "Worker", "platform": "linux", "lastSeenAt": null }
                    ])])
                    .boxed(),
                ),
                methods::RUN_COMMAND => {
                    if self.old_remote && params["targetDeviceId"] == "dev-remote" {
                        return Err(RpcError::UnknownMethod(method.into()));
                    }
                    self.calls.lock().unwrap().push(params);
                    RpcReply::Value(json!({ "exitCode": 0, "output": "hi\n", "timedOut": false, "durationMs": 5 }))
                }
                other => return Err(RpcError::UnknownMethod(other.into())),
            })
        }
    }

    fn tools(engine: Arc<Engine>) -> Tools {
        let origin = Origin { chat_id: Some("chat-1".into()), ..Default::default() };
        Tools::new(Arc::new(Zeron::with_client(zeron_rpc::memory_client(engine), origin)))
    }

    #[tokio::test]
    async fn a_command_runs_on_the_device_it_names() {
        let engine = Arc::new(Engine { calls: Default::default(), old_remote: false });
        let tools = tools(engine.clone());
        let ran = tools
            .call(RUN_COMMAND, json!({ "command": "echo hi", "device": "Worker", "cwd": "/srv", "timeout_secs": 5 }))
            .await
            .unwrap();
        assert_eq!(ran["output"], "hi\n");
        assert_eq!(ran["exitCode"], 0);
        assert_eq!(ran["device"], json!({ "id": "dev-remote", "name": "Worker", "local": false }));
        let sent = engine.calls.lock().unwrap().last().cloned().unwrap();
        assert_eq!(
            sent,
            json!({ "command": "echo hi", "cwd": "/srv", "timeoutMs": 5000, "chatId": "chat-1", "targetDeviceId": "dev-remote" })
        );

        // This device: no target; a timeout is bounded.
        tools.call(RUN_COMMAND, json!({ "command": "ls", "timeout_secs": 100000 })).await.unwrap();
        let sent = engine.calls.lock().unwrap().last().cloned().unwrap();
        assert!(sent.get("targetDeviceId").is_none());
        assert_eq!(sent["timeoutMs"], MAX_TIMEOUT_SECS * 1000);

        assert!(tools.call(RUN_COMMAND, json!({ "command": " " })).await.unwrap_err().contains("empty"));
    }

    #[tokio::test]
    async fn an_older_device_says_so() {
        let tools = tools(Arc::new(Engine { calls: Default::default(), old_remote: true }));
        let error = tools.call(RUN_COMMAND, json!({ "command": "ls", "device": "dev-remote" })).await.unwrap_err();
        assert!(error.contains("too old"), "{error}");
    }
}
