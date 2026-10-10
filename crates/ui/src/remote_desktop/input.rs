//! Semantic GPUI key events plus native composed text. GPUI does not expose
//! physical scancodes, so printable text uses its platform text-input handler.
use super::desktop::Desktop;
use gpui::{prelude::*, *};
use std::{collections::HashSet, ops::Range};
use zeron_rdp::{InputEvent, PointerButton};
actions!(remote_desktop, [ReleaseCapture]);
pub const RELEASE_SHORTCUT: &str = "ctrl-alt-shift-escape";
#[derive(Clone)]
pub enum DesktopEvent {
    Input(InputEvent),
    Move(u16, u16),
    Release,
    Geometry(u16, u16),
}
impl EventEmitter<DesktopEvent> for Desktop {}

/// Override the first stroke of every local binding while this context owns
/// focus, including user customizations and chord prefixes. NoAction lets raw
/// input proceed without dispatching the matched global action.
pub fn bind_keys(cx: &mut App) {
    let strokes: HashSet<String> = cx
        .key_bindings()
        .borrow()
        .bindings()
        .filter_map(|b| b.keystrokes().first().map(|k| k.unparse()))
        .collect();
    let chords: Vec<String> = cx
        .key_bindings()
        .borrow()
        .bindings()
        .map(|b| {
            b.keystrokes()
                .iter()
                .map(|k| k.unparse())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect();
    cx.bind_keys(
        chords
            .iter()
            .map(|key| KeyBinding::new(key, NoAction, Some("RemoteDesktop"))),
    );
    cx.bind_keys(
        strokes
            .iter()
            .map(|key| KeyBinding::new(key, NoAction, Some("RemoteDesktop"))),
    );
    cx.bind_keys([KeyBinding::new(
        RELEASE_SHORTCUT,
        ReleaseCapture,
        Some("RemoteDesktop"),
    )]);
}

pub fn scancode(key: &str) -> Option<u16> {
    Some(match key {
        "escape" => 1,
        "backspace" => 14,
        "tab" => 15,
        "enter" => 28,
        "space" => 57,
        "up" => 0xe048,
        "down" => 0xe050,
        "left" => 0xe04b,
        "right" => 0xe04d,
        "home" => 0xe047,
        "end" => 0xe04f,
        "pageup" => 0xe049,
        "pagedown" => 0xe051,
        "insert" => 0xe052,
        "delete" => 0xe053,
        "f1" => 59,
        "f2" => 60,
        "f3" => 61,
        "f4" => 62,
        "f5" => 63,
        "f6" => 64,
        "f7" => 65,
        "f8" => 66,
        "f9" => 67,
        "f10" => 68,
        "f11" => 87,
        "f12" => 88,
        "a" => 30,
        "b" => 48,
        "c" => 46,
        "d" => 32,
        "e" => 18,
        "f" => 33,
        "g" => 34,
        "h" => 35,
        "i" => 23,
        "j" => 36,
        "k" => 37,
        "l" => 38,
        "m" => 50,
        "n" => 49,
        "o" => 24,
        "p" => 25,
        "q" => 16,
        "r" => 19,
        "s" => 31,
        "t" => 20,
        "u" => 22,
        "v" => 47,
        "w" => 17,
        "x" => 45,
        "y" => 21,
        "z" => 44,
        "1" => 2,
        "2" => 3,
        "3" => 4,
        "4" => 5,
        "5" => 6,
        "6" => 7,
        "7" => 8,
        "8" => 9,
        "9" => 10,
        "0" => 11,
        "-" => 12,
        "=" => 13,
        "[" => 26,
        "]" => 27,
        ";" => 39,
        "'" => 40,
        "`" => 41,
        "\\" => 43,
        "," => 51,
        "." => 52,
        "/" => 53,
        _ => return None,
    })
}
fn button(button: MouseButton) -> PointerButton {
    match button {
        MouseButton::Left => PointerButton::Left,
        MouseButton::Right => PointerButton::Right,
        MouseButton::Middle => PointerButton::Middle,
        MouseButton::Navigate(NavigationDirection::Back) => PointerButton::Back,
        MouseButton::Navigate(NavigationDirection::Forward) => PointerButton::Forward,
    }
}
impl Desktop {
    pub fn release_capture(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.release_input(cx);
        if self.focus.is_focused(window) {
            window.blur();
        }
        cx.notify();
    }
    pub(super) fn release_input(&mut self, cx: &mut Context<Self>) {
        self.pressed.clear();
        self.buttons.clear();
        self.modifiers = Modifiers::default();
        self.composition.clear();
        cx.emit(DesktopEvent::Release);
    }
    fn emit_key(&mut self, code: u16, down: bool, cx: &mut Context<Self>) {
        cx.emit(DesktopEvent::Input(InputEvent::ScanCode { code, down }));
    }
    fn modifiers(&mut self, next: Modifiers, press: bool, cx: &mut Context<Self>) {
        for (code, old, new) in [
            (29, self.modifiers.control, next.control),
            (56, self.modifiers.alt, next.alt),
            (42, self.modifiers.shift, next.shift),
            (0xe05b, self.modifiers.platform, next.platform),
        ] {
            if old && !new {
                self.emit_key(code, false, cx);
            } else if !old && new && press {
                self.emit_key(code, true, cx);
            }
        }
        if press {
            self.modifiers = next;
        } else {
            self.modifiers.control &= next.control;
            self.modifiers.alt &= next.alt;
            self.modifiers.shift &= next.shift;
            self.modifiers.platform &= next.platform;
        }
    }
    pub(super) fn key_down(
        &mut self,
        event: &KeyDownEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.enabled {
            return;
        }
        let k = &event.keystroke;
        let m = k.modifiers;
        let altgr = m.control && m.alt && k.key_char.is_some();
        let composed = k.key_char.as_ref().is_some_and(|s| !s.is_ascii());
        let shortcut = !altgr && !composed && (m.control || m.platform || m.alt);
        let special = k.key_char.is_none()
            || matches!(
                k.key.as_str(),
                "enter" | "tab" | "escape" | "backspace" | "delete"
            );
        if (shortcut || special)
            && let Some(code) = scancode(&k.key)
        {
            self.modifiers(m, true, cx);
            self.pressed.insert(k.key.clone(), code);
            self.emit_key(code, true, cx);
            cx.stop_propagation();
        }
        // Normal text is committed only by EntityInputHandler, including IME.
    }
    pub(super) fn key_up(&mut self, event: &KeyUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(code) = self.pressed.remove(&event.keystroke.key) {
            self.emit_key(code, false, cx);
            cx.stop_propagation();
        }
        self.modifiers(event.keystroke.modifiers, false, cx);
    }
    pub(super) fn modifiers_changed(
        &mut self,
        event: &ModifiersChangedEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.modifiers(event.modifiers, false, cx);
    }
    fn position(&self, p: Point<Pixels>, clamp: bool) -> Option<(u16, u16)> {
        self.transform.remote(
            (p.x - self.bounds.origin.x).into(),
            (p.y - self.bounds.origin.y).into(),
            clamp,
        )
    }
    pub(super) fn mouse_down(
        &mut self,
        e: &MouseDownEvent,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.enabled {
            return;
        }
        if let Some((x, y)) = self.position(e.position, false) {
            w.focus(&self.focus, cx);
            self.buttons.insert(e.button);
            self.pointer = Some((x, y));
            cx.emit(DesktopEvent::Input(InputEvent::Button {
                button: button(e.button),
                down: true,
                x,
                y,
            }));
            cx.stop_propagation();
            cx.notify();
        }
    }
    pub(super) fn mouse_up(&mut self, e: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.buttons.remove(&e.button)
            && let Some((x, y)) = self.position(e.position, true)
        {
            cx.emit(DesktopEvent::Input(InputEvent::Button {
                button: button(e.button),
                down: false,
                x,
                y,
            }));
            cx.stop_propagation();
        }
    }
    pub(super) fn mouse_move(
        &mut self,
        e: &MouseMoveEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let next = self.position(e.position, !self.buttons.is_empty());
        if self.enabled
            && next.is_some()
            && !matches!(self.remote_cursor, zeron_rdp::RemoteCursor::Default)
        {
            super::cursor::hide(cx);
        }
        if self.pointer != next {
            self.pointer = next;
            cx.notify();
        }
        if self.enabled
            && let Some((x, y)) = next
        {
            cx.emit(DesktopEvent::Move(x, y));
        }
    }
    pub(super) fn scroll(&mut self, e: &ScrollWheelEvent, w: &mut Window, cx: &mut Context<Self>) {
        if !self.enabled || !self.focus.is_focused(w) {
            return;
        }
        let delta = e.delta.pixel_delta(px(24.));
        for (horizontal, amount) in [(false, f32::from(delta.y)), (true, f32::from(delta.x))] {
            if amount.abs() >= 1. {
                cx.emit(DesktopEvent::Input(InputEvent::Wheel {
                    horizontal,
                    amount: amount.round().clamp(-255., 255.) as i16,
                }));
            }
        }
        cx.stop_propagation();
    }
}
impl EntityInputHandler for Desktop {
    fn accepts_text_input(&self, window: &mut Window, _: &mut Context<Self>) -> bool {
        self.enabled && self.focus.is_focused(window)
    }
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let utf16: Vec<_> = self.composition.encode_utf16().collect();
        let start = range.start.min(utf16.len());
        let end = range.end.min(utf16.len()).max(start);
        *actual = Some(start..end);
        Some(String::from_utf16_lossy(&utf16[start..end]))
    }
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let len = self.composition.encode_utf16().count();
        Some(UTF16Selection {
            range: len..len,
            reversed: false,
        })
    }
    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        (!self.composition.is_empty()).then(|| 0..self.composition.encode_utf16().count())
    }
    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.composition.clear();
    }
    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.enabled && self.focus.is_focused(window) {
            self.modifiers(Modifiers::default(), true, cx);
            cx.emit(DesktopEvent::Input(InputEvent::Text(text.to_string())));
        }
        self.composition.clear();
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.composition = text.into();
        cx.notify();
    }
    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        Some(Bounds::new(self.bounds.origin, size(px(1.), px(20.))))
    }
    fn character_index_for_point(
        &mut self,
        _: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        Some(0)
    }
}

#[cfg(test)]
mod tests {
    use gpui::{KeyBinding, KeyContext, Keystroke, TestAppContext};
    #[gpui::test]
    fn remote_desktop_capture_disables_custom_actions_and_chord_prefixes(cx: &mut TestAppContext) {
        cx.update(|cx| {
            cx.bind_keys([
                KeyBinding::new("ctrl-c", crate::shell::ToggleSidebar, None),
                KeyBinding::new("ctrl-k ctrl-w", crate::shell::ToggleSidebar, None),
            ]);
            super::bind_keys(cx);
            let keymap = cx.key_bindings();
            let keymap = keymap.borrow();
            for key in ["ctrl-c", "ctrl-k"] {
                let (bindings, pending) = keymap.bindings_for_input(
                    &[Keystroke::parse(key).unwrap()],
                    &[KeyContext::parse("RemoteDesktop").unwrap()],
                );
                assert!(bindings.is_empty(), "{key} dispatched a local action");
                assert!(!pending, "{key} was delayed as a chord");
            }
            let (bindings, _) = keymap.bindings_for_input(
                &[Keystroke::parse(super::RELEASE_SHORTCUT).unwrap()],
                &[KeyContext::parse("RemoteDesktop").unwrap()],
            );
            assert_eq!(bindings.len(), 1);
            assert!(bindings[0].action().as_any().is::<super::ReleaseCapture>());
            let (bindings, _) =
                keymap.bindings_for_input(&[Keystroke::parse("ctrl-c").unwrap()], &[]);
            assert_eq!(
                bindings.len(),
                1,
                "Local shortcut must still work outside desktop focus"
            );
        });
    }
    #[gpui::test]
    fn remote_desktop_native_text_commits_once(cx: &mut TestAppContext) {
        use std::{cell::RefCell, rc::Rc};
        let window = cx.add_window(|window, cx| {
            let mut desktop = super::Desktop::new(window, cx);
            desktop.enabled = true;
            window.focus(&desktop.focus, cx);
            desktop
        });
        let events = Rc::new(RefCell::new(Vec::new()));
        let sink = events.clone();
        let entity = window.root(cx).unwrap();
        let _sub = cx.update(|cx| {
            cx.subscribe(&entity, move |_, event: &super::DesktopEvent, _| {
                if let super::DesktopEvent::Input(zeron_rdp::InputEvent::Text(text)) = event {
                    sink.borrow_mut().push(text.clone());
                }
            })
        });
        cx.simulate_input(window.into(), "ñá");
        assert_eq!(events.borrow().concat(), "ñá");
    }
}

/// Native IME handlers may outlive the last rendered frame on window closure.
/// Retain only a weak view, upgrading for a single callback, so a focused
/// desktop and its image cannot survive the owning surface/window.
pub(super) struct WeakInputHandler {
    pub view: WeakEntity<Desktop>,
    pub bounds: Bounds<Pixels>,
}
impl WeakInputHandler {
    fn handler(&self) -> Option<ElementInputHandler<Desktop>> {
        Some(ElementInputHandler::new(self.bounds, self.view.upgrade()?))
    }
}
impl InputHandler for WeakInputHandler {
    fn selected_text_range(
        &mut self,
        ignore: bool,
        w: &mut Window,
        cx: &mut App,
    ) -> Option<UTF16Selection> {
        self.handler()?.selected_text_range(ignore, w, cx)
    }
    fn marked_text_range(&mut self, w: &mut Window, cx: &mut App) -> Option<Range<usize>> {
        self.handler()?.marked_text_range(w, cx)
    }
    fn text_for_range(
        &mut self,
        r: Range<usize>,
        actual: &mut Option<Range<usize>>,
        w: &mut Window,
        cx: &mut App,
    ) -> Option<String> {
        self.handler()?.text_for_range(r, actual, w, cx)
    }
    fn replace_text_in_range(
        &mut self,
        r: Option<Range<usize>>,
        text: &str,
        w: &mut Window,
        cx: &mut App,
    ) {
        if let Some(mut h) = self.handler() {
            h.replace_text_in_range(r, text, w, cx);
        }
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        r: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        w: &mut Window,
        cx: &mut App,
    ) {
        if let Some(mut h) = self.handler() {
            h.replace_and_mark_text_in_range(r, text, selected, w, cx);
        }
    }
    fn unmark_text(&mut self, w: &mut Window, cx: &mut App) {
        if let Some(mut h) = self.handler() {
            h.unmark_text(w, cx);
        }
    }
    fn bounds_for_range(
        &mut self,
        r: Range<usize>,
        w: &mut Window,
        cx: &mut App,
    ) -> Option<Bounds<Pixels>> {
        self.handler()?.bounds_for_range(r, w, cx)
    }
    fn character_index_for_point(
        &mut self,
        p: Point<Pixels>,
        w: &mut Window,
        cx: &mut App,
    ) -> Option<usize> {
        self.handler()?.character_index_for_point(p, w, cx)
    }
    fn set_selected_text_range(&mut self, r: Range<usize>, w: &mut Window, cx: &mut App) {
        if let Some(mut h) = self.handler() {
            h.set_selected_text_range(r, w, cx);
        }
    }
    fn element_bounds(&mut self, w: &mut Window, cx: &mut App) -> Option<Bounds<Pixels>> {
        self.handler()?.element_bounds(w, cx)
    }
    fn text_length_utf16(&mut self, w: &mut Window, cx: &mut App) -> Option<usize> {
        self.handler()?.text_length_utf16(w, cx)
    }
    fn accepts_text_input(&mut self, w: &mut Window, cx: &mut App) -> bool {
        self.handler()
            .is_some_and(|mut h| h.accepts_text_input(w, cx))
    }
    fn prefers_ime_for_printable_keys(&mut self, w: &mut Window, cx: &mut App) -> bool {
        self.accepts_text_input(w, cx)
    }
}

#[cfg(test)]
mod native_lifetime_tests {
    use super::{Desktop, WeakInputHandler};
    use gpui::{Bounds, TestAppContext};
    #[gpui::test]
    fn remote_desktop_native_input_handler_does_not_retain_a_closed_window(
        cx: &mut TestAppContext,
    ) {
        let window = cx.add_window(|w, cx| Desktop::new(w, cx));
        let handler = window
            .update(cx, |_, w, cx| {
                let h = WeakInputHandler {
                    view: cx.entity().downgrade(),
                    bounds: Bounds::default(),
                };
                w.remove_window();
                h
            })
            .unwrap();
        cx.run_until_parked();
        assert!(handler.handler().is_none());
    }
}
