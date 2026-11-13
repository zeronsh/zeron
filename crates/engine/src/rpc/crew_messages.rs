//! A crew thread's message to its own orchestrator is a report, never a user
//! turn: whatever path sends it (Zeron's `send_message`, a queued row, a raw
//! `QueueCommand`), it's filed as a `message` report (the first line its
//! summary, the whole text its detail). This keeps older prompts and agents
//! that `send_message` their orchestrator working without the spam.

use serde_json::{Value, json};
use zeron_proto::orchestration::ReportState;
use zeron_rpc::{Caller, RpcError, RpcReply, methods};

use super::EngineRpc;
use crate::orchestration::report::{ReportParams, file_report};

/// The text an agent's send carries to `chatId`, when `method` sends one.
fn sent_text(method: &str, params: &Value) -> Option<String> {
    let text = match method {
        methods::QUEUE_MESSAGE => params["text"].as_str(),
        methods::QUEUE_COMMAND => match params["command"]["kind"].as_str() {
            Some("run") => params["command"]["request"]["prompt"].as_str(),
            Some("steer") => params["command"]["prompt"].as_str(),
            _ => None,
        },
        _ => None,
    }?;
    // The sender's own routing header (the MCP writes one) isn't the message.
    let body = match zeron_proto::agent_messages::agent_message_sender(text) {
        Some((_, body)) => body,
        None => text,
    };
    Some(body.to_owned())
}

/// A report's summary and detail from a message's text.
pub(crate) fn as_report(text: &str) -> ReportParams {
    let text = text.trim();
    let summary = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("Message").to_owned();
    ReportParams {
        state: ReportState::Message,
        summary,
        detail: text.to_owned(),
        item_id: None,
        links: Vec::new(),
        progress: None,
    }
}

/// The crew rules each thread was last sent, by thread: a thread is sent
/// its rules again only when they changed.
fn rules_sent() -> &'static std::sync::Mutex<std::collections::HashMap<String, u64>> {
    static SENT: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, u64>>> =
        std::sync::OnceLock::new();
    SENT.get_or_init(Default::default)
}

fn rules_hash(rules: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    rules.trim().hash(&mut h);
    h.finish()
}

/// Where a send's text sits in its params.
fn text_slot<'a>(method: &str, params: &'a mut Value) -> Option<&'a mut Value> {
    match method {
        methods::QUEUE_MESSAGE => Some(&mut params["text"]),
        methods::QUEUE_COMMAND => match params["command"]["kind"].as_str() {
            Some("run") => Some(&mut params["command"]["request"]["prompt"]),
            Some("steer") => Some(&mut params["command"]["prompt"]),
            _ => None,
        },
        _ => None,
    }
}

impl EngineRpc {
    /// An agent's message to another chat, for the comms feed.
    pub(super) fn note_agent_message(&self, method: &str, params: &Value) {
        let Caller::Agent { chat: Some(caller) } = zeron_rpc::caller() else {
            return;
        };
        let (Some(target), Some(text)) = (params["chatId"].as_str(), sent_text(method, params)) else {
            return;
        };
        if target == caller {
            return;
        }
        let summary = zeron_proto::orchestration::prompt_text::one_line(&text, 140);
        self.orchestration()
            .comms()
            .say(zeron_proto::orchestration::CommKind::Message, &caller, target, &summary);
    }

    /// An orchestrator's send to a thread in its own crew starts with its
    /// crew rules (`[Crew rules from <name>]`) the first time, and again
    /// whenever they change.
    pub(super) fn with_crew_rules(&self, method: &str, params: &mut Value) {
        let Caller::Agent { chat: Some(caller) } = zeron_rpc::caller() else {
            return;
        };
        let Some(target) = params["chatId"].as_str().map(str::to_owned) else {
            return;
        };
        let orchestration = self.orchestration();
        let Some(orchestrator) = orchestration.orchestrator(&caller) else {
            return;
        };
        if orchestrator.crew_rules.trim().is_empty()
            || orchestration.crew_of(&target).is_none_or(|m| m.orchestrator_id != caller)
        {
            return;
        }
        let Some(slot) = text_slot(method, params) else { return };
        let Some(text) = slot.as_str() else { return };
        let hash = rules_hash(&orchestrator.crew_rules);
        {
            let mut sent = rules_sent().lock().unwrap_or_else(|e| e.into_inner());
            if sent.get(&target) == Some(&hash) {
                return;
            }
            sent.insert(target.clone(), hash);
        }
        // Under the sender's own header (the gate writes it afresh).
        let body = match zeron_proto::agent_messages::agent_message_sender(text) {
            Some((sender, body)) if sender.chat_id == caller => body,
            _ => text,
        };
        let rules = zeron_proto::orchestration::role_prompts::crew_rules_block(&orchestrator.name, &orchestrator.crew_rules);
        *slot = json!(format!("{rules}{body}"));
    }

    /// `Some(reply)` when this call is a crew thread messaging its own
    /// orchestrator: filed as a report instead, replying as the send would.
    pub(super) async fn crew_message_as_report(
        &self,
        method: &str,
        params: &Value,
    ) -> Result<Option<RpcReply>, RpcError> {
        if !matches!(method, methods::QUEUE_MESSAGE | methods::QUEUE_COMMAND) {
            return Ok(None);
        }
        let Caller::Agent { chat: Some(caller) } = zeron_rpc::caller() else {
            return Ok(None);
        };
        let Some(target) = params["chatId"].as_str() else {
            return Ok(None);
        };
        let Some(text) = sent_text(method, params) else {
            return Ok(None);
        };
        // The orchestrator is the crew row's: a send anywhere else is a send.
        let Some(member) = self.orchestration().crew_of(&caller) else {
            return Ok(None);
        };
        if member.orchestrator_id != target || text.trim().is_empty() {
            return Ok(None);
        }
        let filed = file_report(self, &caller, as_report(&text)).await?;
        let id = filed["id"].as_str().unwrap_or_default().to_owned();
        let reply = match method {
            methods::QUEUE_MESSAGE => json!({ "id": id, "report": id }),
            _ => json!({ "commandId": id, "report": id }),
        };
        Ok(Some(RpcReply::value(&reply)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_reads_as_a_report() {
        let r = as_report("\n  Tests pass on main\nDetails: 412 passed\n");
        assert_eq!(r.state, ReportState::Message);
        assert_eq!(r.summary, "Tests pass on main");
        assert_eq!(r.detail, "Tests pass on main\nDetails: 412 passed");
    }

    #[test]
    fn the_sent_text_is_the_body_under_the_header() {
        let text = zeron_proto::agent_messages::agent_message("Crew (abcd1234)", "abcd1234-full", "done: merged");
        assert_eq!(
            sent_text(methods::QUEUE_MESSAGE, &json!({ "chatId": "o", "text": text })).as_deref(),
            Some("done: merged")
        );
        let run = json!({ "chatId": "o", "command": { "kind": "run", "request": { "prompt": "hi" } } });
        assert_eq!(sent_text(methods::QUEUE_COMMAND, &run).as_deref(), Some("hi"));
        let steer = json!({ "chatId": "o", "command": { "kind": "steer", "prompt": "yo" } });
        assert_eq!(sent_text(methods::QUEUE_COMMAND, &steer).as_deref(), Some("yo"));
        let stop = json!({ "chatId": "o", "command": { "kind": "interrupt" } });
        assert_eq!(sent_text(methods::QUEUE_COMMAND, &stop), None);
    }
}
