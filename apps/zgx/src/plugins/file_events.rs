//! Changes under the templates folder, from the kernel (kqueue, macOS): the
//! app waits for one with nothing running meanwhile. It once walked the
//! folder twice a second, and the idle window woke for it every time.
//!
//! zgx watches its own scripts this way (zgx-script's `file_events.rs`, not
//! public); this is that, for files the app reads itself. Each file and
//! each directory holding one are watched, so a file an editor replaces (a
//! save as write-and-rename) is seen as well as one written in place, and a
//! folder made or removed is seen in the directory it's in.
use std::{
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

pub struct FileEvents {
    /// The queue, shared with its thread: it closes when both have let go,
    /// so neither uses its number after the other closed it.
    queue: Arc<OwnedFd>,
    /// Set when this goes: its thread lets go of the queue and ends.
    done: Arc<AtomicBool>,
    /// What is watched now, and the descriptors that watch it (closing one
    /// drops what watched it).
    watched: (Vec<PathBuf>, Vec<PathBuf>),
    /// Which file each path was when it was opened (device, inode): a file
    /// saved by replacing it is a new one, and its old descriptor hears
    /// nothing of it.
    identities: Vec<Option<(u64, u64)>>,
    fds: Vec<OwnedFd>,
    tell: zgx::task::LatestSender<()>,
    changed: zgx::task::LatestReceiver<()>,
}

/// Which file `path` is now (its device and inode), or none.
fn identity(path: &std::path::Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).ok().map(|m| (m.dev(), m.ino()))
}

/// Registers `change` on `queue`, reading nothing back.
fn register(queue: &OwnedFd, change: &libc::kevent) {
    // SAFETY: `queue` is open while the caller holds it; one change is read
    // from `change`, and no event is written.
    unsafe {
        libc::kevent(queue.as_raw_fd(), change, 1, std::ptr::null_mut(), 0, std::ptr::null());
    }
}

/// The queue's own event, which wakes its thread as this goes.
fn wake_event(fflags: u32, flags: u16) -> libc::kevent {
    libc::kevent {
        ident: 0,
        filter: libc::EVFILT_USER,
        flags,
        fflags,
        data: 0,
        udata: std::ptr::null_mut(),
    }
}

impl FileEvents {
    /// A queue and the thread that waits on it; `None` if the kernel gives
    /// no queue.
    pub fn new() -> Option<Self> {
        // SAFETY: `kqueue` takes no arguments; it returns a new descriptor,
        // or -1.
        let queue = unsafe { libc::kqueue() };
        if queue < 0 {
            return None;
        }
        // SAFETY: `queue` was just opened, and nothing else owns it.
        let queue = Arc::new(unsafe { OwnedFd::from_raw_fd(queue) });
        register(&queue, &wake_event(0, libc::EV_ADD | libc::EV_CLEAR));
        let (tell, changed) = zgx::task::latest::<()>();
        let done = Arc::new(AtomicBool::new(false));
        let (ended, waiting, told) = (done.clone(), queue.clone(), tell.clone());
        std::thread::Builder::new()
            .name("zeron-zgx templates".into())
            .spawn(move || {
                // SAFETY: `kevent` is plain data, for which zeroes are valid
                // (a null `udata`).
                let mut events: [libc::kevent; 8] = unsafe { std::mem::zeroed() };
                loop {
                    // SAFETY: `waiting` is open while this thread holds it,
                    // and `events` has room for the `events.len()` events
                    // the kernel may write.
                    let n = unsafe {
                        libc::kevent(
                            waiting.as_raw_fd(),
                            std::ptr::null(),
                            0,
                            events.as_mut_ptr(),
                            events.len() as libc::c_int,
                            std::ptr::null(),
                        )
                    };
                    if ended.load(Ordering::Acquire)
                        || (n < 0 && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted)
                    {
                        return;
                    }
                    if n > 0 {
                        told.send(());
                    }
                }
            })
            .ok()?;
        Some(Self { queue, done, watched: Default::default(), identities: Vec::new(), fds: Vec::new(), tell, changed })
    }

    /// Watch `files` (written, replaced, removed) and `dirs` (an entry made,
    /// removed or renamed) from now on, instead of what was watched before.
    /// Watching what is already watched opens nothing again, unless one of
    /// its files was replaced since (an editor's write-and-rename): the
    /// descriptor watching it watches the file that was, so it's opened
    /// again on the one there now.
    pub fn watch(&mut self, files: Vec<PathBuf>, dirs: Vec<PathBuf>) {
        let wanted = (files, dirs);
        let identities: Vec<Option<(u64, u64)>> = wanted.0.iter().chain(wanted.1.iter()).map(|p| identity(p)).collect();
        if wanted == self.watched && identities == self.identities {
            return;
        }
        // Closing a descriptor drops what watched it.
        self.fds.clear();
        let file = libc::NOTE_WRITE | libc::NOTE_EXTEND | libc::NOTE_ATTRIB | libc::NOTE_DELETE | libc::NOTE_RENAME | libc::NOTE_REVOKE;
        let dir = libc::NOTE_WRITE | libc::NOTE_DELETE | libc::NOTE_RENAME | libc::NOTE_REVOKE;
        for (path, notes) in wanted.0.iter().map(|p| (p, file)).chain(wanted.1.iter().map(|d| (d, dir))) {
            let Ok(c_path) = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()) else {
                continue;
            };
            // SAFETY: `c_path` is a NUL-terminated path, read for the call.
            let fd = unsafe { libc::open(c_path.as_ptr(), libc::O_EVTONLY | libc::O_CLOEXEC) };
            if fd < 0 {
                continue;
            }
            // SAFETY: `fd` was just opened, and nothing else owns it.
            let fd = unsafe { OwnedFd::from_raw_fd(fd) };
            register(
                &self.queue,
                &libc::kevent {
                    ident: fd.as_raw_fd() as usize,
                    filter: libc::EVFILT_VNODE,
                    flags: libc::EV_ADD | libc::EV_CLEAR,
                    fflags: notes,
                    data: 0,
                    udata: std::ptr::null_mut(),
                },
            );
            self.fds.push(fd);
        }
        self.watched = wanted;
        self.identities = identities;
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

impl Drop for FileEvents {
    fn drop(&mut self) {
        self.fds.clear();
        self.done.store(true, Ordering::Release);
        register(&self.queue, &wake_event(libc::NOTE_TRIGGER, 0));
    }
}

#[cfg(test)]
mod tests {
    use super::FileEvents;
    use std::{
        future::Future,
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
        while heard(events, Duration::from_millis(100)) {}
    }

    #[test]
    fn writes_new_files_and_folders_are_heard_and_quiet_is_quiet() {
        let dir = std::env::temp_dir().join(format!("zeron-zgx-templates-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let screens = dir.join("screens");
        std::fs::create_dir_all(&screens).unwrap();
        let file = screens.join("home.luau");
        std::fs::write(&file, "return 1").unwrap();
        let mut events = FileEvents::new().unwrap();
        events.watch(vec![file.clone()], vec![dir.clone(), screens.clone()]);
        quiet(&mut events);
        // Nothing changes: nothing is heard.
        assert!(!heard(&mut events, Duration::from_millis(300)));
        // Written in place.
        std::fs::write(&file, "return 2").unwrap();
        assert!(heard(&mut events, Duration::from_secs(5)));
        quiet(&mut events);
        // A new file beside it, and a new folder.
        std::fs::write(screens.join("chat.luau"), "return 3").unwrap();
        assert!(heard(&mut events, Duration::from_secs(5)));
        quiet(&mut events);
        std::fs::create_dir(screens.join("shell")).unwrap();
        assert!(heard(&mut events, Duration::from_secs(5)));
        quiet(&mut events);
        // A poke wakes it as a change does.
        events.poke().send(());
        assert!(heard(&mut events, Duration::from_secs(5)));
        drop(events);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_saved_by_replacing_it_is_still_heard_when_next_written_in_place() {
        let dir = std::env::temp_dir().join(format!("zeron-zgx-replaced-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("sidebar.luau");
        std::fs::write(&file, "return 1").unwrap();
        let mut events = FileEvents::new().unwrap();
        events.watch(vec![file.clone()], vec![dir.clone()]);
        quiet(&mut events);
        // Saved as an editor does: written beside it, renamed over it.
        let aside = dir.join(".sidebar.luau.tmp");
        std::fs::write(&aside, "return 2").unwrap();
        std::fs::rename(&aside, &file).unwrap();
        assert!(heard(&mut events, Duration::from_secs(5)));
        quiet(&mut events);
        // The loop watches again (the same paths) and looks.
        events.watch(vec![file.clone()], vec![dir.clone()]);
        quiet(&mut events);
        // Then written in place: the new file is the one watched.
        std::fs::write(&file, "return 3").unwrap();
        assert!(heard(&mut events, Duration::from_secs(5)), "a write to the replaced file went unheard");
        drop(events);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
