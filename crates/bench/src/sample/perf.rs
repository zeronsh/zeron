//! User-space `perf_event_open` counts for `exec-counted`.
//!
//! The wrapper forks, blocks the child on a pipe, and opens
//! `instructions`, `cycles`, and `task-clock` on that pid with
//! `inherit`, `exclude_kernel`, `exclude_hv`, and `enable_on_exec`.
//! Those events are allowed at `perf_event_paranoid=2` for one's own
//! tasks. Inherited counts include a task only after it has exited, so
//! the totals are read after `waitpid`. A failed open leaves that field
//! null and the command still runs.

use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::os::unix::io::RawFd;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, ExitStatus};
use std::sync::atomic::{AtomicI32, Ordering};

use anyhow::{Context, Result, bail};
use serde::Serialize;

/// Totals written to `perf.json`. Null fields are counters that did not open.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PerfTotals {
    pub instructions_u: Option<u64>,
    pub cycles_u: Option<u64>,
    pub task_clock_ns: Option<u64>,
}

const PERF_TYPE_HARDWARE: u32 = 0;
const PERF_TYPE_SOFTWARE: u32 = 1;
const PERF_COUNT_HW_CPU_CYCLES: u64 = 0;
const PERF_COUNT_HW_INSTRUCTIONS: u64 = 1;
const PERF_COUNT_SW_TASK_CLOCK: u64 = 1;
const PERF_ATTR_SIZE_VER0: u32 = 64;
/// `PERF_FLAG_FD_CLOEXEC`. The engine's fd count must not include these.
const PERF_FLAG_FD_CLOEXEC: u64 = 1 << 3;

/// `disabled | inherit | exclude_kernel | exclude_hv | enable_on_exec`.
///
/// `disabled` is set so `enable_on_exec` is what starts the count, at
/// `execve`, not while the child is blocked on the setup pipe.
const USER_COUNTER_FLAGS: u64 = (1 << 0) | (1 << 1) | (1 << 5) | (1 << 6) | (1 << 12);

const FORWARDED: [libc::c_int; 4] = [libc::SIGHUP, libc::SIGINT, libc::SIGQUIT, libc::SIGTERM];

static CHILD_PID: AtomicI32 = AtomicI32::new(0);

/// `perf_event_attr` prefix, 64 bytes (`PERF_ATTR_SIZE_VER0`).
#[repr(C)]
struct PerfEventAttr {
    type_: u32,
    size: u32,
    config: u64,
    sample_period: u64,
    sample_type: u64,
    read_format: u64,
    flags: u64,
    wakeup: u32,
    bp_type: u32,
    bp_addr: u64,
}

const _: () = assert!(std::mem::size_of::<PerfEventAttr>() == PERF_ATTR_SIZE_VER0 as usize);

/// Run `command` and, when a trial path is configured, write `perf.json`.
///
/// The returned status is the command's. Call this from a single-threaded
/// process: it `fork`s. The `zeron-bench` CLI does that before it starts a
/// runtime. A counter that fails to open is null in the report; the command
/// still runs.
pub fn run_counted(command: &[OsString]) -> Result<ExitStatus> {
    let Some((program, args)) = command.split_first() else {
        bail!("exec-counted needs a command");
    };

    let mut old_mask = unsafe { std::mem::zeroed() };
    block_forwarded(&mut old_mask)?;
    install_handlers();

    let mut pipes = [0; 2];
    // SAFETY: `pipes` is a two-fd buffer we own. `pipe2` only writes it.
    let rc = unsafe { libc::pipe2(pipes.as_mut_ptr(), libc::O_CLOEXEC) };
    if rc != 0 {
        let err = io::Error::last_os_error();
        restore_mask(&old_mask);
        return Err(err).context("pipe");
    }

    // SAFETY: this process has one thread. `fork` duplicates it.
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        let err = io::Error::last_os_error();
        close_fd(pipes[0]);
        close_fd(pipes[1]);
        restore_mask(&old_mask);
        return Err(err).context("fork");
    }
    if pid == 0 {
        close_fd(pipes[1]);
        child_exec(pipes[0], program, args, &old_mask);
    }

    close_fd(pipes[0]);
    // Both sides call `setpgid`. The child leads its own group so a signal
    // forwarded to that group reaches grandchildren.
    // SAFETY: `pid` is the child we just forked.
    unsafe { libc::setpgid(pid, pid) };
    CHILD_PID.store(pid, Ordering::Release);

    let counters = attach(pid, true);
    restore_mask(&old_mask);

    let go = 1u8;
    // SAFETY: `pipes[1]` is the write end we own. SIGPIPE is ignored below
    // so a child that already died does not kill the wrapper.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
        libc::write(pipes[1], &go as *const u8 as *const libc::c_void, 1);
        libc::close(pipes[1]);
    }

    let status = wait_child(pid)?;
    let totals = counters.read();
    if let Some(path) = perf_json_path()
        && let Err(err) = write_totals(&path, &totals)
    {
        eprintln!("zeron-bench: perf.json {}: {err}", path.display());
    }
    Ok(status)
}

fn child_exec(read_fd: RawFd, program: &OsString, args: &[OsString], old_mask: &libc::sigset_t) {
    // The child inherited the wrapper's handlers and blocked mask. `execve`
    // keeps an ignored signal ignored, so restore the defaults first.
    for sig in FORWARDED {
        unsafe { libc::signal(sig, libc::SIG_DFL) };
    }
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL) };
    restore_mask(old_mask);
    unsafe { libc::setpgid(0, 0) };
    let mut byte = 0u8;
    loop {
        // SAFETY: `byte` is one byte we own. `read` only writes it.
        let n = unsafe { libc::read(read_fd, &mut byte as *mut u8 as *mut libc::c_void, 1) };
        if n >= 0 {
            break;
        }
        if io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
            break;
        }
    }
    close_fd(read_fd);
    let err = Command::new(program).args(args).exec();
    eprintln!("zeron-bench: exec {}: {err}", program.to_string_lossy());
    unsafe { libc::_exit(127) };
}

fn wait_child(pid: i32) -> Result<ExitStatus> {
    loop {
        let mut status = 0;
        // SAFETY: `status` is an int we own. `waitpid` only writes it.
        let rc = unsafe { libc::waitpid(pid, &mut status, 0) };
        if rc < 0 {
            let err = io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            return Err(err).context("waitpid");
        }
        return Ok(ExitStatus::from_raw(status));
    }
}

struct Counters {
    instructions: Option<RawFd>,
    cycles: Option<RawFd>,
    task_clock: Option<RawFd>,
}

impl Drop for Counters {
    fn drop(&mut self) {
        for fd in [
            self.instructions.take(),
            self.cycles.take(),
            self.task_clock.take(),
        ]
        .into_iter()
        .flatten()
        {
            close_fd(fd);
        }
    }
}

impl Counters {
    fn read(&self) -> PerfTotals {
        PerfTotals {
            instructions_u: self.instructions.and_then(read_u64),
            cycles_u: self.cycles.and_then(read_u64),
            task_clock_ns: self.task_clock.and_then(read_u64),
        }
    }
}

fn attach(pid: i32, report: bool) -> Counters {
    Counters {
        instructions: open_named(
            pid,
            "instructions",
            PERF_TYPE_HARDWARE,
            PERF_COUNT_HW_INSTRUCTIONS,
            report,
        ),
        cycles: open_named(
            pid,
            "cycles",
            PERF_TYPE_HARDWARE,
            PERF_COUNT_HW_CPU_CYCLES,
            report,
        ),
        task_clock: open_named(
            pid,
            "task-clock",
            PERF_TYPE_SOFTWARE,
            PERF_COUNT_SW_TASK_CLOCK,
            report,
        ),
    }
}

fn open_named(pid: i32, name: &str, type_: u32, config: u64, report: bool) -> Option<RawFd> {
    match perf_event_open(pid, type_, config) {
        Ok(fd) => Some(fd),
        Err(err) => {
            if report {
                eprintln!("zeron-bench: perf {name}: {err}");
            }
            None
        }
    }
}

fn perf_event_open(pid: i32, type_: u32, config: u64) -> io::Result<RawFd> {
    let attr = PerfEventAttr {
        type_,
        size: PERF_ATTR_SIZE_VER0,
        config,
        sample_period: 0,
        sample_type: 0,
        read_format: 0,
        flags: USER_COUNTER_FLAGS,
        wakeup: 0,
        bp_type: 0,
        bp_addr: 0,
    };
    // SAFETY: `attr` is a valid VER0 attribute. `pid` is a task we own,
    // `cpu` -1 follows it across CPUs, and `group_fd` -1 leaves it ungrouped.
    let rc = unsafe {
        libc::syscall(
            libc::SYS_perf_event_open,
            &attr as *const PerfEventAttr,
            libc::c_long::from(pid),
            -1 as libc::c_long,
            -1 as libc::c_long,
            PERF_FLAG_FD_CLOEXEC as libc::c_ulong,
        )
    };
    if rc < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(rc as RawFd)
}

fn read_u64(fd: RawFd) -> Option<u64> {
    let mut buf = [0u8; 8];
    // SAFETY: `buf` is 8 bytes we own. `read` only writes it.
    let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast::<libc::c_void>(), buf.len()) };
    if n != 8 {
        return None;
    }
    Some(u64::from_ne_bytes(buf))
}

fn close_fd(fd: RawFd) {
    if fd >= 0 {
        unsafe { libc::close(fd) };
    }
}

/// Where `perf.json` goes.
///
/// `ZB_PERF_JSON` wins. Otherwise the parent of a trial `ZERON_DATA_DIR`
/// (`…/trials/<n>-<side>/data`). Paths under `/tmp` or a live `.zeron`
/// profile are skipped.
fn perf_json_path() -> Option<PathBuf> {
    if let Some(raw) = std::env::var_os("ZB_PERF_JSON")
        && !raw.is_empty()
    {
        let path = PathBuf::from(raw);
        return safe_output(&path).then_some(path);
    }
    let data = PathBuf::from(std::env::var_os("ZERON_DATA_DIR")?);
    let trial = data.parent()?;
    if trial.parent()?.file_name()? != "trials" {
        return None;
    }
    let path = trial.join("perf.json");
    safe_output(&path).then_some(path)
}

fn safe_output(path: &Path) -> bool {
    if unsafe_bench_path(path) {
        return false;
    }
    let Some(parent) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) else {
        return false;
    };
    match parent.canonicalize() {
        Ok(canon) => {
            let full = canon.join(path.file_name().unwrap_or_default());
            !unsafe_bench_path(&canon) && !unsafe_bench_path(&full)
        }
        // The directory is created at write time. The lexical check above
        // already rejected `/tmp` and `.zeron`.
        Err(_) => true,
    }
}

fn unsafe_bench_path(path: &Path) -> bool {
    let tmp = Path::new("/tmp");
    if path == tmp || path.starts_with(tmp) {
        return true;
    }
    path.components().any(|component| match component {
        Component::Normal(name) => name == ".zeron",
        _ => false,
    })
}

fn write_totals(path: &Path, totals: &PerfTotals) -> io::Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }
    let mut body = serde_json::to_vec_pretty(totals).map_err(io::Error::other)?;
    body.push(b'\n');
    let tmp = path.with_extension("json.tmp");
    {
        let mut file = fs::File::create(&tmp)?;
        file.write_all(&body)?;
        file.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    Ok(())
}

fn block_forwarded(old: &mut libc::sigset_t) -> Result<()> {
    let mut set = unsafe { std::mem::zeroed() };
    // SAFETY: `set` and `old` are sigsets we own.
    let rc = unsafe {
        libc::sigemptyset(&mut set);
        for sig in FORWARDED {
            libc::sigaddset(&mut set, sig);
        }
        libc::sigprocmask(libc::SIG_BLOCK, &set, old)
    };
    if rc != 0 {
        return Err(io::Error::last_os_error()).context("block signals");
    }
    Ok(())
}

fn restore_mask(old: &libc::sigset_t) {
    unsafe { libc::sigprocmask(libc::SIG_SETMASK, old, std::ptr::null_mut()) };
}

fn install_handlers() {
    let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
    action.sa_sigaction = forward_signal as *const () as usize;
    action.sa_flags = libc::SA_RESTART;
    unsafe { libc::sigemptyset(&mut action.sa_mask) };
    for sig in FORWARDED {
        // SAFETY: `action` is a filled sigaction. The handler only calls `kill`.
        unsafe { libc::sigaction(sig, &action, std::ptr::null_mut()) };
    }
}

extern "C" fn forward_signal(sig: libc::c_int) {
    let child = CHILD_PID.load(Ordering::Acquire);
    if child > 1 {
        unsafe {
            if libc::kill(-child, sig) != 0 {
                libc::kill(child, sig);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::{lock_process_env, set_process_var, unset_process_var};
    use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

    #[test]
    fn user_counter_flags_match_the_kernel_bitfield() {
        // disabled | inherit | exclude_kernel | exclude_hv | enable_on_exec
        assert_eq!(USER_COUNTER_FLAGS, 0x1063);
    }

    #[test]
    fn init_cannot_be_counted() {
        let counters = attach(1, false);
        let totals = counters.read();
        assert_eq!(totals.instructions_u, None);
        assert_eq!(totals.cycles_u, None);
        assert_eq!(totals.task_clock_ns, None);
    }

    #[test]
    fn perf_json_path_honors_an_explicit_file_and_a_trial() {
        let _guard = lock_process_env();
        let saved = ["ZB_PERF_JSON", "ZERON_DATA_DIR"]
            .into_iter()
            .map(|key| (key, std::env::var_os(key)))
            .collect::<Vec<_>>();
        unset_process_var("ZB_PERF_JSON");
        unset_process_var("ZERON_DATA_DIR");

        assert!(perf_json_path().is_none());

        let explicit = scratch("explicit").join("perf.json");
        set_process_var("ZB_PERF_JSON", &explicit);
        assert_eq!(perf_json_path(), Some(explicit));

        set_process_var("ZB_PERF_JSON", "/tmp/perf.json");
        assert!(perf_json_path().is_none());
        set_process_var("ZB_PERF_JSON", "/home/user/.zeron/perf.json");
        assert!(perf_json_path().is_none());

        unset_process_var("ZB_PERF_JSON");
        let trial = scratch("trial").join("runs/t/trials/0-a");
        let data = trial.join("data");
        fs::create_dir_all(&data).unwrap();
        set_process_var("ZERON_DATA_DIR", &data);
        assert_eq!(perf_json_path(), Some(trial.join("perf.json")));

        set_process_var("ZERON_DATA_DIR", "/home/user/.zeron");
        assert!(perf_json_path().is_none());
        set_process_var("ZERON_DATA_DIR", "/tmp/runs/t/trials/0-a/data");
        assert!(perf_json_path().is_none());

        for (key, value) in saved {
            match value {
                Some(value) => set_process_var(key, value),
                None => unset_process_var(key),
            }
        }
    }

    fn scratch(name: &str) -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, AtomicOrdering::Relaxed);
        let dir =
            PathBuf::from("/home/user/Documents/dev/tmp/zeron-perf/scratch/b11-perf-counted/perf")
                .join(format!("{name}-{n}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }
}
