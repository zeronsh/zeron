//! Math alphanumeric styles: `\mathbf`, `\mathbb`, `\mathcal`, … are plain
//! Unicode code points (U+1D400 block plus the older Letterlike holes), which
//! every OpenType math font covers to some degree.

/// A math alphabet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Variant {
    /// TeX's default: Latin and lowercase Greek italic, the rest upright.
    #[default]
    Normal,
    Upright,
    Bold,
    Italic,
    BoldItalic,
    Script,
    Fraktur,
    DoubleStruck,
    Sans,
    SansBold,
    Mono,
}

/// Starts of the Latin capital / small runs, per alphabet.
fn latin(v: Variant) -> Option<(u32, u32)> {
    Some(match v {
        Variant::Bold => (0x1D400, 0x1D41A),
        Variant::Italic | Variant::Normal => (0x1D434, 0x1D44E),
        Variant::BoldItalic => (0x1D468, 0x1D482),
        Variant::Script => (0x1D49C, 0x1D4B6),
        Variant::Fraktur => (0x1D504, 0x1D51E),
        Variant::DoubleStruck => (0x1D538, 0x1D552),
        Variant::Sans => (0x1D5A0, 0x1D5BA),
        Variant::SansBold => (0x1D5D4, 0x1D5EE),
        Variant::Mono => (0x1D670, 0x1D68A),
        Variant::Upright => return None,
    })
}

/// Letters that predate the math block and live in Letterlike Symbols.
fn hole(ch: char, v: Variant) -> Option<char> {
    Some(match (v, ch) {
        (Variant::Italic | Variant::Normal, 'h') => 'ℎ',
        (Variant::Script, 'B') => 'ℬ',
        (Variant::Script, 'E') => 'ℰ',
        (Variant::Script, 'F') => 'ℱ',
        (Variant::Script, 'H') => 'ℋ',
        (Variant::Script, 'I') => 'ℐ',
        (Variant::Script, 'L') => 'ℒ',
        (Variant::Script, 'M') => 'ℳ',
        (Variant::Script, 'R') => 'ℛ',
        (Variant::Script, 'e') => 'ℯ',
        (Variant::Script, 'g') => 'ℊ',
        (Variant::Script, 'o') => 'ℴ',
        (Variant::Fraktur, 'C') => 'ℭ',
        (Variant::Fraktur, 'H') => 'ℌ',
        (Variant::Fraktur, 'I') => 'ℑ',
        (Variant::Fraktur, 'R') => 'ℜ',
        (Variant::Fraktur, 'Z') => 'ℨ',
        (Variant::DoubleStruck, 'C') => 'ℂ',
        (Variant::DoubleStruck, 'H') => 'ℍ',
        (Variant::DoubleStruck, 'N') => 'ℕ',
        (Variant::DoubleStruck, 'P') => 'ℙ',
        (Variant::DoubleStruck, 'Q') => 'ℚ',
        (Variant::DoubleStruck, 'R') => 'ℝ',
        (Variant::DoubleStruck, 'Z') => 'ℤ',
        _ => return None,
    })
}

fn digits(v: Variant) -> Option<u32> {
    Some(match v {
        Variant::Bold | Variant::BoldItalic => 0x1D7CE,
        Variant::DoubleStruck => 0x1D7D8,
        Variant::Sans => 0x1D7E2,
        Variant::SansBold => 0x1D7EC,
        Variant::Mono => 0x1D7F6,
        _ => return None,
    })
}

/// Start of a 58-slot Greek run: 25 capitals (with ϴ in the U+03A2 hole),
/// ∇, then 25 smalls (with ς), ∂ ϵ ϑ ϰ ϕ ϱ ϖ.
fn greek(v: Variant) -> Option<u32> {
    Some(match v {
        Variant::Bold => 0x1D6A8,
        Variant::Italic => 0x1D6E2,
        Variant::BoldItalic => 0x1D71C,
        Variant::SansBold => 0x1D756,
        _ => return None,
    })
}

fn greek_slot(ch: char) -> Option<(bool, u32)> {
    let c = ch as u32;
    Some(match ch {
        'Α'..='Ω' => (true, c - 0x391),
        'ϴ' => (true, 17),
        '∇' => (true, 25),
        'α'..='ω' => (false, c - 0x3B1),
        '∂' => (false, 25),
        'ϵ' => (false, 26),
        'ϑ' => (false, 27),
        'ϰ' => (false, 28),
        'ϕ' => (false, 29),
        'ϱ' => (false, 30),
        'ϖ' => (false, 31),
        _ => return None,
    })
}

/// `ch` in alphabet `v`, or `ch` itself when the alphabet has no such letter.
pub(crate) fn styled(ch: char, v: Variant) -> char {
    let mapped = match ch {
        'A'..='Z' | 'a'..='z' => hole(ch, v).or_else(|| {
            let (cap, small) = latin(v)?;
            let c = if ch.is_ascii_uppercase() {
                cap + (ch as u32 - 'A' as u32)
            } else {
                small + (ch as u32 - 'a' as u32)
            };
            char::from_u32(c)
        }),
        '0'..='9' => digits(v).and_then(|d| char::from_u32(d + (ch as u32 - '0' as u32))),
        'ı' if matches!(v, Variant::Normal | Variant::Italic) => Some('𝚤'),
        'ȷ' if matches!(v, Variant::Normal | Variant::Italic) => Some('𝚥'),
        _ => greek_slot(ch).and_then(|(upper, slot)| {
            // TeX convention: lowercase Greek is italic by default, capitals upright.
            let v = match v {
                Variant::Normal if !upper && ch != '∂' => Variant::Italic,
                v => v,
            };
            let base = greek(v)?;
            char::from_u32(base + if upper { slot } else { 26 + slot })
        }),
    };
    mapped.unwrap_or(ch)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alphabets() {
        assert_eq!(styled('x', Variant::Normal), '𝑥');
        assert_eq!(styled('h', Variant::Normal), 'ℎ');
        assert_eq!(styled('R', Variant::DoubleStruck), 'ℝ');
        assert_eq!(styled('A', Variant::DoubleStruck), '𝔸');
        assert_eq!(styled('L', Variant::Script), 'ℒ');
        assert_eq!(styled('x', Variant::Bold), '𝐱');
        assert_eq!(styled('2', Variant::Normal), '2');
        assert_eq!(styled('2', Variant::Bold), '𝟐');
        assert_eq!(styled('α', Variant::Normal), '𝛼');
        assert_eq!(styled('ω', Variant::Normal), '𝜔');
        assert_eq!(styled('ϕ', Variant::Normal), '𝜙');
        assert_eq!(styled('Γ', Variant::Normal), 'Γ');
        assert_eq!(styled('Ω', Variant::Italic), '𝛺');
        assert_eq!(styled('∇', Variant::Bold), '𝛁');
        assert_eq!(styled('x', Variant::Upright), 'x');
        assert_eq!(styled('ı', Variant::Normal), '𝚤');
    }
}
