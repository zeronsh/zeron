//! Low-overhead process-wide profiling counters. Snapshots are approximate under concurrency.
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
}
#[inline]
pub fn add(c: C, n: u64) {
    COUNTERS[c as usize].fetch_add(n, Ordering::Relaxed);
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
