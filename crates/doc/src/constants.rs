//! Constants carried over from zeron `packages/session-doc/src/constants.ts`.
//! Per the original design these are starting points — re-measure with real heavy sessions.

/// Max bytes for a single message entry before continuation splitting.
pub const MSG_INLINE_MAX: usize = 256 * 1024;
/// History retention window for shallow-snapshot trimming (days). Dropped
/// 30 → 3 with the edge copy (2026-08-04): a month of op history per doc
/// blew the shared loro-wasm heap on the edge isolate before any trim ran.
pub const RETAIN_DAYS: u32 = 3;
/// Session DO folds its update log into the snapshot at this size (lossless).
pub const COMPACT_LOG_BYTES: usize = 8 * 1024 * 1024;
/// Soft ceiling for total doc size before aggressive trim.
pub const SOFT_CEILING_BYTES: usize = 25 * 1024 * 1024;
/// Host commits streamed assistant segments into the doc at this cadence (ms).
pub const STREAM_COMMIT_MS: u64 = 120;
/// Session DO batches update-log flushes at this cadence (ms).
pub const DO_FLUSH_MS: u64 = 5_000;
/// Byte budget for the in-memory doc LRU on device backends.
pub const DOC_LRU_BYTE_BUDGET: usize = 80 * 1024 * 1024;
/// Number of trailing messages materialized into the tail sidecar.
pub const TAIL_MESSAGE_COUNT: usize = 64;
/// Minimum spacing between terminal output batches (ms). Output after a quiet
/// spell (an echoed keystroke) is sent at once; only a continuous stream is
/// paced, so a flood can't outrun the viewer's frames.
pub const TERMINAL_OUTPUT_BATCH_MS: u64 = 4;
/// Default TTL for durable commands.
pub const COMMAND_DEFAULT_TTL_MS: i64 = 24 * 60 * 60 * 1000;
/// Current session doc schema version (`meta.schemaVersion`).
pub const SESSION_SCHEMA_VERSION: u32 = 1;
