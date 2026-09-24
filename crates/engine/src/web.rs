//! The engine-served web client bundle.
//!
//! `build.rs` stages the Vite production build (or, in debug, the placeholder
//! pages from `assets/web/`) into `web-staging/` so this module sees a single
//! stable source of truth. rust-embed embeds that tree into the engine
//! binary: at request time in debug it reads the staged directory from disk,
//! in release it serves bytes compiled into the binary. Either way the
//! release artifact is a single file with no external asset dependency, and
//! the bundle version is the engine version by construction.

use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "web-staging"]
pub(crate) struct WebAssets;

/// MIME type for an embedded file, by extension. Unknown extensions fall
/// back to `application/octet-stream` so the engine still answers (with a
/// non-specific type) rather than 500-ing on a missing hint.
pub(crate) fn content_type(name: &str) -> &'static str {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}
