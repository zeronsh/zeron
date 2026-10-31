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
#[command(
    name = "zeron-bench",
    about = "Isolated launcher and A/B harness for the Zeron engine"
)]
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
    /// Run `command` under user-space perf counters. Does not change its environment.
    ExecCounted {
        /// Command and arguments. A leading `--` is ignored.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 1..)]
        command: Vec<OsString>,
    },
    /// Build `zeron-engine` for a git revision into `bins/<sha12>/`, reusing a matching sha256.
    Build {
        /// Git revision (`upstream/main`, a branch, or a commit).
        rev: String,
        /// Bench root. Defaults to `ZB_ROOT` or `~/Documents/dev/tmp/zeron-perf`.
        #[arg(long)]
        zb_root: Option<PathBuf>,
        /// Git worktree used to resolve the revision. Defaults to the current checkout.
        #[arg(long)]
        repo: Option<PathBuf>,
    },
    /// Compare two revisions on a scenario (S-stream, S-boot, S-idle, S-switch; `S-stream:K=2,watch=one` sets options).
    Ab {
        /// Baseline revision.
        a: String,
        /// Candidate revision.
        b: String,
        /// `stream`, `boot`, `idle` or `switch` (or `S-stream`...), with optional `:key=value,...`. `S-build` is refused. `--inject` replays fake trials instead.
        #[arg(long)]
        scenario: String,
        /// `quick` (2 pairs), `standard` (5), or `full` (7).
        #[arg(long)]
        tier: String,
        /// Bench root. Defaults to `ZB_ROOT` or `~/Documents/dev/tmp/zeron-perf`.
        #[arg(long)]
        zb_root: Option<PathBuf>,
        /// Git worktree used to build revisions.
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Base seed for trial seeds.
        #[arg(long, default_value_t = 1)]
        seed: u64,
        /// JSON file of injected per-side trials.
        #[arg(long)]
        inject: Option<PathBuf>,
        /// Do not build. The revisions are labels only.
        #[arg(long)]
        no_build: bool,
        /// Sample host CPU for 5s and record it. Off by default for injected runs.
        #[arg(long)]
        preflight: bool,
    },
    /// Self-pair control: two copies of one revision, stored as the noise floor.
    Control {
        /// Revision to run against itself.
        rev: String,
        /// Scenario name. `S-build` is refused.
        #[arg(long)]
        scenario: String,
        /// `quick`, `standard`, or `full`.
        #[arg(long)]
        tier: String,
        #[arg(long)]
        zb_root: Option<PathBuf>,
        #[arg(long)]
        repo: Option<PathBuf>,
        #[arg(long, default_value_t = 1)]
        seed: u64,
        #[arg(long)]
        inject: Option<PathBuf>,
        #[arg(long)]
        no_build: bool,
        #[arg(long)]
        preflight: bool,
    },
    /// Rewrite `summary.json` and `report.md` for an existing run directory.
    Report {
        /// `runs/<id>` directory.
        run: PathBuf,
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
            Command::Build { rev, zb_root, repo } => match build_rev(&rev, zb_root, repo) {
                Ok(()) => ExitCode::SUCCESS,
                Err(err) => {
                    eprintln!("zeron-bench: {err:#}");
                    ExitCode::from(1)
                }
            },
            // The scenarios start their own runtime per trial.
            Command::Ab {
                a,
                b,
                scenario,
                tier,
                zb_root,
                repo,
                seed,
                inject,
                no_build,
                preflight,
            } => exit_of(ab_command(
                a, b, scenario, tier, zb_root, repo, seed, inject, no_build, preflight, false,
            )),
            Command::Control {
                rev,
                scenario,
                tier,
                zb_root,
                repo,
                seed,
                inject,
                no_build,
                preflight,
            } => exit_of(ab_command(
                rev.clone(),
                rev,
                scenario,
                tier,
                zb_root,
                repo,
                seed,
                inject,
                no_build,
                preflight,
                true,
            )),
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
        Command::ExecCounted { .. } | Command::Build { .. } => {
            unreachable!("handled before the runtime")
        }
        Command::Ab { .. } | Command::Control { .. } => {
            unreachable!("handled before the runtime")
        }
        Command::Report { run } => {
            let report = zeron_bench::ab::regenerate_report(&run)?;
            print!("{report}");
            Ok(())
        }
    }
}

#[cfg(unix)]
fn exit_of(result: Result<()>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("zeron-bench: {err:#}");
            ExitCode::from(1)
        }
    }
}

#[cfg(unix)]
fn build_rev(rev: &str, zb_root: Option<PathBuf>, repo: Option<PathBuf>) -> Result<()> {
    let zb_root = resolve_root(zb_root)?;
    let repo = resolve_repo(repo)?;
    zeron_bench::ab::cli_build(rev, zb_root, repo)?;
    Ok(())
}

#[cfg(unix)]
fn ab_command(
    a: String,
    b: String,
    scenario: String,
    tier: String,
    zb_root: Option<PathBuf>,
    repo: Option<PathBuf>,
    seed: u64,
    inject: Option<PathBuf>,
    no_build: bool,
    preflight: bool,
    control: bool,
) -> Result<()> {
    let tier = zeron_bench::ab::Tier::parse(&tier)?;
    let result = zeron_bench::ab::run_ab(&zeron_bench::ab::AbOptions {
        a,
        b,
        scenario,
        tier,
        zb_root: resolve_root(zb_root)?,
        repo: resolve_repo(repo)?,
        seed,
        inject,
        no_build,
        control,
        preflight,
        run_id: None,
        fingerprint: None,
        now_unix: None,
    })?;
    println!("run={}", result.run_dir.display());
    print!("{}", result.report);
    Ok(())
}

#[cfg(unix)]
fn resolve_root(zb_root: Option<PathBuf>) -> Result<PathBuf> {
    let root = match zb_root {
        Some(path) if path.is_absolute() => path,
        Some(path) => std::env::current_dir()?.join(path),
        None => zeron_bench::default_zb_root(),
    };
    std::fs::create_dir_all(&root)
        .with_context(|| format!("create bench root {}", root.display()))?;
    Ok(root)
}

#[cfg(unix)]
fn resolve_repo(repo: Option<PathBuf>) -> Result<PathBuf> {
    match repo {
        Some(path) if path.is_absolute() => Ok(path),
        Some(path) => Ok(std::env::current_dir()?.join(path)),
        None => std::env::current_dir().context("current directory"),
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
