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
/// How long a reply's text is before it's taken as the answer, not a
/// preamble ("Looking.").
const SUBSTANTIAL: usize = 80;
/// The most a whole reply (`answerFull`) keeps.
const FULL_MAX: usize = 8000;
/// The most of a reply the condensing run reads.
const CONDENSE_INPUT_MAX: usize = 4000;

/// How long after a start dumps left routing are swept (queued sends
/// drain first).
const SWEEP_AFTER: std::time::Duration = std::time::Duration::from_secs(20);

pub(crate) fn start(orchestration: Orchestration) {
    let asks = orchestration.clone();
    orchestration.spawn(async move { ask_banners(asks).await });
    let sweep = orchestration.clone();
    orchestration.spawn(async move {
        tokio::time::sleep(SWEEP_AFTER).await;
        if let Some(rpc) = sweep.rpc() {
            let settled = super::stale_dumps::sweep(&sweep, &rpc);
            if !settled.is_empty() {
                tracing::info!(dumps = settled.len(), "brain dump: dumps left routing now need the person");
            }
            // Cards waiting on a decision their thread put in words: its ask.
            rpc.settle_card_states().await;
        }
    });
    // An agent's message about to run in a Brain dump or an orchestrator
    // hosted here, whatever engine sent it: filed instead.
    if let Some(rpc) = orchestration.rpc() {
        let incoming = orchestration.clone();
        rpc.doc_host_ref().set_incoming_messages(std::sync::Arc::new(move |chat: &str, prompt: &str| {
            incoming.is_on() && super::incoming::consume(&incoming, chat, prompt)
        }));
    }
    // Question routes nobody answered.
    let chase = orchestration.clone();
    orchestration.spawn(async move {
        let mut tick = tokio::time::interval(CHASE_EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            if let Some(rpc) = chase.rpc() {
                let perf_t = std::time::Instant::now();
                super::incoming::chase_unanswered(&chase, &rpc, Utc::now());
                tracing::debug!(target: "perf", what = "chase_tick", us = perf_t.elapsed().as_micros() as u64);
            }
        }
    });
    super::turn_asks::start(orchestration.clone());
    // Crew threads left idle, archived (the profile's setting).
    super::crew_archive::start(orchestration.clone());
    // Questions still waiting on a reply that's there: one whose
    // transcript synced in after every retry, or came while this engine
    // wasn't running and its thread hasn't ended a turn since.
    let sweep = orchestration.clone();
    orchestration.spawn(async move {
        tokio::time::sleep(SWEEP_AFTER).await;
        let mut tick = tokio::time::interval(SWEEP_QUESTIONS_EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            if let Some(rpc) = sweep.rpc() {
                let perf_t = std::time::Instant::now();
                sweep_questions(&sweep, &rpc).await;
                tracing::debug!(target: "perf", what = "qa_sweep", us = perf_t.elapsed().as_micros() as u64);
            }
        }
    });
    let run = orchestration.clone();
    run.spawn(async move { route_answers(orchestration).await });
}

/// Whether a condensed answer came back as a title ("Board Inbox And Next
/// Columns"), not a sentence: no closing stop, and every word capitalised.
/// The one-shot runs on the harnesses' title path, whose system prompt asks
/// for titles; such a line is dropped and the reply's own first sentence
/// stands.
pub(crate) fn reads_as_title(answer: &str) -> bool {
    let text = answer.trim();
    if text.ends_with(['.', '!', '?', ')', '`']) {
        return false;
    }
    let words: Vec<&str> = text.split_whitespace().filter(|w| w.chars().next().is_some_and(char::is_alphabetic)).collect();
    words.len() >= 2 && words.iter().all(|w| w.chars().next().is_some_and(char::is_uppercase))
}

/// How often questions still waiting are looked for in their transcripts.
const SWEEP_QUESTIONS_EVERY: std::time::Duration = std::time::Duration::from_secs(300);

/// Take in the reply each question route of this engine's dumps still
/// waits on, where its target's transcript (here, or as synced) has one.
/// How many it took in.
pub(crate) async fn sweep_questions(orchestration: &Orchestration, rpc: &crate::rpc::EngineRpc) -> usize {
    let here = orchestration.device_id().to_owned();
    let mut by_chat: HashMap<String, Vec<WaitingRoute>> = HashMap::new();
    let dumps = orchestration.scan(|s| s.dumps.iter().filter(|d| d.device_id == here).cloned().collect::<Vec<_>>());
    for d in &dumps {
        for r in &d.routes {
            if r.intent != RouteIntent::Question || r.state != RouteState::Delivered {
                continue;
            }
            let Some(chat) = target_chat(&r.target).filter(|c| !c.is_empty()) else { continue };
            let message = if r.message.trim().is_empty() { r.excerpt.clone() } else { r.message.clone() };
            by_chat
                .entry(chat.to_owned())
                .or_default()
                .push((d.id.clone(), r.id.clone(), message, r.excerpt.clone(), r.intent));
        }
    }
    let checked: usize = by_chat.values().map(Vec::len).sum();
    let mut taken = 0;
    for (chat, waiting) in by_chat {
        let asked = waiting.len();
        taken += asked - take_outcomes(orchestration, rpc, &chat, None, waiting).await.len();
    }
    tracing::info!(checked, filled = taken, "q&a: questions still waiting looked for in their transcripts");
    taken
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
/// turn is done. `None` while it isn't. ([`answer_to`], its text.)
pub(crate) fn reply_to(entries: &[SessionMessageEntry], message: &str) -> Option<String> {
    answer_to(entries, message).map(|(text, _)| text)
}

/// The reply to the message carrying `message` in `entries`, and when it
/// was written (ms since the epoch): the first real reply of the turn after
/// it (before the next message to the chat; its last text when none is
/// more than a line), once that turn is done.
/// Each of several questions sent to one thread so has the reply that
/// follows its own delivery. One sent right before another message (two in
/// a row, the thread answering both at once) has the first reply after it.
/// `None` while there's none, or while it's still being written.
pub(crate) fn answer_to(entries: &[SessionMessageEntry], message: &str) -> Option<(String, i64)> {
    let message = message.trim();
    if message.is_empty() {
        return None;
    }
    let at = entries
        .iter()
        .rposition(|e| e.role == MessageRole::User && texts(e).contains(message))?;
    let after = &entries[at + 1..];
    let mut turn: Vec<&SessionMessageEntry> = after
        .iter()
        .take_while(|e| e.role != MessageRole::User)
        .filter(|e| e.role == MessageRole::Assistant)
        .collect();
    if !turn.iter().any(|e| !texts(e).trim().is_empty()) {
        // (Another message came before any reply: the first turn that
        // replies after it.)
        turn = after
            .iter()
            .skip_while(|e| e.role != MessageRole::Assistant || texts(e).trim().is_empty())
            .take_while(|e| e.role != MessageRole::User)
            .filter(|e| e.role == MessageRole::Assistant)
            .collect();
    }
    if turn.is_empty() || turn.iter().any(|e| e.status == Some(MessageStatus::Streaming)) {
        return None;
    }
    // Its first real reply (a line like "Looking." aside), else its last
    // word: a long turn goes on to other work after it answers.
    let said: Vec<&&SessionMessageEntry> = turn.iter().filter(|e| !texts(e).trim().is_empty()).collect();
    let reply = said
        .iter()
        .find(|e| texts(e).trim().chars().count() >= SUBSTANTIAL)
        .or_else(|| said.last())?;
    Some((texts(reply).trim().chars().take(FULL_MAX).collect(), reply.created_at))
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
    latest_reply_until(entries, message, &[])
}

/// [`latest_reply`], up to the delivery of any of `others` (other routes'
/// messages to the same chat): what came after one is that route's, so
/// each card keeps the outcome of its own work.
pub(crate) fn latest_reply_until(entries: &[SessionMessageEntry], message: &str, others: &[String]) -> Option<String> {
    let message = message.trim();
    if message.is_empty() {
        return None;
    }
    let at = entries
        .iter()
        .rposition(|e| e.role == MessageRole::User && texts(e).contains(message))?;
    let next = entries[at + 1..]
        .iter()
        .position(|e| {
            e.role == MessageRole::User && {
                let t = texts(e);
                others.iter().map(|o| o.trim()).any(|o| !o.is_empty() && o != message && t.contains(o))
            }
        })
        .map_or(entries.len(), |i| at + 1 + i);
    let after: Vec<_> = entries[at + 1..next].iter().filter(|e| e.role == MessageRole::Assistant).collect();
    if after.is_empty() || after.iter().any(|e| e.status == Some(MessageStatus::Streaming)) {
        return None;
    }
    let reply = after.iter().rev().map(|e| texts(e)).find(|t| !t.trim().is_empty())?;
    Some(reply.trim().chars().take(FULL_MAX).collect())
}

/// Where a task route stands once its target's turn ended, from the
/// target's session: failed when the turn errored; done when it went idle
/// with nothing running or left on its todo list; else still going
/// (`Answered`: an outcome so far, its card in Now, never Review).
pub(crate) fn task_state(session: &Session) -> RouteState {
    // Stopped by its usage limit: resumed at the reset, still going.
    if session.limited.is_some() {
        return RouteState::Answered;
    }
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

/// A local chat's turn started since `before` (a message reached it).
fn started(before: Option<&Session>, now: &Session) -> bool {
    now.status == SessionStatus::Working && before.is_some_and(|b| b.status != SessionStatus::Working)
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
    record_outcome_at(orchestration, rpc, chat, dump_id, route_id, question, full, task, None).await
}

/// How long ago a reply may have been written and still be news: one
/// older (taken in after a restart, or synced in late) is recorded quietly,
/// with no banner and nothing in Comms.
const STILL_NEWS: TimeDelta = TimeDelta::minutes(10);

/// [`record_outcome`] for a reply written at `at` (its transcript's time;
/// `None`: now). The route's `answeredAt` is that time, so an answer taken
/// in late says when it came; an old one is recorded quietly.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn record_outcome_at(
    orchestration: &Orchestration,
    rpc: &crate::rpc::EngineRpc,
    chat: &str,
    dump_id: &str,
    route_id: &str,
    question: &str,
    full: String,
    task: Option<RouteState>,
    at: Option<chrono::DateTime<Utc>>,
) -> bool {
    let news = at.is_none_or(|at| Utc::now() - at <= STILL_NEWS);
    use zeron_proto::orchestration::{ANSWER_MAX, CommKind, model_answer, plain_answer, prompt_text::one_line};
    let plain = Some(plain_answer(&full)).filter(|a| !a.is_empty());
    let quick = plain.clone().unwrap_or_else(|| one_line(&full, ANSWER_MAX));
    let state = task.unwrap_or(RouteState::Answered);
    // A later turn that only says it sent its result (to the Brain dump)
    // keeps the outcome it sent; only the state moves.
    let keeps_outcome = task.is_some() && plain.is_none();
    // Finished (its thread at rest): a decision or question for the person
    // means it isn't (its card stays in Now, the question an ask, raised
    // below); else what it leaves them to evaluate (an open PR, a result to
    // check, screenshots) puts its card in Review, and nothing, Done. From
    // this reply, else (a turn that only says it sent its result) the
    // outcome it sent.
    let (state, review) = if task == Some(RouteState::Done) {
        let sent = orchestration
            .workspace()
            .read_orchestration_with(|doc| doc.dump(dump_id))
            .and_then(|d| d.routes.into_iter().find(|r| r.id == route_id))
            .and_then(|r| r.answer_full)
            .filter(|_| keeps_outcome);
        let texts: Vec<&str> = std::iter::once(full.as_str()).chain(sent.as_deref()).collect();
        let mut decided = None;
        for text in &texts {
            if waits_on_decision(orchestration, rpc, chat, text).await {
                decided = Some((RouteState::Answered, None));
                break;
            }
        }
        match decided {
            Some(d) => d,
            None => (
                RouteState::Done,
                texts.iter().find_map(|t| zeron_proto::turn_question::review_of(t)).map(|r| r.line()),
            ),
        }
    } else {
        (state, None)
    };
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
        if task.is_some() {
            route.review = review.clone();
        }
        // (A decision left the route going: it stays open for the turns
        // after the person's answer.)
        if route.answered_at.is_none() {
            route.answered_at = Some(at.unwrap_or_else(Utc::now));
        }
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
    if news {
        orchestration.comms().say(kind, chat, super::comms::YOU, &quick);
    }
    // Its card on the board follows.
    rpc.sync_route_card(dump_id, route_id);
    // A chase of this route ("hasn't answered") is moot now.
    if task.is_none() {
        super::incoming::settle_chases(orchestration, route_id);
    }
    // An outcome that ends on a question for the person: an ask from the
    // thread, answered back to it.
    if let Some(found) = super::incoming::ends_on_question(&full) {
        rpc.ask_for(chat, &found, Some((dump_id, route_id)));
    }
    if news && (task.is_none() || state == RouteState::Failed) {
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
        .filter(|a| !a.is_empty() && !reads_as_title(a));
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

/// Whether a finished turn's `text` ends waiting on a decision or answer
/// from the person ([`zeron_proto::turn_question::detect`]): plainly, or a
/// borderline one the cheap model confirms (it stands when the model can't
/// be asked, as for asks).
pub(crate) async fn waits_on_decision(
    orchestration: &Orchestration,
    rpc: &crate::rpc::EngineRpc,
    chat: &str,
    text: &str,
) -> bool {
    let harness = rpc
        .workspace_host()
        .chat(chat)
        .ok()
        .flatten()
        .and_then(|c| c.config.map(|c| c.harness))
        .unwrap_or_else(|| rpc.doc_host_ref().default_harness());
    let one_shot = orchestration.progress().one_shot(rpc);
    super::turn_asks::settle_verdict(text, |prompt| one_shot(harness, prompt)).await.is_some()
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
    let Some(rpc) = orchestration.rpc() else { return };
    let mut sessions_rx = rpc.sessions_ref().watch_sessions();
    drop(rpc);
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
        let dumps = orchestration.scan(|s| s.dumps.clone());
        let open = |chats: &[(String, Session)]| -> usize {
            chats.iter().map(|(chat, _)| waiting_on(&dumps, chat).len()).sum()
        };
        tracing::info!(
            chats = here.len() + older.len(),
            routes = open(&here) + open(&older),
            "q&a: startup pass over turns that ended while this engine wasn't running"
        );
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
                // A message reached a thread: the turn-end asks it answers close.
                let replied: Vec<String> =
                    now.iter().filter(|s| started(seen.get(&s.chat_id), s)).map(|s| s.chat_id.clone()).collect();
                if let Some(rpc) = orchestration.rpc() {
                    for chat in replied {
                        let (o, r) = (orchestration.clone(), rpc.clone());
                        tokio::spawn(async move { super::turn_asks::on_reply(&o, &r, &chat).await });
                    }
                }
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
/// Whether this engine launched `chat`: a dump of its own routed to it, or
/// it is in the crew of an orchestrator whose chat runs here.
pub(crate) fn launched_here(orchestration: &Orchestration, dumps: &[Dump], chat: &str) -> bool {
    let here = orchestration.device_id();
    if dumps.iter().any(|d| d.device_id == here && d.routes.iter().any(|r| target_chat(&r.target) == Some(chat))) {
        return true;
    }
    orchestration.crew_of(chat).is_some_and(|m| {
        orchestration.workspace().chat(&m.orchestrator_id).ok().flatten().is_some_and(|o| o.device_id == here)
    })
}

fn turns_ended(
    orchestration: &Orchestration,
    rpc: &std::sync::Arc<crate::rpc::EngineRpc>,
    finished: Vec<(String, Session)>,
    hosted: Hosted,
    just_now: bool,
) {
    let dumps = orchestration.scan(|s| s.dumps.clone());
    let here = orchestration.device_id().to_owned();
    for (chat, session) in finished {
        if hosted == Hosted::Here && just_now {
            // A Brain dump's turn that left a dump routing.
            super::stale_dumps::after_turn(orchestration, rpc, &chat);
            // A launched thread's turn that ended on a question for the person.
            let (o, r, c, s) = (orchestration.clone(), rpc.clone(), chat.clone(), session.clone());
            tokio::spawn(async move {
                super::turn_asks::after_turn(&o, &r, &c, &s).await;
            });
        }
        if hosted == Hosted::Older && files_own_routes(orchestration, &session.device_id) {
            continue;
        }
        // A thread on an older host (no turn-end asks of its own) that this
        // engine launched: a dump here routed to it, or it is in the crew of
        // an orchestrator hosted here. Its question is raised here, once its
        // transcript has synced in; the ask's id is the chat and the asking
        // message, so another engine raising it too makes the same row.
        if hosted == Hosted::Older && just_now && launched_here(orchestration, &dumps, &chat) {
            let (o, r, c, s) = (orchestration.clone(), rpc.clone(), chat.clone(), session.clone());
            orchestration.spawn(async move {
                tokio::time::sleep(SYNC_RETRIES[0] + SYNC_RETRIES[1]).await;
                super::turn_asks::after_turn(&o, &r, &c, &s).await;
            });
        }
        let waiting: Vec<_> = waiting_on(&dumps, &chat)
            .into_iter()
            .filter(|w| hosted == Hosted::Here || dumps.iter().any(|d| d.id == w.dump_id && d.device_id == here))
            .map(|w| (w.dump_id.to_owned(), w.route_id.to_owned(), w.message, w.excerpt.to_owned(), w.intent))
            .collect();
        if waiting.is_empty() {
            continue;
        }
        let run = orchestration.clone();
        let (orchestration, rpc) = (orchestration.clone(), rpc.clone());
        run.spawn(async move {
            // An older host's transcript may still be on its way.
            let retries: &[std::time::Duration] = if hosted == Hosted::Older && just_now { &SYNC_RETRIES } else { &[] };
            let mut left = waiting;
            for wait in std::iter::once(None).chain(retries.iter().copied().map(Some)) {
                if let Some(wait) = wait {
                    tokio::time::sleep(wait).await;
                }
                left = take_outcomes(&orchestration, &rpc, &chat, Some(&session), left).await;
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
    session: Option<&Session>,
    waiting: Vec<WaitingRoute>,
) -> Vec<WaitingRoute> {
    let _perf = crate::PerfGuard::new("take_outcomes");
    let Ok(handle) = rpc.doc_host_ref().open(chat) else { return waiting };
    let Ok(entries) = handle.doc().read_entries() else { return waiting };
    // Every route's message to this chat: a task's outcome ends where the
    // next one was delivered.
    let others: Vec<String> = orchestration
        .state()
        .dumps
        .iter()
        .flat_map(|d| &d.routes)
        .filter(|r| target_chat(&r.target) == Some(chat))
        .map(|r| if r.message.trim().is_empty() { r.excerpt.clone() } else { r.message.clone() })
        .collect();
    let mut left = Vec::new();
    for w in waiting {
        let (dump_id, route_id, message, excerpt, intent) = &w;
        let question = *intent == RouteIntent::Question;
        // (A question's reply, and when it was written.)
        let found = if question {
            answer_to(&entries, message).map(|(text, ms)| (text, (ms > 0).then(|| chrono::DateTime::from_timestamp_millis(ms)).flatten()))
        } else {
            latest_reply_until(&entries, message, &others).map(|text| (text, None))
        };
        let Some((full, at)) = found else {
            left.push(w);
            continue;
        };
        // (A task's state is its session's: none known, it waits.)
        let task = match (question, session) {
            (true, _) => None,
            (false, Some(session)) => Some(task_state(session)),
            (false, None) => {
                left.push(w);
                continue;
            }
        };
        let asked = if question { excerpt.clone() } else { message.clone() };
        record_outcome_at(orchestration, rpc, chat, dump_id, route_id, &asked, full, task, at).await;
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
            live: Default::default(),
        };
        assert_eq!(task_state(&base), RouteState::Done);
        let half = Session { live: zeron_proto::SessionLive { todo: Some(TodoProgress { done: 1, total: 2, current: None }), ..base.live.clone() }, ..base.clone() };
        assert_eq!(task_state(&half), RouteState::Answered);
        let all = Session { live: zeron_proto::SessionLive { todo: Some(TodoProgress { done: 2, total: 2, current: None }), ..base.live.clone() }, ..base.clone() };
        assert_eq!(task_state(&all), RouteState::Done);
        let helping = Session { running_subagents: 1, ..base.clone() };
        assert_eq!(task_state(&helping), RouteState::Answered);
        let failed = Session { status: SessionStatus::Errored, ..base };
        assert_eq!(task_state(&failed), RouteState::Failed);
    }

    #[test]
    fn each_question_has_the_reply_after_its_own_delivery() {
        use MessageRole::*;
        use MessageStatus::*;
        let at = |e: SessionMessageEntry, ms: i64| SessionMessageEntry { created_at: ms, ..e };
        let entries = vec![
            at(entry("u1", User, "[Message from Zeron chat Brain dump (b).]\n\nAre Inbox and Next used?", Complete), 100),
            at(entry("a1", Assistant, "Essentially never.", Complete), 200),
            at(entry("u2", User, "[Message from Zeron chat Brain dump (b).]\n\nWhy is Progress empty?", Complete), 300),
            at(entry("a2", Assistant, "Looking.", Complete), 400),
            at(entry("a3", Assistant, "Its rows froze.", Complete), 500),
            // Two in a row, answered at once.
            at(entry("u3", User, "Which model does the Brain dump use?", Complete), 600),
            at(entry("u4", User, "And where are the orchestrators?", Complete), 700),
            at(entry("a4", Assistant, "Haiku; the sidebar.", Complete), 800),
        ];
        assert_eq!(answer_to(&entries, "Are Inbox and Next used?"), Some(("Essentially never.".into(), 200)));
        assert_eq!(answer_to(&entries, "Why is Progress empty?"), Some(("Its rows froze.".into(), 500)));
        assert_eq!(answer_to(&entries, "Which model does the Brain dump use?"), Some(("Haiku; the sidebar.".into(), 800)));
        assert_eq!(answer_to(&entries, "And where are the orchestrators?"), Some(("Haiku; the sidebar.".into(), 800)));
        assert_eq!(answer_to(&entries[..6], "Which model does the Brain dump use?"), None, "no reply yet");
    }

    /// The rule over a real replica, offline and read-only: each question
    /// route in an orchestration snapshot (JSON, as WatchOrchestration sends
    /// it), its target's transcript from a copy of a docs.sqlite3, and the
    /// reply it would take in. Run on purpose:
    /// `LIVE_DOCS=<copy> LIVE_STATE=<json> cargo test -p zeron-engine live_replay -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_replay() {
        let (Some(db), Some(state)) = (std::env::var_os("LIVE_DOCS"), std::env::var_os("LIVE_STATE")) else {
            return;
        };
        let conn = rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let state: serde_json::Value = serde_json::from_slice(&std::fs::read(state).unwrap()).unwrap();
        let mut found = 0;
        let mut asked = 0;
        for d in state["dumps"].as_array().unwrap() {
            for r in d["routes"].as_array().unwrap() {
                let Some(chat) = r["target"]["id"].as_str().filter(|_| r["intent"] == "question") else { continue };
                asked += 1;
                let doc = zeron_doc::SessionDoc::from_doc(loro::LoroDoc::new());
                let snapshot: Option<Vec<u8>> =
                    conn.query_row("select bytes from snapshots where doc_id = ?1", [chat], |row| row.get(0)).ok();
                if let Some(bytes) = snapshot {
                    doc.import(&bytes).unwrap();
                }
                let mut deltas = conn.prepare("select bytes from snapshot_deltas where doc_id = ?1 order by ordinal").unwrap();
                for bytes in deltas.query_map([chat], |row| row.get::<_, Vec<u8>>(0)).unwrap() {
                    let _ = doc.import(&bytes.unwrap());
                }
                let entries = doc.read_entries().unwrap_or_default();
                let message = r["message"].as_str().unwrap_or_default();
                let reply = answer_to(&entries, message);
                if reply.is_some() {
                    found += 1;
                }
                println!(
                    "{} -> {} ({} entries): {}",
                    one_line_of(r["excerpt"].as_str().unwrap_or_default(), 60),
                    &chat[..8],
                    entries.len(),
                    reply.map_or("NONE".into(), |(t, at)| format!("@{at} {}", one_line_of(&t, 100))),
                );
            }
        }
        println!("{found} of {asked} questions have a reply");
    }

    fn one_line_of(text: &str, max: usize) -> String {
        text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(max).collect()
    }

    #[test]
    fn a_title_is_not_an_answer() {
        assert!(reads_as_title("Board Inbox And Next Columns"));
        assert!(reads_as_title("Xeron Org Lines Count"));
        assert!(!reads_as_title("Essentially never: cards land in Now."));
        assert!(!reads_as_title("The Map reads that same setting, so it's correct"));
        assert!(!reads_as_title("Friday."));
        assert!(!reads_as_title("Yes"));
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
        // Another route delivered since: this one's outcome stops there.
        let mut two = entries.clone();
        two.push(entry("u9", User, "[Message from Zeron chat Brain dump (b).]\n\nNow fix the legend.", Complete));
        two.push(entry("a9", Assistant, "Legend fixed.", Complete));
        let others = vec!["Check device names.".to_owned(), "Now fix the legend.".to_owned()];
        assert_eq!(latest_reply_until(&two, "Check device names.", &others).as_deref(), Some("All done."));
        assert_eq!(latest_reply_until(&two, "Now fix the legend.", &others).as_deref(), Some("Legend fixed."));
        assert_eq!(latest_reply(&two, "Check device names.").as_deref(), Some("Legend fixed."), "unscoped: the newest");
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
            live: Default::default(),
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
            origin: Default::default(),
            turn_id: None,
            updated_at: None,
            escalated_at: None,
            answered_via: None,
            images: Vec::new(),
            recommendation: None,
            links: Vec::new(),
            context: None,
            deferred_until: None,
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
