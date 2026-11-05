//! Decoding image assets for textures and environment maps.

use crate::scene::TextureData;

/// Largest side an image asset may have.
pub const MAX_IMAGE_SIDE: u32 = 8192;
/// Most pixels an image may decode to (64 MB as RGBA8).
pub const MAX_IMAGE_PIXELS: u64 = 4096 * 4096;
/// Most pixels for HDR, which decodes to four floats a pixel (128 MB).
pub const MAX_HDR_PIXELS: u64 = 4096 * 2048;

/// A decoded image, top row first.
#[derive(Debug, Clone)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub pixels: Pixels,
}

#[derive(Debug, Clone)]
pub enum Pixels {
    /// sRGB-encoded RGBA, straight alpha.
    Rgba8(Vec<u8>),
    /// Linear RGBA light (HDR environment maps).
    RgbaF32(Vec<f32>),
}

fn reader(bytes: &[u8]) -> Result<image::ImageReader<std::io::Cursor<&[u8]>>, String> {
    image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| format!("unreadable image: {e}"))
}

/// The image's size, without decoding it: a cheap validity check.
pub fn dimensions(bytes: &[u8]) -> Result<(u32, u32), String> {
    let reader = reader(bytes)?;
    if reader.format().is_none() {
        return Err("not a PNG, JPEG, WebP, or Radiance HDR image".into());
    }
    let hdr = reader.format() == Some(image::ImageFormat::Hdr);
    let (width, height) = reader
        .into_dimensions()
        .map_err(|e| format!("unreadable image: {e}"))?;
    check_size(width, height, hdr)?;
    Ok((width, height))
}

/// Sides within 1..=MAX_IMAGE_SIDE and pixels within the decode budget.
fn check_size(width: u32, height: u32, hdr: bool) -> Result<(), String> {
    if width == 0 || height == 0 || width.max(height) > MAX_IMAGE_SIDE {
        return Err(format!(
            "a {width}x{height} image; sides run 1 to {MAX_IMAGE_SIDE} pixels"
        ));
    }
    let budget = if hdr {
        MAX_HDR_PIXELS
    } else {
        MAX_IMAGE_PIXELS
    };
    if u64::from(width) * u64::from(height) > budget {
        return Err(format!(
            "a {width}x{height} image; at most {budget} pixels{}",
            if hdr { " for HDR" } else { "" }
        ));
    }
    Ok(())
}

/// Decode an encoded image asset.
pub fn decode(bytes: &[u8]) -> Result<Image, String> {
    dimensions(bytes)?;
    let reader = reader(bytes)?;
    let hdr = reader.format() == Some(image::ImageFormat::Hdr);
    let image = reader
        .decode()
        .map_err(|e| format!("unreadable image: {e}"))?;
    let (width, height) = (image.width(), image.height());
    let pixels = if hdr {
        Pixels::RgbaF32(image.into_rgba32f().into_raw())
    } else {
        Pixels::Rgba8(image.into_rgba8().into_raw())
    };
    Ok(Image {
        width,
        height,
        pixels,
    })
}

/// Decode any texture a scene uses.
pub fn decode_texture(data: &TextureData) -> Result<Image, String> {
    match data {
        TextureData::Encoded(bytes) => decode(bytes),
        TextureData::Rgba {
            width,
            height,
            pixels,
        } => {
            check_size(*width, *height, false)?;
            if pixels.len() != *width as usize * *height as usize * 4 {
                return Err("image pixels don't match its size".into());
            }
            Ok(Image {
                width: *width,
                height: *height,
                pixels: Pixels::Rgba8(pixels.to_vec()),
            })
        }
    }
}
