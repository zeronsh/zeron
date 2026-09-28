//! Orchestration transcript fixture for appshots: `zeron chat` exec chips
//! (spawn/tell/wait/output), a native subagent chip beside them for
//! comparison, and single and grouped child-update cards — all against
//! isolated fixture data. No agent messages are sent.
use gpui::{AppContext, AsyncApp, Bounds, WindowBounds, WindowOptions, px, size};
use std::{path::PathBuf, sync::Arc, time::Duration};

async fn pause(cx: &mut AsyncApp, ms: u64) {
    cx.background_executor()
        .timer(Duration::from_millis(ms))
        .await;
}

/// `render_to_image` is unimplemented on the Linux backend, so the fixture
/// runs on the private headless Wayland display and the compositor output is
/// grabbed with `grim` — real chrome, real translucency, nothing on the
/// user's desktop.
fn capture(
    _window: gpui::AnyWindowHandle,
    _cx: &mut AsyncApp,
    directory: &std::path::Path,
    name: &str,
) -> anyhow::Result<()> {
    let out = std::process::Command::new("grim")
        .arg(directory.join(format!("{name}.png")))
        .output()?;
    anyhow::ensure!(
        out.status.success(),
        "grim failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(())
}

fn port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn chat_config(harness: zeron_proto::HarnessId, model: &str) -> zeron_proto::ChatConfig {
    zeron_proto::ChatConfig {
        harness,
        model: Some(model.into()),
        reasoning: None,
        model_options: Default::default(),
        sandbox: zeron_proto::SandboxLevel::WorkspaceWrite,
    }
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter("warn").init();
    let output = PathBuf::from(std::env::args().nth(1).expect("output directory"));
    std::fs::create_dir_all(&output)?;
    let temp = tempfile::tempdir()?;
    let runtime = tokio::runtime::Runtime::new()?;
    let core = runtime.block_on(async {
        zeron_engine::EngineCore::assemble(
            &temp.path().join("engine"),
            Arc::new(zeron_engine::default_registry()),
            zeron_proto::HarnessId::ClaudeCode,
            None,
        )
    })?;
    core.workspace.create_chat(
        "fan-out-parent",
        None,
        Some(&core.device_id),
        Some(chat_config(
            zeron_proto::HarnessId::ClaudeCode,
            "claude-opus-4-7",
        )),
        None,
    )?;
    core.workspace
        .rename_chat("fan-out-parent", "Fan out the review")?;
    // The spawned children the chips and update cards resolve against.
    let child_a = "3f6b2a18-9c4d-4e5f-8a7b-1c2d3e4f5a6b";
    let child_b = "aa10bb22-1111-2222-3333-444455556666";
    core.workspace.create_chat_with_parent(
        child_a,
        None,
        Some(&core.device_id),
        Some(chat_config(zeron_proto::HarnessId::Codex, "gpt-5.3-codex")),
        None,
        Some("fan-out-parent".into()),
        true,
    )?;
    core.workspace
        .rename_chat(child_a, "Audit the sync layer (Codex)")?;
    core.workspace.create_chat_with_parent(
        child_b,
        None,
        Some(&core.device_id),
        Some(chat_config(
            zeron_proto::HarnessId::ClaudeCode,
            "claude-sonnet-4-6",
        )),
        None,
        Some("fan-out-parent".into()),
        true,
    )?;
    core.workspace
        .rename_chat(child_b, "Sketch the retry backoff")?;
    // A user-opened side chat — belongs on the Chats tab only.
    core.workspace.create_chat_with_parent(
        "side-chat",
        None,
        Some(&core.device_id),
        Some(chat_config(zeron_proto::HarnessId::Pi, "pi-1")),
        None,
        Some("fan-out-parent".into()),
        false,
    )?;
    core.workspace.rename_chat("side-chat", "Quick follow-up")?;
    let ipc_port = port();
    let _ipc = runtime.block_on(zeron_engine::serve_ipc(ipc_port, core.rpc_service()))?;
    let data = temp.path().join("ui");
    std::fs::create_dir(&data)?;
    let boot = zeron_ui::EngineBootConfig {
        data_dir: data.clone(),
        ipc_port,
        edge_url: String::new(),
        edge_token: None,
        org_id: None,
        workos_client_id: None,
        default_harness: zeron_proto::HarnessId::ClaudeCode,
    };
    let handle = runtime.block_on(zeron_ui::state::EngineHandle::bootstrap(boot.clone()))?;
    let chats = core.workspace.read_chats()?;
    let device = core.device_id.clone();
    let failure = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
    let result = failure.clone();
    gpui_platform::application().with_assets(zeron_ui::icons::Assets).run(move |cx| {
        use zeron_ui::*;
        gpui_tokio::init(cx); gpui_base::init(cx);
        let settings=settings::UiSettings::default(); settings::init(settings.clone(),data.clone(),cx);
        let fonts=typography::register_fonts(cx); typography::init(settings.ui_font_family.clone(), settings.ui_font_size, settings.terminal_font_family.clone(), settings.terminal_font_size, settings.code_font_family.clone(), settings.code_font_size, fonts, cx);
        theme_library::init(data.clone(),cx); appearance::init(appearance::AppearanceMode::Dark,settings.theme_selection,settings.accent,settings.surface,cx);
        history::init(settings.git_history_columns,settings.git_history_column_widths,settings.git_history_column_order,settings.git_history_author_display,cx);
        composer::init(cx,settings.composer_send_behavior); terminal::panel::init(cx); app_menus::init(cx);
        let state=cx.new(|_| { let mut s=state::AppState::new(); s.fixture_attachment_engine(handle); s.connection=zeron_proto::view::ConnectionStatus::Ready; s.workspace_scope=Some(zeron_proto::WorkspaceScope::Development); s.local_device_id=Some(device.clone()); s.devices=vec![serde_json::from_value(serde_json::json!({"id":device,"name":"This device","platform":std::env::consts::OS,"lastSeenAt":null})).unwrap()]; s.chats=chats; s.selected_chat=Some("fan-out-parent".into()); s.auto_selected=true; s.chats_synced=true; s.spaces_synced=true; s.no_project=true; s });
        // One child is still mid-run: its send reads as Working, so the
        // spawn chip and the update cards show the live spinner.
        state.update(cx,|s,_| s.begin_pending_send(child_a,"pending-send",chrono::Utc::now()));
        let window=cx.open_window(WindowOptions {window_background:theme::Theme::of(cx).window_background_appearance(),window_bounds:Some(WindowBounds::Windowed(Bounds::new(gpui::point(px(20.),px(40.)),size(px(1100.),px(900.))))),..Default::default()},|_,cx|cx.new(|cx|shell::Shell::new(state.clone(),boot,cx))).unwrap();
        state.update(cx,|_,cx|cx.notify()); cx.activate(true);
        cx.spawn(async move |cx| {
            let run:anyhow::Result<()>=async {
                let transcript = serde_json::json!([
                    {"id":"u1","role":"user","parts":[{"id":"t","kind":"text","text":"Fan out the review: audit the sync layer and sketch the retry backoff, then read both reports back."}],"createdAt":1788900000000_i64,"deviceId":device},
                    {"id":"a1","role":"assistant","status":"complete","createdAt":1788900001000_i64,"deviceId":device,"parts":[
                        {"id":"p0","kind":"text","text":"Splitting the work into two child chats now — I'll wait for both and read their outputs."},
                        {"id":"n1","kind":"tool","call":{"kind":"unknown","name":"Agent: Verify the lock ordering","input":{"model":"claude-opus-4-7"}},"resolved":true,"subagentRef":"fan-out-parent--sub--n1","subagentStatus":"done"},
                        {"id":"t1","kind":"tool","call":{"kind":"exec","command":"zeron chat spawn --title \"Audit the sync layer\" --harness codex --model gpt-5.3-codex --prompt-file /tmp/brief-sync.md"},"resolved":true,"output":"Spawning child chat…\n@chat:3f6b2a18-9c4d-4e5f-8a7b-1c2d3e4f5a6b"},
                        {"id":"t2","kind":"tool","call":{"kind":"exec","command":"zeron chat spawn --title \"Sketch the retry backoff\" --model claude-sonnet-4-6 --prompt-file /tmp/brief-backoff.md"},"resolved":true,"output":"Spawning child chat…\n@chat:aa10bb22-1111-2222-3333-444455556666"},
                        {"id":"t3","kind":"tool","call":{"kind":"exec","command":"zeron chat tell 3f6b2a18 \"focus on the ledger rewrite\" --mode steer"},"resolved":true,"output":"delivered"},
                        {"id":"t4","kind":"tool","call":{"kind":"exec","command":"zeron chat spawn --title \"Sweep stale locks\" --prompt-file /tmp/brief-locks.md"},"resolved":false},
                        {"id":"t5","kind":"tool","call":{"kind":"exec","command":"zeron chat wait aa10bb22 --timeout 30m"},"resolved":true,"output":"aa10bb22 completed"}
                    ]},
                    {"id":"u2","role":"assistant","status":"complete","createdAt":1788900002000_i64,"deviceId":device,"parts":[
                        {"id":"cu1","kind":"childUpdate","childChatId":"aa10bb22-1111-2222-3333-444455556666","childTitle":"Sketch the retry backoff","outcome":"completed","excerpt":"Proposed a capped exponential backoff: 250ms base, 8s ceiling, jittered ±20%. The sync registry retries transient writes inline and escalates permanent failures to the change queue."}
                    ]},
                    {"id":"a2","role":"assistant","status":"complete","createdAt":1788900003000_i64,"deviceId":device,"parts":[
                        {"id":"t6","kind":"tool","call":{"kind":"exec","command":"zeron chat output aa10bb22"},"resolved":true,"output":"Backoff design: capped exponential, 250ms→8s, ±20% jitter.\nTransient write errors retry inline; permanent failures go to the change queue.\nOpen question: whether the ledger rewrite needs its own budget."},
                        {"id":"t7","kind":"tool","call":{"kind":"exec","command":"zeron chat wait 3f6b2a18 stale-child-9 --timeout 30m"},"resolved":true,"output":"stale-child-9 failed\n3f6b2a18 still running"}
                    ]},
                    {"id":"u3","role":"assistant","status":"complete","createdAt":1788900004000_i64,"deviceId":device,"parts":[
                        {"id":"cu2","kind":"childUpdate","childChatId":"3f6b2a18-9c4d-4e5f-8a7b-1c2d3e4f5a6b","childTitle":"Audit the sync layer","outcome":"needsInput","excerpt":"The ledger rewrite touches the registry's write path — do you want the audit read-only, or may the child propose the migration diff too?"},
                        {"id":"cu3","kind":"childUpdate","childChatId":"missing-child-id","childTitle":"Sweep stale locks","outcome":"errored","excerpt":"thread panicked at 'lock table mismatch'"}
                    ]},
                    {"id":"m1","role":"user","createdAt":1788900004500_i64,"deviceId":device,"parts":[{"id":"t","kind":"text","text":"[Message from Zeron chat Audit the sync layer (@chat:3f6b2a18-9c4d-4e5f-8a7b-1c2d3e4f5a6b). Reply with `zeron chat tell 3f6b2a18 <message>`.]\n\nRead-only audit is done. The ledger rewrite needs one migration: add a nullable `settled_at` column before the backfill runs.\n\nLock ordering is consistent across every write path I traced."}]},
                    {"id":"a3","role":"assistant","status":"complete","createdAt":1788900005000_i64,"deviceId":device,"parts":[
                        {"id":"p1","kind":"text","text":"Backoff sketch is done. The sync audit needs a decision before it continues, and the lock sweep crashed — I'll summarize once the audit settles. Waiting on @chat:3f6b2a18-9c4d-4e5f-8a7b-1c2d3e4f5a6b before summarizing.\n\n@chat:aa10bb22-1111-2222-3333-444455556666"},
                        {"id":"t8","kind":"tool","call":{"kind":"exec","command":"zeron chat tell stale-child-9 \"restart with the fixture lock table\""},"resolved":true,"isError":true,"output":"error: no such chat: stale-child-9"}
                    ]}
                ]);
                pause(cx,1000).await;
                state.update(cx,|s,cx| {s.receive_transcript_frame(zeron_doc::TranscriptFrame::Reset {reset:serde_json::from_value(transcript).unwrap()},cx).unwrap();cx.notify();});
                pause(cx,1200).await;
                window.update(cx,|s,_,cx|s.fixture_appshots_transcript_start(cx))?;
                pause(cx,500).await;
                capture(window.into(),cx,&output,"orchestration-transcript-dark")?;
                window.update(cx,|s,_,cx|s.fixture_appshots_transcript_agent_folds(true,cx))?;
                pause(cx,700).await;
                capture(window.into(),cx,&output,"orchestration-expanded-dark")?;
                window.update(cx,|s,_,cx|s.fixture_appshots_transcript_end(cx))?;
                pause(cx,600).await;
                capture(window.into(),cx,&output,"orchestration-expanded-end-dark")?;
                cx.update(|cx|appearance::set_mode(appearance::AppearanceMode::Light,cx));
                pause(cx,700).await;
                capture(window.into(),cx,&output,"orchestration-expanded-end-light")?;
                window.update(cx,|s,_,cx|s.fixture_appshots_transcript_start(cx))?;
                pause(cx,500).await;
                capture(window.into(),cx,&output,"orchestration-expanded-light")?;
                window.update(cx,|s,_,cx|s.fixture_appshots_transcript_agent_folds(false,cx))?;
                pause(cx,700).await;
                capture(window.into(),cx,&output,"orchestration-transcript-light")?;
                std::fs::write(output.join("result.txt"),"orchestration: collapsible agent activity groups, child-update report rows. Fixture data only; no agent traffic.\n")?;
                Ok(())
            }.await;
            if let Err(error)=run {eprintln!("orchestration fixture failed: {error:#}");*result.lock().unwrap()=Some(format!("{error:#}"));}
            let _=window.update(cx,|_,w,_|w.remove_window());pause(cx,100).await;cx.update(|cx|cx.quit());
        }).detach();
    });
    runtime.block_on(core.shutdown());
    if let Some(error) = failure.lock().unwrap().take() {
        anyhow::bail!(error);
    }
    Ok(())
}
