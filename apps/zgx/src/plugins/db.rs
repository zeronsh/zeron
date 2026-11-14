//! Each plugin's database (`zeron.db`), as its windows reach it: the file
//! its service (the engine) opens too, `<data>/plugins/.storage/<id>.sqlite3`.
//! What it may do and how two processes share it is `zeron-plugin-db`'s.

use std::path::PathBuf;

pub use zeron_plugin_db::{call, path_in};

/// Where plugin `id`'s database is (none without a data dir).
pub fn path(id: &str) -> Option<PathBuf> {
    path_in(&crate::engine::data_dir()?.join("plugins").join(".storage"), id)
}
