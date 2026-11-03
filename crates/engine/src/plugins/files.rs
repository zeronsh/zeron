//! The files a plugin is made of (contract "Files hashed").

use std::path::Path;

use super::manifest::MANIFEST_FILE;
use super::signing::{FileHashes, sha256_hex};

pub const MAX_FILES: usize = 2_000;
pub const MAX_BYTES: u64 = 50 * 1024 * 1024;
pub const LINK_ERROR: &str = "Plugins can't contain links.";

/// Every regular file under `root` (recursively) as `/`-separated relative
/// paths → hex sha256, except the manifest and anything under a dot-named
/// component. A symlink anywhere hashed (the root included) is an error.
pub fn hash_files(root: &Path) -> Result<FileHashes, String> {
    let meta =
        std::fs::symlink_metadata(root).map_err(|_| "Its folder can't be read".to_string())?;
    if meta.file_type().is_symlink() {
        return Err(LINK_ERROR.into());
    }
    let mut files = FileHashes::new();
    let mut total = 0u64;
    walk(root, "", &mut files, &mut total)?;
    Ok(files)
}

fn walk(dir: &Path, prefix: &str, files: &mut FileHashes, total: &mut u64) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|_| "Its folder can't be read".to_string())?;
    for entry in entries {
        let entry = entry.map_err(|_| "Its folder can't be read".to_string())?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            return Err("It has a file whose name isn't valid text".into());
        };
        if name.starts_with('.') {
            continue;
        }
        let rel = if prefix.is_empty() {
            name.to_owned()
        } else {
            format!("{prefix}/{name}")
        };
        let file_type = entry
            .file_type()
            .map_err(|_| "Its folder can't be read".to_string())?;
        if file_type.is_symlink() {
            return Err(LINK_ERROR.into());
        }
        if file_type.is_dir() {
            walk(&entry.path(), &rel, files, total)?;
            continue;
        }
        if !file_type.is_file() {
            // Sockets, fifos: not part of a plugin.
            continue;
        }
        if rel == MANIFEST_FILE {
            continue;
        }
        if files.len() >= MAX_FILES {
            return Err(format!("It has more than {MAX_FILES} files"));
        }
        let bytes =
            std::fs::read(entry.path()).map_err(|_| format!("Its file {rel} can't be read"))?;
        *total += bytes.len() as u64;
        if *total > MAX_BYTES {
            return Err("It's bigger than 50 MB".into());
        }
        files.insert(rel, sha256_hex(&bytes));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugins_hash_files_skips_manifest_and_dotfiles() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(MANIFEST_FILE), "{}").unwrap();
        std::fs::write(dir.path().join("init.luau"), "a").unwrap();
        std::fs::write(dir.path().join(".DS_Store"), "x").unwrap();
        std::fs::create_dir_all(dir.path().join(".git/objects")).unwrap();
        std::fs::write(dir.path().join(".git/objects/1"), "x").unwrap();
        std::fs::create_dir_all(dir.path().join("lib/deep")).unwrap();
        std::fs::write(dir.path().join("lib/deep/x.luau"), "b").unwrap();
        std::fs::write(dir.path().join("lib/.hidden"), "c").unwrap();
        let files = hash_files(dir.path()).unwrap();
        assert_eq!(
            files.keys().cloned().collect::<Vec<_>>(),
            vec!["init.luau".to_string(), "lib/deep/x.luau".to_string()]
        );
        assert_eq!(files["init.luau"], sha256_hex(b"a"));
    }

    #[cfg(unix)]
    #[test]
    fn plugins_symlinks_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("init.luau"), "a").unwrap();
        std::fs::create_dir(dir.path().join("lib")).unwrap();
        std::os::unix::fs::symlink("/etc/hosts", dir.path().join("lib/hosts")).unwrap();
        assert_eq!(hash_files(dir.path()).unwrap_err(), LINK_ERROR);

        // A linked folder too, and a plugin folder that is itself a link.
        let other = tempfile::tempdir().unwrap();
        std::fs::write(other.path().join("init.luau"), "a").unwrap();
        let link = dir.path().join("linked");
        std::os::unix::fs::symlink(other.path(), &link).unwrap();
        assert_eq!(hash_files(&link).unwrap_err(), LINK_ERROR);
    }
}
