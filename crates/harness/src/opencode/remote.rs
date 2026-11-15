//! An OpenCode server on another computer, in place of this one's background
//! service (Settings → Providers → OpenCode settings → Remote server), as
//! OpenCode's desktop app connects to one: its address and password, a
//! pairing link it made (redeemed once for a session token, the password
//! from then on), or SSH — the service on the far end found (started, and
//! OpenCode installed there first, if it has none), reached through a local
//! port forward.
//!
//! Runs, the session sync, catalogs and sign-ins all go to it. Its tools act
//! on its own disk; only client MCP (Zeron's tools) runs here.

use std::time::Duration;

use serde::Deserialize;

use super::service::{self, ServiceInfo};
use crate::HarnessError;
use crate::process::{Child, Command, Stdio};

/// Where the remote server is, as Settings say.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Remote {
    /// An address Zeron can reach (a server online, on Tailscale, a custom
    /// local instance) and its password.
    Address { url: String, password: String },
    /// A host SSH reaches (`user@host`, a `~/.ssh/config` alias), or an
    /// `ssh …` command naming one.
    Ssh { target: String },
}

static REMOTE: std::sync::Mutex<Option<Remote>> = std::sync::Mutex::new(None);

/// Set from the person's OpenCode settings: `None` is this computer's own
/// service.
pub fn set_remote(remote: Option<Remote>) {
    let ssh = matches!(remote, Some(Remote::Ssh { .. }));
    let mut current = REMOTE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if *current != remote {
        GENERATION.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
    }
    *current = remote;
    drop(current);
    // (A forward no longer wanted closes; a changed one is replaced on use.)
    if !ssh && let Ok(mut tunnel) = TUNNEL.try_lock() {
        *tunnel = None;
    }
}

/// Bumped each time the remote server changes: catalogs read before are
/// another server's.
static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub(super) fn generation() -> u64 {
    GENERATION.load(std::sync::atomic::Ordering::Acquire)
}

pub(super) fn current() -> Option<Remote> {
    REMOTE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// How long an SSH step may take (a first connection, an install).
const SSH_TIMEOUT: Duration = Duration::from_secs(30);
const INSTALL_TIMEOUT: Duration = Duration::from_secs(300);
/// How long a forward may take to answer once SSH holds it.
const FORWARD_READY: Duration = Duration::from_secs(10);

/// A pairing link (`http://host:port/auth/connect/<code>`, from `opencode
/// pair` or the server's Settings → Pairing) redeemed: the server's address
/// and the session token that is its password from now on. The code is
/// single-use.
pub async fn redeem_pairing_link(link: &str) -> Result<(String, String), HarnessError> {
    let invalid = || {
        HarnessError::Protocol(
            "Paste the full pairing link from `opencode pair`, or from Settings → Pairing on the server's computer."
                .into(),
        )
    };
    let parsed = reqwest::Url::parse(link.trim()).map_err(|_| invalid())?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(invalid());
    }
    let code_at = parsed.path().find("/auth/connect/").ok_or_else(invalid)?;
    let base = format!(
        "{}{}",
        parsed.origin().ascii_serialization(),
        &parsed.path()[..code_at]
    );
    #[derive(Deserialize)]
    struct Session {
        token: String,
    }
    let response = super::http_client()
        .get(parsed.as_str())
        .header(reqwest::header::ACCEPT, "application/json")
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| HarnessError::Protocol(format!("couldn't reach the server: {e}")))?;
    if !response.status().is_success() {
        return Err(HarnessError::Protocol(format!(
            "the server refused the pairing link ({}): it may be used already or expired",
            response.status()
        )));
    }
    let session: Session = response
        .json()
        .await
        .map_err(|e| HarnessError::Protocol(format!("the server's pairing answer didn't read: {e}")))?;
    Ok((base, session.token))
}

/// The SSH forward to the remote service, one for the whole engine (every
/// harness and the session sync share it), held while it's in use.
static TUNNEL: tokio::sync::Mutex<Option<Tunnel>> = tokio::sync::Mutex::const_new(None);

struct Tunnel {
    target: String,
    info: ServiceInfo,
    child: Child,
}

impl Tunnel {
    fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

/// The remote server, connected: its address and password for every request
/// (a local forward, over SSH, kept open between calls).
pub(super) async fn connect(
    remote: &Remote,
    client: &reqwest::Client,
    local_version: Option<&semver::Version>,
) -> Result<ServiceInfo, HarnessError> {
    match remote {
        Remote::Address { url, password } => {
            let info = ServiceInfo {
                url: url.trim().trim_end_matches('/').to_owned(),
                password: password.clone(),
                version: None,
            };
            if service::healthy(client, &info).await {
                Ok(info)
            } else {
                Err(HarnessError::Protocol(format!(
                    "Couldn't connect to the OpenCode server at {}. Check its address and password (on the server's computer: `opencode service get password`).",
                    info.url
                )))
            }
        }
        Remote::Ssh { target } => {
            let mut tunnel = TUNNEL.lock().await;
            if let Some(held) = tunnel.as_mut()
                && held.target == *target
                && held.alive()
                && service::healthy(client, &held.info).await
            {
                return Ok(held.info.clone());
            }
            *tunnel = None;
            let opened = open_tunnel(target, client, local_version).await?;
            let info = opened.info.clone();
            *tunnel = Some(opened);
            Ok(info)
        }
    }
}

/// `ssh …`'s target as typed: its options and its host.
#[derive(Debug, PartialEq, Eq)]
struct SshTarget {
    args: Vec<String>,
    host: String,
}

/// A host, or an `ssh` command naming one with the options that pick how
/// it's reached (as OpenCode's desktop app takes it): `-p`, `-l`, `-i`,
/// `-F`, `-J`, `-o` for the connection's own settings, `-4`, `-6`, `-C`,
/// `-A`, `-a`. Anything else (a remote command, a forward) is refused.
fn ssh_target(input: &str) -> Result<SshTarget, HarnessError> {
    let invalid = || {
        HarnessError::Protocol(format!(
            "`{}` isn't an SSH host or an `ssh` command naming one",
            input.trim()
        ))
    };
    let mut words = split_words(input).ok_or_else(invalid)?;
    if words.first().is_some_and(|w| w == "ssh") {
        words.remove(0);
    }
    const OPTIONS: [&str; 10] = [
        "hostname",
        "user",
        "port",
        "identityfile",
        "identityagent",
        "identitiesonly",
        "proxyjump",
        "proxycommand",
        "connecttimeout",
        "addressfamily",
    ];
    let mut args = Vec::new();
    let mut words = words.into_iter().peekable();
    while let Some(word) = words.next_if(|w| w.starts_with('-')) {
        if matches!(word.as_str(), "-4" | "-6" | "-C" | "-A" | "-a") {
            args.push(word);
            continue;
        }
        let flag = word.get(..2).ok_or_else(invalid)?.to_owned();
        if !matches!(flag.as_str(), "-p" | "-l" | "-i" | "-F" | "-J" | "-o") {
            return Err(invalid());
        }
        let value = if word.len() > 2 {
            word[2..].to_owned()
        } else {
            words.next().ok_or_else(invalid)?
        };
        let bad = value.starts_with('-')
            || (flag == "-p" && !value.parse::<u16>().is_ok_and(|p| p > 0))
            || (flag == "-o"
                && !OPTIONS.contains(
                    &value
                        .split(['=', ' ', '\t'])
                        .next()
                        .unwrap_or_default()
                        .to_ascii_lowercase()
                        .as_str(),
                ));
        if bad {
            return Err(invalid());
        }
        args.push(flag);
        args.push(value);
    }
    let host = words.next().ok_or_else(invalid)?;
    let ok = words.next().is_none()
        && !host.starts_with('-')
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_@.:[]%-".contains(c));
    if !ok {
        return Err(invalid());
    }
    Ok(SshTarget { args, host })
}

/// Words as a shell splits them: on blanks, with single and double quotes.
fn split_words(input: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut word: Option<String> = None;
    let mut quote: Option<char> = None;
    for c in input.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => word.get_or_insert_default().push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                word.get_or_insert_default();
            }
            (None, c) if c.is_whitespace() => words.extend(word.take()),
            (None, c) => word.get_or_insert_default().push(c),
        }
    }
    if quote.is_some() {
        return None;
    }
    words.extend(word);
    Some(words)
}

/// The options every SSH call here carries (OpenCode's desktop app's): no
/// terminal, no remote command of the config's, a bounded connect, and no
/// password prompt — keys or an agent only, as nothing could answer it.
fn ssh_args(target: &SshTarget) -> Vec<String> {
    [
        "-T",
        "-o",
        "BatchMode=yes",
        "-o",
        "ConnectTimeout=10",
        "-o",
        "ServerAliveInterval=15",
        "-o",
        "ServerAliveCountMax=3",
        "-o",
        "RemoteCommand=none",
        "-o",
        "RequestTTY=no",
        "-o",
        "PermitLocalCommand=no",
    ]
    .into_iter()
    .map(str::to_owned)
    .chain(target.args.iter().cloned())
    .collect()
}

const MISSING: &str = "ZERON_OPENCODE=missing";
const VERSION: &str = "ZERON_OPENCODE_VERSION=";
const STATUS: &str = "OPENCODE_SSH_STATUS=";
const BEGIN: &str = "OPENCODE_SSH_REGISTRATION_BEGIN";
const END: &str = "OPENCODE_SSH_REGISTRATION_END";

/// The script the far end runs (`sh -l -s`): find OpenCode (installing it
/// first with OpenCode's own v2 installer when `install` says which
/// version), start its service, and print its registrations.
fn bootstrap_script(install: Option<&str>) -> String {
    let install = match install {
        Some(version) => format!(
            "if [ -z \"$cli\" ]; then\n  curl -fsSL https://raw.githubusercontent.com/anomalyco/opencode/v2/install | bash -s -- {version} >&2\n  if [ -x \"$HOME/.opencode/bin/opencode\" ]; then cli=\"$HOME/.opencode/bin/opencode\"; fi\nfi\n"
        ),
        None => String::new(),
    };
    format!(
        r#"set -eu
cli=$(command -v opencode || true)
if [ -z "$cli" ] && [ -x "$HOME/.opencode/bin/opencode" ]; then cli="$HOME/.opencode/bin/opencode"; fi
{install}if [ -z "$cli" ]; then printf '{MISSING}\n'; exit 0; fi
printf '{VERSION}%s\n' "$("$cli" --version 2>/dev/null || true)"
"$cli" service start >/dev/null 2>&1 || true
status=$("$cli" service status 2>/dev/null) || exit 0
printf '{STATUS}%s\n' "$status"
for file in "${{XDG_STATE_HOME:-$HOME/.local/state}}"/opencode/service*.json; do
  if [ ! -f "$file" ]; then continue; fi
  printf '{BEGIN}\n'
  cat "$file"
  printf '\n{END}\n'
done
"#
    )
}

/// What the far end said: OpenCode missing, or its version and the service
/// that answers (its registration whose address its status names).
#[derive(Debug, PartialEq)]
enum Found {
    Missing,
    Service {
        version: Option<String>,
        info: Option<ServiceInfo>,
    },
}

fn found(output: &str) -> Found {
    if output.lines().any(|line| line.trim() == MISSING) {
        return Found::Missing;
    }
    let version = output
        .lines()
        .rev()
        .find_map(|line| line.strip_prefix(VERSION))
        .map(|v| {
            let v = v.trim();
            v.rsplit_once(" v").map_or(v, |(_, v)| v).to_owned()
        })
        .filter(|v| !v.is_empty());
    let status = output
        .lines()
        .rev()
        .find_map(|line| line.strip_prefix(STATUS))
        .map(str::trim);
    let registrations: Vec<ServiceInfo> = output
        .split(BEGIN)
        .skip(1)
        .filter_map(|chunk| serde_json::from_str(chunk.split(END).next()?.trim()).ok())
        .collect();
    let info = registrations
        .iter()
        .find(|info| Some(info.url.trim_end_matches('/')) == status.map(|s| s.trim_end_matches('/')))
        .or(registrations.first())
        .cloned();
    Found::Service { version, info }
}

/// Where a remote service listens, as its own machine sees it: only a
/// loopback address is forwarded to (OpenCode's desktop app's rule).
fn listen_port(url: &str) -> Option<(String, u16)> {
    let parsed = reqwest::Url::parse(url).ok()?;
    if parsed.scheme() != "http" {
        return None;
    }
    let host = match parsed.host_str()? {
        "127.0.0.1" | "localhost" | "0.0.0.0" => "127.0.0.1",
        "[::1]" | "[::]" | "::1" | "::" => "[::1]",
        _ => return None,
    };
    Some((host.to_owned(), parsed.port_or_known_default()?))
}

async fn run_ssh(target: &SshTarget, script: &str, timeout: Duration) -> Result<String, HarnessError> {
    use tokio::io::AsyncWriteExt as _;
    let mut cmd = Command::new(ssh_exe());
    cmd.args(ssh_args(target))
        .arg(&target.host)
        .arg("sh -l -s")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = cmd
        .spawn()
        .map_err(|e| HarnessError::Protocol(format!("couldn't run ssh: {e}")))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(script.as_bytes())
            .await
            .map_err(|e| HarnessError::Protocol(format!("ssh: {e}")))?;
    }
    let output = tokio::time::timeout(timeout, child.wait_with_output())
        .await
        .map_err(|_| HarnessError::Protocol(format!("ssh to {} timed out", target.host)))?
        .map_err(|e| HarnessError::Protocol(format!("ssh: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(HarnessError::Protocol(format!(
            "ssh to {} failed: {}",
            target.host,
            stderr.trim().lines().last().unwrap_or("it exited with an error")
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn ssh_exe() -> &'static str {
    if cfg!(windows) { "ssh.exe" } else { "ssh" }
}

async fn open_tunnel(
    input: &str,
    client: &reqwest::Client,
    local_version: Option<&semver::Version>,
) -> Result<Tunnel, HarnessError> {
    let target = ssh_target(input)?;
    let mut said = found(&run_ssh(&target, &bootstrap_script(None), SSH_TIMEOUT).await?);
    if said == Found::Missing {
        // OpenCode's own installer, at this computer's version when it has
        // a 2.x (the two then speak alike), else its latest.
        let pin = local_version
            .filter(|v| v.major >= 2)
            .map_or_else(String::new, |v| format!("--version {v}"));
        said = found(&run_ssh(&target, &bootstrap_script(Some(&pin)), INSTALL_TIMEOUT).await?);
    }
    let (version, info) = match said {
        Found::Missing => {
            return Err(HarnessError::Protocol(format!(
                "OpenCode isn't installed on {} and couldn't be installed there",
                target.host
            )));
        }
        Found::Service { version, info } => (version, info),
    };
    if let Some(major) = version
        .as_deref()
        .and_then(|v| v.split('.').next()?.parse::<u64>().ok())
        && major < 2
    {
        return Err(HarnessError::Protocol(format!(
            "OpenCode on {} is {}: a remote server needs OpenCode 2 (update it there)",
            target.host,
            version.as_deref().unwrap_or_default()
        )));
    }
    let info = info.ok_or_else(|| {
        HarnessError::Protocol(format!("OpenCode's service didn't start on {}", target.host))
    })?;
    let (host, port) = listen_port(&info.url).ok_or_else(|| {
        HarnessError::Protocol(format!(
            "OpenCode's service on {} listens at {}, which isn't forwarded (only a loopback address is)",
            target.host, info.url
        ))
    })?;
    let local = super::free_localhost_port()
        .ok_or_else(|| HarnessError::Protocol("no free localhost port for the SSH forward".into()))?;
    let mut cmd = Command::new(ssh_exe());
    cmd.args(["-o", "ControlMaster=no", "-o", "ControlPersist=no"])
        .args(ssh_args(&target))
        .args(["-o", "ExitOnForwardFailure=yes", "-L"])
        .arg(format!("127.0.0.1:{local}:{host}:{port}"))
        .arg(&target.host)
        .arg("sh -c 'exec cat >/dev/null'")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let mut child = cmd
        .spawn()
        .map_err(|e| HarnessError::Protocol(format!("couldn't run ssh: {e}")))?;
    let forwarded = ServiceInfo {
        url: format!("http://127.0.0.1:{local}"),
        password: info.password,
        version: version.or(info.version),
    };
    let deadline = tokio::time::Instant::now() + FORWARD_READY;
    loop {
        if service::healthy(client, &forwarded).await {
            return Ok(Tunnel {
                target: input.to_owned(),
                info: forwarded,
                child,
            });
        }
        if !matches!(child.try_wait(), Ok(None)) || tokio::time::Instant::now() >= deadline {
            return Err(HarnessError::Protocol(format!(
                "the SSH forward to OpenCode's service on {} didn't answer",
                target.host
            )));
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ssh_target_is_a_host_or_a_command_naming_one() {
        assert_eq!(
            ssh_target("me@box").unwrap(),
            SshTarget { args: vec![], host: "me@box".into() }
        );
        assert_eq!(
            ssh_target("ssh -p 2222 -i '~/.ssh/my key' -o User=me box.tail1234.ts.net").unwrap(),
            SshTarget {
                args: ["-p", "2222", "-i", "~/.ssh/my key", "-o", "User=me"].map(String::from).to_vec(),
                host: "box.tail1234.ts.net".into(),
            }
        );
        assert_eq!(ssh_target("ssh -p2222 box").unwrap().args, ["-p", "2222"]);
        // A forward, a remote command, a bad port, an option it may not set:
        // refused.
        for bad in ["ssh -L 1:a:2 box", "ssh box rm -rf /", "ssh -p 0 box", "ssh -o LocalCommand=x box", "", "ssh"] {
            assert!(ssh_target(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_far_end_says_its_version_and_its_service() {
        let output = "ZERON_OPENCODE_VERSION=opencode v2.3.0\nOPENCODE_SSH_STATUS=http://127.0.0.1:4096\n\
            OPENCODE_SSH_REGISTRATION_BEGIN\n{\"url\":\"http://127.0.0.1:9999\",\"password\":\"old\"}\nOPENCODE_SSH_REGISTRATION_END\n\
            OPENCODE_SSH_REGISTRATION_BEGIN\n{\"url\":\"http://127.0.0.1:4096\",\"password\":\"pw\",\"version\":\"2.3.0\"}\nOPENCODE_SSH_REGISTRATION_END\n";
        let Found::Service { version, info } = found(output) else { panic!("no service") };
        assert_eq!(version.as_deref(), Some("2.3.0"));
        let info = info.unwrap();
        assert_eq!((info.url.as_str(), info.password.as_str()), ("http://127.0.0.1:4096", "pw"));
        assert_eq!(found("noise\nZERON_OPENCODE=missing\n"), Found::Missing);
        assert_eq!(listen_port("http://127.0.0.1:4096"), Some(("127.0.0.1".into(), 4096)));
        assert_eq!(listen_port("http://10.0.0.2:4096"), None);
    }
}
