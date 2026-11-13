//! Routed tasks land on the board. Every task the Brain dump delivers to a
//! thread (or to an orchestrator) gets one card, linked to the thread and
//! the route, and the card follows the route: Now while the work goes on,
//! Review once an outcome lands with work left, Done when the route is done,
//! Blocked when it failed or its thread asks the person something. Its
//! outcome is the route's answer.

use zeron_proto::orchestration::{
    AskState, Dump, Priority, Route, RouteIntent, RouteRef, RouteState, RouteTarget, WorkItemPatch, WorkState,
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

/// The card's state for its route: `asking` is whether its thread waits on
/// an open ask.
pub(crate) fn card_state(route: &Route, asking: bool) -> WorkState {
    if asking {
        return WorkState::Blocked;
    }
    match route.state {
        RouteState::Done => WorkState::Done,
        RouteState::Failed => WorkState::Blocked,
        RouteState::Answered => WorkState::Review,
        _ => WorkState::Active,
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

    /// Bring the card `item` up to date with `route`: its state and outcome.
    pub(crate) fn sync_card(&self, item: &str, route: &Route) {
        let orch = self.orchestration();
        let Some(card) = orch.item(item) else { return };
        let asking = orch
            .state()
            .asks
            .iter()
            .any(|a| a.state == AskState::Open && card.threads.contains(&a.from_chat));
        let state = card_state(route, asking);
        let outcome = route.answer.clone();
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
        assert_eq!(card_state(&route, false), WorkState::Active);
        route.state = RouteState::Answered;
        assert_eq!(card_state(&route, false), WorkState::Review);
        assert_eq!(card_state(&route, true), WorkState::Blocked, "its thread asks the person");
        route.state = RouteState::Done;
        assert_eq!(card_state(&route, false), WorkState::Done);
        route.state = RouteState::Failed;
        assert_eq!(card_state(&route, false), WorkState::Blocked);
    }
}
