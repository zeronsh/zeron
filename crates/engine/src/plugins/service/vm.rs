//! One plugin service's Luau VM, on a thread of its own.
//!
//! The sandbox is the window's (zgx-script's plugin VMs): only the safe
//! standard libraries, globals read-only (`luaL_sandbox`), a memory cap,
//! and a time budget for each entry into the VM, so a runaway loop fails
//! its own call instead of hanging the service. `require` is replaced by
//! one that reads only the plugin's own `.luau` files.
//!
//! The VM owns nothing but Luau: what reaches the world (programs, the
//! network, notifications, events to windows) goes through [`Ops`], which
//! the host implements with the plugin's permissions. Ops finish on any
//! thread and post their outcome back here, where the waiting task resumes.

use std::cell::{Cell, RefCell};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap, HashMap};
use std::panic::AssertUnwindSafe;
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use mlua::chunk::ChunkMode;
use mlua::{Function, Lua, LuaOptions, StdLib, Table, Value, VmState};
use serde_json::Value as Json;
use tokio::sync::oneshot;

const PRELUDE: &str = include_str!("prelude.luau");
/// A source file, at most.
const MAX_SCRIPT_BYTES: usize = 1 << 20;
/// What `zeron.store` keeps, at most (its JSON).
pub const MAX_STORE_BYTES: usize = 1 << 20;
/// Tables nested deeper than this don't cross to or from JSON.
const MAX_DEPTH: usize = 64;
/// What Luau says when an allocation fails (the memory cap).
const OUT_OF_MEMORY: &str = "not enough memory";

#[derive(Debug, Clone)]
pub struct Limits {
    /// The VM's heap, at most.
    pub memory: usize,
    /// Each entry into the VM (loading, a call starting, a task resuming).
    pub slice: Duration,
    /// How long a stopping VM waits for its calls in flight.
    pub drain: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            memory: 64 << 20,
            slice: Duration::from_secs(2),
            drain: Duration::from_secs(60),
        }
    }
}

/// What to run.
pub struct Spec {
    pub plugin: String,
    /// The plugin's files (its snapshot).
    pub dir: PathBuf,
    /// The service's file, relative to `dir` (`service.luau`).
    pub entry: String,
    /// `{ id, name }` of this device, as `zeron.device`.
    pub device: Json,
    /// `zeron.store`'s file (`None`: kept in memory only).
    pub store: Option<PathBuf>,
    pub limits: Limits,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Method,
    Tool,
    Timer,
    /// An event a `zeron.on` listener hears (plugins/agents).
    Event,
}

impl Kind {
    fn as_str(self) -> &'static str {
        match self {
            Kind::Method => "method",
            Kind::Tool => "tool",
            Kind::Timer => "timer",
            Kind::Event => "event",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Failure {
    /// The plugin's own: error() in its handler (or a refusal it didn't catch).
    Error(String),
    /// It serves nothing by that name.
    Unknown,
}

/// A call's outcome. A receiver that errors instead means the VM ended
/// before it answered (it stopped, ran out of memory, or crashed).
pub type Reply = Result<Json, Failure>;

/// An op's outcome, posted back to the VM.
pub type Done = Box<dyn FnOnce(Result<Json, String>) + Send>;

/// The world, as the host lets this plugin reach it.
pub trait Ops: Send + Sync + 'static {
    /// Start `kind` (`run`, `fetch`, `notify`) with `args`; `done` with its
    /// outcome, from any thread. A refusal is `done(Err(why))`.
    fn start(&self, kind: &str, args: Json, done: Done);
    /// `zeron.emit`: to the plugin's windows.
    fn emit(&self, event: &str, data: Json);
    /// `print`, and what failed with nobody to tell.
    fn log(&self, line: &str);
    /// `zeron.watch`: changes under `folders` (each all the way down) told
    /// to `heard` as they happen, from any thread, until the handle drops.
    /// A refusal is `Err(why)`.
    fn watch(&self, folders: Vec<PathBuf>, heard: Heard) -> Result<Box<dyn Send>, String> {
        let _ = (folders, heard);
        Err("zeron.watch isn't something this service can do".into())
    }
}

/// What a watch heard: in which of its folders (by place), and where in it.
#[derive(Debug, Clone, PartialEq)]
pub enum Seen {
    /// This path (relative to the folder, `/`-separated) changed.
    Path(usize, String),
    /// Anything in this folder may have changed (events were lost, or the
    /// watch just went live).
    All(usize),
}

/// Where a watch tells what it heard.
pub type Heard = Box<dyn Fn(Seen) + Send + Sync>;

/// A watch's changes, a burst at a time: told `quiet` after its last.
struct Watching {
    _handle: Box<dyn Send>,
    quiet: Duration,
    /// Each folder's changed paths since last told (`None`: anything).
    pending: BTreeMap<usize, Option<std::collections::BTreeSet<String>>>,
    due: Option<Instant>,
}

/// A burst's paths in one folder, at most: past them, it's "anything".
const MAX_WATCH_PATHS: usize = 256;

/// What a service serves, as it said while loading.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Described {
    /// Each method, and whether it's safe to run again.
    pub methods: BTreeMap<String, bool>,
    pub tools: Vec<String>,
    /// Each timer's key (`#1`, `#2`: by its place) and period in seconds.
    pub timers: Vec<(String, f64)>,
    /// Each `zeron.on` listener: its key (`@1`, `@2`: by its place), the
    /// event it hears and its options (a threshold's `below`, …).
    pub listeners: Vec<(String, String, Json)>,
}

/// How a VM stands, read by the host without going through its thread.
#[derive(Debug)]
pub struct Status {
    in_flight: AtomicUsize,
    pending: AtomicUsize,
    last: Mutex<Instant>,
    ended: AtomicBool,
}

impl Status {
    fn new() -> Self {
        Self {
            in_flight: AtomicUsize::new(0),
            pending: AtomicUsize::new(0),
            last: Mutex::new(Instant::now()),
            ended: AtomicBool::new(false),
        }
    }

    fn touch(&self) {
        *self.last.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
    }

    /// Calls it hasn't answered, and ops it's waiting on.
    pub fn busy(&self) -> bool {
        self.in_flight.load(Ordering::SeqCst) > 0 || self.pending.load(Ordering::SeqCst) > 0
    }

    /// Since it last did anything.
    pub fn idle_for(&self) -> Duration {
        self.last.lock().unwrap_or_else(|e| e.into_inner()).elapsed()
    }

    pub fn ended(&self) -> bool {
        self.ended.load(Ordering::SeqCst)
    }
}

enum ToVm {
    Call {
        kind: Kind,
        name: String,
        args: Json,
        call: Json,
        reply: oneshot::Sender<Reply>,
    },
    Done {
        op: u64,
        result: Result<Json, String>,
    },
    Heard {
        watch: u64,
        seen: Seen,
    },
    Stop,
}

/// A running VM. Cheap to clone; the thread ends after [`Vm::stop`] (or
/// when every handle is gone).
#[derive(Clone)]
pub struct Vm {
    tx: mpsc::Sender<ToVm>,
    status: Arc<Status>,
}

impl Vm {
    /// Start a VM on a thread of its own. The receiver says what it serves
    /// once its top level ran, or why it couldn't load.
    pub fn start(spec: Spec, ops: Arc<dyn Ops>) -> (Vm, oneshot::Receiver<Result<Described, String>>) {
        let (tx, rx) = mpsc::channel();
        let (loaded_tx, loaded_rx) = oneshot::channel();
        let status = Arc::new(Status::new());
        let vm = Vm {
            tx: tx.clone(),
            status: status.clone(),
        };
        let name = format!("plugin-service:{}", spec.plugin);
        let spawned = std::thread::Builder::new().name(name).spawn(move || {
            let plugin = spec.plugin.clone();
            let thread_status = status.clone();
            let outcome = std::panic::catch_unwind(AssertUnwindSafe(move || {
                run(spec, ops, rx, tx, thread_status, loaded_tx)
            }));
            if outcome.is_err() {
                tracing::warn!(%plugin, "plugins: a service's VM crashed");
            }
            status.ended.store(true, Ordering::SeqCst);
        });
        if spawned.is_err() {
            vm.status.ended.store(true, Ordering::SeqCst);
        }
        (vm, loaded_rx)
    }

    /// Ask it something. The receiver errors when it ended before answering.
    pub fn call(&self, kind: Kind, name: &str, args: Json, call: Json) -> oneshot::Receiver<Reply> {
        let (reply, rx) = oneshot::channel();
        self.status.in_flight.fetch_add(1, Ordering::SeqCst);
        self.status.touch();
        let sent = self.tx.send(ToVm::Call {
            kind,
            name: name.to_owned(),
            args,
            call,
            reply,
        });
        if sent.is_err() {
            self.status.in_flight.fetch_sub(1, Ordering::SeqCst);
        }
        rx
    }

    /// Stop once the calls in flight are answered (or the drain time is up),
    /// after its `on_stop`s.
    pub fn stop(&self) {
        let _ = self.tx.send(ToVm::Stop);
    }

    pub fn status(&self) -> &Status {
        &self.status
    }

    /// The same VM (not just the same plugin's).
    pub fn same(&self, other: &Vm) -> bool {
        Arc::ptr_eq(&self.status, &other.status)
    }
}

/// The VM's own bookkeeping, shared by its natives.
struct State {
    replies: RefCell<HashMap<u64, oneshot::Sender<Reply>>>,
    next_reply: Cell<u64>,
    next_op: Cell<u64>,
    sleepers: RefCell<BinaryHeap<Reverse<(Instant, u64)>>>,
    store_path: Option<PathBuf>,
    store: RefCell<Option<serde_json::Map<String, Json>>>,
    /// Out of memory: it ends, and what it hadn't answered is dropped.
    dead: Cell<bool>,
    status: Arc<Status>,
    /// `zeron.watch`es, by id.
    watches: RefCell<HashMap<u64, Watching>>,
    next_watch: Cell<u64>,
}

impl State {
    /// A change a watch heard, kept for its burst's end.
    fn heard(&self, watch: u64, seen: Seen) {
        let mut watches = self.watches.borrow_mut();
        let Some(w) = watches.get_mut(&watch) else { return };
        let (place, path) = match seen {
            Seen::Path(place, path) => (place, Some(path)),
            Seen::All(place) => (place, None),
        };
        let entry = w.pending.entry(place).or_insert_with(|| Some(Default::default()));
        match (entry.as_mut(), path) {
            (Some(paths), Some(path)) if paths.len() < MAX_WATCH_PATHS || paths.contains(&path) => {
                paths.insert(path);
            }
            (Some(_), _) => *entry = None,
            (None, _) => {}
        }
        w.due = Some(Instant::now() + w.quiet);
    }

    /// The soonest a watch's burst ends.
    fn watch_due(&self) -> Option<Instant> {
        self.watches.borrow().values().filter_map(|w| w.due).min()
    }

    /// The watches whose bursts ended, each with what it heard.
    fn watches_due(&self) -> Vec<(u64, BTreeMap<usize, Option<std::collections::BTreeSet<String>>>)> {
        let now = Instant::now();
        let mut out = Vec::new();
        for (id, w) in self.watches.borrow_mut().iter_mut() {
            if w.due.is_some_and(|at| at <= now) {
                w.due = None;
                out.push((*id, std::mem::take(&mut w.pending)));
            }
        }
        out
    }

    fn answer(&self, id: u64, reply: Reply) {
        if let Some(tx) = self.replies.borrow_mut().remove(&id) {
            self.status.in_flight.fetch_sub(1, Ordering::SeqCst);
            let _ = tx.send(reply);
        }
    }

    fn store(&self) -> std::cell::RefMut<'_, Option<serde_json::Map<String, Json>>> {
        let mut store = self.store.borrow_mut();
        if store.is_none() {
            let read = self
                .store_path
                .as_ref()
                .and_then(|path| std::fs::read(path).ok())
                .and_then(|bytes| serde_json::from_slice::<Json>(&bytes).ok());
            *store = Some(match read {
                Some(Json::Object(map)) => map,
                _ => serde_json::Map::new(),
            });
        }
        store
    }
}

fn run(
    spec: Spec,
    ops: Arc<dyn Ops>,
    rx: mpsc::Receiver<ToVm>,
    tx: mpsc::Sender<ToVm>,
    status: Arc<Status>,
    loaded: oneshot::Sender<Result<Described, String>>,
) {
    let limits = spec.limits.clone();
    let state = Rc::new(State {
        replies: RefCell::default(),
        next_reply: Cell::new(1),
        next_op: Cell::new(1),
        sleepers: RefCell::default(),
        store_path: spec.store.clone(),
        store: RefCell::default(),
        dead: Cell::new(false),
        status: status.clone(),
        watches: RefCell::default(),
        next_watch: Cell::new(1),
    });
    let deadline: Rc<Cell<Option<Instant>>> = Rc::default();
    let (lua, entry) = match setup(&spec, &ops, &tx, &state, &deadline) {
        Ok(vm) => vm,
        Err(error) => {
            let _ = loaded.send(Err(error));
            return;
        }
    };
    // Each entry into the VM gets the slice's budget.
    let enter = |f: &dyn Fn() -> mlua::Result<()>| -> mlua::Result<()> {
        deadline.set(Some(Instant::now() + limits.slice));
        let result = f();
        deadline.set(None);
        status.touch();
        if let Err(mlua::Error::MemoryError(_)) = &result {
            state.dead.set(true);
        }
        result
    };

    let source = match read_source(&spec.dir, &spec.entry) {
        Ok(source) => source,
        Err(error) => {
            let _ = loaded.send(Err(error));
            return;
        }
    };
    let chunk_name = format!("@{}", spec.entry);
    if let Err(error) = enter(&|| lua.load(source.as_str()).set_name(chunk_name.as_str()).set_mode(ChunkMode::Text).exec()) {
        let _ = loaded.send(Err(lua_message(&error)));
        return;
    }
    let described = match entry.describe.call::<Table>(()).map_err(|e| lua_message(&e)).and_then(|t| describe(&t)) {
        Ok(described) => described,
        Err(error) => {
            let _ = loaded.send(Err(error));
            return;
        }
    };
    if loaded.send(Ok(described)).is_err() {
        return;
    }

    let mut stopping: Option<Instant> = None;
    loop {
        if state.dead.get() {
            break;
        }
        if let Some(until) = stopping
            && (state.replies.borrow().is_empty() || Instant::now() >= until)
        {
            let _ = enter(&|| entry.stopping.call::<()>(()));
            break;
        }
        let mut wake = state.sleepers.borrow().peek().map(|Reverse((at, _))| *at);
        if let Some(at) = state.watch_due() {
            wake = Some(wake.map_or(at, |w| w.min(at)));
        }
        if let Some(until) = stopping {
            wake = Some(wake.map_or(until, |at| at.min(until)));
        }
        let message = match wake {
            Some(at) => match rx.recv_timeout(at.saturating_duration_since(Instant::now())) {
                Ok(message) => Some(message),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            },
            None => match rx.recv() {
                Ok(message) => Some(message),
                Err(_) => break,
            },
        };
        match message {
            Some(ToVm::Call {
                kind,
                name,
                args,
                call,
                reply,
            }) => {
                let id = state.next_reply.get();
                state.next_reply.set(id + 1);
                state.replies.borrow_mut().insert(id, reply);
                let result = enter(&|| {
                    let args = to_lua(&lua, &args, 0)?;
                    let call = to_lua(&lua, &call, 0)?;
                    entry.dispatch.call::<()>((kind.as_str(), name.as_str(), args, call, id))
                });
                // (Out of memory: left unanswered, as the VM ends.)
                if let Err(error) = result
                    && !state.dead.get()
                {
                    state.answer(id, Err(Failure::Error(lua_message(&error))));
                }
            }
            Some(ToVm::Done { op, result }) => {
                status.pending.fetch_sub(1, Ordering::SeqCst);
                let outcome = enter(&|| {
                    let (ok, value) = match &result {
                        Ok(value) => (true, to_lua(&lua, value, 0)?),
                        Err(why) => (false, Value::String(lua.create_string(why)?)),
                    };
                    entry.done.call::<()>((op, ok, value))
                });
                if let Err(error) = outcome {
                    ops.log(&format!("a task failed: {}", lua_message(&error)));
                }
            }
            Some(ToVm::Heard { watch, seen }) => state.heard(watch, seen),
            Some(ToVm::Stop) => {
                stopping.get_or_insert(Instant::now() + limits.drain);
                // (Nothing more is heard: it's going.)
                state.watches.borrow_mut().clear();
            }
            None => {}
        }
        // Watches whose bursts ended: what each heard, by folder (its
        // paths, or `true`: anything).
        for (watch, pending) in state.watches_due() {
            let outcome = enter(&|| {
                let changed = lua.create_table()?;
                for (place, paths) in &pending {
                    let value = match paths {
                        Some(paths) => Value::Table(lua.create_sequence_from(paths.iter().map(String::as_str))?),
                        None => Value::Boolean(true),
                    };
                    changed.raw_set(place + 1, value)?;
                }
                entry.heard.call::<()>((watch, changed))
            });
            if let Err(error) = outcome {
                ops.log(&format!("a watch failed: {}", lua_message(&error)));
            }
        }
        // Sleeps that are due.
        loop {
            let due = {
                let mut sleepers = state.sleepers.borrow_mut();
                match sleepers.peek() {
                    Some(Reverse((at, _))) if *at <= Instant::now() => sleepers.pop().map(|Reverse((_, op))| op),
                    _ => None,
                }
            };
            let Some(op) = due else { break };
            status.pending.fetch_sub(1, Ordering::SeqCst);
            if let Err(error) = enter(&|| entry.done.call::<()>((op, true, Value::Nil))) {
                ops.log(&format!("a task failed: {}", lua_message(&error)));
            }
        }
    }
    // What's left unanswered is dropped: its callers hear the VM ended.
    let left = state.replies.borrow_mut().drain().count();
    status.in_flight.fetch_sub(left.min(status.in_flight.load(Ordering::SeqCst)), Ordering::SeqCst);
    status.pending.store(0, Ordering::SeqCst);
}

/// The prelude's entry points.
struct Entry {
    dispatch: Function,
    describe: Function,
    done: Function,
    stopping: Function,
    heard: Function,
}

fn setup(
    spec: &Spec,
    ops: &Arc<dyn Ops>,
    tx: &mpsc::Sender<ToVm>,
    state: &Rc<State>,
    deadline: &Rc<Cell<Option<Instant>>>,
) -> Result<(Lua, Entry), String> {
    let fail = |e: mlua::Error| lua_message(&e);
    let lua = Lua::new_with(
        StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::UTF8 | StdLib::BIT | StdLib::COROUTINE | StdLib::OS,
        LuaOptions::default(),
    )
    .map_err(fail)?;
    lua.set_memory_limit(spec.limits.memory).map_err(fail)?;
    let ticks = Cell::new(0u32);
    let watched = deadline.clone();
    lua.set_interrupt(move |_| {
        // The clock every 256th safepoint: a runaway loop stops within a
        // few milliseconds of its budget.
        let tick = ticks.get().wrapping_add(1);
        ticks.set(tick);
        if tick.is_multiple_of(256)
            && let Some(deadline) = watched.get()
            && Instant::now() > deadline
        {
            return Err(mlua::Error::RuntimeError(
                "the service ran past its time budget (an endless loop, or too much work at once: await between parts)"
                    .into(),
            ));
        }
        Ok(VmState::Continue)
    });

    let globals = lua.globals();
    // Nothing that reads files or compiles code from text.
    for name in ["loadstring", "require"] {
        globals.raw_set(name, Value::Nil).map_err(fail)?;
    }
    let log_ops = ops.clone();
    globals
        .raw_set(
            "print",
            lua.create_function(move |_, args: mlua::Variadic<Value>| {
                let line: Vec<String> = args.iter().map(display).collect();
                log_ops.log(&line.join("\t"));
                Ok(())
            })
            .map_err(fail)?,
        )
        .map_err(fail)?;
    install_require(&lua, &spec.dir).map_err(fail)?;

    let host = natives(&lua, spec, ops, tx, state).map_err(fail)?;
    let (service, dispatch, describe, done, stopping, heard): (Function, Function, Function, Function, Function, Function) =
        lua.load(PRELUDE).set_name("=zeron").set_mode(ChunkMode::Text).call(host).map_err(fail)?;
    globals.raw_set("zeron_service", service).map_err(fail)?;
    lua.sandbox(true).map_err(fail)?;
    Ok((
        lua,
        Entry {
            dispatch,
            describe,
            done,
            stopping,
            heard,
        },
    ))
}

fn natives(
    lua: &Lua,
    spec: &Spec,
    ops: &Arc<dyn Ops>,
    tx: &mpsc::Sender<ToVm>,
    state: &Rc<State>,
) -> mlua::Result<Table> {
    let host = lua.create_table()?;
    host.set("plugin", spec.plugin.as_str())?;
    host.set("device", to_lua(lua, &spec.device, 0)?)?;

    let json = lua.create_table()?;
    json.set(
        "encode",
        lua.create_function(|_, value: Value| {
            let json = to_json(&value, 0).map_err(mlua::Error::runtime)?;
            Ok(json.to_string())
        })?,
    )?;
    json.set(
        "decode",
        lua.create_function(|lua, text: mlua::LuaString| {
            let json: Json = serde_json::from_slice(&text.as_bytes())
                .map_err(|e| mlua::Error::runtime(format!("that isn't JSON: {e}")))?;
            to_lua(lua, &json, 0)
        })?,
    )?;
    json.set_readonly(true);
    host.set("json", json)?;

    let log_ops = ops.clone();
    host.set(
        "log",
        lua.create_function(move |_, line: String| {
            log_ops.log(&line);
            Ok(())
        })?,
    )?;

    let (op_state, op_ops, op_tx) = (state.clone(), ops.clone(), tx.clone());
    host.set(
        "op",
        lua.create_function(move |_, (kind, args): (String, Value)| {
            let id = op_state.next_op.get();
            op_state.next_op.set(id + 1);
            op_state.status.pending.fetch_add(1, Ordering::SeqCst);
            if kind == "sleep" {
                let seconds = match &args {
                    Value::Table(t) => t.get::<Option<f64>>("seconds")?.unwrap_or(0.0),
                    _ => 0.0,
                };
                let at = Instant::now() + Duration::from_secs_f64(seconds.clamp(0.0, 24.0 * 3600.0));
                op_state.sleepers.borrow_mut().push(Reverse((at, id)));
                return Ok(id);
            }
            let args = to_json(&args, 0).map_err(mlua::Error::runtime)?;
            let tx = op_tx.clone();
            op_ops.start(
                &kind,
                args,
                Box::new(move |result| {
                    let _ = tx.send(ToVm::Done { op: id, result });
                }),
            );
            Ok(id)
        })?,
    )?;

    let reply_state = state.clone();
    host.set(
        "reply",
        lua.create_function(move |_, (id, ok, value): (u64, bool, Value)| {
            let reply = if ok {
                to_json(&value, 0).map_err(|why| Failure::Error(format!("its answer can't be sent: {why}")))
            } else {
                let why = display(&value);
                if why.contains(OUT_OF_MEMORY) {
                    // Left unanswered: the VM ends, and its callers hear so.
                    reply_state.dead.set(true);
                    return Ok(());
                }
                Err(Failure::Error(why))
            };
            reply_state.answer(id, reply);
            Ok(())
        })?,
    )?;
    let unknown_state = state.clone();
    host.set(
        "unknown",
        lua.create_function(move |_, id: u64| {
            unknown_state.answer(id, Err(Failure::Unknown));
            Ok(())
        })?,
    )?;

    let emit_ops = ops.clone();
    host.set(
        "emit",
        lua.create_function(move |_, (event, data): (String, Value)| {
            let data = to_json(&data, 0).map_err(mlua::Error::runtime)?;
            emit_ops.emit(&event, data);
            Ok(())
        })?,
    )?;

    // `zeron.watch`: the folders, and how long a burst is quiet before it's
    // told (ms); its id, or an error.
    let (watch_state, watch_ops, watch_tx) = (state.clone(), ops.clone(), tx.clone());
    host.set(
        "watch",
        lua.create_function(move |_, (folders, quiet): (Vec<String>, Option<f64>)| {
            if folders.is_empty() || folders.iter().any(|f| f.trim().is_empty()) {
                return Err(mlua::Error::runtime("zeron.watch: the folders to watch"));
            }
            let id = watch_state.next_watch.get();
            watch_state.next_watch.set(id + 1);
            let tx = watch_tx.clone();
            let heard: Heard = Box::new(move |seen| {
                let _ = tx.send(ToVm::Heard { watch: id, seen });
            });
            let handle = watch_ops
                .watch(folders.into_iter().map(PathBuf::from).collect(), heard)
                .map_err(mlua::Error::runtime)?;
            let quiet = Duration::from_secs_f64(quiet.unwrap_or(250.0).clamp(0.0, 60_000.0) / 1000.0);
            watch_state.watches.borrow_mut().insert(
                id,
                Watching {
                    _handle: handle,
                    quiet,
                    pending: BTreeMap::new(),
                    due: None,
                },
            );
            Ok(id)
        })?,
    )?;
    let unwatch_state = state.clone();
    host.set(
        "unwatch",
        lua.create_function(move |_, id: u64| {
            unwatch_state.watches.borrow_mut().remove(&id);
            Ok(())
        })?,
    )?;

    let get_state = state.clone();
    host.set(
        "store_get",
        lua.create_function(move |lua, key: String| {
            let store = get_state.store();
            match store.as_ref().and_then(|m| m.get(&key)) {
                Some(value) => to_lua(lua, value, 0),
                None => Ok(Value::Nil),
            }
        })?,
    )?;
    let set_state = state.clone();
    host.set(
        "store_set",
        lua.create_function(move |_, (key, value): (String, Value)| {
            let value = to_json(&value, 0).map_err(mlua::Error::runtime)?;
            let mut store = set_state.store();
            let map = store.get_or_insert_with(serde_json::Map::new);
            let was = if value.is_null() { map.remove(&key) } else { map.insert(key.clone(), value) };
            let bytes = serde_json::to_vec(&*map).unwrap_or_default();
            if bytes.len() > MAX_STORE_BYTES {
                match was {
                    Some(was) => map.insert(key, was),
                    None => map.remove(&key),
                };
                return Err(mlua::Error::runtime("zeron.store: that's more than 1 MB to keep"));
            }
            if let Some(path) = &set_state.store_path {
                write_replace(path, &bytes).map_err(|e| mlua::Error::runtime(format!("zeron.store: couldn't save: {e}")))?;
            }
            Ok(())
        })?,
    )?;
    Ok(host)
}

/// `require("./x")`: a `.luau` file of the plugin's own, relative to the file
/// asking, read once. Nothing outside its folder, nothing by alias.
fn install_require(lua: &Lua, root: &Path) -> mlua::Result<()> {
    let root = root.to_path_buf();
    let loaded: Rc<RefCell<HashMap<String, Value>>> = Rc::default();
    let require = lua.create_function(move |lua, path: String| {
        let from = lua
            .inspect_stack(1, |debug| debug.source().source.map(|s| s.into_owned()))
            .flatten()
            .and_then(|s| s.strip_prefix('@').map(str::to_owned))
            .unwrap_or_default();
        let rel = resolve(&from, &path).ok_or_else(|| {
            mlua::Error::runtime(format!(
                "require(\"{path}\"): a service requires its plugin's own files, by a relative path (\"./x\")"
            ))
        })?;
        if let Some(value) = loaded.borrow().get(&rel) {
            return Ok(value.clone());
        }
        let source = read_source(&root, &rel).map_err(mlua::Error::runtime)?;
        // Text only: Luau doesn't validate bytecode, so a planted binary chunk
        // could corrupt the engine (R5).
        let value: Value = lua.load(source.as_str()).set_name(format!("@{rel}")).set_mode(ChunkMode::Text).call(())?;
        let value = if value.is_nil() { Value::Boolean(true) } else { value };
        loaded.borrow_mut().insert(rel, value.clone());
        Ok(value)
    })?;
    lua.globals().raw_set("require", require)
}

/// `path` from the file `from` (both relative to the plugin's folder), as a
/// `.luau` file's path inside it; `None` outside it or by alias.
fn resolve(from: &str, path: &str) -> Option<String> {
    if !(path.starts_with("./") || path.starts_with("../")) {
        return None;
    }
    let mut parts: Vec<String> = Path::new(from)
        .parent()
        .map(|p| {
            p.components()
                .filter_map(|c| match c {
                    Component::Normal(n) => Some(n.to_string_lossy().into_owned()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    for component in Path::new(path).components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                parts.pop()?;
            }
            Component::Normal(name) => {
                let name = name.to_string_lossy();
                if name.starts_with('.') {
                    return None;
                }
                parts.push(name.into_owned());
            }
            _ => return None,
        }
    }
    let mut rel = parts.join("/");
    if rel.is_empty() {
        return None;
    }
    if !rel.ends_with(".luau") {
        rel.push_str(".luau");
    }
    Some(rel)
}

fn read_source(root: &Path, rel: &str) -> Result<String, String> {
    let path = root.join(rel);
    // (A link inside the snapshot can't lead out of it.)
    let canonical = std::fs::canonicalize(&path).map_err(|_| format!("{rel} isn't in the plugin"))?;
    let root = std::fs::canonicalize(root).map_err(|_| format!("{rel} isn't in the plugin"))?;
    if !canonical.starts_with(&root) {
        return Err(format!("{rel} isn't in the plugin"));
    }
    let bytes = std::fs::read(&canonical).map_err(|_| format!("{rel} can't be read"))?;
    if bytes.len() > MAX_SCRIPT_BYTES {
        return Err(format!("{rel} is over 1 MB"));
    }
    String::from_utf8(bytes).map_err(|_| format!("{rel} isn't UTF-8 text"))
}

fn describe(table: &Table) -> Result<Described, String> {
    let fail = |e: mlua::Error| lua_message(&e);
    let mut described = Described::default();
    let methods: Table = table.get("methods").map_err(fail)?;
    for pair in methods.pairs::<String, bool>() {
        let (name, safe) = pair.map_err(fail)?;
        described.methods.insert(name, safe);
    }
    let tools: Table = table.get("tools").map_err(fail)?;
    for name in tools.sequence_values::<String>() {
        described.tools.push(name.map_err(fail)?);
    }
    described.tools.sort();
    let timers: Table = table.get("timers").map_err(fail)?;
    for timer in timers.sequence_values::<Table>() {
        let timer = timer.map_err(fail)?;
        described
            .timers
            .push((timer.get("key").map_err(fail)?, timer.get("seconds").map_err(fail)?));
    }
    if let Ok(listeners) = table.get::<Table>("listeners") {
        for listener in listeners.sequence_values::<Table>() {
            let listener = listener.map_err(fail)?;
            let opts: Value = listener.get("opts").map_err(fail)?;
            described.listeners.push((
                listener.get("key").map_err(fail)?,
                listener.get("event").map_err(fail)?,
                to_json(&opts, 0).unwrap_or(Json::Null),
            ));
        }
    }
    Ok(described)
}

/// An error as the plugin's author should read it.
pub fn lua_message(error: &mlua::Error) -> String {
    let message = match error {
        mlua::Error::CallbackError { cause, .. } => lua_message(cause),
        mlua::Error::MemoryError(_) => OUT_OF_MEMORY.to_owned(),
        mlua::Error::SyntaxError { message, .. } => message.clone(),
        mlua::Error::RuntimeError(message) => message.clone(),
        other => other.to_string(),
    };
    // (Where it was is in the message; the traceback is noise to a caller.)
    match message.split_once("\nstack traceback:") {
        Some((head, _)) => head.to_owned(),
        None => message,
    }
}

fn display(value: &Value) -> String {
    match value {
        Value::Nil => "nil".into(),
        Value::Boolean(b) => b.to_string(),
        Value::Integer(i) => i.to_string(),
        Value::Number(n) => {
            if n.fract() == 0.0 && n.abs() < 1e15 {
                format!("{}", *n as i64)
            } else {
                n.to_string()
            }
        }
        Value::String(s) => s.to_string_lossy(),
        Value::Table(_) => to_json(value, 0).map(|j| j.to_string()).unwrap_or_else(|_| "table".into()),
        Value::Error(e) => lua_message(e),
        other => other.type_name().to_owned(),
    }
}

/// JSON as Luau: objects and arrays as tables, `null` as nil.
pub fn to_lua(lua: &Lua, json: &Json, depth: usize) -> mlua::Result<Value> {
    if depth > MAX_DEPTH {
        return Err(mlua::Error::runtime("that's nested too deep"));
    }
    Ok(match json {
        Json::Null => Value::Nil,
        Json::Bool(b) => Value::Boolean(*b),
        Json::Number(n) => Value::Number(n.as_f64().unwrap_or(0.0)),
        Json::String(s) => Value::String(lua.create_string(s)?),
        Json::Array(items) => {
            let table = lua.create_table_with_capacity(items.len(), 0)?;
            for (i, item) in items.iter().enumerate() {
                table.raw_set(i + 1, to_lua(lua, item, depth + 1)?)?;
            }
            Value::Table(table)
        }
        Json::Object(map) => {
            let table = lua.create_table_with_capacity(0, map.len())?;
            for (key, item) in map {
                table.raw_set(key.as_str(), to_lua(lua, item, depth + 1)?)?;
            }
            Value::Table(table)
        }
    })
}

/// Luau as JSON: a table with keys 1..n is an array, any other an object
/// (an empty one `{}`); functions and the like don't cross.
pub fn to_json(value: &Value, depth: usize) -> Result<Json, String> {
    if depth > MAX_DEPTH {
        return Err("it's nested too deep (or refers to itself)".into());
    }
    Ok(match value {
        Value::Nil => Json::Null,
        Value::Boolean(b) => Json::Bool(*b),
        Value::Integer(i) => Json::from(*i),
        Value::Number(n) => number(*n),
        Value::String(s) => Json::String(s.to_string_lossy()),
        Value::Table(table) => {
            let len = table.raw_len();
            let mut pairs: Vec<(Value, Value)> = Vec::new();
            for pair in table.pairs::<Value, Value>() {
                pairs.push(pair.map_err(|e| lua_message(&e))?);
            }
            let index = |key: &Value| match key {
                Value::Integer(i) => usize::try_from(*i).ok(),
                Value::Number(n) if n.fract() == 0.0 && *n >= 1.0 => Some(*n as usize),
                _ => None,
            };
            let is_array = len > 0 && pairs.len() == len && pairs.iter().all(|(k, _)| index(k).is_some_and(|i| (1..=len).contains(&i)));
            if is_array {
                let mut items = vec![Json::Null; len];
                for (key, item) in &pairs {
                    if let Some(i) = index(key) {
                        items[i - 1] = to_json(item, depth + 1)?;
                    }
                }
                Json::Array(items)
            } else {
                let mut object = serde_json::Map::new();
                for (key, item) in &pairs {
                    let key = match key {
                        Value::String(s) => s.to_string_lossy(),
                        Value::Integer(_) | Value::Number(_) => display(key),
                        other => return Err(format!("a table's key can't be a {}", other.type_name())),
                    };
                    object.insert(key, to_json(item, depth + 1)?);
                }
                Json::Object(object)
            }
        }
        other => return Err(format!("a {} can't be sent", other.type_name())),
    })
}

fn number(n: f64) -> Json {
    if !n.is_finite() {
        Json::Null
    } else if n.fract() == 0.0 && n.abs() < 9.0e15 {
        Json::from(n as i64)
    } else {
        Json::from(n)
    }
}

/// Write beside `path` and rename over it (readers never see half a file).
fn write_replace(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("store");
    let temp = path.with_file_name(format!(".{name}.tmp-{}", std::process::id()));
    std::fs::write(&temp, bytes).and_then(|()| std::fs::rename(&temp, path))
}

#[cfg(test)]
mod tests;
