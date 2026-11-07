//! Pointer events → Dear ImGui IO: a real context, no desktop.
//!
//! The mapping is driven the way the window host drives it (`input::pointer`
//! with the framework's own `InputEvent`s), and the IO is read back after a
//! frame, which is when Dear ImGui applies queued input.

use std::cell::RefCell;
use std::rc::Rc;

use dear_imgui_rs::{ClipboardBackend, Key as ImGuiKey, MouseButton};
use zgx::input::{InputEvent, Key, Modifiers, PointerButton};
use zgx::platform::{Platform, PlatformService};

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
    context
        .build_frame(spec(), false, |_| {}, |_| {})
        .expect("a frame");
}

fn io<T>(context: &mut WindowContext, read: impl FnOnce(&dear_imgui_rs::Io) -> T) -> T {
    context
        .with_active(|context| read(context.io_mut()))
        .expect("an active context")
}

fn frame_with<T>(context: &mut WindowContext, read: impl FnOnce(&dear_imgui_rs::Ui) -> T) -> T {
    let read = RefCell::new(Some(read));
    let result: RefCell<Option<T>> = RefCell::new(None);
    let size = spec();
    context
        .build_frame(
            size,
            false,
            |_| {},
            |ui| {
                let ui = unsafe { &*ui };
                crate::plugins::imgui::frame::root_window(ui, &size, None, || {
                    let read = read.borrow_mut().take().expect("one read");
                    *result.borrow_mut() = Some(read(ui));
                });
            },
        )
        .expect("a frame");
    result.borrow_mut().take().expect("a result")
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
fn cancelling_after_a_drag_drops_the_press_and_the_pointer() {
    let _guard = imgui_guard();
    let mut context = context();
    move_to(&mut context, 20.0, 30.0);
    press(&mut context, PointerButton::Primary, true);
    frame(&mut context);
    assert!(io(&mut context, |io| io.mouse_down(MouseButton::Left)));

    input::cancel(&mut context);
    frame(&mut context);
    assert!(!io(&mut context, |io| io.mouse_down(MouseButton::Left)));
    assert!(!io(&mut context, |io| io.mouse_down(MouseButton::Right)));
    assert!(!io(&mut context, |io| io.mouse_down(MouseButton::Middle)));
    let pos = io(&mut context, |io| io.mouse_pos());
    assert!(
        pos[0] < -1000.0 && pos[1] < -1000.0,
        "the pointer is out of range after the drag: {pos:?}"
    );
}

#[test]
fn a_scroll_moves_the_wheel_under_the_pointer() {
    let _guard = imgui_guard();
    let mut context = context();
    assert!(!input::pointer(&mut context, &InputEvent::PointerEnter));
    assert!(input::pointer(
        &mut context,
        &InputEvent::Scroll {
            x: 10.0,
            y: 10.0,
            delta_x: 0.0,
            delta_y: 1.0
        }
    ));
    frame(&mut context);
    assert_eq!(io(&mut context, |io| io.mouse_pos()), [10.0, 10.0]);
    let wheel = frame_with(&mut context, |ui| ui.io().mouse_wheel());
    assert!(wheel > 0.0, "a wheel down is a positive wheel: {wheel}");
}

fn key_down(key: Key, modifiers: Modifiers, repeat: bool) -> InputEvent {
    InputEvent::KeyDown {
        key,
        modifiers,
        repeat,
    }
}

fn key_up(key: Key, modifiers: Modifiers) -> InputEvent {
    InputEvent::KeyUp { key, modifiers }
}

fn draw_field(ui: &dear_imgui_rs::Ui, text: &mut String) {
    ui.input_text("field", text).build();
}

fn field_center(ui: &dear_imgui_rs::Ui, text: &mut String) -> [f32; 2] {
    ui.input_text("field", text).build();
    let min = ui.item_rect_min();
    let max = ui.item_rect_max();
    [(min[0] + max[0]) / 2.0, (min[1] + max[1]) / 2.0]
}

fn frame_field(context: &mut WindowContext, text: &mut String) {
    frame_with(context, |ui| draw_field(ui, text));
}

fn focus_field(context: &mut WindowContext, text: &mut String) {
    let center = frame_with(context, |ui| field_center(ui, text));
    input::input(
        context,
        &InputEvent::PointerMove {
            x: center[0],
            y: center[1],
        },
    );
    frame_field(context, text);
    input::input(
        context,
        &InputEvent::PointerDown {
            x: center[0],
            y: center[1],
            button: PointerButton::Primary,
        },
    );
    frame_field(context, text);
    input::input(
        context,
        &InputEvent::PointerUp {
            x: center[0],
            y: center[1],
            button: PointerButton::Primary,
        },
    );
    frame_field(context, text);
}

fn type_into(context: &mut WindowContext, text: &mut String, input: &str) {
    for ch in input.chars() {
        let key = Key::Character(ch.to_string());
        input::input(context, &key_down(key.clone(), Modifiers::default(), false));
        input::input(context, &InputEvent::Text(ch.to_string()));
        input::input(context, &key_up(key, Modifiers::default()));
        frame_field(context, text);
    }
}

#[test]
fn a_key_down_and_up_track_its_imgui_key_and_modifiers() {
    let _guard = imgui_guard();
    let mut context = context();
    let modifiers = Modifiers {
        shift: true,
        control: true,
        alt: false,
        meta: false,
    };
    input::input(
        &mut context,
        &key_down(Key::Character("s".into()), modifiers, false),
    );
    assert!(frame_with(&mut context, |ui| ui.is_key_down(ImGuiKey::S)));
    assert!(io(&mut context, |io| io.key_ctrl()));
    assert!(io(&mut context, |io| io.key_shift()));
    input::input(&mut context, &key_up(Key::Character("s".into()), modifiers));
    assert!(!frame_with(&mut context, |ui| ui.is_key_down(ImGuiKey::S)));
}

#[test]
fn typing_edits_a_focused_input_text() {
    let _guard = imgui_guard();
    let mut context = context();
    let mut text: String = String::new();
    focus_field(&mut context, &mut text);
    type_into(&mut context, &mut text, "hero");
    assert_eq!(text, "hero");
}

#[test]
fn multibyte_text_round_trips_through_the_input_queue() {
    let _guard = imgui_guard();
    let mut context = context();
    let mut text = String::new();
    focus_field(&mut context, &mut text);
    type_into(&mut context, &mut text, "日本語 Привет 🎉");
    assert_eq!(text.as_str(), "日本語 Привет 🎉");
    assert!(!text.contains('\u{fffd}'));
}

#[test]
fn an_ime_preedit_does_not_insert_and_a_commit_inserts_once() {
    let _guard = imgui_guard();
    let mut context = context();
    let mut text = String::new();
    focus_field(&mut context, &mut text);
    input::input(
        &mut context,
        &InputEvent::ImePreedit {
            text: "にほん".into(),
            cursor: Some((9, 9)),
        },
    );
    let preedit = context
        .with_active(|context| context.ime_preedit())
        .expect("an active context");
    assert_eq!(
        preedit.as_ref().map(|preedit| preedit.text()),
        Some("にほん")
    );
    assert!(text.is_empty(), "the preedit never edits the buffer");
    input::input(&mut context, &InputEvent::ImeCommit("日本".into()));
    frame_field(&mut context, &mut text);
    assert_eq!(text, "日本");
}

#[test]
fn an_ime_cancel_leaves_nothing_behind() {
    let _guard = imgui_guard();
    let mut context = context();
    let mut text = String::new();
    focus_field(&mut context, &mut text);
    input::input(
        &mut context,
        &InputEvent::ImePreedit {
            text: "にほん".into(),
            cursor: Some((9, 9)),
        },
    );
    input::input(
        &mut context,
        &InputEvent::ImePreedit {
            text: String::new(),
            cursor: None,
        },
    );
    let preedit = context
        .with_active(|context| context.ime_preedit())
        .expect("an active context");
    assert!(preedit.is_none());
    frame_field(&mut context, &mut text);
    assert!(text.is_empty());
}

#[test]
fn one_windows_events_stay_in_that_window() {
    let _guard = imgui_guard();
    let mut only = context();
    let mut other = context();
    input::input(
        &mut only,
        &key_down(Key::Character("a".into()), Modifiers::default(), false),
    );
    assert!(frame_with(&mut only, |ui| ui.is_key_down(ImGuiKey::A)));
    assert!(!frame_with(&mut other, |ui| ui.is_key_down(ImGuiKey::A)));
}

struct FocusLineup {
    field: bool,
    a: bool,
    b: bool,
    slider: bool,
    a_pressed: bool,
}

fn focus_lineup(ui: &dear_imgui_rs::Ui, focus_field: bool, value: &mut i32) -> FocusLineup {
    if focus_field {
        ui.set_keyboard_focus_here();
    }
    let mut text = String::new();
    ui.input_text("field", &mut text).build();
    let field = ui.is_item_focused();
    let a_pressed = ui.button("A");
    let a = ui.is_item_focused();
    ui.button("B");
    let b = ui.is_item_focused();
    ui.slider_i32("slider", value, 0, 10);
    let slider = ui.is_item_focused();
    FocusLineup {
        field,
        a,
        b,
        slider,
        a_pressed,
    }
}

#[test]
fn tab_and_space_drive_the_focused_widget() {
    let _guard = imgui_guard();
    let mut context = context();
    let mut value = 0;
    context
        .with_active(|context| input::configure_io(context.io_mut(), 0.5, 6.0))
        .expect("an active context");
    frame_with(&mut context, |ui| focus_lineup(ui, true, &mut value));
    let setup = frame_with(&mut context, |ui| focus_lineup(ui, false, &mut value));
    assert!(setup.field, "the field is where the keyboard starts");

    input::input(
        &mut context,
        &key_down(Key::Tab, Modifiers::default(), false),
    );
    frame_with(&mut context, |ui| focus_lineup(ui, false, &mut value));
    input::input(&mut context, &key_up(Key::Tab, Modifiers::default()));
    let focused = frame_with(&mut context, |ui| focus_lineup(ui, false, &mut value));
    assert!(focused.a, "Tab from the field focuses the first button");

    input::input(
        &mut context,
        &key_down(Key::Space, Modifiers::default(), false),
    );
    let pressed = frame_with(&mut context, |ui| focus_lineup(ui, false, &mut value));
    assert!(pressed.a_pressed, "Space presses the focused button");
    input::input(&mut context, &key_up(Key::Space, Modifiers::default()));
    let released = frame_with(&mut context, |ui| focus_lineup(ui, false, &mut value));
    assert!(!released.a_pressed, "a release is not a press");

    let shifted = Modifiers {
        shift: true,
        ..Modifiers::default()
    };
    input::input(&mut context, &key_down(Key::Tab, shifted, false));
    frame_with(&mut context, |ui| focus_lineup(ui, false, &mut value));
    input::input(&mut context, &key_up(Key::Tab, shifted));
    let back = frame_with(&mut context, |ui| focus_lineup(ui, false, &mut value));
    assert!(back.field, "Shift+Tab walks back to the field");

    let mut reached_slider = false;
    for _ in 0..3 {
        input::input(
            &mut context,
            &key_down(Key::Tab, Modifiers::default(), false),
        );
        frame_with(&mut context, |ui| focus_lineup(ui, false, &mut value));
        input::input(&mut context, &key_up(Key::Tab, Modifiers::default()));
        reached_slider = frame_with(&mut context, |ui| focus_lineup(ui, false, &mut value)).slider;
    }
    assert!(
        reached_slider,
        "Tab walks through both buttons to the slider"
    );
}

#[test]
fn the_zgx_clipboard_is_imgui_s_text_source_and_sink() {
    let held: Rc<RefCell<String>> = Rc::new(RefCell::new("clip-check-42".into()));
    struct MockClipboard(Rc<RefCell<String>>);
    impl Platform for MockClipboard {
        fn clipboard_text(&self) -> Option<String> {
            Some(self.0.borrow().clone())
        }

        fn set_clipboard_text(&self, text: String) -> bool {
            *self.0.borrow_mut() = text;
            true
        }

        fn open_files(
            &self,
            _: zgx::platform::FilePicker,
        ) -> zgx::platform::Pending<Result<Vec<std::path::PathBuf>, String>> {
            Box::pin(async { Ok(Vec::new()) })
        }

        fn save_file(
            &self,
            _: zgx::platform::FilePicker,
        ) -> zgx::platform::Pending<Result<Option<std::path::PathBuf>, String>> {
            Box::pin(async { Ok(None) })
        }
    }
    let mut backend = input::ZgxClipboard(Rc::new(PlatformService(Rc::new(MockClipboard(
        held.clone(),
    )))));
    assert_eq!(backend.get().as_deref(), Some("clip-check-42"));
    backend.set("copied");
    assert_eq!(held.borrow().as_str(), "copied");
}
