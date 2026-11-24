//! Routed tasks land on the board. Every task the Brain dump delivers to a
//! thread (or to an orchestrator) gets one card, linked to the thread and
//! the route. Its place is worked out like any card's (`derive.rs`), its
//! route one of the facts: while the route's target does the card's work,
//! the route's end is the card's report (done with nothing for the person:
//! Done; with something to look at: Needs you, "Looks good" or what to
//! change). Its outcome is the route's answer, after "Review: …" when it
//! left something to look at.

use zeron_proto::orchestration::{
    CheckWhy, Checking, Dump, Mover, Place, Priority, Route, RouteIntent, RouteRef, RouteState, RouteTarget, WorkItemPatch,
    WorkState, prompt_text::one_line,
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

/// The column a route's state meant before places (what `derive.rs` reads
/// of a route now is its state and review line themselves). Kept for the
/// cards of boards an older engine keeps.
#[cfg_attr(not(test), allow(dead_code))]
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

fn texts_of(e: &zeron_doc::SessionMessageEntry) -> String {
    e.parts
        .iter()
        .filter_map(|p| match p {
            zeron_doc::MessagePart::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

/// The words a card or a decision is about, lowercase: "R9" and "round 9"
/// alike (`round9`).
fn topic_words(s: &str) -> std::collections::BTreeSet<String> {
    const STOP: &[&str] = &[
        "the", "and", "for", "your", "you", "decision", "decide", "waiting", "still", "pick", "choose", "with", "this",
        "that", "from", "into", "are", "was", "keep",
    ];
    let lower = s.to_lowercase();
    let words: Vec<&str> = lower.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let mut out = std::collections::BTreeSet::new();
    for (i, w) in words.iter().enumerate() {
        if let Some(n) = w.strip_prefix('r').filter(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit())) {
            out.insert(format!("round{n}"));
        } else if *w == "round" && words.get(i + 1).is_some_and(|n| n.chars().all(|c| c.is_ascii_digit())) {
            out.insert(format!("round{}", words[i + 1]));
        } else if w.len() >= 3 && !STOP.contains(w) && !w.chars().all(|c| c.is_ascii_digit()) {
            // (Variant, variants, variation: one word.)
            let stem: String = w.chars().take(5).collect();
            out.insert(stem);
        }
    }
    out
}

/// The card a decision belongs to: the one whose title (and what it was
/// sent) shares the most words with it, else the first.
fn best_card<'a>(
    part: &zeron_proto::turn_question::Found,
    cards: &[(&'a Dump, &'a Route, &'a zeron_proto::orchestration::WorkItem)],
) -> (&'a Dump, &'a Route, &'a zeron_proto::orchestration::WorkItem) {
    let words = topic_words(&format!("{} {}", part.question, part.options.join(" ")));
    let score = |(_, r, c): &(&Dump, &Route, &zeron_proto::orchestration::WorkItem)| {
        topic_words(&format!("{} {}", c.title, r.excerpt)).intersection(&words).count()
    };
    let best = cards.iter().enumerate().max_by_key(|(i, c)| (score(c), std::cmp::Reverse(*i))).map(|(_, c)| *c);
    match best {
        Some(c) if score(&c) > 0 => c,
        _ => cards[0],
    }
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
        let fresh = existing.is_none();
        let to_thread = thread.is_some();
        let made = match existing {
            Some(id) => Ok(id),
            None => orch.patch_item(WorkItemPatch {
                orchestrator_id: Some(owner.clone()),
                title: Some(route_card_title(route, sent)),
                detail: Some(sent.trim().to_owned()),
                priority: Some(priority_of(&format!("{} {sent}", route.excerpt))),
                threads: Some(thread.into_iter().collect()),
                source: Some(source),
                ..Default::default()
            }),
        };
        match made {
            Ok(id) => {
                route.item_id = Some(id.clone());
                if fresh {
                    // Its work is on its way: to its thread, or (routed to an
                    // orchestrator) as that orchestrator's own turn.
                    let now = chrono::Utc::now();
                    self.with_card(&id, now, |row, _| {
                        if to_thread {
                            row.work_at = Some(now);
                        } else {
                            row.checking = Some(Checking { since: now, why: CheckWhy::Created });
                        }
                    });
                }
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

    /// Bring the card `item` up to date with `route`: its outcome, and its
    /// place (the route's state and review line are facts of it).
    pub(crate) fn sync_card(&self, item: &str, route: &Route) {
        let orch = self.orchestration();
        let Some(card) = orch.item(item) else { return };
        let outcome = card_outcome(route);
        let now = chrono::Utc::now();
        if outcome.is_some() && card.outcome != outcome {
            let patch = WorkItemPatch { id: Some(card.id.clone()), outcome: outcome.map(Some), ..Default::default() };
            if let Err(error) = orch.patch_item(patch) {
                tracing::warn!(%error, "board: a card wasn't brought up to date");
            }
        }
        // Its route has an outcome: its target was heard from since the
        // work was sent.
        if !matches!(route.state, RouteState::Delivered | RouteState::Pending | RouteState::Undone | RouteState::Other)
            && self.board_card(item).is_some_and(|row| row.heard_at.is_none_or(|h| row.work_at.is_some_and(|sent| h < sent)))
        {
            self.with_card(item, now, |row, _| row.heard_at = Some(now));
        }
        self.derive_cards(&[item.to_owned()], Mover::Engine, now);
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

    /// Once at start: every card at work waiting on a decision its thread
    /// put in words gets its ask ([`Self::ask_for_waiting_cards`]). (The
    /// columns an older engine left are mapped when each card is first
    /// worked out, `board.rs`.) `(asked, [])`.
    pub(crate) async fn settle_card_states(&self) -> (Vec<String>, Vec<String>) {
        let asked = self.ask_for_waiting_cards(&[]).await;
        if !asked.is_empty() {
            tracing::info!(cards = ?asked, "board: asks raised for cards waiting on a decision");
        }
        (asked, Vec::new())
    }

    /// Card `card_id` (route `route_id` of dump `dump_id`, its thread `chat`)
    /// waits on the decision `found`: its ask is raised (one per question,
    /// on this card), unless a message that reached the thread after the one
    /// that asked answers it ([`super::turn_asks::answered_after_asking`]),
    /// which closes any open ask for it instead. `true` when it raised one.
    #[allow(clippy::too_many_arguments)]
    async fn ask_or_settle_decision(
        &self,
        chat: &str,
        entries: &[zeron_doc::SessionMessageEntry],
        sent: &str,
        found: &zeron_proto::turn_question::Found,
        dump_id: &str,
        route_id: &str,
        card_id: &str,
    ) -> bool {
        match super::turn_asks::answered_after_asking(self.orchestration(), self, chat, entries, Some(sent)).await {
            Some(reply) => {
                let open: Vec<String> = self
                    .orchestration()
                    .state()
                    .asks
                    .iter()
                    .filter(|a| a.from_chat == chat && a.state == zeron_proto::orchestration::AskState::Open)
                    .filter(|a| {
                        a.item_id.as_deref() == Some(card_id)
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
                false
            }
            None => self.ask_for(chat, found, Some((dump_id, route_id))).is_some(),
        }
    }

    /// Routed cards not closed, waiting on a decision their thread put in
    /// words, with no open ask for it: their ask, raised (or, answered
    /// since, the open one closed). The cards it asked for. (`done`: cards
    /// to pass over.)
    pub(crate) async fn ask_for_waiting_cards(&self, done: &[String]) -> Vec<String> {
        use zeron_proto::turn_question::{Verdict, detect, split_compound};
        let state = self.orchestration().state();
        // Each thread's cards in Now (not approved, not failed, not just
        // settled), with their routes.
        let mut by_chat: std::collections::BTreeMap<String, Vec<(&Dump, &Route, &zeron_proto::orchestration::WorkItem)>> =
            Default::default();
        for dump in &state.dumps {
            for route in &dump.routes {
                let Some(item) = route.item_id.as_deref() else { continue };
                let Some(card) = state.items.iter().find(|i| i.id == item && !i.removed) else { continue };
                // (Not closed, paused or waiting to start: by its row, else,
                // from before places, its old column.)
                let at_work = match state.cards.iter().find(|c| c.id == item) {
                    Some(row) => matches!(row.place, Place::Running | Place::NeedsYou | Place::Snoozed),
                    None => card.state == WorkState::Active,
                };
                if !at_work
                    || card.approved_at.is_some()
                    || route.state == RouteState::Failed
                    || done.iter().any(|d| d == item)
                {
                    continue;
                }
                let Some(chat) = super::watchers::target_chat(&route.target) else { continue };
                by_chat.entry(chat.to_owned()).or_default().push((dump, route, card));
            }
        }
        let mut asked = Vec::new();
        for (chat, mut cards) in by_chat {
            let entries = self
                .doc_host_ref()
                .open(&chat)
                .ok()
                .and_then(|h| h.doc().read_entries().ok())
                .unwrap_or_default();
            let sent_of = |route: &Route| -> String {
                if route.message.trim().is_empty() { route.excerpt.clone() } else { route.message.clone() }
            };
            // Earliest delivered first: the look-back starts at the first.
            let delivered = |route: &Route| {
                let sent = sent_of(route);
                entries
                    .iter()
                    .rposition(|e| e.role == zeron_doc::MessageRole::User && texts_of(e).contains(sent.trim()))
                    .unwrap_or(usize::MAX)
            };
            cards.sort_by_key(|(_, r, _)| delivered(r));
            let first_sent = sent_of(cards[0].1);
            match super::turn_asks::last_decision_since(&entries, Some(&first_sent)) {
                // The thread's latest decision since: each choice it names on
                // its own, on the card it belongs to.
                Some((at, found)) => {
                    let message = entries[at].id.clone();
                    let parts = split_compound(&found);
                    for (i, part) in parts.iter().enumerate() {
                        let turn = if parts.len() > 1 { super::turn_asks::part_turn(&message, i) } else { message.clone() };
                        let (dump, route, card) = best_card(part, &cards);
                        match super::turn_asks::part_answer(self.orchestration(), self, &chat, &entries, at, part).await {
                            Some(reply) => self.close_asks_for(&chat, part, &super::turn_asks::ask_id(&chat, &turn), &card.id, &reply),
                            None => {
                                let mut launch = super::turn_asks::launch_of_chat(self.orchestration(), self, &chat).unwrap_or_default();
                                launch.item_id = Some(card.id.clone());
                                let plan = super::turn_asks::raise(
                                    self.orchestration(),
                                    self,
                                    &chat,
                                    Some(&turn),
                                    part,
                                    &launch,
                                    Some((&dump.id, &route.id)),
                                    chrono::Utc::now(),
                                );
                                if matches!(plan, super::turn_asks::Plan::Create(_)) {
                                    asked.push(card.id.clone());
                                }
                            }
                        }
                    }
                }
                // None in the transcript: what each card's route or own
                // outcome says.
                None => {
                    for (dump, route, card) in &cards {
                        let found = [route.answer_full.as_deref(), card.outcome.as_deref()].into_iter().flatten().find_map(|t| {
                            match detect(t) {
                                Verdict::Ask(found) => Some(found),
                                _ => None,
                            }
                        });
                        let Some(found) = found else { continue };
                        let sent = sent_of(route);
                        if self.ask_or_settle_decision(&chat, &entries, &sent, &found, &dump.id, &route.id, &card.id).await {
                            asked.push(card.id.clone());
                        }
                    }
                }
            }
        }
        asked
    }

    /// Open asks of `chat` for `part` (its id, its card, or the same
    /// question), closed as answered in the thread by `reply`.
    fn close_asks_for(&self, chat: &str, part: &zeron_proto::turn_question::Found, id: &str, card: &str, reply: &str) {
        let open: Vec<String> = self
            .orchestration()
            .state()
            .asks
            .iter()
            .filter(|a| a.from_chat == chat && a.state == zeron_proto::orchestration::AskState::Open)
            .filter(|a| {
                a.id == id
                    || zeron_proto::turn_question::same_question(
                        &a.question,
                        &a.options.iter().map(|o| o.label.clone()).collect::<Vec<_>>(),
                        &part.question,
                        &part.options,
                    )
                    || (a.item_id.as_deref() == Some(card) && a.question == part.question)
            })
            .map(|a| a.id.clone())
            .collect();
        for id in open {
            super::turn_asks::close(self.orchestration(), &id, zeron_proto::orchestration::AnswerVia::Thread, reply, chrono::Utc::now());
        }
    }

    /// The cards `chat` works on, worked out now (it raised or answered an
    /// ask).
    pub(crate) fn sync_cards_of(&self, chat: &str) {
        let cards = self.cards_of_chat(chat);
        self.derive_cards(&cards, Mover::Engine, chrono::Utc::now());
    }

    /// The person asks for changes on card `id`: `text` to its thread (else
    /// its orchestrator) as a message; a closed card is open again (its
    /// route going again, nothing left to look at).
    pub(crate) fn request_changes(&self, id: &str, text: &str) -> Result<(), RpcError> {
        let orch = self.orchestration();
        let card = orch.item(id).ok_or_else(|| RpcError::Failed(format!("No board item {id}")))?;
        let to = card
            .threads
            .first()
            .cloned()
            .or(card.orchestrator_id.clone())
            .ok_or_else(|| RpcError::Failed("That card has no thread to send it to".into()))?;
        let now = chrono::Utc::now();
        // Open again, whoever closed it; its thread's old word no longer
        // closes it; work is on its way.
        let to_owner = Some(&to) == card.orchestrator_id.as_ref();
        self.with_card(id, now, |row, item| {
            row.closed = None;
            row.word = None;
            row.stall = None;
            row.held = false;
            row.work_at = Some(now);
            if to_owner {
                row.checking = Some(Checking { since: now, why: CheckWhy::Answer });
            }
            item.approved_at = None;
        });
        let message = zeron_proto::orchestration::prompt_text::changes_requested(&card.title, text);
        super::ops::deliver(self, &to, &message, None, super::ops::When::Next, Some(zeron_doc::Injected::changes(&card.id)))?;
        orch.comms().say(zeron_proto::orchestration::CommKind::Message, super::comms::YOU, &to, &one_line(text, 140));
        if let (Some(route), Some(source)) = (self.route_of_card(&card), card.source.clone()) {
            orch.workspace().write_orchestration(|doc| {
                let mut d = doc.dump(&source.dump_id)?;
                let r = d.routes.iter_mut().find(|r| r.id == route.id)?;
                r.state = RouteState::Answered;
                r.review = None;
                doc.upsert_dump(&d);
                Some(())
            });
        }
        self.derive_cards(&[id.to_owned()], Mover::You, now);
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
        let state = orch.state();
        let open = |i: &zeron_proto::orchestration::WorkItem| match state.cards.iter().find(|c| c.id == i.id) {
            Some(row) => row.closed.is_none(),
            None => !matches!(i.state, WorkState::Done | WorkState::Dropped),
        };
        if let Some(card) = state.items.iter().find(|i| i.threads.iter().any(|t| t == chat_id) && open(i)) {
            return Ok(card.id.clone());
        }
        // (Running while its chat works; else where its facts put it.)
        orch.patch_item(WorkItemPatch {
            orchestrator_id: Some(orch.crew_of(chat_id).map(|m| m.orchestrator_id)),
            title: Some(one_line(chat.title.as_deref().unwrap_or("Untitled"), TITLE_MAX)),
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
