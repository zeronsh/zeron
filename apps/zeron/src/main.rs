//! Crew — headed by default; `comet headless` runs the engine alone.
//! Runtime coordinates and data are isolated from installed Crew clients.

#![cfg_attr(windows, windows_subsystem = "windows")]

mod auth_cli;
mod crew_cli;
mod daemon;
mod environment;
mod paths;
mod update_cli;
mod session_cli;
mod remote_backup_cli;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "comet",
    version,
    about = "Multi-device controller for coding agents"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    /// Open a Crew conversation URL.
    #[arg(value_name = "URL")]
    open_url: Option<String>,
    #[command(flatten)]
    runtime: crew_cli::RuntimeArgs,
    #[cfg(windows)]
    #[arg(long, hide = true)]
    wait_for_exit: Option<u32>,
}

#[derive(Subcommand)]
enum Command {
    /// Run the engine without a UI (local-only unless a saved session enables sync).
    Headless(crew_cli::HeadlessArgs),
    /// The packaged desktop's background engine. It keeps sessions alive
    /// after the Crew window closes.
    #[command(hide = true)]
    Engine,
    /// Sign in and enable sync on the next engine start.
    Login,
    /// Remove the saved session and return to local-only on the next start.
    Logout,
    /// Show workspace mode, optional auth, and engine status.
    Status,
    #[command(hide = true)]
    ScaffoldAuthority,
    /// Coordinate native Crew sessions through the connected engine.
    Session {
        #[command(subcommand)]
        command: session_cli::SessionCommand,
    },
    /// Control Scaffold using durable immutable operation IDs.
    Scaffold {
        #[command(subcommand)]
        command: session_cli::ScaffoldCommand,
    },
    /// Explicitly import a stopped-writer backup into a NEW separate profile.
    ImportBackup {
        #[arg(long)]
        backup: std::path::PathBuf,
        #[arg(long)]
        destination: std::path::PathBuf,
        /// Perform the same complete validation and report without writing a profile.
        #[arg(long)]
        validate_only: bool,
        /// Read back an existing imported destination against the immutable source.
        #[arg(long, conflicts_with = "validate_only")]
        verify_only: bool,
        /// Read-only current controller authentication; do not connect to an engine.
        #[arg(long, requires_all = ["source_context", "device_id"])]
        identity_session: Option<std::path::PathBuf>,
        #[arg(long, requires = "identity_session")]
        source_context: Option<std::path::PathBuf>,
        /// Preserved owner controller device ID for this destination.
        #[arg(long, requires = "identity_session")]
        device_id: Option<String>,
    },
    /// Import another authority's native profile (a stopped-writer copy) into the
    /// running signed-in engine, e.g. Crew Canary history into Crew Staging.
    ImportNativeProfile(session_cli::ImportNativeProfileArgs),
    /// Build an importer manifest from a consistent stopped-writer profile copy.
    ExportBackup(session_cli::ExportBackupArgs),
    /// Inspect immutable copied bootstrap evidence without starting a runtime.
    InspectBackup(session_cli::InspectBackupArgs),
    /// Export a fenced remote profile copy using provider-supplied exact context.
    ExportRemoteBackup(remote_backup_cli::Args),
    /// Import a fenced remote backup offline; never starts a runtime.
    ImportRemoteBackup(remote_backup_cli::ImportArgs),
    /// Read back a remote import against its immutable source offline.
    VerifyRemoteBackup(remote_backup_cli::ImportArgs),
    /// Live sync introspection from the running engine: per-room connection
    /// state, last pushed-frame/ack ages, rejoin/probe/resync counters.
    Sync,
    #[cfg(target_os = "linux")]
    /// Trigger an Appshot in the running headed instance (desktop shortcut fallback).
    Appshot,
    /// Serve the Crew MCP (Model Context Protocol) server on stdin/stdout,
    /// proxying to the running engine's IPC. Agents use it to create, read,
    /// and message chats. Logs go to stderr; stdout is the protocol.
    Mcp,
    /// Manage the build-selected Crew engine as a background service.
    Daemon {
        #[command(subcommand)]
        command: DaemonCommand,
    },
    /// Check for a newer release and apply it (download → verify → swap →
    /// service restart). `--check` only reports (exits 1 when one is available).
    Update {
        #[arg(long)]
        check: bool,
    },
}

#[derive(Subcommand)]
enum DaemonCommand {
    /// Install, enable, and start the isolated service.
    Install,
    /// Stop and remove the service.
    Uninstall,
    /// Start the installed service.
    Start,
    /// Stop the service.
    Stop,
    /// Restart the service.
    Restart,
    /// Show the service manager's view of the daemon.
    Status,
}


/// glibc gives threads their own arenas (up to 8 × cores) and on free returns
/// only the top of each heap, so churn freed mid-heap stays resident: a
/// headless engine held 6.6GB in 129 arenas after a day. `malloc_trim` also
/// releases the free pages inside every arena, so long-running modes call it
/// once a minute. Capping arenas instead (`M_ARENA_MAX=2`) reclaimed less and
/// cost ~6x the engine's CPU in arena-lock contention while chats streamed
/// (docs/memory-plan.md).
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn spawn_malloc_trimmer() {
    const PERIOD: std::time::Duration = std::time::Duration::from_secs(60);
    let spawned = std::thread::Builder::new()
        .name("malloc-trim".into())
        .spawn(|| {
            loop {
                std::thread::sleep(PERIOD);
                let started = std::time::Instant::now();
                // SAFETY: malloc_trim takes no pointers and locks each arena
                // itself, so it is safe to call from any thread at any time.
                let released = unsafe { libc::malloc_trim(0) } != 0;
                tracing::debug!(released, elapsed = ?started.elapsed(), "malloc_trim");
            }
        });
    if let Err(error) = spawned {
        tracing::warn!(%error, "malloc trimmer not started");
    }
}

fn main() -> anyhow::Result<()> {
    if let Some(result) = zeron_harness::omp::run_writer_probe_from_env() {
        return result.map_err(anyhow::Error::from);
    }
    if let Some(result) = zeron_harness::omp::run_supervisor_from_env() {
        return result.map_err(anyhow::Error::from);
    }
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("--noop-browser")) {
        return Ok(());
    }
    #[cfg(windows)]
    attach_parent_console();
    let mut cli = Cli::parse();
    if matches!(cli.command, Some(Command::Engine)) {
        let config = cli.runtime.config(false, true, false)?;
        return zeron_app::background::run_with_config(config);
    }
    // Provider migration must never initialize auth, paths, locks or a runtime
    // against its old profile. Context files are bindings, not cloud authority.
    cli.command = match cli.command {
        Some(Command::ExportRemoteBackup(args)) => return remote_backup_cli::run(args),
        Some(Command::ImportRemoteBackup(args)) => return remote_backup_cli::import(args),
        Some(Command::VerifyRemoteBackup(args)) => return remote_backup_cli::verify(args),
        Some(Command::InspectBackup(args)) => return session_cli::inspect(args),
        Some(Command::ExportBackup(args)) if args.offline() => {
            return tokio::runtime::Runtime::new()?.block_on(session_cli::export(args, 0, None));
        }
        Some(Command::ImportBackup { backup, destination, validate_only, verify_only,
            identity_session: Some(identity), source_context: Some(context), device_id: Some(device) }) => {
            return tokio::runtime::Runtime::new()?.block_on(session_cli::import_offline(
                backup, destination, validate_only, verify_only, identity, context, device));
        }
        command => command,
    };
    // IPC clients may inherit another engine's port without changing future app launches.
    let persist_runtime = matches!(&cli.command,
        None | Some(Command::Headless(_) | Command::Login | Command::Logout)
        | Some(Command::Daemon { command: DaemonCommand::Install | DaemonCommand::Start | DaemonCommand::Restart }));
    let own_runtime = persist_runtime || matches!(&cli.command,
        Some(Command::Status | Command::Daemon { .. }));
    let mut config = cli.runtime.config(persist_runtime, own_runtime, cli.command.is_none())?;
    #[cfg(windows)]
    if let Some(pid) = cli.wait_for_exit {
        zeron_update::windows::wait_for_exit(pid)?;
    } else if matches!(&cli.command, None | Some(Command::Headless(_))) {
        zeron_update::windows::cleanup_previous_image();
    }
    // Long-running modes log at info, one-shot CLI commands at warn (RUST_LOG
    // overrides either).
    // loro's internal block-encode diagnostics log at info and flood
    // journald on every snapshot export — enough to fill a disk on a
    // long-running headless host. Quiet them by default (RUST_LOG still
    // overrides the whole filter).
    let long_running = matches!(&cli.command, None | Some(Command::Headless(_)));
    let default_filter = if long_running {
        "info,loro_internal=warn,loro=warn"
    } else {
        "warn"
    };
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| default_filter.into());
    // Long-running modes mirror stdout logging to {data_dir}/logs — a headed
    // app launched from Finder has no visible stdout, which left every sync
    // wedge report ("stale until restart") with zero diagnostics even though
    // the engine logs the exact failure line. One file per launch, previous
    // launch kept as `.old`.
    let log_file = if long_running {
        let mode = if cli.command.is_some() {
            "headless"
        } else {
            "headed"
        };
        open_log_file(&config.data_dir, mode)
    } else {
        None
    };
    {
        use tracing_subscriber::layer::SubscriberExt;
        use tracing_subscriber::util::SubscriberInitExt;
        // `zeron mcp` owns stdout for the protocol: a single log line on it
        // would corrupt the JSON-RPC stream, so its diagnostics go to stderr.
        if !long_running {
            tracing_subscriber::registry()
                .with(filter)
                .with(
                    tracing_subscriber::fmt::layer()
                        .with_ansi(false)
                        .with_writer(std::io::stderr),
                )
                .init();
        } else {
            let registry = tracing_subscriber::registry()
                .with(filter)
                .with(tracing_subscriber::fmt::layer());
            match log_file {
                Some(file) => registry
                    .with(
                        tracing_subscriber::fmt::layer()
                            .with_ansi(false)
                            .with_writer(std::sync::Arc::new(file)),
                    )
                    .init(),
                None => registry.init(),
            }
        }
    }

    if long_running {
        // Finder launches have no visible stderr. Mirror the panic location
        // and backtrace into the same rotating log as engine diagnostics.
        let default_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            tracing::error!(panic = %info,
                backtrace = %std::backtrace::Backtrace::force_capture(),
                "application panic");
            default_hook(info);
        }));
        #[cfg(all(target_os = "linux", target_env = "gnu"))]
        spawn_malloc_trimmer();
    }

    match cli.command {
        Some(Command::Headless(args)) => {
            if let Some(path) = args.device_bootstrap_file {
                crew_cli::configure_device(&mut config, &path)?;
            }
            let runtime = tokio::runtime::Runtime::new()?;
            runtime.block_on(zeron_engine::Engine::new(config).run())
        }
        Some(Command::Login) => {
            let runtime = tokio::runtime::Runtime::new()?;
            runtime.block_on(auth_cli::login(config))
        }
        Some(Command::Logout) => {
            let runtime = tokio::runtime::Runtime::new()?;
            runtime.block_on(auth_cli::logout(config))
        }
        Some(Command::Status) => {
            let runtime = tokio::runtime::Runtime::new()?;
            runtime.block_on(auth_cli::status(config))
        }
        Some(Command::ScaffoldAuthority) => {
            let runtime = tokio::runtime::Runtime::new()?;
            runtime.block_on(crew_cli::scaffold_authority(
                config.ipc_port,
                &config.data_dir,
            ))
        }
        Some(Command::Session { command }) => {
            tokio::runtime::Runtime::new()?.block_on(session_cli::run(command, config.ipc_port, &config.data_dir))
        }
        Some(Command::Scaffold { command }) => {
            tokio::runtime::Runtime::new()?.block_on(session_cli::scaffold(command, config.ipc_port, &config.data_dir))
        }
        Some(Command::ImportBackup { backup, destination, validate_only, verify_only, .. }) => {
            tokio::runtime::Runtime::new()?.block_on(session_cli::import(
                backup, destination, validate_only, verify_only, config.ipc_port, &config.data_dir,
            ))
        }
        Some(Command::ExportBackup(args)) => {
            tokio::runtime::Runtime::new()?.block_on(session_cli::export(args, config.ipc_port, Some(&config.data_dir)))
        }
        Some(Command::ImportNativeProfile(args)) => {
            tokio::runtime::Runtime::new()?.block_on(session_cli::import_native_profile(args, config.ipc_port, &config.data_dir))
        }
        Some(Command::ExportRemoteBackup(_) | Command::ImportRemoteBackup(_) | Command::VerifyRemoteBackup(_) | Command::InspectBackup(_)) => unreachable!("offline migration dispatched before runtime configuration"),
        Some(Command::Engine) => unreachable!("background engine dispatched before runtime configuration"),
        Some(Command::Sync) => {
            let runtime = tokio::runtime::Runtime::new()?;
            runtime.block_on(sync_cli(config.ipc_port, &config.data_dir))
        }
        Some(Command::Mcp) => {
            // One bridge per agent session, and it only relays: stdio
            // requests out to the engine over IPC and replies back. Two
            // workers keep a burst of parallel tool calls (large replies to
            // encode) close to per-core speed without a worker per core in
            // every agent's bridge.
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()?;
            let mut mcp = zeron_mcp::McpConfig::from_env();
            mcp.ipc_port = config.ipc_port;
            mcp.data_dir = Some(config.data_dir.clone());
            if let Ok(id) = std::env::var("COMET_SESSION_ID") { mcp.origin.chat_id = Some(id); }
            if let Ok(id) = std::env::var("COMET_DEVICE_ID") { mcp.origin.device_id = Some(id); }
            runtime.block_on(zeron_mcp::run(mcp))
        }
        #[cfg(target_os = "linux")]
        Some(Command::Appshot) => {
            zeron_ui::appshots::request_running_appshot(&config.data_dir)
                .map_err(anyhow::Error::msg)
        }
        Some(Command::Update { check }) => {
            let runtime = tokio::runtime::Runtime::new()?;
            runtime.block_on(update_cli::update(&config, check))
        }
        Some(Command::Daemon { command }) => match command {
            DaemonCommand::Install => daemon::install(&config),
            DaemonCommand::Uninstall => daemon::uninstall(),
            DaemonCommand::Start => daemon::start(),
            DaemonCommand::Stop => daemon::stop(),
            DaemonCommand::Restart => daemon::restart(),
            DaemonCommand::Status => daemon::status(),
        },
        None => {
            zeron_ui::run_app(zeron_ui::UiConfig {
                data_dir: config.data_dir,
                ipc_port: config.ipc_port,
                edge_url: config.edge_url,
                workos_client_id: config.workos_client_id,
                edge_token: config.edge_token,
                org_id: config.org_id,
                default_harness: config.default_harness,
                initial_url: cli.open_url,
                scaffold: config.scaffold,
                managed_inference: config.managed_inference,
            });
            Ok(())
        }
    }
}

#[cfg(windows)]
fn attach_parent_console() {
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Console::{
        ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE,
        STD_OUTPUT_HANDLE, SetStdHandle,
    };

    // The GUI subsystem prevents Explorer from creating a console at startup.
    // Reuse an existing parent's console for CLI output and cargo run, without
    // allocating one. Attach before Clap so help and argument errors work too.
    // Preserve redirected pipes/files: attaching may replace standard handles.
    unsafe {
        let saved = [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE]
            .map(|id| (id, GetStdHandle(id)));
        if AttachConsole(ATTACH_PARENT_PROCESS) != 0 {
            for (id, handle) in saved {
                if !handle.is_null() && handle != INVALID_HANDLE_VALUE {
                    SetStdHandle(id, handle);
                }
            }
        }
    }
}


/// `zeron sync`: dial the running engine's IPC and print per-room sync state.
/// The introspection surface every 2026-08 incident was missing — "is this
/// device's workspace room actually receiving?" as a one-liner.
async fn sync_cli(ipc_port: u16, data_dir: &std::path::Path) -> anyhow::Result<()> {
    let client = zeron_rpc::connect_ipc(ipc_port, Some(data_dir))
        .await
        .map_err(|e| {
            anyhow::anyhow!("no engine listening on 127.0.0.1:{ipc_port} ({e}) — is Crew running?")
        })?;
    let status = client
        .call(zeron_rpc::methods::SYNC_STATUS, serde_json::json!({}))
        .await
        .map_err(|e| anyhow::anyhow!("SyncStatus failed: {e}"))?;
    let now = status.get("nowMs").and_then(|v| v.as_i64()).unwrap_or(0);
    let age = |ms: i64| -> String {
        if ms <= 0 {
            return "never".into();
        }
        let s = (now - ms).max(0) / 1000;
        if s >= 3600 {
            format!("{}h{}m ago", s / 3600, (s % 3600) / 60)
        } else if s >= 60 {
            format!("{}m{}s ago", s / 60, s % 60)
        } else {
            format!("{s}s ago")
        }
    };
    let room_line = |room: Option<&serde_json::Value>| -> String {
        let Some(room) = room else {
            return "no room (dialing or edge-less)".into();
        };
        let get = |k: &str| room.get(k).and_then(|v| v.as_i64()).unwrap_or(0);
        // REJECTED is loud and only shown when nonzero: rejected writes with
        // a fresh-looking room is exactly the latched-session wedge
        // (2026-08-04) this readout previously masked.
        let rejected = get("rejected");
        format!(
            "{} pushed {} · acked {} · rejoins {} probes {} resyncs {} drops {}{}",
            if room.get("connected").and_then(|v| v.as_bool()) == Some(true) {
                "connected ·"
            } else {
                "DISCONNECTED ·"
            },
            age(get("lastPushedMs")),
            age(get("lastAckMs")),
            get("rejoins"),
            get("probes"),
            get("fullResyncs"),
            get("disconnects"),
            if rejected > 0 {
                format!(" REJECTED {rejected}")
            } else {
                String::new()
            },
        )
    };
    println!(
        "Device:    {}",
        status
            .get("deviceId")
            .and_then(|v| v.as_str())
            .unwrap_or("?")
    );
    println!(
        "Workspace: {}",
        room_line(status.get("workspace").filter(|v| !v.is_null()))
    );
    let chats = status
        .get("chats")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    if chats.is_empty() {
        println!("Chats:     none open");
    }
    // Chat rooms speak chat2: cursor/head tell "am I caught up?", pending
    // tells "did my writes leave?", resets/rejected are the loud tells.
    let chat_line = |room: Option<&serde_json::Value>| -> String {
        let Some(room) = room else {
            return "no room (dialing or edge-less)".into();
        };
        let get = |k: &str| room.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
        let resets = get("serverResets");
        let rejected = get("rejected");
        format!(
            "{} cursor {}/{} · pending {} · rows {} ({}KB) · rejoins {} drops {}{}{}",
            if room.get("connected").and_then(|v| v.as_bool()) == Some(true) {
                "connected ·"
            } else {
                "DISCONNECTED ·"
            },
            get("cursor"),
            get("headSeq"),
            get("pendingPushes"),
            get("rowCount"),
            get("rowBytes") / 1024,
            get("rejoins"),
            get("disconnects"),
            if resets > 0 {
                format!(" RESETS {resets}")
            } else {
                String::new()
            },
            if rejected > 0 {
                format!(" REJECTED {rejected}")
            } else {
                String::new()
            },
        )
    };
    for chat in &chats {
        println!(
            "Chat {}: {}",
            chat.get("chatId")
                .and_then(|v| v.as_str())
                .map(|s| &s[..s.len().min(8)])
                .unwrap_or("?"),
            chat_line(chat.get("room").filter(|v| !v.is_null()))
        );
    }
    Ok(())
}

/// `{data_dir}/logs/zeron-{mode}.log`, previous launch preserved as `.old`.
/// Headed and headless are separate files so an embedded-engine app and a
/// daemon on the same machine never interleave writes.
///
/// The returned file holds an exclusive `flock` for the process lifetime:
/// rotate-on-launch is only safe when nothing is still WRITING the current
/// file. On 2026-08-04 a dev build launched twice next to the running
/// installed app — the first rename put the daemon's live log at `.old`, the
/// second unlinked it entirely, and the daemon spent the rest of the incident
/// logging to an orphaned inode (an entire day of sync diagnostics gone at
/// the exact moment they were needed). A launch that finds the canonical file
/// locked logs to `zeron-{mode}.{pid}.log` instead; the next lock-holding
/// launch sweeps pid-suffixed files older than a week.
fn open_log_file(data_dir: &std::path::Path, mode: &str) -> Option<std::fs::File> {
    open_log_file_in(&data_dir.join("logs"), mode)
}

/// Dir-parameterized body of [`open_log_file`] (unit-testable without env).
fn open_log_file_in(dir: &std::path::Path, mode: &str) -> Option<std::fs::File> {
    std::fs::create_dir_all(dir).ok()?;
    let path = dir.join(format!("zeron-{mode}.log"));
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        // Probe the CURRENT inode for a live writer before touching it.
        let preexisting = path.exists();
        let existing = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .ok()?;
        let rc = unsafe { libc::flock(existing.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if rc != 0 {
            // A live process owns the canonical log — leave it alone.
            return std::fs::File::create(
                dir.join(format!("zeron-{mode}.{}.log", std::process::id())),
            )
            .ok();
        }
        // No live writer: rotate, create fresh, and lock it as ours. (The
        // probe's flock dies with `existing`; a first-ever launch has nothing
        // to rotate — the probe itself created the empty file.)
        drop(existing);
        if preexisting {
            let _ = std::fs::rename(&path, dir.join(format!("zeron-{mode}.log.old")));
        }
        let file = std::fs::File::create(&path).ok()?;
        unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        sweep_stale_pid_logs(dir, mode);
        Some(file)
    }
    #[cfg(not(unix))]
    {
        let _ = std::fs::rename(&path, dir.join(format!("zeron-{mode}.log.old")));
        std::fs::File::create(&path).ok()
    }
}

#[cfg(all(test, unix))]
mod log_file_tests {
    use super::open_log_file_in;

    #[test]
    fn second_launch_never_rotates_a_live_processes_log() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path();
        // First launch owns the canonical file and keeps writing.
        let first = open_log_file_in(dir, "headed").expect("first log");
        assert!(dir.join("zeron-headed.log").is_file());
        // Second launch while the first is alive: canonical file untouched,
        // pid-suffixed overflow file instead (the 2026-08-04 clobber).
        let second = open_log_file_in(dir, "headed").expect("second log");
        let pid_path = dir.join(format!("zeron-headed.{}.log", std::process::id()));
        assert!(pid_path.is_file(), "expected pid-suffixed overflow log");
        assert!(
            !dir.join("zeron-headed.log.old").exists(),
            "live canonical log must not be rotated away"
        );
        drop(second);
        // After the owner exits, a fresh launch rotates normally.
        drop(first);
        let third = open_log_file_in(dir, "headed").expect("third log");
        assert!(
            dir.join("zeron-headed.log.old").is_file(),
            "rotation resumes"
        );
        drop(third);
    }
}

/// Delete `zeron-{mode}.{pid}.log` overflow files older than a week — they
/// only exist when a second instance raced a live one for the canonical log.
#[cfg(unix)]
fn sweep_stale_pid_logs(dir: &std::path::Path, mode: &str) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let prefix = format!("zeron-{mode}.");
    let week = std::time::Duration::from_secs(7 * 24 * 60 * 60);
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(middle) = name
            .strip_prefix(&prefix)
            .and_then(|rest| rest.strip_suffix(".log"))
        else {
            continue;
        };
        if !middle.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > week);
        if stale {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod migration_cli_tests {
    use super::*;

    #[test]
    fn migration_commands_require_explicit_source_destination_and_writer_confirmation() {
        let args = ["comet", "export-backup", "--source-profile-root", "/original",
            "--source-backup-root", "/copy", "--database-relative", "projects/project/account/docs.sqlite3",
            "--destination", "/export"];
        assert!(Cli::try_parse_from(args).is_err());
        assert!(matches!(Cli::try_parse_from(args.into_iter().chain(["--writers-stopped"])).unwrap().command,
            Some(Command::ExportBackup(_))));
        assert!(matches!(Cli::try_parse_from(["comet", "import-backup", "--backup", "/export",
            "--destination", "/native", "--validate-only"]).unwrap().command,
            Some(Command::ImportBackup { validate_only: true, .. })));
        assert!(Cli::try_parse_from(args.into_iter().chain(["--writers-stopped", "--identity-session", "/existing-identity"])).is_err());
        let offline = Cli::try_parse_from(args.into_iter().chain(["--writers-stopped", "--identity-session", "/existing-identity",
            "--source-context", "/context.json", "--source-storage-directory", "/source-storage",
            "--terminal-authority-root", "/terminal-authority", "--inventory-attachments-only"])).unwrap();
        assert!(matches!(offline.command, Some(Command::ExportBackup(args)) if args.offline()));
    }
}
