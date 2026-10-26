//! Small, static project thumbnails for native image views.

pub(crate) fn thumbnail(mime: &str, bytes: Vec<u8>) -> Result<Vec<u8>, String> {
    if bytes.len() > zeron_proto::MAX_WORKSPACE_IMAGE_BYTES {
        return Err("Icon is too large".into());
    }
    if mime == "image/svg+xml" {
        let options = resvg::usvg::Options {
            // Never resolve filesystem or network references in repository SVGs.
            image_href_resolver: resvg::usvg::ImageHrefResolver {
                resolve_data: Box::new(|_, _, _| None),
                resolve_string: Box::new(|_, _| None),
            },
            ..Default::default()
        };
        let tree = resvg::usvg::Tree::from_data(&bytes, &options).map_err(|_| "Invalid SVG")?;
        let size = tree.size();
        let scale = 64.0 / size.width().max(size.height());
        let mut pixmap = resvg::tiny_skia::Pixmap::new(
            (size.width() * scale).ceil().max(1.0) as u32,
            (size.height() * scale).ceil().max(1.0) as u32,
        )
        .ok_or("Invalid icon size")?;
        resvg::render(
            &tree,
            resvg::tiny_skia::Transform::from_scale(scale, scale),
            &mut pixmap.as_mut(),
        );
        return pixmap
            .encode_png()
            .map_err(|_| "Could not encode icon".into());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn svg_becomes_a_bounded_png() {
        let bytes = thumbnail("image/svg+xml", br##"<svg xmlns="http://www.w3.org/2000/svg" width="1000" height="500"><rect width="1000" height="500" fill="#f00"/></svg>"##.to_vec()).unwrap();
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(u32::from_be_bytes(bytes[16..20].try_into().unwrap()), 64);
        assert_eq!(u32::from_be_bytes(bytes[20..24].try_into().unwrap()), 32);
    }
}
