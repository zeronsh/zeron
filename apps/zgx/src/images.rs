//! A picture on this Mac, for the lightbox (ui/ds/lightbox.luau): copied to
//! the clipboard, or shown in Finder. A script can't load a file into a
//! `Bitmap` for `ui.clipboard.write_image`, so the host does it.
//!
//!   image.copy { path }     its pixels on the clipboard, read and decoded
//!                           off the window's thread; true once it took them
//!   image.reveal { path }   shown in Finder (elsewhere, its folder opened)
//!
//! (A picture's size and whether its file is there are zgx's own now:
//! `ui.image_size`, and `ui.image`'s `on_error` and `fallback`.)

use std::{cell::RefCell, path::Path, rc::Rc, sync::Arc};

use zgx::platform::PlatformService;
use zgx_script::{ScriptHost, Value};

/// The largest file read for a copy: past this it is refused, not decoded.
const MAX_BYTES: u64 = 64 * 1024 * 1024;

pub fn install(host: &ScriptHost, platform: Rc<RefCell<Option<PlatformService>>>) {
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

fn path_of(arg: &Value) -> Result<String, String> {
    let path = arg.field("path").map(crate::auth::text).unwrap_or_default();
    if path.is_empty() {
        return Err("no path".into());
    }
    if !Path::new(&path).is_absolute() {
        return Err("That image isn't a file on this Mac".into());
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
    fn only_absolute_paths_are_files_here() {
        assert!(path_of(&Value::record([("path", Value::from("assets/x.png"))])).is_err());
        assert!(path_of(&Value::record([("path", Value::from("/tmp/x.png"))])).is_ok());
    }
}
