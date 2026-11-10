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

    /// The intro asks for the keys only while it still has them (or zgx has
    /// heard nothing else): switched away from (alt-tab, Cmd-Tab, a click on
    /// another app), it stays at the back. Asking would pull it in front of
    /// the app switched to, the whole display under it, and on Windows press
    /// Alt for it (winit's way past focus-stealing prevention), which ends
    /// a switch still being made.
    fn take_keys(&self) {
        if self.windows.intro_active() {
            self.windows.focus_intro();
        }
    }

    /// `reveal`: setup's window shows, under the scene.
    pub(crate) fn reveal(&self) {
        if self.windows.setup_exists() {
            self.windows.show_setup();
        }
    }

    /// `done`: setup's window takes over, and the intro's closes. It takes
    /// the keys if the app still has them, in either window (`reveal` gave
    /// them to setup's as it showed): switched away from during the
    /// hand-off, it shows at the back, as `interactive` leaves the intro.
    pub(crate) fn done(&self) {
        self.handed_over.set(true);
        if self.windows.setup_exists() {
            let in_front = self.windows.intro_active() || self.windows.setup_active();
            self.windows.show_setup();
            if in_front {
                self.windows.focus_setup();
            }
        } else {
            self.windows.open_setup();
        }
        self.windows.close_intro();
    }

    /// The intro's window closed, by `done` or otherwise. Otherwise (Alt+F4,
    /// the taskbar's Close window, the window manager's close: it has no
    /// close button of its own) is the app being quit: setup's window, made
    /// hidden at launch, closes with it, and with no window left the app
    /// quits. Left open, it kept the process running with nothing shown.
    pub(crate) fn intro_closed(&self) {
        if !self.handed_over.replace(true) && self.windows.setup_exists() {
            self.windows.close_setup();
        }
    }
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

#[cfg(test)]
mod tests {
    //! The hand-off over a record of what it asked of the windows, fed what
    //! the platform reports as alt-tab and a close do to the intro.

    use super::{Handoff, IntroWindows};
    use std::cell::{Cell, RefCell};

    /// Two windows as the hand-off sees them: their activity as zgx last
    /// heard it (active until told otherwise), and what was asked.
    #[derive(Default)]
    struct Windows {
        intro_away: Cell<bool>,
        setup_away: Cell<bool>,
        setup: Cell<bool>,
        asked: RefCell<Vec<&'static str>>,
    }

    impl Windows {
        fn ask(&self, what: &'static str) {
            self.asked.borrow_mut().push(what);
        }
    }

    impl IntroWindows for &Windows {
        fn intro_active(&self) -> bool {
            !self.intro_away.get()
        }
        fn setup_active(&self) -> bool {
            !self.setup_away.get()
        }
        fn setup_exists(&self) -> bool {
            self.setup.get()
        }
        fn focus_intro(&self) {
            self.ask("focus intro");
        }
        fn show_setup(&self) {
            self.ask("show setup");
        }
        fn focus_setup(&self) {
            self.ask("focus setup");
        }
        fn close_setup(&self) {
            self.ask("close setup");
            self.setup.set(false);
        }
        fn open_setup(&self) {
            self.ask("open setup");
        }
        fn close_intro(&self) {
            self.ask("close intro");
        }
    }

    /// Setup's window made hidden at launch, as `prepare` makes it.
    fn launched(windows: &Windows) -> Handoff<&Windows> {
        let handoff = Handoff::new(windows);
        windows.setup.set(true);
        handoff.prepared();
        windows.asked.borrow_mut().clear();
        handoff
    }

    fn asked(windows: &Windows) -> Vec<&'static str> {
        windows.asked.borrow().clone()
    }

    /// Nobody switched away: the way in takes the keys, as it always did.
    #[test]
    fn the_way_in_takes_the_keys_of_an_intro_still_in_front() {
        let windows = Windows::default();
        let handoff = launched(&windows);
        handoff.interactive();
        assert_eq!(asked(&windows), ["focus intro"]);
    }

    /// Alt-tab (Cmd-Tab) away before the way in comes (Focused(false)): it
    /// comes without pulling the intro back in front of the app switched to.
    #[test]
    fn the_way_in_leaves_an_intro_switched_away_from_at_the_back() {
        let windows = Windows::default();
        let handoff = launched(&windows);
        windows.intro_away.set(true);
        handoff.interactive();
        assert_eq!(asked(&windows), Vec::<&str>::new(), "the intro took focus back from the app switched to");
    }

    /// Away and back (Focused(false), then Focused(true)): in front again,
    /// it takes the keys as before.
    #[test]
    fn the_way_in_takes_the_keys_once_switched_back_to() {
        let windows = Windows::default();
        let handoff = launched(&windows);
        windows.intro_away.set(true);
        windows.intro_away.set(false);
        handoff.interactive();
        assert_eq!(asked(&windows), ["focus intro"]);
    }

    /// The hand-off as it plays: setup's window shows, takes the keys, and
    /// the intro's closes, which leaves setup's window open.
    #[test]
    fn done_hands_over_to_setup_s_window() {
        let windows = Windows::default();
        let handoff = launched(&windows);
        handoff.interactive();
        handoff.reveal();
        handoff.done();
        handoff.intro_closed();
        assert_eq!(asked(&windows), ["focus intro", "show setup", "show setup", "focus setup", "close intro"]);
    }

    /// Switched away during the hand-off (after `reveal`, setup's window has
    /// the keys, then loses them): setup's window shows, at the back.
    #[test]
    fn done_leaves_setup_s_window_at_the_back_when_switched_away_from() {
        let windows = Windows::default();
        let handoff = launched(&windows);
        handoff.reveal();
        windows.intro_away.set(true);
        windows.setup_away.set(true);
        windows.asked.borrow_mut().clear();
        handoff.done();
        assert_eq!(asked(&windows), ["show setup", "close intro"]);
    }

    /// Alt+F4 (Cmd-W, the taskbar's Close window) on the intro: the app
    /// quits, setup's hidden window with it, not left running unseen.
    #[test]
    fn closing_the_intro_closes_setup_s_hidden_window() {
        let windows = Windows::default();
        let handoff = launched(&windows);
        handoff.interactive();
        windows.asked.borrow_mut().clear();
        handoff.intro_closed();
        assert_eq!(asked(&windows), ["close setup"], "setup's hidden window outlived the intro: the process runs on, nothing shown");
    }

    /// Closed before setup's window was made: nothing else to close.
    #[test]
    fn closing_the_intro_before_setup_s_window_closes_nothing_else() {
        let windows = Windows::default();
        let handoff = Handoff::new(&windows);
        handoff.intro_closed();
        assert_eq!(asked(&windows), Vec::<&str>::new());
    }
}
