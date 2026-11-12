//! The Zeron engine this app works through: its account (src/auth.rs), the
//! agents on this Mac (src/agents.rs) and GitHub for Cloud (src/github.rs)
//! are all the engine's, reached over its local IPC ([`rpc`]). One
//! connection for every window, kept open: dialled again when it drops,
//! the engine started when nothing answers ([`manager`], which also says
//! which engine and refuses production's).
//!
//! Callers `call` a method (waiting for a connection first) or ask to hear
//! each new connection (`on_connect`) to start their watches on it.
//!
//! A launch has no connection until its engine answers, and that can take as
//! long as the engine before it takes to leave. Until then nothing is wrong:
//! calls wait and screens stay loading (`keeps_waiting`, `says_unreachable`).
//! An engine that answered and went is reconnected to in the same way, and
//! only said as unreachable after [`CONNECT_WAIT`].

pub mod lists;
pub mod manager;
pub mod rpc;

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::{Duration, Instant},
};

use serde_json::{Value as Json, json};
use zgx_desktop::{TaskSpawner, WindowContext};

use manager::{Config, Spawner};
use rpc::{DialError, Rpc};

/// How long a connection lost, or an engine this app can't start, takes to
/// be said as `offline`.
const CONNECT_WAIT: Duration = Duration::from_secs(20);

type Listener = Rc<dyn Fn(&Rpc)>;

/// This device's id, as the engine on `rpc` says it: asked again, waiting
/// a little longer each time (to 5 s), until it answers or `rpc` closes.
/// Asked once, a failed ask (the engine busy, its socket dropping mid-call)
/// left the app not knowing which device is here for the whole connection:
/// this device's own chats were then routed and labelled as another
/// device's ("being edited on another device", a chat that kept loading).
pub async fn local_device(rpc: &Rpc) -> Option<String> {
    ask_until_answered(
        || rpc.call("LocalDevice", json!({})),
        || rpc.is_closed(),
        |wait| Box::pin(zgx::timer::sleep(wait)),
    )
    .await
}

async fn ask_until_answered<A, AF>(
    ask: A,
    closed: impl Fn() -> bool,
    sleep: impl Fn(Duration) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()>>>,
) -> Option<String>
where
    A: Fn() -> AF,
    AF: std::future::Future<Output = Result<Json, String>>,
{
    let mut wait = Duration::from_millis(250);
    loop {
        if closed() {
            return None;
        }
        if let Ok(answer) = ask().await
            && let Some(id) = answer["deviceId"].as_str()
        {
            return Some(id.to_owned());
        }
        sleep(wait).await;
        wait = (wait * 2).min(Duration::from_secs(5));
    }
}

pub struct Engine {
    config: Result<Config, manager::Refusal>,
    /// The connection, while there is one.
    rpc: RefCell<Option<Rpc>>,
    /// Its engine's workspace (`local`, `synced`, `development`), once asked.
    scope: RefCell<Option<String>>,
    /// Whether its session is signed in, as the account last heard.
    signed_in: Cell<bool>,
    /// A restart asked of this connection's engine (once each).
    restarting: Cell<bool>,
    listeners: RefCell<Vec<Listener>>,
    /// An engine has answered in this run.
    answered: Cell<bool>,
    /// Since when there has been no connection (the launch, or its loss).
    down_since: Cell<Instant>,
    /// What hears that no connection is now said as unreachable.
    unreachable_listeners: RefCell<Vec<Rc<dyn Fn()>>>,
    /// The latest window's tasks: the connection lives on them, and moves to
    /// the next window's when that window closes.
    tasks: RefCell<Option<TaskSpawner>>,
    supervising: Cell<bool>,
}

thread_local! {
    static ENGINE: Rc<Engine> = Rc::new(Engine::new());
}

/// The engine's data dir, from the config (see `Engine::data_dir`).
pub fn data_dir() -> Option<std::path::PathBuf> {
    get().data_dir()
}

pub fn get() -> Rc<Engine> {
    ENGINE.with(Rc::clone)
}

/// The engine for `cx`'s window: the connection starts with the first
/// window to ask, and moves on if its window closes.
pub fn install(cx: &mut WindowContext) {
    let engine = get();
    *engine.tasks.borrow_mut() = Some(cx.tasks.clone());
    if !engine.supervising.get() {
        engine.supervising.set(true);
        cx.tasks.spawn(engine.clone().supervise());
    }
}

/// Clears `supervising` however the supervisor ends (its window closing
/// drops it), so the next window starts it again.
struct Supervising(Rc<Engine>);

impl Drop for Supervising {
    fn drop(&mut self) {
        self.0.supervising.set(false);
        self.0.rpc.borrow_mut().take();
        self.0.down_since.set(Instant::now());
    }
}

impl Engine {
    fn new() -> Self {
        let account = crate::auth::endpoint();
        let config = Config::from_env(&account.edge, &account.client, account.port);
        match &config {
            Ok(config) => eprintln!("zeron-zgx: engine: {}", config.describe()),
            Err(why) => eprintln!("zeron-zgx: engine: refused ({why}); nothing that needs it works"),
        }
        Self {
            config,
            rpc: RefCell::default(),
            scope: RefCell::default(),
            signed_in: Cell::new(false),
            restarting: Cell::new(false),
            listeners: RefCell::default(),
            answered: Cell::new(false),
            down_since: Cell::new(Instant::now()),
            unreachable_listeners: RefCell::default(),
            tasks: RefCell::default(),
            supervising: Cell::new(false),
        }
    }

    /// Where the engine's IPC is (its port and data folder, which holds its
    /// token): for a helper process of this app's own that talks to it
    /// directly (src/terminal.rs's bridge).
    pub fn ipc_endpoint(&self) -> Option<(u16, std::path::PathBuf)> {
        self.config.as_ref().ok().map(|c| (c.port, c.data_dir.clone()))
    }

    /// How this app reaches and starts its engine (none: refused).
    pub fn config(&self) -> Option<&Config> {
        self.config.as_ref().ok()
    }

    /// Asks the engine to stop: the connection ends and the supervisor
    /// starts the engine on disk in its place (if this app may start one).
    pub async fn stop(&self) -> Result<(), String> {
        let Some(rpc) = self.rpc.borrow().clone() else {
            return Err("offline".into());
        };
        match rpc.call("StopEngine", json!({})).await {
            Ok(_) => Ok(()),
            Err(error) if error == "closed" => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// The connection, while there is one (to start a watch on it now).
    pub fn current(&self) -> Option<Rpc> {
        self.rpc.borrow().clone()
    }

    /// The engine's data dir, as this app's config has it (nothing when the
    /// config refused one).
    pub fn data_dir(&self) -> Option<std::path::PathBuf> {
        self.config.as_ref().ok().map(|c| c.data_dir.clone())
    }

    /// `work` on the connection's tasks.
    pub fn spawn(&self, work: impl std::future::Future<Output = ()> + 'static) {
        if let Some(tasks) = &*self.tasks.borrow() {
            tasks.spawn(work);
        }
    }

    /// `listener` hears every connection from now on (the current one at
    /// once): where it starts its watches.
    pub fn on_connect(&self, listener: impl Fn(&Rpc) + 'static) {
        let listener: Listener = Rc::new(listener);
        self.listeners.borrow_mut().push(listener.clone());
        let current = self.rpc.borrow().clone();
        if let Some(rpc) = current {
            listener(&rpc);
        }
    }

    /// `listener` hears when no connection comes to be said as unreachable,
    /// for a screen that shows it to read [`Engine::unreachable`] again.
    pub fn on_unreachable(&self, listener: impl Fn() + 'static) {
        self.unreachable_listeners.borrow_mut().push(Rc::new(listener));
    }

    /// Whether no connection is to be said as "can't reach the engine". (With
    /// a connection, it's not asked.)
    pub fn unreachable(&self) -> bool {
        says_unreachable(self.config.is_err(), self.answered.get(), self.can_start(), self.down_since.get().elapsed())
    }

    /// Whether this app can start an engine, should none answer.
    pub fn can_start(&self) -> bool {
        self.config.as_ref().is_ok_and(|config| config.startable().is_some())
    }

    /// The connection, waiting up to `wait` for one.
    pub async fn connection(&self, wait: Duration) -> Option<Rpc> {
        let until = Instant::now() + wait;
        loop {
            if let Some(rpc) = self.rpc.borrow().clone().filter(|rpc| !rpc.is_closed()) {
                return Some(rpc);
            }
            if self.config.is_err() || Instant::now() >= until {
                return None;
            }
            zgx::timer::sleep(Duration::from_millis(100)).await;
        }
    }

    /// The connection for a call: waiting for it for as long as a launch's
    /// engine is on its way, else up to [`CONNECT_WAIT`].
    async fn connection_for_call(&self) -> Option<Rpc> {
        let until = Instant::now() + CONNECT_WAIT;
        loop {
            if let Some(rpc) = self.rpc.borrow().clone().filter(|rpc| !rpc.is_closed()) {
                return Some(rpc);
            }
            let waiting = keeps_waiting(self.answered.get(), self.can_start());
            if self.config.is_err() || (!waiting && Instant::now() >= until) {
                return None;
            }
            zgx::timer::sleep(Duration::from_millis(100)).await;
        }
    }

    /// A unary call: its answer, or the engine's error, or `offline` when
    /// there's no engine to ask.
    pub async fn call(&self, method: &str, params: Json) -> Result<Json, String> {
        let Some(rpc) = self.connection_for_call().await else {
            return Err("offline".into());
        };
        rpc.call(method, params).await
    }

    /// [`Engine::call`] that says `offline` after [`CONNECT_WAIT`] whatever
    /// the engine is doing: for what someone is waiting on an answer to.
    pub async fn call_soon(&self, method: &str, params: Json) -> Result<Json, String> {
        let Some(rpc) = self.connection(CONNECT_WAIT).await else {
            return Err("offline".into());
        };
        rpc.call(method, params).await
    }

    /// What the account heard of the session: the engine starts again if
    /// its workspace no longer follows it (see [`manager::needs_restart`]).
    pub fn session(&self, signed_in: bool) {
        self.signed_in.set(signed_in);
        self.follow_session();
    }

    fn follow_session(&self) {
        let scope = self.scope.borrow().clone();
        if self.restarting.get() || !manager::needs_restart(scope.as_deref(), self.signed_in.get()) {
            return;
        }
        let Some(rpc) = self.rpc.borrow().clone() else {
            return;
        };
        self.restarting.set(true);
        eprintln!("zeron-zgx: engine: signed in on a local workspace; starting it again to sync");
        self.spawn(async move {
            // It answers, then stops; the connection ends and a new engine
            // is started in its place.
            if let Err(error) = rpc.call("StopEngine", json!({})).await
                && error != "closed"
            {
                eprintln!("zeron-zgx: engine: didn't stop: {error}");
            }
        });
    }

    async fn supervise(self: Rc<Self>) {
        let _supervising = Supervising(self.clone());
        let Ok(config) = &self.config else {
            return;
        };
        let mut spawner = Spawner::new();
        let mut delay = Duration::from_millis(250);
        let mut said = None::<String>;
        let mut said_unreachable = false;
        // The token the engine last refused, if it refused the last dial.
        let mut refused = None::<Option<String>>;
        loop {
            let token = rpc::read_token(&config.data_dir);
            // Refused again with the same token: nothing will change by
            // dialling as often (a data dir with a copy of an engine's token,
            // which that engine replaced as it started again).
            let stale = refused.as_ref() == Some(&token);
            let mut refused_now = false;
            match rpc::connect(config.port, token.clone()).await {
                Ok(rpc) => {
                    delay = Duration::from_millis(250);
                    said = None;
                    said_unreachable = false;
                    refused = None;
                    self.answered.set(true);
                    eprintln!("zeron-zgx: engine: connected");
                    *self.rpc.borrow_mut() = Some(rpc.clone());
                    self.scope.borrow_mut().take();
                    self.restarting.set(false);
                    // Which workspace it opened: whether a session change
                    // needs it started again.
                    let (me, info) = (self.clone(), rpc.clone());
                    self.spawn(async move {
                        if let Ok(answer) = info.call("EngineInfo", json!({})).await {
                            *me.scope.borrow_mut() = answer["workspaceScope"].as_str().map(Into::into);
                            me.follow_session();
                        }
                    });
                    let listeners = self.listeners.borrow().clone();
                    for listener in listeners {
                        listener(&rpc);
                    }
                    let reason = rpc.pump().await;
                    self.rpc.borrow_mut().take();
                    self.down_since.set(Instant::now());
                    eprintln!("zeron-zgx: engine: connection ended ({reason})");
                }
                Err(DialError::Refused) => {
                    refused = None;
                    // (Started one, or an engine is about to let go of the
                    // data dir: it's a moment away, not worth backing off.)
                    if spawner.start_if_due(config) || spawner.held() {
                        delay = Duration::from_millis(500);
                    }
                }
                // A new engine is writing its token: try again shortly. The
                // same token refused twice: said once, and tried less often.
                Err(DialError::Unauthorized) => {
                    if stale && said.as_deref() != Some(STALE_TOKEN) {
                        eprintln!(
                            "zeron-zgx: engine: {STALE_TOKEN} ({}): trying it again less often",
                            config.data_dir.join("ipc-token").display()
                        );
                        said = Some(STALE_TOKEN.to_string());
                    }
                    refused = Some(token);
                    refused_now = true;
                }
                Err(DialError::Other(error)) => {
                    refused = None;
                    if said.as_deref() != Some(&error) {
                        eprintln!("zeron-zgx: engine: can't connect: {error}");
                        said = Some(error);
                    }
                }
            }
            if !said_unreachable && self.rpc.borrow().is_none() && self.unreachable() {
                said_unreachable = true;
                let listeners = self.unreachable_listeners.borrow().clone();
                for listener in listeners {
                    listener();
                }
            }
            // While its token stays one the engine refused, only the file is
            // looked at (every few seconds, no dial): a new token is tried as
            // it comes, the old one again only at the long wait's end.
            let waited = Instant::now();
            loop {
                zgx::timer::sleep(delay.min(TOKEN_LOOK)).await;
                let changed = refused.as_ref().is_some_and(|was| *was != rpc::read_token(&config.data_dir));
                if changed || waited.elapsed() >= delay {
                    break;
                }
            }
            delay = next_delay(delay, stale && refused_now);
        }
    }
}

/// What a refused dial says when the token was the one refused before.
const STALE_TOKEN: &str = "the engine refuses this window's token";
/// How often a refused token's file is looked at for a new one.
const TOKEN_LOOK: Duration = Duration::from_secs(3);
/// The longest wait between dials with a token already refused.
const STALE_TOKEN_WAIT: Duration = Duration::from_secs(60);

/// The wait before the dial after this one: doubling, to 3 s, as it always
/// has; to a minute while the engine keeps refusing the same token.
fn next_delay(delay: Duration, stale_token: bool) -> Duration {
    (delay * 2).min(if stale_token { STALE_TOKEN_WAIT } else { Duration::from_secs(3) })
}

/// Whether a call keeps waiting for a connection past its time.
fn keeps_waiting(answered: bool, startable: bool) -> bool {
    !answered && startable
}

/// Whether no connection is to be said as "can't reach the engine".
fn says_unreachable(refused: bool, answered: bool, startable: bool, down_for: Duration) -> bool {
    refused || (!keeps_waiting(answered, startable) && down_for >= CONNECT_WAIT)
}

/// An engine error as a script's outcome word: `offline` (no engine, or the
/// connection dropped mid-call), `signed_out`, or `failed`.
pub fn outcome(error: &str) -> &'static str {
    let lower = error.to_ascii_lowercase();
    if error == "offline" || error == "closed" || lower.contains("unreachable") {
        "offline"
    } else if lower.contains("signed out") || lower.contains("signed-in") || lower.contains("sign in") {
        "signed_out"
    } else {
        "failed"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LONG: Duration = Duration::from_secs(3600);

    fn run<T>(future: impl std::future::Future<Output = T>) -> T {
        let mut future = std::pin::pin!(future);
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        loop {
            if let std::task::Poll::Ready(out) = future.as_mut().poll(&mut cx) {
                return out;
            }
        }
    }

    /// 2026-11-11, work-laptop: the one ask for this device's id failed and
    /// was never made again, so the Mac's own chats were treated as another
    /// device's. Failed asks are made again, waiting longer each time, until
    /// the engine answers; a closed connection stops it.
    #[test]
    fn this_device_is_asked_for_until_the_engine_answers() {
        let asks = Cell::new(0);
        let waits = RefCell::new(Vec::new());
        let here = run(ask_until_answered(
            || {
                asks.set(asks.get() + 1);
                let answer = match asks.get() {
                    1 => Err("closed".to_owned()),
                    2 => Ok(json!({})),
                    3 => Err("timed out".to_owned()),
                    _ => Ok(json!({ "deviceId": "mac" })),
                };
                std::future::ready(answer)
            },
            || false,
            |wait| {
                waits.borrow_mut().push(wait);
                Box::pin(std::future::ready(()))
            },
        ));
        assert_eq!(here.as_deref(), Some("mac"));
        assert_eq!(asks.get(), 4);
        assert_eq!(
            *waits.borrow(),
            [250, 500, 1000].map(Duration::from_millis).to_vec()
        );

        let closed = run(ask_until_answered(
            || std::future::ready(Err("closed".to_owned())),
            || true,
            |_| Box::pin(std::future::ready(())),
        ));
        assert_eq!(closed, None);
    }

    /// A window whose data dir holds a copy of an engine's token (a test
    /// window on the user's engine) once dialled every 3 s for as long as it
    /// ran after that engine started again, each a refused handshake in the
    /// engine's log: the same token refused again backs off to a minute.
    /// Anything else retries as it always has, to 3 s.
    #[test]
    fn a_token_refused_again_is_dialled_less_often() {
        let dials_in_ten_minutes = |stale: bool| {
            let (mut delay, mut at, mut dials) = (Duration::from_millis(250), Duration::ZERO, 0);
            while at < Duration::from_secs(600) {
                dials += 1;
                at += delay;
                delay = next_delay(delay, stale);
            }
            dials
        };
        assert!(dials_in_ten_minutes(false) >= 195, "an engine starting is retried as before");
        assert!(dials_in_ten_minutes(true) <= 20, "{} dials", dials_in_ten_minutes(true));
        assert_eq!(next_delay(Duration::from_secs(3), false), Duration::from_secs(3));
        assert_eq!(next_delay(Duration::from_secs(40), true), STALE_TOKEN_WAIT);
    }

    /// Starting up, an engine this app can start is on its way, however long
    /// the one before it takes to leave: nothing gives up on it.
    #[test]
    fn a_launch_keeps_waiting_for_an_engine_it_can_start() {
        assert!(keeps_waiting(false, true));
        assert!(!says_unreachable(false, false, true, LONG));
    }

    /// With no engine to start, or a config refused, nothing is coming.
    #[test]
    fn nothing_waits_for_an_engine_that_can_t_come() {
        assert!(!keeps_waiting(false, false));
        assert!(!says_unreachable(false, false, false, Duration::ZERO));
        assert!(says_unreachable(false, false, false, CONNECT_WAIT));
        assert!(says_unreachable(true, false, true, Duration::ZERO));
    }

    /// Once it has answered, losing it is a moment's reconnect, and past that
    /// it's said as before.
    #[test]
    fn losing_an_engine_that_answered_is_said_after_a_while() {
        assert!(!keeps_waiting(true, true));
        assert!(!says_unreachable(false, true, true, Duration::from_secs(1)));
        assert!(says_unreachable(false, true, true, CONNECT_WAIT));
    }
}
