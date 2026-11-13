//! Two things every engine watches for.
//!
//! - **Ask banners**: an open ask, wherever it was raised, is a desktop
//!   banner on this device once (the ids already shown are kept on disk, so
//!   a restart doesn't show them again; asks older than [`ASK_FRESH`] never
//!   show).
//! - **Route answers**: a Brain dump route with intent `question` that was
//!   delivered to a chat hosted here takes that chat's reply as its answer
//!   once the turn after the delivery ends: `answered`, the whole reply as
//!   `answerFull`, and `answer` one line the receipt shows (the reply's
//!   first real sentence at once, then the cheap one-shot model's one-line
//!   answer to the question when it can make one).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use chrono::{Duration as TimeDelta, Utc};
use zeron_doc::{MessagePart, MessageRole, MessageStatus, SessionMessageEntry};
use zeron_proto::orchestration::{AskState, Dump, OrchestrationState, RouteIntent, RouteState, RouteTarget};
use zeron_proto::{Session, SessionStatus};

use super::Orchestration;

/// An ask raised longer ago than this shows no banner.
pub(crate) const ASK_FRESH: TimeDelta = TimeDelta::minutes(10);
/// The most ask ids kept as shown.
const SHOWN_KEPT: usize = 1000;
/// The most a whole reply (`answerFull`) keeps.
const FULL_MAX: usize = 8000;
/// The most of a reply the condensing run reads.
const CONDENSE_INPUT_MAX: usize = 4000;

/// How long after a start dumps left routing are swept (queued sends
/// drain first).
const SWEEP_AFTER: std::time::Duration = std::time::Duration::from_secs(20);

pub(crate) fn start(orchestration: Orchestration) {
    let asks = orchestration.clone();
    tokio::spawn(async move { ask_banners(asks).await });
    let sweep = orchestration.clone();
    tokio::spawn(async move {
        tokio::time::sleep(SWEEP_AFTER).await;
        if let Some(rpc) = sweep.rpc() {
            let settled = super::stale_dumps::sweep(&sweep, &rpc);
            if !settled.is_empty() {
                tracing::info!(dumps = settled.len(), "brain dump: dumps left routing now need the person");
            }
        }
    });
    tokio::spawn(async move { route_answers(orchestration).await });
}

// ── Ask banners ─────────────────────────────────────────────────────────────

/// The ask ids this device has shown, kept in its data dir.
pub(crate) struct Shown {
    path: Option<PathBuf>,
    ids: Vec<String>,
    set: HashSet<String>,
}

impl Shown {
    pub(crate) fn load(dir: Option<&Path>) -> Self {
        let path = dir.map(|d| d.join("orchestration").join("asks-shown.json"));
        let ids: Vec<String> = path
            .as_ref()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        let set = ids.iter().cloned().collect();
        Self { path, ids, set }
    }

    /// Mark `id` shown; `false` when it already was.
    pub(crate) fn mark(&mut self, id: &str) -> bool {
        if !self.set.insert(id.to_owned()) {
            return false;
        }
        self.ids.push(id.to_owned());
        if self.ids.len() > SHOWN_KEPT {
            let gone: Vec<_> = self.ids.drain(..self.ids.len() - SHOWN_KEPT).collect();
            for id in gone {
                self.set.remove(&id);
            }
        }
        if let Some(path) = &self.path {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Ok(bytes) = serde_json::to_vec(&self.ids)
                && let Err(error) = std::fs::write(path, bytes)
            {
                tracing::warn!(%error, "asks: shown list not saved");
            }
        }
        true
    }
}

/// The open, fresh asks in `state` not shown yet, marked shown now.
pub(crate) fn asks_to_show<'a>(
    state: &'a OrchestrationState,
    shown: &mut Shown,
    now: chrono::DateTime<Utc>,
) -> Vec<&'a zeron_proto::orchestration::Ask> {
    state
        .asks
        .iter()
        .filter(|a| a.state == AskState::Open && now - a.created_at <= ASK_FRESH)
        .filter(|a| shown.mark(&a.id))
        .collect()
}

async fn ask_banners(orchestration: Orchestration) {
    let mut rx = orchestration.subscribe();
    let mut shown: Option<Shown> = None;
    loop {
        if let Some(rpc) = orchestration.rpc() {
            let shown = shown.get_or_insert_with(|| Shown::load(Some(rpc.data_dir())));
            let state = rx.borrow_and_update().clone();
            for ask in asks_to_show(&state, shown, Utc::now()) {
                let title = orchestration
                    .workspace()
                    .chat(&ask.from_chat)
                    .ok()
                    .flatten()
                    .and_then(|c| c.title)
                    .unwrap_or_else(|| "A chat".into());
                rpc.notify_person(&title, &super::excerpt(&ask.question, 200), Some(&ask.from_chat));
            }
        }
        if rx.changed().await.is_err() {
            break;
        }
    }
}

// ── Route answers ───────────────────────────────────────────────────────────

fn target_chat(target: &RouteTarget) -> Option<&str> {
    match target {
        RouteTarget::Orchestrator { id }
        | RouteTarget::Chat { id }
        | RouteTarget::NewOrchestrator { id }
        | RouteTarget::NewThread { id } => Some(id),
        _ => None,
    }
}

fn texts(entry: &SessionMessageEntry) -> String {
    entry
        .parts
        .iter()
        .filter_map(|p| match p {
            MessagePart::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

/// The reply to the message carrying `message` in `entries`: the last
/// assistant text after it, before the next message to the chat, once that
/// turn is done. `None` while it isn't.
pub(crate) fn reply_to(entries: &[SessionMessageEntry], message: &str) -> Option<String> {
    let message = message.trim();
    if message.is_empty() {
        return None;
    }
    let at = entries
        .iter()
        .rposition(|e| e.role == MessageRole::User && texts(e).contains(message))?;
    let turn: Vec<_> = entries[at + 1..]
        .iter()
        .take_while(|e| e.role != MessageRole::User)
        .filter(|e| e.role == MessageRole::Assistant)
        .collect();
    if turn.is_empty() || turn.iter().any(|e| e.status == Some(MessageStatus::Streaming)) {
        return None;
    }
    let answer = turn.iter().rev().map(|e| texts(e)).find(|t| !t.trim().is_empty())?;
    Some(answer.trim().chars().take(FULL_MAX).collect())
}

/// `(dump id, route id, message, question)` of every question route
/// waiting on an answer from `chat`.
fn waiting_on<'a>(dumps: &'a [Dump], chat: &str) -> Vec<(&'a str, &'a str, String, &'a str)> {
    dumps
        .iter()
        .flat_map(|d| d.routes.iter().map(move |r| (d, r)))
        .filter(|(_, r)| r.intent == RouteIntent::Question && r.state == RouteState::Delivered)
        .filter(|(_, r)| target_chat(&r.target) == Some(chat))
        .map(|(d, r)| {
            let message = if r.message.trim().is_empty() { r.excerpt.clone() } else { r.message.clone() };
            (d.id.as_str(), r.id.as_str(), message, r.excerpt.as_str())
        })
        .collect()
}

/// A local chat's turn ended since `before` (it settled with a new
/// completed turn).
fn ended(before: Option<&Session>, now: &Session) -> bool {
    !matches!(now.status, SessionStatus::Working | SessionStatus::AwaitingInput)
        && now.last_completed_turn.is_some()
        && before.is_none_or(|b| {
            b.last_completed_turn != now.last_completed_turn
                || matches!(b.status, SessionStatus::Working | SessionStatus::AwaitingInput)
        })
}

/// `full`, `chat`'s reply to a question route, recorded as its answer: at
/// once the reply's first real sentence ([`plain_answer`]), then the cheap
/// one-shot model's one-line answer to `question` from it (the title path
/// on `chat`'s harness's fastest model; quoted data), when it makes one.
/// `false` when the route had no answer waiting.
pub(crate) async fn record_answer(
    orchestration: &Orchestration,
    rpc: &crate::rpc::EngineRpc,
    chat: &str,
    dump_id: &str,
    route_id: &str,
    question: &str,
    full: String,
) -> bool {
    use zeron_proto::orchestration::{ANSWER_MAX, model_answer, plain_answer, prompt_text::one_line};
    let quick = Some(plain_answer(&full)).filter(|a| !a.is_empty()).unwrap_or_else(|| one_line(&full, ANSWER_MAX));
    let recorded = orchestration.workspace().write_orchestration(|doc| {
        let Some(mut dump) = doc.dump(dump_id) else { return false };
        let Some(route) = dump.routes.iter_mut().find(|r| r.id == route_id) else { return false };
        if route.state != RouteState::Delivered {
            return false;
        }
        route.state = RouteState::Answered;
        route.answer = Some(quick.clone());
        route.answer_full = Some(full.clone());
        dump.updated_at = Utc::now();
        doc.upsert_dump(&dump);
        true
    });
    if !recorded {
        return false;
    }
    orchestration.comms().say(zeron_proto::orchestration::CommKind::Answer, chat, super::comms::YOU, &quick);
    let reply: String = zeron_proto::redact::redact_command_output(&full).chars().take(CONDENSE_INPUT_MAX).collect();
    let data = serde_json::json!({ "question": one_line(question, 600), "reply": reply });
    let prompt = format!(
        "{}\n\nQuestion and reply (JSON):\n{}",
        zeron_harness::ANSWER_INSTRUCTIONS,
        serde_json::to_string_pretty(&data).unwrap_or_default()
    );
    let harness = rpc
        .workspace_host()
        .chat(chat)
        .ok()
        .flatten()
        .and_then(|c| c.config.map(|c| c.harness))
        .unwrap_or_else(|| rpc.doc_host_ref().default_harness());
    let condensed = orchestration.progress().one_shot(rpc)(harness, prompt)
        .await
        .map(|raw| model_answer(&raw))
        .filter(|a| !a.is_empty());
    if let Some(condensed) = condensed {
        orchestration.workspace().write_orchestration(|doc| {
            let Some(mut dump) = doc.dump(dump_id) else { return };
            let Some(route) = dump.routes.iter_mut().find(|r| r.id == route_id) else { return };
            // Only over the answer it condenses (not one a reroute replaced).
            if route.state != RouteState::Answered || route.answer_full.as_deref() != Some(full.as_str()) {
                return;
            }
            route.answer = Some(condensed);
            dump.updated_at = Utc::now();
            doc.upsert_dump(&dump);
        });
    }
    true
}

async fn route_answers(orchestration: Orchestration) {
    let mut sessions_rx = loop {
        match orchestration.rpc() {
            Some(rpc) => break rpc.sessions_ref().watch_sessions(),
            None => tokio::time::sleep(std::time::Duration::from_secs(1)).await,
        }
    };
    let mut seen: HashMap<String, Session> =
        sessions_rx.borrow_and_update().iter().map(|s| (s.chat_id.clone(), s.clone())).collect();
    while sessions_rx.changed().await.is_ok() {
        let now: Vec<Session> = sessions_rx.borrow_and_update().clone();
        let finished: Vec<String> = now
            .iter()
            .filter(|s| ended(seen.get(&s.chat_id), s))
            .map(|s| s.chat_id.clone())
            .collect();
        seen = now.into_iter().map(|s| (s.chat_id.clone(), s)).collect();
        if finished.is_empty() {
            continue;
        }
        // (No services right now: nothing to read the transcripts with.)
        let Some(rpc) = orchestration.rpc() else { continue };
        let dumps = orchestration.state().dumps;
        for chat in finished {
            // A Brain dump's turn that left a dump routing.
            super::stale_dumps::after_turn(&orchestration, &rpc, &chat);
            let waiting = waiting_on(&dumps, &chat);
            if waiting.is_empty() {
                continue;
            }
            let Ok(handle) = rpc.doc_host_ref().open(&chat) else { continue };
            let Ok(entries) = handle.doc().read_entries() else { continue };
            for (dump_id, route_id, message, question) in waiting {
                let Some(full) = reply_to(&entries, &message) else { continue };
                let (orchestration, rpc) = (orchestration.clone(), rpc.clone());
                let (chat, dump_id, route_id, question) =
                    (chat.clone(), dump_id.to_owned(), route_id.to_owned(), question.to_owned());
                tokio::spawn(async move {
                    record_answer(&orchestration, &rpc, &chat, &dump_id, &route_id, &question, full).await;
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeron_proto::orchestration::{Ask, AskKind};

    fn entry(id: &str, role: MessageRole, text: &str, status: MessageStatus) -> SessionMessageEntry {
        SessionMessageEntry {
            id: id.into(),
            role,
            parts: vec![MessagePart::Text { id: "t".into(), text: text.into() }],
            created_at: 0,
            device_id: "d".into(),
            status: Some(status),
            continuation_of: None,
            duration_ms: None,
        }
    }

    #[test]
    fn the_answer_is_the_last_reply_text_of_the_turn_after_delivery() {
        use MessageRole::*;
        use MessageStatus::*;
        let mut entries = vec![
            entry("u0", User, "earlier", Complete),
            entry("a0", Assistant, "old reply", Complete),
            entry("u1", User, "[Message from Zeron chat Brain dump (b).]\n\nWhen does 2.0 ship?", Complete),
            entry("a1", Assistant, "Looking.", Complete),
        ];
        assert_eq!(reply_to(&entries, "When does 2.0 ship?").as_deref(), Some("Looking."));
        entries.push(entry("a2", Assistant, "Friday.", Streaming));
        assert_eq!(reply_to(&entries, "When does 2.0 ship?"), None, "still streaming");
        entries[4].status = Some(Complete);
        assert_eq!(reply_to(&entries, "When does 2.0 ship?").as_deref(), Some("Friday."));
        entries.push(entry("u2", User, "next thing", Complete));
        entries.push(entry("a3", Assistant, "not this", Complete));
        assert_eq!(reply_to(&entries, "When does 2.0 ship?").as_deref(), Some("Friday."), "only that turn");
        assert_eq!(reply_to(&entries, "never sent"), None);
        assert_eq!(reply_to(&entries[..3], "When does 2.0 ship?"), None, "no reply yet");
    }

    #[test]
    fn a_turn_ends_once_per_completed_turn() {
        let session = |status, turn: Option<&str>| Session {
            last_completed_turn: turn.map(Into::into),
            chat_id: "c".into(),
            device_id: "d".into(),
            status,
            started_at: None,
            updated_at: Utc::now(),
            last_turn_outcome: None,
            terminal_activity: None,
            running_subagents: 0,
            background_tasks: vec![],
            waiting: None,
            activity: None,
            todo: None,
        };
        let working = session(SessionStatus::Working, Some("t0"));
        let done = session(SessionStatus::Idle, Some("t1"));
        assert!(ended(Some(&working), &done));
        assert!(!ended(Some(&done), &done), "a heartbeat isn't an end");
        assert!(!ended(Some(&done), &working));
    }

    fn ask(id: &str, minutes_ago: i64, state: AskState) -> Ask {
        Ask {
            id: id.into(),
            from_chat: "c".into(),
            orchestrator_id: None,
            item_id: None,
            kind: AskKind::Question,
            question: "?".into(),
            options: vec![],
            urgent: false,
            state,
            answer: None,
            created_at: Utc::now() - TimeDelta::minutes(minutes_ago),
            answered_at: None,
        }
    }

    #[test]
    fn an_ask_shows_once_on_each_device_even_across_restarts() {
        let dir = tempfile::tempdir().unwrap();
        let state = OrchestrationState {
            asks: vec![ask("fresh", 1, AskState::Open), ask("stale", 30, AskState::Open), ask("done", 1, AskState::Answered)],
            ..Default::default()
        };
        let mut shown = Shown::load(Some(dir.path()));
        let first: Vec<_> = asks_to_show(&state, &mut shown, Utc::now()).iter().map(|a| a.id.clone()).collect();
        assert_eq!(first, ["fresh"]);
        assert!(asks_to_show(&state, &mut shown, Utc::now()).is_empty(), "once");
        let mut restarted = Shown::load(Some(dir.path()));
        assert!(asks_to_show(&state, &mut restarted, Utc::now()).is_empty(), "not again after a restart");
    }
}
