//! Split surface hosts through the shell's real open, activate and close
//! paths, without booting an engine.
use super::right_layout::{DropZone, Edge};
use super::right_panes::RightPaneDrop;
use super::*;
use gpui::{AppContext, TestAppContext, VisualTestContext};

struct SplitHost {
    shell: Entity<Shell>,
    _data_dir: tempfile::TempDir,
}

impl Render for SplitHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.shell.update(cx, |shell, cx| {
            let right = shell.render_right_pane(window, cx);
            div()
                .size_full()
                .flex()
                .flex_col()
                .child(
                    div()
                        .h(px(40.))
                        .child(shell.render_right_titlebar_strips(-8.0, cx)),
                )
                .child(div().flex_1().min_h_0().child(right))
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
            shell.active_chat = "parent".into();
            shell.reduced_motion = true;
            for doc in ["first", "second", "third"] {
                shell.add_subagent_surface("parent".into(), doc.into(), doc.into(), false, cx);
            }
            shell.right_tween = None;
            shell
        });
        SplitHost {
            shell,
            _data_dir: dir,
        }
    });
    let shell = host.read_with(cx, |host, _| host.shell.clone());
    cx.update(|window, cx| window.draw(cx).clear());
    (shell, cx)
}

/// The app's geometry: the titlebar overlays the full-height content, so a
/// side chat's transcript runs under the strip.
struct OverlayHost {
    shell: Entity<Shell>,
}

impl Render for OverlayHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.shell.update(cx, |shell, cx| {
            let right = shell.render_right_pane(window, cx);
            div().size_full().relative().child(right).child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .h(px(Theme::TITLEBAR_HEIGHT))
                    .child(shell.render_right_titlebar_strips(-8.0, cx)),
            )
        })
    }
}

fn tab(id: u64) -> RightSurface {
    RightSurface::Subagent(id)
}

fn panes(shell: &Shell, cx: &App) -> Vec<Vec<RightSurface>> {
    let Some(layout) = shell.right_split(cx) else {
        return vec![
            shell
                .right_surface_rows(cx)
                .into_iter()
                .map(|(tab, ..)| tab)
                .collect(),
        ];
    };
    layout
        .pane_ids()
        .into_iter()
        .map(|pane| {
            shell
                .right_pane_rows(Some(pane), cx)
                .into_iter()
                .map(|(tab, ..)| tab)
                .collect()
        })
        .collect()
}

fn close(shell: &Entity<Shell>, cx: &mut VisualTestContext, surface: RightSurface) {
    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            shell.close_right_surface(surface, window, cx)
        });
        window.draw(cx).clear();
    });
}

#[gpui::test]
fn a_split_lives_until_one_pane_is_left(cx: &mut TestAppContext) {
    let (shell, cx) = setup(cx);
    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            assert!(shell.split_right_tab(tab(3), None, Edge::Right, cx));
            assert_eq!(panes(shell, cx), vec![vec![tab(1), tab(2)], vec![tab(3)]]);
            assert_eq!(shell.resolved_right_active(cx), tab(3));
        });
        window.draw(cx).clear();
    });
    // Closing the right pane's only tab closes the pane and dissolves the
    // split; the left pane's tab shows again.
    close(&shell, cx, tab(3));
    shell.read_with(cx, |shell, cx| {
        assert!(shell.right_split(cx).is_none());
        assert_eq!(shell.right_tabs["parent"], vec![tab(1), tab(2)]);
        assert_eq!(shell.resolved_right_active(cx), tab(2));
        assert!(shell.right_pane_open(cx));
    });
}

#[gpui::test]
fn a_pane_lone_tab_cannot_split_off_itself(cx: &mut TestAppContext) {
    let (shell, cx) = setup(cx);
    shell.update(cx, |shell, cx| {
        assert!(shell.split_right_tab(tab(3), None, Edge::Bottom, cx));
        let lone = shell.right_pane_of(tab(3), cx);
        assert!(!shell.split_right_tab(tab(3), lone, Edge::Right, cx));
        assert_eq!(shell.right_split(cx).unwrap().pane_count(), 2);
    });
}

#[gpui::test]
fn showing_a_tab_focuses_its_pane_and_new_tabs_join_the_focused_one(cx: &mut TestAppContext) {
    let (shell, cx) = setup(cx);
    shell.update(cx, |shell, cx| {
        shell.split_right_tab(tab(3), None, Edge::Right, cx);
        let left = shell.right_pane_of(tab(1), cx).unwrap();
        shell.set_right_active(tab(1), cx);
        assert_eq!(shell.right_split(cx).unwrap().focused(), left);
        // The right pane keeps showing its own tab.
        let right = shell.right_pane_of(tab(3), cx).unwrap();
        assert_eq!(shell.resolved_pane_active(right, cx), tab(3));
        shell.add_subagent_surface("parent".into(), "fourth".into(), "fourth".into(), false, cx);
        assert_eq!(shell.right_pane_of(tab(4), cx), Some(left));
        assert_eq!(shell.resolved_right_active(cx), tab(4));
        // Focusing the right pane shows its tab as the chat's active one.
        shell.focus_right_pane(right, cx);
        assert_eq!(shell.resolved_right_active(cx), tab(3));
    });
}

#[gpui::test]
fn tab_menus_and_close_fallbacks_stay_within_a_pane(cx: &mut TestAppContext) {
    let (shell, cx) = setup(cx);
    shell.update(cx, |shell, cx| {
        shell.add_subagent_surface("parent".into(), "fourth".into(), "fourth".into(), false, cx);
        shell.split_right_tab(tab(4), None, Edge::Right, cx);
        let right = shell.right_pane_of(tab(4), cx).unwrap();
        shell.move_right_tab(tab(3), right, None, cx);
        assert_eq!(
            panes(shell, cx),
            vec![vec![tab(1), tab(2)], vec![tab(4), tab(3)]]
        );
        assert_eq!(
            shell.tabs_to_close(tab(4), TabCloseAction::Others, cx),
            vec![tab(3)]
        );
        assert_eq!(
            shell.tabs_to_close(tab(1), TabCloseAction::Right, cx),
            vec![tab(2)]
        );
    });
    // The right pane's shown tab closes onto its neighbour, not a tab of
    // the left pane.
    close(&shell, cx, tab(3));
    shell.read_with(cx, |shell, cx| {
        assert_eq!(shell.resolved_right_active(cx), tab(4));
    });
}

#[gpui::test]
fn close_pane_closes_its_tabs_and_the_pane(cx: &mut TestAppContext) {
    let (shell, cx) = setup(cx);
    shell.update(cx, |shell, cx| {
        shell.split_right_tab(tab(3), None, Edge::Bottom, cx);
        let bottom = shell.right_pane_of(tab(3), cx).unwrap();
        shell.move_right_tab(tab(2), bottom, None, cx);
    });
    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| shell.close_right_pane(tab(3), window, cx));
        window.draw(cx).clear();
    });
    shell.read_with(cx, |shell, cx| {
        assert!(shell.right_split(cx).is_none());
        assert_eq!(shell.right_tabs["parent"], vec![tab(1)]);
        assert_eq!(shell.resolved_right_active(cx), tab(1));
    });
}

#[gpui::test]
fn dropping_on_a_pane_edge_splits_and_on_another_pane_moves(cx: &mut TestAppContext) {
    let (shell, cx) = setup(cx);
    shell.update(cx, |shell, cx| {
        shell.commit_right_pane_drop(
            RightPaneDrop {
                pane: None,
                zone: DropZone::Edge(Edge::Left),
                surface: tab(2),
            },
            cx,
        );
        assert_eq!(panes(shell, cx), vec![vec![tab(2)], vec![tab(1), tab(3)]]);
        let left = shell.right_pane_of(tab(2), cx);
        shell.commit_right_pane_drop(
            RightPaneDrop {
                pane: left,
                zone: DropZone::Center,
                surface: tab(1),
            },
            cx,
        );
        assert_eq!(panes(shell, cx), vec![vec![tab(2), tab(1)], vec![tab(3)]]);
        assert_eq!(shell.resolved_right_active(cx), tab(1));
    });
    // Both panes render, each with its own strip.
    cx.update(|window, cx| window.draw(cx).clear());
}

#[gpui::test]
fn each_chat_keeps_its_own_split(cx: &mut TestAppContext) {
    let (shell, cx) = setup(cx);
    shell.update(cx, |shell, cx| {
        shell.split_right_tab(tab(3), None, Edge::Right, cx);
        shell.active_chat = "other".into();
        assert!(shell.right_split(cx).is_none());
        shell.active_chat = "parent".into();
        assert!(shell.right_split(cx).is_some());
    });
}

/// A lower pane's own strip sits above a side chat in flow: its transcript
/// fades and parks prompts from its top, not below a titlebar band.
#[gpui::test]
fn a_lower_side_chat_clears_no_titlebar(cx: &mut TestAppContext) {
    let (shell, cx) = setup(cx);
    let inset = |cx: &mut VisualTestContext| {
        cx.update(|window, cx| window.draw(cx).clear());
        shell.read_with(cx, |shell, cx| {
            let id = shell.side_chat_seq;
            shell.side_chats[&id]
                .transcript
                .read(cx)
                .own_send_top_inset()
        })
    };
    shell.update(cx, |shell, cx| {
        let chat = serde_json::from_value(serde_json::json!({
            "id": "side", "parentChatId": "parent", "deviceId": "local",
            "archived": false, "createdAt": Utc::now(),
        }))
        .unwrap();
        shell.open_side_chat(chat, shell.panel_key(cx), cx);
    });
    let under_titlebar = inset(cx);
    shell.update(cx, |shell, cx| {
        let side = RightSurface::SideChat(shell.side_chat_seq);
        assert!(shell.split_right_tab(side, None, Edge::Bottom, cx));
    });
    assert_eq!(inset(cx), under_titlebar - Theme::TITLEBAR_HEIGHT);
    // Back under the band once the pane above it closes.
    for id in 1..=3 {
        close(&shell, cx, tab(id));
    }
    assert!(shell.read_with(cx, |shell, cx| shell.right_split(cx).is_none()));
    assert_eq!(inset(cx), under_titlebar);
}

/// Over a side chat, the titlebar strip sits on the chat's drop target: a
/// tab dragged along the strip reorders instead of splitting the host.
#[gpui::test]
fn reordering_over_a_side_chat_does_not_split(cx: &mut TestAppContext) {
    use gpui::{Modifiers, MouseButton};
    let (shell, _) = setup(cx);
    let (_, cx) = cx.add_window_view(|_, _| OverlayHost {
        shell: shell.clone(),
    });
    shell.update(cx, |shell, cx| {
        let chat = serde_json::from_value(serde_json::json!({
            "id": "side", "parentChatId": "parent", "deviceId": "local",
            "archived": false, "createdAt": Utc::now(),
        }))
        .unwrap();
        shell.open_side_chat(chat, shell.panel_key(cx), cx);
    });
    cx.update(|window, cx| window.draw(cx).clear());
    let drag_to = |cx: &mut VisualTestContext, to: Point<Pixels>| {
        let from = cx.debug_bounds("right-surface-tab-0").unwrap().center();
        cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_move(to, Some(MouseButton::Left), Modifiers::default());
        cx.update(|window, cx| window.draw(cx).clear());
        cx.simulate_mouse_move(to, Some(MouseButton::Left), Modifiers::default());
        cx.update(|window, cx| window.draw(cx).clear());
        let armed = shell.read_with(cx, |shell, _| shell.right_pane_drop.map(|drop| drop.zone));
        cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
        cx.update(|window, cx| window.draw(cx).clear());
        armed
    };
    let over = cx.debug_bounds("right-surface-tab-2").unwrap().center();
    assert_eq!(drag_to(cx, over), None);
    shell.read_with(cx, |shell, cx| {
        assert!(shell.right_split(cx).is_none());
        assert_eq!(
            panes(shell, cx),
            vec![vec![tab(2), tab(3), tab(1), RightSurface::SideChat(1)]]
        );
    });
    // Just below the band the chat's top edge still splits.
    let below = gpui::point(over.x, px(Theme::TITLEBAR_HEIGHT + 8.0));
    assert_eq!(drag_to(cx, below), Some(DropZone::Edge(Edge::Top)));
    shell.read_with(cx, |shell, cx| {
        assert_eq!(
            panes(shell, cx),
            vec![
                vec![tab(2)],
                vec![tab(3), tab(1), RightSurface::SideChat(1)]
            ]
        );
    });
}
