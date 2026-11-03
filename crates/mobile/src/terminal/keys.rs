//! Keys → PTY bytes, as the desktop terminal encodes them
//! (`crates/ui/src/terminal/view.rs` `keystroke_bytes` / `paste_bytes`):
//! arrows follow DECCKM, Ctrl folds letters to C0 controls, Alt prefixes ESC.

/// A named key (the keyboard's, the key bar's, or a hardware keyboard's).
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum TerminalKey {
    Enter,
    Backspace,
    Tab,
    Escape,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Delete,
    Insert,
    /// F1–F12.
    Function {
        number: u8,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, uniffi::Record)]
pub struct KeyModifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

pub fn key_bytes(key: TerminalKey, mods: KeyModifiers, app_cursor: bool) -> Vec<u8> {
    let mut out = if mods.alt { vec![0x1b] } else { Vec::new() };
    let seq = |csi: &[u8], ss3: &[u8]| {
        if app_cursor {
            ss3.to_vec()
        } else {
            csi.to_vec()
        }
    };
    // xterm modifier parameter: 1 + shift + 2·alt + 4·ctrl.
    let param = 1 + mods.shift as u8 + 4 * mods.ctrl as u8;
    let modified = |final_byte: u8| format!("\x1b[1;{param}{}", final_byte as char).into_bytes();
    let body = match key {
        TerminalKey::Enter => b"\r".to_vec(),
        TerminalKey::Backspace if mods.ctrl => vec![0x08],
        TerminalKey::Backspace => vec![0x7f],
        TerminalKey::Tab if mods.shift => b"\x1b[Z".to_vec(),
        TerminalKey::Tab => b"\t".to_vec(),
        TerminalKey::Escape => vec![0x1b],
        TerminalKey::Up
        | TerminalKey::Down
        | TerminalKey::Right
        | TerminalKey::Left
        | TerminalKey::Home
        | TerminalKey::End => {
            let final_byte = match key {
                TerminalKey::Up => b'A',
                TerminalKey::Down => b'B',
                TerminalKey::Right => b'C',
                TerminalKey::Left => b'D',
                TerminalKey::Home => b'H',
                _ => b'F',
            };
            if param > 1 {
                modified(final_byte)
            } else {
                seq(&[0x1b, b'[', final_byte], &[0x1b, b'O', final_byte])
            }
        }
        TerminalKey::Insert => b"\x1b[2~".to_vec(),
        TerminalKey::Delete => b"\x1b[3~".to_vec(),
        TerminalKey::PageUp => b"\x1b[5~".to_vec(),
        TerminalKey::PageDown => b"\x1b[6~".to_vec(),
        TerminalKey::Function { number } => match number {
            1 => b"\x1bOP".to_vec(),
            2 => b"\x1bOQ".to_vec(),
            3 => b"\x1bOR".to_vec(),
            4 => b"\x1bOS".to_vec(),
            5 => b"\x1b[15~".to_vec(),
            6 => b"\x1b[17~".to_vec(),
            7 => b"\x1b[18~".to_vec(),
            8 => b"\x1b[19~".to_vec(),
            9 => b"\x1b[20~".to_vec(),
            10 => b"\x1b[21~".to_vec(),
            11 => b"\x1b[23~".to_vec(),
            12 => b"\x1b[24~".to_vec(),
            _ => Vec::new(),
        },
    };
    out.extend(body);
    out
}

/// Typed text. With Ctrl held (the key bar's sticky Ctrl), a letter or one
/// of `@[\]^_?/ ` folds to its control code; Alt prefixes ESC. Return is
/// `\r` — `\n` is Ctrl-J to full-screen programs.
pub fn text_bytes(text: &str, mods: KeyModifiers) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() + 1);
    for ch in text.chars() {
        if mods.alt {
            out.push(0x1b);
        }
        if mods.ctrl
            && let Some(code) = control_code(ch)
        {
            out.push(code);
            continue;
        }
        match ch {
            '\n' => out.push(b'\r'),
            // Smart punctuation the iOS keyboard may still slip in.
            '“' | '”' => out.push(b'"'),
            '‘' | '’' => out.push(b'\''),
            _ => {
                let mut buf = [0u8; 4];
                out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
            }
        }
    }
    out
}

fn control_code(ch: char) -> Option<u8> {
    match ch.to_ascii_lowercase() {
        c @ 'a'..='z' => Some(c as u8 - b'a' + 1),
        '@' | ' ' | '2' => Some(0x00),
        '[' | '3' => Some(0x1b),
        '\\' | '4' => Some(0x1c),
        ']' | '5' => Some(0x1d),
        '^' | '6' => Some(0x1e),
        '_' | '/' | '7' => Some(0x1f),
        '?' | '8' => Some(0x7f),
        _ => None,
    }
}

/// A paste: newlines as Return, wrapped for programs that asked for
/// bracketed paste (minus the one sequence that could end the bracket early).
pub fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    let normalized = text.replace("\r\n", "\n").replace('\n', "\r");
    if bracketed {
        let mut out = b"\x1b[200~".to_vec();
        out.extend_from_slice(normalized.replace("\x1b[201~", "").as_bytes());
        out.extend_from_slice(b"\x1b[201~");
        out
    } else {
        // Without the bracket, control bytes would run as keystrokes.
        normalized
            .chars()
            .filter(|c| !c.is_control() || matches!(c, '\r' | '\t'))
            .collect::<String>()
            .into_bytes()
    }
}

/// Pointer buttons as xterm numbers them.
pub const MOUSE_LEFT: u8 = 0;
pub const MOUSE_WHEEL_UP: u8 = 64;
pub const MOUSE_WHEEL_DOWN: u8 = 65;
const MOUSE_NONE: u8 = 3;

/// A pointer event for a program that asked for them (the desktop's
/// `mouse_report`): SGR (1006) when it asked for that, else the legacy
/// encoding, which can't address cells past 222.
pub fn mouse_report(
    button: u8,
    col: usize,
    row: usize,
    release: bool,
    sgr: bool,
) -> Option<Vec<u8>> {
    let mut code = button;
    if sgr {
        let end = if release { 'm' } else { 'M' };
        return Some(format!("\x1b[<{code};{};{}{end}", col + 1, row + 1).into_bytes());
    }
    if release {
        code = (code & !0b11) | MOUSE_NONE;
    }
    let encode = |v: usize| u8::try_from(v + 1 + 32).ok().filter(|b| *b < 255);
    Some(vec![
        0x1b,
        b'[',
        b'M',
        32 + code,
        encode(col)?,
        encode(row)?,
    ])
}

/// Focus in/out for programs that asked (1004).
pub fn focus_report(focused: bool) -> &'static [u8] {
    if focused { b"\x1b[I" } else { b"\x1b[O" }
}

/// Wheel rows as arrow keys for a full-screen program without pointer
/// reporting (alternate scroll, 1007). Positive rows scroll up.
pub fn wheel_arrows(rows: i32, app_cursor: bool) -> Vec<u8> {
    let arrow: &[u8] = match (rows > 0, app_cursor) {
        (true, true) => b"\x1bOA",
        (true, false) => b"\x1b[A",
        (false, true) => b"\x1bOB",
        (false, false) => b"\x1b[B",
    };
    arrow.repeat(rows.unsigned_abs() as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: KeyModifiers = KeyModifiers {
        ctrl: false,
        alt: false,
        shift: false,
    };

    #[test]
    fn arrows_follow_the_cursor_mode() {
        assert_eq!(key_bytes(TerminalKey::Up, NONE, false), b"\x1b[A");
        assert_eq!(key_bytes(TerminalKey::Up, NONE, true), b"\x1bOA");
        let ctrl = KeyModifiers { ctrl: true, ..NONE };
        assert_eq!(key_bytes(TerminalKey::Left, ctrl, false), b"\x1b[1;5D");
    }

    #[test]
    fn ctrl_and_alt_fold_typed_text() {
        let ctrl = KeyModifiers { ctrl: true, ..NONE };
        assert_eq!(text_bytes("c", ctrl), vec![0x03]);
        assert_eq!(text_bytes("C", ctrl), vec![0x03]);
        assert_eq!(text_bytes("[", ctrl), vec![0x1b]);
        let alt = KeyModifiers { alt: true, ..NONE };
        assert_eq!(text_bytes("b", alt), b"\x1bb");
        assert_eq!(text_bytes("hi\n", NONE), b"hi\r");
        assert_eq!(text_bytes("“x”", NONE), b"\"x\"");
    }

    #[test]
    fn pointer_reports_in_both_encodings() {
        assert_eq!(
            mouse_report(MOUSE_LEFT, 4, 1, false, true).unwrap(),
            b"\x1b[<0;5;2M"
        );
        assert_eq!(
            mouse_report(MOUSE_LEFT, 4, 1, true, true).unwrap(),
            b"\x1b[<0;5;2m"
        );
        assert_eq!(
            mouse_report(MOUSE_LEFT, 0, 0, true, false).unwrap(),
            vec![0x1b, b'[', b'M', 32 + 3, 33, 33]
        );
        assert_eq!(
            mouse_report(MOUSE_LEFT, 300, 0, false, false),
            None,
            "legacy can't reach col 301"
        );
        assert_eq!(wheel_arrows(-2, false), b"\x1b[B\x1b[B");
    }

    #[test]
    fn pastes_bracket_or_strip_controls() {
        assert_eq!(paste_bytes("a\nb", true), b"\x1b[200~a\rb\x1b[201~");
        assert_eq!(paste_bytes("a\x1b[201~b", true), b"\x1b[200~ab\x1b[201~");
        assert_eq!(paste_bytes("ls\x03\n", false), b"ls\r");
    }
}
