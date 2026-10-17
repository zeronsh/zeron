//! Host-local UUID -> native file lookup. Never rewrite Pi's conversation files.
use crate::HarnessError;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
};

#[derive(Clone)]
pub(super) struct Store {
    root: PathBuf,
    agent: PathBuf,
    legacy: PathBuf,
}
/// Pi's settings directory: an explicit override, else `PI_CODING_AGENT_DIR`,
/// else `~/.pi/agent`.
pub(super) fn agent_dir(explicit: Option<PathBuf>) -> PathBuf {
    explicit.unwrap_or_else(|| {
        crate::model_context::root(
            "PI_CODING_AGENT_DIR",
            crate::executable::home_or_current_dir().join(".pi/agent"),
        )
    })
}
impl Store {
    pub fn new(root: Option<PathBuf>, agent: Option<PathBuf>) -> Self {
        let home = crate::executable::home_or_current_dir();
        let agent = agent_dir(agent);
        Self {
            root: root.unwrap_or_else(|| agent.join("zeron-sessions")),
            agent,
            legacy: home.join(".pi/pi-acp/session-map.json"),
        }
    }
    /// Whether a steering mode is already set in the settings Pi loads for
    /// `cwd` (global, then project).
    pub fn steering_mode_configured(&self, cwd: &Path) -> bool {
        [
            self.agent.join("settings.json"),
            cwd.join(".pi/settings.json"),
        ]
        .iter()
        .any(|settings| json_file(settings).get("steeringMode").is_some())
    }
    fn key(&self, id: &str) -> PathBuf {
        self.root
            .join(format!("{:x}.json", Sha256::digest(id.as_bytes())))
    }
    pub fn remember(&self, id: &str, file: &Path) -> Result<(), HarnessError> {
        if !file.is_absolute() {
            return Err(HarnessError::Protocol(
                "Pi returned a non-absolute session file".into(),
            ));
        }
        self.write(id, &json!({"sessionId":id,"sessionFile":file}))
    }
    /// The native child must remain addressable after publication and restart,
    /// including sessions stored outside Pi's standard search directories.
    pub fn remember_fork(&self, id: &str, file: &Path) -> Result<(), HarnessError> {
        self.remember(id, file)?;
        // Windows FlushFileBuffers requires a writable handle.
        std::fs::OpenOptions::new()
            .write(true)
            .open(self.key(id))?
            .sync_all()?;
        #[cfg(unix)]
        {
            std::fs::File::open(&self.root)?.sync_all()?;
            if let Some(parent) = self.root.parent().filter(|p| !p.as_os_str().is_empty()) {
                std::fs::File::open(parent)?.sync_all()?;
            }
        }
        Ok(())
    }
    fn write(&self, id: &str, value: &Value) -> Result<(), HarnessError> {
        std::fs::create_dir_all(&self.root)?;
        let path = self.key(id);
        if json_file(&path) == *value {
            return Ok(());
        }
        let temp = self.root.join(format!("{}.tmp", uuid::Uuid::new_v4()));
        std::fs::write(&temp, serde_json::to_vec(value).unwrap())?;
        let result = std::fs::rename(&temp, &path);
        if result.is_err() {
            let _ = std::fs::remove_file(&temp);
        }
        result.map_err(Into::into)
    }
    pub fn mark_submitted(&self, id: &str) -> Result<(), HarnessError> {
        let mut record = json_file(&self.key(id));
        if let Some(map) = record.as_object_mut() {
            if map.remove("emptyState").is_some() {
                self.write(id, &record)?;
            }
        }
        Ok(())
    }
    pub fn remember_empty(&self, state: &Value, cwd: &Path) -> Result<(), HarnessError> {
        let id = state["sessionId"]
            .as_str()
            .ok_or_else(|| HarnessError::Protocol("Pi omitted sessionId".into()))?;
        let mut record = json_file(&self.key(id));
        if record.is_object() {
            record["emptyState"] = state.clone();
            record["cwd"] = json!(cwd.canonicalize()?);
            self.write(id, &record)?;
        }
        Ok(())
    }
    pub fn resume_args(&self, id: &str, cwd: &Path) -> Result<Vec<String>, HarnessError> {
        match self.resolve(id, cwd) {
            Ok(file) => Ok(vec!["--session".into(), file.display().to_string()]),
            Err(error) => {
                let record = json_file(&self.key(id));
                let state = &record["emptyState"];
                let file = record["sessionFile"].as_str().map(Path::new);
                // Only our own post-ACK empty-session proof permits recreation.
                // Deleted history, custom extension entries, and uncertain crashed
                // prompts must never become a fresh conversation silently.
                if state.is_null()
                    || record["cwd"] != json!(cwd.canonicalize()?)
                    || !file.is_some_and(|p| {
                        p.is_absolute()
                            && p.metadata()
                                .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
                    })
                {
                    return Err(error);
                }
                let mut args = vec![
                    "--session-id".into(),
                    id.into(),
                    "--session-dir".into(),
                    file.unwrap().parent().unwrap().display().to_string(),
                ];
                for (flag, value) in [
                    ("--provider", &state["model"]["provider"]),
                    ("--model", &state["model"]["id"]),
                    ("--thinking", &state["thinkingLevel"]),
                ] {
                    if let Some(value) = value.as_str() {
                        args.extend([flag.into(), value.into()]);
                    }
                }
                Ok(args)
            }
        }
    }
    pub fn resolve(&self, id: &str, cwd: &Path) -> Result<PathBuf, HarnessError> {
        let mut candidates = vec![];
        if let Some(file) = json_file(&self.key(id))["sessionFile"].as_str() {
            candidates.push(PathBuf::from(file));
        }
        if let Some(file) = json_file(&self.legacy)["sessions"][id]["sessionFile"].as_str() {
            candidates.push(PathBuf::from(file));
        }
        let mut roots = vec![self.agent.join("sessions")];
        for settings in [
            self.agent.join("settings.json"),
            cwd.join(".pi/settings.json"),
        ] {
            if let Some(root) = json_file(&settings)["sessionDir"].as_str() {
                let path = if let Some(rest) = root.strip_prefix("~/") {
                    crate::executable::home_or_current_dir().join(rest)
                } else {
                    cwd.join(root)
                };
                roots.push(path);
            }
        }
        for file in candidates {
            if matches_id(&file, id) {
                return Ok(file.canonicalize()?);
            }
        }
        for root in roots {
            // Pi stores session files under per-project directories. No symlink traversal.
            let mut dirs = vec![(root, 0)];
            let mut seen = 0;
            while let Some((dir, depth)) = dirs.pop() {
                let Ok(entries) = std::fs::read_dir(dir) else {
                    continue;
                };
                for entry in entries.flatten() {
                    seen += 1;
                    if seen > 20000 {
                        return Err(HarnessError::Protocol(
                            "Pi session search exceeded 20000 entries".into(),
                        ));
                    }
                    let Ok(kind) = entry.file_type() else {
                        continue;
                    };
                    let path = entry.path();
                    if kind.is_dir() && depth < 2 {
                        dirs.push((path, depth + 1));
                    } else if kind.is_file()
                        && path.extension().is_some_and(|e| e == "jsonl")
                        && matches_id(&path, id)
                    {
                        return Ok(path.canonicalize()?);
                    }
                }
            }
        }
        Err(HarnessError::Protocol(format!(
            "native session file for {id} was not found"
        )))
    }
}
fn json_file(path: &Path) -> Value {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or(Value::Null)
}
fn matches_id(path: &Path, id: &str) -> bool {
    use std::io::Read;
    let Ok(file) = std::fs::File::open(path) else {
        return false;
    };
    let mut line = String::new();
    if BufReader::new(file.take(65536))
        .read_line(&mut line)
        .is_err()
    {
        return false;
    }
    serde_json::from_str::<Value>(&line).is_ok_and(|v| v["type"] == "session" && v["id"] == id)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_fork_mapping_is_durable_and_reopens_custom_session_directory() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("child with spaces.jsonl");
        std::fs::write(&file, "{\"type\":\"session\",\"id\":\"native-child\"}\n").unwrap();
        let index = dir.path().join("index");
        let agent = dir.path().join("isolated-agent");
        Store::new(Some(index.clone()), Some(agent.clone()))
            .remember_fork("native-child", &file)
            .unwrap();
        let reopened = Store::new(Some(index), Some(agent));
        assert_eq!(
            reopened.resolve("native-child", dir.path()).unwrap(),
            file.canonicalize().unwrap()
        );
        std::fs::remove_file(&file).unwrap();
        assert!(reopened.resolve("native-child", dir.path()).is_err());
    }
    #[test]
    fn only_proven_empty_local_sessions_can_be_recreated_and_submission_revokes_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::new(Some(dir.path().join("index")), None);
        store.agent = dir.path().join("agent");
        store.legacy = dir.path().join("legacy.json");
        let file = dir.path().join("session.jsonl");
        let state = json!({"sessionId":"empty","sessionFile":file,"model":{"provider":"mock","id":"test"},"thinkingLevel":"off"});
        store.remember("empty", &file).unwrap();
        assert!(store.resume_args("empty", dir.path()).is_err());
        store.remember_empty(&state, dir.path()).unwrap();
        assert_eq!(
            store.resume_args("empty", dir.path()).unwrap(),
            vec![
                "--session-id",
                "empty",
                "--session-dir",
                dir.path().to_str().unwrap(),
                "--provider",
                "mock",
                "--model",
                "test",
                "--thinking",
                "off"
            ]
        );
        let other = tempfile::tempdir().unwrap();
        assert!(store.resume_args("empty", other.path()).is_err());
        store.mark_submitted("empty").unwrap();
        assert!(store.resume_args("empty", dir.path()).is_err());
        std::fs::write(&file, "{\"type\":\"session\",\"id\":\"empty\"}\n").unwrap();
        assert_eq!(
            store.resume_args("empty", dir.path()).unwrap()[0],
            "--session"
        );
        std::fs::remove_file(&file).unwrap();
        assert!(
            store.resume_args("empty", dir.path()).is_err(),
            "missing persisted history must not be recreated"
        );
    }

    #[test]
    fn an_explicit_steering_mode_in_either_settings_file_is_respected() {
        let dir = tempfile::tempdir().unwrap();
        let agent = dir.path().join("agent");
        let cwd = dir.path().join("project");
        std::fs::create_dir_all(&agent).unwrap();
        std::fs::create_dir_all(cwd.join(".pi")).unwrap();
        let store = Store::new(Some(dir.path().join("index")), Some(agent.clone()));
        assert!(!store.steering_mode_configured(&cwd));
        std::fs::write(
            agent.join("settings.json"),
            r#"{"retry":{"enabled":false}}"#,
        )
        .unwrap();
        assert!(!store.steering_mode_configured(&cwd));
        std::fs::write(
            cwd.join(".pi/settings.json"),
            r#"{"steeringMode":"one-at-a-time"}"#,
        )
        .unwrap();
        assert!(store.steering_mode_configured(&cwd));
        std::fs::remove_file(cwd.join(".pi/settings.json")).unwrap();
        std::fs::write(agent.join("settings.json"), r#"{"steeringMode":"all"}"#).unwrap();
        assert!(store.steering_mode_configured(&cwd));
    }

    #[test]
    fn resolves_legacy_and_native_files_and_rejects_wrong_identity() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session with spaces.jsonl");
        std::fs::write(&path, "{\"type\":\"session\",\"id\":\"old-id\"}\n").unwrap();
        // Windows expands short names and adds a verbatim prefix on canonicalization.
        let expected = path.canonicalize().unwrap();
        let mut store = Store::new(Some(dir.path().join("index")), None);
        store.agent = dir.path().join("agent");
        store.legacy = dir.path().join("legacy.json");
        std::fs::write(
            &store.legacy,
            json!({"version":1,"sessions":{"old-id":{"sessionFile":path}}}).to_string(),
        )
        .unwrap();
        assert_eq!(store.resolve("old-id", dir.path()).unwrap(), expected);
        store.remember("old-id", &path).unwrap();
        std::fs::remove_file(&store.legacy).unwrap();
        assert_eq!(store.resolve("old-id", dir.path()).unwrap(), expected);
        store.remember("wrong-id", &path).unwrap();
        assert!(store.resolve("wrong-id", dir.path()).is_err());
    }
}
