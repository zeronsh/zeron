//! Where omp keeps sessions, and finding one by id to resume it.
//!
//! omp writes each session as `<timestamp>_<sessionId>.jsonl`, under a
//! per-project directory of its session store, and its first lines are a
//! `title` record and the `session` header (`{"type":"session","id":…}`).
//! A session writes its file only once it has a message, so a run that never
//! got that far leaves nothing to resume. Zeron never writes omp's files.
use crate::HarnessError;
use serde_json::Value;
use std::{
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
};

/// omp's agent directory for the default profile: `PI_CODING_AGENT_DIR`, or
/// `~/.omp/agent`; a named profile (`OMP_PROFILE`, else `PI_PROFILE`) keeps
/// its own under `~/.omp/profiles/<name>/agent`.
pub(super) fn agent_dir() -> PathBuf {
    agent_dir_in(&|name| std::env::var(name).ok())
}

/// `agent_dir`, reading its variables through `env` (a run's own, which
/// override the engine's: omp is launched with them).
pub(super) fn agent_dir_in(env: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    let home = crate::executable::home_or_current_dir();
    let set = |name: &str| {
        env(name)
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
    };
    let profile = set("OMP_PROFILE")
        .or_else(|| set("PI_PROFILE"))
        .filter(|p| p != "default");
    match profile {
        Some(name) => home.join(".omp/profiles").join(name).join("agent"),
        None => set("PI_CODING_AGENT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".omp/agent")),
    }
}

/// The directories a session may be in: an explicit `--session-dir`, the
/// launch override omp reads (`PI_CODING_AGENT_SESSION_DIR`), then the
/// profile's store; each variable as the run sets it, else the engine's.
pub(super) fn roots(
    explicit: Option<&Path>,
    run_env: &std::collections::BTreeMap<String, String>,
) -> Vec<PathBuf> {
    let env = |name: &str| {
        run_env
            .get(name)
            .cloned()
            .or_else(|| std::env::var(name).ok())
    };
    let mut roots: Vec<PathBuf> = explicit.map(Path::to_path_buf).into_iter().collect();
    if let Some(dir) = env("PI_CODING_AGENT_SESSION_DIR").filter(|d| !d.is_empty()) {
        roots.push(dir.into());
    }
    roots.push(agent_dir_in(&env).join("sessions"));
    roots
}

/// The file of session `id`, searched in `roots` (and one directory below
/// each: omp's per-project folders). No symlinks are followed.
pub(super) fn find(id: &str, roots: &[PathBuf]) -> Result<PathBuf, HarnessError> {
    if id.is_empty() || id.contains(['/', '\\']) || id.contains("..") {
        return Err(HarnessError::Protocol(format!(
            "not an omp session id: {id}"
        )));
    }
    let suffix = format!("_{id}.jsonl");
    let mut seen = 0usize;
    for root in roots {
        let mut dirs = vec![(root.clone(), 0)];
        while let Some((dir, depth)) = dirs.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                seen += 1;
                if seen > 50_000 {
                    return Err(HarnessError::Protocol(
                        "omp session search exceeded 50000 entries".into(),
                    ));
                }
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                let path = entry.path();
                if kind.is_dir() && depth < 1 {
                    dirs.push((path, depth + 1));
                } else if kind.is_file()
                    && entry.file_name().to_string_lossy().ends_with(&suffix)
                    && header_id(&path).as_deref() == Some(id)
                {
                    return Ok(path);
                }
            }
        }
    }
    Err(HarnessError::Protocol(format!(
        "omp session {id} was not found"
    )))
}

/// The id in a session file's `session` header (within its first lines).
fn header_id(path: &Path) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    let mut reader = BufReader::new(file.take(256 * 1024));
    let mut line = String::new();
    for _ in 0..8 {
        line.clear();
        if reader.read_line(&mut line).ok()? == 0 {
            return None;
        }
        if let Ok(record) = serde_json::from_str::<Value>(&line)
            && record["type"] == "session"
        {
            return record["id"].as_str().map(str::to_owned);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A run's own variables say where its sessions are, over the engine's:
    /// omp is launched with them, so a resume looks where it writes.
    #[test]
    fn a_runs_own_variables_say_where_its_sessions_are() {
        let mut env = std::collections::BTreeMap::new();
        env.insert(
            "PI_CODING_AGENT_SESSION_DIR".to_owned(),
            "/tmp/omp-run-sessions".to_owned(),
        );
        assert!(roots(None, &env).contains(&PathBuf::from("/tmp/omp-run-sessions")));
        env.insert("OMP_PROFILE".to_owned(), "work".to_owned());
        assert!(
            roots(None, &env)
                .last()
                .unwrap()
                .ends_with(".omp/profiles/work/agent/sessions")
        );
        let mut dir = std::collections::BTreeMap::new();
        dir.insert(
            "PI_CODING_AGENT_DIR".to_owned(),
            "/tmp/omp-agent".to_owned(),
        );
        assert_eq!(
            roots(None, &dir).last().unwrap(),
            &PathBuf::from("/tmp/omp-agent/sessions")
        );
    }

    #[test]
    fn sessions_are_found_by_id_in_project_folders_and_checked_against_their_header() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("--private-tmp-work--");
        std::fs::create_dir_all(&project).unwrap();
        let id = "01a1d49f-be6e-72f8-b068-12a6d1bc8460";
        let file = project.join(format!("2026-11-13T11-16-41-198Z_{id}.jsonl"));
        // The head of a real omp 18.20.0 session file.
        std::fs::write(
            &file,
            format!(
                "{{\"type\":\"title\",\"v\":1,\"title\":\"\",\"pad\":\"   \"}}\n{{\"type\":\"session\",\"version\":3,\"id\":\"{id}\",\"cwd\":\"/tmp/work\"}}\n"
            ),
        )
        .unwrap();
        let roots = vec![dir.path().join("missing"), dir.path().to_path_buf()];
        assert_eq!(find(id, &roots).unwrap(), file);
        // A file named for the id whose header names another is not it.
        let other = "0badbad0-0000-0000-0000-000000000000";
        std::fs::write(
            project.join(format!("x_{other}.jsonl")),
            format!("{{\"type\":\"session\",\"id\":\"{id}\"}}\n"),
        )
        .unwrap();
        assert!(find(other, &roots).is_err());
        assert!(find("../etc", &roots).is_err());
        assert!(find("", &roots).is_err());
        // Directly in an explicit --session-dir.
        let flat = tempfile::tempdir().unwrap();
        let file = flat.path().join(format!("t_{other}.jsonl"));
        std::fs::write(
            &file,
            format!("{{\"type\":\"session\",\"id\":\"{other}\"}}\n"),
        )
        .unwrap();
        assert_eq!(find(other, &[flat.path().to_path_buf()]).unwrap(), file);
    }
}
