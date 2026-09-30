//! Live engine handoff — replace this engine process with a newer build
//! without disturbing what it is running.
//!
//! The engine hands itself to its successor with `execve`, which keeps the
//! PID: terminals and agents stay children of the same process, and only file
//! descriptors survive. See `docs/live-update.md`.
//!
//! - [`Manifest`] — versioned JSON in an unlinked anonymous file (its fd number
//!   travels in `ZERON_HANDOFF_FD`) describing everything the successor
//!   inherits: the IPC listener, the instance lock, terminals and runs.
//! - [`EngineCore::handoff`] — the old image: veto check, preflight, freeze
//!   (reversible), write the manifest, `execve`.
//! - [`Adoption`] / `Engine::run_adopting` — the new image: read the
//!   manifest, adopt the lock, listener, terminals and runs, then commit. A
//!   failure before commit re-execs the old binary, which adopts the same
//!   manifest, so a rollback is just another adopt boot.
//!
//! Unix only (Linux, macOS). Windows keeps its restart-based update.

mod coordinator;
mod fds;
mod manifest;

pub use coordinator::{
    ANY_TARGET_ENV, HandoffError, SuccessorCaps, check_target, exec_into, preflight,
    preflight_report, prune_rollback_copies, roll_back, rollback_exe,
};
pub use fds::{adopt_fd, is_cloexec, listener_from_inherited, set_inheritable};
pub use manifest::{
    Adoption, HANDOFF_ATTEMPT_ENV, HANDOFF_FD_ENV, HANDOFF_ROLLED_BACK_ENV, MANIFEST_VERSION,
    Manifest, ManifestError, PendingInputRecord, RunHandoff, read_adoption_from_env,
};
