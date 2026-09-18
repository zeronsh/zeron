//! Offline New project flow fixture. Uses synthetic devices and folder responses.
use gpui::{AppContext, Bounds, WindowBounds, WindowOptions, px, size};
use zeron_ui::*;

fn main() -> anyhow::Result<()> {
    let runtime = tokio::runtime::Runtime::new()?;
    let _guard = runtime.enter();
    tracing_subscriber::fmt().with_env_filter("warn").init();
    let temp = tempfile::tempdir()?;
    let data = temp.path().to_path_buf();
    gpui_platform::application().with_assets(icons::Assets).run(move |cx| {
        gpui_tokio::init(cx); gpui_base::init(cx);
        let mut settings = settings::UiSettings::default();
        settings.sidebar_show_branch = true;
        if let Ok(path) = std::env::var("ZERON_FIXTURE_BACKGROUND") {
            settings.new_thread_composer_background = Some(settings::NewThreadComposerBackground { path, name: "Uploaded background".into() });
        }
        settings.surface = zeron_theme::SurfacePreference::Frosted;
        settings.save(&data).unwrap();
        settings::init(settings.clone(), data.clone(), cx);
        let fonts = typography::register_fonts(cx);
        typography::init(settings.ui_font_family.clone(), settings.ui_font_size, settings.terminal_font_family.clone(), settings.terminal_font_size, settings.code_font_family.clone(), settings.code_font_size, fonts, cx);
        theme_library::init(data.clone(), cx);
        appearance::init(if std::env::var_os("ZERON_PALETTE_LIGHT").is_some() { appearance::AppearanceMode::Light } else { appearance::AppearanceMode::Dark }, settings.theme_selection, settings.accent, settings.surface, cx);
        history::init(settings.git_history_columns, settings.git_history_column_widths,
            settings.git_history_column_order, settings.git_history_author_display, cx);
        composer::init(cx, settings.composer_send_behavior); terminal::panel::init(cx); app_menus::init(cx);
        let state = cx.new(|_| {
            let mut s = state::AppState::new();
            s.connection = zeron_proto::view::ConnectionStatus::Ready;
            s.workspace_scope = Some(zeron_proto::WorkspaceScope::Local);
            s.local_device_id = Some("local".into());
            s.devices = serde_json::from_value(serde_json::json!([
                {"id":"local","name":"Studio Mac","platform":"macos","lastSeenAt":null},
                {"id":"remote","name":"Build server","platform":"linux","lastSeenAt":null},
                {"id":"laptop","name":"Travel laptop","platform":"macos","lastSeenAt":null}
            ])).unwrap();
            s.selected_chat = if std::env::var_os("ZERON_FIXTURE_BACKGROUND").is_some() { None } else { Some("browser-fixture".into()) }; s.selected_space = Some("project".into());
            s.auto_selected = true; s.chats_synced = true; s.spaces_synced = true;
            s.spaces = vec![serde_json::from_value(serde_json::json!({"id":"project","deviceId":"local","path":"/tmp/fieldnotes","createdAt":"2026-09-08T00:00:00Z"})).unwrap()];
            s.chats = vec![serde_json::from_value(serde_json::json!({"id":"browser-fixture","deviceId":"local","spaceId":"project","title":"Build the Fieldnotes workspace","archived":false,"createdAt":"2026-09-08T00:00:00Z","config":{"harness":"claude-code","model":"claude-sonnet-4-6","reasoning":null,"sandbox":"workspace-write"}})).unwrap()];

            for (ix, title) in ["Polish the command palette", "Fix authentication redirects", "Add deployment status", "Review pull request comments", "Improve keyboard navigation", "Update project documentation", "Refine composer spacing", "Audit chat sync", "Build settings search"].iter().enumerate() {
                let mut chat = s.chats[0].clone();
                chat.id = format!("chat-{ix}");
                chat.title = Some((*title).into());
                chat.branch = Some(format!("fieldnotes/{}", title.to_lowercase().replace(' ', "-")));
                chat.source_context = Some(zeron_proto::ConversationSourceContext {
                    checkout_id: "fixture-checkout".into(), repo_root: "/tmp/fieldnotes".into(),
                    cwd: "/tmp/fieldnotes".into(), branch: chat.branch.clone().unwrap(),
                    head_sha: None, observed_at: chrono::Utc::now(),
                });
                chat.created_at = chrono::Utc::now() - chrono::Duration::minutes((ix as i64 + 1) * 17);
                s.chats.push(chat);
            }
            s
        });
        let boot = EngineBootConfig { data_dir: data, ipc_port: 0, edge_url: String::new(), edge_token: None, org_id: None, workos_client_id: None, default_harness: HarnessId::ClaudeCode };
        let window = cx.open_window(WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(gpui::point(px(12.),px(30.)), size(px(1100.),px(800.))))),
            titlebar: Some(gpui::TitlebarOptions { title: None, appears_transparent: true, traffic_light_position: Some(gpui::point(px(14.),px(14.))) }),
            app_owns_titlebar_drag: true,
            ..Default::default()
        }, |_, cx| cx.new(|cx| shell::Shell::new(state.clone(), boot, cx))).unwrap();
        cx.spawn(async move |cx| {
            loop {
                cx.background_executor().timer(std::time::Duration::from_millis(100)).await;
                if window.update(cx, |shell, _, cx| shell.fixture_project_responses(cx)).is_err() { break; }
            }
        }).detach();
        state.update(cx, |_, cx| cx.notify());
        cx.activate(true);
    });
    Ok(())
}
