//! Importing a Claude Code transcript: row, title, resume handshake, doc contents, idempotence.

use std::path::Path;
use std::sync::Arc;

use zeron_doc::{MessagePart, MessageRole};
use zeron_engine::claude_import::{ClaudeImportEvent, ClaudeImporter};
use zeron_engine::{EngineCore, EngineProfile, HarnessId, default_registry};

const SESSION: &str = "11111111-2222-3333-4444-555555555555";

fn assemble(data_dir: &Path) -> EngineCore {
    let profile = EngineProfile::local(data_dir).expect("local profile");
    EngineCore::assemble_with_profile(profile, Arc::new(default_registry()), HarnessId::Mock, None)
        .expect("assemble profile")
}

/// A transcript shaped like the CLI's: metadata preamble, a typed prompt, a
/// reply with a tool round-trip, an injected turn that must not import, a title.
fn write_transcript(config_dir: &Path, cwd: &str) {
    let projects = config_dir.join("projects").join("-w-proj");
    std::fs::create_dir_all(&projects).expect("projects dir");
    let lines = [
        r#"{"type":"mode","mode":"default"}"#.to_string(),
        format!(
            r#"{{"type":"user","promptSource":"typed","cwd":"{cwd}","gitBranch":"main","timestamp":"2026-09-16T20:27:13.490Z","message":{{"role":"user","content":"make the tests pass"}}}}"#
        ),
        r#"{"type":"assistant","timestamp":"2026-09-16T20:27:20.000Z","message":{"role":"assistant","content":[{"type":"thinking","thinking":"weighing it","signature":"s"},{"type":"text","text":"Running them now."},{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"cargo test"}}]}}"#.to_string(),
        r#"{"type":"user","timestamp":"2026-09-16T20:27:25.000Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","is_error":false,"content":"ok"}]}}"#.to_string(),
        r#"{"type":"user","promptSource":"system","timestamp":"2026-09-16T20:27:26.000Z","message":{"role":"user","content":"<task-notification>ignore me</task-notification>"}}"#.to_string(),
        r#"{"type":"assistant","timestamp":"2026-09-16T20:27:30.000Z","message":{"role":"assistant","content":[{"type":"text","text":"All green."}]}}"#.to_string(),
        r#"{"type":"attachment","attachment":{"type":"total_tokens_reminder"}}"#.to_string(),
        format!(r#"{{"type":"ai-title","aiTitle":"Making the tests pass","sessionId":"{SESSION}"}}"#),
    ];
    std::fs::write(
        projects.join(format!("{SESSION}.jsonl")),
        format!("{}\n", lines.join("\n")),
    )
    .expect("write transcript");
}

fn importer(core: &EngineCore, config_dir: &Path) -> ClaudeImporter {
    ClaudeImporter::new(
        config_dir.to_path_buf(),
        &core.device_id,
        core.workspace.clone(),
        core.doc_host.clone(),
    )
}

fn run(importer: &ClaudeImporter, ids: &[String]) -> Vec<ClaudeImportEvent> {
    let mut events = Vec::new();
    importer
        .run(ids, |event| events.push(event))
        .expect("import");
    events
}

fn summary(events: &[ClaudeImportEvent]) -> (usize, usize, usize) {
    match events.last().expect("summary event") {
        ClaudeImportEvent::Summary {
            imported,
            skipped,
            messages,
            errors,
        } => {
            assert!(errors.is_empty(), "import errors: {errors:?}");
            (*imported, *skipped, *messages)
        }
        other => panic!("last event was not a summary: {other:?}"),
    }
}

#[tokio::test]
async fn imports_a_transcript_into_a_resumable_chat() {
    let data = tempfile::tempdir().expect("data dir");
    let claude = tempfile::tempdir().expect("claude dir");
    let work = tempfile::tempdir().expect("work dir");
    let cwd = work.path().to_string_lossy().to_string();
    write_transcript(claude.path(), &cwd);

    let core = assemble(data.path());
    let importer = importer(&core, claude.path());

    let listed = importer.list();
    assert_eq!(listed.len(), 1, "one transcript is on offer");
    assert_eq!(listed[0].session.session_id, SESSION);
    assert_eq!(listed[0].session.title, "Making the tests pass");
    assert!(!listed[0].already_imported);

    let events = run(&importer, &[]);
    assert_eq!(summary(&events), (1, 0, 2), "one chat, two messages");
    assert!(
        matches!(&events[0], ClaudeImportEvent::Start { sessions } if *sessions == 1),
        "the run opens with a start event"
    );

    let chat = core
        .workspace
        .chat(SESSION)
        .expect("read chat")
        .expect("the chat row exists under the claude session id");
    assert_eq!(chat.title.as_deref(), Some("Making the tests pass"));
    assert_eq!(chat.cwd.as_deref(), Some(cwd.as_str()));
    assert_eq!(
        chat.harness_session_id.as_deref(),
        Some(SESSION),
        "the row carries the resume handshake"
    );
    assert_eq!(chat.harness_session_cwd.as_deref(), Some(cwd.as_str()));
    assert_eq!(chat.config.map(|c| c.harness), Some(HarnessId::ClaudeCode));
    assert!(chat.space_id.is_some(), "the chat lands in a project");

    // The transcript's own clock, not the moment of import: the sidebar has to
    // sort an imported chat by when the work actually happened.
    assert_eq!(
        chat.created_at.timestamp_millis(),
        1_789_590_433_490,
        "created_at is the first record's timestamp"
    );
    assert_eq!(
        chat.last_message_at.map(|at| at.timestamp_millis()),
        Some(1_789_590_450_000),
        "last_message_at is the last record's timestamp, not now"
    );
    assert!(
        chat.last_message_preview
            .as_deref()
            .is_some_and(|p| p.contains("All green")),
        "the preview still comes from the last message"
    );

    let entries = core
        .doc_host
        .open(SESSION)
        .expect("open doc")
        .doc()
        .read_entries()
        .expect("read entries");
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].role, MessageRole::User);
    assert!(
        matches!(&entries[0].parts[0], MessagePart::Text { text, .. } if text == "make the tests pass")
    );
    assert_eq!(entries[1].role, MessageRole::Assistant);
    let kinds: Vec<&str> = entries[1]
        .parts
        .iter()
        .map(|p| match p {
            MessagePart::Text { .. } => "text",
            MessagePart::Reasoning { .. } => "reasoning",
            MessagePart::Tool { .. } => "tool",
            _ => "other",
        })
        .collect();
    assert_eq!(
        kinds,
        ["reasoning", "text", "tool", "text"],
        "the injected turn never became a message"
    );
    assert!(
        matches!(&entries[1].parts[2], MessagePart::Tool { resolved, is_error, .. } if *resolved && !*is_error),
        "the tool call is paired with its result"
    );

    core.shutdown().await;
}

#[tokio::test]
async fn re_importing_the_same_transcript_changes_nothing() {
    let data = tempfile::tempdir().expect("data dir");
    let claude = tempfile::tempdir().expect("claude dir");
    let work = tempfile::tempdir().expect("work dir");
    write_transcript(claude.path(), &work.path().to_string_lossy());

    let core = assemble(data.path());
    let importer = importer(&core, claude.path());
    assert_eq!(summary(&run(&importer, &[])).0, 1);

    assert!(
        importer.list()[0].already_imported,
        "a listed transcript reports that it is already here"
    );
    let again = run(&importer, &[]);
    assert_eq!(
        summary(&again),
        (0, 0, 0),
        "an un-named re-run offers nothing new"
    );
    let explicit = run(&importer, &[SESSION.to_string()]);
    assert_eq!(
        summary(&explicit),
        (0, 1, 0),
        "naming it explicitly skips rather than duplicates"
    );

    let entries = core
        .doc_host
        .open(SESSION)
        .expect("open doc")
        .doc()
        .read_entries()
        .expect("read entries");
    assert_eq!(entries.len(), 2, "the doc was not written twice");

    core.shutdown().await;
}

/// Deleting a chat tombstones its row but leaves the doc, so re-importing has
/// to recognize the entries it already wrote rather than doubling them.
#[tokio::test]
async fn re_importing_after_deleting_the_chat_does_not_double_the_transcript() {
    let data = tempfile::tempdir().expect("data dir");
    let claude = tempfile::tempdir().expect("claude dir");
    let work = tempfile::tempdir().expect("work dir");
    write_transcript(claude.path(), &work.path().to_string_lossy());

    let core = assemble(data.path());
    let importer = importer(&core, claude.path());
    assert_eq!(summary(&run(&importer, &[])).0, 1);

    assert!(
        core.workspace.delete_chat(SESSION).expect("delete chat"),
        "the row is removed"
    );
    assert_eq!(
        summary(&run(&importer, &[])),
        (1, 0, 0),
        "the chat comes back, but writes no further messages"
    );

    let entries = core
        .doc_host
        .open(SESSION)
        .expect("open doc")
        .doc()
        .read_entries()
        .expect("read entries");
    assert_eq!(entries.len(), 2, "still one copy of the transcript");

    core.shutdown().await;
}
