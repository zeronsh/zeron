//! Keystroke → PTY bytes, through Ghostty's own key encoder: legacy
//! sequences, xterm modifyOtherKeys and the kitty keyboard protocol, each as
//! the running program asked for them. On macOS, Ghostty's default
//! line-editing keybinds come first (Cmd+←/→/⌫, Option+←/→).
//!
//! Which keys are bytes and which are typed text is [`super::view::route_key`]'s
//! call; this encodes the bytes.

use ghostty_vt::Terminal;
use ghostty_vt::key::{Action, Encoder, Event, Key, Mods, OptionAsAlt};
use gpui::Modifiers;

/// Encode a keystroke for the program on `term`, which [`super::view::route_key`]
/// decided is bytes rather than typed text. `key_char` is the text the key
/// stands for (for Alt as Meta, the key without Option). `None` means "not
/// ours": a lone modifier, or a key with nothing to send.
pub fn encode(
    term: &Terminal,
    key: &str,
    key_char: Option<&str>,
    mods: &Modifiers,
) -> Option<Vec<u8>> {
    let mut chars = key.chars();
    let single = chars.next().filter(|_| chars.next().is_none());
    let physical = physical_key(key);
    // The text the layout produced, never a control character (Ghostty derives
    // those from the key and modifiers).
    let text = key_char
        .filter(|t| !t.is_empty() && !t.chars().any(char::is_control))
        .or(single.map(|_| key))
        .or((key == "space").then_some(" "));
    if physical == Key::Unidentified && text.is_none() {
        return None;
    }
    let mut ghostty_mods = Mods::empty();
    for (on, bit) in [
        (mods.shift, Mods::SHIFT),
        (mods.alt, Mods::ALT),
        (mods.control, Mods::CTRL),
    ] {
        if on {
            ghostty_mods |= bit;
        }
    }
    // Shift used up making the text (`A`), so it is not also a modifier.
    let consumed = match (text, single) {
        (Some(text), Some(base)) if mods.shift && !text.chars().eq(std::iter::once(base)) => {
            Mods::SHIFT
        }
        _ => Mods::empty(),
    };

    let mut event = Event::new().ok()?;
    event
        .set_action(Action::Press)
        .set_key(physical)
        .set_mods(ghostty_mods)
        .set_consumed_mods(consumed)
        .set_utf8(text);
    if let Some(base) = single {
        event.set_unshifted_codepoint(base);
    }
    let mut encoder = Encoder::new().ok()?;
    encoder.set_options_from_terminal(term);
    // Option reaches here only as Meta: `route_key` sends the symbols a layout
    // types with Option as text.
    encoder.set_macos_option_as_alt(OptionAsAlt::True);
    let mut out = Vec::with_capacity(8);
    encoder.encode_to_vec(&event, &mut out).ok()?;
    (!out.is_empty()).then_some(out)
}

/// Ghostty's default macOS keybinds for a shell's line editor: Cmd+← / Cmd+→
/// to the line's start or end (Ctrl+A, Ctrl+E), Cmd+⌫ deletes to its start
/// (Ctrl+U), Option+← / Option+→ a word back or forward (ESC b, ESC f).
pub fn macos_keybind(key: &str, mods: &Modifiers) -> Option<Vec<u8>> {
    if !cfg!(target_os = "macos") || mods.shift || mods.control {
        return None;
    }
    let bytes: &[u8] = match (mods.platform, mods.alt, key) {
        (true, false, "left") => b"\x01",
        (true, false, "right") => b"\x05",
        (true, false, "backspace") => b"\x15",
        (false, true, "left") => b"\x1bb",
        (false, true, "right") => b"\x1bf",
        _ => return None,
    };
    Some(bytes.to_vec())
}

/// gpui's key name → the US-layout physical key Ghostty encodes by.
// ponytail: gpui gives no keycode, so a key another layout types (`ж`) is
// Unidentified and goes as its text; kitty's base-layout key for it is lost.
fn physical_key(key: &str) -> Key {
    use Key::*;
    const LETTERS: [Key; 26] = [
        A, B, C, D, E, F, G, H, I, J, K, L, M, N, O, P, Q, R, S, T, U, V, W, X, Y, Z,
    ];
    const DIGITS: [Key; 10] = [
        Digit0, Digit1, Digit2, Digit3, Digit4, Digit5, Digit6, Digit7, Digit8, Digit9,
    ];
    const FUNCTION: [Key; 24] = [
        F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12, F13, F14, F15, F16, F17, F18, F19, F20,
        F21, F22, F23, F24,
    ];
    let mut chars = key.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        return match c.to_ascii_lowercase() {
            c @ 'a'..='z' => LETTERS[(c as u8 - b'a') as usize],
            c @ '0'..='9' => DIGITS[(c as u8 - b'0') as usize],
            '`' => Backquote,
            '\\' => Backslash,
            '[' => BracketLeft,
            ']' => BracketRight,
            ',' => Comma,
            '=' => Equal,
            '-' => Minus,
            '.' => Period,
            '\'' => Quote,
            ';' => Semicolon,
            '/' => Slash,
            ' ' => Space,
            _ => Unidentified,
        };
    }
    if let Some(n) = key.strip_prefix('f').and_then(|n| n.parse::<usize>().ok())
        && (1..=FUNCTION.len()).contains(&n)
    {
        return FUNCTION[n - 1];
    }
    match key {
        "enter" => Enter,
        "tab" => Tab,
        "backspace" => Backspace,
        "escape" => Escape,
        "space" => Space,
        "up" => ArrowUp,
        "down" => ArrowDown,
        "left" => ArrowLeft,
        "right" => ArrowRight,
        "home" => Home,
        "end" => End,
        "pageup" => PageUp,
        "pagedown" => PageDown,
        "insert" => Insert,
        "delete" => Delete,
        _ => Unidentified,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::emulator::Emulator;

    fn mods() -> Modifiers {
        Modifiers::default()
    }

    /// Encode against a fresh terminal, after feeding it `setup`.
    fn enc(setup: &[u8], key: &str, key_char: Option<&str>, mods: Modifiers) -> Option<Vec<u8>> {
        let mut e = Emulator::new(20, 4);
        e.feed(setup);
        encode(e.terminal(), key, key_char, &mods)
    }

    fn plain(key: &str, key_char: Option<&str>, mods: Modifiers) -> Option<Vec<u8>> {
        enc(b"", key, key_char, mods)
    }

    #[test]
    fn named_keys_and_sequences() {
        assert_eq!(plain("enter", None, mods()), Some(b"\r".to_vec()));
        assert_eq!(plain("backspace", None, mods()), Some(vec![0x7f]));
        assert_eq!(plain("tab", None, mods()), Some(b"\t".to_vec()));
        let shift = Modifiers {
            shift: true,
            ..mods()
        };
        assert_eq!(plain("tab", None, shift), Some(b"\x1b[Z".to_vec()));
        assert_eq!(plain("escape", None, mods()), Some(vec![0x1b]));
        assert_eq!(plain("space", None, mods()), Some(b" ".to_vec()));
        assert_eq!(plain("delete", None, mods()), Some(b"\x1b[3~".to_vec()));
        assert_eq!(plain("pageup", None, mods()), Some(b"\x1b[5~".to_vec()));
        assert_eq!(plain("f1", None, mods()), Some(b"\x1bOP".to_vec()));
        assert_eq!(plain("f5", None, mods()), Some(b"\x1b[15~".to_vec()));
        // Unknown multi-char keys are not ours.
        assert_eq!(plain("capslock", None, mods()), None);
    }

    #[test]
    fn arrows_follow_the_cursor_key_mode_and_carry_modifiers() {
        assert_eq!(plain("up", None, mods()), Some(b"\x1b[A".to_vec()));
        assert_eq!(
            enc(b"\x1b[?1h", "up", None, mods()),
            Some(b"\x1bOA".to_vec())
        );
        assert_eq!(
            enc(b"\x1b[?1h", "end", None, mods()),
            Some(b"\x1bOF".to_vec())
        );
        let shift = Modifiers {
            shift: true,
            ..mods()
        };
        assert_eq!(plain("up", None, shift), Some(b"\x1b[1;2A".to_vec()));
    }

    #[test]
    fn ctrl_combos_map_to_control_bytes() {
        let ctrl = Modifiers {
            control: true,
            ..mods()
        };
        assert_eq!(plain("c", Some("c"), ctrl), Some(vec![0x03]));
        assert_eq!(plain("z", None, ctrl), Some(vec![0x1a]));
        assert_eq!(plain("space", None, ctrl), Some(vec![0x00]));
        assert_eq!(plain("\\", None, ctrl), Some(vec![0x1c]));
        // Ghostty follows fixterms here: Ctrl+[ (and Ctrl+I, Ctrl+M) go as
        // CSI u, so a program can tell them from Escape, Tab and Enter.
        assert_eq!(plain("[", None, ctrl), Some(b"\x1b[91;5u".to_vec()));
    }

    /// Alt reaches the encoder as Meta: an ESC prefix on text, a modifier
    /// parameter on named keys.
    #[test]
    fn alt_is_meta() {
        let alt = Modifiers {
            alt: true,
            ..mods()
        };
        assert_eq!(plain("b", Some("b"), alt), Some(b"\x1bb".to_vec()));
        let alt_shift = Modifiers { shift: true, ..alt };
        assert_eq!(plain("b", Some("B"), alt_shift), Some(b"\x1bB".to_vec()));
        let alt_ctrl = Modifiers {
            control: true,
            ..alt
        };
        assert_eq!(plain("c", None, alt_ctrl), Some(vec![0x1b, 0x03]));
        assert_eq!(plain("up", None, alt), Some(b"\x1b[1;3A".to_vec()));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn ghostty_macos_line_editing_keybinds() {
        let cmd = Modifiers {
            platform: true,
            ..mods()
        };
        let alt = Modifiers {
            alt: true,
            ..mods()
        };
        assert_eq!(
            macos_keybind("left", &cmd),
            Some(vec![0x01]),
            "the line's start"
        );
        assert_eq!(
            macos_keybind("right", &cmd),
            Some(vec![0x05]),
            "the line's end"
        );
        assert_eq!(macos_keybind("backspace", &cmd), Some(vec![0x15]));
        assert_eq!(
            macos_keybind("left", &alt),
            Some(b"\x1bb".to_vec()),
            "a word back"
        );
        assert_eq!(
            macos_keybind("right", &alt),
            Some(b"\x1bf".to_vec()),
            "a word forward"
        );
        assert_eq!(macos_keybind("j", &cmd), None);
    }

    /// Once the program pushes kitty's disambiguate flag, keys the legacy
    /// encoding has no room for get their own codes: Claude Code's
    /// Shift+Enter newline, Ctrl+Tab, Ctrl+Enter.
    #[test]
    fn kitty_keyboard_protocol_when_the_program_asks() {
        let kitty = b"\x1b[>1u";
        let shift = Modifiers {
            shift: true,
            ..mods()
        };
        let ctrl = Modifiers {
            control: true,
            ..mods()
        };
        assert_eq!(
            enc(kitty, "enter", None, shift),
            Some(b"\x1b[13;2u".to_vec())
        );
        assert_eq!(
            enc(kitty, "enter", None, ctrl),
            Some(b"\x1b[13;5u".to_vec())
        );
        assert_eq!(enc(kitty, "tab", None, ctrl), Some(b"\x1b[9;5u".to_vec()));
        assert_eq!(
            enc(kitty, "c", Some("c"), ctrl),
            Some(b"\x1b[99;5u".to_vec())
        );
        assert_eq!(
            enc(kitty, "escape", None, mods()),
            Some(b"\x1b[27u".to_vec())
        );
        assert_eq!(enc(kitty, "enter", None, mods()), Some(b"\r".to_vec()));
        // Without the protocol, Ghostty still sets Shift+Enter apart from
        // Enter, in xterm's CSI 27 form.
        assert_eq!(plain("enter", None, shift), Some(b"\x1b[27;2;13~".to_vec()));
    }
}
