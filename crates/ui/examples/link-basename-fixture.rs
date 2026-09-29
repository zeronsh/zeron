//! File-link label fixture: a canned reply whose inline-code paths resolve
//! to files in a temp checkout, one inside a sentence and three as a list,
//! plus a span that resolves to nothing. Captures the labels on the private
//! headless Wayland display. No agent messages are sent.
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
fn capture(directory: &std::path::Path, name: &str) -> anyhow::Result<()> {
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

const REPLY: &str = "The entry point is `2026-09-29/Some Long Folder Name/SOURCES.md`, which lists \
the source notes for that day.\n\nFiles to review:\n\n- \
`2026-09-29/Some Long Folder Name/SOURCES.md`\n- \
`2026-09-29/Another Long Folder Name/DESCRIPTION.txt`\n- \
`2026-09-29/A Third Long Folder Name/README.md`\n\nOne span stays code: \
`2026-09-29/Missing Folder/NOTES.md`.";

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter("warn").init();
    let output = PathBuf::from(std::env::args().nth(1).expect("output directory"));
    std::fs::create_dir_all(&output)?;
    let temp = tempfile::tempdir()?;
    let workspace = temp.path().join("checkout");
    for relative in [
        "2026-09-29/Some Long Folder Name/SOURCES.md",
        "2026-09-29/Another Long Folder Name/DESCRIPTION.txt",
        "2026-09-29/A Third Long Folder Name/README.md",
    ] {
        let path = workspace.join(relative);
        std::fs::create_dir_all(path.parent().expect("nested file"))?;
        std::fs::write(path, "fixture\n")?;
    }
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
        "link-basename-fixture",
        None,
        Some(&core.device_id),
        None,
        Some(workspace.to_string_lossy().into_owned()),
    )?;
    core.workspace
        .rename_chat("link-basename-fixture", "File link labels")?;
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
    let failure = Arc::new(std::sync::Mutex::new(None::<String>));
    let result = failure.clone();
    gpui_platform::application()
        .with_assets(zeron_ui::icons::Assets)
        .run(move |cx| {
            use zeron_ui::*;
            gpui_tokio::init(cx);
            gpui_base::init(cx);
            let settings = settings::UiSettings::default();
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
                    serde_json::from_value(serde_json::json!({
                        "id": device, "name": "This device",
                        "platform": std::env::consts::OS, "lastSeenAt": null
                    }))
                    .unwrap(),
                ];
                s.chats = chats;
                s.selected_chat = Some("link-basename-fixture".into());
                s.auto_selected = true;
                s.chats_synced = true;
                s.spaces_synced = true;
                s.no_project = true;
                s
            });
            let window = cx
                .open_window(
                    WindowOptions {
                        window_background: theme::Theme::of(cx).window_background_appearance(),
                        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                            gpui::point(px(20.), px(40.)),
                            size(px(1100.), px(850.)),
                        ))),
                        ..Default::default()
                    },
                    |_, cx| cx.new(|cx| shell::Shell::new(state.clone(), boot, cx)),
                )
                .unwrap();
            state.update(cx, |_, cx| cx.notify());
            cx.activate(true);
            cx.spawn(async move |cx| {
                let run: anyhow::Result<()> = async {
                    let transcript = serde_json::json!([
                        {"id":"prompt","role":"user","createdAt":1788900000000_i64,"deviceId":device,
                         "parts":[{"id":"text","kind":"text","text":"Which notes describe the sources?"}]},
                        {"id":"assistant","role":"assistant","status":"complete","createdAt":1788900001000_i64,"deviceId":device,
                         "parts":[{"id":"text","kind":"text","text":REPLY}]}
                    ]);
                    pause(cx, 1000).await;
                    state.update(cx, |s, cx| {
                        s.receive_transcript_frame(
                            zeron_doc::TranscriptFrame::Reset {
                                reset: serde_json::from_value(transcript).unwrap(),
                            },
                            cx,
                        )
                        .unwrap();
                        cx.notify();
                    });
                    pause(cx, 1200).await;
                    window.update(cx, |s, _, cx| s.fixture_appshots_transcript_start(cx))?;
                    pause(cx, 500).await;
                    capture(&output, "basename-links-dark")?;
                    cx.update(|cx| appearance::set_mode(appearance::AppearanceMode::Light, cx));
                    pause(cx, 800).await;
                    capture(&output, "basename-links-light")?;
                    std::fs::write(
                        output.join("result.txt"),
                        format!("file link label fixture:\n---\n{REPLY}\n"),
                    )?;
                    Ok(())
                }
                .await;
                if let Err(error) = run {
                    eprintln!("file link label fixture failed: {error:#}");
                    *result.lock().unwrap() = Some(format!("{error:#}"));
                }
                let _ = window.update(cx, |_, w, _| w.remove_window());
                pause(cx, 100).await;
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
    runtime.block_on(core.shutdown());
    if let Some(error) = failure.lock().unwrap().take() {
        anyhow::bail!(error);
    }
    Ok(())
}
