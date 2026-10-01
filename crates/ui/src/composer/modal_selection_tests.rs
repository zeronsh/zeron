use super::*;
use crate::markdown::{render, selection};
use crate::popover;
use gpui::{StyledText, TestAppContext, VisualTestContext, canvas};

const BACKGROUND_KEY: &str = "modal-selection-background";
const BACKGROUND_TEXT: &str = "Transcript text behind the modal must never be selected.";
const INPUT_TEXT: &str = "Editable session title";

struct ModalSelectionHarness {
    input: Entity<ComposerInput>,
    modal_open: bool,
}

impl Render for ModalSelectionHarness {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let text: SharedString = BACKGROUND_TEXT.into();
        let styled = StyledText::new(text.clone());
        let layout = styled.layout().clone();
        let underlay = canvas(
            |bounds, window, _| window.insert_hitbox(bounds, gpui::HitboxBehavior::Normal),
            move |_, hitbox, window, _| {
                render::paint_text_selection(
                    window,
                    hitbox,
                    &BACKGROUND_KEY.into(),
                    &text,
                    &layout,
                    &theme,
                );
            },
        )
        .absolute()
        .size_full();
        let theme = Theme::of(cx);
        div()
            .size_full()
            .relative()
            .text_size(px(14.0))
            .line_height(px(22.0))
            .child(render::selection_frame_reset())
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        render::selectable_text_wrap()
                            .w(px(560.0))
                            .child(underlay)
                            .child(styled),
                    ),
            )
            .when(self.modal_open, |root| {
                root.child(popover::modal(
                    "selection-regression-modal",
                    window.viewport_size(),
                    popover::dialog_card(theme)
                        .child(popover::dialog_field(self.input.clone().into_any_element()))
                        .into_any_element(),
                ))
            })
    }
}

// Clean up even when a regression assertion panics: selection is process-global.
struct SelectionCleanup;

impl Drop for SelectionCleanup {
    fn drop(&mut self) {
        render::clear_selection_surface(BACKGROUND_KEY);
    }
}

fn fixture(
    cx: &mut TestAppContext,
    modal_open: bool,
) -> (Entity<ModalSelectionHarness>, &mut VisualTestContext) {
    cx.update(|cx| {
        cx.set_global(Theme::dark());
        motion::set_reduced_motion(cx, true);
    });
    let (view, cx) = cx.add_window_view(|_, cx| ModalSelectionHarness {
        input: cx.new(|cx| {
            let mut input = ComposerInput::new("Session title", cx);
            input.set_text(INPUT_TEXT, cx);
            input
        }),
        modal_open,
    });
    cx.simulate_resize(size(px(640.0), px(240.0)));
    draw(cx);
    (view, cx)
}

fn draw(cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        window.refresh();
        window.draw(cx).clear();
    });
}

fn drag(cx: &mut VisualTestContext, start: Point<Pixels>, end: Point<Pixels>) -> bool {
    cx.simulate_event(MouseDownEvent {
        button: MouseButton::Left,
        position: start,
        click_count: 1,
        ..Default::default()
    });
    let background_started_dragging = selection::is_dragging();
    cx.simulate_event(MouseMoveEvent {
        position: end,
        pressed_button: Some(MouseButton::Left),
        ..Default::default()
    });
    cx.simulate_event(MouseUpEvent {
        button: MouseButton::Left,
        position: end,
        ..Default::default()
    });
    background_started_dragging
}

#[gpui::test]
fn modal_input_drag_does_not_select_transcript(cx: &mut TestAppContext) {
    let _selection = selection::test_state_lock();
    let _cleanup = SelectionCleanup;
    let (view, cx) = fixture(cx, true);
    let input = cx.read(|cx| view.read(cx).input.clone());
    let input_bounds = cx.read(|cx| input.read(cx).last_bounds.expect("input must be painted"));
    let background_bounds = render::selection_test_bounds(BACKGROUND_KEY);
    let start = input_bounds.origin + point(px(5.0), px(8.0));
    let end = start + point(px(80.0), px(0.0));
    assert!(input_bounds.contains(&start) && input_bounds.contains(&end));
    assert!(
        background_bounds.contains(&start) && background_bounds.contains(&end),
        "the input gesture must overlap selectable transcript text"
    );

    let background_started_dragging = drag(cx, start, end);
    let background_selection = selection::selected_text();
    assert!(
        cx.read(|cx| !input.read(cx).selected_range.is_empty()),
        "dragging must still select text inside the modal input"
    );

    // Check the same gesture without the modal before asserting isolation, so
    // disabling selection altogether cannot make this regression test pass.
    selection::clear_if_owner(BACKGROUND_KEY);
    view.update(cx, |view, cx| {
        view.modal_open = false;
        cx.notify();
    });
    draw(cx);
    assert!(drag(cx, start, end));
    assert!(
        selection::selected_text().is_some(),
        "the transcript must remain selectable after closing the modal"
    );

    assert_eq!(
        (background_started_dragging, background_selection),
        (false, None),
        "a drag inside the modal must neither start nor retain a transcript selection"
    );
}

#[gpui::test]
fn modal_input_copy_does_not_copy_transcript_selection(cx: &mut TestAppContext) {
    let _selection = selection::test_state_lock();
    let _cleanup = SelectionCleanup;
    let (view, cx) = fixture(cx, false);
    let input = cx.read(|cx| view.read(cx).input.clone());

    // A selection made before opening the popup must not become the field's
    // Copy fallback. Seed it independently of the mouse-isolation regression.
    selection::begin_with_span(BACKGROUND_KEY, BACKGROUND_TEXT, 0..BACKGROUND_TEXT.len());
    selection::end_active_drag();
    assert_eq!(selection::selected_text().as_deref(), Some(BACKGROUND_TEXT));
    view.update(cx, |view, cx| {
        view.modal_open = true;
        cx.notify();
    });
    cx.update(|window, cx| {
        input.update(cx, |input, cx| {
            window.focus(&input.focus_handle, cx);
            input.select_all(&SelectAll, window, cx);
        });
    });
    draw(cx);
    cx.dispatch_action(Copy);
    assert_eq!(
        cx.read_from_clipboard().and_then(|item| item.text()),
        Some(INPUT_TEXT.to_string()),
        "Copy must reach the focused modal input and copy its own selection"
    );

    const SENTINEL: &str = "existing clipboard contents";
    cx.update(|_, cx| {
        input.update(cx, |input, cx| input.move_to(INPUT_TEXT.len(), cx));
        cx.write_to_clipboard(ClipboardItem::new_string(SENTINEL.to_string()));
    });
    draw(cx);
    assert!(cx.read(|cx| input.read(cx).selected_range.is_empty()));
    cx.dispatch_action(Copy);
    assert_eq!(
        cx.read_from_clipboard().and_then(|item| item.text()),
        Some(SENTINEL.to_string()),
        "Copy in a modal input without a selection must leave the clipboard unchanged"
    );
}
