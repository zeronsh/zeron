//! Project worktrees: one worktree of a whole project whose folder holds
//! several git repositories (and isn't one itself).
//!
//! A project folder like
//!
//! ```text
//! acme/            (no .git)
//!   AGENTS.md
//!   api/           (a repository)
//!   web/           (a repository)
//!   packages/      (a plain folder)
//!     ui/          (a repository)
//!     README.md
//! ```
//!
//! gets, for a chat in a new worktree of it, a folder laid out the same way
//! under the worktree location (`{root}/acme/<name>/`): a worktree of each
//! repository chosen at its own path, all on one `zeron/<name>` branch, each
//! off its own current commit (or the run's base ref, where that repository
//! has a branch of that name). Everything else of the project — its own
//! files and plain folders, such as `AGENTS.md` above — is a link to the
//! original, so the agent reads the project as it is; a repository not
//! chosen is left out, never linked (the agent would change the original).
//!
//! Which repositories: the project folder's own subfolders, and one level
//! further inside a plain subfolder (`packages/ui`). Never dot folders,
//! `node_modules`, linked worktrees or submodules (each belongs to another
//! repository), nor a link (it may lead out of the project).
//!
//! Nothing records a project worktree but the folder itself: its linked
//! worktrees say which repositories it holds (their `.git` files), so a
//! rename after the chat's title or a removal works from the folder alone,
//! and survives a restart midway.

use super::*;
use zeron_proto::{ProjectRepositories, ProjectRepository};

/// Repositories a listing names at most (a folder of many more is a
/// workspace of clones, not a project to check out whole).
pub(crate) const PROJECT_REPOSITORIES_MAX: usize = 64;
/// How long looking through a project folder may take: it can be on a
/// dead network mount, where a bare `readdir` hangs for minutes.
const LISTING_TIMEOUT: Duration = Duration::from_secs(6);
/// Entries looked at in any one folder (a folder of a million files is
/// read no further than this).
const ENTRIES_MAX: usize = 4_000;
/// Repositories asked about (branch, commits) at once.
const GIT_CONCURRENCY: usize = 8;

/// A project worktree as it was made: its folder (where the chat runs) and
/// shared branch, as one [`Worktree`] whose `repo_path` is the project's
/// folder, and each repository's own worktree in it.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectWorktree {
    pub worktree: Worktree,
    pub repositories: Vec<Worktree>,
}

/// What a folder's `.git` says it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Marker {
    /// No `.git`: a plain folder.
    None,
    /// A repository of its own (a `.git` folder, or a `gitdir:` file for a
    /// repository whose git dir lives elsewhere).
    Repository,
    /// A linked worktree (`gitdir: …/worktrees/<name>`).
    Linked,
    /// A submodule (`gitdir: …/modules/<name>`): its parent repository's.
    Submodule,
}

fn marker(dir: &Path) -> Marker {
    let dot = dir.join(".git");
    let Ok(meta) = std::fs::symlink_metadata(&dot) else {
        return Marker::None;
    };
    if meta.is_dir() {
        return Marker::Repository;
    }
    if !meta.is_file() || meta.len() > 4096 {
        return Marker::None;
    }
    let Ok(text) = std::fs::read_to_string(&dot) else {
        return Marker::None;
    };
    let Some(gitdir) = text
        .lines()
        .next()
        .and_then(|line| line.strip_prefix("gitdir:"))
        .map(|dir| dir.trim().replace('\\', "/"))
    else {
        return Marker::None;
    };
    if gitdir.contains("/worktrees/") {
        Marker::Linked
    } else if gitdir.contains("/modules/") {
        Marker::Submodule
    } else {
        Marker::Repository
    }
}

/// A repository (or, in a project worktree, a linked worktree) found in a
/// folder: its `/`-separated path there.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Found {
    path: String,
    linked: bool,
}

/// The repositories in `root` (see the module docs for where it looks),
/// sorted by path, and whether there were more than
/// [`PROJECT_REPOSITORIES_MAX`]. `linked`: linked worktrees too (a project
/// worktree's own).
fn discover(root: &Path, linked: bool) -> (Vec<Found>, bool) {
    let mut found = Vec::new();
    let truncated = walk(root, "", 0, linked, &mut found);
    found.sort_by(|a, b| a.path.cmp(&b.path));
    (found, truncated)
}

/// One folder's subfolders, by name: never a link, a dot folder or
/// `node_modules`, nor a name that isn't UTF-8 (a repository's path rides
/// JSON).
fn subfolders(dir: &Path) -> Vec<String> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = read
        .take(ENTRIES_MAX)
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| !name.starts_with('.') && name != "node_modules")
        .collect();
    names.sort();
    names
}

/// True when more than the cap were found (the walk stops there).
fn walk(dir: &Path, prefix: &str, depth: usize, linked: bool, found: &mut Vec<Found>) -> bool {
    for name in subfolders(dir) {
        let path = format!("{prefix}{name}");
        let at = dir.join(&name);
        let kind = marker(&at);
        let take = match kind {
            Marker::Repository => Some(false),
            Marker::Linked if linked => Some(true),
            _ => None,
        };
        if let Some(is_linked) = take {
            if found.len() == PROJECT_REPOSITORIES_MAX {
                return true;
            }
            found.push(Found {
                path,
                linked: is_linked,
            });
        } else if kind == Marker::None
            && depth == 0
            && walk(&at, &format!("{path}/"), 1, linked, found)
        {
            return true;
        }
    }
    false
}

/// The repositories (and linked worktrees) in `folder`, as [`discover`]
/// finds them: each one's `/`-separated path there and whether it's a
/// linked worktree. Blocking: the caller's thread reads the folder.
pub fn repositories_in(folder: &Path) -> Vec<(String, bool)> {
    discover(folder, true)
        .0
        .into_iter()
        .map(|found| (found.path, found.linked))
        .collect()
}

/// Whether `relative` names a place inside a folder: `/`-separated plain
/// names, none of them `.`, `..` or empty, and no root or drive.
fn is_inside(relative: &str) -> bool {
    !relative.is_empty()
        && !relative.contains('\\')
        && !relative.contains(':')
        && relative
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

/// Whether `path` is a project worktree: a folder with no `.git` of its own
/// holding at least one linked worktree where [`discover`] looks. Only the
/// filesystem is read, on a disposable thread (dead mounts).
pub(crate) async fn is_project_worktree_folder(path: &Path) -> bool {
    let path = path.to_path_buf();
    let probe = disposable_worker("project-worktree-probe", move || {
        path.is_dir()
            && std::fs::symlink_metadata(path.join(".git")).is_err()
            && discover(&path, true).0.iter().any(|found| found.linked)
    });
    matches!(
        tokio::time::timeout(LISTING_TIMEOUT, probe).await,
        Ok(Some(true))
    )
}

/// What [`lay_out`] made of the project: how many links (for the log).
#[derive(Debug, Default)]
struct Layout {
    links: usize,
    failed: usize,
}

/// Fill the project worktree `into` with links to `project`'s own entries,
/// leaving room for the repositories `chosen` (their worktrees go there) and
/// leaving out the ones not chosen (`all` holds every repository's path). A
/// folder holding a repository is made, not linked, and its own entries
/// linked in turn. Best effort: a link that can't be made (Windows without
/// the right to make one) is counted and passed over.
fn lay_out(project: &Path, into: &Path, chosen: &HashSet<String>, all: &HashSet<String>) -> Layout {
    let mut layout = Layout::default();
    lay_out_level(project, into, "", chosen, all, &mut layout);
    layout
}

fn lay_out_level(
    from: &Path,
    into: &Path,
    prefix: &str,
    chosen: &HashSet<String>,
    all: &HashSet<String>,
    layout: &mut Layout,
) {
    let Ok(read) = std::fs::read_dir(from) else {
        return;
    };
    for entry in read.take(ENTRIES_MAX).filter_map(Result::ok) {
        let name = entry.file_name();
        let Some(text) = name.to_str() else {
            continue;
        };
        let path = format!("{prefix}{text}");
        // A repository: its worktree's place, or (not chosen) left out.
        if all.contains(&path) || chosen.contains(&path) {
            continue;
        }
        let inner = format!("{path}/");
        let holds_repository = all.iter().any(|repo| repo.starts_with(&inner));
        let is_dir = entry.file_type().is_ok_and(|kind| kind.is_dir());
        if holds_repository && is_dir {
            let folder = into.join(&name);
            if std::fs::create_dir(&folder).is_ok() {
                lay_out_level(&entry.path(), &folder, &inner, chosen, all, layout);
            } else {
                layout.failed += 1;
            }
            continue;
        }
        if link(&entry.path(), &into.join(&name), is_dir).is_ok() {
            layout.links += 1;
        } else {
            layout.failed += 1;
        }
    }
}

#[cfg(unix)]
fn link(original: &Path, at: &Path, _is_dir: bool) -> std::io::Result<()> {
    std::os::unix::fs::symlink(original, at)
}

#[cfg(windows)]
fn link(original: &Path, at: &Path, is_dir: bool) -> std::io::Result<()> {
    if is_dir {
        std::os::windows::fs::symlink_dir(original, at)
    } else {
        std::os::windows::fs::symlink_file(original, at)
    }
}

#[cfg(not(any(unix, windows)))]
fn link(_original: &Path, _at: &Path, _is_dir: bool) -> std::io::Result<()> {
    Err(std::io::Error::other("links are not supported here"))
}

impl Repos {
    /// `ListProjectRepositories`: the repositories in the project folder at
    /// `path` — what a worktree of the whole project can check out — each
    /// with its current branch and whether it has a commit to branch from.
    /// A folder inside a repository says so (`is_repo`) and lists none: its
    /// worktrees are that repository's. In a project worktree, its linked
    /// worktrees (`linked`).
    pub async fn project_repositories(
        &self,
        path: &Path,
    ) -> Result<ProjectRepositories, EngineError> {
        if !Self::path_exists(path).await {
            return Err(EngineError::Other(format!(
                "{} is not on this device",
                path.display()
            )));
        }
        if self.is_repo(path).await {
            return Ok(ProjectRepositories {
                is_repo: true,
                ..Default::default()
            });
        }
        let (found, truncated) = self.discover(path, true).await?;
        let repositories = stream::iter(found)
            .map(|found| async move {
                let at = path.join(&found.path);
                let (branch, head) = tokio::join!(self.current_branch(&at), self.head_sha(&at));
                ProjectRepository {
                    path: found.path,
                    branch: branch.ok().filter(|name| name != "HEAD"),
                    has_commits: matches!(head, Ok(Some(_))),
                    linked: found.linked,
                }
            })
            .buffered(GIT_CONCURRENCY)
            .collect()
            .await;
        Ok(ProjectRepositories {
            is_repo: false,
            repositories,
            truncated,
        })
    }

    /// [`discover`] on a disposable thread, under [`LISTING_TIMEOUT`].
    async fn discover(&self, path: &Path, linked: bool) -> Result<(Vec<Found>, bool), EngineError> {
        let root = path.to_path_buf();
        let walk = disposable_worker("project-repositories", move || discover(&root, linked));
        match tokio::time::timeout(LISTING_TIMEOUT, walk).await {
            Ok(Some(found)) => Ok(found),
            Ok(None) => Err(EngineError::Other(
                "Couldn't look through the project's folder".into(),
            )),
            Err(_) => Err(EngineError::Other(format!(
                "{} took too long to read",
                path.display()
            ))),
        }
    }

    /// A worktree of the project at `project` (a folder holding several
    /// repositories), checking out the repositories at `repositories` (paths
    /// in it): a folder laid out like the project under the worktree
    /// location, a worktree of each on one fresh `zeron/<name>` branch off
    /// `base` where that repository has a branch so named, else off its
    /// current commit. All or nothing: a repository that can't be checked
    /// out undoes the others and the folder, and the error names it.
    pub async fn create_project_worktree(
        &self,
        project: &Path,
        repositories: &[String],
        base: &str,
    ) -> Result<ProjectWorktree, EngineError> {
        // The repositories asked for, each once, as the host finds them now.
        let mut chosen: Vec<String> = Vec::new();
        for path in repositories {
            let path = path.trim_matches('/').to_string();
            if !is_inside(&path) {
                return Err(EngineError::Other(format!(
                    "{path:?} is not a folder in the project"
                )));
            }
            if !chosen.contains(&path) {
                chosen.push(path);
            }
        }
        if chosen.is_empty() {
            return Err(EngineError::Other(
                "Choose at least one repository for the worktree".into(),
            ));
        }
        if self.is_repo(project).await {
            return Err(EngineError::Other(format!(
                "{} is a repository: its worktree checks out the one repository",
                project.display()
            )));
        }
        let (found, _) = self.discover(project, false).await?;
        let all: HashSet<String> = found.iter().map(|found| found.path.clone()).collect();
        for path in &chosen {
            if !all.contains(path) {
                return Err(EngineError::Other(format!(
                    "{path} is no longer a repository in {}",
                    project.display()
                )));
            }
            if self.head_sha(&project.join(path)).await?.is_none() {
                return Err(EngineError::Other(format!(
                    "{path} has no commit yet, so there is nothing to branch from"
                )));
            }
        }
        chosen.sort();

        let project_name = project
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "project".to_string());
        let root = self.inner.worktrees.root();
        let base_dir = root.join(&project_name);
        // A location inside the project would put its worktrees among the
        // project's own folders (and the agent's view of them).
        if let (Ok(canonical_root), Ok(canonical_project)) =
            (std::fs::canonicalize(&root), std::fs::canonicalize(project))
            && canonical_root
                .join(&project_name)
                .starts_with(&canonical_project)
        {
            return Err(EngineError::Other(format!(
                "Worktrees for {project_name} would be created inside the project at {}. Choose a different worktree location in Settings.",
                project.display()
            )));
        }
        std::fs::create_dir_all(&base_dir)?;

        // A name no repository has a branch for, its folder claimed by
        // making it (two chats starting at once never share one).
        let mut taken: HashSet<String> = HashSet::new();
        for path in &chosen {
            taken.extend(self.branches(&project.join(path)).await.unwrap_or_default());
        }
        let mut claimed = None;
        for attempt in 0..50u64 {
            let name = generated_worktree_name(attempt);
            if taken.contains(&format!("zeron/{name}")) {
                continue;
            }
            let folder = base_dir.join(&name);
            match std::fs::create_dir(&folder) {
                Ok(()) => {
                    claimed = Some((name, folder));
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        }
        let (name, folder) = claimed
            .ok_or_else(|| EngineError::Other("Could not allocate a worktree name".into()))?;
        let branch = format!("zeron/{name}");

        let made = self
            .fill_project_worktree(project, &folder, &chosen, &all, &branch, base)
            .await;
        let repositories = match made {
            Ok(repositories) => repositories,
            Err((error, done)) => {
                self.undo_project_worktree(&folder, &done, &branch).await;
                return Err(error);
            }
        };
        tracing::info!(
            project = %project.display(),
            path = %folder.display(),
            branch = %branch,
            repositories = repositories.len(),
            "project worktree made"
        );
        Ok(ProjectWorktree {
            worktree: Worktree {
                repo_path: project.to_string_lossy().into_owned(),
                path: folder.to_string_lossy().into_owned(),
                branch,
                name,
                checkout_id: None,
            },
            repositories,
        })
    }

    /// The links and worktrees of a project worktree just claimed at
    /// `folder`. On failure, the error and the worktrees made so far.
    async fn fill_project_worktree(
        &self,
        project: &Path,
        folder: &Path,
        chosen: &[String],
        all: &HashSet<String>,
        branch: &str,
        base: &str,
    ) -> Result<Vec<Worktree>, (EngineError, Vec<Worktree>)> {
        let wanted: HashSet<String> = chosen.iter().cloned().collect();
        let (source, into, every) = (project.to_path_buf(), folder.to_path_buf(), all.clone());
        match disposable_worker("project-worktree-layout", move || {
            lay_out(&source, &into, &wanted, &every)
        })
        .await
        {
            Some(layout) if layout.failed > 0 => tracing::warn!(
                path = %folder.display(),
                links = layout.links,
                failed = layout.failed,
                "project worktree: some of the project's own files couldn't be linked"
            ),
            Some(layout) => {
                tracing::debug!(path = %folder.display(), links = layout.links, "project worktree laid out")
            }
            None => tracing::warn!(path = %folder.display(), "project worktree: layout didn't run"),
        }
        let mut done: Vec<Worktree> = Vec::new();
        for path in chosen {
            let repository = project.join(path);
            let at = folder.join(path);
            let step = async {
                #[cfg(windows)]
                validate_windows_worktree_path(&at.to_string_lossy())?;
                if let Some(parent) = at.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let from = if base != "HEAD" && self.branch_exists(&repository, base).await {
                    base
                } else {
                    "HEAD"
                };
                self.add_worktree_checkout(&repository, &at, branch, from)
                    .await?;
                Ok::<_, EngineError>(())
            }
            .await;
            if let Err(error) = step {
                // (A failed add may have registered the checkout before
                // failing: undone with the rest.)
                done.push(Worktree {
                    repo_path: repository.to_string_lossy().into_owned(),
                    path: at.to_string_lossy().into_owned(),
                    branch: branch.to_string(),
                    name: String::new(),
                    checkout_id: None,
                });
                return Err((
                    EngineError::Other(format!("Couldn't check out {path}: {error}")),
                    done,
                ));
            }
            let checkout_id = self.checkout_identity(&at).await.ok().map(|id| id.id);
            done.push(Worktree {
                repo_path: repository.to_string_lossy().into_owned(),
                path: at.to_string_lossy().into_owned(),
                branch: branch.to_string(),
                name: path.clone(),
                checkout_id,
            });
        }
        Ok(done)
    }

    /// Undo a project worktree that didn't finish: each worktree made (and
    /// the branch it was made on, which this creation made), then the folder.
    async fn undo_project_worktree(&self, folder: &Path, done: &[Worktree], branch: &str) {
        for worktree in done {
            let repository = Path::new(&worktree.repo_path);
            if Path::new(&worktree.path).exists() {
                let _ = self
                    .git(
                        &["worktree", "remove", "--force", &worktree.path],
                        Some(repository),
                    )
                    .await;
            }
            let _ = self.git(&["worktree", "prune"], Some(repository)).await;
            let _ = self.git(&["branch", "-D", branch], Some(repository)).await;
        }
        // (Links are removed as links: their originals are never touched.)
        if let Err(error) = std::fs::remove_dir_all(folder) {
            tracing::warn!(path = %folder.display(), %error, "project worktree: couldn't remove the unfinished folder");
        }
    }

    /// The linked worktrees in the project worktree at `folder`, each with
    /// the repository it belongs to (its common git dir).
    async fn project_worktree_checkouts(
        &self,
        folder: &Path,
    ) -> Result<Vec<(PathBuf, String)>, EngineError> {
        let (found, _) = self.discover(folder, true).await?;
        let mut out = Vec::new();
        for found in found.into_iter().filter(|found| found.linked) {
            let at = folder.join(&found.path);
            let common = self
                .git(
                    &["rev-parse", "--path-format=absolute", "--git-common-dir"],
                    Some(&at),
                )
                .await?;
            out.push((at, common));
        }
        Ok(out)
    }

    /// Whether `folder` is a project worktree of the project at `project`:
    /// its linked worktrees all belong to repositories in that folder.
    pub(crate) async fn is_project_worktree_of(&self, folder: &Path, project: &Path) -> bool {
        if !is_project_worktree_folder(folder).await {
            return false;
        }
        let Ok(project) = std::fs::canonicalize(project) else {
            return false;
        };
        let Ok(checkouts) = self.project_worktree_checkouts(folder).await else {
            return false;
        };
        !checkouts.is_empty()
            && checkouts.iter().all(|(_, common)| {
                std::fs::canonicalize(common).is_ok_and(|common| common.starts_with(&project))
            })
    }

    /// [`Self::rename_worktree_branch`] for a project worktree: every one of
    /// its worktrees still on `expected_branch` (and that the folder's own
    /// `zeron/<name>`) moves to one name from `title`, the same in each — a
    /// name taken in any of them gets the folder's suffix, as a single
    /// worktree's does. One that can't be renamed puts back those that were.
    pub(crate) async fn rename_project_worktree_branch(
        &self,
        folder: &Path,
        expected_branch: &str,
        title: &str,
    ) -> Result<String, EngineError> {
        let name = folder
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        if expected_branch != format!("zeron/{name}") {
            return Ok(expected_branch.to_string());
        }
        let mut on_it = Vec::new();
        for (at, _) in self.project_worktree_checkouts(folder).await? {
            // (One switched to another branch since is the person's.)
            if self.current_branch(&at).await.ok().as_deref() == Some(expected_branch) {
                on_it.push(at);
            }
        }
        if on_it.is_empty() {
            return Ok(expected_branch.to_string());
        }
        let preferred = worktree_branch_from_title(title);
        if preferred == expected_branch {
            return Ok(preferred);
        }
        let mut hasher = Sha256::new();
        hasher.update(folder.to_string_lossy().as_bytes());
        let suffix = &hex(&hasher.finalize())[..6];
        let mut target = preferred.clone();
        for at in &on_it {
            if self.branch_exists(at, &preferred).await {
                target = format!("{preferred}-{suffix}");
                break;
            }
        }
        for at in &on_it {
            if self.branch_exists(at, &target).await {
                return Err(EngineError::Other(format!(
                    "Branch already exists: {target}"
                )));
            }
        }
        let mut renamed: Vec<&PathBuf> = Vec::new();
        for at in &on_it {
            if let Err(error) = self
                .git(&["branch", "-m", "--", expected_branch, &target], Some(at))
                .await
            {
                for back in renamed {
                    let _ = self
                        .git(
                            &["branch", "-m", "--", &target, expected_branch],
                            Some(back),
                        )
                        .await;
                }
                return Err(error);
            }
            renamed.push(at);
        }
        Ok(target)
    }

    /// [`Self::delete_worktree`] for a project worktree: each of its
    /// worktrees removed from its repository (and its branch, where Zeron
    /// made it), then the folder with the links in it (never what they
    /// point to). A folder holding no worktree is refused.
    pub(crate) async fn delete_project_worktree(&self, folder: &Path) -> Result<(), EngineError> {
        let checkouts = self.project_worktree_checkouts(folder).await?;
        if checkouts.is_empty() {
            return Err(EngineError::Other(format!(
                "{} is not a worktree",
                folder.display()
            )));
        }
        for (at, common) in checkouts {
            let branch = self.current_branch(&at).await.unwrap_or_default();
            let git_dir = format!("--git-dir={common}");
            let path = at.to_string_lossy();
            if self
                .git(&[&git_dir, "worktree", "remove", "--force", &path], None)
                .await
                .is_err()
            {
                let _ = std::fs::remove_dir_all(&at);
            }
            let _ = self.git(&[&git_dir, "worktree", "prune"], None).await;
            if branch.starts_with("zeron/") {
                let _ = self.git(&[&git_dir, "branch", "-D", &branch], None).await;
            }
        }
        std::fs::remove_dir_all(folder)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn git(cwd: &Path, args: &[&str]) -> String {
        let output = tokio::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@test")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@test")
            .output()
            .await
            .expect("git spawns");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    async fn repository(at: &Path, branch: &str) {
        std::fs::create_dir_all(at).unwrap();
        git(at, &["init", "-q", "-b", branch]).await;
        std::fs::write(at.join("README.md"), "hello\n").unwrap();
        git(at, &["add", "."]).await;
        git(at, &["commit", "-q", "-m", "first"]).await;
    }

    /// acme/{AGENTS.md, api (main), web (develop), packages/{ui, README.md},
    /// empty (no commit), node_modules/dep, .cache/hidden}
    async fn acme(tmp: &Path) -> PathBuf {
        let project = tmp.join("acme");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join("AGENTS.md"), "shared\n").unwrap();
        repository(&project.join("api"), "main").await;
        repository(&project.join("web"), "develop").await;
        repository(&project.join("packages/ui"), "main").await;
        std::fs::write(project.join("packages/README.md"), "packages\n").unwrap();
        std::fs::create_dir_all(project.join("empty")).unwrap();
        git(&project.join("empty"), &["init", "-q", "-b", "main"]).await;
        repository(&project.join("node_modules/dep"), "main").await;
        repository(&project.join(".cache/hidden"), "main").await;
        project
    }

    fn repos(tmp: &Path) -> Repos {
        Repos::with_worktrees_root(&tmp.join("data"), "test", tmp.join("worktrees"))
    }

    #[test]
    fn a_place_inside_a_folder_is_plain_names() {
        for good in ["api", "packages/web", "a.b"] {
            assert!(is_inside(good), "{good}");
        }
        for bad in [
            "", "..", "../x", "a/../b", "/abs", "a//b", "a/./b", "C:/x", "a\\b",
        ] {
            assert!(!is_inside(bad), "{bad}");
        }
    }

    #[tokio::test]
    async fn a_project_lists_its_repositories_one_level_into_plain_folders() {
        let tmp = tempfile::tempdir().unwrap();
        let project = acme(tmp.path()).await;
        let listing = repos(tmp.path())
            .project_repositories(&project)
            .await
            .unwrap();
        assert!(!listing.is_repo);
        let paths: Vec<_> = listing
            .repositories
            .iter()
            .map(|r| r.path.as_str())
            .collect();
        assert_eq!(paths, ["api", "empty", "packages/ui", "web"]);
        let web = &listing.repositories[3];
        assert_eq!(web.branch.as_deref(), Some("develop"));
        assert!(web.has_commits && !web.linked);
        assert!(!listing.repositories[1].has_commits, "no commit yet");
    }

    #[tokio::test]
    async fn a_repository_lists_none_of_its_own() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("solo");
        repository(&repo, "main").await;
        repository(&repo.join("vendor/inner"), "main").await;
        let listing = repos(tmp.path()).project_repositories(&repo).await.unwrap();
        assert!(listing.is_repo);
        assert!(listing.repositories.is_empty());
    }

    #[tokio::test]
    async fn a_project_worktree_is_laid_out_like_the_project_on_one_branch() {
        let tmp = tempfile::tempdir().unwrap();
        let project = acme(tmp.path()).await;
        let repos = repos(tmp.path());
        let made = repos
            .create_project_worktree(
                &project,
                &["web".into(), "packages/ui".into(), "web/".into()],
                "HEAD",
            )
            .await
            .unwrap();
        let folder = PathBuf::from(&made.worktree.path);
        assert!(folder.starts_with(tmp.path().join("worktrees").join("acme")));
        assert_eq!(
            made.worktree.branch,
            format!("zeron/{}", made.worktree.name)
        );
        assert_eq!(made.worktree.repo_path, project.to_string_lossy());
        let paths: Vec<_> = made.repositories.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(paths, ["packages/ui", "web"]);
        for path in ["web", "packages/ui"] {
            let at = folder.join(path);
            assert_eq!(
                git(&at, &["branch", "--show-current"]).await,
                made.worktree.branch
            );
            assert_eq!(marker(&at), Marker::Linked);
        }
        // Each off its own current commit (web's is develop's).
        assert_eq!(
            git(&folder.join("web"), &["rev-parse", "HEAD"]).await,
            git(&project.join("web"), &["rev-parse", "develop"]).await
        );
        // The project's own files are links; a plain folder holding a
        // repository is made, its other entries linked.
        #[cfg(unix)]
        {
            assert!(
                std::fs::symlink_metadata(folder.join("AGENTS.md"))
                    .unwrap()
                    .file_type()
                    .is_symlink()
            );
            assert!(
                !std::fs::symlink_metadata(folder.join("packages"))
                    .unwrap()
                    .file_type()
                    .is_symlink()
            );
            assert!(
                std::fs::symlink_metadata(folder.join("packages/README.md"))
                    .unwrap()
                    .file_type()
                    .is_symlink()
            );
            assert_eq!(
                std::fs::read_to_string(folder.join("AGENTS.md")).unwrap(),
                "shared\n"
            );
        }
        // Repositories not chosen are left out, never linked.
        assert!(!folder.join("api").exists());
        assert!(!folder.join("empty").exists());
        // The project folder itself gains nothing.
        assert!(!project.join(".git").exists());

        assert!(repos.is_project_worktree_of(&folder, &project).await);
        assert!(!repos.is_project_worktree_of(&project, &project).await);
        let listing = repos.project_repositories(&folder).await.unwrap();
        assert!(listing.repositories.iter().all(|r| r.linked));
        assert_eq!(listing.repositories.len(), 2);
    }

    #[tokio::test]
    async fn a_base_ref_is_used_where_a_repository_has_it() {
        let tmp = tempfile::tempdir().unwrap();
        let project = acme(tmp.path()).await;
        git(&project.join("api"), &["branch", "release"]).await;
        git(
            &project.join("api"),
            &["commit", "-q", "--allow-empty", "-m", "later"],
        )
        .await;
        let made = repos(tmp.path())
            .create_project_worktree(&project, &["api".into(), "web".into()], "release")
            .await
            .unwrap();
        let folder = PathBuf::from(&made.worktree.path);
        assert_eq!(
            git(&folder.join("api"), &["rev-parse", "HEAD"]).await,
            git(&project.join("api"), &["rev-parse", "release"]).await,
        );
        assert_eq!(
            git(&folder.join("web"), &["rev-parse", "HEAD"]).await,
            git(&project.join("web"), &["rev-parse", "HEAD"]).await,
        );
    }

    #[tokio::test]
    async fn what_cant_be_checked_out_is_refused_before_anything_is_made() {
        let tmp = tempfile::tempdir().unwrap();
        let project = acme(tmp.path()).await;
        let repos = repos(tmp.path());
        for (asked, says) in [
            (vec![], "at least one"),
            (vec!["../api".to_string()], "not a folder"),
            (
                vec!["node_modules/dep".to_string()],
                "no longer a repository",
            ),
            (vec![".cache/hidden".to_string()], "no longer a repository"),
            (vec!["empty".to_string()], "no commit"),
        ] {
            let error = repos
                .create_project_worktree(&project, &asked, "HEAD")
                .await
                .unwrap_err()
                .to_string();
            assert!(error.contains(says), "{asked:?}: {error}");
        }
        let solo = tmp.path().join("solo");
        repository(&solo, "main").await;
        assert!(
            repos
                .create_project_worktree(&solo, &["x".into()], "HEAD")
                .await
                .is_err()
        );
        let left = std::fs::read_dir(tmp.path().join("worktrees").join("acme"))
            .map(|read| read.count())
            .unwrap_or(0);
        assert_eq!(left, 0, "nothing claimed");
    }

    #[tokio::test]
    async fn one_repository_failing_undoes_the_others() {
        let tmp = tempfile::tempdir().unwrap();
        let project = acme(tmp.path()).await;
        // web's checkout fails: a post-checkout hook that refuses.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let hook = project.join("web/.git/hooks/post-checkout");
            std::fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
            std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        #[cfg(not(unix))]
        return;
        let repos = repos(tmp.path());
        let error = repos
            .create_project_worktree(&project, &["api".into(), "web".into()], "HEAD")
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("web"), "{error}");
        let left = std::fs::read_dir(tmp.path().join("worktrees").join("acme"))
            .unwrap()
            .count();
        assert_eq!(left, 0, "the folder is gone");
        for repo in ["api", "web"] {
            let branches = git(&project.join(repo), &["branch", "--list", "zeron/*"]).await;
            assert!(branches.is_empty(), "{repo}: {branches}");
            let list = git(&project.join(repo), &["worktree", "list", "--porcelain"]).await;
            assert_eq!(list.matches("worktree ").count(), 1, "{repo}: {list}");
        }
        // The originals the links pointed to are untouched.
        assert_eq!(
            std::fs::read_to_string(project.join("AGENTS.md")).unwrap(),
            "shared\n"
        );
    }

    #[tokio::test]
    async fn a_title_renames_every_repository_s_branch_alike() {
        let tmp = tempfile::tempdir().unwrap();
        let project = acme(tmp.path()).await;
        let repos = repos(tmp.path());
        // web already has the preferred name: both get the suffix.
        git(&project.join("web"), &["branch", "zeron/fix-login"]).await;
        let made = repos
            .create_project_worktree(&project, &["api".into(), "web".into()], "HEAD")
            .await
            .unwrap();
        let folder = PathBuf::from(&made.worktree.path);
        let renamed = repos
            .rename_worktree_branch(&folder, &made.worktree.branch, "Fix login")
            .await
            .unwrap();
        assert!(renamed.starts_with("zeron/fix-login-"), "{renamed}");
        for repo in ["api", "web"] {
            assert_eq!(
                git(&folder.join(repo), &["branch", "--show-current"]).await,
                renamed
            );
        }
        // Renamed already: the old name no longer matches, nothing moves.
        let again = repos
            .rename_worktree_branch(&folder, &made.worktree.branch, "Other")
            .await
            .unwrap();
        assert_eq!(again, made.worktree.branch);
        assert_eq!(
            git(&folder.join("api"), &["branch", "--show-current"]).await,
            renamed
        );
    }

    #[tokio::test]
    async fn deleting_a_project_worktree_removes_its_worktrees_not_the_originals() {
        let tmp = tempfile::tempdir().unwrap();
        let project = acme(tmp.path()).await;
        let repos = repos(tmp.path());
        let made = repos
            .create_project_worktree(&project, &["api".into(), "packages/ui".into()], "HEAD")
            .await
            .unwrap();
        let folder = PathBuf::from(&made.worktree.path);
        repos.delete_worktree(&project, &folder).await.unwrap();
        assert!(!folder.exists());
        for repo in ["api", "packages/ui"] {
            let branches = git(&project.join(repo), &["branch", "--list", "zeron/*"]).await;
            assert!(branches.is_empty(), "{repo}: {branches}");
        }
        assert_eq!(
            std::fs::read_to_string(project.join("AGENTS.md")).unwrap(),
            "shared\n"
        );
        assert!(project.join("packages/README.md").exists());
        // A plain folder is never taken for one.
        let plain = tmp.path().join("plain");
        std::fs::create_dir_all(plain.join("sub")).unwrap();
        assert!(!is_project_worktree_folder(&plain).await);
    }

    #[tokio::test]
    async fn the_cap_stops_a_listing() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("many");
        for i in 0..=PROJECT_REPOSITORIES_MAX {
            std::fs::create_dir_all(root.join(format!("r{i:03}/.git"))).unwrap();
        }
        let (found, truncated) = discover(&root, false);
        assert_eq!(found.len(), PROJECT_REPOSITORIES_MAX);
        assert!(truncated);
    }
}
