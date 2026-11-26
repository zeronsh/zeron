use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use base64::Engine as _;
use futures::StreamExt;
use futures::stream::BoxStream;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use zeron_proto::{
    AccessMode, AgentEvent, DoneStatus, HarnessId, Model, ModelOption, ModelOptionChoice,
    ReasoningLevel, RunRequest, SteeringMode, ToolCall,
};

use crate::{Harness, HarnessError, RunControls};

const DEFAULT_API_BASE: &str = "https://api.cursor.com";

pub struct CursorCloudHarness {
    client: reqwest::Client,
    api_base: String,
    auth_file: PathBuf,
}

impl Default for CursorCloudHarness {
    fn default() -> Self {
        Self {
            client: reqwest::Client::new(),
            api_base: std::env::var("CURSOR_CLOUD_API_BASE")
                .ok()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_API_BASE.into()),
            auth_file: super::catalog::sdk_auth_file(),
        }
    }
}

fn protocol_error(error: reqwest::Error) -> HarnessError {
    HarnessError::Protocol(format!("Cursor Cloud request failed: {error}"))
}

async fn post_json(
    client: &reqwest::Client,
    api_base: &str,
    key: &str,
    path: &str,
    body: &Value,
) -> Result<reqwest::Response, HarnessError> {
    let response = client
        .post(format!("{api_base}{path}"))
        .bearer_auth(key)
        .json(body)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(protocol_error)?;
    if response.status().is_success() {
        Ok(response)
    } else {
        Err(CursorCloudHarness::response_error(response).await)
    }
}

impl CursorCloudHarness {
    pub fn new() -> Self {
        Self::default()
    }

    #[cfg(test)]
    pub(crate) fn with_api_base(api_base: impl Into<String>, auth_file: PathBuf) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_base: api_base.into(),
            auth_file,
        }
    }

    fn api_key_from_file(&self) -> Option<String> {
        let value: Value = serde_json::from_slice(&std::fs::read(&self.auth_file).ok()?).ok()?;
        let key = value.get("apiKey")?.as_str()?.trim();
        if key.is_empty() {
            return None;
        }
        let expired = value
            .get("apiKeyExpiresAtMs")
            .and_then(Value::as_u64)
            .is_some_and(|at| {
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis()
                    >= u128::from(at)
            });
        (!expired).then(|| key.to_owned())
    }

    fn api_key(&self, env: &BTreeMap<String, String>) -> Result<String, HarnessError> {
        if let Some(key) = env
            .get("CURSOR_API_KEY")
            .map(String::as_str)
            .map(str::trim)
            .filter(|key| !key.is_empty())
        {
            return Ok(key.to_owned());
        }
        if let Some(key) = std::env::var("CURSOR_API_KEY")
            .ok()
            .map(|key| key.trim().to_owned())
            .filter(|key| !key.is_empty())
        {
            return Ok(key);
        }
        if let Some(key) = self.api_key_from_file() {
            return Ok(key);
        }
        Err(HarnessError::NotSignedIn(
            "Cursor Cloud needs a Cursor API key — sign in to Cursor in Settings → Providers, or add CURSOR_API_KEY.".into(),
        ))
    }

    async fn response_error(response: reqwest::Response) -> HarnessError {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        let message = serde_json::from_str::<Value>(&body)
            .ok()
            .and_then(|value| {
                value
                    .get("error")
                    .and_then(|error| error.get("message"))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .filter(|message| !message.is_empty())
            .unwrap_or_else(|| body.chars().take(500).collect());
        HarnessError::Protocol(format!("Cursor Cloud API {status}: {message}"))
    }

    async fn post(
        &self,
        key: &str,
        path: &str,
        body: &Value,
    ) -> Result<reqwest::Response, HarnessError> {
        post_json(&self.client, &self.api_base, key, path, body).await
    }

    async fn get_json(&self, key: &str, path: &str) -> Result<Value, HarnessError> {
        let response = self
            .client
            .get(format!("{}{path}", self.api_base))
            .bearer_auth(key)
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .map_err(protocol_error)?;
        if !response.status().is_success() {
            return Err(Self::response_error(response).await);
        }
        response
            .json()
            .await
            .map_err(|error| HarnessError::Protocol(error.to_string()))
    }

    async fn start_run(&self, key: &str, request: &RunRequest) -> Result<RemoteRun, HarnessError> {
        let mut body = run_body(request);

        if let Some(agent_id) = request.resume.as_deref().filter(|id| !id.is_empty()) {
            let response = self
                .post(key, &format!("/v1/agents/{agent_id}/runs"), &body)
                .await?;
            let value: Value = response
                .json()
                .await
                .map_err(|error| HarnessError::Protocol(error.to_string()))?;
            let run = value.get("run").cloned().unwrap_or(value);
            return parse_remote_run(run, agent_id.to_owned());
        }

        if let Some(repo) = repo_config(&request.cwd) {
            body["repos"] = json!([repo]);
        }
        body["autoCreatePR"] = json!(false);
        let response = self.post(key, "/v1/agents", &body).await?;
        let value: Value = response
            .json()
            .await
            .map_err(|error| HarnessError::Protocol(error.to_string()))?;
        let agent_id = value
            .get("agent")
            .and_then(|agent| agent.get("id"))
            .and_then(Value::as_str)
            .ok_or_else(|| {
                HarnessError::Protocol("Cursor Cloud create response had no agent id".into())
            })?
            .to_owned();
        let run = value.get("run").cloned().ok_or_else(|| {
            HarnessError::Protocol("Cursor Cloud create response had no run".into())
        })?;
        parse_remote_run(run, agent_id)
    }
}

fn run_body(request: &RunRequest) -> Value {
    let prompt = prompt_json(request);
    let mut body = json!({ "prompt": prompt });
    if let Some(model) = request.model.as_deref().filter(|model| !model.is_empty()) {
        let params = request
            .model_options
            .iter()
            .filter_map(|(id, value)| {
                let value = value.as_str().map(str::to_owned).or_else(|| {
                    value
                        .as_bool()
                        .map(|value| value.to_string())
                        .or_else(|| value.as_number().map(ToString::to_string))
                })?;
                (!value.is_empty()).then(|| json!({ "id": id, "value": value }))
            })
            .collect::<Vec<_>>();
        let mut selection = json!({ "id": model });
        if !params.is_empty() {
            selection["params"] = Value::Array(params);
        }
        body["model"] = selection;
    }
    body["mode"] = json!(if request.plan { "plan" } else { "agent" });
    body
}

#[derive(Debug)]
struct RemoteRun {
    agent_id: String,
    run_id: String,
}

fn parse_remote_run(run: Value, fallback_agent_id: String) -> Result<RemoteRun, HarnessError> {
    let run_id = run
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| HarnessError::Protocol("Cursor Cloud response had no run id".into()))?
        .to_owned();
    let agent_id = run
        .get("agentId")
        .and_then(Value::as_str)
        .unwrap_or(&fallback_agent_id)
        .to_owned();
    Ok(RemoteRun { agent_id, run_id })
}

fn prompt_json(request: &RunRequest) -> Value {
    let images = request
        .attachments
        .iter()
        .take(5)
        .filter_map(|path| image_json(Path::new(path)))
        .collect::<Vec<_>>();
    let mut prompt = json!({ "text": request.prompt });
    if !images.is_empty() {
        prompt["images"] = Value::Array(images);
    }
    prompt
}

fn image_json(path: &Path) -> Option<Value> {
    let mime = match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => return None,
    };
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() > 15 * 1024 * 1024 {
        return None;
    }
    Some(json!({
        "data": base64::engine::general_purpose::STANDARD.encode(bytes),
        "mimeType": mime,
    }))
}

fn git_output(cwd: &str, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

fn normalize_repo_url(url: &str) -> String {
    let mut url = url.trim().trim_end_matches('/').to_owned();
    if let Some(rest) = url.strip_prefix("git@github.com:") {
        url = format!("https://github.com/{rest}");
    } else if let Some(rest) = url.strip_prefix("ssh://git@github.com/") {
        url = format!("https://github.com/{rest}");
    }
    if url.ends_with(".git") {
        url.truncate(url.len() - 4);
    }
    url
}

fn repo_config(cwd: &str) -> Option<Value> {
    let inside = git_output(cwd, &["rev-parse", "--is-inside-work-tree"])?;
    if inside != "true" {
        return None;
    }
    let url = git_output(cwd, &["remote", "get-url", "origin"])?;
    let url = normalize_repo_url(&url);
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return None;
    }
    let branch = git_output(
        cwd,
        &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"],
    )
    .and_then(|_| git_output(cwd, &["branch", "--show-current"]))
    .filter(|branch| branch != "HEAD" && !branch.is_empty());
    let mut repo = json!({ "url": url });
    if let Some(branch) = branch {
        repo["startingRef"] = json!(branch);
    }
    Some(repo)
}

fn decode_models(value: &Value) -> Result<Vec<Model>, HarnessError> {
    let items = value
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut models = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let Some(id) = item.get("id").and_then(Value::as_str) else {
            continue;
        };
        let default_variant_params = item
            .get("variants")
            .and_then(Value::as_array)
            .and_then(|variants| {
                variants
                    .iter()
                    .find(|variant| variant.get("isDefault").and_then(Value::as_bool) == Some(true))
                    .or_else(|| variants.first())
            })
            .and_then(|variant| variant.get("params"))
            .and_then(Value::as_array);
        let options = item
            .get("parameters")
            .and_then(Value::as_array)
            .map(|parameters| {
                parameters
                    .iter()
                    .filter_map(|parameter| {
                        let id = parameter.get("id")?.as_str()?.to_owned();
                        let choices = parameter
                            .get("values")
                            .and_then(Value::as_array)?
                            .iter()
                            .filter_map(|choice| {
                                let value = choice.get("value")?.as_str()?;
                                Some(ModelOptionChoice {
                                    id: value.to_owned(),
                                    label: choice
                                        .get("displayName")
                                        .and_then(Value::as_str)
                                        .unwrap_or(value)
                                        .to_owned(),
                                })
                            })
                            .collect::<Vec<_>>();
                        let default_choice = default_variant_params
                            .and_then(|params| {
                                params.iter().find_map(|param| {
                                    if param.get("id").and_then(Value::as_str) == Some(id.as_str())
                                    {
                                        param.get("value")?.as_str().map(str::to_owned)
                                    } else {
                                        None
                                    }
                                })
                            })
                            .or_else(|| choices.first().map(|choice| choice.id.clone()))?;
                        Some(ModelOption {
                            label: parameter
                                .get("displayName")
                                .and_then(Value::as_str)
                                .unwrap_or(&id)
                                .to_owned(),
                            id,
                            choices,
                            default_choice,
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        models.push(Model {
            id: id.to_owned(),
            label: item
                .get("displayName")
                .and_then(Value::as_str)
                .unwrap_or(id)
                .to_owned(),
            description: item
                .get("description")
                .and_then(Value::as_str)
                .map(str::to_owned),
            provider: None,
            reasoning_levels: Vec::new(),
            options,
            default: index == 0,
        });
    }
    if models.is_empty() {
        return Err(HarnessError::Protocol(
            "Cursor Cloud returned no cloud-agent models".into(),
        ));
    }
    Ok(models)
}

#[async_trait]
impl Harness for CursorCloudHarness {
    fn id(&self) -> HarnessId {
        HarnessId::CursorCloud
    }

    fn display_name(&self) -> &str {
        "Cursor Cloud"
    }

    fn supports_steering(&self) -> bool {
        true
    }

    fn steering_mode(&self) -> SteeringMode {
        SteeringMode::StepBoundary
    }

    fn reasoning_levels(&self) -> &[ReasoningLevel] {
        &[]
    }

    fn installed(&self) -> bool {
        std::env::var_os("ZERON_DEVICE_PLATFORM")
            .is_some_and(|platform| platform.to_string_lossy() == "cloud")
            || std::env::var_os("CURSOR_API_KEY").is_some_and(|key| !key.is_empty())
            || self.api_key_from_file().is_some()
    }

    fn deterministic_turn_end(&self) -> bool {
        true
    }

    fn access_modes(&self) -> &[AccessMode] {
        &[AccessMode::FullAccess]
    }

    fn supports_plan(&self) -> bool {
        true
    }

    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        let key = self.api_key(&crate::catalog_env::get(HarnessId::CursorCloud))?;
        let value = self.get_json(&key, "/v1/models").await?;
        decode_models(&value)
    }

    fn fallback_models(&self) -> Vec<Model> {
        vec![Model {
            id: "default".into(),
            label: "Auto".into(),
            description: Some("Cursor Cloud default".into()),
            provider: None,
            reasoning_levels: Vec::new(),
            options: Vec::new(),
            default: true,
        }]
    }

    async fn run(
        &self,
        request: RunRequest,
        mut controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        let key = self.api_key(&request.env)?;
        let remote = self.start_run(&key, &request).await?;
        let (tx, rx) = mpsc::unbounded_channel();
        let client = self.client.clone();
        let api_base = self.api_base.clone();
        let model = request.model.clone().unwrap_or_else(|| "default".into());
        let cwd = request.cwd.clone();
        tokio::spawn(async move {
            drive_remote_run(client, api_base, key, remote, model, cwd, &mut controls, tx).await;
        });
        Ok(futures::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|value| (value, rx))
        })
        .boxed())
    }
}

fn emit(tx: &mpsc::UnboundedSender<Result<AgentEvent, HarnessError>>, event: AgentEvent) -> bool {
    tx.send(Ok(event)).is_ok()
}

struct SseEvent {
    event: String,
    data: String,
}

fn drain_sse(buffer: &mut Vec<u8>) -> Vec<SseEvent> {
    let mut events = Vec::new();
    loop {
        let Some(pos) = buffer.windows(2).position(|pair| pair == b"\n\n") else {
            break;
        };
        let block = buffer.drain(..pos).collect::<Vec<_>>();
        buffer.drain(..2.min(buffer.len()));
        let Ok(text) = String::from_utf8(block) else {
            continue;
        };
        let mut event = String::new();
        let mut data = String::new();
        for line in text.lines() {
            if line.starts_with("event:") {
                event = line[6..].trim_start().to_owned();
            } else if line.starts_with("data:") {
                if !data.is_empty() {
                    data.push('\n');
                }
                data.push_str(line[5..].trim_start());
            }
        }
        if !event.is_empty() {
            events.push(SseEvent { event, data });
        }
    }
    events
}

fn decode_cloud_tool(name: &str, args: &Value) -> ToolCall {
    let string = |keys: &[&str]| {
        keys.iter()
            .find_map(|key| args.get(*key).and_then(Value::as_str))
            .unwrap_or("")
            .to_owned()
    };
    match name {
        "run_terminal_cmd" | "shell" | "bash" => ToolCall::Exec {
            command: string(&["command", "cmd", "script"]),
        },
        "read_file" | "read" => ToolCall::ReadFile {
            path: string(&["path", "filePath", "file_path"]),
        },
        "write_file" | "write" => ToolCall::WriteFile {
            path: string(&["path", "filePath", "file_path"]),
            content: None,
        },
        "edit_file" | "edit" => ToolCall::EditFile {
            path: string(&["path", "filePath", "file_path"]),
            old_string: None,
            new_string: None,
        },
        "grep" | "search" => ToolCall::Search {
            pattern: string(&["pattern", "query"]),
            path: args.get("path").and_then(Value::as_str).map(str::to_owned),
        },
        "glob" => ToolCall::Glob {
            pattern: string(&["pattern", "globPattern"]),
        },
        "web_search" | "webSearch" => ToolCall::WebSearch {
            query: string(&["query", "search"]),
        },
        "web_fetch" | "webFetch" => ToolCall::WebFetch {
            url: string(&["url"]),
            prompt: None,
        },
        _ => ToolCall::Unknown {
            name: name.to_owned(),
            input: (!args.is_null()).then(|| args.clone()),
        },
    }
}

fn tool_output(result: Option<&Value>) -> Option<String> {
    let result = result?;
    if let Some(content) = result
        .get("success")
        .and_then(|success| success.get("content"))
        .and_then(Value::as_str)
    {
        return Some(zeron_proto::command_output_cap(content).unwrap_or_default());
    }
    if let Some(error) = result
        .get("error")
        .and_then(|error| error.get("message").or_else(|| error.get("content")))
        .and_then(Value::as_str)
    {
        return Some(error.to_owned());
    }
    serde_json::to_string(result)
        .ok()
        .filter(|text| text != "null")
        .map(|text| zeron_proto::command_output_cap(&text).unwrap_or_default())
}

async fn cancel_run(
    client: &reqwest::Client,
    api_base: &str,
    key: &str,
    agent_id: &str,
    run_id: &str,
) {
    let _ = client
        .post(format!(
            "{api_base}/v1/agents/{agent_id}/runs/{run_id}/cancel"
        ))
        .bearer_auth(key)
        .timeout(Duration::from_secs(10))
        .send()
        .await;
}

struct StreamOutcome {
    status: String,
    result: Option<String>,
    git: Option<Value>,
    queued: usize,
    interrupted: bool,
}

async fn stream_current_run(
    client: &reqwest::Client,
    api_base: &str,
    key: &str,
    agent_id: &str,
    run_id: &str,
    controls: &mut RunControls,
    tx: &mpsc::UnboundedSender<Result<AgentEvent, HarnessError>>,
) -> StreamOutcome {
    let url = format!("{api_base}/v1/agents/{agent_id}/runs/{run_id}/stream");
    let response = match client
        .get(url)
        .bearer_auth(key)
        .header("accept", "text/event-stream")
        .send()
        .await
    {
        Ok(response) if response.status().is_success() => response,
        Ok(response) => {
            let error = CursorCloudHarness::response_error(response).await;
            emit(
                tx,
                AgentEvent::Error {
                    message: error.to_string(),
                },
            );
            return StreamOutcome {
                status: "ERROR".into(),
                result: None,
                git: None,
                queued: 0,
                interrupted: false,
            };
        }
        Err(error) => {
            emit(
                tx,
                AgentEvent::Error {
                    message: format!("Cursor Cloud stream failed: {error}"),
                },
            );
            return StreamOutcome {
                status: "ERROR".into(),
                result: None,
                git: None,
                queued: 0,
                interrupted: false,
            };
        }
    };

    let mut bytes = response.bytes_stream();
    let mut buffer = Vec::new();
    let mut seen_tools = HashSet::new();
    let mut outcome = StreamOutcome {
        status: "RUNNING".into(),
        result: None,
        git: None,
        queued: 0,
        interrupted: false,
    };

    loop {
        tokio::select! {
            _ = controls.interrupt.cancelled() => {
                cancel_run(client, api_base, key, agent_id, run_id).await;
                outcome.interrupted = true;
                return outcome;
            }
            steer = controls.steering.recv() => {
                let Some(steer) = steer else {
                    continue;
                };
                let images = steer
                    .attachments
                    .iter()
                    .take(5)
                    .filter_map(|path| image_json(Path::new(path)))
                    .collect::<Vec<_>>();
                let mut prompt = json!({ "text": steer.prompt });
                if !images.is_empty() {
                    prompt["images"] = Value::Array(images);
                }
                let body = json!({ "prompt": prompt, "delivery": "steer" });
                match post_json(
                    client,
                    api_base,
                    key,
                    &format!("/v1/agents/{agent_id}/runs"),
                    &body,
                )
                .await
                {
                    Ok(response) => {
                        let delivery = response.json::<Value>().await.ok()
                            .and_then(|value| value.get("delivery").and_then(Value::as_str).map(str::to_owned));
                        if delivery.as_deref() == Some("queued") {
                            outcome.queued += 1;
                        }
                        emit(tx, AgentEvent::Steered {
                            assistant_message_id: None,
                            next_assistant_message_id: None,
                        });
                    }
                    Err(error) => {
                        emit(tx, AgentEvent::Error {
                            message: format!("Cursor Cloud couldn't steer the run: {error}"),
                        });
                    }
                }
            }
            chunk = bytes.next() => {
                let Some(chunk) = chunk else {
                    return outcome;
                };
                match chunk {
                    Ok(chunk) => {
                        buffer.extend_from_slice(&chunk);
                        for event in drain_sse(&mut buffer) {
                            let Ok(data) = serde_json::from_str::<Value>(&event.data) else {
                                if event.event != "done" && event.event != "heartbeat" {
                                    data_warnings(tx, &event.event);
                                }
                                continue;
                            };
                            match event.event.as_str() {
                                "assistant" => {
                                    if let Some(text) = data.get("text").and_then(Value::as_str)
                                        && !emit(tx, AgentEvent::TextDelta { text: text.to_owned() }) {
                                            return outcome;
                                        }
                                }
                                "thinking" => {
                                    if let Some(text) = data.get("text").and_then(Value::as_str) {
                                        emit(tx, AgentEvent::ReasoningDelta { text: text.to_owned() });
                                    }
                                }
                                "tool_call" => {
                                    let id = data.get("callId").and_then(Value::as_str).unwrap_or("").to_owned();
                                    let name = data.get("name").and_then(Value::as_str).unwrap_or("tool");
                                    let args = data.get("args").cloned().unwrap_or(Value::Null);
                                    let status = data.get("status").and_then(Value::as_str).unwrap_or("running");
                                    if !id.is_empty() && seen_tools.insert(id.clone()) {
                                        emit(tx, AgentEvent::ToolCall {
                                            id: id.clone(),
                                            call: decode_cloud_tool(name, &args),
                                        });
                                    }
                                    if status == "completed" && !id.is_empty() {
                                        let result = data.get("result");
                                        emit(tx, AgentEvent::ToolResult {
                                            id,
                                            is_error: result.and_then(|r| r.get("error")).is_some(),
                                            output: tool_output(result),
                                            diff: None,
                                            preview_paths: Vec::new(),
                                            image_payloads: Vec::new(),
                                            exit_code: result
                                                .and_then(|r| {
                                                    r.get("exitCode").or_else(|| {
                                                        r.get("success").and_then(|success| success.get("exitCode"))
                                                    })
                                                })
                                                .and_then(Value::as_i64)
                                                .map(|c| c as i32),
                                        });
                                    }
                                }
                                "status" => {
                                    if let Some(status) = data.get("status").and_then(Value::as_str) {
                                        outcome.status = status.to_owned();
                                    }
                                }
                                "result" => {
                                    if let Some(status) = data.get("status").and_then(Value::as_str) {
                                        outcome.status = status.to_owned();
                                    }
                                    outcome.result = data.get("text").and_then(Value::as_str).map(str::to_owned);
                                    outcome.git = data.get("git").cloned();
                                }
                                "error" => {
                                    let message = data.get("message").and_then(Value::as_str).unwrap_or("Cursor Cloud run failed");
                                    emit(tx, AgentEvent::Error { message: message.to_owned() });
                                    outcome.status = "ERROR".into();
                                }
                                "done" => return outcome,
                                _ => {}
                            }
                        }
                    }
                    Err(error) => {
                        emit(tx, AgentEvent::Error {
                            message: format!("Cursor Cloud stream failed: {error}"),
                        });
                        outcome.status = "ERROR".into();
                        return outcome;
                    }
                }
            }
        }
    }
}

fn data_warnings(tx: &mpsc::UnboundedSender<Result<AgentEvent, HarnessError>>, event: &str) {
    emit(
        tx,
        AgentEvent::Error {
            message: format!("Cursor Cloud sent a malformed {event} event"),
        },
    );
}

fn git_markdown(git: &Value) -> Option<String> {
    let branches = git.get("branches")?.as_array()?;
    let mut parts = Vec::new();
    for branch in branches {
        let repo = branch
            .get("repoUrl")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let name = branch
            .get("branch")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        if repo.is_empty() {
            parts.push(format!("branch `{name}`"));
        } else {
            parts.push(format!("branch [`{name}`](https://{repo}/tree/{name})"));
        }
        if let Some(pr) = branch.get("prUrl").and_then(Value::as_str)
            && !pr.is_empty()
        {
            parts.push(format!("PR: {pr}"));
        }
    }
    (!parts.is_empty()).then(|| format!("\n\nCursor Cloud pushed {}.", parts.join("; ")))
}

async fn wait_for_next_run(
    client: &reqwest::Client,
    api_base: &str,
    key: &str,
    agent_id: &str,
    seen: &HashSet<String>,
) -> Option<String> {
    for _ in 0..240 {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let response = client
            .get(format!("{api_base}/v1/agents/{agent_id}"))
            .bearer_auth(key)
            .timeout(Duration::from_secs(10))
            .send()
            .await;
        if let Ok(response) = response
            && response.status().is_success()
            && let Ok(agent) = response.json::<Value>().await
            && let Some(id) = agent.get("latestRunId").and_then(Value::as_str)
            && !seen.contains(id)
        {
            return Some(id.to_owned());
        }
    }
    None
}

async fn drive_remote_run(
    client: reqwest::Client,
    api_base: String,
    key: String,
    remote: RemoteRun,
    model: String,
    cwd: String,
    controls: &mut RunControls,
    tx: mpsc::UnboundedSender<Result<AgentEvent, HarnessError>>,
) {
    let agent_id = remote.agent_id;
    let mut run_id = remote.run_id;
    let mut seen = HashSet::from([run_id.clone()]);
    if !emit(
        &tx,
        AgentEvent::SessionStarted {
            harness: HarnessId::CursorCloud,
            model,
            tools: Vec::new(),
            cwd,
            session_id: agent_id.clone(),
            assistant_message_id: uuid::Uuid::new_v4().to_string(),
        },
    ) {
        return;
    }

    let mut final_git = None;
    let (final_status, final_result) = loop {
        let outcome =
            stream_current_run(&client, &api_base, &key, &agent_id, &run_id, controls, &tx).await;
        let interrupted = outcome.interrupted;
        let queued = outcome.queued;
        let status = outcome.status;
        let result = outcome.result;
        final_git = outcome.git.or(final_git);
        if interrupted {
            break ("CANCELLED".to_owned(), result);
        }
        if queued == 0 {
            break (status, result);
        }
        let Some(next) = wait_for_next_run(&client, &api_base, &key, &agent_id, &seen).await else {
            break (status, result);
        };
        seen.insert(next.clone());
        run_id = next;
    };

    if let Some(markdown) = final_git.as_ref().and_then(git_markdown) {
        emit(&tx, AgentEvent::TextDelta { text: markdown });
    }
    let status = match final_status.as_str() {
        "FINISHED" => DoneStatus::Completed,
        "CANCELLED" => DoneStatus::Interrupted,
        _ => DoneStatus::Errored,
    };
    let error = (status == DoneStatus::Errored)
        .then(|| format!("Cursor Cloud run ended as {final_status}"));
    emit(
        &tx,
        AgentEvent::Done {
            status,
            result: final_result,
            error,
            session_id: Some(agent_id),
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_events_parse_multiline_frames() {
        let mut bytes =
            b"id: 1\nevent: assistant\ndata: {\"text\":\"a\"}\n\nid: 2\nevent: done\ndata: {}\n\n"
                .to_vec();
        let events = drain_sse(&mut bytes);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event, "assistant");
        assert_eq!(events[0].data, "{\"text\":\"a\"}");
        assert!(bytes.is_empty());
    }

    #[test]
    fn repository_urls_normalize_to_https_without_dot_git() {
        assert_eq!(
            normalize_repo_url("git@github.com:owner/repo.git"),
            "https://github.com/owner/repo"
        );
        assert_eq!(
            normalize_repo_url("ssh://git@github.com/owner/repo.git"),
            "https://github.com/owner/repo"
        );
    }

    #[test]
    fn cloud_tool_names_decode_to_native_shapes() {
        assert_eq!(
            decode_cloud_tool("run_terminal_cmd", &json!({"command":"cargo test"})),
            ToolCall::Exec {
                command: "cargo test".into()
            }
        );
        assert_eq!(
            decode_cloud_tool("read_file", &json!({"path":"src/main.rs"})),
            ToolCall::ReadFile {
                path: "src/main.rs".into()
            }
        );
    }

    fn request(mut extra: Value) -> RunRequest {
        let mut base = json!({
            "prompt": "do it",
            "cwd": ".",
            "sandbox": "workspace-write"
        });
        base.as_object_mut()
            .unwrap()
            .extend(extra.as_object_mut().unwrap().clone());
        serde_json::from_value(base).unwrap()
    }

    #[test]
    fn run_body_carries_model_params_and_plan_mode() {
        let body = run_body(&request(json!({
            "model": "composer-2",
            "plan": true,
            "modelOptions": { "fast": "true" }
        })));
        assert_eq!(body["prompt"]["text"], "do it");
        assert_eq!(body["model"]["id"], "composer-2");
        assert_eq!(body["model"]["params"][0]["id"], "fast");
        assert_eq!(body["model"]["params"][0]["value"], "true");
        assert_eq!(body["mode"], "plan");
    }

    #[test]
    fn models_use_the_default_variant_for_option_defaults() {
        let models = decode_models(&json!({
            "items": [{
                "id": "composer-2",
                "displayName": "Composer 2",
                "parameters": [{
                    "id": "fast",
                    "displayName": "Fast",
                    "values": [
                        { "value": "true", "displayName": "Fast" },
                        { "value": "false", "displayName": "Precise" }
                    ]
                }],
                "variants": [{
                    "displayName": "Composer 2",
                    "isDefault": true,
                    "params": [{ "id": "fast", "value": "false" }]
                }]
            }]
        }))
        .unwrap();
        assert_eq!(models.len(), 1);
        assert!(models[0].default);
        assert_eq!(models[0].options[0].default_choice, "false");
        assert_eq!(models[0].options[0].choices[1].label, "Precise");
    }

    #[test]
    fn git_result_becomes_branch_and_pr_links() {
        let markdown = git_markdown(&json!({
            "branches": [{
                "repoUrl": "github.com/acme/repo",
                "branch": "cursor/fix-123",
                "prUrl": "https://github.com/acme/repo/pull/7"
            }]
        }))
        .unwrap();
        assert!(
            markdown
                .contains("[`cursor/fix-123`](https://github.com/acme/repo/tree/cursor/fix-123)")
        );
        assert!(markdown.contains("https://github.com/acme/repo/pull/7"));
    }

    #[test]
    fn remote_run_reads_run_and_agent_ids() {
        let remote = parse_remote_run(
            json!({ "id": "run-1", "agentId": "bc-agent" }),
            "fallback".into(),
        )
        .unwrap();
        assert_eq!(remote.run_id, "run-1");
        assert_eq!(remote.agent_id, "bc-agent");
    }

    async fn read_http_request(socket: &mut tokio::net::TcpStream) -> (String, Value) {
        use tokio::io::AsyncReadExt;

        let mut bytes = Vec::new();
        let mut chunk = [0_u8; 8192];
        loop {
            let read = socket.read(&mut chunk).await.unwrap();
            assert!(read > 0, "connection closed before a full request");
            bytes.extend_from_slice(&chunk[..read]);
            let Some(headers_end) = bytes.windows(4).position(|window| window == b"\r\n\r\n")
            else {
                continue;
            };
            let headers = String::from_utf8_lossy(&bytes[..headers_end]).to_string();
            let content_length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    if name.eq_ignore_ascii_case("content-length") {
                        value.trim().parse::<usize>().ok()
                    } else {
                        None
                    }
                })
                .unwrap_or(0);
            if bytes.len() >= headers_end + 4 + content_length {
                let body = &bytes[headers_end + 4..headers_end + 4 + content_length];
                return (headers, serde_json::from_slice(body).unwrap_or(Value::Null));
            }
        }
    }

    async fn write_http_response(
        socket: &mut tokio::net::TcpStream,
        status: &str,
        content_type: &str,
        body: &str,
    ) {
        use tokio::io::AsyncWriteExt;

        let response = format!(
            "HTTP/1.1 {status}\r\ncontent-type: {content_type}\r\nconnection: close\r\ncontent-length: {}\r\n\r\n{body}",
            body.len()
        );
        socket.write_all(response.as_bytes()).await.unwrap();
    }

    #[tokio::test]
    async fn run_creates_a_cloud_agent_and_streams_events_to_done() {
        use tokio::io::AsyncWriteExt;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let (headers, body) = read_http_request(&mut socket).await;
            assert!(headers.starts_with("POST /v1/agents "));
            assert_eq!(body["prompt"]["text"], "do it");
            assert_eq!(body["mode"], "agent");
            let create = json!({
                "agent": { "id": "bc-test" },
                "run": { "id": "run-1", "agentId": "bc-test", "status": "CREATING" }
            })
            .to_string();
            write_http_response(&mut socket, "201 Created", "application/json", &create).await;
            let _ = socket.shutdown().await;

            let (mut socket, _) = listener.accept().await.unwrap();
            let (headers, _) = read_http_request(&mut socket).await;
            assert!(headers.starts_with("GET /v1/agents/bc-test/runs/run-1/stream "));
            let stream = concat!(
                "event: assistant\n",
                "data: {\"text\":\"hello\"}\n\n",
                "event: tool_call\n",
                "data: {\"callId\":\"call-1\",\"name\":\"run_terminal_cmd\",\"status\":\"running\",\"args\":{\"command\":\"echo ok\"}}\n\n",
                "event: tool_call\n",
                "data: {\"callId\":\"call-1\",\"name\":\"run_terminal_cmd\",\"status\":\"completed\",\"result\":{\"success\":{\"content\":\"ok\",\"exitCode\":0}}}\n\n",
                "event: result\n",
                "data: {\"runId\":\"run-1\",\"status\":\"FINISHED\",\"text\":\"done\"}\n\n",
                "event: done\n",
                "data: {}\n\n"
            );
            write_http_response(&mut socket, "200 OK", "text/event-stream", stream).await;
            let _ = socket.shutdown().await;
        });

        let directory = tempfile::tempdir().unwrap();
        let auth_file = directory.path().join("auth.json");
        std::fs::write(&auth_file, json!({ "apiKey": "test-key" }).to_string()).unwrap();
        let harness = CursorCloudHarness::with_api_base(format!("http://{address}"), auth_file);
        let (_steering_tx, steering) = mpsc::channel(1);
        let mut stream = harness
            .run(
                request(json!({})),
                RunControls {
                    realtime: None,
                    execution_lease: None,
                    request_input: Box::new(|_| {
                        let (tx, rx) = tokio::sync::oneshot::channel();
                        drop(tx);
                        rx
                    }),
                    steering,
                    interrupt: tokio_util::sync::CancellationToken::new(),
                    turn: crate::TurnControl::default(),
                },
            )
            .await
            .unwrap();
        let mut events = Vec::new();
        while let Some(event) = stream.next().await {
            let event = event.unwrap();
            let done = matches!(event, AgentEvent::Done { .. });
            events.push(event);
            if done {
                break;
            }
        }
        server.await.unwrap();

        assert!(events.iter().any(|event| matches!(
            event,
            AgentEvent::SessionStarted { session_id, .. } if session_id == "bc-test"
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            AgentEvent::TextDelta { text } if text == "hello"
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            AgentEvent::ToolResult { output: Some(output), exit_code: Some(0), .. } if output == "ok"
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            AgentEvent::Done { status: DoneStatus::Completed, session_id: Some(session_id), .. }
                if session_id == "bc-test"
        )));
    }
}
