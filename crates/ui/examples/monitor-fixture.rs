//! Native Monitor surface evidence with isolated data. Renders the production
//! shell against an in-process engine, so every figure is this machine's own,
//! delivered through the real `WatchSystemStats` stream. No agent messages are
//! sent.
//!
//!     cargo run -p zeron-ui --example monitor-fixture \
//!         --features monitor-fixture -- <output dir>
use gpui::{AppContext, AsyncApp, Bounds, WindowBounds, WindowOptions, px, size};
use std::{path::PathBuf, sync::Arc, time::Duration};
use zeron_ui::*;

async fn pause(cx: &mut AsyncApp, ms: u64) {
    cx.background_executor()
        .timer(Duration::from_millis(ms))
        .await;
}

fn capture(
    window: gpui::AnyWindowHandle,
    cx: &mut AsyncApp,
    directory: &std::path::Path,
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
        .create_chat("monitor-fixture", None, Some(&core.device_id), None, None)?;
    core.workspace
        .rename_chat("monitor-fixture", "Profile idle CPU usage")?;
    let ipc_port = port();
    let _ipc = runtime.block_on(zeron_engine::serve_ipc(ipc_port, core.rpc_service()))?;
    let data = temp.path().join("ui");
    std::fs::create_dir(&data)?;
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
    let failure = Arc::new(std::sync::Mutex::new(None));
    let result = failure.clone();
    gpui_platform::application()
        .with_assets(icons::Assets)
        .run(move |cx| {
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
                        "id": device,
                        "name": "This device",
                        "platform": std::env::consts::OS,
                        "lastSeenAt": null
                    }))
                    .unwrap(),
                ];
                s.chats = chats;
                s.selected_chat = Some("monitor-fixture".into());
                s.auto_selected = true;
                s.chats_synced = true;
                s.spaces_synced = true;
                s
            });
            let window = cx
                .open_window(
                    WindowOptions {
                        window_background: theme::Theme::of(cx).window_background_appearance(),
                        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                            gpui::point(px(20.), px(40.)),
                            size(px(1100.), px(900.)),
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
                    pause(cx, 1000).await;
                    window.update(cx, |shell, _, cx| shell.fixture_open_monitor(cx))?;
                    // The stream primes CPU deltas, then ticks every two seconds:
                    // let the one-minute graphs fill with real samples.
                    pause(cx, 64_000).await;
                    let view = |cx: &mut AsyncApp, cores, memory, scroll: f32| {
                        window.update(cx, |shell, _, cx| {
                            shell.fixture_monitor_view(cores, memory, scroll, cx)
                        })
                    };
                    for (mode, name) in [
                        (appearance::AppearanceMode::Dark, "dark"),
                        (appearance::AppearanceMode::Light, "light"),
                    ] {
                        cx.update(|cx| appearance::set_mode(mode, cx));
                        view(cx, false, false, 0.0)?;
                        pause(cx, 700).await;
                        capture(window.into(), cx, &output, &format!("monitor-{name}-1-top"))?;
                        view(cx, true, false, 0.0)?;
                        pause(cx, 500).await;
                        capture(
                            window.into(),
                            cx,
                            &output,
                            &format!("monitor-{name}-2-cores"),
                        )?;
                        view(cx, true, false, 820.0)?;
                        pause(cx, 500).await;
                        capture(
                            window.into(),
                            cx,
                            &output,
                            &format!("monitor-{name}-3-network-disk"),
                        )?;
                        view(cx, true, false, 4000.0)?;
                        pause(cx, 500).await;
                        capture(
                            window.into(),
                            cx,
                            &output,
                            &format!("monitor-{name}-4-processes-cpu"),
                        )?;
                        view(cx, true, true, 4000.0)?;
                        pause(cx, 500).await;
                        capture(
                            window.into(),
                            cx,
                            &output,
                            &format!("monitor-{name}-5-processes-memory"),
                        )?;
                    }
                    Ok(())
                }
                .await;
                if let Err(error) = run {
                    *result.lock().unwrap() = Some(error.to_string());
                }
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
    if let Some(error) = failure.lock().unwrap().take() {
        anyhow::bail!(error);
    }
    Ok(())
}
