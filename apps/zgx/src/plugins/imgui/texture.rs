//! The pictures a plugin draws: its own files, decoded once, drawn by handle.
//!
//! `imgui.texture{ path = "assets/checker.png" }` reads a file inside the
//! plugin's own folder, decodes it to straight-alpha RGBA8 and returns a
//! handle. Luau never sees a GPU object: the handle is a number the registry
//! maps back to pixels, and the renderer uploads them the first time a frame
//! draws them.
//!
//! A handle is only good in the plugin that made it: each plugin has its own
//! registry, and an id that is not in it — another plugin's, a made-up
//! number, or a path string — is refused instead of drawn.

use std::cell::{Cell, RefCell};
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;

/// The largest file a plugin may hand the renderer: a picture bigger than
/// this is a mistake, not a texture.
pub const MAX_TEXTURE_BYTES: u64 = 32 * 1024 * 1024;

/// The widest and tallest texture a plugin may load.
pub const MAX_TEXTURE_SIDE: u32 = 8192;

/// The first id a plugin picture gets: high enough that no font atlas
/// (Dear ImGui numbers those from zero) and no other plugin's picture can be
/// mistaken for it.
const FIRST_TEXTURE: u64 = 1 << 40;

thread_local! {
    /// Hands out texture ids no two plugins share, so one plugin's handle is
    /// never found in another's registry by accident.
    static NEXT_TEXTURE: Cell<u64> = const { Cell::new(FIRST_TEXTURE) };
}

fn next_texture_id() -> u64 {
    NEXT_TEXTURE.with(|next| {
        let id = next.get() + 1;
        next.set(id);
        id
    })
}

/// A plugin's picture, decoded: RGBA8, rows top to bottom, straight alpha.
#[derive(Debug, PartialEq, Eq)]
pub struct TextureImage {
    pub id: u64,
    /// The path the plugin named, as `imgui.texture_size` and diagnostics
    /// report it.
    pub path: String,
    pub width: u32,
    pub height: u32,
    pixels: Vec<u8>,
}

impl TextureImage {
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }
}

/// Reads a file a plugin named, inside the plugin's own folder.
///
/// A path that leaves the folder (`..`, an absolute one, a symlinked
/// directory above it) is refused: a plugin draws its own pictures, not the
/// person's files.
pub fn read_plugin_file(root: &Path, path: &str, kind: &str) -> Result<Vec<u8>, String> {
    let named = Path::new(path);
    if named.as_os_str().is_empty() {
        return Err(format!("{kind} needs a path inside the plugin's folder."));
    }
    let mut relative = PathBuf::new();
    for part in named.components() {
        match part {
            Component::Normal(part) => relative.push(part),
            Component::CurDir => {}
            _ => {
                return Err(format!(
                    "can't use \"{path}\": it leaves the plugin's own folder. {kind} is read from there."
                ));
            }
        }
    }
    let full = root.join(&relative);
    let Ok(canonical) = full.canonicalize() else {
        return Err(format!("there is no \"{path}\" in this plugin."));
    };
    let Ok(root) = root.canonicalize() else {
        return Err(format!(
            "this plugin's folder is gone, so \"{path}\" can't be read."
        ));
    };
    if !canonical.starts_with(&root) {
        return Err(format!(
            "can't use \"{path}\": it leaves the plugin's own folder. {kind} is read from there."
        ));
    }
    let meta = std::fs::metadata(&canonical)
        .map_err(|_| format!("there is no \"{path}\" in this plugin."))?;
    if !meta.is_file() {
        return Err(format!("\"{path}\" is not a file this plugin can read."));
    }
    if meta.len() > MAX_TEXTURE_BYTES {
        return Err(format!("{kind} \"{path}\" is too large to load."));
    }
    std::fs::read(&canonical).map_err(|error| format!("couldn't read \"{path}\": {error}"))
}

/// A plugin's pictures, by id.
#[derive(Default)]
pub struct PluginTextures {
    images: RefCell<Vec<Rc<TextureImage>>>,
}

impl PluginTextures {
    pub fn new() -> Self {
        Self::default()
    }

    /// `imgui.texture{ path }`: the file decoded now, so a broken picture is
    /// refused where the plugin asked for it.
    pub fn register(&self, root: &Path, path: &str) -> Result<Rc<TextureImage>, String> {
        let bytes = read_plugin_file(root, path, "a picture")?;
        let (width, height, pixels) = decode(&bytes, path)?;
        let image = Rc::new(TextureImage {
            id: next_texture_id(),
            path: path.to_string(),
            width,
            height,
            pixels,
        });
        self.images.borrow_mut().push(image.clone());
        Ok(image)
    }

    /// The picture `id` names, if this plugin made it.
    pub fn get(&self, id: u64) -> Option<Rc<TextureImage>> {
        self.images
            .borrow()
            .iter()
            .find(|image| image.id == id)
            .cloned()
    }

    /// Every picture this plugin loaded, for a frame that must draw them.
    pub fn all(&self) -> Vec<Rc<TextureImage>> {
        self.images.borrow().clone()
    }

    /// Whether `id` is one of this plugin's pictures.
    pub fn holds(&self, id: u64) -> bool {
        self.images.borrow().iter().any(|image| image.id == id)
    }
}

/// A PNG or JPEG as straight-alpha RGBA8.
fn decode(bytes: &[u8], path: &str) -> Result<(u32, u32, Vec<u8>), String> {
    let refused = |why: String| format!("couldn't read the picture \"{path}\": {why}");
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let png_only = decoder.read_info();
    match png_only {
        Ok(mut reader) => {
            let (width, height) = {
                let info = reader.info();
                (info.width, info.height)
            };
            if width == 0 || height == 0 {
                return Err(refused("the picture has no pixels".into()));
            }
            if width > MAX_TEXTURE_SIDE || height > MAX_TEXTURE_SIDE {
                return Err(refused(format!(
                    "the picture is larger than {MAX_TEXTURE_SIDE} by {MAX_TEXTURE_SIDE}"
                )));
            }
            let mut buffer = vec![
                0;
                reader
                    .output_buffer_size()
                    .ok_or_else(|| refused("the picture is too large".into()))?
            ];
            reader
                .next_frame(&mut buffer)
                .map_err(|error| refused(error.to_string()))?;
            let (colour, _) = reader.output_color_type();
            let pixels = match colour {
                png::ColorType::Rgba => buffer,
                png::ColorType::Rgb => widen(&buffer, 3, width, height),
                png::ColorType::GrayscaleAlpha => widen(&buffer, 2, width, height),
                png::ColorType::Grayscale => widen(&buffer, 1, width, height),
                png::ColorType::Indexed => {
                    return Err(refused("the palette was not expanded".into()));
                }
            };
            Ok((width, height, pixels))
        }
        Err(png_error) => {
            let decoded = zgx_gpu::assets::decode_image(bytes)
                .map_err(|error| refused(format!("{png_error}; {error}")))?;
            let (width, height) = (decoded.width(), decoded.height());
            if width == 0 || height == 0 {
                return Err(refused("the picture has no pixels".into()));
            }
            if width > MAX_TEXTURE_SIDE || height > MAX_TEXTURE_SIDE {
                return Err(refused(format!(
                    "the picture is larger than {MAX_TEXTURE_SIDE} by {MAX_TEXTURE_SIDE}"
                )));
            }
            Ok((width, height, decoded.pixels().to_vec()))
        }
    }
}

/// `channels` interleaved samples per pixel, widened to RGBA with opaque
/// alpha (`2` is grey with its own alpha).
fn widen(buffer: &[u8], channels: usize, width: u32, height: u32) -> Vec<u8> {
    let pixels = width as usize * height as usize;
    let mut out = Vec::with_capacity(pixels * 4);
    for pixel in 0..pixels {
        let at = pixel * channels;
        match channels {
            1 => out.extend_from_slice(&[buffer[at], buffer[at], buffer[at], 255]),
            2 => out.extend_from_slice(&[buffer[at], buffer[at], buffer[at], buffer[at + 1]]),
            _ => out.extend_from_slice(&[buffer[at], buffer[at + 1], buffer[at + 2], 255]),
        }
    }
    out
}
