//! Projects composed of repositories from anywhere on their device.
//!
//! A project's folder can include repositories that live elsewhere on its
//! device: each is a link in the folder (`api → ~/code/api`), and the folder's
//! `.zeron-project.json` names them. Everything that reads a project of
//! repositories from its folder — the listing, a worktree of the project, the
//! Git panel, iOS and Android — then finds them as it finds the folder's own:
//!
//! ```text
//! ~/.zeron/projects/acme/          (made by ComposeProject)
//!   .zeron-project.json            {"repositories":[{"name":"api","path":…}]}
//!   api  → ~/code/api              (a repository, included)
//!   web  → ~/work/web-app          (included under another name)
//!   AGENTS.md                      (the project's own: written by anyone)
//! ```
//!
//! Only links the file names are Zeron's: taking a repository out removes its
//! link, never the repository, and never anything else in the folder. A link
//! whose repository has moved or gone is listed as missing until it's taken
//! out or included again from its new place. A folder of your own (one that
//! already holds repositories) can include more the same way.

use super::project_worktrees::{Found, Marker, PROJECT_REPOSITORIES_MAX, is_inside, link, marker};
use super::*;
use serde::{Deserialize, Serialize};
use zeron_proto::{ComposedProject, ProjectMember, ProjectRepositories};

/// The file in a project's folder naming the repositories it includes.
pub(crate) const MANIFEST: &str = ".zeron-project.json";
/// The longest that file is read (it names at most a few dozen paths).
const MANIFEST_MAX_BYTES: u64 = 256 * 1024;
/// The longest name a project's folder or an included repository's link has.
const NAME_MAX_CHARS: usize = 80;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    repositories: Vec<Included>,
}

/// One included repository: its link's name in the folder, and where it
/// pointed when included (absolute).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Included {
    name: String,
    path: String,
}

fn read_manifest(folder: &Path) -> Manifest {
    let at = folder.join(MANIFEST);
    match std::fs::metadata(&at) {
        Ok(meta) if meta.is_file() && meta.len() <= MANIFEST_MAX_BYTES => {}
        _ => return Manifest::default(),
    }
    std::fs::read_to_string(&at)
        .ok()
        .and_then(|text| serde_json::from_str::<Manifest>(&text).ok())
        .unwrap_or_default()
}

/// Written whole (a temporary file renamed over it); none included: no file.
fn write_manifest(folder: &Path, included: &[Included]) -> std::io::Result<()> {
    let at = folder.join(MANIFEST);
    if included.is_empty() {
        return match std::fs::remove_file(&at) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        };
    }
    let manifest = Manifest {
        version: 1,
        repositories: included.to_vec(),
    };
    let text = serde_json::to_string_pretty(&manifest).map_err(std::io::Error::other)?;
    let temporary = folder.join(format!("{MANIFEST}.{}", std::process::id()));
    std::fs::write(&temporary, format!("{text}\n"))?;
    std::fs::rename(&temporary, &at).inspect_err(|_| {
        let _ = std::fs::remove_file(&temporary);
    })
}

fn is_link(at: &Path) -> bool {
    std::fs::symlink_metadata(at).is_ok_and(|meta| meta.file_type().is_symlink())
}

/// Where the link at `at` (in `folder`) leads, absolute.
fn link_target(folder: &Path, at: &Path) -> Option<PathBuf> {
    let target = std::fs::read_link(at).ok()?;
    Some(if target.is_absolute() {
        target
    } else {
        folder.join(target)
    })
}

/// A link removed as a link (a folder's on Windows), never what it leads to.
fn unlink(at: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    if std::fs::remove_dir(at).is_ok() {
        return Ok(());
    }
    std::fs::remove_file(at)
}

/// `raw` as one folder entry's name: trimmed, separators and characters no
/// file system takes made dashes, no leading dot (never hidden), no
/// trailing dot or space (Windows), at most [`NAME_MAX_CHARS`]. `None` when
/// nothing is left.
pub(crate) fn entry_name(raw: &str) -> Option<String> {
    let cleaned: String = raw
        .trim()
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '-'
            } else {
                c
            }
        })
        .take(NAME_MAX_CHARS)
        .collect();
    let cleaned = cleaned
        .trim_start_matches(['.', ' '])
        .trim_end_matches(['.', ' '])
        .to_string();
    (!cleaned.is_empty() && cleaned != "node_modules").then_some(cleaned)
}

/// The repositories `root` includes from elsewhere, added to what [`super::
/// project_worktrees`] found in it: each whose link is still there (a real
/// folder in its place is the folder's own now), missing when it no longer
/// leads to a repository. True when the cap was reached.
pub(super) fn included(root: &Path, found: &mut Vec<Found>) -> bool {
    for entry in read_manifest(root).repositories {
        if !is_inside(&entry.name)
            || entry.name.contains('/')
            || found.iter().any(|found| found.path == entry.name)
        {
            continue;
        }
        let at = root.join(&entry.name);
        if !is_link(&at) {
            continue;
        }
        let target = link_target(root, &at).unwrap_or_else(|| PathBuf::from(&entry.path));
        let present = marker(&target) == Marker::Repository;
        if found.len() == PROJECT_REPOSITORIES_MAX {
            return true;
        }
        found.push(Found {
            path: entry.name,
            linked: false,
            location: Some(target),
            missing: !present,
        });
    }
    false
}

/// Where the repositories `project` includes are (whether or not they're
/// still there).
pub(super) fn included_locations(project: &Path) -> Vec<PathBuf> {
    read_manifest(project)
        .repositories
        .into_iter()
        .filter_map(|entry| {
            let at = project.join(&entry.name);
            is_link(&at)
                .then(|| link_target(project, &at).unwrap_or_else(|| PathBuf::from(&entry.path)))
        })
        .collect()
}

/// One repository asked for, checked: where it really is, and the name it
/// asked to go by.
struct Wanted {
    at: PathBuf,
    name: Option<String>,
}

impl Repos {
    /// Where new projects' folders go unless another is chosen.
    pub(crate) fn projects_root(&self) -> PathBuf {
        self.inner.projects_root.clone()
    }

    /// Each repository asked for, as it is on this device: a repository's
    /// own top folder (not a folder inside one, a linked worktree or a
    /// submodule), outside `folder` and not holding it. Each once.
    async fn wanted_repositories(
        &self,
        folder: Option<&Path>,
        members: &[ProjectMember],
    ) -> Result<Vec<Wanted>, EngineError> {
        let folder = folder.and_then(|folder| std::fs::canonicalize(folder).ok());
        let mut wanted: Vec<Wanted> = Vec::new();
        for member in members {
            let path = expand_home(member.path.trim())
                .map_err(|error| EngineError::Other(error.to_string()))?;
            let asked = absolutize(Path::new(&path));
            let Ok(at) = std::fs::canonicalize(&asked) else {
                return Err(EngineError::Other(format!(
                    "There's no folder at {}",
                    asked.display()
                )));
            };
            match marker(&at) {
                Marker::Repository => {}
                Marker::Linked => {
                    return Err(EngineError::Other(format!(
                        "{} is a worktree of another repository: include that repository instead",
                        asked.display()
                    )));
                }
                Marker::Submodule => {
                    return Err(EngineError::Other(format!(
                        "{} is a submodule: include the repository holding it instead",
                        asked.display()
                    )));
                }
                Marker::None => {
                    let message = if self.is_repo(&at).await {
                        format!(
                            "{} is inside a repository: include its top folder instead",
                            asked.display()
                        )
                    } else {
                        format!("{} isn't a git repository", asked.display())
                    };
                    return Err(EngineError::Other(message));
                }
            }
            if let Some(folder) = &folder {
                if at.starts_with(folder) {
                    return Err(EngineError::Other(format!(
                        "{} is already in the project's folder",
                        asked.display()
                    )));
                }
                if folder.starts_with(&at) {
                    return Err(EngineError::Other(format!(
                        "{} holds the project's folder, so it can't be in it",
                        asked.display()
                    )));
                }
            }
            if wanted.iter().any(|w| w.at == at) {
                continue;
            }
            let name = match member.name.as_deref().map(str::trim) {
                Some(raw) if !raw.is_empty() => Some(entry_name(raw).ok_or_else(|| {
                    EngineError::Other(format!("{raw:?} can't be a folder's name"))
                })?),
                _ => None,
            };
            wanted.push(Wanted { at, name });
        }
        Ok(wanted)
    }

    /// `ComposeProject`: a new folder for a project named `name` in
    /// `parent` (the projects folder by default) including `members`.
    /// Its folder is the name, or `name 2`… when that's taken (claimed by
    /// making it). Nothing is left behind when a repository can't be
    /// included.
    pub async fn compose_project(
        &self,
        name: &str,
        parent: Option<&str>,
        members: &[ProjectMember],
    ) -> Result<ComposedProject, EngineError> {
        let folder_name =
            entry_name(name).ok_or_else(|| EngineError::Other("Name the project".into()))?;
        // Checked first: a mistake leaves no empty folder behind.
        self.wanted_repositories(None, members).await?;
        let parent = match parent.map(str::trim).filter(|parent| !parent.is_empty()) {
            Some(parent) => {
                let parent = absolutize(Path::new(
                    &expand_home(parent).map_err(|error| EngineError::Other(error.into()))?,
                ));
                if !parent.is_dir() {
                    return Err(EngineError::Other(format!(
                        "No such folder: {}",
                        parent.display()
                    )));
                }
                parent
            }
            None => {
                let root = self.projects_root();
                std::fs::create_dir_all(&root)?;
                root
            }
        };
        let mut claimed = None;
        for n in 1..100 {
            let candidate = if n == 1 {
                folder_name.clone()
            } else {
                format!("{folder_name} {n}")
            };
            let folder = parent.join(&candidate);
            match std::fs::create_dir(&folder) {
                Ok(()) => {
                    claimed = Some(folder);
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        }
        let folder = claimed.ok_or_else(|| {
            EngineError::Other(format!(
                "There are already folders named {folder_name} in {}",
                parent.display()
            ))
        })?;
        match self.set_project_repositories(&folder, members).await {
            Ok(repositories) => {
                tracing::info!(path = %folder.display(), repositories = repositories.repositories.len(), "project composed");
                Ok(ComposedProject {
                    path: folder.to_string_lossy().into_owned(),
                    repositories,
                })
            }
            Err(error) => {
                // (Only links and the list are in it: removed as links.)
                let _ = std::fs::remove_dir_all(&folder);
                Err(error)
            }
        }
    }

    /// `SetProjectRepositories`: the project at `folder` includes exactly
    /// `members`, in that order, each by a link in the folder named for it
    /// (its own name, or the one asked, made unique beside the folder's own
    /// entries). Those it included before and doesn't now are taken out:
    /// their links removed, nothing else. A repository kept keeps its name
    /// unless another is asked. The folder's own repositories are its own,
    /// not changed here. Answers the project's repositories as listed then.
    pub async fn set_project_repositories(
        &self,
        folder: &Path,
        members: &[ProjectMember],
    ) -> Result<ProjectRepositories, EngineError> {
        if !folder.is_dir() {
            return Err(EngineError::Other(format!(
                "{} is not on this device",
                folder.display()
            )));
        }
        if self.is_repo(folder).await {
            return Err(EngineError::Other(format!(
                "{} is a repository: a project including others needs a folder of its own",
                folder.display()
            )));
        }
        let wanted = self.wanted_repositories(Some(folder), members).await?;
        let before = read_manifest(folder).repositories;
        let mine: HashSet<String> = before
            .iter()
            .filter(|entry| is_link(&folder.join(&entry.name)))
            .map(|entry| entry.name.clone())
            .collect();
        let place = |entry: &Included| {
            link_target(folder, &folder.join(&entry.name))
                .and_then(|at| std::fs::canonicalize(at).ok())
                .or_else(|| std::fs::canonicalize(&entry.path).ok())
        };

        // The folder's own entries keep their names (case aside, as macOS
        // and Windows see them).
        let mut taken: HashSet<String> = std::fs::read_dir(folder)?
            .filter_map(Result::ok)
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| !mine.contains(name))
            .map(|name| name.to_lowercase())
            .collect();
        taken.insert(MANIFEST.to_lowercase());

        // Each wanted one's name: asked, else as it was, else its folder's.
        let mut plan: Vec<(String, PathBuf)> = Vec::new();
        for want in &wanted {
            let kept = before
                .iter()
                .find(|entry| mine.contains(&entry.name) && place(entry).as_ref() == Some(&want.at))
                .map(|entry| entry.name.clone());
            let preferred = want
                .name
                .clone()
                .or(kept)
                .or_else(|| {
                    want.at
                        .file_name()
                        .and_then(|name| entry_name(&name.to_string_lossy()))
                })
                .unwrap_or_else(|| "repository".to_string());
            let mut name = preferred.clone();
            let mut n = 2;
            while taken.contains(&name.to_lowercase()) {
                name = format!("{preferred}-{n}");
                n += 1;
            }
            taken.insert(name.to_lowercase());
            plan.push((name, want.at.clone()));
        }

        // Out first (a name can pass from one repository to another).
        let mut failure: Option<EngineError> = None;
        let mut staying: Vec<Included> = Vec::new();
        for entry in &before {
            if !mine.contains(&entry.name) {
                continue;
            }
            let at = folder.join(&entry.name);
            let same = plan
                .iter()
                .any(|(name, to)| name == &entry.name && place(entry).as_ref() == Some(to));
            if same {
                continue;
            }
            if let Err(error) = unlink(&at) {
                failure.get_or_insert(EngineError::Other(format!(
                    "Couldn't take {} out of the project: {error}",
                    entry.name
                )));
                staying.push(entry.clone());
            }
        }
        let mut included: Vec<Included> = Vec::new();
        for (name, to) in plan {
            let at = folder.join(&name);
            let entry = Included {
                name: name.clone(),
                path: to.to_string_lossy().into_owned(),
            };
            if is_link(&at)
                && link_target(folder, &at)
                    .and_then(|t| std::fs::canonicalize(t).ok())
                    .as_ref()
                    == Some(&to)
            {
                included.push(entry);
                continue;
            }
            match link(&to, &at, true) {
                Ok(()) => included.push(entry),
                Err(error) => {
                    failure.get_or_insert(EngineError::Other(format!(
                        "Couldn't include {name} in the project: {error}"
                    )));
                }
            }
        }
        included.extend(staying);
        write_manifest(folder, &included)?;
        if let Some(error) = failure {
            return Err(error);
        }
        self.project_repositories(folder).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn git(cwd: &Path, args: &[&str]) {
        let out = tokio::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .output()
            .await
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    async fn repository(at: &Path) {
        std::fs::create_dir_all(at).unwrap();
        git(at, &["init", "-q", "-b", "main"]).await;
        std::fs::write(at.join("README.md"), "hi\n").unwrap();
        git(at, &["add", "."]).await;
        git(at, &["commit", "-q", "-m", "first"]).await;
    }

    fn repos(tmp: &Path) -> Repos {
        Repos::with_worktrees_root(&tmp.join("data"), "dev", tmp.join("worktrees"))
    }

    fn member(path: &Path) -> ProjectMember {
        ProjectMember {
            path: path.to_string_lossy().into_owned(),
            name: None,
        }
    }

    #[test]
    fn names_are_one_plain_entry() {
        assert_eq!(
            entry_name("  Acme Platform ").as_deref(),
            Some("Acme Platform")
        );
        assert_eq!(entry_name("a/b:c").as_deref(), Some("a-b-c"));
        assert_eq!(entry_name(".hidden.").as_deref(), Some("hidden"));
        assert_eq!(entry_name("  ..  "), None);
        assert_eq!(entry_name("node_modules"), None);
    }

    #[tokio::test]
    async fn a_composed_project_lists_repositories_from_anywhere() {
        let tmp = tempfile::tempdir().unwrap();
        let tmp = &std::fs::canonicalize(tmp.path()).unwrap();
        repository(&tmp.join("code/api")).await;
        repository(&tmp.join("elsewhere/web-app")).await;
        let repos = repos(tmp);
        let made = repos
            .compose_project(
                "Acme",
                None,
                &[
                    member(&tmp.join("code/api")),
                    ProjectMember {
                        path: tmp.join("elsewhere/web-app").to_string_lossy().into_owned(),
                        name: Some("web".into()),
                    },
                ],
            )
            .await
            .unwrap();
        assert_eq!(Path::new(&made.path), tmp.join("data/projects/Acme"));
        let listed: Vec<_> = made
            .repositories
            .repositories
            .iter()
            .map(|r| {
                (
                    r.path.as_str(),
                    r.location.clone().unwrap(),
                    r.has_commits,
                    r.missing,
                )
            })
            .collect();
        assert_eq!(
            listed,
            [
                (
                    "api",
                    tmp.join("code/api").to_string_lossy().into_owned(),
                    true,
                    false
                ),
                (
                    "web",
                    tmp.join("elsewhere/web-app").to_string_lossy().into_owned(),
                    true,
                    false
                ),
            ]
        );
        // A second of the same name gets a folder of its own.
        let again = repos
            .compose_project("Acme", None, &[member(&tmp.join("code/api"))])
            .await
            .unwrap();
        assert_eq!(Path::new(&again.path), tmp.join("data/projects/Acme 2"));
    }

    #[tokio::test]
    async fn what_isnt_a_repository_s_top_is_refused_and_leaves_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let tmp = &std::fs::canonicalize(tmp.path()).unwrap();
        repository(&tmp.join("code/api")).await;
        std::fs::create_dir_all(tmp.join("code/api/src")).unwrap();
        std::fs::create_dir_all(tmp.join("plain")).unwrap();
        let repos = repos(tmp);
        for (path, says) in [
            (tmp.join("code/api/src"), "inside a repository"),
            (tmp.join("plain"), "isn't a git repository"),
            (tmp.join("nowhere"), "no folder at"),
        ] {
            let error = repos
                .compose_project("Acme", None, &[member(&path)])
                .await
                .unwrap_err()
                .to_string();
            assert!(error.contains(says), "{error}");
        }
        assert!(!tmp.join("data/projects/Acme").exists());
    }

    #[tokio::test]
    async fn changing_what_a_project_includes_touches_only_its_links() {
        let tmp = tempfile::tempdir().unwrap();
        let tmp = &std::fs::canonicalize(tmp.path()).unwrap();
        // A folder of the person's own, holding a repository and a file.
        let project = tmp.join("acme");
        repository(&project.join("api")).await;
        std::fs::write(project.join("AGENTS.md"), "be kind\n").unwrap();
        repository(&tmp.join("code/web")).await;
        repository(&tmp.join("code/docs")).await;
        // One named like the folder's own repository.
        repository(&tmp.join("other/api")).await;
        let repos = repos(tmp);

        let listed = repos
            .set_project_repositories(
                &project,
                &[
                    member(&tmp.join("code/web")),
                    member(&tmp.join("other/api")),
                ],
            )
            .await
            .unwrap();
        let paths: Vec<_> = listed
            .repositories
            .iter()
            .map(|r| r.path.as_str())
            .collect();
        assert_eq!(paths, ["api", "api-2", "web"]);
        assert!(listed.repositories[0].location.is_none());

        // Out with web, in with docs: web's link goes, nothing else.
        let listed = repos
            .set_project_repositories(
                &project,
                &[
                    member(&tmp.join("other/api")),
                    member(&tmp.join("code/docs")),
                ],
            )
            .await
            .unwrap();
        let paths: Vec<_> = listed
            .repositories
            .iter()
            .map(|r| r.path.as_str())
            .collect();
        assert_eq!(paths, ["api", "api-2", "docs"]);
        assert!(!project.join("web").exists());
        assert!(tmp.join("code/web/README.md").exists());
        assert!(project.join("AGENTS.md").exists() && project.join("api/.git").exists());

        // None: the list goes too.
        repos.set_project_repositories(&project, &[]).await.unwrap();
        assert!(!project.join(MANIFEST).exists() && !project.join("docs").exists());
        assert!(project.join("api/.git").exists());
    }

    #[tokio::test]
    async fn a_repository_gone_from_its_place_is_missing_not_lost() {
        let tmp = tempfile::tempdir().unwrap();
        let tmp = &std::fs::canonicalize(tmp.path()).unwrap();
        repository(&tmp.join("code/api")).await;
        repository(&tmp.join("code/web")).await;
        let repos = repos(tmp);
        let made = repos
            .compose_project(
                "Acme",
                None,
                &[member(&tmp.join("code/api")), member(&tmp.join("code/web"))],
            )
            .await
            .unwrap();
        std::fs::rename(tmp.join("code/web"), tmp.join("code/web-moved")).unwrap();
        let listed = repos
            .project_repositories(Path::new(&made.path))
            .await
            .unwrap();
        let web = &listed.repositories[1];
        assert!(web.missing && !web.has_commits);
        assert_eq!(listed.choosable(), ["api"]);
    }

    #[tokio::test]
    async fn a_worktree_of_a_composed_project_checks_out_where_each_really_is() {
        let tmp = tempfile::tempdir().unwrap();
        let tmp = &std::fs::canonicalize(tmp.path()).unwrap();
        repository(&tmp.join("code/api")).await;
        repository(&tmp.join("work/web-app")).await;
        let repos = repos(tmp);
        let made = repos
            .compose_project(
                "Acme",
                None,
                &[
                    member(&tmp.join("code/api")),
                    ProjectMember {
                        path: tmp.join("work/web-app").to_string_lossy().into_owned(),
                        name: Some("web".into()),
                    },
                ],
            )
            .await
            .unwrap();
        let project = PathBuf::from(&made.path);
        std::fs::write(project.join("AGENTS.md"), "be kind\n").unwrap();
        let worktree = repos
            .create_project_worktree(&project, &["api".into(), "web".into()], "HEAD")
            .await
            .unwrap();
        let folder = PathBuf::from(&worktree.worktree.path);
        // Real worktrees in it, of the real repositories; the project's own
        // file linked; its list of included repositories not.
        for name in ["api", "web"] {
            let at = folder.join(name);
            assert!(!is_link(&at) && at.join(".git").is_file(), "{name}");
        }
        assert!(is_link(&folder.join("AGENTS.md")));
        assert!(!folder.join(MANIFEST).exists());
        assert_eq!(
            worktree.repositories[1].repo_path,
            tmp.join("work/web-app").to_string_lossy()
        );
        assert!(repos.is_project_worktree_of(&folder, &project).await);
        repos.delete_project_worktree(&folder).await.unwrap();
        assert!(tmp.join("work/web-app/README.md").exists());
    }
}
