//! Real PTY → engine projection → registry → sidebar derivation and MCP.
#![cfg(unix)]

use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use chrono::Utc;
use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_mcp::{Origin, Tools, Zeron};
use zeron_proto::{ChatIndicator, HarnessId, Session, SessionStatus, TerminalEvent};
use zeron_rpc::memory_client;

fn assemble(path: &std::path::Path) -> EngineCore {
    EngineCore::assemble(
        path,
        Arc::new(HarnessRegistry::new()),
        HarnessId::Mock,
        None,
    )
    .unwrap()
}

async fn state(
    rows: &mut tokio::sync::watch::Receiver<Vec<Session>>,
    expected: ChatIndicator,
    core: &EngineCore,
) -> Session {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let snapshot = rows.borrow_and_update().clone();
            let chat = core.workspace.chat("chat").unwrap().unwrap();
            if let Some(row) = snapshot.into_iter().find(|row| row.chat_id == "chat")
                && zeron_proto::view::display_status(&chat, Some(&row), Utc::now()) == expected
            {
                return row;
            }
            rows.changed().await.unwrap();
        }
    })
    .await
    .expect("program status reaches the shared session projection")
}

fn emit(core: &EngineCore, id: &str, body: &str) {
    // Shell input contains printable escapes, so command echo cannot itself
    // impersonate an OSC report before printf actually runs.
    core.terminals
        .write_bytes(id, format!("printf '\\033]7501;{body}\\007'\r").as_bytes())
        .unwrap();
}

#[tokio::test]
async fn pty_states_match_sidebar_mcp_and_synced_registry_without_a_viewport() {
    let temp = tempfile::tempdir().unwrap();
    let core = assemble(temp.path());
    core.workspace
        .create_chat(
            "chat",
            None,
            Some(&core.device_id),
            None,
            Some(temp.path().display().to_string()),
        )
        .unwrap();
    let terminal = core
        .terminals
        .open_with_shell(temp.path().to_str().unwrap(), 80, 24, Some("/bin/sh"))
        .unwrap();
    core.terminals.associate_chat(&terminal.id, "chat").unwrap();
    let mut rows = core.sessions.watch_sessions();
    let zeron = Arc::new(Zeron::with_client(
        memory_client(core.rpc_service()),
        Origin::default(),
    ));
    let tools = Tools::new(zeron.clone());

    emit(&core, &terminal.id, "state=working:app=demo");
    let working = state(&mut rows, ChatIndicator::Working, &core).await;
    assert_eq!(
        working.status,
        SessionStatus::Idle,
        "native control state is untouched"
    );
    assert!(working.last_completed_turn.is_none());
    assert_eq!(
        tools
            .call("get_chat", serde_json::json!({"chat":"chat"}))
            .await
            .unwrap()["status"],
        "working"
    );

    emit(
        &core,
        &terminal.id,
        "state=blocked:kind=question:app=demo:msg=V2hpY2ggYnJhbmNoPw==",
    );
    let blocked = state(&mut rows, ChatIndicator::AwaitingInput, &core).await;
    let listed = tools
        .call("list_chats", serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(listed["chats"][0]["status"], "awaitingInput");
    assert_eq!(
        listed["chats"][0]["terminalActivity"]["report"]["kind"],
        "question"
    );
    assert_eq!(
        listed["chats"][0]["terminalActivity"]["report"]["message"],
        "Which branch?"
    );
    assert_eq!(
        core.workspace.watch_session_rows().borrow()[0].terminal_activity,
        blocked.terminal_activity
    );
    let waited = tools
        .call(
            "wait_for_turn",
            serde_json::json!({"chat":"chat", "timeout_secs":1}),
        )
        .await
        .unwrap();
    assert_eq!(waited["turn"]["outcome"], "awaitingInput");
    assert_eq!(waited["turn"]["status"], "awaitingInput");

    emit(&core, &terminal.id, "state=working:app=demo");
    state(&mut rows, ChatIndicator::Working, &core).await;
    emit(&core, &terminal.id, "state=done:app=demo");
    let done = state(&mut rows, ChatIndicator::Completed, &core).await;
    assert!(
        done.last_completed_turn.is_none(),
        "OSC done cannot forge a native completion receipt"
    );
    assert_eq!(
        tools
            .call("get_chat", serde_json::json!({"chat":"chat"}))
            .await
            .unwrap()["status"],
        "completed"
    );
    core.workspace.mark_chat_seen("chat", Utc::now()).unwrap();
    assert_eq!(
        tools
            .call("get_chat", serde_json::json!({"chat":"chat"}))
            .await
            .unwrap()["status"],
        "idle"
    );

    // A terminal working/done cycle must not complete a native send that has
    // been accepted but has not started. It needs native completion evidence.
    let chat = core.workspace.chat("chat").unwrap().unwrap();
    let (waited, ()) = tokio::join!(
        zeron.wait_for_turn(&chat, Some(&done), true, Duration::from_millis(200)),
        async {
            tokio::time::sleep(Duration::from_millis(25)).await;
            emit(&core, &terminal.id, "state=working:app=demo");
            state(&mut rows, ChatIndicator::Working, &core).await;
            emit(&core, &terminal.id, "state=done:app=demo");
            state(&mut rows, ChatIndicator::Completed, &core).await;
        }
    );
    assert_eq!(serde_json::to_value(waited.unwrap().0).unwrap(), "timedOut");

    emit(
        &core,
        &terminal.id,
        "state=blocked:id=worker:kind=permission:app=demo",
    );
    state(&mut rows, ChatIndicator::AwaitingInput, &core).await;
    emit(&core, &terminal.id, "state=done:app=demo");
    // A finished root does not hide its still-blocked worker.
    assert_eq!(
        tools
            .call("get_chat", serde_json::json!({"chat":"chat"}))
            .await
            .unwrap()["status"],
        "awaitingInput"
    );
    core.terminals.close(&terminal.id).unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if rows
                .borrow_and_update()
                .iter()
                .all(|row| row.terminal_activity.is_none())
            {
                break;
            }
            rows.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    assert_eq!(
        tools
            .call("get_chat", serde_json::json!({"chat":"chat"}))
            .await
            .unwrap()["status"],
        "idle"
    );
    // A lost terminal owner must not turn a blocked report into success.
    let mut stale = blocked;
    // Our own registry rows are intentionally replaced by local live state.
    // A remote owner's row exercises the offline/stale registry path instead.
    stale.device_id = "offline-owner".into();
    stale.updated_at =
        chrono::DateTime::from_timestamp_millis(Utc::now().timestamp_millis() - 46_000).unwrap();
    let mut registry = core.workspace.watch_session_rows();
    core.workspace.record_session(&stale);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if registry.borrow_and_update().iter().any(|row| row == &stale) {
                break;
            }
            registry.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    let summary = tools
        .call("get_chat", serde_json::json!({"chat":"chat"}))
        .await
        .unwrap();
    assert_eq!(summary["status"], "idle");
    assert_eq!(summary["statusStale"], true);
    let waited = tools
        .call(
            "wait_for_turn",
            serde_json::json!({"chat":"chat","timeout_secs":1}),
        )
        .await
        .unwrap();
    assert_eq!(waited["turn"]["outcome"], "timedOut");
    assert_eq!(waited["turn"]["statusStale"], true);
    core.shutdown().await;
}

#[tokio::test]
async fn feature_detection_is_answered_by_the_pty_owner_without_a_terminal_emulator() {
    let temp = tempfile::tempdir().unwrap();
    let core = assemble(temp.path());
    let terminal = core
        .terminals
        .open_with_shell(temp.path().to_str().unwrap(), 80, 24, Some("/bin/sh"))
        .unwrap();
    let reply =
        b"\x1b]7501;?:states=idle,working,done,blocked,error:kinds=permission,question,auth\x1b\\";
    core.terminals.write_bytes(&terminal.id, format!(
        "stty -echo -icanon min 1 time 0; printf '\\033]7501;?\\007'; reply=$(dd bs=1 count={} 2>/dev/null); case \"$reply\" in *states=idle,working,done,blocked,error*) printf '\\nFEATURE_OK\\n';; esac; exit\r", reply.len()
    ).as_bytes()).unwrap();
    // This subscribes only to raw output; no terminal emulator is present to
    // implement detection on the engine's behalf.
    let mut output = core.terminals.subscribe(&terminal.id, None).unwrap();
    let captured = tokio::time::timeout(Duration::from_secs(10), async {
        let mut bytes = Vec::new();
        while let Some(event) = output.recv().await {
            if let TerminalEvent::Data { data, .. } = event {
                bytes.extend(
                    base64::engine::general_purpose::STANDARD
                        .decode(data)
                        .unwrap(),
                );
            }
        }
        bytes
    })
    .await;
    core.shutdown().await;
    let captured = String::from_utf8_lossy(captured.as_ref().expect("feature probe completes"));
    assert!(captured.contains("\r\nFEATURE_OK\r\n"), "{captured}");
}
