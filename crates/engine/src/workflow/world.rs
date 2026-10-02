//! The world a workflow script can touch: its shell gates and read-only views
//! of the project. Everything here is blocking (call it from a blocking
//! thread) and **confined to the project root**.
//!
//! * `run` starts one program — no shell — with the project root (or a
//!   sub-folder of it) as its working directory, kills it at its timeout or
//!   when the run is cancelled, and keeps at most [`MAX_RUN_OUTPUT_BYTES`] of
//!   each stream.
//! * `files.*` / `git.*` reads never truncate silently: a result past the cap
//!   is an error telling the script to narrow the request.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use zeron_workflow::host::{ReadOp, RunReply, RunRequest};
use zeron_workflow::limits::{MAX_READ_BYTES, MAX_READ_ENTRIES, MAX_RUN_OUTPUT_BYTES};

use super::store::confine;

/// Files larger than this are skipped by `files.grep`.
const GREP_MAX_FILE_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone)]
pub struct World {
    root: PathBuf,
}

impl World {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    // ── commands ───────────────────────────────────────────────────────────

    pub fn run(&self, req: &RunRequest, cancel: &Arc<AtomicBool>) -> RunReply {
        let cwd = match req.cwd.as_deref() {
            None => self.root.clone(),
            Some(rel) => match confine(&self.root, rel) {
                Ok(p) if p.is_dir() => p,
                Ok(_) => return start_error(format!("cwd {rel:?} is not a directory")),
                Err(e) => return start_error(e),
            },
        };
        let mut command = Command::new(&req.program);
        command
            .args(&req.args)
            .current_dir(&cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            // Its own process group, so a timeout kills what it spawned too.
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let mut child = match command.spawn() {
            Ok(c) => c,
            Err(e) => return start_error(format!("could not start {:?}: {e}", req.program)),
        };
        let out = child.stdout.take().map(capped_reader);
        let err = child.stderr.take().map(capped_reader);
        let deadline = Instant::now() + Duration::from_secs(req.timeout_s);
        let mut timed_out = false;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) => {}
                Err(_) => break None,
            }
            if cancel.load(Ordering::Relaxed) || Instant::now() >= deadline {
                timed_out = !cancel.load(Ordering::Relaxed);
                kill_tree(&mut child);
                break child.wait().ok();
            }
            std::thread::sleep(Duration::from_millis(15));
        };
        let (stdout, out_truncated) = out
            .map(|h| h.join().unwrap_or_default())
            .unwrap_or_default();
        let (stderr, err_truncated) = err
            .map(|h| h.join().unwrap_or_default())
            .unwrap_or_default();
        RunReply {
            exit_code: if timed_out {
                None
            } else {
                status.and_then(|s| s.code())
            },
            stdout: String::from_utf8_lossy(&stdout).into_owned(),
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
            timed_out,
            truncated: out_truncated || err_truncated,
            start_error: None,
        }
    }

    // ── reads ──────────────────────────────────────────────────────────────

    pub fn read(&self, op: &ReadOp) -> Result<Value, String> {
        match op {
            ReadOp::Glob { pattern } => self.glob(pattern),
            ReadOp::Read { path } => self.read_file(path),
            ReadOp::Grep { pattern, glob } => self.grep(pattern, glob.as_deref()),
            ReadOp::GitChangedFiles { base } => self.git_changed(base.as_deref()),
            ReadOp::GitDiff { base, path } => self.git_diff(base.as_deref(), path.as_deref()),
            ReadOp::GitStatus => self.git_status(),
            ReadOp::GitLog { limit, path } => self.git_log(*limit, path.as_deref()),
        }
    }

    fn walker(&self, glob: Option<&str>) -> Result<ignore::Walk, String> {
        let mut builder = ignore::WalkBuilder::new(&self.root);
        builder
            .standard_filters(true)
            .hidden(false)
            .follow_links(false);
        builder.filter_entry(|e| e.file_name() != ".git");
        if let Some(glob) = glob {
            let mut ov = ignore::overrides::OverrideBuilder::new(&self.root);
            ov.add(glob)
                .map_err(|e| format!("bad glob {glob:?}: {e}"))?;
            builder.overrides(ov.build().map_err(|e| format!("bad glob {glob:?}: {e}"))?);
        }
        Ok(builder.build())
    }

    fn relative(&self, p: &Path) -> String {
        p.strip_prefix(&self.root)
            .unwrap_or(p)
            .to_string_lossy()
            .replace('\\', "/")
    }

    fn glob(&self, pattern: &str) -> Result<Value, String> {
        let mut files = Vec::new();
        for entry in self.walker(Some(pattern))?.flatten() {
            if entry.file_type().is_some_and(|t| t.is_file()) {
                files.push(self.relative(entry.path()));
                if files.len() > MAX_READ_ENTRIES {
                    return Err(format!(
                        "{pattern:?} matches more than {MAX_READ_ENTRIES} files; narrow the pattern"
                    ));
                }
            }
        }
        files.sort();
        Ok(json!(files))
    }

    fn read_file(&self, path: &str) -> Result<Value, String> {
        let candidate = self.root.join(path);
        // Escapes are errors; a plain missing file is None.
        if let Err(e) = confine(&self.root, path)
            && candidate.exists()
        {
            return Err(e);
        }
        let Ok(full) = confine(&self.root, path) else {
            if path.contains("..") || Path::new(path).is_absolute() {
                return Err(format!("{path:?} is outside the project"));
            }
            return Ok(Value::Null);
        };
        if !full.is_file() {
            return Err(format!("{path:?} is not a file"));
        }
        let size = std::fs::metadata(&full).map_err(|e| e.to_string())?.len();
        if size as usize > MAX_READ_BYTES {
            return Err(format!(
                "{path:?} is {size} bytes; files.read returns at most {MAX_READ_BYTES} (read a slice with run(\"head\", ...), or grep for what you need)"
            ));
        }
        let bytes = std::fs::read(&full).map_err(|e| e.to_string())?;
        String::from_utf8(bytes)
            .map(Value::String)
            .map_err(|_| format!("{path:?} is not UTF-8 text"))
    }

    fn grep(&self, pattern: &str, glob: Option<&str>) -> Result<Value, String> {
        let re = regex::RegexBuilder::new(pattern)
            .size_limit(1 << 20)
            .build()
            .map_err(|e| format!("bad regular expression: {e}"))?;
        let mut hits = Vec::new();
        let mut paths: Vec<PathBuf> = Vec::new();
        for entry in self.walker(glob)?.flatten() {
            if entry.file_type().is_some_and(|t| t.is_file()) {
                paths.push(entry.into_path());
            }
        }
        paths.sort();
        for path in paths {
            let Ok(meta) = std::fs::metadata(&path) else {
                continue;
            };
            if meta.len() > GREP_MAX_FILE_BYTES {
                continue;
            }
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            if bytes[..bytes.len().min(8192)].contains(&0) {
                continue; // binary
            }
            let text = String::from_utf8_lossy(&bytes);
            for (n, line) in text.lines().enumerate() {
                if re.is_match(line) {
                    hits.push(json!({
                        "path": self.relative(&path),
                        "line": n + 1,
                        "text": line.chars().take(240).collect::<String>(),
                    }));
                    if hits.len() > MAX_READ_ENTRIES {
                        return Err(format!(
                            "{pattern:?} matches more than {MAX_READ_ENTRIES} lines; narrow the pattern or the glob"
                        ));
                    }
                }
            }
        }
        Ok(json!(hits))
    }

    fn git(&self, args: &[&str]) -> Result<String, String> {
        let out = Command::new("git")
            .args(args)
            .current_dir(&self.root)
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::null())
            .output()
            .map_err(|e| format!("could not run git: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "git {} failed: {}",
                args.first().copied().unwrap_or_default(),
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    fn capped_lines(&self, text: String, what: &str) -> Result<Value, String> {
        let lines: Vec<&str> = text.lines().filter(|l| !l.is_empty()).collect();
        if lines.len() > MAX_READ_ENTRIES {
            return Err(format!(
                "{what} has {} entries; the limit is {MAX_READ_ENTRIES}",
                lines.len()
            ));
        }
        Ok(json!(lines))
    }

    fn git_changed(&self, base: Option<&str>) -> Result<Value, String> {
        let mut names = self.git(&["diff", "--name-only", base.unwrap_or("HEAD"), "--"])?;
        if base.is_none() {
            names.push('\n');
            names.push_str(&self.git(&["ls-files", "--others", "--exclude-standard"])?);
        }
        let mut files: Vec<&str> = names.lines().filter(|l| !l.is_empty()).collect();
        files.sort_unstable();
        files.dedup();
        if files.len() > MAX_READ_ENTRIES {
            return Err(format!(
                "{} changed files; the limit is {MAX_READ_ENTRIES}",
                files.len()
            ));
        }
        Ok(json!(files))
    }

    fn git_diff(&self, base: Option<&str>, path: Option<&str>) -> Result<Value, String> {
        let mut args = vec![
            "diff",
            "--no-color",
            "--no-ext-diff",
            base.unwrap_or("HEAD"),
            "--",
        ];
        if let Some(p) = path {
            args.push(p);
        }
        let text = self.git(&args)?;
        if text.len() > MAX_READ_BYTES {
            return Err(format!(
                "the diff is {} bytes; git.diff returns at most {MAX_READ_BYTES} (pass path=... for one file at a time)",
                text.len()
            ));
        }
        Ok(Value::String(text))
    }

    fn git_status(&self) -> Result<Value, String> {
        let text = self.git(&["status", "--porcelain=v1"])?;
        self.capped_lines(text, "git status")
    }

    fn git_log(&self, limit: u32, path: Option<&str>) -> Result<Value, String> {
        let n = format!("-n{limit}");
        let mut args = vec!["log", &n, "--format=%H%x1f%s%x1f%an%x1f%aI", "--"];
        if let Some(p) = path {
            args.push(p);
        }
        let text = self.git(&args)?;
        let entries: Vec<Value> = text
            .lines()
            .filter_map(|l| {
                let mut f = l.split('\u{1f}');
                Some(json!({
                    "hash": f.next()?,
                    "subject": f.next()?,
                    "author": f.next()?,
                    "date": f.next()?,
                }))
            })
            .collect();
        Ok(json!(entries))
    }
}

fn start_error(message: String) -> RunReply {
    RunReply {
        start_error: Some(message),
        ..RunReply::default()
    }
}

/// Read a stream on its own thread, keeping at most the output cap.
fn capped_reader<R: Read + Send + 'static>(
    mut reader: R,
) -> std::thread::JoinHandle<(Vec<u8>, bool)> {
    std::thread::spawn(move || {
        let mut kept = Vec::new();
        let mut truncated = false;
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let room = MAX_RUN_OUTPUT_BYTES.saturating_sub(kept.len());
                    kept.extend_from_slice(&buf[..n.min(room)]);
                    if n > room {
                        truncated = true; // keep draining so the child never blocks
                    }
                }
            }
        }
        (kept, truncated)
    })
}

#[cfg(unix)]
fn kill_tree(child: &mut std::process::Child) {
    // Negative pid: the whole process group created at spawn.
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    let _ = child.kill();
}

#[cfg(not(unix))]
fn kill_tree(child: &mut std::process::Child) {
    let _ = child.kill();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> (tempfile::TempDir, World) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("proj");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/a.rs"), "fn a() {}\n// TODO one\n").unwrap();
        std::fs::write(root.join("src/b.rs"), "fn b() {}\n").unwrap();
        std::fs::write(root.join("README.md"), "hello\n").unwrap();
        std::fs::write(dir.path().join("secret.txt"), "top secret").unwrap();
        let world = World::new(root.canonicalize().unwrap());
        (dir, world)
    }

    fn req(program: &str, args: &[&str], timeout_s: u64) -> RunRequest {
        RunRequest {
            key: zeron_workflow::site::SiteKey {
                site: "s".into(),
                ordinal: 0,
            },
            program: program.into(),
            args: args.iter().map(ToString::to_string).collect(),
            cwd: None,
            timeout_s,
        }
    }

    #[test]
    fn globs_greps_and_reads_stay_inside_the_project() {
        let (_dir, world) = project();
        assert_eq!(
            world
                .read(&ReadOp::Glob {
                    pattern: "src/**/*.rs".into()
                })
                .unwrap(),
            json!(["src/a.rs", "src/b.rs"])
        );
        let hits = world
            .read(&ReadOp::Grep {
                pattern: "TODO".into(),
                glob: Some("*.rs".into()),
            })
            .unwrap();
        assert_eq!(hits[0]["path"], "src/a.rs");
        assert_eq!(hits[0]["line"], 2);
        assert_eq!(
            world
                .read(&ReadOp::Read {
                    path: "README.md".into()
                })
                .unwrap(),
            json!("hello\n")
        );
        assert_eq!(
            world
                .read(&ReadOp::Read {
                    path: "nope.txt".into()
                })
                .unwrap(),
            Value::Null
        );
        for escape in ["../secret.txt", "/etc/passwd", "src/../../secret.txt"] {
            assert!(
                world
                    .read(&ReadOp::Read {
                        path: escape.into()
                    })
                    .is_err(),
                "{escape}"
            );
        }
        assert!(
            world
                .read(&ReadOp::Grep {
                    pattern: "(".into(),
                    glob: None
                })
                .is_err()
        );
    }

    #[test]
    fn reads_over_the_caps_are_rejected_not_truncated() {
        let (_dir, world) = project();
        let big = "x".repeat(MAX_READ_BYTES + 1);
        std::fs::write(world.root().join("big.txt"), big).unwrap();
        let err = world
            .read(&ReadOp::Read {
                path: "big.txt".into(),
            })
            .unwrap_err();
        assert!(err.contains("at most"), "{err}");
        for i in 0..(MAX_READ_ENTRIES + 5) {
            std::fs::write(world.root().join("src").join(format!("f{i}.txt")), "").unwrap();
        }
        let err = world
            .read(&ReadOp::Glob {
                pattern: "src/*.txt".into(),
            })
            .unwrap_err();
        assert!(err.contains("narrow the pattern"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn commands_return_values_and_are_confined_and_bounded() {
        let (_dir, world) = project();
        let cancel = Arc::new(AtomicBool::new(false));
        let ok = world.run(
            &req("sh", &["-c", "echo out; echo err >&2; exit 3"], 10),
            &cancel,
        );
        assert_eq!(ok.exit_code, Some(3));
        assert_eq!(ok.stdout.trim(), "out");
        assert_eq!(ok.stderr.trim(), "err");
        assert!(!ok.timed_out);
        // cwd is a sub-folder of the project; anything else is refused.
        let mut r = req("pwd", &[], 10);
        r.cwd = Some("src".into());
        assert!(world.run(&r, &cancel).stdout.trim().ends_with("proj/src"));
        r.cwd = Some("../".into());
        assert!(world.run(&r, &cancel).start_error.is_some());
        // Missing program: a value, not a panic.
        assert!(
            world
                .run(&req("definitely-not-a-program", &[], 5), &cancel)
                .start_error
                .is_some()
        );
        // Timeout kills the whole group.
        let started = Instant::now();
        let slow = world.run(&req("sh", &["-c", "sleep 30 & sleep 30"], 1), &cancel);
        assert!(slow.timed_out && slow.exit_code.is_none());
        assert!(started.elapsed() < Duration::from_secs(10));
        // Output is capped.
        let flood = world.run(&req("sh", &["-c", "yes x | head -c 400000"], 10), &cancel);
        assert!(flood.truncated);
        assert_eq!(flood.stdout.len(), MAX_RUN_OUTPUT_BYTES);
        // Cancellation stops it promptly.
        let cancel2 = Arc::new(AtomicBool::new(false));
        let c = cancel2.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            c.store(true, Ordering::SeqCst);
        });
        let started = Instant::now();
        let r = world.run(&req("sleep", &["30"], 60), &cancel2);
        assert!(!r.timed_out);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn git_reads_in_a_repository() {
        let (_dir, world) = project();
        let git = |args: &[&str]| {
            Command::new("git")
                .args(args)
                .current_dir(world.root())
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        };
        if !git(&["init", "-q"]) {
            return; // no git on this machine
        }
        assert!(git(&["add", "."]) && git(&["commit", "-q", "-m", "first"]));
        std::fs::write(world.root().join("src/a.rs"), "fn a() {}\n// changed\n").unwrap();
        std::fs::write(world.root().join("new.txt"), "n").unwrap();
        let changed = world.read(&ReadOp::GitChangedFiles { base: None }).unwrap();
        assert_eq!(changed, json!(["new.txt", "src/a.rs"]));
        let diff = world
            .read(&ReadOp::GitDiff {
                base: None,
                path: Some("src/a.rs".into()),
            })
            .unwrap();
        assert!(diff.as_str().unwrap().contains("+// changed"));
        let status = world.read(&ReadOp::GitStatus).unwrap();
        assert!(status.as_array().unwrap().len() >= 2);
        let log = world
            .read(&ReadOp::GitLog {
                limit: 5,
                path: None,
            })
            .unwrap();
        assert_eq!(log[0]["subject"], "first");
    }
}
