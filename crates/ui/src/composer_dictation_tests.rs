use super::*;
use crate::dictation::{Event, Phase, Transcriber};
use gpui::{AppContext, TestAppContext};
use std::{cell::RefCell, collections::VecDeque};

#[derive(Default)]
struct FakeState {
    events: VecDeque<Event>,
    finishes: usize,
    drops: usize,
}

struct Fake(Rc<RefCell<FakeState>>);
impl Transcriber for Fake {
    fn poll(&mut self) -> Option<Event> {
        self.0.borrow_mut().events.pop_front()
    }
    fn finish(&mut self) {
        self.0.borrow_mut().finishes += 1;
    }
}
impl Drop for Fake {
    fn drop(&mut self) {
        self.0.borrow_mut().drops += 1;
    }
}

fn start(input: &mut ComposerInput, cx: &mut Context<ComposerInput>) -> Rc<RefCell<FakeState>> {
    let fake = Rc::new(RefCell::new(FakeState::default()));
    input.begin_dictation(Box::new(Fake(fake.clone())), cx);
    fake
}

fn deliver(
    input: &mut ComposerInput,
    fake: &Rc<RefCell<FakeState>>,
    events: impl IntoIterator<Item = Event>,
    cx: &mut Context<ComposerInput>,
) {
    fake.borrow_mut().events.extend(events);
    input.poll_dictation(input.dictation.generation, cx);
}

#[gpui::test]
fn dictation_replaces_unicode_selection_and_undoes_as_one_edit(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_global(Theme::dark()));
    let window = cx.add_window(|_, cx| ComposerInput::new("Draft", cx));
    window
        .update(cx, |input, window, cx| {
            input.set_text("👩🏽‍💻 replace café", cx);
            let begin = input.content.find("replace").unwrap();
            input.selected_range = begin..begin + "replace".len();
            let fake = start(input, cx);
            deliver(
                input,
                &fake,
                [
                    Event::Listening,
                    Event::Partial("你好".into()),
                    Event::Partial("bonjour 🌍".into()),
                ],
                cx,
            );
            assert_eq!(input.content, "👩🏽‍💻 bonjour 🌍 café");
            assert_eq!(input.undo_stack.len(), 1);
            input.finish_dictation(false, cx);
            deliver(input, &fake, [Event::Final("salut 🌍".into())], cx);
            assert_eq!(input.content, "👩🏽‍💻 salut 🌍 café");
            assert_eq!(fake.borrow().drops, 1);
            input.undo(&Undo, window, cx);
            assert_eq!(input.content, "👩🏽‍💻 replace café");
            assert_eq!(input.selected_range, begin..begin + 7);
            input.redo(&Redo, window, cx);
            assert_eq!(input.content, "👩🏽‍💻 salut 🌍 café");
        })
        .unwrap();
}

#[gpui::test]
fn dictation_manual_edit_stops_before_replacing_partial(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_global(Theme::dark()));
    let window = cx.add_window(|_, cx| ComposerInput::new("Draft", cx));
    window
        .update(cx, |input, window, cx| {
            input.set_text("before after", cx);
            input.selected_range = 7..7;
            let fake = start(input, cx);
            let old_generation = input.dictation.generation;
            deliver(input, &fake, [Event::Partial("speech ".into())], cx);
            input.replace_text_in_range(Some(7..13), "typed", window, cx);
            assert_eq!(input.content, "before typed after");
            assert_eq!(fake.borrow().drops, 1);
            fake.borrow_mut()
                .events
                .push_back(Event::Final("stale".into()));
            assert!(!input.poll_dictation(old_generation, cx));
            assert_eq!(input.content, "before typed after");
            input.undo(&Undo, window, cx);
            assert_eq!(input.content, "before speech after");
            input.undo(&Undo, window, cx);
            assert_eq!(input.content, "before after");
        })
        .unwrap();
}

#[gpui::test]
fn dictation_ime_commit_preserves_composition_and_history(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_global(Theme::dark()));
    let window = cx.add_window(|_, cx| ComposerInput::new("Draft", cx));
    window
        .update(cx, |input, window, cx| {
            let fake = start(input, cx);
            deliver(input, &fake, [Event::Partial("Hello ".into())], cx);
            input.replace_and_mark_text_in_range(None, "に", None, window, cx);
            assert_eq!(fake.borrow().drops, 1);
            input.replace_and_mark_text_in_range(None, "日本", None, window, cx);
            input.replace_text_in_range(None, "日本語", window, cx);
            assert_eq!(input.content, "Hello 日本語");
            input.undo(&Undo, window, cx);
            assert_eq!(input.content, "Hello ");
            input.undo(&Undo, window, cx);
            assert_eq!(input.content, "");
        })
        .unwrap();
}

#[gpui::test]
fn dictation_selection_move_and_undo_during_permissions_cancel(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_global(Theme::dark()));
    let window = cx.add_window(|_, cx| ComposerInput::new("Draft", cx));
    window
        .update(cx, |input, window, cx| {
            input.set_text("draft", cx);
            let fake = start(input, cx);
            input.undo(&Undo, window, cx); // No history is still a deliberate cancellation.
            assert_eq!(fake.borrow().drops, 1);
            let fake = start(input, cx);
            deliver(input, &fake, [Event::Partial(" words".into())], cx);
            input.move_to(0, cx);
            assert_eq!(fake.borrow().drops, 1);
            assert_eq!(input.content, "draft words");
            assert_eq!(input.selected_range, 0..0);
        })
        .unwrap();
}

#[gpui::test]
fn dictation_send_finalizes_once_and_keeps_latest_final(cx: &mut TestAppContext) {
    let input = cx.new(|cx| ComposerInput::new("Draft", cx));
    let mut events = cx.events::<DictationInputEvent, _>(&input);
    input.update(cx, |input, cx| {
        let fake = start(input, cx);
        deliver(
            input,
            &fake,
            [Event::Listening, Event::Partial("hel".into())],
            cx,
        );
        assert!(input.finish_dictation(true, cx));
        assert!(input.finish_dictation(true, cx));
        assert!(input.finish_dictation(false, cx));
        assert_eq!(fake.borrow().finishes, 1);
        deliver(
            input,
            &fake,
            [
                Event::Final("hello".into()),
                Event::Final("duplicate".into()),
            ],
            cx,
        );
        assert_eq!(input.content, "hello");
        assert_eq!(input.dictation.phase, Phase::Idle);
        assert_eq!(fake.borrow().drops, 1);
    });
    let mut sends = 0;
    while let Ok(event) = events.try_recv() {
        if matches!(event, DictationInputEvent::Submit(_)) {
            sends += 1;
        }
    }
    assert_eq!(sends, 1);
}

#[gpui::test]
fn dictation_failure_cancels_send_and_retry_preserves_draft(cx: &mut TestAppContext) {
    let input = cx.new(|cx| ComposerInput::new("Draft", cx));
    let mut events = cx.events::<DictationInputEvent, _>(&input);
    input.update(cx, |input, cx| {
        let fake = start(input, cx);
        deliver(input, &fake, [Event::Partial("keep me".into())], cx);
        input.finish_dictation(true, cx);
        deliver(input, &fake, [Event::Failed("Disconnected".into())], cx);
        assert_eq!(input.content, "keep me");
        assert_eq!(input.dictation.phase, Phase::Failed("Disconnected".into()));
        assert_eq!(fake.borrow().drops, 1);
        for error in [
            Event::Denied("Permission denied".into()),
            Event::Unavailable("Locale unavailable".into()),
        ] {
            let fake = start(input, cx);
            deliver(input, &fake, [error], cx);
            assert_eq!(input.content, "keep me");
            assert!(!input.dictation.phase.active());
            assert_eq!(fake.borrow().drops, 1);
        }
        let fake = start(input, cx);
        deliver(
            input,
            &fake,
            [
                Event::Partial(" again".into()),
                Event::Final(" again".into()),
            ],
            cx,
        );
        assert_eq!(input.content, "keep me again");
    });
    while let Ok(event) = events.try_recv() {
        assert!(!matches!(event, DictationInputEvent::Submit(_)));
    }
}

#[gpui::test]
fn dictation_empty_final_retains_selection_or_partial(cx: &mut TestAppContext) {
    let input = cx.new(|cx| ComposerInput::new("Draft", cx));
    input.update(cx, |input, cx| {
        for partial in [None, Some("replacement")] {
            input.set_text("selected", cx);
            input.selected_range = 0..8;
            let fake = start(input, cx);
            if let Some(text) = partial {
                deliver(input, &fake, [Event::Partial(text.into())], cx);
            }
            input.finish_dictation(false, cx);
            deliver(input, &fake, [Event::Final(String::new())], cx);
            assert_eq!(input.content, partial.unwrap_or("selected"));
            assert_eq!(input.undo_stack.len(), usize::from(partial.is_some()));
        }
    });
}

#[gpui::test]
fn dictation_draft_swap_invalidates_permissions_finalization_and_stale_results(
    cx: &mut TestAppContext,
) {
    let input = cx.new(|cx| ComposerInput::new("Draft", cx));
    let mut events = cx.events::<DictationInputEvent, _>(&input);
    input.update(cx, |input, cx| {
        for finalizing in [false, true] {
            input.set_text("original", cx);
            let fake = start(input, cx);
            let generation = input.dictation.generation;
            if finalizing {
                input.finish_dictation(true, cx);
            }
            input.set_text("different chat / queued draft", cx);
            fake.borrow_mut()
                .events
                .extend([Event::Listening, Event::Final("late".into())]);
            assert!(!input.poll_dictation(generation, cx));
            assert_eq!(fake.borrow().drops, 1);
            assert_eq!(input.content, "different chat / queued draft");
            assert!(input.undo_stack.is_empty());
        }
    });
    while let Ok(event) = events.try_recv() {
        assert!(!matches!(event, DictationInputEvent::Submit(_)));
    }
}

#[gpui::test]
fn dictation_cancelled_native_capture_drops_pending_send(cx: &mut TestAppContext) {
    let input = cx.new(|cx| ComposerInput::new("Draft", cx));
    let mut events = cx.events::<DictationInputEvent, _>(&input);
    input.update(cx, |input, cx| {
        let fake = start(input, cx);
        deliver(input, &fake, [Event::Partial("keep draft".into())], cx);
        input.finish_dictation(true, cx);
        deliver(input, &fake, [Event::Cancelled], cx);
        assert_eq!(fake.borrow().drops, 1);
        assert_eq!(input.content, "keep draft");
        assert_eq!(input.dictation.phase, Phase::Idle);
    });
    while let Ok(event) = events.try_recv() {
        assert!(!matches!(event, DictationInputEvent::Submit(_)));
    }
}

#[gpui::test]
fn dictation_releasing_input_releases_microphone(cx: &mut TestAppContext) {
    let input = cx.new(|cx| ComposerInput::new("Draft", cx));
    let fake = input.update(cx, |input, cx| start(input, cx));
    drop(input);
    cx.update(|_| {}); // Entity cleanup runs at the end of the update.
    assert_eq!(fake.borrow().drops, 1);
}

#[gpui::test]
fn dictation_empty_send_does_not_interrupt_or_send(cx: &mut TestAppContext) {
    let state = cx.new(|_| AppState::new());
    let composer = cx.new(|cx| Composer::new(state, cx));
    composer.update(cx, |composer, cx| {
        composer.input.update(cx, |input, cx| {
            let fake = start(input, cx);
            input.finish_dictation(true, cx);
            deliver(input, &fake, [Event::Final(String::new())], cx);
        });
    });
    composer.read_with(cx, |composer, _| {
        assert!(
            composer.failure.is_none(),
            "must not attempt a send to the disconnected engine"
        );
        assert!(!composer.sending);
        assert!(composer.interrupting.is_empty());
    });
}

#[gpui::test]
fn dictation_timeout_preserves_latest_partial_without_sending(cx: &mut TestAppContext) {
    let input = cx.new(|cx| ComposerInput::new("Draft", cx));
    let mut events = cx.events::<DictationInputEvent, _>(&input);
    input.update(cx, |input, cx| {
        let fake = start(input, cx);
        deliver(input, &fake, [Event::Partial("latest".into())], cx);
        // Drive the same deadline without a wall-clock sleep or real audio.
        input
            .dictation
            .finish(true, Instant::now() - crate::dictation::FINALIZE_TIMEOUT);
        let generation = input.dictation.generation;
        assert!(!input.poll_dictation(generation, cx));
        assert!(!input.poll_dictation(generation, cx));
        assert_eq!(input.content, "latest");
        assert_eq!(fake.borrow().drops, 1);
    });
    let mut sends = 0;
    while let Ok(event) = events.try_recv() {
        if matches!(event, DictationInputEvent::Submit(_)) {
            sends += 1;
        }
    }
    assert_eq!(sends, 0);
}

#[gpui::test]
fn dictation_mention_and_slash_completion_commit_partial_first(cx: &mut TestAppContext) {
    let input = cx.new(|cx| ComposerInput::new("Draft", cx));
    input.update(cx, |input, cx| {
        input.enable_mentions();
        input.set_text("@sr", cx);
        let fake = start(input, cx);
        deliver(input, &fake, [Event::Partial(" extra".into())], cx);
        input.replace_mention(0..3, "src/lib.rs", false, cx);
        assert_eq!(fake.borrow().drops, 1);
        assert!(input.content.contains("src/lib.rs"));
        assert!(input.content.ends_with(" extra"));
        input.set_text("/com", cx);
        let fake = start(input, cx);
        input.replace_plain_token(0..4, "/compact", cx);
        assert_eq!(fake.borrow().drops, 1);
        assert_eq!(input.content, "/compact ");
    });
}

#[gpui::test]
fn dictation_queue_save_waits_for_final_and_cancel_drops_capture(cx: &mut TestAppContext) {
    let state = cx.new(|_| AppState::new());
    let composer = cx.new(|cx| Composer::new(state, cx));
    let fake = composer.update(cx, |composer, cx| {
        composer.editing_queued = Some("queued-row".into());
        let fake = composer.input.update(cx, |input, cx| {
            let fake = start(input, cx);
            deliver(
                input,
                &fake,
                [Event::Listening, Event::Partial("draft".into())],
                cx,
            );
            fake
        });
        assert!(composer.commit_queue_edit(cx));
        assert!(composer.commit_queue_edit(cx));
        assert!(
            composer.failure.is_none(),
            "queue save must wait for the final result"
        );
        assert_eq!(fake.borrow().finishes, 1);
        fake
    });
    composer.update(cx, |composer, cx| {
        composer.input.update(cx, |input, cx| {
            deliver(input, &fake, [Event::Final("final queue draft".into())], cx)
        });
    });
    composer.read_with(cx, |composer, cx| {
        assert_eq!(composer.input.read(cx).text(), "final queue draft");
        // A disconnected test host reports a lease error only AFTER finalization.
        assert!(
            composer
                .failure
                .as_ref()
                .is_some_and(|s| s.contains("lease"))
        );
    });
    composer.update(cx, |composer, cx| {
        let fake = composer.input.update(cx, |input, cx| start(input, cx));
        composer.cancel_queue_edit(cx);
        assert_eq!(fake.borrow().drops, 1);
        let fake = composer.input.update(cx, |input, cx| start(input, cx));
        composer.clear_queue_edit(cx);
        assert_eq!(
            fake.borrow().drops,
            1,
            "clearing an edit without a saved draft must still stop capture"
        );
    });
}

#[gpui::test]
fn dictation_completed_send_event_cannot_send_a_replacement_draft(cx: &mut TestAppContext) {
    let state = cx.new(|_| AppState::new());
    let composer = cx.new(|cx| Composer::new(state, cx));
    composer.update(cx, |composer, cx| {
        composer.input.update(cx, |input, cx| {
            let fake = start(input, cx);
            input.finish_dictation(true, cx);
            deliver(input, &fake, [Event::Final("old draft".into())], cx);
            // The Submit effect is queued; invalidate it before effects flush.
            input.set_text("new draft", cx);
        });
    });
    composer.read_with(cx, |composer, cx| {
        assert!(composer.failure.is_none());
        assert!(!composer.sending);
        assert_eq!(composer.input.read(cx).text(), "new draft");
    });
}

#[cfg(target_os = "macos")]
#[gpui::test]
fn dictation_shortcut_holds_until_its_modifier_is_released(cx: &mut TestAppContext) {
    let (_dir, handle) = super::tests::composer_focus_window(cx);
    cx.update(|cx| {
        crate::shell::apply_keymap(
            cx,
            &crate::settings::KeymapConfig::default(),
            ComposerSendBehavior::Enter,
        )
    });
    let fake = handle
        .update(cx, |composer, _, cx| {
            composer.input.update(cx, |input, cx| {
                let fake = start(input, cx);
                deliver(
                    input,
                    &fake,
                    [Event::Listening, Event::Partial("keyboard".into())],
                    cx,
                );
                fake
            })
        })
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| window.draw(cx).clear())
        .unwrap();
    // Key down and its auto-repeat hold the recording open.
    cx.simulate_keystrokes(handle.into(), "cmd-d cmd-d");
    cx.run_until_parked();
    assert_eq!(fake.borrow().finishes, 0);
    let mut visual = gpui::VisualTestContext::from_window(handle.into(), cx);
    visual.simulate_modifiers_change(gpui::Modifiers::command());
    assert_eq!(
        fake.borrow().finishes,
        0,
        "D up alone never arrives on macOS"
    );
    // Letting go of Command ends the hold.
    visual.simulate_modifiers_change(gpui::Modifiers::none());
    assert_eq!(fake.borrow().finishes, 1);
    handle
        .update(cx, |composer, window, cx| {
            assert!(composer.input.read(cx).focus_handle.is_focused(window));
            assert_eq!(composer.input.read(cx).dictation.phase, Phase::Finalizing);
            composer.input.update(cx, |input, cx| {
                deliver(input, &fake, [Event::Final("keyboard".into())], cx)
            });
        })
        .unwrap();
}

#[gpui::test]
fn dictation_pointer_hold_finishes_on_release_even_outside_the_button(cx: &mut TestAppContext) {
    let (dir, handle) = super::tests::composer_focus_window(cx);
    enable_dictation(dir.path(), cx);
    for release_outside in [false, true] {
        let fake = handle
            .update(cx, |composer, _, cx| {
                composer.input.update(cx, |input, cx| {
                    let fake = start(input, cx);
                    deliver(input, &fake, [Event::Listening], cx);
                    fake
                })
            })
            .unwrap();
        cx.update_window(handle.into(), |_, window, cx| window.draw(cx).clear())
            .unwrap();
        let mut visual = gpui::VisualTestContext::from_window(handle.into(), cx);
        let button = visual.debug_bounds("composer-dictation").unwrap();
        visual.simulate_mouse_down(
            button.center(),
            MouseButton::Left,
            gpui::Modifiers::default(),
        );
        assert_eq!(fake.borrow().finishes, 0, "holding keeps recording");
        let up = if release_outside {
            button.origin - point(px(40.0), px(40.0))
        } else {
            button.center()
        };
        visual.simulate_mouse_up(up, MouseButton::Left, gpui::Modifiers::default());
        assert_eq!(fake.borrow().drops, 0, "release must finish, not cancel");
        assert_eq!(fake.borrow().finishes, 1, "outside={release_outside}");
        handle
            .update(cx, |composer, _, cx| {
                composer.input.update(cx, |input, cx| {
                    assert_eq!(input.dictation.phase, Phase::Finalizing);
                    deliver(input, &fake, [Event::Final("Bonjour".into())], cx);
                });
            })
            .unwrap();
    }
}

#[gpui::test]
fn dictation_tap_explains_hold_to_talk_without_transcribing(cx: &mut TestAppContext) {
    let (dir, handle) = super::tests::composer_focus_window(cx);
    enable_dictation(dir.path(), cx);
    let fake = handle
        .update(cx, |composer, _, cx| {
            let fake = composer.input.update(cx, |input, cx| {
                input.set_text("keep", cx);
                let fake = start(input, cx);
                deliver(input, &fake, [Event::Listening], cx);
                fake
            });
            // As if this press had just started the session.
            composer.dictation_hold = Some(DictationHold {
                source: HoldSource::Pointer,
                started: Some(Instant::now()),
            });
            fake
        })
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| window.draw(cx).clear())
        .unwrap();
    let mut visual = gpui::VisualTestContext::from_window(handle.into(), cx);
    let button = visual.debug_bounds("composer-dictation").unwrap();
    visual.simulate_mouse_up(
        button.center(),
        MouseButton::Left,
        gpui::Modifiers::default(),
    );
    assert_eq!(fake.borrow().finishes, 0);
    assert_eq!(fake.borrow().drops, 1, "a tap releases the microphone");
    handle
        .read_with(cx, |composer, cx| {
            let input = composer.input.read(cx);
            assert_eq!(input.dictation.phase, Phase::Tapped);
            assert_eq!(input.text(), "keep");
            assert!(composer.dictation_hold.is_none());
        })
        .unwrap();
}

#[gpui::test]
fn dictation_switching_windows_releases_capture_and_retains_draft(cx: &mut TestAppContext) {
    let (_dir, handle) = super::tests::composer_focus_window(cx);
    handle
        .update(cx, |_, window, _| window.activate_window())
        .unwrap();
    cx.run_until_parked();
    let fake = handle
        .update(cx, |composer, _, cx| {
            composer.input.update(cx, |input, cx| {
                let fake = start(input, cx);
                deliver(
                    input,
                    &fake,
                    [Event::Listening, Event::Partial("keep on switch".into())],
                    cx,
                );
                input.finish_dictation(true, cx);
                fake
            })
        })
        .unwrap();
    let other = cx.add_window(|_, cx| ComposerInput::new("Other window", cx));
    other
        .update(cx, |_, window, _| window.activate_window())
        .unwrap();
    cx.run_until_parked();
    assert_eq!(fake.borrow().drops, 1);
    handle
        .read_with(cx, |composer, cx| {
            assert_eq!(composer.input.read(cx).text(), "keep on switch");
            assert_eq!(composer.input.read(cx).dictation.phase, Phase::Idle);
            assert!(composer.failure.is_none());
        })
        .unwrap();
}

#[gpui::test]
fn dictation_question_takeover_stops_capture_without_losing_draft(cx: &mut TestAppContext) {
    let state = cx.new(|_| AppState::new());
    let composer = cx.new(|cx| Composer::new(state.clone(), cx));
    let fake = composer.update(cx, |composer, cx| {
        composer.input.update(cx, |input, cx| {
            let fake = start(input, cx);
            deliver(input, &fake, [Event::Partial("preserved draft".into())], cx);
            fake
        })
    });
    state.update(cx, |state, cx| {
        state.transcript = vec![SessionMessageEntry {
            duration_ms: None,
            native_fork_point: None,
            id: "question-message".into(),
            role: MessageRole::Assistant,
            created_at: 0,
            device_id: "host".into(),
            status: None,
            continuation_of: None,
            parts: vec![MessagePart::Input {
                id: "input".into(),
                request_id: "request".into(),
                resolved: false,
                questions: vec![UserInputQuestion {
                    id: "q".into(),
                    header: "Choose".into(),
                    question: "Continue?".into(),
                    options: vec!["Yes".into()],
                    multi_select: false,
                    prefill: None,
                    multiline: false,
                }],
            }],
        }];
        cx.notify();
    });
    assert_eq!(fake.borrow().drops, 1);
    composer.read_with(cx, |composer, cx| {
        assert!(composer.wizard.is_some());
        // The question borrows the editor; the dictated draft waits under the
        // chat's key until the question closes.
        assert_eq!(composer.input.read(cx).text(), "");
        assert_eq!(
            composer
                .drafts
                .get(&composer.current_key)
                .map(String::as_str),
            Some("preserved draft")
        );
    });
}

#[gpui::test]
fn dictation_pointer_stop_keeps_capture_until_final_result(cx: &mut TestAppContext) {
    let (dir, handle) = super::tests::composer_focus_window(cx);
    enable_dictation(dir.path(), cx);
    let fake = handle
        .update(cx, |composer, _, cx| {
            composer.input.update(cx, |input, cx| {
                let fake = start(input, cx);
                deliver(input, &fake, [Event::Listening], cx);
                fake
            })
        })
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| window.draw(cx).clear())
        .unwrap();
    let mut visual = gpui::VisualTestContext::from_window(handle.into(), cx);
    let button = visual.debug_bounds("composer-dictation").unwrap();
    visual.simulate_click(button.center(), gpui::Modifiers::default());
    assert_eq!(
        fake.borrow().drops,
        0,
        "Stop must finish, not cancel, the recording"
    );
    assert_eq!(fake.borrow().finishes, 1);
    handle
        .update(cx, |composer, _, cx| {
            composer.input.update(cx, |input, cx| {
                assert_eq!(input.dictation.phase, Phase::Finalizing);
                deliver(input, &fake, [Event::Final("Bonjour".into())], cx);
                assert_eq!(input.text(), "Bonjour");
            })
        })
        .unwrap();
}

#[gpui::test]
fn dictation_pointer_send_waits_for_final_and_submits_once(cx: &mut TestAppContext) {
    let (_dir, handle) = super::tests::composer_focus_window(cx);
    let (input, fake) = handle
        .update(cx, |composer, _, cx| {
            let fake = composer.input.update(cx, |input, cx| {
                let fake = start(input, cx);
                deliver(
                    input,
                    &fake,
                    [Event::Listening, Event::Partial("unfinished words".into())],
                    cx,
                );
                fake
            });
            (composer.input.clone(), fake)
        })
        .unwrap();
    let mut events = cx.events::<DictationInputEvent, _>(&input);
    cx.update_window(handle.into(), |_, window, cx| window.draw(cx).clear())
        .unwrap();
    let mut visual = gpui::VisualTestContext::from_window(handle.into(), cx);
    let button = visual.debug_bounds("composer-send").unwrap();
    visual.simulate_click(button.center(), gpui::Modifiers::default());
    visual.simulate_click(button.center(), gpui::Modifiers::default());
    assert_eq!(fake.borrow().drops, 0);
    assert_eq!(fake.borrow().finishes, 1);
    handle
        .read_with(cx, |composer, cx| {
            assert!(
                composer.failure.is_none(),
                "neither click may attempt an early send"
            );
            assert_eq!(composer.input.read(cx).text(), "unfinished words");
        })
        .unwrap();
    input.update(cx, |input, cx| {
        assert_eq!(input.dictation.phase, Phase::Finalizing);
        deliver(input, &fake, [Event::Final("send this once".into())], cx);
    });
    let mut sends = 0;
    while let Ok(event) = events.try_recv() {
        if matches!(event, DictationInputEvent::Submit(_)) {
            sends += 1;
        }
    }
    assert_eq!(sends, 1);
}

#[gpui::test]
fn dictation_silence_explains_empty_result_and_never_sends_existing_draft(cx: &mut TestAppContext) {
    let input = cx.new(|cx| ComposerInput::new("Draft", cx));
    let mut events = cx.events::<DictationInputEvent, _>(&input);
    input.update(cx, |input, cx| {
        input.set_text("keep my draft", cx);
        let fake = start(input, cx);
        deliver(input, &fake, [Event::Listening], cx);
        input.finish_dictation(true, cx);
        deliver(input, &fake, [Event::Final(String::new())], cx);
        assert_eq!(input.text(), "keep my draft");
        assert_eq!(input.dictation.phase, Phase::NoSpeech);
    });
    while let Ok(event) = events.try_recv() {
        assert!(!matches!(event, DictationInputEvent::Submit(_)));
    }
}

fn enable_dictation(dir: &std::path::Path, cx: &mut TestAppContext) {
    let model = dir.join("models/parakeet-tdt-0.6b-v3-int8");
    std::fs::create_dir_all(&model).unwrap();
    for file in zeron_voice::manifest().files {
        std::fs::File::create(model.join(file.name))
            .unwrap()
            .set_len(file.size)
            .unwrap();
    }
    std::fs::write(model.join("verified"), zeron_voice::manifest().revision).unwrap();
    cx.update(|cx| {
        crate::dictation::init(dir.into(), cx);
        crate::settings::update(crate::settings::SavePolicy::Immediate, cx, |s| {
            s.dictation_enabled = true
        });
    });
}

#[gpui::test]
fn dictation_focus_within_composer_keeps_capture_but_leaving_cancels(cx: &mut TestAppContext) {
    let (dir, handle) = super::tests::composer_focus_window(cx);
    enable_dictation(dir.path(), cx);
    handle
        .update(cx, |_, window, _| window.activate_window())
        .unwrap();
    cx.run_until_parked();
    let fake = handle
        .update(cx, |composer, _, cx| {
            composer.input.update(cx, |input, cx| {
                let fake = start(input, cx);
                deliver(input, &fake, [Event::Listening], cx);
                fake
            })
        })
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| window.draw(cx).clear())
        .unwrap();
    handle
        .update(cx, |composer, window, cx| {
            // The composer surface is the ancestor of the editor and its controls.
            window.focus(&composer.dictation_focus, cx);
        })
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| window.draw(cx).clear())
        .unwrap();
    cx.run_until_parked();
    assert_eq!(
        fake.borrow().drops,
        0,
        "focus within the composer must retain capture"
    );
    let elsewhere = cx.update(|cx| cx.focus_handle());
    handle
        .update(cx, |composer, window, cx| {
            window.focus(&elsewhere, cx);
            assert!(composer.input.read(cx).dictation.phase.active());
        })
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| window.draw(cx).clear())
        .unwrap();
    cx.run_until_parked();
    assert_eq!(fake.borrow().drops, 1);
    handle
        .read_with(cx, |composer, cx| {
            assert_eq!(composer.input.read(cx).dictation.phase, Phase::Idle)
        })
        .unwrap();
}

#[gpui::test]
fn dictation_cancel_control_preserves_draft_and_clears_pending_send(cx: &mut TestAppContext) {
    let (_dir, handle) = super::tests::composer_focus_window(cx);
    let (input, fake) = handle
        .update(cx, |composer, _, cx| {
            let fake = composer.input.update(cx, |input, cx| {
                input.set_text("Keep this draft", cx);
                let fake = start(input, cx);
                deliver(input, &fake, [Event::Listening], cx);
                input.finish_dictation(true, cx);
                fake
            });
            (composer.input.clone(), fake)
        })
        .unwrap();
    let mut events = cx.events::<DictationInputEvent, _>(&input);
    cx.update_window(handle.into(), |_, window, cx| window.draw(cx).clear())
        .unwrap();
    let mut visual = gpui::VisualTestContext::from_window(handle.into(), cx);
    let cancel = visual.debug_bounds("dictation-dismiss").unwrap();
    visual.simulate_click(cancel.center(), gpui::Modifiers::default());
    assert_eq!(fake.borrow().drops, 1);
    input.read_with(cx, |input, _| {
        assert_eq!(input.text(), "Keep this draft");
        assert_eq!(input.dictation.phase, Phase::Idle);
    });
    while let Ok(event) = events.try_recv() {
        assert!(!matches!(event, DictationInputEvent::Submit(_)));
    }
}

#[gpui::test]
fn dictation_cancel_control_is_keyboard_operable(cx: &mut TestAppContext) {
    let (_dir, handle) = super::tests::composer_focus_window(cx);
    let fake = handle
        .update(cx, |composer, window, cx| {
            let fake = composer.input.update(cx, |input, cx| {
                input.set_text("Keep this draft", cx);
                let fake = start(input, cx);
                deliver(input, &fake, [Event::Listening], cx);
                fake
            });
            window.focus(&composer.input.focus_handle(cx), cx);
            fake
        })
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.draw(cx).clear();
        window.focus_next(cx);
        window.draw(cx).clear();
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(
        fake.borrow().drops,
        0,
        "tabbing to Cancel must retain capture"
    );
    cx.simulate_keystrokes(handle.into(), "space");
    cx.run_until_parked();
    assert_eq!(fake.borrow().drops, 1);
    assert_eq!(fake.borrow().finishes, 0, "Cancel must not transcribe");
    handle
        .read_with(cx, |composer, cx| {
            assert_eq!(composer.input.read(cx).text(), "Keep this draft");
            assert_eq!(composer.input.read(cx).dictation.phase, Phase::Idle);
        })
        .unwrap();
}

#[gpui::test]
fn dictation_keeps_draft_stable_when_pasted_references_resolve(cx: &mut TestAppContext) {
    let input = cx.new(|cx| ComposerInput::new("Draft", cx));
    let mut events = cx.events::<DictationInputEvent, _>(&input);
    input.update(cx, |input, cx| {
        input.set_text("@README.md ", cx);
        let original = input.text().to_owned();
        let revision = input.edit_revision;
        let fake = start(input, cx);
        deliver(input, &fake, [Event::Listening], cx);
        input.finish_dictation(true, cx);
        input.apply_pasted_references(
            &original,
            original.len(),
            revision,
            vec![(0..10, local_file_link("README.md", false))],
            cx,
        );
        assert_eq!(input.text(), original);
        deliver(input, &fake, [Event::Final("explain this".into())], cx);
        assert_eq!(input.text(), "@README.md explain this");
    });
    let mut sends = 0;
    while let Ok(event) = events.try_recv() {
        if matches!(event, DictationInputEvent::Submit(_)) {
            sends += 1;
        }
    }
    assert_eq!(sends, 1);
}

#[gpui::test]
fn dictation_escape_from_composer_controls_cancels_pending_send(cx: &mut TestAppContext) {
    let (_dir, handle) = super::tests::composer_focus_window(cx);
    let fake = handle
        .update(cx, |composer, window, cx| {
            let fake = composer.input.update(cx, |input, cx| {
                input.set_text("Keep this draft", cx);
                let fake = start(input, cx);
                deliver(input, &fake, [Event::Listening], cx);
                input.finish_dictation(true, cx);
                fake
            });
            window.focus(&composer.dictation_focus, cx);
            fake
        })
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| window.draw(cx).clear())
        .unwrap();
    cx.simulate_keystrokes(handle.into(), "escape");
    cx.run_until_parked();
    assert_eq!(fake.borrow().drops, 1);
    handle
        .read_with(cx, |composer, cx| {
            assert_eq!(composer.input.read(cx).dictation.phase, Phase::Idle);
            assert_eq!(composer.input.read(cx).text(), "Keep this draft");
            assert!(composer.failure.is_none());
        })
        .unwrap();
}

#[gpui::test]
fn dictation_empty_undo_and_redo_notify_cancellation(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_global(Theme::dark()));
    let window = cx.add_window(|_, cx| ComposerInput::new("Draft", cx));
    let input = window.update(cx, |_, _, cx| cx.entity()).unwrap();
    let mut events = cx.events::<DictationInputEvent, _>(&input);
    for redo in [false, true] {
        let fake = input.update(cx, |input, cx| {
            let fake = start(input, cx);
            deliver(input, &fake, [Event::Listening], cx);
            fake
        });
        while events.try_recv().is_ok() {}
        window
            .update(cx, |input, window, cx| {
                if redo {
                    input.redo(&Redo, window, cx);
                } else {
                    input.undo(&Undo, window, cx);
                }
                assert_eq!(input.dictation.phase, Phase::Idle);
            })
            .unwrap();
        assert_eq!(fake.borrow().drops, 1);
        assert!(matches!(
            events.try_recv(),
            Ok(DictationInputEvent::Changed)
        ));
    }
}

#[gpui::test]
fn dictation_blank_results_preserve_partial_and_pending_send(cx: &mut TestAppContext) {
    let input = cx.new(|cx| ComposerInput::new("Draft", cx));
    let mut events = cx.events::<DictationInputEvent, _>(&input);
    input.update(cx, |input, cx| {
        let fake = start(input, cx);
        deliver(
            input,
            &fake,
            [Event::Partial("Keep these words".into())],
            cx,
        );
        deliver(input, &fake, [Event::Partial("   ".into())], cx);
        assert_eq!(input.text(), "Keep these words");
        input.finish_dictation(true, cx);
        deliver(input, &fake, [Event::Final(" \n\t ".into())], cx);
        assert_eq!(input.text(), "Keep these words");
        assert_eq!(input.dictation.phase, Phase::Idle);
    });
    let mut sends = 0;
    while let Ok(event) = events.try_recv() {
        if matches!(event, DictationInputEvent::Submit(_)) {
            sends += 1;
        }
    }
    assert_eq!(sends, 1);
}

#[test]
fn voice_morph_reverses_mid_flight_without_jumping() {
    let start = std::time::Instant::now();
    let mut tween = super::VoiceTween::default();
    tween.retarget(1.0, start, false);
    let mid = start + super::VOICE_MORPH / 3;
    let at_reverse = tween.value(mid);
    assert!(at_reverse > 0.0 && at_reverse < 1.0, "{at_reverse}");
    tween.retarget(0.0, mid, false);
    assert!((tween.value(mid) - at_reverse).abs() < 1e-6);
    assert_eq!(tween.value(mid + super::VOICE_MORPH), 0.0);
    // Reduced motion snaps to the target.
    tween.retarget(1.0, mid, true);
    assert_eq!(tween.value(mid), 1.0);
}

#[gpui::test]
fn dictation_stop_while_loading_keeps_selection_until_final_and_sends_once(
    cx: &mut TestAppContext,
) {
    let input = cx.new(|cx| ComposerInput::new("Draft", cx));
    let mut events = cx.events::<DictationInputEvent, _>(&input);
    input.update(cx, |input, cx| {
        input.set_text("keep selected suffix", cx);
        input.selected_range = 5..13;
        let fake = start(input, cx);
        assert_eq!(input.dictation.phase, Phase::Requesting);
        deliver(input, &fake, [Event::Listening], cx);
        input.finish_dictation(false, cx);
        input.finish_dictation(true, cx);
        // A queued readiness event cannot put a stopped session back in Listening.
        deliver(input, &fake, [Event::Listening, Event::Finalizing], cx);
        assert_eq!(input.dictation.phase, Phase::Finalizing);
        assert_eq!(input.content, "keep selected suffix");
        assert_eq!(input.selected_range, 5..13);
        assert_eq!(fake.borrow().finishes, 1);
        deliver(
            input,
            &fake,
            [Event::Final("speech".into()), Event::Final("late".into())],
            cx,
        );
        assert_eq!(input.content, "keep speech suffix");
        assert_eq!(input.dictation.phase, Phase::Idle);
        assert_eq!(fake.borrow().drops, 1);
    });
    let mut sends = 0;
    while let Ok(event) = events.try_recv() {
        sends += usize::from(matches!(event, DictationInputEvent::Submit(_)));
    }
    assert_eq!(sends, 1);
}

#[gpui::test]
fn dictation_load_failure_or_timeout_preserves_selected_draft_and_rejects_late_final(
    cx: &mut TestAppContext,
) {
    let input = cx.new(|cx| ComposerInput::new("Draft", cx));
    let mut events = cx.events::<DictationInputEvent, _>(&input);
    input.update(cx, |input, cx| {
        for timeout in [false, true] {
            input.set_text("keep my draft", cx);
            input.selected_range = 0..13;
            let fake = start(input, cx);
            let generation = input.dictation.generation;
            deliver(input, &fake, [Event::Listening], cx);
            if timeout {
                input
                    .dictation
                    .finish(true, Instant::now() - crate::dictation::FINALIZE_TIMEOUT);
                fake.borrow_mut().events.push_back(Event::Finalizing);
                input.poll_dictation(generation, cx);
            } else {
                input.finish_dictation(true, cx);
                deliver(
                    input,
                    &fake,
                    [Event::Failed("Could not load the model".into())],
                    cx,
                );
            }
            assert!(matches!(input.dictation.phase, Phase::Failed(_)));
            fake.borrow_mut()
                .events
                .push_back(Event::Final("late speech".into()));
            assert!(!input.poll_dictation(generation, cx));
            assert_eq!(input.content, "keep my draft");
            assert_eq!(input.selected_range, 0..13);
            assert!(input.undo_stack.is_empty());
            assert_eq!(fake.borrow().drops, 1);
        }
    });
    while let Ok(event) = events.try_recv() {
        assert!(!matches!(event, DictationInputEvent::Submit(_)));
    }
}

#[gpui::test]
fn dictation_microphone_key_hold_keeps_focus_and_queue_lease_blocks_start(cx: &mut TestAppContext) {
    let (dir, handle) = super::tests::composer_focus_window(cx);
    enable_dictation(dir.path(), cx);
    handle
        .update(cx, |composer, _, cx| {
            // Enter/Space on the focused microphone must leave focus there,
            // or its key-up never reaches the hold.
            composer.focus_pending = false;
            composer.press_dictation(HoldSource::Button, cx);
            assert!(composer.input.read(cx).dictation.phase.active());
            assert!(!composer.focus_pending);
            composer
                .input
                .update(cx, |input, _| input.cancel_dictation());
            composer.dictation_hold = None;
            // An edit lease still in flight is about to replace the draft.
            composer.queue_edit_pending_id = Some("row".into());
            composer.press_dictation(HoldSource::Pointer, cx);
            assert!(!composer.input.read(cx).dictation.phase.active());
            assert!(composer.dictation_hold.is_none());
        })
        .unwrap();
}

#[gpui::test]
fn dictation_another_source_takes_over_a_hold_whose_release_was_lost(cx: &mut TestAppContext) {
    let (dir, handle) = super::tests::composer_focus_window(cx);
    enable_dictation(dir.path(), cx);
    let fake = handle
        .update(cx, |composer, _, cx| {
            let fake = composer.input.update(cx, |input, cx| {
                let fake = start(input, cx);
                deliver(input, &fake, [Event::Listening], cx);
                fake
            });
            composer.dictation_hold = Some(DictationHold {
                source: HoldSource::Button,
                started: None,
            });
            composer.press_dictation(HoldSource::Pointer, cx);
            composer.release_dictation(HoldSource::Pointer, cx);
            fake
        })
        .unwrap();
    assert_eq!(fake.borrow().finishes, 1);
}

#[gpui::test]
fn dictation_shortcut_falls_through_while_dictation_is_off(cx: &mut TestAppContext) {
    gpui::actions!(dictation_test, [Probe]);
    let (_dir, handle) = super::tests::composer_focus_window(cx);
    let fired = Rc::new(std::cell::Cell::new(false));
    let probe = fired.clone();
    cx.update(|cx| {
        crate::shell::apply_keymap(
            cx,
            &crate::settings::KeymapConfig::default(),
            ComposerSendBehavior::Enter,
        );
        // Another binding of the same chord, as a user may already have.
        cx.bind_keys([KeyBinding::new("secondary-d", Probe, None)]);
        cx.on_action(move |_: &Probe, _| probe.set(true));
    });
    cx.simulate_keystrokes(handle.into(), "secondary-d");
    assert!(
        fired.get(),
        "dictation is off by default and must not eat ⌘D"
    );
    handle
        .read_with(cx, |composer, cx| {
            assert!(composer.dictation_hold.is_none());
            assert!(composer.input.read(cx).dictation_key.is_none());
        })
        .unwrap();
}
