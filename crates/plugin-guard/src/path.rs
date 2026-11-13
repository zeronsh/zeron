//! A path inside a plugin or template package (`ui/badge.luau`), before
//! anything joins it to a folder on disk. Every package or manifest path
//! that reaches the filesystem goes through [`join`]: whatever a package
//! names stays inside the folder it's written to, on every system.
//!
//! What's refused, and why:
//! - anything but plain parts (`/x`, `..`, `.`, `a//b`, `\`): it would leave
//!   the folder, or mean something else on Windows;
//! - `:` anywhere: `C:x` / `D:x.cmd` is drive-relative on Windows (it
//!   escapes the folder), and `x:stream` is an NTFS stream;
//! - Windows' reserved device names, with or without an extension (`CON`,
//!   `NUL.txt`, `com1.luau`, `LPT¹`): writing one talks to a device;
//! - a part ending in `.` or space: Windows drops those, so `a.` names `a`;
//! - control characters and `<>"|?*` (never valid on Windows).
//!
//! [`fold`] is the other half: how a case-insensitive filesystem compares
//! two paths, for checks that a path isn't a protected one (macOS and
//! Windows would open `Screens/Plugins/page.luau`, or `ſcreens/…`, as
//! `screens/plugins/page.luau`).

use std::path::{Component, Path, PathBuf};

/// The longest package path.
pub const MAX_PATH: usize = 1024;

/// Why `rel` can't be a package path, if it can't.
pub fn problem(rel: &str) -> Option<String> {
    let refuse = |why: &str| Some(format!("\"{rel}\" can't be a file's path in a package: {why}"));
    if rel.is_empty() || rel.len() > MAX_PATH {
        return refuse("a path is 1 to 1024 bytes");
    }
    if rel.starts_with('/') {
        return refuse("it's relative to the package");
    }
    for part in rel.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            return refuse("its parts are names (no empty part, \".\" or \"..\")");
        }
        if let Some(c) = part.chars().find(|c| c.is_control() || matches!(c, '\\' | ':' | '<' | '>' | '"' | '|' | '?' | '*')) {
            return refuse(&format!("{c:?} isn't allowed in a name"));
        }
        if part.ends_with('.') || part.ends_with(' ') {
            return refuse("a name can't end with a dot or a space");
        }
        if reserved_on_windows(part) {
            return refuse("that name is a device on Windows");
        }
    }
    // (Belt and braces: what the platform's own parser makes of it.)
    if !Path::new(rel).components().all(|c| matches!(c, Component::Normal(_))) {
        return refuse("its parts are names");
    }
    None
}

/// Whether `rel` is a package path (see the module's rules).
pub fn is_safe(rel: &str) -> bool {
    problem(rel).is_none()
}

/// `rel` under `base`, if `rel` is a package path. The one way a package
/// or manifest path becomes a path on disk.
pub fn join(base: &Path, rel: &str) -> Result<PathBuf, String> {
    match problem(rel) {
        Some(problem) => Err(problem),
        None => Ok(rel.split('/').fold(base.to_path_buf(), |path, part| path.join(part))),
    }
}

/// Windows' device names: `CON`, `PRN`, `AUX`, `NUL`, `COM0`–`COM9`,
/// `LPT0`–`LPT9` (and their superscript-digit forms), `CONIN$`, `CONOUT$`,
/// in any case, with anything after a dot, and with spaces before it.
pub fn reserved_on_windows(part: &str) -> bool {
    let stem = part.split('.').next().unwrap_or_default().trim_end_matches(' ');
    let upper = stem.to_ascii_uppercase();
    if matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$") {
        return true;
    }
    let mut chars = stem.chars();
    let head: String = chars.by_ref().take(3).collect::<String>().to_ascii_uppercase();
    let rest: Vec<char> = chars.collect();
    (head == "COM" || head == "LPT")
        && rest.len() == 1
        && (rest[0].is_ascii_digit() || matches!(rest[0], '¹' | '²' | '³'))
}

/// `rel` as a case-insensitive, normalising filesystem compares it: `\`
/// as `/`, each character's uppercase then its lowercase (`ſ` → `s`,
/// `K` (Kelvin) → `k`, `ı` → `i`), and each part without the dots and
/// spaces Windows drops from its end. Two paths that fold the same can
/// name the same file on macOS or Windows; compare folds for "is this the
/// protected file".
pub fn fold(rel: &str) -> String {
    let unified: String = rel
        .chars()
        .map(|c| if c == '\\' { '/' } else { c })
        .flat_map(char::to_uppercase)
        .flat_map(char::to_lowercase)
        .collect();
    unified
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .map(|part| part.trim_end_matches(['.', ' ']))
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_names_join() {
        let base = Path::new("/staged");
        assert_eq!(join(base, "ui/badge.luau").unwrap(), Path::new("/staged/ui/badge.luau"));
        for ok in ["init.luau", "a/b/c.png", "café.png", ".hidden", "console.luau", "com10.txt", "lpt.txt", "nul-ish.luau"] {
            assert!(is_safe(ok), "{ok}");
        }
    }

    /// R9: a drive-relative path (`D:x.cmd`, `C:../…/Startup/x.cmd`) would
    /// leave the staging folder on Windows; so would a device name or a
    /// trailing dot. Every one is refused before any join.
    #[test]
    fn what_escapes_on_any_system_is_refused() {
        for bad in [
            "",
            "/etc/passwd",
            "../x",
            "a/../b",
            "./a",
            "a//b",
            "a\\b",
            "..\\x",
            "D:x.cmd",
            "C:../Roaming/Microsoft/Windows/Start Menu/Programs/Startup/x.cmd",
            "C:/x",
            "a/b:stream",
            "CON",
            "nul.txt",
            "Aux.tar.gz",
            "ui/COM1.luau",
            "lpt9",
            "COM¹",
            "conin$",
            "NUL .txt",
            "a.",
            "a ",
            "dir./x",
            "a\0b",
            "a\nb",
            "a?b",
            "a*b",
            "a|b",
            "a<b",
            "a\"b",
        ] {
            assert!(problem(bad).is_some(), "{bad:?} was accepted");
            assert!(join(Path::new("/staged"), bad).is_err(), "{bad:?}");
        }
        assert!(!is_safe(&"a".repeat(MAX_PATH + 1)));
    }

    /// R30: what a case-insensitive filesystem would take for the
    /// protected file folds the same.
    #[test]
    fn folding_matches_what_macos_and_windows_open() {
        let protected = fold("screens/plugins/page.luau");
        for same in [
            "screens/plugins/page.luau",
            "Screens/Plugins/PAGE.luau",
            "\u{17f}creens/plugins/page.luau",
            "screens\\plugins\\page.luau",
            "screens/plugins./page.luau",
            "screens/plugins/page.luau ",
            "screens//plugins/./page.luau",
        ] {
            assert_eq!(fold(same), protected, "{same:?}");
        }
        assert_eq!(fold("\u{212a}"), "k");
        assert_ne!(fold("screens/plugin/page.luau"), protected);
    }
}
