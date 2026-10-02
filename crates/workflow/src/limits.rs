//! Every cap in one place: the script language's, the world reads', and the
//! publication caps the host enforces. Numbers are documented in
//! `docs/workflows.md`; the guide quotes the ones an author can hit.

use std::time::Duration;

/// Longest script the engine accepts.
pub const MAX_SCRIPT_BYTES: usize = 256 * 1024;
/// Longest `instructions` string of one ask.
pub const MAX_INSTRUCTIONS_BYTES: usize = 64 * 1024;
/// `files.glob` / `git.*` list results: more than this is an error, never a
/// silent truncation (narrow the pattern).
pub const MAX_READ_ENTRIES: usize = 2000;
/// Bytes returned by `files.read`, `git.diff` and friends; larger is an error.
pub const MAX_READ_BYTES: usize = 256 * 1024;
/// Output kept per stream of `run()`; the rest is dropped and `truncated` set.
pub const MAX_RUN_OUTPUT_BYTES: usize = 128 * 1024;
pub const DEFAULT_RUN_TIMEOUT_S: u64 = 300;
pub const MAX_RUN_TIMEOUT_S: u64 = 3600;
/// `report()` items per run and bytes per item (JSON-encoded).
pub const MAX_REPORTS: usize = 256;
pub const MAX_REPORT_BYTES: usize = 16 * 1024;
/// `log()` lines per run and bytes per line.
pub const MAX_LOG_LINES: usize = 1000;
pub const MAX_LOG_LINE_BYTES: usize = 1024;
/// Artifacts: distinct ids per run, kept versions per id, content bytes.
pub const MAX_ARTIFACT_IDS: usize = 32;
pub const MAX_ARTIFACT_VERSIONS: usize = 16;
pub const MAX_ARTIFACT_BYTES: usize = 512 * 1024;
pub const MAX_TABLE_ROWS: usize = 5000;
pub const MAX_TABLE_COLUMNS: usize = 32;
pub const MAX_METRICS_ITEMS: usize = 200;
/// Items a single `pmap` / `parallel` call may fan out over.
pub const MAX_PMAP_ITEMS: usize = 5000;
/// Worker threads a `pmap` uses (items beyond this queue behind them).
pub const PMAP_WORKERS: usize = 64;

/// What bounds one script evaluation. `compute` counts only time spent
/// running the interpreter, not time blocked on `.result()` / `run()`.
#[derive(Debug, Clone)]
pub struct Limits {
    /// Interpreter ticks (calls and loop back-edges) per thread.
    pub ticks: u64,
    /// Interpreter CPU-ish wall time, excluding blocked waits.
    pub compute: Duration,
    /// Starlark heap bytes per thread.
    pub heap_bytes: usize,
    pub callstack: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            ticks: 50_000_000,
            compute: Duration::from_secs(60),
            heap_bytes: 128 << 20,
            callstack: 64,
        }
    }
}
