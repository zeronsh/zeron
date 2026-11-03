//! Development switches, read from the environment: on in debug builds
//! alone, never in a release or the beta. Each one lets whoever sets the
//! environment past a decision the user makes:
//! - `ZERON_ZGX_DEV_PLUGINS` loads unsigned plugins with all they ask for;
//! - `ZERON_ZGX_REVIEW` / `ZERON_ZGX_TEMPLATE_REVIEW` serve devtools (click,
//!   type) on a consent window;
//! - `ZGX_INSPECT_SOCKET` serves devtools on the main window.
//!
//! An agent (or anything else) launching Zeron sets its environment, so a
//! shipped build reads none of them. Every read goes through `switch`
//! (a test holds the source to it).

use std::ffi::OsString;

/// The development switches, by name.
pub const SWITCHES: &[&str] = &[
    "ZERON_ZGX_DEV_PLUGINS",
    "ZERON_ZGX_DEV_GITHUB",
    "ZERON_ZGX_DEV_SERVICE",
    "ZERON_ZGX_REVIEW",
    "ZERON_ZGX_TEMPLATE_REVIEW",
    "ZGX_INSPECT_SOCKET",
];

/// The switches scripts/smoke.sh drives a release build with (as it does
/// `--inspect`): read by any build but the beta, which ignores its
/// arguments too. `ZERON_ZGX_UI` runs other scripts as the app's;
/// `ZERON_ZGX_NO_BROWSER` hands the sign-in page to a file.
pub const SMOKE_SWITCHES: &[&str] = &["ZERON_ZGX_UI", "ZERON_ZGX_NO_BROWSER"];

/// Whether this build reads `SWITCHES`.
pub const ON: bool = cfg!(debug_assertions);

/// The switch `name`'s value, in a debug build.
pub fn switch(name: &str) -> Option<OsString> {
    switch_in(ON, name)
}

/// The smoke switch `name`'s value, in any build but the beta.
pub fn smoke_switch(name: &str) -> Option<OsString> {
    switch_in(!cfg!(feature = "beta"), name)
}

/// `switch`, in a build that does (`on`) or doesn't read them.
fn switch_in(on: bool, name: &str) -> Option<OsString> {
    on.then(|| std::env::var_os(name)).flatten().filter(|v| !v.is_empty())
}

/// `ZGX_INSPECT_SOCKET=1`, as zgx reads it (`inspect::asked`).
pub fn inspect_asked() -> bool {
    switch("ZGX_INSPECT_SOCKET").is_some_and(|v| v != "0")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// R28: a release build (or the beta) reads no development switch, set
    /// or not. (`PATH` is set in every test run: read as if it were one.)
    #[test]
    fn a_release_build_reads_no_development_switch() {
        assert!(std::env::var_os("PATH").is_some());
        assert_eq!(switch_in(false, "PATH"), None);
        assert!(switch_in(true, "PATH").is_some());
    }

    /// R28: nothing else in the app reads a development switch, or asks zgx
    /// whether the environment asks for devtools (`inspect::asked` reads
    /// `ZGX_INSPECT_SOCKET` itself, in any build): each read is `switch`'s,
    /// so a release build honours none (`ZERON_ZGX_DEV_PLUGINS` once
    /// loaded unsigned plugins with all they asked in the shipped app).
    #[test]
    fn every_development_switch_is_read_through_switch() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut found = Vec::new();
        let mut stack = vec![root];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().is_none_or(|e| e != "rs") || path.ends_with("dev.rs") {
                    continue;
                }
                let text = std::fs::read_to_string(&path).unwrap();
                for (n, line) in text.lines().enumerate() {
                    let reads = line.contains("env::var") || line.contains("inspect::asked");
                    if reads
                        && (line.contains("inspect::asked")
                            || SWITCHES.iter().chain(SMOKE_SWITCHES).any(|s| line.contains(&format!("\"{s}\""))))
                    {
                        found.push(format!("{}:{}", path.display(), n + 1));
                    }
                }
            }
        }
        assert!(found.is_empty(), "read without dev::switch: {found:?}");
    }
}
