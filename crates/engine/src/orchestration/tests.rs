//! `Orchestrate` ops against a whole (local, edge-less) engine.

use futures::StreamExt;
use serde_json::{Value, json};
use zeron_doc::SessionCommandPayload;
use zeron_proto::orchestration::*;
use zeron_proto::{ChildKind, HarnessId, agent_messages};
use zeron_rpc::{Caller, RpcReply, RpcService, methods, with_caller};

use super::ChatRole;
use crate::registry::HarnessRegistry;

/// Where test chats run: another device, so nothing here starts a run of
/// what's sent to them (the commands stay queued for that host).
const REMOTE: &str = "dev-remote";

async fn core() -> (tempfile::TempDir, crate::EngineCore) {
    let temp = tempfile::tempdir().unwrap();
    let core = crate::EngineCore::assemble_with_profile(
        crate::EngineProfile::local(temp.path()).unwrap(),
        std::sync::Arc::new(HarnessRegistry::new()),
        HarnessId::Codex,
        None,
    )
    .unwrap();
    (temp, core)
}

fn agent(chat: &str) -> Caller {
    Caller::Agent {
        chat: Some(chat.into()),
    }
}

async fn call(
    core: &crate::EngineCore,
    caller: Caller,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    let rpc = core.rpc_service();
    match with_caller(caller, rpc.handle(method, params)).await {
        Ok(RpcReply::Value(value)) => Ok(value),
        Ok(RpcReply::Stream(mut stream)) => Ok(stream.next().await.unwrap_or(Value::Null)),
        Err(error) => Err(error.to_string()),
    }
}

async fn op(core: &crate::EngineCore, caller: Caller, params: Value) -> Result<String, String> {
    call(core, caller, methods::ORCHESTRATE, params)
        .await
        .map(|v| v["id"].as_str().unwrap_or_default().to_owned())
}

/// The person's chat on [`REMOTE`], optionally under `parent` as `kind`.
async fn chat(core: &crate::EngineCore, id: &str, title: &str, parent: Option<(&str, &str)>) {
    let mut params = json!({"op":"createChat", "chatId":id, "deviceId":REMOTE,
        "config":{"harness":"codex","sandbox":"workspace-write"}});
    if let Some((parent, kind)) = parent {
        params["parentChatId"] = json!(parent);
        params["childKind"] = json!(kind);
    }
    call(core, Caller::Local, methods::MUTATE, params)
        .await
        .unwrap();
    call(
        core,
        Caller::Local,
        methods::MUTATE,
        json!({"op":"renameChat","chatId":id,"title":title}),
    )
    .await
    .unwrap();
}

/// An orchestrator the person made, hosted on [`REMOTE`].
async fn orchestrator(core: &crate::EngineCore, name: &str) -> String {
    op(
        core,
        Caller::Local,
        json!({"op":"createOrchestrator","name":name,"goal":"g","deviceId":REMOTE}),
    )
    .await
    .unwrap()
}

/// What was sent to a chat as runs or steers, oldest first.
fn sent(core: &crate::EngineCore, chat: &str) -> Vec<String> {
    let handle = core.doc_host.open(chat).unwrap();
    handle
        .doc()
        .read_commands()
        .unwrap()
        .into_iter()
        .filter_map(|c| match c.payload {
            SessionCommandPayload::Run { request, .. } => Some(request.prompt),
            SessionCommandPayload::Steer { prompt, .. } => Some(prompt),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn an_orchestrator_is_a_chat_with_a_row_and_its_first_prompt_runs() {
    let (_temp, core) = core().await;
    let id = op(
        &core,
        Caller::Local,
        json!({"op":"createOrchestrator","name":"Auth","goal":"Ship login","deviceId":REMOTE,
            "crew":{"maxRunning":2},"prompt":"Plan the work"}),
    )
    .await
    .unwrap();
    let chat_row = core.workspace.chat(&id).unwrap().unwrap();
    assert_eq!(chat_row.title.as_deref(), Some("Auth"));
    assert_eq!(chat_row.device_id, REMOTE);
    assert!(
        chat_row.is_top_level(),
        "an orchestrator is a top-level chat"
    );
    let row = core.orchestration.orchestrator(&id).unwrap();
    assert_eq!(
        (row.kind, row.hue, row.crew.max_running),
        (OrchestratorKind::Standard, seed_hue(&id), Some(2))
    );
    assert_eq!(
        core.orchestration.role_of(&id),
        ChatRole::Orchestrator { id: id.clone() }
    );
    assert_eq!(sent(&core, &id), vec!["Plan the work"]);

    // The watch sends it.
    let state = call(
        &core,
        Caller::Local,
        methods::WATCH_ORCHESTRATION,
        json!({}),
    )
    .await
    .unwrap();
    let state: OrchestrationState = serde_json::from_value(state).unwrap();
    assert!(state.orchestrators.iter().any(|o| o.id == id));

    // Promote an existing chat; the person only.
    chat(&core, "plain", "Plain", None).await;
    chat(&core, "other", "Other", None).await;
    let promoted = op(
        &core,
        Caller::Local,
        json!({"op":"promoteChat","chatId":"plain","name":"Ops"}),
    )
    .await
    .unwrap();
    assert_eq!(promoted, "plain");
    assert_eq!(
        core.orchestration.orchestrator("plain").unwrap().name,
        "Ops"
    );
    assert!(
        op(
            &core,
            agent(&id),
            json!({"op":"promoteChat","chatId":"other","name":"X"})
        )
        .await
        .is_err()
    );
    core.sessions.shutdown().await;
}

#[tokio::test]
async fn a_dump_is_routed_and_each_fragment_delivered_where_it_goes() {
    let (_temp, core) = core().await;
    let auth = orchestrator(&core, "Auth").await;
    chat(&core, "notes-chat", "Notes", None).await;
    let text = "fix the login redirect. buy milk. maybe the docs or the site?";
    let dump = op(&core, Caller::Local, json!({"op":"dump","text":text}))
        .await
        .unwrap();

    // The Brain dump exists, here, hidden, with the dump sent to it.
    let state = core.orchestration.state();
    let brain = state
        .orchestrators
        .iter()
        .find(|o| o.kind == OrchestratorKind::BrainDump)
        .unwrap()
        .id
        .clone();
    let brain_chat = core.workspace.chat(&brain).unwrap().unwrap();
    assert_eq!(brain_chat.device_id, core.device_id);
    assert!(!brain_chat.is_top_level());
    assert_eq!(sent(&core, &brain), vec![format!("[Dump {dump}]\n{text}")]);
    assert_eq!(state.dumps[0].state, DumpState::Routing);
    // A second dump reuses it.
    op(&core, Caller::Local, json!({"op":"dump","text":"another"}))
        .await
        .unwrap();
    assert_eq!(
        core.orchestration
            .state()
            .orchestrators
            .iter()
            .filter(|o| o.kind == OrchestratorKind::BrainDump)
            .count(),
        1
    );

    // Only the Brain dump routes.
    let route = |excerpt: &str, target: Value, message: &str| json!({"op":"route","dumpId":dump,"excerpt":excerpt,"target":target,"intent":"task","message":message});
    assert!(
        op(
            &core,
            agent(&auth),
            route("buy milk", json!({"kind":"note"}), "")
        )
        .await
        .is_err()
    );

    let to_auth = op(
        &core,
        agent(&brain),
        route(
            "fix the login redirect",
            json!({"kind":"orchestrator","id":auth}),
            "Fix the login redirect loop",
        ),
    )
    .await
    .unwrap();
    let delivered = sent(&core, &auth);
    let (sender, body) = agent_messages::agent_message_sender(delivered.last().unwrap()).unwrap();
    assert_eq!(
        (sender.chat_id.as_str(), body),
        (brain.as_str(), "Fix the login redirect loop")
    );

    op(
        &core,
        agent(&brain),
        route("buy milk", json!({"kind":"note"}), "buy milk"),
    )
    .await
    .unwrap();
    assert!(
        core.orchestration
            .orchestrator(&brain)
            .unwrap()
            .notes
            .contains("- buy milk")
    );
    let item_route = op(
        &core,
        agent(&brain),
        route("buy milk", json!({"kind":"item","id":""}), ""),
    )
    .await
    .unwrap();
    let unsure = op(
        &core,
        agent(&brain),
        json!({"op":"route","dumpId":dump,"excerpt":"maybe the docs or the site?","target":{"kind":"unsure"},
            "intent":"task","message":"Update the docs","candidates":[{"kind":"orchestrator","id":auth}]}),
    )
    .await
    .unwrap();
    op(
        &core,
        agent(&brain),
        json!({"op":"dumpDone","dumpId":dump,"summary":"3 places, 1 to pick"}),
    )
    .await
    .unwrap();

    let d = core
        .workspace
        .read_orchestration_with(|doc| doc.dump(&dump))
        .unwrap();
    assert_eq!(d.state, DumpState::NeedsYou);
    assert_eq!(d.summary.as_deref(), Some("3 places, 1 to pick"));
    let first = d.routes.iter().find(|r| r.id == to_auth).unwrap();
    assert_eq!(
        (first.state, first.span),
        (RouteState::Delivered, Some((0, 22)))
    );
    let item_id = d
        .routes
        .iter()
        .find(|r| r.id == item_route)
        .unwrap()
        .item_id
        .clone()
        .unwrap();
    let item = core.orchestration.item(&item_id).unwrap();
    assert_eq!(
        (item.orchestrator_id.as_deref(), item.state),
        (None, WorkState::Inbox)
    );
    assert_eq!(
        item.source,
        Some(RouteRef {
            dump_id: dump.clone(),
            route_id: item_route
        })
    );

    // The person picks a place for the unsure one: delivered, dump routed.
    op(
        &core,
        Caller::Local,
        json!({"op":"reroute","dumpId":dump,"routeId":unsure,"target":{"kind":"chat","id":"notes-chat"}}),
    )
    .await
    .unwrap();
    let d = core
        .workspace
        .read_orchestration_with(|doc| doc.dump(&dump))
        .unwrap();
    assert_eq!(d.state, DumpState::Routed);
    let to_notes = sent(&core, "notes-chat");
    let (_, body) = agent_messages::agent_message_sender(to_notes.last().unwrap()).unwrap();
    assert_eq!(body, "Update the docs");

    // Moving a delivered one retracts it where it went.
    op(
        &core,
        Caller::Local,
        json!({"op":"reroute","dumpId":dump,"routeId":to_auth,"target":{"kind":"chat","id":"notes-chat"}}),
    )
    .await
    .unwrap();
    let to_auth_now = sent(&core, &auth);
    let (_, retraction) =
        agent_messages::agent_message_sender(to_auth_now.last().unwrap()).unwrap();
    assert!(
        retraction.starts_with("[Brain dump] Ignore the earlier note \"fix the login redirect"),
        "{retraction}"
    );

    // Routes make new places: a thread, an orchestrator.
    op(
        &core,
        agent(&brain),
        route("buy milk", json!({"kind":"newThread","id":""}), "Buy milk"),
    )
    .await
    .unwrap();
    op(
        &core,
        agent(&brain),
        route(
            "docs",
            json!({"kind":"newOrchestrator","id":""}),
            "Own the docs",
        ),
    )
    .await
    .unwrap();
    let d = core
        .workspace
        .read_orchestration_with(|doc| doc.dump(&dump))
        .unwrap();
    for route in d.routes.iter().rev().take(2) {
        let (RouteTarget::NewThread { id } | RouteTarget::NewOrchestrator { id }) = &route.target
        else {
            panic!("{route:?}");
        };
        let new = core.workspace.chat(id).unwrap().unwrap();
        assert!(new.is_top_level(), "the Brain dump's chats are top-level");
        assert_eq!(sent(&core, id).len(), 1);
    }
    let comms = call(&core, Caller::Local, methods::WATCH_COMMS, json!({}))
        .await
        .unwrap();
    assert!(
        comms
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["kind"] == "route")
    );
    core.sessions.shutdown().await;
}

#[tokio::test]
async fn an_answer_goes_to_the_chat_that_asked() {
    let (_temp, core) = core().await;
    chat(&core, "asker", "Asker", None).await;
    assert!(
        op(
            &core,
            Caller::Local,
            json!({"op":"ask","kind":"question","question":"?"})
        )
        .await
        .is_err()
    );
    let id = op(
        &core,
        agent("asker"),
        json!({"op":"ask","kind":"decision","question":"Merge the auth PR now?","options":[{"label":"Yes"}]}),
    )
    .await
    .unwrap();
    let ask = core
        .workspace
        .read_orchestration_with(|doc| doc.ask(&id))
        .unwrap();
    assert_eq!(
        (ask.state, ask.from_chat.as_str()),
        (AskState::Open, "asker")
    );
    // Only the person answers.
    assert!(
        op(
            &core,
            agent("asker"),
            json!({"op":"answerAsk","id":id,"answer":"yes"})
        )
        .await
        .is_err()
    );
    op(
        &core,
        Caller::Local,
        json!({"op":"answerAsk","id":id,"answer":"Yes"}),
    )
    .await
    .unwrap();
    let ask = core
        .workspace
        .read_orchestration_with(|doc| doc.ask(&id))
        .unwrap();
    assert_eq!(
        (ask.state, ask.answer.as_deref()),
        (AskState::Answered, Some("Yes"))
    );
    assert!(ask.answered_at.is_some());
    assert_eq!(
        sent(&core, "asker"),
        vec!["[Answer from you] Merge the auth PR now? → Yes"]
    );
    assert!(
        op(
            &core,
            Caller::Local,
            json!({"op":"answerAsk","id":id,"answer":"No"})
        )
        .await
        .is_err()
    );

    let other = op(
        &core,
        agent("asker"),
        json!({"op":"ask","kind":"blocker","question":"Stuck"}),
    )
    .await
    .unwrap();
    op(&core, Caller::Local, json!({"op":"dismissAsk","id":other}))
        .await
        .unwrap();
    let ask = core
        .workspace
        .read_orchestration_with(|doc| doc.ask(&other))
        .unwrap();
    assert_eq!(ask.state, AskState::Dismissed);
    let comms = core.orchestration.comms().recent();
    assert!(
        comms
            .iter()
            .any(|e| e.kind == CommKind::Ask && e.from == "asker")
    );
    assert!(
        comms
            .iter()
            .any(|e| e.kind == CommKind::Answer && e.to == "asker")
    );
    core.sessions.shutdown().await;
}

#[tokio::test]
async fn crews_reports_and_the_board() {
    let (_temp, core) = core().await;
    let auth = orchestrator(&core, "Auth").await;
    let billing = orchestrator(&core, "Billing").await;
    let item = op(
        &core,
        agent(&auth),
        json!({"op":"upsertItem","item":{"title":"Login redirect"}}),
    )
    .await
    .unwrap();
    assert_eq!(
        core.orchestration
            .item(&item)
            .unwrap()
            .orchestrator_id
            .as_deref(),
        Some(auth.as_str())
    );

    // The orchestrator's own children are its crew.
    let rpc = core.rpc_service();
    with_caller(
        agent(&auth),
        rpc.handle(
            methods::MUTATE,
            json!({"op":"createChat","chatId":"worker","deviceId":REMOTE,"parentChatId":auth,
                "config":{"harness":"codex","sandbox":"workspace-write"}}),
        ),
    )
    .await
    .unwrap();
    let worker = core.workspace.chat("worker").unwrap().unwrap();
    assert_eq!(
        (worker.parent_chat_id.as_deref(), worker.child_kind),
        (Some(auth.as_str()), Some(ChildKind::Crew))
    );
    assert!(!worker.is_top_level());
    assert_eq!(
        core.orchestration
            .crew_of("worker")
            .unwrap()
            .orchestrator_id,
        auth
    );

    // The person puts an existing thread in the crew, on an item.
    chat(&core, "helper", "Helper", None).await;
    op(
        &core,
        Caller::Local,
        json!({"op":"addToCrew","orchestratorId":auth,"chatId":"helper","role":"tests","itemId":item}),
    )
    .await
    .unwrap();
    assert_eq!(
        core.orchestration.role_of("helper"),
        ChatRole::Crew {
            orchestrator_id: auth.clone(),
            item_id: Some(item.clone())
        }
    );
    assert_eq!(
        core.workspace.chat("helper").unwrap().unwrap().child_kind,
        Some(ChildKind::Crew)
    );
    assert_eq!(
        core.orchestration.item(&item).unwrap().threads,
        vec!["helper"]
    );

    // It reports: crew row, item progress and links, comms.
    op(
        &core,
        agent("helper"),
        json!({"op":"report","state":"progress","summary":"half way","progress":0.5,
            "links":[{"kind":"pr","label":"#12","target":"https://x/pr/12"}]}),
    )
    .await
    .unwrap();
    let member = core.orchestration.crew_of("helper").unwrap();
    assert_eq!(member.last_report.unwrap().summary, "half way");
    let board = core.orchestration.item(&item).unwrap();
    assert_eq!((board.progress, board.links.len()), (Some(0.5), 1));
    assert!(
        core.orchestration
            .comms()
            .recent()
            .iter()
            .any(|e| e.kind == CommKind::Report && e.to == auth)
    );
    // Only a crew thread reports.
    assert!(
        op(
            &core,
            agent(&auth),
            json!({"op":"report","state":"done","summary":"x"})
        )
        .await
        .is_err()
    );

    // Its notes are the orchestrator's own; the rest is the person's.
    op(
        &core,
        agent(&auth),
        json!({"op":"updateOrchestrator","id":auth,"notes":"plan"}),
    )
    .await
    .unwrap();
    assert!(
        op(
            &core,
            agent(&auth),
            json!({"op":"updateOrchestrator","id":auth,"paused":true})
        )
        .await
        .is_err()
    );
    assert!(
        op(
            &core,
            agent(&auth),
            json!({"op":"updateOrchestrator","id":billing,"notes":"x"})
        )
        .await
        .is_err()
    );
    op(
        &core,
        Caller::Local,
        json!({"op":"updateOrchestrator","id":auth,"wake":{"everyMinutes":1,"prompt":"hi"}}),
    )
    .await
    .unwrap();
    assert_eq!(
        core.orchestration
            .orchestrator(&auth)
            .unwrap()
            .wake
            .unwrap()
            .every_minutes,
        Some(5)
    );

    // Released: top-level again, the row kept.
    op(
        &core,
        agent(&auth),
        json!({"op":"releaseFromCrew","chatId":"helper"}),
    )
    .await
    .unwrap();
    assert!(
        core.workspace
            .chat("helper")
            .unwrap()
            .unwrap()
            .is_top_level()
    );
    assert!(core.orchestration.crew_of("helper").is_none());
    assert!(
        core.workspace
            .read_orchestration_with(|doc| doc.crew_member("helper"))
            .unwrap()
            .released
    );
    core.sessions.shutdown().await;
}

/// Roles come from rows; crews and items stay their orchestrator's.
#[tokio::test]
async fn agents_stay_inside_their_own_orchestrator() {
    let (_temp, core) = core().await;
    let auth = orchestrator(&core, "Auth").await;
    let billing = orchestrator(&core, "Billing").await;
    let mine = op(
        &core,
        agent(&auth),
        json!({"op":"upsertItem","item":{"title":"Mine"}}),
    )
    .await
    .unwrap();
    let theirs = op(
        &core,
        agent(&billing),
        json!({"op":"upsertItem","item":{"title":"Theirs"}}),
    )
    .await
    .unwrap();
    let loose = op(
        &core,
        Caller::Local,
        json!({"op":"upsertItem","item":{"title":"Loose","orchestratorId":null}}),
    )
    .await
    .unwrap();
    assert_eq!(
        core.orchestration.item(&loose).unwrap().orchestrator_id,
        None
    );
    let rpc = core.rpc_service();
    let create = |caller: Caller, params: Value| {
        let rpc = rpc.clone();
        async move {
            with_caller(caller, rpc.handle(methods::MUTATE, params))
                .await
                .map(drop)
        }
    };
    let new_chat = |id: &str, parent: &str, kind: Option<&str>| {
        let mut p = json!({"op":"createChat","chatId":id,"deviceId":REMOTE,"parentChatId":parent,
            "config":{"harness":"codex","sandbox":"workspace-write"}});
        if let Some(kind) = kind {
            p["childKind"] = json!(kind);
        }
        p
    };

    // (1) An id shaped like the Brain dump's is no Brain dump: no role, a
    // ceiling, and an agent can't mint one (nor a voice orchestrator's).
    let fake = format!("{BRAIN_DUMP_CHAT_PREFIX}fake");
    chat(&core, &fake, "Fake", None).await;
    call(
        &core,
        Caller::Local,
        methods::MUTATE,
        json!({"op":"setChatConfig","chatId":fake,
            "config":{"harness":"codex","sandbox":"read-only","access":"supervised"}}),
    )
    .await
    .unwrap();
    assert_eq!(core.orchestration.role_of(&fake), ChatRole::Plain);
    assert!(
        op(
            &core,
            agent(&fake),
            json!({"op":"createOrchestrator","name":"X"})
        )
        .await
        .is_err()
    );
    create(
        agent(&fake),
        json!({"op":"createChat","chatId":"capped","deviceId":REMOTE,
            "config":{"harness":"codex","sandbox":"danger-full-access","access":"full-access"}}),
    )
    .await
    .unwrap();
    let capped = core
        .workspace
        .chat("capped")
        .unwrap()
        .unwrap()
        .config
        .unwrap();
    assert_eq!(capped.sandbox, zeron_proto::SandboxLevel::ReadOnly);
    for reserved in [
        format!("{BRAIN_DUMP_CHAT_PREFIX}x"),
        format!("{}x", zeron_proto::voice::ORCHESTRATOR_CHAT_PREFIX),
    ] {
        let params = json!({"op":"createChat","chatId":reserved,"deviceId":REMOTE,
            "config":{"harness":"codex","sandbox":"workspace-write"}});
        assert!(create(agent(&auth), params).await.is_err(), "{reserved}");
    }

    // (2) A crew thread is only its creator's.
    chat(&core, "plain", "Plain", None).await;
    assert!(
        create(agent("plain"), new_chat("stolen", &billing, Some("crew")))
            .await
            .is_err()
    );
    assert!(
        create(agent(&auth), new_chat("stolen2", &billing, Some("crew")))
            .await
            .is_err()
    );
    assert!(
        create(agent("plain"), new_chat("stolen3", "plain", Some("crew")))
            .await
            .is_err()
    );
    assert!(core.workspace.chat("stolen").unwrap().is_none());
    assert!(core.orchestration.crew_of("stolen2").is_none());
    create(agent(&auth), new_chat("ours", &auth, Some("crew")))
        .await
        .unwrap();
    assert_eq!(
        core.orchestration.crew_of("ours").unwrap().orchestrator_id,
        auth
    );
    // Nor another's crew thread, nor its items.
    create(agent(&billing), new_chat("billing-worker", &billing, None))
        .await
        .unwrap();
    assert_eq!(
        core.orchestration
            .crew_of("billing-worker")
            .unwrap()
            .orchestrator_id,
        billing
    );
    for refused in [
        json!({"op":"addToCrew","orchestratorId":auth,"chatId":"billing-worker"}),
        json!({"op":"addToCrew","orchestratorId":billing,"chatId":"plain"}),
        json!({"op":"addToCrew","orchestratorId":auth,"chatId":"plain","itemId":theirs}),
        json!({"op":"releaseFromCrew","chatId":"billing-worker"}),
    ] {
        assert!(
            op(&core, agent(&auth), refused.clone()).await.is_err(),
            "{refused}"
        );
    }
    assert_eq!(
        core.orchestration
            .crew_of("billing-worker")
            .unwrap()
            .orchestrator_id,
        billing
    );

    // (3) A report naming another's item counts for its own item.
    op(
        &core,
        Caller::Local,
        json!({"op":"addToCrew","orchestratorId":auth,"chatId":"plain","itemId":mine}),
    )
    .await
    .unwrap();
    op(
        &core,
        agent("plain"),
        json!({"op":"report","state":"progress","summary":"s","itemId":theirs,"progress":0.9}),
    )
    .await
    .unwrap();
    assert_eq!(core.orchestration.item(&theirs).unwrap().progress, None);
    assert!(core.orchestration.item(&theirs).unwrap().threads.is_empty());
    assert_eq!(core.orchestration.item(&mine).unwrap().progress, Some(0.9));
    // An ask names only its own orchestrator's item.
    assert!(
        op(
            &core,
            agent("plain"),
            json!({"op":"ask","kind":"question","question":"?","itemId":theirs})
        )
        .await
        .is_err()
    );
    op(
        &core,
        agent("plain"),
        json!({"op":"ask","kind":"question","question":"?","itemId":mine}),
    )
    .await
    .unwrap();
    // The board: its own, loose ones only by claiming them, its own crew.
    for refused in [
        json!({"op":"upsertItem","item":{"id":theirs,"state":"done"}}),
        json!({"op":"upsertItem","item":{"id":loose,"state":"active"}}),
        json!({"op":"upsertItem","item":{"id":mine,"orchestratorId":billing}}),
        json!({"op":"upsertItem","item":{"id":mine,"threads":["billing-worker"]}}),
        json!({"op":"upsertItem","item":{"title":"x","orchestratorId":billing}}),
    ] {
        assert!(
            op(&core, agent(&auth), refused.clone()).await.is_err(),
            "{refused}"
        );
    }
    op(
        &core,
        agent(&auth),
        json!({"op":"upsertItem","item":{"id":loose,"orchestratorId":auth}}),
    )
    .await
    .unwrap();
    assert_eq!(
        core.orchestration
            .item(&loose)
            .unwrap()
            .orchestrator_id
            .as_deref(),
        Some(auth.as_str())
    );
    op(
        &core,
        agent(&auth),
        json!({"op":"upsertItem","item":{"id":mine,"state":"done","threads":["plain","ours"]}}),
    )
    .await
    .unwrap();
    assert!(core.orchestration.item(&mine).unwrap().done_at.is_some());
    // A plain chat keeps no board, and an orchestrator doesn't route.
    assert!(
        op(
            &core,
            agent("other-chat"),
            json!({"op":"upsertItem","item":{"title":"x"}})
        )
        .await
        .is_err()
    );
    assert!(
        op(
            &core,
            agent(&auth),
            json!({"op":"route","dumpId":"d","excerpt":"x","target":{"kind":"note"},"intent":"note"})
        )
        .await
        .is_err()
    );
    core.sessions.shutdown().await;
}

/// Delegation depth: a subagent creates nothing; a crew thread its own
/// subagents.
#[tokio::test]
async fn crew_threads_delegate_and_subagents_do_not() {
    let (_temp, core) = core().await;
    let auth = orchestrator(&core, "Auth").await;
    chat(&core, "top", "Top", None).await;
    chat(&core, "sub", "Sub", Some(("top", "subagent"))).await;
    let rpc = core.rpc_service();
    let child = |id: &str, parent: &str| {
        json!({"op":"createChat","chatId":id,"deviceId":REMOTE,"parentChatId":parent,"childKind":"subagent",
            "config":{"harness":"codex","sandbox":"workspace-write"}})
    };
    assert!(
        with_caller(
            agent("sub"),
            rpc.handle(methods::MUTATE, child("deeper", "sub"))
        )
        .await
        .is_err()
    );
    assert!(
        with_caller(
            agent("top"),
            rpc.handle(methods::MUTATE, child("deeper", "sub"))
        )
        .await
        .is_err()
    );
    with_caller(
        agent(&auth),
        rpc.handle(
            methods::MUTATE,
            json!({"op":"createChat","chatId":"crewed","deviceId":REMOTE,"parentChatId":auth,
                "config":{"harness":"codex","sandbox":"workspace-write"}}),
        ),
    )
    .await
    .unwrap();
    with_caller(
        agent("crewed"),
        rpc.handle(methods::MUTATE, child("its-sub", "crewed")),
    )
    .await
    .unwrap();
    let sub = core.workspace.chat("its-sub").unwrap().unwrap();
    assert_eq!(
        (sub.parent_chat_id.as_deref(), sub.child_kind),
        (Some("crewed"), Some(ChildKind::Subagent))
    );
    assert!(
        with_caller(
            agent("its-sub"),
            rpc.handle(methods::MUTATE, child("nope", "its-sub"))
        )
        .await
        .is_err()
    );
    core.sessions.shutdown().await;
}

#[tokio::test]
async fn a_scheduled_wake_runs_once_on_its_host() {
    let (_temp, core) = core().await;
    // Hosted here, so it's this device's to wake (and the other not).
    let here = op(
        &core,
        Caller::Local,
        json!({"op":"createOrchestrator","name":"Here","deviceId":core.device_id}),
    )
    .await
    .unwrap();
    let away = orchestrator(&core, "Away").await;
    for id in [&here, &away] {
        op(
            &core,
            Caller::Local,
            json!({"op":"updateOrchestrator","id":id,"wake":{"everyMinutes":30,"prompt":"check in"}}),
        )
        .await
        .unwrap();
    }
    let rpc = core.rpc_service();
    let now = chrono::Utc::now();
    assert_eq!(
        super::wake::wake_due(&core.orchestration, &rpc, now),
        vec![here.clone()]
    );
    assert!(
        super::wake::wake_due(&core.orchestration, &rpc, now).is_empty(),
        "once"
    );
    assert!(
        core.orchestration
            .orchestrator(&here)
            .unwrap()
            .wake
            .unwrap()
            .last_woke_at
            .is_some()
    );
    assert!(
        core.orchestration
            .orchestrator(&away)
            .unwrap()
            .wake
            .unwrap()
            .last_woke_at
            .is_none()
    );
    core.sessions.shutdown().await;
}

#[tokio::test]
async fn loads_include_this_device() {
    let (_temp, core) = core().await;
    let loads = call(&core, Caller::Local, methods::WATCH_DEVICE_LOADS, json!({}))
        .await
        .unwrap();
    let loads: Vec<DeviceLoad> = serde_json::from_value(loads).unwrap();
    let me = loads
        .iter()
        .find(|l| l.device_id == core.device_id)
        .unwrap();
    assert!(me.online);
    core.sessions.shutdown().await;
}

#[tokio::test]
async fn the_watch_publishes_each_change_coalesced() {
    let (_temp, core) = core().await;
    let mut rx = core.orchestration.subscribe();
    rx.mark_unchanged();
    for name in ["A", "B", "C"] {
        orchestrator(&core, name).await;
    }
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            rx.changed().await.unwrap();
            if rx.borrow_and_update().orchestrators.len() == 3 {
                break;
            }
        }
    })
    .await
    .expect("the watch caught up");
    core.sessions.shutdown().await;
}

/// An agent's crew is only threads it started; the person adds any plain one.
#[tokio::test]
async fn an_agents_crew_is_only_threads_it_started() {
    let (_temp, core) = core().await;
    let auth = orchestrator(&core, "Auth").await;
    let billing = orchestrator(&core, "Billing").await;
    chat(&core, "mine-top", "My chat", None).await;
    chat(&core, "someones-sub", "Sub", Some(("mine-top", "subagent"))).await;
    chat(&core, "its-own", "Own", Some((&auth, "subagent"))).await;
    let voice = format!("{}x", zeron_proto::voice::ORCHESTRATOR_CHAT_PREFIX);
    let fake_brain = format!("{BRAIN_DUMP_CHAT_PREFIX}x");
    chat(&core, &voice, "Voice", None).await;
    chat(&core, &fake_brain, "Fake", None).await;
    let brain_dump = {
        op(&core, Caller::Local, json!({"op":"dump","text":"x"}))
            .await
            .unwrap();
        core.orchestration
            .state()
            .orchestrators
            .into_iter()
            .find(|o| o.kind == OrchestratorKind::BrainDump)
            .unwrap()
            .id
    };
    let add = |chat: &str| json!({"op":"addToCrew","orchestratorId":auth,"chatId":chat});
    for refused in [
        "mine-top",
        "someones-sub",
        voice.as_str(),
        fake_brain.as_str(),
        brain_dump.as_str(),
        billing.as_str(),
    ] {
        assert!(
            op(&core, agent(&auth), add(refused)).await.is_err(),
            "{refused}"
        );
        assert!(core.orchestration.crew_of(refused).is_none(), "{refused}");
    }
    // The person's own chat keeps its place in their sidebar.
    assert!(
        core.workspace
            .chat("mine-top")
            .unwrap()
            .unwrap()
            .is_top_level()
    );
    // Its own child joins.
    op(&core, agent(&auth), add("its-own")).await.unwrap();
    assert_eq!(
        core.orchestration
            .crew_of("its-own")
            .unwrap()
            .orchestrator_id,
        auth
    );

    // A crew create of a chat that exists is no way to add it.
    let rpc = core.rpc_service();
    let crew_create = |id: &str| {
        json!({"op":"createChat","chatId":id,"deviceId":REMOTE,"parentChatId":auth,"childKind":"crew",
            "config":{"harness":"codex","sandbox":"workspace-write"}})
    };
    assert!(
        with_caller(
            agent(&auth),
            rpc.handle(methods::MUTATE, crew_create("mine-top"))
        )
        .await
        .is_err()
    );
    assert!(core.orchestration.crew_of("mine-top").is_none());
    assert!(
        with_caller(
            agent(&auth),
            rpc.handle(methods::MUTATE, crew_create(&voice))
        )
        .await
        .is_err()
    );
    with_caller(
        agent(&auth),
        rpc.handle(methods::MUTATE, crew_create("fresh")),
    )
    .await
    .unwrap();
    assert_eq!(
        core.orchestration.crew_of("fresh").unwrap().orchestrator_id,
        auth
    );

    // The person may still add any plain thread, but not an orchestrator,
    // Zeron's own chats, or another chat's delegate.
    op(&core, Caller::Local, add("mine-top")).await.unwrap();
    assert_eq!(
        core.orchestration
            .crew_of("mine-top")
            .unwrap()
            .orchestrator_id,
        auth
    );
    for refused in [
        voice.as_str(),
        fake_brain.as_str(),
        brain_dump.as_str(),
        billing.as_str(),
        "someones-sub",
    ] {
        assert!(
            op(&core, Caller::Local, add(refused)).await.is_err(),
            "{refused}"
        );
    }
    core.sessions.shutdown().await;
}
