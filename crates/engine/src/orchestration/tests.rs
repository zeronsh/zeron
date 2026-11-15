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
    assert_eq!(sent(&core, &brain), vec![zeron_proto::orchestration::prompt_text::dump_message(&dump, &text)]);
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
            "intent":"task","message":"Update the docs",
            "candidates":[{"kind":"orchestrator","id":auth},{"kind":"chat","id":"notes-chat"}]}),
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
    // The unsure one asks the person where it goes.
    let where_ask = core
        .workspace
        .read_orchestration_with(|doc| doc.read_asks())
        .into_iter()
        .find(|a| a.route_id.as_deref() == Some(unsure.as_str()))
        .unwrap();
    assert_eq!(where_ask.state, AskState::Open);
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
    // Moving it settles its ask.
    let settled = core.workspace.read_orchestration_with(|doc| doc.ask(&where_ask.id)).unwrap();
    assert_eq!(settled.state, AskState::Answered);
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

/// A crew thread named by the orchestrator that starts it carries its
/// title from its first write: before any turn, rename or message, the
/// chat list the Map reads (WatchChats) names it, under its orchestrator,
/// and the crew lists it.
#[tokio::test]
async fn a_just_created_crew_thread_is_named_in_the_chat_list() {
    let (_temp, core) = core().await;
    let fixes = orchestrator(&core, "Zeron fixes").await;
    let rpc = core.rpc_service();
    with_caller(
        agent(&fixes),
        rpc.handle(
            methods::MUTATE,
            json!({"op":"createChat","chatId":"asks","deviceId":REMOTE,"parentChatId":fixes,
                "childKind":"crew","title":"  End-of-turn question asks ",
                "config":{"harness":"codex","sandbox":"workspace-write"}}),
        ),
    )
    .await
    .unwrap();
    // The row as written, and as the chat list's watch has it now.
    let row = core.workspace.chat("asks").unwrap().unwrap();
    assert_eq!(row.title.as_deref(), Some("End-of-turn question asks"));
    assert_eq!(row.last_message_at, None, "no turn, no message");
    // (The list publishes as the registry is read again: the first time
    // it holds the thread, it names it.)
    let mut rx = core.workspace.watch_chats();
    let listed = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if let Some(c) = rx.borrow_and_update().iter().find(|c| c.id == "asks") {
                return c.clone();
            }
            rx.changed().await.unwrap();
        }
    })
    .await
    .expect("listed");
    assert_eq!(listed.title.as_deref(), Some("End-of-turn question asks"));
    let frame = call(&core, Caller::Local, methods::WATCH_CHATS, json!({}))
        .await
        .unwrap();
    let wire = frame
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "asks")
        .cloned()
        .expect("in WatchChats");
    assert_eq!(wire["title"], "End-of-turn question asks");
    assert_eq!(wire["parentChatId"], json!(fixes));
    assert_eq!(wire["childKind"], "crew");
    assert_eq!(core.orchestration.crew_of("asks").unwrap().orchestrator_id, fixes);

    // A blank title is none (the app names it otherwise).
    with_caller(
        agent(&fixes),
        rpc.handle(
            methods::MUTATE,
            json!({"op":"createChat","chatId":"blank","deviceId":REMOTE,"parentChatId":fixes,
                "childKind":"crew","title":"   ","config":{"harness":"codex","sandbox":"workspace-write"}}),
        ),
    )
    .await
    .unwrap();
    assert_eq!(core.workspace.chat("blank").unwrap().unwrap().title, None);
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

// ── Turn-end asks ───────────────────────────────────────────────────────────

/// The final message of thread f3d1b425 ("Codex Slash Commands Missing"),
/// as it ended its turn: work done, then a decision for the person in plain
/// text. It went back to the Brain dump as an ordinary message; nothing
/// raised an ask.
const CODEX_FINAL: &str = "The test helper `text()` turns nil into `\"\"`. The field really is nil, and the Luau side ignores `\"\"` either way. Fixing the assertion:\n\nAll green. Cleaning up the scratch build dir, then replying to Brain dump.\n\nI fixed the Codex `/` menu: a Codex chat now lists `/compact`, `/review` and Codex's skills, then Zeron's own commands. The change is tested but not committed, and the running app needs a rebuild to pick it up. I sent the same summary to Brain dump.\n\n**Cause.** The engine and Codex were fine. Queried live, the engine returns Codex's two commands and all its skills. The problem was in the zgx composer, in two places:\n1. **Wrong agent lookup.** The composer worked out which agent to ask only by finding the chat's model in the local model catalog. That lookup finds nothing for a Codex chat on the default model (no model set) or one hosted on another device. The result was \"no agent\", which means Zeron's commands only. Claude chats usually name a model the catalog lists, so they worked.\n2. **Skills kept out of `/`.** The \"Separate / commands\" setting is on by default for every agent, so Codex's skills appeared only under `$`. Claude Code doesn't hit this because it reports its skills as slash commands.\n\n**Fix.** It's on branch `fix/codex-slash-commands`, based on `zeron/zgx`, in the worktree `~/Documents/dev/zeron-wt-codex-slash`. Your local `main` is 423 commits behind upstream and isn't what the app runs, so I didn't base it there.\n- Each chat now also reports the agent its own settings record (`apps/zgx/src/chat.rs`). All four composers fall back to that agent when the catalog doesn't list the chat's model. The four are the chat page, the side lanes, and the same two in the zeron-v2 template.\n- Codex now lists its skills in `/` by default (`prefs.luau`). `$` still works, and the per-agent setting can turn this back off.\n\n**Tests:**\n- The new UI test fails without the fix and passes with it.\n- The full UI suite: 1358 passed, 1 failed. The failure (\"long messages open folded\") fails the same way without my changes.\n- The related Rust tests pass and lint is clean on the changed files.\n\n`/compact` and `/review` are the only Codex slash commands that work through Zeron. Codex's other built-in commands, like `/model` and `/status`, aren't available through `codex app-server`, and you have no custom prompts in `~/.codex/prompts`.\n\nShould I commit and open a PR against `zeron/zgx`, or merge it into `zeron/orchestrators`?";

const CODEX_THREAD: &str = "f3d1b425-0de7-4b32-a511-b18ab99e7300";
const CODEX_OPTIONS: [&str; 2] = ["Commit and open a PR against zeron/zgx", "Merge into zeron/orchestrators"];

fn said(id: &str, role: zeron_doc::MessageRole, text: &str) -> zeron_doc::SessionMessageEntry {
    zeron_doc::SessionMessageEntry {
        id: id.into(),
        role,
        parts: vec![zeron_doc::MessagePart::Text { id: format!("{id}-t"), text: text.into() }],
        created_at: chrono::Utc::now().timestamp_millis(),
        device_id: REMOTE.into(),
        status: Some(zeron_doc::MessageStatus::Complete),
        continuation_of: None,
        duration_ms: None,
    }
}

fn push(core: &crate::EngineCore, chat: &str, id: &str, role: zeron_doc::MessageRole, text: &str) {
    core.doc_host.open(chat).unwrap().doc().push_message(&said(id, role, text)).unwrap();
}

fn idle(chat: &str) -> zeron_proto::Session {
    zeron_proto::Session {
        last_completed_turn: Some("t".into()),
        chat_id: chat.into(),
        device_id: REMOTE.into(),
        status: zeron_proto::SessionStatus::Idle,
        started_at: None,
        updated_at: chrono::Utc::now(),
        last_turn_outcome: None,
        terminal_activity: None,
        running_subagents: 0,
        background_tasks: vec![],
        waiting: None,
        activity: None,
        todo: None,
    }
}

async fn turn_ended(core: &crate::EngineCore, chat: &str) {
    super::turn_asks::after_turn(&core.orchestration, &core.rpc_service(), chat, &idle(chat)).await;
}

fn asks_from(core: &crate::EngineCore, chat: &str) -> Vec<Ask> {
    core.orchestration.state().asks.into_iter().filter(|a| a.from_chat == chat).collect()
}

/// The Codex thread in "Zeron fixes"'s crew, its turn just ended on its
/// question. The orchestrator's id.
async fn codex_thread(core: &crate::EngineCore) -> String {
    let fixes = orchestrator(core, "Zeron fixes").await;
    chat(core, CODEX_THREAD, "Codex Slash Commands Missing", None).await;
    op(core, Caller::Local, json!({"op":"addToCrew","orchestratorId":fixes,"chatId":CODEX_THREAD}))
        .await
        .unwrap();
    push(core, CODEX_THREAD, "u1", zeron_doc::MessageRole::User, "Codex chats show no slash commands. Fix it.");
    push(core, CODEX_THREAD, "ba10a88f", zeron_doc::MessageRole::Assistant, CODEX_FINAL);
    fixes
}

#[tokio::test]
async fn the_codex_question_becomes_one_ask_and_the_quick_reply_closes_it() {
    let (_temp, core) = core().await;
    let fixes = codex_thread(&core).await;
    turn_ended(&core, CODEX_THREAD).await;
    // The same turn seen again: still one.
    turn_ended(&core, CODEX_THREAD).await;
    let asks = asks_from(&core, CODEX_THREAD);
    assert_eq!(asks.len(), 1, "{asks:#?}");
    let ask = &asks[0];
    assert_eq!((ask.state, ask.origin, ask.kind), (AskState::Open, AskOrigin::TurnEnd, AskKind::Decision));
    assert_eq!(ask.options.iter().map(|o| o.label.as_str()).collect::<Vec<_>>(), CODEX_OPTIONS);
    assert!(ask.question.starts_with("Should I commit and open a PR"), "{}", ask.question);
    assert_eq!((ask.orchestrator_id.as_deref(), ask.turn_id.as_deref()), (Some(fixes.as_str()), Some("ba10a88f")));
    // Its orchestrator got one turn with it, the question as data.
    let told: Vec<_> = sent(&core, &fixes).into_iter().filter(|m| m.starts_with("[Crew ask]")).collect();
    assert_eq!(told.len(), 1, "{told:?}");
    assert!(told[0].contains(&ask.id) && told[0].contains("Merge into zeron/orchestrators"), "{}", told[0]);

    // The person picks an option under Needs you.
    op(&core, Caller::Local, json!({"op":"answerAsk","id":ask.id,"answer":CODEX_OPTIONS[0]})).await.unwrap();
    let ask = &asks_from(&core, CODEX_THREAD)[0];
    assert_eq!((ask.state, ask.answered_via), (AskState::Answered, Some(AnswerVia::Person)));
    assert!(
        sent(&core, CODEX_THREAD).iter().any(|m| m.starts_with("[Answer from you] Should I commit") && m.ends_with(CODEX_OPTIONS[0])),
        "{:?}",
        sent(&core, CODEX_THREAD)
    );
    // The thread's next turn does it: no new ask, nothing reopened.
    push(&core, CODEX_THREAD, "u2", zeron_doc::MessageRole::User, "[Answer from you] … → Commit and open a PR against zeron/zgx");
    push(&core, CODEX_THREAD, "a2", zeron_doc::MessageRole::Assistant, "Opened https://github.com/zeronsh/zeron/pull/1403 against `zeron/zgx`.");
    turn_ended(&core, CODEX_THREAD).await;
    let asks = asks_from(&core, CODEX_THREAD);
    assert_eq!((asks.len(), asks[0].state), (1, AskState::Answered));
    core.sessions.shutdown().await;
}

#[tokio::test]
async fn a_reply_in_the_thread_closes_the_codex_ask() {
    let (_temp, core) = core().await;
    codex_thread(&core).await;
    turn_ended(&core, CODEX_THREAD).await;
    // The person answers in the thread itself (as they did).
    push(&core, CODEX_THREAD, "804423c1", zeron_doc::MessageRole::User, "yes, please open a PR");
    super::turn_asks::on_reply(&core.orchestration, &core.rpc_service(), CODEX_THREAD).await;
    let asks = asks_from(&core, CODEX_THREAD);
    assert_eq!(asks.len(), 1);
    assert_eq!(
        (asks[0].state, asks[0].answered_via, asks[0].answer.as_deref()),
        (AskState::Answered, Some(AnswerVia::Thread), Some("yes, please open a PR"))
    );
    // Its next turn ends without a question: still the one, answered.
    push(&core, CODEX_THREAD, "65e388ba", zeron_doc::MessageRole::Assistant, "I opened the PR against `zeron/zgx`: https://github.com/zeronsh/zeron/pull/1403");
    turn_ended(&core, CODEX_THREAD).await;
    let asks = asks_from(&core, CODEX_THREAD);
    assert_eq!((asks.len(), asks[0].state), (1, AskState::Answered));
    core.sessions.shutdown().await;
}

#[tokio::test]
async fn a_question_asked_again_updates_its_ask() {
    let (_temp, core) = core().await;
    codex_thread(&core).await;
    turn_ended(&core, CODEX_THREAD).await;
    let first = asks_from(&core, CODEX_THREAD)[0].clone();
    // Woken by its build, it asks again in other words: the same ask.
    push(&core, CODEX_THREAD, "a3", zeron_doc::MessageRole::Assistant, "The rebuild finished.\n\nStill need your call: should I open a PR against `zeron/zgx`, or merge into `zeron/orchestrators`?");
    turn_ended(&core, CODEX_THREAD).await;
    let asks = asks_from(&core, CODEX_THREAD);
    assert_eq!(asks.len(), 1, "{asks:#?}");
    assert_eq!((asks[0].id.as_str(), asks[0].turn_id.as_deref()), (first.id.as_str(), Some("a3")));
    assert!(asks[0].updated_at.is_some() && asks[0].question.starts_with("Still need your call"));
    // A reply that didn't settle it, then the same question: reopened.
    push(&core, CODEX_THREAD, "u4", zeron_doc::MessageRole::User, "what's the difference?");
    super::turn_asks::on_reply(&core.orchestration, &core.rpc_service(), CODEX_THREAD).await;
    assert_eq!(asks_from(&core, CODEX_THREAD)[0].state, AskState::Answered);
    push(&core, CODEX_THREAD, "a4", zeron_doc::MessageRole::Assistant, "A PR goes to review first; a merge lands it now.\n\nShould I commit and open a PR against `zeron/zgx`, or merge it into `zeron/orchestrators`?");
    turn_ended(&core, CODEX_THREAD).await;
    let asks = asks_from(&core, CODEX_THREAD);
    assert_eq!((asks.len(), asks[0].state, asks[0].answered_via), (1, AskState::Open, None));
    // Another question is another ask.
    push(&core, CODEX_THREAD, "u5", zeron_doc::MessageRole::User, "open a PR");
    push(&core, CODEX_THREAD, "a5", zeron_doc::MessageRole::Assistant, "Opened PR #1403.\n\nWant me to also delete the old worktree?");
    turn_ended(&core, CODEX_THREAD).await;
    assert_eq!(asks_from(&core, CODEX_THREAD).len(), 2);
    core.sessions.shutdown().await;
}

#[tokio::test]
async fn a_later_turn_that_acts_on_the_ask_closes_it() {
    let (_temp, core) = core().await;
    codex_thread(&core).await;
    turn_ended(&core, CODEX_THREAD).await;
    // No reply, yet its next turn (a background build woke it) merged.
    push(&core, CODEX_THREAD, "a6", zeron_doc::MessageRole::Assistant, "Nobody objected, so I merged it into `zeron/orchestrators` and rebuilt the app.");
    turn_ended(&core, CODEX_THREAD).await;
    let asks = asks_from(&core, CODEX_THREAD);
    assert_eq!(asks.len(), 1);
    assert_eq!(
        (asks[0].state, asks[0].answered_via, asks[0].answer.as_deref()),
        (AskState::Answered, Some(AnswerVia::Action), Some(CODEX_OPTIONS[1]))
    );
    core.sessions.shutdown().await;
}

#[tokio::test]
async fn an_orchestrator_answers_only_its_own_crews_ask() {
    let (_temp, core) = core().await;
    let fixes = codex_thread(&core).await;
    let other = orchestrator(&core, "Billing").await;
    turn_ended(&core, CODEX_THREAD).await;
    let id = asks_from(&core, CODEX_THREAD)[0].id.clone();
    let answer = |who: &str| op(&core, agent(who), json!({"op":"answerAsk","id":id,"answer":CODEX_OPTIONS[1]}));
    assert!(answer(&other).await.is_err(), "not its crew");
    assert!(answer(CODEX_THREAD).await.is_err(), "not the asker");
    assert!(op(&core, agent(&fixes), json!({"op":"dismissAsk","id":id})).await.is_err(), "only the person dismisses");
    // "Control Room work stays local": within its goal, it answers.
    answer(&fixes).await.unwrap();
    let ask = &asks_from(&core, CODEX_THREAD)[0];
    assert_eq!((ask.state, ask.answered_via), (AskState::Answered, Some(AnswerVia::Orchestrator)));
    assert!(
        sent(&core, CODEX_THREAD).iter().any(|m| m.starts_with("[Answer from your orchestrator]") && m.ends_with(CODEX_OPTIONS[1])),
        "{:?}",
        sent(&core, CODEX_THREAD)
    );
    assert!(answer(&fixes).await.is_err(), "once");
    // An ask the thread raised itself is the person's, and so is a turn-end
    // one once the thread left the crew.
    let own = op(&core, agent(CODEX_THREAD), json!({"op":"ask","kind":"decision","question":"Deploy?"})).await.unwrap();
    assert!(op(&core, agent(&fixes), json!({"op":"answerAsk","id":own,"answer":"yes"})).await.is_err(), "its ask tool's");
    op(&core, Caller::Local, json!({"op":"dismissAsk","id":own})).await.unwrap();
    push(&core, CODEX_THREAD, "u9", zeron_doc::MessageRole::User, "next");
    push(&core, CODEX_THREAD, "a9", zeron_doc::MessageRole::Assistant, "Done. Want me to delete the old worktree?");
    turn_ended(&core, CODEX_THREAD).await;
    let late = asks_from(&core, CODEX_THREAD).into_iter().find(|a| a.turn_id.as_deref() == Some("a9")).unwrap().id;
    op(&core, Caller::Local, json!({"op":"releaseFromCrew","chatId":CODEX_THREAD})).await.ok();
    core.orchestration.workspace().write_orchestration(|doc| {
        if let Some(mut m) = doc.crew_member(CODEX_THREAD) {
            m.released = true;
            doc.upsert_crew_member(&m);
        }
    });
    assert!(op(&core, agent(&fixes), json!({"op":"answerAsk","id":late,"answer":"yes"})).await.is_err(), "released");
    core.sessions.shutdown().await;
}

#[tokio::test]
async fn a_stale_ask_is_raised_once() {
    let (_temp, core) = core().await;
    codex_thread(&core).await;
    turn_ended(&core, CODEX_THREAD).await;
    let rpc = core.rpc_service();
    let here = |_: &str| true;
    let now = chrono::Utc::now();
    assert!(super::turn_asks::escalate_with(&core.orchestration, &rpc, now, &here).is_empty(), "fresh");
    let later = now + super::turn_asks::STALE_AFTER + chrono::Duration::minutes(1);
    let elsewhere = |_: &str| false;
    assert!(super::turn_asks::escalate_with(&core.orchestration, &rpc, later, &elsewhere).is_empty(), "its host raises it");
    let raised = super::turn_asks::escalate_with(&core.orchestration, &rpc, later, &here);
    assert_eq!(raised.len(), 1);
    let ask = &asks_from(&core, CODEX_THREAD)[0];
    assert!(ask.urgent && ask.escalated_at == Some(later) && ask.state == AskState::Open);
    let much_later = later + chrono::Duration::hours(3);
    assert!(super::turn_asks::escalate_with(&core.orchestration, &rpc, much_later, &here).is_empty(), "once");
    core.sessions.shutdown().await;
}

#[tokio::test]
async fn only_launched_threads_and_only_real_questions_raise_asks() {
    let (_temp, core) = core().await;
    // The person's own chat: theirs to read.
    chat(&core, "plain", "Plain", None).await;
    push(&core, "plain", "a", zeron_doc::MessageRole::Assistant, "Should I open a PR or merge it?");
    turn_ended(&core, "plain").await;
    assert!(asks_from(&core, "plain").is_empty());
    // A thread the Brain dump started: its asks go to the person only.
    op(&core, Caller::Local, json!({"op":"dump","text":"x"})).await.unwrap();
    let brain = core.orchestration.state().orchestrators.into_iter().find(|o| o.kind == OrchestratorKind::BrainDump).unwrap().id;
    chat(&core, "routed", "Routed", None).await;
    core.rpc_service().mark_started_by("routed", &brain, None, None);
    push(&core, "routed", "u", zeron_doc::MessageRole::User, "fix it");
    push(&core, "routed", "a", zeron_doc::MessageRole::Assistant, "Fixed. Want me to open a PR?");
    turn_ended(&core, "routed").await;
    let asks = asks_from(&core, "routed");
    assert_eq!(asks.len(), 1);
    assert_eq!((asks[0].orchestrator_id.as_deref(), asks[0].options.len()), (None, 2));
    assert!(!sent(&core, &brain).iter().any(|m| m.starts_with("[Crew ask]")));
    // Nothing pending: a closer, or a question it answered itself.
    push(&core, "routed", "u2", zeron_doc::MessageRole::User, "ok");
    push(&core, "routed", "a2", zeron_doc::MessageRole::Assistant, "Pushed. Anything else?");
    turn_ended(&core, "routed").await;
    push(&core, "routed", "u3", zeron_doc::MessageRole::User, "why did CI fail?");
    push(&core, "routed", "a3", zeron_doc::MessageRole::Assistant, "Why did it fail? The cache key ignored the target triple. Fixed.");
    turn_ended(&core, "routed").await;
    assert_eq!(asks_from(&core, "routed").len(), 1);
    // A turn that asked through `ask` itself raises nothing more.
    push(&core, "routed", "u4", zeron_doc::MessageRole::User, "next");
    op(&core, agent("routed"), json!({"op":"ask","kind":"decision","question":"Deploy now?","options":[{"label":"Yes"},{"label":"No"}]}))
        .await
        .unwrap();
    push(&core, "routed", "a4", zeron_doc::MessageRole::Assistant, "Ready. Should I deploy now?");
    turn_ended(&core, "routed").await;
    let asks = asks_from(&core, "routed");
    assert_eq!(asks.iter().filter(|a| a.origin == AskOrigin::TurnEnd).count(), 1, "{asks:#?}");
    core.sessions.shutdown().await;
}

#[tokio::test]
async fn a_borderline_end_goes_to_the_small_model() {
    let (_temp, core) = core().await;
    let fixes = orchestrator(&core, "Ops").await;
    chat(&core, "crew", "Crew", None).await;
    op(&core, Caller::Local, json!({"op":"addToCrew","orchestratorId":fixes,"chatId":"crew"})).await.unwrap();
    let says = std::sync::Arc::new(std::sync::Mutex::new(Some("{\"pending\": false}".to_owned())));
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    {
        let (says, calls) = (says.clone(), calls.clone());
        core.orchestration.progress().set_note_fn(std::sync::Arc::new(move |_, _| {
            calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let said = says.lock().unwrap().clone();
            Box::pin(async move { said })
        }));
    }
    push(&core, "crew", "u", zeron_doc::MessageRole::User, "check the logs");
    push(&core, "crew", "a", zeron_doc::MessageRole::Assistant, "Two restarts at 03:10. Is that the window you meant?");
    turn_ended(&core, "crew").await;
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(asks_from(&core, "crew").is_empty(), "the model said nothing is pending");
    // The model can't be reached: raised anyway, never lost.
    *says.lock().unwrap() = None;
    push(&core, "crew", "u2", zeron_doc::MessageRole::User, "and now?");
    push(&core, "crew", "a2", zeron_doc::MessageRole::Assistant, "One more at 04:00. Is that the window you meant?");
    turn_ended(&core, "crew").await;
    assert_eq!(asks_from(&core, "crew").len(), 1);
    // A plain decision never needs the model.
    push(&core, "crew", "u3", zeron_doc::MessageRole::User, "ok");
    push(&core, "crew", "a3", zeron_doc::MessageRole::Assistant, "Go/no-go on the restart?");
    turn_ended(&core, "crew").await;
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    core.sessions.shutdown().await;
}

#[tokio::test]
async fn a_question_sent_to_the_brain_dump_and_ended_on_is_one_ask() {
    let (_temp, core) = core().await;
    codex_thread(&core).await;
    // The thread sends its summary to the Brain dump (no route asked for
    // it), then ends its turn on the same words, as f3d1b425 did.
    core.rpc_service().brain_update(CODEX_THREAD, CODEX_FINAL);
    let asks = asks_from(&core, CODEX_THREAD);
    assert_eq!(asks.len(), 1, "{asks:#?}");
    assert_eq!(asks[0].options.iter().map(|o| o.label.as_str()).collect::<Vec<_>>(), CODEX_OPTIONS);
    turn_ended(&core, CODEX_THREAD).await;
    let asks = asks_from(&core, CODEX_THREAD);
    assert_eq!(asks.len(), 1, "{asks:#?}");
    assert_eq!((asks[0].state, asks[0].turn_id.as_deref()), (AskState::Open, Some("ba10a88f")));
    // The same message again: nothing new.
    core.rpc_service().brain_update(CODEX_THREAD, CODEX_FINAL);
    assert_eq!(asks_from(&core, CODEX_THREAD).len(), 1);
    core.sessions.shutdown().await;
}

// ── Board cards and asks ────────────────────────────────────────────────────

/// A thread the Brain dump routed `tasks` to, one card each, in order: the
/// dump, the Brain dump, and each route's (id, card).
async fn routed_thread(core: &crate::EngineCore, chat: &str, tasks: &[&str]) -> (String, String, Vec<(String, String)>) {
    self::chat(core, chat, "Redesign Map Graph Mode", None).await;
    let dump = op(core, Caller::Local, json!({"op":"dump","text":tasks.join(". ")})).await.unwrap();
    let brain = core.orchestration.state().orchestrators.into_iter().find(|o| o.kind == OrchestratorKind::BrainDump).unwrap().id;
    let mut routes = Vec::new();
    for task in tasks {
        let route = op(core, agent(&brain), json!({"op":"route","dumpId":dump,"excerpt":task,"target":{"kind":"chat","id":chat},
            "intent":"task","message":task,"title":task})).await.unwrap();
        let item = core.workspace.read_orchestration_with(|d| d.dump(&dump)).unwrap().routes.into_iter().find(|r| r.id == route).unwrap().item_id.unwrap();
        routes.push((route, item));
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    (dump, brain, routes)
}

fn card(core: &crate::EngineCore, item: &str) -> WorkState {
    core.orchestration.item(item).unwrap().state
}

#[tokio::test]
async fn a_thread_with_finished_cards_that_asks_waits_on_its_newest_card_and_none_is_blocked() {
    let (_temp, core) = core().await;
    let map = "9ad131f1-map";
    let (dump, _, routes) = routed_thread(&core, map, &["Fix the Nothing running line", "Fix the Timeline legend", "New Topology variants"]).await;
    let rpc = core.rpc_service();
    // The first is finished; the second has an outcome so far (at work).
    super::watchers::record_outcome(&core.orchestration, &rpc, map, &dump, &routes[0].0, "x", "Fixed the line.".into(), Some(RouteState::Done)).await;
    super::watchers::record_outcome(&core.orchestration, &rpc, map, &dump, &routes[1].0, "x", "Legend fixed.".into(), Some(RouteState::Answered)).await;
    // A quiet chase from the restart is open from the thread too (what
    // blocked every card of it live).
    let chase = Ask {
        id: "chase".into(), from_chat: map.into(), orchestrator_id: None, item_id: None, kind: AskKind::Decision,
        question: "Redesign Map Graph Mode hasn't answered: …".into(),
        options: vec![AskOption::new("Ask again"), AskOption::new("Dismiss")],
        urgent: false, state: AskState::Open, answer: None, created_at: chrono::Utc::now(), answered_at: None,
        dump_id: Some(dump.clone()), route_id: None, quiet: true, origin: AskOrigin::Tool, turn_id: None,
        updated_at: None, escalated_at: None, answered_via: None,
        images: Vec::new(), recommendation: None, links: Vec::new(), context: None, deferred_until: None,
    };
    // Its newest work ends on a question for the person; the chase opened
    // during that turn (an engine's, not the thread's own `ask`).
    push(&core, map, "u3", zeron_doc::MessageRole::User, "New Topology variants");
    let chase = Ask { created_at: chrono::Utc::now() + chrono::Duration::seconds(1), ..chase };
    core.orchestration.workspace().write_orchestration(|doc| doc.upsert_ask(&chase));
    rpc.sync_cards_of(map);
    push(&core, map, "a3", zeron_doc::MessageRole::Assistant, "Four variants are built and scored.\n\nWhich do you want: the inline tag, or the coloured edge?");
    turn_ended(&core, map).await;
    let asks: Vec<_> = asks_from(&core, map).into_iter().filter(|a| a.origin == AskOrigin::TurnEnd).collect();
    assert_eq!(asks.len(), 1, "{asks:#?}");
    // One card waits on the person: the one the asking turn's work is on.
    assert_eq!(asks[0].item_id.as_deref(), Some(routes[2].1.as_str()));
    assert_eq!(
        [card(&core, &routes[0].1), card(&core, &routes[1].1), card(&core, &routes[2].1)],
        [WorkState::Done, WorkState::Active, WorkState::Active],
        "finished stays finished, an outcome so far stays at work, none Blocked"
    );
    // A real failure is still Blocked.
    super::watchers::record_outcome(&core.orchestration, &rpc, map, &dump, &routes[2].0, "x", "The build broke.".into(), Some(RouteState::Failed)).await;
    assert_eq!(card(&core, &routes[2].1), WorkState::Blocked);
    assert_eq!(card(&core, &routes[0].1), WorkState::Done);
    core.sessions.shutdown().await;
}

#[tokio::test]
async fn cards_an_ask_once_blocked_are_restated_from_their_routes_once() {
    let (_temp, core) = core().await;
    let map = "9ad131f1-map";
    let (dump, _, routes) = routed_thread(&core, map, &["Fix the Timeline legend", "Every card in Now", "Rebuild the map"]).await;
    let rpc = core.rpc_service();
    super::watchers::record_outcome(&core.orchestration, &rpc, map, &dump, &routes[0].0, "x", "Legend fixed.".into(), Some(RouteState::Done)).await;
    super::watchers::record_outcome(&core.orchestration, &rpc, map, &dump, &routes[2].0, "x", "It broke.".into(), Some(RouteState::Failed)).await;
    // As the live engine left them: Blocked by an ask, though done or at work.
    for (_, item) in &routes[..2] {
        core.orchestration
            .patch_item(WorkItemPatch { id: Some(item.clone()), state: Some(WorkState::Blocked), ..Default::default() })
            .unwrap();
    }
    let (mut restated, kept) = rpc.settle_card_states().await;
    restated.sort();
    assert!(kept.is_empty());
    let mut want = vec![routes[0].1.clone(), routes[1].1.clone()];
    want.sort();
    assert_eq!(restated, want);
    assert_eq!(
        [card(&core, &routes[0].1), card(&core, &routes[1].1), card(&core, &routes[2].1)],
        [WorkState::Done, WorkState::Active, WorkState::Blocked],
        "the failed one stays Blocked"
    );
    assert_eq!(rpc.settle_card_states().await, (vec![], vec![]), "idempotent");
    core.sessions.shutdown().await;
}

#[tokio::test]
async fn a_card_the_person_moves_stays_until_its_work_changes() {
    let (_temp, core) = core().await;
    let map = "9ad131f1-map";
    let (dump, _, routes) = routed_thread(&core, map, &["Fix the Timeline legend", "Rebuild the map"]).await;
    let rpc = core.rpc_service();
    let (legend, rebuild) = (&routes[0], &routes[1]);
    // A loose card (no orchestrator) whose work failed: Blocked.
    assert_eq!(core.orchestration.item(&legend.1).unwrap().orchestrator_id, None, "loose");
    super::watchers::record_outcome(&core.orchestration, &rpc, map, &dump, &legend.0, "x", "It broke.".into(), Some(RouteState::Failed)).await;
    assert_eq!(card(&core, &legend.1), WorkState::Blocked);
    // The person moves it to Done (the board's Move to, or a drag).
    op(&core, Caller::Local, json!({"op":"upsertItem","item":{"id":legend.1,"state":"done"}})).await.unwrap();
    assert_eq!(card(&core, &legend.1), WorkState::Done);
    let moved = core.orchestration.item(&legend.1).unwrap();
    assert!(moved.moved_at.is_some() && moved.moved_route_state == Some(RouteState::Failed));
    // Re-synced, and the start's pass: it stays Done.
    rpc.sync_route_card(&dump, &legend.0);
    rpc.sync_cards_of(map);
    assert_eq!(rpc.settle_card_states().await, (vec![], vec![]));
    assert_eq!(card(&core, &legend.1), WorkState::Done);

    // Another card, at work (an outcome so far), moved to Done; then its
    // work really fails.
    super::watchers::record_outcome(&core.orchestration, &rpc, map, &dump, &rebuild.0, "x", "Rebuilding; tests next.".into(), Some(RouteState::Answered)).await;
    assert_eq!(card(&core, &rebuild.1), WorkState::Active);
    op(&core, Caller::Local, json!({"op":"upsertItem","item":{"id":rebuild.1,"state":"done"}})).await.unwrap();
    rpc.sync_cards_of(map);
    assert_eq!(card(&core, &rebuild.1), WorkState::Done, "kept while nothing new happens");
    super::watchers::record_outcome(&core.orchestration, &rpc, map, &dump, &rebuild.0, "x", "The rebuild failed.".into(), Some(RouteState::Failed)).await;
    assert_eq!(card(&core, &rebuild.1), WorkState::Blocked, "a new failure still shows");
    assert_eq!(core.orchestration.item(&rebuild.1).unwrap().moved_at, None, "the move is spent");
    core.sessions.shutdown().await;
}

#[tokio::test]
async fn a_card_set_by_hand_before_the_mark_existed_is_kept_at_start() {
    let (_temp, core) = core().await;
    let map = "9ad131f1-map";
    let (dump, _, routes) = routed_thread(&core, map, &["Feedback on the Timeline legend"]).await;
    let rpc = core.rpc_service();
    super::watchers::record_outcome(&core.orchestration, &rpc, map, &dump, &routes[0].0, "x", "Fixed.".into(), Some(RouteState::Done)).await;
    // Set by hand on an older engine (no mark): back in Now though its
    // route is done. (One left in Review is re-judged instead: the Review
    // test below.)
    core.orchestration
        .patch_item(WorkItemPatch { id: Some(routes[0].1.clone()), state: Some(WorkState::Active), ..Default::default() })
        .unwrap();
    assert_eq!(rpc.settle_card_states().await, (vec![], vec![routes[0].1.clone()]));
    rpc.sync_cards_of(map);
    assert_eq!(card(&core, &routes[0].1), WorkState::Active, "a later sync keeps it");
    assert_eq!(rpc.settle_card_states().await, (vec![], vec![]), "once");
    core.sessions.shutdown().await;
}

/// Crew auto-archive against a whole engine: the settings op (the person's
/// alone), an idle thread archived through the chat row as the person's
/// Archive does, its card keeping its column with the thread a chat link,
/// the archive noted, and an unarchive starting its idle time again.
#[tokio::test]
async fn idle_crew_threads_are_archived_and_an_unarchive_starts_over() {
    use super::crew_archive::{Step, apply, plan};
    let (_temp, core) = core().await;
    let auth = orchestrator(&core, "Auth").await;

    // The settings: defaults, changed by the person, refused to an agent.
    let state = core.workspace.orchestration_state();
    assert_eq!((state.settings.crew_auto_archive, state.settings.crew_idle_hours), (true, 12));
    let reply = call(&core, Caller::Local, methods::ORCHESTRATE,
        json!({"op":"updateSettings","crewIdleHours":6})).await.unwrap();
    assert_eq!(reply["crewIdleHours"], 6);
    assert!(op(&core, agent(&auth), json!({"op":"updateSettings","crewAutoArchive":false})).await.is_err());
    assert!(op(&core, Caller::Local, json!({"op":"updateSettings","crewIdleHours":0})).await.is_err(), "out of bounds");
    let settings = core.workspace.orchestration_state().settings;
    assert_eq!((settings.crew_auto_archive, settings.crew_idle_hours), (true, 6));

    // A crew thread on a card in Review (done working, waiting on merge).
    let item = op(&core, agent(&auth), json!({"op":"upsertItem","item":{"title":"Login redirect","state":"review"}}))
        .await
        .unwrap();
    chat(&core, "worker", "Worker", Some((auth.as_str(), "crew"))).await;
    op(&core, Caller::Local, json!({"op":"addToCrew","orchestratorId":auth,"chatId":"worker","role":"fix","itemId":item}))
        .await
        .unwrap();
    assert!(core.orchestration.item(&item).unwrap().threads.contains(&"worker".to_owned()));

    let chats = || core.workspace.read_chats().unwrap();
    let st = || core.workspace.orchestration_state();
    let now = chrono::Utc::now();
    let later = now + chrono::Duration::hours(7);
    assert!(plan(now, REMOTE, &chats(), &st(), |_| None).is_empty(), "not idle yet");
    let steps = plan(later, REMOTE, &chats(), &st(), |_| None);
    assert!(matches!(steps.as_slice(), [Step::Archive { chat_id, .. }] if chat_id == "worker"), "{steps:?}");
    // The orchestrator's own chat isn't among them.
    assert!(!steps.iter().any(|s| matches!(s, Step::Archive { chat_id, .. } if *chat_id == auth)));
    assert_eq!(apply(&core.orchestration, &steps, later), 1);

    assert!(core.workspace.chat("worker").unwrap().unwrap().archived, "archived as the person's Archive does");
    let card = core.orchestration.item(&item).unwrap();
    assert_eq!(card.state, WorkState::Review, "its column kept");
    assert!(!card.threads.contains(&"worker".to_owned()));
    assert!(card.links.iter().any(|l| l.kind == LinkKind::Chat && l.target == "worker" && l.label == "Worker"));
    assert!(core.orchestration.crew_of("worker").unwrap().archived_seen_at.is_some());
    // Again: nothing more to do (idempotent).
    assert!(plan(later, REMOTE, &chats(), &st(), |_| None).is_empty());

    // The person unarchives it: revived, and kept a whole window from then.
    call(&core, Caller::Local, methods::MUTATE, json!({"op":"setChatArchived","chatId":"worker","archived":false}))
        .await
        .unwrap();
    let revive_at = later + chrono::Duration::hours(1);
    let steps = plan(revive_at, REMOTE, &chats(), &st(), |_| None);
    assert_eq!(steps, vec![Step::Revived { chat_id: "worker".into() }]);
    apply(&core.orchestration, &steps, revive_at);
    let member = core.orchestration.crew_of("worker").unwrap();
    assert_eq!((member.archived_seen_at, member.revived_at), (None, Some(revive_at)));
    assert!(plan(revive_at + chrono::Duration::hours(5), REMOTE, &chats(), &st(), |_| None).is_empty());
    assert!(!plan(revive_at + chrono::Duration::hours(6), REMOTE, &chats(), &st(), |_| None).is_empty());

    // The setting off: nothing archived, however idle.
    op(&core, Caller::Local, json!({"op":"updateSettings","crewAutoArchive":false})).await.ok();
    assert!(plan(revive_at + chrono::Duration::hours(100), REMOTE, &chats(), &st(), |_| None).is_empty());
}

// ── Review means the person ─────────────────────────────────────────────────

async fn outcome(core: &crate::EngineCore, chat: &str, dump: &str, route: &str, text: &str, state: RouteState) {
    super::watchers::record_outcome(&core.orchestration, &core.rpc_service(), chat, dump, route, "x", text.into(), Some(state)).await;
}

fn card_outcome_of(core: &crate::EngineCore, item: &str) -> String {
    core.orchestration.item(item).unwrap().outcome.unwrap_or_default()
}

#[tokio::test]
async fn review_holds_only_what_waits_on_the_person() {
    let (_temp, core) = core().await;
    let map = "9ad131f1-map";
    let (dump, _, r) = routed_thread(
        &core,
        map,
        &["Map device rename not reflected", "Graph map: stop zoom reflow jitter", "Open a PR for the slash fix", "Topology variants", "Graph map: curved lines", "Rebuild the app"],
    )
    .await;
    // A turn ends while the thread still works (background build, todo
    // left): Now, never Review.
    outcome(&core, map, &dump, &r[0].0, "Renamed in the heading; now wiring Topology and the hover card.", RouteState::Answered).await;
    assert_eq!(card(&core, &r[0].1), WorkState::Active);
    // A mid-work progress message to the Brain dump: Now.
    core.rpc_service().file_with_brain(map, "Working on the zoom jitter: the reflow comes from the fit pass; testing a fix.");
    for _ in 0..100 {
        if core.workspace.read_orchestration_with(|d| d.dump(&dump)).unwrap().routes.iter().any(|x| x.state == RouteState::Answered && x.id != r[0].0) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(r[..2].iter().all(|(_, item)| card(&core, item) == WorkState::Active), "mid-work: Now");
    // Finished with an open PR: Review, the PR first.
    outcome(&core, map, &dump, &r[2].0, "I opened the PR against `zeron/zgx`: https://github.com/zeronsh/zeron/pull/1403\n\nNot merged yet.", RouteState::Done).await;
    assert_eq!(card(&core, &r[2].1), WorkState::Review);
    assert!(card_outcome_of(&core, &r[2].1).starts_with("Review: PR https://github.com/zeronsh/zeron/pull/1403\n\n"), "{}", card_outcome_of(&core, &r[2].1));
    // Finished on a decision for the person: not finished; Now, and the
    // question is one ask, on this card.
    outcome(&core, map, &dump, &r[3].0, "Four variants are built and scored.\n\nWhich do you want: the inline tag, or the coloured edge?", RouteState::Done).await;
    assert_eq!(card(&core, &r[3].1), WorkState::Active);
    let asks: Vec<_> = asks_from(&core, map).into_iter().filter(|a| a.state == AskState::Open).collect();
    assert_eq!(asks.len(), 1, "{asks:#?}");
    assert_eq!(asks[0].item_id.as_deref(), Some(r[3].1.as_str()));
    assert_eq!(asks[0].options.len(), 2);
    // Finished with nothing for the person: Done.
    outcome(&core, map, &dump, &r[4].0, "Messages now follow the curved lines; the 31 map tests pass. Committed as a1b2c3d.", RouteState::Done).await;
    assert_eq!(card(&core, &r[4].1), WorkState::Done);
    // (A design that landed, with its screenshots: Review, where they are.)
    let (dump2, _, s) = routed_thread(&core, "packed-map", &["Graph map: area scales with content"]).await;
    outcome(&core, "packed-map", &dump2, &s[0].0, "Packed layout is in behind a switch.\n\nScreenshots are in `~/Documents/dev/tmp/orch/ui-map/shots/`.", RouteState::Done).await;
    assert_eq!(card(&core, &s[0].1), WorkState::Review);
    assert!(card_outcome_of(&core, &s[0].1).starts_with("Review: Screenshots: ~/Documents/dev/tmp/orch/ui-map/shots/"));
    // Failed: Blocked.
    outcome(&core, map, &dump, &r[5].0, "The build failed: linker error.", RouteState::Failed).await;
    assert_eq!(card(&core, &r[5].1), WorkState::Blocked);
    // A card the person moves stays: the PR one to Done.
    op(&core, Caller::Local, json!({"op":"upsertItem","item":{"id":r[2].1,"state":"done"}})).await.unwrap();
    core.rpc_service().sync_cards_of(map);
    assert_eq!(card(&core, &r[2].1), WorkState::Done);
    core.sessions.shutdown().await;
}

#[tokio::test]
async fn the_start_restates_cards_the_old_rule_left_in_review() {
    let (_temp, core) = core().await;
    let map = "9ad131f1-map";
    let (dump, _, r) = routed_thread(&core, map, &["Map device rename", "Changes panel diff scroll", "Timeline legend", "Topology variants", "Mine to keep"]).await;
    let rpc = core.rpc_service();
    // As the old rule left them: an outcome so far (thread state unknown
    // here), a finished PR, a finished nothing, a finished decision the
    // Brain dump put in Review by hand, and one the person moved there.
    outcome(&core, map, &dump, &r[0].0, "Renamed in the heading; now wiring Topology.", RouteState::Answered).await;
    outcome(&core, map, &dump, &r[1].0, "**PR #1402 is open** and waiting for review/merge: https://github.com/zeronsh/zgx/pull/1402", RouteState::Done).await;
    outcome(&core, map, &dump, &r[2].0, "Legend fixed; tests pass.", RouteState::Done).await;
    outcome(&core, map, &dump, &r[3].0, "Variants built.", RouteState::Done).await;
    outcome(&core, map, &dump, &r[4].0, "Done.", RouteState::Done).await;
    // (Old routes carried no review: the PR one's is cleared to match.)
    core.orchestration.workspace().write_orchestration(|doc| {
        let mut d = doc.dump(&dump).unwrap();
        for route in &mut d.routes {
            route.review = None;
        }
        doc.upsert_dump(&d);
    });
    for (_, item) in &r[..4] {
        core.orchestration.patch_item(WorkItemPatch { id: Some(item.clone()), state: Some(WorkState::Review), ..Default::default() }).unwrap();
    }
    core.orchestration
        .patch_item(WorkItemPatch { id: Some(r[3].1.clone()), outcome: Some(Some("Waiting on your pick (built on orch/topo-variants). Recommend (a).".into())), ..Default::default() })
        .unwrap();
    op(&core, Caller::Local, json!({"op":"upsertItem","item":{"id":r[4].1,"state":"review"}})).await.unwrap();

    let (restated, _) = rpc.settle_card_states().await;
    assert_eq!(
        [card(&core, &r[0].1), card(&core, &r[1].1), card(&core, &r[2].1), card(&core, &r[3].1), card(&core, &r[4].1)],
        [WorkState::Active, WorkState::Review, WorkState::Done, WorkState::Active, WorkState::Review],
        "working → Now; PR stays Review; a decision → Now with its ask; nothing → Done; moved by hand kept"
    );
    assert!(card_outcome_of(&core, &r[1].1).starts_with("Review: PR https://github.com/zeronsh/zgx/pull/1402"));
    // The card waiting on a decision got the ask it never had, on it.
    let asks: Vec<_> = asks_from(&core, map).into_iter().filter(|a| a.state == AskState::Open).collect();
    assert_eq!(asks.len(), 1, "{asks:#?}");
    assert_eq!(asks[0].item_id.as_deref(), Some(r[3].1.as_str()));
    assert!(asks[0].question.contains("Waiting on your pick"));
    assert!(restated.contains(&r[0].1) && restated.contains(&r[2].1));
    assert_eq!(rpc.settle_card_states().await, (vec![], vec![]), "once");
    core.sessions.shutdown().await;
}

#[tokio::test]
async fn approve_closes_a_review_and_request_changes_sends_it_back_to_work() {
    let (_temp, core) = core().await;
    let map = "9ad131f1-map";
    let (dump, _, r) = routed_thread(&core, map, &["Open a PR for the slash fix", "Graph map: area scales with content"]).await;
    outcome(&core, map, &dump, &r[0].0, "Opened https://github.com/zeronsh/zeron/pull/1403, not merged yet.", RouteState::Done).await;
    outcome(&core, map, &dump, &r[1].0, "Packed layout landed.\n\nScreenshots are in `~/x/shots/`.", RouteState::Done).await;
    assert_eq!([card(&core, &r[0].1), card(&core, &r[1].1)], [WorkState::Review, WorkState::Review]);
    // Only the person.
    let brain = core.orchestration.state().orchestrators.into_iter().find(|o| o.kind == OrchestratorKind::BrainDump).unwrap().id;
    assert!(op(&core, agent(&brain), json!({"op":"approveItem","id":r[0].1})).await.is_err());
    assert!(op(&core, agent(&brain), json!({"op":"requestChanges","id":r[1].1,"text":"x"})).await.is_err());
    // Approve: Done, recorded, and it stays Done.
    op(&core, Caller::Local, json!({"op":"approveItem","id":r[0].1})).await.unwrap();
    let approved = core.orchestration.item(&r[0].1).unwrap();
    assert_eq!(approved.state, WorkState::Done);
    assert!(approved.approved_at.is_some());
    core.rpc_service().sync_cards_of(map);
    assert_eq!(rpc_settle(&core).await, (vec![], vec![]));
    assert_eq!(card(&core, &r[0].1), WorkState::Done);
    // Request changes: an empty one is refused; the text goes to the
    // thread, and the card is back in Now.
    assert!(op(&core, Caller::Local, json!({"op":"requestChanges","id":r[1].1,"text":"  "})).await.is_err());
    op(&core, Caller::Local, json!({"op":"requestChanges","id":r[1].1,"text":"Islands still overlap at 900px; give empty machines a corner strip."})).await.unwrap();
    assert_eq!(card(&core, &r[1].1), WorkState::Active);
    assert!(
        sent(&core, map).iter().any(|m| m == "[Changes requested on \"Graph map: area scales with content\"] Islands still overlap at 900px; give empty machines a corner strip."),
        "{:?}",
        sent(&core, map)
    );
    // Its next finished turn is judged afresh.
    outcome(&core, map, &dump, &r[1].0, "Fixed the overlap; committed.", RouteState::Done).await;
    assert_eq!(card(&core, &r[1].1), WorkState::Done);
    core.sessions.shutdown().await;
}

async fn rpc_settle(core: &crate::EngineCore) -> (Vec<String>, Vec<String>) {
    core.rpc_service().settle_card_states().await
}

#[tokio::test]
async fn a_thread_on_an_older_host_is_asked_for_by_the_engine_that_launched_it() {
    let (_temp, core) = core().await;
    // 9ad131f1 lives on another engine (a 0.3.2 host with no turn-end
    // asks): a dump of this engine routed to it, so this one raises its
    // questions; a chat nothing here launched is left alone.
    let (_dump, _, _) = routed_thread(&core, "9ad131f1-map", &["Map machine headings: decide R9"]).await;
    chat(&core, "someone-else", "Elsewhere", None).await;
    let dumps = core.orchestration.state().dumps;
    assert!(super::watchers::launched_here(&core.orchestration, &dumps, "9ad131f1-map"));
    assert!(!super::watchers::launched_here(&core.orchestration, &dumps, "someone-else"));
    // Its turn ends there on a decision; raised here once, however often
    // it is seen (the ask's id is the chat and the asking message).
    push(&core, "9ad131f1-map", "u1", zeron_doc::MessageRole::User, "Map machine headings");
    push(&core, "9ad131f1-map", "a1", zeron_doc::MessageRole::Assistant, "Round 9 is merged locally.\n\nThe graph-label decision (accept round 9 or keep iterating) is still waiting on you.");
    turn_ended(&core, "9ad131f1-map").await;
    turn_ended(&core, "9ad131f1-map").await;
    assert_eq!(asks_from(&core, "9ad131f1-map").len(), 1);
    core.sessions.shutdown().await;
}

/// The Brain dump's id (after a dump made it).
fn brain_of(core: &crate::EngineCore) -> String {
    core.orchestration
        .state()
        .orchestrators
        .into_iter()
        .find(|o| o.kind == OrchestratorKind::BrainDump)
        .unwrap()
        .id
}

fn route_row(core: &crate::EngineCore, dump: &str, route: &str) -> Route {
    core.workspace
        .read_orchestration_with(|d| d.dump(dump))
        .unwrap()
        .routes
        .into_iter()
        .find(|r| r.id == route)
        .unwrap()
}

#[tokio::test]
async fn a_reply_on_an_ask_reference_answers_the_ask() {
    let (_temp, core) = core().await;
    chat(&core, "asker", "Asker", None).await;
    let ask = op(
        &core,
        agent("asker"),
        json!({"op":"ask","kind":"decision","question":"Merge now or wait?","options":[{"label":"Merge"},{"label":"Wait"}]}),
    )
    .await
    .unwrap();
    let dump = op(
        &core,
        Caller::Local,
        json!({"op":"dump","text":"merge","refs":[
            {"kind":"ask","id":ask,"label":"Merge now or wait?","context":{"chat":"asker","options":"Merge / Wait"}},
            {"kind":"hologram","id":"h"}
        ]}),
    )
    .await
    .unwrap();
    let brain = brain_of(&core);
    let row = core.workspace.read_orchestration_with(|d| d.dump(&dump)).unwrap();
    assert_eq!(row.refs.len(), 1, "an unknown kind is dropped: {:?}", row.refs);
    let to_brain = sent(&core, &brain);
    assert!(to_brain.last().unwrap().contains(&format!("@ask id={ask}")), "{to_brain:?}");

    // Only a reference the dump has.
    // (The Brain dump's rewording never answers as the person: the message
    // says "Wait", the person wrote "merge", and "merge" is the answer.)
    let route = |reference: &str| json!({"op":"route","dumpId":dump,"excerpt":"merge","target":{"kind":"note"},
        "intent":"info","message":"Wait","reference":reference});
    let wrong = op(&core, agent(&brain), route("not-a-ref")).await.unwrap_err();
    assert!(wrong.contains("not one of"), "{wrong}");
    // An excerpt that isn't the person's words, verbatim, answers nothing.
    let made_up = json!({"op":"route","dumpId":dump,"excerpt":"wait for CI","target":{"kind":"note"},
        "intent":"info","message":"wait","reference":ask});
    let refused = op(&core, agent(&brain), made_up).await.unwrap_err();
    assert!(refused.contains("verbatim"), "{refused}");
    let still = core.workspace.read_orchestration_with(|d| d.ask(&ask)).unwrap();
    assert_eq!(still.state, AskState::Open, "a refused reply leaves the ask open");

    let id = op(&core, agent(&brain), route(&ask)).await.unwrap();
    let answered = core.workspace.read_orchestration_with(|d| d.ask(&ask)).unwrap();
    assert_eq!(answered.state, AskState::Answered);
    assert_eq!(answered.answer.as_deref(), Some("Option 1: Merge"), "the reply picks the option it names, as a chip does");
    assert_eq!(answered.answered_via, Some(AnswerVia::Person));
    let to_asker = sent(&core, "asker");
    assert!(
        to_asker.last().unwrap().starts_with("[Answer from you] Merge now or wait? \u{2192} Option 1: Merge"),
        "{to_asker:?}"
    );
    let r = route_row(&core, &dump, &id);
    assert_eq!(r.target, RouteTarget::Chat { id: "asker".into() });
    assert_eq!((r.state, r.intent), (RouteState::Delivered, RouteIntent::Info));
    assert_eq!(r.reference.as_deref(), Some(ask.as_str()));
    assert!(r.delivered_at.is_some());
    core.sessions.shutdown().await;
}

#[tokio::test]
async fn a_comment_on_a_card_reference_reaches_the_cards_thread() {
    let (_temp, core) = core().await;
    chat(&core, "worker", "Worker", None).await;
    chat(&core, "fixer", "Fixer", None).await;
    let upsert = |title: &str, thread: &str, state: &str| {
        json!({"op":"upsertItem","item":{"title":title,"threads":[thread],"state":state}})
    };
    let card = op(&core, Caller::Local, upsert("Topology lines", "worker", "active")).await.unwrap();
    let review = op(&core, Caller::Local, upsert("Curved edges", "worker", "review")).await.unwrap();
    let rework = op(&core, Caller::Local, upsert("Dashed rails", "fixer", "review")).await.unwrap();
    let loose = op(&core, Caller::Local, json!({"op":"upsertItem","item":{"title":"Someday","detail":"Old idea"}}))
        .await
        .unwrap();
    let dump = op(
        &core,
        Caller::Local,
        json!({"op":"dump","text":"keep the labels left. looks good, ship it. use dashed lines there. thicker rails please",
            "refs":[{"kind":"card","id":card,"label":"Topology lines"},{"kind":"card","id":loose,"label":"Someday"},
                    {"kind":"card","id":review,"label":"Curved edges"},{"kind":"card","id":rework,"label":"Dashed rails"}]}),
    )
    .await
    .unwrap();
    let brain = brain_of(&core);
    let route = |excerpt: &str, message: &str, reference: &str| {
        json!({"op":"route","dumpId":dump,"excerpt":excerpt,"target":{"kind":"note"},"intent":"info","message":message,"reference":reference})
    };

    // A card at work: the comment goes to its thread, the card untouched.
    let before = sent(&core, "worker").len();
    let id = op(&core, agent(&brain), route("keep the labels left", "Keep the labels on the left.", &card)).await.unwrap();
    let to_worker = sent(&core, "worker");
    assert_eq!(to_worker.len(), before + 1);
    assert!(
        to_worker.last().unwrap().contains("[On your card \"Topology lines\"] Keep the labels on the left."),
        "{to_worker:?}"
    );
    let r = route_row(&core, &dump, &id);
    assert_eq!((r.target.clone(), r.state), (RouteTarget::Chat { id: "worker".into() }, RouteState::Delivered));
    assert_eq!(r.item_id, None, "the card stays its own route's");
    assert_eq!(core.orchestration.item(&card).unwrap().state, WorkState::Active);

    // A Review card the person approves: approveItem, as its button does.
    op(&core, agent(&brain), route("looks good, ship it", "Looks good, ship it.", &review)).await.unwrap();
    let approved = core.orchestration.item(&review).unwrap();
    assert_eq!(approved.state, WorkState::Done);
    assert!(approved.approved_at.is_some());

    // Anything else on a Review card asks for changes, in the person's
    // words: the card back in Now, its thread told.
    let before = sent(&core, "fixer").len();
    op(&core, agent(&brain), route("thicker rails please", "Make the rails thicker and also redo the colours", &rework))
        .await
        .unwrap();
    assert_eq!(core.orchestration.item(&rework).unwrap().state, WorkState::Active);
    let to_fixer = sent(&core, "fixer");
    assert_eq!(to_fixer.len(), before + 1);
    assert!(to_fixer.last().unwrap().contains("thicker rails please"), "{to_fixer:?}");
    assert!(!to_fixer.last().unwrap().contains("colours"), "never the Brain dump's rewording: {to_fixer:?}");

    // Once per card per dump: a second reply on it is a comment, not a verdict.
    op(&core, agent(&brain), route("use dashed lines there", "Approved, looks good", &rework)).await.unwrap();
    assert_eq!(core.orchestration.item(&rework).unwrap().state, WorkState::Active);

    // A "looks good" cut out of "doesn't look good" approves nothing: the
    // whole sentence is judged, and sent as the change asked for.
    let again = op(&core, Caller::Local, upsert("Legend", "fixer", "review")).await.unwrap();
    let dump2 = op(&core, Caller::Local, json!({"op":"dump","text":"it doesn't look good yet",
        "refs":[{"kind":"card","id":again,"label":"Legend","context":{"state":"review"}}]})).await.unwrap();
    op(&core, agent(&brain), json!({"op":"route","dumpId":dump2,"excerpt":"look good","target":{"kind":"note"},
        "intent":"info","message":"looks good","reference":again})).await.unwrap();
    assert_eq!(core.orchestration.item(&again).unwrap().state, WorkState::Active, "changes asked, not approved");
    assert!(sent(&core, "fixer").last().unwrap().contains("it doesn't look good yet"));
    // (A card not in Review when the person dumped: the window's "review" is ignored.)
    let late = op(&core, Caller::Local, upsert("Late", "fixer", "active")).await.unwrap();
    let dump3 = op(&core, Caller::Local, json!({"op":"dump","text":"approve",
        "refs":[{"kind":"card","id":late,"label":"Late","context":{"state":"review"}}]})).await.unwrap();
    op(&core, Caller::Local, json!({"op":"upsertItem","item":{"id":late,"state":"review"}})).await.unwrap();
    op(&core, agent(&brain), json!({"op":"route","dumpId":dump3,"excerpt":"approve","target":{"kind":"note"},
        "intent":"info","message":"approve","reference":late})).await.unwrap();
    assert_eq!(core.orchestration.item(&late).unwrap().state, WorkState::Review, "not a verdict");

    // A card with no thread or owner keeps the comment.
    let id = op(
        &core,
        agent(&brain),
        json!({"op":"route","dumpId":dump,"excerpt":"use dashed lines there","target":{"kind":"note"},
            "intent":"info","message":"Use dashed lines","reference":loose}),
    )
    .await
    .unwrap();
    assert_eq!(core.orchestration.item(&loose).unwrap().detail, "Old idea\n\nUse dashed lines");
    assert_eq!(route_row(&core, &dump, &id).target, RouteTarget::Item { id: loose.clone() });
    core.sessions.shutdown().await;
}

#[tokio::test]
async fn a_device_reference_is_context_and_the_place_a_new_card_goes() {
    let (_temp, core) = core().await;
    core.workspace.upsert_device_row(&zeron_proto::Device {
        id: "dev-x".into(),
        name: "x16-win".into(),
        platform: "windows".into(),
        last_seen_at: Some(chrono::Utc::now()),
        created_at: None,
        version: None,
        cursor_sdk_version: None,
        capabilities: Vec::new(),
    });
    let dump = op(
        &core,
        Caller::Local,
        json!({"op":"dump","text":"later: fix the installer there",
            "refs":[{"kind":"device","id":"dev-x","label":"x16-win","context":{"os":"windows"}},
                    {"kind":"device","id":"dev-gone","label":"old box"}]}),
    )
    .await
    .unwrap();
    let brain = brain_of(&core);
    let row = core.workspace.read_orchestration_with(|d| d.dump(&dump)).unwrap();
    assert_eq!(row.refs[0].context("os"), Some("windows"));
    let route = |reference: &str| json!({"op":"route","dumpId":dump,"excerpt":"later: fix the installer there",
        "target":{"kind":"item"},"intent":"task","message":"Fix the installer","title":"Fix the installer","reference":reference});
    let id = op(&core, agent(&brain), route("dev-x")).await.unwrap();
    let r = route_row(&core, &dump, &id);
    assert_eq!(r.reference.as_deref(), Some("dev-x"));
    assert_eq!(r.device_id.as_deref(), Some("dev-x"), "the device hint places it");
    let card = core.orchestration.item(r.item_id.as_deref().unwrap()).unwrap();
    assert_eq!(card.device_id.as_deref(), Some("dev-x"));
    // A device Zeron doesn't know is no hint (and no error).
    let id = op(&core, agent(&brain), route("dev-gone")).await.unwrap();
    assert_ne!(route_row(&core, &dump, &id).device_id.as_deref(), Some("dev-gone"));
    core.sessions.shutdown().await;
}

/// The cheap model, for "does this message answer that question?" only:
/// `says` (None: it can't be reached). How often it was asked.
fn answers_model(core: &crate::EngineCore, says: Option<&str>) -> std::sync::Arc<std::sync::atomic::AtomicUsize> {
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (n, says) = (calls.clone(), says.map(str::to_owned));
    core.orchestration.progress().set_note_fn(std::sync::Arc::new(move |_, prompt: String| {
        let said = if prompt.starts_with(zeron_proto::turn_question::ANSWERS_INSTRUCTIONS) {
            n.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            says.clone()
        } else {
            None
        };
        Box::pin(async move { said })
    }));
    calls
}

fn relayed(body: &str) -> String {
    zeron_proto::agent_messages::agent_message("Brain dump (brain-du)", "brain-dump-1", body)
}

/// A card the old rule left in Review on `chat`, whose thread's turn (after
/// the card's delivery) ended asking `asked`. Its card id.
async fn stale_decision_card(core: &crate::EngineCore, chat: &str, title: &str, asked: &str) -> String {
    let (dump, _, r) = routed_thread(core, chat, &[title]).await;
    push(core, chat, &format!("{chat}-u1"), zeron_doc::MessageRole::User, title);
    push(core, chat, &format!("{chat}-a1"), zeron_doc::MessageRole::Assistant, asked);
    outcome(core, chat, &dump, &r[0].0, asked, RouteState::Done).await;
    // (No ask from its outcome: the start's backfill is what's tested.)
    for a in asks_from(core, chat) {
        op(core, Caller::Local, json!({"op":"dismissAsk","id":a.id})).await.unwrap();
    }
    core.orchestration.workspace().write_orchestration(|doc| {
        let mut d = doc.dump(&dump).unwrap();
        d.routes[0].state = RouteState::Done;
        d.routes[0].review = None;
        doc.upsert_dump(&d);
    });
    core.orchestration
        .patch_item(WorkItemPatch { id: Some(r[0].1.clone()), state: Some(WorkState::Review), ..Default::default() })
        .unwrap();
    r[0].1.clone()
}

fn open_asks(core: &crate::EngineCore, chat: &str) -> Vec<Ask> {
    asks_from(core, chat).into_iter().filter(|a| a.state == AskState::Open).collect()
}

const PICK: &str = "Both variants are built.\n\nShould I merge it into `zeron/orchestrators`, or open a PR against `zeron/zgx`?";

#[tokio::test]
async fn a_direct_reply_in_the_thread_answers_without_the_model() {
    let (_temp, core) = core().await;
    let calls = answers_model(&core, Some("no"));
    let card_id = stale_decision_card(&core, "t-direct", "Ship the fix", PICK).await;
    push(&core, "t-direct", "u2", zeron_doc::MessageRole::User, "hold off for now, I'll look tomorrow");
    core.rpc_service().settle_card_states().await;
    assert!(open_asks(&core, "t-direct").is_empty(), "the person replied in the thread");
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert_eq!(card(&core, &card_id), WorkState::Active);
    core.sessions.shutdown().await;
}

#[tokio::test]
async fn a_relayed_message_naming_an_option_answers_without_the_model() {
    let (_temp, core) = core().await;
    let calls = answers_model(&core, Some("no"));
    stale_decision_card(&core, "t-option", "Ship the fix", PICK).await;
    push(&core, "t-option", "u2", zeron_doc::MessageRole::User, &relayed("The person says: open a PR against zeron/zgx, please."));
    core.rpc_service().settle_card_states().await;
    assert!(open_asks(&core, "t-option").is_empty());
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0, "named an option: no model");
    core.sessions.shutdown().await;
}

#[tokio::test]
async fn a_relayed_message_about_other_work_leaves_the_question_open() {
    let (_temp, core) = core().await;
    let calls = answers_model(&core, Some("no"));
    let card_id = stale_decision_card(&core, "t-other", "Ship the fix", PICK).await;
    push(&core, "t-other", "u2", zeron_doc::MessageRole::User, &relayed("New graph-map layout direction: size each machine's island by what it holds."));
    core.rpc_service().settle_card_states().await;
    let open = open_asks(&core, "t-other");
    assert_eq!(open.len(), 1, "{open:#?}");
    assert_eq!(open[0].item_id.as_deref(), Some(card_id.as_str()));
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    // Judged once: the start's pass again asks the model nothing new.
    core.rpc_service().settle_card_states().await;
    assert!(calls.load(std::sync::atomic::Ordering::SeqCst) <= 1);
    core.sessions.shutdown().await;
}

#[tokio::test]
async fn without_the_model_a_relayed_message_leaves_the_question_open() {
    let (_temp, core) = core().await;
    let calls = answers_model(&core, None);
    stale_decision_card(&core, "t-nomodel", "Ship the fix", PICK).await;
    push(&core, "t-nomodel", "u2", zeron_doc::MessageRole::User, &relayed("They'd like this wrapped up soon."));
    core.rpc_service().settle_card_states().await;
    assert_eq!(open_asks(&core, "t-nomodel").len(), 1, "unreachable: raised");
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    // A turn-end ask's close by a reply follows the same rule.
    let fixes = codex_thread(&core).await;
    let _ = fixes;
    turn_ended(&core, CODEX_THREAD).await;
    push(&core, CODEX_THREAD, "r1", zeron_doc::MessageRole::User, &relayed("Unrelated: the Map needs a legend pass."));
    super::turn_asks::on_reply(&core.orchestration, &core.rpc_service(), CODEX_THREAD).await;
    assert_eq!(open_asks(&core, CODEX_THREAD).len(), 1, "not answered: kept");
    push(&core, CODEX_THREAD, "r2", zeron_doc::MessageRole::User, &relayed("Decision: merge into zeron/orchestrators."));
    super::turn_asks::on_reply(&core.orchestration, &core.rpc_service(), CODEX_THREAD).await;
    let codex = asks_from(&core, CODEX_THREAD);
    assert_eq!((codex.len(), codex[0].state, codex[0].answered_via), (1, AskState::Answered, Some(AnswerVia::Thread)));
    assert_eq!(codex[0].answer.as_deref(), Some("Decision: merge into zeron/orchestrators."));
    core.sessions.shutdown().await;
}

/// The live "Topology lines: pick a variant" card: its thread asked, and the
/// person picked later (a relayed message at 19:40). The start's backfill
/// raises no ask for it, and closes the one that was open; a card whose
/// thread asked and heard nothing since gets its ask.
#[tokio::test]
async fn the_start_raises_no_ask_the_person_already_answered() {
    let (_temp, core) = core().await;
    let rpc = core.rpc_service();
    let asked = "The four Topology variations are built and scored.\n\n| Version | Score |\n|---|---|\n| (a) inline tag | 8 |\n| (d) coloured edge | 8.5 |\n\nI recommend (a) with all three. Tell me which version you want and I'll polish it and merge it.\n\nThe graph label decision (accept round 9, or keep iterating) is also still yours to make.";
    let mut cards = Vec::new();
    for (chat, title) in [("9ad131f1-map", "Topology lines: pick a variant"), ("other-map", "Map machine headings: decide R9")] {
        let (dump, _, r) = routed_thread(&core, chat, &[title]).await;
        push(&core, chat, "u1", zeron_doc::MessageRole::User, title);
        push(&core, chat, "a1", zeron_doc::MessageRole::Assistant, asked);
        // As the old rule left it: in Review, its route done with no review.
        outcome(&core, chat, &dump, &r[0].0, asked, RouteState::Done).await;
        core.orchestration.workspace().write_orchestration(|doc| {
            let mut d = doc.dump(&dump).unwrap();
            d.routes[0].state = RouteState::Done;
            d.routes[0].review = None;
            doc.upsert_dump(&d);
        });
        // The first's ask (from its outcome) stays open; the second's is
        // dismissed, as if it was never raised.
        if chat == "other-map" {
            for a in asks_from(&core, chat) {
                op(&core, Caller::Local, json!({"op":"dismissAsk","id":a.id})).await.unwrap();
            }
        }
        core.orchestration
            .patch_item(WorkItemPatch { id: Some(r[0].1.clone()), state: Some(WorkState::Review), ..Default::default() })
            .unwrap();
        cards.push((chat, r[0].1.clone()));
    }
    let open_first = asks_from(&core, "9ad131f1-map").into_iter().filter(|a| a.state == AskState::Open).count();
    assert_eq!(open_first, 1);
    // The person's pick reaches the first thread, relayed by the Brain dump:
    // it names no option, so the cheap model judges it (and says it does).
    let calls = answers_model(&core, Some("yes"));
    push(&core, "9ad131f1-map", "u2", zeron_doc::MessageRole::User, &relayed("The person picked for the Topology variants. They want TWO options, switchable by the user."));
    rpc.settle_card_states().await;
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    // Answered: no open ask, the old one closed by the thread's reply.
    let first = asks_from(&core, "9ad131f1-map");
    assert!(first.iter().all(|a| a.state != AskState::Open), "{first:#?}");
    assert!(first.iter().any(|a| a.answered_via == Some(AnswerVia::Thread) && a.answer.as_deref().is_some_and(|t| t.contains("The person picked"))));
    // Unanswered: its ask, on its card.
    let second: Vec<_> = asks_from(&core, "other-map").into_iter().filter(|a| a.state == AskState::Open).collect();
    assert_eq!(second.len(), 1, "{second:#?}");
    assert_eq!(second[0].item_id.as_deref(), Some(cards[1].1.as_str()));
    // Both wait in Now, not Review.
    assert_eq!([card(&core, &cards[0].1), card(&core, &cards[1].1)], [WorkState::Active, WorkState::Active]);
    core.sessions.shutdown().await;
}

/// The floating composer's @ask (and Needs you's quick reply) answers through
/// answerAsk in the person's words: its delivery to the thread is tied to
/// the ask, never judged by the model, even when none can be reached.
#[tokio::test]
async fn an_answered_asks_delivery_answers_without_the_model() {
    let (_temp, core) = core().await;
    let calls = answers_model(&core, None);
    stale_decision_card(&core, "t-at-ask", "Ship the fix", PICK).await;
    let delivered = zeron_proto::orchestration::prompt_text::answer_delivery(
        "Should I merge it into zeron/orchestrators, or open a PR against zeron/zgx?",
        "neither yet, rerun the flaky test first",
    );
    assert!(zeron_proto::agent_messages::agent_message_sender(&delivered).is_none(), "not a relayed message");
    push(&core, "t-at-ask", "u2", zeron_doc::MessageRole::User, &delivered);
    core.rpc_service().settle_card_states().await;
    assert!(open_asks(&core, "t-at-ask").is_empty());
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    core.sessions.shutdown().await;
}

// ── Decisions: pictures, a pick, putting off ────────────────────────────────

const PNG: &[u8] = b"\x89PNG\r\n\x1a\n-a-picture";

/// A chat of the person's whose folder is `cwd`.
async fn chat_in(core: &crate::EngineCore, id: &str, cwd: &std::path::Path) {
    call(
        core,
        Caller::Local,
        methods::MUTATE,
        json!({"op":"createChat", "chatId":id, "deviceId":REMOTE, "cwd": cwd.to_str().unwrap(),
               "config":{"harness":"codex","sandbox":"workspace-write"}}),
    )
    .await
    .unwrap();
}

fn ask_of(core: &crate::EngineCore, id: &str) -> Ask {
    core.workspace.read_orchestration_with(|doc| doc.ask(id)).unwrap()
}

#[tokio::test]
async fn a_decision_keeps_its_pictures_and_pick_and_an_option_answers_it() {
    let (_temp, core) = core().await;
    let work = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(work.path().join("shots")).unwrap();
    std::fs::write(work.path().join("shots/tag.png"), PNG).unwrap();
    std::fs::write(work.path().join("shots/now.png"), PNG).unwrap();
    std::fs::write(elsewhere.path().join("edge.png"), PNG).unwrap();
    chat_in(&core, "asker", work.path()).await;
    let id = op(
        &core,
        agent("asker"),
        json!({"op":"ask","kind":"decision","question":"Which variant?",
               "options":[
                   {"label":"(a) Tag","detail":"No lines","images":["shots/tag.png"],"scores":{"Claude":8,"GPT":8.5}},
                   {"label":"(b) Edge","images":[elsewhere.path().join("edge.png").to_str().unwrap(), "https://x.dev/edge.png", "http://x.dev/plain.png", "https://127.0.0.1/edge.png"]}
               ],
               "images":["shots/now.png", "notes.txt"],
               "recommendation":{"option":0,"why":"Two of three picked it."},
               "links":[{"kind":"branch","label":"orch/topo-variants","target":"orch/topo-variants"}],
               "context":"Built on `c1e090c7e`."}),
    )
    .await
    .unwrap();
    let ask = ask_of(&core, &id);
    // Its own folder's pictures are kept in the uploads store; the one
    // outside it and the non-picture are left out; a URL stays.
    let store = std::fs::canonicalize(core.uploads.dir()).unwrap();
    let tag = &ask.options[0].images;
    assert_eq!(tag.len(), 1, "{tag:?}");
    assert!(std::path::Path::new(&tag[0]).starts_with(&store) && tag[0].ends_with("-tag.png"), "{tag:?}");
    assert_eq!(std::fs::read(&tag[0]).unwrap(), PNG);
    // (Only https to a public name: plain http and loopback are left out.)
    assert_eq!(ask.options[1].images, ["https://x.dev/edge.png"]);
    assert_eq!(ask.images.len(), 1);
    assert!(ask.images[0].ends_with("-now.png"));
    assert_eq!(ask.options[0].scores.0[1], ("GPT".to_owned(), Score::Number(8.5)));
    assert_eq!(ask.recommendation.as_ref().and_then(|r| r.option), Some(0));
    assert_eq!(ask.links[0].target, "orch/topo-variants");
    assert_eq!(ask.context.as_deref(), Some("Built on `c1e090c7e`."));
    // A pick out of range, or nothing at all, is refused.
    assert!(op(&core, agent("asker"), json!({"op":"ask","kind":"decision","question":"Q?","options":[{"label":"a"}],"recommendation":{"option":2}})).await.is_err());
    assert!(op(&core, Caller::Local, json!({"op":"answerAsk","id":id,"answer":""})).await.is_err());
    assert!(op(&core, Caller::Local, json!({"op":"answerAsk","id":id,"option":5})).await.is_err());
    assert_eq!(ask_of(&core, &id).state, AskState::Open);
    // The person picks (a) and adds a word.
    op(&core, Caller::Local, json!({"op":"answerAsk","id":id,"option":0,"answer":"and polish it"})).await.unwrap();
    let ask = ask_of(&core, &id);
    assert_eq!((ask.state, ask.answered_via), (AskState::Answered, Some(AnswerVia::Person)));
    assert_eq!(ask.answer.as_deref(), Some("Option 1: (a) Tag \u{2014} and polish it"));
    assert_eq!(sent(&core, "asker"), vec!["[Answer from you] Which variant? \u{2192} Option 1: (a) Tag \u{2014} and polish it"]);
    // An option alone does too.
    let id = op(&core, agent("asker"), json!({"op":"ask","kind":"decision","question":"Ship?","options":[{"label":"Now"},{"label":"Later"}]})).await.unwrap();
    op(&core, Caller::Local, json!({"op":"answerAsk","id":id,"option":1})).await.unwrap();
    assert_eq!(ask_of(&core, &id).answer.as_deref(), Some("Option 2: Later"));
    assert!(sent(&core, "asker").last().unwrap().ends_with("\u{2192} Option 2: Later"));
    core.sessions.shutdown().await;
}

#[tokio::test]
async fn a_deferred_ask_is_hidden_until_its_time_then_back_once() {
    let (_temp, core) = core().await;
    chat(&core, "asker", "Asker", None).await;
    let id = op(&core, agent("asker"), json!({"op":"ask","kind":"question","question":"Which bucket?"})).await.unwrap();
    let now = chrono::Utc::now();
    let until = now + chrono::Duration::hours(1);
    // Only the person puts it off.
    assert!(op(&core, agent("asker"), json!({"op":"deferAsk","id":id,"until":until})).await.is_err());
    op(&core, Caller::Local, json!({"op":"deferAsk","id":id,"until":until})).await.unwrap();
    let ask = ask_of(&core, &id);
    assert!(ask.deferred(now) && ask.state == AskState::Open);
    let rpc = core.rpc_service();
    let here = |_: &str| true;
    // Not back yet, and not raised as stale meanwhile.
    assert!(super::turn_asks::bring_back_with(&core.orchestration, &rpc, now, &here).is_empty());
    let stale = now + super::turn_asks::STALE_AFTER + chrono::Duration::minutes(1);
    assert!(super::turn_asks::escalate_with(&core.orchestration, &rpc, stale, &here).is_empty(), "put off, not stale");
    // Its time: back, once, its wait started over.
    let later = until + chrono::Duration::minutes(1);
    assert_eq!(super::turn_asks::bring_back_with(&core.orchestration, &rpc, later, &|_| false), Vec::<String>::new(), "its host brings it back");
    assert_eq!(super::turn_asks::bring_back_with(&core.orchestration, &rpc, later, &here), vec![id.clone()]);
    let ask = ask_of(&core, &id);
    assert!(ask.deferred_until.is_none() && !ask.deferred(later) && ask.updated_at == Some(later));
    assert!(super::turn_asks::bring_back_with(&core.orchestration, &rpc, later + chrono::Duration::hours(1), &here).is_empty(), "once");
    assert!(super::turn_asks::escalate_with(&core.orchestration, &rpc, later + chrono::Duration::minutes(5), &here).is_empty(), "fresh again");
    // Put off and brought back by hand: `until` absent.
    op(&core, Caller::Local, json!({"op":"deferAsk","id":id,"until":until + chrono::Duration::days(1)})).await.unwrap();
    assert!(ask_of(&core, &id).deferred_until.is_some());
    op(&core, Caller::Local, json!({"op":"deferAsk","id":id})).await.unwrap();
    assert!(ask_of(&core, &id).deferred_until.is_none());
    // Answered: no more putting off.
    op(&core, Caller::Local, json!({"op":"answerAsk","id":id,"answer":"us-east"})).await.unwrap();
    assert!(op(&core, Caller::Local, json!({"op":"deferAsk","id":id,"until":until})).await.is_err());
    core.sessions.shutdown().await;
}

#[tokio::test]
async fn a_tool_ask_gets_the_message_that_asked_when_its_turn_ends() {
    let (_temp, core) = core().await;
    chat(&core, "asker", "Asker", None).await;
    push(&core, "asker", "u1", zeron_doc::MessageRole::User, "Pick a bucket and go.");
    push(&core, "asker", "a1", zeron_doc::MessageRole::Assistant, "Two buckets fit; asking.");
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let id = op(&core, agent("asker"), json!({"op":"ask","kind":"question","question":"Which bucket?"})).await.unwrap();
    assert!(ask_of(&core, &id).turn_id.is_none());
    // (Written after it: a later millisecond.)
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    push(&core, "asker", "a2", zeron_doc::MessageRole::Assistant, "Waiting on the bucket.");
    turn_ended(&core, "asker").await;
    assert_eq!(ask_of(&core, &id).turn_id.as_deref(), Some("a1"));
    core.sessions.shutdown().await;
}

/// Thread 9ad131f1's Topology turn (its final message, shortened).
const TOPOLOGY_FINAL: &str = "The four Topology variations are built and scored, and they're waiting for your pick. Nothing is merged. They sit behind a fixture switch on `orch/topo-variants` (commit `c1e090c7e`).\n\n| Version | Claude | GPT | Grok |\n|---|---:|---:|---:|\n| Current: a gutter column per rail | 5 | 6 | 5 |\n| (a) inline tag \"↳ Brain dump\", no cross-machine lines | **8** | **8.5** | 8 |\n| (b) line in its dot's column, stepping aside | 5 | 5.5 | 4 |\n\nI recommend (a) with all three. Tell me which version you want and I'll polish it and merge it.\n\nScreenshots are in `shots/`:\n- topo-{columns,tag,dotcol}-dark-v2.png\n\nThe graph label decision (accept round 9, or keep iterating) is also still yours to make.";

#[tokio::test]
async fn the_topology_turn_becomes_a_decision_with_its_variants_pictures_and_pick() {
    let (_temp, core) = core().await;
    let work = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(work.path().join("shots")).unwrap();
    for name in ["columns", "tag", "dotcol"] {
        std::fs::write(work.path().join(format!("shots/topo-{name}-dark-v2.png")), PNG).unwrap();
    }
    let fixes = orchestrator(&core, "Zeron fixes").await;
    let map = "9ad131f1-cd18-43de-9ef8-47ba98880783";
    chat_in(&core, map, work.path()).await;
    op(&core, Caller::Local, json!({"op":"addToCrew","orchestratorId":fixes,"chatId":map})).await.unwrap();
    push(&core, map, "u1", zeron_doc::MessageRole::User, "New Topology variants");
    push(&core, map, "a1", zeron_doc::MessageRole::Assistant, TOPOLOGY_FINAL);
    turn_ended(&core, map).await;
    let asks = asks_from(&core, map);
    assert_eq!(asks.len(), 1, "{asks:#?}");
    let ask = &asks[0];
    assert_eq!((ask.kind, ask.turn_id.as_deref()), (AskKind::Decision, Some("a1")));
    assert!(ask.question.starts_with("Tell me which version you want"), "{}", ask.question);
    let labels: Vec<&str> = ask.options.iter().map(|o| o.label.as_str()).collect();
    assert_eq!(labels, ["(a) Inline tag \"↳ Brain dump\"", "(b) Line in its dot's column"]);
    assert_eq!(ask.options[0].scores.0.len(), 3);
    let r = ask.recommendation.as_ref().unwrap();
    assert_eq!((r.option, r.why.as_deref()), (Some(0), Some("With all three")));
    // (a)'s screenshot by its name, stored; the rest the ask's own.
    assert_eq!(ask.options[0].images.len(), 1, "{:?}", ask.options);
    assert!(ask.options[0].images[0].ends_with("-topo-tag-dark-v2.png"), "{:?}", ask.options[0].images);
    assert!(std::path::Path::new(&ask.options[0].images[0]).starts_with(std::fs::canonicalize(core.uploads.dir()).unwrap()));
    assert_eq!(ask.images.len(), 2, "{:?}", ask.images);
    assert_eq!(ask.links[0].target, "orch/topo-variants");
    assert!(ask.context.as_deref().unwrap().contains("Also waiting on you: The graph label decision"));
    // The person picks (a): the thread hears which.
    op(&core, Caller::Local, json!({"op":"answerAsk","id":ask.id,"option":0})).await.unwrap();
    assert!(sent(&core, map).iter().any(|m| m.ends_with("Option 1: (a) Inline tag \"↳ Brain dump\"")), "{:?}", sent(&core, map));
    core.sessions.shutdown().await;
}

/// Live, after the restart onto 4f34e371e: "Map machine headings: decide
/// R9" sat in Now, moved there by the Brain dump (marked as moved by hand),
/// still waiting on a decision, and the start raised no ask for it. The
/// mark keeps a card's column, never its question.
#[tokio::test]
async fn a_card_moved_by_hand_into_now_still_gets_its_decision_ask() {
    let (_temp, core) = core().await;
    let calls = answers_model(&core, None);
    let r9 = "Round 9 is merged locally (e357a3876); final scores are Claude 8, GPT 8.5, Grok 9.\n\nThe graph-label decision (accept round 9 or keep iterating) is still waiting on you.";
    let mut cards = Vec::new();
    for chat in ["r9-open", "r9-answered"] {
        let (dump, brain, r) = routed_thread(&core, chat, &["Map machine headings: decide R9"]).await;
        push(&core, chat, &format!("{chat}-u1"), zeron_doc::MessageRole::User, "Map machine headings: decide R9");
        push(&core, chat, &format!("{chat}-a1"), zeron_doc::MessageRole::Assistant, r9);
        outcome(&core, chat, &dump, &r[0].0, r9, RouteState::Done).await;
        // As live: no ask for it, and the Brain dump moved the card to Now.
        for a in asks_from(&core, chat) {
            op(&core, Caller::Local, json!({"op":"dismissAsk","id":a.id})).await.unwrap();
        }
        op(&core, agent(&brain), json!({"op":"upsertItem","item":{"id":r[0].1,"state":"active"}})).await.unwrap();
        let moved = core.orchestration.item(&r[0].1).unwrap();
        assert!(moved.moved_at.is_some() && moved.state == WorkState::Active);
        cards.push((chat, r[0].1.clone()));
    }
    // The second: the person answered in its thread since.
    push(&core, "r9-answered", "r9-answered-u2", zeron_doc::MessageRole::User, "keep iterating on the labels");
    core.rpc_service().settle_card_states().await;
    // Unanswered: one ask, on its card.
    let open = open_asks(&core, "r9-open");
    assert_eq!(open.len(), 1, "{open:#?}");
    assert_eq!(open[0].item_id.as_deref(), Some(cards[0].1.as_str()));
    assert!(open[0].question.contains("graph-label decision"));
    // Answered: none.
    assert!(open_asks(&core, "r9-answered").is_empty());
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0, "a reply in the thread needs no model");
    // Their columns, and their marks, as they were.
    for (_, item) in &cards {
        let c = core.orchestration.item(item).unwrap();
        assert!(c.state == WorkState::Active && c.moved_at.is_some());
    }
    // Once: the start's pass again raises nothing more.
    core.rpc_service().settle_card_states().await;
    assert_eq!(open_asks(&core, "r9-open").len(), 1);
    assert_eq!(asks_from(&core, "r9-open").iter().filter(|a| a.state == AskState::Open).count(), 1);
    core.sessions.shutdown().await;
}

/// Live, on engine 9e4a5a751: R9's card sat in Now with no ask, because its
/// thread moved on to other work after asking: its latest reply and the
/// card's own outcome said nothing of the decision. The start looks back
/// through the thread since the card's delivery for the decision.
#[tokio::test]
async fn a_decision_followed_by_other_work_still_gets_its_ask() {
    let (_temp, core) = core().await;
    let calls = answers_model(&core, Some("no"));
    let chat = "r9-moved-on";
    let (dump, _, r) = routed_thread(&core, chat, &["Map machine headings: decide R9"]).await;
    push(&core, chat, "u1", zeron_doc::MessageRole::User, "Map machine headings: decide R9");
    push(&core, chat, "a1", zeron_doc::MessageRole::Assistant, "Round 9 is merged locally.\n\nThe graph-label decision (accept round 9 or keep iterating) is still waiting on you.");
    // Other work since: a relayed task about something else, progress, done.
    push(&core, chat, "u2", zeron_doc::MessageRole::User, &relayed("New graph-map layout direction: size each machine's island by what it holds."));
    push(&core, chat, "a2", zeron_doc::MessageRole::Assistant, "Working on the packed layout; the sim's data shapes first.");
    push(&core, chat, "u3", zeron_doc::MessageRole::User, &relayed("Also fix the Timeline legend."));
    push(&core, chat, "a3", zeron_doc::MessageRole::Assistant, "Timeline legend fixed to Report, Ask, Failed, Done; committed.");
    // Its card in Now, its route's and its own outcome about the later work.
    outcome(&core, chat, &dump, &r[0].0, "Timeline legend fixed to Report, Ask, Failed, Done; committed.", RouteState::Answered).await;
    assert_eq!(card(&core, &r[0].1), WorkState::Active);
    assert!(open_asks(&core, chat).is_empty());
    core.rpc_service().settle_card_states().await;
    let open = open_asks(&core, chat);
    assert_eq!(open.len(), 1, "{open:#?}");
    assert_eq!(open[0].item_id.as_deref(), Some(r[0].1.as_str()));
    assert!(open[0].question.contains("graph-label decision"), "{}", open[0].question);
    // Each relayed message since was judged once: neither answers it.
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert_eq!(card(&core, &r[0].1), WorkState::Active, "its column as it was");
    core.sessions.shutdown().await;
}
