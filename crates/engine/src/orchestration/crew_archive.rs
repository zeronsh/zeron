//! Crew threads left idle are archived (the profile's
//! `OrchestrationSettings::crew_auto_archive`, after `crew_idle_hours`).
//!
//! Each engine sweeps the crew threads it hosts (it knows their turns): a
//! chat with `childKind` crew and a crew row, never an orchestrator's or the
//! Brain dump's own chat. A thread is idle from the latest of: its creation,
//! its last message in or out (`lastMessageAt`), its last turn's start and
//! end, its joining the crew, its last report, and its last unarchive. It is
//! kept while it works or waits on the person or on work of its own, while an
//! ask of its is open, and while its card is in Now or Blocked.
//!
//! Archiving goes through the person's own path (the chat row's `archived`),
//! so the Archived list and Unarchive work as ever. Its card keeps its
//! column; the thread leaves the card's `threads` and stays reachable as a
//! chat link. The sweep notes on the crew row when it first sees the chat
//! archived (by anyone) and when it next sees it unarchived (`revivedAt`):
//! an unarchive is activity, so a chat the person brought back is never
//! archived again until it has been idle a whole window since.

use chrono::{DateTime, Duration as TimeDelta, Utc};
use zeron_proto::orchestration::{
    AskState, CardStatus, CrewMember, LinkKind, OrchestrationState, Place, WorkLink, WorkState,
};
use zeron_proto::{Chat, ChildKind, Session, SessionStatus};

use super::Orchestration;

/// How long after a start the first sweep runs.
pub(crate) const FIRST_AFTER: std::time::Duration = std::time::Duration::from_secs(90);
/// How often the sweep runs after that.
pub(crate) const EVERY: std::time::Duration = std::time::Duration::from_secs(12 * 60);

/// What the sweep does to one chat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Step {
    /// Idle past the window: archive it (and note it on its crew row).
    Archive { chat_id: String, idle: TimeDelta },
    /// Archived (by someone): noted, so an unarchive can be told.
    SeenArchived { chat_id: String },
    /// Unarchived since it was seen archived: activity now.
    Revived { chat_id: String },
}

/// Why a crew thread idle past the window is kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kept {
    Working,
    AwaitingInput,
    OpenAsk,
    CardInPlay,
}

/// The latest activity of a crew thread.
pub(crate) fn last_activity(chat: &Chat, member: &CrewMember, session: Option<&Session>) -> DateTime<Utc> {
    let mut at = chat.created_at.max(member.joined_at);
    let mut take = |t: Option<DateTime<Utc>>| {
        if let Some(t) = t {
            at = at.max(t);
        }
    };
    take(chat.last_message_at);
    take(member.last_report.as_ref().map(|r| r.at));
    take(member.revived_at);
    if let Some(s) = session {
        take(s.started_at);
        take(s.last_turn_outcome.map(|o| o.finished_at));
        take(s.waiting.as_ref().map(|w| w.since));
    }
    at
}

/// Why `chat` is kept however long it's idle, if it is.
pub(crate) fn kept(
    chat: &Chat,
    member: &CrewMember,
    session: Option<&Session>,
    state: &OrchestrationState,
) -> Option<Kept> {
    if let Some(s) = session {
        match s.status {
            SessionStatus::Working => return Some(Kept::Working),
            SessionStatus::AwaitingInput => return Some(Kept::AwaitingInput),
            _ => {}
        }
        // Waiting on work of its own (a background task, a subagent).
        if s.waiting.is_some() {
            return Some(Kept::Working);
        }
    }
    if state.asks.iter().any(|a| a.state == AskState::Open && a.from_chat == chat.id) {
        return Some(Kept::OpenAsk);
    }
    // A card at work, paused, or waiting on the person's answer (which
    // comes back to its thread); not one that only waits to be looked at.
    // By its row; from before places, by its old column.
    let in_play = |i: &zeron_proto::orchestration::WorkItem| match state.cards.iter().find(|c| c.id == i.id) {
        Some(row) => {
            row.place == Place::Running
                || (matches!(row.place, Place::NeedsYou | Place::Snoozed) && row.status != CardStatus::Review)
                || i.paused()
        }
        None => matches!(i.state, WorkState::Active | WorkState::Blocked),
    };
    let on_card = state.items.iter().filter(|i| !i.removed).any(|i| {
        in_play(i) && (i.threads.contains(&chat.id) || member.item_id.as_deref() == Some(i.id.as_str()))
    });
    on_card.then_some(Kept::CardInPlay)
}

/// What the sweep does now, on the crew threads hosted on `here`.
pub(crate) fn plan(
    now: DateTime<Utc>,
    here: &str,
    chats: &[Chat],
    state: &OrchestrationState,
    session: impl Fn(&str) -> Option<Session>,
) -> Vec<Step> {
    let settings = &state.settings;
    let window = settings.crew_idle();
    let mut steps = Vec::new();
    for chat in chats {
        if chat.device_id != here || chat.child_kind != Some(ChildKind::Crew) {
            continue;
        }
        // Never an orchestrator's or the Brain dump's own chat.
        if state.orchestrators.iter().any(|o| o.id == chat.id) {
            continue;
        }
        let Some(member) = state.crew.iter().find(|m| m.chat_id == chat.id) else {
            continue;
        };
        if chat.archived {
            if member.archived_seen_at.is_none() {
                steps.push(Step::SeenArchived { chat_id: chat.id.clone() });
            }
            continue;
        }
        if member.archived_seen_at.is_some() {
            steps.push(Step::Revived { chat_id: chat.id.clone() });
            continue;
        }
        if !settings.crew_auto_archive {
            continue;
        }
        let live = session(&chat.id);
        let idle = now - last_activity(chat, member, live.as_ref());
        if idle < window || kept(chat, member, live.as_ref(), state).is_some() {
            continue;
        }
        steps.push(Step::Archive { chat_id: chat.id.clone(), idle });
    }
    steps
}

/// Do `steps`. How many chats it archived.
pub(crate) fn apply(orchestration: &Orchestration, steps: &[Step], now: DateTime<Utc>) -> usize {
    let workspace = orchestration.workspace();
    let mut archived = 0;
    for step in steps {
        match step {
            Step::Archive { chat_id, idle } => {
                let title = workspace.chat(chat_id).ok().flatten().and_then(|c| c.title);
                match workspace.set_chat_archived(chat_id, true) {
                    Ok(true) => {}
                    Ok(false) => continue,
                    Err(err) => {
                        tracing::warn!(chat = %chat_id, error = %err, "crew auto-archive: archiving failed");
                        continue;
                    }
                }
                archived += 1;
                let cards = workspace.write_orchestration(|doc| {
                    if let Some(mut member) = doc.crew_member(chat_id) {
                        member.archived_seen_at = Some(now);
                        doc.upsert_crew_member(&member);
                    }
                    unlink_cards(doc, chat_id, title.as_deref(), now)
                });
                tracing::info!(
                    chat = %chat_id,
                    reason = "idle",
                    idle_hours = idle.num_minutes() as f64 / 60.0,
                    cards = ?cards,
                    "crew auto-archive: archived an idle crew thread"
                );
            }
            Step::SeenArchived { chat_id } => workspace.write_orchestration(|doc| {
                if let Some(mut member) = doc.crew_member(chat_id).filter(|m| m.archived_seen_at.is_none()) {
                    member.archived_seen_at = Some(now);
                    doc.upsert_crew_member(&member);
                }
            }),
            Step::Revived { chat_id } => workspace.write_orchestration(|doc| {
                if let Some(mut member) = doc.crew_member(chat_id).filter(|m| m.archived_seen_at.is_some()) {
                    member.archived_seen_at = None;
                    member.revived_at = Some(now);
                    doc.upsert_crew_member(&member);
                    tracing::info!(chat = %chat_id, "crew auto-archive: unarchived; its idle time starts again");
                }
            }),
        }
    }
    archived
}

/// The cards `chat_id` worked on, its archive done: it leaves their
/// `threads` and stays on each as a chat link. Their columns are untouched.
/// The cards changed.
pub(crate) fn unlink_cards(
    doc: &mut zeron_doc::RegistryDoc,
    chat_id: &str,
    title: Option<&str>,
    now: DateTime<Utc>,
) -> Vec<String> {
    let mut changed = Vec::new();
    for mut item in doc.read_work_items().into_iter().filter(|i| !i.removed) {
        if !item.threads.iter().any(|t| t == chat_id) {
            continue;
        }
        item.threads.retain(|t| t != chat_id);
        if !item.links.iter().any(|l| l.kind == LinkKind::Chat && l.target == chat_id) {
            let label = title.map(str::trim).filter(|t| !t.is_empty()).unwrap_or("Archived thread");
            item.links.push(WorkLink { kind: LinkKind::Chat, label: label.to_owned(), target: chat_id.to_owned() });
        }
        item.updated_at = now;
        doc.upsert_work_item(&item);
        changed.push(item.id.clone());
    }
    changed
}

/// One sweep, on what this engine hosts. How many chats it archived.
pub(crate) fn sweep(orchestration: &Orchestration, rpc: &crate::rpc::EngineRpc) -> usize {
    let chats = match orchestration.workspace().read_chats() {
        Ok(chats) => chats,
        Err(err) => {
            tracing::warn!(error = %err, "crew auto-archive: chats unreadable");
            return 0;
        }
    };
    let state = orchestration.workspace().orchestration_state();
    let now = Utc::now();
    let steps = plan(now, orchestration.device_id(), &chats, &state, |id| rpc.sessions_ref().session_status(id));
    apply(orchestration, &steps, now)
}

/// The sweep, once shortly after a start and then every [`EVERY`].
pub(crate) fn start(orchestration: Orchestration) {
    let run = orchestration.clone();
    run.spawn(async move {
        tokio::time::sleep(FIRST_AFTER).await;
        let mut tick = tokio::time::interval(EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            if let Some(rpc) = orchestration.rpc() {
                let perf_t = std::time::Instant::now();
                let archived = sweep(&orchestration, &rpc);
                tracing::debug!(target: "perf", what = "archive_sweep", us = perf_t.elapsed().as_micros() as u64);
                if archived > 0 {
                    tracing::info!(archived, "crew auto-archive: sweep done");
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use zeron_proto::orchestration::{Ask, OrchestrationSettings, Orchestrator, WorkItem};

    const HERE: &str = "here";

    fn t0() -> DateTime<Utc> {
        "2026-11-13T00:00:00Z".parse().unwrap()
    }

    fn hours(h: i64) -> DateTime<Utc> {
        t0() + TimeDelta::hours(h)
    }

    fn chat(id: &str, kind: Option<&str>, archived: bool) -> Chat {
        serde_json::from_value(json!({
            "id": id, "deviceId": HERE, "title": id, "archived": archived, "cwd": null, "branch": null,
            "config": null, "lastMessagePreview": null, "lastMessageAt": t0(), "createdAt": t0(),
            "parentChatId": "orc", "childKind": kind,
        }))
        .unwrap()
    }

    fn member(chat: &str) -> CrewMember {
        serde_json::from_value(json!({ "chatId": chat, "orchestratorId": "orc", "joinedAt": t0() })).unwrap()
    }

    fn orchestrator(id: &str, kind: &str) -> Orchestrator {
        serde_json::from_value(json!({ "id": id, "kind": kind, "name": id, "hue": 10,
            "createdAt": t0(), "updatedAt": t0() }))
        .unwrap()
    }

    fn state(crew: Vec<CrewMember>) -> OrchestrationState {
        OrchestrationState {
            orchestrators: vec![orchestrator("orc", "standard"), orchestrator("brain", "brainDump")],
            crew,
            // (A 12-hour window the person chose: what these tests time.)
            settings: OrchestrationSettings { crew_auto_archive: true, crew_idle_hours: 12, crew_idle_hours_set: true },
            cards: Vec::new(),
            ..Default::default()
        }
    }

    fn session(chat: &str, status: &str, extra: serde_json::Value) -> Session {
        let mut v = json!({ "chatId": chat, "deviceId": HERE, "status": status, "updatedAt": t0(),
            "startedAt": null });
        if let (Some(v), Some(extra)) = (v.as_object_mut(), extra.as_object()) {
            v.extend(extra.clone());
        }
        serde_json::from_value(v).unwrap()
    }

    fn none(_: &str) -> Option<Session> {
        None
    }

    fn archived_ids(steps: &[Step]) -> Vec<String> {
        steps
            .iter()
            .filter_map(|s| match s {
                Step::Archive { chat_id, .. } => Some(chat_id.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_crew_thread_idle_past_the_window_is_archived_not_before() {
        let chats = [chat("c", Some("crew"), false)];
        let st = state(vec![member("c")]);
        assert!(plan(hours(11), HERE, &chats, &st, none).is_empty(), "11h: not yet");
        let steps = plan(hours(12), HERE, &chats, &st, none);
        assert_eq!(steps, vec![Step::Archive { chat_id: "c".into(), idle: TimeDelta::hours(12) }]);
    }

    #[test]
    fn activity_starts_the_timer_again() {
        let st = state(vec![member("c")]);
        // A message at 10h.
        let mut c = chat("c", Some("crew"), false);
        c.last_message_at = Some(hours(10));
        assert!(archived_ids(&plan(hours(20), HERE, &[c.clone()], &st, none)).is_empty());
        assert_eq!(archived_ids(&plan(hours(22), HERE, &[c], &st, none)), ["c"]);
        // A turn that ended at 15h (no message row moved).
        let c = chat("c", Some("crew"), false);
        let ended = |_: &str| Some(session("c", "idle", json!({ "lastTurnOutcome": { "status": "completed", "finishedAt": hours(15) } })));
        assert!(archived_ids(&plan(hours(26), HERE, &[c.clone()], &st, ended)).is_empty());
        assert_eq!(archived_ids(&plan(hours(27), HERE, &[c.clone()], &st, ended)), ["c"]);
        // A report at 20h.
        let mut m = member("c");
        m.last_report = Some(serde_json::from_value(json!({ "id": "r", "state": "progress", "summary": "s", "at": hours(20) })).unwrap());
        assert!(archived_ids(&plan(hours(31), HERE, &[c], &state(vec![m]), none)).is_empty());
    }

    #[test]
    fn orchestrators_the_brain_dump_and_other_chats_are_never_archived() {
        let chats = [
            chat("orc", Some("crew"), false),
            chat("brain", Some("crew"), false),
            chat("side", Some("side"), false),
            chat("sub", Some("subagent"), false),
            chat("mine", None, false),
        ];
        let crew = chats.iter().map(|c| member(&c.id)).collect();
        assert!(plan(hours(1000), HERE, &chats, &state(crew), none).is_empty());
    }

    #[test]
    fn a_thread_hosted_elsewhere_or_with_no_crew_row_is_left_alone() {
        let mut there = chat("there", Some("crew"), false);
        there.device_id = "elsewhere".into();
        let loose = chat("loose", Some("crew"), false);
        assert!(plan(hours(1000), HERE, &[there, loose], &state(vec![member("there")]), none).is_empty());
    }

    #[test]
    fn working_waiting_asking_or_on_a_card_in_play_keeps_it() {
        let chats = [chat("c", Some("crew"), false)];
        let late = hours(100);
        for (status, extra) in [
            ("working", json!({})),
            ("awaitingInput", json!({})),
            ("idle", json!({ "waiting": { "since": t0(), "on": [] } })),
        ] {
            let live = |_: &str| Some(session("c", status, extra.clone()));
            assert!(plan(late, HERE, &chats, &state(vec![member("c")]), live).is_empty(), "{status}");
        }
        // An open ask of its; answered, it no longer holds it.
        let mut ask: Ask = serde_json::from_value(json!({ "id": "a", "fromChat": "c", "kind": "question",
            "question": "q", "state": "open", "createdAt": t0() }))
        .unwrap();
        let mut st = state(vec![member("c")]);
        st.asks = vec![ask.clone()];
        assert!(plan(late, HERE, &chats, &st, none).is_empty(), "open ask");
        ask.state = AskState::Answered;
        st.asks = vec![ask];
        assert_eq!(archived_ids(&plan(late, HERE, &chats, &st, none)), ["c"]);
        // Its card in Now or Blocked keeps it; in Review or Done, not.
        for (column, held) in [("active", true), ("blocked", true), ("review", false), ("done", false), ("planned", false)] {
            let item: WorkItem = serde_json::from_value(json!({ "id": "i", "title": "t", "state": column,
                "priority": "normal", "threads": ["c"], "createdAt": t0(), "updatedAt": t0() }))
            .unwrap();
            let mut st = state(vec![member("c")]);
            st.items = vec![item];
            assert_eq!(plan(late, HERE, &chats, &st, none).is_empty(), held, "{column}");
        }
        // Its card by its crew row (not yet among the card's threads).
        let item: WorkItem = serde_json::from_value(json!({ "id": "i", "title": "t", "state": "active",
            "priority": "normal", "createdAt": t0(), "updatedAt": t0() }))
        .unwrap();
        let mut m = member("c");
        m.item_id = Some("i".into());
        let mut st = state(vec![m]);
        st.items = vec![item];
        assert!(plan(late, HERE, &chats, &st, none).is_empty(), "its crew row's card");
    }

    #[test]
    fn the_setting_off_archives_nothing_and_its_window_is_followed() {
        let chats = [chat("c", Some("crew"), false)];
        let mut st = state(vec![member("c")]);
        st.settings = OrchestrationSettings { crew_auto_archive: false, crew_idle_hours: 12, crew_idle_hours_set: true };
        assert!(plan(hours(1000), HERE, &chats, &st, none).is_empty());
        st.settings = OrchestrationSettings { crew_auto_archive: true, crew_idle_hours: 1, crew_idle_hours_set: true };
        assert_eq!(archived_ids(&plan(hours(1), HERE, &chats, &st, none)), ["c"]);
        st.settings.crew_idle_hours = 72;
        assert!(plan(hours(71), HERE, &chats, &st, none).is_empty());
        assert_eq!(archived_ids(&plan(hours(72), HERE, &chats, &st, none)), ["c"]);
        // Out of bounds: held inside them (0 → 1 hour).
        st.settings.crew_idle_hours = 0;
        assert!(plan(t0() + TimeDelta::minutes(59), HERE, &chats, &st, none).is_empty());
    }

    #[test]
    fn an_archive_is_noted_and_an_unarchive_starts_the_timer_again() {
        // Archived (by the person or the sweep): noted once.
        let archived = [chat("c", Some("crew"), true)];
        assert_eq!(plan(hours(1), HERE, &archived, &state(vec![member("c")]), none), vec![Step::SeenArchived { chat_id: "c".into() }]);
        let mut seen = member("c");
        seen.archived_seen_at = Some(hours(1));
        assert!(plan(hours(2), HERE, &archived, &state(vec![seen.clone()]), none).is_empty(), "noted once");
        // Unarchived: revived, not archived again in that sweep.
        let back = [chat("c", Some("crew"), false)];
        assert_eq!(plan(hours(50), HERE, &back, &state(vec![seen]), none), vec![Step::Revived { chat_id: "c".into() }]);
        // Revived at 50h: idle from then.
        let mut revived = member("c");
        revived.revived_at = Some(hours(50));
        assert!(plan(hours(61), HERE, &back, &state(vec![revived.clone()]), none).is_empty());
        assert_eq!(archived_ids(&plan(hours(62), HERE, &back, &state(vec![revived]), none)), ["c"]);
    }

    #[test]
    fn the_default_window_is_four_hours() {
        let chats = [chat("c", Some("crew"), false)];
        let mut st = state(vec![member("c")]);
        st.settings = OrchestrationSettings::default();
        assert_eq!(st.settings.crew_idle_hours, 4);
        assert!(plan(t0() + TimeDelta::minutes(239), HERE, &chats, &st, none).is_empty());
        assert_eq!(archived_ids(&plan(hours(4), HERE, &chats, &st, none)), ["c"]);
    }

    #[test]
    fn an_untouched_twelve_reads_as_four_and_a_chosen_twelve_stands() {
        // The old default, written with another setting, never chosen.
        let untouched: OrchestrationSettings =
            serde_json::from_value(json!({ "crewAutoArchive": true, "crewIdleHours": 12 })).unwrap();
        assert_eq!(untouched.migrated().crew_idle_hours, 4);
        // Chosen by the person: kept.
        let chosen: OrchestrationSettings =
            serde_json::from_value(json!({ "crewAutoArchive": true, "crewIdleHours": 12, "crewIdleHoursSet": true })).unwrap();
        assert_eq!(chosen.migrated().crew_idle_hours, 12);
        // Another window, set or not: kept.
        let other: OrchestrationSettings =
            serde_json::from_value(json!({ "crewAutoArchive": false, "crewIdleHours": 24 })).unwrap();
        assert_eq!((other.clone().migrated().crew_idle_hours, other.migrated().crew_auto_archive), (24, false));
        // No row at all: the default.
        let none_yet: OrchestrationSettings = serde_json::from_value(json!({})).unwrap();
        assert_eq!(none_yet.migrated().crew_idle_hours, 4);
    }
}
