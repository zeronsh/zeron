//! Compact-mode subagent fold evidence: a live turn that launched four
//! subagents, compact transcript, work fold open — the production Shell over
//! an isolated in-process engine. No agent runs.
//!
//! `cargo run -p zeron-ui --features appshots-fixture,resource-profile --example subagent-fold-fixture -- [hold-secs] [app-id]`
//! prints `READY` once the fold is open, keeps the window up for `hold-secs`
//! (default 20) so the compositor can capture it, then quits.
use gpui::{AppContext, Bounds, WindowBounds, WindowOptions, px, size};
use serde_json::json;
use std::{io::Write, sync::Arc, time::Duration};
use zeron_doc::{MessagePart, MessageRole, MessageStatus, SessionMessageEntry, SubagentStatus};
use zeron_proto::ToolCall;
use zeron_ui::*;

fn port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// A spawn chip bound to its subagent doc, as the claude driver emits it.
fn agent(id: &str, description: &str) -> MessagePart {
    MessagePart::Tool {
        id: id.into(),
        call: ToolCall::Unknown {
            name: format!("Agent: {description}"),
            input: Some(json!({ "description": description, "subagent_type": "general-purpose" })),
        },
        is_error: false,
        resolved: true,
        output: None,
        diff: None,
        output_ref: None,
        output_bytes: None,
        diff_ref: None,
        diff_stats: None,
        subagent_ref: Some(format!("chat--sub--{id}")),
        subagent_status: Some(SubagentStatus::Running),
        subagent_tail: None,
    }
}

/// The reported turn: still streaming while its four subagents run.
fn entries(device: &str) -> Vec<SessionMessageEntry> {
    let now = chrono::Utc::now().timestamp_millis();
    vec![
        SessionMessageEntry {
            id: "u1".into(),
            role: MessageRole::User,
            parts: vec![MessagePart::Text {
                id: "p0".into(),
                text: "Map how the transcript renders tool groups, in parallel, and report back.".into(),
            }],
            created_at: now - 332_000,
            device_id: device.into(),
            status: None,
            continuation_of: None,
            duration_ms: None,
        },
        SessionMessageEntry {
            id: "a1".into(),
            role: MessageRole::Assistant,
            parts: vec![
                MessagePart::Reasoning {
                    id: "r0".into(),
                    text: "Four independent areas — fan them out in parallel.".into(),
                },
                MessagePart::Text {
                    id: "t0".into(),
                    text: "Launching 4 searches in parallel — row building, fold geometry, spawn chips, and tests.".into(),
                },
                agent("s1", "Map transcript row building"),
                agent("s2", "Audit fold height math"),
                agent("s3", "Trace subagent chip rendering"),
                agent("s4", "Find existing fold tests"),
            ],
            created_at: now - 330_000,
            device_id: device.into(),
            status: Some(MessageStatus::Streaming),
            continuation_of: None,
            duration_ms: None,
        },
    ]
}

async fn pause(cx: &mut gpui::AsyncApp, ms: u64) {
    cx.background_executor()
        .timer(Duration::from_millis(ms))
        .await;
}

fn main() -> anyhow::Result<()> {
    let hold: u64 = std::env::args()
        .nth(1)
        .and_then(|secs| secs.parse().ok())
        .unwrap_or(20);
    let app_id = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "zeron-fixture-subagent-fold".into());
    tracing_subscriber::fmt()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "warn".into()))
        .init();
    let temp = tempfile::tempdir()?;
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace)?;
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
        "subagents",
        None,
        Some(&core.device_id),
        None,
        Some(workspace.to_string_lossy().into_owned()),
    )?;
    core.workspace.rename_chat("subagents", "Tool fold audit")?;
    let data = temp.path().join("ui");
    std::fs::create_dir(&data)?;
    // The viewport reads the IPC bearer from its own data dir.
    let ipc_port = port();
    let _ipc = runtime.block_on(zeron_engine::serve_ipc(ipc_port, &data, core.rpc_service()))?;
    let boot = EngineBootConfig {
        data_dir: data.clone(),
        ipc_port,
        edge_url: String::new(),
        edge_token: None,
        org_id: None,
        workos_client_id: None,
        default_harness: zeron_proto::HarnessId::ClaudeCode,
    };
    let handle = runtime.block_on(state::EngineHandle::bootstrap(boot.clone()))?;
    let chats = core.workspace.read_chats()?;
    let device = core.device_id.clone();
    let entries = entries(&device);

    gpui_platform::application()
        .with_assets(icons::Assets)
        .run(move |cx| {
            gpui_tokio::init(cx);
            gpui_base::init(cx);
            let mut settings = settings::UiSettings::default();
            // The reported setup is the compact transcript. Reduced motion
            // lands every fold and reveal on its final frame, so a capture
            // never catches a tween mid-flight.
            settings.transcript_compact_mode = true;
            settings.reduce_motion = motion::ReduceMotion::On;
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
            let state = cx.new(|_| {
                let mut s = state::AppState::new();
                s.fixture_attachment_engine(handle);
                s.connection = zeron_proto::view::ConnectionStatus::Ready;
                s.workspace_scope = Some(zeron_proto::WorkspaceScope::Development);
                s.local_device_id = Some(device.clone());
                s.devices = vec![
                    serde_json::from_value(json!({"id": device, "name": "This device",
                    "platform": std::env::consts::OS, "lastSeenAt": null}))
                    .unwrap(),
                ];
                s.chats = chats;
                s.selected_chat = Some("subagents".into());
                s.auto_selected = true;
                s.chats_synced = true;
                s.spaces_synced = true;
                s.no_project = true;
                s
            });
            let window = cx
                .open_window(
                    WindowOptions {
                        app_id: Some(app_id),
                        window_background: theme::Theme::of(cx).window_background_appearance(),
                        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                            gpui::point(px(40.), px(40.)),
                            size(px(1280.), px(820.)),
                        ))),
                        ..Default::default()
                    },
                    |_, cx| cx.new(|cx| shell::Shell::new(state.clone(), boot, cx)),
                )
                .unwrap();
            cx.on_window_closed(|cx, _| cx.quit()).detach();
            state.update(cx, |_, cx| cx.notify());
            cx.activate(true);
            cx.spawn(async move |cx| {
                pause(cx, 800).await;
                state.update(cx, |s, cx| {
                    s.receive_transcript_frame(
                        zeron_doc::TranscriptFrame::Reset { reset: entries },
                        cx,
                    )
                    .unwrap();
                    cx.notify();
                });
                pause(cx, 600).await;
                // Open the compact work fold, as a click on its header would.
                if let Ok(transcript) = window.update(cx, |shell, _, _| shell.fixture_transcript()) {
                    transcript.update(cx, |t, cx| t.fixture_toggle_fold("a1#work", cx));
                }
                pause(cx, 600).await;
                // A window the compositor never showed gets no frame
                // callbacks: draw and present the settled scene explicitly so
                // a toplevel capture sees it (as browser-fixture does).
                for _ in 0..2 {
                    gpui::AnyWindowHandle::from(window)
                        .update(cx, |_, w, cx| {
                            w.refresh();
                            let arena = w.draw(cx);
                            w.present_if_needed();
                            arena.clear();
                        })
                        .ok();
                    pause(cx, 300).await;
                }
                println!("READY");
                std::io::stdout().flush().ok();
                pause(cx, hold * 1000).await;
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
    runtime.block_on(core.shutdown());
    Ok(())
}
