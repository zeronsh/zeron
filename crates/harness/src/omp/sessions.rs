//! Host-local UUID -> native file lookup. Never rewrite OMP's conversation files.
use crate::{
    HarnessError,
    pi::sessions::{find_session, json_file, write_record},
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub(super) struct Store {
    root: PathBuf,
    agent: PathBuf,
}
/// A named profile makes OMP ignore `PI_CODING_AGENT_DIR`, so a forced agent
/// directory also clears these.
pub(super) const PROFILE_VARS: &[&str] = &["OMP_PROFILE", "PI_PROFILE"];
/// OMP's agent directory, resolved as `omp` resolves it (`utils/src/dirs.ts`):
/// an explicit override; else a named profile (`OMP_PROFILE`, or `PI_PROFILE`
/// when `OMP_PROFILE` is unset) at `~/<PI_CONFIG_DIR or .omp>/profiles/<name>/agent`;
/// else `PI_CODING_AGENT_DIR` (Pi's variable name); else `~/<PI_CONFIG_DIR or .omp>/agent`.
pub(super) fn agent_dir(explicit: Option<PathBuf>) -> PathBuf {
    agent_dir_in(
        explicit,
        &crate::executable::home_or_current_dir(),
        &|key| std::env::var_os(key),
    )
}
pub(crate) fn agent_dir_in(
    explicit: Option<PathBuf>,
    home: &Path,
    env: &dyn Fn(&str) -> Option<std::ffi::OsString>,
) -> PathBuf {
    if let Some(dir) = explicit {
        return dir;
    }
    let set = |key| env(key).filter(|v| !v.is_empty());
    let root = home.join(set("PI_CONFIG_DIR").unwrap_or_else(|| ".omp".into()));
    let profile_dir = |name: String| root.join("profiles").join(name).join("agent");
    // Empty, "default" and invalid names select the default profile.
    let named = |value: Option<std::ffi::OsString>| {
        let name = value?.to_string_lossy().trim().to_owned();
        let valid = name != "default"
            && !name.ends_with('.')
            && name.len() <= 64
            && name.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
            && name.chars().all(|c| {
                c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-')
            });
        valid.then_some(name)
    };
    // A defined-but-empty OMP_PROFILE selects the default profile, not PI_PROFILE.
    let profile = match env("OMP_PROFILE") {
        Some(value) => named(Some(value)),
        None => named(env("PI_PROFILE")),
    };
    if let Some(name) = profile {
        return profile_dir(name);
    }
    if let Some(dir) = set("PI_CODING_AGENT_DIR").map(PathBuf::from) {
        // A parent's profile switch exports that profile's agent dir; it is
        // not a default-profile override.
        if named(env("PI_PROFILE")).map(profile_dir).as_ref() != Some(&dir) {
            return dir;
        }
    }
    root.join("agent")
}
impl Store {
    pub fn new(root: Option<PathBuf>, agent: Option<PathBuf>) -> Self {
        let agent = agent_dir(agent);
        Self {
            root: root.unwrap_or_else(|| agent.join("zeron-sessions")),
            agent,
        }
    }
    fn key(&self, id: &str) -> PathBuf {
        self.root
            .join(format!("{:x}.json", Sha256::digest(id.as_bytes())))
    }
    pub fn remember(&self, id: &str, file: &Path) -> Result<(), HarnessError> {
        if !file.is_absolute() {
            return Err(HarnessError::Protocol(
                "Oh My Pi returned a non-absolute session file".into(),
            ));
        }
        let mut record = json!({"sessionId":id,"sessionFile":file});
        let previous = json_file(&self.key(id));
        if previous["sessionFile"] == record["sessionFile"] && previous["submitted"] == true {
            record["submitted"] = json!(true);
        }
        write_record(&self.root, &self.key(id), &record)
    }
    /// A turn started in this session, so OMP has written (or will write) its file.
    pub fn mark_submitted(&self, id: &str) -> Result<(), HarnessError> {
        let mut record = json_file(&self.key(id));
        if record.is_object() && record["submitted"] != true {
            record["submitted"] = json!(true);
            write_record(&self.root, &self.key(id), &record)?;
        }
        Ok(())
    }
    /// Zeron knows this session and no turn ever started in it.
    pub fn never_submitted(&self, id: &str) -> bool {
        let record = json_file(&self.key(id));
        record.is_object() && record["submitted"] != true
    }
    pub fn resume_args(&self, id: &str, _cwd: &Path) -> Result<Vec<String>, HarnessError> {
        let file = self.resolve(id)?;
        Ok(vec!["--resume".into(), file.display().to_string()])
    }
    pub fn resolve(&self, id: &str) -> Result<PathBuf, HarnessError> {
        let candidates = json_file(&self.key(id))["sessionFile"]
            .as_str()
            .map(PathBuf::from)
            .into_iter()
            .collect();
        find_session(candidates, vec![self.agent.join("sessions")], id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TITLE: &str = "{\"type\":\"title\",\"v\":1,\"title\":\"x\",\"pad\":\"   \"}\n";

    #[test]
    fn resolves_title_prefixed_files_by_index_and_by_search() {
        let dir = tempfile::tempdir().unwrap();
        let agent = dir.path().join("agent");
        let project = agent.join("sessions/-github-app");
        std::fs::create_dir_all(&project).unwrap();
        let file = project.join("2026-10-16T00-00-00-000Z_abc.jsonl");
        std::fs::write(
            &file,
            format!("{TITLE}{{\"type\":\"session\",\"version\":3,\"id\":\"abc\"}}\n"),
        )
        .unwrap();
        let store = Store::new(Some(dir.path().join("index")), Some(agent));
        let expected = file.canonicalize().unwrap();
        assert_eq!(store.resolve("abc").unwrap(), expected);
        store.remember("abc", &file).unwrap();
        assert_eq!(
            store.resume_args("abc", dir.path()).unwrap(),
            vec!["--resume".to_string(), expected.display().to_string()]
        );
        assert!(store.resolve("other").is_err());
    }

    #[test]
    fn agent_dir_follows_omp_override_profile_and_config_dir_order() {
        let home = PathBuf::from("/home/u");
        let resolve = |vars: &[(&str, &str)]| {
            let vars: Vec<(String, String)> = vars
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            agent_dir_in(None, &home, &move |key| {
                vars.iter().find(|(k, _)| k == key).map(|(_, v)| v.into())
            })
        };
        assert_eq!(resolve(&[]), home.join(".omp/agent"));
        assert_eq!(
            resolve(&[("OMP_PROFILE", "work")]),
            home.join(".omp/profiles/work/agent")
        );
        assert_eq!(
            resolve(&[("OMP_PROFILE", ""), ("PI_PROFILE", "work")]),
            home.join(".omp/agent"),
            "an empty OMP_PROFILE selects the default profile"
        );
        assert_eq!(
            resolve(&[("PI_PROFILE", "work"), ("PI_CONFIG_DIR", ".omp-dev")]),
            home.join(".omp-dev/profiles/work/agent")
        );
        assert_eq!(
            resolve(&[("PI_CODING_AGENT_DIR", "/x/agent")]),
            PathBuf::from("/x/agent")
        );
        assert_eq!(
            resolve(&[("PI_CODING_AGENT_DIR", "/x/agent"), ("OMP_PROFILE", "work")]),
            home.join(".omp/profiles/work/agent"),
            "a named profile ignores PI_CODING_AGENT_DIR"
        );
        assert_eq!(
            resolve(&[
                ("OMP_PROFILE", ""),
                ("PI_PROFILE", "work"),
                ("PI_CODING_AGENT_DIR", "/home/u/.omp/profiles/work/agent")
            ]),
            home.join(".omp/agent"),
            "a profile-derived PI_CODING_AGENT_DIR is not an override"
        );
        assert_eq!(
            resolve(&[
                ("OMP_PROFILE", "default"),
                ("PI_CODING_AGENT_DIR", "/x/agent")
            ]),
            PathBuf::from("/x/agent")
        );
        assert_eq!(
            resolve(&[("OMP_PROFILE", "Bad Name")]),
            home.join(".omp/agent")
        );
        assert_eq!(
            agent_dir_in(Some("/explicit".into()), &home, &|_| Some("/env".into())),
            PathBuf::from("/explicit")
        );
    }

    #[test]
    fn only_known_sessions_without_a_turn_count_as_never_submitted() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(Some(dir.path().join("index")), Some(dir.path().into()));
        let file = dir.path().join("lazy.jsonl");
        assert!(!store.never_submitted("lazy"), "unknown ids are not proof");
        store.remember("lazy", &file).unwrap();
        assert!(store.never_submitted("lazy"));
        store.mark_submitted("lazy").unwrap();
        assert!(!store.never_submitted("lazy"));
        // Refreshing the same file keeps the proof that a turn started.
        store.remember("lazy", &file).unwrap();
        assert!(!store.never_submitted("lazy"));
    }
}
