//! The intro's hand-off to setup's window, as what it does with the two
//! windows (`IntroWindows`): main.rs's `intro` drives the host's windows
//! through it, the tests a record of what was asked.

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use zgx::reactive::Signal;
use zgx_desktop::{WindowFactory, WindowHandle};

/// The windows the hand-off works: the intro's own, over the whole display,
/// and setup's, made hidden by `prepare` (if it was) until `reveal`.
pub(crate) trait IntroWindows {
    /// Whether the intro's window is the active one, as zgx last heard (so
    /// until it hears otherwise, as the window opens).
    fn intro_active(&self) -> bool;
    /// Whether setup's window is, once it exists (as for the intro's).
    fn setup_active(&self) -> bool;
    /// Whether setup's window was made (`prepare`).
    fn setup_exists(&self) -> bool;
    fn focus_intro(&self);
    fn show_setup(&self);
    fn focus_setup(&self);
    fn close_setup(&self);
    /// Setup's window made now and shown: `done` with none prepared.
    fn open_setup(&self);
    fn close_intro(&self);
}

pub(crate) struct Handoff<W: IntroWindows> {
    windows: W,
    /// `done` ran: setup's window is the app's now.
    handed_over: Cell<bool>,
}

impl<W: IntroWindows> Handoff<W> {
    pub(crate) fn new(windows: W) -> Self {
        Self { windows, handed_over: Cell::new(false) }
    }

    /// `interactive`: the intro takes the keys for its way in.
    pub(crate) fn interactive(&self) {
        self.take_keys();
    }

    /// Setup's window made (`prepare`): the keys stay with the intro until
    /// it hands over.
    pub(crate) fn prepared(&self) {
        self.take_keys();
    }

    fn take_keys(&self) {
        self.windows.focus_intro();
    }

    /// `reveal`: setup's window shows, under the scene.
    pub(crate) fn reveal(&self) {
        if self.windows.setup_exists() {
            self.windows.show_setup();
        }
    }

    /// `done`: setup's window takes over, and the intro's closes.
    pub(crate) fn done(&self) {
        self.handed_over.set(true);
        if self.windows.setup_exists() {
            self.windows.show_setup();
            self.windows.focus_setup();
        } else {
            self.windows.open_setup();
        }
        self.windows.close_intro();
    }

    /// The intro's window closed, by `done` or otherwise.
    pub(crate) fn intro_closed(&self) {}
}

/// The host's windows: the intro's (`intro`, with its activity), and
/// setup's once `prepare` made it (`setup`, its activity in `setup_active`
/// as its window builds), else made by `done` through `windows`.
pub(crate) struct HostWindows {
    pub(crate) intro: WindowHandle,
    pub(crate) intro_active: Signal<bool>,
    pub(crate) setup: Rc<RefCell<Option<WindowHandle>>>,
    pub(crate) setup_active: Rc<RefCell<Option<Signal<bool>>>>,
    pub(crate) windows: WindowFactory,
    /// Builds setup's window when `done` makes it (none prepared).
    pub(crate) open_setup: Box<dyn Fn(&WindowFactory)>,
}

impl IntroWindows for HostWindows {
    fn intro_active(&self) -> bool {
        self.intro_active.with_untracked(|active| *active)
    }
    fn setup_active(&self) -> bool {
        self.setup_active.borrow().as_ref().is_some_and(|active| active.with_untracked(|active| *active))
    }
    fn setup_exists(&self) -> bool {
        self.setup.borrow().is_some()
    }
    fn focus_intro(&self) {
        self.intro.request_focus();
    }
    fn show_setup(&self) {
        if let Some(setup) = &*self.setup.borrow() {
            setup.set_visible(true);
        }
    }
    fn focus_setup(&self) {
        if let Some(setup) = &*self.setup.borrow() {
            setup.request_focus();
        }
    }
    fn close_setup(&self) {
        if let Some(setup) = &*self.setup.borrow() {
            setup.close();
        }
    }
    fn open_setup(&self) {
        (self.open_setup)(&self.windows);
    }
    fn close_intro(&self) {
        self.intro.close();
    }
}

