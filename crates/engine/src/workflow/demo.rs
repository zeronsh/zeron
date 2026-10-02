//! `ZERON_MOCK_WORKFLOW=1`: scripted workflow agents for demos, screenshots
//! and UI work — no model anywhere.
//!
//! Every ask is answered by a script keyed on the agent's *name*, so a
//! workflow script decides what a run looks like:
//!
//! | name contains | the agent … |
//! | --- | --- |
//! | `flaky` | hits an authentication error: the run stops `stopped(provider)` |
//! | `fail` | ends its turn without a result: that ask fails, the script goes on |
//! | `ask` (starts with) | escalates a question and waits for the answer |
//! | `slow` | takes six times as long |
//! | `quick` | takes a third of the time |
//!
//! Pacing is deterministic: a base unit (`ZERON_MOCK_WORKFLOW_PACE_MS`,
//! default 1000) stretched by a few percent from the name, and progress ticks
//! (turn / tool calls / last tool) spread over the wait so a card shows agents
//! mid-work. Each actor gets a *real* hidden child chat with a short
//! transcript, so "open this agent's chat" works end to end.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use zeron_doc::{MessagePart, MessageRole, MessageStatus, SessionMessageEntry};
use zeron_proto::ChatConfig;

use crate::ask::{
    AskBackend, AskError, AskFailure, AskOutcome, AskSpec, AskUsage, FakeAsk, FakeReply,
    sample_value,
};
use crate::workspace_host::WorkspaceHost;
use crate::{DocHost, new_id};

/// Base time of one ask, from `ZERON_MOCK_WORKFLOW_PACE_MS`.
pub fn pace() -> Duration {
    std::env::var("ZERON_MOCK_WORKFLOW_PACE_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or(Duration::from_millis(1000))
}

/// The scripted reply for an ask labelled `label` (pure; tested).
pub fn script_for(label: &str, schema: &Value, pace: Duration) -> FakeReply {
    let lower = label.to_lowercase();
    let spread = label
        .bytes()
        .fold(0u64, |a, b| a.wrapping_add(u64::from(b)))
        % 5;
    let factor = if lower.contains("slow") {
        6.0
    } else if lower.contains("quick") {
        0.34
    } else {
        1.0 + spread as f64 * 0.35
    };
    let total = pace.mul_f64(factor);
    if lower.contains("flaky") {
        return FakeReply::After(
            pace * 2,
            Box::new(FakeReply::Fail(AskError::TurnFailed(
                "401 Unauthorized: invalid API key".into(),
            ))),
        );
    }
    if lower.contains("fail") {
        return FakeReply::After(total, Box::new(FakeReply::Fail(AskError::NoResult)));
    }
    let is_text = schema["properties"].get("text").is_some()
        && schema["properties"]
            .as_object()
            .is_some_and(|p| p.len() == 1);
    let result = if is_text {
        json!({"text": format!("{label} looked at the change and found nothing blocking.")})
    } else {
        sample_value(schema, label)
    };
    let done = FakeReply::ResultWithUsage(
        result,
        AskUsage {
            input_tokens: 3_000 + spread * 700,
            output_tokens: 400 + spread * 90,
            elapsed_ms: total.as_millis() as u64,
            turns: 2,
        },
    );
    let third = total / 3;
    let work = FakeReply::Progress(
        vec![
            (third, 1, 3, Some("Read".into())),
            (third, 1, 7, Some("Grep".into())),
            (total - third * 2, 2, 11, Some("Edit".into())),
        ],
        Box::new(done),
    );
    if lower.starts_with("ask") {
        return FakeReply::Escalate {
            question: "The change adds a migration. Should I keep the old column for one \
                       release (safe, slower) or drop it now (clean, irreversible)?"
                .into(),
            context: "Both options pass the tests; the old column is read by nothing in this repo."
                .into(),
            then: Box::new(work),
        };
    }
    work
}

/// [`FakeAsk`] with real child chats: scripted answers, genuine transcripts.
pub struct DemoWorkflowAsk {
    fake: Arc<FakeAsk>,
    workspace: WorkspaceHost,
    doc_host: DocHost,
}

impl DemoWorkflowAsk {
    pub fn new(workspace: WorkspaceHost, doc_host: DocHost) -> Arc<Self> {
        let fake = FakeAsk::new();
        let pace = pace();
        fake.on_call(move |call| {
            Some(script_for(&call.spec.label, &call.spec.result_schema, pace))
        });
        Arc::new(Self {
            fake,
            workspace,
            doc_host,
        })
    }

    /// The hidden child chat of a new actor (archived at once, like the real
    /// ask layer's), stamped so the UI can tell it from a person's chat.
    fn child_for(&self, parent: &str, spec: &AskSpec) -> Option<String> {
        let parent_chat = self.workspace.chat(parent).ok().flatten()?;
        let id = new_id();
        let config = parent_chat.config.clone().or_else(|| {
            Some(ChatConfig {
                harness: zeron_proto::HarnessId::Mock,
                model: None,
                reasoning: None,
                model_options: Default::default(),
                sandbox: zeron_proto::SandboxLevel::WorkspaceWrite,
            })
        });
        self.workspace
            .create_chat_with_parent(
                &id,
                parent_chat.space_id.as_deref(),
                Some(&parent_chat.device_id),
                config,
                parent_chat.cwd.clone(),
                Some(parent.to_owned()),
            )
            .ok()?;
        let _ = self.workspace.rename_chat(
            &id,
            &spec.title.clone().unwrap_or_else(|| spec.label.clone()),
        );
        let _ = self.workspace.set_chat_archived(&id, true);
        if let (Ok(handle), Some(tag)) = (self.doc_host.open(&id), &spec.workflow_actor) {
            let _ = handle.doc().set_workflow_actor(tag);
        }
        Some(id)
    }

    fn write(&self, child: &str, role: MessageRole, text: String) {
        let Ok(handle) = self.doc_host.open(child) else {
            return;
        };
        let id = new_id();
        let _ = handle.doc().push_message(&SessionMessageEntry {
            id,
            role,
            parts: vec![MessagePart::Text {
                id: "t0".into(),
                text,
            }],
            created_at: chrono::Utc::now().timestamp_millis(),
            device_id: String::new(),
            status: Some(MessageStatus::Complete),
            continuation_of: None,
            duration_ms: None,
            origin: None,
        });
    }
}

#[async_trait]
impl AskBackend for DemoWorkflowAsk {
    async fn ask(
        &self,
        parent_chat_id: &str,
        mut spec: AskSpec,
        cancel: CancellationToken,
    ) -> Result<AskOutcome, AskFailure> {
        if spec.persistent
            && spec.reuse_child.is_none()
            && let Some(child) = self.child_for(parent_chat_id, &spec)
        {
            spec.reuse_child = Some(child);
        }
        let child = spec.reuse_child.clone();
        if let Some(child) = &child {
            self.write(child, MessageRole::User, spec.prompt.clone());
        }
        let outcome = self.fake.ask(parent_chat_id, spec, cancel).await;
        if let (Some(child), Ok(out)) = (&child, &outcome) {
            let body = serde_json::to_string_pretty(&out.result).unwrap_or_default();
            self.write(
                child,
                MessageRole::Assistant,
                format!("I looked into it and submitted my result.\n\n```json\n{body}\n```"),
            );
        }
        outcome
    }

    async fn answer_escalation(&self, child_chat_id: &str, qid: &str, answer: String) -> bool {
        self.fake
            .answer_escalation(child_chat_id, qid, answer)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_schema() -> Value {
        json!({"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"]})
    }

    fn unit() -> Duration {
        Duration::from_millis(100)
    }

    #[test]
    fn names_choose_the_behaviour() {
        let kind = |label: &str| match script_for(label, &text_schema(), unit()) {
            FakeReply::After(_, inner) => match *inner {
                FakeReply::Fail(AskError::TurnFailed(_)) => "provider",
                FakeReply::Fail(AskError::NoResult) => "no-result",
                _ => "other",
            },
            FakeReply::Escalate { .. } => "escalate",
            FakeReply::Progress(..) => "works",
            _ => "other",
        };
        assert_eq!(kind("flaky verifier"), "provider");
        assert_eq!(kind("fail checker"), "no-result");
        assert_eq!(kind("ask architect"), "escalate");
        assert_eq!(kind("security reviewer"), "works");
        // "ask" only as a prefix: a name that merely contains it is an ordinary agent
        assert_eq!(kind("task planner"), "works");
    }

    #[test]
    fn pacing_is_deterministic_and_ordered_by_name() {
        let total = |label: &str| match script_for(label, &text_schema(), unit()) {
            FakeReply::Progress(ticks, _) => ticks.iter().map(|t| t.0).sum::<Duration>(),
            _ => panic!("works expected"),
        };
        assert_eq!(total("reviewer"), total("reviewer"));
        assert!(total("slow reviewer") > total("reviewer"));
        assert!(total("quick reviewer") < total("reviewer"));
        // ticks add up to the whole wait and walk turns / calls forward
        match script_for("reviewer", &text_schema(), unit()) {
            FakeReply::Progress(ticks, inner) => {
                assert_eq!(ticks.len(), 3);
                assert!(ticks.windows(2).all(|w| w[0].2 < w[1].2));
                assert!(matches!(*inner, FakeReply::ResultWithUsage(..)));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn answers_satisfy_the_schema() {
        let schema = json!({
            "type": "object",
            "properties": {"verdict": {"type": "string", "enum": ["pass", "fail"]}},
            "required": ["verdict"]
        });
        match script_for("judge", &schema, unit()) {
            FakeReply::Progress(_, inner) => match *inner {
                FakeReply::ResultWithUsage(v, usage) => {
                    assert!(crate::ask::validate_result(&schema, &v).is_ok());
                    assert!(usage.total_tokens() > 0);
                }
                _ => panic!(),
            },
            _ => panic!(),
        }
    }
}
