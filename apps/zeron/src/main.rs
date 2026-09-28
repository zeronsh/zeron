//! zeron — headed by default; `zeron headless` runs the engine alone. Both start
//! local-only without credentials. `zeron login` and `zeron logout` select the
//! profile used by the next engine start without mutating a live runtime.

#![cfg_attr(windows, windows_subsystem = "windows")]

mod auth_cli;
mod daemon;
mod paths;
mod update_cli;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "zeron",
    version,
    about = "Multi-device controller for coding agents"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    /// Open a Zeron conversation URL.
    #[arg(value_name = "URL")]
    open_url: Option<String>,
    #[cfg(windows)]
    #[arg(long, hide = true)]
    wait_for_exit: Option<u32>,
}

#[derive(Subcommand)]
enum Command {
    /// Run the engine without a UI (local-only unless a saved session enables sync).
    Headless,
    /// Sign in and enable sync on the next engine start.
    Login,
    /// Remove the saved session and return to local-only on the next start.
    Logout,
    /// Show workspace mode, optional auth, and engine status.
    Status,
    /// Live sync introspection from the running engine: per-room connection
    /// state, last pushed-frame/ack ages, rejoin/probe/resync counters.
    Sync,
    #[cfg(target_os = "linux")]
    /// Trigger an Appshot in the running headed instance (desktop shortcut fallback).
    Appshot,
    /// Serve the Zeron MCP (Model Context Protocol) server on stdin/stdout,
    /// proxying to the running engine's IPC. Agents use it to create, read,
    /// and message chats. Logs go to stderr; stdout is the protocol.
    Mcp,
    /// Work with chats from a shell or an agent session: spawn, message,
    /// wait for, read and manage chats. Logs go to stderr; stdout is the
    /// result (or one JSON document with --json).
    Chat {
        #[command(subcommand)]
        command: Box<zeron_mcp::cli::ChatCommand>,
    },
    /// List agent harnesses available on this device.
    Harness {
        #[command(subcommand)]
        command: zeron_mcp::cli::HarnessCommand,
    },
    /// List the models a harness offers on this device.
    Model {
        #[command(subcommand)]
        command: zeron_mcp::cli::ModelCommand,
    },
    /// Print the Zeron agent guide (no engine needed).
    Guide {
        /// Chapter name (see bare `zeron guide` for the list).
        chapter: Option<String>,
        /// Print one JSON document on stdout.
        #[arg(long)]
        json: bool,
    },
    /// Manage `zeron headless` as a background service (launchd / systemd --user).
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
    /// Install, enable, and start the service (captures ZERON_* env).
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

/// Production edge (Cloudflare Worker + Durable Objects on the zeron.sh zone).
/// `ZERON_EDGE_URL` overrides (local dev / self-hosting).
const DEFAULT_EDGE_URL: &str = "https://edge.zeron.sh";

/// Production WorkOS AuthKit client id — public knowledge (it appears in every
/// authorize URL), so baking it in is safe. Overridden by `ZERON_WORKOS_CLIENT_ID`;
/// set it to the empty string — or set a dev bearer via `ZERON_EDGE_TOKEN` — to
/// force dev-mode auth instead.
const DEFAULT_WORKOS_CLIENT_ID: &str = "client_01KWD0EAKZKD50YCQJNYSRE4BY";

fn edge_url_from_env() -> String {
    std::env::var("ZERON_EDGE_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_EDGE_URL.into())
}

/// WorkOS client id resolution: explicit env wins (empty string = dev mode);
/// otherwise a `ZERON_EDGE_TOKEN` dev bearer keeps dev mode (smoke tests,
/// local wrangler); otherwise the baked production client id makes optional
/// sync available while a bare start remains local-only.
fn workos_client_id_from_env(edge_token: &Option<String>) -> Option<String> {
    match std::env::var("ZERON_WORKOS_CLIENT_ID") {
        Ok(v) if v.trim().is_empty() => None,
        Ok(v) => Some(v),
        Err(_) if edge_token.is_some() => None,
        Err(_) => Some(DEFAULT_WORKOS_CLIENT_ID.into()),
    }
}

/// mimalloc, macOS only: libmalloc never returns the streaming churn's
/// high-water pages, so transient allocation became permanent RSS
/// (docs/memory-plan.md §1). Pinned to mimalloc v2 in the workspace manifest —
/// the crate's default v3 has the same pathology (churn retained as permanent
/// RSS, ~6x glibc's growth on identical workloads, no idle recovery). Linux
/// measured flat on glibc, so it keeps the system allocator.
#[cfg(target_os = "macos")]
#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() -> anyhow::Result<()> {
    #[cfg(windows)]
    attach_parent_console();
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            let _ = error.print();
            std::process::exit(usage_exit_code(
                error.exit_code(),
                invoked_command(std::env::args().skip(1)).as_deref(),
            ));
        }
    };
    #[cfg(windows)]
    if let Some(pid) = cli.wait_for_exit {
        zeron_update::windows::wait_for_exit(pid)?;
    } else if matches!(&cli.command, None | Some(Command::Headless)) {
        zeron_update::windows::cleanup_previous_image();
    }
    // Inside a chat the engine stamps ZERON_CLI at its own binary, but a tool
    // shell that rebuilds PATH from the login profile can resolve `zeron` to a
    // different install. The agent-facing commands converge on the stamped
    // binary; a failure falls through to running in-process.
    if matches!(
        &cli.command,
        Some(
            Command::Chat { .. }
                | Command::Harness { .. }
                | Command::Model { .. }
                | Command::Guide { .. }
        )
    ) {
        maybe_reexec_injected_cli();
    }
    // Long-running modes log at info, one-shot CLI commands at warn (RUST_LOG
    // overrides either).
    // loro's internal block-encode diagnostics log at info and flood
    // journald on every snapshot export — enough to fill a disk on a
    // long-running headless host. Quiet them by default (RUST_LOG still
    // overrides the whole filter).
    let long_running = matches!(&cli.command, None | Some(Command::Headless));
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
        open_log_file(mode)
    } else {
        None
    };
    {
        use tracing_subscriber::layer::SubscriberExt;
        use tracing_subscriber::util::SubscriberInitExt;
        // `zeron mcp` owns stdout for the protocol, and the `chat`/`harness`/
        // `model`/`guide` commands own it for their result document (`--json`
        // output must stay a single clean document): a single log line on it
        // would corrupt the stream, so their diagnostics go to stderr.
        if matches!(
            &cli.command,
            Some(
                Command::Mcp
                    | Command::Chat { .. }
                    | Command::Harness { .. }
                    | Command::Model { .. }
                    | Command::Guide { .. }
            )
        ) {
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
    }

    match cli.command {
        Some(Command::Headless) => {
            let runtime = tokio::runtime::Runtime::new()?;
            runtime.block_on(async {
                let engine = zeron_engine::Engine::new(engine_config_from_env());
                engine.run().await
            })
        }
        Some(Command::Login) => {
            let runtime = tokio::runtime::Runtime::new()?;
            runtime.block_on(auth_cli::login(engine_config_from_env()))
        }
        Some(Command::Logout) => {
            let runtime = tokio::runtime::Runtime::new()?;
            runtime.block_on(auth_cli::logout(engine_config_from_env()))
        }
        Some(Command::Status) => {
            let runtime = tokio::runtime::Runtime::new()?;
            runtime.block_on(auth_cli::status(engine_config_from_env()))
        }
        Some(Command::Sync) => {
            let runtime = tokio::runtime::Runtime::new()?;
            runtime.block_on(sync_cli(engine_config_from_env().ipc_port))
        }
        Some(Command::Mcp) => {
            let runtime = tokio::runtime::Runtime::new()?;
            runtime.block_on(zeron_mcp::run(zeron_mcp::McpConfig::from_env()))
        }
        Some(Command::Chat { command }) => {
            let runtime = tokio::runtime::Runtime::new()?;
            std::process::exit(
                runtime.block_on(zeron_mcp::cli::run_chat(*command, ipc_port_from_env())),
            );
        }
        Some(Command::Harness { command }) => {
            let runtime = tokio::runtime::Runtime::new()?;
            std::process::exit(
                runtime.block_on(zeron_mcp::cli::run_harness(command, ipc_port_from_env())),
            );
        }
        Some(Command::Model { command }) => {
            let runtime = tokio::runtime::Runtime::new()?;
            std::process::exit(
                runtime.block_on(zeron_mcp::cli::run_model(command, ipc_port_from_env())),
            );
        }
        Some(Command::Guide { chapter, json }) => {
            std::process::exit(zeron_mcp::cli::run_guide(chapter, json));
        }
        #[cfg(target_os = "linux")]
        Some(Command::Appshot) => {
            zeron_ui::appshots::request_running_appshot(&engine_config_from_env().data_dir)
                .map_err(anyhow::Error::msg)
        }
        Some(Command::Update { check }) => {
            let runtime = tokio::runtime::Runtime::new()?;
            runtime.block_on(update_cli::update(&edge_url_from_env(), check))
        }
        Some(Command::Daemon { command }) => match command {
            DaemonCommand::Install => daemon::install(&engine_config_from_env().data_dir),
            DaemonCommand::Uninstall => daemon::uninstall(),
            DaemonCommand::Start => daemon::start(),
            DaemonCommand::Stop => daemon::stop(),
            DaemonCommand::Restart => daemon::restart(),
            DaemonCommand::Status => daemon::status(),
        },
        None => {
            let edge_token = std::env::var("ZERON_EDGE_TOKEN").ok();
            // Headed: the UI probes ZERON_IPC_PORT and connects to a running
            // daemon, or embeds the engine in-process (ARCHITECTURE §1).
            zeron_ui::run_app(zeron_ui::UiConfig {
                data_dir: paths::data_dir(),
                ipc_port: ipc_port_from_env(),
                edge_url: edge_url_from_env(),
                workos_client_id: workos_client_id_from_env(&edge_token),
                edge_token,
                org_id: std::env::var("ZERON_ORG_ID").ok(),
                default_harness: zeron_ui::HarnessId::ClaudeCode,
                initial_url: cli.open_url,
            });
            Ok(())
        }
    }
}

/// The exit code for a clap parse failure. Help and version still exit 0.
/// Everything else is a usage error: on the agent-facing commands (`chat`,
/// `harness`, `model`, `guide`) that is 1 — those commands reserve 2 for
/// "awaiting input" — while every other command keeps clap's default 2.
fn usage_exit_code(clap_code: i32, invoked: Option<&str>) -> i32 {
    if clap_code == 0 {
        return 0;
    }
    match invoked {
        Some("chat" | "harness" | "model" | "guide") => 1,
        _ => clap_code,
    }
}

/// The invoked subcommand: the first argv token (after the binary name)
/// that is not a global flag. `--wait-for-exit <pid>` (Windows) is the only
/// global flag carrying a separate value, so its value is skipped.
fn invoked_command(args: impl Iterator<Item = String>) -> Option<String> {
    let mut skip_value = false;
    for arg in args {
        if skip_value {
            skip_value = false;
            continue;
        }
        if arg.starts_with('-') {
            if arg == "--wait-for-exit" {
                skip_value = true;
            }
            continue;
        }
        return Some(arg);
    }
    None
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

/// The env-resolved engine configuration shared by `headless`, `login`,
/// `logout`, and `status` — one resolution so the CLI auth commands always
/// operate on the exact session the daemon will load.
fn engine_config_from_env() -> zeron_engine::EngineConfig {
    // Dev-mode bearer (no WorkOS): an explicit token enables sync.
    let edge_token = std::env::var("ZERON_EDGE_TOKEN").ok();
    zeron_engine::EngineConfig {
        data_dir: paths::data_dir(),
        edge_url: edge_url_from_env(),
        ipc_port: ipc_port_from_env(),
        default_harness: harness_from_env(),
        // WorkOS mode: the signed-in session's org wins; ZERON_ORG_ID (dev
        // default "dev-org") scopes the workspace room otherwise.
        org_id: std::env::var("ZERON_ORG_ID").ok(),
        // Real auth against production by default; see
        // `workos_client_id_from_env` for the dev-mode escape hatches.
        workos_client_id: workos_client_id_from_env(&edge_token),
        edge_token,
    }
}

/// `ZERON_IPC_PORT` for the loopback engine IPC — the one resolver shared by
/// `sync`, `chat`, `harness`, `model`, the engine config and the headed app's
/// daemon probe (`guide` needs no engine).
fn ipc_port_from_env() -> u16 {
    std::env::var("ZERON_IPC_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(27654)
}

/// Which binary `zeron chat`/`harness`/`model`/`guide` should re-exec into:
/// the `ZERON_CLI` path the engine stamped, when it exists and is a different
/// binary than the running one. `ZERON_CLI_REEXEC` breaks the exec loop when
/// the injected path itself invokes `zeron` again.
fn injected_cli_target(
    injected: Option<&std::ffi::OsStr>,
    current_exe: Option<&std::path::Path>,
    reexec_guard_set: bool,
) -> Option<std::path::PathBuf> {
    if reexec_guard_set {
        return None;
    }
    let injected = injected?;
    if injected.is_empty() {
        return None;
    }
    let injected = std::path::PathBuf::from(injected);
    if !injected.is_file() {
        return None;
    }
    let canon = std::fs::canonicalize(&injected).unwrap_or_else(|_| injected.clone());
    let current = current_exe.and_then(|p| std::fs::canonicalize(p).ok());
    if current.as_deref() == Some(canon.as_path()) {
        return None;
    }
    Some(injected)
}

fn maybe_reexec_injected_cli() {
    let target = injected_cli_target(
        std::env::var_os("ZERON_CLI").as_deref(),
        std::env::current_exe().ok().as_deref(),
        std::env::var_os("ZERON_CLI_REEXEC").is_some(),
    );
    let Some(target) = target else { return };
    // Safety: this runs before the tokio runtime or any thread exists — the
    // process is single-threaded, so mutating the environment cannot race.
    // The child must not re-exec again — the guard is inherited.
    unsafe { std::env::set_var("ZERON_CLI_REEXEC", "1") };
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let argv: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
        // exec() only returns on failure — fall through to in-process.
        let _ = std::process::Command::new(&target).args(&argv).exec();
    }
    #[cfg(not(unix))]
    {
        let argv: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
        if let Ok(mut child) = std::process::Command::new(&target).args(&argv).spawn()
            && let Ok(status) = child.wait()
        {
            std::process::exit(status.code().unwrap_or(1));
        }
    }
}

/// `ZERON_HARNESS` (kebab-case id) picks the default harness for chats without a
/// config row — `mock` powers the e2e smoke; default `claude-code`.
fn harness_from_env() -> zeron_engine::HarnessId {
    match std::env::var("ZERON_HARNESS").as_deref().map(str::trim) {
        Ok("mock") => zeron_engine::HarnessId::Mock,
        Ok("codex") => zeron_engine::HarnessId::Codex,
        Ok("cursor") => zeron_engine::HarnessId::Cursor,
        Ok("devin") => zeron_engine::HarnessId::Devin,
        Ok("grok") => zeron_engine::HarnessId::Grok,
        Ok("hermes") => zeron_engine::HarnessId::Hermes,
        Ok("pi") => zeron_engine::HarnessId::Pi,
        Ok("antigravity") => zeron_engine::HarnessId::Antigravity,
        _ => zeron_engine::HarnessId::ClaudeCode,
    }
}

/// `zeron sync`: dial the running engine's IPC and print per-room sync state.
/// The introspection surface every 2026-08 incident was missing — "is this
/// device's workspace room actually receiving?" as a one-liner.
async fn sync_cli(ipc_port: u16) -> anyhow::Result<()> {
    let client = zeron_rpc::connect_ws(&format!("ws://127.0.0.1:{ipc_port}"))
        .await
        .map_err(|e| {
            anyhow::anyhow!("no engine listening on 127.0.0.1:{ipc_port} ({e}) — is zeron running?")
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
fn open_log_file(mode: &str) -> Option<std::fs::File> {
    let dir = paths::data_dir().join("logs");
    open_log_file_in(&dir, mode)
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
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("zeron-cli-test-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn injected_cli_unset_or_guarded_stays_in_process() {
        assert!(injected_cli_target(None, Some(Path::new("/bin/true")), false).is_none());
        assert!(
            injected_cli_target(
                Some(std::ffi::OsStr::new("/bin/false")),
                Some(Path::new("/bin/true")),
                true
            )
            .is_none()
        );
    }

    #[test]
    fn injected_cli_missing_file_stays_in_process() {
        let target = injected_cli_target(
            Some(std::ffi::OsStr::new("/nonexistent/zeron")),
            Some(Path::new("/bin/true")),
            false,
        );
        assert!(target.is_none());
    }

    #[test]
    fn injected_cli_same_binary_via_symlink_stays_in_process() {
        let dir = scratch("symlink");
        let real = dir.join("zeron-real");
        std::fs::write(&real, b"").unwrap();
        let link = dir.join("zeron-link");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real, &link).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(&real, &link).unwrap();
        assert!(injected_cli_target(Some(link.as_os_str()), Some(real.as_path()), false).is_none());
    }

    #[test]
    fn injected_cli_different_binary_reexecs() {
        let dir = scratch("different");
        let injected = dir.join("zeron-stamped");
        let running = dir.join("zeron-other");
        std::fs::write(&injected, b"").unwrap();
        std::fs::write(&running, b"").unwrap();
        assert_eq!(
            injected_cli_target(Some(injected.as_os_str()), Some(running.as_path()), false)
                .as_deref(),
            Some(injected.as_path())
        );
    }

    fn argv(argv: &[&str]) -> Option<String> {
        invoked_command(argv.iter().map(|s| s.to_string()))
    }

    #[test]
    fn usage_exit_code_maps_agent_commands_to_one() {
        for invoked in ["chat", "harness", "model", "guide"] {
            assert_eq!(usage_exit_code(2, Some(invoked)), 1, "{invoked}");
        }
        assert_eq!(usage_exit_code(2, Some("status")), 2);
        assert_eq!(usage_exit_code(2, Some("headless")), 2);
        assert_eq!(usage_exit_code(2, None), 2);
        // Help and version keep their exit 0 on every command.
        assert_eq!(usage_exit_code(0, Some("chat")), 0);
        assert_eq!(usage_exit_code(0, Some("status")), 0);
    }

    #[test]
    fn invoked_command_skips_global_flags() {
        assert_eq!(argv(&["chat", "spawn"]).as_deref(), Some("chat"));
        assert_eq!(
            argv(&["--wait-for-exit", "42", "chat"]).as_deref(),
            Some("chat")
        );
        assert_eq!(
            argv(&["--wait-for-exit=42", "model"]).as_deref(),
            Some("model")
        );
        assert_eq!(
            argv(&["--verbose", "guide", "chats"]).as_deref(),
            Some("guide")
        );
        assert_eq!(argv(&[]), None);
        assert_eq!(argv(&["--help"]), None);
    }
}
