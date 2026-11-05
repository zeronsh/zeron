//! Floating session card reference stills: the production Shell in a window
//! too narrow to dock the session card beside the transcript, so the
//! titlebar's card toggle floats it over the conversation. Offline (no
//! engine): the card shows the chat's device, project and branch.
//!
//!     cargo build -p zeron-ui --example session-card-fixture --features appshots-fixture
//!     target/debug/examples/session-card-fixture <out-dir> <name>
//!
//! Writes `<name>.png` and `<name>.txt` (theme facts). ZERON_PALETTE_LIGHT=1
//! renders the light theme; ZERON_CARD_OPAQUE=1 forces the opaque surface
//! (the default is the shipping "theme default" surface). ZERON_CARD_TOGGLE=x,y
//! overrides the toggle's computed position (logical px).
use anyhow::Context as _;
use gpui::{AppContext, Bounds, WindowBounds, WindowOptions, px, size};
use serde_json::json;
use std::path::PathBuf;
use zeron_ui::*;

/// Main column 1100 − 256 = 844 < 1092, the least that docks the card
/// beside the default 736px reading column.
const WINDOW: (f32, f32) = (1100.0, 760.0);
const SIDEBAR: f32 = 256.0;

const REPLY: &str = "The explorer now lists directories first, then files by name, and \
the editor tab keeps its own path crumbs. I also moved the workspace resolver behind \
`resolve`, which refuses anything outside the checkout root, and added tests for \
symlinks that point out of the tree.\n\nNext I can wire the same listing into the \
command palette's file search, so both stay in step.";

async fn pause(cx: &mut gpui::AsyncApp, ms: u64) {
    cx.background_executor()
        .timer(std::time::Duration::from_millis(ms))
        .await;
}

fn input(window: gpui::AnyWindowHandle, cx: &mut gpui::AsyncApp, event: gpui::PlatformInput) {
    window
        .update(cx, |_, w, cx| w.dispatch_event(event, cx))
        .ok();
}

fn mouse_move(x: f32, y: f32) -> gpui::PlatformInput {
    gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
        position: gpui::point(px(x), px(y)),
        pressed_button: None,
        modifiers: Default::default(),
    })
}

fn rgba(color: gpui::Hsla) -> String {
    let c = gpui::Rgba::from(color);
    format!(
        "#{:02x}{:02x}{:02x} alpha {:.3}",
        (c.r * 255.0).round() as u8,
        (c.g * 255.0).round() as u8,
        (c.b * 255.0).round() as u8,
        c.a
    )
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let output = PathBuf::from(args.next().context("output directory")?);
    let name = args.next().unwrap_or_else(|| "card".into());
    std::fs::create_dir_all(&output)?;
    let runtime = tokio::runtime::Runtime::new()?;
    let _guard = runtime.enter();
    tracing_subscriber::fmt().with_env_filter("warn").init();
    let temp = tempfile::tempdir()?;
    let data = temp.path().to_path_buf();
    let light = std::env::var_os("ZERON_PALETTE_LIGHT").is_some();
    let opaque = std::env::var_os("ZERON_CARD_OPAQUE").is_some();
    let toggle = std::env::var("ZERON_CARD_TOGGLE")
        .ok()
        .and_then(|v| {
            let (x, y) = v.split_once(',')?;
            Some((x.trim().parse::<f32>().ok()?, y.trim().parse::<f32>().ok()?))
        })
        // Pane toggle 28 at the 6px right pad, files toggle 4 left of it,
        // then the 8px row gap and the 28px card toggle; 4px top pad.
        .unwrap_or((WINDOW.0 - 6.0 - 28.0 - 4.0 - 28.0 - 8.0 - 14.0, 4.0 + 17.0));
    let path = "/Users/ana/code/zeron";
    let mut entries = Vec::new();
    for (ix, prompt) in [
        "Sort the explorer so folders come first",
        "Keep the path crumbs on each editor tab",
    ]
    .iter()
    .enumerate()
    {
        let at = 1_788_900_000_000_i64 + ix as i64 * 60_000;
        entries.push(json!({"id": format!("u{ix}"), "role": "user", "createdAt": at, "deviceId": "local",
            "parts": [{"id": "t", "kind": "text", "text": prompt}]}));
        entries.push(json!({"id": format!("a{ix}"), "role": "assistant", "createdAt": at + 1000, "deviceId": "local", "status": "complete",
            "parts": [{"id": "t", "kind": "text", "text": REPLY}]}));
    }
    gpui_platform::application().with_assets(icons::Assets).run(move |cx| {
        gpui_tokio::init(cx);
        gpui_base::init(cx);
        let mut settings = settings::UiSettings::default();
        settings.sidebar_width = SIDEBAR;
        settings.github_star_banner_dismissed = true;
        if opaque {
            settings.surface = zeron_theme::SurfacePreference::Opaque;
        }
        settings.save(&data).unwrap();
        settings::init(settings.clone(), data.clone(), cx);
        let fonts = typography::register_fonts(cx);
        typography::init(settings.ui_font_family.clone(), settings.ui_font_size, settings.terminal_font_family.clone(), settings.terminal_font_size, settings.code_font_family.clone(), settings.code_font_size, fonts, cx);
        theme_library::init(data.clone(), cx);
        let mode = if light { appearance::AppearanceMode::Light } else { appearance::AppearanceMode::Dark };
        appearance::init(mode, settings.theme_selection, settings.accent, settings.surface, cx);
        history::init(settings.git_history_columns, settings.git_history_column_widths, settings.git_history_column_order, settings.git_history_author_display, cx);
        composer::init(cx, settings.composer_send_behavior);
        terminal::panel::init(cx);
        app_menus::init(cx);
        let state = cx.new(|_| {
            let mut s = state::AppState::new();
            s.connection = zeron_proto::view::ConnectionStatus::Ready;
            s.workspace_scope = Some(zeron_proto::WorkspaceScope::Local);
            s.local_device_id = Some("local".into());
            s.devices = vec![serde_json::from_value(json!({"id":"local","name":"Studio Mac","platform":"macos","lastSeenAt":null})).unwrap()];
            s.spaces = vec![serde_json::from_value(json!({"id":"space","deviceId":"local","path":path,"name":"zeron","gitDetected":true,"createdAt":"2026-10-25T00:00:00Z"})).unwrap()];
            s.chats = vec![serde_json::from_value(json!({"id":"card","deviceId":"local","title":"Explorer ordering","archived":false,"cwd":path,"branch":"main","spaceId":"space","createdAt":"2026-10-25T00:00:00Z","config":{"harness":"claude-code","model":"claude-sonnet-4-6","reasoning":null,"sandbox":"workspace-write"}})).unwrap()];
            s.selected_space = Some("space".into());
            s.selected_chat = Some("card".into());
            s.auto_selected = true;
            s.chats_synced = true;
            s.spaces_synced = true;
            s
        });
        let boot = EngineBootConfig { data_dir: data.clone(), ipc_port: 0, edge_url: String::new(), edge_token: None, org_id: None, workos_client_id: None, default_harness: HarnessId::ClaudeCode };
        let window = cx.open_window(WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(gpui::point(px(40.), px(40.)), size(px(WINDOW.0), px(WINDOW.1))))),
            titlebar: Some(gpui::TitlebarOptions { title: None, appears_transparent: true, traffic_light_position: Some(gpui::point(px(14.), px(14.))) }),
            app_owns_titlebar_drag: true,
            window_background: theme::Theme::of(cx).window_background_appearance(),
            ..Default::default()
        }, |_, cx| cx.new(|cx| shell::Shell::new(state.clone(), boot, cx))).unwrap();
        appearance::reapply_window_background(cx);
        state.update(cx, |_, cx| cx.notify());
        cx.activate(true);
        let theme = theme::Theme::of(cx).clone();
        let popup = theme.for_popup();
        let facts = format!(
            "window {}x{} logical, sidebar {SIDEBAR}, toggle click {toggle:?}\nsurface preference {:?}, is_frost {}, is_glass {}\npopup border {}\npopup surface_bg {}\n",
            WINDOW.0, WINDOW.1, settings.surface, theme.is_frost(), theme.is_glass(),
            rgba(popup.border), rgba(popover::surface_bg(&popup)),
        );
        let window: gpui::AnyWindowHandle = window.into();
        cx.spawn(async move |cx| {
            pause(cx, 600).await;
            state.update(cx, |s, cx| {
                s.receive_transcript_frame(zeron_doc::TranscriptFrame::Reset { reset: serde_json::from_value(serde_json::Value::Array(entries)).unwrap() }, cx).ok();
                cx.notify();
            });
            pause(cx, 1500).await;
            let at = gpui::point(px(toggle.0), px(toggle.1));
            input(window, cx, mouse_move(toggle.0, toggle.1));
            pause(cx, 80).await;
            input(window, cx, gpui::PlatformInput::MouseDown(gpui::MouseDownEvent { position: at, button: gpui::MouseButton::Left, click_count: 1, ..Default::default() }));
            input(window, cx, gpui::PlatformInput::MouseUp(gpui::MouseUpEvent { position: at, button: gpui::MouseButton::Left, click_count: 1, ..Default::default() }));
            pause(cx, 150).await;
            // Park the pointer over the composer so no hover shows.
            input(window, cx, mouse_move(500.0, WINDOW.1 - 30.0));
            pause(cx, 900).await;
            let scale = window.update(cx, |_, w, _| w.scale_factor()).unwrap_or(0.0);
            window.update(cx, |_, w, cx| {
                w.draw(cx).clear();
                if let Ok(image) = w.render_to_image() {
                    image.save(output.join(format!("{name}.png"))).ok();
                }
            }).ok();
            std::fs::write(output.join(format!("{name}.txt")), format!("{facts}scale {scale}\n")).ok();
            let _ = window.update(cx, |_, w, _| w.remove_window());
            pause(cx, 100).await;
            std::process::exit(0);
        })
        .detach();
    });
    Ok(())
}
