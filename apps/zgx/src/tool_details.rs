//! What a tool row opens to, as the shipping transcript's (crates/ui
//! transcript.rs `tool_detail`, `blob_detail`): an edit's diff, a command's
//! output, a read's or a search's result.
//!
//! The chat's doc keeps little of a tool (chat2-sync): a one-line summary of
//! its output, each file's lines added and removed, and refs to the whole
//! output and the whole diff in the sidecar, fetched only when a reader asks
//! (`FetchToolBlob`). Older docs carry the diff itself, drawn as it is.
//!
//!   chat.tool_blob { ref }    the sidecar's: { kind = "diff", path, lines,
//!                             added, removed, more } for a `….diff` ref, else
//!                             { kind = "output", lines, more }
//!   chat.tool_output { chat, id }
//!                             a command's whole output, from the device that
//!                             ran it (`ReadToolOutput`, redacted there as the
//!                             doc's tail is): { kind = "output", text,
//!                             trimmed, exit_code }; failing, one word:
//!                             `offline`, `update` (that device's engine is
//!                             too old), `unavailable` (not kept), `running`,
//!                             `failed`
//!
//! The doc keeps a command's last ten lines (`outputTruncated` when there
//! were more, with `outputLines`, the whole output's count, and its
//! `exitCode`); its transcript row names the doc it is in (`chat`, stamped
//! by src/chat.rs) so "Show all N lines" can ask that doc's host.
//!
//! A diff's lines are the changes pane's: hunks with three lines of context
//! round each change, each line its kind (" ", "+", "-", "hunk"), its old
//! and new numbers, its text.

use serde_json::{Value as Json, json};
use zgx_script::{ScriptHost, Value};

use crate::engine;

/// Lines an inline or fetched diff shows (the shipping app's
/// `DIFF_DETAIL_MAX_LINES`), and a fetched output (`FULL_OUTPUT_MAX_LINES`).
pub const DIFF_MAX_LINES: usize = 600;
pub const OUTPUT_MAX_LINES: usize = 400;
/// Characters kept of one output line (`OUTPUT_LINE_MAX_CHARS`).
const LINE_MAX_CHARS: usize = 8 * 1024;

/// Chunks a whole output may take: the engine serves at most its last MB,
/// 48 KB at a time (`TOOL_OUTPUT_MAX_BYTES`, `TOOL_OUTPUT_CHUNK_BYTES`).
const OUTPUT_MAX_CHUNKS: usize = 64;

pub fn install(host: &ScriptHost) {
    host.async_action("chat.tool_output", |arg| async move {
        let doc = arg.field("chat").map(crate::auth::text).unwrap_or_default();
        let id = arg.field("id").map(crate::auth::text).unwrap_or_default();
        if doc.is_empty() || id.is_empty() {
            return Err("failed".into());
        }
        let mut params = json!({ "chatId": doc, "toolId": id, "offset": 0 });
        if let Some(device) = crate::chat::host_of(&doc) {
            params["targetDeviceId"] = Json::String(device);
        }
        let mut text = String::new();
        for _ in 0..OUTPUT_MAX_CHUNKS {
            let chunk = engine::get().call("ReadToolOutput", params.clone()).await.map_err(|e| output_error(&e))?;
            text.push_str(chunk["text"].as_str().unwrap_or_default());
            if chunk["done"].as_bool() == Some(true) {
                return Ok(whole_output_value(&text, &chunk));
            }
            let next = chunk["nextOffset"].as_u64().unwrap_or(0);
            if next <= params["offset"].as_u64().unwrap_or(0) {
                break;
            }
            params["offset"] = json!(next);
        }
        Err("failed".into())
    });

    host.async_action("chat.tool_blob", |arg| async move {
        let blob = arg.field("ref").map(crate::auth::text).unwrap_or_default();
        if blob.is_empty() {
            return Err("no ref".into());
        }
        let reply = engine::get().call("FetchToolBlob", json!({ "blobRef": blob })).await?;
        let text = reply["text"].as_str().unwrap_or_default();
        blob_value(&blob, text).ok_or_else(|| "Nothing to show".to_string())
    });
}

/// A whole output read from its host, as the row shows it (its text, made
/// clean by ds/tool_detail as the doc's is).
fn whole_output_value(text: &str, last: &Json) -> Value {
    let mut fields = vec![
        ("kind", Value::from("output")),
        ("text", Value::from(text)),
        ("trimmed", Value::Bool(last["trimmed"].as_bool() == Some(true))),
    ];
    if let Some(code) = last["exitCode"].as_i64() {
        fields.push(("exit_code", Value::Num(code as f64)));
    }
    Value::record(fields)
}

/// A failed whole-output read as the one word ds/tool_detail words for the
/// reader.
fn output_error(error: &str) -> String {
    let word = if engine::outcome(error) == "offline" {
        "offline"
    } else if error.to_ascii_lowercase().contains("unknown method") {
        "update"
    } else if error.contains("tool_output.unavailable") || error.contains("chat not found") {
        "unavailable"
    } else if error.contains("tool_output.not_finished") {
        "running"
    } else {
        "failed"
    };
    word.to_owned()
}

/// A fetched sidecar blob, as the row shows it.
fn blob_value(blob: &str, text: &str) -> Option<Value> {
    if blob.ends_with(".diff") {
        let diff: Json = serde_json::from_str(text).ok()?;
        return diff_value(&diff);
    }
    output_value(text, OUTPUT_MAX_LINES)
}

/// An output's lines, trailing blank lines dropped, at most `max` (and how
/// many more there were).
pub fn output_value(text: &str, max: usize) -> Option<Value> {
    let mut lines: Vec<String> = text.lines().map(cut).collect();
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    if lines.is_empty() {
        return None;
    }
    let more = lines.len().saturating_sub(max);
    lines.truncate(max);
    Some(Value::record([
        ("kind", Value::from("output")),
        ("lines", Value::List(lines.iter().map(|l| Value::from(l.as_str())).collect::<Vec<_>>().into())),
        ("more", Value::Num(more as f64)),
    ]))
}

fn cut(line: &str) -> String {
    match line.char_indices().nth(LINE_MAX_CHARS) {
        Some((at, _)) => format!("{}…", &line[..at]),
        None => line.to_owned(),
    }
}

#[derive(Debug, Clone, PartialEq)]
struct Line {
    kind: &'static str,
    old: Option<usize>,
    new: Option<usize>,
    text: String,
}

/// A file's diff (`ToolDiff`: `{ path, oldText?, newText }`) as lines.
pub fn diff_value(diff: &Json) -> Option<Value> {
    let path = diff["path"].as_str().unwrap_or_default();
    let old = diff["oldText"].as_str().unwrap_or("");
    let new = diff["newText"].as_str()?;
    let (mut lines, added, removed) = diff_lines(old, new);
    if lines.is_empty() {
        return None;
    }
    let more = lines.len().saturating_sub(DIFF_MAX_LINES);
    lines.truncate(DIFF_MAX_LINES);
    let num = |n: Option<usize>| n.map_or(Value::Nil, |n| Value::Num(n as f64));
    Some(Value::record([
        ("kind", Value::from("diff")),
        ("path", Value::from(path)),
        (
            "lines",
            Value::List(
                lines
                    .into_iter()
                    .map(|l| {
                        Value::record([
                            ("kind", Value::from(l.kind)),
                            ("old", num(l.old)),
                            ("new", num(l.new)),
                            ("text", Value::from(l.text.as_str())),
                        ])
                    })
                    .collect::<Vec<_>>()
                    .into(),
            ),
        ),
        ("added", Value::Num(added as f64)),
        ("removed", Value::Num(removed as f64)),
        ("more", Value::Num(more as f64)),
    ]))
}

/// The changes pane's hunks: three lines of context round each change, a
/// header for each hunk, numbers from 1.
fn diff_lines(old: &str, new: &str) -> (Vec<Line>, usize, usize) {
    let diff = similar::TextDiff::from_lines(old, new);
    let mut out = Vec::new();
    let (mut added, mut removed) = (0, 0);
    for group in diff.grouped_ops(3) {
        let (Some(first), Some(last)) = (group.first(), group.last()) else {
            continue;
        };
        let (o, n) = (first.old_range().start, first.new_range().start);
        let (ol, nl) = (last.old_range().end - o, last.new_range().end - n);
        out.push(Line { kind: "hunk", old: None, new: None, text: format!("@@ -{},{ol} +{},{nl} @@", o + 1, n + 1) });
        for op in &group {
            for change in diff.iter_changes(op) {
                let text = change.value().trim_end_matches(['\n', '\r']).to_string();
                let (kind, old, new) = match change.tag() {
                    similar::ChangeTag::Equal => (" ", change.old_index().map(|i| i + 1), change.new_index().map(|i| i + 1)),
                    similar::ChangeTag::Delete => {
                        removed += 1;
                        ("-", change.old_index().map(|i| i + 1), None)
                    }
                    similar::ChangeTag::Insert => {
                        added += 1;
                        ("+", None, change.new_index().map(|i| i + 1))
                    }
                };
                out.push(Line { kind, old, new, text });
            }
        }
    }
    (out, added, removed)
}

/// What a tool part carries for its row to open to, beyond its line: its
/// output's summary, its files' counts, the sidecar's refs, and an older
/// doc's diff itself.
pub fn tool_fields(part: &Json) -> Vec<(&'static str, Value)> {
    let mut fields = Vec::new();
    if let Some(output) = part["output"].as_str().filter(|o| !o.trim().is_empty()) {
        fields.push(("output", Value::from(output)));
    }
    let text = |k: &str| part[k].as_str().filter(|v| !v.is_empty());
    if let Some(r) = text("outputRef") {
        fields.push(("output_ref", Value::from(r)));
    }
    if let Some(n) = part["outputBytes"].as_u64() {
        fields.push(("output_bytes", Value::Num(n as f64)));
    }
    if let Some(code) = part["exitCode"].as_i64() {
        fields.push(("exit_code", Value::Num(code as f64)));
    }
    // The doc kept only the end of a command's output: how many lines the
    // whole has, and the doc to ask its host for it in.
    if part["outputTruncated"].as_bool() == Some(true) {
        fields.push(("output_truncated", Value::Bool(true)));
        if let Some(n) = part["outputLines"].as_u64() {
            fields.push(("output_lines", Value::Num(n as f64)));
        }
        if let Some(doc) = text("chat") {
            fields.push(("chat", Value::from(doc)));
        }
    }
    if let Some(r) = text("diffRef") {
        fields.push(("diff_ref", Value::from(r)));
    }
    if let Some(stats) = part["diffStats"].as_array().filter(|s| !s.is_empty()) {
        let rows: Vec<Value> = stats
            .iter()
            .map(|s| {
                Value::record([
                    ("path", Value::from(s["path"].as_str().unwrap_or_default())),
                    ("added", Value::Num(s["additions"].as_f64().unwrap_or(0.0))),
                    ("removed", Value::Num(s["deletions"].as_f64().unwrap_or(0.0))),
                ])
            })
            .collect();
        fields.push(("stats", Value::List(rows.into())));
    }
    if part["diff"].is_object()
        && let Some(diff) = diff_value(&part["diff"])
    {
        fields.push(("diff", diff));
    }
    fields
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_diff_is_hunks_with_three_lines_of_context_numbered_from_one() {
        let old = (1..=20).map(|i| format!("line {i}\n")).collect::<String>();
        let new = old.replace("line 10\n", "line ten\nline 10.5\n");
        let (lines, added, removed) = diff_lines(&old, &new);
        assert_eq!((added, removed), (2, 1));
        assert_eq!(lines[0].kind, "hunk");
        assert_eq!(lines[0].text, "@@ -7,7 +7,8 @@");
        assert_eq!(lines[1], Line { kind: " ", old: Some(7), new: Some(7), text: "line 7".into() });
        let minus = lines.iter().find(|l| l.kind == "-").unwrap();
        assert_eq!((minus.old, minus.new, minus.text.as_str()), (Some(10), None, "line 10"));
        let plus: Vec<_> = lines.iter().filter(|l| l.kind == "+").map(|l| (l.new, l.text.as_str())).collect();
        assert_eq!(plus, vec![(Some(10), "line ten"), (Some(11), "line 10.5")]);
        assert_eq!(lines.last().unwrap().text, "line 13");
    }

    #[test]
    fn output_drops_trailing_blanks_and_counts_what_it_leaves_out() {
        let text = (1..=30).map(|i| format!("{i}\n")).collect::<String>() + "\n\n";
        let Some(Value::Map(fields)) = output_value(&text, 24) else { panic!() };
        let get = |k: &str| fields.iter().find(|(n, _)| &**n == k).map(|(_, v)| v.clone()).unwrap();
        let Value::List(lines) = get("lines") else { panic!() };
        assert_eq!(lines.len(), 24);
        assert!(matches!(get("more"), Value::Num(n) if n == 6.0));
        assert!(output_value("\n \n", 24).is_none());
    }

    #[test]
    fn a_part_brings_its_summary_counts_and_refs() {
        let part = json!({
            "output": "ok",
            "outputRef": "c/p1",
            "outputBytes": 12345,
            "diffRef": "c/p1.diff",
            "diffStats": [{ "path": "src/a.rs", "additions": 3, "deletions": 1 }],
        });
        let names: Vec<_> = tool_fields(&part).into_iter().map(|(k, _)| k).collect();
        assert_eq!(names, vec!["output", "output_ref", "output_bytes", "diff_ref", "stats"]);
        let old = json!({ "diff": { "path": "a.txt", "oldText": "a\n", "newText": "b\n" } });
        assert_eq!(tool_fields(&old).into_iter().map(|(k, _)| k).collect::<Vec<_>>(), vec!["diff"]);
    }

    #[test]
    fn a_failed_whole_output_read_is_one_word() {
        for (error, word) in [
            ("offline", "offline"),
            ("relay: device unreachable", "offline"),
            ("unknown method: ReadToolOutput", "update"),
            ("capability error: tool_output.unavailable", "unavailable"),
            ("bad params: chat not found", "unavailable"),
            ("capability error: tool_output.not_finished", "running"),
            ("bad params: Only a command's output can be shown in full.", "failed"),
        ] {
            assert_eq!(output_error(error), word, "{error}");
        }
    }

    #[test]
    fn a_whole_output_brings_its_text_trim_and_exit_code() {
        let value = whole_output_value("a\nb", &json!({ "done": true, "trimmed": true, "exitCode": 1 }));
        assert_eq!(value.field("text"), Some(&Value::from("a\nb")));
        assert_eq!(value.field("trimmed"), Some(&Value::Bool(true)));
        assert!(matches!(value.field("exit_code"), Some(Value::Num(n)) if *n == 1.0));
    }
}
