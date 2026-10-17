//! Leading-edge batching for PTY output (pure; `pump_output` owns the timer).
//!
//! Output after a quiet window is emitted at once, so a keystroke's echo never
//! waits out a batch. Output arriving inside the window after an emit is held
//! and rides one event when the window closes — a flood still emits at most
//! once per window, exactly like a trailing batch.

use std::time::Duration;

use tokio::time::Instant;

/// What `pump_output` does with a chunk.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum OutputStep {
    /// Quiet window: emit these bytes now.
    Emit(Vec<u8>),
    /// First bytes held inside a window: arm the flush timer for its end.
    Arm(Instant),
    /// Held behind an already armed flush.
    Wait,
}

#[derive(Debug)]
pub(super) struct OutputBatch {
    window: Duration,
    buffer: Vec<u8>,
    window_end: Option<Instant>,
}

impl OutputBatch {
    pub fn new(window: Duration) -> Self {
        Self {
            window,
            buffer: Vec::new(),
            window_end: None,
        }
    }

    pub fn push(&mut self, chunk: &[u8], now: Instant) -> OutputStep {
        let was_empty = self.buffer.is_empty();
        self.buffer.extend_from_slice(chunk);
        match self.window_end {
            Some(end) if now < end => {
                if was_empty {
                    OutputStep::Arm(end)
                } else {
                    OutputStep::Wait
                }
            }
            _ => OutputStep::Emit(self.flush(now)),
        }
    }

    /// Everything held, opening the next window at `now`.
    pub fn flush(&mut self, now: Instant) -> Vec<u8> {
        self.window_end = Some(now + self.window);
        std::mem::take(&mut self.buffer)
    }

    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOW: Duration = Duration::from_millis(12);

    #[test]
    fn quiet_output_is_emitted_immediately() {
        let mut batch = OutputBatch::new(WINDOW);
        let t0 = Instant::now();
        assert_eq!(batch.push(b"a", t0), OutputStep::Emit(b"a".to_vec()));
        // After the window closes, the next echo is immediate again.
        assert_eq!(
            batch.push(b"b", t0 + WINDOW),
            OutputStep::Emit(b"b".to_vec())
        );
        assert!(batch.is_empty());
    }

    #[test]
    fn output_inside_the_window_rides_one_flush_at_its_end() {
        let mut batch = OutputBatch::new(WINDOW);
        let t0 = Instant::now();
        batch.push(b"a", t0);
        let t1 = t0 + Duration::from_millis(3);
        assert_eq!(batch.push(b"b", t1), OutputStep::Arm(t0 + WINDOW));
        assert_eq!(batch.push(b"c", t1), OutputStep::Wait);
        assert_eq!(batch.flush(t0 + WINDOW), b"bc".to_vec());
        // The flush opened a new window: a flood keeps one event per window.
        let t2 = t0 + WINDOW + Duration::from_millis(1);
        assert_eq!(batch.push(b"d", t2), OutputStep::Arm(t0 + WINDOW * 2));
    }

    #[test]
    fn held_output_goes_with_a_chunk_that_lands_after_the_window() {
        let mut batch = OutputBatch::new(WINDOW);
        let t0 = Instant::now();
        batch.push(b"a", t0);
        batch.push(b"b", t0 + Duration::from_millis(1));
        // The timer has not fired yet, but the window is over: emit it all.
        assert_eq!(
            batch.push(b"c", t0 + WINDOW),
            OutputStep::Emit(b"bc".to_vec())
        );
    }
}
