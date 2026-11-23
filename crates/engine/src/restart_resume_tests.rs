//! Which chats a restart resumes ([`to_resume`]) and what they are told.

use std::collections::HashMap;

use chrono::{DateTime, Duration as TimeDelta, Utc};
use zeron_proto::orchestration::{OrchestrationState, Priority, WorkItem, WorkState};
use zeron_proto::{Chat, DoneStatus, NativeTurnOutcome, Session, SessionStatus};

use super::*;

fn t0() -> DateTime<Utc> {
    "2026-11-16T12:00:00Z".parse().unwrap()
}

fn mins(n: i64) -> DateTime<Utc> {
    t0() + TimeDelta::minutes(n)
}

fn card(state: WorkState) -> WorkItem {
    WorkItem {
        id: "c1".into(),
        orchestrator_id: Some("o1".into()),
        title: "Phase 1: ship".into(),
        detail: String::new(),
        state,
        priority: Priority::Normal,
        progress: None,
        threads: vec!["t1".into()],
        links: vec![],
        outcome: None,
        source: None,
        order: 1.0,
        created_at: t0(),
        updated_at: t0(),
        done_at: None,
        removed: false,
        removed_at: None,
        space_id: None,
        device_id: None,
        waiting: None,
        moved_at: Some(t0()),
        moved_route_state: None,
        approved_at: None,
        paused_at: None,
        check: None,
        threads_at: None,
        flow: None,
        depends_on: Vec::new(),
    }
}

fn chat(id: &str, device: &str) -> Chat {
    serde_json::from_value(serde_json::json!({
        "id": id, "deviceId": device, "createdAt": t0(), "updatedAt": t0(), "lastMessageAt": t0(), "archived": false,
    }))
    .unwrap()
}

fn session(chat: &str, status: SessionStatus) -> Session {
    Session {
        last_completed_turn: Some("t".into()),
        chat_id: chat.into(),
        device_id: "d1".into(),
        status,
        started_at: Some(mins(1)),
        updated_at: mins(1),
        last_turn_outcome: None,
        terminal_activity: None,
        running_subagents: 0,
        background_tasks: vec![],
        waiting: None,
        live: Default::default(),
    }
}

fn crew_state(cards: Vec<WorkItem>) -> OrchestrationState {
    let mut state = OrchestrationState::default();
    state.crew = vec![serde_json::from_value(serde_json::json!({
        "chatId": "t1", "orchestratorId": "o1", "role": "", "joinedAt": t0(),
    }))
    .unwrap()];
    state.items = cards;
    state
}

fn limited(chat: &str, since: DateTime<Utc>) -> Session {
    Session {
        status: SessionStatus::Errored,
        started_at: None,
        updated_at: since,
        last_turn_outcome: Some(NativeTurnOutcome { status: DoneStatus::Errored, finished_at: since }),
        live: zeron_proto::SessionLive { limited: Some(zeron_proto::usage_limit::SessionLimit {
            provider: "claude".into(),
            account_slot: None,
            account_label: None,
            window: Some("5-hour".into()),
            until: Some(since + TimeDelta::hours(2)),
            since,
            attempt: 0,
            switched_to: None,
        }), ..Default::default() },
        ..session(chat, SessionStatus::Errored)
    }
}

#[test]
fn a_crew_thread_at_work_when_the_engine_stopped_is_resumed() {
    let chats: HashMap<String, Chat> = [chat("t1", "here")].into_iter().map(|c| (c.id.clone(), c)).collect();
    let state = crew_state(vec![card(WorkState::Active)]);
    let at = mins(30);
    let none = HashMap::new();
    let idle_now: HashMap<String, Session> = [("t1".to_owned(), session("t1", SessionStatus::Idle))].into();
    // Mid-turn, or waiting on its own work, or running subagents: resumed.
    let working = session("t1", SessionStatus::Working);
    assert_eq!(ids(to_resume(&[working.clone()], &[], &[], &none, &state, &chats, at)), ["t1"]);
    assert_eq!(ids(to_resume(&[working.clone()], &[], &[], &idle_now, &state, &chats, at)), ["t1"], "its row set idle since");
    let stale_working: HashMap<String, Session> = [("t1".to_owned(), working.clone())].into();
    assert_eq!(ids(to_resume(&[working.clone()], &[], &[], &stale_working, &state, &chats, at)), ["t1"], "the old row, no heartbeat");
    let mut waiting = session("t1", SessionStatus::Idle);
    waiting.waiting = Some(serde_json::from_value(serde_json::json!({"since": mins(1), "on": []})).unwrap());
    assert_eq!(ids(to_resume(&[waiting], &[], &[], &none, &state, &chats, at)), ["t1"]);
    let mut subagents = session("t1", SessionStatus::Idle);
    subagents.running_subagents = 2;
    assert_eq!(ids(to_resume(&[subagents], &[], &[], &none, &state, &chats, at)), ["t1"]);
    // Its last turn ended normally, or it waits on the person: nothing to pick up.
    assert!(to_resume(&[session("t1", SessionStatus::Idle)], &[], &[], &none, &state, &chats, at).is_empty());
    assert!(to_resume(&[session("t1", SessionStatus::AwaitingInput)], &[], &[], &none, &state, &chats, at).is_empty());
}

#[test]
fn a_thread_already_picked_up_paused_or_not_crew_is_not_resumed() {
    let chats: HashMap<String, Chat> = [chat("t1", "here")].into_iter().map(|c| (c.id.clone(), c)).collect();
    let working = session("t1", SessionStatus::Working);
    let at = mins(30);
    let state = crew_state(vec![card(WorkState::Active)]);
    // Revived by the engine's own recovery: in flight now, or a turn done since.
    let mut live = session("t1", SessionStatus::Working);
    live.updated_at = at;
    let now: HashMap<String, Session> = [("t1".to_owned(), live)].into();
    assert!(ids(to_resume(&[working.clone()], &[], &[], &now, &state, &chats, at)).is_empty());
    let mut later = session("t1", SessionStatus::Idle);
    later.last_completed_turn = Some("t2".into());
    let now: HashMap<String, Session> = [("t1".to_owned(), later)].into();
    assert!(ids(to_resume(&[working.clone()], &[], &[], &now, &state, &chats, at)).is_empty());
    // Its card paused by the person.
    let mut paused = card(WorkState::Blocked);
    paused.paused_at = Some(mins(1));
    assert!(to_resume(&[working.clone()], &[], &[], &HashMap::new(), &crew_state(vec![paused]), &chats, at).is_empty());
    // Not in a crew, or archived.
    assert!(to_resume(&[working.clone()], &[], &[], &HashMap::new(), &OrchestrationState::default(), &chats, at).is_empty());
    let mut archived = chat("t1", "here");
    archived.archived = true;
    let gone: HashMap<String, Chat> = [("t1".to_owned(), archived)].into();
    assert!(to_resume(&[working], &[], &[], &HashMap::new(), &state, &gone, at).is_empty());
}

fn ids(list: Vec<Resume>) -> Vec<String> {
    list.into_iter().map(|r| r.chat).collect()
}

#[test]
fn a_chat_whose_wait_the_shutdown_ended_is_resumed_crew_or_not() {
    // Its turn completed into a wait on its own build; the shutdown ended
    // the build and the wait, so its row by the next boot says Idle.
    let chats: HashMap<String, Chat> =
        [chat("t1", "here"), chat("solo", "here")].into_iter().map(|c| (c.id.clone(), c)).collect();
    let state = crew_state(vec![card(WorkState::Active)]);
    let at = mins(30);
    let rows = [session("t1", SessionStatus::Idle), session("solo", SessionStatus::Idle)];
    assert!(to_resume(&rows, &[], &[], &HashMap::new(), &state, &chats, at).is_empty(), "ended normally, as its row reads");
    let waits = vec![
        ("solo".to_owned(), vec!["cargo build -p zeron-engine".to_owned()]),
        ("t1".to_owned(), vec!["deploy build".to_owned()]),
    ];
    let out = to_resume(&rows, &waits, &[], &HashMap::new(), &state, &chats, at);
    assert_eq!(
        out,
        [
            Resume { chat: "solo".into(), waited_on: vec!["cargo build -p zeron-engine".into()], subagents: None },
            Resume { chat: "t1".into(), waited_on: vec!["deploy build".into()], subagents: None },
        ],
        "a standalone chat too"
    );
    // A standalone chat that was mid-turn is the engine's own recovery's.
    let mut mid = session("solo", SessionStatus::Working);
    mid.last_completed_turn = None;
    assert!(to_resume(&[mid], &[], &[], &HashMap::new(), &state, &chats, at).is_empty());
    // Ran since (picked up by something else): not again.
    let mut ran = session("solo", SessionStatus::Idle);
    ran.last_completed_turn = Some("t9".into());
    let now: HashMap<String, Session> = [("solo".to_owned(), ran)].into();
    assert!(ids(to_resume(&rows, &waits, &[], &now, &state, &chats, at)) == ["t1"]);
}

#[test]
fn the_resume_says_what_the_restart_stopped() {
    use zeron_proto::orchestration::prompt_text::{RESUMED_AFTER_RESTART, resumed_after_restart};
    assert_eq!(resumed_after_restart(&[]), RESUMED_AFTER_RESTART);
    let text = resumed_after_restart(&["cargo build -p zeron-engine".into()]);
    assert!(text.starts_with(RESUMED_AFTER_RESTART));
    assert!(text.contains("(cargo build -p zeron-engine)") && text.contains("run it again"), "{text}");
}

#[test]
fn a_chat_whose_subagents_the_stop_ended_is_resumed_crew_or_not() {
    let chats: HashMap<String, Chat> =
        [chat("t1", "here"), chat("solo", "here")].into_iter().map(|c| (c.id.clone(), c)).collect();
    let state = crew_state(vec![card(WorkState::Active)]);
    let at = mins(30);
    // Its turn ended; its subagents ran on (the shutdown zeroed its row).
    let rows = [session("t1", SessionStatus::Idle), session("solo", SessionStatus::Idle)];
    let subs = vec![("solo".to_owned(), vec!["solo--sub--toolu_1".to_owned()])];
    let out = to_resume(&rows, &[], &subs, &HashMap::new(), &state, &chats, at);
    assert_eq!(out, [Resume { chat: "solo".into(), waited_on: vec![], subagents: Some(vec!["solo--sub--toolu_1".into()]) }]);
    // A crash leaves the count on its row.
    let mut crashed = session("t1", SessionStatus::Idle);
    crashed.running_subagents = 2;
    let out = to_resume(&[crashed], &[], &[], &HashMap::new(), &state, &chats, at);
    assert_eq!(out, [Resume { chat: "t1".into(), waited_on: vec![], subagents: Some(vec![]) }]);
}

#[test]
fn the_subagent_resume_names_them() {
    use zeron_proto::orchestration::prompt_text::resumed_after_restart_subagents;
    let text = resumed_after_restart_subagents(&["9c1c9639--sub--toolu_01".into()]);
    assert_eq!(
        text,
        "[Resumed] The engine restarted; your subagents (9c1c9639--sub--toolu_01) were stopped. Check their transcripts and rerun what didn't finish."
    );
    assert!(resumed_after_restart_subagents(&[]).starts_with("[Resumed] The engine restarted; your subagents were stopped."));
}

#[test]
fn a_chat_waiting_on_its_usage_limit_gets_its_limits_resume_not_a_restart_one() {
    let chats: HashMap<String, Chat> = [chat("t1", "here")].into_iter().map(|c| (c.id.clone(), c)).collect();
    let state = crew_state(vec![card(WorkState::Active)]);
    // Limited at the stop, its subagents running too: one "[Resumed]", the
    // limit's at its reset.
    let row = limited("t1", mins(2));
    let subs = vec![("t1".to_owned(), vec!["t1--sub--toolu_1".to_owned()])];
    assert!(to_resume(&[row.clone()], &[], &subs, &HashMap::new(), &state, &chats, mins(30)).is_empty());
    // Switched to another account meanwhile: it's at work there, so a
    // restart resume is its.
    let mut switched = row;
    switched.limited.as_mut().unwrap().switched_to = Some(zeron_proto::usage_limit::SwitchedAccount {
        account_slot: "b".into(),
        account_label: "b@x".into(),
        at: mins(3),
        harness: None,
        model: None,
    });
    assert_eq!(ids(to_resume(&[switched], &[], &subs, &HashMap::new(), &state, &chats, mins(30))), ["t1"]);
}

/// Against a whole engine: a chat the stop cut off is resumed once on the
/// next start, Orchestrators on or off.
mod engine {
    use std::sync::Arc;

    use zeron_proto::{AgentEvent, DoneStatus, HarnessId};

    use crate::registry::HarnessRegistry;
    use crate::sessions::tests::{done, settled, wake_rig, waiting_agent, waits_on};

    #[tokio::test]
    async fn a_chat_waiting_on_its_background_work_when_the_engine_stops_is_resumed_once() {
        use zeron_rpc::{Caller, RpcService, methods, with_caller};
        let rig = wake_rig("chat-wait-stop", true).await;
        // The chat as the workspace lists it (hosted here).
        let device = rig.core.workspace.device_id().to_owned();
        with_caller(
            Caller::Local,
            rig.core.rpc_service().handle(
                methods::MUTATE,
                serde_json::json!({"op":"createChat","chatId":rig.chat,"deviceId":device,
                    "config":{"harness":"mock","sandbox":"workspace-write"}}),
            ),
        )
        .await
        .unwrap();
        // Its turn completed into a wait on its own build.
        rig.turn.set_background(1);
        rig.turn.set_background_agents(vec![waiting_agent("build")]);
        rig.feed.send(done(DoneStatus::Completed)).unwrap();
        assert_eq!(waits_on(&settled(&rig).await), ["build"]);
        // The engine stops: the build and the wait end with it.
        rig.core.shutdown().await;
        let dir = rig._dir.path().to_path_buf();
        drop(rig.core);

        let core = crate::EngineCore::assemble(&dir, Arc::new(HarnessRegistry::new()), HarnessId::Mock, None).unwrap();
        let rpc = core.rpc_service();
        let before = core.resumes.take_before_stop();
        assert!(
            before.iter().filter(|s| s.chat_id == "chat-wait-stop").all(|s| s.waiting.is_none()),
            "its row no longer says it waited: the journal's mark does"
        );
        assert_eq!(rpc.resume_after_restart(before), ["chat-wait-stop"]);
        // Once: the mark is taken.
        assert!(rpc.resume_after_restart(core.resumes.take_before_stop()).is_empty());
        let handle = core.doc_host.open("chat-wait-stop").unwrap();
        let mut said: Vec<String> = handle
            .doc()
            .read_commands()
            .unwrap()
            .into_iter()
            .filter_map(|c| match c.payload {
                zeron_doc::SessionCommandPayload::Run { request, .. } => Some(request.prompt),
                _ => None,
            })
            .collect();
        said.extend(handle.doc().read_queue().unwrap().into_iter().map(|q| q.text));
        let resumed: Vec<&String> = said.iter().filter(|p| p.contains("[Resumed] The engine restarted")).collect();
        assert_eq!(resumed.len(), 1, "{said:?}");
        assert!(resumed[0].contains("agent build") && resumed[0].contains("run it again"), "{}", resumed[0]);
        core.shutdown().await;
    }

    #[tokio::test]
    async fn a_chat_whose_subagent_runs_when_the_engine_stops_is_resumed_once() {
        use zeron_rpc::{Caller, RpcService, methods, with_caller};
        let rig = wake_rig("chat-sub-stop", false).await;
        let device = rig.core.workspace.device_id().to_owned();
        with_caller(
            Caller::Local,
            rig.core.rpc_service().handle(
                methods::MUTATE,
                serde_json::json!({"op":"createChat","chatId":rig.chat,"deviceId":device,
                    "config":{"harness":"mock","sandbox":"workspace-write"}}),
            ),
        )
        .await
        .unwrap();
        // The parent spawns a subagent, which streams; the parent's turn ends
        // while it runs on.
        rig.feed
            .send(AgentEvent::ToolCall {
                id: "toolu_sub1".into(),
                call: zeron_proto::ToolCall::Unknown { name: "Agent".into(), input: None },
            })
            .unwrap();
        rig.feed
            .send(AgentEvent::Subagent {
                parent_tool_use_id: "toolu_sub1".into(),
                event: Box::new(AgentEvent::TextDelta { text: "looking".into() }),
            })
            .unwrap();
        rig.feed.send(done(DoneStatus::Completed)).unwrap();
        let s = settled(&rig).await;
        assert_eq!(s.running_subagents, 1, "{s:?}");
        // The engine stops: the subagent with it.
        rig.core.shutdown().await;
        let dir = rig._dir.path().to_path_buf();
        drop(rig.core);

        let core = crate::EngineCore::assemble(&dir, Arc::new(HarnessRegistry::new()), HarnessId::Mock, None).unwrap();
        let rpc = core.rpc_service();
        let before = core.resumes.take_before_stop();
        assert!(
            before.iter().filter(|s| s.chat_id == "chat-sub-stop").all(|s| s.running_subagents == 0),
            "its row no longer counts the subagent: the journal's mark does"
        );
        assert_eq!(rpc.resume_after_restart(before), ["chat-sub-stop"]);
        assert!(rpc.resume_after_restart(core.resumes.take_before_stop()).is_empty(), "once");
        let handle = core.doc_host.open("chat-sub-stop").unwrap();
        let mut said: Vec<String> = handle
            .doc()
            .read_commands()
            .unwrap()
            .into_iter()
            .filter_map(|c| match c.payload {
                zeron_doc::SessionCommandPayload::Run { request, .. } => Some(request.prompt),
                _ => None,
            })
            .collect();
        said.extend(handle.doc().read_queue().unwrap().into_iter().map(|q| q.text));
        let resumed: Vec<&String> = said.iter().filter(|p| p.contains("[Resumed] The engine restarted; your subagents")).collect();
        assert_eq!(resumed.len(), 1, "{said:?}");
        assert!(resumed[0].contains("chat-sub-stop--sub--toolu_sub1"), "{}", resumed[0]);
        core.shutdown().await;
    }

}
