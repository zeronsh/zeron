//! Key presses → PTY bytes for toolkits that report keys as named keys plus
//! modifier flags (Android's soft keyboard and extra-keys row, hardware
//! keyboards). xterm conventions: DECCKM switches unmodified arrows/Home/End
//! to SS3, modified ones use the `CSI 1;<mod>` form, Alt ESC-prefixes, Ctrl
//! folds letters and a few symbols onto C0 controls.

/// A non-text key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Enter,
    Backspace,
    Tab,
    Escape,
    Up,
    Down,
    Right,
    Left,
    Home,
    End,
    Insert,
    Delete,
    PageUp,
    PageDown,
    /// F1–F12.
    F(u8),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

impl Modifiers {
    /// The xterm modifier parameter (`1` = none).
    fn param(self) -> u8 {
        1 + self.shift as u8 + 2 * self.alt as u8 + 4 * self.ctrl as u8
    }

    fn any(self) -> bool {
        self.ctrl || self.alt || self.shift
    }
}

/// Bytes for a named key. `app_cursor` is DECCKM
/// ([`crate::Emulator::app_cursor_mode`]).
pub fn key_bytes(key: Key, mods: Modifiers, app_cursor: bool) -> Vec<u8> {
    let alt_prefixed = |bytes: &[u8]| {
        let mut out = Vec::with_capacity(bytes.len() + 1);
        if mods.alt {
            out.push(0x1b);
        }
        out.extend_from_slice(bytes);
        out
    };
    // Cursor keys: `CSI <c>` / `SS3 <c>`, or `CSI 1;<mod> <c>` when modified.
    let cursor = |c: u8| {
        if mods.any() {
            format!("\x1b[1;{}{}", mods.param(), c as char).into_bytes()
        } else if app_cursor {
            vec![0x1b, b'O', c]
        } else {
            vec![0x1b, b'[', c]
        }
    };
    // Tilde keys: `CSI <n> ~` / `CSI <n>;<mod> ~`.
    let tilde = |n: u8| {
        if mods.any() {
            format!("\x1b[{n};{}~", mods.param()).into_bytes()
        } else {
            format!("\x1b[{n}~").into_bytes()
        }
    };
    match key {
        Key::Enter => alt_prefixed(b"\r"),
        Key::Backspace => alt_prefixed(if mods.ctrl { &[0x08] } else { &[0x7f] }),
        Key::Tab if mods.shift => b"\x1b[Z".to_vec(),
        Key::Tab => alt_prefixed(b"\t"),
        Key::Escape => alt_prefixed(&[0x1b]),
        Key::Up => cursor(b'A'),
        Key::Down => cursor(b'B'),
        Key::Right => cursor(b'C'),
        Key::Left => cursor(b'D'),
        Key::Home => cursor(b'H'),
        Key::End => cursor(b'F'),
        Key::Insert => tilde(2),
        Key::Delete => tilde(3),
        Key::PageUp => tilde(5),
        Key::PageDown => tilde(6),
        Key::F(n @ 1..=4) => {
            let c = b'P' + (n - 1);
            if mods.any() {
                format!("\x1b[1;{}{}", mods.param(), c as char).into_bytes()
            } else {
                vec![0x1b, b'O', c]
            }
        }
        Key::F(n) => {
            let code = match n {
                5 => 15,
                6 => 17,
                7 => 18,
                8 => 19,
                9 => 20,
                10 => 21,
                11 => 23,
                _ => 24,
            };
            tilde(code)
        }
    }
}

/// Bytes for typed text with sticky modifiers (the extra-keys row's Ctrl/Alt
/// applied to the next character). Ctrl maps letters and `@[\]^_ ?` onto C0
/// controls (caret notation) and leaves other characters as typed.
pub fn text_bytes(text: &str, mods: Modifiers) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() + 1);
    for ch in text.chars() {
        if mods.alt {
            out.push(0x1b);
        }
        match (mods.ctrl, ch) {
            (true, c) if c.is_ascii_alphabetic() => {
                out.push(c.to_ascii_lowercase() as u8 - b'a' + 1)
            }
            (true, '@' | ' ' | '2') => out.push(0x00),
            (true, '[' | '3') => out.push(0x1b),
            (true, '\\' | '4') => out.push(0x1c),
            (true, ']' | '5') => out.push(0x1d),
            (true, '^' | '6') => out.push(0x1e),
            (true, '_' | '/' | '7' | '-') => out.push(0x1f),
            (true, '?' | '8') => out.push(0x7f),
            (_, '\n') => out.push(b'\r'),
            (_, c) => {
                let mut buf = [0u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            }
        }
    }
    out
}

/// Clipboard text as PTY input: wrapped in `ESC [200~ … ESC [201~` under
/// bracketed paste (with any embedded end marker stripped so a paste can't
/// escape the bracket), newlines as CR otherwise.
pub fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    let sanitized = text.replace("\x1b[201~", "");
    if bracketed {
        let mut out = b"\x1b[200~".to_vec();
        out.extend_from_slice(sanitized.as_bytes());
        out.extend_from_slice(b"\x1b[201~");
        out
    } else {
        sanitized
            .replace("\r\n", "\r")
            .replace('\n', "\r")
            .into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Modifiers = Modifiers {
        ctrl: false,
        alt: false,
        shift: false,
    };
    const CTRL: Modifiers = Modifiers {
        ctrl: true,
        alt: false,
        shift: false,
    };
    const ALT: Modifiers = Modifiers {
        ctrl: false,
        alt: true,
        shift: false,
    };

    #[test]
    fn arrows_follow_decckm_unless_modified() {
        assert_eq!(key_bytes(Key::Up, NONE, false), b"\x1b[A");
        assert_eq!(key_bytes(Key::Up, NONE, true), b"\x1bOA");
        assert_eq!(key_bytes(Key::Left, CTRL, true), b"\x1b[1;5D");
        assert_eq!(key_bytes(Key::Right, ALT, false), b"\x1b[1;3C");
    }

    #[test]
    fn editing_and_function_keys() {
        assert_eq!(key_bytes(Key::Enter, NONE, false), b"\r");
        assert_eq!(key_bytes(Key::Backspace, NONE, false), b"\x7f");
        assert_eq!(key_bytes(Key::Escape, NONE, false), b"\x1b");
        assert_eq!(key_bytes(Key::Tab, NONE, false), b"\t");
        assert_eq!(
            key_bytes(
                Key::Tab,
                Modifiers {
                    shift: true,
                    ..NONE
                },
                false
            ),
            b"\x1b[Z"
        );
        assert_eq!(key_bytes(Key::Delete, NONE, false), b"\x1b[3~");
        assert_eq!(key_bytes(Key::PageDown, CTRL, false), b"\x1b[6;5~");
        assert_eq!(key_bytes(Key::F(1), NONE, false), b"\x1bOP");
        assert_eq!(key_bytes(Key::F(5), NONE, false), b"\x1b[15~");
        assert_eq!(key_bytes(Key::F(12), NONE, false), b"\x1b[24~");
    }

    #[test]
    fn sticky_ctrl_and_alt_on_text() {
        assert_eq!(text_bytes("c", CTRL), [0x03]);
        assert_eq!(text_bytes("D", CTRL), [0x04]);
        assert_eq!(text_bytes("[", CTRL), [0x1b]);
        assert_eq!(text_bytes(" ", CTRL), [0x00]);
        assert_eq!(text_bytes("x", ALT), b"\x1bx");
        assert_eq!(text_bytes("é\n", NONE), "é\r".as_bytes());
    }

    #[test]
    fn pastes_bracket_and_cannot_escape() {
        assert_eq!(paste_bytes("a\nb", false), b"a\rb");
        assert_eq!(
            paste_bytes("x\x1b[201~y", true),
            b"\x1b[200~xy\x1b[201~".to_vec()
        );
    }

    #[test]
    fn emulator_is_send() {
        fn send<T: Send>() {}
        send::<crate::Emulator>();
    }
}
