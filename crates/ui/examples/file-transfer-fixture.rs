//! Isolated native review fixture for device file transfers: the titlebar
//! transfers panel, the incoming toast, the "Send to device" picker and the
//! Settings → Devices receive section, over fake rows (no engine, nothing is
//! sent). ZERON_TRANSFER_SHOT = panel (default) | toast | send | settings;
//! ZERON_PALETTE_LIGHT selects the light appearance.
use gpui::{AppContext, Bounds, WindowBounds, WindowOptions, px, size};
use zeron_proto::{
    FileTransfer, FileTransferDirection as Direction, FileTransferItem,
    FileTransferItemKind as Kind, FileTransferState as State, FileTransferTransport as Transport,
};
use zeron_ui::*;

#[allow(clippy::too_many_arguments)]
fn row(
    id: &str,
    direction: Direction,
    peer: (&str, &str),
    state: State,
    item: (&str, Kind, u64, u64),
    done: u64,
    rate: u64,
    age_secs: i64,
) -> FileTransfer {
    let now = chrono::Utc::now().timestamp_millis();
    let (name, kind, size, files) = item;
    FileTransfer {
        id: id.into(),
        direction,
        peer_device_id: peer.0.into(),
        peer_device_name: peer.1.into(),
        state,
        transport: Some(if rate > 5_000_000 {
            Transport::P2p
        } else {
            Transport::Relay
        }),
        items: vec![FileTransferItem {
            name: name.into(),
            kind,
            size,
            file_count: files,
            path: Some(format!("/home/alex/Zeron Transfers/{}/{name}", peer.1)),
        }],
        file_count: files,
        total_bytes: size,
        done_bytes: done,
        bytes_per_sec: rate,
        destination: Some(format!("/home/alex/Zeron Transfers/{}", peer.1)),
        skipped: 0,
        error: None,
        created_at: now - age_secs * 1000,
        updated_at: now - age_secs * 1000,
        finished_at: state.is_terminal().then_some(now - age_secs * 1000),
    }
}

fn main() -> anyhow::Result<()> {
    let runtime = tokio::runtime::Runtime::new()?;
    let _guard = runtime.enter();
    tracing_subscriber::fmt().with_env_filter("warn").init();
    let shot = std::env::var("ZERON_TRANSFER_SHOT").unwrap_or_else(|_| "panel".into());
    let temp = tempfile::tempdir()?;
    let data = temp.path().to_path_buf();
    gpui_platform::application().with_assets(icons::Assets).run(move |cx| {
        gpui_tokio::init(cx);
        gpui_base::init(cx);
        let mut settings = settings::UiSettings::default();
        settings.sidebar_width = 280.0;
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
        let project_path = data.join("fieldnotes");
        std::fs::create_dir_all(project_path.join("build")).unwrap();
        let ft = zeron_proto::capabilities::FILE_TRANSFER_V1;
        let now = chrono::Utc::now();
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
                ("local", "Workstation", "linux", Some(now), vec![ft]),
                ("laptop", "ThinkPad X1", "linux", Some(now), vec![ft]),
                ("server", "Build server", "linux", Some(now), vec![ft]),
                ("mac", "MacBook Air", "macos", Some(now), vec!["harness-updates-v1"]),
                ("nas", "Attic NAS", "linux", None, vec![ft]),
            ]
            .into_iter()
            .map(|(id, name, platform, seen, caps)| {
                serde_json::from_value(serde_json::json!({
                    "id": id, "name": name, "platform": platform,
                    "lastSeenAt": seen, "capabilities": caps,
                }))
                .unwrap()
            })
            .collect();
            s.selected_chat = Some("fixture".into());
            s.selected_space = Some("project".into());
            s.auto_selected = true;
            s.chats_synced = true;
            s.spaces_synced = true;
            s.spaces = vec![serde_json::from_value(serde_json::json!({"id":"project","deviceId":"local","path":project_path,"createdAt":"2026-09-08T00:00:00Z"})).unwrap()];
            s.chats = vec![serde_json::from_value(serde_json::json!({"id":"fixture","deviceId":"local","spaceId":"project","cwd":project_path,"title":"Ship the release build","archived":false,"createdAt":"2026-09-08T00:00:00Z","config":{"harness":"claude-code","model":"claude-sonnet-4-6","reasoning":null,"sandbox":"workspace-write"}})).unwrap()];
            let mut failed = row(
                "fail",
                Direction::Outgoing,
                ("nas", "Attic NAS"),
                State::Failed,
                ("dataset.tar.zst", Kind::File, 7_400_000_000, 1),
                2_100_000_000,
                0,
                240,
            );
            failed.error =
                Some("Attic NAS stopped responding and didn't come back within 15 minutes".into());
            s.file_transfers = vec![
                row(
                    "ask",
                    Direction::Incoming,
                    ("laptop", "ThinkPad X1"),
                    State::AwaitingAcceptance,
                    ("Screenshots", Kind::Folder, 18_400_000, 23),
                    0,
                    0,
                    5,
                ),
                row(
                    "recv",
                    Direction::Incoming,
                    ("server", "Build server"),
                    State::Transferring,
                    ("fieldnotes-0.4.0.tar.gz", Kind::File, 48_300_000, 1),
                    31_000_000,
                    12_400_000,
                    20,
                ),
                row(
                    "send",
                    Direction::Outgoing,
                    ("laptop", "ThinkPad X1"),
                    State::Transferring,
                    ("design-review.mp4", Kind::File, 1_200_000_000, 1),
                    380_000_000,
                    3_100_000,
                    60,
                ),
                row(
                    "done",
                    Direction::Incoming,
                    ("server", "Build server"),
                    State::Completed,
                    ("coverage-report.pdf", Kind::File, 2_400_000, 1),
                    2_400_000,
                    0,
                    120,
                ),
                failed,
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
                        size(px(1100.), px(760.)),
                    ))),
                    app_owns_titlebar_drag: true,
                    ..Default::default()
                },
                |_, cx| cx.new(|cx| shell::Shell::new(state.clone(), boot, cx)),
            )
            .unwrap();
        state.update(cx, |_, cx| cx.notify());
        cx.spawn(async move |cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(700))
                .await;
            window
                .update(cx, |shell, _, cx| match shot.as_str() {
                    "toast" => shell.fixture_file_transfer_toast("ask", cx),
                    "send" => shell.fixture_send_menu(
                        "fixture",
                        "build/fieldnotes-0.4.0.tar.gz",
                        gpui::point(px(420.), px(160.)),
                        cx,
                    ),
                    "settings" => shell.fixture_devices_settings(
                        zeron_proto::FileTransferSettings {
                            require_confirmation: true,
                            inbox_dir: None,
                        },
                        cx,
                    ),
                    _ => shell.fixture_file_transfers_panel(cx),
                })
                .ok();
        })
        .detach();
    });
    Ok(())
}
