//! Native UI-scale evidence from an isolated profile; no engine or model calls.
//! Run with ZERON_OPEN_ROUTE=settings/appearance.
use gpui::{AppContext, AsyncApp, Bounds, WindowBounds, WindowOptions, px, size};
use std::{path::PathBuf, time::Duration};
use zeron_ui::*;

async fn pause(cx: &mut AsyncApp) {
    cx.background_executor()
        .timer(Duration::from_millis(300))
        .await;
}

fn press(handle: gpui::AnyWindowHandle, cx: &mut AsyncApp, combo: &str) -> anyhow::Result<()> {
    handle.update(cx, |_, window, cx| {
        window.draw(cx).clear();
        window.present_if_needed();
        let keystroke = gpui::Keystroke::parse(combo).unwrap();
        window.dispatch_event(
            gpui::PlatformInput::KeyDown(gpui::KeyDownEvent {
                keystroke: keystroke.clone(),
                is_held: false,
                prefer_character_input: false,
            }),
            cx,
        );
        window.dispatch_event(
            gpui::PlatformInput::KeyUp(gpui::KeyUpEvent { keystroke }),
            cx,
        );
    })?;
    Ok(())
}

fn capture(
    handle: gpui::AnyWindowHandle,
    cx: &mut AsyncApp,
    output: &std::path::Path,
    name: &str,
    expected: u16,
) -> anyhow::Result<()> {
    handle.update(cx, |_, window, cx| {
        assert_eq!(settings::current(cx).ui_scale_percent, expected);
        assert_eq!(window.ui_scale(), ui_scale::factor(expected));
        window.draw(cx).clear();
        window.present_if_needed();
        let path = output.join(format!("{name}.png"));
        #[cfg(target_os = "linux")]
        {
            anyhow::ensure!(
                std::env::var_os("WAYLAND_DISPLAY").is_none(),
                "Linux capture needs an isolated X11 display with WAYLAND_DISPLAY unset"
            );
            let status = std::process::Command::new("import")
                .args(["-silent", "-window", "UI scale fixture"])
                .arg(path)
                .status()?;
            anyhow::ensure!(status.success(), "native X11 window capture failed");
        }
        #[cfg(not(target_os = "linux"))]
        window.render_to_image()?.save(path)?;
        Ok(())
    })?
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter("warn").init();
    let output = PathBuf::from(std::env::args().nth(1).expect("output directory"));
    std::fs::create_dir_all(&output)?;
    let temp = tempfile::tempdir()?;
    let data = temp.path().to_path_buf();
    let runtime = tokio::runtime::Runtime::new()?;
    let _guard = runtime.enter();
    gpui_platform::application()
        .with_assets(icons::Assets)
        .run(move |cx| {
            gpui_tokio::init(cx);
            gpui_base::init(cx);
            let mut saved = settings::UiSettings::default();
            saved.reduce_motion = motion::ReduceMotion::On;
            saved.appearance = appearance::AppearanceMode::Dark;
            settings::init(saved.clone(), data.clone(), cx);
            let fonts = typography::register_fonts(cx);
            typography::init(
                saved.ui_font_family.clone(),
                saved.ui_font_size,
                saved.terminal_font_family.clone(),
                saved.terminal_font_size,
                saved.code_font_family.clone(),
                saved.code_font_size,
                fonts,
                cx,
            );
            theme_library::init(data.clone(), cx);
            appearance::init(
                saved.appearance,
                saved.theme_selection,
                saved.accent,
                saved.surface,
                cx,
            );
            history::init(
                saved.git_history_columns,
                saved.git_history_column_widths,
                saved.git_history_column_order,
                saved.git_history_author_display,
                cx,
            );
            motion::init(
                saved.reduce_motion,
                saved.pause_animations_in_background,
                cx,
            );
            composer::init(cx, saved.composer_send_behavior);
            terminal::panel::init(cx);
            app_menus::init(cx);
            let state = cx.new(|_| {
                let mut state = state::AppState::new();
                state.connection = zeron_proto::view::ConnectionStatus::Ready;
                state.workspace_scope = Some(zeron_proto::WorkspaceScope::Local);
                state.local_device_id = Some("fixture".into());
                state.auto_selected = true;
                state.chats_synced = true;
                state.spaces_synced = true;
                state
            });
            let boot = EngineBootConfig {
                data_dir: data.clone(),
                ipc_port: 0,
                edge_url: String::new(),
                edge_token: None,
                org_id: None,
                workos_client_id: None,
                default_harness: HarnessId::Mock,
            };
            let handle = cx
                .open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                            gpui::point(px(20.), px(20.)),
                            size(px(1440.), px(1080.)),
                        ))),
                        window_min_size: Some(size(px(900.), px(600.))),
                        window_decorations: cfg!(target_os = "linux")
                            .then_some(gpui::WindowDecorations::Client),
                        app_owns_titlebar_drag: true,
                        window_background: theme::Theme::of(cx).window_background_appearance(),
                        app_id: Some("zeron-ui-scale-fixture".into()),
                        titlebar: Some(gpui::TitlebarOptions {
                            title: Some("UI scale fixture".into()),
                            appears_transparent: true,
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                    |window, cx| {
                        window.set_rem_size(px(typography::font_size(cx).pixels()));
                        cx.new(|cx| shell::Shell::new(state, boot, cx))
                    },
                )
                .unwrap();
            cx.activate(true);
            cx.spawn(async move |cx| {
                let result: anyhow::Result<()> = async {
                    pause(cx).await;
                    capture(handle.into(), cx, &output, "appearance-100", 100)?;
                    for _ in 0..5 {
                        press(handle.into(), cx, "ctrl-shift-+")?;
                    }
                    pause(cx).await;
                    capture(handle.into(), cx, &output, "appearance-150", 150)?;
                    assert_eq!(settings::UiSettings::load(&data).ui_scale_percent, 150);
                    // Native bounds updates must preserve the selected zoom.
                    handle.update(cx, |_, window, cx| window.bounds_changed(cx))?;
                    capture(
                        handle.into(),
                        cx,
                        &output,
                        "appearance-150-after-resize",
                        150,
                    )?;
                    press(handle.into(), cx, "ctrl-shift-0")?;
                    pause(cx).await;
                    capture(handle.into(), cx, &output, "appearance-reset-100", 100)?;
                    handle.update(cx, |_, window, cx| {
                        appearance::set_mode(appearance::AppearanceMode::Light, cx);
                        ui_scale::set(120, window, cx);
                    })?;
                    pause(cx).await;
                    capture(handle.into(), cx, &output, "appearance-light-120", 120)?;
                    Ok(())
                }
                .await;
                result.expect("UI scale fixture failed");
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
    Ok(())
}
