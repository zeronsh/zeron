//! Run with `web-input-tests` and the matching local zui runtime; see input-tests.md.
use super::*;
use crate::composer::ComposerInput;
use gpui::{Modifiers, PlatformInput, TestAppContext, point};

struct InputSurfaces {
    terminal: Entity<TerminalPanel>,
    composer: Entity<ComposerInput>,
}

impl Render for InputSurfaces {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .w(px(300.))
                    .h(px(200.))
                    .flex_none()
                    .child(self.terminal.clone()),
            )
            .child(
                div()
                    .w(px(300.))
                    .h(px(100.))
                    .flex_none()
                    .child(self.composer.clone()),
            )
    }
}

fn press(window: &mut Window, cx: &mut App, x: f32, y: f32) -> gpui::DispatchEventResult {
    window.dispatch_event(
        PlatformInput::MouseDown(MouseDownEvent {
            button: MouseButton::Left,
            position: point(px(x), px(y)),
            modifiers: Modifiers::default(),
            click_count: 1,
            first_mouse: false,
        }),
        cx,
    )
}

#[test]
fn terminal_touch_focus_from_neutral_and_composer_without_repaint() {
    let mut cx = TestAppContext::single();
    cx.update(|cx| cx.set_global(Theme::dark()));
    let host = cx.add_window(|_, cx| {
        let state = cx.new(|_| {
            let mut state = AppState::new();
            state.selected_chat = Some("terminal-input-test".into());
            state
        });
        InputSurfaces {
            terminal: cx.new(|cx| TerminalPanel::new_embedded(state, cx)),
            composer: cx.new(|cx| ComposerInput::new("Type here", cx)),
        }
    });
    cx.update_window(host.into(), |_, window, cx| window.draw(cx).clear())
        .unwrap();
    host.update(&mut cx, |view, window, cx| {
        // These are actual application elements and actual input dispatches.
        // No render or stale platform handler can supply the answer mid-gesture.
        assert_eq!(press(window, cx, 350., 350.).text_input_focus, None);
        assert_eq!(press(window, cx, 20., 20.).text_input_focus, Some(true));
        assert!(view.terminal.read(cx).focus_handle.is_focused(window));
        assert_eq!(press(window, cx, 20., 220.).text_input_focus, Some(true));
        assert_eq!(press(window, cx, 20., 20.).text_input_focus, Some(true));
        // Retained logical focus after OS dismissal must not reopen on blank.
        assert_eq!(press(window, cx, 350., 350.).text_input_focus, None);
        assert_eq!(press(window, cx, 20., 20.).text_input_focus, Some(true));
    })
    .unwrap();
}

#[test]
fn terminal_web_text_commits_once_and_preserves_key_encoding() {
    use gpui::EntityInputHandler;
    let mut cx = TestAppContext::single();
    cx.update(|cx| cx.set_global(Theme::dark()));
    let host = cx.add_window(|_, cx| {
        let state = cx.new(|_| {
            let mut state = AppState::new();
            state.selected_chat = Some("terminal-input-test".into());
            state
        });
        TerminalPanel::new_embedded(state, cx)
    });
    host.update(&mut cx, |panel, window, cx| {
        panel.chats.insert(
            "terminal-input-test".into(),
            ChatTabs {
                active: 0,
                tabs: vec![TerminalTab {
                    key: 1,
                    title: "test".into(),
                    terminal_id: Some("test".into()),
                    emulator: Emulator::new(80, 24),
                    exited: None,
                    last_seq: 0,
                    coalescer: InputCoalescer::default(),
                    flush_task: None,
                    resize_task: None,
                    _run: None,
                }],
            },
        );
        let drain = |panel: &mut TerminalPanel| {
            panel.chats.get_mut("terminal-input-test").unwrap().tabs[0]
                .coalescer
                .take()
        };
        assert!(panel.selected_text_range(false, window, cx).is_none());
        panel.replace_and_mark_text_in_range(None, "ni", None, window, cx);
        panel.replace_and_mark_text_in_range(None, "你好", None, window, cx);
        assert!(drain(panel).is_empty(), "preedit must never reach the PTY");
        panel.replace_text_in_range(None, "你好🙂", window, cx);
        panel.unmark_text(window, cx);
        assert_eq!(drain(panel), "你好🙂".as_bytes());
        panel.replace_text_in_range(None, "\n", window, cx);
        assert_eq!(drain(panel), b"\r");
        for (key, expected) in [
            ("a", b"a".as_slice()),
            ("backspace", b"\x7f"),
            ("delete", b"\x1b[3~"),
            ("ctrl-c", b"\x03"),
        ] {
            panel.on_key_down(
                &KeyDownEvent {
                    keystroke: gpui::Keystroke::parse(key).unwrap(),
                    is_held: false,
                    prefer_character_input: false,
                },
                window,
                cx,
            );
            assert_eq!(drain(panel), expected);
        }
        panel.chats.get_mut("terminal-input-test").unwrap().tabs[0].exited = Some(0);
        panel.replace_text_in_range(None, "ignored", window, cx);
        assert!(
            drain(panel).is_empty(),
            "exited terminals still reject input"
        );
    })
    .unwrap();
}
