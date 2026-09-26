//! Character classes the analysis and measurement passes care about.

use unicode_bidi::{BidiClass, bidi_class};

pub(crate) const SOFT_HYPHEN: char = '\u{AD}';
pub(crate) const OBJECT_REPLACEMENT: char = '\u{FFFC}';

/// Characters that end a line unconditionally (UAX #14 BK/CR/LF/NL) once CR and FF have been
/// normalized to `\n`.
#[inline]
pub(crate) fn is_hard_break(c: char) -> bool {
    matches!(
        c,
        '\n' | '\u{0B}' | '\u{0C}' | '\r' | '\u{85}' | '\u{2028}' | '\u{2029}'
    )
}

/// Default_Ignorable_Code_Point (DerivedCoreProperties). Shapers render these invisibly even when
/// the face has no glyph, so they must not force a fallback measurement.
#[inline]
pub(crate) fn is_default_ignorable(c: char) -> bool {
    let u = c as u32;
    matches!(
        u,
        0x00AD
            | 0x034F
            | 0x061C
            | 0x115F..=0x1160
            | 0x17B4..=0x17B5
            | 0x180B..=0x180F
            | 0x200B..=0x200F
            | 0x202A..=0x202E
            | 0x2060..=0x206F
            | 0x3164
            | 0xFE00..=0xFE0F
            | 0xFEFF
            | 0xFFA0
            | 0xFFF0..=0xFFF8
            | 0x1BCA0..=0x1BCA3
            | 0x1D173..=0x1D17A
            | 0xE0000..=0xE0FFF
    )
}

/// Characters that force emoji presentation / keycaps. A face may carry a text glyph for the base
/// (`↩`, `1`), but the platform renders these sequences with the emoji font, so the whole piece
/// has to go through the fallback measurer.
#[inline]
pub(crate) fn forces_emoji(c: char) -> bool {
    matches!(c, '\u{FE0F}' | '\u{20E3}')
}

/// Strong right-to-left (bidi class R or AL).
#[inline]
pub(crate) fn is_strong_rtl(c: char) -> bool {
    // Every R/AL code point lives in these blocks (Hebrew..NKo..Arabic Extended, RLM,
    // presentation forms, the SMP RTL blocks); skip the table search for everything else.
    matches!(
        c as u32,
        0x0590..=0x08FF | 0x200F | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFE | 0x10800..=0x10FFF
            | 0x1E800..=0x1EFFF
    ) && matches!(bidi_class(c), BidiClass::R | BidiClass::AL)
}
