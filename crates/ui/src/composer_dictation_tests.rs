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
fn dictation_timeout_commits_latest_partial_and_sends_once(cx: &mut TestAppContext) {
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
    assert_eq!(sends, 1);
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
fn dictation_keyboard_stop_restores_editor_focus(cx: &mut TestAppContext) {
    let (_dir, handle) = super::tests::composer_focus_window(cx);
    cx.update(|cx| init(cx, ComposerSendBehavior::Enter));
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
    cx.simulate_keystrokes(handle.into(), "cmd-shift-d");
    cx.run_until_parked();
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
                }],
            }],
        }];
        cx.notify();
    });
    assert_eq!(fake.borrow().drops, 1);
    composer.read_with(cx, |composer, cx| {
        assert!(composer.wizard.is_some());
        assert_eq!(composer.input.read(cx).text(), "preserved draft");
    });
}
