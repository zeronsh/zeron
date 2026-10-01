//! Paths: canonicalisation, temp folders, and what each agent needs to write
//! (and must not read) to work inside a sandbox.

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use zeron_proto::HarnessId;

/// `path` with symlinks resolved as far as it exists (macOS `/tmp` →
/// `/private/tmp`, `/var` → `/private/var`); the part that doesn't exist yet
/// is appended lexically (`.` dropped, `..` popped). Sandboxes match
/// resolved paths, so every rule is written in this form.
pub fn canonicalize_lossy(path: &Path) -> PathBuf {
    if let Ok(real) = std::fs::canonicalize(path) {
        return real;
    }
    let components: Vec<Component> = path.components().collect();
    let mut out = PathBuf::new();
    let mut rest: &[Component] = &components;
    for split in (1..components.len()).rev() {
        let prefix: PathBuf = components[..split].iter().collect();
        if let Ok(real) = std::fs::canonicalize(&prefix) {
            out = real;
            rest = &components[split..];
            break;
        }
    }
    for component in rest {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Folders every sandboxed process may write: the shared and per-user temp
/// directories (macOS `confstr(_CS_DARWIN_USER_TEMP_DIR)` = `$TMPDIR` and its
/// cache sibling), canonicalised.
pub fn system_temp_dirs() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    if cfg!(target_os = "macos") {
        out.push("/private/tmp".into());
        out.push("/private/var/tmp".into());
        #[cfg(target_os = "macos")]
        for name in [
            libc::_CS_DARWIN_USER_TEMP_DIR,
            libc::_CS_DARWIN_USER_CACHE_DIR,
        ] {
            if let Some(dir) = darwin_confstr(name) {
                out.push(dir);
            }
        }
    } else {
        out.push("/tmp".into());
        out.push("/var/tmp".into());
        out.push("/dev/shm".into());
    }
    if let Some(tmp) = std::env::var_os("TMPDIR").filter(|t| !t.is_empty()) {
        let tmp = PathBuf::from(tmp);
        if tmp.is_absolute() {
            out.push(tmp);
        }
    }
    let mut out: Vec<PathBuf> = out.iter().map(|p| canonicalize_lossy(p)).collect();
    out.sort();
    out.dedup();
    out
}

#[cfg(target_os = "macos")]
fn darwin_confstr(name: libc::c_int) -> Option<PathBuf> {
    let mut buf = vec![0u8; 1024];
    // SAFETY: the buffer is writable for its full length; confstr writes at
    // most `len` bytes including the terminator and returns the needed size.
    let len = unsafe { libc::confstr(name, buf.as_mut_ptr().cast(), buf.len()) };
    if len == 0 || len > buf.len() {
        return None;
    }
    buf.truncate(len - 1);
    use std::os::unix::ffi::OsStringExt;
    let path = PathBuf::from(OsString::from_vec(buf));
    path.is_absolute().then_some(path)
}

/// What one agent needs on top of the workspace: where it keeps its state,
/// what must stay read-only inside that, and what it must not read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentPaths {
    pub writable: Vec<PathBuf>,
    pub writable_prefixes: Vec<PathBuf>,
    pub read_only: Vec<PathBuf>,
    pub hidden: Vec<PathBuf>,
    /// Environment the agent should get when sandboxed (e.g. turn off a
    /// self-updater that would write outside its state).
    pub env: Vec<(OsString, OsString)>,
}

/// Which OS's conventions to use; [`default_agent_paths`] uses the current one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    MacOs,
    Linux,
    Windows,
}

impl Platform {
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Platform::MacOs
        } else if cfg!(windows) {
            Platform::Windows
        } else {
            Platform::Linux
        }
    }
}

/// [`agent_paths_for`] on this OS with this process's environment.
pub fn default_agent_paths(harness: HarnessId, home: &Path) -> AgentPaths {
    agent_paths_for(harness, home, Platform::current(), &|name| {
        std::env::var_os(name)
    })
}

/// The agent's state (writable), its self-modification hooks (read-only), and
/// the secret stores it has no business reading (hidden): the usual
/// credential folders, Zeron's own account vault, and every *other* agent's
/// login. Env overrides the agents honour (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`,
/// `XDG_*`, …) are read through `env`.
pub fn agent_paths_for(
    harness: HarnessId,
    home: &Path,
    platform: Platform,
    env: &dyn Fn(&str) -> Option<OsString>,
) -> AgentPaths {
    let dirs = Dirs::new(home, platform, env);
    let own = dirs.agent(harness);
    let mut hidden: Vec<PathBuf> = COMMON_SECRETS.iter().map(|rel| home.join(rel)).collect();
    match platform {
        Platform::MacOs => {
            hidden.extend(MACOS_SECRETS.iter().map(|rel| home.join(rel)));
            if !own.keychain {
                hidden.push(home.join("Library/Keychains"));
            }
        }
        Platform::Linux => hidden.extend(LINUX_SECRETS.iter().map(|rel| home.join(rel))),
        Platform::Windows => {}
    }
    hidden.push(dirs.zeron.join("agent-accounts"));
    hidden.push(dirs.zeron.join("session.json"));
    for other in ALL_HARNESSES {
        if *other != harness {
            hidden.extend(dirs.agent(*other).credentials);
        }
    }
    hidden.retain(|h| !own.credentials.contains(h));
    AgentPaths {
        writable: own.writable,
        writable_prefixes: own.prefixes,
        read_only: own.read_only,
        hidden,
        env: own.env,
    }
}

/// Git's own files for `workspace`: in a linked worktree (Zeron's
/// `~/.zeron/worktrees/…`) or a subfolder of a repository the git dirs live
/// outside the workspace, so commits need them `writable`; hooks and config
/// are `read_only` either way, because a hook or `core.fsmonitor` written by
/// the agent would run *unconfined* the next time Zeron or the user runs git
/// there. Nothing when the workspace isn't in a repository.
///
/// Only meaningful for [`SandboxMode::WorkspaceWrite`](crate::SandboxMode):
/// [`SandboxSpec::for_agent`](crate::SandboxSpec::for_agent) drops the
/// writable half otherwise.
pub fn git_paths(workspace: &Path) -> AgentPaths {
    let mut out = AgentPaths::default();
    let Some(dot_git) = workspace
        .ancestors()
        .map(|a| a.join(".git"))
        .find(|g| g.exists())
    else {
        return out;
    };
    let git_dir = if dot_git.is_dir() {
        dot_git.clone()
    } else {
        let Some(target) = std::fs::read_to_string(&dot_git)
            .ok()
            .and_then(|s| s.strip_prefix("gitdir:").map(|t| t.trim().to_owned()))
        else {
            return out;
        };
        let base = dot_git.parent().unwrap_or(workspace);
        base.join(target)
    };
    let common = std::fs::read_to_string(git_dir.join("commondir"))
        .ok()
        .map(|rel| git_dir.join(rel.trim()))
        .unwrap_or_else(|| git_dir.clone());
    let git_dir = canonicalize_lossy(&git_dir);
    let common = canonicalize_lossy(&common);
    let workspace = canonicalize_lossy(workspace);
    for dir in [&git_dir, &common] {
        if !dir.starts_with(&workspace) && !out.writable.contains(dir) {
            out.writable.push(dir.clone());
        }
    }
    out.read_only.push(common.join("hooks"));
    out.read_only.push(common.join("config"));
    if git_dir != common {
        out.read_only.push(git_dir.join("config.worktree"));
    }
    out
}

/// Package-manager caches. Not in any default: a writable cache that holds
/// code (crate sources, modules) lets a confined agent plant code that other
/// projects build unconfined later. Offer it as an explicit opt-in.
pub fn toolchain_cache_paths(home: &Path, platform: Platform) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = [
        ".npm",
        ".cargo/registry",
        ".cargo/git",
        "go/pkg/mod",
        ".bun/install/cache",
        ".gradle/caches",
        ".m2/repository",
    ]
    .iter()
    .map(|rel| home.join(rel))
    .collect();
    out.push(match platform {
        Platform::MacOs => home.join("Library/Caches"),
        _ => home.join(".cache"),
    });
    out
}

const ALL_HARNESSES: &[HarnessId] = &[
    HarnessId::ClaudeCode,
    HarnessId::Codex,
    HarnessId::Cursor,
    HarnessId::Devin,
    HarnessId::Grok,
    HarnessId::Hermes,
    HarnessId::Pi,
    HarnessId::Opencode,
    HarnessId::Antigravity,
    HarnessId::Mock,
];

/// Secret stores under `$HOME` on every OS: SSH and GPG keys, cloud and
/// cluster credentials, plaintext token stores, registry publish tokens and
/// password-manager state.
const COMMON_SECRETS: &[&str] = &[
    ".ssh",
    ".gnupg",
    ".aws",
    ".azure",
    ".config/gcloud",
    ".kube",
    ".docker/config.json",
    ".netrc",
    ".git-credentials",
    ".config/git/credentials",
    ".npmrc",
    ".pypirc",
    ".cargo/credentials",
    ".cargo/credentials.toml",
    ".password-store",
    ".vault-token",
    ".terraform.d/credentials.tfrc.json",
    ".config/op",
];

/// Browser profiles (cookies are live sessions) and password managers.
const MACOS_SECRETS: &[&str] = &[
    "Library/Cookies",
    "Library/Application Support/Google/Chrome",
    "Library/Application Support/Chromium",
    "Library/Application Support/BraveSoftware",
    "Library/Application Support/Microsoft Edge",
    "Library/Application Support/Firefox",
    "Library/Application Support/Arc",
    "Library/Group Containers/2BUA8C4S2C.com.1password",
];

const LINUX_SECRETS: &[&str] = &[
    ".local/share/keyrings",
    ".mozilla",
    ".config/google-chrome",
    ".config/chromium",
    ".config/BraveSoftware",
    ".config/microsoft-edge",
];

struct AgentState {
    writable: Vec<PathBuf>,
    prefixes: Vec<PathBuf>,
    read_only: Vec<PathBuf>,
    credentials: Vec<PathBuf>,
    /// Keeps its login in the macOS Keychain, so `~/Library/Keychains` must
    /// stay readable (the Security framework reads the keychain file
    /// in-process; hiding it logs Claude Code out — verified).
    keychain: bool,
    env: Vec<(OsString, OsString)>,
}

struct Dirs<'a> {
    home: &'a Path,
    platform: Platform,
    env: &'a dyn Fn(&str) -> Option<OsString>,
    data: PathBuf,
    config: PathBuf,
    cache: PathBuf,
    state: PathBuf,
    zeron: PathBuf,
}

impl<'a> Dirs<'a> {
    fn new(home: &'a Path, platform: Platform, env: &'a dyn Fn(&str) -> Option<OsString>) -> Self {
        let xdg = |name: &str, default: &str| {
            absolute_env(env, name).unwrap_or_else(|| home.join(default))
        };
        Self {
            home,
            platform,
            env,
            data: xdg("XDG_DATA_HOME", ".local/share"),
            config: xdg("XDG_CONFIG_HOME", ".config"),
            cache: xdg("XDG_CACHE_HOME", ".cache"),
            state: xdg("XDG_STATE_HOME", ".local/state"),
            zeron: absolute_env(env, "ZERON_DATA_DIR").unwrap_or_else(|| home.join(".zeron")),
        }
    }

    fn dir(&self, name: &str, default: &str) -> PathBuf {
        absolute_env(self.env, name).unwrap_or_else(|| self.home.join(default))
    }

    fn agent(&self, harness: HarnessId) -> AgentState {
        let home = self.home;
        let mac = self.platform == Platform::MacOs;
        let mut s = AgentState {
            writable: Vec::new(),
            prefixes: Vec::new(),
            read_only: Vec::new(),
            credentials: Vec::new(),
            keychain: false,
            env: Vec::new(),
        };
        match harness {
            HarnessId::ClaudeCode => {
                let custom = absolute_env(self.env, "CLAUDE_CONFIG_DIR");
                let config = custom.clone().unwrap_or_else(|| home.join(".claude"));
                s.writable.push(config.clone());
                // MCP and debug logs.
                s.writable.push(if mac {
                    home.join("Library/Caches/claude-cli-nodejs")
                } else {
                    self.cache.join("claude-cli-nodejs")
                });
                // Without CLAUDE_CONFIG_DIR the global state file sits in
                // $HOME and is replaced atomically via `.claude.json.tmp.*`.
                if custom.is_none() {
                    s.prefixes.push(home.join(".claude.json"));
                }
                // User-level hooks/commands/agents run on every later launch,
                // including unsandboxed ones.
                s.read_only
                    .extend(["settings.json", "commands", "agents"].map(|rel| config.join(rel)));
                s.credentials.push(config.join(".credentials.json"));
                s.keychain = true;
                // The native installer's updater rewrites ~/.local/share/claude
                // and ~/.local/bin/claude; Zeron updates harnesses itself.
                s.env.push(("DISABLE_AUTOUPDATER".into(), "1".into()));
            }
            HarnessId::Codex => {
                let codex = self.dir("CODEX_HOME", ".codex");
                s.writable.push(codex.clone());
                // MCP servers and `notify` commands, and the CLI itself.
                s.read_only.push(codex.join("config.toml"));
                s.read_only.push(codex.join("packages"));
                s.credentials.push(codex.join("auth.json"));
            }
            HarnessId::Cursor => {
                s.writable.push(home.join(".cursor"));
                s.writable.push(self.config.join("cursor"));
                s.writable.push(
                    absolute_env(self.env, "ZERON_CURSOR_STATE_DIR")
                        .unwrap_or_else(|| self.zeron.join("cursor-state")),
                );
                s.credentials.push(home.join(".cursor/sdk/auth.json"));
                s.keychain = true;
            }
            HarnessId::Devin => {
                s.writable.push(self.data.join("devin"));
                s.writable.push(self.config.join("devin"));
                s.credentials.push(self.data.join("devin/credentials.toml"));
                if mac {
                    let support = home.join("Library/Application Support/devin");
                    s.credentials.push(support.join("credentials.toml"));
                    s.writable.push(support);
                }
                s.keychain = true;
            }
            HarnessId::Grok => {
                let grok = self.dir("GROK_HOME", ".grok");
                s.credentials.push(grok.join("auth.json"));
                s.writable.push(grok);
                s.keychain = true;
            }
            HarnessId::Hermes => {
                let hermes = self.dir("HERMES_HOME", ".hermes");
                s.credentials.push(hermes.join("auth.json"));
                s.credentials.push(self.data.join("hermes/auth.json"));
                s.writable.push(hermes);
                s.writable.push(self.data.join("hermes"));
                s.keychain = true;
            }
            HarnessId::Pi => {
                let agent = self.dir("PI_CODING_AGENT_DIR", ".pi/agent");
                s.writable.push(home.join(".pi"));
                if !agent.starts_with(home.join(".pi")) {
                    s.writable.push(agent.clone());
                }
                s.credentials.push(agent.join("auth.json"));
            }
            HarnessId::Opencode => {
                s.writable.push(self.data.join("opencode"));
                s.writable.push(self.config.join("opencode"));
                s.writable.push(self.cache.join("opencode"));
                s.writable.push(self.state.join("opencode"));
                s.writable.push(home.join(".opencode"));
                s.credentials.push(self.data.join("opencode/auth.json"));
            }
            HarnessId::Antigravity => {
                let gemini = self.dir("GEMINI_HOME", ".gemini");
                s.credentials
                    .push(gemini.join("antigravity-acp/acp_token.json"));
                s.writable.push(gemini);
                s.keychain = true;
            }
            HarnessId::Mock => {}
        }
        s
    }
}

fn absolute_env(env: &dyn Fn(&str) -> Option<OsString>, name: &str) -> Option<PathBuf> {
    env(name)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_env(_: &str) -> Option<OsString> {
        None
    }

    #[test]
    fn canonicalize_resolves_existing_prefix_and_keeps_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let real = std::fs::canonicalize(dir.path()).unwrap();
        assert_eq!(canonicalize_lossy(dir.path()), real);
        assert_eq!(
            canonicalize_lossy(&dir.path().join("not/yet/../there/./x")),
            real.join("not/there/x")
        );
        #[cfg(unix)]
        {
            let link = dir.path().join("link");
            std::os::unix::fs::symlink(dir.path().join("target"), &link).unwrap();
            std::fs::create_dir(dir.path().join("target")).unwrap();
            assert_eq!(
                canonicalize_lossy(&link.join("child")),
                real.join("target/child")
            );
        }
        assert_eq!(
            canonicalize_lossy(Path::new("/zeron-no-such-root/a")),
            PathBuf::from("/zeron-no-such-root/a")
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_aliases_resolve_to_private() {
        assert_eq!(
            canonicalize_lossy(Path::new("/tmp/zeron-x")),
            PathBuf::from("/private/tmp/zeron-x")
        );
        assert_eq!(
            canonicalize_lossy(Path::new("/var/tmp")),
            PathBuf::from("/private/var/tmp")
        );
        let temps = system_temp_dirs();
        assert!(temps.contains(&PathBuf::from("/private/tmp")));
        assert!(
            temps.iter().any(|t| t.starts_with("/private/var/folders")),
            "per-user temp dir missing: {temps:?}"
        );
    }

    #[test]
    fn claude_gets_its_state_and_hides_everyone_elses_login() {
        let home = Path::new("/h");
        let p = agent_paths_for(HarnessId::ClaudeCode, home, Platform::MacOs, &no_env);
        assert_eq!(
            p.writable,
            vec![
                PathBuf::from("/h/.claude"),
                "/h/Library/Caches/claude-cli-nodejs".into()
            ]
        );
        assert_eq!(p.writable_prefixes, vec![PathBuf::from("/h/.claude.json")]);
        assert!(p.read_only.contains(&"/h/.claude/settings.json".into()));
        assert!(p.hidden.contains(&"/h/.ssh".into()));
        assert!(p.hidden.contains(&"/h/.codex/auth.json".into()));
        assert!(p.hidden.contains(&"/h/.pi/agent/auth.json".into()));
        assert!(p.hidden.contains(&"/h/.zeron/agent-accounts".into()));
        assert!(!p.hidden.contains(&"/h/.claude/.credentials.json".into()));
        // Claude Code's login lives in the Keychain.
        assert!(!p.hidden.contains(&"/h/Library/Keychains".into()));
        assert_eq!(p.env, vec![("DISABLE_AUTOUPDATER".into(), "1".into())]);
    }

    #[test]
    fn codex_hides_the_keychain_and_claudes_credentials() {
        let p = agent_paths_for(HarnessId::Codex, Path::new("/h"), Platform::MacOs, &no_env);
        assert_eq!(p.writable, vec![PathBuf::from("/h/.codex")]);
        assert!(p.hidden.contains(&"/h/Library/Keychains".into()));
        assert!(p.hidden.contains(&"/h/.claude/.credentials.json".into()));
        assert!(!p.hidden.contains(&"/h/.codex/auth.json".into()));
        assert!(p.read_only.contains(&"/h/.codex/packages".into()));
    }

    #[test]
    fn env_overrides_move_state_and_credentials() {
        let env = |name: &str| -> Option<OsString> {
            match name {
                "CLAUDE_CONFIG_DIR" => Some("/cfg/claude".into()),
                "CODEX_HOME" => Some("/cfg/codex".into()),
                "XDG_DATA_HOME" => Some("/xdg/data".into()),
                "ZERON_DATA_DIR" => Some("relative/ignored".into()),
                _ => None,
            }
        };
        let p = agent_paths_for(
            HarnessId::ClaudeCode,
            Path::new("/h"),
            Platform::Linux,
            &env,
        );
        assert_eq!(
            p.writable,
            vec![
                PathBuf::from("/cfg/claude"),
                "/h/.cache/claude-cli-nodejs".into()
            ]
        );
        // With CLAUDE_CONFIG_DIR the state file lives inside the config dir.
        assert!(p.writable_prefixes.is_empty());
        assert!(p.hidden.contains(&"/cfg/codex/auth.json".into()));
        assert!(p.hidden.contains(&"/xdg/data/opencode/auth.json".into()));
        assert!(p.hidden.contains(&"/h/.zeron/session.json".into()));
        assert!(p.hidden.contains(&"/h/.local/share/keyrings".into()));
        assert!(!p.hidden.iter().any(|h| h.starts_with("/h/Library")));
    }

    #[test]
    fn git_paths_cover_linked_worktrees() {
        let root = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(root.path()).unwrap();
        let repo = root.join("repo");
        let common = repo.join(".git");
        let wt_git = common.join("worktrees/feature");
        std::fs::create_dir_all(&wt_git).unwrap();
        std::fs::write(wt_git.join("commondir"), "../..\n").unwrap();
        let worktree = root.join("wt");
        std::fs::create_dir_all(worktree.join("src")).unwrap();
        std::fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\n", wt_git.display()),
        )
        .unwrap();

        let p = git_paths(&worktree.join("src"));
        assert_eq!(p.writable, vec![wt_git.clone(), common.clone()]);
        assert_eq!(
            p.read_only,
            vec![
                common.join("hooks"),
                common.join("config"),
                wt_git.join("config.worktree")
            ]
        );

        // A plain checkout: everything is inside the workspace already.
        let p = git_paths(&repo);
        assert!(p.writable.is_empty());
        assert_eq!(
            p.read_only,
            vec![common.join("hooks"), common.join("config")]
        );

        assert_eq!(git_paths(&root.join("elsewhere")), AgentPaths::default());
    }
}
