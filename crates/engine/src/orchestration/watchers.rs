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
    // An agent's message about to run in a Brain dump or an orchestrator
    // hosted here, whatever engine sent it: filed instead.
    if let Some(rpc) = orchestration.rpc() {
        let incoming = orchestration.clone();
        rpc.doc_host_ref().set_incoming_messages(std::sync::Arc::new(move |chat: &str, prompt: &str| {
            super::incoming::consume(&incoming, chat, prompt)
        }));
    }
    // Question routes nobody answered.
    let chase = orchestration.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(CHASE_EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            if let Some(rpc) = chase.rpc() {
                super::incoming::chase_unanswered(&chase, &rpc, Utc::now());
            }
        }
    });
    tokio::spawn(async move { route_answers(orchestration).await });
}

/// How often question routes are checked for going unanswered.
const CHASE_EVERY: std::time::Duration = std::time::Duration::from_secs(60);

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

pub(crate) fn target_chat(target: &RouteTarget) -> Option<&str> {
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

/// One route waiting on what comes of it from a chat.
pub(crate) struct Waiting<'a> {
    pub dump_id: &'a str,
    pub route_id: &'a str,
    /// What it was sent (found again in the target's transcript).
    pub message: String,
    pub excerpt: &'a str,
    pub intent: RouteIntent,
}

/// Every route waiting on `chat`: a question until it is answered; any
/// other delivered route (a task, info, priority, cancel) until it is done,
/// its outcome updated each turn.
pub(crate) fn waiting_on<'a>(dumps: &'a [Dump], chat: &str) -> Vec<Waiting<'a>> {
    dumps
        .iter()
        .flat_map(|d| d.routes.iter().map(move |r| (d, r)))
        .filter(|(_, r)| match r.intent {
            RouteIntent::Question => r.state == RouteState::Delivered,
            _ => matches!(r.state, RouteState::Delivered | RouteState::Answered),
        })
        .filter(|(_, r)| target_chat(&r.target) == Some(chat))
        .map(|(d, r)| Waiting {
            dump_id: &d.id,
            route_id: &r.id,
            message: if r.message.trim().is_empty() { r.excerpt.clone() } else { r.message.clone() },
            excerpt: &r.excerpt,
            intent: r.intent,
        })
        .collect()
}

/// The latest reply after the message carrying `message` (across the
/// turns since), once the newest turn is done. `None` while it isn't.
pub(crate) fn latest_reply(entries: &[SessionMessageEntry], message: &str) -> Option<String> {
    let message = message.trim();
    if message.is_empty() {
        return None;
    }
    let at = entries
        .iter()
        .rposition(|e| e.role == MessageRole::User && texts(e).contains(message))?;
    let after: Vec<_> = entries[at + 1..].iter().filter(|e| e.role == MessageRole::Assistant).collect();
    if after.is_empty() || after.iter().any(|e| e.status == Some(MessageStatus::Streaming)) {
        return None;
    }
    let reply = after.iter().rev().map(|e| texts(e)).find(|t| !t.trim().is_empty())?;
    Some(reply.trim().chars().take(FULL_MAX).collect())
}

/// Where a task route stands once its target's turn ended, from the
/// target's session: failed when the turn errored; done when it went idle
/// with nothing running or left on its todo list; else still going.
pub(crate) fn task_state(session: &Session) -> RouteState {
    if session
        .last_turn_outcome
        .is_some_and(|o| o.status == zeron_proto::DoneStatus::Errored)
        || session.status == SessionStatus::Errored
    {
        return RouteState::Failed;
    }
    let busy = session.waiting.is_some()
        || session.running_subagents > 0
        || !session.background_tasks.is_empty()
        || session.todo.as_ref().is_some_and(|t| t.done < t.total);
    if busy { RouteState::Answered } else { RouteState::Done }
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
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) async fn record_answer(
    orchestration: &Orchestration,
    rpc: &crate::rpc::EngineRpc,
    chat: &str,
    dump_id: &str,
    route_id: &str,
    question: &str,
    full: String,
) -> bool {
    record_outcome(orchestration, rpc, chat, dump_id, route_id, question, full, None).await
}

/// [`record_answer`] for any route: `task` is where a task route stands now
/// ([`task_state`]; `None` for a question, which is answered once). A task's
/// outcome is updated each turn until it is done or failed. Shows in comms
/// from the target to the person, and as a banner for a question or a
/// failure.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn record_outcome(
    orchestration: &Orchestration,
    rpc: &crate::rpc::EngineRpc,
    chat: &str,
    dump_id: &str,
    route_id: &str,
    question: &str,
    full: String,
    task: Option<RouteState>,
) -> bool {
    use zeron_proto::orchestration::{ANSWER_MAX, CommKind, model_answer, plain_answer, prompt_text::one_line};
    let plain = Some(plain_answer(&full)).filter(|a| !a.is_empty());
    let quick = plain.clone().unwrap_or_else(|| one_line(&full, ANSWER_MAX));
    let state = task.unwrap_or(RouteState::Answered);
    // A later turn that only says it sent its result (to the Brain dump)
    // keeps the outcome it sent; only the state moves.
    let keeps_outcome = task.is_some() && plain.is_none();
    let recorded = orchestration.workspace().write_orchestration(|doc| {
        let Some(mut dump) = doc.dump(dump_id) else { return false };
        let Some(route) = dump.routes.iter_mut().find(|r| r.id == route_id) else { return false };
        let open = match task {
            None => route.state == RouteState::Delivered,
            Some(_) => matches!(route.state, RouteState::Delivered | RouteState::Answered),
        };
        // The same reply and state again (a heartbeat): nothing new.
        if !open || (route.state == state && route.answer_full.as_deref() == Some(full.as_str())) {
            return false;
        }
        route.state = state;
        if !(keeps_outcome && route.answer.is_some()) {
            route.answer = Some(quick.clone());
            route.answer_full = Some(full.clone());
        }
        dump.updated_at = Utc::now();
        doc.upsert_dump(&dump);
        true
    });
    if !recorded {
        return false;
    }
    let kind = if task.is_some() { CommKind::Report } else { CommKind::Answer };
    orchestration.comms().say(kind, chat, super::comms::YOU, &quick);
    // Its card on the board follows.
    rpc.sync_route_card(dump_id, route_id);
    // A chase of this route ("hasn't answered") is moot now.
    if task.is_none() {
        super::incoming::settle_chases(orchestration, route_id);
    }
    // An outcome that ends on a question for the person: an ask from the
    // thread, answered back to it.
    if let Some(asks) = super::incoming::ends_on_question(&full) {
        rpc.ask_for(chat, &asks, Some((dump_id, route_id)));
    }
    if task.is_none() || state == RouteState::Failed {
        let title = rpc
            .workspace_host()
            .chat(chat)
            .ok()
            .flatten()
            .and_then(|c| c.title)
            .unwrap_or_else(|| "A thread".into());
        rpc.notify_person(&title, &quick, Some(chat));
    }
    let reply: String = zeron_proto::redact::redact_command_output(&full).chars().take(CONDENSE_INPUT_MAX).collect();
    let (instructions, label, key) = match task {
        None => (zeron_harness::ANSWER_INSTRUCTIONS, "Question and reply", "question"),
        Some(_) => (zeron_harness::OUTCOME_INSTRUCTIONS, "Request and reply", "request"),
    };
    let data = serde_json::json!({ key: one_line(question, 600), "reply": reply });
    let prompt = format!(
        "{instructions}\n\n{label} (JSON):\n{}",
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
            // Only over the answer it condenses (not one a reroute or a later
            // turn replaced).
            if route.answer_full.as_deref() != Some(full.as_str()) {
                return;
            }
            route.answer = Some(condensed);
            dump.updated_at = Utc::now();
            doc.upsert_dump(&dump);
        });
    }
    true
}

/// Whether the engine hosting a chat on `device_id` files what comes of
/// the routes to it itself: this one, or one that says so. For a chat on an
/// older host, the dump's own engine does it from the synced session row
/// and transcript.
pub(crate) fn files_own_routes(orchestration: &Orchestration, device_id: &str) -> bool {
    device_id == orchestration.device_id()
        || orchestration.workspace().read_devices().unwrap_or_default().iter().any(|d| {
            d.id == device_id && d.supports(zeron_proto::capabilities::ORCHESTRATION_ROUTES_V1)
        })
}

/// Which turns' ends a watch is about.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Hosted {
    /// A chat this engine runs: every route to it, any dump's.
    Here,
    /// A chat on an older host: the routes of this engine's own dumps.
    Older,
}

/// How long after an older host's turn ended its transcript may still be
/// syncing in: the reply is looked for again after each of these.
const SYNC_RETRIES: [std::time::Duration; 4] = [
    std::time::Duration::from_secs(3),
    std::time::Duration::from_secs(10),
    std::time::Duration::from_secs(30),
    std::time::Duration::from_secs(90),
];

async fn route_answers(orchestration: Orchestration) {
    let mut sessions_rx = loop {
        match orchestration.rpc() {
            Some(rpc) => break rpc.sessions_ref().watch_sessions(),
            None => tokio::time::sleep(std::time::Duration::from_secs(1)).await,
        }
    };
    let mut rows_rx = orchestration.workspace().watch_session_rows();
    let mut seen: HashMap<String, Session> =
        sessions_rx.borrow_and_update().iter().map(|s| (s.chat_id.clone(), s.clone())).collect();
    let mut seen_rows: HashMap<String, Session> =
        rows_rx.borrow_and_update().iter().map(|s| (s.chat_id.clone(), s.clone())).collect();
    // What ended while this engine wasn't running (or before this fix).
    if let Some(rpc) = orchestration.rpc() {
        let idle: Vec<(String, Session)> = seen_rows
            .values()
            .filter(|s| !matches!(s.status, SessionStatus::Working | SessionStatus::AwaitingInput))
            .map(|s| (s.chat_id.clone(), s.clone()))
            .collect();
        let (here, older): (Vec<_>, Vec<_>) =
            idle.into_iter().partition(|(_, s)| s.device_id == orchestration.device_id());
        turns_ended(&orchestration, &rpc, here, Hosted::Here, false);
        turns_ended(&orchestration, &rpc, older, Hosted::Older, false);
    }
    loop {
        let (finished, hosted) = tokio::select! {
            changed = sessions_rx.changed() => {
                if changed.is_err() {
                    break;
                }
                let now: Vec<Session> = sessions_rx.borrow_and_update().clone();
                let finished: Vec<(String, Session)> = now
                    .iter()
                    .filter(|s| ended(seen.get(&s.chat_id), s))
                    .map(|s| (s.chat_id.clone(), s.clone()))
                    .collect();
                seen = now.into_iter().map(|s| (s.chat_id.clone(), s)).collect();
                (finished, Hosted::Here)
            }
            changed = rows_rx.changed() => {
                if changed.is_err() {
                    break;
                }
                let now: Vec<Session> = rows_rx.borrow_and_update().clone();
                let here = orchestration.device_id().to_owned();
                let finished: Vec<(String, Session)> = now
                    .iter()
                    .filter(|s| s.device_id != here && ended(seen_rows.get(&s.chat_id), s))
                    .map(|s| (s.chat_id.clone(), s.clone()))
                    .collect();
                seen_rows = now.into_iter().map(|s| (s.chat_id.clone(), s)).collect();
                (finished, Hosted::Older)
            }
        };
        if finished.is_empty() {
            continue;
        }
        // (No services right now: nothing to read the transcripts with.)
        let Some(rpc) = orchestration.rpc() else { continue };
        turns_ended(&orchestration, &rpc, finished, hosted, true);
    }
}

/// Take in what came of the routes to chats whose turn ended.
fn turns_ended(
    orchestration: &Orchestration,
    rpc: &std::sync::Arc<crate::rpc::EngineRpc>,
    finished: Vec<(String, Session)>,
    hosted: Hosted,
    just_now: bool,
) {
    let dumps = orchestration.state().dumps;
    let here = orchestration.device_id().to_owned();
    for (chat, session) in finished {
        if hosted == Hosted::Here && just_now {
            // A Brain dump's turn that left a dump routing.
            super::stale_dumps::after_turn(orchestration, rpc, &chat);
        }
        if hosted == Hosted::Older && files_own_routes(orchestration, &session.device_id) {
            continue;
        }
        let waiting: Vec<_> = waiting_on(&dumps, &chat)
            .into_iter()
            .filter(|w| hosted == Hosted::Here || dumps.iter().any(|d| d.id == w.dump_id && d.device_id == here))
            .map(|w| (w.dump_id.to_owned(), w.route_id.to_owned(), w.message, w.excerpt.to_owned(), w.intent))
            .collect();
        if waiting.is_empty() {
            continue;
        }
        let (orchestration, rpc) = (orchestration.clone(), rpc.clone());
        tokio::spawn(async move {
            // An older host's transcript may still be on its way.
            let retries: &[std::time::Duration] = if hosted == Hosted::Older && just_now { &SYNC_RETRIES } else { &[] };
            let mut left = waiting;
            for wait in std::iter::once(None).chain(retries.iter().copied().map(Some)) {
                if let Some(wait) = wait {
                    tokio::time::sleep(wait).await;
                }
                left = take_outcomes(&orchestration, &rpc, &chat, &session, left).await;
                if left.is_empty() {
                    break;
                }
            }
        });
    }
}

type WaitingRoute = (String, String, String, String, RouteIntent);

/// Record what `chat`'s transcript says came of each route; the routes it
/// says nothing about yet.
async fn take_outcomes(
    orchestration: &Orchestration,
    rpc: &crate::rpc::EngineRpc,
    chat: &str,
    session: &Session,
    waiting: Vec<WaitingRoute>,
) -> Vec<WaitingRoute> {
    let Ok(handle) = rpc.doc_host_ref().open(chat) else { return waiting };
    let Ok(entries) = handle.doc().read_entries() else { return waiting };
    let mut left = Vec::new();
    for w in waiting {
        let (dump_id, route_id, message, excerpt, intent) = &w;
        let question = *intent == RouteIntent::Question;
        let full = if question { reply_to(&entries, message) } else { latest_reply(&entries, message) };
        let Some(full) = full else {
            left.push(w);
            continue;
        };
        let task = (!question).then(|| task_state(session));
        let asked = if question { excerpt.clone() } else { message.clone() };
        record_outcome(orchestration, rpc, chat, dump_id, route_id, &asked, full, task).await;
    }
    left
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
    fn a_task_is_done_when_its_target_has_nothing_left() {
        use zeron_proto::orchestration::TodoProgress;
        let base = Session {
            last_completed_turn: Some("t".into()),
            chat_id: "c".into(),
            device_id: "d".into(),
            status: SessionStatus::Idle,
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
        assert_eq!(task_state(&base), RouteState::Done);
        let half = Session { todo: Some(TodoProgress { done: 1, total: 2, current: None }), ..base.clone() };
        assert_eq!(task_state(&half), RouteState::Answered);
        let all = Session { todo: Some(TodoProgress { done: 2, total: 2, current: None }), ..base.clone() };
        assert_eq!(task_state(&all), RouteState::Done);
        let helping = Session { running_subagents: 1, ..base.clone() };
        assert_eq!(task_state(&helping), RouteState::Answered);
        let failed = Session { status: SessionStatus::Errored, ..base };
        assert_eq!(task_state(&failed), RouteState::Failed);
    }

    #[test]
    fn the_latest_reply_spans_the_turns_since_delivery() {
        use MessageRole::*;
        use MessageStatus::*;
        let entries = vec![
            entry("u1", User, "Check device names.", Complete),
            entry("a1", Assistant, "Half done.", Complete),
            entry("u2", User, "go on", Complete),
            entry("a2", Assistant, "All done.", Complete),
        ];
        assert_eq!(latest_reply(&entries, "Check device names.").as_deref(), Some("All done."));
        assert_eq!(reply_to(&entries, "Check device names.").as_deref(), Some("Half done."));
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
            dump_id: None,
            route_id: None,
            quiet: Default::default(),
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
