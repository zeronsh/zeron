//! Selection-bar fixture: select part of a reply, show the floating action
//! bar, then run "Add to chat" and capture the composer holding the quote —
//! all against isolated fixture data. No agent messages are sent.
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

const REPLY: &str = "Selecting text in the transcript reveals a small action bar above the selection. \
Add to chat appends the selection to the composer as a Markdown quote, while Reply in side chat forks \
the conversation at the message the selection ends in.\n\nThe bar hides again as soon as the selection \
clears, the transcript scrolls, or Escape is pressed.";

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
    core.workspace
        .create_chat("selection-fixture", None, Some(&core.device_id), None, None)?;
    core.workspace
        .rename_chat("selection-fixture", "Selection actions")?;
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
                s.selected_chat = Some("selection-fixture".into());
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
                         "parts":[{"id":"text","kind":"text","text":"How do the selection actions behave?"}]},
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
                    window.update(cx, |s, _, cx| s.fixture_appshots_transcript_end(cx))?;
                    pause(cx, 400).await;
                    let selected = window.update(cx, |s, _, cx| {
                        // Start the selection mid-paragraph, with a full
                        // line of prose above it, so the bar floats over text:
                        // with a translucent surface, that text would show
                        // through if the card did not blur its backdrop.
                        let start = REPLY
                            .find("while Reply in side chat")
                            .expect("selection anchor must exist");
                        s.fixture_appshots_select_range(
                            "assistant#text.0:0",
                            start..start + 150,
                            cx,
                        )
                    })?;
                    anyhow::ensure!(selected, "the reply paragraph must be painted");
                    pause(cx, 500).await;
                    // Capture the opaque pair explicitly; the default surface
                    // is translucent on this fixture's themes.
                    cx.update(|cx| {
                        appearance::set_surface(
                            zeron_theme::SurfacePreference::Opaque,
                            cx,
                        )
                    });
                    pause(cx, 700).await;
                    capture(window.into(), cx, &output, "selection-bar-dark")?;
                    cx.update(|cx| appearance::set_mode(appearance::AppearanceMode::Light, cx));
                    pause(cx, 700).await;
                    capture(window.into(), cx, &output, "selection-bar-light")?;
                    cx.update(|cx| {
                        appearance::set_surface(
                            zeron_theme::SurfacePreference::Frosted,
                            cx,
                        );
                        appearance::set_mode(appearance::AppearanceMode::Dark, cx);
                    });
                    pause(cx, 800).await;
                    capture(
                        window.into(),
                        cx,
                        &output,
                        "selection-bar-frost-dark",
                    )?;
                    cx.update(|cx| appearance::set_mode(appearance::AppearanceMode::Light, cx));
                    pause(cx, 700).await;
                    capture(
                        window.into(),
                        cx,
                        &output,
                        "selection-bar-frost-light",
                    )?;
                    cx.update(|cx| appearance::set_mode(appearance::AppearanceMode::Dark, cx));
                    pause(cx, 700).await;
                    window.update(cx, |s, _, cx| s.fixture_appshots_add_selection(cx))?;
                    pause(cx, 900).await;
                    capture(window.into(), cx, &output, "selection-added-dark")?;
                    let draft = window.update(cx, |s, _, cx| {
                        s.fixture_appshots_composer()
                            .read(cx)
                            .fixture_appshots_draft(cx)
                            .to_owned()
                    })?;
                    anyhow::ensure!(
                        draft.starts_with("> "),
                        "composer must hold the quote, got {draft:?}"
                    );
                    std::fs::write(
                        output.join("result.txt"),
                        format!(
                            "selection fixture: bar opaque/frost dark+light + quoted draft\n---\n{draft}"
                        ),
                    )?;
                    Ok(())
                }
                .await;
                if let Err(error) = run {
                    eprintln!("selection fixture failed: {error:#}");
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
