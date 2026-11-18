//! The cheap model's one-shot runs for orchestration (progress notes, the
//! end-of-turn borderline check, reply verdicts, Q&A condensing), through
//! one limiter: at most [`MAX_RUNNING`] subprocesses at once engine-wide, one
//! run for the same input at a time (later askers share its reply), and a
//! short memory of replies by input, so the same question isn't put to the
//! model again a moment later. Each run is a CLI process of seconds.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use futures::FutureExt;
use tokio::sync::{OnceCell, Semaphore};
use zeron_proto::HarnessId;

use super::progress::NoteFn;

/// One-shot runs at once, engine-wide.
pub(crate) const MAX_RUNNING: usize = 2;
/// A reply is reused for the same input this long.
const KEEP_REPLY: Duration = Duration::from_secs(30 * 60);
/// No reply (the model unsure or unreachable): not asked again for this long.
const KEEP_NOTHING: Duration = Duration::from_secs(5 * 60);
/// Replies remembered at most.
const KEEP_AT_MOST: usize = 256;

type Key = u64;

fn key(harness: HarnessId, prompt: &str) -> Key {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    format!("{harness:?}").hash(&mut h);
    prompt.hash(&mut h);
    h.finish()
}

#[derive(Default)]
struct Memory {
    replies: HashMap<Key, (Instant, Option<String>)>,
    running: HashMap<Key, Arc<OnceCell<Option<String>>>>,
}

/// Counters (read by tests and the `perf` log).
#[derive(Default, Debug)]
pub(crate) struct Counts {
    /// Runs started (subprocesses).
    pub started: AtomicU64,
    /// Answered from memory.
    pub remembered: AtomicU64,
    /// Shared another caller's run of the same input.
    pub shared: AtomicU64,
    /// The most running at once.
    pub most_at_once: AtomicU64,
    running: AtomicU64,
}

pub(crate) struct OneShots {
    permits: Arc<Semaphore>,
    memory: Mutex<Memory>,
    pub(crate) counts: Counts,
}

impl Default for OneShots {
    fn default() -> Self {
        Self::new(MAX_RUNNING)
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl OneShots {
    pub(crate) fn new(max_running: usize) -> Self {
        Self {
            permits: Arc::new(Semaphore::new(max_running.max(1))),
            memory: Mutex::new(Memory::default()),
            counts: Counts::default(),
        }
    }

    /// `run` through the limiter.
    pub(crate) fn wrap(self: &Arc<Self>, run: NoteFn) -> NoteFn {
        let this = self.clone();
        Arc::new(move |harness, prompt| {
            let this = this.clone();
            let run = run.clone();
            async move { this.run(harness, prompt, run).await }.boxed()
        })
    }

    /// `run(harness, prompt)`'s reply: remembered, shared with a run of the
    /// same input under way, or run when a slot is free.
    pub(crate) async fn run(&self, harness: HarnessId, prompt: String, run: NoteFn) -> Option<String> {
        // A model run is seconds: never with the registry held.
        debug_assert_eq!(
            crate::workspace_host::registry_locks_held(),
            0,
            "a one-shot model run started under the registry lock"
        );
        let key = key(harness, &prompt);
        let cell = {
            let mut memory = lock(&self.memory);
            let now = Instant::now();
            if let Some((at, reply)) = memory.replies.get(&key) {
                let keep = if reply.is_some() { KEEP_REPLY } else { KEEP_NOTHING };
                if now.duration_since(*at) < keep {
                    self.counts.remembered.fetch_add(1, Ordering::Relaxed);
                    return reply.clone();
                }
            }
            match memory.running.get(&key) {
                Some(cell) => {
                    self.counts.shared.fetch_add(1, Ordering::Relaxed);
                    cell.clone()
                }
                None => {
                    let cell = Arc::new(OnceCell::new());
                    memory.running.insert(key, cell.clone());
                    cell
                }
            }
        };
        let reply = cell
            .get_or_init(|| async {
                let asked = Instant::now();
                let _permit = self.permits.clone().acquire_owned().await.ok();
                let running = self.counts.running.fetch_add(1, Ordering::AcqRel) + 1;
                self.counts.most_at_once.fetch_max(running, Ordering::AcqRel);
                self.counts.started.fetch_add(1, Ordering::Relaxed);
                let started = Instant::now();
                let reply = run(harness, prompt).await;
                self.counts.running.fetch_sub(1, Ordering::AcqRel);
                tracing::debug!(
                    target: "perf",
                    what = "one_shot_run",
                    wait_us = started.duration_since(asked).as_micros() as u64,
                    us = started.elapsed().as_micros() as u64,
                    running,
                    started = self.counts.started.load(Ordering::Relaxed),
                    remembered = self.counts.remembered.load(Ordering::Relaxed),
                    shared = self.counts.shared.load(Ordering::Relaxed),
                );
                reply
            })
            .await
            .clone();
        let mut memory = lock(&self.memory);
        if memory.running.get(&key).is_some_and(|c| Arc::ptr_eq(c, &cell)) {
            memory.running.remove(&key);
            remember(&mut memory.replies, key, reply.clone());
        }
        reply
    }
}

fn remember(replies: &mut HashMap<Key, (Instant, Option<String>)>, key: Key, reply: Option<String>) {
    let now = Instant::now();
    if replies.len() >= KEEP_AT_MOST {
        replies.retain(|_, (at, reply)| {
            now.duration_since(*at) < if reply.is_some() { KEEP_REPLY } else { KEEP_NOTHING }
        });
    }
    if replies.len() >= KEEP_AT_MOST
        && let Some(oldest) = replies.iter().min_by_key(|(_, (at, _))| *at).map(|(k, _)| *k)
    {
        replies.remove(&oldest);
    }
    replies.insert(key, (now, reply));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    /// A run that takes `ms`, counting how many run at once and in all.
    fn slow(ms: u64, now: Arc<AtomicUsize>, most: Arc<AtomicUsize>, calls: Arc<AtomicUsize>) -> NoteFn {
        Arc::new(move |_, prompt: String| {
            let (now, most, calls) = (now.clone(), most.clone(), calls.clone());
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                let n = now.fetch_add(1, Ordering::SeqCst) + 1;
                most.fetch_max(n, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(ms)).await;
                now.fetch_sub(1, Ordering::SeqCst);
                (!prompt.contains("unsure")).then(|| format!("re: {prompt}"))
            }
            .boxed()
        })
    }

    fn counters() -> (Arc<AtomicUsize>, Arc<AtomicUsize>, Arc<AtomicUsize>) {
        Default::default()
    }

    #[tokio::test]
    async fn no_more_than_the_cap_run_at_once() {
        let shots = Arc::new(OneShots::new(2));
        let (now, most, calls) = counters();
        let run = shots.wrap(slow(30, now, most.clone(), calls.clone()));
        let all = (0..8).map(|i| run(HarnessId::ClaudeCode, format!("note {i}")));
        let replies = futures::future::join_all(all).await;
        assert_eq!(calls.load(Ordering::SeqCst), 8, "different inputs each run");
        assert_eq!(most.load(Ordering::SeqCst), 2, "two at once at most");
        assert!(replies.iter().all(Option::is_some));
        assert_eq!(shots.counts.most_at_once.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn the_same_input_runs_once_and_is_remembered() {
        let shots = Arc::new(OneShots::new(2));
        let (now, most, calls) = counters();
        let run = shots.wrap(slow(30, now, most, calls.clone()));
        let same = (0..5).map(|_| run(HarnessId::ClaudeCode, "is this answered?".into()));
        let replies = futures::future::join_all(same).await;
        assert_eq!(calls.load(Ordering::SeqCst), 1, "one run shared by every asker");
        assert!(replies.iter().all(|r| r.as_deref() == Some("re: is this answered?")));
        assert_eq!(shots.counts.shared.load(Ordering::SeqCst), 4);
        // Asked again later: from memory.
        assert_eq!(run(HarnessId::ClaudeCode, "is this answered?".into()).await.as_deref(), Some("re: is this answered?"));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        // Another harness is another input.
        run(HarnessId::Codex, "is this answered?".into()).await;
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn an_unsure_reply_is_not_asked_again_at_once() {
        let shots = Arc::new(OneShots::new(2));
        let (now, most, calls) = counters();
        let run = shots.wrap(slow(1, now, most, calls.clone()));
        assert_eq!(run(HarnessId::ClaudeCode, "unsure verdict".into()).await, None);
        assert_eq!(run(HarnessId::ClaudeCode, "unsure verdict".into()).await, None);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn memory_is_bounded() {
        let mut replies = HashMap::new();
        for k in 0..(KEEP_AT_MOST as u64 * 2) {
            remember(&mut replies, k, Some("x".into()));
        }
        assert_eq!(replies.len(), KEEP_AT_MOST);
    }
}
