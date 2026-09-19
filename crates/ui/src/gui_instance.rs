//! One GUI per data directory, independent of the engine/daemon lock.
//! The OS lock is authoritative; the authenticated loopback endpoint only
//! forwards activation requests. Stale endpoint files never imply ownership.

use std::{
    collections::VecDeque,
    fs::{File, OpenOptions, TryLockError},
    io::{self, BufRead, BufReader, Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use futures::channel::mpsc::{UnboundedReceiver, unbounded};
use serde::{Deserialize, Serialize};

const VERSION: u32 = 1;
const MAX_MESSAGE: u64 = 16 * 1024;
const RETRY_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum LaunchRequest {
    Activate,
    NewWindow,
    OpenUrl(String),
}

#[derive(Serialize, Deserialize)]
struct Endpoint {
    version: u32,
    port: u16,
    token: String,
}

#[derive(Serialize, Deserialize)]
struct Message {
    version: u32,
    token: String,
    id: String,
    request: LaunchRequest,
}

pub enum Launch {
    Primary(GuiInstance),
    Forwarded,
}

pub struct GuiInstance {
    // Drop the listener and endpoint before releasing ownership.
    _lock: File,
    endpoint_path: PathBuf,
    stop: Arc<AtomicBool>,
    listener: Option<thread::JoinHandle<()>>,
    pub(crate) incoming: Option<UnboundedReceiver<LaunchRequest>>,
}

impl GuiInstance {
    /// Called before logging or opening engine stores, so a forwarded launch
    /// has no application initialization side effects.
    pub fn acquire(data_dir: &Path, request: LaunchRequest) -> anyhow::Result<Launch> {
        std::fs::create_dir_all(data_dir)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(data_dir.join("ui.lock"))?;
        let endpoint_path = data_dir.join("ui-endpoint.json");
        let id = uuid::Uuid::new_v4().to_string();
        let deadline = Instant::now() + RETRY_TIMEOUT;
        loop {
            match lock.try_lock() {
                Ok(()) => return Self::listen(lock, endpoint_path).map(Launch::Primary),
                Err(TryLockError::Error(error)) => return Err(error.into()),
                Err(TryLockError::WouldBlock) => {}
            }
            if let Ok(bytes) = std::fs::read(&endpoint_path)
                && let Ok(endpoint) = serde_json::from_slice::<Endpoint>(&bytes)
            {
                if endpoint.version != VERSION {
                    anyhow::bail!(
                        "The running Zeron uses a different window protocol; quit it and reopen Zeron"
                    );
                }
                if forward(&endpoint, &id, &request).is_ok() {
                    return Ok(Launch::Forwarded);
                }
            }
            if Instant::now() >= deadline {
                anyhow::bail!(
                    "Zeron is running but did not accept the window request; try again after it finishes starting or quitting"
                );
            }
            thread::sleep(Duration::from_millis(50));
        }
    }

    fn listen(lock: File, endpoint_path: PathBuf) -> anyhow::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let endpoint = Endpoint {
            version: VERSION,
            port: listener.local_addr()?.port(),
            token: uuid::Uuid::new_v4().to_string(),
        };
        // Replace stale metadata under the lock. Truncating would temporarily
        // expose an invalid JSON document to a concurrent launcher.
        let temporary = endpoint_path.with_extension("tmp");
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let _ = std::fs::remove_file(&temporary);
        let mut file = options.open(&temporary)?;
        serde_json::to_writer(&mut file, &endpoint)?;
        file.sync_all()?;
        drop(file);
        // Windows rename does not replace an existing destination.
        let _ = std::fs::remove_file(&endpoint_path);
        std::fs::rename(temporary, &endpoint_path)?;
        let (tx, rx) = unbounded();
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let thread = thread::Builder::new()
            .name("zeron-ui-launches".into())
            .spawn(move || {
                let mut accepted = VecDeque::<String>::new();
                while !stopping.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            let result = (|| -> anyhow::Result<()> {
                                // macOS/BSD inherit the listener's nonblocking mode.
                                // Reading a complete request needs blocking I/O with timeouts.
                                stream.set_nonblocking(false)?;
                                stream.set_read_timeout(Some(Duration::from_millis(500)))?;
                                stream.set_write_timeout(Some(Duration::from_millis(500)))?;
                                let message: Message = read_message(&stream)?;
                                anyhow::ensure!(
                                    message.version == VERSION && message.token == endpoint.token,
                                    "invalid launch credentials"
                                );
                                if !accepted.contains(&message.id) {
                                    tx.unbounded_send(message.request)?;
                                    accepted.push_back(message.id);
                                    if accepted.len() > 1024 {
                                        accepted.pop_front();
                                    }
                                }
                                stream.write_all(b"ok\n")?;
                                Ok(())
                            })();
                            if let Err(error) = result {
                                tracing::debug!(%error, "UI launch request rejected");
                            }
                        }
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(20))
                        }
                        Err(error) => {
                            tracing::warn!(%error, "UI launch listener failed");
                            break;
                        }
                    }
                }
            })?;
        Ok(Self {
            _lock: lock,
            endpoint_path,
            stop,
            listener: Some(thread),
            incoming: Some(rx),
        })
    }
}

fn read_message(stream: &TcpStream) -> anyhow::Result<Message> {
    let mut bytes = Vec::new();
    BufReader::new(stream.take(MAX_MESSAGE + 1)).read_until(b'\n', &mut bytes)?;
    anyhow::ensure!(
        bytes.len() <= MAX_MESSAGE as usize && bytes.last() == Some(&b'\n'),
        "invalid launch message length"
    );
    Ok(serde_json::from_slice(&bytes)?)
}

fn forward(endpoint: &Endpoint, id: &str, request: &LaunchRequest) -> anyhow::Result<()> {
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, endpoint.port));
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_millis(200))?;
    stream.set_read_timeout(Some(Duration::from_millis(500)))?;
    stream.set_write_timeout(Some(Duration::from_millis(500)))?;
    let message = Message {
        version: VERSION,
        token: endpoint.token.clone(),
        id: id.into(),
        request: request.clone(),
    };
    let bytes = serde_json::to_vec(&message)?;
    anyhow::ensure!(
        bytes.len() < MAX_MESSAGE as usize,
        "window request is too large"
    );
    stream.write_all(&bytes)?;
    stream.write_all(b"\n")?;
    let mut ack = [0; 3];
    stream.read_exact(&mut ack)?;
    anyhow::ensure!(&ack == b"ok\n", "window request was rejected");
    Ok(())
}

impl Drop for GuiInstance {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(listener) = self.listener.take() {
            let _ = listener.join();
        }
        let _ = std::fs::remove_file(&self.endpoint_path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launches_forward_once_and_a_stale_endpoint_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let Launch::Primary(mut primary) =
            GuiInstance::acquire(dir.path(), LaunchRequest::Activate).unwrap()
        else {
            panic!()
        };
        let endpoint: Endpoint =
            serde_json::from_slice(&std::fs::read(dir.path().join("ui-endpoint.json")).unwrap())
                .unwrap();
        let rx = primary.incoming.as_mut().unwrap();
        forward(&endpoint, "same-request", &LaunchRequest::NewWindow).unwrap();
        forward(&endpoint, "same-request", &LaunchRequest::NewWindow).unwrap();
        assert_eq!(rx.try_recv().unwrap(), LaunchRequest::NewWindow);
        assert!(rx.try_recv().is_err());
        assert!(matches!(
            GuiInstance::acquire(dir.path(), LaunchRequest::Activate).unwrap(),
            Launch::Forwarded
        ));
        assert_eq!(rx.try_recv().unwrap(), LaunchRequest::Activate);
        let stale = serde_json::to_vec(&endpoint).unwrap();
        drop(primary);
        std::fs::write(dir.path().join("ui-endpoint.json"), stale).unwrap();
        assert!(matches!(
            GuiInstance::acquire(dir.path(), LaunchRequest::Activate).unwrap(),
            Launch::Primary(_)
        ));
    }

    #[test]
    fn fragmented_launch_is_acknowledged_and_forwarded_once() {
        let dir = tempfile::tempdir().unwrap();
        let Launch::Primary(mut primary) =
            GuiInstance::acquire(dir.path(), LaunchRequest::Activate).unwrap()
        else {
            panic!()
        };
        let endpoint: Endpoint =
            serde_json::from_slice(&std::fs::read(dir.path().join("ui-endpoint.json")).unwrap())
                .unwrap();
        let message = Message {
            version: VERSION,
            token: endpoint.token.clone(),
            id: "fragmented-request".into(),
            request: LaunchRequest::NewWindow,
        };
        let bytes = serde_json::to_vec(&message).unwrap();
        let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, endpoint.port)).unwrap();
        stream.set_nodelay(true).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(2)))
            .unwrap();

        // Give the listener time to read an incomplete JSON document, then to
        // wait for the newline separately. Neither fragment is a full request.
        let split = bytes.len() / 2;
        stream.write_all(&bytes[..split]).unwrap();
        thread::sleep(Duration::from_millis(100));
        stream.write_all(&bytes[split..]).unwrap();
        thread::sleep(Duration::from_millis(100));
        stream.write_all(b"\n").unwrap();
        let mut ack = [0; 3];
        stream.read_exact(&mut ack).unwrap();
        assert_eq!(&ack, b"ok\n");

        forward(&endpoint, &message.id, &message.request).unwrap();
        let rx = primary.incoming.as_mut().unwrap();
        assert_eq!(rx.try_recv().unwrap(), LaunchRequest::NewWindow);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn wrong_token_is_rejected_and_data_directories_are_independent() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let Launch::Primary(mut primary) =
            GuiInstance::acquire(a.path(), LaunchRequest::Activate).unwrap()
        else {
            panic!()
        };
        let mut endpoint: Endpoint =
            serde_json::from_slice(&std::fs::read(a.path().join("ui-endpoint.json")).unwrap())
                .unwrap();
        endpoint.token = "wrong".into();
        assert!(forward(&endpoint, "bad", &LaunchRequest::NewWindow).is_err());
        assert!(primary.incoming.as_mut().unwrap().try_recv().is_err());
        assert!(matches!(
            GuiInstance::acquire(b.path(), LaunchRequest::Activate).unwrap(),
            Launch::Primary(_)
        ));
    }
}
