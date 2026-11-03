//! Inline chat renames on the sidebar row, without booting an engine.
use super::*;
use gpui::{AppContext, Modifiers, TestAppContext, VisualTestContext};

struct SidebarHost(Entity<Shell>);

impl Render for SidebarHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.0.update(cx, |shell, cx| {
            shell.focus_rename_chat(window, cx);
            div()
                .w(px(280.0))
                .h(px(800.0))
                .capture_key_down(cx.listener(Shell::on_key_down_capture))
                .child(shell.render_chat_sidebar(&Theme::default(), cx))
        })
    }
}

fn setup(cx: &mut TestAppContext) -> (Entity<Shell>, &mut VisualTestContext) {
    let dir = tempfile::tempdir().unwrap();
    cx.update(|cx| {
        gpui_base::init(cx);
        cx.set_global(Theme::default());
        crate::app_menus::init(cx);
        settings::init(UiSettings::default(), dir.path(), cx);
        crate::history::init(
            Default::default(),
            Default::default(),
            Default::default(),
            Default::default(),
            cx,
        );
        apply_keymap(
            cx,
            &KeymapConfig::default(),
            ComposerSendBehavior::default(),
        );
    });
    let (host, cx) = cx.add_window_view(|_, cx| {
        SidebarHost(cx.new(|cx| {
            let state = cx.new(|_| AppState::new());
            let mut shell = Shell::new(
                state,
                EngineBootConfig {
                    data_dir: dir.path().into(),
                    ipc_port: 0,
                    edge_url: "http://127.0.0.1:1".into(),
                    edge_token: None,
                    org_id: None,
                    workos_client_id: None,
                    default_harness: zeron_proto::HarnessId::Mock,
                },
                cx,
            );
            shell.settings.sidebar_organization = SidebarOrganization::InOneList;
            shell.reduced_motion = true;
            shell.state.update(cx, |state, _| {
                state.workspace_scope = Some(WorkspaceScope::Local);
                state.local_device_id = Some("local".into());
                state.no_project = true;
                state.chats = ["older", "newer"]
                    .into_iter()
                    .enumerate()
                    .map(|(ix, id)| {
                        serde_json::from_value(serde_json::json!({
                            "id": id, "title": id, "deviceId": "local", "archived": false,
                            "createdAt": Utc::now() - chrono::Duration::minutes(10 - ix as i64),
                        }))
                        .unwrap()
                    })
                    .collect();
            });
            shell
        }))
    });
    let shell = host.read_with(cx, |host, _| host.0.clone());
    cx.update(|window, cx| {
        window.activate_window();
        window.draw(cx).clear();
    });
    (shell, cx)
}

fn redraw(cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        window.refresh();
        window.draw(cx).clear();
    });
}

fn double_click(cx: &mut VisualTestContext, position: Point<Pixels>) {
    for click_count in [1, 2] {
        cx.simulate_event(MouseDownEvent {
            button: MouseButton::Left,
            position,
            click_count,
            ..Default::default()
        });
        cx.simulate_event(MouseUpEvent {
            button: MouseButton::Left,
            position,
            click_count,
            ..Default::default()
        });
    }
    redraw(cx);
}

/// A rename that reached `mutate` reports the missing engine on the sidebar.
fn mutated(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> bool {
    shell.read_with(cx, |shell, _| shell.sidebar_notice.is_some())
}

#[gpui::test]
fn double_click_edits_a_sidebar_title_in_place(cx: &mut TestAppContext) {
    let (shell, cx) = setup(cx);
    let row = cx.debug_bounds("chat-older").unwrap();
    double_click(cx, row.center());

    // The title swaps for a focused field holding the whole name, selected;
    // no modal opens.
    assert!(cx.debug_bounds("chat-title-older").is_none());
    let field = cx.debug_bounds("chat-title-editor-older").unwrap();
    assert!(row.contains(&field.center()));
    let input = shell.read_with(cx, |shell, _| {
        let rename = shell.chat_rename.as_ref().unwrap();
        assert_eq!(rename.chat_id, "older");
        assert_eq!(rename.surface, ChatRenameSurface::Sidebar);
        rename.input.clone()
    });
    cx.update(|window, cx| {
        assert!(input.focus_handle(cx).is_focused(window));
        assert_eq!(input.read(cx).text(), "older");
    });

    // Clicking inside the field keeps editing instead of activating the row.
    cx.simulate_click(field.center(), Modifiers::default());
    redraw(cx);
    assert!(shell.read_with(cx, |shell, _| shell.chat_rename.is_some()));

    // Escape drops the edit without renaming.
    cx.simulate_keystrokes("escape");
    redraw(cx);
    assert!(shell.read_with(cx, |shell, _| shell.chat_rename.is_none()));
    assert!(cx.debug_bounds("chat-title-older").is_some());
    assert!(!mutated(&shell, cx));

    // Enter on an unchanged title closes without a mutation.
    double_click(cx, row.center());
    cx.simulate_keystrokes("enter");
    redraw(cx);
    assert!(shell.read_with(cx, |shell, _| shell.chat_rename.is_none()));
    assert!(!mutated(&shell, cx));

    // Typing replaces the selected name; Enter commits it.
    double_click(cx, row.center());
    cx.simulate_input("Renamed");
    let input = shell.read_with(cx, |shell, _| {
        shell.chat_rename.as_ref().unwrap().input.clone()
    });
    assert_eq!(
        input.read_with(cx, |input, _| input.text().to_string()),
        "Renamed"
    );
    cx.simulate_keystrokes("enter");
    redraw(cx);
    assert!(shell.read_with(cx, |shell, _| shell.chat_rename.is_none()));
    assert!(mutated(&shell, cx));
}

#[gpui::test]
fn menu_rename_opens_the_same_field_and_blur_commits(cx: &mut TestAppContext) {
    let (shell, cx) = setup(cx);
    shell.update(cx, |shell, cx| shell.open_rename_chat("newer".into(), cx));
    redraw(cx);
    assert!(cx.debug_bounds("chat-title-editor-newer").is_some());
    assert!(cx.debug_bounds("chat-title-editor-older").is_none());

    cx.simulate_input("Moved on");
    // Focus leaving the field saves the edit.
    cx.update(|window, _| window.blur());
    redraw(cx);
    assert!(shell.read_with(cx, |shell, _| shell.chat_rename.is_none()));
    assert!(mutated(&shell, cx));
}

/// Open a rename the way `/rename` does — without touching the row.
fn rename(shell: &Entity<Shell>, cx: &mut VisualTestContext, chat_id: &str) {
    shell.update(cx, |shell, cx| shell.open_rename_chat(chat_id.into(), cx));
    redraw(cx);
}

fn editing(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Option<String> {
    shell.read_with(cx, |shell, _| {
        shell
            .chat_rename
            .as_ref()
            .map(|rename| rename.chat_id.clone())
    })
}

fn add_chats(shell: &Entity<Shell>, cx: &mut VisualTestContext, ids: &[&str], archived: bool) {
    shell.update(cx, |shell, cx| {
        shell.state.update(cx, |state, _| {
            for (ix, id) in ids.iter().enumerate() {
                state.chats.push(
                    serde_json::from_value(serde_json::json!({
                        "id": id, "title": id, "deviceId": "local", "archived": archived,
                        "createdAt": Utc::now() - chrono::Duration::minutes(30 + ix as i64),
                    }))
                    .unwrap(),
                );
            }
        });
        cx.notify();
    });
    redraw(cx);
}

#[gpui::test]
fn rename_opens_a_collapsed_pinned_section_with_its_motion(cx: &mut TestAppContext) {
    let (shell, cx) = setup(cx);
    shell.update(cx, |shell, cx| {
        shell.set_chat_pinned("older".into(), true, cx);
        shell.pinned_open = false;
        cx.notify();
    });
    redraw(cx);

    rename(&shell, cx, "older");
    assert_eq!(editing(&shell, cx).as_deref(), Some("older"));
    shell.read_with(cx, |shell, _| {
        assert!(shell.pinned_open);
        // The queued reveal became the disclosure's opening motion.
        assert!(shell.sidebar_reveal_motions.is_empty());
        let motion = shell.sidebar_disclosure_motion["pinned"];
        assert_eq!(motion.from, 0.0);
        assert!(motion.to > 0.0);
    });
    assert!(cx.debug_bounds("chat-title-editor-older").is_some());
}

#[gpui::test]
fn rename_opens_collapsed_sessions_groups_and_the_sidebar(cx: &mut TestAppContext) {
    let (shell, cx) = setup(cx);
    shell.update(cx, |shell, cx| {
        shell.sessions_open = false;
        shell.settings.sidebar_collapsed = true;
        cx.notify();
    });
    redraw(cx);
    rename(&shell, cx, "newer");
    shell.read_with(cx, |shell, _| {
        assert!(shell.sessions_open);
        assert!(!shell.settings.sidebar_collapsed);
    });
    assert_eq!(editing(&shell, cx).as_deref(), Some("newer"));

    // Grouped by project: the chat's collapsed group opens.
    shell.update(cx, |shell, cx| {
        shell.finish_rename_chat(false, cx);
        shell.settings.sidebar_organization = SidebarOrganization::ByProject;
        shell
            .sidebar_collapsed_groups
            .insert("project:home:local".into());
        cx.notify();
    });
    redraw(cx);
    rename(&shell, cx, "older");
    shell.read_with(cx, |shell, _| {
        assert!(shell.sidebar_collapsed_groups.is_empty());
        assert!(shell.sidebar_reveal_motions.is_empty());
    });
    assert!(cx.debug_bounds("chat-title-editor-older").is_some());
}

#[gpui::test]
fn rename_pages_the_archived_shelf_to_its_row(cx: &mut TestAppContext) {
    let (shell, cx) = setup(cx);
    let ids: Vec<String> = (0..14).map(|ix| format!("archived-{ix:02}")).collect();
    add_chats(
        &shell,
        cx,
        &ids.iter().map(String::as_str).collect::<Vec<_>>(),
        true,
    );
    shell.update(cx, |shell, cx| {
        shell.archived_open = false;
        cx.notify();
    });
    redraw(cx);

    // Oldest last: the 13th row is past the first page of ten.
    rename(&shell, cx, "archived-12");
    shell.read_with(cx, |shell, _| {
        assert!(shell.archived_open);
        assert_eq!(shell.archived_shown, 35);
    });
    assert!(cx.debug_bounds("chat-title-editor-archived-12").is_some());
}

#[gpui::test]
fn rename_scrolls_a_far_row_into_view(cx: &mut TestAppContext) {
    let (shell, cx) = setup(cx);
    let ids: Vec<String> = (0..40).map(|ix| format!("chat-{ix:02}")).collect();
    add_chats(
        &shell,
        cx,
        &ids.iter().map(String::as_str).collect::<Vec<_>>(),
        false,
    );
    rename(&shell, cx, "chat-39");
    for _ in 0..4 {
        cx.run_until_parked();
        redraw(cx);
    }
    let field = cx.debug_bounds("chat-title-editor-chat-39").unwrap();
    let viewport = shell.read_with(cx, |shell, _| shell.sidebar_scroll.bounds());
    assert!(shell.read_with(cx, |shell, _| shell.sidebar_scroll.offset().y < px(0.0)));
    assert!(field.top() >= viewport.top() && field.bottom() <= viewport.bottom());
}

#[gpui::test]
fn rename_refuses_a_row_the_project_filter_hides(cx: &mut TestAppContext) {
    let (shell, cx) = setup(cx);
    shell.update(cx, |shell, cx| {
        shell.settings.space_filter = Some("elsewhere".into());
        cx.notify();
    });
    redraw(cx);
    rename(&shell, cx, "older");
    assert_eq!(editing(&shell, cx), None);
    // The filter is the user's: it stays, and the sidebar says why.
    shell.read_with(cx, |shell, _| {
        assert_eq!(shell.settings.space_filter.as_deref(), Some("elsewhere"));
        assert!(shell.sidebar_notice.is_some());
    });
}

#[gpui::test]
fn rename_expands_a_collapsed_custom_section(cx: &mut TestAppContext) {
    use zeron_proto::SidebarSectionChange;

    let (shell, cx) = setup(cx);
    shell.update(cx, |shell, cx| {
        for change in [
            SidebarSectionChange::Create {
                id: "work".into(),
                name: "Work".into(),
            },
            SidebarSectionChange::Assign {
                session_id: "older".into(),
                section_id: Some("work".into()),
                after: None,
                before: None,
            },
            SidebarSectionChange::Collapse {
                id: "work".into(),
                collapsed: true,
            },
        ] {
            assert!(shell.change_sidebar_section(change, cx));
        }
    });
    redraw(cx);

    rename(&shell, cx, "older");
    shell.update(cx, |shell, cx| {
        let sections = shell.active_sidebar_sections(cx);
        assert!(!sections[0].collapsed);
        assert!(shell.sidebar_reveal_motions.is_empty());
        assert!(shell.sidebar_disclosure_motion.contains_key("custom:work"));
    });
    assert!(cx.debug_bounds("chat-title-editor-older").is_some());
}

#[gpui::test]
fn inline_rename_keeps_the_row_height_in_compact_and_full_rows(cx: &mut TestAppContext) {
    let (shell, cx) = setup(cx);
    for compact in [true, false] {
        shell.update(cx, |shell, cx| {
            shell.settings.sidebar_compact = compact;
            cx.notify();
        });
        redraw(cx);
        let row = cx.debug_bounds("chat-older").unwrap();
        double_click(cx, row.center());

        // The field takes the title's place without growing the row.
        let field = cx.debug_bounds("chat-title-editor-older").unwrap();
        assert!(row.contains(&field.center()), "compact: {compact}");
        assert_eq!(
            cx.debug_bounds("chat-older").unwrap().size.height,
            row.size.height,
            "compact: {compact}"
        );

        cx.simulate_keystrokes("escape");
        redraw(cx);
        assert!(shell.read_with(cx, |shell, _| shell.chat_rename.is_none()));
    }
}
