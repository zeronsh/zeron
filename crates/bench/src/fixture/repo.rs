//! Synthetic repositories used as project roots by fixture engines.
use anyhow::{Result, ensure};
use std::{fs, path::Path, process::Command};

fn git(path: &Path, args: &[&str]) -> Result<()> {
    let status = Command::new("/usr/bin/git")
        .current_dir(path)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", path)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "zeron-bench")
        .env("GIT_AUTHOR_EMAIL", "bench@example.invalid")
        .env("GIT_COMMITTER_NAME", "zeron-bench")
        .env("GIT_COMMITTER_EMAIL", "bench@example.invalid")
        .env("GIT_AUTHOR_DATE", "2000-01-01T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2000-01-01T00:00:00Z")
        .args(args)
        .status()?;
    ensure!(status.success(), "git {:?} failed", args);
    Ok(())
}

pub fn create(path: &Path, dirty: bool) -> Result<()> {
    fs::create_dir(path)?;
    git(path, &["init", "-q"])?;
    for n in 0..200 {
        fs::write(
            path.join(format!("file-{n:03}.txt")),
            format!("fixture file {n}\n"),
        )?;
    }
    git(path, &["add", "."])?;
    git(path, &["commit", "-q", "-m", "Fixture 0"])?;
    for n in 1..50 {
        let name = format!("file-{n:03}.txt");
        fs::write(path.join(&name), format!("fixture revision {n}\n"))?;
        git(path, &["add", &name])?;
        git(path, &["commit", "-q", "-m", &format!("Fixture {n}")])?;
    }
    if dirty {
        for n in 0..30 {
            fs::write(
                path.join(format!("file-{n:03}.txt")),
                format!("dirty fixture {n}\n"),
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn repository_has_history_and_dirty_files() {
        let path = std::env::temp_dir().join(format!("zb-repo-{}", crate::mono_now()));
        create(&path, true).unwrap();
        let commits = Command::new("/usr/bin/git")
            .current_dir(&path)
            .args(["rev-list", "--count", "HEAD"])
            .output()
            .unwrap();
        assert!(commits.status.success());
        assert_eq!(String::from_utf8(commits.stdout).unwrap().trim(), "50");
        let status = Command::new("/usr/bin/git")
            .current_dir(&path)
            .args(["status", "--porcelain"])
            .output()
            .unwrap();
        assert!(status.status.success());
        assert_eq!(
            String::from_utf8(status.stdout).unwrap().lines().count(),
            30
        );
        fs::remove_dir_all(path).unwrap();
    }
}
