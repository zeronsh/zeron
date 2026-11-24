//! A released build (`main` as of v0.2.120) reads this build's transcript
//! frames: `MessagePart::Injected` / `MessagePart::CrewReports` go on the
//! wire as text parts with no text, which its `Text` takes (ignoring the
//! field), where `kind: "injected"` failed its whole frame ("bad frame":
//! Zeron MCP's `read_chat`/`wait_for_turn`, the client's subagent viewer).
//!
//! `released` is main's definitions, copied as they are there
//! (crates/doc/src/parts.rs, schema.rs, transcript_delta.rs).

use serde_json::json;
use zeron_doc::{
    Injected, MessagePart, MessageRole, MessageStatus, PartPatch, SessionMessageEntry, TranscriptFrame,
    TranscriptUpsert,
};

mod released {
    use serde::{Deserialize, Serialize};
    use zeron_doc::{MessageRole, MessageStatus, SubagentStatus, ToolDiffStat};
    use zeron_proto::{ToolCall, ToolDiff, ToolReview, UserInputAnswer, UserInputQuestion};

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(tag = "kind", rename_all = "camelCase")]
    pub enum MessagePart {
        Text {
            id: String,
            text: String,
        },
        #[serde(rename_all = "camelCase")]
        Image {
            id: String,
            path: String,
            name: String,
            mime_type: String,
        },
        Visual {
            id: String,
            path: String,
            title: String,
        },
        Reasoning {
            id: String,
            text: String,
        },
        #[serde(rename_all = "camelCase")]
        Tool {
            id: String,
            call: ToolCall,
            #[serde(default)]
            is_error: bool,
            #[serde(default)]
            resolved: bool,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            output: Option<String>,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            diff: Option<ToolDiff>,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            output_ref: Option<String>,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            output_bytes: Option<u64>,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            diff_ref: Option<String>,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            diff_stats: Option<Vec<ToolDiffStat>>,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            subagent_ref: Option<String>,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            subagent_status: Option<SubagentStatus>,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            subagent_tail: Option<String>,
            #[serde(default, skip_serializing_if = "Vec::is_empty")]
            preview_paths: Vec<String>,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            exit_code: Option<i32>,
            #[serde(default, skip_serializing_if = "std::ops::Not::not")]
            output_truncated: bool,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            output_lines: Option<u64>,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            review: Option<ToolReview>,
        },
        #[serde(rename_all = "camelCase")]
        Input {
            id: String,
            request_id: String,
            questions: Vec<UserInputQuestion>,
            #[serde(default)]
            resolved: bool,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            answers: Option<Vec<UserInputAnswer>>,
        },
        Error {
            id: String,
            message: String,
        },
        #[serde(rename_all = "camelCase")]
        Fork {
            id: String,
            source_chat_id: String,
            source_title: String,
        },
        #[serde(rename_all = "camelCase")]
        Switch {
            id: String,
            from: String,
            to: String,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            from_harness: Option<String>,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            to_harness: Option<String>,
        },
    }

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct SessionMessageEntry {
        pub id: String,
        pub role: MessageRole,
        pub parts: Vec<MessagePart>,
        pub created_at: i64,
        pub device_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub status: Option<MessageStatus>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub continuation_of: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub duration_ms: Option<i64>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(untagged)]
    pub enum TranscriptFrame {
        Reset {
            reset: Vec<SessionMessageEntry>,
        },
        Delta {
            #[serde(default)]
            upsert: Vec<TranscriptUpsert>,
            #[serde(default)]
            append: Vec<TextAppend>,
            #[serde(default)]
            remove: Vec<String>,
            #[serde(default, skip_serializing_if = "Vec::is_empty")]
            parts: Vec<PartPatch>,
            count: usize,
        },
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct PartPatch {
        pub entry: String,
        pub at: usize,
        pub part: MessagePart,
        pub len: usize,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct TranscriptUpsert {
        pub after: Option<String>,
        pub entry: SessionMessageEntry,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct TextAppend {
        pub entry: String,
        pub part: String,
        pub text: String,
        pub len: usize,
    }
}

fn report(id: &str) -> zeron_proto::orchestration::Report {
    serde_json::from_value(json!({
        "id": id,
        "orchestratorId": "orch-1",
        "fromChat": "thread-1",
        "itemId": "card-1",
        "state": "done",
        "summary": "Shipped the fix",
        "detail": "All green.",
        "progress": 1.0,
        "at": "2026-11-16T18:37:00Z",
    }))
    .unwrap()
}

fn said() -> Injected {
    Injected::answer("ak-1", "thread-1", Some("m-9"), true)
}

fn user(id: &str, parts: Vec<MessagePart>) -> SessionMessageEntry {
    SessionMessageEntry {
        id: id.into(),
        role: MessageRole::User,
        parts,
        created_at: 1_700_000_000_000,
        device_id: "dev-1".into(),
        status: Some(MessageStatus::Complete),
        continuation_of: None,
        duration_ms: None,
    }
}

/// An answer delivered into a thread, and a Crew updates message, as the
/// engine writes them.
fn entries() -> Vec<SessionMessageEntry> {
    vec![
        user(
            "u-1",
            vec![
                MessagePart::Text { id: "t0".into(), text: "[Answer from your orchestrator] Yes".into() },
                MessagePart::Injected { id: "injected".into(), injected: said() },
            ],
        ),
        user(
            "u-2",
            vec![
                MessagePart::Text { id: "t0".into(), text: zeron_doc::crew_reports_fallback_text(&[report("r-1")]) },
                MessagePart::CrewReports { id: "crew".into(), reports: vec![report("r-1"), report("r-2")] },
            ],
        ),
    ]
}

fn empty_text(id: &str) -> released::MessagePart {
    released::MessagePart::Text { id: id.into(), text: String::new() }
}

/// What a released build makes of `entries()`: the same, each new part an
/// empty text beside the text it was sent.
fn as_released(entries: &[released::SessionMessageEntry]) {
    assert_eq!(entries.len(), 2);
    assert_eq!(
        entries[0].parts,
        vec![
            released::MessagePart::Text { id: "t0".into(), text: "[Answer from your orchestrator] Yes".into() },
            empty_text("injected"),
        ]
    );
    assert_eq!(entries[1].parts[1], empty_text("crew"));
    let released::MessagePart::Text { text, .. } = &entries[1].parts[0] else {
        panic!("its text: {:?}", entries[1].parts[0]);
    };
    assert!(text.starts_with("Crew updates\n- "), "{text}");
}

#[test]
fn a_released_build_reads_a_reset_with_them() {
    let wire = serde_json::to_value(TranscriptFrame::reset(&entries())).unwrap();
    let frame: released::TranscriptFrame = serde_json::from_value(wire).expect("a released build reads it");
    let released::TranscriptFrame::Reset { reset } = frame else {
        panic!("a reset: {frame:?}");
    };
    as_released(&reset);
}

#[test]
fn a_released_build_reads_a_delta_and_a_part_patch_with_them() {
    let entries = entries();
    let delta = TranscriptFrame::Delta {
        upsert: entries
            .iter()
            .map(|entry| TranscriptUpsert { after: None, entry: entry.clone() })
            .collect(),
        append: Vec::new(),
        remove: Vec::new(),
        parts: vec![
            PartPatch { entry: "u-1".into(), at: 1, part: entries[0].parts[1].clone(), len: 2 },
            PartPatch { entry: "u-2".into(), at: 1, part: entries[1].parts[1].clone(), len: 2 },
        ],
        count: 2,
    };
    let wire = serde_json::to_string(&delta).unwrap();
    let frame: released::TranscriptFrame = serde_json::from_str(&wire).expect("a released build reads it");
    let released::TranscriptFrame::Delta { upsert, parts, count, .. } = frame else {
        panic!("a delta: {frame:?}");
    };
    assert_eq!(count, 2);
    as_released(&upsert.into_iter().map(|u| u.entry).collect::<Vec<_>>());
    assert_eq!(parts[0].part, empty_text("injected"));
    assert_eq!(parts[1].part, empty_text("crew"));
}

#[test]
fn they_go_as_text_parts_and_come_back_whole() {
    let entries = entries();
    let wire = serde_json::to_value(&entries).unwrap();
    assert_eq!(
        wire[0]["parts"][1],
        json!({
            "kind": "text",
            "id": "injected",
            "text": "",
            "injected": { "kind": "answer", "askId": "ak-1", "chatId": "thread-1", "turnId": "m-9", "by": "orchestrator" },
        })
    );
    let crew = &wire[1]["parts"][1];
    assert_eq!(crew["kind"], "text");
    assert_eq!(crew["text"], "");
    assert_eq!(crew["crewReports"].as_array().map(Vec::len), Some(2));
    assert_eq!(crew["crewReports"][0]["fromChat"], "thread-1");
    assert!(crew.get("reports").is_none());

    let back: Vec<SessionMessageEntry> = serde_json::from_value(wire).unwrap();
    assert_eq!(back, entries);
    // And through a frame, as a reader applies it.
    let frame: TranscriptFrame =
        serde_json::from_str(&serde_json::to_string(&TranscriptFrame::reset(&entries)).unwrap()).unwrap();
    let TranscriptFrame::Reset { reset } = frame else { panic!("a reset") };
    assert_eq!(reset, entries);
}

#[test]
fn the_shapes_written_before_still_read() {
    let legacy = json!([
        { "kind": "injected", "id": "injected", "injected": { "kind": "answer", "askId": "ak-1", "chatId": "thread-1", "turnId": "m-9", "by": "orchestrator" } },
        { "kind": "crewReports", "id": "crew", "reports": [serde_json::to_value(report("r-1")).unwrap()] },
    ]);
    let parts: Vec<MessagePart> = serde_json::from_value(legacy).unwrap();
    assert_eq!(
        parts,
        vec![
            MessagePart::Injected { id: "injected".into(), injected: said() },
            MessagePart::CrewReports { id: "crew".into(), reports: vec![report("r-1")] },
        ]
    );
    let frame: TranscriptFrame = serde_json::from_value(json!({
        "reset": [{ "id": "u-1", "role": "user", "createdAt": 1, "deviceId": "d", "parts": [
            { "kind": "text", "id": "t0", "text": "hi" },
            { "kind": "injected", "id": "injected", "injected": { "kind": "scheduled" } },
        ] }]
    }))
    .unwrap();
    let TranscriptFrame::Reset { reset } = frame else { panic!("a reset") };
    assert_eq!(reset[0].parts[1], MessagePart::Injected { id: "injected".into(), injected: Injected::scheduled() });
}

#[test]
fn what_this_build_cant_read_of_them_degrades() {
    let parts: Vec<MessagePart> = serde_json::from_value(json!([
        // An `injected` a newer peer wrote in a shape this build can't read:
        // the empty text it rides as.
        { "kind": "text", "id": "a", "text": "", "injected": 7 },
        // Null is none.
        { "kind": "text", "id": "b", "text": "words", "injected": null },
        // A report this build can't read costs that report, not the card.
        { "kind": "text", "id": "c", "text": "", "crewReports": [{ "id": "bad" }, serde_json::to_value(report("r-1")).unwrap()] },
        // A newer injected kind, and fields this build doesn't know.
        { "kind": "text", "id": "d", "text": "", "injected": { "kind": "someday", "whatever": 1 } },
        // Plain text is plain text.
        { "kind": "text", "id": "e", "text": "hello" },
    ]))
    .unwrap();
    assert_eq!(
        parts,
        vec![
            MessagePart::Text { id: "a".into(), text: String::new() },
            MessagePart::Text { id: "b".into(), text: "words".into() },
            MessagePart::CrewReports { id: "c".into(), reports: vec![report("r-1")] },
            MessagePart::Injected { id: "d".into(), injected: Injected::of("someday") },
            MessagePart::Text { id: "e".into(), text: "hello".into() },
        ]
    );
}
