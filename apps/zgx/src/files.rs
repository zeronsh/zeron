//! A session's files, for the scripts: its project's folders listed one at
//! a time, its files read and saved, renamed, moved and deleted, and told
//! when they change on disk. They are the engine's (crates/engine/src/
//! workspace_files.rs), on whichever device hosts the session, through the
//! same calls as the shipping app's files pane (crates/ui/src/files/
//! client.rs): one on another device carries its id (`targetDeviceId`), and
//! the engine relays it there.
//!
//! What scripts send as `target`:
//!   { chat = <chat id>?, space = <project id>?, device = <device id>?,
//!     root = <the folder on that device>? }
//! (a chat's, or else a project's; `device` is where it is, this Mac's when
//! missing; `root` is needed only to make files).
//!
//! The calls (`ui.call`, each a promise):
//!   files.list { target, dir }     a folder's entries: { checkout, can_move,
//!                                  can_delete, here, truncated, entries = { { name,
//!                                  path, dir, size?, ignored, readonly, rev?,
//!                                  kind } } }
//!   files.read { target, path }    { checkout, path, text?, hash?, size,
//!                                  encoding, line_ending?, readonly?, truncated,
//!                                  here (on this Mac: an image shows from its
//!                                  path) }
//!   files.image { target, path, checkout }
//!                                  an image on another device, its bytes
//!                                  read from it: { src (a file of its bytes
//!                                  here, named by their hash), size }
//!   files.artwork { target }       the project's own picture (its
//!                                  repository's logo, where the shipping
//!                                  app looks): { src } (a file here), or
//!                                  nil; a target on this Mac needs `root`
//!   files.chat_image { target, path }
//!                                  a picture a reply names (`![…](path)`),
//!                                  read through the engine from strictly
//!                                  inside the chat's folder on its device
//!                                  (`ReadChatImageChunk`, as the shipping
//!                                  app's reply images): { src (a file of its
//!                                  bytes here, named by their hash), size }
//!   files.attachment { target, path }
//!                                  a picture sent with a message in a chat on
//!                                  another device, read from there
//!                                  (`ReadAttachmentChunk`): { src, size }
//!   files.search_content { target, query, limit?, per_file? }
//!                                  lines holding `query`, best first: {
//!                                  matches = { { path, line, column, text,
//!                                  ranges } }, truncated, indexing } (line
//!                                  and column from 1; `ranges` the matches'
//!                                  `{ from, to }` bytes in `text`). A newer
//!                                  search's start drops an older one's answer.
//!   files.write { target, path, text, hash, checkout, encoding?, line_ending? }
//!                                  { status = "written", hash, size }, or
//!                                  { status = "conflict", reason, hash? }
//!   files.move { target, from, to, rev, kind, checkout }
//!   files.delete { target, path, rev, kind, checkout, recursive? }
//!                                  { status = "applied" } or
//!                                  { status = "rejected", reason, message }
//!   files.search { target, query } paths whose names match, best first:
//!                                  { { path, dir } }
//!   files.create { target, path, dir, checkout }
//!                                  made empty (the engine's
//!                                  CreateWorkspaceEntry, on any device; an
//!                                  older engine, on this Mac only):
//!                                  { status = "applied" } or rejected
//!   files.reveal { target, path }  shown in Finder, and
//!   files.open_external { target, path }
//!                                  opened in its app: on this Mac only
//! and actions:
//!   files.watch { key, target }    follow its changes (again after every
//!                                  reconnection), until
//!   files.unwatch key
//! Changes are told in the shared state `files.events`, `{ [key] = next }`:
//! each watched folder's next frame number. A script reads its own folder's
//! frames past the last it saw with `ui.call_sync("files.frames", { key,
//! since })`: `{ { n, key, sequence, resync, changes = { { kind, path,
//! old_path? } } } }`, the latest kept, `n` counting up per folder. It starts
//! over (`resync`) when it missed some. A reconnection adds a `resync`
//! frame for every watch. (A folder's frames stay with the host: a change
//! copies nothing into the script but that one number.)

use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, VecDeque},
    path::{Component, Path, PathBuf},
    rc::Rc,
    time::Duration,
};

use serde_json::{Map, Value as Json, json};
use zgx_desktop::WindowContext;
use zgx_script::{ScriptHost, Shared, Value};

use crate::engine::{self, rpc};

/// Frames kept for scripts to catch up on.
const KEPT: usize = 256;
/// A folder's pages followed at most (the engine pages by a few hundred):
/// past it, the listing says `truncated`.
const MAX_PAGES: usize = 64;
/// How long a watch waits for a connection before trying again.
const WATCH_WAIT: Duration = Duration::from_secs(20);

struct Slot {
    target: Value,
    /// Moves on every (re)start: an older start stops.
    generation: u64,
    watch: Option<rpc::Watch>,
    /// Its git status, followed beside its changes (`WatchWorkspaceGitStatus`,
    /// the shipping app's crates/ui/src/files/git_status.rs).
    git_watch: Option<rpc::Watch>,
    /// Let go of (`files.unwatch`) and ending after [`GRACE`] unless asked
    /// for again: the token of that ending, 0 while it's wanted.
    release: u64,
    /// How many ask for it now (two panes of one folder): it's let go of
    /// only as the last does.
    users: u32,
}

impl Slot {
    /// One more asks for it: whether it's running already (kept). Still
    /// starting, that start goes on, a second never beside it.
    fn join(&mut self, target: Value) -> bool {
        // (Users counted again from what's still asking: one let go of
        // within the grace counts none.)
        self.users = if self.release != 0 { 1 } else { self.users + 1 };
        self.release = 0;
        self.target = target;
        self.watch.is_some()
    }

    /// One fewer asks for it: whether that was the last (its grace starts).
    fn leave(&mut self) -> bool {
        self.users = self.users.saturating_sub(1);
        self.users == 0 && self.release == 0
    }
}

/// How long a watch let go of lives on: a screen made again (the scripts
/// reloaded, a template swapped in) asks for it again within this and
/// finds it kept, with nothing missed.
const GRACE: Duration = Duration::from_secs(5);

struct Files {
    events: RefCell<Vec<Shared>>,
    /// `files.git`: each watched folder's git status, `{ [key] = { [path] =
    /// status } }` (status as ds/file_tree says them; absent, clean or not
    /// known).
    git: RefCell<Vec<Shared>>,
    statuses: RefCell<HashMap<String, Value>>,
    /// Each folder's latest frames, and its next frame's number.
    frames: RefCell<HashMap<String, VecDeque<Value>>>,
    next: RefCell<HashMap<String, u64>>,
    watches: RefCell<HashMap<String, Slot>>,
    /// Content searches started: an answer for an older one is dropped.
    content_searches: Cell<u64>,
    /// This engine's device id, once asked (cleared on every connection).
    device: RefCell<Option<String>>,
    started: Cell<bool>,
}

thread_local! {
    static FILES: Rc<Files> = Rc::new(Files::new());
}

pub fn install(cx: &mut WindowContext, host: &ScriptHost) {
    engine::install(cx);
    let files = FILES.with(Rc::clone);
    files.events.borrow_mut().push(host.shared("files.events", files.events_value()));
    files.git.borrow_mut().push(host.shared("files.git", files.git_value()));
    let f = files.clone();
    host.function("files.frames", move |arg| f.frames_since(&arg));
    if !files.started.get() {
        files.started.set(true);
        let f = files.clone();
        engine::get().on_connect(move |_| {
            // A new connection: another engine, perhaps, and every watch
            // died with the last one.
            f.device.borrow_mut().take();
            let keys: Vec<String> = f.watches.borrow().keys().cloned().collect();
            for key in keys {
                f.clone().restart(key, true);
            }
        });
    }

    let f = files.clone();
    host.async_action("files.list", move |arg| {
        let f = f.clone();
        async move { f.list(arg).await }
    });
    let f = files.clone();
    host.async_action("files.read", move |arg| {
        let f = f.clone();
        async move { f.read(arg).await }
    });
    let f = files.clone();
    host.async_action("files.chat_image", move |arg| {
        let f = f.clone();
        async move { f.chat_image(arg).await }
    });
    let f = files.clone();
    host.async_action("files.attachment", move |arg| {
        let f = f.clone();
        async move { f.attachment(arg).await }
    });
    let f = files.clone();
    host.async_action("files.image", move |arg| {
        let f = f.clone();
        async move { f.image(arg).await }
    });
    let f = files.clone();
    host.async_action("files.artwork", move |arg| {
        let f = f.clone();
        async move { f.artwork(arg).await }
    });
    let f = files.clone();
    host.async_action("files.write", move |arg| {
        let f = f.clone();
        async move { crate::plugins::preview::refuse("files.write")?; f.write(arg).await }
    });
    let f = files.clone();
    host.async_action("files.move", move |arg| {
        let f = f.clone();
        async move { crate::plugins::preview::refuse("files.move")?; f.move_entry(arg).await }
    });
    let f = files.clone();
    host.async_action("files.delete", move |arg| {
        let f = f.clone();
        async move { crate::plugins::preview::refuse("files.delete")?; f.delete(arg).await }
    });
    let f = files.clone();
    host.async_action("files.search", move |arg| {
        let f = f.clone();
        async move { f.search(arg).await }
    });
    let f = files.clone();
    host.async_action("files.search_content", move |arg| {
        let f = f.clone();
        async move { f.search_content(arg).await }
    });
    let f = files.clone();
    host.async_action("files.create", move |arg| {
        let f = f.clone();
        async move { crate::plugins::preview::refuse("files.create")?; f.create(arg).await }
    });
    let f = files.clone();
    host.async_action("files.reveal", move |arg| {
        let f = f.clone();
        async move { f.show(arg, true).await }
    });
    let f = files.clone();
    host.async_action("files.open_external", move |arg| {
        let f = f.clone();
        async move { f.show(arg, false).await }
    });
    let f = files.clone();
    host.async_action("files.watch", move |arg| {
        let f = f.clone();
        async move {
            let key = text(arg.field("key"));
            let target = arg.field("target").cloned().ok_or("no target")?;
            if key.is_empty() {
                return Err("no key".into());
            }
            // The same folder's watch, still running (a session in the same
            // folder, or a screen made again within the grace): kept, its
            // frames all there.
            {
                let mut watches = f.watches.borrow_mut();
                if let Some(slot) = watches.get_mut(&key) {
                    // (Still starting: what it tells is read from the start,
                    // so nothing kept is assumed.)
                    let kept = slot.join(target);
                    return Ok(Value::record([("kept", Value::Bool(kept))]));
                }
                watches.insert(key.clone(), Slot { target, generation: 0, watch: None, git_watch: None, release: 0, users: 1 });
            }
            f.clone().restart(key, false);
            Ok(Value::record([("kept", Value::Bool(false))]))
        }
    });
    let f = files;
    host.action("files.unwatch", move |key| {
        let key = text(Some(&key));
        let token = {
            let mut watches = f.watches.borrow_mut();
            let Some(slot) = watches.get_mut(&key) else {
                return;
            };
            if !slot.leave() {
                return;
            }
            slot.release = operation_token();
            slot.release
        };
        let f = f.clone();
        engine::get().spawn(async move {
            zgx::timer::sleep(GRACE).await;
            let mut watches = f.watches.borrow_mut();
            if watches.get(&key).is_some_and(|slot| slot.release == token) {
                watches.remove(&key);
                drop(watches);
                f.set_git(&key, None);
                // (Its frames go; its numbering stays, so a script that
                // read some and watches again sees it missed the rest.)
                f.frames.borrow_mut().remove(&key);
            }
        });
    });
}

/// A number unique to this run, never 0.
fn operation_token() -> u64 {
    thread_local! {
        static NEXT: Cell<u64> = const { Cell::new(0) };
    }
    NEXT.with(|next| {
        next.set(next.get() + 1);
        next.get()
    })
}

/// The device to route a call to (`targetDeviceId`): `device` when it is
/// another device than this Mac's engine, else none.
pub(crate) async fn route_to(device: Option<String>) -> Option<String> {
    let device = device.filter(|d| !d.is_empty())?;
    let files = FILES.with(Rc::clone);
    match files.local_device().await {
        Some(local) if local == device => None,
        _ => Some(device),
    }
}

fn text(value: Option<&Value>) -> String {
    match value {
        Some(value) => crate::auth::text(value),
        None => String::new(),
    }
}

fn opt_text(value: Option<&Value>) -> Option<String> {
    let t = text(value);
    (!t.is_empty()).then_some(t)
}

/// Where a repository keeps its own picture, best first: the shipping app's
/// (crates/ui/src/shell/project_icon.rs `ICON_PATHS`), those zgx draws (PNG
/// and JPEG; its SVGs and ICOs aren't).
const ARTWORK_PATHS: &[&str] = &[
    "public/apple-touch-icon.png",
    "apple-touch-icon.png",
    "public/favicon.png",
    "public/icon.png",
    "public/logo.png",
    "favicon.png",
    "app/icon.png",
    "src/app/icon.png",
    "src-tauri/icons/icon.png",
    "assets/icon.png",
    "src/assets/icon.png",
];
/// The largest artwork taken (a repository's logo, not its hero image).
const ARTWORK_MAX: u64 = 2 * 1024 * 1024;

/// A local checkout's artwork: the first of `ARTWORK_PATHS` under the root
/// of the work tree holding `root` (a project may be a subfolder; a linked
/// worktree's `.git` is a file).
fn local_artwork(root: &Path) -> Option<PathBuf> {
    let canonical = root.canonicalize().ok()?;
    let top = canonical.ancestors().find(|p| p.join(".git").exists()).unwrap_or(&canonical);
    ARTWORK_PATHS.iter().map(|p| top.join(p)).find(|p| {
        std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.len() > 0 && m.len() <= ARTWORK_MAX)
    })
}

/// The most of an image read from another device, and its chunks (the
/// engine's `MAX_WORKSPACE_IMAGE_BYTES` and `WORKSPACE_IMAGE_CHUNK_BYTES`).
const IMAGE_MAX: usize = 8 * 1024 * 1024;
const IMAGE_CHUNK: usize = 384 * 1024;
/// The most of a sent or named picture read from another device: the
/// shipping app's attachments' (crates/ui attachments.rs
/// `MAX_ATTACHMENT_BYTES`), so any picture sent from it shows.
const PICTURE_MAX: usize = 24 * 1024 * 1024;

/// Where images read from other devices are kept: the app's own data
/// folder, never the system's shared temporary one (on Linux anyone's,
/// where another user could leave a file in a picture's place). What's
/// there a day old goes as the first is kept, each launch.
fn image_dir() -> Result<std::path::PathBuf, String> {
    engine::data_dir()
        .map(|d| d.join("picture-cache"))
        .ok_or_else(|| "The image couldn't be kept here.".to_string())
}

/// A sent or named picture as its chunks come (`ReadAttachmentChunk`,
/// `ReadChatImageChunk`): every chunk the same kind of picture, each one
/// bringing bytes and moving on, the whole at most `PICTURE_MAX`. So it's
/// read whole or refused however small the engine's chunks are, and
/// never read forever.
#[derive(Default)]
struct PictureRead {
    bytes: Vec<u8>,
    mime: String,
    offset: u64,
}

impl PictureRead {
    /// One chunk taken: `Ok(true)` once the picture is whole.
    fn take(&mut self, chunk: &Json) -> Result<bool, String> {
        use base64::Engine as _;
        let mime = chunk["mimeType"].as_str().unwrap_or("");
        if !matches!(mime, "image/png" | "image/jpeg" | "image/webp" | "image/gif")
            || (!self.mime.is_empty() && self.mime != mime)
        {
            return Err("That isn't a picture this can show.".into());
        }
        mime.clone_into(&mut self.mime);
        let data = base64::engine::general_purpose::STANDARD
            .decode(chunk["data"].as_str().unwrap_or(""))
            .map_err(|_| "The image came back garbled.".to_string())?;
        self.bytes.extend_from_slice(&data);
        if self.bytes.len() > PICTURE_MAX {
            return Err("The image is too large to show.".into());
        }
        if chunk["done"].as_bool().unwrap_or(false) {
            return Ok(true);
        }
        // (A chunk that brings nothing, or doesn't move on, would never end.)
        let next = chunk["nextOffset"].as_u64().unwrap_or(0);
        if data.is_empty() || next <= self.offset {
            return Err("The image changed while it was read.".into());
        }
        self.offset = next;
        Ok(false)
    }
}

/// `bytes` kept as a file named by their hash (one already there is the
/// same picture), with the extension its media type says.
fn keep_image(hash: &str, mime: &str, bytes: &[u8]) -> Result<std::path::PathBuf, String> {
    thread_local! {
        static PRUNED: Cell<bool> = const { Cell::new(false) };
    }
    let dir = image_dir()?;
    if !PRUNED.with(|p| p.replace(true))
        && let Ok(entries) = std::fs::read_dir(&dir)
    {
        let day = std::time::Duration::from_secs(24 * 60 * 60);
        for entry in entries.flatten() {
            let old = entry.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok());
            if old.is_some_and(|age| age > day) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    let name: String = hash.chars().filter(char::is_ascii_alphanumeric).take(64).collect();
    if name.is_empty() {
        return Err("The image came back without its hash.".into());
    }
    let ext = match mime {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/svg+xml" => "svg",
        "image/bmp" => "bmp",
        "image/tiff" => "tiff",
        _ => "img",
    };
    let file = dir.join(format!("{name}.{ext}"));
    if std::fs::metadata(&file).is_ok_and(|m| m.len() == bytes.len() as u64) {
        return Ok(file);
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("The image couldn't be kept: {e}"))?;
    // (Written aside and moved into place: another window reading the same
    // picture never sees half of it.)
    let part = dir.join(format!("{name}.{}.part", std::process::id()));
    std::fs::write(&part, bytes)
        .and_then(|_| std::fs::rename(&part, &file))
        .map_err(|e| format!("The image couldn't be kept: {e}"))?;
    Ok(file)
}

/// The engine's content matches as the scripts read them (lines and
/// columns from 1).
fn content_matches(found: &Json) -> Value {
    let matches = found["matches"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|m| {
            let ranges = m["ranges"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|r| {
                    let (a, b) = (r.get(0)?.as_u64()?, r.get(1)?.as_u64()?);
                    Some(Value::record([("from", Value::Num(a as f64)), ("to", Value::Num(b as f64))]))
                })
                .collect();
            Value::record([
                ("path", json_text(m, "path")),
                ("line", num(m["line"].as_u64())),
                // (The engine's is 0-based bytes; lines and columns go
                // out from 1, as `Panel.open_file` takes them.)
                ("column", num(m["column"].as_u64().map(|c| c + 1))),
                ("text", json_text(m, "preview")),
                ("ranges", list(ranges)),
            ])
        })
        .collect();
    Value::record([
        ("matches", list(matches)),
        ("truncated", Value::Bool(found["truncated"].as_bool().unwrap_or(false))),
        ("indexing", Value::Bool(found["indexing"].as_bool().unwrap_or(false))),
    ])
}

fn num(n: Option<u64>) -> Value {
    n.map_or(Value::Nil, |n| Value::Num(n as f64))
}

fn json_text(value: &Json, key: &str) -> Value {
    value[key].as_str().map_or(Value::Nil, Value::from)
}

fn list(items: Vec<Value>) -> Value {
    Value::List(items.into())
}

/// A word for the screen from an engine error: "offline" when it couldn't
/// be reached, what to do when it's from before this call, else what it
/// said.
fn said(error: String) -> String {
    if engine::outcome(&error) == "offline" {
        "offline".into()
    } else if error.to_ascii_lowercase().contains("unknown method") {
        "Update Zeron to use this here.".into()
    } else {
        error
    }
}

impl Files {
    fn new() -> Self {
        Self {
            events: RefCell::default(),
            git: RefCell::default(),
            statuses: RefCell::default(),
            frames: RefCell::default(),
            next: RefCell::default(),
            watches: RefCell::default(),
            content_searches: Cell::new(0),
            device: RefCell::default(),
            started: Cell::new(false),
        }
    }

    fn git_value(&self) -> Value {
        let statuses = self.statuses.borrow();
        // (`record` sorts its keys: a map's are looked up by halves.)
        Value::record(statuses.iter().map(|(k, v)| (k.as_str(), v.clone())))
    }

    /// Folder `key`'s git status now (`None`: not known, cleared).
    fn set_git(&self, key: &str, files: Option<Value>) {
        {
            let mut statuses = self.statuses.borrow_mut();
            match files {
                Some(files) => {
                    statuses.insert(key.to_owned(), files);
                }
                None => {
                    if statuses.remove(key).is_none() {
                        return;
                    }
                }
            }
        }
        let value = self.git_value();
        for shared in self.git.borrow().iter() {
            shared.set(value.clone());
        }
    }

    fn events_value(&self) -> Value {
        Value::record(self.next.borrow().iter().map(|(key, n)| (key.as_str(), Value::Num(*n as f64))))
    }

    /// `files.frames { key, since }`: the folder's kept frames past `since`.
    fn frames_since(&self, arg: &Value) -> Value {
        let key = text(arg.field("key"));
        let since = match arg.field("since") {
            Some(Value::Num(n)) => *n,
            _ => 0.,
        };
        let frames = self.frames.borrow();
        let out: Vec<Value> = frames
            .get(&key)
            .into_iter()
            .flatten()
            .filter(|f| matches!(f.field("n"), Some(Value::Num(n)) if *n > since))
            .cloned()
            .collect();
        list(out)
    }

    fn push_frame(&self, key: &str, sequence: Option<u64>, resync: bool, changes: Vec<Value>) {
        let n = {
            let mut next = self.next.borrow_mut();
            let at = next.entry(key.to_string()).or_insert(1);
            let n = *at;
            *at += 1;
            n
        };
        let frame = Value::record([
            ("n", Value::Num(n as f64)),
            ("key", Value::from(key)),
            ("sequence", num(sequence)),
            ("resync", Value::Bool(resync)),
            ("changes", list(changes)),
        ]);
        {
            let mut all = self.frames.borrow_mut();
            let frames = all.entry(key.to_string()).or_default();
            frames.push_back(frame);
            while frames.len() > KEPT {
                frames.pop_front();
            }
        }
        let value = self.events_value();
        for shared in self.events.borrow().iter() {
            shared.set(value.clone());
        }
    }

    /// This engine's device: a target elsewhere carries its own.
    async fn local_device(&self) -> Option<String> {
        if let Some(id) = self.device.borrow().clone() {
            return Some(id);
        }
        let info = engine::get().call("EngineInfo", json!({})).await.ok()?;
        let id = info["deviceId"].as_str()?.to_string();
        *self.device.borrow_mut() = Some(id.clone());
        Some(id)
    }

    /// Whether `target` is on this Mac.
    async fn is_local(&self, target: &Value) -> bool {
        match opt_text(target.field("device")) {
            None => true,
            Some(device) => self.local_device().await.is_some_and(|local| local == device),
        }
    }

    /// The request's fields for `target`: the workspace (`chatId` or
    /// `spaceId`) and, for another device, `targetDeviceId`.
    async fn params(&self, target: Option<&Value>) -> Result<Map<String, Json>, String> {
        let target = target.ok_or("no target")?;
        let mut out = Map::new();
        if let Some(chat) = opt_text(target.field("chat")) {
            out.insert("chatId".into(), Json::String(chat));
        } else if let Some(space) = opt_text(target.field("space")) {
            out.insert("spaceId".into(), Json::String(space));
        } else {
            return Err("no session or project".into());
        }
        if let Some(device) = opt_text(target.field("device"))
            && self.local_device().await.is_none_or(|local| local != device)
        {
            out.insert("targetDeviceId".into(), Json::String(device));
        }
        Ok(out)
    }

    async fn call(&self, method: &str, params: Map<String, Json>) -> Result<Json, String> {
        engine::get().call(method, Json::Object(params)).await.map_err(said)
    }

    async fn list(&self, arg: Value) -> Result<Value, String> {
        let mut params = self.params(arg.field("target")).await?;
        params.insert("directory".into(), Json::String(text(arg.field("dir"))));
        params.insert("includeIgnored".into(), Json::Bool(true));
        let mut entries = Vec::new();
        let mut first: Option<Json> = None;
        let mut truncated = false;
        for page_number in 0.. {
            let page = self.call("ListWorkspaceDirectory", params.clone()).await?;
            for entry in page["entries"].as_array().into_iter().flatten() {
                entries.push(entry_value(entry));
            }
            truncated = page["truncated"].as_bool().unwrap_or(false);
            let cursor = page["nextCursor"].as_str().map(str::to_owned);
            if first.is_none() {
                first = Some(page);
            }
            match cursor {
                Some(cursor) if page_number + 1 < MAX_PAGES => {
                    params.insert("cursor".into(), Json::String(cursor));
                }
                Some(_) => {
                    truncated = true;
                    break;
                }
                None => break,
            }
        }
        let first = first.unwrap_or(Json::Null);
        let caps = &first["mutationCapabilities"];
        let here = match arg.field("target") {
            Some(target) => self.is_local(target).await,
            None => false,
        };
        Ok(Value::record([
            ("here", Value::Bool(here)),
            ("checkout", json_text(&first, "checkoutId")),
            ("can_move", Value::Bool(caps["moveEntry"].as_bool().unwrap_or(false))),
            ("can_delete", Value::Bool(caps["deleteEntry"].as_bool().unwrap_or(false))),
            ("truncated", Value::Bool(truncated)),
            ("entries", list(entries)),
        ]))
    }

    async fn read(&self, arg: Value) -> Result<Value, String> {
        let mut params = self.params(arg.field("target")).await?;
        params.insert("path".into(), Json::String(text(arg.field("path"))));
        let file = self.call("ReadWorkspaceFile", params).await?;
        let here = match arg.field("target") {
            Some(target) => self.is_local(target).await,
            None => false,
        };
        Ok(Value::record([
            ("here", Value::Bool(here)),
            ("checkout", json_text(&file, "checkoutId")),
            ("path", json_text(&file, "path")),
            ("text", json_text(&file, "text")),
            ("hash", json_text(&file, "contentHash")),
            ("size", num(file["size"].as_u64())),
            ("encoding", json_text(&file, "encoding")),
            ("line_ending", json_text(&file, "lineEnding")),
            ("readonly", json_text(&file, "readOnlyReason")),
            ("truncated", Value::Bool(file["truncated"].as_bool().unwrap_or(false))),
        ]))
    }

    /// An image on another device: its bytes, read from that device a
    /// chunk at a time (the shipping app's `read_image`), kept in a file here
    /// named by their hash, which `ui.image` shows (and lets go of as it
    /// does any file's).
    async fn image(&self, arg: Value) -> Result<Value, String> {
        use base64::Engine as _;
        let mut params = self.params(arg.field("target")).await?;
        let path = text(arg.field("path"));
        let checkout = text(arg.field("checkout"));
        // (A file outside the project goes by its own path, no checkout.)
        if checkout.is_empty() && !path.starts_with('/') {
            return Err("The project's checkout isn't known.".into());
        }
        params.insert("path".into(), Json::String(path));
        params.insert("expectedCheckoutId".into(), Json::String(checkout.clone()));
        params.insert("offset".into(), json!(0));
        params.insert("expectedContentHash".into(), Json::Null);
        let mut bytes = Vec::new();
        let (mut mime, mut size, mut hash) = (None::<String>, None::<u64>, None::<String>);
        for _ in 0..=IMAGE_MAX / IMAGE_CHUNK {
            let chunk = self.call("ReadWorkspaceImage", params.clone()).await?;
            let this_hash = chunk["contentHash"].as_str().unwrap_or("").to_owned();
            let this_mime = chunk["mimeType"].as_str().unwrap_or("").to_owned();
            let this_size = chunk["size"].as_u64().unwrap_or(0);
            let data = base64::engine::general_purpose::STANDARD
                .decode(chunk["data"].as_str().unwrap_or(""))
                .map_err(|_| "The image came back garbled.".to_string())?;
            if chunk["checkoutId"].as_str().unwrap_or("") != checkout
                || this_hash.is_empty()
                || this_size > IMAGE_MAX as u64
                || data.len() > IMAGE_CHUNK
                || hash.as_ref().is_some_and(|h| *h != this_hash)
                || mime.as_ref().is_some_and(|m| *m != this_mime)
                || size.is_some_and(|s| s != this_size)
            {
                return Err("The image changed while it was read.".into());
            }
            bytes.extend_from_slice(&data);
            if bytes.len() as u64 > this_size {
                return Err("The image changed while it was read.".into());
            }
            (mime, size, hash) = (Some(this_mime), Some(this_size), Some(this_hash));
            if chunk["done"].as_bool().unwrap_or(false) {
                if bytes.len() as u64 != this_size {
                    return Err("The image changed while it was read.".into());
                }
                let file = keep_image(hash.as_deref().unwrap_or(""), mime.as_deref().unwrap_or(""), &bytes)?;
                return Ok(Value::record([
                    ("src", Value::from(file.to_string_lossy().as_ref())),
                    ("size", num(size)),
                ]));
            }
            params.insert("offset".into(), json!(chunk["nextOffset"].as_u64().unwrap_or(0)));
            params.insert("expectedContentHash".into(), Json::String(hash.clone().unwrap_or_default()));
        }
        Err("The image is too large to show.".into())
    }

    /// A project's own picture (its repository's logo, as the shipping app
    /// finds it): `{ src }`, a file here, or nil when it has none. This
    /// Mac's checkout is looked at on disk; another device's through the
    /// engine, a path at a time, its bytes kept here by their hash.
    async fn artwork(&self, arg: Value) -> Result<Value, String> {
        let target = arg.field("target").ok_or("no target")?;
        let found = |path: PathBuf| Value::record([("src", Value::from(path.to_string_lossy().as_ref()))]);
        if self.is_local(target).await {
            let root = PathBuf::from(text(target.field("root")));
            if !root.is_absolute() {
                return Ok(Value::Nil);
            }
            // (A dozen stats on this Mac's disk: done here.)
            return Ok(local_artwork(&root).map_or(Value::Nil, found));
        }
        let params = self.params(Some(target)).await?;
        for path in ARTWORK_PATHS {
            let mut read = params.clone();
            read.insert("path".into(), Json::String((*path).into()));
            // (Not there: the next. The device unreachable: none for now.)
            let file = match self.call("ReadWorkspaceFile", read).await {
                Ok(file) => file,
                Err(_) => continue,
            };
            if file["size"].as_u64().is_some_and(|n| n > ARTWORK_MAX) {
                continue;
            }
            let checkout = json_text(&file, "checkoutId");
            let arg = Value::record([
                ("target", target.clone()),
                ("path", Value::from(*path)),
                ("checkout", checkout),
            ]);
            if let Ok(image) = self.image(arg).await
                && let Some(src) = image.field("src")
            {
                return Ok(Value::record([("src", src.clone())]));
            }
        }
        Ok(Value::Nil)
    }

    /// A picture a reply names: read through the engine, which reads it
    /// only from strictly inside the chat's folder, never through a link,
    /// and only a PNG, JPEG, WebP or GIF whose first bytes say so; a chunk
    /// at a time, at most `PICTURE_MAX`; kept here as a file named by a hash
    /// of its bytes (one already there is the same picture).
    async fn chat_image(&self, arg: Value) -> Result<Value, String> {
        self.picture(arg, "ReadChatImageChunk").await
    }

    /// A picture sent with a message in a chat on another device: its bytes
    /// read from that device (`ReadAttachmentChunk`, the shipping app's
    /// transcript thumbnails: only the uploads there and the chats' folders),
    /// kept here as `chat_image`'s are.
    async fn attachment(&self, arg: Value) -> Result<Value, String> {
        self.picture(arg, "ReadAttachmentChunk").await
    }

    /// A picture's bytes, a chunk at a time by `method`, kept as a file here
    /// named by a hash of them: `{ src, size }`.
    async fn picture(&self, arg: Value, method: &str) -> Result<Value, String> {
        use std::hash::{Hash, Hasher};
        let mut params = self.params(arg.field("target")).await?;
        if !params.contains_key("chatId") {
            return Err("no chat".into());
        }
        // (An attachment goes by its path alone, on the chat's device.)
        if method == "ReadAttachmentChunk" {
            params.remove("chatId");
        }
        let path = text(arg.field("path"));
        if !path.starts_with('/') {
            return Err("That picture isn't a path on the chat's device.".into());
        }
        params.insert("path".into(), Json::String(path));
        let mut read = PictureRead::default();
        loop {
            params.insert("offset".into(), json!(read.offset));
            let chunk = self.call(method, params.clone()).await?;
            if read.take(&chunk)? {
                break;
            }
        }
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        read.bytes.hash(&mut hasher);
        let hash = format!("r{:016x}{:x}", hasher.finish(), read.bytes.len());
        let file = keep_image(&hash, &read.mime, &read.bytes)?;
        Ok(Value::record([
            ("src", Value::from(file.to_string_lossy().as_ref())),
            ("size", num(Some(read.bytes.len() as u64))),
        ]))
    }

    /// Lines holding the query (the engine's `SearchWorkspaceContent`, the
    /// shipping app's palette "In Files": its caps), answered only if no
    /// newer search started meanwhile. The engine works off this thread;
    /// this only waits for it.
    async fn search_content(&self, arg: Value) -> Result<Value, String> {
        let mine = self.content_searches.get() + 1;
        self.content_searches.set(mine);
        let mut params = self.params(arg.field("target")).await?;
        let cap = |key: &str, fallback: u64, most: u64| {
            arg.field(key).and_then(Value::as_f64).map_or(fallback, |n| (n.max(1.) as u64).min(most))
        };
        params.insert("query".into(), Json::String(text(arg.field("query"))));
        params.insert("limit".into(), json!(cap("limit", 100, 500)));
        params.insert("perFileLimit".into(), json!(cap("per_file", 3, 50)));
        let found = self.call("SearchWorkspaceContent", params).await?;
        if self.content_searches.get() != mine {
            return Err("superseded".into());
        }
        Ok(content_matches(&found))
    }

    async fn search(&self, arg: Value) -> Result<Value, String> {
        let mut params = self.params(arg.field("target")).await?;
        params.insert("query".into(), Json::String(text(arg.field("query"))));
        params.insert("includeIgnored".into(), Json::Bool(false));
        params.insert("limit".into(), json!(200));
        let found = self.call("SearchWorkspaceFiles", params).await?;
        let items = found
            .as_array()
            .into_iter()
            .flatten()
            .map(|m| {
                Value::record([
                    ("path", json_text(m, "path")),
                    ("dir", Value::Bool(m["kind"].as_str() == Some("directory"))),
                ])
            })
            .collect();
        Ok(list(items))
    }

    async fn write(&self, arg: Value) -> Result<Value, String> {
        let mut params = self.params(arg.field("target")).await?;
        let encoding = match text(arg.field("encoding")).as_str() {
            "utf8Bom" => "utf8Bom",
            _ => "utf8",
        };
        let line_ending = match text(arg.field("line_ending")).as_str() {
            "crlf" => "crlf",
            _ => "lf",
        };
        params.insert("path".into(), Json::String(text(arg.field("path"))));
        params.insert("text".into(), Json::String(text(arg.field("text"))));
        params.insert("expectedContentHash".into(), Json::String(text(arg.field("hash"))));
        params.insert("expectedCheckoutId".into(), Json::String(text(arg.field("checkout"))));
        params.insert("encoding".into(), Json::String(encoding.into()));
        params.insert("lineEnding".into(), Json::String(line_ending.into()));
        let outcome = self.call("WriteWorkspaceFile", params).await?;
        Ok(match outcome["status"].as_str() {
            Some("written") => {
                let file = &outcome["file"];
                Value::record([
                    ("status", Value::from("written")),
                    ("hash", json_text(file, "contentHash")),
                    ("size", num(file["size"].as_u64())),
                ])
            }
            _ => Value::record([
                ("status", Value::from("conflict")),
                ("reason", json_text(&outcome, "reason")),
                ("hash", json_text(&outcome, "currentContentHash")),
            ]),
        })
    }

    fn mutation(outcome: &Json) -> Value {
        match outcome["status"].as_str() {
            Some("applied") => Value::record([("status", Value::from("applied"))]),
            _ => Value::record([
                ("status", Value::from("rejected")),
                ("reason", json_text(outcome, "reason")),
                ("message", json_text(outcome, "message")),
            ]),
        }
    }

    async fn move_entry(&self, arg: Value) -> Result<Value, String> {
        let mut params = self.params(arg.field("target")).await?;
        params.insert("operationId".into(), Json::String(operation_id()));
        params.insert("expectedCheckoutId".into(), Json::String(text(arg.field("checkout"))));
        params.insert("sourcePath".into(), Json::String(text(arg.field("from"))));
        params.insert("destinationPath".into(), Json::String(text(arg.field("to"))));
        params.insert("expectedSourceRevision".into(), Json::String(text(arg.field("rev"))));
        params.insert("expectedKind".into(), Json::String(kind(arg.field("kind"))));
        let outcome = self.call("MoveWorkspaceEntry", params).await?;
        Ok(Self::mutation(&outcome))
    }

    async fn delete(&self, arg: Value) -> Result<Value, String> {
        let mut params = self.params(arg.field("target")).await?;
        params.insert("operationId".into(), Json::String(operation_id()));
        params.insert("expectedCheckoutId".into(), Json::String(text(arg.field("checkout"))));
        params.insert("path".into(), Json::String(text(arg.field("path"))));
        params.insert("expectedSourceRevision".into(), Json::String(text(arg.field("rev"))));
        params.insert("expectedKind".into(), Json::String(kind(arg.field("kind"))));
        params.insert(
            "recursive".into(),
            Json::Bool(arg.field("recursive").is_some_and(Value::truthy)),
        );
        let outcome = self.call("DeleteWorkspaceEntry", params).await?;
        Ok(Self::mutation(&outcome))
    }

    /// An empty file or folder at `path`: the engine's, on whichever device
    /// hosts it. An engine from before the call makes it here, on this Mac.
    async fn create(&self, arg: Value) -> Result<Value, String> {
        let target = arg.field("target").ok_or("no target")?;
        let relative = text(arg.field("path"));
        let dir = arg.field("dir").is_some_and(Value::truthy);
        let mut params = self.params(Some(target)).await?;
        params.insert("operationId".into(), Json::String(operation_id()));
        params.insert("expectedCheckoutId".into(), Json::String(text(arg.field("checkout"))));
        params.insert("path".into(), Json::String(relative.clone()));
        params.insert("kind".into(), Json::String(if dir { "directory" } else { "file" }.into()));
        match engine::get().call("CreateWorkspaceEntry", Json::Object(params)).await {
            Ok(outcome) => Ok(Self::mutation(&outcome)),
            Err(error) if error.to_ascii_lowercase().contains("unknown method") => {
                if !self.is_local(target).await {
                    return Err("That device's Zeron is too old to make files. Update it, then try again.".into());
                }
                let root = opt_text(target.field("root")).ok_or("This session has no folder to make files in")?;
                make(Path::new(&root), &relative, dir)?;
                Ok(Value::record([("status", Value::from("applied"))]))
            }
            Err(error) => Err(said(error)),
        }
    }

    /// A file on this Mac shown in Finder (`reveal`) or opened in its app.
    async fn show(&self, arg: Value, reveal: bool) -> Result<Value, String> {
        let target = arg.field("target").ok_or("no target")?;
        if !self.is_local(target).await {
            return Err("That file is on another device".into());
        }
        let root = opt_text(target.field("root")).ok_or("This session's folder isn't known")?;
        let path = inside(Path::new(&root), &text(arg.field("path")))?;
        let opened = if reveal { reveal_in_finder(&path) } else { open::that_detached(&path) };
        opened.map_err(|error| error.to_string())?;
        Ok(Value::Nil)
    }

    /// (Re)starts `key`'s watch on the connection, once it has one; a
    /// restart after a reconnection adds a `resync` frame.
    fn restart(self: Rc<Self>, key: String, resync: bool) {
        let generation = {
            let mut watches = self.watches.borrow_mut();
            let Some(slot) = watches.get_mut(&key) else {
                return;
            };
            slot.generation += 1;
            slot.watch = None;
            slot.git_watch = None;
            slot.generation
        };
        let me = self.clone();
        engine::get().spawn(async move {
            let current = |me: &Files| {
                me.watches.borrow().get(&key).filter(|s| s.generation == generation).map(|s| s.target.clone())
            };
            let Some(target) = current(&me) else {
                return;
            };
            let Some(rpc) = engine::get().connection(WATCH_WAIT).await else {
                // No engine yet: on_connect starts it when there is one.
                return;
            };
            let params = match me.params(Some(&target)).await {
                Ok(params) => params,
                Err(error) => {
                    eprintln!("zeron-zgx: files: can't follow {key}: {error}");
                    return;
                }
            };
            if current(&me).is_none() {
                return;
            }
            // Its git status beside it, on the same request (older engines
            // don't answer: the tree shows none).
            let (told, git_key) = (me.clone(), key.clone());
            let git_watch = rpc.watch("WatchWorkspaceGitStatus", Json::Object(params.clone()), move |item| {
                told.set_git(&git_key, git_files(&item["status"]));
            });
            let (heard, frame_key) = (me.clone(), key.clone());
            let watch = rpc.watch("WatchWorkspaceFiles", Json::Object(params), move |item| {
                let changes = item["changes"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|change| {
                        Value::record([
                            ("kind", json_text(change, "kind")),
                            ("path", json_text(change, "path")),
                            ("old_path", json_text(change, "oldPath")),
                        ])
                    })
                    .collect();
                heard.push_frame(
                    &frame_key,
                    item["sequence"].as_u64(),
                    item["resyncRequired"].as_bool().unwrap_or(false),
                    changes,
                );
            });
            let mut watches = me.watches.borrow_mut();
            if let Some(slot) = watches.get_mut(&key)
                && slot.generation == generation
            {
                slot.watch = Some(watch);
                slot.git_watch = Some(git_watch);
                drop(watches);
                if resync {
                    me.push_frame(&key, None, true, Vec::new());
                }
            }
        });
    }
}

/// What ds/file_tree calls a file's git status, from its index and
/// worktree states (the shipping app's `decoration`, git_status.rs).
fn git_kind(index: &str, worktree: &str) -> &'static str {
    let states = [index, worktree];
    let has = |s: &str| states.contains(&s);
    if has("unmerged") || matches!((index, worktree), ("added", "added") | ("deleted", "deleted")) {
        "conflict"
    } else if has("deleted") && has("untracked") {
        "modified"
    } else if has("deleted") {
        "deleted"
    } else if has("renamed") || has("copied") {
        "renamed"
    } else if has("modified") || has("typeChanged") {
        "modified"
    } else if has("untracked") {
        "untracked"
    } else {
        "added"
    }
}

/// A status frame's files, `{ [path] = status }`; `None` while it isn't
/// known whole (`complete = false` is unavailable, not clean).
fn git_files(status: &Json) -> Option<Value> {
    if !status.is_object() || !status["complete"].as_bool().unwrap_or(false) {
        return None;
    }
    let fields: Vec<(Rc<str>, Value)> = status["files"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|file| {
            let path = file["path"].as_str().filter(|p| !p.is_empty() && !p.starts_with('/'))?;
            let kind = git_kind(file["index"].as_str().unwrap_or(""), file["worktree"].as_str().unwrap_or(""));
            Some((Rc::from(path), Value::from(kind)))
        })
        .collect();
    // (Sorted, as a map's keys must be: by `record`.)
    Some(Value::record(fields))
}

fn entry_value(entry: &Json) -> Value {
    let kind = entry["kind"].as_str().unwrap_or("file");
    Value::record([
        ("name", json_text(entry, "name")),
        ("path", json_text(entry, "path")),
        ("dir", Value::Bool(kind == "directory")),
        ("kind", Value::from(kind)),
        ("size", num(entry["size"].as_u64())),
        ("ignored", Value::Bool(entry["ignored"].as_bool().unwrap_or(false))),
        ("readonly", Value::Bool(entry["readOnly"].as_bool().unwrap_or(false))),
        ("rev", json_text(entry, "mutationRevision")),
    ])
}

/// A mutation's id, unique to this run (the engine matches its change
/// frames to it).
fn operation_id() -> String {
    thread_local! {
        static NEXT: Cell<u64> = const { Cell::new(0) };
    }
    let n = NEXT.with(|next| {
        next.set(next.get() + 1);
        next.get()
    });
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("zgx-{:x}-{nanos:x}-{n}", std::process::id())
}

fn kind(value: Option<&Value>) -> String {
    match text(value).as_str() {
        "directory" => "directory",
        "symlink" => "symlink",
        _ => "file",
    }
    .into()
}

/// `relative` under `root`, as it is on disk (links followed), refused if
/// that's outside the root.
fn inside(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let rel = Path::new(relative);
    if !rel.components().all(|c| matches!(c, Component::Normal(_) | Component::CurDir)) {
        return Err("That path can't be used".into());
    }
    let root = root.canonicalize().map_err(|_| "This session's folder isn't there".to_string())?;
    let path = root.join(rel).canonicalize().map_err(|_| "That file isn't there any more".to_string())?;
    if !path.starts_with(&root) {
        return Err("That's outside this project".into());
    }
    Ok(path)
}

#[cfg(target_os = "macos")]
fn reveal_in_finder(path: &Path) -> std::io::Result<()> {
    spawn_reaped(std::process::Command::new("/usr/bin/open").arg("-R").arg(path)).map(|_| ())
}

/// `command` started, and waited on off this thread when it ends (a child
/// never waited on stays behind as a zombie: one per Reveal). Its pid.
#[cfg(unix)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn spawn_reaped(command: &mut std::process::Command) -> std::io::Result<u32> {
    let mut child = command.spawn()?;
    let pid = child.id();
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(pid)
}

/// Elsewhere: its folder, opened.
#[cfg(not(target_os = "macos"))]
fn reveal_in_finder(path: &Path) -> std::io::Result<()> {
    open::that_detached(path.parent().unwrap_or(path))
}

/// Makes an empty file (or a folder) at `relative` under `root`: a plain
/// relative path, its folder already there and inside the root (links
/// followed), nothing already at its name.
fn make(root: &Path, relative: &str, dir: bool) -> Result<(), String> {
    if !root.is_absolute() {
        return Err("This session's folder isn't known".into());
    }
    let rel = Path::new(relative);
    let plain = !relative.is_empty()
        && rel.components().all(|c| matches!(c, Component::Normal(_)))
        && !rel.components().any(|c| c.as_os_str() == ".git");
    if !plain {
        return Err("That name can't be used".into());
    }
    let root = root.canonicalize().map_err(|_| "This session's folder isn't there".to_string())?;
    let target: PathBuf = root.join(rel);
    let parent = target.parent().ok_or("That name can't be used")?;
    let parent = parent.canonicalize().map_err(|_| "Its folder isn't there any more".to_string())?;
    if !parent.starts_with(&root) {
        return Err("That's outside this project".into());
    }
    let name = target.file_name().ok_or("That name can't be used")?;
    let target = parent.join(name);
    let made = if dir {
        std::fs::create_dir(&target)
    } else {
        std::fs::OpenOptions::new().write(true).create_new(true).open(&target).map(|_| ())
    };
    made.map_err(|error| match error.kind() {
        std::io::ErrorKind::AlreadyExists => "Something already has that name".to_string(),
        std::io::ErrorKind::PermissionDenied => "Zeron isn't allowed to write there".to_string(),
        _ => error.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::make;

    /// A picture's chunk as the engine sends it (`ReadAttachmentChunk`).
    fn chunk(mime: &str, data: &[u8], next: u64, done: bool) -> serde_json::Value {
        use base64::Engine as _;
        serde_json::json!({
            "mimeType": mime,
            "data": base64::engine::general_purpose::STANDARD.encode(data),
            "nextOffset": next,
            "done": done,
        })
    }

    /// Feeds `size` bytes in `step`-sized chunks, as the engine would.
    fn read_all(size: usize, step: usize) -> Result<super::PictureRead, String> {
        let mut read = super::PictureRead::default();
        let mut at = 0usize;
        loop {
            let end = (at + step).min(size);
            let data = vec![7u8; end - at];
            if read.take(&chunk("image/png", &data, end as u64, end == size))? {
                return Ok(read);
            }
            assert_eq!(read.offset, end as u64);
            at = end;
        }
    }

    /// A sent screenshot of a few megabytes is read whole in the engine's
    /// 45 KB chunks (it was called too large past about 1 MB when the
    /// chunks were counted), up to the shipping app's 24 MB; past that,
    /// refused.
    #[test]
    fn a_picture_is_read_whole_in_any_chunks_up_to_the_most() {
        let read = read_all(1_700_000, 45_000).unwrap();
        assert_eq!(read.bytes.len(), 1_700_000);
        assert_eq!(read.mime, "image/png");
        assert_eq!(read_all(super::PICTURE_MAX, 1 << 20).unwrap().bytes.len(), super::PICTURE_MAX);
        assert_eq!(
            read_all(super::PICTURE_MAX + 1, 1 << 20).err().as_deref(),
            Some("The image is too large to show.")
        );
    }

    /// A chunk that brings nothing or doesn't move on is refused at once
    /// (never read forever), as is a picture changing its kind midway or
    /// one this can't show.
    #[test]
    fn a_picture_read_that_stalls_or_changes_is_refused() {
        let mut read = super::PictureRead::default();
        assert!(read.take(&chunk("image/png", &[], 10, false)).is_err());
        let mut read = super::PictureRead::default();
        assert_eq!(read.take(&chunk("image/png", &[1; 10], 10, false)), Ok(false));
        assert!(read.take(&chunk("image/png", &[1; 10], 10, false)).is_err());
        let mut read = super::PictureRead::default();
        assert_eq!(read.take(&chunk("image/png", &[1; 10], 10, false)), Ok(false));
        assert!(read.take(&chunk("image/jpeg", &[1; 10], 20, true)).is_err());
        assert!(super::PictureRead::default().take(&chunk("image/svg+xml", b"<svg/>", 6, true)).is_err());
    }

    /// A repository's artwork is found where the shipping app looks, from
    /// the work tree's root even for a project in a subfolder; one too big,
    /// or none, is none.
    #[test]
    fn a_projects_artwork_is_its_repositorys_logo() {
        let dir = std::env::temp_dir().join(format!("zgx-artwork-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::create_dir_all(dir.join("app/web")).unwrap();
        assert_eq!(super::local_artwork(&dir.join("app/web")), None, "none yet");
        std::fs::create_dir_all(dir.join("public")).unwrap();
        std::fs::write(dir.join("favicon.png"), b"png").unwrap();
        std::fs::write(dir.join("public/logo.png"), b"png").unwrap();
        // (Best first: public/logo.png before favicon.png.)
        let found = super::local_artwork(&dir.join("app/web")).unwrap();
        assert!(found.ends_with("public/logo.png"), "{found:?}");
        std::fs::write(dir.join("public/logo.png"), vec![0u8; (super::ARTWORK_MAX + 1) as usize]).unwrap();
        assert!(super::local_artwork(&dir).unwrap().ends_with("favicon.png"), "a too big one is passed over");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_child_started_for_the_finder_is_waited_on() {
        let pid = super::spawn_reaped(&mut std::process::Command::new("/usr/bin/true")).expect("started");
        // Gone once it ends: a zombie (never waited on) would still answer.
        let mut gone = false;
        for _ in 0..100 {
            if unsafe { libc::kill(pid as i32, 0) } != 0 {
                gone = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(gone, "pid {pid} stayed behind");
    }

    #[test]
    fn a_folder_watched_twice_is_let_go_only_by_the_last_and_starts_once() {
        use zgx_script::Value;
        let mut slot = super::Slot { target: Value::Nil, generation: 0, watch: None, git_watch: None, release: 0, users: 1 };
        // Asked for again while it starts: not running yet (not kept), and
        // no second start (the caller doesn't restart a joined slot).
        assert!(!slot.join(Value::Nil));
        assert_eq!(slot.users, 2);
        // One lets go: the other still has it.
        assert!(!slot.leave(), "one of two letting go mustn't end it");
        // The last does: its grace starts.
        assert!(slot.leave());
        // Let go of, then asked for within the grace: one user again.
        slot.release = 42;
        slot.join(Value::Nil);
        assert_eq!((slot.users, slot.release), (1, 0));
        // An unwatch past the count changes nothing.
        assert!(slot.leave());
        slot.release = 7;
        assert!(!slot.leave(), "a second unwatch mustn't start another grace");
    }

    #[test]
    fn content_matches_read_from_one_with_their_ranges() {
        use zgx_script::Value;
        let found = serde_json::json!({
            "matches": [{ "path": "src/a.rs", "line": 12, "column": 4, "preview": "let ack = 1;", "ranges": [[4, 7]] }],
            "truncated": true,
            "indexing": false,
        });
        let out = super::content_matches(&found);
        let Some(Value::List(matches)) = out.field("matches") else { panic!("no matches: {out:?}") };
        let m = &matches[0];
        assert_eq!(crate::auth::text(m.field("path").unwrap()), "src/a.rs");
        assert!(matches!(m.field("line"), Some(Value::Num(n)) if *n == 12.));
        assert!(matches!(m.field("column"), Some(Value::Num(n)) if *n == 5.), "a column from 1");
        let Some(Value::List(ranges)) = m.field("ranges") else { panic!("no ranges") };
        assert!(matches!(ranges[0].field("from"), Some(Value::Num(n)) if *n == 4.));
        assert!(matches!(out.field("truncated"), Some(Value::Bool(true))));
    }

    #[test]
    fn each_folder_reads_only_its_own_frames_numbered_its_own_way() {
        use zgx_script::Value;
        let files = super::Files::new();
        let n_of = |v: &Value| match v.field("n") {
            Some(Value::Num(n)) => *n as u64,
            _ => 0,
        };
        let asked = |key: &str, since: f64| {
            let arg = Value::record([("key", Value::from(key)), ("since", Value::Num(since))]);
            match files.frames_since(&arg) {
                Value::List(items) => items.iter().map(n_of).collect::<Vec<_>>(),
                other => panic!("not a list: {other:?}"),
            }
        };
        files.push_frame("a", Some(1), false, vec![]);
        files.push_frame("b", Some(1), false, vec![]);
        files.push_frame("a", Some(2), false, vec![]);
        assert_eq!(asked("a", 0.), vec![1, 2], "a's frames, numbered from 1");
        assert_eq!(asked("b", 0.), vec![1], "b's alone, numbered on its own");
        assert_eq!(asked("a", 1.), vec![2], "only those past what was read");
        assert_eq!(asked("c", 0.), Vec::<u64>::new(), "a folder not watched has none");
        // What's shared is each folder's next number, nothing more.
        let told = files.events_value();
        assert!(matches!(told.field("a"), Some(Value::Num(n)) if *n == 3.));
        assert!(matches!(told.field("b"), Some(Value::Num(n)) if *n == 2.));
        // Past the most kept, the oldest go: a reader that far behind sees a
        // gap and starts over.
        for _ in 0..super::KEPT + 5 {
            files.push_frame("b", None, false, vec![]);
        }
        let b = asked("b", 0.);
        assert_eq!(b.len(), super::KEPT);
        assert!(b[0] > 1);
    }

    #[test]
    fn git_states_read_as_the_tree_marks_them() {
        use super::{git_files, git_kind};
        assert_eq!(git_kind("unchanged", "modified"), "modified");
        assert_eq!(git_kind("added", "unchanged"), "added");
        assert_eq!(git_kind("unchanged", "untracked"), "untracked");
        assert_eq!(git_kind("renamed", "unchanged"), "renamed");
        assert_eq!(git_kind("deleted", "unchanged"), "deleted");
        assert_eq!(git_kind("unmerged", "unmerged"), "conflict");
        assert_eq!(git_kind("added", "added"), "conflict");
        // Not known whole: nothing rather than "all clean".
        assert!(git_files(&serde_json::json!({"complete": false, "files": []})).is_none());
        assert!(git_files(&serde_json::Value::Null).is_none());
        let files = git_files(&serde_json::json!({"complete": true, "files": [
            {"path": "src/a.rs", "index": "unchanged", "worktree": "modified"},
            {"path": "/abs", "index": "added", "worktree": "unchanged"},
        ]}))
        .expect("known");
        assert_eq!(files.field("src/a.rs").map(|v| crate::auth::text(v)).as_deref(), Some("modified"));
        assert!(files.field("/abs").is_none(), "a path outside the folder is left out");
    }

    #[test]
    fn makes_inside_the_root_only() {
        let dir = std::env::temp_dir().join(format!("zgx-files-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        make(&dir, "src/new.rs", false).unwrap();
        assert!(dir.join("src/new.rs").is_file());
        make(&dir, "docs", true).unwrap();
        assert!(dir.join("docs").is_dir());
        assert!(make(&dir, "src/new.rs", false).is_err(), "never over a file");
        assert!(make(&dir, "../out.rs", false).is_err());
        assert!(make(&dir, "/etc/x", false).is_err());
        assert!(make(&dir, ".git/x", false).is_err());
        assert!(make(&dir, "missing/x.rs", false).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn shows_only_what_is_inside_the_root() {
        use super::inside;
        let dir = std::env::temp_dir().join(format!("zgx-files-show-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/a.rs"), "").unwrap();
        assert!(inside(&dir, "src/a.rs").unwrap().ends_with("src/a.rs"));
        assert!(inside(&dir, "../etc").is_err());
        assert!(inside(&dir, "/etc/hosts").is_err());
        assert!(inside(&dir, "src/missing.rs").is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/etc", dir.join("away")).unwrap();
            assert!(inside(&dir, "away/hosts").is_err(), "a link out of the root is outside it");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
