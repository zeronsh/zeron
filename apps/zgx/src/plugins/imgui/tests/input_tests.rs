//! Pointer events → Dear ImGui IO: a real context, no desktop.
//!
//! The mapping is driven the way the window host drives it (`input::pointer`
//! with the framework's own `InputEvent`s), and the IO is read back after a
//! frame, which is when Dear ImGui applies queued input.

use dear_imgui_rs::MouseButton;
use zgx::input::{InputEvent, PointerButton};

use super::support::imgui_guard;
use crate::plugins::imgui::input;
use crate::plugins::imgui::runtime::{FrameSpec, WindowContext};

fn context() -> WindowContext {
    WindowContext::try_create("input test").expect("a context")
}

/// The window's size in logical points, as its frame reports it.
fn spec() -> FrameSpec {
    FrameSpec::from_device_pixels((520, 300), 1.0)
}

/// One frame, with nothing drawn: Dear ImGui applies what the events queued.
fn frame(context: &mut WindowContext) {
    context.build_frame(spec(), false, |_| {}).expect("a frame");
}

fn io<T>(context: &mut WindowContext, read: impl FnOnce(&dear_imgui_rs::Io) -> T) -> T {
    context
        .with_active(|context| read(context.io_mut()))
        .expect("an active context")
}

fn move_to(context: &mut WindowContext, x: f32, y: f32) -> bool {
    input::pointer(context, &InputEvent::PointerMove { x, y })
}

fn press(context: &mut WindowContext, button: PointerButton, down: bool) -> bool {
    let event = if down {
        InputEvent::PointerDown {
            x: 20.0,
            y: 30.0,
            button,
        }
    } else {
        InputEvent::PointerUp {
            x: 20.0,
            y: 30.0,
            button,
        }
    };
    input::pointer(context, &event)
}

#[test]
fn a_pointer_move_puts_the_pointer_where_the_window_says() {
    let _guard = imgui_guard();
    let mut context = context();
    assert!(
        move_to(&mut context, 20.0, 30.0),
        "a move changes what is drawn"
    );
    frame(&mut context);
    assert_eq!(io(&mut context, |io| io.mouse_pos()), [20.0, 30.0]);
}

#[test]
fn a_press_is_down_on_the_frame_after_it_and_up_after_the_release() {
    let _guard = imgui_guard();
    let mut context = context();
    move_to(&mut context, 20.0, 30.0);
    frame(&mut context);

    assert!(press(&mut context, PointerButton::Primary, true));
    frame(&mut context);
    assert!(io(&mut context, |io| io.mouse_down(MouseButton::Left)));
    assert!(
        !io(&mut context, |io| io.mouse_down(MouseButton::Right)),
        "the left press is not the right button"
    );

    assert!(press(&mut context, PointerButton::Primary, false));
    frame(&mut context);
    assert!(!io(&mut context, |io| io.mouse_down(MouseButton::Left)));
}

#[test]
fn the_second_and_middle_buttons_keep_their_own_state() {
    let _guard = imgui_guard();
    let mut context = context();
    move_to(&mut context, 20.0, 30.0);
    press(&mut context, PointerButton::Secondary, true);
    frame(&mut context);
    assert!(io(&mut context, |io| io.mouse_down(MouseButton::Right)));
    assert!(!io(&mut context, |io| io.mouse_down(MouseButton::Left)));

    press(&mut context, PointerButton::Secondary, false);
    press(&mut context, PointerButton::Middle, true);
    frame(&mut context);
    assert!(!io(&mut context, |io| io.mouse_down(MouseButton::Right)));
    assert!(io(&mut context, |io| io.mouse_down(MouseButton::Middle)));
}

#[test]
fn leaving_moves_the_pointer_out_of_every_widget() {
    let _guard = imgui_guard();
    let mut context = context();
    move_to(&mut context, 20.0, 30.0);
    frame(&mut context);
    assert_eq!(io(&mut context, |io| io.mouse_pos()), [20.0, 30.0]);

    assert!(input::pointer(&mut context, &InputEvent::PointerLeave));
    frame(&mut context);
    let pos = io(&mut context, |io| io.mouse_pos());
    assert!(
        pos[0] < -1000.0 && pos[1] < -1000.0,
        "the pointer is out of the window, not at its corner: {pos:?}"
    );
}

#[test]
fn a_cancelled_gesture_releases_every_button() {
    let _guard = imgui_guard();
    let mut context = context();
    move_to(&mut context, 20.0, 30.0);
    press(&mut context, PointerButton::Primary, true);
    frame(&mut context);
    assert!(io(&mut context, |io| io.mouse_down(MouseButton::Left)));

    assert!(input::pointer(&mut context, &InputEvent::PointerCancel));
    frame(&mut context);
    assert!(!io(&mut context, |io| io.mouse_down(MouseButton::Left)));
}

#[test]
fn events_that_are_not_the_pointer_are_left_for_the_input_layer() {
    let _guard = imgui_guard();
    let mut context = context();
    assert!(
        !input::pointer(&mut context, &InputEvent::PointerEnter),
        "an enter carries no position: the move after it does"
    );
    assert!(!input::pointer(
        &mut context,
        &InputEvent::Scroll {
            x: 10.0,
            y: 10.0,
            delta_x: 0.0,
            delta_y: 1.0
        }
    ));
}
