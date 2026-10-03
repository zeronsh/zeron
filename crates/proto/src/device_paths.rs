//! Lexical navigation of absolute folders on another device. `std::path`
//! would interpret remote Windows paths using the UI machine's OS rules.

fn drive_root(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\'
}

fn unc_root_len(path: &str, prefix: usize) -> Option<usize> {
    let server_end = prefix + path.get(prefix..)?.find('\\')?;
    if server_end == prefix {
        return None;
    }
    let share_start = server_end + 1;
    let rest = path.get(share_start..)?;
    let share_end = share_start + rest.find('\\').unwrap_or(rest.len());
    (share_end > share_start).then_some(share_end)
}

/// Return a Windows spelling and its root boundary, preserving extended prefixes.
fn windows_folder(path: &str) -> Option<(String, usize)> {
    let path = path.replace('/', "\\");
    let root = if path
        .get(..8)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(r"\\?\UNC\"))
    {
        unc_root_len(&path, 8)?
    } else if let Some(rest) = path.strip_prefix(r"\\?\") {
        if drive_root(rest) {
            7
        } else {
            return None;
        }
    } else if drive_root(&path) {
        3
    } else if path.starts_with(r"\\") {
        unc_root_len(&path, 2)?
    } else {
        return None;
    };
    Some((path, root))
}

/// Append a folder name using the target path's separators, including `\\?\`.
pub fn child_folder(base: &str, name: &str) -> String {
    if let Some((base, _)) = windows_folder(base) {
        format!(
            "{}\\{}",
            base.trim_end_matches('\\'),
            name.replace('/', "\\")
        )
    } else if base.ends_with('/') {
        format!("{base}{name}")
    } else {
        format!("{base}/{name}")
    }
}

/// Stop at a drive or UNC share root, even across repeated Up operations.
pub fn parent_folder(path: &str) -> Option<String> {
    if let Some((path, root)) = windows_folder(path) {
        let end = path.trim_end_matches('\\').len();
        if end <= root {
            return None;
        }
        let separator = path[..end].rfind('\\')?;
        return Some(path[..separator.max(root)].to_string());
    }
    let path = path.trim_end_matches('/');
    match path.rfind('/')? {
        0 => Some("/".into()),
        index => Some(path[..index].into()),
    }
}

/// Expand either Windows home spelling using the target device's home.
/// Backslashes remain literal filename characters on Unix.
pub fn expand_home(path: &str, home: &str) -> Option<String> {
    if path == "~" {
        return Some(home.into());
    }
    let relative = path
        .strip_prefix("~/")
        .or_else(|| windows_folder(home).and_then(|_| path.strip_prefix(r"~\")))?;
    Some(child_folder(home, relative))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_navigation_preserves_prefixes_and_stops_at_roots() {
        for root in [
            r"C:\",
            r"D:\",
            r"\\?\C:\",
            r"\\server\share",
            r"\\?\UNC\server\share",
        ] {
            let parent = child_folder(root, "Área de trabajo");
            let child = child_folder(&parent, "repo");
            assert!(!child.contains('/'), "{child}");
            assert_eq!(parent_folder(&child), Some(parent.clone()));
            assert_eq!(parent_folder(&parent), Some(root.into()));
            assert_eq!(parent_folder(root), None);
            assert_eq!(parent_folder(&format!("{root}\\")), None);
        }
    }

    #[test]
    fn forward_slash_windows_paths_remain_windows_after_navigation() {
        for (path, root) in [
            ("C:/repos", r"C:\"),
            ("//server/share/repos", r"\\server\share"),
            ("//?/UNC/server/share/repos", r"\\?\UNC\server\share"),
        ] {
            let parent = parent_folder(path).unwrap();
            assert_eq!(parent, root);
            assert_eq!(parent_folder(&parent), None);
        }
        assert_eq!(
            child_folder(r"\\?\C:\repos", "project"),
            r"\\?\C:\repos\project"
        );
    }

    #[test]
    fn home_expansion_uses_the_host_path_format() {
        for home in [r"C:\Users\Ana", r"\\?\C:\Users\Ana", r"\\server\users\Ana"] {
            assert_eq!(
                expand_home(r"~\Worktrees\repo", home),
                Some(child_folder(home, r"Worktrees\repo"))
            );
            assert_eq!(
                expand_home("~/Worktrees/repo", home),
                Some(child_folder(home, r"Worktrees\repo"))
            );
        }
        assert_eq!(
            expand_home("~/worktrees", "/home/ana"),
            Some("/home/ana/worktrees".into())
        );
        assert_eq!(expand_home(r"~\worktrees", "/home/ana"), None);
        assert_eq!(
            expand_home("~", r"C:\Users\Ana"),
            Some(r"C:\Users\Ana".into())
        );
    }

    #[test]
    fn unix_navigation_preserves_backslashes_and_case() {
        assert_eq!(
            child_folder(r"/home/ana/a\b", "Repo"),
            r"/home/ana/a\b/Repo"
        );
        assert_eq!(
            parent_folder(r"/home/ana/a\b/Repo"),
            Some(r"/home/ana/a\b".into())
        );
        assert_eq!(parent_folder("/home"), Some("/".into()));
        assert_eq!(parent_folder("/"), None);
    }
}
