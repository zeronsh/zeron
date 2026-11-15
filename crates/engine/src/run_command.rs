//! `RunCommand`: one shell command run to its end on this device, for an
//! agent on this device or another of the person's (Zeron MCP's
//! `run_command`, forwarded by `targetDeviceId`).
//!
//! It works like an agent's own shell tool, not a terminal: no PTY and no
//! session. The user's shell runs the command as a login shell (`-l -c`), so
//! it gets the same PATH as their terminal. Stdin is empty, so a prompt for
//! input ends instead of hanging. The reply carries the output and the exit
//! code. What it costs is bounded:
//!
//! - **Memory**: at most [`HEAD`] + [`TAIL`] bytes of output are kept per run
//!   (the start and the end); what's between is counted, not kept.
//! - **Time**: every run has a timeout (at most [`MAX_TIMEOUT`]). When it
//!   expires, or the call is dropped, the command's whole process group is
//!   killed (Unix), so nothing it started lives on.
//! - **Concurrency**: at most [`MAX_RUNNING`] commands at once per device;
//!   beyond that a run is refused, not queued.
//!
//! Something the command deliberately puts in the background (`nohup x &`)
//! outlives a run that ends normally, as it would in a terminal. The run
//! stops reading output [`DRAIN`] after the shell exits, so such a process
//! never holds the reply open.

use std::process::Stdio;
use std::time::{Duration, Instant};

use serde::Serialize;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::sync::Semaphore;

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
pub const MAX_TIMEOUT: Duration = Duration::from_secs(600);
/// Output kept from the start of a run, and from its end.
const HEAD: usize = 16 * 1024;
const TAIL: usize = 48 * 1024;
/// How long output is still read once the shell has exited.
const DRAIN: Duration = Duration::from_millis(250);
pub const MAX_RUNNING: usize = 16;

static RUNNING: Semaphore = Semaphore::const_new(MAX_RUNNING);

/// How one run ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunOutcome {
    /// `None` when it was killed (timed out, or by a signal).
    pub exit_code: Option<i32>,
    /// Stdout and stderr together, in the order they arrived. When it was
    /// too long, its start and end with a line saying how much was left out.
    pub output: String,
    /// Bytes of output left out of the middle.
    #[serde(skip_serializing_if = "is_zero")]
    pub omitted_bytes: u64,
    pub timed_out: bool,
    pub duration_ms: u64,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

/// Run `command` in `cwd` with the user's shell, for at most `timeout`.
pub async fn run(cwd: &str, command: &str, timeout: Duration) -> Result<RunOutcome, String> {
    run_with_shell(&crate::terminals::selected_shell(), cwd, command, timeout).await
}

pub(crate) async fn run_with_shell(
    shell: &str,
    cwd: &str,
    command: &str,
    timeout: Duration,
) -> Result<RunOutcome, String> {
    if command.trim().is_empty() {
        return Err("The command is empty".into());
    }
    if !std::fs::metadata(cwd).is_ok_and(|m| m.is_dir()) {
        return Err(format!("No folder {cwd} on this device"));
    }
    let _slot = RUNNING.try_acquire().map_err(|_| {
        format!("{MAX_RUNNING} commands are already running on this device: wait for one to finish")
    })?;
    let timeout = timeout.min(MAX_TIMEOUT);
    let started = Instant::now();

    let mut cmd = tokio::process::Command::new(shell);
    if crate::terminals::is_powershell(shell) {
        cmd.args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", command]);
    } else if cfg!(windows) {
        cmd.args(["/C", command]);
    } else {
        cmd.args(["-l", "-c", command]);
    }
    cmd.current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Nothing waits on a pager or paints for a terminal that isn't there.
        .env("TERM", "dumb")
        .env("PAGER", "cat")
        .env("GIT_PAGER", "cat")
        .env("GIT_TERMINAL_PROMPT", "0")
        .kill_on_drop(true);
    // Its own process group, so a timeout ends everything it started.
    #[cfg(unix)]
    cmd.process_group(0);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Couldn't start {shell}: {e}"))?;
    let group = Group::new(child.id());

    let (sender, mut chunks) = tokio::sync::mpsc::channel::<Vec<u8>>(16);
    let readers = [
        child.stdout.take().map(|out| tokio::spawn(pump(out, sender.clone()))),
        child.stderr.take().map(|err| tokio::spawn(pump(err, sender.clone()))),
    ];
    drop(sender);

    let mut output = Capped::default();
    let deadline = tokio::time::Instant::now() + timeout;
    let mut status = None;
    let mut timed_out = false;
    // Until the shell exits (or time is up), taking output as it comes.
    while status.is_none() {
        tokio::select! {
            exited = child.wait() => status = Some(exited.map_err(|e| e.to_string())?),
            Some(chunk) = chunks.recv() => output.push(&chunk),
            () = tokio::time::sleep_until(deadline) => {
                timed_out = true;
                group.kill();
                let _ = child.kill().await;
                status = child.wait().await.ok();
                break;
            }
        }
    }
    // What's still in the pipes, briefly: a process it left in the
    // background may keep them open for good.
    let drain = tokio::time::Instant::now() + DRAIN;
    loop {
        tokio::select! {
            chunk = chunks.recv() => match chunk {
                Some(chunk) => output.push(&chunk),
                None => break,
            },
            () = tokio::time::sleep_until(drain) => break,
        }
    }
    for reader in readers.into_iter().flatten() {
        reader.abort();
    }
    group.finished();

    let exit_code = if timed_out { None } else { status.and_then(|s| s.code()) };
    let (output, omitted_bytes) = output.finish();
    Ok(RunOutcome {
        exit_code,
        output,
        omitted_bytes,
        timed_out,
        duration_ms: started.elapsed().as_millis() as u64,
    })
}

/// Read `from` into `to` until it ends or nobody listens.
async fn pump(mut from: impl AsyncRead + Unpin, to: tokio::sync::mpsc::Sender<Vec<u8>>) {
    let mut buf = vec![0u8; 8 * 1024];
    loop {
        match from.read(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(n) => {
                if to.send(buf[..n].to_vec()).await.is_err() {
                    return;
                }
            }
        }
    }
}

/// The command's process group. Dropped before [`Group::finished`] (the
/// call was abandoned) or on a timeout, everything in it is killed.
struct Group {
    #[cfg_attr(not(unix), allow(dead_code))]
    pid: Option<u32>,
    done: std::sync::atomic::AtomicBool,
}

impl Group {
    fn new(pid: Option<u32>) -> Self {
        Self { pid, done: false.into() }
    }

    fn kill(&self) {
        #[cfg(unix)]
        if let Some(pid) = self.pid.and_then(|p| i32::try_from(p).ok()).filter(|p| *p > 0) {
            // SAFETY: signals the group this run made (process_group(0)).
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
    }

    fn finished(&self) {
        self.done.store(true, std::sync::atomic::Ordering::Release);
    }
}

impl Drop for Group {
    fn drop(&mut self) {
        if !self.done.load(std::sync::atomic::Ordering::Acquire) {
            self.kill();
        }
    }
}

/// Output kept within [`HEAD`] + [`TAIL`] bytes: the first bytes, the last
/// bytes, and a count of the rest.
#[derive(Default)]
struct Capped {
    head: Vec<u8>,
    tail: std::collections::VecDeque<u8>,
    omitted: u64,
}

impl Capped {
    fn push(&mut self, mut bytes: &[u8]) {
        if self.head.len() < HEAD {
            let take = bytes.len().min(HEAD - self.head.len());
            self.head.extend_from_slice(&bytes[..take]);
            bytes = &bytes[take..];
        }
        if bytes.len() >= TAIL {
            self.omitted += (self.tail.len() + bytes.len() - TAIL) as u64;
            self.tail.clear();
            bytes = &bytes[bytes.len() - TAIL..];
        }
        let overflow = (self.tail.len() + bytes.len()).saturating_sub(TAIL);
        self.tail.drain(..overflow);
        self.omitted += overflow as u64;
        self.tail.extend(bytes);
    }

    fn finish(self) -> (String, u64) {
        let head = String::from_utf8_lossy(&self.head).into_owned();
        let tail: Vec<u8> = self.tail.into();
        let tail = String::from_utf8_lossy(&tail).into_owned();
        if self.omitted == 0 {
            return (head + &tail, 0);
        }
        (
            format!("{head}\n… {} bytes of output left out …\n{tail}", self.omitted),
            self.omitted,
        )
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    async fn sh(command: &str, timeout: Duration) -> RunOutcome {
        let dir = std::env::temp_dir();
        run_with_shell("/bin/sh", dir.to_str().unwrap(), command, timeout).await.unwrap()
    }

    #[tokio::test]
    async fn a_command_gives_its_output_and_exit_code() {
        let ran = sh("echo out; echo err >&2; exit 3", DEFAULT_TIMEOUT).await;
        assert_eq!(ran.exit_code, Some(3));
        assert!(ran.output.contains("out\n") && ran.output.contains("err\n"), "{ran:?}");
        assert!(!ran.timed_out);
        // No input to wait for: a prompt reads end of input.
        let ran = sh("read line; echo \"got [$line]\"", DEFAULT_TIMEOUT).await;
        assert_eq!(ran.output, "got []\n");
    }

    #[tokio::test]
    async fn it_runs_where_it_is_asked() {
        let dir = tempfile::tempdir().unwrap();
        let ran = run_with_shell("/bin/sh", dir.path().to_str().unwrap(), "pwd", DEFAULT_TIMEOUT).await.unwrap();
        let here = std::fs::canonicalize(dir.path()).unwrap();
        assert_eq!(std::fs::canonicalize(ran.output.trim()).unwrap(), here);
        assert!(run_with_shell("/bin/sh", "/no/such/folder", "pwd", DEFAULT_TIMEOUT).await.is_err());
        assert!(run_with_shell("/bin/sh", "/tmp", "  ", DEFAULT_TIMEOUT).await.is_err());
    }

    #[tokio::test]
    async fn a_timeout_kills_everything_the_command_started() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("survived");
        let command = format!("(sleep 1; touch {}) & sleep 30", marker.display());
        let started = Instant::now();
        let ran = sh(&command, Duration::from_millis(300)).await;
        assert!(ran.timed_out);
        assert_eq!(ran.exit_code, None);
        assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert!(!marker.exists(), "a child outlived the timeout");
    }

    #[tokio::test]
    async fn an_abandoned_run_kills_everything_it_started() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("survived");
        let command = format!("sleep 1; touch {}", marker.display());
        let run = sh(&command, DEFAULT_TIMEOUT);
        assert!(tokio::time::timeout(Duration::from_millis(200), run).await.is_err());
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert!(!marker.exists(), "the command outlived its caller");
    }

    #[tokio::test]
    async fn something_left_in_the_background_never_holds_the_reply() {
        let started = Instant::now();
        let ran = sh("sleep 30 & echo started", DEFAULT_TIMEOUT).await;
        assert_eq!(ran.exit_code, Some(0));
        assert_eq!(ran.output, "started\n");
        assert!(started.elapsed() < Duration::from_secs(3), "{:?}", started.elapsed());
    }

    #[tokio::test]
    async fn long_output_keeps_its_start_and_end_within_bounds() {
        let ran = sh("echo FIRST; head -c 1000000 /dev/zero | tr '\\0' x; echo; echo LAST", DEFAULT_TIMEOUT).await;
        assert!(ran.output.starts_with("FIRST\n"));
        assert!(ran.output.ends_with("LAST\n"));
        assert!(ran.omitted_bytes > 900_000);
        assert!(ran.output.len() < HEAD + TAIL + 100);
    }

    #[test]
    fn capped_output_counts_what_it_leaves_out() {
        let mut capped = Capped::default();
        capped.push(&vec![b'a'; HEAD]);
        capped.push(&vec![b'b'; TAIL + 10]);
        capped.push(b"end");
        let (output, omitted) = capped.finish();
        assert_eq!(omitted, 13);
        assert!(output.starts_with(&"a".repeat(HEAD)));
        assert!(output.ends_with("bend"));
    }
}
