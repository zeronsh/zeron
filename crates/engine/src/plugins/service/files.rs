//! `zeron.write_file` ("files.write"): a service writes a file where you'd
//! look for it, and nowhere else.
//!
//! - A relative path lands in the plugin's own folder in Documents:
//!   `~/Documents/<plugin name>/<path>` ("Design: …" exports).
//! - A whole path must be inside one of this device's chats' or projects'
//!   folders (where an agent's work is).
//!
//! Folders on the way are made. A path through a link that leads out of
//! those places, or onto a link, is refused, and so is `..`.

use std::path::{Component, Path, PathBuf};

use base64::Engine as _;
use serde_json::{Value as Json, json};

/// A file's size at most.
pub const MAX_BYTES: usize = 64 << 20;

/// Writes `args` (`{ path, data, base64? }`) for plugin `name`: `{ path, bytes }`
/// with the file's whole path. `folders`: this device's chats' and
/// projects' folders; `home`: the person's home.
pub fn write(args: &Json, name: &str, folders: &[String], home: &Path) -> Result<Json, String> {
    let path = args["path"].as_str().filter(|p| !p.trim().is_empty()).ok_or("zeron.write_file needs a path")?;
    let data = args["data"].as_str().ok_or("zeron.write_file needs data (a string)")?;
    let bytes = if args["base64"].as_bool() == Some(true) {
        base64::engine::general_purpose::STANDARD
            .decode(data.trim())
            .map_err(|e| format!("zeron.write_file: data isn't base64 ({e})"))?
    } else {
        data.as_bytes().to_vec()
    };
    if bytes.len() > MAX_BYTES {
        return Err(format!("zeron.write_file: {} MB at most", MAX_BYTES >> 20));
    }
    let asked = Path::new(path);
    if asked.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(format!("zeron.write_file: \"{path}\" can't go up a folder (..)"));
    }
    let (root, target) = if asked.is_absolute() {
        let root = folders
            .iter()
            .map(PathBuf::from)
            .filter(|f| f.is_absolute())
            .find(|f| asked.starts_with(f))
            .ok_or_else(|| format!("zeron.write_file: \"{path}\" isn't inside one of your chats' or projects' folders"))?;
        (root, asked.to_path_buf())
    } else {
        let root = home.join("Documents").join(folder_name(name));
        (root.clone(), root.join(asked))
    };
    if target == root || target.file_name().is_none() {
        return Err(format!("zeron.write_file: \"{path}\" names a folder, not a file"));
    }
    let parent = target.parent().ok_or("zeron.write_file: no folder to write in")?;
    std::fs::create_dir_all(parent).map_err(|e| format!("zeron.write_file: couldn't make {}: {e}", parent.display()))?;
    // Where the folders really are, links followed: still inside.
    let real_root = std::fs::canonicalize(&root).map_err(|e| format!("zeron.write_file: {e}"))?;
    let real_parent = std::fs::canonicalize(parent).map_err(|e| format!("zeron.write_file: {e}"))?;
    if !real_parent.starts_with(&real_root) {
        return Err(format!("zeron.write_file: \"{path}\" goes through a link out of {}", root.display()));
    }
    let file = real_parent.join(target.file_name().expect("checked above"));
    if let Ok(meta) = std::fs::symlink_metadata(&file) {
        if meta.file_type().is_symlink() {
            return Err(format!("zeron.write_file: \"{path}\" is a link; it won't write through it"));
        }
        if meta.is_dir() {
            return Err(format!("zeron.write_file: \"{path}\" is a folder"));
        }
    }
    std::fs::write(&file, &bytes).map_err(|e| format!("zeron.write_file: couldn't write {}: {e}", file.display()))?;
    // (Said as asked: canonical paths on Windows read `\\?\C:\…`.)
    Ok(json!({ "path": target.to_string_lossy(), "bytes": bytes.len() }))
}

/// The plugin's name as a folder's: what Windows and macOS refuse in a
/// name, made `-`.
fn folder_name(name: &str) -> String {
    let clean: String = name
        .chars()
        .map(|c| if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control() { '-' } else { c })
        .collect();
    let clean = clean.trim().trim_matches('.').trim().to_owned();
    if clean.is_empty() { "Plugin".into() } else { clean }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(home: &Path, folders: &[&Path], path: &str, data: &str, b64: bool) -> Result<Json, String> {
        let folders: Vec<String> = folders.iter().map(|f| f.to_string_lossy().into_owned()).collect();
        write(&json!({ "path": path, "data": data, "base64": b64 }), "Design: pure?", &folders, home)
    }

    #[test]
    fn a_relative_path_lands_in_the_plugins_documents_folder() {
        let home = tempfile::tempdir().unwrap();
        let r = w(home.path(), &[], "Shop/01-home.png", "aGk=", true).unwrap();
        let file = home.path().join("Documents").join("Design- pure-").join("Shop").join("01-home.png");
        assert_eq!(std::fs::read(&file).unwrap(), b"hi");
        assert_eq!(r["bytes"], 2);
        assert_eq!(std::fs::canonicalize(r["path"].as_str().unwrap()).unwrap(), std::fs::canonicalize(&file).unwrap());
        // Written again: replaced.
        w(home.path(), &[], "Shop/01-home.png", "<p>", false).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "<p>");
    }

    #[test]
    fn a_whole_path_must_be_in_a_chats_folder() {
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let inside = project.path().join("design").join("index.html");
        w(home.path(), &[project.path()], &inside.to_string_lossy(), "<html>", false).unwrap();
        assert_eq!(std::fs::read_to_string(&inside).unwrap(), "<html>");
        let out = elsewhere.path().join("x.html");
        let e = w(home.path(), &[project.path()], &out.to_string_lossy(), "x", false).unwrap_err();
        assert!(e.contains("isn't inside"), "{e}");
        assert!(!out.exists());
    }

    #[test]
    fn up_a_folder_a_folder_and_bad_data_are_refused() {
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        for (path, data, b64) in [
            ("../../escape.txt", "x", false),
            ("a/../../escape.txt", "x", false),
            ("", "x", false),
            ("a.png", "not base64!", true),
        ] {
            assert!(w(home.path(), &[], path, data, b64).is_err(), "{path}");
        }
        let sneaky = format!("{}/../escape.txt", project.path().display());
        assert!(w(home.path(), &[project.path()], &sneaky, "x", false).is_err());
        assert!(!project.path().parent().unwrap().join("escape.txt").exists());
        std::fs::create_dir_all(home.path().join("Documents/Design- pure-/dir")).unwrap();
        assert!(w(home.path(), &[], "dir", "x", false).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn links_out_are_refused() {
        let home = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let mine = home.path().join("Documents/Design- pure-");
        std::fs::create_dir_all(&mine).unwrap();
        // A folder that's a link out, and a file that's a link.
        std::os::unix::fs::symlink(outside.path(), mine.join("out")).unwrap();
        let e = w(home.path(), &[], "out/x.txt", "x", false).unwrap_err();
        assert!(e.contains("link"), "{e}");
        assert!(!outside.path().join("x.txt").exists());
        let target = outside.path().join("victim.txt");
        std::fs::write(&target, "keep").unwrap();
        std::os::unix::fs::symlink(&target, mine.join("v.txt")).unwrap();
        assert!(w(home.path(), &[], "v.txt", "x", false).is_err());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "keep");
    }
}
