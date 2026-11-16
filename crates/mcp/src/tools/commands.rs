//! Shell commands on this device or any other of the person's online
//! devices, as an agent's own shell tool runs them on its machine: run one to
//! its end (`run_command`), or leave one running in the background and read
//! (`command_output`), stop (`stop_command`) or list (`list_commands`) it.
//! The engine's `RunCommand` & co., sent to another device by
//! `targetDeviceId`. The device that runs a command bounds it: its output,
//! its time, and everything it started, which ends at a timeout, a stop, or
//! when this chat is archived or deleted (crates/engine/src/run_command.rs).
//! The engine allows it only from a chat with full access and no sandbox
//! (rpc/agent_gate.rs).
//!
//! Kept here: each device's folder after the last command, so `cd` carries
//! over to the next command there, as in a shell (one short path per device).
//! A background command's id names its device (`cmd-…@device`), so nothing
//! else is kept.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::{Value, json};
use zeron_rpc::{RpcError, methods};

use super::{ToolDef, Tools};

pub(super) const RUN_COMMAND: &str = "run_command";
pub(super) const COMMAND_OUTPUT: &str = "command_output";
pub(super) const STOP_COMMAND: &str = "stop_command";
pub(super) const LIST_COMMANDS: &str = "list_commands";
pub(super) const NAMES: [&str; 4] = [RUN_COMMAND, COMMAND_OUTPUT, STOP_COMMAND, LIST_COMMANDS];

const DEFAULT_TIMEOUT_SECS: u64 = 120;
const MAX_TIMEOUT_SECS: u64 = 600;

/// Each device's folder after this chat's last command there.
#[derive(Default)]
pub(super) struct CommandState {
    cwd: tokio::sync::Mutex<HashMap<String, String>>,
}

fn device_schema() -> Value {
    json!({ "type": "string", "description": "Device id or exact name (see list_devices). Omit for this device." })
}

fn id_schema() -> Value {
    json!({ "type": "string", "description": "The command's id, from run_command with background (it names its device)." })
}

pub(super) fn catalog() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: RUN_COMMAND,
            description: "Run a shell command on this device or on any other of the person's online devices (see list_devices; a Cloud machine must be awake), exactly as you would with your own shell tool: builds, tests, git, package managers, scripts, servers. It runs in the user's shell there (PowerShell on Windows) with the PATH and variables their login shell sets, and no input or terminal. The working directory carries over between commands on the same device (`cd` sticks; environment variables don't); the first command on a device starts in `cwd`, or your chat's folder on its own device, or home. Returns `output` (stdout and stderr together; long output keeps its start and end), `exitCode`, and `timedOut`: a command still running at `timeout_secs` (default 120, at most 600) is killed with everything it started, and anything a command leaves running (`server &`) ends with it. For something that keeps running — a dev server, a watcher, a long build — pass `background: true`: it returns an `id` at once; read what it prints with command_output and stop it with stop_command. Background commands stop on their own when this chat is archived or deleted. Only a chat with full access may use this.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "The command, as you'd type it in a terminal there." },
                    "device": device_schema(),
                    "cwd": { "type": "string", "description": "Folder on that device to run it in (sets where later commands there start, too)." },
                    "timeout_secs": { "type": "integer", "minimum": 1, "maximum": MAX_TIMEOUT_SECS, "default": DEFAULT_TIMEOUT_SECS },
                    "background": { "type": "boolean", "default": false, "description": "Leave it running and return its id at once." }
                },
                "required": ["command"]
            }),
        },
        ToolDef {
            name: COMMAND_OUTPUT,
            description: "What a background command printed since you last asked (its newest 48 KiB; `omittedBytes` counts what's gone), and whether it's `running`, `exited` (with `exitCode`) or `stopped`. To wait for something (a server to be ready), run_command `sleep 2` and ask again.",
            input_schema: json!({ "type": "object", "properties": { "id": id_schema() }, "required": ["id"] }),
        },
        ToolDef {
            name: STOP_COMMAND,
            description: "Stop a background command and everything it started; returns what it printed since you last asked.",
            input_schema: json!({ "type": "object", "properties": { "id": id_schema() }, "required": ["id"] }),
        },
        ToolDef {
            name: LIST_COMMANDS,
            description: "The background commands this chat started on a device (this one when omitted): each one's id, command, folder, status and how long it ran. Ended ones are listed for an hour.",
            input_schema: json!({ "type": "object", "properties": { "device": device_schema() } }),
        },
    ]
}

#[derive(Deserialize)]
pub(super) struct RunArgs {
    command: String,
    device: Option<String>,
    cwd: Option<String>,
    timeout_secs: Option<u64>,
    #[serde(default)]
    background: bool,
}

#[derive(Deserialize)]
struct IdArgs {
    id: String,
}

#[derive(Deserialize)]
struct ListArgs {
    device: Option<String>,
}

fn parse<T: serde::de::DeserializeOwned>(args: Value) -> Result<T, String> {
    let args = if args.is_null() { json!({}) } else { args };
    serde_json::from_value(args).map_err(|e| format!("invalid arguments: {e}"))
}

/// An engine error in words to act on.
fn said(error: RpcError) -> anyhow::Error {
    match error {
        RpcError::UnknownMethod(_) => anyhow::anyhow!(
            "That device's Zeron is too old to run commands: update Zeron there and restart it"
        ),
        RpcError::Failed(message) => anyhow::anyhow!(message),
        other => anyhow::anyhow!("{other}"),
    }
}

impl Tools {
    pub(super) async fn commands(&self, tool: &str, args: Value) -> Result<Value, String> {
        let result = match tool {
            RUN_COMMAND => self.run_command(parse(args)?).await,
            COMMAND_OUTPUT => {
                self.command_call(methods::READ_COMMAND_OUTPUT, parse(args)?)
                    .await
            }
            STOP_COMMAND => self.command_call(methods::STOP_COMMAND, parse(args)?).await,
            _ => self.list_commands(parse(args)?).await,
        };
        result.map_err(|e| format!("{e:#}"))
    }

    /// The device to send to (`None`: this engine's), and how a reply names it.
    async fn command_device(&self, key: Option<&str>) -> anyhow::Result<(Option<String>, Value)> {
        let (devices, local) =
            tokio::try_join!(self.zeron.devices(), self.zeron.local_device_id())?;
        let id = self.zeron.resolve_device_id(key).await?;
        let name = devices.iter().find(|d| d.id == id).map(|d| d.name.clone());
        let named = json!({ "id": id, "name": name, "local": id == local });
        Ok(((id != local).then_some(id), named))
    }

    async fn run_command(&self, args: RunArgs) -> anyhow::Result<Value> {
        anyhow::ensure!(!args.command.trim().is_empty(), "command is empty");
        let (target, device) = self.command_device(args.device.as_deref()).await?;
        let device_id = device["id"].as_str().unwrap_or_default().to_owned();
        let asked = args
            .cwd
            .as_deref()
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .map(str::to_owned);
        let remembered = match &asked {
            Some(_) => None,
            None => self.command_state.cwd.lock().await.get(&device_id).cloned(),
        };
        let timeout = args
            .timeout_secs
            .unwrap_or(DEFAULT_TIMEOUT_SECS)
            .clamp(1, MAX_TIMEOUT_SECS);
        let mut params = json!({ "command": args.command, "timeoutMs": timeout * 1000 });
        if args.background {
            params["background"] = json!(true);
        }
        if let Some(chat) = &self.zeron.origin().chat_id {
            params["chatId"] = json!(chat);
        }
        if let Some(target) = &target {
            params["targetDeviceId"] = json!(target);
        }
        let send = |cwd: Option<&String>| {
            let mut params = params.clone();
            if let Some(cwd) = cwd {
                params["cwd"] = json!(cwd);
            }
            self.zeron.call_rpc(methods::RUN_COMMAND, params)
        };
        let reply = match send(asked.as_ref().or(remembered.as_ref())).await {
            // The folder it was left in is gone: start where a first one would.
            Err(RpcError::Failed(message))
                if remembered.is_some() && message.starts_with("No folder") =>
            {
                self.command_state.cwd.lock().await.remove(&device_id);
                send(None).await
            }
            other => other,
        };
        let mut outcome = reply.map_err(said)?;
        if args.background {
            if let Some(id) = outcome["id"].as_str() {
                outcome["id"] = json!(format!("{id}@{device_id}"));
            }
        } else if let Some(cwd) = outcome["cwd"].as_str() {
            self.command_state
                .cwd
                .lock()
                .await
                .insert(device_id, cwd.to_owned());
        } else if let Some(asked) = asked {
            // A shell that doesn't say where it ended: where it was asked.
            self.command_state.cwd.lock().await.insert(device_id, asked);
        }
        outcome["device"] = device;
        Ok(outcome)
    }

    async fn command_call(&self, method: &str, args: IdArgs) -> anyhow::Result<Value> {
        let (id, device) = match args.id.rsplit_once('@') {
            Some((id, device)) => (id.to_owned(), Some(device.to_owned())),
            None => (args.id.clone(), None),
        };
        let (target, named) = self.command_device(device.as_deref()).await?;
        let mut params = json!({ "id": id });
        if let Some(target) = &target {
            params["targetDeviceId"] = json!(target);
        }
        let mut status = self.zeron.call_rpc(method, params).await.map_err(said)?;
        status["id"] = json!(args.id);
        status["device"] = named;
        Ok(status)
    }

    async fn list_commands(&self, args: ListArgs) -> anyhow::Result<Value> {
        let (target, named) = self.command_device(args.device.as_deref()).await?;
        let device_id = named["id"].as_str().unwrap_or_default().to_owned();
        let mut params = json!({});
        if let Some(chat) = &self.zeron.origin().chat_id {
            params["chatId"] = json!(chat);
        }
        if let Some(target) = &target {
            params["targetDeviceId"] = json!(target);
        }
        let mut listed = self
            .zeron
            .call_rpc(methods::LIST_BACKGROUND_COMMANDS, params)
            .await
            .map_err(said)?;
        for command in listed.as_array_mut().into_iter().flatten() {
            if let Some(id) = command["id"].as_str() {
                command["id"] = json!(format!("{id}@{device_id}"));
            }
        }
        Ok(json!({ "device": named, "commands": listed }))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::zeron::{Origin, Zeron};

    /// Two devices. `RunCommand` ends wherever `cd` took it; the rest answer
    /// with what they were asked.
    struct Engine {
        calls: std::sync::Mutex<Vec<(String, Value)>>,
        old_remote: bool,
    }

    #[async_trait::async_trait]
    impl zeron_rpc::RpcService for Engine {
        async fn handle(
            &self,
            method: &str,
            params: Value,
        ) -> Result<zeron_rpc::RpcReply, RpcError> {
            use futures::StreamExt;
            use zeron_rpc::RpcReply;
            if method.contains("Command") {
                if self.old_remote && params["targetDeviceId"] == "dev-remote" {
                    return Err(RpcError::UnknownMethod(method.into()));
                }
                self.calls
                    .lock()
                    .unwrap()
                    .push((method.to_owned(), params.clone()));
            }
            Ok(match method {
                methods::LOCAL_DEVICE => RpcReply::Value(json!({ "deviceId": "dev-local" })),
                methods::WATCH_DEVICES => RpcReply::Stream(
                    futures::stream::iter(vec![json!([
                        { "id": "dev-local", "name": "Laptop", "platform": "linux", "lastSeenAt": null },
                        { "id": "dev-remote", "name": "Worker", "platform": "linux", "lastSeenAt": null }
                    ])])
                    .boxed(),
                ),
                methods::RUN_COMMAND if params["background"] == true => {
                    RpcReply::Value(json!({ "id": "cmd-1", "status": "running", "command": params["command"] }))
                }
                methods::RUN_COMMAND => {
                    if params["cwd"] == "/gone" {
                        return Err(RpcError::Failed("No folder /gone on this device".into()));
                    }
                    let cwd = params["command"]
                        .as_str()
                        .and_then(|c| c.strip_prefix("cd "))
                        .map(str::to_owned)
                        .or_else(|| params["cwd"].as_str().map(str::to_owned))
                        .unwrap_or_else(|| "/home/me".into());
                    RpcReply::Value(json!({ "exitCode": 0, "output": "hi\n", "timedOut": false, "durationMs": 5, "cwd": cwd }))
                }
                methods::READ_COMMAND_OUTPUT | methods::STOP_COMMAND => {
                    RpcReply::Value(json!({ "id": params["id"], "status": "running", "output": "listening\n" }))
                }
                methods::LIST_BACKGROUND_COMMANDS => RpcReply::Value(json!([{ "id": "cmd-1", "status": "running" }])),
                other => return Err(RpcError::UnknownMethod(other.into())),
            })
        }
    }

    fn tools(engine: Arc<Engine>) -> Tools {
        let origin = Origin {
            chat_id: Some("chat-1".into()),
            ..Default::default()
        };
        Tools::new(Arc::new(Zeron::with_client(
            zeron_rpc::memory_client(engine),
            origin,
        )))
    }

    fn last(engine: &Engine) -> (String, Value) {
        engine.calls.lock().unwrap().last().cloned().unwrap()
    }

    #[tokio::test]
    async fn a_command_runs_on_the_device_it_names_and_cd_carries_over_there() {
        let engine = Arc::new(Engine {
            calls: Default::default(),
            old_remote: false,
        });
        let tools = tools(engine.clone());
        let ran = tools
            .call(RUN_COMMAND, json!({ "command": "echo hi", "device": "Worker", "cwd": "/srv", "timeout_secs": 5 }))
            .await
            .unwrap();
        assert_eq!(
            (ran["output"].as_str(), ran["exitCode"].as_i64()),
            (Some("hi\n"), Some(0))
        );
        assert_eq!(
            ran["device"],
            json!({ "id": "dev-remote", "name": "Worker", "local": false })
        );
        assert_eq!(
            last(&engine).1,
            json!({ "command": "echo hi", "cwd": "/srv", "timeoutMs": 5000, "chatId": "chat-1", "targetDeviceId": "dev-remote" })
        );
        // cd there sticks there, and only there.
        tools
            .call(
                RUN_COMMAND,
                json!({ "command": "cd /srv/app", "device": "Worker" }),
            )
            .await
            .unwrap();
        tools
            .call(RUN_COMMAND, json!({ "command": "ls", "device": "Worker" }))
            .await
            .unwrap();
        assert_eq!(last(&engine).1["cwd"], "/srv/app");
        tools
            .call(
                RUN_COMMAND,
                json!({ "command": "ls", "timeout_secs": 100000 }),
            )
            .await
            .unwrap();
        let (_, here) = last(&engine);
        assert!(
            here.get("cwd").is_none() && here.get("targetDeviceId").is_none(),
            "{here}"
        );
        assert_eq!(here["timeoutMs"], MAX_TIMEOUT_SECS * 1000);
        // A folder that's gone: back to where a first command starts.
        tools
            .call(RUN_COMMAND, json!({ "command": "cd /gone" }))
            .await
            .unwrap();
        tools
            .call(RUN_COMMAND, json!({ "command": "ls" }))
            .await
            .unwrap();
        assert!(last(&engine).1.get("cwd").is_none());

        assert!(
            tools
                .call(RUN_COMMAND, json!({ "command": " " }))
                .await
                .unwrap_err()
                .contains("empty")
        );
    }

    #[tokio::test]
    async fn a_background_command_is_found_again_by_its_id_wherever_it_runs() {
        let engine = Arc::new(Engine {
            calls: Default::default(),
            old_remote: false,
        });
        let tools = tools(engine.clone());
        let started = tools
            .call(
                RUN_COMMAND,
                json!({ "command": "npm run dev", "device": "Worker", "background": true }),
            )
            .await
            .unwrap();
        assert_eq!(started["id"], "cmd-1@dev-remote");
        assert_eq!(last(&engine).1["background"], true);

        let read = tools
            .call(COMMAND_OUTPUT, json!({ "id": "cmd-1@dev-remote" }))
            .await
            .unwrap();
        assert_eq!(read["output"], "listening\n");
        assert_eq!(read["id"], "cmd-1@dev-remote");
        assert_eq!(
            last(&engine),
            (
                methods::READ_COMMAND_OUTPUT.to_owned(),
                json!({ "id": "cmd-1", "targetDeviceId": "dev-remote" })
            )
        );

        tools
            .call(STOP_COMMAND, json!({ "id": "cmd-1@dev-remote" }))
            .await
            .unwrap();
        assert_eq!(last(&engine).0, methods::STOP_COMMAND);

        let listed = tools
            .call(LIST_COMMANDS, json!({ "device": "Worker" }))
            .await
            .unwrap();
        assert_eq!(listed["commands"][0]["id"], "cmd-1@dev-remote");
        assert_eq!(
            last(&engine).1,
            json!({ "chatId": "chat-1", "targetDeviceId": "dev-remote" })
        );
    }

    #[tokio::test]
    async fn an_older_device_says_so() {
        let tools = tools(Arc::new(Engine {
            calls: Default::default(),
            old_remote: true,
        }));
        let error = tools
            .call(
                RUN_COMMAND,
                json!({ "command": "ls", "device": "dev-remote" }),
            )
            .await
            .unwrap_err();
        assert!(error.contains("too old"), "{error}");
    }
}
