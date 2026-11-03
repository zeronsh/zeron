//! A plugin's programs ("commands"): the ones its manifest names
//! (`"commands": ["gh"]`), run as you when it asks (`zeron.run(program,
//! args, { cwd, stdin, timeout, env })`). One runner for the engine's
//! services and the app's windows, so both hold a plugin to the same rules.
//!
//!   { program, args = { … }, cwd?, stdin?, timeout?, env? }
//!     → { status, stdout, stderr }   (status -1: stopped at its timeout)
//!
//! **What this is.** A program named in `commands` runs as you. Most
//! programs can run others or write any file (`sh`, `python3`, `node`,
//! `git clone … ~/.config/…`, `gh release download -O ~/.zshrc`), so
//! naming one is native power by another name: [`risk`] says so, and
//! consent asks for it every time, worded "can do anything you can".
//!
//! **What this holds anyway**, so the plain one-liners a reviewer or a
//! person wouldn't expect from "runs git" don't work:
//! - no shell: each argument reaches the program as it is;
//! - the program is found by its name on your login shell's PATH, never a
//!   path, and the plugin can't change PATH;
//! - `env`: only harmless variables ([`ENV_ALLOWED`], and `ZERON_PLUGIN_…`
//!   names of the plugin's own), never one that changes what runs or what
//!   loads (`GIT_*` but a few, `BASH_ENV`, `NODE_OPTIONS`, `PERL5OPT`,
//!   `PYTHON*`, `GH_CONFIG_DIR`, `*_PRELOAD`, `DYLD_*`, `LD_*`, `PATH`, …);
//! - git ([`git_args`]): only git's own commands (no aliases, no
//!   `git-<x>` from the PATH), no `-c` but harmless keys, no
//!   `--config-env`, `--exec-path`, `-p`, no `--upload-pack`/
//!   `--receive-pack`/`--exec`/`--output`/`--template`/…, no `-C` outside
//!   the run's folder, `git config` read-only; and every git run (and every
//!   git that `gh` starts) has `core.fsmonitor=false`, hooks off
//!   (`core.hooksPath=/dev/null`), `protocol.ext.allow=never`, no system
//!   config, no pager, no editor, no prompt;
//! - gh ([`gh_args`]): only gh's own commands (no aliases or extensions),
//!   never `alias`, `extension`, `codespace`, `auth` but `auth status`
//!   (without the token), `config` but reading non-token keys, and no git
//!   flags passed through `repo clone`/`repo fork`.

use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use serde_json::{Value as Json, json};

/// What a run prints, kept at most (each of stdout and stderr).
pub const MAX_OUTPUT: u64 = 16 * 1024 * 1024;
/// A run's longest wait unless it asks less, and the most it may ask.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);
pub const MAX_TIMEOUT: Duration = Duration::from_secs(300);

#[cfg(windows)]
const SEPARATOR: char = ';';
#[cfg(not(windows))]
const SEPARATOR: char = ':';

/// A program a manifest may name to run (`commands`): its name alone, as
/// it's found on the PATH, never a path (`gh`, `git`, `node`, `python3`).
pub fn valid_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    (1..=64).contains(&bytes.len())
        && bytes[0].is_ascii_alphanumeric()
        && bytes.iter().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'+' | b'-'))
}

/// How much a program named in `commands` can do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Risk {
    /// It neither runs other programs nor writes files (it may read
    /// them): `date`, `uname`, `cat`, `jq`.
    Contained,
    /// It can do anything you can: a shell, an interpreter, git, gh, or
    /// any program not known to be contained. Asked every time, never
    /// granted without asking, and worded "can do anything you can".
    Anything,
}

/// Programs known to neither run others nor write files, by
/// [`normalized`] name. Everything else is [`Risk::Anything`].
const CONTAINED: &[&str] = &[
    "basename", "cat", "cksum", "cut", "date", "df", "dirname", "du", "echo", "false", "file", "grep", "head",
    "hostname", "id", "jq", "ls", "md5", "md5sum", "nproc", "printenv", "printf", "ps", "pwd", "readlink",
    "realpath", "sha1sum", "sha256sum", "shasum", "sleep", "stat", "sw_vers", "tail", "tr", "true", "uname",
    "uptime", "wc", "which", "whoami",
];

/// What `program` can do (see [`Risk`]).
pub fn risk(program: &str) -> Risk {
    if CONTAINED.contains(&normalized(program).as_str()) { Risk::Contained } else { Risk::Anything }
}

/// Whether any of a manifest's `commands` can do anything you can.
pub fn any_can_do_anything(commands: &[String]) -> bool {
    commands.iter().any(|c| risk(c) == Risk::Anything)
}

/// A program's name as the system finds it: lowercase (macOS and Windows
/// find `Git` as `git`), without a Windows extension.
pub fn normalized(program: &str) -> String {
    let lower = program.to_ascii_lowercase();
    for ext in [".exe", ".cmd", ".bat", ".com"] {
        if let Some(stem) = lower.strip_suffix(ext) {
            return stem.to_owned();
        }
    }
    lower
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    Git,
    /// `git-lfs`, `git-upload-pack`, `scalar`: git's own config applies.
    GitTool,
    Gh,
    Other,
}

fn family(name: &str) -> Family {
    match normalized(name).as_str() {
        "git" => Family::Git,
        "gh" => Family::Gh,
        "scalar" => Family::GitTool,
        n if n.starts_with("git-") => Family::GitTool,
        _ => Family::Other,
    }
}

// ---------------------------------------------------------------------------
// Environment

/// Variables a run may set, by exact name: words and formatting, git's
/// identity and prompts, gh's quiet switches. Nothing that names a program,
/// a file to load, a config to read, or a host.
pub const ENV_ALLOWED: &[&str] = &[
    "LANG",
    "LANGUAGE",
    "LC_ALL",
    "LC_COLLATE",
    "LC_CTYPE",
    "LC_MESSAGES",
    "LC_MONETARY",
    "LC_NUMERIC",
    "LC_TIME",
    "TZ",
    "TERM",
    "NO_COLOR",
    "FORCE_COLOR",
    "CLICOLOR",
    "CLICOLOR_FORCE",
    "COLUMNS",
    "LINES",
    "CI",
    "GIT_TERMINAL_PROMPT",
    "GIT_OPTIONAL_LOCKS",
    "GIT_LITERAL_PATHSPECS",
    "GIT_MERGE_AUTOEDIT",
    "GIT_AUTHOR_NAME",
    "GIT_AUTHOR_EMAIL",
    "GIT_AUTHOR_DATE",
    "GIT_COMMITTER_NAME",
    "GIT_COMMITTER_EMAIL",
    "GIT_COMMITTER_DATE",
    // (Checked: a `.index` file in a git folder; see `git_index_path`.)
    "GIT_INDEX_FILE",
    "GH_PROMPT_DISABLED",
    "GH_NO_UPDATE_NOTIFIER",
    "GH_SPINNER_DISABLED",
    "GH_REPO",
];

/// The plugin's own variables: nothing else reads these.
pub const ENV_OWN_PREFIX: &str = "ZERON_PLUGIN_";

/// A run's own environment variables (`env = { LC_ALL = "C" }`), set over
/// yours: allowed names ([`ENV_ALLOWED`] or `ZERON_PLUGIN_…`), and text.
pub fn env_of(asked: &Json) -> Result<Vec<(String, String)>, String> {
    let Some(env) = asked.get("env").filter(|e| !e.is_null()) else {
        return Ok(Vec::new());
    };
    let Some(env) = env.as_object() else {
        return Err("A run's env is a table of names and their text".into());
    };
    let mut out = Vec::with_capacity(env.len());
    for (key, value) in env {
        let named = !key.is_empty()
            && key.len() <= 64
            && !key.as_bytes()[0].is_ascii_digit()
            && key.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_');
        if !named {
            return Err(format!("\"{key}\" isn't an environment variable's name (capitals, digits and _)"));
        }
        if !(ENV_ALLOWED.contains(&key.as_str()) || (key.starts_with(ENV_OWN_PREFIX) && key.len() > ENV_OWN_PREFIX.len())) {
            return Err(format!(
                "A run can't set {key}: a plugin sets only harmless variables (LANG, LC_…, TZ, NO_COLOR, git's identity, …) and its own ({ENV_OWN_PREFIX}…)"
            ));
        }
        let Some(value) = value.as_str() else {
            return Err(format!("{key}'s value is text"));
        };
        if value.len() > 4096 || value.contains('\0') {
            return Err(format!("{key}'s value is too long, or holds a NUL"));
        }
        if key == "GIT_INDEX_FILE" && !git_index_path(value) {
            return Err("GIT_INDEX_FILE is a .index file in a repository's git folder".into());
        }
        out.push((key.clone(), value.to_owned()));
    }
    Ok(out)
}

/// An index file of a plugin's own: an absolute `….index` path whose folder
/// is a repository's git folder (it holds `HEAD`). Writing one can't reach
/// anything but that repository.
fn git_index_path(value: &str) -> bool {
    let path = Path::new(value);
    path.is_absolute()
        && path.components().all(|c| !matches!(c, Component::ParentDir | Component::CurDir))
        && path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.ends_with(".index") && n.len() > ".index".len())
        && path.parent().is_some_and(|dir| dir.join("HEAD").is_file())
}

/// Config every git (and every git `gh` starts) runs with: no filesystem
/// monitor to run, no `ext::` transport, no pager.
pub const GIT_HARDENED: &[(&str, &str)] = &[("core.fsmonitor", "false"), ("protocol.ext.allow", "never"), ("core.pager", "cat")];

/// And hooks off: for every run but one of [`GIT_HOOKED`] in a checkout of
/// yours (`trusted`), where git runs the repository's hooks as it always
/// does (a commit's pre-commit, a push's pre-push).
pub const GIT_NO_HOOKS: (&str, &str) = ("core.hooksPath", "/dev/null");

/// git's commands whose hooks are yours to keep (they commit, merge, move
/// a branch or push): run with the repository's hooks in a trusted checkout.
pub const GIT_HOOKED: &[&str] =
    &["commit", "merge", "pull", "push", "rebase", "cherry-pick", "revert", "am", "switch", "checkout"];

/// The variables a git-family run gets over the plugin's: [`GIT_HARDENED`]
/// (and, unless `hooks`, [`GIT_NO_HOOKS`]) as command-line config
/// (`GIT_CONFIG_COUNT`, after any of yours), no system config, nothing to
/// open (pager, editor) or ask.
fn git_env(hooks: bool) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let start: usize = std::env::var("GIT_CONFIG_COUNT").ok().and_then(|n| n.parse().ok()).unwrap_or(0);
    let config: Vec<(&str, &str)> = GIT_HARDENED.iter().copied().chain((!hooks).then_some(GIT_NO_HOOKS)).collect();
    for (i, (key, value)) in config.iter().enumerate() {
        out.push((format!("GIT_CONFIG_KEY_{}", start + i), (*key).to_owned()));
        out.push((format!("GIT_CONFIG_VALUE_{}", start + i), (*value).to_owned()));
    }
    out.push(("GIT_CONFIG_COUNT".into(), (start + config.len()).to_string()));
    for (key, value) in [
        ("GIT_CONFIG_NOSYSTEM", "1"),
        ("GIT_TERMINAL_PROMPT", "0"),
        ("GIT_PAGER", "cat"),
        ("PAGER", "cat"),
        ("GIT_EDITOR", "true"),
        ("GIT_SEQUENCE_EDITOR", "true"),
    ] {
        out.push((key.into(), value.into()));
    }
    out
}

// ---------------------------------------------------------------------------
// git

/// git's own commands a plugin may run. Built-in names can't be aliases
/// (git refuses an alias that hides one), and nothing here runs a program
/// its arguments name. Missing on purpose: `archive`, `bundle`,
/// `checkout-index`, `credential`, `daemon`, `difftool`, `fast-import`,
/// `filter-branch`, `for-each-repo`, `help`, `hook`, `instaweb`,
/// `maintenance`, `mergetool`, `send-email`, `upload-pack`, … and any
/// `git <x>` that would run `git-<x>` from the PATH.
const GIT_COMMANDS: &[&str] = &[
    "add", "am", "annotate", "apply", "bisect", "blame", "branch", "cat-file", "check-attr", "check-ignore",
    "check-mailmap", "check-ref-format", "checkout", "cherry", "cherry-pick", "clean", "clone", "commit",
    "commit-graph", "commit-tree", "config", "count-objects", "describe", "diff", "diff-files", "diff-index",
    "diff-tree", "fetch", "for-each-ref", "format-patch", "fsck", "gc", "grep", "hash-object", "init",
    "interpret-trailers", "log", "ls-files", "ls-remote", "ls-tree", "merge", "merge-base", "merge-file",
    "merge-tree", "mktag", "mktree", "mv", "name-rev", "notes", "prune", "pull", "push", "range-diff",
    "read-tree", "rebase", "reflog", "remote", "repack", "replace", "rerere", "reset", "restore", "rev-list",
    "rev-parse", "revert", "rm", "shortlog", "show", "show-branch", "show-ref", "sparse-checkout", "stash",
    "status", "stripspace", "submodule", "switch", "symbolic-ref", "tag", "update-index", "update-ref", "var",
    "verify-commit", "verify-pack", "verify-tag", "version", "whatchanged", "worktree", "write-tree",
];

/// `--<name>` options refused after git's command, and any abbreviation of
/// one (git takes a unique prefix): each runs a program it names, writes a
/// file anywhere, or reads config from an argument.
const GIT_REFUSED_LONG: &[&str] = &[
    "upload-pack",
    "receive-pack",
    "exec",
    "output",
    "output-directory",
    "config",
    "config-env",
    "template",
    "separate-git-dir",
    "extcmd",
    "open-files-in-pager",
    "exec-path",
    "export-marks",
    "import-marks",
];

/// Short options refused after a given command (in any cluster: `-qc`).
fn git_refused_short(command: &str) -> &'static [char] {
    match command {
        "clone" => &['c', 'u'],
        "ls-remote" => &['u'],
        "format-patch" => &['o'],
        "grep" => &['O'],
        "rebase" => &['x'],
        _ => &[],
    }
}

/// `-c key=value` keys a plugin may set: how git prints and words things,
/// who it says you are. None names a program or a file.
fn git_safe_config(key: &str, value: &str) -> bool {
    let key = key.to_ascii_lowercase();
    if value.contains(['\n', '\0']) {
        return false;
    }
    match key.as_str() {
        "core.quotepath" | "core.abbrev" | "core.autocrlf" | "core.safecrlf" | "core.longpaths" | "color.ui"
        | "column.ui" | "i18n.logoutputencoding" | "i18n.commitencoding" | "log.showsignature" | "log.decorate"
        | "diff.renames" | "diff.noprefix" | "diff.mnemonicprefix" | "diff.relative" | "status.showuntrackedfiles"
        | "status.renames" | "user.name" | "user.email" | "commit.gpgsign" | "tag.gpgsign" | "init.defaultbranch"
        | "merge.conflictstyle" | "pull.rebase" | "pull.ff" | "rebase.autostash" | "fetch.prune" | "gc.auto"
        | "maintenance.auto" | "advice.detachedhead" => true,
        "core.pager" => value == "cat",
        // (What every run has anyway, said again.)
        "core.fsmonitor" => value == "false",
        "core.hookspath" => value == "/dev/null",
        "protocol.ext.allow" => value == "never",
        _ => key.starts_with("color.") || key.starts_with("advice."),
    }
}

/// `path` (from `base` when relative) with `.` and `..` worked out
/// lexically; none if it climbs past the root.
fn resolved(base: Option<&Path>, path: &str) -> Option<PathBuf> {
    let joined = match base {
        _ if Path::new(path).is_absolute() => PathBuf::from(path),
        Some(base) => base.join(path),
        None => return None,
    };
    let mut out: Vec<Component> = Vec::new();
    for c in joined.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !matches!(out.last(), Some(Component::Normal(_))) {
                    return None;
                }
                out.pop();
            }
            c => out.push(c),
        }
    }
    Some(out.iter().collect())
}

/// Lexically: `path` (from `cwd` when relative) inside `cwd`.
fn inside(cwd: &Path, path: &str) -> bool {
    resolved(Some(cwd), path).is_some_and(|p| p.starts_with(cwd))
}

/// What a git run a plugin may make does.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitRun {
    /// Its command (`commit`), none for `git --version`.
    pub command: Option<String>,
    /// The folder it works in (the run's, after any `-C`), when known.
    pub dir: Option<PathBuf>,
    /// It names its git folder or work tree (`--git-dir`, `--work-tree`):
    /// what it works on isn't `dir` alone.
    pub elsewhere: bool,
}

impl GitRun {
    /// Whether it runs the repository's hooks: one of [`GIT_HOOKED`], in a
    /// folder `trusted` says is a checkout of yours.
    pub fn hooks(&self, trusted: &dyn Fn(&Path) -> bool) -> bool {
        !self.elsewhere
            && self.command.as_deref().is_some_and(|c| GIT_HOOKED.contains(&c))
            && self.dir.as_deref().is_some_and(trusted)
    }
}

/// git's arguments, if a plugin may run them (`cwd`: the run's folder):
/// what the run does, or why it may not. See the module's notes.
pub fn git_args(args: &[String], cwd: Option<&Path>) -> Result<GitRun, String> {
    let mut run = GitRun { dir: cwd.map(Path::to_path_buf), ..GitRun::default() };
    let refuse = |why: String| Err(format!("Zeron doesn't run git that way: {why}"));
    // A folder git is pointed at: inside the run's folder, when it has one.
    let check_dir = |flag: &str, dir: &str| -> Result<(), String> {
        match cwd {
            Some(cwd) if !inside(cwd, dir) => Err(format!(
                "Zeron doesn't run git that way: {flag} {dir} is outside the run's folder ({})",
                cwd.display()
            )),
            None if !Path::new(dir).is_absolute() => {
                Err(format!("Zeron doesn't run git that way: {flag} {dir} needs a whole path, or the run a cwd"))
            }
            _ => Ok(()),
        }
    };
    let mut i = 0;
    // git's own options, before its command.
    let command = loop {
        let Some(arg) = args.get(i) else {
            return Ok(run); // (`git`, `git --version`: nothing runs.)
        };
        i += 1;
        let arg = arg.as_str();
        if !arg.starts_with('-') {
            break arg;
        }
        let (flag, attached) = match arg.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f, Some(v)),
            _ => (arg, None),
        };
        match flag {
            "--version" | "--no-pager" | "-P" | "--no-replace-objects" | "--no-optional-locks" | "--literal-pathspecs"
            | "--glob-pathspecs" | "--noglob-pathspecs" | "--icase-pathspecs" | "--no-advice" | "--no-lazy-fetch"
            | "--bare"
                if attached.is_none() => {}
            "-C" | "--git-dir" | "--work-tree" => {
                let value = match attached {
                    Some(v) => v,
                    None => {
                        i += 1;
                        match args.get(i - 1) {
                            Some(v) => v.as_str(),
                            None => return refuse(format!("{flag} needs a folder")),
                        }
                    }
                };
                check_dir(flag, value)?;
                if flag == "-C" {
                    run.dir = resolved(run.dir.as_deref(), value);
                } else {
                    run.elsewhere = true;
                }
            }
            "--namespace" => {
                if attached.is_none() {
                    i += 1;
                }
            }
            "-c" => {
                let Some(pair) = args.get(i) else {
                    return refuse("-c needs a key=value".into());
                };
                i += 1;
                let (key, value) = pair.split_once('=').unwrap_or((pair.as_str(), "true"));
                if !git_safe_config(key, value) {
                    return refuse(format!("-c {key} could change what git runs; a plugin sets only how it prints"));
                }
            }
            _ => return refuse(format!("{arg} isn't an option a plugin may give git")),
        }
    };
    if !GIT_COMMANDS.contains(&command) {
        return refuse(format!("\"{command}\" isn't one of git's own commands a plugin may run"));
    }
    run.command = Some(command.to_owned());
    let shorts = git_refused_short(command);
    let rest = &args[i..];
    let mut words: Vec<&str> = Vec::new();
    let mut j = 0;
    while j < rest.len() {
        let arg = rest[j].as_str();
        j += 1;
        if arg == "--" || arg == "--end-of-options" {
            break;
        }
        if let Some(long) = arg.strip_prefix("--") {
            let (name, value) = match long.split_once('=') {
                Some((n, v)) => (n, Some(v)),
                None => (long, None),
            };
            if let Some(refused) = GIT_REFUSED_LONG.iter().find(|r| !name.is_empty() && r.starts_with(name)) {
                return refuse(format!("--{refused} runs a program or writes outside the repository"));
            }
            if name.len() >= 2 && "index-output".starts_with(name) {
                let value = match value {
                    Some(v) => Some(v),
                    None => {
                        j += 1;
                        rest.get(j - 1).map(String::as_str)
                    }
                };
                if !value.is_some_and(git_index_path) {
                    return refuse("--index-output is a .index file in a repository's git folder".into());
                }
            }
            continue;
        }
        if let Some(cluster) = arg.strip_prefix('-').filter(|c| !c.is_empty()) {
            if let Some(c) = cluster.chars().find(|c| shorts.contains(c)) {
                return refuse(format!("git {command} -{c} runs a program or writes outside the repository"));
            }
            continue;
        }
        words.push(arg);
    }
    match command {
        "bisect" if words.first() == Some(&"run") => Err("Zeron doesn't run git that way: git bisect run runs a program".into()),
        "submodule" if words.contains(&"foreach") => Err("Zeron doesn't run git that way: git submodule foreach runs a program".into()),
        "config" => {
            let all: Vec<&str> = rest.iter().map(String::as_str).collect();
            let reads = all.iter().any(|a| {
                matches!(*a, "--get" | "--get-all" | "--get-regexp" | "--get-urlmatch" | "--get-color" | "--get-colorbool" | "-l" | "--list")
            }) || matches!(words.first(), Some(&"get") | Some(&"list"));
            let writes = all.iter().any(|a| {
                matches!(*a, "--add" | "--unset" | "--unset-all" | "--replace-all" | "--rename-section" | "--remove-section" | "-e" | "--edit")
            }) || matches!(words.first(), Some(&"set") | Some(&"unset") | Some(&"rename-section") | Some(&"remove-section") | Some(&"edit"));
            if reads && !writes { Ok(run) } else { refuse("a plugin reads git's config, never writes it".into()) }
        }
        _ => Ok(run),
    }
}

// ---------------------------------------------------------------------------
// gh

/// gh's own commands a plugin may run (never an alias or an extension).
const GH_COMMANDS: &[&str] = &[
    "api", "attestation", "auth", "browse", "cache", "completion", "config", "gist", "gpg-key", "issue", "label",
    "org", "pr", "project", "release", "repo", "ruleset", "run", "search", "secret", "ssh-key", "status",
    "variable", "version", "workflow",
];

/// gh's arguments, if a plugin may run them.
pub fn gh_args(args: &[String]) -> Result<(), String> {
    let refuse = |why: &str| Err(format!("Zeron doesn't run gh that way: {why}"));
    let words: Vec<&str> = args.iter().map(String::as_str).filter(|a| !a.starts_with('-')).collect();
    let Some(&command) = words.first() else {
        return Ok(()); // (`gh --version`.)
    };
    if args.first().is_some_and(|a| a.starts_with('-') && a != "--version" && a != "--help") {
        return refuse("gh's command comes first");
    }
    match command {
        "alias" | "extension" | "extensions" | "ext" => return refuse("aliases and extensions run programs"),
        "codespace" | "cs" => return refuse("codespace commands run programs here (ssh, cp)"),
        _ if !GH_COMMANDS.contains(&command) => return refuse("that isn't one of gh's own commands"),
        _ => {}
    }
    let sub = words.get(1).copied().unwrap_or_default();
    match command {
        "auth" => {
            if sub != "status" {
                return refuse("a plugin may ask gh auth status, nothing else of gh auth");
            }
            if args.iter().any(|a| a == "-t" || a.starts_with("--show-token") || (a.starts_with('-') && !a.starts_with("--") && a.contains('t'))) {
                return refuse("gh's token is never shown to a plugin");
            }
        }
        "config" => {
            let reads_token = words.iter().any(|w| w.to_ascii_lowercase().contains("token"));
            if !matches!(sub, "get" | "list") || reads_token {
                return refuse("a plugin reads gh's settings (never a token), never changes them");
            }
        }
        "repo" if matches!(sub, "clone" | "fork") && args.iter().any(|a| a == "--") => {
            return refuse("git's own flags can't be passed through gh");
        }
        _ => {}
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Running

/// Your login shell's PATH, read once, then the process's and the usual
/// places programs are installed.
fn user_path() -> &'static str {
    static PATH: OnceLock<String> = OnceLock::new();
    PATH.get_or_init(|| {
        let mut dirs: Vec<String> = Vec::new();
        #[cfg(not(windows))]
        {
            let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
            if let Ok(out) = Command::new(&shell)
                .args(["-l", "-c", "printf %s \"$PATH\""])
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .output()
                && out.status.success()
            {
                dirs.extend(String::from_utf8_lossy(&out.stdout).split(SEPARATOR).filter(|d| !d.is_empty()).map(str::to_string));
            }
        }
        let usual: &[&str] =
            if cfg!(windows) { &[] } else { &["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin", "/usr/sbin", "/sbin"] };
        let process = std::env::var("PATH").unwrap_or_default();
        for extra in process.split(SEPARATOR).map(str::to_string).chain(usual.iter().map(|d| d.to_string())) {
            if !extra.is_empty() && !dirs.contains(&extra) {
                dirs.push(extra);
            }
        }
        dirs.join(&SEPARATOR.to_string())
    })
}

#[cfg(not(windows))]
fn executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(windows)]
fn executable(path: &Path) -> bool {
    path.is_file()
}

/// Where `name` is on `path`: the first executable file of that name.
fn find(name: &str, path: &str) -> Option<PathBuf> {
    let names: Vec<String> = if cfg!(windows) {
        ["", ".exe", ".cmd", ".bat"].iter().map(|ext| format!("{name}{ext}")).collect()
    } else {
        vec![name.to_owned()]
    };
    path.split(SEPARATOR)
        .filter(|dir| Path::new(dir).is_absolute())
        .flat_map(|dir| names.iter().map(move |n| Path::new(dir).join(n)))
        .find(|p| executable(p))
}

/// Run what `asked` says, if `allowed` names its program and the rules
/// above let it: the answer, or why it wasn't run. Blocks: call it off the
/// async runtime (or the window's thread).
/// Every git runs with hooks off.
pub fn run(asked: &Json, allowed: &[String]) -> Result<Json, String> {
    run_on(asked, allowed, user_path(), &|_| false)
}

/// [`run`], where a git that commits, merges or pushes ([`GIT_HOOKED`]) in
/// a folder `trusted` says is a checkout of yours (a chat's, a project's)
/// runs the repository's hooks, as git does when you run it.
pub fn run_trusting(asked: &Json, allowed: &[String], trusted: &dyn Fn(&Path) -> bool) -> Result<Json, String> {
    run_on(asked, allowed, user_path(), trusted)
}

/// [`run_trusting`], finding programs on `path`.
pub fn run_on(asked: &Json, allowed: &[String], path: &str, trusted: &dyn Fn(&Path) -> bool) -> Result<Json, String> {
    let program = asked["program"].as_str().unwrap_or_default();
    if !valid_name(program) || !allowed.iter().any(|a| a == program) {
        return Err(format!("This plugin doesn't run \"{program}\": its manifest's \"commands\" names what it may run"));
    }
    let mut args: Vec<String> = Vec::new();
    for arg in asked["args"].as_array().into_iter().flatten() {
        match arg {
            Json::String(s) => args.push(s.clone()),
            Json::Number(n) => args.push(n.to_string()),
            Json::Bool(b) => args.push(b.to_string()),
            _ => return Err("Each of a run's args is text".into()),
        }
    }
    if args.iter().any(|a| a.contains('\0')) {
        return Err("A run's args can't hold a NUL".into());
    }
    let cwd = match asked["cwd"].as_str().filter(|c| !c.is_empty()) {
        Some(cwd) => {
            let dir = Path::new(cwd);
            if !dir.is_absolute() || !dir.is_dir() {
                return Err(format!("There's no folder {cwd} on this device"));
            }
            Some(dir.to_path_buf())
        }
        None => None,
    };
    let env = env_of(asked)?;
    let found =
        find(program, path).ok_or_else(|| format!("\"{program}\" isn't installed on this device (or isn't on your PATH)"))?;
    // What runs, by its name and by the file it really is (a link named
    // `g` to git is git).
    let real = std::fs::canonicalize(&found).ok();
    let real_name = real.as_deref().and_then(Path::file_name).and_then(|n| n.to_str()).unwrap_or(program);
    let families = [family(program), family(real_name)];
    let is = |f: Family| families.contains(&f);
    let hooks = if is(Family::Git) {
        let run = git_args(&args, cwd.as_deref())?;
        !is(Family::Gh) && run.hooks(trusted)
    } else {
        false
    };
    if is(Family::Gh) {
        gh_args(&args)?;
    }

    let mut command = Command::new(&found);
    command.args(&args).env("PATH", path).stdout(Stdio::piped()).stderr(Stdio::piped());
    for (key, value) in env {
        command.env(key, value);
    }
    // (Over the plugin's: git, and the git gh starts, always hardened.)
    if is(Family::Git) || is(Family::GitTool) || is(Family::Gh) {
        for (key, value) in git_env(hooks) {
            command.env(key, value);
        }
    }
    if let Some(cwd) = &cwd {
        command.current_dir(cwd);
    }
    let stdin = asked["stdin"].as_str().map(str::to_string);
    command.stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() });
    let timeout = asked["timeout"]
        .as_f64()
        .filter(|s| s.is_finite() && *s > 0.)
        .map(|s| Duration::from_secs_f64(s.min(MAX_TIMEOUT.as_secs_f64())))
        .unwrap_or(DEFAULT_TIMEOUT);
    let mut child = command.spawn().map_err(|e| format!("\"{program}\" didn't start: {e}"))?;
    if let (Some(text), Some(mut pipe)) = (stdin, child.stdin.take()) {
        // (Written beside the wait: a program reading slowly can't stall it.)
        std::thread::spawn(move || {
            let _ = pipe.write_all(text.as_bytes());
        });
    }
    let read = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(pipe) = pipe {
                let _ = pipe.take(MAX_OUTPUT).read_to_end(&mut bytes);
            }
            String::from_utf8_lossy(&bytes).into_owned()
        })
    };
    let out = read(child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let err = read(child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let start = Instant::now();
    let status = loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(status) => break status.code().unwrap_or(-1),
            None if start.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                break -1;
            }
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    };
    Ok(json!({
        "status": status,
        "stdout": out.join().unwrap_or_default(),
        "stderr": err.join().unwrap_or_default(),
    }))
}

#[cfg(all(test, unix))]
mod tests;
