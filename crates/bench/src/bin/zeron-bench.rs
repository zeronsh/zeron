//! `zeron-bench` — isolated engine launcher.
//!
//! `launch` builds a trial under `ZB_ROOT` (or `--zb-root`) and starts the
//! engine with [`zeron_bench::engine_env`], which does not inherit this
//! process's environment. `doctor` reports whether a user scope, smaps, perf,
//! and RAPL are usable, and whether a live engine was detected (read-only).
//!
//! `exec-counted` is the argv shape the scope uses to start the engine. It is
//! a passthrough until the perf unit wraps the command: it replaces this
//! process with the given command and does not touch the environment.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "zeron-bench", about = "Isolated launcher for the Zeron engine")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Start an engine in an isolated trial, hold it, then stop it.
    Launch {
        /// Engine executable (`zeron-engine`).
        #[arg(long)]
        engine: PathBuf,
        /// Seconds to leave the engine running after it is ready. Zero is allowed.
        #[arg(long)]
        hold: u64,
        /// Bench root. Defaults to `ZB_ROOT` or `~/Documents/dev/tmp/zeron-perf`.
        #[arg(long)]
        zb_root: Option<PathBuf>,
    },
    /// Report scope, smaps, perf, RAPL, and the live engine.
    Doctor,
    /// Replace this process with `command`. Does not change the environment.
    ExecCounted {
        /// Command and arguments. A leading `--` is ignored.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 1..)]
        command: Vec<OsString>,
    },
}

fn main() -> ExitCode {
    #[cfg(not(unix))]
    {
        eprintln!("zeron-bench only runs on unix");
        return ExitCode::from(1);
    }
    #[cfg(unix)]
    {
        let cli = Cli::parse();
        // `exec-counted` must run before a runtime exists: it replaces the process.
        match cli.command {
            Command::ExecCounted { command } => match exec_counted(command) {
                Ok(()) => ExitCode::SUCCESS,
                Err(err) => {
                    eprintln!("zeron-bench: {err:#}");
                    ExitCode::from(127)
                }
            },
            other => match run(other) {
                Ok(()) => ExitCode::SUCCESS,
                Err(err) => {
                    eprintln!("zeron-bench: {err:#}");
                    ExitCode::from(1)
                }
            },
        }
    }
}

#[cfg(unix)]
fn run(command: Command) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("start the bench runtime")?;
    runtime.block_on(async_main(command))
}

#[cfg(unix)]
async fn async_main(command: Command) -> Result<()> {
    match command {
        Command::Launch {
            engine,
            hold,
            zb_root,
        } => launch_hold(engine, hold, zb_root).await,
        Command::Doctor => {
            print!("{}", zeron_bench::doctor());
            Ok(())
        }
        Command::ExecCounted { .. } => unreachable!("exec-counted is handled before the runtime"),
    }
}

#[cfg(unix)]
async fn launch_hold(engine: PathBuf, hold: u64, zb_root: Option<PathBuf>) -> Result<()> {
    let zb_root = match zb_root {
        Some(path) if path.is_absolute() => path,
        Some(path) => std::env::current_dir()
            .context("current directory")?
            .join(path),
        None => zeron_bench::default_zb_root(),
    };
    std::fs::create_dir_all(&zb_root)
        .with_context(|| format!("create bench root {}", zb_root.display()))?;
    let mut trial = zeron_bench::Trial::create(&zb_root, "launch")?;
    trial.bench_bin = std::env::current_exe().context("resolve zeron-bench")?;
    trial.allocate_port()?;
    let port = trial.port;
    let launched = zeron_bench::launch(&engine, &trial).await?;
    let scope = launched.scope.clone().unwrap_or_else(|| "-".into());
    println!(
        "event=spawn pid={} scope={scope} port={port} containment={} t_spawn={} t_listen={} t_ready={}",
        launched.pid,
        launched.containment(),
        launched.t_spawn,
        launched.t_listen,
        launched.t_ready,
    );
    if hold > 0 {
        tokio::time::sleep(std::time::Duration::from_secs(hold)).await;
    }
    let report = launched.stop().await?;
    println!(
        "event=stop containment={} shutdown_ms={} stop_engine_answered={} how={:?}",
        report.containment,
        report.shutdown.as_millis(),
        report.stop_engine_answered,
        report.how,
    );
    Ok(())
}

#[cfg(unix)]
fn exec_counted(mut command: Vec<OsString>) -> Result<()> {
    use std::os::unix::process::CommandExt;
    if command.first().is_some_and(|arg| arg == "--") {
        command.remove(0);
    }
    let Some(program) = command.first().cloned() else {
        bail!("exec-counted needs a command");
    };
    command.remove(0);
    // `exec` returns only on failure. The environment is whatever the scope
    // (`env -i`) or the process-group spawn already set.
    let err = std::process::Command::new(program).args(&command).exec();
    Err(err).context("exec-counted")
}
