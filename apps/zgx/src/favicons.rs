//! A page's icon for the panel's browser tab (ui/screens/panel/browser.luau):
//! the page names it (`link[rel~=icon]`, else `/favicon.ico`), and the host
//! fetches it, since a script's `ui.image` takes only files (zgx's remote
//! sources need a fetcher the host doesn't install). Kept under the data
//! dir by a hash of the URL, so a page seen again shows its icon at once.
//!
//!   favicon.fetch { url }   the icon's file, or nil when there is none
//!                           (not http(s), too big, not an image, or the
//!                           server says no)

use sha2::{Digest, Sha256};
use std::{io::Read, path::PathBuf, time::Duration};
use zgx_script::{ScriptHost, Value};

/// Past this the icon is refused, not read.
const MAX_BYTES: u64 = 512 * 1024;
const TIMEOUT: Duration = Duration::from_secs(8);

pub fn install(host: &ScriptHost) {
    host.async_action("favicon.fetch", |arg| async move {
        let url = arg.field("url").map(crate::auth::text).unwrap_or_default();
        // A request of the host's own would carry out what a preview put in
        // its URL. No icon, as for a page without one.
        if crate::plugins::preview::current().is_some() {
            return Ok(Value::Nil);
        }
        if !(url.starts_with("https://") || url.starts_with("http://")) || url.chars().any(char::is_control) {
            return Ok(Value::Nil);
        }
        let Some(dir) = crate::engine::data_dir().map(|d| d.join("favicons")) else {
            return Ok(Value::Nil);
        };
        let path = file_for(&dir, &url);
        if path.is_file() {
            return Ok(Value::from(path.to_string_lossy().as_ref()));
        }
        let fetched = off_thread(move || fetch(&url, &path)).await;
        Ok(match fetched {
            Some(Ok(path)) => Value::from(path.to_string_lossy().as_ref()),
            _ => Value::Nil,
        })
    });
}

/// Where `url`'s icon is kept: the hash of the URL, with the extension its
/// path ends in (zgx decodes by the file's bytes; the extension is for a
/// look in Finder).
fn file_for(dir: &std::path::Path, url: &str) -> PathBuf {
    let mut hash = Sha256::new();
    hash.update(url.as_bytes());
    let name: String = hash.finalize().iter().take(12).map(|b| format!("{b:02x}")).collect();
    let ext = url
        .split(['?', '#'])
        .next()
        .and_then(|p| p.rsplit('.').next())
        .filter(|e| matches!(*e, "ico" | "png" | "svg" | "jpg" | "jpeg" | "gif" | "webp"))
        .unwrap_or("ico");
    dir.join(format!("{name}.{ext}"))
}

fn fetch(url: &str, path: &PathBuf) -> Result<PathBuf, String> {
    let agent = ureq::AgentBuilder::new().timeout(TIMEOUT).build();
    let response = agent.get(url).call().map_err(|e| e.to_string())?;
    let kind = response.content_type().to_ascii_lowercase();
    if !(kind.starts_with("image/") || kind == "application/octet-stream") {
        return Err(format!("not an image: {kind}"));
    }
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_BYTES {
        return Err("empty, or too big".into());
    }
    // Only what `ui.image` can draw: a file it can't (a format left out of
    // the `image` features, an HTML page sent as octet-stream) would leave
    // the tab's icon blank, where no icon shows the globe.
    if !drawable(&bytes) {
        return Err("not an image the app can draw".into());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    // (Whole or not at all: written beside, then moved into place.)
    let tmp = path.with_extension("part");
    std::fs::write(&tmp, &bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())?;
    Ok(path.clone())
}

/// Whether `ui.image` can draw these bytes: an SVG, or a raster the `image`
/// crate decodes with the features this app turns on (Cargo.toml).
pub(crate) fn drawable(bytes: &[u8]) -> bool {
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(512)]);
    let head = head.trim_start_matches('\u{feff}').trim_start();
    if head.starts_with("<svg") || (head.starts_with("<?xml") && head.contains("<svg")) {
        return true;
    }
    image::load_from_memory(bytes).is_ok()
}

pub(crate) async fn off_thread<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (tx, mut rx) = zgx::task::latest();
    std::thread::spawn(move || {
        tx.send(work());
    });
    rx.recv().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_icons_the_app_can_draw_are_kept() {
        // A 1x1 .ico (its one image a PNG, as modern .ico files hold), a PNG,
        // an SVG; and what a server may send as octet-stream that isn't one.
        let png: &[u8] = &[
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0x0d, 0x49, 0x48, 0x44, 0x52, 0, 0, 0, 1, 0, 0, 0, 1,
            8, 6, 0, 0, 0, 0x1f, 0x15, 0xc4, 0x89, 0, 0, 0, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8, 0xcf,
            0xc0, 0xf0, 0x1f, 0, 0x05, 0, 0x01, 0xff, 0x89, 0x99, 0x3d, 0x1d, 0, 0, 0, 0, 0x49, 0x45, 0x4e, 0x44, 0xae,
            0x42, 0x60, 0x82,
        ];
        let mut ico = vec![0, 0, 1, 0, 1, 0, 1, 1, 0, 0, 1, 0, 32, 0];
        ico.extend_from_slice(&(png.len() as u32).to_le_bytes());
        ico.extend_from_slice(&22u32.to_le_bytes());
        ico.extend_from_slice(png);
        assert!(drawable(png));
        assert!(drawable(&ico), "an .ico decodes (the `ico` feature)");
        assert!(drawable(b"<svg xmlns='http://www.w3.org/2000/svg'/>"));
        assert!(!drawable(b"<!doctype html><title>Not found</title>"));
    }

    #[test]
    fn an_icons_file_is_named_by_its_url_and_keeps_an_image_extension() {
        let dir = std::path::Path::new("/tmp/favicons");
        let a = file_for(dir, "https://example.com/favicon.ico");
        let b = file_for(dir, "https://example.com/icon.png?v=2");
        let c = file_for(dir, "https://example.com/icon");
        assert_ne!(a, b);
        assert!(a.to_string_lossy().ends_with(".ico"));
        assert!(b.to_string_lossy().ends_with(".png"));
        assert!(c.to_string_lossy().ends_with(".ico"));
        assert_eq!(a, file_for(dir, "https://example.com/favicon.ico"));
    }
}
