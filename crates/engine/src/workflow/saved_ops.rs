//! The service's saved-workflow operations: list, get, save (with approval),
//! delete, and a workflow's run history. The file work is `saved.rs`.

use std::path::PathBuf;

use serde_json::Value;
use zeron_proto::{
    SavedArg, SavedRunRef, SavedScope, SavedWorkflowDetail, SavedWorkflowList, UserInputQuestion,
    WorkflowRunHeader, valid_saved_name,
};
use zeron_workflow::saved::{self, SavedMeta};

use super::saved::{ProjectRef, SavedError, SavedStore};
use super::{Approval, DENY_LABEL, SAVE_LABEL, WorkflowService, lock, prompts};

/// Where a request's project comes from.
#[derive(Debug, Clone, Default)]
pub struct SavedContext {
    /// The chat whose folder is the project.
    pub chat_id: Option<String>,
    /// A project (space) this device owns.
    pub space_id: Option<String>,
}

/// What to save.
#[derive(Debug, Clone)]
pub struct SaveRequest {
    pub name: String,
    pub description: String,
    pub when_to_use: Option<String>,
    /// Declared arguments; `None` keeps what the source script's own
    /// frontmatter declares (a re-save of a saved workflow), or none.
    pub args: Option<Vec<SavedArg>>,
    pub scope: SavedScope,
    pub source: SaveSource,
    /// A person pressed Save in a dialog: their click is the approval. Never
    /// set for an agent's call.
    pub by_user: bool,
    /// With `by_user`: replace an existing file (the dialog asked first).
    pub overwrite: bool,
}

#[derive(Debug, Clone)]
pub enum SaveSource {
    /// The script a run of this chat used (an ad-hoc run worth keeping).
    FromRun(String),
    Script(String),
}

#[derive(Debug, Clone)]
pub struct SaveOutcome {
    pub summary: zeron_proto::SavedWorkflowSummary,
    pub path: PathBuf,
    pub overwrote: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum SaveError {
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Saved(#[from] SavedError),
    #[error("not saved: {0}")]
    Denied(String),
}

impl WorkflowService {
    pub fn saved_store(&self) -> SavedStore {
        lock(&self.shared.saved).clone()
    }

    /// Point the global scope somewhere else (tests; `ZERON_WORKFLOWS_DIR`
    /// covers the app).
    pub fn set_global_workflows_dir(&self, dir: PathBuf) {
        *lock(&self.shared.saved) = SavedStore::new(dir);
    }

    /// The project a request names. `Ok(None)`: no project given.
    pub fn saved_project(&self, ctx: &SavedContext) -> Result<Option<ProjectRef>, String> {
        if let Some(chat_id) = ctx.chat_id.as_deref() {
            let chat = self
                .shared
                .workspace
                .chat(chat_id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("no such chat: {chat_id}"))?;
            let root = chat
                .cwd
                .as_deref()
                .map(PathBuf::from)
                .and_then(|p| p.canonicalize().ok())
                .filter(|p| p.is_dir())
                .ok_or("the chat's project folder is not available on this device")?;
            return Ok(Some(ProjectRef {
                root,
                space_id: chat.space_id.clone(),
            }));
        }
        if let Some(space_id) = ctx.space_id.as_deref() {
            let space = self
                .shared
                .workspace
                .space(space_id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("no such project: {space_id}"))?;
            if space.device_id != self.shared.workspace.device_id() {
                return Err("that project is on another device; address that device".into());
            }
            let root = PathBuf::from(&space.path)
                .canonicalize()
                .ok()
                .filter(|p| p.is_dir())
                .ok_or("the project's folder is not available on this device")?;
            return Ok(Some(ProjectRef {
                root,
                space_id: Some(space.id),
            }));
        }
        Ok(None)
    }

    /// Every project this device owns whose folder exists.
    pub fn local_projects(&self) -> Vec<ProjectRef> {
        let device = self.shared.workspace.device_id().to_owned();
        let mut seen = std::collections::HashSet::new();
        self.shared
            .workspace
            .read_spaces()
            .unwrap_or_default()
            .into_iter()
            .filter(|s| s.device_id == device)
            .filter_map(|s| {
                let root = PathBuf::from(&s.path).canonicalize().ok()?;
                (root.is_dir() && seen.insert(root.clone())).then_some(ProjectRef {
                    root,
                    space_id: Some(s.id),
                })
            })
            .collect()
    }

    /// Saved workflows: for one project (`ctx`), or — `all` — for every
    /// project on this device (the Settings page).
    pub fn saved_list(&self, ctx: &SavedContext, all: bool) -> Result<SavedWorkflowList, String> {
        let projects = if all {
            self.local_projects()
        } else {
            self.saved_project(ctx)?.into_iter().collect()
        };
        Ok(self.saved_store().list(&projects))
    }

    /// One workflow with its script and the analysis an approval would show.
    pub fn saved_get(
        &self,
        ctx: &SavedContext,
        name: &str,
        scope: Option<SavedScope>,
    ) -> Result<SavedWorkflowDetail, String> {
        let project = self.saved_project(ctx)?;
        let loaded = self
            .saved_store()
            .resolve(name, scope, project.as_ref())
            .map_err(|e| e.to_string())?;
        let (graph, diagnostics) =
            match zeron_workflow::analyze(&format!("{name}.star"), &loaded.text) {
                Ok(a) => (Some(a.graph), Vec::new()),
                Err(d) => (None, d.iter().map(ToString::to_string).collect()),
            };
        Ok(SavedWorkflowDetail {
            summary: loaded.summary,
            script: loaded.text,
            graph,
            diagnostics,
        })
    }

    /// Delete a saved workflow's file (a person's action; there is no agent
    /// tool for it).
    pub fn saved_delete(
        &self,
        ctx: &SavedContext,
        name: &str,
        scope: SavedScope,
    ) -> Result<(), String> {
        let project = self.saved_project(ctx)?;
        if scope == SavedScope::Project && project.is_none() {
            return Err("name the project (chatId or spaceId) of the workflow to delete".into());
        }
        self.saved_store()
            .delete(name, scope, project.as_ref())
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// Save a workflow file. An agent's request is put to the user as a
    /// question on the chat's live turn (the same machinery as a workflow
    /// approval); a person's dialog is its own approval (`by_user`).
    ///
    /// Everything that can be checked is checked **before** anyone is asked:
    /// name, frontmatter, script analysis, the source run's ownership.
    pub async fn saved_save(
        &self,
        chat_id: &str,
        req: SaveRequest,
    ) -> Result<SaveOutcome, SaveError> {
        valid_saved_name(&req.name).map_err(SaveError::Invalid)?;
        if !req.scope.writable() {
            return Err(SaveError::Invalid(
                "built-in workflows are read-only; save a copy as global or project".into(),
            ));
        }
        let ctx = SavedContext {
            chat_id: Some(chat_id.to_owned()),
            space_id: None,
        };
        let project = self.saved_project(&ctx).map_err(SaveError::Invalid)?;
        if req.scope == SavedScope::Project && project.is_none() {
            return Err(SaveError::Invalid("the chat has no project folder".into()));
        }
        let script = match &req.source {
            SaveSource::Script(s) => s.clone(),
            SaveSource::FromRun(run_id) => {
                let meta =
                    self.shared.store.read_meta(run_id).map_err(|_| {
                        SaveError::Invalid(format!("no such workflow run: {run_id}"))
                    })?;
                if meta.chat_id != chat_id {
                    return Err(SaveError::Invalid(format!(
                        "run {run_id} belongs to another chat; you can only save your own chat's runs"
                    )));
                }
                self.shared
                    .store
                    .read_script(run_id)
                    .map_err(|e| SaveError::Invalid(e.to_string()))?
            }
        };
        // Arguments: explicit, else what the script's own frontmatter says.
        let args = match req.args.clone() {
            Some(a) => a,
            None if saved::has_frontmatter(&script) => {
                saved::parse(&format!("{}.star", req.name), None, &script)
                    .map(|f| f.meta.args)
                    .unwrap_or_default()
            }
            None => Vec::new(),
        };
        let meta = SavedMeta {
            name: Some(req.name.clone()),
            description: req.description.trim().to_owned(),
            when_to_use: req
                .when_to_use
                .as_deref()
                .map(str::trim)
                .filter(|w| !w.is_empty())
                .map(str::to_owned),
            args,
        };
        // Render + analyse now (errors are the caller's to fix), write later.
        let text = saved::render(&meta, &script).map_err(SaveError::Invalid)?;
        let analysis = zeron_workflow::analyze(&format!("{}.star", req.name), &text)
            .map_err(|d| SaveError::Saved(SavedError::Diagnostics(d)))?;

        let store = self.saved_store();
        let root = project.as_ref().map(|p| p.root.clone());
        let dir = match req.scope {
            SavedScope::Global => store.global_dir().to_path_buf(),
            _ => super::saved::project_dir(
                root.as_deref().unwrap_or_else(|| std::path::Path::new("")),
            ),
        };
        let target = dir.join(format!("{}.star", req.name));
        let exists = std::fs::symlink_metadata(&target).is_ok();
        let overwrite = if req.by_user {
            req.overwrite
        } else {
            // What else answers to this name here?
            let list = store.list(project.as_slice());
            let shadow = list
                .workflows
                .iter()
                .filter(|w| w.name == req.name && w.scope != req.scope)
                .map(|w| (w.scope.precedence() > req.scope.precedence(), w.scope))
                .min_by_key(|(hides, s)| (!*hides, s.precedence()));
            let path_text = match (req.scope, root.as_deref()) {
                (SavedScope::Project, Some(r)) => target.strip_prefix(r).map_or_else(
                    |_| target.display().to_string(),
                    |p| p.display().to_string(),
                ),
                _ => target.display().to_string(),
            };
            let question = UserInputQuestion {
                id: "workflow-save".into(),
                header: "Save workflow".into(),
                question: prompts::save_text(&prompts::SaveFacts {
                    name: &req.name,
                    scope: req.scope,
                    path: &path_text,
                    replaces: exists,
                    description: &meta.description,
                    when_to_use: meta.when_to_use.as_deref(),
                    args: &meta.args,
                    shadow,
                    graph: &analysis.graph,
                    script: saved::strip_frontmatter(&script),
                }),
                options: vec![SAVE_LABEL.into(), DENY_LABEL.into()],
                multi_select: false,
                prefill: None,
                multiline: false,
                meta: None,
            };
            let approver = lock(&self.shared.approver).clone();
            if let Approval::Denied(reason) = approver.approve(chat_id, question).await {
                return Err(SaveError::Denied(reason));
            }
            // What the person approved: replacing, if the question said so.
            exists
        };
        let written = store.write(req.scope, root.as_deref(), &meta, &script, overwrite)?;
        let list = store.list(project.as_slice());
        let summary = list
            .workflows
            .into_iter()
            .find(|w| w.name == req.name && w.scope == req.scope)
            .ok_or_else(|| {
                SaveError::Invalid("the saved workflow could not be read back".into())
            })?;
        Ok(SaveOutcome {
            summary,
            path: written.path,
            overwrote: written.overwrote,
        })
    }

    /// Runs of one saved workflow on this device, newest first. A project
    /// workflow's runs are those of that project only.
    pub fn saved_runs(
        &self,
        name: &str,
        scope: SavedScope,
        project: Option<&ProjectRef>,
        limit: usize,
    ) -> Vec<WorkflowRunHeader> {
        let root = project.map(|p| p.root.to_string_lossy().into_owned());
        let mut metas: Vec<_> = self
            .shared
            .store
            .list_metas()
            .into_iter()
            .filter(|m| {
                m.saved.as_ref().is_some_and(|s| {
                    s == &SavedRunRef {
                        name: name.to_owned(),
                        scope,
                    }
                }) && (scope != SavedScope::Project || Some(&m.project_root) == root.as_ref())
            })
            .collect();
        metas.sort_by_key(|m| std::cmp::Reverse((m.created_at, m.run_id.clone())));
        metas.truncate(limit);
        let ids: Vec<String> = metas.iter().map(|m| m.run_id.clone()).collect();
        let by_id: std::collections::HashMap<String, WorkflowRunHeader> = self
            .list(None)
            .into_iter()
            .filter(|h| ids.contains(&h.run_id))
            .map(|h| (h.run_id.clone(), h))
            .collect();
        ids.into_iter()
            .filter_map(|id| by_id.get(&id).cloned())
            .collect()
    }
}

/// `args` JSON as the RPC and MCP layers receive it → declarations. Either a
/// list of `{name, type, …}` (order kept) or — like the frontmatter reads — an
/// object keyed by argument name (`{"base": {"type": "string", …}}`).
pub fn parse_args_declaration(value: &Value) -> Result<Vec<SavedArg>, String> {
    let one = |name: Option<&str>, v: &Value| -> Result<SavedArg, String> {
        let mut v = v.clone();
        if let (Some(name), Some(o)) = (name, v.as_object_mut()) {
            o.insert("name".into(), Value::String(name.to_owned()));
        }
        serde_json::from_value::<SavedArg>(v.clone()).map_err(|e| {
            format!("bad argument declaration {v}: {e} (expected name, type [string|int|number|bool|json], required?, default?, description?)")
        })
    };
    match value {
        Value::Array(items) => items.iter().map(|v| one(None, v)).collect(),
        Value::Object(map) => map.iter().map(|(k, v)| one(Some(k), v)).collect(),
        Value::Null => Ok(Vec::new()),
        _ => {
            Err("`args` must be an object keyed by argument name, or a list of declarations".into())
        }
    }
}
