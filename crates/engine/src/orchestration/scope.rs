//! What an agent sees of orchestration (`WatchOrchestration`, `WatchComms`
//! with an agent's token): its own part, by its role in the rows. The
//! person's window sees everything.
//!
//! - a plain chat: every orchestrator's id, kind and name (enough to know it
//!   is none of them), nothing else;
//! - an orchestrator: its own row, its crew, its items and the loose ones,
//!   the asks it or its crew raised; no dumps;
//! - a crew thread: its own crew row, its orchestrator's name and crew
//!   rules, its item;
//! - the Brain dump: everything but other orchestrators' rules and notes.

use futures::StreamExt;
use futures::stream::BoxStream;
use zeron_proto::orchestration::{CommEvent, CommKind, CrewPolicy, OrchestrationState, Orchestrator, OrchestratorKind};

use super::Orchestration;

/// The role `chat` has in `state`'s rows.
enum Seen<'a> {
    Plain,
    Orchestrator(&'a str),
    BrainDump(&'a str),
    Crew { orchestrator: &'a str, item: Option<&'a str> },
}

fn seen<'a>(state: &'a OrchestrationState, chat: Option<&str>) -> Seen<'a> {
    let Some(chat) = chat else { return Seen::Plain };
    if let Some(o) = state.orchestrators.iter().find(|o| o.id == chat) {
        return match o.kind {
            OrchestratorKind::Standard => Seen::Orchestrator(&o.id),
            OrchestratorKind::BrainDump => Seen::BrainDump(&o.id),
            OrchestratorKind::Other => Seen::Plain,
        };
    }
    match state.crew.iter().find(|m| m.chat_id == chat && !m.released) {
        Some(m) => Seen::Crew {
            orchestrator: &m.orchestrator_id,
            item: m.item_id.as_deref(),
        },
        None => Seen::Plain,
    }
}

/// An orchestrator as anyone may know it: id, kind and name.
fn named(o: &Orchestrator) -> Orchestrator {
    Orchestrator {
        id: o.id.clone(),
        kind: o.kind,
        name: o.name.clone(),
        goal: String::new(),
        rules: String::new(),
        crew_rules: String::new(),
        notes: String::new(),
        hue: o.hue,
        crew: CrewPolicy::default(),
        wake: None,
        paused: o.paused,
        archived: o.archived,
        created_at: o.created_at,
        updated_at: o.updated_at,
    }
}

/// `state` as the agent of `chat` may see it.
pub(crate) fn scoped(state: &OrchestrationState, chat: Option<&str>) -> OrchestrationState {
    match seen(state, chat) {
        Seen::Plain => OrchestrationState {
            orchestrators: state.orchestrators.iter().map(named).collect(),
            ..Default::default()
        },
        Seen::Orchestrator(id) => {
            let crew: Vec<_> = state.crew.iter().filter(|m| m.orchestrator_id == id).cloned().collect();
            let mine = |chat: &str| chat == id || crew.iter().any(|m| m.chat_id == chat);
            OrchestrationState {
                orchestrators: state.orchestrators.iter().filter(|o| o.id == id).cloned().collect(),
                items: state
                    .items
                    .iter()
                    .filter(|i| i.orchestrator_id.as_deref().is_none_or(|o| o == id))
                    .cloned()
                    .collect(),
                asks: state
                    .asks
                    .iter()
                    .filter(|a| a.orchestrator_id.as_deref() == Some(id) || mine(&a.from_chat))
                    .cloned()
                    .collect(),
                crew,
                dumps: Vec::new(),
            }
        }
        Seen::Crew { orchestrator, item } => OrchestrationState {
            orchestrators: state
                .orchestrators
                .iter()
                .filter(|o| o.id == orchestrator)
                .map(|o| Orchestrator { crew_rules: o.crew_rules.clone(), ..named(o) })
                .collect(),
            crew: state.crew.iter().filter(|m| Some(m.chat_id.as_str()) == chat).cloned().collect(),
            items: state.items.iter().filter(|i| Some(i.id.as_str()) == item).cloned().collect(),
            asks: Vec::new(),
            dumps: Vec::new(),
        },
        Seen::BrainDump(id) => OrchestrationState {
            orchestrators: state
                .orchestrators
                .iter()
                .map(|o| {
                    if o.id == id {
                        o.clone()
                    } else {
                        Orchestrator { goal: o.goal.clone(), crew: o.crew.clone(), ..named(o) }
                    }
                })
                .collect(),
            ..state.clone()
        },
    }
}

/// Whether the agent of `chat` sees `event`: what it (or, for an
/// orchestrator, its crew) sent or was sent; the Brain dump also every route.
pub(crate) fn sees(event: &CommEvent, state: &OrchestrationState, chat: Option<&str>) -> bool {
    let Some(chat) = chat else { return false };
    let touches = |id: &str| event.from == id || event.to == id;
    if touches(chat) {
        return true;
    }
    match seen(state, Some(chat)) {
        Seen::Orchestrator(id) => state
            .crew
            .iter()
            .filter(|m| m.orchestrator_id == id)
            .any(|m| touches(&m.chat_id)),
        Seen::BrainDump(_) => event.kind == CommKind::Route,
        _ => false,
    }
}

impl Orchestration {
    /// [`Self::stream`], as the agent of `chat` may see it.
    pub(crate) fn scoped_stream(&self, chat: Option<String>) -> BoxStream<'static, serde_json::Value> {
        let mut rx = self.subscribe();
        rx.mark_unchanged();
        let first = serde_json::to_value(scoped(&self.state(), chat.as_deref())).ok();
        futures::stream::iter(first)
            .chain(futures::stream::unfold((rx, chat), |(mut rx, chat)| async move {
                rx.changed().await.ok()?;
                let value = serde_json::to_value(scoped(&rx.borrow_and_update(), chat.as_deref())).ok()?;
                Some((value, (rx, chat)))
            }))
            .boxed()
    }

    /// The comms stream, as the agent of `chat` may see it (frames that
    /// would be empty are left out after the first).
    pub(crate) fn scoped_comms(&self, chat: Option<String>) -> BoxStream<'static, serde_json::Value> {
        let orchestration = self.clone();
        super::comms::stream(self.comms())
            .enumerate()
            .filter_map(move |(index, frame)| {
                let state = orchestration.state();
                let kept: Vec<CommEvent> = serde_json::from_value::<Vec<CommEvent>>(frame)
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|e| sees(e, &state, chat.as_deref()))
                    .collect();
                let value = (index == 0 || !kept.is_empty()).then(|| serde_json::to_value(kept).ok()).flatten();
                futures::future::ready(value)
            })
            .boxed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use zeron_proto::orchestration::{
        Ask, AskKind, AskState, CrewMember, Dump, DumpState, Priority, WorkItem, WorkState,
    };

    fn orch(id: &str, kind: OrchestratorKind) -> Orchestrator {
        Orchestrator {
            id: id.into(),
            kind,
            name: format!("{id} name"),
            goal: format!("{id} goal"),
            rules: format!("{id} rules"),
            crew_rules: format!("{id} crew rules"),
            notes: format!("{id} notes"),
            hue: 1,
            crew: CrewPolicy::default(),
            wake: None,
            paused: false,
            archived: false,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn member(chat: &str, o: &str, item: Option<&str>) -> CrewMember {
        CrewMember {
            chat_id: chat.into(),
            orchestrator_id: o.into(),
            item_id: item.map(Into::into),
            role: String::new(),
            last_report: None,
            released: false,
            joined_at: Utc::now(),
        }
    }

    fn item(id: &str, o: Option<&str>) -> WorkItem {
        WorkItem {
            id: id.into(),
            orchestrator_id: o.map(Into::into),
            title: id.into(),
            detail: String::new(),
            state: WorkState::Active,
            priority: Priority::Normal,
            progress: None,
            threads: vec![],
            links: vec![],
            outcome: None,
            source: None,
            order: 0.0,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            done_at: None,
            removed: Default::default(),
            removed_at: Default::default(),
            space_id: None,
            device_id: None,
            waiting: None,
        }
    }

    fn ask(id: &str, from: &str, o: Option<&str>) -> Ask {
        Ask {
            id: id.into(),
            from_chat: from.into(),
            orchestrator_id: o.map(Into::into),
            item_id: None,
            kind: AskKind::Question,
            question: "?".into(),
            options: vec![],
            urgent: false,
            state: AskState::Open,
            answer: None,
            created_at: Utc::now(),
            answered_at: None,
            dump_id: None,
            route_id: None,
        }
    }

    fn state() -> OrchestrationState {
        OrchestrationState {
            orchestrators: vec![
                orch("a", OrchestratorKind::Standard),
                orch("b", OrchestratorKind::Standard),
                orch("brain", OrchestratorKind::BrainDump),
            ],
            crew: vec![member("a1", "a", Some("ia")), member("b1", "b", Some("ib"))],
            items: vec![item("ia", Some("a")), item("ib", Some("b")), item("loose", None)],
            asks: vec![ask("q-a", "a1", Some("a")), ask("q-b", "b1", Some("b")), ask("q-plain", "x", None)],
            dumps: vec![Dump {
                id: "d".into(),
                text: "secret plans".into(),
                state: DumpState::Routed,
                routes: vec![],
                summary: None,
                device_id: "dev".into(),
                created_at: Utc::now(),
                updated_at: Utc::now(),
                attachments: Default::default(),
            }],
        }
    }

    fn ids<T>(rows: &[T], id: impl Fn(&T) -> &str) -> Vec<String> {
        rows.iter().map(|r| id(r).to_owned()).collect()
    }

    #[test]
    fn a_plain_chat_sees_only_who_orchestrates() {
        let s = scoped(&state(), Some("x"));
        assert_eq!(ids(&s.orchestrators, |o| &o.id), ["a", "b", "brain"]);
        assert!(s.orchestrators.iter().all(|o| o.rules.is_empty() && o.notes.is_empty() && o.goal.is_empty()));
        assert!(s.crew.is_empty() && s.items.is_empty() && s.asks.is_empty() && s.dumps.is_empty());
        assert_eq!(scoped(&state(), None).crew.len(), 0);
    }

    #[test]
    fn an_orchestrator_sees_its_own_part() {
        let s = scoped(&state(), Some("a"));
        assert_eq!(ids(&s.orchestrators, |o| &o.id), ["a"]);
        assert_eq!(s.orchestrators[0].notes, "a notes");
        assert_eq!(ids(&s.crew, |m| &m.chat_id), ["a1"]);
        assert_eq!(ids(&s.items, |i| &i.id), ["ia", "loose"]);
        assert_eq!(ids(&s.asks, |a| &a.id), ["q-a"]);
        assert!(s.dumps.is_empty());
    }

    #[test]
    fn a_crew_thread_sees_its_row_its_orchestrators_name_and_rules_and_its_item() {
        let s = scoped(&state(), Some("a1"));
        assert_eq!(ids(&s.orchestrators, |o| &o.id), ["a"]);
        assert_eq!(s.orchestrators[0].crew_rules, "a crew rules");
        assert!(s.orchestrators[0].rules.is_empty() && s.orchestrators[0].notes.is_empty());
        assert_eq!(ids(&s.crew, |m| &m.chat_id), ["a1"]);
        assert_eq!(ids(&s.items, |i| &i.id), ["ia"]);
        assert!(s.asks.is_empty() && s.dumps.is_empty());
    }

    #[test]
    fn the_brain_dump_sees_all_but_others_rules_and_notes() {
        let s = scoped(&state(), Some("brain"));
        assert_eq!(s.orchestrators.len(), 3);
        let a = s.orchestrators.iter().find(|o| o.id == "a").unwrap();
        assert_eq!(a.goal, "a goal");
        assert!(a.rules.is_empty() && a.notes.is_empty() && a.crew_rules.is_empty());
        let own = s.orchestrators.iter().find(|o| o.id == "brain").unwrap();
        assert_eq!(own.notes, "brain notes");
        assert_eq!(s.crew.len(), 2);
        assert_eq!(s.items.len(), 3);
        assert_eq!(s.dumps.len(), 1);
    }

    #[test]
    fn comms_show_an_agent_its_own_traffic() {
        let state = state();
        let event = |kind, from: &str, to: &str| CommEvent {
            id: "e".into(),
            kind,
            from: from.into(),
            to: to.into(),
            summary: String::new(),
            at: Utc::now(),
        };
        let report = event(CommKind::Report, "a1", "a");
        assert!(sees(&report, &state, Some("a")));
        assert!(sees(&report, &state, Some("a1")));
        assert!(!sees(&report, &state, Some("b")));
        assert!(!sees(&report, &state, Some("b1")));
        let crew_talk = event(CommKind::Message, "a1", "x");
        assert!(sees(&crew_talk, &state, Some("a")), "its crew's traffic");
        let route = event(CommKind::Route, "brain", "b");
        assert!(sees(&route, &state, Some("brain")));
        assert!(sees(&event(CommKind::Route, "y", "z"), &state, Some("brain")));
        assert!(!sees(&event(CommKind::Message, "y", "z"), &state, Some("brain")));
        assert!(!sees(&report, &state, None));
    }
}
