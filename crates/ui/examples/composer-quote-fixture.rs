//! Composer quote fixture: put a Markdown blockquote draft in the composer,
//! move the caret between the quote and the question below it, and capture the
//! live preview on the private headless Wayland display. No agent messages are
//! sent.
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

const DRAFT: &str = "> quoted line one\n> quoted line two that wraps across the rail so the bar stays continuous while the text keeps flowing past the column width\n\nmy question";

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
        "composer-quote-fixture",
        None,
        Some(&core.device_id),
        None,
        None,
    )?;
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
                s.selected_chat = Some("composer-quote-fixture".into());
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
                    pause(cx, 1200).await;
                    let quote_end = DRAFT.find('\n').unwrap();
                    let set = |cx: &mut AsyncApp, caret: usize| -> anyhow::Result<()> {
                        window.update(cx, |shell, window, cx| {
                            let composer = shell.fixture_appshots_composer();
                            composer.update(cx, |composer, cx| {
                                composer.fixture_set_draft(DRAFT, caret, window, cx)
                            });
                        })?;
                        Ok(())
                    };
                    set(cx, DRAFT.len())?;
                    pause(cx, 900).await;
                    capture(&output, "composer-quote-dark")?;
                    // Caret inside the quote: the raw marker stays editable on
                    // the line it belongs to.
                    set(cx, quote_end + 3)?;
                    pause(cx, 700).await;
                    capture(&output, "composer-quote-active-dark")?;
                    cx.update(|cx| appearance::set_mode(appearance::AppearanceMode::Light, cx));
                    set(cx, DRAFT.len())?;
                    pause(cx, 900).await;
                    capture(&output, "composer-quote-light")?;
                    let draft = window.update(cx, |s, _, cx| {
                        s.fixture_appshots_composer()
                            .read(cx)
                            .fixture_appshots_draft(cx)
                            .to_owned()
                    })?;
                    anyhow::ensure!(draft == DRAFT, "composer draft must round-trip");
                    std::fs::write(
                        output.join("result.txt"),
                        format!("composer quote fixture:\n---\n{draft}\n"),
                    )?;
                    Ok(())
                }
                .await;
                if let Err(error) = run {
                    eprintln!("composer quote fixture failed: {error:#}");
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
