//! Suspend/resume detection without OS hooks — wake is an EVENT, not a
//! timeout.
//!
//! A 1s ticker compares wall-clock elapsed to monotonic elapsed since the
//! previous tick. Monotonic clocks (macOS `mach_absolute_time`, Linux
//! `CLOCK_MONOTONIC`) exclude suspend, so a wall jump far beyond the tick
//! means the process just woke from system sleep. Subscribers — room actors,
//! relay links, the token refresh loop — reconnect/refresh immediately
//! instead of discovering half-open sockets by silence-lease timeout
//! (Discord/Slack-style instant recovery; user report: "doesn't fix until I
//! restart the app" / "shouldn't take a minute").
//!
//! The detector task is a lazily-spawned process-wide singleton; `subscribe`
//! must first be called from within a tokio runtime (every caller is an
//! async context). Broadcast receivers that lag simply miss duplicate wake
//! events — each subscriber treats ANY received event as "reconnect now",
//! so a missed one is at worst covered by the next silence lease.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

use tokio::sync::{broadcast, watch};

const TICK: Duration = Duration::from_secs(1);
/// Wall time must outrun monotonic time by this much in one tick to count as
/// a suspend — far above scheduler jitter, far below any real sleep.
const JUMP_THRESHOLD: Duration = Duration::from_secs(5);

static CHANNEL: OnceLock<broadcast::Sender<()>> = OnceLock::new();
static ONLINE: OnceLock<broadcast::Sender<()>> = OnceLock::new();
/// OS-reported network path status. `false` (the default, and the permanent
/// value on platforms without a path monitor) means "assume a path exists" —
/// backoff loops behave exactly as before. Only an explicit
/// [`set_path_online(false)`] from a platform monitor flips this true.
static PATH_OFFLINE: AtomicBool = AtomicBool::new(false);

/// See [`set_resting`].
static RESTING: OnceLock<watch::Sender<bool>> = OnceLock::new();

/// Set once the OS delivers wake events itself ([`use_os_wake_events`]).
static OS_EVENTS: AtomicBool = AtomicBool::new(false);

/// Subscribe to system-wake events (the detector spawns on first call, unless
/// the OS delivers them: [`use_os_wake_events`]).
pub fn subscribe() -> broadcast::Receiver<()> {
    CHANNEL
        .get_or_init(|| {
            let (tx, _) = broadcast::channel(4);
            if !OS_EVENTS.load(Ordering::Acquire) {
                spawn_detector(tx.clone());
            }
            tx
        })
        .subscribe()
}

/// The process's OS tells it of each wake ([`notify_wake`]), so the
/// clock-comparing detector, which wakes the process every [`TICK`] to look,
/// isn't needed: it doesn't start, or stops at its next tick. Call before the
/// first [`subscribe`] where possible (the macOS background engine registers
/// for IOKit power notifications before its engine starts).
pub fn use_os_wake_events() {
    OS_EVENTS.store(true, Ordering::Release);
}

/// The system just woke (from the OS's own notification): what the detector
/// would have broadcast.
pub fn notify_wake() {
    tracing::info!("wake: system resumed from sleep");
    if let Some(tx) = CHANNEL.get() {
        let _ = tx.send(());
    }
}

fn spawn_detector(detector: broadcast::Sender<()>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(TICK);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        interval.tick().await; // consume the immediate first tick
        let mut wall = SystemTime::now();
        let mut mono = Instant::now();
        loop {
            interval.tick().await;
            if OS_EVENTS.load(Ordering::Acquire) {
                return;
            }
            let wall_elapsed = SystemTime::now()
                .duration_since(wall)
                .unwrap_or(Duration::ZERO);
            let mono_elapsed = mono.elapsed();
            if wall_elapsed > mono_elapsed + JUMP_THRESHOLD {
                tracing::info!(
                    slept_s = wall_elapsed.as_secs(),
                    "wake: system resumed from suspend"
                );
                let _ = detector.send(());
            }
            wall = SystemTime::now();
            mono = Instant::now();
        }
    });
}

/// A ping or heartbeat interval whose ticks fall on whole multiples of
/// `period` of wall-clock time (Unix epoch), so this process's periodic beats
/// — registry and chat socket pings, the device room's, peer links', the
/// presence beat — land on the same instants and wake it once, not once
/// each. A 10 s and a 15 s beat then meet every 30 s. The first tick is the
/// next such instant (within one period), not at once. Missed ticks are
/// skipped, which keeps the ticks on the grid.
pub fn beat_interval(period: Duration) -> tokio::time::Interval {
    let mut interval = tokio::time::interval_at(next_beat(period), period);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    interval
}

/// The next instant that is a whole multiple of `period` since the Unix
/// epoch. One less than 100 ms away is skipped for the one after: a sleep
/// that woke on a beat (the wall clock a hair behind the monotonic one) must
/// not find that same beat again.
pub fn next_beat(period: Duration) -> tokio::time::Instant {
    let now = tokio::time::Instant::now();
    let period_ns = period.as_nanos().max(1);
    let wall_ns = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_nanos();
    let mut wait = period_ns - wall_ns % period_ns;
    if wait < 100_000_000 && period_ns > 200_000_000 {
        wait += period_ns;
    }
    now + Duration::from_nanos(wait.min(u64::MAX as u128) as u64)
}

fn online_channel() -> &'static broadcast::Sender<()> {
    ONLINE.get_or_init(|| broadcast::channel(4).0)
}

/// Subscribe to connectivity-regained events. There is no cross-platform OS
/// hook for "the wifi came back", so the signal is empirical: ANY successful
/// WebSocket dial in this process ([`crate::dial::connect_ws`]) broadcasts
/// here, and every socket waiting out a reconnect backoff treats it as
/// "redial now with fresh backoff". One recovered socket un-parks the whole
/// fleet instead of each one sleeping out its own (up to 30s) delay — the
/// network-flap sibling of the suspend/resume event above. Waiters should
/// `try_recv`-drain stale events before arming, so only successes that happen
/// DURING their wait cut it short.
pub fn subscribe_online() -> broadcast::Receiver<()> {
    online_channel().subscribe()
}

/// Broadcast that a dial just succeeded (see [`subscribe_online`]).
pub fn notify_online() {
    let _ = online_channel().send(());
}

/// Report the OS network-path status (macOS `NWPathMonitor`; other platforms
/// simply never call this). Two effects:
///
/// - offline → online broadcasts an online event, so every parked backoff
///   redials the instant the path returns instead of waiting out its timer;
/// - while offline, [`path_is_offline`] tells backoff waiters to park on the
///   event buses rather than burn dial attempts the OS says cannot succeed.
pub fn set_path_online(online: bool) {
    let was_offline = PATH_OFFLINE.swap(!online, Ordering::Relaxed);
    if online && was_offline {
        tracing::info!("wake: network path restored");
        notify_online();
    } else if !online && !was_offline {
        tracing::info!("wake: network path lost; parking reconnect backoffs");
    }
}

/// True while the OS reports no viable network path. Waiters treat this as
/// "park until an event", never as a hard guarantee — the monitor may be
/// absent or wrong, so parked waits keep a coarse safety timer.
pub fn path_is_offline() -> bool {
    PATH_OFFLINE.load(Ordering::Relaxed)
}

fn resting() -> &'static watch::Sender<bool> {
    RESTING.get_or_init(|| watch::channel(false).0)
}

/// Nobody's using this process, and the OS may not suspend it for hours (Android
/// before 14 can leave one running). Sockets close and nothing dials, polls or retries
/// until this is cleared, so an idle phone's radio can sleep. Unlike [`path_is_offline`]
/// there's no safety timer: the network works, so a dial would succeed and wake it.
/// Nothing durable depends on this: outboxes hold their rows and resume on return.
pub fn set_resting(rest: bool) {
    let was = resting().send_replace(rest);
    if was && !rest {
        notify_online();
    }
}

pub fn is_resting() -> bool {
    *resting().borrow()
}

/// Resolves when [`set_resting`] says to let go.
pub async fn until_resting() {
    let _ = resting().subscribe().wait_for(|rest| *rest).await.is_ok();
}

/// Resolves when nothing's resting, at once if nothing is.
pub async fn until_awake() {
    let _ = resting().subscribe().wait_for(|rest| !*rest).await.is_ok();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Beats land on whole multiples of their period of wall time, so a 10 s
    /// and a 15 s beat meet, and the next one is never the one just passed.
    #[tokio::test]
    async fn beats_fall_on_the_wall_clock_grid() {
        for period in [Duration::from_secs(10), Duration::from_secs(15)] {
            let at = next_beat(period);
            let wait = at - tokio::time::Instant::now();
            assert!(wait >= Duration::from_millis(99) && wait <= period + Duration::from_millis(100));
            let wall = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap() + wait;
            let off = (wall.as_millis() % period.as_millis()) as i64;
            let off = off.min(period.as_millis() as i64 - off);
            assert!(off < 20, "{period:?} beat {off} ms off the grid");
        }
    }
}
