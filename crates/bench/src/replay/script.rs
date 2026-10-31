//! Version 1 `.zbr.jsonl`: a meta line followed by timed raw frames/directives.
//! `frames` counts raw `f` lines. The hash covers top-level text deltas in order,
//! including rendered markers; thinking, tool output, and echoed prompts do not count.
use std::path::Path;

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;

use super::cli::{Output, Session, fresh_id};
use crate::probe::{ProbeEvent, ProbeSender};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Meta {
    pub zb: String,
    pub v: u32,
    pub name: String,
    pub seed: u64,
    pub text_bytes: usize,
    pub text_sha256: String,
    pub frames: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Step {
    Frame { dt: u64, f: Value },
    Directive(Directive),
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "zb", rename_all = "snake_case")]
pub enum Directive {
    Marker {
        dt: u64,
    },
    Exec {
        dt: u64,
        argv: Vec<String>,
        cwd: String,
        tool: String,
    },
    TurnEnd {
        dt: u64,
        #[serde(default)]
        usage: Value,
    },
}
impl Step {
    fn dt(&self) -> u64 {
        match self {
            Self::Frame { dt, .. }
            | Self::Directive(
                Directive::Marker { dt }
                | Directive::Exec { dt, .. }
                | Directive::TurnEnd { dt, .. },
            ) => *dt,
        }
    }
}
#[derive(Clone, Debug)]
pub struct Script {
    pub meta: Meta,
    pub steps: Vec<Step>,
}
impl Script {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        Self::parse(
            &std::fs::read_to_string(path.as_ref())
                .with_context(|| format!("read script {}", path.as_ref().display()))?,
        )
    }
    pub fn parse(source: &str) -> Result<Self> {
        let mut lines = source.lines().filter(|line| !line.trim().is_empty());
        let meta: Meta = serde_json::from_str(lines.next().context("missing script meta")?)?;
        ensure!(
            meta.zb == "meta" && meta.v == 1,
            "unsupported script version"
        );
        let steps: Vec<Step> = lines
            .enumerate()
            .map(|(n, line)| {
                serde_json::from_str(line).with_context(|| format!("script line {}", n + 2))
            })
            .collect::<Result<_>>()?;
        ensure!(
            steps
                .iter()
                .filter(|s| matches!(s, Step::Frame { .. }))
                .count()
                == meta.frames,
            "meta.frames does not match raw frame count"
        );
        let ends = steps
            .iter()
            .filter(|s| matches!(s, Step::Directive(Directive::TurnEnd { .. })))
            .count();
        ensure!(
            ends <= 1
                && (ends == 0
                    || matches!(
                        steps.last(),
                        Some(Step::Directive(Directive::TurnEnd { .. }))
                    )),
            "turn_end must occur once, at the end"
        );
        Ok(Self { meta, steps })
    }
}

pub fn text_sha256(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

#[derive(Clone, Debug)]
pub struct Play {
    pub path: String,
    pub speed: f64,
    /// Additional evenly distributed markers (explicit marker directives always apply).
    pub markers: usize,
    pub loops: usize,
}
impl Play {
    pub fn parse(prompt: &str) -> Result<Option<Self>> {
        let Some(args) = prompt.strip_prefix("zb:play ") else {
            return Ok(None);
        };
        let mut args = args.split_whitespace();
        let path = args
            .next()
            .context("zb:play requires a script path")?
            .to_owned();
        let mut play = Self {
            path,
            speed: 1.0,
            markers: 0,
            loops: 1,
        };
        for arg in args {
            let (key, value) = arg.split_once('=').context("expected key=value")?;
            match key {
                "speed" => play.speed = value.parse()?,
                "markers" => play.markers = value.parse()?,
                "loop" => play.loops = value.parse()?,
                _ => bail!("unknown play option {key}"),
            }
        }
        ensure!(
            play.speed.is_finite() && play.speed >= 0.0,
            "speed must be finite and nonnegative"
        );
        ensure!(play.loops > 0, "loop must be positive");
        Ok(Some(play))
    }
}

fn template(value: &mut Value, session: &Session, msg: &str, tool: &str) {
    match value {
        Value::String(s) => {
            *s = s
                .replace("$SESSION_ID", &session.id)
                .replace("$MSG_ID", msg)
                .replace("$CWD", &session.cwd)
                .replace("$TOOL_ID", tool);
        }
        Value::Array(values) => values
            .iter_mut()
            .for_each(|v| template(v, session, msg, tool)),
        Value::Object(values) => values
            .values_mut()
            .for_each(|v| template(v, session, msg, tool)),
        _ => {}
    }
}
async fn send(
    tx: &mpsc::Sender<(u64, Output)>,
    generation: u64,
    frame: Value,
    marker: Option<(String, u64)>,
) -> Result<()> {
    tx.send((generation, Output::Frame(frame, marker)))
        .await
        .context("output closed")
}

pub(crate) async fn play(
    play: &Play,
    session: &Session,
    tx: &mpsc::Sender<(u64, Output)>,
    generation: u64,
) -> Result<(String, Value, Option<Value>)> {
    // File reads and parsing never hold up the control loop.
    let path = play.path.clone();
    let script = tokio::task::spawn_blocking(move || Script::load(path)).await??;
    let auto_assistant = !script.steps.iter().any(|s| {
        matches!(s,
        Step::Frame { f, .. } if f["type"] == "assistant")
    });
    let total_deltas = script.steps.iter().filter(|s| matches!(s,
        Step::Frame { f, .. } if f["event"]["delta"]["type"] == "text_delta" && f["parent_tool_use_id"].is_null())).count();
    let marker_count = play.markers.min(total_deltas);
    let mut seq = 0;
    let mut all_text = String::new();
    let mut usage = json!({"input_tokens":0,"output_tokens":0});
    let mut result = None;
    for _ in 0..play.loops {
        let mut loop_text = String::new();
        let mut pending_marker = false;
        let mut delta_no = 0;
        let mut block = Value::Null;
        let mut rendered_blocks = std::collections::BTreeMap::new();
        let mut marked_message = false;
        let mut msg = fresh_id()?;
        let mut tool = fresh_id()?;
        for step in &script.steps {
            if play.speed > 0.0 {
                let ms = step.dt() as f64 / play.speed;
                ensure!(ms <= u64::MAX as f64, "scaled delay overflow");
                tokio::time::sleep(std::time::Duration::from_secs_f64(ms / 1000.0)).await;
            }
            match step {
                Step::Directive(Directive::Marker { .. }) => pending_marker = true,
                Step::Directive(Directive::TurnEnd { usage: u, .. }) => {
                    if !u.is_null() {
                        usage = u.clone();
                    }
                }
                Step::Directive(Directive::Exec {
                    argv, cwd, tool, ..
                }) => {
                    exec(argv, cwd, tool, session, tx, generation).await?;
                }
                Step::Frame { f, .. } => {
                    let mut f = f.clone();
                    if f["event"]["type"] == "message_start" {
                        msg = fresh_id()?;
                    }
                    if f["event"]["type"] == "content_block_start"
                        && f["event"]["content_block"]["type"] == "tool_use"
                    {
                        tool = fresh_id()?;
                    }
                    template(&mut f, session, &msg, &tool);
                    // Recorded result boundaries (including background wake
                    // turns) must retain their original order and payload.
                    if f["type"] == "result" {
                        result = Some(Value::Null);
                    }
                    let mut mark = None;
                    if f["type"] == "stream_event" && f["parent_tool_use_id"].is_null() {
                        let event = &mut f["event"];
                        if event["type"] == "content_block_start" {
                            block = event["content_block"].clone();
                        }
                        if event["type"] == "content_block_delta" {
                            let delta = &mut event["delta"];
                            match delta["type"].as_str().unwrap_or("") {
                                "text_delta" => {
                                    let mut text = delta["text"]
                                        .as_str()
                                        .context("text_delta lacks text")?
                                        .to_owned();
                                    let auto = marker_count > 0
                                        && total_deltas > 0
                                        && (delta_no * marker_count / total_deltas
                                            != (delta_no + 1) * marker_count / total_deltas);
                                    delta_no += 1;
                                    if pending_marker || auto {
                                        seq += 1;
                                        text.push_str(&format!(" [[zb:{}:{seq}]]", session.chat));
                                        mark = Some((session.chat.clone(), seq));
                                        pending_marker = false;
                                        marked_message = true;
                                    }
                                    loop_text.push_str(&text);
                                    append_block(&mut block, "text", &text);
                                    delta["text"] = text.into();
                                }
                                "thinking_delta" => append_block(
                                    &mut block,
                                    "thinking",
                                    delta["thinking"].as_str().unwrap_or(""),
                                ),
                                "input_json_delta" => append_block(
                                    &mut block,
                                    "partial_json",
                                    delta["partial_json"].as_str().unwrap_or(""),
                                ),
                                _ => {}
                            }
                        }
                    }
                    let stop = f["event"]["type"] == "content_block_stop"
                        && f["parent_tool_use_id"].is_null();
                    if stop && !block.is_null() {
                        rendered_blocks
                            .insert(f["event"]["index"].as_u64().unwrap_or(0), block.clone());
                    }
                    if f["type"] == "assistant" && f["parent_tool_use_id"].is_null() {
                        if marked_message
                            && let Some(content) = f["message"]["content"].as_array_mut()
                        {
                            for (index, item) in content.iter_mut().enumerate() {
                                if item["type"] == "text"
                                    && let Some(rendered) = rendered_blocks.get(&(index as u64))
                                {
                                    item["text"] = rendered["text"].clone();
                                }
                            }
                        }
                        rendered_blocks.clear();
                        marked_message = false;
                        msg = fresh_id()?;
                    }
                    send(tx, generation, f, mark).await?;
                    if stop && auto_assistant && !block.is_null() {
                        if block["type"] == "tool_use" {
                            if let Some(partial) =
                                block.as_object_mut().unwrap().remove("partial_json")
                            {
                                block["input"] =
                                    serde_json::from_str(partial.as_str().unwrap_or("{}"))?;
                            }
                        }
                        send(tx, generation, assistant(session, vec![block.take()]), None).await?;
                    }
                }
            }
        }
        ensure!(!pending_marker, "marker without a following text_delta");
        if auto_assistant && !block.is_null() {
            send(tx, generation, assistant(session, vec![block]), None).await?;
        }
        // Extra marker knobs intentionally change the text/hash. Explicit marker
        // scripts put their rendered text hash in meta for a fixed chat identity.
        if play.markers == 0 && play.loops == 1 {
            ensure!(
                loop_text.len() == script.meta.text_bytes
                    && text_sha256(&loop_text) == script.meta.text_sha256,
                "script text does not match meta bytes/hash"
            );
        }
        all_text.push_str(&loop_text);
    }
    Ok((all_text, usage, result))
}
fn append_block(block: &mut Value, key: &str, text: &str) {
    if block.is_null() {
        *block = json!({"type": if key == "thinking" {"thinking"} else {"text"}});
    }
    match &mut block[key] {
        Value::String(content) => content.push_str(text),
        value => *value = text.into(),
    }
}
pub(crate) fn assistant(session: &Session, content: Vec<Value>) -> Value {
    json!({"type":"assistant","uuid":fresh_id().unwrap_or_default(),"session_id":session.id,
        "parent_tool_use_id":null,"message":{"id":fresh_id().unwrap_or_default(),"type":"message",
        "role":"assistant","model":session.model,"content":content,"stop_reason":"end_turn",
        "usage":{"input_tokens":0,"output_tokens":0}}})
}
async fn exec(
    argv: &[String],
    cwd: &str,
    tool: &str,
    session: &Session,
    tx: &mpsc::Sender<(u64, Output)>,
    generation: u64,
) -> Result<()> {
    ensure!(!argv.is_empty(), "exec argv is empty");
    let mut args = json!(argv);
    template(&mut args, session, "", "");
    let args: Vec<String> = serde_json::from_value(args)?;
    let cwd = cwd.replace("$CWD", &session.cwd);
    let id = fresh_id()?;
    send(
        tx,
        generation,
        assistant(
            session,
            vec![json!({"type":"tool_use","id":id,"name":tool,
        "input":{"command":args.join(" ")}})],
        ),
        None,
    )
    .await?;
    let t0 = crate::mono_now();
    let mut command = tokio::process::Command::new(&args[0]);
    command
        .args(&args[1..])
        .current_dir(cwd)
        .kill_on_drop(true)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    // Interrupt also tears down grandchildren of shell/build commands.
    command.process_group(0);
    let child = command.spawn()?;
    let group = ExecGroup(child.id().context("exec child has no pid")?);
    let output = child.wait_with_output().await?;
    drop(group);
    let t1 = crate::mono_now();
    ProbeSender::from_env().send(&ProbeEvent::Exec {
        argv: args,
        t0,
        t1,
        status: output.status.code().unwrap_or(-1),
        makeflags_present: std::env::var_os("MAKEFLAGS").is_some()
            || std::env::var_os("CARGO_MAKEFLAGS").is_some(),
    });
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    send(
        tx,
        generation,
        json!({"type":"user","session_id":session.id,"parent_tool_use_id":null,
        "message":{"role":"user","content":[{"type":"tool_result","tool_use_id":id,
            "content":text,"is_error":!output.status.success()}]}}),
        None,
    )
    .await
}

struct ExecGroup(u32);
impl Drop for ExecGroup {
    fn drop(&mut self) {
        // This id is the private group just created above, never the engine's.
        unsafe {
            libc::kill(-(self.0 as i32), libc::SIGKILL);
        }
    }
}
