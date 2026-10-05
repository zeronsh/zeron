use super::*;
use gpui::{AppContext, TestAppContext, VisualTestContext};

struct DropHost {
    shell: Entity<Shell>,
    full_shell: bool,
    direct_sidebar: bool,
    _data_dir: tempfile::TempDir,
}

impl Render for DropHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Direct rows retain debug bounds between frames; the native fixture
        // exercises the full shell with its cached sidebar.
        if self.direct_sidebar {
            return self.shell.update(cx, |shell, cx| {
                let sidebar = shell.render_chat_sidebar(&Theme::of(cx).clone(), cx);
                let main = shell.render_main(window, 600., 600., cx);
                let right = shell.render_right_pane(window, cx);
                div()
                    .size_full()
                    .flex()
                    .child(
                        div()
                            .w(px(shell.settings.sidebar_width))
                            .h_full()
                            .child(sidebar),
                    )
                    .child(div().w(px(600.)).h_full().child(main))
                    .child(right)
                    .into_any_element()
            });
        }
        if self.full_shell {
            return div()
                .size_full()
                .child(self.shell.clone())
                .into_any_element();
        }
        self.shell.update(cx, |shell, cx| {
            let main = shell.render_main(window, 400., 400., cx);
            let right = shell.render_right_pane(window, cx);
            // Tree and search rows stamp the explorer's current origin; the
            // stale source models a drag that outlived its workspace.
            let origin = shell
                .files
                .get(&shell.panel_key(cx))
                .and_then(|files| files.read(cx).interaction_origin(cx));
            div()
                .size_full()
                .flex()
                .flex_col()
                .child(div().h(px(40.)).child(shell.render_right_tab_strip(cx)))
                // Supply the tree/search payload without needing an engine
                // to enumerate a checkout. Targets and file tabs are real UI.
                .child(
                    div().h(px(24.)).flex().children(
                        [
                            ("file", "src/tree.rs", false, origin.clone()),
                            ("directory", "src", true, origin),
                            ("stale", "src/stale.rs", false, None),
                        ]
                        .into_iter()
                        .map(|(id, path, directory, origin)| {
                            div()
                                .id(id)
                                .debug_selector(move || id.into())
                                .w(px(100.))
                                .h_full()
                                .on_drag(
                                    WorkspacePathDrag::new(path.into(), directory).with_origin(
                                        origin,
                                        crate::files::WorkspacePathSource::Tree,
                                        None,
                                    ),
                                    |payload, _, _, cx| {
                                        crate::files::workspace_path_drag_ghost(payload, cx)
                                    },
                                )
                                .child(id)
                        }),
                    ),
                )
                .child(
                    div()
                        .flex()
                        .flex_1()
                        .min_h_0()
                        .child(div().w(px(400.)).h_full().child(main))
                        .child(right),
                )
                .into_any_element()
        })
    }
}

fn setup(cx: &mut TestAppContext) -> (Entity<Shell>, &mut VisualTestContext) {
    setup_with_shell(cx, false)
}

fn setup_with_shell(
    cx: &mut TestAppContext,
    full_shell: bool,
) -> (Entity<Shell>, &mut VisualTestContext) {
    setup_options(cx, full_shell, false)
}

fn setup_options(
    cx: &mut TestAppContext,
    full_shell: bool,
    direct_sidebar: bool,
) -> (Entity<Shell>, &mut VisualTestContext) {
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
    });
    let (host, cx) = cx.add_window_view(|_, cx| {
        let shell = cx.new(|cx| {
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
            // This fixture has no engine. Keep state notifications from
            // invoking the shell's disconnect cleanup of all side chats.
            shell._state_observation = cx.observe(&shell.state, |_, _, _| {});
            shell.state.update(cx, |state, _| {
                state.workspace_scope = Some(WorkspaceScope::Local);
                state.connection = ConnectionStatus::Ready;
                state.local_device_id = Some("local".into());
                state.no_project = true;
                state.chats = vec![
                    serde_json::from_value(serde_json::json!({
                        "id": "parent", "deviceId": "local", "archived": false,
                        "createdAt": Utc::now(),
                    }))
                    .unwrap(),
                ];
                state.selected_chat = Some("parent".into());
                let mut reference = state.chats[0].clone();
                reference.id = "reference".into();
                reference.title = Some("Authentication design".into());
                state.chats.push(reference);
            });
            shell.settings.sidebar_organization = SidebarOrganization::InOneList;
            shell.settings.sidebar_collapsed = false;
            shell.active_chat = "parent".into();
            shell.splash = SplashPhase::Gone;
            shell.reduced_motion = true;
            shell
        });
        DropHost {
            shell,
            full_shell,
            direct_sidebar,
            _data_dir: dir,
        }
    });
    let shell = host.read_with(cx, |host, _| host.shell.clone());
    cx.update(|window, cx| window.draw(cx).clear());
    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            shell.add_file_surface("src/tab.rs".into(), window, cx);
            let explorer = cx.new(|cx| {
                crate::files::FilesSurface::new_explorer(
                    shell.state.clone(),
                    shell.active_chat.clone(),
                    false,
                    cx,
                )
            });
            shell.files.insert(shell.panel_key(cx), explorer);
            let mut side = shell.state.read(cx).chats[0].clone();
            side.id = "saved-side".into();
            side.parent_chat_id = Some("parent".into());
            shell.open_side_chat(side, shell.panel_key(cx), cx);
            shell.create_child_chat(None, cx);
            assert_eq!(shell.side_chat_seq, 2);
            shell.right_tween = None;
        });
    });
    cx.update(|window, cx| window.draw(cx).clear());
    (shell, cx)
}

#[gpui::test]
fn full_shell_file_tab_drops_reach_side_chat_body(cx: &mut TestAppContext) {
    let (shell, cx) = setup_with_shell(cx, true);
    for expanded in [false, true] {
        cx.update(|window, cx| {
            shell.update(cx, |shell, cx| {
                shell.right_pane_expanded = expanded;
                shell.right_tween = None;
                cx.notify();
            });
            window.draw(cx).clear();
        });
        for over_input in [false, true] {
            let to = cx.update(|_, cx| {
                let bounds = shell.read(cx).side_chats[&2]
                    .composer
                    .read(cx)
                    .surface_bounds()
                    .get()
                    .unwrap();
                if over_input {
                    bounds.center()
                } else {
                    gpui::point(bounds.center().x, px(180.))
                }
            });
            let before = shell.read_with(cx, |shell, cx| text(&shell.side_chats[&2].composer, cx));
            drag(cx, "right-surface-tab-0", to);
            shell.read_with(cx, |shell, cx| {
                let after = text(&shell.side_chats[&2].composer, cx);
                assert_eq!(
                    after.matches("zeron-file:src/tab.rs").count(),
                    before.matches("zeron-file:src/tab.rs").count() + 1,
                    "drop failed: expanded={expanded}, over_input={over_input}"
                );
                assert!(text(&shell.composer, cx).is_empty());
            });
        }
    }
}

fn drag(cx: &mut VisualTestContext, source: &'static str, to: Point<Pixels>) {
    let from = cx
        .debug_bounds(source)
        .unwrap_or_else(|| panic!("missing drag source {source}"))
        .center();
    cx.simulate_mouse_down(from, MouseButton::Left, gpui::Modifiers::default());
    cx.simulate_mouse_move(to, Some(MouseButton::Left), gpui::Modifiers::default());
    cx.update(|window, cx| {
        assert!(cx.has_active_drag(), "drag did not start from {source}");
        window.draw(cx).clear();
    });
    cx.simulate_mouse_move(to, Some(MouseButton::Left), gpui::Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear());
    cx.simulate_mouse_up(to, MouseButton::Left, gpui::Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear());
}

fn text(composer: &Entity<Composer>, cx: &App) -> String {
    composer.read(cx).input.read(cx).text().to_owned()
}

fn drag_chat(cx: &mut VisualTestContext, source: &'static str, to: Point<Pixels>) {
    cx.update(|window, cx| window.draw(cx).clear());
    let from = cx.debug_bounds(source).unwrap().center();
    cx.simulate_mouse_move(from, None, gpui::Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear());
    cx.simulate_mouse_down(from, MouseButton::Left, gpui::Modifiers::default());
    cx.simulate_mouse_move(
        from + gpui::point(px(8.), px(0.)),
        Some(MouseButton::Left),
        gpui::Modifiers::default(),
    );
    cx.update(|window, cx| {
        assert!(cx.has_active_drag(), "{source}");
        window.draw(cx).clear();
    });
    cx.simulate_mouse_move(to, Some(MouseButton::Left), gpui::Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear());
    cx.simulate_mouse_move(to, Some(MouseButton::Left), gpui::Modifiers::default());
    cx.simulate_mouse_up(to, MouseButton::Left, gpui::Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear());
}

#[gpui::test]
fn sidebar_chat_drops_add_context_to_the_target_pane_once(cx: &mut TestAppContext) {
    let (shell, cx) = setup_options(cx, false, true);
    cx.simulate_resize(gpui::size(px(1400.), px(900.)));
    cx.update(|window, cx| window.draw(cx).clear());
    let pins = shell.read_with(cx, |shell, cx| shell.active_sidebar_pins(cx));
    let selected = shell.read_with(cx, |shell, cx| shell.state.read(cx).selected_chat.clone());
    let to = cx.debug_bounds("chat-dropzone").unwrap().center();
    drag_chat(cx, "chat-reference", to);
    drag_chat(cx, "chat-reference", to);
    shell.read_with(cx, |shell, cx| {
        let raw = text(&shell.composer, cx);
        let links = zeron_proto::chat_mentions::chat_mention_links(&raw);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].1.chat_id, "reference");
        assert_eq!(links[0].1.title, "Authentication design");
        assert_eq!(shell.active_sidebar_pins(cx), pins);
        assert_eq!(shell.state.read(cx).selected_chat, selected);
        assert!(shell.sidebar_session_transfer.is_none());
        assert!(text(&shell.side_chats[&2].composer, cx).is_empty());
    });
    // Main self-drops are ignored, including drops over the composer itself.
    let before = shell.read_with(cx, |shell, cx| text(&shell.composer, cx));
    drag_chat(cx, "chat-parent", to);
    shell.read_with(cx, |shell, cx| {
        assert_eq!(text(&shell.composer, cx), before)
    });
    for id in [1, 2] {
        let to = cx.update(|window, cx| {
            shell.update(cx, |shell, cx| {
                shell.activate_right_surface(RightSurface::SideChat(id), window, cx)
            });
            window.draw(cx).clear();
            shell.read(cx).side_chats[&id]
                .composer
                .read(cx)
                .surface_bounds()
                .get()
                .unwrap()
                .center()
        });
        drag_chat(cx, "chat-reference", to);
        shell.read_with(cx, |shell, cx| {
            let raw = text(&shell.side_chats[&id].composer, cx);
            assert_eq!(
                zeron_proto::chat_mentions::chat_mention_links(&raw)[0]
                    .1
                    .chat_id,
                "reference"
            );
            assert_eq!(text(&shell.composer, cx), before);
        });
    }
}

#[gpui::test]
fn workspace_drops_target_saved_and_unsaved_side_chat_composers(cx: &mut TestAppContext) {
    let (shell, cx) = setup(cx);
    for (id, source, path, directory) in [
        (2, "file", "src/tree.rs", false),
        (1, "directory", "src", true),
    ] {
        let to = cx.update(|window, cx| {
            shell.update(cx, |shell, cx| {
                shell.activate_right_surface(RightSurface::SideChat(id), window, cx);
                window.focus(&shell.composer.focus_handle(cx), cx);
            });
            window.draw(cx).clear();
            shell.read(cx).side_chats[&id]
                .composer
                .read(cx)
                .surface_bounds()
                .get()
                .unwrap()
                .center()
        });
        drag(cx, source, to);
        cx.update(|window, cx| {
            let shell = shell.read(cx);
            let composer = &shell.side_chats[&id].composer;
            assert_eq!(
                text(composer, cx),
                format!(
                    "{} ",
                    zeron_proto::file_mentions::local_file_link(path, directory)
                )
            );
            assert!(composer.focus_handle(cx).is_focused(window));
            assert!(text(&shell.composer, cx).is_empty());
            if id == 2 {
                assert!(text(&shell.side_chats[&1].composer, cx).is_empty());
                assert!(shell.side_chats[&2].state.read(cx).side_chat_unsaved());
            } else {
                assert!(text(&shell.side_chats[&2].composer, cx).contains("src/tree.rs"));
            }
        });
    }
    // A payload without a live origin attaches to neither side nor main chats.
    let side = cx.update(|_, cx| {
        shell.read(cx).side_chats[&1]
            .composer
            .read(cx)
            .surface_bounds()
            .get()
            .unwrap()
            .center()
    });
    drag(cx, "stale", side);
    drag(cx, "stale", gpui::point(px(200.), px(150.)));
    shell.read_with(cx, |shell, cx| {
        assert!(!text(&shell.side_chats[&1].composer, cx).contains("stale.rs"));
        assert!(text(&shell.composer, cx).is_empty());
    });
}

#[gpui::test]
fn file_tab_drops_attach_to_side_chat_transcript_and_main_without_reordering(
    cx: &mut TestAppContext,
) {
    let (shell, cx) = setup(cx);
    let original_tabs = shell.read_with(cx, |shell, cx| {
        shell.right_tabs[&shell.panel_key(cx)].clone()
    });
    let to = cx.update(|_, cx| {
        let bounds = shell.read(cx).side_chats[&2]
            .composer
            .read(cx)
            .surface_bounds()
            .get()
            .unwrap();
        gpui::point(bounds.center().x, px(150.))
    });
    drag(cx, "right-surface-tab-0", to);
    shell.read_with(cx, |shell, cx| {
        assert!(text(&shell.side_chats[&2].composer, cx).contains("zeron-file:src/tab.rs"));
        assert!(text(&shell.side_chats[&1].composer, cx).is_empty());
        assert!(text(&shell.composer, cx).is_empty());
        assert_eq!(shell.resolved_right_active(cx), RightSurface::SideChat(2));
        assert_eq!(shell.right_tabs[&shell.panel_key(cx)], original_tabs);
    });
    drag(cx, "right-surface-tab-0", gpui::point(px(200.), px(150.)));
    shell.read_with(cx, |shell, cx| {
        assert!(text(&shell.composer, cx).contains("zeron-file:src/tab.rs"));
        assert_eq!(shell.right_tabs[&shell.panel_key(cx)], original_tabs);
    });
    // Chat tabs have no workspace path and must not insert a mention.
    let before = shell.read_with(cx, |shell, cx| text(&shell.side_chats[&2].composer, cx));
    drag(cx, "right-surface-tab-1", to);
    shell.read_with(cx, |shell, cx| {
        assert_eq!(text(&shell.side_chats[&2].composer, cx), before);
        assert_eq!(shell.right_tabs[&shell.panel_key(cx)], original_tabs);
    });
}
