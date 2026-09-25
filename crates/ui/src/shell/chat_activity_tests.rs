//! Exercise the activity popup together with the shell's real context menus
//! and dialogs, including actions painted outside the parent popup's bounds.

use super::*;
use gpui::{TestAppContext, VisualTestContext};

struct ActivityHost {
    shell: Entity<Shell>,
    _observe: Subscription,
    _data_dir: tempfile::TempDir,
}

impl Render for ActivityHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.shell.update(cx, |shell, cx| {
            let overlays = shell.render_overlays(window.viewport_size(), window, cx);
            div()
                .size_full()
                .flex()
                .flex_col()
                .justify_end()
                .items_end()
                .p(px(20.0))
                .pr(px(160.0))
                .capture_key_down(cx.listener(Shell::on_key_down_capture))
                .child(shell.chat_activity.clone())
                .children(overlays)
        })
    }
}

fn setup(cx: &mut TestAppContext) -> (Entity<Shell>, &mut VisualTestContext) {
    let dir = tempfile::tempdir().unwrap();
    cx.update(|cx| {
        gpui_base::init(cx);
        cx.set_global(Theme::dark());
        crate::app_menus::init(cx);
    });
    let (host, cx) = cx.add_window_view(|_, cx| {
        let shell = cx.new(|cx| {
            let state = cx.new(|_| {
                let mut state = AppState::new();
                state.selected_chat = Some("parent".into());
                state.chats = serde_json::from_value(serde_json::json!([
                    { "id": "parent", "deviceId": "local", "archived": false,
                      "createdAt": Utc::now() },
                    { "id": "child", "deviceId": "local", "archived": false,
                      "createdAt": Utc::now(), "parentChatId": "parent", "title": "Side chat" }
                ]))
                .unwrap();
                state
            });
            Shell::new(
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
            )
        });
        let observe = cx.observe(&shell, |_, _, cx| cx.notify());
        ActivityHost {
            shell,
            _observe: observe,
            _data_dir: dir,
        }
    });
    let shell = host.read_with(cx, |host, _| host.shell.clone());
    cx.update(|window, cx| window.draw(cx).clear());
    (shell, cx)
}

fn click(cx: &mut VisualTestContext, selector: &'static str) {
    let position = cx.debug_bounds(selector).expect(selector).center();
    cx.simulate_click(position, gpui::Modifiers::default());
}

fn open_context_menu(shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    let row = cx.debug_bounds("chat-activity-chat-child").unwrap();
    let position = gpui::point(row.right() - px(18.0), row.center().y);
    cx.simulate_mouse_down(position, MouseButton::Right, gpui::Modifiers::default());
    cx.simulate_mouse_up(position, MouseButton::Right, gpui::Modifiers::default());
    shell.read_with(cx, |shell, cx| {
        assert!(
            shell.chat_activity.read(cx).is_open(),
            "right-click closed the activity list"
        );
        assert_eq!(shell.chat_menu.as_open().unwrap().chat_id, "child");
    });
    assert!(cx.debug_bounds("chat-activity-menu").is_some());
    assert!(cx.debug_bounds("chat-menu-rename").is_some());
}

#[gpui::test]
fn activity_context_actions_keep_parent_open_through_dialogs(cx: &mut TestAppContext) {
    let (shell, cx) = setup(cx);
    click(cx, "chat-activity-trigger");
    for (action, cancel) in [
        ("chat-menu-rename", "rename-chat-cancel"),
        ("chat-menu-delete", "delete-chat-cancel"),
    ] {
        open_context_menu(&shell, cx);
        let parent = cx.debug_bounds("chat-activity-menu").unwrap();
        let action_position = cx.debug_bounds(action).unwrap().center();
        assert!(
            !parent.contains(&action_position),
            "exercise the child menu outside its parent"
        );
        click(cx, action);
        shell.read_with(cx, |shell, cx| {
            assert!(shell.chat_activity.read(cx).is_open());
            if action == "chat-menu-rename" {
                assert_eq!(shell.rename_dialog.as_ref().unwrap().chat_id, "child");
            } else {
                assert_eq!(shell.delete_confirm.as_deref(), Some("child"));
            }
        });
        // Finish the context menu's exit before interacting with the modal.
        shell.update(cx, |shell, cx| {
            shell.chat_menu = popover::Popup::default();
            cx.notify();
        });
        click(cx, cancel);
        shell.read_with(cx, |shell, cx| {
            assert!(
                shell.chat_activity.read(cx).is_open(),
                "cancel closed the activity list"
            );
            assert!(shell.rename_dialog.is_none());
            assert!(shell.delete_confirm.is_none());
        });
    }
    // Outside dismissal works again after the dialog closes.
    cx.simulate_click(gpui::point(px(5.0), px(5.0)), gpui::Modifiers::default());
    shell.read_with(cx, |shell, cx| {
        assert!(!shell.chat_activity.read(cx).is_open())
    });
}

#[gpui::test]
fn activity_context_escape_and_outside_click_dismiss_only_the_child(cx: &mut TestAppContext) {
    let (shell, cx) = setup(cx);
    click(cx, "chat-activity-trigger");
    for escape in [true, false] {
        open_context_menu(&shell, cx);
        if escape {
            cx.simulate_keystrokes("escape");
        } else {
            cx.simulate_click(gpui::point(px(5.0), px(5.0)), gpui::Modifiers::default());
        }
        shell.update(cx, |shell, cx| {
            assert!(shell.chat_activity.read(cx).is_open());
            assert!(!shell.chat_menu.is_open());
            shell.chat_menu = popover::Popup::default();
            cx.notify();
        });
    }
}
