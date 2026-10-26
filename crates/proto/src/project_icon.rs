//! Checkout-relative project artwork paths.

pub const ICON_PATHS: &[&str] = &[
    "public/apple-touch-icon.png",
    "apple-touch-icon.png",
    "public/favicon.svg",
    "favicon.svg",
    "public/favicon.png",
    "public/icon.png",
    "public/logo.png",
    "favicon.png",
    "app/icon.png",
    "src/app/icon.png",
    "public/favicon.ico",
    "favicon.ico",
    "app/favicon.ico",
    "static/favicon.ico",
    "src-tauri/icons/icon.png",
    "assets/icon.png",
    "src/assets/icon.png",
];

pub fn normalize_path(path: Option<&str>) -> Result<Option<String>, &'static str> {
    let Some(path) = path.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    if path.len() > 1024
        || path.starts_with('/')
        || path.contains(['\\', ':'])
        || path.chars().any(char::is_control)
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err("Use a path inside the project, such as apps/web/public/icon.png");
    }
    let extension = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    if !matches!(
        extension.as_str(),
        "png" | "svg" | "ico" | "jpg" | "jpeg" | "webp" | "gif"
    ) {
        return Err("Choose a PNG, SVG, ICO, JPEG, WebP or GIF image");
    }
    Ok(Some(path.to_owned()))
}

pub fn candidates(icon_path: Option<&str>) -> Vec<&str> {
    match icon_path {
        Some(path) => vec![path],
        None => ICON_PATHS.to_vec(),
    }
}

pub const IMAGE_GLOB: &str = "*.{png,svg,ico,jpg,jpeg,webp,gif}";

pub fn browse_query(filter: &str) -> String {
    format!("{} {IMAGE_GLOB}", filter.trim()).trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn artwork_stays_inside_the_checkout() {
        assert_eq!(
            normalize_path(Some(" apps/web/icon.png "))
                .unwrap()
                .as_deref(),
            Some("apps/web/icon.png")
        );
        assert_eq!(normalize_path(Some(" ")).unwrap(), None);
        for path in [
            "/tmp/icon.png",
            "../icon.png",
            "a/../icon.png",
            "C:\\icon.png",
            "https://example.com/icon.png",
            "a//icon.png",
            "file.txt",
        ] {
            assert!(normalize_path(Some(path)).is_err(), "{path}");
        }
    }
}
