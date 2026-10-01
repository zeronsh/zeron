//! Offline screenshots of the permissions menu (modes and sandbox) on the real
//! shell. Writes PNGs to the directory given as the first argument; no agent
//! runs and nothing leaves the machine.
//!
//!   cargo run -p zeron-ui --features appshots-fixture --example permissions-fixture -- /tmp/shots
use gpui::{AppContext, AsyncApp, Bounds, WindowBounds, WindowOptions, px, size};
use std::{path::PathBuf, sync::Arc, time::Duration};
use zeron_ui::*;

async fn pause(cx: &mut AsyncApp, ms: u64) {
    cx.background_executor()
        .timer(Duration::from_millis(ms))
        .await;
}

fn capture(
    window: gpui::AnyWindowHandle,
    cx: &mut AsyncApp,
    directory: &std::path::Path,
    name: &str,
) -> anyhow::Result<()> {
    window.update(cx, |_, w, cx| {
        w.draw(cx).clear();
        w.render_to_image()?
            .save(directory.join(format!("{name}.png")))?;
        Ok(())
    })?
}

fn port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn chat(id: &str, title: &str, harness: &str, model: &str, policy: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "id": id, "deviceId": "local", "spaceId": "project", "title": title,
        "archived": false, "createdAt": "2026-09-30T00:00:00Z",
        "config": {
            "harness": harness, "model": model, "reasoning": null,
            "sandbox": "workspace-write", "policy": policy
        }
    })
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter("warn").init();
    let output = PathBuf::from(std::env::args().nth(1).expect("output directory"));
    std::fs::create_dir_all(&output)?;
    let temp = tempfile::tempdir()?;
    let runtime = tokio::runtime::Runtime::new()?;
    let core = runtime.block_on(async {
        zeron_engine::EngineCore::assemble(
            &temp.path().join("engine"),
            Arc::new(zeron_engine::default_registry()),
            zeron_proto::HarnessId::ClaudeCode,
            None,
        )
    })?;
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
    let device = core.device_id.clone();
    let chats: Vec<zeron_proto::Chat> = vec![
        chat(
            "claude",
            "Fix the flaky sync test",
            "claude-code",
            "claude-sonnet-4-6",
            serde_json::json!({}),
        ),
        chat(
            "cursor",
            "Tidy the settings page",
            "cursor",
            "auto",
            serde_json::json!({}),
        ),
        chat(
            "claude-ask",
            "Review the release notes",
            "claude-code",
            "claude-sonnet-4-6",
            serde_json::json!({"mode": "ask"}),
        ),
    ]
    .into_iter()
    .map(|mut row| {
        row["deviceId"] = device.clone().into();
        serde_json::from_value(row).unwrap()
    })
    .collect();
    let failure = Arc::new(std::sync::Mutex::new(None));
    let result = failure.clone();
    gpui_platform::application().with_assets(icons::Assets).run(move |cx| {
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
            s.devices = vec![serde_json::from_value(serde_json::json!({
                "id": device, "name": "This device",
                "platform": std::env::consts::OS, "lastSeenAt": null
            }))
            .unwrap()];
            s.spaces = vec![serde_json::from_value(serde_json::json!({
                "id": "project", "deviceId": device, "path": "/tmp/fieldnotes",
                "createdAt": "2026-09-08T00:00:00Z"
            }))
            .unwrap()];
            s.chats = chats;
            s.selected_chat = Some("claude".into());
            s.selected_space = Some("project".into());
            s.auto_selected = true;
            s.chats_synced = true;
            s.spaces_synced = true;
            s
        });
        let window = cx
            .open_window(
                WindowOptions {
                    window_background: theme::Theme::of(cx).window_background_appearance(),
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        gpui::point(px(20.), px(40.)),
                        size(px(1100.), px(760.)),
                    ))),
                    ..Default::default()
                },
                |_, cx| cx.new(|cx| shell::Shell::new(state.clone(), boot, cx)),
            )
            .unwrap();
        state.update(cx, |_, cx| cx.notify());
        cx.activate(true);
        cx.spawn(async move |cx| {
            let run: anyhow::Result<()> = async {
                pause(cx, 2500).await;
                let composer = window.update(cx, |s, _, _| s.fixture_appshots_composer())?;
                let open_menu = |cx: &mut AsyncApp| {
                    window.update(cx, |_, w, cx| {
                        composer.update(cx, |c, cx| {
                            c.pickers().update(cx, |p, cx| p.fixture_open_mode_menu(w, cx))
                        })
                    })
                };
                let close_menu = |cx: &mut AsyncApp| {
                    window.update(cx, |_, _, cx| {
                        composer.update(cx, |c, cx| {
                            c.pickers().update(cx, |p, cx| p.fixture_close_menu(cx))
                        })
                    })
                };
                // A harness that honours every mode.
                open_menu(cx)?;
                pause(cx, 900).await;
                capture(window.into(), cx, &output, "mode-menu-claude")?;
                close_menu(cx)?;
                pause(cx, 600).await;
                capture(window.into(), cx, &output, "chip-bypass")?;
                // A chat already in Ask.
                close_menu(cx)?;
                state.update(cx, |s, cx| {
                    s.selected_chat = Some("claude-ask".into());
                    cx.notify();
                });
                pause(cx, 900).await;
                pause(cx, 600).await;
                capture(window.into(), cx, &output, "chip-ask")?;
                open_menu(cx)?;
                pause(cx, 900).await;
                capture(window.into(), cx, &output, "mode-menu-ask")?;
                // A harness that never asks: the menu says so.
                close_menu(cx)?;
                state.update(cx, |s, cx| {
                    s.selected_chat = Some("cursor".into());
                    cx.notify();
                });
                pause(cx, 900).await;
                open_menu(cx)?;
                pause(cx, 900).await;
                capture(window.into(), cx, &output, "mode-menu-cursor")?;
                // An approval prompt in the question panel.
                close_menu(cx)?;
                state.update(cx, |s, cx| {
                    s.selected_chat = Some("claude-ask".into());
                    cx.notify();
                });
                pause(cx, 600).await;
                state.update(cx, |s, cx| {
                    s.receive_transcript_frame(
                        zeron_doc::TranscriptFrame::Reset {
                            reset: serde_json::from_value(serde_json::json!([
                                {"id": "user", "role": "user", "createdAt": 1788900000000_i64, "deviceId": device,
                                 "parts": [{"id": "t", "kind": "text", "text": "Run the sync tests and fix whatever fails."}]},
                                {"id": "assistant", "role": "assistant", "createdAt": 1788900001000_i64, "deviceId": device, "status": "streaming",
                                 "parts": [
                                    {"id": "a", "kind": "text", "text": "I'll start by running the sync crate's tests."},
                                    {"id": "in-r1", "kind": "input", "requestId": "r1", "resolved": false,
                                     "questions": [{
                                        "id": "approval:demo", "header": "Permission",
                                        "question": "Allow the agent to run `cargo test -p zeron-sync`?",
                                        "options": ["Allow once", "Always allow", "Deny"], "multiSelect": false
                                     }]}
                                 ]}
                            ]))
                            .unwrap(),
                        },
                        cx,
                    )
                    .unwrap();
                    cx.notify();
                });
                pause(cx, 1200).await;
                capture(window.into(), cx, &output, "approval-prompt")?;
                // A plan waiting for its decision.
                state.update(cx, |s, cx| {
                    s.receive_transcript_frame(
                        zeron_doc::TranscriptFrame::Reset {
                            reset: serde_json::from_value(serde_json::json!([
                                {"id": "user", "role": "user", "createdAt": 1788900000000_i64, "deviceId": device,
                                 "parts": [{"id": "t", "kind": "text", "text": "The sync test is flaky. Figure out why and fix it."}]},
                                {"id": "assistant", "role": "assistant", "createdAt": 1788900001000_i64, "deviceId": device, "status": "streaming",
                                 "parts": [
                                    {"id": "a", "kind": "text", "text": "I've read the test and the sync code. Here's what I'd do."},
                                    {"id": "in-r2", "kind": "input", "requestId": "r2", "resolved": false,
                                     "questions": [{
                                        "id": "plan:demo", "header": "Plan",
                                        "question": "## Fix the flaky sync test\n\nThe test races the debounce timer against a wall clock.\n\n1. Inject a fake clock into `SyncScheduler`\n2. Advance it explicitly in `reconnect_resumes_after_backoff`\n3. Run the suite 50 times to confirm it is stable\n\n```sh\ncargo test -p zeron-sync -- --test-threads=1\n```\n\nNo public API changes.",
                                        "options": zeron_proto::policy::plan_options(), "multiSelect": false
                                     }]}
                                 ]}
                            ]))
                            .unwrap(),
                        },
                        cx,
                    )
                    .unwrap();
                    cx.notify();
                });
                pause(cx, 1200).await;
                capture(window.into(), cx, &output, "plan-card")?;
                Ok(())
            }
            .await;
            if let Err(error) = run {
                *result.lock().unwrap() = Some(error.to_string());
            }
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
    if let Some(error) = failure.lock().unwrap().take() {
        anyhow::bail!(error);
    }
    Ok(())
}
