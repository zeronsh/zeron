use super::*;
use gpui::{AppContext, TestAppContext, VisualTestContext};

struct DropHost {
    shell: Entity<Shell>,
    full_shell: bool,
    _data_dir: tempfile::TempDir,
}

impl Render for DropHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
            });
            shell.active_chat = "parent".into();
            shell.splash = SplashPhase::Gone;
            shell.reduced_motion = true;
            shell
        });
        DropHost {
            shell,
            full_shell,
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
    let from = cx.debug_bounds(source).unwrap().center();
    cx.simulate_mouse_down(from, MouseButton::Left, gpui::Modifiers::default());
    cx.simulate_mouse_move(to, Some(MouseButton::Left), gpui::Modifiers::default());
    cx.update(|window, cx| {
        assert!(cx.has_active_drag());
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

/// Real files of each kind a user might drag in from the desktop: an archive,
/// an image, a folder and a file over the size limit.
fn external_files() -> (tempfile::TempDir, gpui::ExternalPaths) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("notes.zip"),
        b"PK\x03\x04 not really zipped",
    )
    .unwrap();
    let mut png = std::io::Cursor::new(Vec::new());
    image::RgbImage::new(1, 1)
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    std::fs::write(dir.path().join("shot.png"), png.into_inner()).unwrap();
    std::fs::create_dir(dir.path().join("folder")).unwrap();
    // Sparse, so the test never writes 24 MB to disk.
    std::fs::File::create(dir.path().join("huge.bin"))
        .unwrap()
        .set_len(crate::attachments::MAX_ATTACHMENT_BYTES + 1)
        .unwrap();
    let paths = ["notes.zip", "shot.png", "folder", "huge.bin"]
        .into_iter()
        .map(|name| dir.path().join(name))
        .collect();
    (dir, gpui::ExternalPaths(paths))
}

/// Drag files in from outside the app and release them at `to` the way the
/// platform reports it: enter, hover, then drop.
fn drop_external(cx: &mut VisualTestContext, paths: gpui::ExternalPaths, to: Point<Pixels>) {
    cx.simulate_event(gpui::FileDropEvent::Entered {
        position: to,
        paths,
    });
    cx.update(|window, cx| {
        assert!(cx.has_active_drag());
        window.draw(cx).clear();
    });
    cx.simulate_event(gpui::FileDropEvent::Pending { position: to });
    cx.update(|window, cx| window.draw(cx).clear());
    cx.simulate_event(gpui::FileDropEvent::Submit { position: to });
    cx.run_until_parked();
    cx.update(|window, cx| {
        assert!(!cx.has_active_drag());
        window.draw(cx).clear();
    });
}

fn staged_names(composer: &Entity<Composer>, cx: &App) -> Vec<String> {
    composer
        .read(cx)
        .staged()
        .iter()
        .map(|att| att.name.clone())
        .collect()
}

/// A mixed drop from outside the app lands in the conversation it is released
/// over, main or side chat: the archive and the image are staged with chips in
/// that draft, and the folder and the oversize file are both named in that
/// composer's error notice. Composers not yet dropped on stay untouched.
#[gpui::test]
fn external_file_drops_stage_files_and_refuse_folders_and_oversize_files(cx: &mut TestAppContext) {
    use zeron_proto::attachment_mentions::attachment_mention_link;
    let (shell, cx) = setup_with_shell(cx, true);
    let (_files, paths) = external_files();
    let targets = [None, Some(2), Some(1)];
    let composer = |shell: &Shell, target: Option<u64>| match target {
        None => shell.composer.clone(),
        Some(id) => shell.side_chats[&id].composer.clone(),
    };
    for (n, target) in targets.into_iter().enumerate() {
        let to = match target {
            None => cx.debug_bounds("chat-dropzone").unwrap().center(),
            Some(id) => cx.update(|window, cx| {
                shell.update(cx, |shell, cx| {
                    shell.activate_right_surface(RightSurface::SideChat(id), window, cx);
                    shell.right_tween = None;
                });
                window.draw(cx).clear();
                let bounds = shell.read(cx).side_chats[&id]
                    .composer
                    .read(cx)
                    .surface_bounds()
                    .get()
                    .unwrap();
                // Over the side chat's transcript, not its input.
                gpui::point(bounds.center().x, px(180.))
            }),
        };
        drop_external(cx, paths.clone(), to);
        shell.read_with(cx, |shell, cx| {
            let dropped = composer(shell, target);
            assert_eq!(
                staged_names(&dropped, cx),
                ["notes.zip", "Image 2.png"],
                "target={target:?}"
            );
            assert_eq!(
                text(&dropped, cx),
                format!(
                    "{} {} ",
                    attachment_mention_link(1, Some("notes.zip")),
                    attachment_mention_link(2, None)
                ),
                "target={target:?}"
            );
            assert_eq!(
                dropped.read(cx).visible_failure().as_deref(),
                Some("folder is not a file. huge.bin is too large (24 MB max)."),
                "target={target:?}"
            );
            for other in targets[n + 1..].iter().map(|other| composer(shell, *other)) {
                assert!(staged_names(&other, cx).is_empty(), "target={target:?}");
                assert!(text(&other, cx).is_empty(), "target={target:?}");
                assert!(other.read(cx).failure().is_none(), "target={target:?}");
            }
        });
    }
}
