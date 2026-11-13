//! A dump the Brain dump left `routing`: its turn ended (or the engine came
//! back) without every fragment routed. What has no place yet becomes an
//! `unsure` route, with the likeliest orchestrators as candidates, and the
//! dump needs the person. Nothing is ever deleted.

use chrono::Utc;
use zeron_doc::SessionCommandPayload;
use zeron_proto::orchestration::{
    Dump, DumpState, OrchestratorKind, Route, RouteIntent, RouteState, RouteTarget,
};

use super::Orchestration;
use crate::rpc::EngineRpc;

/// The most candidates an unsure fragment offers.
const CANDIDATES_MAX: usize = 5;

/// The parts of `text` no route covers that say something (byte spans,
/// trimmed). `None` when a route paraphrased (no span), so what it covers
/// can't be told: then the dump counts as accounted for.
pub(crate) fn unrouted(text: &str, routes: &[Route]) -> Option<Vec<(usize, usize)>> {
    let routed: Vec<&Route> = routes.iter().filter(|r| r.state != RouteState::Undone).collect();
    if routed.iter().any(|r| r.span.is_none()) {
        return None;
    }
    let mut covered = vec![false; text.len()];
    for r in &routed {
        let (from, to) = r.span.unwrap_or_default();
        let (from, to) = (from as usize, (to as usize).min(text.len()));
        for c in covered.iter_mut().take(to).skip(from) {
            *c = true;
        }
    }
    let mut out = Vec::new();
    let mut start = None;
    for i in 0..=text.len() {
        let free = i < text.len() && !covered[i];
        match (free, start) {
            (true, None) => start = Some(i),
            (false, Some(from)) => {
                start = None;
                // On char boundaries, trimmed, and saying something.
                let (mut from, mut to) = (from, i);
                while !text.is_char_boundary(from) {
                    from += 1;
                }
                while to > from && !text.is_char_boundary(to) {
                    to -= 1;
                }
                let piece = &text[from..to];
                let lead = piece.len() - piece.trim_start().len();
                let trail = piece.len() - piece.trim_end().len();
                let (from, to) = (from + lead, to - trail);
                if from < to && text[from..to].chars().any(char::is_alphanumeric) {
                    out.push((from, to));
                }
            }
            _ => {}
        }
    }
    Some(out)
}

/// `dump` settled: an `unsure` route (with `candidates`) for each fragment
/// with no place, and its state from its routes. `None` when it isn't
/// `routing` any more.
pub(crate) fn settle(dump: &Dump, candidates: &[RouteTarget]) -> Option<Dump> {
    if dump.state != DumpState::Routing {
        return None;
    }
    let mut dump = dump.clone();
    let gaps = match unrouted(&dump.text, &dump.routes) {
        Some(gaps) => gaps,
        None => Vec::new(),
    };
    for (from, to) in gaps {
        dump.routes.push(Route {
            id: uuid::Uuid::new_v4().simple().to_string(),
            excerpt: dump.text[from..to].to_owned(),
            span: Some((from as u32, to as u32)),
            target: RouteTarget::Unsure,
            intent: RouteIntent::Task,
            message: String::new(),
            title: None,
            state: RouteState::Pending,
            candidates: candidates.to_vec(),
            answer: None,
            answer_full: None,
            answered_at: None,
            item_id: None,
            space_id: None,
            device_id: None,
            attachments: Default::default(),
            delivered_at: Default::default(),
        });
    }
    dump.state = super::brain::settled_state(&dump);
    dump.updated_at = Utc::now();
    Some(dump)
}

/// The likeliest places for a fragment with none: the orchestrators,
/// most recently active first.
fn candidates(orchestration: &Orchestration) -> Vec<RouteTarget> {
    let mut orchestrators: Vec<_> = orchestration
        .state()
        .orchestrators
        .into_iter()
        .filter(|o| o.kind == OrchestratorKind::Standard && !o.archived)
        .collect();
    orchestrators.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    orchestrators
        .into_iter()
        .take(CANDIDATES_MAX)
        .map(|o| RouteTarget::Orchestrator { id: o.id })
        .collect()
}

/// The marker a dump's message to the Brain dump carries.
fn marker(dump_id: &str) -> String {
    format!("[Dump {dump_id}]")
}

/// Settle `dump_id` if still routing. Whether it changed.
fn settle_now(orchestration: &Orchestration, rpc: &EngineRpc, dump_id: &str) -> bool {
    let candidates = rpc.existing_candidates(candidates(orchestration));
    let Some(dump) = orchestration.workspace().read_orchestration_with(|doc| doc.dump(dump_id)) else {
        return false;
    };
    let Some(mut settled) = settle(&dump, &candidates) else { return false };
    // Each part with no place asks the person where it goes.
    let brain = rpc.brain_of(&dump);
    let fresh: Vec<String> = settled.routes[dump.routes.len()..].iter().map(|r| r.id.clone()).collect();
    for route_id in fresh {
        let snapshot = settled.clone();
        if let Some(route) = settled.routes.iter_mut().find(|r| r.id == route_id) {
            rpc.ask_where(&snapshot, route, brain.as_deref());
        }
    }
    orchestration.workspace().write_orchestration(|doc| {
        // Routed meanwhile: leave it be.
        if doc.dump(dump_id).is_some_and(|d| d.state == DumpState::Routing) {
            doc.upsert_dump(&settled);
        }
    });
    true
}

/// The Brain dump `brain`'s turn ended: every dump it was given and has
/// answered but left routing is settled. The dumps settled.
pub(crate) fn after_turn(orchestration: &Orchestration, rpc: &EngineRpc, brain: &str) -> Vec<String> {
    if !matches!(orchestration.role_of(brain), super::ChatRole::BrainDump { .. }) {
        return Vec::new();
    }
    let routing: Vec<Dump> = orchestration
        .state()
        .dumps
        .into_iter()
        .filter(|d| d.state == DumpState::Routing)
        .collect();
    if routing.is_empty() {
        return Vec::new();
    }
    let Ok(handle) = rpc.doc_host_ref().open(brain) else { return Vec::new() };
    let Ok(entries) = handle.doc().read_entries() else { return Vec::new() };
    routing
        .iter()
        .filter(|d| super::watchers::reply_to(&entries, &marker(&d.id)).is_some())
        .filter(|d| settle_now(orchestration, rpc, &d.id))
        .map(|d| d.id.clone())
        .collect()
}

/// After a start: every dump still routing whose Brain dump is hosted here
/// and idle, and that isn't still on its way to it (queued or a pending
/// command), is settled. The dumps settled.
pub(crate) fn sweep(orchestration: &Orchestration, rpc: &EngineRpc) -> Vec<String> {
    let device = orchestration.device_id().to_owned();
    let state = orchestration.state();
    let chats = orchestration.workspace().read_chats().unwrap_or_default();
    let brains: Vec<String> = state
        .orchestrators
        .iter()
        .filter(|o| o.kind == OrchestratorKind::BrainDump)
        .filter(|o| chats.iter().any(|c| c.id == o.id && c.device_id == device))
        .map(|o| o.id.clone())
        .collect();
    let mut settled = Vec::new();
    for brain in brains {
        if rpc.sessions_ref().turn_in_flight(&brain) {
            continue;
        }
        let Ok(handle) = rpc.doc_host_ref().open(&brain) else { continue };
        let queued: Vec<String> = handle
            .doc()
            .read_queue()
            .unwrap_or_default()
            .into_iter()
            .map(|q| q.text)
            .chain(handle.doc().read_commands().unwrap_or_default().into_iter().filter_map(|c| {
                if c.status != zeron_doc::SessionCommandStatus::Pending {
                    return None;
                }
                match c.payload {
                    SessionCommandPayload::Run { request, .. } => Some(request.prompt),
                    SessionCommandPayload::Steer { prompt, .. } => Some(prompt),
                    _ => None,
                }
            }))
            .collect();
        for dump in state.dumps.iter().filter(|d| d.state == DumpState::Routing) {
            let marker = marker(&dump.id);
            if queued.iter().any(|q| q.contains(&marker)) {
                continue; // still on its way
            }
            if settle_now(orchestration, rpc, &dump.id) {
                settled.push(dump.id.clone());
            }
        }
    }
    settled
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(text: &str, excerpt: &str) -> Route {
        Route {
            id: excerpt.into(),
            excerpt: excerpt.into(),
            span: super::super::brain::span_of(text, excerpt),
            target: RouteTarget::Chat { id: "c".into() },
            intent: RouteIntent::Task,
            message: String::new(),
            title: None,
            state: RouteState::Delivered,
            candidates: vec![],
            answer: None,
            answer_full: None,
            answered_at: None,
            item_id: None,
            space_id: None,
            device_id: None,
            attachments: Default::default(),
            delivered_at: Default::default(),
        }
    }

    fn dump(text: &str, routes: Vec<Route>) -> Dump {
        Dump {
            id: "d".into(),
            text: text.into(),
            state: DumpState::Routing,
            routes,
            summary: None,
            device_id: "dev".into(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            attachments: Default::default(),
        }
    }

    #[test]
    fn what_no_route_covers_is_found_and_paraphrase_counts_as_covered() {
        let text = "fix auth. — then ship 2.0 friday.";
        assert_eq!(unrouted(text, &[]).unwrap(), vec![(0, text.len())]);
        let gaps = unrouted(text, &[route(text, "fix auth.")]).unwrap();
        assert_eq!(gaps.iter().map(|(a, b)| &text[*a..*b]).collect::<Vec<_>>(), ["— then ship 2.0 friday."]);
        assert!(unrouted(text, &[route(text, "fix auth."), route(text, "— then ship 2.0 friday.")]).unwrap().is_empty());
        let mut paraphrase = route(text, "nowhere");
        paraphrase.span = None;
        assert_eq!(unrouted(text, &[paraphrase]), None);
    }

    /// A Brain dump hosted here, idle, with a dump from before a restart
    /// still routing and nothing on its way: the sweep settles it; one still
    /// queued for it is left alone.
    #[tokio::test]
    async fn the_start_sweep_settles_what_an_idle_brain_dump_left_routing() {
        use zeron_proto::HarnessId;
        let temp = tempfile::tempdir().unwrap();
        let core = crate::EngineCore::assemble_with_profile(
            crate::EngineProfile::local(temp.path()).unwrap(),
            std::sync::Arc::new(crate::registry::HarnessRegistry::new()),
            HarnessId::Codex,
            None,
        )
        .unwrap();
        let rpc = core.rpc_service();
        let brain = rpc.ensure_brain_dump().unwrap();
        let mut stale = dump("old thought", vec![]);
        stale.id = "stale".into();
        let mut waiting = dump("new thought", vec![]);
        waiting.id = "waiting".into();
        core.workspace.write_orchestration(|doc| {
            doc.upsert_dump(&stale);
            doc.upsert_dump(&waiting);
        });
        core.doc_host
            .queue_message(&brain, &zeron_proto::orchestration::prompt_text::dump_message("waiting", "new thought"), Vec::new())
            .ok();
        // (Its own engine would run that queued row; this one has no harness
        // to, so it stays queued.)
        let settled = sweep(&core.orchestration, &rpc);
        assert_eq!(settled, ["stale"]);
        let read = |id: &str| core.workspace.read_orchestration_with(|d| d.dump(id)).unwrap();
        assert_eq!(read("stale").state, DumpState::NeedsYou);
        assert_eq!(read("stale").routes[0].target, RouteTarget::Unsure);
        assert_eq!(read("waiting").state, DumpState::Routing);
    }

    #[test]
    fn a_dump_left_routing_needs_the_person_and_keeps_its_routes() {
        let candidates = vec![RouteTarget::Orchestrator { id: "o".into() }];
        let text = "add a -l alias";
        let settled = settle(&dump(text, vec![]), &candidates).unwrap();
        assert_eq!(settled.state, DumpState::NeedsYou);
        assert_eq!(settled.routes.len(), 1);
        assert_eq!(settled.routes[0].target, RouteTarget::Unsure);
        assert_eq!(settled.routes[0].excerpt, text);
        assert_eq!(settled.routes[0].candidates, candidates);
        // Fully routed but never finished: routed, nothing added.
        let done = settle(&dump(text, vec![route(text, text)]), &candidates).unwrap();
        assert_eq!(done.state, DumpState::Routed);
        assert_eq!(done.routes.len(), 1);
        // Not routing any more: left alone.
        let mut routed = dump(text, vec![]);
        routed.state = DumpState::Routed;
        assert!(settle(&routed, &candidates).is_none());
    }
}
