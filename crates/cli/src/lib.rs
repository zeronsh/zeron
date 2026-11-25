//! `zeron`'s command line, for every binary that answers to it: the Zeron
//! app (`apps/zeron`), its engine helper (`zeron-engine`) and the zgx
//! interface (`apps/zgx`). One parser, one help text, one set of exit codes,
//! with no engine and no interface linked in.
//!
//! [`run`] parses this process's arguments and runs what needs neither
//! (`update`, `daemon`, `--noop-browser`, help and version). What's left is
//! the caller's:
//!   - no subcommand: open the window ([`Launch::Window`]);
//!   - a subcommand that runs the engine's code here ([`EngineCommand`]:
//!     `headless`, `engine`, `login`, `logout`, `status`, `sync`, `quiesce`,
//!     `mcp`, `plugin`, `template`). A binary that links the engine runs it
//!     itself ([`Engines::InProcess`], `zeron_app::run_engine_command`); an
//!     interface that doesn't hands the whole command line to the engine
//!     helper beside it ([`Engines::Helper`]: `Contents/MacOS/Zeron Engine`
//!     in a macOS bundle), which replaces this process and runs it there.

pub mod daemon;
mod env;
pub mod logging;
pub mod paths;
pub mod update_cli;

use clap::{CommandFactory as _, FromArgMatches as _, Parser, Subcommand};

pub use env::{
    DEFAULT_EDGE_URL, DEFAULT_IPC_PORT, DEFAULT_WORKOS_CLIENT_ID, edge_url_from_env, ipc_port_from,
    ipc_port_from_env, workos_client_id_from_env,
};
pub use logging::{RotatingLog, init_logging, long_running_setup, open_log_file};

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
    /// Open with Zeron's own interface, whatever template is in use (as
    /// holding Alt, or ⌥ on a Mac, as Zeron opens does).
    #[arg(long)]
    safe_mode: bool,
    #[cfg(windows)]
    #[arg(long, hide = true)]
    wait_for_exit: Option<u32>,
}

#[derive(Subcommand)]
enum Command {
    /// Run the engine without a UI (local-only unless a saved session enables sync).
    Headless,
    /// The desktop app's background engine (`zeron-engine` where it ships
    /// separately): started by the app, it keeps sessions running with the
    /// app closed.
    #[command(hide = true)]
    Engine,
    /// Sign in and enable sync on the next engine start.
    Login,
    /// Remove the saved session and return to local-only on the next start.
    Logout,
    /// Show workspace mode, optional auth, and engine status.
    Status,
    /// Live sync introspection from the running engine: per-room connection
    /// state, last pushed-frame/ack ages, rejoin/probe/resync counters.
    Sync,
    /// Cloud sleep handshake: ask the running engine to start no new work.
    /// Exits 0 when quiesced, 75 when busy (a turn in flight, a message
    /// waiting) — the edge then keeps the machine awake.
    #[command(hide = true)]
    Quiesce,
    #[cfg(target_os = "linux")]
    /// Trigger an Appshot in the running headed instance (desktop shortcut fallback).
    Appshot,
    /// Serve the Zeron MCP (Model Context Protocol) server on stdin/stdout,
    /// proxying to the running engine's IPC. Agents use it to create, read,
    /// and message chats. Logs go to stderr; stdout is the protocol.
    ///
    /// With a subcommand, manage the other MCP servers Zeron gives agents
    /// (Linear, Notion, your own…) through the running Zeron, as Settings →
    /// MCP does.
    Mcp {
        // (None: serve the Zeron MCP server on stdio, as agents run it.)
        #[command(subcommand)]
        command: Option<McpCommand>,
    },
    /// Make, sign, check and list plugins (through the running Zeron).
    Plugin {
        #[command(subcommand)]
        command: PluginCommand,
    },
    /// List, make, use and delete templates: whole replacements of Zeron's
    /// interface (through the running Zeron). `zeron template use zeron`
    /// puts Zeron's own back.
    Template {
        #[command(subcommand)]
        command: TemplateCommand,
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
    /// Watches a new version while it proves itself after an update: started by
    /// the old version just before it hands over.
    #[command(name = "update-watchdog", hide = true)]
    UpdateWatchdog,
    /// An agent's models, discovered now with the credentials this process's
    /// environment points it at, as JSON on stdout: what a Cloud sign-in on
    /// this computer keeps as the new account's (started by the engine).
    #[command(name = "discover-models", hide = true)]
    DiscoverModels {
        /// The agent (`antigravity`, `grok`, …).
        harness: String,
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

#[derive(Subcommand)]
pub enum PluginCommand {
    /// Make a new plugin in Zeron's plugins folder, signed as you.
    New {
        /// Its id: lowercase letters, digits and dashes.
        id: String,
        /// Its name (defaults to the id).
        #[arg(long)]
        name: Option<String>,
        /// A permission it asks for (repeat for more).
        #[arg(long = "permission", value_name = "P")]
        permissions: Vec<String>,
    },
    /// Sign a plugin as you: an id in the plugins folder, or a folder's path.
    Sign { target: String },
    /// Check a plugin folder's signature and who made it.
    Verify { path: String },
    /// List the plugins Zeron knows.
    List,
    /// Save a plugin as one signed .zeron-plugin file to share (attach it
    /// to a GitHub release, or send it): anyone installs it from that file
    /// or its link. Your own plugin is signed again first.
    Export {
        /// The plugin's id (`zeron plugin list` names them).
        id: String,
        /// Where to save it: a folder, or a file ending .zeron-plugin
        /// (defaults to Zeron's plugin-exports folder).
        #[arg(long, value_name = "PATH", value_parser = absolute_path)]
        to: Option<std::path::PathBuf>,
    },
    /// Check a plugin against the store's rules (README, listing, version,
    /// files): every problem, before anything is sent. Needs no Zeron
    /// running and no sign-in. Exits 1 when it doesn't meet them.
    Check {
        /// The plugin's folder, or the id of one in Zeron's plugins folder.
        target: String,
    },
    /// Publish a plugin to the store: it's live at once, for anyone to
    /// install, and marked verified once a reviewer has read it. In CI, set
    /// ZERON_DEPLOY_TOKEN to a deploy token (never pass it as an argument):
    /// the folder is checked, packed and sent with the token, with no Zeron
    /// running and no sign-in (the store records it as the token's, and
    /// installs it as yours). Without it, your own plugin goes through the
    /// running Zeron, signed with this device's key and sent with your
    /// session. Exits 0 when published, 1 when it doesn't meet the store's
    /// rules, 2 for a usage or set-up problem, 3 when the store refuses the
    /// token or the account, 4 when the store can't be reached (try again).
    Publish {
        /// The plugin's folder (with ZERON_DEPLOY_TOKEN), or the id of your
        /// plugin in Zeron's plugins folder.
        target: String,
        /// Check and pack it, but don't send it.
        #[arg(long)]
        dry_run: bool,
    },
    /// Your plugins in the store: each version, whether Zeron has verified
    /// it, and a reviewer's reason when it declined to. Through the running
    /// Zeron, with your session: never a deploy token's.
    Status {
        /// Only this plugin.
        id: Option<String>,
    },
    /// Deploy tokens: what a CI job publishes plugins and templates with.
    /// Each is your account's, for those you name (an id is a plugin's or a
    /// template's) or all of your plugins, fixed when it's made. Made,
    /// listed and revoked through the running Zeron, with your session
    /// (never with a deploy token); a plugin's first version comes from your
    /// machine.
    Token {
        #[command(subcommand)]
        command: TokenCommand,
    },
    /// Stop new installs of a version (installed copies keep working).
    /// Through the running Zeron, with your session: never a deploy token's.
    Yank { id: String, version: String },
    /// Undo `yank`.
    Unyank { id: String, version: String },
}

#[derive(Subcommand)]
pub enum TokenCommand {
    /// Make a deploy token: it's shown this once (neither Zeron nor the
    /// store keeps it). Name what it publishes (`--plugins a,b`, plugins and
    /// templates alike), or `--all` of your plugins, or `--all --templates`
    /// for your plugins and templates, those you first publish later
    /// included; that can't change afterwards. Making one needs a fresh
    /// sign-in, which this asks for.
    Create {
        /// What it's for, like "GitHub Actions".
        #[arg(long)]
        name: String,
        /// What it publishes, by id: plugins and templates (yours in the
        /// store already).
        #[arg(long, value_delimiter = ',', value_name = "IDS", required_unless_present = "all", conflicts_with = "all")]
        plugins: Vec<String>,
        /// Every plugin of yours, those you first publish later included.
        /// Templates too with `--templates`.
        #[arg(long)]
        all: bool,
        /// With `--all`: every template of yours as well as every plugin.
        #[arg(long, requires = "all")]
        templates: bool,
        /// Stop it working after this many days.
        #[arg(long, value_name = "DAYS")]
        expires_in_days: Option<u32>,
    },
    /// List your deploy tokens and what each publishes (never the tokens).
    List,
    /// Revoke a deploy token: it publishes nothing from now on.
    Revoke {
        /// The token's id, as `zeron plugin token list` shows it.
        id: String,
    },
}

/// A path as given, made absolute against the current folder (the engine
/// takes full paths only).
fn absolute_path(text: &str) -> Result<std::path::PathBuf, String> {
    if text.trim().is_empty() {
        return Err("give a path".into());
    }
    std::path::absolute(text).map_err(|e| e.to_string())
}

#[derive(Subcommand)]
pub enum TemplateCommand {
    /// List the templates on this device, Zeron's own first.
    List,
    /// Run Zeron's window with a template. `zeron template use zeron` puts
    /// Zeron's own back, even with Zeron closed.
    Use { id: String },
    /// Make a new template: a full copy of Zeron's interface to rewrite.
    Create {
        /// Its id: lowercase letters, digits and dashes.
        id: String,
        /// Its name (defaults to the id).
        #[arg(long)]
        name: Option<String>,
        /// A template to start from (defaults to Zeron's own).
        #[arg(long)]
        from: Option<String>,
    },
    /// Show a template's errors, out-of-date files and what it inherits.
    Status { id: String },
    /// Delete a template (it's set aside in the templates folder's .removed).
    Delete { id: String },
    /// Print Zeron's own files by hash, for the store's stock/<version>.json
    /// (the release's: what a template's first reviewer reads against).
    #[command(hide = true)]
    Stock {
        /// Zeron's own template folder, as the app lays it out (`.zeron`).
        zeron: std::path::PathBuf,
    },
    /// Check a template against the store's rules (README, preview, listing,
    /// version, files): every problem, before anything is sent. Needs no
    /// Zeron running and no sign-in. Exits 1 when it doesn't meet them.
    Check {
        /// The template's folder, or the id of one in Zeron's templates
        /// folder.
        target: String,
    },
    /// Publish a template to the store: it's live at once, for anyone to
    /// install, and marked verified once a reviewer has read it. With
    /// ZERON_DEPLOY_TOKEN set to a deploy token (never pass it as an
    /// argument): the folder is checked, packed and sent with the token, with
    /// no Zeron running and no sign-in. Exits as `zeron plugin publish` does.
    Publish {
        /// The template's folder, or the id of one in Zeron's templates
        /// folder.
        target: String,
        /// Check and pack it, but don't send it.
        #[arg(long)]
        dry_run: bool,
    },
}

/// `zeron mcp …`: the MCP servers Zeron gives agents. A server is named by
/// its id, the name agents see it by (`linear_work`), or its service and
/// account (`linear/Work`).
#[derive(Subcommand, Debug, PartialEq, Eq)]
pub enum McpCommand {
    /// List the servers, by service: each account, the name agents see it
    /// by, whether it's signed in and where it's kept. Then the ones there
    /// are to add.
    List {
        /// Print the running Zeron's whole list (servers and the ones to
        /// add) as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Add a server: one of Zeron's by its id (`zeron mcp list` names them),
    /// a remote one by its address (`--url`), or one this device runs
    /// (`--command`). One that signs in in the browser starts its sign-in.
    Add(McpAdd),
    /// Sign in to a server in the browser (Ctrl-C cancels).
    SignIn {
        server: String,
        #[command(flatten)]
        client: McpClientArgs,
    },
    /// Sign out of a server, keeping it (sign in again with `sign-in`).
    SignOut { server: String },
    /// Name a server's account ("Work"), telling it apart from the service's
    /// other accounts, here and to agents. An empty name clears it.
    Rename { server: String, account: String },
    /// Turn a server on: agents get it again.
    On { server: String },
    /// Turn a server off: agents don't get it until it's on again.
    Off { server: String },
    /// Remove a server and its sign-in.
    Remove {
        server: String,
        /// Don't ask first.
        #[arg(long, short)]
        yes: bool,
    },
}

/// `zeron mcp add`'s arguments: exactly one of a preset's id, `--url` or
/// `--command`.
#[derive(clap::Args, Debug, PartialEq, Eq)]
#[command(group(clap::ArgGroup::new("server").required(true).args(["preset", "url", "command"])))]
pub struct McpAdd {
    /// One of Zeron's servers, by id (`linear`, `notion`, …).
    #[arg(value_name = "ID")]
    pub preset: Option<String>,
    /// A remote server's address: https://…, or http:// on this computer.
    #[arg(long, value_parser = remote_url)]
    pub url: Option<String>,
    /// A server this device runs, speaking MCP on its stdin/stdout; its
    /// arguments follow `--`.
    #[arg(long, value_name = "CMD", value_parser = non_empty, conflicts_with_all = ["auth", "token_env", "client_id"])]
    pub command: Option<String>,
    /// Its name (`--url` and `--command` servers'; a preset has its own).
    #[arg(long, value_parser = non_empty, required_unless_present = "preset", conflicts_with = "preset")]
    pub name: Option<String>,
    /// The account's name ("Work"): what tells two accounts of one service
    /// apart. Needed to add a second account of a service.
    #[arg(long, value_name = "NAME")]
    pub account: Option<String>,
    /// How a `--url` server signs in (default: oauth, or bearer with
    /// `--token-env`).
    #[arg(long, value_enum, conflicts_with = "preset")]
    pub auth: Option<McpAuth>,
    /// The environment variable holding its token, for one that signs in
    /// with a bearer token (never on the command line, where other
    /// processes and your shell's history would see it).
    #[arg(long, value_name = "VAR")]
    pub token_env: Option<String>,
    /// An environment variable of this shell's to hand a `--command` server,
    /// with its value now (repeat for more).
    #[arg(long = "env", value_name = "VAR", conflicts_with_all = ["preset", "url"])]
    pub env: Vec<String>,
    #[command(flatten)]
    pub client: McpClientArgs,
    /// The `--command` server's arguments.
    #[arg(last = true, value_name = "ARGS", conflicts_with_all = ["preset", "url"])]
    pub args: Vec<String>,
}

/// Your own OAuth app, for a server that can't register Zeron's itself
/// (it answers `mcp_client_required`).
#[derive(clap::Args, Debug, PartialEq, Eq, Default)]
pub struct McpClientArgs {
    /// Your OAuth app's client ID, for a server that signs in only with an
    /// app of your own.
    #[arg(long, value_name = "ID", value_parser = non_empty)]
    pub client_id: Option<String>,
    /// The environment variable holding that app's client secret.
    #[arg(long, value_name = "VAR", requires = "client_id")]
    pub client_secret_env: Option<String>,
}

/// How a custom remote MCP server signs in.
#[derive(clap::ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpAuth {
    /// In the browser (OAuth).
    Oauth,
    /// With a token (`--token-env`).
    Bearer,
    /// Not at all.
    None,
}

/// A remote MCP server's address: `https://…`, or plain `http://` only to
/// this computer (anywhere else its sign-in would cross the network in the
/// clear).
fn remote_url(text: &str) -> Result<String, String> {
    let url = text.trim();
    if url.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("an address has no spaces".into());
    }
    let host_rest = if let Some(rest) = url.strip_prefix("https://") {
        rest
    } else if let Some(rest) = url.strip_prefix("http://") {
        let loopback = ["127.0.0.1", "localhost", "[::1]"].iter().any(|host| {
            rest.strip_prefix(host)
                .is_some_and(|after| after.is_empty() || after.starts_with([':', '/', '?']))
        });
        if !loopback {
            return Err("use https:// (plain http:// only to this computer)".into());
        }
        rest
    } else {
        return Err("give an https:// address".into());
    };
    if host_rest.is_empty() || host_rest.starts_with(['/', '?', ':']) {
        return Err("the address has no host".into());
    }
    Ok(url.to_owned())
}

fn non_empty(text: &str) -> Result<String, String> {
    match text.trim() {
        "" => Err("it can't be empty".into()),
        text => Ok(text.to_owned()),
    }
}

/// A subcommand that runs the engine's own code in this process: what the
/// engine helper (or a binary linking the engine) does with it.
pub enum EngineCommand {
    Headless,
    Engine,
    Login,
    Logout,
    Status,
    Sync,
    Quiesce,
    /// `zeron mcp`: the Zeron MCP server on stdio.
    Mcp,
    /// `zeron mcp <subcommand>`: the MCP servers Zeron gives agents.
    McpServers(McpCommand),
    Plugin(PluginCommand),
    Template(TemplateCommand),
    DiscoverModels(String),
}

/// Where this binary runs [`EngineCommand`]s.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engines {
    /// Here: the binary links the engine (`zeron`, `zeron-engine`).
    InProcess,
    /// In the engine helper beside this binary, which replaces this process
    /// with the same arguments (an interface that doesn't link the engine).
    Helper,
}

/// What's left for the binary once [`run`] has parsed its arguments.
pub enum Launch {
    /// A command ran here; exit with its result.
    Done(anyhow::Result<()>),
    /// Run this in-process ([`Engines::InProcess`] only); logging is set up.
    Engine(EngineCommand),
    /// No subcommand: open the window, on a conversation URL if one was
    /// given; `safe_mode` with Zeron's own interface (`--safe-mode`).
    Window { open_url: Option<String>, safe_mode: bool },
    /// Ask the running headed instance for an Appshot (the interface's own).
    #[cfg(target_os = "linux")]
    Appshot,
}

/// The parser, as `binary`'s `--version` names it: `version` is the build's
/// (the workspace's, or a beta interface's own, `0.2.136-beta.1`).
pub fn command(version: &'static str) -> clap::Command {
    Cli::command().version(version)
}

/// Parse this process's arguments and run what needs neither the engine nor
/// the interface. Help, `--version` and argument errors print and exit here,
/// as clap does.
pub fn run(version: &'static str, engines: Engines) -> Launch {
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("--noop-browser")) {
        return Launch::Done(Ok(()));
    }
    #[cfg(windows)]
    attach_parent_console();
    let cli = match Cli::from_arg_matches(&command(version).get_matches()) {
        Ok(cli) => cli,
        Err(error) => error.exit(),
    };
    if engines == Engines::Helper && cli.command.as_ref().is_some_and(Command::runs_engine) {
        // The helper parses the same arguments and does all of the below
        // itself (logging, the command). Windows' image cleanup is this
        // executable's, though (the helper isn't the installed zeron.exe): an
        // install that's only ever run headless must lose the image an update
        // set aside too.
        #[cfg(windows)]
        if matches!(&cli.command, Some(Command::Headless) | Some(Command::Engine)) {
            zeron_update::windows::cleanup_previous_image();
        }
        let exe = match std::env::current_exe() {
            Ok(exe) => exe,
            Err(error) => return Launch::Done(Err(error.into())),
        };
        let error = zeron_launch::exec_helper(&exe, std::env::args_os().skip(1));
        return Launch::Done(Err(anyhow::anyhow!("couldn't start the engine: {error}")));
    }
    #[cfg(windows)]
    if let Some(pid) = cli.wait_for_exit {
        if let Err(error) = zeron_update::windows::wait_for_exit(pid) {
            return Launch::Done(Err(error));
        }
    } else if matches!(
        &cli.command,
        None | Some(Command::Headless) | Some(Command::Engine)
    ) {
        zeron_update::windows::cleanup_previous_image();
    }
    let long_running = matches!(&cli.command, None | Some(Command::Headless));
    let mode = if cli.command.is_some() {
        "headless"
    } else {
        "headed"
    };
    // `zeron mcp` owns stdout for the protocol: a single log line on it
    // would corrupt the JSON-RPC stream, so its diagnostics go to stderr.
    // The background engine logs on its own (`zeron_app::background`).
    if !matches!(&cli.command, Some(Command::Engine)) {
        init_logging(
            long_running,
            mode,
            matches!(
                &cli.command,
                Some(Command::Mcp { .. } | Command::Plugin { .. } | Command::Template { .. } | Command::DiscoverModels { .. })
            ),
        );
    }
    if long_running {
        long_running_setup();
    }

    let engine = |command| match engines {
        Engines::InProcess => Launch::Engine(command),
        // (Handed to the helper above.)
        Engines::Helper => unreachable!("engine commands run in the helper"),
    };
    match cli.command {
        Some(Command::Headless) => engine(EngineCommand::Headless),
        Some(Command::Engine) => engine(EngineCommand::Engine),
        Some(Command::Login) => engine(EngineCommand::Login),
        Some(Command::Logout) => engine(EngineCommand::Logout),
        Some(Command::Status) => engine(EngineCommand::Status),
        Some(Command::Sync) => engine(EngineCommand::Sync),
        Some(Command::Quiesce) => engine(EngineCommand::Quiesce),
        Some(Command::Mcp { command: None }) => engine(EngineCommand::Mcp),
        Some(Command::Mcp { command: Some(command) }) => engine(EngineCommand::McpServers(command)),
        Some(Command::DiscoverModels { harness }) => engine(EngineCommand::DiscoverModels(harness)),
        Some(Command::Plugin { command }) => engine(EngineCommand::Plugin(command)),
        Some(Command::Template { command }) => engine(EngineCommand::Template(command)),
        #[cfg(target_os = "linux")]
        Some(Command::Appshot) => Launch::Appshot,
        Some(Command::Update { check }) => Launch::Done(
            tokio::runtime::Runtime::new()
                .map_err(anyhow::Error::from)
                .and_then(|runtime| runtime.block_on(update_cli::update(&edge_url_from_env(), check))),
        ),
        Some(Command::UpdateWatchdog) => Launch::Done(zeron_update::transaction::run_watchdog()),
        Some(Command::Daemon { command }) => Launch::Done(match command {
            DaemonCommand::Install => daemon::install(&paths::data_dir()),
            DaemonCommand::Uninstall => daemon::uninstall(),
            DaemonCommand::Start => daemon::start(),
            DaemonCommand::Stop => daemon::stop(),
            DaemonCommand::Restart => daemon::restart(),
            DaemonCommand::Status => daemon::status(),
        }),
        None => Launch::Window {
            open_url: cli.open_url,
            safe_mode: cli.safe_mode,
        },
    }
}

impl Command {
    /// Whether it runs the engine's code ([`EngineCommand`]).
    fn runs_engine(&self) -> bool {
        match self {
            Self::Headless
            | Self::Engine
            | Self::Login
            | Self::Logout
            | Self::Status
            | Self::Sync
            | Self::Quiesce
            | Self::Mcp { .. }
            | Self::Plugin { .. }
            | Self::Template { .. }
            | Self::DiscoverModels { .. } => true,
            #[cfg(target_os = "linux")]
            Self::Appshot => false,
            Self::Update { .. } | Self::UpdateWatchdog | Self::Daemon { .. } => false,
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

/// Every subcommand path of the command line (`[]`, `["plugin"]`,
/// `["plugin", "new"]`, …), hidden ones included: what the parity checks of
/// each binary that runs it walk (`apps/zeron/tests/cli.rs`,
/// `apps/zgx/tests/cli.rs`).
pub fn subcommand_paths(version: &'static str) -> Vec<Vec<String>> {
    fn walk(command: &clap::Command, path: &mut Vec<String>, out: &mut Vec<Vec<String>>) {
        out.push(path.clone());
        for sub in command.get_subcommands() {
            if sub.get_name() == "help" {
                continue;
            }
            path.push(sub.get_name().to_owned());
            walk(sub, path, out);
            path.pop();
        }
    }
    let mut out = Vec::new();
    walk(&command(version), &mut Vec::new(), &mut out);
    out
}

/// `zeron <path…> --help` as a binary named `zeron` prints it: clap's own
/// answer to those arguments.
pub fn help_for(version: &'static str, path: &[String]) -> String {
    let argv = std::iter::once("zeron".to_owned())
        .chain(path.iter().cloned())
        .chain(std::iter::once("--help".to_owned()));
    match command(version).try_get_matches_from(argv) {
        Err(help) if help.kind() == clap::error::ErrorKind::DisplayHelp => help.render().to_string(),
        other => panic!("zeron {} --help didn't print help: {other:?}", path.join(" ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_line_is_well_formed() {
        command("0.0.0").debug_assert();
    }

    #[test]
    fn every_subcommand_is_reachable() {
        let paths = subcommand_paths("0.0.0");
        for expected in [
            &["headless"][..],
            &["engine"],
            &["login"],
            &["logout"],
            &["status"],
            &["sync"],
            &["quiesce"],
            &["mcp"],
            &["mcp", "list"],
            &["mcp", "add"],
            &["mcp", "sign-in"],
            &["mcp", "remove"],
            &["plugin", "new"],
            &["plugin", "export"],
            &["plugin", "check"],
            &["plugin", "publish"],
            &["plugin", "status"],
            &["plugin", "token", "create"],
            &["plugin", "token", "list"],
            &["plugin", "token", "revoke"],
            &["plugin", "yank"],
            &["plugin", "unyank"],
            &["template", "use"],
            &["daemon", "install"],
            &["update"],
        ] {
            assert!(
                paths.iter().any(|path| path == expected),
                "{expected:?} missing from {paths:?}"
            );
        }
    }

    /// What runs where: the engine's commands go to the engine (in-process
    /// or the helper), the rest run here.
    #[test]
    fn engine_commands_are_the_ones_that_run_the_engine() {
        let parse = |args: &[&str]| {
            let argv = std::iter::once("zeron").chain(args.iter().copied());
            Cli::from_arg_matches(&command("0.0.0").try_get_matches_from(argv).unwrap())
                .unwrap()
                .command
        };
        for args in [
            &["headless"][..],
            &["engine"],
            &["login"],
            &["logout"],
            &["status"],
            &["sync"],
            &["quiesce"],
            &["mcp"],
            &["plugin", "list"],
            &["plugin", "export", "focus"],
            &["template", "list"],
        ] {
            assert!(parse(args).unwrap().runs_engine(), "{args:?}");
        }
        for args in [&["update", "--check"][..], &["daemon", "status"]] {
            assert!(!parse(args).unwrap().runs_engine(), "{args:?}");
        }
        assert!(parse(&[]).is_none());
    }

    /// `--to` reaches the engine as a full path: a relative one is taken
    /// from the current folder.
    #[test]
    fn plugin_export_takes_a_full_path() {
        let export = |args: &[&str]| {
            let argv = ["zeron", "plugin", "export"].into_iter().chain(args.iter().copied());
            match Cli::from_arg_matches(&command("0.0.0").try_get_matches_from(argv).unwrap())
                .unwrap()
                .command
            {
                Some(Command::Plugin { command: PluginCommand::Export { id, to } }) => (id, to),
                _ => panic!("not plugin export"),
            }
        };
        assert_eq!(export(&["focus"]), ("focus".to_owned(), None));
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(
            export(&["focus", "--to", "dist/focus.zeron-plugin"]).1,
            Some(cwd.join("dist/focus.zeron-plugin"))
        );
        assert_eq!(export(&["focus", "--to", "."]).1.unwrap(), cwd);
        let full = if cfg!(windows) { r"C:\out" } else { "/tmp/out" };
        assert_eq!(export(&["focus", "--to", full]).1, Some(std::path::PathBuf::from(full)));
        assert!(
            command("0.0.0")
                .try_get_matches_from(["zeron", "plugin", "export", "focus", "--to", ""])
                .is_err()
        );
        assert!(command("0.0.0").try_get_matches_from(["zeron", "plugin", "export"]).is_err());
    }

    fn mcp(args: &[&str]) -> Result<Option<McpCommand>, clap::Error> {
        let argv = ["zeron", "mcp"].into_iter().chain(args.iter().copied());
        match Cli::from_arg_matches(&command("0.0.0").try_get_matches_from(argv)?)?.command {
            Some(Command::Mcp { command }) => Ok(command),
            _ => panic!("not mcp"),
        }
    }

    /// Plain `zeron mcp` is still the stdio server agents' configs run; a
    /// subcommand manages the servers.
    #[test]
    fn plain_mcp_still_serves_stdio() {
        assert_eq!(mcp(&[]).unwrap(), None);
        assert!(mcp(&["--no-such-flag"]).is_err());
        assert!(mcp(&["serve"]).is_err());
        assert_eq!(mcp(&["list"]).unwrap(), Some(McpCommand::List { json: false }));
        assert_eq!(mcp(&["list", "--json"]).unwrap(), Some(McpCommand::List { json: true }));
        assert_eq!(
            mcp(&["sign-in", "linear/Work"]).unwrap(),
            Some(McpCommand::SignIn { server: "linear/Work".into(), client: McpClientArgs::default() })
        );
        assert_eq!(mcp(&["sign-out", "linear"]).unwrap(), Some(McpCommand::SignOut { server: "linear".into() }));
        assert_eq!(
            mcp(&["rename", "linear", ""]).unwrap(),
            Some(McpCommand::Rename { server: "linear".into(), account: String::new() })
        );
        assert_eq!(mcp(&["on", "linear"]).unwrap(), Some(McpCommand::On { server: "linear".into() }));
        assert_eq!(mcp(&["off", "linear"]).unwrap(), Some(McpCommand::Off { server: "linear".into() }));
        assert_eq!(
            mcp(&["remove", "linear", "-y"]).unwrap(),
            Some(McpCommand::Remove { server: "linear".into(), yes: true })
        );
        assert!(mcp(&["remove"]).is_err());
        // The stdio server and the management commands both run the engine's
        // code (the helper's, for an interface without it).
        let parse = |args: &[&str]| {
            let argv = ["zeron", "mcp"].into_iter().chain(args.iter().copied());
            Cli::from_arg_matches(&command("0.0.0").try_get_matches_from(argv).unwrap()).unwrap().command.unwrap()
        };
        assert!(parse(&[]).runs_engine());
        assert!(parse(&["list"]).runs_engine());
    }

    #[test]
    fn mcp_add_takes_one_kind_of_server() {
        let add = |args: &[&str]| match mcp(&[&["add"][..], args].concat()) {
            Ok(Some(McpCommand::Add(add))) => Ok(add),
            Ok(other) => panic!("not add: {other:?}"),
            Err(error) => Err(error),
        };
        let preset = add(&["linear", "--account", "Work"]).unwrap();
        assert_eq!(preset.preset.as_deref(), Some("linear"));
        assert_eq!(preset.account.as_deref(), Some("Work"));
        assert_eq!((preset.url, preset.command, preset.name), (None, None, None));

        let remote = add(&["--url", "https://mcp.example.com/mcp", "--name", "Example", "--token-env", "EXAMPLE_TOKEN"]).unwrap();
        assert_eq!(remote.url.as_deref(), Some("https://mcp.example.com/mcp"));
        assert_eq!(remote.name.as_deref(), Some("Example"));
        assert_eq!(remote.token_env.as_deref(), Some("EXAMPLE_TOKEN"));
        assert_eq!(add(&["--url", "https://x.dev", "--name", "X", "--auth", "none"]).unwrap().auth, Some(McpAuth::None));
        assert!(add(&["--url", "http://localhost:3000/mcp", "--name", "Dev"]).is_ok());
        assert!(add(&["--url", "http://127.0.0.1", "--name", "Dev"]).is_ok());

        let local = add(&["--command", "npx", "--name", "Files", "--env", "ROOT", "--", "-y", "@acme/files", "--root"]).unwrap();
        assert_eq!(local.command.as_deref(), Some("npx"));
        assert_eq!(local.args, ["-y", "@acme/files", "--root"]);
        assert_eq!(local.env, ["ROOT"]);

        for bad in [
            &[][..],
            // Two kinds at once.
            &["linear", "--url", "https://x.dev"],
            &["--url", "https://x.dev", "--command", "npx", "--name", "X"],
            // A custom server needs a name; a preset has its own.
            &["--url", "https://x.dev"],
            &["--command", "npx"],
            &["linear", "--name", "Mine"],
            &["--url", "https://x.dev", "--name", " "],
            // Plain http only to this computer; something with a host.
            &["--url", "http://mcp.example.com", "--name", "X"],
            &["--url", "http://localhost.evil.com", "--name", "X"],
            &["--url", "ftp://x.dev", "--name", "X"],
            &["--url", "https://", "--name", "X"],
            &["--url", "https://x.dev/a b", "--name", "X"],
            // What belongs to one kind isn't taken with another.
            &["linear", "--auth", "none"],
            &["--command", "npx", "--name", "X", "--token-env", "T"],
            &["--command", "npx", "--name", "X", "--client-id", "abc"],
            &["linear", "--env", "ROOT"],
            &["linear", "--", "-y"],
            &["--url", "https://x.dev", "--name", "X", "--client-secret-env", "S"],
        ] {
            assert!(add(bad).is_err(), "zeron mcp add {bad:?} parsed");
        }
    }

    #[test]
    fn mcp_sign_in_takes_your_own_app() {
        let Some(McpCommand::SignIn { server, client }) =
            mcp(&["sign-in", "slack", "--client-id", "123.456", "--client-secret-env", "SLACK_SECRET"]).unwrap()
        else {
            panic!("not sign-in");
        };
        assert_eq!(server, "slack");
        assert_eq!(client.client_id.as_deref(), Some("123.456"));
        assert_eq!(client.client_secret_env.as_deref(), Some("SLACK_SECRET"));
        // A secret only with its app's ID; never the secret itself.
        assert!(mcp(&["sign-in", "slack", "--client-secret-env", "S"]).is_err());
        assert!(mcp(&["sign-in", "slack", "--client-secret", "s3cret"]).is_err());
    }

    #[test]
    fn the_version_is_the_build_s() {
        let version = command("0.2.136-beta.1").render_version();
        assert_eq!(version.trim(), "zeron 0.2.136-beta.1");
    }
}
