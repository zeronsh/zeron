//! Isolated "What's new" review fixture: the real launch path against a local
//! stand-in for the GitHub releases API.
//!
//! `ZERON_CHANGELOG_FIXTURE_JSON` — releases JSON to serve (the
//! `GET /repos/zeronsh/zeron/releases` shape); `ZERON_CHANGELOG_SEEN` — the
//! last version "already seen" (default `0.2.97`); `ZERON_PALETTE_LIGHT` —
//! light appearance; `ZERON_CHANGELOG_SCROLL` — scroll the body by that many px
//! four seconds after launch.
use std::io::{Read, Write};

use gpui::{AppContext, Bounds, WindowBounds, WindowOptions, px, size};
use zeron_ui::*;

/// Serve `body` as JSON to every request on a loopback port.
fn serve(body: String) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind fixture server");
    let url = format!("http://{}/releases", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut stream = stream;
            let mut request = [0u8; 2048];
            let _ = stream.read(&mut request);
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });
    url
}

fn main() -> anyhow::Result<()> {
    let runtime = tokio::runtime::Runtime::new()?;
    let _guard = runtime.enter();
    tracing_subscriber::fmt().with_env_filter("warn").init();
    let json = std::fs::read_to_string(
        std::env::var("ZERON_CHANGELOG_FIXTURE_JSON")
            .expect("set ZERON_CHANGELOG_FIXTURE_JSON to a releases JSON file"),
    )?;
    // SAFETY: single-threaded at this point; read by `Changelog::init`.
    unsafe { std::env::set_var("ZERON_CHANGELOG_API", serve(json)) };
    let temp = tempfile::tempdir()?;
    let data = temp.path().to_path_buf();
    gpui_platform::application()
        .with_assets(icons::Assets)
        .run(move |cx| {
            gpui_tokio::init(cx);
            gpui_base::init(cx);
            let mut settings = settings::UiSettings::default();
            settings.surface = zeron_theme::SurfacePreference::Frosted;
            settings.last_seen_changelog_version = Some(
                std::env::var("ZERON_CHANGELOG_SEEN").unwrap_or_else(|_| "0.2.97".into()),
            );
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
            appearance::init(mode, settings.theme_selection, settings.accent, settings.surface, cx);
            history::init(
                settings.git_history_columns,
                settings.git_history_column_widths,
                settings.git_history_column_order,
                settings.git_history_author_display,
                cx,
            );
            motion::init(settings.reduce_motion, settings.pause_animations_in_background, cx);
            composer::init(cx, settings.composer_send_behavior);
            terminal::panel::init(cx);
            app_menus::init(cx);
            changelog::Changelog::init(true, cx);
            if let Some(offset) = std::env::var("ZERON_CHANGELOG_SCROLL")
                .ok()
                .and_then(|value| value.parse::<f32>().ok())
            {
                cx.spawn(async move |cx| {
                    cx.background_executor()
                        .timer(std::time::Duration::from_secs(4))
                        .await;
                    cx.update(|cx| changelog::scroll_to(offset, cx));
                })
                .detach();
            }

            let project_path = data.join("fieldnotes");
            std::fs::create_dir_all(&project_path).unwrap();
            let state = cx.new(|_| {
                let mut s = state::AppState::new();
                s.connection = zeron_proto::view::ConnectionStatus::Ready;
                s.workspace_scope = Some(zeron_proto::WorkspaceScope::Local);
                s.local_device_id = Some("local".into());
                s.devices = vec![
                    serde_json::from_value(serde_json::json!({"id":"local","name":"This device","platform":std::env::consts::OS,"lastSeenAt":null})).unwrap(),
                ];
                s.selected_chat = Some("chat-0".into());
                s.selected_space = Some("project".into());
                s.auto_selected = true;
                s.chats_synced = true;
                s.spaces_synced = true;
                s.spaces = vec![
                    serde_json::from_value(serde_json::json!({"id":"project","deviceId":"local","path":project_path,"createdAt":"2026-09-08T00:00:00Z"})).unwrap(),
                ];
                for (ix, title) in [
                    "Build the Fieldnotes workspace",
                    "Polish the command palette",
                    "Fix authentication redirects",
                    "Review pull request comments",
                    "Improve keyboard navigation",
                ]
                .iter()
                .enumerate()
                {
                    s.chats.push(
                        serde_json::from_value(serde_json::json!({"id":format!("chat-{ix}"),"deviceId":"local","spaceId":"project","title":title,"archived":false,"createdAt":chrono::Utc::now() - chrono::Duration::minutes((ix as i64 + 1) * 17),"config":{"harness":"claude-code","model":"claude-sonnet-4-6","reasoning":null,"sandbox":"workspace-write"}})).unwrap(),
                    );
                }
                s
            });
            let boot = EngineBootConfig {
                data_dir: data,
                ipc_port: 0,
                edge_url: String::new(),
                edge_token: None,
                org_id: None,
                workos_client_id: None,
                default_harness: HarnessId::ClaudeCode,
            };
            let _window = cx
                .open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                            gpui::point(px(12.), px(30.)),
                            size(px(1100.), px(800.)),
                        ))),
                        titlebar: Some(gpui::TitlebarOptions {
                            title: None,
                            appears_transparent: true,
                            traffic_light_position: Some(gpui::point(px(14.), px(14.))),
                        }),
                        app_owns_titlebar_drag: true,
                        ..Default::default()
                    },
                    |_, cx| cx.new(|cx| shell::Shell::new(state.clone(), boot, cx)),
                )
                .unwrap();
            state.update(cx, |_, cx| cx.notify());
            cx.activate(true);
        });
    Ok(())
}
