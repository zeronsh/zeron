//! Zeron's version, as `ZERON_VERSION`: the root workspace's
//! (`[workspace.package] version` in ../../Cargo.toml). zgx is a workspace of
//! its own at 0.0.0, so its crate version says nothing of which Zeron this
//! is. Templates are stamped with it (src/plugins/live.rs), and one made for
//! another Zeron reads as out of date.
//!
//! And the app's own version, as `ZERON_APP_VERSION`: a beta build's
//! (`ZERON_BETA_VERSION=0.2.136-beta.1` at build time, the release
//! workflow's), else Zeron's. `zeron --version` prints it and the updater
//! compares releases with it.

fn main() {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.toml");
    println!("cargo:rerun-if-changed={}", manifest.display());
    println!("cargo:rerun-if-env-changed=ZERON_BETA_VERSION");
    let text = std::fs::read_to_string(&manifest).unwrap_or_default();
    let version = workspace_version(&text).unwrap_or_default();
    let app = std::env::var("ZERON_BETA_VERSION")
        .ok()
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| version.clone());
    println!("cargo:rustc-env=ZERON_VERSION={version}");
    println!("cargo:rustc-env=ZERON_APP_VERSION={app}");
}

/// `version = "…"` under `[workspace.package]`.
fn workspace_version(toml: &str) -> Option<String> {
    let mut inside = false;
    for line in toml.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            inside = line == "[workspace.package]";
        } else if inside && let Some(rest) = line.strip_prefix("version") {
            let value = rest.trim_start().strip_prefix('=')?.trim();
            return Some(value.trim_matches('"').to_string());
        }
    }
    None
}
