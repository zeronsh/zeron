use std::fs::{File, OpenOptions, TryLockError};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context as _, bail};
use futures::channel::mpsc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::runtime::Handle;
use tokio::task::JoinHandle;
use zeron_update::InstallKind;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum LaunchRequest {
    Activate,
    OpenProject { path: String },
    OpenUrl { url: String },
}

pub enum Claim {
    Primary {
        guard: InstanceGuard,
        requests: mpsc::UnboundedReceiver<LaunchRequest>,
        submit: mpsc::UnboundedSender<LaunchRequest>,
    },
    Forwarded,
}

pub struct InstanceGuard {
    _lock: File,
    server: JoinHandle<()>,
    #[cfg(unix)]
    socket: std::path::PathBuf,
}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        self.server.abort();
        #[cfg(unix)]
        let _ = std::fs::remove_file(&self.socket);
    }
}

const LOCK_FILE: &str = "headed.lock";
const REPLY_OK: &[u8] = b"ok\n";
const REPLY_REJECTED: &[u8] = b"rejected\n";
const MAX_REQUEST_BYTES: u64 = 64 * 1024;
const RETRY: Duration = Duration::from_millis(50);
const CONNECTION_BUDGET: Duration = Duration::from_secs(5);
const CLAIM_BUDGET: Duration = Duration::from_secs(15);
const HAND_OFF_BUDGET: Duration = Duration::from_secs(30);

pub fn claim_or_forward(
    data_dir: &Path,
    request: LaunchRequest,
    runtime: &Handle,
) -> anyhow::Result<Claim> {
    std::fs::create_dir_all(data_dir)
        .with_context(|| format!("creating {}", data_dir.display()))?;
    let endpoint = endpoint(data_dir);
    let deadline = Instant::now() + CLAIM_BUDGET;
    loop {
        if let Some(lock) = try_lock(data_dir)? {
            let (submit, requests) = mpsc::unbounded();
            let guard = serve(lock, endpoint, runtime, submit.clone())?;
            let _ = submit.unbounded_send(request);
            return Ok(Claim::Primary {
                guard,
                requests,
                submit,
            });
        }
        let Err(error) = forward(runtime, &endpoint, &request) else {
            return Ok(Claim::Forwarded);
        };
        if Instant::now() >= deadline {
            bail!("Zeron is already running but did not accept the request: {error}");
        }
        std::thread::sleep(RETRY);
    }
}

pub fn hand_off(data_dir: &Path, request: LaunchRequest, runtime: &Handle) -> anyhow::Result<()> {
    std::fs::create_dir_all(data_dir)
        .with_context(|| format!("creating {}", data_dir.display()))?;
    if try_lock(data_dir)?.is_some() {
        spawn_detached()?;
    }
    let endpoint = endpoint(data_dir);
    let deadline = Instant::now() + HAND_OFF_BUDGET;
    loop {
        let Err(error) = forward(runtime, &endpoint, &request) else {
            return Ok(());
        };
        if Instant::now() >= deadline {
            bail!("Zeron did not come up to take the request: {error}");
        }
        std::thread::sleep(RETRY);
    }
}

fn forward(runtime: &Handle, endpoint: &Endpoint, request: &LaunchRequest) -> anyhow::Result<()> {
    runtime.block_on(async {
        match tokio::time::timeout(CONNECTION_BUDGET, send(endpoint, request)).await {
            Ok(result) => result,
            Err(_) => bail!("the running Zeron did not answer"),
        }
    })
}

pub fn installed() -> bool {
    !matches!(zeron_update::detect_install(), InstallKind::Unmanaged)
}

fn spawn_detached() -> anyhow::Result<()> {
    let mut command = match zeron_update::detect_install() {
        InstallKind::MacApp { bundle } => {
            let mut command = Command::new("open");
            command.arg(&bundle);
            for (key, value) in std::env::vars_os() {
                if let (Some(key), Some(value)) = (key.to_str(), value.to_str())
                    && key.starts_with("ZERON_")
                {
                    command.arg("--env").arg(format!("{key}={value}"));
                }
            }
            command.arg("--args");
            command
        }
        _ => Command::new(std::env::current_exe().context("locating the zeron executable")?),
    };
    command
        .arg("--detached")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        use windows_sys::Win32::System::Threading::{CREATE_NEW_PROCESS_GROUP, DETACHED_PROCESS};
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        unsafe {
            command.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }
    command.spawn().context("starting Zeron")?;
    Ok(())
}

fn try_lock(data_dir: &Path) -> anyhow::Result<Option<File>> {
    let path = data_dir.join(LOCK_FILE);
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .with_context(|| format!("opening {}", path.display()))?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(error)) => {
            Err(error).with_context(|| format!("locking {}", path.display()))
        }
    }
}

fn data_dir_key(data_dir: &Path) -> String {
    let absolute = std::path::absolute(data_dir).unwrap_or_else(|_| data_dir.to_path_buf());
    let text = absolute.to_string_lossy();
    let text = if cfg!(windows) {
        text.to_lowercase()
    } else {
        text.into_owned()
    };
    let digest = Sha256::digest(text.as_bytes());
    digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

async fn answer<S: AsyncRead + AsyncWrite + Unpin>(
    stream: S,
    submit: mpsc::UnboundedSender<LaunchRequest>,
) {
    let (reader, mut writer) = tokio::io::split(stream);
    let mut reader = BufReader::new(reader.take(MAX_REQUEST_BYTES));
    let mut line = String::new();
    let read = tokio::time::timeout(CONNECTION_BUDGET, reader.read_line(&mut line)).await;
    let reply = match read {
        Ok(Ok(count)) if count > 0 => {
            match serde_json::from_str::<LaunchRequest>(line.trim_end()) {
                Ok(request) => {
                    let _ = submit.unbounded_send(request);
                    REPLY_OK
                }
                Err(error) => {
                    tracing::warn!(%error, "rejecting a malformed launch request");
                    REPLY_REJECTED
                }
            }
        }
        _ => return,
    };
    if writer.write_all(reply).await.is_ok() {
        let _ = tokio::time::timeout(CONNECTION_BUDGET, reader.read(&mut [0_u8; 1])).await;
    }
}

async fn exchange<S: AsyncRead + AsyncWrite + Unpin>(
    stream: S,
    request: &LaunchRequest,
) -> anyhow::Result<()> {
    let (reader, mut writer) = tokio::io::split(stream);
    let mut payload = serde_json::to_vec(request)?;
    payload.push(b'\n');
    writer.write_all(&payload).await?;
    let mut reply = String::new();
    BufReader::new(reader.take(MAX_REQUEST_BYTES))
        .read_line(&mut reply)
        .await?;
    if reply.as_bytes() != REPLY_OK {
        bail!("the running Zeron rejected the request");
    }
    Ok(())
}

#[cfg(unix)]
mod transport {
    use super::*;
    use tokio::net::{UnixListener, UnixStream};

    const SOCKET_FILE: &str = "headed.sock";
    const SOCKET_PATH_LIMIT: usize = 100;

    pub(super) type Endpoint = std::path::PathBuf;

    pub(super) fn endpoint(data_dir: &Path) -> Endpoint {
        let direct = data_dir.join(SOCKET_FILE);
        if direct.as_os_str().len() < SOCKET_PATH_LIMIT {
            direct
        } else {
            std::env::temp_dir().join(format!("zeron-headed-{}.sock", data_dir_key(data_dir)))
        }
    }

    pub(super) fn serve(
        lock: File,
        socket: Endpoint,
        runtime: &Handle,
        submit: mpsc::UnboundedSender<LaunchRequest>,
    ) -> anyhow::Result<InstanceGuard> {
        let _enter = runtime.enter();
        let _ = std::fs::remove_file(&socket);
        let listener = UnixListener::bind(&socket)
            .with_context(|| format!("listening on {}", socket.display()))?;
        let server = runtime.spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, _)) => {
                        tokio::spawn(answer(stream, submit.clone()));
                    }
                    Err(error) => {
                        tracing::warn!(%error, "launch socket accept failed");
                        tokio::time::sleep(RETRY).await;
                    }
                }
            }
        });
        Ok(InstanceGuard {
            _lock: lock,
            server,
            socket,
        })
    }

    pub(super) async fn send(socket: &Endpoint, request: &LaunchRequest) -> anyhow::Result<()> {
        let stream = UnixStream::connect(socket).await?;
        exchange(stream, request).await
    }
}

#[cfg(windows)]
mod transport {
    use super::*;
    use tokio::net::windows::named_pipe::{ClientOptions, ServerOptions};
    use windows_sys::Win32::Foundation::ERROR_PIPE_BUSY;

    pub(super) type Endpoint = String;

    pub(super) fn endpoint(data_dir: &Path) -> Endpoint {
        format!(r"\\.\pipe\zeron-headed-{}", data_dir_key(data_dir))
    }

    pub(super) fn serve(
        lock: File,
        name: Endpoint,
        runtime: &Handle,
        submit: mpsc::UnboundedSender<LaunchRequest>,
    ) -> anyhow::Result<InstanceGuard> {
        let _enter = runtime.enter();
        let first = ServerOptions::new()
            .first_pipe_instance(true)
            .create(&name)
            .with_context(|| format!("listening on {name}"))?;
        let server = runtime.spawn(async move {
            let mut server = first;
            loop {
                if let Err(error) = server.connect().await {
                    tracing::warn!(%error, "launch pipe connect failed");
                    tokio::time::sleep(RETRY).await;
                    continue;
                }
                let next = loop {
                    match ServerOptions::new().create(&name) {
                        Ok(next) => break next,
                        Err(error) => {
                            tracing::warn!(%error, "launch pipe re-create failed");
                            tokio::time::sleep(RETRY).await;
                        }
                    }
                };
                let client = std::mem::replace(&mut server, next);
                tokio::spawn(answer(client, submit.clone()));
            }
        });
        Ok(InstanceGuard {
            _lock: lock,
            server,
        })
    }

    pub(super) async fn send(name: &Endpoint, request: &LaunchRequest) -> anyhow::Result<()> {
        let client = loop {
            match ClientOptions::new().open(name) {
                Ok(client) => break client,
                Err(error) if error.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) => {
                    tokio::time::sleep(RETRY).await;
                }
                Err(error) => return Err(error.into()),
            }
        };
        exchange(client, request).await
    }
}

use transport::{Endpoint, endpoint, send, serve};

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt as _;

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    #[test]
    fn a_second_launch_forwards_its_request_to_the_primary() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = runtime();
        let Claim::Primary {
            guard,
            mut requests,
            ..
        } = claim_or_forward(dir.path(), LaunchRequest::Activate, runtime.handle()).unwrap()
        else {
            panic!("first launch must become primary");
        };
        let project = LaunchRequest::OpenProject {
            path: dir.path().to_string_lossy().into_owned(),
        };
        assert!(matches!(
            claim_or_forward(dir.path(), project.clone(), runtime.handle()).unwrap(),
            Claim::Forwarded
        ));
        let url = LaunchRequest::OpenUrl {
            url: "zeron://open/chat/a?workspace=b".into(),
        };
        assert!(matches!(
            claim_or_forward(dir.path(), url.clone(), runtime.handle()).unwrap(),
            Claim::Forwarded
        ));
        assert_eq!(
            runtime.block_on(requests.next()),
            Some(LaunchRequest::Activate)
        );
        assert_eq!(runtime.block_on(requests.next()), Some(project));
        assert_eq!(runtime.block_on(requests.next()), Some(url));
        drop(guard);
    }

    #[test]
    fn a_released_primary_lets_the_next_launch_claim() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = runtime();
        let Claim::Primary { guard, .. } =
            claim_or_forward(dir.path(), LaunchRequest::Activate, runtime.handle()).unwrap()
        else {
            panic!("first launch must become primary");
        };
        drop(guard);
        drop(runtime);
        #[cfg(unix)]
        assert!(!endpoint(dir.path()).exists());
        let runtime = self::runtime();
        assert!(matches!(
            claim_or_forward(dir.path(), LaunchRequest::Activate, runtime.handle()).unwrap(),
            Claim::Primary { .. }
        ));
    }
}
