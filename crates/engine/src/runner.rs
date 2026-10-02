//! Cloud runner mode (docs/design/cloud-device.md, "Runner identity").
//!
//! A Cloud device is an ordinary headless engine whose identity is not a
//! WorkOS session but an Ed25519 key it enrolled with the edge once:
//!
//! - **Detection** at boot: `ZERON_RUNNER_ENROLL={orgId}.{userId}.{deviceId}.{code}`
//!   naming the device `{data_dir}/runner.json` already holds (or no env) →
//!   runner as persisted; the env naming a DIFFERENT device (or no
//!   runner.json) → enroll as that device. A sandbox forked or restored from
//!   a template carries the old disk — runner.json, runner-key, device-id and
//!   the broker's managed credential files — so a mismatched enrollment first
//!   discards that copied identity (never a native Claude login, never user
//!   files or repos). Neither → an ordinary laptop/server engine.
//! - **Enrollment**: generate (or reuse) the key at `{data_dir}/runner-key`
//!   (PKCS#8, 0600 — always a NEW key: the vault never re-admits a revoked one),
//!   `POST {edge}/runner/enroll`, then persist `runner.json`
//!   (0600) and `{data_dir}/device-id` = the edge-minted device id. Network
//!   failures retry with backoff; a 4xx is fatal (the one-time code is spent).
//! - **Tokens**: `POST {edge}/runner/token` signed over
//!   `zeron-runner-token\n{orgId}\n{userId}\n{deviceId}\n{ts}` — driven by
//!   [`crate::Auth`]'s runner mode, so every room/relay/RPC bearer consumer
//!   works unchanged.
//! - **Heartbeat**: `POST {edge}/runner/heartbeat {activeRuns, clients}` every
//!   60 s, feeding the CloudAccount DO's per-session idle sleep.
//! - **Session**: every Cloud session runs in its own sandbox, provisioned with
//!   `ZERON_CLOUD_{ACCOUNT,CHAT,REPO,PATH,BRANCH}` (persisted to
//!   `cloud-session.json` for restarts). Before the engine executes anything
//!   it clones the project's repository and checks out the session's branch
//!   ([`prepare_checkout`]); its device row carries the `cloud-session`
//!   capability so device lists show the account's one Cloud device instead.
//!
//! The device's platform string comes from `ZERON_DEVICE_PLATFORM` (`cloud`
//! on a Cloud device); a `cloud` platform restricts the harness catalog to
//! Codex + Claude Code (see [`crate::registry`]).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
use ring::signature::{Ed25519KeyPair, KeyPair};
use serde::{Deserialize, Serialize};

use crate::EngineError;
use crate::http_error::describe_http_error;

/// `{data_dir}/cloud-session.json` — this session machine's project.
pub const SESSION_FILE: &str = "cloud-session.json";

/// What a session sandbox was provisioned for: the account's logical Cloud
/// device, the chat it runs, and the repository it works in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudSession {
    /// The account's logical Cloud device (`ZERON_CLOUD_ACCOUNT`).
    pub account: String,
    /// The session's top-level chat (`ZERON_CLOUD_CHAT`).
    pub chat_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
}

impl CloudSession {
    fn from_vars(var: impl Fn(&str) -> Option<String>) -> Option<Self> {
        let get = |name: &str| {
            var(name)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        Some(Self {
            account: get("ZERON_CLOUD_ACCOUNT")?,
            chat_id: get("ZERON_CLOUD_CHAT")?,
            repo: get("ZERON_CLOUD_REPO"),
            path: get("ZERON_CLOUD_PATH"),
            branch: get("ZERON_CLOUD_BRANCH"),
        })
    }

    /// From the provisioning env (persisted for the next boot), else from the
    /// persisted file. `None` on a laptop and on a non-session runner.
    pub fn resolve(data_dir: &Path) -> Option<Self> {
        Self::resolve_with(data_dir, |name| std::env::var(name).ok())
    }

    pub fn resolve_with(data_dir: &Path, var: impl Fn(&str) -> Option<String>) -> Option<Self> {
        let file = data_dir.join(SESSION_FILE);
        if let Some(session) = Self::from_vars(var) {
            match serde_json::to_vec_pretty(&session) {
                Ok(bytes) => {
                    if let Err(err) = write_private_atomic(&file, &bytes) {
                        tracing::warn!(error = %err, "runner: could not persist the Cloud session");
                    }
                }
                Err(err) => {
                    tracing::warn!(error = %err, "runner: could not encode the Cloud session")
                }
            }
            return Some(session);
        }
        let raw = std::fs::read(&file).ok()?;
        serde_json::from_slice(&raw).ok()
    }

    /// The branch this session works on: one per session, so parallel
    /// sessions on one repository never share a branch.
    pub fn session_branch(&self) -> String {
        // 12 alphanumerics of the chat id (a UUID's dashes skipped): unique
        // enough that two sessions on one repository never push one branch.
        let short: String = self
            .chat_id
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .take(12)
            .collect();
        format!("zeron/cloud-{short}")
    }

    /// The session repository as GitHub's `owner/name` (what a `github`
    /// vault grant names), when it is a github.com repository.
    pub fn github_repo(&self) -> Option<String> {
        github_full_name(self.repo.as_deref()?)
    }
}

/// `owner/name` of a github.com clone URL (`https://github.com/o/n(.git)`,
/// `git@github.com:o/n.git`), else `None`.
pub fn github_full_name(url: &str) -> Option<String> {
    let url = url.trim();
    let rest = url
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("http://github.com/"))
        .or_else(|| url.strip_prefix("git@github.com:"))
        .or_else(|| url.strip_prefix("ssh://git@github.com/"))?;
    let rest = rest.trim_end_matches('/');
    let rest = rest.strip_suffix(".git").unwrap_or(rest);
    let (owner, name) = rest.split_once('/')?;
    let valid = |part: &str| {
        !part.is_empty()
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    (valid(owner) && valid(name)).then(|| format!("{owner}/{name}"))
}

static SESSION: std::sync::OnceLock<Option<CloudSession>> = std::sync::OnceLock::new();

/// This process's Cloud session (set once at runner boot).
pub fn session() -> Option<&'static CloudSession> {
    SESSION.get().and_then(Option::as_ref)
}

/// Record the session resolved at boot (first call wins).
pub fn set_session(session: Option<CloudSession>) {
    let _ = SESSION.set(session);
}

/// Clone the session's repository into its path and check out the session
/// branch, before the engine executes any of the chat's commands. Idempotent
/// across restarts and wakes: an existing checkout is left as it is. Git
/// authenticates through the credential store the broker keeps for the
/// GitHub grant; public repositories clone without one.
pub async fn prepare_checkout(
    session: &CloudSession,
    credentials: Option<&crate::credentials::CredentialBroker>,
) -> Result<(), EngineError> {
    prepare_checkout_with(session, credentials, Duration::from_secs(2)).await
}

async fn prepare_checkout_with(
    session: &CloudSession,
    credentials: Option<&crate::credentials::CredentialBroker>,
    retry_base: Duration,
) -> Result<(), EngineError> {
    let (Some(repo), Some(path)) = (session.repo.as_deref(), session.path.as_deref()) else {
        return Ok(());
    };
    let path = Path::new(path);
    if path.join(".git").exists() {
        // The disk survived a stop: this boot is a wake.
        record_setup("wake", "Woke the machine".into(), None, None, true);
        return Ok(());
    }
    record_setup("start", "Started a machine".into(), None, None, true);
    let full_name = github_full_name(repo).unwrap_or_else(|| repo.to_string());
    let clone_started = std::time::Instant::now();
    let no_credentials = match credentials {
        Some(broker) => broker.ensure_git_credentials().await.err(),
        None => None,
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut delay = retry_base;
    let mut last = String::new();
    for attempt in 1..=4 {
        let mut clone = tokio::process::Command::new("git");
        clone.arg("clone");
        if let Some(branch) = session.branch.as_deref() {
            clone.args(["--branch", branch]);
        }
        clone.arg("--").arg(repo).arg(path);
        clone.env("GIT_TERMINAL_PROMPT", "0").kill_on_drop(true);
        match clone.output().await {
            Ok(out) if out.status.success() => {
                last.clear();
                break;
            }
            Ok(out) => last = String::from_utf8_lossy(&out.stderr).trim().to_string(),
            Err(err) => last = err.to_string(),
        }
        // A half-written clone would make the next attempt fail on the
        // existing directory; it never held anything of the user's.
        let _ = std::fs::remove_dir_all(path);
        tracing::warn!(attempt, error = %last, "runner: cloning the session's repository failed");
        if attempt < 4 {
            tokio::time::sleep(delay).await;
            delay *= 2;
        }
    }
    let clone_command = format!(
        "git clone{} {repo} {}",
        session
            .branch
            .as_deref()
            .map(|b| format!(" --branch {b}"))
            .unwrap_or_default(),
        path.display()
    );
    if !last.is_empty() {
        let github = no_credentials
            .map(|reason| format!(" (no GitHub access: {reason})"))
            .unwrap_or_default();
        let message = format!("couldn't clone {repo}: {last}{github}");
        record_setup(
            "clone",
            format!("Couldn't clone {full_name}"),
            Some(clone_command),
            Some(message.clone()),
            false,
        );
        return Err(EngineError::Other(message));
    }
    record_setup(
        "clone",
        format!("Cloned {full_name}"),
        Some(clone_command),
        Some(format!("in {}", format_wait(clone_started.elapsed()))),
        true,
    );
    let branch = session.session_branch();
    let checkout = tokio::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["checkout", "-B", &branch])
        .kill_on_drop(true)
        .output()
        .await?;
    let checkout_command = format!("git checkout -B {branch}");
    if checkout.status.success() {
        record_setup(
            "branch",
            format!("Checked out {branch}"),
            Some(checkout_command),
            None,
            true,
        );
    } else {
        let stderr = String::from_utf8_lossy(&checkout.stderr).trim().to_string();
        tracing::warn!(branch, error = %stderr,
            "runner: couldn't create the session branch; staying on the default branch");
        record_setup(
            "branch",
            format!("Couldn't check out {branch}"),
            Some(checkout_command),
            Some(stderr),
            false,
        );
    }
    tracing::info!(repo, path = %path.display(), branch, "runner: session checkout ready");
    Ok(())
}

/// One setup step this boot performed, waiting for the first run to show it
/// ([`take_setup_parts`]).
struct SetupStep {
    step: &'static str,
    name: String,
    command: Option<String>,
    output: Option<String>,
    ok: bool,
}

static SETUP: Mutex<Vec<SetupStep>> = Mutex::new(Vec::new());

fn record_setup(
    step: &'static str,
    name: String,
    command: Option<String>,
    output: Option<String>,
    ok: bool,
) {
    SETUP
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(SetupStep {
            step,
            name,
            command,
            output,
            ok,
        });
}

/// Whether this boot has setup steps no run has shown yet.
pub fn has_setup_steps() -> bool {
    !SETUP
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .is_empty()
}

/// This boot's setup steps as tool chips (taken once: they open the first
/// answer after the machine came up). `waited` — how long the message that
/// brought the machine up waited for it — goes on the start/wake step.
pub fn take_setup_parts(waited: Option<Duration>) -> Vec<zeron_doc::MessagePart> {
    let steps = std::mem::take(&mut *SETUP.lock().unwrap_or_else(PoisonError::into_inner));
    setup_parts(steps, waited)
}

fn setup_parts(steps: Vec<SetupStep>, waited: Option<Duration>) -> Vec<zeron_doc::MessagePart> {
    steps
        .into_iter()
        .map(|step| {
            let mut input = serde_json::json!({ zeron_proto::CLOUD_SETUP_KEY: step.step });
            if let Some(command) = &step.command {
                input["command"] = serde_json::Value::String(command.clone());
            }
            let output = match (step.step, waited) {
                ("start" | "wake", Some(waited)) => {
                    Some(format!("ready after {}", format_wait(waited)))
                }
                _ => step.output,
            };
            zeron_doc::MessagePart::Tool {
                id: format!("cloud-setup-{}", crate::new_id()),
                call: zeron_proto::ToolCall::Unknown {
                    name: step.name,
                    input: Some(input),
                },
                is_error: !step.ok,
                resolved: true,
                output,
                diff: None,
                output_ref: None,
                output_bytes: None,
                diff_ref: None,
                diff_stats: None,
                subagent_ref: None,
                subagent_status: None,
                subagent_tail: None,
            }
        })
        .collect()
}

/// "4.2s", "1m 12s".
fn format_wait(waited: Duration) -> String {
    let secs = waited.as_secs();
    if secs >= 60 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{:.1}s", waited.as_secs_f64())
    }
}

/// `{data_dir}/runner.json` — `{orgId, userId, deviceId, edgeUrl}`.
pub const RUNNER_FILE: &str = "runner.json";
/// `{data_dir}/runner-key` — the Ed25519 private key, PKCS#8 DER, 0600.
pub const RUNNER_KEY_FILE: &str = "runner-key";
/// One-time enrollment directive set by the provisioning workflow.
pub const ENROLL_ENV: &str = "ZERON_RUNNER_ENROLL";
/// Generic `Device::platform` override (`cloud` on a Cloud device).
pub const PLATFORM_ENV: &str = "ZERON_DEVICE_PLATFORM";
/// How often a runner reports activity to its CloudDevice DO.
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(60);

const HTTP_TIMEOUT: Duration = Duration::from_secs(15);
const ENROLL_RETRY_BASE: Duration = Duration::from_secs(1);
const ENROLL_RETRY_CAP: Duration = Duration::from_secs(60);
const TOKEN_DOMAIN: &str = "zeron-runner-token";
const GRANT_DOMAIN: &str = "zeron-vault-grant";

/// This engine's `Device::platform`: `ZERON_DEVICE_PLATFORM` when set, else
/// the OS (`macos` / `linux` / `windows`).
pub fn device_platform() -> String {
    platform_from(std::env::var(PLATFORM_ENV).ok())
}

fn platform_from(value: Option<String>) -> String {
    value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| std::env::consts::OS.to_string())
}

/// Whether this engine is a Cloud device (`ZERON_DEVICE_PLATFORM=cloud`).
pub fn is_cloud_platform() -> bool {
    device_platform() == zeron_proto::CLOUD_PLATFORM
}

/// The persisted runner identity (`runner.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunnerConfig {
    pub org_id: String,
    pub user_id: String,
    pub device_id: String,
    pub edge_url: String,
}

/// A parsed `ZERON_RUNNER_ENROLL` value. `Debug` never prints the code.
#[derive(Clone, PartialEq, Eq)]
pub struct EnrollCode {
    pub org_id: String,
    pub user_id: String,
    pub device_id: String,
    code: String,
}

impl std::fmt::Debug for EnrollCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EnrollCode")
            .field("org_id", &self.org_id)
            .field("user_id", &self.user_id)
            .field("device_id", &self.device_id)
            .field("code", &"<redacted>")
            .finish()
    }
}

impl EnrollCode {
    /// `{orgId}.{userId}.{deviceId}.{code}`; the code (base64url) is last, so
    /// only the three ids must be dot-free.
    pub fn parse(raw: &str) -> Result<Self, EngineError> {
        let mut parts = raw.trim().splitn(4, '.');
        let mut next = || parts.next().map(str::trim).filter(|s| !s.is_empty());
        match (next(), next(), next(), next()) {
            (Some(org_id), Some(user_id), Some(device_id), Some(code)) => Ok(Self {
                org_id: org_id.into(),
                user_id: user_id.into(),
                device_id: device_id.into(),
                code: code.into(),
            }),
            _ => Err(EngineError::Other(format!(
                "{ENROLL_ENV} must be {{orgId}}.{{userId}}.{{deviceId}}.{{code}}"
            ))),
        }
    }
}

/// The enrolled runner: its ids plus the signing key. Timestamps handed out
/// by [`Self::next_ts`] are strictly increasing (the edge's replay fence
/// rejects a `ts` that is not greater than the last accepted one).
pub struct RunnerIdentity {
    config: RunnerConfig,
    key: Ed25519KeyPair,
    last_ts: Mutex<i64>,
}

impl std::fmt::Debug for RunnerIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunnerIdentity")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

/// Body of `POST /runner/token`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenRequest {
    pub org_id: String,
    pub user_id: String,
    pub device_id: String,
    pub ts: i64,
    pub sig: String,
}

/// Body of `POST /vault/{orgId}/grant`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GrantRequest {
    pub provider: String,
    pub device_id: String,
    pub ts: i64,
    pub sig: String,
    /// `github` grants: the `owner/name` the App installation token is for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
}

impl RunnerIdentity {
    /// Load `runner.json` + `runner-key`. `Ok(None)` when this is not a
    /// runner; an error when `runner.json` exists but the identity is unusable
    /// (a runner must never silently fall back to another auth mode).
    pub fn load(data_dir: &Path) -> Result<Option<Self>, EngineError> {
        let path = data_dir.join(RUNNER_FILE);
        let raw = match std::fs::read(&path) {
            Ok(raw) => raw,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(err.into()),
        };
        let config: RunnerConfig = serde_json::from_slice(&raw).map_err(|err| {
            EngineError::Other(format!("invalid runner identity {}: {err}", path.display()))
        })?;
        let key_path = data_dir.join(RUNNER_KEY_FILE);
        let pkcs8 = std::fs::read(&key_path).map_err(|err| {
            EngineError::Other(format!(
                "runner key {} is unreadable ({err}); this Cloud device must be re-provisioned",
                key_path.display()
            ))
        })?;
        Self::from_parts(config, &pkcs8).map(Some)
    }

    fn from_parts(config: RunnerConfig, pkcs8: &[u8]) -> Result<Self, EngineError> {
        let key = Ed25519KeyPair::from_pkcs8(pkcs8)
            .map_err(|err| EngineError::Other(format!("invalid runner key: {err}")))?;
        Ok(Self {
            config,
            key,
            last_ts: Mutex::new(0),
        })
    }

    pub fn config(&self) -> &RunnerConfig {
        &self.config
    }

    pub fn org_id(&self) -> &str {
        &self.config.org_id
    }

    pub fn user_id(&self) -> &str {
        &self.config.user_id
    }

    pub fn device_id(&self) -> &str {
        &self.config.device_id
    }

    pub fn edge_url(&self) -> &str {
        self.config.edge_url.trim_end_matches('/')
    }

    /// The raw 32-byte public key, base64url without padding.
    pub fn public_key(&self) -> String {
        B64URL.encode(self.key.public_key().as_ref())
    }

    /// Unix ms, strictly greater than every `ts` this identity handed out
    /// before — even when the wall clock repeats or steps backwards.
    pub fn next_ts(&self) -> i64 {
        self.next_ts_at(crate::now_ms())
    }

    pub(crate) fn next_ts_at(&self, now: i64) -> i64 {
        let mut last = self.last_ts.lock().unwrap_or_else(PoisonError::into_inner);
        let ts = now.max(*last + 1);
        *last = ts;
        ts
    }

    /// base64url (no padding) Ed25519 signature over `message`.
    pub fn sign(&self, message: &str) -> String {
        B64URL.encode(self.key.sign(message.as_bytes()).as_ref())
    }

    pub fn token_message(org_id: &str, user_id: &str, device_id: &str, ts: i64) -> String {
        format!("{TOKEN_DOMAIN}\n{org_id}\n{user_id}\n{device_id}\n{ts}")
    }

    pub fn grant_message(user_id: &str, device_id: &str, provider: &str, ts: i64) -> String {
        format!("{GRANT_DOMAIN}\n{user_id}\n{device_id}\n{provider}\n{ts}")
    }

    /// A freshly signed `/runner/token` body.
    pub fn token_request(&self) -> TokenRequest {
        let ts = self.next_ts();
        let message = Self::token_message(self.org_id(), self.user_id(), self.device_id(), ts);
        TokenRequest {
            org_id: self.org_id().into(),
            user_id: self.user_id().into(),
            device_id: self.device_id().into(),
            ts,
            sig: self.sign(&message),
        }
    }

    /// A freshly signed `/vault/{orgId}/grant` body. `repo` (`owner/name`)
    /// rides along for `github` grants; it isn't part of the signed message.
    pub fn grant_request(&self, provider: &str, repo: Option<&str>) -> GrantRequest {
        let ts = self.next_ts();
        let message = Self::grant_message(self.user_id(), self.device_id(), provider, ts);
        GrantRequest {
            provider: provider.into(),
            device_id: self.device_id().into(),
            ts,
            sig: self.sign(&message),
            repo: repo.map(str::to_owned),
        }
    }
}

pub(crate) fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .user_agent(concat!("zeron/", env!("CARGO_PKG_VERSION")))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// Boot-time runner detection (see the module docs). Reads
/// `ZERON_RUNNER_ENROLL` from the environment.
pub async fn bootstrap(
    data_dir: &Path,
    edge_url: &str,
) -> Result<Option<RunnerIdentity>, EngineError> {
    let enroll = std::env::var(ENROLL_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty());
    bootstrap_with(
        data_dir,
        edge_url,
        enroll.as_deref(),
        &http_client(),
        &crate::credentials::BrokerConfig::detect(data_dir),
    )
    .await
}

/// [`bootstrap`] with an explicit enrollment directive, HTTP client, and
/// the credential-file locations a discarded identity is scrubbed from.
pub async fn bootstrap_with(
    data_dir: &Path,
    edge_url: &str,
    enroll: Option<&str>,
    http: &reqwest::Client,
    credentials: &crate::credentials::BrokerConfig,
) -> Result<Option<RunnerIdentity>, EngineError> {
    let persisted = RunnerIdentity::load(data_dir);
    let code = match enroll.map(EnrollCode::parse) {
        None => None,
        Some(Ok(code)) => Some(code),
        // A broken directive must not brick an already-enrolled device.
        Some(Err(err)) if matches!(persisted, Ok(Some(_))) => {
            tracing::warn!(error = %err, "runner: ignoring a malformed {ENROLL_ENV}");
            None
        }
        Some(Err(err)) => return Err(err),
    };
    match (persisted, code) {
        (Ok(Some(identity)), Some(code)) if identity.device_id() == code.device_id => {
            tracing::info!(device = %identity.device_id(),
                "runner: already enrolled as this device; ignoring {ENROLL_ENV}");
            ensure_device_id(data_dir, identity.device_id())?;
            Ok(Some(identity))
        }
        (Ok(Some(identity)), None) => {
            ensure_device_id(data_dir, identity.device_id())?;
            Ok(Some(identity))
        }
        (Ok(None), None) => Ok(None),
        // An unusable runner.json with nothing to re-enroll with stays fatal.
        (Err(err), None) => Err(err),
        // A new device id (or no usable identity): this disk was copied from
        // another device (fork / template snapshot) or never enrolled.
        (persisted, Some(code)) => {
            if let Ok(Some(old)) = &persisted {
                tracing::warn!(previous = %old.device_id(), device = %code.device_id,
                    "runner: this disk carries another Cloud device's identity; \
                     discarding it and enrolling as the new device");
            }
            // Always scrub first: a template snapshot may carry managed
            // credential files (or a stray key) even without runner.json.
            discard_identity(data_dir, credentials);
            enroll_with_retry(data_dir, edge_url, &code, http)
                .await
                .map(Some)
        }
    }
}

/// Forget a copied runner identity: its key + runner.json and every
/// credential file the broker manages. User files, repositories, and a native
/// Claude login (real refresh token) are left alone.
fn discard_identity(data_dir: &Path, credentials: &crate::credentials::BrokerConfig) {
    for name in [RUNNER_FILE, RUNNER_KEY_FILE, SESSION_FILE] {
        match std::fs::remove_file(data_dir.join(name)) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => tracing::warn!(file = name, error = %err, "runner: discard failed"),
        }
    }
    crate::credentials::discard_managed_credentials(credentials);
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct EdgeErrorBody {
    #[serde(default)]
    pub error: String,
    #[serde(default)]
    pub message: String,
}

impl EdgeErrorBody {
    pub(crate) async fn read(res: reqwest::Response) -> Self {
        res.json::<Self>().await.unwrap_or_default()
    }

    pub(crate) fn describe(&self) -> String {
        match (self.error.is_empty(), self.message.is_empty()) {
            (true, true) => "no detail".into(),
            (false, true) => self.error.clone(),
            (true, false) => self.message.clone(),
            (false, false) => format!("{}: {}", self.error, self.message),
        }
    }
}

async fn enroll_with_retry(
    data_dir: &Path,
    edge_url: &str,
    code: &EnrollCode,
    http: &reqwest::Client,
) -> Result<RunnerIdentity, EngineError> {
    std::fs::create_dir_all(data_dir)?;
    let pkcs8 = generate_key(data_dir)?;
    let key = Ed25519KeyPair::from_pkcs8(&pkcs8)
        .map_err(|err| EngineError::Other(format!("invalid runner key: {err}")))?;
    let edge = edge_url.trim_end_matches('/');
    let url = format!("{edge}/runner/enroll");
    let body = serde_json::json!({
        "orgId": code.org_id,
        "userId": code.user_id,
        "deviceId": code.device_id,
        "code": code.code,
        "publicKey": B64URL.encode(key.public_key().as_ref()),
    });
    tracing::info!(device = %code.device_id, org = %code.org_id, %edge, "runner: enrolling this Cloud device");
    let mut delay = ENROLL_RETRY_BASE;
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        match http.post(&url).json(&body).send().await {
            Ok(res) if res.status().is_success() => break,
            Ok(res) => {
                let status = res.status().as_u16();
                let detail = EdgeErrorBody::read(res).await.describe();
                if (400..500).contains(&status) && !matches!(status, 408 | 429) {
                    tracing::error!(
                        status,
                        %detail,
                        device = %code.device_id,
                        "runner: enrollment REJECTED by the edge — the one-time code is invalid, \
                         expired or already used. Re-provision this Cloud device (Settings → Cloud)."
                    );
                    return Err(EngineError::Other(format!(
                        "runner enrollment rejected by the edge ({status}): {detail}"
                    )));
                }
                tracing::warn!(status, %detail, attempt, retry_s = delay.as_secs(),
                    "runner: enrollment failed; retrying");
            }
            Err(err) => {
                tracing::warn!(error = %describe_http_error(err), attempt, retry_s = delay.as_secs(),
                    "runner: edge unreachable during enrollment; retrying");
            }
        }
        tokio::time::sleep(delay).await;
        delay = (delay * 2).min(ENROLL_RETRY_CAP);
    }
    let config = RunnerConfig {
        org_id: code.org_id.clone(),
        user_id: code.user_id.clone(),
        device_id: code.device_id.clone(),
        edge_url: edge.to_string(),
    };
    let bytes = serde_json::to_vec_pretty(&config)
        .map_err(|err| EngineError::Other(format!("serialize runner identity: {err}")))?;
    write_private_atomic(&data_dir.join(RUNNER_FILE), &bytes)?;
    ensure_device_id(data_dir, &config.device_id)?;
    tracing::info!(device = %config.device_id, "runner: enrolled");
    RunnerIdentity::from_parts(config, &pkcs8)
}

/// Every enrollment mints a NEW key (persisted 0600, replacing any old one):
/// the vault never re-admits a revoked key under any device id, and a key
/// copied from another device's disk must never be reused.
fn generate_key(data_dir: &Path) -> Result<Vec<u8>, EngineError> {
    let rng = ring::rand::SystemRandom::new();
    let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng)
        .map_err(|err| EngineError::Other(format!("generate runner key: {err}")))?;
    write_private_atomic(&data_dir.join(RUNNER_KEY_FILE), pkcs8.as_ref())?;
    Ok(pkcs8.as_ref().to_vec())
}

/// Make `{data_dir}/device-id` the edge-minted id, so the registry row, the
/// device room and the vault all agree on who this engine is.
fn ensure_device_id(data_dir: &Path, device_id: &str) -> Result<(), EngineError> {
    let path = data_dir.join("device-id");
    match std::fs::read_to_string(&path) {
        Ok(current) if current.trim() == device_id => return Ok(()),
        Ok(current) if !current.trim().is_empty() => tracing::warn!(
            previous = %current.trim(),
            device = %device_id,
            "runner: replacing the local device id with the enrolled Cloud device id"
        ),
        Ok(_) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(err.into()),
    }
    write_atomic(&path, device_id.as_bytes(), None)
}

/// Write `bytes` to `path` owner-only (0600) via a same-directory temp file +
/// rename, so readers never observe a partial file.
pub(crate) fn write_private_atomic(path: &Path, bytes: &[u8]) -> Result<(), EngineError> {
    write_atomic(path, bytes, Some(0o600))
}

fn write_atomic(path: &Path, bytes: &[u8], mode: Option<u32>) -> Result<(), EngineError> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir)?;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let temp: PathBuf = dir.join(format!(
        ".{name}.tmp-{}-{}",
        std::process::id(),
        crate::new_id()
    ));
    let result = (|| -> std::io::Result<()> {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        if let Some(mode) = mode {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(mode);
        }
        #[cfg(not(unix))]
        let _ = mode;
        let mut file = options.open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result.map_err(Into::into)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct HeartbeatBody {
    active_runs: usize,
    clients: usize,
}

/// Counts sessions with a turn in flight (the heartbeat's `activeRuns`).
pub type ActiveRuns = Arc<dyn Fn() -> usize + Send + Sync>;

/// Relay requests served to remote devices since the last heartbeat — the
/// heartbeat's `clients`. Filled by [`RelayActivity::wrap`] around the RPC
/// service the host relay serves; drained by each successful heartbeat.
#[derive(Clone, Default)]
pub struct RelayActivity {
    served: Arc<std::sync::atomic::AtomicUsize>,
}

impl RelayActivity {
    /// Wrap the relay-served RPC service so every request a remote device
    /// makes counts, except the relay's own liveness traffic
    /// ([`zeron_rpc::is_liveness_probe`]). A watch stream counts once, when
    /// opened: its continuation frames never re-enter the service, and echo
    /// keepalives are relay-control frames that never reach it at all.
    pub fn wrap(&self, inner: Arc<dyn zeron_rpc::RpcService>) -> Arc<dyn zeron_rpc::RpcService> {
        Arc::new(CountingService {
            inner,
            activity: self.clone(),
        })
    }

    fn note(&self) {
        self.served
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    fn take(&self) -> usize {
        self.served.swap(0, std::sync::atomic::Ordering::Relaxed)
    }

    /// Put back a count a failed heartbeat could not deliver.
    fn restore(&self, count: usize) {
        self.served
            .fetch_add(count, std::sync::atomic::Ordering::Relaxed);
    }
}

struct CountingService {
    inner: Arc<dyn zeron_rpc::RpcService>,
    activity: RelayActivity,
}

#[async_trait::async_trait]
impl zeron_rpc::RpcService for CountingService {
    async fn handle(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<zeron_rpc::RpcReply, zeron_rpc::RpcError> {
        if !zeron_rpc::is_liveness_probe(method, &params) {
            self.activity.note();
        }
        self.inner.handle(method, params).await
    }
}

/// Post `{activeRuns, clients}` to `{edge}/runner/heartbeat` every `every`
/// (first beat immediately).
///
/// - `activeRuns`: chats with a turn in flight right now.
/// - `clients`: relay RPC REQUESTS served to remote devices since the
///   previous delivered heartbeat (see [`RelayActivity`]) — not open links or
///   sockets. A laptop that merely keeps a link or a watch stream open must
///   not keep the Cloud device awake (and billed) forever; the edge treats
///   `clients > 0` as activity that postpones idle sleep. A heartbeat that
///   fails to deliver carries its count over to the next one.
pub fn spawn_heartbeat(
    edge_url: &str,
    tokens: Arc<dyn zeron_rpc::TokenSource>,
    active_runs: ActiveRuns,
    activity: RelayActivity,
    every: Duration,
) -> tokio::task::JoinHandle<()> {
    let url = format!("{}/runner/heartbeat", edge_url.trim_end_matches('/'));
    tokio::spawn(async move {
        let http = http_client();
        let mut ticker = tokio::time::interval(every);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            let token = match tokens.token().await {
                Ok(token) => token,
                Err(err) => {
                    tracing::debug!(error = %err, "runner: heartbeat skipped (no token)");
                    continue;
                }
            };
            let body = HeartbeatBody {
                active_runs: active_runs(),
                clients: activity.take(),
            };
            match http.post(&url).bearer_auth(token).json(&body).send().await {
                Ok(res) if res.status().is_success() => {
                    tracing::debug!(
                        active_runs = body.active_runs,
                        clients = body.clients,
                        "runner: heartbeat"
                    );
                }
                Ok(res) => {
                    activity.restore(body.clients);
                    let status = res.status().as_u16();
                    let detail = EdgeErrorBody::read(res).await.describe();
                    tracing::warn!(status, %detail, "runner: heartbeat rejected");
                }
                Err(err) => {
                    activity.restore(body.clients);
                    tracing::debug!(error = %describe_http_error(err), "runner: heartbeat failed");
                }
            }
        }
    })
}

/// Runner-only background work owned by the engine core: the credential
/// broker (and its refresher), the heartbeat, and the relay activity counter
/// the heartbeat drains. Dropping it stops the background tasks.
pub struct CloudRunner {
    broker: crate::credentials::CredentialBroker,
    activity: RelayActivity,
    heartbeat: tokio::task::JoinHandle<()>,
}

impl CloudRunner {
    pub fn new(
        broker: crate::credentials::CredentialBroker,
        activity: RelayActivity,
        heartbeat: tokio::task::JoinHandle<()>,
    ) -> Self {
        Self {
            broker,
            activity,
            heartbeat,
        }
    }

    pub fn broker(&self) -> &crate::credentials::CredentialBroker {
        &self.broker
    }

    /// The counter the host relay's served requests feed.
    pub fn activity(&self) -> &RelayActivity {
        &self.activity
    }
}

impl Drop for CloudRunner {
    fn drop(&mut self) {
        self.heartbeat.abort();
        self.broker.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session_vars(name: &str) -> Option<String> {
        match name {
            "ZERON_CLOUD_ACCOUNT" => Some("cloud-acct".into()),
            "ZERON_CLOUD_CHAT" => Some("0123456789abcdef".into()),
            "ZERON_CLOUD_REPO" => Some("https://github.com/acme/app.git".into()),
            "ZERON_CLOUD_PATH" => Some("/home/user/app".into()),
            "ZERON_CLOUD_BRANCH" => Some("main".into()),
            _ => None,
        }
    }

    #[test]
    fn a_session_resolves_from_env_and_survives_a_restart_without_it() {
        let dir = tempfile::tempdir().unwrap();
        let session = CloudSession::resolve_with(dir.path(), session_vars).unwrap();
        assert_eq!(session.chat_id, "0123456789abcdef");
        assert_eq!(session.session_branch(), "zeron/cloud-0123456789ab");
        // Next boot: the systemd env is gone, the file remains.
        assert_eq!(
            CloudSession::resolve_with(dir.path(), |_| None),
            Some(session)
        );
        // A laptop (no env, no file) is not a session.
        let laptop = tempfile::tempdir().unwrap();
        assert_eq!(CloudSession::resolve_with(laptop.path(), |_| None), None);
        // Account + chat are required.
        assert_eq!(
            CloudSession::resolve_with(laptop.path(), |name| (name == "ZERON_CLOUD_CHAT")
                .then(|| "c".into())),
            None
        );
    }

    #[test]
    fn setup_steps_become_resolved_cloud_tool_chips() {
        let step = |step, name: &str, command: Option<&str>, output: Option<&str>, ok| SetupStep {
            step,
            name: name.into(),
            command: command.map(str::to_string),
            output: output.map(str::to_string),
            ok,
        };
        let steps = vec![
            step("start", "Started a machine", None, None, true),
            step(
                "clone",
                "Cloned acme/app",
                Some("git clone https://github.com/acme/app.git /home/user/app"),
                Some("in 3.2s"),
                true,
            ),
            step(
                "branch",
                "Couldn't check out zeron/cloud-x",
                Some("git checkout -B zeron/cloud-x"),
                Some("fatal: nope"),
                false,
            ),
        ];
        let parts = setup_parts(steps, Some(Duration::from_secs(72)));
        let summary: Vec<(String, Option<String>, bool, Option<String>)> = parts
            .iter()
            .map(|part| match part {
                zeron_doc::MessagePart::Tool {
                    call,
                    output,
                    is_error,
                    resolved,
                    ..
                } => {
                    assert!(resolved);
                    let zeron_proto::ToolCall::Unknown { name, .. } = call else {
                        panic!("{call:?}")
                    };
                    (
                        name.clone(),
                        zeron_proto::cloud_setup_step(call).map(str::to_string),
                        *is_error,
                        output.clone(),
                    )
                }
                other => panic!("{other:?}"),
            })
            .collect();
        let row = |name: &str, step: &str, error, output: &str| {
            (
                name.to_string(),
                Some(step.to_string()),
                error,
                Some(output.to_string()),
            )
        };
        assert_eq!(
            summary,
            [
                row("Started a machine", "start", false, "ready after 1m 12s"),
                row("Cloned acme/app", "clone", false, "in 3.2s"),
                row(
                    "Couldn't check out zeron/cloud-x",
                    "branch",
                    true,
                    "fatal: nope"
                ),
            ]
        );
        assert_eq!(format_wait(Duration::from_millis(4_200)), "4.2s");
    }

    #[test]
    fn github_grants_name_the_session_repository_as_owner_slash_name() {
        let dir = tempfile::tempdir().unwrap();
        let session = CloudSession::resolve_with(dir.path(), session_vars).unwrap();
        assert_eq!(session.github_repo().as_deref(), Some("acme/app"));
        for (url, want) in [
            (
                "https://github.com/octocat/Hello-World",
                Some("octocat/Hello-World"),
            ),
            (
                "https://github.com/octocat/app.js.git/",
                Some("octocat/app.js"),
            ),
            (
                "git@github.com:acme-inc/repo_1.git",
                Some("acme-inc/repo_1"),
            ),
            ("ssh://git@github.com/acme/app.git", Some("acme/app")),
            ("https://gitlab.com/acme/app.git", None),
            ("https://github.com/acme", None),
            ("https://github.com/acme/app/tree/main", None),
            ("https://github.com.evil.example/acme/app", None),
        ] {
            assert_eq!(github_full_name(url).as_deref(), want, "{url}");
        }
    }

    fn git(dir: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[tokio::test]
    async fn the_checkout_clones_once_on_the_session_branch() {
        let dir = tempfile::tempdir().unwrap();
        let origin = dir.path().join("origin");
        std::fs::create_dir_all(&origin).unwrap();
        git(&origin, &["init", "-q", "-b", "main"]);
        git(
            &origin,
            &[
                "-c",
                "user.email=t@t",
                "-c",
                "user.name=t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "init",
            ],
        );
        let path = dir.path().join("work").join("app");
        let session = CloudSession {
            account: "cloud-acct".into(),
            chat_id: "abcdef0123".into(),
            repo: Some(origin.to_string_lossy().into_owned()),
            path: Some(path.to_string_lossy().into_owned()),
            branch: Some("main".into()),
        };
        prepare_checkout(&session, None).await.unwrap();
        let head = std::process::Command::new("git")
            .arg("-C")
            .arg(&path)
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&head.stdout).trim(),
            "zeron/cloud-abcdef0123"
        );
        // A wake or restart keeps the working tree (uncommitted work included).
        std::fs::write(path.join("notes.txt"), "work in progress").unwrap();
        prepare_checkout(&session, None).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(path.join("notes.txt")).unwrap(),
            "work in progress"
        );
        // An unreachable repository fails clearly and leaves nothing behind.
        let broken = CloudSession {
            repo: Some(dir.path().join("missing").to_string_lossy().into_owned()),
            path: Some(dir.path().join("other").to_string_lossy().into_owned()),
            branch: None,
            ..session
        };
        let err = prepare_checkout_with(&broken, None, Duration::from_millis(1))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("couldn't clone"), "{err}");
        assert!(!dir.path().join("other").exists());
    }

    fn identity(dir: &Path) -> RunnerIdentity {
        let rng = ring::rand::SystemRandom::new();
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng).unwrap();
        std::fs::write(dir.join(RUNNER_KEY_FILE), pkcs8.as_ref()).unwrap();
        let config = RunnerConfig {
            org_id: "org_1".into(),
            user_id: "user_1".into(),
            device_id: "cloud-1".into(),
            edge_url: "https://edge.example/".into(),
        };
        std::fs::write(dir.join(RUNNER_FILE), serde_json::to_vec(&config).unwrap()).unwrap();
        RunnerIdentity::load(dir).unwrap().unwrap()
    }

    #[test]
    fn enroll_code_parses_four_parts_and_redacts_the_code() {
        let code = EnrollCode::parse(" org_1.user_1.cloud-abc.c0de-_x ").unwrap();
        assert_eq!(code.org_id, "org_1");
        assert_eq!(code.user_id, "user_1");
        assert_eq!(code.device_id, "cloud-abc");
        assert_eq!(code.code, "c0de-_x");
        assert!(!format!("{code:?}").contains("c0de"));
        for bad in ["", "a.b.c", "a..c.d", "a.b.c."] {
            assert!(EnrollCode::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn ts_is_strictly_increasing_even_when_the_clock_repeats_or_rewinds() {
        let dir = tempfile::tempdir().unwrap();
        let id = identity(dir.path());
        assert_eq!(id.next_ts_at(1_000), 1_000);
        assert_eq!(id.next_ts_at(1_000), 1_001);
        assert_eq!(id.next_ts_at(500), 1_002);
        assert_eq!(id.next_ts_at(5_000), 5_000);
        let a = id.next_ts();
        let b = id.next_ts();
        assert!(b > a);
    }

    #[test]
    fn token_and_grant_signatures_verify_with_the_published_key() {
        use ring::signature::{ED25519, UnparsedPublicKey};
        let dir = tempfile::tempdir().unwrap();
        let id = identity(dir.path());
        assert_eq!(id.edge_url(), "https://edge.example");
        let public = B64URL.decode(id.public_key()).unwrap();
        assert_eq!(public.len(), 32);
        let key = UnparsedPublicKey::new(&ED25519, public);

        let token = id.token_request();
        let message = format!("zeron-runner-token\norg_1\nuser_1\ncloud-1\n{}", token.ts);
        key.verify(message.as_bytes(), &B64URL.decode(&token.sig).unwrap())
            .expect("token signature verifies");

        let grant = id.grant_request("codex", None);
        assert!(grant.ts > token.ts);
        let message = format!("zeron-vault-grant\nuser_1\ncloud-1\ncodex\n{}", grant.ts);
        key.verify(message.as_bytes(), &B64URL.decode(&grant.sig).unwrap())
            .expect("grant signature verifies");
        // A different domain never verifies (no cross-protocol replay).
        let forged = format!("zeron-runner-token\nuser_1\ncloud-1\ncodex\n{}", grant.ts);
        assert!(
            key.verify(forged.as_bytes(), &B64URL.decode(&grant.sig).unwrap())
                .is_err()
        );
    }

    #[test]
    fn platform_override_is_trimmed_and_falls_back_to_the_os() {
        assert_eq!(platform_from(Some(" cloud ".into())), "cloud");
        assert_eq!(platform_from(Some("  ".into())), std::env::consts::OS);
        assert_eq!(platform_from(None), std::env::consts::OS);
    }

    #[test]
    fn a_runner_json_without_a_usable_key_is_an_error_not_a_fallback() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(RUNNER_FILE),
            r#"{"orgId":"o","userId":"u","deviceId":"d","edgeUrl":"http://e"}"#,
        )
        .unwrap();
        assert!(RunnerIdentity::load(dir.path()).is_err());
        let empty = tempfile::tempdir().unwrap();
        assert!(RunnerIdentity::load(empty.path()).unwrap().is_none());
    }
}
