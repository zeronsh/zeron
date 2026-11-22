//! Which plugins are on in this app, for its own Rust: the set `plugins.on`
//! says to the scripts (`Bridge::publish_on`: on in the engine's list, and
//! loaded in the window), told here at the same moment. A part of the app
//! that only runs with a plugin (Orchestrators' streams, src/orchestrators.rs)
//! asks [`on`] and follows [`follow`]: one source for the scripts' hooks and
//! for the Rust behind them.
//!
//! Nothing is on until the window's plugins are known: a fresh install's
//! default, and a window with no plugins bridge (setup's).

use std::{
    cell::{Cell, RefCell},
    collections::BTreeSet,
    rc::Rc,
};

struct Follower {
    id: &'static str,
    /// As it was last told.
    was: Cell<bool>,
    told: Rc<dyn Fn(bool)>,
}

thread_local! {
    static ON: RefCell<BTreeSet<String>> = RefCell::default();
    static FOLLOWERS: RefCell<Vec<Rc<Follower>>> = RefCell::default();
}

/// Whether plugin `id` is on now.
pub fn on(id: &str) -> bool {
    ON.with(|on| on.borrow().contains(id))
}

/// `told(on)` each time plugin `id` turns on or off, from now on (as long as
/// the app runs). Not called for how it is now: ask [`on`].
pub fn follow(id: &'static str, told: impl Fn(bool) + 'static) {
    let was = Cell::new(on(id));
    FOLLOWERS.with(|f| f.borrow_mut().push(Rc::new(Follower { id, was, told: Rc::new(told) })));
}

/// The plugins on are now `ids`: each follower whose plugin turned on or off
/// is told (after the set is in place, so what it calls reads it as it is).
pub(crate) fn set<'a>(ids: impl IntoIterator<Item = &'a str>) {
    let now: BTreeSet<String> = ids.into_iter().map(str::to_string).collect();
    let changed = ON.with(|on| {
        let mut on = on.borrow_mut();
        if *on == now {
            return false;
        }
        *on = now;
        true
    });
    if !changed {
        return;
    }
    // (Taken out first: one told may follow another.)
    let followers: Vec<Rc<Follower>> = FOLLOWERS.with(|f| f.borrow().clone());
    for f in followers {
        let now = on(f.id);
        if f.was.replace(now) != now {
            (f.told)(now);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_follower_hears_its_plugin_turn_on_and_off_and_nothing_else() {
        let heard: Rc<RefCell<Vec<bool>>> = Rc::default();
        let h = heard.clone();
        follow("orchestrators", move |on| h.borrow_mut().push(on));
        assert!(!on("orchestrators"), "off until the window's plugins are known");
        set(["git"]);
        assert!(heard.borrow().is_empty(), "another plugin's switch");
        set(["git", "orchestrators"]);
        assert!(on("orchestrators"));
        set(["orchestrators", "git"]);
        set(["orchestrators"]);
        assert_eq!(*heard.borrow(), vec![true], "told once, as it turned on");
        set([]);
        assert!(!on("orchestrators"));
        assert_eq!(*heard.borrow(), vec![true, false]);
    }
}
