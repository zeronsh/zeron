//! One decoding contract for background installation and rendering. Attachment
//! formats are broader (notably SVG), so attachment staging is not validation.

pub(crate) fn decode(bytes: &[u8]) -> image::ImageResult<image::DynamicImage> {
    // Inspect the exact bytes that will be saved, not the source extension or
    // a second read of a file that could change between validation and copy.
    image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()?
        .decode()
}
