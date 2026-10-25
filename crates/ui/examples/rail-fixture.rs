//! Prompt-rail review fixture: a long conversation (ZERON_RAIL_PROMPTS, 60 by
//! default) in an isolated, offline shell. With `--capture <dir>` it films the
//! rail pinned to the bottom, hovered, wheel-scrolled and after a jump, then
//! quits; ZERON_PALETTE_LIGHT=1 renders the light theme.
use gpui::{AppContext, Bounds, WindowBounds, WindowOptions, px, size};
use serde_json::json;
use std::path::{Path, PathBuf};
use zeron_ui::*;

const PROMPTS: [&str; 12] = [
    "Why does the order sheet double-count croissants on Saturdays?",
    "Add a weekly export of the flour inventory to CSV",
    "Can the oven schedule warn when two batches overlap?",
    "Rename the `batch_size` setting to `tray_count` everywhere",
    "Write tests for the sourdough starter feeding reminders",
    "How do I tell the bakery the oven grant was denied?",
    "Make the till receipt show the allergen icons",
    "The delivery map is blank on Safari — what changed?",
    "Split the pastry menu into breakfast and afternoon",
    "Why is the nightly backup taking forty minutes now?",
    "Draft release notes for the loyalty card update",
    "Move the supplier invoices into their own table",
];

const REPLIES: [&str; 4] = [
    "Found it: the Saturday rota adds the market stall's preorders twice. I merged the two queries and added a regression test with last week's numbers.",
    "Done. The export runs every Monday at 6am and lands in `exports/`. Each row carries the bin, the weight on hand and the reorder threshold, so purchasing can filter it in a spreadsheet.\n\nI also added a manual **Export now** button on the inventory page for the odd mid-week check.",
    "Yes. The scheduler now compares each batch's window against the oven's booked slots and flags any overlap in amber, with the clashing batch named in the tooltip.",
    "Here is what I changed:\n\n- the settings schema and its migration\n- every read site (14 files)\n- the admin form label\n\nThe old key still loads for one release so existing configs keep working.",
];

fn tick_columns(
    image: &image::RgbaImage,
    scale: f32,
    from: f32,
    to: f32,
) -> Option<(f32, Vec<f32>)> {
    let lum = |x: u32, y: u32| {
        let p = image.get_pixel(x, y);
        0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32
    };
    let mut best: Option<(f32, Vec<f32>)> = None;
    let (top, bottom) = (60, image.height().saturating_sub(200));
    let mut x = from;
    while x < to {
        let px_x = (x * scale) as u32;
        if px_x >= image.width() {
            break;
        }
        let mut column: Vec<f32> = (top..bottom).map(|y| lum(px_x, y)).collect();
        let mut sorted = column.clone();
        sorted.sort_by(|a, b| a.total_cmp(b));
        let background = sorted[sorted.len() / 2];
        let mut runs = Vec::new();
        let mut start: Option<usize> = None;
        column.push(background);
        for (ix, value) in column.iter().enumerate() {
            let on = (value - background).abs() > 8.0;
            match (on, start) {
                (true, None) => start = Some(ix),
                (false, Some(s)) => {
                    // A tick is ~2 logical px tall; text strokes run longer.
                    if (ix - s) as f32 <= 3.5 * scale {
                        runs.push((top as f32 + (s + ix) as f32 / 2.0) / scale);
                    }
                    start = None;
                }
                _ => {}
            }
        }
        if runs.len() >= 5 && best.as_ref().is_none_or(|(_, b)| runs.len() > b.len()) {
            best = Some((x, runs));
        }
        x += 1.0;
    }
    best
}

async fn pause(cx: &mut gpui::AsyncApp, ms: u64) {
    cx.background_executor()
        .timer(std::time::Duration::from_millis(ms))
        .await;
}

fn capture(
    window: gpui::AnyWindowHandle,
    cx: &mut gpui::AsyncApp,
    dir: &Path,
    name: &str,
) -> Option<image::RgbaImage> {
    window
        .update(cx, |_, w, cx| {
            w.draw(cx).clear();
            let image = w.render_to_image().ok()?;
            image.save(dir.join(format!("{name}.png"))).ok();
            Some(image)
        })
        .ok()
        .flatten()
}

fn input(window: gpui::AnyWindowHandle, cx: &mut gpui::AsyncApp, event: gpui::PlatformInput) {
    window
        .update(cx, |_, w, cx| {
            w.dispatch_event(event, cx);
        })
        .ok();
}

fn mouse_move(x: f32, y: f32) -> gpui::PlatformInput {
    gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
        position: gpui::point(px(x), px(y)),
        pressed_button: None,
        modifiers: Default::default(),
    })
}

async fn film(
    window: gpui::AnyWindowHandle,
    dir: PathBuf,
    sidebar: f32,
    cx: &mut gpui::AsyncApp,
) -> anyhow::Result<()> {
    std::fs::create_dir_all(&dir)?;
    let scale = window.update(cx, |_, w, _| w.scale_factor())?;
    let rest =
        capture(window, cx, &dir, "rail-rest").ok_or_else(|| anyhow::anyhow!("no capture"))?;
    let (x, ticks) = tick_columns(&rest, scale, sidebar, sidebar + 140.0)
        .ok_or_else(|| anyhow::anyhow!("no rail found"))?;
    let report = format!(
        "rail column x={x} ticks={} first={:?} last={:?}\n",
        ticks.len(),
        ticks.first(),
        ticks.last()
    );
    // Hover a tick in the middle of the window.
    let mid = ticks[ticks.len() / 2];
    input(window, cx, mouse_move(x + 2.0, mid));
    pause(cx, 300).await;
    capture(window, cx, &dir, "rail-hover");
    // Wheel the rail toward the start of the conversation.
    input(window, cx, mouse_move(x + 2.0, mid));
    for _ in 0..6 {
        input(
            window,
            cx,
            gpui::PlatformInput::ScrollWheel(gpui::ScrollWheelEvent {
                position: gpui::point(px(x + 2.0), px(mid)),
                delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.), px(60.))),
                modifiers: Default::default(),
                touch_phase: gpui::TouchPhase::Moved,
            }),
        );
        pause(cx, 40).await;
    }
    input(window, cx, mouse_move(x + 2.0, mid));
    pause(cx, 300).await;
    let scrolled =
        capture(window, cx, &dir, "rail-scrolled").ok_or_else(|| anyhow::anyhow!("no capture"))?;
    // Jump to the topmost tick now in view; the rail follows the jump.
    let (_, ticks) = tick_columns(&scrolled, scale, x - 1.0, x + 1.0)
        .ok_or_else(|| anyhow::anyhow!("rail lost"))?;
    let target = ticks[ticks.len().min(4) - 1];
    input(window, cx, mouse_move(x + 2.0, target));
    pause(cx, 120).await;
    let at = gpui::point(px(x + 2.0), px(target));
    input(
        window,
        cx,
        gpui::PlatformInput::MouseDown(gpui::MouseDownEvent {
            position: at,
            button: gpui::MouseButton::Left,
            click_count: 1,
            ..Default::default()
        }),
    );
    input(
        window,
        cx,
        gpui::PlatformInput::MouseUp(gpui::MouseUpEvent {
            position: at,
            button: gpui::MouseButton::Left,
            click_count: 1,
            ..Default::default()
        }),
    );
    pause(cx, 900).await;
    input(window, cx, mouse_move(x + 400.0, target));
    pause(cx, 300).await;
    capture(window, cx, &dir, "rail-jumped");
    std::fs::write(dir.join("result.txt"), report)?;
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let runtime = tokio::runtime::Runtime::new()?;
    let _guard = runtime.enter();
    tracing_subscriber::fmt().with_env_filter("warn").init();
    let temp = tempfile::tempdir()?;
    let data = temp.path().to_path_buf();
    let prompts: usize = std::env::var("ZERON_RAIL_PROMPTS")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(60);
    let capture_dir = std::env::args()
        .skip_while(|arg| arg != "--capture")
        .nth(1)
        .map(PathBuf::from);
    let mut entries = Vec::new();
    for ix in 0..prompts {
        let at = 1_788_900_000_000_i64 + ix as i64 * 60_000;
        entries.push(
            json!({"id": format!("u{ix}"), "role": "user", "createdAt": at, "deviceId": "local",
            "parts": [{"id": "t", "kind": "text", "text": PROMPTS[ix % PROMPTS.len()]}]}),
        );
        entries.push(json!({"id": format!("a{ix}"), "role": "assistant", "createdAt": at + 1000, "deviceId": "local", "status": "complete",
            "parts": [{"id": "t", "kind": "text", "text": REPLIES[ix % REPLIES.len()]}]}));
    }
    let sidebar = 280.0;
    gpui_platform::application().with_assets(icons::Assets).run(move |cx| {
        gpui_tokio::init(cx);
        gpui_base::init(cx);
        let mut settings = settings::UiSettings::default();
        settings.sidebar_width = sidebar;
        settings.save(&data).unwrap();
        settings::init(settings.clone(), data.clone(), cx);
        let fonts = typography::register_fonts(cx);
        typography::init(settings.ui_font_family.clone(), settings.ui_font_size, settings.terminal_font_family.clone(), settings.terminal_font_size, settings.code_font_family.clone(), settings.code_font_size, fonts, cx);
        theme_library::init(data.clone(), cx);
        let mode = if std::env::var_os("ZERON_PALETTE_LIGHT").is_some() { appearance::AppearanceMode::Light } else { appearance::AppearanceMode::Dark };
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
            s.devices = vec![serde_json::from_value(json!({"id":"local","name":"This device","platform":std::env::consts::OS,"lastSeenAt":null})).unwrap()];
            s.chats = vec![serde_json::from_value(json!({"id":"rail","deviceId":"local","title":"Bakery ordering app","archived":false,"createdAt":"2026-10-25T00:00:00Z","config":{"harness":"claude-code","model":"claude-sonnet-4-6","reasoning":null,"sandbox":"workspace-write"}})).unwrap()];
            s.selected_chat = Some("rail".into());
            s.auto_selected = true;
            s.chats_synced = true;
            s.spaces_synced = true;
            s.no_project = true;
            s
        });
        let boot = EngineBootConfig { data_dir: data.clone(), ipc_port: 0, edge_url: String::new(), edge_token: None, org_id: None, workos_client_id: None, default_harness: HarnessId::ClaudeCode };
        let window = cx.open_window(WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(gpui::point(px(40.), px(40.)), size(px(1280.), px(860.))))),
            titlebar: Some(gpui::TitlebarOptions { title: None, appears_transparent: true, traffic_light_position: Some(gpui::point(px(14.), px(14.))) }),
            app_owns_titlebar_drag: true,
            window_background: theme::Theme::of(cx).window_background_appearance(),
            ..Default::default()
        }, |_, cx| cx.new(|cx| shell::Shell::new(state.clone(), boot, cx))).unwrap();
        appearance::reapply_window_background(cx);
        cx.on_window_closed(|cx, _| cx.quit()).detach();
        state.update(cx, |_, cx| cx.notify());
        cx.activate(true);
        let window: gpui::AnyWindowHandle = window.into();
        cx.spawn(async move |cx| {
            pause(cx, 600).await;
            state.update(cx, |s, cx| {
                s.receive_transcript_frame(zeron_doc::TranscriptFrame::Reset { reset: serde_json::from_value(serde_json::Value::Array(entries)).unwrap() }, cx).unwrap();
                cx.notify();
            });
            let Some(dir) = capture_dir else { return };
            pause(cx, 1500).await;
            if let Err(error) = film(window, dir.clone(), sidebar, cx).await {
                eprintln!("capture failed: {error:#}");
                std::fs::write(dir.join("result.txt"), format!("FAILED: {error:#}\n")).ok();
            }
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
    Ok(())
}
