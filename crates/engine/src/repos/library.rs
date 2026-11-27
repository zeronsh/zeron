//! The device's repositories (`ListLocalRepositories`): those found under its
//! home, and those kept on purpose (added, cloned or created through Zeron,
//! `repos.json`), newest worked-in first. Making a new one
//! (`CreateLocalRepository`): a folder with a first commit, and on GitHub too
//! when the device's GitHub CLI is signed in. Forgetting one
//! (`ForgetLocalRepository`) only stops listing it; nothing on disk changes.
//!
//! The look through home is bounded (depth, folders, time) and remembered for
//! a minute; it never goes into a repository (its nested ones are its own),
//! a dot folder, or the folders where dependencies and builds pile up.

use super::project_worktrees::{Marker, marker};
use super::*;
use std::time::Instant;
use zeron_proto::{CreatedRepository, LocalRepositories, LocalRepository};

/// How deep under home a repository is looked for (`~/src/github.com/o/r`).
const SCAN_DEPTH: usize = 4;
/// Folders read at most in one look.
const SCAN_FOLDERS_MAX: usize = 25_000;
/// Repositories listed at most.
const SCAN_REPOSITORIES_MAX: usize = 400;
/// Entries read at most in any one folder.
const SCAN_ENTRIES_MAX: usize = 4_000;
/// How long one look may take.
const SCAN_TIMEOUT: Duration = Duration::from_secs(6);
/// How long a look is trusted before the next listing looks again.
const SCAN_FRESH: Duration = Duration::from_secs(60);
/// How long whether the GitHub CLI is signed in is trusted.
const PUBLISH_FRESH: Duration = Duration::from_secs(5 * 60);
/// Repositories read at once.
const FACTS_CONCURRENCY: usize = 16;
/// Folders never looked in: where an OS keeps its own things, and where
/// dependencies and builds pile up (`go/pkg` is Go's module cache).
const SKIPPED: &[&str] = &[
    "node_modules",
    "Library",
    "Applications",
    "Pictures",
    "Music",
    "Movies",
    "Public",
    "AppData",
    "snap",
    "target",
    "vendor",
    "venv",
    "__pycache__",
    "Pods",
    "DerivedData",
    "bower_components",
];

/// One look through home: the repositories found, nearest first, and
/// whether it stopped short.
#[derive(Debug, Clone, Default)]
pub(crate) struct Scan {
    paths: Vec<PathBuf>,
    truncated: bool,
}

fn scan(home: &Path, skip: &[PathBuf]) -> Scan {
    let started = Instant::now();
    let mut out = Scan::default();
    let mut queue = std::collections::VecDeque::from([(home.to_path_buf(), 0usize)]);
    let mut read = 0usize;
    while let Some((dir, depth)) = queue.pop_front() {
        read += 1;
        if read > SCAN_FOLDERS_MAX || started.elapsed() > SCAN_TIMEOUT {
            out.truncated = true;
            break;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut names: Vec<(String, PathBuf)> = entries
            .take(SCAN_ENTRIES_MAX)
            .filter_map(Result::ok)
            // (A link is never followed: it may lead anywhere, or in a loop.)
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
            .filter_map(|entry| Some((entry.file_name().into_string().ok()?, entry.path())))
            .filter(|(name, _)| !name.starts_with('.') && !SKIPPED.contains(&name.as_str()))
            .collect();
        names.sort();
        for (name, at) in names {
            if skip.iter().any(|skipped| at.starts_with(skipped))
                || (name == "pkg" && dir.file_name().is_some_and(|parent| parent == "go"))
            {
                continue;
            }
            match marker(&at) {
                Marker::Repository => {
                    if out.paths.len() == SCAN_REPOSITORIES_MAX {
                        out.truncated = true;
                        return out;
                    }
                    out.paths.push(at);
                }
                Marker::None if depth + 1 < SCAN_DEPTH => queue.push_back((at, depth + 1)),
                _ => {}
            }
        }
    }
    out
}

/// What a repository's own `.git` folder says, read without running git: its
/// branch (none when detached), whether that has a commit, its `origin`'s
/// GitHub repository, and when it was last worked in. `None` for one whose
/// git dir lives elsewhere (a `gitdir:` file): git is asked instead.
fn read_facts(repository: &Path) -> Option<(Option<String>, bool, Option<String>, Option<i64>)> {
    let git = repository.join(".git");
    if !std::fs::symlink_metadata(&git).is_ok_and(|meta| meta.is_dir()) {
        return None;
    }
    let head = std::fs::read_to_string(git.join("HEAD")).ok()?;
    let head = head.trim();
    let (branch, has_commits) = match head.strip_prefix("ref: refs/heads/") {
        Some(branch) => {
            let reference = format!("refs/heads/{branch}");
            let loose = git.join(&reference).is_file();
            let packed = std::fs::read_to_string(git.join("packed-refs")).is_ok_and(|packed| {
                packed.lines().any(|line| {
                    line.split_once(' ')
                        .is_some_and(|(_, name)| name == reference)
                })
            });
            (Some(branch.to_string()), loose || packed)
        }
        None => (None, head.len() >= 40),
    };
    let github = std::fs::read_to_string(git.join("config"))
        .ok()
        .and_then(|config| origin_url(&config))
        .and_then(|url| parse_github_remote(&url))
        .map(|(owner, name)| format!("{owner}/{name}"));
    let touched = ["index", "HEAD", "logs/HEAD", "FETCH_HEAD"]
        .iter()
        .filter_map(|name| std::fs::metadata(git.join(name)).ok()?.modified().ok())
        .max()
        .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|since| since.as_millis() as i64);
    Some((branch, has_commits, github, touched))
}

/// `[remote "origin"] url = …` in a git config.
fn origin_url(config: &str) -> Option<String> {
    let mut in_origin = false;
    for line in config.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_origin = line.replace(' ', "") == "[remote\"origin\"]";
            continue;
        }
        if in_origin
            && let Some((key, value)) = line.split_once('=')
            && key.trim() == "url"
        {
            return Some(value.trim().to_string());
        }
    }
    None
}

/// Where a new repository goes unless another folder is chosen: the folder
/// holding most of `found` (two or more), else a usual code folder in home,
/// else home.
fn default_parent(home: &Path, found: &[LocalRepository]) -> PathBuf {
    let mut counts: HashMap<PathBuf, (usize, i64)> = HashMap::new();
    for repository in found {
        let Some(parent) = Path::new(&repository.path).parent() else {
            continue;
        };
        if !parent.starts_with(home) || parent == home {
            continue;
        }
        let entry = counts.entry(parent.to_path_buf()).or_default();
        entry.0 += 1;
        entry.1 = entry.1.max(repository.touched_at.unwrap_or(0));
    }
    if let Some((parent, _)) = counts
        .into_iter()
        .filter(|(_, (count, _))| *count >= 2)
        .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(&a.0)))
    {
        return parent;
    }
    [
        "Developer",
        "code",
        "Code",
        "src",
        "dev",
        "projects",
        "Projects",
        "repos",
        "git",
        "work",
    ]
    .iter()
    .map(|name| home.join(name))
    .find(|at| at.is_dir())
    .unwrap_or_else(|| home.to_path_buf())
}

impl Repos {
    fn hidden_path(&self) -> PathBuf {
        self.inner.data_dir.join("repos-hidden.json")
    }

    fn load_hidden(&self) -> Vec<String> {
        std::fs::read_to_string(self.hidden_path())
            .ok()
            .and_then(|raw| serde_json::from_str::<Vec<String>>(&raw).ok())
            .unwrap_or_default()
    }

    fn save_hidden(&self, hidden: &[String]) -> Result<(), EngineError> {
        let json = serde_json::to_string_pretty(hidden)
            .map_err(|e| EngineError::Other(format!("hidden repositories: {e}")))?;
        std::fs::create_dir_all(&self.inner.data_dir)?;
        std::fs::write(self.hidden_path(), json)?;
        Ok(())
    }

    /// Listed again (added, cloned or made where one was forgotten).
    pub(super) fn unhide(&self, path: &str) {
        let mut hidden = self.load_hidden();
        let before = hidden.len();
        hidden.retain(|p| p != path);
        if hidden.len() != before {
            let _ = self.save_hidden(&hidden);
        }
    }

    /// One look through home ([`scan`]), or the last if it's fresh.
    async fn found_repositories(&self, refresh: bool) -> Scan {
        let mut cached = self.inner.found_repositories.lock().await;
        if !refresh
            && let Some((at, scan)) = cached.as_ref()
            && at.elapsed() < SCAN_FRESH
        {
            return scan.clone();
        }
        let home = home_dir();
        // Zeron's own folders hold worktrees and links, not repositories.
        let skip = vec![
            self.inner.worktrees.root(),
            self.projects_root(),
            self.inner.data_dir.clone(),
        ];
        let looked = disposable_worker("local-repositories", move || scan(&home, &skip));
        let found = tokio::time::timeout(SCAN_TIMEOUT + Duration::from_secs(2), looked)
            .await
            .ok()
            .flatten()
            .unwrap_or(Scan {
                paths: Vec::new(),
                truncated: true,
            });
        *cached = Some((Instant::now(), found.clone()));
        found
    }

    /// Whether `gh` is signed in on this device (asked at most every few
    /// minutes; never more than a few seconds).
    async fn can_publish(&self) -> bool {
        if let Some((at, ready)) = *self.inner.can_publish.lock().unwrap()
            && at.elapsed() < PUBLISH_FRESH
        {
            return ready;
        }
        let mut gh = tokio::process::Command::new("gh");
        gh.args(["auth", "status", "--hostname", "github.com"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            gh.as_std_mut().creation_flags(0x08000000);
        }
        let ready = matches!(
            tokio::time::timeout(Duration::from_secs(4), gh.status()).await,
            Ok(Ok(status)) if status.success()
        );
        *self.inner.can_publish.lock().unwrap() = Some((Instant::now(), ready));
        ready
    }

    /// One repository as listed, `None` when it's no longer one.
    async fn local_repository(&self, path: PathBuf, kept: bool) -> Option<LocalRepository> {
        let name = path.file_name()?.to_string_lossy().into_owned();
        let read = path.clone();
        let facts = disposable_worker("repository-facts", move || read_facts(&read))
            .await
            .flatten();
        let (branch, has_commits, github, touched_at) = match facts {
            Some(facts) => facts,
            None => {
                if !Self::path_exists(&path).await || !self.is_repo(&path).await {
                    return None;
                }
                let (branch, head, github) = tokio::join!(
                    self.current_branch(&path),
                    self.head_sha(&path),
                    self.github_repo(&path)
                );
                (
                    branch.ok().filter(|name| name != "HEAD"),
                    matches!(head, Ok(Some(_))),
                    github,
                    None,
                )
            }
        };
        Some(LocalRepository {
            path: path.to_string_lossy().into_owned(),
            name,
            branch,
            has_commits,
            github,
            touched_at,
            kept,
        })
    }

    /// `ListLocalRepositories`: the repositories kept and found here, newest
    /// worked-in first, without those forgotten. `refresh` looks through
    /// home again even when the last look is fresh.
    pub async fn local_repositories(&self, refresh: bool) -> LocalRepositories {
        let (found, can_publish) =
            tokio::join!(self.found_repositories(refresh), self.can_publish());
        let hidden: HashSet<String> = self.load_hidden().into_iter().collect();
        let kept: Vec<String> = self.load_paths();
        let mut seen = HashSet::new();
        let wanted: Vec<(PathBuf, bool)> = kept
            .iter()
            .map(|path| (PathBuf::from(path), true))
            .chain(found.paths.iter().map(|path| (path.clone(), false)))
            .filter(|(path, _)| !hidden.contains(&*path.to_string_lossy()))
            .filter(|(path, _)| seen.insert(path.clone()))
            .collect();
        let mut repositories: Vec<LocalRepository> = stream::iter(wanted)
            .map(|(path, kept)| self.local_repository(path, kept))
            .buffer_unordered(FACTS_CONCURRENCY)
            .filter_map(|repository| async move { repository })
            .collect()
            .await;
        repositories.sort_by(|a, b| {
            b.touched_at
                .cmp(&a.touched_at)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                .then_with(|| a.path.cmp(&b.path))
        });
        let default_parent = default_parent(&home_dir(), &repositories);
        LocalRepositories {
            repositories,
            default_parent: default_parent.to_string_lossy().into_owned(),
            home: home_dir().to_string_lossy().into_owned(),
            can_publish,
            truncated: found.truncated,
        }
    }

    /// `CreateLocalRepository`: a new repository named `name` in `parent`
    /// (where most of the device's are, by default) on `main`, with a
    /// README as its first commit unless `empty`, and on GitHub too
    /// (`publish`: private or not) through the device's GitHub CLI. The
    /// repository stands even when its first commit or its GitHub copy
    /// can't be made; the answer says which (`warning`).
    pub async fn create_local_repository(
        &self,
        name: &str,
        parent: Option<&str>,
        empty: bool,
        publish: Option<bool>,
    ) -> Result<CreatedRepository, EngineError> {
        let clean: String = name
            .trim()
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                    c
                } else {
                    '-'
                }
            })
            .collect();
        let clean = clean.trim_matches(['-', '.']).to_string();
        if clean.is_empty() {
            return Err(EngineError::Other(
                "Name the repository with letters, digits, dots, dashes or underscores".into(),
            ));
        }
        let parent = match parent.map(str::trim).filter(|parent| !parent.is_empty()) {
            Some(parent) => absolutize(Path::new(
                &expand_home(parent).map_err(|error| EngineError::Other(error.into()))?,
            )),
            None => PathBuf::from(self.local_repositories(false).await.default_parent),
        };
        if !parent.is_dir() {
            return Err(EngineError::Other(format!(
                "No such folder: {}",
                parent.display()
            )));
        }
        let target = parent.join(&clean);
        // Claimed by making it: never into a folder already there.
        match std::fs::create_dir(&target) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(EngineError::Other(format!(
                    "Already exists: {}",
                    target.display()
                )));
            }
            Err(error) => return Err(error.into()),
        }
        if let Err(error) = self.init_main(&target).await {
            let _ = std::fs::remove_dir_all(&target);
            return Err(error);
        }
        let mut warning = None;
        let mut committed = false;
        if !empty {
            std::fs::write(target.join("README.md"), format!("# {clean}\n"))?;
            self.git(&["add", "README.md"], Some(&target)).await?;
            match self
                .git(&["commit", "-q", "-m", "Initial commit"], Some(&target))
                .await
            {
                Ok(_) => committed = true,
                Err(error) => {
                    let text = error.to_string();
                    warning = Some(
                        if text.contains("tell me who you are")
                            || text.contains("auto-detect email")
                        {
                            "There's no first commit: git on this device doesn't know your name and email yet (git config --global user.name / user.email)".to_string()
                        } else {
                            format!("There's no first commit: {text}")
                        },
                    );
                }
            }
        }
        if let Some(private) = publish
            && let Err(error) = self.publish(&target, &clean, private, committed).await
        {
            warning = Some(format!("Made here, but not on GitHub: {error}"));
        }
        let path = target.to_string_lossy().into_owned();
        self.register(&path)?;
        self.unhide(&path);
        *self.inner.found_repositories.lock().await = None;
        let repository = self
            .local_repository(target.clone(), true)
            .await
            .ok_or_else(|| {
                EngineError::Other(format!("{} isn't a repository", target.display()))
            })?;
        tracing::info!(path = %path, published = publish.is_some(), "repository created");
        Ok(CreatedRepository {
            repository,
            warning,
        })
    }

    /// `git init` on `main`, on any git (`-b` is 2.28's).
    async fn init_main(&self, at: &Path) -> Result<(), EngineError> {
        if self
            .git(&["init", "-q", "-b", "main"], Some(at))
            .await
            .is_ok()
        {
            return Ok(());
        }
        self.git(&["init", "-q"], Some(at)).await?;
        self.git(&["symbolic-ref", "HEAD", "refs/heads/main"], Some(at))
            .await?;
        Ok(())
    }

    /// `gh repo create` from the folder, its `origin` set (and its commit
    /// pushed, when it has one).
    async fn publish(
        &self,
        at: &Path,
        name: &str,
        private: bool,
        push: bool,
    ) -> Result<(), EngineError> {
        let mut gh = tokio::process::Command::new("gh");
        gh.args(["repo", "create", name])
            .arg(if private { "--private" } else { "--public" })
            .arg("--source")
            .arg(at)
            .args(["--remote", "origin"])
            .current_dir(at)
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true);
        if push {
            gh.arg("--push");
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            gh.as_std_mut().creation_flags(0x08000000);
        }
        let output = tokio::time::timeout(Duration::from_secs(90), gh.output())
            .await
            .map_err(|_| EngineError::Other("GitHub took too long to answer".into()))?
            .map_err(|error| {
                EngineError::Other(format!("the GitHub CLI didn't start ({error})"))
            })?;
        if output.status.success() {
            return Ok(());
        }
        let said = String::from_utf8_lossy(&output.stderr);
        let said = said
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("it refused");
        Err(EngineError::Other(said.to_string()))
    }

    /// `ForgetLocalRepository`: no longer listed here (nor kept). Nothing on
    /// disk changes; adding it again lists it again.
    pub fn forget_local_repository(&self, path: &str) -> Result<(), EngineError> {
        let mut kept = self.load_paths();
        kept.retain(|p| p != path);
        self.save_paths(&kept)?;
        let mut hidden = self.load_hidden();
        if !hidden.iter().any(|p| p == path) {
            hidden.push(path.to_string());
        }
        self.save_hidden(&hidden)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_is_read_from_a_config() {
        let config = "[core]\n\tbare = false\n[remote \"upstream\"]\n\turl = https://github.com/a/b\n[remote \"origin\"]\n\turl = git@github.com:acme/api.git\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n";
        assert_eq!(
            origin_url(config).as_deref(),
            Some("git@github.com:acme/api.git")
        );
        assert_eq!(origin_url("[core]\n"), None);
    }

    #[test]
    fn a_look_through_home_stops_at_repositories_and_skips_the_noise() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        for repository in [
            "code/api",
            "code/web",
            "src/github.com/acme/tool",
            "code/api/nested",
        ] {
            std::fs::create_dir_all(home.join(repository).join(".git")).unwrap();
        }
        for noise in [
            "code/web/node_modules/dep",
            ".config/thing",
            "Library/Caches/repo",
            "go/pkg/mod/github.com/x/y",
            "a/b/c/d/too-deep",
        ] {
            std::fs::create_dir_all(home.join(noise).join(".git")).unwrap();
        }
        let found = scan(home, &[]);
        let mut paths: Vec<_> = found
            .paths
            .iter()
            .map(|path| {
                path.strip_prefix(home)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        paths.sort();
        assert_eq!(paths, ["code/api", "code/web", "src/github.com/acme/tool"]);
    }

    #[test]
    fn a_new_repository_goes_where_most_are() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let at = |path: &str, touched: i64| LocalRepository {
            path: home.join(path).to_string_lossy().into_owned(),
            name: path.rsplit('/').next().unwrap().into(),
            branch: None,
            has_commits: true,
            github: None,
            touched_at: Some(touched),
            kept: false,
        };
        let found = [
            at("code/a", 1),
            at("code/b", 2),
            at("work/c", 9),
            at("work/d", 3),
            at("x/e", 99),
        ];
        // Two places with two each: the one worked in last.
        assert_eq!(default_parent(home, &found), home.join("work"));
        // None twice: a usual folder, else home.
        std::fs::create_dir_all(home.join("code")).unwrap();
        assert_eq!(default_parent(home, &found[..1]), home.join("code"));
        assert_eq!(default_parent(&home.join("empty"), &[]), home.join("empty"));
    }

    #[tokio::test]
    async fn a_new_repository_starts_with_a_commit_on_main() {
        let tmp = tempfile::tempdir().unwrap();
        let repos =
            Repos::with_worktrees_root(&tmp.path().join("data"), "dev", tmp.path().join("wt"));
        std::fs::create_dir_all(tmp.path().join("code")).unwrap();
        let parent = tmp.path().join("code").to_string_lossy().into_owned();
        // (An identity of its own, whatever the machine's.)
        for (key, value) in [
            ("GIT_AUTHOR_NAME", "Test"),
            ("GIT_AUTHOR_EMAIL", "test@example.com"),
            ("GIT_COMMITTER_NAME", "Test"),
            ("GIT_COMMITTER_EMAIL", "test@example.com"),
        ] {
            // SAFETY: the same values wherever another test reads them.
            unsafe { std::env::set_var(key, value) };
        }
        let made = repos
            .create_local_repository("My Tool!", Some(&parent), false, None)
            .await
            .unwrap();
        assert_eq!(made.warning, None);
        assert_eq!(made.repository.name, "My-Tool");
        assert_eq!(made.repository.branch.as_deref(), Some("main"));
        assert!(made.repository.has_commits && made.repository.kept);
        assert!(tmp.path().join("code/My-Tool/README.md").is_file());
        // Not over a folder already there.
        let again = repos
            .create_local_repository("My-Tool", Some(&parent), true, None)
            .await
            .unwrap_err();
        assert!(again.to_string().contains("Already exists"));
        // Forgotten: kept no more, nothing deleted.
        repos
            .forget_local_repository(&made.repository.path)
            .unwrap();
        assert!(!repos.load_paths().contains(&made.repository.path));
        assert!(tmp.path().join("code/My-Tool/.git").is_dir());
    }
}
