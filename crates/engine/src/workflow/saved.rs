//! Saved workflows on disk: scanning, resolving, writing, deleting.
//!
//! A saved workflow is `<name>.star` in one of three places:
//!
//! * **project** — `<project>/.zeron/workflows/`, committed with the project;
//! * **global** — `$ZERON_WORKFLOWS_DIR`, else `~/.zeron/workflows/` (user
//!   files, next to `~/.zeron/worktrees`, not under the data dir);
//! * **built-in** — compiled into the app (`zeron_workflow::saved::builtins`).
//!
//! A project's file shadows a global one of the same name, which shadows a
//! built-in. The format (frontmatter) is `zeron_workflow::saved`.
//!
//! No cache and no watcher: a listing reads two tiny directories, so every
//! call rescans and an edit in an editor is visible on the next call. Callers
//! refresh when they show the list and after each save or delete.
//!
//! Safety, in the order a hostile repository would attack it:
//!
//! * a project's files are untrusted data. Names are the file stem and must be
//!   slugs; the project's `.zeron/workflows` folder and every file in it must
//!   be a real directory/regular file inside the project (no symlinks), at
//!   most 256 KB; at most [`MAX_FILES`] are read; one bad file never fails the
//!   listing, it is reported and skipped;
//! * writes only ever target `<dir>/<validated name>.star`, are created
//!   through a temp file in the same directory and renamed into place, never
//!   follow a symlink, and refuse to replace an existing file unless the
//!   caller says so.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use zeron_proto::saved_workflow::*;
use zeron_workflow::Diagnostic;
use zeron_workflow::limits::MAX_SCRIPT_BYTES;
use zeron_workflow::saved::{self, SavedMeta};

/// Files read per folder; the rest are ignored (and said so).
pub const MAX_FILES: usize = 200;
const EXT: &str = "star";

/// Where the global workflows live: `ZERON_WORKFLOWS_DIR` (tests, relocated
/// homes; empty reads as unset), else `~/.zeron/workflows`.
pub fn default_global_dir() -> PathBuf {
    std::env::var_os("ZERON_WORKFLOWS_DIR")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::repos::home_dir().join(".zeron").join("workflows"))
}

/// A project the scan covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectRef {
    pub root: PathBuf,
    pub space_id: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum SavedError {
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    NotFound(String),
    #[error("a {scope} workflow named {name:?} already exists")]
    Exists { name: String, scope: SavedScope },
    #[error("{}", zeron_workflow::diagnostic::render(.0))]
    Diagnostics(Vec<Diagnostic>),
    #[error("{0}")]
    Io(String),
}

/// A workflow read from disk (or the built-ins) with its full text.
#[derive(Debug, Clone)]
pub struct Loaded {
    pub summary: SavedWorkflowSummary,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct SavedStore {
    global_dir: PathBuf,
}

/// What a write did.
#[derive(Debug, Clone)]
pub struct Written {
    pub path: PathBuf,
    pub overwrote: bool,
    pub text: String,
}

fn modified_ms(meta: &std::fs::Metadata) -> Option<i64> {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
}

pub fn project_dir(root: &Path) -> PathBuf {
    root.join(".zeron").join("workflows")
}

impl SavedStore {
    pub fn new(global_dir: PathBuf) -> Self {
        Self { global_dir }
    }

    pub fn global_dir(&self) -> &Path {
        &self.global_dir
    }

    fn dir_for(&self, scope: SavedScope, project: Option<&Path>) -> Result<PathBuf, SavedError> {
        match scope {
            SavedScope::Global => Ok(self.global_dir.clone()),
            SavedScope::Project => project
                .map(project_dir)
                .ok_or_else(|| SavedError::Invalid("a project workflow needs a project".into())),
            SavedScope::Builtin => Err(SavedError::Invalid(
                "built-in workflows are read-only".into(),
            )),
        }
    }

    // ── reading ────────────────────────────────────────────────────────────

    /// The built-ins, as summaries with their text.
    pub fn builtins(&self) -> Vec<Loaded> {
        saved::builtins()
            .iter()
            .filter_map(|b| {
                let file =
                    saved::parse(&format!("{}.star", b.name), Some(b.name), b.source).ok()?;
                Some(Loaded {
                    summary: SavedWorkflowSummary {
                        name: b.name.to_owned(),
                        scope: SavedScope::Builtin,
                        description: file.meta.description,
                        when_to_use: file.meta.when_to_use,
                        args: file.meta.args,
                        path: None,
                        project_root: None,
                        space_id: None,
                        modified_at: None,
                        shadowed_by: None,
                        shadows: Vec::new(),
                    },
                    text: b.source.to_owned(),
                })
            })
            .collect()
    }

    /// Every usable file in one folder, plus what could not be used.
    fn scan(
        &self,
        scope: SavedScope,
        dir: &Path,
        project: Option<&ProjectRef>,
    ) -> (Vec<Loaded>, Vec<SavedWorkflowInvalid>) {
        let mut ok = Vec::new();
        let mut bad = Vec::new();
        let invalid = |path: &Path, reason: String| SavedWorkflowInvalid {
            path: path.to_string_lossy().into_owned(),
            scope,
            reason,
            project_root: project.map(|p| p.root.to_string_lossy().into_owned()),
            space_id: project.and_then(|p| p.space_id.clone()),
        };
        // A project's folder must be the project's own: refuse symlinks on the
        // way (a cloned repository can contain any link it likes).
        if let Some(p) = project {
            for part in [p.root.join(".zeron"), dir.to_path_buf()] {
                match std::fs::symlink_metadata(&part) {
                    Ok(m) if m.file_type().is_symlink() => {
                        bad.push(invalid(
                            &part,
                            "ignored: the folder is a symbolic link".into(),
                        ));
                        return (ok, bad);
                    }
                    Ok(_) => {}
                    Err(_) => return (ok, bad),
                }
            }
        }
        let Ok(read) = std::fs::read_dir(dir) else {
            return (ok, bad);
        };
        let mut names: Vec<(String, PathBuf)> = read
            .flatten()
            .filter_map(|e| {
                let path = e.path();
                if path.extension().and_then(|x| x.to_str()) != Some(EXT) {
                    return None;
                }
                let stem = path.file_stem()?.to_string_lossy().into_owned();
                Some((stem, path))
            })
            .collect();
        names.sort();
        if names.len() > MAX_FILES {
            bad.push(invalid(
                dir,
                format!("only the first {MAX_FILES} workflows in this folder are read"),
            ));
            names.truncate(MAX_FILES);
        }
        for (name, path) in names {
            let meta = if project.is_some() {
                std::fs::symlink_metadata(&path)
            } else {
                std::fs::metadata(&path)
            };
            let meta = match meta {
                Ok(m) if m.is_file() => m,
                Ok(_) => {
                    bad.push(invalid(&path, "ignored: not a regular file".into()));
                    continue;
                }
                Err(e) => {
                    bad.push(invalid(&path, format!("could not read it: {e}")));
                    continue;
                }
            };
            if let Err(msg) = valid_saved_name(&name) {
                bad.push(invalid(&path, msg));
                continue;
            }
            if meta.len() > MAX_SCRIPT_BYTES as u64 {
                bad.push(invalid(
                    &path,
                    format!("larger than {} KB", MAX_SCRIPT_BYTES / 1024),
                ));
                continue;
            }
            let text = match std::fs::read_to_string(&path) {
                Ok(t) => t,
                Err(e) => {
                    bad.push(invalid(&path, format!("could not read it: {e}")));
                    continue;
                }
            };
            let label = format!("{name}.star");
            match saved::parse(&label, Some(&name), &text) {
                Ok(file) => ok.push(Loaded {
                    summary: SavedWorkflowSummary {
                        name,
                        scope,
                        description: file.meta.description,
                        when_to_use: file.meta.when_to_use,
                        args: file.meta.args,
                        path: Some(path.to_string_lossy().into_owned()),
                        project_root: project.map(|p| p.root.to_string_lossy().into_owned()),
                        space_id: project.and_then(|p| p.space_id.clone()),
                        modified_at: modified_ms(&meta),
                        shadowed_by: None,
                        shadows: Vec::new(),
                    },
                    text,
                }),
                Err(d) => bad.push(invalid(&path, zeron_workflow::diagnostic::render(&d))),
            }
        }
        (ok, bad)
    }

    /// Everything visible to the given projects, with texts and shadow facts:
    /// built-ins, globals, and each project's own. With exactly one project,
    /// shadowing is resolved for it (`shadowed_by`); in any case `shadows`
    /// says what a file hides.
    fn collect(&self, projects: &[ProjectRef]) -> (Vec<Loaded>, Vec<SavedWorkflowInvalid>) {
        let mut all: Vec<Loaded> = self.builtins();
        let mut invalid = Vec::new();
        let (globals, bad) = self.scan(SavedScope::Global, &self.global_dir, None);
        all.extend(globals);
        invalid.extend(bad);
        for p in projects {
            let (found, bad) = self.scan(SavedScope::Project, &project_dir(&p.root), Some(p));
            all.extend(found);
            invalid.extend(bad);
        }
        let single = projects.len() == 1;
        let pairs: Vec<(String, SavedScope)> = all
            .iter()
            .map(|l| (l.summary.name.clone(), l.summary.scope))
            .collect();
        for l in &mut all {
            let w = &mut l.summary;
            let mut scopes: Vec<SavedScope> = pairs
                .iter()
                .filter(|(n, _)| *n == w.name)
                .map(|(_, s)| *s)
                .collect();
            scopes.sort();
            scopes.dedup();
            w.shadows = scopes
                .iter()
                .copied()
                .filter(|s| s.precedence() > w.scope.precedence())
                .collect();
            if single {
                w.shadowed_by = scopes
                    .iter()
                    .copied()
                    .find(|s| s.precedence() < w.scope.precedence());
            }
        }
        all.sort_by(|a, b| {
            let key = |l: &Loaded| {
                (
                    l.summary.scope.precedence(),
                    l.summary.project_root.clone(),
                    l.summary.name.clone(),
                )
            };
            key(a).cmp(&key(b))
        });
        (all, invalid)
    }

    pub fn list(&self, projects: &[ProjectRef]) -> SavedWorkflowList {
        let (all, invalid) = self.collect(projects);
        SavedWorkflowList {
            workflows: all.into_iter().map(|l| l.summary).collect(),
            invalid,
            global_dir: Some(self.global_dir.to_string_lossy().into_owned()),
        }
    }

    /// Find one workflow. With a `scope`, only there; without, the highest
    /// scope that has the name (project, then global, then built-in).
    pub fn resolve(
        &self,
        name: &str,
        scope: Option<SavedScope>,
        project: Option<&ProjectRef>,
    ) -> Result<Loaded, SavedError> {
        valid_saved_name(name).map_err(SavedError::Invalid)?;
        let (all, invalid) = self.collect(project.map(std::slice::from_ref).unwrap_or(&[]));
        // `collect` is ordered project, global, built-in: the first match wins.
        if let Some(found) = all
            .iter()
            .find(|l| l.summary.name == name && scope.is_none_or(|s| s == l.summary.scope))
        {
            return Ok(found.clone());
        }
        // A file that exists but is unusable deserves its own message.
        if let Some(bad) = invalid.iter().find(|i| {
            Path::new(&i.path).file_stem().and_then(|s| s.to_str()) == Some(name)
                && scope.is_none_or(|s| s == i.scope)
        }) {
            return Err(SavedError::Invalid(format!(
                "the {} workflow {name:?} cannot be used: {}",
                bad.scope, bad.reason
            )));
        }
        let names: Vec<String> = all.iter().map(|l| l.summary.name.clone()).collect();
        Err(SavedError::NotFound(match scope {
            Some(s) => format!("no {s} workflow named {name:?}"),
            None => format!(
                "no saved workflow named {name:?}{}",
                if names.is_empty() {
                    String::new()
                } else {
                    format!(" (available: {})", names.join(", "))
                }
            ),
        }))
    }

    // ── writing ────────────────────────────────────────────────────────────

    /// Write `meta` + `script` as `<name>.star` in `scope`. The file is
    /// analysed first (a broken script is never saved), written through a temp
    /// file and renamed into place. Replacing an existing file needs
    /// `overwrite`.
    pub fn write(
        &self,
        scope: SavedScope,
        project: Option<&Path>,
        meta: &SavedMeta,
        script: &str,
        overwrite: bool,
    ) -> Result<Written, SavedError> {
        let text = saved::render(meta, script).map_err(SavedError::Invalid)?;
        let name = meta.name.as_deref().unwrap_or_default();
        zeron_workflow::analyze(&format!("{name}.star"), &text).map_err(SavedError::Diagnostics)?;
        let dir = self.prepare_dir(scope, project)?;
        let target = dir.join(format!("{name}.{EXT}"));
        debug_assert_eq!(target.parent(), Some(dir.as_path()));
        let existing = std::fs::symlink_metadata(&target).ok();
        if let Some(m) = &existing {
            if m.file_type().is_symlink() {
                return Err(SavedError::Invalid(format!(
                    "{} is a symbolic link; remove it first",
                    target.display()
                )));
            }
            if !m.is_file() {
                return Err(SavedError::Invalid(format!(
                    "{} exists and is not a file",
                    target.display()
                )));
            }
            if !overwrite {
                return Err(SavedError::Exists {
                    name: name.to_owned(),
                    scope,
                });
            }
        }
        let io = |e: std::io::Error| {
            SavedError::Io(format!("could not write {}: {e}", target.display()))
        };
        let mut tmp = tempfile::Builder::new()
            .prefix(".saving-")
            .suffix(".tmp")
            .tempfile_in(&dir)
            .map_err(io)?;
        tmp.write_all(text.as_bytes()).map_err(io)?;
        tmp.as_file().sync_all().map_err(io)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // A shareable file, not the temp file's private 0600.
            let _ = std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o644));
        }
        if overwrite {
            tmp.persist(&target).map_err(|e| io(e.error))?;
        } else {
            // Atomic "create only": a file that appeared since the check wins.
            tmp.persist_noclobber(&target).map_err(|e| {
                if e.error.kind() == std::io::ErrorKind::AlreadyExists {
                    SavedError::Exists {
                        name: name.to_owned(),
                        scope,
                    }
                } else {
                    io(e.error)
                }
            })?;
        }
        Ok(Written {
            path: target,
            overwrote: existing.is_some(),
            text,
        })
    }

    /// The folder for `scope`, created if need be. For a project: neither
    /// `.zeron` nor `workflows` may be a symlink, and the result must lie
    /// inside the project.
    fn prepare_dir(
        &self,
        scope: SavedScope,
        project: Option<&Path>,
    ) -> Result<PathBuf, SavedError> {
        let dir = self.dir_for(scope, project)?;
        let io =
            |e: std::io::Error| SavedError::Io(format!("could not create {}: {e}", dir.display()));
        if scope == SavedScope::Project {
            let root = project.ok_or_else(|| SavedError::Invalid("no project".into()))?;
            for part in [root.join(".zeron"), dir.clone()] {
                if let Ok(m) = std::fs::symlink_metadata(&part)
                    && (m.file_type().is_symlink() || !m.is_dir())
                {
                    return Err(SavedError::Invalid(format!(
                        "{} must be a real folder inside the project, not a link or file",
                        part.display()
                    )));
                }
            }
            std::fs::create_dir_all(&dir).map_err(io)?;
            let canon = dir.canonicalize().map_err(io)?;
            let root = root.canonicalize().map_err(io)?;
            if !canon.starts_with(&root) {
                return Err(SavedError::Invalid(
                    "the workflows folder resolves outside the project".into(),
                ));
            }
            Ok(canon)
        } else {
            std::fs::create_dir_all(&dir).map_err(io)?;
            Ok(dir)
        }
    }

    /// Remove a saved workflow's file. Built-ins cannot be deleted.
    pub fn delete(
        &self,
        name: &str,
        scope: SavedScope,
        project: Option<&ProjectRef>,
    ) -> Result<PathBuf, SavedError> {
        valid_saved_name(name).map_err(SavedError::Invalid)?;
        let dir = self.dir_for(scope, project.map(|p| p.root.as_path()))?;
        let target = dir.join(format!("{name}.{EXT}"));
        match std::fs::symlink_metadata(&target) {
            Ok(m) if m.is_file() || (scope == SavedScope::Global && m.file_type().is_symlink()) => {
            }
            Ok(_) => {
                return Err(SavedError::Invalid(format!(
                    "{} is not a regular file; delete it by hand",
                    target.display()
                )));
            }
            Err(_) => {
                return Err(SavedError::NotFound(format!(
                    "no {scope} workflow named {name:?}"
                )));
            }
        }
        std::fs::remove_file(&target)
            .map_err(|e| SavedError::Io(format!("could not delete {}: {e}", target.display())))?;
        Ok(target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn script() -> &'static str {
        "def main(args):\n    phase(\"p\")\n    return agent(\"a\").ask(\"hi\").result().value\n"
    }

    fn meta(name: &str) -> SavedMeta {
        SavedMeta {
            name: Some(name.into()),
            description: format!("{name} does a thing"),
            when_to_use: None,
            args: vec![SavedArg {
                name: "base".into(),
                ty: SavedArgType::String,
                required: false,
                default: Some(json!("main")),
                description: None,
            }],
        }
    }

    struct Fx {
        _tmp: tempfile::TempDir,
        store: SavedStore,
        root: PathBuf,
        global: PathBuf,
    }

    fn fx() -> Fx {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("project");
        let global = tmp.path().join("home").join("workflows");
        std::fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        Fx {
            store: SavedStore::new(global.clone()),
            _tmp: tmp,
            root,
            global,
        }
    }

    fn project(fx: &Fx) -> ProjectRef {
        ProjectRef {
            root: fx.root.clone(),
            space_id: Some("space-1".into()),
        }
    }

    #[test]
    fn save_then_list_and_resolve_round_trip() {
        let fx = fx();
        let w = fx
            .store
            .write(
                SavedScope::Project,
                Some(&fx.root),
                &meta("mine"),
                script(),
                false,
            )
            .unwrap();
        assert_eq!(w.path, fx.root.join(".zeron/workflows/mine.star"));
        assert!(!w.overwrote);
        let list = fx.store.list(&[project(&fx)]);
        let mine = list.workflows.iter().find(|w| w.name == "mine").unwrap();
        assert_eq!(mine.scope, SavedScope::Project);
        assert_eq!(mine.args[0].default, Some(json!("main")));
        assert_eq!(mine.space_id.as_deref(), Some("space-1"));
        assert!(mine.modified_at.is_some());
        let loaded = fx.store.resolve("mine", None, Some(&project(&fx))).unwrap();
        assert!(loaded.text.contains("def main(args)"));
        assert!(loaded.text.starts_with("# zeron-workflow\n# name: mine\n"));
        // Built-ins are always there.
        assert!(
            list.workflows
                .iter()
                .any(|w| w.scope == SavedScope::Builtin && w.name == "pr-review")
        );
        assert!(list.invalid.is_empty(), "{:?}", list.invalid);
    }

    #[test]
    fn project_shadows_global_shadows_builtin() {
        let fx = fx();
        fx.store
            .write(
                SavedScope::Global,
                None,
                &meta("pr-review"),
                script(),
                false,
            )
            .unwrap();
        fx.store
            .write(
                SavedScope::Project,
                Some(&fx.root),
                &meta("pr-review"),
                script(),
                false,
            )
            .unwrap();
        let list = fx.store.list(&[project(&fx)]);
        let by_scope = |s| {
            list.workflows
                .iter()
                .find(|w| w.name == "pr-review" && w.scope == s)
                .unwrap()
        };
        assert_eq!(by_scope(SavedScope::Project).shadowed_by, None);
        assert_eq!(
            by_scope(SavedScope::Project).shadows,
            [SavedScope::Global, SavedScope::Builtin]
        );
        assert_eq!(
            by_scope(SavedScope::Global).shadowed_by,
            Some(SavedScope::Project)
        );
        assert_eq!(by_scope(SavedScope::Global).shadows, [SavedScope::Builtin]);
        assert_eq!(
            by_scope(SavedScope::Builtin).shadowed_by,
            Some(SavedScope::Project)
        );
        // Resolution picks the winner, or the scope asked for.
        let p = project(&fx);
        assert_eq!(
            fx.store
                .resolve("pr-review", None, Some(&p))
                .unwrap()
                .summary
                .scope,
            SavedScope::Project
        );
        assert_eq!(
            fx.store
                .resolve("pr-review", Some(SavedScope::Global), Some(&p))
                .unwrap()
                .summary
                .scope,
            SavedScope::Global
        );
        // Without the project the global one wins.
        assert_eq!(
            fx.store
                .resolve("pr-review", None, None)
                .unwrap()
                .summary
                .scope,
            SavedScope::Global
        );
        // Deleting the project's file uncovers the next one.
        fx.store
            .delete("pr-review", SavedScope::Project, Some(&p))
            .unwrap();
        assert_eq!(
            fx.store
                .resolve("pr-review", None, Some(&p))
                .unwrap()
                .summary
                .scope,
            SavedScope::Global
        );
    }

    #[test]
    fn a_listing_of_all_projects_does_not_claim_a_single_winner() {
        let fx = fx();
        fx.store
            .write(SavedScope::Global, None, &meta("shared"), script(), false)
            .unwrap();
        let other = fx.root.parent().unwrap().join("other");
        std::fs::create_dir_all(&other).unwrap();
        let other = other.canonicalize().unwrap();
        fx.store
            .write(
                SavedScope::Project,
                Some(&other),
                &meta("shared"),
                script(),
                false,
            )
            .unwrap();
        let list = fx.store.list(&[
            project(&fx),
            ProjectRef {
                root: other,
                space_id: None,
            },
        ]);
        let g = list
            .workflows
            .iter()
            .find(|w| w.scope == SavedScope::Global && w.name == "shared")
            .unwrap();
        assert_eq!(
            g.shadowed_by, None,
            "shadowing is per project; the UI derives it"
        );
        let p = list
            .workflows
            .iter()
            .find(|w| w.scope == SavedScope::Project && w.name == "shared")
            .unwrap();
        assert_eq!(p.shadows, [SavedScope::Global]);
    }

    #[test]
    fn edits_on_disk_show_up_on_the_next_list() {
        let fx = fx();
        let dir = fx.root.join(".zeron/workflows");
        std::fs::create_dir_all(&dir).unwrap();
        assert!(
            fx.store
                .list(&[project(&fx)])
                .workflows
                .iter()
                .all(|w| w.scope == SavedScope::Builtin)
        );
        std::fs::write(
            dir.join("by-hand.star"),
            "# zeron-workflow\n# description: written in an editor\n\ndef main(args):\n    phase(\"p\")\n    agent(\"a\").ask(\"x\").result()\n",
        )
        .unwrap();
        let list = fx.store.list(&[project(&fx)]);
        assert!(
            list.workflows
                .iter()
                .any(|w| w.name == "by-hand" && w.description == "written in an editor")
        );
        std::fs::write(
            dir.join("by-hand.star"),
            "# zeron-workflow\n# description: edited\n",
        )
        .unwrap();
        let list = fx.store.list(&[project(&fx)]);
        assert!(
            list.workflows
                .iter()
                .any(|w| w.name == "by-hand" && w.description == "edited")
        );
        std::fs::remove_file(dir.join("by-hand.star")).unwrap();
        assert!(
            !fx.store
                .list(&[project(&fx)])
                .workflows
                .iter()
                .any(|w| w.name == "by-hand")
        );
    }

    #[test]
    fn bad_files_are_reported_and_never_fail_the_list() {
        let fx = fx();
        let dir = fx.root.join(".zeron/workflows");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("good.star"),
            "# zeron-workflow\n# description: ok\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("broken.star"),
            "# zeron-workflow\n# bogus: 1\n# description: d\n",
        )
        .unwrap();
        std::fs::write(dir.join("no-mark.star"), "def main(args): pass\n").unwrap();
        std::fs::write(
            dir.join("Bad Name.star"),
            "# zeron-workflow\n# description: d\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("mismatch.star"),
            "# zeron-workflow\n# name: other\n# description: d\n",
        )
        .unwrap();
        std::fs::write(dir.join("notes.txt"), "ignored").unwrap();
        std::fs::write(dir.join("binary.star"), [0xff, 0xfe, 0x00, 0x01]).unwrap();
        std::fs::write(dir.join("huge.star"), vec![b'#'; MAX_SCRIPT_BYTES + 10]).unwrap();
        std::fs::create_dir(dir.join("adir.star")).unwrap();
        let list = fx.store.list(&[project(&fx)]);
        let names: Vec<_> = list
            .workflows
            .iter()
            .filter(|w| w.scope == SavedScope::Project)
            .map(|w| w.name.as_str())
            .collect();
        assert_eq!(names, ["good"]);
        let reasons: Vec<(String, String)> = list
            .invalid
            .iter()
            .map(|i| {
                (
                    Path::new(&i.path)
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                    i.reason.clone(),
                )
            })
            .collect();
        let reason = |file: &str| {
            reasons
                .iter()
                .find(|(f, _)| f == file)
                .map(|(_, r)| r.clone())
                .unwrap_or_else(|| panic!("{file}: {reasons:?}"))
        };
        assert!(reason("broken.star").contains("unknown key `bogus`"));
        assert!(reason("no-mark.star").contains("starts with the line `# zeron-workflow`"));
        assert!(reason("Bad Name.star").contains("lowercase"));
        assert!(reason("mismatch.star").contains("does not match the file name"));
        assert!(reason("binary.star").contains("could not read"));
        assert!(reason("huge.star").contains("larger than 256 KB"));
        assert!(reason("adir.star").contains("not a regular file"));
        assert!(!reasons.iter().any(|(f, _)| f == "notes.txt"));
        // Asking for a broken one by name explains why, instead of "not found".
        let err = fx
            .store
            .resolve("broken", None, Some(&project(&fx)))
            .unwrap_err();
        assert!(
            err.to_string().contains("cannot be used") && err.to_string().contains("bogus"),
            "{err}"
        );
    }

    #[test]
    fn only_the_first_files_of_a_huge_folder_are_read() {
        let fx = fx();
        let dir = fx.root.join(".zeron/workflows");
        std::fs::create_dir_all(&dir).unwrap();
        for i in 0..(MAX_FILES + 20) {
            std::fs::write(
                dir.join(format!("w{i:04}.star")),
                "# zeron-workflow\n# description: d\n",
            )
            .unwrap();
        }
        let list = fx.store.list(&[project(&fx)]);
        let n = list
            .workflows
            .iter()
            .filter(|w| w.scope == SavedScope::Project)
            .count();
        assert_eq!(n, MAX_FILES);
        assert!(
            list.invalid
                .iter()
                .any(|i| i.reason.contains("only the first 200"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_in_a_project_are_never_followed() {
        use std::os::unix::fs::symlink;
        let fx = fx();
        let outside = fx.root.parent().unwrap().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(
            outside.join("evil.star"),
            "# zeron-workflow\n# description: planted\n",
        )
        .unwrap();
        // A file symlink inside a real folder.
        let dir = fx.root.join(".zeron/workflows");
        std::fs::create_dir_all(&dir).unwrap();
        symlink(outside.join("evil.star"), dir.join("evil.star")).unwrap();
        let list = fx.store.list(&[project(&fx)]);
        assert!(!list.workflows.iter().any(|w| w.name == "evil"));
        assert!(
            list.invalid
                .iter()
                .any(|i| i.reason.contains("not a regular file"))
        );
        // Writing over it is refused too.
        let err = fx
            .store
            .write(
                SavedScope::Project,
                Some(&fx.root),
                &meta("evil"),
                script(),
                true,
            )
            .unwrap_err();
        assert!(err.to_string().contains("symbolic link"), "{err}");
        assert_eq!(
            std::fs::read_to_string(outside.join("evil.star")).unwrap(),
            "# zeron-workflow\n# description: planted\n"
        );
        // The folder itself as a link out of the project.
        std::fs::remove_dir_all(fx.root.join(".zeron")).unwrap();
        std::fs::create_dir_all(fx.root.join(".zeron")).unwrap();
        symlink(&outside, fx.root.join(".zeron/workflows")).unwrap();
        let list = fx.store.list(&[project(&fx)]);
        assert!(!list.workflows.iter().any(|w| w.name == "evil"));
        assert!(
            list.invalid
                .iter()
                .any(|i| i.reason.contains("symbolic link"))
        );
        let err = fx
            .store
            .write(
                SavedScope::Project,
                Some(&fx.root),
                &meta("fresh"),
                script(),
                false,
            )
            .unwrap_err();
        assert!(err.to_string().contains("not a link or file"), "{err}");
        assert!(
            !outside.join("fresh.star").exists(),
            "nothing was written outside the project"
        );
        // `.zeron` itself linked out.
        std::fs::remove_dir_all(fx.root.join(".zeron")).unwrap();
        symlink(&outside, fx.root.join(".zeron")).unwrap();
        assert!(
            fx.store
                .write(
                    SavedScope::Project,
                    Some(&fx.root),
                    &meta("fresh"),
                    script(),
                    false
                )
                .is_err()
        );
        assert!(!outside.join("workflows").exists());
        // Deleting through a link is refused.
        assert!(
            fx.store
                .delete("evil", SavedScope::Project, Some(&project(&fx)))
                .is_err()
        );
        assert!(outside.join("evil.star").exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_linked_global_folder_is_the_users_own_choice() {
        use std::os::unix::fs::symlink;
        let fx = fx();
        let dotfiles = fx.root.parent().unwrap().join("dotfiles");
        std::fs::create_dir_all(&dotfiles).unwrap();
        std::fs::create_dir_all(fx.global.parent().unwrap()).unwrap();
        symlink(&dotfiles, &fx.global).unwrap();
        fx.store
            .write(SavedScope::Global, None, &meta("mine"), script(), false)
            .unwrap();
        assert!(dotfiles.join("mine.star").exists());
        assert!(
            fx.store
                .list(&[])
                .workflows
                .iter()
                .any(|w| w.name == "mine")
        );
    }

    #[test]
    fn overwriting_needs_the_flag_and_the_old_file_survives_a_refusal() {
        let fx = fx();
        fx.store
            .write(SavedScope::Global, None, &meta("keep"), script(), false)
            .unwrap();
        let path = fx.global.join("keep.star");
        let before = std::fs::read_to_string(&path).unwrap();
        let mut changed = meta("keep");
        changed.description = "different".into();
        let err = fx
            .store
            .write(SavedScope::Global, None, &changed, script(), false)
            .unwrap_err();
        assert!(matches!(err, SavedError::Exists { .. }), "{err}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
        let w = fx
            .store
            .write(SavedScope::Global, None, &changed, script(), true)
            .unwrap();
        assert!(w.overwrote);
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("# description: different")
        );
        // No temp files are left behind.
        let leftovers: Vec<_> = std::fs::read_dir(&fx.global)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n != "keep.star")
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    #[test]
    fn a_broken_script_is_never_saved() {
        let fx = fx();
        let err = fx
            .store
            .write(
                SavedScope::Global,
                None,
                &meta("broken"),
                "def main(args):\n    while True:\n        pass\n",
                false,
            )
            .unwrap_err();
        assert!(matches!(err, SavedError::Diagnostics(_)), "{err}");
        assert!(!fx.global.join("broken.star").exists());
    }

    #[test]
    fn names_cannot_escape_the_workflows_folder() {
        let fx = fx();
        for bad in [
            "../evil",
            "..",
            "a/b",
            "a\\b",
            "/abs/path",
            "",
            "UPPER",
            "x.star",
            "con",
        ] {
            let mut m = meta("x");
            m.name = Some(bad.into());
            assert!(
                fx.store
                    .write(SavedScope::Global, None, &m, script(), true)
                    .is_err(),
                "{bad:?}"
            );
            assert!(fx.store.resolve(bad, None, None).is_err(), "{bad:?}");
            assert!(
                fx.store.delete(bad, SavedScope::Global, None).is_err(),
                "{bad:?}"
            );
        }
        // Nothing appeared next to or above the folder.
        assert!(!fx.global.parent().unwrap().join("evil.star").exists());
        assert!(!fx.global.exists() || std::fs::read_dir(&fx.global).unwrap().count() == 0);
    }

    #[test]
    fn builtins_are_read_only() {
        let fx = fx();
        let mut m = meta("pr-review");
        m.name = Some("pr-review".into());
        assert!(
            fx.store
                .write(SavedScope::Builtin, None, &m, script(), true)
                .is_err()
        );
        assert!(
            fx.store
                .delete("pr-review", SavedScope::Builtin, None)
                .is_err()
        );
        assert!(
            fx.store
                .resolve("pr-review", Some(SavedScope::Builtin), None)
                .is_ok()
        );
    }

    #[test]
    fn missing_names_list_what_exists() {
        let fx = fx();
        let err = fx.store.resolve("nope", None, None).unwrap_err();
        assert!(matches!(err, SavedError::NotFound(_)));
        assert!(
            err.to_string().contains("available:") && err.to_string().contains("pr-review"),
            "{err}"
        );
        let err = fx
            .store
            .resolve("nope", Some(SavedScope::Project), None)
            .unwrap_err();
        assert_eq!(err.to_string(), "no project workflow named \"nope\"");
    }

    #[test]
    fn delete_removes_exactly_the_one_file() {
        let fx = fx();
        fx.store
            .write(SavedScope::Global, None, &meta("a"), script(), false)
            .unwrap();
        fx.store
            .write(SavedScope::Global, None, &meta("b"), script(), false)
            .unwrap();
        fx.store.delete("a", SavedScope::Global, None).unwrap();
        assert!(!fx.global.join("a.star").exists() && fx.global.join("b.star").exists());
        assert!(matches!(
            fx.store.delete("a", SavedScope::Global, None),
            Err(SavedError::NotFound(_))
        ));
    }

    #[test]
    fn the_global_dir_honours_the_environment_override() {
        // Only the pure part: the override is read at call time.
        let key = "ZERON_WORKFLOWS_DIR";
        let prior = std::env::var_os(key);
        // SAFETY: tests in this crate that touch this variable run in this one test.
        unsafe { std::env::set_var(key, "/tmp/zeron-wf-override") };
        assert_eq!(
            default_global_dir(),
            PathBuf::from("/tmp/zeron-wf-override")
        );
        unsafe { std::env::set_var(key, "") };
        assert!(
            default_global_dir().ends_with(".zeron/workflows")
                || default_global_dir().ends_with(".zeron\\workflows")
        );
        match prior {
            Some(v) => unsafe { std::env::set_var(key, v) },
            None => unsafe { std::env::remove_var(key) },
        }
    }
}
