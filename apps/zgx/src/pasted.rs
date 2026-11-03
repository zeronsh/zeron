//! A picture pasted into the composer (a screenshot, an image editor's or a
//! browser's copy): the clipboard holds pixels, not a file, and a message's
//! attachments are files, so the host writes it out as a PNG and the
//! composer carries that, as Zeron's composer does (`paste_image`).
//!
//!   chat.paste_image   the clipboard's image as a PNG in this app's own
//!                      temporary folder: its path, or nil when the
//!                      clipboard holds no image

use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    rc::Rc,
};

use zgx::platform::PlatformService;
use zgx_script::{ScriptHost, Value};

pub fn install(host: &ScriptHost, platform: Rc<RefCell<Option<PlatformService>>>) {
    host.async_action("chat.paste_image", move |_| {
        let platform = platform.clone();
        async move {
            let service = platform.borrow().clone().ok_or("No clipboard here")?;
            let Some(image) = service.0.clipboard_image().await else {
                return Ok(Value::Nil);
            };
            let (w, h) = (image.width(), image.height());
            let pixels = image.pixels().to_vec();
            let path = off_thread(move || write_png(&folder(), w, h, &pixels))
                .await
                .ok_or("The picture couldn't be saved")??;
            Ok(Value::from(path.to_string_lossy().as_ref()))
        }
    });
}

/// Where pasted pictures are kept until they're sent: this app's own folder
/// in the system's temporary one.
fn folder() -> PathBuf {
    std::env::temp_dir().join("zeron-zgx-pasted")
}

/// `pixels` (RGBA8, `w` by `h`) as a PNG of its own in `dir`, named for when
/// it was pasted: its path.
fn write_png(dir: &Path, w: u32, h: u32, pixels: &[u8]) -> Result<PathBuf, String> {
    if w == 0 || h == 0 || pixels.len() != w as usize * h as usize * 4 {
        return Err("The clipboard's picture is empty".into());
    }
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default();
    let mut n = 0;
    let path = loop {
        let name = if n == 0 { format!("Pasted image {stamp}.png") } else { format!("Pasted image {stamp}-{n}.png") };
        let path = dir.join(name);
        if !path.exists() {
            break path;
        }
        n += 1;
    };
    let file = std::fs::File::create(&path).map_err(|e| e.to_string())?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(pixels).map_err(|e| e.to_string())?;
    writer.finish().map_err(|e| e.to_string())?;
    Ok(path)
}

/// `work` on a thread of its own, its answer awaited here (None only if it
/// panicked), as images.rs's.
async fn off_thread<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Option<T> {
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
    fn a_pasted_picture_is_written_as_a_png_of_its_own() {
        let dir = std::env::temp_dir().join(format!("zeron-zgx-pasted-test-{}", std::process::id()));
        let pixels = [255u8, 0, 0, 255, 0, 0, 255, 128];
        let a = write_png(&dir, 2, 1, &pixels).expect("written");
        let b = write_png(&dir, 2, 1, &pixels).expect("written again");
        assert_ne!(a, b, "a second paste doesn't overwrite the first");
        assert!(a.extension().is_some_and(|e| e == "png"));
        let decoded = zgx_gpu::assets::decode_image(&std::fs::read(&a).unwrap()).expect("a PNG");
        assert_eq!((decoded.width(), decoded.height()), (2, 1));
        assert_eq!(&decoded.pixels()[..4], &[255, 0, 0, 255]);
        assert!(write_png(&dir, 2, 2, &pixels).is_err(), "pixels that don't fill it");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
