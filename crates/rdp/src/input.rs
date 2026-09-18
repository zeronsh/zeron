use crate::{InputEvent, PointerButton};
use ironrdp::{
    input::{Database, MouseButton, MousePosition, Operation, Scancode, WheelRotations},
    pdu::input::fast_path::FastPathInputEvent,
};
#[derive(Default)]
pub(crate) struct InputState {
    database: Database,
    keyboard_layout: u32,
}
impl InputState {
    pub fn new(keyboard_layout: u32) -> Self {
        Self {
            database: Database::new(),
            keyboard_layout,
        }
    }
    pub fn apply(&mut self, event: InputEvent) -> Vec<FastPathInputEvent> {
        let operations = match event {
            InputEvent::ScanCode { code, down } => vec![if down {
                Operation::KeyPressed(Scancode::from_u16(code))
            } else {
                Operation::KeyReleased(Scancode::from_u16(code))
            }],
            InputEvent::Text(text) => text
                .chars()
                .flat_map(|c| text_operations(c, self.keyboard_layout))
                .collect(),
            InputEvent::Button { button, down, x, y } => {
                let button = match button {
                    PointerButton::Left => MouseButton::Left,
                    PointerButton::Right => MouseButton::Right,
                    PointerButton::Middle => MouseButton::Middle,
                    PointerButton::Back => MouseButton::X1,
                    PointerButton::Forward => MouseButton::X2,
                };
                vec![
                    Operation::MouseMove(MousePosition { x, y }),
                    if down {
                        Operation::MouseButtonPressed(button)
                    } else {
                        Operation::MouseButtonReleased(button)
                    },
                ]
            }
            InputEvent::Wheel { horizontal, amount } => {
                vec![Operation::WheelRotations(WheelRotations {
                    is_vertical: !horizontal,
                    rotation_units: amount.clamp(-255, 255),
                })]
            }
        };
        self.database.apply(operations).to_vec()
    }
    pub fn pointer(&mut self, x: u16, y: u16) -> Vec<FastPathInputEvent> {
        self.database
            .apply([Operation::MouseMove(MousePosition { x, y })])
            .to_vec()
    }
    pub fn release(&mut self) -> Vec<FastPathInputEvent> {
        self.database.release_all().to_vec()
    }
    pub fn ctrl_alt_delete(&mut self) -> Vec<FastPathInputEvent> {
        let mut events = self.release();
        for (code, down) in [
            (0x1d, true),
            (0x38, true),
            (0xe053, true),
            (0xe053, false),
            (0x38, false),
            (0x1d, false),
        ] {
            events.extend(self.apply(InputEvent::ScanCode { code, down }));
        }
        events
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use ironrdp::pdu::input::fast_path::KeyboardFlags;
    #[test]
    fn releases_include_keys_and_buttons_and_are_not_replayed() {
        let mut state = InputState::default();
        state.apply(InputEvent::ScanCode {
            code: 29,
            down: true,
        });
        state.apply(InputEvent::Button {
            button: PointerButton::Left,
            down: true,
            x: 100,
            y: 50,
        });
        assert_eq!(state.release().len(), 2);
        assert!(state.release().is_empty());
        assert_eq!(state.ctrl_alt_delete().len(), 6);
        assert!(state.release().is_empty());
    }
    #[test]
    fn spanish_dead_keys_emit_one_composition_and_leave_no_modifiers() {
        let mut state = InputState::new(0x040a);
        let events = state.apply(InputEvent::Text("áÜ".into()));
        assert!(
            events
                .iter()
                .all(|e| matches!(e, FastPathInputEvent::KeyboardEvent(..)))
        );
        assert_eq!(events.len(), 12);
        assert!(state.release().is_empty());
    }
    #[test]
    fn unicode_is_utf16_with_paired_edges_and_no_scancodes() {
        let mut state = InputState::default();
        let events = state.apply(InputEvent::Text("ñá🦀".into()));
        assert_eq!(events.len(), 8);
        assert!(
            events
                .iter()
                .all(|e| matches!(e, FastPathInputEvent::UnicodeKeyboardEvent(..)))
        );
        assert!(
            matches!(events[1],FastPathInputEvent::UnicodeKeyboardEvent(f,_) if f.contains(KeyboardFlags::RELEASE))
        );
        assert!(state.release().is_empty());
    }
}

/// xrdp maps Unicode events through the negotiated keymap. Spanish accented
/// vowels are dead-key compositions there, so emit their actual key sequence
/// once. Other characters continue to use Unicode input, including IME text.
fn text_operations(c: char, layout: u32) -> Vec<Operation> {
    if matches!(layout, 0x040a | 0x080a) {
        let lower = c.to_lowercase().next().unwrap_or(c);
        let base: Option<u16> = match lower {
            'á' => Some(30),
            'é' => Some(18),
            'í' => Some(23),
            'ó' => Some(24),
            'ú' | 'ü' => Some(22),
            _ => None,
        };
        if let Some(base) = base {
            let dead: u16 = if layout == 0x080a { 0x1a } else { 0x28 };
            let mut ops = Vec::new();
            if lower == 'ü' {
                ops.push(Operation::KeyPressed(42u16.into()));
            }
            ops.extend([
                Operation::KeyPressed(dead.into()),
                Operation::KeyReleased(dead.into()),
            ]);
            if lower == 'ü' {
                ops.push(Operation::KeyReleased(42u16.into()));
            }
            if c.is_uppercase() {
                ops.push(Operation::KeyPressed(42u16.into()));
            }
            ops.extend([
                Operation::KeyPressed(base.into()),
                Operation::KeyReleased(base.into()),
            ]);
            if c.is_uppercase() {
                ops.push(Operation::KeyReleased(42u16.into()));
            }
            return ops;
        }
    }
    vec![
        Operation::UnicodeKeyPressed(c),
        Operation::UnicodeKeyReleased(c),
    ]
}
