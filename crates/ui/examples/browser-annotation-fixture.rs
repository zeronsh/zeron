//! Interactive embedded-browser + annotation session with isolated data. A
//! local mock web app is served on 127.0.0.1; the chat's browser tab opens
//! it. Close the window to exit. No agent runs.
use gpui::{AppContext, Bounds, WindowBounds, WindowOptions, px, size};
use serde_json::json;
use std::{
    io::{BufRead, BufReader, Write},
    net::TcpListener,
    sync::Arc,
};
use zeron_ui::*;

const SITE: &str = include_str!("browser-annotation-site.html");

fn port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// A minimal HTTP/1.1 responder: every path serves the mock app, except
/// `/next` (a second page for history) and `/favicon.svg`.
fn serve_site() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || {
                let mut reader = BufReader::new(&stream);
                let mut request = String::new();
                if reader.read_line(&mut request).is_err() {
                    return;
                }
                let mut line = String::new();
                while reader.read_line(&mut line).is_ok_and(|n| n > 2) {
                    line.clear();
                }
                let path = request.split_whitespace().nth(1).unwrap_or("/");
                let (kind, body) = match path {
                    "/favicon.svg" => (
                        "image/svg+xml",
                        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32"><rect width="32" height="32" rx="8" fill="#7aa2ff"/><path d="M9 22 16 9l7 13z" fill="#0b0d12"/></svg>"##.to_string(),
                    ),
                    "/next" => (
                        "text/html; charset=utf-8",
                        SITE.replace("<!--PAGE-->", "Changelog"),
                    ),
                    _ => ("text/html; charset=utf-8", SITE.replace("<!--PAGE-->", "Overview")),
                };
                let mut stream = &stream;
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            });
        }
    });
    port
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(std::env::var("ZERON_FIXTURE_LOG").unwrap_or_else(|_| "warn".into()))
        .init();
    let site = format!("http://127.0.0.1:{}/", serve_site());
    let temp = tempfile::tempdir()?;
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace)?;
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
        "annotations",
        None,
        Some(&core.device_id),
        None,
        Some(workspace.to_string_lossy().into_owned()),
    )?;
    core.workspace
        .rename_chat("annotations", "Polish the pricing page")?;
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
    // A sent message with two annotations, and the exact text a harness
    // receives for it (the real expansion, shown back as the reply).
    let team = zeron_proto::annotation::BrowserAnnotation {
        index: 1,
        url: site.clone(),
        title: "Northwind · Overview".into(),
        element: "article#plan-team.plan-card.featured".into(),
        selector: "#plan-team".into(),
        path: vec!["body".into(), "main".into(), "div.pricing-grid".into()],
        role: "article".into(),
        name: "Team Popular $24 / seat / month".into(),
        text: "Team Popular $24 / seat / month Unlimited projects Incident timelines SSO and audit log Choose Team".into(),
        html: "<article class=\"plan-card featured\" id=\"plan-team\">
  <h3>Team <span class=\"badge\">Popular</span></h3>
  <div class=\"price\">$24 <small>/ seat / month</small></div>
  <ul>…</ul>
  <button class=\"btn btn-primary\" data-testid=\"choose-team\">Choose Team</button>
</article>".into(),
        rect: [196, 669, 129, 246],
        viewport: [520, 808],
        styles: vec![
            ["display".into(), "block".into()],
            ["padding".into(), "22px".into()],
            ["border-radius".into(), "14px".into()],
            ["background-color".into(), "rgb(18, 21, 29)".into()],
        ],
    };
    let toggle = zeron_proto::annotation::BrowserAnnotation {
        index: 2,
        element: "div.toggle".into(),
        selector: "main > div.toggle".into(),
        role: "group".into(),
        name: "Billing period".into(),
        text: "Monthly Yearly −20%".into(),
        html: "<div class=\"toggle\" role=\"group\" aria-label=\"Billing period\">
  <button aria-pressed=\"true\">Monthly</button>
  <button aria-pressed=\"false\">Yearly <span class=\"badge\">−20%</span></button>
</div>"
            .into(),
        rect: [40, 604, 211, 44],
        styles: vec![
            ["display".into(), "inline-flex".into()],
            ["padding".into(), "3px".into()],
        ],
        ..team.clone()
    };
    let sent = format!(
        "Make {} the visual anchor and give {} more breathing room.",
        team.link(),
        toggle.link()
    );
    let received = zeron_proto::annotation::annotation_prompt(&sent);
    let entries = json!([
        {"id": "u1", "role": "user", "createdAt": 1788900000000_i64, "deviceId": device,
         "parts": [{"id": "t", "kind": "text", "text": "The pricing cards feel cramped. Can you tighten the hierarchy?"}]},
        {"id": "a1", "role": "assistant", "createdAt": 1788900001000_i64, "deviceId": device, "status": "complete",
         "parts": [{"id": "p", "kind": "text", "text": "Sure. Point me at the parts you mean: use the annotation button in the browser toolbar, click an element on the page, and it lands in your message as code."}]},
        {"id": "u2", "role": "user", "createdAt": 1788900002000_i64, "deviceId": device,
         "parts": [{"id": "t", "kind": "text", "text": sent}]},
        {"id": "a2", "role": "assistant", "createdAt": 1788900003000_i64, "deviceId": device, "status": "complete",
         "parts": [{"id": "p", "kind": "text", "text": format!("Got both. This is exactly what I received: plain text, so it works with any model, vision or not.

````text
{received}
````")}]},
    ]);
    // ZERON_FIXTURE_QUESTION=1: end on a pending agent question whose
    // multi-line answer shows the panel growing with its text.
    let mut entries = entries;
    if std::env::var_os("ZERON_FIXTURE_QUESTION").is_some() {
        entries.as_array_mut().unwrap().push(json!(
            {"id": "a3", "role": "assistant", "createdAt": 1788900004000_i64, "deviceId": device, "status": "streaming",
             "parts": [{"id": "q", "kind": "input", "requestId": "r1", "resolved": false, "questions": [{
                "id": "spacing", "header": "Spacing", "question": "How much room should the pricing cards get?",
                "options": ["Tighter", "Keep as is", "More breathing room"],
                "prefill": "More room between cards,
but keep the Team card as the anchor.
Also align the buttons to one baseline.
And shorten the feature lists."}]}]}
        ));
    }

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
                    serde_json::from_value(json!({"id": device, "name": "This device",
                    "platform": std::env::consts::OS, "lastSeenAt": null}))
                    .unwrap(),
                ];
                s.chats = chats;
                s.selected_chat = Some("annotations".into());
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
                            gpui::point(px(40.), px(40.)),
                            size(px(1440.), px(900.)),
                        ))),
                        ..Default::default()
                    },
                    |_, cx| cx.new(|cx| shell::Shell::new(state.clone(), boot, cx)),
                )
                .unwrap();
            cx.on_window_closed(|cx, _| cx.quit()).detach();
            state.update(cx, |_, cx| cx.notify());
            cx.activate(true);
            cx.spawn(async move |cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(800))
                    .await;
                state.update(cx, |s, cx| {
                    s.receive_transcript_frame(
                        zeron_doc::TranscriptFrame::Reset {
                            reset: serde_json::from_value(entries).unwrap(),
                        },
                        cx,
                    )
                    .unwrap();
                    cx.notify();
                });
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(400))
                    .await;
                let _ = window.update(cx, |shell, window, cx| {
                    shell.fixture_open_browser(site, window, cx)
                });
            })
            .detach();
        });
    runtime.block_on(core.shutdown());
    Ok(())
}
