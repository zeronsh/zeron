//! Usage-limit waits and resumes against a whole (local, edge-less) engine:
//! the limited rows are this engine's, the chats hosted on another device so
//! nothing here starts a run of what's sent to them (it stays queued).

use chrono::{DateTime, Duration as TimeDelta, Utc};
use serde_json::{Value, json};
use zeron_doc::SessionCommandPayload;
use zeron_proto::usage_limit::{RESUMED_AFTER_LIMIT, SessionLimit};
use zeron_proto::{AgentAccountsSnapshot, HarnessId};
use zeron_rpc::{Caller, RpcReply, RpcService, methods, with_caller};

use super::*;
use crate::registry::HarnessRegistry;
use crate::run_journal::LimitMark;

const REMOTE: &str = "dev-remote";

async fn core_at(dir: &std::path::Path) -> crate::EngineCore {
    crate::EngineCore::assemble_with_profile(
        crate::EngineProfile::local(dir).unwrap(),
        std::sync::Arc::new(HarnessRegistry::new()),
        HarnessId::Codex,
        None,
    )
    .unwrap()
}

async fn call(core: &crate::EngineCore, caller: Caller, method: &str, params: Value) -> Value {
    match with_caller(caller, core.rpc_service().handle(method, params)).await {
        Ok(RpcReply::Value(value)) => value,
        Ok(RpcReply::Stream(_)) => Value::Null,
        Err(error) => panic!("{method}: {error}"),
    }
}

async fn op(core: &crate::EngineCore, params: Value) -> String {
    call(core, Caller::Local, methods::ORCHESTRATE, params).await["id"].as_str().unwrap_or_default().to_owned()
}

async fn chat(core: &crate::EngineCore, id: &str) {
    call(
        core,
        Caller::Local,
        methods::MUTATE,
        json!({"op":"createChat", "chatId":id, "deviceId":REMOTE, "config":{"harness":"codex","sandbox":"workspace-write"}}),
    )
    .await;
}

async fn orchestrator(core: &crate::EngineCore, name: &str) -> String {
    op(core, json!({"op":"createOrchestrator","name":name,"goal":"g","deviceId":REMOTE})).await
}

/// A crew thread of `orch`, on `item` when given.
async fn crew(core: &crate::EngineCore, orch: &str, id: &str, item: Option<&str>) {
    chat(core, id).await;
    let mut params = json!({"op":"addToCrew","orchestratorId":orch,"chatId":id});
    if let Some(item) = item {
        params["itemId"] = json!(item);
    }
    op(core, params).await;
}

fn limit(since: DateTime<Utc>, until: Option<DateTime<Utc>>) -> SessionLimit {
    SessionLimit {
        provider: "codex".into(),
        account_slot: None,
        account_label: None,
        window: Some("5-hour".into()),
        until,
        since,
        attempt: 0,
        switched_to: None,
    }
}

/// `chat`'s turn ended on `limit` here (its row and its mark).
fn limited(core: &crate::EngineCore, chat: &str, limit: SessionLimit) {
    let mark = LimitMark { chat_id: chat.into(), limit, resumed_at: None };
    assert!(core.sessions.restore_limited(&mark));
    core.sessions.update_limited(chat, |_| {}, None);
}

/// How many times `chat` was sent the resume.
fn resumes_sent(core: &crate::EngineCore, chat: &str) -> usize {
    let handle = core.doc_host.open(chat).unwrap();
    let mut said: Vec<(String, Option<zeron_doc::Injected>)> = handle
        .doc()
        .read_commands()
        .unwrap()
        .into_iter()
        .filter_map(|c| match c.payload {
            SessionCommandPayload::Run { request, injected, .. } => Some((request.prompt, injected)),
            SessionCommandPayload::Steer { prompt, injected, .. } => Some((prompt, injected)),
            _ => None,
        })
        .collect();
    said.extend(handle.doc().read_queue().unwrap().into_iter().map(|q| (q.text, q.injected)));
    said.iter()
        .filter(|(text, injected)| {
            text.contains(RESUMED_AFTER_LIMIT)
                && injected.as_ref().is_some_and(|i| i.kind == zeron_doc::injected::LIMIT_RESET)
        })
        .count()
}

#[tokio::test]
async fn the_resume_goes_once_at_the_reset_orchestrators_first_then_crew_staggered() {
    let temp = tempfile::tempdir().unwrap();
    let core = core_at(temp.path()).await;
    let rpc = core.rpc_service();
    let orch = orchestrator(&core, "Billing").await;
    crew(&core, &orch, "crew-a", None).await;
    chat(&core, "plain-a").await;
    let t = Utc::now();
    let until = t + TimeDelta::hours(1);
    // The plain chat hit it first, the orchestrator last: order is by role.
    limited(&core, "plain-a", limit(t - TimeDelta::hours(3), Some(until)));
    limited(&core, "crew-a", limit(t - TimeDelta::hours(2), Some(until)));
    limited(&core, &orch, limit(t - TimeDelta::hours(1), Some(until)));
    let mut book = LimitBook::default();

    assert!(rpc.limits_pass(&mut book, t).await.is_empty(), "not before the reset");
    assert!(book.next_wake(&core.sessions.limited_rows(), t) <= until, "wakes at the reset");

    let reset = until + TimeDelta::seconds(1);
    assert_eq!(rpc.limits_pass(&mut book, reset).await, [orch.clone()], "the orchestrator first, the rest later");
    assert!(rpc.limits_pass(&mut book, reset).await.is_empty(), "once");
    // Then the crew, then the rest, each 20 to 60 s after the one before.
    let crew_at = book.fire_at("crew-a").unwrap();
    let plain_at = book.fire_at("plain-a").unwrap();
    for (gap, what) in [(crew_at - reset, "crew after the orchestrator"), (plain_at - crew_at, "the rest after the crew")] {
        assert!((20..=60).contains(&gap.num_seconds()), "{what}: {gap}");
    }
    assert!(rpc.limits_pass(&mut book, crew_at - TimeDelta::seconds(1)).await.is_empty());
    assert_eq!(rpc.limits_pass(&mut book, crew_at).await, ["crew-a"]);
    assert_eq!(rpc.limits_pass(&mut book, plain_at).await, ["plain-a"]);
    assert!(rpc.limits_pass(&mut book, reset + TimeDelta::seconds(600)).await.is_empty());
    for chat in [orch.as_str(), "crew-a", "plain-a"] {
        assert_eq!(resumes_sent(&core, chat), 1, "{chat}");
        // Still limited until the resumed turn starts (its host clears it).
        assert!(core.sessions.session_status(chat).unwrap().limited.is_some());
    }
    core.shutdown().await;
}

#[tokio::test]
async fn a_limit_survives_a_restart_and_its_resume_still_goes_once() {
    let temp = tempfile::tempdir().unwrap();
    let t = Utc::now();
    let until = t + TimeDelta::hours(1);
    let since = t - TimeDelta::hours(2);
    {
        let core = core_at(temp.path()).await;
        chat(&core, "r1").await;
        limited(&core, "r1", limit(since, Some(until)));
        core.shutdown().await;
    }
    // Restarted: the wait is re-armed from the mark, its row as it was.
    {
        let core = core_at(temp.path()).await;
        let rpc = core.rpc_service();
        let mut book = LimitBook::default();
        assert_eq!(rpc.restore_limits(&mut book), ["r1"]);
        let row = core.sessions.session_status("r1").expect("its row is back");
        assert_eq!(row.limited.as_ref().map(|l| (l.since, l.until)), Some((since, Some(until))));
        assert!(rpc.limits_pass(&mut book, t).await.is_empty());
        assert_eq!(rpc.limits_pass(&mut book, until + TimeDelta::seconds(1)).await, ["r1"]);
        core.shutdown().await;
    }
    // Restarted again before the resumed turn started: not sent again.
    {
        let core = core_at(temp.path()).await;
        let rpc = core.rpc_service();
        let mut book = LimitBook::default();
        assert_eq!(rpc.restore_limits(&mut book), ["r1"]);
        assert!(rpc.limits_pass(&mut book, until + TimeDelta::seconds(90)).await.is_empty());
        assert_eq!(resumes_sent(&core, "r1"), 1);
        assert_eq!(book.resumed(), ["r1"]);
        core.shutdown().await;
    }
}

#[tokio::test]
async fn a_paused_cards_thread_is_not_resumed_and_an_archived_chat_drops_its_wait() {
    let temp = tempfile::tempdir().unwrap();
    let core = core_at(temp.path()).await;
    let rpc = core.rpc_service();
    let orch = orchestrator(&core, "Auth").await;
    let item = op(&core, json!({"op":"upsertItem","item":{"title":"Login redirect","orchestratorId":orch}})).await;
    crew(&core, &orch, "paused-t", Some(&item)).await;
    op(&core, json!({"op":"pauseItem","id":item})).await;
    assert!(core.orchestration.item(&item).unwrap().paused());
    chat(&core, "archived-t").await;
    call(&core, Caller::Local, methods::MUTATE, json!({"op":"setChatArchived","chatId":"archived-t","archived":true})).await;
    let t = Utc::now();
    limited(&core, "paused-t", limit(t - TimeDelta::hours(1), Some(t)));
    limited(&core, "archived-t", limit(t - TimeDelta::hours(1), Some(t)));
    let mut book = LimitBook::default();
    let later = t + TimeDelta::seconds(1);
    assert!(rpc.limits_pass(&mut book, later).await.is_empty());
    assert!(rpc.limits_pass(&mut book, later + TimeDelta::minutes(30)).await.is_empty());
    assert_eq!(resumes_sent(&core, "paused-t"), 0);
    assert!(core.sessions.session_status("paused-t").unwrap().limited.is_some(), "it waits on");
    assert_eq!(resumes_sent(&core, "archived-t"), 0);
    assert!(core.sessions.session_status("archived-t").unwrap().limited.is_none(), "its wait is over");
    core.shutdown().await;
}

#[tokio::test]
async fn the_account_switch_is_asked_only_where_the_orchestrator_allows_it() {
    let temp = tempfile::tempdir().unwrap();
    let core = core_at(temp.path()).await;
    let rpc = core.rpc_service();
    let allows = orchestrator(&core, "Allows").await;
    let waits = orchestrator(&core, "Waits").await;
    op(&core, json!({"op":"updateOrchestrator","id":allows,"crew":{"onLimit":{"switchAccount":true}}})).await;
    assert!(rpc.on_limit_policy(&allows).switch_account);
    assert!(!rpc.on_limit_policy(&waits).switch_account);
    crew(&core, &allows, "sw-crew-allows", None).await;
    crew(&core, &waits, "sw-crew-waits", None).await;
    chat(&core, "sw-plain").await;
    let all = [allows.as_str(), waits.as_str(), "sw-crew-allows", "sw-crew-waits", "sw-plain"];
    let t = Utc::now();
    for chat in all {
        // A fresh limit, as its turn just recorded it.
        limited(&core, chat, limit(t, Some(t + TimeDelta::hours(1))));
    }
    let mut book = LimitBook::default();
    rpc.limits_pass(&mut book, t).await;
    let mut asked: Vec<String> =
        SWITCH_ASKED.lock().unwrap().iter().filter(|c| all.contains(&c.as_str())).cloned().collect();
    asked.sort();
    let mut expected = vec![allows.clone(), "sw-crew-allows".to_owned()];
    expected.sort();
    assert_eq!(asked, expected);
    // No other account had quota here: each waits for its reset.
    for chat in all {
        assert!(core.sessions.session_status(chat).unwrap().limited.unwrap().waiting(), "{chat}");
    }
    core.shutdown().await;
}

fn snapshot(fetched: DateTime<Utc>, windows: Value) -> AgentAccountsSnapshot {
    serde_json::from_value(json!({
        "accounts": [{
            "id": "slot-1", "harness": HarnessId::Codex, "email": "me@example.com", "planLabel": null,
            "active": true, "usageWindows": windows, "usageFetchedAt": fetched.timestamp_millis(),
        }],
        "warnings": [],
    }))
    .unwrap()
}

#[test]
fn a_limit_without_a_time_takes_its_accounts_probe_and_a_relimit_backs_off() {
    let t: DateTime<Utc> = "2026-11-17T12:00:00Z".parse().unwrap();
    let resets = t + TimeDelta::minutes(90);
    let accounts = snapshot(
        t,
        json!([
            {"label": "5h", "usedFraction": 1.0, "resetsAt": resets},
            {"label": "Week", "usedFraction": 0.4, "resetsAt": t + TimeDelta::days(3)},
        ]),
    );
    let done = complete(limit(t, None), Some(&accounts), t);
    assert_eq!(done.until, Some(resets), "the exhausted window's reset");
    assert_eq!((done.account_slot.as_deref(), done.account_label.as_deref()), (Some("slot-1"), Some("me@example.com")));
    // The error's own words win.
    assert_eq!(complete(limit(t, Some(t + TimeDelta::minutes(5))), Some(&accounts), t).until, Some(t + TimeDelta::minutes(5)));
    // No probe: unknown, re-checked.
    assert_eq!(complete(limit(t, None), None, t).until, None);
    // A limit right after a resume waits at least its backoff.
    let again = complete(SessionLimit { attempt: 1, ..limit(t, Some(t + TimeDelta::minutes(1))) }, None, t);
    assert_eq!(again.until, Some(t + check_backoff(1)));
    let later = complete(SessionLimit { attempt: 1, ..limit(t, Some(t + TimeDelta::hours(3))) }, None, t);
    assert_eq!(later.until, Some(t + TimeDelta::hours(3)));
}

#[tokio::test]
async fn a_resume_that_runs_straight_back_into_the_limit_waits_again_with_a_backoff() {
    let temp = tempfile::tempdir().unwrap();
    let core = core_at(temp.path()).await;
    let rpc = core.rpc_service();
    chat(&core, "relimit").await;
    let t = Utc::now();
    let mut book = LimitBook::default();
    limited(&core, "relimit", limit(t - TimeDelta::hours(2), Some(t)));
    assert_eq!(rpc.limits_pass(&mut book, t + TimeDelta::seconds(1)).await, ["relimit"]);
    // The resumed turn ran straight back into it: a new limit, attempt 1
    // (see `sessions`' own test), waiting again with its reset pushed to the
    // backoff.
    let since = t + TimeDelta::minutes(1);
    core.sessions.update_limited(
        "relimit",
        |l| *l = Some(SessionLimit { attempt: 1, ..limit(since, Some(since + TimeDelta::seconds(30))) }),
        None,
    );
    assert!(rpc.limits_pass(&mut book, since).await.is_empty());
    let l = core.sessions.session_status("relimit").unwrap().limited.unwrap();
    assert_eq!((l.attempt, l.until), (1, Some(since + check_backoff(1))));
    assert!(rpc.limits_pass(&mut book, since + TimeDelta::minutes(5)).await.is_empty(), "no spam meanwhile");
    assert_eq!(rpc.limits_pass(&mut book, since + check_backoff(1)).await, ["relimit"]);
    assert_eq!(resumes_sent(&core, "relimit"), 2, "once per reset");
    core.shutdown().await;
}


#[tokio::test]
async fn a_chat_a_restart_just_resumed_gets_no_second_resume_at_its_reset() {
    let temp = tempfile::tempdir().unwrap();
    let core = core_at(temp.path()).await;
    let rpc = core.rpc_service();
    chat(&core, "plain-a").await;
    let t = Utc::now();
    let until = t + TimeDelta::hours(1);
    limited(&core, "plain-a", limit(t - TimeDelta::hours(2), Some(until)));
    let mut book = LimitBook::default();
    let reset = until + TimeDelta::seconds(1);
    // A restart path sent it its "[Resumed]" a moment ago.
    core.resumes.note_resumed(&["plain-a".to_owned()], reset);
    assert!(rpc.limits_pass(&mut book, reset).await.is_empty(), "not a second message");
    assert_eq!(resumes_sent(&core, "plain-a"), 0);
    // Still limited past the grace (that turn never started): its own resume.
    let later = reset + TimeDelta::seconds(crate::restart_resume::RESUMED_GRACE_SECS + 1);
    assert_eq!(rpc.limits_pass(&mut book, later).await, ["plain-a"]);
    assert_eq!(resumes_sent(&core, "plain-a"), 1);
    core.shutdown().await;
}
