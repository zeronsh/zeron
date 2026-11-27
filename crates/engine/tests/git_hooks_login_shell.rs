//! End-to-end: a git hook that needs a tool only the login shell's PATH has
//! still runs when the engine creates a worktree. A Dock launch hands the
//! engine launchd's bare PATH; `git lfs install`'s global post-checkout hook
//! calls `git-lfs` from Homebrew, so `git worktree add` used to check out the
//! tree, fail the hook, and leave the new chat unable to start.
//!
//! This file must stay a single test: it mutates process env (SHELL/PATH/HOME)
//! and warms the process-global login-shell snapshot cache, so it needs its
//! own test binary with no parallel siblings.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

fn write_executable(path: &Path, body: &str) {
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn run_git(cwd: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("git fixture command starts");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
async fn worktree_hook_finds_tools_on_the_login_shell_path() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();

    let repo = root.join("repo");
    std::fs::create_dir(&repo).unwrap();
    run_git(&repo, &["init", "-q", "-b", "main"]);
    run_git(
        &repo,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "init",
        ],
    );
    // The hook git-lfs installs, reduced to its shape: fail unless the tool
    // is on PATH, then call it.
    write_executable(
        &repo.join(".git/hooks/post-checkout"),
        "#!/bin/sh\ncommand -v hook-tool >/dev/null 2>&1 || { echo 'hook-tool was not found on your path' >&2; exit 2; }\nexec hook-tool\n",
    );

    // Only the git shim is directly on PATH (a GUI/service launch).
    let direct_bin = root.join("direct-bin");
    std::fs::create_dir(&direct_bin).unwrap();
    let git_executable = std::env::var_os("PATH")
        .into_iter()
        .flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .map(|path| path.join("git"))
        .find(|path| path.is_file())
        .expect("git available");
    std::os::unix::fs::symlink(&git_executable, direct_bin.join("git")).unwrap();

    // The user's shell puts `hook-tool` on PATH.
    let shell_bin = root.join("shell-bin");
    std::fs::create_dir(&shell_bin).unwrap();
    let marker = root.join("hook-tool-ran");
    write_executable(
        &shell_bin.join("hook-tool"),
        &format!("#!/bin/sh\ntouch \"{}\"\n", marker.display()),
    );
    let fake_shell = root.join("fake-shell");
    write_executable(
        &fake_shell,
        &format!(
            "#!/bin/sh\nPATH=\"{}:/usr/bin:/bin\"; export PATH\n\
             while [ \"$#\" -gt 0 ]; do\n\
               if [ \"$1\" = \"-c\" ]; then shift; exec /bin/sh -c \"$1\"; fi\n\
               shift\n\
             done\nexit 1\n",
            shell_bin.display()
        ),
    );

    // SAFETY: single-test binary — nothing else reads env concurrently.
    // HOME also keeps the machine's global git config (and any global
    // core.hooksPath) out of the test.
    unsafe {
        std::env::set_var("SHELL", &fake_shell);
        std::env::set_var("HOME", &root);
        std::env::set_var("PATH", &direct_bin);
        std::env::remove_var("XDG_CONFIG_HOME");
        std::env::remove_var("ZERON_NO_LOGIN_SHELL");
    }

    let repos = zeron_engine::Repos::with_worktrees_root(
        &root.join("settings"),
        "test-device",
        root.join("worktrees"),
    );
    let worktree = repos
        .create_worktree(&repo, "main")
        .await
        .expect("the hook finds hook-tool through the login-shell PATH");
    assert!(Path::new(&worktree.path).join(".git").is_file());
    assert!(marker.is_file(), "the hook ran hook-tool");
}
