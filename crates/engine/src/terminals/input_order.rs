//! Per-writer ordering for `WriteTerminal` input.
//!
//! RPC requests are served concurrently (one task per frame, on the local engine
//! that forwards and again on the host), so two writes sent back to back may reach
//! the PTY swapped. Clients that stamp `{writerId, seq}` get their bytes applied
//! in `seq` order: early frames are held until the gap fills, and a gap that stays
//! open past [`INPUT_GAP_TIMEOUT`] (a frame lost with a dropped link) releases
//! what is held instead of stalling the terminal.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

/// How long held input waits for a missing earlier frame.
pub(super) const INPUT_GAP_TIMEOUT: Duration = Duration::from_millis(250);
/// Writers tracked per terminal (one per attached tab); the least recently
/// used is forgotten past this.
const MAX_WRITERS: usize = 16;

/// What one accepted frame means for the PTY.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Accepted {
    /// Bytes now writable, in order (possibly empty).
    pub ready: Vec<u8>,
    /// A gap just opened: the caller arms a [`INPUT_GAP_TIMEOUT`] release.
    pub arm_gap_timer: bool,
}

#[derive(Debug)]
struct WriterOrder {
    next: u64,
    held: BTreeMap<u64, Vec<u8>>,
    held_bytes: usize,
    /// When the oldest still-open gap appeared.
    gap_since: Option<Instant>,
    last_used: Instant,
}

impl WriterOrder {
    fn new(now: Instant) -> Self {
        Self {
            next: 0,
            held: BTreeMap::new(),
            held_bytes: 0,
            gap_since: None,
            last_used: now,
        }
    }

    /// Everything held, in order; the sequence resumes after the last frame.
    fn release_held(&mut self) -> Vec<u8> {
        let mut ready = Vec::with_capacity(self.held_bytes);
        for (seq, bytes) in std::mem::take(&mut self.held) {
            ready.extend_from_slice(&bytes);
            self.next = seq + 1;
        }
        self.held_bytes = 0;
        self.gap_since = None;
        ready
    }
}

#[derive(Debug, Default)]
pub(super) struct InputOrder {
    writers: HashMap<String, WriterOrder>,
}

impl InputOrder {
    /// Accept frame `seq` from `writer`. Sequences start at 0 per writer.
    /// `max_held` bounds the bytes parked behind a gap; past it they release.
    pub fn accept(
        &mut self,
        writer: &str,
        seq: u64,
        bytes: Vec<u8>,
        max_held: usize,
        now: Instant,
    ) -> Accepted {
        if !self.writers.contains_key(writer) && self.writers.len() >= MAX_WRITERS {
            let oldest = self
                .writers
                .iter()
                .min_by_key(|(_, order)| order.last_used)
                .map(|(id, _)| id.clone());
            if let Some(oldest) = oldest {
                self.writers.remove(&oldest);
            }
        }
        let order = self
            .writers
            .entry(writer.to_string())
            .or_insert_with(|| WriterOrder::new(now));
        order.last_used = now;

        if seq < order.next {
            // Arrived after its gap was given up on: late beats lost input.
            return Accepted {
                ready: bytes,
                arm_gap_timer: false,
            };
        }
        if seq > order.next {
            order.held_bytes += bytes.len();
            order.held.insert(seq, bytes);
            if order.held_bytes > max_held {
                return Accepted {
                    ready: order.release_held(),
                    arm_gap_timer: false,
                };
            }
            let opened = order.gap_since.is_none();
            if opened {
                order.gap_since = Some(now);
            }
            return Accepted {
                ready: Vec::new(),
                arm_gap_timer: opened,
            };
        }

        let mut ready = bytes;
        order.next += 1;
        while let Some(bytes) = order.held.remove(&order.next) {
            order.held_bytes -= bytes.len();
            ready.extend_from_slice(&bytes);
            order.next += 1;
        }
        // Frames still held sit behind a newer gap: restart its clock. The
        // timer armed for the first gap is still pending and re-reads it.
        order.gap_since = (!order.held.is_empty()).then_some(now);
        Accepted {
            ready,
            arm_gap_timer: false,
        }
    }

    /// Gap timer check for `writer`: `Ok(bytes)` once its gap has been open
    /// for [`INPUT_GAP_TIMEOUT`], `Err(Some(deadline))` to wait longer, or
    /// `Err(None)` when no gap remains.
    pub fn expire_gap(&mut self, writer: &str, now: Instant) -> Result<Vec<u8>, Option<Instant>> {
        let Some(order) = self.writers.get_mut(writer) else {
            return Err(None);
        };
        let Some(since) = order.gap_since else {
            return Err(None);
        };
        let deadline = since + INPUT_GAP_TIMEOUT;
        if now < deadline {
            return Err(Some(deadline));
        }
        Ok(order.release_held())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAX: usize = 64 * 1024;

    fn accept(order: &mut InputOrder, seq: u64, bytes: &str, now: Instant) -> Accepted {
        order.accept("w", seq, bytes.as_bytes().to_vec(), MAX, now)
    }

    #[test]
    fn in_order_frames_pass_straight_through() {
        let mut order = InputOrder::default();
        let now = Instant::now();
        assert_eq!(accept(&mut order, 0, "a", now).ready, b"a");
        assert_eq!(accept(&mut order, 1, "b", now).ready, b"b");
    }

    #[test]
    fn swapped_frames_are_reordered() {
        let mut order = InputOrder::default();
        let now = Instant::now();
        let early = accept(&mut order, 1, "b", now);
        assert!(early.ready.is_empty());
        assert!(early.arm_gap_timer);
        assert_eq!(accept(&mut order, 0, "a", now).ready, b"ab");
        assert_eq!(order.expire_gap("w", now), Err(None));
    }

    #[test]
    fn writers_are_independent() {
        let mut order = InputOrder::default();
        let now = Instant::now();
        assert!(
            order
                .accept("x", 1, b"x1".to_vec(), MAX, now)
                .ready
                .is_empty()
        );
        assert_eq!(order.accept("y", 0, b"y0".to_vec(), MAX, now).ready, b"y0");
        assert_eq!(
            order.accept("x", 0, b"x0".to_vec(), MAX, now).ready,
            b"x0x1"
        );
    }

    #[test]
    fn stale_gap_releases_held_input_and_resumes_after_it() {
        let mut order = InputOrder::default();
        let now = Instant::now();
        accept(&mut order, 1, "b", now);
        accept(&mut order, 2, "c", now);
        assert_eq!(
            order.expire_gap("w", now + Duration::from_millis(10)),
            Err(Some(now + INPUT_GAP_TIMEOUT))
        );
        assert_eq!(
            order.expire_gap("w", now + INPUT_GAP_TIMEOUT),
            Ok(b"bc".to_vec())
        );
        assert_eq!(accept(&mut order, 3, "d", now).ready, b"d");
        // The lost frame showing up late is still written, not dropped.
        assert_eq!(accept(&mut order, 0, "a", now).ready, b"a");
    }

    #[test]
    fn a_second_gap_restarts_the_clock() {
        let mut order = InputOrder::default();
        let start = Instant::now();
        accept(&mut order, 1, "b", start);
        accept(&mut order, 3, "d", start);
        let later = start + Duration::from_millis(200);
        let filled = accept(&mut order, 0, "a", later);
        assert_eq!(filled.ready, b"ab");
        assert!(!filled.arm_gap_timer);
        assert_eq!(
            order.expire_gap("w", start + INPUT_GAP_TIMEOUT),
            Err(Some(later + INPUT_GAP_TIMEOUT))
        );
    }

    #[test]
    fn held_bytes_are_bounded() {
        let mut order = InputOrder::default();
        let now = Instant::now();
        assert!(
            order
                .accept("w", 1, vec![b'b'; 8], 10, now)
                .ready
                .is_empty()
        );
        assert_eq!(order.accept("w", 2, vec![b'c'; 8], 10, now).ready.len(), 16);
        assert_eq!(order.expire_gap("w", now + INPUT_GAP_TIMEOUT), Err(None));
    }

    #[test]
    fn least_recently_used_writer_is_forgotten() {
        let mut order = InputOrder::default();
        let start = Instant::now();
        for i in 0..MAX_WRITERS {
            let now = start + Duration::from_millis(i as u64);
            order.accept(&format!("w{i}"), 0, b"x".to_vec(), MAX, now);
        }
        order.accept("new", 0, b"x".to_vec(), MAX, start + Duration::from_secs(1));
        assert_eq!(order.writers.len(), MAX_WRITERS);
        assert!(!order.writers.contains_key("w0"));
    }
}
