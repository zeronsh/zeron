//! Changes under the templates folder, from FSEvents (macOS): the app waits
//! for one with nothing running meanwhile. It once walked the folder twice a
//! second, and the idle window woke for it every time.
//!
//! zgx watches its own scripts this way (zgx-script's `file_events.rs`, not
//! public); this is that, for files the app reads itself. One stream watches
//! the folders holding them, by path, so a file an editor replaces (a save as
//! write-and-rename) is heard as well as one written in place, and a folder
//! made or removed is heard in the folder it's in. Nothing is held open per
//! file: an app the Finder starts may open 256 files, and a descriptor for
//! each watched one used them all. Of a folder's entries only the watched
//! ones count, so the Finder's `.DS_Store` beside them wakes nothing.
use std::{
    collections::{HashMap, HashSet},
    ffi::{CStr, OsString, c_char, c_void},
    os::unix::{ffi::OsStrExt, fs::DirEntryExt, fs::MetadataExt},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, PoisonError},
};

pub struct FileEvents {
    /// What the stream's callback matches its events against, and whom it
    /// tells.
    shared: Arc<Shared>,
    /// The stream, while anything is watched, and the folders it watches.
    stream: Option<Stream>,
    tell: zgx::task::LatestSender<()>,
    changed: zgx::task::LatestReceiver<()>,
}

/// Shared with the stream, which holds it while it may call back.
struct Shared {
    wanted: Mutex<Wanted>,
    tell: zgx::task::LatestSender<()>,
}

/// The paths a change to which is heard, as FSEvents names them (with every
/// symbolic link and firmlink resolved: `/private/tmp`, not `/tmp`).
#[derive(Default)]
struct Wanted {
    /// Each watched file (and, if it's a link, what it links to): written,
    /// replaced or removed.
    files: HashSet<PathBuf>,
    /// Each watched folder: an entry made, removed or renamed, or the folder
    /// itself.
    /// Its entries as last seen (each one's inode), to tell an entry made,
    /// removed or replaced from one written in place: FSEvents' flags say
    /// "created" of a file made a while ago.
    dirs: HashMap<PathBuf, HashMap<OsString, u64>>,
}

impl FileEvents {
    /// Nothing watched yet; `None` if the system gives no queue to hear
    /// changes on.
    pub fn new() -> Option<Self> {
        let (tell, changed) = zgx::task::latest::<()>();
        Some(Self { shared: Arc::new(Shared { wanted: Mutex::default(), tell: tell.clone() }), stream: None, tell, changed })
    }

    /// Watch `files` (written, replaced, removed) and `dirs` (an entry made,
    /// removed or renamed) from now on, instead of what was watched before.
    /// Each call resolves their links again (one may point elsewhere now);
    /// the stream starts again only if the folders it watches changed.
    pub fn watch(&mut self, files: Vec<PathBuf>, dirs: Vec<PathBuf>) {
        let (wanted, roots) = resolve(&files, &dirs);
        // Matched against before the stream (if new) can hear anything.
        *self.shared.wanted.lock().unwrap_or_else(PoisonError::into_inner) = wanted;
        if self.stream.as_ref().map(|s| &s.roots) != Some(&roots) {
            // The new stream starts before the old one stops: a change between
            // the two is heard by one of them.
            self.stream = (!roots.is_empty()).then(|| Stream::start(roots, &self.shared)).flatten();
        }
    }

    /// Something to wake `changed` without the kernel (the folder moved).
    pub fn poke(&self) -> zgx::task::LatestSender<()> {
        self.tell.clone()
    }

    /// Wait for a change to what is watched, or a poke; `false` once
    /// nothing can change any more.
    pub async fn changed(&mut self) -> bool {
        self.changed.recv().await.is_some()
    }
}

/// What `files` and `dirs` are, as FSEvents names them, and the fewest
/// folders that hold them all (a stream hears everything under each).
fn resolve(files: &[PathBuf], dirs: &[PathBuf]) -> (Wanted, Vec<PathBuf>) {
    let mut wanted = Wanted::default();
    let mut folders = Vec::new();
    // Many files share a folder: each is resolved once.
    let mut resolved: HashMap<&Path, PathBuf> = HashMap::new();
    for file in files {
        let (Some(dir), Some(name)) = (file.parent(), file.file_name()) else { continue };
        let dir = resolved.entry(dir).or_insert_with(|| real(if dir.as_os_str().is_empty() { Path::new(".") } else { dir })).clone();
        // A link is heard where it is and where it leads.
        if std::fs::symlink_metadata(file).is_ok_and(|m| m.file_type().is_symlink())
            && let Ok(target) = std::fs::canonicalize(file)
        {
            if let Some(parent) = target.parent() {
                folders.push(parent.to_path_buf());
            }
            wanted.files.insert(target);
        }
        wanted.files.insert(dir.join(name));
        folders.push(dir);
    }
    for dir in dirs {
        let dir = real(dir);
        wanted.dirs.insert(dir.clone(), entries(&dir));
        folders.push(dir);
    }
    (wanted, roots(folders))
}

/// `path` with its links resolved, as far as it exists (the rest as it is):
/// a folder not made yet is heard once it is.
fn real(path: &Path) -> PathBuf {
    let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut rest = Vec::new();
    let mut at = path.as_path();
    loop {
        if let Ok(found) = std::fs::canonicalize(at) {
            return rest.into_iter().rev().fold(found, |p, name| p.join(name));
        }
        match (at.parent(), at.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name.to_owned());
                at = parent;
            }
            _ => return path,
        }
    }
}

/// `folders` without those inside another of them.
fn roots(mut folders: Vec<PathBuf>) -> Vec<PathBuf> {
    folders.sort();
    folders.dedup();
    // Sorted, a folder's insides follow it.
    let mut kept: Vec<PathBuf> = Vec::new();
    for folder in folders {
        if !kept.last().is_some_and(|k| folder.starts_with(k)) {
            kept.push(folder);
        }
    }
    kept
}

impl Wanted {
    /// Whether an event FSEvents reports, of `path` with `flags`, is a change
    /// to what is watched: a watched file's, a watched folder made, removed or
    /// moved, or an entry of one made, removed or replaced (not written in
    /// place: the folder doesn't change).
    fn concerns(&mut self, path: &Path, flags: u32) -> bool {
        // Events lost, or a watched folder itself moved: look again.
        if flags & (MUST_SCAN_SUB_DIRS | USER_DROPPED | KERNEL_DROPPED | ROOT_CHANGED) != 0 {
            return true;
        }
        if self.files.contains(path) {
            return true;
        }
        if self.dirs.contains_key(path) {
            return flags & (ITEM_CREATED | ITEM_REMOVED | ITEM_RENAMED) != 0;
        }
        let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
            return false;
        };
        let Some(entries) = self.dirs.get_mut(dir).filter(|_| name != ".DS_Store") else {
            return false;
        };
        let now = std::fs::symlink_metadata(path).ok().map(|m| m.ino());
        if now == entries.get(name).copied() {
            return false;
        }
        match now {
            Some(inode) => entries.insert(name.to_owned(), inode),
            None => entries.remove(name),
        };
        true
    }
}

/// The entries of `dir` and their inodes (none if it isn't there).
fn entries(dir: &Path) -> HashMap<OsString, u64> {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| (entry.file_name(), entry.ino()))
        .collect()
}

/// An FSEvents stream over `roots`, calling back on a queue of its own.
struct Stream {
    roots: Vec<PathBuf>,
    stream: FSEventStreamRef,
    /// The queue it calls back on, released after it (fields drop after
    /// `drop`).
    _queue: DispatchQueue,
}

/// How long the stream gathers events before it calls back. The first after
/// a quiet spell comes at once (`NO_DEFER`); this is the wait for the rest of
/// a burst, a save's write and rename.
const LATENCY: f64 = 0.01;

impl Stream {
    fn start(roots: Vec<PathBuf>, shared: &Arc<Shared>) -> Option<Self> {
        let paths: Vec<CFTypeRef> = roots
            .iter()
            .filter_map(|root| std::ffi::CString::new(root.as_os_str().as_bytes()).ok())
            // SAFETY: a NUL-terminated path, read for the call; a new string
            // or null.
            .map(|c| unsafe { CFStringCreateWithFileSystemRepresentation(std::ptr::null(), c.as_ptr()) })
            .filter(|s| !s.is_null())
            .collect();
        // SAFETY: `paths` holds `paths.len()` strings, which the array retains
        // (the CFType callbacks) while it lives.
        let array = unsafe { CFArrayCreate(std::ptr::null(), paths.as_ptr(), paths.len() as isize, &kCFTypeArrayCallBacks) };
        for path in &paths {
            // SAFETY: each was created above and is released once; the array
            // holds its own.
            unsafe { CFRelease(*path) };
        }
        if array.is_null() {
            return None;
        }
        let context = FSEventStreamContext {
            version: 0,
            info: Arc::as_ptr(shared).cast_mut().cast(),
            retain: Some(retain),
            release: Some(release),
            copy_description: None,
        };
        // SAFETY: `context` is read for the call, its `info` retained by the
        // stream through `retain`; `array` is a CFArray of CFStrings.
        let stream =
            unsafe { FSEventStreamCreate(std::ptr::null(), callback, &context, array, SINCE_NOW, LATENCY, FILE_EVENTS | NO_DEFER) };
        // SAFETY: created above; the stream holds its own if it wants it.
        unsafe { CFRelease(array) };
        if stream.is_null() {
            return None;
        }
        // SAFETY: a NUL-terminated label, and no attributes: a serial queue.
        let queue = DispatchQueue(unsafe { dispatch_queue_create(c"zeron-zgx templates".as_ptr(), std::ptr::null()) });
        // SAFETY: the stream is created and not yet started; `queue` lives as
        // long as `Self` (it's released after the stream is invalidated).
        let started = unsafe {
            FSEventStreamSetDispatchQueue(stream, queue.0);
            FSEventStreamStart(stream) != 0
        };
        let stream = Self { roots, stream, _queue: queue };
        started.then_some(stream)
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        // SAFETY: the stream was created and given its queue in `start`;
        // stopped (if it started) and invalidated before its last release,
        // which releases its `info` (`release`) once nothing more can call
        // back.
        unsafe {
            FSEventStreamStop(self.stream);
            FSEventStreamInvalidate(self.stream);
            FSEventStreamRelease(self.stream);
        }
    }
}

struct DispatchQueue(*mut c_void);

impl Drop for DispatchQueue {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the queue `dispatch_queue_create` gave, released once;
            // what was queued on it keeps it until it has run.
            unsafe { dispatch_release(self.0) };
        }
    }
}

// The stream's handle on `Shared`: one count of the `Arc` per retain.
extern "C" fn retain(info: *const c_void) -> *const c_void {
    // SAFETY: `info` is `Arc::as_ptr` of a `Shared` some holder keeps alive
    // (the `FileEvents` creating the stream, or the stream's own count).
    unsafe { Arc::increment_strong_count(info.cast::<Shared>()) };
    info
}

extern "C" fn release(info: *const c_void) {
    // SAFETY: one count `retain` took, given back once.
    unsafe { Arc::decrement_strong_count(info.cast::<Shared>()) };
}

extern "C" fn callback(_stream: FSEventStreamRef, info: *mut c_void, count: usize, paths: *mut c_void, flags: *const u32, _ids: *const u64) {
    // SAFETY: the stream holds a count of `info` (`retain`) while it calls
    // back.
    let shared = unsafe { &*info.cast::<Shared>() };
    let paths = paths.cast::<*const c_char>();
    let mut wanted = shared.wanted.lock().unwrap_or_else(PoisonError::into_inner);
    let concerned = (0..count).any(|i| {
        // SAFETY: FSEvents passes `count` paths (C strings, as the stream
        // wasn't made with CF types) and as many flags, valid for the call.
        let (path, flags) = unsafe { (CStr::from_ptr(*paths.add(i)), *flags.add(i)) };
        wanted.concerns(Path::new(std::ffi::OsStr::from_bytes(path.to_bytes())), flags)
    });
    drop(wanted);
    if concerned {
        shared.tell.send(());
    }
}

// CoreServices' FSEvents and what it takes, from CoreFoundation and
// libdispatch.
type CFTypeRef = *const c_void;
type FSEventStreamRef = *mut c_void;

#[repr(C)]
struct FSEventStreamContext {
    version: isize,
    info: *mut c_void,
    retain: Option<extern "C" fn(*const c_void) -> *const c_void>,
    release: Option<extern "C" fn(*const c_void)>,
    copy_description: Option<extern "C" fn(*const c_void) -> CFTypeRef>,
}

type FSEventStreamCallback = extern "C" fn(FSEventStreamRef, *mut c_void, usize, *mut c_void, *const u32, *const u64);

const SINCE_NOW: u64 = u64::MAX;
// Stream flags.
const NO_DEFER: u32 = 0x02;
const FILE_EVENTS: u32 = 0x10;
// Event flags.
const MUST_SCAN_SUB_DIRS: u32 = 0x01;
const USER_DROPPED: u32 = 0x02;
const KERNEL_DROPPED: u32 = 0x04;
const ROOT_CHANGED: u32 = 0x20;
const ITEM_CREATED: u32 = 0x100;
const ITEM_REMOVED: u32 = 0x200;
const ITEM_RENAMED: u32 = 0x800;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFTypeArrayCallBacks: c_void;
    fn CFStringCreateWithFileSystemRepresentation(allocator: CFTypeRef, path: *const c_char) -> CFTypeRef;
    fn CFArrayCreate(allocator: CFTypeRef, values: *const CFTypeRef, count: isize, callbacks: *const c_void) -> CFTypeRef;
    fn CFRelease(object: CFTypeRef);
}

#[link(name = "CoreServices", kind = "framework")]
unsafe extern "C" {
    fn FSEventStreamCreate(
        allocator: CFTypeRef,
        callback: FSEventStreamCallback,
        context: *const FSEventStreamContext,
        paths: CFTypeRef,
        since: u64,
        latency: f64,
        flags: u32,
    ) -> FSEventStreamRef;
    fn FSEventStreamSetDispatchQueue(stream: FSEventStreamRef, queue: *mut c_void);
    fn FSEventStreamStart(stream: FSEventStreamRef) -> u8;
    fn FSEventStreamStop(stream: FSEventStreamRef);
    fn FSEventStreamInvalidate(stream: FSEventStreamRef);
    fn FSEventStreamRelease(stream: FSEventStreamRef);
}

unsafe extern "C" {
    fn dispatch_queue_create(label: *const c_char, attributes: *const c_void) -> *mut c_void;
    fn dispatch_release(object: *mut c_void);
}

#[cfg(test)]
mod tests {
    use super::FileEvents;
    use std::{
        future::Future,
        path::PathBuf,
        pin::pin,
        sync::Arc,
        task::{Context, Poll, Wake, Waker},
        time::{Duration, Instant},
    };

    struct Unpark(std::thread::Thread);
    impl Wake for Unpark {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }

    /// Whether a change is reported within `limit`.
    fn heard(events: &mut FileEvents, limit: Duration) -> bool {
        let waker = Waker::from(Arc::new(Unpark(std::thread::current())));
        let mut cx = Context::from_waker(&waker);
        let mut changed = pin!(events.changed());
        let until = Instant::now() + limit;
        loop {
            if let Poll::Ready(more) = changed.as_mut().poll(&mut cx) {
                return more;
            }
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            std::thread::park_timeout(left);
        }
    }

    fn quiet(events: &mut FileEvents) {
        while heard(events, Duration::from_millis(200)) {}
    }

    const SOON: Duration = Duration::from_secs(5);
    const QUIET: Duration = Duration::from_millis(300);

    /// Under the temporary folder, which is behind a link (`/var` is
    /// `/private/var`): watched as named, heard as resolved.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zeron-zgx-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn writes_new_files_and_folders_are_heard_and_quiet_is_quiet() {
        let dir = scratch("templates");
        let screens = dir.join("screens");
        std::fs::create_dir_all(&screens).unwrap();
        let file = screens.join("home.luau");
        std::fs::write(&file, "return 1").unwrap();
        let mut events = FileEvents::new().unwrap();
        events.watch(vec![file.clone()], vec![dir.clone(), screens.clone()]);
        quiet(&mut events);
        // Nothing changes: nothing is heard.
        assert!(!heard(&mut events, QUIET));
        // Written in place.
        std::fs::write(&file, "return 2").unwrap();
        assert!(heard(&mut events, SOON));
        quiet(&mut events);
        // A new file beside it, and a new folder.
        std::fs::write(screens.join("chat.luau"), "return 3").unwrap();
        assert!(heard(&mut events, SOON));
        quiet(&mut events);
        std::fs::create_dir(screens.join("shell")).unwrap();
        assert!(heard(&mut events, SOON));
        quiet(&mut events);
        // Removed.
        std::fs::remove_file(screens.join("chat.luau")).unwrap();
        assert!(heard(&mut events, SOON));
        quiet(&mut events);
        // A poke wakes it as a change does.
        events.poke().send(());
        assert!(heard(&mut events, SOON));
        drop(events);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_saved_by_replacing_it_is_still_heard_when_next_written_in_place() {
        let dir = scratch("replaced");
        let file = dir.join("sidebar.luau");
        std::fs::write(&file, "return 1").unwrap();
        let mut events = FileEvents::new().unwrap();
        events.watch(vec![file.clone()], vec![dir.clone()]);
        quiet(&mut events);
        // Saved as an editor does: written beside it, renamed over it.
        let aside = dir.join(".sidebar.luau.tmp");
        std::fs::write(&aside, "return 2").unwrap();
        std::fs::rename(&aside, &file).unwrap();
        assert!(heard(&mut events, SOON));
        quiet(&mut events);
        // The loop watches again (the same paths) and looks.
        events.watch(vec![file.clone()], vec![dir.clone()]);
        quiet(&mut events);
        // Then written in place: the new file is the one watched.
        std::fs::write(&file, "return 3").unwrap();
        assert!(heard(&mut events, SOON), "a write to the replaced file went unheard");
        drop(events);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn what_isn_t_watched_under_a_watched_folder_wakes_nothing() {
        let dir = scratch("unrelated");
        let (screens, art) = (dir.join("screens"), dir.join("art"));
        std::fs::create_dir_all(&screens).unwrap();
        std::fs::create_dir_all(art.join("deep")).unwrap();
        let file = screens.join("home.luau");
        std::fs::write(&file, "return 1").unwrap();
        std::fs::write(screens.join("notes.txt"), "beside").unwrap();
        std::fs::write(art.join("deep").join("icon.svg"), "<svg/>").unwrap();
        let mut events = FileEvents::new().unwrap();
        // The file and its folder; `art` is under the stream's folder too,
        // and not watched.
        events.watch(vec![file.clone()], vec![screens.clone()]);
        quiet(&mut events);
        // The Finder's file in a watched folder.
        std::fs::write(screens.join(".DS_Store"), "finder").unwrap();
        // A file beside it, written in place.
        std::fs::write(screens.join("notes.txt"), "changed").unwrap();
        // What's under a folder that isn't watched.
        std::fs::write(art.join("deep").join("icon.svg"), "<svg></svg>").unwrap();
        std::fs::write(art.join("deep").join("new.svg"), "<svg/>").unwrap();
        assert!(!heard(&mut events, QUIET));
        std::fs::write(&file, "return 2").unwrap();
        assert!(heard(&mut events, SOON));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_folder_reached_by_a_link_is_heard() {
        let dir = scratch("links");
        let real = dir.join("real");
        std::fs::create_dir(&real).unwrap();
        std::os::unix::fs::symlink(&real, dir.join("linked")).unwrap();
        let file = dir.join("linked").join("home.luau");
        std::fs::write(&file, "return 1").unwrap();
        let mut events = FileEvents::new().unwrap();
        events.watch(vec![file.clone()], vec![dir.join("linked")]);
        quiet(&mut events);
        std::fs::write(real.join("home.luau"), "return 2").unwrap();
        assert!(heard(&mut events, SOON));
        quiet(&mut events);
        std::fs::write(real.join("other.luau"), "").unwrap();
        assert!(heard(&mut events, SOON));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Hundreds of files, and not a descriptor for each.
    #[test]
    fn watched_files_hold_nothing_open() {
        let dir = scratch("many");
        let open = || std::fs::read_dir("/dev/fd").unwrap().count();
        let mut dirs = vec![dir.clone()];
        let files: Vec<PathBuf> = (0..1000)
            .map(|i| {
                let sub = dir.join(format!("d{}", i % 40));
                if std::fs::create_dir(&sub).is_ok() {
                    dirs.push(sub.clone());
                }
                let file = sub.join(format!("{i}.luau"));
                std::fs::write(&file, "").unwrap();
                file
            })
            .collect();
        let before = open();
        let mut events = FileEvents::new().unwrap();
        events.watch(files.clone(), dirs);
        // (Other tests open files meanwhile: a descriptor a file would be a
        // thousand.)
        assert!(open() < before + 100, "{} open for {} files", open().saturating_sub(before), files.len());
        quiet(&mut events);
        std::fs::write(&files[567], "changed").unwrap();
        assert!(heard(&mut events, SOON));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
