//! Turn-end asks: a thread that orchestration launched (a crew thread, a
//! thread the Brain dump routed or started, a subagent of either, an
//! orchestrator) and that ended its turn on a plain-text question for the
//! person is idle, not awaiting input; its question would live only in its
//! transcript. Here it becomes an ordinary [`Ask`] under Needs you.
//!
//! - **Create.** At each turn end of such a thread, its final message is read
//!   with [`zeron_proto::turn_question::detect`]; a borderline one goes to
//!   the cheapest model of the chat's harness (the one-shot title path),
//!   and when that model can't be reached the ask is raised anyway (better
//!   one ask too many than one lost). A turn that called `ask` itself
//!   raises nothing more.
//! - **One per turn.** Its id is made from the chat and the asking
//!   message's id, so seeing the same turn again finds it. A thread that
//!   asks the same question again on a later turn updates its ask (or
//!   reopens it, when a reply didn't settle it) instead of adding one.
//! - **Crew.** A crew thread's ask also goes to its orchestrator as a
//!   `[Crew ask]` turn: it may answer with `answer_ask` when its goal, rules
//!   and the person's earlier decisions settle it, or leave it. The person
//!   sees the ask all along.
//! - **Close.** Answered from Needs you (`answerAsk`, delivered to the
//!   thread); any message reaching the thread after the asking turn (the
//!   person in its chat, the Brain dump's or an orchestrator's relay): that
//!   reply is its answer; or the thread acting on it: a later turn, with no
//!   reply between, that says it did one of the options, naming what is
//!   distinctive in it ([`zeron_proto::turn_question::acted_on`]). Yes/no
//!   asks never close by action, only by an answer.
//! - **Escalate.** An open ask left [`STALE_AFTER`] is made urgent once,
//!   shown again as a banner on the device hosting its thread, and leads
//!   Needs you.

use std::sync::Arc;

use chrono::{DateTime, Duration as TimeDelta, Utc};
use zeron_doc::{MessagePart, MessageRole, MessageStatus, SessionMessageEntry};
use zeron_proto::orchestration::{
    AnswerVia, Ask, AskKind, AskOption, AskOrigin, AskState, CommKind, OrchestrationState, OrchestratorKind,
};
use zeron_proto::turn_question::{self, Found, FoundKind, Verdict};
use zeron_proto::{Session, SessionStatus};

use super::Orchestration;
use super::comms::YOU;
use crate::rpc::EngineRpc;

/// How long an ask waits before it is raised.
pub(crate) const STALE_AFTER: TimeDelta = TimeDelta::minutes(30);
/// How often stale asks are looked for.
pub(crate) const ESCALATE_EVERY: std::time::Duration = std::time::Duration::from_secs(60);
/// How long after it closed an ask may be reopened by the same question.
const REOPEN_WITHIN: TimeDelta = TimeDelta::hours(24);
/// The most of a reply an ask keeps as its answer.
const ANSWER_MAX: usize = 2000;

/// How a chat came from orchestration, for its asks.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Launch {
    /// The orchestrator it works for (its own id for an orchestrator).
    pub orchestrator_id: Option<String>,
    pub item_id: Option<String>,
    /// The orchestrator chat that gets a turn with its asks.
    pub hand_to: Option<String>,
}

/// Whether `chat` is one orchestration launched, and for whom. From the
/// rows: an orchestrator (not the Brain dump: that is the person's own
/// chat), a crew thread, a route's target, a chat an orchestrator or the
/// Brain dump started (`started_by`), or a subagent of any of those.
pub(crate) fn launch_of(
    state: &OrchestrationState,
    chat: &str,
    started_by: Option<&str>,
    parent: Option<&str>,
) -> Option<Launch> {
    let direct = |chat: &str| -> Option<Launch> {
        if let Some(o) = state.orchestrators.iter().find(|o| o.id == chat) {
            return match o.kind {
                OrchestratorKind::Standard => Some(Launch { orchestrator_id: Some(o.id.clone()), ..Default::default() }),
                _ => None,
            };
        }
        if let Some(m) = state.crew.iter().find(|m| m.chat_id == chat && !m.released) {
            return Some(Launch {
                orchestrator_id: Some(m.orchestrator_id.clone()),
                item_id: m.item_id.clone(),
                hand_to: Some(m.orchestrator_id.clone()),
            });
        }
        // The route delivered to it last: what its turns since are about,
        // and the card a question it ends on waits on (none for a question
        // route).
        let routed = state
            .dumps
            .iter()
            .flat_map(|d| d.routes.iter().map(move |r| (r.delivered_at.unwrap_or(d.created_at), r)))
            .filter(|(_, r)| super::watchers::target_chat(&r.target) == Some(chat))
            .max_by_key(|(at, _)| *at)
            .map(|(_, r)| r);
        routed.map(|r| Launch { item_id: r.item_id.clone(), ..Default::default() })
    };
    if state.orchestrators.iter().any(|o| o.id == chat) {
        return direct(chat);
    }
    if let Some(launch) = direct(chat) {
        return Some(launch);
    }
    let starter = |by: &str| -> Option<Launch> {
        let o = state.orchestrators.iter().find(|o| o.id == by)?;
        Some(match o.kind {
            OrchestratorKind::Standard => {
                Launch { orchestrator_id: Some(o.id.clone()), item_id: None, hand_to: Some(o.id.clone()) }
            }
            _ => Launch::default(),
        })
    };
    if let Some(launch) = started_by.and_then(starter) {
        return Some(launch);
    }
    // A subagent: of an orchestrator or the Brain dump, or of a thread
    // they launched (its question is its parent's to see; the orchestrator
    // isn't handed it).
    let parent = parent?;
    if let Some(launch) = starter(parent) {
        return Some(launch);
    }
    direct(parent).map(|l| Launch { hand_to: None, ..l })
}

/// The ask id of `chat`'s turn that ended on message `turn`.
pub(crate) fn ask_id(chat: &str, turn: &str) -> String {
    format!("turn-{chat}-{turn}")
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

/// The turn's final message: the last assistant entry with words after
/// the last user entry (its id and words), once nothing streams.
pub(crate) fn final_message(entries: &[SessionMessageEntry]) -> Option<(String, String)> {
    let start = entries.iter().rposition(|e| e.role == MessageRole::User).map_or(0, |i| i + 1);
    let turn = &entries[start..];
    if turn.iter().any(|e| e.status == Some(MessageStatus::Streaming)) {
        return None;
    }
    turn.iter()
        .rev()
        .filter(|e| e.role == MessageRole::Assistant)
        .map(|e| (e.id.clone(), texts(e)))
        .find(|(_, t)| !t.trim().is_empty())
}

/// When the turn that ended started (its user message), epoch millis.
fn turn_started(entries: &[SessionMessageEntry]) -> i64 {
    entries.iter().rev().find(|e| e.role == MessageRole::User).map_or(0, |e| e.created_at)
}

/// What raising `found` for `chat`'s turn `turn` does to `asks`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Plan {
    /// This turn already has its ask (or nothing to do).
    Nothing,
    Create(Ask),
    /// The same question asked again: the existing ask, updated (reopened
    /// when a reply or an action had closed it).
    Update(Ask),
}

fn options(found: &Found) -> Vec<AskOption> {
    found.options.iter().map(|o| AskOption { label: o.clone(), detail: String::new() }).collect()
}

fn labels(ask: &Ask) -> Vec<String> {
    ask.options.iter().map(|o| o.label.clone()).collect()
}

/// `turn`: the message that asked, when a turn's end raised it; `None`
/// for one a message (to the Brain dump, or a route's outcome) raised,
/// which gets a fresh id and is only ever deduplicated by its question.
pub(crate) fn plan(
    asks: &[Ask],
    chat: &str,
    turn: Option<&str>,
    found: &Found,
    launch: &Launch,
    now: DateTime<Utc>,
) -> Plan {
    let id = match turn {
        Some(turn) => ask_id(chat, turn),
        None => uuid::Uuid::new_v4().simple().to_string(),
    };
    if let Some(turn) = turn
        && asks.iter().any(|a| a.id == id || a.from_chat == chat && a.turn_id.as_deref() == Some(turn))
    {
        return Plan::Nothing;
    }
    let same = |a: &&Ask| {
        a.from_chat == chat
            && a.origin == AskOrigin::TurnEnd
            && turn_question::same_question(&a.question, &labels(a), &found.question, &found.options)
    };
    // Asked again: the open one, else one a reply or an action closed lately
    // (the person's own answer from Needs you stands).
    let again = asks.iter().filter(same).find(|a| a.state == AskState::Open).or_else(|| {
        asks.iter().filter(same).filter(|a| {
            a.state == AskState::Answered
                && matches!(a.answered_via, Some(AnswerVia::Thread | AnswerVia::Action))
                && a.answered_at.is_some_and(|at| now - at <= REOPEN_WITHIN)
        })
        .max_by_key(|a| a.answered_at)
    });
    if let Some(old) = again {
        let mut ask = old.clone();
        ask.question = found.question.clone();
        if !found.options.is_empty() {
            ask.options = options(found);
        }
        if turn.is_none() && ask.state == AskState::Open && ask.question == found.question {
            // The same words again from a message: nothing new.
            return Plan::Nothing;
        }
        if let Some(turn) = turn {
            ask.turn_id = Some(turn.to_owned());
        }
        // Asked again after a newer delivery: that delivery's card waits.
        if launch.item_id.is_some() {
            ask.item_id = launch.item_id.clone();
        }
        ask.updated_at = Some(now);
        if ask.state != AskState::Open {
            ask.state = AskState::Open;
            ask.answer = None;
            ask.answered_at = None;
            ask.answered_via = None;
        }
        return Plan::Update(ask);
    }
    Plan::Create(Ask {
        id,
        from_chat: chat.to_owned(),
        orchestrator_id: launch.orchestrator_id.clone(),
        item_id: launch.item_id.clone(),
        kind: match found.kind {
            FoundKind::Decision => AskKind::Decision,
            FoundKind::Approval => AskKind::Approval,
            FoundKind::Question => AskKind::Question,
        },
        question: found.question.clone(),
        options: options(found),
        urgent: false,
        state: AskState::Open,
        answer: None,
        created_at: now,
        answered_at: None,
        dump_id: None,
        route_id: None,
        quiet: false,
        origin: AskOrigin::TurnEnd,
        turn_id: turn.map(str::to_owned),
        updated_at: None,
        escalated_at: None,
        answered_via: None,
    })
}

/// What answered a decision `chat`'s transcript asks after the message
/// carrying `sent` (a route's delivery; the whole transcript without it):
/// the message that reached the thread after the last message there that
/// waits on the person ([`turn_question::detect`]), its words. `None`: no
/// such message (or no message asking) yet. The same rule as a turn-end
/// ask's close by a reply ([`replied`]).
pub(crate) fn answered_after_asking(entries: &[SessionMessageEntry], sent: Option<&str>) -> Option<String> {
    let from = sent
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(|s| entries.iter().rposition(|e| e.role == MessageRole::User && texts(e).contains(s)))
        .map_or(0, |i| i + 1);
    let asked = entries[from..]
        .iter()
        .rposition(|e| e.role == MessageRole::Assistant && matches!(turn_question::detect(&texts(e)), Verdict::Ask(_)))
        .map(|i| from + i)?;
    let reply = entries[asked + 1..].iter().find(|e| e.role == MessageRole::User)?;
    Some(zeron_proto::orchestration::prompt_text::one_line(&texts(reply), ANSWER_MAX))
}

/// `chat`'s open turn-end asks that a message after their turn answers:
/// each with that message's words.
pub(crate) fn replied(asks: &[Ask], chat: &str, entries: &[SessionMessageEntry]) -> Vec<(String, String)> {
    open_turn_asks(asks, chat)
        .filter_map(|a| {
            let at = entries.iter().position(|e| Some(e.id.as_str()) == a.turn_id.as_deref())?;
            let reply = entries[at + 1..].iter().find(|e| e.role == MessageRole::User)?;
            Some((a.id.clone(), zeron_proto::orchestration::prompt_text::one_line(&texts(reply), ANSWER_MAX)))
        })
        .collect()
}

/// `chat`'s open turn-end asks a later turn acted on, no message to it
/// between: each with the option it did.
pub(crate) fn acted(asks: &[Ask], chat: &str, entries: &[SessionMessageEntry]) -> Vec<(String, String)> {
    open_turn_asks(asks, chat)
        .filter_map(|a| {
            let at = entries.iter().position(|e| Some(e.id.as_str()) == a.turn_id.as_deref())?;
            let after = &entries[at + 1..];
            if after.iter().any(|e| e.role == MessageRole::User) {
                return None;
            }
            let later: Vec<String> = after.iter().filter(|e| e.role == MessageRole::Assistant).map(texts).collect();
            let i = turn_question::acted_on(&labels(a), &later.join("\n\n"))?;
            Some((a.id.clone(), a.options[i].label.clone()))
        })
        .collect()
}

fn open_turn_asks<'a>(asks: &'a [Ask], chat: &'a str) -> impl Iterator<Item = &'a Ask> + 'a {
    asks.iter().filter(move |a| a.from_chat == chat && a.origin == AskOrigin::TurnEnd && a.state == AskState::Open)
}

/// The open asks gone stale and not raised yet, from chats `hosted` here.
pub(crate) fn stale(asks: &[Ask], now: DateTime<Utc>, hosted: &dyn Fn(&str) -> bool) -> Vec<String> {
    asks.iter()
        .filter(|a| a.state == AskState::Open && a.escalated_at.is_none())
        .filter(|a| now - a.updated_at.unwrap_or(a.created_at) >= STALE_AFTER)
        .filter(|a| hosted(&a.from_chat))
        .map(|a| a.id.clone())
        .collect()
}

/// Close ask `id` (still open) as answered `via`, with `answer`.
pub(crate) fn close(orch: &Orchestration, id: &str, via: AnswerVia, answer: &str, now: DateTime<Utc>) -> Option<Ask> {
    orch.workspace().write_orchestration(|doc| {
        let mut ask = doc.ask(id)?;
        if ask.state != AskState::Open {
            return None;
        }
        ask.state = AskState::Answered;
        ask.answer = Some(answer.to_owned());
        ask.answered_at = Some(now);
        ask.answered_via = Some(via);
        doc.upsert_ask(&ask);
        Some(ask)
    })
}

/// Close what `chat`'s transcript settles: a reply after an ask's turn, or
/// a later turn that did one of its options.
pub(crate) fn settle(orch: &Orchestration, rpc: &EngineRpc, chat: &str, entries: &[SessionMessageEntry]) -> usize {
    let asks = orch.state().asks;
    let now = Utc::now();
    let mut closed = 0;
    for (id, reply) in replied(&asks, chat, entries) {
        if close(orch, &id, AnswerVia::Thread, &reply, now).is_some() {
            orch.comms().say(CommKind::Answer, YOU, chat, &reply);
            closed += 1;
        }
    }
    for (id, option) in acted(&asks, chat, entries) {
        if close(orch, &id, AnswerVia::Action, &option, now).is_some() {
            orch.comms().say(CommKind::Report, chat, YOU, &option);
            closed += 1;
        }
    }
    if closed > 0 {
        rpc.sync_cards_of(chat);
    }
    closed
}

/// Raise `found` for `chat`'s turn `turn`: create, update or nothing (see
/// [`plan`]), atomically against what's stored. A new crew ask also goes
/// to its orchestrator.
#[allow(clippy::too_many_arguments)]
pub(crate) fn raise(
    orch: &Orchestration,
    rpc: &EngineRpc,
    chat: &str,
    turn: Option<&str>,
    found: &Found,
    launch: &Launch,
    route: Option<(&str, &str)>,
    now: DateTime<Utc>,
) -> Plan {
    let plan = orch.workspace().write_orchestration(|doc| {
        let mut plan = plan(&doc.read_asks(), chat, turn, found, launch, now);
        if let Plan::Create(ask) | Plan::Update(ask) = &mut plan {
            // A route's outcome: the ask is linked to that route.
            if let Some((dump, route)) = route
                && ask.dump_id.is_none()
            {
                ask.dump_id = Some(dump.to_owned());
                ask.route_id = Some(route.to_owned());
                // Its route's card, when it has one, is the one that waits.
                if let Some(item) = doc.dump(dump).and_then(|d| d.routes.into_iter().find(|r| r.id == route)).and_then(|r| r.item_id) {
                    ask.item_id = Some(item);
                }
            }
            doc.upsert_ask(ask);
        }
        plan
    });
    match &plan {
        Plan::Create(ask) => {
            rpc.sync_cards_of(chat);
            orch.comms().say(CommKind::Ask, chat, YOU, &ask.question);
            if let Some(to) = &launch.hand_to {
                let title = rpc
                    .workspace_host()
                    .chat(chat)
                    .ok()
                    .flatten()
                    .and_then(|c| c.title)
                    .unwrap_or_else(|| chat.chars().take(8).collect());
                let text = zeron_proto::orchestration::prompt_text::crew_ask(&title, &ask.id, &ask.question, &labels(ask));
                if let Err(error) = super::ops::deliver(rpc, to, &text, None, super::ops::When::Next) {
                    tracing::warn!(%error, "turn-end ask: its orchestrator wasn't told");
                }
            }
        }
        Plan::Update(_) => rpc.sync_cards_of(chat),
        Plan::Nothing => {}
    }
    plan
}

/// What the end of a turn asks, settled: the heuristics, then (borderline)
/// the model. `ask` runs the model on a prompt; `None` from it (no model)
/// keeps the heuristic's find.
pub(crate) async fn settle_verdict<F, Fut>(text: &str, ask: F) -> Option<Found>
where
    F: FnOnce(String) -> Fut,
    Fut: std::future::Future<Output = Option<String>>,
{
    match turn_question::detect(text) {
        Verdict::None => None,
        Verdict::Ask(found) => Some(found),
        Verdict::Borderline(found) => match ask(turn_question::classify_prompt(text)).await {
            Some(raw) => match turn_question::parse_model(&raw, &found) {
                Some(said) => said,
                None => Some(found),
            },
            None => Some(found),
        },
    }
}

/// How `chat` came from orchestration, from its rows ([`launch_of`]).
pub(crate) fn launch_of_chat(orch: &Orchestration, rpc: &EngineRpc, chat: &str) -> Option<Launch> {
    let row = rpc.workspace_host().chat(chat).ok().flatten();
    let started_by = row.as_ref().and_then(|c| c.started_by.as_ref()).map(|s| s.chat_id.clone());
    let parent = row.as_ref().and_then(|c| c.parent_chat_id.clone());
    launch_of(&orch.state(), chat, started_by.as_deref(), parent.as_deref())
}

/// `chat`'s turn ended (`session` now): close what it settled, then raise
/// what it asks.
pub(crate) async fn after_turn(orch: &Orchestration, rpc: &Arc<EngineRpc>, chat: &str, session: &Session) {
    if matches!(session.status, SessionStatus::AwaitingInput | SessionStatus::Errored) {
        return;
    }
    let state = orch.state();
    let row = rpc.workspace_host().chat(chat).ok().flatten();
    let started_by = row.as_ref().and_then(|c| c.started_by.as_ref()).map(|s| s.chat_id.clone());
    let parent = row.as_ref().and_then(|c| c.parent_chat_id.clone());
    let Some(launch) = launch_of(&state, chat, started_by.as_deref(), parent.as_deref()) else { return };
    let Ok(handle) = rpc.doc_host_ref().open(chat) else { return };
    let Ok(entries) = handle.doc().read_entries() else { return };
    settle(orch, rpc, chat, &entries);
    let Some((turn, text)) = final_message(&entries) else { return };
    // It asked through `ask` this turn: that is its ask.
    let since = turn_started(&entries);
    // (Its own: not a chase or a "where should this go?" Zeron raised for
    // it, which are quiet or tied to a dump.)
    if state.asks.iter().any(|a| {
        a.from_chat == chat
            && a.origin == AskOrigin::Tool
            && !a.quiet
            && a.dump_id.is_none()
            && a.state == AskState::Open
            && a.created_at.timestamp_millis() >= since
    }) {
        return;
    }
    let harness = row
        .as_ref()
        .and_then(|c| c.config.as_ref().map(|c| c.harness))
        .unwrap_or_else(|| rpc.doc_host_ref().default_harness());
    let one_shot = orch.progress().one_shot(rpc);
    let Some(found) = settle_verdict(&text, |prompt| one_shot(harness, prompt)).await else { return };
    raise(orch, rpc, chat, Some(&turn), &found, &launch, None, Utc::now());
}

/// A message reached `chat` (its turn started): the turn-end asks it
/// answers close.
pub(crate) fn on_reply(orch: &Orchestration, rpc: &EngineRpc, chat: &str) {
    if !orch.state().asks.iter().any(|a| a.from_chat == chat && a.origin == AskOrigin::TurnEnd && a.state == AskState::Open) {
        return;
    }
    let Ok(handle) = rpc.doc_host_ref().open(chat) else { return };
    let Ok(entries) = handle.doc().read_entries() else { return };
    settle(orch, rpc, chat, &entries);
}

/// Raise the stale asks of chats hosted here: urgent, a banner again.
pub(crate) fn escalate(orch: &Orchestration, rpc: &EngineRpc, now: DateTime<Utc>) -> Vec<String> {
    let hosted: std::collections::HashSet<String> =
        rpc.sessions_ref().watch_sessions().borrow().iter().map(|s| s.chat_id.clone()).collect();
    escalate_with(orch, rpc, now, &|c| hosted.contains(c))
}

/// [`escalate`] for the chats `hosted` says are here.
pub(crate) fn escalate_with(
    orch: &Orchestration,
    rpc: &EngineRpc,
    now: DateTime<Utc>,
    hosted: &dyn Fn(&str) -> bool,
) -> Vec<String> {
    let due = stale(&orch.state().asks, now, hosted);
    let mut raised = Vec::new();
    for id in due {
        let ask = orch.workspace().write_orchestration(|doc| {
            let mut ask = doc.ask(&id)?;
            if ask.state != AskState::Open || ask.escalated_at.is_some() {
                return None;
            }
            ask.urgent = true;
            ask.escalated_at = Some(now);
            doc.upsert_ask(&ask);
            Some(ask)
        });
        let Some(ask) = ask else { continue };
        let title = rpc
            .workspace_host()
            .chat(&ask.from_chat)
            .ok()
            .flatten()
            .and_then(|c| c.title)
            .unwrap_or_else(|| "A thread".into());
        rpc.notify_person(&format!("Still waiting on you: {title}"), &super::excerpt(&ask.question, 200), Some(&ask.from_chat));
        orch.comms().say(CommKind::Ask, &ask.from_chat, YOU, &ask.question);
        rpc.sync_cards_of(&ask.from_chat);
        raised.push(ask.id);
    }
    raised
}

/// Look for stale asks every [`ESCALATE_EVERY`].
pub(crate) fn start(orch: Orchestration) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(ESCALATE_EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            if let Some(rpc) = orch.rpc() {
                escalate(&orch, &rpc, Utc::now());
            }
        }
    });
}
