//! `zeron-bench` — isolated engine launcher.
//!
//! `launch` builds a trial under `ZB_ROOT` (or `--zb-root`) and starts the
//! engine with [`zeron_bench::engine_env`], which does not inherit this
//! process's environment. `doctor` reports whether a user scope, smaps, perf,
//! and RAPL are usable, and whether a live engine was detected (read-only).
//!
//! `exec-counted` is the argv shape the scope uses to start the engine. It
//! forks, counts user-space instructions and cycles (and task-clock) for the
//! command, forwards signals, and writes `perf.json` when the trial path or
//! `ZB_PERF_JSON` says where. The command's environment is left unchanged.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
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
    /// Generate fixtures.
    Fixture {
        #[command(subcommand)]
        command: FixtureCommand,
    },
    /// Report scope, smaps, perf, RAPL, and the live engine.
    Doctor,
    /// Run `command` under user-space perf counters. Does not change its environment.
    ExecCounted {
        /// Command and arguments. A leading `--` is ignored.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 1..)]
        command: Vec<OsString>,
    },
}

#[derive(Subcommand)]
enum FixtureCommand {
    #[command(hide = true)]
    SnapshotWorker { source: PathBuf, output: PathBuf },
    Gen {
        name: String,
        #[arg(long)]
        engine: Option<PathBuf>,
        #[arg(long, default_value_t = 42)]
        seed: u64,
        #[arg(long)]
        from_real_profile: bool,
        /// Source revision of the supplied engine, when known.
        #[arg(long)]
        fixture_revision: Option<String>,
        /// Replay cadence for T fixtures (default: opus).
        #[arg(long)]
        cadence: Option<String>,
    },
    VerifyReal {
        path: PathBuf,
        #[arg(long)]
        engine: PathBuf,
    },
    Verify {
        path: PathBuf,
        #[arg(long)]
        engine: PathBuf,
    },
    Convert {
        input: PathBuf,
        #[arg(long, default_value = "opus")]
        cadence: String,
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
        // `exec-counted` forks, so it runs before a runtime exists.
        match cli.command {
            Command::ExecCounted { command } => exec_counted(command),
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
        Command::Fixture { command } => match command {
            FixtureCommand::SnapshotWorker { source, output } => {
                zeron_bench::fixture::real::copy_mounted(&source, &output)
            }
            FixtureCommand::Gen {
                name,
                engine,
                seed,
                from_real_profile,
                fixture_revision,
                cadence,
            } => {
                let path = if name == "P-real" && from_real_profile {
                    zeron_bench::fixture::real::snapshot()?
                } else {
                    anyhow::ensure!(!from_real_profile, "--from-real-profile requires P-real");
                    let engine = engine.context("--engine is required for synthetic profiles")?;
                    zeron_bench::fixture::profile::generate(
                        &name,
                        &engine,
                        seed,
                        fixture_revision.as_deref(),
                        cadence.as_deref(),
                    )
                    .await?
                };
                println!("{}", path.display());
                Ok(())
            }
            FixtureCommand::VerifyReal { path, engine } => {
                let count = zeron_bench::fixture::real::verify(&path, &engine).await?;
                println!("{count} chats");
                Ok(())
            }
            FixtureCommand::Verify { path, engine } => {
                zeron_bench::fixture::multi::verify(&path, &engine).await?;
                println!("200 chats, 20 aborted");
                Ok(())
            }
            FixtureCommand::Convert { input, cadence } => {
                let source = std::fs::read_to_string(&input)?;
                let script = zeron_bench::fixture::scripts::convert(&source, &cadence)?;
                let root = zeron_bench::fixture::safe_root()?;
                let dir = zeron_bench::fixture::fixture_dir(&root, &["fixtures", "private"])?;
                use std::io::Write;
                use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
                std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
                let path = dir.join(format!("converted-{}.zbr.jsonl", zeron_bench::mono_now()));
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(&path)?;
                file.write_all(script.as_bytes())?;
                println!("{}", path.display());
                Ok(())
            }
        },
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
fn exec_counted(mut command: Vec<OsString>) -> ExitCode {
    use std::os::unix::process::ExitStatusExt;
    if command.first().is_some_and(|arg| arg == "--") {
        command.remove(0);
    }
    if command.is_empty() {
        eprintln!("zeron-bench: exec-counted needs a command");
        return ExitCode::from(127);
    }
    let status = match zeron_bench::run_counted(&command) {
        Ok(status) => status,
        Err(err) => {
            eprintln!("zeron-bench: {err:#}");
            return ExitCode::from(127);
        }
    };
    if let Some(sig) = status.signal() {
        // SAFETY: the default action reports the same signal the command died
        // from. The perf report is already written.
        unsafe {
            libc::signal(sig, libc::SIG_DFL);
            libc::raise(sig);
        }
        return ExitCode::from(128u8.saturating_add(sig as u8));
    }
    ExitCode::from(status.code().unwrap_or(1) as u8)
}
