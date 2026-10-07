//! The SSH transport: one russh session per machine, the engine's loopback
//! WebSocket IPC carried over a `direct-tcpip` channel (nothing listens on
//! the phone), TOFU host-key pinning, ed25519 key generation/import.

use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use russh::client;
use russh::keys::ssh_key::{LineEnding, private::Ed25519Keypair};
use russh::keys::{HashAlg, PrivateKey, PrivateKeyWithHashAlg, PublicKeyOrCertificate};

use super::{SshAuth, SshError, SshTarget};

/// TCP connect + SSH handshake budget (mainland mobile networks are slow).
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(20);
/// SSH-level keepalive: a dead path is noticed within ~interval × max.
const KEEPALIVE: Duration = Duration::from_secs(15);
const KEEPALIVE_MAX: usize = 3;

/// A freshly generated or imported key, in OpenSSH formats.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshKeyPair {
    /// `-----BEGIN OPENSSH PRIVATE KEY-----` PEM (unencrypted).
    pub private_openssh: String,
    /// `ssh-ed25519 AAAA… comment` — the authorized_keys line.
    pub public_openssh: String,
    /// `SHA256:…` (ssh-keygen -lf format).
    pub fingerprint: String,
}

/// New ed25519 key from the OS RNG.
pub fn generate_ed25519(comment: &str) -> Result<SshKeyPair, SshError> {
    use ring::rand::SecureRandom;
    let mut seed = [0u8; 32];
    ring::rand::SystemRandom::new()
        .fill(&mut seed)
        .map_err(|_| SshError::Key("system RNG unavailable".into()))?;
    let mut key = PrivateKey::from(Ed25519Keypair::from_seed(&seed));
    key.set_comment(comment);
    describe(&key)
}

/// Parse a pasted private key (OpenSSH/PEM/PPK, optionally encrypted) and
/// return it re-encoded unencrypted plus its public half.
pub fn import_key(text: &str, passphrase: Option<&str>) -> Result<SshKeyPair, SshError> {
    let key = decode(text, passphrase)?;
    describe(&key)
}

fn decode(text: &str, passphrase: Option<&str>) -> Result<PrivateKey, SshError> {
    russh::keys::decode_secret_key(text.trim(), passphrase.filter(|p| !p.is_empty()))
        .map_err(|e| SshError::Key(format!("can't read the private key: {e}")))
}

fn describe(key: &PrivateKey) -> Result<SshKeyPair, SshError> {
    let private_openssh = key
        .to_openssh(LineEnding::LF)
        .map_err(|e| SshError::Key(e.to_string()))?
        .to_string();
    let public = key.public_key();
    let public_openssh = public
        .to_openssh()
        .map_err(|e| SshError::Key(e.to_string()))?;
    Ok(SshKeyPair {
        private_openssh,
        public_openssh,
        fingerprint: public.fingerprint(HashAlg::Sha256).to_string(),
    })
}

/// What the server presented during the handshake.
#[derive(Debug, Clone, Default)]
struct SeenKey {
    fingerprint: String,
    algorithm: String,
}

struct Handler {
    pinned: Option<String>,
    seen: Arc<Mutex<Option<SeenKey>>>,
}

impl client::Handler for Handler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let public = match key {
            PublicKeyOrCertificate::PublicKey { key, .. } => key.clone(),
            PublicKeyOrCertificate::Certificate(cert) => {
                russh::keys::PublicKey::from(cert.public_key().clone())
            }
        };
        let fingerprint = public.fingerprint(HashAlg::Sha256).to_string();
        let algorithm = public.algorithm().as_str().to_owned();
        let ok = self.pinned.as_deref() == Some(fingerprint.as_str());
        *crate::lock(&self.seen) = Some(SeenKey {
            fingerprint,
            algorithm,
        });
        Ok(ok)
    }
}

/// An authenticated SSH session to one machine.
pub(crate) struct SshSession {
    handle: client::Handle<Handler>,
    pub(crate) host_fingerprint: String,
    pub(crate) host_algorithm: String,
}

impl SshSession {
    pub(crate) fn is_closed(&self) -> bool {
        self.handle.is_closed()
    }

    /// Open the engine's WebSocket over a fresh `direct-tcpip` channel.
    pub(crate) async fn open_engine(
        &self,
        engine_port: u16,
    ) -> Result<zeron_rpc::RpcClient, SshError> {
        self.open_engine_counted(engine_port)
            .await
            .map(|(rpc, _)| rpc)
    }

    /// [`Self::open_engine`], with a running count of the bytes received on
    /// the channel (after SSH decompression): how far a multi-MB message in
    /// progress has come, which the WebSocket only hands over once whole.
    pub(crate) async fn open_engine_counted(
        &self,
        engine_port: u16,
    ) -> Result<(zeron_rpc::RpcClient, Arc<AtomicU64>), SshError> {
        let channel = tokio::time::timeout(
            HANDSHAKE_TIMEOUT,
            self.handle
                .channel_open_direct_tcpip("127.0.0.1", u32::from(engine_port), "127.0.0.1", 0),
        )
        .await
        .map_err(|_| SshError::Engine("timed out opening the tunnel".into()))?
        .map_err(|e| {
            SshError::Engine(format!(
                "the machine refused a tunnel to 127.0.0.1:{engine_port} ({e}). Is Zeron running there?"
            ))
        })?;
        let received = Arc::new(AtomicU64::new(0));
        let stream = Counted {
            inner: channel.into_stream(),
            received: received.clone(),
        };
        zeron_rpc::connect_ws_stream(&format!("ws://127.0.0.1:{engine_port}/"), stream)
            .await
            .map(|rpc| (rpc, received))
            .map_err(|e| {
                SshError::Engine(format!(
                    "no Zeron engine answered on 127.0.0.1:{engine_port} ({e})"
                ))
            })
    }

    /// Run one read-only command on the machine (an `exec` channel) and
    /// return what it printed, at most `limit` bytes; `None` if it can't be
    /// run, fails, or takes longer than `within`.
    pub(crate) async fn exec_read(
        &self,
        command: &str,
        limit: usize,
        within: Duration,
    ) -> Option<Vec<u8>> {
        let run = async {
            let mut channel = self.handle.channel_open_session().await.ok()?;
            channel.exec(true, command).await.ok()?;
            let mut out = Vec::new();
            let mut status = None;
            while let Some(msg) = channel.wait().await {
                match msg {
                    russh::ChannelMsg::Data { data } => {
                        if out.len() + data.len() > limit {
                            return None;
                        }
                        out.extend_from_slice(&data);
                    }
                    russh::ChannelMsg::ExitStatus { exit_status } => status = Some(exit_status),
                    russh::ChannelMsg::Failure => return None,
                    russh::ChannelMsg::Close => break,
                    _ => {}
                }
            }
            (status == Some(0)).then_some(out)
        };
        tokio::time::timeout(within, run).await.ok().flatten()
    }

    pub(crate) async fn close(&self) {
        let _ = self
            .handle
            .disconnect(russh::Disconnect::ByApplication, "", "en")
            .await;
    }
}

/// Connect + verify the host key + authenticate.
pub(crate) async fn connect(target: &SshTarget) -> Result<SshSession, SshError> {
    connect_within(target, HANDSHAKE_TIMEOUT).await
}

/// [`connect`], giving up reaching the address (TCP + SSH handshake) after
/// `reach` instead of the default.
pub(crate) async fn connect_within(
    target: &SshTarget,
    reach: Duration,
) -> Result<SshSession, SshError> {
    let config = Arc::new(client::Config {
        keepalive_interval: Some(KEEPALIVE),
        keepalive_max: KEEPALIVE_MAX,
        inactivity_timeout: None,
        nodelay: true,
        // russh parks its whole session loop when a channel's queue is full;
        // leave room for a burst of snapshot frames.
        channel_buffer_size: 1024,
        // Ask for zlib (OpenSSH's delayed zlib@openssh.com first). russh
        // lists "none" first by default, so the session ran uncompressed
        // although Windows OpenSSH offers zlib (Compression delayed is its
        // default). Transcript JSON deflates ~3x (measured on a 0.2.101
        // engine: tail 56→17 KB, reset 1.7→0.55 MB / 8.4→2.95 MB, a running
        // turn's 80 KB update → 24 KB), which on a ~20 KB/s DERP relay is the
        // difference between a running chat keeping up and falling minutes
        // behind. A server without zlib negotiates "none" as before.
        preferred: russh::Preferred {
            compression: std::borrow::Cow::Borrowed(&[
                russh::compression::ZLIB_LEGACY,
                russh::compression::ZLIB,
                russh::compression::NONE,
            ]),
            ..russh::Preferred::DEFAULT
        },
        ..Default::default()
    });
    let seen = Arc::new(Mutex::new(None));
    let handler = Handler {
        pinned: target
            .host_key_fingerprint
            .clone()
            .filter(|f| !f.trim().is_empty()),
        seen: seen.clone(),
    };
    let addr = (target.host.trim().to_owned(), target.port);
    let connected = tokio::time::timeout(reach, client::connect(config, addr, handler)).await;
    let seen_key = crate::lock(&seen).clone();
    let mut handle = match connected {
        Err(_) => {
            return Err(SshError::Connect(format!(
                "timed out reaching {}:{}",
                target.host, target.port
            )));
        }
        Ok(Err(err)) => {
            // A rejected host key surfaces as a handshake error: report it
            // as the pinning outcome, not a network failure.
            if let Some(key) = seen_key {
                return Err(host_key_error(target, key));
            }
            return Err(SshError::Connect(connect_message(&err, target)));
        }
        Ok(Ok(handle)) => handle,
    };
    let key = seen_key.unwrap_or_default();
    let user = target.user.trim().to_owned();
    let auth = match &target.auth {
        SshAuth::Key {
            private_key,
            passphrase,
        } => {
            let key = decode(private_key, passphrase.as_deref())?;
            // ed25519/ecdsa only (no RSA feature): no hash negotiation.
            let hash = None;
            handle
                .authenticate_publickey(
                    user.clone(),
                    PrivateKeyWithHashAlg::new(Arc::new(key), hash),
                )
                .await
        }
        SshAuth::Password { password } => {
            handle
                .authenticate_password(user.clone(), password.clone())
                .await
        }
    }
    .map_err(|e| SshError::Auth(e.to_string()))?;
    if !auth.success() {
        return Err(SshError::Auth(match target.auth {
            SshAuth::Key { .. } => format!(
                "the machine rejected this phone's key for user \"{user}\" — add the public key to authorized_keys (administrators_authorized_keys for admin accounts on Windows)"
            ),
            SshAuth::Password { .. } => format!("wrong password for user \"{user}\""),
        }));
    }
    Ok(SshSession {
        handle,
        host_fingerprint: key.fingerprint,
        host_algorithm: key.algorithm,
    })
}

fn host_key_error(target: &SshTarget, key: SeenKey) -> SshError {
    match target
        .host_key_fingerprint
        .as_deref()
        .filter(|f| !f.trim().is_empty())
    {
        None => SshError::HostKeyUnknown {
            fingerprint: key.fingerprint,
            algorithm: key.algorithm,
        },
        Some(expected) => SshError::HostKeyMismatch {
            expected: expected.to_owned(),
            actual: key.fingerprint,
            algorithm: key.algorithm,
        },
    }
}

fn connect_message(err: &russh::Error, target: &SshTarget) -> String {
    let text = err.to_string();
    if text.contains("refused") {
        format!(
            "{}:{} refused the connection — is OpenSSH Server running?",
            target.host, target.port
        )
    } else {
        format!("can't reach {}:{} ({text})", target.host, target.port)
    }
}

/// Status probe: SSH auth + tunnel + `EngineInfo`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeResult {
    pub host_key_fingerprint: String,
    pub host_key_algorithm: String,
    pub engine_device_id: String,
    pub engine_version: Option<String>,
    pub latency_ms: u64,
}

pub async fn probe(target: &SshTarget) -> Result<ProbeResult, SshError> {
    let started = Instant::now();
    let session = connect(target).await?;
    let result = async {
        let rpc = session.open_engine(target.engine_port).await?;
        let info = tokio::time::timeout(
            Duration::from_secs(15),
            rpc.call(zeron_rpc::methods::ENGINE_INFO, serde_json::json!({})),
        )
        .await
        .map_err(|_| SshError::Engine("EngineInfo timed out".into()))?
        .map_err(|e| SshError::Engine(e.to_string()))?;
        Ok(ProbeResult {
            host_key_fingerprint: session.host_fingerprint.clone(),
            host_key_algorithm: session.host_algorithm.clone(),
            engine_device_id: info
                .get("deviceId")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_owned(),
            engine_version: info
                .get("version")
                .and_then(|v| v.as_str())
                .map(str::to_owned),
            latency_ms: started.elapsed().as_millis() as u64,
        })
    }
    .await;
    session.close().await;
    result
}

/// A byte stream that counts what it reads (see
/// [`SshSession::open_engine_counted`]).
struct Counted<S> {
    inner: S,
    received: Arc<AtomicU64>,
}

impl<S: tokio::io::AsyncRead + Unpin> tokio::io::AsyncRead for Counted<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let polled = Pin::new(&mut self.inner).poll_read(cx, buf);
        if let Poll::Ready(Ok(())) = polled {
            let read = (buf.filled().len() - before) as u64;
            self.received.fetch_add(read, Ordering::Relaxed);
        }
        polled
    }
}

impl<S: tokio::io::AsyncWrite + Unpin> tokio::io::AsyncWrite for Counted<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}
