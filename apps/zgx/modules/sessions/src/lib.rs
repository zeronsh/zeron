//! The chat list as a native module (Cargo.toml says why). On the
//! interface's thread: what scripts call, and the jobs the connection's
//! thread posts. On the connection's thread: the engine's socket, read and
//! parsed there, and each list diffed there against the last by chat id, so
//! the interface's thread makes values only for the rows that changed or
//! moved and writes them by key (`Shared::upsert_at`, `Shared::remove`).
//! The diff reads each row's bytes where the frame has them (a hash each,
//! and its id), keeps only the last list's ids and hashes, and parses only
//! the rows it posts; the rows that keep their order are a longest
//! increasing run of their old places, the rest each placed before the row
//! that follows it.

use serde::Deserialize;
use serde_json::{Value as Json, json, value::RawValue};
use std::{
    cell::RefCell,
    collections::HashMap,
    net::TcpStream,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
use zgx_module::{Host, Promise, Value};

/// What the connection's thread is asked from the interface's.
enum Command {
    Rename {
        id: String,
        title: String,
        promise: Promise<Value>,
    },
}

/// What it cost, counted on both threads: nanoseconds, and how many.
#[derive(Default)]
struct Costs {
    frames: AtomicU64,
    /// From a post to its job beginning, each.
    waits: Mutex<Vec<u64>>,
    /// The interface's thread: making the list's values and setting them.
    on_ui: AtomicU64,
    /// The connection's thread: reading a frame, parsing it and diffing it.
    off_ui: AtomicU64,
    /// Rows made on the interface's thread: in whole lists, and patched.
    whole: AtomicU64,
    rows: AtomicU64,
    /// Lists the engine sent, each (those that changed nothing aren't
    /// posted, so aren't `frames`).
    lists: AtomicU64,
}

/// What changed in the list since the last, found on the connection's
/// thread: what the interface's thread writes.
enum Patch {
    /// The whole list: the first.
    Whole(Vec<Json>),
    /// Rows by `id`: these written where they go (new, changed or moved),
    /// these removed.
    Rows { rows: Vec<(Json, Placed)>, gone: Vec<String> },
}

/// Where a posted row goes: `zgx_module::Place`, owned to cross threads.
enum Placed {
    Stay,
    Before(String),
    End,
}

/// A frame from the engine, its parts borrowed from the text.
#[derive(Deserialize)]
struct Frame<'a> {
    id: Option<u64>,
    #[serde(borrow)]
    item: Option<&'a RawValue>,
    err: Option<Json>,
}
/// A row's id, borrowed where it can be.
#[derive(Deserialize)]
struct Keyed<'a> {
    #[serde(borrow)]
    id: std::borrow::Cow<'a, str>,
}

/// The places in `seq` of one of its longest increasing runs (patience
/// sorting: n log n).
fn longest_increasing(seq: &[usize]) -> Vec<usize> {
    // `tails[k]`: where in `seq` the smallest last of a run k+1 long is.
    let mut tails: Vec<usize> = Vec::new();
    let mut before: Vec<Option<usize>> = vec![None; seq.len()];
    for (i, &x) in seq.iter().enumerate() {
        let k = tails.partition_point(|&t| seq[t] < x);
        before[i] = k.checked_sub(1).map(|k| tails[k]);
        if k == tails.len() {
            tails.push(i);
        } else {
            tails[k] = i;
        }
    }
    let mut run = Vec::with_capacity(tails.len());
    let mut at = tails.last().copied();
    while let Some(i) = at {
        run.push(i);
        at = before[i];
    }
    run.reverse();
    run
}

/// The last list, as the connection's thread diffs against it: its ids in
/// order, and a hash of each row's bytes by id (no row is kept).
#[derive(Default)]
struct Last {
    order: Vec<String>,
    hashes: HashMap<String, u64>,
}
impl Last {
    /// What changed in `list` (its rows as the frame has them) since this,
    /// which becomes `list`. Only the rows posted are parsed.
    fn diff(&mut self, list: &[&RawValue], first: bool) -> Result<Patch, serde_json::Error> {
        use std::hash::{Hash, Hasher};
        let mut order = Vec::with_capacity(list.len());
        let mut hashes = HashMap::with_capacity(list.len());
        for row in list {
            let id = serde_json::from_str::<Keyed>(row.get())?.id.into_owned();
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            row.get().hash(&mut hasher);
            hashes.insert(id.clone(), hasher.finish());
            order.push(id);
        }
        let patch = if first {
            Patch::Whole(list.iter().map(|row| serde_json::from_str(row.get())).collect::<Result<_, _>>()?)
        } else {
            let gone: Vec<String> = self.order.iter().filter(|id| !hashes.contains_key(*id)).cloned().collect();
            // The rows that keep their order: a longest increasing run of
            // their old places. The rest of the old ones move; new ones are
            // put. Each is placed before the row after it (or at the end).
            let was: HashMap<&str, usize> = self.order.iter().enumerate().map(|(i, id)| (id.as_str(), i)).collect();
            let old: Vec<(usize, usize)> = order.iter().enumerate().filter_map(|(i, id)| was.get(id.as_str()).map(|&w| (i, w))).collect();
            let places: Vec<usize> = old.iter().map(|&(_, w)| w).collect();
            let mut stays = vec![false; order.len()];
            for k in longest_increasing(&places) {
                stays[old[k].0] = true;
            }
            let mut rows = Vec::new();
            for (i, id) in order.iter().enumerate() {
                let changed = self.hashes.get(id) != hashes.get(id);
                if stays[i] && !changed {
                    continue;
                }
                let placed = if stays[i] {
                    Placed::Stay
                } else {
                    order.get(i + 1).map_or(Placed::End, |next| Placed::Before(next.clone()))
                };
                rows.push((serde_json::from_str(list[i].get())?, placed));
            }
            Patch::Rows { rows, gone }
        };
        self.order = order;
        self.hashes = hashes;
        Ok(patch)
    }
}

/// A value of the engine's JSON: made on the interface's thread.
fn value_of(json: &Json) -> Value {
    match json {
        Json::Null => Value::NIL,
        Json::Bool(b) => Value::from(*b),
        Json::Number(n) => Value::from(n.as_f64().unwrap_or(0.)),
        Json::String(s) => Value::from(s.as_str()),
        Json::Array(items) => Value::list(items.iter().map(value_of)),
        Json::Object(fields) => Value::record(fields.iter().map(|(k, v)| (k.as_str(), value_of(v)))),
    }
}

type Socket = tungstenite::WebSocket<TcpStream>;

/// The engine's socket, as the app dials it (src/engine/rpc.rs).
fn dial(port: u16, data_dir: &std::path::Path) -> Result<Socket, String> {
    use tungstenite::client::IntoClientRequest;
    let stream = TcpStream::connect(("127.0.0.1", port)).map_err(|e| e.to_string())?;
    let _ = stream.set_nodelay(true);
    let mut request = format!("ws://127.0.0.1:{port}")
        .into_client_request()
        .map_err(|e| e.to_string())?;
    let token = std::fs::read_to_string(data_dir.join("ipc-token")).ok();
    if let Some(token) = token.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        let header = tungstenite::http::HeaderValue::from_str(&format!("Bearer {token}")).map_err(|e| e.to_string())?;
        request.headers_mut().insert(tungstenite::http::header::AUTHORIZATION, header);
    }
    let config = tungstenite::protocol::WebSocketConfig::default()
        .max_message_size(Some(256 << 20))
        .max_frame_size(Some(256 << 20));
    let (socket, _) = tungstenite::client::client_with_config(request, stream, Some(config)).map_err(|e| e.to_string())?;
    Ok(socket)
}

/// The connection's thread: the chat list watched, each list posted to the
/// interface's thread; renames sent, each answer settling its promise.
fn connected(ui: zgx_module::Ui, port: u16, data_dir: PathBuf, commands: mpsc::Receiver<Command>, costs: Arc<Costs>) {
    let mut socket = match dial(port, &data_dir) {
        Ok(socket) => socket,
        Err(why) => {
            ui.post(move |host| {
                if let Some(error) = host.shared("sessions.error", Value::NIL) {
                    let _ = error.set(Value::from(why.as_str()));
                }
            });
            return;
        }
    };
    let watch = json!({ "id": 1, "method": "WatchChats", "params": {} });
    if socket.send(tungstenite::Message::text(watch.to_string())).is_err() {
        return;
    }
    // (Read in short waits, so renames asked meanwhile go out.)
    let _ = socket.get_mut().set_read_timeout(Some(Duration::from_millis(20)));
    let mut next = 2u64;
    let mut pending: HashMap<u64, Promise<Value>> = HashMap::new();
    let mut last = Last::default();
    let mut first = true;
    loop {
        while let Ok(command) = commands.try_recv() {
            match command {
                Command::Rename { id, title, promise } => {
                    let call = json!({
                        "id": next,
                        "method": "Mutate",
                        "params": { "op": "renameChat", "chatId": id, "title": title },
                    });
                    if socket.send(tungstenite::Message::text(call.to_string())).is_ok() {
                        pending.insert(next, promise);
                    } else {
                        promise.resolve_with(|_| Err("the engine's connection closed".to_string().into()));
                    }
                    next += 1;
                }
            }
        }
        let message = match socket.read() {
            Ok(message) => message,
            Err(tungstenite::Error::Io(e)) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => continue,
            Err(_) => return,
        };
        // (From the frame read: what waiting for one took isn't work.)
        let started = Instant::now();
        let tungstenite::Message::Text(text) = message else { continue };
        for line in text.lines() {
            let Ok(frame) = serde_json::from_str::<Frame>(line) else { continue };
            let id = frame.id;
            if id == Some(1) {
                let Some(item) = frame.item else { continue };
                let Ok(list) = serde_json::from_str::<Vec<&RawValue>>(item.get()) else { continue };
                costs.lists.fetch_add(1, Ordering::Relaxed);
                let Ok(patch) = last.diff(&list, std::mem::take(&mut first)) else { continue };
                if let Patch::Rows { rows, gone } = &patch
                    && rows.is_empty()
                    && gone.is_empty()
                {
                    // (Nothing of the list changed: nothing to post.)
                    costs.off_ui.fetch_add(started.elapsed().as_nanos() as u64, Ordering::Relaxed);
                    continue;
                }
                let posted = Instant::now();
                costs.off_ui.fetch_add(posted.duration_since(started).as_nanos() as u64, Ordering::Relaxed);
                let costs = costs.clone();
                ui.post(move |host| {
                    let begun = Instant::now();
                    costs.waits.lock().unwrap().push(begun.duration_since(posted).as_nanos() as u64);
                    if let Some(chats) = host.shared("sessions.chats", Value::NIL) {
                        let written = match &patch {
                            Patch::Whole(list) => chats.set(Value::list(list.iter().map(value_of))).map(drop),
                            Patch::Rows { rows, gone } => chats
                                .upsert_at(
                                    "id",
                                    rows.iter().map(|(row, placed)| {
                                        let place = match placed {
                                            Placed::Stay => zgx_module::Place::Stay,
                                            Placed::Before(key) => zgx_module::Place::Before(key),
                                            Placed::End => zgx_module::Place::End,
                                        };
                                        (value_of(row), place)
                                    }),
                                )
                                .and_then(|()| chats.remove("id", gone.iter().map(String::as_str)).map(drop)),
                        };
                        if let Err(why) = written
                            && let Some(error) = host.shared("sessions.error", Value::NIL)
                        {
                            let _ = error.set(Value::from(why.0.as_str()));
                        }
                        match patch {
                            Patch::Whole(list) => costs.whole.fetch_add(list.len() as u64, Ordering::Relaxed),
                            Patch::Rows { rows, .. } => costs.rows.fetch_add(rows.len() as u64, Ordering::Relaxed),
                        };
                    }
                    costs.frames.fetch_add(1, Ordering::Relaxed);
                    costs.on_ui.fetch_add(begun.elapsed().as_nanos() as u64, Ordering::Relaxed);
                });
            } else if let Some(promise) = id.and_then(|id| pending.remove(&id)) {
                let answer = frame.err.as_ref().and_then(Json::as_str).map(str::to_owned);
                promise.resolve_with(move |_| match answer {
                    None => Ok(Value::from(true)),
                    Some(why) => Err(why.into()),
                });
            }
        }
    }
}

zgx_module::register!("sessions", |host: &Host| {
    let ui = host.ui();
    let costs = Arc::new(Costs::default());
    let (to_thread, commands) = mpsc::channel::<Command>();
    let commands = RefCell::new(Some(commands));
    // `{ port, data_dir }`: the engine to watch, from the app. Once.
    let thread_costs = costs.clone();
    host.function("sessions.connect", move |arg| {
        let port = arg.field("port").and_then(|p| p.num()).unwrap_or(0.) as u16;
        let data_dir = PathBuf::from(arg.field("data_dir").and_then(|d| d.str()).unwrap_or(""));
        if let Some(commands) = commands.borrow_mut().take() {
            let (ui, costs) = (ui.clone(), thread_costs.clone());
            std::thread::Builder::new()
                .name("sessions-engine".into())
                .spawn(move || connected(ui, port, data_dir, commands, costs))
                .ok();
        }
        Value::NIL
    });
    // A chat renamed over the module's own connection; it comes back by the
    // watch. `{ id, title }`.
    host.async_action("sessions.rename_module", move |arg, promise| {
        let id = arg.field("id").and_then(|v| v.str()).unwrap_or("").to_owned();
        let title = arg.field("title").and_then(|v| v.str()).unwrap_or("").to_owned();
        if let Err(mpsc::SendError(Command::Rename { promise, .. })) = to_thread.send(Command::Rename { id, title, promise }) {
            promise.resolve_with(|_| Err("not connected (sessions.connect)".to_string().into()));
        }
    });
    // What it cost so far.
    host.function("sessions.stats", move |_| {
        let mut waits = costs.waits.lock().unwrap().clone();
        waits.sort_unstable();
        let ms = |ns: u64| ns as f64 / 1e6;
        let at = |q: f64| waits.get(((waits.len() as f64 - 1.) * q).round() as usize).copied().map_or(0., ms);
        let (on, off) = (costs.on_ui.load(Ordering::Relaxed), costs.off_ui.load(Ordering::Relaxed));
        Value::record([
            ("frames", Value::from(costs.frames.load(Ordering::Relaxed) as f64)),
            ("wait_median_ms", Value::from(at(0.5))),
            ("wait_p90_ms", Value::from(at(0.9))),
            ("wait_max_ms", Value::from(at(1.))),
            ("ui_ms", Value::from(ms(on))),
            ("thread_ms", Value::from(ms(off))),
            ("off_ui_share", Value::from(if on + off == 0 { 0. } else { off as f64 / (on + off) as f64 })),
            ("lists", Value::from(costs.lists.load(Ordering::Relaxed) as f64)),
            ("rows_whole", Value::from(costs.whole.load(Ordering::Relaxed) as f64)),
            ("rows_patched", Value::from(costs.rows.load(Ordering::Relaxed) as f64)),
        ])
    });
});
