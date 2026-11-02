//! One decoding contract for background installation and rendering. Attachment
//! formats are broader (notably SVG), so attachment staging is not validation.

pub(crate) fn decode(bytes: &[u8]) -> image::ImageResult<image::DynamicImage> {
    // Inspect the exact bytes that will be saved, not the source extension or
    // a second read of a file that could change between validation and copy.
    image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()?
        .decode()
}

/// Turn decoded pixels clockwise by a quarter turn. The settings normalize the
/// angle, so anything else passes through untouched.
pub(crate) fn rotate(image: image::RgbaImage, rotation: u16) -> image::RgbaImage {
    match crate::settings::normalize_rotation(rotation) {
        90 => image::imageops::rotate90(&image),
        180 => image::imageops::rotate180(&image),
        270 => image::imageops::rotate270(&image),
        _ => image,
    }
}
