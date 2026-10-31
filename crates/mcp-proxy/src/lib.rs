//! One stdio MCP server per proxy. No engine, UI, or MCP bridge dependencies.
pub mod cache;
pub mod config;
pub mod discovery;
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
    run_config(config)
}

/// Shared pre-protocol entry for Claude interception, Codex overrides and CLI.
/// Known channels retain the original stdio/env without even starting Tokio.
pub fn run_config(mut config: Config) -> io::Result<()> {
    let eager = if config.selftest {
        false
    } else {
        match cache::launch_key(&config) {
            Ok(key) => {
                let outcome = discovery::discovery(&config.data_dir, &key);
                config.launch_identity = Some(key);
                if let Ok(Some(class)) = &outcome {
                    let _ = discovery::record_run_discovery(&config, *class);
                }
                match outcome {
                    Ok(Some(discovery::Discovery::Channel | discovery::Discovery::Failed))
                    | Err(_) => true,
                    _ => false,
                }
            }
            Err(_) => true,
        }
    };
    if eager {
        // This exec bypasses the proxy loop. Preserve a run-local hold for
        // channels and uncertain legacy fallbacks before handing off stdio.
        if let Some(dir) = &config.run_dir {
            let _ = std::fs::create_dir_all(dir).and_then(|()| {
                cache::atomic_write(&dir.join("channel-live"), b"channel-or-unknown")
            });
        }
        #[cfg(unix)]
        return exec_real(&config);
        #[cfg(not(unix))]
        {
            config.mode = Mode::KeepAlive;
        }
    }
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        #[cfg(unix)]
        Err(_) => return exec_real(&config),
        #[cfg(not(unix))]
        Err(e) => return Err(e),
    };
    let result = runtime.block_on(proxy::run(config));
    // Tokio's stdin thread may be blocked in read after a signal; do not wait for it.
    runtime.shutdown_timeout(Duration::from_millis(100));
    result
}

#[cfg(unix)]
fn exec_real(config: &Config) -> io::Result<()> {
    use std::os::unix::process::CommandExt;
    let mut command = std::process::Command::new(&config.command);
    command.args(&config.args);
    if let Some(arg0) = &config.child_argv0 {
        command.arg0(arg0);
    }
    if let Some(cwd) = &config.cwd {
        command.current_dir(cwd);
    }
    Err(command.exec())
}
