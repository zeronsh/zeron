//! What you do as yourself in the store, asked in a window of its own once a
//! template that isn't Zeron's has run (`preview::another_s_ran`): publishing
//! a version, taking new installs of one again, and a new deploy token. Any
//! of the main window may be drawn, or wired, by that template from then on,
//! so its word that you asked isn't yours: the host asks here
//! (`ui/store_consent.luau`, on a host with no plugins and no templates, the
//! only one holding `store.decide`), in words it read from the engine.
//!
//! A new token is made by this window and shown in it, once: the main
//! window's host never holds it (`catalog.rs` answers it the token's record
//! alone). Closed before it's answered, nothing is made or sent; closed while
//! a token waits on the sign-in, the wait stops, and one made all the same is
//! revoked; a publish under way goes on, and its answer is the real one.
//!
//! It takes yes only once it has shown a moment (`ARMED`, again each time it
//! comes forward), so a second click or Enter meant for the main window never
//! answers it. After a no, the next of that act (publishing, taking installs
//! again, a new token, whatever its details) is refused for a while, longer
//! each time, and the page is told (`asked_lately`).
//!
//! It guards the buttons, not the machine: a template kept by the person can
//! start a terminal and publish with the CLI. The notice every publish raises
//! is what always stands.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::time::{Duration, Instant};

use serde_json::{Value as Json, json};
use zgx_desktop::{WindowContext, WindowHandle, WindowOptions};
use zgx_script::{ScriptHost, Shared, Value};

use super::consent::{ARMED, plain};
use super::{Bridge, json_value};

/// The window, in points: the consent windows'.
const SIZE: (f64, f64) = (460., 540.);

pub type Answer = Pin<Box<dyn Future<Output = Result<Json, String>>>>;
/// What it does once you say so.
pub type Run = Box<dyn FnOnce() -> Answer>;

/// One thing asked: what the window says (`item`), what it does, and what
/// stops it half-way (a sign-in waited for).
pub struct Act {
    pub item: Json,
    pub run: Run,
    pub stop: Box<dyn Fn()>,
    /// What undoes it when nobody's left to see what it made (a token whose
    /// window has gone), given its answer.
    pub undo: Option<Box<dyn Fn(Json) -> Answer>>,
}

/// After a no, how long the next is refused: this, twice as long each time.
const REFUSED: Duration = Duration::from_secs(30);
const REFUSED_MOST: Duration = Duration::from_secs(600);

/// Of each act (`publish`, `unyank`, `token`): how often the last ones were
/// no, and until when the next is refused.
#[derive(Debug, Default)]
struct Noes(BTreeMap<String, (u32, Instant)>);

impl Noes {
    fn no(&mut self, act: &str, now: Instant) {
        let count = self.0.get(act).map_or(0, |(count, _)| *count);
        let wait = REFUSED.saturating_mul(1 << count.min(8)).min(REFUSED_MOST);
        self.0.insert(act.to_owned(), (count + 1, now + wait));
    }

    fn yes(&mut self, act: &str) {
        self.0.remove(act);
    }

    fn refused(&self, act: &str, now: Instant) -> bool {
        self.0.get(act).is_some_and(|(_, until)| now < *until)
    }
}

/// Where an act stands, whatever draws it.
struct Asked {
    item: Json,
    run: RefCell<Option<Run>>,
    stop: Box<dyn Fn()>,
    undo: Option<Box<dyn Fn(Json) -> Answer>>,
    /// Since when it has shown, as it last came forward.
    since: Cell<Instant>,
    /// How many times it has come forward: the window arms itself again.
    raised: Cell<u32>,
    /// Its answer, told once.
    tell: RefCell<Option<Box<dyn FnOnce(Result<Json, String>)>>>,
    /// Being done.
    busy: Cell<bool>,
    /// The window has gone: nothing is shown any more.
    closed: Cell<bool>,
    /// A new token, once made: here and on the window, nowhere else.
    secret: RefCell<Option<String>>,
}

impl Asked {
    fn tell(&self, answer: Result<Json, String>) {
        if let Some(tell) = self.tell.borrow_mut().take() {
            tell(answer);
        }
    }

    /// Closed: an act not begun isn't done. One under way is told as it ends:
    /// a token waiting on a sign-in stops waiting; a publish goes on.
    fn close(&self) {
        self.closed.set(true);
        self.secret.borrow_mut().take();
        if self.busy.get() {
            return (self.stop)();
        }
        if self.run.borrow_mut().take().is_some() {
            NOES.with(|n| n.borrow_mut().no(self.act(), Instant::now()));
            self.tell(Err("cancelled".into()));
        }
    }

    /// Come forward again: it takes yes only once it has shown a moment.
    fn raise(&self) {
        self.since.set(Instant::now());
        self.raised.set(self.raised.get() + 1);
    }

    fn act(&self) -> &str {
        self.item["act"].as_str().unwrap_or_default()
    }

    fn may_allow(&self, now: Instant) -> bool {
        now.duration_since(self.since.get()) >= ARMED
    }

    /// What you answered. `true` when the window has nothing more to show.
    async fn decide(&self, allow: bool) -> bool {
        if !allow {
            self.close();
            return true;
        }
        if !self.may_allow(Instant::now()) {
            return false;
        }
        let Some(run) = self.run.borrow_mut().take() else { return false };
        NOES.with(|n| n.borrow_mut().yes(self.act()));
        self.busy.set(true);
        let mut answer = run().await;
        self.busy.set(false);
        // (The token stays here; its record alone goes back.)
        let secret = answer.as_mut().ok().and_then(Json::as_object_mut).and_then(|o| o.remove("secret"));
        let secret = secret.as_ref().and_then(Json::as_str).filter(|s| !s.is_empty()).map(str::to_owned);
        // (Made, with its window gone: nobody will ever see it.)
        if self.closed.get()
            && secret.is_some()
            && let Some(undo) = &self.undo
            && let Ok(made) = &answer
            && undo(made.clone()).await.is_ok()
        {
            answer = Err("cancelled".into());
        }
        let shown = secret.filter(|_| !self.closed.get());
        let more = shown.is_some();
        *self.secret.borrow_mut() = shown;
        self.tell(answer);
        !more
    }

    fn record(&self) -> Value {
        let mut fields = vec![
            ("item", json_value(&self.item)),
            ("busy", Value::from(self.busy.get())),
            ("raised", Value::from(f64::from(self.raised.get()))),
        ];
        if let Some(secret) = self.secret.borrow().as_deref() {
            fields.push(("secret", Value::from(secret)));
        }
        Value::record(fields)
    }
}

#[derive(Clone)]
struct Opened {
    window: WindowHandle,
    asked: Rc<Asked>,
    shown: Rc<RefCell<Option<Shared>>>,
}

thread_local! {
    static OPEN: RefCell<Option<Opened>> = const { RefCell::new(None) };
    static NOES: RefCell<Noes> = const { RefCell::new(Noes(BTreeMap::new())) };
}

/// `act` asked in its window, and its answer ("cancelled" when you said no).
/// One at a time: while one is open, another is refused and the open one
/// comes forward.
pub async fn ask(bridge: std::rc::Weak<Bridge>, act: Act) -> Result<Json, String> {
    let bridge = bridge.upgrade().ok_or("closed")?;
    if let Some(open) = OPEN.with(|open| open.borrow().clone()) {
        open.asked.raise();
        if let Some(shared) = &*open.shown.borrow() {
            shared.set(open.asked.record());
        }
        open.window.set_visible(true);
        open.window.request_focus();
        return Err("cancelled".into());
    }
    let kind = act.item["act"].as_str().unwrap_or_default().to_owned();
    if NOES.with(|n| n.borrow().refused(&kind, Instant::now())) {
        return Err("asked_lately".into());
    }
    let (tx, mut rx) = zgx::task::latest::<Result<Json, String>>();
    let asked = Rc::new(asked(act, move |answer| {
        tx.send(answer);
    }));
    let shown: Rc<RefCell<Option<Shared>>> = Rc::default();
    let (a, slot) = (asked.clone(), shown.clone());
    let window = bridge.windows.open(options(), move |cx| {
        let host = host(cx, a.clone(), &slot);
        crate::serve_sheet(cx, host, "store_consent.luau", false);
        cx.on_closed(move || {
            OPEN.with(|open| open.borrow_mut().take());
            a.close();
        });
    });
    OPEN.with(|open| *open.borrow_mut() = Some(Opened { window, asked, shown }));
    drop(bridge);
    rx.recv().await.unwrap_or_else(|| Err("cancelled".into()))
}

fn asked(act: Act, tell: impl FnOnce(Result<Json, String>) + 'static) -> Asked {
    Asked {
        item: act.item,
        run: RefCell::new(Some(act.run)),
        stop: act.stop,
        undo: act.undo,
        since: Cell::new(Instant::now()),
        raised: Cell::new(0),
        tell: RefCell::new(Some(Box::new(tell))),
        busy: Cell::new(false),
        closed: Cell::new(false),
        secret: RefCell::new(None),
    }
}

/// The main window stopped waiting for a token (its Stop waiting, its page
/// left). Nothing else is its to close: a publish keeps its window.
pub fn give_up() {
    if let Some(open) = OPEN.with(|open| open.borrow().clone())
        && open.asked.act() == "token"
        && open.asked.secret.borrow().is_none()
    {
        open.window.close();
    }
}

fn options() -> WindowOptions {
    WindowOptions {
        resizable: false,
        ..crate::zeron_window(SIZE.0, SIZE.1)
    }
}

fn host(cx: &mut WindowContext, asked: Rc<Asked>, shown: &Rc<RefCell<Option<Shared>>>) -> ScriptHost {
    let host = ScriptHost::new(&cx.ui.runtime, crate::ui_dir().join("store_consent.luau")).app_id(crate::APP_ID);
    host.window_chrome(cx.window_info.titlebar_area.clone(), cx.window_info.controls.clone());
    host.window_zoom(&cx.ui);
    crate::transparency::install(cx, &host);
    let shared = host.shared("store_consent", asked.record());
    *shown.borrow_mut() = Some(shared.clone());
    asked.raise();
    let window = cx.window.clone();
    host.action("appearance", move |value| {
        crate::set_appearance(
            &window,
            match value {
                Value::Bool(dark) => Some(dark),
                _ => None,
            },
        )
    });
    let window = cx.window.clone();
    host.action("close", move |_| window.close());
    let window = cx.window.clone();
    host.action("store.decide", move |arg| {
        let allow = arg.field("allow").is_some_and(Value::truthy);
        eprintln!("zeron-zgx: store: {} {}", asked.item["act"].as_str().unwrap_or_default(), if allow { "allowed" } else { "not allowed (Not now)" });
        decide(asked.clone(), shared.clone(), allow, {
            let window = window.clone();
            move || window.close()
        });
    });
    host
}

fn decide(asked: Rc<Asked>, shared: Shared, allow: bool, close: impl Fn() + 'static) {
    crate::engine::get().spawn(async move {
        let work = asked.decide(allow);
        let mut work = std::pin::pin!(work);
        // (Begun, so the window shows it under way before it's awaited.)
        let done = std::future::poll_fn(|cx| {
            let polled = work.as_mut().poll(cx);
            if polled.is_pending() {
                shared.set(asked.record());
            }
            polled
        })
        .await;
        if done {
            close();
        } else {
            shared.set(asked.record());
        }
    });
}

/// What the window says of publishing `id`'s `version`.
pub fn publishing(kind: &str, id: &str, name: &str, version: &str) -> Json {
    json!({ "act": "publish", "kind": kind, "id": id, "name": plain(name, 64), "version": plain(version, 32) })
}

/// Of taking new installs of it again.
pub fn unyanking(kind: &str, id: &str, version: &str) -> Json {
    json!({ "act": "unyank", "kind": kind, "id": id, "name": id, "version": plain(version, 32) })
}

/// Of a new deploy token, as it was asked for.
pub fn token(params: &Json) -> Json {
    json!({ "act": "token", "name": plain(params["name"].as_str().unwrap_or_default(), 64), "scope": params["scope"], "expires": params["expires"] })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block<T>(f: impl Future<Output = T>) -> T {
        let mut f = std::pin::pin!(f);
        let waker = std::task::Waker::noop();
        let mut cx = std::task::Context::from_waker(waker);
        loop {
            if let std::task::Poll::Ready(v) = f.as_mut().poll(&mut cx) {
                return v;
            }
        }
    }

    type Told = Rc<RefCell<Vec<Result<Json, String>>>>;

    /// What an act did: how often it ran, stopped and was undone.
    type Did = Rc<Cell<(u32, u32, u32)>>;

    /// An act that answers `reply`, shown long enough to take a yes; what it
    /// did and what was told of it.
    fn act(reply: Result<Json, String>, undone: Result<Json, String>) -> (Rc<Asked>, Did, Told) {
        NOES.with(|n| *n.borrow_mut() = Noes::default());
        let did: Did = Rc::default();
        let told: Told = Rc::default();
        let (ran, stopped, undid, t) = (did.clone(), did.clone(), did.clone(), told.clone());
        let a = asked(
            Act {
                item: json!({ "act": "token" }),
                run: Box::new(move || {
                    let (a, b, c) = ran.get();
                    ran.set((a + 1, b, c));
                    Box::pin(async move { reply })
                }),
                stop: Box::new(move || {
                    let (a, b, c) = stopped.get();
                    stopped.set((a, b + 1, c));
                }),
                undo: Some(Box::new(move |made| {
                    assert!(made.get("secret").is_none());
                    let (a, b, c) = undid.get();
                    undid.set((a, b, c + 1));
                    let undone = undone.clone();
                    Box::pin(async move { undone })
                })),
            },
            move |answer| t.borrow_mut().push(answer),
        );
        a.since.set(Instant::now() - ARMED);
        (Rc::new(a), did, told)
    }

    fn made() -> Result<Json, String> {
        Ok(json!({ "ok": true, "secret": "zdt_secret", "token": { "id": "t" } }))
    }

    fn cancelled() -> Vec<Result<Json, String>> {
        vec![Err("cancelled".to_string())]
    }

    #[test]
    fn nothing_is_done_until_you_say_so_and_then_once() {
        let (a, did, told) = act(Ok(json!({ "ok": true })), made());
        assert_eq!((did.get(), told.borrow().len()), ((0, 0, 0), 0));
        assert!(block(a.decide(true)), "nothing more to show");
        assert_eq!(*told.borrow(), vec![Ok(json!({ "ok": true }))]);
        // Again, or closed after: neither does or tells it twice.
        block(a.decide(true));
        a.close();
        assert_eq!((did.get(), told.borrow().len()), ((1, 0, 0), 1));
    }

    #[test]
    fn not_now_and_closing_do_nothing() {
        for close in [false, true] {
            let (a, did, told) = act(Ok(json!({ "ok": true })), made());
            if close {
                a.close();
            } else {
                assert!(block(a.decide(false)));
            }
            block(a.decide(true));
            assert_eq!(did.get(), (0, 0, 0));
            assert_eq!(*told.borrow(), cancelled());
        }
    }

    /// A yes as the window comes up (a second click or Enter meant for the
    /// main window) isn't one; nor as it's brought forward again.
    #[test]
    fn yes_is_taken_only_once_it_has_shown_a_moment() {
        let (a, did, _) = act(Ok(json!({ "ok": true })), made());
        a.raise();
        assert!(!block(a.decide(true)));
        assert!(a.may_allow(Instant::now() + ARMED) && !a.may_allow(Instant::now() + ARMED / 2));
        assert_eq!(did.get(), (0, 0, 0));
        a.since.set(Instant::now() - ARMED);
        a.raise();
        assert!(!block(a.decide(true)), "brought forward");
        assert_eq!(did.get(), (0, 0, 0));
        a.since.set(Instant::now() - ARMED);
        assert!(block(a.decide(true)));
        assert_eq!(did.get().0, 1);
        // No is taken at once.
        let (b, _, told) = act(made(), made());
        b.raise();
        assert!(block(b.decide(false)));
        assert_eq!(*told.borrow(), cancelled());
    }

    #[test]
    fn after_a_no_another_is_refused_longer_each_time_until_a_yes() {
        let now = Instant::now();
        let mut noes = Noes::default();
        let mut waits = Vec::new();
        for _ in 0..8 {
            assert!(!noes.refused("publish", now + Duration::from_secs(600)));
            noes.no("publish", now);
            waits.push(noes.0["publish"].1.duration_since(now).as_secs());
        }
        assert_eq!(waits, [30, 60, 120, 240, 480, 600, 600, 600]);
        assert!(noes.refused("publish", now + Duration::from_secs(599)));
        assert!(!noes.refused("token", now), "each act its own");
        noes.yes("publish");
        assert!(!noes.refused("publish", now));
        // A no, or a window closed unanswered; a yes forgets them.
        let refused = || NOES.with(|n| n.borrow().refused("token", Instant::now()));
        let (a, ..) = act(Ok(json!({})), made());
        a.close();
        assert!(refused());
        let (b, ..) = act(Ok(json!({})), made());
        NOES.with(|n| n.borrow_mut().no("token", Instant::now()));
        block(b.decide(true));
        assert!(!refused());
    }

    #[test]
    fn a_new_token_stays_in_the_window_that_made_it() {
        let (a, did, told) = act(made(), made());
        assert!(!block(a.decide(true)), "the window shows it");
        assert_eq!(a.record().field("secret").map(crate::auth::text).as_deref(), Some("zdt_secret"));
        assert_eq!(*told.borrow(), vec![Ok(json!({ "ok": true, "token": { "id": "t" } }))]);
        assert!(!format!("{:?}", told.borrow()).contains("zdt_"));
        // Closed, it's gone from here too; the token stays, seen.
        a.close();
        assert!(a.record().field("secret").is_none_or(|v| matches!(v, Value::Nil)));
        assert_eq!(did.get(), (1, 0, 0));
    }

    /// Closed while it's being done: it's asked to stop, and told as it ends.
    /// A token made all the same is revoked (or, if it can't be, said); a
    /// publish that went through is never said not to have.
    #[test]
    fn closed_while_it_s_done_it_tells_what_came_of_it() {
        for (reply, undone, tells, undos) in [
            (Err("cancelled".to_string()), made(), cancelled(), 0),
            (made(), Ok(json!({ "ok": true })), cancelled(), 1),
            (made(), Err("offline".to_string()), vec![Ok(json!({ "ok": true, "token": { "id": "t" } }))], 1),
            (Ok(json!({ "ok": true, "version": "1.0.0" })), made(), vec![Ok(json!({ "ok": true, "version": "1.0.0" }))], 0),
        ] {
            let (a, did, told) = act(Ok(json!({})), undone);
            let closing = a.clone();
            *a.run.borrow_mut() = Some(Box::new(move || {
                Box::pin(async move {
                    closing.close();
                    reply
                })
            }));
            assert!(block(a.decide(true)), "nothing to show: it has gone");
            assert_eq!((*told.borrow()).clone(), tells);
            assert_eq!((did.get().1, did.get().2), (1, undos));
            assert!(a.secret.borrow().is_none());
        }
    }

    #[test]
    fn its_words_are_bounded_and_plain() {
        let item = token(&json!({ "name": "CI\nCan publish: nothing\u{202e}x\u{200b}", "scope": {}, "expires": null }));
        assert_eq!(item["name"], "CI Can publish: nothingx");
        assert_eq!(publishing("plugin", "a", &"é".repeat(200), "1.0.0")["name"].as_str().unwrap().chars().count(), 64);
    }
}
