//! Incremental reads of the registry's typed lists.
//!
//! Every mutation of a row's overlay (its authoritative row or a pending op
//! on it) records the row's `(kind, id)` here. A typed read then re-parses
//! only the rows touched since that read, instead of every row of the kind:
//! on a five-device account the registry changes about twice a second and
//! holds hundreds of chats and sessions, and each full read decoded all of
//! them. Per-kind change counters let a publisher skip lists whose kind
//! didn't change at all.

use std::any::{Any, TypeId};
use std::collections::{HashMap, HashSet};

/// One cached typed list: the parsed value of each live row, by id.
struct Entry {
    kind: String,
    /// `HashMap<String, T>` for the entry's `T`.
    rows: Box<dyn Any + Send>,
    /// Ids whose overlay changed since the last read of this entry.
    dirty: HashSet<String>,
    /// Everything may have changed (a full replace): rebuild on next read.
    all: bool,
}

#[derive(Default)]
pub(super) struct RowCache {
    entries: HashMap<(String, TypeId), Entry>,
    /// Per-kind change counters, plus an epoch for changes to every kind.
    versions: HashMap<String, u64>,
    epoch: u64,
}

impl RowCache {
    /// `(kind, id)`'s overlay may have changed.
    pub(super) fn touch(&mut self, kind: &str, id: &str) {
        for entry in self.entries.values_mut() {
            if entry.kind == kind && !entry.all {
                entry.dirty.insert(id.to_owned());
            }
        }
        *self.versions.entry(kind.to_owned()).or_default() += 1;
    }

    /// Any row of any kind may have changed.
    pub(super) fn touch_all(&mut self) {
        for entry in self.entries.values_mut() {
            entry.all = true;
            entry.dirty.clear();
        }
        self.epoch += 1;
    }

    /// Changes whenever any row of `kind` may have changed.
    pub(super) fn version(&self, kind: &str) -> u64 {
        self.epoch + self.versions.get(kind).copied().unwrap_or(0)
    }

    /// The typed list of `kind`, brought up to date: `rebuild` produces every
    /// live row (first read, or after a full replace); `refresh` re-reads one
    /// id (`None` when it no longer shows). Returns the values in no order.
    /// `list` names the typed list (its raw and output types), so two
    /// readers of one kind never share an entry.
    pub(super) fn read<T: Clone + Send + 'static>(
        &mut self,
        kind: &str,
        list: TypeId,
        rebuild: impl FnOnce() -> Vec<(String, T)>,
        mut refresh: impl FnMut(&str) -> Option<T>,
    ) -> Vec<T> {
        let key = (kind.to_owned(), list);
        let entry = self.entries.entry(key).or_insert_with(|| Entry {
            kind: kind.to_owned(),
            rows: Box::new(HashMap::<String, T>::new()),
            dirty: HashSet::new(),
            all: true,
        });
        let rows = entry
            .rows
            .downcast_mut::<HashMap<String, T>>()
            .expect("one entry per (kind, type)");
        if entry.all {
            *rows = rebuild().into_iter().collect();
            entry.all = false;
            entry.dirty.clear();
        } else {
            for id in entry.dirty.drain() {
                match refresh(&id) {
                    Some(value) => {
                        rows.insert(id, value);
                    }
                    None => {
                        rows.remove(&id);
                    }
                }
            }
        }
        rows.values().cloned().collect()
    }
}
