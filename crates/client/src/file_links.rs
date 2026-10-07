//! File links in agent messages (`[report.md](zeron-file:docs/report.md)`,
//! `[x](/home/me/proj/x.md:12)`, `file:///C:/proj/x.md`, `C:\proj\x.md`)
//! resolved to a path the engine's `ReadWorkspaceFile` accepts: relative to
//! the chat's workspace, never escaping it.

/// A text file read from the chat's workspace on its computer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceFile {
    /// Workspace-relative path that was read.
    pub path: String,
    /// `None` for a binary file.
    pub text: Option<String>,
    pub size: u64,
    /// The engine cut a large file short.
    pub truncated: bool,
}

/// Is `url` a link to a file (rather than a web page or app link)?
pub fn is_file_link(url: &str) -> bool {
    let u = url.trim();
    let lower = u.to_ascii_lowercase();
    if lower.starts_with("zeron-file:") || lower.starts_with("file:") {
        return true;
    }
    match lower.split_once(':') {
        // `C:\…` / `C:/…`: a drive letter, not a scheme.
        Some((scheme, _)) if scheme.len() == 1 => true,
        Some((scheme, _))
            if scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c)) =>
        {
            // `docs/x.md:12` / `README.md:12`: a path with a line ref, not a scheme.
            scheme.contains('/') || scheme.contains('.')
        }
        _ => true,
    }
}

/// The workspace-relative path `url` points at, given the workspace `roots`
/// (the chat's checkout first). `None` when it's not a file link, or it
/// points outside every root (the engine only serves workspace files).
pub fn workspace_link_path(url: &str, roots: &[&str]) -> Option<String> {
    if !is_file_link(url) {
        return None;
    }
    let mut path = url.trim().to_owned();
    let lower = path.to_ascii_lowercase();
    if lower.starts_with("zeron-file:") {
        path = path["zeron-file:".len()..].to_owned();
    } else if lower.starts_with("file://") {
        path = path["file://".len()..].to_owned();
        // file:///C:/x → C:/x
        if path.len() > 3 && path.starts_with('/') && path.as_bytes()[2] == b':' {
            path.remove(0);
        }
    } else if lower.starts_with("file:") {
        path = path["file:".len()..].to_owned();
    }
    if let Some(i) = path.find(['#', '?']) {
        path.truncate(i);
    }
    let mut path = percent_decode(&path).replace('\\', "/");
    // `x.md:12` / `x.md:12:3` (editor-style line refs).
    for _ in 0..2 {
        if let Some((head, tail)) = path.rsplit_once(':')
            && !tail.is_empty()
            && tail.chars().all(|c| c.is_ascii_digit())
            && head.len() > 2
        {
            path = head.to_owned();
        }
    }
    let path = path.trim().to_owned();
    if path.is_empty() {
        return None;
    }
    let absolute = path.starts_with('/') || is_drive_path(&path);
    let relative = if absolute {
        roots.iter().find_map(|root| strip_root(&path, root))?
    } else {
        path.trim_start_matches("./").to_owned()
    };
    let parts: Vec<&str> = relative
        .split('/')
        .filter(|p| !p.is_empty() && *p != ".")
        .collect();
    if parts.is_empty() || parts.contains(&"..") {
        return None;
    }
    Some(parts.join("/"))
}

fn is_drive_path(p: &str) -> bool {
    let b = p.as_bytes();
    b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'/'
}

fn strip_root(path: &str, root: &str) -> Option<String> {
    let root = root.replace('\\', "/");
    let root = root.trim_end_matches('/');
    if root.is_empty() {
        return None;
    }
    // Windows paths compare case-insensitively.
    let (p, r) = if is_drive_path(path) {
        (path.to_ascii_lowercase(), root.to_ascii_lowercase())
    } else {
        (path.to_owned(), root.to_owned())
    };
    let rest = p.strip_prefix(&r)?;
    if !rest.starts_with('/') {
        return None;
    }
    Some(path[r.len() + 1..].to_owned())
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT: &[&str] = &["/home/me/proj"];
    const WIN: &[&str] = &["D:\\MyWords\\VibeCoding\\DuckCommander"];

    #[test]
    fn mention_links_and_relative_paths() {
        assert_eq!(
            workspace_link_path("zeron-file:docs/report.md", ROOT).as_deref(),
            Some("docs/report.md")
        );
        assert_eq!(
            workspace_link_path("docs/report.md", ROOT).as_deref(),
            Some("docs/report.md")
        );
        assert_eq!(
            workspace_link_path("./notes/plan%20v2.md#top", ROOT).as_deref(),
            Some("notes/plan v2.md")
        );
    }

    #[test]
    fn absolute_paths_inside_the_workspace() {
        assert_eq!(
            workspace_link_path("/home/me/proj/docs/report.md:12", ROOT).as_deref(),
            Some("docs/report.md")
        );
        assert_eq!(
            workspace_link_path("file:///home/me/proj/README.md", ROOT).as_deref(),
            Some("README.md")
        );
        assert_eq!(
            workspace_link_path("D:\\MyWords\\VibeCoding\\DuckCommander\\docs\\设计.md", WIN)
                .as_deref(),
            Some("docs/设计.md")
        );
        assert_eq!(
            workspace_link_path("file:///d:/mywords/VibeCoding/DuckCommander/a.md:3:1", WIN)
                .as_deref(),
            Some("a.md")
        );
    }

    #[test]
    fn web_links_and_escapes_are_refused() {
        assert_eq!(workspace_link_path("https://example.com/a.md", ROOT), None);
        assert_eq!(workspace_link_path("mailto:me@example.com", ROOT), None);
        assert_eq!(workspace_link_path("/etc/passwd", ROOT), None);
        assert_eq!(workspace_link_path("/home/me/project2/a.md", ROOT), None);
        assert_eq!(workspace_link_path("../secrets.md", ROOT), None);
        assert_eq!(workspace_link_path("zeron-file:", ROOT), None);
        assert!(is_file_link("C:\\x.md"));
        assert_eq!(
            workspace_link_path("README.md:7", ROOT).as_deref(),
            Some("README.md")
        );
        assert!(!is_file_link("https://x"));
    }
}
