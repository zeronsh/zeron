//! Package evidence for the installation a check describes. A manifest
//! establishes package identity, not which global package manager owns it:
//! npm, pnpm and bun can expose the same payload through different shims.
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Package {
    pub root: PathBuf,
    pub name: String,
}

pub(super) fn package_for(launcher: &Path) -> Option<Package> {
    let resolved = launcher.canonicalize().ok()?;
    for root in resolved.parent()?.ancestors() {
        let manifest = root.join("package.json");
        let Ok(bytes) = std::fs::read(manifest) else {
            continue;
        };
        let Ok(json) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            continue;
        };
        let Some(name) = json["name"].as_str() else {
            continue;
        };
        // The selected file must be a declared CLI, not just a file somewhere
        // beneath a directory whose name happens to resemble a package.
        let bin = &json["bin"];
        let entries: Vec<&str> = if let Some(bin) = bin.as_str() {
            vec![bin]
        } else {
            bin.as_object()
                .map(|bins| bins.values().filter_map(|v| v.as_str()).collect())
                .unwrap_or_default()
        };
        if !entries
            .iter()
            .any(|entry| root.join(entry).canonicalize().ok().as_ref() == Some(&resolved))
        {
            continue;
        }
        return Some(Package {
            root: root.to_owned(),
            name: name.into(),
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn package_identity_requires_a_matching_bin_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("node_modules/agent/bin/agent");
        std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
        std::fs::write(&bin, "payload").unwrap();
        assert!(package_for(&bin).is_none());
        let manifest = bin.parent().unwrap().parent().unwrap().join("package.json");
        std::fs::write(
            &manifest,
            r#"{"name":"agent","version":"1.0.0","bin":{"agent":"bin/other"}}"#,
        )
        .unwrap();
        assert!(package_for(&bin).is_none());
        std::fs::write(
            &manifest,
            r#"{"name":"agent","version":"1.0.0","bin":{"agent":"bin/agent"}}"#,
        )
        .unwrap();
        assert_eq!(package_for(&bin).unwrap().name, "agent");
    }
}
