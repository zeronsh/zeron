//! File → icon asset, resolved exactly like the desktop (`ui/src/file_icons.rs`,
//! same bundled VS Code-theme manifest): exact basename, then the longest
//! compound extension, then the generic document. Returns the asset name the
//! platform bundles (`fileicon-files-rust`).

use std::collections::HashMap;
use std::sync::LazyLock;

use serde::Deserialize;

#[derive(Deserialize)]
struct Manifest {
    #[serde(rename = "iconDefinitions")]
    icon_definitions: HashMap<String, IconDefinition>,
    file: Option<String>,
    #[serde(default, rename = "fileExtensions")]
    file_extensions: HashMap<String, String>,
    #[serde(default, rename = "fileNames")]
    file_names: HashMap<String, String>,
}

#[derive(Deserialize)]
struct IconDefinition {
    #[serde(rename = "iconPath")]
    icon_path: String,
}

static MANIFEST: LazyLock<Manifest> = LazyLock::new(|| {
    let mut m: Manifest = serde_json::from_str(include_str!("../../../ui/src/file-icons.json")).expect("bundled file-icon manifest");
    m.file_extensions = m.file_extensions.into_iter().map(|(k, v)| (k.to_ascii_lowercase(), v)).collect();
    m.file_names = m.file_names.into_iter().map(|(k, v)| (k.to_ascii_lowercase(), v)).collect();
    m
});

fn definition_asset(definition: &str) -> Option<&'static str> {
    let definition = match definition {
        "less" => "brackets-sky",
        "yml" => "yaml",
        d => d,
    };
    MANIFEST.icon_definitions.get(definition)?.icon_path.strip_prefix("./icons/")
}

pub(crate) fn basename(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

fn resolve(name: &str) -> &'static str {
    if let Some(a) = MANIFEST.file_names.get(name).and_then(|d| definition_asset(d)) {
        return a;
    }
    for (i, _) in name.match_indices('.') {
        let ext = &name[i + 1..];
        if ext.is_empty() {
            continue;
        }
        if let Some(a) = MANIFEST.file_extensions.get(ext).and_then(|d| definition_asset(d)) {
            return a;
        }
    }
    MANIFEST.file.as_deref().and_then(definition_asset).unwrap_or("files/document.svg")
}

/// Asset name for a file path's icon: `fileicon-files-<name>`.
pub(crate) fn file_icon_asset(path: &str) -> String {
    let asset = resolve(&basename(path).to_ascii_lowercase());
    format!("fileicon-{}", asset.trim_end_matches(".svg").replace('/', "-"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_like_desktop() {
        assert_eq!(file_icon_asset("crates/mobile/src/lib.rs"), "fileicon-files-rust");
        assert_eq!(file_icon_asset("/x/Cargo.toml"), file_icon_asset("Cargo.toml"));
        assert!(file_icon_asset("weird.unknownext").starts_with("fileicon-files-"));
    }
}
