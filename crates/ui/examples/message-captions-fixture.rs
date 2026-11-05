//! Message caption review fixture: a chat that messaged another, switched
//! provider, then received that chat's reply, in an isolated, offline shell.
//! With `--capture <dir>` it saves the transcript at rest and with the
//! sender caption hovered, then quits; ZERON_PALETTE_LIGHT=1 renders the
//! light theme.
use gpui::{AppContext, Bounds, WindowBounds, WindowOptions, px, size};
use serde_json::json;
use std::path::{Path, PathBuf};
use zeron_ui::*;

const SENDER_ID: &str = "cd27c5c2-6f1e-4d1a-9a52-3b0c8e7d4f10";

async fn pause(cx: &mut gpui::AsyncApp, ms: u64) {
    cx.background_executor()
        .timer(std::time::Duration::from_millis(ms))
        .await;
}

fn capture(window: gpui::AnyWindowHandle, cx: &mut gpui::AsyncApp, dir: &Path, name: &str) {
    window
        .update(cx, |_, w, cx| {
            w.draw(cx).clear();
            match w.render_to_image() {
                Ok(image) => image.save(dir.join(format!("{name}.png"))).unwrap(),
                Err(error) => eprintln!("{name}: capture failed: {error:#}"),
            }
        })
        .ok();
}

fn entries() -> serde_json::Value {
    let at = |minutes: i64| 1_788_900_000_000_i64 + minutes * 60_000;
    let text = |id: &str, role: &str, minutes: i64, text: &str| {
        json!({"id": id, "role": role, "createdAt": at(minutes), "deviceId": "local", "status": "complete",
            "parts": [{"id": "t", "kind": "text", "text": text}]})
    };
    let routed = format!(
        "[Message from Zeron chat zgui prod: Windows platform (WING-PC) (cd27c5c2). Reply to it with the Zeron `send_message` tool, chat {SENDER_ID}.]\n\n\
         Windows lane: the matrices are done on current main (#513, #542, #543). Windows in front and verified uncovered on every run, Zeron minimized, alternating, 0 invalid runs; dist builds, shared DejaVu font."
    );
    json!([
        text("u0", "user", 0, "Profile the render loop on Linux and Windows and tell me where the frame time goes."),
        // The agent messages the Windows chat (by title): the call reads as
        // the message it sent, not a tool.
        {"id": "a0", "role": "assistant", "createdAt": at(1), "deviceId": "local", "status": "complete",
            "parts": [
                {"id": "t", "kind": "text", "text": "Linux is done: layout is 61% of the frame, paint 22%. I'll ask the Windows chat to run the same matrix."},
                {"id": "k1", "kind": "tool", "isError": false, "resolved": true,
                    "call": {"kind": "mcp", "server": "zeron", "tool": "send_message", "input": {
                        "chat": "zgui prod: Windows platform (WING-PC)",
                        "text": "Run the render-loop matrix on current main (layout, paint, present per frame) and report the split. Linux: layout 61%, paint 22%."}}},
                {"id": "t2", "kind": "text", "text": "It'll report back here."},
            ]},
        {"id": "switch:1", "role": "system", "createdAt": at(2), "deviceId": "local", "status": "complete",
            "parts": [{"id": "switch:1", "kind": "switch", "from": "Claude Code", "to": "Codex",
                "fromHarness": "claude-code", "toHarness": "codex"}]},
        text("u1", "user", 2, "Carry on from here and draft the summary table."),
        text("a1", "assistant", 3, "Here's the Linux half; the Windows column fills in when its results arrive."),
        text("u2", "user", 4, &routed),
        text("a2", "assistant", 5, "Got it — Windows matches Linux within 3%. The table is complete."),
    ])
}

fn main() -> anyhow::Result<()> {
    let runtime = tokio::runtime::Runtime::new()?;
    let _guard = runtime.enter();
    tracing_subscriber::fmt().with_env_filter("warn").init();
    let temp = tempfile::tempdir()?;
    let data = temp.path().to_path_buf();
    let capture_dir = std::env::args()
        .skip_while(|arg| arg != "--capture")
        .nth(1)
        .map(PathBuf::from);
    let entries = entries();
    gpui_platform::application().with_assets(icons::Assets).run(move |cx| {
        gpui_tokio::init(cx);
        gpui_base::init(cx);
        let mut settings = settings::UiSettings::default();
        settings.sidebar_width = 260.0;
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
        let chat = |id: &str, title: &str, harness: &str, model: &str| {
            serde_json::from_value(json!({"id": id, "deviceId": "local", "title": title, "archived": false, "createdAt": "2026-10-25T00:00:00Z",
                "config": {"harness": harness, "model": model, "reasoning": null, "sandbox": "workspace-write"}})).unwrap()
        };
        let state = cx.new(|_| {
            let mut s = state::AppState::new();
            s.connection = zeron_proto::view::ConnectionStatus::Ready;
            s.workspace_scope = Some(zeron_proto::WorkspaceScope::Local);
            s.local_device_id = Some("local".into());
            s.devices = vec![serde_json::from_value(json!({"id":"local","name":"This device","platform":std::env::consts::OS,"lastSeenAt":null})).unwrap()];
            s.chats = vec![
                chat("captions", "Render loop profiling", "codex", "gpt-5.5"),
                chat(SENDER_ID, "zgui prod: Windows platform (WING-PC)", "claude-code", "claude-opus-5-5"),
            ];
            s.selected_chat = Some("captions".into());
            s.auto_selected = true;
            s.chats_synced = true;
            s.spaces_synced = true;
            s.no_project = true;
            s
        });
        let boot = EngineBootConfig { data_dir: data.clone(), ipc_port: 0, edge_url: String::new(), edge_token: None, org_id: None, workos_client_id: None, default_harness: HarnessId::ClaudeCode };
        let window = cx.open_window(WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(gpui::point(px(40.), px(40.)), size(px(1180.), px(820.))))),
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
                s.receive_transcript_frame(zeron_doc::TranscriptFrame::Reset { reset: serde_json::from_value(entries).unwrap() }, cx).unwrap();
                cx.notify();
            });
            let Some(dir) = capture_dir else { return };
            std::fs::create_dir_all(&dir).ok();
            pause(cx, 1500).await;
            capture(window, cx, &dir, "captions");
            // Hover the sender caption: right-aligned, just over its bubble.
            if let Some((x, y)) = std::env::var("ZERON_CAPTION_HOVER").ok().and_then(|at| {
                let (x, y) = at.split_once(',')?;
                Some((x.parse::<f32>().ok()?, y.parse::<f32>().ok()?))
            }) {
                window
                    .update(cx, |_, w, cx| {
                        w.dispatch_event(gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
                            position: gpui::point(px(x), px(y)),
                            pressed_button: None,
                            modifiers: Default::default(),
                        }), cx);
                    })
                    .ok();
                pause(cx, 300).await;
                capture(window, cx, &dir, "captions-hover");
            }
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
    Ok(())
}
