//! Seeing through a terminal's foreground process: an app keeping shells in
//! a session server attaches each terminal with a small client, so the
//! client is in front. The app says what really is ([`resolve_with`]), and
//! terminals name themselves (and ask before closing) by that instead, as
//! the desktop host's `resolve_terminal_processes` does for Ghostty's.
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::{
        Mutex, Weak,
        atomic::{AtomicU64, Ordering},
    },
};

/// The program in front of a session, as the app's resolver sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Foreground {
    /// As it presents itself (`claude`, `vim`, `zsh`).
    pub name: String,
    /// Its process ID.
    pub pid: u32,
    /// At rest: the session's shell itself is in front, nothing running in
    /// it that closing would end.
    pub idle: bool,
    /// What the program reports it is doing (OSC 7501), when it reports.
    pub status: Option<zgx::terminal::ProgramStatus>,
}

type Resolver = Rc<dyn Fn(u32) -> Option<Foreground>>;
thread_local! {
    static RESOLVER: RefCell<Option<Resolver>> = const { RefCell::new(None) };
    static PUSHED: Cell<bool> = const { Cell::new(false) };
}
/// Bumped each time the app says what is in front may have changed.
static CHANGES: AtomicU64 = AtomicU64::new(0);
type Wake = Weak<Mutex<Option<zgx::task::LatestSender<()>>>>;
/// Every session's wake, to ask each again.
static WAKES: Mutex<Vec<Wake>> = Mutex::new(Vec::new());

/// Install how to see through a terminal's foreground process (given its
/// pid): `Some` replaces what the terminal reports, `None` keeps it. Called
/// on the UI thread when output arrives (once it settles) and, unless the
/// app says when it changes ([`set_pushed`]), twice a second for terminals
/// it resolves.
pub fn resolve_with(resolver: impl Fn(u32) -> Option<Foreground> + 'static) {
    RESOLVER.with(|slot| *slot.borrow_mut() = Some(Rc::new(resolver)));
}

/// Whether the app hears every change to what is in front of the terminals
/// it resolves and says so ([`changed`]): those are not polled then.
pub fn set_pushed(pushed: bool) {
    if PUSHED.replace(pushed) && !pushed {
        changed();
    }
}

/// What is in front of the terminals may have changed: each asks the
/// resolver again. From any thread.
pub fn changed() {
    CHANGES.fetch_add(1, Ordering::AcqRel);
    let mut wakes = WAKES.lock().unwrap();
    wakes.retain(|wake| match wake.upgrade() {
        Some(wake) => {
            if let Some(sender) = &*wake.lock().unwrap() {
                sender.send(());
            }
            true
        }
        None => false,
    });
}

pub(crate) fn register(wake: Wake) {
    let mut wakes = WAKES.lock().unwrap();
    // Terminals closed since go (an app opening and closing many keeps none).
    wakes.retain(|wake| wake.strong_count() > 0);
    wakes.push(wake);
}
pub(crate) fn changes() -> u64 {
    CHANGES.load(Ordering::Acquire)
}
pub(crate) fn pushed() -> bool {
    PUSHED.get()
}
pub(crate) fn resolve(pid: u32) -> Option<Foreground> {
    let resolver = RESOLVER.with(|slot| slot.borrow().clone())?;
    resolver(pid)
}
