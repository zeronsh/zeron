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
pub const MAX_URL_LEN: usize = 300;
pub const MAX_LICENSE_LEN: usize = 64;
pub const MAX_NAME_LEN: usize = 60;
pub const MAX_DESCRIPTION_LEN: usize = 200;
/// An icon is an SVG at most this big (as `scan::icon_svg` reads one).
pub const MAX_ICON_BYTES: usize = 32 * 1024;
pub const SCREENSHOT_TYPES: &[&str] = &["png", "jpg", "jpeg", "webp", "gif"];

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
}

/// One thing in the way of publishing (or, as a warning, worth knowing).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Problem {
    pub rule: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub words: String,
}

impl Problem {
    fn new(rule: &'static str, field: Option<&'static str>, words: impl Into<String>) -> Self {
        Self {
            rule,
            field,
            path: None,
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
            homepage: text(raw.get("homepage")),
            repository: text(raw.get("repository")),
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

/// Whether `id` is one the store keeps for Zeron: its own plugins', or
/// starting `zeron`. (The edge also keeps brand and lookalike names, as
/// usernames are kept: those it says when it refuses.)
pub fn reserved_for_store(id: &str) -> bool {
    BUILTIN_IDS.contains(&id)
        || id.starts_with("zeron")
        || manifest::check_id(id).is_err_and(|e| e == manifest::RESERVED_ID_ERROR)
}

/// An `https://` link without spaces or control characters, of a sensible
/// length, with a host.
fn https_url(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    url.len() <= MAX_URL_LEN
        && !host.is_empty()
        && !host.starts_with('.')
        && host.contains('.')
        && !host.contains(['@', '\\'])
        && !url.chars().any(|c| c.is_whitespace() || c.is_control())
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

/// Whether `path`'s bytes are a picture by their first bytes (PNG, JPEG,
/// GIF, WebP).
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
                format!("\"{}\" is kept for Zeron", manifest.id),
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
            Some(Value::String(url)) if https_url(url) => {}
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
                let there =
                    package::plain_path(path) && files.get(*path).is_some_and(|b| is_picture(b));
                if !typed || !there {
                    problems.push(Problem::at(
                        BAD_LISTING,
                        Some("screenshots"),
                        path,
                        "A screenshot must be a PNG, JPEG, WebP or GIF picture in it",
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
    if let Some(changelog) = files.get(CHANGELOG)
        && changelog_notes(&String::from_utf8_lossy(changelog), &manifest.version).is_none()
    {
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
}
