//! Zeron's version, as `ZERON_VERSION`: the root workspace's
//! (`[workspace.package] version` in ../../Cargo.toml). zgx is a workspace of
//! its own at 0.0.0, so its crate version says nothing of which Zeron this
//! is. Templates are stamped with it (src/plugins/live.rs), and one made for
//! another Zeron reads as out of date.

fn main() {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.toml");
    println!("cargo:rerun-if-changed={}", manifest.display());
    let text = std::fs::read_to_string(&manifest).unwrap_or_default();
    println!("cargo:rustc-env=ZERON_VERSION={}", workspace_version(&text).unwrap_or_default());
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
