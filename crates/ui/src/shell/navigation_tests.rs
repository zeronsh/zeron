//! Real pane rendering and key dispatch without booting an engine.
use super::*;
use gpui::{AppContext, TestAppContext, VisualTestContext};

struct NavigationHost {
    shell: Entity<Shell>,
    _data_dir: tempfile::TempDir,
}

impl Render for NavigationHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.shell.update(cx, |shell, cx| {
            shell
                .navigation_focus
                .remember(&shell.shortcut_focus, window, cx);
            let root = shell.shortcut_focus.clone();
            let (preferred, neutral) = shell.navigation_focus_fallback(cx);
            window.defer(cx, move |window, cx| {
                restore_mounted_focus(&root, &preferred, &neutral, window, cx);
            });
            let main = shell.render_main(window, 400., 400., cx);
            let right = shell.render_right_pane(window, cx);
            div()
                .size_full()
                .flex()
                .flex_col()
                .track_focus(&shell.shortcut_focus)
                .child(div().track_focus(&shell.unfocused))
                .on_action(cx.listener(|shell, _: &NextSession, window, cx| {
                    shell.cycle_navigation(true, window, cx);
                }))
                .on_action(cx.listener(|shell, _: &PrevSession, window, cx| {
                    shell.cycle_navigation(false, window, cx);
                }))
                .child(div().h(px(40.)).child(shell.render_right_tab_strip(cx)))
                .child(
                    div()
                        .flex()
                        .flex_1()
                        .min_h_0()
                        .child(
                            div()
                                .id("navigation-main")
                                .debug_selector(|| "navigation-main".into())
                                .w(px(400.))
                                .h_full()
                                .child(main),
                        )
                        .when(shell.right_pane_open(cx), |el| el.child(right)),
                )
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
            shell.settings.sidebar_organization = SidebarOrganization::InOneList;
            shell.state.update(cx, |state, _| {
                state.workspace_scope = Some(WorkspaceScope::Local);
                state.local_device_id = Some("local".into());
                state.no_project = true;
                state.chats = ["parent", "other"]
                    .into_iter()
                    .enumerate()
                    .map(|(ix, id)| {
                        serde_json::from_value(serde_json::json!({
                            "id": id, "title": id, "deviceId": "local", "archived": false,
                            "createdAt": Utc::now() - chrono::Duration::minutes(ix as i64),
                        }))
                        .unwrap()
                    })
                    .collect();
                state.selected_chat = Some("parent".into());
            });
            shell.active_chat = "parent".into();
            shell.reduced_motion = true;
            for doc in ["first", "second"] {
                shell.add_subagent_surface("parent".into(), doc.into(), doc.into(), false, cx);
            }
            shell
                .composer
                .update(cx, |composer, _| composer.focus_pending = false);
            shell
        });
        NavigationHost {
            shell,
            _data_dir: dir,
        }
    });
    let shell = host.read_with(cx, |host, _| host.shell.clone());
    cx.update(|window, cx| window.draw(cx).clear());
    (shell, cx)
}

fn press(cx: &mut VisualTestContext, forward: bool) {
    let id = if forward {
        ShortcutId::NextSession
    } else {
        ShortcutId::PrevSession
    };
    cx.simulate_keystrokes(&platform_combo(id.default_combo()));
    cx.update(|window, cx| window.draw(cx).clear());
}

fn assert_surface(shell: &Entity<Shell>, cx: &mut VisualTestContext, surface: RightSurface) {
    cx.update(|window, cx| {
        let shell = shell.read(cx);
        assert_eq!(shell.resolved_right_active(cx), surface);
        assert_eq!(
            shell.state.read(cx).selected_chat.as_deref(),
            Some("parent")
        );
        assert!(shell.navigation_focus.in_right(window, cx));
    });
}

#[gpui::test]
fn repeated_keys_stay_in_read_only_pane_and_follow_reordering(cx: &mut TestAppContext) {
    let (shell, cx) = setup(cx);
    let tab = cx.debug_bounds("right-surface-tab-0").unwrap().center();
    cx.simulate_click(tab, gpui::Modifiers::default());
    for expected in [2, 1, 2, 1] {
        press(cx, true);
        assert_surface(&shell, cx, RightSurface::Subagent(expected));
    }
    shell.update(cx, |shell, cx| {
        shell.add_subagent_surface("parent".into(), "third".into(), "Third".into(), false, cx);
        shell.reorder_right_tabs(2, 0, cx);
    });
    press(cx, true);
    assert_surface(&shell, cx, RightSurface::Subagent(1));
    press(cx, false);
    assert_surface(&shell, cx, RightSurface::Subagent(3));
    press(cx, false);
    assert_surface(&shell, cx, RightSurface::Subagent(2));
}

#[gpui::test]
fn clicking_main_transcript_switches_back_to_session_navigation(cx: &mut TestAppContext) {
    let (shell, cx) = setup(cx);
    let tab = cx.debug_bounds("right-surface-tab-0").unwrap().center();
    cx.simulate_click(tab, gpui::Modifiers::default());
    press(cx, true);
    let main = cx.debug_bounds("navigation-main").unwrap();
    cx.simulate_click(
        main.origin + gpui::point(px(100.), px(150.)),
        gpui::Modifiers::default(),
    );
    press(cx, true);
    shell.read_with(cx, |shell, cx| {
        assert_eq!(shell.state.read(cx).selected_chat.as_deref(), Some("other"));
    });
}

#[gpui::test]
fn side_chat_to_diff_retains_right_focus_and_custom_bindings(cx: &mut TestAppContext) {
    let (shell, cx) = setup(cx);
    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            let chat = serde_json::from_value(serde_json::json!({
                "id": "side", "parentChatId": "parent", "deviceId": "local",
                "archived": false, "createdAt": Utc::now(),
            }))
            .unwrap();
            shell.open_side_chat(chat, shell.panel_key(cx), cx);
            shell.add_diff_surface(window, cx);
            shell.activate_right_surface(RightSurface::SideChat(1), window, cx);
        })
    });
    cx.update(|window, cx| window.draw(cx).clear());
    cx.update(|window, cx| {
        assert!(
            shell.read(cx).side_chats[&1]
                .composer
                .focus_handle(cx)
                .is_focused(window)
        );
        let mut keymap = KeymapConfig::default();
        keymap.next_session = "alt-down".into();
        keymap.prev_session = "alt-up".into();
        apply_keymap(cx, &keymap, ComposerSendBehavior::default());
    });
    cx.simulate_keystrokes("alt-down");
    cx.update(|window, cx| window.draw(cx).clear());
    assert_surface(&shell, cx, RightSurface::Diff(1));
    cx.simulate_keystrokes("alt-down");
    cx.update(|window, cx| window.draw(cx).clear());
    assert_surface(&shell, cx, RightSurface::Subagent(1));
    cx.simulate_keystrokes("alt-up");
    cx.update(|window, cx| window.draw(cx).clear());
    assert_surface(&shell, cx, RightSurface::Diff(1));
    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            shell.activate_right_surface(RightSurface::SideChat(1), window, cx);
        })
    });
    cx.update(|window, cx| window.draw(cx).clear());
    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            shell.close_right_surface(RightSurface::SideChat(1), window, cx);
        })
    });
    cx.update(|window, cx| window.draw(cx).clear());
    // Closing returns to the most recently visited tab, then cycles in strip order.
    assert_surface(&shell, cx, RightSurface::Diff(1));
    cx.simulate_keystrokes("alt-down");
    cx.update(|window, cx| window.draw(cx).clear());
    assert_surface(&shell, cx, RightSurface::Subagent(1));
}

#[gpui::test]
fn closing_active_and_last_tabs_recovers_the_correct_pane(cx: &mut TestAppContext) {
    let (shell, cx) = setup(cx);
    let tab = cx.debug_bounds("right-surface-tab-0").unwrap().center();
    cx.simulate_click(tab, gpui::Modifiers::default());
    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            shell.close_right_surface(RightSurface::Subagent(1), window, cx);
        })
    });
    press(cx, true);
    assert_surface(&shell, cx, RightSurface::Subagent(2));
    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            shell.close_right_surface(RightSurface::Subagent(2), window, cx);
            assert!(!shell.right_pane_open(cx));
            assert!(shell.composer.focus_handle(cx).is_focused(window));
        })
    });
    press(cx, true);
    shell.read_with(cx, |shell, cx| {
        assert_eq!(shell.state.read(cx).selected_chat.as_deref(), Some("other"));
    });
}

#[gpui::test]
fn modal_and_picker_block_navigation_in_both_panes(cx: &mut TestAppContext) {
    let (shell, cx) = setup(cx);
    for right in [false, true] {
        cx.update(|window, cx| {
            shell.update(cx, |shell, cx| {
                let focus = if right {
                    &shell.navigation_focus.right
                } else {
                    &shell.navigation_focus.main
                };
                window.focus(focus, cx);
                shell.delete_confirm = Some("parent".into());
            })
        });
        press(cx, true);
        press(cx, false);
        shell.update(cx, |shell, cx| {
            assert_eq!(
                shell.state.read(cx).selected_chat.as_deref(),
                Some("parent")
            );
            assert_eq!(shell.resolved_right_active(cx), RightSurface::Subagent(2));
            shell.delete_confirm = None;
        });
    }
    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            shell
                .composer
                .update(cx, |composer, cx| composer.open_model_menu(window, cx));
        })
    });
    press(cx, true);
    shell.read_with(cx, |shell, cx| {
        assert_eq!(
            shell.state.read(cx).selected_chat.as_deref(),
            Some("parent")
        );
        assert_eq!(shell.resolved_right_active(cx), RightSurface::Subagent(2));
    });
}

#[gpui::test]
fn empty_picker_and_stale_tabs_do_not_change_sessions(cx: &mut TestAppContext) {
    let (shell, cx) = setup(cx);
    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            let key = shell.panel_key(cx);
            shell.right_tabs.insert(key, vec![RightSurface::File(999)]);
            shell.set_right_active(RightSurface::Picker, cx);
            window.focus(&shell.navigation_focus.right, cx);
        })
    });
    press(cx, true);
    press(cx, false);
    assert_surface(&shell, cx, RightSurface::Picker);
}

#[gpui::test]
fn hiding_reopening_and_expanding_preserve_navigation_boundaries(cx: &mut TestAppContext) {
    let (shell, cx) = setup(cx);
    let tab = cx.debug_bounds("right-surface-tab-0").unwrap().center();
    cx.simulate_click(tab, gpui::Modifiers::default());
    shell.update(cx, |shell, cx| shell.toggle_right_pane_expand(cx));
    press(cx, true);
    assert_surface(&shell, cx, RightSurface::Subagent(2));
    shell.update(cx, |shell, cx| shell.set_surfaces_open(false, cx));
    cx.update(|window, cx| window.draw(cx).clear());
    cx.update(|window, cx| {
        assert!(!shell.read(cx).navigation_focus.in_right(window, cx));
        assert!(shell.read(cx).composer.focus_handle(cx).is_focused(window));
    });
    shell.update(cx, |shell, cx| shell.set_surfaces_open(true, cx));
    cx.update(|window, cx| window.draw(cx).clear());
    cx.update(|window, cx| {
        assert!(shell.read(cx).composer.focus_handle(cx).is_focused(window));
    });
    // Restored tabs do not take navigation away from the main composer.
    press(cx, true);
    shell.read_with(cx, |shell, cx| {
        assert_eq!(shell.state.read(cx).selected_chat.as_deref(), Some("other"));
    });
}

#[gpui::test]
fn embedded_terminal_and_browser_focus_route_to_right_tabs(cx: &mut TestAppContext) {
    let (shell, cx) = setup(cx);
    shell.update(cx, |shell, cx| {
        // No engine in this harness: reserve a real emulator tab without
        // starting a PTY, then use the normal surface activation path.
        shell.right_terminal_panel(cx).update(cx, |panel, cx| {
            panel.reserve_tab_for_chat("parent".into(), "Test terminal", cx);
        });
        shell.add_terminal_surface(cx);
    });
    cx.update(|window, cx| window.draw(cx).clear());
    cx.update(|window, cx| {
        let terminal = shell.read(cx).right_terminal.as_ref().unwrap().read(cx);
        assert!(terminal.focus_handle().is_focused(window));
    });
    press(cx, true);
    assert_surface(&shell, cx, RightSurface::Subagent(1));
    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            shell.add_browser_surface(None, window, cx);
        })
    });
    cx.update(|window, cx| window.draw(cx).clear());
    press(cx, true);
    assert_surface(&shell, cx, RightSurface::Subagent(1));
    // Exercise the GPUI target the native browser bridge focuses before it
    // redispatches a shortcut (the native OS event itself needs platform QA).
    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            shell.set_right_active(RightSurface::Browser(1), cx);
            window.focus(&shell.browsers[&1].focus_handle(cx), cx);
        })
    });
    cx.update(|window, cx| window.draw(cx).clear());
    press(cx, true);
    assert_surface(&shell, cx, RightSurface::Subagent(1));
}

#[gpui::test]
fn bottom_terminal_keeps_session_navigation(cx: &mut TestAppContext) {
    let (shell, cx) = setup(cx);
    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            shell.terminal_panel(cx).update(cx, |panel, cx| {
                panel.reserve_tab_for_chat("parent".into(), "Test terminal", cx);
            });
            shell.toggle_terminal(window, cx);
        })
    });
    cx.update(|window, cx| window.draw(cx).clear());
    cx.update(|window, cx| {
        assert!(
            shell
                .read(cx)
                .terminal
                .as_ref()
                .unwrap()
                .read(cx)
                .focus_handle()
                .is_focused(window)
        );
    });
    press(cx, true);
    shell.read_with(cx, |shell, cx| {
        assert_eq!(shell.state.read(cx).selected_chat.as_deref(), Some("other"));
    });
}

#[gpui::test]
fn each_session_cycles_only_its_own_tabs(cx: &mut TestAppContext) {
    let (shell, cx) = setup(cx);
    shell.update(cx, |shell, cx| shell.open_chat("other".into(), cx));
    cx.update(|window, cx| window.draw(cx).clear());
    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            shell.add_subagent_surface("other".into(), "third".into(), "Third".into(), false, cx);
            shell.activate_right_surface(RightSurface::Subagent(3), window, cx);
        })
    });
    press(cx, true);
    shell.read_with(cx, |shell, cx| {
        assert_eq!(shell.state.read(cx).selected_chat.as_deref(), Some("other"));
        assert_eq!(shell.resolved_right_active(cx), RightSurface::Subagent(3));
    });
    shell.update(cx, |shell, cx| shell.open_chat("parent".into(), cx));
    cx.update(|window, cx| window.draw(cx).clear());
    let tab = cx.debug_bounds("right-surface-tab-0").unwrap().center();
    cx.simulate_click(tab, gpui::Modifiers::default());
    press(cx, true);
    assert_surface(&shell, cx, RightSurface::Subagent(2));
    press(cx, true);
    assert_surface(&shell, cx, RightSurface::Subagent(1));
}

/// Settles a transcript selection and seeds the clipboard with a sentinel, so
/// a keystroke that copies nothing is distinguishable from one that does.
fn select_transcript_text(cx: &mut VisualTestContext) {
    crate::markdown::selection::begin_with_span("transcript-row:0", "selected reply", 0..8);
    crate::markdown::selection::end_active_drag();
    cx.update(|_, cx| cx.write_to_clipboard(ClipboardItem::new_string("sentinel".into())));
}

fn clipboard_text(cx: &mut VisualTestContext) -> Option<String> {
    cx.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text()))
}

fn clear_transcript_selection() {
    crate::markdown::selection::clear_if_owner("transcript-row:0");
}

#[gpui::test]
fn copy_shortcut_copies_transcript_selection_after_clicking_the_transcript(
    cx: &mut TestAppContext,
) {
    let _selection = crate::markdown::selection::test_state_lock();
    let (shell, cx) = setup(cx);
    cx.update(|window, cx| window.focus(&shell.read(cx).composer.focus_handle(cx), cx));
    let main = cx.debug_bounds("navigation-main").unwrap();
    cx.simulate_click(
        main.origin + gpui::point(px(100.), px(150.)),
        gpui::Modifiers::default(),
    );
    cx.update(|window, cx| window.draw(cx).clear());
    cx.update(|window, cx| {
        let shell = shell.read(cx);
        assert!(!shell.composer.focus_handle(cx).is_focused(window));
        assert!(shell.navigation_focus.main.is_focused(window));
    });
    select_transcript_text(cx);
    cx.simulate_keystrokes("ctrl-c");
    assert_eq!(clipboard_text(cx).as_deref(), Some("selected"));
    clear_transcript_selection();
}

#[gpui::test]
fn copy_shortcut_copies_side_chat_selection_from_the_right_pane(cx: &mut TestAppContext) {
    let _selection = crate::markdown::selection::test_state_lock();
    let (shell, cx) = setup(cx);
    cx.update(|window, cx| {
        let right = shell.read(cx).navigation_focus.right.clone();
        window.focus(&right, cx);
    });
    cx.update(|window, cx| window.draw(cx).clear());
    select_transcript_text(cx);
    cx.simulate_keystrokes("ctrl-c");
    assert_eq!(clipboard_text(cx).as_deref(), Some("selected"));
    clear_transcript_selection();
}

#[gpui::test]
fn copy_shortcut_prefers_the_focused_composer_selection(cx: &mut TestAppContext) {
    let _selection = crate::markdown::selection::test_state_lock();
    let (shell, cx) = setup(cx);
    cx.update(|window, cx| window.focus(&shell.read(cx).composer.focus_handle(cx), cx));
    cx.update(|window, cx| window.draw(cx).clear());
    cx.simulate_input("draft text");
    select_transcript_text(cx);
    cx.simulate_keystrokes("ctrl-a ctrl-c");
    assert_eq!(clipboard_text(cx).as_deref(), Some("draft text"));
    clear_transcript_selection();
}

#[gpui::test]
fn ctrl_c_in_a_terminal_is_not_taken_by_a_transcript_selection(cx: &mut TestAppContext) {
    let _selection = crate::markdown::selection::test_state_lock();
    let (shell, cx) = setup(cx);
    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            shell.terminal_panel(cx).update(cx, |panel, cx| {
                panel.reserve_tab_for_chat("parent".into(), "Test terminal", cx);
            });
            shell.toggle_terminal(window, cx);
        })
    });
    cx.update(|window, cx| window.draw(cx).clear());
    select_transcript_text(cx);
    cx.simulate_keystrokes("ctrl-c");
    assert_eq!(clipboard_text(cx).as_deref(), Some("sentinel"));
    clear_transcript_selection();
}
