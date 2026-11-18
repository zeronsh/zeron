//! What a store listing's name or description may not say (amendment 45):
//! that it carries the store's mark, or that the plugin is Zeron's own.
//!
//! One rule for the device's check and the edge, which reads the same data
//! file (`docs/plugin-store-data.json`: `mark_claims`, `claims_zeron`) and
//! the shared listing cases (`docs/plugin-package-vectors.json`):
//!
//! - The text is folded: Unicode NFKC, lowercased, NFKD without its
//!   nonspacing marks ([`mark_text`]), then every Default_Ignorable_Code_Point
//!   (variation selectors among them) and enclosing mark removed, so nothing
//!   drawn as nothing, or around a letter, splits a word.
//! - It claims the mark when it has one of [`MARK_GLYPHS`] anywhere, or one
//!   of [`MARK_WORDS`] as a whole word: no ASCII letter or digit right
//!   before or after it. A word's letters may be spelled with characters
//!   that pass for them ([`LOOKALIKES`], generated from Unicode's
//!   confusables, UTS #39, plus the Latin small capitals and the digit 3);
//!   `i` and `l` stand for each other ([`INTERCHANGEABLE`]); a letter may be
//!   two characters that pass for it together ([`PAIRS`]: `cl` for d); and
//!   single characters apart by up to [`MAX_SPACING`] spaces or punctuation
//!   marks read as one word ("V. E. R. I. F. I. E. D.").
//! - A name or description may not hold a character drawn as nothing
//!   ([`unseen`]).
//! - [`claims_zeron`]: after the same folding, one of [`ZERON_PHRASES`] as a
//!   whole phrase. Only Zeron's own publisher may say it: the edge refuses
//!   it unless the publisher carries the `maintainer` badge; a device, which
//!   can't know the publisher, warns.

use std::collections::BTreeSet;

use icu_properties::props::{BinaryProperty, DefaultIgnorableCodePoint, Emoji, VariationSelector, WhiteSpace};
use unicode_properties::{GeneralCategory, GeneralCategoryGroup, UnicodeGeneralCategory};

#[path = "mark_lookalikes.rs"]
mod generated;

pub use generated::{LOOKALIKES, SOURCE, SOURCE_SHA256, SOURCE_VERSION};

/// Words a listing's name or description may not use: only the store marks
/// a version verified.
pub const MARK_WORDS: &[&str] = &["verified", "official"];

/// Ticks a listing's name or description may not have, anywhere:
/// ✓ ✔ ☑ ✅ 🗸 🗹 √ 𐄂 🮱. (Of Unicode's other check marks and ballot boxes,
/// none reads as a tick: ⍻ is crossed through, ☐ ☒ 🗵 🗷 ⮽ 🗳 are empty
/// or crossed boxes, ⟤ ⟥ ⦍–⦐ squares and brackets with a short stroke.)
pub const MARK_GLYPHS: &[char] = &['\u{2713}', '\u{2714}', '\u{2611}', '\u{2705}', '\u{1F5F8}', '\u{1F5F9}', '\u{221A}', '\u{10102}', '\u{1FBB1}'];

/// Letters that stand for each other, their look-alikes too (a capital I and
/// a lowercase l are one stroke in sans-serif).
pub const INTERCHANGEABLE: &[(char, char)] = &[('i', 'l')];

/// A letter two characters can pass for together (each with its
/// look-alikes): UTS #39 has no such entries.
pub const PAIRS: &[(char, &str)] = &[('d', "cl")];

/// At most this many spaces or punctuation marks between single characters
/// read as one word.
pub const MAX_SPACING: usize = 3;

/// Phrases only Zeron's own plugins may use (`claims_zeron`), as folded:
/// separators (spaces, punctuation but the apostrophe) read as one space.
pub const ZERON_PHRASES: &[&str] = &[
    "by zeron",
    "from zeron",
    "zeron team",
    "zeron's own",
    "zeron approved",
    "zeron certified",
    "zeron endorsed",
    "zeron recommended",
    "zeron staff",
];

/// `text` as the rules read it before ignorables go: Unicode NFKC (so
/// 𝐯𝐞𝐫𝐢𝐟𝐢𝐞𝐝 and ｖｅｒｉｆｉｅｄ are "verified"), lowercased, then NFKD
/// without its nonspacing marks (so "vérified" and "VERİFİED", whose
/// lowercase keeps a dot above, are too).
pub fn mark_text(text: &str) -> String {
    let lower = icu_normalizer::ComposingNormalizerBorrowed::new_nfkc().normalize(text).to_lowercase();
    icu_normalizer::DecomposingNormalizerBorrowed::new_nfkd()
        .normalize(&lower)
        .chars()
        .filter(|c| c.general_category() != GeneralCategory::NonspacingMark)
        .collect()
}

/// [`mark_text`] without any character drawn as nothing, nor any enclosing
/// mark (General_Category Me: a keycap's, a circle's), which the fold
/// keeps: none splits a word.
fn folded(text: &str) -> Vec<char> {
    mark_text(text)
        .chars()
        .filter(|c| !DefaultIgnorableCodePoint::for_char(*c) && c.general_category() != GeneralCategory::EnclosingMark)
        .collect()
}

/// A space (White_Space, or U+FEFF) or a punctuation mark (General_Category
/// P*): what separates spaced letters.
fn separator(c: char) -> bool {
    WhiteSpace::for_char(c) || c == '\u{FEFF}' || c.general_category_group() == GeneralCategoryGroup::Punctuation
}

fn ascii_word(c: Option<&char>) -> bool {
    c.is_some_and(|c| c.is_ascii_alphanumeric())
}

/// Whether `text` holds a character a name or description may not: a
/// control or format character (Cc, Cf), any other
/// Default_Ignorable_Code_Point (the Hangul fillers, which are letters), a
/// variation selector but U+FE0F, or a U+FE0F not directly after a
/// character with the Emoji property ("⭐️" is fine).
pub fn unseen(text: &str) -> bool {
    let mut before: Option<char> = None;
    for c in text.chars() {
        let selector = VariationSelector::for_char(c);
        let bad = matches!(c.general_category(), GeneralCategory::Control | GeneralCategory::Format)
            || (DefaultIgnorableCodePoint::for_char(c) && !selector)
            || (selector && c != '\u{FE0F}')
            || (c == '\u{FE0F}' && !before.is_some_and(Emoji::for_char));
        if bad {
            return true;
        }
        before = Some(c);
    }
    false
}

/// The characters that pass for `letter` (itself among them).
fn lookalikes_of(letter: char) -> BTreeSet<char> {
    let mut chars: BTreeSet<char> = LOOKALIKES.iter().filter(|(l, _)| *l == letter).flat_map(|(_, s)| s.chars()).collect();
    chars.insert(letter);
    chars
}

/// What may stand for `letter` in a word: its look-alikes, and those of the
/// letter it's interchangeable with.
fn class_of(letter: char) -> BTreeSet<char> {
    let mut chars = lookalikes_of(letter);
    for (a, b) in INTERCHANGEABLE {
        let other = if *a == letter { Some(*b) } else if *b == letter { Some(*a) } else { None };
        if let Some(other) = other {
            chars.extend(lookalikes_of(other));
        }
    }
    chars
}

/// Whether `word`'s letters, each a character of its class or a pair, match
/// `text` from `at` to an end with no ASCII letter or digit right after.
fn spells(text: &[char], at: usize, word: &[char]) -> bool {
    let Some((&letter, rest)) = word.split_first() else {
        return !ascii_word(text.get(at));
    };
    if text.get(at).is_some_and(|c| class_of(letter).contains(c)) && spells(text, at + 1, rest) {
        return true;
    }
    PAIRS.iter().filter(|(l, _)| *l == letter).any(|(_, pair)| {
        let pair: Vec<char> = pair.chars().collect();
        pair.iter().enumerate().all(|(n, p)| text.get(at + n).is_some_and(|c| lookalikes_of(*p).contains(c))) && spells(text, at + pair.len(), rest)
    })
}

/// Whether `text` spells `word` with look-alikes, as a whole word.
fn spelled(text: &[char], word: &str) -> bool {
    let word: Vec<char> = word.chars().collect();
    (0..text.len()).any(|at| !ascii_word(at.checked_sub(1).and_then(|b| text.get(b))) && spells(text, at, &word))
}

/// `text` with each run of single characters, apart by 1 to
/// [`MAX_SPACING`] separators, read as one word: "V E R I F I E D",
/// "V.E.R.I.F.I.E.D.", "V. E. R. I. F. I. E. D." (a character is single when
/// a separator or the text's edge is on each side of it).
fn unspaced(text: &[char]) -> Vec<char> {
    let mut out = Vec::with_capacity(text.len());
    let mut at = 0;
    while at < text.len() {
        let starts = !separator(text[at]) && (at == 0 || separator(text[at - 1]));
        if starts {
            let mut run = vec![text[at]];
            let mut last = at;
            loop {
                let gap = text[last + 1..].iter().take_while(|c| separator(**c)).count();
                let next = last + 1 + gap;
                // The next single character: after 1 to 3 separators, with
                // a separator or the end right after it.
                if !(1..=MAX_SPACING).contains(&gap) || next >= text.len() || text.get(next + 1).is_some_and(|c| !separator(*c)) {
                    break;
                }
                run.push(text[next]);
                last = next;
            }
            if run.len() > 1 {
                out.extend(run);
                at = last + 1;
                continue;
            }
        }
        out.push(text[at]);
        at += 1;
    }
    out
}

/// What in `text` claims the store's mark, if anything: one of
/// [`MARK_GLYPHS`], or the [`MARK_WORDS`] word it spells.
pub fn mark_claim(text: &str) -> Option<String> {
    let folded = folded(text);
    if let Some(glyph) = folded.iter().find(|c| MARK_GLYPHS.contains(c)) {
        return Some(glyph.to_string());
    }
    // The words as written, first; then spelled with look-alikes, in the
    // text and with its spaced letters read as words.
    let plain: String = folded.iter().collect();
    let written = MARK_WORDS.iter().find(|word| {
        plain.match_indices(**word).any(|(at, _)| !ascii_word(plain[..at].chars().next_back().as_ref()) && !ascii_word(plain[at + word.len()..].chars().next().as_ref()))
    });
    if let Some(word) = written {
        return Some((*word).to_owned());
    }
    let readings = [folded.clone(), unspaced(&folded)];
    MARK_WORDS.iter().find(|word| readings.iter().any(|reading| spelled(reading, word))).map(|word| (*word).to_owned())
}

/// Which of [`ZERON_PHRASES`] `text` says, if any: folded as
/// [`mark_claim`] folds, each run of separators but the apostrophe read as
/// one space (’ and ʼ as '), the phrase whole.
pub fn claims_zeron(text: &str) -> Option<String> {
    let mut plain = String::new();
    for c in folded(text) {
        let c = if matches!(c, '\u{2019}' | '\u{02BC}') { '\'' } else { c };
        if c != '\'' && separator(c) {
            if !plain.ends_with(' ') {
                plain.push(' ');
            }
        } else {
            plain.push(c);
        }
    }
    ZERON_PHRASES
        .iter()
        .find(|phrase| {
            plain.match_indices(**phrase).any(|(at, _)| {
                !ascii_word(plain[..at].chars().next_back().as_ref()) && !ascii_word(plain[at + phrase.len()..].chars().next().as_ref())
            })
        })
        .map(|phrase| (*phrase).to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mark_s_words_whole_and_its_glyphs() {
        for (text, claim) in [
            ("Verified Focus", Some("verified")),
            ("The OFFICIAL git plugin", Some("official")),
            ("verified_tools", Some("verified")),
            ("Focus \u{2713}", Some("\u{2713}")),
            ("Focus \u{221A}", Some("\u{221A}")),
            ("Focus \u{10102}", Some("\u{10102}")),
            ("Focus \u{1fbb1}", Some("\u{1fbb1}")),
            ("Focus \u{237b}", None),
            ("Focus \u{2612}", None),
            ("\u{1d42f}\u{1d41e}\u{1d42b}\u{1d422}\u{1d41f}\u{1d422}\u{1d41e}\u{1d41d} Focus", Some("verified")),
            ("V\u{e9}rified", Some("verified")),
            ("VER\u{130}F\u{130}ED", Some("verified")),
            ("An unofficial client", None),
            ("Unverified links, flagged", None),
            ("Officially fun", None),
            ("Focus Board", None),
        ] {
            assert_eq!(mark_claim(text).as_deref(), claim, "{text}");
        }
    }

    #[test]
    fn look_alikes_spacing_and_ignorables_spell_it_too() {
        for text in [
            "V\u{435}r\u{456}f\u{456}\u{435}d Focus",
            "\u{3bd}erified",
            "\u{1d20}\u{1d07}\u{280}\u{26a}\u{a730}\u{26a}\u{1d07}\u{1d05} Focus",
            "\u{a4e6}\u{a4f0}\u{a4e3}\u{a4f2}\u{a4dd}\u{a4f2}\u{a4f0}\u{a4d3} Focus",
            "VER1F1ED",
            "0FFICIAL tools",
            "v3rified",
            "officia1",
            "VERlFlED Focus",
            "OFFlClAL Git",
            "The officiaI client",
            "verifiecl tools",
            "V E R I F I E D",
            "V.E.R.I.F.I.E.D. Focus",
            "V. E. R. I. F. I. E. D. Focus",
            "V\u{b7}E\u{b7}R\u{b7}I\u{b7}F\u{b7}I\u{b7}E\u{b7}D",
            "V\u{2013}E\u{2013}R\u{2013}I\u{2013}F\u{2013}I\u{2013}E\u{2013}D",
            "V  E  R  I  F  I  E  D",
            "V\u{a0}E\u{a0}R\u{a0}I\u{a0}F\u{a0}I\u{a0}E\u{a0}D",
            "V\u{3000}E\u{3000}R\u{3000}I\u{3000}F\u{3000}I\u{3000}E\u{3000}D",
            "V E R \u{406} F \u{406} E D",
            "Veri\u{115f}fied Focus",
            "Veri\u{3164}fied Focus",
            "Veri\u{fe0f}fied Focus",
            "Veri\u{e0100}fied Focus",
            "Veri\u{200b}fied",
            "Veri\u{20e3}fied",
            "Veri\u{20dd}fied",
            "\u{1f165}\u{1f154}\u{1f161}\u{1f158}\u{1f155}\u{1f158}\u{1f154}\u{1f153} Focus",
            "\u{1f185}\u{1f174}\u{1f181}\u{1f178}\u{1f175}\u{1f178}\u{1f174}\u{1f173} Focus",
            "\u{1f1fb} \u{1f1ea} \u{1f1f7} \u{1f1ee} \u{1f1eb} \u{1f1ee} \u{1f1ea} \u{1f1e9}",
        ] {
            assert!(mark_claim(text).is_some(), "{text}");
        }
        for text in [
            "Unv\u{435}rified Links",
            "For every \u{3bf}fficer",
            "Off\u{456}cially the best",
            "\u{41e}\u{444}\u{438}\u{446}\u{438}\u{430}\u{43b}\u{44c}\u{43d}\u{44b}\u{439} \u{43a}\u{43b}\u{438}\u{435}\u{43d}\u{442}",
            "verify your builds",
            "verification",
            "officialdom",
            "Top 10 tools",
            "v1.0 verifier",
            "100% local",
            "Verifiedly 3D",
            "A B testing",
            "U.S. weather",
            "V I M keys",
            "J. R. R. Tolkien notes",
            "R\u{b7}E\u{b7}M songs",
            "Oil fields",
            "Lift",
            "Civil filed",
            "Stars \u{2b50}\u{fe0f}",
        ] {
            assert_eq!(mark_claim(text), None, "{text}");
        }
    }

    #[test]
    fn only_an_emoji_s_own_selector_is_seen() {
        for ok in ["Stars \u{2b50}\u{fe0f}", "Hearts \u{2764}\u{fe0f}", "Keys 1\u{fe0f}\u{20e3}", "Top 1\u{fe0f}\u{20e3} timer", "Focus Board"] {
            assert!(!unseen(ok), "{ok}");
        }
        for bad in ["Focus\u{fe0f}", "Fo\u{fe0f}cus", "Stars \u{2b50}\u{fe0f}\u{fe0f}", "Stars \u{2b50}\u{fe0e}", "Focus\u{fe00}", "Focus\u{e0100}", "Focus\u{3164}Board", "Focus\u{115f}", "a\u{202e}b", "\u{fe0f}Focus", "\u{1f469}\u{200d}\u{1f4bb} Dev", "\u{1f3f4}\u{e0067}\u{e0062}\u{e0073}\u{e0063}\u{e0074}\u{e007f} Focus"] {
            assert!(unseen(bad), "{bad:?}");
        }
    }

    #[test]
    fn saying_it_s_zeron_s_own() {
        for (text, claim) in [
            ("Notes, by Zeron", Some("by zeron")),
            ("From ZERON: the board", Some("from zeron")),
            ("Made by the Zeron-team", Some("zeron team")),
            ("Zeron\u{2019}s own notes", Some("zeron's own")),
            ("A Zeron approved board", Some("zeron approved")),
            ("zeron\u{2014}recommended", Some("zeron recommended")),
            ("Notes for Zeron", None),
            ("Works with Zeron", None),
            ("Zeronaut tools", None),
            ("by zeronaut", None),
        ] {
            assert_eq!(claims_zeron(text).as_deref(), claim, "{text}");
        }
    }

    /// `UTS39_CONFUSABLES=<confusables.txt> cargo test -p zeron-engine --lib
    /// plugins::mark::tests::lookalikes_are_current` rewrites
    /// `mark_lookalikes.rs` from it; without, nothing is checked.
    #[test]
    fn lookalikes_are_current() {
        let Ok(path) = std::env::var("UTS39_CONFUSABLES") else { return };
        let bytes = std::fs::read(&path).expect("confusables.txt");
        let source = generate(&String::from_utf8(bytes.clone()).unwrap(), &super::super::signing::sha256_hex(&bytes));
        std::fs::write(concat!(env!("CARGO_MANIFEST_DIR"), "/src/plugins/mark_lookalikes.rs"), source).unwrap();
    }

    /// For each letter of the mark's words, every character that passes for
    /// it, folded as [`mark_text`] folds and as written (kept unless it is,
    /// or folds to, an ASCII letter, which the words match already): from
    /// confusables.txt, every single-character source whose prototype folds
    /// to that letter, then the Latin small capitals and the digit 3 for e.
    fn generate(text: &str, sha256: &str) -> String {
        let version = text.lines().find_map(|l| l.strip_prefix("# Version: ")).unwrap_or("unknown").trim().to_owned();
        let letters: BTreeSet<char> = MARK_WORDS.iter().flat_map(|w| w.chars()).collect();
        let mut table: std::collections::BTreeMap<char, BTreeSet<char>> = letters.iter().map(|l| (*l, BTreeSet::new())).collect();
        // Each kept as folded here and as written, so a runtime whose case
        // data is older or newer than this one's (a new letter it doesn't
        // lowercase) still finds it.
        let mut add = |letter: char, ch: &str| {
            let folded: Vec<char> = mark_text(ch).chars().collect();
            let written: Vec<char> = ch.chars().collect();
            let Some(set) = table.get_mut(&letter) else { return };
            // (Folding to an ASCII letter: the words as written match it.)
            if folded.iter().any(char::is_ascii_alphabetic) {
                return;
            }
            for candidate in [folded, written] {
                if let [one] = candidate[..]
                    && !one.is_ascii_alphabetic()
                    && one != letter
                {
                    set.insert(one);
                }
            }
        };
        let from_hex = |field: &str| field.split_whitespace().filter_map(|h| u32::from_str_radix(h, 16).ok().and_then(char::from_u32)).collect::<String>();
        for line in text.lines().filter(|l| !l.starts_with('#')) {
            let mut fields = line.split(';');
            let (Some(source), Some(target)) = (fields.next(), fields.next()) else { continue };
            let target: Vec<char> = mark_text(&from_hex(target)).chars().collect();
            if let [one] = target[..]
                && letters.contains(&one)
            {
                add(one, &from_hex(source));
            }
        }
        // The Latin letter small capitals, which UTS #39 doesn't list as
        // passing for the lowercase letters they look like.
        for (letter, cp) in [
            ('a', 0x1d00), ('c', 0x1d04), ('d', 0x1d05), ('e', 0x1d07), ('f', 0xa730), ('i', 0x026a), ('l', 0x029f), ('o', 0x1d0f), ('r', 0x0280), ('v', 0x1d20),
        ] {
            add(letter, &char::from_u32(cp).unwrap().to_string());
        }
        // The boxed emoji letters NFKC leaves alone, each its letter in a
        // box: NEGATIVE CIRCLED and NEGATIVE SQUARED LATIN CAPITAL LETTER
        // A…Z, and the REGIONAL INDICATOR SYMBOL LETTERs.
        for first in [0x1f150, 0x1f170, 0x1f1e6] {
            for (n, letter) in ('a'..='z').enumerate() {
                add(letter, &char::from_u32(first + n as u32).unwrap().to_string());
            }
        }
        add('e', "3");
        let escape = |c: char| if c.is_ascii_alphanumeric() { c.to_string() } else { format!("\\u{{{:x}}}", c as u32) };
        let rows: Vec<String> = table.iter().map(|(l, set)| format!("    ('{l}', \"{}\"),", set.iter().map(|c| escape(*c)).collect::<String>())).collect();
        format!(
            "//! Generated by `mark.rs` (`lookalikes_are_current`) from Unicode's\n//! confusables.txt: do not edit. For each letter of the mark's words, the\n//! characters that pass for it, folded as `mark_text` folds them.\n\npub const SOURCE: &str = \"UTS #39 confusables.txt\";\npub const SOURCE_VERSION: &str = \"{version}\";\npub const SOURCE_SHA256: &str = \"{sha256}\";\n\npub const LOOKALIKES: &[(char, &str)] = &[\n{}\n];\n",
            rows.join("\n")
        )
    }
}
