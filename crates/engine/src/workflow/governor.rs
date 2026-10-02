//! Concurrency control: the per-model AIMD governor and the admission gate.
//!
//! [`Governor`] is pure (time is a parameter). Multiplicative decrease on a
//! rate-limit error (cap x 0.75, never below 1, plus an optional cooldown
//! from `Retry-After`), additive increase (+1) after four consecutive
//! successes, and a full reset once the model has been quiet for five
//! minutes. [`Gate`] is the async counting gate the scheduler admits asks
//! through; its cap can move while asks hold permits.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tokio::sync::Notify;

pub const SUCCESSES_PER_STEP: u32 = 4;
pub const IDLE_RESET_MS: u64 = 5 * 60 * 1000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Governor {
    ceiling: u32,
    cap: u32,
    successes: u32,
    last_event_ms: u64,
    cooldown_until_ms: u64,
}

impl Governor {
    pub fn new(ceiling: u32) -> Self {
        let ceiling = ceiling.max(1);
        Self {
            ceiling,
            cap: ceiling,
            successes: 0,
            last_event_ms: 0,
            cooldown_until_ms: 0,
        }
    }

    pub fn ceiling(&self) -> u32 {
        self.ceiling
    }

    /// The current cap (applies the idle reset first).
    pub fn cap(&mut self, now_ms: u64) -> u32 {
        if self.cap < self.ceiling && now_ms.saturating_sub(self.last_event_ms) >= IDLE_RESET_MS {
            self.cap = self.ceiling;
            self.successes = 0;
        }
        self.cap
    }

    /// A success; `true` when the cap moved.
    pub fn on_success(&mut self, now_ms: u64) -> bool {
        let before = self.cap(now_ms);
        self.last_event_ms = now_ms;
        self.successes += 1;
        if self.successes >= SUCCESSES_PER_STEP {
            self.successes = 0;
            if self.cap < self.ceiling {
                self.cap += 1;
            }
        }
        self.cap != before
    }

    /// A rate-limit error; `true` when the cap moved. `retry_after_ms`
    /// opens a cooldown during which [`Self::cooldown_remaining_ms`] is >0.
    pub fn on_rate_limit(&mut self, now_ms: u64, retry_after_ms: Option<u64>) -> bool {
        let before = self.cap(now_ms);
        self.last_event_ms = now_ms;
        self.successes = 0;
        self.cap = ((self.cap as f64 * 0.75).floor() as u32).max(1);
        if let Some(ms) = retry_after_ms {
            self.cooldown_until_ms = self.cooldown_until_ms.max(now_ms + ms);
        }
        self.cap != before
    }

    /// Time new asks of this model should still hold off.
    pub fn cooldown_remaining_ms(&self, now_ms: u64) -> u64 {
        self.cooldown_until_ms.saturating_sub(now_ms)
    }

    pub fn is_throttled(&self) -> bool {
        self.cap < self.ceiling
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A counting gate with a movable cap.
pub struct Gate {
    state: Mutex<GateState>,
    notify: Notify,
}

struct GateState {
    in_flight: u32,
    cap: u32,
}

/// Holds one slot; dropping it frees the slot.
pub struct Permit {
    gate: Arc<Gate>,
}

impl Drop for Permit {
    fn drop(&mut self) {
        lock(&self.gate.state).in_flight -= 1;
        self.gate.notify.notify_waiters();
    }
}

impl Gate {
    pub fn new(cap: u32) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(GateState {
                in_flight: 0,
                cap: cap.max(1),
            }),
            notify: Notify::new(),
        })
    }

    pub fn set_cap(&self, cap: u32) {
        lock(&self.state).cap = cap.max(1);
        self.notify.notify_waiters();
    }

    pub fn cap(&self) -> u32 {
        lock(&self.state).cap
    }

    pub fn in_flight(&self) -> u32 {
        lock(&self.state).in_flight
    }

    /// Wait for a free slot.
    pub async fn acquire(self: &Arc<Self>) -> Permit {
        loop {
            // Register interest before checking, so a release between the
            // check and the wait is not lost.
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            {
                let mut s = lock(&self.state);
                if s.in_flight < s.cap {
                    s.in_flight += 1;
                    return Permit { gate: self.clone() };
                }
            }
            notified.await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn rate_limits_cut_the_cap_by_a_quarter_never_below_one() {
        let mut g = Governor::new(8);
        let mut caps = Vec::new();
        for i in 0..6 {
            g.on_rate_limit(1000 + i, None);
            caps.push(g.cap(1000 + i));
        }
        assert_eq!(caps, [6, 4, 3, 2, 1, 1]);
        assert!(g.is_throttled());
    }

    #[test]
    fn four_successes_add_one_slot_up_to_the_ceiling() {
        let mut g = Governor::new(4);
        g.on_rate_limit(0, None); // 4 -> 3
        g.on_rate_limit(1, None); // 3 -> 2
        assert_eq!(g.cap(2), 2);
        for i in 0..3 {
            assert!(!g.on_success(10 + i));
        }
        assert!(g.on_success(20));
        assert_eq!(g.cap(21), 3);
        for i in 0..4 {
            g.on_success(30 + i);
        }
        assert_eq!(g.cap(40), 4);
        for i in 0..20 {
            assert!(!g.on_success(50 + i), "never above the ceiling");
        }
        // A rate limit resets the success streak.
        g.on_rate_limit(100, None);
        for i in 0..3 {
            g.on_success(110 + i);
        }
        g.on_rate_limit(120, None);
        g.on_success(130);
        assert_eq!(g.cap(131), 2);
    }

    #[test]
    fn five_idle_minutes_restore_the_ceiling() {
        let mut g = Governor::new(8);
        g.on_rate_limit(1_000, None);
        g.on_rate_limit(1_001, None);
        assert_eq!(g.cap(2_000), 4);
        assert_eq!(g.cap(1_001 + IDLE_RESET_MS - 1), 4);
        assert_eq!(g.cap(1_001 + IDLE_RESET_MS), 8);
        assert!(!g.is_throttled());
    }

    #[test]
    fn retry_after_opens_a_cooldown() {
        let mut g = Governor::new(4);
        g.on_rate_limit(1_000, Some(30_000));
        assert_eq!(g.cooldown_remaining_ms(1_000), 30_000);
        assert_eq!(g.cooldown_remaining_ms(21_000), 10_000);
        assert_eq!(g.cooldown_remaining_ms(40_000), 0);
        g.on_rate_limit(2_000, Some(1_000)); // a shorter one never shortens it
        assert_eq!(g.cooldown_remaining_ms(1_000), 30_000);
    }

    #[tokio::test]
    async fn the_gate_admits_up_to_its_cap_and_follows_a_moving_cap() {
        let gate = Gate::new(2);
        let a = gate.acquire().await;
        let _b = gate.acquire().await;
        let waiting = {
            let gate = gate.clone();
            tokio::spawn(async move { gate.acquire().await })
        };
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(!waiting.is_finished(), "a third ask waits");
        assert_eq!(gate.in_flight(), 2);
        drop(a);
        let c = tokio::time::timeout(Duration::from_secs(1), waiting)
            .await
            .expect("admitted on release")
            .unwrap();
        // Raising the cap admits another waiter; lowering it does not evict.
        let more = {
            let gate = gate.clone();
            tokio::spawn(async move { gate.acquire().await })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!more.is_finished());
        gate.set_cap(3);
        let d = tokio::time::timeout(Duration::from_secs(1), more)
            .await
            .unwrap()
            .unwrap();
        gate.set_cap(1);
        assert_eq!(gate.in_flight(), 3, "running asks are never cut");
        drop((c, d));
        assert_eq!(gate.in_flight(), 1);
    }
}
