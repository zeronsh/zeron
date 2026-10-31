//! Build one engine revision into an immutable `bins/<sha12>/` cache.
//!
//! Worktrees are `~/Documents/dev/zeron-wt/zb-<sha12>` and cargo targets are
//! `~/Documents/dev/zeron-wt/targets/zb-<sha12>`. Neither is the user's
//! checkout. `cargo build` is not passed `-j`: that would leave the shared
//! jobserver. The job cap travels in `CARGO_MAKEFLAGS` / `MAKEFLAGS`
//! (`-j12 --jobserver-auth=fifo:…`), which is the pool every perf worker
//! draws from.
//!
//! A second call reuses `bins/<sha12>/zeron-engine` while its sha256 still
//! matches `build.json`. That path does not spawn cargo.

use std::fs::{self, File};
use std::io::Read;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::env::passwd_home;

/// `-j12` here is the jobserver token count, not a cargo `--jobs` flag.
pub const JOBSERVER_SLOTS: &str = "-j12";

#[derive(Clone, Debug)]
pub struct BuildRequest {
    /// Git revision: branch, tag, or commit, as `git rev-parse` accepts it.
    pub rev: String,
    /// Any worktree of the zeron repo. `git worktree add` runs against it.
    pub repo: PathBuf,
    /// Receives `bins/<sha12>/`. Defaults to `ZB_ROOT` at the CLI.
    pub zb_root: PathBuf,
    /// Parent of `zb-<sha12>` worktrees. Production: `~/Documents/dev/zeron-wt`.
    pub worktree_parent: PathBuf,
    /// Parent of `zb-<sha12>` target dirs. Production: `~/Documents/dev/zeron-wt/targets`.
    pub target_parent: PathBuf,
    /// Shared jobserver fifo. Cargo is not started when this path is missing
    /// and the binary is not already cached.
    pub jobserver_fifo: PathBuf,
    /// Replaces `PATH` for the cargo invocation. `None` filters the current
    /// `PATH` and drops any `swiftly` entry.
    pub path_override: Option<std::ffi::OsString>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuiltBinary {
    pub rev: String,
    pub sha: String,
    pub sha12: String,
    pub sha256: String,
    pub path: PathBuf,
    pub reused: bool,
    pub worktree: PathBuf,
    pub target_dir: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct BuildJson {
    rev: String,
    sha: String,
    sha12: String,
    sha256: String,
    profile: String,
    features: String,
    rustc: String,
    path: String,
    worktree: String,
    target_dir: String,
}

pub fn default_worktree_parent() -> PathBuf {
    home().join("Documents/dev/zeron-wt")
}

pub fn default_target_parent() -> PathBuf {
    default_worktree_parent().join("targets")
}

pub fn default_jobserver_fifo() -> PathBuf {
    home().join("Documents/dev/tmp/zeron-perf/jobserver/fifo")
}

fn home() -> PathBuf {
    passwd_home().unwrap_or_else(|| PathBuf::from("/home/user"))
}

pub fn jobserver_makeflags(fifo: &Path) -> String {
    format!("{JOBSERVER_SLOTS} --jobserver-auth=fifo:{}", fifo.display())
}

/// Resolve `rev`, reuse a matching cached engine, or build it.
pub fn build_revision(req: &BuildRequest) -> Result<BuiltBinary> {
    reject_forbidden(&req.zb_root)?;
    reject_forbidden(&req.worktree_parent)?;
    reject_forbidden(&req.target_parent)?;
    let sha = git_commit(&req.repo, &req.rev)?;
    let sha12 = sha12_of(&sha)?;
    let worktree = req.worktree_parent.join(format!("zb-{sha12}"));
    let target_dir = req.target_parent.join(format!("zb-{sha12}"));
    let bin_dir = req.zb_root.join("bins").join(&sha12);
    let bin_path = bin_dir.join("zeron-engine");
    let meta_path = bin_dir.join("build.json");
    if let Some(sha256) = cache_hit(&bin_path, &meta_path, &sha) {
        return Ok(BuiltBinary {
            rev: req.rev.clone(),
            sha,
            sha12,
            sha256,
            path: bin_path,
            reused: true,
            worktree,
            target_dir,
        });
    }
    if !req.jobserver_fifo.exists() {
        bail!(
            "jobserver fifo {} is missing; refusing to build without the shared pool",
            req.jobserver_fifo.display()
        );
    }
    ensure_worktree(&req.repo, &worktree, &sha)?;
    let path_env = match &req.path_override {
        Some(path) => path_without_swiftly(path),
        None => path_without_swiftly(&std::env::var_os("PATH").unwrap_or_default()),
    };
    let cargo =
        find_on_path("cargo", &path_env).context("cargo is not on PATH after dropping swiftly")?;
    let makeflags = jobserver_makeflags(&req.jobserver_fifo);
    // No `-j` / `--jobs`. See the module comment.
    let args = ["build", "--release", "-p", "zeron", "--bin", "zeron-engine"];
    debug_assert!(
        args.iter()
            .all(|arg| *arg != "--jobs" && !arg.starts_with("-j"))
    );
    let output = Command::new(&cargo)
        .args(args)
        .current_dir(&worktree)
        .env("CARGO_TARGET_DIR", &target_dir)
        .env("CARGO_MAKEFLAGS", &makeflags)
        .env("MAKEFLAGS", &makeflags)
        .env("PATH", &path_env)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("CARGO_BUILD_JOBS")
        .output()
        .with_context(|| format!("spawn cargo in {}", worktree.display()))?;
    if !output.status.success() {
        bail!(
            "cargo build --release -p zeron --bin zeron-engine failed (status {}):\n{}",
            output.status,
            tail_text(&output.stderr, &output.stdout)
        );
    }
    let built = target_dir.join("release").join("zeron-engine");
    if !built.is_file() {
        bail!("cargo succeeded but {} is missing", built.display());
    }
    fs::create_dir_all(&bin_dir).with_context(|| format!("create {}", bin_dir.display()))?;
    publish_file(&built, &bin_path)?;
    let sha256 = sha256_file(&bin_path)?;
    let rustc = rustc_verbose(&worktree, &path_env).unwrap_or_else(|_| "unknown".into());
    let meta = BuildJson {
        rev: req.rev.clone(),
        sha: sha.clone(),
        sha12: sha12.clone(),
        sha256: sha256.clone(),
        profile: "release".into(),
        features: String::new(),
        rustc,
        path: bin_path.display().to_string(),
        worktree: worktree.display().to_string(),
        target_dir: target_dir.display().to_string(),
    };
    let body = serde_json::to_vec_pretty(&meta).context("encode build.json")?;
    let meta_tmp = bin_dir.join("build.json.tmp");
    fs::write(&meta_tmp, body).with_context(|| format!("write {}", meta_tmp.display()))?;
    fs::rename(&meta_tmp, &meta_path).with_context(|| format!("rename {}", meta_path.display()))?;
    Ok(BuiltBinary {
        rev: req.rev.clone(),
        sha,
        sha12,
        sha256,
        path: bin_path,
        reused: false,
        worktree,
        target_dir,
    })
}

/// Two distinct files with the same bytes, for a self-pair control.
pub fn distinct_copies(engine: &Path, dir: &Path) -> Result<(PathBuf, PathBuf, String)> {
    let a = dir.join("a").join("zeron-engine");
    let b = dir.join("b").join("zeron-engine");
    publish_file(engine, &a)?;
    publish_file(engine, &b)?;
    let sha_a = sha256_file(&a)?;
    let sha_b = sha256_file(&b)?;
    if sha_a != sha_b {
        bail!("control copies of {} differ", engine.display());
    }
    if a == b {
        bail!("control copies are the same path");
    }
    Ok((a, b, sha_a))
}

fn cache_hit(bin: &Path, meta_path: &Path, sha: &str) -> Option<String> {
    let text = fs::read_to_string(meta_path).ok()?;
    let meta: BuildJson = serde_json::from_str(&text).ok()?;
    if meta.sha != sha || meta.profile != "release" {
        return None;
    }
    let got = sha256_file(bin).ok()?;
    if got != meta.sha256 {
        return None;
    }
    Some(got)
}

fn ensure_worktree(repo: &Path, worktree: &Path, sha: &str) -> Result<()> {
    if worktree.join(".git").exists() {
        let head = git_commit(worktree, "HEAD")?;
        if head != sha {
            bail!(
                "worktree {} is at {head}, not {sha}; refusing to overwrite it",
                worktree.display()
            );
        }
        return Ok(());
    }
    if worktree.exists() {
        bail!("{} exists and is not a git worktree", worktree.display());
    }
    if let Some(parent) = worktree.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let output = Command::new("/usr/bin/git")
        .arg("-C")
        .arg(repo)
        .args(["worktree", "add", "--detach"])
        .arg(worktree)
        .arg(sha)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .context("spawn git worktree add")?;
    if !output.status.success() {
        bail!(
            "git worktree add {} {sha} failed: {}",
            worktree.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

fn git_commit(repo: &Path, rev: &str) -> Result<String> {
    let spec = format!("{rev}^{{commit}}");
    let output = Command::new("/usr/bin/git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "--verify"])
        .arg(&spec)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .with_context(|| format!("spawn git rev-parse {spec}"))?;
    if !output.status.success() {
        bail!(
            "git rev-parse {spec} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let sha = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if sha.len() < 12 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("git rev-parse returned {sha:?}");
    }
    Ok(sha)
}

fn sha12_of(sha: &str) -> Result<String> {
    if sha.len() < 12 {
        bail!("commit {sha} is shorter than 12 hex digits");
    }
    Ok(sha[..12].to_string())
}

fn publish_file(src: &Path, dest: &Path) -> Result<()> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let tmp = dest.with_extension("tmp");
    fs::copy(src, &tmp).with_context(|| format!("copy {} to {}", src.display(), tmp.display()))?;
    let mut perms = fs::metadata(&tmp)?.permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&tmp, perms)?;
    fs::rename(&tmp, dest).with_context(|| format!("rename {}", dest.display()))?;
    Ok(())
}

pub(crate) fn sha256_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex(&hasher.finalize())
}

pub(crate) fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file
            .read(&mut buf)
            .with_context(|| format!("read {}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    const H: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(H[(byte >> 4) as usize] as char);
        out.push(H[(byte & 0xf) as usize] as char);
    }
    out
}

fn rustc_verbose(worktree: &Path, path_env: &std::ffi::OsStr) -> Result<String> {
    let rustc = find_on_path("rustc", path_env).context("rustc not on PATH")?;
    let output = Command::new(rustc)
        .arg("-vV")
        .current_dir(worktree)
        .env("PATH", path_env)
        .output()
        .context("spawn rustc -vV")?;
    if !output.status.success() {
        bail!("rustc -vV failed");
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn path_without_swiftly(path: &std::ffi::OsStr) -> std::ffi::OsString {
    let mut out = Vec::new();
    for dir in path.as_bytes().split(|b| *b == b':') {
        let text = String::from_utf8_lossy(dir);
        if text.to_ascii_lowercase().contains("swiftly") {
            continue;
        }
        if !out.is_empty() {
            out.push(b':');
        }
        out.extend_from_slice(dir);
    }
    std::ffi::OsString::from_vec(out)
}

fn find_on_path(name: &str, path: &std::ffi::OsStr) -> Option<PathBuf> {
    for dir in path.as_bytes().split(|b| *b == b':') {
        if dir.is_empty() {
            continue;
        }
        let candidate = Path::new(std::ffi::OsStr::from_bytes(dir)).join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn reject_forbidden(path: &Path) -> Result<()> {
    let text = path.to_string_lossy();
    if text == "/tmp" || text.starts_with("/tmp/") {
        bail!("{} is under /tmp", path.display());
    }
    if path.components().any(|c| c.as_os_str() == ".zeron") {
        bail!("{} contains a .zeron component", path.display());
    }
    Ok(())
}

fn tail_text(stderr: &[u8], stdout: &[u8]) -> String {
    let mut combined = String::from_utf8_lossy(stderr).into_owned();
    if !stdout.is_empty() {
        combined.push('\n');
        combined.push_str(&String::from_utf8_lossy(stdout));
    }
    if combined.len() > 4000 {
        combined = combined[combined.len() - 4000..].to_string();
    }
    combined
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_live_under_zeron_wt() {
        let root = default_worktree_parent();
        assert!(root.ends_with("Documents/dev/zeron-wt"), "{root:?}");
        assert!(
            default_target_parent().ends_with("Documents/dev/zeron-wt/targets"),
            "{:?}",
            default_target_parent()
        );
        let flags = jobserver_makeflags(Path::new(
            "/home/user/Documents/dev/tmp/zeron-perf/jobserver/fifo",
        ));
        assert_eq!(
            flags,
            "-j12 --jobserver-auth=fifo:/home/user/Documents/dev/tmp/zeron-perf/jobserver/fifo"
        );
        assert!(reject_forbidden(Path::new("/tmp/zb")).is_err());
    }

    #[test]
    fn second_call_reuses_the_binary_without_cargo() {
        let scratch = scratch_dir("build-reuse");
        let repo = scratch.join("repo");
        let wt_parent = scratch.join("wt");
        let target_parent = scratch.join("targets");
        let zb_root = scratch.join("zb");
        let stub = scratch.join("stub");
        let log = scratch.join("cargo.log");
        fs::create_dir_all(&stub).unwrap();
        fs::write(
            stub.join("cargo"),
            format!(
                "#!/bin/sh\nlog={}\nprintf 'argv:' >> \"$log\"\nfor a in \"$@\"; do printf ' [%s]' \"$a\" >> \"$log\"; done\nprintf '\\nMAKEFLAGS=%s\\nTARGET=%s\\nPATH=%s\\n' \"$CARGO_MAKEFLAGS\" \"$CARGO_TARGET_DIR\" \"$PATH\" >> \"$log\"\nfor a in \"$@\"; do\n  case \"$a\" in\n    -j|-j*|--jobs) echo rejected-j >> \"$log\"; exit 3 ;;\n  esac\ndone\nmkdir -p \"$CARGO_TARGET_DIR/release\"\nprintf 'engine-v1' > \"$CARGO_TARGET_DIR/release/zeron-engine\"\n",
                shell_quote(&log)
            ),
        )
        .unwrap();
        let mut perms = fs::metadata(stub.join("cargo")).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(stub.join("cargo"), perms).unwrap();
        fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-b", "main"]);
        fs::write(repo.join("README"), "bench\n").unwrap();
        git(&repo, &["add", "README"]);
        git(&repo, &["commit", "-m", "init"]);
        let fifo = scratch.join("fifo");
        assert!(
            Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .unwrap()
                .success()
        );
        let path = {
            let mut p = std::ffi::OsString::from(stub.as_os_str());
            p.push(":/opt/swiftly/bin:/usr/bin:/bin");
            p
        };
        let req = BuildRequest {
            rev: "HEAD".into(),
            repo: repo.clone(),
            zb_root: zb_root.clone(),
            worktree_parent: wt_parent.clone(),
            target_parent: target_parent.clone(),
            jobserver_fifo: fifo,
            path_override: Some(path),
        };
        let first = build_revision(&req).unwrap();
        assert!(!first.reused);
        assert!(first.path.is_file());
        assert_eq!(first.sha256, sha256_file(&first.path).unwrap());
        assert!(first.worktree.starts_with(&wt_parent));
        assert!(first.worktree.ends_with(format!("zb-{}", first.sha12)));
        assert!(first.target_dir.starts_with(&target_parent));
        assert!(first.target_dir.ends_with(format!("zb-{}", first.sha12)));
        let log_text = fs::read_to_string(&log).unwrap();
        assert_eq!(log_text.matches("argv:").count(), 1, "{log_text}");
        assert!(log_text.contains("[build]"), "{log_text}");
        assert!(log_text.contains("[--release]"), "{log_text}");
        assert!(!log_text.contains("[-j"), "{log_text}");
        assert!(log_text.contains("--jobserver-auth=fifo:"), "{log_text}");
        assert!(!log_text.contains("swiftly"), "{log_text}");
        let lines_after_first = log_text.lines().count();
        let second = build_revision(&req).unwrap();
        assert!(second.reused);
        assert_eq!(second.sha256, first.sha256);
        assert_eq!(second.path, first.path);
        assert_eq!(
            fs::read_to_string(&log).unwrap().lines().count(),
            lines_after_first,
            "second call invoked cargo"
        );
        // A byte change invalidates the cache and cargo runs again.
        fs::write(&first.path, b"tampered").unwrap();
        let third = build_revision(&req).unwrap();
        assert!(!third.reused);
        assert_eq!(fs::read(&third.path).unwrap(), b"engine-v1");
        assert!(fs::read_to_string(&log).unwrap().matches("argv:").count() >= 2);
    }

    fn shell_quote(path: &Path) -> String {
        format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
    }

    fn git(repo: &Path, args: &[&str]) {
        let status = Command::new("/usr/bin/git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "zeron-bench")
            .env("GIT_AUTHOR_EMAIL", "bench@example.invalid")
            .env("GIT_COMMITTER_NAME", "zeron-bench")
            .env("GIT_COMMITTER_EMAIL", "bench@example.invalid")
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }

    fn scratch_dir(name: &str) -> PathBuf {
        let dir = PathBuf::from("/home/user")
            .join("Documents/dev/tmp/zeron-perf/scratch/b6-ab-orchestrator/tests")
            .join(format!("{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }
}
