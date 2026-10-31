//! One stdio MCP server per proxy. No engine, UI, or MCP bridge dependencies.
pub mod cache;
pub mod config;
pub mod npx;
pub mod process;
mod proxy;
pub mod rpc;
pub mod shim;

pub use config::{Config, Mode};
pub use process::cleanup_run_dir;
use std::{io, path::PathBuf, time::Duration};

pub fn data_dir() -> PathBuf {
    std::env::var_os("ZERON_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            #[cfg(windows)]
            {
                PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap_or_default()).join("Zeron")
            }
            #[cfg(not(windows))]
            {
                PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".zeron")
            }
        })
}
/// Keep spawn on the main thread: Linux PDEATHSIG belongs to the spawning thread.
pub fn run_cli(args: impl IntoIterator<Item = String>, data_dir: PathBuf) -> io::Result<()> {
    let config = Config::parse(args, data_dir)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let result = runtime.block_on(proxy::run(config));
    // Tokio's stdin thread may be blocked in read after a signal; do not wait for it.
    runtime.shutdown_timeout(Duration::from_millis(100));
    result
}
