//! Routed tasks land on the board. Every task the Brain dump delivers to a
//! thread (or to an orchestrator) gets one card, linked to the thread and
//! the route, and the card follows the route: Now while the work goes on
//! (outcomes so far included), Review once it finished leaving the person
//! something (an open PR, a decision, a result to check), Done when it
//! finished with nothing for them, Blocked when it failed. Its outcome is
//! the route's answer, after "Review: …" in Review. A card moved by hand
//! keeps its column until its route's state changes.

use zeron_proto::orchestration::{
    Dump, Priority, Route, RouteIntent, RouteRef, RouteState, RouteTarget, WorkItemPatch, WorkState,
    prompt_text::one_line,
};
use zeron_rpc::RpcError;

use crate::rpc::EngineRpc;

/// The most a card's title keeps.
pub(crate) const TITLE_MAX: usize = 60;

/// A short title from what a route sent: its first clause, without any
/// header or bracketed marker Zeron put in front.
pub(crate) fn card_title(message: &str) -> String {
    let mut text = message.trim();
    // `[For your crew thread …] …` and the like.
    while text.starts_with('[') {
        match text.find(']') {
            Some(end) => text = text[end + 1..].trim_start(),
            None => break,
        }
    }
    let first = text
        .split(['\n', '.', ';', '!', '?'])
        .map(str::trim)
        .find(|s| !s.is_empty())
        .unwrap_or("");
    let first = first.split(" — ").next().unwrap_or(first);
    let title = one_line(first, TITLE_MAX);
    if title.is_empty() { "Untitled".into() } else { title }
}

/// A routed task's card title: the few words the Brain dump named it with,
/// else (an older Brain dump, which named none) made from what it sent.
/// The whole of what it sent stays the card's detail.
pub(crate) fn route_card_title(route: &Route, sent: &str) -> String {
    match route.title.as_deref().map(|t| one_line(t, TITLE_MAX)) {
        Some(title) if !title.is_empty() => title,
        _ => card_title(sent),
    }
}

/// How urgent the wording says it is.
pub(crate) fn priority_of(text: &str) -> Priority {
    let t = text.to_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| t.contains(n));
    if has(&["urgent", "asap", "right now", "immediately", "critical", "blocker"]) {
        Priority::Urgent
    } else if has(&["when you get a chance", "next week", "someday", "eventually", "low priority", "no rush", "whenever"]) {
        Priority::Low
    } else {
        Priority::Normal
    }
}

/// The card's state for its route. Review holds only what waits on the
/// person: a finished task that left them something (`review`: an open PR,
/// a decision, a result to check). Finished with nothing for them, Done;
/// still going (delivered, or an outcome so far while its thread works),
/// Now; failed, Blocked. A question its thread asks is a word on one card
/// ("Waiting on you"), never a column.
pub(crate) fn card_state(route: &Route) -> WorkState {
    match route.state {
        RouteState::Failed => WorkState::Blocked,
        RouteState::Done if route.review.is_some() => WorkState::Review,
        RouteState::Done => WorkState::Done,
        _ => WorkState::Active,
    }
}

/// A card's outcome: what to review first ("Review: …"), then its route's.
pub(crate) fn card_outcome(route: &Route) -> Option<String> {
    match (&route.review, &route.answer) {
        (Some(review), Some(answer)) => Some(format!("Review: {review}\n\n{answer}")),
        (Some(review), None) => Some(format!("Review: {review}")),
        (None, answer) => answer.clone(),
    }
}

/// Every route's message to `chat` (what it was sent), across `dumps`.
fn dump_messages_to(dumps: &[Dump], chat: &str) -> Vec<String> {
    dumps
        .iter()
        .flat_map(|d| &d.routes)
        .filter(|r| super::watchers::target_chat(&r.target) == Some(chat))
        .map(|r| if r.message.trim().is_empty() { r.excerpt.clone() } else { r.message.clone() })
        .collect()
}

/// The thread a route's work runs in, and the orchestrator that owns it.
fn placed(rpc: &EngineRpc, target: &RouteTarget) -> (Option<String>, Option<String>) {
    match target {
        RouteTarget::Orchestrator { id } | RouteTarget::NewOrchestrator { id } if !id.is_empty() => {
            (None, Some(id.clone()))
        }
        RouteTarget::Chat { id } | RouteTarget::NewThread { id } if !id.is_empty() => {
            let owner = rpc.orchestration().crew_of(id).map(|m| m.orchestrator_id);
            (Some(id.clone()), owner)
        }
        _ => (None, None),
    }
}

impl EngineRpc {
    /// The card for a task `route` of `dump` that was just delivered, made
    /// once (its id on the route); then brought up to date. Nothing for any
    /// other route.
    pub(crate) fn card_for_route(&self, dump: &Dump, route: &mut Route) {
        if route.intent != RouteIntent::Task
            || !matches!(route.state, RouteState::Delivered | RouteState::Answered | RouteState::Done)
        {
            return;
        }
        let (thread, owner) = placed(self, &route.target);
        if thread.is_none() && owner.is_none() {
            return;
        }
        let orch = self.orchestration();
        // Made once: a card taken off the board stays off.
        if let Some(id) = route.item_id.clone() {
            self.sync_card(&id, route);
            return;
        }
        let source = RouteRef { dump_id: dump.id.clone(), route_id: route.id.clone() };
        let existing = orch.workspace().read_orchestration_with(|doc| {
            doc.read_work_items().into_iter().find(|i| i.source.as_ref() == Some(&source))
        });
        if existing.as_ref().is_some_and(|i| i.removed) {
            route.item_id = existing.map(|i| i.id);
            return;
        }
        let existing = existing.map(|i| i.id);
        let sent = if route.message.trim().is_empty() { &route.excerpt } else { &route.message };
        let made = match existing {
            Some(id) => Ok(id),
            None => orch.patch_item(WorkItemPatch {
                orchestrator_id: Some(owner.clone()),
                title: Some(route_card_title(route, sent)),
                detail: Some(sent.trim().to_owned()),
                state: Some(WorkState::Active),
                priority: Some(priority_of(&format!("{} {sent}", route.excerpt))),
                threads: Some(thread.into_iter().collect()),
                source: Some(source),
                ..Default::default()
            }),
        };
        match made {
            Ok(id) => {
                route.item_id = Some(id.clone());
                self.sync_card(&id, route);
            }
            Err(error) => tracing::warn!(%error, "board: a routed task's card wasn't made"),
        }
    }

    /// The route a card came from, if it came from one.
    fn route_of_card(&self, card: &zeron_proto::orchestration::WorkItem) -> Option<Route> {
        let source = card.source.as_ref()?;
        self.orchestration()
            .workspace()
            .read_orchestration_with(|doc| doc.dump(&source.dump_id))
            .and_then(|d| d.routes.into_iter().find(|r| r.id == source.route_id))
    }

    /// Card `id`'s column was set by hand at `at`: kept while its route's
    /// state stays what it is now.
    pub(crate) fn mark_moved(&self, id: &str, at: chrono::DateTime<chrono::Utc>) {
        let orch = self.orchestration();
        let Some(card) = orch.item(id) else { return };
        let route_state = self.route_of_card(&card).map(|r| r.state);
        orch.workspace().write_orchestration(|doc| {
            if let Some(mut card) = doc.work_item(id) {
                card.moved_at = Some(at);
                card.moved_route_state = route_state;
                doc.upsert_work_item(&card);
            }
        });
    }

    fn clear_moved(&self, id: &str) {
        self.orchestration().workspace().write_orchestration(|doc| {
            if let Some(mut card) = doc.work_item(id)
                && card.moved_at.is_some()
            {
                card.moved_at = None;
                card.moved_route_state = None;
                doc.upsert_work_item(&card);
            }
        });
    }

    /// Bring the card `item` up to date with `route`: its state and outcome.
    /// A card moved by hand keeps its column while its route's state is the
    /// one it had then; once the route moves on (it failed, was rerouted,
    /// finished since), the route's state is the card's again.
    pub(crate) fn sync_card(&self, item: &str, route: &Route) {
        let orch = self.orchestration();
        let Some(card) = orch.item(item) else { return };
        let kept = card.moved_at.is_some() && card.moved_route_state == Some(route.state);
        if card.moved_at.is_some() && !kept {
            self.clear_moved(item);
        }
        let state = if kept { card.state } else { card_state(route) };
        let outcome = card_outcome(route);
        if card.state == state && (outcome.is_none() || card.outcome == outcome) {
            return;
        }
        let patch = WorkItemPatch {
            id: Some(card.id.clone()),
            state: Some(state),
            outcome: outcome.map(Some),
            ..Default::default()
        };
        if let Err(error) = orch.patch_item(patch) {
            tracing::warn!(%error, "board: a card wasn't brought up to date");
        }
    }

    /// Every routed card of `dump_id`'s route `route_id`, brought up to date.
    pub(crate) fn sync_route_card(&self, dump_id: &str, route_id: &str) {
        let route = self
            .orchestration()
            .workspace()
            .read_orchestration_with(|doc| doc.dump(dump_id))
            .and_then(|d| d.routes.into_iter().find(|r| r.id == route_id));
        if let Some(route) = route
            && let Some(item) = route.item_id.clone()
        {
            self.sync_card(&item, &route);
        }
    }

    /// Routed cards set straight once at start: one Blocked though its
    /// route didn't fail (an open ask made it Blocked before an ask stopped
    /// meaning Blocked) is re-derived from its route; one in another column
    /// than its route says, never marked, was moved by hand before the mark
    /// existed: marked now, so it stays. Cards already marked are left
    /// alone; run again, it does nothing. `(re-stated, kept)`.
    pub(crate) async fn settle_card_states(&self) -> (Vec<String>, Vec<String>) {
        let state = self.orchestration().state();
        let (mut restated, mut kept) = (Vec::new(), Vec::new());
        for dump in &state.dumps {
            for route in &dump.routes {
                let Some(item) = route.item_id.as_deref() else { continue };
                let Some(card) = state.items.iter().find(|i| i.id == item && !i.removed) else { continue };
                if card.moved_at.is_some() {
                    continue;
                }
                // In Review under the old rule (an outcome so far, or
                // finished): what waits on the person, if anything, from its
                // thread now and what it said.
                if card.state == WorkState::Review && route.review.is_none() && route.state != RouteState::Failed {
                    if self.restate_review(dump, route, card).await {
                        restated.push(item.to_owned());
                    }
                    continue;
                }
                let derived = card_state(route);
                if card.state == derived {
                    continue;
                }
                if card.state == WorkState::Blocked {
                    self.sync_card(item, route);
                    restated.push(item.to_owned());
                } else {
                    self.mark_moved(item, card.updated_at);
                    kept.push(item.to_owned());
                }
            }
        }
        (restated, kept)
    }

    /// A routed card left in Review by the old rule (Review meant "an
    /// outcome so far"), never moved by hand, re-stated by the new one:
    /// - its thread at work: Now;
    /// - waiting on a decision or answer from the person (its thread's
    ///   latest reply to it, its route's outcome, or its own outcome text,
    ///   say so): Now, and its question raised as an ask if it has none;
    /// - something left to evaluate (an open PR, a result to check,
    ///   screenshots): Review, that first in its outcome;
    /// - else Done (a route still going whose thread isn't known: Now).
    /// Its route takes the same. `true` when the card changed.
    async fn restate_review(
        &self,
        dump: &Dump,
        route: &Route,
        card: &zeron_proto::orchestration::WorkItem,
    ) -> bool {
        use zeron_proto::turn_question::{Verdict, detect, review_of};
        let chat = super::watchers::target_chat(&route.target).map(str::to_owned);
        // Its thread's session: hosted here, else its host's synced row.
        let session = chat.as_deref().and_then(|c| {
            self.sessions_ref()
                .watch_sessions()
                .borrow()
                .iter()
                .find(|s| s.chat_id == c)
                .cloned()
                .or_else(|| {
                    self.orchestration().workspace().watch_session_rows().borrow().iter().find(|s| s.chat_id == c).cloned()
                })
        });
        // (Only a route still going asks whether its thread works: a done
        // one stays done whatever its thread does now.)
        let busy = session.as_ref().filter(|_| route.state != RouteState::Done).map(|s| {
            s.status == zeron_proto::SessionStatus::Working || super::watchers::task_state(s) == RouteState::Answered
        });
        let entries = chat
            .as_deref()
            .and_then(|c| self.doc_host_ref().open(c).ok()?.doc().read_entries().ok())
            .unwrap_or_default();
        let sent = if route.message.trim().is_empty() { &route.excerpt } else { &route.message };
        let latest = chat.as_deref().and_then(|c| {
            let others: Vec<String> = dump_messages_to(&self.orchestration().state().dumps, c);
            super::watchers::latest_reply_until(&entries, sent, &others)
        });
        let marked = card
            .outcome
            .as_deref()
            .and_then(|o| o.lines().next())
            .and_then(|l| l.strip_prefix("Review: "))
            .map(|l| l.trim_matches('*').trim().to_owned())
            .filter(|l| !l.is_empty());
        let evidence: Vec<&str> =
            [latest.as_deref(), route.answer_full.as_deref(), card.outcome.as_deref()].into_iter().flatten().collect();
        let decision = evidence.iter().find_map(|t| match detect(t) {
            Verdict::Ask(found) => Some(found),
            _ => None,
        });
        let (state, review) = if busy == Some(true) {
            (RouteState::Answered, None)
        } else if decision.is_some() {
            (RouteState::Answered, None)
        } else {
            let review = marked.or_else(|| evidence.iter().find_map(|t| review_of(t)).map(|r| r.line()));
            match (busy, route.state, &review) {
                (_, RouteState::Done, _) | (Some(false), _, _) | (None, _, Some(_)) => (RouteState::Done, review),
                _ => (route.state, None),
            }
        };
        let updated = self.orchestration().workspace().write_orchestration(|doc| {
            let Some(mut d) = doc.dump(&dump.id) else { return None };
            let r = d.routes.iter_mut().find(|r| r.id == route.id)?;
            r.state = state;
            r.review = review.clone();
            let r = r.clone();
            doc.upsert_dump(&d);
            Some(r)
        });
        let Some(route) = updated else { return false };
        // Waiting on a decision: its ask (one per question; none raised
        // twice), on this card. Not one the person already answered (a
        // message reached the thread after the one that asked: their reply,
        // a relayed answer, an answered ask's delivery), which closes any
        // open ask for it; nor for a card approved or moved by hand.
        if let (Some(found), Some(chat)) = (&decision, chat.as_deref())
            && busy != Some(true)
            && card.approved_at.is_none()
            && card.moved_at.is_none()
        {
            match super::turn_asks::answered_after_asking(self.orchestration(), self, chat, &entries, Some(sent)).await {
                Some(reply) => {
                    let open: Vec<String> = self
                        .orchestration()
                        .state()
                        .asks
                        .iter()
                        .filter(|a| a.from_chat == chat && a.state == zeron_proto::orchestration::AskState::Open)
                        .filter(|a| {
                            a.item_id.as_deref() == Some(card.id.as_str())
                                || zeron_proto::turn_question::same_question(
                                    &a.question,
                                    &a.options.iter().map(|o| o.label.clone()).collect::<Vec<_>>(),
                                    &found.question,
                                    &found.options,
                                )
                        })
                        .map(|a| a.id.clone())
                        .collect();
                    for id in open {
                        super::turn_asks::close(self.orchestration(), &id, zeron_proto::orchestration::AnswerVia::Thread, &reply, chrono::Utc::now());
                    }
                }
                None => {
                    self.ask_for(chat, found, Some((&dump.id, &route.id)));
                }
            }
        }
        self.sync_card(&card.id, &route);
        self.orchestration().item(&card.id).is_some_and(|c| c.state != WorkState::Review || c.outcome != card.outcome)
    }

    /// The routed cards `chat` works on, brought up to date (it raised or
    /// answered an ask).
    pub(crate) fn sync_cards_of(&self, chat: &str) {
        let state = self.orchestration().state();
        for dump in &state.dumps {
            for route in &dump.routes {
                if let Some(item) = &route.item_id
                    && state.items.iter().any(|i| &i.id == item && i.threads.iter().any(|t| t == chat))
                {
                    self.sync_card(item, route);
                }
            }
        }
    }

    /// The person approves card `id` (from Review): Done, its approval
    /// recorded, kept Done (a move by hand) while its route stays as it is.
    pub(crate) fn approve_item(&self, id: &str, at: chrono::DateTime<chrono::Utc>) -> Result<(), RpcError> {
        let orch = self.orchestration();
        let card = orch.item(id).ok_or_else(|| RpcError::Failed(format!("No board item {id}")))?;
        orch.patch_item(WorkItemPatch { id: Some(id.to_owned()), state: Some(WorkState::Done), ..Default::default() })
            .map_err(RpcError::Failed)?;
        orch.workspace().write_orchestration(|doc| {
            if let Some(mut c) = doc.work_item(id) {
                c.approved_at = Some(at);
                doc.upsert_work_item(&c);
            }
        });
        self.mark_moved(id, at);
        let to = card.threads.first().cloned().or(card.orchestrator_id.clone()).unwrap_or_else(|| id.to_owned());
        orch.comms().say(zeron_proto::orchestration::CommKind::Answer, super::comms::YOU, &to, &format!("Approved: {}", one_line(&card.title, 80)));
        Ok(())
    }

    /// The person asks for changes on card `id`: `text` to its thread (else
    /// its orchestrator) as a message, the card back in Now (its route going
    /// again, nothing left to review).
    pub(crate) fn request_changes(&self, id: &str, text: &str) -> Result<(), RpcError> {
        let orch = self.orchestration();
        let card = orch.item(id).ok_or_else(|| RpcError::Failed(format!("No board item {id}")))?;
        let to = card
            .threads
            .first()
            .cloned()
            .or(card.orchestrator_id.clone())
            .ok_or_else(|| RpcError::Failed("That card has no thread to send it to".into()))?;
        let message = zeron_proto::orchestration::prompt_text::changes_requested(&card.title, text);
        super::ops::deliver(self, &to, &message, None, super::ops::When::Next)?;
        orch.comms().say(zeron_proto::orchestration::CommKind::Message, super::comms::YOU, &to, &one_line(text, 140));
        let route = self.route_of_card(&card);
        match route {
            Some(route) => {
                let source = card.source.clone().expect("a routed card has its source");
                let updated = orch.workspace().write_orchestration(|doc| {
                    let mut d = doc.dump(&source.dump_id)?;
                    let r = d.routes.iter_mut().find(|r| r.id == route.id)?;
                    r.state = RouteState::Answered;
                    r.review = None;
                    let r = r.clone();
                    doc.upsert_dump(&d);
                    Some(r)
                });
                if let Some(route) = updated {
                    self.sync_card(id, &route);
                }
            }
            None => {
                orch.patch_item(WorkItemPatch { id: Some(id.to_owned()), state: Some(WorkState::Active), ..Default::default() })
                    .map_err(RpcError::Failed)?;
                self.mark_moved(id, chrono::Utc::now());
            }
        }
        Ok(())
    }

    /// The person takes card `id` off the board: kept, marked removed.
    pub(crate) fn remove_item(&self, id: &str) -> Result<(), RpcError> {
        self.orchestration().workspace().write_orchestration(|doc| {
            let mut item = doc.work_item(id).ok_or_else(|| RpcError::Failed(format!("No board item {id}")))?;
            if !item.removed {
                let now = chrono::Utc::now();
                item.removed = true;
                item.removed_at = Some(now);
                item.updated_at = now;
                doc.upsert_work_item(&item);
            }
            Ok(())
        })
    }

    /// The person's "add to the board" for a chat: its card (the one it
    /// already has on a board, when it has one not done). Its id.
    pub(crate) fn add_to_board(&self, chat_id: &str) -> Result<String, RpcError> {
        let chat = self
            .workspace_host()
            .chat(chat_id)
            .map_err(|e| RpcError::Failed(e.to_string()))?
            .ok_or_else(|| RpcError::Failed(format!("No chat {chat_id}")))?;
        let orch = self.orchestration();
        if let Some(card) = orch
            .state()
            .items
            .into_iter()
            .find(|i| i.threads.iter().any(|t| t == chat_id) && !matches!(i.state, WorkState::Done | WorkState::Dropped))
        {
            return Ok(card.id);
        }
        let working = self.sessions_ref().turn_in_flight(chat_id);
        orch.patch_item(WorkItemPatch {
            orchestrator_id: Some(orch.crew_of(chat_id).map(|m| m.orchestrator_id)),
            title: Some(one_line(chat.title.as_deref().unwrap_or("Untitled"), TITLE_MAX)),
            state: Some(if working { WorkState::Active } else { WorkState::Planned }),
            threads: Some(vec![chat_id.to_owned()]),
            ..Default::default()
        })
        .map_err(RpcError::Failed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_are_the_first_clause_without_markers() {
        assert_eq!(card_title("Make device names unique. A second one gets -1."), "Make device names unique");
        assert_eq!(card_title("[For your crew thread Lines (c1)] Add -l as an alias of --lines"), "Add -l as an alias of --lines");
        assert_eq!(card_title("Fix the login — it 500s on Safari"), "Fix the login");
        assert_eq!(card_title(&"x".repeat(200)).chars().count(), TITLE_MAX);
        assert_eq!(card_title("   "), "Untitled");
    }

    #[test]
    fn a_routed_card_takes_the_title_the_brain_dump_named() {
        let sent = "Feature request for the Board: the person wants to delete a card from its menu, with an Undo";
        let mut route: Route = serde_json::from_value(serde_json::json!({
            "id": "r", "excerpt": "x", "target": { "kind": "chat", "id": "c" }, "intent": "task", "state": "delivered",
            "message": sent, "title": "  Delete cards from the Board\n"
        }))
        .unwrap();
        assert_eq!(route_card_title(&route, sent), "Delete cards from the Board");
        route.title = Some("   ".into());
        assert_eq!(route_card_title(&route, sent), card_title(sent), "a blank title falls back");
        route.title = None;
        assert_eq!(route_card_title(&route, sent), card_title(sent), "an older Brain dump named none");
    }

    #[test]
    fn wording_sets_priority() {
        assert_eq!(priority_of("fix prod asap"), Priority::Urgent);
        assert_eq!(priority_of("This is urgent"), Priority::Urgent);
        assert_eq!(priority_of("tidy the docs when you get a chance"), Priority::Low);
        assert_eq!(priority_of("look at it next week"), Priority::Low);
        assert_eq!(priority_of("add a -l alias"), Priority::Normal);
    }

    #[test]
    fn a_card_follows_its_route() {
        let mut route: Route = serde_json::from_value(serde_json::json!({
            "id": "r", "excerpt": "x", "target": { "kind": "chat", "id": "c" }, "intent": "task", "state": "delivered"
        }))
        .unwrap();
        assert_eq!(card_state(&route), WorkState::Active);
        // An outcome so far, its thread still at work: Now, never Review.
        route.state = RouteState::Answered;
        assert_eq!(card_state(&route), WorkState::Active);
        route.state = RouteState::Done;
        assert_eq!(card_state(&route), WorkState::Done);
        // Finished, with something for the person: Review, that first.
        route.review = Some("PR https://github.com/zeronsh/zeron/pull/1403".into());
        route.answer = Some("Opened the PR.".into());
        assert_eq!(card_state(&route), WorkState::Review);
        assert_eq!(card_outcome(&route).as_deref(), Some("Review: PR https://github.com/zeronsh/zeron/pull/1403\n\nOpened the PR."));
        route.review = None;
        // Only a real failure is Blocked.
        route.state = RouteState::Failed;
        assert_eq!(card_state(&route), WorkState::Blocked);
    }
}
