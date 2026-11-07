//! The window's own events → Dear ImGui IO for a plugin's window.
//!
//! The framework routes a window's document events to the view mounted in it
//! (`on_document_event`), with positions in logical window points — the same
//! units Dear ImGui's `io.DisplaySize` uses for the window, so they pass
//! through unchanged. Input is per window by construction: one window's
//! pointer, keys and text never reach another's.

use std::rc::Rc;

use dear_imgui_rs::{ClipboardBackend, ConfigFlags, Context, Io, Key as ImGuiKey, MouseButton};
use zgx::input::{InputEvent, Key, Modifiers, PointerButton};
use zgx::platform::PlatformService;

use super::runtime::WindowContext;

/// Applies one document event to `context`'s IO, answering whether it
/// changed what the window draws (so the caller asks for a frame).
pub fn input(context: &mut WindowContext, event: &InputEvent) -> bool {
    context
        .with_active(|context| {
            let io = apply(context.io_mut(), event);
            let ime = apply_ime(context, event);
            io || ime
        })
        .unwrap_or(false)
}

/// The pointer-only name the first input layer kept, now a full-event alias.
pub fn pointer(context: &mut WindowContext, event: &InputEvent) -> bool {
    input(context, event)
}

pub fn cancel(context: &mut WindowContext) {
    let _ = context.with_active(|context| {
        let io = context.io_mut();
        io.add_mouse_pos_event([-f32::MAX, -f32::MAX]);
        release(io);
    });
}

pub fn focus(context: &mut WindowContext, focused: bool) -> bool {
    context
        .with_active(|context| {
            let io = context.io_mut();
            if focused {
                io.add_focus_event(true);
            } else {
                release(io);
                io.add_focus_event(false);
            }
            true
        })
        .unwrap_or(false)
}

fn release(io: &mut Io) {
    for button in [MouseButton::Left, MouseButton::Right, MouseButton::Middle] {
        io.add_mouse_button_event(button, false);
    }
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
            release(io);
            true
        }
        InputEvent::Scroll {
            x,
            y,
            delta_x,
            delta_y,
        } => {
            io.add_mouse_pos_event([*x, *y]);
            io.add_mouse_wheel_event([*delta_x, *delta_y]);
            true
        }
        InputEvent::KeyDown { key, modifiers, .. } => key_event(io, key, true, *modifiers),
        InputEvent::KeyUp { key, modifiers } => key_event(io, key, false, *modifiers),
        InputEvent::Text(text) => {
            io.add_input_characters_utf8(text);
            !text.is_empty()
        }
        InputEvent::Focus => {
            io.add_focus_event(true);
            true
        }
        InputEvent::Blur => {
            io.add_focus_event(false);
            true
        }
        InputEvent::ImePreedit { .. } | InputEvent::ImeCommit(_) => false,
        _ => false,
    }
}

/// The composition half of an IME event, which lives on the context rather
/// than the IO queue: the preedit only draws, and a commit edits the active
/// text widget exactly once.
pub fn apply_ime(context: &mut Context, event: &InputEvent) -> bool {
    match event {
        InputEvent::ImePreedit { text, cursor } => {
            if text.is_empty() {
                context.clear_ime_preedit();
                return true;
            }
            if !context.has_ime_composition() {
                context.notify_ime_enabled(true);
            }
            context.set_ime_preedit(text, *cursor).is_ok()
        }
        InputEvent::ImeCommit(text) => {
            if text.is_empty() {
                return false;
            }
            if context.has_ime_composition() {
                let _ = context.commit_ime_text(text);
            } else {
                context.io_mut().add_input_characters_utf8(text);
            }
            true
        }
        _ => false,
    }
}

/// Sets Dear ImGui's double-click time and distance from the host's own
/// values, so a double-click means the same thing in a plugin window as it
/// does in the rest of the app.
pub fn configure_io(io: &mut Io, double_click_time: f32, double_click_max_dist: f32) {
    io.set_config_flags(io.config_flags() | ConfigFlags::NAV_ENABLE_KEYBOARD);
    let seconds = double_click_time.clamp(0.1, 5.0);
    if io.mouse_single_click_delay() <= seconds {
        io.set_mouse_single_click_delay(seconds + 0.05);
    }
    io.set_mouse_double_click_time(seconds);
    io.set_mouse_double_click_max_dist(double_click_max_dist.clamp(1.0, 200.0));
}

/// The zgx clipboard, behind Dear ImGui's clipboard backend. It reads and
/// writes the same platform clipboard `ui.clipboard` uses; the capability
/// gate stays on `ui.clipboard`, not on the keyboard's own copy and paste.
pub struct ZgxClipboard(pub Rc<PlatformService>);

impl ClipboardBackend for ZgxClipboard {
    fn get(&mut self) -> Option<String> {
        self.0.0.clipboard_text()
    }

    fn set(&mut self, value: &str) {
        self.0.0.set_clipboard_text(value.to_string());
    }
}

fn key_event(io: &mut Io, key: &Key, down: bool, modifiers: Modifiers) -> bool {
    io.add_key_event(ImGuiKey::ModCtrl, modifiers.control);
    io.add_key_event(ImGuiKey::ModShift, modifiers.shift);
    io.add_key_event(ImGuiKey::ModAlt, modifiers.alt);
    io.add_key_event(ImGuiKey::ModSuper, modifiers.meta);
    if let Some(key) = imgui_key(key) {
        io.add_key_event(key, down);
    }
    true
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

fn imgui_key(key: &Key) -> Option<ImGuiKey> {
    match key {
        Key::Tab => Some(ImGuiKey::Tab),
        Key::Enter => Some(ImGuiKey::Enter),
        Key::Space => Some(ImGuiKey::Space),
        Key::Escape => Some(ImGuiKey::Escape),
        Key::Backspace => Some(ImGuiKey::Backspace),
        Key::Delete => Some(ImGuiKey::Delete),
        Key::ArrowLeft => Some(ImGuiKey::LeftArrow),
        Key::ArrowRight => Some(ImGuiKey::RightArrow),
        Key::ArrowUp => Some(ImGuiKey::UpArrow),
        Key::ArrowDown => Some(ImGuiKey::DownArrow),
        Key::Home => Some(ImGuiKey::Home),
        Key::End => Some(ImGuiKey::End),
        Key::PageUp => Some(ImGuiKey::PageUp),
        Key::PageDown => Some(ImGuiKey::PageDown),
        Key::Insert => Some(ImGuiKey::Insert),
        Key::ContextMenu => Some(ImGuiKey::Menu),
        Key::Function(number) => function_key(*number),
        Key::Character(text) => character_key(text),
    }
}

fn function_key(number: u8) -> Option<ImGuiKey> {
    Some(match number {
        1 => ImGuiKey::F1,
        2 => ImGuiKey::F2,
        3 => ImGuiKey::F3,
        4 => ImGuiKey::F4,
        5 => ImGuiKey::F5,
        6 => ImGuiKey::F6,
        7 => ImGuiKey::F7,
        8 => ImGuiKey::F8,
        9 => ImGuiKey::F9,
        10 => ImGuiKey::F10,
        11 => ImGuiKey::F11,
        12 => ImGuiKey::F12,
        13 => ImGuiKey::F13,
        14 => ImGuiKey::F14,
        15 => ImGuiKey::F15,
        16 => ImGuiKey::F16,
        17 => ImGuiKey::F17,
        18 => ImGuiKey::F18,
        19 => ImGuiKey::F19,
        20 => ImGuiKey::F20,
        21 => ImGuiKey::F21,
        22 => ImGuiKey::F22,
        23 => ImGuiKey::F23,
        24 => ImGuiKey::F24,
        _ => return None,
    })
}

fn character_key(text: &str) -> Option<ImGuiKey> {
    let mut chars = text.chars();
    let first = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    match first {
        'a'..='z' | 'A'..='Z' => Some(match first.to_ascii_lowercase() {
            'a' => ImGuiKey::A,
            'b' => ImGuiKey::B,
            'c' => ImGuiKey::C,
            'd' => ImGuiKey::D,
            'e' => ImGuiKey::E,
            'f' => ImGuiKey::F,
            'g' => ImGuiKey::G,
            'h' => ImGuiKey::H,
            'i' => ImGuiKey::I,
            'j' => ImGuiKey::J,
            'k' => ImGuiKey::K,
            'l' => ImGuiKey::L,
            'm' => ImGuiKey::M,
            'n' => ImGuiKey::N,
            'o' => ImGuiKey::O,
            'p' => ImGuiKey::P,
            'q' => ImGuiKey::Q,
            'r' => ImGuiKey::R,
            's' => ImGuiKey::S,
            't' => ImGuiKey::T,
            'u' => ImGuiKey::U,
            'v' => ImGuiKey::V,
            'w' => ImGuiKey::W,
            'x' => ImGuiKey::X,
            'y' => ImGuiKey::Y,
            'z' => ImGuiKey::Z,
            _ => return None,
        }),
        '0' => Some(ImGuiKey::Key0),
        '1' => Some(ImGuiKey::Key1),
        '2' => Some(ImGuiKey::Key2),
        '3' => Some(ImGuiKey::Key3),
        '4' => Some(ImGuiKey::Key4),
        '5' => Some(ImGuiKey::Key5),
        '6' => Some(ImGuiKey::Key6),
        '7' => Some(ImGuiKey::Key7),
        '8' => Some(ImGuiKey::Key8),
        '9' => Some(ImGuiKey::Key9),
        ' ' => Some(ImGuiKey::Space),
        '\'' => Some(ImGuiKey::Apostrophe),
        ',' => Some(ImGuiKey::Comma),
        '-' => Some(ImGuiKey::Minus),
        '.' => Some(ImGuiKey::Period),
        '/' => Some(ImGuiKey::Slash),
        ';' => Some(ImGuiKey::Semicolon),
        '=' => Some(ImGuiKey::Equal),
        '[' => Some(ImGuiKey::LeftBracket),
        '\\' => Some(ImGuiKey::Backslash),
        ']' => Some(ImGuiKey::RightBracket),
        '`' => Some(ImGuiKey::GraveAccent),
        _ => None,
    }
}
