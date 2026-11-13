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
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        use std::{path::PathBuf, process::Command};
        let source = "../../crates/ui/src/dictation/permission.m";
        println!("cargo:rerun-if-changed={source}");
        let out = PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
        let object = out.join("dictation-permission.o");
        assert!(
            Command::new("clang")
                .args(["-fobjc-arc", "-c", source, "-o"])
                .arg(&object)
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("ar")
                .arg("crus")
                .arg(out.join("libdictation-permission.a"))
                .arg(object)
                .status()
                .unwrap()
                .success()
        );
        println!("cargo:rustc-link-search=native={}", out.display());
        println!("cargo:rustc-link-lib=static=dictation-permission");
        println!("cargo:rustc-link-lib=framework=AVFoundation");
    }

    // (Read as it runs, not with `env!`: a target dir shared between
    // checkouts can run a build script compiled in another, which then
    // read that checkout's manifest, or none once it was removed.)
    let dir = std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR");
    let manifest = std::path::Path::new(&dir).join("../../Cargo.toml");
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

    // On Windows, Zeron's icon in the exe: without one the taskbar shows
    // the system's blank one, and zgx has none to give the window.
    println!("cargo:rerun-if-changed=../../dist/windows/zeron.rc");
    println!("cargo:rerun-if-changed=../../dist/windows/zeron.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile_for(
            "../../dist/windows/zeron.rc",
            &["zeron-zgx"],
            embed_resource::NONE,
        )
        .manifest_required()
        .expect("Windows app icon resource compilation failed");
    }
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
