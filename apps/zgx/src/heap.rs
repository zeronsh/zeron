//! The allocator's startup peak, handed back once launch has settled.
//!
//! Launch allocates far more than it keeps: compiling and running the
//! interface's scripts, the plugins', the engine's first answers and the
//! agents' model lists commit 150-180 MB of mimalloc's heap for about 27 MB
//! in use a minute later. mimalloc keeps the freed pages dirty for reuse,
//! and the system counts them against Zeron (`footprint`'s "App-Specific
//! Tag 1"). Once the window has settled, a forced collect returns them; the
//! heap commits again as it needs to, as it would anyway.
//!
//! `ZERON_ZGX_HEAP_STATS=1` prints mimalloc's own statistics (by size class,
//! peak and current) to stderr before and after each collect;
//! `ZERON_ZGX_HEAP_COLLECT=0` leaves the heap as it is, to compare.

use std::time::Duration;
use zgx_desktop::WindowContext;

/// When the collects run, from the main window's opening: after the first
/// engine answers, plugins and model lists (about 4 s on a warm Mac), and
/// once more after a session's first load.
const AFTER: [Duration; 2] = [Duration::from_secs(10), Duration::from_secs(60)];

unsafe extern "C" {
    // mimalloc's own (mimalloc.h), in the library zgx-desktop's `mimalloc`
    // feature links as the program's allocator.
    fn mi_collect(force: bool);
    fn mi_stats_print(out: *mut std::ffi::c_void);
}

thread_local! {
    static INSTALLED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The collects, scheduled on the main window's tasks (once a process).
pub fn install(cx: &mut WindowContext) {
    if INSTALLED.with(|i| i.replace(true)) {
        return;
    }
    if std::env::var("ZERON_ZGX_HEAP_COLLECT").is_ok_and(|v| v == "0") {
        return;
    }
    let stats = std::env::var("ZERON_ZGX_HEAP_STATS").is_ok_and(|v| v == "1");
    cx.tasks.spawn(async move {
        let mut waited = Duration::ZERO;
        for at in AFTER {
            zgx::timer::sleep(at - waited).await;
            waited = at;
            collect(stats);
        }
    });
}

/// Hands mimalloc's free pages back to the system (on the main thread,
/// whose heap holds what the scripts and the interface allocate).
fn collect(stats: bool) {
    if stats {
        eprintln!("zeron-zgx: heap: before collect");
        // SAFETY: mimalloc's printer with no output callback (stderr).
        unsafe { mi_stats_print(std::ptr::null_mut()) };
    }
    // SAFETY: mimalloc's collect, on the calling thread's heap and its free
    // arenas; it frees nothing in use.
    unsafe { mi_collect(true) };
    if stats {
        eprintln!("zeron-zgx: heap: after collect");
        // SAFETY: as above.
        unsafe { mi_stats_print(std::ptr::null_mut()) };
    }
}
