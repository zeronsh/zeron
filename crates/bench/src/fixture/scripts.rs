//! Replay script generation and conversion.
use crate::replay::script::text_sha256;
use anyhow::{Context, Result};
use rand_chacha::{
    ChaCha8Rng,
    rand_core::{RngCore, SeedableRng},
};
use serde_json::{Value, json};

pub fn from_text(name: &str, seed: u64, text: &str, cadence: &str) -> Result<String> {
    make_script(name, seed, text, cadence, false)
}

pub fn crash_script(seed: u64, text: &str) -> Result<String> {
    make_script("crash", seed, text, "opus", true)
}

fn make_script(name: &str, seed: u64, text: &str, cadence: &str, crash: bool) -> Result<String> {
    let (chunk, dt) = match cadence {
        "opus" => (9, 35),
        "haiku" => (14, 20),
        "burst" => (4096, 0),
        _ => anyhow::bail!("unknown cadence {cadence}"),
    };
    let names = ["Read", "Grep", "Edit", "Bash", "Write"];
    let tool = names[(seed as usize) % names.len()];
    let input = match tool {
        "Read" => json!({"file_path":"src/main.rs"}),
        "Grep" => json!({"pattern":"fixture","path":"."}),
        "Edit" => json!({"file_path":"src/main.rs","old_string":"old","new_string":"new"}),
        "Bash" => json!({"command":"printf ready"}),
        _ => json!({"file_path":"notes.md","content":"fixture"}),
    };
    let partial = serde_json::to_string(&input)?;
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let u = (f64::from(rng.next_u32()) + 1.0) / (f64::from(u32::MAX) + 2.0);
    let v = (f64::from(rng.next_u32()) + 1.0) / (f64::from(u32::MAX) + 2.0);
    let z = (-2.0 * u.ln()).sqrt() * (std::f64::consts::TAU * v).cos();
    let output_bytes = (2000.0_f64.ln() + 1.82 * z).exp().clamp(1.0, 200_000.0) as usize;
    let output = "x".repeat(output_bytes);
    let tool_pause = if crash {
        dt
    } else {
        match cadence {
            "opus" => 500 + u64::from(rng.next_u32() % 5501),
            "haiku" => 300 + u64::from(rng.next_u32() % 2701),
            _ => 0,
        }
    };
    let mut frames = vec![
        json!({"dt":0,"f":{"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}}}),
        json!({"dt":dt,"f":{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Inspect the relevant files and summarize the result."}}}}),
        json!({"dt":0,"f":{"type":"stream_event","event":{"type":"content_block_stop","index":0}}}),
        json!({"dt":dt,"f":{"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"$TOOL_ID","name":tool,"input":{}}}}}),
        json!({"dt":dt,"f":{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":partial}}}}),
        json!({"dt":0,"f":{"type":"stream_event","event":{"type":"content_block_stop","index":0}}}),
        json!({"dt":tool_pause,"f":{"type":"user","session_id":"$SESSION_ID","parent_tool_use_id":null,"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"$TOOL_ID","content":output}]}}}),
        json!({"dt":0,"f":{"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}}}),
    ];
    let mut start = 0;
    while start < text.len() {
        let length = if cadence == "burst" {
            chunk
        } else {
            let u = (f64::from(rng.next_u32()) + 1.0) / (f64::from(u32::MAX) + 2.0);
            let v = (f64::from(rng.next_u32()) + 1.0) / (f64::from(u32::MAX) + 2.0);
            let z = (-2.0 * u.ln()).sqrt() * (std::f64::consts::TAU * v).cos();
            ((chunk as f64 * (0.65 * z).exp()) as usize).clamp(1, 40)
        };
        let delay = if dt == 0 {
            0
        } else {
            dt * (70 + (rng.next_u32() % 61) as u64) / 100
        };
        let mut end = (start + length).min(text.len());
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        if end == start {
            end = text[start..]
                .char_indices()
                .nth(1)
                .map(|(i, _)| start + i)
                .unwrap_or(text.len());
        }
        frames.push(json!({"dt":delay,"f":{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":&text[start..end]}}}}));
        start = end;
    }
    frames.push(
        json!({"dt":0,"f":{"type":"stream_event","event":{"type":"content_block_stop","index":0}}}),
    );
    encode(name, seed, text, frames)
}

/// Replay a checked-in Claude subagent exchange without contacting a provider.
pub fn subagent_script() -> Result<String> {
    let recording = include_str!(
        "../../../harness/tests/fixtures/claude/live-2.1.310-foreground-subagent-forwarded.jsonl"
    );
    let mut frames = Vec::new();
    let mut text = String::new();
    for line in recording.lines() {
        let mut frame: Value = serde_json::from_str(line)?;
        if frame["type"] == "system" && (frame["subtype"] == "init" || frame["subtype"] == "status")
        {
            continue;
        }
        if frame["type"] == "user"
            && frame["parent_tool_use_id"].is_null()
            && frame["message"]["content"].as_str().is_some()
        {
            continue;
        }
        if frame["type"] == "stream_event"
            && frame["parent_tool_use_id"].is_null()
            && frame["event"]["delta"]["type"] == "text_delta"
        {
            text.push_str(frame["event"]["delta"]["text"].as_str().unwrap_or(""));
        }
        if frame.get("session_id").is_some() {
            frame["session_id"] = "$SESSION_ID".into();
        }
        frames.push(json!({"dt":0,"f":frame}));
    }
    encode("subagent", 0, &text, frames)
}

/// A replay corpus retaining each frame type in the checked-in live recordings.
pub fn conformance_script() -> Result<String> {
    let recordings = [
        include_str!(
            "../../../harness/tests/fixtures/claude/live-2.1.228-background-subagent.jsonl"
        ),
        include_str!(
            "../../../harness/tests/fixtures/claude/live-2.1.310-foreground-subagent-forwarded.jsonl"
        ),
        include_str!("../../../harness/tests/fixtures/claude/live-2.1.310-forked-skill.jsonl"),
        include_str!(
            "../../../harness/tests/fixtures/claude/live-2.1.310-typed-fork-slow-steps-transcript.jsonl"
        ),
        include_str!(
            "../../../harness/tests/fixtures/claude/live-2.1.310-typed-fork-slow-steps.jsonl"
        ),
        include_str!(
            "../../../harness/tests/fixtures/claude/live-2.1.310-typed-forked-command.jsonl"
        ),
    ];
    let mut frames = Vec::new();
    let mut text = String::new();
    for recording in recordings {
        for line in recording.lines() {
            let mut frame: Value = serde_json::from_str(line)?;
            if frame["type"] == "stream_event"
                && frame["parent_tool_use_id"].is_null()
                && frame["event"]["delta"]["type"] == "text_delta"
            {
                text.push_str(frame["event"]["delta"]["text"].as_str().unwrap_or(""));
            }
            if frame.get("session_id").is_some() {
                frame["session_id"] = "$SESSION_ID".into();
            }
            frames.push(json!({"dt":0,"f":frame}));
        }
    }
    encode("conformance", 0, &text, frames)
}

fn encode(name: &str, seed: u64, text: &str, frames: Vec<Value>) -> Result<String> {
    let mut out = serde_json::to_string(
        &json!({"zb":"meta","v":1,"name":name,"seed":seed,"text_bytes":text.len(),"text_sha256":text_sha256(text),"frames":frames.len()}),
    )?;
    out.push('\n');
    for frame in frames {
        out.push_str(&serde_json::to_string(&frame)?);
        out.push('\n');
    }
    out.push_str("{\"dt\":0,\"zb\":\"turn_end\",\"usage\":{}}\n");
    Ok(out)
}

pub fn convert(input: &str, cadence: &str) -> Result<String> {
    let (chunk, dt) = match cadence {
        "opus" => (9, 35),
        "haiku" => (14, 20),
        "burst" => (4096, 0),
        _ => anyhow::bail!("unknown cadence {cadence}"),
    };
    let mut text = String::new();
    let mut frames = Vec::new();
    for (i, line) in input.lines().enumerate() {
        let mut value: Value =
            serde_json::from_str(line).with_context(|| format!("line {}", i + 1))?;
        if value.get("session_id").is_some() {
            value["session_id"] = "$SESSION_ID".into();
        }
        if value.get("sessionId").is_some() {
            value["sessionId"] = "$SESSION_ID".into();
        }
        match value["type"].as_str() {
            Some("assistant") if value["parent_tool_use_id"].is_null() => {
                if let Some(content) = value["message"]["content"].as_array() {
                    for (index, block) in content.iter().enumerate() {
                        match block["type"].as_str() {
                            Some("text") => {
                                let body = block["text"].as_str().unwrap_or("");
                                text.push_str(body);
                                frames.push(json!({"dt":0,"f":{"type":"stream_event","event":{"type":"content_block_start","index":index,"content_block":{"type":"text","text":""}}}}));
                                let mut start = 0;
                                while start < body.len() {
                                    let mut end = (start + chunk).min(body.len());
                                    while !body.is_char_boundary(end) {
                                        end -= 1;
                                    }
                                    if end == start {
                                        end = body[start..]
                                            .char_indices()
                                            .nth(1)
                                            .map(|(i, _)| start + i)
                                            .unwrap_or(body.len());
                                    }
                                    frames.push(json!({"dt":dt,"f":{"type":"stream_event","event":{"type":"content_block_delta","index":index,"delta":{"type":"text_delta","text":&body[start..end]}}}}));
                                    start = end;
                                }
                                frames.push(json!({"dt":0,"f":{"type":"stream_event","event":{"type":"content_block_stop","index":index}}}));
                            }
                            _ => {}
                        }
                    }
                }
                frames.push(json!({"dt":0,"f":value}));
            }
            Some("user")
                if value["message"]["content"]
                    .as_array()
                    .is_some_and(|v| v.iter().any(|b| b["type"] == "tool_result")) =>
            {
                frames.push(json!({"dt":0,"f":value}))
            }
            _ => {}
        }
    }
    encode("converted", 0, &text, frames)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deterministic_and_parseable() {
        let text = crate::fixture::text::generate(42, 100_000);
        let a = from_text("test", 42, &text, "opus").unwrap();
        assert_eq!(a, from_text("test", 42, &text, "opus").unwrap());
        let script = crate::replay::script::Script::parse(&a).unwrap();
        assert_eq!(script.meta.text_bytes, 100_000);
        assert_eq!(script.meta.text_sha256, text_sha256(&text));
    }
    #[test]
    fn conversion_preserves_tool_frames() {
        let input = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hello"},{"type":"tool_use","id":"x","name":"Read","input":{"file_path":"a"}}]}}
{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"x","content":"data"}]}}"#;
        let output = convert(input, "opus").unwrap();
        let parsed = crate::replay::script::Script::parse(&output).unwrap();
        assert_eq!(parsed.meta.text_bytes, 5);
        assert!(output.contains("tool_use"));
        assert!(output.contains("tool_result"));
    }
    #[test]
    fn conformance_script_preserves_recorded_frame_types() {
        let script = crate::replay::script::Script::parse(&conformance_script().unwrap()).unwrap();
        let types: std::collections::BTreeSet<_> = script
            .steps
            .iter()
            .filter_map(|step| match step {
                crate::replay::script::Step::Frame { f, .. } => f["type"].as_str(),
                _ => None,
            })
            .collect();
        for kind in [
            "system",
            "rate_limit_event",
            "stream_event",
            "assistant",
            "user",
            "result",
            "control_request",
            "attachment",
            "api-request-blob",
        ] {
            assert!(types.contains(kind), "missing {kind}");
        }
    }
}
