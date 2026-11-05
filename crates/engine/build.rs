//! The git commit this engine is built from, for its build id
//! (`zeron_engine::build_id`): `ZERON_GIT_COMMIT`, or "unknown" outside a
//! checkout.
fn main() {
    let commit = std::process::Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=ZERON_GIT_COMMIT={commit}");
    // (Run again only when this script changes: the build's own time is
    // the executable's, read as it runs, so a rebuild changes the id without
    // running this again.)
    println!("cargo:rerun-if-changed=build.rs");
}
