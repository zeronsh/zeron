//! Handing the allocator's free pages back after a large, short-lived
//! allocation (the home folder index), when the program's allocator keeps
//! them.
//!
//! The engine doesn't pick its allocator: the macOS app binaries run it on
//! mimalloc (`apps/zeron/src/allocator.rs`), and they hand in its forced
//! collect. Memory a thread allocated stays in that thread's pages; once the
//! thread has exited they're abandoned, and freeing what's in them later (an
//! index dropped on another thread) doesn't return them to the system until
//! a forced collect reclaims them. Without a trim set (tests, other
//! platforms' system allocators) this does nothing.

use std::sync::OnceLock;

static TRIM: OnceLock<fn()> = OnceLock::new();

/// The allocator's forced collect (once a process; later calls are ignored).
pub fn set_trim(trim: fn()) {
    let _ = TRIM.set(trim);
}

/// Return the allocator's free pages to the system, if it can be asked to.
pub(crate) fn trim() {
    if let Some(trim) = TRIM.get() {
        trim();
    }
}
