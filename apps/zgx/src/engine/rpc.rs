//! A client for a Zeron engine's local IPC: one WebSocket at
//! `ws://127.0.0.1:{port}`, one JSON object per message. Requests are
//! `{id, method, params}`; replies `{id, ok}` or `{id, err}`; watches stream
//! `{id, item}` until `{id, done}` and stop with `{id, cancel: true}`
//! (crates/rpc/src/lib.rs in zeronsh/zeron).
//!
//! The engine refuses a handshake without `Authorization: Bearer <token>`,
//! the token it writes to `{data_dir}/ipc-token` on every start
//! (crates/rpc/src/ipc_auth.rs), so a dial reads the file afresh.
//!
//! Adapted from zgx's `examples/support/zeron_rpc.rs` (which predates the
//! token): a background thread owns the socket; the UI thread routes frames
//! by id. The thread sleeps until the socket has something or a request is
//! sent (a byte down a pipe it waits on beside the socket), so an idle
//! connection costs nothing; it once read with a 15 ms timeout to look for
//! requests, and woke some 70 times a second for nothing.

use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, VecDeque},
    future::Future,
    net::{SocketAddr, TcpStream},
    path::Path,
    pin::Pin,
    rc::Rc,
    sync::{Arc, Mutex, mpsc},
    task::{Poll, Waker},
    time::Duration,
};
#[cfg(unix)]
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

/// How long a dial may take: a real engine answers on localhost in
/// milliseconds; something else holding the port must not hang us.
const DIAL_TIMEOUT: Duration = Duration::from_secs(5);

/// Why a dial didn't connect.
#[derive(Debug)]
pub enum DialError {
    /// Nothing serves the port.
    Refused,
    /// The engine refused the token (stale, or none yet): it may be
    /// restarting, about to write a new one.
    Unauthorized,
    Other(String),
}

/// The engine's current IPC token, if it has written one.
pub fn read_token(data_dir: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(data_dir.join("ipc-token")).ok()?;
    let token = raw.trim();
    (!token.is_empty()).then(|| token.to_string())
}

#[derive(Default)]
struct Inbox {
    frames: VecDeque<Value>,
    closed: Option<String>,
    waker: Option<Waker>,
}

/// Frames from the network thread, awaited on the UI thread.
#[derive(Clone)]
struct Frames(Arc<Mutex<Inbox>>);

impl Frames {
    fn push(&self, frame: Option<Value>, closed: Option<String>) {
        let waker = {
            let mut inbox = self.0.lock().unwrap();
            inbox.frames.extend(frame);
            if closed.is_some() {
                inbox.closed = closed;
            }
            inbox.waker.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }

    /// The next batch of frames, or why the connection ended.
    fn next(&self) -> impl Future<Output = Result<Vec<Value>, String>> + '_ {
        std::future::poll_fn(move |cx| {
            let mut inbox = self.0.lock().unwrap();
            if !inbox.frames.is_empty() {
                return Poll::Ready(Ok(inbox.frames.drain(..).collect()));
            }
            if let Some(reason) = inbox.closed.clone() {
                return Poll::Ready(Err(reason));
            }
            inbox.waker = Some(cx.waker().clone());
            Poll::Pending
        })
    }
}

type Socket = tungstenite::WebSocket<TcpStream>;

/// The handshake, blocking (on the network thread).
fn dial(port: u16, token: Option<&str>) -> Result<Socket, DialError> {
    use tungstenite::client::IntoClientRequest;
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    let stream = TcpStream::connect_timeout(&address, DIAL_TIMEOUT).map_err(|error| {
        if error.kind() == std::io::ErrorKind::ConnectionRefused {
            DialError::Refused
        } else {
            DialError::Other(error.to_string())
        }
    })?;
    let _ = stream.set_read_timeout(Some(DIAL_TIMEOUT));
    let _ = stream.set_nodelay(true);
    let mut request = format!("ws://127.0.0.1:{port}")
        .into_client_request()
        .map_err(|error| DialError::Other(error.to_string()))?;
    if let Some(token) = token {
        let value = tungstenite::http::HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|_| DialError::Unauthorized)?;
        request
            .headers_mut()
            .insert(tungstenite::http::header::AUTHORIZATION, value);
    }
    // A long reply can be large; local IPC allows up to 256 MiB.
    const LIMIT: usize = 256 << 20;
    let config = tungstenite::protocol::WebSocketConfig::default()
        .max_message_size(Some(LIMIT))
        .max_frame_size(Some(LIMIT));
    match tungstenite::client::client_with_config(request, stream, Some(config)) {
        Ok((socket, _)) => Ok(socket),
        Err(tungstenite::HandshakeError::Failure(tungstenite::Error::Http(response)))
            if response.status().as_u16() == 401 =>
        {
            Err(DialError::Unauthorized)
        }
        Err(error) => Err(DialError::Other(error.to_string())),
    }
}

/// Whether a socket call only found nothing to do yet: no frame to read, or
/// no room to write (the frame waits in tungstenite's buffer).
pub(crate) fn would_block(error: &tungstenite::Error) -> bool {
    matches!(error, tungstenite::Error::Io(e)
        if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut))
}

/// A pipe for waking a thread that waits in [`wait`]: both ends
/// non-blocking and closed on exec. (Read end, write end.)
#[cfg(unix)]
pub(crate) fn pipe() -> std::io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0; 2];
    // SAFETY: pipe fills two fds, owned from here on; fcntl only sets flags.
    unsafe {
        if libc::pipe(fds.as_mut_ptr()) != 0 {
            return Err(std::io::Error::last_os_error());
        }
        let ends = (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1]));
        for fd in fds {
            libc::fcntl(fd, libc::F_SETFL, libc::fcntl(fd, libc::F_GETFL) | libc::O_NONBLOCK);
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
        }
        Ok(ends)
    }
}

/// Wakes the thread waiting on `pipe`'s other end (a byte; one already
/// waiting is enough, so a full pipe is fine).
#[cfg(unix)]
pub(crate) fn ring(bell: &OwnedFd) {
    // SAFETY: a one-byte write to an fd this owns.
    unsafe {
        libc::write(bell.as_raw_fd(), [1u8].as_ptr().cast(), 1);
    }
}

/// Empties a pipe [`ring`] wrote to; false once its write end has closed.
#[cfg(unix)]
pub(crate) fn drain(heard: &OwnedFd) -> bool {
    let mut buf = [0u8; 64];
    loop {
        // SAFETY: reads into a buffer of its own length from an fd this owns.
        match unsafe { libc::read(heard.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) } {
            0 => return false,
            n if n > 0 => continue,
            _ => return true,
        }
    }
}

/// Sleeps until one of `fds` is ready as asked, or a signal arrives, with no
/// timeout: what `revents` says, filled in.
#[cfg(unix)]
pub(crate) fn wait(fds: &mut [libc::pollfd]) {
    // SAFETY: `fds` is a valid slice of pollfds for its whole length.
    unsafe {
        libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, -1);
    }
}

#[cfg(unix)]
pub(crate) fn poll_fd(fd: &impl AsRawFd, events: libc::c_short) -> libc::pollfd {
    libc::pollfd { fd: fd.as_raw_fd(), events, revents: 0 }
}

/// The network thread: requests out, frames in, and asleep between.
fn serve(mut socket: Socket, rx: mpsc::Receiver<String>, frames: Frames, #[cfg(unix)] heard: OwnedFd) {
    // Without a pipe to wait on (not unix), short reads let the thread
    // interleave outgoing requests.
    #[cfg(unix)]
    let _ = socket.get_ref().set_nonblocking(true);
    #[cfg(not(unix))]
    let _ = socket.get_ref().set_read_timeout(Some(Duration::from_millis(15)));
    let fail = |error: tungstenite::Error| frames.push(None, Some(error.to_string()));
    loop {
        loop {
            match rx.try_recv() {
                Ok(text) => match socket.write(tungstenite::Message::text(text)) {
                    Ok(()) => {}
                    Err(error) if would_block(&error) => {}
                    Err(error) => return fail(error),
                },
                Err(mpsc::TryRecvError::Empty) => break,
                // The client was dropped: close quietly.
                Err(mpsc::TryRecvError::Disconnected) => {
                    let _ = socket.close(None);
                    let _ = socket.flush();
                    return;
                }
            }
        }
        // What didn't go yet waits until the socket has room.
        let unsent = match socket.flush() {
            Ok(()) => false,
            Err(error) if would_block(&error) => true,
            Err(error) => return fail(error),
        };
        // Every frame there is.
        loop {
            match socket.read() {
                Ok(tungstenite::Message::Text(text)) => {
                    for line in text.as_str().lines().filter(|l| !l.trim().is_empty()) {
                        if let Ok(frame) = serde_json::from_str(line) {
                            frames.push(Some(frame), None);
                        }
                    }
                }
                Ok(tungstenite::Message::Close(_)) => {
                    frames.push(None, Some("closed".into()));
                    return;
                }
                Ok(_) => {}
                Err(error) if would_block(&error) => break,
                Err(error) => return fail(error),
            }
        }
        #[cfg(unix)]
        {
            let out = if unsent { libc::POLLOUT } else { 0 };
            let mut fds = [poll_fd(socket.get_ref(), libc::POLLIN | out), poll_fd(&heard, libc::POLLIN)];
            wait(&mut fds);
            if fds[1].revents != 0 {
                // (Its write end closes with the client, whose channel
                // says so next time round.)
                drain(&heard);
            }
        }
        #[cfg(not(unix))]
        let _ = unsent;
    }
}

/// Connects on a background thread, resolving once the socket is open.
pub fn connect(port: u16, token: Option<String>) -> impl Future<Output = Result<Rpc, DialError>> {
    let frames = Frames(Arc::default());
    let (tx, rx) = mpsc::channel::<String>();
    let opened = Arc::new(Mutex::new((None::<Result<(), DialError>>, None::<Waker>)));
    #[cfg(unix)]
    let (heard, bell) = match pipe() {
        Ok((heard, bell)) => (Some(heard), Some(Arc::new(bell))),
        Err(error) => {
            opened.lock().unwrap().0 = Some(Err(DialError::Other(error.to_string())));
            (None, None)
        }
    };
    let (thread_frames, thread_opened) = (frames.clone(), opened.clone());
    let open = move |result: Result<(), DialError>| {
        let waker = {
            let mut state = thread_opened.lock().unwrap();
            state.0 = Some(result);
            state.1.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    };
    #[cfg(unix)]
    let ready = heard.is_some();
    #[cfg(not(unix))]
    let ready = true;
    let spawned = ready.then(|| {
        std::thread::Builder::new().name("zeron-ipc".into()).spawn(move || {
            let socket = match dial(port, token.as_deref()) {
                Ok(socket) => socket,
                Err(error) => {
                    open(Err(error));
                    return;
                }
            };
            open(Ok(()));
            #[cfg(unix)]
            if let Some(heard) = heard {
                serve(socket, rx, thread_frames, heard);
            }
            #[cfg(not(unix))]
            serve(socket, rx, thread_frames);
        })
    });
    if let Some(Err(error)) = spawned {
        let mut state = opened.lock().unwrap();
        state.0 = Some(Err(DialError::Other(error.to_string())));
    }
    let mut client = Some((tx, frames));
    #[cfg(unix)]
    let mut bell = bell;
    std::future::poll_fn(move |cx| {
        let mut state = opened.lock().unwrap();
        match state.0.take() {
            Some(Ok(())) => {
                let (tx, frames) = client.take().expect("polled after completion");
                Poll::Ready(Ok(Rpc(Rc::new(Inner {
                    tx,
                    #[cfg(unix)]
                    bell: bell.take().expect("a bell, as the thread was started"),
                    frames,
                    next_id: Cell::new(1),
                    pending: RefCell::default(),
                    closed: Cell::new(false),
                }))))
            }
            Some(Err(error)) => Poll::Ready(Err(error)),
            None => {
                state.1 = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    })
}

type Reply = Rc<RefCell<(Option<Result<Value, String>>, Option<Waker>)>>;

enum Pending {
    Call(Reply),
    /// `None` while its callback runs (it may start or stop other watches).
    Watch(Option<Box<dyn FnMut(Value)>>),
}

struct Inner {
    tx: mpsc::Sender<String>,
    /// Wakes the network thread for what `tx` sent. (After `tx`, so the
    /// channel has closed by the time the thread wakes to its pipe closing.)
    #[cfg(unix)]
    bell: Arc<OwnedFd>,
    frames: Frames,
    next_id: Cell<u64>,
    pending: RefCell<HashMap<u64, Pending>>,
    closed: Cell<bool>,
}

/// One connection. Cheap to clone; calls made after it closed fail at once.
#[derive(Clone)]
pub struct Rpc(Rc<Inner>);

impl Rpc {
    fn send(&self, method: &str, params: Value) -> u64 {
        let id = self.0.next_id.get();
        self.0.next_id.set(id + 1);
        self.post(json!({ "id": id, "method": method, "params": params }).to_string());
        id
    }

    /// A frame for the network thread to send, and the thread woken for it.
    fn post(&self, text: String) {
        let _ = self.0.tx.send(text);
        #[cfg(unix)]
        ring(&self.0.bell);
    }

    pub fn is_closed(&self) -> bool {
        self.0.closed.get()
    }

    /// A unary call: its `ok`, or its `err`.
    pub fn call(&self, method: &str, params: Value) -> Pin<Box<dyn Future<Output = Result<Value, String>>>> {
        let reply: Reply = Rc::default();
        if self.is_closed() {
            reply.borrow_mut().0 = Some(Err("closed".into()));
        } else {
            let id = self.send(method, params);
            self.0
                .pending
                .borrow_mut()
                .insert(id, Pending::Call(reply.clone()));
        }
        Box::pin(std::future::poll_fn(move |cx| {
            let mut slot = reply.borrow_mut();
            match slot.0.take() {
                Some(result) => Poll::Ready(result),
                None => {
                    slot.1 = Some(cx.waker().clone());
                    Poll::Pending
                }
            }
        }))
    }

    /// A watch: `on_item` sees the current value, then every change, until
    /// the handle drops or the connection ends.
    pub fn watch(&self, method: &str, params: Value, on_item: impl FnMut(Value) + 'static) -> Watch {
        let id = self.send(method, params);
        self.0
            .pending
            .borrow_mut()
            .insert(id, Pending::Watch(Some(Box::new(on_item))));
        Watch { rpc: self.clone(), id }
    }

    /// Routes frames until the connection ends; returns why it ended.
    pub async fn pump(&self) -> String {
        loop {
            let frames = match self.0.frames.next().await {
                Ok(frames) => frames,
                Err(reason) => {
                    self.0.closed.set(true);
                    // Outstanding calls fail, so their awaiters move on.
                    for (_, pending) in self.0.pending.borrow_mut().drain() {
                        if let Pending::Call(reply) = pending {
                            let mut slot = reply.borrow_mut();
                            slot.0 = Some(Err("closed".into()));
                            if let Some(waker) = slot.1.take() {
                                waker.wake();
                            }
                        }
                    }
                    return reason;
                }
            };
            for frame in frames {
                self.route(frame);
            }
        }
    }

    fn route(&self, frame: Value) {
        let Some(id) = frame.get("id").and_then(Value::as_u64) else {
            return;
        };
        let result = if let Some(error) = frame.get("err") {
            Some(Err(error.as_str().unwrap_or("error").to_owned()))
        } else {
            frame.get("ok").map(|ok| Ok(ok.clone()))
        };
        let mut pending = self.0.pending.borrow_mut();
        match pending.get_mut(&id) {
            Some(Pending::Call(_)) => {
                if let (Some(Pending::Call(reply)), Some(result)) = (pending.remove(&id), result) {
                    drop(pending);
                    let mut slot = reply.borrow_mut();
                    slot.0 = Some(result);
                    if let Some(waker) = slot.1.take() {
                        waker.wake();
                    }
                }
            }
            Some(Pending::Watch(callback)) => {
                if frame.get("done").is_some() || frame.get("err").is_some() {
                    pending.remove(&id);
                    return;
                }
                let Some(item) = frame.get("item").cloned() else {
                    return;
                };
                let Some(mut on_item) = callback.take() else {
                    return;
                };
                drop(pending);
                on_item(item);
                if let Some(Pending::Watch(slot)) = self.0.pending.borrow_mut().get_mut(&id) {
                    *slot = Some(on_item);
                }
            }
            None => {}
        }
    }
}

/// Cancels its watch when dropped.
pub struct Watch {
    rpc: Rpc,
    id: u64,
}

impl Drop for Watch {
    fn drop(&mut self) {
        if self.rpc.0.pending.borrow_mut().remove(&self.id).is_some() && !self.rpc.is_closed() {
            self.rpc.post(json!({ "id": self.id, "cancel": true }).to_string());
        }
    }
}
