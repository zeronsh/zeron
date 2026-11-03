//! Working folders of chats without a project.
//!
//! Each projectless chat runs in its own folder under [`SCRATCH_ROOT`] on its
//! host (t3code's Scratch project does the same under `~/.t3/scratch`). The
//! cwd is stored home-relative because the creating device can't know the
//! host's home; the host expands it, and creates the folder, when it runs.
//! Chats created before scratch folders store `~` and keep running in the
//! host's home, so their agent sessions (keyed by cwd) still resume.

use chrono::{DateTime, Utc};

/// Home-relative parent of every projectless chat's folder.
pub const SCRATCH_ROOT: &str = "~/.zeron/scratch";

/// The scratch cwd for a new projectless chat: `<date>-<id>`, the id cut to
/// its last 8 letters and digits (the random end of a UUID or ULID).
pub fn scratch_cwd(chat_id: &str, created_at: DateTime<Utc>) -> String {
    let id: Vec<char> = chat_id
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect();
    let id: String = id[id.len().saturating_sub(8)..].iter().collect();
    let date = created_at.format("%Y-%m-%d");
    if id.is_empty() {
        format!("{SCRATCH_ROOT}/{date}")
    } else {
        format!("{SCRATCH_ROOT}/{date}-{id}")
    }
}

/// The legacy projectless cwd: the host's home.
pub fn is_home_cwd(cwd: &str) -> bool {
    matches!(cwd.trim(), "" | "~" | "~/")
}

/// A projectless chat's own folder under [`SCRATCH_ROOT`].
pub fn is_scratch_cwd(cwd: &str) -> bool {
    cwd.trim()
        .strip_prefix(SCRATCH_ROOT)
        .and_then(|rest| rest.strip_prefix('/'))
        .is_some_and(|name| !name.is_empty() && !name.contains(['/', '\\']))
}

/// Whether a chat with this cwd (and no project) is projectless: no cwd, the
/// host's home, or a scratch folder.
pub fn is_projectless_cwd(cwd: Option<&str>) -> bool {
    cwd.is_none_or(|cwd| is_home_cwd(cwd) || is_scratch_cwd(cwd))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(date: &str) -> DateTime<Utc> {
        format!("{date}T12:00:00Z").parse().unwrap()
    }

    #[test]
    fn scratch_cwd_names_the_folder_by_date_and_id_tail() {
        assert_eq!(
            scratch_cwd("9f1c2a3b-4d5e-4f60-8a7b-1c2d3e4f5a6b", at("2026-10-11")),
            "~/.zeron/scratch/2026-10-11-3e4f5a6b"
        );
        assert_eq!(
            scratch_cwd("Ab-1", at("2026-01-02")),
            "~/.zeron/scratch/2026-01-02-ab1"
        );
        assert_eq!(
            scratch_cwd("--", at("2026-01-02")),
            "~/.zeron/scratch/2026-01-02"
        );
    }

    #[test]
    fn projectless_cwds() {
        for cwd in ["~", "~/", "", "~/.zeron/scratch/2026-10-11-3e4f5a6b"] {
            assert!(is_projectless_cwd(Some(cwd)), "{cwd}");
        }
        assert!(is_projectless_cwd(None));
        for cwd in [
            "/home/w/dev/zeron",
            "~/dev/zeron",
            "~/.zeron/scratch",
            "~/.zeron/scratch/",
            "~/.zeron/scratch/a/b",
            "~/.zeron/worktrees/x",
        ] {
            assert!(!is_projectless_cwd(Some(cwd)), "{cwd}");
        }
    }
}
