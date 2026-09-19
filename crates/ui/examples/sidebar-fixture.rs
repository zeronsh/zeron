//! Isolated native sidebar review fixture. ZERON_SIDEBAR_COMPACT / ZERON_SIDEBAR_HIDE_LABEL select layout.
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
        settings.sidebar_compact = std::env::var_os("ZERON_SIDEBAR_COMPACT").is_some();
        settings.sidebar_show_project_label = std::env::var_os("ZERON_SIDEBAR_HIDE_LABEL").is_none();
        settings.sidebar_organization = settings::SidebarOrganization::InOneList;
        settings.sidebar_width = 310.0;
        settings.sidebar_collapsed = std::env::var_os("ZERON_SIDEBAR_PEEK").is_some();
        settings.sidebar_pins_mut("local".into()).extend(["chat-0".into(), "chat-1".into()]);
        let project_path = data.join("fieldnotes");
        std::fs::create_dir_all(project_path.join("public")).unwrap();
        std::fs::write(project_path.join("public/favicon.svg"), r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><rect x="1" y="1" width="22" height="22" rx="6" fill="#668cf5"/><path d="M7 6h11v3h-8v3h6v3h-6v4H7Z" fill="white"/></svg>"##).unwrap();
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
            s.devices = vec![serde_json::from_value(serde_json::json!({"id":"local","name":"This device","platform":std::env::consts::OS,"lastSeenAt":null})).unwrap()];
            s.selected_chat = Some("browser-fixture".into()); s.selected_space = Some("project".into());
            s.auto_selected = true; s.chats_synced = true; s.spaces_synced = true;
            s.spaces = vec![serde_json::from_value(serde_json::json!({"id":"project","deviceId":"local","path":project_path,"createdAt":"2026-09-08T00:00:00Z"})).unwrap()];
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
            s.devices.push(serde_json::from_value(serde_json::json!({"id":"remote","name":"Build server","platform":"linux","lastSeenAt":chrono::Utc::now()})).unwrap());
            s.spaces.push(serde_json::from_value(serde_json::json!({"id":"backend","deviceId":"remote","name":"API server","path":"/projects/backend","createdAt":chrono::Utc::now()})).unwrap());
            for ix in [2, 3, 5, 7, 8] {
                s.chats[ix].device_id = "remote".into();
                s.chats[ix].space_id = Some("backend".into());
            }
            for ix in [1, 2, 4, 8] {
                let chat = s.chats[ix].clone();
                let source = chat.source_context.as_ref().unwrap();
                s.fixture_sidebar_change_request(zeron_proto::CheckoutChangeRequestStatus {
                    checkout_id: source.checkout_id.clone(), device_id: chat.device_id.clone(), cwd: source.repo_root.clone(), branch: source.branch.clone(), updated_at: chrono::Utc::now(),
                    change_request: Some(zeron_proto::ChangeRequestSummary { provider: "github".into(), number: 412 + ix as u64, title: chat.title.clone().unwrap(), url: "https://github.com/zeronsh/zeron/pull/412".into(), state: zeron_proto::ChangeRequestState::Open, base_ref: "main".into(), head_ref: source.branch.clone() }),
                });
            }
            s.chats[4].last_message_at = Some(chrono::Utc::now());
            s.chats[8].archived = true;
            s
        });
        let boot = EngineBootConfig { data_dir: data, ipc_port: 0, edge_url: String::new(), edge_token: None, org_id: None, workos_client_id: None, default_harness: HarnessId::ClaudeCode };
        let _window = cx.open_window(WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(gpui::point(px(12.),px(30.)), size(px(1100.),px(800.))))),
            titlebar: Some(gpui::TitlebarOptions { title: None, appears_transparent: true, traffic_light_position: Some(gpui::point(px(14.),px(14.))) }),
            app_owns_titlebar_drag: true,
            window_background: theme::Theme::of(cx).window_background_appearance(),
            ..Default::default()
        }, |_, cx| cx.new(|cx| shell::Shell::new(state.clone(), boot, cx))).unwrap();
        state.update(cx, |s, cx| {
            // Put content underneath the floating sidebar so its backdrop blur
            // can be compared with the contextual menus in this isolated app.
            let entries = serde_json::from_value(serde_json::json!([
                {"id":"fixture-user","role":"user","parts":[{"id":"text","kind":"text","text":"Review the Fieldnotes workspace and its sidebar."}],"createdAt":1788900000000_i64,"deviceId":"local"},
                {"id":"fixture-assistant","role":"assistant","parts":[{"id":"text","kind":"text","text":"## Fieldnotes workspace\n\nThe floating sidebar should reveal the conversation behind its glass surface. Move to the left edge to preview your sessions, then move back into the conversation.\n\n### Workspace details\n\n| Project | Branch | Status |\n| --- | --- | --- |\n| Fieldnotes | sidebar-preview | Ready for review |\n| API server | authentication | In progress |\n| Documentation | getting-started | Published |\n\n```rust\nlet workspace = Workspace::new(\n    \"Fieldnotes\",\n    Surface::Frosted,\n);\nworkspace.open_sidebar();\n```\n\nThe titlebar stays available while the sidebar is open. Contextual menus use the same frosted surface.\n\n### Next steps\n\n- Review the session list\n- Open the view options menu\n- Move between the panel and the conversation\n- Pin the sidebar using the titlebar toggle"}],"createdAt":1788900001000_i64,"deviceId":"local","status":"complete"}
            ])).unwrap();
            s.receive_transcript_frame(zeron_doc::TranscriptFrame::Reset { reset: entries }, cx).unwrap();
            cx.notify();
        });
        cx.activate(true);
    });
    Ok(())
}
