//! Owned agent processes. Non-Windows builds retain Tokio's command and child.
//! Windows uses native creation-time Job Object membership, never an implicit shell.
#[cfg(not(windows))]
pub use std::process::Stdio;
#[cfg(not(windows))]
pub use tokio::process::{Child, ChildStdin, ChildStdout, Command};
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) mod cmd_shim;

/// The file an npm `.cmd` shim runs (its node script or native `.exe`),
/// when the shim is one npm's cmd-shim wrote: on Windows the shim is all a
/// PATH lookup finds, and only its target says which package (and so which
/// owner) it belongs to.
pub fn npm_shim_target(shim: &std::path::Path) -> Option<std::path::PathBuf> {
    use std::io::Read as _;
    let is_shim = shim
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("cmd") || ext.eq_ignore_ascii_case("bat"));
    if !is_shim {
        return None;
    }
    let file = std::fs::File::open(shim).ok()?;
    if file.metadata().ok()?.len() > cmd_shim::MAX_SHIM_BYTES {
        return None;
    }
    let mut contents = String::new();
    file.take(cmd_shim::MAX_SHIM_BYTES)
        .read_to_string(&mut contents)
        .ok()?;
    let relative = match cmd_shim::parse(&contents)? {
        cmd_shim::ShimTarget::Exe(relative) => relative,
        cmd_shim::ShimTarget::Node { script, .. } => script,
    };
    let mut target = shim.parent()?.to_path_buf();
    // The shim spells its target with `\`, whatever platform reads it.
    target.extend(relative.split(['\\', '/']).filter(|part| !part.is_empty()));
    Some(target)
}
#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{Child, ChildStdin, ChildStdout, Command, Stdio};

/// Backoff for a Windows process creation that failed transiently.
#[cfg(any(windows, test))]
const SPAWN_RETRY_DELAYS: [std::time::Duration; 3] = [
    std::time::Duration::from_millis(100),
    std::time::Duration::from_millis(300),
    std::time::Duration::from_millis(1000),
];

/// Sleep in a synchronous caller without stalling the async runtime it may
/// run on: a multi-thread runtime's worker hands its other tasks off first
/// (`block_in_place`). Outside a runtime, on a current-thread runtime (whose
/// only thread would block regardless) or on a blocking-pool thread, a plain
/// sleep.
#[cfg(any(windows, test))]
fn sleep_off_runtime(delay: std::time::Duration) {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
            tokio::task::block_in_place(|| std::thread::sleep(delay));
        }
        _ => std::thread::sleep(delay),
    }
}

/// Whether a failed Windows process creation is worth retrying: a sharing
/// or lock violation on the image, or access denied on one modified in the
/// last minute (the malware scanner holding a CLI that just updated).
/// `image_age` is how long ago the launched image was last written.
#[cfg(any(windows, test))]
fn spawn_retryable(error: &std::io::Error, image_age: Option<std::time::Duration>) -> bool {
    match error.raw_os_error() {
        Some(32 | 33) => true,
        Some(5) => image_age.is_some_and(|age| age < std::time::Duration::from_secs(60)),
        _ => false,
    }
}

/// Mark an agent that does not exit promptly on stdin EOF: Windows' teardown
/// then terminates it rather than waiting out a grace for an exit that will
/// not come. Unix teardown signals instead; nothing changes there.
pub(crate) fn ignores_stdin_eof(child: &mut Child) {
    #[cfg(windows)]
    child.ignore_stdin_eof();
    #[cfg(not(windows))]
    let _ = child;
}

#[cfg(unix)]
pub(crate) fn signal_target(child: &Child) -> Option<i32> {
    let pid = child.id()? as i32;
    // ACP children lead a private group; other harnesses retain pid signaling.
    // SAFETY: getpgid only inspects the owned, unreaped child.
    Some(if unsafe { libc::getpgid(pid) } == pid {
        -pid
    } else {
        pid
    })
}
#[cfg(windows)]
pub(crate) fn signal_target(child: &Child) -> Option<std::sync::Arc<crate::windows_process::Job>> {
    Some(child.job.clone())
}

/// One process a descendant snapshot found: its pid and, where the process
/// table reports it, its start time, which tells it apart from an unrelated
/// process that later inherits the pid. Each snapshot carries its own
/// identities to [`terminate_tree`]: runs never share or overwrite them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(unix), allow(dead_code))]
pub(crate) struct Tracked {
    pub(crate) pid: i32,
    start: Option<u64>,
}

#[cfg(unix)]
impl Tracked {
    /// `pid` as the process table describes it right now.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn now(pid: i32) -> Self {
        Self {
            pid,
            start: proc_table::start_time(pid),
        }
    }
}

/// Every live descendant of `root` — children, grandchildren, … — from one
/// process-table snapshot. Agent CLIs start each shell in its OWN process
/// group (Claude's background Bash, Codex's exec sessions), so neither a
/// signal to the agent nor to its group reaches them, and a background
/// shell outlives its agent (verified live, Claude 2.1.286: its own task
/// stop and SIGTERM both leave the shell running). Snapshot while the agent
/// is alive: once it exits, its orphans are reparented and untraceable.
///
/// Each descendant carries its start time, so [`terminate_tree`] can tell
/// the process it found from an unrelated one that inherited the pid after
/// it exited.
#[cfg(unix)]
pub(crate) async fn descendants(root: u32) -> Vec<Tracked> {
    let mut table = tokio::task::spawn_blocking(proc_table::read)
        .await
        .unwrap_or_default();
    if table.is_empty() {
        // No /proc or libproc view (or a sandbox hid it): ask ps.
        table = ps_table().await;
    }
    tree_of(root as i32, &table)
        .into_iter()
        .map(|pid| Tracked {
            pid,
            start: table
                .iter()
                .find(|entry| entry.pid == pid)
                .and_then(|entry| entry.start),
        })
        .collect()
}

/// One process-table row; `start` identifies the process across pid reuse.
#[cfg(unix)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ProcEntry {
    pid: i32,
    ppid: i32,
    /// Its process group (0 where the table doesn't say).
    pgid: i32,
    start: Option<u64>,
}

#[cfg(unix)]
fn tree_of(root: i32, table: &[ProcEntry]) -> Vec<i32> {
    let mut found = Vec::new();
    let mut frontier = vec![root];
    while let Some(parent) = frontier.pop() {
        for entry in table {
            if entry.ppid == parent && entry.pid > 1 && !found.contains(&entry.pid) {
                found.push(entry.pid);
                frontier.push(entry.pid);
            }
        }
    }
    found
}

#[cfg(unix)]
async fn ps_table() -> Vec<ProcEntry> {
    let Ok(out) = tokio::process::Command::new("ps")
        .args(["-A", "-o", "pid=,ppid=,pgid="])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .await
    else {
        tracing::warn!("no process table (/proc or ps): agent descendants cannot be traced");
        return Vec::new();
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let mut cols = line.split_whitespace().map(str::parse::<i32>);
            let (pid, ppid) = (cols.next()?.ok()?, cols.next()?.ok()?);
            let pgid = cols.next().and_then(Result::ok).unwrap_or(0);
            Some(ProcEntry {
                pid,
                ppid,
                pgid,
                start: proc_table::start_time(pid),
            })
        })
        .collect()
}

/// Native process-table readers: Linux's /proc (slim images often ship no
/// `ps`) and macOS's libproc. Both report each process's start time.
#[cfg(unix)]
mod proc_table {
    use super::ProcEntry;

    #[cfg(target_os = "linux")]
    pub(super) fn read() -> Vec<ProcEntry> {
        let Ok(dir) = std::fs::read_dir("/proc") else {
            return Vec::new();
        };
        dir.flatten()
            .filter_map(|entry| entry.file_name().to_str()?.parse::<i32>().ok())
            .filter_map(|pid| {
                let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
                let (ppid, pgid, start) = parse_stat(&stat)?;
                Some(ProcEntry {
                    pid,
                    ppid,
                    pgid,
                    start: Some(start),
                })
            })
            .collect()
    }

    #[cfg(target_os = "linux")]
    pub(super) fn start_time(pid: i32) -> Option<u64> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        parse_stat(&stat).map(|(_, _, start)| start)
    }

    #[cfg(target_os = "macos")]
    pub(super) fn read() -> Vec<ProcEntry> {
        // SAFETY: a null buffer asks only for the current pid count.
        let count = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
        if count <= 0 {
            return Vec::new();
        }
        // Headroom for processes started between the two calls.
        let mut pids = vec![0 as libc::pid_t; count as usize + 64];
        let bytes = (pids.len() * std::mem::size_of::<libc::pid_t>()) as libc::c_int;
        // SAFETY: `bytes` is the buffer's true size.
        let filled = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), bytes) };
        if filled <= 0 {
            return Vec::new();
        }
        pids.truncate((filled as usize).min(pids.len()));
        pids.into_iter()
            .filter_map(|pid| {
                let info = bsd_info(pid)?;
                Some(ProcEntry {
                    pid,
                    ppid: info.pbi_ppid as i32,
                    pgid: info.pbi_pgid as i32,
                    start: Some(start_of(&info)),
                })
            })
            .collect()
    }

    #[cfg(target_os = "macos")]
    pub(super) fn start_time(pid: i32) -> Option<u64> {
        bsd_info(pid).map(|info| start_of(&info))
    }

    #[cfg(target_os = "macos")]
    fn bsd_info(pid: i32) -> Option<libc::proc_bsdinfo> {
        // SAFETY: proc_bsdinfo is plain data; proc_pidinfo writes at most
        // `size` bytes into it and reports how many it wrote.
        unsafe {
            let mut info: libc::proc_bsdinfo = std::mem::zeroed();
            let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
            let written = libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTBSDINFO,
                0,
                (&mut info as *mut libc::proc_bsdinfo).cast(),
                size,
            );
            (written == size).then_some(info)
        }
    }

    #[cfg(target_os = "macos")]
    fn start_of(info: &libc::proc_bsdinfo) -> u64 {
        info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec
    }

    #[cfg(target_os = "linux")]
    pub(super) fn zombie(pid: i32) -> bool {
        std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .ok()
            .and_then(|stat| {
                let rest = &stat[stat.rfind(')')? + 1..];
                rest.split_whitespace().next().map(|state| state == "Z")
            })
            .unwrap_or(false)
    }

    #[cfg(target_os = "linux")]
    pub(super) fn argv(pid: i32) -> Option<Vec<String>> {
        let raw = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
        Some(
            raw.split(|byte| *byte == 0)
                .filter(|arg| !arg.is_empty())
                .map(|arg| String::from_utf8_lossy(arg).into_owned())
                .collect(),
        )
    }

    /// The process's name as the kernel keeps it (its executable's file
    /// name, at most 15 bytes): never its arguments.
    #[cfg(target_os = "linux")]
    pub(super) fn name(pid: i32) -> Option<String> {
        let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
        let comm = comm.trim_end_matches('\n');
        (!comm.is_empty()).then(|| comm.to_owned())
    }

    /// The process's name as the kernel keeps it (its executable's file
    /// name, at most 32 bytes): never its arguments.
    #[cfg(target_os = "macos")]
    pub(super) fn name(pid: i32) -> Option<String> {
        let info = bsd_info(pid)?;
        let field = |raw: &[libc::c_char]| {
            let bytes: Vec<u8> = raw
                .iter()
                .take_while(|byte| **byte != 0)
                .map(|byte| *byte as u8)
                .collect();
            (!bytes.is_empty()).then(|| String::from_utf8_lossy(&bytes).into_owned())
        };
        field(&info.pbi_name).or_else(|| field(&info.pbi_comm))
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    pub(super) fn name(_pid: i32) -> Option<String> {
        None
    }

    #[cfg(target_os = "macos")]
    pub(super) fn zombie(pid: i32) -> bool {
        // SZOMB (sys/proc.h).
        bsd_info(pid).is_some_and(|info| info.pbi_status == 5)
    }

    /// `KERN_PROCARGS2`: argc, the exec path, padding, then argv.
    #[cfg(target_os = "macos")]
    pub(super) fn argv(pid: i32) -> Option<Vec<String>> {
        let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
        let mut size: libc::size_t = 0;
        // SAFETY: a null buffer asks for the size only.
        let asked = unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                3,
                std::ptr::null_mut(),
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        };
        if asked != 0 || size < 4 {
            return None;
        }
        let mut buf = vec![0u8; size];
        // SAFETY: `buf` is `size` bytes; sysctl writes at most that and says how many.
        let read = unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                3,
                buf.as_mut_ptr().cast(),
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        };
        if read != 0 || size < 4 {
            return None;
        }
        buf.truncate(size);
        let argc = i32::from_ne_bytes(buf[..4].try_into().ok()?).max(0) as usize;
        let mut rest = &buf[4..];
        // The exec path, then NUL padding up to argv.
        let path_end = rest.iter().position(|byte| *byte == 0)?;
        rest = &rest[path_end..];
        let start = rest.iter().position(|byte| *byte != 0)?;
        rest = &rest[start..];
        Some(
            rest.split(|byte| *byte == 0)
                .take(argc)
                .map(|arg| String::from_utf8_lossy(arg).into_owned())
                .collect(),
        )
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    pub(super) fn zombie(_pid: i32) -> bool {
        false
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    pub(super) fn argv(_pid: i32) -> Option<Vec<String>> {
        None
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    pub(super) fn read() -> Vec<ProcEntry> {
        Vec::new()
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    pub(super) fn start_time(_pid: i32) -> Option<u64> {
        None
    }

    /// `(ppid, pgrp, starttime)` from a `/proc/<pid>/stat` line: fields 4,
    /// 5 and 22, counted after the parenthesized command name (which may
    /// itself hold spaces and parentheses).
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub(super) fn parse_stat(stat: &str) -> Option<(i32, i32, u64)> {
        let rest = &stat[stat.rfind(')')? + 1..];
        let fields: Vec<&str> = rest.split_whitespace().collect();
        Some((
            fields.get(1)?.parse().ok()?,
            fields.get(2)?.parse().ok()?,
            fields.get(19)?.parse().ok()?,
        ))
    }
}

/// An MCP server a run was configured with: its command and leading args,
/// as the agent starts it (`McpServer::command`/`args`).
pub type KnownServer = (String, Vec<String>);

/// The live descendants of `root` (an agent CLI) that aren't MCP servers its
/// run was configured with (`known`, and whatever those start), each as
/// `"<pid> <name>"`: the process's name only, never its arguments (they
/// can hold anything, and this is logged). Independent of what the agent reports about its own
/// background work: a shell or dev server it started shows up here whether
/// or not the agent listed it. Zombies don't count. Blocking: one
/// process-table read and an argv read per child.
///
/// An agent that leads its own process group (every agent Zeron starts
/// through `owned::configure`) also holds on to what was left in that
/// group: a stop signals the whole group, so `cmd &` run by a shell that
/// has since exited (re-parented to init, but still in the group) would be
/// ended with the agent. Those count too, whoever their parent is now.
/// (Claude Code 2.1.322 runs each Bash command in a session and group of
/// its own, so what its commands leave behind is outside its group: the
/// stop never signals it, and it outlives the agent either way.)
///
/// A process table that can't be read answers with one entry saying so, so
/// a caller holding on "anything here" errs toward keeping the agent.
/// (Windows: not traced here; always empty.)
pub fn unexpected_descendants(root: u32, known: &[KnownServer]) -> Vec<String> {
    #[cfg(unix)]
    {
        let table = proc_table::read();
        if table.is_empty() {
            return vec!["(the process table can't be read)".to_owned()];
        }
        let root = root as i32;
        // Its group only if it leads one: an agent sharing its parent's
        // group would count every process of that parent's.
        let group = table
            .iter()
            .any(|entry| entry.pid == root && entry.pgid == root)
            .then_some(root);
        let tree = group.map(|_| tree_of(root, &table)).unwrap_or_default();
        let mut found = Vec::new();
        for entry in table.iter().filter(|entry| {
            entry.pid > 1
                && entry.pid != root
                && (entry.ppid == root
                    // Left in the group, and no longer under the agent
                    // (what is under it counts through its child).
                    || (group == Some(entry.pgid) && !tree.contains(&entry.pid)))
        }) {
            if proc_table::zombie(entry.pid) {
                continue;
            }
            let argv = proc_table::argv(entry.pid).unwrap_or_default();
            if known
                .iter()
                .any(|(command, args)| started_as(&argv, command, args))
            {
                continue;
            }
            let name = proc_table::name(entry.pid)
                .or_else(|| {
                    let program = std::path::Path::new(argv.first()?).file_name()?;
                    Some(program.to_string_lossy().into_owned())
                })
                .unwrap_or_else(|| "?".to_owned());
            found.push(format!("{} {name}", entry.pid));
        }
        found
    }
    #[cfg(not(unix))]
    {
        let _ = (root, known);
        Vec::new()
    }
}

/// Whether a process's `argv` is `command args…`: as given, or with an
/// interpreter in front (a `#!` script runs as `node /path/server …`), the
/// command named by path or by its file name.
#[cfg_attr(not(unix), allow(dead_code))]
fn started_as(argv: &[String], command: &str, args: &[String]) -> bool {
    let name = |path: &str| {
        std::path::Path::new(path)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    let wanted = name(command);
    (0..argv.len().min(3)).any(|i| {
        (argv[i] == command || (!wanted.is_empty() && name(&argv[i]) == wanted))
            && argv[i + 1..].starts_with(args)
    })
}

/// Start times recorded by [`descendants`] snapshots, consulted before every
/// signal [`terminate_tree`] sends. The shared drivers pass bare pids
/// between the two, so the identity travels here.
/// Whether `pid` is still the process a snapshot found: same start time when
/// one was recorded, merely alive otherwise.
#[cfg(unix)]
fn still_same(pid: i32, recorded: Option<u64>) -> bool {
    match recorded {
        Some(start) => proc_table::start_time(pid) == Some(start),
        // SAFETY: signal 0 only checks for existence.
        None => (unsafe { libc::kill(pid, 0) }) == 0,
    }
}

/// Terminate `tracked` processes and the process groups they lead: SIGTERM,
/// then SIGKILL whatever is still running after `grace`. A pid whose process
/// has exited and been reused by another is never signaled.
#[cfg(unix)]
pub(crate) async fn terminate_tree(tracked: &[Tracked], grace: std::time::Duration) {
    let recorded: Vec<(i32, Option<u64>)> = tracked
        .iter()
        .map(|process| (process.pid, process.start))
        .collect();
    // A group leader's whole group (its own children included); anything
    // else by pid. A leader verified now names its group for as long as the
    // group has members: the kernel never hands out a live group's id.
    let targets: Vec<(i32, Option<u64>)> = recorded
        .iter()
        .filter(|&&(pid, start)| still_same(pid, start))
        .map(|&(pid, start)| {
            // SAFETY: getpgid only inspects; a vanished pid returns -1.
            if unsafe { libc::getpgid(pid) } == pid {
                (-pid, start)
            } else {
                (pid, start)
            }
        })
        .collect();
    // SAFETY: signal 0 only checks for existence.
    let alive = |target: i32| unsafe { libc::kill(target, 0) } == 0;
    // The grace can outlast a process; a plain pid is re-verified before
    // each signal, as it may have been reused in between.
    let current = |&(target, start): &(i32, Option<u64>)| {
        if target < 0 {
            alive(target)
        } else {
            still_same(target, start)
        }
    };
    for target in &targets {
        if current(target) {
            crate::send_signal(&target.0, crate::Signal::Term);
        }
    }
    let deadline = tokio::time::Instant::now() + grace;
    while targets.iter().any(current) && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    for target in &targets {
        if current(target) {
            crate::send_signal(&target.0, crate::Signal::Kill);
        }
    }
}

/// A retry's wait leaves a multi-thread runtime's other tasks running, and
/// is safe wherever a synchronous spawn can be called from.
#[cfg(test)]
#[test]
fn a_spawn_retry_wait_does_not_stall_the_runtime() {
    use std::time::Duration;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let ticker = tokio::spawn(async {
            let mut ticks = 0;
            for _ in 0..5 {
                tokio::time::sleep(Duration::from_millis(20)).await;
                ticks += 1;
            }
            ticks
        });
        // On the runtime's only worker: without the hand-off the ticker
        // could not run until the wait was over.
        tokio::spawn(async { sleep_off_runtime(Duration::from_millis(400)) });
        let ticks = tokio::time::timeout(Duration::from_millis(300), ticker).await;
        assert_eq!(ticks.ok().and_then(Result::ok), Some(5));
        tokio::task::spawn_blocking(|| sleep_off_runtime(Duration::from_millis(1)))
            .await
            .unwrap();
    });
    tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(async { sleep_off_runtime(Duration::from_millis(1)) });
    sleep_off_runtime(Duration::from_millis(1));
}

#[cfg(test)]
#[test]
fn windows_spawn_retries_only_sharing_failures_and_fresh_images() {
    use std::io::Error;
    use std::time::Duration;
    let fresh = Some(Duration::from_secs(5));
    let old = Some(Duration::from_secs(3600));
    assert!(spawn_retryable(&Error::from_raw_os_error(32), old));
    assert!(spawn_retryable(&Error::from_raw_os_error(33), None));
    assert!(spawn_retryable(&Error::from_raw_os_error(5), fresh));
    assert!(!spawn_retryable(&Error::from_raw_os_error(5), old));
    assert!(!spawn_retryable(&Error::from_raw_os_error(5), None));
    // ERROR_FILE_NOT_FOUND, ERROR_BAD_EXE_FORMAT: retrying cannot help.
    assert!(!spawn_retryable(&Error::from_raw_os_error(2), fresh));
    assert!(!spawn_retryable(&Error::from_raw_os_error(193), fresh));
    assert_eq!(SPAWN_RETRY_DELAYS.len(), 3);
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// A grandchild in its own process group (how agent CLIs start shells)
    /// survives its parent's death; the descendant snapshot still finds it
    /// and `terminate_tree` ends it.
    #[tokio::test]
    async fn tree_teardown_reaches_shells_in_their_own_process_group() {
        /// Ends the fixture shell even when an assertion fails first.
        struct Reap(i32);
        impl Drop for Reap {
            fn drop(&mut self) {
                // SAFETY: SIGKILL to the fixture's own pid.
                unsafe { libc::kill(self.0, libc::SIGKILL) };
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let ready = dir.path().join("ready");
        // The child reports its pid only once it leads its own group: waiting
        // for it to exist was not enough (it had not called setpgid yet).
        let script = format!(
            "python3 -c 'import os, time; os.setpgid(0, 0); open(\"{}\", \"w\").write(str(os.getpid())); time.sleep(60)' & sleep 60",
            ready.display()
        );
        let mut parent = tokio::process::Command::new("sh")
            .args(["-c", &script])
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let root = parent.id().unwrap();
        let mut shell = None;
        for _ in 0..250 {
            if let Some(pid) = std::fs::read_to_string(&ready)
                .ok()
                .and_then(|s| s.trim().parse::<i32>().ok())
            {
                shell = Some(pid);
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let shell = shell.expect("the shell leads its own process group");
        let _reap = Reap(shell);
        assert_eq!(unsafe { libc::getpgid(shell) }, shell);
        let tree = descendants(root).await;
        assert!(tree.iter().any(|found| found.pid == shell), "{tree:?}");
        // The parent dies hard (an unresponsive agent's SIGKILL): the shell
        // is orphaned, not killed.
        parent.kill().await.unwrap();
        assert_eq!(unsafe { libc::kill(shell, 0) }, 0, "orphan survives");
        terminate_tree(&tree, std::time::Duration::from_secs(2)).await;
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert_ne!(unsafe { libc::kill(shell, 0) }, 0, "orphan ended");
    }

    #[test]
    fn proc_stat_fields_survive_hostile_command_names() {
        // Field 22 (starttime) and 4 (ppid), with a comm holding ") (".
        let stat = "4242 (evil) (name) S 17 4242 4242 0 -1 4194560 100 0 0 0 1 2 0 0 20 0 1 0 987654 1000 10";
        assert_eq!(proc_table::parse_stat(stat), Some((17, 4242, 987654)));
        assert_eq!(proc_table::parse_stat("4242 (truncated) S 17"), None);
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[tokio::test]
    async fn native_table_finds_children_with_start_times() {
        let mut child = tokio::process::Command::new("sleep")
            .arg("30")
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let pid = child.id().unwrap() as i32;
        let table = proc_table::read();
        let me = std::process::id() as i32;
        let entry = table
            .iter()
            .find(|entry| entry.pid == pid)
            .expect("child in the native table");
        assert_eq!(entry.ppid, me);
        assert!(entry.start.is_some());
        assert_eq!(proc_table::start_time(pid), entry.start);
        assert!(tree_of(me, &table).contains(&pid));
        child.kill().await.unwrap();
    }

    /// A pid whose recorded start time no longer matches belongs to some
    /// other process now: teardown must leave it alone.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[tokio::test]
    async fn teardown_skips_a_pid_that_was_reused() {
        let mut child = tokio::process::Command::new("sleep")
            .arg("30")
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let pid = child.id().unwrap() as i32;
        let start = proc_table::start_time(pid).unwrap();
        let impostor = Tracked {
            pid,
            start: Some(start + 1),
        };
        terminate_tree(&[impostor], std::time::Duration::from_millis(200)).await;
        assert!(
            child.try_wait().unwrap().is_none(),
            "an impostor pid was signaled"
        );
        terminate_tree(&[Tracked::now(pid)], std::time::Duration::from_millis(200)).await;
        let status = tokio::time::timeout(std::time::Duration::from_secs(5), child.wait())
            .await
            .expect("the verified process is ended")
            .unwrap();
        assert!(!status.success());
    }

    #[tokio::test]
    async fn unix_launch_retains_tokio_types_and_output() {
        let mut command: tokio::process::Command = Command::new(std::env::current_exe().unwrap());
        command
            .arg("--list")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .kill_on_drop(true);
        let child: tokio::process::Child = command.spawn().unwrap();
        let result = child.wait_with_output().await.unwrap();
        assert!(result.status.success());
        assert!(
            String::from_utf8_lossy(&result.stdout)
                .contains("unix_launch_retains_tokio_types_and_output")
        );
        let result = command.output().await.unwrap();
        assert!(result.status.success());
        assert!(!result.stdout.is_empty());
    }
}

pub(crate) mod owned;

#[cfg(all(test, unix))]
mod unexpected_descendants_tests {
    use super::*;

    /// `sh` standing in for an agent, with two children: one a configured
    /// MCP server, one not (a background shell's command).
    fn agent() -> std::process::Child {
        let child = std::process::Command::new("/bin/sh")
            .args(["-c", "sleep 3601 & sleep 3602 & wait"])
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while unexpected_descendants(child.id(), &[]).len() < 2 {
            assert!(
                std::time::Instant::now() < deadline,
                "the children never started"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        child
    }

    fn end(mut agent: std::process::Child) {
        let group = agent.id();
        for pid in proc_table::read().iter().filter(|e| e.ppid == group as i32) {
            // SAFETY: our own test's children.
            unsafe { libc::kill(pid.pid, libc::SIGKILL) };
        }
        let _ = agent.kill();
        let _ = agent.wait();
    }

    #[test]
    fn a_child_that_is_no_configured_mcp_server_is_reported() {
        let agent = agent();
        let mcp: Vec<KnownServer> = vec![("sleep".into(), vec!["3602".into()])];
        let found = unexpected_descendants(agent.id(), &mcp);
        let unconfigured = proc_table::read()
            .into_iter()
            .find(|entry| {
                entry.ppid == agent.id() as i32
                    && proc_table::argv(entry.pid)
                        .is_some_and(|argv| argv.ends_with(&["3601".into()]))
            })
            .unwrap()
            .pid;
        // Its name only: never its arguments.
        assert_eq!(found, vec![format!("{unconfigured} sleep")]);
        // Both configured: nothing unexpected.
        let both: Vec<KnownServer> = vec![
            ("/bin/sleep".into(), vec!["3601".into()]),
            ("sleep".into(), vec!["3602".into()]),
        ];
        assert_eq!(
            unexpected_descendants(agent.id(), &both),
            Vec::<String>::new()
        );
        end(agent);
    }

    #[test]
    fn a_server_run_through_an_interpreter_still_counts_as_configured() {
        let argv = |args: &[&str]| args.iter().map(|a| a.to_string()).collect::<Vec<_>>();
        let npx = argv(&["node", "/opt/homebrew/bin/npx", "-y", "server"]);
        assert!(started_as(&npx, "npx", &argv(&["-y", "server"])));
        assert!(!started_as(&npx, "npx", &argv(&["-y", "other"])));
        assert!(started_as(
            &argv(&["/x/zeron-engine", "mcp-proxy"]),
            "/x/zeron-engine",
            &argv(&["mcp-proxy"])
        ));
        assert!(!started_as(
            &argv(&["/bin/zsh", "-c", "npm run dev"]),
            "npx",
            &[]
        ));
    }

    /// An agent leading its own group (as Zeron starts them) whose shell ran
    /// `sleep 3608 &` and exited: the orphan is re-parented, but still in
    /// the group a stop signals, so it counts. Configured as an MCP
    /// server, it doesn't.
    #[test]
    fn an_orphan_left_in_the_agents_group_is_reported() {
        use std::os::unix::process::CommandExt as _;
        let mut agent = std::process::Command::new("/bin/sh")
            .args(["-c", "/bin/sh -c 'sleep 3608 >/dev/null 2>&1 &'; read _"])
            .stdin(std::process::Stdio::piped())
            .process_group(0)
            .spawn()
            .unwrap();
        let group = agent.id() as i32;
        /// Ends the agent's whole group, the orphan with it.
        struct Reap(i32);
        impl Drop for Reap {
            fn drop(&mut self) {
                // SAFETY: SIGKILL to the test's own process group.
                unsafe { libc::kill(-self.0, libc::SIGKILL) };
            }
        }
        let _reap = Reap(group);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let orphan = loop {
            let table = proc_table::read();
            let orphan = table.iter().find(|entry| {
                entry.pgid == group
                    && entry.ppid != group
                    && entry.pid != group
                    && !proc_table::zombie(entry.pid)
                    && proc_table::argv(entry.pid)
                        .is_some_and(|argv| argv.last().is_some_and(|arg| arg == "3608"))
            });
            // (Its shell has exited: the agent has no child left.)
            if let Some(orphan) = orphan
                && !table
                    .iter()
                    .any(|entry| entry.ppid == group && !proc_table::zombie(entry.pid))
            {
                break orphan.pid;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the orphan never appeared"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        let found = unexpected_descendants(group as u32, &[]);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].starts_with(&format!("{orphan} ")), "{found:?}");
        let mcp: Vec<KnownServer> = vec![("sleep".into(), vec!["3608".into()])];
        assert_eq!(
            unexpected_descendants(group as u32, &mcp),
            Vec::<String>::new()
        );
        let _ = agent.kill();
        let _ = agent.wait();
    }

    /// An agent sharing its parent's group (this test runner's) counts its
    /// own children only, never the rest of that group.
    #[test]
    fn an_agent_without_its_own_group_counts_only_its_children() {
        let agent = agent();
        assert_eq!(unexpected_descendants(agent.id(), &[]).len(), 2);
        end(agent);
    }
}
