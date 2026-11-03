//! Native visual QA for the settings search. Renders the production Shell
//! offscreen (never activated, no engine) on Settings, reaches the field with
//! a real ⌘F, types queries through the field, opens a hit with a real Enter,
//! and captures each state:
//!
//!   ZERON_OPEN_ROUTE=settings/general cargo run -p zeron-ui \
//!     --example settings-search-fixture --features appshots-fixture -- <output-dir> [name]
//!
//! `ZERON_PALETTE_LIGHT` selects the light palette, `ZERON_FIXTURE_FROSTED`
//! the frosted surface, and `ZERON_FIXTURE_SIZE=<w>x<h>` the window size.
use gpui::{
    AppContext, AsyncApp, Bounds, KeyDownEvent, Keystroke, PlatformInput, WindowBounds,
    WindowOptions, point, px, size,
};
use std::{path::PathBuf, time::Duration};
use zeron_ui::*;

async fn pause(cx: &mut AsyncApp, ms: u64) {
    cx.background_executor()
        .timer(Duration::from_millis(ms))
        .await;
}

/// Let `ms` pass a frame at a time. An offscreen window may get no display
/// link, and the reveal moves only as frames are drawn.
async fn animate(window: gpui::AnyWindowHandle, cx: &mut AsyncApp, ms: u64) {
    for _ in 0..ms / 16 {
        let _ = window.update(cx, |_, w, cx| {
            w.refresh();
            w.draw(cx).clear();
        });
        pause(cx, 16).await;
    }
}

fn capture(
    window: gpui::AnyWindowHandle,
    cx: &mut AsyncApp,
    directory: &std::path::Path,
    name: &str,
) -> anyhow::Result<()> {
    let path = directory.join(format!("{name}.png"));
    window.update(cx, |_, w, cx| {
        w.draw(cx).clear();
        w.render_to_image()?.save(&path)?;
        anyhow::Ok(())
    })??;
    Ok(())
}

/// A real key press through the window's dispatch.
fn press(window: gpui::AnyWindowHandle, cx: &mut AsyncApp, keystroke: &str) {
    let keystroke = Keystroke::parse(keystroke).expect("keystroke");
    let _ = window.update(cx, |_, w, cx| {
        w.dispatch_event(
            PlatformInput::KeyDown(KeyDownEvent {
                keystroke,
                is_held: false,
                prefer_character_input: false,
            }),
            cx,
        );
    });
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter("warn").init();
    let mut args = std::env::args().skip(1);
    let output = PathBuf::from(args.next().expect("output directory"));
    let name = args.next().unwrap_or_else(|| "settings-search".into());
    std::fs::create_dir_all(&output)?;
    let (width, height) = std::env::var("ZERON_FIXTURE_SIZE")
        .ok()
        .and_then(|value| {
            let (w, h) = value.split_once('x')?;
            Some((w.parse::<f32>().ok()?, h.parse::<f32>().ok()?))
        })
        .unwrap_or((1100.0, 760.0));
    let runtime = tokio::runtime::Runtime::new()?;
    let _guard = runtime.enter();
    let temp = tempfile::tempdir()?;
    let data = temp.path().to_path_buf();
    gpui_platform::application()
        .with_assets(icons::Assets)
        .run(move |cx| {
            gpui_tokio::init(cx);
            gpui_base::init(cx);
            let mut settings = settings::UiSettings::default();
            settings.sidebar_width = 260.0;
            settings.surface = if std::env::var_os("ZERON_FIXTURE_FROSTED").is_some() {
                zeron_theme::SurfacePreference::Frosted
            } else {
                zeron_theme::SurfacePreference::Opaque
            };
            settings.save(&data).unwrap();
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
            let mode = if std::env::var_os("ZERON_PALETTE_LIGHT").is_some() {
                appearance::AppearanceMode::Light
            } else {
                appearance::AppearanceMode::Dark
            };
            appearance::init(
                mode,
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
                s.connection = zeron_proto::view::ConnectionStatus::Ready;
                s.workspace_scope = Some(zeron_proto::WorkspaceScope::Local);
                s.local_device_id = Some("local".into());
                s.devices = vec![
                    serde_json::from_value(serde_json::json!({
                        "id": "local", "name": "This device",
                        "platform": std::env::consts::OS, "lastSeenAt": null
                    }))
                    .unwrap(),
                ];
                s.auto_selected = true;
                s.chats_synced = true;
                s.spaces_synced = true;
                s
            });
            let boot = EngineBootConfig {
                data_dir: data.clone(),
                ipc_port: 0,
                edge_url: String::new(),
                edge_token: None,
                org_id: None,
                workos_client_id: None,
                default_harness: HarnessId::ClaudeCode,
            };
            // Offscreen and never activated: the user's windows keep focus.
            let window = cx
                .open_window(
                    WindowOptions {
                        window_background: theme::Theme::of(cx).window_background_appearance(),
                        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                            point(px(-12000.), px(-12000.)),
                            size(px(width), px(height)),
                        ))),
                        focus: false,
                        show: true,
                        ..Default::default()
                    },
                    |_, cx| cx.new(|cx| shell::Shell::new(state.clone(), boot, cx)),
                )
                .unwrap();
            state.update(cx, |_, cx| cx.notify());
            cx.spawn(async move |cx| {
                let any: gpui::AnyWindowHandle = window.into();
                let run: anyhow::Result<()> = async {
                    let shot = |cx: &mut AsyncApp, step: &str| {
                        capture(any, cx, &output, &format!("{name}-{step}"))
                    };
                    let query = |cx: &mut AsyncApp, text: &str| {
                        window
                            .update(cx, |shell, _, cx| shell.fixture_settings_search(text, cx))
                            .map_err(anyhow::Error::from)
                    };
                    pause(cx, 1500).await;
                    shot(cx, "1-idle")?;
                    press(
                        any,
                        cx,
                        if cfg!(target_os = "macos") {
                            "cmd-f"
                        } else {
                            "ctrl-f"
                        },
                    );
                    pause(cx, 100).await;
                    shot(cx, "2-focused")?;
                    for (step, text) in [
                        ("3-theme", "theme"),
                        ("4-typo", "notifcations"),
                        ("5-shortcuts", "sidebar"),
                        ("6-empty", "teleport"),
                    ] {
                        query(cx, text)?;
                        pause(cx, 250).await;
                        shot(cx, step)?;
                    }
                    // Far down Appearance: the page glides there, then the row
                    // glows.
                    query(cx, "math size")?;
                    pause(cx, 250).await;
                    shot(cx, "7-math")?;
                    press(any, cx, "enter");
                    animate(any, cx, 250).await;
                    shot(cx, "8-gliding")?;
                    animate(any, cx, 550).await;
                    shot(cx, "9-landed")?;
                    animate(any, cx, 2000).await;
                    shot(cx, "10-settled")?;
                    Ok(())
                }
                .await;
                if let Err(error) = run {
                    eprintln!("settings search fixture failed: {error:#}");
                }
                let _ = window.update(cx, |_, w, _| w.remove_window());
                pause(cx, 100).await;
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
    Ok(())
}
