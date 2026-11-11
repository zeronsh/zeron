//! Changes under the templates folder, from directory change notifications
//! (Windows): the app waits for one with nothing running meanwhile, as it
//! does on macOS (file_events.rs, FSEvents). Windows once looked twice a
//! second instead, and each look's second half (`publish_page`: every script
//! walked, read and scanned) ran on the UI thread, about 6% of a core at
//! idle, minimized or not.
//!
//! One thread per watched root reads `ReadDirectoryChangesW` over its tree;
//! an event names the entry, so as on macOS only what's watched counts: a
//! watched file written, replaced or removed, a watched folder made or
//! removed, an entry of one made, removed or renamed, or a folder made on
//! the way to one not made yet. A file written beside them (the app
//! composing `.live`, a log) wakes nothing.
//!
//! Nothing is let go of silently:
//! - each read is in place before `watch` returns (overlapped I/O, the first
//!   read issued by `Reader::start`), so a change right after a look is
//!   heard;
//! - lost events (the buffer overflowed: no bytes, or `ERROR_NOTIFY_ENUM_DIR`)
//!   wake the loop, which looks at everything;
//! - a reader that stops by itself (its root removed, a file system that
//!   doesn't notify, a dropped share) wakes the loop and is started again at
//!   the next `watch`, its roots the same or not;
//! - a root that can't be read, or a reader that stopped, leaves the watch
//!   `degraded`: the loop then looks on its timer too (`changed_or_every`),
//!   as it did before there were events.
//!
//! A reader owns its directory handle: it's closed by the reader's thread,
//! after the read is cancelled and done, and the reader's drop waits for that.
//! A folder behind a junction or symbolic link is read as a root of its own
//! (a tree read doesn't follow them). A root not made yet is read from the
//! nearest folder that is, by its own entries.
use std::{
    collections::HashSet,
    os::windows::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    task::Poll,
    time::Duration,
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, ERROR_NOTIFY_ENUM_DIR, GetLastError, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0},
    Storage::FileSystem::{
        CreateFileW, FILE_ACTION_ADDED, FILE_ACTION_REMOVED, FILE_ACTION_RENAMED_NEW_NAME,
        FILE_ACTION_RENAMED_OLD_NAME, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OVERLAPPED, FILE_LIST_DIRECTORY, FILE_NOTIFY_CHANGE_DIR_NAME,
        FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, ReadDirectoryChangesW,
    },
    System::{
        IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED},
        Threading::{CreateEventW, INFINITE, ResetEvent, SetEvent, WaitForMultipleObjects},
    },
};

pub struct FileEvents {
    shared: Arc<Shared>,
    /// One reader per root, and the roots they read.
    readers: Vec<Reader>,
    roots: Vec<(PathBuf, bool)>,
    /// A root couldn't be read at the last `watch`.
    unread: bool,
    tell: zgx::task::LatestSender<()>,
    changed: zgx::task::LatestReceiver<()>,
}

/// What the readers match their events against, and whom they tell.
struct Shared {
    wanted: Mutex<Wanted>,
    tell: zgx::task::LatestSender<()>,
}

/// The paths a change to which is heard, spelt for comparing (absolute,
/// `\` separators, lower case: Windows' names don't differ by case).
#[derive(Default)]
struct Wanted {
    files: HashSet<String>,
    dirs: HashSet<String>,
}

impl FileEvents {
    /// Nothing watched yet.
    pub fn new() -> Option<Self> {
        let (tell, changed) = zgx::task::latest::<()>();
        Some(Self {
            shared: Arc::new(Shared { wanted: Mutex::default(), tell: tell.clone() }),
            readers: Vec::new(),
            roots: Vec::new(),
            unread: false,
            tell,
            changed,
        })
    }

    /// Watch `files` (written, replaced, removed) and `dirs` (an entry made,
    /// removed or renamed) from now on, instead of what was watched before.
    /// The readers start again if the roots they read changed, or one of them
    /// stopped or couldn't start.
    pub fn watch(&mut self, files: Vec<PathBuf>, dirs: Vec<PathBuf>) {
        let wanted = Wanted {
            files: files.iter().map(|f| key(f)).collect(),
            dirs: dirs.iter().map(|d| key(d)).collect(),
        };
        // Matched against before a new reader can hear anything.
        *self.shared.wanted.lock().unwrap_or_else(PoisonError::into_inner) = wanted;
        let folders = files.iter().filter_map(|f| f.parent().map(Path::to_path_buf)).chain(dirs);
        let roots = roots(folders.map(|f| reachable(&absolute(&f))).collect());
        if roots != self.roots || self.degraded() {
            // The new readers' first reads are in place before the old ones
            // stop: a change between the two is heard by one of them.
            let mut readers = Vec::with_capacity(roots.len());
            let mut unread = false;
            for (root, tree) in &roots {
                match Reader::start(root, *tree, &self.shared) {
                    Some(reader) => readers.push(reader),
                    None => unread = true,
                }
            }
            self.readers = readers;
            self.roots = roots;
            self.unread = unread;
        }
    }

    /// Whether something watched may change unheard: a root couldn't be
    /// read, or a reader stopped. The loop looks on a timer too meanwhile.
    pub fn degraded(&self) -> bool {
        self.unread || self.readers.iter().any(Reader::stopped)
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

    /// `changed`, or `every` gone by while the watch is `degraded` (what it
    /// may not hear, looked at as often as before there were events).
    pub async fn changed_or_every(&mut self, every: Duration) -> bool {
        if !self.degraded() {
            return self.changed().await;
        }
        let mut changed = std::pin::pin!(self.changed.recv());
        let mut timer = std::pin::pin!(zgx::timer::sleep(every));
        std::future::poll_fn(|cx| {
            if let Poll::Ready(more) = changed.as_mut().poll(cx) {
                return Poll::Ready(more.is_some());
            }
            if timer.as_mut().poll(cx).is_ready() {
                return Poll::Ready(true);
            }
            Poll::Pending
        })
        .await
    }
}

fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

/// `path` as compared: absolute, `\` separators, no trailing one, lower case.
fn key(path: &Path) -> String {
    let text = absolute(path).to_string_lossy().replace('/', "\\");
    text.strip_prefix(r"\\?\").unwrap_or(&text).trim_end_matches('\\').to_lowercase()
}

/// Whether `path` is under `root` (and isn't it), by their keys.
fn under(path: &Path, root: &Path) -> bool {
    key(path).starts_with(&format!("{}\\", key(root)))
}

/// The folder to read for `folder`: itself and its whole tree if it's there;
/// else the nearest one that is, for its own entries only (the next folder
/// on the way made is heard, and the loop reads from there).
fn reachable(folder: &Path) -> (PathBuf, bool) {
    if folder.is_dir() {
        return (folder.to_path_buf(), true);
    }
    let mut at = folder;
    while let Some(parent) = at.parent() {
        if parent.is_dir() {
            return (parent.to_path_buf(), false);
        }
        at = parent;
    }
    (folder.to_path_buf(), false)
}

/// Whether `path` is a junction or a symbolic link (a reparse point a tree
/// read doesn't follow).
fn is_link(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0)
}

/// Whether reaching `folder` from `root`, a folder above it, goes through a
/// link: a tree read of `root` wouldn't hear it.
fn through_link(root: &Path, folder: &Path) -> bool {
    let root = key(root);
    let mut at = Some(folder);
    while let Some(dir) = at {
        if key(dir) == root {
            return false;
        }
        if is_link(dir) {
            return true;
        }
        at = dir.parent();
    }
    false
}

/// The fewest roots that hear `folders`: a tree read holds every folder under
/// it but those reached through a link, which are roots of their own.
fn roots(mut folders: Vec<(PathBuf, bool)>) -> Vec<(PathBuf, bool)> {
    folders.sort_by_key(|(f, tree)| (key(f), !tree));
    folders.dedup_by(|b, a| key(&a.0) == key(&b.0));
    let mut kept: Vec<(PathBuf, bool)> = Vec::new();
    for (folder, tree) in folders {
        let heard = kept.iter().any(|(k, k_tree)| *k_tree && under(&folder, k) && !through_link(k, &folder));
        if !heard {
            kept.push((folder, tree));
        }
    }
    kept
}

impl Wanted {
    /// Whether `action` on `path` (a full path) is a change to what is
    /// watched: a watched file's (any action), a watched folder made, removed
    /// or renamed, an entry of one made, removed or renamed (a file only
    /// written beside watched ones isn't: the folder doesn't change), or a
    /// folder made on the way to something watched that isn't there yet.
    fn concerns(&self, path: &Path, action: u32) -> bool {
        let entry = matches!(
            action,
            FILE_ACTION_ADDED | FILE_ACTION_REMOVED | FILE_ACTION_RENAMED_OLD_NAME | FILE_ACTION_RENAMED_NEW_NAME
        );
        let at = key(path);
        if self.files.contains(&at) {
            return true;
        }
        if self.dirs.contains(&at) {
            return entry;
        }
        if entry && path.parent().is_some_and(|dir| self.dirs.contains(&key(dir))) {
            return true;
        }
        if matches!(action, FILE_ACTION_ADDED | FILE_ACTION_RENAMED_NEW_NAME) {
            let inside = format!("{at}\\");
            return self.dirs.iter().chain(&self.files).any(|watched| watched.starts_with(&inside));
        }
        false
    }
}

/// A thread reading one root's changes until dropped (or until it can't).
struct Reader {
    /// Set to stop the thread (an event handle, kept as its value: a HANDLE
    /// is not Send).
    stop: usize,
    /// The thread stopped by itself: its root is unheard.
    stopped: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

const BUFFER: usize = 64 * 1024;

/// One reader's overlapped read: its buffer (u32s, as the records are
/// DWORD-aligned) and `OVERLAPPED`, boxed so their addresses hold while the
/// read is pending.
struct Io {
    handle: HANDLE,
    done: HANDLE,
    tree: bool,
    buffer: Vec<u32>,
    overlapped: OVERLAPPED,
}
// The thread is its one user once it starts.
unsafe impl Send for Io {}

impl Io {
    /// Issue the next read; false if it couldn't be.
    fn issue(&mut self) -> bool {
        let filter = FILE_NOTIFY_CHANGE_FILE_NAME
            | FILE_NOTIFY_CHANGE_DIR_NAME
            | FILE_NOTIFY_CHANGE_LAST_WRITE
            | FILE_NOTIFY_CHANGE_SIZE;
        // SAFETY: our event, handle, buffer and OVERLAPPED; the buffer and
        // OVERLAPPED stay where they are (boxed) until the read is done.
        unsafe {
            ResetEvent(self.done);
            self.overlapped = std::mem::zeroed();
            self.overlapped.hEvent = self.done;
            ReadDirectoryChangesW(
                self.handle,
                self.buffer.as_mut_ptr().cast(),
                (self.buffer.len() * 4) as u32,
                self.tree as i32,
                filter,
                std::ptr::null_mut(),
                &mut self.overlapped,
                None,
            ) != 0
        }
    }
}

impl Drop for Io {
    fn drop(&mut self) {
        // SAFETY: the handles this read owns, closed once, after it's done.
        unsafe {
            CloseHandle(self.handle);
            CloseHandle(self.done);
        }
    }
}

impl Reader {
    /// A reader of `root` (its tree with `tree`), its first read in place
    /// when it returns; `None` if it can't be read.
    fn start(root: &Path, tree: bool, shared: &Arc<Shared>) -> Option<Self> {
        let wide: Vec<u16> = root.as_os_str().encode_wide_nul();
        // SAFETY: a NUL-terminated path; the handle is owned by `Io` below.
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                FILE_LIST_DIRECTORY,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED,
                std::ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE || handle.is_null() {
            return None;
        }
        // SAFETY: two unnamed manual-reset events of ours.
        let (done, stop) = unsafe {
            (CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()), CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()))
        };
        if done.is_null() || stop.is_null() {
            // SAFETY: what was made above, closed once.
            unsafe {
                CloseHandle(handle);
                if !done.is_null() {
                    CloseHandle(done);
                }
                if !stop.is_null() {
                    CloseHandle(stop);
                }
            }
            return None;
        }
        // SAFETY: an all-zero OVERLAPPED is its documented initial state.
        let overlapped = unsafe { std::mem::zeroed() };
        let mut io = Box::new(Io { handle, done, tree, buffer: vec![0u32; BUFFER / 4], overlapped });
        if !io.issue() {
            // SAFETY: the stop event, ours; `io` closes the rest.
            unsafe { CloseHandle(stop) };
            return None;
        }
        let stopped = Arc::new(AtomicBool::new(false));
        let (by_itself, shared, root) = (stopped.clone(), shared.clone(), root.to_path_buf());
        let stop_value = stop as usize;
        let spawned = std::thread::Builder::new()
            .name("zeron-zgx templates".into())
            .spawn(move || read(io, stop_value as HANDLE, &root, &shared, &by_itself));
        match spawned {
            Ok(thread) => Some(Self { stop: stop_value, stopped, thread: Some(thread) }),
            Err(_) => {
                // (The closure, and `io` with it, is dropped: its read is
                // cancelled by closing its handle.)
                // SAFETY: the stop event, ours.
                unsafe { CloseHandle(stop) };
                None
            }
        }
    }

    fn stopped(&self) -> bool {
        self.stopped.load(Ordering::Acquire)
    }
}

/// The reader's thread: each read's records matched and told, until `stop`
/// is set or a read fails (then `stopped`, and the loop told).
fn read(mut io: Box<Io>, stop: HANDLE, root: &Path, shared: &Shared, stopped: &AtomicBool) {
    loop {
        let waits = [io.done, stop];
        // SAFETY: our two events, alive across the wait.
        let woke = unsafe { WaitForMultipleObjects(2, waits.as_ptr(), 0, INFINITE) };
        if woke != WAIT_OBJECT_0 {
            // Stopped (or the wait failed): the read cancelled and let finish
            // before its buffer goes.
            let mut read = 0u32;
            // SAFETY: our handle and its pending read's OVERLAPPED.
            unsafe {
                CancelIoEx(io.handle, &io.overlapped);
                GetOverlappedResult(io.handle, &io.overlapped, &mut read, 1);
            }
            return;
        }
        let mut read = 0u32;
        // SAFETY: the read just signalled done; its OVERLAPPED is ours.
        let ok = unsafe { GetOverlappedResult(io.handle, &io.overlapped, &mut read, 0) } != 0;
        let concerned = if ok {
            // No bytes: the changes overflowed the buffer. Look at everything.
            read == 0 || {
                let wanted = shared.wanted.lock().unwrap_or_else(PoisonError::into_inner);
                records(&io.buffer, read as usize).any(|(rel, action)| wanted.concerns(&root.join(rel), action))
            }
        } else {
            // SAFETY: reads this thread's last error.
            let error = unsafe { GetLastError() };
            if error != ERROR_NOTIFY_ENUM_DIR {
                // The root removed, a file system that doesn't notify, a share
                // gone: unheard from now on. The loop looks, and starts it again.
                stopped.store(true, Ordering::Release);
                shared.tell.send(());
                return;
            }
            // Too many changes to list: look at everything.
            true
        };
        if concerned {
            shared.tell.send(());
        }
        if !io.issue() {
            stopped.store(true, Ordering::Release);
            shared.tell.send(());
            return;
        }
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        // SAFETY: the stop event, alive until the thread has seen it.
        unsafe { SetEvent(self.stop as HANDLE) };
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        // SAFETY: the stop event, closed once, after the thread is done.
        unsafe { CloseHandle(self.stop as HANDLE) };
    }
}

/// The records in the first `len` bytes of `buffer`: each one's name
/// (relative to the root) and action. The last record isn't padded to the
/// struct's size (a one-letter name ends 2 bytes short of it): what's
/// checked is the fixed fields and the name, read from the bytes.
fn records(buffer: &[u32], len: usize) -> impl Iterator<Item = (PathBuf, u32)> + '_ {
    // SAFETY: the buffer's own bytes, as many as it holds.
    let bytes: &[u8] = unsafe { std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), buffer.len() * 4) };
    let bytes = &bytes[..len.min(bytes.len())];
    let field = |at: usize| -> Option<u32> { Some(u32::from_ne_bytes(bytes.get(at..at + 4)?.try_into().ok()?)) };
    // NextEntryOffset, Action, FileNameLength, then FileName.
    const NAME: usize = 12;
    let mut at = Some(0usize);
    std::iter::from_fn(move || {
        let offset = at?;
        let (next, action, name_len) = (field(offset)?, field(offset + 4)?, field(offset + 8)? as usize);
        let name = bytes.get(offset + NAME..offset + NAME + name_len)?;
        let units: Vec<u16> = name.chunks_exact(2).map(|pair| u16::from_ne_bytes([pair[0], pair[1]])).collect();
        at = (next != 0).then(|| offset + next as usize);
        Some((PathBuf::from(String::from_utf16_lossy(&units)), action))
    })
}

trait WideNul {
    fn encode_wide_nul(&self) -> Vec<u16>;
}
impl WideNul for std::ffi::OsStr {
    fn encode_wide_nul(&self) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        self.encode_wide().chain(std::iter::once(0)).collect()
    }
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
        assert!(!events.degraded());
        quiet(&mut events);
        assert!(!heard(&mut events, QUIET), "nothing changed, nothing heard");
        std::fs::write(&file, "return 2").unwrap();
        assert!(heard(&mut events, SOON), "written in place");
        quiet(&mut events);
        std::fs::write(screens.join("chat.luau"), "return 3").unwrap();
        assert!(heard(&mut events, SOON), "a new file beside it");
        quiet(&mut events);
        std::fs::create_dir(screens.join("shell")).unwrap();
        assert!(heard(&mut events, SOON), "a new folder");
        quiet(&mut events);
        std::fs::remove_file(screens.join("chat.luau")).unwrap();
        assert!(heard(&mut events, SOON), "removed");
        quiet(&mut events);
        events.poke().send(());
        assert!(heard(&mut events, SOON), "a poke");
        drop(events);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The last record of a read isn't padded: a one-letter name is still
    /// heard.
    #[test]
    fn a_one_letter_name_is_heard() {
        let dir = scratch("letter");
        let mut events = FileEvents::new().unwrap();
        events.watch(vec![], vec![dir.clone()]);
        quiet(&mut events);
        std::fs::write(dir.join("a"), "").unwrap();
        assert!(heard(&mut events, SOON), "a file named `a`");
        quiet(&mut events);
        std::fs::create_dir(dir.join("b")).unwrap();
        assert!(heard(&mut events, SOON), "a folder named `b`");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_saved_by_replacing_it_is_heard() {
        let dir = scratch("replaced");
        let file = dir.join("sidebar.luau");
        std::fs::write(&file, "return 1").unwrap();
        let mut events = FileEvents::new().unwrap();
        events.watch(vec![file.clone()], vec![dir.clone()]);
        quiet(&mut events);
        let aside = dir.join(".sidebar.luau.tmp");
        std::fs::write(&aside, "return 2").unwrap();
        std::fs::rename(&aside, &file).unwrap();
        assert!(heard(&mut events, SOON));
        quiet(&mut events);
        events.watch(vec![file.clone()], vec![dir.clone()]);
        quiet(&mut events);
        std::fs::write(&file, "return 3").unwrap();
        assert!(heard(&mut events, SOON), "a write to the replaced file went unheard");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// What the app writes itself (`.live`, under the templates folder) and
    /// files written beside watched ones wake nothing: the idle window stays
    /// asleep.
    #[test]
    fn what_isn_t_watched_under_a_watched_folder_wakes_nothing() {
        let dir = scratch("unrelated");
        let (screens, live) = (dir.join("screens"), dir.join(".live"));
        std::fs::create_dir_all(&screens).unwrap();
        std::fs::create_dir_all(live.join("deep")).unwrap();
        let file = screens.join("home.luau");
        std::fs::write(&file, "return 1").unwrap();
        std::fs::write(screens.join("notes.txt"), "beside").unwrap();
        std::fs::write(live.join("deep").join("app.luau"), "return 1").unwrap();
        let mut events = FileEvents::new().unwrap();
        events.watch(vec![file.clone()], vec![dir.clone(), screens.clone()]);
        quiet(&mut events);
        std::fs::write(screens.join("notes.txt"), "changed").unwrap();
        std::fs::write(live.join("deep").join("app.luau"), "return 2").unwrap();
        std::fs::write(live.join("deep").join("new.luau"), "return 3").unwrap();
        assert!(!heard(&mut events, QUIET), "a write beside, or under an unwatched folder, woke it");
        std::fs::write(&file, "return 2").unwrap();
        assert!(heard(&mut events, SOON));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The templates folder before it's made, two levels down: each folder
    /// on the way is heard, and so is the folder itself.
    #[test]
    fn a_folder_not_made_yet_is_heard_once_it_is() {
        let dir = scratch("later");
        let templates = dir.join("data").join("zeron").join("templates");
        let watch = |events: &mut FileEvents| events.watch(vec![templates.join(".active")], vec![templates.clone()]);
        let mut events = FileEvents::new().unwrap();
        watch(&mut events);
        quiet(&mut events);
        std::fs::write(dir.join("engine.log"), "busy").unwrap();
        assert!(!heard(&mut events, QUIET), "a file beside the missing folders woke it");
        for made in [dir.join("data"), dir.join("data").join("zeron"), templates.clone()] {
            std::fs::create_dir(&made).unwrap();
            assert!(heard(&mut events, SOON), "{} made", made.display());
            // Watched again (as the loop does), from the folder now there.
            watch(&mut events);
            quiet(&mut events);
        }
        std::fs::write(templates.join(".active"), "mine").unwrap();
        assert!(heard(&mut events, SOON), "its .active written");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A watched folder removed and made again before the loop next watches
    /// (the roots the same as before): its reader stopped, the loop is told,
    /// and the next watch starts it again.
    #[test]
    fn a_root_removed_and_made_again_is_read_again() {
        let dir = scratch("again");
        let templates = dir.join("templates");
        std::fs::create_dir(&templates).unwrap();
        let file = templates.join(".active");
        std::fs::write(&file, "one").unwrap();
        let mut events = FileEvents::new().unwrap();
        events.watch(vec![file.clone()], vec![templates.clone()]);
        quiet(&mut events);
        std::fs::remove_dir_all(&templates).unwrap();
        std::fs::create_dir(&templates).unwrap();
        assert!(heard(&mut events, SOON), "the removal");
        quiet(&mut events);
        events.watch(vec![file.clone()], vec![templates.clone()]);
        assert!(!events.degraded(), "read again");
        quiet(&mut events);
        std::fs::write(&file, "two").unwrap();
        assert!(heard(&mut events, SOON), "a write in the folder made again");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A template folder that's a junction to a checkout elsewhere (a tree
    /// read doesn't follow it): read as a root of its own.
    #[test]
    fn a_folder_reached_through_a_junction_is_heard() {
        let dir = scratch("junction");
        let (templates, checkout) = (dir.join("templates"), dir.join("checkout"));
        std::fs::create_dir_all(&templates).unwrap();
        std::fs::create_dir_all(checkout.join("screens")).unwrap();
        let linked = templates.join("mine");
        let made = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&linked)
            .arg(&checkout)
            .output()
            .unwrap();
        assert!(made.status.success(), "mklink /J: {}", String::from_utf8_lossy(&made.stdout));
        let file = linked.join("screens").join("home.luau");
        std::fs::write(&file, "return 1").unwrap();
        let mut events = FileEvents::new().unwrap();
        events.watch(vec![templates.join(".active"), file.clone()], vec![templates.clone(), linked.clone(), linked.join("screens")]);
        quiet(&mut events);
        std::fs::write(checkout.join("screens").join("home.luau"), "return 2").unwrap();
        assert!(heard(&mut events, SOON), "a write behind the junction");
        quiet(&mut events);
        std::fs::write(checkout.join("screens").join("chat.luau"), "").unwrap();
        assert!(heard(&mut events, SOON), "a new file behind the junction");
        drop(events);
        let _ = std::fs::remove_dir(&linked);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Readers replaced by a watch of other folders: the new ones hear, and
    /// the old ones are gone (dropping them waits for their threads).
    #[test]
    fn a_watch_of_other_folders_replaces_its_readers() {
        let (first, second) = (scratch("first"), scratch("second"));
        let mut events = FileEvents::new().unwrap();
        events.watch(vec![], vec![first.clone()]);
        quiet(&mut events);
        events.watch(vec![], vec![second.clone()]);
        quiet(&mut events);
        std::fs::write(first.join("old.luau"), "").unwrap();
        assert!(!heard(&mut events, QUIET), "the folder no longer watched");
        std::fs::write(second.join("new.luau"), "").unwrap();
        assert!(heard(&mut events, SOON), "the folder watched now");
        let _ = std::fs::remove_dir_all(&first);
        let _ = std::fs::remove_dir_all(&second);
    }

    /// A root that can't be read (a drive that isn't there) leaves the watch
    /// degraded: the loop looks on its timer too.
    #[test]
    fn a_root_that_can_t_be_read_is_degraded() {
        let missing = ('D'..='Z').rev().map(|d| PathBuf::from(format!("{d}:\\"))).find(|p| !p.exists()).expect("a free drive letter");
        let mut events = FileEvents::new().unwrap();
        events.watch(vec![missing.join("templates").join(".active")], vec![missing.join("templates")]);
        assert!(events.degraded());
        let dir = scratch("readable");
        events.watch(vec![], vec![dir.clone()]);
        assert!(!events.degraded(), "readable again");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
