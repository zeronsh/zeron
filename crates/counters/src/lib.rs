//! Low-overhead process-wide profiling counters. Snapshots are approximate under concurrency.
//!
//! Integrated engine semantics: DocReadEntries* counts full transcript reads
//! (including mirror rebuilds), never incremental entry reads. SnapshotExports*
//! covers all SessionDoc full exports, including failed-save compactions; SnapshotDeltaRows
//! and SnapshotDeltaBytes count committed delta rows and their update + root bytes.
//! SqliteWriteTxns counts begun write transactions and autocommit write attempts,
//! including batched outbox writes (once per batch), salvage receipts and delta
//! quarantine writes, but not
//! read transactions or SQLite maintenance. JournalAppends counts successfully
//! written event records, even when batched; JournalBytes includes separator
//! newlines. JournalSyncs stays zero: the journal writer does not fsync.
//! StreamCommits counts coalesced publication windows, including leading-edge
//! windows, once for the parent and dirty subagents together. ClaudeJsonParses
//! counts JSON text parse attempts, including discriminator/number validation,
//! direct frame parsing and repair/fallback; from_value is not another text parse.
//! GitSpawns shares U16's diff-sync/Repos spawn-attempt counter. PreviewProcScans
//! counts full process-walk attempts, including walks ended by a budget; cached
//! owner validation is excluded. FsWatch* counts installed path registrations
//! (recursive roots may cover multiple kernel watches); temporary bridge watches
//! are excluded. SalvageChats counts inspection attempts, including receipt skips.
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

macro_rules! counters {
    ($($name:ident),* $(,)?) => {
        #[repr(usize)]
        #[derive(Clone, Copy, Debug)]
        pub enum C { $($name,)* Count }
        static COUNTERS: [AtomicU64; C::Count as usize] = [const { AtomicU64::new(0) }; C::Count as usize];
        const NAMES: [&str; C::Count as usize] = [$(stringify!($name),)*];
    };
}
counters! {
   DocReadEntriesCalls,
   DocReadEntriesEntries,
   PublishMessagesCalls,
   PublishMessagesNanos,
   PublishMessagesWakes,
   StreamCommits,
   StreamEventsFolded,
   SnapshotExports,
   SnapshotExportBytes,
   SnapshotExportNanos,
   SqliteWriteTxns,
   JournalAppends,
   JournalBytes,
   JournalSyncs,
   ClaudeLines,
   ClaudeJsonParses,
   DiffRepairTicks,
   GitSpawns,
   PreviewProcScans,
   PreviewsJsonWrites,
   PreviewsJsonBytes,
   FsWatchAdds,
   FsWatchLive,
   SalvageChats,
   SalvageNanos,
   RpcItemsOut,
   RpcBytesOut,
   SnapshotDeltaRows,
   SnapshotDeltaBytes,
}
#[inline]
pub fn add(c: C, n: u64) {
    COUNTERS[c as usize].fetch_add(n, Ordering::Relaxed);
}
/// Read one approximate counter without allocating a full snapshot.
#[inline]
pub fn get(c: C) -> u64 {
    COUNTERS[c as usize].load(Ordering::Relaxed)
}
#[inline]
pub fn sub(c: C, n: u64) {
    COUNTERS[c as usize].fetch_sub(n, Ordering::Relaxed);
}
pub struct Timer {
    counter: C,
    start: Instant,
}
#[inline]
pub fn timer(c: C) -> Timer {
    Timer {
        counter: c,
        start: Instant::now(),
    }
}
impl Drop for Timer {
    fn drop(&mut self) {
        add(
            self.counter,
            self.start.elapsed().as_nanos().min(u64::MAX as u128) as u64,
        );
    }
}
pub fn snapshot() -> Vec<(&'static str, u64)> {
    NAMES
        .iter()
        .zip(COUNTERS.iter())
        .map(|(&n, c)| (n, c.load(Ordering::Relaxed)))
        .collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn counters_and_timer() {
        let before = snapshot();
        add(C::DocReadEntriesCalls, 7);
        sub(C::DocReadEntriesCalls, 2);
        {
            let _t = timer(C::PublishMessagesNanos);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let after = snapshot();
        assert_eq!(
            after[C::DocReadEntriesCalls as usize].1 - before[C::DocReadEntriesCalls as usize].1,
            5
        );
        assert!(
            after[C::PublishMessagesNanos as usize].1 > before[C::PublishMessagesNanos as usize].1
        );
        assert_eq!(after[C::ClaudeLines as usize].0, "ClaudeLines");
    }
}
