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
//! - The mark's words in other languages ([`OTHER_LANGUAGES`]): whole words
//!   in scripts with spaces, substrings in Chinese, Japanese and Korean.
//! - Claims of being Zeron's (`claims_zeron`), "zeron" read as the mark's
//!   words are: in a name, any "zeron" not the object of one of
//!   [`NAME_FORMS`] ([`claims_zeron_in_name`]); in a description, "zeron"
//!   after a word of origin or beside a word of standing ([`claims_zeron`]).
//!   Only Zeron's own publisher may: the edge refuses it unless the publisher
//!   carries the `maintainer` badge; a device, which can't know the
//!   publisher, warns.

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

/// The mark's words in the store's other languages: `(language, substring,
/// words)`, each folded as the text is; whole words (no letter or digit of
/// any script right before or after) where words are spaced, substrings in
/// Chinese, Japanese and Korean. "No oficial" and "非官方" are refused as
/// "not official" is: the author rewords.
pub const OTHER_LANGUAGES: &[(&str, bool, &[&str])] = &[
    ("es", false, &["verificado", "verificada", "verificados", "verificadas", "oficial", "oficiales"]),
    ("fr", false, &["v\u{e9}rifi\u{e9}", "v\u{e9}rifi\u{e9}e", "v\u{e9}rifi\u{e9}s", "v\u{e9}rifi\u{e9}es", "officiel", "officielle", "officiels", "officielles"]),
    ("de", false, &[
        "verifiziert", "verifizierte", "verifizierter", "verifiziertes", "verifizierten",
        "gepr\u{fc}ft", "gepr\u{fc}fte", "gepr\u{fc}fter", "gepr\u{fc}ftes", "gepr\u{fc}ften",
        "offiziell", "offizielle", "offizieller", "offizielles", "offiziellen",
    ]),
    ("it", false, &["verificato", "verificata", "verificati", "verificate", "ufficiale", "ufficiali"]),
    ("pt", false, &["verificado", "verificada", "verificados", "verificadas", "oficial", "oficiais"]),
    ("ru", false, &[
        "\u{43f}\u{440}\u{43e}\u{432}\u{435}\u{440}\u{435}\u{43d}\u{43e}",
        "\u{43f}\u{440}\u{43e}\u{432}\u{435}\u{440}\u{435}\u{43d}\u{43d}\u{44b}\u{439}",
        "\u{43f}\u{440}\u{43e}\u{432}\u{435}\u{440}\u{435}\u{43d}\u{43d}\u{430}\u{44f}",
        "\u{43f}\u{440}\u{43e}\u{432}\u{435}\u{440}\u{435}\u{43d}\u{43d}\u{43e}\u{435}",
        "\u{43f}\u{440}\u{43e}\u{432}\u{435}\u{440}\u{435}\u{43d}\u{43d}\u{44b}\u{435}",
        "\u{432}\u{435}\u{440}\u{438}\u{444}\u{438}\u{446}\u{438}\u{440}\u{43e}\u{432}\u{430}\u{43d}",
        "\u{432}\u{435}\u{440}\u{438}\u{444}\u{438}\u{446}\u{438}\u{440}\u{43e}\u{432}\u{430}\u{43d}\u{43d}\u{44b}\u{439}",
        "\u{43e}\u{444}\u{438}\u{446}\u{438}\u{430}\u{43b}\u{44c}\u{43d}\u{44b}\u{439}",
        "\u{43e}\u{444}\u{438}\u{446}\u{438}\u{430}\u{43b}\u{44c}\u{43d}\u{430}\u{44f}",
        "\u{43e}\u{444}\u{438}\u{446}\u{438}\u{430}\u{43b}\u{44c}\u{43d}\u{43e}\u{435}",
        "\u{43e}\u{444}\u{438}\u{446}\u{438}\u{430}\u{43b}\u{44c}\u{43d}\u{44b}\u{435}",
        "\u{43e}\u{444}\u{438}\u{446}\u{438}\u{430}\u{43b}\u{44c}\u{43d}\u{43e}",
    ]),
    ("zh", true, &["\u{5b98}\u{65b9}", "\u{5df2}\u{9a8c}\u{8bc1}", "\u{5df2}\u{9a57}\u{8b49}", "\u{5df2}\u{8ba4}\u{8bc1}", "\u{5df2}\u{8a8d}\u{8b49}"]),
    ("ja", true, &["\u{516c}\u{5f0f}", "\u{8a8d}\u{8a3c}\u{6e08}", "\u{691c}\u{8a3c}\u{6e08}", "\u{30aa}\u{30d5}\u{30a3}\u{30b7}\u{30e3}\u{30eb}"]),
    ("ko", true, &["\u{acf5}\u{c2dd}", "\u{c778}\u{c99d}\u{b428}", "\u{c778}\u{c99d}\u{b41c}", "\u{ac80}\u{c99d}\u{b428}", "\u{ac80}\u{c99d}\u{b41c}"]),
];

/// The brand: a listing may say it only about Zeron, never as Zeron, unless
/// its publisher is Zeron's own.
pub const ZERON: &str = "zeron";

/// In a name, "zeron" only as the object of one of these, by the store's
/// locale: `(language, forms)`. Anything else ("Zeron Labs", "Team Zeron",
/// "Zeron Git", "Zeron's git", "Zeron-made") reads as ours.
pub const NAME_FORMS: &[(&str, &[&str])] = &[
    ("en", &["for", "in", "with", "on", "to"]),
    // (As the app's zh-Hans strings say them: 为 Zeron, 在 Zeron, 使用 Zeron;
    // with 适用于/用于 "made for", 给 "for", 与/和 "with", and their
    // traditional forms. Never 由 "by".)
    ("zh", &[
        "\u{4e3a}", "\u{70ba}", "\u{5728}", "\u{4f7f}\u{7528}", "\u{9002}\u{7528}\u{4e8e}", "\u{9069}\u{7528}\u{65bc}", "\u{7528}\u{4e8e}", "\u{7528}\u{65bc}", "\u{7ed9}", "\u{7d66}", "\u{4e0e}", "\u{8207}", "\u{548c}",
    ]),
];

/// In a description, "zeron" right after one of these reads as its maker.
pub const ORIGIN_WORDS: &[&str] = &["by", "from", "by the", "made by", "built by", "von", "par", "de", "da", "di", "del", "van"];

/// In a description, "zeron" right before or after one of these (or its
/// possessive, "zeron's team") reads as its standing.
pub const STANDING_WORDS: &[&str] = &[
    "team", "labs", "inc", "hq", "staff", "approved", "certified", "endorsed", "recommended", "made", "built", "own", "\u{5b98}\u{65b9}", "\u{516c}\u{5f0f}", "\u{acf5}\u{c2dd}",
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

/// An apostrophe as a phrase may be written: ' ’ ʼ.
fn apostrophe(c: char) -> bool {
    matches!(c, '\'' | '\u{2019}' | '\u{02BC}')
}

/// Where `spelling` (folded) ends when it matches `text` from `at`, with no
/// ASCII letter or digit right after: each letter a character of its class
/// or a pair; an apostrophe any of [`apostrophe`]'s; a space a run of one or
/// more separators but an apostrophe; a `~` a run of none or more (between
/// words of scripts that don't space them).
fn spell_end(text: &[char], at: usize, spelling: &[char]) -> Option<usize> {
    let Some((&letter, rest)) = spelling.split_first() else {
        return (!ascii_word(text.get(at))).then_some(at);
    };
    let gap = |from: usize| (from..text.len()).take_while(move |n| separator(text[*n]) && !apostrophe(text[*n])).map(|n| n + 1);
    match letter {
        ' ' => gap(at).find_map(|next| spell_end(text, next, rest)),
        '~' => std::iter::once(at).chain(gap(at)).find_map(|next| spell_end(text, next, rest)),
        '\'' => text.get(at).filter(|c| apostrophe(**c)).and_then(|_| spell_end(text, at + 1, rest)),
        _ => {
            if text.get(at).is_some_and(|c| class_of(letter).contains(c))
                && let Some(end) = spell_end(text, at + 1, rest)
            {
                return Some(end);
            }
            PAIRS.iter().filter(|(l, _)| *l == letter).find_map(|(_, pair)| {
                let pair: Vec<char> = pair.chars().collect();
                let fits = pair.iter().enumerate().all(|(n, p)| text.get(at + n).is_some_and(|c| lookalikes_of(*p).contains(c)));
                if fits { spell_end(text, at + pair.len(), rest) } else { None }
            })
        }
    }
}

/// Where each whole match of `words` (a word, or a phrase) in `text` ends.
fn ends(text: &[char], words: &str) -> BTreeSet<usize> {
    let spelling: Vec<char> = mark_text(words).chars().collect();
    (0..text.len())
        .filter(|at| !ascii_word(at.checked_sub(1).and_then(|b| text.get(b))))
        .filter_map(|at| spell_end(text, at, &spelling))
        .collect()
}

/// Whether `text` spells `words` with look-alikes, whole.
fn spelled(text: &[char], words: &str) -> bool {
    !ends(text, words).is_empty()
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
    if let Some(word) = MARK_WORDS.iter().find(|word| readings.iter().any(|reading| spelled(reading, word))) {
        return Some((*word).to_owned());
    }
    // In the store's other languages.
    let any_word = |c: Option<&char>| c.is_some_and(|c| matches!(c.general_category_group(), GeneralCategoryGroup::Letter | GeneralCategoryGroup::Number));
    OTHER_LANGUAGES.iter().flat_map(|(_, substring, words)| words.iter().map(move |w| (*substring, *w))).find_map(|(substring, word)| {
        let wanted: Vec<char> = folded_word(word);
        let found = readings.iter().any(|reading| {
            (0..reading.len()).any(|at| {
                reading[at..].starts_with(&wanted)
                    && (substring || (!any_word(at.checked_sub(1).and_then(|b| reading.get(b))) && !any_word(reading.get(at + wanted.len()))))
            })
        });
        found.then(|| word.to_owned())
    })
}

/// A word of a list, folded as the text is.
fn folded_word(word: &str) -> Vec<char> {
    folded(word)
}

/// `a` then `b`, as a spelling: apart by a space, or by `~` (none or more
/// separators) when either is in a script that doesn't space its words.
fn joined(a: &str, b: &str) -> String {
    if a.is_ascii() && b.is_ascii() { format!("{a} {b}") } else { format!("{a}~{b}") }
}

/// The spellings that make a name's "zeron" the object of a form.
fn name_forms() -> Vec<String> {
    NAME_FORMS.iter().flat_map(|(_, forms)| forms.iter().map(|form| joined(form, ZERON))).collect()
}

/// The spellings a description may not use: "zeron" after a word of origin,
/// or beside a word of standing.
pub fn zeron_phrases() -> Vec<String> {
    let mut phrases: Vec<String> = ORIGIN_WORDS.iter().map(|o| joined(o, ZERON)).collect();
    for standing in STANDING_WORDS {
        phrases.push(joined(ZERON, standing));
        phrases.push(joined(standing, ZERON));
        if standing.is_ascii() {
            phrases.push(format!("{ZERON}'s {standing}"));
        }
    }
    phrases
}

/// Whether a name says "zeron" other than as the object of one of
/// [`NAME_FORMS`] ("Themes for Zeron" may; "Zeron Labs", "Git by Zeron"
/// may not): `Some("zeron")` if it does. Read as the mark's words are.
pub fn claims_zeron_in_name(text: &str) -> Option<String> {
    let folded = folded(text);
    let forms = name_forms();
    [folded.clone(), unspaced(&folded)]
        .iter()
        .any(|reading| {
            let allowed: BTreeSet<usize> = forms.iter().flat_map(|form| ends(reading, form)).collect();
            ends(reading, ZERON).iter().any(|end| !allowed.contains(end))
        })
        .then(|| ZERON.to_owned())
}

/// Which of [`zeron_phrases`] a description says, if any, read as the
/// mark's words are: each letter itself or a look-alike, its apostrophe any
/// of ' ’ ʼ, each space any run of separators but an apostrophe, whole, in
/// the text or with its spaced letters read as one word ("Git by Zer0n",
/// "Z.E.R.O.N. approved"). Prose about Zeron ("adds a panel to Zeron's
/// sidebar") says none.
pub fn claims_zeron(text: &str) -> Option<String> {
    let folded = folded(text);
    let readings = [folded.clone(), unspaced(&folded)];
    zeron_phrases().into_iter().find(|phrase| readings.iter().any(|reading| spelled(reading, phrase))).map(|phrase| phrase.replace('~', " "))
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
    fn a_name_says_zeron_only_as_what_it_s_for() {
        for name in [
            "Zeron Labs", "Team Zeron", "Zeron Git", "Zeron's git", "Zeron-made", "Git by Zeron", "Git by Zer0n", "Git by Z\u{435}ron",
            "Git by \u{1d22}\u{1d07}\u{280}\u{1d0f}\u{274}", "Z.E.R.O.N. approved", "z e r o n approved", "Zeron notes", "Zeron\u{63d2}\u{4ef6}",
            "Themes for Zeron, by Zeron",
            "\u{7531}Zeron\u{5236}\u{4f5c}\u{7684}\u{7b14}\u{8bb0}",
        ] {
            assert_eq!(claims_zeron_in_name(name).as_deref(), Some("zeron"), "{name}");
        }
        for name in [
            "Themes for Zeron", "Git in Zeron", "Notes with Zeron", "Tabs on Zeron", "Send to Zeron", "Zeronaut", "Lazeron", "A board by Zenon",
            "\u{9002}\u{7528}\u{4e8e}Zeron\u{7684}\u{4e3b}\u{9898}", "\u{4e3a} Zeron \u{505a}\u{7684}\u{7b14}\u{8bb0}", "\u{4f7f}\u{7528} Zeron \u{7684}\u{6280}\u{5de7}", "Focus Board", "Heron",
        ] {
            assert_eq!(claims_zeron_in_name(name), None, "{name}");
        }
    }

    #[test]
    fn a_description_may_say_what_zeron_is_but_not_that_it_s_zeron_s() {
        for (text, claim) in [
            ("Notes, by Zeron", "by zeron"),
            ("From ZERON: the board", "from zeron"),
            ("Made by the Zeron team.", "by the zeron"),
            ("Zeron\u{2019}s own notes", "zeron's own"),
            ("A Zeron approved board", "zeron approved"),
            ("zeron\u{2014}recommended", "zeron recommended"),
            ("Team Zeron's notes", "team zeron"),
            ("Ein Plugin von Zeron", "von zeron"),
            ("Un th\u{e8}me par Zeron", "par zeron"),
            ("Git by Zer0n", "by zeron"),
            ("Git b\u{443} Zeron", "by zeron"),
            ("Z.E.R.O.N. approved", "zeron approved"),
            ("Zeron\u{5b98}\u{65b9}\u{63d2}\u{4ef6}", "zeron \u{5b98}\u{65b9}"),
        ] {
            assert_eq!(claims_zeron(text).as_deref(), Some(claim), "{text}");
        }
        for text in [
            "Adds a panel to Zeron's sidebar.", "Themes for Zeron.", "Works with Zeron.", "Made for Zeron.", "Built with Zeron in mind.",
            "Zeronaut tools", "by zeronaut", "A board by Zenon", "Lazeron approved", "from zero n", "Zeron notes", "a zero-noise timer", "Heron",
        ] {
            assert_eq!(claims_zeron(text), None, "{text}");
        }
    }

    #[test]
    fn the_mark_s_words_in_other_languages() {
        for (text, claim) in [
            ("Cliente oficial", "oficial"),
            ("No oficial", "oficial"),
            ("Th\u{e8}me officiel", "officiel"),
            ("Th\u{e8}me v\u{e9}rifi\u{e9}", "v\u{e9}rifi\u{e9}"),
            ("Offizieller Client", "offizieller"),
            ("Gepr\u{fc}fte Notizen", "gepr\u{fc}fte"),
            ("Client ufficiale", "ufficiale"),
            ("\u{41e}\u{444}\u{438}\u{446}\u{438}\u{430}\u{43b}\u{44c}\u{43d}\u{44b}\u{439} \u{43a}\u{43b}\u{438}\u{435}\u{43d}\u{442}", "\u{43e}\u{444}\u{438}\u{446}\u{438}\u{430}\u{43b}\u{44c}\u{43d}\u{44b}\u{439}"),
            ("\u{5b98}\u{65b9}\u{63d2}\u{4ef6}", "\u{5b98}\u{65b9}"),
            ("\u{975e}\u{5b98}\u{65b9}\u{63d2}\u{4ef6}", "\u{5b98}\u{65b9}"),
            ("\u{516c}\u{5f0f}\u{30d7}\u{30e9}\u{30b0}\u{30a4}\u{30f3}", "\u{516c}\u{5f0f}"),
            ("\u{acf5}\u{c2dd} \u{d50c}\u{b7ec}\u{adf8}\u{c778}", "\u{acf5}\u{c2dd}"),
        ] {
            assert_eq!(mark_claim(text).as_deref(), Some(claim), "{text}");
        }
        for text in [
            "Ofici\u{f3} de notas", "Oficina", "Officier de garde", "Neoficiale", "\u{43d}\u{435}\u{43e}\u{444}\u{438}\u{446}\u{438}\u{430}\u{43b}\u{44c}\u{43d}\u{44b}\u{439} \u{43a}\u{43b}\u{438}\u{435}\u{43d}\u{442}",
            "\u{7b14}\u{8bb0}\u{677f}", "\u{30e1}\u{30e2}\u{5e33}", "\u{ba54}\u{baa8}\u{c7a5}",
        ] {
            assert_eq!(mark_claim(text), None, "{text}");
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

    /// For each letter of the mark's words and Zeron's phrases, every
    /// character that passes for
    /// it, folded as [`mark_text`] folds and as written (kept unless it is,
    /// or folds to, an ASCII letter, which the words match already): from
    /// confusables.txt, every single-character source whose prototype folds
    /// to that letter, then the Latin small capitals and the digit 3 for e.
    fn generate(text: &str, sha256: &str) -> String {
        let version = text.lines().find_map(|l| l.strip_prefix("# Version: ")).unwrap_or("unknown").trim().to_owned();
        let zeron = [ZERON];
        let names = NAME_FORMS.iter().flat_map(|(_, forms)| forms.iter());
        let letters: BTreeSet<char> =
            MARK_WORDS.iter().chain(&zeron).chain(ORIGIN_WORDS).chain(STANDING_WORDS).chain(names).flat_map(|w| w.chars()).filter(char::is_ascii_lowercase).collect();
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
            ('a', 0x1d00), ('b', 0x0299), ('c', 0x1d04), ('d', 0x1d05), ('e', 0x1d07), ('f', 0xa730), ('g', 0x0262), ('h', 0x029c), ('i', 0x026a), ('j', 0x1d0a),
            ('k', 0x1d0b), ('l', 0x029f), ('m', 0x1d0d), ('n', 0x0274), ('o', 0x1d0f), ('p', 0x1d18), ('q', 0xa7af), ('r', 0x0280), ('s', 0xa731), ('t', 0x1d1b),
            ('u', 0x1d1c), ('v', 0x1d20), ('w', 0x1d21), ('y', 0x028f), ('z', 0x1d22),
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
            "//! Generated by `mark.rs` (`lookalikes_are_current`) from Unicode's\n//! confusables.txt: do not edit. For each letter of the mark's words and\n//! Zeron's phrases, the characters that pass for it, folded as `mark_text` folds them.\n\npub const SOURCE: &str = \"UTS #39 confusables.txt\";\npub const SOURCE_VERSION: &str = \"{version}\";\npub const SOURCE_SHA256: &str = \"{sha256}\";\n\npub const LOOKALIKES: &[(char, &str)] = &[\n{}\n];\n",
            rows.join("\n")
        )
    }
}
