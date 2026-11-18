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

/// Where a word of [`OTHER_LANGUAGES`] is refused in a description (a name
/// refuses every one, folded).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InDescription {
    /// Folded, as a name reads it.
    Folded,
    /// Only as written with its accents (French "vérifié", whose folded
    /// form is also "vérifie", "checks").
    Accented,
    /// Never: as written it's also an ordinary word (German "verifiziert",
    /// "verifies"; "offiziell", "officially").
    NameOnly,
}

/// A word of [`OTHER_LANGUAGES`].
#[derive(Debug, Clone, Copy)]
pub struct OtherWord {
    pub word: &'static str,
    pub description: InDescription,
    /// Refused only in a text that also holds kana: Japanese 公式 is
    /// "official", Chinese 公式 "formula".
    pub with_kana: bool,
    /// Refused in a name only directly before one of these (a space
    /// allowed): Korean 공식 is "official" before 플러그인, else also
    /// "formula". Empty: anywhere.
    pub before: &'static [&'static str],
}

const fn w(word: &'static str) -> OtherWord {
    OtherWord { word, description: InDescription::Folded, with_kana: false, before: &[] }
}

const fn accent(word: &'static str) -> OtherWord {
    OtherWord { word, description: InDescription::Accented, with_kana: false, before: &[] }
}

const fn name_only(word: &'static str) -> OtherWord {
    OtherWord { word, description: InDescription::NameOnly, with_kana: false, before: &[] }
}

const fn with_kana(word: &'static str) -> OtherWord {
    OtherWord { word, description: InDescription::NameOnly, with_kana: true, before: &[] }
}

/// The mark's words in the store's other languages: `(language, substring,
/// words)`, each folded as the text is; whole words (no letter or digit of
/// any script right before or after) where words are spaced, substrings in
/// Chinese, Japanese and Korean. A word is here only when its matched form
/// has no common other meaning in text about software, or with the narrower
/// reading it needs ([`InDescription`], `with_kana`). "No oficial" and
/// "非官方" are refused as "not official" is: the author rewords.
pub const OTHER_LANGUAGES: &[(&str, bool, &[OtherWord])] = &[
    ("es", false, &[w("verificado"), w("verificada"), w("verificados"), w("verificadas"), w("oficial"), w("oficiales")]),
    ("fr", false, &[
        accent("v\u{e9}rifi\u{e9}"), accent("v\u{e9}rifi\u{e9}e"), accent("v\u{e9}rifi\u{e9}s"), accent("v\u{e9}rifi\u{e9}es"),
        w("officiel"), w("officielle"), w("officiels"), w("officielles"),
    ]),
    ("de", false, &[
        // "verifiziert" is also "verifies"; "geprüft" also "checked" (as
        // inputs are); "offiziell" also "officially".
        name_only("verifiziert"), w("verifizierte"), w("verifizierter"), w("verifiziertes"), w("verifizierten"),
        name_only("gepr\u{fc}ft"), name_only("gepr\u{fc}fte"), name_only("gepr\u{fc}fter"), name_only("gepr\u{fc}ftes"), name_only("gepr\u{fc}ften"),
        name_only("offiziell"), w("offizielle"), w("offizieller"), w("offizielles"), w("offiziellen"),
    ]),
    ("it", false, &[w("verificato"), w("verificata"), w("verificati"), w("verificate"), w("ufficiale"), w("ufficiali")]),
    ("pt", false, &[w("verificado"), w("verificada"), w("verificados"), w("verificadas"), w("oficial"), w("oficiais")]),
    ("ru", false, &[
        // A word ending in "*" is a stem: any ending (Russian's case forms).
        // "провер(ен)…" is also "checked", "proven" (a proven way): a name
        // only. "официальн…" listed in full: its stem would take
        // "официально", "officially", which English doesn't refuse.
        name_only("\u{43f}\u{440}\u{43e}\u{432}\u{435}\u{440}\u{435}\u{43d}*"),
        w("\u{432}\u{435}\u{440}\u{438}\u{444}\u{438}\u{446}\u{438}\u{440}\u{43e}\u{432}\u{430}\u{43d}*"),
        w("\u{43e}\u{444}\u{438}\u{446}\u{438}\u{430}\u{43b}\u{44c}\u{43d}\u{44b}\u{439}"),
        w("\u{43e}\u{444}\u{438}\u{446}\u{438}\u{430}\u{43b}\u{44c}\u{43d}\u{430}\u{44f}"),
        w("\u{43e}\u{444}\u{438}\u{446}\u{438}\u{430}\u{43b}\u{44c}\u{43d}\u{43e}\u{435}"),
        w("\u{43e}\u{444}\u{438}\u{446}\u{438}\u{430}\u{43b}\u{44c}\u{43d}\u{44b}\u{435}"),
        w("\u{43e}\u{444}\u{438}\u{446}\u{438}\u{430}\u{43b}\u{44c}\u{43d}\u{43e}\u{433}\u{43e}"),
        w("\u{43e}\u{444}\u{438}\u{446}\u{438}\u{430}\u{43b}\u{44c}\u{43d}\u{43e}\u{439}"),
        w("\u{43e}\u{444}\u{438}\u{446}\u{438}\u{430}\u{43b}\u{44c}\u{43d}\u{43e}\u{43c}\u{443}"),
        w("\u{43e}\u{444}\u{438}\u{446}\u{438}\u{430}\u{43b}\u{44c}\u{43d}\u{44b}\u{43c}"),
        w("\u{43e}\u{444}\u{438}\u{446}\u{438}\u{430}\u{43b}\u{44c}\u{43d}\u{43e}\u{43c}"),
        w("\u{43e}\u{444}\u{438}\u{446}\u{438}\u{430}\u{43b}\u{44c}\u{43d}\u{443}\u{44e}"),
        w("\u{43e}\u{444}\u{438}\u{446}\u{438}\u{430}\u{43b}\u{44c}\u{43d}\u{44b}\u{445}"),
        w("\u{43e}\u{444}\u{438}\u{446}\u{438}\u{430}\u{43b}\u{44c}\u{43d}\u{44b}\u{43c}\u{438}"),
    ]),
    // 官方 "official"; 已验证 "verified". Not 已认证 (also "authenticated").
    ("zh", true, &[w("\u{5b98}\u{65b9}"), w("\u{5df2}\u{9a8c}\u{8bc1}"), w("\u{5df2}\u{9a57}\u{8b49}")]),
    // In a name, 公式 and 公認 only beside kana (Chinese 公式 is "formula");
    // 検証済 "verified"; オフィシャル "official". Not 認証済み
    // ("authenticated") nor 確認済み ("checked"). In a description 公式 and
    // 公認 count near a word for the plugin ([`AMBIGUOUS`]).
    ("ja", true, &[with_kana("\u{516c}\u{5f0f}"), with_kana("\u{516c}\u{8a8d}"), w("\u{691c}\u{8a3c}\u{6e08}"), w("\u{30aa}\u{30d5}\u{30a3}\u{30b7}\u{30e3}\u{30eb}")]),
    // 검증됨 "verified"; in a name 공식 only before 플러그인, 앱, 패키지, 확장,
    // 버전 (else also "formula"). Not 공인 (공인인증서 is an ordinary term),
    // 인증됨 ("authenticated"), 확인됨 ("checked").
    ("ko", true, &[
        w("\u{ac80}\u{c99d}\u{b428}"),
        OtherWord {
            word: "\u{acf5}\u{c2dd}",
            description: InDescription::NameOnly,
            with_kana: false,
            before: &["\u{d50c}\u{b7ec}\u{adf8}\u{c778}", "\u{c571}", "\u{d328}\u{d0a4}\u{c9c0}", "\u{d655}\u{c7a5}", "\u{bc84}\u{c804}"],
        },
    ]),
];

/// Words beside which "zeron" claims standing though alone they're not the
/// mark (each of [`OTHER_LANGUAGES`] is too): 公式, 공식, 认证, 認證, 認証,
/// 인증 and their "done" forms, 確認済み, 확인됨.
pub const STANDING_OTHER: &[&str] = &[
    "\u{516c}\u{5f0f}", "\u{acf5}\u{c2dd}", "\u{8ba4}\u{8bc1}", "\u{8a8d}\u{8b49}", "\u{8a8d}\u{8a3c}", "\u{c778}\u{c99d}",
    "\u{5df2}\u{8ba4}\u{8bc1}", "\u{5df2}\u{8a8d}\u{8b49}", "\u{8a8d}\u{8a3c}\u{6e08}", "\u{c778}\u{c99d}\u{b428}", "\u{78ba}\u{8a8d}\u{6e08}", "\u{d655}\u{c778}\u{b428}",
    "\u{516c}\u{8a8d}", "\u{acf5}\u{c778}",
];

/// In a description, the words that alone aren't the mark but say it near a
/// word for the plugin (rule 2: "已认证插件", "認証済みプラグイン", "공식
/// 플러그인"): [`STANDING_OTHER`]'s.
pub const AMBIGUOUS: &[&str] = STANDING_OTHER;

/// Words for the plugin itself, by language (rule 2), folded as the text
/// is: whole words where words are spaced (a phrase's words in order), and
/// substrings in Chinese, Japanese and Korean.
pub const PLUGIN_WORDS: &[(&str, bool, &[&str])] = &[
    ("en", false, &[
        "plugin", "plugins", "plug-in", "extension", "extensions", "add-on", "add-ons", "addon", "addons", "integration", "integrations", "client",
        "app", "apps", "tool", "tools", "theme", "themes", "package", "packages", "version", "versions", "release", "releases", "build", "builds",
    ]),
    ("es", false, &["complemento", "complementos", "extensi\u{f3}n", "extensiones", "cliente", "aplicaci\u{f3}n", "herramienta", "herramientas", "tema", "temas", "paquete", "versi\u{f3}n", "integraci\u{f3}n"]),
    ("fr", false, &["module", "modules", "outil", "outils", "th\u{e8}me", "th\u{e8}mes", "paquet", "int\u{e9}gration", "application"]),
    ("de", false, &["erweiterung", "erweiterungen", "anwendung", "werkzeug", "paket", "pakete"]),
    ("it", false, &["estensione", "estensioni", "applicazione", "strumento", "strumenti", "pacchetto", "versione", "integrazione"]),
    ("pt", false, &["extens\u{e3}o", "extens\u{f5}es", "aplicativo", "ferramenta", "ferramentas", "pacote", "vers\u{e3}o", "integra\u{e7}\u{e3}o"]),
    ("ru", false, &[
        "\u{43f}\u{43b}\u{430}\u{433}\u{438}\u{43d}*", "\u{440}\u{430}\u{441}\u{448}\u{438}\u{440}\u{435}\u{43d}\u{438}*", "\u{43a}\u{43b}\u{438}\u{435}\u{43d}\u{442}*",
        "\u{43f}\u{440}\u{438}\u{43b}\u{43e}\u{436}\u{435}\u{43d}\u{438}*", "\u{438}\u{43d}\u{441}\u{442}\u{440}\u{443}\u{43c}\u{435}\u{43d}\u{442}*", "\u{43f}\u{430}\u{43a}\u{435}\u{442}*",
        "\u{432}\u{435}\u{440}\u{441}\u{438}*", "\u{438}\u{43d}\u{442}\u{435}\u{433}\u{440}\u{430}\u{446}\u{438}*",
        "\u{442}\u{435}\u{43c}\u{430}", "\u{442}\u{435}\u{43c}\u{44b}", "\u{442}\u{435}\u{43c}\u{435}", "\u{442}\u{435}\u{43c}\u{443}", "\u{442}\u{435}\u{43c}\u{43e}\u{439}",
    ]),
    ("zh", true, &["\u{63d2}\u{4ef6}", "\u{6269}\u{5c55}", "\u{64f4}\u{5c55}", "\u{5ba2}\u{6237}\u{7aef}", "\u{5ba2}\u{6236}\u{7aef}", "\u{5e94}\u{7528}", "\u{61c9}\u{7528}", "\u{5de5}\u{5177}", "\u{4e3b}\u{9898}", "\u{4e3b}\u{984c}", "\u{7248}\u{672c}", "\u{96c6}\u{6210}"]),
    ("ja", true, &["\u{30d7}\u{30e9}\u{30b0}\u{30a4}\u{30f3}", "\u{62e1}\u{5f35}", "\u{30a2}\u{30d7}\u{30ea}", "\u{30af}\u{30e9}\u{30a4}\u{30a2}\u{30f3}\u{30c8}", "\u{30c4}\u{30fc}\u{30eb}", "\u{30c6}\u{30fc}\u{30de}", "\u{30d1}\u{30c3}\u{30b1}\u{30fc}\u{30b8}", "\u{30d0}\u{30fc}\u{30b8}\u{30e7}\u{30f3}", "\u{9023}\u{643a}"]),
    ("ko", true, &["\u{d50c}\u{b7ec}\u{adf8}\u{c778}", "\u{d655}\u{c7a5}", "\u{c571}", "\u{d074}\u{b77c}\u{c774}\u{c5b8}\u{d2b8}", "\u{b3c4}\u{ad6c}", "\u{d14c}\u{b9c8}", "\u{d328}\u{d0a4}\u{c9c0}", "\u{bc84}\u{c804}", "\u{d1b5}\u{d569}"]),
];

/// Words that say a mark word of the subject when it follows them directly
/// (rule 3: "is verified", "now official"). Not Italian "è" nor
/// Portuguese "é": folded, both are "e", "and".
pub const SUBJECT_WORDS: &[&str] = &[
    "is", "are", "was", "were", "be", "been", "now", "fully", "officially",
    "es", "son", "est\u{e1}", "est\u{e1}n", "fue", "ahora", "totalmente", "oficialmente",
    "est", "sont", "\u{e9}t\u{e9}", "maintenant", "enti\u{e8}rement", "officiellement",
    "ist", "sind", "wurde", "jetzt", "vollst\u{e4}ndig",
    "sono", "stato", "stata", "ora", "ufficialmente", "completamente",
    "s\u{e3}o", "foi", "agora",
    "\u{442}\u{435}\u{43f}\u{435}\u{440}\u{44c}", "\u{43f}\u{43e}\u{43b}\u{43d}\u{43e}\u{441}\u{442}\u{44c}\u{44e}", "\u{43e}\u{444}\u{438}\u{446}\u{438}\u{430}\u{43b}\u{44c}\u{43d}\u{43e}", "\u{44f}\u{432}\u{43b}\u{44f}\u{435}\u{442}\u{441}\u{44f}",
    "\u{662f}", "\u{5df2}\u{7ecf}", "\u{5df2}\u{7d93}", "\u{73b0}\u{5728}", "\u{73fe}\u{5728}", "\u{5b8c}\u{5168}", "\u{6b63}\u{5f0f}",
];

/// A negation directly before a mark word lifts rules 1 to 3 ("not
/// official", "cliente no oficial", "非官方客户端", "비공식"); in a script
/// that doesn't space words, directly before it inside its word too.
pub const NEGATIONS: &[&str] = &[
    "not", "no", "non", "n\u{e3}o", "nicht", "kein", "keine", "keiner", "pas", "\u{43d}\u{435}", "\u{43d}\u{435}\u{442}", "\u{975e}", "\u{4e0d}", "\u{672a}", "\u{be44}", "\u{bbf8}",
];

/// Where a sentence ends (rules 1 and 2): . ! ? ; and their full-width
/// forms, and a line break.
pub const SENTENCE_ENDS: &[char] = &['.', '!', '?', ';', '\u{3002}', '\u{ff01}', '\u{ff1f}', '\u{ff1b}', '\n'];

/// A mark word beside one of these is said of the store or Zeron (rule 4):
/// no negation lifts it. ("By …" directly after it is beside too:
/// "verified by the store".)
pub const STORE_WORDS: &[&str] = &["zeron", "the store", "the plugin store", "by zeron", "by the store", "by the plugin store"];

/// Saying the plugin is part of Zeron: in a name with "zeron", any of these
/// anywhere; in a description, one directly before "(with | in | into | to
/// | for | of) zeron" ("ships with Zeron", "part of Zeron"; a word that
/// ends in one of those is directly before "zeron").
pub const BELONGING: &[&str] = &[
    "built-in", "builtin", "built in", "built into", "bundled", "included", "preinstalled", "pre-installed", "ships", "shipped", "comes with", "native",
    "default", "stock", "core", "first-party", "part of",
];

/// The words that join a word of [`BELONGING`] to "zeron".
pub const BELONGING_JOINS: &[&str] = &["with", "in", "into", "to", "for", "of"];

/// [`BELONGING`] in Chinese: 内置 "built-in", 自带 "comes with", 预装
/// "preinstalled", 默认 "default", 原生 "native", and the traditional forms.
/// In a description, directly before 于, 在 or 到 "zeron" ("内置于
/// Zeron"), or after 随 "zeron" ("随 Zeron 自带"); "Zeron 内置终端" ("Zeron's
/// built-in terminal") says nothing.
pub const BELONGING_ZH: &[&str] = &[
    "\u{5185}\u{7f6e}", "\u{5167}\u{7f6e}", "\u{81ea}\u{5e26}", "\u{81ea}\u{5e36}", "\u{9884}\u{88c5}", "\u{9810}\u{88dd}", "\u{9ed8}\u{8ba4}", "\u{9ed8}\u{8a8d}", "\u{539f}\u{751f}",
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
pub const ORIGIN_WORDS: &[&str] = &["by", "from", "by the", "made by", "built by", "von", "par", "de", "da", "di", "del", "van", "\u{7531}", "\u{6765}\u{81ea}"];

/// In a description, "zeron" right before or after one of these (or its
/// possessive, "zeron's team") reads as its standing.
pub const STANDING_WORDS: &[&str] = &[
    "team", "labs", "inc", "hq", "staff", "approved", "certified", "endorsed", "recommended", "own", "\u{5b98}\u{65b9}", "\u{516c}\u{5f0f}", "\u{acf5}\u{c2dd}",
];

/// Standing only directly after a bare "zeron" ("Zeron-made", "Zeron
/// built"), never after "zeron's" and never as "built-in" / "built in".
pub const STANDING_AFTER_ZERON: &[&str] = &["made", "built"];

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

/// The letter of the table `c` passes for, if it's one of its look-alikes
/// (a Cyrillic о is an o): a word's letter is read as that letter.
fn reads_as(c: char) -> char {
    if LOOKALIKES.iter().any(|(l, _)| *l == c) {
        return c;
    }
    LOOKALIKES.iter().find(|(_, chars)| chars.contains(c)).map_or(c, |(l, _)| *l)
}

/// What may stand for `letter` in a word: its look-alikes, and those of the
/// letter it's interchangeable with (a letter of another script that is
/// itself a look-alike reads as the letter it passes for: [`reads_as`]).
/// Computed once for each letter asked.
fn class_of(letter: char) -> std::sync::Arc<BTreeSet<char>> {
    static CLASSES: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<char, std::sync::Arc<BTreeSet<char>>>>> = std::sync::OnceLock::new();
    let classes = CLASSES.get_or_init(Default::default);
    if let Some(class) = classes.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(&letter) {
        return class.clone();
    }
    let base = reads_as(letter);
    let mut chars = lookalikes_of(base);
    chars.insert(letter);
    for (a, b) in INTERCHANGEABLE {
        let other = if *a == base { Some(*b) } else if *b == base { Some(*a) } else { None };
        if let Some(other) = other {
            chars.extend(lookalikes_of(other));
        }
    }
    let class = std::sync::Arc::new(chars);
    classes.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(letter, class.clone());
    class
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
/// [`MARK_GLYPHS`], the [`MARK_WORDS`] word it spells, or a word of
/// [`OTHER_LANGUAGES`]: in a name (`in_name`) anywhere ([`name_mark`]); in
/// a description only when said of the plugin itself
/// ([`description_mark`]).
pub fn mark_claim(text: &str, in_name: bool) -> Option<String> {
    if in_name { name_mark(text) } else { description_mark(text) }
}

/// A name's mark: any glyph, any mark word, each of [`OTHER_LANGUAGES`]
/// folded (with its kana and `before` conditions).
fn name_mark(text: &str) -> Option<String> {
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
    // A word that alone isn't the mark, directly before a word for the
    // plugin (a space allowed): "认证插件", "공인 플러그인", "公認プラグイン".
    let cjk_plugin_words: Vec<Vec<char>> = PLUGIN_WORDS.iter().filter(|(_, substring, _)| *substring).flat_map(|(_, _, words)| words.iter().map(|w| folded_word(w))).collect();
    for word in AMBIGUOUS {
        let wanted = folded_word(word);
        let before_plugin = readings.iter().any(|reading| {
            spans(reading, &wanted, true).into_iter().any(|(_, end)| {
                let rest: Vec<char> = reading[end..].iter().copied().skip_while(|c| separator(*c)).collect();
                cjk_plugin_words.iter().any(|p| rest.starts_with(p))
            })
        });
        if before_plugin {
            return Some((*word).to_owned());
        }
    }
    // In the store's other languages.
    let kana = has_kana(text);
    OTHER_LANGUAGES.iter().flat_map(|(_, substring, words)| words.iter().map(move |w| (*substring, w))).find_map(|(substring, word)| {
        if word.with_kana && !kana {
            return None;
        }
        let (listed_word, stem) = listed(word.word);
        let wanted = folded_word(listed_word);
        readings.iter().find_map(|reading| {
            spelled_spans(reading, &wanted, substring, stem).into_iter().find_map(|(start, end)| {
                let before = word.before.is_empty() || {
                    let rest: Vec<char> = reading[end..].iter().copied().skip_while(|c| separator(*c)).collect();
                    word.before.iter().any(|b| rest.starts_with(&folded_word(b)))
                };
                // (A stem says the word as written: "проверено".)
                before.then(|| if stem { reading[start..end].iter().collect() } else { word.word.to_owned() })
            })
        })
    })
}

fn has_kana(text: &str) -> bool {
    text.chars().any(|c| matches!(c, '\u{3040}'..='\u{30FF}' | '\u{31F0}'..='\u{31FF}' | '\u{FF66}'..='\u{FF9F}'))
}

/// A Han ideograph: a word by itself (Chinese and Japanese don't space
/// their words).
fn han(c: char) -> bool {
    matches!(c, '\u{3400}'..='\u{4DBF}' | '\u{4E00}'..='\u{9FFF}' | '\u{F900}'..='\u{FAFF}' | '\u{20000}'..='\u{3134F}')
}

/// The words of `text`, `(start, end)`: each run of characters that aren't
/// separators, each Han ideograph a word by itself.
fn words_of(text: &[char]) -> Vec<(usize, usize)> {
    let mut words = Vec::new();
    let mut start: Option<usize> = None;
    for (at, c) in text.iter().enumerate() {
        if separator(*c) || han(*c) {
            if let Some(from) = start.take() {
                words.push((from, at));
            }
            if han(*c) {
                words.push((at, at + 1));
            }
        } else if start.is_none() {
            start = Some(at);
        }
    }
    if let Some(from) = start {
        words.push((from, text.len()));
    }
    words
}

/// Where `wanted` is in `text`: anywhere (`substring`), or whole (no letter
/// or digit of any script right before or after).
fn spans(text: &[char], wanted: &[char], substring: bool) -> Vec<(usize, usize)> {
    let any_word = |c: Option<&char>| c.is_some_and(|c| matches!(c.general_category_group(), GeneralCategoryGroup::Letter | GeneralCategoryGroup::Number));
    if wanted.is_empty() {
        return Vec::new();
    }
    (0..text.len())
        .filter(|at| {
            text[*at..].starts_with(wanted)
                && (substring || (!any_word(at.checked_sub(1).and_then(|b| text.get(b))) && !any_word(text.get(at + wanted.len()))))
        })
        .map(|at| (at, at + wanted.len()))
        .collect()
}

/// A listed word and whether it's a stem (written with a trailing `*`: any
/// ending).
fn listed(word: &str) -> (&str, bool) {
    word.strip_suffix('*').map_or((word, false), |stem| (stem, true))
}

fn any_word(c: Option<&char>) -> bool {
    c.is_some_and(|c| matches!(c.general_category_group(), GeneralCategoryGroup::Letter | GeneralCategoryGroup::Number))
}

/// Where `word`'s letters, each one of its class or a pair, end when they
/// match `text` from `at` (no boundary asked).
fn letters_end(text: &[char], at: usize, word: &[char]) -> Option<usize> {
    let Some((&letter, rest)) = word.split_first() else { return Some(at) };
    if text.get(at).is_some_and(|c| class_of(letter).contains(c))
        && let Some(end) = letters_end(text, at + 1, rest)
    {
        return Some(end);
    }
    PAIRS.iter().filter(|(l, _)| *l == letter).find_map(|(_, pair)| {
        let pair: Vec<char> = pair.chars().collect();
        let fits = pair.iter().enumerate().all(|(n, p)| text.get(at + n).is_some_and(|c| lookalikes_of(*p).contains(c)));
        if fits { letters_end(text, at + pair.len(), rest) } else { None }
    })
}

/// Where a listed word is spelled in `text`, with the look-alikes as the
/// mark's words are: anywhere (`substring`), or whole (no letter or digit of
/// any script right before or after); a stem with any ending.
fn spelled_spans(text: &[char], word: &[char], substring: bool, stem: bool) -> Vec<(usize, usize)> {
    if word.is_empty() {
        return Vec::new();
    }
    (0..text.len())
        .filter(|at| substring || !any_word(at.checked_sub(1).and_then(|b| text.get(b))))
        .filter_map(|at| {
            let end = letters_end(text, at, word)?;
            if stem {
                let end = end + text[end..].iter().take_while(|c| any_word(Some(c))).count();
                return Some((at, end));
            }
            (substring || !any_word(text.get(end))).then_some((at, end))
        })
        .collect()
}

/// A reading of a description: its text, its words, and how a listed word
/// is put in its form.
struct Reading {
    text: Vec<char>,
    words: Vec<(usize, usize)>,
    form: fn(&str) -> Vec<char>,
}

impl Reading {
    fn new(text: Vec<char>, form: fn(&str) -> Vec<char>) -> Self {
        let words = words_of(&text);
        Reading { text, words, form }
    }

    /// The words a span covers: (first, last).
    fn covers(&self, (start, end): (usize, usize)) -> Option<(usize, usize)> {
        let first = self.words.iter().position(|w| w.1 > start)?;
        let last = self.words.iter().rposition(|w| w.0 < end)?;
        (first <= last).then_some((first, last))
    }

    fn sentence_end_in(&self, from: usize, to: usize) -> bool {
        from < to && self.text[from..to].iter().any(|c| SENTENCE_ENDS.contains(c))
    }

    /// Whether the words `at - n .. at` (directly before word `at`, nothing
    /// ending a sentence between) read as `phrase`.
    fn before_is(&self, at: usize, phrase: &str) -> bool {
        let wanted = words_of(&(self.form)(phrase)).len();
        let phrase_text: Vec<char> = (self.form)(phrase);
        let phrase_words: Vec<Vec<char>> = words_of(&phrase_text).iter().map(|(a, b)| phrase_text[*a..*b].to_vec()).collect();
        if wanted == 0 || at < wanted || self.sentence_end_in(self.words[at - wanted].0, self.words[at].0) {
            return false;
        }
        (0..wanted).all(|n| {
            let (a, b) = self.words[at - wanted + n];
            same_word(&self.text[a..b], &phrase_words[n])
        })
    }

    /// Whether the words directly after word `at` read as `phrase`.
    fn after_is(&self, at: usize, phrase: &str) -> bool {
        let phrase_text: Vec<char> = (self.form)(phrase);
        let phrase_words: Vec<Vec<char>> = words_of(&phrase_text).iter().map(|(a, b)| phrase_text[*a..*b].to_vec()).collect();
        let wanted = phrase_words.len();
        if wanted == 0 || at + wanted >= self.words.len() || self.sentence_end_in(self.words[at].1, self.words[at + wanted].1) {
            return false;
        }
        (0..wanted).all(|n| {
            let (a, b) = self.words[at + 1 + n];
            same_word(&self.text[a..b], &phrase_words[n])
        })
    }

    /// Whether a mark word at `span` is said of the plugin itself (or of
    /// the store, or Zeron). `ambiguous`: a word that alone isn't the mark
    /// (rule 2 and 4 only).
    fn said_of_plugin(&self, span: (usize, usize), ambiguous: bool) -> bool {
        let Some((first, last)) = self.covers(span) else { return false };
        // 4: beside "zeron", "the store", "the plugin store". No negation
        // lifts it.
        if STORE_WORDS.iter().any(|w| self.before_is(first, w) || self.after_is(last, w)) {
            return true;
        }
        // A negation directly before lifts 1 to 3: the word before, or (in
        // a script that doesn't space words) inside this word.
        let inside: Vec<char> = self.text[self.words[first].0..span.0].to_vec();
        if NEGATIONS.iter().any(|n| self.before_is(first, n) || (!inside.is_empty() && inside.ends_with(&(self.form)(n)))) {
            return false;
        }
        if !ambiguous {
            // 1: the text's first word, or alone as a sentence.
            if first == 0 {
                return true;
            }
            let from = self.text[..span.0].iter().rposition(|c| SENTENCE_ENDS.contains(c)).map_or(0, |at| at + 1);
            let to = self.text[span.1..].iter().position(|c| SENTENCE_ENDS.contains(c)).map_or(self.text.len(), |at| span.1 + at);
            let alone = self.words.iter().enumerate().filter(|(_, w)| w.0 >= from && w.1 <= to).all(|(n, _)| (first..=last).contains(&n));
            if alone {
                return true;
            }
            // 3: directly after a word that says it of the subject.
            if SUBJECT_WORDS.iter().any(|w| self.before_is(first, w)) {
                return true;
            }
        }
        // 2: within two words of a word for the plugin, nothing ending a
        // sentence between.
        PLUGIN_WORDS.iter().any(|(_, substring, words)| {
            words.iter().any(|word| {
                let (word, stem) = listed(word);
                let wanted = (self.form)(word);
                let found: Vec<(usize, usize)> = if *substring {
                    spans(&self.text, &wanted, true)
                } else {
                    self.phrase_spans(&wanted, stem)
                };
                found.into_iter().any(|other| {
                    let Some((o_first, o_last)) = self.covers(other) else { return false };
                    let (gap, from, to) = if o_first > last {
                        (o_first - last - 1, span.1, other.0)
                    } else if first > o_last {
                        (first - o_last - 1, other.1, span.0)
                    } else {
                        (0, 0, 0)
                    };
                    gap <= 2 && !self.sentence_end_in(from, to)
                })
            })
        })
    }

    /// Where a word or phrase of spaced words is, whole word by word (its
    /// last word a stem: any ending).
    fn phrase_spans(&self, phrase: &[char], stem: bool) -> Vec<(usize, usize)> {
        let parts: Vec<Vec<char>> = words_of(phrase).iter().map(|(a, b)| phrase[*a..*b].to_vec()).collect();
        if parts.is_empty() || parts.len() > self.words.len() {
            return Vec::new();
        }
        (0..=self.words.len() - parts.len())
            .filter(|at| {
                parts.iter().enumerate().all(|(n, part)| {
                    let (a, b) = self.words[at + n];
                    let last = n + 1 == parts.len();
                    same_word(&self.text[a..b], part) || (stem && last && self.text[a..b].starts_with(part))
                }) && !self.sentence_end_in(self.words[*at].0, self.words[at + parts.len() - 1].1)
            })
            .map(|at| (self.words[at].0, self.words[at + parts.len() - 1].1))
            .collect()
    }
}

/// Whether a word of a text is `word`: the same, or (for "zeron") spelled
/// with look-alikes.
fn same_word(text: &[char], word: &[char]) -> bool {
    if text == word {
        return true;
    }
    let zeron: Vec<char> = ZERON.chars().collect();
    word == zeron.as_slice() && spell_end(text, 0, &zeron) == Some(text.len())
}

/// A description's mark: a glyph anywhere; a mark word only when said of the
/// plugin itself (the text's first word or alone as a sentence; within two
/// words of a word for the plugin; directly after a word that says it of
/// the subject; beside "zeron" or the store), unless a negation is directly
/// before it (not for the last).
fn description_mark(text: &str) -> Option<String> {
    let folded_text = folded(text);
    if let Some(glyph) = folded_text.iter().find(|c| MARK_GLYPHS.contains(c)) {
        return Some(glyph.to_string());
    }
    let folded_readings = [Reading::new(folded_text.clone(), folded_word), Reading::new(unspaced(&folded_text), folded_word)];
    for reading in &folded_readings {
        for word in MARK_WORDS {
            let spelling: Vec<char> = mark_text(word).chars().collect();
            let found = (0..reading.text.len())
                .filter(|at| !ascii_word(at.checked_sub(1).and_then(|b| reading.text.get(b))))
                .filter_map(|at| spell_end(&reading.text, at, &spelling).map(|end| (at, end)));
            if found.into_iter().any(|span| reading.said_of_plugin(span, false)) {
                return Some((*word).to_owned());
            }
        }
        for (_, substring, words) in OTHER_LANGUAGES {
            for word in words.iter().filter(|w| w.description == InDescription::Folded) {
                let (listed_word, stem) = listed(word.word);
                if let Some((start, end)) = spelled_spans(&reading.text, &folded_word(listed_word), *substring, stem).into_iter().find(|span| reading.said_of_plugin(*span, false)) {
                    return Some(if stem { reading.text[start..end].iter().collect() } else { word.word.to_owned() });
                }
            }
        }
        // Not the mark alone, but near a word for the plugin (or beside
        // the store): 已认证插件, 公式プラグイン, 공식 플러그인.
        let ambiguous = OTHER_LANGUAGES
            .iter()
            .flat_map(|(_, _, words)| words.iter().filter(|w| w.description == InDescription::NameOnly && (w.with_kana || !w.before.is_empty())).map(|w| w.word))
            .chain(AMBIGUOUS.iter().copied());
        for word in ambiguous {
            if spans(&reading.text, &folded_word(word), true).into_iter().any(|span| reading.said_of_plugin(span, true)) {
                return Some(word.to_owned());
            }
        }
    }
    // Words refused only as written with their accents.
    let kept = kept_accents(text);
    for reading in [Reading::new(kept.clone(), kept_accents), Reading::new(unspaced(&kept), kept_accents)] {
        for (_, substring, words) in OTHER_LANGUAGES {
            for word in words.iter().filter(|w| w.description == InDescription::Accented) {
                if spans(&reading.text, &kept_accents(word.word), *substring).into_iter().any(|span| reading.said_of_plugin(span, false)) {
                    return Some(word.word.to_owned());
                }
            }
        }
    }
    None
}

/// `text` folded but keeping its accents: NFKC and lowercase, ignorables
/// and enclosing marks gone (how a description is read for a word that's
/// refused only as written with them).
fn kept_accents(text: &str) -> Vec<char> {
    icu_normalizer::ComposingNormalizerBorrowed::new_nfkc()
        .normalize(text)
        .to_lowercase()
        .chars()
        .filter(|c| !DefaultIgnorableCodePoint::for_char(*c) && c.general_category() != GeneralCategory::EnclosingMark)
        .collect()
}

/// A word of a list, folded as the text is.
fn folded_word(word: &str) -> Vec<char> {
    folded(word)
}

/// `a` then `b`, as a spelling: apart by a space, or by `~` (none or more
/// separators) when either is in a script that doesn't space its words.
fn joined(a: &str, b: &str) -> String {
    if mark_text(a).is_ascii() && mark_text(b).is_ascii() { format!("{a} {b}") } else { format!("{a}~{b}") }
}

/// The spellings that make a name's "zeron" the object of a form.
fn name_forms() -> Vec<String> {
    NAME_FORMS.iter().flat_map(|(_, forms)| forms.iter().map(|form| joined(form, ZERON))).collect()
}

/// The spellings a description may not use: "zeron" after a word of origin,
/// or beside a word of standing.
pub fn zeron_phrases() -> Vec<String> {
    let mut phrases: Vec<String> = ORIGIN_WORDS.iter().map(|o| joined(o, ZERON)).collect();
    let other = OTHER_LANGUAGES.iter().flat_map(|(_, _, words)| words.iter().map(|w| listed(w.word).0));
    for standing in STANDING_WORDS.iter().copied().chain(STANDING_OTHER.iter().copied()).chain(other) {
        phrases.push(joined(ZERON, standing));
        phrases.push(joined(standing, ZERON));
        if mark_text(standing).is_ascii() {
            phrases.push(format!("{ZERON}'s {standing}"));
        }
    }
    phrases
}

/// Whether a name says "zeron" other than as the object of one of
/// [`NAME_FORMS`] ("Themes for Zeron" may; "Zeron Labs", "Git by Zeron"
/// may not), or says it beside a word of [`BELONGING`] anywhere ("Default
/// Git for Zeron"): `Some("zeron")` if it does. Read as the mark's words are.
pub fn claims_zeron_in_name(text: &str) -> Option<String> {
    let folded = folded(text);
    let forms = name_forms();
    [folded.clone(), unspaced(&folded)]
        .iter()
        .any(|reading| {
            let zerons = ends(reading, ZERON);
            let allowed: BTreeSet<usize> = forms.iter().flat_map(|form| ends(reading, form)).collect();
            let belonging = || BELONGING.iter().chain(BELONGING_ZH).any(|b| !ends(reading, b).is_empty());
            zerons.iter().any(|end| !allowed.contains(end)) || (!zerons.is_empty() && belonging())
        })
        .then(|| ZERON.to_owned())
}

/// The spellings that say the plugin is part of Zeron, in a description.
fn belonging_phrases() -> Vec<String> {
    let mut phrases = Vec::new();
    for word in BELONGING {
        if BELONGING_JOINS.iter().any(|join| word.ends_with(&format!(" {join}"))) {
            phrases.push(format!("{word} {ZERON}"));
        } else {
            phrases.extend(BELONGING_JOINS.iter().map(|join| format!("{word} {join} {ZERON}")));
        }
    }
    for word in BELONGING_ZH {
        for join in ["\u{4e8e}", "\u{65bc}", "\u{5728}", "\u{5230}"] {
            phrases.push(format!("{word}~{join}~{ZERON}"));
        }
        phrases.push(format!("\u{968f}~{ZERON}~{word}"));
        phrases.push(format!("\u{96a8}~{ZERON}~{word}"));
    }
    phrases
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
    if let Some(phrase) = zeron_phrases().into_iter().chain(belonging_phrases()).find(|phrase| readings.iter().any(|reading| spelled(reading, phrase))) {
        return Some(phrase.replace('~', " "));
    }
    // "made", "built" only directly after a bare "zeron", never as
    // "built-in" or "built in".
    let in_word: Vec<char> = "in".chars().collect();
    STANDING_AFTER_ZERON
        .iter()
        .find(|word| {
            readings.iter().any(|reading| {
                ends(reading, &format!("{ZERON} {word}")).into_iter().any(|end| {
                    let next: Vec<char> = reading[end..].iter().copied().skip_while(|c| separator(*c)).collect();
                    let gap = reading.len() - end - next.len();
                    !(gap > 0 && (next.starts_with(&in_word) && !ascii_word(next.get(2))))
                })
            })
        })
        .map(|word| format!("{ZERON} {word}"))
}

/// What a keyword says that it may not, if anything: read as a name is,
/// "-" and " " between its parts, a part (or its parts joined) that is one
/// of the mark's words in any language a keyword's letters can write, or
/// that is "zeron" ("ze-ron" as "veri-fied"). No badge lifts it: every plugin here is for Zeron,
/// so the word says nothing in a keyword and only ever serves a claim.
pub fn keyword_claim(keyword: &str) -> Option<String> {
    let parts: Vec<Vec<char>> = keyword.split(['-', ' ']).filter(|p| !p.is_empty()).map(|p| folded(p)).collect();
    let joined: Vec<char> = parts.concat();
    let whole = |text: &[char], word: &str| {
        let spelling: Vec<char> = mark_text(word).chars().collect();
        spell_end(text, 0, &spelling) == Some(text.len())
    };
    if parts.iter().chain([&joined]).any(|part| whole(part, ZERON)) {
        return Some(ZERON.to_owned());
    }
    let ascii_words = OTHER_LANGUAGES.iter().flat_map(|(_, _, words)| words.iter().map(|w| listed(w.word).0)).filter(|w| mark_text(w).is_ascii());
    MARK_WORDS.iter().copied().chain(ascii_words).find(|word| parts.iter().chain([&joined]).any(|part| whole(part, word))).map(str::to_owned)
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
            assert_eq!(mark_claim(text, false).as_deref(), claim, "{text}");
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
            assert!(mark_claim(text, false).is_some(), "{text}");
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
            assert_eq!(mark_claim(text, false), None, "{text}");
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
    fn the_mark_s_words_in_other_languages_but_never_an_ordinary_word() {
        // (text, in a name, in a description)
        for (text, name, description) in [
            ("Cliente oficial", Some("oficial"), Some("oficial")),
            ("No oficial", Some("oficial"), None),
            ("Extension officielle", Some("officielle"), Some("officielle")),
            ("Th\u{e8}me v\u{e9}rifi\u{e9}", Some("v\u{e9}rifi\u{e9}"), Some("v\u{e9}rifi\u{e9}")),
            // Folded, "vérifie" ("checks") reads as "vérifié": a name only.
            ("V\u{e9}rifie la syntaxe de vos fichiers", Some("v\u{e9}rifi\u{e9}"), None),
            ("Offizieller Client", Some("offizieller"), Some("offizieller")),
            ("Verifiziert Signaturen", Some("verifiziert"), None),
            ("Offiziell unterst\u{fc}tzt", Some("offiziell"), None),
            ("Eingaben werden gepr\u{fc}ft", Some("gepr\u{fc}ft"), None),
            ("Client ufficiale", Some("ufficiale"), Some("ufficiale")),
            ("Tema verificado", Some("verificado"), Some("verificado")),
            ("\u{41e}\u{444}\u{438}\u{446}\u{438}\u{430}\u{43b}\u{44c}\u{43d}\u{44b}\u{439} \u{43a}\u{43b}\u{438}\u{435}\u{43d}\u{442}", Some("\u{43e}\u{444}\u{438}\u{446}\u{438}\u{430}\u{43b}\u{44c}\u{43d}\u{44b}\u{439}"), Some("\u{43e}\u{444}\u{438}\u{446}\u{438}\u{430}\u{43b}\u{44c}\u{43d}\u{44b}\u{439}")),
            ("\u{41f}\u{440}\u{43e}\u{432}\u{435}\u{440}\u{435}\u{43d}\u{43e}: 5 \u{444}\u{430}\u{439}\u{43b}\u{43e}\u{432}", Some("\u{43f}\u{440}\u{43e}\u{432}\u{435}\u{440}\u{435}\u{43d}\u{43e}"), None),
            ("\u{5b98}\u{65b9}\u{63d2}\u{4ef6}", Some("\u{5b98}\u{65b9}"), Some("\u{5b98}\u{65b9}")),
            ("\u{975e}\u{5b98}\u{65b9}\u{63d2}\u{4ef6}", Some("\u{5b98}\u{65b9}"), None),
            ("\u{5df2}\u{9a8c}\u{8bc1}\u{7684}\u{7b14}\u{8bb0}", Some("\u{5df2}\u{9a8c}\u{8bc1}"), Some("\u{5df2}\u{9a8c}\u{8bc1}")),
            ("\u{516c}\u{5f0f}\u{30d7}\u{30e9}\u{30b0}\u{30a4}\u{30f3}", Some("\u{516c}\u{5f0f}"), Some("\u{516c}\u{5f0f}")),
            ("\u{691c}\u{8a3c}\u{6e08}\u{307f}\u{306e}\u{30e1}\u{30e2}", Some("\u{691c}\u{8a3c}\u{6e08}"), Some("\u{691c}\u{8a3c}\u{6e08}")),
            ("\u{ac80}\u{c99d}\u{b428} \u{d50c}\u{b7ec}\u{adf8}\u{c778}", Some("\u{ac80}\u{c99d}\u{b428}"), Some("\u{ac80}\u{c99d}\u{b428}")),
        ] {
            assert_eq!(mark_claim(text, true).as_deref(), name, "{text} (name)");
            assert_eq!(mark_claim(text, false).as_deref(), description, "{text} (description)");
        }
        // Ordinary words: never.
        for text in [
            "\u{6570}\u{5b66}\u{516c}\u{5f0f}\u{7f16}\u{8f91}\u{5668}", "\u{c218}\u{d559} \u{acf5}\u{c2dd}", "\u{8eab}\u{4efd}\u{8ba4}\u{8bc1}", "\u{8a8d}\u{8a3c}\u{30c8}\u{30fc}\u{30af}\u{30f3}",
            "\u{c778}\u{c99d} \u{d1a0}\u{d070}", "Oficina de notas", "Officier de garde", "Verifica la sintassi", "Verifica tus archivos",
            "\u{43d}\u{435}\u{43e}\u{444}\u{438}\u{446}\u{438}\u{430}\u{43b}\u{44c}\u{43d}\u{44b}\u{439} \u{43a}\u{43b}\u{438}\u{435}\u{43d}\u{442}", "Inoffizieller Client",
            "\u{7b14}\u{8bb0}\u{677f}", "\u{30e1}\u{30e2}\u{5e33}", "\u{ba54}\u{baa8}\u{c7a5}",
        ] {
            assert_eq!(mark_claim(text, true), None, "{text} (name)");
            assert_eq!(mark_claim(text, false), None, "{text} (description)");
        }
        // Beside "zeron", each is still a claim.
        for text in ["Zeron \u{516c}\u{5f0f}", "\u{acf5}\u{c2dd} Zeron", "Zeron \u{8ba4}\u{8bc1}", "\u{c778}\u{c99d} Zeron", "Zeron officiel", "Zeron verifiziert", "\u{7531} Zeron \u{5236}\u{4f5c}", "\u{6765}\u{81ea}Zeron"] {
            assert!(claims_zeron(text).is_some(), "{text}");
        }
    }

    #[test]
    fn a_description_says_the_mark_only_of_the_plugin_itself() {
        for text in [
            // 1: first word, or alone as a sentence.
            "Official Git tools.", "Notes, kept tidy. Verified.", "VERIFIED Focus board.",
            // 2: within two words of a word for the plugin.
            "A verified plugin for notes.", "The official Git extension.", "Cliente oficial para Git.",
            "\u{516c}\u{5f0f} Git \u{62e1}\u{5f35}\u{3002}", "\u{516c}\u{5f0f}\u{30d7}\u{30e9}\u{30b0}\u{30a4}\u{30f3}", "\u{acf5}\u{c2dd} Git \u{d50c}\u{b7ec}\u{adf8}\u{c778}",
            "\u{5df2}\u{8ba4}\u{8bc1}\u{63d2}\u{4ef6}", "\u{8a8d}\u{8a3c}\u{6e08}\u{307f}\u{30d7}\u{30e9}\u{30b0}\u{30a4}\u{30f3}", "A board, the official version.",
            // 3: directly after a word that says it of the subject.
            "This board is verified.", "Now official: boards for everyone.", "Fully verified and signed.", "Der Client ist offizielle Software.",
            // 4: beside Zeron or the store; no negation lifts it.
            "Boards, Zeron verified.", "Not Zeron official.", "Checked: verified by the store.",
            // Look-alikes and spacing count as anywhere.
            "Notes. V.E.R.I.F.I.E.D.", "A v3rified plugin.",
        ] {
            assert!(description_mark(text).is_some(), "{text}");
        }
        for text in [
            "Opens the official Rust documentation.", "Shows GitHub's verified badge next to commits.", "Not official, just useful.",
            "An unofficial client for Git.", "A non official client for Git.", "Cliente no oficial de Git.", "\u{975e}\u{5b98}\u{65b9}\u{5ba2}\u{6237}\u{7aef}",
            "\u{c774} \u{d50c}\u{b7ec}\u{adf8}\u{c778}\u{c740} \u{be44}\u{acf5}\u{c2dd} \u{d074}\u{b77c}\u{c774}\u{c5b8}\u{d2b8}\u{c785}\u{b2c8}\u{b2e4}",
            "Lists your verified email addresses.", "Links to the official site. A tool for boards.", "Adds a button to Zeron's built-in terminal.",
            "V\u{e9}rifie la syntaxe de vos fichiers", "Verifiziert Signaturen.", "Das Plugin verifiziert Signaturen.", "Eingaben werden gepr\u{fc}ft.",
            "\u{6570}\u{5b66}\u{516c}\u{5f0f}\u{7f16}\u{8f91}\u{5668}", "\u{c218}\u{d559} \u{acf5}\u{c2dd}", "\u{8eab}\u{4efd}\u{8ba4}\u{8bc1}", "\u{8a8d}\u{8a3c}\u{30c8}\u{30fc}\u{30af}\u{30f3}", "\u{c778}\u{c99d} \u{d1a0}\u{d070}",
        ] {
            assert_eq!(description_mark(text), None, "{text}");
        }
    }

    #[test]
    fn russian_case_forms_and_look_alikes_in_every_language() {
        // Names: any case form; look-alike letters either way.
        for name in [
            "\u{41e}\u{444}\u{438}\u{446}\u{438}\u{430}\u{43b}\u{44c}\u{43d}\u{43e}\u{433}\u{43e} \u{43a}\u{43b}\u{438}\u{435}\u{43d}\u{442}\u{430}",
            "\u{412}\u{435}\u{440}\u{438}\u{444}\u{438}\u{446}\u{438}\u{440}\u{43e}\u{432}\u{430}\u{43d}\u{43d}\u{43e}\u{433}\u{43e} \u{43f}\u{43b}\u{430}\u{433}\u{438}\u{43d}\u{430}",
            "o\u{444}\u{438}\u{446}\u{438}\u{430}\u{43b}\u{44c}\u{43d}\u{44b}\u{439} \u{43a}\u{43b}\u{438}\u{435}\u{43d}\u{442}",
            "0ficial", "\u{43e}fi\u{441}ial", "Offici3l", "Verificat0",
        ] {
            assert!(mark_claim(name, true).is_some(), "{name}");
        }
        for name in ["Oficina", "\u{41e}\u{444}\u{438}\u{446}\u{438}\u{430}\u{43b}\u{44c}\u{43d}\u{43e} \u{43d}\u{435}\u{442}", "\u{43d}\u{435}\u{43e}\u{444}\u{438}\u{446}\u{438}\u{430}\u{43b}\u{44c}\u{43d}\u{44b}\u{439}"] {
            assert_eq!(mark_claim(name, true), None, "{name}");
        }
        // Descriptions: said of the plugin, in any case form.
        for text in [
            "\u{423}\u{441}\u{442}\u{430}\u{43d}\u{43e}\u{432}\u{438}\u{442}\u{435} \u{43e}\u{444}\u{438}\u{446}\u{438}\u{430}\u{43b}\u{44c}\u{43d}\u{443}\u{44e} \u{442}\u{435}\u{43c}\u{443}.",
            "\u{414}\u{43b}\u{44f} \u{43e}\u{444}\u{438}\u{446}\u{438}\u{430}\u{43b}\u{44c}\u{43d}\u{43e}\u{433}\u{43e} \u{43f}\u{43b}\u{430}\u{433}\u{438}\u{43d}\u{430} Git.",
            "Cliente \u{43e}ficial para Git.",
        ] {
            assert!(mark_claim(text, false).is_some(), "{text}");
        }
        for text in [
            "\u{42d}\u{442}\u{43e} \u{43d}\u{435} \u{43e}\u{444}\u{438}\u{446}\u{438}\u{430}\u{43b}\u{44c}\u{43d}\u{43e}\u{435} \u{43f}\u{440}\u{438}\u{43b}\u{43e}\u{436}\u{435}\u{43d}\u{438}\u{435}.",
            "\u{41f}\u{440}\u{43e}\u{432}\u{435}\u{440}\u{435}\u{43d}\u{43d}\u{44b}\u{439} \u{441}\u{43f}\u{43e}\u{441}\u{43e}\u{431} \u{445}\u{440}\u{430}\u{43d}\u{438}\u{442}\u{44c} \u{437}\u{430}\u{43c}\u{435}\u{442}\u{43a}\u{438}.",
            "\u{41f}\u{43e}\u{43a}\u{430}\u{437}\u{44b}\u{432}\u{430}\u{435}\u{442} \u{43e}\u{444}\u{438}\u{446}\u{438}\u{430}\u{43b}\u{44c}\u{43d}\u{443}\u{44e} \u{434}\u{43e}\u{43a}\u{443}\u{43c}\u{435}\u{43d}\u{442}\u{430}\u{446}\u{438}\u{44e} Rust.",
        ] {
            assert_eq!(mark_claim(text, false), None, "{text}");
        }
    }

    #[test]
    fn the_identity_lane_s_fifth_probe() {
        for name in ["Built-in Zeron Git", "Built in Zeron", "Ships with Zeron", "Bundled with Zeron", "Included in Zeron", "Comes with Zeron", "Native to Zeron", "Default in Zeron"] {
            assert!(claims_zeron_in_name(name).is_some(), "{name}");
        }
        for name in ["\u{4f7f}\u{7528} Zeron \u{7684}\u{6280}\u{5de7}", "\u{4f7f}\u{7528}Zeron\u{7684}Git"] {
            assert_eq!(claims_zeron_in_name(name), None, "{name}");
        }
        for name in [
            "\u{41e}\u{444}\u{438}\u{446}\u{438}\u{430}\u{43b}\u{44c}\u{43d}\u{43e}\u{439}", "\u{434}\u{43b}\u{44f} \u{43e}\u{444}\u{438}\u{446}\u{438}\u{430}\u{43b}\u{44c}\u{43d}\u{44b}\u{445}",
            "\u{41e}\u{444}\u{438}\u{446}\u{438}\u{430}\u{43b}\u{44c}\u{43d}\u{443}\u{44e}", "\u{41f}\u{440}\u{43e}\u{432}\u{435}\u{440}\u{435}\u{43d}", "\u{43f}\u{440}\u{43e}\u{432}\u{435}\u{440}\u{435}\u{43d}\u{43d}\u{44b}\u{445}",
            "Verifi\u{441}ado", "0ficial", "\u{516c}\u{8a8d}\u{30d7}\u{30e9}\u{30b0}\u{30a4}\u{30f3}", "\u{acf5}\u{c778} \u{d50c}\u{b7ec}\u{adf8}\u{c778}", "\u{8ba4}\u{8bc1}\u{63d2}\u{4ef6}",
        ] {
            assert!(mark_claim(name, true).is_some(), "{name}");
        }
        for name in ["\u{6570}\u{5b66}\u{516c}\u{5f0f}", "\u{516c}\u{5f0f}\u{7f16}\u{8f91}\u{5668}", "\u{c218}\u{d559} \u{acf5}\u{c2dd}", "\u{acf5}\u{c2dd} \u{acc4}\u{c0b0}\u{ae30}"] {
            assert_eq!(mark_claim(name, true), None, "{name}");
        }
    }

    #[test]
    fn saying_a_plugin_is_part_of_zeron() {
        for text in ["Ships with Zeron.", "Included in Zeron.", "Part of Zeron.", "Comes with Zeron.", "Built into Zeron.", "Zeron-made boards.", "Zeron built.", "\u{5185}\u{7f6e}\u{4e8e}Zeron\u{3002}"] {
            assert!(claims_zeron(text).is_some(), "{text}");
        }
        for text in ["Adds a button to Zeron's built-in terminal.", "Zeron built-in terminal tools.", "Uses Zeron's made-up names.", "Zeron\u{5185}\u{7f6e}\u{7ec8}\u{7aef}\u{7684}\u{6309}\u{94ae}"] {
            assert_eq!(claims_zeron(text), None, "{text}");
        }
        for name in ["Built-in Zeron Git", "Default Git for Zeron", "Native notes for Zeron"] {
            assert!(claims_zeron_in_name(name).is_some(), "{name}");
        }
        assert_eq!(claims_zeron_in_name("Themes for Zeron"), None);
    }

    #[test]
    fn a_keyword_says_neither_the_mark_nor_zeron() {
        for (keyword, claim) in [
            ("verified", "verified"), ("veri-fied", "verified"), ("0fficial", "official"), ("of-ficial", "official"), ("oficial", "oficial"), ("verifie", "v\u{e9}rifi\u{e9}"),
            ("zeron", "zeron"), ("zeron-themes", "zeron"), ("themes zeron", "zeron"), ("zer0n", "zeron"), ("2eron", "zeron"), ("ze-ron", "zeron"),
        ] {
            assert_eq!(keyword_claim(keyword).as_deref(), Some(claim), "{keyword}");
        }
        for keyword in ["themes", "zero", "heron", "verify", "officer", "zeronaut", "notes board"] {
            assert_eq!(keyword_claim(keyword), None, "{keyword}");
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
        let others: BTreeSet<char> = OTHER_LANGUAGES.iter().flat_map(|(_, _, words)| words.iter()).flat_map(|w| mark_text(w.word).chars().collect::<Vec<_>>()).filter(char::is_ascii_lowercase).collect();
        let letters: BTreeSet<char> = letters.union(&others).copied().collect();
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
        // (The usernames' digit fold: 2 for z.)
        add('z', "2");
        let escape = |c: char| if c.is_ascii_alphanumeric() { c.to_string() } else { format!("\\u{{{:x}}}", c as u32) };
        let rows: Vec<String> = table.iter().map(|(l, set)| format!("    ('{l}', \"{}\"),", set.iter().map(|c| escape(*c)).collect::<String>())).collect();
        format!(
            "//! Generated by `mark.rs` (`lookalikes_are_current`) from Unicode's\n//! confusables.txt: do not edit. For each letter of the mark's words and\n//! Zeron's phrases, the characters that pass for it, folded as `mark_text` folds them.\n\npub const SOURCE: &str = \"UTS #39 confusables.txt\";\npub const SOURCE_VERSION: &str = \"{version}\";\npub const SOURCE_SHA256: &str = \"{sha256}\";\n\npub const LOOKALIKES: &[(char, &str)] = &[\n{}\n];\n",
            rows.join("\n")
        )
    }
}
