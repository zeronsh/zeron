//! Files move in fixed-size blocks, each carrying its own SHA-256. A block
//! the receiver verified and wrote is never sent again: that set is what a
//! resumed session reports back to the sender.

use serde::{Deserialize, Serialize};

/// Block size: the resume granularity and the largest data frame.
pub const BLOCK_SIZE: u64 = 1024 * 1024;

pub fn block_count(size: u64) -> u64 {
    size.div_ceil(BLOCK_SIZE)
}

/// Length of block `index` in a file of `size` bytes.
pub fn block_len(size: u64, index: u64) -> u64 {
    let start = index * BLOCK_SIZE;
    size.saturating_sub(start).min(BLOCK_SIZE)
}

/// Sorted, disjoint, half-open ranges of block indexes. Serialized as
/// `[[start, end], …]` — compact for the common "a prefix is done" shape.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RangeSet(Vec<(u64, u64)>);

impl RangeSet {
    pub fn full(count: u64) -> Self {
        if count == 0 {
            Self::default()
        } else {
            Self(vec![(0, count)])
        }
    }

    pub fn len(&self) -> u64 {
        self.0.iter().map(|(s, e)| e - s).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn contains(&self, index: u64) -> bool {
        let at = self.0.partition_point(|(_, end)| *end <= index);
        self.0.get(at).is_some_and(|(start, _)| *start <= index)
    }

    /// Add `index`; returns false when it was already present.
    pub fn insert(&mut self, index: u64) -> bool {
        let at = self.0.partition_point(|(_, end)| *end < index);
        if let Some((start, end)) = self.0.get_mut(at) {
            if *start <= index && index < *end {
                return false;
            }
            if *end == index {
                *end += 1;
                // Coalesce with the next range when they now touch.
                if let Some(&(next_start, next_end)) = self.0.get(at + 1)
                    && next_start == index + 1
                {
                    self.0[at].1 = next_end;
                    self.0.remove(at + 1);
                }
                return true;
            }
            if *start == index + 1 {
                *start = index;
                return true;
            }
        }
        self.0.insert(at, (index, index + 1));
        true
    }

    /// Indexes below `count` not in the set, ascending.
    pub fn missing(&self, count: u64) -> Vec<u64> {
        let mut out = Vec::new();
        let mut next = 0;
        for &(start, end) in &self.0 {
            out.extend(next.min(count)..start.min(count));
            next = end;
        }
        out.extend(next.min(count)..count);
        out
    }

    /// Bytes these blocks cover in a file of `size` bytes.
    pub fn bytes(&self, size: u64) -> u64 {
        let count = block_count(size);
        self.0
            .iter()
            .map(|&(start, end)| {
                let end = end.min(count);
                if start >= end {
                    return 0;
                }
                let short = if end == count {
                    BLOCK_SIZE - block_len(size, count - 1)
                } else {
                    0
                };
                (end - start) * BLOCK_SIZE - short
            })
            .sum()
    }

    /// Reject sets that name blocks a file doesn't have (a peer's report).
    pub fn within(&self, count: u64) -> bool {
        self.0.iter().all(|(s, e)| s < e && *e <= count)
            && self.0.windows(2).all(|w| w[0].1 < w[1].0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunking_covers_every_byte_once() {
        assert_eq!(block_count(0), 0);
        assert_eq!(block_count(1), 1);
        assert_eq!(block_count(BLOCK_SIZE), 1);
        assert_eq!(block_count(BLOCK_SIZE + 1), 2);
        let size = 5 * BLOCK_SIZE + 123;
        let total: u64 = (0..block_count(size)).map(|i| block_len(size, i)).sum();
        assert_eq!(total, size);
        assert_eq!(block_len(size, 5), 123);
        assert_eq!(block_len(size, 6), 0);
    }

    #[test]
    fn range_set_inserts_coalesce_and_report_missing() {
        let mut set = RangeSet::default();
        for i in [5, 0, 1, 3, 2, 9, 4] {
            assert!(set.insert(i));
        }
        assert!(!set.insert(3));
        assert_eq!(set, RangeSet(vec![(0, 6), (9, 10)]));
        assert_eq!(set.len(), 7);
        assert!(set.contains(0) && set.contains(5) && set.contains(9));
        assert!(!set.contains(6) && !set.contains(10));
        assert_eq!(set.missing(12), vec![6, 7, 8, 10, 11]);
        assert_eq!(set.missing(4), Vec::<u64>::new());
        assert!(set.within(10));
        assert!(!set.within(9));
        assert!(!RangeSet(vec![(3, 3)]).within(10));
        assert!(!RangeSet(vec![(0, 4), (2, 6)]).within(10));
        assert_eq!(RangeSet::full(3).missing(3), Vec::<u64>::new());
        let size = 3 * BLOCK_SIZE + 10;
        assert_eq!(RangeSet::full(4).bytes(size), size);
        assert_eq!(RangeSet(vec![(0, 1), (3, 4)]).bytes(size), BLOCK_SIZE + 10);
        let json = serde_json::to_string(&set).unwrap();
        assert_eq!(json, "[[0,6],[9,10]]");
    }
}
