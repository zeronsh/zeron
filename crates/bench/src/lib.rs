//! Isolated launcher for profiling the Zeron engine.
//!
//! The bench never inherits the caller's environment and never uses the
//! user's data dir or IPC port. Later units (sampler, replay, scenarios)
//! build on the types re-exported here; their shapes are frozen for that.
//!
//! Unix only. On other targets the crate still builds, and [`launch`] is absent.

#[cfg(unix)]
mod env;
#[cfg(unix)]
mod launch;

pub mod sample;

#[cfg(unix)]
pub use env::{
    DEFAULT_IPC_PORT, RealEngine, Trial, ZB_ROOT_ENV, assert_isolated, default_zb_root,
    detect_real_engine, engine_env, mono_now, trial_ipc_token,
};
#[cfg(unix)]
pub use launch::{DoctorReport, Engine, ShutdownReport, StopHow, doctor, launch};
