//! A plugin's store listing: the manifest fields that describe it in the
//! store (`categories`, `keywords`, `homepage`, `repository`, `license`,
//! `screenshots`, `engines.zeron`, with `description` and `icon`), its
//! `README.md` (its page) and `CHANGELOG.md` (each version's notes), and
//! the rules a plugin meets to be published. A template's ([`check_template`],
//! contract "Templates in the store") are the same where they can be, with
//! its own: a `preview.png` for its cover, its screenshots under
//! `screenshots/`, a `zeron_version`, none of a plugin's asks.
//!
//! Every field is optional for a plugin that's only sideloaded: one that has
//! none of them, or a malformed one, installs and runs as before
//! (`Manifest::problem` never looks at them). Only `engines.zeron` matters
//! outside the store: a device refuses a plugin whose range leaves its
//! Zeron out (`version::accepts`).
//!
//! The rules ([`check`]) are the store's, run on the device so the person
//! sees every problem before uploading; the edge runs the same ones on what
//! it receives (`docs/plugin-package-vectors.json` holds the shared cases).
//! Each problem carries a stable `rule` word (the edge's error words where
//! they're the same thing), the `field` or `path` it's about, and English
//! words for whoever reads it raw.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use serde_json::Value;

use super::manifest::{self, Manifest};
use super::mark::Item;
use super::package;
use super::version::{Range, Version};

pub const README: &str = "README.md";
pub const CHANGELOG: &str = "CHANGELOG.md";

pub const MAX_CATEGORIES: usize = 2;
pub const MAX_KEYWORDS: usize = 8;
pub const MAX_KEYWORD_LEN: usize = 32;
pub const MAX_SCREENSHOTS: usize = 8;
pub const MAX_URL_LEN: usize = 2048;
pub const MAX_LICENSE_LEN: usize = 64;
pub const MAX_NAME_LEN: usize = 64;
pub const MAX_DESCRIPTION_LEN: usize = 300;
/// An icon is an SVG at most this big (as `scan::icon_svg` reads one).
pub const MAX_ICON_BYTES: usize = 32 * 1024;
pub const MAX_README_BYTES: usize = 512 * 1024;
pub const MAX_CHANGELOG_BYTES: usize = 512 * 1024;
/// The manifest's canonical JSON, without its signature.
pub const MAX_MANIFEST_BYTES: usize = 256 * 1024;
/// What the store takes, as uploaded: the `.zeron-plugin` file (amendment
/// 34). Sideloaded packages keep the engine's larger limit.
pub const MAX_PUBLISH_BYTES: usize = 20 * 1024 * 1024;
/// The manifest as it's written in the package (its signature included).
pub const MAX_MANIFEST_JSON_BYTES: usize = 1024 * 1024;
/// JSON values in that manifest: every value counted once (the manifest
/// itself, each member's value, each array element), keys not.
pub const MAX_MANIFEST_VALUES: usize = 100_000;

/// The JSON values in `value`, itself included (keys aren't values).
pub fn count_values(value: &Value) -> usize {
    1 + match value {
        Value::Array(items) => items.iter().map(count_values).sum(),
        Value::Object(map) => map.values().map(count_values).sum(),
        _ => 0,
    }
}
/// A version's notes (its CHANGELOG.md section).
pub const MAX_NOTES_BYTES: usize = 64 * 1024;
/// Screenshots are PNG or JPEG pictures.
pub use zeron_plugin_guard::template::SCREENSHOT_TYPES;

/// The store's categories: a fixed list (the edge serves the same one at
/// `/store/v1/categories`), `(id, name)`.
pub const CATEGORIES: &[(&str, &str)] = &[
    ("agents", "Agents"),
    ("appearance", "Appearance"),
    ("chat", "Chat"),
    ("developer-tools", "Developer tools"),
    ("git", "Git"),
    ("integrations", "Integrations"),
    ("productivity", "Productivity"),
    ("utilities", "Utilities"),
];

/// Ids nobody publishes: Zeron's own plugins'.
pub const BUILTIN_IDS: &[&str] = &["git", "github", "parallax-scenes"];

/// The store site's own pages beside plugin pages (`/plugins/<id>`) and
/// templates', and the two prefixes' own names: an item with one of these ids
/// would be shadowed there, so the store never lists one. Zeron runs a plugin with one of these ids as any other (one
/// made before they were kept still loads); only publishing is refused.
/// (Some are usernames the edge keeps too; they're kept here all the same,
/// so the site's pages never rest on another list.)
pub const SITE_IDS: &[&str] = &[
    "all", "new", "search", "browse", "discover", "featured", "popular", "updated", "category", "categories",
    "keyword", "keywords", "tags", "publishing", "publish", "guide", "docs", "help", "about", "account", "admin",
    "api", "install", "me", "settings", "report", "reports", "review", "queue", "audit", "terms", "privacy",
    "sitemap", "feed", "static", "assets", "plugins", "templates",
];

/// The stable words a problem is reported by.
pub mod rule {
    /// The manifest is one Zeron won't run (`Manifest::problem`).
    pub const BAD_PACKAGE: &str = "bad_package";
    pub const ID_RESERVED: &str = "id_reserved";
    pub const NOT_SEMVER: &str = "not_semver";
    pub const MISSING_README: &str = "missing_readme";
    /// A field the store needs is missing (`field` says which).
    pub const MISSING_LISTING: &str = "missing_listing";
    /// A listing field is there but wrong (`field` says which).
    pub const BAD_LISTING: &str = "bad_listing";
    /// (Worth knowing alone.) What a template's folder has and its package
    /// doesn't carry: a file (`path`), or a member of its manifest (`word`).
    pub const LEFT_OUT: &str = "left_out";
    /// (Worth knowing alone.) A template's file (`path`) that's sent and never
    /// used: where it is, Zeron's own files have a folder, or a file.
    pub const NOT_USED: &str = "not_used";
    pub const TOO_LARGE: &str = "too_large";
    /// A template carrying one of the files no template replaces
    /// (`template_packages::never_templated`); `path` says which.
    pub const PROTECTED_FILE: &str = "protected_file";
    // Warnings: shown, never blocking.
    /// A file that isn't text or a picture (a binary, bytecode): the
    /// reviewer looks closely at these.
    pub const BINARY_FILE: &str = "binary_file";
    /// `CHANGELOG.md` has no section for this version.
    pub const NO_NOTES: &str = "no_notes";
}

/// Why a name, description or keyword is `bad_listing`, for an app that
/// says it in its own words (the device's check and the edge's answer
/// alike): a closed list.
pub mod cause {
    /// Longer than its limit (a name, a description).
    pub const TOO_LONG: &str = "too_long";
    /// A description of more than one line.
    pub const NOT_ONE_LINE: &str = "not_one_line";
    /// A character drawn as nothing; `word` is its code point (`U+200B`).
    pub const UNSEEN_CHAR: &str = "unseen_char";
    /// The store's mark in words, any language or spelling; `word` is
    /// where, as written (a keyword: the keyword).
    pub const MARK_WORD: &str = "mark_word";
    /// A check mark (`word`, as written).
    pub const MARK_GLYPH: &str = "mark_glyph";
    /// A keyword (`word`) not of lowercase letters, digits, dashes and
    /// single spaces, or too long.
    pub const KEYWORD_SHAPE: &str = "keyword_shape";
    /// More keywords than the store takes.
    pub const TOO_MANY: &str = "too_many";
    /// A keyword (`word`) given twice.
    pub const REPEATED: &str = "repeated";
    pub const ALL: &[&str] = &[TOO_LONG, NOT_ONE_LINE, UNSEEN_CHAR, MARK_WORD, MARK_GLYPH, KEYWORD_SHAPE, TOO_MANY, REPEATED];
}

/// A [`Problem::word`] is at most this many characters.
pub const MAX_PROBLEM_WORD_CHARS: usize = 64;

/// One thing in the way of publishing (or, as a warning, worth knowing).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Problem {
    pub rule: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The line of `path` it's about (1 the first), when it's one line's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    /// Why, for a name, description or keyword (one of [`cause::ALL`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cause: Option<&'static str>,
    /// The text it's about, as written ([`problem_word`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub word: Option<String>,
    pub words: String,
}

/// `text` as a [`Problem::word`]: without control or format characters
/// nor any other drawn as nothing (Default_Ignorable_Code_Point), at most
/// [`MAX_PROBLEM_WORD_CHARS`] characters.
pub fn problem_word(text: &str) -> String {
    use icu_properties::props::{BinaryProperty, DefaultIgnorableCodePoint};
    text.chars().filter(|c| !unseen_char(*c) && !DefaultIgnorableCodePoint::for_char(*c)).take(MAX_PROBLEM_WORD_CHARS).collect()
}

impl Problem {
    fn new(rule: &'static str, field: Option<&'static str>, words: impl Into<String>) -> Self {
        Self {
            rule,
            field,
            path: None,
            line: None,
            cause: None,
            word: None,
            words: words.into(),
        }
    }

    /// With its cause and the text it's about.
    fn because(mut self, cause: &'static str, word: Option<&str>) -> Self {
        self.cause = Some(cause);
        self.word = word.map(problem_word).filter(|w| !w.is_empty());
        self
    }

    /// A name's or description's mark claim (`claim`, in `text`).
    fn mark(self, text: &str, claim: &str, item: Item) -> Self {
        let glyph = claim.chars().count() == 1 && claim.chars().all(|c| MARK_GLYPHS.contains(&c));
        let written = super::mark::as_written_of(text, claim, item);
        self.because(if glyph { cause::MARK_GLYPH } else { cause::MARK_WORD }, Some(written.as_deref().unwrap_or(claim)))
    }

    /// An unseen character in `text`.
    fn unseen_in(self, text: &str) -> Self {
        let code = super::mark::first_unseen(text).map(|c| format!("U+{:04X}", c as u32));
        self.because(cause::UNSEEN_CHAR, code.as_deref())
    }

    fn at(
        rule: &'static str,
        field: Option<&'static str>,
        path: &str,
        words: impl Into<String>,
    ) -> Self {
        Self {
            rule,
            field,
            path: Some(path.to_owned()),
            line: None,
            cause: None,
            word: None,
            words: words.into(),
        }
    }
}

/// The listing fields of a manifest, read leniently: each as written when
/// it's the right shape, otherwise absent (and [`check`] says why).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Listing {
    pub categories: Vec<String>,
    pub keywords: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    pub screenshots: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// `engines.zeron`, as written.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub engines_zeron: Option<String>,
}

fn texts(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn text(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).map(str::to_owned)
}

impl Listing {
    pub fn of(raw: &Value) -> Self {
        Self {
            categories: texts(raw.get("categories")),
            keywords: texts(raw.get("keywords")),
            // As the store keeps them: the parsed URL's text.
            homepage: text(raw.get("homepage")).map(|u| store_link(&u).unwrap_or(u)),
            repository: text(raw.get("repository")).map(|u| store_link(&u).unwrap_or(u)),
            license: text(raw.get("license")),
            screenshots: texts(raw.get("screenshots")),
            icon: text(raw.get("icon")),
            engines_zeron: text(raw.get("engines").and_then(|e| e.get("zeron"))),
        }
    }

    /// A template's: as [`Listing::of`], with no categories or icon (a
    /// template has neither), and `engines_zeron` read from its
    /// `zeron_version`: `>=` it, the Zeron it was made from and later.
    pub fn of_template(raw: &Value) -> Self {
        let made_from = text(raw.get("zeron_version")).map(|v| v.trim().to_owned()).filter(|v| Version::parse(v).is_ok());
        Self {
            categories: Vec::new(),
            icon: None,
            engines_zeron: made_from.map(|v| format!(">={v}")),
            ..Self::of(raw)
        }
    }
}

/// The range a manifest's `engines.zeron` names: none when it names none,
/// `Err` when `engines` or its `zeron` is there but the wrong shape (a
/// device can't tell which Zerons it's for, so it isn't installed).
pub fn zeron_range(raw: &Value) -> Result<Option<String>, String> {
    match raw.get("engines") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Object(engines)) => match engines.get("zeron") {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(range)) => Ok(Some(range.clone())),
            Some(_) => Err("Its \"engines.zeron\" must be a range of versions, as text".into()),
        },
        Some(_) => Err(
            "Its \"engines\" must be an object (\"engines\": { \"zeron\": \">=0.4.0\" })".into(),
        ),
    }
}

/// Whether `id` is one the store keeps for Zeron: its own plugins', its
/// site's pages ([`SITE_IDS`]), or starting `zeron`. (The edge also keeps
/// brand and lookalike names, as usernames are kept: those it says when it
/// refuses.)
pub fn reserved_for_store(id: &str) -> bool {
    BUILTIN_IDS.contains(&id)
        || SITE_IDS.contains(&id)
        || id.starts_with("zeron")
        || manifest::check_id(id).is_err_and(|e| e == manifest::RESERVED_ID_ERROR)
}

/// A `homepage` or `repository` as the store takes one (amendment 31):
/// `https://`, at most [`MAX_URL_LEN`], no backslash, whitespace, control
/// character or `user:password@` part, and parsing as a URL (WHATWG). The
/// parsed URL's text is what's kept: `https://example.com` is kept as
/// `https://example.com/`.
pub fn store_link(url: &str) -> Option<String> {
    if !url.starts_with("https://")
        || url.len() > MAX_URL_LEN
        || url.chars().any(|c| c == '\\' || c.is_whitespace() || c.is_control())
    {
        return None;
    }
    let parsed = url::Url::parse(url).ok()?;
    let plain = parsed.scheme() == "https" && parsed.host().is_some() && parsed.username().is_empty() && parsed.password().is_none();
    plain.then(|| parsed.to_string())
}

/// What a listing's name, description or keywords may not say: the
/// store's mark (`mark.rs`).
pub use super::mark::{MARK_GLYPHS, MARK_WORDS, keyword_claim, mark_claim, mark_text};

/// Whether `text` has a character drawn as nothing, which what a listing
/// shows may not (amendment 31): a control or format character, any other
/// default-ignorable one, a variation selector but an emoji's own
/// (`mark::unseen`).
fn unseen(text: &str) -> bool {
    super::mark::unseen(text)
}

/// Whether `c` is a control or format character (Unicode Cc or Cf): a
/// newline, an escape sequence's ESC, a direction override, an invisible
/// joiner.
pub fn unseen_char(c: char) -> bool {
    use unicode_properties::{GeneralCategory, UnicodeGeneralCategory};
    matches!(c.general_category(), GeneralCategory::Control | GeneralCategory::Format)
}

/// `text` as it's shown of a package, wherever it came from: one line of at
/// most `max` characters, with none that's drawn as nothing but an emoji's
/// selector, and whatever's drawn as space (a line's or paragraph's end, a
/// blank Braille cell) one space between words. (What's shown alone: what's
/// signed or compared is as it is.)
pub fn shown(text: &str, max: usize) -> String {
    let mut buf = [0; 4];
    let mut out = String::new();
    let mut len = 0;
    let mut space = false;
    for c in text.chars() {
        if c.is_control() || c.is_whitespace() || c == '\u{2800}' {
            space = !out.is_empty();
        } else if !unseen_char(c) && !zeron_plugin_guard::template::unseen(c.encode_utf8(&mut buf)) {
            if len + usize::from(space) >= max {
                break;
            }
            if std::mem::take(&mut space) {
                out.push(' ');
                len += 1;
            }
            out.push(c);
            len += 1;
        }
    }
    out
}

/// `text` for a terminal or a CI log, as one line: each control or format
/// character written out (`\u{1b}`), so words from a plugin or the store
/// can't start a line of their own, colour one, or reorder it.
pub fn escaped(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if unseen_char(c) {
            out.extend(c.escape_unicode());
        } else {
            out.push(c);
        }
    }
    out
}

/// Two of a package's paths that a disk folding names (macOS's, Windows')
/// makes meet ([`clashes`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Clash<'a> {
    /// One file there: `path`, met after `other`.
    SameFile { path: &'a str, other: &'a str },
    /// `file` is a file where `inside` needs a folder of that name; `path`
    /// is whichever of them was met second.
    Folder { file: &'a str, inside: &'a str, path: &'a str },
}

impl Clash<'_> {
    /// The path met second: the one that clashes with what came before.
    pub fn path(&self) -> &str {
        match self {
            Clash::SameFile { path, .. } | Clash::Folder { path, .. } => path,
        }
    }

    /// In words naming both paths.
    pub fn words(&self) -> String {
        match self {
            Clash::SameFile { path, other } => format!("{path:?} and {other:?} are one file on macOS and Windows"),
            Clash::Folder { file, inside, .. } => format!("{file:?} is a file where {inside:?} needs a folder of that name on macOS and Windows"),
        }
    }
}

/// Every clash among `paths`, in their order, as a disk that folds names
/// makes them ([`fold_path`]): two that are one file, or a file where
/// another needs a folder of that name. `taken` (a manifest's name, a file
/// the installer writes beside it) are there first, as files.
pub fn clashes<'a>(paths: impl IntoIterator<Item = &'a str>, taken: &[&'a str]) -> Vec<Clash<'a>> {
    // Each folded file: the path it was. Each folded folder: a path inside it.
    let mut files: BTreeMap<String, &'a str> = taken.iter().map(|rel| (fold_path(rel), *rel)).collect();
    let mut folders: BTreeMap<String, &'a str> = BTreeMap::new();
    let mut found = Vec::new();
    for rel in paths {
        let folded = fold_path(rel);
        if let Some(other) = files.get(&folded) {
            found.push(Clash::SameFile { path: rel, other });
            continue;
        }
        if let Some(inside) = folders.get(&folded) {
            found.push(Clash::Folder { file: rel, inside, path: rel });
            continue;
        }
        let parts: Vec<&str> = folded.split('/').collect();
        let mut clashed = false;
        for end in 1..parts.len() {
            let folder = parts[..end].join("/");
            if let Some(file) = files.get(&folder) {
                found.push(Clash::Folder { file, inside: rel, path: rel });
                clashed = true;
                break;
            }
        }
        if clashed {
            continue;
        }
        for end in 1..parts.len() {
            folders.entry(parts[..end].join("/")).or_insert(rel);
        }
        files.insert(folded, rel);
    }
    found
}

/// A path as macOS and Windows compare it: without the selectors of emoji
/// (no name is told apart by one), then Unicode NFC, then plugin-guard's
/// `fold` (case, separators, trailing dots and spaces). Two
/// paths that fold the same are one file there.
pub fn fold_path(rel: &str) -> String {
    let rel: String = rel.chars().filter(|c| !zeron_plugin_guard::path::selector(*c)).collect();
    let nfc = icu_normalizer::ComposingNormalizerBorrowed::new_nfc().normalize(&rel);
    zeron_plugin_guard::path::fold(&nfc)
}

/// A keyword: lowercase letters, digits, spaces and dashes, not starting or
/// ending with a space, 1 to [`MAX_KEYWORD_LEN`] characters.
fn keyword(k: &str) -> bool {
    !k.is_empty()
        && k.len() <= MAX_KEYWORD_LEN
        && k.trim() == k
        && !k.contains("  ")
        && k.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b' ')
}

/// Whether bytes are a screenshot the store shows: a PNG or a JPEG, by its
/// signature.
fn is_screenshot(bytes: &[u8]) -> bool {
    bytes.starts_with(b"\x89PNG\r\n\x1a\n") || bytes.starts_with(b"\xff\xd8\xff")
}

/// Whether bytes are a picture by their first bytes (PNG, JPEG, GIF, WebP).
fn is_picture(bytes: &[u8]) -> bool {
    bytes.starts_with(b"\x89PNG\r\n\x1a\n")
        || bytes.starts_with(b"\xff\xd8\xff")
        || bytes.starts_with(b"GIF87a")
        || bytes.starts_with(b"GIF89a")
        || (bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP")
}

/// Whether a file is something a reviewer reads as text, or a picture.
/// Anything else (a compiled program, bytecode, an archive) is flagged.
fn readable(rel: &str, bytes: &[u8]) -> bool {
    let compiled = [
        ".luac", ".pyc", ".class", ".wasm", ".so", ".dylib", ".dll", ".exe", ".node", ".jar",
        ".zip",
    ];
    if compiled
        .iter()
        .any(|end| rel.to_ascii_lowercase().ends_with(end))
    {
        return false;
    }
    is_picture(bytes) || (std::str::from_utf8(bytes).is_ok() && !bytes.contains(&0))
}

/// The notes `CHANGELOG.md` gives `version`: the section under the heading
/// (`#` to `######`) whose first word is that version (`1.2.0`, `v1.2.0`,
/// `[1.2.0]`; build metadata as written), up to the next heading as high or
/// higher; trimmed. None when there's no such heading, or the section is
/// empty.
pub fn changelog_notes(changelog: &str, version: &str) -> Option<String> {
    let heading = |line: &str| -> Option<(usize, String)> {
        let level = line.bytes().take_while(|b| *b == b'#').count();
        if !(1..=6).contains(&level) || !line[level..].starts_with(' ') {
            return None;
        }
        let first = line[level..].split_whitespace().next().unwrap_or("");
        let first = first
            .trim_start_matches('[')
            .split(']')
            .next()
            .unwrap_or("");
        let first = first.strip_prefix(['v', 'V']).unwrap_or(first);
        Some((level, first.to_owned()))
    };
    let mut lines = changelog.lines();
    let level = loop {
        let line = lines.next()?;
        if let Some((level, first)) = heading(line)
            && first == version
        {
            break level;
        }
    };
    let mut section = Vec::new();
    for line in lines {
        if heading(line).is_some_and(|(l, _)| l <= level) {
            break;
        }
        section.push(line);
    }
    let notes = section.join("\n").trim().to_owned();
    (!notes.is_empty()).then_some(notes)
}

/// What stands in the way of publishing `manifest` with `files` (its
/// package's, the manifest not among them), and what's worth knowing
/// (warnings): each problem once, in a stable order.
pub fn check(
    manifest: &Manifest,
    files: &BTreeMap<String, Vec<u8>>,
) -> (Vec<Problem>, Vec<Problem>) {
    use rule::*;
    let item = Item::Plugin;
    let mut problems = Vec::new();
    let mut warnings = Vec::new();
    let raw = &manifest.raw;
    if let Some(problem) = manifest.problem() {
        problems.push(Problem::new(BAD_PACKAGE, Some("manifest"), problem));
    }
    match manifest::check_id(&manifest.id) {
        Err(e) if e != manifest::RESERVED_ID_ERROR => {
            problems.push(Problem::new(BAD_PACKAGE, Some("id"), e))
        }
        _ => reserved_id(&manifest.id, &mut problems),
    }
    if let Err(e) = Version::parse(&manifest.version) {
        problems.push(Problem::new(NOT_SEMVER, Some("version"), e));
    }
    check_name(&manifest.name, item, &mut problems);
    if !manifest.entry.is_empty() && !files.contains_key(&manifest.entry) {
        problems.push(Problem::new(
            BAD_PACKAGE,
            Some("entry"),
            format!("Its entry file {} is missing", manifest.entry),
        ));
    }
    check_readme(files, None, &mut problems);
    check_description(&manifest.description, item, &mut problems);
    match raw.get("icon") {
        None | Some(Value::Null) => problems.push(Problem::new(
            MISSING_LISTING,
            Some("icon"),
            "It needs an \"icon\": an SVG file in it",
        )),
        Some(Value::String(icon)) => {
            let ok = icon.ends_with(".svg")
                && files.get(icon).is_some_and(|bytes| {
                    bytes.len() <= MAX_ICON_BYTES
                        && std::str::from_utf8(bytes)
                            .is_ok_and(|t| t.trim_start().starts_with("<svg"))
                });
            if !ok {
                problems.push(Problem::at(
                    BAD_LISTING,
                    Some("icon"),
                    icon,
                    format!(
                        "Its icon must be an SVG file in it, at most {} KB",
                        MAX_ICON_BYTES / 1024
                    ),
                ));
            }
        }
        Some(_) => problems.push(Problem::new(
            BAD_LISTING,
            Some("icon"),
            "Its \"icon\" must be the path of an SVG file in it",
        )),
    }

    // Where it's filed and found.
    match raw.get("categories") {
        None | Some(Value::Null) => {
            problems.push(Problem::new(
                MISSING_LISTING,
                Some("categories"),
                "It needs a category (\"categories\": [\"productivity\"])",
            ));
        }
        Some(Value::Array(items)) => {
            let known: Vec<&str> = CATEGORIES.iter().map(|(id, _)| *id).collect();
            let names: Vec<Option<&str>> = items.iter().map(Value::as_str).collect();
            let unique: BTreeSet<_> = names.iter().collect();
            if items.is_empty() {
                problems.push(Problem::new(
                    MISSING_LISTING,
                    Some("categories"),
                    "It needs a category",
                ));
            } else if items.len() > MAX_CATEGORIES || unique.len() != names.len() {
                problems.push(Problem::new(
                    BAD_LISTING,
                    Some("categories"),
                    format!("It's in 1 or {MAX_CATEGORIES} categories, each once"),
                ));
            } else if let Some(bad) = names.iter().find(|n| n.is_none_or(|n| !known.contains(&n))) {
                problems.push(Problem::new(
                    BAD_LISTING,
                    Some("categories"),
                    format!(
                        "\"{}\" isn't one of the store's categories ({})",
                        bad.unwrap_or("?"),
                        known.join(", ")
                    ),
                ));
            }
        }
        Some(_) => problems.push(Problem::new(
            BAD_LISTING,
            Some("categories"),
            "Its \"categories\" must be a list",
        )),
    }
    check_keywords(raw, item, &mut problems);
    check_links(raw, &mut problems);
    check_screenshots(raw, files, None, &mut problems);
    match zeron_range(raw) {
        Ok(None) => {}
        Ok(Some(range)) => {
            if let Err(e) = Range::parse(&range) {
                problems.push(Problem::new(BAD_LISTING, Some("engines"), e));
            }
        }
        Err(e) => problems.push(Problem::new(BAD_LISTING, Some("engines"), e)),
    }
    check_changelog_size(files, &mut problems);
    check_manifest_size(&manifest.unsigned_value(), &mut problems);
    // A name that could be another of its files' (a device refuses the
    // package as it reads it), in code point order.
    for rel in files.keys() {
        if super::template_packages::short_name(rel) {
            problems.push(Problem::at(BAD_PACKAGE, Some("files"), rel, format!("{rel:?} could be a Windows short name of another file (a ~ with a digit after it): rename it")));
        } else if super::template_packages::unseen(rel) {
            problems.push(Problem::at(BAD_PACKAGE, Some("files"), rel, format!("{rel:?} has a control or invisible character in its name: rename it")));
        }
    }
    // Two paths that are one file on a Mac or a Windows machine (the
    // manifest's own name among them), or a file where another needs a
    // folder of that name (amendment 42), in code point order.
    check_clashes(files, &[super::manifest::MANIFEST_FILE], &mut problems);
    check_size(files, item, &mut problems);
    worth_knowing(files, &manifest.version, &mut problems, &mut warnings);
    (problems, warnings)
}

/// What a listing is of, as a refusal's words say it.
fn noun(item: Item) -> &'static str {
    match item {
        Item::Plugin => "plugin",
        Item::Template => "template",
    }
}

/// `id_reserved` when the store keeps `id` (one namespace: a plugin's or a
/// template's alike).
fn reserved_id(id: &str, problems: &mut Vec<Problem>) {
    if reserved_for_store(id) {
        problems.push(Problem::new(
            rule::ID_RESERVED,
            Some("id"),
            if SITE_IDS.contains(&id) {
                format!("\"{id}\" is kept for the store's own pages: pick another id")
            } else {
                format!("\"{id}\" is kept for Zeron")
            },
        ));
    }
}

/// Its name: at most [`MAX_NAME_LEN`], nothing drawn as nothing, no mark.
fn check_name(name: &str, item: Item, problems: &mut Vec<Problem>) {
    use rule::*;
    if name.chars().count() > MAX_NAME_LEN {
        problems.push(Problem::new(
            BAD_LISTING,
            Some("name"),
            format!("Its name is longer than {MAX_NAME_LEN} characters"),
        ).because(cause::TOO_LONG, None));
    } else if unseen(name) {
        problems.push(Problem::new(
            BAD_LISTING,
            Some("name"),
            "Its name has a control or invisible formatting character",
        ).unseen_in(name));
    } else if let Some(claim) = super::mark::mark_claim_of(name, true, item) {
        problems.push(Problem::new(
            BAD_LISTING,
            Some("name"),
            format!("Its name says \"{claim}\": only the store marks a {} verified or official. Name it for what it does, without that word", noun(item)),
        ).mark(name, &claim, item));
    }
}

/// Its page: a `README.md` of text, at most [`MAX_README_BYTES`]; never one
/// holding `stub` (the one Zeron wrote, unchanged).
fn check_readme(files: &BTreeMap<String, Vec<u8>>, stub: Option<&str>, problems: &mut Vec<Problem>) {
    use rule::*;
    match files.get(README) {
        Some(bytes) if bytes.len() > MAX_README_BYTES => problems.push(Problem::at(
            TOO_LARGE,
            None,
            README,
            "README.md is over 512 KB",
        )),
        Some(bytes) if std::str::from_utf8(bytes).is_ok_and(|t| stub.is_some_and(|stub| t.contains(stub))) => problems.push(Problem::at(
            MISSING_README,
            None,
            README,
            "Its README.md is still the one Zeron wrote: say what it is and how it looks, for its page in the store",
        )),
        Some(bytes) if std::str::from_utf8(bytes).is_ok_and(|t| !t.trim().is_empty()) => {}
        Some(_) => problems.push(Problem::at(
            MISSING_README,
            None,
            README,
            "Its README.md is empty or isn't text",
        )),
        None => problems.push(Problem::new(
            MISSING_README,
            None,
            "It needs a README.md: its page in the store",
        )),
    }
}

/// What the store shows on its card: one line, at most
/// [`MAX_DESCRIPTION_LEN`], nothing drawn as nothing, no mark said of it.
fn check_description(description: &str, item: Item, problems: &mut Vec<Problem>) {
    use rule::*;
    let description = description.trim();
    if description.is_empty() {
        problems.push(Problem::new(
            MISSING_LISTING,
            Some("description"),
            "It needs a \"description\": one line saying what it does",
        ));
    } else if description.chars().count() > MAX_DESCRIPTION_LEN || description.contains('\n') {
        let why = if description.chars().count() > MAX_DESCRIPTION_LEN { cause::TOO_LONG } else { cause::NOT_ONE_LINE };
        problems.push(Problem::new(
            BAD_LISTING,
            Some("description"),
            format!("Its description is one line of at most {MAX_DESCRIPTION_LEN} characters"),
        ).because(why, None));
    } else if unseen(description) {
        problems.push(Problem::new(
            BAD_LISTING,
            Some("description"),
            "Its description has a control or invisible formatting character",
        ).unseen_in(description));
    } else if let Some(claim) = super::mark::mark_claim_of(description, false, item) {
        let noun = noun(item);
        problems.push(Problem::new(
            BAD_LISTING,
            Some("description"),
            format!("Its description says \"{claim}\" of the {noun} itself: only the store marks a {noun} verified or official. Say what it does instead, or say whose thing it is (\"works with the official Slack app\")"),
        ).mark(description, &claim, item));
    }
}

/// Its keywords: at most [`MAX_KEYWORDS`], each once, each a keyword, none
/// saying the mark.
fn check_keywords(raw: &Value, item: Item, problems: &mut Vec<Problem>) {
    use rule::*;
    match raw.get("keywords") {
        None | Some(Value::Null) => {}
        Some(Value::Array(items)) => {
            let words: Vec<Option<&str>> = items.iter().map(Value::as_str).collect();
            let unique: BTreeSet<_> = words.iter().collect();
            if items.len() > MAX_KEYWORDS || unique.len() != words.len() {
                let twice = words.iter().enumerate().find_map(|(at, w)| words[..at].contains(w).then_some(*w)).flatten();
                let problem = Problem::new(
                    BAD_LISTING,
                    Some("keywords"),
                    format!("It has at most {MAX_KEYWORDS} keywords, each once"),
                );
                problems.push(if items.len() > MAX_KEYWORDS { problem.because(cause::TOO_MANY, None) } else { problem.because(cause::REPEATED, twice) });
            } else if let Some(bad) = words.iter().find(|w| w.is_none_or(|w| !keyword(w))) {
                problems.push(Problem::new(
                    BAD_LISTING,
                    Some("keywords"),
                    format!(
                        "Keyword \"{}\" must be lowercase letters, digits, dashes and single spaces, at most {MAX_KEYWORD_LEN} characters",
                        bad.unwrap_or("?")
                    ),
                ).because(cause::KEYWORD_SHAPE, *bad));
            } else if let Some((keyword, claim)) = words.iter().flatten().find_map(|w| keyword_claim(w).map(|claim| (w, claim))) {
                let noun = noun(item);
                problems.push(Problem::new(
                    BAD_LISTING,
                    Some("keywords"),
                    format!("Keyword \"{keyword}\" says \"{claim}\": only the store marks a {noun} verified or official. Drop it, or use a word for what the {noun} does"),
                ).because(cause::MARK_WORD, Some(keyword)));
            }
        }
        Some(_) => problems.push(Problem::new(
            BAD_LISTING,
            Some("keywords"),
            "Its \"keywords\" must be a list",
        )),
    }
}

/// Its `homepage` and `repository` (https links) and `license` (a short
/// name).
fn check_links(raw: &Value, problems: &mut Vec<Problem>) {
    use rule::*;
    for field in ["homepage", "repository"] {
        match raw.get(field) {
            None | Some(Value::Null) => {}
            Some(Value::String(url)) if store_link(url).is_some() => {}
            Some(_) => problems.push(Problem::new(
                BAD_LISTING,
                Some(if field == "homepage" {
                    "homepage"
                } else {
                    "repository"
                }),
                format!("Its \"{field}\" must be an https:// link"),
            )),
        }
    }
    match raw.get("license") {
        None | Some(Value::Null) => {}
        Some(Value::String(l))
            if !l.trim().is_empty()
                && l.len() <= MAX_LICENSE_LEN
                && !l.chars().any(char::is_control) => {}
        Some(_) => problems.push(Problem::new(
            BAD_LISTING,
            Some("license"),
            format!(
                "Its \"license\" is a short name, like MIT (at most {MAX_LICENSE_LEN} characters)"
            ),
        )),
    }
}

/// Its screenshots: at most [`MAX_SCREENSHOTS`], each once, each a PNG or
/// JPEG in it (under `folder`, when its pictures must be there).
fn check_screenshots(raw: &Value, files: &BTreeMap<String, Vec<u8>>, folder: Option<&str>, problems: &mut Vec<Problem>) {
    use rule::*;
    match raw.get("screenshots") {
        None | Some(Value::Null) => {}
        Some(Value::Array(items)) => {
            let paths: Vec<Option<&str>> = items.iter().map(Value::as_str).collect();
            let unique: BTreeSet<_> = paths.iter().collect();
            if items.len() > MAX_SCREENSHOTS || unique.len() != paths.len() {
                problems.push(Problem::new(
                    BAD_LISTING,
                    Some("screenshots"),
                    format!("It has at most {MAX_SCREENSHOTS} screenshots, each once"),
                ));
            }
            for path in &paths {
                let Some(path) = path else {
                    problems.push(Problem::new(
                        BAD_LISTING,
                        Some("screenshots"),
                        "Each screenshot is the path of a picture in it",
                    ));
                    continue;
                };
                let typed = path.rsplit_once('.').is_some_and(|(_, ext)| {
                    SCREENSHOT_TYPES.contains(&ext.to_ascii_lowercase().as_str())
                });
                let there = package::plain_path(path)
                    && folder.is_none_or(|folder| path.strip_prefix(folder).is_some_and(|rest| rest.starts_with('/')))
                    && files.get(*path).is_some_and(|b| is_screenshot(b));
                if !typed || !there {
                    problems.push(Problem::at(
                        BAD_LISTING,
                        Some("screenshots"),
                        path,
                        match folder {
                            None => "A screenshot must be a PNG or JPEG picture in it".to_owned(),
                            Some(folder) => format!("A screenshot must be a PNG or JPEG picture in its {folder}/ folder"),
                        },
                    ));
                }
            }
        }
        Some(_) => problems.push(Problem::new(
            BAD_LISTING,
            Some("screenshots"),
            "Its \"screenshots\" must be a list",
        )),
    }
}

fn check_changelog_size(files: &BTreeMap<String, Vec<u8>>, problems: &mut Vec<Problem>) {
    if files.get(CHANGELOG).is_some_and(|log| log.len() > MAX_CHANGELOG_BYTES) {
        problems.push(Problem::at(rule::TOO_LARGE, None, CHANGELOG, "CHANGELOG.md is over 512 KB"));
    }
}

/// The manifest as written, without the signature (which the store keeps
/// apart): at most [`MAX_MANIFEST_BYTES`].
fn check_manifest_size(unsigned: &Value, problems: &mut Vec<Problem>) {
    if super::signing::canonical_json(unsigned).len() > MAX_MANIFEST_BYTES {
        problems.push(Problem::new(rule::TOO_LARGE, Some("manifest"), "Its manifest is over 256 KB"));
    }
}

/// Two of its paths that are one file on a disk folding names, or a file
/// where another needs a folder of that name; `taken` (files written beside
/// them) first.
fn check_clashes(files: &BTreeMap<String, Vec<u8>>, taken: &[&str], problems: &mut Vec<Problem>) {
    for clash in clashes(files.keys().map(String::as_str), taken) {
        problems.push(Problem::at(rule::BAD_LISTING, Some("files"), clash.path(), clash.words()));
    }
}

/// How big: at most `MAX_FILES` files and `MAX_BYTES`.
fn check_size(files: &BTreeMap<String, Vec<u8>>, item: Item, problems: &mut Vec<Problem>) {
    let total: u64 = files.values().map(|b| b.len() as u64).sum();
    if files.len() > super::files::MAX_FILES || total > super::files::MAX_BYTES {
        problems.push(Problem::new(
            rule::TOO_LARGE,
            None,
            format!("It's too big for a {} (2,000 files, 50 MB)", noun(item)),
        ));
    }
}

/// Worth knowing: files that aren't text or a picture (warnings), and
/// `version`'s notes in `CHANGELOG.md` (too long is a problem; none, a
/// warning).
fn worth_knowing(files: &BTreeMap<String, Vec<u8>>, version: &str, problems: &mut Vec<Problem>, warnings: &mut Vec<Problem>) {
    use rule::*;
    for (rel, bytes) in files {
        if !readable(rel, bytes) {
            warnings.push(Problem::at(
                BINARY_FILE,
                None,
                rel,
                "It isn't text or a picture: reviewers look closely at files like this",
            ));
        }
    }
    let notes = files
        .get(CHANGELOG)
        .filter(|log| log.len() <= MAX_CHANGELOG_BYTES)
        .map(|log| changelog_notes(&String::from_utf8_lossy(log), version));
    if let Some(Some(notes)) = &notes
        && notes.len() > MAX_NOTES_BYTES
    {
        problems.push(Problem::at(TOO_LARGE, None, CHANGELOG, "This version's changelog section is over 64 KB"));
    }
    if let Some(None) = notes {
        warnings.push(Problem::at(
            NO_NOTES,
            None,
            CHANGELOG,
            format!(
                "CHANGELOG.md has no section for {version}: this version shows without notes"
            ),
        ));
    }
}

// ── Templates ──────────────────────────────────────────────────────────────

/// A template's preview, its card's cover: a PNG at most this big.
pub const MAX_PREVIEW_BYTES: usize = 4 * 1024 * 1024;
/// A template's listing files: carried and installed, never part of its
/// interface (never loaded, never a change of its code). `screenshots/`:
/// the PNG and JPEG files under it ([`template_listing_file`]).
pub const TEMPLATE_LISTING_FILES: &[&str] = &[README, CHANGELOG, super::templates::PREVIEW, "screenshots/"];
/// What a template's manifest must have, and may have, for the store.
pub const TEMPLATE_REQUIRED: &[&str] = &["id", "name", "description", "version", "zeron_version"];
pub const TEMPLATE_OPTIONAL: &[&str] = &["keywords", "screenshots", "homepage", "repository", "license"];
/// Members a template's manifest may not have in the store: the app's own
/// marks of Zeron's templates, and a plugin's (a template asks for nothing:
/// it runs as Zeron).
pub const TEMPLATE_REFUSED_MEMBERS: &[&str] =
    &["builtin_theme", "builtin_template", "permissions", "commands", "services", "tools", "engines", "icon", "categories"];
/// The sentence the README Zeron writes into a new template holds
/// (`guides::template_readme`): a README holding it is that one, unchanged.
pub const TEMPLATE_README_STUB: &str = "This README isn't part of the interface.";

/// Which of a template's files are its listing (carried and installed, never
/// composed, no change of its code), which under `screenshots/` aren't
/// (refused in the store, never installed from a package, never composed),
/// and where its pictures are. One answer with the app's.
pub use zeron_plugin_guard::template::{SCREENSHOTS as TEMPLATE_SCREENSHOTS, listing_file as template_listing_file, stray_screenshot as template_stray_screenshot};

/// Whether PNG `bytes` are animated (an `acTL` chunk): a cover is a still.
fn animated_png(bytes: &[u8]) -> bool {
    let mut at = PNG_SIGNATURE.len();
    while let Some(head) = bytes.get(at..at + 8) {
        if &head[4..] == b"acTL" {
            return true;
        }
        let len = u32::from_be_bytes([head[0], head[1], head[2], head[3]]) as usize;
        at = at.saturating_add(12).saturating_add(len);
    }
    false
}

/// What stands in the way of publishing template `manifest` (its signed
/// manifest, as the package carries it) with `files` (every file the
/// package carries, its manifest not among them), and what's worth knowing:
/// each problem once, in a stable order. A template asks for nothing (it
/// runs as Zeron), so its rules are the listing's, its files' and its
/// preview's.
pub fn check_template(manifest: &Value, files: &BTreeMap<String, Vec<u8>>) -> (Vec<Problem>, Vec<Problem>) {
    use rule::*;
    let item = Item::Template;
    let mut problems = Vec::new();
    let mut warnings = Vec::new();
    let Some(raw) = manifest.as_object() else {
        problems.push(Problem::new(BAD_PACKAGE, Some("manifest"), "Its manifest isn't an object"));
        return (problems, warnings);
    };
    // An allow list: those it's never told apart by name first, then any
    // other, in code point order.
    for member in TEMPLATE_REFUSED_MEMBERS {
        if raw.contains_key(*member) {
            problems.push(Problem::new(
                BAD_LISTING,
                Some(member),
                format!("A template has no \"{member}\": it asks for nothing, and runs as Zeron itself"),
            ));
        }
    }
    let mut others: Vec<&str> = raw
        .keys()
        .map(String::as_str)
        .filter(|key| *key != "signature" && ![TEMPLATE_REQUIRED, TEMPLATE_OPTIONAL, TEMPLATE_REFUSED_MEMBERS].iter().any(|list| list.contains(key)))
        .collect();
    others.sort_unstable();
    for member in others {
        let mut problem = Problem::new(BAD_LISTING, Some("manifest"), format!("\"{}\" isn't something a template's manifest says in the store: leave it out", problem_word(member)));
        problem.word = Some(problem_word(member)).filter(|w| !w.is_empty());
        problems.push(problem);
    }
    let text = |key: &str| raw.get(key).and_then(Value::as_str).map(str::trim);
    let id = text("id").unwrap_or("");
    if !super::templates::valid_id(id) {
        problems.push(Problem::new(
            BAD_PACKAGE,
            Some("id"),
            format!("\"{id}\" can't be a template's id: use 2 to 40 lowercase letters, digits and dashes, starting with a letter or digit"),
        ));
    } else {
        reserved_id(id, &mut problems);
    }
    let version = text("version").unwrap_or("");
    if let Err(e) = Version::parse(version) {
        problems.push(Problem::new(NOT_SEMVER, Some("version"), e));
    }
    match text("zeron_version") {
        None | Some("") => problems.push(Problem::new(
            MISSING_LISTING,
            Some("zeron_version"),
            "It needs a \"zeron_version\": the Zeron it was made from",
        )),
        Some(made_from) => {
            if let Err(e) = Version::parse(made_from) {
                problems.push(Problem::new(NOT_SEMVER, Some("zeron_version"), e));
            }
        }
    }
    match text("name") {
        None | Some("") => problems.push(Problem::new(MISSING_LISTING, Some("name"), "It needs a \"name\"")),
        Some(name) => check_name(name, item, &mut problems),
    }
    check_readme(files, Some(TEMPLATE_README_STUB), &mut problems);
    check_description(text("description").unwrap_or(""), item, &mut problems);
    let preview = super::templates::PREVIEW;
    match files.get(preview) {
        None => problems.push(Problem::new(
            MISSING_LISTING,
            Some("preview"),
            "It needs a preview.png: its card's cover in the store",
        )),
        Some(bytes) if bytes.len() > MAX_PREVIEW_BYTES || !bytes.starts_with(PNG_SIGNATURE) || animated_png(bytes) => problems.push(Problem::at(
            BAD_LISTING,
            Some("preview"),
            preview,
            format!("Its preview.png must be a still PNG picture, at most {} MB", MAX_PREVIEW_BYTES / 1024 / 1024),
        )),
        Some(_) => {}
    }
    check_keywords(manifest, item, &mut problems);
    check_links(manifest, &mut problems);
    check_screenshots(manifest, files, Some(TEMPLATE_SCREENSHOTS), &mut problems);
    check_changelog_size(files, &mut problems);
    let mut unsigned = manifest.clone();
    if let Some(m) = unsigned.as_object_mut() {
        m.remove("signature");
    }
    check_manifest_size(&unsigned, &mut problems);
    // Its files: each one Zeron writes, none of those no template replaces.
    for rel in files.keys() {
        if super::templates::inside(std::path::Path::new("t"), rel).is_none() || super::template_packages::unseen(rel) {
            problems.push(Problem::at(BAD_PACKAGE, Some("files"), rel, format!("It has a file Zeron won't write: {rel:?}")));
        } else if super::template_packages::protected(rel) {
            problems.push(Problem::at(
                PROTECTED_FILE,
                Some("files"),
                rel,
                format!("{rel:?} is one of Zeron's own files no template replaces (the plugins and templates pages, your account, .luaurc, ds/themes/template-dark.json and template-light.json, any tokens but ds/tokens.luau): leave it out"),
            ));
        } else if super::template_packages::short_name(rel) {
            problems.push(Problem::at(
                PROTECTED_FILE,
                Some("files"),
                rel,
                format!("{rel:?} could be a Windows short name of one of Zeron's own files (a ~ with a digit after it): rename it"),
            ));
        } else if template_stray_screenshot(rel) {
            problems.push(Problem::at(BAD_LISTING, Some("files"), rel, format!("{rel:?} isn't a PNG or JPEG: {TEMPLATE_SCREENSHOTS}/ holds its pictures alone")));
        } else if rel.starts_with(TEMPLATE_SCREENSHOTS) && template_listing_file(rel) && !is_screenshot(&files[rel]) {
            problems.push(Problem::at(BAD_LISTING, Some("files"), rel, format!("{rel:?} isn't a PNG or JPEG picture, whatever its name says")));
        }
    }
    check_clashes(files, &[super::templates::MANIFEST, super::template_packages::IMPORTED], &mut problems);
    check_size(files, item, &mut problems);
    worth_knowing(files, version, &mut problems, &mut warnings);
    (problems, warnings)
}

/// A PNG's first bytes.
const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub const ICON: &[u8] = b"<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 16 16\"><circle cx=\"8\" cy=\"8\" r=\"6\"/></svg>";
    pub const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";

    /// A manifest and files that meet every rule.
    pub fn listed() -> (Value, BTreeMap<String, Vec<u8>>) {
        let manifest = serde_json::json!({
            "id": "focus-board", "name": "Focus Board", "version": "1.2.0", "entry": "init.luau",
            "description": "A board of what you're focused on.", "icon": "icon.svg",
            "categories": ["productivity"], "keywords": ["focus", "board"],
            "homepage": "https://example.com/focus", "repository": "https://github.com/example/focus",
            "license": "MIT", "screenshots": ["shots/board.png"], "engines": { "zeron": ">=0.3.0" },
        });
        let files: BTreeMap<String, Vec<u8>> = [
            ("init.luau", b"return {}".to_vec()),
            ("icon.svg", ICON.to_vec()),
            ("README.md", b"# Focus Board\n\nWhat you're on.".to_vec()),
            (
                "CHANGELOG.md",
                b"# Changes\n\n## 1.2.0\n\n- Boards.\n\n## 1.1.0\n\n- First.".to_vec(),
            ),
            ("shots/board.png", PNG.to_vec()),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v))
        .collect();
        (manifest, files)
    }

    fn rules(
        manifest: &Value,
        files: &BTreeMap<String, Vec<u8>>,
    ) -> Vec<(&'static str, Option<&'static str>)> {
        let (problems, _) = check(&Manifest::from_value(manifest.clone()).unwrap(), files);
        problems.iter().map(|p| (p.rule, p.field)).collect()
    }

    #[test]
    fn a_listed_plugin_meets_every_rule() {
        let (manifest, files) = listed();
        let (problems, warnings) = check(&Manifest::from_value(manifest.clone()).unwrap(), &files);
        assert_eq!((problems, warnings), (vec![], vec![]));
        let listing = Listing::of(&manifest);
        assert_eq!(listing.categories, vec!["productivity"]);
        assert_eq!(listing.engines_zeron.as_deref(), Some(">=0.3.0"));
    }

    /// A name that could be another file's: of a plugin's and of a
    /// template's, the template's short names as its protected files are.
    #[test]
    fn a_path_that_could_be_another_s_is_refused() {
        let (manifest, mut files) = listed();
        let at = |problems: Vec<Problem>| -> Vec<(&'static str, Option<&'static str>, Option<String>)> {
            problems.into_iter().filter(|p| p.field == Some("files")).map(|p| (p.rule, p.field, p.path)).collect()
        };
        let odd = ["ui/BADGE~1.LUA", "in\u{200c}it.luau"];
        for rel in ["ui/a~b.luau", odd[0], odd[1]] {
            files.insert(rel.into(), b"return 1".to_vec());
        }
        let refused = |rule| odd.map(|rel| (rule, Some("files"), Some(rel.to_owned())));
        let (problems, _) = check(&Manifest::from_value(manifest).unwrap(), &files);
        let mut expected = refused(rule::BAD_PACKAGE);
        expected.sort_by(|a, b| a.2.cmp(&b.2));
        assert_eq!(at(problems), expected);
        let (problems, _) = check_template(&serde_json::json!({ "id": "midnight" }), &files);
        assert_eq!(at(problems), [(rule::BAD_PACKAGE, Some("files"), Some(odd[1].to_owned())), (rule::PROTECTED_FILE, Some("files"), Some(odd[0].to_owned()))]);
    }

    #[test]
    fn a_name_description_or_keyword_refused_says_why_and_where() {
        use serde_json::json;
        let (base, files) = listed();
        let first = |key: &str, value: Value| {
            let mut m = base.clone();
            m[key] = value;
            let (problems, _) = check(&Manifest::from_value(m).unwrap(), &files);
            let p = problems.into_iter().next().unwrap();
            (p.rule, p.field, p.cause, p.word)
        };
        let bad = |field, cause, word: Option<&str>| (rule::BAD_LISTING, Some(field), Some(cause), word.map(str::to_owned));
        assert_eq!(first("name", json!("Verified Focus")), bad("name", cause::MARK_WORD, Some("Verified")));
        assert_eq!(first("name", json!("Focus \u{2705}")), bad("name", cause::MARK_GLYPH, Some("\u{2705}")));
        assert_eq!(first("name", json!("n".repeat(65))), bad("name", cause::TOO_LONG, None));
        assert_eq!(first("name", json!("Focus\u{200b}Board")), bad("name", cause::UNSEEN_CHAR, Some("U+200B")));
        assert_eq!(first("description", json!("a\nb")), bad("description", cause::NOT_ONE_LINE, None));
        assert_eq!(first("description", json!("Official Git tools.")), bad("description", cause::MARK_WORD, Some("Official")));
        assert_eq!(first("keywords", json!(["a", "b", "a"])), bad("keywords", cause::REPEATED, Some("a")));
        assert_eq!(first("keywords", json!(["a", "b", "c", "d", "e", "f", "g", "h", "i"])), bad("keywords", cause::TOO_MANY, None));
        assert_eq!(first("keywords", json!(["Focus"])), bad("keywords", cause::KEYWORD_SHAPE, Some("Focus")));
        assert_eq!(first("keywords", json!(["timer\u{fe0f}"])), bad("keywords", cause::KEYWORD_SHAPE, Some("timer")));
        assert_eq!(first("keywords", json!(["veri-fied"])), bad("keywords", cause::MARK_WORD, Some("veri-fied")));
        // Other fields' problems have no cause.
        assert_eq!(first("license", json!("")).2, None);
        // The word is bounded.
        assert_eq!(problem_word(&format!("a\u{202e}{}", "x".repeat(100))), format!("a{}", "x".repeat(63)));
    }

    #[test]
    fn the_site_s_pages_are_never_a_listed_plugin_s_id_but_still_run() {
        let (base, files) = listed();
        for id in SITE_IDS {
            // Each a valid id Zeron runs and nothing else already kept (but
            // `templates`, which no plugin ever had; a template could).
            assert_eq!(manifest::check_id(id).is_ok(), *id != "templates", "{id} still runs");
            assert!(super::super::templates::valid_id(id), "{id}");
            assert!(!BUILTIN_IDS.contains(id) && !id.starts_with("zeron") && (*id == "templates" || !manifest::RESERVED_IDS.contains(id)), "{id}");
            let mut m = base.clone();
            m["id"] = serde_json::json!(id);
            assert_eq!(rules(&m, &files), vec![(rule::ID_RESERVED, Some("id"))], "{id}");
        }
        // Only the words themselves.
        for id in ["feed-reader", "allergies", "news-feed"] {
            assert!(!reserved_for_store(id), "{id}");
        }
    }

    #[test]
    fn a_sideloaded_plugin_needs_none_of_it() {
        // No listing fields at all, or malformed ones: still a plugin Zeron runs.
        let bare =
            serde_json::json!({ "id": "focus", "name": "F", "version": "1", "entry": "init.luau" });
        assert_eq!(Manifest::from_value(bare.clone()).unwrap().problem(), None);
        let mut odd = bare.clone();
        for (k, v) in [
            ("categories", serde_json::json!("x")),
            ("keywords", serde_json::json!(7)),
            ("homepage", serde_json::json!(["x"])),
            ("screenshots", serde_json::json!({})),
        ] {
            odd[k] = v;
        }
        assert_eq!(Manifest::from_value(odd.clone()).unwrap().problem(), None);
        assert_eq!(Listing::of(&odd), Listing::default());
        // But the store needs them.
        let files: BTreeMap<String, Vec<u8>> =
            [("init.luau".to_owned(), b"return 1".to_vec())].into();
        assert_eq!(
            rules(&bare, &files),
            vec![
                ("not_semver", Some("version")),
                ("missing_readme", None),
                ("missing_listing", Some("description")),
                ("missing_listing", Some("icon")),
                ("missing_listing", Some("categories")),
            ]
        );
    }

    #[test]
    fn each_listing_rule() {
        let (base, files) = listed();
        let with = |key: &str, value: Value| {
            let mut m = base.clone();
            m[key] = value;
            rules(&m, &files)
        };
        let bad = |field| vec![("bad_listing", Some(field))];
        assert_eq!(
            with("categories", serde_json::json!(["games"])),
            bad("categories")
        );
        assert_eq!(
            with("categories", serde_json::json!(["git", "chat", "agents"])),
            bad("categories")
        );
        assert_eq!(
            with("categories", serde_json::json!(["git", "git"])),
            bad("categories")
        );
        assert_eq!(
            with("categories", serde_json::json!([])),
            vec![("missing_listing", Some("categories"))]
        );
        assert_eq!(
            with("keywords", serde_json::json!(["Focus"])),
            bad("keywords")
        );
        assert_eq!(
            with(
                "keywords",
                serde_json::json!(["a", "b", "c", "d", "e", "f", "g", "h", "i"])
            ),
            bad("keywords")
        );
        assert_eq!(
            with("keywords", serde_json::json!(["code review", "git-flow"])),
            vec![]
        );
        assert_eq!(
            with("homepage", serde_json::json!("http://example.com")),
            bad("homepage")
        );
        assert_eq!(
            with("repository", serde_json::json!("https://exa mple.com")),
            bad("repository")
        );
        assert_eq!(with("license", serde_json::json!("")), bad("license"));
        assert_eq!(
            with("screenshots", serde_json::json!(["shots/missing.png"])),
            bad("screenshots")
        );
        assert_eq!(
            with("screenshots", serde_json::json!(["init.luau"])),
            bad("screenshots")
        );
        assert_eq!(
            with("screenshots", serde_json::json!(["../x.png"])),
            bad("screenshots")
        );
        assert_eq!(
            with("engines", serde_json::json!({ "zeron": "0.4" })),
            bad("engines")
        );
        assert_eq!(
            with("engines", serde_json::json!(">=0.4.0")),
            bad("engines")
        );
        assert_eq!(with("icon", serde_json::json!("README.md")), bad("icon"));
        assert_eq!(
            with("description", serde_json::json!("a\nb")),
            bad("description")
        );
        assert_eq!(
            with("version", serde_json::json!("1.2")),
            vec![("not_semver", Some("version"))]
        );
        assert_eq!(
            with("id", serde_json::json!("zeron-tools")),
            vec![("id_reserved", Some("id"))]
        );
        assert_eq!(
            with("id", serde_json::json!("github")),
            vec![("id_reserved", Some("id"))]
        );
        assert_eq!(
            with("id", serde_json::json!("templates")),
            vec![("id_reserved", Some("id"))]
        );
        assert_eq!(
            with("permissions", serde_json::json!(["root"])),
            vec![("bad_package", Some("manifest"))]
        );
        let mut no_readme = files.clone();
        no_readme.remove(README);
        assert_eq!(rules(&base, &no_readme), vec![("missing_readme", None)]);
    }

    /// Amendment 31: what a reader would see differently from what's
    /// there is refused.
    #[test]
    fn what_a_listing_shows_is_what_s_there() {
        let (base, files) = listed();
        let with = |key: &str, value: Value| {
            let mut m = base.clone();
            m[key] = value;
            rules(&m, &files)
        };
        let bad = |field| vec![("bad_listing", Some(field))];
        assert_eq!(with("name", serde_json::json!("Focus\u{202e}draoB")), bad("name"));
        assert_eq!(with("name", serde_json::json!("Focus\u{200b}Board")), bad("name"));
        assert_eq!(with("description", serde_json::json!("A board\u{2066}.")), bad("description"));
        for (written, kept) in [
            ("https://example.com", "https://example.com/"),
            ("https://Example.com/x/../y", "https://example.com/y"),
            ("https://github.com/a/b?x=1#y", "https://github.com/a/b?x=1#y"),
        ] {
            assert_eq!(with("homepage", serde_json::json!(written)), vec![], "{written}");
            assert_eq!(store_link(written).as_deref(), Some(kept));
        }
        for refused in ["https:\\\\evil.com", "https://user:pw@example.com/", "https://user@example.com/", "https://exa mple.com/", "http://example.com/", "https://example.com/\u{7}"] {
            assert_eq!(with("homepage", serde_json::json!(refused)), bad("homepage"), "{refused}");
        }
        // Two paths that are one file on a Mac or a Windows machine.
        for (a, b) in [("README.md", "readme.md"), ("caf\u{e9}.txt", "cafe\u{301}.txt"), ("shots/a.png", "Shots/A.png"), ("x.txt", "x.txt.")] {
            let mut two = files.clone();
            two.insert(a.into(), b"1".to_vec());
            two.insert(b.into(), b"2".to_vec());
            let problems = check(&Manifest::from_value(base.clone()).unwrap(), &two).0;
            assert!(problems.iter().any(|p| p.field == Some("files")), "{a} {b}: {problems:?}");
        }
        let mut manifest_too = files.clone();
        manifest_too.insert("ZERON-PLUGIN.JSON".into(), b"{}".to_vec());
        assert_eq!(rules(&base, &manifest_too), bad("files"));
        // A file where another needs a folder of that name (amendment 42),
        // either order, the manifest's name too; the path met second named.
        for (a, b, at) in [("ui", "UI/x.luau", "ui"), ("Ui", "ui/x.luau", "ui/x.luau"), ("ZERON-PLUGIN.JSON/x.luau", "init2.luau", "ZERON-PLUGIN.JSON/x.luau")] {
            let mut two = files.clone();
            two.insert(a.into(), b"1".to_vec());
            two.insert(b.into(), b"2".to_vec());
            let problems = check(&Manifest::from_value(base.clone()).unwrap(), &two).0;
            assert_eq!(problems.iter().map(|p| (p.rule, p.field, p.path.as_deref())).collect::<Vec<_>>(), [("bad_listing", Some("files"), Some(at))], "{a} {b}");
            assert!(problems[0].words.contains("needs a folder of that name"), "{}", problems[0].words);
        }
        // A path that clashes records nothing: later paths are held only to
        // what came before them without a problem (in code point order).
        for (paths, clashing) in [
            (["UI/a", "Ui", "ui/b"], &["Ui"][..]),
            (["cafe\u{301}/a", "caf\u{e9}", "caf\u{e9}/b"], &["caf\u{e9}"]),
            (["A", "A/B", "a/b/c"], &["A/B", "a/b/c"]),
            (["A", "a", "a/x"], &["a", "a/x"]),
        ] {
            let found: Vec<String> = clashes(paths, &[]).iter().map(|c| c.path().to_owned()).collect();
            assert_eq!(found, clashing, "{paths:?}");
        }
        // Names that only look alike are anyone's.
        let mut alike = files.clone();
        for rel in ["ui/a.luau", "ui2", "uix/b.luau"] {
            alike.insert(rel.into(), b"1".to_vec());
        }
        assert_eq!(rules(&base, &alike), vec![]);
        // Sizes.
        let mut big = base.clone();
        big["x-notes"] = serde_json::json!("x".repeat(MAX_MANIFEST_BYTES));
        assert_eq!(rules(&big, &files), vec![("too_large", Some("manifest"))]);
        let mut log = files.clone();
        log.insert(CHANGELOG.into(), vec![b'x'; MAX_CHANGELOG_BYTES + 1]);
        assert_eq!(rules(&base, &log), vec![("too_large", None)]);
    }

    #[test]
    fn warnings_never_block() {
        let (manifest, mut files) = listed();
        files.insert("bin/tool.wasm".into(), b"\0asm".to_vec());
        files.insert("lib/x.luac".into(), b"text but bytecode".to_vec());
        files.insert(CHANGELOG.into(), b"## 1.1.0\n\n- First.".to_vec());
        let (problems, warnings) = check(&Manifest::from_value(manifest).unwrap(), &files);
        assert!(problems.is_empty());
        let warned: Vec<(&str, Option<&str>)> = warnings
            .iter()
            .map(|w| (w.rule, w.path.as_deref()))
            .collect();
        assert_eq!(
            warned,
            vec![
                ("binary_file", Some("bin/tool.wasm")),
                ("binary_file", Some("lib/x.luac")),
                ("no_notes", Some("CHANGELOG.md"))
            ]
        );
    }

    #[test]
    fn a_version_s_notes_come_from_its_changelog_section() {
        let log = "# Changelog\n\n## [1.2.0] - 2026-11-01\n\n### Added\n- Boards.\n\n## v1.1.0\n- First.\n";
        assert_eq!(
            changelog_notes(log, "1.2.0").as_deref(),
            Some("### Added\n- Boards.")
        );
        assert_eq!(changelog_notes(log, "1.1.0").as_deref(), Some("- First."));
        assert_eq!(changelog_notes(log, "1.0.0"), None);
        assert_eq!(
            changelog_notes("## 1.0.0\n\n## 0.9.0\n- x", "1.0.0"),
            None,
            "an empty section"
        );
        assert_eq!(
            changelog_notes("##1.0.0\n- x", "1.0.0"),
            None,
            "a heading has a space"
        );
    }

    #[test]
    fn engines_zeron_shapes() {
        assert_eq!(zeron_range(&serde_json::json!({})), Ok(None));
        assert_eq!(
            zeron_range(&serde_json::json!({ "engines": { "node": ">=18" } })),
            Ok(None)
        );
        assert_eq!(
            zeron_range(&serde_json::json!({ "engines": { "zeron": "^0.4.0" } })),
            Ok(Some("^0.4.0".into()))
        );
        assert!(zeron_range(&serde_json::json!({ "engines": { "zeron": 4 } })).is_err());
        assert!(zeron_range(&serde_json::json!({ "engines": [] })).is_err());
    }

    #[test]
    fn a_name_or_description_claiming_the_store_s_mark_is_refused() {
        for (text, claim) in [
            ("Verified Focus", Some("verified")),
            ("The OFFICIAL git plugin", Some("official")),
            ("verified_tools", Some("verified")),
            ("Focus \u{2713}", Some("\u{2713}")),
            ("Focus \u{2705} board", Some("\u{2705}")),
            ("\u{1d42f}\u{1d41e}\u{1d42b}\u{1d422}\u{1d41f}\u{1d422}\u{1d41e}\u{1d41d} Focus", Some("verified")),
            ("\u{ff36}\u{ff25}\u{ff32}\u{ff29}\u{ff26}\u{ff29}\u{ff25}\u{ff24}", Some("verified")),
            ("An unofficial client", None),
            ("Unverified links, flagged", None),
            ("Officially fun", None),
            ("V\u{e9}rified", Some("verified")),
            ("VER\u{130}F\u{130}ED", Some("verified")),
            ("Focus Board", None),
        ] {
            assert_eq!(mark_claim(text, false).as_deref(), claim, "{text}");
        }
        let (mut m, files) = listed();
        m["name"] = serde_json::json!("Focus \u{2714}");
        m["description"] = serde_json::json!("The official plugin.");
        let (problems, _) = check(&Manifest::from_value(m).unwrap(), &files);
        let said: Vec<(&str, Option<&str>)> = problems.iter().map(|p| (p.rule, p.field)).collect();
        assert_eq!(said, [(rule::BAD_LISTING, Some("name")), (rule::BAD_LISTING, Some("description"))]);
        assert!(problems[1].words.contains("\"official\""), "{}", problems[1].words);
    }
}
