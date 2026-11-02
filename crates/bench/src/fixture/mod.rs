pub mod claude_home;
pub mod claude_model;
pub mod multi;
pub mod profile;
pub mod real;
mod repo;
pub mod scripts;
pub mod text;

/// Resolve an existing bench root before creating any fixture files.
pub fn safe_root() -> anyhow::Result<std::path::PathBuf> {
    use anyhow::{Context, ensure};
    let path = crate::default_zb_root();
    let root = std::fs::canonicalize(&path)
        .with_context(|| format!("bench root {} must already exist", path.display()))?;
    ensure!(root.is_dir(), "bench root is not a directory");
    ensure!(
        !root.starts_with("/tmp") && !root.components().any(|c| c.as_os_str() == ".zeron"),
        "unsafe bench root"
    );
    Ok(root)
}

/// Create only real directories below the resolved bench root, never following links.
pub fn fixture_dir(
    root: &std::path::Path,
    components: &[&str],
) -> anyhow::Result<std::path::PathBuf> {
    use anyhow::{bail, ensure};
    let mut path = root.to_path_buf();
    for part in components {
        ensure!(
            !part.is_empty() && *part != "." && *part != ".." && !part.contains('/'),
            "invalid directory component"
        );
        path.push(part);
        match std::fs::create_dir(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
        let meta = std::fs::symlink_metadata(&path)?;
        if !meta.is_dir() || meta.file_type().is_symlink() {
            bail!("unsafe fixture directory {}", path.display());
        }
    }
    Ok(path)
}

pub(crate) fn assistant_text(view: &crate::transcript::ChatView) -> String {
    view.entries
        .iter()
        .filter(|e| e["role"] == "assistant")
        .flat_map(|e| e["parts"].as_array().into_iter().flatten())
        .filter(|p| p["kind"] == "text")
        .filter_map(|p| p["text"].as_str())
        .collect()
}
