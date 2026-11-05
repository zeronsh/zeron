use super::*;

const PATH: &str = "/usr/bin:/bin:/opt/homebrew/bin:/usr/local/bin";

fn allowed(names: &[&str]) -> Vec<String> {
    names.iter().map(|s| s.to_string()).collect()
}

fn s(args: &[&str]) -> Vec<String> {
    args.iter().map(|a| a.to_string()).collect()
}

fn have(program: &str) -> bool {
    find(program, PATH).is_some()
}

#[test]
fn it_runs_only_what_the_manifest_names() {
    let ran = run_on(&json!({ "program": "echo", "args": ["hi", "$HOME", "a b"] }), &allowed(&["echo"]), PATH, &|_| false).unwrap();
    // (No shell: `$HOME` and the space reach it as they are.)
    assert_eq!((ran["status"].as_i64(), ran["stdout"].as_str()), (Some(0), Some("hi $HOME a b\n")));
    assert!(run_on(&json!({ "program": "ls" }), &allowed(&["echo"]), PATH, &|_| false).unwrap_err().contains("doesn't run"));
    for bad in ["/bin/echo", "../echo", "echo;ls", ""] {
        assert!(run_on(&json!({ "program": bad }), &allowed(&[bad]), PATH, &|_| false).is_err(), "{bad}");
    }
}

#[test]
fn it_takes_stdin_a_folder_and_a_timeout() {
    let ran = run_on(&json!({ "program": "cat", "stdin": "piped in" }), &allowed(&["cat"]), PATH, &|_| false).unwrap();
    assert_eq!(ran["stdout"], "piped in");
    let ran = run_on(&json!({ "program": "pwd", "cwd": "/tmp" }), &allowed(&["pwd"]), PATH, &|_| false).unwrap();
    assert!(ran["stdout"].as_str().unwrap().trim_end().ends_with("tmp"), "{ran}");
    assert!(run_on(&json!({ "program": "pwd", "cwd": "relative" }), &allowed(&["pwd"]), PATH, &|_| false).is_err());
    let ran = run_on(&json!({ "program": "sleep", "args": ["5"], "timeout": 0.2 }), &allowed(&["sleep"]), PATH, &|_| false).unwrap();
    assert_eq!(ran["status"].as_i64(), Some(-1));
    let ran = run_on(&json!({ "program": "ls", "args": ["/no/such/place"] }), &allowed(&["ls"]), PATH, &|_| false).unwrap();
    assert!(ran["status"].as_i64().unwrap() != 0 && !ran["stderr"].as_str().unwrap().is_empty(), "{ran}");
    assert!(run_on(&json!({ "program": "no-such-program-zeron" }), &allowed(&["no-such-program-zeron"]), PATH, &|_| false)
        .unwrap_err()
        .contains("isn't installed"));
}

/// R2: the environment is an allow-list: harmless words and formatting,
/// and the plugin's own `ZERON_PLUGIN_…`, never what changes what runs.
#[test]
fn it_sets_only_harmless_variables() {
    let ran = run_on(
        &json!({ "program": "printenv", "args": ["ZERON_PLUGIN_WORD"], "env": { "ZERON_PLUGIN_WORD": "hello", "LC_ALL": "C" } }),
        &allowed(&["printenv"]),
        PATH,
        &|_| false,
    )
    .unwrap();
    assert_eq!(ran["stdout"], "hello\n");
    for bad in [
        json!({ "PATH": "/tmp" }),
        json!({ "DYLD_INSERT_LIBRARIES": "/tmp/x.dylib" }),
        json!({ "LD_PRELOAD": "/tmp/x.so" }),
        json!({ "GIT_SSH_COMMAND": "touch /tmp/x" }),
        json!({ "GIT_CONFIG_COUNT": "1" }),
        json!({ "GIT_CONFIG_PARAMETERS": "'alias.x=!sh'" }),
        json!({ "GIT_EXEC_PATH": "/tmp" }),
        json!({ "GIT_EXTERNAL_DIFF": "/tmp/x" }),
        json!({ "GIT_DIR": "/tmp" }),
        json!({ "BASH_ENV": "/tmp/x" }),
        json!({ "ENV": "/tmp/x" }),
        json!({ "NODE_OPTIONS": "--require /tmp/x.js" }),
        json!({ "PERL5OPT": "-Mx" }),
        json!({ "PYTHONPATH": "/tmp" }),
        json!({ "PYTHONSTARTUP": "/tmp/x" }),
        json!({ "GH_CONFIG_DIR": "/tmp" }),
        json!({ "GH_HOST": "evil.example" }),
        json!({ "EDITOR": "touch /tmp/x" }),
        json!({ "PAGER": "touch /tmp/x" }),
        json!({ "HOME": "/tmp" }),
        json!({ "ZERON_PLUGIN_": "x" }),
        json!({ "ZERON_IPC_TOKEN_FILE": "/tmp/x" }),
        json!({ "lower": "x" }),
        json!({ "1X": "x" }),
        json!({ "LANG": 3 }),
        json!(["LANG"]),
    ] {
        assert!(run_on(&json!({ "program": "printenv", "env": bad }), &allowed(&["printenv"]), PATH, &|_| false).is_err(), "{bad}");
    }
}

/// R2 (the review's PoC): `git -c 'alias.pwn=!touch X' pwn` ran a shell
/// command, and so did `GIT_SSH_COMMAND=touch Y git ls-remote ssh://…`.
#[test]
fn git_config_and_environment_dont_run_code() {
    if !have("git") {
        eprintln!("(git isn't installed: skipped)");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let x = dir.path().join("X");
    let y = dir.path().join("Y");
    let git = allowed(&["git"]);
    let alias = format!("alias.pwn=!touch {}", x.display());
    let refused = run_on(&json!({ "program": "git", "args": ["-c", alias, "pwn"] }), &git, PATH, &|_| false);
    assert!(refused.is_err(), "{refused:?}");
    let ssh = run_on(
        &json!({ "program": "git", "args": ["ls-remote", "ssh://example.invalid/x"], "env": { "GIT_SSH_COMMAND": format!("touch {}", y.display()) } }),
        &git,
        PATH,
        &|_| false,
    );
    assert!(ssh.is_err(), "{ssh:?}");
    // Its other routes.
    let z = dir.path().join("Z");
    for args in [
        json!(["-c", format!("core.sshCommand=touch {}", z.display()), "ls-remote", "ssh://example.invalid/x"]),
        json!(["-c", format!("core.fsmonitor=touch {}", z.display()), "status"]),
        json!(["--config-env=core.fsmonitor=HOME", "status"]),
        json!(["--exec-path=/tmp", "status"]),
        json!(["-p", "log"]),
        json!(["ls-remote", format!("--upload-pack=touch {}; git-upload-pack", z.display()), "."]),
        json!(["ls-remote", "--upload-pa", format!("touch {}", z.display()), "."]),
        json!(["ls-remote", "-u", format!("touch {}", z.display()), "."]),
        json!(["fetch", format!("--upload-pack=touch {}", z.display()), "."]),
        json!(["clone", "-c", format!("core.fsmonitor=touch {}", z.display()), ".", "y"]),
        json!(["clone", "--config", "filter.x.smudge=sh", ".", "y"]),
        json!(["clone", "-qc", "core.fsmonitor=sh", ".", "y"]),
        json!(["clone", "--template=/tmp/t", ".", "y"]),
        json!(["log", "-1", "--format=[core]%n\tfsmonitor = sh", format!("--output={}", z.display())]),
        json!(["log", "--outp", z.display().to_string()]),
        json!(["push", "--receive-pack=sh", "origin"]),
        json!(["rebase", "--exec", "sh", "main"]),
        json!(["rebase", "-x", "sh", "main"]),
        json!(["grep", "-O", "sh", "x"]),
        json!(["bisect", "run", "sh"]),
        json!(["submodule", "foreach", "sh"]),
        json!(["config", "--global", "alias.x", "!sh"]),
        json!(["config", "core.fsmonitor", "sh"]),
        json!(["config", "set", "core.fsmonitor", "sh"]),
        json!(["difftool", "--extcmd=sh"]),
        json!(["credential", "fill"]),
        json!(["help", "-w", "log"]),
        json!(["maintenance", "start"]),
        json!(["pwn"]),
        json!(["lfs", "x"]),
    ] {
        let ran = run_on(&json!({ "program": "git", "args": args, "cwd": dir.path() }), &git, PATH, &|_| false);
        assert!(ran.is_err(), "git {args} ran: {ran:?}");
    }
    assert!(!x.exists() && !y.exists() && !z.exists(), "git ran a command");
}

/// R2: a repository whose own config runs things (an fsmonitor, hooks)
/// runs none of them for a plugin's git: every run is hardened.
#[test]
fn a_planted_repository_runs_nothing() {
    if !have("git") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    let git = allowed(&["git"]);
    let ok = |args: Json| {
        let ran = run_on(&json!({ "program": "git", "args": args, "cwd": repo }), &git, PATH, &|_| false).unwrap();
        assert_eq!(ran["status"], 0, "{ran}");
        ran
    };
    std::fs::create_dir(&repo).unwrap();
    ok(json!(["init", "-q"]));
    let fsmonitor = dir.path().join("fsmonitor-ran");
    let hook = dir.path().join("hook-ran");
    let config = repo.join(".git/config");
    let mut text = std::fs::read_to_string(&config).unwrap();
    text.push_str(&format!("[core]\n\tfsmonitor = \"touch {}; false\"\n", fsmonitor.display()));
    std::fs::write(&config, text).unwrap();
    let hooks = repo.join(".git/hooks");
    std::fs::create_dir_all(&hooks).unwrap();
    std::fs::write(hooks.join("pre-commit"), format!("#!/bin/sh\ntouch {}\n", hook.display())).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(hooks.join("pre-commit"), std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    std::fs::write(repo.join("a.txt"), "a").unwrap();
    ok(json!(["status", "--porcelain"]));
    ok(json!(["add", "a.txt"]));
    ok(json!(["-c", "user.name=T", "-c", "user.email=t@example.invalid", "commit", "-q", "-m", "a"]));
    assert!(!fsmonitor.exists(), "the repository's fsmonitor ran");
    assert!(!hook.exists(), "the repository's hook ran");
}

#[test]
fn git_as_plugins_use_it_still_runs() {
    let cwd = Path::new("/work/repo");
    for args in [
        s(&["status", "--porcelain=v2", "--branch"]),
        s(&["-C", "/work/repo/sub", "log", "--oneline"]),
        s(&["-C", "sub", "status"]),
        s(&["-c", "core.quotepath=off", "-c", "color.ui=false", "-c", "core.pager=cat", "diff", "--no-ext-diff"]),
        s(&["switch", "-c", "feature"]),
        s(&["log", "-c", "--format=%H"]),
        s(&["config", "--get", "user.email"]),
        s(&["config", "get", "user.email"]),
        s(&["rev-parse", "--git-dir", "--show-toplevel"]),
        s(&["fetch", "-u", "origin"]),
        s(&["format-patch", "--to=a@example.com", "-1"]),
        s(&["log", "--", "--output=not-an-option"]),
        s(&["--no-optional-locks", "status"]),
        s(&["--version"]),
    ] {
        assert!(git_args(&args, Some(cwd)).is_ok(), "{args:?}: {:?}", git_args(&args, Some(cwd)));
    }
    // `-C` outside the run's folder; relative without one.
    for (args, cwd) in [
        (s(&["-C", "/etc", "status"]), Some(cwd)),
        (s(&["-C", "../other", "status"]), Some(cwd)),
        (s(&["--git-dir=/elsewhere/.git", "status"]), Some(cwd)),
        (s(&["-C", "relative", "status"]), None),
        (s(&["-c", "core.pager=less", "log"]), Some(cwd)),
        (s(&["-c", "pager.log=sh", "log"]), Some(cwd)),
    ] {
        assert!(git_args(&args, cwd).is_err(), "{args:?}");
    }
    assert!(git_args(&s(&["-C", "/anywhere", "status"]), None).is_ok(), "-C is the folder when a run has none");
}

#[test]
fn a_git_by_another_name_is_still_git() {
    if !have("git") || !cfg!(target_os = "macos") {
        return;
    }
    // (macOS finds `GIT` as git: the rules follow the program, not its spelling.)
    let ran = run_on(&json!({ "program": "GIT", "args": ["-c", "alias.x=!true", "x"] }), &allowed(&["GIT"]), PATH, &|_| false);
    assert!(ran.is_err(), "{ran:?}");
}

#[test]
fn an_index_of_the_plugins_own_is_a_git_folders() {
    let dir = tempfile::tempdir().unwrap();
    let git_dir = dir.path().join(".git");
    std::fs::create_dir(&git_dir).unwrap();
    std::fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();
    let index = git_dir.join("zeron-git.index").display().to_string();
    assert!(git_index_path(&index));
    assert!(git_args(&s(&["read-tree", &format!("--index-output={index}"), "-m", "HEAD"]), Some(dir.path())).is_ok());
    for bad in [
        dir.path().join("zeron-git.index").display().to_string(),
        git_dir.join("config").display().to_string(),
        "relative.index".into(),
        format!("{}/../x.index", git_dir.display()),
    ] {
        assert!(!git_index_path(&bad), "{bad}");
        assert!(git_args(&s(&["read-tree", &format!("--index-output={bad}"), "HEAD"]), Some(dir.path())).is_err(), "{bad}");
        assert!(env_of(&json!({ "env": { "GIT_INDEX_FILE": bad } })).is_err());
    }
    assert!(env_of(&json!({ "env": { "GIT_INDEX_FILE": index } })).is_ok());
}

/// R2: gh never hands a plugin its token, an alias or an extension.
#[test]
fn gh_never_gives_a_token_or_runs_a_program() {
    for args in [
        s(&["auth", "token"]),
        s(&["auth", "status", "--show-token"]),
        s(&["auth", "status", "-t"]),
        s(&["auth", "status", "-ht", "github.com"]),
        s(&["auth", "login", "--with-token"]),
        s(&["auth", "git-credential", "get"]),
        s(&["alias", "set", "--shell", "pwn", "touch X"]),
        s(&["extension", "install", "evil/gh-evil"]),
        s(&["ext", "install", "evil/gh-evil"]),
        s(&["codespace", "ssh", "--", "-oProxyCommand=sh"]),
        s(&["config", "set", "browser", "sh"]),
        s(&["config", "get", "oauth_token"]),
        s(&["repo", "clone", "a/b", "--", "--config", "core.fsmonitor=sh"]),
        s(&["pwn"]),
        s(&["-R", "a/b", "alias", "set", "x", "y"]),
    ] {
        assert!(gh_args(&args).is_err(), "{args:?}");
    }
    for args in [
        s(&["api", "graphql", "--input", "-"]),
        s(&["api", "--method", "GET", "/user"]),
        s(&["repo", "view", "--json", "nameWithOwner", "--jq", ".nameWithOwner"]),
        s(&["auth", "status"]),
        s(&["config", "get", "git_protocol"]),
        s(&["pr", "list", "-R", "a/b"]),
        s(&["--version"]),
    ] {
        assert!(gh_args(&args).is_ok(), "{args:?}");
    }
    if have("gh") {
        let ran = run_on(&json!({ "program": "gh", "args": ["auth", "token"] }), &allowed(&["gh"]), PATH, &|_| false);
        assert!(ran.is_err(), "{ran:?}");
    }
}

#[test]
fn risk_says_what_can_do_anything() {
    for anything in ["git", "gh", "sh", "bash", "zsh", "python3", "python3.12", "node", "osascript", "perl", "ruby", "GIT", "git.exe", "rg", "curl", "find", "some-tool"] {
        assert_eq!(risk(anything), Risk::Anything, "{anything}");
    }
    for contained in ["date", "uname", "jq", "cat", "echo", "sw_vers", "Date"] {
        assert_eq!(risk(contained), Risk::Contained, "{contained}");
    }
    assert!(any_can_do_anything(&allowed(&["date", "gh"])));
    assert!(!any_can_do_anything(&allowed(&["date", "jq"])));
}

/// In a checkout of yours, a commit runs your hooks (pre-commit, as git
/// always does); reading never does, nor does anything elsewhere, and the
/// filesystem monitor is off for all of them.
#[test]
fn your_checkouts_hooks_run_on_a_commit_and_nowhere_else() {
    if !have("git") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    let git = allowed(&["git"]);
    let known = repo.clone();
    let trusted = move |p: &Path| p.starts_with(&known);
    let ran = |args: Json, trusting: bool| {
        let asked = json!({ "program": "git", "args": args, "cwd": repo });
        let ran = if trusting { run_on(&asked, &git, PATH, &trusted) } else { run_on(&asked, &git, PATH, &|_| false) }.unwrap();
        assert_eq!(ran["status"], 0, "{ran}");
    };
    ran(json!(["init", "-q"]), true);
    let hook = dir.path().join("hook-ran");
    let fsmonitor = dir.path().join("fsmonitor-ran");
    let hooks = repo.join(".git/hooks");
    std::fs::write(hooks.join("pre-commit"), format!("#!/bin/sh\ntouch {}\n", hook.display())).unwrap();
    std::fs::write(hooks.join("post-checkout"), format!("#!/bin/sh\ntouch {}\n", hook.display())).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        for h in ["pre-commit", "post-checkout"] {
            std::fs::set_permissions(hooks.join(h), std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
    let config = repo.join(".git/config");
    let mut text = std::fs::read_to_string(&config).unwrap();
    text.push_str(&format!("[core]\n\tfsmonitor = \"touch {}; false\"\n", fsmonitor.display()));
    std::fs::write(&config, text).unwrap();
    std::fs::write(repo.join("a.txt"), "a").unwrap();
    let who = ["-c", "user.name=T", "-c", "user.email=t@example.invalid"];
    // Reading, in your checkout: nothing runs.
    ran(json!(["status", "--porcelain"]), true);
    ran(json!(["add", "a.txt"]), true);
    assert!(!hook.exists() && !fsmonitor.exists());
    // A commit outside what's trusted: no hook.
    ran(json!([who[0], who[1], who[2], who[3], "commit", "-q", "-m", "a"]), false);
    assert!(!hook.exists(), "a hook ran outside your checkouts");
    // A commit in your checkout: your pre-commit runs.
    std::fs::write(repo.join("a.txt"), "b").unwrap();
    ran(json!(["add", "a.txt"]), true);
    ran(json!([who[0], who[1], who[2], who[3], "commit", "-q", "-m", "b"]), true);
    assert!(hook.exists(), "your pre-commit didn't run");
    assert!(!fsmonitor.exists(), "the filesystem monitor ran");
    // Which runs keep hooks, by command and place.
    let cwd = Some(Path::new("/work/repo"));
    let mine = |p: &Path| p.starts_with("/work/repo");
    assert!(git_args(&s(&["commit", "-m", "x"]), cwd).unwrap().hooks(&mine));
    assert!(git_args(&s(&["-C", "sub", "push"]), cwd).unwrap().hooks(&mine));
    assert!(!git_args(&s(&["status"]), cwd).unwrap().hooks(&mine));
    assert!(!git_args(&s(&["diff"]), cwd).unwrap().hooks(&mine));
    assert!(!git_args(&s(&["--git-dir=/work/repo/.git", "commit"]), cwd).unwrap().hooks(&mine));
    assert!(!git_args(&s(&["-C", "/elsewhere", "commit"]), None).unwrap().hooks(&mine));
    assert!(git_args(&s(&["-C", "/work/repo", "commit"]), None).unwrap().hooks(&mine));
}
