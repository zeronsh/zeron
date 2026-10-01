//! Native General settings screenshots using isolated, neutral fixture data.
use gpui::{AppContext, AsyncApp, Bounds, WindowBounds, WindowOptions, px, size};
use std::{path::PathBuf, sync::Arc, time::Duration};
use zeron_ui::*;

async fn pause(cx: &mut AsyncApp) {
    cx.background_executor()
        .timer(Duration::from_millis(600))
        .await;
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter("error").init();
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
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let ipc_port = listener.local_addr()?.port();
    drop(listener);
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
            theme_library::init(data, cx);
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
                let mut state = state::AppState::new();
                state.fixture_attachment_engine(handle);
                state.connection = zeron_proto::view::ConnectionStatus::Ready;
                state.workspace_scope = Some(zeron_proto::WorkspaceScope::Local);
                state.chats_synced = true;
                state.spaces_synced = true;
                state
            });
            let window = cx
                .open_window(
                    WindowOptions {
                        window_background: theme::Theme::of(cx).window_background_appearance(),
                        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                            gpui::point(px(20.), px(40.)),
                            size(px(1100.), px(800.)),
                        ))),
                        ..Default::default()
                    },
                    |_, cx| cx.new(|cx| shell::Shell::new(state, boot, cx)),
                )
                .unwrap();
            cx.activate(true);
            cx.spawn(async move |cx| {
                let run: anyhow::Result<()> = async {
                    pause(cx).await;
                    for (mode, dropdown, width, name) in [
                        (
                            keep_awake::KeepAwakeMode::WhilePromptRunning,
                            false,
                            1100.,
                            "prompt-running",
                        ),
                        (
                            keep_awake::KeepAwakeMode::WhilePromptRunning,
                            true,
                            1100.,
                            "awake-options",
                        ),
                        (
                            keep_awake::KeepAwakeMode::WhileAppOpen,
                            false,
                            1100.,
                            "app-open",
                        ),
                        (keep_awake::KeepAwakeMode::Off, false, 1100., "off"),
                        (
                            keep_awake::KeepAwakeMode::WhilePromptRunning,
                            false,
                            900.,
                            "narrow",
                        ),
                    ] {
                        window.update(cx, |shell, w, cx| {
                            w.resize(size(px(width), px(800.)));
                            shell.fixture_general_settings(mode, dropdown, cx);
                        })?;
                        pause(cx).await;
                        gpui::AnyWindowHandle::from(window).update(
                            cx,
                            |_, w, cx| -> anyhow::Result<()> {
                                w.draw(cx).clear();
                                w.render_to_image()?
                                    .save(output.join(format!("{name}.png")))?;
                                Ok(())
                            },
                        )??;
                    }
                    Ok(())
                }
                .await;
                if let Err(error) = run {
                    *result.lock().unwrap() = Some(format!("{error:#}"));
                }
                let _ = window.update(cx, |_, w, _| w.remove_window());
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
