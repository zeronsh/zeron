//! Credit Zeron in the commits its agents make: a
//! `Co-authored-by: zeronsh[bot]` trailer, which GitHub counts as a
//! contribution by the Zeron app.
//!
//! Agents commit with `git` themselves (every harness, any repository), so
//! the trailer comes from a git hook that only the agents' processes see:
//! their environment points `core.hooksPath` at a folder of wrappers
//! (`GIT_CONFIG_COUNT` config, appended to any the user already sets). Each
//! wrapper runs the repository's own hook — its `core.hooksPath` from the
//! repository or user config, else `.git/hooks` — and `commit-msg` then adds
//! the trailer once that hook passed. Commits made by hand never see it.
//!
//! Always on for a Cloud machine (Zeron's own); on a user's device while its
//! setting is on (off by default: the user's own repositories).

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use serde::{Deserialize, Serialize};

use crate::EngineError;

/// The GitHub App's bot account (`zeronsh[bot]`, user id 346184242): its
/// noreply address is what GitHub resolves a co-author trailer to.
pub const TRAILER: &str =
    "Co-authored-by: zeronsh[bot] <346184242+zeronsh[bot]@users.noreply.github.com>";

/// Hooks the wrappers stand in for. `reference-transaction` and
/// `post-index-change` are left out: they run on every ref or index write,
/// and a wrapper there would slow every git command an agent runs.
const HOOKS: &[&str] = &[
    "applypatch-msg",
    "pre-applypatch",
    "post-applypatch",
    "pre-commit",
    "pre-merge-commit",
    "prepare-commit-msg",
    "commit-msg",
    "post-commit",
    "pre-rebase",
    "post-checkout",
    "post-merge",
    "pre-push",
    "post-rewrite",
    "pre-auto-gc",
    "push-to-checkout",
    "sendemail-validate",
];

/// `{ "enabled": bool }` — the device's setting, and whether this is a Cloud
/// machine (always on there).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitCreditStatus {
    pub enabled: bool,
    pub cloud: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Stored {
    enabled: bool,
}

/// Off by default on a user's device: their own repositories.
impl Default for Stored {
    fn default() -> Self {
        Self { enabled: false }
    }
}

pub struct CommitCredit {
    settings_path: PathBuf,
    hooks_dir: PathBuf,
    cloud: bool,
    stored: Mutex<Stored>,
}

impl CommitCredit {
    pub fn open(data_dir: &Path, cloud: bool) -> Self {
        let settings_path = data_dir.join("commit-credit.json");
        let stored = std::fs::read(&settings_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self {
            settings_path,
            hooks_dir: data_dir.join("git-hooks"),
            cloud,
            stored: Mutex::new(stored),
        }
    }

    pub fn status(&self) -> CommitCreditStatus {
        CommitCreditStatus {
            enabled: self.cloud
                || self
                    .stored
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .enabled,
            cloud: self.cloud,
        }
    }

    pub fn set(&self, enabled: bool) -> Result<CommitCreditStatus, EngineError> {
        if self.cloud {
            return Err(EngineError::Other(
                "Cloud machines always credit Zeron in their commits.".into(),
            ));
        }
        let next = Stored { enabled };
        let parent = self
            .settings_path
            .parent()
            .ok_or_else(|| EngineError::Other("settings have no folder".into()))?;
        std::fs::create_dir_all(parent)?;
        let mut temp = tempfile::NamedTempFile::new_in(parent)?;
        serde_json::to_writer_pretty(&mut temp, &next)
            .map_err(|e| EngineError::Other(e.to_string()))?;
        temp.write_all(b"\n")?;
        temp.persist(&self.settings_path)
            .map_err(|e| EngineError::Io(e.error))?;
        *self.stored.lock().unwrap_or_else(PoisonError::into_inner) = next;
        Ok(self.status())
    }

    /// Point an agent's git at the hook wrappers when crediting is on. The
    /// wrappers are (re)written first, so a deleted folder heals; a failure
    /// leaves the run as it was rather than failing it.
    pub fn apply(&self, env: &mut BTreeMap<String, String>) {
        if !self.status().enabled {
            return;
        }
        if let Err(err) = write_hooks(&self.hooks_dir) {
            tracing::warn!(error = %err, "commit credit: writing git hooks failed");
            return;
        }
        add_git_config(env, "core.hooksPath", &self.hooks_dir.to_string_lossy());
    }
}

/// Append `key = value` to the git config an environment carries
/// (`GIT_CONFIG_COUNT` / `GIT_CONFIG_KEY_n` / `GIT_CONFIG_VALUE_n`), after
/// any the run or the engine's own environment already set.
fn add_git_config(env: &mut BTreeMap<String, String>, key: &str, value: &str) {
    let count = env
        .get("GIT_CONFIG_COUNT")
        .cloned()
        .or_else(|| std::env::var("GIT_CONFIG_COUNT").ok())
        .and_then(|count| count.trim().parse::<usize>().ok())
        .unwrap_or(0);
    // Entries the engine inherited must ride along under the new count.
    for n in 0..count {
        for name in [
            format!("GIT_CONFIG_KEY_{n}"),
            format!("GIT_CONFIG_VALUE_{n}"),
        ] {
            if !env.contains_key(&name)
                && let Ok(value) = std::env::var(&name)
            {
                env.insert(name, value);
            }
        }
    }
    env.insert(format!("GIT_CONFIG_KEY_{count}"), key.to_string());
    env.insert(format!("GIT_CONFIG_VALUE_{count}"), value.to_string());
    env.insert("GIT_CONFIG_COUNT".into(), (count + 1).to_string());
}

/// The wrapper every hook name runs: the repository's own hook first, then
/// (for `commit-msg`) the trailer.
fn wrapper_script() -> String {
    format!(
        r#"#!/bin/sh
# Zeron: run this repository's own hook, then credit Zeron in the commit.
# Written by the Zeron engine (commit_credit.rs); safe to delete.
name=$(basename "$0")
here=$(cd "$(dirname "$0")" && pwd -P)
common=$(git rev-parse --path-format=absolute --git-common-dir 2>/dev/null)
# The hooks path the repository or user configured (not ours, which only
# this process's environment carries), else the repository's hooks folder.
dir=$(git config --file "$common/config" core.hooksPath 2>/dev/null \
    || git config --global core.hooksPath 2>/dev/null \
    || git config --system core.hooksPath 2>/dev/null)
case "$dir" in
    "") dir="$common/hooks" ;;
    "~/"*) dir="$HOME/${{dir#\~/}}" ;;
esac
original="$dir/$name"
if [ -x "$original" ] && [ "$(cd "$dir" 2>/dev/null && pwd -P)" != "$here" ]; then
    if [ "$name" = commit-msg ]; then
        "$original" "$@" || exit $?
    else
        exec "$original" "$@"
    fi
fi
if [ "$name" = commit-msg ] && [ -n "$1" ]; then
    git interpret-trailers --in-place --if-exists addIfDifferent \
        --trailer "{TRAILER}" "$1" 2>/dev/null || true
fi
exit 0
"#
    )
}

fn write_hooks(dir: &Path) -> std::io::Result<()> {
    let script = wrapper_script();
    std::fs::create_dir_all(dir)?;
    for name in HOOKS {
        let path = dir.join(name);
        if std::fs::read_to_string(&path).is_ok_and(|current| current == script) {
            continue;
        }
        let mut temp = tempfile::NamedTempFile::new_in(dir)?;
        temp.write_all(script.as_bytes())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            temp.as_file()
                .set_permissions(std::fs::Permissions::from_mode(0o755))?;
        }
        temp.persist(&path).map_err(|e| e.error)?;
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(dir: &Path, env: &BTreeMap<String, String>, args: &[&str]) -> std::process::Output {
        let out = Command::new("git")
            .current_dir(dir)
            .envs(env)
            .env("GIT_AUTHOR_NAME", "T")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "T")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .env("HOME", dir)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(args)
            .output()
            .unwrap();
        out
    }

    fn message(dir: &Path) -> String {
        let out = git(dir, &BTreeMap::new(), &["log", "-1", "--format=%B"]);
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    #[test]
    fn agent_commits_credit_zeron_and_keep_the_repositorys_hooks() {
        let data = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        let none = BTreeMap::new();
        assert!(git(repo.path(), &none, &["init", "-q"]).status.success());
        // The repository's own commit-msg hook: it must still run (and see
        // the message before the trailer), and can still reject.
        let hook = repo.path().join(".git/hooks/commit-msg");
        std::fs::write(&hook, "#!/bin/sh\ngrep -q reject \"$1\" && exit 1\necho ran >> \"$(git rev-parse --git-dir)/hook-ran\"\n").unwrap();
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();

        let credit = CommitCredit::open(data.path(), false);
        credit.set(true).unwrap();
        let mut env = BTreeMap::new();
        credit.apply(&mut env);
        assert_eq!(
            env.get("GIT_CONFIG_KEY_0").map(String::as_str),
            Some("core.hooksPath")
        );

        assert!(
            git(
                repo.path(),
                &env,
                &["commit", "-q", "--allow-empty", "-m", "Add a thing"]
            )
            .status
            .success()
        );
        let first = message(repo.path());
        assert!(first.contains(TRAILER), "{first}");
        assert!(
            repo.path().join(".git/hook-ran").exists(),
            "the repository's hook ran"
        );

        // An amend keeps one trailer, not two.
        assert!(
            git(
                repo.path(),
                &env,
                &["commit", "-q", "--amend", "--allow-empty", "--no-edit"]
            )
            .status
            .success()
        );
        assert_eq!(message(repo.path()).matches(TRAILER).count(), 1);

        // The repository's hook still rejects.
        assert!(
            !git(
                repo.path(),
                &env,
                &["commit", "-q", "--allow-empty", "-m", "reject me"]
            )
            .status
            .success()
        );

        // A commit made by hand (no agent environment) is left alone.
        assert!(
            git(
                repo.path(),
                &none,
                &["commit", "-q", "--allow-empty", "-m", "By hand"]
            )
            .status
            .success()
        );
        assert!(!message(repo.path()).contains("zeronsh"));
    }

    #[test]
    fn the_device_setting_is_off_by_default_and_always_on_for_cloud() {
        let data = tempfile::tempdir().unwrap();
        let credit = CommitCredit::open(data.path(), false);
        assert!(
            !credit.status().enabled,
            "off by default on a user's device"
        );
        credit.set(true).unwrap();
        assert!(
            CommitCredit::open(data.path(), false).status().enabled,
            "persisted"
        );
        credit.set(false).unwrap();
        let mut env = BTreeMap::new();
        credit.apply(&mut env);
        assert!(env.is_empty());
        // Persisted.
        assert!(!CommitCredit::open(data.path(), false).status().enabled);

        let cloud = CommitCredit::open(data.path(), true);
        assert!(cloud.status().enabled, "always on for Cloud");
        assert!(cloud.set(false).is_err());
    }

    #[test]
    fn existing_git_config_from_the_environment_is_kept() {
        let mut env = BTreeMap::from([
            ("GIT_CONFIG_COUNT".to_string(), "1".to_string()),
            ("GIT_CONFIG_KEY_0".to_string(), "user.name".to_string()),
            ("GIT_CONFIG_VALUE_0".to_string(), "Agent".to_string()),
        ]);
        add_git_config(&mut env, "core.hooksPath", "/hooks");
        assert_eq!(env["GIT_CONFIG_COUNT"], "2");
        assert_eq!(env["GIT_CONFIG_KEY_0"], "user.name");
        assert_eq!(env["GIT_CONFIG_KEY_1"], "core.hooksPath");
        assert_eq!(env["GIT_CONFIG_VALUE_1"], "/hooks");
    }
}
