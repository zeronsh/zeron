//! Which part of the doc a commit touched, and a commands ledger cache that
//! rides on it.
//!
//! The per-chat change worker used to treat every doc event alike: republish
//! the transcript, drain the command ledger, drain the queue. The ledger read
//! (`read_commands`, a deep value of the whole `commands` list) therefore ran
//! once or more per ~120ms streaming commit, though a streaming turn never
//! touches `commands`, and its cost grows with chat age.
//!
//! [`ChangeMask::of_event`] classifies a diff event by root container in
//! O(#diffs), without touching the doc. Consumers keep an
//! [`AtomicChangeMask`] each, so one consumer taking its bits never hides a
//! change from another. [`CommandsCache`] keeps the decoded ledger between
//! ticks and re-reads it only after the mask reported a `commands` change.
//!
//! The cache's oracle is `SessionDoc::read_commands`: under
//! `cfg(debug_assertions)`, and in release when `ZERON_PERF_VERIFY=1` (sampled
//! 1 in 32), a cache hit is compared with a fresh read. On a mismatch it logs
//! an error, returns the fresh result and drops the cache. It never panics.

use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use loro::Index;
use loro::event::DiffEvent;

use crate::commands::SessionCommandEntry;
use crate::schema::{DocError, SessionDoc};
use crate::transcript_mirror::{Verify, verify_sampled};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A set of root containers a doc event touched.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct ChangeMask(u8);

impl ChangeMask {
    pub const NONE: Self = Self(0);
    /// The `messages` transcript list, or anything under it.
    pub const MESSAGES: Self = Self(1);
    /// The `commands` ledger.
    pub const COMMANDS: Self = Self(2);
    /// The pending-message `queue`.
    pub const QUEUE: Self = Self(4);
    /// Every other root (`meta`, registry maps, ...).
    pub const META: Self = Self(8);
    /// An event that cannot be attributed: assume everything changed.
    pub const ALL: Self = Self(1 | 2 | 4 | 8);

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Every bit of `other` is set.
    pub fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// At least one bit of `other` is set.
    pub fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    pub fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Classify a doc event by the root container each diff lives under.
    /// O(#diffs); never reads the doc. An unresolvable path counts as `ALL`,
    /// the same rule the transcript mirror applies.
    pub fn of_event(event: &DiffEvent<'_>) -> Self {
        let mut mask = Self::NONE;
        for change in &event.events {
            if change.is_unknown {
                return Self::ALL;
            }
            mask = mask.union(match change.path.first() {
                Some((_, Index::Key(key))) => match key.as_str() {
                    "messages" => Self::MESSAGES,
                    "commands" => Self::COMMANDS,
                    "queue" => Self::QUEUE,
                    _ => Self::META,
                },
                _ => return Self::ALL,
            });
        }
        mask
    }
}

/// A [`ChangeMask`] shared between the doc callback (which sets bits) and one
/// consumer (which takes them). Set before the consumer is woken, take after:
/// a change is never missed, at worst a wake finds nothing left to do.
#[derive(Default)]
pub struct AtomicChangeMask(AtomicU8);

impl AtomicChangeMask {
    pub fn set(&self, mask: ChangeMask) {
        self.0.fetch_or(mask.0, Ordering::AcqRel);
    }

    /// Everything set since the last take, cleared.
    pub fn take(&self) -> ChangeMask {
        ChangeMask(self.0.swap(0, Ordering::AcqRel))
    }
}

/// The decoded `commands` ledger, shared until the next ledger change.
///
/// [`CommandsCache::invalidate`] is called from the doc callback when the mask
/// carries `COMMANDS`. Loro emits that callback synchronously inside the
/// committing call, and every writer of `commands` commits before returning,
/// so by the time a write returns the cache already knows it is stale.
pub struct CommandsCache {
    generation: AtomicU64,
    /// The generation sampled BEFORE the read that produced the entry, so a
    /// commit racing the read can only leave the entry looking older than it
    /// is, never newer.
    cached: Mutex<Option<(u64, Arc<Vec<SessionCommandEntry>>)>>,
    verify: Verify,
    fills: AtomicU64,
    hits: AtomicU64,
    mismatches: AtomicU64,
}

impl Default for CommandsCache {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandsCache {
    pub fn new() -> Self {
        Self {
            generation: AtomicU64::new(0),
            cached: Mutex::new(None),
            verify: Verify::Auto,
            fills: AtomicU64::new(0),
            hits: AtomicU64::new(0),
            mismatches: AtomicU64::new(0),
        }
    }

    pub fn with_verify(mut self, verify: Verify) -> Self {
        self.verify = verify;
        self
    }

    /// The ledger changed: the next [`Self::get`] re-reads it.
    pub fn invalidate(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    /// Deep ledger reads performed to fill the cache (oracle reads excluded).
    pub fn fills(&self) -> u64 {
        self.fills.load(Ordering::Relaxed)
    }

    /// Cross-check failures seen so far.
    pub fn mismatches(&self) -> u64 {
        self.mismatches.load(Ordering::Relaxed)
    }

    /// `doc.read_commands()`, re-read only when the ledger changed since the
    /// last call. Holds the cache lock across the read so concurrent callers
    /// share one fill; the doc callback never takes that lock.
    pub fn get(&self, doc: &SessionDoc) -> Result<Arc<Vec<SessionCommandEntry>>, DocError> {
        let mut cached = lock(&self.cached);
        let generation = self.generation.load(Ordering::SeqCst);
        if let Some((at, commands)) = &*cached
            && *at == generation
        {
            let hits = self.hits.fetch_add(1, Ordering::Relaxed);
            let commands = commands.clone();
            if self.verify_due(hits) {
                // An unreadable fresh ledger is a read error, not evidence the
                // cache is wrong; the cached value stands.
                if let Ok(fresh) = doc.read_commands()
                    && fresh != *commands
                    // A commit between the sample and the oracle read is a
                    // race, not a stale cache: the callback already moved the
                    // generation.
                    && self.generation.load(Ordering::SeqCst) == generation
                {
                    self.mismatches.fetch_add(1, Ordering::Relaxed);
                    tracing::error!(
                        cached = commands.len(),
                        fresh = fresh.len(),
                        "commands cache disagrees with read_commands; dropping it"
                    );
                    let fresh = Arc::new(fresh);
                    *cached = None;
                    return Ok(fresh);
                }
            }
            return Ok(commands);
        }
        self.fills.fetch_add(1, Ordering::Relaxed);
        let commands = Arc::new(doc.read_commands()?);
        *cached = Some((generation, commands.clone()));
        Ok(commands)
    }

    fn verify_due(&self, hits: u64) -> bool {
        match self.verify {
            Verify::Never => false,
            Verify::Always => true,
            Verify::Auto => verify_sampled(hits),
        }
    }
}
