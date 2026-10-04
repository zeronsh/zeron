//! Live check of mid-chat provider switching against the REAL Claude Code and
//! Codex CLIs signed in on this machine. Ignored by default (it spends a few
//! tiny turns of real usage and needs both CLIs logged in):
//!
//! ```text
//! cargo test -p zeron-engine --test provider_switch_live -- --ignored --nocapture
//! ```

use std::time::{Duration, Instant};

use zeron_doc::{MessagePart, MessageRole};
use zeron_engine::{EngineCore, default_registry};
use zeron_proto::{HarnessId, RunRequest, SandboxLevel, SessionStatus};

const CHAT: &str = "live-switch";
const CODEWORD: &str = "TANGERINE-4417";

fn request(harness: HarnessId, prompt: &str, cwd: &str) -> RunRequest {
    RunRequest {
        mcp: None,
        prompt: prompt.into(),
        harness: Some(harness),
        model: None,
        reasoning: None,
        model_options: Default::default(),
        cwd: cwd.into(),
        sandbox: SandboxLevel::ReadOnly,
        auto_approve: true,
        resume: None,
        attachments: vec![],
        worktree: None,
    }
}

/// One turn; returns (assistant reply, wall time).
async fn turn(
    core: &EngineCore,
    harness: HarnessId,
    prompt: &str,
    id: &str,
    cwd: &str,
) -> (String, Duration) {
    let started = Instant::now();
    core.sessions
        .dispatch(
            CHAT,
            harness,
            request(harness, prompt, cwd),
            Some(id.into()),
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(240), async {
        // Wait for the turn to start, then to finish.
        while core.sessions.session_status(CHAT).is_none() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        loop {
            tokio::time::sleep(Duration::from_millis(100)).await;
            if core
                .sessions
                .session_status(CHAT)
                .is_some_and(|s| s.status == SessionStatus::Idle)
                && !core.sessions.turn_in_flight(CHAT)
                && reply(core).is_some_and(|(after, _)| after == id)
            {
                break;
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{harness:?} turn timed out"));
    let elapsed = started.elapsed();
    (reply(core).unwrap().1, elapsed)
}

/// (id of the user message the newest reply follows, reply text).
fn reply(core: &EngineCore) -> Option<(String, String)> {
    let entries = core
        .doc_host
        .open(CHAT)
        .unwrap()
        .doc()
        .read_entries()
        .unwrap();
    let ix = entries
        .iter()
        .rposition(|e| e.role == MessageRole::Assistant)?;
    let user = entries[..ix]
        .iter()
        .rev()
        .find(|e| e.role == MessageRole::User)?
        .id
        .clone();
    let text = entries[ix]
        .parts
        .iter()
        .filter_map(|p| match p {
            MessagePart::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("");
    Some((user, text))
}

#[tokio::test]
#[ignore = "drives the real claude and codex CLIs"]
async fn a_conversation_moves_from_claude_to_codex_and_back_keeping_its_context() {
    let data = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let cwd = work.path().to_str().unwrap();
    let core = EngineCore::assemble(
        data.path(),
        std::sync::Arc::new(default_registry()),
        HarnessId::ClaudeCode,
        None,
    )
    .unwrap();
    core.workspace
        .create_chat(CHAT, None, Some(&core.device_id), None, Some(cwd.into()))
        .unwrap();

    let (r1, t1) = turn(
        &core,
        HarnessId::ClaudeCode,
        &format!("Remember this codeword: {CODEWORD}. Reply with just the word OK."),
        "m1",
        cwd,
    )
    .await;
    eprintln!("[claude ] turn 1 ({t1:.1?}): {r1:?}");
    let claude_session = core
        .workspace
        .chat(CHAT)
        .unwrap()
        .unwrap()
        .harness_session_id;
    assert!(claude_session.is_some(), "claude minted a session");

    // ── switch to Codex ──
    let (r2, t2) = turn(
        &core,
        HarnessId::Codex,
        "What codeword did I ask you to remember? Reply with only the codeword.",
        "m2",
        cwd,
    )
    .await;
    eprintln!("[codex  ] turn 2 ({t2:.1?}) after switch: {r2:?}");
    assert!(
        r2.contains(CODEWORD),
        "codex recalled the codeword; got {r2:?}"
    );
    let row = core.workspace.chat(CHAT).unwrap().unwrap();
    assert_eq!(row.harness_session_harness.as_deref(), Some("codex"));
    assert_ne!(
        row.harness_session_id, claude_session,
        "no session shared across providers"
    );

    // ── same provider again: resumes natively, no replay ──
    let (r3, t3) = turn(
        &core,
        HarnessId::Codex,
        "Spell that codeword backwards, letters and digits only, no punctuation. Reply with only that.",
        "m3",
        cwd,
    )
    .await;
    eprintln!("[codex  ] turn 3 ({t3:.1?}) native resume: {r3:?}");
    assert!(r3.to_uppercase().contains("7144"), "got {r3:?}");

    // ── and back to Claude ──
    let (r4, t4) = turn(
        &core,
        HarnessId::ClaudeCode,
        "What was the original codeword? Reply with only the codeword.",
        "m4",
        cwd,
    )
    .await;
    eprintln!("[claude ] turn 4 ({t4:.1?}) after switching back: {r4:?}");
    assert!(
        r4.contains(CODEWORD),
        "claude recalled after the round trip; got {r4:?}"
    );

    let seams: Vec<_> = core
        .doc_host
        .open(CHAT)
        .unwrap()
        .doc()
        .read_entries()
        .unwrap()
        .into_iter()
        .flat_map(|e| e.parts)
        .filter_map(|p| match p {
            MessagePart::Switch { from, to, .. } => Some(format!("{from} -> {to}")),
            _ => None,
        })
        .collect();
    eprintln!("[seams  ] {seams:?}");
    assert_eq!(seams.len(), 2);
    core.shutdown().await;
}
