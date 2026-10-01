//! Version-drift tolerance for engine payloads.
//!
//! The desktop engine ships far more often than the phone app. Everything
//! the direct backend reads from it goes through here first: a row or
//! transcript item that the app's types can't read is *repaired* (unknown
//! optional values dropped, unknown enum values mapped to a safe default,
//! unknown message kinds turned into a visible "unsupported" placeholder)
//! instead of failing the whole frame and leaving a blank list or chat.

use std::collections::HashSet;

use serde::de::DeserializeOwned;
use serde_json::{Map, Value};
use zeron_doc::{MessagePart, SessionMessageEntry};

/// Replace a value at a dotted path when present (`"config.sandbox"`).
pub(crate) type Substitution = (&'static str, Value);

/// Try `value` as `T`; on failure repair it: first the listed substitutions,
/// then dropping one nested object field, then one top-level field (never
/// `id`). Returns the row and what was changed (`None` = parsed as is).
pub(crate) fn decode_row<T: DeserializeOwned>(
    value: &Value,
    substitutions: &[Substitution],
) -> Result<(T, Option<String>), String> {
    let first_err = match serde_json::from_value::<T>(value.clone()) {
        Ok(row) => return Ok((row, None)),
        Err(err) => err.to_string(),
    };
    let Value::Object(map) = value else {
        return Err(first_err);
    };
    let base = map.clone();
    let mut substituted = map.clone();
    let mut notes = Vec::new();
    for (path, fallback) in substitutions {
        if let Some(slot) = pointer_mut(&mut substituted, path)
            && *slot != *fallback
        {
            *slot = fallback.clone();
            notes.push(format!("{path}→{fallback}"));
        }
    }
    if !notes.is_empty()
        && let Ok(row) = serde_json::from_value::<T>(Value::Object(substituted))
    {
        return Ok((row, Some(format!("replaced {}", notes.join(", ")))));
    }
    // Drop one nested field (an unknown enum inside an optional object).
    for (key, inner) in &base {
        let Value::Object(inner) = inner else {
            continue;
        };
        for nested in inner.keys() {
            let mut candidate = base.clone();
            if let Some(Value::Object(obj)) = candidate.get_mut(key) {
                obj.remove(nested);
            }
            if let Ok(row) = serde_json::from_value::<T>(Value::Object(candidate)) {
                return Ok((row, Some(format!("ignored {key}.{nested}"))));
            }
        }
    }
    // Drop one top-level field (optional fields default).
    for key in base.keys().filter(|k| k.as_str() != "id") {
        let mut candidate = base.clone();
        candidate.remove(key);
        if let Ok(row) = serde_json::from_value::<T>(Value::Object(candidate)) {
            return Ok((row, Some(format!("ignored {key}"))));
        }
    }
    Err(first_err)
}

fn pointer_mut<'a>(map: &'a mut Map<String, Value>, path: &str) -> Option<&'a mut Value> {
    let mut parts = path.split('.');
    let mut slot = map.get_mut(parts.next()?)?;
    for part in parts {
        slot = slot.as_object_mut()?.get_mut(part)?;
    }
    Some(slot)
}

/// A watch snapshot decoded row by row.
#[derive(Debug)]
pub(crate) struct DecodedRows<T> {
    pub(crate) rows: Vec<T>,
    /// Ids of every row in the frame, parsed or not (deletion guard).
    pub(crate) ids: HashSet<String>,
    /// Rows that could not be read even after repair.
    pub(crate) errors: Vec<String>,
    /// Rows kept after a repair (what was changed).
    pub(crate) repaired: Vec<String>,
}

/// Decode a registry snapshot leniently: a JSON array (or an object wrapping
/// exactly one array, for forward compatibility); rows are repaired where
/// possible and otherwise reported, never fatal.
pub(crate) fn decode_rows<T: DeserializeOwned>(
    value: Value,
    substitutions: &[Substitution],
) -> Result<DecodedRows<T>, String> {
    let items = match value {
        Value::Array(items) => items,
        Value::Object(mut map) => {
            let arrays: Vec<String> = map
                .iter()
                .filter(|(_, v)| v.is_array())
                .map(|(k, _)| k.clone())
                .collect();
            match arrays.as_slice() {
                [key] => match map.remove(key) {
                    Some(Value::Array(items)) => items,
                    _ => unreachable!("checked above"),
                },
                _ => return Err("expected a list of rows, got an object".into()),
            }
        }
        other => {
            return Err(format!(
                "expected a list of rows, got {}",
                json_kind(&other)
            ));
        }
    };
    let mut decoded = DecodedRows {
        rows: Vec::with_capacity(items.len()),
        ids: HashSet::with_capacity(items.len()),
        errors: Vec::new(),
        repaired: Vec::new(),
    };
    for item in items {
        let id = item.get("id").and_then(|v| v.as_str()).map(str::to_owned);
        if let Some(id) = &id {
            decoded.ids.insert(id.clone());
        }
        let label = match &id {
            Some(id) => format!("row {}", short_id(id)),
            None => "row without id".to_owned(),
        };
        match decode_row::<T>(&item, substitutions) {
            Ok((row, note)) => {
                decoded.rows.push(row);
                if let Some(note) = note {
                    decoded.repaired.push(format!("{label}: {note}"));
                }
            }
            Err(err) => decoded.errors.push(format!("{label}: {err}")),
        }
    }
    Ok(decoded)
}

/// Placeholder body for a message part this app version can't show.
pub(crate) fn unsupported_text(kind: &str) -> String {
    format!("_Unsupported content ({kind}). Update the app to view it._")
}

/// Make a `WatchDocMessages` update readable by this app version, in place.
/// Returns how many entries/parts were repaired.
pub(crate) fn sanitize_transcript_update(update: &mut Value) -> usize {
    let Value::Object(map) = update else { return 0 };
    let mut repaired = 0;
    if let Some(Value::Array(entries)) = map.get_mut("reset") {
        for (i, entry) in entries.iter_mut().enumerate() {
            repaired += sanitize_entry(entry, i);
        }
    }
    if let Some(Value::Array(upserts)) = map.get_mut("upsert") {
        for (i, upsert) in upserts.iter_mut().enumerate() {
            if let Some(entry) = upsert.get_mut("entry") {
                repaired += sanitize_entry(entry, i);
            }
        }
    }
    // Additive side channels: unreadable → absent.
    for key in ["contextUsage", "replayBaseline"] {
        let bad = match map.get(key) {
            None | Some(Value::Null) => false,
            Some(v) if key == "contextUsage" => {
                serde_json::from_value::<zeron_proto::ContextUsage>(v.clone()).is_err()
            }
            Some(v) => {
                serde_json::from_value::<zeron_doc::transcript_delta::TranscriptBaseline>(v.clone())
                    .is_err()
            }
        };
        if bad {
            map.remove(key);
            repaired += 1;
        }
    }
    repaired
}

fn sanitize_entry(entry: &mut Value, index: usize) -> usize {
    if serde_json::from_value::<SessionMessageEntry>(entry.clone()).is_ok() {
        return 0;
    }
    let mut repaired = 1;
    let Value::Object(map) = entry else {
        *entry = fallback_entry(&format!("unreadable-{index}"), "message");
        return repaired;
    };
    let id = map
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| format!("unreadable-{index}"));
    map.insert("id".into(), Value::String(id.clone()));
    if !matches!(
        map.get("role").and_then(Value::as_str),
        Some("user" | "assistant" | "system")
    ) {
        map.insert("role".into(), Value::String("assistant".into()));
    }
    if !matches!(map.get("status"), None | Some(Value::Null))
        && !matches!(
            map.get("status").and_then(Value::as_str),
            Some("streaming" | "complete" | "aborted")
        )
    {
        map.remove("status");
    }
    if !map.get("createdAt").is_some_and(Value::is_i64) {
        map.insert("createdAt".into(), Value::from(0));
    }
    if !map.get("deviceId").is_some_and(Value::is_string) {
        map.insert("deviceId".into(), Value::String(String::new()));
    }
    for key in ["continuationOf", "durationMs"] {
        let ok = match map.get(key) {
            None | Some(Value::Null) => true,
            Some(v) if key == "durationMs" => v.is_i64(),
            Some(v) => v.is_string(),
        };
        if !ok {
            map.remove(key);
        }
    }
    match map.get_mut("parts") {
        Some(Value::Array(parts)) => {
            for (i, part) in parts.iter_mut().enumerate() {
                if serde_json::from_value::<MessagePart>(part.clone()).is_err() {
                    *part = repair_part(part, &id, i);
                    repaired += 1;
                }
            }
        }
        _ => {
            map.insert("parts".into(), Value::Array(Vec::new()));
        }
    }
    if serde_json::from_value::<SessionMessageEntry>(entry.clone()).is_err() {
        *entry = fallback_entry(&id, "message");
    }
    repaired
}

fn repair_part(part: &Value, entry_id: &str, index: usize) -> Value {
    let kind = part
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_owned();
    let id = part
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{entry_id}-part-{index}"));
    let mut candidate = part.clone();
    if let Value::Object(map) = &mut candidate {
        map.insert("id".into(), Value::String(id.clone()));
        // A tool call of a kind this app doesn't know renders as a generic
        // tool chip named after it.
        if kind == "tool"
            && let Some(call) = map.get("call").cloned()
            && serde_json::from_value::<zeron_proto::ToolCall>(call.clone()).is_err()
        {
            let name = ["name", "tool", "kind"]
                .iter()
                .find_map(|k| call.get(*k).and_then(Value::as_str))
                .unwrap_or("tool")
                .to_owned();
            map.insert(
                "call".into(),
                serde_json::json!({ "kind": "unknown", "name": name, "input": call }),
            );
        }
    }
    if let Ok((part, _)) = decode_row::<MessagePart>(&candidate, &[]) {
        return serde_json::to_value(part).unwrap_or(Value::Null);
    }
    serde_json::json!({ "kind": "text", "id": id, "text": unsupported_text(&kind) })
}

fn fallback_entry(id: &str, kind: &str) -> Value {
    serde_json::json!({
        "id": id,
        "role": "assistant",
        "parts": [{ "kind": "text", "id": format!("{id}-unsupported"), "text": unsupported_text(kind) }],
        "createdAt": 0,
        "deviceId": "",
    })
}

pub(crate) fn json_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "a list",
        Value::Object(_) => "an object",
    }
}

pub(crate) fn short_id(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

/// `major.minor.patch` (leading `v` and any `-suffix` ignored).
pub(crate) fn parse_version(text: &str) -> Option<(u32, u32, u32)> {
    let core = text.trim().trim_start_matches('v');
    let core = core.split(['-', '+', ' ']).next()?;
    let mut it = core.split('.');
    let major = it.next()?.parse().ok()?;
    let minor = it.next().map_or(Some(0), |p| p.parse().ok())?;
    let patch = it.next().map_or(Some(0), |p| p.parse().ok())?;
    Some((major, minor, patch))
}
