//! A precise, bounded clock owned only while pulse animations are mounted.
use futures::channel::oneshot;
use std::{
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    sync::mpsc,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::WAIT_OBJECT_0,
    System::Threading::{
        CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, CreateWaitableTimerExW, SetWaitableTimerEx,
        TIMER_ALL_ACCESS, WaitForSingleObject,
    },
};

pub(super) struct Clock {
    requests: mpsc::Sender<(Instant, oneshot::Sender<bool>)>,
    deadline: Instant,
    interval: Duration,
}
impl Clock {
    pub(super) fn new(interval: Duration) -> Option<Self> {
        // No process-wide timer-resolution change. Older systems can use the
        // existing executor timer when this per-handle facility is unavailable.
        let raw = unsafe {
            CreateWaitableTimerExW(
                std::ptr::null(),
                std::ptr::null(),
                CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
                TIMER_ALL_ACCESS,
            )
        };
        if raw.is_null() {
            return None;
        }
        let timer = unsafe { OwnedHandle::from_raw_handle(raw) };
        let (requests, receiver) = mpsc::channel::<(Instant, oneshot::Sender<bool>)>();
        std::thread::Builder::new()
            .name("animation-pulse".into())
            .spawn(move || {
                for (deadline, response) in receiver {
                    let delay = deadline.saturating_duration_since(Instant::now());
                    let due = -(delay.as_nanos().div_ceil(100).clamp(1, i64::MAX as u128) as i64);
                    let handle = timer.as_raw_handle();
                    let armed = unsafe {
                        SetWaitableTimerEx(
                            handle,
                            &due,
                            0,
                            None,
                            std::ptr::null(),
                            std::ptr::null(),
                            0,
                        )
                    } != 0;
                    let ok = armed && unsafe { WaitForSingleObject(handle, 1000) } == WAIT_OBJECT_0;
                    if response.send(ok).is_err() || !ok {
                        break;
                    }
                }
                // Receiver disconnects when the animation task parks or is dropped.
                // OwnedHandle closes the timer on every exit, including failures.
            })
            .ok()?;
        Some(Self {
            requests,
            deadline: Instant::now(),
            interval,
        })
    }
    pub(super) async fn tick(&mut self) -> bool {
        let now = Instant::now();
        self.deadline += self.interval;
        if self.deadline <= now {
            self.deadline = now + self.interval;
        }
        let (send, receive) = oneshot::channel();
        if self.requests.send((self.deadline, send)).is_err() {
            return false;
        }
        receive.await.unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_high_resolution_ticks_are_delivered() {
        // Exercise the real timer with a thread-safe executor, independently
        // of GPUI's deterministic scheduler and its thread-affine wakeups.
        let mut clock = Clock::new(Duration::from_millis(2))
            .expect("Windows high-resolution timer must be available");
        for _ in 0..3 {
            assert!(futures::executor::block_on(clock.tick()));
        }
    }
}
