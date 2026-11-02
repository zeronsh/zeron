//! A side chat's run ending while it shows in the right pane, beside the
//! chat whose session card lists it (#1108). Its lane row must read seen
//! once the run ends in front of you, as the open chat's row does, rather
//! than keep the completed check until its tab is closed and opened again.
//!
//! The production `Shell` attaches to a real engine with isolated data, as
//! at launch, and the side chat's run goes through the engine's command
//! queue to a scripted mock harness: no agent starts and nothing leaves this
//! machine. The seen marker follows the active window, so the fixture
//! activates its own.
//!
//! ```sh
//! cargo run -p zeron-ui --features appshots-fixture --example side-chat-seen-fixture -- <dir>
//! ```
//!
//! Writes `running.png` (the run under way), `finished.png` (two seconds
//! after it ended) and `result.txt`, and prints PASS once the lane row reads
//! seen, FAIL with what it read otherwise, or INCONCLUSIVE (exit 2) when the
//! window had lost focus as the run ended.
use anyhow::Context as _;
use gpui::{AppContext, AsyncApp, Bounds, Entity, WindowBounds, WindowOptions, px, size};
use serde_json::json;
use std::{path::Path, sync::Arc, time::Duration};
use zeron_doc::{
    MessagePart, MessageRole, MessageStatus, SessionCommandPayload, SessionMessageEntry,
};
use zeron_proto::{AgentEvent, DoneStatus, HarnessId, RunRequest, SandboxLevel, SessionStatus};
use zeron_rpc::methods;
use zeron_ui::*;

const PROJECT: &str = "release-tools";
const MAIN: &str = "release-upload";
const SIDE: &str = "backoff-timings";

/// The side chat's reply, one event per `ZERON_MOCK_DELAY_MS`.
fn script() -> Vec<AgentEvent> {
    vec![
        AgentEvent::TextDelta {
            text: "Reading the retry loop in `scripts/release.sh`.\n\n".into(),
        },
        AgentEvent::ToolCall {
            id: "read".into(),
            call: zeron_proto::ToolCall::Exec {
                command: "rg -n 'sleep|attempt' scripts/release.sh".into(),
            },
        },
        AgentEvent::ToolResult {
            id: "read".into(),
            is_error: false,
            output: None,
            diff: None,
        },
        AgentEvent::TextDelta {
            text: "The three waits add up to 14 s (2 s, 4 s, then 8 s), well inside the \
                   job's 10-minute timeout, so the retries can't push the release over it."
                .into(),
        },
        AgentEvent::Done {
            status: DoneStatus::Completed,
            result: None,
            error: None,
            session_id: None,
        },
    ]
}

/// The open chat's conversation so far.
fn conversation(device: &str) -> Vec<SessionMessageEntry> {
    let now = chrono::Utc::now();
    let entry = |id: &str, role, minutes_ago: i64, text: &str| SessionMessageEntry {
        id: id.into(),
        role,
        parts: vec![MessagePart::Text {
            id: format!("{id}-text"),
            text: text.into(),
        }],
        created_at: (now - chrono::Duration::minutes(minutes_ago)).timestamp_millis(),
        device_id: device.into(),
        status: Some(MessageStatus::Complete),
        continuation_of: None,
        duration_ms: None,
    };
    vec![
        entry(
            "u1",
            MessageRole::User,
            14,
            "Can you add retries to the release upload? It failed twice on a flaky network \
             last night.",
        ),
        entry(
            "a1",
            MessageRole::Assistant,
            12,
            "Added a retry with exponential backoff around the upload in \
             `scripts/release.sh`: three attempts, waiting 2 s, then 4 s, then 8 s. Each \
             failed attempt logs its HTTP status, and the last one exits non-zero so the job \
             fails.\n\nAgainst a local server that drops the first two requests, the upload \
             went through on the third.",
        ),
    ]
}

async fn pause(cx: &mut AsyncApp, ms: u64) {
    cx.background_executor()
        .timer(Duration::from_millis(ms))
        .await;
}

/// Wait up to `limit_ms` for `ready` to hold over the shell's state.
async fn until(
    state: &Entity<state::AppState>,
    cx: &mut AsyncApp,
    what: &str,
    limit_ms: u64,
    ready: impl Fn(&state::AppState) -> bool,
) -> anyhow::Result<()> {
    let mut waited = 0;
    while !state.read_with(cx, |s, _| ready(s)) {
        anyhow::ensure!(waited < limit_ms, "timed out waiting for {what}");
        pause(cx, 100).await;
        waited += 100;
    }
    Ok(())
}

fn side_status(state: &state::AppState) -> Option<SessionStatus> {
    state.session_for(SIDE).map(|session| session.status)
}

fn capture(
    window: gpui::AnyWindowHandle,
    cx: &mut AsyncApp,
    directory: &Path,
    name: &str,
) -> anyhow::Result<()> {
    window.update(cx, |_, w, cx| {
        w.draw(cx).clear();
        w.render_to_image()?
            .save(directory.join(format!("{name}.png")))?;
        Ok(())
    })?
}

fn port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

async fn drive(
    window: gpui::WindowHandle<shell::Shell>,
    state: &Entity<state::AppState>,
    cwd: &str,
    output: &Path,
    cx: &mut AsyncApp,
) -> anyhow::Result<Verdict> {
    until(state, cx, "the engine to attach", 20_000, |s| {
        matches!(s.connection, zeron_proto::view::ConnectionStatus::Ready) && s.chats_synced
    })
    .await?;
    window.update(cx, |shell, _, cx| shell.fixture_open_chat(MAIN, cx))?;
    pause(cx, 1_500).await;
    window.update(cx, |shell, _, cx| {
        shell.fixture_open_side_chat(SIDE, cx);
        // Room for the session card beside the transcript.
        shell.fixture_right_pane_width(372.0, cx);
    })?;
    pause(cx, 1_500).await;

    // Sent to the side chat the way its composer sends: a queued run.
    let engine = state
        .read_with(cx, |s, _| s.engine().cloned())
        .context("no engine")?;
    let command = serde_json::to_value(SessionCommandPayload::Run {
        request: RunRequest {
            mcp: None,
            prompt: "Do the retry waits fit inside the job's timeout?".into(),
            harness: Some(HarnessId::Mock),
            model: None,
            reasoning: None,
            model_options: serde_json::Map::new(),
            cwd: cwd.into(),
            sandbox: SandboxLevel::WorkspaceWrite,
            auto_approve: false,
            access: Default::default(),
            plan: false,
            resume: None,
            attachments: Vec::new(),
            worktree: None,
            env: Default::default(),
        },
        message_id: "side-ask".into(),
    })?;
    engine
        .client()
        .call(
            methods::QUEUE_COMMAND,
            json!({ "chatId": SIDE, "command": command }),
        )
        .await?;
    until(state, cx, "the side chat's run to start", 20_000, |s| {
        side_status(s) == Some(SessionStatus::Working)
    })
    .await?;
    pause(cx, 1_200).await;
    capture(window.into(), cx, output, "running")?;
    until(state, cx, "the side chat's run to end", 30_000, |s| {
        side_status(s) == Some(SessionStatus::Idle)
            && s.chats
                .iter()
                .any(|c| c.id == SIDE && c.last_message_at.is_some())
    })
    .await?;
    // Seen follows the active window: had focus gone elsewhere (another
    // app taken to the front), a completed row would prove nothing.
    let active = window.update(cx, |_, w, _| w.is_window_active())?;
    pause(cx, 2_000).await;
    capture(window.into(), cx, output, "finished")?;

    let (status, last_message, last_seen) = state.read_with(cx, |s, _| {
        let row = s.chats.iter().find(|c| c.id == SIDE);
        (
            row.map(|c| s.display_status_for(c, chrono::Utc::now())),
            row.and_then(|c| c.last_message_at),
            row.and_then(|c| c.last_seen_at),
        )
    });
    let verdict = if status == Some(zeron_proto::ChatIndicator::Idle) {
        Verdict::Pass
    } else if active {
        Verdict::Fail
    } else {
        Verdict::Inconclusive
    };
    let report = format!(
        "window active as the run ended: {active}\nside chat's lane row: {status:?}\nlast message: {last_message:?}\nlast seen: {last_seen:?}\n{}\n",
        match verdict {
            Verdict::Pass => "PASS",
            Verdict::Fail => "FAIL: the side chat on screen is not marked seen",
            Verdict::Inconclusive => "INCONCLUSIVE: the window was not active as the run ended",
        }
    );
    print!("{report}");
    std::fs::write(output.join("result.txt"), report)?;
    Ok(verdict)
}

#[derive(Clone, Copy)]
enum Verdict {
    Pass,
    Fail,
    Inconclusive,
}

fn main() -> anyhow::Result<()> {
    let output = std::path::PathBuf::from(std::env::args().nth(1).context("output directory")?);
    std::fs::create_dir_all(&output)?;
    let output = output.canonicalize()?;
    // SAFETY: no other thread exists yet. The mock paces its reply so the
    // run shows under way; the device lists under a neutral name.
    unsafe {
        std::env::set_var("ZERON_MOCK_DELAY_MS", "700");
        std::env::set_var("ZERON_DEVICE_NAME", "MacBook Pro");
    }
    tracing_subscriber::fmt().with_env_filter("warn").init();
    let temp = tempfile::tempdir()?;
    let runtime = tokio::runtime::Runtime::new()?;
    let registry = zeron_engine::HarnessRegistry::new();
    registry.register(Arc::new(zeron_harness::mock::MockHarness {
        script: script(),
    }));
    let core = runtime.block_on(async {
        zeron_engine::EngineCore::assemble(
            &temp.path().join("engine"),
            Arc::new(registry),
            HarnessId::Mock,
            None,
        )
    })?;
    let device = core.device_id.clone();
    let project = temp.path().join(PROJECT);
    std::fs::create_dir_all(&project)?;
    let cwd = project.to_string_lossy().into_owned();
    core.workspace
        .create_space(PROJECT, &device, &cwd, Some(PROJECT.into()), false)?;
    let config: zeron_proto::ChatConfig = serde_json::from_value(json!({
        "harness": "mock", "model": "fable-5", "reasoning": null, "sandbox": "workspace-write"
    }))?;
    core.workspace
        .create_chat(MAIN, Some(PROJECT), None, Some(config.clone()), None)?;
    core.workspace
        .rename_chat(MAIN, "Retry the release upload")?;
    core.workspace.create_chat_with_parent(
        SIDE,
        Some(PROJECT),
        None,
        Some(config),
        None,
        Some(MAIN.into()),
    )?;
    core.workspace
        .rename_chat(SIDE, "Check the backoff timings")?;
    runtime.block_on(async {
        let chat = core.doc_host.open(MAIN)?;
        for entry in conversation(&device) {
            chat.doc().push_message(&entry)?;
        }
        anyhow::Ok(())
    })?;
    let ipc_port = port();
    let data = temp.path().join("ui");
    std::fs::create_dir(&data)?;
    let _ipc = runtime.block_on(zeron_engine::serve_ipc(ipc_port, &data, core.rpc_service()))?;
    let boot = EngineBootConfig {
        data_dir: data.clone(),
        ipc_port,
        edge_url: String::new(),
        edge_token: None,
        org_id: None,
        workos_client_id: None,
        default_harness: HarnessId::Mock,
    };
    gpui_platform::application()
        .with_assets(icons::Assets)
        .run(move |cx| {
            gpui_tokio::init(cx);
            gpui_base::init(cx);
            let mut settings = settings::UiSettings::default();
            // Room to dock the session card beside the transcript while the
            // right pane shows the side chat, in a 1300-point window.
            settings.sidebar_collapsed = true;
            settings.transcript_width = settings::TRANSCRIPT_WIDTH_MIN;
            settings.surface = zeron_theme::SurfacePreference::Opaque;
            settings::init(settings.clone(), data.clone(), cx);
            let fonts = typography::register_fonts(cx);
            typography::init(
                settings.ui_font_family.clone(),
                settings.ui_font_size,
                settings.terminal_font_family.clone(),
                settings.terminal_font_size,
                settings.code_font_family.clone(),
                settings.code_font_size,
                fonts,
                cx,
            );
            theme_library::init(data.clone(), cx);
            appearance::init(
                appearance::AppearanceMode::Dark,
                settings.theme_selection,
                settings.accent,
                settings.surface,
                cx,
            );
            history::init(
                settings.git_history_columns,
                settings.git_history_column_widths,
                settings.git_history_column_order,
                settings.git_history_author_display,
                cx,
            );
            composer::init(cx, settings.composer_send_behavior);
            terminal::panel::init(cx);
            app_menus::init(cx);
            let state = cx.new(|_| state::AppState::new());
            let window = cx
                .open_window(
                    WindowOptions {
                        window_background: theme::Theme::of(cx).window_background_appearance(),
                        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                            gpui::point(px(16.), px(40.)),
                            size(px(1300.), px(900.)),
                        ))),
                        ..Default::default()
                    },
                    |_, cx| cx.new(|cx| shell::Shell::new(state.clone(), boot.clone(), cx)),
                )
                .unwrap();
            // Attach as at launch: the engine's watches fill the state.
            state::AppState::bootstrap(state.clone(), boot, cx);
            cx.activate(true);
            cx.spawn(async move |cx| {
                let code = match drive(window, &state, &cwd, &output, cx).await {
                    Ok(Verdict::Pass) => 0,
                    Ok(Verdict::Fail) => 1,
                    Ok(Verdict::Inconclusive) => 2,
                    Err(error) => {
                        eprintln!("side chat seen fixture failed: {error:#}");
                        1
                    }
                };
                // On macOS quit ends the process: give the verdict first.
                if code != 0 {
                    std::process::exit(code);
                }
                let _ = window.update(cx, |_, w, _| w.remove_window());
                pause(cx, 100).await;
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
    runtime.block_on(core.shutdown());
    Ok(())
}
