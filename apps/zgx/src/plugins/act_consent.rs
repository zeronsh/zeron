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
//! alone). Closed before it's answered, or while it's being made, nothing is
//! made or sent.
//!
//! It guards the buttons, not the machine: a template kept by the person can
//! start a terminal and publish with the CLI. The notice every publish raises
//! is what always stands.

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;

use serde_json::{Value as Json, json};
use zgx_desktop::{WindowContext, WindowHandle, WindowOptions};
use zgx_script::{ScriptHost, Shared, Value};

use super::{Bridge, json_value};

/// The window, in points: the consent windows'.
const SIZE: (f64, f64) = (460., 480.);

pub type Answer = Pin<Box<dyn Future<Output = Result<Json, String>>>>;
/// What it does once you say so.
pub type Run = Box<dyn FnOnce() -> Answer>;

/// One thing asked: what the window says (`item`), what it does, and what
/// stops it half-way (a sign-in waited for).
pub struct Act {
    pub item: Json,
    pub run: Run,
    pub stop: Box<dyn Fn()>,
}

/// Where an act stands, whatever draws it.
struct Asked {
    item: Json,
    run: RefCell<Option<Run>>,
    stop: Box<dyn Fn()>,
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

    /// Closed: an act not begun isn't done, and one waiting on a sign-in
    /// stops waiting.
    fn close(&self) {
        self.closed.set(true);
        self.run.borrow_mut().take();
        self.secret.borrow_mut().take();
        if self.busy.get() {
            (self.stop)();
        }
        self.tell(Err("cancelled".into()));
    }

    /// What you answered. `true` when the window has nothing more to show.
    async fn decide(&self, allow: bool) -> bool {
        if !allow {
            self.close();
            return true;
        }
        let Some(run) = self.run.borrow_mut().take() else { return false };
        self.busy.set(true);
        let mut answer = run().await;
        self.busy.set(false);
        // (The token stays here; its record alone goes back.)
        let secret = answer.as_mut().ok().and_then(Json::as_object_mut).and_then(|o| o.remove("secret"));
        let shown = secret.as_ref().and_then(Json::as_str).filter(|s| !s.is_empty() && !self.closed.get()).map(str::to_owned);
        let more = shown.is_some();
        *self.secret.borrow_mut() = shown;
        self.tell(answer);
        !more
    }

    fn record(&self) -> Value {
        let mut fields = vec![("item", json_value(&self.item)), ("busy", Value::from(self.busy.get()))];
        if let Some(secret) = self.secret.borrow().as_deref() {
            fields.push(("secret", Value::from(secret)));
        }
        Value::record(fields)
    }
}

thread_local! {
    static OPEN: RefCell<Option<(WindowHandle, Rc<Asked>)>> = const { RefCell::new(None) };
}

/// `act` asked in its window, and its answer ("cancelled" when you said no).
/// One at a time: while one is open, another is refused and the open one
/// comes forward.
pub async fn ask(bridge: std::rc::Weak<Bridge>, act: Act) -> Result<Json, String> {
    let bridge = bridge.upgrade().ok_or("closed")?;
    let open = OPEN.with(|open| open.borrow().as_ref().map(|(window, _)| window.clone()));
    if let Some(window) = open {
        window.set_visible(true);
        window.request_focus();
        return Err("cancelled".into());
    }
    let (tx, mut rx) = zgx::task::latest::<Result<Json, String>>();
    let asked = Rc::new(asked(act, move |answer| {
        tx.send(answer);
    }));
    let shown = asked.clone();
    let window = bridge.windows.open(options(), move |cx| {
        let host = host(cx, shown.clone());
        crate::serve_sheet(cx, host, "store_consent.luau", false);
        cx.on_closed(move || {
            OPEN.with(|open| open.borrow_mut().take());
            shown.close();
        });
    });
    OPEN.with(|open| *open.borrow_mut() = Some((window, asked)));
    drop(bridge);
    rx.recv().await.unwrap_or_else(|| Err("cancelled".into()))
}

fn asked(act: Act, tell: impl FnOnce(Result<Json, String>) + 'static) -> Asked {
    Asked {
        item: act.item,
        run: RefCell::new(Some(act.run)),
        stop: act.stop,
        tell: RefCell::new(Some(Box::new(tell))),
        busy: Cell::new(false),
        closed: Cell::new(false),
        secret: RefCell::new(None),
    }
}

/// Nothing more waits for the main window's answer: it stopped waiting
/// before you answered (its Stop waiting, its page left).
pub fn give_up() {
    let open = OPEN.with(|open| open.borrow().as_ref().map(|(window, asked)| (window.clone(), asked.clone())));
    if let Some((window, asked)) = open
        && asked.secret.borrow().is_none()
    {
        window.close();
    }
}

fn options() -> WindowOptions {
    WindowOptions {
        resizable: false,
        ..crate::zeron_window(SIZE.0, SIZE.1)
    }
}

fn host(cx: &mut WindowContext, asked: Rc<Asked>) -> ScriptHost {
    let host = ScriptHost::new(&cx.ui.runtime, crate::ui_dir().join("store_consent.luau")).app_id(crate::APP_ID);
    host.window_chrome(cx.window_info.titlebar_area.clone(), cx.window_info.controls.clone());
    host.window_zoom(&cx.ui);
    crate::transparency::install(cx, &host);
    let shared = host.shared("store_consent", asked.record());
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
    json!({ "act": "publish", "kind": kind, "id": id, "name": name, "version": version })
}

/// Of taking new installs of it again.
pub fn unyanking(kind: &str, id: &str, version: &str) -> Json {
    json!({ "act": "unyank", "kind": kind, "id": id, "name": id, "version": version })
}

/// Of a new deploy token, as it was asked for.
pub fn token(params: &Json) -> Json {
    json!({ "act": "token", "name": params["name"], "scope": params["scope"], "expires": params["expires"] })
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

    /// An act that answers `reply`, what it did and what was told of it.
    fn act(reply: Result<Json, String>) -> (Asked, Rc<Cell<(u32, u32)>>, Told) {
        let did: Rc<Cell<(u32, u32)>> = Rc::default();
        let told: Told = Rc::default();
        let (ran, stopped, t) = (did.clone(), did.clone(), told.clone());
        let a = asked(
            Act {
                item: json!({ "act": "token" }),
                run: Box::new(move || {
                    ran.set((ran.get().0 + 1, ran.get().1));
                    Box::pin(async move { reply })
                }),
                stop: Box::new(move || stopped.set((stopped.get().0, stopped.get().1 + 1))),
            },
            move |answer| t.borrow_mut().push(answer),
        );
        (a, did, told)
    }

    #[test]
    fn nothing_is_done_until_you_say_so_and_then_once() {
        let (a, did, told) = act(Ok(json!({ "ok": true })));
        assert_eq!((did.get(), told.borrow().len()), ((0, 0), 0));
        assert!(block(a.decide(true)), "nothing more to show");
        assert_eq!(*told.borrow(), vec![Ok(json!({ "ok": true }))]);
        // Again, or closed after: neither does or tells it twice.
        block(a.decide(true));
        a.close();
        assert_eq!((did.get(), told.borrow().len()), ((1, 0), 1));
    }

    #[test]
    fn not_now_and_closing_do_nothing() {
        for close in [false, true] {
            let (a, did, told) = act(Ok(json!({ "ok": true })));
            if close {
                a.close();
            } else {
                assert!(block(a.decide(false)));
            }
            block(a.decide(true));
            assert_eq!(did.get(), (0, 0));
            assert_eq!(*told.borrow(), vec![Err("cancelled".to_string())]);
        }
    }

    #[test]
    fn a_new_token_stays_in_the_window_that_made_it() {
        let (a, _, told) = act(Ok(json!({ "ok": true, "secret": "zdt_secret", "token": { "id": "t" } })));
        assert!(!block(a.decide(true)), "the window shows it");
        assert_eq!(a.record().field("secret").map(crate::auth::text).as_deref(), Some("zdt_secret"));
        assert_eq!(*told.borrow(), vec![Ok(json!({ "ok": true, "token": { "id": "t" } }))]);
        assert!(!format!("{:?}", told.borrow()).contains("zdt_"));
        // Closed, it's gone from here too.
        a.close();
        assert!(a.record().field("secret").is_none_or(|v| matches!(v, Value::Nil)));
    }

    #[test]
    fn closing_while_it_waits_on_a_sign_in_stops_the_wait_and_drops_the_token() {
        let (a, did, told) = act(Ok(json!({ "secret": "zdt_secret" })));
        a.busy.set(true);
        a.close();
        assert_eq!(did.get(), (0, 1));
        assert_eq!(*told.borrow(), vec![Err("cancelled".to_string())]);
        // One made all the same isn't kept by a window that has gone.
        let (b, _, _) = act(Ok(json!({ "secret": "zdt_secret" })));
        b.closed.set(true);
        assert!(block(b.decide(true)));
        assert!(b.secret.borrow().is_none());
    }
}
