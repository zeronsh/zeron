//! Agent messages to the Brain dump or an orchestrator, wherever they are
//! caught: by the sender's engine (its RPC path, the fast path) or by the
//! receiving host when it is about to run one (any sender, any version).
//!
//! - From a thread a route went to, to the Brain dump: that route's outcome.
//! - From a crew thread to its orchestrator: a report.
//! - Anything else to the Brain dump: an update the person reads in Comms,
//!   never a turn of the Brain dump's own (nobody reads its transcript).
//! - A message or outcome that ends on a question for the person raises an
//!   ask from that thread: answering it goes back to the thread.
//!
//! And question routes nobody answered are chased ([`chase_unanswered`]).

use chrono::{Duration as TimeDelta, Utc};
use zeron_proto::orchestration::{
    Ask, AskKind, AskOption, AskState, CommKind, Dump, Route, RouteIntent, RouteState, prompt_text::one_line,
};
use zeron_rpc::RpcError;

use zeron_proto::turn_question::{Found, Verdict};

use super::{ChatRole, Orchestration};
use crate::rpc::EngineRpc;

/// How long a question route may go unanswered before the person is told.
pub(crate) const UNANSWERED_AFTER: TimeDelta = TimeDelta::minutes(20);
/// The chase ask's options.
pub(crate) const ASK_AGAIN: &str = "Ask again";
pub(crate) const DISMISS: &str = "Dismiss";

/// The question `text` ends on for the person, if it does
/// ([`zeron_proto::turn_question::detect`]: a plain one or a borderline one;
/// a message is read once, with no model run).
pub(crate) fn ends_on_question(text: &str) -> Option<Found> {
    match zeron_proto::turn_question::detect(text) {
        Verdict::Ask(found) | Verdict::Borderline(found) => Some(found),
        Verdict::None => None,
    }
}

/// The open route of the newest dump that targets `chat`: `(dump, route,
/// intent, what it was sent)`.
pub(crate) fn open_route_of(dumps: &[Dump], chat: &str) -> Option<(String, String, RouteIntent, String)> {
    open_route_where(dumps, chat, |_| true)
}

/// [`open_route_of`] among the routes `keep` keeps.
fn open_route_where(
    dumps: &[Dump],
    chat: &str,
    keep: impl Fn(&zeron_proto::orchestration::Route) -> bool,
) -> Option<(String, String, RouteIntent, String)> {
    let mut dumps: Vec<_> = dumps.iter().collect();
    dumps.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    dumps.into_iter().find_map(|d| {
        d.routes
            .iter()
            .filter(|r| matches!(r.state, RouteState::Delivered | RouteState::Answered))
            .find(|r| super::watchers::target_chat(&r.target) == Some(chat) && keep(r))
            .map(|r| {
                let sent = if r.message.trim().is_empty() { r.excerpt.clone() } else { r.message.clone() };
                (d.id.clone(), r.id.clone(), r.intent, sent)
            })
    })
}

/// The open route a message from `sender` answers: one to it, else one to
/// its orchestrator whose card its crew row is on (the work that route
/// became). Another crew thread's message to the Brain dump is an update.
pub(crate) fn route_answered_by(
    dumps: &[Dump],
    sender: &str,
    crew: Option<&zeron_proto::orchestration::CrewMember>,
) -> Option<(String, String, RouteIntent, String)> {
    open_route_of(dumps, sender).or_else(|| {
        let crew = crew?;
        let card = crew.item_id.as_deref()?;
        open_route_where(dumps, &crew.orchestrator_id, |r| r.item_id.as_deref() == Some(card))
    })
}

impl EngineRpc {
    /// A message from `sender` to the Brain dump: the outcome of the open
    /// route that went to it (or to the orchestrator whose crew it is in),
    /// else an update in Comms. Either way an ask when it ends on a question
    /// for the person. Never a turn of the Brain dump's. The route it
    /// answers, or the update's id.
    pub(crate) fn file_with_brain(&self, sender: &str, body: &str) -> String {
        let orchestration = self.orchestration().clone();
        let body = body.trim().to_owned();
        let dumps = orchestration.state().dumps;
        let routed = route_answered_by(&dumps, sender, orchestration.crew_of(sender).as_ref());
        let Some((dump, route, intent, asked)) = routed else {
            return self.brain_update(sender, &body);
        };
        let task = (intent != RouteIntent::Question).then_some(RouteState::Answered);
        let sender = sender.to_owned();
        let route_id = route.clone();
        match orchestration.rpc() {
            Some(rpc) => {
                tokio::spawn(async move {
                    let filed = super::watchers::record_outcome(
                        &orchestration, &rpc, &sender, &dump, &route, &asked, body.clone(), task,
                    )
                    .await;
                    // Its route answered already: still the person's to read.
                    if !filed {
                        rpc.brain_update(&sender, &body);
                    }
                });
            }
            None => {
                tracing::warn!("brain dump: no services to file an outcome with");
                return self.brain_update(&sender, &body);
            }
        }
        route_id
    }

    /// A message to the Brain dump no open route asked for: an update from
    /// `sender` to the person in Comms, and an ask when it ends on a
    /// question. The update's id.
    pub(crate) fn brain_update(&self, sender: &str, body: &str) -> String {
        let orchestration = self.orchestration();
        // (Read first: the update and its ask land together.)
        let found = ends_on_question(body);
        orchestration.comms().say(CommKind::Message, sender, super::comms::YOU, &one_line(body, 140));
        if let Some(found) = found {
            self.ask_for(sender, &found, None);
        }
        uuid::Uuid::new_v4().simple().to_string()
    }

    /// Raise an ask from `chat` (answering it goes back to it) for `found`,
    /// the way a turn's end does ([`super::turn_asks::raise`]): one per
    /// question, an open one asked again updated, its crew's orchestrator
    /// told. Linked to a dump's route when it is that route's outcome. Its id.
    pub(crate) fn ask_for(&self, chat: &str, found: &Found, route: Option<(&str, &str)>) -> Option<String> {
        let orchestration = self.orchestration();
        let launch = super::turn_asks::launch_of_chat(orchestration, self, chat).unwrap_or_else(|| super::turn_asks::Launch {
            orchestrator_id: orchestration.crew_of(chat).map(|m| m.orchestrator_id),
            ..Default::default()
        });
        match super::turn_asks::raise(orchestration, self, chat, None, found, &launch, route, Utc::now()) {
            super::turn_asks::Plan::Create(ask) | super::turn_asks::Plan::Update(ask) => Some(ask.id),
            super::turn_asks::Plan::Nothing => None,
        }
    }

    /// An answer to an ask tied to a route: "Where should this go?" moves
    /// the route; "hasn't answered" asks again or lets it be. `false` for any
    /// other ask (its answer goes back to its chat as usual).
    pub(crate) async fn answer_route_ask(&self, ask: &Ask, answer: &str) -> Result<bool, RpcError> {
        let (Some(dump_id), Some(route_id)) = (ask.dump_id.as_deref(), ask.route_id.as_deref()) else {
            return Ok(false);
        };
        if is_chase(ask) {
            if answer.trim().eq_ignore_ascii_case(ASK_AGAIN) {
                self.ask_again(dump_id, route_id)?;
            }
            return Ok(true);
        }
        let route = self
            .orchestration()
            .workspace()
            .read_orchestration_with(|doc| doc.dump(dump_id))
            .and_then(|d| d.routes.into_iter().find(|r| r.id == route_id));
        let unsure = route.is_some_and(|r| {
            r.state == RouteState::Pending && r.target == zeron_proto::orchestration::RouteTarget::Unsure
        });
        if unsure {
            self.answer_where(ask, answer).await?;
            return Ok(true);
        }
        Ok(false)
    }

    /// Send a question route's message to its target again.
    fn ask_again(&self, dump_id: &str, route_id: &str) -> Result<(), RpcError> {
        let orchestration = self.orchestration();
        let Some(dump) = orchestration.workspace().read_orchestration_with(|doc| doc.dump(dump_id)) else {
            return Ok(());
        };
        let Some(route) = dump.routes.iter().find(|r| r.id == route_id) else { return Ok(()) };
        let Some(target) = super::watchers::target_chat(&route.target) else { return Ok(()) };
        let message = if route.message.trim().is_empty() { route.excerpt.clone() } else { route.message.clone() };
        let brain = self.brain_of(&dump);
        self.deliver_forwarding(target, &message, brain.as_deref(), &route.attachments)?;
        orchestration.workspace().write_orchestration(|doc| {
            let Some(mut dump) = doc.dump(dump_id) else { return };
            if let Some(route) = dump.routes.iter_mut().find(|r| r.id == route_id) {
                route.delivered_at = Some(Utc::now());
            }
            dump.updated_at = Utc::now();
            doc.upsert_dump(&dump);
        });
        Ok(())
    }
}

/// Whether `ask` is a chase of an unanswered route (its two options).
fn is_chase(ask: &Ask) -> bool {
    ask.options.len() == 2 && ask.options[0].label == ASK_AGAIN && ask.options[1].label == DISMISS
}

/// The open chases of route `route_id`, dismissed: it was answered.
pub(crate) fn settle_chases(orchestration: &Orchestration, route_id: &str) {
    let open: Vec<Ask> = orchestration
        .state()
        .asks
        .into_iter()
        .filter(|a| a.state == AskState::Open && a.route_id.as_deref() == Some(route_id) && is_chase(a))
        .collect();
    if open.is_empty() {
        return;
    }
    let chats: Vec<String> = open.iter().map(|a| a.from_chat.clone()).collect();
    orchestration.workspace().write_orchestration(|doc| {
        for mut ask in open {
            ask.state = AskState::Dismissed;
            ask.answered_at = Some(Utc::now());
            doc.upsert_ask(&ask);
        }
    });
    // Its thread's cards follow (as any ask's close does).
    if let Some(rpc) = orchestration.rpc() {
        for chat in chats {
            rpc.sync_cards_of(&chat);
        }
    }
}

/// Catch an agent message the host is about to run in `receiver`: `true`
/// when it was filed instead (an outcome, a report or an update).
pub(crate) fn consume(orchestration: &Orchestration, receiver: &str, prompt: &str) -> bool {
    let Some((sender, body)) = zeron_proto::agent_messages::agent_message_sender(prompt) else {
        return false;
    };
    if body.trim().is_empty() || sender.chat_id == receiver {
        return false;
    }
    let Some(rpc) = orchestration.rpc() else { return false };
    match orchestration.role_of(receiver) {
        ChatRole::BrainDump { .. } => {
            rpc.file_with_brain(&sender.chat_id, body);
            true
        }
        ChatRole::Orchestrator { id } => {
            if orchestration.crew_of(&sender.chat_id).is_none_or(|m| m.orchestrator_id != id) {
                return false;
            }
            let params = super::report::message_report(body);
            let sender = sender.chat_id.clone();
            tokio::spawn(async move {
                if let Err(error) = super::report::file_report(&rpc, &sender, params).await {
                    tracing::warn!(%error, "a crew thread's message wasn't filed as a report");
                }
            });
            true
        }
        _ => false,
    }
}

/// Every question route of a dump here still waiting on its answer after
/// [`UNANSWERED_AFTER`]: the person is asked (quietly) whether to ask again.
/// The asks raised.
pub(crate) fn chase_unanswered(orchestration: &Orchestration, rpc: &EngineRpc, now: chrono::DateTime<Utc>) -> Vec<String> {
    let here = orchestration.device_id().to_owned();
    // Scanned without the registry lock; each chase is decided again under
    // it before it's written.
    let due: Vec<Chase> = orchestration.scan(|state| {
        let mut due = Vec::new();
        for dump in state.dumps.iter().filter(|d| d.device_id == here) {
            for route in &dump.routes {
                let Some(since) = chase_due(dump, route, now) else { continue };
                if chased(&state.asks, &route.id, since) {
                    continue;
                }
                let Some(target) = super::watchers::target_chat(&route.target) else { continue };
                due.push(Chase {
                    dump_id: dump.id.clone(),
                    route_id: route.id.clone(),
                    target: target.to_owned(),
                    item_id: route.item_id.clone(),
                    excerpt: route.excerpt.clone(),
                });
            }
        }
        due
    });
    let mut raised = Vec::new();
    for chase in due {
        let title = rpc
            .workspace_host()
            .chat(&chase.target)
            .ok()
            .flatten()
            .and_then(|c| c.title)
            .unwrap_or_else(|| "The thread".into());
        let ask = Ask {
            id: uuid::Uuid::new_v4().simple().to_string(),
            from_chat: chase.target.clone(),
            orchestrator_id: None,
            item_id: chase.item_id.clone(),
            kind: AskKind::Decision,
            question: format!("{} hasn't answered: {}", one_line(&title, 60), one_line(&chase.excerpt, 200)),
            options: vec![
                AskOption::new(ASK_AGAIN),
                AskOption::new(DISMISS),
            ],
            urgent: false,
            state: AskState::Open,
            answer: None,
            created_at: now,
            answered_at: None,
            dump_id: Some(chase.dump_id.clone()),
            route_id: Some(chase.route_id.clone()),
            quiet: true,
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
        };
        let written = orchestration.workspace().write_orchestration(|doc| {
            let Some(dump) = doc.dump(&chase.dump_id) else { return false };
            let Some(route) = dump.routes.iter().find(|r| r.id == chase.route_id) else { return false };
            let Some(since) = chase_due(&dump, route, now) else { return false };
            if chased(&doc.read_asks(), &chase.route_id, since) {
                return false;
            }
            doc.upsert_ask(&ask);
            true
        });
        if written {
            raised.push(ask.id);
        }
    }
    raised
}

/// A question route [`chase_unanswered`] may chase.
struct Chase {
    dump_id: String,
    route_id: String,
    target: String,
    item_id: Option<String>,
    excerpt: String,
}

/// When `route` was delivered, if it's a question still waiting on its
/// answer after [`UNANSWERED_AFTER`].
fn chase_due(dump: &Dump, route: &Route, now: chrono::DateTime<Utc>) -> Option<chrono::DateTime<Utc>> {
    if route.intent != RouteIntent::Question || route.state != RouteState::Delivered || route.answer.is_some() {
        return None;
    }
    let since = route.delivered_at.unwrap_or(dump.created_at);
    (now - since >= UNANSWERED_AFTER).then_some(since)
}

/// Whether route `route_id` was chased already: one at a time, and not
/// again for the same delivery (`since`).
fn chased(asks: &[Ask], route_id: &str, since: chrono::DateTime<Utc>) -> bool {
    asks.iter().any(|a| {
        a.route_id.as_deref() == Some(route_id) && is_chase(a) && (a.state == AskState::Open || a.created_at >= since)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_crew_thread_answers_only_the_route_its_card_came_from() {
        use zeron_proto::orchestration::{CrewMember, Route};
        let route = |id: &str, to: &str, item: Option<&str>| -> Route {
            serde_json::from_value(serde_json::json!({
                "id": id, "excerpt": "x", "target": { "kind": "orchestrator", "id": to },
                "intent": "task", "state": "delivered", "span": [0, 1], "message": "do x",
                "itemId": item,
            }))
            .unwrap()
        };
        let mut dump: Dump = serde_json::from_value(serde_json::json!({
            "id": "d", "text": "x", "state": "routed", "createdAt": "2026-11-13T00:00:00Z",
            "updatedAt": "2026-11-13T00:00:00Z", "deviceId": "here"
        }))
        .unwrap();
        dump.routes = vec![route("r1", "orch", Some("card-1")), route("r2", "orch", None)];
        let dumps = [dump];
        let member = |item: Option<&str>| -> CrewMember {
            serde_json::from_value(serde_json::json!({
                "chatId": "crew", "orchestratorId": "orch", "itemId": item, "joinedAt": "2026-11-13T00:00:00Z"
            }))
            .unwrap()
        };
        let on_card = member(Some("card-1"));
        let other = member(Some("card-9"));
        let loose = member(None);
        assert_eq!(route_answered_by(&dumps, "crew", Some(&on_card)).map(|r| r.1).as_deref(), Some("r1"));
        assert_eq!(route_answered_by(&dumps, "crew", Some(&other)), None, "another card's thread");
        assert_eq!(route_answered_by(&dumps, "crew", Some(&loose)), None, "a thread on no card");
        assert_eq!(route_answered_by(&dumps, "crew", None), None);
        assert_eq!(route_answered_by(&dumps, "orch", None).map(|r| r.1).as_deref(), Some("r1"), "its own target");
    }

    #[test]
    fn a_closing_question_is_found_and_a_statement_is_not() {
        let q = |text: &str| ends_on_question(text).map(|f| f.question);
        let reply = "Inbox and Next are never written by anything.\n\nThey're dead columns. Wire them up or remove them? Say which.";
        let found = ends_on_question(reply).unwrap();
        assert!(found.question.contains("Wire them up or remove them?"), "{}", found.question);
        assert_eq!(found.options, ["Wire them up", "Remove them"]);
        assert_eq!(q("All checks pass. Names get -1, -2."), None);
        assert_eq!(q("Should I also rename the old ones?").as_deref(), Some("Should I also rename the old ones?"));
        assert!(q("Done. Let me know if you want the tests too").is_some_and(|q| q.contains("Let me know if you want the tests too")));
        // A question in the middle, answered after, isn't the ending.
        assert_eq!(q("Why did it fail? The cache was stale.\n\nFixed it."), None);
        assert_eq!(q(""), None);
    }
}
