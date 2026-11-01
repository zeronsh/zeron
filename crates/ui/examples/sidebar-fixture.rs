//! Isolated native sidebar review fixture. ZERON_SIDEBAR_COMPACT / ZERON_SIDEBAR_HIDE_LABEL select layout;
//! ZERON_SIDEBAR_BY_PROJECT groups by project, with a remote clone of the local repository.
//! ZERON_SIDEBAR_WORKTREES replaces the data with three git projects split into worktrees.
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
        let worktrees = std::env::var_os("ZERON_SIDEBAR_WORKTREES").is_some();
        settings.sidebar_show_worktrees = worktrees;
        let by_project = worktrees || std::env::var_os("ZERON_SIDEBAR_BY_PROJECT").is_some();
        settings.sidebar_organization = if by_project { settings::SidebarOrganization::ByProject } else { settings::SidebarOrganization::InOneList };
        settings.sidebar_width = 310.0;
        let pins = if worktrees { ["f4", "a2"] } else { ["chat-0", "chat-1"] };
        settings.sidebar_pins_mut("local".into()).extend(pins.map(String::from));
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
            if std::env::var_os("ZERON_SIDEBAR_ACCOUNT").is_some() {
                s.workspace_scope = Some(zeron_proto::WorkspaceScope::Synced);
                s.auth = Some(zeron_proto::AuthState::SignedIn {
                    user: zeron_proto::UserProfile { id: "fixture-user".into(), email: "alex@example.test".into(), name: Some("Alex".into()) },
                    org_id: Some("fixture-org".into()),
                });
            }
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
            if by_project {
                s.spaces[0].repository_id = Some("github.com/zeronsh/fieldnotes".into());
                s.spaces.push(serde_json::from_value(serde_json::json!({"id":"fieldnotes-remote","deviceId":"remote","path":"/projects/fieldnotes","repositoryId":"github.com/zeronsh/fieldnotes","createdAt":chrono::Utc::now()})).unwrap());
                for ix in [3, 7] {
                    s.chats[ix].space_id = Some("fieldnotes-remote".into());
                }
            }
            s.chats[4].last_message_at = Some(chrono::Utc::now());
            s.chats[8].archived = true;
            if worktrees {
                worktree_dataset(&mut s, &data);
                s.sidebar_preferences.pinned_session_ids = pins.map(String::from).to_vec();
            }
            s
        });
        let boot = EngineBootConfig { data_dir: data, ipc_port: 0, edge_url: String::new(), edge_token: None, org_id: None, workos_client_id: None, default_harness: HarnessId::ClaudeCode };
        let _window = cx.open_window(WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(gpui::point(px(12.),px(30.)), size(px(1100.),px(800.))))),
            titlebar: Some(gpui::TitlebarOptions { title: None, appears_transparent: true, traffic_light_position: Some(gpui::point(px(14.),px(14.))) }),
            app_owns_titlebar_drag: true,
            // Same backdrop as the app window, or the frosted surface has nothing to blur.
            window_background: theme::Theme::of(cx).window_background_appearance(),
            ..Default::default()
        }, |_, cx| cx.new(|cx| shell::Shell::new(state.clone(), boot, cx))).unwrap();
        appearance::reapply_window_background(cx);
        state.update(cx, |_, cx| cx.notify());
        cx.activate(true);
    });
    Ok(())
}

#[derive(Clone, Copy)]
enum Status { Idle, Done, Working(u32), Input, Failed }

fn worktree_dataset(s: &mut state::AppState, root: &std::path::Path) {
    use zeron_proto::ChangeRequestState::{Merged, Open};
    let now = chrono::Utc::now();
    let ago = |minutes: i64| now - chrono::Duration::minutes(minutes);
    for name in ["fieldnotes", "zeron"] {
        std::fs::create_dir_all(root.join(name)).unwrap();
    }
    let path = |name: &str| root.join(name).to_string_lossy().into_owned();
    // (space, device, name, path, repository)
    let spaces = [
        ("fieldnotes", "local", "fieldnotes", path("fieldnotes"), "github.com/zeronsh/fieldnotes"),
        ("fieldnotes-remote", "remote", "fieldnotes", "/projects/fieldnotes".to_string(), "github.com/zeronsh/fieldnotes"),
        ("zeron", "local", "zeron", path("zeron"), "github.com/zeronsh/zeron"),
        ("api", "remote", "API server", "/projects/api".to_string(), "github.com/zeronsh/api"),
    ];
    s.spaces = spaces.iter().map(|(id, device, name, path, repository)| {
        let mut space: zeron_proto::Space = serde_json::from_value(serde_json::json!({"id":id,"deviceId":device,"name":name,"path":path,"createdAt":ago(60 * 24 * 30)})).unwrap();
        space.git_detected = true;
        space.checkout_id = Some(format!("{id}:main"));
        space.repository_id = Some((*repository).into());
        space
    }).collect();
    // A worktree named "main" is its project's own checkout.
    let worktrees: [(&str, &str, &str, Option<(u64, zeron_proto::ChangeRequestState)>, &[(&str, &str, zeron_proto::HarnessId, Status, i64, i64)]); 14] = {
        use zeron_proto::HarnessId::{ClaudeCode as Claude, Codex};
        use Status::*;
        [
            ("fieldnotes", "main", "main", None, &[("f1", "Build the Fieldnotes workspace", Claude, Idle, 43200, 43200)]),
            ("fieldnotes", "review", "fieldnotes/review-pull-request-comments", Some((416, Open)), &[
                ("f2", "Review pull request comments", Claude, Done, 0, 300),
                ("f3", "Answer the ranking question from review", Claude, Working(2), 1, 20),
            ]),
            ("fieldnotes", "palette", "fieldnotes/polish-the-command-palette", Some((413, Open)), &[
                ("f4", "Polish the command palette", Claude, Idle, 17, 400),
                ("f5", "Recent commands above suggestions?", Claude, Input, 9, 12),
            ]),
            ("fieldnotes", "settings", "fieldnotes/build-settings-search", None, &[
                ("f6", "Build settings search", Claude, Idle, 120, 240),
                ("f7", "Update project documentation", Claude, Idle, 60, 180),
            ]),
            ("fieldnotes-remote", "deploy", "fieldnotes/add-deployment-status", None, &[
                ("f8", "Add deployment status", Claude, Idle, 51, 90),
                ("f9", "Refine composer spacing", Codex, Idle, 65, 100),
            ]),
            ("fieldnotes", "export", "fieldnotes/export-markdown", Some((398, Merged)), &[("f10", "Export notes as Markdown", Claude, Idle, 4300, 5000)]),
            ("zeron", "main", "main", None, &[
                ("z1", "Bump version to v0.2.131", Claude, Idle, 190, 200),
                ("z2", "Why does sync drop the last frame?", Codex, Idle, 1460, 1500),
            ]),
            ("zeron", "tree", "zeron/sidebar-tree", Some((1114, Open)), &[
                ("z3", "Group sessions under their worktree", Claude, Working(0), 0, 45),
                ("z4", "Snapshot the sidebar fixture", Claude, Input, 4, 30),
                ("z5", "Review keyboard navigation in the tree", Codex, Done, 38, 40),
            ]),
            ("zeron", "share", "zeron/ios-share-sheet", None, &[
                ("z6", "Share images from the iOS sheet", Codex, Idle, 22, 140),
                ("z7", "Spike: share extension memory limits", Claude, Failed, 95, 120),
            ]),
            ("zeron", "workos", "zeron/workos-orgs", Some((1113, Merged)), &[("z8", "Record every WorkOS organization", Claude, Idle, 300, 560)]),
            ("zeron", "bg", "zeron/bg-process-panel", Some((1092, Merged)), &[("z9", "Show what an agent left running", Claude, Idle, 1500, 1700)]),
            ("api", "main", "main", None, &[("a1", "Improve keyboard navigation", Claude, Idle, 60, 70)]),
            ("api", "auth", "api/fix-authentication-redirects", Some((414, Open)), &[
                ("a2", "Fix authentication redirects", Claude, Idle, 34, 80),
                ("a3", "Audit chat sync", Claude, Idle, 120, 130),
            ]),
            ("api", "rate", "api/rate-limits", None, &[("a4", "Add per-key rate limits", Codex, Working(0), 6, 50)]),
        ]
    };
    let template = s.chats[0].clone();
    s.chats.clear();
    s.sessions.clear();
    for (space_id, name, branch, pr, chats) in worktrees {
        let space = s.spaces.iter().find(|space| space.id == space_id).unwrap().clone();
        let checkout_id = format!("{space_id}:{name}");
        let repo_root = if name == "main" { space.path.clone() } else { format!("{}/.worktrees/{name}", space.path) };
        for &(id, title, harness, status, last_minutes, created_minutes) in chats {
            let mut chat = template.clone();
            chat.id = id.into();
            chat.title = Some(title.into());
            chat.device_id = space.device_id.clone();
            chat.space_id = Some(space.id.clone());
            chat.branch = Some(branch.into());
            chat.cwd = Some(repo_root.clone());
            chat.source_context = Some(zeron_proto::ConversationSourceContext {
                checkout_id: checkout_id.clone(), repo_root: repo_root.clone(), cwd: repo_root.clone(),
                branch: branch.into(), head_sha: None, observed_at: now,
            });
            if let Some(config) = chat.config.as_mut() { config.harness = harness; }
            chat.created_at = ago(created_minutes);
            chat.last_message_at = Some(ago(last_minutes));
            chat.last_seen_at = matches!(status, Status::Idle).then(|| ago(last_minutes) + chrono::Duration::seconds(30));
            chat.archived = id == "a3";
            let session = match status {
                Status::Working(subagents) => Some((zeron_proto::SessionStatus::Working, subagents)),
                Status::Input => Some((zeron_proto::SessionStatus::AwaitingInput, 0)),
                Status::Failed => Some((zeron_proto::SessionStatus::Errored, 0)),
                Status::Idle | Status::Done => None,
            };
            if let Some((status, running_subagents)) = session {
                // Sessions go stale 45s after their last update without an engine; keep the live statuses showing.
                s.sessions.push(zeron_proto::Session {
                    last_turn_outcome: None, terminal_activity: None, last_completed_turn: None,
                    chat_id: chat.id.clone(), device_id: chat.device_id.clone(), status,
                    started_at: Some(ago(last_minutes)), updated_at: now + chrono::Duration::days(365), running_subagents, background_tasks: Vec::new(),
                });
            }
            s.chats.push(chat);
        }
        if let Some((number, state)) = pr {
            s.fixture_sidebar_change_request(zeron_proto::CheckoutChangeRequestStatus {
                checkout_id: checkout_id.clone(), device_id: space.device_id.clone(), cwd: repo_root.clone(), branch: branch.into(), updated_at: now,
                change_request: Some(zeron_proto::ChangeRequestSummary { provider: "github".into(), number, title: chats[0].1.into(), url: format!("https://github.com/zeronsh/{space_id}/pull/{number}"), state, base_ref: "main".into(), head_ref: branch.into() }),
            });
        }
    }
    s.selected_chat = Some("f2".into());
    s.selected_space = Some("fieldnotes".into());
}
