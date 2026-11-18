//! A plugin's store listing: the manifest fields that describe it in the
//! store (`categories`, `keywords`, `homepage`, `repository`, `license`,
//! `screenshots`, `engines.zeron`, with `description` and `icon`), its
//! `README.md` (its page) and `CHANGELOG.md` (each version's notes), and
//! the rules a plugin meets to be published.
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
pub const SCREENSHOT_TYPES: &[&str] = &["png", "jpg", "jpeg"];

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

/// The store site's own pages beside plugin pages (`/plugins/<id>`): a
/// plugin with one of these ids would be shadowed there, so the store never
/// lists one. Zeron runs a plugin with one of these ids as any other (one
/// made before they were kept still loads); only publishing is refused.
/// (Some are usernames the edge keeps too; they're kept here all the same,
/// so the site's pages never rest on another list.)
pub const SITE_IDS: &[&str] = &[
    "all", "new", "search", "browse", "discover", "featured", "popular", "updated", "category", "categories",
    "keyword", "keywords", "tags", "publishing", "publish", "guide", "docs", "help", "about", "account", "admin",
    "api", "install", "me", "settings", "report", "reports", "review", "queue", "audit", "terms", "privacy",
    "sitemap", "feed", "static", "assets",
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
    pub const TOO_LARGE: &str = "too_large";
    // Warnings: shown, never blocking.
    /// A file that isn't text or a picture (a binary, bytecode): the
    /// reviewer looks closely at these.
    pub const BINARY_FILE: &str = "binary_file";
    /// `CHANGELOG.md` has no section for this version.
    pub const NO_NOTES: &str = "no_notes";
    /// A name or description says the plugin is Zeron's own (a warning
    /// here: the store refuses it unless the publisher is Zeron).
    pub const CLAIMS_ZERON: &str = "claims_zeron";
}

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
    pub words: String,
}

impl Problem {
    fn new(rule: &'static str, field: Option<&'static str>, words: impl Into<String>) -> Self {
        Self {
            rule,
            field,
            path: None,
            line: None,
            words: words.into(),
        }
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

/// What a listing's name or description may not say: the store's mark,
/// or that it's Zeron's own (`mark.rs`).
pub use super::mark::{MARK_GLYPHS, MARK_WORDS, claims_zeron, claims_zeron_in_name, keyword_claim, mark_claim, mark_text};

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

/// A path as macOS and Windows compare it: Unicode NFC, then
/// plugin-guard's `fold` (case, separators, trailing dots and spaces). Two
/// paths that fold the same are one file there.
pub fn fold_path(rel: &str) -> String {
    let nfc = icu_normalizer::ComposingNormalizerBorrowed::new_nfc().normalize(rel);
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
        _ if reserved_for_store(&manifest.id) => {
            problems.push(Problem::new(
                ID_RESERVED,
                Some("id"),
                if SITE_IDS.contains(&manifest.id.as_str()) {
                    format!("\"{}\" is kept for the store's own pages: pick another id", manifest.id)
                } else {
                    format!("\"{}\" is kept for Zeron", manifest.id)
                },
            ));
        }
        _ => {}
    }
    if let Err(e) = Version::parse(&manifest.version) {
        problems.push(Problem::new(NOT_SEMVER, Some("version"), e));
    }
    if manifest.name.chars().count() > MAX_NAME_LEN {
        problems.push(Problem::new(
            BAD_LISTING,
            Some("name"),
            format!("Its name is longer than {MAX_NAME_LEN} characters"),
        ));
    } else if unseen(&manifest.name) {
        problems.push(Problem::new(
            BAD_LISTING,
            Some("name"),
            "Its name has a control or invisible formatting character",
        ));
    } else if let Some(claim) = mark_claim(&manifest.name, true) {
        problems.push(Problem::new(
            BAD_LISTING,
            Some("name"),
            format!("Its name says \"{claim}\": only the store marks a plugin verified or official. Name it for what it does, without that word"),
        ));
    } else if claims_zeron_in_name(&manifest.name).is_some() {
        warnings.push(Problem::new(
            CLAIMS_ZERON,
            Some("name"),
            "Its name says \"Zeron\" other than as what it's for (\"for Zeron\", \"in Zeron\"): only Zeron's own plugins may, and the store refuses it from anyone else",
        ));
    }
    if !manifest.entry.is_empty() && !files.contains_key(&manifest.entry) {
        problems.push(Problem::new(
            BAD_PACKAGE,
            Some("entry"),
            format!("Its entry file {} is missing", manifest.entry),
        ));
    }

    // Its page.
    match files.get(README) {
        Some(bytes) if bytes.len() > MAX_README_BYTES => problems.push(Problem::at(
            TOO_LARGE,
            None,
            README,
            "README.md is over 512 KB",
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

    // What the store shows on its card.
    let description = manifest.description.trim();
    if description.is_empty() {
        problems.push(Problem::new(
            MISSING_LISTING,
            Some("description"),
            "It needs a \"description\": one line saying what it does",
        ));
    } else if description.chars().count() > MAX_DESCRIPTION_LEN || description.contains('\n') {
        problems.push(Problem::new(
            BAD_LISTING,
            Some("description"),
            format!("Its description is one line of at most {MAX_DESCRIPTION_LEN} characters"),
        ));
    } else if unseen(description) {
        problems.push(Problem::new(
            BAD_LISTING,
            Some("description"),
            "Its description has a control or invisible formatting character",
        ));
    } else if let Some(claim) = mark_claim(description, false) {
        problems.push(Problem::new(
            BAD_LISTING,
            Some("description"),
            format!("Its description says \"{claim}\" of the plugin itself: only the store marks a plugin verified or official. Say what it does instead, or say whose thing it is (\"works with the official Slack app\")"),
        ));
    } else if let Some(claim) = claims_zeron(description) {
        warnings.push(Problem::new(
            CLAIMS_ZERON,
            Some("description"),
            format!("Its description says \"{claim}\": only Zeron's own plugins may, and the store refuses it from anyone else"),
        ));
    }
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
    match raw.get("keywords") {
        None | Some(Value::Null) => {}
        Some(Value::Array(items)) => {
            let words: Vec<Option<&str>> = items.iter().map(Value::as_str).collect();
            let unique: BTreeSet<_> = words.iter().collect();
            if items.len() > MAX_KEYWORDS || unique.len() != words.len() {
                problems.push(Problem::new(
                    BAD_LISTING,
                    Some("keywords"),
                    format!("It has at most {MAX_KEYWORDS} keywords, each once"),
                ));
            } else if let Some(bad) = words.iter().find(|w| w.is_none_or(|w| !keyword(w))) {
                problems.push(Problem::new(
                    BAD_LISTING,
                    Some("keywords"),
                    format!(
                        "Keyword \"{}\" must be lowercase letters, digits, dashes and single spaces, at most {MAX_KEYWORD_LEN} characters",
                        bad.unwrap_or("?")
                    ),
                ));
            } else if let Some((keyword, claim)) = words.iter().flatten().find_map(|w| keyword_claim(w).map(|claim| (w, claim))) {
                // (No badge lifts this: every plugin here is for Zeron.)
                problems.push(Problem::new(
                    BAD_LISTING,
                    Some("keywords"),
                    format!("Keyword \"{keyword}\" says \"{claim}\": a keyword may say neither the store's mark nor Zeron. Drop it, or use a word for what the plugin does"),
                ));
            }
        }
        Some(_) => problems.push(Problem::new(
            BAD_LISTING,
            Some("keywords"),
            "Its \"keywords\" must be a list",
        )),
    }
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
                    && files.get(*path).is_some_and(|b| is_screenshot(b));
                if !typed || !there {
                    problems.push(Problem::at(
                        BAD_LISTING,
                        Some("screenshots"),
                        path,
                        "A screenshot must be a PNG or JPEG picture in it",
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
    match zeron_range(raw) {
        Ok(None) => {}
        Ok(Some(range)) => {
            if let Err(e) = Range::parse(&range) {
                problems.push(Problem::new(BAD_LISTING, Some("engines"), e));
            }
        }
        Err(e) => problems.push(Problem::new(BAD_LISTING, Some("engines"), e)),
    }

    if files.get(CHANGELOG).is_some_and(|log| log.len() > MAX_CHANGELOG_BYTES) {
        problems.push(Problem::at(TOO_LARGE, None, CHANGELOG, "CHANGELOG.md is over 512 KB"));
    }
    // (As written: without the signature, which the store keeps apart.)
    if super::signing::canonical_json(&manifest.unsigned_value()).len() > MAX_MANIFEST_BYTES {
        problems.push(Problem::new(TOO_LARGE, Some("manifest"), "Its manifest is over 256 KB"));
    }
    // Two paths that are one file on a Mac or a Windows machine (the
    // manifest's own name among them), or a file where another needs a
    // folder of that name (amendment 42), in code point order.
    for clash in clashes(files.keys().map(String::as_str), &[super::manifest::MANIFEST_FILE]) {
        problems.push(Problem::at(BAD_LISTING, Some("files"), clash.path(), clash.words()));
    }

    // How big.
    let total: u64 = files.values().map(|b| b.len() as u64).sum();
    if files.len() > super::files::MAX_FILES || total > super::files::MAX_BYTES {
        problems.push(Problem::new(
            TOO_LARGE,
            None,
            "It's too big for a plugin (2,000 files, 50 MB)",
        ));
    }

    // Worth knowing.
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
        .map(|log| changelog_notes(&String::from_utf8_lossy(log), &manifest.version));
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
                "CHANGELOG.md has no section for {}: this version shows without notes",
                manifest.version
            ),
        ));
    }
    (problems, warnings)
}

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

    #[test]
    fn the_site_s_pages_are_never_a_listed_plugin_s_id_but_still_run() {
        let (base, files) = listed();
        for id in SITE_IDS {
            // Each a valid id Zeron runs and nothing else already kept.
            assert_eq!(manifest::check_id(id), Ok(()), "{id} still runs");
            assert!(!BUILTIN_IDS.contains(id) && !id.starts_with("zeron") && !manifest::RESERVED_IDS.contains(id), "{id}");
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
