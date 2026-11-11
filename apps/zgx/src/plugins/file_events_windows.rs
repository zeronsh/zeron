//! Changes under the templates folder, from directory change notifications
//! (Windows): the app waits for one with nothing running meanwhile, as it
//! does on macOS (file_events.rs, FSEvents). Windows once looked twice a
//! second instead, and each look's second half (`publish_templates`: every
//! template's files walked and stat'd) ran on the UI thread, about 6% of a
//! core at idle, minimized or not.
//!
//! One thread per watched root reads `ReadDirectoryChangesW` over its whole
//! tree; an event names the entry, so as on macOS only what's watched
//! counts: a watched file written, replaced or removed, a watched folder
//! made or removed, or an entry of one made, removed or renamed. A file
//! written beside them (the app composing `.live`, a log) wakes nothing.
//! A root not made yet is heard from the nearest folder that is, by its
//! own entries only; once made, the loop watches again.
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE},
    Storage::FileSystem::{
        CreateFileW, FILE_ACTION_ADDED, FILE_ACTION_REMOVED, FILE_ACTION_RENAMED_NEW_NAME,
        FILE_ACTION_RENAMED_OLD_NAME, FILE_FLAG_BACKUP_SEMANTICS, FILE_LIST_DIRECTORY,
        FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE,
        FILE_NOTIFY_CHANGE_SIZE, FILE_NOTIFY_INFORMATION, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, OPEN_EXISTING, ReadDirectoryChangesW,
    },
    System::IO::CancelIoEx,
};

pub struct FileEvents {
    shared: Arc<Shared>,
    /// One reader per root, and the roots they read.
    readers: Vec<Reader>,
    roots: Vec<(PathBuf, bool)>,
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
            tell,
            changed,
        })
    }

    /// Watch `files` (written, replaced, removed) and `dirs` (an entry made,
    /// removed or renamed) from now on, instead of what was watched before.
    /// The readers start again only if the roots they read changed.
    pub fn watch(&mut self, files: Vec<PathBuf>, dirs: Vec<PathBuf>) {
        let wanted = Wanted {
            files: files.iter().map(|f| key(f)).collect(),
            dirs: dirs.iter().map(|d| key(d)).collect(),
        };
        // Matched against before a new reader can hear anything.
        *self.shared.wanted.lock().unwrap_or_else(PoisonError::into_inner) = wanted;
        let folders = files.iter().filter_map(|f| f.parent().map(Path::to_path_buf)).chain(dirs);
        let roots = roots(folders.map(|f| reachable(&absolute(&f))).collect());
        if roots != self.roots {
            // The new readers start before the old ones stop: a change between
            // the two is heard by one of them.
            let readers = roots.iter().filter_map(|(root, tree)| Reader::start(root, *tree, &self.shared)).collect();
            self.readers = readers;
            self.roots = roots;
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

fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

/// `path` as compared: absolute, `\` separators, no trailing one, lower case.
fn key(path: &Path) -> String {
    let text = absolute(path).to_string_lossy().replace('/', "\\");
    text.strip_prefix(r"\\?\").unwrap_or(&text).trim_end_matches('\\').to_lowercase()
}

/// The folder to read for `folder`: itself and its whole tree if it's there;
/// else the nearest one that is, for its own entries only (the missing one
/// made is heard; nothing deeper in a big folder wakes anything).
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

/// The fewest roots that hear `folders`: a tree read holds every folder
/// under it.
fn roots(mut folders: Vec<(PathBuf, bool)>) -> Vec<(PathBuf, bool)> {
    folders.sort_by_key(|(f, tree)| (key(f), !tree));
    folders.dedup_by(|b, a| key(&a.0) == key(&b.0));
    let mut kept: Vec<(PathBuf, bool)> = Vec::new();
    for (folder, tree) in folders {
        let under = kept.iter().any(|(k, k_tree)| *k_tree && key(&folder).starts_with(&format!("{}\\", key(k))));
        if !under {
            kept.push((folder, tree));
        }
    }
    kept
}

impl Wanted {
    /// Whether `action` on `path` (a full path) is a change to what is
    /// watched: a watched file's (any action), a watched folder made, removed
    /// or renamed, or an entry of one made, removed or renamed (a file only
    /// written beside watched ones isn't: the folder doesn't change).
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
        entry && path.parent().is_some_and(|dir| self.dirs.contains(&key(dir)))
    }
}

/// A thread reading one root's changes until dropped.
struct Reader {
    stop: Arc<AtomicBool>,
    /// The directory handle (a HANDLE is not Send: kept as its value).
    handle: usize,
}

const BUFFER: usize = 64 * 1024;

impl Reader {
    fn start(root: &Path, tree: bool, shared: &Arc<Shared>) -> Option<Self> {
        let wide: Vec<u16> = root.as_os_str().encode_wide_nul();
        // SAFETY: a NUL-terminated path; the handle is closed by the thread.
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                FILE_LIST_DIRECTORY,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS,
                std::ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE || handle.is_null() {
            return None;
        }
        let stop = Arc::new(AtomicBool::new(false));
        let (stopped, shared, root) = (stop.clone(), shared.clone(), root.to_path_buf());
        let value = handle as usize;
        let spawned = std::thread::Builder::new().name("zeron-zgx templates".into()).spawn(move || {
            let handle = value as HANDLE;
            // u32-aligned, as FILE_NOTIFY_INFORMATION records are.
            let mut buffer = vec![0u32; BUFFER / 4];
            let filter = FILE_NOTIFY_CHANGE_FILE_NAME
                | FILE_NOTIFY_CHANGE_DIR_NAME
                | FILE_NOTIFY_CHANGE_LAST_WRITE
                | FILE_NOTIFY_CHANGE_SIZE;
            while !stopped.load(Ordering::Acquire) {
                let mut read = 0u32;
                // SAFETY: our handle and buffer, alive across the call (which
                // `CancelIoEx` ends early when the reader is dropped).
                let ok = unsafe {
                    ReadDirectoryChangesW(
                        handle,
                        buffer.as_mut_ptr().cast(),
                        BUFFER as u32,
                        tree as i32,
                        filter,
                        &mut read,
                        std::ptr::null_mut(),
                        None,
                    )
                };
                if stopped.load(Ordering::Acquire) || ok == 0 {
                    break;
                }
                // Nothing read: the changes overflowed the buffer. Look again.
                let concerned = read == 0
                    || records(&buffer, read as usize).any(|(rel, action)| {
                        shared.wanted.lock().unwrap_or_else(PoisonError::into_inner).concerns(&root.join(rel), action)
                    });
                if concerned {
                    shared.tell.send(());
                }
            }
            // SAFETY: the handle this thread owns, closed once.
            unsafe { CloseHandle(handle) };
        });
        if spawned.is_err() {
            // SAFETY: no thread took it.
            unsafe { CloseHandle(handle) };
            return None;
        }
        Some(Self { stop, handle: value })
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        // SAFETY: ends the thread's read on the handle it still owns (it
        // closes it only after the read returns).
        unsafe { CancelIoEx(self.handle as HANDLE, std::ptr::null()) };
    }
}

/// The records in the first `len` bytes of `buffer`: each one's name
/// (relative to the root) and action.
fn records(buffer: &[u32], len: usize) -> impl Iterator<Item = (PathBuf, u32)> + '_ {
    let bytes = len.min(buffer.len() * 4);
    let base = buffer.as_ptr().cast::<u8>();
    let mut at = Some(0usize);
    std::iter::from_fn(move || {
        let offset = at?;
        let header = std::mem::size_of::<FILE_NOTIFY_INFORMATION>();
        if offset + header > bytes {
            return None;
        }
        // SAFETY: a record starts at `offset`, inside the bytes read, aligned
        // (the system aligns each to a DWORD).
        let info = unsafe { &*base.add(offset).cast::<FILE_NOTIFY_INFORMATION>() };
        let name_len = info.FileNameLength as usize / 2;
        let name_at = offset + std::mem::offset_of!(FILE_NOTIFY_INFORMATION, FileName);
        if name_at + name_len * 2 > bytes {
            return None;
        }
        // SAFETY: `name_len` UTF-16 units at `name_at`, inside the bytes read.
        let name = unsafe { std::slice::from_raw_parts(base.add(name_at).cast::<u16>(), name_len) };
        let rel = PathBuf::from(String::from_utf16_lossy(name));
        at = (info.NextEntryOffset != 0).then(|| offset + info.NextEntryOffset as usize);
        Some((rel, info.Action))
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

    /// The templates folder before it's made: heard once it is.
    #[test]
    fn a_folder_not_made_yet_is_heard_once_it_is() {
        let dir = scratch("later");
        let templates = dir.join("templates");
        let mut events = FileEvents::new().unwrap();
        events.watch(vec![templates.join(".active")], vec![templates.clone()]);
        quiet(&mut events);
        std::fs::write(dir.join("engine.log"), "busy").unwrap();
        assert!(!heard(&mut events, QUIET), "a file beside the missing folder woke it");
        std::fs::create_dir(&templates).unwrap();
        assert!(heard(&mut events, SOON), "the folder made");
        // Watched again (as the loop does): now its own entries count.
        events.watch(vec![templates.join(".active")], vec![templates.clone()]);
        quiet(&mut events);
        std::fs::write(templates.join(".active"), "mine").unwrap();
        assert!(heard(&mut events, SOON), "its .active written");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
