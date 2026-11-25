//! Which of a template's files are what, for Zeron's engine and its app
//! alike: what's its listing, what's never loaded, what a keep counts, and
//! the digest it's kept by. One answer, so neither installs without asking
//! what the other would ask about. Paths are the template's own, `/` between
//! their parts.

use std::collections::BTreeMap;

use sha2::{Digest, Sha256};

use crate::path::fold;

/// Its manifest, at its top.
pub const MANIFEST: &str = "zeron-template.json";
pub const README: &str = "README.md";
pub const CHANGELOG: &str = "CHANGELOG.md";
/// Its card's picture.
pub const PREVIEW: &str = "preview.png";
/// The folder of its pictures, and what a picture's name ends in.
pub const SCREENSHOTS: &str = "screenshots";
pub const SCREENSHOT_TYPES: &[&str] = &["png", "jpg", "jpeg"];

/// Paths no template replaces: allowing plugins, your account and signing
/// in, and the aliases every module resolves by. It only grows.
pub const NEVER_TEMPLATED: &[&str] = &["screens/plugins/", "screens/templates/", "account/", ".luaurc"];

/// Whether `rel` is one no template replaces. Compared as a case-insensitive
/// filesystem (macOS's, Windows') names files (`path::fold`):
/// `Screens/Plugins/page.luau`, `ſcreens/plugins/page.luau` (U+017F folds to
/// s) and `screens/plugins./page.luau` are Zeron's plugins page there too.
pub fn protected(rel: &str) -> bool {
    let rel = fold(rel);
    NEVER_TEMPLATED.iter().any(|p| {
        let p = fold(p);
        rel == p || rel.starts_with(&format!("{p}/"))
    })
}

/// Whether `rel` is one of its listing files: its README, its changelog, its
/// preview, the pictures under `screenshots/`. Never loaded, and no change of
/// its code.
pub fn listing_file(rel: &str) -> bool {
    match rel.strip_prefix(SCREENSHOTS).and_then(|rest| rest.strip_prefix('/')) {
        Some(rest) => rest.rsplit_once('.').is_some_and(|(_, ext)| SCREENSHOT_TYPES.contains(&ext.to_ascii_lowercase().as_str())),
        None => [README, CHANGELOG, PREVIEW].contains(&rel),
    }
}

/// Whether `rel` is under `screenshots/`, as a disk that folds names has it,
/// and isn't a listing file. Code, so a change to it asks; never loaded.
pub fn stray_screenshot(rel: &str) -> bool {
    !listing_file(rel) && fold(rel).strip_prefix(SCREENSHOTS).is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

/// Whether any part of `rel` could be a Windows short name (`TEMPLA~1`,
/// `LUAURC~1`): a `~` with a digit after it. Windows opens the file that has
/// it, so `screens/TEMPLA~1/page.luau` is `screens/templates/page.luau`
/// there. Never loaded, on any system.
pub fn short_name(rel: &str) -> bool {
    rel.as_bytes().windows(2).any(|w| w[0] == b'~' && w[1].is_ascii_digit())
}

/// Whether `rel` holds a character drawn as nothing (Default_Ignorable, or a
/// control), but the selectors of ordinary emoji (`path::selector`, which
/// `path::fold` compares names without). Some filesystems (HFS+) leave them out of a name they look up, so
/// `screens/tem\u{200c}plates/page.luau` is `screens/templates/page.luau` there.
/// Never loaded, on any system.
pub fn unseen(rel: &str) -> bool {
    rel.chars().any(|c| {
        c.is_control()
            || matches!(c,
                '\u{ad}' | '\u{34f}' | '\u{61c}' | '\u{115f}'..='\u{1160}' | '\u{17b4}'..='\u{17b5}' | '\u{180b}'..='\u{180f}'
                | '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{206f}' | '\u{3164}' | '\u{fe00}'..='\u{fe0d}'
                | '\u{feff}' | '\u{ffa0}' | '\u{fff0}'..='\u{fff8}' | '\u{1bca0}'..='\u{1bca3}' | '\u{1d173}'..='\u{1d17a}'
                | '\u{e0000}'..='\u{e0fff}')
    })
}

/// Whether `rel` isn't part of its interface: its manifest, or its listing.
pub fn meta(rel: &str) -> bool {
    rel == MANIFEST || listing_file(rel)
}

/// Whether `rel` is laid over Zeron's own when it's used.
pub fn composed(rel: &str) -> bool {
    counted(rel) && !stray_screenshot(rel) && !short_name(rel) && !unseen(rel)
}

/// Whether `rel` counts as its code: what a keep covers, and a change to
/// asks. (Wider than what's composed: a stray asks, though it never runs.)
pub fn counted(rel: &str) -> bool {
    !meta(rel) && !protected(rel)
}

/// Whether `rel` is a plain relative path: normal parts, none a dotfile but
/// `.luaurc` at the top.
pub fn plain(rel: &str) -> bool {
    !rel.is_empty()
        && std::path::Path::new(rel).components().all(|c| match c {
            std::path::Component::Normal(name) => {
                let name = name.to_string_lossy();
                !name.starts_with('.') || rel == ".luaurc"
            }
            _ => false,
        })
}

/// The digest a template is kept by: each file that counts (of `files`, the
/// plain ones: an engine's own dotfiles are no part of it), in path order, as
/// its path's length, its path, its length and its bytes.
pub fn digest<B: AsRef<[u8]>>(files: &BTreeMap<String, B>) -> String {
    let mut h = Counting::default();
    for (rel, bytes) in files.iter().filter(|(rel, _)| plain(rel) && counted(rel)) {
        h.add(rel, bytes.as_ref());
    }
    h.finish()
}

/// [`digest`], a file at a time, for files read as they're counted: add them
/// in path order.
#[derive(Default)]
pub struct Counting(Sha256);

impl Counting {
    pub fn add(&mut self, rel: &str, bytes: &[u8]) {
        self.0.update((rel.len() as u64).to_le_bytes());
        self.0.update(rel.as_bytes());
        self.0.update((bytes.len() as u64).to_le_bytes());
        self.0.update(bytes);
    }

    pub fn finish(self) -> String {
        self.0.finalize().iter().map(|b| format!("{b:02x}")).collect()
    }
}

/// Whether `rel` (from `apps/zgx/ui`) is the app's interface, which a
/// template made from Zeron's own copies. Not it: the gallery and its demos,
/// tests and their fixtures, the intro, the consent windows (each its own
/// host, always Zeron's own), and the official templates' files.
pub fn of_the_app(rel: &str) -> bool {
    let first = rel.split('/').next().unwrap_or(rel);
    !(matches!(first, "demos" | "tests" | ".zgx")
        || official_source(rel)
        || matches!(rel, "gallery.luau" | "intro.luau" | "consent.luau" | "template_consent.luau" | "store_consent.luau")
        || (!rel.contains('/') && rel.ends_with("_fixture.luau")))
}

/// Whether `rel` (from `apps/zgx/ui`) is one of the official templates' files.
pub fn official_source(rel: &str) -> bool {
    rel.starts_with("templates/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_each_path_is() {
        // (Listing, stray, protected, short name; then composed, counted.)
        for (rel, listing, stray, kept_out, short, runs, counts) in [
            ("app.luau", false, false, false, false, true, true),
            ("screens/shell/sidebar.luau", false, false, false, false, true, true),
            (MANIFEST, false, false, false, false, false, false),
            ("README.md", true, false, false, false, false, false),
            ("CHANGELOG.md", true, false, false, false, false, false),
            ("preview.png", true, false, false, false, false, false),
            ("screenshots/a.png", true, false, false, false, false, false),
            ("screenshots/deep/a.JPEG", true, false, false, false, false, false),
            ("changelog.md", false, false, false, false, true, true),
            ("docs/README.md", false, false, false, false, true, true),
            ("screenshots.luau", false, false, false, false, true, true),
            ("screens/screenshots/x.luau", false, false, false, false, true, true),
            ("screenshots", false, true, false, false, false, true),
            ("screenshots/png", false, true, false, false, false, true),
            ("screenshots/x.luau", false, true, false, false, false, true),
            ("screenshots/a.png.luau", false, true, false, false, false, true),
            ("Screenshots/a.png", false, true, false, false, false, true),
            ("\u{17f}creenshots/x.luau", false, true, false, false, false, true),
            ("screenshots./x.luau", false, true, false, false, false, true),
            (".luaurc", false, false, true, false, false, false),
            ("account/session.luau", false, false, true, false, false, false),
            ("Screens/Plugins/page.luau", false, false, true, false, false, false),
            ("\u{17f}creens/templates/page.luau", false, false, true, false, false, false),
            ("screens/plugins./page.luau", false, false, true, false, false, false),
            ("screens/plugins.luau", false, false, false, false, true, true),
            ("screens/TEMPLA~1/page.luau", false, false, false, true, false, true),
            ("LUAURC~1", false, false, false, true, false, true),
            ("screens/a~b.luau", false, false, false, false, true, true),
            ("screens/tem\u{200c}plates/page.luau", false, false, false, false, false, true),
            (".luau\u{feff}rc", false, false, false, false, false, true),
            ("account\u{e0041}/x.luau", false, false, false, false, false, true),
            ("screens/a\u{7f}.luau", false, false, false, false, false, true),
            ("screens/a\u{fe0d}.luau", false, false, false, false, false, true),
            ("screens/\u{2764}\u{fe0f}.luau", false, false, false, false, true, true),
            ("screens/a\u{fe0e}.luau", false, false, false, false, true, true),
            ("README\u{fe0f}.md", false, false, false, false, true, true),
            ("screens/templa\u{fe0f}tes/page.luau", false, false, true, false, false, false),
            ("\u{fe0e}.luaurc", false, false, true, false, false, false),
            ("screenshots\u{fe0f}/a.png", false, true, false, false, false, true),
            ("screens/设置.luau", false, false, false, false, true, true),
            ("screens/café.luau", false, false, false, false, true, true),
        ] {
            assert_eq!(
                (listing_file(rel), stray_screenshot(rel), protected(rel), short_name(rel), composed(rel), counted(rel)),
                (listing, stray, kept_out, short, runs, counts),
                "{rel}"
            );
        }
    }

    #[test]
    fn what_s_protected_only_grows() {
        for released in ["screens/plugins/", "screens/templates/", "account/", ".luaurc"] {
            assert!(NEVER_TEMPLATED.contains(&released), "{released}");
        }
    }

    /// The vector both sides are held to.
    #[test]
    fn the_digest_is_of_what_counts() {
        let mut files: BTreeMap<String, &[u8]> = BTreeMap::from([
            ("app.luau".to_owned(), b"return 1".as_slice()),
            ("screens/card.luau".to_owned(), b"-- card"),
            ("screenshots/x.luau".to_owned(), b"return 2"),
            ("screens/TEMPLA~1/page.luau".to_owned(), b"-- short"),
        ]);
        let vector = "4fee345a38a1f2a65523a1d25ac01fd7cdde15f4524afbe1d7ce72933f47827e";
        assert_eq!(digest(&files), vector);
        // None of these is any part of it.
        for rel in [MANIFEST, "README.md", "CHANGELOG.md", "preview.png", "screenshots/a.png", ".luaurc", "account/x.luau", "screens/plugins/x.luau", ".imported.json", ".lineage.json", ".base.json", "screens/.hidden.luau"] {
            files.insert(rel.to_owned(), b"x");
        }
        assert_eq!(digest(&files), vector);
        files.insert("screenshots/y.svg".to_owned(), b"x");
        assert_ne!(digest(&files), vector);
    }

    #[test]
    fn the_app_s_files_are_what_a_template_copies() {
        for rel in ["app.luau", ".luaurc", "ds/tokens.luau", "screens/shell/view.luau", "locales/en.json", "ds/themes/zeron-dark.json", "screens/x_fixture.luau"] {
            assert!(of_the_app(rel), "{rel}");
        }
        for rel in ["demos/a.luau", "tests/a.test.luau", ".zgx/x", "gallery.luau", "intro.luau", "consent.luau", "template_consent.luau", "store_consent.luau", "chat_fixture.luau", "templates/zeron-v2/app.luau"] {
            assert!(!of_the_app(rel), "{rel}");
        }
    }
}
