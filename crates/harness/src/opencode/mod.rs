//! Native opencode driver — the same HTTP + SSE protocol the opencode
//! desktop app speaks, replacing the ACP path (`opencode acp`).
//!
//! Why not ACP: opencode's own ACP layer settles a prompt on the FIRST
//! `session.status{idle}` it observes after subscribing, uncorrelated with
//! the submitted turn, and it ignores `session.error` and `session.idle`
//! entirely (`packages/opencode/src/acp/event.ts`, 1.18.x) — the source of
//! "done while still working" and silently-hung turns. It also drops all
//! subagent traffic and thinking never reaches the ACP wire usefully. The
//! desktop app doesn't use ACP; neither do we.
//!
//! Two server generations are spoken, detected at boot from version-bearing
//! endpoints ([`Protocol`]): the 1.18 "v1" wire (verified against 1.18.31)
//! and the 2.x `/api/*` wire (2.0.3 turns; 2.0.11 discovery and schema,
//! with scripted coverage for the 2.0.4+ command and agent routes):
//! - spawn `opencode serve --port <free> --hostname 127.0.0.1` with
//!   `OPENCODE_SERVER_PASSWORD=<uuid>` (HTTP Basic, username `opencode`);
//!   readiness probes `/api/info`, `/api/status`, `/api/health` (2.x),
//!   then `/global/health` (1.x), accepting version-bearing JSON.
//! - 2.x discovery uses `GET /api/model` (with a plugin-settle poll),
//!   `/api/agent` (Agent model option), and `/api/command`.
//! - 2.x creates via `POST /api/session` with `location.directory` and
//!   optional `agent`; resumed selection uses `/api/session/{id}/agent`.
//!   Session model selection uses `/api/session/{id}/model`; cancellation
//!   uses `/api/session/{id}/interrupt`; recovery uses `GET /api/session/active`.
//! - slash commands use `POST /api/session/{id}/command`: `command` through
//!   2.0.3, `name` from 2.0.4, with `text` arguments. 2.x directory scoping
//!   uses the `x-opencode-directory` header.
//! - one global SSE bus (`GET /api/event` on 2.x, `GET /global/event` on
//!   1.x) carries every session's traffic, child (subagent) sessions
//!   included, token-level. 2.x frames are rewritten into the 1.x payload
//!   shapes ([`normalize_v2_frame`]) so the turn engine is wire-agnostic.
//! - a turn is `POST /api/session/{id}/prompt` (2.x; model set once on the
//!   session) or `POST /session/{id}/prompt_async` (1.x, fire-and-forget
//!   204); the END of the turn is the terminal execution/status frame for
//!   THAT session — exactly what the desktop's working-predicate keys on.
//!   Busy is re-asserted at the top of every agent-loop iteration, so idle
//!   after busy is authoritative, not a lull.
//! - reasoning streams as reasoning parts on both wires →
//!   [`AgentEvent::ReasoningDelta`], the thinking feed.
//! - the agent's `question` tool reaches the engine's input bridge on both
//!   wires: 1.x emits `question.asked` (reply `POST /question/{id}/reply`),
//!   while 2.x (2.0.24+) creates a session FORM titled "Questions"
//!   (`form.created`, reply `POST /api/session/{id}/form/{formID}/reply`
//!   with `{answer:{q0:..}}`; verified live against 2.0.24), folded into
//!   the same question flow.
//!
//! Failure surfacing (the #169 class): a dying provider is VISIBLE here —
//! `session.status{type:"retry", attempt, message}` streams per attempt.
//! Attempt ≥ [`RETRY_REPORT_ATTEMPT`] surfaces an error chip; attempt ≥
//! [`RETRY_ABORT_ATTEMPT`] aborts the turn instead of retrying forever.
//! A prompt that produces NO session-scoped event within
//! [`default_stall_bound`] (`ZERON_OPENCODE_STALL_MS`, 0 disables) of the
//! server accepting it errors out instead of spinning "Working" forever.
//! Both wires report busy within milliseconds of accepting a prompt, before
//! any provider bootstrap, so a slow first turn never trips it (verified
//! live, 1.18.31 and 2.1.2). A server process that dies is noticed within
//! [`SERVER_POLL`], and a bus silent past [`BUS_SILENCE`] is reconnected —
//! unless the server, still alive, fails a health check there
//! ([`BUS_PROBE_TIMEOUT`]) or the bus stays frameless past
//! [`BUS_WEDGE_BUDGET`]: then the run errors and the server is shut down.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use zeron_proto::{
    AgentEvent, DoneStatus, HarnessId, Model, ModelOption, ModelOptionChoice, ReasoningLevel,
    RunRequest, SlashCommand, SteeringMode, TodoItem, TodoStatus, ToolCall, UserInputAnswer,
    UserInputQuestion,
};

use crate::process::{Child, Command, Stdio};
use crate::{Harness, HarnessError, RunControls};

/// opencode loads plugins and MCP config before the server answers; cold
/// plugin-heavy starts can take minutes. Shared by chat startup and model
/// discovery (same boot either way).
const DEFAULT_STARTUP_TIMEOUT: Duration = Duration::from_secs(300);
const STARTUP_TIMEOUT_ENV: &str = "ZERON_OPENCODE_STARTUP_TIMEOUT_SECS";

/// Health-poll cadence while the server boots.
const HEALTH_POLL: Duration = Duration::from_millis(150);

/// Bound on ordinary (non-SSE) HTTP calls: everything is loopback and the
/// only slow route is a cold /provider catalog. The synchronous per-turn
/// command endpoint deliberately bypasses this because its response can take
/// the whole turn; its detached task still reports HTTP and transport errors
/// to the generation that launched it.
const CALL_TIMEOUT: Duration = Duration::from_secs(60);

/// Bus reconnect: the server is our own child on loopback, so a dropped
/// stream with a live process is transient — retry briefly, then treat the
/// run as dead (transcript integrity is gone once frames are missed).
const BUS_RECONNECT_DELAY: Duration = Duration::from_millis(250);
const BUS_RECONNECT_ATTEMPTS: u32 = 40;

/// Longest the bus may stay silent before the stream is presumed wedged and
/// reconnected (the reconnect re-syncs status and pending requests). Both
/// generations keep an idle stream talking: 1.x sends a `server.heartbeat`
/// event every 10s (1.14.17 through 1.18.31), 2.x a `: heartbeat` comment
/// every 15s (2.1.2); both verified live.
const BUS_SILENCE: Duration = Duration::from_secs(45);

/// A bus silent past [`BUS_SILENCE`] gets the server checked before it is
/// reconnected: a process that is stopped or hung still accepts loopback
/// connections (the kernel completes the handshake), so each reconnect
/// would otherwise wait out another silence, and the reconnect budget only
/// runs out after half an hour of "Working" (observed live, 1.18.21 and
/// 2.1.2, against a SIGSTOPped server: the connect succeeds, the request is
/// never answered). Any HTTP answer to its info route proves it alive;
/// none within this bound ends the run.
const BUS_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Longest the bus may go without one frame (heartbeats included) before
/// the server is presumed wedged even though it still answers health
/// checks: a few silences' worth of reconnects.
const BUS_WEDGE_BUDGET: Duration = Duration::from_secs(150);

/// Bus liveness bounds; tests shrink them.
#[derive(Clone, Copy, Debug)]
struct BusTimeouts {
    silence: Duration,
    probe: Duration,
    wedged: Duration,
}

impl Default for BusTimeouts {
    fn default() -> Self {
        Self {
            silence: BUS_SILENCE,
            probe: BUS_PROBE_TIMEOUT,
            wedged: BUS_WEDGE_BUDGET,
        }
    }
}

/// How often the run checks that its own server process is still alive. A
/// crash otherwise surfaces only once the bus exhausts its reconnects, and
/// each refused loopback connect costs about 2s on Windows.
const SERVER_POLL: Duration = Duration::from_millis(500);

/// Prompt deliveries: one retry, posted only once the session is checked
/// for the message (see [`deliver_prompt`]).
const PROMPT_ATTEMPTS: u32 = 2;
const PROMPT_RETRY_DELAY: Duration = Duration::from_millis(500);

/// Permission and question replies: the agent blocks until one lands.
const REPLY_ATTEMPTS: u32 = 3;
const REPLY_TIMEOUT: Duration = Duration::from_secs(10);

/// Budget for aborting a stopped turn's subagent sessions.
const CHILD_ABORT_BUDGET: Duration = Duration::from_secs(15);

/// Provider-retry surfacing: report at 3 (a chip; the turn keeps trying),
/// abort at 8 (opencode itself would retry forever — #169's silent loop).
const RETRY_REPORT_ATTEMPT: u64 = 3;
const RETRY_ABORT_ATTEMPT: u64 = 8;

/// Default bound on prompt-send → first session-scoped bus event.
const DEFAULT_STALL_BOUND: Duration = Duration::from_secs(60);
const STALL_ENV: &str = "ZERON_OPENCODE_STALL_MS";

/// What a wedged/silent run usually means for opencode.
const STALL_HINT: &str = "The model provider is likely unreachable or rejecting requests. \
     Check the model/provider setup (`opencode auth list`, opencode.json) or the opencode \
     log (~/.local/share/opencode/log).";

fn startup_timeout() -> Duration {
    std::env::var(STARTUP_TIMEOUT_ENV)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|s| *s > 0)
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_STARTUP_TIMEOUT)
}

fn stall_bound() -> Option<Duration> {
    match std::env::var(STALL_ENV) {
        Ok(v) => match v.parse::<u64>() {
            Ok(0) => None,
            Ok(ms) => Some(Duration::from_millis(ms)),
            Err(_) => Some(DEFAULT_STALL_BOUND),
        },
        Err(_) => Some(DEFAULT_STALL_BOUND),
    }
}

fn opencode_install_paths() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    // `crate::executable::home_dir` checks USERPROFILE too, so the Unix-style
    // install locations resolve on Windows (`~/.local/bin/opencode.exe`, the
    // bun layout) instead of silently vanishing with an unset HOME.
    if let Some(home) = crate::executable::home_dir() {
        dirs.push(home.join(".opencode").join("bin").join("opencode"));
        dirs.push(home.join(".local").join("bin").join("opencode"));
        dirs.push(home.join(".bun").join("bin").join("opencode"));
        dirs.push(home.join(".npm-global").join("bin").join("opencode"));
    }
    dirs.push(PathBuf::from("/opt/homebrew/bin/opencode"));
    dirs.push(PathBuf::from("/usr/local/bin/opencode"));
    dirs
}

const INSTALL_HINT: &str = "opencode (searched PATH, the login shell's PATH, ~/.opencode/bin, \
     ~/.local/bin, ~/.bun/bin, ~/.npm-global/bin, /opt/homebrew/bin, /usr/local/bin, and \
     fnm/nvm/volta/pnpm/bun install dirs; Windows also checks USERPROFILE, \
     %APPDATA%\\npm, and explicit NVM_SYMLINK/VOLTA_HOME/PNPM_HOME; install with \
     `curl -fsSL https://opencode.ai/install | bash` or \
     `npm install -g @opencode/cli`, then `opencode auth login`; set \
     OPENCODE_EXECUTABLE to override)";

/// The user's opencode: `OPENCODE_EXECUTABLE` when it exists, else PATH (plus
/// install locations). The override is validated at launch through
/// [`crate::executable::validate_native_override`] — including the `.cmd`
/// shims npm installs on Windows.
fn resolve_opencode_executable() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("OPENCODE_EXECUTABLE")
        && !path.is_empty()
    {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    crate::acp::find_on_paths("opencode", opencode_install_paths())
}

/// Effort ladder surfaced in the picker; applied per run by picking the
/// first of these variant ids the chosen model actually advertises. Must
/// mirror the registry descriptor.
const REASONING_LEVELS: &[ReasoningLevel] = &[
    ReasoningLevel::Low,
    ReasoningLevel::Medium,
    ReasoningLevel::High,
    ReasoningLevel::XHigh,
    ReasoningLevel::Max,
];

/// Preference-ordered variant ids for a requested effort (opencode's
/// reasoning variants: models.dev metadata or `variants` in opencode.json).
fn variant_candidates(reasoning: Option<ReasoningLevel>) -> Vec<&'static str> {
    let Some(level) = reasoning else {
        return Vec::new();
    };
    match level {
        ReasoningLevel::Minimal => vec!["minimal", "low"],
        ReasoningLevel::Low => vec!["low", "minimal"],
        ReasoningLevel::Medium => vec!["medium"],
        ReasoningLevel::High => vec!["high"],
        ReasoningLevel::XHigh => vec!["xhigh", "x-high", "high"],
        ReasoningLevel::Max => vec!["max", "xhigh", "high"],
        ReasoningLevel::Ultra | ReasoningLevel::Ultracode | ReasoningLevel::Ultrathink => {
            vec!["ultra", "max", "high"]
        }
    }
}

fn variant_to_level(id: &str) -> Option<ReasoningLevel> {
    match id {
        "minimal" => Some(ReasoningLevel::Minimal),
        "low" => Some(ReasoningLevel::Low),
        "medium" => Some(ReasoningLevel::Medium),
        "high" => Some(ReasoningLevel::High),
        "xhigh" | "x-high" => Some(ReasoningLevel::XHigh),
        "max" => Some(ReasoningLevel::Max),
        _ => None,
    }
}

/// Startup retries for a server that exited on a transient fault (a locked
/// database, a lost migration race, a port claimed between our release and
/// its bind), backing off
/// linearly from [`SPAWN_RETRY_DELAY`].
const SPAWN_RETRIES: u32 = 4;
const SPAWN_RETRY_DELAY: Duration = Duration::from_millis(250);

/// Oldest release the 1.x wire is verified against; older servers may never
/// answer a health probe this driver knows.
const TESTED_SINCE: &str = "1.14";

enum SpawnFailure {
    /// Launching again can succeed.
    Retryable(HarnessError),
    Fatal(HarnessError),
}

/// opencode's report of its shared SQLite database held by another process.
fn is_database_locked(line: &str) -> bool {
    let line = line.to_ascii_lowercase();
    line.contains("database is locked") || line.contains("sqlite_busy")
}

/// A schema migration that lost a race with another process's: concurrent
/// first starts on a fresh database each run the migration DDL, and the
/// losers exit with "Unexpected error" / "Failed query:", the statement on
/// the lines after it (observed live, 1.18.21: 3-5 of 6 at once). A relaunch
/// finds the schema in place. Fed line by line.
#[derive(Default)]
struct MigrationRace {
    /// The last non-blank line was a bare "Failed query:".
    after_failed_query: bool,
}

impl MigrationRace {
    fn feed(&mut self, line: &str) -> bool {
        let line = line.trim().to_ascii_lowercase();
        let is_ddl = |sql: &str| {
            ["create ", "alter ", "drop "]
                .iter()
                .any(|verb| sql.starts_with(verb))
        };
        if let Some((_, sql)) = line.split_once("failed query:") {
            let sql = sql.trim();
            self.after_failed_query = sql.is_empty();
            return is_ddl(sql);
        }
        if line.is_empty() {
            return false;
        }
        std::mem::take(&mut self.after_failed_query) && is_ddl(&line)
    }
}

/// The port we picked was claimed before the server bound it.
fn is_address_in_use(text: &str) -> bool {
    let text = text.to_ascii_lowercase();
    text.contains("eaddrinuse")
        || text.contains("address already in use")
        || text.contains("failed to start server. is port")
        // WSAEADDRINUSE's message.
        || text.contains("only one usage of each socket address")
}

/// A kernel-assigned free localhost port, released for the child to claim.
fn free_localhost_port() -> Option<u16> {
    std::net::TcpListener::bind(("127.0.0.1", 0))
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .ok()
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

pub struct OpencodeHarness {
    executable: Option<PathBuf>,
    /// Test seam: an already-running server (no spawn, no auth unless given).
    base_url: Option<String>,
    interrupt_grace: Duration,
    kill_grace: Duration,
    bus_timeouts: BusTimeouts,
    startup_timeout: Duration,
    models_cache: crate::catalog::Catalog,
    commands_cache: tokio::sync::OnceCell<Vec<SlashCommand>>,
    /// Coalesce concurrent picker/title probes: several cold opencode boots
    /// at once are slower than one.
    probe_lock: tokio::sync::Mutex<()>,
}

impl Default for OpencodeHarness {
    fn default() -> Self {
        Self {
            executable: None,
            base_url: None,
            interrupt_grace: Duration::from_secs(2),
            kill_grace: Duration::from_secs(3),
            bus_timeouts: BusTimeouts::default(),
            startup_timeout: startup_timeout(),
            models_cache: crate::catalog::Catalog::default(),
            commands_cache: tokio::sync::OnceCell::new(),
            probe_lock: tokio::sync::Mutex::new(()),
        }
    }
}

impl OpencodeHarness {
    pub fn new() -> Self {
        Self::default()
    }

    /// Use a fixed binary instead of PATH/known-location resolution.
    pub fn with_executable(mut self, path: impl Into<PathBuf>) -> Self {
        self.executable = Some(path.into());
        self
    }

    /// Drive an already-running server (tests): no spawn, no basic auth.
    pub fn with_base_url(mut self, base: impl Into<String>) -> Self {
        self.base_url = Some(base.into());
        self
    }

    pub fn with_graces(mut self, interrupt: Duration, kill: Duration) -> Self {
        self.interrupt_grace = interrupt;
        self.kill_grace = kill;
        self
    }

    /// Shrink the event-bus liveness bounds (tests): the silence that gets
    /// the server health-checked, that check's answer bound, and the
    /// longest the bus may go without a frame.
    pub fn with_bus_timeouts(
        mut self,
        silence: Duration,
        probe: Duration,
        wedged: Duration,
    ) -> Self {
        self.bus_timeouts = BusTimeouts {
            silence,
            probe,
            wedged,
        };
        self
    }

    fn resolve_executable(&self) -> Result<PathBuf, HarnessError> {
        if let Some(exe) = &self.executable {
            return Ok(exe.clone());
        }
        resolve_opencode_executable().ok_or_else(|| HarnessError::NotInstalled(INSTALL_HINT.into()))
    }

    /// Boot (or attach to) a server for a run/probe. Probes have no chat cwd:
    /// they boot in the user's home, where global provider config lives.
    async fn server(
        &self,
        cwd: Option<&str>,
        mcp: Option<&zeron_proto::McpServer>,
    ) -> Result<Server, HarnessError> {
        self.server_with_env(cwd, mcp, &Default::default()).await
    }

    /// [`Self::server`] for a run: the host-resolved run environment
    /// ([`RunRequest::env`]) rides the spawned `opencode serve`.
    async fn server_with_env(
        &self,
        cwd: Option<&str>,
        mcp: Option<&zeron_proto::McpServer>,
        run_env: &std::collections::BTreeMap<String, String>,
    ) -> Result<Server, HarnessError> {
        if let Some(base) = &self.base_url {
            return Ok(Server::attached(base.clone()));
        }
        let exe = self.resolve_executable()?;
        Server::spawn_with_env(&exe, cwd, self.startup_timeout, mcp, run_env).await
    }

    /// One short-lived server answers both discovery calls. Also primes the
    /// commands cache so concurrent picker/composer fetches share one boot.
    async fn probe_models(&self) -> Result<Vec<Model>, HarnessError> {
        let _guard = self.probe_lock.lock().await;
        let mut server = self.server(None, None).await?;
        let result = async {
            let providers = server.provider_catalog(None).await?;
            let mut models = models_from_providers(&providers);
            if server.protocol().await == Protocol::V2 {
                let agents = server.get_json("/api/agent", None).await?;
                let option = agent_option(&agents);
                for model in &mut models {
                    model.options.push(option.clone());
                }
            }
            if models.is_empty() {
                return Err(HarnessError::Protocol(
                    "opencode advertised no models (`opencode auth login` to configure a provider)"
                        .into(),
                ));
            }
            if let Ok(commands) = server.commands_wire(None).await {
                let _ = self.commands_cache.set(commands_from_wire(&commands));
            }
            Ok(models)
        }
        .await;
        server.shutdown(self.kill_grace).await;
        result
    }

    async fn probe_commands(&self) -> Result<Vec<SlashCommand>, HarnessError> {
        let _guard = self.probe_lock.lock().await;
        if let Some(commands) = self.commands_cache.get() {
            return Ok(commands.clone());
        }
        let mut server = self.server(None, None).await?;
        let result = server
            .commands_wire(None)
            .await
            .map(|v| commands_from_wire(&v));
        server.shutdown(self.kill_grace).await;
        result
    }
}

#[async_trait]
impl Harness for OpencodeHarness {
    fn id(&self) -> HarnessId {
        HarnessId::Opencode
    }
    fn display_name(&self) -> &str {
        // Must match the registry's lazy descriptor.
        "OpenCode"
    }
    fn supports_steering(&self) -> bool {
        true
    }
    /// Steers queue and deliver as the next prompt when the live turn goes
    /// idle — opencode has no mid-turn injection on this wire.
    fn steering_mode(&self) -> SteeringMode {
        // Steers preempt the generation (never a running tool) and continue
        // the turn immediately; see `maybe_preempt!`.
        SteeringMode::StepBoundary
    }
    fn reasoning_levels(&self) -> &[ReasoningLevel] {
        REASONING_LEVELS
    }
    fn installed(&self) -> bool {
        self.executable.is_some()
            || self.base_url.is_some()
            || resolve_opencode_executable().is_some()
    }
    fn executable_path(&self) -> Option<PathBuf> {
        self.resolve_executable().ok()
    }
    /// `session.status{idle}` is a real terminal frame per turn: the engine
    /// can retire its quiesce watchdogs.
    fn deterministic_turn_end(&self) -> bool {
        true
    }
    /// Aborting the session ends the turn; the server and the session take
    /// the next prompt.
    fn stops_turn_in_place(&self) -> bool {
        true
    }
    /// The model and effort ride each prompt (1.x) or are set on the
    /// session (2.x), so a change applies from the next prompt without a new
    /// server. Clearing one back to the default cannot be expressed that way.
    fn reconfigures_in_place(&self, live: &RunRequest, next: &RunRequest) -> bool {
        live.cwd == next.cwd
            && (next.model.is_some() || live.model.is_none())
            && (next.reasoning.is_some() || live.reasoning.is_none())
    }

    /// Live discovery off `GET /provider` (what the desktop app populates its
    /// picker from). Account/config changes invalidate the last-good catalog;
    /// explicit refreshes bypass cooldown while overlapping probes coalesce.
    fn model_context(&self) -> Result<Option<crate::ModelContext>, HarnessError> {
        let binary = if let Some(base) = &self.base_url {
            PathBuf::from(base)
        } else {
            self.resolve_executable()?
        };
        crate::model_context::context(self.id(), &binary, &[]).map(Some)
    }
    async fn model_catalog(&self, force: bool) -> Result<crate::ModelCatalog, HarnessError> {
        self.model_context()?.unwrap().log();
        self.models_cache
            .get_with_timeout(
                force,
                self.startup_timeout * 3 + Duration::from_secs(1),
                || self.model_context().map(|c| c.unwrap().key()),
                || self.probe_models(),
            )
            .await
    }
    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        self.model_catalog(true).await.map(|c| c.models)
    }

    async fn skills(
        &self,
        cwd: &std::path::Path,
    ) -> Result<Option<Vec<zeron_proto::invocation::Skill>>, HarnessError> {
        let mut skills = crate::skills::discover(self.id(), cwd).await?;
        let _guard = self.probe_lock.lock().await;
        let directory = cwd
            .to_str()
            .ok_or_else(|| HarnessError::Protocol("Project path is not UTF-8".into()))?;
        let mut server = self.server(Some(directory), None).await?;
        let result = server.commands_wire(Some(directory)).await;
        server.shutdown(self.kill_grace).await;
        let commands = result?;
        merge_skill_commands(&mut skills, &commands);
        Ok(Some(skills))
    }

    async fn commands(&self) -> Result<Vec<SlashCommand>, HarnessError> {
        self.commands_cache
            .get_or_try_init(|| self.probe_commands())
            .await
            .cloned()
    }

    async fn commands_for(&self, cwd: &std::path::Path) -> Result<Vec<SlashCommand>, HarnessError> {
        let _guard = self.probe_lock.lock().await;
        let directory = cwd
            .to_str()
            .ok_or_else(|| HarnessError::Protocol("Project path is not UTF-8".into()))?;
        let mut server = self.server(Some(directory), None).await?;
        let result = server
            .commands_wire(Some(directory))
            .await
            .map(|v| commands_from_wire(&v));
        server.shutdown(self.kill_grace).await;
        result
    }

    async fn run(
        &self,
        mut request: RunRequest,
        controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        // The engine intentionally leaves OpenCode's canonical invocation
        // intact. Capture the selected identity before converting it to the
        // provider's `/command arguments` text.
        let initial_native_command_selected = selected_native_command(&request.prompt, self.id());
        request.prompt = zeron_proto::invocation::harness_prompt(&request.prompt, self.id());
        let cwd = (!request.cwd.is_empty()).then(|| request.cwd.clone());
        let server = self
            .server_with_env(cwd.as_deref(), request.mcp.as_ref(), &request.env)
            .await?;
        let (event_tx, event_rx) = mpsc::channel::<Result<AgentEvent, HarnessError>>(256);
        tokio::spawn(run_session(Session {
            server,
            event_tx,
            controls,
            request,
            interrupt_grace: self.interrupt_grace,
            kill_grace: self.kill_grace,
            bus_timeouts: self.bus_timeouts,
            known_commands: None, // Resolve the live session directory, never a global probe cache.
            initial_native_command_selected,
        }));
        Ok(futures::stream::unfold(event_rx, |mut rx| async move {
            rx.recv().await.map(|ev| (ev, rx))
        })
        .boxed())
    }
}

// ---------------------------------------------------------------------------
// Server process + HTTP plumbing
// ---------------------------------------------------------------------------

struct Server {
    child: Option<Child>,
    base: String,
    /// `Authorization` header value (`Basic <b64>`), when we own the process.
    auth: Option<String>,
    client: reqwest::Client,
    stderr_tail: crate::StderrTail,
    /// Wire generation, resolved once via the health endpoints.
    protocol: tokio::sync::OnceCell<Protocol>,
    version: tokio::sync::OnceCell<ServerVersion>,
    /// Keeps preview discovery off our own server's port while it lives.
    _port: Option<crate::agent_ports::Reservation>,
}

/// The attached server's wire generation: the 1.x "v1" global namespace
/// (`/global/*`, `/session/*`, prompt_async) and the 2.x `/api/*` surface
/// (session-scoped model, prompt with delivery, interrupt).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Protocol {
    V1,
    V2,
}

#[derive(Clone, Debug)]
struct ServerVersion {
    raw: String,
    number: Option<(u64, u64, u64)>,
}

impl ServerVersion {
    fn parse(raw: &str) -> Self {
        let number = (|| {
            let start = raw.find(|c: char| c.is_ascii_digit())?;
            let mut parts = raw[start..].split('.');
            let major = parts.next()?.parse().ok()?;
            let minor = parts.next()?.parse().ok()?;
            let patch = parts
                .next()?
                .split(|c: char| !c.is_ascii_digit())
                .next()?
                .parse()
                .ok()?;
            Some((major, minor, patch))
        })();
        Self {
            raw: raw.to_owned(),
            number,
        }
    }
}

impl Protocol {
    /// Probe newest to oldest. A version-bearing JSON response distinguishes
    /// the API from web UI catch-alls; `None` means the server is still booting.
    async fn detect(server: &Server) -> Result<Option<Self>, HarnessError> {
        Self::detect_preferring(server, None).await
    }

    /// [`Self::detect`], probing `preferred`'s endpoints first. Every probe of
    /// the other generation hits the web UI catch-all (which 1.x proxies
    /// upstream), and while the port is still closed each costs a refused
    /// loopback connect — about 2s on Windows, which retries the SYN.
    async fn detect_preferring(
        server: &Server,
        preferred: Option<Protocol>,
    ) -> Result<Option<Self>, HarnessError> {
        let mut probes = [
            ("/api/info", Protocol::V2),
            ("/api/status", Protocol::V2),
            ("/api/health", Protocol::V2),
            ("/global/health", Protocol::V1),
        ];
        if preferred == Some(Protocol::V1) {
            probes.rotate_right(1);
        }
        for (path, protocol) in probes {
            let Ok(resp) = server.get_raw(path).await else {
                continue;
            };
            if matches!(resp.status().as_u16(), 401 | 403) {
                return Err(HarnessError::Protocol(format!(
                    "opencode authentication rejected at {path}: {}",
                    resp.status()
                )));
            }
            if resp.status().is_success()
                && let Ok(v) = resp.json::<Value>().await
                && let Some(version) = v
                    .get("version")
                    .and_then(Value::as_str)
                    .filter(|v| !v.trim().is_empty())
                    .or_else(|| {
                        v.pointer("/data/version")
                            .and_then(Value::as_str)
                            .filter(|v| !v.trim().is_empty())
                    })
            {
                let _ = server.version.set(ServerVersion::parse(version));
                return Ok(Some(protocol));
            }
        }
        Ok(None)
    }
}

impl Server {
    fn attached(base: String) -> Self {
        Self {
            child: None,
            base: base.trim_end_matches('/').to_owned(),
            auth: None,
            client: http_client(),
            stderr_tail: crate::StderrTail::default(),
            protocol: tokio::sync::OnceCell::new(),
            version: tokio::sync::OnceCell::new(),
            _port: None,
        }
    }

    /// The resolved wire generation: detected against the live server on
    /// first use (spawn seeds the cell during the readiness loop).
    async fn protocol(&self) -> Protocol {
        *self
            .protocol
            .get_or_init(|| async {
                Protocol::detect(self)
                    .await
                    .ok()
                    .flatten()
                    .unwrap_or(Protocol::V1)
            })
            .await
    }

    #[cfg(test)]
    async fn spawn(
        exe: &std::path::Path,
        cwd: Option<&str>,
        startup: Duration,
        mcp: Option<&zeron_proto::McpServer>,
    ) -> Result<Self, HarnessError> {
        Self::spawn_with_env(exe, cwd, startup, mcp, &Default::default()).await
    }

    /// Spawn `opencode serve` on a free loopback port with a per-run Basic
    /// password, and wait for a generation-bearing health answer (which
    /// also resolves [`Protocol`]). `run_env` is the host-resolved run
    /// environment ([`RunRequest::env`]).
    ///
    /// Every opencode process shares one SQLite database; a server booting
    /// while another migrates or checkpoints it can exit with "database is
    /// locked" (more often on Windows, whose file locks are mandatory and
    /// whose malware scan holds new WAL files), and concurrent first starts
    /// race each other's schema migration ([`MigrationRace`]). Those exits
    /// alone are retried, and on a fresh port, which also covers the port
    /// race inherent in handing a released port to the child.
    async fn spawn_with_env(
        exe: &std::path::Path,
        cwd: Option<&str>,
        startup: Duration,
        mcp: Option<&zeron_proto::McpServer>,
        run_env: &std::collections::BTreeMap<String, String>,
    ) -> Result<Self, HarnessError> {
        // Known up front (cached per binary): it picks the MCP config shape
        // and which health endpoints to probe first.
        let version = opencode_version(exe).await;
        let mut attempt = 0;
        loop {
            match Self::spawn_once(exe, cwd, startup, mcp, run_env, version.as_ref()).await {
                Err(SpawnFailure::Retryable(error)) if attempt < SPAWN_RETRIES => {
                    attempt += 1;
                    tracing::warn!(
                        target: "zeron_harness::opencode",
                        attempt,
                        %error,
                        "opencode serve exited at startup on a transient fault; retrying"
                    );
                    tokio::time::sleep(SPAWN_RETRY_DELAY * attempt).await;
                }
                Err(SpawnFailure::Retryable(error) | SpawnFailure::Fatal(error)) => {
                    return Err(error);
                }
                Ok(server) => return Ok(server),
            }
        }
    }

    async fn spawn_once(
        exe: &std::path::Path,
        cwd: Option<&str>,
        startup: Duration,
        mcp: Option<&zeron_proto::McpServer>,
        run_env: &std::collections::BTreeMap<String, String>,
        version: Option<&semver::Version>,
    ) -> Result<Self, SpawnFailure> {
        let fatal = SpawnFailure::Fatal;
        let port = free_localhost_port().ok_or_else(|| {
            fatal(HarnessError::Protocol(
                "no free localhost port for opencode serve".into(),
            ))
        })?;
        let password = uuid::Uuid::new_v4().to_string();
        let preferred = version.map(|v| {
            if v.major >= 2 {
                Protocol::V2
            } else {
                Protocol::V1
            }
        });
        let mut cmd = Command::new(exe);
        cmd.arg("serve")
            .arg("--port")
            .arg(port.to_string())
            .arg("--hostname")
            .arg("127.0.0.1");
        crate::compose_child_path(&mut cmd, exe);
        crate::apply_run_env(&mut cmd, run_env);
        if let Some(mcp) = mcp {
            // The config shape differs by generation. If even the cold probe
            // can't tell, run without the Zeron MCP server rather than fail.
            match preferred {
                Some(protocol) => {
                    // Set after the run env, merging into its inline config
                    // (the user's layer for this run) rather than replacing it.
                    let inherited = run_env
                        .get("OPENCODE_CONFIG_CONTENT")
                        .cloned()
                        .or_else(|| std::env::var("OPENCODE_CONFIG_CONTENT").ok());
                    cmd.env(
                        "OPENCODE_CONFIG_CONTENT",
                        mcp_config(inherited.as_deref(), mcp, protocol).map_err(fatal)?,
                    );
                }
                None => tracing::warn!(
                    binary_path = %exe.display(),
                    "opencode version unknown; starting without the Zeron MCP server"
                ),
            }
        }
        // Ours win over anything inherited: 2.x prefers OPENCODE_PASSWORD
        // over the legacy name, and a user's own value (or username) would
        // otherwise lock us out of our own server (401).
        cmd.env("OPENCODE_PASSWORD", &password)
            .env("OPENCODE_SERVER_PASSWORD", &password)
            .env("OPENCODE_SERVER_USERNAME", "opencode")
            .env("OPENCODE_CLIENT", "zeron");
        // Probes have no project: run them from home, where the global
        // config lives, never the engine's own working directory (which for
        // a GUI or service launch can be System32 or the install folder).
        match cwd {
            Some(cwd) => {
                cmd.current_dir(cwd);
            }
            None => {
                if let Some(home) = crate::executable::home_dir().filter(|h| h.is_dir()) {
                    cmd.current_dir(home);
                }
            }
        }
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = cmd.spawn().map_err(|e| {
            let cwd = cwd.map(std::path::Path::new);
            fatal(crate::executable::spawn_error(e, cwd, || {
                crate::executable::binary_hint(exe)
            }))
        })?;
        let stderr_tail = crate::StderrTail::default();
        // Matched per line: the tail keeps only the last few, and Node prints
        // the EADDRINUSE message above a stack trace that scrolls it out.
        let transient = Arc::new(std::sync::atomic::AtomicBool::new(false));
        if let Some(stderr) = child.stderr.take() {
            let tail = stderr_tail.clone();
            let transient = transient.clone();
            tokio::spawn(async move {
                let mut lines = crate::lossy_lines::lossy_lines(stderr);
                let mut migration = MigrationRace::default();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::debug!(target: "zeron_harness::opencode", "stderr: {line}");
                    if migration.feed(&line)
                        || is_database_locked(&line)
                        || is_address_in_use(&line)
                    {
                        transient.store(true, std::sync::atomic::Ordering::Release);
                    }
                    tail.push(&line);
                }
                tail.close();
            });
        }
        // stdout announces the bound port; it is drained for the process's
        // whole life, since a full pipe would block the server's next write.
        let listening = Arc::new(tokio::sync::Notify::new());
        if let Some(stdout) = child.stdout.take() {
            let listening = listening.clone();
            tokio::spawn(async move {
                let mut lines = crate::lossy_lines::lossy_lines(stdout);
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::debug!(target: "zeron_harness::opencode", "stdout: {line}");
                    if line.to_ascii_lowercase().contains("server listening on") {
                        listening.notify_one();
                    }
                }
            });
        }

        use base64::Engine as _;
        let auth = format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(format!("opencode:{password}"))
        );
        let mut server = Self {
            child: Some(child),
            base: format!("http://127.0.0.1:{port}"),
            auth: Some(auth),
            client: http_client(),
            stderr_tail,
            protocol: tokio::sync::OnceCell::new(),
            version: tokio::sync::OnceCell::new(),
            _port: Some(crate::agent_ports::reserve(port)),
        };

        // Readiness: the server binds a few seconds into the process's life
        // (minutes on plugin-heavy cold starts). An exited child fails fast
        // with its stderr tail instead of burning the whole budget.
        let deadline = tokio::time::Instant::now() + startup;
        loop {
            if let Some(child) = server.child.as_mut()
                && let Ok(Some(status)) = child.try_wait()
            {
                server.stderr_tail.wait_closed().await;
                let error = HarnessError::Protocol(crate::crash_message(
                    "opencode serve",
                    Some(status),
                    &server.stderr_tail,
                ));
                return Err(if transient.load(std::sync::atomic::Ordering::Acquire) {
                    SpawnFailure::Retryable(error)
                } else {
                    fatal(error)
                });
            }
            let detected = match Protocol::detect_preferring(&server, preferred).await {
                Ok(detected) => detected,
                Err(error) => {
                    server.shutdown(Duration::from_secs(1)).await;
                    return Err(fatal(error));
                }
            };
            if let Some(protocol) = detected {
                tracing::debug!(
                    version = server.version.get().map(|v| v.raw.as_str()),
                    "opencode ready"
                );
                let _ = server.protocol.set(protocol);
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                server.shutdown(Duration::from_secs(1)).await;
                let version = version.map_or_else(|| "unknown".to_owned(), |v| v.to_string());
                return Err(fatal(HarnessError::Protocol(format!(
                    "opencode serve (version {version}) did not become healthy within {}s. \
                     If this opencode is older than {TESTED_SINCE}, update it; otherwise \
                     raise {} if this machine's plugin load is genuinely slow",
                    startup.as_secs(),
                    STARTUP_TIMEOUT_ENV,
                ))));
            }
            // The listening line means the port is bound: probe right away
            // rather than finishing the poll interval.
            tokio::select! {
                _ = tokio::time::sleep(HEALTH_POLL) => {}
                _ = listening.notified() => {}
            }
        }
        Ok(server)
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let mut req = self.client.request(method, format!("{}{path}", self.base));
        if let Some(auth) = &self.auth {
            req = req.header(reqwest::header::AUTHORIZATION, auth.clone());
        }
        req
    }

    /// Bounded health probe. opencode's boot window ACCEPTS connections but
    /// parks the request until the app is ready — and a request parked early
    /// enough is never answered at all (observed live, 1.18.18), so an
    /// unbounded send() wedges the whole startup. Abandon and re-poll.
    async fn get_raw(&self, path: &str) -> Result<reqwest::Response, reqwest::Error> {
        self.request(reqwest::Method::GET, path)
            .timeout(Duration::from_secs(2))
            .send()
            .await
    }

    /// GET with the session's directory scope (the server's per-request
    /// instance selector; both carriers set, matching the official SDK).
    async fn get_json(&self, path: &str, directory: Option<&str>) -> Result<Value, HarnessError> {
        self.get_response(path, directory)
            .await?
            .json()
            .await
            .map_err(|e| HarnessError::Protocol(format!("opencode GET {path}: {e}")))
    }

    async fn get<T: serde::de::DeserializeOwned + Send + 'static>(
        &self,
        path: &str,
        directory: Option<&str>,
    ) -> Result<T, HarnessError> {
        let response = self.get_response(path, directory).await?;
        decode_json_response(response)
            .await
            .map_err(|e| HarnessError::Protocol(format!("opencode GET {path}: {e}")))
    }

    async fn get_response(
        &self,
        path: &str,
        directory: Option<&str>,
    ) -> Result<reqwest::Response, HarnessError> {
        let req = self
            .request(reqwest::Method::GET, path)
            .timeout(CALL_TIMEOUT);
        let resp = self
            .scoped(req, directory)
            .await
            .send()
            .await
            .map_err(|e| HarnessError::Protocol(format!("opencode GET {path}: {e}")))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(HarnessError::Protocol(format!(
                "opencode GET {path}: {status} {}",
                truncate_body(&body)
            )));
        }
        Ok(resp)
    }

    async fn post_json(
        &self,
        path: &str,
        directory: Option<&str>,
        body: &Value,
    ) -> Result<Value, HarnessError> {
        let (status, text) = self.post_json_raw(path, directory, body).await?;
        if !status.is_success() {
            return Err(HarnessError::Protocol(post_error_message(
                path, status, &text,
            )));
        }
        Ok(serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    /// POST returning `(status, raw body)`; only transport failures error.
    /// HTTP status handling is the caller's (the lazy-migration retry in
    /// [`create_session`] needs to see 5xx, not a preformatted message).
    async fn post_json_raw(
        &self,
        path: &str,
        directory: Option<&str>,
        body: &Value,
    ) -> Result<(reqwest::StatusCode, String), HarnessError> {
        let req = self
            .request(reqwest::Method::POST, path)
            .timeout(CALL_TIMEOUT)
            .json(body);
        let resp = self
            .scoped(req, directory)
            .await
            .send()
            .await
            .map_err(|e| HarnessError::Protocol(format!("opencode POST {path}: {e}")))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        Ok((status, text))
    }

    /// Directory scope: the server's per-request instance selector. V1 takes
    /// it as a query param (plus header, matching the official SDK); V2's
    /// strict query validation rejects unknown params — there the header
    /// alone scopes the request (verified live, 2.0.3).
    async fn scoped(
        &self,
        mut req: reqwest::RequestBuilder,
        directory: Option<&str>,
    ) -> reqwest::RequestBuilder {
        if let Some(dir) = directory {
            req = req.header("x-opencode-directory", encode_directory(dir));
            if self.protocol().await == Protocol::V1 {
                req = req.query(&[("directory", dir)]);
            }
        }
        req
    }

    async fn shutdown(&mut self, kill_grace: Duration) {
        if let Some(child) = self.child.as_mut() {
            // The server and every command it started.
            crate::shutdown_agent(child, Vec::new(), kill_grace).await;
        }
    }

    /// Session lookup for resume; both wires answer with the info object
    /// (2.x wrapped in `{data}`).
    async fn session_info(
        &self,
        session_id: &str,
        directory: Option<&str>,
    ) -> Result<Value, HarnessError> {
        let path = match self.protocol().await {
            Protocol::V1 => format!("/session/{session_id}"),
            Protocol::V2 => format!("/api/session/{session_id}"),
        };
        let info = self.get_json(&path, directory).await?;
        Ok(unwrap_data(info))
    }

    /// Provider catalog for the picker, variant picking, and context
    /// windows. V1 advertises it on `/provider`; 2.x splits it into a flat
    /// enabled-model list on `/api/model` that folds into the same shape.
    async fn provider_catalog(
        &self,
        directory: Option<&str>,
    ) -> Result<ProviderCatalog, HarnessError> {
        match self.protocol().await {
            Protocol::V1 => self.get("/provider", directory).await,
            Protocol::V2 => {
                // The 2.x catalog syncs from models.dev shortly after the
                // health endpoint opens: an empty list right then is a race,
                // not a fact — poll briefly before believing it.
                for attempt in 0..5 {
                    let list: V2ModelList = self.get("/api/model", directory).await?;
                    if !list.data.is_empty() || attempt == 4 {
                        return Ok(catalog_from_v2_models(list.data));
                    }
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
                unreachable!("loop returns on the last attempt")
            }
        }
    }

    /// Slash commands; both wires carry `{name, description}` entries (2.x
    /// wrapped in `{data}`).
    async fn commands_wire(&self, directory: Option<&str>) -> Result<Value, HarnessError> {
        let path = match self.protocol().await {
            Protocol::V1 => "/command",
            Protocol::V2 => "/api/command",
        };
        let raw = self.get_json(path, directory).await?;
        Ok(unwrap_data(raw))
    }

    /// Is the session mid-turn? V1 keeps a status map on `/session/status`;
    /// V2 lists running sessions on `/api/session/active`. `Err` = can't
    /// tell — the caller treats that as still running.
    async fn session_running(
        &self,
        session_id: &str,
        directory: Option<&str>,
    ) -> Result<bool, HarnessError> {
        let path = match self.protocol().await {
            Protocol::V1 => "/session/status",
            Protocol::V2 => "/api/session/active",
        };
        let map = unwrap_data(self.get_json(path, directory).await?);
        Ok(map
            .get(session_id)
            .is_some_and(|state| state.get("type").and_then(Value::as_str) != Some("idle")))
    }

    /// End the live turn.
    async fn abort_session(
        &self,
        session_id: &str,
        directory: Option<&str>,
    ) -> Result<Value, HarnessError> {
        let path = match self.protocol().await {
            Protocol::V1 => format!("/session/{session_id}/abort"),
            Protocol::V2 => format!("/api/session/{session_id}/interrupt"),
        };
        self.post_json(&path, directory, &Value::Null).await
    }

    /// 2.x: model and reasoning variant ride the SESSION (set once per run),
    /// not the prompt.
    async fn set_model(
        &self,
        session_id: &str,
        provider: &str,
        model: &str,
        variant: Option<&str>,
        directory: Option<&str>,
    ) -> Result<(), HarnessError> {
        let mut model_ref = json!({ "providerID": provider, "id": model });
        if let Some(variant) = variant {
            model_ref["variant"] = json!(variant);
        }
        let path = format!("/api/session/{session_id}/model");
        self.post_json(&path, directory, &json!({ "model": model_ref }))
            .await
            .map(|_| ())
    }

    /// The same server for a detached task (no process ownership).
    fn handle(&self) -> Server {
        Server {
            child: None,
            base: self.base.clone(),
            auth: self.auth.clone(),
            client: self.client.clone(),
            stderr_tail: crate::StderrTail::default(),
            protocol: self.protocol.clone(),
            version: self.version.clone(),
            _port: None,
        }
    }

    /// The process exit, once our own server has died.
    fn exited(&mut self) -> Option<std::process::ExitStatus> {
        self.child.as_mut()?.try_wait().ok().flatten()
    }

    /// Did a prompt whose POST went unanswered reach the session anyway?
    /// Running means it did; so does its message existing under the id we
    /// minted (both wires store the prompt under it).
    async fn prompt_landed(&self, session_id: &str, message_id: &str, dir: Option<&str>) -> bool {
        let bound = Duration::from_secs(5);
        if let Ok(Ok(true)) =
            tokio::time::timeout(bound, self.session_running(session_id, dir)).await
        {
            return true;
        }
        let path = match self.protocol().await {
            Protocol::V1 => format!("/session/{session_id}/message/{message_id}"),
            Protocol::V2 => format!("/api/session/{session_id}/message/{message_id}"),
        };
        match tokio::time::timeout(bound, self.get_json(&path, dir)).await {
            Ok(Ok(message)) => [
                message.pointer("/info/id"),
                message.pointer("/data/id"),
                message.get("id"),
            ]
            .into_iter()
            .flatten()
            .any(|id| id.as_str() == Some(message_id)),
            _ => false,
        }
    }

    /// Requests still waiting on an answer, as bus payloads. The bus never
    /// replays, so a `permission.asked` or question emitted while it was
    /// reconnecting exists only here; unanswered, it blocks the turn for
    /// good. 1.x lists every pending request of the directory; 2.x lists
    /// them per session (forms from 2.0.11). Ownership is checked where the
    /// payloads are handled, like live ones. Best effort: a failed list adds
    /// nothing.
    async fn pending_requests(&self, sessions: &[String], dir: Option<&str>) -> Vec<Value> {
        let mut payloads = Vec::new();
        let listed = |path: String| async move {
            match self.get_json(&path, dir).await {
                Ok(list) => unwrap_data(list).as_array().cloned().unwrap_or_default(),
                Err(e) => {
                    tracing::debug!(
                        target: "zeron_harness::opencode",
                        "pending request sweep: {e}"
                    );
                    Vec::new()
                }
            }
        };
        let asked = |kind: &str, properties: Value| json!({"type": kind, "properties": properties});
        match self.protocol().await {
            Protocol::V1 => {
                for request in listed("/permission".into()).await {
                    payloads.push(asked("permission.asked", request));
                }
                for request in listed("/question".into()).await {
                    payloads.push(asked("question.asked", request));
                }
            }
            Protocol::V2 => {
                for session in sessions {
                    for request in listed(format!("/api/session/{session}/permission")).await {
                        payloads.push(asked("permission.asked", request));
                    }
                    for form in listed(format!("/api/session/{session}/form")).await {
                        payloads.extend(v2_question_form_payload(&form));
                    }
                }
            }
        }
        payloads
    }

    /// Abort `known` subagent sessions after their root was stopped. Only
    /// foreground subagents end with their parent; a background one keeps
    /// running (and spending) on a server a stopped turn leaves up. 1.x also
    /// lists the root's children, covering any whose creation the bus
    /// missed. Bounded; failures (an already-idle or vanished child) are
    /// ignored.
    async fn abort_children(&self, root: &str, mut known: Vec<String>, dir: Option<&str>) {
        let work = async {
            if self.protocol().await == Protocol::V1
                && let Ok(listed) = self
                    .get_json(&format!("/session/{root}/children"), dir)
                    .await
            {
                known.extend(
                    listed
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|child| child.get("id").and_then(Value::as_str))
                        .map(str::to_owned),
                );
            }
            known.sort();
            known.dedup();
            futures::future::join_all(known.iter().map(|child| self.abort_session(child, dir)))
                .await;
        };
        if tokio::time::timeout(CHILD_ABORT_BUDGET, work)
            .await
            .is_err()
        {
            tracing::warn!(
                target: "zeron_harness::opencode",
                "aborting subagent sessions timed out"
            );
        }
    }
}

fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        // Every call goes to our own loopback `opencode serve` and carries its
        // Basic-auth password. reqwest's system/env proxy has no loopback
        // exemption, so without this a machine-wide proxy would receive it.
        .no_proxy()
        .connect_timeout(Duration::from_secs(2))
        // No global timeout: the SSE stream lives for the whole run and
        // sync calls are bounded per call site.
        .build()
        .unwrap_or_default()
}

/// The official SDK sends `x-opencode-directory` percent-encoded. Both
/// generations decode it with `decodeURIComponent`, so a Windows path
/// (`C%3A%5CUsers%5C…`) arrives byte-identical to what the SDK's
/// `encodeURIComponent` produces; `/` is left as is, which decodes the same.
fn encode_directory(dir: &str) -> String {
    percent_encode_path(dir)
}

/// Percent-encode every byte outside the unreserved set, keeping `/`.
fn percent_encode_path(dir: &str) -> String {
    let mut out = String::with_capacity(dir.len());
    for b in dir.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn truncate_body(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.len() <= 300 {
        trimmed.to_owned()
    } else {
        let mut end = 300;
        while end > 0 && !trimmed.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &trimmed[..end])
    }
}

/// POST failure text; 5xx adds where the body's `ref` resolves — opencode's
/// own error bodies say only "Check server logs for details.", which sends
/// users to us instead of the log that holds the actual cause.
fn post_error_message(path: &str, status: reqwest::StatusCode, body: &str) -> String {
    let base = format!("opencode POST {path}: {status} {}", truncate_body(body));
    if status.is_server_error() {
        format!(
            "{base} (server-side fault; the body's ref keys the cause in \
             ~/.local/share/opencode/log/opencode.log)"
        )
    } else {
        base
    }
}

// ---------------------------------------------------------------------------
// Discovery mapping
// ---------------------------------------------------------------------------

/// `GET /provider` → picker models: `{providerID}/{modelID}` ids, effort
/// ladder from the model's reasoning variants.
///
/// Only retain what the picker and run setup consume. `/provider` includes
/// the entire models.dev catalog, with large nested capabilities/config maps.
/// Building and cloning a Value for it measured a 112 MB engine heap peak.
/// Serde skips unknown fields and variant bodies without allocating trees.
#[derive(Debug, Default, serde::Deserialize)]
struct ProviderCatalog {
    all: Option<Vec<Provider>>,
    connected: Option<Vec<String>>,
}

#[derive(Debug, serde::Deserialize)]
struct Provider {
    id: Option<String>,
    name: Option<String>,
    models: Option<std::collections::BTreeMap<String, ProviderModel>>,
}

#[derive(Debug, serde::Deserialize)]
struct ProviderModel {
    #[serde(default)]
    limit: ProviderLimit,
    name: Option<String>,
    variants: Option<std::collections::BTreeMap<String, serde::de::IgnoredAny>>,
}

#[derive(Debug, Default, serde::Deserialize)]
struct ProviderLimit {
    context: Option<u64>,
}

/// Decode large catalogs without collecting a second, full HTTP body first.
/// The blocking parser reads through a 64KiB buffer; network reads stay on
/// Tokio. Dropping the caller also interrupts an outstanding body read.
async fn decode_json_response<T: serde::de::DeserializeOwned + Send + 'static>(
    response: reqwest::Response,
) -> Result<T, String> {
    let cancel = tokio_util::sync::CancellationToken::new();
    let _cancel_on_drop = cancel.clone().drop_guard();
    let stream = response
        .bytes_stream()
        .map(|chunk| chunk.map_err(std::io::Error::other))
        .take_until(cancel.cancelled_owned());
    let reader = tokio_util::io::StreamReader::new(Box::pin(stream));
    let reader = tokio_util::io::SyncIoBridge::new(reader);
    tokio::task::spawn_blocking(move || {
        serde_json::from_reader(std::io::BufReader::with_capacity(64 * 1024, reader))
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Filtered to CONNECTED providers: `all` is the entire models.dev catalog
/// (measured live: 194 providers / 7,203 models, nearly all needing an API
/// key the user hasn't set) — offering it verbatim made the picker slow to
/// open, slow to scroll, and full of models every run of which fails with
/// "Model not found" (field report, v0.2.21). `connected` names exactly
/// the usable set (credentialed + config-declared + the anonymous Zen
/// tier). An absent/empty `connected` (older server) falls back to `all`.
fn models_from_providers(providers: &ProviderCatalog) -> Vec<Model> {
    let connected: std::collections::HashSet<&str> = providers
        .connected
        .iter()
        .flatten()
        .map(String::as_str)
        .collect();
    let mut out = Vec::new();
    for provider in providers.all.iter().flatten() {
        let Some(provider_id) = provider.id.as_deref() else {
            continue;
        };
        if !connected.is_empty() && !connected.contains(provider_id) {
            continue;
        }
        let provider_name = provider.name.as_deref().unwrap_or(provider_id);
        let Some(models) = &provider.models else {
            continue;
        };
        let mut provider_models: Vec<Model> = models
            .iter()
            .map(|(model_id, model)| {
                let label = model.name.as_deref().unwrap_or(model_id).to_owned();
                let mut levels: Vec<ReasoningLevel> = model
                    .variants
                    .as_ref()
                    .map(|variants| {
                        variants
                            .keys()
                            .filter_map(|k| variant_to_level(k))
                            .collect()
                    })
                    .unwrap_or_default();
                levels.sort();
                levels.dedup();
                Model {
                    id: format!("{provider_id}/{model_id}"),
                    label,
                    description: Some(provider_name.to_owned()),
                    reasoning_levels: levels,
                    options: Vec::new(),
                }
            })
            .collect();
        provider_models.sort_by(|a, b| a.label.cmp(&b.label));
        out.extend(provider_models);
    }
    out
}

/// Stored with the models so overlapping discovery calls share the same probe.
fn agent_option(agents: &Value) -> ModelOption {
    let mut choices = vec![ModelOptionChoice {
        id: String::new(),
        label: "Server default".into(),
    }];
    for agent in agents
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if agent.get("hidden").and_then(Value::as_bool) == Some(true)
            || agent.get("mode").and_then(Value::as_str) == Some("subagent")
        {
            continue;
        }
        if let (Some(id), Some(name)) = (
            agent.get("id").and_then(Value::as_str),
            agent.get("name").and_then(Value::as_str),
        ) && !id.is_empty()
        {
            choices.push(ModelOptionChoice {
                id: id.into(),
                label: name.into(),
            });
        }
    }
    ModelOption {
        id: "agent".into(),
        label: "Agent".into(),
        choices,
        default_choice: String::new(),
    }
}

/// OpenCode exposes plugin/configured skills in its native command catalog.
/// Source metadata prevents a command with the same name being misclassified.
fn merge_skill_commands(skills: &mut Vec<zeron_proto::invocation::Skill>, commands: &Value) {
    for command in commands.as_array().into_iter().flatten() {
        if command["source"] != "skill" {
            continue;
        }
        let Some(name) = command["name"]
            .as_str()
            .filter(|name| zeron_proto::invocation::valid_skill_command_name(name))
        else {
            continue;
        };
        if let Some(skill) = skills.iter_mut().find(|skill| skill.name == name) {
            skill.command = Some(zeron_proto::invocation::SkillCommand {
                name: name.into(),
                harness: HarnessId::Opencode,
            });
        } else {
            skills.push(zeron_proto::invocation::Skill {
                name: name.into(),
                path: format!("opencode-skill:{name}"),
                description: command["description"].as_str().unwrap_or_default().into(),
                enabled: true,
                command: Some(zeron_proto::invocation::SkillCommand {
                    name: name.into(),
                    harness: HarnessId::Opencode,
                }),
            });
        }
    }
}

fn commands_from_wire(commands: &Value) -> Vec<SlashCommand> {
    commands
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|c| {
                    let name = c.get("name").and_then(Value::as_str)?;
                    Some(SlashCommand {
                        name: name.to_owned(),
                        description: c
                            .get("description")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                        input_hint: None,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// `GET /api/model` (2.x): `{"location": .., "data": [Model.Info]}`.
#[derive(Debug, serde::Deserialize)]
struct V2ModelList {
    #[serde(default)]
    data: Vec<V2Model>,
}

#[derive(Debug, serde::Deserialize)]
struct V2Model {
    #[serde(rename = "providerID")]
    provider_id: String,
    id: String,
    name: Option<String>,
    #[serde(default)]
    limit: ProviderLimit,
    #[serde(default)]
    variants: Vec<V2Variant>,
    enabled: Option<bool>,
}

/// 2.x carries each variant as `{id, settings}` (settings ignored).
#[derive(Debug, serde::Deserialize)]
struct V2Variant {
    id: String,
}

/// 2.x's flat model list folds into the 1.x provider-catalog shape, so
/// variant picking and context windows stay wire-agnostic downstream.
/// Providers keep their id as display name (`/api/model` names no
/// providers).
fn catalog_from_v2_models(models: Vec<V2Model>) -> ProviderCatalog {
    let mut order: Vec<String> = Vec::new();
    let mut grouped: HashMap<String, Vec<(String, ProviderModel)>> = HashMap::new();
    for model in models {
        if model.enabled == Some(false) {
            continue;
        }
        let provider_id = model.provider_id.clone();
        let entry = grouped.entry(provider_id.clone()).or_insert_with(|| {
            order.push(provider_id);
            Vec::new()
        });
        entry.push((
            model.id.clone(),
            ProviderModel {
                limit: model.limit,
                name: model.name,
                variants: Some(
                    model
                        .variants
                        .into_iter()
                        .map(|v| (v.id, serde::de::IgnoredAny))
                        .collect(),
                ),
            },
        ));
    }
    let all = order
        .into_iter()
        .map(|id| {
            let mut models = grouped.remove(&id).unwrap_or_default();
            models.sort_by(|a, b| a.0.cmp(&b.0));
            Provider {
                id: Some(id.clone()),
                name: Some(id),
                models: Some(models.into_iter().collect()),
            }
        })
        .collect();
    ProviderCatalog {
        all: Some(all),
        connected: None,
    }
}

/// Both wires wrap responses in `{data}` on 2.x only; unwrap when present.
fn unwrap_data(value: Value) -> Value {
    value
        .get("data")
        .cloned()
        .filter(|d| d.is_object() || d.is_array())
        .unwrap_or(value)
}

// ---------------------------------------------------------------------------
// Session
// ---------------------------------------------------------------------------

struct Session {
    server: Server,
    event_tx: mpsc::Sender<Result<AgentEvent, HarnessError>>,
    controls: RunControls,
    request: RunRequest,
    interrupt_grace: Duration,
    kill_grace: Duration,
    bus_timeouts: BusTimeouts,
    known_commands: Option<Vec<SlashCommand>>,
    /// True only when the composer supplied a canonical leading command or
    /// provider-backed skill. Raw slash text deliberately stays false.
    initial_native_command_selected: bool,
}

fn new_message_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Rotate the assistant message id; returns (previous, next).
fn rotate(id: &mut String) -> (String, String) {
    let prev = std::mem::replace(id, new_message_id());
    (prev, id.clone())
}

async fn send(tx: &mpsc::Sender<Result<AgentEvent, HarnessError>>, ev: AgentEvent) -> bool {
    tx.send(Ok(ev)).await.is_ok()
}

/// What the bus reader hands the session loop.
enum BusMsg {
    /// A stream (re)connected and delivered its first frame. The FIRST one
    /// gates the initial prompt: the v1 bus has NO replay, and a
    /// fast-failing turn (bad model id errors in ~200ms) can emit
    /// busy → session.error → idle before a late subscription exists —
    /// observed live (1.18.21), leaving only the stall watchdog. Later ones
    /// mean a reconnect gap that may have swallowed our idle: the loop
    /// re-syncs from `GET /session/status`.
    Connected,
    Event(Value),
    /// The stream is gone past the reconnect budget (or the reader saw the
    /// consumer close).
    Disconnected,
    /// The server is up but no longer serving: the bus went silent and the
    /// server failed its health check, or no frame came within the wedge
    /// budget. Carries the run's error.
    Unresponsive(String),
}

/// Per-part streaming state (dedup between full-part snapshots and deltas).
#[derive(Default)]
struct PartState {
    /// "text" | "reasoning" | "tool".
    kind: String,
    /// Bytes of part text already emitted (snapshots resend the full text).
    emitted: usize,
    tool_started: bool,
    tool_done: bool,
}

/// Streaming state for one opencode session's feed (ours or a child's).
#[derive(Default)]
struct SessionFeed {
    /// messageID → is-assistant (user prompt echoes must not render).
    assistant_messages: HashMap<String, bool>,
    /// Parts whose message ROLE isn't known yet, replayed when it lands.
    pending_parts: Vec<Value>,
    parts: HashMap<String, PartState>,
}

/// A spawned subagent (a `task` tool call on OUR session).
struct ChildRun {
    /// The parent-feed tool part id its traffic is tagged with.
    parent_tool_use_id: String,
    feed: SessionFeed,
    /// Chip settled (tagged Done sent); late traffic drops.
    done: bool,
}

/// A `task` chip awaiting its child session id.
struct PendingSpawn {
    tool_part_id: String,
    description: String,
}

struct TurnState {
    /// A prompt is in flight (busy expected/observed; idle settles it).
    active: bool,
    /// An idle belongs to this prompt only after its busy/retry transition,
    /// or after we explicitly abort it. A trailing idle from the previous
    /// turn must not settle a just-submitted boundary steer.
    idle_ready: bool,
    idle_confirmations: u8,
    status_poll: Option<tokio::time::Instant>,
    status_backoff: Duration,
    /// Bus events about our session seen since the prompt was posted.
    saw_activity: bool,
    /// Renderable content (text/reasoning/tool) seen this turn.
    saw_content: bool,
    /// Terminal error to fold into Done.
    error: Option<String>,
    /// Provider-retry chip already surfaced this turn.
    retry_reported: bool,
    /// This turn was aborted because the provider retry loop hit the cap.
    aborted_for_retry: bool,
    /// Deadline for the first session-scoped event after the prompt.
    stall_deadline: Option<tokio::time::Instant>,
    /// The prompt POST has not been answered yet. The stall clock waits for
    /// it: a server slow to admit a prompt (a contended database on Windows)
    /// has not yet had the chance to make progress, and the POST is bounded
    /// on its own.
    prompt_in_flight: bool,
    /// Main-session tool calls started and not yet finished.
    open_tools: std::collections::HashSet<String>,
    /// Aborted to deliver a steer immediately: its idle/interrupted frame is
    /// a steer boundary, not the end of the run.
    preempted: bool,
}

/// What a detached turn request reports back. `generation` binds it to the
/// turn that launched it: the synchronous command endpoint (or a slow
/// prompt, or a reply the user took minutes to give) can answer after the
/// event bus has already settled that turn and started a queued successor.
struct TurnUpdate {
    generation: u64,
    signal: TurnSignal,
}

enum TurnSignal {
    /// The server took the prompt: the stall clock starts now.
    PromptAccepted,
    /// The prompt never reached the server's queue.
    PromptFailed(String),
    /// The native command endpoint answered with an error.
    CommandFailed(String),
    /// A permission or question answer could not be delivered: the agent is
    /// blocked on a reply it will never get.
    ReplyFailed(String),
}

/// A canonical leading command or provider-backed skill was selected in the
/// composer. Raw slash text is deliberately excluded: OpenCode's command set
/// is live, project-scoped state and cannot be inferred by static preflight.
fn selected_native_command(prompt: &str, harness: HarnessId) -> bool {
    let delivered = zeron_proto::invocation::harness_prompt(prompt, harness);
    if zeron_proto::invocation::leading_command(&delivered).is_none() {
        return false;
    }
    zeron_proto::invocation::invocation_links(prompt)
        .into_iter()
        .any(|(range, invocation)| {
            let selected_for_harness = match invocation {
                zeron_proto::invocation::Invocation::Command { .. } => true,
                zeron_proto::invocation::Invocation::Skill {
                    command: Some(command),
                    ..
                } => command.harness == harness,
                _ => false,
            };
            selected_for_harness
                && prompt[..range.start]
                    .trim_matches([' ', '\t', '\r', '\n'])
                    .is_empty()
        })
}

impl TurnState {
    /// `prompt_in_flight`: [`post_prompt`] posted a prompt whose acceptance
    /// will be reported (a native command reports only failure).
    fn begin(stall: Option<Duration>, prompt_in_flight: bool) -> Self {
        Self {
            prompt_in_flight,
            active: true,
            idle_ready: false,
            idle_confirmations: 0,
            status_poll: None,
            status_backoff: Duration::from_millis(100),
            saw_activity: false,
            saw_content: false,
            error: None,
            retry_reported: false,
            aborted_for_retry: false,
            stall_deadline: stall.map(|d| tokio::time::Instant::now() + d),
            open_tools: Default::default(),
            preempted: false,
        }
    }

    fn note_activity(&mut self) {
        self.saw_activity = true;
        self.stall_deadline = None;
    }
}

async fn run_session(session: Session) {
    let Session {
        mut server,
        event_tx,
        controls,
        request,
        interrupt_grace,
        kill_grace,
        bus_timeouts,
        known_commands,
        initial_native_command_selected,
    } = session;
    let RunControls {
        realtime: _,
        execution_lease: _execution_lease,
        request_input,
        mut steering,
        interrupt,
        turn: turn_control,
    } = controls;
    let request_input = Arc::new(request_input);
    let directory = (!request.cwd.is_empty()).then(|| request.cwd.clone());
    let dir = directory.as_deref();

    let agent = request
        .model_options
        .get("agent")
        .and_then(Value::as_str)
        .filter(|a| !a.is_empty());

    // ---- session create/resume -------------------------------------------
    let setup = async {
        let session_id = match &request.resume {
            Some(resume) => {
                // Sessions are durable server-side: resume = reuse the id.
                match server.session_info(resume, dir).await {
                    Ok(info) => {
                        let id = info
                            .get("id")
                            .and_then(Value::as_str)
                            .unwrap_or(resume)
                            .to_owned();
                        if server.protocol().await == Protocol::V2
                            && let Some(agent) = agent
                        {
                            server
                                .post_json(
                                    &format!("/api/session/{id}/agent"),
                                    dir,
                                    &json!({"agent": agent}),
                                )
                                .await?;
                        }
                        id
                    }
                    Err(e) => {
                        tracing::debug!(
                            target: "zeron_harness::opencode",
                            "session resume failed (starting fresh): {e}"
                        );
                        create_session(&server, dir, agent).await?
                    }
                }
            }
            None => create_session(&server, dir, agent).await?,
        };

        // Provider catalog: resolves the model's advertised reasoning
        // variants so the requested effort only rides models that have it.
        let providers = server.provider_catalog(dir).await.unwrap_or_default();

        // 2.x: the requested model (+ variant) is set on the session once;
        // prompts carry only text and files.
        if server.protocol().await == Protocol::V2
            && let Some((provider, model_id)) = request
                .model
                .as_deref()
                .and_then(|m| m.split_once('/'))
                .map(|(p, m)| (p.to_owned(), m.to_owned()))
        {
            let variant = pick_variant(&providers, &provider, &model_id, request.reasoning);
            server
                .set_model(&session_id, &provider, &model_id, variant.as_deref(), dir)
                .await?;
        }
        Ok::<(String, ProviderCatalog), HarnessError>((session_id, providers))
    };
    let (session_id, providers) = tokio::select! {
        res = setup => match res {
            Ok(v) => v,
            Err(e) => {
                let _ = send(&event_tx, AgentEvent::Done {
                    status: DoneStatus::Errored,
                    result: None,
                    error: Some(e.to_string()),
                    session_id: None,
                }).await;
                server.shutdown(kill_grace).await;
                return;
            }
        },
        _ = interrupt.cancelled() => {
            let _ = send(&event_tx, AgentEvent::Done {
                status: DoneStatus::Interrupted,
                result: None,
                error: None,
                session_id: None,
            }).await;
            server.shutdown(kill_grace).await;
            return;
        }
    };

    let (mut model, mut variant) = model_selection(&providers, &request);
    let context_windows: HashMap<String, u64> = providers
        .all
        .iter()
        .flatten()
        .flat_map(|provider| {
            provider
                .models
                .iter()
                .flat_map(|models| models.iter())
                .filter_map(|(id, model)| {
                    Some((
                        format!("{}/{}", provider.id.as_deref()?, id),
                        model.limit.context.filter(|n| *n > 0)?,
                    ))
                })
        })
        .collect();
    drop(providers);

    let mut assistant_message_id = new_message_id();
    if !send(
        &event_tx,
        AgentEvent::SessionStarted {
            harness: HarnessId::Opencode,
            model: request.model.clone().unwrap_or_default(),
            tools: Vec::new(),
            cwd: request.cwd.clone(),
            session_id: session_id.clone(),
            assistant_message_id: assistant_message_id.clone(),
        },
    )
    .await
    {
        server.shutdown(kill_grace).await;
        return;
    }

    // Advertise slash commands (composer popup); a warm cache skips the call.
    let commands = match known_commands {
        Some(commands) => commands,
        None => server
            .commands_wire(dir)
            .await
            .map(|v| commands_from_wire(&v))
            .unwrap_or_default(),
    };
    if !commands.is_empty()
        && !send(
            &event_tx,
            AgentEvent::AvailableCommands {
                commands: commands.clone(),
            },
        )
        .await
    {
        server.shutdown(kill_grace).await;
        return;
    }

    // ---- SSE bus ----------------------------------------------------------
    let (bus_tx, mut bus_rx) = mpsc::channel::<BusMsg>(256);
    let bus_handle = tokio::spawn(bus_task(
        server.base.clone(),
        server.auth.clone(),
        server.protocol().await,
        bus_tx.clone(),
        bus_timeouts,
    ));

    // ---- first prompt -----------------------------------------------------
    // The bus has no replay: wait for the subscription to be LIVE before
    // prompting, or a fast-failing turn's whole lifecycle can slip into the
    // gap (observed live: busy → error → idle inside ~200ms). Bounded — the
    // stall watchdog still guards a bus that never comes up.
    let connect_wait = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            match bus_rx.recv().await {
                Some(BusMsg::Connected) | None => return,
                // Nothing else can arrive before Connected; drop defensively.
                Some(_) => {}
            }
        }
    })
    .await;
    if connect_wait.is_err() {
        tracing::debug!(
            target: "zeron_harness::opencode",
            "event bus not connected within 15s; prompting anyway"
        );
    }
    let stall = stall_bound();
    let (updates_tx, mut updates_rx) = mpsc::unbounded_channel::<TurnUpdate>();
    let mut turn_generation = 0_u64;
    // The live prompt's delivery: a stop or interrupt cancels it, so a
    // retry can never post the stopped prompt again.
    let mut delivery = tokio_util::sync::CancellationToken::new();
    let first_post = post_prompt(
        &server,
        &session_id,
        dir,
        &commands,
        &request.prompt,
        initial_native_command_selected,
        turn_generation,
        &updates_tx,
        &delivery,
        TurnSpec {
            model: model.as_ref(),
            variant: variant.as_deref(),
            attachments: &request.attachments,
        },
    )
    .await;
    let awaiting_ack = match first_post {
        Ok(awaiting_ack) => awaiting_ack,
        Err(e) => {
            let _ = send(
                &event_tx,
                AgentEvent::Error {
                    message: e.to_string(),
                },
            )
            .await;
            let _ = send(
                &event_tx,
                AgentEvent::Done {
                    status: DoneStatus::Errored,
                    result: None,
                    error: Some(e.to_string()),
                    session_id: Some(session_id.clone()),
                },
            )
            .await;
            bus_handle.abort();
            server.shutdown(kill_grace).await;
            return;
        }
    };
    let mut turn = TurnState::begin(stall, awaiting_ack);
    let mut server_poll = tokio::time::interval(SERVER_POLL);
    server_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // Permission and question ids already handled: a reconnect's sweep
    // re-lists requests the live bus may also have delivered.
    let mut seen_requests: std::collections::HashSet<String> = Default::default();

    // ---- main loop --------------------------------------------------------
    let mut main_feed = SessionFeed::default();
    let mut children: HashMap<String, ChildRun> = HashMap::new();
    let mut pending_spawns: VecDeque<PendingSpawn> = VecDeque::new();
    // Child sessions created before their spawn chip was seen (id → title).
    let mut unbound_children: HashMap<String, String> = HashMap::new();
    let mut queued_steers: VecDeque<QueuedSteer> = VecDeque::new();
    let mut steering_open = true;
    let mut interrupt_requested = false;
    // A turn stop (`TurnControl::stop_turn`) in flight: the session is
    // aborted like an interrupt, but the server and session stay up.
    let mut stopping = false;
    // A stop the server never acknowledged: the turn may still be running,
    // so the runtime is torn down and the stop reported only once it is gone.
    let mut stop_teardown = false;
    let mut pending_usage: Option<AgentEvent> = None;
    let mut done_sent = false;

    // Post-abort grace: the abort endpoint promised an idle; if it never
    // arrives the run hard-stops. Unlike the stall bound this is NOT
    // disarmed by activity — only idle ends an abort.
    let mut abort_deadline: Option<tokio::time::Instant> = None;

    // Idle settlement, shared by the idle bus events and the post-reconnect
    // status re-sync (a macro so `break`/`continue` act on the caller's
    // loop): emit held usage, then Interrupted / next queued steer /
    // AssistantMessageCompleted + Done.
    macro_rules! settle_idle {
        ($label:lifetime) => {{
            if !turn.active {
                continue $label;
            }
            turn.active = false;
            if let Some(usage) = pending_usage.take()
                && !interrupt_requested
                && !stopping
                && !send(&event_tx, usage).await
            {
                break $label;
            }
            if interrupt_requested || stopping {
                settle_children(&mut children, &event_tx, DoneStatus::Interrupted).await;
                let _ = send(&event_tx, AgentEvent::Done {
                    status: DoneStatus::Interrupted,
                    result: None,
                    error: None,
                    session_id: Some(session_id.clone()),
                }).await;
                done_sent = true;
                if interrupt_requested {
                    break $label;
                }
                // A stopped turn: the server and session take the next prompt.
                stopping = false;
                abort_deadline = None;
                continue $label;
            }
            if let Some(first) = queued_steers.pop_front() {
                turn_generation = turn_generation.wrapping_add(1);
                let native_command_selected = first.native;
                let config = first.config;
                let mut attachments = first.attachments;
                // Plain-text steers waiting together go out as one prompt,
                // each confirmed by its own Steered boundary; a native
                // command always travels alone, and a changed configuration
                // starts a new prompt (the ones after it were sent with it).
                let mut texts = vec![first.text];
                while !native_command_selected
                    && queued_steers
                        .front()
                        .is_some_and(|next| !next.native && next.config.is_none())
                {
                    let next = queued_steers.pop_front().expect("front checked");
                    texts.push(next.text);
                    attachments.extend(next.attachments);
                }
                let mut consumer_gone = false;
                for _ in &texts {
                    let (prev, next) = rotate(&mut assistant_message_id);
                    if !send(&event_tx, AgentEvent::Steered {
                        assistant_message_id: Some(prev),
                        next_assistant_message_id: Some(next),
                    }).await {
                        consumer_gone = true;
                        break;
                    }
                }
                if consumer_gone {
                    break $label;
                }
                let steer = texts.join("\n\n");
                let adopted = match config {
                    Some(next) => adopt_config(&server, &session_id, dir, &next)
                        .await
                        .map(|selection| (model, variant) = selection),
                    None => Ok(()),
                };
                delivery = tokio_util::sync::CancellationToken::new();
                let sent = match adopted {
                    Ok(()) => post_prompt(
                        &server,
                        &session_id,
                        dir,
                        &commands,
                        &steer,
                        native_command_selected,
                        turn_generation,
                        &updates_tx,
                        &delivery,
                        TurnSpec {
                            model: model.as_ref(),
                            variant: variant.as_deref(),
                            attachments: &attachments,
                        },
                    )
                    .await,
                    Err(e) => Err(e),
                };
                match sent {
                    Ok(awaiting_ack) => {
                        turn = TurnState::begin(stall, awaiting_ack);
                        continue $label;
                    }
                    Err(e) => {
                        let _ = send(&event_tx, AgentEvent::Error {
                            message: e.to_string(),
                        }).await;
                        turn.error = Some(e.to_string());
                        turn.aborted_for_retry = true;
                        // Fall through to Done below.
                    }
                }
            }
            let (prev, _next) = rotate(&mut assistant_message_id);
            if !send(&event_tx, AgentEvent::AssistantMessageCompleted {
                assistant_message_id: prev,
            }).await {
                break $label;
            }
            let errored = turn.aborted_for_retry
                || (turn.error.is_some() && !turn.saw_content);
            let _ = send(&event_tx, AgentEvent::Done {
                status: if errored {
                    DoneStatus::Errored
                } else {
                    DoneStatus::Completed
                },
                result: None,
                error: if errored { turn.error.clone() } else { None },
                session_id: Some(session_id.clone()),
            }).await;
            done_sent = true;
            if errored || !steering_open {
                break $label;
            }
            // Keep the mailbox and server alive between successful turns.
            // Closing here races the engine's next queued dispatch: it may
            // accept a prompt into a dying mailbox and replay it out of order.
            continue $label;
        }};
    }

    // Immediate steering: a queued steer aborts the current generation as
    // soon as no tool is running (a running tool is never killed), and the
    // abort's idle promotes the steer — the way Codex `turn/steer` behaves.
    macro_rules! maybe_preempt {
        () => {{
            if turn.active
                && !turn.preempted
                && !interrupt_requested
                && !stopping
                && !queued_steers.is_empty()
                && turn.open_tools.is_empty()
            {
                turn.preempted = true;
                turn.idle_ready = true;
                let abort = tokio::time::timeout(
                    Duration::from_secs(5),
                    server.abort_session(&session_id, dir),
                )
                .await;
                if !matches!(abort, Ok(Ok(_))) {
                    // Deliver at the natural turn end instead.
                    tracing::warn!(
                        target: "zeron_harness::opencode",
                        "steer preempt abort failed; delivering at turn end"
                    );
                }
            }
        }};
    }

    // The server is gone: it exited (`Some` status), or its bus could not be
    // revived. Settle everything still open — the turn as errored unless it
    // was being interrupted or stopped — and end the run, so the next
    // message starts a fresh server rather than failing against a dead
    // port. Between turns the Done still carries the crash message, never a
    // later transport error.
    macro_rules! settle_lost_server {
        ($label:lifetime, message: $message:expr) => {{
            let message: String = $message;
            if turn.active {
                let _ = send(
                    &event_tx,
                    AgentEvent::Error {
                        message: message.clone(),
                    },
                )
                .await;
            }
            settle_children(&mut children, &event_tx, DoneStatus::Interrupted).await;
            let halted = interrupt_requested || stopping;
            let _ = send(
                &event_tx,
                AgentEvent::Done {
                    status: if halted {
                        DoneStatus::Interrupted
                    } else {
                        DoneStatus::Errored
                    },
                    result: None,
                    error: (!halted).then_some(message),
                    session_id: Some(session_id.clone()),
                },
            )
            .await;
            done_sent = true;
            break $label;
        }};
        ($label:lifetime, $status:expr) => {{
            let message = crate::crash_message("opencode serve", $status, &server.stderr_tail);
            settle_lost_server!($label, message: message)
        }};
    }

    // Route one bus payload through the turn engine and act on the outcome.
    macro_rules! dispatch_bus_event {
        ($label:lifetime, $event:expr) => {{
            let event: Value = $event;
            let outcome = handle_bus_event(BusCtx {
                event: &event,
                session_id: &session_id,
                server: &server,
                dir,
                event_tx: &event_tx,
                request_input: &request_input,
                main_feed: &mut main_feed,
                children: &mut children,
                pending_spawns: &mut pending_spawns,
                unbound_children: &mut unbound_children,
                turn: &mut turn,
                pending_usage: &mut pending_usage,
                context_windows: &context_windows,
                seen_requests: &mut seen_requests,
                generation: turn_generation,
                updates_tx: &updates_tx,
            })
            .await;
            match outcome {
                BusOutcome::Continue => maybe_preempt!(),
                BusOutcome::ConsumerGone => break $label,
                BusOutcome::TurnIdle => settle_idle!($label),
                // Our own steer preempt: a steer boundary.
                BusOutcome::TurnInterrupted
                    if turn.preempted && !interrupt_requested && !stopping =>
                {
                    settle_idle!($label)
                }
                BusOutcome::TurnInterrupted => {
                    interrupt_requested = true;
                    settle_idle!($label);
                }
            }
        }};
    }

    // After the root is aborted (stop or interrupt), its subagent sessions
    // go too; detached, so the turn settles on the root's own idle.
    macro_rules! abort_children {
        () => {{
            let known: Vec<String> = children
                .keys()
                .chain(unbound_children.keys())
                .cloned()
                .collect();
            let handle = server.handle();
            let root = session_id.clone();
            let dir = directory.clone();
            tokio::spawn(async move {
                handle.abort_children(&root, known, dir.as_deref()).await;
            });
        }};
    }

    'main: loop {
        // The stall watchdog only arms while a turn awaits its first sign of
        // life; a running tool's silence never trips it (events already
        // proved the turn alive and disarmed it). Computed by value each
        // iteration so the future doesn't hold a borrow of `turn`.
        let deadline = abort_deadline.or_else(|| {
            (turn.active && !turn.saw_activity && !turn.prompt_in_flight)
                .then_some(turn.stall_deadline)
                .flatten()
        });
        let stall_sleep = async {
            match deadline {
                Some(deadline) => tokio::time::sleep_until(deadline).await,
                None => std::future::pending().await,
            }
        };

        tokio::select! {
            biased;

            _ = event_tx.closed() => break 'main,

            // End the turn, not the server: abort the session's generation
            // and keep both up for the next prompt. Steers still waiting for
            // this turn's end go with it.
            _ = turn_control.stop_requested(), if !interrupt_requested => {
                queued_steers.clear();
                if turn.active && !stopping {
                    stopping = true;
                    delivery.cancel();
                    turn.idle_ready = true;
                    let abort = tokio::time::timeout(
                        Duration::from_secs(5),
                        server.abort_session(&session_id, dir),
                    )
                    .await;
                    if matches!(abort, Ok(Ok(_))) {
                        abort_deadline = Some(tokio::time::Instant::now() + interrupt_grace);
                        abort_children!();
                    } else {
                        // The turn may still be running: this session must
                        // never take a prompt on top of it.
                        tracing::warn!(
                            target: "zeron_harness::opencode",
                            "turn stop abort failed; tearing the runtime down"
                        );
                        stop_teardown = true;
                        break 'main;
                    }
                } else if !turn.active {
                    // Between turns: nothing to abort. Settle the stop so the
                    // host sees the turn over.
                    let _ = send(&event_tx, AgentEvent::Done {
                        status: DoneStatus::Interrupted,
                        result: None,
                        error: None,
                        session_id: Some(session_id.clone()),
                    }).await;
                    done_sent = true;
                }
            }

            _ = interrupt.cancelled(), if !interrupt_requested => {
                interrupt_requested = true;
                delivery.cancel();
                if turn.active {
                    turn.idle_ready = true;
                    let abort = tokio::time::timeout(
                        Duration::from_secs(5),
                        server.abort_session(&session_id, dir),
                    )
                    .await;
                    if !matches!(abort, Ok(Ok(_))) {
                        // The abort endpoint failing means the server itself
                        // is wedged: settle now and tear down hard.
                        settle_children(&mut children, &event_tx, DoneStatus::Interrupted).await;
                        let _ = send(&event_tx, AgentEvent::Done {
                            status: DoneStatus::Interrupted,
                            result: None,
                            error: None,
                            session_id: Some(session_id.clone()),
                        }).await;
                        done_sent = true;
                        break 'main;
                    }
                    // Abort emits session.status{idle} (with or without a
                    // live runner) — the settle path below finishes up. The
                    // grace guards a server that never delivers it.
                    abort_deadline = Some(tokio::time::Instant::now() + interrupt_grace);
                    abort_children!();
                } else {
                    let _ = send(&event_tx, AgentEvent::Done {
                        status: DoneStatus::Interrupted,
                        result: None,
                        error: None,
                        session_id: Some(session_id.clone()),
                    }).await;
                    done_sent = true;
                    break 'main;
                }
            }

            update = updates_rx.recv() => {
                let Some(update) = update else { continue 'main; };
                if update.generation != turn_generation || !turn.active || interrupt_requested || stopping {
                    tracing::debug!(
                        target: "zeron_harness::opencode",
                        update_generation = update.generation,
                        active_generation = turn_generation,
                        "ignoring a request outcome from a retired turn"
                    );
                    continue 'main;
                }
                let message = match update.signal {
                    TurnSignal::PromptAccepted => {
                        turn.prompt_in_flight = false;
                        if !turn.saw_activity {
                            turn.stall_deadline = stall.map(|d| tokio::time::Instant::now() + d);
                        }
                        continue 'main;
                    }
                    // Its POST failed, but the bus already shows the turn
                    // running: the server took the prompt after all.
                    TurnSignal::PromptFailed(message) if turn.idle_ready => {
                        tracing::warn!(
                            target: "zeron_harness::opencode",
                            "prompt POST failed but the turn is running: {message}"
                        );
                        turn.prompt_in_flight = false;
                        continue 'main;
                    }
                    TurnSignal::PromptFailed(message) | TurnSignal::CommandFailed(message) => message,
                    TurnSignal::ReplyFailed(message) => {
                        // Whatever waits on the reply would never resume.
                        let _ = tokio::time::timeout(
                            Duration::from_secs(5),
                            server.abort_session(&session_id, dir),
                        )
                        .await;
                        message
                    }
                };
                // A request that failed because the server died reports the
                // crash, not the transport error it caused.
                if let Some(status) = server.exited() {
                    server.stderr_tail.wait_closed().await;
                    settle_lost_server!('main, Some(status));
                }
                settle_children(&mut children, &event_tx, DoneStatus::Interrupted).await;
                let _ = send(
                    &event_tx,
                    AgentEvent::Error {
                        message: message.clone(),
                    },
                )
                .await;
                let _ = send(
                    &event_tx,
                    AgentEvent::Done {
                        status: DoneStatus::Errored,
                        result: None,
                        error: Some(message),
                        session_id: Some(session_id.clone()),
                    },
                )
                .await;
                done_sent = true;
                break 'main;
            }

            steer = steering.recv(), if steering_open => {
                match steer {
                    Some(steer) => {
                        let native_command_selected =
                            selected_native_command(&steer.prompt, HarnessId::Opencode);
                        let prompt = zeron_proto::invocation::harness_prompt(
                            &steer.prompt,
                            HarnessId::Opencode,
                        );
                        if turn.active {
                            queued_steers.push_back(QueuedSteer {
                                text: prompt,
                                native: native_command_selected,
                                attachments: steer.attachments,
                                config: steer.config,
                            });
                            maybe_preempt!();
                        } else {
                            // A server that died since the last turn reports
                            // its crash, not this prompt's refused connect.
                            if let Some(status) = server.exited() {
                                server.stderr_tail.wait_closed().await;
                                settle_lost_server!('main, Some(status));
                            }
                            turn_generation = turn_generation.wrapping_add(1);
                            // Between turns: the engine routes the next message
                            // of a parked session here — images included.
                            let (prev, next) = rotate(&mut assistant_message_id);
                            let _ = send(&event_tx, AgentEvent::Steered {
                                assistant_message_id: Some(prev),
                                next_assistant_message_id: Some(next),
                            }).await;
                            let adopted = match &steer.config {
                                Some(next) => adopt_config(&server, &session_id, dir, next)
                                    .await
                                    .map(|selection| (model, variant) = selection),
                                None => Ok(()),
                            };
                            delivery = tokio_util::sync::CancellationToken::new();
                            let sent = match adopted {
                                Ok(()) => post_prompt(
                                    &server,
                                    &session_id,
                                    dir,
                                    &commands,
                                    &prompt,
                                    native_command_selected,
                                    turn_generation,
                                    &updates_tx,
                                    &delivery,
                                    TurnSpec {
                                        model: model.as_ref(),
                                        variant: variant.as_deref(),
                                        attachments: &steer.attachments,
                                    },
                                )
                                .await,
                                Err(e) => Err(e),
                            };
                            match sent {
                                Ok(awaiting_ack) => turn = TurnState::begin(stall, awaiting_ack),
                                Err(error) => {
                                    let message = error.to_string();
                                    let _ = send(
                                        &event_tx,
                                        AgentEvent::Error {
                                            message: message.clone(),
                                        },
                                    )
                                    .await;
                                    let _ = send(
                                        &event_tx,
                                        AgentEvent::Done {
                                            status: DoneStatus::Errored,
                                            result: None,
                                            error: Some(message),
                                            session_id: Some(session_id.clone()),
                                        },
                                    )
                                    .await;
                                    done_sent = true;
                                    break 'main;
                                }
                            }
                        }
                    }
                    None => {
                        steering_open = false;
                        if !turn.active { break 'main; }
                    },
                }
            }

            _ = stall_sleep => {
                if abort_deadline.is_some() && stopping {
                    // The stop's abort acknowledged nothing within the grace:
                    // the turn may still be running. Tear the runtime down.
                    tracing::warn!(
                        target: "zeron_harness::opencode",
                        "turn stop unacknowledged; tearing the runtime down"
                    );
                    stop_teardown = true;
                    break 'main;
                }
                if abort_deadline.is_some() {
                    // Abort acknowledged nothing within the grace: hard stop.
                    settle_children(&mut children, &event_tx, DoneStatus::Interrupted).await;
                    let _ = send(&event_tx, AgentEvent::Done {
                        status: DoneStatus::Interrupted,
                        result: None,
                        error: None,
                        session_id: Some(session_id.clone()),
                    }).await;
                    done_sent = true;
                    break 'main;
                }
                let message = format!(
                    "opencode made no progress for {}s after the prompt. {STALL_HINT}",
                    stall.unwrap_or(DEFAULT_STALL_BOUND).as_secs()
                );
                let _ = send(&event_tx, AgentEvent::Error { message: message.clone() }).await;
                let _ = tokio::time::timeout(
                    Duration::from_secs(5),
                    server.abort_session(&session_id, dir),
                )
                .await;
                settle_children(&mut children, &event_tx, DoneStatus::Interrupted).await;
                let _ = send(&event_tx, AgentEvent::Done {
                    status: DoneStatus::Errored,
                    result: None,
                    error: Some(message),
                    session_id: Some(session_id.clone()),
                }).await;
                done_sent = true;
                break 'main;
            }

            _ = tokio::time::sleep_until(turn.status_poll.unwrap_or_else(tokio::time::Instant::now)),
                if turn.status_poll.is_some() && turn.active => {
                match tokio::time::timeout(Duration::from_secs(2), server.session_running(&session_id, dir)).await {
                    Ok(Ok(false)) => settle_idle!('main),
                    _ => {
                        turn.status_backoff = (turn.status_backoff * 2).min(Duration::from_secs(2));
                        turn.status_poll = Some(tokio::time::Instant::now() + turn.status_backoff);
                    }
                }
            }

            msg = bus_rx.recv() => {
                let Some(msg) = msg else { break 'main };
                match msg {
                    BusMsg::Connected => {
                        // A RECONNECT may have swallowed frames (no replay).
                        // Requests asked in the gap are re-listed and handled
                        // like live ones (the agent waits on them), then the
                        // turn re-syncs from the server's own status surface
                        // — not running means idle. Can't tell: leave the
                        // turn running; the next disconnect or event decides.
                        if !interrupt_requested && !stopping {
                            let sessions: Vec<String> = std::iter::once(session_id.clone())
                                .chain(children.keys().cloned())
                                .chain(unbound_children.keys().cloned())
                                .collect();
                            let pending = tokio::time::timeout(
                                Duration::from_secs(10),
                                server.pending_requests(&sessions, dir),
                            )
                            .await
                            .unwrap_or_default();
                            for event in pending {
                                dispatch_bus_event!('main, event);
                            }
                        }
                        // A prompt the server has not answered yet may simply
                        // not be queued yet: not running proves nothing.
                        if turn.active
                            && !turn.prompt_in_flight
                            && !server
                                .session_running(&session_id, dir)
                                .await
                                .unwrap_or(true)
                        {
                            settle_idle!('main);
                        }
                    }
                    BusMsg::Disconnected => {
                        let crashed = server.exited();
                        if crashed.is_some() {
                            server.stderr_tail.wait_closed().await;
                        }
                        settle_lost_server!('main, crashed);
                    }
                    BusMsg::Unresponsive(message) => {
                        // A server that died meanwhile reports its crash.
                        if let Some(status) = server.exited() {
                            server.stderr_tail.wait_closed().await;
                            settle_lost_server!('main, Some(status));
                        }
                        tracing::warn!(target: "zeron_harness::opencode", "{message}");
                        // Ending the run tears the server down.
                        settle_lost_server!('main, message: message);
                    }
                    BusMsg::Event(event) => {
                        if interrupt_requested || stopping {
                            // Only the terminal idle/interrupt acknowledgement may
                            // affect an aborted turn; discard late content and usage.
                            // v1's /global/event wraps it ({payload}), like
                            // `handle_bus_event` reads: matching the envelope
                            // missed every acknowledgement, and each stop
                            // waited out the grace.
                            let payload = event.get("payload").unwrap_or(&event);
                            let kind = payload.get("type").and_then(Value::as_str);
                            let ours = payload.pointer("/properties/sessionID").and_then(Value::as_str) == Some(session_id.as_str());
                            let idle = kind == Some("session.idle") || kind == Some("session.interrupted")
                                || (kind == Some("session.status") && payload.pointer("/properties/status/type").and_then(Value::as_str) == Some("idle"));
                            if ours && idle { settle_idle!('main); }
                            continue;
                        }
                        dispatch_bus_event!('main, event);
                    }
                }
            }

            // Last, so frames the server wrote before dying are handled
            // before its crash ends the run.
            _ = server_poll.tick(), if server.child.is_some() => {
                if let Some(status) = server.exited() {
                    server.stderr_tail.wait_closed().await;
                    settle_lost_server!('main, Some(status));
                }
            }
        }
    }

    // A run that ended mid-delivery never posts its prompt again.
    delivery.cancel();
    if stop_teardown {
        // Close the mailbox before the stop reads settled: the next prompt
        // must start a fresh runtime, never land in this one.
        drop(steering);
        bus_handle.abort();
        server.shutdown(kill_grace).await;
        settle_children(&mut children, &event_tx, DoneStatus::Interrupted).await;
        let _ = send(
            &event_tx,
            AgentEvent::Done {
                status: DoneStatus::Interrupted,
                result: None,
                error: None,
                session_id: Some(session_id.clone()),
            },
        )
        .await;
        return;
    }
    if !done_sent {
        // Consumer went away (stream dropped): nothing to report to.
        tracing::debug!(target: "zeron_harness::opencode", "run loop ended without settling");
    }
    bus_handle.abort();
    server.shutdown(kill_grace).await;
}

async fn create_session(
    server: &Server,
    dir: Option<&str>,
    agent: Option<&str>,
) -> Result<String, HarnessError> {
    if server.protocol().await == Protocol::V2 {
        // 2.x takes the run directory in the BODY (`location.directory`) —
        // the header is ignored on this route (observed live, 2.0.3) — and
        // wraps the answer in `{data}`. Its schema is stable; the 1.x-only
        // lazy-migration crash below doesn't exist there.
        let mut body = match dir {
            Some(dir) => json!({ "location": { "directory": dir } }),
            None => json!({}),
        };
        if let Some(agent) = agent {
            body["agent"] = json!(agent);
        }
        let created = server.post_json("/api/session", dir, &body).await?;
        return created
            .pointer("/data/id")
            .or_else(|| created.get("id"))
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| {
                HarnessError::Protocol("opencode session create returned no id".into())
            });
    }
    // opencode 1.18.x lazily migrates a directory's legacy rows on its
    // FIRST directory-scoped request (`Project.migrateProjectId`). On a db
    // whose schema a newer 2.x install has rewritten, that migration throws
    // mid-request — observed live: `SQLiteError: no such column:
    // project_id` → `500 UnknownError` on `POST /session` — but the project
    // row it inserted commits BEFORE the throw, so the identical request
    // retried immediately succeeds. Retry once on any 5xx: this failure
    // class then costs one round-trip instead of a dead turn.
    for attempt in 0..2 {
        let (status, text) = server.post_json_raw("/session", dir, &json!({})).await?;
        if status.is_success() {
            let created = serde_json::from_str::<Value>(&text).unwrap_or(Value::Null);
            return created
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| {
                    HarnessError::Protocol("opencode session create returned no id".into())
                });
        }
        if attempt == 0 && status.is_server_error() {
            tracing::debug!(
                target: "zeron_harness::opencode",
                "POST /session answered {status}; retrying once (the lazy-migration crash self-heals)"
            );
            tokio::time::sleep(Duration::from_millis(250)).await;
            continue;
        }
        return Err(HarnessError::Protocol(post_error_message(
            "/session", status, &text,
        )));
    }
    unreachable!("create_session retry loop returns from every path")
}

fn context_usage_event(info: &Value, context_windows: &HashMap<String, u64>) -> Option<AgentEvent> {
    let tokens = info.get("tokens")?;
    let counts: Vec<u64> = [
        tokens.get("input"),
        tokens.get("output"),
        tokens.pointer("/cache/read"),
        tokens.pointer("/cache/write"),
    ]
    .into_iter()
    .flatten()
    .filter_map(Value::as_u64)
    .collect();
    let tokens = tokens
        .get("total")
        .and_then(Value::as_u64)
        .filter(|n| *n > 0)
        .or_else(|| {
            (!counts.is_empty()).then(|| counts.into_iter().fold(0u64, u64::saturating_add))
        });
    // New assistant placeholders carry zero counters before the request runs.
    if tokens == Some(0) && info.pointer("/time/completed").is_none() {
        return None;
    }
    let window = info
        .get("providerID")
        .and_then(Value::as_str)
        .zip(info.get("modelID").and_then(Value::as_str))
        .and_then(|(provider, model)| context_windows.get(&format!("{provider}/{model}")).copied());
    (tokens.is_some() || window.is_some()).then_some(AgentEvent::ContextUsage { tokens, window })
}

/// The requested effort as a variant id the model actually advertises.
/// A message waiting for the turn's end.
struct QueuedSteer {
    text: String,
    /// It selects a native command (which always travels alone).
    native: bool,
    attachments: Vec<String>,
    /// The configuration it was sent with, adopted before its prompt.
    config: Option<Box<RunRequest>>,
}

/// The `provider/model` pair and reasoning variant a request runs with.
fn model_selection(
    providers: &ProviderCatalog,
    request: &RunRequest,
) -> (Option<(String, String)>, Option<String>) {
    let model = request
        .model
        .as_deref()
        .and_then(|m| m.split_once('/'))
        .map(|(provider, model)| (provider.to_owned(), model.to_owned()));
    let variant = model.as_ref().and_then(|(provider, model_id)| {
        pick_variant(providers, provider, model_id, request.reasoning)
    });
    (model, variant)
}

/// Adopt `next`'s model and effort for the prompts that follow: 1.x carries
/// them on each prompt, 2.x sets them on the session first. The catalog is
/// fetched again here rather than held for the session's life.
async fn adopt_config(
    server: &Server,
    session_id: &str,
    dir: Option<&str>,
    next: &RunRequest,
) -> Result<(Option<(String, String)>, Option<String>), HarnessError> {
    let providers = server.provider_catalog(dir).await.unwrap_or_default();
    let (model, variant) = model_selection(&providers, next);
    if server.protocol().await == Protocol::V2
        && let Some((provider, model_id)) = &model
    {
        server
            .set_model(session_id, provider, model_id, variant.as_deref(), dir)
            .await?;
    }
    Ok((model, variant))
}

fn pick_variant(
    providers: &ProviderCatalog,
    provider_id: &str,
    model_id: &str,
    reasoning: Option<ReasoningLevel>,
) -> Option<String> {
    let candidates = variant_candidates(reasoning);
    if candidates.is_empty() {
        return None;
    }
    let variants = providers
        .all
        .as_ref()?
        .iter()
        .find(|p| p.id.as_deref() == Some(provider_id))?
        .models
        .as_ref()?
        .get(model_id)?
        .variants
        .as_ref()?;
    candidates
        .into_iter()
        .find(|c| variants.contains_key(*c))
        .map(str::to_owned)
}

/// Build a `prompt_async` body: text part + attachment file parts.
fn prompt_body(
    prompt: &str,
    model: Option<(&str, &str)>,
    variant: Option<&str>,
    attachments: &[String],
    message_id: Option<&str>,
) -> Value {
    let mut parts = vec![json!({ "type": "text", "text": prompt })];
    for path in attachments {
        parts.push(json!({
            "type": "file",
            "mime": mime_for(path),
            "filename": attachment_name(path),
            "url": file_url(path),
        }));
    }
    let mut body = serde_json::Map::new();
    if let Some(id) = message_id {
        body.insert("messageID".into(), json!(id));
    }
    body.insert("parts".into(), Value::Array(parts));
    if let Some((provider, model)) = model {
        body.insert(
            "model".into(),
            json!({ "providerID": provider, "modelID": model }),
        );
    }
    if let Some(variant) = variant {
        body.insert("variant".into(), Value::String(variant.to_owned()));
    }
    Value::Object(body)
}

/// 2.x prompt body: plain text plus `{uri, name}` file attachments. Model
/// and variant do NOT ride here — they were set on the session at run
/// start (`POST /api/session/{id}/model`).
fn prompt_body_v2(prompt: &str, attachments: &[String], message_id: Option<&str>) -> Value {
    let files: Vec<Value> = attachments
        .iter()
        .map(|path| json!({ "uri": file_url(path), "name": attachment_name(path) }))
        .collect();
    let mut body = json!({ "text": prompt, "files": files });
    if let Some(id) = message_id {
        body["id"] = json!(id);
    }
    body
}

fn attachment_name(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// `path` as the `file:` URL Node's `fileURLToPath` (how both wires read an
/// attachment) maps back to the same path. Every byte outside the unreserved
/// set is percent-encoded, so `#`, `?`, `%` and spaces survive; a Windows
/// drive path becomes `file:///C:/…`, a UNC path `file://server/share/…`
/// (its server is the URL's host), and verbatim `\\?\` prefixes are dropped.
fn file_url(path: &str) -> String {
    let unc = path.strip_prefix(r"\\?\UNC\").or_else(|| {
        path.strip_prefix(r"\\")
            .filter(|rest| !rest.starts_with(r"?\"))
    });
    if let Some(unc) = unc {
        let unc = unc.replace('\\', "/");
        let (host, share) = unc.split_once('/').unwrap_or((&unc, ""));
        return format!("file://{host}/{}", percent_encode_path(share));
    }
    let path = path.strip_prefix(r"\\?\").unwrap_or(path);
    let bytes = path.as_bytes();
    if bytes.len() >= 2
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes.get(2), None | Some(b'\\' | b'/'))
    {
        let rest = path[2..].replace('\\', "/");
        return format!("file:///{}:{}", &path[..1], percent_encode_path(&rest));
    }
    let root = if path.starts_with('/') { "" } else { "/" };
    format!("file://{root}{}", percent_encode_path(path))
}

fn mime_for(path: &str) -> &'static str {
    match std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("pdf") => "application/pdf",
        _ => "application/octet-stream",
    }
}

/// What a posted turn carries besides its text (1.x folds model/variant
/// into the prompt body; 2.x ignores them there — set on the session).
struct TurnSpec<'a> {
    model: Option<&'a (String, String)>,
    variant: Option<&'a str>,
    attachments: &'a [String],
}

fn command_body_v2(
    version: Option<&ServerVersion>,
    name: &str,
    text: &str,
    attachments: &[String],
) -> Value {
    if version
        .and_then(|v| v.number)
        .is_some_and(|v| v >= (2, 0, 4))
    {
        let mut body = json!({"name": name, "text": text});
        if !attachments.is_empty() {
            body["files"] = prompt_body_v2(text, attachments, None)["files"].clone();
        }
        body
    } else {
        json!({"command": name, "text": text})
    }
}

/// Send a turn: a leading `/command` known to the agent routes through the
/// command endpoint (the desktop parity — the server does NOT parse slash
/// text out of an ordinary prompt); everything else is a prompt.
/// Both are fire-and-forget for the loop: the command endpoint is
/// synchronous on the wire, so it rides a detached task and the bus
/// delivers the actual turn.
///
/// `Ok(true)`: a prompt whose acceptance (or failure) `updates` will carry;
/// `Ok(false)`: a native command, which reports only failure. Cancelling
/// `delivery` (the turn was stopped) ends a prompt's delivery: it is never
/// posted again, and nothing is reported.
#[allow(clippy::too_many_arguments)]
async fn post_prompt(
    server: &Server,
    session_id: &str,
    dir: Option<&str>,
    commands: &[SlashCommand],
    prompt: &str,
    native_command_selected: bool,
    turn_generation: u64,
    updates: &mpsc::UnboundedSender<TurnUpdate>,
    delivery: &tokio_util::sync::CancellationToken,
    spec: TurnSpec<'_>,
) -> Result<bool, HarnessError> {
    let TurnSpec {
        model,
        variant,
        attachments,
    } = spec;
    let protocol = server.protocol().await;
    if let Some((name, arguments)) =
        native_command_request(prompt, commands, native_command_selected)?
    {
        if !attachments.is_empty() {
            return Err(HarnessError::Protocol(
                "OpenCode commands cannot include attachments; send them in a separate prompt"
                    .into(),
            ));
        }
        // 1.x names the args `arguments`; 2.x `text`.
        let (path, cmd_body) = match protocol {
            Protocol::V1 => (
                format!("/session/{session_id}/command"),
                json!({ "command": name, "arguments": arguments }),
            ),
            Protocol::V2 => (
                format!("/api/session/{session_id}/command"),
                command_body_v2(server.version.get(), name, &arguments, attachments),
            ),
        };
        let server = server.handle();
        let dir_owned = dir.map(str::to_owned);
        let path_owned = path.clone();
        let updates = updates.clone();
        tokio::spawn(async move {
            // The command endpoint blocks for the whole turn; the bus
            // carries the real events, so this response is ignored —
            // but it must not be cut off mid-turn by CALL_TIMEOUT.
            let mut req = server
                .request(reqwest::Method::POST, &path_owned)
                .json(&cmd_body);
            req = server.scoped(req, dir_owned.as_deref()).await;
            let failure = match req.send().await {
                Ok(response) if response.status().is_success() => None,
                Ok(response) => {
                    let status = response.status();
                    let body = response.text().await.unwrap_or_default();
                    Some(post_error_message(&path_owned, status, &body))
                }
                Err(error) => Some(format!("opencode POST {path_owned}: {error}")),
            };
            if let Some(message) = failure {
                let _ = updates.send(TurnUpdate {
                    generation: turn_generation,
                    signal: TurnSignal::CommandFailed(message),
                });
            }
        });
        return Ok(false);
    }
    let message_id = prompt_message_id();
    let (path, body) = match protocol {
        Protocol::V1 => (
            format!("/session/{session_id}/prompt_async"),
            prompt_body(
                prompt,
                model.map(|(provider, model)| (provider.as_str(), model.as_str())),
                variant,
                attachments,
                Some(&message_id),
            ),
        ),
        Protocol::V2 => (
            format!("/api/session/{session_id}/prompt"),
            prompt_body_v2(prompt, attachments, Some(&message_id)),
        ),
    };
    let server = server.handle();
    let session_id = session_id.to_owned();
    let updates = updates.clone();
    let dir = dir.map(str::to_owned);
    let delivery = delivery.clone();
    // The bus owns turn completion. A stalled HTTP acknowledgement must not
    // prevent cancellation or event consumption; each POST is bounded.
    tokio::spawn(async move {
        let delivered = deliver_prompt(
            &server,
            &path,
            &body,
            &session_id,
            &message_id,
            dir.as_deref(),
            &delivery,
        )
        .await;
        if delivery.is_cancelled() {
            return;
        }
        let signal = match delivered {
            Ok(()) => TurnSignal::PromptAccepted,
            Err(message) => TurnSignal::PromptFailed(message),
        };
        let _ = updates.send(TurnUpdate {
            generation: turn_generation,
            signal,
        });
    });
    Ok(true)
}

/// POST a prompt until the server has it. A 4xx is a definitive rejection.
/// A timeout, a dropped connection or a 5xx is ambiguous — the server may
/// have queued the prompt and only the answer was lost — so before posting
/// again (and before giving up) the session is checked for it: running, or
/// holding the message under the id we minted, and only a prompt it does
/// not have is posted again. That check, not the server, is what keeps a
/// repeat from being a duplicate turn: 2.x does not reject a known id, it
/// answers 200 and updates the stored message in place (verified live,
/// 2.1.2). A stopped turn's prompt (`cancel`) is never posted again.
async fn deliver_prompt(
    server: &Server,
    path: &str,
    body: &Value,
    session_id: &str,
    message_id: &str,
    dir: Option<&str>,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<(), String> {
    let mut last_error = String::new();
    for attempt in 0..PROMPT_ATTEMPTS {
        if attempt > 0 {
            tokio::select! {
                _ = cancel.cancelled() => return Err("prompt delivery cancelled".into()),
                _ = tokio::time::sleep(PROMPT_RETRY_DELAY * attempt) => {}
            }
            if server.prompt_landed(session_id, message_id, dir).await {
                return Ok(());
            }
            if cancel.is_cancelled() {
                return Err("prompt delivery cancelled".into());
            }
        }
        match server.post_json_raw(path, dir, body).await {
            Ok((status, _)) if status.is_success() => return Ok(()),
            Ok((status, text)) if status.is_client_error() => {
                return Err(post_error_message(path, status, &text));
            }
            Ok((status, text)) => last_error = post_error_message(path, status, &text),
            Err(error) => last_error = error.to_string(),
        }
        tracing::warn!(
            target: "zeron_harness::opencode",
            attempt,
            "prompt POST failed: {last_error}"
        );
    }
    if server.prompt_landed(session_id, message_id, dir).await {
        return Ok(());
    }
    Err(last_error)
}

/// A message id in opencode's own ascending form: `msg_`, 12 hex digits of
/// the millisecond clock ×4096 plus a per-millisecond counter, then 14
/// base62 characters. Sessions order their messages by id, so ours must
/// sort like one the server would mint at the same moment. Both wires take
/// it on the prompt (1.x `messageID`, 2.x `id`), which makes a lost answer
/// checkable and a retry safe ([`deliver_prompt`]).
fn prompt_message_id() -> String {
    static CLOCK: std::sync::Mutex<(u64, u64)> = std::sync::Mutex::new((0, 0));
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default();
    let counter = {
        let mut clock = CLOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if clock.0 != now {
            *clock = (now, 0);
        }
        clock.1 += 1;
        clock.1
    };
    let stamp = now.wrapping_mul(0x1000).wrapping_add(counter) & 0xFFFF_FFFF_FFFF;
    const BASE62: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let random: String = uuid::Uuid::new_v4().as_bytes()[..14]
        .iter()
        .map(|b| BASE62[usize::from(*b) % BASE62.len()] as char)
        .collect();
    format!("msg_{stamp:012x}{random}")
}

/// Resolve a delivered leading slash command against the run's live,
/// project-scoped catalog. Canonical composer selections must remain commands:
/// if their discovered entry vanished, reporting that race is safer than
/// silently submitting the decoded slash text as an ordinary model prompt.
/// Raw slash text retains OpenCode's historical prompt fallback.
fn native_command_request<'a>(
    prompt: &'a str,
    commands: &[SlashCommand],
    selected: bool,
) -> Result<Option<(&'a str, &'a str)>, HarnessError> {
    let Some((name, arguments)) = zeron_proto::invocation::leading_command(prompt) else {
        return if selected {
            Err(HarnessError::Protocol(
                "The selected OpenCode command is no longer available in this project".into(),
            ))
        } else {
            Ok(None)
        };
    };
    if commands.iter().any(|command| command.name == name) {
        Ok(Some((name, arguments)))
    } else if selected {
        Err(HarnessError::Protocol(format!(
            "The selected OpenCode command /{name} is no longer available in this project"
        )))
    } else {
        Ok(None)
    }
}

// ---------------------------------------------------------------------------
// Bus event handling
// ---------------------------------------------------------------------------

enum BusOutcome {
    Continue,
    /// Our session's turn reached idle.
    TurnIdle,
    TurnInterrupted,
    ConsumerGone,
}

type RequestInput = Box<
    dyn Fn(Vec<UserInputQuestion>) -> tokio::sync::oneshot::Receiver<Vec<UserInputAnswer>>
        + Send
        + Sync,
>;

struct BusCtx<'a> {
    event: &'a Value,
    session_id: &'a str,
    server: &'a Server,
    dir: Option<&'a str>,
    event_tx: &'a mpsc::Sender<Result<AgentEvent, HarnessError>>,
    request_input: &'a Arc<RequestInput>,
    main_feed: &'a mut SessionFeed,
    children: &'a mut HashMap<String, ChildRun>,
    pending_spawns: &'a mut VecDeque<PendingSpawn>,
    unbound_children: &'a mut HashMap<String, String>,
    turn: &'a mut TurnState,
    pending_usage: &'a mut Option<AgentEvent>,
    context_windows: &'a HashMap<String, u64>,
    /// Permission/question ids already handled (a reconnect re-lists them).
    seen_requests: &'a mut std::collections::HashSet<String>,
    /// The live turn, for reply failures.
    generation: u64,
    updates_tx: &'a mpsc::UnboundedSender<TurnUpdate>,
}

/// Deliver an answer the agent is blocked on. Each round tries `routes` in
/// order (1.x keeps a session-scoped fallback); a timeout, a transport fault
/// or a 5xx retries with backoff. A request every route reports missing
/// (404) is no longer pending — answered elsewhere, or gone with its turn —
/// so that counts as delivered rather than failing the turn.
async fn deliver_reply(
    server: &Server,
    routes: &[(String, Value)],
    dir: Option<&str>,
) -> Result<(), String> {
    let mut last_error = String::new();
    for attempt in 0..REPLY_ATTEMPTS {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_secs(1) * attempt).await;
        }
        let mut missing = 0;
        let mut retryable = false;
        for (path, body) in routes {
            match tokio::time::timeout(REPLY_TIMEOUT, server.post_json_raw(path, dir, body)).await {
                Ok(Ok((status, _))) if status.is_success() => return Ok(()),
                Ok(Ok((status, text))) => {
                    missing += usize::from(status == reqwest::StatusCode::NOT_FOUND);
                    retryable |= !status.is_client_error();
                    last_error = post_error_message(path, status, &text);
                }
                Ok(Err(error)) => {
                    retryable = true;
                    last_error = error.to_string();
                }
                Err(_) => {
                    retryable = true;
                    last_error = format!(
                        "opencode POST {path}: no answer within {}s",
                        REPLY_TIMEOUT.as_secs()
                    );
                }
            }
        }
        if missing == routes.len() {
            return Ok(());
        }
        if !retryable {
            break;
        }
    }
    Err(last_error)
}

/// Wrap an event as subagent-attributed traffic.
fn tag(parent: &str, event: AgentEvent) -> AgentEvent {
    AgentEvent::Subagent {
        parent_tool_use_id: parent.to_owned(),
        event: Box::new(event),
    }
}

async fn settle_children(
    children: &mut HashMap<String, ChildRun>,
    event_tx: &mpsc::Sender<Result<AgentEvent, HarnessError>>,
    status: DoneStatus,
) {
    for child in children.values_mut() {
        if child.done {
            continue;
        }
        child.done = true;
        let _ = event_tx
            .send(Ok(tag(
                &child.parent_tool_use_id,
                AgentEvent::Done {
                    status,
                    result: None,
                    error: None,
                    session_id: None,
                },
            )))
            .await;
    }
}

/// Route one `/global/event` payload. The envelope is
/// `{directory, payload: {type, properties}}`; `sync` mirror frames drop.
async fn handle_bus_event(ctx: BusCtx<'_>) -> BusOutcome {
    let BusCtx {
        event,
        session_id,
        server,
        dir,
        event_tx,
        request_input,
        main_feed,
        children,
        pending_spawns,
        unbound_children,
        turn,
        pending_usage,
        context_windows,
        seen_requests,
        generation,
        updates_tx,
    } = ctx;
    // Envelope styles: /global/event wraps ({payload: {...}}); a bare
    // /event feed (tests) delivers the payload directly.
    let payload = event.get("payload").unwrap_or(event);
    let kind = payload.get("type").and_then(Value::as_str).unwrap_or("");
    if kind == "sync" || kind.is_empty() {
        return BusOutcome::Continue;
    }
    let props = payload.get("properties").unwrap_or(&Value::Null);
    let event_session = props
        .get("sessionID")
        .and_then(Value::as_str)
        .or_else(|| {
            props
                .get("info")
                .and_then(|i| i.get("sessionID").or_else(|| i.get("id")))
                .and_then(Value::as_str)
        })
        .or_else(|| {
            props
                .get("part")
                .and_then(|p| p.get("sessionID"))
                .and_then(Value::as_str)
        });

    let is_ours = event_session == Some(session_id);
    if is_ours && kind == "session.interrupted" {
        return BusOutcome::TurnInterrupted;
    }
    let status = props
        .get("status")
        .and_then(|s| s.get("type"))
        .and_then(Value::as_str);
    if is_ours && (kind == "session.idle" || (kind == "session.status" && status == Some("idle"))) {
        // OpenCode can emit both idle encodings for one completion. The first
        // may submit a queued prompt before we consume the second. Until that
        // prompt starts (or fails/gets aborted), the second is stale. It must
        // neither complete the prompt nor disarm its startup watchdog.
        if turn.idle_ready || turn.error.is_some() {
            turn.idle_confirmations += 1;
            if turn.idle_confirmations >= 2 {
                return BusOutcome::TurnIdle;
            }
        }
        turn.status_poll = Some(tokio::time::Instant::now() + turn.status_backoff);
        return BusOutcome::Continue;
    }
    if is_ours && turn.active {
        turn.idle_confirmations = 0;
        turn.status_poll = None;
        turn.status_backoff = Duration::from_millis(100);
        turn.note_activity();
    }

    match kind {
        "session.status" if is_ours => {
            let status = props.get("status").unwrap_or(&Value::Null);
            match status.get("type").and_then(Value::as_str) {
                Some("busy") => turn.idle_ready = true,
                Some("retry") => {
                    turn.idle_ready = true;
                    let attempt = status.get("attempt").and_then(Value::as_u64).unwrap_or(0);
                    let message = status
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("provider error");
                    if attempt >= RETRY_ABORT_ATTEMPT && !turn.aborted_for_retry {
                        turn.aborted_for_retry = true;
                        turn.error = Some(format!(
                            "the provider kept failing after {attempt} attempts: {message}"
                        ));
                        let msg = format!(
                            "Giving up after {attempt} provider retries: {message}. {STALL_HINT}"
                        );
                        if !send(event_tx, AgentEvent::Error { message: msg }).await {
                            return BusOutcome::ConsumerGone;
                        }
                        let _ = tokio::time::timeout(
                            Duration::from_secs(5),
                            server.abort_session(session_id, dir),
                        )
                        .await;
                    } else if attempt >= RETRY_REPORT_ATTEMPT && !turn.retry_reported {
                        turn.retry_reported = true;
                        let msg = format!(
                            "The provider is failing and opencode is retrying (attempt \
                             {attempt}): {message}"
                        );
                        if !send(event_tx, AgentEvent::Error { message: msg }).await {
                            return BusOutcome::ConsumerGone;
                        }
                    }
                }
                _ => {}
            }
            BusOutcome::Continue
        }
        "session.error" | "session.warning" => {
            // Errors are session-scoped but a missing id still concerns us
            // (global provider failures).
            if event_session.is_some() && !is_ours {
                return BusOutcome::Continue;
            }
            let error = props.get("error").unwrap_or(&Value::Null);
            let name = error.get("name").and_then(Value::as_str).unwrap_or("");
            if name == "MessageAbortedError" {
                // The abort echo of an interrupt — not an error chip.
                return BusOutcome::Continue;
            }
            let message = error
                .get("data")
                .and_then(|d| d.get("message"))
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| {
                    if name.is_empty() {
                        "opencode reported an error".to_owned()
                    } else {
                        name.to_owned()
                    }
                });
            // opencode emits the same failure twice (once bare, once wrapped
            // with an exception-name/stack prefix) — one chip per distinct
            // failure: dedupe when either first line contains the other.
            let first = |m: &str| m.lines().next().unwrap_or(m).trim().to_owned();
            let line = first(&message);
            let duplicate = turn.error.as_deref().is_some_and(|prev| {
                let prev = first(prev);
                !line.is_empty() && (prev.contains(&line) || line.contains(&prev))
            });
            if kind == "session.error" {
                turn.error = Some(message.clone());
            }
            if !duplicate && !send(event_tx, AgentEvent::Error { message }).await {
                return BusOutcome::ConsumerGone;
            }
            BusOutcome::Continue
        }
        "session.created" => {
            let info = props.get("info").unwrap_or(&Value::Null);
            // Only DIRECT children bind; a grandchild's parentID is the
            // child's session and renders inside the child's own doc.
            if info.get("parentID").and_then(Value::as_str) != Some(session_id) {
                return BusOutcome::Continue;
            }
            let Some(child_id) = info.get("id").and_then(Value::as_str) else {
                return BusOutcome::Continue;
            };
            if children.contains_key(child_id) {
                return BusOutcome::Continue;
            }
            let title = info
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if !bind_child(children, pending_spawns, child_id, title) {
                unbound_children.insert(child_id.to_owned(), title.to_owned());
            }
            BusOutcome::Continue
        }
        "message.updated" => {
            let info = props.get("info").unwrap_or(&Value::Null);
            let (Some(session), Some(message), Some(role)) = (
                info.get("sessionID").and_then(Value::as_str),
                info.get("id").and_then(Value::as_str),
                info.get("role").and_then(Value::as_str),
            ) else {
                return BusOutcome::Continue;
            };
            if session == session_id {
                main_feed
                    .assistant_messages
                    .entry(message.to_owned())
                    .or_insert(role == "assistant");
                // Token usage rides the assistant message; the last one
                // before idle wins, emitted right before Done.
                if role == "assistant"
                    && let Some(tokens) = info.get("tokens")
                {
                    if let Some(usage) = context_usage_event(info, context_windows)
                        && !send(event_tx, usage).await
                    {
                        return BusOutcome::ConsumerGone;
                    }
                    let input = tokens.get("input").and_then(Value::as_u64).unwrap_or(0);
                    let output = tokens.get("output").and_then(Value::as_u64).unwrap_or(0);
                    if input > 0 || output > 0 {
                        *pending_usage = Some(AgentEvent::Usage {
                            input_tokens: input,
                            output_tokens: output,
                        });
                    }
                }
                let events = replay_pending(main_feed, message, true, turn);
                return forward(event_tx, events).await;
            }
            if let Some(child) = children.get_mut(session) {
                child
                    .feed
                    .assistant_messages
                    .entry(message.to_owned())
                    .or_insert(role == "assistant");
                // A NEW user message on a settled child is a steer resuming
                // it: un-latch so resumed traffic streams to the same chip.
                if role == "user" && child.done {
                    child.done = false;
                }
                let events = replay_pending(&mut child.feed, message, false, turn);
                let parent = child.parent_tool_use_id.clone();
                let tagged = events.into_iter().map(|ev| tag(&parent, ev)).collect();
                return forward(event_tx, tagged).await;
            }
            BusOutcome::Continue
        }
        "message.part.updated" => {
            let part = props.get("part").unwrap_or(&Value::Null);
            let Some(session) = part.get("sessionID").and_then(Value::as_str) else {
                return BusOutcome::Continue;
            };
            if session == session_id {
                let events = part_snapshot_events(
                    main_feed,
                    part,
                    true,
                    Some((children, pending_spawns, unbound_children)),
                );
                mark_content(turn, &events);
                let mut settle: Vec<AgentEvent> = Vec::new();
                // A completed `task` part settles its child chip.
                if let Some((child_session, failed)) = task_completion(part) {
                    let by_meta = children
                        .get_mut(&child_session)
                        .map(|c| (child_session.clone(), c));
                    let target = match by_meta {
                        Some(v) => Some(v),
                        None => {
                            // No metadata binding: settle whichever child
                            // streams to this part's chip.
                            let part_id = part.get("id").and_then(Value::as_str).unwrap_or("");
                            children
                                .iter_mut()
                                .find(|(_, c)| c.parent_tool_use_id == part_id)
                                .map(|(id, c)| (id.clone(), c))
                        }
                    };
                    if let Some((_, child)) = target
                        && !child.done
                    {
                        child.done = true;
                        settle.push(tag(
                            &child.parent_tool_use_id,
                            AgentEvent::Done {
                                status: if failed {
                                    DoneStatus::Errored
                                } else {
                                    DoneStatus::Completed
                                },
                                result: None,
                                error: None,
                                session_id: None,
                            },
                        ));
                    }
                }
                let mut all = events;
                all.extend(settle);
                return forward(event_tx, all).await;
            }
            if let Some(child) = children.get_mut(session) {
                if child.done {
                    return BusOutcome::Continue;
                }
                let events = part_snapshot_events(&mut child.feed, part, false, None);
                let parent = child.parent_tool_use_id.clone();
                let tagged = events.into_iter().map(|ev| tag(&parent, ev)).collect();
                return forward(event_tx, tagged).await;
            }
            BusOutcome::Continue
        }
        "message.part.delta" => {
            let (Some(session), Some(part_id), Some(delta)) = (
                props.get("sessionID").and_then(Value::as_str),
                props.get("partID").and_then(Value::as_str),
                props.get("delta").and_then(Value::as_str),
            ) else {
                return BusOutcome::Continue;
            };
            if props.get("field").and_then(Value::as_str) != Some("text") {
                return BusOutcome::Continue;
            }
            if session == session_id {
                let events = part_delta_events(main_feed, props, part_id, delta);
                mark_content(turn, &events);
                return forward(event_tx, events).await;
            }
            if let Some(child) = children.get_mut(session) {
                if child.done {
                    return BusOutcome::Continue;
                }
                let events = part_delta_events(&mut child.feed, props, part_id, delta);
                let parent = child.parent_tool_use_id.clone();
                let tagged = events.into_iter().map(|ev| tag(&parent, ev)).collect();
                return forward(event_tx, tagged).await;
            }
            BusOutcome::Continue
        }
        "permission.asked" => {
            // A global bus includes unrelated sessions. Never answer their
            // requests, or a malformed request without an explicit owner.
            let Some(session) = event_session.filter(|session| {
                *session == session_id
                    || children.get(*session).is_some_and(|child| !child.done)
                    || unbound_children.contains_key(*session)
            }) else {
                return BusOutcome::Continue;
            };
            let Some(id) = props.get("id").and_then(Value::as_str) else {
                return BusOutcome::Continue;
            };
            if !seen_requests.insert(id.to_owned()) {
                return BusOutcome::Continue;
            }
            let session = session.to_owned();
            let protocol = server.protocol().await;
            // 1.x: global permission endpoint + a session-scoped fallback;
            // 2.x: the reply rides the session's permission route
            // (the key changed from reply to decision in 2.0.4).
            let (reply_path, fallback_path) = match protocol {
                Protocol::V1 => (
                    format!("/permission/{id}/reply"),
                    Some(format!("/session/{session}/permissions/{id}")),
                ),
                Protocol::V2 => (
                    format!("/api/session/{session}/permission/{id}/reply"),
                    None,
                ),
            };
            let server = server.handle();
            let dir_owned = dir.map(str::to_owned);
            let updates_tx = updates_tx.clone();
            let reply_key = if protocol == Protocol::V2
                && server
                    .version
                    .get()
                    .and_then(|v| v.number)
                    .is_some_and(|v| v >= (2, 0, 4))
            {
                "decision"
            } else {
                "reply"
            };
            tokio::spawn(async move {
                // Like Claude and Codex, normal Zeron sessions run unattended,
                // regardless of RunRequest.auto_approve. Approve each owned
                // request without writing durable permission rules via "always".
                // Genuine agent questions use the separate question.asked path.
                let reply = "once";
                let mut routes = vec![(reply_path, json!({ reply_key: reply }))];
                if let Some(fallback_path) = fallback_path {
                    routes.push((fallback_path, json!({ "response": reply })));
                }
                if let Err(error) = deliver_reply(&server, &routes, dir_owned.as_deref()).await {
                    let _ = updates_tx.send(TurnUpdate {
                        generation,
                        signal: TurnSignal::ReplyFailed(format!(
                            "Could not approve an opencode permission request, so the \
                             agent cannot continue: {error}"
                        )),
                    });
                }
            });
            BusOutcome::Continue
        }
        "question.asked" => {
            // A global bus includes unrelated sessions. Never answer their
            // requests, or a malformed request without an explicit owner.
            let Some(session) = event_session.filter(|session| {
                *session == session_id
                    || children.get(*session).is_some_and(|child| !child.done)
                    || unbound_children.contains_key(*session)
            }) else {
                return BusOutcome::Continue;
            };
            let session = session.to_owned();
            let Some(id) = props.get("id").and_then(Value::as_str) else {
                return BusOutcome::Continue;
            };
            // Panel ids are minted per question, like Claude and Codex; the
            // paired key is what a 2.x form reply addresses.
            let pairs = map_questions(props);
            if pairs.is_empty() || !seen_requests.insert(id.to_owned()) {
                return BusOutcome::Continue;
            }
            // 2.x (2.0.24+) ships the `question` tool as a session form; its
            // answers ride the session's form route, keyed by field.
            let is_form = props.get("form").and_then(Value::as_bool).unwrap_or(false);
            // The engine's input bridge is the SOLE emitter of
            // `InputRequested`/`InputResolved`: it mints the request id,
            // parks the resolver for `respond_input`, and surfaces both
            // events. Emitting our own copy here (keyed by opencode's id)
            // folded a SECOND, unanswerable input part into the doc whose id
            // no resolver knew — the QuestionPanel answered that twin and the
            // run never resumed (the same fix Claude and Codex got).
            let rx = (request_input)(pairs.iter().map(|(_, question)| question.clone()).collect());
            let server = server.handle();
            let dir_owned = dir.map(str::to_owned);
            let request_id = id.to_owned();
            let updates_tx = updates_tx.clone();
            tokio::spawn(async move {
                let answer = match rx.await {
                    Ok(answers) if is_form => Some((
                        format!("/api/session/{session}/form/{request_id}/reply"),
                        form_answer_body(&pairs, &answers),
                    )),
                    Ok(answers) => {
                        let ordered: Vec<Vec<String>> = pairs
                            .iter()
                            .map(|(_, q)| {
                                answers
                                    .iter()
                                    .find(|a| a.question_id == q.id)
                                    .map(|a| a.labels.clone())
                                    .unwrap_or_default()
                            })
                            .collect();
                        Some((
                            format!("/question/{request_id}/reply"),
                            json!({ "answers": ordered }),
                        ))
                    }
                    // A dropped resolver means the run — and with it this
                    // OpenCode child — is already going away. Pending forms
                    // are instance state and die with it, so there is nothing
                    // left to cancel; only the 1.x route gets an explicit
                    // reject.
                    Err(_) if is_form => None,
                    Err(_) => {
                        if let Err(e) = server
                            .post_json(
                                &format!("/question/{request_id}/reject"),
                                dir_owned.as_deref(),
                                &Value::Null,
                            )
                            .await
                        {
                            tracing::debug!(
                                target: "zeron_harness::opencode",
                                "question reject failed: {e}"
                            );
                        }
                        None
                    }
                };
                if let Some(route) = answer
                    && let Err(error) = deliver_reply(&server, &[route], dir_owned.as_deref()).await
                {
                    let _ = updates_tx.send(TurnUpdate {
                        generation,
                        signal: TurnSignal::ReplyFailed(format!(
                            "Could not deliver the answer to opencode, so the agent \
                             cannot continue: {error}"
                        )),
                    });
                }
            });
            BusOutcome::Continue
        }
        _ => BusOutcome::Continue,
    }
}

async fn forward(
    event_tx: &mpsc::Sender<Result<AgentEvent, HarnessError>>,
    events: Vec<AgentEvent>,
) -> BusOutcome {
    for ev in events {
        if event_tx.send(Ok(ev)).await.is_err() {
            return BusOutcome::ConsumerGone;
        }
    }
    BusOutcome::Continue
}

fn mark_content(turn: &mut TurnState, events: &[AgentEvent]) {
    for ev in events {
        match ev {
            AgentEvent::ToolCall { id, .. } => {
                turn.open_tools.insert(id.clone());
            }
            AgentEvent::ToolResult { id, .. } => {
                turn.open_tools.remove(id);
            }
            _ => {}
        }
    }
    if turn.active
        && events.iter().any(|ev| {
            matches!(
                ev,
                AgentEvent::TextDelta { .. }
                    | AgentEvent::ReasoningDelta { .. }
                    | AgentEvent::ToolCall { .. }
            )
        })
    {
        turn.saw_content = true;
    }
}

/// A completed/errored `task` part → (child session id, failed).
fn task_completion(part: &Value) -> Option<(String, bool)> {
    if part.get("type").and_then(Value::as_str) != Some("tool")
        || !matches!(
            part.get("tool").and_then(Value::as_str),
            Some("task" | "subagent")
        )
    {
        return None;
    }
    let state = part.get("state")?;
    let status = state.get("status").and_then(Value::as_str)?;
    if !matches!(status, "completed" | "error") {
        return None;
    }
    let child = state
        .get("metadata")
        .and_then(|m| m.get("sessionId").or_else(|| m.get("sessionID")))
        .and_then(Value::as_str)
        .unwrap_or_default();
    Some((child.to_owned(), status == "error"))
}

/// Bind a fresh child session to a spawn chip: description match against the
/// child title (`"{description} (@{agent} subagent)"`), else FIFO.
fn bind_child(
    children: &mut HashMap<String, ChildRun>,
    pending: &mut VecDeque<PendingSpawn>,
    child_id: &str,
    title: &str,
) -> bool {
    let ix = pending
        .iter()
        .position(|p| !p.description.is_empty() && title.starts_with(&p.description))
        .or(if pending.is_empty() { None } else { Some(0) });
    match ix.and_then(|i| pending.remove(i)) {
        Some(p) => {
            children.insert(
                child_id.to_owned(),
                ChildRun {
                    parent_tool_use_id: p.tool_part_id,
                    feed: SessionFeed::default(),
                    done: false,
                },
            );
            true
        }
        None => false,
    }
}

/// Replay parts that raced ahead of their message's role fact.
fn replay_pending(
    feed: &mut SessionFeed,
    message: &str,
    is_main: bool,
    turn: &mut TurnState,
) -> Vec<AgentEvent> {
    let held: Vec<Value> = std::mem::take(&mut feed.pending_parts)
        .into_iter()
        .filter(|part| part.get("messageID").and_then(Value::as_str) == Some(message))
        .collect();
    let events: Vec<AgentEvent> = held
        .iter()
        .flat_map(|part| part_snapshot_events(feed, part, is_main, None))
        .collect();
    if is_main {
        mark_content(turn, &events);
    }
    events
}

/// Spawn-registration context for the MAIN feed (`task` parts register
/// pending chips; child feeds pass `None` — no recursive viz).
type SpawnCtx<'a> = (
    &'a mut HashMap<String, ChildRun>,
    &'a mut VecDeque<PendingSpawn>,
    &'a mut HashMap<String, String>,
);

/// A part snapshot: emit whatever text extends what already streamed, open /
/// resolve tool chips. Snapshots and deltas interleave — `emitted` (bytes of
/// part text already sent) is the dedup line between them.
fn part_snapshot_events(
    feed: &mut SessionFeed,
    part: &Value,
    is_main: bool,
    spawn_ctx: Option<SpawnCtx<'_>>,
) -> Vec<AgentEvent> {
    let Some(part_id) = part.get("id").and_then(Value::as_str) else {
        return Vec::new();
    };
    let message_id = part
        .get("messageID")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let kind = part.get("type").and_then(Value::as_str).unwrap_or_default();
    match kind {
        "text" | "reasoning" => {
            if kind == "text" && feed.assistant_messages.get(message_id).is_none() {
                // Role unknown: hold the part instead of guessing (dedup by
                // part id — snapshots re-deliver).
                if !feed
                    .pending_parts
                    .iter()
                    .any(|p| p.get("id").and_then(Value::as_str) == Some(part_id))
                {
                    feed.pending_parts.push(part.clone());
                }
                return Vec::new();
            }
            if kind == "text" && feed.assistant_messages.get(message_id) == Some(&false) {
                // A user-role text part. On the MAIN feed it is our own
                // prompt echo (the engine already wrote the user entry);
                // on a child feed it is the message INTO the child — its
                // spawn prompt or a steer, rendered as a user entry.
                if is_main {
                    return Vec::new();
                }
                let text = part.get("text").and_then(Value::as_str).unwrap_or_default();
                if text.trim().is_empty() {
                    return Vec::new();
                }
                let entry = feed
                    .parts
                    .entry(part_id.to_owned())
                    .or_insert_with(|| PartState {
                        kind: kind.to_owned(),
                        ..PartState::default()
                    });
                if entry.emitted > 0 {
                    return Vec::new();
                }
                entry.emitted = text.len();
                return vec![AgentEvent::UserMessage {
                    text: text.to_owned(),
                }];
            }
            if feed.assistant_messages.get(message_id) != Some(&true) {
                // Reasoning ahead of its message.updated: hold it too.
                if kind == "reasoning" {
                    if !feed
                        .pending_parts
                        .iter()
                        .any(|p| p.get("id").and_then(Value::as_str) == Some(part_id))
                    {
                        feed.pending_parts.push(part.clone());
                    }
                }
                return Vec::new();
            }
            let text = part.get("text").and_then(Value::as_str).unwrap_or_default();
            let entry = feed
                .parts
                .entry(part_id.to_owned())
                .or_insert_with(|| PartState {
                    kind: kind.to_owned(),
                    ..PartState::default()
                });
            let Some(suffix) = text
                .get(entry.emitted..)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
            else {
                // Shorter (or mid-char) snapshot: a rewrite this decoder
                // doesn't model — drop it rather than duplicate text.
                return Vec::new();
            };
            entry.emitted = text.len();
            vec![if entry.kind == "reasoning" {
                AgentEvent::ReasoningDelta { text: suffix }
            } else {
                AgentEvent::TextDelta { text: suffix }
            }]
        }
        "tool" => {
            let tool = part.get("tool").and_then(Value::as_str).unwrap_or_default();
            let status = part
                .get("state")
                .and_then(|s| s.get("status"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            let input = part
                .get("state")
                .and_then(|s| s.get("input"))
                .cloned()
                .unwrap_or(Value::Null);
            // The task chip's stable id is the PART id (the completion and
            // the child settle key on it); ordinary tools key on callID.
            let call_id = if matches!(tool, "task" | "subagent") && is_main {
                part_id.to_owned()
            } else {
                part.get("callID")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .unwrap_or(part_id)
                    .to_owned()
            };
            let entry = feed
                .parts
                .entry(part_id.to_owned())
                .or_insert_with(|| PartState {
                    kind: "tool".to_owned(),
                    ..PartState::default()
                });
            let mut events = Vec::new();
            let has_input = input.as_object().is_some_and(|o| !o.is_empty());
            // `running` means the input is final — including a tool that takes
            // no arguments, which must count as open so a steer never aborts
            // it (preemption waits for open tools).
            if !entry.tool_started
                && (has_input || matches!(status, "running" | "completed" | "error"))
            {
                entry.tool_started = true;
                events.push(AgentEvent::ToolCall {
                    id: call_id.clone(),
                    call: oc_tool_call(tool, &input),
                });
                // A task spawn on the MAIN feed registers a pending chip so
                // the child's session.created (or its metadata) can bind.
                if matches!(tool, "task" | "subagent")
                    && is_main
                    && let Some((children, pending, unbound)) = spawn_ctx
                {
                    register_spawn(children, pending, unbound, part, part_id, &input);
                }
            }
            if entry.tool_started && !entry.tool_done && matches!(status, "completed" | "error") {
                entry.tool_done = true;
                let output = part
                    .get("state")
                    .and_then(|s| {
                        s.get("output")
                            .or_else(|| s.get("error"))
                            .and_then(Value::as_str)
                    })
                    .filter(|t| !t.is_empty())
                    .map(|t| cap_text(t, OUTPUT_CAP));
                events.push(AgentEvent::ToolResult {
                    id: call_id,
                    is_error: status == "error",
                    output,
                    diff: None,
                });
            }
            events
        }
        // step-start / step-finish / snapshot / patch bookkeeping: not
        // transcript.
        _ => Vec::new(),
    }
}

/// Register a `task` spawn chip and bind it eagerly when the tool's own
/// metadata already names the child session (opencode stamps
/// `state.metadata.sessionId` at spawn).
fn register_spawn(
    children: &mut HashMap<String, ChildRun>,
    pending: &mut VecDeque<PendingSpawn>,
    unbound: &mut HashMap<String, String>,
    part: &Value,
    part_id: &str,
    input: &Value,
) {
    let known = pending.iter().any(|p| p.tool_part_id == part_id)
        || children.values().any(|c| c.parent_tool_use_id == part_id);
    if known {
        return;
    }
    let child_id = part
        .get("state")
        .and_then(|s| s.get("metadata"))
        .and_then(|m| m.get("sessionId").or_else(|| m.get("sessionID")))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !child_id.is_empty() && !children.contains_key(child_id) {
        unbound.remove(child_id);
        children.insert(
            child_id.to_owned(),
            ChildRun {
                parent_tool_use_id: part_id.to_owned(),
                feed: SessionFeed::default(),
                done: false,
            },
        );
        return;
    }
    let description = input
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    // A child that raced ahead of its chip binds now.
    let matched = unbound
        .iter()
        .find(|(_, title)| !description.is_empty() && title.starts_with(&description))
        .map(|(id, _)| id.clone())
        .or_else(|| {
            (unbound.len() == 1)
                .then(|| unbound.keys().next().cloned())
                .flatten()
        });
    if let Some(id) = matched {
        unbound.remove(&id);
        children.insert(
            id,
            ChildRun {
                parent_tool_use_id: part_id.to_owned(),
                feed: SessionFeed::default(),
                done: false,
            },
        );
        return;
    }
    pending.push_back(PendingSpawn {
        tool_part_id: part_id.to_owned(),
        description,
    });
}

/// A text delta appends to its part. Deltas follow the part's opening
/// `message.part.updated` (which fixes the kind); an unknown part defaults
/// to assistant text only when its message is known assistant.
fn part_delta_events(
    feed: &mut SessionFeed,
    props: &Value,
    part_id: &str,
    delta: &str,
) -> Vec<AgentEvent> {
    if delta.is_empty() {
        return Vec::new();
    }
    let message_id = props
        .get("messageID")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if feed.assistant_messages.get(message_id) != Some(&true) {
        return Vec::new();
    }
    let entry = feed
        .parts
        .entry(part_id.to_owned())
        .or_insert_with(|| PartState {
            kind: "text".to_owned(),
            ..PartState::default()
        });
    if entry.kind == "tool" {
        return Vec::new();
    }
    entry.emitted += delta.len();
    vec![if entry.kind == "reasoning" {
        AgentEvent::ReasoningDelta {
            text: delta.to_owned(),
        }
    } else {
        AgentEvent::TextDelta {
            text: delta.to_owned(),
        }
    }]
}

/// `question.asked` → the input panel's questions, each paired with the key a
/// form reply must address (the form field key; positional `q0`.. when the
/// payload carries none). Panel ids are minted per question, like Claude and
/// Codex — wire values never become panel identity.
fn map_questions(props: &Value) -> Vec<(String, UserInputQuestion)> {
    props
        .get("questions")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .enumerate()
                .filter_map(|(ix, q)| {
                    let question = q.get("question").and_then(Value::as_str)?;
                    let key = q
                        .get("key")
                        .and_then(Value::as_str)
                        .filter(|key| !key.is_empty())
                        .map(str::to_owned)
                        .unwrap_or_else(|| format!("q{ix}"));
                    Some((
                        key,
                        UserInputQuestion {
                            id: new_message_id(),
                            header: q
                                .get("header")
                                .and_then(Value::as_str)
                                .unwrap_or("Question")
                                .to_owned(),
                            question: question.to_owned(),
                            options: q
                                .get("options")
                                .and_then(Value::as_array)
                                .map(|opts| {
                                    opts.iter()
                                        .filter_map(|o| o.get("label").and_then(Value::as_str))
                                        .map(str::to_owned)
                                        .collect()
                                })
                                .unwrap_or_default(),
                            prefill: None,
                            multiline: false,
                            multi_select: q
                                .get("multiple")
                                .and_then(Value::as_bool)
                                .unwrap_or(false),
                        },
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// 2.x question forms take answers keyed by field (`q0`..): one label for a
/// single-select `string` field (empty when unanswered), the label array for
/// a `multiselect` field.
fn form_answer_body(pairs: &[(String, UserInputQuestion)], answers: &[UserInputAnswer]) -> Value {
    let mut answer = serde_json::Map::new();
    for (key, q) in pairs {
        let labels = answers
            .iter()
            .find(|a| a.question_id == q.id)
            .map(|a| a.labels.clone())
            .unwrap_or_default();
        let value = if q.multi_select {
            json!(labels)
        } else {
            json!(labels.into_iter().next().unwrap_or_default())
        };
        answer.insert(key.clone(), value);
    }
    json!({ "answer": answer })
}

/// Cap for tool outputs entering the event stream (journal keeps the rest).
const OUTPUT_CAP: usize = 4096;

fn cap_text(text: &str, cap: usize) -> String {
    if text.len() <= cap {
        return text.to_owned();
    }
    let mut end = cap;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

/// Type an opencode-native tool invocation.
fn oc_tool_call(name: &str, input: &Value) -> ToolCall {
    let s = |keys: &[&str]| {
        keys.iter()
            .find_map(|k| input.get(*k))
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
    };
    match name {
        "bash" => ToolCall::Exec {
            command: s(&["command"]).unwrap_or_default(),
        },
        "read" => ToolCall::ReadFile {
            path: s(&["filePath", "file_path", "path"]).unwrap_or_default(),
        },
        "write" => ToolCall::WriteFile {
            path: s(&["filePath", "file_path", "path"]).unwrap_or_default(),
            content: s(&["content"]),
        },
        "edit" => ToolCall::EditFile {
            path: s(&["filePath", "file_path", "path"]).unwrap_or_default(),
            old_string: s(&["oldString", "old_string"]),
            new_string: s(&["newString", "new_string"]),
        },
        "patch" => ToolCall::ApplyPatch {
            path: s(&["filePath", "file_path", "path"]),
        },
        "grep" => ToolCall::Search {
            pattern: s(&["pattern"]).unwrap_or_default(),
            path: s(&["path", "include"]),
        },
        "glob" => ToolCall::Glob {
            pattern: s(&["pattern"]).unwrap_or_default(),
        },
        "webfetch" => ToolCall::WebFetch {
            url: s(&["url"]).unwrap_or_default(),
            prompt: None,
        },
        "websearch" => ToolCall::WebSearch {
            query: s(&["query"]).unwrap_or_default(),
        },
        "todowrite" => ToolCall::Todo {
            items: input
                .get("todos")
                .and_then(Value::as_array)
                .map(|a| a.as_slice())
                .unwrap_or_default()
                .iter()
                .map(|t| {
                    TodoItem::new(
                        t.get("content").and_then(Value::as_str).unwrap_or_default(),
                        TodoStatus::parse(
                            t.get("status").and_then(Value::as_str).unwrap_or_default(),
                        ),
                    )
                })
                .collect(),
        },
        // The genus-gated spawn naming every driver shares.
        "task" | "subagent" => ToolCall::Unknown {
            name: s(&["description"])
                .map(|d| format!("Agent: {d}"))
                .unwrap_or_else(|| "Agent".into()),
            input: (!input.is_null()).then(|| input.clone()),
        },
        _ => ToolCall::Unknown {
            name: name.to_owned(),
            input: (!input.is_null()).then(|| input.clone()),
        },
    }
}

// ---------------------------------------------------------------------------
// SSE bus reader
// ---------------------------------------------------------------------------

/// Rewrite one 2.x `/api/event` frame into the 1.x-shaped bus payloads
/// (`{type, properties}`) the session loop consumes, so the whole turn
/// engine — feeds, settle, retries, subagents — stays wire-agnostic. The
/// 2.x frame is `{id, type, data: {sessionID, assistantMessageID, ..}}`
/// (all shapes captured live from a 2.0.3 server); the names and shapes
/// of the 2.x event vocabulary:
/// - turn lifecycle: `session.execution.started` (busy) and
///   `.succeeded`/`.interrupted`/`.failed` (idle; failed carries the error).
/// - per step: `session.step.started` fixes the assistant message id its
///   items stream under.
/// - streaming items: `session.text.delta` (+ started/ended snapshots) and
///   `session.reasoning.started/.ended`; `ordinal` numbers the item within
///   the step — the synthetic part id.
/// - tools: `session.tool.input.started` (carries the NAME),
///   `.input.ended` (args as text), `.called` (args as object),
///   `.success` (content array) / `.error`.
/// - usage: `session.step.ended` carries the step's own tokens; the
///   cumulative `session.usage.updated` totals are ignored.
///
/// `tool_names` tracks pending calls by session, message, and provider call id.
/// `session_models` remembers the latest model for step.ended, which omits it.
type V2ToolKey = (String, String, String);
const MAX_PENDING_V2_TOOLS: usize = 4096;
/// Cache cap; overflow clears the cache instead of failing the run.
const MAX_V2_SESSION_MODELS: usize = 4096;

#[derive(Clone, Debug, PartialEq, Eq)]
struct V2ModelIdentity {
    provider_id: String,
    model_id: String,
}

fn v2_model_identity(model: &Value) -> Option<V2ModelIdentity> {
    let provider_id = model.get("providerID")?.as_str()?;
    let model_id = model.get("id")?.as_str()?;
    (!provider_id.is_empty() && !model_id.is_empty()).then(|| V2ModelIdentity {
        provider_id: provider_id.to_owned(),
        model_id: model_id.to_owned(),
    })
}

#[cfg(test)]
fn normalize_v2_frame(event: Value, tool_names: &mut HashMap<V2ToolKey, String>) -> Vec<Value> {
    normalize_v2_frame_with_session_models(event, tool_names, &mut HashMap::new())
}

fn normalize_v2_frame_with_session_models(
    event: Value,
    tool_names: &mut HashMap<V2ToolKey, String>,
    session_models: &mut HashMap<String, V2ModelIdentity>,
) -> Vec<Value> {
    let kind = event.get("type").and_then(Value::as_str).unwrap_or("");
    let data = event.get("data").cloned().unwrap_or(Value::Null);
    // Form frames keep their owner inside `data.form` (the question tool's
    // 2.0.24+ transport); everything else must name a session.
    if kind == "form.created" {
        return v2_question_form_payload(data.get("form").unwrap_or(&data));
    }
    if data
        .get("sessionID")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
    {
        return Vec::new();
    }
    let session = || data.get("sessionID").cloned().unwrap_or(Value::Null);
    let message = || {
        data.get("assistantMessageID")
            .cloned()
            .unwrap_or(Value::Null)
    };
    let tool_key = || {
        (
            data.get("sessionID")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            data.get("assistantMessageID")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            data.get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        )
    };
    if matches!(
        kind,
        "session.execution.succeeded"
            | "session.execution.interrupted"
            | "session.execution.failed"
    ) {
        tool_names.retain(|(owner, _, _), _| {
            Some(owner.as_str()) != data.get("sessionID").and_then(Value::as_str)
        });
    }
    match kind {
        "session.status" => vec![json!({"type": "session.status", "properties": data})],
        "session.retry.scheduled" => vec![json!({
            "type": "session.status",
            "properties": {"sessionID": session(), "status": {
                "type": "retry", "attempt": data.get("attempt"), "next": data.get("at"),
                "message": data.pointer("/error/message").and_then(Value::as_str).filter(|s| !s.is_empty())
                    .or_else(|| data.pointer("/error/type").and_then(Value::as_str)).unwrap_or("provider retry"),
            }}
        })],
        "session.tool.progress" => {
            let id = data.get("id").and_then(Value::as_str).unwrap_or_default();
            let name = tool_names
                .get(&tool_key())
                .map(String::as_str)
                .unwrap_or_default();
            vec![v2_tool_part(
                &data,
                id,
                name,
                &json!({"status": "running", "metadata": data.get("metadata")}),
            )]
        }
        "session.execution.started" => vec![json!({
            "type": "session.status",
            "properties": { "sessionID": session(), "status": { "type": "busy" } }
        })],
        "session.execution.interrupted" => vec![json!({
            "type": "session.interrupted",
            "properties": { "sessionID": session() }
        })],
        "session.execution.succeeded" => vec![json!({
            "type": "session.idle",
            "properties": { "sessionID": session() }
        })],
        "session.execution.failed" => {
            vec![
                v2_error_payload(&data),
                json!({
                    "type": "session.idle",
                    "properties": { "sessionID": session() }
                }),
            ]
        }
        "session.step.failed" => {
            // An interrupt surfaces as step.failed{aborted}; the terminal
            // execution.interrupted frame settles the turn. Any other
            // failure is a step-level provider error: surface it as a chip,
            // the turn itself keeps going (execution.* decides its end).
            if data.pointer("/error/type").and_then(Value::as_str) == Some("aborted") {
                return Vec::new();
            }
            let mut warning = v2_error_payload(&data);
            warning["type"] = json!("session.warning");
            vec![warning]
        }
        "session.step.started" => {
            if let (Some(session_id), Some(model)) = (
                data.get("sessionID").and_then(Value::as_str),
                data.get("model").and_then(v2_model_identity),
            ) {
                session_models.insert(session_id.to_owned(), model);
            }
            vec![json!({
                "type": "message.updated",
                "properties": {
                    "info": { "sessionID": session(), "id": message(), "role": "assistant" }
                }
            })]
        }
        "session.text.started" | "session.text.ended" => {
            vec![v2_stream_part(
                &data,
                "text",
                data.get("text").cloned().unwrap_or(json!("")),
            )]
        }
        "session.text.delta" | "session.reasoning.delta" => vec![json!({
            "type": "message.part.delta",
            "properties": {
                "sessionID": session(),
                "messageID": message(),
                "partID": v2_part_id(&data, if kind == "session.reasoning.delta" { 'r' } else { 't' }),
                "field": "text",
                "delta": data.get("delta").cloned().unwrap_or(json!("")),
            }
        })],
        "session.reasoning.started" | "session.reasoning.ended" => vec![v2_stream_part(
            &data,
            "reasoning",
            data.get("text").cloned().unwrap_or(json!("")),
        )],
        "session.tool.input.started" => {
            let id = data.get("id").and_then(Value::as_str).unwrap_or_default();
            let name = data.get("name").and_then(Value::as_str).unwrap_or_default();
            tool_names.insert(tool_key(), name.to_owned());
            vec![v2_tool_part(
                &data,
                id,
                name,
                &json!({ "status": "pending" }),
            )]
        }
        "session.tool.called" => {
            let id = data.get("id").and_then(Value::as_str).unwrap_or_default();
            let name = tool_names
                .get(&tool_key())
                .map(String::as_str)
                .unwrap_or_default();
            let state = json!({
                "status": "running",
                "input": data.get("input").cloned().unwrap_or(json!({})),
            });
            vec![v2_tool_part(&data, id, name, &state)]
        }
        "session.tool.success" => {
            let id = data.get("id").and_then(Value::as_str).unwrap_or_default();
            let name = tool_names.remove(&tool_key()).unwrap_or_default();
            let output = data
                .get("content")
                .and_then(Value::as_array)
                .map(|parts| {
                    parts
                        .iter()
                        .filter_map(|p| p.get("text").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default();
            vec![v2_tool_part(
                &data,
                id,
                &name,
                &json!({ "status": "completed", "output": output }),
            )]
        }
        "session.tool.failed" | "session.tool.error" => {
            let id = data.get("id").and_then(Value::as_str).unwrap_or_default();
            let name = tool_names.remove(&tool_key()).unwrap_or_default();
            let error = data.get("error").cloned().unwrap_or(Value::Null);
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or_else(|| error.as_str().map(str::to_owned))
                .unwrap_or_default();
            vec![v2_tool_part(
                &data,
                id,
                &name,
                &json!({ "status": "error", "error": message }),
            )]
        }
        "session.step.ended" => {
            let tokens = data.get("tokens").cloned().unwrap_or(Value::Null);
            if !tokens.is_object() {
                return Vec::new();
            }
            // One step's tokens are the prompt it sent plus its reply, which
            // is what the 1.x assistant message carried. The frame omits the
            // model, so the session's latest step supplies it and the
            // advertised context limit resolves.
            let mut info = json!({
                "sessionID": session(),
                "id": "usage",
                "role": "assistant",
                "tokens": tokens,
            });
            if let Some(model) = data
                .get("sessionID")
                .and_then(Value::as_str)
                .and_then(|id| session_models.get(id))
            {
                info["providerID"] = json!(model.provider_id);
                info["modelID"] = json!(model.model_id);
            }
            vec![json!({
                "type": "message.updated",
                "properties": { "info": info }
            })]
        }
        // Cumulative session totals (every step, title, and compaction
        // summed), so they measure spend, not context occupancy.
        "session.usage.updated" => Vec::new(),
        "session.created" => {
            let Some(id) = data.get("sessionID").and_then(Value::as_str) else {
                return Vec::new();
            };
            vec![json!({
                "type": "session.created",
                "properties": {
                    "info": {
                        "id": id,
                        "parentID": data.get("parentID").cloned().unwrap_or(Value::Null),
                        "title": data
                            .get("title")
                            .or_else(|| data.get("slug"))
                            .cloned()
                            .unwrap_or(Value::Null),
                    }
                }
            })]
        }
        // Same event name as 1.x; preserve permission details for the reply
        // and any interactive approval. The route differs by generation. Observed live
        // on 2.0.3 when a tool reaches outside the workspace.
        "permission.asked" => {
            if data.get("id").and_then(Value::as_str).is_none() {
                return Vec::new();
            }
            vec![json!({
                "type": "permission.asked",
                "properties": data
            })]
        }
        _ => Vec::new(),
    }
}

/// 2.x (2.0.24+) implements the agent's `question` tool as a session FORM:
/// `form.created` carries `form.metadata.kind == "question"` and one field
/// per question (`q0`..: `title` = header, `description` = question text,
/// `options`, `multiselect` for multi-select; observed live against 2.0.24).
/// Fold it into the 1.x `question.asked` payload — the shared question flow —
/// carrying each field's key so the reply, which rides the session's form
/// route, can address it (panel ids are minted downstream). Forms from other
/// features (metadata kind absent) are not the question panel's to answer.
fn v2_question_form_payload(form: &Value) -> Vec<Value> {
    if form.pointer("/metadata/kind").and_then(Value::as_str) != Some("question") {
        return Vec::new();
    }
    let (Some(id), Some(session_id)) = (
        form.get("id").and_then(Value::as_str),
        form.get("sessionID").and_then(Value::as_str),
    ) else {
        return Vec::new();
    };
    let questions: Vec<Value> = form
        .get("fields")
        .and_then(Value::as_array)
        .map(|fields| {
            fields
                .iter()
                .filter_map(|field| {
                    let question = field
                        .get("description")
                        .and_then(Value::as_str)
                        .filter(|q| !q.trim().is_empty())
                        .or_else(|| field.get("title").and_then(Value::as_str))
                        .filter(|q| !q.trim().is_empty())?;
                    Some(json!({
                        "key": field
                            .get("key")
                            .and_then(Value::as_str)
                            .unwrap_or_default(),
                        "header": field
                            .get("title")
                            .and_then(Value::as_str)
                            .filter(|h| !h.is_empty())
                            .unwrap_or("Question"),
                        "question": question,
                        "multiple": field.get("type").and_then(Value::as_str)
                            == Some("multiselect"),
                        // The answer is the option's VALUE (the question tool
                        // sets value == label, so display stays right).
                        "options": field
                            .get("options")
                            .and_then(Value::as_array)
                            .map(|opts| {
                                opts.iter()
                                    .filter_map(|o| {
                                        o.get("value")
                                            .and_then(Value::as_str)
                                            .or_else(|| o.get("label").and_then(Value::as_str))
                                            .map(|label| json!({ "label": label }))
                                    })
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default(),
                    }))
                })
                .collect()
        })
        .unwrap_or_default();
    if questions.is_empty() {
        return Vec::new();
    }
    vec![json!({
        "type": "question.asked",
        "properties": {
            "id": id,
            "sessionID": session_id,
            "form": true,
            "questions": questions,
        }
    })]
}

/// The 2.x error shape (`{type, message}`) folded into the 1.x
/// `session.error` properties (`{name, data: {message}}`). 2.x provider
/// failures can carry an EMPTY message (observed live: `provider.auth`
/// with `message: ""`) — fall back to the type so the chip isn't blank.
fn v2_error_payload(data: &Value) -> Value {
    let error = data.get("error").cloned().unwrap_or(Value::Null);
    let name = error.get("type").and_then(Value::as_str).unwrap_or("");
    let message = match error.get("message").and_then(Value::as_str) {
        Some(message) if !message.is_empty() => message,
        _ => name,
    };
    json!({
        "type": "session.error",
        "properties": {
            "sessionID": data.get("sessionID").cloned().unwrap_or(Value::Null),
            "error": {
                "name": name,
                "data": { "message": message },
            },
        }
    })
}

/// Synthetic part id for a streamed 2.x text/reasoning item: message id +
/// kind + ordinal (text and reasoning number separately).
fn v2_part_id(data: &Value, kind: char) -> String {
    let message = data
        .get("assistantMessageID")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let ordinal = data.get("ordinal").and_then(Value::as_u64).unwrap_or(0);
    format!("{message}:{kind}{ordinal}")
}

/// A text/reasoning item snapshot in the 1.x part shape.
fn v2_stream_part(data: &Value, part_type: &str, text: Value) -> Value {
    let kind_char = if part_type == "text" { 't' } else { 'r' };
    json!({
        "type": "message.part.updated",
        "properties": { "part": {
            "sessionID": data.get("sessionID").cloned().unwrap_or(Value::Null),
            "id": v2_part_id(data, kind_char),
            "messageID": data.get("assistantMessageID").cloned().unwrap_or(Value::Null),
            "type": part_type,
            "text": text,
        }}
    })
}

/// A tool frame in the 1.x tool-part shape (`callID` keys the chip).
fn v2_tool_part(data: &Value, id: &str, name: &str, state: &Value) -> Value {
    // Provider call ids can repeat across assistant messages. Both the feed's
    // state key and the emitted chip id must keep those calls distinct.
    let id = format!(
        "{}:{}:{id}",
        data.get("sessionID")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        data.get("assistantMessageID")
            .and_then(Value::as_str)
            .unwrap_or_default()
    );
    json!({
        "type": "message.part.updated",
        "properties": { "part": {
            "sessionID": data.get("sessionID").cloned().unwrap_or(Value::Null),
            "messageID": data.get("assistantMessageID").cloned().unwrap_or(Value::Null),
            "id": id,
            "callID": id,
            "type": "tool",
            "tool": name,
            "state": state,
        }}
    })
}

/// Tail the event bus into the session loop: `/global/event` on 1.x,
/// `/api/event` on 2.x (which serves nothing unless `Accept:
/// text/event-stream` is sent — observed live, 2.0.3). Reconnects on
/// transient drops (the server is our own child on loopback); past the
/// budget the loop learns via [`BusMsg::Disconnected`] and errors the run —
/// missed frames mean the transcript can no longer be trusted. A server
/// that stops serving without exiting ends it via [`BusMsg::Unresponsive`]
/// ([`BUS_PROBE_TIMEOUT`], [`BUS_WEDGE_BUDGET`]).
async fn bus_task(
    base: String,
    auth: Option<String>,
    protocol: Protocol,
    tx: mpsc::Sender<BusMsg>,
    timeouts: BusTimeouts,
) {
    let BusTimeouts {
        silence,
        probe,
        wedged,
    } = timeouts;
    let client = http_client();
    let url = match protocol {
        Protocol::V1 => format!("{base}/global/event"),
        Protocol::V2 => format!("{base}/api/event"),
    };
    // 2.1.2 answers 404 on `/api/health`; `/api/info` is its live route.
    let health = match protocol {
        Protocol::V1 => format!("{base}/global/health"),
        Protocol::V2 => format!("{base}/api/info"),
    };
    // The server just answered its startup and session calls.
    let mut last_frame = tokio::time::Instant::now();
    // Attempts in a row that ended in silence.
    let mut silences: u32 = 0;
    let mut failures: u32 = 0;
    let mut v2_tool_names: HashMap<V2ToolKey, String> = HashMap::new();
    let mut v2_session_models: HashMap<String, V2ModelIdentity> = HashMap::new();
    loop {
        if tx.is_closed() {
            return;
        }
        let mut req = client
            .get(&url)
            .header(reqwest::header::ACCEPT, "text/event-stream");
        if let Some(auth) = &auth {
            req = req.header(reqwest::header::AUTHORIZATION, auth.clone());
        }
        // A server that accepts the connection but never answers it must
        // not park the bus forever either.
        let attempt = tokio::time::Instant::now();
        match tokio::time::timeout(silence, req.send()).await {
            Ok(Ok(resp)) if resp.status().is_success() => {
                let framed = stream_bus(
                    &tx,
                    resp,
                    protocol,
                    silence,
                    &mut last_frame,
                    &mut v2_tool_names,
                    &mut v2_session_models,
                )
                .await;
                if tx.is_closed() {
                    return;
                }
                // Only a stream that delivered a frame proves the server
                // healthy: one accepted and closed (or wedged) before its
                // first frame keeps counting toward the budget, or an
                // accept-then-close server would be retried forever.
                if framed {
                    failures = 0;
                }
            }
            _ => {}
        }
        // (stream_bus sends Connected itself once the first frame lands —
        // an accepted-but-parked boot-window connection must not count.)
        let quiet = last_frame.elapsed();
        if quiet < silence {
            silences = 0;
        } else {
            // A stream that framed, then fell silent, is the first silence.
            silences = if last_frame >= attempt {
                1
            } else {
                silences + 1
            };
            // Any answer at all proves the process is serving.
            let mut check = client.get(&health).timeout(probe);
            if let Some(auth) = &auth {
                check = check.header(reqwest::header::AUTHORIZATION, auth.clone());
            }
            let unresponsive = if check.send().await.is_err() {
                Some(format!(
                    "opencode server stopped responding: no event for {}s, and no answer \
                     to a health check within {}s",
                    quiet.as_secs(),
                    probe.as_secs_f32()
                ))
            } else {
                // Two silences waited out, so a clock that jumped across a
                // machine sleep (one timer firing late) never trips it.
                (silences >= 2 && quiet >= wedged).then(|| {
                    format!(
                        "opencode server stopped responding: no event for {}s",
                        quiet.as_secs()
                    )
                })
            };
            if let Some(message) = unresponsive {
                let message =
                    format!("{message}. It was shut down; the next message starts a fresh one");
                let _ = tx.send(BusMsg::Unresponsive(message)).await;
                return;
            }
        }
        failures += 1;
        if failures > BUS_RECONNECT_ATTEMPTS {
            let _ = tx.send(BusMsg::Disconnected).await;
            return;
        }
        tokio::time::sleep(BUS_RECONNECT_DELAY).await;
    }
}

/// Forward one stream's events; returns whether it delivered a frame, and
/// stamps `last_frame` with each. A stream silent for `silence` (heartbeats
/// included) is abandoned so the caller reconnects.
async fn stream_bus(
    tx: &mpsc::Sender<BusMsg>,
    resp: reqwest::Response,
    protocol: Protocol,
    silence: Duration,
    last_frame: &mut tokio::time::Instant,
    v2_tool_names: &mut HashMap<V2ToolKey, String>,
    v2_session_models: &mut HashMap<String, V2ModelIdentity>,
) -> bool {
    let mut stream = resp.bytes_stream();
    let mut decoder = SseDecoder::default();
    let mut announced = false;
    loop {
        let bytes = match tokio::time::timeout(silence, stream.next()).await {
            Ok(Some(Ok(bytes))) => bytes,
            Ok(Some(Err(_)) | None) => return announced,
            Err(_) => {
                tracing::warn!(
                    target: "zeron_harness::opencode",
                    "event bus silent for {}s; reconnecting",
                    silence.as_secs()
                );
                return announced;
            }
        };
        let frames = decoder.frames;
        let events = decoder.feed(&bytes);
        if decoder.frames > frames {
            *last_frame = tokio::time::Instant::now();
        }
        if !announced && decoder.frames > 0 {
            announced = true;
            if tx.send(BusMsg::Connected).await.is_err() {
                return announced;
            }
        }
        for data in events {
            let Ok(event) = serde_json::from_str::<Value>(&data) else {
                continue;
            };
            if protocol == Protocol::V2 {
                let payloads =
                    normalize_v2_frame_with_session_models(event, v2_tool_names, v2_session_models);
                if v2_tool_names.len() > MAX_PENDING_V2_TOOLS {
                    let _ = tx.send(BusMsg::Disconnected).await;
                    return announced;
                }
                // Model identity only feeds usage frames. Dropping the
                // cache degrades to "window preserved" and refills on the
                // next step.started, so — unlike leaked pending tools — it
                // must not fail the run, especially now that the cache
                // survives reconnects.
                if v2_session_models.len() > MAX_V2_SESSION_MODELS {
                    v2_session_models.clear();
                }
                for payload in payloads {
                    if tx.send(BusMsg::Event(payload)).await.is_err() {
                        return announced;
                    }
                }
            } else if tx.send(BusMsg::Event(event)).await.is_err() {
                return announced;
            }
        }
    }
}

/// Incremental `text/event-stream` decoder (the WHATWG framing). Lines end
/// in LF, CR or CRLF — a CR closing one chunk may pair with the LF opening
/// the next — `:` lines are comments, an event's `data` lines join with LF,
/// and a blank line dispatches it. Each line decodes lossily on its own, so
/// an invalid byte costs a character rather than the event, and every byte
/// is scanned once however large a frame grows.
#[derive(Default)]
struct SseDecoder {
    line: Vec<u8>,
    data: String,
    has_data: bool,
    /// The current block has seen a line (a comment or any field).
    in_frame: bool,
    /// The last chunk ended in CR: an LF opening the next one belongs to it.
    after_cr: bool,
    /// The first line has been read (a leading byte-order mark is dropped).
    started: bool,
    /// Complete blocks seen, comment-only ones (heartbeats) included.
    frames: u64,
}

impl SseDecoder {
    /// Feed one chunk; returns the data of every event it completed.
    fn feed(&mut self, mut bytes: &[u8]) -> Vec<String> {
        let mut events = Vec::new();
        if bytes.is_empty() {
            return events;
        }
        if std::mem::take(&mut self.after_cr) && bytes[0] == b'\n' {
            bytes = &bytes[1..];
        }
        while let Some(end) = bytes.iter().position(|b| matches!(b, b'\n' | b'\r')) {
            self.line.extend_from_slice(&bytes[..end]);
            let cr = bytes[end] == b'\r';
            bytes = &bytes[end + 1..];
            if cr {
                match bytes.first() {
                    Some(b'\n') => bytes = &bytes[1..],
                    Some(_) => {}
                    None => self.after_cr = true,
                }
            }
            self.end_line(&mut events);
        }
        self.line.extend_from_slice(bytes);
        events
    }

    fn end_line(&mut self, events: &mut Vec<String>) {
        let raw = std::mem::take(&mut self.line);
        let decoded = String::from_utf8_lossy(&raw);
        let mut line: &str = &decoded;
        if !std::mem::replace(&mut self.started, true) {
            line = line.strip_prefix('\u{feff}').unwrap_or(line);
        }
        if line.is_empty() {
            if std::mem::take(&mut self.in_frame) {
                self.frames += 1;
            }
            if std::mem::take(&mut self.has_data) {
                events.push(std::mem::take(&mut self.data));
            }
            return;
        }
        self.in_frame = true;
        if line.starts_with(':') {
            return;
        }
        let (field, value) = line.split_once(':').unwrap_or((line, ""));
        if field == "data" {
            if self.has_data {
                self.data.push('\n');
            }
            self.data.push_str(value.strip_prefix(' ').unwrap_or(value));
            self.has_data = true;
        }
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod context_tests {
    use super::*;
    #[test]
    fn context_includes_cache_and_uses_reported_model_limit() {
        let windows = HashMap::from([("provider/model".into(), 200000)]);
        let info = json!({"providerID":"provider","modelID":"model", "tokens": {
            "input": 200, "output": 100, "reasoning": 50, "cache": {"read":40000,"write":1800}
        }});
        assert_eq!(
            context_usage_event(&info, &windows),
            Some(AgentEvent::ContextUsage {
                tokens: Some(42100),
                window: Some(200000)
            })
        );
        assert_eq!(
            context_usage_event(&json!({"tokens":{"input":0,"output":0}}), &windows),
            None
        );
        assert_eq!(
            context_usage_event(
                &json!({"time":{"completed":1},"tokens":{"input":0,"output":0}}),
                &windows
            ),
            Some(AgentEvent::ContextUsage {
                tokens: Some(0),
                window: None
            })
        );
    }

    #[test]
    fn malformed_usage_is_ignored_without_panicking() {
        let windows = HashMap::from([("provider/model".to_owned(), 200_000)]);
        for info in [
            json!({}),
            json!({"tokens": null}),
            json!({"tokens": "12"}),
            json!({"tokens": [1, 2]}),
            json!({"tokens": {"input": -1, "output": 1.5, "cache": "x"}}),
            json!({"providerID": 1, "modelID": null, "tokens": {"input": "3"}}),
        ] {
            assert_eq!(context_usage_event(&info, &windows), None, "{info}");
        }
        // Unexpected model fields drop only the window, never the count.
        assert_eq!(
            context_usage_event(
                &json!({"providerID": ["provider"], "modelID": "model",
                        "tokens": {"input": 7, "cache": {"read": "x", "write": 3}}}),
                &windows
            ),
            Some(AgentEvent::ContextUsage {
                tokens: Some(10),
                window: None
            })
        );
    }

    #[test]
    fn unknown_model_preserves_the_previous_window() {
        // A catalog that failed to load (or lags the 2.x model sync)
        // advertises nothing; an absent model must not read as "no limit".
        let info = json!({"providerID":"provider","modelID":"unknown",
                          "tokens":{"input":10,"output":2}});
        assert_eq!(
            context_usage_event(&info, &HashMap::new()),
            Some(AgentEvent::ContextUsage {
                tokens: Some(12),
                window: None
            })
        );
    }
}

/// Budget for a `--version` the shared probe gave up on: a freshly installed
/// binary's first exec waits on the OS malware scan (1.1s on an M-series Mac
/// for 2.0.20, longer on slower machines and under Windows Defender).
const COLD_VERSION_TIMEOUT: Duration = Duration::from_secs(30);

/// The shared probe caps `--version` at 2s and caches a timeout for the
/// binary's lifetime, so one cold first exec after an upgrade failed every
/// later run instantly until restart. Retry past that cache with a longer
/// budget, and on success clear the cached failure for other callers.
async fn opencode_version(exe: &std::path::Path) -> Option<semver::Version> {
    let cached_exe = exe.to_path_buf();
    let cached =
        tokio::task::spawn_blocking(move || crate::executable::binary_version(&cached_exe))
            .await
            .ok()
            .flatten();
    if cached.is_some() {
        return cached;
    }
    let mut cmd = Command::new(exe);
    cmd.arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let output = tokio::time::timeout(COLD_VERSION_TIMEOUT, cmd.output())
        .await
        .ok()?
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let version = crate::executable::parse_version(&output.stdout)
        .or_else(|| crate::executable::parse_version(&output.stderr))?;
    if let Some(stem) = exe.file_stem().and_then(|s| s.to_str()) {
        crate::executable::invalidate_versions(&[stem]);
    }
    Some(version)
}

/// Inline config is the final user config layer. Preserve inherited overrides
/// and other MCP servers; never write chat identity into a shared config file.
fn mcp_config(
    inherited: Option<&str>,
    mcp: &zeron_proto::McpServer,
    protocol: Protocol,
) -> Result<String, HarnessError> {
    let mut config: Value = match inherited.filter(|s| !s.trim().is_empty()) {
        Some(raw) => deser_hjson::from_str(raw).map_err(|_| {
            HarnessError::Protocol("OPENCODE_CONFIG_CONTENT must be a valid config object".into())
        })?,
        None => json!({}),
    };
    let object = config.as_object_mut().ok_or_else(|| {
        HarnessError::Protocol("OPENCODE_CONFIG_CONTENT must be an object".into())
    })?;
    let servers = object.entry("mcp").or_insert_with(|| json!({}));
    let servers = if protocol == Protocol::V2 {
        servers
            .as_object_mut()
            .ok_or_else(|| {
                HarnessError::Protocol("OPENCODE_CONFIG_CONTENT.mcp must be an object".into())
            })?
            .entry("servers")
            .or_insert_with(|| json!({}))
    } else {
        servers
    };
    let servers = servers.as_object_mut().ok_or_else(|| {
        HarnessError::Protocol("OPENCODE_CONFIG_CONTENT.mcp must be an object".into())
    })?;
    let command: Vec<&str> = std::iter::once(mcp.command.as_str())
        .chain(mcp.args.iter().map(String::as_str))
        .collect();
    let mut server = json!({"type": "local", "command": command, "environment": mcp.env});
    match protocol {
        Protocol::V1 => server["enabled"] = json!(true),
        Protocol::V2 => server["disabled"] = json!(false),
    }
    servers.insert(mcp.name.clone(), server);
    Ok(config.to_string())
}

#[cfg(test)]
mod mcp_injection_tests {
    use super::*;

    #[cfg(unix)]
    #[tokio::test]
    async fn mcp_injection_reaches_isolated_server_processes() {
        use std::os::unix::fs::PermissionsExt;
        let fixture = tempfile::tempdir().unwrap();
        for major in [1, 2] {
            let exe = fixture.path().join(format!("opencode-{major}"));
            let script = format!(
                r#"#!/usr/bin/env node
const http = require('node:http');
const version = '{major}.0.0';
if (process.argv.includes('--version')) {{ console.log(version); process.exit(0); }}
const config = JSON.parse(process.env.OPENCODE_CONFIG_CONTENT);
const server = {major} === 1 ? config.mcp.zeron : config.mcp.servers.zeron;
if (!server || server.command[1] !== 'mcp') throw new Error('missing MCP config');
const port = Number(process.argv[process.argv.indexOf('--port') + 1]);
http.createServer((req, res) => {{
  res.setHeader('content-type', 'application/json');
  if (req.url === '/config-probe') {{ res.end(JSON.stringify(server)); return; }}
  if (req.url === ({major} === 1 ? '/global/health' : '/api/info')) {{
    res.end(JSON.stringify({{version}})); return;
  }}
  res.statusCode = 404; res.end('{{}}');
}}).listen(port, '127.0.0.1');
"#
            );
            std::fs::write(&exe, script).unwrap();
            std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o700)).unwrap();
            let first = zeron_proto::McpServer {
                name: "zeron".into(),
                command: "/path with spaces/zeron".into(),
                args: vec!["mcp".into()],
                env: [("ZERON_CHAT_ID".into(), "first".into())].into(),
            };
            let mut second = first.clone();
            second.env.insert("ZERON_CHAT_ID".into(), "second".into());
            let mut a = Server::spawn(
                &exe,
                fixture.path().to_str(),
                Duration::from_secs(5),
                Some(&first),
            )
            .await
            .unwrap();
            let mut b = Server::spawn(
                &exe,
                fixture.path().to_str(),
                Duration::from_secs(5),
                Some(&second),
            )
            .await
            .unwrap();
            let a_config = a.get_json("/config-probe", None).await.unwrap();
            let b_config = b.get_json("/config-probe", None).await.unwrap();
            a.shutdown(Duration::from_millis(100)).await;
            b.shutdown(Duration::from_millis(100)).await;
            assert_eq!(a_config["environment"]["ZERON_CHAT_ID"], "first");
            assert_eq!(b_config["environment"]["ZERON_CHAT_ID"], "second");
            assert_eq!(
                a_config["command"],
                json!(["/path with spaces/zeron", "mcp"])
            );
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn startup_retries_a_port_taken_before_bind() {
        use std::os::unix::fs::PermissionsExt;
        let fixture = tempfile::tempdir().unwrap();
        let exe = fixture.path().join("opencode");
        // The first launch loses the port race the way Node reports it: the
        // EADDRINUSE line, then more stack than the stderr tail keeps.
        let script = r#"#!/usr/bin/env node
const fs = require('node:fs');
const http = require('node:http');
if (process.argv.includes('--version')) { console.log('1.14.0'); process.exit(0); }
const raced = require('node:path').join(__dirname, 'raced');
if (!fs.existsSync(raced)) {
  fs.writeFileSync(raced, '');
  console.error('Error: listen EADDRINUSE: address already in use 127.0.0.1:1');
  for (let i = 0; i < 12; i++) console.error(`    at frame${i} (node:net:${i})`);
  process.exit(1);
}
const port = Number(process.argv[process.argv.indexOf('--port') + 1]);
http.createServer((req, res) => {
  res.setHeader('content-type', 'application/json');
  res.end(JSON.stringify({version: '1.14.0'}));
}).listen(port, '127.0.0.1', () => console.log(`opencode server listening on http://127.0.0.1:${port}`));
"#;
        std::fs::write(&exe, script).unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut server = Server::spawn(&exe, fixture.path().to_str(), Duration::from_secs(5), None)
            .await
            .unwrap();
        server.shutdown(Duration::from_millis(100)).await;
        assert!(fixture.path().join("raced").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unknown_version_starts_without_mcp_instead_of_failing() {
        use std::os::unix::fs::PermissionsExt;
        let fixture = tempfile::tempdir().unwrap();
        let exe = fixture.path().join("opencode");
        // `--version` fails (a probe that timed out or printed no version).
        let script = r#"#!/usr/bin/env node
const http = require('node:http');
if (process.argv.includes('--version')) process.exit(1);
const config = process.env.OPENCODE_CONFIG_CONTENT ?? null;
const port = Number(process.argv[process.argv.indexOf('--port') + 1]);
http.createServer((req, res) => {
  res.setHeader('content-type', 'application/json');
  if (req.url === '/config-probe') { res.end(JSON.stringify({config})); return; }
  if (req.url === '/api/info') { res.end(JSON.stringify({version: '2.0.20'})); return; }
  res.statusCode = 404; res.end('{}');
}).listen(port, '127.0.0.1');
"#;
        std::fs::write(&exe, script).unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o700)).unwrap();
        let mcp = zeron_proto::McpServer {
            name: "zeron".into(),
            command: "zeron".into(),
            args: vec!["mcp".into()],
            env: Default::default(),
        };
        let mut server = Server::spawn(
            &exe,
            fixture.path().to_str(),
            Duration::from_secs(5),
            Some(&mcp),
        )
        .await
        .expect("an unknown version must not fail the run");
        let probe = server.get_json("/config-probe", None).await.unwrap();
        server.shutdown(Duration::from_millis(100)).await;
        assert_eq!(probe["config"], Value::Null);
    }

    /// A freshly installed binary's first `--version` outlasts the shared
    /// 2s probe (OS malware scan). That cached failure used to fail every
    /// run; the cold retry must still find 2.x and inject the MCP config.
    #[cfg(unix)]
    #[tokio::test]
    async fn cold_first_version_probe_still_injects_mcp() {
        use std::os::unix::fs::PermissionsExt;
        let fixture = tempfile::tempdir().unwrap();
        let exe = fixture.path().join("opencode");
        let warmed = fixture.path().join("warmed");
        let script = format!(
            r#"#!/usr/bin/env node
const fs = require('node:fs');
const http = require('node:http');
if (process.argv.includes('--version')) {{
  const cold = !fs.existsSync({warmed:?});
  fs.writeFileSync({warmed:?}, '');
  setTimeout(() => {{ console.log('opencode v2.0.20'); process.exit(0); }}, cold ? 3000 : 0);
}} else {{
  const config = JSON.parse(process.env.OPENCODE_CONFIG_CONTENT);
  const auth = {{
    password: process.env.OPENCODE_PASSWORD,
    legacy: process.env.OPENCODE_SERVER_PASSWORD,
  }};
  const port = Number(process.argv[process.argv.indexOf('--port') + 1]);
  http.createServer((req, res) => {{
    res.setHeader('content-type', 'application/json');
    if (req.url === '/config-probe') {{
      res.end(JSON.stringify({{server: config.mcp.servers.zeron, auth}})); return;
    }}
    if (req.url === '/api/info') {{ res.end(JSON.stringify({{version: '2.0.20'}})); return; }}
    res.statusCode = 404; res.end('{{}}');
  }}).listen(port, '127.0.0.1');
}}
"#
        );
        std::fs::write(&exe, script).unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o700)).unwrap();
        let mcp = zeron_proto::McpServer {
            name: "zeron".into(),
            command: "zeron".into(),
            args: vec!["mcp".into()],
            env: Default::default(),
        };
        for _ in 0..2 {
            let mut server = Server::spawn(
                &exe,
                fixture.path().to_str(),
                Duration::from_secs(10),
                Some(&mcp),
            )
            .await
            .expect("a slow first --version must not fail the run");
            let probe = server.get_json("/config-probe", None).await.unwrap();
            server.shutdown(Duration::from_millis(100)).await;
            assert_eq!(probe["server"]["command"], json!(["zeron", "mcp"]));
            // 2.x reads OPENCODE_PASSWORD first: both must carry ours.
            assert!(probe["auth"]["password"].is_string());
            assert_eq!(probe["auth"]["password"], probe["auth"]["legacy"]);
        }
    }

    #[test]
    fn mcp_injection_preserves_config_and_scopes_identity_for_both_protocols() {
        let mut mcp = zeron_proto::McpServer {
            name: "zeron".into(),
            command: "/path with spaces/zeron".into(),
            args: vec!["mcp".into()],
            env: [("ZERON_CHAT_ID".into(), "first".into())].into(),
        };
        for protocol in [Protocol::V1, Protocol::V2] {
            let inherited = match protocol {
                Protocol::V1 => {
                    r#"{"model":"keep", "mcp":{"user":{"type":"remote","url":"https://example.test"}}}"#
                }
                Protocol::V2 => {
                    r#"{"model":"keep", "mcp":{"servers":{"user":{"type":"remote","url":"https://example.test"}}}}"#
                }
            };
            let first: Value =
                serde_json::from_str(&mcp_config(Some(inherited), &mcp, protocol).unwrap())
                    .unwrap();
            mcp.env.insert("ZERON_CHAT_ID".into(), "second".into());
            let second: Value =
                serde_json::from_str(&mcp_config(Some(inherited), &mcp, protocol).unwrap())
                    .unwrap();
            assert_eq!(first["model"], "keep");
            let pointer = if protocol == Protocol::V1 {
                "/mcp"
            } else {
                "/mcp/servers"
            };
            let servers = first.pointer(pointer).unwrap();
            assert_eq!(servers["user"]["url"], "https://example.test");
            assert_eq!(
                servers["zeron"]["command"],
                json!(["/path with spaces/zeron", "mcp"])
            );
            assert_eq!(servers["zeron"]["environment"]["ZERON_CHAT_ID"], "first");
            assert_eq!(
                second.pointer(pointer).unwrap()["zeron"]["environment"]["ZERON_CHAT_ID"],
                "second"
            );
            if protocol == Protocol::V1 {
                assert_eq!(servers["zeron"]["enabled"], true);
            } else {
                assert_eq!(servers["zeron"]["disabled"], false);
                assert!(servers["zeron"].get("enabled").is_none());
            }
            for invalid in ["[]", "{", r#"{"mcp":false}"#] {
                assert!(mcp_config(Some(invalid), &mcp, protocol).is_err());
            }
            mcp.env.insert("ZERON_CHAT_ID".into(), "first".into());
        }
    }
}
