//! A picture on this computer, for the lightbox (ui/ds/lightbox.luau): copied to
//! the clipboard, or shown in Finder. A script can't load a file into a
//! `Bitmap` for `ui.clipboard.write_image`, so the host does it.
//!
//!   image.copy { path }     its pixels on the clipboard, read and decoded
//!                           off the window's thread; true once it took them
//!   image.reveal { path }   shown in Finder (elsewhere, its folder opened)
//!   image.tones { path }    its tonal range, { lo, hi }: the 2nd and 98th
//!                           percentiles of its lightness (0 to 1), read off
//!                           the window's thread and kept by path (a new
//!                           chat's ASCII stretches its glyphs across it)
//!
//! (A picture's size and whether its file is there are zgx's own now:
//! `ui.image_size`, and `ui.image`'s `on_error` and `fallback`.)

use std::{cell::RefCell, path::Path, rc::Rc, sync::Arc};

use zgx::platform::PlatformService;
use zgx_script::{ScriptHost, Value};

/// The largest file read for a copy: past this it is refused, not decoded.
const MAX_BYTES: u64 = 64 * 1024 * 1024;

pub fn install(host: &ScriptHost, platform: Rc<RefCell<Option<PlatformService>>>) {
    install_tones(host);
    host.async_action("image.copy", move |arg| {
        let platform = platform.clone();
        async move {
            let path = path_of(&arg)?;
            let image = off_thread(move || decode(&path)).await.ok_or("The image couldn't be read")??;
            let service = platform.borrow().clone().ok_or("No clipboard here")?;
            if service.0.set_clipboard_image(image).await {
                Ok(Value::Bool(true))
            } else {
                Err("The clipboard didn't take the image".into())
            }
        }
    });
    host.async_action("image.reveal", |arg| async move {
        let path = path_of(&arg)?;
        reveal(Path::new(&path)).map_err(|e| e.to_string())?;
        Ok(Value::Nil)
    });
}

/// `image.tones`, alone: what a headless window (the audit shots) needs.
pub fn install_tones(host: &ScriptHost) {
    host.async_action("image.tones", |arg| async move {
        let path = path_of(&arg)?;
        let (lo, hi) = off_thread(move || tones(&path)).await.ok_or("The image couldn't be read")??;
        Ok(Value::record([("lo", Value::from(lo as f64)), ("hi", Value::from(hi as f64))]))
    });
}

/// The 2nd and 98th percentiles of a picture's lightness (its sRGB values
/// weighed as Rec. 709 weighs them, 0 to 1), from about 65,000 of its
/// pixels; kept by path, as a picture kept for a background never changes.
fn tones(path: &str) -> Result<(f32, f32), String> {
    static KEPT: std::sync::Mutex<Vec<(String, (f32, f32))>> = std::sync::Mutex::new(Vec::new());
    if let Some((_, t)) = KEPT.lock().ok().and_then(|k| k.iter().find(|(p, _)| p == path).cloned()) {
        return Ok(t);
    }
    let image = decode(path)?;
    let t = range(image.pixels(), (image.width() * image.height()) as usize);
    if let Ok(mut kept) = KEPT.lock() {
        if kept.len() >= 8 {
            kept.remove(0);
        }
        kept.push((path.to_string(), t));
    }
    Ok(t)
}

/// The 2nd and 98th percentiles of the lightness of `n` RGBA pixels.
fn range(rgba: &[u8], n: usize) -> (f32, f32) {
    let step = (n / 65_536).max(1);
    let mut counts = [0u32; 256];
    let mut total = 0u32;
    for px in rgba.chunks_exact(4).step_by(step) {
        let l = 0.2126 * px[0] as f32 + 0.7152 * px[1] as f32 + 0.0722 * px[2] as f32;
        counts[(l.round() as usize).min(255)] += 1;
        total += 1;
    }
    let at = |share: f32| {
        let want = (total as f32 * share).ceil() as u32;
        let mut seen = 0;
        for (i, c) in counts.iter().enumerate() {
            seen += c;
            if seen >= want.max(1) {
                return i as f32 / 255.;
            }
        }
        1.
    };
    (at(0.02), at(0.98))
}

fn path_of(arg: &Value) -> Result<String, String> {
    let path = arg.field("path").map(crate::auth::text).unwrap_or_default();
    if path.is_empty() {
        return Err("no path".into());
    }
    if !Path::new(&path).is_absolute() {
        return Err("That image isn't a file on this computer".into());
    }
    Ok(path)
}

/// The file's pixels, decoded as `ui.image` decodes it.
fn decode(path: &str) -> Result<Arc<zgx::image::ImageData>, String> {
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if meta.len() > MAX_BYTES {
        return Err("The image is too large to copy".into());
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    zgx_gpu::assets::decode_image(&bytes).map_err(|e| e.to_string())
}

/// `work` on a thread of its own, its answer awaited here (None only if it
/// panicked), as chat.rs's.
async fn off_thread<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (tx, mut rx) = zgx::task::latest();
    std::thread::spawn(move || {
        tx.send(work());
    });
    rx.recv().await
}

#[cfg(target_os = "macos")]
fn reveal(path: &Path) -> std::io::Result<()> {
    reveal_with("/usr/bin/open", path).map(|_| ())
}

/// `program -R path` started and waited on when it ends (files.rs's
/// `spawn_reaped`: a child never waited on stays a zombie, one per Reveal).
/// Its pid.
#[cfg(unix)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn reveal_with(program: &str, path: &Path) -> std::io::Result<u32> {
    crate::files::spawn_reaped(std::process::Command::new(program).arg("-R").arg(path))
}

#[cfg(not(target_os = "macos"))]
fn reveal(path: &Path) -> std::io::Result<()> {
    open::that_detached(path.parent().unwrap_or(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_a_png_to_rgba() {
        let dir = std::env::temp_dir().join(format!("zgx-images-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("one.png");
        let mut png = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut png, 3, 2);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.write_header().unwrap().write_image_data(&[1, 2, 3, 255].repeat(6)).unwrap();
        }
        std::fs::write(&path, png).unwrap();
        let image = decode(path.to_str().unwrap()).unwrap();
        assert_eq!((image.width(), image.height()), (3, 2));
        assert_eq!(&image.pixels()[..4], &[1, 2, 3, 255]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_child_started_to_reveal_is_waited_on() {
        let pid = reveal_with("/usr/bin/true", Path::new("/tmp")).expect("started");
        // Gone once it ends: a zombie (never waited on) would still answer.
        let mut gone = false;
        for _ in 0..100 {
            if unsafe { libc::kill(pid as i32, 0) } != 0 {
                gone = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(gone, "pid {pid} stayed behind");
    }

    #[test]
    fn a_pictures_tones_span_its_lightness_past_its_extremes() {
        // A ramp from 20 to 220: its extremes' 2% are left out at each end.
        let rgba: Vec<u8> = (0..1000)
            .flat_map(|i| {
                let v = (20 + i / 5) as u8;
                [v, v, v, 255]
            })
            .collect();
        let (lo, hi) = range(&rgba, 1000);
        assert!((lo - 24. / 255.).abs() < 1.5 / 255. && (hi - 216. / 255.).abs() < 1.5 / 255., "{lo} {hi}");
    }

    #[test]
    fn only_absolute_paths_are_files_here() {
        assert!(path_of(&Value::record([("path", Value::from("assets/x.png"))])).is_err());
        assert!(path_of(&Value::record([("path", Value::from("/tmp/x.png"))])).is_ok());
    }
}
