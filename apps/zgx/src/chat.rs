//! A chat, for the chat page's scripts (ui/screens/chat): the session open
//! in a window, its transcript as it streams, the models to pick from, and
//! what can be asked of it. It speaks the engine's IPC as the shipping app's
//! chat view does (crates/ui: state.rs's transcript watch, composer.rs's
//! sends, queue.rs, pickers.rs).
//!
//! What scripts see, per window:
//!   `chat.session`   the open session: { id, state = "loading" | "ready" |
//!                    "missing" | "offline", title, project, device, harness,
//!                    model, effort, fast, access, plan, running, context,
//!                    context_tokens?, context_window?, ask?, queue, todos,
//!                    tasks }, nil for none
//!   `chat.messages`  its transcript as ds/transcript's messages, keyed by id
//!   `chat.draft`     the reply being written: { id, text }
//! and for every window, `chat.catalog`: { state, harnesses, models } as
//! ds.ModelPicker takes them, a model's `value` "<harness>/<model>".
//!
//! A reply streams as `append` frames, which carry only its new words: they
//! reach `chat.draft` alone, so a word costs a small state's write, not the
//! whole transcript's. Anything else (a tool, a new entry, the turn ending)
//! publishes `chat.messages` again whole, which `Shared::set` reconciles, so
//! the messages that didn't change keep their rows.
//!
//! Calls (each rejecting with an outcome word: offline, signed_out, missing,
//! failed):
//!   chat.open(id?) (an action)    watch that chat, the last one let go
//!   chat.catalog (an action)      the models, looked up again
//!   chat.send(req)                run a message, or queue it while a turn runs
//!   chat.interrupt(id)
//!   chat.respond({ id, request, decision = "once" | "always" | "deny" |
//!                  "answer", values?, text? })
//!   chat.configure({ id, model?, effort?, fast?, access?, plan? })
//!   chat.queue({ id, op = "remove" | "send" | "move", item, to? })
//!
//! The wire (crates/rpc, crates/doc): `WatchDocMessages` (a reset, then
//! deltas: upserts after an anchor, text appended to a part, removals, the
//! count to check against; byte lengths), `WatchSessions` and `WatchChats`
//! (whole lists), `WatchQueue`; runs, interrupts and answers through
//! `QueueCommand`; config through `Mutate setChatConfig`; uploads through
//! `UploadChunk` / `UploadCommit` (for a chat on a Cloud machine, staged here
//! and sent by the engine through the edge's store: `WatchHeldSends`). `ZERON_ZGX_MOCK=1` offers the engine's
//! mock agent, for trying a turn without a paid one.

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::{Rc, Weak},
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::{Value as Json, json};
use zgx_desktop::WindowContext;
use zgx_script::{ScriptHost, Shared, Value};

use crate::auth::text;
use crate::engine::{
    self,
    rpc::{Rpc, Watch},
};

type Outcome = &'static str;

/// A working session the engine hasn't touched for this long has died
/// with its host (proto/view.rs).
const STALE_SECONDS: i64 = 45;
/// How much of an upload goes in one chunk, in base64 characters (the
/// shipping app's).
const CHUNK: usize = 680_000;

thread_local! {
    static CHATS: Rc<Chats> = Rc::new(Chats::default());
}

/// What every window shares: the engine's chats and sessions, this device,
/// the agents and their models.
#[derive(Default)]
struct Chats {
    /// The engine's outbox (`WatchHeldSends`): sends waiting for their files
    /// to reach the edge's store.
    held: RefCell<Vec<Json>>,
    chats: RefCell<HashMap<String, Json>>,
    chats_heard: Cell<bool>,
    sessions: RefCell<HashMap<String, Json>>,
    here: RefCell<Option<String>>,
    harnesses: RefCell<Vec<Json>>,
    /// Models by harness, as the engine lists them.
    models: RefCell<HashMap<String, Vec<Json>>>,
    catalog: RefCell<Vec<Shared>>,
    catalog_state: Cell<CatalogState>,
    views: RefCell<Vec<Weak<View>>>,
    /// The engine's live session and chat lists, heard as changes.
    listening: RefCell<Vec<engine::lists::Listening>>,
    /// The engine's outbox, on the connection there is.
    held_watch: RefCell<Option<Watch>>,
    /// Chats this window made that the engine's list may not have yet
    /// (`made`): its next list replaces them, or lets them go.
    made_here: RefCell<Vec<String>>,
    /// New chats on Cloud this window opened before Cloud made them (the
    /// shipping app's optimistic new chat): shown at once, their first message
    /// sending, watched once made (`chat.created`).
    creating: RefCell<HashMap<String, Json>>,
    /// Sends to a chat still being made (its first message not gone yet):
    /// shown, sending, and sent in order once its first message has gone.
    after_create: RefCell<HashMap<String, Vec<Value>>>,
    rpc: RefCell<Option<Rpc>>,
    /// What each chat waiting on its person asks: "approval" or
    /// "question", by chat (`chat.asks`, for the sidebar's rows).
    asks: RefCell<HashMap<String, &'static str>>,
    asks_shared: RefCell<Vec<Shared>>,
    /// A look at a waiting chat's latest entries, to tell which, until
    /// its first frame comes.
    ask_looks: RefCell<HashMap<String, Watch>>,
    /// The agents and models of the device the composer runs on, when that
    /// isn't this one (a remote session's, or Work in's for a new chat):
    /// `chat.catalog` is then its catalog, as the shipping app's pickers
    /// read their run device's (`list_harnesses(device)`).
    remote: RefCell<Option<Remote>>,
    /// Each device's catalog as last looked at whole (some looked at ahead of
    /// time, `look_ahead`): moving to it shows this at once, while it's looked
    /// at again, so a model is never missing for the moment that takes.
    known_remotes: RefCell<HashMap<String, Remote>>,
    /// Which look at `remote` is the latest (`reload_remote`): an older one's
    /// answers, for this device or another, change nothing.
    remote_turn: Cell<u64>,
    /// The look at `remote` settled before all its answers came
    /// (`settle_remote`): its catalog stands complete with what it has, and
    /// answers still to come fill it in without its going back to looking.
    remote_settled: Cell<u64>,
    /// The chats views have let go of lately, the most recent last
    /// (`LET_GO_MOST`, `LET_GO_KEPT`): one opened again (switched back to,
    /// or a chat on its own becoming a split) shows its rows at once, from
    /// the first frame, rather than a skeleton while it's read and built
    /// again; the engine's copy then brings what changed.
    let_go: RefCell<Vec<LetGo>>,
    /// Moves with each chat let go of: only the last one's look at the
    /// footprint, once the app has rested, counts.
    let_go_turn: Cell<u64>,
}

/// A transcript's entry. A settled one, built into its row and older than
/// the newest few, keeps only its id and a sum of what it was: its row says
/// all it says, and the engine has it if it's wanted again.
struct Entry {
    id: Rc<str>,
    json: Option<Json>,
    /// `sum_of` its json, once that's gone.
    sum: u64,
    /// Since its row was last built, only these of its parts changed (each
    /// by a part patch): the rest are as they were built. None: not known
    /// (it came whole, or grew a text).
    patched: Option<std::collections::HashSet<String>>,
}

impl Entry {
    fn new(json: Json) -> Self {
        Self { id: json["id"].as_str().unwrap_or_default().into(), json: Some(json), sum: 0, patched: None }
    }

    /// What it is now, to tell it from another copy of it.
    fn sum(&self) -> u64 {
        self.json.as_ref().map_or(self.sum, sum_of)
    }
}

/// `entry["field"]`: as the engine sent it; nil for one settled.
impl std::ops::Index<&str> for Entry {
    type Output = Json;
    fn index(&self, field: &str) -> &Json {
        self.json.as_ref().map_or(&Json::Null, |json| &json[field])
    }
}

/// How many of a chat's newest entries keep what the engine sent: what a turn
/// still changes, and what its question and draft are read from.
const WHOLE_TAIL: usize = 32;

/// A sum of a JSON value, the same for equal values (an object's keys are
/// sorted: this workspace's serde_json has no `preserve_order`).
fn sum_of(json: &Json) -> u64 {
    // The engine's `entry_sum` (crates/doc/src/transcript_delta.rs), byte for
    // byte: a reopen says what it holds by these, and the engine sends only
    // the entries whose sums differ. FNV-1a, an object's fields in key order.
    struct Fnv(u64);
    impl Fnv {
        fn bytes(&mut self, bytes: &[u8]) {
            for byte in bytes {
                self.0 ^= u64::from(*byte);
                self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
            }
        }
        fn len(&mut self, len: usize) {
            self.bytes(&(len as u64).to_le_bytes());
        }
    }
    fn feed(json: &Json, h: &mut Fnv) {
        match json {
            Json::Null => h.bytes(&[0]),
            Json::Bool(b) => h.bytes(&[1, u8::from(*b)]),
            Json::Number(n) => {
                let n = n.to_string();
                h.bytes(&[2]);
                h.len(n.len());
                h.bytes(n.as_bytes());
            }
            Json::String(s) => {
                h.bytes(&[3]);
                h.len(s.len());
                h.bytes(s.as_bytes());
            }
            Json::Array(items) => {
                h.bytes(&[4]);
                h.len(items.len());
                items.iter().for_each(|item| feed(item, h));
            }
            Json::Object(fields) => {
                h.bytes(&[5]);
                h.len(fields.len());
                let mut keys: Vec<&String> = fields.keys().collect();
                keys.sort();
                for key in keys {
                    h.len(key.len());
                    h.bytes(key.as_bytes());
                    feed(&fields[key], h);
                }
            }
        }
    }
    let mut h = Fnv(0xcbf2_9ce4_8422_2325);
    feed(json, &mut h);
    h.0
}

/// A settled entry's row as its row holds it out of view: what makes it,
/// compressed, opened as a script reads it. The transcript's scripts, a
/// template's or a plugin's read the row it opens to, as they would any.
struct FoldedEntry {
    bytes: Box<[u8]>,
    context: Rc<RowContext>,
}

/// What a chat's rows are built with (`built_entry`).
struct RowContext {
    doc: String,
    device: Option<String>,
    model: Option<String>,
}

impl zgx_script::Unfold for FoldedEntry {
    fn unfold(&self) -> Value {
        let json = lz4_flex::block::decompress_size_prepended(&self.bytes).ok();
        let entry = json.and_then(|bytes| serde_json::from_slice::<Json>(&bytes).ok()).unwrap_or(Json::Null);
        let RowContext { doc, device, model } = &*self.context;
        built_entry(&entry, doc, device.as_deref(), model.as_deref()).unwrap_or(Value::Nil)
    }
}

/// `bytes` compressed: fast to make, as a long chat's are made each time it opens,
/// and to open, as a scroll does. (LZ4 makes them in a quarter of the time deflate's
/// fastest level does and opens them in a twelfth, for rows a quarter larger, measured
/// on 16,271 rows of real chats; deflate spends most of its time on a table per row.)
///
/// Held at its own length: LZ4 writes into a buffer made for its worst case
/// (about the input's size), and a shrinking realloc keeps a block where it
/// is when it loses less than half, so a row kept nearly its uncompressed
/// size (30.7 MB for a 15,443-row chat whose rows compress to 20.4 MB).
fn compressed(bytes: &[u8]) -> Box<[u8]> {
    Box::from(lz4_flex::block::compress_prepend_size(bytes).as_slice())
}

/// An entry's row, as a view keeps it: its subagents apart, as the transcript
/// lists them without opening every row (`Subagents`).
#[derive(Clone, Default)]
struct Built {
    row: Option<Value>,
    subagents: Option<Rc<[Value]>>,
    /// Folded, its compressed bytes' length (what a chat kept is weighed by).
    folded: usize,
}

/// `entry` built: its row, folded if `fold`, which a row that shows or may
/// change isn't. What it shows of it is read as the row is, and held in `Built`.
fn built_for(entry: &Json, context: &Rc<RowContext>, fold: bool) -> Built {
    let RowContext { doc, device, model } = &**context;
    let row = built_entry(entry, doc, device.as_deref(), model.as_deref());
    let subagents = row.as_ref().and_then(subagents_in);
    let mut folded = 0;
    let row = match row {
        Some(_) if fold && entry["status"].as_str() != Some("streaming") => match (entry["id"].as_str(), serde_json::to_vec(entry)) {
            (Some(id), Ok(bytes)) => Some(Value::folded(
                id.into(),
                FoldedEntry {
                    bytes: {
                        let bytes = compressed(&bytes);
                        folded = bytes.len();
                        bytes
                    },
                    context: context.clone(),
                },
            )),
            _ => row,
        },
        row => row,
    };
    Built { row, subagents, folded }
}

/// A long chat's settled entries, as a worker thread makes them: what they fold
/// to, and all it read of them. Building and compressing thousands of rows on the
/// main thread takes the window's time and strews its heap's pages with what
/// little stays, so they stay held after the chat has gone. The worker's are its own.
struct Folding {
    results: std::sync::mpsc::Receiver<Vec<FoldedOff>>,
}

/// An entry, folded on the worker (`Folding`).
struct FoldedOff {
    /// Where it stood, and its id, to find it again.
    at: usize,
    id: String,
    /// None: it shows nothing.
    bytes: Option<Box<[u8]>>,
    subagents: Vec<SubagentOff>,
    sum: u64,
    /// The plan in it, if there is one (`Open::older_todo`).
    todo: Option<Json>,
}

/// A subagent's row in `Open`'s list, as plain data to cross threads.
struct SubagentOff {
    reference: String,
    title: Option<String>,
    status: Option<String>,
    at: Option<f64>,
}

impl SubagentOff {
    fn of(sub: &Value) -> Option<Self> {
        let words = |key: &str| match sub.field(key) {
            Some(Value::Str(words)) => Some(words.to_string()),
            _ => None,
        };
        Some(Self {
            reference: words("ref")?,
            title: words("title"),
            status: words("status"),
            at: sub.field("at").and_then(Value::as_f64),
        })
    }

    fn value(&self) -> Value {
        let words = |words: &Option<String>| words.as_deref().map_or(Value::Nil, Value::from);
        Value::record([
            ("ref", Value::from(self.reference.as_str())),
            ("title", words(&self.title)),
            ("status", words(&self.status)),
            ("at", self.at.map_or(Value::Nil, Value::Num)),
        ])
    }
}

/// Whether `entry` plainly has a row, from its parts alone: building it would
/// say so too, at the cost of all a long chat's rows each time it opens. `false`
/// only means that takes building it.
fn plainly_has_row(entry: &Json) -> bool {
    if entry["id"].as_str().is_none() {
        return false;
    }
    match entry["role"].as_str() {
        Some("user") => true,
        // As `blocks_of`: a tool that isn't its output alone, or a thought with words.
        Some("assistant") => entry["parts"].as_array().into_iter().flatten().any(|part| match part["kind"].as_str() {
            Some("tool") => !display_tool(part),
            Some("reasoning") => !part["text"].as_str().unwrap_or_default().trim().is_empty(),
            _ => false,
        }),
        _ => false,
    }
}

/// Whether a part of `entry` names a subagent (`tool_row`), all `subagents_in` finds.
fn names_subagent(entry: &Json) -> bool {
    entry["parts"].as_array().into_iter().flatten().any(|part| part["subagentRef"].as_str().is_some_and(|name| !name.is_empty()))
}

/// `entry`, as the worker folds it.
fn fold_off(at: usize, id: String, entry: &Json, doc: &str, device: Option<&str>, model: Option<&str>) -> FoldedOff {
    // (Its row is built here only if what's wanted of it isn't plain from its parts.)
    let plain = plainly_has_row(entry) && !names_subagent(entry);
    let row = (!plain).then(|| built_entry(entry, doc, device, model)).flatten();
    let subagents = row.as_ref().and_then(subagents_in).map_or(Vec::new(), |found| found.iter().filter_map(SubagentOff::of).collect());
    let bytes = (plain || row.is_some()).then(|| serde_json::to_vec(entry).ok()).flatten().map(|bytes| compressed(&bytes));
    FoldedOff { at, id, bytes, subagents, sum: sum_of(entry), todo: latest_todo(std::iter::once(entry)).cloned() }
}

/// How many entries the worker sends at a time.
const FOLDED_AT_ONCE: usize = 64;

/// A worker folding `entries`, newest first.
fn folding(entries: Vec<(usize, String, Json)>, context: &RowContext) -> Folding {
    let (sender, results) = std::sync::mpsc::channel();
    let (doc, device, model) = (context.doc.clone(), context.device.clone(), context.model.clone());
    let spawned = std::thread::Builder::new().name("zeron-fold".into()).spawn(move || {
        let mut batch = Vec::with_capacity(FOLDED_AT_ONCE);
        for (at, id, entry) in entries {
            batch.push(fold_off(at, id, &entry, &doc, device.as_deref(), model.as_deref()));
            if batch.len() == FOLDED_AT_ONCE {
                // (The chat has gone, or a reset replaced them: nothing wants the rest.)
                if sender.send(std::mem::take(&mut batch)).is_err() {
                    return;
                }
                batch.reserve(FOLDED_AT_ONCE);
            }
        }
        let _ = sender.send(batch);
    });
    if let Err(error) = spawned {
        eprintln!("zeron-zgx: a chat's rows: no worker ({error})");
    }
    Folding { results }
}

/// What a view's chat had read and built, as it let it go.
struct LetGo {
    /// When it was let go of.
    at: std::time::Instant,
    /// About what it holds, in bytes (`let_go_weight`).
    bytes: usize,
    id: String,
    entries: Vec<Entry>,
    older_todo: Option<(usize, Json)>,
    built: HashMap<Rc<str>, Built>,
    built_model: Option<Option<String>>,
    context: Option<f64>,
    context_usage: (Option<f64>, Option<f64>),
}

/// How long a chat let go of is kept for a view to open again, and how many
/// are kept at most (a long chat's rows are its whole transcript, built).
const LET_GO_KEPT: std::time::Duration = std::time::Duration::from_secs(600);
const LET_GO_MOST: usize = 6;
/// How long after a chat is let go of the app is taken to have rested, its
/// footprint then looked at (`room_to_keep`).
const LET_GO_SETTLE: std::time::Duration = std::time::Duration::from_secs(5);
/// And at most about this much of them, the oldest going first: a long
/// chat's kept rows (7,900 entries, 18 MB as JSON) cost some 14 MB, and the
/// app rests near 265 MB of its 300 MB: one long chat, or a few others.
const LET_GO_BYTES: usize = 20 << 20;
/// Nothing kept while the app's footprint (what Activity Monitor shows) is
/// past this: what's kept is the first to go when memory is short.
#[cfg(target_os = "macos")]
const LET_GO_FOOTPRINT: u64 = 280 << 20;

/// The process's footprint in bytes, as Activity Monitor shows it.
#[cfg(target_os = "macos")]
fn footprint() -> Option<u64> {
    let mut info: libc::rusage_info_v4 = unsafe { std::mem::zeroed() };
    // SAFETY: `info` is a rusage_info_v4 the call fills in, as RUSAGE_INFO_V4 asks.
    let read = unsafe { libc::proc_pid_rusage(std::process::id() as i32, libc::RUSAGE_INFO_V4, (&mut info as *mut libc::rusage_info_v4).cast()) };
    (read == 0).then_some(info.ri_phys_footprint)
}

/// Whether the app has room to keep a chat let go of.
fn room_to_keep() -> bool {
    #[cfg(target_os = "macos")]
    if footprint().is_some_and(|bytes| bytes > LET_GO_FOOTPRINT) {
        return false;
    }
    true
}

/// `ZERON_ZGX_KEEP_CHATS=0`: chats let go of are kept as they were before
/// (one, two seconds, for a split taking it), to compare what keeping costs.
fn keeping_chats() -> bool {
    thread_local! {
        static ON: bool = std::env::var("ZERON_ZGX_KEEP_CHATS").map_or(true, |v| v != "0");
    }
    ON.with(|on| *on)
}
/// What each entry costs beyond its folded bytes (its id, sum, row slots).
const LET_GO_PER_ENTRY: usize = 512;

/// About what a chat's kept rows hold: each settled row's folded bytes, each
/// entry's slots, and the newest entries' JSON, whole, with the rows built
/// from it.
fn let_go_weight(entries: &[Entry], built: &HashMap<Rc<str>, Built>) -> usize {
    let folded: usize = built.values().map(|b| b.folded).sum();
    let whole: usize = entries
        .iter()
        .filter_map(|e| e.json.as_ref())
        .map(|json| serde_json::to_vec(json).map_or(0, |b| b.len()))
        .sum();
    folded + entries.len() * LET_GO_PER_ENTRY + 2 * whole
}

/// The chats let go of that are still kept: none older than `LET_GO_KEPT`,
/// and the newest `LET_GO_MOST`.
fn prune_let_go(kept: &mut Vec<LetGo>) {
    let (most, kept_for) = if keeping_chats() { (LET_GO_MOST, LET_GO_KEPT) } else { (1, std::time::Duration::from_secs(2)) };
    kept.retain(|held| held.at.elapsed() < kept_for);
    let over = kept.len().saturating_sub(most);
    kept.drain(..over);
    // (The oldest first, until what's kept fits.)
    while keeping_chats() && kept.iter().map(|held| held.bytes).sum::<usize>() > LET_GO_BYTES {
        kept.remove(0);
    }
}

/// How long after connecting other devices' catalogs start being looked at.
const LOOK_AHEAD_AFTER: std::time::Duration = std::time::Duration::from_secs(6);

/// How long after a device's catalog didn't come it's asked again: each
/// try waits the next, the last one again after that (`load_remote`).
const RETRY_AFTER: [std::time::Duration; 4] = [
    std::time::Duration::from_secs(1),
    std::time::Duration::from_secs(3),
    std::time::Duration::from_secs(8),
    std::time::Duration::from_secs(15),
];
/// How long a look at another device's catalog may go unanswered before it
/// settles with what it has (`settle_remote`): an offline device's list
/// never comes, and the model picker shouldn't wait on it forever.
const SETTLE_AFTER: std::time::Duration = std::time::Duration::from_secs(4);

/// How many times an agent's models are asked again on another device
/// before it's left out (one signed out fails every time).
const MODEL_RETRIES: usize = 2;

/// How many entries a reset may leave to build at once; more go to a worker and
/// in slices.
const BUILT_AT_ONCE: usize = 64;
/// A slice of building, and the rest after it, so frames get the thread.
const BUILD_SLICE: std::time::Duration = std::time::Duration::from_millis(3);
const BUILD_REST: std::time::Duration = std::time::Duration::from_millis(5);

/// Another device's catalog, read through the engine (`targetDeviceId`).
/// (Kept in `known_remotes`, `ready` is whether it was whole: every agent's
/// models came.)
#[derive(Clone)]
struct Remote {
    device: String,
    harnesses: Vec<Json>,
    models: HashMap<String, Vec<Json>>,
    ready: bool,
}

/// A catalog looked at ahead of time, kept unless the device has one: one
/// short of an agent's models gives way to a whole one.
fn keep_known(known: &mut HashMap<String, Remote>, remote: Remote) {
    if known.get(&remote.device).is_none_or(|r| !r.ready && remote.ready) {
        known.insert(remote.device.clone(), remote);
    }
}

#[derive(Clone, Copy, Default, PartialEq)]
enum CatalogState {
    #[default]
    Looking,
    Loading,
    Ready,
}

/// A window's open chat.
struct View {
    session: Shared,
    messages: Shared,
    /// Its transcript's subagents, so reading them doesn't read a transcript.
    subagents: Shared,
    draft: Shared,
    rpc: RefCell<Option<Rpc>>,
    open: RefCell<Option<Open>>,
    /// Prompts not yet in the engine's transcript, by chat. They outlive
    /// the open view: a send can still be in flight while another chat shows.
    pending: RefCell<HashMap<String, Vec<Pending>>>,
    /// Moves on every open: an older watch's frames are dropped.
    generation: Cell<u64>,
    /// A finished subagent's transcript, read whole from its uploaded blob
    /// (a lane's): nothing to watch while it stands.
    snapshot: Cell<bool>,
}

/// A prompt sent and not yet in the transcript: shown under it meanwhile,
/// sending, or failed with what sends it again.
struct Pending {
    id: String,
    words: String,
    paths: Vec<String>,
    /// "sending" or "failed".
    waiting: &'static str,
    /// The command that sends it again, once it failed.
    retry: Option<Json>,
    /// When it was sent, as its hover strip says.
    at: Option<String>,
    /// In the engine's outbox: the engine says how it stands, and retries it.
    held: bool,
    /// Becomes a queue row when it leaves the outbox, and shows there.
    row: bool,
    /// The files the engine delivers for it, for a run sent again.
    transfers: Vec<Json>,
}

impl Pending {
    fn value(&self) -> Value {
        // (Its passages as chips, as the sent message shows them.)
        let (words, chats) = split_chats(&self.words);
        let (words, comments) = split_comments(&words);
        let (words, quotes) = split_quotes(&words);
        let (words, picks) = split_annotations(&words);
        Value::record([
            ("id", Value::from(self.id.as_str())),
            ("role", Value::from("user")),
            ("text", Value::from(words.as_str())),
            ("attachments", carried_value(&self.paths, &quotes, &picks, &comments, &chats)),
            ("waiting", Value::from(self.waiting)),
            // Still in the engine's outbox, its files uploading: nothing has
            // gone to its chat yet (a Cloud machine isn't woken for it).
            ("held", Value::Bool(self.held)),
            // (This device's own copy: its pictures are the files it was
            // sent from, here, which the chat's device never had. The
            // chat's copy, once it comes, is marked `waiting` too until
            // its reply begins, so that can't tell them apart.)
            ("here", Value::Bool(true)),
            ("at", self.at.as_deref().map_or(Value::Nil, Value::from)),
            ("copy", copy_value(&words)),
        ])
    }
}

/// An edit lease on a queued message (the engine's `BeginQueuedMessageEdit`).
#[derive(Clone)]
struct Lease {
    item: String,
    lease: String,
    hash: Json,
}

impl Open {
    /// Back a page in `request` (being answered here): its last page's
    /// answer taken off, and what was picked there. None on the first page,
    /// or for another chat's or request's question.
    fn back_a_page(&mut self, id: &str, request: &str) -> Option<Vec<String>> {
        if self.id != id || self.answering.as_deref() != Some(request) {
            return None;
        }
        self.answers.pop().map(|(_, labels)| labels)
    }
}

#[derive(Default)]
struct Open {
    id: String,
    /// The transcript's entries, as the engine's doc has them (`Entry`).
    entries: Vec<Entry>,
    /// The plan as the newest settled entry holding one left it, and where
    /// that entry stands: the entries are gone, the plan still shows.
    older_todo: Option<(usize, Json)>,
    /// A reset has come (the newest entries, at least).
    heard: bool,
    context: Option<f64>,
    /// What the context holds as the agent last reported it: tokens in it
    /// and the window's size (either may be missing), for the ring's card.
    context_usage: (Option<f64>, Option<f64>),
    queue: Vec<Json>,
    /// Its messages on their way and not yet taken by its agent, as the engine's
    /// `WatchWaitingMessages` lists them (none from an engine without it).
    waiting: Vec<Json>,
    /// The session as last published, to republish the transcript when its
    /// turn starts or ends (the prompt's working line follows it).
    session_seen: Json,
    /// A question's answers so far, page by page, for the request asked.
    answers: Vec<(String, Vec<String>)>,
    answering: Option<String>,
    /// A queued message being edited here: its lease, held while the
    /// composer has its text.
    editing: Option<Lease>,
    watches: Vec<Watch>,
    /// Each entry as last published (nil: it shows nothing), by its id, and
    /// the model's name it was built with: a republish builds again only
    /// the entries a frame changed, and hands the rest over as the same
    /// values, so the script's copy (reconciled by id) does no work for
    /// them.
    built: HashMap<Rc<str>, Built>,
    built_model: Option<Option<String>>,
    /// What its rows are built with, shared by those folded.
    row_context: Option<Rc<RowContext>>,
    /// The chat's model changed while settled entries' rows carry the last
    /// one's name: its whole copy is asked for again to build them from.
    model_moved: bool,
    /// Its whole copy, when it comes, builds every row again (`model_moved`).
    rebuild: bool,
    /// Opened with the rows another view had (`LetGo`), which the engine's
    /// first frames, its newest entries, mustn't replace.
    has_whole: bool,
    /// Building a reset's entries a slice at a time (`build_in_steps`): what
    /// shows stays as it is until that's done.
    stepping: bool,
    /// The worker folding its settled entries, meanwhile.
    folding: Option<Folding>,
    /// They have gone to it (once a build).
    builds_started: bool,
    /// This view is the one doing it (another view of the chat waits).
    builds: bool,
}

pub fn install(_cx: &mut WindowContext, host: &ScriptHost) {
    let chats = CHATS.with(Rc::clone);
    chats.catalog.borrow_mut().push(host.shared("chat.catalog", chats.catalog_value()));
    chats.asks_shared.borrow_mut().push(host.shared("chat.asks", chats.asks_value()));
    let view = Rc::new(View {
        session: host.shared("chat.session", Value::Nil),
        messages: host.shared("chat.messages", Value::List(Rc::from([]))),
        subagents: host.shared("chat.subagents", Value::List(Rc::from([]))),
        draft: host.shared("chat.draft", draft_value("", "")),
        rpc: RefCell::default(),
        open: RefCell::default(),
        pending: RefCell::default(),
        generation: Cell::new(0),
        snapshot: Cell::new(false),
    });
    chats.views.borrow_mut().push(Rc::downgrade(&view));
    if chats.views.borrow().len() == 1 {
        let weak = Rc::downgrade(&chats);
        engine::get().on_connect(move |rpc| {
            if let Some(chats) = weak.upgrade() {
                chats.connected(rpc);
            }
        });
        let weak = Rc::downgrade(&chats);
        engine::get().on_unreachable(move || {
            if let Some(chats) = weak.upgrade() {
                chats.publish_views();
            }
        });
    }
    let weak = Rc::downgrade(&view);
    engine::get().on_connect(move |rpc| {
        if let Some(view) = weak.upgrade() {
            *view.rpc.borrow_mut() = Some(rpc.clone());
            view.hold_rows();
            view.subscribe();
        }
    });

    let v = view.clone();
    host.action("chat.open", move |id| {
        let id = text(&id);
        v.open_chat((!id.is_empty()).then_some(id));
    });
    // A window's lanes: more chats watched beside the open one, each in a
    // slot of its own (a panel tab's side chat, or a subagent's doc), its
    // states `chat.lane.<slot>.session`, `.messages`, `.draft`, as the open
    // chat's are. `{ slot, id, frozen?, chat? }`; no id lets the slot's chat
    // go. `frozen`: a finished subagent's doc, read from its upload first.
    let lanes: Rc<RefCell<HashMap<String, Rc<View>>>> = Rc::default();
    {
        let (lanes, host) = (lanes.clone(), host.clone());
        host.clone().action("chat.open_lane", move |args| {
            let slot = args.field("slot").map(text).unwrap_or_default();
            if slot.is_empty() {
                return;
            }
            let id = args.field("id").map(text).filter(|s| !s.is_empty());
            let view = lanes.borrow_mut().entry(slot.clone()).or_insert_with(|| lane_view(&host, &slot)).clone();
            // A finished subagent: its uploaded transcript first.
            match (id, args.field("frozen").is_some_and(Value::truthy)) {
                (Some(id), true) => view.open_frozen(id, args.field("chat").map(text).filter(|s| !s.is_empty())),
                (id, _) => view.open_chat(id),
            }
        });
    }
    // The view a call is about: the lane that has its chat open, else the
    // window's open chat.
    let which = {
        let (main, lanes) = (view.clone(), lanes.clone());
        move |args: &Value| -> Rc<View> {
            let id = args.field("session").or_else(|| args.field("id")).map(text).unwrap_or_default();
            lanes.borrow().values().find(|l| l.open_id().as_deref() == Some(id.as_str())).cloned().unwrap_or_else(|| main.clone())
        }
    };
    // Settings' compact transcript, on or off (`chat.compact`, set by
    // screens/settings/prefs): a reply being written is laid out compact too
    // only while it's on, so each of its changes builds what shows. Turned
    // on, those replies are built again with it. (Kept as long as the
    // window's runtime: one watch a window.)
    let compact = host.shared("chat.compact", Value::Bool(false)).signal();
    let c = chats.clone();
    std::mem::forget(_cx.ui.runtime.effect(move || {
        let on = compact.get().truthy();
        c.compact_shown(on);
    }));
    // A new chat on Cloud, opened before Cloud has made it (the shipping app
    // selects a new chat and shows its first message at once): known here as
    // a chat on Cloud, its first message sending (`req`: the send it will
    // get, `session` its id). `chat.created` watches it once Cloud has made
    // it; `chat.create_failed` lets it go.
    let (c, pick) = (chats.clone(), which.clone());
    host.action("chat.creating", move |req| {
        let id = req.field("session").map(text).unwrap_or_default();
        let message_id = req.field("message_id").map(text).unwrap_or_default();
        if id.is_empty() || message_id.is_empty() {
            return;
        }
        let mut row = json!({ "id": id, "cloud": true });
        if let Some(project) = req.field("project").map(text).filter(|p| !p.is_empty()) {
            row["spaceId"] = Json::from(project);
        }
        c.creating.borrow_mut().insert(id.clone(), row.clone());
        c.made(&id, row);
        let view = pick(&req);
        let mut pending = view.pending.borrow_mut();
        let prompts = pending.entry(id).or_default();
        prompts.retain(|p| p.id != message_id);
        prompts.push(Pending {
            id: message_id,
            words: req.field("text").map(text).unwrap_or_default(),
            paths: local_paths(&req),
            waiting: "sending",
            retry: None,
            at: stamp(now_seconds()),
            held: false,
            row: false,
            transfers: Vec::new(),
        });
    });
    let c = chats.clone();
    host.action("chat.created", move |id| {
        let id = text(&id);
        if c.creating.borrow_mut().remove(&id).is_none() {
            return;
        }
        for view in c.views() {
            if view.open_id().as_deref() == Some(id.as_str()) {
                view.subscribe();
                view.publish_session();
            }
        }
    });
    let c = chats.clone();
    host.action("chat.create_failed", move |id| {
        let id = text(&id);
        c.creating.borrow_mut().remove(&id);
        c.after_create.borrow_mut().remove(&id);
        c.made_here.borrow_mut().retain(|m| *m != id);
        c.chats.borrow_mut().remove(&id);
        for view in c.views() {
            view.pending.borrow_mut().remove(&id);
        }
    });
    let c = chats.clone();
    host.action("chat.catalog", move |_| {
        let c = c.clone();
        engine::get().spawn(async move { c.load_catalog(true).await });
    });
    // The composer's device: its catalog is `chat.catalog` from now on (nil
    // or this device: this one's).
    let c = chats.clone();
    host.action("chat.catalog_on", move |arg| {
        let device = arg.field("device").map(crate::auth::text).unwrap_or_default();
        c.catalog_on(&device);
    });
    let pick = which.clone();
    host.async_action("chat.send", move |req| {
        let v = pick(&req);
        async move {
            crate::plugins::preview::refuse("chat.send")?;
            let first = req.field("fresh").is_some_and(Value::truthy);
            let id = req.field("session").map(text).unwrap_or_default();
            let sent = v.send(req).await;
            // A new chat's first message gone: what was sent to it meanwhile
            // goes now, in order.
            if first {
                loop {
                    let next = CHATS.with(|c| c.after_create.borrow_mut().remove(&id)).unwrap_or_default();
                    if next.is_empty() {
                        break;
                    }
                    for later in next {
                        let _ = v.send(later).await;
                    }
                }
            }
            sent.map(|_| Value::Nil).map_err(String::from)
        }
    });
    host.async_action("chat.interrupt", move |id| async move {
        command(&text(&id), json!({ "kind": "interrupt" })).await?;
        Ok(Value::Nil)
    });
    // A background process of the chat's stopped, on its host (the shipping
    // app's `stop_background_task`): whether it's stopping.
    host.async_action("chat.stop_task", move |args| async move { stop_task(args).await.map_err(String::from) });
    host.async_action("chat.stop_waiting", move |id| async move { stop_waiting(text(&id)).await.map_err(String::from) });
    let pick = which.clone();
    host.async_action("chat.respond", move |args| {
        let v = pick(&args);
        async move { crate::plugins::preview::refuse("chat.respond")?; v.respond(args).await.map(|_| Value::Nil).map_err(String::from) }
    });
    // A question's Back: its page before again, with what was picked there.
    let back = which.clone();
    host.async_action("chat.question_back", move |args| {
        let v = back(&args);
        async move { Ok::<Value, String>(v.question_back(&args)) }
    });
    let v = view.clone();
    host.async_action("chat.configure", move |args| {
        let v = v.clone();
        // (Its model and effort are a preview's to change; what an agent may
        // do unasked isn't.)
        let access = args.field("access").is_some();
        async move { if access { crate::plugins::preview::refuse("chat.configure")?; } v.configure(args).await.map(|_| Value::Nil).map_err(String::from) }
    });
    let v = view.clone();
    host.async_action("chat.queue", move |args| {
        let v = v.clone();
        async move { crate::plugins::preview::refuse("chat.queue")?; v.queue_op(args).await.map(|_| Value::Nil).map_err(String::from) }
    });
    let pick = which.clone();
    host.async_action("chat.retry", move |args| {
        let v = pick(&args);
        async move { crate::plugins::preview::refuse("chat.retry")?; v.retry(args).await.map(|_| Value::Nil).map_err(String::from) }
    });
    // A send in this device's outbox (still uploading, or stopped) taken out
    // for good: `{ id, item, message? }` → whether it was there to take
    // ("unsupported": an engine from before it can).
    let pick = which.clone();
    host.async_action("chat.remove_held", move |args| {
        let v = pick(&args);
        async move { crate::plugins::preview::refuse("chat.remove_held")?; v.remove_held(args).await.map_err(String::from) }
    });
    let pick = which.clone();
    host.async_action("chat.queue_edit", move |args| {
        let v = pick(&args);
        async move { crate::plugins::preview::refuse("chat.queue_edit")?; v.begin_edit(args).await.map_err(String::from) }
    });
    let pick = which.clone();
    host.async_action("chat.queue_finish", move |args| {
        let v = pick(&args);
        async move { crate::plugins::preview::refuse("chat.queue_finish")?; v.finish_edit(args).await.map(|_| Value::Nil).map_err(String::from) }
    });
    // A project's branches, from git where it is (the shipping app's
    // `ensure_refs`): `{ path, device? }`, the device only when it isn't this
    // one (the engine forwards it there).
    host.async_action("chat.refs", move |args| async move { refs(args).await.map_err(String::from) });
}

/// Where a subagent's uploaded transcript is: `{chat}/{doc}`, the chat it
/// was spawned in named by its doc (`{chat}--sub--{n}`; a copied spawn chip
/// keeps its first chat's), else the one it's shown under.
fn blob_ref(chat: Option<&str>, doc: &str) -> String {
    let source = doc.split_once("--sub--").map(|(source, _)| source).or(chat).unwrap_or(doc);
    format!("{source}/{doc}")
}

/// `FetchToolBlob`'s reply as a transcript's reset frame: its `text`, a
/// JSON list of entries.
fn snapshot_frame(reply: &Json) -> Option<Json> {
    let entries: Json = serde_json::from_str(reply.get("text")?.as_str()?).ok()?;
    entries.is_array().then(|| json!({ "reset": entries }))
}

/// A lane's view: its own states, heard as the window's open chat is (the
/// catalog, sessions and every connection reach it too).
fn lane_view(host: &ScriptHost, slot: &str) -> Rc<View> {
    let chats = CHATS.with(Rc::clone);
    let view = Rc::new(View {
        session: host.shared(&format!("chat.lane.{slot}.session"), Value::Nil),
        messages: host.shared(&format!("chat.lane.{slot}.messages"), Value::List(Rc::from([]))),
        subagents: host.shared(&format!("chat.lane.{slot}.subagents"), Value::List(Rc::from([]))),
        draft: host.shared(&format!("chat.lane.{slot}.draft"), draft_value("", "")),
        rpc: RefCell::default(),
        open: RefCell::default(),
        pending: RefCell::default(),
        generation: Cell::new(0),
        snapshot: Cell::new(false),
    });
    chats.views.borrow_mut().push(Rc::downgrade(&view));
    let weak = Rc::downgrade(&view);
    engine::get().on_connect(move |rpc| {
        if let Some(view) = weak.upgrade() {
            *view.rpc.borrow_mut() = Some(rpc.clone());
            view.hold_rows();
            view.subscribe();
        }
    });
    view
}

/// A project's refs, as the new chat's ref picker lists them: each branch,
/// whether it's the folder's current one, and the worktree it's checked out
/// in. The current first, then as git lists them.
async fn refs(args: Value) -> Result<Value, Outcome> {
    let field = |key: &str| args.field(key).map(text).filter(|s| !s.is_empty());
    let path = field("path").ok_or("missing")?;
    let mut params = json!({ "repoPath": path });
    if let Some(device) = field("device") {
        params["targetDeviceId"] = Json::from(device);
    }
    let found = engine::get().call("ListRefs", params).await.map_err(|e| engine::outcome(&e))?;
    Ok(refs_value(&found))
}

fn refs_value(found: &Json) -> Value {
    let mut rows: Vec<&Json> = found.as_array().into_iter().flatten().filter(|r| r["name"].is_string()).collect();
    rows.sort_by_key(|r| r["current"].as_bool() != Some(true));
    Value::List(
        rows.into_iter()
            .map(|r| {
                let mut fields = vec![
                    ("name", Value::from(r["name"].as_str().unwrap_or_default())),
                    ("current", Value::Bool(r["current"].as_bool() == Some(true))),
                ];
                if let Some(path) = r["worktreePath"].as_str() {
                    fields.push(("worktree", Value::from(path)));
                }
                Value::record(fields)
            })
            .collect::<Vec<_>>()
            .into(),
    )
}

// -- every window's -----------------------------------------------------------

/// A frame's list, its items by `field`. Taken, not copied: these lists come whole
/// for every change in any chat, a chat's every field, and a copy made here would
/// be blocks by the thousand among the window's own, where they'd stay till the next.
impl Chats {
    fn connected(self: &Rc<Self>, rpc: &Rpc) {
        let me = self.clone();
        engine::get().spawn(async move { me.look_ahead().await });
        let mut listening = Vec::new();
        let weak = Rc::downgrade(self);
        listening.push(engine::lists::listen(rpc, "WatchSessions", move |frame| {
            let Some(me) = weak.upgrade() else { return };
            frame.apply_to_map(&mut me.sessions.borrow_mut());
            me.follow_asks();
            me.publish_views();
            // A turn starting or ending: the newest prompt's line follows.
            for view in me.views() {
                let changed = {
                    let mut slot = view.open.borrow_mut();
                    let Some(open) = slot.as_mut() else { continue };
                    let now = me.sessions.borrow().get(&open.id).cloned().unwrap_or(Json::Null);
                    let changed = now["status"] != open.session_seen["status"]
                        || now["startedAt"] != open.session_seen["startedAt"];
                    open.session_seen = now;
                    changed
                };
                if changed {
                    view.publish_messages();
                }
            }
        }));
        let weak = Rc::downgrade(self);
        listening.push(engine::lists::listen(rpc, "WatchChats", move |frame| {
            let Some(me) = weak.upgrade() else { return };
            {
                let mut chats = me.chats.borrow_mut();
                frame.apply_to_map(&mut chats);
                // (One made here and still not in the list goes, as it did
                // when each list replaced them all.)
                let creating = me.creating.borrow();
                let mut kept = Vec::new();
                for id in me.made_here.borrow_mut().drain(..) {
                    let listed = frame.list.as_array().is_some_and(|rows| rows.iter().any(|r| r["id"].as_str() == Some(&id)));
                    // (One still being made on Cloud stays until the list has it.)
                    if !listed && let Some(row) = creating.get(&id) {
                        // (Put back if the list came whole: it replaced them all.)
                        chats.entry(id.clone()).or_insert_with(|| row.clone());
                        kept.push(id);
                    } else if !listed && frame.changes.is_some() {
                        chats.remove(&id);
                    }
                }
                me.made_here.borrow_mut().extend(kept);
            }
            me.chats_heard.set(true);
            me.publish_views();
        }));
        *self.listening.borrow_mut() = listening;
        let weak = Rc::downgrade(self);
        // (An engine without an outbox ends this at once, and nothing's held.)
        *self.held_watch.borrow_mut() = Some(rpc.watch("WatchHeldSends", json!({}), move |item| {
            let Some(me) = weak.upgrade() else { return };
            let held = item.as_array().cloned().unwrap_or_default();
            *me.held.borrow_mut() = held.clone();
            for view in me.views() {
                view.follow_held(&held);
            }
        }));
        *self.rpc.borrow_mut() = Some(rpc.clone());
        self.ask_looks.borrow_mut().clear();
        let me = self.clone();
        let rpc = rpc.clone();
        engine::get().spawn(async move {
            if let Some(here) = engine::local_device(&rpc).await {
                *me.here.borrow_mut() = Some(here);
            }
            // The composer's other device's, if it's on one: what an earlier
            // connection was asking is gone with it.
            me.reload_remote();
            // Every composer needs it: looked up once a connection is there.
            me.load_catalog(false).await;
        });
    }

    /// Keeps `chat.asks` to the chats waiting on their person: a new one
    /// is looked at (its latest entries) to tell an approval from a
    /// question; one no longer waiting goes.
    fn follow_asks(self: &Rc<Self>) {
        let waiting: Vec<String> = {
            let sessions = self.sessions.borrow();
            sessions
                .iter()
                .filter(|(_, s)| s["status"].as_str() == Some("awaitingInput") && !stale(&s["updatedAt"]))
                .map(|(id, _)| id.clone())
                .collect()
        };
        let mut changed = false;
        self.asks.borrow_mut().retain(|id, _| {
            let keep = waiting.contains(id);
            changed |= !keep;
            keep
        });
        self.ask_looks.borrow_mut().retain(|id, _| waiting.contains(id));
        let rpc = self.rpc.borrow().clone().filter(|r| !r.is_closed());
        for id in waiting {
            if self.asks.borrow().contains_key(&id) || self.ask_looks.borrow().contains_key(&id) {
                continue;
            }
            // The open chat's own transcript says it at once.
            let open = self.views().iter().find_map(|v| {
                let open = v.open.borrow();
                open.as_ref().filter(|o| o.id == id && o.heard).map(|o| ask_kind(o.entries.iter().rev().filter_map(|e| e.json.as_ref())))
            });
            if let Some(kind) = open {
                self.asks.borrow_mut().insert(id, kind);
                changed = true;
                continue;
            }
            let Some(rpc) = &rpc else { continue };
            let weak = Rc::downgrade(self);
            let chat = id.clone();
            let look = rpc.watch(
                "WatchDocMessages",
                json!({ "chatId": id, "openingTail": true }),
                move |item| {
                    let Some(me) = weak.upgrade() else { return };
                    let Some(entries) = item["reset"].as_array() else { return };
                    let kind = ask_kind(entries.iter().rev());
                    me.asks.borrow_mut().insert(chat.clone(), kind);
                    // Seen: let it go (dropped once this callback returns).
                    let done = me.ask_looks.borrow_mut().remove(&chat);
                    me.publish_asks();
                    drop(done);
                },
            );
            self.ask_looks.borrow_mut().insert(id, look);
        }
        if changed {
            self.publish_asks();
        }
    }

    fn asks_value(&self) -> Value {
        Value::record(self.asks.borrow().iter().map(|(id, kind)| (id.as_str(), Value::from(*kind))))
    }

    fn publish_asks(&self) {
        let value = self.asks_value();
        for shared in self.asks_shared.borrow().iter() {
            shared.set(value.clone());
        }
    }

    fn views(&self) -> Vec<Rc<View>> {
        self.views.borrow().iter().filter_map(Weak::upgrade).collect()
    }

    /// Keeps what a view's chat had (`let_go`), for a view that opens it
    /// again: the most recent `LET_GO_MOST`, for `LET_GO_KEPT`.
    fn keep_let_go(self: &Rc<Self>, old: Option<Open>) {
        let Some(old) = old else { return };
        let mut kept = self.let_go.borrow_mut();
        // (Not while its rows are on a worker: it has only the ones they
        // left. Nor rows to be built again for a moved model's name: opened
        // again they'd say what they hold and keep the old name; any older
        // copy of it goes too.)
        if old.rebuild {
            kept.retain(|held| held.id != old.id);
            return;
        }
        if !old.heard || old.stepping {
            return;
        }
        let Open { id, entries, older_todo, built, built_model, context, context_usage, .. } = old;
        let bytes = let_go_weight(&entries, &built);
        kept.retain(|held| held.id != id);
        // (Too big on its own to keep: not kept, and what is kept stays.)
        if keeping_chats() && bytes > LET_GO_BYTES {
            return;
        }
        kept.push(LetGo { at: std::time::Instant::now(), bytes, id, entries, older_todo, built, built_model, context, context_usage });
        prune_let_go(&mut kept);
        eprintln!(
            "zeron-zgx: chat: kept chats let go of: {}, {:.1} MB",
            kept.len(),
            kept.iter().map(|held| held.bytes).sum::<usize>() as f64 / 1e6
        );
        drop(kept);
        // The app's footprint once it has come to rest after the switch (as
        // it lets a chat go it's opening the next, drawing it, its GPU
        // memory counted too: past any mark at nearly every switch): past
        // its mark then, what's kept goes.
        let turn = self.let_go_turn.get().wrapping_add(1);
        self.let_go_turn.set(turn);
        let me = self.clone();
        engine::get().spawn(async move {
            zgx::timer::sleep(LET_GO_SETTLE).await;
            if me.let_go_turn.get() == turn && !room_to_keep() && !me.let_go.borrow().is_empty() {
                me.let_go.borrow_mut().clear();
                eprintln!("zeron-zgx: chat: kept chats let go of: none (the app's footprint at rest is past its mark)");
            }
        });
    }

    /// What a view let go of lately, if it was this chat: an `Open` that has
    /// its rows.
    fn take_let_go(&self, id: &str) -> Option<Open> {
        let mut kept = self.let_go.borrow_mut();
        prune_let_go(&mut kept);
        let at = kept.iter().position(|held| held.id == id)?;
        let held = kept.remove(at);
        Some(Open {
            id: held.id,
            entries: held.entries,
            older_todo: held.older_todo,
            heard: true,
            context: held.context,
            context_usage: held.context_usage,
            built: held.built,
            built_model: held.built_model,
            has_whole: true,
            ..Open::default()
        })
    }

    fn publish_views(&self) {
        for view in self.views() {
            view.publish_session();
        }
    }

    /// The agents that can take a chat, and each one's models.
    async fn load_catalog(self: &Rc<Self>, again: bool) {
        if self.catalog_state.get() == CatalogState::Loading {
            return;
        }
        if self.catalog_state.get() == CatalogState::Ready && !again {
            return;
        }
        self.catalog_state.set(CatalogState::Loading);
        let mock = std::env::var("ZERON_ZGX_MOCK").is_ok_and(|v| v == "1");
        let list = match engine::get().call("ListHarnesses", json!({ "includeRegistry": true })).await {
            Ok(list) => list,
            Err(error) => {
                eprintln!("zeron-zgx: chat: the agents didn't list: {error}");
                self.catalog_state.set(CatalogState::Looking);
                return;
            }
        };
        let harnesses: Vec<Json> = list
            .as_array()
            .into_iter()
            .flatten()
            .filter(|h| {
                let id = h["id"].as_str().unwrap_or_default();
                if id == "mock" {
                    return mock;
                }
                h["installed"].as_bool() != Some(false) && h["enabled"].as_bool() != Some(false)
            })
            .cloned()
            .collect();
        // Each agent's models looked up at once, each shown as it comes (one
        // that has to start its agent takes seconds; one signed out fails).
        let ids: Vec<String> = harnesses
            .iter()
            .filter_map(|h| h["id"].as_str().map(Into::into))
            .collect();
        crate::agents::remember_icons(&harnesses);
        *self.harnesses.borrow_mut() = harnesses;
        if !again {
            self.models.borrow_mut().retain(|id, _| ids.contains(id));
        }
        let left = Rc::new(Cell::new(ids.len()));
        if ids.is_empty() {
            self.catalog_state.set(CatalogState::Ready);
            self.publish_catalog();
        }
        for id in ids {
            let (me, left) = (self.clone(), left.clone());
            engine::get().spawn(async move {
                match engine::get()
                    .call("ListModels", json!({ "harness": id, "force": again }))
                    .await
                {
                    Ok(list) => {
                        me.models
                            .borrow_mut()
                            .insert(id, list.as_array().cloned().unwrap_or_default());
                    }
                    Err(error) => eprintln!("zeron-zgx: chat: {id}'s models didn't list: {error}"),
                }
                left.set(left.get() - 1);
                if left.get() == 0 {
                    me.catalog_state.set(CatalogState::Ready);
                }
                me.publish_catalog();
            });
        }
    }

    /// Settings' compact transcript turned on or off (`chat.compact`): the
    /// replies being written are built again, with or without its layout.
    fn compact_shown(&self, on: bool) {
        if COMPACT_SHOWN.replace(on) == on {
            return;
        }
        for view in self.views() {
            let rebuilt = {
                let mut slot = view.open.borrow_mut();
                let Some(open) = slot.as_mut() else { continue };
                let Open { entries, built, .. } = open;
                let mut any = false;
                for e in entries.iter().filter(|e| e["status"].as_str() == Some("streaming")) {
                    any |= built.remove(&e.id).is_some();
                }
                any
            };
            if rebuilt {
                view.publish_messages();
            }
        }
    }

    fn publish_catalog(&self) {
        let value = self.catalog_value();
        for shared in self.catalog.borrow().iter() {
            shared.set(value.clone());
        }
        self.publish_views();
    }

    /// The composer now runs on `device` ("" or this one: here).
    fn catalog_on(self: &Rc<Self>, device: &str) {
        let here = self.here.borrow().clone();
        if device.is_empty() || here.as_deref() == Some(device) {
            if self.remote.borrow_mut().take().is_some() {
                self.publish_catalog();
            }
            return;
        }
        if self.remote.borrow().as_ref().is_some_and(|r| r.device == device) {
            return;
        }
        let known = self.known_remotes.borrow().get(device).cloned();
        *self.remote.borrow_mut() = Some(known.unwrap_or_else(|| Remote {
            device: device.to_owned(),
            harnesses: Vec::new(),
            models: HashMap::new(),
            ready: false,
        }));
        self.publish_catalog();
        self.reload_remote();
    }

    /// The composer's other device's catalog looked at again, a look of its
    /// own: whatever an earlier one (this device's or another's) still hears
    /// changes nothing.
    fn reload_remote(self: &Rc<Self>) {
        let Some(device) = self.remote.borrow().as_ref().map(|r| r.device.clone()) else { return };
        let turn = self.remote_turn.get() + 1;
        self.remote_turn.set(turn);
        let me = self.clone();
        let look = device.clone();
        engine::get().spawn(async move { me.load_remote(look, turn).await });
        // Unanswered by then (the device offline, or slow), it settles.
        let me = self.clone();
        engine::get().spawn(async move {
            zgx::timer::sleep(SETTLE_AFTER).await;
            me.settle_remote(&device, turn);
        });
    }

    /// The look `turn` at `device`'s catalog settles with what it has: the
    /// catalog kept from its last whole look, or nothing listed, never this
    /// device's in its place. It stands complete (the picker names what it
    /// knows, and stops waiting), and its answers still to come fill it in.
    fn settle_remote(&self, device: &str, turn: u64) {
        if !self.on_remote(device, turn) {
            return;
        }
        self.remote_settled.set(turn);
        let changed = match self.remote.borrow_mut().as_mut() {
            Some(r) if !r.ready => {
                r.ready = true;
                true
            }
            _ => false,
        };
        if changed {
            self.publish_catalog();
        }
    }

    /// Whether the look `turn` has settled (`settle_remote`).
    fn settled(&self, turn: u64) -> bool {
        self.remote_settled.get() == turn
    }

    /// Whether the look `turn` is still the composer's, on `device`.
    fn on_remote(&self, device: &str, turn: u64) -> bool {
        self.remote_turn.get() == turn && self.remote.borrow().as_ref().is_some_and(|r| r.device == device)
    }

    /// `device`'s agents, then each one's models as they come, while the
    /// composer is still on it (and this is still its look: `turn`).
    async fn load_remote(self: &Rc<Self>, device: String, turn: u64) {
        let mut tries = 0;
        let list = loop {
            let list = engine::get()
                .call("ListHarnesses", json!({ "targetDeviceId": device, "includeRegistry": true }))
                .await;
            if !self.on_remote(&device, turn) {
                return;
            }
            match list {
                Ok(list) => break list,
                Err(error) => {
                    // (Unreachable for now: its catalog settles with what it
                    // has, never this device's in its place, and it's asked
                    // again.)
                    eprintln!("zeron-zgx: chat: {device}'s agents didn't list: {error}");
                    self.settle_remote(&device, turn);
                    zgx::timer::sleep(RETRY_AFTER[tries.min(RETRY_AFTER.len() - 1)]).await;
                    tries += 1;
                    if !self.on_remote(&device, turn) {
                        return;
                    }
                }
            }
        };
        let harnesses: Vec<Json> = list.as_array().into_iter().flatten().filter(|h| offered(h, &device)).cloned().collect();
        let ids: Vec<String> = harnesses.iter().filter_map(|h| h["id"].as_str().map(Into::into)).collect();
        if let Some(r) = self.remote.borrow_mut().as_mut() {
            crate::agents::remember_icons(&harnesses);
            r.harnesses = harnesses;
            r.ready = ids.is_empty() || self.settled(turn);
        }
        self.publish_catalog();
        let left = Rc::new(Cell::new(ids.len()));
        // (Whether every agent's models came: one that didn't isn't kept as whole.)
        let whole = Rc::new(Cell::new(true));
        for id in ids {
            let (me, left, whole, device) = (self.clone(), left.clone(), whole.clone(), device.clone());
            engine::get().spawn(async move {
                let mut tries = 0;
                let list = loop {
                    let list = engine::get()
                        .call("ListModels", json!({ "harness": id, "targetDeviceId": device }))
                        .await;
                    if !me.on_remote(&device, turn) {
                        return;
                    }
                    match list {
                        Ok(list) => break Some(list),
                        Err(error) => {
                            eprintln!("zeron-zgx: chat: {id}'s models on {device} didn't list: {error}");
                            if tries == MODEL_RETRIES {
                                break None;
                            }
                            zgx::timer::sleep(RETRY_AFTER[tries]).await;
                            tries += 1;
                            if !me.on_remote(&device, turn) {
                                return;
                            }
                        }
                    }
                };
                left.set(left.get() - 1);
                {
                    let mut remote = me.remote.borrow_mut();
                    let Some(r) = remote.as_mut() else { return };
                    match list {
                        Some(list) => {
                            r.models.insert(id, list.as_array().cloned().unwrap_or_default());
                        }
                        // (What it had before, if anything, stays.)
                        None => whole.set(false),
                    }
                    let all = left.get() == 0;
                    r.ready = all || me.settled(turn);
                    if all {
                        // Kept short, it isn't ready: moving to it shows it
                        // while it's looked at again.
                        let mut kept = r.clone();
                        kept.ready = whole.get();
                        me.known_remotes.borrow_mut().insert(device.clone(), kept);
                    }
                }
                me.publish_catalog();
            });
        }
    }

    /// The other devices' catalogs looked at ahead of time, one at a time so
    /// launch doesn't feel it, so a chat on one opens with its model known.
    async fn look_ahead(self: &Rc<Self>) {
        zgx::timer::sleep(LOOK_AHEAD_AFTER).await;
        for device in crate::sessions::other_devices_online() {
            // (One kept short of an agent's models is looked at again.)
            if self.known_remotes.borrow().get(&device).is_some_and(|r| r.ready) {
                continue;
            }
            let params = json!({ "targetDeviceId": device, "includeRegistry": true });
            let Ok(list) = engine::get().call("ListHarnesses", params).await else { continue };
            let harnesses: Vec<Json> = list.as_array().into_iter().flatten().filter(|h| offered(h, &device)).cloned().collect();
            let mut models = HashMap::new();
            let mut whole = true;
            for id in harnesses.iter().filter_map(|h| h["id"].as_str()) {
                let params = json!({ "harness": id, "targetDeviceId": device });
                match engine::get().call("ListModels", params).await {
                    Ok(list) => {
                        models.insert(id.to_owned(), list.as_array().cloned().unwrap_or_default());
                    }
                    Err(_) => whole = false,
                }
            }
            crate::agents::remember_icons(&harnesses);
            // (One opened meanwhile has its own, as new.)
            keep_known(&mut self.known_remotes.borrow_mut(), Remote { device, harnesses, models, ready: whole });
        }
    }

    fn catalog_value(&self) -> Value {
        if let Some(r) = self.remote.borrow().as_ref() {
            return catalog_of(&r.harnesses, &r.models, r.ready, Some(&r.device));
        }
        catalog_of(
            &self.harnesses.borrow(),
            &self.models.borrow(),
            self.catalog_state.get() == CatalogState::Ready,
            None,
        )
    }

    fn chat(&self, id: &str) -> Option<Json> {
        self.chats.borrow().get(id).cloned()
    }

    /// Whether opening chat `id` marks it seen: when that changes what it
    /// shows (`seen_would_change`), or its row isn't known yet.
    fn marks_seen(&self, id: &str) -> bool {
        let chats = self.chats.borrow();
        let Some(chat) = chats.get(id) else { return true };
        let session = self.sessions.borrow().get(id).cloned().unwrap_or(Json::Null);
        seen_would_change(chat, &session, chrono::Utc::now())
    }

    /// A chat this window just made (`sessions.create`), known before the
    /// engine's list says so: its first message, sent at once, goes where
    /// and how it was made (a side chat in its parent's folder, with its
    /// parent's agent and model). The engine's next list replaces it.
    fn made(&self, id: &str, row: Json) {
        if !self.chats.borrow().contains_key(id) {
            self.made_here.borrow_mut().push(id.to_owned());
        }
        self.chats.borrow_mut().entry(id.to_owned()).or_insert(row);
    }

    /// Whether the chat's turn runs: working or waiting on an answer, and
    /// heard from lately.
    fn running(&self, id: &str) -> bool {
        let sessions = self.sessions.borrow();
        let Some(s) = sessions.get(id) else { return false };
        let live = matches!(s["status"].as_str(), Some("working" | "awaitingInput"));
        live && !stale(&s["updatedAt"])
    }

    /// `model`'s name in `harness`'s catalog; with none, its first model's
    /// (what the agent runs when a chat names none).
    fn label_of(&self, harness: &str, model: Option<&str>) -> Option<String> {
        let find = |by: &HashMap<String, Vec<Json>>| {
            by.get(harness)?
                .iter()
                .find(|m| model.is_none() || m["id"].as_str() == model)
                .and_then(|m| m["label"].as_str().map(Into::into))
        };
        // (This device's, else the composer's other device's, else any other
        // device's looked at.)
        find(&self.models.borrow())
            .or_else(|| self.remote.borrow().as_ref().and_then(|r| find(&r.models)))
            .or_else(|| self.known_remotes.borrow().values().find_map(|r| find(&r.models)))
    }

    fn steers(&self, harness: &str) -> bool {
        self.harnesses
            .borrow()
            .iter()
            .find(|h| h["id"].as_str() == Some(harness))
            .is_some_and(|h| h["supportsSteering"].as_bool() == Some(true))
    }

    /// The device to name for a call routed to the chat's host, when that
    /// isn't this one.
    fn target(&self, chat: &Json) -> Option<String> {
        let device = chat["deviceId"].as_str()?;
        (self.here.borrow().as_deref() != Some(device)).then(|| device.to_owned())
    }
}

/// A chat just made here (`sessions.create`), as the engine's list will
/// have it: `params` is its `createChat` (`chatId`, `deviceId`, `spaceId`,
/// `config`, `cwd`, `branch`, `parentChatId`).
pub(crate) fn made(params: &Json) {
    let Some(id) = params["chatId"].as_str() else { return };
    let mut row = json!({ "id": id });
    for wire in ["deviceId", "spaceId", "config", "cwd", "branch", "parentChatId"] {
        if !params[wire].is_null() {
            row[wire] = params[wire].clone();
        }
    }
    CHATS.with(|c| c.made(id, row));
}

/// `chat.catalog`: the agents with models listed, each model as the picker
/// takes it; ready once any agent's models are in (more may follow), or all
/// have answered (`done`). (`device`: whose, when it isn't this one.)
fn catalog_of(listed: &[Json], by: &HashMap<String, Vec<Json>>, done: bool, device: Option<&str>) -> Value {
    let any = by.values().any(|l| !l.is_empty());
    let state = if any || done { "ready" } else { "looking" };
    let mut harnesses = Vec::new();
    let mut models = Vec::new();
    for h in listed {
        let id = h["id"].as_str().unwrap_or_default();
        let Some(list) = by.get(id).filter(|l| !l.is_empty()) else {
            continue;
        };
        harnesses.push(Value::record([
            ("id", Value::from(id)),
            ("label", Value::from(h["name"].as_str().unwrap_or(id))),
            ("mark", Value::from(mark(id).as_str())),
            // An agent added from the ACP registry draws its own icon.
            ("icon", h["icon"].as_str().map_or(Value::Nil, Value::from)),
        ]));
        for m in list {
            models.push(model_value(id, m));
        }
    }
    let mut fields = vec![
        ("state", Value::from(state)),
        // (Every agent's models in: till then, a model may just not be here yet.)
        ("complete", Value::Bool(done)),
        ("harnesses", Value::List(harnesses.into())),
        ("models", Value::List(models.into())),
    ];
    if let Some(device) = device {
        fields.push(("device", Value::from(device)));
    }
    Value::record(fields)
}

/// The model picker's entry for a model: its value "<harness>/<id>", its
/// reasoning (low to high) and where it starts, whether it has Fast, and
/// its other options (Devin Fusion's lead and sidekick): each its id, its
/// name, its choices (id and name) and the one it starts on.
fn model_value(harness: &str, m: &Json) -> Value {
    let id = m["id"].as_str().unwrap_or_default();
    let levels: Vec<&str> = m["reasoningLevels"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Json::as_str)
        .collect();
    let efforts: Vec<Value> = levels
        .iter()
        .map(|l| Value::record([("value", Value::from(*l)), ("label", Value::from(effort_label(l)))]))
        .collect();
    let start = ["high", "medium"]
        .into_iter()
        .find(|l| levels.contains(l))
        .or(levels.last().copied());
    let fast = fast_option(m);
    let options: Vec<Value> = m["options"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|o| {
            // (Fast is `fast`, not an option of its own.)
            let id = o["id"].as_str().unwrap_or_default();
            !id.is_empty() && fast.as_ref().is_none_or(|(key, _, _)| key != id)
        })
        .filter_map(|o| {
            let id = o["id"].as_str()?;
            let choices: Vec<Value> = o["choices"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|c| {
                    let cid = c["id"].as_str()?;
                    Some(Value::record([
                        ("id", Value::from(cid)),
                        ("label", Value::from(c["label"].as_str().unwrap_or(cid))),
                    ]))
                })
                .collect();
            if choices.is_empty() {
                return None;
            }
            let default = o["defaultChoice"]
                .as_str()
                .filter(|d| o["choices"].as_array().is_some_and(|l| l.iter().any(|c| c["id"].as_str() == Some(d))))
                .map(str::to_owned)
                .or_else(|| o["choices"][0]["id"].as_str().map(str::to_owned))?;
            Some(Value::record([
                ("id", Value::from(id)),
                ("label", Value::from(o["label"].as_str().unwrap_or(id))),
                ("default", Value::from(default.as_str())),
                ("choices", Value::List(choices.into())),
            ]))
        })
        .collect();
    let mut fields = vec![
        ("value", Value::from(format!("{harness}/{id}").as_str())),
        ("label", Value::from(m["label"].as_str().unwrap_or(id))),
        ("harness", Value::from(harness)),
        ("fast", Value::Bool(fast.is_some())),
    ];
    if !efforts.is_empty() {
        fields.push(("efforts", Value::List(efforts.into())));
    }
    if let Some(start) = start {
        fields.push(("effort", Value::from(start)));
    }
    if !options.is_empty() {
        fields.push(("options", Value::List(options.into())));
    }
    // The agent's own default (Claude lists best-first, not default-first).
    if m["default"].as_bool() == Some(true) {
        fields.push(("default", Value::Bool(true)));
    }
    Value::record(fields)
}

/// A model's Fast option: its id, the choice that turns it on, and the one
/// that turns it off (the shipping app's `fast_mode_values`: Claude's
/// `fastMode` on/off, Codex's `serviceTier` fast/default, Devin's `speed`
/// fast/standard).
fn fast_option(m: &Json) -> Option<(String, String, String)> {
    m["options"].as_array()?.iter().find_map(|o| {
        let id = o["id"].as_str()?;
        let choices: Vec<&str> = o["choices"].as_array()?.iter().filter_map(|c| c["id"].as_str()).collect();
        let has = |c: &str| choices.contains(&c);
        let on = if matches!(id, "fastMode" | "fast_mode") && has("on") {
            "on"
        } else if id == "fast" && has("true") {
            "true"
        } else if has("fast") {
            "fast"
        } else {
            return None;
        };
        let default = o["defaultChoice"].as_str().filter(|d| *d != on && has(d));
        let off = default.or_else(|| choices.iter().copied().find(|c| *c != on))?;
        Some((id.to_owned(), on.to_owned(), off.to_owned()))
    })
}

/// Whether a chat's model options have Fast on.
fn fast_in(options: &Json) -> bool {
    options["fastMode"].as_str() == Some("on")
        || options["fast_mode"].as_str() == Some("on")
        || options["fast"].as_str() == Some("true")
        || options["serviceTier"].as_str() == Some("fast")
        || options["speed"].as_str() == Some("fast")
}

/// A chat's model options as the composer's picks: each option's chosen
/// choice, by id (strings only; Fast's is `fast`, read by `fast_in`).
fn options_value(options: &Json) -> Value {
    let fields: Vec<(&str, Value)> = options
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(k, v)| v.as_str().map(|s| (k.as_str(), Value::from(s))))
        .collect();
    Value::record(fields)
}

fn effort_label(level: &str) -> &str {
    match level {
        "minimal" => "Minimal",
        "low" => "Low",
        "medium" => "Medium",
        "high" => "High",
        "xhigh" => "Extra high",
        "max" => "Max",
        "ultra" => "Ultra",
        "ultracode" => "Ultracode",
        "ultrathink" => "Ultrathink",
        other => other,
    }
}

/// ds.Mark's name for a harness. The mock agent (development only) borrows
/// Pi's.
fn mark(id: &str) -> String {
    crate::agents::glyph(id).unwrap_or_else(|| "pi".into())
}

// -- a window's open chat -----------------------------------------------------

impl View {
    fn open_chat(self: &Rc<Self>, id: Option<String>) {
        let same = self.open.borrow().as_ref().map(|o| o.id.clone()) == id;
        if same {
            return;
        }
        let chats = CHATS.with(Rc::clone);
        let was_snapshot = self.snapshot.replace(false);
        self.generation.set(self.generation.get() + 1);
        let next = id.map(|id| {
            // A chat being made has nothing more to come yet: its rows (its
            // first message) are all there is.
            let heard = chats.creating.borrow().contains_key(&id);
            chats.take_let_go(&id).unwrap_or(Open { id, heard, ..Open::default() })
        });
        let has_rows = next.as_ref().is_some_and(|o| o.heard);
        let old = self.open.replace(next);
        if !was_snapshot {
            chats.keep_let_go(old);
        }
        if has_rows {
            self.publish_messages();
        } else {
            self.messages.set(Value::List(Rc::from([])));
            self.subagents.set(Value::List(Rc::from([])));
        }
        self.draft.set(draft_value("", ""));
        self.subscribe();
        let held = chats.held.borrow().clone();
        self.follow_held(&held);
        self.publish_messages();
        self.publish_session();
        // Its finished state seen, as the shipping app marks it: only while
        // there's something to see (opening a chat looked at already writes
        // nothing, so it doesn't send every window the chat list again).
        if let Some(id) = self.open_id().filter(|id| chats.marks_seen(id)) {
            engine::get().spawn(async move {
                let _ = engine::get().call("Mutate", json!({ "op": "markChatSeen", "chatId": id })).await;
            });
        }
    }

    /// A finished subagent's doc `id`, under `chat`: its uploaded transcript
    /// first (the shipping app's `spawn_subagent_snapshot_fetch`), its live
    /// doc if that can't be had.
    fn open_frozen(self: &Rc<Self>, id: String, chat: Option<String>) {
        if self.open_id().as_deref() == Some(id.as_str()) {
            return;
        }
        self.generation.set(self.generation.get() + 1);
        self.snapshot.set(true);
        *self.open.borrow_mut() = Some(Open { id: id.clone(), ..Open::default() });
        self.messages.set(Value::List(Rc::from([])));
        self.subagents.set(Value::List(Rc::from([])));
        self.draft.set(draft_value("", ""));
        self.publish_session();
        let generation = self.generation.get();
        let weak = Rc::downgrade(self);
        let blob = blob_ref(chat.as_deref(), &id);
        engine::get().spawn(async move {
            let reply = engine::get().call("FetchToolBlob", json!({ "blobRef": blob })).await;
            let Some(view) = weak.upgrade() else { return };
            if view.generation.get() != generation {
                return;
            }
            match reply.ok().as_ref().and_then(snapshot_frame) {
                Some(frame) => view.heard_frame(frame),
                // (Not uploaded, or not readable: its live doc.)
                None => {
                    view.snapshot.set(false);
                    view.subscribe();
                }
            }
        });
    }

    fn open_id(&self) -> Option<String> {
        self.open.borrow().as_ref().map(|o| o.id.clone())
    }

    /// The open chat's watches, on the connection there is (again on each
    /// new one).
    fn subscribe(self: &Rc<Self>) {
        if self.snapshot.get() {
            return;
        }
        let Some(rpc) = self.rpc.borrow().clone().filter(|r| !r.is_closed()) else {
            return;
        };
        let Some(id) = self.open_id() else { return };
        // (Not made yet: nothing to watch until it is, `chat.created`.)
        if CHATS.with(|c| c.creating.borrow().contains_key(&id)) {
            return;
        }
        let generation = self.generation.get();
        let weak = Rc::downgrade(self);
        // (`partPatches`: a long reply's changed parts alone, `apply`'s
        // `parts`; an engine that doesn't know it sends replies whole.)
        let mut params = json!({ "chatId": id, "openingTail": true, "partPatches": true });
        // Its rows kept (opened before): what it holds, each entry's id and
        // sum, so the engine sends what changed since (a `since` frame), not
        // the whole transcript. An engine that doesn't know it sends it whole.
        // (Not when its rows are to be built again, the model's name moved:
        // a settled entry keeps no words to build from, so it needs them all.)
        if let Some(open) = self.open.borrow().as_ref() {
            params = with_known(params, open);
        }
        let transcript = rpc.watch(
            "WatchDocMessages",
            params,
            move |item| {
                if let Some(view) = weak.upgrade()
                    && view.generation.get() == generation
                {
                    view.heard_frame(item);
                }
            },
        );
        let weak = Rc::downgrade(self);
        let queue = rpc.watch("WatchQueue", json!({ "chatId": id }), move |item| {
            if let Some(view) = weak.upgrade()
                && view.generation.get() == generation
            {
                if let Some(open) = view.open.borrow_mut().as_mut() {
                    open.queue = item["items"].as_array().cloned().unwrap_or_default();
                }
                view.publish_session();
            }
        });
        // (An engine from before it ends the watch at once: nothing waits, as before.)
        let weak = Rc::downgrade(self);
        let waiting = rpc.watch("WatchWaitingMessages", json!({ "chatId": id }), move |item| {
            if let Some(view) = weak.upgrade()
                && view.generation.get() == generation
            {
                if let Some(open) = view.open.borrow_mut().as_mut() {
                    open.waiting = item["items"].as_array().cloned().unwrap_or_default();
                }
                view.publish_session();
            }
        });
        if let Some(open) = self.open.borrow_mut().as_mut() {
            open.watches = vec![transcript, queue, waiting];
        }
    }

    /// On a new connection: the rows this view has stay, as a chat opened
    /// again with its kept rows (`has_whole`), and the engine is told what
    /// they are (`known`) to send what changed since, not them all again.
    fn hold_rows(&self) {
        if let Some(open) = self.open.borrow_mut().as_mut()
            && open.heard
            && !open.entries.is_empty()
        {
            open.has_whole = true;
        }
    }

    /// Watches the open chat again from a reset (its copy went out of step).
    fn resubscribe(self: &Rc<Self>) {
        if let Some(open) = self.open.borrow_mut().as_mut() {
            open.watches.clear();
            open.folding = None;
            open.stepping = false;
            open.builds = false;
        }
        self.generation.set(self.generation.get() + 1);
        self.subscribe();
    }

    fn heard_frame(self: &Rc<Self>, mut frame: Json) {
        let applied = 'applied: {
            let mut slot = self.open.borrow_mut();
            let Some(open) = slot.as_mut() else { return };
            if let Some(usage) = frame.get("contextUsage").filter(|u| !u.is_null()) {
                let (tokens, window) = (usage["tokens"].as_f64(), usage["window"].as_f64());
                open.context = tokens.zip(window).filter(|(_, w)| *w > 0.).map(|(t, w)| (t / w).min(1.));
                open.context_usage = (tokens, window);
            }
            // Opened with all its rows (another view's, or its own with the
            // model's name moved): not down to the newest few, until the
            // engine's whole copy comes. Meanwhile what's newest still moves:
            // the engine's opening tail is taken into the rows it has (its
            // entries, as they are now, in their places, and any new after
            // them), then each change after it, counted against the tail,
            // not these rows (`apply_counted`). Dropped until the whole copy
            // came, a turn being written stood still, its words and calls
            // arriving all at once seconds later (a long chat's whole copy).
            let mut waiting = false;
            if open.has_whole && frame["since"].as_bool() == Some(true) {
                // What changed since the rows it kept: they're the whole copy.
                open.has_whole = false;
                if std::mem::take(&mut open.rebuild) {
                    open.built.clear();
                }
            }
            if open.has_whole {
                if frame.get("reset").is_none() {
                    waiting = true;
                } else if frame["historyPending"].as_bool() == Some(true) {
                    let tail = match frame.get_mut("reset").map(Json::take) {
                        Some(Json::Array(list)) => list,
                        _ => Vec::new(),
                    };
                    for entry in tail {
                        let id = entry["id"].as_str().unwrap_or_default().to_owned();
                        open.built.remove(id.as_str());
                        match open.entries.iter_mut().find(|e| *e.id == *id) {
                            Some(held) => *held = Entry::new(entry),
                            None => open.entries.push(Entry::new(entry)),
                        }
                    }
                    open.heard = true;
                    break 'applied Ok(Change::Rows);
                } else {
                    open.has_whole = false;
                    if std::mem::take(&mut open.rebuild) {
                        open.built.clear();
                    }
                }
            }
            let before = frame.get("reset").map(|_| std::mem::take(&mut open.entries));
            if before.is_some() {
                open.folding = None;
            }
            let mut result = apply_counted(&mut open.entries, &mut frame, !waiting);
            // A `since` frame says the order the rows must now be in: held
            // out of the engine's order, they're asked for whole instead.
            if let Some(expected) = frame["orderSum"].as_u64()
                && result.is_ok()
                // (Rows without an id are left out, as `known` leaves them.)
                && order_sum(open.entries.iter().filter(|e| !e.id.is_empty()).map(|e| &*e.id)) != expected
            {
                result = Err(());
            }
            // What it changed, to build again: on a reset, the entries that
            // differ from what this view had, unless another view of this
            // chat has them built; else the ones it touched.
            if let Some(before) = before {
                keep_unchanged(&mut open.built, &before, &open.entries);
                open.older_todo = None;
                self.share_built(open);
            } else if result.is_err() {
                open.built.clear();
            } else {
                for key in ["upsert", "append", "parts"] {
                    for item in frame[key].as_array().into_iter().flatten() {
                        let id = if key == "upsert" { &item["entry"]["id"] } else { &item["entry"] };
                        if let Some(id) = id.as_str() {
                            open.built.remove(id);
                        }
                    }
                }
                for id in frame["remove"].as_array().into_iter().flatten() {
                    if let Some(id) = id.as_str() {
                        open.built.remove(id);
                    }
                }
            }
            if result.is_ok() {
                open.heard = true;
                let ids: Vec<&str> = open.entries.iter().map(|e| &*e.id).collect();
                let mut pending = self.pending.borrow_mut();
                if let Some(prompts) = pending.get_mut(&open.id) {
                    prompts.retain(|p| !ids.contains(&p.id.as_str()));
                    if prompts.is_empty() {
                        pending.remove(&open.id);
                    }
                }
            }
            result
        };
        match applied {
            Ok(Change::Quiet) => {}
            Ok(Change::Words) => self.publish_draft(),
            Ok(Change::Rows) => {
                if self.long_to_build(&frame) {
                    self.build_in_steps();
                } else if !self.open.borrow().as_ref().is_some_and(|o| o.stepping) {
                    self.publish_messages();
                }
                self.publish_session();
            }
            Err(()) => self.resubscribe(),
        }
    }

    /// What another view of this chat has built, for the entries here that
    /// are the same there: a chat in a split is watched by its pane and, when
    /// focused, by the window too.
    fn share_built(&self, open: &mut Open) {
        let chats = CHATS.with(Rc::clone);
        let let_go = chats.let_go.borrow();
        if let Some(held) = let_go.iter().find(|h| h.id == open.id) {
            reuse_built(open, &held.entries, &held.built, &held.built_model);
        }
        for view in chats.views() {
            if std::ptr::eq(&*view, self) {
                continue;
            }
            let Ok(other) = view.open.try_borrow() else { continue };
            if let Some(other) = other.as_ref().filter(|o| o.id == open.id) {
                reuse_built(open, &other.entries, &other.built, &other.built_model);
            }
        }
    }

    /// The reply being written: the last entry's words while it streams.
    fn publish_draft(&self) {
        let value = {
            let open = self.open.borrow();
            match open.as_ref().and_then(|o| o.entries.last()).and_then(|e| e.json.as_ref()) {
                Some(json) => draft_of(json),
                None => draft_value("", ""),
            }
        };
        self.draft.set(value);
    }

    /// Whether a reset left so much to build that it goes a little at a time
    /// (`build_in_steps`): the newest entries already show from the frame
    /// before it, and a long chat's whole history would hold up the window.
    fn long_to_build(&self, frame: &Json) -> bool {
        if frame.get("reset").is_none() || frame["historyPending"].as_bool() == Some(true) {
            return false;
        }
        let slot = self.open.borrow();
        let Some(open) = slot.as_ref() else { return false };
        !open.stepping
            && open.entries.iter().filter(|e| !e.id.is_empty() && !open.built.contains_key(&e.id)).count()
                > BUILT_AT_ONCE
    }

    /// Builds this chat's entries a slice at a time between frames, newest
    /// first, then publishes them. Another view of the chat that's building
    /// them is waited for, and what it built taken.
    fn build_in_steps(self: &Rc<Self>) {
        let waits = self.sibling_building();
        if let Some(open) = self.open.borrow_mut().as_mut() {
            open.stepping = true;
            open.builds = !waits;
            open.builds_started = false;
        }
        let generation = self.generation.get();
        let weak = Rc::downgrade(self);
        engine::get().spawn(async move {
            loop {
                zgx::timer::sleep(BUILD_REST).await;
                let Some(view) = weak.upgrade() else { return };
                if view.generation.get() != generation {
                    return;
                }
                if view.step() {
                    continue;
                }
                if let Some(open) = view.open.borrow_mut().as_mut() {
                    open.stepping = false;
                    open.builds = false;
                }
                view.publish_messages();
                view.publish_session();
                return;
            }
        });
    }

    /// Another view of this chat, building it now.
    fn sibling_building(&self) -> bool {
        let Some(id) = self.open_id() else { return false };
        CHATS.with(Rc::clone).views().iter().any(|view| {
            !std::ptr::eq(&**view, self)
                && view.open.try_borrow().is_ok_and(|o| o.as_ref().is_some_and(|o| o.id == id && o.builds))
        })
    }

    /// One slice of `build_in_steps`: whether there's more.
    fn step(&self) -> bool {
        if self.sibling_building() {
            return true;
        }
        let chats = CHATS.with(Rc::clone);
        let mut slot = self.open.borrow_mut();
        let Some(open) = slot.as_mut() else { return false };
        if !open.builds {
            open.builds = true;
            self.share_built(open);
        }
        let context = build_context(open, &chats);
        let whole_from = open.entries.len().saturating_sub(WHOLE_TAIL);
        let started = std::time::Instant::now();
        // The settled ones to the worker, once, with what's left of them here.
        if !open.builds_started {
            open.builds_started = true;
            let Open { entries, built, .. } = open;
            let off: Vec<(usize, String, Json)> = entries
                .iter_mut()
                .enumerate()
                .take(whole_from)
                .rev()
                .filter(|(_, e)| !e.id.is_empty() && !built.contains_key(&e.id))
                .filter_map(|(at, e)| Some((at, e.id.to_string(), e.json.take()?)))
                .collect();
            if !off.is_empty() {
                open.folding = Some(folding(off, &context));
            }
        }
        // What it has done.
        while let Some(folding) = &open.folding {
            let batch = match folding.results.try_recv() {
                Ok(batch) => batch,
                Err(std::sync::mpsc::TryRecvError::Empty) => return true,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    open.folding = None;
                    break;
                }
            };
            for folded in batch {
                settle_off(open, folded, &context);
            }
            if started.elapsed() >= BUILD_SLICE {
                return true;
            }
        }
        // The newest, whole, here; and any left settled that hasn't its row.
        let Open { entries, older_todo, built, .. } = open;
        for (at, entry) in entries.iter_mut().enumerate().rev() {
            if entry.id.is_empty() || built.contains_key(&entry.id) {
                if at < whole_from && entry.json.is_some() && !entry.id.is_empty() {
                    settle(entry, at, older_todo);
                }
                continue;
            }
            let Some(json) = &entry.json else { continue };
            built.insert(entry.id.clone(), built_for(json, &context, at < whole_from));
            if at < whole_from {
                settle(entry, at, older_todo);
            }
            if started.elapsed() >= BUILD_SLICE {
                return true;
            }
        }
        false
    }

    fn publish_messages(&self) {
        let chats = CHATS.with(Rc::clone);
        let subagents;
        let list = {
            let mut slot = self.open.borrow_mut();
            let Some(open) = slot.as_mut() else { return };
            // (Being built a slice at a time: it publishes when that's done.)
            if open.stepping {
                return;
            }
            let context = build_context(open, &chats);
            let whole_from = open.entries.len().saturating_sub(WHOLE_TAIL);
            let Open { entries, older_todo, built, .. } = open;
            let mut list: Vec<Value> = Vec::with_capacity(entries.len());
            let mut seen = Subagents::default();
            for (at, e) in entries.iter_mut().enumerate() {
                if e.id.is_empty() {
                    list.extend(e.json.as_ref().and_then(|json| message(json, context.model.as_deref())));
                    continue;
                }
                let row = match (built.get(&e.id), &e.json) {
                    (Some(row), _) => row.clone(),
                    (None, Some(json)) => {
                        // Changed by part patches alone: its other parts are as
                        // they were built, and aren't looked at again.
                        let patched = e.patched.take();
                        let row = trusting(patched.as_ref(), || built_for(json, &context, at < whole_from));
                        built.insert(e.id.clone(), row.clone());
                        e.patched = Some(std::collections::HashSet::new());
                        row
                    }
                    (None, None) => Built::default(),
                };
                list.extend(row.row);
                seen.add(row.subagents.as_deref());
                if at < whole_from && e.json.is_some() {
                    settle(e, at, older_todo);
                }
            }
            subagents = seen.0;
            let open = &*open;
            // The newest prompt, while its reply hasn't begun: sending,
            // then working, as the shipping app's working trailer says
            // (transcript.rs `sending_bridge`): sending while the send is
            // newer than the session's turn, working once the turn starts.
            if let Some(last) = open.entries.last()
                && last["role"].as_str() == Some("user")
                && let Some(Value::Map(_)) = list.last()
            {
                let sent = last["createdAt"].as_f64().unwrap_or(0.) / 1000.;
                let session = chats.sessions.borrow().get(&open.id).cloned().unwrap_or(Json::Null);
                let turn = seconds(&session["startedAt"]).map(|t| t as f64);
                let running = chats.running(&open.id);
                let fresh = now_seconds() as f64 - sent < SEND_GRACE_SECONDS;
                // Still on its way as the engine says (a Cloud machine coming
                // up for it can take longer than the grace).
                let awaited = open.waiting.iter().any(|w| {
                    w["messageId"].as_str().is_some_and(|m| Some(m) == last["id"].as_str()) && w["failure"].is_null()
                });
                let waiting = match turn {
                    Some(turn) if running && turn + 1. >= sent => Some(("working", turn)),
                    _ if running || fresh || awaited => Some(("sending", sent)),
                    _ => None,
                };
                if let Some((waiting, since)) = waiting
                    && let Some(shown) = list.pop()
                {
                    list.push(with_fields(
                        shown,
                        [("waiting", Value::from(waiting)), ("since", Value::Num(since))],
                    ));
                }
            }
            if let Some(pending) = self.pending.borrow().get(&open.id) {
                list.extend(pending.iter().map(Pending::value));
            }
            list
        };
        // The list first: a reply that ended takes its words into its row
        // before the draft lets them go.
        self.subagents.set(Value::List(subagents.into()));
        self.messages.set(Value::List(list.into()));
        self.publish_draft();
        self.after_model_moved();
    }

    /// The whole copy asked for again, if the model moved (`Open::model_moved`):
    /// what shows stays until it has all come.
    fn after_model_moved(self: &View) {
        let Some(me) = CHATS.with(Rc::clone).views().into_iter().find(|v| std::ptr::eq(&**v, self)) else { return };
        let moved = me.open.borrow_mut().as_mut().is_some_and(|open| {
            let moved = std::mem::take(&mut open.model_moved);
            if moved && !me.snapshot.get() {
                open.has_whole = true;
                open.rebuild = true;
            }
            moved
        });
        if moved && !me.snapshot.get() {
            me.resubscribe();
        }
    }

    fn publish_session(&self) {
        let chats = CHATS.with(Rc::clone);
        let value = {
            let open = self.open.borrow();
            let no_connection = self.rpc.borrow().as_ref().is_none_or(Rpc::is_closed);
            let offline = chat_offline(no_connection, engine::get().unreachable());
            match open.as_ref() {
                None => Value::Nil,
                Some(open) => session_value(&chats, open, offline),
            }
        };
        self.session.set(value);
    }

    async fn send(self: &Rc<Self>, req: Value) -> Result<(), Outcome> {
        let chats = CHATS.with(Rc::clone);
        let id = req.field("session").map(text).filter(|s| !s.is_empty()).ok_or("missing")?;
        let fresh = req.field("fresh").is_some_and(Value::truthy);
        let chat = chats.chat(&id).unwrap_or(Json::Null);
        if chat.is_null() && !fresh {
            return Err("missing");
        }
        let target = chats.target(&chat);
        let words = req.field("text").map(text).unwrap_or_default();
        // Its id: the page's (it shows the prompt at once) or a new one.
        let message_id = req
            .field("message_id")
            .map(text)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(new_id);
        // Shown until the transcript has it (the page's own copy kept in
        // step as the list is published again meanwhile), sending.
        let mut pending = self.pending.borrow_mut();
        let prompts = pending.entry(id.clone()).or_default();
        // (Shown already while its chat was being made: `chat.creating`.)
        prompts.retain(|p| p.id != message_id);
        prompts.push(Pending {
            id: message_id.clone(),
            words: words.clone(),
            paths: local_paths(&req),
            waiting: "sending",
            retry: None,
            at: stamp(now_seconds()),
            held: false,
            row: false,
            transfers: Vec::new(),
        });
        drop(pending);
        self.publish_messages();
        // To a chat Cloud is still making: after its first message, which
        // goes once it's made (`chat.send`'s flush).
        if !fresh && chats.creating.borrow().contains_key(&id) {
            chats.after_create.borrow_mut().entry(id).or_default().push(req);
            return Ok(());
        }
        // What it carries, uploaded to the chat's host.
        let mut paths = Vec::new();
        let mut images = 0;
        // (Each file here, by what it was sent as: an Appshot's context names
        // its picture by the path the agent has.)
        let mut sent_as: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        // (Asked once, when there's a file to send: `store_takes_files`.)
        let mut through_store = None;
        let mut transfers: Vec<Json> = Vec::new();
        for a in req.field("attachments").map(Value::list).unwrap_or_default() {
            let path = a.field("path").map(text).unwrap_or_default();
            let name = a.field("name").map(text).unwrap_or_else(|| path.clone());
            if path.is_empty() {
                continue;
            }
            if a.field("kind").map(text).as_deref() == Some("image") {
                images += 1;
            }
            // A file of the session's own project (the files pane's): on its
            // computer already, so it goes by its path there.
            if a.field("on_host").is_some_and(Value::truthy) {
                paths.push(path);
                continue;
            }
            let store = match through_store {
                Some(store) => store,
                None => {
                    let store = goes_through_store(target.as_deref()).await;
                    through_store = Some(store);
                    store
                }
            };
            let uploaded = if store {
                stage(&path, &name).await.map(|(reference, transfer)| {
                    transfers.push(transfer);
                    reference
                })
            } else {
                upload(&path, &name, target.as_deref()).await
            };
            match uploaded {
                Ok(at) => {
                    sent_as.insert(path.clone(), at.clone());
                    paths.push(at)
                }
                Err(why) => {
                    // Failed before it could go: the prompt says so.
                    self.mark_failed(&id, &message_id, None);
                    return Err(why);
                }
            }
        }
        // Appshots: the apps they show, as untrusted observed content after
        // the words (src/appshots, the shipping app's `with_appshots`).
        let shots: Vec<crate::appshots::Context> = req
            .field("appshots")
            .map(Value::list)
            .unwrap_or_default()
            .iter()
            .map(|s| {
                let field = |k: &str| s.field(k).map(text).filter(|v| !v.is_empty());
                let path = field("path").unwrap_or_default();
                crate::appshots::Context {
                    app: field("app").unwrap_or_else(|| "Application".into()),
                    bundle: field("bundle"),
                    title: field("title"),
                    image: sent_as.get(&path).cloned().unwrap_or(path),
                    text: field("text").unwrap_or_default(),
                    truncated: s.field("truncated").is_some_and(Value::truthy),
                }
            })
            .collect();
        let words = crate::appshots::with_appshots(&words, &shots);
        let prompt = with_attachments(&words, &paths, images);
        if !fresh && chats.running(&id) {
            // Queued: it shows in the queue, not the transcript.
            self.forget_pending(&id, &message_id);
            self.publish_messages();
            let mut params = json!({ "chatId": id, "text": words, "holdForTurnEnd": true });
            if !paths.is_empty() {
                params["attachments"] = json!(paths);
            }
            if !transfers.is_empty() {
                params["transfers"] = json!(transfers);
            }
            return engine::get()
                .call("QueueMessage", params)
                .await
                .map(|_| ())
                .map_err(|e| engine::outcome(&e));
        }
        let config = merged_config(&chat["config"], &req);
        // A chat just made takes the config it was started with. (Never one
        // without an agent: the engine turns that away as bad params, and
        // the chat keeps the config it was made with: a side chat's, its
        // parent's.)
        if fresh && config["harness"].is_null() {
            eprintln!("zeron-zgx: chat: {id}: no agent to send with (the chat isn't known here yet)");
            self.mark_failed(&id, &message_id, None);
            return Err("missing");
        }
        if fresh {
            engine::get()
                .call("Mutate", json!({ "op": "setChatConfig", "chatId": id, "config": config }))
                .await
                .map_err(|e| {
                    eprintln!("zeron-zgx: chat: a new chat's config wasn't set: {e}");
                    self.mark_failed(&id, &message_id, None);
                    engine::outcome(&e)
                })?;
        }
        let field = |key: &str| req.field(key).map(text).filter(|s| !s.is_empty());
        let cwd = field("cwd")
            .or_else(|| chat["cwd"].as_str().map(Into::into))
            .unwrap_or_else(|| "~".into());
        let mut request = json!({
            "prompt": prompt,
            "harness": config["harness"],
            "model": config["model"],
            "reasoning": config["reasoning"],
            "modelOptions": config.get("modelOptions").cloned().unwrap_or(json!({})),
            "cwd": cwd,
            "sandbox": "workspace-write",
            "autoApprove": false,
            "resume": null,
        });
        if let Some(access) = config.get("access") {
            request["access"] = access.clone();
        }
        if let Some(plan) = config.get("plan") {
            request["plan"] = plan.clone();
        }
        if !paths.is_empty() {
            request["attachments"] = json!(paths);
        }
        if fresh && req.field("worktree").is_some_and(Value::truthy) {
            request["worktree"] = json!({
                "repoPath": cwd,
                "base": field("branch").unwrap_or_else(|| "HEAD".into()),
                "spaceId": field("project"),
            });
        }
        let run = json!({ "kind": "run", "messageId": message_id, "request": request });
        let sent = command_with(&id, run.clone(), &transfers).await;
        if sent.is_err() {
            // Not delivered: the prompt stays, failed, to send again.
            if let Some(pending) = self.pending.borrow_mut().get_mut(&id)
                && let Some(p) = pending.iter_mut().find(|p| p.id == message_id)
            {
                p.transfers = transfers;
            }
            self.mark_failed(&id, &message_id, Some(run));
        } else if !fresh && runs_otherwise(&chat["config"], &config) {
            // Sent with another agent, model or effort than the chat had (the
            // composer's pick, kept as a draft until now): the chat takes
            // them, as the shipping app's next turn does, and its icon with
            // them. (Shown at once; the engine's word follows. A config that
            // doesn't stick leaves the run as sent.)
            if let Some(c) = chats.chats.borrow_mut().get_mut(&id) {
                c["config"] = config.clone();
            }
            chats.publish_views();
            if let Err(e) = engine::get()
                .call("Mutate", json!({ "op": "setChatConfig", "chatId": id, "config": config }))
                .await
            {
                eprintln!("zeron-zgx: chat: a sent pick didn't reach the chat's config: {e}");
            }
        }
        self.publish_messages();
        sent
    }

    /// The engine's outbox changed. What's in it shows as a prompt however it
    /// was sent, even from before a restart: sending, or failed with a retry.
    /// What left it is in the chat now, or (a row) in the queue, which shows it.
    fn follow_held(&self, held: &[Json]) {
        let mut changed = false;
        {
            let mut pending = self.pending.borrow_mut();
            let message_of = |send: &Json| send["messageId"].as_str().map(str::to_owned);
            let still_held =
                |id: &str| held.iter().any(|send| send["messageId"].as_str() == Some(id));
            pending.retain(|_, prompts| {
                prompts.retain_mut(|p| {
                    if !p.held || still_held(&p.id) {
                        return true;
                    }
                    changed = true;
                    p.held = false;
                    p.waiting = "sending";
                    !p.row
                });
                !prompts.is_empty()
            });
            for send in held {
                let (Some(chat), Some(message)) = (send["chatId"].as_str(), message_of(send)) else {
                    continue;
                };
                let waiting = if send["failure"].is_string() { "failed" } else { "sending" };
                let row = send["queueRow"].as_bool() == Some(true);
                let prompts = pending.entry(chat.to_owned()).or_default();
                match prompts.iter_mut().find(|p| p.id == message) {
                    Some(p) => {
                        changed |= !p.held || p.waiting != waiting;
                        p.held = true;
                        p.row = row;
                        p.waiting = waiting;
                        p.retry = None;
                    }
                    None => {
                        let (words, paths) =
                            split_attachments(send["prompt"].as_str().unwrap_or_default());
                        prompts.push(Pending {
                            id: message,
                            words,
                            paths,
                            waiting,
                            retry: None,
                            at: stamp(send["createdAt"].as_i64().unwrap_or(0) / 1000),
                            held: true,
                            row,
                            transfers: Vec::new(),
                        });
                        changed = true;
                    }
                }
            }
        }
        if changed {
            self.publish_messages();
        }
    }

    /// A pending prompt that didn't go: failed, with the command that sends
    /// it again (none: it never got that far, and is sent again whole).
    fn mark_failed(&self, id: &str, message_id: &str, retry: Option<Json>) {
        if let Some(pending) = self.pending.borrow_mut().get_mut(id)
            && let Some(p) = pending.iter_mut().find(|p| p.id == message_id)
        {
            p.waiting = "failed";
            p.retry = retry;
        }
        self.publish_messages();
    }

    fn forget_pending(&self, id: &str, message_id: &str) {
        let mut pending = self.pending.borrow_mut();
        if let Some(prompts) = pending.get_mut(id) {
            prompts.retain(|p| p.id != message_id);
            if prompts.is_empty() {
                pending.remove(id);
            }
        }
    }

    /// A send taken out of this device's outbox (`RemoveHeldSend`, #1256): it
    /// never goes out. Its prompt shown here as on its way goes with it.
    async fn remove_held(self: &Rc<Self>, args: Value) -> Result<Value, Outcome> {
        let id = args.field("id").map(text).unwrap_or_default();
        let item = args.field("item").map(text).unwrap_or_default();
        let reply = engine::get()
            .call("RemoveHeldSend", json!({ "chatId": id, "id": item }))
            .await
            .map_err(|e| held_removal_failure(&e))?;
        let removed = reply["removed"].as_bool() == Some(true);
        if removed && let Some(message) = args.field("message").map(text).filter(|m| !m.is_empty()) {
            self.forget_pending(&id, &message);
            self.publish_messages();
        }
        Ok(Value::Bool(removed))
    }

    /// A failed prompt sent again (its run as it was), sending meanwhile.
    async fn retry(self: &Rc<Self>, args: Value) -> Result<(), Outcome> {
        let id = args.field("id").map(text).unwrap_or_default();
        let message_id = args.field("message").map(text).unwrap_or_default();
        if self.open_id().as_deref() != Some(&id) {
            return Err("missing");
        }
        let (run, transfers, held) = {
            let mut pending = self.pending.borrow_mut();
            let prompts = pending.get_mut(&id).ok_or("missing")?;
            let p = prompts.iter_mut().find(|p| p.id == message_id).ok_or("missing")?;
            p.waiting = "sending";
            (p.retry.clone(), p.transfers.clone(), p.held)
        };
        self.publish_messages();
        // In the engine's outbox: the engine has what it takes to send it.
        if held {
            return engine::get()
                .call("RetryDelivery", json!({ "chatId": id }))
                .await
                .map(|_| ())
                .map_err(|e| engine::outcome(&e));
        }
        let Some(run) = run else {
            // It never got as far as a run (an upload failed): the composer
            // has the words; the row goes.
            self.forget_pending(&id, &message_id);
            self.publish_messages();
            return Err("failed");
        };
        let sent = command_with(&id, run.clone(), &transfers).await;
        if sent.is_err() {
            self.mark_failed(&id, &message_id, Some(run));
        }
        self.publish_messages();
        sent
    }

    async fn respond(self: &Rc<Self>, args: Value) -> Result<(), Outcome> {
        let id = args.field("id").map(text).unwrap_or_default();
        let request = args.field("request").map(text).unwrap_or_default();
        let decision = args.field("decision").map(text).unwrap_or_default();
        let questions = {
            let open = self.open.borrow();
            let open = open.as_ref().filter(|o| o.id == id).ok_or("missing")?;
            asked(open.entries.iter().rev().filter_map(|e| e.json.as_ref()))
                .filter(|p| p["requestId"].as_str() == Some(&request))
                .map(|p| p["questions"].as_array().cloned().unwrap_or_default())
                .ok_or("missing")?
        };
        let answers: Vec<Json> = match decision.as_str() {
            // The engine allows once: Yes is all it takes.
            "once" | "always" => single(&questions, "Yes"),
            "deny" if is_approval(&questions) => single(&questions, "No"),
            "deny" => Vec::new(),
            _ => {
                let mut labels: Vec<String> = args
                    .field("values")
                    .map(Value::list)
                    .unwrap_or_default()
                    .iter()
                    .map(text)
                    .collect();
                let typed = args.field("text").map(text).unwrap_or_default();
                if labels.is_empty() && !typed.trim().is_empty() {
                    labels.push(typed.trim().to_owned());
                }
                // A page answered: the next one, until the last.
                let done = {
                    let mut slot = self.open.borrow_mut();
                    let open = slot.as_mut().ok_or("missing")?;
                    if open.answering.as_deref() != Some(&request) {
                        open.answering = Some(request.clone());
                        open.answers.clear();
                    }
                    let page = open.answers.len();
                    let qid = questions.get(page).and_then(|q| q["id"].as_str()).unwrap_or_default();
                    open.answers.push((qid.to_owned(), labels));
                    open.answers.len() >= questions.len()
                };
                if !done {
                    self.publish_session();
                    return Ok(());
                }
                let open = self.open.borrow();
                open.as_ref()
                    .map(|o| {
                        o.answers
                            .iter()
                            .map(|(q, labels)| json!({ "questionId": q, "labels": labels }))
                            .collect()
                    })
                    .unwrap_or_default()
            }
        };
        let result = command(
            &id,
            json!({ "kind": "respondInput", "requestId": request, "answers": answers }),
        )
        .await;
        if let Some(open) = self.open.borrow_mut().as_mut() {
            open.answers.clear();
            open.answering = None;
        }
        self.publish_session();
        result
    }

    /// Back a page in the question being answered (the shipping app's
    /// wizard `back`): its last page's answer taken off, so that page shows
    /// again; what was picked there (its labels), for the composer to show
    /// chosen. Nil on the first page.
    fn question_back(&self, args: &Value) -> Value {
        let id = args.field("id").map(text).unwrap_or_default();
        let request = args.field("request").map(text).unwrap_or_default();
        let popped = self.open.borrow_mut().as_mut().and_then(|o| o.back_a_page(&id, &request));
        let Some(labels) = popped else {
            return Value::Nil;
        };
        self.publish_session();
        Value::List(labels.iter().map(|l| Value::from(l.as_str())).collect::<Vec<_>>().into())
    }

    async fn configure(self: &Rc<Self>, args: Value) -> Result<(), Outcome> {
        let chats = CHATS.with(Rc::clone);
        let id = args.field("id").map(text).unwrap_or_default();
        let chat = chats.chat(&id).ok_or("missing")?;
        let config = merged_config(&chat["config"], &args);
        // Shown at once; the engine's word follows.
        if let Some(c) = chats.chats.borrow_mut().get_mut(&id) {
            c["config"] = config.clone();
        }
        chats.publish_views();
        engine::get()
            .call("Mutate", json!({ "op": "setChatConfig", "chatId": id, "config": config }))
            .await
            .map(|_| ())
            .map_err(|e| engine::outcome(&e))
    }

    /// A queued message's text, taken to be edited here: the lease asked
    /// for, and kept (renewed before it lapses) until the edit finishes.
    async fn begin_edit(self: &Rc<Self>, args: Value) -> Result<Value, Outcome> {
        let chats = CHATS.with(Rc::clone);
        let id = args.field("id").map(text).unwrap_or_default();
        let item = args.field("item").map(text).unwrap_or_default();
        let chat = chats.chat(&id).ok_or("missing")?;
        let mut params = json!({
            "chatId": id,
            "id": item,
            "editorDeviceId": chats.here.borrow().clone(),
            "editorInstanceId": instance_id(),
        });
        if let Some(t) = chats.target(&chat) {
            params["targetDeviceId"] = json!(t);
        }
        let answer = engine::get()
            .call("BeginQueuedMessageEdit", params)
            .await
            .map_err(|e| engine::outcome(&e))?;
        match answer["outcome"].as_str() {
            Some("acquired") => {}
            // Said as "on another device" only when it is: this device's
            // other window holding it is only "can't edit it now".
            Some("locked") if held_elsewhere(chats.here.borrow().as_deref(), answer["ownerDeviceId"].as_str().unwrap_or("")) => {
                return Err("locked");
            }
            _ => return Err("missing"),
        }
        let lease = Lease {
            item: item.clone(),
            lease: answer["leaseId"].as_str().unwrap_or_default().to_owned(),
            hash: answer["baseTextHash"].clone(),
        };
        if let Some(open) = self.open.borrow_mut().as_mut().filter(|o| o.id == id) {
            open.editing = Some(lease.clone());
        }
        self.publish_session();
        self.keep_lease(id, lease, answer["expiresAtMs"].as_f64());
        // Its files too, as the composer's chips: already on the chat's
        // host (they went with the message), so saving names them again
        // rather than sending them up a second time.
        let attachments: Vec<Value> = answer["attachments"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Json::as_str)
            .map(queued_attachment)
            .collect();
        Ok(Value::record([
            ("text", Value::from(answer["text"].as_str().unwrap_or_default())),
            ("attachments", Value::List(attachments.into())),
        ]))
    }

    /// Renews the lease halfway to each expiry, while it is still held.
    fn keep_lease(self: &Rc<Self>, chat: String, lease: Lease, expires: Option<f64>) {
        let weak = Rc::downgrade(self);
        engine::get().spawn(async move {
            let mut expires = expires;
            loop {
                let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0., |d| d.as_millis() as f64);
                let wait = expires.map_or(10_000., |at| ((at - now) / 2.).clamp(1_000., 30_000.));
                zgx::timer::sleep(std::time::Duration::from_millis(wait as u64)).await;
                let Some(me) = weak.upgrade() else { return };
                let held = me
                    .open
                    .borrow()
                    .as_ref()
                    .and_then(|o| o.editing.as_ref())
                    .is_some_and(|e| e.lease == lease.lease);
                if !held {
                    return;
                }
                let answer = engine::get()
                    .call(
                        "RenewQueuedMessageEdit",
                        json!({ "chatId": chat, "id": lease.item, "leaseId": lease.lease }),
                    )
                    .await;
                match answer {
                    Ok(a) if a["outcome"].as_str() == Some("renewed") => expires = a["expiresAtMs"].as_f64(),
                    _ => {
                        // Lost (taken elsewhere, or the message went): the
                        // composer keeps the words; saving says so.
                        return;
                    }
                }
            }
        });
    }

    /// The edit ended: its words saved over the queued message ("commit"),
    /// or the message left as it was ("cancel").
    async fn finish_edit(self: &Rc<Self>, args: Value) -> Result<(), Outcome> {
        let chats = CHATS.with(Rc::clone);
        let id = args.field("id").map(text).unwrap_or_default();
        let action = args.field("action").map(text).unwrap_or_else(|| "cancel".into());
        let lease = self
            .open
            .borrow_mut()
            .as_mut()
            .filter(|o| o.id == id)
            .and_then(|o| o.editing.take())
            .ok_or("missing")?;
        self.publish_session();
        let chat = chats.chat(&id).ok_or("missing")?;
        let mut params = json!({
            "chatId": id,
            "id": lease.item,
            "leaseId": lease.lease,
            "action": action,
        });
        let target = chats.target(&chat);
        if action == "commit" {
            params["text"] = json!(args.field("text").map(text).unwrap_or_default());
            params["expectedTextHash"] = lease.hash.clone();
            // The files it carries now (the composer's chips): the ones it
            // had by their paths on the host, any added sent up first. (None
            // given: it keeps the ones it had.)
            if let Some(list) = args.field("attachments") {
                let mut paths = Vec::new();
                for a in list.list() {
                    let path = a.field("path").map(text).unwrap_or_default();
                    if path.is_empty() {
                        continue;
                    }
                    if a.field("on_host").is_some_and(Value::truthy) {
                        paths.push(path);
                        continue;
                    }
                    let name = a.field("name").map(text).unwrap_or_else(|| path.clone());
                    paths.push(upload(&path, &name, target.as_deref()).await?);
                }
                params["attachments"] = json!(paths);
            }
        }
        if let Some(t) = &target {
            params["targetDeviceId"] = json!(t);
        }
        let answer = engine::get()
            .call("FinishQueuedMessageEdit", params)
            .await
            .map_err(|e| engine::outcome(&e))?;
        match answer["outcome"].as_str() {
            Some("committed" | "cancelled" | "discarded" | "released") => Ok(()),
            Some("conflict") => Err("conflict"),
            _ => Err("missing"),
        }
    }

    async fn queue_op(self: &Rc<Self>, args: Value) -> Result<(), Outcome> {
        let chats = CHATS.with(Rc::clone);
        let id = args.field("id").map(text).unwrap_or_default();
        let item = args.field("item").map(text).unwrap_or_default();
        let chat = chats.chat(&id).ok_or("missing")?;
        let mut params = json!({ "chatId": id, "id": item });
        let target = chats.target(&chat);
        let routed = |mut p: Json| {
            if let Some(t) = &target {
                p["targetDeviceId"] = json!(t);
            }
            p
        };
        let (method, params) = match args.field("op").map(text).as_deref() {
            Some("remove") => ("RemoveQueuedMessage", routed(params)),
            Some("send") => {
                let harness = chat["config"]["harness"].as_str().unwrap_or_default();
                let method = if chats.running(&id) && chats.steers(harness) {
                    "SteerQueuedMessageNow"
                } else {
                    "SendQueuedMessageNow"
                };
                (method, routed(params))
            }
            Some("move") => {
                let to = args.field("to").and_then(Value::as_f64).unwrap_or(1.);
                params["toIndex"] = json!((to as i64 - 1).max(0));
                ("MoveQueuedMessage", params)
            }
            _ => return Err("failed"),
        };
        engine::get()
            .call(method, params)
            .await
            .map(|_| ())
            .map_err(|e| engine::outcome(&e))
    }
}

/// A queued message's file as the composer's chip: its path on the chat's
/// host (or the `pending://` ref it travels by from another device), named
/// as it was attached (an upload's id prefix off), a picture shown as one
/// where it can be read.
fn queued_attachment(path: &str) -> Value {
    let base = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let name = match base.split_once('-') {
        Some((id, rest)) if id.len() == 8 && id.bytes().all(|b| b.is_ascii_hexdigit()) && !rest.is_empty() => rest,
        _ => base,
    };
    let lower = name.to_ascii_lowercase();
    let image = [".png", ".jpg", ".jpeg", ".gif", ".webp", ".heic", ".bmp", ".tiff"]
        .iter()
        .any(|e| lower.ends_with(e));
    let mut fields = vec![
        ("path", Value::from(path)),
        ("name", Value::from(name)),
        ("kind", Value::from(if image { "image" } else { "file" })),
        ("on_host", Value::Bool(true)),
    ];
    if image && !path.starts_with("pending://") {
        fields.push(("src", Value::from(path)));
    }
    Value::record(fields)
}

/// A command for a chat (`QueueCommand`): a run, an interrupt, an answer.
/// `StopBackgroundTask` for `{ session, task }`, routed to the chat's host.
async fn stop_task(args: Value) -> Result<Value, Outcome> {
    let id = args.field("session").map(text).unwrap_or_default();
    let task = args.field("task").map(text).unwrap_or_default();
    if id.is_empty() || task.is_empty() {
        return Err("missing");
    }
    let chats = CHATS.with(Rc::clone);
    let mut params = json!({ "chatId": id, "taskId": task });
    if let Some(t) = chats.chat(&id).and_then(|c| chats.target(&c)) {
        params["targetDeviceId"] = json!(t);
    }
    let answer = engine::get()
        .call("StopBackgroundTask", params)
        .await
        .map_err(|e| engine::outcome(&e))?;
    Ok(Value::Bool(answer["stopping"].as_bool() == Some(true)))
}

/// The person's Stop waiting for a chat (`StopWaiting`), routed to its host:
/// what its agent said it waits on no longer holds it.
async fn stop_waiting(id: String) -> Result<Value, Outcome> {
    if id.is_empty() {
        return Err("missing");
    }
    let chats = CHATS.with(Rc::clone);
    let mut params = json!({ "chatId": id });
    if let Some(t) = chats.chat(&id).and_then(|c| chats.target(&c)) {
        params["targetDeviceId"] = json!(t);
    }
    let answer = engine::get()
        .call("StopWaiting", params)
        .await
        .map_err(|e| engine::outcome(&e))?;
    Ok(Value::Bool(answer["stopped"].as_bool() == Some(true)))
}

async fn command(chat: &str, command: Json) -> Result<(), Outcome> {
    command_with(chat, command, &[]).await
}

/// [`command`], with the files the engine delivers for it.
async fn command_with(chat: &str, command: Json, transfers: &[Json]) -> Result<(), Outcome> {
    let mut params = json!({ "chatId": chat, "command": command });
    if !transfers.is_empty() {
        params["transfers"] = json!(transfers);
    }
    engine::get()
        .call("QueueCommand", params)
        .await
        .map(|_| ())
        .map_err(|e| {
            eprintln!("zeron-zgx: chat: {} for {chat} wasn't taken: {e}", command["kind"]);
            engine::outcome(&e)
        })
}

/// A file uploaded to the device the chat runs on: its path there.
/// The most a file attached may weigh (the shipping app's
/// `MAX_ATTACHMENT_BYTES`).
const MAX_ATTACHMENT_BYTES: u64 = 24 * 1024 * 1024;

/// `work` on a thread of its own, its answer awaited here (None only if it
/// panicked): a file read and encoded off the window's thread.
async fn off_thread<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (tx, mut rx) = zgx::task::latest();
    std::thread::spawn(move || {
        tx.send(work());
    });
    rx.recv().await
}

/// An engine that holds a Cloud chat's message until its files are in the edge's
/// store (crates/proto `capabilities::ATTACHMENT_STORE_V1`).
const ATTACHMENT_STORE_V1: &str = "attachment-store-v1";

/// Whether a chat's files go through the edge's store: its machine may be
/// asleep, so they can't wait on reaching it. Any other chat, or an engine
/// that doesn't know the store, uploads straight to the host.
fn store_takes_files(target: Option<&str>, capabilities: &Json) -> bool {
    target.is_some_and(|device| zeron_proto::is_cloud_device(device, ""))
        && capabilities
            .as_array()
            .is_some_and(|list| list.iter().any(|c| c.as_str() == Some(ATTACHMENT_STORE_V1)))
}

async fn goes_through_store(target: Option<&str>) -> bool {
    if !target.is_some_and(|device| zeron_proto::is_cloud_device(device, "")) {
        return false;
    }
    match engine::get().call("EngineInfo", json!({})).await {
        Ok(info) => store_takes_files(target, &info["capabilities"]),
        Err(_) => false,
    }
}

/// A file's name as a chat names it: no folders.
fn base_name(name: &str) -> &str {
    std::path::Path::new(name)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(name)
}

/// How a run names a file the engine delivers, and what tells it to.
fn stored_file(upload_id: &str, name: &str) -> (String, Json) {
    let file_name = base_name(name);
    (
        format!("pending://{upload_id}/{file_name}"),
        json!({ "uploadId": upload_id, "fileName": file_name }),
    )
}

/// A file committed to this device's engine, which takes it on from there:
/// how the prompt names it, and what the send tells the engine to deliver.
async fn stage(path: &str, name: &str) -> Result<(String, Json), Outcome> {
    let upload_id = new_id();
    upload_as(&upload_id, path, name, None).await?;
    Ok(stored_file(&upload_id, name))
}

async fn upload(path: &str, name: &str, target: Option<&str>) -> Result<String, Outcome> {
    upload_as(&new_id(), path, name, target).await
}

async fn upload_as(
    upload_id: &str,
    path: &str,
    name: &str,
    target: Option<&str>,
) -> Result<String, Outcome> {
    // Read and encoded off the window's thread, and no larger than the cap.
    let owned = path.to_owned();
    let encoded = off_thread(move || -> Result<String, Outcome> {
        let meta = std::fs::metadata(&owned).map_err(|_| "failed")?;
        if !meta.is_file() {
            return Err("failed");
        }
        if meta.len() > MAX_ATTACHMENT_BYTES {
            return Err("too_large");
        }
        let bytes = std::fs::read(&owned).map_err(|_| "failed")?;
        Ok(base64(&bytes))
    })
    .await
    .ok_or("failed")??;
    let routed = |mut p: Json| {
        if let Some(t) = target {
            p["targetDeviceId"] = json!(t);
        }
        p
    };
    let mut seq = 0;
    let mut at = 0;
    while at < encoded.len() || seq == 0 {
        let end = (at + CHUNK).min(encoded.len());
        engine::get()
            .call(
                "UploadChunk",
                routed(json!({ "uploadId": upload_id, "seq": seq, "data": &encoded[at..end] })),
            )
            .await
            .map_err(|e| engine::outcome(&e))?;
        seq += 1;
        at = end;
    }
    let file_name = base_name(name);
    let committed = engine::get()
        .call("UploadCommit", routed(json!({ "uploadId": upload_id, "fileName": file_name })))
        .await
        .map_err(|e| engine::outcome(&e))?;
    committed["path"].as_str().map(Into::into).ok_or("failed")
}

/// The chat's config with what the script changed: a model as
/// "<harness>/<model>", its effort, Fast, the access mode and plan mode.
/// Whether a run's config picks another agent, model, effort or model option
/// than the chat's.
fn runs_otherwise(chat: &Json, run: &Json) -> bool {
    ["harness", "model", "reasoning", "modelOptions"].iter().any(|k| {
        let a = chat.get(*k).filter(|v| !v.is_null());
        let b = run.get(*k).filter(|v| !v.is_null() && **v != json!({}));
        let a = a.filter(|v| **v != json!({}));
        a != b
    })
}

fn merged_config(config: &Json, args: &Value) -> Json {
    let mut c = if config.is_object() { config.clone() } else { json!({}) };
    if c.get("sandbox").is_none() {
        c["sandbox"] = json!("workspace-write");
    }
    if c.get("modelOptions").is_none() {
        c["modelOptions"] = json!({});
    }
    for key in ["harness", "model", "reasoning"] {
        if c.get(key).is_none() {
            c[key] = Json::Null;
        }
    }
    let field = |key: &str| args.field(key).map(text).filter(|s| !s.is_empty());
    if let Some(model) = field("model") {
        match model.split_once('/') {
            Some((harness, model)) => {
                c["harness"] = json!(harness);
                c["model"] = json!(model);
            }
            None => c["model"] = json!(model),
        }
    }
    if let Some(harness) = field("harness") {
        c["harness"] = json!(harness);
    }
    if let Some(effort) = field("effort") {
        c["reasoning"] = json!(effort);
    }
    if let Some(access) = field("access") {
        match access.as_str() {
            "supervised" => c["access"] = json!("supervised"),
            "edits" => c["access"] = json!("auto-accept-edits"),
            _ => {
                if let Some(map) = c.as_object_mut() {
                    map.remove("access");
                }
            }
        }
    }
    if let Some(plan) = args.field("plan") {
        if plan.truthy() {
            c["plan"] = json!(true);
        } else if let Some(map) = c.as_object_mut() {
            map.remove("plan");
        }
    }
    // The model's Fast option, as the catalog lists it (nil: it has none,
    // or the catalog isn't in yet).
    let fast_of_model = {
        let chats = CHATS.with(Rc::clone);
        let harness = c["harness"].as_str().unwrap_or_default().to_owned();
        let model = c["model"].as_str().unwrap_or_default().to_owned();
        let found = chats
            .models
            .borrow()
            .get(&harness)
            .and_then(|l| l.iter().find(|m| m["id"].as_str() == Some(&model)).cloned());
        found.and_then(|m| fast_option(&m))
    };
    // The model's other options, picked whole: each option's choice by id
    // (one at its default is left out, as the shipping app leaves it);
    // Fast's own stays as it is (`fast` below sets it).
    if let Some(Value::Map(picked)) = args.field("options") {
        let mut next = serde_json::Map::new();
        if let Some((key, _, _)) = &fast_of_model
            && let Some(v) = c["modelOptions"].get(key)
        {
            next.insert(key.clone(), v.clone());
        }
        for (key, choice) in picked.iter() {
            if let Value::Str(choice) = choice
                && !choice.is_empty()
            {
                next.insert(key.to_string(), json!(&**choice));
            }
        }
        c["modelOptions"] = Json::Object(next);
    }
    if let Some(fast) = args.field("fast")
        && let Some((key, on, off)) = fast_of_model
    {
        c["modelOptions"][key] = json!(if fast.truthy() { on } else { off });
    }
    c
}

/// The prompt with its attachments named after it, as the shipping app
/// writes it (crates/ui/src/attachments.rs).
fn with_attachments(words: &str, paths: &[String], images: usize) -> String {
    if paths.is_empty() {
        return words.to_owned();
    }
    let body = if !words.trim().is_empty() {
        words.to_owned()
    } else if images == paths.len() {
        "See the attached image(s).".into()
    } else {
        "See the attached file(s).".into()
    };
    let mut out = format!("{body}\n\nAttached images (local files — open them to view):");
    for p in paths {
        out.push_str("\n- ");
        out.push_str(p);
    }
    out
}

/// A person's words, without the attachments' trailer, and the paths it
/// named.
fn split_attachments(prompt: &str) -> (String, Vec<String>) {
    const TRAILER: &str = "\n\nAttached images (local files — open them to view):";
    let Some(at) = prompt.find(TRAILER) else {
        return (prompt.to_owned(), Vec::new());
    };
    let paths = prompt[at + TRAILER.len()..]
        .lines()
        .filter_map(|l| l.strip_prefix("- "))
        .map(|l| l.trim().to_owned())
        .collect();
    let body = &prompt[..at];
    let body = match body {
        "See the attached image(s)." | "See the attached file(s)." => "",
        other => other,
    };
    (body.to_owned(), paths)
}

/// The paths a request's attachments have on this device.
fn local_paths(req: &Value) -> Vec<String> {
    req.field("attachments")
        .map(Value::list)
        .unwrap_or_default()
        .iter()
        .filter_map(|a| a.field("path").map(text))
        .filter(|p| !p.is_empty())
        .collect()
}

/// The block the composer adds for passages attached from the conversation
/// (ds/quote.luau `with_quotes`): its header, then `- who, when: words`, a
/// further line of words set in two.
const QUOTES_HEADER: &str = "Selected from this conversation:";

/// A prompt's words apart from its attached passages (where each was said,
/// and its words), as the shipping app parts review comments from a prompt
/// (crates/ui comments.rs `extract_badge`): only a whole trailing block.
fn split_quotes(words: &str) -> (String, Vec<(String, String)>) {
    split_block(words, QUOTES_HEADER)
}

/// The block the composer adds for review comments left on a file (the
/// shipping app's comments.rs `with_comments`): its header, then `- path:line:
/// body`, a further line of a body set in two.
const COMMENTS_HEADER: &str = "Review comments (each cites the workspace file and line it belongs to):";

/// The block for comments left on a diff (the shipping app's
/// `COMMENT_BLOCK_HEADER`): each `- path:line (L|R): body`, a range
/// `path:10-14`.
const DIFF_COMMENTS_HEADER: &str = "Comments on the diff (each cites the file and line it belongs to; L = line number in the original file, R = in the changed file):";

/// A prompt's words apart from its review comments (each its file and line,
/// and its words): only a whole trailing block, as `extract_badge` takes it,
/// under either header.
fn split_comments(words: &str) -> (String, Vec<(String, String)>) {
    let (mut head, mut bullets) = split_block_raw(words, COMMENTS_HEADER);
    if bullets.is_empty() {
        (head, bullets) = split_block_raw(words, DIFF_COMMENTS_HEADER);
    }
    let comments = bullets
        .into_iter()
        .map(|bullet| {
            // "path:line (R): body" (a side's marker, the earliest), or
            // "path:line: body" (the first ": " after a line number).
            let side = [" (L): ", " (R): "].into_iter().filter_map(|m| bullet.find(m).map(|at| (at, m.len()))).min_by_key(|(at, _)| *at);
            let mut cut = None;
            for (at, _) in bullet.match_indices(": ") {
                let before = &bullet[..at];
                if before.rsplit_once(':').is_some_and(|(_, n)| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit() || b == b'-')) {
                    cut = Some((at, 2));
                    break;
                }
            }
            let cut = match (side, cut) {
                (Some(s), Some(c)) => Some(if s.0 < c.0 { s } else { c }),
                (s, c) => s.or(c),
            };
            match cut {
                Some((at, len)) => (bullet[..at].to_owned(), bullet[at + len..].to_owned()),
                None => (String::new(), bullet),
            }
        })
        .collect();
    (head, comments)
}

/// A comment chip on the sent message (the shipping app's badge): how many,
/// and where they are.
fn comments_chip(comments: &[(String, String)]) -> Value {
    let label = if comments.len() == 1 { "1 comment".to_owned() } else { format!("{} comments", comments.len()) };
    let places = comments.iter().map(|(place, _)| place.as_str()).filter(|p| !p.is_empty()).collect::<Vec<_>>().join(", ");
    Value::record([
        ("kind", Value::from("file")),
        ("icon", Value::from("chat-round-line")),
        ("name", Value::from(label.as_str())),
        ("meta", Value::from(places.as_str())),
    ])
}

/// A trailing block under `header` of `- …` bullets (continued in two):
/// the words before it and each bullet's text, joined with its continuation.
fn split_block_raw(words: &str, header: &str) -> (String, Vec<String>) {
    let marker = format!("\n\n{header}\n");
    let start = format!("{header}\n");
    let (head, block) = if let Some(at) = words.rfind(&marker) {
        (&words[..at], &words[at + marker.len()..])
    } else if let Some(rest) = words.strip_prefix(&start) {
        ("", rest)
    } else {
        return (words.to_owned(), Vec::new());
    };
    if block.is_empty() || !block.lines().all(|l| l.starts_with("- ") || l.starts_with("  ")) {
        return (words.to_owned(), Vec::new());
    }
    let mut out: Vec<String> = Vec::new();
    for line in block.lines() {
        if let Some(bullet) = line.strip_prefix("- ") {
            out.push(bullet.to_owned());
        } else if let (Some(more), Some(last)) = (line.strip_prefix("  "), out.last_mut()) {
            last.push('\n');
            last.push_str(more);
        }
    }
    (head.to_owned(), out)
}

/// A trailing block of `- place: words` bullets under `header`.
fn split_block(words: &str, header: &str) -> (String, Vec<(String, String)>) {
    let (head, bullets) = split_block_raw(words, header);
    let pairs = bullets
        .into_iter()
        .map(|bullet| match bullet.split_once(": ") {
            Some((place, body)) => (place.to_owned(), body.to_owned()),
            None => (String::new(), bullet),
        })
        .collect();
    (head, pairs)
}

/// A passage's chip on the sent message (as the composer showed it): where
/// it was said, its words on one line, cut at a word.
fn quote_chip(place: &str, body: &str) -> Value {
    let line = body.split_whitespace().collect::<Vec<_>>().join(" ");
    let excerpt = if line.chars().count() <= 72 {
        line
    } else {
        let cut: String = line.chars().take(72).collect();
        match cut.rfind(' ') {
            Some(at) => format!("{}…", &cut[..at]),
            None => format!("{cut}…"),
        }
    };
    Value::record([
        ("kind", Value::from("file")),
        ("icon", Value::from("cursor")),
        ("name", Value::from(place)),
        ("meta", Value::from(excerpt.as_str())),
    ])
}

/// The shipping app's browser annotation token (crates/proto annotation.rs):
/// `[Annotation N](zeron-annotation:<hex JSON>)`, as ds/annotation.luau
/// writes it after a prompt's words.
const ANNOTATION_SCHEME: &str = "](zeron-annotation:";

/// An element picked on a page, as a sent message shows it (the composer's
/// chip, ds/composer_cargo): its page, then its number and what it says.
struct Pick {
    index: u32,
    element: String,
    url: String,
    words: String,
}

/// A prompt's words apart from its annotation tokens (each read as its
/// label, "Annotation N", a line of nothing but a label dropped), and the
/// picks they stood for.
fn split_annotations(words: &str) -> (String, Vec<Pick>) {
    if !words.contains(ANNOTATION_SCHEME) {
        return (words.to_owned(), Vec::new());
    }
    let mut out = String::with_capacity(words.len());
    let mut picks = Vec::new();
    let mut rest = words;
    while let Some(at) = rest.find(ANNOTATION_SCHEME) {
        let head = &rest[..at];
        let Some(open) = head.rfind('[') else {
            out.push_str(&rest[..at + ANNOTATION_SCHEME.len()]);
            rest = &rest[at + ANNOTATION_SCHEME.len()..];
            continue;
        };
        let label = &head[open + 1..];
        let hex_start = at + ANNOTATION_SCHEME.len();
        let Some(close) = rest[hex_start..].find(')') else {
            out.push_str(rest);
            rest = "";
            break;
        };
        let hex = &rest[hex_start..hex_start + close];
        let pick = (hex.len() % 2 == 0)
            .then(|| {
                (0..hex.len())
                    .step_by(2)
                    .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
                    .collect::<Option<Vec<u8>>>()
            })
            .flatten()
            .and_then(|bytes| serde_json::from_slice::<Json>(&bytes).ok())
            .map(|json| {
                let text = |key: &str| json[key].as_str().unwrap_or_default().split_whitespace().collect::<Vec<_>>().join(" ");
                let words = [text("text"), text("name")].into_iter().find(|w| !w.is_empty()).unwrap_or_default();
                Pick {
                    index: json["index"].as_u64().unwrap_or(0) as u32,
                    element: json["element"].as_str().unwrap_or_default().to_owned(),
                    url: json["url"].as_str().unwrap_or_default().to_owned(),
                    words,
                }
            });
        match pick {
            Some(pick) => {
                out.push_str(&head[..open]);
                out.push_str(label);
                picks.push(pick);
            }
            None => out.push_str(&rest[..hex_start + close + 1]),
        }
        rest = &rest[hex_start + close + 1..];
    }
    out.push_str(rest);
    // A line that was only a token says nothing now.
    let kept: Vec<&str> = out
        .lines()
        .filter(|l| !(l.starts_with("Annotation ") && l[11..].trim().chars().all(|c| c.is_ascii_digit())))
        .collect();
    let mut body = kept.join("\n");
    while body.ends_with('\n') {
        body.pop();
    }
    (body, picks)
}

/// A page's address as a chip names it: no scheme, no "www.", no lone
/// trailing slash (ds/composer_cargo's `page_of`).
fn page_of(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let rest = rest.strip_prefix("www.").unwrap_or(rest);
    match rest.strip_suffix('/') {
        Some(host) if !host.contains('/') => host.to_owned(),
        _ => rest.to_owned(),
    }
}

/// The chip a sent pick shows, as the composer's did: its page over its
/// number and what it says (else its element).
fn annotation_chip(pick: &Pick) -> Value {
    let label = format!("Annotation {}", pick.index);
    let page = page_of(&pick.url);
    let what = if pick.words.is_empty() { pick.element.as_str() } else { pick.words.as_str() };
    let (name, meta) = if page.is_empty() {
        (label, pick.element.clone())
    } else {
        (page, format!("{label} · {what}"))
    };
    Value::record([
        ("kind", Value::from("file")),
        ("icon", Value::from("cursor")),
        ("name", Value::from(name.as_str())),
        ("meta", Value::from(meta.as_str())),
    ])
}

/// The block the composer adds for other chats attached (screens/chat/
/// carried.luau `with_chats`): its header, then `- [title](zeron-chat:id)`.
/// The links stay in the prompt (the engine reads each out to the agent);
/// the sent message shows each as a card, as the composer did.
const CHATS_HEADER: &str = "Chats attached:";

/// A prompt's words apart from the chats attached to it (each its title and
/// id): only a whole trailing block of chat links, as the other blocks.
fn split_chats(words: &str) -> (String, Vec<(String, String)>) {
    let (head, bullets) = split_block_raw(words, CHATS_HEADER);
    if bullets.is_empty() {
        return (words.to_owned(), Vec::new());
    }
    let mut chats = Vec::new();
    for bullet in &bullets {
        let links = zeron_proto::chat_mentions::chat_mention_links(bullet);
        // Each line one canonical link and nothing else, or it's words.
        match links.as_slice() {
            [link] if link.range == (0..bullet.len()) => chats.push((link.title.clone(), link.chat_id.clone())),
            _ => return (words.to_owned(), Vec::new()),
        }
    }
    (head, chats)
}

/// An attached chat's card on the sent message: its title, its id (a press
/// opens it) and its agent's mark, when this window knows the chat.
fn chat_chip(title: &str, id: &str) -> Value {
    let harness = CHATS
        .try_with(|chats| {
            chats
                .chats
                .try_borrow()
                .ok()
                .and_then(|known| known.get(id).and_then(|chat| chat["config"]["harness"].as_str().map(str::to_owned)))
        })
        .ok()
        .flatten();
    let mut fields = vec![
        ("kind", Value::from("chat")),
        ("name", Value::from(title)),
        ("chat", Value::from(id)),
    ];
    if let Some(harness) = harness.filter(|h| !h.is_empty()) {
        fields.push(("mark", Value::from(mark(&harness).as_str())));
    }
    Value::record(fields)
}

/// The chips a sent prompt carries: its files and pictures, then its
/// passages, then the elements picked on a page, then the chats attached.
fn carried_value(
    paths: &[String],
    quotes: &[(String, String)],
    picks: &[Pick],
    comments: &[(String, String)],
    chats: &[(String, String)],
) -> Value {
    let Value::List(files) = attachments_value(paths) else { unreachable!() };
    let mut all: Vec<Value> = files.iter().cloned().collect();
    all.extend(quotes.iter().map(|(place, body)| quote_chip(place, body)));
    all.extend(picks.iter().map(annotation_chip));
    if !comments.is_empty() {
        all.push(comments_chip(comments));
    }
    all.extend(chats.iter().map(|(title, id)| chat_chip(title, id)));
    Value::List(all.into())
}

fn attachments_value(paths: &[String]) -> Value {
    Value::List(
        paths
            .iter()
            .map(|p| {
                let name = std::path::Path::new(p)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(p);
                let image = is_image(p);
                let mut fields = vec![
                    ("kind", Value::from(if image { "image" } else { "file" })),
                    ("name", Value::from(name)),
                ];
                if image {
                    fields.push(("src", Value::from(p.as_str())));
                }
                Value::record(fields)
            })
            .collect::<Vec<_>>()
            .into(),
    )
}

/// A picture the window can draw (zgx decodes no GIF or HEIC yet: those go
/// as files, so a thumbnail never fails).
fn is_image(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    [".png", ".jpg", ".jpeg", ".webp"]
        .iter()
        .any(|e| lower.ends_with(e))
}

// -- the transcript -----------------------------------------------------------

/// What a frame changed: only words of a reply being written, rows, or
/// nothing that shows (more of a tool's output, which no row shows).
enum Change {
    Quiet,
    Words,
    Rows,
}

/// The model's name a chat's replies carry, building them again if it's
/// another now, and the device its visuals are read from (the chat's host,
/// when it isn't this one).
fn build_context(open: &mut Open, chats: &Chats) -> Rc<RowContext> {
    let chat = chats.chat(&open.id).unwrap_or(Json::Null);
    let harness = chat["config"]["harness"].as_str().unwrap_or("claude-code").to_owned();
    let model = chat["config"]["model"].as_str().map(|m| chats.label_of(&harness, Some(m)).unwrap_or_else(|| m.to_owned()));
    if open.built_model.as_ref() != Some(&model) {
        // (Settled entries are the engine's now: the rows show the last name
        // until its copy comes, `model_moved`, and are built again from that.)
        if open.entries.iter().all(|e| e.json.is_some()) {
            open.built.clear();
        } else {
            open.model_moved = true;
        }
        open.built_model = Some(model.clone());
    }
    let device = chats.target(&chat);
    match &open.row_context {
        Some(context) if context.doc == open.id && context.device == device && context.model == model => context.clone(),
        _ => {
            let context = Rc::new(RowContext { doc: open.id.clone(), device, model });
            open.row_context = Some(context.clone());
            context
        }
    }
}

/// An entry as its row reads it.
pub(crate) fn built_entry(
    entry: &Json,
    doc: &str,
    device: Option<&str>,
    model: Option<&str>,
) -> Option<Value> {
    // (One still being written is built again with its next change.)
    let settled = entry["status"].as_str() != Some("streaming");
    if settled {
        ROW_BUILD.set(Some(RowBuild::default()));
    } else {
        LIVE_PASS.set(Some(Default::default()));
        // Changed in its last group's calls alone, as it was last built:
        // only those built again (`LiveRow`).
        if let Some(row) = live_row_patched(entry, model) {
            LIVE_PASS.set(None);
            return Some(row);
        }
    }
    let mut row = match with_host(entry, doc, device) {
        Some(entry) => message(&entry, model),
        None => message(entry, model),
    };
    if !settled && let Some(row) = &row {
        live_row_keep(entry, row, model);
    }
    LIVE_PASS.set(None);
    if let Some(mut build) = ROW_BUILD.take()
        && let Some(row) = &mut row
    {
        share_parts(row, &mut build.seen);
    }
    row
}

/// A reply being written, as it was last built (`built_entry`), for a change
/// to the calls of its last group alone (a call arriving, one finishing, by
/// part patches): its row made again from the parts that changed, not from
/// all of them. What it keeps is what the whole build makes of the parts no
/// patch touched: those are trusted to be as they were (`PATCHED`).
struct LiveRow {
    /// Its parts' ids, in order, and where each is.
    ids: Vec<Rc<str>>,
    at: rustc_hash::FxHashMap<Rc<str>, usize>,
    /// Where its last group starts: every part from there is a plain call
    /// or thought (`plain_part`), all of them the group.
    tail: usize,
    /// What each part of the group adds to its line, and their sum.
    tallies: Vec<Tally>,
    group: GroupTally,
    /// Its row's `tools`, those of the last group at their end, in order.
    tools: Vec<Value>,
    /// Its blocks before the last group's.
    head: Vec<Value>,
    text: Value,
    started: f64,
    /// Its last text part, if any (before the last group: none is in it), as
    /// `draft_of` looks for it.
    last_text: Option<usize>,
}
thread_local! {
    /// Replies being written (`LiveRow`), by id; a few at once.
    static LIVE_ROWS: RefCell<rustc_hash::FxHashMap<String, LiveRow>> = RefCell::default();
    /// While a row is built whole for a test (`built_whole`): `LIVE_ROWS`
    /// neither read nor kept.
    static LIVE_ROWS_OFF: Cell<bool> = const { Cell::new(false) };
    /// Rows made from their kept ones (`live_row_patched`), for tests.
    static LIVE_ROW_HITS: Cell<u64> = const { Cell::new(0) };
}

/// `build`, its rows built whole: `LIVE_ROWS` untouched (a test's reference).
#[cfg(test)]
pub(crate) fn built_whole<T>(build: impl FnOnce() -> T) -> T {
    LIVE_ROWS_OFF.set(true);
    let made = build();
    LIVE_ROWS_OFF.set(false);
    made
}

/// How many rows were made from their kept ones so far (`live_row_patched`).
#[cfg(test)]
pub(crate) fn live_row_hits() -> u64 {
    LIVE_ROW_HITS.get()
}
/// The most replies `LIVE_ROWS` keeps before it starts again.
const LIVE_ROWS_MOST: usize = 16;

/// A part a last group's change may be made from alone: a call that isn't a
/// display's nor a subagent's, nor cut short (`with_host`), or a thought
/// with words. Each is a row of the group and of `tools`, in its place.
fn plain_part(p: &Json) -> bool {
    match p["kind"].as_str() {
        Some("tool") => {
            !display_tool(p) && !is_agent(p) && p["outputTruncated"].as_bool() != Some(true)
        }
        Some("reasoning") => !p["text"].as_str().unwrap_or_default().trim().is_empty(),
        _ => false,
    }
}

/// The last group's block, from its parts' tools (`tools`, the group's at the
/// end) and its tally: as `blocks_of` makes it while the reply is written.
fn live_group_block(
    parts: &[Json],
    tail: usize,
    tools: &[Value],
    group: &GroupTally,
) -> Option<Value> {
    let n = parts.len() - tail;
    let mut shown: Vec<Value> = tools[tools.len() - n..].to_vec();
    // (The part being written: its own value.)
    let last = shown.last_mut()?;
    *last = tool_live(parts.last()?, true)?;
    let first = parts[tail]["id"].as_str().unwrap_or_default();
    Some(Value::record([
        ("id", Value::from(format!("g:{first}").as_str())),
        ("kind", Value::from("tools")),
        ("summary", Value::from(group.text().as_str())),
        ("live", Value::Bool(true)),
        ("tools", Value::List(shown.into())),
    ]))
}

/// A reply's row as `message` makes it while it's written, from its parts
/// (`text`, `tools` and `blocks` given).
fn live_row_value(
    entry: &Json,
    text: Value,
    tools: Vec<Value>,
    blocks: Vec<Value>,
    started: f64,
    model: Option<&str>,
) -> Value {
    let mut fields = vec![
        ("id", Value::from(entry["id"].as_str().unwrap_or_default())),
        ("role", Value::from("assistant")),
        ("text", text),
        ("tools", Value::List(tools.into())),
        ("blocks", Value::List(blocks.into())),
        ("streaming", Value::Bool(true)),
        ("started", Value::Num(started)),
    ];
    if let Some(model) = model {
        fields.push(("model", Value::from(model)));
    }
    if let Some(ms) = entry["durationMs"].as_f64() {
        fields.push(("worked", Value::Num((ms / 1000.).round().max(1.))));
    }
    Value::record(fields)
}

/// Keep `row`, just built in whole from `entry` (a reply being written), for
/// its next change (`live_row_patched`), if its last group is made of plain
/// parts and making it from them gives the very block the build did.
fn live_row_keep(entry: &Json, row: &Value, model: Option<&str>) {
    let Some(id) = entry["id"].as_str() else {
        return;
    };
    if LIVE_ROWS_OFF.get() {
        return;
    }
    LIVE_ROWS.with_borrow_mut(|rows| {
        rows.remove(id);
    });
    // (Laid out compact as well, it's built in whole each time.)
    if COMPACT_SHOWN.get() || entry["role"].as_str() != Some("assistant") {
        return;
    }
    let Some(parts) = entry["parts"].as_array() else {
        return;
    };
    let tail = parts
        .iter()
        .rposition(|p| !plain_part(p))
        .map_or(0, |at| at + 1);
    if tail == parts.len() {
        return;
    }
    let (Some(Value::List(tools)), Some(Value::List(blocks))) =
        (row.field("tools").cloned(), row.field("blocks").cloned())
    else {
        return;
    };
    let n = parts.len() - tail;
    if tools.len() < n || blocks.is_empty() {
        return;
    }
    let tallies: Vec<Tally> = parts[tail..].iter().map(Tally::of).collect();
    let mut group = GroupTally::default();
    for t in &tallies {
        group.add(t);
    }
    // The group as the parts make it, against what the build made: anything
    // the build saw that this doesn't (a display call before the run, joining
    // it) and it isn't kept.
    let made = live_group_block(parts, tail, &tools, &group);
    if made.as_ref() != blocks.last() {
        return;
    }
    let ids: Vec<Rc<str>> = parts
        .iter()
        .map(|p| Rc::from(p["id"].as_str().unwrap_or_default()))
        .collect();
    let at = ids
        .iter()
        .enumerate()
        .map(|(i, id)| (id.clone(), i))
        .collect();
    let kept = LiveRow {
        ids,
        at,
        tail,
        tallies,
        group,
        tools: tools.to_vec(),
        head: blocks[..blocks.len() - 1].to_vec(),
        text: row.field("text").cloned().unwrap_or(Value::Nil),
        started: row
            .field("started")
            .and_then(|v| {
                if let Value::Num(n) = v {
                    Some(*n)
                } else {
                    None
                }
            })
            .unwrap_or(0.),
        last_text: parts[..tail]
            .iter()
            .rposition(|p| p["kind"].as_str() == Some("text")),
    };
    let _ = model;
    LIVE_ROWS.with_borrow_mut(|rows| {
        if rows.len() >= LIVE_ROWS_MOST {
            rows.clear();
        }
        rows.insert(id.to_owned(), kept);
    });
}

/// `entry`'s row from its kept one (`LiveRow`), if only part patches changed
/// it since and only in its last group: plain parts changed in it or added at
/// its end. `None` for anything else, built in whole.
fn live_row_patched(entry: &Json, model: Option<&str>) -> Option<Value> {
    if COMPACT_SHOWN.get() || LIVE_ROWS_OFF.get() {
        return None;
    }
    let id = entry["id"].as_str()?;
    let parts = entry["parts"].as_array()?;
    let patched: Vec<String> =
        PATCHED.with_borrow(|p| p.as_ref().map(|p| p.iter().cloned().collect()))?;
    LIVE_ROWS.with_borrow_mut(|rows| {
        let row = rows.get_mut(id)?;
        let was = row.ids.len();
        if parts.len() < was {
            return None;
        }
        // Each part patched: in the last group or after it, a plain one, the
        // one that was there; every part added, patched.
        let mut changed: Vec<usize> = Vec::with_capacity(patched.len());
        for pid in &patched {
            let at = match row.at.get(pid.as_str()) {
                Some(&at) => at,
                None => {
                    was + parts[was..]
                        .iter()
                        .position(|p| p["id"].as_str() == Some(pid.as_str()))?
                }
            };
            if at < row.tail
                || parts[at]["id"].as_str() != Some(pid.as_str())
                || !plain_part(&parts[at])
            {
                return None;
            }
            changed.push(at);
        }
        if (was..parts.len()).any(|at| !changed.contains(&at)) {
            return None;
        }
        changed.sort_unstable();
        let first_tool = row.tools.len() - (was - row.tail);
        // The part written before (its row no longer the one being written).
        for &at in &changed {
            let tally = Tally::of(&parts[at]);
            let value = tool_live(&parts[at], false)?;
            let j = at - row.tail;
            if at < was {
                row.group.remove(&row.tallies[j]);
                row.tallies[j] = tally.clone();
                row.tools[first_tool + j] = value;
            } else {
                let pid: Rc<str> = Rc::from(parts[at]["id"].as_str().unwrap_or_default());
                row.at.insert(pid.clone(), at);
                row.ids.push(pid);
                row.tallies.push(tally.clone());
                row.tools.push(value);
            }
            row.group.add(&tally);
        }
        let block = live_group_block(parts, row.tail, &row.tools, &row.group)?;
        let mut blocks = row.head.clone();
        blocks.push(block);
        LIVE_ROW_HITS.set(LIVE_ROW_HITS.get() + 1);
        Some(live_row_value(
            entry,
            row.text.clone(),
            row.tools.clone(),
            blocks,
            row.started,
            model,
        ))
    })
}

/// The newest plan in these entries, newest first.
fn latest_todo<'a>(newest_first: impl Iterator<Item = &'a Json>) -> Option<&'a Json> {
    newest_first
        .flat_map(|e| e["parts"].as_array().into_iter().flatten().rev())
        .find(|p| p["kind"].as_str() == Some("tool") && p["call"]["kind"].as_str() == Some("todo"))
}

/// What the worker made of an entry, kept: its row folded, as `built_for` and
/// `settle` would have left it.
fn settle_off(open: &mut Open, folded: FoldedOff, context: &Rc<RowContext>) {
    let FoldedOff { at, id, bytes, subagents, sum, todo } = folded;
    // (Where it stood, unless a frame has moved it. Gone: nothing to keep.)
    let Open { entries, built, older_todo, .. } = open;
    let found = entries.get(at).filter(|e| *e.id == *id).map(|_| at).or_else(|| entries.iter().position(|e| *e.id == *id));
    let Some(place) = found else { return };
    let entry = &mut entries[place];
    // (A newer copy came meanwhile: it's built from that.)
    if entry.json.is_some() {
        return;
    }
    entry.sum = sum;
    if let Some(part) = todo
        && older_todo.as_ref().is_none_or(|(kept, _)| *kept < place)
    {
        *older_todo = Some((place, part));
    }
    let folded = bytes.as_ref().map_or(0, |bytes| bytes.len());
    let row = bytes.map(|bytes| Value::folded(entry.id.clone(), FoldedEntry { bytes, context: context.clone() }));
    let subagents = (!subagents.is_empty()).then(|| subagents.iter().map(SubagentOff::value).collect());
    built.insert(entry.id.clone(), Built { row, subagents, folded });
}

/// `entry`, built, down to its id and sum; a plan in it is kept (`todos`).
fn settle(entry: &mut Entry, at: usize, older_todo: &mut Option<(usize, Json)>) {
    let Some(json) = entry.json.take() else { return };
    entry.sum = sum_of(&json);
    if older_todo.as_ref().is_none_or(|(place, _)| *place < at)
        && let Some(part) = latest_todo(std::iter::once(&json))
    {
        *older_todo = Some((at, part.clone()));
    }
}

/// A row's own repeats, one each: a reply holds its tools in `tools`, in
/// `blocks` and in `compact`, its words in `text`, `copy` and a block, and
/// every record its keys. Only one of each stays, shared.
#[derive(Default)]
struct Seen {
    words: std::collections::HashSet<Rc<str>>,
    records: HashMap<Rc<str>, Value>,
}

/// A short word is kept for every row, while there are few of them.
const COMMON_LEN: usize = 16;
const COMMON_MOST: usize = 4096;
thread_local! {
    static COMMON: RefCell<std::collections::HashSet<Rc<str>>> = RefCell::default();
    /// The settled row being built (`built_entry`): its tools so far, by the
    /// part each is of, each built once however many of its layouts show it.
    static ROW_BUILD: RefCell<Option<RowBuild>> = const { RefCell::new(None) };
    /// The calls of replies being written, each as last built, by its id and
    /// whether it was the part being written: its part as it was, and its
    /// value (`tool_live`).
    static LIVE_TOOLS: RefCell<rustc_hash::FxHashMap<String, [Option<(Json, Option<Value>)>; 2]>> = RefCell::default();
    /// The reply being written as it's built (`built_entry`): each of its
    /// parts' tool by where the part is, as `LIVE_TOOLS` gave it, so the
    /// layouts that show a part (its tools, its blocks, compact) look it up
    /// once.
    static LIVE_PASS: RefCell<Option<rustc_hash::FxHashMap<(*const Json, bool), Option<Value>>>> = const { RefCell::new(None) };
    /// While an entry changed by part patches alone is built (`trusting`):
    /// the parts that changed. Every other part is as `LIVE_TOOLS` last built
    /// it, its value taken without looking at the part again.
    static PATCHED: RefCell<Option<rustc_hash::FxHashSet<String>>> = const { RefCell::new(None) };
    /// Settings' compact transcript is on (`chat.compact`).
    static COMPACT_SHOWN: Cell<bool> = const { Cell::new(false) };
}

/// `build`, with `patched` (the parts changed since the entry's row was last
/// built, if only those) trusted to be all that changed.
pub(crate) fn trusting<T>(patched: Option<&std::collections::HashSet<String>>, build: impl FnOnce() -> T) -> T {
    PATCHED.set(patched.map(|patched| patched.iter().cloned().collect()));
    let made = build();
    PATCHED.set(None);
    made
}

/// The most calls `LIVE_TOOLS` keeps before it starts again.
const LIVE_TOOLS_MOST: usize = 8192;

#[derive(Default)]
struct RowBuild {
    seen: Seen,
    tools: HashMap<*const Json, Option<Value>>,
}

fn share_word(text: &mut Rc<str>, own: &mut std::collections::HashSet<Rc<str>>) {
    if text.len() <= COMMON_LEN {
        let shared = COMMON.with_borrow_mut(|common| {
            if let Some(known) = common.get(&**text) {
                return Some(known.clone());
            }
            (common.len() < COMMON_MOST).then(|| {
                common.insert(text.clone());
                text.clone()
            })
        });
        if let Some(shared) = shared {
            *text = shared;
            return;
        }
    }
    match own.get(&**text) {
        Some(known) => *text = known.clone(),
        None => {
            own.insert(text.clone());
        }
    }
}

fn share_parts(value: &mut Value, seen: &mut Seen) {
    match value {
        Value::Str(text) => share_word(text, &mut seen.words),
        Value::List(items) => {
            if let Some(items) = Rc::get_mut(items) {
                items.iter_mut().for_each(|item| share_parts(item, seen));
            }
        }
        Value::Map(fields) => {
            if let Some(fields) = Rc::get_mut(fields) {
                for (key, field) in fields.iter_mut() {
                    share_word(key, &mut seen.words);
                    share_parts(field, seen);
                }
            }
            // (A record with an id, as a tool is: its one copy.)
            let Some(Value::Str(id)) = value.field("id").cloned() else { return };
            match seen.records.get(&id) {
                Some(known) if *known == *value => *value = known.clone(),
                Some(_) => {}
                None => {
                    seen.records.insert(id, value.clone());
                }
            }
        }
        _ => {}
    }
}

/// The engine's `order_sum` (crates/doc/src/transcript_delta.rs): the
/// entries' ids in order, FNV-1a, each ended by a zero byte.
fn order_sum<'a>(ids: impl IntoIterator<Item = &'a str>) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for id in ids {
        for byte in id.as_bytes().iter().chain(&[0u8]) {
            h ^= u64::from(*byte);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    h
}

/// `params` with what `open` holds (`known`), when it has rows to keep
/// and they aren't to be built again (`rebuild`).
fn with_known(mut params: Json, open: &Open) -> Json {
    if open.has_whole && !open.rebuild {
        params["known"] =
            Json::Array(open.entries.iter().filter(|e| !e.id.is_empty()).map(|e| json!([&*e.id, e.sum()])).collect());
    }
    params
}

/// Entries by their ids.
fn by_id(entries: &[Entry]) -> HashMap<&str, &Entry> {
    entries.iter().map(|e| (&*e.id, e)).collect()
}

/// `built` less what a reset changed: only the entries it has as they were.
fn keep_unchanged(built: &mut HashMap<Rc<str>, Built>, before: &[Entry], now: &[Entry]) {
    if built.is_empty() {
        return;
    }
    let (before, now) = (by_id(before), by_id(now));
    built.retain(|id, _| {
        let id: &str = id;
        before.get(id).zip(now.get(id)).is_some_and(|(was, is)| was.sum() == is.sum())
    });
}

/// Into `open.built`, the values built from `entries` for those of its own
/// entries that are the same. (Built for another model's name: none.)
fn reuse_built(
    open: &mut Open,
    entries: &[Entry],
    built: &HashMap<Rc<str>, Built>,
    built_model: &Option<Option<String>>,
) {
    if built_model.is_none() || (open.built_model.is_some() && open.built_model != *built_model) {
        return;
    }
    open.built_model = built_model.clone();
    let theirs = by_id(entries);
    for entry in &open.entries {
        if !entry.id.is_empty()
            && !open.built.contains_key(&entry.id)
            && let Some(value) = built.get(&entry.id)
            && theirs.get(&*entry.id).is_some_and(|their| their.sum() == entry.sum())
        {
            open.built.insert(entry.id.clone(), value.clone());
        }
    }
}

/// A frame applied to `entries` (crates/doc/src/transcript_delta.rs's
/// `apply_transcript_frame`); `Err` when the copy is out of step.
fn apply(entries: &mut Vec<Entry>, frame: &mut Json) -> Result<Change, ()> {
    apply_counted(entries, frame, true)
}

/// `apply`, its count checked or not (`counted`): not while a view holds a
/// chat's whole rows and the engine counts its opening tail (`heard_frame`).
fn apply_counted(entries: &mut Vec<Entry>, frame: &mut Json, counted: bool) -> Result<Change, ()> {
    if let Some(reset) = frame.get_mut("reset") {
        // (Taken, not copied: a long chat's whole transcript, twice over.)
        *entries = match reset.take() {
            Json::Array(list) => list.into_iter().map(Entry::new).collect(),
            _ => Vec::new(),
        };
        return Ok(Change::Rows);
    }
    let frame = &*frame;
    let mut rows = false;
    let mut words_changed = false;
    for id in frame["remove"].as_array().into_iter().flatten() {
        let before = entries.len();
        entries.retain(|e| id.as_str() != Some(&*e.id));
        rows |= entries.len() != before;
    }
    for up in frame["upsert"].as_array().into_iter().flatten() {
        let entry = &up["entry"];
        let id = entry["id"].as_str().unwrap_or_default();
        entries.retain(|e| &*e.id != id);
        let at = match up["after"].as_str() {
            None => 0,
            Some(after) => entries
                .iter()
                .position(|e| &*e.id == after)
                .map(|i| i + 1)
                .ok_or(())?,
        };
        entries.insert(at, Entry::new(entry.clone()));
        rows = true;
    }
    for add in frame["append"].as_array().into_iter().flatten() {
        let entry = entries
            .iter_mut()
            .find(|e| add["entry"].as_str() == Some(&*e.id))
            .ok_or(())?;
        // (A settled entry growing again: its whole copy is the engine's.)
        entry.patched = None;
        let entry = entry.json.as_mut().ok_or(())?;
        let streaming = entry["status"].as_str() == Some("streaming");
        let part = entry["parts"]
            .as_array_mut()
            .and_then(|parts| parts.iter_mut().find(|p| p["id"] == add["part"]))
            .ok_or(())?;
        let kind = part["kind"].as_str().unwrap_or_default().to_owned();
        let more = add["text"].as_str().unwrap_or_default();
        // Grown in place (a long reply isn't copied whole for each word).
        let was_empty = match part.get_mut("text") {
            Some(Json::String(text)) => {
                let empty = text.trim().is_empty();
                text.push_str(more);
                empty
            }
            _ => {
                part["text"] = Json::String(more.to_owned());
                true
            }
        };
        let len = part["text"].as_str().map_or(0, str::len);
        if add["len"].as_u64().is_some_and(|want| want as usize != len) {
            return Err(());
        }
        match kind.as_str() {
            // Words of the reply being written: the draft; of a finished
            // one, its row. A text part that had none (written empty, or
            // blank) has no block in its row yet (`blocks_of`): its first
            // words make it, as a thought's do.
            "text" if streaming && was_empty && !more.trim().is_empty() => rows = true,
            "text" if streaming => words_changed = true,
            "text" => rows = true,
            // Thinking shows as a row once it has words; more of them, as
            // it's written, are the draft's, as a reply's are. (More of a
            // tool's output shows nothing new.)
            "reasoning" if was_empty && !more.trim().is_empty() => rows = true,
            "reasoning" if streaming => words_changed = true,
            _ => {}
        }
    }
    // Parts of an entry replaced or added (`partPatches`): the entry otherwise
    // as it was, its other parts too (`Entry::patched`).
    for patch in frame["parts"].as_array().into_iter().flatten() {
        let entry = entries.iter_mut().find(|e| patch["entry"].as_str() == Some(&*e.id)).ok_or(())?;
        let at = patch["at"].as_u64().ok_or(())? as usize;
        let part = &patch["part"];
        let id = part["id"].as_str().ok_or(())?.to_owned();
        let json = entry.json.as_mut().ok_or(())?;
        let parts = json["parts"].as_array_mut().ok_or(())?;
        if at < parts.len() && parts[at]["id"].as_str() == Some(&id) {
            parts[at] = part.clone();
        } else if at == parts.len() {
            parts.push(part.clone());
        } else {
            return Err(());
        }
        if patch["len"].as_u64().is_some_and(|len| (len as usize) < parts.len()) {
            return Err(());
        }
        if let Some(patched) = &mut entry.patched {
            patched.insert(id);
        }
        rows = true;
    }
    if counted && frame["count"].as_u64().is_some_and(|n| n as usize != entries.len()) {
        return Err(());
    }
    Ok(if rows {
        Change::Rows
    } else if words_changed {
        Change::Words
    } else {
        Change::Quiet
    })
}

/// `entry` with what its parts read from the chat's host: each visual names
/// `device`, its host, when that isn't this one; each command whose output
/// the doc cut to its last lines names `doc`, the doc it is in, so its row
/// can ask the host for the rest (`chat.tool_output`). Nil when it has
/// neither: the entry as it is.
fn with_host(entry: &Json, doc: &str, device: Option<&str>) -> Option<Json> {
    let parts = entry["parts"].as_array()?;
    let visual = |p: &Json| device.is_some() && p["kind"].as_str() == Some("visual");
    let cut = |p: &Json| p["kind"].as_str() == Some("tool") && p["outputTruncated"].as_bool() == Some(true);
    if !parts.iter().any(|p| visual(p) || cut(p)) {
        return None;
    }
    let mut entry = entry.clone();
    for part in entry["parts"].as_array_mut().into_iter().flatten() {
        if visual(part)
            && let Some(device) = device
        {
            part["device"] = Json::String(device.to_owned());
        }
        if cut(part) {
            part["chat"] = Json::String(doc.to_owned());
        }
    }
    Some(entry)
}

/// The device a read of `doc`'s host data goes to (`targetDeviceId`), when
/// that isn't this one: a subagent's doc (`{chat}--sub--{id}`) is its root
/// chat's.
pub(crate) fn host_of(doc: &str) -> Option<String> {
    let chats = CHATS.with(Rc::clone);
    let source = doc.split_once("--sub--").map_or(doc, |(chat, _)| chat);
    chats.target(&chats.chat(source)?)
}

/// An entry's words: its text parts, each a paragraph.
fn entry_text(entry: &Json) -> String {
    let mut out = String::new();
    for p in entry["parts"].as_array().into_iter().flatten() {
        let words = match p["kind"].as_str() {
            Some("text") => p["text"].as_str().unwrap_or_default().to_owned(),
            Some("error") => format!("> {}", p["message"].as_str().unwrap_or_default()),
            _ => continue,
        };
        if words.trim().is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(&words);
    }
    out
}

/// A transcript's subagents: each spawn that names its doc, once, in the order they
/// were spawned, with when their turn began (a message's `tools` hold all of its).
fn subagents_in(message: &Value) -> Option<Rc<[Value]>> {
    let Some(Value::List(tools)) = message.field("tools") else { return None };
    let found: Vec<Value> = tools
        .iter()
        .filter_map(|tool| {
            let sub = tool.field("subagent")?;
            let reference = sub.field("ref").filter(|r| matches!(r, Value::Str(_)))?;
            let field = |key: &str| sub.field(key).cloned().unwrap_or(Value::Nil);
            Some(Value::record([
                ("ref", reference.clone()),
                ("title", field("title")),
                ("status", field("status")),
                ("at", message.field("started").cloned().unwrap_or(Value::Nil)),
            ]))
        })
        .collect();
    (!found.is_empty()).then(|| found.into())
}

/// A transcript's subagents as its rows are added, each once, in order.
#[derive(Default)]
struct Subagents(Vec<Value>, std::collections::HashSet<Rc<str>>);

impl Subagents {
    fn add(&mut self, found: Option<&[Value]>) {
        for sub in found.into_iter().flatten() {
            if let Some(Value::Str(reference)) = sub.field("ref")
                && self.1.insert(reference.clone())
            {
                self.0.push(sub.clone());
            }
        }
    }
}

/// An entry as ds/transcript's message; nil for one it doesn't show.
pub(crate) fn message(entry: &Json, model: Option<&str>) -> Option<Value> {
    let id = entry["id"].as_str()?;
    match entry["role"].as_str()? {
        "user" => {
            let text = entry["parts"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|p| p["kind"].as_str() == Some("text"))
                .filter_map(|p| p["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n\n");
            // A message from another chat: its routing header is the agent's to
            // read; the transcript shows who sent it, over its words.
            let sender = zeron_proto::agent_messages::agent_message_sender(&text);
            let body = sender.as_ref().map_or(text.as_str(), |(_, body)| *body);
            let sender = sender.as_ref().map_or(Value::Nil, |(from, _)| {
                Value::record([("chat", Value::from(from.chat_id.as_str())), ("title", Value::from(from.title.as_str()))])
            });
            let (words, mut paths) = split_attachments(body);
            // (An Appshot's context isn't what the person said: its picture
            // says which app instead.)
            let (words, shots) = crate::appshots::split_appshots(&words);
            let (words, chats) = split_chats(&words);
            let (words, comments) = split_comments(&words);
            let (words, quotes) = split_quotes(&words);
            let (words, picks) = split_annotations(&words);
            for p in entry["parts"].as_array().into_iter().flatten() {
                if p["kind"].as_str() == Some("image")
                    && let Some(path) = p["path"].as_str()
                    && !paths.iter().any(|q| q == path)
                {
                    paths.push(path.to_owned());
                }
            }
            // Always its time under it, as the shipping app's prompts carry
            // it (the strip shows on hover), and its words to copy.
            let created = entry["createdAt"].as_f64().unwrap_or(0.) / 1000.;
            Some(Value::record([
                ("id", Value::from(id)),
                ("role", Value::from("user")),
                ("text", Value::from(words.as_str())),
                ("attachments", crate::appshots::tag_attachments(carried_value(&paths, &quotes, &picks, &comments, &chats), &shots)),
                ("at", stamp(created as i64).map_or(Value::Nil, |at| Value::from(at.as_str()))),
                ("copy", copy_value(&words)),
                ("sender", sender),
            ]))
        }
        "assistant" => {
            let streaming = entry["status"].as_str() == Some("streaming");
            let created = entry["createdAt"].as_f64().unwrap_or(0.) / 1000.;
            let tools: Vec<Value> = entry["parts"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|p| !display_tool(p))
                .filter_map(tool)
                .collect();
            // A settled reply with nothing to show (only an empty thought,
            // say) has no row, as in the shipping app: drawn, it would be an
            // empty lane whose time floats over the next reply.
            let shown = blocks(entry);
            if !streaming && matches!(&shown, Value::List(list) if list.is_empty()) {
                return None;
            }
            let mut fields = vec![
                ("id", Value::from(id)),
                ("role", Value::from("assistant")),
                ("text", Value::from(entry_text(entry).as_str())),
                ("tools", Value::List(tools.into())),
                ("blocks", shown),
                ("streaming", Value::Bool(streaming)),
                ("started", Value::Num(created)),
            ];
            if let Some(model) = model {
                fields.push(("model", Value::from(model)));
            }
            // Its time and words to copy once it has settled, as the
            // shipping app's strip under a reply ("No timestamp hover
            // mid-stream").
            if !streaming {
                if let Some(at) = stamp(created as i64) {
                    fields.push(("at", Value::from(at.as_str())));
                }
                fields.push(("copy", copy_value(&reply_copy_text(entry))));
            }
            if let Some(ms) = entry["durationMs"].as_f64() {
                fields.push(("worked", Value::Num((ms / 1000.).round().max(1.))));
            }
            // The same reply laid out for Settings' compact transcript (one
            // being written, only while that shows: built with each of its
            // changes, it'd cost each the whole reply again).
            if (!streaming || COMPACT_SHOWN.get())
                && let Some(compact) = compact_blocks(entry)
            {
                fields.push(("compact", compact));
            }
            Some(Value::record(fields))
        }
        // The provider-switch seam (the engine's system entry when a chat
        // moves to another agent): `[mark] From → [mark] To` over the prompt
        // that switched, as the shipping app's `switch_marker`.
        "system" => {
            let part = entry["parts"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|p| p["kind"].as_str() == Some("switch"))?;
            let side = |name: &str, harness: &str| {
                let mark = part[harness].as_str().and_then(switch_mark);
                (
                    Value::from(single_line(part[name].as_str().unwrap_or_default()).as_str()),
                    mark.map_or(Value::Nil, |m| Value::from(m.as_str())),
                )
            };
            let (from, from_mark) = side("from", "fromHarness");
            let (to, to_mark) = side("to", "toHarness");
            Some(Value::record([
                ("id", Value::from(id)),
                ("role", Value::from("switch")),
                ("text", Value::from("")),
                ("from", from),
                ("to", to),
                ("from_mark", from_mark),
                ("to_mark", to_mark),
            ]))
        }
        _ => None,
    }
}

/// A switch side's mark: its agent's, the mock drawn as Claude, as the
/// shipping app's `harness_brand_icon`.
fn switch_mark(harness: &str) -> Option<String> {
    if harness == "mock" {
        return Some("claude".into());
    }
    crate::agents::glyph(harness)
}

/// A name on one line (the shipping app's `single_line`).
fn single_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// What a message's copy action copies; nil (no action) with nothing to copy.
fn copy_value(text: &str) -> Value {
    if text.trim().is_empty() { Value::Nil } else { Value::from(text) }
}

/// A reply's words to copy: its text parts as written, no tools, no errors
/// (the shipping app's `assistant_copy_text`).
fn reply_copy_text(entry: &Json) -> String {
    entry["parts"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|p| p["kind"].as_str() == Some("text"))
        .filter_map(|p| p["text"].as_str())
        .filter(|t| !t.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// A reply's parts in order, as the shipping app lays them out (crates/ui
/// transcript.rs, its non-compact rows): its text, and between the text each
/// run of consecutive tools (thinking with them) as a group of its own,
/// keyed by its first part. A subagent's tools never share a group with the
/// others, nor thinking with them. A group is open while the reply streams
/// and it is the reply's tail.
fn blocks(entry: &Json) -> Value {
    let parts: Vec<&Json> = entry["parts"].as_array().into_iter().flatten().collect();
    blocks_of(&parts, entry["status"].as_str() == Some("streaming"))
}

/// The blocks of these parts of a reply (`streaming`: still being written).
fn blocks_of(parts: &[&Json], streaming: bool) -> Value {
    let last_ix = parts.len().saturating_sub(1);
    let mut out: Vec<Value> = Vec::new();
    // The group being gathered: its parts, and the last one's index.
    let mut group: Vec<&Json> = Vec::new();
    let mut group_last = 0usize;
    let flush = |out: &mut Vec<Value>, group: &mut Vec<&Json>, group_last: usize| {
        if group.is_empty() {
            return;
        }
        let first = group[0]["id"].as_str().unwrap_or_default();
        // (The part being written, if the group is the reply's tail: a
        // thought then shows its latest lines.)
        let live = streaming && group_last == last_ix;
        let tools: Vec<Value> = group
            .iter()
            .enumerate()
            .filter_map(|(i, p)| tool_live(p, live && i + 1 == group.len()))
            .collect();
        out.push(Value::record([
            ("id", Value::from(format!("g:{first}").as_str())),
            ("kind", Value::from("tools")),
            ("summary", Value::from(group_summary(group).as_str())),
            ("live", Value::Bool(streaming && group_last == last_ix)),
            ("tools", Value::List(tools.into())),
        ]));
        group.clear();
    };
    // The pictures an agent showed one after another (`show_image`, a
    // generated image), gathered into one block: a display call that worked
    // between them doesn't part them; anything else does.
    let mut pictures: Vec<&Json> = Vec::new();
    for (ix, &part) in parts.iter().enumerate() {
        let kind = part["kind"].as_str();
        if kind == Some("image") {
            flush(&mut out, &mut group, group_last);
            pictures.push(part);
            continue;
        }
        if display_tool(part) && display_failed_block(part, parts[ix + 1..].iter().copied()).is_none() {
            continue;
        }
        out.extend(images_block(&mut pictures));
        match kind {
            // A display tool (`show_visual`, `show_image`) is its output
            // alone: no row, in no group or count. Failed, a quiet line at
            // its place says so (unless a later call of it showed).
            Some("tool") if display_tool(part) => {
                if let Some(failed) = display_failed_block(part, parts[ix + 1..].iter().copied()) {
                    flush(&mut out, &mut group, group_last);
                    out.push(failed);
                }
            }
            Some("tool") => {
                if group.first().is_some_and(|head| is_agent(head) != is_agent(part)) {
                    flush(&mut out, &mut group, group_last);
                }
                group.push(part);
                group_last = ix;
            }
            Some("reasoning") => {
                if part["text"].as_str().unwrap_or_default().trim().is_empty() {
                    continue;
                }
                if group.first().is_some_and(|head| is_agent(head)) {
                    flush(&mut out, &mut group, group_last);
                }
                group.push(part);
                group_last = ix;
            }
            kind => {
                flush(&mut out, &mut group, group_last);
                // An agent's visual (`show_visual`): its script on the
                // chat's host, run and drawn by src/visuals.rs and
                // ds/visual (`device` only when that isn't this one).
                if kind == Some("visual") {
                    out.push(visual_block(part));
                    continue;
                }
                let words = match kind {
                    Some("text") => part["text"].as_str().unwrap_or_default().to_owned(),
                    Some("error") => format!("> {}", part["message"].as_str().unwrap_or_default()),
                    _ => continue,
                };
                if words.trim().is_empty() {
                    continue;
                }
                out.push(Value::record([
                    ("id", Value::from(part["id"].as_str().unwrap_or_default())),
                    ("kind", Value::from("text")),
                    ("text", Value::from(words.as_str())),
                ]));
            }
        }
    }
    flush(&mut out, &mut group, group_last);
    out.extend(images_block(&mut pictures));
    Value::List(out.into())
}

/// A reply as the shipping app lays it out in compact mode (transcript.rs
/// `rows_for_entry` with `compact`): all its work, its tools, its thinking
/// and what it said between them, folded into one group where the first
/// of it was, its line the tools' summary while it works and "Worked for
/// 42s" once done; then the reply, the run of text it ends on, shown. While
/// it streams there is no reply yet: everything rides the group. Inside the
/// group, opened, the reply's ordinary blocks. Nil when it did no work.
fn compact_blocks(entry: &Json) -> Option<Value> {
    let parts = entry["parts"].as_array()?;
    let streaming = entry["status"].as_str() == Some("streaming");
    let said = |p: &Json| {
        p["kind"].as_str() == Some("text") && !p["text"].as_str().unwrap_or_default().trim().is_empty()
    };
    let thought = |p: &Json| {
        p["kind"].as_str() == Some("reasoning") && !p["text"].as_str().unwrap_or_default().trim().is_empty()
    };
    let reply_start = if streaming {
        None
    } else {
        parts.iter().rposition(said).map(|mut start| {
            while start > 0 && said(&parts[start - 1]) {
                start -= 1;
            }
            start
        })
    };
    // (A display tool is no work: its output shows in the reply.)
    let worked_tool = |p: &Json| p["kind"].as_str() == Some("tool") && !display_tool(p);
    let is_work = |ix: usize, p: &Json| {
        worked_tool(p) || thought(p) || (said(p) && reply_start.is_none_or(|start| ix < start))
    };
    let tools: Vec<&Json> = parts.iter().filter(|p| worked_tool(p) || thought(p)).collect();
    if tools.is_empty() {
        return None;
    }
    let work_parts: Vec<&Json> =
        parts.iter().enumerate().filter(|(ix, p)| is_work(*ix, p)).map(|(_, p)| p).collect();
    let worked = (!streaming)
        .then(|| entry["durationMs"].as_f64())
        .flatten()
        .filter(|ms| *ms > 0.)
        .map(|ms| ((ms / 1000.) as i64).max(1));
    let line = match worked {
        Some(secs) => format!("Worked for {}", elapsed(secs)),
        None => group_summary(&tools),
    };
    let work = Value::record([
        ("id", Value::from("work")),
        ("kind", Value::from("work")),
        ("summary", Value::from(line.as_str())),
        ("live", Value::Bool(streaming)),
        ("tools", Value::List(tools.iter().filter_map(|p| tool(p)).collect::<Vec<_>>().into())),
        ("blocks", blocks_of(&work_parts, streaming)),
    ]);
    let mut out: Vec<Value> = Vec::new();
    let mut placed = false;
    let mut pictures: Vec<&Json> = Vec::new();
    for (ix, part) in parts.iter().enumerate() {
        if part["kind"].as_str() == Some("image") {
            pictures.push(part);
            continue;
        }
        if display_tool(part) && display_failed_block(part, &parts[ix + 1..]).is_none() {
            continue;
        }
        out.extend(images_block(&mut pictures));
        if is_work(ix, part) {
            if !placed {
                out.push(work.clone());
                placed = true;
            }
            continue;
        }
        let words = match part["kind"].as_str() {
            Some("text") => part["text"].as_str().unwrap_or_default().to_owned(),
            Some("error") => format!("> {}", part["message"].as_str().unwrap_or_default()),
            Some("visual") => {
                out.push(visual_block(part));
                continue;
            }
            Some("tool") => {
                out.extend(display_failed_block(part, &parts[ix + 1..]));
                continue;
            }
            _ => continue,
        };
        if words.trim().is_empty() {
            continue;
        }
        out.push(Value::record([
            ("id", Value::from(part["id"].as_str().unwrap_or_default())),
            ("kind", Value::from("text")),
            ("text", Value::from(words.as_str())),
        ]));
    }
    out.extend(images_block(&mut pictures));
    Some(Value::List(out.into()))
}

/// Tools whose output is what they show (a picture, a visual): drawn as
/// that output alone, never as a tool row, in no group, count or summary.
/// The images chat and the visuals chat share this rule.
const DISPLAY_TOOLS: &[&str] = &["show_image", "show_visual"];

/// Whether `part` is a display tool's call: an MCP tool by its name, bare or
/// as a harness names it (`mcp__zeron__show_visual`).
fn display_tool(part: &Json) -> bool {
    if part["kind"].as_str() != Some("tool") {
        return false;
    }
    let call = &part["call"];
    let name = match call["kind"].as_str() {
        Some("mcp") => call["tool"].as_str(),
        Some("unknown") => call["name"].as_str(),
        _ => None,
    };
    name.and_then(|n| n.rsplit("__").next()).is_some_and(|n| DISPLAY_TOOLS.contains(&n))
}

/// A display tool's name without its server (`show_image`), if `part` is one.
fn display_name(part: &Json) -> Option<&str> {
    if !display_tool(part) {
        return None;
    }
    let call = &part["call"];
    call["tool"].as_str().or(call["name"].as_str()).and_then(|n| n.rsplit("__").next())
}

/// A display tool's call that failed, as the quiet line ds/transcript draws
/// in its place (`display_failed`): which it was (`tool`: "image" or
/// "visual") and why, in its own words (ds/reply_picture's `plain` keeps
/// only what's plain of it). Nil for one that worked or is still working
/// (its output, or nothing yet, shows), and for one a later call of the
/// same tool in the reply (`later`, its parts after this one) got past:
/// the agent tried again and it showed, so the stumble isn't worth a line.
fn display_failed_block<'a>(part: &Json, later: impl IntoIterator<Item = &'a Json>) -> Option<Value> {
    let name = display_name(part)?;
    if part["isError"].as_bool() != Some(true) {
        return None;
    }
    if later.into_iter().any(|p| display_name(p) == Some(name) && p["isError"].as_bool() != Some(true)) {
        return None;
    }
    let why = part["output"].as_str().map(str::trim).unwrap_or_default();
    Some(Value::record([
        ("id", Value::from(part["id"].as_str().unwrap_or_default())),
        ("kind", Value::from("display_failed")),
        ("tool", Value::from(name.strip_prefix("show_").unwrap_or(name))),
        ("text", Value::from(why)),
    ]))
}

/// The pictures gathered (image parts in a row) as one ds/transcript block,
/// `images`, keyed by the first: each its path on the chat's host and its
/// name. Nil for none; the gathering is emptied.
fn images_block(pictures: &mut Vec<&Json>) -> Option<Value> {
    let first = pictures.first()?["id"].as_str().unwrap_or_default().to_owned();
    let list: Vec<Value> = pictures
        .drain(..)
        .map(|p| {
            Value::record([
                ("id", Value::from(p["id"].as_str().unwrap_or_default())),
                ("path", Value::from(p["path"].as_str().unwrap_or_default())),
                ("name", Value::from(p["name"].as_str().unwrap_or_default())),
            ])
        })
        .collect();
    Some(Value::record([
        ("id", Value::from(first.as_str())),
        ("kind", Value::from("images")),
        ("images", Value::List(list.into())),
    ]))
}

/// A visual part as ds/transcript's block: its id (the visual's, which
/// seeds it), its script's path and title, and its host's device when that
/// isn't this one.
fn visual_block(part: &Json) -> Value {
    let field = |k: &str| part[k].as_str().map_or(Value::Nil, Value::from);
    Value::record([
        ("id", Value::from(part["id"].as_str().unwrap_or_default())),
        ("kind", Value::from("visual")),
        ("path", field("path")),
        ("title", field("title")),
        ("device", field("device")),
    ])
}

/// How long, as the shipping app says it (transcript.rs `format_elapsed`).
fn elapsed(secs: i64) -> String {
    let secs = secs.max(0);
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3_600 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else if secs < 86_400 {
        format!("{}h {}m", secs / 3_600, (secs % 3_600) / 60)
    } else {
        format!("{}d {}h", secs / 86_400, (secs % 86_400) / 3_600)
    }
}

/// A subagent's tab title, as the shipping app's `subagent_tab_title`: the
/// tool's name, else its description, else its prompt, the "Agent"/"Task"
/// genus taken off, the first line, at most 40 characters.
fn subagent_title(call: &Json) -> String {
    fn strip(text: &str) -> &str {
        let t = text.trim();
        for prefix in ["agent", "task"] {
            if t.len() >= prefix.len() && t.is_char_boundary(prefix.len()) && t[..prefix.len()].eq_ignore_ascii_case(prefix) {
                let rest = &t[prefix.len()..];
                if rest.is_empty() {
                    return "";
                }
                if rest.starts_with(':') || rest.starts_with(char::is_whitespace) {
                    return rest.trim_start_matches(':').trim();
                }
            }
        }
        t
    }
    fn line(text: &str) -> Option<String> {
        let first = text.lines().find(|l| !l.trim().is_empty())?.trim();
        let mut out: String = first.chars().take(40).collect();
        if first.chars().count() > 40 {
            out.push('…');
        }
        Some(out)
    }
    let name = match call["kind"].as_str() {
        Some("mcp") => call["tool"].as_str(),
        Some("unknown") => call["name"].as_str(),
        _ => return "Subagent".into(),
    };
    let input = &call["input"];
    [name, input["description"].as_str(), input["prompt"].as_str()]
        .into_iter()
        .flatten()
        .find_map(|t| line(strip(t)))
        .unwrap_or_else(|| "Subagent".into())
}

/// A subagent's call (its own group, as the shipping app's agent chips).
fn is_agent(part: &Json) -> bool {
    let call = &part["call"];
    let name = match call["kind"].as_str() {
        Some("mcp") => call["tool"].as_str(),
        Some("unknown") => call["name"].as_str(),
        _ => None,
    };
    part["kind"].as_str() == Some("tool") && name.is_some_and(|n| n == "Agent" || n.starts_with("Agent: "))
}

/// A group's line, as the shipping app writes it (crates/ui transcript.rs
/// `tool_group_summary` over crates/proto view.rs's): its thinking, then
/// what its tools did, then how many failed; the first letter raised.
fn group_summary(group: &[&Json]) -> String {
    let mut tally = GroupTally::default();
    for p in group {
        tally.add(&Tally::of(p));
    }
    tally.text()
}

/// What one part of a group adds to its line (`group_summary`): a thought,
/// a call of a kind (and the file it edited), or nothing.
#[derive(Clone, PartialEq, Debug)]
enum Tally {
    None,
    Thought,
    Call {
        kind: CallKind,
        failed: bool,
        edited: Option<String>,
    },
}
#[derive(Clone, Copy, PartialEq, Debug)]
enum CallKind {
    Command,
    Edit,
    Read,
    Search,
    Fetch,
    Todo,
    Other,
    /// A Cloud machine's own setup step (`woke`: its wake).
    Cloud {
        woke: bool,
    },
}
impl Tally {
    fn of(p: &Json) -> Tally {
        match p["kind"].as_str() {
            Some("reasoning") => Tally::Thought,
            Some("tool") => {
                let failed = p["isError"].as_bool() == Some(true);
                let call = &p["call"];
                let (kind, edited) = match call["kind"].as_str().unwrap_or_default() {
                    "exec" => (CallKind::Command, None),
                    "writeFile" | "editFile" => (
                        CallKind::Edit,
                        Some(call["path"].as_str().unwrap_or_default().to_owned()),
                    ),
                    "applyPatch" => (
                        CallKind::Edit,
                        Some(call["path"].as_str().unwrap_or("patch").to_owned()),
                    ),
                    "readFile" => (CallKind::Read, None),
                    "search" | "glob" | "webSearch" => (CallKind::Search, None),
                    "webFetch" => (CallKind::Fetch, None),
                    "todo" => (CallKind::Todo, None),
                    _ => match cloud_setup_step(p) {
                        Some(step) => (CallKind::Cloud { woke: step == "wake" }, None),
                        None => (CallKind::Other, None),
                    },
                };
                Tally::Call {
                    kind,
                    failed,
                    edited,
                }
            }
            _ => Tally::None,
        }
    }
}
/// A group's parts, counted for its line: kept as parts come, change and go
/// (a reply being written: `LiveRow`), so the line is made without looking
/// at the group's other parts again.
#[derive(Default, Clone)]
struct GroupTally {
    thoughts: usize,
    calls: usize,
    commands: usize,
    reads: usize,
    searches: usize,
    fetches: usize,
    todos: usize,
    other: usize,
    /// A Cloud machine's setup steps (counted in `other` too), and its wakes.
    cloud: usize,
    woke: usize,
    failed: usize,
    /// Each file once, however often it was edited: how many edits each.
    edited: HashMap<String, usize>,
}
impl GroupTally {
    fn add(&mut self, tally: &Tally) {
        self.count(tally, true);
    }
    fn remove(&mut self, tally: &Tally) {
        self.count(tally, false);
    }
    fn count(&mut self, tally: &Tally, add: bool) {
        let step = |n: &mut usize| {
            if add {
                *n += 1;
            } else {
                *n -= 1;
            }
        };
        match tally {
            Tally::None => {}
            Tally::Thought => step(&mut self.thoughts),
            Tally::Call {
                kind,
                failed,
                edited,
            } => {
                step(&mut self.calls);
                if *failed {
                    step(&mut self.failed);
                }
                match kind {
                    CallKind::Command => step(&mut self.commands),
                    CallKind::Read => step(&mut self.reads),
                    CallKind::Search => step(&mut self.searches),
                    CallKind::Fetch => step(&mut self.fetches),
                    CallKind::Todo => step(&mut self.todos),
                    CallKind::Other => step(&mut self.other),
                    CallKind::Cloud { woke } => {
                        step(&mut self.other);
                        step(&mut self.cloud);
                        if *woke {
                            step(&mut self.woke);
                        }
                    }
                    CallKind::Edit => {
                        let path = edited.clone().unwrap_or_default();
                        if add {
                            *self.edited.entry(path).or_default() += 1;
                        } else if let Some(n) = self.edited.get_mut(&path) {
                            *n -= 1;
                            if *n == 0 {
                                self.edited.remove(&path);
                            }
                        }
                    }
                }
            }
        }
    }
    /// The line: its thinking, then what its tools did, then how many failed;
    /// the first letter raised.
    fn text(&self) -> String {
        // A Cloud machine's setup alone reads as what happened to the machine,
        // as the shipping app's (zeron_proto `tool_group_summary`).
        if self.calls > 0 && self.cloud == self.calls && self.thoughts == 0 {
            let mut line = if self.woke > 0 { "Woke the Cloud machine" } else { "Set up the Cloud machine" }.to_owned();
            if self.failed > 0 {
                line.push_str(&format!(" · {} failed", self.failed));
            }
            return line;
        }
        let mut segments: Vec<String> = Vec::new();
        match self.thoughts {
            0 => {}
            1 => segments.push("thought process".into()),
            n => segments.push(format!("thought {n} times")),
        }
        if self.calls > 0 {
            let plural = |n: usize, one: &str, many: &str| {
                format!("{n} {}", if n == 1 { one } else { many })
            };
            let mut tools: Vec<String> = Vec::new();
            if self.commands > 0 {
                tools.push(format!(
                    "ran {}",
                    plural(self.commands, "command", "commands")
                ));
            }
            if !self.edited.is_empty() {
                tools.push(format!(
                    "edited {}",
                    plural(self.edited.len(), "file", "files")
                ));
            }
            if self.reads > 0 {
                tools.push(format!("read {}", plural(self.reads, "file", "files")));
            }
            if self.searches > 0 {
                tools.push(format!(
                    "searched {}",
                    plural(self.searches, "time", "times")
                ));
            }
            if self.fetches > 0 {
                tools.push(format!("fetched {}", plural(self.fetches, "page", "pages")));
            }
            if self.todos > 0 {
                tools.push("updated todos".into());
            }
            if self.other > 0 {
                tools.push(format!("called {}", plural(self.other, "tool", "tools")));
            }
            if tools.is_empty() {
                tools.push(plural(self.calls, "tool", "tools"));
            }
            if self.failed > 0 {
                tools.push(format!("{} failed", self.failed));
            }
            segments.push(tools.join(" · "));
        }
        let mut summary = segments.join(" · ");
        if let Some(first) = summary.get_mut(..1) {
            first.make_ascii_uppercase();
        }
        summary
    }
}

/// A part as ds/activity's tool: its kind, what it did, to what.
fn tool(part: &Json) -> Option<Value> {
    tool_live(part, false)
}

/// A thought's words as its row shows them: all of them. The row folds
/// them itself (ds/activity `thought_row`: four lines, "Show all N lines"),
/// so a cut here would leave "Show all" short of the whole thought.
fn thought_words(text: &str) -> String {
    text.trim().to_owned()
}

/// `tool`, for the part a reply is writing now (`live`): built once for the
/// settled row being built, however many of its layouts show it.
fn tool_live(part: &Json, live: bool) -> Option<Value> {
    let at = std::ptr::from_ref(part);
    if let Some(built) = ROW_BUILD.with_borrow(|build| build.as_ref()?.tools.get(&at).cloned()) {
        return built;
    }
    // A reply being written is built again with each change of it (a call
    // arriving, one finishing), its other calls as they were: each of those
    // is the very value it was (`LIVE_TOOLS`), so only what changed is built,
    // and the page, given the same values, has nothing to do for them.
    let settled = ROW_BUILD.with_borrow(Option::is_some);
    if !settled && let Some(id) = part["id"].as_str() {
        if let Some(built) = LIVE_PASS.with_borrow(|pass| pass.as_ref()?.get(&(at, live)).cloned()) {
            return built;
        }
        let built = live_tool(id, part, live);
        LIVE_PASS.with_borrow_mut(|pass| {
            if let Some(pass) = pass {
                pass.insert((at, live), built.clone());
            }
        });
        return built;
    }
    let mut built = tool_row(part, live);
    ROW_BUILD.with_borrow_mut(|build| {
        if let Some(build) = build {
            if let Some(built) = &mut built {
                share_parts(built, &mut build.seen);
            }
            build.tools.insert(at, built.clone());
        }
    });
    built
}

/// The tool of a part of a reply being written: as `LIVE_TOOLS` has it if
/// the part is as it was, else built and kept.
fn live_tool(id: &str, part: &Json, live: bool) -> Option<Value> {
    // (A part no patch touched is the one built: not compared again.)
    let unchanged = PATCHED.with_borrow(|patched| patched.as_ref().is_some_and(|patched| !patched.contains(id)));
    let kept = LIVE_TOOLS.with_borrow(|kept| {
        kept.get(id)?[usize::from(live)].as_ref().filter(|(was, _)| unchanged || was == part).map(|(_, built)| built.clone())
    });
    if let Some(built) = kept {
        return built;
    }
    let built = tool_row(part, live);
    LIVE_TOOLS.with_borrow_mut(|kept| {
        if kept.len() >= LIVE_TOOLS_MOST {
            kept.clear();
        }
        if !kept.contains_key(id) {
            kept.insert(id.to_owned(), Default::default());
        }
        if let Some(slots) = kept.get_mut(id) {
            slots[usize::from(live)] = Some((part.clone(), built.clone()));
        }
    });
    built
}

/// The Cloud machine setup step a tool part records ("start", "wake",
/// "clone", "branch"), if it is one (zeron_proto `cloud_setup_step`).
fn cloud_setup_step(part: &Json) -> Option<&str> {
    let call = &part["call"];
    if call["kind"].as_str() != Some("unknown") {
        return None;
    }
    call["input"][zeron_proto::CLOUD_SETUP_KEY].as_str()
}

fn tool_row(part: &Json, live: bool) -> Option<Value> {
    let id = part["id"].as_str()?;
    let (kind, verb, target): (&str, String, Option<String>) = match part["kind"].as_str()? {
        "reasoning" => {
            let words = part["text"].as_str().unwrap_or_default();
            if words.trim().is_empty() {
                return None;
            }
            // Its row opens to its words, as prose.
            return Some(Value::record([
                ("id", Value::from(id)),
                ("kind", Value::from("thought")),
                ("verb", Value::from("Thought process")),
                ("call", Value::from(thought_words(words).as_str())),
                ("prose", Value::Bool(true)),
                // (Its row opens itself while it's written.)
                ("live", Value::Bool(live)),
            ]));
        }
        "tool" => {
            let call = &part["call"];
            let s = |k: &str| call[k].as_str().map(str::to_owned);
            match call["kind"].as_str().unwrap_or_default() {
                "exec" => ("exec", "Ran".into(), s("command")),
                "readFile" => ("read", "Read".into(), s("path").map(|p| base(&p))),
                "writeFile" => ("write", "Wrote".into(), s("path").map(|p| base(&p))),
                "editFile" | "applyPatch" => ("edit", "Edited".into(), s("path").map(|p| base(&p))),
                "search" => ("search", "Searched".into(), s("pattern")),
                "glob" => ("glob", "Listed".into(), s("pattern")),
                "webFetch" => ("web", "Fetched".into(), s("url")),
                "webSearch" => ("web", "Searched the web".into(), s("query")),
                "todo" => ("todo", "Updated the plan".into(), None),
                // A Cloud machine's own setup ("Woke the machine", "Cloned
                // acme/app"): its name is what it did, the machine's glyph
                // (git's for its checkout), as the shipping app draws it.
                "unknown" if cloud_setup_step(part).is_some() => {
                    let kind = if matches!(cloud_setup_step(part), Some("clone" | "branch")) { "git" } else { "cloud" };
                    (kind, s("name").unwrap_or_default(), None)
                }
                "mcp" => {
                    let tool = s("tool").unwrap_or_default();
                    if tool == "Agent" || tool.starts_with("Agent: ") {
                        ("agent", "Ran a subagent".into(), tool.strip_prefix("Agent: ").map(Into::into))
                    } else {
                        ("exec", "Used".into(), Some(format!("{}.{tool}", s("server").unwrap_or_default())))
                    }
                }
                _ => {
                    let name = s("name").unwrap_or_else(|| "a tool".into());
                    if name == "Agent" || name.starts_with("Agent: ") {
                        ("agent", "Ran a subagent".into(), name.strip_prefix("Agent: ").map(Into::into))
                    } else {
                        ("exec", "Used".into(), Some(name))
                    }
                }
            }
        }
        _ => return None,
    };
    let mut fields = vec![
        ("id", Value::from(id)),
        ("kind", Value::from(kind)),
        ("verb", Value::from(verb.as_str())),
    ];
    // Its line is one line, whatever the agent wrote (the shipping app's
    // `tool_chip_content`: a command's lines, a pattern's newlines, joined).
    let target = target.map(|t| single_line(&t)).filter(|t| !t.is_empty());
    if let Some(target) = &target {
        fields.push(("target", Value::from(target.as_str())));
    }
    // The whole call, for the row opened, when it says more than the line.
    if let Some(call) = call_text(part).filter(|c| Some(c) != target.as_ref()) {
        fields.push(("call", Value::from(call.as_str())));
    }
    if part["isError"].as_bool() == Some(true) {
        fields.push(("failed", Value::Bool(true)));
    }
    // A subagent's spawn: the doc it runs in (its lane, a panel tab of its
    // own), how it stands, and its tab's title (the shipping app's).
    if kind == "agent"
        && let Some(doc) = part["subagentRef"].as_str().filter(|r| !r.is_empty())
    {
        fields.push((
            "subagent",
            Value::record([
                ("ref", Value::from(doc)),
                ("status", part["subagentStatus"].as_str().map_or(Value::Nil, Value::from)),
                ("title", Value::from(subagent_title(&part["call"]).as_str())),
            ]),
        ));
    }
    // What its row opens to (src/tool_details.rs).
    fields.extend(crate::tool_details::tool_fields(part));
    let stats = part["diffStats"].as_array();
    if let Some(stats) = stats.filter(|s| !s.is_empty()) {
        let sum = |k: &str| stats.iter().filter_map(|s| s[k].as_f64()).sum::<f64>();
        fields.push(("added", Value::Num(sum("additions"))));
        fields.push(("removed", Value::Num(sum("deletions"))));
    }
    Some(Value::record(fields))
}

/// How many lines of a call the opened row shows, and how long a line may
/// run (a minified megabyte isn't shaped whole).
const CALL_MAX_LINES: usize = 200;
const CALL_LINE_MAX_CHARS: usize = 2_000;

/// A tool's whole call, as the shipping app's `call_block` writes it: the
/// whole command, pattern (and where), url (and what was asked of it), the
/// plan's items one a line, an MCP or other tool's input as JSON. None for
/// thinking, and where the call is only a path (the line names it).
fn call_text(part: &Json) -> Option<String> {
    if part["kind"].as_str() != Some("tool") {
        return None;
    }
    let call = &part["call"];
    // A Cloud setup step opens to the command it ran (a clone's), never the
    // step's own bookkeeping.
    if cloud_setup_step(part).is_some() {
        return call["input"]["command"].as_str().filter(|c| !c.trim().is_empty()).map(str::to_owned);
    }
    let s = |k: &str| call[k].as_str().filter(|v| !v.trim().is_empty());
    let input = |head: String| match call.get("input").filter(|v| !v.is_null()) {
        Some(v) => format!("{head}\n{}", serde_json::to_string_pretty(v).unwrap_or_default()),
        None => head,
    };
    let text = match call["kind"].as_str().unwrap_or_default() {
        "exec" => s("command")?.to_owned(),
        "search" => match s("path") {
            Some(path) => format!("{} in {path}", s("pattern")?),
            None => s("pattern")?.to_owned(),
        },
        "glob" => s("pattern")?.to_owned(),
        "webFetch" => match s("prompt") {
            Some(prompt) => format!("{}\n{prompt}", s("url")?),
            None => s("url")?.to_owned(),
        },
        "webSearch" => s("query")?.to_owned(),
        "todo" => call["items"]
            .as_array()?
            .iter()
            .map(|i| {
                let mark = match (i["status"].as_str(), i["done"].as_bool()) {
                    (Some("completed"), _) | (None, Some(true)) => "[x]",
                    (Some("inProgress"), _) => "[~]",
                    _ => "[ ]",
                };
                format!("{mark} {}", i["text"].as_str().unwrap_or_default())
            })
            .collect::<Vec<_>>()
            .join("\n"),
        "mcp" => input(format!("{} · {}", s("server").unwrap_or_default(), s("tool").unwrap_or_default())),
        "unknown" => input(s("name").unwrap_or("Tool").to_owned()),
        _ => return None,
    };
    let text = text.trim_end();
    if text.is_empty() {
        return None;
    }
    let mut lines: Vec<String> = text
        .lines()
        .take(CALL_MAX_LINES)
        .map(|l| match l.char_indices().nth(CALL_LINE_MAX_CHARS) {
            Some((cut, _)) => format!("{}…", &l[..cut]),
            None => l.to_owned(),
        })
        .collect();
    let more = text.lines().count().saturating_sub(CALL_MAX_LINES);
    if more > 0 {
        lines.push(format!("… {more} more lines"));
    }
    Some(lines.join("\n"))
}

fn base(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(path)
        .to_owned()
}

/// The question the agent waits on: the last reply's unresolved input.
fn asked<'a>(mut newest_first: impl Iterator<Item = &'a Json>) -> Option<&'a Json> {
    let last = newest_first.find(|e| e["role"].as_str() == Some("assistant"))?;
    last["parts"]
        .as_array()?
        .iter()
        .rev()
        .find(|p| p["kind"].as_str() == Some("input") && p["resolved"].as_bool() != Some(true))
}

/// What a waiting chat asks, from its latest entries: an approval, else a
/// question (also when the ask isn't among them).
fn ask_kind<'a>(newest_first: impl Iterator<Item = &'a Json>) -> &'static str {
    match asked(newest_first).and_then(|p| p["questions"].as_array()) {
        Some(questions) if is_approval(questions) => "approval",
        _ => "question",
    }
}

fn is_approval(questions: &[Json]) -> bool {
    questions.len() == 1 && {
        let q = &questions[0];
        let options: Vec<&str> = q["options"].as_array().into_iter().flatten().filter_map(Json::as_str).collect();
        options == ["Yes", "No"] && q["header"].as_str().is_some_and(|h| h.starts_with("Approve"))
    }
}

fn single(questions: &[Json], label: &str) -> Vec<Json> {
    questions
        .iter()
        .filter_map(|q| q["id"].as_str())
        .map(|id| json!({ "questionId": id, "labels": [label] }))
        .collect()
}

/// What the composer shows of a question: an approval (what it would do),
/// or the page of questions not yet answered.
fn ask_value(part: &Json, answered: usize) -> Option<Value> {
    let request = part["requestId"].as_str()?;
    let questions = part["questions"].as_array()?;
    if is_approval(questions) {
        let q = &questions[0];
        let words = q["question"].as_str().unwrap_or_default();
        let title = match q["header"].as_str().unwrap_or_default() {
            "Approve command" => "Run this command?",
            "Approve file change" => "Make this change?",
            _ => "Allow this?",
        };
        // The command, when the question quotes one ("wants to run `x`").
        let command = words.split('`').nth(1).filter(|c| !c.is_empty());
        let mut fields = vec![("title", Value::from(title))];
        match command {
            Some(c) => fields.push(("command", Value::from(c))),
            None => fields.push(("detail", Value::from(words))),
        }
        return Some(Value::record([
            ("request", Value::from(request)),
            ("kind", Value::from("approval")),
            ("approval", Value::record(fields)),
        ]));
    }
    let page = answered.min(questions.len().saturating_sub(1));
    let q = questions.get(page)?;
    let options: Vec<Value> = q["options"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Json::as_str)
        .map(|o| Value::record([("value", Value::from(o)), ("label", Value::from(o))]))
        .collect();
    let mut question = vec![
        ("text", Value::from(q["question"].as_str().unwrap_or_default())),
        ("options", Value::List(options.into())),
        ("multi", Value::Bool(q["multiSelect"].as_bool() == Some(true))),
        ("page", Value::Num((page + 1) as f64)),
        ("pages", Value::Num(questions.len() as f64)),
    ];
    if let Some(h) = q["header"].as_str().filter(|h| !h.is_empty()) {
        question.push(("header", Value::from(h)));
    }
    Some(Value::record([
        ("request", Value::from(request)),
        ("kind", Value::from("question")),
        ("question", Value::record(question)),
    ]))
}

/// Whether the open chat is said as offline.
fn chat_offline(no_connection: bool, unreachable: bool) -> bool {
    no_connection && unreachable
}

fn session_value(chats: &Chats, open: &Open, offline: bool) -> Value {
    let chat = chats.chat(&open.id);
    // Ready once its transcript has come (this device's copy, read first,
    // whatever its host's state): its rows show at once, and the chat itself
    // (its config, the composer's model) fills in as the chats list brings
    // it. Waiting on that list as well, a chat kept here stood as a skeleton
    // until the whole list came. Missing only when the list is known, hasn't
    // it, and nothing of it is here.
    let state = if open.heard && (chat.is_some() || !open.entries.is_empty() || !chats.chats_heard.get()) {
        "ready"
    } else if chats.chats_heard.get() && chat.is_none() {
        "missing"
    } else if offline {
        "offline"
    } else {
        "loading"
    };
    let chat = chat.unwrap_or(Json::Null);
    let config = &chat["config"];
    let s = |j: &Json| j.as_str().map(Value::from).unwrap_or(Value::Nil);
    let harness = config["harness"].as_str().unwrap_or("claude-code");
    let model = config["model"]
        .as_str()
        .map(|m| Value::from(format!("{harness}/{m}").as_str()))
        .unwrap_or(Value::Nil);
    // Its name, for a picker whose catalog doesn't list it (its device's not
    // looked at yet, or not offering it): the catalog's, else its id (as
    // `build_context`'s). None named: the agent's own default, its
    // catalog's first.
    let model_label = match config["model"].as_str() {
        Some(m) => Value::from(chats.label_of(harness, Some(m)).unwrap_or_else(|| m.to_owned()).as_str()),
        None => chats.label_of(harness, None).map_or(Value::Nil, |l| Value::from(l.as_str())),
    };
    let access = match config["access"].as_str() {
        Some("supervised") => "supervised",
        Some("auto-accept-edits") => "edits",
        _ => "full",
    };
    let fast = fast_in(&config["modelOptions"]);
    let options = options_value(&config["modelOptions"]);
    let answered = if open.answering.is_some() { open.answers.len() } else { 0 };
    let ask = asked(open.entries.iter().rev().filter_map(|e| e.json.as_ref())).and_then(|p| ask_value(p, answered)).unwrap_or(Value::Nil);
    let session = chats.sessions.borrow().get(&open.id).cloned().unwrap_or(Json::Null);
    let live = chats.running(&open.id);
    let tasks = background_tasks(&session);
    let queue: Vec<Value> = open
        .queue
        .iter()
        .map(|q| {
            let mut fields = vec![("id", s(&q["id"])), ("text", s(&q["text"]))];
            // When it was queued (seconds): one that waited long is said so
            // before it goes.
            if let Some(ms) = q["issuedAt"].as_f64() {
                fields.push(("at", Value::Num(ms / 1000.)));
            }
            let mine = open.editing.as_ref().is_some_and(|e| q["id"].as_str() == Some(&e.item));
            if mine {
                fields.push(("editing", Value::Bool(true)));
            }
            let gate = &q["deliveryGate"];
            if gate["kind"].as_str() == Some("editing") && !mine {
                let who = gate["ownerDeviceId"].as_str().unwrap_or_default();
                if held_elsewhere(chats.here.borrow().as_deref(), who) {
                    fields.push(("locked", Value::from("Being edited on another device")));
                }
            }
            Value::record(fields)
        })
        .collect();
    let mut fields = vec![
        ("id", Value::from(open.id.as_str())),
        ("state", Value::from(state)),
        ("title", s(&chat["title"])),
        ("project", s(&chat["spaceId"])),
        ("device", s(&chat["deviceId"])),
        // Its folder on that device (a reply's pictures resolve by it).
        ("cwd", s(&chat["cwd"])),
        ("harness", Value::from(harness)),
        ("model", model),
        ("model_label", model_label),
        ("effort", s(&config["reasoning"])),
        ("fast", Value::Bool(fast)),
        ("options", options),
        ("access", Value::from(access)),
        ("plan", Value::Bool(config["plan"].as_bool() == Some(true))),
        ("running", Value::Bool(live)),
        ("context", open.context.map(Value::Num).unwrap_or(Value::Nil)),
        ("context_tokens", open.context_usage.0.map(Value::Num).unwrap_or(Value::Nil)),
        ("context_window", open.context_usage.1.map(Value::Num).unwrap_or(Value::Nil)),
        ("ask", ask),
        ("queue", Value::List(queue.into())),
        ("waiting", waiting_value(&open.waiting)),
        // A new chat on Cloud before its machine is known (`chat.creating`).
        ("cloud", Value::Bool(chat["cloud"].as_bool() == Some(true))),
        ("todos", todos(open)),
        ("tasks", Value::List(tasks.into())),
    ];
    if let Some(branch) = chat["branch"].as_str() {
        fields.push(("branch", Value::from(branch)));
    }
    Value::record(fields)
}

/// The processes the chat's agent left running in the background, oldest
/// first: listed while its session row is fresh, whatever the turn does
/// (the shipping app's `view::background_tasks`). They outlive the turn
/// that started them, and the engine's heartbeat keeps the row fresh while
/// they run; gating them on a live turn emptied the tray each time a turn
/// ended and filled it again as the next began.
fn background_tasks(session: &Json) -> Vec<Value> {
    if stale(&session["updatedAt"]) {
        return Vec::new();
    }
    let s = |j: &Json| j.as_str().map(Value::from).unwrap_or(Value::Nil);
    session["backgroundTasks"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|t| {
            let command = t["command"].as_str().unwrap_or_default();
            let name = t["description"].as_str().filter(|d| !d.is_empty()).unwrap_or(command);
            let mut fields = vec![
                ("id", s(&t["id"])),
                ("kind", Value::from("process")),
                ("name", Value::from(name)),
            ];
            if name != command {
                fields.push(("detail", Value::from(command)));
            }
            if let Some(since) = seconds(&t["startedAt"]) {
                fields.push(("since", Value::Num(since as f64)));
            }
            Value::record(fields)
        })
        .collect()
}

/// The plan: the latest todo list the agent wrote.
fn todos(open: &Open) -> Value {
    let latest = latest_todo(open.entries.iter().rev().filter_map(|e| e.json.as_ref()))
        .or(open.older_todo.as_ref().map(|(_, part)| part));
    let Some(part) = latest else {
        return Value::List(Rc::from([]));
    };
    let items: Vec<Value> = part["call"]["items"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .map(|(i, t)| {
            let state = match t["status"].as_str() {
                Some("completed") => "done",
                Some("inProgress") => "doing",
                _ if t["done"].as_bool() == Some(true) => "done",
                _ => "pending",
            };
            Value::record([
                ("id", Value::from(format!("todo{i}").as_str())),
                ("text", Value::from(t["text"].as_str().unwrap_or_default())),
                ("state", Value::from(state)),
            ])
        })
        .collect();
    Value::List(items.into())
}

/// How long a sent prompt reads "sending" with no turn to show for it, as
/// the shipping app's grace before it calls a send undelivered.
const SEND_GRACE_SECONDS: f64 = 60.;

/// A record with more fields (or some replaced).
fn with_fields<const N: usize>(record: Value, extra: [(&str, Value); N]) -> Value {
    let Value::Map(fields) = &record else { return record };
    let mut all: Vec<(std::rc::Rc<str>, Value)> = fields.iter().cloned().collect();
    for (key, value) in extra {
        all.retain(|(k, _)| &**k != key);
        all.push((key.into(), value));
    }
    Value::record(all)
}

/// The draft for the transcript's last entry: while it streams, the words
/// being written (the reply's last text part, its block `part`, apart from
/// the rest of the reply) and the thought it is at; else none.
pub(crate) fn draft_of(e: &Json) -> Value {
    if e["status"].as_str() != Some("streaming") {
        return draft_value("", "");
    }
    let id = e["id"].as_str().unwrap_or_default();
    // Its row kept as it is now (`LiveRow`): where its last text is, without
    // looking back through the calls after it.
    let kept = e["parts"]
        .as_array()
        .filter(|_| !LIVE_ROWS_OFF.get())
        .and_then(|parts| {
            LIVE_ROWS.with_borrow(|rows| {
                let row = rows.get(id)?;
                let last = parts.last()?["id"].as_str()?;
                (row.ids.len() == parts.len() && row.ids.last().is_some_and(|l| **l == *last))
                    .then_some(row.last_text)
            })
        });
    if let Some(None) = kept
        && let Some(Value::Str(text)) =
            LIVE_ROWS.with_borrow(|rows| rows.get(id).map(|row| row.text.clone()))
    {
        return with_thought(draft_value(id, &text), e["parts"].as_array());
    }
    let tail = e["parts"].as_array().and_then(|parts| match kept {
        Some(Some(at)) => parts.get(at).filter(|p| p["kind"].as_str() == Some("text")),
        _ => parts
            .iter()
            .rev()
            .find(|p| p["kind"].as_str() == Some("text")),
    });
    let words = match tail {
        // (`text`, the tail's words too: the page reads the block's, and of
        // `text` only whether any are yet.)
        Some(tail) => Value::record([
            ("id", Value::from(id)),
            (
                "text",
                Value::from(tail["text"].as_str().unwrap_or_default()),
            ),
            ("part", Value::from(tail["id"].as_str().unwrap_or_default())),
            (
                "part_text",
                Value::from(tail["text"].as_str().unwrap_or_default()),
            ),
        ]),
        None => draft_value(id, &entry_text(e)),
    };
    with_thought(words, e["parts"].as_array())
}

fn draft_value(id: &str, text: &str) -> Value {
    Value::record([("id", Value::from(id)), ("text", Value::from(text))])
}

/// `draft` with the thought a reply is at, when its last part is one: its id
/// and its words so far, as its row shows them (`tool_live`). The row reads
/// them here, so a tick of them doesn't publish the whole transcript.
fn with_thought(draft: Value, parts: Option<&Vec<Json>>) -> Value {
    match parts.and_then(|parts| parts.last()).filter(|p| p["kind"].as_str() == Some("reasoning")) {
        Some(thought) => with_fields(
            draft,
            [
                ("thought", Value::from(thought["id"].as_str().unwrap_or_default())),
                ("thought_text", Value::from(thought_words(thought["text"].as_str().unwrap_or_default()).as_str())),
            ],
        ),
        None => draft,
    }
}

// -- small things ---------------------------------------------------------------

fn now_seconds() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// A time as Unix seconds: milliseconds since 1970, or RFC 3339.
fn seconds(value: &Json) -> Option<i64> {
    if let Some(ms) = value.as_f64() {
        return Some((ms / 1000.) as i64);
    }
    let stamp = value.as_str()?;
    let (date, time) = stamp.split_once('T')?;
    let mut date = date.split('-').map(|p| p.parse::<i64>().ok());
    let (y, m, d) = (date.next()??, date.next()??, date.next()??);
    let clock: String = time.chars().take(8).collect();
    let mut clock = clock.split(':').map(|p| p.parse::<i64>().ok());
    let (h, min, s) = (clock.next()??, clock.next()??, clock.next()??);
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let mut out = days * 86_400 + h * 3600 + min * 60 + s;
    if let Some(sign_at) = time.get(8..).and_then(|t| t.find(['+', '-'])) {
        let offset = &time[8 + sign_at..];
        let sign = if offset.starts_with('-') { -1 } else { 1 };
        let mut parts = offset[1..].split(':').map(|p| p.parse::<i64>().unwrap_or(0));
        let (oh, om) = (parts.next().unwrap_or(0), parts.next().unwrap_or(0));
        out -= sign * (oh * 3600 + om * 60);
    }
    Some(out)
}

/// Why `RemoveHeldSend` didn't answer: "unsupported" from an engine from before
/// it (which can't take a send back), else as any call's.
fn held_removal_failure(error: &str) -> Outcome {
    let lower = error.to_ascii_lowercase();
    if lower.contains("unknown method") || lower.contains("unknownmethod") {
        "unsupported"
    } else {
        engine::outcome(error)
    }
}

/// A chat's messages not yet taken by its agent: each its id, the message it
/// becomes, its words (without the attachments' trailer: the files' names,
/// when there are no words), when it went out and when it stops being run
/// (seconds, as the scripts' times are), whether it's still in this device's
/// outbox, and why it stopped trying, if it did.
fn waiting_value(items: &[Json]) -> Value {
    let secs = |v: &Json| v.as_f64().map_or(Value::Nil, |ms| Value::Num(ms / 1000.));
    let s = |v: &Json| v.as_str().filter(|t| !t.is_empty()).map_or(Value::Nil, Value::from);
    let words = |v: &Json| {
        let (words, paths) = split_attachments(v.as_str().unwrap_or_default());
        let names = paths.iter().map(|p| p.rsplit('/').next().unwrap_or(p)).collect::<Vec<_>>().join(", ");
        let shown = if words.trim().is_empty() { names } else { words };
        if shown.is_empty() { Value::Nil } else { Value::from(shown.as_str()) }
    };
    Value::List(
        items
            .iter()
            .map(|w| {
                Value::record([
                    ("id", s(&w["id"])),
                    ("message", s(&w["messageId"])),
                    ("text", words(&w["prompt"])),
                    ("sent", secs(&w["sentAt"])),
                    ("expires", secs(&w["expiresAt"])),
                    ("held", Value::Bool(w["held"].as_bool() == Some(true))),
                    ("failure", s(&w["failure"])),
                ])
            })
            .collect::<Vec<_>>()
            .into(),
    )
}

/// Whether a device's agent is one its composer offers. On a Cloud session's
/// machine, one Cloud has no account for still is: the chat already runs it, so
/// its model must read as itself (a new chat on Cloud, picked by the logical
/// device, still offers only those it can run).
fn offered(h: &Json, device: &str) -> bool {
    let session_machine = device.starts_with("cloud-");
    h["id"].as_str() != Some("mock")
        && h["installed"].as_bool() != Some(false)
        && (session_machine || h["enabled"].as_bool() != Some(false))
}

fn stale(updated: &Json) -> bool {
    seconds(updated).is_some_and(|at| now_seconds() - at > STALE_SECONDS)
}

/// Whether marking `chat` seen at `now` changes anything a viewport reads
/// from it (`session`: its live state, or null). Every reader of a chat's
/// `lastSeenAt` compares it with one of three stamps (zeron_proto's status
/// and sleep rules): its last message, its terminal activity's report, and
/// when its sleep ended. Once the mark is at or past all three, a newer one
/// changes nothing on any device, yet it is a synced write: every window
/// everywhere is sent the whole chat list again for it. Unknown (no row,
/// a stamp that doesn't read): marked, as before.
fn seen_would_change(chat: &Json, session: &Json, now: chrono::DateTime<chrono::Utc>) -> bool {
    let at = |v: &Json| -> Option<Option<chrono::DateTime<chrono::Utc>>> {
        match v {
            Json::Null => Some(None),
            Json::String(s) => chrono::DateTime::parse_from_rfc3339(s).ok().map(|t| Some(t.to_utc())),
            Json::Number(n) => n.as_i64().and_then(chrono::DateTime::from_timestamp_millis).map(Some),
            _ => None,
        }
    };
    if chat.is_null() {
        return true;
    }
    let Some(Some(seen)) = at(&chat["lastSeenAt"]) else { return true };
    let sleep = &chat["sleep"];
    // (A sleep still to end: a mark now is before it, and changes nothing.)
    let woke = match (at(&sleep["wokeAt"]), at(&sleep["until"])) {
        (Some(Some(woke)), _) => Some(woke),
        (Some(None), Some(Some(until))) => Some(until).filter(|until| *until <= now),
        (Some(None), Some(None)) => None,
        _ => return true,
    };
    let stamps = [at(&chat["lastMessageAt"]), at(&session["terminalActivity"]["updatedAt"]), Some(woke)];
    stamps.into_iter().any(|stamp| match stamp {
        None => true,
        Some(stamp) => stamp.is_some_and(|stamp| stamp >= seen),
    })
}

/// "Jul 1, 4:12 PM", in this Mac's time zone (the shipping app's
/// `format_timestamp`).
#[cfg(unix)]
fn stamp(at: i64) -> Option<String> {
    #[repr(C)]
    struct Tm {
        sec: i32,
        min: i32,
        hour: i32,
        mday: i32,
        mon: i32,
        year: i32,
        wday: i32,
        yday: i32,
        isdst: i32,
        gmtoff: std::ffi::c_long,
        zone: *const std::ffi::c_char,
    }
    unsafe extern "C" {
        fn localtime_r(time: *const i64, out: *mut Tm) -> *mut Tm;
    }
    if at <= 0 {
        return None;
    }
    let mut tm = std::mem::MaybeUninit::<Tm>::zeroed();
    // SAFETY: `localtime_r` writes the broken-down local time into the
    // caller's struct (the C library's `struct tm`, laid out as above on
    // macOS and glibc) and reads only `at`.
    let tm = unsafe {
        if localtime_r(&at, tm.as_mut_ptr()).is_null() {
            return None;
        }
        tm.assume_init()
    };
    let hour = match tm.hour % 12 {
        0 => 12,
        h => h,
    };
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let month = MONTHS.get(tm.mon as usize).copied().unwrap_or("Jan");
    Some(format!(
        "{month} {}, {hour}:{:02} {}",
        tm.mday,
        tm.min,
        if tm.hour < 12 { "AM" } else { "PM" }
    ))
}

#[cfg(not(unix))]
fn stamp(_at: i64) -> Option<String> {
    None
}

/// Whether a queued message's edit lease is another device's: only once
/// this device is known, and only for an owner that isn't it. (Unknown, a
/// lease of this device's own was said as being edited on another device.)
fn held_elsewhere(here: Option<&str>, owner: &str) -> bool {
    here.is_some_and(|here| here != owner)
}

/// This window process's id, as an editor of queued messages.
fn instance_id() -> String {
    thread_local! {
        static ID: String = new_id();
    }
    ID.with(Clone::clone)
}

/// A random UUID (v4), as the engine's own are.
fn new_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut bytes = [0u8; 16];
    for (i, chunk) in bytes.chunks_mut(8).enumerate() {
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u128(SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
        hasher.write_usize(i);
        chunk.copy_from_slice(&hasher.finish().to_le_bytes());
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &hex[0..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..32])
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = match chunk.len() {
            3 => (u32::from(chunk[0]) << 16) | (u32::from(chunk[1]) << 8) | u32::from(chunk[2]),
            2 => (u32::from(chunk[0]) << 16) | (u32::from(chunk[1]) << 8),
            _ => u32::from(chunk[0]) << 16,
        };
        let sym = |shift: u32| TABLE[((n >> shift) & 63) as usize] as char;
        out.push(sym(18));
        out.push(sym(12));
        out.push(if chunk.len() > 1 { sym(6) } else { '=' });
        out.push(if chunk.len() > 2 { sym(0) } else { '=' });
    }
    out
}

#[cfg(test)]
#[path = "chat_pending_tests.rs"]
mod pending_tests;

#[cfg(test)]
#[path = "chat_stream_tests.rs"]
mod stream_tests;

#[cfg(test)]
mod tests {
    /// Opening a chat marks it seen only while that changes something any
    /// viewport reads (its last message, its terminal's report, the end of
    /// its sleep): a chat looked at already writes nothing, so its devices'
    /// windows aren't each sent the whole chat list again for nothing.
    #[test]
    fn opening_a_chat_marks_it_seen_only_when_that_changes_something() {
        use super::seen_would_change as would;
        use serde_json::{Value as Json, json};
        let now = chrono::DateTime::parse_from_rfc3339("2026-11-07T05:00:00Z").unwrap().to_utc();
        let none = Json::Null;
        // Looked at since its last word: nothing to mark.
        let seen = json!({ "lastMessageAt": "2026-11-07T04:00:00Z", "lastSeenAt": "2026-11-07T04:30:00.250Z" });
        assert!(!would(&seen, &none, now));
        // Said something since: marked.
        let unseen = json!({ "lastMessageAt": "2026-11-07T04:40:00Z", "lastSeenAt": "2026-11-07T04:30:00Z" });
        assert!(would(&unseen, &none, now));
        // Never seen, a row not known, or a stamp that doesn't read: marked, as before.
        assert!(would(&json!({ "lastMessageAt": "2026-11-07T04:00:00Z" }), &none, now));
        assert!(would(&Json::Null, &none, now));
        assert!(would(&json!({ "lastMessageAt": "soon", "lastSeenAt": "2026-11-07T04:30:00Z" }), &none, now));
        // Its terminal reported since it was seen: marked.
        let terminal = json!({ "terminalActivity": { "updatedAt": "2026-11-07T04:45:00Z" } });
        assert!(would(&seen, &terminal, now));
        let earlier = json!({ "terminalActivity": { "updatedAt": "2026-11-07T04:10:00Z" } });
        assert!(!would(&seen, &earlier, now));
        // Woke (early, or its time passed) since it was seen: marked.
        let woke = |sleep: Json| {
            let mut chat = seen.clone();
            chat["sleep"] = sleep;
            chat
        };
        let at = "2026-11-07T03:00:00Z";
        assert!(would(&woke(json!({ "at": at, "until": "2026-11-07T06:00:00Z", "wokeAt": "2026-11-07T04:50:00Z" })), &none, now));
        assert!(would(&woke(json!({ "at": at, "until": "2026-11-07T04:55:00Z" })), &none, now));
        // Still asleep (a mark now is before its end), or woke before it was seen: nothing.
        assert!(!would(&woke(json!({ "at": at, "until": "2026-11-07T06:00:00Z" })), &none, now));
        assert!(!would(&woke(json!({ "at": at, "until": "2026-11-07T04:10:00Z" })), &none, now));
        // Milliseconds, as the doc keeps them.
        let ms = json!({ "lastMessageAt": 1_794_000_000_500_i64, "lastSeenAt": 1_794_000_000_400_i64 });
        assert!(would(&ms, &none, now));
    }

    /// A queued message's files come back to an edit as chips named as they
    /// were attached, already on the host, a picture shown where it can be.
    #[test]
    fn a_queued_file_is_a_chip_named_as_it_was_attached() {
        let chip = super::queued_attachment("/data/uploads/9e97a72f-shot.png");
        assert_eq!(chip.field("name"), Some(&Value::from("shot.png")));
        assert_eq!(chip.field("kind"), Some(&Value::from("image")));
        assert_eq!(chip.field("on_host"), Some(&Value::Bool(true)));
        assert_eq!(chip.field("src"), Some(&Value::from("/data/uploads/9e97a72f-shot.png")));
        let file = super::queued_attachment("/repo/notes/design-notes.txt");
        assert_eq!(file.field("name"), Some(&Value::from("design-notes.txt")), "only an upload's id prefix goes");
        assert_eq!(file.field("kind"), Some(&Value::from("file")));
        assert_eq!(file.field("src"), None);
        let remote = super::queued_attachment("pending://up-1/photo.jpg");
        assert_eq!(remote.field("name"), Some(&Value::from("photo.jpg")));
        assert_eq!(remote.field("kind"), Some(&Value::from("image")));
        assert_eq!(remote.field("src"), None, "nothing here to show yet");
        assert_eq!(remote.field("path"), Some(&Value::from("pending://up-1/photo.jpg")));
    }

    #[test]
    fn a_reset_keeps_what_it_left_as_it_was() {
        let built_of = |ids: &[&str]| -> std::collections::HashMap<Rc<str>, super::Built> {
            ids.iter().map(|id| (Rc::from(*id), super::Built::default())).collect()
        };
        let mut before: Vec<super::Entry> = [
            serde_json::json!({ "id": "a", "text": "same" }),
            serde_json::json!({ "id": "b", "text": "was" }),
            serde_json::json!({ "id": "gone", "text": "left" }),
        ]
        .into_iter()
        .map(super::Entry::new)
        .collect();
        // (Settled: told from its sum alone.)
        let mut no_plan = None;
        super::settle(&mut before[0], 0, &mut no_plan);
        assert!(before[0].json.is_none());
        let now: Vec<super::Entry> = [
            serde_json::json!({ "id": "a", "text": "same" }),
            serde_json::json!({ "id": "b", "text": "now" }),
            serde_json::json!({ "id": "new", "text": "came" }),
        ]
        .into_iter()
        .map(super::Entry::new)
        .collect();
        let mut built = built_of(&["a", "b", "gone"]);
        super::keep_unchanged(&mut built, &before, &now);
        assert!(built.contains_key("a"));
        assert!(!built.contains_key("b"), "changed");
        assert!(!built.contains_key("gone"), "left the transcript");
        assert!(!built.contains_key("new"), "never built");
    }

    #[test]
    fn a_settled_entry_keeps_its_plan_and_grows_only_from_a_whole_copy() {
        let plan = |text: &str| {
            serde_json::json!({ "id": text, "role": "assistant", "parts": [
                { "id": "p", "kind": "tool", "call": { "kind": "todo", "items": [{ "text": text, "status": "pending" }] } },
            ] })
        };
        let mut older = None;
        let (mut first, mut second) = (super::Entry::new(plan("first")), super::Entry::new(plan("second")));
        // (Newest first, as a long chat's rows are built, and the other way.)
        super::settle(&mut second, 1, &mut older);
        super::settle(&mut first, 0, &mut older);
        assert_eq!(older.as_ref().unwrap().1["call"]["items"][0]["text"], "second");
        let open = super::Open { entries: vec![first, second], older_todo: older, ..super::Open::default() };
        assert_eq!(super::todos(&open).list()[0].field("text"), Some(&Value::from("second")));
        // Words for an entry whose copy is gone: out of step, so a reset comes.
        let mut entries = open.entries;
        let mut words = serde_json::json!({ "append": [{ "entry": "first", "part": "p", "text": "x", "len": 1 }] });
        assert!(super::apply(&mut entries, &mut words).is_err());
    }

    /// The worker's fold, kept, is what folding here keeps.
    #[test]
    fn the_worker_folds_as_the_window_would() {
        let entry = serde_json::json!({
            "id": "e1", "role": "assistant", "status": "complete", "createdAt": 1000.0,
            "parts": [
                { "id": "t", "kind": "text", "text": "Some words" },
                { "id": "k", "kind": "tool", "subagentRef": "doc--sub--1", "subagentStatus": "done",
                  "call": { "kind": "mcp", "tool": "Agent: look", "server": "s" } },
            ],
        });
        let context = Rc::new(super::RowContext { doc: "chat".into(), device: None, model: Some("Opus".into()) });
        let here = super::built_for(&entry, &context, true);
        let folding = super::folding(vec![(0, "e1".into(), entry.clone())], &context);
        let folded = folding.results.recv().unwrap().pop().unwrap();
        let mut open = super::Open { entries: vec![super::Entry::new(entry.clone())], ..super::Open::default() };
        open.entries[0].json = None;
        super::settle_off(&mut open, folded, &context);
        let there = open.built.get("e1").expect("kept");
        assert_eq!(there.row.as_ref().unwrap().to_json(), here.row.as_ref().unwrap().to_json());
        assert_eq!(there.subagents.as_ref().map(|s| s[0].to_json()), here.subagents.as_ref().map(|s| s[0].to_json()));
        assert_eq!(open.entries[0].sum, super::sum_of(&entry));
    }

    /// A folded row's bytes take their own length, not LZ4's worst-case
    /// buffer: rows compressing as real ones do (about a third smaller) kept
    /// that buffer, as a shrinking realloc left it where it was.
    #[test]
    fn a_folded_rows_bytes_take_their_own_length() {
        unsafe extern "C" {
            // mimalloc's own, the program's allocator (zgx-desktop's `mimalloc`).
            fn mi_usable_size(p: *const std::ffi::c_void) -> usize;
        }
        // Words drawn from a small vocabulary, as a transcript's are: they
        // compress, but not by half.
        let words = ["the", "row", "tool", "output", "src/chat.rs", "cargo", "test", "fold", "error", "warning", "fn", "let"];
        let (mut seed, mut text) = (7u32, String::new());
        for _ in 0..400 {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            text.push_str(words[(seed >> 16) as usize % words.len()]);
            text.push(if seed & 0x100 == 0 { ' ' } else { '\n' });
            text.push_str(&((seed >> 8) % 1000).to_string());
        }
        let raw = serde_json::to_vec(&serde_json::json!({ "id": "e1", "role": "assistant", "text": text })).unwrap();
        let folded = super::compressed(&raw);
        assert!(folded.len() > raw.len() / 2 && folded.len() < raw.len(), "a ratio like real rows': {} of {}", folded.len(), raw.len());
        // SAFETY: mimalloc reports the size of the block it gave the box.
        let held = unsafe { mi_usable_size(folded.as_ptr().cast()) };
        assert!(held <= folded.len() + folded.len() / 8 + 16, "held {held} for {} bytes ({} raw)", folded.len(), raw.len());
    }

    /// A row out of view holds what makes it, not itself: what it opens to is
    /// the row it would have been, and a row with subagents still lists them.
    #[test]
    fn a_folded_row_opens_to_the_row_it_was() {
        let entry = serde_json::json!({
            "id": "e1", "role": "assistant", "status": "complete", "createdAt": 1000.0,
            "parts": [
                { "id": "t", "kind": "text", "text": "Some words ".repeat(200) },
                { "id": "k", "kind": "tool", "subagentRef": "doc--sub--1", "subagentStatus": "done",
                  "call": { "kind": "mcp", "tool": "Agent: look", "server": "s" } },
            ],
        });
        let context = Rc::new(super::RowContext { doc: "chat".into(), device: None, model: Some("Opus".into()) });
        let whole = super::built_for(&entry, &context, false);
        let folded = super::built_for(&entry, &context, true);
        let Some(Value::Folded(_)) = folded.row else { panic!("folded") };
        let (whole_row, folded_row) = (whole.row.unwrap(), folded.row.unwrap());
        // What a script gets, and what it reads of it.
        assert_eq!(folded_row.to_json(), whole_row.to_json());
        assert_eq!(folded_row.field("id").map(text).as_deref(), Some("e1"));
        assert_eq!(folded.subagents.as_deref().map(<[Value]>::len), Some(1));
        assert_eq!(folded.subagents, whole.subagents);
        // It's kept small: words that repeat, a good deal less than they are.
        let streaming = serde_json::json!({ "id": "e2", "role": "assistant", "status": "streaming", "parts": [] });
        assert!(!matches!(super::built_for(&streaming, &context, true).row, Some(Value::Folded(_))), "still being written");
    }

    #[test]
    fn what_a_kept_compressor_compresses_opens_again() {
        let long = "words that repeat ".repeat(4000);
        for input in [long.as_bytes(), b"short", b""] {
            let out = super::compressed(input);
            assert_eq!(lz4_flex::block::decompress_size_prepended(&out).unwrap(), input);
        }
        assert!(super::compressed(long.as_bytes()).len() < long.len() / 10);
    }

    #[test]
    fn a_row_holds_each_of_its_repeats_once() {
        let tool = || Value::record([("id", Value::from("t1")), ("kind", Value::from("exec")), ("call", Value::from("cargo build"))]);
        let words = "the same words";
        let mut row = Value::record([
            ("text", Value::from(words)),
            ("copy", Value::from(words)),
            ("tools", Value::List(Rc::from([tool()]))),
            ("blocks", Value::List(Rc::from([Value::record([("kind", Value::from("group")), ("tools", Value::List(Rc::from([tool()])))])]))),
        ]);
        let before = row.clone();
        // (Built just now: nothing else holds its parts.)
        drop(before.clone());
        let same = row == before;
        drop(before);
        super::share_parts(&mut row, &mut super::Seen::default());
        assert!(same);
        let tools = row.field("tools").unwrap().list();
        let grouped = row.field("blocks").unwrap().list()[0].field("tools").unwrap().list();
        let (Value::Map(one), Value::Map(two)) = (&tools[0], &grouped[0]) else { panic!("records") };
        assert!(Rc::ptr_eq(one, two), "one copy of a tool");
        let (Some(Value::Str(text)), Some(Value::Str(copy))) = (row.field("text"), row.field("copy")) else { panic!("words") };
        assert!(Rc::ptr_eq(text, copy), "one copy of its words");
    }

    /// A settled reply's tools are built once, for all its layouts, and read
    /// exactly as they would built for each.
    #[test]
    fn a_settled_row_builds_each_tool_once_and_reads_the_same() {
        let entry = serde_json::json!({
            "id": "e", "role": "assistant", "status": "complete", "createdAt": 1000., "durationMs": 5000.,
            "parts": [
                { "id": "r", "kind": "reasoning", "text": "thinking it over" },
                { "id": "t", "kind": "tool", "call": { "kind": "exec", "command": "cargo build" }, "output": "ok" },
                { "id": "w", "kind": "text", "text": "Done." },
            ],
        });
        let row = super::built_entry(&entry, "doc", None, None).unwrap();
        assert_eq!(row, super::message(&entry, None).unwrap());
        let tools = row.field("tools").unwrap().list();
        let grouped = row.field("blocks").unwrap().list()[0].field("tools").unwrap().list();
        let (Value::Map(one), Value::Map(two)) = (&tools[1], &grouped[1]) else { panic!("records") };
        assert!(Rc::ptr_eq(one, two), "one tool, in both layouts");
        // (Built for this row alone: nothing's kept for the next.)
        assert!(super::ROW_BUILD.with_borrow(Option::is_none));
    }

    #[test]
    fn another_views_rows_are_taken_only_as_they_are_here() {
        let entries: Vec<super::Entry> = [
            serde_json::json!({ "id": "a", "text": "same" }),
            serde_json::json!({ "id": "b", "text": "theirs" }),
        ]
        .into_iter()
        .map(super::Entry::new)
        .collect();
        let mut open = super::Open {
            id: "chat".into(),
            entries: [
                serde_json::json!({ "id": "a", "text": "same" }),
                serde_json::json!({ "id": "b", "text": "mine" }),
            ]
            .into_iter()
            .map(super::Entry::new)
            .collect(),
            ..super::Open::default()
        };
        let theirs: std::collections::HashMap<Rc<str>, super::Built> =
            [(Rc::from("a"), super::Built::default()), (Rc::from("b"), super::Built::default())].into_iter().collect();
        // (Built for another model's name: none of it.)
        open.built_model = Some(Some("mine".into()));
        super::reuse_built(&mut open, &entries, &theirs, &Some(Some("theirs".into())));
        assert!(open.built.is_empty());
        open.built_model = None;
        super::reuse_built(&mut open, &entries, &theirs, &Some(Some("theirs".into())));
        assert!(open.built.contains_key("a"));
        assert!(!open.built.contains_key("b"));
        assert_eq!(open.built_model, Some(Some("theirs".into())));
    }


    #[test]
    fn a_sent_pick_shows_its_page_over_its_number_and_words() {
        let pick = Pick {
            index: 2,
            element: "button#export.btn".into(),
            url: "https://www.example.com/dashboard/revenue?tab=1".into(),
            words: "Export report".into(),
        };
        let chip = annotation_chip(&pick);
        assert!(matches!(chip.field("name"), Some(Value::Str(s)) if s.as_ref() == "example.com/dashboard/revenue?tab=1"));
        assert!(matches!(chip.field("meta"), Some(Value::Str(s)) if s.as_ref() == "Annotation 2 · Export report"));
        assert_eq!(page_of("http://localhost:3000/"), "localhost:3000");
        let bare = Pick { index: 1, element: "div.card".into(), url: String::new(), words: String::new() };
        assert!(matches!(annotation_chip(&bare).field("name"), Some(Value::Str(s)) if s.as_ref() == "Annotation 1"));
    }

    #[test]
    fn a_subagent_spawn_carries_its_doc_status_and_title() {
        let spawn = serde_json::json!({
            "id": "t1", "kind": "tool",
            "call": { "kind": "unknown", "name": "Agent: scan the repo", "input": { "prompt": "Look around" } },
            "subagentRef": "chat-1--sub--7", "subagentStatus": "running",
        });
        let row = super::tool(&spawn).unwrap();
        let sub = row.field("subagent").unwrap();
        assert_eq!(sub.field("ref").map(crate::auth::text).as_deref(), Some("chat-1--sub--7"));
        assert_eq!(sub.field("status").map(crate::auth::text).as_deref(), Some("running"));
        assert_eq!(sub.field("title").map(crate::auth::text).as_deref(), Some("scan the repo"));
        // A spawn the engine hasn't stamped yet, and any other tool: none.
        let unstamped = serde_json::json!({ "id": "t2", "kind": "tool", "call": { "kind": "unknown", "name": "Agent" } });
        assert!(super::tool(&unstamped).unwrap().field("subagent").is_none());
        let read = serde_json::json!({ "id": "t3", "kind": "tool", "call": { "kind": "readFile", "path": "a.rs" }, "subagentRef": "x" });
        assert!(super::tool(&read).unwrap().field("subagent").is_none());
    }

    /// A side chat made here and sent to at once (before the engine's list
    /// has it): its sender knows it as it was made, its parent's agent and
    /// folder, never a config with no agent (which the engine refuses as
    /// bad params: the user's "Your message didn't send", Nov 2).
    #[test]
    fn a_chat_just_made_is_sent_as_it_was_made() {
        let id = "77777777-7777-4777-8777-777777777777";
        let params = serde_json::json!({
            "op": "createChat",
            "chatId": id,
            "parentChatId": "p",
            "deviceId": "d",
            "spaceId": "s",
            "cwd": "/repo",
            "config": { "harness": "devin", "model": "swe-2-high", "reasoning": "high", "modelOptions": {}, "sandbox": "workspace-write" },
        });
        super::made(&params);
        let row = super::CHATS.with(|c| c.chat(id)).expect("known at once");
        assert_eq!(row["cwd"], "/repo");
        assert_eq!(row["parentChatId"], "p");
        assert!(row.get("op").is_none() && row.get("chatId").is_none());
        let none = zgx_script::Value::from_json("{}").unwrap();
        let config = super::merged_config(&row["config"], &none);
        assert_eq!(config["harness"], "devin");
        assert_eq!(config["model"], "swe-2-high");
        // Unknown, with nothing picked: no agent (the sender stops there).
        assert!(super::merged_config(&serde_json::Value::Null, &none)["harness"].is_null());
        // The composer's pick wins.
        let picked = zgx_script::Value::from_json(r#"{"model":"codex/gpt-6.1"}"#).unwrap();
        let config = super::merged_config(&row["config"], &picked);
        assert_eq!((config["harness"].as_str(), config["model"].as_str()), (Some("codex"), Some("gpt-6.1")));
    }

    #[test]
    fn a_finished_subagent_s_upload_is_its_transcript() {
        assert_eq!(super::blob_ref(Some("c2"), "c1--sub--3"), "c1/c1--sub--3", "a copied chip keeps its first chat");
        assert_eq!(super::blob_ref(Some("c2"), "odd-doc"), "c2/odd-doc");
        let reply = serde_json::json!({ "text": "[{\"id\":\"e1\",\"role\":\"assistant\"}]" });
        assert_eq!(super::snapshot_frame(&reply).unwrap()["reset"][0]["id"], "e1");
        assert!(super::snapshot_frame(&serde_json::json!({ "text": "not json" })).is_none());
        assert!(super::snapshot_frame(&serde_json::json!({})).is_none());
    }

    #[test]
    fn a_thought_s_row_gets_all_its_words_and_folds_them_itself() {
        let part = json!({ "id": "r", "kind": "reasoning", "text": "Look at the parser.\nThen the host." });
        let row = super::tool(&part).unwrap();
        assert!(matches!(row.field("verb"), Some(Value::Str(s)) if s.as_ref() == "Thought process"));
        assert!(matches!(row.field("call"), Some(Value::Str(s)) if s.as_ref() == "Look at the parser.\nThen the host."));
        assert!(matches!(row.field("prose"), Some(Value::Bool(true))));
        // Settled, its fold shows its first lines; being written, its latest.
        assert!(matches!(row.field("live"), Some(Value::Bool(false))));
        let writing = super::tool_live(&part, true).unwrap();
        assert!(matches!(writing.field("live"), Some(Value::Bool(true))));
        assert!(super::tool(&json!({ "id": "e", "kind": "reasoning", "text": "  " })).is_none());
        // Long: every line reaches the row (it folds them under "Show all").
        let long: Vec<String> = (1..=30).map(|n| format!("line {n}")).collect();
        let words = super::thought_words(&format!("\n{}\n", long.join("\n")));
        assert!(words.starts_with("line 1\n") && words.ends_with("\nline 30"), "{words}");
        assert_eq!(words.lines().count(), 30);
    }

    /// 2026-11-11, work-laptop: a queued message on the Mac hosting it said
    /// "being edited on another device" while the app didn't know yet which
    /// device it was on. Only a known, different owner is another device.
    #[test]
    fn only_a_different_owner_is_another_device() {
        assert!(super::held_elsewhere(Some("mac"), "phone"));
        assert!(!super::held_elsewhere(Some("mac"), "mac"));
        assert!(!super::held_elsewhere(None, "mac"));
    }

    #[test]
    fn another_devices_catalog_is_the_composers_and_names_its_device() {
        let chats = Rc::new(Chats::default());
        *chats.here.borrow_mut() = Some("mac".into());
        *chats.harnesses.borrow_mut() = vec![json!({ "id": "codex", "name": "Codex" })];
        chats.models.borrow_mut().insert("codex".into(), vec![json!({ "id": "gpt", "label": "GPT" })]);
        let local = chats.catalog_value();
        assert!(local.field("device").is_none());
        // On the box: its agents, once listed (looking until then).
        *chats.remote.borrow_mut() = Some(Remote {
            device: "box".into(),
            harnesses: vec![json!({ "id": "claude-code", "name": "Claude Code" })],
            models: HashMap::new(),
            ready: false,
        });
        let waiting = chats.catalog_value();
        assert!(matches!(waiting.field("state"), Some(Value::Str(s)) if s.as_ref() == "looking"));
        assert!(matches!(waiting.field("device"), Some(Value::Str(s)) if s.as_ref() == "box"));
        if let Some(r) = chats.remote.borrow_mut().as_mut() {
            r.models.insert("claude-code".into(), vec![json!({ "id": "opus", "label": "Opus" })]);
            r.ready = true;
        }
        let there = chats.catalog_value();
        let Some(Value::List(harnesses)) = there.field("harnesses") else { panic!("harnesses") };
        assert_eq!(harnesses.len(), 1);
        assert!(matches!(harnesses[0].field("id"), Some(Value::Str(s)) if s.as_ref() == "claude-code"));
        // Labels: this device's, else the box's.
        assert_eq!(chats.label_of("codex", Some("gpt")).as_deref(), Some("GPT"));
        assert_eq!(chats.label_of("claude-code", Some("opus")).as_deref(), Some("Opus"));
        // Back here: this device's again.
        chats.catalog_on("mac");
        assert!(chats.remote.borrow().is_none());
        assert!(chats.catalog_value().field("device").is_none());
    }

    /// A look at another device's catalog answers for itself alone: once the
    /// composer has moved on (another device, or the same one looked at
    /// again since), what it hears changes nothing.
    #[test]
    fn only_the_latest_look_at_a_devices_catalog_counts() {
        let chats = Chats::default();
        *chats.remote.borrow_mut() = Some(Remote {
            device: "laptop".into(),
            harnesses: Vec::new(),
            models: HashMap::new(),
            ready: false,
        });
        chats.remote_turn.set(2);
        assert!(chats.on_remote("laptop", 2));
        assert!(!chats.on_remote("laptop", 1), "an earlier look at the same device");
        assert!(!chats.on_remote("linux", 2), "another device's");
        chats.remote.borrow_mut().take();
        assert!(!chats.on_remote("laptop", 2), "back here");
    }

    /// A look at another device's catalog that goes unanswered (the device
    /// offline) settles: complete with what it has, nothing listed here, never
    /// this device's in its place, so the picker stops waiting on it. Only
    /// the latest look settles, and once settled it stays complete.
    #[test]
    fn an_unanswered_look_at_a_devices_catalog_settles_with_what_it_has() {
        let chats = Chats::default();
        *chats.here.borrow_mut() = Some("mac".into());
        *chats.harnesses.borrow_mut() = vec![json!({ "id": "codex", "name": "Codex" })];
        chats.models.borrow_mut().insert("codex".into(), vec![json!({ "id": "gpt", "label": "GPT" })]);
        *chats.remote.borrow_mut() = Some(Remote {
            device: "box".into(),
            harnesses: Vec::new(),
            models: HashMap::new(),
            ready: false,
        });
        chats.remote_turn.set(2);
        let complete = |chats: &Chats| matches!(chats.catalog_value().field("complete"), Some(Value::Bool(true)));
        assert!(!complete(&chats), "looking");
        chats.settle_remote("box", 1);
        assert!(!complete(&chats), "an earlier look settles nothing");
        chats.settle_remote("box", 2);
        assert!(complete(&chats), "settled");
        let settled = chats.catalog_value();
        assert!(matches!(settled.field("device"), Some(Value::Str(s)) if s.as_ref() == "box"));
        let Some(Value::List(harnesses)) = settled.field("harnesses") else { panic!("harnesses") };
        assert!(harnesses.is_empty(), "nothing listed: never this device's in its place");
        assert!(chats.settled(2) && !chats.settled(3));
    }

    /// One looked at ahead of time without all its agents' models is kept as
    /// not ready, and gives way to a whole one; a whole one stays.
    #[test]
    fn a_catalog_short_of_an_agents_models_gives_way_to_a_whole_one() {
        let remote = |ready: bool, model: &str| Remote {
            device: "laptop".into(),
            harnesses: vec![json!({ "id": "codex" })],
            models: [("codex".to_owned(), vec![json!({ "id": model })])].into_iter().collect(),
            ready,
        };
        let mut known = HashMap::new();
        keep_known(&mut known, remote(false, "short"));
        assert!(!known["laptop"].ready);
        keep_known(&mut known, remote(true, "whole"));
        assert!(known["laptop"].ready);
        keep_known(&mut known, remote(true, "later"));
        keep_known(&mut known, remote(false, "later"));
        assert_eq!(known["laptop"].models["codex"][0]["id"], "whole");
    }

    /// A chat on another device names its model before (or without) that
    /// device's catalog: its id, then the catalog's name once one has it.
    #[test]
    fn a_sessions_model_is_named_without_its_devices_catalog() {
        let chats = Chats::default();
        chats.chats.borrow_mut().insert(
            "c".into(),
            json!({ "id": "c", "deviceId": "laptop", "config": { "harness": "pi", "model": "kimi-k3" } }),
        );
        let open = Open { id: "c".into(), ..Open::default() };
        let label = |chats: &Chats| session_value(chats, &open, false).field("model_label").map(text);
        assert_eq!(label(&chats).as_deref(), Some("kimi-k3"));
        chats.known_remotes.borrow_mut().insert(
            "laptop".into(),
            Remote {
                device: "laptop".into(),
                harnesses: Vec::new(),
                models: [("pi".to_owned(), vec![json!({ "id": "kimi-k3", "label": "Kimi K3" })])].into_iter().collect(),
                ready: true,
            },
        );
        assert_eq!(label(&chats).as_deref(), Some("Kimi K3"));
        let model = session_value(&chats, &open, false).field("model").map(text);
        assert_eq!(model.as_deref(), Some("pi/kimi-k3"));
    }

    #[test]
    fn back_takes_a_questions_last_page_off_and_gives_its_pick() {
        let mut open = Open { id: "c".into(), ..Open::default() };
        assert_eq!(open.back_a_page("c", "q"), None, "nothing answered yet");
        open.answering = Some("q".into());
        open.answers = vec![("branch".into(), vec!["fix".into()]), ("who".into(), vec!["ada".into()])];
        assert_eq!(open.back_a_page("other", "q"), None);
        assert_eq!(open.back_a_page("c", "r"), None);
        assert_eq!(open.back_a_page("c", "q"), Some(vec!["ada".to_owned()]));
        assert_eq!(open.answers.len(), 1, "the page before stays answered");
        assert_eq!(open.back_a_page("c", "q"), Some(vec!["fix".to_owned()]));
        assert_eq!(open.back_a_page("c", "q"), None, "on the first page");
    }

    #[test]
    fn subagent_titles_follow_the_shipping_app() {
        let t = |call: serde_json::Value| super::subagent_title(&call);
        assert_eq!(t(serde_json::json!({ "kind": "unknown", "name": "Agent: scan repo" })), "scan repo");
        // A bare genus has nothing to say: its description, then its prompt.
        assert_eq!(t(serde_json::json!({ "kind": "unknown", "name": "Agent", "input": { "description": "Find the flake" } })), "Find the flake");
        assert_eq!(t(serde_json::json!({ "kind": "mcp", "tool": "Task", "input": { "prompt": "\n  Read lib.rs\nthen more" } })), "Read lib.rs");
        assert_eq!(t(serde_json::json!({ "kind": "unknown", "name": "Taskmaster" })), "Taskmaster");
        assert_eq!(t(serde_json::json!({ "kind": "exec" })), "Subagent");
        let long = "x".repeat(50);
        assert_eq!(t(serde_json::json!({ "kind": "unknown", "name": long })).chars().count(), 41);
    }

    use super::*;

    /// The activity fixture (ui/activity_fixture.luau), headless.
    fn activity_app() -> zgx_script::testing::TestApp {
        let entry = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ui/activity_fixture.luau");
        let app = zgx_script::testing::TestApp::new(|runtime| ScriptHost::new(runtime, entry));
        app.frames(10);
        app
    }

    const TOGGLE: &str = "role:button \"Read 1 file · ran 1 command\"";

    #[test]
    fn a_press_on_a_groups_toggle_selects_no_text() {
        let mut app = activity_app();
        app.click(TOGGLE);
        app.frames(20);
        assert!(!app.ui.has_text_selection(), "a click on the toggle selected text");
        app.click(TOGGLE);
        app.frames(20);
        app.double_click(TOGGLE);
        app.frames(20);
        assert!(!app.ui.has_text_selection(), "a double click on the toggle selected text");
    }

    /// The group open (its agent working), then closed by a press, a double
    /// press, a press that drags off: the fold moves the words under the
    /// pointer as it closes.
    #[test]
    fn closing_an_open_group_selects_no_text() {
        let mut app = activity_app();
        app.host.shared("activity_live", zgx_script::Value::Bool(false)).set(zgx_script::Value::Bool(true));
        app.frames(20);
        app.click(TOGGLE);
        app.frames(30);
        assert!(!app.ui.has_text_selection(), "closing the group with a click selected text");
        app.double_click(TOGGLE);
        app.frames(30);
        assert!(!app.ui.has_text_selection(), "a double click on the toggle selected text");
        app.drag(TOGGLE, "~\"And more words\"");
        app.frames(30);
        assert!(!app.ui.has_text_selection(), "a drag from the toggle selected text");
    }

    /// The same in a session's transcript (ui/chat_fixture.luau): its
    /// selectable list of measured rows, a reply's group folding in it.
    #[test]
    fn toggling_a_group_in_the_transcript_selects_no_text() {
        let entry = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ui/chat_fixture.luau");
        let mut app = zgx_script::testing::TestApp::new(|runtime| ScriptHost::new(runtime, entry));
        app.frames(10);
        app.host.run_command("go_s1");
        app.frames(30);
        app.host.run_command("start_reply");
        app.host.run_command("add_tool");
        app.host.run_command("add_tool");
        app.frames(30);
        let toggle = "role:button \"Ran 2 commands\"";
        for _ in 0..3 {
            app.click(toggle);
            app.frames(30);
            assert!(!app.ui.has_text_selection(), "a click on the toggle selected text");
        }
        app.double_click(toggle);
        app.frames(30);
        assert!(!app.ui.has_text_selection(), "a double click on the toggle selected text");
        app.drag(toggle, "~\"Part 30 works\"");
        app.frames(30);
        assert!(!app.ui.has_text_selection(), "a drag from the toggle selected text");
    }

    /// The interface as boxes, without node ids (a rebuild makes new
    /// ones): what a person sees move, appear or go.
    /// A tree line's box: its top and height.
    fn top_and_height(line: &str) -> Option<(f32, f32)> {
        let words: Vec<&str> = line.split_whitespace().collect();
        words.windows(2).rev().find_map(|pair| {
            let (_, y) = pair[0].split_once(',')?;
            let (_, h) = pair[1].split_once('x')?;
            Some((y.parse().ok()?, h.parse().ok()?))
        })
    }

    fn boxes(app: &mut zgx_script::testing::TestApp) -> Vec<String> {
        let tree = app.tree();
        // The window's height (the root's box): what lies wholly above or
        // below it isn't seen, and moves no one sees.
        let window = tree.lines().next().and_then(top_and_height).map_or(f32::MAX, |(_, h)| h);
        tree.lines()
            // (The transcript's scrollbar shows only while it scrolls, and
            // goes once the view rests, on the clock: the frames here don't
            // run that clock, so it is still out from opening the session.)
            .filter(|line| !line.contains("#transcript-rail"))
            .filter(|line| top_and_height(line).is_none_or(|(y, h)| y + h > 0. && y < window))
            .map(|line| {
                let line = line.trim_start();
                match line.split_once(' ') {
                    Some((id, rest)) if id.starts_with('@') => rest.to_owned(),
                    _ => line.to_owned(),
                }
            })
            .collect()
    }

    /// What moved between two pictures of the interface, line by line.
    /// Two tree lines the same but for rounding: the tree prints boxes in
    /// whole points, so a node a hair either side of a half point (a pill
    /// at 156.9993 and at 157.0, its centred words at 163.4993 and 163.5)
    /// prints a point apart without moving.
    fn same_but_rounding(a: &str, b: &str) -> bool {
        let split = |s: &str| -> (String, Vec<i64>) {
            let mut shape = String::new();
            let mut numbers = Vec::new();
            let mut digits = String::new();
            for c in s.chars().chain(std::iter::once(' ')) {
                if c.is_ascii_digit() || (c == '-' && digits.is_empty()) {
                    digits.push(c);
                    continue;
                }
                if let Ok(n) = digits.parse::<i64>() {
                    numbers.push(n);
                    shape.push('#');
                } else {
                    shape.push_str(&digits);
                }
                digits.clear();
                shape.push(c);
            }
            (shape, numbers)
        };
        let ((sa, na), (sb, nb)) = (split(a), split(b));
        sa == sb && na.len() == nb.len() && na.iter().zip(&nb).all(|(x, y)| (x - y).abs() <= 1)
    }

    fn moved(before: &[String], after: &[String]) -> Vec<String> {
        let mut out = Vec::new();
        for i in 0..before.len().max(after.len()) {
            let (a, b) = (before.get(i), after.get(i));
            let same = match (a, b) {
                (Some(a), Some(b)) => same_but_rounding(a, b),
                _ => false,
            };
            if !same {
                out.push(format!("{a:?} -> {b:?}"));
            }
        }
        out
    }

    /// A save reloads the scripts, which rebuilds the interface from kept
    /// state: nothing should move, rise or fade for it, on a new chat or a
    /// session.
    #[test]
    fn a_reload_moves_nothing() {
        let entry = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ui/chat_fixture.luau");
        let mut app = zgx_script::testing::TestApp::new(|runtime| ScriptHost::new(runtime, entry));
        // A new chat; a session; its composer's queue tray; an approval in
        // the composer; a reply streaming with its group open and
        // the working line; a reply's groups; long folded messages; a view
        // scrolled up, the way back to the latest out.
        let scenes: [(&str, &[&str]); 9] = [
            ("home", &["go_home"]),
            ("session", &["go_s1"]),
            ("queue tray", &["go_s2", "queue_one"]),
            ("approval", &["go_s2", "ask_approval"]),
            ("streaming", &["go_s1", "start_reply", "add_tool", "stream_words"]),
            ("groups", &["go_s5"]),
            ("long messages", &["go_s4"]),
            ("scrolled up", &["go_s1"]),
            // Replies far taller than a row's estimate: the view put back
            // among rows not yet measured, a reply's lane out under the
            // pointer.
            ("scrolled up among tall replies", &["go_s7"]),
        ];
        for (route, commands) in scenes {
            for command in commands {
                app.host.run_command(command);
                app.frames(10);
            }
            if route == "scrolled up" {
                app.scroll("#transcript", -600.);
            }
            if route == "scrolled up among tall replies" {
                // Far up, several replies' worth, then a reply hovered.
                for _ in 0..6 {
                    app.scroll("#transcript", -400.);
                    app.frames(5);
                }
                app.frames(30);
                app.hover("#transcript");
            }
            app.frames(60);
            // (What settles on the clock, a folded group's rows let go, a
            // scroll's rest: let it, as a person waits before a save.)
            // Twice: what the first wait's timers start (the way back to
            // the latest rising in once the view is away) runs its course.
            for _ in 0..2 {
                std::thread::sleep(std::time::Duration::from_millis(1000));
                app.frames(60);
            }
            let settled = boxes(&mut app);
            let _ = app.host.reload();
            let mut changes = Vec::new();
            for frame in 1..=40 {
                app.frame();
                let now = boxes(&mut app);
                let diff = moved(&settled, &now);
                if !diff.is_empty() {
                    changes.push(format!("{route}, frame {frame}: {}", diff.join("; ")));
                }
            }
            assert!(changes.is_empty(), "a reload moved things:\n{}", changes[..changes.len().min(6)].join("\n"));
        }
    }

    #[test]
    fn a_double_click_on_the_words_still_selects_them() {
        let mut app = activity_app();
        app.double_click("~\"Some reply text\"");
        app.frames(5);
        assert!(app.ui.has_text_selection(), "the reply's words should stay selectable");
    }

    #[test]
    fn review_comments_part_from_the_prompt_as_one_chip() {
        let text = "Fix these\n\nReview comments (each cites the workspace file and line it belongs to):\n- src/app.rs:12: Rename this.\n  It reads badly.\n- docs/a: b.md:3: Typo: here";
        let (words, comments) = split_comments(text);
        assert_eq!(words, "Fix these");
        assert_eq!(
            comments,
            vec![
                ("src/app.rs:12".to_owned(), "Rename this.\nIt reads badly.".to_owned()),
                ("docs/a: b.md:3".to_owned(), "Typo: here".to_owned()),
            ]
        );
        let chip = comments_chip(&comments);
        assert!(matches!(chip.field("name"), Some(Value::Str(s)) if s.as_ref() == "2 comments"));
        assert!(matches!(chip.field("meta"), Some(Value::Str(s)) if s.as_ref() == "src/app.rs:12, docs/a: b.md:3"));
        // The header mid-prompt, not a trailing block: left alone.
        let plain = "See Review comments (each cites the workspace file and line it belongs to): later";
        assert_eq!(split_comments(plain), (plain.to_owned(), Vec::new()));
    }

    #[test]
    fn comments_on_a_diff_part_from_the_prompt_with_their_sides_and_ranges() {
        // As screens/chat/comments writes them (production's
        // COMMENT_BLOCK_HEADER): a side each, a range, the code fenced under.
        let text = "Address the review comments below.\n\nComments on the diff (each cites the file and line it belongs to; L = line number in the original file, R = in the changed file):\n- src/a.rs:10-12 (R): Why: this?\n  ```\n  let x = 1;\n  ```\n- src/a.rs:4 (L): Gone.";
        let (words, comments) = split_comments(text);
        assert_eq!(words, "Address the review comments below.");
        assert_eq!(
            comments,
            vec![("src/a.rs:10-12".to_owned(), "Why: this?\n```\nlet x = 1;\n```".to_owned()), ("src/a.rs:4".to_owned(), "Gone.".to_owned())]
        );
    }

    #[test]
    fn appends_change_only_what_shows() {
        let mut entries = Vec::new();
        let mut reset = json!({ "reset": [
            { "id": "a", "role": "assistant", "status": "streaming", "parts": [
                { "kind": "tool", "id": "k", "text": "out" },
                { "kind": "reasoning", "id": "r", "text": "" },
                { "kind": "text", "id": "t", "text": "Hi" },
            ] },
            { "id": "b", "role": "assistant", "status": "complete", "parts": [{ "kind": "text", "id": "t", "text": "Done" }] },
        ] });
        apply(&mut entries, &mut reset).unwrap();
        let frame = |entry: &str, part: &str, text: &str, len: usize| {
            json!({ "upsert": [], "remove": [], "append": [{ "entry": entry, "part": part, "text": text, "len": len }], "count": 2 })
        };
        // More of a tool's output: nothing shows it.
        assert!(matches!(apply(&mut entries, &mut frame("a", "k", "put", 6)), Ok(Change::Quiet)));
        assert_eq!(entries[0]["parts"][0]["text"], "output");
        // Thinking's first words make its row; more of them are the draft's.
        assert!(matches!(apply(&mut entries, &mut frame("a", "r", "Hmm", 3)), Ok(Change::Rows)));
        assert!(matches!(apply(&mut entries, &mut frame("a", "r", "…", 6)), Ok(Change::Words)));
        // Words of the reply being written: the draft; of a finished one,
        // its row.
        assert!(matches!(apply(&mut entries, &mut frame("a", "t", " there", 8)), Ok(Change::Words)));
        assert!(matches!(apply(&mut entries, &mut frame("b", "t", "!", 5)), Ok(Change::Rows)));
        // Out of step (a length that doesn't match): resubscribe.
        assert!(apply(&mut entries, &mut frame("a", "t", "x", 99)).is_err());
    }

    #[test]
    fn a_text_part_s_first_words_make_its_block() {
        let mut entries = Vec::new();
        let mut reset = json!({ "reset": [
            { "id": "a", "role": "assistant", "status": "streaming", "parts": [
                { "kind": "reasoning", "id": "r", "text": "Hmm" },
                { "kind": "text", "id": "t", "text": "" },
            ] },
        ] });
        apply(&mut entries, &mut reset).unwrap();
        let frame = |text: &str, len: usize| {
            json!({ "upsert": [], "remove": [], "append": [{ "entry": "a", "part": "t", "text": text, "len": len }], "count": 1 })
        };
        // Blank still: nothing to show.
        assert!(matches!(apply(&mut entries, &mut frame("\n", 1)), Ok(Change::Words)));
        // Its first words: its row (the block it had none of); then the draft's.
        assert!(matches!(apply(&mut entries, &mut frame("Hi", 3)), Ok(Change::Rows)));
        assert!(matches!(apply(&mut entries, &mut frame(" there", 9)), Ok(Change::Words)));
    }

    #[test]
    fn the_draft_carries_the_thought_a_reply_is_at() {
        let draft = || draft_value("a", "");
        let thought = json!({ "kind": "reasoning", "id": "r", "text": "  Weighing it.\n" });
        let text = json!({ "kind": "text", "id": "t", "text": "Hi" });
        // At a thought: its id, and its words as its row shows them.
        let parts = vec![text.clone(), thought.clone()];
        let at_it = with_thought(draft(), Some(&parts));
        assert_eq!(text_of(&at_it, "thought"), "r");
        assert_eq!(text_of(&at_it, "thought_text"), "Weighing it.");
        // Past it (the reply's text or a tool has begun): its row has its words.
        let parts = vec![thought, text];
        assert!(with_thought(draft(), Some(&parts)).field("thought").is_none());
        assert!(with_thought(draft(), None).field("thought").is_none());
    }

    #[test]
    fn a_frame_grows_a_reply_by_its_words_alone() {
        let mut entries = Vec::new();
        let mut reset = json!({ "reset": [
            { "id": "u", "role": "user", "parts": [{ "kind": "text", "id": "t0", "text": "hi" }] },
            { "id": "a", "role": "assistant", "status": "streaming", "parts": [{ "kind": "text", "id": "t0", "text": "Hel" }] },
        ] });
        assert!(matches!(apply(&mut entries, &mut reset), Ok(Change::Rows)));
        let mut words = json!({ "upsert": [], "remove": [], "append": [{ "entry": "a", "part": "t0", "text": "lo", "len": 5 }], "count": 2 });
        assert!(matches!(apply(&mut entries, &mut words), Ok(Change::Words)));
        assert_eq!(entry_text(entries[1].json.as_ref().unwrap()), "Hello");
        // Out of step: a length that doesn't add up.
        let mut wrong = json!({ "upsert": [], "remove": [], "append": [{ "entry": "a", "part": "t0", "text": "!", "len": 9 }], "count": 2 });
        assert!(apply(&mut entries, &mut wrong).is_err());
    }

    #[test]
    fn a_transcript_s_subagents_are_listed_once_each_in_order() {
        let tool = |r: &str, title: &str| {
            Value::record([("id", Value::from(r)), ("subagent", Value::record([("ref", Value::from(r)), ("title", Value::from(title))]))])
        };
        let message = |started: f64, tools: Vec<Value>| {
            Value::record([("started", Value::Num(started)), ("tools", Value::List(tools.into()))])
        };
        let plain = Value::record([("id", Value::from("t")), ("verb", Value::from("Ran"))]);
        let list = [
            message(1., vec![tool("a", "first"), plain]),
            Value::record([("role", Value::from("user"))]),
            message(2., vec![tool("a", "again"), tool("b", "second")]),
        ];
        let mut seen = Subagents::default();
        for message in &list {
            seen.add(subagents_in(message).as_deref());
        }
        let found = seen.0;
        let refs: Vec<String> = found.iter().map(|s| s.field("ref").map(text).unwrap_or_default()).collect();
        assert_eq!(refs, ["a", "b"]);
        assert_eq!(found[0].field("title").map(text).as_deref(), Some("first"));
        assert_eq!(found[1].field("at").and_then(Value::as_f64), Some(2.));
    }

    #[test]
    fn a_message_from_another_chat_says_who_over_its_words() {
        let id = "cd27c5c2-0000-4000-8000-000000000000";
        let said = |text: String| json!({ "id": "u", "role": "user", "parts": [{ "kind": "text", "id": "t", "text": text }] });
        let m = message(&said(zeron_proto::agent_messages::agent_message("Layout pass (cd27c5c2)", id, "All done.")), None).unwrap();
        let sender = m.field("sender").unwrap();
        let is = |v: Option<&Value>, want: &str| matches!(v, Some(Value::Str(s)) if s.as_ref() == want);
        assert!(is(sender.field("chat"), id));
        assert!(is(sender.field("title"), "Layout pass"));
        // Its words alone, to show and to copy: the header is the agent's.
        assert!(is(m.field("text"), "All done."));
        assert!(is(m.field("copy"), "All done."));
        // A person's that merely starts alike is theirs, as written.
        let typed = "[Message from Zeron chat x]\n\nhi";
        let m = message(&said(typed.to_owned()), None).unwrap();
        assert!(m.field("sender").is_none_or(|s| matches!(s, Value::Nil)));
        assert!(is(m.field("text"), typed));
    }

    /// Chats attached travel in a block after the words (carried.luau
    /// `with_chats`): the sent message shows each as a card, with its
    /// agent's mark when the chat is known, and the words without them. A
    /// chat linked in the words themselves (an @ mention) stays there.
    #[test]
    fn attached_chats_are_cards_and_mentioned_ones_stay_in_the_words() {
        let block = format!("{CHATS_HEADER}\n- [Fix the build](zeron-chat:c1)\n- [Write \\[docs\\]](zeron-chat:c2)");
        let (words, chats) = split_chats(&format!("look at these\n\n{block}"));
        assert_eq!(words, "look at these");
        assert_eq!(
            chats,
            vec![("Fix the build".to_owned(), "c1".to_owned()), ("Write [docs]".to_owned(), "c2".to_owned())]
        );
        // Only the block: no words of its own.
        let (words, chats) = split_chats(&block);
        assert_eq!((words.as_str(), chats.len()), ("", 2));
        // A chat mentioned in the words is left in them.
        let mention = "ask [Fix the build](zeron-chat:c1) about it";
        assert_eq!(split_chats(mention), (mention.to_owned(), Vec::new()));
        // A block line that isn't one chat link alone: the words, untouched.
        for odd in [
            format!("hi\n\n{CHATS_HEADER}\n- see [Fix](zeron-chat:c1)"),
            format!("hi\n\n{CHATS_HEADER}\n- [site](https://example.com)"),
        ] {
            assert_eq!(split_chats(&odd), (odd.clone(), Vec::new()), "{odd}");
        }
        // Its card: the title, the id, the agent's mark once the chat is known.
        let card = chat_chip("Fix the build", "c1");
        assert!(matches!(card.field("kind"), Some(Value::Str(s)) if s.as_ref() == "chat"));
        assert!(matches!(card.field("chat"), Some(Value::Str(s)) if s.as_ref() == "c1"));
        assert!(card.field("mark").is_none(), "an unknown chat has no mark");
        CHATS.with(|c| c.chats.borrow_mut().insert("c1".into(), json!({ "id": "c1", "config": { "harness": "codex" } })));
        let card = chat_chip("Fix the build", "c1");
        assert!(matches!(card.field("mark"), Some(Value::Str(s)) if s.as_ref() == mark("codex")));
        CHATS.with(|c| c.chats.borrow_mut().remove("c1"));
    }
    #[test]
    fn a_prompts_attached_passages_show_as_chips_on_the_sent_message() {
        let text = "Why this one?\n\nSelected from this conversation:\n- GPT-5.5, Sep 8, 1:43 PM: Layout dominates.\n  Paint is second.\n- You: the first ask\n\nAttached images (local files — open them to view):\n- /tmp/shot.png";
        let entry = json!({ "id": "u", "role": "user", "createdAt": 1_788_900_000_000_i64,
            "parts": [{ "id": "t", "kind": "text", "text": text }] });
        let shown = message(&entry, None).expect("a prompt shows");
        assert!(matches!(shown.field("text"), Some(Value::Str(s)) if s.as_ref() == "Why this one?"));
        let Some(Value::List(chips)) = shown.field("attachments") else { panic!("no attachments") };
        let names: Vec<String> = chips
            .iter()
            .map(|c| match c.field("name") {
                Some(Value::Str(s)) => s.to_string(),
                _ => String::new(),
            })
            .collect();
        assert_eq!(names, vec!["shot.png", "GPT-5.5, Sep 8, 1:43 PM", "You"]);
        assert!(matches!(chips[1].field("meta"), Some(Value::Str(s)) if s.as_ref() == "Layout dominates. Paint is second."));
        // A prompt only quoting (no words of its own), and one quoting the
        // header mid-text, untouched.
        let (words, quotes) = split_quotes("Selected from this conversation:\n- You: hi");
        assert_eq!((words.as_str(), quotes.len()), ("", 1));
        let plain = "See: Selected from this conversation: is a header";
        assert_eq!(split_quotes(plain), (plain.to_owned(), Vec::new()));
    }

    #[test]
    fn a_run_with_another_agent_model_or_effort_changes_the_chat_and_nothing_else_does() {
        let chat = json!({ "harness": "claude-code", "model": "sonnet", "reasoning": "medium", "modelOptions": {}, "sandbox": "workspace-write" });
        // The chat's own config, filled out as a run's is: the same.
        let same = merged_config(&chat, &Value::record([("text", Value::from("hi"))]));
        assert!(!runs_otherwise(&chat, &same));
        // A config with no options or nulls reads as one with empty ones.
        assert!(!runs_otherwise(&json!({ "harness": "claude-code", "model": "sonnet", "reasoning": "medium" }), &same));
        // Another agent's model, or another effort: the chat takes it.
        let other = merged_config(&chat, &Value::record([("model", Value::from("gpt")), ("harness", Value::from("codex"))]));
        assert!(runs_otherwise(&chat, &other));
        assert_eq!(other["harness"], json!("codex"));
        let effort = merged_config(&chat, &Value::record([("effort", Value::from("high"))]));
        assert!(runs_otherwise(&chat, &effort));
        // A model's options (Devin Fusion's lead and sidekick), picked
        // whole: what's picked is the chat's, what isn't is gone; a choice
        // at its default is left out of the picks.
        let chat = json!({ "harness": "devin", "model": "fusion", "reasoning": "high", "modelOptions": { "lead": "a", "sidekick": "x" } });
        let picked = merged_config(&chat, &Value::record([("options", Value::record([("lead", Value::from("b"))]))]));
        assert!(runs_otherwise(&chat, &picked));
        assert_eq!(picked["modelOptions"], json!({ "lead": "b" }));
        let none = merged_config(&chat, &Value::record([("options", Value::record::<&str>([]))]));
        assert_eq!(none["modelOptions"], json!({}));
    }

    #[test]
    fn a_models_options_reach_the_picker_but_fast_is_its_own() {
        let m = json!({
            "id": "fusion", "label": "Fusion", "reasoningLevels": ["low", "high"],
            "options": [
                { "id": "lead", "label": "Lead", "defaultChoice": "a", "choices": [{ "id": "a", "label": "Lead A" }, { "id": "b", "label": "Lead B" }] },
                { "id": "speed", "label": "Fast Mode", "defaultChoice": "standard", "choices": [{ "id": "standard", "label": "Standard" }, { "id": "fast", "label": "Fast" }] },
                { "id": "empty", "label": "Nothing", "defaultChoice": "", "choices": [] }
            ]
        });
        assert_eq!(fast_option(&m), Some(("speed".into(), "fast".into(), "standard".into())));
        let v = model_value("devin", &m);
        assert!(matches!(v.field("fast"), Some(Value::Bool(true))));
        let options = v.field("options").map(Value::list).unwrap_or_default();
        assert_eq!(options.len(), 1);
        assert_eq!(options[0].field("id").map(text).as_deref(), Some("lead"));
        assert_eq!(options[0].field("default").map(text).as_deref(), Some("a"));
        assert_eq!(options[0].field("choices").map(Value::list).map(<[Value]>::len), Some(2));
        // A chat's picks: Fast read as Fast, the rest as they are.
        let config = json!({ "speed": "fast", "lead": "b", "n": 3 });
        assert!(fast_in(&config));
        assert_eq!(options_value(&config), Value::record([("lead", Value::from("b")), ("speed", Value::from("fast"))]));
        // Claude's fastMode, off by default.
        let claude = json!({ "id": "opus", "options": [{ "id": "fastMode", "label": "Fast", "defaultChoice": "off", "choices": [{ "id": "off", "label": "Off" }, { "id": "on", "label": "On" }] }] });
        assert_eq!(fast_option(&claude), Some(("fastMode".into(), "on".into(), "off".into())));
        assert!(model_value("claude-code", &claude).field("options").is_none());
        // The agent's default rides along when marked, and only then.
        assert!(model_value("claude-code", &claude).field("default").is_none());
        let marked = json!({ "id": "claude-opus-5-5", "default": true });
        assert!(matches!(model_value("claude-code", &marked).field("default"), Some(Value::Bool(true))));
    }

    #[test]
    fn a_tools_line_is_one_line_and_its_whole_call_rides_with_it() {
        let str_of = |v: &Value, k: &str| match v.field(k) {
            Some(Value::Str(s)) => Some(s.to_string()),
            _ => None,
        };
        let part = |call: Json| json!({ "id": "p", "kind": "tool", "call": call });
        // A command of five lines: its line joins them; its call keeps them.
        let script = "echo '--- a'; git grep -n x\necho '--- b'; ls -d crates\necho '--- c'; git branch -a\necho '--- d'; gh pr list\necho '--- e'; gh issue list";
        let ran = tool(&part(json!({ "kind": "exec", "command": script }))).expect("a tool");
        let line = str_of(&ran, "target").expect("a line");
        assert!(!line.contains('\n'));
        assert!(line.starts_with("echo '--- a'; git grep -n x echo '--- b'"));
        assert_eq!(str_of(&ran, "call").as_deref(), Some(script));
        // A command of one short line: nothing more to open to.
        let short = tool(&part(json!({ "kind": "exec", "command": "cargo test" }))).expect("a tool");
        assert_eq!(str_of(&short, "target").as_deref(), Some("cargo test"));
        assert_eq!(str_of(&short, "call"), None);
        // A pattern with a newline and where it looked; an MCP call's input.
        let search = tool(&part(json!({ "kind": "search", "pattern": "fn a\\(\n", "path": "src" }))).expect("a tool");
        assert_eq!(str_of(&search, "target").as_deref(), Some("fn a\\("));
        assert_eq!(str_of(&search, "call").as_deref(), Some("fn a\\(\n in src"));
        let mcp = tool(&part(json!({ "kind": "mcp", "server": "gh", "tool": "issues", "input": { "q": "plugin" } }))).expect("a tool");
        assert_eq!(str_of(&mcp, "target").as_deref(), Some("gh.issues"));
        assert_eq!(str_of(&mcp, "call").as_deref(), Some("gh · issues\n{\n  \"q\": \"plugin\"\n}"));
        // An edit names its file; its strings never reach its line.
        let edit = tool(&part(json!({ "kind": "editFile", "path": "src/a.rs", "old_string": "x\ny", "new_string": "z" }))).expect("a tool");
        assert_eq!(str_of(&edit, "target").as_deref(), Some("a.rs"));
        assert_eq!(str_of(&edit, "call"), None);
        // A call past its cap says how much more there was.
        let long = (0..250).map(|i| format!("echo {i}")).collect::<Vec<_>>().join("\n");
        let big = tool(&part(json!({ "kind": "exec", "command": long }))).expect("a tool");
        let call = str_of(&big, "call").expect("a call");
        assert_eq!(call.lines().count(), CALL_MAX_LINES + 1);
        assert!(call.ends_with("… 50 more lines"));
    }

    #[test]
    fn a_compact_reply_folds_its_work_and_shows_the_text_it_ends_on() {
        let tool = |id: &str, command: &str| {
            json!({ "id": id, "kind": "tool", "isError": false, "resolved": true, "call": { "kind": "exec", "command": command } })
        };
        let text = |id: &str, words: &str| json!({ "id": id, "kind": "text", "text": words });
        let reply = |status: &str| {
            json!({ "id": "a", "role": "assistant", "status": status, "durationMs": 65_000,
                "parts": [text("p1", "Looking."), tool("k1", "ls"), text("p2", "Then:"), tool("k2", "cargo test"), text("p3", "Done."), text("p4", "All green.")] })
        };
        let kinds = |v: &Value| -> Vec<(String, String)> {
            let Value::List(list) = v else { panic!("a list") };
            list.iter()
                .map(|b| {
                    let field = |k: &str| match b.field(k) {
                        Some(Value::Str(s)) => s.to_string(),
                        _ => String::new(),
                    };
                    (field("id"), field("kind"))
                })
                .collect()
        };
        // Settled: one work group where the work began ("Worked for 1m 5s"),
        // then the run of text it ends on.
        let done = compact_blocks(&reply("complete")).expect("it did work");
        assert_eq!(
            kinds(&done),
            vec![("work".into(), "work".into()), ("p3".into(), "text".into()), ("p4".into(), "text".into())]
        );
        let Value::List(list) = &done else { unreachable!() };
        assert!(matches!(list[0].field("summary"), Some(Value::Str(s)) if s.as_ref() == "Worked for 1m 5s"));
        // Inside it, the ordinary blocks of its work: its words and groups.
        assert_eq!(
            kinds(list[0].field("blocks").unwrap()),
            vec![
                ("p1".into(), "text".into()),
                ("g:k1".into(), "tools".into()),
                ("p2".into(), "text".into()),
                ("g:k2".into(), "tools".into()),
            ]
        );
        // Still being written: no reply yet, everything rides the group, its
        // line the tools' summary.
        let live = compact_blocks(&reply("streaming")).expect("it did work");
        assert_eq!(kinds(&live), vec![("work".into(), "work".into())]);
        // No work, no group.
        let plain = json!({ "id": "b", "role": "assistant", "status": "complete", "parts": [text("t", "Hi.")] });
        assert!(compact_blocks(&plain).is_none());
    }

    #[test]
    fn a_settled_reply_with_nothing_to_show_has_no_row() {
        let reply = |status: &str| {
            json!({ "id": "a", "role": "assistant", "status": status, "createdAt": 1_788_900_000_000_i64,
                "parts": [{ "id": "r", "kind": "reasoning", "text": "" }] })
        };
        // Settled and empty: no row (no lane, no time over the next reply).
        assert!(message(&reply("complete"), None).is_none());
        // Still being written: its row holds the working line, and no time.
        let live = message(&reply("streaming"), None).expect("a reply being written has a row");
        assert!(live.field("at").is_none_or(|v| matches!(v, Value::Nil)));
    }

    #[test]
    fn an_upsert_lands_after_its_anchor() {
        let mut entries = vec![Entry::new(json!({ "id": "a" })), Entry::new(json!({ "id": "c" }))];
        let mut frame = json!({ "upsert": [{ "after": "a", "entry": { "id": "b" } }], "remove": [], "append": [], "count": 3 });
        assert!(matches!(apply(&mut entries, &mut frame), Ok(Change::Rows)));
        let ids: Vec<_> = entries.iter().map(|e| e["id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["a", "b", "c"]);
    }

    /// No connection yet is the chat loading: "can't load" is for an engine
    /// that can't be reached.
    #[test]
    fn a_chat_waiting_on_its_connection_is_loading() {
        assert!(!super::chat_offline(true, false));
        assert!(super::chat_offline(true, true));
        assert!(!super::chat_offline(false, true));
    }

    #[test]
    fn attachments_round_trip_through_the_prompt() {
        let prompt = with_attachments("", &["/tmp/a.png".into()], 1);
        let (words, paths) = split_attachments(&prompt);
        assert_eq!(words, "");
        assert_eq!(paths, ["/tmp/a.png"]);
    }

    /// A Cloud machine's own setup (the entry its engine writes under the
    /// message that woke it) reads as what happened to the machine: each step
    /// by its name, the machine's glyph or git's, and a clone opening to its
    /// command, never the step's bookkeeping.
    #[test]
    fn a_cloud_machines_setup_reads_as_the_machine() {
        let step = |id: &str, name: &str, step: &str, command: Option<&str>, error: bool| {
            let mut input = json!({ "cloudSetup": step });
            if let Some(c) = command {
                input["command"] = json!(c);
            }
            json!({ "kind": "tool", "id": id, "call": { "kind": "unknown", "name": name, "input": input }, "resolved": true, "isError": error })
        };
        let woke = json!({ "id": "m.cloud-setup", "role": "assistant", "status": "streaming", "parts": [
            step("m.cloud-machine", "Woke the machine", "wake", None, false),
            step("m.cloud-clone", "Cloned acme/app", "clone", Some("git clone acme/app"), false),
        ] });
        let Value::List(woke) = blocks(&woke) else { panic!("no blocks") };
        assert_eq!(text_of(&woke[0], "summary"), "Woke the Cloud machine");
        let Some(Value::List(tools)) = woke[0].field("tools") else { panic!("no tools") };
        assert_eq!(text_of(&tools[0], "kind"), "cloud");
        assert_eq!(text_of(&tools[0], "verb"), "Woke the machine");
        assert!(tools[0].field("target").is_none() && tools[0].field("call").is_none());
        assert_eq!(text_of(&tools[1], "kind"), "git");
        assert_eq!(text_of(&tools[1], "call"), "git clone acme/app");
        let started = json!({ "id": "n.cloud-setup", "role": "assistant", "status": "done", "parts": [
            step("n.cloud-machine", "Started a machine", "start", None, false),
            step("n.cloud-clone", "Couldn't clone acme/app", "clone", None, true),
        ] });
        let Value::List(started) = blocks(&started) else { panic!("no blocks") };
        assert_eq!(text_of(&started[0], "summary"), "Set up the Cloud machine · 1 failed");
        // An agent's own unknown tool is still one.
        let other = json!({ "id": "o", "role": "assistant", "status": "done", "parts": [
            { "kind": "tool", "id": "t", "call": { "kind": "unknown", "name": "Frobnicate", "input": { "x": 1 } }, "resolved": true, "isError": false },
        ] });
        let Value::List(other) = blocks(&other) else { panic!("no blocks") };
        assert_eq!(text_of(&other[0], "summary"), "Called 1 tool");
    }

    #[test]
    fn a_replys_tools_group_between_its_text() {
        let text = |id: &str| json!({ "kind": "text", "id": id, "text": "words" });
        let exec = |id: &str| json!({ "kind": "tool", "id": id, "call": { "kind": "exec", "command": "ls" }, "resolved": true, "isError": false });
        let read = |id: &str| json!({ "kind": "tool", "id": id, "call": { "kind": "readFile", "path": "a.rs" }, "resolved": true, "isError": false });
        let entry = json!({ "id": "a", "role": "assistant", "status": "streaming", "parts": [
            { "kind": "reasoning", "id": "r1", "text": "hmm" },
            text("t1"), exec("x1"), exec("x2"), read("x3"), text("t2"), exec("x4"), read("x5"),
        ] });
        let Value::List(blocks) = blocks(&entry) else { panic!("no blocks") };
        let shape: Vec<String> = blocks
            .iter()
            .map(|b| format!("{}:{}", text_of(b, "kind"), text_of(b, "id")))
            .collect();
        assert_eq!(shape, ["tools:g:r1", "text:t1", "tools:g:x1", "text:t2", "tools:g:x4"]);
        assert_eq!(text_of(&blocks[0], "summary"), "Thought process");
        assert_eq!(text_of(&blocks[2], "summary"), "Ran 2 commands · read 1 file");
        // Open only at the tail of a streaming reply.
        assert!(!blocks[2].field("live").is_some_and(Value::truthy));
        assert!(blocks[4].field("live").is_some_and(Value::truthy));
    }

    fn text_of(v: &Value, key: &str) -> String {
        v.field(key).map(text).unwrap_or_default()
    }

    #[test]
    fn base64_pads() {
        assert_eq!(base64(b"Ma"), "TWE=");
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"M"), "TQ==");
    }

    #[test]
    fn a_visual_part_is_a_block_of_its_own_read_from_its_host() {
        let entry = json!({ "id": "a1", "role": "assistant", "status": "complete", "parts": [
            { "kind": "text", "id": "t1", "text": "Here it is:" },
            { "kind": "visual", "id": "v:1", "path": "/u/v_1-visual.luau", "title": "Latency" },
        ]});
        let shown = blocks(&entry);
        let list = shown.list();
        assert_eq!(list.len(), 2);
        assert_eq!(list[1].field("kind"), Some(&Value::from("visual")));
        assert_eq!(list[1].field("id"), Some(&Value::from("v:1")));
        assert!(list[1].field("device").is_none_or(|d| *d == Value::Nil), "this device's: no device");
        // A chat on another device: its visuals are read from there.
        assert!(with_host(&entry, "c1", None).is_none());
        let remote = with_host(&entry, "c1", Some("dev-2")).expect("it has a visual");
        let shown = blocks(&remote);
        assert_eq!(shown.list()[1].field("device"), Some(&Value::from("dev-2")));
    }

    /// A command whose output the doc cut names the doc it is in (its row
    /// asks that doc's host for the rest), with its exit code and the whole
    /// output's lines; one kept whole names nothing.
    #[test]
    fn a_cut_command_output_names_its_doc_lines_and_exit_code() {
        let exec = |id: &str, cut: bool| {
            let mut part = json!({ "kind": "tool", "id": id, "resolved": true, "isError": true, "exitCode": 2,
                                   "output": "error: boom", "call": { "kind": "exec", "command": "make" } });
            if cut {
                part["outputTruncated"] = json!(true);
                part["outputLines"] = json!(300);
            }
            part
        };
        let entry = json!({ "id": "a1", "role": "assistant", "status": "complete",
                            "parts": [exec("t1", true), exec("t2", false)] });
        let stamped = with_host(&entry, "c1--sub--x", None).expect("a cut output");
        let row = tool(&stamped["parts"][0]).unwrap();
        assert_eq!(row.field("chat"), Some(&Value::from("c1--sub--x")));
        assert_eq!(row.field("output_truncated"), Some(&Value::Bool(true)));
        assert!(matches!(row.field("output_lines"), Some(Value::Num(n)) if *n == 300.0));
        assert!(matches!(row.field("exit_code"), Some(Value::Num(n)) if *n == 2.0));
        let whole = tool(&stamped["parts"][1]).unwrap();
        assert!(whole.field("chat").is_none() && whole.field("output_truncated").is_none());
        assert!(with_host(&json!({ "parts": [exec("t2", false)] }), "c1", None).is_none());
    }

    #[test]
    fn a_row_plainly_there_is_one_building_it_makes() {
        let tool = |id: &str| json!({ "kind": "tool", "id": id, "resolved": true, "call": { "kind": "exec", "command": "ls" } });
        let show = json!({ "kind": "tool", "id": "s", "resolved": true, "call": { "kind": "mcp", "server": "zeron", "tool": "show_visual", "input": {} } });
        let thought = |text: &str| json!({ "kind": "reasoning", "id": "r", "text": text });
        let reply = |parts: Vec<Json>| json!({ "id": "a", "role": "assistant", "status": "done", "parts": parts });
        let cases = [
            (json!({ "id": "u", "role": "user", "parts": [] }), true),
            (reply(vec![tool("t")]), true),
            (reply(vec![thought("words")]), true),
            (reply(vec![show.clone(), tool("t")]), true),
            // Not plain: nothing said, though building may find a row.
            (reply(vec![thought("  ")]), false),
            (reply(vec![show]), false),
            (reply(vec![]), false),
            (json!({ "role": "user", "parts": [] }), false),
            (json!({ "id": "x", "role": "switch", "parts": [] }), false),
        ];
        for (entry, plain) in cases {
            assert_eq!(super::plainly_has_row(&entry), plain, "{entry}");
            if plain {
                assert!(super::message(&entry, None).is_some(), "plainly a row, but none built: {entry}");
            }
        }
    }

    #[test]
    fn a_subagent_is_named_in_its_part_or_is_in_no_row() {
        let part = |extra: Json| {
            let mut part = json!({ "kind": "tool", "id": "t", "resolved": true, "call": { "kind": "unknown", "name": "Agent: look", "input": { "prompt": "Look" } } });
            part.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            part
        };
        let reply = |part: Json| json!({ "id": "a", "role": "assistant", "status": "done", "parts": [part] });
        let named = reply(part(json!({ "subagentRef": "doc--sub--1", "subagentStatus": "done" })));
        assert!(super::names_subagent(&named));
        assert!(super::subagents_in(&super::message(&named, None).unwrap()).is_some());
        for unnamed in [reply(part(json!({}))), reply(part(json!({ "subagentRef": "" })))] {
            assert!(!super::names_subagent(&unnamed));
            assert!(super::subagents_in(&super::message(&unnamed, None).unwrap()).is_none());
        }
    }

    #[test]
    fn display_tools_are_their_output_alone() {
        let mcp = |id: &str, tool: &str, resolved: bool, error: bool| {
            json!({ "kind": "tool", "id": id, "resolved": resolved, "isError": error, "output": "no such file: chart.png",
                    "call": { "kind": "mcp", "server": "zeron", "tool": tool, "input": {} } })
        };
        let exec = |id: &str| json!({ "kind": "tool", "id": id, "resolved": true, "isError": false, "call": { "kind": "exec", "command": "ls" } });
        let reply = |parts: Vec<Json>| json!({ "id": "a1", "role": "assistant", "status": "complete", "parts": parts });
        // A call that worked, or is working: nothing of its own, in no group.
        let entry = reply(vec![exec("x1"), mcp("s1", "show_visual", true, false), exec("x2")]);
        let shown = blocks(&entry);
        assert_eq!(shown.list().len(), 1, "one group round it");
        assert_eq!(shown.list()[0].field("summary"), Some(&Value::from("Ran 2 commands")));
        assert_eq!(shown.list()[0].field("tools").unwrap().list().len(), 2);
        let running = reply(vec![mcp("s1", "mcp__zeron__show_image", false, false)]);
        assert!(blocks(&running).list().is_empty());
        let message = message(&entry, None).unwrap();
        assert_eq!(message.field("tools").unwrap().list().len(), 2, "the message's tools leave it out");
        // Failed: a quiet line with why, where it was.
        let failed = reply(vec![exec("x1"), mcp("s1", "show_image", true, true), exec("x2")]);
        let list = blocks(&failed);
        let kinds: Vec<String> = list.list().iter().map(|b| crate::auth::text(b.field("kind").unwrap())).collect();
        assert_eq!(kinds, ["tools", "display_failed", "tools"]);
        assert_eq!(list.list()[1].field("text"), Some(&Value::from("no such file: chart.png")));
        // Compact: no work of its own, the line in the reply.
        let only = reply(vec![mcp("s1", "show_visual", true, true), json!({ "kind": "text", "id": "t1", "text": "Done." })]);
        assert!(compact_blocks(&only).is_none(), "no work at all");
        let mixed = reply(vec![exec("x1"), mcp("s1", "show_visual", true, true), json!({ "kind": "text", "id": "t1", "text": "Done." })]);
        let compact = compact_blocks(&mixed).unwrap();
        let kinds: Vec<String> = compact.list().iter().map(|b| crate::auth::text(b.field("kind").unwrap())).collect();
        assert_eq!(kinds, ["work", "display_failed", "text"]);
    }

    #[test]
    fn pictures_an_agent_shows_sit_in_the_reply_in_place() {
        let mcp = |id: &str, error: bool| {
            json!({ "kind": "tool", "id": id, "resolved": true, "isError": error, "output": "visual:2: attempt to index nil",
                    "call": { "kind": "mcp", "server": "zeron", "tool": "mcp__zeron__show_image", "input": {} } })
        };
        let image = |id: &str, name: &str| json!({ "kind": "image", "id": id, "path": format!("/u/{name}"), "name": name, "mimeType": "image/png" });
        let exec = |id: &str| json!({ "kind": "tool", "id": id, "resolved": true, "isError": false, "call": { "kind": "exec", "command": "ls" } });
        let text = |id: &str, t: &str| json!({ "kind": "text", "id": id, "text": t });
        let reply = |parts: Vec<Json>| json!({ "id": "a1", "role": "assistant", "status": "complete", "parts": parts });
        let kinds = |v: &Value| -> Vec<String> { v.list().iter().map(|b| crate::auth::text(b.field("kind").unwrap())).collect() };
        // In a run of tools: the picture breaks out where its call was, the
        // tools either side fold as ever, and it counts in neither.
        let entry = reply(vec![exec("x1"), mcp("s1", false), image("i1", "a.png"), exec("x2"), exec("x3")]);
        let shown = blocks(&entry);
        assert_eq!(kinds(&shown), ["tools", "images", "tools"]);
        assert_eq!(shown.list()[0].field("summary"), Some(&Value::from("Ran 1 command")));
        assert_eq!(shown.list()[2].field("summary"), Some(&Value::from("Ran 2 commands")));
        let pics = shown.list()[1].field("images").unwrap().list();
        assert_eq!(pics.len(), 1);
        assert_eq!(pics[0].field("path"), Some(&Value::from("/u/a.png")));
        assert_eq!(pics[0].field("name"), Some(&Value::from("a.png")));
        // Two shown one after another (each its own call): one block, keyed
        // by the first, so a second arriving joins it.
        let two = reply(vec![text("t1", "Two:"), mcp("s1", false), image("i1", "a.png"), mcp("s2", false), image("i2", "b.png"), text("t2", "Done.")]);
        let shown = blocks(&two);
        assert_eq!(kinds(&shown), ["text", "images", "text"]);
        assert_eq!(shown.list()[1].field("id"), Some(&Value::from("i1")));
        assert_eq!(shown.list()[1].field("images").unwrap().list().len(), 2);
        // A reply of a picture alone has a row.
        assert!(message(&reply(vec![mcp("s1", false), image("i1", "a.png")]), None).is_some());
        // Compact: the picture in the reply, not the work.
        let compact = compact_blocks(&entry).unwrap();
        assert_eq!(kinds(&compact), ["work", "images"]);
        // Failed: which tool, and why as said (ds/reply_picture keeps what's plain).
        let failed = blocks(&reply(vec![mcp("s1", true), text("t1", "Sorry.")]));
        assert_eq!(kinds(&failed), ["display_failed", "text"]);
        assert_eq!(failed.list()[0].field("tool"), Some(&Value::from("image")));
        // Failed, then shown on another try: the stumble isn't drawn.
        let retried = blocks(&reply(vec![mcp("s1", true), mcp("s2", false), image("i1", "a.png")]));
        assert_eq!(kinds(&retried), ["images"]);
    }

    #[test]
    fn waiting_messages_read_in_seconds_with_their_failure() {
        let items = vec![
            json!({ "id": "c1", "chatId": "s2", "messageId": "m1", "prompt": "Ship it", "issuedBy": "mac",
                "sentAt": 1_800_000_000_000_i64, "expiresAt": 1_800_604_800_000_i64 }),
            json!({ "id": "o1", "chatId": "s2", "prompt": "With files", "issuedBy": "mac", "sentAt": 1_800_000_000_000_i64,
                "held": true, "failure": "too many files are waiting for Cloud machines to collect them; retry once one has" }),
        ];
        let out: Json = serde_json::from_str(&super::waiting_value(&items).to_json()).unwrap();
        assert_eq!(out[0]["text"], "Ship it");
        assert_eq!(out[0]["message"], "m1");
        assert_eq!(out[0]["sent"], 1_800_000_000.0);
        assert_eq!(out[0]["expires"], 1_800_604_800.0);
        assert_eq!(out[0]["held"], false);
        assert!(out[0]["failure"].is_null());
        assert_eq!(out[1]["held"], true);
        assert!(out[1]["expires"].is_null());
        assert!(out[1]["failure"].as_str().unwrap().starts_with("too many files"));
    }

    #[test]
    fn an_outbox_send_reads_as_its_words_or_its_files() {
        let trailer = "\n\nAttached images (local files — open them to view):\n- /tmp/up/a.png\n- /tmp/up/b.pdf";
        let items = vec![
            json!({ "id": "o1", "prompt": format!("Look at these{trailer}"), "held": true }),
            json!({ "id": "o2", "prompt": format!("See the attached image(s).{trailer}"), "held": true }),
        ];
        let out: Json = serde_json::from_str(&super::waiting_value(&items).to_json()).unwrap();
        assert_eq!(out[0]["text"], "Look at these");
        assert_eq!(out[1]["text"], "a.png, b.pdf");
    }

    #[test]
    fn an_engine_without_removal_says_so() {
        assert_eq!(super::held_removal_failure("unknown method: RemoveHeldSend"), "unsupported");
        assert_eq!(super::held_removal_failure("offline"), "offline");
        assert_eq!(super::held_removal_failure("chat not found"), "failed");
    }

    #[test]
    fn a_cloud_chats_own_agent_stays_offered_without_an_account() {
        let codex = json!({ "id": "codex", "installed": true, "enabled": false });
        // On the chat's own machine: listed, so its model reads as itself.
        assert!(super::offered(&codex, "cloud-s-chat-1"));
        // Anywhere else (a computer, the logical Cloud device a new chat picks
        // from): only what can run there.
        assert!(!super::offered(&codex, "cloud"));
        assert!(!super::offered(&codex, "mac"));
        assert!(!super::offered(&json!({ "id": "mock" }), "cloud-s-chat-1"));
        assert!(!super::offered(&json!({ "id": "claude-code", "installed": false }), "cloud-s-chat-1"));
        assert!(super::offered(&json!({ "id": "claude-code" }), "mac"));
    }

    #[test]
    fn background_tasks_outlive_the_turn_and_go_with_a_stale_row() {
        use super::{Value, background_tasks, now_seconds};
        let fresh = (now_seconds() * 1000) as f64;
        let row = |status: &str, updated: f64| {
            serde_json::json!({
                "chatId": "c1", "status": status, "updatedAt": updated,
                "backgroundTasks": [
                    { "id": "b1", "command": "cargo test", "startedAt": fresh },
                    { "id": "b2", "command": "npm run dev", "description": "Dev server" },
                ],
            })
        };
        let ids = |tasks: &[Value]| -> Vec<String> {
            tasks.iter().map(|t| crate::auth::text(t.field("id").unwrap())).collect()
        };
        // Working, the turn ended (idle, completed) or waiting: the same rows,
        // each time, so the tray neither empties nor fills again.
        for status in ["working", "idle", "completed", "awaitingInput", "failed"] {
            assert_eq!(ids(&background_tasks(&row(status, fresh))), ["b1", "b2"], "{status}");
        }
        let tasks = background_tasks(&row("idle", fresh));
        assert_eq!(tasks[1].field("name"), Some(&Value::from("Dev server")));
        assert_eq!(tasks[1].field("detail"), Some(&Value::from("npm run dev")));
        assert_eq!(tasks[0].field("detail"), None);
        // A row gone quiet (its engine stopped beating): they went with it.
        let quiet = fresh - 120_000.;
        assert!(background_tasks(&row("idle", quiet)).is_empty());
        assert!(background_tasks(&serde_json::Value::Null).is_empty());
    }

    /// A view holding a chat's whole rows (another view's, just let go; or its
    /// own, the model's name moved) while the engine's whole copy is on its
    /// way: the turn being written goes on showing, its thought's words and a
    /// call arriving, rather than standing still until the whole copy comes
    /// and all of it showing at once. The opening tail is taken into the
    /// rows it holds, the changes after it counted against the tail.
    /// A real long chat's whole copy, taken in on the main thread: how long
    /// each frame holds it. Set ZERON_RESET_FRAME to a frame file (a
    /// `TranscriptUpdate` reset) to run it.
    #[test]
    fn a_long_chats_whole_copy_on_the_main_thread() {
        use super::{Entry, Open, View, draft_value};
        use std::cell::{Cell, RefCell};
        use std::rc::Rc;
        use zgx_script::{ScriptHost, Value, testing::TestApp};
        let Ok(path) = std::env::var("ZERON_RESET_FRAME") else { return };
        let bytes = std::fs::read(&path).unwrap();
        let start = std::time::Instant::now();
        let whole: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        eprintln!("parsing the frame ({} MB, the network thread's work): {:?}", bytes.len() / 1_000_000, start.elapsed());
        let n = whole["reset"].as_array().unwrap().len();
        let app = TestApp::new(|runtime| ScriptHost::inline(runtime, "app", "return ui.text { \"x\" }"));
        let view = |open: Open| {
            Rc::new(View {
                session: app.host.shared("t.session", Value::Nil),
                messages: app.host.shared("t.messages", Value::List(Rc::from([]))),
                subagents: app.host.shared("t.subagents", Value::List(Rc::from([]))),
                draft: app.host.shared("t.draft", draft_value("", "")),
                rpc: RefCell::default(),
                open: RefCell::new(Some(open)),
                pending: RefCell::default(),
                generation: Cell::new(0),
                snapshot: Cell::new(false),
            })
        };
        let time = |label: &str, view: &Rc<View>, frame: serde_json::Value| {
            let start = std::time::Instant::now();
            view.heard_frame(frame);
            eprintln!("{label}: {:?}", start.elapsed());
        };
        // Cold: the tail, then the whole copy.
        let cold = view(Open { id: "c".into(), ..Open::default() });
        let tail: Vec<_> = whole["reset"].as_array().unwrap()[n - 49..].to_vec();
        time("cold open, tail frame (49)", &cold, serde_json::json!({ "reset": tail, "historyPending": true }));
        time(&format!("cold open, whole copy ({n})"), &cold, whole.clone());
        // Held rows (a reopen on an engine without `known`): the whole copy again.
        let held: Vec<Entry> = whole["reset"].as_array().unwrap().iter().cloned().map(Entry::new).collect();
        let warm = view(Open { id: "c".into(), entries: held, heard: true, has_whole: true, ..Open::default() });
        time(&format!("held rows, whole copy again ({n})"), &warm, whole.clone());
        let start = std::time::Instant::now();
        drop(whole);
        eprintln!("dropping the frame's JSON: {:?}", start.elapsed());
    }

    /// The app's entry sums are the engine's (`zeron_doc::entry_sum`): a
    /// reopen says what it holds by them. Pinned in both crates.
    #[test]
    fn entry_sums_match_the_engines() {
        let a: serde_json::Value = serde_json::from_str(r#"{"id":"x","n":1.5,"p":[{"k":"t","v":null}]}"#).unwrap();
        let b: serde_json::Value = serde_json::from_str(r#"{"p":[{"v":null,"k":"t"}],"n":1.5,"id":"x"}"#).unwrap();
        assert_eq!(super::sum_of(&a), 0xec67_0b99_244f_a276);
        assert_eq!(super::sum_of(&b), super::sum_of(&a));
        // And the order digest (`zeron_doc::order_sum`).
        assert_eq!(super::order_sum(["a", "b"]), 0xab40_d782_0d40_8076);
    }

    /// After the engine's live frames (text appends, part patches, an
    /// upsert), the app's sum of each entry is the engine's `entry_sum` of
    /// it: else every reopen would say it holds something else and get a
    /// reset. The frames and sums come from the engine's own diff
    /// (crates/doc/examples/app_sum_fixture.rs).
    #[test]
    fn sums_after_live_frames_match_the_engines() {
        use super::{Open, View, draft_value};
        use std::cell::{Cell, RefCell};
        use std::rc::Rc;
        use zgx_script::{ScriptHost, Value, testing::TestApp};
        let fixture: serde_json::Value = serde_json::from_str(include_str!("live_frames_fixture.json")).unwrap();
        let app = TestApp::new(|runtime| ScriptHost::inline(runtime, "app", "return ui.text { \"x\" }"));
        let view = Rc::new(View {
            session: app.host.shared("t.session", Value::Nil),
            messages: app.host.shared("t.messages", Value::List(Rc::from([]))),
            subagents: app.host.shared("t.subagents", Value::List(Rc::from([]))),
            draft: app.host.shared("t.draft", draft_value("", "")),
            rpc: RefCell::default(),
            open: RefCell::new(Some(Open { id: "c".into(), ..Open::default() })),
            pending: RefCell::default(),
            generation: Cell::new(0),
            snapshot: Cell::new(false),
        });
        for (n, step) in fixture["steps"].as_array().unwrap().iter().enumerate() {
            view.heard_frame(step["frame"].clone());
            let open = view.open.borrow();
            let entries = &open.as_ref().unwrap().entries;
            let engine: Vec<(String, u64)> = step["sums"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| (p[0].as_str().unwrap().to_owned(), p[1].as_str().unwrap().parse().unwrap()))
                .collect();
            let here: Vec<(String, u64)> = entries.iter().map(|e| (e.id.to_string(), e.sum())).collect();
            assert_eq!(here, engine, "after frame {n}: {}", step["frame"]);
        }
        assert_eq!(view.generation.get(), 0, "no frame asked for the whole copy again");
    }

    /// Rows held (opened before, or a new connection): the engine's `since`
    /// frame is what changed from them, and once applied they're the whole
    /// copy; nothing is fetched whole.
    #[test]
    fn rows_held_take_what_changed_since() {
        use super::{Entry, Open, View, draft_value};
        use serde_json::json;
        use std::cell::{Cell, RefCell};
        use std::rc::Rc;
        use zgx_script::{ScriptHost, Value, testing::TestApp};
        let app = TestApp::new(|runtime| ScriptHost::inline(runtime, "app", "return ui.text { \"x\" }"));
        let text = |id: &str, words: &str| json!({ "id": format!("{id}p"), "kind": "text", "text": words });
        let reply = |id: &str, words: &str| json!({ "id": id, "role": "assistant", "status": "complete", "parts": [text(id, words)], "createdAt": 1_762_000_000_000_i64 });
        let held: Vec<Entry> = (0..50).map(|i| Entry::new(reply(&format!("a{i}"), "done"))).collect();
        let view = Rc::new(View {
            session: app.host.shared("t.session", Value::Nil),
            messages: app.host.shared("t.messages", Value::List(Rc::from([]))),
            subagents: app.host.shared("t.subagents", Value::List(Rc::from([]))),
            draft: app.host.shared("t.draft", draft_value("", "")),
            rpc: RefCell::default(),
            open: RefCell::new(Some(Open { id: "c".into(), entries: held, heard: true, has_whole: true, ..Open::default() })),
            pending: RefCell::default(),
            generation: Cell::new(0),
            snapshot: Cell::new(false),
        });
        let rows = || format!("{:?}", view.messages.signal().with_untracked(Clone::clone));
        // What it holds, as it would tell the engine: ids and the engine's sums.
        let known: Vec<(String, u64)> = view.open.borrow().as_ref().unwrap().entries.iter().map(|e| (e.id.to_string(), e.sum())).collect();
        assert_eq!(known.len(), 50);
        assert_eq!(known[7].1, super::sum_of(&reply("a7", "done")));
        // Told what it holds; not when its rows are to be built again.
        let params = |rebuild: bool| {
            let mut slot = view.open.borrow_mut();
            let open = slot.as_mut().unwrap();
            open.rebuild = rebuild;
            let params = super::with_known(json!({ "chatId": "c" }), open);
            open.rebuild = false;
            params
        };
        assert_eq!(params(false)["known"].as_array().map(Vec::len), Some(50));
        assert!(params(true).get("known").is_none(), "a rebuild needs the whole copy");
        // The engine's answer: one entry new since.
        view.heard_frame(json!({ "since": true, "upsert": [{ "after": "a49", "entry": reply("a50", "new since") }], "count": 51 }));
        {
            let open = view.open.borrow();
            let open = open.as_ref().unwrap();
            assert!(!open.has_whole, "the held rows and what changed are the whole copy");
            assert_eq!(open.entries.len(), 51);
        }
        assert!(rows().contains("new since"), "{}", rows());
        // A live change after it applies as ever.
        view.heard_frame(json!({ "upsert": [{ "after": "a50", "entry": reply("a51", "live") }], "count": 52 }));
        assert_eq!(view.open.borrow().as_ref().unwrap().entries.len(), 52);

        // The engine's order, said: held so, the frame is taken.
        let ids = |view: &View| view.open.borrow().as_ref().unwrap().entries.iter().map(|e| e.id.to_string()).collect::<Vec<_>>();
        let reopen = |view: &View| view.open.borrow_mut().as_mut().unwrap().has_whole = true;
        reopen(&view);
        let mut order: Vec<String> = ids(&view);
        order.push("a52".into());
        let sum = super::order_sum(order.iter().map(String::as_str));
        let generation = view.generation.get();
        view.heard_frame(json!({ "since": true, "upsert": [{ "after": "a51", "entry": reply("a52", "more") }], "count": 53, "orderSum": sum }));
        assert_eq!(view.generation.get(), generation, "held in the engine's order: taken");
        assert_eq!(ids(&view), order);
        // A row without an id (a malformed one the app kept) is left out of
        // the order, as it's left out of `known`: no reload for it.
        reopen(&view);
        view.open.borrow_mut().as_mut().unwrap().entries.insert(1, super::Entry::new(json!({ "role": "user", "parts": [] })));
        let sum = super::order_sum(order.iter().map(String::as_str));
        view.heard_frame(json!({ "since": true, "count": 54, "orderSum": sum }));
        assert_eq!(view.generation.get(), generation, "a row without an id doesn't make it reload");
        view.open.borrow_mut().as_mut().unwrap().entries.remove(1);
        // Held in another order than the engine's: asked for whole again.
        reopen(&view);
        let mut other = ids(&view);
        other.swap(0, 1);
        let sum = super::order_sum(other.iter().map(String::as_str));
        view.heard_frame(json!({ "since": true, "count": 53, "orderSum": sum }));
        assert_eq!(view.generation.get(), generation + 1, "out of the engine's order: watched again, for the whole copy");
        assert!(!view.open.borrow().as_ref().unwrap().has_whole, "and without saying what it holds");
    }

    #[test]
    fn a_turn_being_written_goes_on_while_the_whole_copy_is_awaited() {
        use super::{Entry, Open, View, draft_value};
        use serde_json::json;
        use std::cell::{Cell, RefCell};
        use std::rc::Rc;
        use zgx_script::{ScriptHost, Value, testing::TestApp};
        let app = TestApp::new(|runtime| ScriptHost::inline(runtime, "app", "return ui.text { \"x\" }"));
        let text = |id: &str, words: &str| json!({ "id": format!("{id}p"), "kind": "text", "text": words });
        let prompt = |id: &str| json!({ "id": id, "role": "user", "parts": [text(id, "go")], "createdAt": 1_762_000_000_000_i64 });
        let reply = |id: &str| json!({ "id": id, "role": "assistant", "status": "complete", "parts": [text(id, "done")], "createdAt": 1_762_000_000_000_i64 });
        let thought = "I'm checking the swap.\n\nExec keeps the process start";
        let writing = |words: &str| {
            json!({ "id": "a1", "role": "assistant", "status": "streaming", "createdAt": 1_762_000_000_000_i64, "parts": [
                { "id": "r1", "kind": "reasoning", "text": words },
            ] })
        };
        // The rows it holds: a long chat's, its turn as it was let go.
        let held: Vec<Entry> = [prompt("u0"), reply("a0"), prompt("u1"), writing(thought)].into_iter().map(Entry::new).collect();
        let view = Rc::new(View {
            session: app.host.shared("t.session", Value::Nil),
            messages: app.host.shared("t.messages", Value::List(Rc::from([]))),
            subagents: app.host.shared("t.subagents", Value::List(Rc::from([]))),
            draft: app.host.shared("t.draft", draft_value("", "")),
            rpc: RefCell::default(),
            open: RefCell::new(Some(Open { id: "c".into(), entries: held, heard: true, has_whole: true, ..Open::default() })),
            pending: RefCell::default(),
            generation: Cell::new(0),
            snapshot: Cell::new(false),
        });
        let draft = || format!("{:?}", view.draft.signal().with_untracked(Clone::clone));
        let rows = || format!("{:?}", view.messages.signal().with_untracked(Clone::clone));
        // The engine's opening tail: its newest entries as they are now.
        let now = format!("{thought} time");
        view.heard_frame(json!({ "reset": [prompt("u1"), writing(&now)], "historyPending": true, "count": 2 }));
        // More of the thought, counted against the tail.
        let more = ", so the etime proves nothing.";
        let len = now.len() + more.len();
        view.heard_frame(json!({ "append": [{ "entry": "a1", "part": "r1", "text": more, "len": len }], "count": 2 }));
        assert!(draft().contains("proves nothing"), "the thought's new words reach the draft: {}", draft());
        // A call arriving: its row shows.
        let call = json!({ "id": "c1", "kind": "tool", "call": { "kind": "exec", "command": "tail engine.log" }, "status": "running" });
        view.heard_frame(json!({ "parts": [{ "entry": "a1", "at": 1, "part": call, "len": 2 }], "count": 2 }));
        assert!(rows().contains("tail engine.log"), "the call shows: {}", rows());
        // Its older rows are still the ones held (not down to the tail).
        assert_eq!(view.open.borrow().as_ref().map(|o| o.entries.len()), Some(4));
        // The whole copy: what shows is the engine's.
        view.heard_frame(json!({ "reset": [prompt("u0"), reply("a0"), prompt("u1"), reply("a1")] }));
        assert!(!view.open.borrow().as_ref().unwrap().has_whole);
        assert!(!rows().contains("tail engine.log"), "the whole copy replaces it: {}", rows());
    }

    /// A chat opened shows its rows once its transcript has come (this
    /// device's copy, read first whatever its host's state), not once the
    /// chats list has it too; "missing" only when the list is known, hasn't
    /// it, and nothing of it came; loading only while nothing has.
    #[test]
    fn a_chat_with_its_transcript_here_is_ready_before_the_chats_list_knows_it() {
        use super::{Chats, Entry, Open, session_value, text};
        use serde_json::json;
        let chats = Chats::default();
        let state = |chats: &Chats, open: &Open| session_value(chats, open, false).field("state").map(text).unwrap_or_default();
        let rows = vec![Entry::new(json!({ "id": "u0", "role": "user", "parts": [] }))];
        let mut open = Open { id: "c".into(), ..Open::default() };
        assert_eq!(state(&chats, &open), "loading", "nothing come yet");
        open.heard = true;
        open.entries = rows;
        // The chats list not come yet (a launch, the list large): its rows show.
        assert_eq!(state(&chats, &open), "ready");
        // The list come, without it (its host gone quiet, another device's):
        // its rows still show.
        chats.chats_heard.set(true);
        assert_eq!(state(&chats, &open), "ready");
        // Nothing of it here, and the list hasn't it: missing.
        open.entries.clear();
        assert_eq!(state(&chats, &open), "missing");
        // Listed: ready.
        chats.chats.borrow_mut().insert("c".into(), json!({ "id": "c", "config": {} }));
        assert_eq!(state(&chats, &open), "ready");
    }

    /// The chats views let go of lately are kept, the most recent few, so
    /// one opened again shows its rows at once (not a skeleton while it's
    /// read and built again), then takes what changed from the engine.
    #[test]
    fn chats_let_go_of_are_kept_to_open_again_with_their_rows() {
        use super::{Chats, Entry, LET_GO_MOST, Open};
        use serde_json::json;
        use std::rc::Rc;
        let chats = Rc::new(Chats::default());
        let open = |id: &str| Open {
            id: id.into(),
            entries: vec![Entry::new(json!({ "id": format!("{id}-m"), "role": "user", "parts": [] }))],
            heard: true,
            ..Open::default()
        };
        for id in ["a", "b", "c"] {
            chats.keep_let_go(Some(open(id)));
        }
        let b = chats.take_let_go("b").expect("b kept, though two were let go after it");
        assert!(b.heard && b.has_whole && b.entries.len() == 1);
        assert!(chats.take_let_go("b").is_none(), "taken once");
        assert!(chats.take_let_go("a").is_some());
        // The most recent few only.
        for i in 0..LET_GO_MOST + 2 {
            chats.keep_let_go(Some(open(&format!("x{i}"))));
        }
        assert!(chats.take_let_go("x0").is_none());
        assert!(chats.take_let_go(&format!("x{}", LET_GO_MOST + 1)).is_some());
        // And no more than LET_GO_BYTES of them: a chat alone over it isn't
        // kept, and those kept before it stay.
        chats.keep_let_go(Some(open("small")));
        let mut huge = open("huge");
        huge.built.insert("huge-m".into(), super::Built { folded: super::LET_GO_BYTES + 1, ..Default::default() });
        chats.keep_let_go(Some(huge));
        assert!(chats.take_let_go("huge").is_none(), "too big to keep");
        assert!(chats.take_let_go("small").is_some());
    }

    /// What keeping chats let go of costs (`let_go`): six chats the size of a
    /// long one (some 7,900 entries, about 28 MB as JSON whole), each built as
    /// a view builds it (its settled rows folded), kept one after another; the
    /// process's footprint (what Activity Monitor shows) after each.
    ///   cargo test --release --bin zeron-zgx kept_chats_cost -- --ignored --nocapture
    #[test]
    #[ignore = "a measurement"]
    #[cfg(target_os = "macos")]
    fn kept_chats_cost() {
        use super::{Chats, Entry, Open, RowContext, WHOLE_TAIL, built_for, settle};
        use serde_json::json;
        use std::rc::Rc;
        fn footprint_mb() -> f64 {
            let mut info: libc::rusage_info_v4 = unsafe { std::mem::zeroed() };
            let r = unsafe { libc::proc_pid_rusage(std::process::id() as i32, libc::RUSAGE_INFO_V4, &mut info as *mut _ as *mut _) };
            assert_eq!(r, 0);
            info.ri_phys_footprint as f64 / 1e6
        }
        let words = |i: usize, n: usize| -> String {
            (0..n).map(|k| format!("word{} ", (i * 31 + k * 7) % 997)).collect()
        };
        let chat = |c: usize| -> Vec<Json> {
            (0..7900)
                .map(|i| {
                    let id = format!("c{c}-e{i}");
                    if i % 2 == 0 {
                        json!({ "id": id, "role": "user", "createdAt": 1_762_000_000_000_i64 + i as i64, "parts": [{ "id": "t0", "kind": "text", "text": words(i, 40) }] })
                    } else {
                        json!({ "id": id, "role": "assistant", "status": "complete", "createdAt": 1_762_000_000_000_i64 + i as i64, "durationMs": 9000, "parts": [
                            { "id": "r0", "kind": "reasoning", "text": words(i, 120) },
                            { "id": "x0", "kind": "tool", "status": "completed", "call": { "kind": "exec", "command": format!("cargo test {i}") }, "output": words(i + 1, 200) },
                            { "id": "t0", "kind": "text", "text": format!("## Part {i}\n\n{}\n\n- a\n- b\n\n```rust\nfn f() {{}}\n```\n", words(i + 2, 160)) },
                        ] })
                    }
                })
                .collect()
        };
        let chats = Rc::new(Chats::default());
        let context = Rc::new(RowContext { doc: "doc".into(), device: None, model: Some("Opus".into()) });
        let base = footprint_mb();
        println!("KEPT base {base:.1} MB");
        // One built as a view builds it and let go of with nothing kept: what a
        // long chat opened at all leaves, cache or not.
        let build = |c: usize| {
            let list = chat(c);
            let whole_from = list.len().saturating_sub(WHOLE_TAIL);
            let mut entries: Vec<Entry> = Vec::with_capacity(list.len());
            let mut built = std::collections::HashMap::new();
            let mut older_todo = None;
            for (at, e) in list.into_iter().enumerate() {
                let row = built_for(&e, &context, at < whole_from);
                let mut entry = Entry::new(e);
                built.insert(entry.id.clone(), row);
                if at < whole_from {
                    settle(&mut entry, at, &mut older_todo);
                }
                entries.push(entry);
            }
            Open { id: format!("c{c}"), entries, built, older_todo, heard: true, ..Open::default() }
        };
        drop(build(99));
        println!("KEPT one opened and let go, none kept: {:.1} MB (+{:.1})", footprint_mb(), footprint_mb() - base);
        zgx_desktop::release_thread_heap();
        println!("KEPT and the heap released: {:.1} MB (+{:.1})", footprint_mb(), footprint_mb() - base);
        for c in 0..6 {
            let list = chat(c);
            let json_mb = list.iter().map(|e| serde_json::to_vec(e).map_or(0, |b| b.len())).sum::<usize>() as f64 / 1e6;
            let whole_from = list.len().saturating_sub(WHOLE_TAIL);
            let mut entries: Vec<Entry> = Vec::with_capacity(list.len());
            let mut built = std::collections::HashMap::new();
            let mut older_todo = None;
            for (at, e) in list.into_iter().enumerate() {
                let row = built_for(&e, &context, at < whole_from);
                let mut entry = Entry::new(e);
                built.insert(entry.id.clone(), row);
                if at < whole_from {
                    settle(&mut entry, at, &mut older_todo);
                }
                entries.push(entry);
            }
            chats.keep_let_go(Some(Open { id: format!("c{c}"), entries, built, older_todo, heard: true, ..Open::default() }));
            let weights: Vec<String> = chats.let_go.borrow().iter().map(|h| format!("{}={:.1}MB", h.id, h.bytes as f64 / 1e6)).collect();
            println!("KEPT {} chats: {:.1} MB (+{:.1}); this one {json_mb:.1} MB as JSON; kept {}", c + 1, footprint_mb(), footprint_mb() - base, weights.join(" "));
        }
        chats.let_go.borrow_mut().clear();
        println!("KEPT none again: {:.1} MB", footprint_mb());
    }

    /// A chat let go while its rows wait to be built again (its model's
    /// name moved) isn't kept, and an older kept copy of it goes: opened
    /// again it reads its rows whole, never says it holds old-model rows.
    #[test]
    fn a_chat_let_go_mid_rebuild_is_not_kept() {
        use super::{Chats, Entry, Open};
        use serde_json::json;
        use std::rc::Rc;
        let chats = Rc::new(Chats::default());
        let open = |rebuild: bool| Open {
            id: "m".into(),
            entries: vec![Entry::new(json!({ "id": "m-1", "role": "user", "parts": [] }))],
            heard: true,
            rebuild,
            ..Open::default()
        };
        chats.keep_let_go(Some(open(false)));
        chats.keep_let_go(Some(open(true)));
        assert!(chats.take_let_go("m").is_none(), "neither copy is kept");
    }
}
