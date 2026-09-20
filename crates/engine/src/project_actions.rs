//! Host-local project Actions and explicit `zeron.json` imports.
//!
//! Commands are intentionally stored outside the synced workspace registry. The
//! owning engine is the only authority that can persist or execute them.

use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use uuid::Uuid;
use zeron_proto::{
    ProjectAction, ProjectActionDraft, ProjectActionIcon, ProjectActionRun, ProjectActionsSnapshot,
};

use crate::{EngineError, Terminals};

pub const MAX_PROJECT_ACTIONS: usize = 50;
pub const MAX_PROJECT_ACTION_NAME_CHARS: usize = 80;
pub const MAX_PROJECT_ACTION_COMMAND_BYTES: usize = 16 * 1024;
pub const MAX_PROJECT_ACTION_ID_BYTES: usize = 96;

const STORE_FILE: &str = "project-actions.json";
const STORE_VERSION: u32 = 1;
const PROJECT_FILE: &str = "zeron.json";
const MAX_PROJECT_FILE_BYTES: u64 = 256 * 1024;
const SETUP_HANDOFF_TTL: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, Clone)]
pub struct ProjectActionSetupHandoff {
    pub setup_action: Option<ProjectActionRun>,
    pub setup_error: Option<String>,
}

struct StoredSetupHandoff {
    chat_id: String,
    completed_at: Instant,
    outcome: ProjectActionSetupHandoff,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectActionsFile {
    version: u32,
    #[serde(default)]
    projects: BTreeMap<String, StoredProjectActions>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredProjectActions {
    project_root: String,
    #[serde(default)]
    actions: Vec<ProjectAction>,
}

struct ProjectActionsStoreInner {
    path: PathBuf,
    state: Mutex<ProjectActionsFile>,
    setup_handoffs: Mutex<HashMap<String, StoredSetupHandoff>>,
}

/// Account-scoped storage for executable project Actions.
#[derive(Clone)]
pub struct ProjectActionsStore {
    inner: Arc<ProjectActionsStoreInner>,
}

impl ProjectActionsStore {
    pub fn open(profile_store_root: &Path) -> Result<Self, EngineError> {
        std::fs::create_dir_all(profile_store_root)?;
        let path = profile_store_root.join(STORE_FILE);
        let state = match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<ProjectActionsFile>(&bytes) {
                Ok(file) if file.version == STORE_VERSION => file,
                Ok(file) => {
                    tracing::warn!(
                        path = %path.display(),
                        version = file.version,
                        "unsupported project Actions store version; starting empty"
                    );
                    empty_file()
                }
                Err(err) => {
                    tracing::warn!(
                        path = %path.display(),
                        error = %err,
                        "invalid project Actions store; starting empty"
                    );
                    empty_file()
                }
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => empty_file(),
            Err(err) => return Err(err.into()),
        };
        Ok(Self {
            inner: Arc::new(ProjectActionsStoreInner {
                path,
                state: Mutex::new(state),
                setup_handoffs: Mutex::new(HashMap::new()),
            }),
        })
    }

    pub fn actions(
        &self,
        space_id: &str,
        project_root: &Path,
    ) -> Result<Vec<ProjectAction>, EngineError> {
        let state = lock(&self.inner.state);
        let Some(project) = state.projects.get(space_id) else {
            return Ok(Vec::new());
        };
        ensure_project_root(project, project_root)?;
        Ok(project.actions.clone())
    }

    pub fn snapshot(
        &self,
        space_id: &str,
        project_root: &Path,
    ) -> Result<ProjectActionsSnapshot, EngineError> {
        let actions = self.actions(space_id, project_root)?;
        Ok(snapshot_from_actions(space_id, project_root, actions))
    }

    pub fn upsert(
        &self,
        space_id: &str,
        project_root: &Path,
        action_id: Option<&str>,
        draft: ProjectActionDraft,
    ) -> Result<ProjectActionsSnapshot, EngineError> {
        let draft = normalize_draft(draft)?;
        let root = root_string(project_root);
        let actions = {
            let mut state = lock(&self.inner.state);
            let mut next = state.clone();
            let project = next
                .projects
                .entry(space_id.to_string())
                .or_insert_with(|| StoredProjectActions {
                    project_root: root.clone(),
                    actions: Vec::new(),
                });
            ensure_project_root(project, project_root)?;

            let index = match action_id {
                Some(action_id) => project
                    .actions
                    .iter()
                    .position(|action| action.id == action_id)
                    .ok_or_else(|| EngineError::Other("Project action not found".into()))?,
                None => {
                    if project.actions.len() >= MAX_PROJECT_ACTIONS {
                        return Err(EngineError::Other(format!(
                            "A project can have at most {MAX_PROJECT_ACTIONS} actions"
                        )));
                    }
                    project.actions.len()
                }
            };

            if draft.run_on_worktree_create {
                for action in &mut project.actions {
                    action.run_on_worktree_create = false;
                }
            }

            let id = action_id
                .map(str::to_string)
                .unwrap_or_else(|| unique_action_id(&draft.name, &project.actions));
            let action = ProjectAction {
                id,
                name: draft.name,
                command: draft.command,
                icon: draft.icon,
                run_on_worktree_create: draft.run_on_worktree_create,
            };
            if index == project.actions.len() {
                project.actions.push(action);
            } else {
                project.actions[index] = action;
            }
            let actions = project.actions.clone();
            persist(&self.inner.path, &next)?;
            *state = next;
            actions
        };
        Ok(snapshot_from_actions(space_id, project_root, actions))
    }

    pub fn delete(
        &self,
        space_id: &str,
        project_root: &Path,
        action_id: &str,
    ) -> Result<ProjectActionsSnapshot, EngineError> {
        let actions = {
            let mut state = lock(&self.inner.state);
            let mut next = state.clone();
            let project = next
                .projects
                .get_mut(space_id)
                .ok_or_else(|| EngineError::Other("Project action not found".into()))?;
            ensure_project_root(project, project_root)?;
            let before = project.actions.len();
            project.actions.retain(|action| action.id != action_id);
            if project.actions.len() == before {
                return Err(EngineError::Other("Project action not found".into()));
            }
            let actions = project.actions.clone();
            persist(&self.inner.path, &next)?;
            *state = next;
            actions
        };
        Ok(snapshot_from_actions(space_id, project_root, actions))
    }

    pub fn action(
        &self,
        space_id: &str,
        project_root: &Path,
        action_id: &str,
    ) -> Result<Option<ProjectAction>, EngineError> {
        Ok(self
            .actions(space_id, project_root)?
            .into_iter()
            .find(|action| action.id == action_id))
    }

    pub fn setup_action(
        &self,
        space_id: &str,
        project_root: &Path,
    ) -> Result<Option<ProjectAction>, EngineError> {
        Ok(self
            .actions(space_id, project_root)?
            .into_iter()
            .find(|action| action.run_on_worktree_create))
    }

    /// Publish the non-durable UI handoff for setup launched while the durable
    /// Run command is drained. The command remains the source of truth; this
    /// cache only lets the sender attach the resulting terminal.
    pub fn complete_setup_handoff(
        &self,
        command_id: &str,
        chat_id: &str,
        outcome: ProjectActionSetupHandoff,
    ) {
        let mut handoffs = lock(&self.inner.setup_handoffs);
        prune_setup_handoffs(&mut handoffs);
        handoffs.insert(
            command_id.to_string(),
            StoredSetupHandoff {
                chat_id: chat_id.to_string(),
                completed_at: Instant::now(),
                outcome,
            },
        );
    }

    pub fn take_setup_handoff(
        &self,
        command_id: &str,
        chat_id: &str,
    ) -> Option<ProjectActionSetupHandoff> {
        let mut handoffs = lock(&self.inner.setup_handoffs);
        prune_setup_handoffs(&mut handoffs);
        if handoffs.get(command_id)?.chat_id != chat_id {
            return None;
        }
        handoffs.remove(command_id).map(|stored| stored.outcome)
    }
}

fn prune_setup_handoffs(handoffs: &mut HashMap<String, StoredSetupHandoff>) {
    handoffs.retain(|_, handoff| handoff.completed_at.elapsed() < SETUP_HANDOFF_TTL);
}

pub fn preferred_action<'a>(
    actions: &'a [ProjectAction],
    preferred_id: Option<&str>,
) -> Option<&'a ProjectAction> {
    preferred_id
        .and_then(|id| actions.iter().find(|action| action.id == id))
        .or_else(|| actions.iter().find(|action| !action.run_on_worktree_create))
        .or_else(|| actions.first())
}

/// Launch a saved Action in a fresh managed terminal using only host-resolved
/// paths and environment values.
pub fn launch_project_action(
    terminals: &Terminals,
    action: &ProjectAction,
    project_root: &Path,
    cwd: &Path,
    cols: u16,
    rows: u16,
) -> Result<ProjectActionRun, EngineError> {
    launch_project_action_with_environment(terminals, action, project_root, cwd, cols, rows, false)
}

/// Launch the setup Action for a newly created worktree. Setup always exposes
/// both reserved paths, even if a future repository backend returns aliases.
pub fn launch_project_setup_action(
    terminals: &Terminals,
    action: &ProjectAction,
    project_root: &Path,
    worktree: &Path,
    cols: u16,
    rows: u16,
) -> Result<ProjectActionRun, EngineError> {
    launch_project_action_with_environment(
        terminals,
        action,
        project_root,
        worktree,
        cols,
        rows,
        true,
    )
}

fn launch_project_action_with_environment(
    terminals: &Terminals,
    action: &ProjectAction,
    project_root: &Path,
    cwd: &Path,
    cols: u16,
    rows: u16,
    always_include_worktree: bool,
) -> Result<ProjectActionRun, EngineError> {
    let mut environment =
        HashMap::from([("ZERON_PROJECT_ROOT".to_string(), root_string(project_root))]);
    if always_include_worktree || cwd != project_root {
        environment.insert("ZERON_WORKTREE_PATH".to_string(), root_string(cwd));
    }
    let session = terminals.open_with_command(
        &root_string(cwd),
        cols,
        rows,
        &environment,
        &action.command,
    )?;
    Ok(ProjectActionRun {
        action_id: action.id.clone(),
        action_name: action.name.clone(),
        terminal: session,
    })
}

fn empty_file() -> ProjectActionsFile {
    ProjectActionsFile {
        version: STORE_VERSION,
        projects: BTreeMap::new(),
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn root_string(project_root: &Path) -> String {
    project_root.to_string_lossy().to_string()
}

fn ensure_project_root(
    project: &StoredProjectActions,
    project_root: &Path,
) -> Result<(), EngineError> {
    if project.project_root == root_string(project_root) {
        Ok(())
    } else {
        Err(EngineError::Other(
            "Project identity no longer matches its stored action configuration".into(),
        ))
    }
}

fn normalize_draft(draft: ProjectActionDraft) -> Result<ProjectActionDraft, EngineError> {
    let name = draft.name.trim().to_string();
    if name.is_empty() {
        return Err(EngineError::Other("Action name is required".into()));
    }
    if name.chars().count() > MAX_PROJECT_ACTION_NAME_CHARS {
        return Err(EngineError::Other(format!(
            "Action name must not exceed {MAX_PROJECT_ACTION_NAME_CHARS} characters"
        )));
    }
    let command = draft.command.trim().to_string();
    if command.is_empty() {
        return Err(EngineError::Other("Action command is required".into()));
    }
    if command.len() > MAX_PROJECT_ACTION_COMMAND_BYTES {
        return Err(EngineError::Other(format!(
            "Action command must not exceed {MAX_PROJECT_ACTION_COMMAND_BYTES} bytes"
        )));
    }
    Ok(ProjectActionDraft {
        name,
        command,
        icon: draft.icon,
        run_on_worktree_create: draft.run_on_worktree_create,
    })
}

fn unique_action_id(name: &str, actions: &[ProjectAction]) -> String {
    let mut slug = String::new();
    let mut needs_separator = false;
    for ch in name.chars().flat_map(char::to_lowercase) {
        if ch.is_alphanumeric() {
            if needs_separator && !slug.is_empty() {
                slug.push('-');
            }
            needs_separator = false;
            slug.push(ch);
        } else {
            needs_separator = true;
        }
    }
    if slug.is_empty() {
        slug.push_str("action");
    }
    truncate_utf8(&mut slug, MAX_PROJECT_ACTION_ID_BYTES);
    if !actions.iter().any(|action| action.id == slug) {
        return slug;
    }
    for suffix in 2usize.. {
        let suffix = format!("-{suffix}");
        let mut candidate = slug.clone();
        truncate_utf8(
            &mut candidate,
            MAX_PROJECT_ACTION_ID_BYTES.saturating_sub(suffix.len()),
        );
        candidate.push_str(&suffix);
        if !actions.iter().any(|action| action.id == candidate) {
            return candidate;
        }
    }
    unreachable!("the numeric action id suffix space is unbounded")
}

fn truncate_utf8(value: &mut String, max_bytes: usize) {
    if value.len() <= max_bytes {
        return;
    }
    let mut boundary = max_bytes;
    while !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    value.truncate(boundary);
    while value.ends_with('-') {
        value.pop();
    }
}

fn persist(path: &Path, file: &ProjectActionsFile) -> Result<(), EngineError> {
    let mut bytes = serde_json::to_vec_pretty(file)
        .map_err(|err| EngineError::Other(format!("serialize project Actions: {err}")))?;
    bytes.push(b'\n');
    let parent = path
        .parent()
        .ok_or_else(|| EngineError::Other("Project Actions store has no parent".into()))?;
    std::fs::create_dir_all(parent)?;
    let temp_path = parent.join(format!(".{STORE_FILE}.tmp-{}", Uuid::new_v4()));
    let result = (|| -> Result<(), EngineError> {
        let mut temp = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp_path)?;
        temp.write_all(&bytes)?;
        temp.sync_all()?;
        std::fs::rename(&temp_path, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp_path);
    }
    result
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectFile {
    actions: Vec<ProjectFileAction>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProjectFileAction {
    name: String,
    command: String,
    icon: ProjectActionIcon,
    #[serde(default)]
    run_on_worktree_create: bool,
}

fn snapshot_from_actions(
    space_id: &str,
    project_root: &Path,
    actions: Vec<ProjectAction>,
) -> ProjectActionsSnapshot {
    let (mut importable_actions, project_file_issue) = read_project_file(project_root);
    importable_actions.retain(|candidate| {
        !actions.iter().any(|saved| {
            saved.command == candidate.command
                || saved.name.to_lowercase() == candidate.name.to_lowercase()
        })
    });
    ProjectActionsSnapshot {
        space_id: space_id.to_string(),
        actions,
        importable_actions,
        project_file_issue,
    }
}

fn read_project_file(project_root: &Path) -> (Vec<ProjectActionDraft>, Option<String>) {
    let path = project_root.join(PROJECT_FILE);
    let file = match open_project_file(&path) {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return (Vec::new(), None),
        Err(err) => return project_file_issue(format!("could not read file: {err}")),
    };
    let bytes = match read_project_file_bytes(file) {
        Ok(bytes) => bytes,
        Err(err) => return project_file_issue(err),
    };
    let file: ProjectFile = match serde_json::from_slice(&bytes) {
        Ok(file) => file,
        Err(err) => return project_file_issue(format!("invalid JSON: {err}")),
    };
    if file.actions.len() > MAX_PROJECT_ACTIONS {
        return project_file_issue(format!(
            "file contains more than {MAX_PROJECT_ACTIONS} actions"
        ));
    }
    let mut actions = Vec::with_capacity(file.actions.len());
    for action in file.actions {
        let draft = ProjectActionDraft {
            name: action.name,
            command: action.command,
            icon: action.icon,
            run_on_worktree_create: action.run_on_worktree_create,
        };
        match normalize_draft(draft) {
            Ok(draft) => actions.push(draft),
            Err(err) => return project_file_issue(err.to_string()),
        }
    }
    (actions, None)
}

fn open_project_file(path: &Path) -> std::io::Result<std::fs::File> {
    // Reject known special files before opening them. The handle check below
    // is still required: the repository can replace this path after metadata.
    // Windows metadata itself opens the target, so use the controlled open there.
    #[cfg(not(windows))]
    if !std::fs::metadata(path)?.is_file() {
        return Err(std::io::Error::other("expected a regular file"));
    }
    open_regular_project_file(path)
}

fn open_regular_project_file(path: &Path) -> std::io::Result<std::fs::File> {
    let invalid = || std::io::Error::other("expected a regular file");
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // A concurrent replacement with a FIFO must not wait for a writer.
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::SECURITY_IDENTIFICATION;
        // An untrusted path may resolve to a pipe; do not allow its server to
        // impersonate this process even though we reject the resulting handle.
        options.security_qos_flags(SECURITY_IDENTIFICATION);
    }
    let file = options.open(path)?;
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{FILE_TYPE_DISK, GetFileType};
        // Windows metadata attributes alone do not distinguish all devices.
        // SAFETY: the handle belongs to the live File and is only queried.
        if unsafe { GetFileType(file.as_raw_handle()) } != FILE_TYPE_DISK {
            return Err(invalid());
        }
    }
    if !file.metadata()?.is_file() {
        return Err(invalid());
    }
    Ok(file)
}

fn read_project_file_bytes(reader: impl Read) -> Result<Vec<u8>, String> {
    // Bound the actual read, even if the file grows or reports an inaccurate
    // size. The extra byte detects overflow without accepting truncated JSON.
    let mut bytes = Vec::new();
    reader
        .take(MAX_PROJECT_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|err| format!("could not read file: {err}"))?;
    if bytes.len() as u64 > MAX_PROJECT_FILE_BYTES {
        return Err(format!(
            "file exceeds the {MAX_PROJECT_FILE_BYTES} byte limit"
        ));
    }
    Ok(bytes)
}

fn project_file_issue(message: String) -> (Vec<ProjectActionDraft>, Option<String>) {
    (Vec::new(), Some(format!("Invalid zeron.json: {message}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn draft(name: &str, command: &str) -> ProjectActionDraft {
        ProjectActionDraft {
            name: name.into(),
            command: command.into(),
            icon: ProjectActionIcon::Play,
            run_on_worktree_create: false,
        }
    }

    fn roots() -> (TempDir, PathBuf, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let store = temp.path().join("profile");
        let project = temp.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        (temp, store, project)
    }

    #[test]
    fn missing_store_round_trips_actions() {
        let (_temp, store_root, project_root) = roots();
        let store = ProjectActionsStore::open(&store_root).unwrap();
        assert!(store.actions("space", &project_root).unwrap().is_empty());
        store
            .upsert("space", &project_root, None, draft(" Dev ", " pnpm dev "))
            .unwrap();

        let reopened = ProjectActionsStore::open(&store_root).unwrap();
        let actions = reopened.actions("space", &project_root).unwrap();
        assert_eq!(actions[0].id, "dev");
        assert_eq!(actions[0].name, "Dev");
        assert_eq!(actions[0].command, "pnpm dev");
    }

    #[test]
    fn corrupt_store_does_not_prevent_open_or_repair_on_write() {
        let (_temp, store_root, project_root) = roots();
        std::fs::create_dir_all(&store_root).unwrap();
        std::fs::write(store_root.join(STORE_FILE), b"not json").unwrap();
        let store = ProjectActionsStore::open(&store_root).unwrap();
        assert!(store.actions("space", &project_root).unwrap().is_empty());
        store
            .upsert("space", &project_root, None, draft("Test", "cargo test"))
            .unwrap();
        assert_eq!(
            ProjectActionsStore::open(&store_root)
                .unwrap()
                .actions("space", &project_root)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn setup_handoff_is_chat_scoped_and_consumed_once() {
        let (_temp, store_root, _project_root) = roots();
        let store = ProjectActionsStore::open(&store_root).unwrap();
        store.complete_setup_handoff(
            "command-1",
            "chat-1",
            ProjectActionSetupHandoff {
                setup_action: None,
                setup_error: Some("setup failed".into()),
            },
        );

        assert!(store.take_setup_handoff("command-1", "chat-2").is_none());
        let outcome = store
            .take_setup_handoff("command-1", "chat-1")
            .expect("matching sender takes handoff");
        assert_eq!(outcome.setup_error.as_deref(), Some("setup failed"));
        assert!(store.take_setup_handoff("command-1", "chat-1").is_none());
    }

    #[test]
    fn upsert_delete_ids_limits_and_root_guard() {
        let (_temp, store_root, project_root) = roots();
        let store = ProjectActionsStore::open(&store_root).unwrap();
        let first = store
            .upsert("space", &project_root, None, draft("Test", "one"))
            .unwrap()
            .actions[0]
            .clone();
        let second = store
            .upsert("space", &project_root, None, draft("Test", "two"))
            .unwrap()
            .actions[1]
            .clone();
        assert_eq!(first.id, "test");
        assert_eq!(second.id, "test-2");
        let renamed = store
            .upsert(
                "space",
                &project_root,
                Some(&first.id),
                draft("Unit tests", "cargo test"),
            )
            .unwrap();
        assert_eq!(renamed.actions[0].id, first.id);
        assert_eq!(renamed.actions[0].name, "Unit tests");
        assert_eq!(
            store
                .delete("space", &project_root, &second.id)
                .unwrap()
                .actions
                .len(),
            1
        );
        assert!(store.actions("space", &project_root.join("moved")).is_err());

        for index in 1..MAX_PROJECT_ACTIONS {
            store
                .upsert(
                    "space",
                    &project_root,
                    None,
                    draft(&format!("Action {index}"), &format!("run {index}")),
                )
                .unwrap();
        }
        assert!(
            store
                .upsert("space", &project_root, None, draft("One too many", "nope"))
                .is_err()
        );
    }

    #[test]
    fn changing_setup_unmarks_the_previous_action_atomically() {
        let (_temp, store_root, project_root) = roots();
        let store = ProjectActionsStore::open(&store_root).unwrap();
        let mut first = draft("Install", "pnpm install");
        first.run_on_worktree_create = true;
        let first_id = store
            .upsert("space", &project_root, None, first)
            .unwrap()
            .actions[0]
            .id
            .clone();
        let mut second = draft("Bootstrap", "make setup");
        second.run_on_worktree_create = true;
        let snapshot = store.upsert("space", &project_root, None, second).unwrap();
        assert!(!snapshot.actions[0].run_on_worktree_create);
        assert!(snapshot.actions[1].run_on_worktree_create);

        let reopened = ProjectActionsStore::open(&store_root).unwrap();
        assert_eq!(
            reopened
                .actions("space", &project_root)
                .unwrap()
                .iter()
                .filter(|action| action.run_on_worktree_create)
                .count(),
            1
        );
        assert_eq!(snapshot.actions[0].id, first_id);
    }

    #[test]
    fn project_file_is_strict_limited_and_deduplicated() {
        let (_temp, store_root, project_root) = roots();
        let store = ProjectActionsStore::open(&store_root).unwrap();
        std::fs::write(
            project_root.join(PROJECT_FILE),
            r#"{"actions":[
                {"name":"Dev","command":"pnpm dev","icon":"play"},
                {"name":"Lint","command":"pnpm lint","icon":"lint","runOnWorktreeCreate":true}
            ]}"#,
        )
        .unwrap();
        let listed = store.snapshot("space", &project_root).unwrap();
        assert_eq!(listed.importable_actions.len(), 2);
        store
            .upsert("space", &project_root, None, draft("DEV", "another"))
            .unwrap();
        let listed = store.snapshot("space", &project_root).unwrap();
        assert_eq!(listed.importable_actions.len(), 1);
        assert_eq!(listed.importable_actions[0].name, "Lint");

        std::fs::write(
            project_root.join(PROJECT_FILE),
            r#"{"actions":[],"unexpected":true}"#,
        )
        .unwrap();
        let invalid = store.snapshot("space", &project_root).unwrap();
        assert!(invalid.importable_actions.is_empty());
        assert!(invalid.project_file_issue.is_some());

        std::fs::write(
            project_root.join(PROJECT_FILE),
            vec![b' '; MAX_PROJECT_FILE_BYTES as usize + 1],
        )
        .unwrap();
        assert!(
            store
                .snapshot("space", &project_root)
                .unwrap()
                .project_file_issue
                .unwrap()
                .contains("exceeds")
        );
    }

    #[cfg(unix)]
    #[test]
    fn project_file_rejects_device_symlinks() {
        let (_temp, store_root, project_root) = roots();
        let store = ProjectActionsStore::open(&store_root).unwrap();
        for target in ["/dev/null", "/dev/zero"] {
            let path = project_root.join(PROJECT_FILE);
            std::os::unix::fs::symlink(target, &path).unwrap();
            let snapshot = store.snapshot("space", &project_root).unwrap();
            assert!(snapshot.importable_actions.is_empty());
            assert!(
                snapshot
                    .project_file_issue
                    .unwrap()
                    .contains("regular file")
            );
            std::fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn project_file_enforces_actual_read_limit() {
        let (_temp, store_root, project_root) = roots();
        let store = ProjectActionsStore::open(&store_root).unwrap();
        let path = project_root.join(PROJECT_FILE);
        let mut json =
            br#"{"actions":[{"name":"Test","command":"echo test","icon":"test"}]}"#.to_vec();
        json.resize(MAX_PROJECT_FILE_BYTES as usize, b' ');
        std::fs::write(&path, &json).unwrap();
        let snapshot = store.snapshot("space", &project_root).unwrap();
        assert!(snapshot.project_file_issue.is_none());
        assert_eq!(snapshot.importable_actions.len(), 1);

        // Grow a file after it has been opened and validated. A metadata-only
        // limit would miss the extra bytes, which are still valid JSON whitespace.
        let file = open_project_file(&path).unwrap();
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(&[b' '; 4096])
            .unwrap();
        assert!(
            read_project_file_bytes(file)
                .unwrap_err()
                .contains("exceeds")
        );

        // Even a reader with no EOF must consume only the limit plus one byte.
        let mut stream = std::io::repeat(b' ').take(u64::MAX);
        assert!(
            read_project_file_bytes(&mut stream)
                .unwrap_err()
                .contains("exceeds")
        );
        assert_eq!(u64::MAX - stream.limit(), MAX_PROJECT_FILE_BYTES + 1);
    }

    #[cfg(windows)]
    #[test]
    fn project_file_rejects_windows_devices() {
        assert!(
            open_regular_project_file(Path::new(r"\\.\NUL"))
                .unwrap_err()
                .to_string()
                .contains("regular file")
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn project_actions_rpc_does_not_block_async_worker() {
        use zeron_rpc::{RpcService, methods};
        let temp = tempfile::tempdir().unwrap();
        let core = crate::EngineCore::assemble(
            temp.path(),
            Arc::new(crate::HarnessRegistry::new()),
            zeron_proto::HarnessId::Mock,
            None,
        )
        .unwrap();
        core.workspace
            .create_space(
                "space",
                &core.device_id,
                &temp.path().to_string_lossy(),
                None,
                true,
            )
            .unwrap();
        let saved = core
            .project_actions
            .upsert("space", temp.path(), None, draft("Test", "echo test"))
            .unwrap();
        let rpc = core.rpc_service();
        for (method, params) in [
            (
                methods::LIST_PROJECT_ACTIONS,
                serde_json::json!({"spaceId": "space"}),
            ),
            (
                methods::UPSERT_PROJECT_ACTION,
                serde_json::json!({"spaceId": "space", "action": {"name": "Build", "command": "echo build", "icon": "build"}}),
            ),
            (
                methods::DELETE_PROJECT_ACTION,
                serde_json::json!({"spaceId": "space", "actionId": saved.actions[0].id}),
            ),
        ] {
            let store = core.project_actions.clone();
            let (ready_tx, ready_rx) = std::sync::mpsc::channel();
            let (release_tx, release_rx) = std::sync::mpsc::channel();
            let blocker = std::thread::spawn(move || {
                let _guard = lock(&store.inner.state);
                ready_tx.send(()).unwrap();
                // A synchronous regression must fail, not deadlock the test.
                release_rx.recv_timeout(Duration::from_secs(5)).is_ok()
            });
            ready_rx.recv().unwrap();
            let mut request = std::pin::pin!(rpc.handle(method, params));
            let poll = futures::poll!(request.as_mut());
            let yielded = poll.is_pending();
            assert!(
                rpc.handle(methods::LOCAL_DEVICE, serde_json::json!({}))
                    .await
                    .is_ok()
            );
            let _ = release_tx.send(());
            let released = blocker.join().unwrap();
            assert!(yielded && released, "{method} blocked the async worker");
            request.await.unwrap();
        }
    }

    #[cfg(unix)]
    #[test]
    fn project_file_regular_symlink_remains_importable() {
        let (temp, store_root, project_root) = roots();
        let target = temp.path().join("actions.json");
        std::fs::write(
            &target,
            r#"{"actions":[{"name":"Setup","command":"echo setup","icon":"configure","runOnWorktreeCreate":true}]}"#,
        )
        .unwrap();
        std::os::unix::fs::symlink(&target, project_root.join(PROJECT_FILE)).unwrap();
        let store = ProjectActionsStore::open(&store_root).unwrap();
        let snapshot = store.snapshot("space", &project_root).unwrap();
        assert!(snapshot.project_file_issue.is_none());
        assert!(snapshot.actions.is_empty());
        assert_eq!(snapshot.importable_actions.len(), 1);
        assert!(
            store
                .setup_action("space", &project_root)
                .unwrap()
                .is_none()
        );
        let saved = store
            .upsert(
                "space",
                &project_root,
                None,
                snapshot.importable_actions[0].clone(),
            )
            .unwrap();
        assert_eq!(saved.actions.len(), 1);
        assert!(saved.importable_actions.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn project_file_special_files_do_not_block() {
        const CHILD_ENV: &str = "ZERON_TEST_PROJECT_FILE_SPECIAL_FILES";
        if std::env::var_os(CHILD_ENV).is_some() {
            use std::os::unix::{ffi::OsStrExt, fs::symlink, net::UnixListener};
            let (_temp, store_root, project_root) = roots();
            let store = ProjectActionsStore::open(&store_root).unwrap();
            let path = project_root.join(PROJECT_FILE);
            let fifo = project_root.join("fifo");
            let fifo_name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
            // SAFETY: a valid NUL-terminated path in an isolated temporary directory.
            assert_eq!(unsafe { libc::mkfifo(fifo_name.as_ptr(), 0o600) }, 0);
            symlink(&fifo, &path).unwrap();
            let assert_rejected = || {
                let snapshot = store.snapshot("space", &project_root).unwrap();
                assert!(snapshot.importable_actions.is_empty());
                assert!(
                    snapshot
                        .project_file_issue
                        .unwrap()
                        .contains("regular file")
                );
            };
            assert_rejected();
            std::fs::remove_file(&path).unwrap();
            // Simulate replacement after the preliminary metadata check. The
            // open itself must not block, and must recheck the resulting handle.
            std::fs::write(&path, br#"{"actions":[]}"#).unwrap();
            assert!(std::fs::metadata(&path).unwrap().is_file());
            std::fs::rename(&fifo, &path).unwrap();
            assert!(
                open_regular_project_file(&path)
                    .unwrap_err()
                    .to_string()
                    .contains("regular file")
            );
            assert_rejected();
            std::fs::remove_file(&path).unwrap();
            let socket = UnixListener::bind(&path).unwrap();
            assert_rejected();
            drop(socket);
            std::fs::remove_file(&path).unwrap();
            std::fs::create_dir(&path).unwrap();
            assert_rejected();
            return;
        }
        // Isolate potential blocking regressions so they fail instead of hanging
        // the test runner forever. No FIFO writer is ever started.
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "project_actions::tests::project_file_special_files_do_not_block",
                "--nocapture",
            ])
            .env(CHILD_ENV, "1")
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success(), "special-file checks failed: {status}");
                break;
            }
            if Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("project file discovery blocked on a special file");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn profile_roots_do_not_share_actions() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        std::fs::create_dir(&project).unwrap();
        let profile_a = ProjectActionsStore::open(&temp.path().join("profile-a")).unwrap();
        let profile_b = ProjectActionsStore::open(&temp.path().join("profile-b")).unwrap();
        profile_a
            .upsert("space", &project, None, draft("Dev", "pnpm dev"))
            .unwrap();
        assert!(profile_b.actions("space", &project).unwrap().is_empty());
    }

    #[test]
    fn primary_prefers_saved_choice_then_non_setup() {
        let setup = ProjectAction {
            id: "setup".into(),
            name: "Setup".into(),
            command: "setup".into(),
            icon: ProjectActionIcon::Configure,
            run_on_worktree_create: true,
        };
        let run = ProjectAction {
            id: "run".into(),
            name: "Run".into(),
            command: "run".into(),
            icon: ProjectActionIcon::Play,
            run_on_worktree_create: false,
        };
        let actions = [setup, run];
        assert_eq!(preferred_action(&actions, None).unwrap().id, "run");
        assert_eq!(
            preferred_action(&actions, Some("setup")).unwrap().id,
            "setup"
        );
        assert_eq!(
            preferred_action(&actions, Some("missing")).unwrap().id,
            "run"
        );
    }
}
