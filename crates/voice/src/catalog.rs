//! Verified backend/model combinations. A model id never accepts a path or URL.
use super::backend::{DictationBackend, Parakeet};
use anyhow::{Result, bail};
use std::path::{Path, PathBuf};

pub const PARAKEET_V3: &str = "parakeet-tdt-0.6b-v3-int8";
#[derive(Clone, Debug, serde::Serialize)]
pub struct Model {
    pub id: &'static str,
    pub backend: &'static str,
    pub name: &'static str,
    pub languages: &'static [&'static str],
    pub download_bytes: u64,
    pub capabilities: super::backend::Capabilities,
}
pub fn models() -> Vec<Model> {
    vec![Model {
        id: PARAKEET_V3,
        backend: "parakeet",
        name: "Parakeet TDT 0.6B v3 (INT8)",
        languages: &[
            "bg", "hr", "cs", "da", "nl", "en", "et", "fi", "fr", "de", "el", "hu", "it", "lv",
            "lt", "mt", "pl", "pt", "ro", "ru", "sk", "sl", "es", "sv", "uk",
        ],
        download_bytes: super::download_size(),
        capabilities: Parakeet.capabilities(),
    }]
}
pub fn model(id: &str) -> Result<Model> {
    models()
        .into_iter()
        .find(|m| m.id == id)
        .ok_or_else(|| anyhow::anyhow!("Unsupported dictation model"))
}
pub fn backend(id: &str) -> Result<Box<dyn DictationBackend>> {
    match id {
        "parakeet" => Ok(Box::new(Parakeet)),
        _ => bail!("Unsupported dictation backend"),
    }
}
pub fn directory(root: &Path, id: &str) -> Result<PathBuf> {
    model(id)?;
    Ok(root.join(id))
}
/// Installation dispatch belongs to the catalog, not to a UI or plugin.
pub fn installed(root: &Path, id: &str) -> Result<bool> {
    let dir = directory(root, id)?;
    match id {
        PARAKEET_V3 => Ok(super::installed(&dir)),
        _ => bail!("No installer for this dictation model"),
    }
}
pub fn download(
    root: &Path,
    id: &str,
    cancel: &std::sync::atomic::AtomicBool,
    progress: impl FnMut(u64),
) -> Result<()> {
    let dir = directory(root, id)?;
    match id {
        PARAKEET_V3 => super::download(&dir, cancel, progress),
        _ => bail!("No installer for this dictation model"),
    }
}

/// The caller serializes model operations against capture and downloads.
pub fn remove(root: &Path, id: &str) -> Result<()> {
    let dir = directory(root, id)?;
    if super::busy() {
        bail!("Dictation is busy; retry when transcription finishes")
    }
    super::unload();
    if dir.exists() {
        std::fs::remove_dir_all(dir)?;
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catalog_only_resolves_verified_combinations() {
        let m = model(PARAKEET_V3).unwrap();
        assert_eq!(m.download_bytes, 670479942);
        assert!(m.languages.contains(&"es"));
        assert!(m.capabilities.local);
        assert!(!m.capabilities.partial_results);
        assert_eq!(backend(m.backend).unwrap().id(), m.backend);
        for bad in ["../model", "/tmp/model", "https://model", "whisper"] {
            assert!(directory(Path::new("cache"), bad).is_err());
            assert!(backend(bad).is_err());
        }
    }
}
