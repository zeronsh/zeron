//! What the `zeron` app/CLI and the `zeron-engine` background engine share:
//! environment-resolved configuration, logging, the allocator, the MCP
//! bridge agents run, and the background engine's entry point.

pub mod background;
pub mod paths;

/// Production edge (Cloudflare Worker + Durable Objects on the zeron.sh zone).
/// `ZERON_EDGE_URL` overrides (local dev / self-hosting).
pub const DEFAULT_EDGE_URL: &str = "https://edge.zeron.sh";

/// Production WorkOS AuthKit client id — public knowledge (it appears in every
/// authorize URL), so baking it in is safe. Overridden by `ZERON_WORKOS_CLIENT_ID`;
/// set it to the empty string — or set a dev bearer via `ZERON_EDGE_TOKEN` — to
/// force dev-mode auth instead.
pub const DEFAULT_WORKOS_CLIENT_ID: &str = "client_01KWD0EAKZKD50YCQJNYSRE4BY";

pub fn edge_url_from_env() -> String {
    std::env::var("ZERON_EDGE_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_EDGE_URL.into())
}

/// WorkOS client id resolution: explicit env wins (empty string = dev mode);
/// otherwise a `ZERON_EDGE_TOKEN` dev bearer keeps dev mode (smoke tests,
/// local wrangler); otherwise the baked production client id makes optional
/// sync available while a bare start remains local-only.
pub fn workos_client_id_from_env(edge_token: &Option<String>) -> Option<String> {
    match std::env::var("ZERON_WORKOS_CLIENT_ID") {
        Ok(v) if v.trim().is_empty() => None,
        Ok(v) => Some(v),
        Err(_) if edge_token.is_some() => None,
        Err(_) => Some(DEFAULT_WORKOS_CLIENT_ID.into()),
    }
}

/// mimalloc, macOS only: libmalloc never returns the streaming churn's
/// high-water pages, so transient allocation became permanent RSS
/// (docs/memory-plan.md §1). Pinned to mimalloc v2 in the workspace manifest —
/// the crate's default v3 has the same pathology (churn retained as permanent
/// RSS, ~6x glibc's growth on identical workloads, no idle recovery). Linux
/// measured flat on glibc, so it keeps the system allocator.
#[cfg(target_os = "macos")]
#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// glibc gives threads their own arenas (up to 8 × cores) and on free returns
/// only the top of each heap, so churn freed mid-heap stays resident: a
/// headless engine held 6.6GB in 129 arenas after a day. `malloc_trim` also
/// releases the free pages inside every arena, so long-running modes call it
/// once a minute. Capping arenas instead (`M_ARENA_MAX=2`) reclaimed less and
/// cost ~6x the engine's CPU in arena-lock contention while chats streamed
/// (docs/memory-plan.md).
#[cfg(all(target_os = "linux", target_env = "gnu"))]
pub fn spawn_malloc_trimmer() {
    const PERIOD: std::time::Duration = std::time::Duration::from_secs(60);
    let spawned = std::thread::Builder::new()
        .name("malloc-trim".into())
        .spawn(|| {
            loop {
                std::thread::sleep(PERIOD);
                let started = std::time::Instant::now();
                // SAFETY: malloc_trim takes no pointers and locks each arena
                // itself, so it is safe to call from any thread at any time.
                let released = unsafe { libc::malloc_trim(0) } != 0;
                tracing::debug!(released, elapsed = ?started.elapsed(), "malloc_trim");
            }
        });
    if let Err(error) = spawned {
        tracing::warn!(%error, "malloc trimmer not started");
    }
}

/// The env-resolved engine configuration shared by `headless`, `login`,
/// `logout`, and `status` — one resolution so the CLI auth commands always
/// operate on the exact session the daemon will load.
pub fn engine_config_from_env() -> zeron_engine::EngineConfig {
    // Dev-mode bearer (no WorkOS): an explicit token enables sync.
    let edge_token = std::env::var("ZERON_EDGE_TOKEN").ok();
    zeron_engine::EngineConfig {
        data_dir: paths::data_dir(),
        edge_url: edge_url_from_env(),
        ipc_port: std::env::var("ZERON_IPC_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(27654),
        default_harness: harness_from_env(),
        // WorkOS mode: the signed-in session's org wins; ZERON_ORG_ID (dev
        // default "dev-org") scopes the workspace room otherwise.
        org_id: std::env::var("ZERON_ORG_ID").ok(),
        // Real auth against production by default; see
        // `workos_client_id_from_env` for the dev-mode escape hatches.
        workos_client_id: workos_client_id_from_env(&edge_token),
        edge_token,
    }
}

/// `ZERON_HARNESS` (kebab-case id) picks the default harness for chats without a
/// config row — `mock` powers the e2e smoke; default `claude-code`.
pub fn harness_from_env() -> zeron_engine::HarnessId {
    match std::env::var("ZERON_HARNESS").as_deref().map(str::trim) {
        Ok("mock") => zeron_engine::HarnessId::Mock,
        Ok("codex") => zeron_engine::HarnessId::Codex,
        Ok("cursor") => zeron_engine::HarnessId::Cursor,
        Ok("devin") => zeron_engine::HarnessId::Devin,
        Ok("grok") => zeron_engine::HarnessId::Grok,
        Ok("hermes") => zeron_engine::HarnessId::Hermes,
        Ok("pi") => zeron_engine::HarnessId::Pi,
        Ok("antigravity") => zeron_engine::HarnessId::Antigravity,
        Ok("muse") => zeron_engine::HarnessId::Muse,
        _ => zeron_engine::HarnessId::ClaudeCode,
    }
}

/// Logging for a process: stdout (stderr for `zeron mcp`, whose stdout is
/// the protocol), mirrored to `{data_dir}/logs/zeron-{mode}.log` for
/// long-running modes — a process launched from Finder or by the app has no
/// visible output. Long-running modes log at info, one-shot commands at warn
/// (`RUST_LOG` overrides either).
pub fn init_logging(long_running: bool, mode: &str, stderr_only: bool) {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    // loro's internal block-encode diagnostics log at info and flood
    // journald on every snapshot export — enough to fill a disk on a
    // long-running headless host. Quiet them by default (RUST_LOG still
    // overrides the whole filter).
    let default_filter = if long_running {
        "info,loro_internal=warn,loro=warn"
    } else {
        "warn"
    };
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| default_filter.into());
    if stderr_only {
        tracing_subscriber::registry()
            .with(filter)
            .with(
                tracing_subscriber::fmt::layer()
                    .with_ansi(false)
                    .with_writer(std::io::stderr),
            )
            .init();
        return;
    }
    let log_file = long_running
        .then(|| open_log_file(mode))
        .flatten()
        .map(|(file, path)| RotatingLog::new(file, path, LOG_LIMIT));
    let registry = tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer());
    match log_file {
        Some(file) => registry
            .with(
                tracing_subscriber::fmt::layer()
                    .with_ansi(false)
                    .with_writer(std::sync::Arc::new(file)),
            )
            .init(),
        None => registry.init(),
    }
}

/// Long-running modes: mirror panics (location and backtrace) into the log,
/// and keep glibc's arenas trimmed.
pub fn long_running_setup() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!(panic = %info,
            backtrace = %std::backtrace::Backtrace::force_capture(),
            "application panic");
        default_hook(info);
    }));
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    spawn_malloc_trimmer();
}

/// `zeron mcp`: one bridge per agent session, and it only relays — stdio
/// requests out to the engine over IPC and replies back. Two workers keep a
/// burst of parallel tool calls (large replies to encode) close to per-core
/// speed without a worker per core in every agent's bridge.
pub fn run_mcp() -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    let mut config = zeron_mcp::McpConfig::from_env();
    // The engine's IPC bearer lives in its data dir (the injected
    // ZERON_IPC_TOKEN_FILE wins when present).
    config.data_dir.get_or_insert_with(paths::data_dir);
    runtime.block_on(zeron_mcp::run(config))
}

/// `{data_dir}/logs/zeron-{mode}.log`, previous launch preserved as `.old`.
/// Headed and headless are separate files so an embedded-engine app and a
/// daemon on the same machine never interleave writes.
///
/// The returned file holds an exclusive `flock` for the process lifetime:
/// rotate-on-launch is only safe when nothing is still WRITING the current
/// file. On 2026-08-04 a dev build launched twice next to the running
/// installed app — the first rename put the daemon's live log at `.old`, the
/// second unlinked it entirely, and the daemon spent the rest of the incident
/// logging to an orphaned inode (an entire day of sync diagnostics gone at
/// the exact moment they were needed). A launch that finds the canonical file
/// locked logs to `zeron-{mode}.{pid}.log` instead; the next lock-holding
/// launch sweeps pid-suffixed files older than a week.
pub fn open_log_file(mode: &str) -> Option<(std::fs::File, std::path::PathBuf)> {
    let dir = paths::data_dir().join("logs");
    open_log_file_in(&dir, mode)
}

/// How large a log grows before it moves to `.old`: the background engine
/// and `zeron headless` run for weeks, and rotating only at launch would let
/// their logs fill the disk.
const LOG_LIMIT: u64 = 64 * 1024 * 1024;

/// A long-running process's log, kept to a bounded size: past `limit` bytes
/// the file moves to `.old` (replacing the previous one) and a fresh one
/// starts, locked as this process's like the first.
pub struct RotatingLog {
    state: std::sync::Mutex<LogState>,
    path: std::path::PathBuf,
    limit: u64,
}

struct LogState {
    file: std::fs::File,
    written: u64,
}

impl RotatingLog {
    pub fn new(file: std::fs::File, path: std::path::PathBuf, limit: u64) -> Self {
        let written = file.metadata().map(|meta| meta.len()).unwrap_or(0);
        Self {
            state: std::sync::Mutex::new(LogState { file, written }),
            path,
            limit,
        }
    }

    fn rotate(&self, state: &mut LogState) {
        use std::io::Write as _;
        let _ = state.file.flush();
        // Whatever happens, count afresh: a failed rotation is retried only
        // after another `limit` of writes, never on every line.
        state.written = 0;
        let mut old = self.path.clone().into_os_string();
        old.push(".old");
        // Windows can't rename a file it holds open: keep writing to it then.
        if std::fs::rename(&self.path, &old).is_err() {
            return;
        }
        if let Ok(file) = std::fs::File::create(&self.path) {
            #[cfg(unix)]
            {
                use std::os::unix::io::AsRawFd;
                unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
            }
            state.file = file;
        }
    }
}

impl std::io::Write for &RotatingLog {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.written >= self.limit {
            self.rotate(&mut state);
        }
        let written = state.file.write(buf)?;
        state.written += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .file
            .flush()
    }
}

/// Dir-parameterized body of [`open_log_file`] (unit-testable without env).
fn open_log_file_in(
    dir: &std::path::Path,
    mode: &str,
) -> Option<(std::fs::File, std::path::PathBuf)> {
    std::fs::create_dir_all(dir).ok()?;
    let path = dir.join(format!("zeron-{mode}.log"));
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        // Probe the CURRENT inode for a live writer before touching it.
        let preexisting = path.exists();
        let existing = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .ok()?;
        let rc = unsafe { libc::flock(existing.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if rc != 0 {
            // A live process owns the canonical log — leave it alone.
            let overflow = dir.join(format!("zeron-{mode}.{}.log", std::process::id()));
            return std::fs::File::create(&overflow)
                .ok()
                .map(|file| (file, overflow));
        }
        // No live writer: rotate, create fresh, and lock it as ours. (The
        // probe's flock dies with `existing`; a first-ever launch has nothing
        // to rotate — the probe itself created the empty file.)
        drop(existing);
        if preexisting {
            let _ = std::fs::rename(&path, dir.join(format!("zeron-{mode}.log.old")));
        }
        let file = std::fs::File::create(&path).ok()?;
        unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        sweep_stale_pid_logs(dir, mode);
        Some((file, path))
    }
    #[cfg(not(unix))]
    {
        let _ = std::fs::rename(&path, dir.join(format!("zeron-{mode}.log.old")));
        std::fs::File::create(&path).ok().map(|file| (file, path))
    }
}

#[cfg(test)]
mod rotating_log_tests {
    use super::RotatingLog;
    use std::io::Write as _;

    #[test]
    fn a_log_past_its_limit_moves_to_old_and_starts_over() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("zeron-engine.log");
        let file = std::fs::File::create(&path).unwrap();
        let log = RotatingLog::new(file, path.clone(), 100);
        let mut writer = &log;
        writer.write_all(&[b'a'; 120]).unwrap();
        // Over the limit: the next write rotates first.
        writer.write_all(b"fresh\n").unwrap();
        writer.flush().unwrap();
        let old = dir.path().join("zeron-engine.log.old");
        assert_eq!(std::fs::metadata(&old).unwrap().len(), 120);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "fresh\n");
    }
}

#[cfg(all(test, unix))]
mod log_file_tests {
    use super::open_log_file_in;

    #[test]
    fn second_launch_never_rotates_a_live_processes_log() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path();
        // First launch owns the canonical file and keeps writing.
        let first = open_log_file_in(dir, "headed").expect("first log");
        assert!(dir.join("zeron-headed.log").is_file());
        // Second launch while the first is alive: canonical file untouched,
        // pid-suffixed overflow file instead (the 2026-08-04 clobber).
        let second = open_log_file_in(dir, "headed").expect("second log");
        let pid_path = dir.join(format!("zeron-headed.{}.log", std::process::id()));
        assert!(pid_path.is_file(), "expected pid-suffixed overflow log");
        assert!(
            !dir.join("zeron-headed.log.old").exists(),
            "live canonical log must not be rotated away"
        );
        drop(second);
        // After the owner exits, a fresh launch rotates normally.
        drop(first);
        let third = open_log_file_in(dir, "headed").expect("third log");
        assert!(
            dir.join("zeron-headed.log.old").is_file(),
            "rotation resumes"
        );
        drop(third);
    }
}

/// Delete `zeron-{mode}.{pid}.log` overflow files older than a week — they
/// only exist when a second instance raced a live one for the canonical log.
#[cfg(unix)]
fn sweep_stale_pid_logs(dir: &std::path::Path, mode: &str) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let prefix = format!("zeron-{mode}.");
    let week = std::time::Duration::from_secs(7 * 24 * 60 * 60);
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(middle) = name
            .strip_prefix(&prefix)
            .and_then(|rest| rest.strip_suffix(".log"))
        else {
            continue;
        };
        if !middle.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > week);
        if stale {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}
