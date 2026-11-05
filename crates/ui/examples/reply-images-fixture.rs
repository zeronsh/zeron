//! Images in agent replies, rendered by the production Shell over a real
//! engine (IPC). `markdown`: one reply naming two images in its chat's folder
//! and nine attempts to reach past it (symlinks need unix), after printing the
//! engine's own `ReadChatImageChunk` verdict for every path. `show-image`: the
//! image parts `show_image` publishes, stored as it stores them.
//!
//! `cargo run -p zeron-ui --features appshots-fixture,resource-profile --example reply-images-fixture -- \
//!    [hold-secs] [app-id] [markdown|show-image]`
//!
//! Prints `READY` once the reads have landed (or been refused), keeps the
//! window up for `hold-secs` (default 20) so the compositor can capture it,
//! then quits. Captures: `docs/screenshots/agent-images/`.
use gpui::{AppContext, Bounds, WindowBounds, WindowOptions, px, size};
use serde_json::json;
use std::{io::Write, path::Path, sync::Arc, time::Duration};
use zeron_ui::*;

fn port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

async fn pause(cx: &mut gpui::AsyncApp, ms: u64) {
    cx.background_executor()
        .timer(Duration::from_millis(ms))
        .await;
}

/// A dark panel with rows of bars, so the image reads as a screenshot.
fn screenshot(path: &Path, width: u32, height: u32, accent: [u8; 3], rows: u32) {
    let image = image::RgbImage::from_fn(width, height, |x, y| {
        let row_height = height / (rows + 1);
        let row = y / row_height;
        let in_bar = row >= 1
            && y % row_height > row_height / 4
            && y % row_height < row_height * 3 / 4
            && x > width / 12
            && x < width / 12 + (width * (5 + row) / 12).min(width * 10 / 12);
        if in_bar {
            image::Rgb(accent.map(|c| c.saturating_sub((row * 18) as u8)))
        } else {
            let shade = 22 + (y * 16 / height) as u8;
            image::Rgb([shade, shade, shade + 6])
        }
    });
    image.save(path).unwrap();
}

/// Bars of a chart on a dark background.
fn chart(path: &Path) {
    let heights = [0.35, 0.6, 0.45, 0.8, 0.7, 0.95];
    let (width, height) = (560u32, 320u32);
    let image = image::RgbImage::from_fn(width, height, |x, y| {
        let slot = width / heights.len() as u32;
        let bar = (x / slot) as usize;
        let inside = x % slot > slot / 5 && x % slot < slot * 4 / 5;
        let top = height as f32 * (1.0 - heights[bar.min(heights.len() - 1)] * 0.85);
        if inside && (y as f32) > top && y < height - 12 {
            image::Rgb([64, 196, 160])
        } else {
            image::Rgb([24, 26, 30])
        }
    });
    image.save(path).unwrap();
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let hold: u64 = args.first().and_then(|s| s.parse().ok()).unwrap_or(20);
    let app_id = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| "zeron-fixture-chat".into());
    tracing_subscriber::fmt()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "warn".into()))
        .init();

    let temp = tempfile::tempdir()?;
    let engine_dir = temp.path().join("engine");
    let folder = temp.path().join("chat-folder");
    let elsewhere = temp.path().join("elsewhere");
    std::fs::create_dir_all(folder.join("shots"))?;
    std::fs::create_dir_all(&elsewhere)?;
    screenshot(
        &folder.join("shots/before.png"),
        640,
        360,
        [120, 150, 255],
        4,
    );
    chart(&folder.join("chart.png"));
    // Never shown: red, so a leak would be obvious.
    screenshot(&elsewhere.join("secret.png"), 640, 360, [255, 60, 60], 2);
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(folder.join("shots/before.png"), folder.join("alias.png"))?;
        std::os::unix::fs::symlink(elsewhere.join("secret.png"), folder.join("out.png"))?;
    }
    std::fs::write(folder.join("fake.png"), b"not an image")?;
    std::fs::File::create(folder.join("big.png"))?.set_len(11 * 1024 * 1024)?;

    let runtime = tokio::runtime::Runtime::new()?;
    let core = runtime.block_on(async {
        zeron_engine::EngineCore::assemble(
            &engine_dir,
            Arc::new(zeron_engine::HarnessRegistry::new()),
            zeron_proto::HarnessId::ClaudeCode,
            None,
        )
    })?;
    let device = core.device_id.clone();
    let chat_id = "reply-images".to_string();
    runtime.block_on(async {
        core.workspace.create_chat(
            &chat_id,
            None,
            Some(&device),
            None,
            Some(folder.to_string_lossy().into_owned()),
        )
    })?;

    let shown = [
        ("relative to the chat's folder", "shots/before.png"),
        ("absolute", &*folder.join("chart.png").to_string_lossy()),
    ]
    .map(|(label, path)| (label, path.to_owned()));
    let refused = [
        ("symlink inside the folder", "alias.png".to_owned()),
        ("symlink out of the folder", "out.png".to_owned()),
        (
            "outside the folder",
            elsewhere.join("secret.png").to_string_lossy().into_owned(),
        ),
        ("`..` escape", "shots/../../elsewhere/secret.png".to_owned()),
        ("fake PNG", "fake.png".to_owned()),
        ("too large (11 MB)", "big.png".to_owned()),
        ("URL", "https://example.com/x.png".to_owned()),
        ("home", "~/Pictures/x.png".to_owned()),
        ("network share", "//host/share/x.png".to_owned()),
    ];

    // The engine's verdict for each path, as the UI would send it.
    let client = runtime.block_on(async { zeron_rpc::memory_client(core.rpc_service()) });
    for (label, path) in shown.iter().chain(&refused) {
        let absolute = if path.starts_with('/') || path.contains(':') || path.starts_with('~') {
            path.clone()
        } else {
            folder.join(path).to_string_lossy().into_owned()
        };
        let verdict = runtime.block_on(client.call(
            zeron_rpc::methods::READ_CHAT_IMAGE_CHUNK,
            json!({ "chatId": chat_id, "path": absolute, "offset": 0 }),
        ));
        match verdict {
            Ok(chunk) => println!("ENGINE ✓ {label}: {} ({})", path, chunk["mimeType"]),
            Err(error) => println!("ENGINE ✗ {label}: {path} → {error}"),
        }
    }

    let entry = |id: &str, role: &str, mut parts: Vec<serde_json::Value>, text: &str| {
        parts.push(json!({ "kind": "text", "id": format!("{id}-text"), "text": text }));
        serde_json::from_value::<zeron_doc::SessionMessageEntry>(json!({
            "id": id, "role": role, "createdAt": 0, "deviceId": device, "status": "complete",
            "parts": parts,
        }))
        .unwrap()
    };
    let entries = if args.get(2).map(String::as_str) == Some("show-image") {
        // What `show_image` publishes: the file stored in uploads, as an
        // image part above the reply's text (`PublishImage`, rpc.rs).
        let parts = [
            ("img:before", "shots/before.png"),
            ("img:chart", "chart.png"),
        ]
        .iter()
        .map(|(id, path)| {
            let bytes = std::fs::read(folder.join(path)).unwrap();
            let name = Path::new(path).file_name().unwrap().to_string_lossy();
            let image = core.uploads.store_shown_image(id, &name, &bytes).unwrap();
            json!({ "kind": "image", "id": id, "path": image.path, "name": image.name,
                        "mimeType": image.mime_type })
        })
        .collect();
        vec![
            entry(
                "ask",
                "user",
                Vec::new(),
                "Show me the before screenshot and the chart.",
            ),
            entry(
                "reply",
                "assistant",
                parts,
                "The last release is the tallest bar.",
            ),
        ]
    } else {
        let mut reply = String::from("Both screenshots, read from the chat's folder:\n\n");
        for (_, path) in &shown {
            reply.push_str(&format!("![screenshot]({path})\n\n"));
        }
        reply.push_str("These stay links, nothing is shown:\n\n");
        for (label, path) in &refused {
            reply.push_str(&format!("- {label}: ![{label}]({path})\n"));
        }
        vec![
            entry("ask", "user", Vec::new(), "Show me the screenshots."),
            entry("reply", "assistant", Vec::new(), &reply),
        ]
    };

    let data = temp.path().join("ui");
    std::fs::create_dir(&data)?;
    let ipc_port = port();
    let _ipc = runtime.block_on(zeron_engine::serve_ipc(ipc_port, &data, core.rpc_service()))?;
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

    gpui_platform::application()
        .with_assets(icons::Assets)
        .run(move |cx| {
            gpui_tokio::init(cx);
            gpui_base::init(cx);
            let mut settings = settings::UiSettings::default();
            settings.reduce_motion = motion::ReduceMotion::On;
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
                s.selected_chat = Some(chat_id.clone());
                s.auto_selected = true;
                s.chats_synced = true;
                s.spaces_synced = true;
                s.no_project = true;
                s
            });
            let window = cx
                .open_window(
                    WindowOptions {
                        app_id: Some(app_id),
                        window_background: theme::Theme::of(cx).window_background_appearance(),
                        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                            gpui::point(px(40.), px(40.)),
                            size(px(1280.), px(1300.)),
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
                pause(cx, 800).await;
                state.update(cx, |s, cx| {
                    s.receive_transcript_frame(
                        zeron_doc::TranscriptFrame::Reset { reset: entries },
                        cx,
                    )
                    .unwrap();
                    cx.notify();
                });
                // A window the compositor never showed gets no frame
                // callbacks: draw and present explicitly, long enough for the
                // reads to land (or be refused) and the rows to settle.
                let present = |cx: &mut gpui::AsyncApp| {
                    gpui::AnyWindowHandle::from(window)
                        .update(cx, |_, w, cx| {
                            w.refresh();
                            let arena = w.draw(cx);
                            w.present_if_needed();
                            arena.clear();
                        })
                        .ok();
                };
                for _ in 0..16 {
                    present(cx);
                    pause(cx, 300).await;
                }
                println!("READY");
                std::io::stdout().flush().ok();
                for _ in 0..(hold * 1000 / 300) {
                    present(cx);
                    pause(cx, 300).await;
                }
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
    runtime.block_on(core.shutdown());
    Ok(())
}
