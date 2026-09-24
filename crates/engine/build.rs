//! Stage the web client bundle into `web-staging/` for rust-embed to bake
//! into the engine binary. The default source is the Vite production build
//! at `web/packages/app/dist/`; `ZERON_WEB_DIST` overrides that for
//! vendored bundles or custom layouts.
//!
//! Behaviour by profile:
//! - source has files: copy them into `web-staging/`. rust-embed embeds
//!   that tree in release, reads it from disk in debug.
//! - source is empty/missing in release: panic. A release binary must
//!   never ship without the React app.
//! - source is empty/missing in debug: fall back to the hand-written
//!   placeholder pages under `assets/web/` so `cargo test` keeps working

use std::{
    env, fs,
    path::{Path, PathBuf},
};

fn main() {
    println!("cargo:rerun-if-env-changed=ZERON_WEB_DIST");

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let source = env::var_os("ZERON_WEB_DIST")
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest_dir.join("../../web/packages/app/dist"));
    let fallback = manifest_dir.join("assets/web");
    let staged = manifest_dir.join("web-staging");

    println!("cargo:rerun-if-changed={}", source.display());
    println!("cargo:rerun-if-changed={}", fallback.display());

    // Wipe any prior staging so removed files don't linger.
    let _ = fs::remove_dir_all(&staged);
    fs::create_dir_all(&staged).expect("could not create crates/engine/web-staging/");

    let profile = env::var("PROFILE").unwrap_or_default();
    let source_has_files = source.is_dir()
        && fs::read_dir(&source)
            .map(|mut it| it.next().is_some())
            .unwrap_or(false);

    let (copied_from, count) = if source_has_files {
        let n = copy_tree(&source, &staged).expect("could not stage web bundle");
        (source.clone(), n)
    } else if profile != "release" && fallback.is_dir() {
        let n = copy_tree(&fallback, &staged).expect("could not stage placeholder web pages");
        (fallback.clone(), n)
    } else {
        panic!(
            "the web bundle at {} is missing or empty — run \
             `pnpm --filter @zeron/app build` first, or set \
             ZERON_WEB_DIST to a built bundle directory",
            source.display()
        );
    };

    println!(
        "cargo:warning=zeron engine: staged {} entries from {} into {}",
        count,
        copied_from.display(),
        staged.display()
    );
}

fn copy_tree(src: &Path, dst: &Path) -> std::io::Result<usize> {
    let mut count = 0;
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let target = dst.join(entry.file_name());
        if file_type.is_dir() {
            count += copy_tree(&entry.path(), &target)?;
        } else {
            fs::copy(&entry.path(), &target)?;
            count += 1;
        }
    }
    Ok(count)
}
