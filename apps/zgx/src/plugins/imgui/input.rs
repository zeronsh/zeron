//! Pointer events → Dear ImGui IO for a plugin's window.
//!
//! The framework routes a window's document events to the view mounted in it
//! (`on_document_event`), with positions in logical window points — the same
//! units Dear ImGui's `io.DisplaySize` uses for the window, so they pass
//! through unchanged. Input is per window by construction: one window's
//! pointer never reaches another's.
//!
//! This is the slice the end-to-end proof needs — where the pointer is, and
//! whether its buttons are down, so `ui.button` reports a release and the
//! button's branch runs. Keyboard, text, IME, the wheel, cursor shapes and
//! focus belong to the input-fidelity feature, which extends this module;
//! nothing here widens what a plugin can reach.

use dear_imgui_rs::{Io, MouseButton};
use zgx::input::{InputEvent, PointerButton};

use super::runtime::WindowContext;

/// Applies one document event to `context`'s IO, answering whether it
/// changed what the window draws (so the caller asks for a frame).
pub fn pointer(context: &mut WindowContext, event: &InputEvent) -> bool {
    context
        .with_active(|context| apply(context.io_mut(), event))
        .unwrap_or(false)
}

pub fn cancel(context: &mut WindowContext) {
    let _ = context.with_active(|context| {
        let io = context.io_mut();
        io.add_mouse_pos_event([-f32::MAX, -f32::MAX]);
        for button in [MouseButton::Left, MouseButton::Right, MouseButton::Middle] {
            io.add_mouse_button_event(button, false);
        }
    });
}

/// The mapping itself, kept apart from the context so a test can drive it:
/// events are queued on the IO as they happen and applied by the next frame,
/// which is exactly what Dear ImGui expects.
pub fn apply(io: &mut Io, event: &InputEvent) -> bool {
    match event {
        InputEvent::PointerMove { x, y } => {
            io.add_mouse_pos_event([*x, *y]);
            true
        }
        InputEvent::PointerDown { x, y, button } => button_event(io, *x, *y, *button, true),
        InputEvent::PointerUp { x, y, button } => button_event(io, *x, *y, *button, false),
        // A move follows an enter with the position; there is nothing to do
        // before it arrives.
        InputEvent::PointerEnter => false,
        // Outside every widget: Dear ImGui uses an out-of-range position for
        // "the pointer is not here", so hover and pressed states end.
        InputEvent::PointerLeave => {
            io.add_mouse_pos_event([-f32::MAX, -f32::MAX]);
            true
        }
        // The platform took the pointer mid-gesture: it is no longer over
        // anything, and a press that was down ends without the widget seeing
        // a release where the pointer was (which would read as a click).
        InputEvent::PointerCancel => {
            io.add_mouse_pos_event([-f32::MAX, -f32::MAX]);
            for button in [MouseButton::Left, MouseButton::Right, MouseButton::Middle] {
                io.add_mouse_button_event(button, false);
            }
            true
        }
        _ => false,
    }
}

/// A press or release at `x`, `y`: the position goes first, so the frame that
/// applies it hovers the widget the pointer is over.
fn button_event(io: &mut Io, x: f32, y: f32, pointer: PointerButton, down: bool) -> bool {
    io.add_mouse_pos_event([x, y]);
    if let Some(button) = imgui_button(pointer) {
        io.add_mouse_button_event(button, down);
    }
    // Extra buttons Dear ImGui has no slot for still move the pointer.
    true
}

fn imgui_button(pointer: PointerButton) -> Option<MouseButton> {
    Some(match pointer {
        PointerButton::Primary => MouseButton::Left,
        PointerButton::Secondary => MouseButton::Right,
        PointerButton::Middle => MouseButton::Middle,
        PointerButton::Other(3) => MouseButton::Extra1,
        PointerButton::Other(4) => MouseButton::Extra2,
        PointerButton::Other(_) => return None,
    })
}
