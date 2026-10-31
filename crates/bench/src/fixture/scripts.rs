//! Replay script generation and conversion.
use crate::replay::script::text_sha256;
use anyhow::{Context, Result};
use serde_json::{Value, json};

pub fn from_text(name: &str, seed: u64, text: &str, cadence: &str) -> Result<String> {
    let (chunk, dt) = match cadence {
        "opus" => (9, 35),
        "haiku" => (14, 20),
        "burst" => (4096, 0),
        _ => anyhow::bail!("unknown cadence {cadence}"),
    };
    let mut frames = vec![
        json!({"dt":0,"f":{"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}}}),
    ];
    let mut start = 0;
    while start < text.len() {
        let mut end = (start + chunk).min(text.len());
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
        frames.push(json!({"dt":dt,"f":{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":&text[start..end]}}}}));
        start = end;
    }
    frames.push(
        json!({"dt":0,"f":{"type":"stream_event","event":{"type":"content_block_stop","index":0}}}),
    );
    encode(name, seed, text, frames)
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
        let value: Value = serde_json::from_str(line).with_context(|| format!("line {}", i + 1))?;
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
}
