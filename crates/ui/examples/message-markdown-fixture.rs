//! Message Markdown review fixture: a chat whose prompts are written in
//! Markdown, one of them sent by another chat through Zeron's
//! `send_message`, in an isolated, offline shell. It shows the user bubbles
//! and the prompt rail; hover a rail tick for its preview card.
//! ZERON_PALETTE_LIGHT=1 renders the light theme.
use gpui::{AppContext, Bounds, WindowBounds, WindowOptions, px, size};
use serde_json::json;
use zeron_ui::*;

const SENDER_ID: &str = "c563133c-2825-4056-8d1e-4b2a18c210ae";

async fn pause(cx: &mut gpui::AsyncApp, ms: u64) {
    cx.background_executor()
        .timer(std::time::Duration::from_millis(ms))
        .await;
}

fn entries() -> serde_json::Value {
    let at = |minutes: i64| 1_788_900_000_000_i64 + minutes * 60_000;
    let text = |id: &str, role: &str, minutes: i64, text: &str| {
        json!({"id": id, "role": role, "createdAt": at(minutes), "deviceId": "local", "status": "complete",
            "parts": [{"id": "t", "kind": "text", "text": text}]})
    };
    let routed = format!(
        "[Message from Zeron chat Harden Orbit For Production (c563133c). Reply to it with the Zeron `send_message` tool, chat {SENDER_ID}.]\n\n\
         You own the **heavyforge** soak. Start with `slow-DO`, then:\n\
         1. two replicas on `10069f6`\n\
         2. the foundation chat's queue"
    );
    json!([
        text("u0", "user", 0, "Profile the render loop on **Linux** and *Windows*.\nKeep `layout` and `paint` separate, and skip the ~~old~~ harness."),
        text("a0", "assistant", 1, "Linux is done: **layout** is 61% of the frame, `paint` 22%. I've asked the soak chat to take the queue."),
        text("u1", "user", 2, "Draft the summary:\n- one row per platform\n- frame time in `ms`"),
        text("a1", "assistant", 3, "Here's the **Linux** half; the Windows column fills in when its results arrive."),
        text("u2", "user", 4, &routed),
        text("a2", "assistant", 5, "I'll check the **managed queue**, completed reports and free disk first, then resume without repeating the finished `10069f6` pair. The remaining order is *slow-DO*, two replicas, then the foundation chat's queue."),
        text("u3", "user", 6, "Then run:\n```sh\ncargo test -p zeron-ui\n```"),
        text("a3", "assistant", 7, "Ran the **zeron-ui** tests:\n\n```\ntest result: ok. 1931 passed\n```\n\n> One ignored, as on main.\n\nAll green, so the summary is ready."),
    ])
}

fn main() -> anyhow::Result<()> {
    let runtime = tokio::runtime::Runtime::new()?;
    let _guard = runtime.enter();
    tracing_subscriber::fmt().with_env_filter("warn").init();
    let temp = tempfile::tempdir()?;
    let data = temp.path().to_path_buf();
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
                chat("markdown", "Render loop profiling", "claude-code", "claude-opus-5-5"),
                chat(SENDER_ID, "Harden Orbit For Production", "codex", "gpt-5.5"),
            ];
            s.selected_chat = Some("markdown".into());
            s.auto_selected = true;
            s.chats_synced = true;
            s.spaces_synced = true;
            s.no_project = true;
            s
        });
        let boot = EngineBootConfig { data_dir: data.clone(), ipc_port: 0, edge_url: String::new(), edge_token: None, org_id: None, workos_client_id: None, default_harness: HarnessId::ClaudeCode };
        let window = cx.open_window(WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(gpui::point(px(0.), px(0.)), size(px(1180.), px(900.))))),
            titlebar: Some(gpui::TitlebarOptions { title: None, appears_transparent: true, traffic_light_position: Some(gpui::point(px(14.), px(14.))) }),
            app_owns_titlebar_drag: true,
            window_background: theme::Theme::of(cx).window_background_appearance(),
            ..Default::default()
        }, |_, cx| cx.new(|cx| shell::Shell::new(state.clone(), boot, cx))).unwrap();
        appearance::reapply_window_background(cx);
        cx.on_window_closed(|cx, _| cx.quit()).detach();
        state.update(cx, |_, cx| cx.notify());
        cx.activate(true);
        let _ = window;
        cx.spawn(async move |cx| {
            pause(cx, 600).await;
            state.update(cx, |s, cx| {
                s.receive_transcript_frame(zeron_doc::TranscriptFrame::Reset { reset: serde_json::from_value(entries).unwrap() }, cx).unwrap();
                cx.notify();
            });
        })
        .detach();
    });
    Ok(())
}
