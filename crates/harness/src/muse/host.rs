//! One owned `muse serve` host: spawn, the `initialize` handshake, calls,
//! and the host's own requests.
use super::rpc::{Client, Failure, Inbound, Transport};
use crate::{
    HarnessError,
    process::{Command, Stdio, owned::Child},
};
use serde_json::{Value, json};
use std::{path::Path, time::Duration};

/// The protocol envelope this driver speaks.
const SCHEMA_VERSION: u64 = 1;

/// How the host is launched.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Mode {
    /// A coding session: the workspace is trusted, sandboxed unless the run
    /// auto-approves everything.
    Session { sandbox: bool },
    /// Model discovery and title generation: no shell, no writes, no log.
    ReadOnly,
}

impl Mode {
    fn args(self) -> Vec<&'static str> {
        match self {
            Mode::Session { sandbox } => {
                let mut args = vec!["serve", "--trust-workspace"];
                if !sandbox {
                    args.push("--disable-sandbox");
                }
                args
            }
            Mode::ReadOnly => vec![
                "serve",
                "--disable-shell",
                "--disable-write",
                "--no-session-log",
            ],
        }
    }
}

pub(super) struct Host {
    pub child: Child,
    pub transport: Transport,
    pub tail: crate::StderrTail,
    stderr_task: tokio::task::JoinHandle<()>,
    exit_deadline: Option<tokio::time::Instant>,
    /// Capabilities the host granted at `initialize` (`sessionMcp`, …).
    pub granted: Vec<String>,
}

impl Drop for Host {
    fn drop(&mut self) {
        self.stderr_task.abort();
    }
}

impl Host {
    pub fn spawn(
        exe: &Path,
        cwd: &Path,
        mode: Mode,
        run_env: &std::collections::BTreeMap<String, String>,
    ) -> Result<Self, HarnessError> {
        let mut cmd = Command::new(exe);
        crate::process::owned::configure(&mut cmd);
        crate::compose_child_path(&mut cmd, exe);
        // An inherited API key would silently replace the user's own Muse
        // login; a run's explicit one (a Cloud device's) still wins below.
        // Zeron updates Muse itself: no background self-update mid-session.
        cmd.env_remove("META_API_KEY")
            .env("MUSE_NO_AUTO_UPDATE", "1");
        crate::apply_run_env(&mut cmd, run_env);
        cmd.args(mode.args())
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = Child::new(cmd.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                HarnessError::NotInstalled(crate::executable::binary_hint(exe))
            } else {
                HarnessError::Io(e)
            }
        })?);
        let tail = crate::StderrTail::default();
        let mut lines = crate::lossy_lines::LossyLines::new(child.stderr.take().expect("piped stderr"));
        let stderr = tail.clone();
        let stderr_task = tokio::spawn(async move {
            while let Ok(Some(line)) = lines.next_line().await {
                stderr.push(&line);
            }
            stderr.close();
        });
        let transport = Transport::new(
            child.stdin.take().expect("piped stdin"),
            child.stdout.take().expect("piped stdout"),
        );
        Ok(Self {
            child,
            transport,
            tail,
            stderr_task,
            exit_deadline: None,
            granted: Vec::new(),
        })
    }

    pub fn client(&self) -> Client {
        self.transport.client.clone()
    }

    /// The SS1.4 handshake: `initialize`, the envelope check, `initialized`.
    pub async fn initialize(&mut self, read_only: bool) -> Result<(), HarnessError> {
        self.initialize_with(read_only, false).await
    }

    /// [`Self::initialize`], opting into Muse's experimental surface when
    /// `experimental` (the account methods live there in 1.4).
    pub async fn initialize_with(
        &mut self,
        read_only: bool,
        experimental: bool,
    ) -> Result<(), HarnessError> {
        let result = self
            .call(
                "initialize",
                json!({
                    "clientInfo": {
                        "name": "zeron",
                        "title": "Zeron",
                        "version": env!("CARGO_PKG_VERSION"),
                    },
                    "capabilities": {
                        "requestedCapabilities": if read_only { json!([]) } else { json!(["sessionMcp"]) },
                        "experimentalApi": experimental,
                    },
                }),
                &mut Vec::new(),
            )
            .await?
            .map_err(|failure| failed("initialize", &failure))?;
        if result["schema"]["version"].as_u64() != Some(SCHEMA_VERSION) {
            return Err(HarnessError::Protocol(
                "Muse speaks an unsupported protocol version; update Zeron or Muse Code".into(),
            ));
        }
        self.granted = result["grantedCapabilities"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|c| c.as_str().map(str::to_owned))
            .collect();
        self.transport.client.notify("initialized")
    }

    /// The next frame the run must see. Host requests are answered here:
    /// the approval/question they carry also arrives as a notification,
    /// which is where the run acts on it.
    pub async fn next(&mut self) -> Result<Inbound, HarnessError> {
        loop {
            let frame = tokio::select! {
                // Drain buffered output before reporting death: a descendant
                // may keep stdout open, so EOF alone cannot govern liveness.
                biased;
                _ = async {
                    match self.exit_deadline {
                        Some(deadline) => tokio::time::sleep_until(deadline).await,
                        None => std::future::pending().await,
                    }
                } => return Err(HarnessError::Protocol("Muse exited".into())),
                status = self.child.wait(), if self.exit_deadline.is_none() => {
                    status?;
                    self.exit_deadline = Some(tokio::time::Instant::now() + Duration::from_millis(100));
                    continue;
                }
                frame = self.transport.incoming.recv() => frame.unwrap_or_else(|| {
                    Err(HarnessError::Protocol("Muse disconnected".into()))
                })?,
            };
            match frame {
                Inbound::Request { id, method, .. } => {
                    let client = &self.transport.client;
                    if matches!(method.as_str(), "approval/request" | "userInput/request") {
                        client.reply(id, json!({}))?;
                    } else {
                        client.reply_error(
                            id,
                            -32601,
                            &format!("method not found: {method}"),
                            "methodNotFound",
                        )?;
                    }
                }
                frame => return Ok(frame),
            }
        }
    }

    /// Send `method` and wait for its answer; notifications that arrive
    /// meanwhile are kept, in order, in `backlog`. The outer error is the
    /// transport; the inner one Muse's refusal.
    pub async fn call(
        &mut self,
        method: &str,
        params: Value,
        backlog: &mut Vec<Inbound>,
    ) -> Result<Result<Value, Failure>, HarnessError> {
        let id = self.transport.client.request(method, params)?;
        loop {
            match self.next().await? {
                Inbound::Response {
                    id: answered,
                    result,
                } if answered == id => return Ok(result),
                frame => backlog.push(frame),
            }
        }
    }

    pub async fn shutdown(&mut self, grace: Duration) {
        self.child.shutdown(grace).await;
    }
}

/// A refusal as the run's error.
pub(super) fn failed(method: &str, failure: &Failure) -> HarnessError {
    HarnessError::Protocol(format!("Muse {method} failed: {}", failure.message))
}
