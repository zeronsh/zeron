use super::*;
use crate::markdown::{render, selection};
use gpui::{TestAppContext, WindowHandle};

struct SelectionHost {
    transcript: Entity<Transcript>,
    focus: FocusHandle,
}

impl Render for SelectionHost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .track_focus(&self.focus)
            .on_key_down(|event, _, cx| navigation_focus::copy_transcript_selection(event, cx))
            .child(self.transcript.clone())
    }
}

fn draw(window: WindowHandle<SelectionHost>, cx: &mut TestAppContext) {
    cx.update_window(window.into(), |_, window, cx| {
        window.refresh();
        window.draw(cx).clear();
    })
    .unwrap();
}

#[gpui::test]
fn the_same_chat_in_two_windows_selects_and_copies_its_own_surface(cx: &mut TestAppContext) {
    let _lock = selection::test_state_lock();
    let dir = tempfile::tempdir().unwrap();
    let owner = cx.update(|cx| {
        gpui_base::init(cx);
        cx.set_global(Theme::dark());
        settings::init(Default::default(), dir.path(), cx);
        crate::window_manager::init(cx);
        cx.new(|_| AppState::new())
    });
    const TEXT: &str = "Shared conversation text is independently selectable.";
    const KEY: &str = "reply#body.0:0";
    let history = vec![zeron_doc::SessionMessageEntry {
        id: "reply".into(),
        role: zeron_doc::MessageRole::Assistant,
        parts: vec![zeron_doc::MessagePart::Text {
            id: "body".into(),
            text: TEXT.into(),
        }],
        created_at: 0,
        device_id: "local".into(),
        status: Some(zeron_doc::MessageStatus::Complete),
        continuation_of: None,
        duration_ms: None,
    }];
    let source = cx.update(|cx| {
        let source = crate::chat_store::acquire("shared-chat".into(), None, cx);
        source.update(cx, |source, cx| {
            source
                .receive_transcript_update(
                    zeron_doc::TranscriptUpdate {
                        frame: zeron_doc::TranscriptFrame::reset(&history),
                        replay_baseline: Some(zeron_doc::TranscriptBaseline::capture(&history)),
                        context_usage: None,
                    },
                    cx,
                )
                .unwrap()
        });
        source
    });
    let windows: Vec<_> = (0..2)
        .map(|_| {
            cx.add_window(|_, cx| {
                let state = cx.new(|cx| AppState::for_window(owner.clone(), cx));
                state.update(cx, |state, cx| {
                    state.select_chat(Some("shared-chat".into()), cx)
                });
                let transcript = cx.new(|cx| Transcript::new(state, cx));
                transcript.update(cx, |transcript, _| transcript.retain_for_route_exit());
                SelectionHost {
                    transcript,
                    focus: cx.focus_handle(),
                }
            })
        })
        .collect();
    for window in &windows {
        draw(*window, cx);
    }
    let bounds = render::selection_test_bounds(KEY);
    let start = bounds.origin + gpui::point(px(1.0), bounds.size.height / 2.0);
    let end = gpui::point(bounds.right() + px(5.0), start.y);
    let surfaces: Vec<_> = windows
        .iter()
        .map(|window| {
            window
                .read_with(cx, |host, _| host.transcript.entity_id().as_u64())
                .unwrap()
        })
        .collect();
    for index in [0, 1, 0] {
        let window = windows[index];
        // A streaming peer may draw several times before this window gets input.
        for _ in 0..3 {
            draw(windows[1 - index], cx);
        }
        cx.update_window(window.into(), |host, window, cx| {
            let host = host.downcast::<SelectionHost>().unwrap();
            let focus = host.read(cx).focus.clone();
            window.focus(&focus, cx);
            for event in [
                gpui::PlatformInput::MouseDown(gpui::MouseDownEvent {
                    button: gpui::MouseButton::Left,
                    position: start,
                    click_count: 1,
                    ..Default::default()
                }),
                gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
                    position: end,
                    pressed_button: Some(gpui::MouseButton::Left),
                    ..Default::default()
                }),
                gpui::PlatformInput::MouseUp(gpui::MouseUpEvent {
                    button: gpui::MouseButton::Left,
                    position: end,
                    ..Default::default()
                }),
            ] {
                window.dispatch_event(event, cx);
            }
            assert_eq!(selection::selected_text().as_deref(), Some(TEXT));
            cx.write_to_clipboard(ClipboardItem::new_string("sentinel".into()));
            window.dispatch_event(
                gpui::PlatformInput::KeyDown(gpui::KeyDownEvent {
                    keystroke: gpui::Keystroke::parse("ctrl-c").unwrap(),
                    is_held: false,
                    prefer_character_input: false,
                }),
                cx,
            );
            assert_eq!(
                cx.read_from_clipboard()
                    .and_then(|item| item.text())
                    .as_deref(),
                Some(TEXT)
            );
        })
        .unwrap();
        assert_eq!(selection::anchor_surface(), surfaces[index]);
        assert_eq!(
            selection::wash_range(surfaces[index], KEY),
            Some(0..TEXT.len())
        );
        assert_eq!(selection::wash_range(surfaces[1 - index], KEY), None);
        selection::clear_if_owner(KEY);
    }
    assert_eq!(render::selection_test_entry_count(KEY), 2);
    cx.update_window(windows[0].into(), |_, window, _| window.remove_window())
        .unwrap();
    cx.run_until_parked();
    assert_eq!(
        render::selection_test_entry_count(KEY),
        1,
        "closing one window removes only its selection entries"
    );
    drop(source);
}
