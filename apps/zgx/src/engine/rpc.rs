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
    Watch(Watching),
}

/// A watch's callbacks.
struct Watching {
    /// `None` while it runs (it may start or stop other watches).
    on_item: Option<Box<dyn FnMut(Value)>>,
    /// Told once if it ends of itself (an error, done, the connection gone).
    on_end: Option<Box<dyn FnOnce(String)>>,
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
        self.watch_ending(method, params, on_item, |_| {})
    }

    /// [`Rpc::watch`], and `on_end(why)` if it ends before its handle drops:
    /// its error, "done", or "closed" (the connection ended). Never after
    /// the handle drops.
    pub fn watch_ending(
        &self,
        method: &str,
        params: Value,
        on_item: impl FnMut(Value) + 'static,
        on_end: impl FnOnce(String) + 'static,
    ) -> Watch {
        let id = self.send(method, params);
        self.0.pending.borrow_mut().insert(
            id,
            Pending::Watch(Watching {
                on_item: Some(Box::new(on_item)),
                on_end: Some(Box::new(on_end)),
            }),
        );
        Watch { rpc: self.clone(), id }
    }

    /// Routes frames until the connection ends; returns why it ended.
    pub async fn pump(&self) -> String {
        loop {
            let frames = match self.0.frames.next().await {
                Ok(frames) => frames,
                Err(reason) => {
                    self.0.closed.set(true);
                    // Outstanding calls fail, so their awaiters move on, and
                    // watches are told they ended.
                    let pending: Vec<Pending> = self.0.pending.borrow_mut().drain().map(|(_, p)| p).collect();
                    for pending in pending {
                        match pending {
                            Pending::Call(reply) => {
                                let mut slot = reply.borrow_mut();
                                slot.0 = Some(Err("closed".into()));
                                if let Some(waker) = slot.1.take() {
                                    waker.wake();
                                }
                            }
                            Pending::Watch(Watching { on_end: Some(on_end), .. }) => on_end("closed".into()),
                            Pending::Watch(_) => {}
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
            Some(Pending::Watch(watching)) => {
                if frame.get("done").is_some() || frame.get("err").is_some() {
                    let on_end = watching.on_end.take();
                    pending.remove(&id);
                    drop(pending);
                    if let Some(on_end) = on_end {
                        let why = match frame.get("err") {
                            Some(error) => error.as_str().unwrap_or("error").to_owned(),
                            None => "done".to_owned(),
                        };
                        on_end(why);
                    }
                    return;
                }
                let Some(item) = frame.get("item").cloned() else {
                    return;
                };
                let Some(mut on_item) = watching.on_item.take() else {
                    return;
                };
                drop(pending);
                on_item(item);
                if let Some(Pending::Watch(watching)) = self.0.pending.borrow_mut().get_mut(&id) {
                    watching.on_item = Some(on_item);
                }
            }
            None => {}
        }
    }
}

/// How a watch waiting for the account's setup is watched again: handed to
/// `Rpc::watch_set_up`'s `wait`, called once setup is finished.
pub type Again = Box<dyn FnOnce()>;

/// A watch the engine may answer `setup_incomplete` (signed in, no
/// organization chosen yet: its deferred.rs), watched again, on the same
/// connection, once setup is finished (`Rpc::watch_set_up`). Cancels it
/// when dropped, and is never watched again after.
pub struct SetUpWatch(Rc<RefCell<Option<Watch>>>);

impl Rpc {
    /// [`Rpc::watch`], for a watch the engine answers `setup_incomplete`
    /// until the account's setup is finished: ended so, `wait` is handed
    /// how to watch again (once it's finished: `auth::set_up`), and
    /// `on_item` hears what the new watch sends. Any other end is its own.
    pub fn watch_set_up(
        &self,
        method: &str,
        params: Value,
        on_item: impl FnMut(Value) + 'static,
        wait: impl Fn(Again) + 'static,
    ) -> SetUpWatch {
        let slot: Rc<RefCell<Option<Watch>>> = Rc::default();
        let on_item: Rc<RefCell<dyn FnMut(Value)>> = Rc::new(RefCell::new(on_item));
        let wait: Rc<dyn Fn(Again)> = Rc::new(wait);
        let watch = set_up_watch(self.clone(), method.to_string(), params, on_item, wait, Rc::downgrade(&slot));
        *slot.borrow_mut() = Some(watch);
        SetUpWatch(slot)
    }
}

fn set_up_watch(
    rpc: Rpc,
    method: String,
    params: Value,
    on_item: Rc<RefCell<dyn FnMut(Value)>>,
    wait: Rc<dyn Fn(Again)>,
    slot: std::rc::Weak<RefCell<Option<Watch>>>,
) -> Watch {
    let heard = on_item.clone();
    let (again_rpc, again_method, again_params) = (rpc.clone(), method.clone(), params.clone());
    rpc.watch_ending(
        &method,
        params,
        move |item| (heard.borrow_mut())(item),
        move |why| {
            if !why.contains("setup_incomplete") {
                return;
            }
            let w = wait.clone();
            wait(Box::new(move || {
                // (Dropped meanwhile, or the connection gone: never again.)
                let Some(held) = slot.upgrade() else { return };
                if again_rpc.is_closed() {
                    return;
                }
                let watch = set_up_watch(again_rpc, again_method, again_params, on_item, w, Rc::downgrade(&held));
                *held.borrow_mut() = Some(watch);
            }));
        },
    )
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// A connection with no socket: frames handed to it as the network
    /// thread would.
    fn loopback() -> (Rpc, Frames, OwnedFd) {
        let (heard, bell) = pipe().unwrap();
        let (tx, _rx) = mpsc::channel::<String>();
        let frames = Frames(Arc::default());
        let rpc = Rpc(Rc::new(Inner {
            tx,
            bell: Arc::new(bell),
            frames: frames.clone(),
            next_id: Cell::new(1),
            pending: RefCell::default(),
            closed: Cell::new(false),
        }));
        (rpc, frames, heard)
    }

    #[test]
    fn a_watch_that_ends_says_why_once_and_never_after_its_handle_drops() {
        let (rpc, frames, _heard) = loopback();
        let ended: Rc<RefCell<Vec<String>>> = Rc::default();
        let items: Rc<Cell<u32>> = Rc::default();
        let (e, i) = (ended.clone(), items.clone());
        let watch = rpc.watch_ending("W", json!({}), move |_| i.set(i.get() + 1), move |why| e.borrow_mut().push(why));
        rpc.route(json!({ "id": watch.id, "item": 1 }));
        rpc.route(json!({ "id": watch.id, "item": 2 }));
        assert_eq!(items.get(), 2);
        rpc.route(json!({ "id": watch.id, "err": "no checkout" }));
        assert_eq!(*ended.borrow(), vec!["no checkout".to_string()]);
        // Ended: nothing more, and dropping it tells nothing again.
        rpc.route(json!({ "id": watch.id, "item": 3 }));
        drop(watch);
        assert_eq!((items.get(), ended.borrow().len()), (2, 1));

        // Dropped first: never told.
        let e = ended.clone();
        let quiet = rpc.watch_ending("W", json!({}), |_| {}, move |why| e.borrow_mut().push(why));
        let id = quiet.id;
        drop(quiet);
        rpc.route(json!({ "id": id, "err": "late" }));
        assert_eq!(ended.borrow().len(), 1);

        // The connection gone: each open watch told "closed".
        let e = ended.clone();
        let _open = rpc.watch_ending("W", json!({}), |_| {}, move |why| e.borrow_mut().push(why));
        frames.push(None, Some("gone".into()));
        let mut pump = std::pin::pin!(rpc.pump());
        let waker = std::task::Waker::noop();
        assert!(pump.as_mut().poll(&mut std::task::Context::from_waker(waker)).is_ready());
        assert_eq!(*ended.borrow(), vec!["no checkout".to_string(), "closed".to_string()]);
    }

    /// The engine answers plugin watches `setup_incomplete` until an
    /// organization is chosen: the watch is asked again once it is (`wait`'s
    /// `again`), and what the new one sends (the plugins) is heard, with no
    /// relaunch. Another end isn't retried; one dropped is never asked again.
    #[test]
    fn a_watch_answered_setup_incomplete_is_watched_again_once_set_up() {
        let (rpc, _frames, _heard) = loopback();
        let heard: Rc<RefCell<Vec<Value>>> = Rc::default();
        let waiting: Rc<RefCell<Vec<Again>>> = Rc::default();
        let (h, w) = (heard.clone(), waiting.clone());
        let watch = rpc.watch_set_up("WatchPlugins", json!({}), move |v| h.borrow_mut().push(v), move |again| w.borrow_mut().push(again));
        let first = watch.0.borrow().as_ref().unwrap().id;
        rpc.route(json!({ "id": first, "err": "capability error: setup_incomplete" }));
        assert!(heard.borrow().is_empty());
        assert_eq!(waiting.borrow().len(), 1, "waits for setup");
        // An organization chosen: watched again, and the plugins arrive.
        let again = waiting.borrow_mut().pop().unwrap();
        again();
        let second = watch.0.borrow().as_ref().unwrap().id;
        assert_ne!(first, second, "a new watch");
        rpc.route(json!({ "id": second, "item": { "plugins": [{ "id": "git", "status": "on" }] } }));
        assert_eq!(heard.borrow().len(), 1);
        assert_eq!(heard.borrow()[0]["plugins"][0]["id"], "git");
        // Another end: its own, no wait.
        rpc.route(json!({ "id": second, "err": "offline" }));
        assert!(waiting.borrow().is_empty());
        // Dropped while waiting: never watched again.
        let (h, w) = (heard.clone(), waiting.clone());
        let dropped = rpc.watch_set_up("WatchPlugins", json!({}), move |v| h.borrow_mut().push(v), move |again| w.borrow_mut().push(again));
        let id = dropped.0.borrow().as_ref().unwrap().id;
        rpc.route(json!({ "id": id, "err": "setup_incomplete" }));
        drop(dropped);
        let again = waiting.borrow_mut().pop().unwrap();
        let before = rpc.0.next_id.get();
        again();
        assert_eq!(rpc.0.next_id.get(), before, "nothing asked");
    }
}
