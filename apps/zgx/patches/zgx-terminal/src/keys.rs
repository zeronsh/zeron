//! What keys, the mouse and pastes send a program, as xterm sends them.
use alacritty_terminal::term::TermMode;
use zgx::input::{Key, Modifiers, PointerButton};

/// xterm's modifier parameter: 1 plus shift 1, alt 2, control 4.
fn parameter(modifiers: Modifiers) -> u8 {
    1 + u8::from(modifiers.shift) + 2 * u8::from(modifiers.alt) + 4 * u8::from(modifiers.control)
}

/// What `key` sends with `modifiers` in `mode`, or `None` for a key that
/// types text (the text event sends it) or that the app keeps (⌘ chords on
/// macOS).
pub(crate) fn encode(key: &Key, modifiers: Modifiers, mode: TermMode) -> Option<Vec<u8>> {
    if modifiers.meta {
        return None;
    }
    let m = parameter(modifiers);
    let plain = m == 1;
    let esc = |bytes: &[u8]| -> Vec<u8> {
        if modifiers.alt {
            [b"\x1b".as_slice(), bytes].concat()
        } else {
            bytes.to_vec()
        }
    };
    // A cursor key: CSI or SS3 (application mode) unmodified, CSI 1;m
    // modified.
    let cursor = |final_byte: u8| -> Vec<u8> {
        if plain {
            if mode.contains(TermMode::APP_CURSOR) {
                vec![0x1b, b'O', final_byte]
            } else {
                vec![0x1b, b'[', final_byte]
            }
        } else {
            format!("\x1b[1;{m}{}", final_byte as char).into_bytes()
        }
    };
    let tilde = |number: u8| -> Vec<u8> {
        if plain {
            format!("\x1b[{number}~").into_bytes()
        } else {
            format!("\x1b[{number};{m}~").into_bytes()
        }
    };
    Some(match key {
        Key::Enter => esc(b"\r"),
        Key::Tab if modifiers.shift => b"\x1b[Z".to_vec(),
        Key::Tab => esc(b"\t"),
        Key::Backspace if modifiers.control => esc(b"\x08"),
        Key::Backspace => esc(b"\x7f"),
        Key::Escape => esc(b"\x1b"),
        Key::Space if modifiers.control => esc(b"\0"),
        Key::Space if modifiers.alt && !cfg!(target_os = "macos") => b"\x1b ".to_vec(),
        Key::Space => return None,
        Key::ArrowUp => cursor(b'A'),
        Key::ArrowDown => cursor(b'B'),
        Key::ArrowRight => cursor(b'C'),
        Key::ArrowLeft => cursor(b'D'),
        Key::Home => cursor(b'H'),
        Key::End => cursor(b'F'),
        Key::Insert => tilde(2),
        Key::Delete => tilde(3),
        Key::PageUp => tilde(5),
        Key::PageDown => tilde(6),
        Key::Function(n @ 1..=4) => {
            let final_byte = b'P' + (n - 1);
            if plain {
                vec![0x1b, b'O', final_byte]
            } else {
                format!("\x1b[1;{m}{}", final_byte as char).into_bytes()
            }
        }
        Key::Function(n @ 5..=20) => {
            const NUMBERS: [u8; 16] = [
                15, 17, 18, 19, 20, 21, 23, 24, 25, 26, 28, 29, 31, 32, 33, 34,
            ];
            tilde(NUMBERS[usize::from(*n) - 5])
        }
        Key::Function(_) | Key::ContextMenu => return None,
        Key::Character(text) => {
            let mut chars = text.chars();
            let (Some(c), None) = (chars.next(), chars.next()) else {
                return None;
            };
            if modifiers.control {
                let control = match c.to_ascii_lowercase() {
                    c @ 'a'..='z' => c as u8 & 0x1f,
                    '@' | '2' | ' ' => 0,
                    '[' | '3' => 0x1b,
                    '\\' | '4' => 0x1c,
                    ']' | '5' => 0x1d,
                    '^' | '6' => 0x1e,
                    '_' | '-' | '/' | '7' => 0x1f,
                    '?' | '8' => 0x7f,
                    _ => return None,
                };
                esc(&[control])
            } else if modifiers.alt && !cfg!(target_os = "macos") {
                // Option types its own characters on macOS (the text
                // event); elsewhere Alt is Meta: ESC before the key.
                esc(c.to_string().as_bytes())
            } else {
                return None;
            }
        }
    })
}

/// A key event's kind, as the kitty keyboard protocol reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KeyEvent {
    Press,
    Repeat,
    Release,
}

/// The kitty keyboard protocol's flags a program asked for.
fn kitty_flags(mode: TermMode) -> TermMode {
    mode & TermMode::KITTY_KEYBOARD_PROTOCOL
}

/// The unshifted key a physical key is on a US layout (`KeyCode` names, as
/// `EventContext::key_code` gives them), for the protocol's base keys.
fn us_key(code: &str) -> Option<char> {
    if let Some(letter) = code.strip_prefix("Key") {
        return letter.chars().next().map(|c| c.to_ascii_lowercase());
    }
    if let Some(digit) = code.strip_prefix("Digit") {
        return digit.chars().next();
    }
    Some(match code {
        "Minus" => '-',
        "Equal" => '=',
        "BracketLeft" => '[',
        "BracketRight" => ']',
        "Backslash" => '\\',
        "Semicolon" => ';',
        "Quote" => '\'',
        "Comma" => ',',
        "Period" => '.',
        "Slash" => '/',
        "Backquote" => '`',
        _ => return None,
    })
}

/// What `key` sends under the kitty keyboard protocol (`CSI … u` and the
/// legacy forms it keeps), when the program turned it on: `None` when it
/// isn't on (xterm's encodings apply), or the key types text the text
/// event sends, or it reports nothing (a release it didn't ask for).
/// `code` is the physical key, `text` what the key types.
pub(crate) fn kitty(
    key: &Key,
    modifiers: Modifiers,
    event: KeyEvent,
    code: Option<&str>,
    mode: TermMode,
) -> Option<Option<Vec<u8>>> {
    let flags = kitty_flags(mode);
    if flags.is_empty() {
        return None;
    }
    // The app's: ⌘ chords on macOS.
    if modifiers.meta && cfg!(target_os = "macos") {
        return Some(None);
    }
    let all_keys = flags.contains(TermMode::REPORT_ALL_KEYS_AS_ESC);
    let events = flags.contains(TermMode::REPORT_EVENT_TYPES);
    let alternates = flags.contains(TermMode::REPORT_ALTERNATE_KEYS);
    if event != KeyEvent::Press && !events {
        // Repeats are presses to a program that doesn't tell them apart.
        if event == KeyEvent::Repeat {
            return kitty(key, modifiers, KeyEvent::Press, code, mode);
        }
        return Some(None);
    }
    let bits = u8::from(modifiers.shift)
        + 2 * u8::from(modifiers.alt)
        + 4 * u8::from(modifiers.control)
        + 8 * u8::from(modifiers.meta);
    let parameter = |number: Option<u32>| -> String {
        let event = match event {
            KeyEvent::Press => "",
            KeyEvent::Repeat => ":2",
            KeyEvent::Release => ":3",
        };
        match (bits, event, number) {
            (0, "", _) => String::new(),
            _ => format!("{}{event}", bits + 1),
        }
    };
    // A key with a legacy CSI form: `CSI 1;mods X` or `CSI number;mods ~`.
    let legacy = |number: u32, final_byte: char| -> Vec<u8> {
        let mods = parameter(Some(number));
        let mut out = if mods.is_empty() {
            if number == 1 {
                format!("\x1b[{final_byte}")
            } else {
                format!("\x1b[{number}{final_byte}")
            }
        } else {
            format!("\x1b[{number};{mods}{final_byte}")
        };
        if final_byte != '~'
            && number == 1
            && mods.is_empty()
            && mode.contains(TermMode::APP_CURSOR)
        {
            out = format!("\x1bO{final_byte}");
        }
        out.into_bytes()
    };
    let csi_u =
        |number: u32, shifted: Option<u32>, base: Option<u32>, text: Option<&str>| -> Vec<u8> {
            let mut key = number.to_string();
            if alternates && (shifted.is_some() || base.is_some()) {
                key.push(':');
                if let Some(shifted) = shifted {
                    key.push_str(&shifted.to_string());
                }
                if let Some(base) = base {
                    key.push(':');
                    key.push_str(&base.to_string());
                }
            }
            let mods = parameter(Some(number));
            let text = text
                .filter(|_| {
                    flags.contains(TermMode::REPORT_ASSOCIATED_TEXT) && event != KeyEvent::Release
                })
                .map(|text| {
                    text.chars()
                        .map(|c| (c as u32).to_string())
                        .collect::<Vec<_>>()
                        .join(":")
                });
            let out = match (mods.is_empty(), text) {
                (true, None) => format!("\x1b[{key}u"),
                (false, None) => format!("\x1b[{key};{mods}u"),
                (true, Some(text)) => format!("\x1b[{key};;{text}u"),
                (false, Some(text)) => format!("\x1b[{key};{mods};{text}u"),
            };
            out.into_bytes()
        };
    // Enter, Tab and Backspace stay as typed unless modified or every key
    // is asked for; Escape is always disambiguated.
    let plain_or = |plain: &[u8], number: u32| -> Option<Vec<u8>> {
        if all_keys || bits != 0 || event != KeyEvent::Press {
            Some(csi_u(number, None, None, None))
        } else {
            Some(plain.to_vec())
        }
    };
    Some(match key {
        Key::Escape => Some(csi_u(27, None, None, None)),
        Key::Enter => plain_or(b"\r", 13),
        Key::Tab => plain_or(b"\t", 9),
        Key::Backspace => plain_or(b"\x7f", 127),
        Key::ArrowUp => Some(legacy(1, 'A')),
        Key::ArrowDown => Some(legacy(1, 'B')),
        Key::ArrowRight => Some(legacy(1, 'C')),
        Key::ArrowLeft => Some(legacy(1, 'D')),
        Key::Home => Some(legacy(1, 'H')),
        Key::End => Some(legacy(1, 'F')),
        Key::Insert => Some(legacy(2, '~')),
        Key::Delete => Some(legacy(3, '~')),
        Key::PageUp => Some(legacy(5, '~')),
        Key::PageDown => Some(legacy(6, '~')),
        Key::Function(1) => Some(legacy(1, 'P')),
        Key::Function(2) => Some(legacy(1, 'Q')),
        Key::Function(3) => Some(legacy(13, '~')),
        Key::Function(4) => Some(legacy(1, 'S')),
        Key::Function(n @ 5..=12) => {
            const NUMBERS: [u32; 8] = [15, 17, 18, 19, 20, 21, 23, 24];
            Some(legacy(NUMBERS[usize::from(*n) - 5], '~'))
        }
        Key::Function(n @ 13..=35) => Some(csi_u(57376 + u32::from(*n) - 13, None, None, None)),
        Key::Function(_) => None,
        Key::ContextMenu => Some(csi_u(57363, None, None, None)),
        Key::Space | Key::Character(_) => {
            let typed = match key {
                Key::Character(text) => text.as_str(),
                _ => " ",
            };
            let mut chars = typed.chars();
            let (Some(c), None) = (chars.next(), chars.next()) else {
                // A key typing several characters (a dead key's): text.
                return Some(None);
            };
            // Text the key types, unmodified or only shifted, goes as
            // text unless every key is asked for.
            let texty = bits & !1 == 0;
            if texty && !all_keys {
                return Some(None);
            }
            let base = code.and_then(us_key);
            // The key's own (unshifted) code: a letter lower case.
            let unshifted = if c.is_alphabetic() {
                c.to_lowercase().next().unwrap_or(c)
            } else if modifiers.shift {
                base.unwrap_or(c)
            } else {
                c
            };
            let shifted = (modifiers.shift && c != unshifted).then_some(c as u32);
            let base = base.filter(|b| *b != unshifted).map(|b| b as u32);
            let text = texty.then_some(typed);
            Some(csi_u(unshifted as u32, shifted, base, text))
        }
    })
}

/// `text` as a paste: bracketed when the program asked (with any end
/// marker in it taken out, so the paste can't end itself early), else with
/// its line ends as Enter.
pub(crate) fn paste(text: &str, mode: TermMode) -> Vec<u8> {
    if mode.contains(TermMode::BRACKETED_PASTE) {
        let inner = text.replace("\x1b[201~", "").replace('\x1b', "");
        format!("\x1b[200~{inner}\x1b[201~").into_bytes()
    } else {
        text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
    }
}

/// A mouse report's event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mouse {
    Press(PointerButton),
    Release(PointerButton),
    /// Moved, with this button held (or none).
    Move(Option<PointerButton>),
    /// The wheel, a line up (`true`) or down.
    Wheel(bool),
}

/// The report of `event` at cell (`column`, `line`), 0-based, if the
/// program's mouse mode wants it.
pub(crate) fn mouse(
    event: Mouse,
    column: usize,
    line: usize,
    modifiers: Modifiers,
    mode: TermMode,
) -> Option<Vec<u8>> {
    if !mode.intersects(TermMode::MOUSE_MODE) {
        return None;
    }
    let button_code = |button: PointerButton| match button {
        PointerButton::Primary => Some(0),
        PointerButton::Middle => Some(1),
        PointerButton::Secondary => Some(2),
        PointerButton::Other(_) => None,
    };
    let (mut code, release) = match event {
        Mouse::Press(button) => (button_code(button)?, false),
        Mouse::Release(button) => (button_code(button)?, true),
        Mouse::Move(held) => {
            match held {
                Some(_) if !mode.intersects(TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION) => {
                    return None;
                }
                None if !mode.contains(TermMode::MOUSE_MOTION) => return None,
                _ => {}
            }
            (32 + held.and_then(button_code).unwrap_or(3), false)
        }
        Mouse::Wheel(up) => (if up { 64 } else { 65 }, false),
    };
    code += 4 * u8::from(modifiers.shift)
        + 8 * u8::from(modifiers.alt)
        + 16 * u8::from(modifiers.control);
    if mode.contains(TermMode::SGR_MOUSE) {
        let end = if release { 'm' } else { 'M' };
        return Some(format!("\x1b[<{code};{};{}{end}", column + 1, line + 1).into_bytes());
    }
    // X10/normal: release is button 3; positions offset by 32, one byte
    // each (two in UTF-8 mode, up to 2015).
    if release {
        code = (code & !3) | 3;
    }
    let mut report = vec![0x1b, b'[', b'M', 32 + code];
    for position in [column, line] {
        let value = position + 1 + 32;
        if mode.contains(TermMode::UTF8_MOUSE) {
            let c = char::from_u32(value.min(2047) as u32)?;
            let mut buffer = [0; 4];
            report.extend_from_slice(c.encode_utf8(&mut buffer).as_bytes());
        } else {
            report.push(value.min(255) as u8);
        }
    }
    Some(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mods(shift: bool, alt: bool, control: bool) -> Modifiers {
        Modifiers {
            shift,
            alt,
            control,
            meta: false,
        }
    }

    #[test]
    fn keys_send_what_xterm_sends() {
        let none = mods(false, false, false);
        let normal = TermMode::empty();
        let send = |key: Key, m: Modifiers, mode: TermMode| encode(&key, m, mode);
        assert_eq!(send(Key::Enter, none, normal).unwrap(), b"\r");
        assert_eq!(send(Key::Backspace, none, normal).unwrap(), b"\x7f");
        assert_eq!(
            send(Key::Tab, mods(true, false, false), normal).unwrap(),
            b"\x1b[Z"
        );
        assert_eq!(send(Key::ArrowUp, none, normal).unwrap(), b"\x1b[A");
        assert_eq!(
            send(Key::ArrowUp, none, TermMode::APP_CURSOR).unwrap(),
            b"\x1bOA"
        );
        assert_eq!(
            send(Key::ArrowLeft, mods(false, false, true), normal).unwrap(),
            b"\x1b[1;5D"
        );
        assert_eq!(
            send(Key::Delete, mods(true, false, false), normal).unwrap(),
            b"\x1b[3;2~"
        );
        assert_eq!(send(Key::Function(1), none, normal).unwrap(), b"\x1bOP");
        assert_eq!(send(Key::Function(5), none, normal).unwrap(), b"\x1b[15~");
        assert_eq!(
            send(Key::Function(12), mods(false, false, true), normal).unwrap(),
            b"\x1b[24;5~"
        );
        let ctrl = mods(false, false, true);
        assert_eq!(
            send(Key::Character("c".into()), ctrl, normal).unwrap(),
            b"\x03"
        );
        assert_eq!(
            send(Key::Character("[".into()), ctrl, normal).unwrap(),
            b"\x1b"
        );
        assert_eq!(send(Key::Space, ctrl, normal).unwrap(), b"\0");
        // Plain characters are the text event's to send.
        assert_eq!(send(Key::Character("a".into()), none, normal), None);
        assert_eq!(send(Key::Space, none, normal), None);
        // The app's: ⌘ chords.
        let meta = Modifiers { meta: true, ..none };
        assert_eq!(send(Key::Character("t".into()), meta, normal), None);
    }

    fn kitty_send(
        key: Key,
        m: Modifiers,
        event: KeyEvent,
        code: Option<&str>,
        mode: TermMode,
    ) -> Option<Vec<u8>> {
        kitty(&key, m, event, code, mode).expect("kitty is on")
    }

    #[test]
    fn kitty_disambiguates_as_the_protocol_says() {
        let none = mods(false, false, false);
        let ctrl = mods(false, false, true);
        let shift = mods(true, false, false);
        let on = TermMode::DISAMBIGUATE_ESC_CODES;
        let press = KeyEvent::Press;
        // Off: xterm's.
        assert_eq!(
            kitty(&Key::Escape, none, press, None, TermMode::empty()),
            None
        );
        assert_eq!(
            kitty_send(Key::Escape, none, press, None, on).unwrap(),
            b"\x1b[27u"
        );
        assert_eq!(
            kitty_send(Key::Character("c".into()), ctrl, press, Some("KeyC"), on).unwrap(),
            b"\x1b[99;5u"
        );
        assert_eq!(
            kitty_send(
                Key::Character("a".into()),
                mods(false, true, false),
                press,
                Some("KeyA"),
                on
            )
            .unwrap(),
            b"\x1b[97;3u"
        );
        // Text keys go as text, shifted or not; Enter, Tab and Backspace too.
        assert_eq!(
            kitty_send(Key::Character("a".into()), none, press, Some("KeyA"), on),
            None
        );
        assert_eq!(
            kitty_send(Key::Character("A".into()), shift, press, Some("KeyA"), on),
            None
        );
        assert_eq!(
            kitty_send(Key::Enter, none, press, None, on).unwrap(),
            b"\r"
        );
        assert_eq!(
            kitty_send(Key::Enter, shift, press, None, on).unwrap(),
            b"\x1b[13;2u"
        );
        assert_eq!(
            kitty_send(Key::Tab, shift, press, None, on).unwrap(),
            b"\x1b[9;2u"
        );
        assert_eq!(
            kitty_send(Key::Backspace, ctrl, press, None, on).unwrap(),
            b"\x1b[127;5u"
        );
        assert_eq!(
            kitty_send(Key::Space, ctrl, press, Some("Space"), on).unwrap(),
            b"\x1b[32;5u"
        );
        // Functional keys keep their legacy forms; F3 is CSI 13 ~.
        assert_eq!(
            kitty_send(Key::ArrowUp, none, press, None, on).unwrap(),
            b"\x1b[A"
        );
        assert_eq!(
            kitty_send(Key::ArrowUp, ctrl, press, None, on).unwrap(),
            b"\x1b[1;5A"
        );
        assert_eq!(
            kitty_send(Key::Function(3), none, press, None, on).unwrap(),
            b"\x1b[13~"
        );
        assert_eq!(
            kitty_send(Key::Delete, shift, press, None, on).unwrap(),
            b"\x1b[3;2~"
        );
        assert_eq!(
            kitty_send(Key::Function(13), none, press, None, on).unwrap(),
            b"\x1b[57376u"
        );
        // Releases only when asked for.
        assert_eq!(
            kitty_send(Key::Escape, none, KeyEvent::Release, None, on),
            None
        );
    }

    #[test]
    fn kitty_reports_events_alternates_and_every_key_when_asked() {
        let none = mods(false, false, false);
        let shift = mods(true, false, false);
        let events = TermMode::DISAMBIGUATE_ESC_CODES | TermMode::REPORT_EVENT_TYPES;
        assert_eq!(
            kitty_send(Key::Escape, none, KeyEvent::Repeat, None, events).unwrap(),
            b"\x1b[27;1:2u"
        );
        assert_eq!(
            kitty_send(Key::Escape, none, KeyEvent::Release, None, events).unwrap(),
            b"\x1b[27;1:3u"
        );
        assert_eq!(
            kitty_send(Key::ArrowLeft, none, KeyEvent::Release, None, events).unwrap(),
            b"\x1b[1;1:3D"
        );
        // Enter's release, as Ghostty reports it.
        assert_eq!(
            kitty_send(Key::Enter, none, KeyEvent::Release, None, events).unwrap(),
            b"\x1b[13;1:3u"
        );
        // Text keys still type text, and have no release to report.
        assert_eq!(
            kitty_send(
                Key::Character("a".into()),
                none,
                KeyEvent::Release,
                Some("KeyA"),
                events
            ),
            None
        );
        let all = TermMode::DISAMBIGUATE_ESC_CODES | TermMode::REPORT_ALL_KEYS_AS_ESC;
        assert_eq!(
            kitty_send(
                Key::Character("a".into()),
                none,
                KeyEvent::Press,
                Some("KeyA"),
                all
            )
            .unwrap(),
            b"\x1b[97u"
        );
        assert_eq!(
            kitty_send(Key::Enter, none, KeyEvent::Press, None, all).unwrap(),
            b"\x1b[13u"
        );
        let alternates = all | TermMode::REPORT_ALTERNATE_KEYS;
        assert_eq!(
            kitty_send(
                Key::Character("A".into()),
                shift,
                KeyEvent::Press,
                Some("KeyA"),
                alternates
            )
            .unwrap(),
            b"\x1b[97:65;2u"
        );
        assert_eq!(
            kitty_send(
                Key::Character("!".into()),
                shift,
                KeyEvent::Press,
                Some("Digit1"),
                alternates
            )
            .unwrap(),
            b"\x1b[49:33;2u"
        );
        let text = all | TermMode::REPORT_ASSOCIATED_TEXT;
        assert_eq!(
            kitty_send(
                Key::Character("a".into()),
                none,
                KeyEvent::Press,
                Some("KeyA"),
                text
            )
            .unwrap(),
            b"\x1b[97;;97u"
        );
        assert_eq!(
            kitty_send(
                Key::Character("A".into()),
                shift,
                KeyEvent::Press,
                Some("KeyA"),
                text
            )
            .unwrap(),
            b"\x1b[97;2;65u"
        );
    }

    #[test]
    fn pastes_are_bracketed_and_cant_end_themselves() {
        assert_eq!(paste("a\nb\r\nc", TermMode::empty()), b"a\rb\rc");
        assert_eq!(
            paste("x\x1b[201~rm -rf", TermMode::BRACKETED_PASTE),
            b"\x1b[200~xrm -rf\x1b[201~"
        );
    }

    #[test]
    fn mouse_reports_in_each_encoding() {
        let none = mods(false, false, false);
        let click = TermMode::MOUSE_REPORT_CLICK;
        let left = PointerButton::Primary;
        assert_eq!(
            mouse(Mouse::Press(left), 0, 0, none, TermMode::empty()),
            None
        );
        assert_eq!(
            mouse(Mouse::Press(left), 2, 4, none, click).unwrap(),
            b"\x1b[M\x20\x23\x25"
        );
        assert_eq!(
            mouse(Mouse::Release(left), 2, 4, none, click).unwrap(),
            b"\x1b[M\x23\x23\x25"
        );
        let sgr = click | TermMode::SGR_MOUSE;
        assert_eq!(
            mouse(Mouse::Press(left), 2, 4, mods(false, false, true), sgr).unwrap(),
            b"\x1b[<16;3;5M"
        );
        assert_eq!(
            mouse(Mouse::Release(left), 2, 4, none, sgr).unwrap(),
            b"\x1b[<0;3;5m"
        );
        assert_eq!(
            mouse(Mouse::Wheel(true), 0, 0, none, sgr).unwrap(),
            b"\x1b[<64;1;1M"
        );
        // Motion only as the mode asks.
        assert_eq!(mouse(Mouse::Move(None), 0, 0, none, sgr), None);
        assert_eq!(mouse(Mouse::Move(Some(left)), 0, 0, none, sgr), None);
        let drag = sgr | TermMode::MOUSE_DRAG;
        assert_eq!(
            mouse(Mouse::Move(Some(left)), 1, 1, none, drag).unwrap(),
            b"\x1b[<32;2;2M"
        );
        let motion = sgr | TermMode::MOUSE_MOTION;
        assert_eq!(
            mouse(Mouse::Move(None), 1, 1, none, motion).unwrap(),
            b"\x1b[<35;2;2M"
        );
    }
}
