//! Transcript delta application and replay marker extraction.
use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;
use sha2::{Digest, Sha256};

#[derive(Debug, Default, Clone)]
pub struct ChatView {
    pub entries: Vec<Value>,
    pub history_pending: bool,
}

impl ChatView {
    /// Apply a reset or delta, rejecting inconsistent counts and byte lengths.
    /// On error the caller must discard or reset this view.
    pub fn apply(&mut self, frame: &Value) -> Result<()> {
        let entries = &mut self.entries;
        if let Some(reset) = frame.get("reset") {
            *entries = reset.as_array().context("reset is not an array")?.clone();
        } else {
            for id in array(frame, "remove")? {
                entries.retain(|e| e.get("id") != Some(id));
            }
            for update in array(frame, "upsert")? {
                let entry = update.get("entry").context("upsert missing entry")?;
                let id = entry.get("id").context("upsert entry missing id")?;
                entries.retain(|e| e.get("id") != Some(id));
                let index = match update.get("after") {
                    None | Some(Value::Null) => 0,
                    Some(after) => entries
                        .iter()
                        .position(|e| e.get("id") == Some(after))
                        .map(|i| i + 1)
                        .unwrap_or(0),
                };
                entries.insert(index, entry.clone());
            }
            for append in array(frame, "append")? {
                let entry = entries
                    .iter_mut()
                    .find(|e| e.get("id") == append.get("entry"))
                    .context("append without entry")?;
                let part = entry
                    .get_mut("parts")
                    .and_then(Value::as_array_mut)
                    .context("entry without parts")?
                    .iter_mut()
                    .find(|p| p.get("id") == append.get("part"))
                    .context("append without part")?;
                let text = match part.get_mut("text") {
                    Some(Value::String(text)) => text,
                    _ => bail!("append without text part"),
                };
                let suffix = append
                    .get("text")
                    .and_then(Value::as_str)
                    .context("append without text")?;
                let expected = append
                    .get("len")
                    .and_then(Value::as_u64)
                    .context("append without length")?;
                ensure!(
                    expected == (text.len() + suffix.len()) as u64,
                    "append length mismatch"
                );
                text.push_str(suffix);
            }
        }
        if let Some(count) = frame.get("count") {
            ensure!(
                count.as_u64() == Some(entries.len() as u64),
                "transcript count mismatch"
            );
        } else if frame.get("reset").is_none() {
            bail!("delta missing count");
        }
        self.history_pending = frame
            .get("historyPending")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        Ok(())
    }

    /// SHA-256 of assistant text and reasoning parts, joined as in resource-profile.mjs.
    pub fn text_sha256(&self) -> String {
        format!("{:x}", Sha256::digest(self.text().as_bytes()))
    }
    pub fn text(&self) -> String {
        self.entries
            .iter()
            .filter(|e| e.get("role").and_then(Value::as_str) == Some("assistant"))
            .flat_map(|e| {
                e.get("parts")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
            })
            .filter(|p| {
                matches!(
                    p.get("kind").and_then(Value::as_str),
                    Some("text" | "reasoning")
                )
            })
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}
fn array<'a>(frame: &'a Value, key: &str) -> Result<&'a [Value]> {
    match frame.get(key) {
        None | Some(Value::Null) => Ok(&[]),
        Some(v) => Ok(v
            .as_array()
            .with_context(|| format!("{key} is not an array"))?),
    }
}

/// Scan received payloads for ASCII replay markers, retaining their receive time.
#[derive(Debug, Default)]
pub struct MarkerScan {
    seen: std::collections::HashSet<(String, u64)>,
}
impl MarkerScan {
    pub fn scan(&mut self, payload: &[u8], t_recv_ns: u64) -> Vec<(String, u64, u64)> {
        let text = String::from_utf8_lossy(payload);
        let mut result = Vec::new();
        for tail in text.split("[[zb:").skip(1) {
            let Some((body, _)) = tail.split_once("]]") else {
                continue;
            };
            let Some((chat, seq)) = body.split_once(':') else {
                continue;
            };
            if chat.len() == 8
                && chat.bytes().all(|b| b.is_ascii_hexdigit())
                && let Ok(seq) = seq.parse()
            {
                if self.seen.insert((chat.to_owned(), seq)) {
                    result.push((chat.to_owned(), seq, t_recv_ns));
                }
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn recorded_engine_frames_replay() {
        let frames: Vec<Value> = serde_json::from_str(include_str!(
            "../tests/fixtures/mock-transcript-frames.json"
        ))
        .unwrap();
        assert_eq!(frames[0]["historyPending"], true);
        assert!(frames[1].get("reset").is_some());
        assert!(
            frames
                .iter()
                .any(|f| f["upsert"].as_array().is_some_and(|v| !v.is_empty()))
        );
        assert!(
            frames
                .iter()
                .any(|f| f["append"].as_array().is_some_and(|v| !v.is_empty()))
        );
        let mut view = ChatView::default();
        for frame in &frames {
            view.apply(frame).unwrap();
        }
        assert_eq!(
            view.text_sha256(),
            "8f2eeae260860c647d38fba1e62be684ee29670c1c81adcd0bca9c58d3668817"
        );
    }
    #[test]
    fn recorded_delta_sequence_and_preview() {
        let mut v = ChatView::default();
        v.apply(&json!({"reset":[{"id":"tail","role":"assistant","parts":[]}],"historyPending":true,"count":1})).unwrap();
        assert!(v.history_pending);
        v.apply(&json!({"reset":[{"id":"a","role":"assistant","parts":[{"id":"p","kind":"text","text":"hi"}]}],"count":1})).unwrap();
        v.apply(&json!({"upsert":[{"entry":{"id":"b","parts":[]},"after":"a"}],"count":2}))
            .unwrap();
        v.apply(&json!({"append":[{"entry":"a","part":"p","text":" é","len":5}],"remove":["b"],"count":1})).unwrap();
        assert_eq!(v.text(), "hi é");
        assert_eq!(
            v.text_sha256(),
            format!("{:x}", Sha256::digest("hi é".as_bytes()))
        );
        assert!(
            v.apply(&json!({"append":[{"entry":"a","part":"p","text":"!","len":99}],"count":1}))
                .is_err()
        );
        assert_eq!(v.text(), "hi é");
        assert!(v.apply(&json!({"count":2})).is_err());
        assert_eq!(
            MarkerScan::default().scan(b"x [[zb:abc01234:12]]", 42),
            vec![("abc01234".into(), 12, 42)]
        );
    }
}
