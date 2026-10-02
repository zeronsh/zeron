//! Isolated native review fixture for agent-spawned chats: a coordinator
//! chat whose transcript holds a Zeron `create_chats` call that made a
//! top-level chat on another device ("GPU box") and a side chat, with the
//! spawned chat listed in the sidebar wearing its "spawned by" marker. No
//! engine, nothing is sent; the tool call carries no output, exactly like a
//! synced doc, so the cards resolve through the chats' provenance.
//!
//! ZERON_AGENT_CHATS_HOVER="x,y" hovers a window point (logical px) after
//! the first frame so a tooltip shows; ZERON_PALETTE_LIGHT selects the light
//! appearance.
use gpui::{AppContext, Bounds, WindowBounds, WindowOptions, px, size};
use zeron_ui::*;

fn main() -> anyhow::Result<()> {
    let runtime = tokio::runtime::Runtime::new()?;
    let _guard = runtime.enter();
    tracing_subscriber::fmt().with_env_filter("warn").init();
    let hover = std::env::var("ZERON_AGENT_CHATS_HOVER").ok().and_then(|at| {
        let (x, y) = at.split_once(',')?;
        Some(gpui::point(px(x.trim().parse().ok()?), px(y.trim().parse().ok()?)))
    });
    let temp = tempfile::tempdir()?;
    let data = temp.path().to_path_buf();
    gpui_platform::application()
        .with_assets(icons::Assets)
        .run(move |cx| {
            gpui_tokio::init(cx);
            gpui_base::init(cx);
            let mut settings = settings::UiSettings::default();
            settings.sidebar_width = 290.0;
            settings.sidebar_organization = settings::SidebarOrganization::InOneList;
            settings.surface = zeron_theme::SurfacePreference::Frosted;
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
            appearance::init(
                if std::env::var_os("ZERON_PALETTE_LIGHT").is_some() {
                    appearance::AppearanceMode::Light
                } else {
                    appearance::AppearanceMode::Dark
                },
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

            let project_path = data.join("tokenizer");
            std::fs::create_dir_all(&project_path).unwrap();
            let now = chrono::Utc::now();
            let ago = |minutes: i64| now - chrono::Duration::minutes(minutes);
            // The coordinator's reply started 6 minutes ago; its create_chats
            // call made both chats a few seconds later.
            let turn_at = ago(6);
            let spawned_at = turn_at + chrono::Duration::seconds(4);
            let config = serde_json::json!({
                "harness": "claude-code", "model": "claude-sonnet-4-6",
                "reasoning": null, "sandbox": "workspace-write",
            });
            let chat = |id: &str,
                        device: &str,
                        title: &str,
                        created: chrono::DateTime<chrono::Utc>,
                        last: chrono::DateTime<chrono::Utc>,
                        parent: Option<&str>,
                        spawned_by: Option<&str>| {
                serde_json::from_value::<zeron_proto::Chat>(serde_json::json!({
                    "id": id, "deviceId": device,
                    "spaceId": if device == "local" { Some("project") } else { None },
                    "cwd": if device == "local" { Some(project_path.clone()) } else { None },
                    "title": title, "archived": false,
                    "createdAt": created, "lastMessageAt": last, "lastSeenAt": last,
                    "parentChatId": parent, "spawnedByChatId": spawned_by,
                    "config": config,
                }))
                .unwrap()
            };
            let state = cx.new(|_| {
                let mut s = state::AppState::new();
                s.connection = zeron_proto::view::ConnectionStatus::Ready;
                s.workspace_scope = Some(zeron_proto::WorkspaceScope::Synced);
                s.auth = Some(zeron_proto::AuthState::SignedIn {
                    user: zeron_proto::UserProfile {
                        id: "fixture-user".into(),
                        email: "alex@example.test".into(),
                        name: Some("Alex".into()),
                    },
                    org_id: Some("fixture-org".into()),
                });
                s.local_device_id = Some("local".into());
                s.devices = [
                    ("local", "Workstation", "linux"),
                    ("gpu", "GPU box", "linux"),
                ]
                .into_iter()
                .map(|(id, name, platform)| {
                    serde_json::from_value(serde_json::json!({
                        "id": id, "name": name, "platform": platform,
                        "lastSeenAt": now, "capabilities": [],
                    }))
                    .unwrap()
                })
                .collect();
                s.selected_chat = Some("coordinator".into());
                s.selected_space = Some("project".into());
                s.auto_selected = true;
                s.chats_synced = true;
                s.spaces_synced = true;
                s.spaces = vec![
                    serde_json::from_value(serde_json::json!({
                        "id": "project", "deviceId": "local", "path": project_path,
                        "createdAt": ago(600),
                    }))
                    .unwrap(),
                ];
                let mut review = chat(
                    "side-review",
                    "local",
                    "Review the tokenizer API surface",
                    spawned_at,
                    ago(1),
                    Some("coordinator"),
                    Some("coordinator"),
                );
                // Finished and not yet looked at: the sidebar's "Done".
                review.last_seen_at = Some(ago(3));
                s.chats = vec![
                    chat(
                        "coordinator",
                        "local",
                        "Coordinate the tokenizer rewrite",
                        ago(40),
                        ago(2),
                        None,
                        None,
                    ),
                    chat(
                        "worker-gpu",
                        "gpu",
                        "Benchmark the tokenizer on CUDA",
                        // The batch's first entry, created first.
                        spawned_at - chrono::Duration::seconds(1),
                        ago(0),
                        None,
                        Some("coordinator"),
                    ),
                    review,
                    chat(
                        "docs",
                        "local",
                        "Update the contributor guide",
                        ago(180),
                        ago(90),
                        None,
                        None,
                    ),
                    chat(
                        "flaky",
                        "local",
                        "Fix the flaky sync test",
                        ago(300),
                        ago(200),
                        None,
                        None,
                    ),
                ];
                s.sessions = vec![
                    serde_json::from_value(serde_json::json!({
                        "chatId": "worker-gpu", "deviceId": "gpu", "status": "working",
                        "startedAt": spawned_at, "updatedAt": now,
                    }))
                    .unwrap(),
                ];
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
            let window = cx
                .open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                            gpui::point(px(0.), px(0.)),
                            size(px(1100.), px(720.)),
                        ))),
                        app_owns_titlebar_drag: true,
                        ..Default::default()
                    },
                    |_, cx| cx.new(|cx| shell::Shell::new(state.clone(), boot, cx)),
                )
                .unwrap();
            state.update(cx, |_, cx| cx.notify());
            let transcript = serde_json::json!([
                {
                    "id": "user", "role": "user", "deviceId": "local",
                    "createdAt": (turn_at - chrono::Duration::seconds(20)).timestamp_millis(),
                    "parts": [{ "id": "text", "kind": "text", "text":
                        "Split the tokenizer rewrite: benchmark it on the GPU box, and have a side chat review the public API while you wire it in." }],
                },
                {
                    "id": "assistant", "role": "assistant", "deviceId": "local",
                    "status": "complete", "durationMs": 48_000,
                    "createdAt": turn_at.timestamp_millis(),
                    "parts": [
                        { "id": "t0", "kind": "text", "text":
                            "I'll fan this out so the benchmark and the review run while I wire the new tokenizer in." },
                        // As the doc fold keeps it: the call, resolved, and
                        // the ids its result named (never the output).
                        { "id": "tool-create", "kind": "tool", "resolved": true, "isError": false,
                          "call": { "kind": "mcp", "server": "zeron", "tool": "create_chats" },
                          "createdChatIds": ["worker-gpu", "side-review"] },
                        { "id": "t1", "kind": "text", "text":
                            "Both are running: the benchmark is a top-level chat on the GPU box, and the API review is a side chat under this one. I'll fold their results in as they report back." },
                    ],
                },
            ]);
            cx.spawn(async move |cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(900))
                    .await;
                state
                    .update(cx, |s, cx| {
                        s.receive_transcript_frame(
                            zeron_doc::TranscriptFrame::Reset {
                                reset: serde_json::from_value(transcript).unwrap(),
                            },
                            cx,
                        )
                        .unwrap();
                        cx.notify();
                    });
                let Some(position) = hover else { return };
                // Through the untyped handle: the typed one leases the Shell,
                // which the dispatched event must update.
                let window: gpui::AnyWindowHandle = window.into();
                for _ in 0..3 {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(400))
                        .await;
                    window
                        .update(cx, |_, window, cx| {
                            window.dispatch_event(
                                gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
                                    position,
                                    pressed_button: None,
                                    modifiers: gpui::Modifiers::default(),
                                }),
                                cx,
                            );
                        })
                        .ok();
                }
            })
            .detach();
        });
    Ok(())
}
