//! Site icons remembered per origin, so a web chip keeps its site's icon after
//! the tab that fetched it closes. Only the browser stores icons; chips only
//! read them, so naming a page never sends a request anywhere.
use gpui::RenderImage;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, Mutex, OnceLock},
};

static DIR: OnceLock<PathBuf> = OnceLock::new();
/// Decoded icons by origin; `None` remembers an origin with no stored icon.
static CACHE: LazyLock<Mutex<HashMap<String, Option<Arc<RenderImage>>>>> =
    LazyLock::new(Default::default);

/// Keep icons under the installation's data directory. The first window wins:
/// every window of one installation shares it.
pub(crate) fn init(data_dir: &Path) {
    let _ = DIR.set(data_dir.join("favicons"));
}

/// `scheme://host[:port]` of an http(s) address: what a site's icon belongs to.
fn origin(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    matches!(parsed.scheme(), "http" | "https").then(|| parsed.origin().ascii_serialization())
}

/// Hashed, so the directory doesn't list the sites this device visited.
fn file(origin: &str) -> Option<PathBuf> {
    let hash = Sha256::digest(origin.as_bytes());
    Some(DIR.get()?.join(format!("{hash:x}.png")))
}

fn decode(png: &[u8]) -> Option<Arc<RenderImage>> {
    let mut pixels = image::load_from_memory_with_format(png, image::ImageFormat::Png)
        .ok()?
        .into_rgba8();
    // GPUI's image atlas uses BGRA byte order.
    for pixel in pixels.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Some(Arc::new(RenderImage::new([image::Frame::new(pixels)])))
}

/// Remember the icon the browser fetched for `page`, a PNG.
pub(crate) fn store(page: &str, png: &[u8]) {
    let Some(origin) = origin(page) else {
        return;
    };
    let path = file(&origin);
    // Most loads find the same icon again; leave the decoded one in place.
    if path
        .as_ref()
        .and_then(|path| std::fs::read(path).ok())
        .is_some_and(|stored| stored == png)
        && CACHE
            .lock()
            .unwrap()
            .get(&origin)
            .is_some_and(Option::is_some)
    {
        return;
    }
    let Some(image) = decode(png) else {
        return;
    };
    if let Some(path) = path {
        let written = (|| -> std::io::Result<()> {
            let dir = path.parent().unwrap();
            std::fs::create_dir_all(dir)?;
            let temp = dir.join(format!(".{}", uuid::Uuid::new_v4()));
            std::fs::write(&temp, png)?;
            std::fs::rename(&temp, &path).inspect_err(|_| {
                let _ = std::fs::remove_file(&temp);
            })
        })();
        if let Err(error) = written {
            tracing::debug!("could not store a site icon: {error}");
        }
    }
    CACHE.lock().unwrap().insert(origin, Some(image));
}

/// The icon this device stored for `url`'s site, if any. Cheap after the
/// first look, so painters can call it every frame.
pub(crate) fn cached(url: &str) -> Option<Arc<RenderImage>> {
    let origin = origin(url)?;
    let mut cache = CACHE.lock().unwrap();
    if let Some(image) = cache.get(&origin) {
        return image.clone();
    }
    let image = file(&origin)
        .and_then(|path| std::fs::read(path).ok())
        .and_then(|png| decode(&png));
    cache.insert(origin, image.clone());
    image
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icons_belong_to_their_origin() {
        assert_eq!(
            origin("https://docs.rs/gpui/latest/?q=1").as_deref(),
            Some("https://docs.rs")
        );
        assert_eq!(
            origin("http://localhost:3000/app").as_deref(),
            Some("http://localhost:3000")
        );
        assert_eq!(origin("file:///etc/hosts"), None);
        assert_eq!(origin("not a url"), None);
    }
}
