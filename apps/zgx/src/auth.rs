//! Zeron's account, for the scripts: the engine's session (src/engine),
//! signed into through the system browser (the hosted sign-in, which makes
//! an account or signs into one), and the calls setup makes as it (a
//! username, a card, the organizations it belongs to).
//!
//! The engine owns the session, as it does for the shipping app: it builds
//! the sign-in, catches the browser's return on its loopback, keeps the
//! session (`session.json` in its data dir) and refreshes it. Everything
//! that works as the account — sync, the agents' Cloud logins, GitHub — is
//! the engine's, so this app holds no session of its own: one sign-in, one
//! account, one organization. Here:
//!   - `account` follows the engine's `AuthStatus`;
//!   - signing in asks the engine for the sign-in page (`SignIn`, opening on
//!     making an account or on signing in), opens it, and waits for the
//!     engine's session to arrive; signing out is the engine's `SignOut`;
//!   - choosing an organization scopes the engine's session (`SelectOrg`);
//!   - the edge calls the engine doesn't make (username, card, onboarding,
//!     invites) carry the engine's bearer (`AccessToken`, local IPC only),
//!     kept until it nears its end. It never reaches scripts: they see the
//!     account (`ui.shared("account")`) and call what needs it by name
//!     (`ui.call("account.claim_username", "mara")`).
//!
//! The engine's edge, sign-in client and loopback port come from here (the
//! engine this app starts is given them, src/engine/manager.rs).
//!
//! Zeron.app's beta (the `beta` feature) is production's only: edge.zeron.sh,
//! its client and port 27641 (the only loopback production's WorkOS client
//! accepts, shared with Zeron.app). It has no staging built in, and the
//! variables below do nothing in it.
//!
//! Development builds are staging's by default, production one variable
//! away:
//!   ZERON_ZGX_ENV=production       edge.zeron.sh, its client, port 27641
//!   ZERON_ZGX_EDGE=<url>           the edge (https://staging.zeron.sh)
//!   ZERON_ZGX_WORKOS_CLIENT=<id>   the sign-in client
//!   ZERON_ZGX_CALLBACK_PORT=<n>    the loopback port that client redirects to
//!   ZERON_ZGX_PRIVATE_SIGNIN=1     sign in in a private Chrome window (no
//!                                  browser session picks the account)
//!
//! What scripts see, `account`:
//!   { state = "restoring" | "signed_out" | "waiting" | "signed_in",
//!     user = { id, email, first_name }?, failure = why the last sign-in
//!     didn't finish?, env = "staging" | "production" | "custom" }
//! and call, each rejecting with an outcome word as its `message`:
//!   account.sign_in(kind)        the user; cancelled, timed_out
//!                                ("create" opens on a new account, asked
//!                                afresh; "existing" on signing in)
//!   account.cancel (an action)   gives up the sign-in waiting on the browser
//!   account.sign_out             nil
//!   account.username             your username, or nil
//!   account.check_username(name) { available, reason = invalid | reserved | taken }
//!   account.claim_username(name) your card; taken, reserved, invalid,
//!                                already_claimed, not_ready, rate_limited
//!   account.card(name)           that card, or nil
//!   account.orgs                 { { id, organization_id, name, role } }
//!   account.create_org(name)     { organization_id, existing? }
//!   account.choose_org(id)       { organization_id }; refused
//!   account.onboarding           the onboarding pages finished: { id }
//!   account.onboarded({ id })    records them; every one finished
//!   account.invite({ organization_id, emails })
//!                                { sent, failed = { { email, error } } };
//!                                invalid, refused, rate_limited
//! Any of them can also reject with offline, signed_out or failed. A card:
//!   { username, first_name, number, joined_on, badges = { { id, name, icon } },
//!     scene, holo, sky }
//!
//! (A browser that turns the sign-in down ends at the engine's page; the
//! engine's session never arrives, so it ends `timed_out` or `cancelled`.
//! Earlier builds kept their own session in the Keychain, `sh.zeron.zgx`;
//! it is left as it is, unused.)

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde_json::{Value as Json, json};
use zgx_desktop::WindowContext;
use zgx_script::{ScriptHost, Shared, Value};

use crate::engine::{self, rpc::Watch};

/// Development builds' only: a beta build has none of it.
#[cfg(not(feature = "beta"))]
const STAGING: Env = Env {
    name: "staging",
    edge: "https://staging.zeron.sh",
    client: "client_01KWD0E9SYPV2WE4QN9TP9DP0W",
    port: 27642,
};
const PRODUCTION: Env = Env {
    name: "production",
    edge: "https://edge.zeron.sh",
    client: "client_01KWD0EAKZKD50YCQJNYSRE4BY",
    port: 27641,
};

/// How long the browser has to come back (the engine forgets a sign-in
/// after 15 minutes).
const SIGN_IN_WAIT: Duration = Duration::from_secs(10 * 60);
/// How long a launch waits for the engine's session before saying signed out.
const RESTORE_WAIT: Duration = Duration::from_secs(20);
/// A bearer this close to its end is asked for again.
const TOKEN_SLACK: i64 = 30;
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);

/// Why a call didn't give its answer: the word scripts get.
type Outcome = &'static str;

struct Env {
    name: &'static str,
    edge: &'static str,
    client: &'static str,
    port: u16,
}

/// Where the account lives: the edge, the sign-in client and the loopback
/// port it redirects to. The engine is started with them.
pub struct Endpoint {
    pub name: &'static str,
    pub edge: String,
    pub client: String,
    pub port: u16,
}

pub fn endpoint() -> Endpoint {
    endpoint_with(|key| std::env::var(key).ok().filter(|v: &String| !v.trim().is_empty()))
}

/// The beta's: production's, whatever the environment says. Nothing
/// of staging's is built into it (`STAGING` isn't), and no variable reaches
/// another edge, client or port.
#[cfg(feature = "beta")]
fn endpoint_with(_var: impl Fn(&str) -> Option<String>) -> Endpoint {
    production()
}

/// A development build's: staging's by default, or as the variables say.
#[cfg(not(feature = "beta"))]
fn endpoint_with(var: impl Fn(&str) -> Option<String>) -> Endpoint {
    let base = match var("ZERON_ZGX_ENV").as_deref() {
        Some("production" | "prod") => &PRODUCTION,
        _ => &STAGING,
    };
    let edge = var("ZERON_ZGX_EDGE");
    let client = var("ZERON_ZGX_WORKOS_CLIENT");
    let port = var("ZERON_ZGX_CALLBACK_PORT").and_then(|p| p.parse().ok());
    let custom = edge.is_some() || client.is_some() || port.is_some();
    Endpoint {
        name: if custom { "custom" } else { base.name },
        edge: edge.unwrap_or_else(|| base.edge.into()).trim_end_matches('/').into(),
        client: client.unwrap_or_else(|| base.client.into()),
        port: port.unwrap_or(base.port),
    }
}

/// Production's edge, sign-in client and loopback port.
pub fn production() -> Endpoint {
    Endpoint {
        name: PRODUCTION.name,
        edge: PRODUCTION.edge.into(),
        client: PRODUCTION.client.into(),
        port: PRODUCTION.port,
    }
}

#[cfg(test)]
mod endpoint_tests {
    use super::*;

    /// Every variable that picks an edge, set to staging's.
    fn staging_vars(key: &str) -> Option<String> {
        match key {
            "ZERON_ZGX_ENV" => Some("staging".into()),
            "ZERON_ZGX_EDGE" => Some("https://staging.zeron.sh".into()),
            "ZERON_ZGX_WORKOS_CLIENT" => Some("client_01KWD0E9SYPV2WE4QN9TP9DP0W".into()),
            "ZERON_ZGX_CALLBACK_PORT" => Some("27642".into()),
            _ => None,
        }
    }

    #[cfg(feature = "beta")]
    #[test]
    fn a_beta_build_is_production_only() {
        for vars in [staging_vars as fn(&str) -> Option<String>, |_: &str| None] {
            let at = endpoint_with(vars);
            assert_eq!(
                (at.name, at.edge.as_str(), at.client.as_str(), at.port),
                ("production", "https://edge.zeron.sh", "client_01KWD0EAKZKD50YCQJNYSRE4BY", 27641)
            );
        }
        assert_eq!(endpoint().edge, "https://edge.zeron.sh");
    }

    #[cfg(not(feature = "beta"))]
    #[test]
    fn a_development_build_is_staging_unless_told() {
        let at = endpoint_with(|_| None);
        assert_eq!((at.name, at.edge.as_str(), at.port), ("staging", STAGING.edge, STAGING.port));
        let at = endpoint_with(|key| (key == "ZERON_ZGX_ENV").then(|| "production".into()));
        assert_eq!((at.name, at.edge.as_str(), at.port), ("production", PRODUCTION.edge, PRODUCTION.port));
        assert_eq!(endpoint_with(staging_vars).name, "custom");
    }
}

#[derive(Clone, PartialEq)]
struct User {
    id: String,
    email: String,
    first_name: Option<String>,
}

/// The engine's session, as its `AuthStatus` says.
#[derive(Clone, PartialEq)]
enum Session {
    /// Not heard yet.
    Unknown,
    SignedOut,
    /// Signed in (`org` once it works in one; none while choosing).
    SignedIn { user: User, org: Option<String> },
}

struct Access {
    token: String,
    expires: i64,
}

struct Auth {
    endpoint: Endpoint,
    accounts: RefCell<Vec<Shared>>,
    session: RefCell<Session>,
    /// A sign-in waiting on the browser.
    waiting: Cell<bool>,
    failure: Cell<Option<Outcome>>,
    /// The launch gave up waiting for the engine: shown signed out.
    gave_up: Cell<bool>,
    access: RefCell<Option<Access>>,
    /// Moves on every sign-in, cancel and sign-out: what comes back for an
    /// older one changes nothing.
    generation: Cell<u64>,
    /// Signed out here, as asked: the next sign-in asks for an account
    /// afresh, not the one the browser is still signed into.
    signed_out_here: Cell<bool>,
    /// The engine's `AuthStatus`, on the current connection.
    watch: RefCell<Option<Watch>>,
    started: Cell<bool>,
    /// What waits for the account to work in an organization (`set_up`).
    set_up_waiters: RefCell<Vec<zgx::task::LatestSender<()>>>,
}

/// What the engine answers a call that needs the workspace while the
/// account's setup isn't finished (signed in, no organization chosen: the
/// engine's deferred.rs `SETUP_INCOMPLETE`).
pub const SETUP_INCOMPLETE: &str = "setup_incomplete";

/// Whether an engine error is "the account's setup isn't finished".
pub fn setup_incomplete(error: &str) -> bool {
    error.contains(SETUP_INCOMPLETE)
}

/// Whether the account works in an organization now (`AuthStatus` with an
/// org): the engine answers workspace calls.
pub fn is_set_up() -> bool {
    AUTH.with(|a| matches!(&*a.session.borrow(), Session::SignedIn { org: Some(_), .. }))
}

/// Once the account works in an organization: at once if it does.
pub async fn set_up() {
    if is_set_up() {
        return;
    }
    let (tell, mut heard) = zgx::task::latest::<()>();
    AUTH.with(|a| a.set_up_waiters.borrow_mut().push(tell));
    heard.recv().await;
}

/// `call` (an engine call), again once the account is set up if it answered
/// `setup_incomplete`: what's asked while setup is still being finished
/// (the templates page, a consent window) arrives once it is.
pub async fn after_setup<T, F, Fut>(call: F) -> Result<T, String>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, String>>,
{
    retry_after(call, set_up).await
}

/// How many times a call waits for setup before its answer is taken as is.
const SETUP_RETRIES: usize = 3;

async fn retry_after<T, F, Fut, W, WFut>(mut call: F, mut wait: W) -> Result<T, String>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, String>>,
    W: FnMut() -> WFut,
    WFut: std::future::Future<Output = ()>,
{
    for _ in 0..SETUP_RETRIES {
        match call().await {
            Err(error) if setup_incomplete(&error) => wait().await,
            other => return other,
        }
    }
    call().await
}

thread_local! {
    static AUTH: Rc<Auth> = Rc::new(Auth::new());
}

/// The account for `host`'s scripts: `account` and the `account.*` calls.
/// The first window to ask starts following the engine's session.
pub fn install(cx: &mut WindowContext, host: &ScriptHost) {
    engine::install(cx);
    let auth = AUTH.with(Rc::clone);
    auth.accounts
        .borrow_mut()
        .push(host.shared("account", auth.value()));
    if !auth.started.get() {
        auth.started.set(true);
        auth.clone().follow();
        cx.tasks.spawn(auth.clone().restore_deadline());
    }

    let a = auth.clone();
    host.async_action("account.sign_in", move |kind| {
        let a = a.clone();
        let create = text(&kind) == "create";
        async move { crate::plugins::preview::refuse("account.sign_in")?; a.sign_in(create).await.map_err(String::from) }
    });
    let a = auth.clone();
    host.action("account.cancel", move |_| a.cancel());
    let a = auth.clone();
    host.async_action("account.sign_out", move |_| {
        let a = a.clone();
        async move { crate::plugins::preview::refuse("account.sign_out")?;
            a.sign_out().await;
            Ok(Value::Nil)
        }
    });
    let a = auth.clone();
    host.async_action("account.username", move |_| {
        let a = a.clone();
        async move { crate::plugins::preview::refuse("account.username")?; a.username().await.map_err(String::from) }
    });
    let a = auth.clone();
    host.async_action("account.check_username", move |name| {
        let a = a.clone();
        async move { crate::plugins::preview::refuse("account.check_username")?; a.check_username(text(&name)).await.map_err(String::from) }
    });
    let a = auth.clone();
    host.async_action("account.claim_username", move |name| {
        let a = a.clone();
        async move { crate::plugins::preview::refuse("account.claim_username")?; a.claim_username(text(&name)).await.map_err(String::from) }
    });
    let a = auth.clone();
    host.async_action("account.card", move |name| {
        let a = a.clone();
        async move { crate::plugins::preview::refuse("account.card")?; a.card(text(&name)).await.map_err(String::from) }
    });
    let a = auth.clone();
    host.async_action("account.orgs", move |_| {
        let a = a.clone();
        async move { crate::plugins::preview::refuse("account.orgs")?; a.orgs().await.map_err(String::from) }
    });
    let a = auth.clone();
    host.async_action("account.create_org", move |name| {
        let a = a.clone();
        async move { crate::plugins::preview::refuse("account.create_org")?; a.create_org(text(&name)).await.map_err(String::from) }
    });
    let a = auth.clone();
    host.async_action("account.choose_org", move |id| {
        let a = a.clone();
        async move { crate::plugins::preview::refuse("account.choose_org")?; a.choose_org(text(&id)).await.map_err(String::from) }
    });
    let a = auth.clone();
    host.async_action("account.onboarding", move |_| {
        let a = a.clone();
        async move { crate::plugins::preview::refuse("account.onboarding")?; a.onboarding().await.map_err(String::from) }
    });
    let a = auth.clone();
    host.async_action("account.onboarded", move |ids| {
        let a = a.clone();
        let ids: Vec<String> = match &ids {
            Value::List(list) => list.iter().map(text).collect(),
            _ => Vec::new(),
        };
        async move { crate::plugins::preview::refuse("account.onboarded")?; a.onboarded(ids).await.map_err(String::from) }
    });
    let a = auth;
    host.async_action("account.invite", move |args| {
        let a = a.clone();
        let id = args.field("organization_id").map(text).unwrap_or_default();
        let emails: Vec<String> = match args.field("emails") {
            Some(Value::List(list)) => list.iter().map(text).collect(),
            _ => Vec::new(),
        };
        async move { crate::plugins::preview::refuse("account.invite")?; a.invite(id, emails).await.map_err(String::from) }
    });
}

/// A call to the edge as the account, for the host's other calls (Settings'
/// profile and organization, src/settings.rs): its status and body, or the
/// outcome word (offline, signed_out, failed).
/// Who is signed in, and the organization the session works in (none while
/// choosing one): what a conversation link's workspace is hashed from.
pub(crate) fn signed_in() -> Option<(String, Option<String>)> {
    AUTH.with(|a| match &*a.session.borrow() {
        Session::SignedIn { user, org } => Some((user.id.clone(), org.clone())),
        _ => None,
    })
}

pub(crate) async fn edge(method: &'static str, path: String, body: Option<Json>) -> Result<(u16, Json), Outcome> {
    let auth = AUTH.with(Rc::clone);
    auth.authed(method, path, body).await
}

/// The same with a body of raw bytes (a photo), as `content_type`.
pub(crate) async fn edge_bytes(method: &'static str, path: String, bytes: Vec<u8>, content_type: &'static str) -> Result<(u16, Json), Outcome> {
    let auth = AUTH.with(Rc::clone);
    auth.authed_bytes(method, path, bytes, content_type).await
}

/// `encode`, for the host's other calls' paths.
pub(crate) fn encode_path(value: &str) -> String {
    encode(value)
}

/// Whether the engine keeps a session, without reading it: a launch signed
/// in skips the intro.
pub fn session_exists() -> bool {
    let account = endpoint();
    engine::manager::Config::from_env(&account.edge, &account.client, account.port)
        .is_ok_and(|config| config.data_dir.join("session.json").is_file())
}

impl Auth {
    fn new() -> Self {
        let endpoint = endpoint();
        eprintln!(
            "zeron-zgx: account: {} ({}, callback port {}), kept by the engine",
            endpoint.name, endpoint.edge, endpoint.port
        );
        Self {
            endpoint,
            accounts: RefCell::default(),
            session: RefCell::new(Session::Unknown),
            waiting: Cell::new(false),
            failure: Cell::new(None),
            gave_up: Cell::new(false),
            access: RefCell::default(),
            generation: Cell::new(0),
            signed_out_here: Cell::new(false),
            watch: RefCell::default(),
            started: Cell::new(false),
            set_up_waiters: RefCell::default(),
        }
    }

    fn value(&self) -> Value {
        let session = self.session.borrow();
        let state = match &*session {
            Session::SignedIn { .. } => "signed_in",
            _ if self.waiting.get() => "waiting",
            Session::Unknown if !self.gave_up.get() => "restoring",
            _ => "signed_out",
        };
        let user = match &*session {
            Session::SignedIn { user, .. } => Value::record([
                ("id", Value::from(user.id.as_str())),
                ("email", Value::from(user.email.as_str())),
                ("first_name", user.first_name.as_deref().map_or(Value::Nil, Value::from)),
            ]),
            _ => Value::Nil,
        };
        Value::record([
            ("state", Value::from(state)),
            ("user", user),
            ("failure", self.failure.get().map_or(Value::Nil, Value::from)),
            ("env", Value::from(self.endpoint.name)),
            // The organization the session works in (Settings' own).
            (
                "organization_id",
                match &*session {
                    Session::SignedIn { org: Some(org), .. } => Value::from(org.as_str()),
                    _ => Value::Nil,
                },
            ),
        ])
    }

    fn publish(&self) {
        let value = self.value();
        for account in self.accounts.borrow().iter() {
            account.set(value.clone());
        }
    }

    fn signed_in(&self) -> bool {
        matches!(&*self.session.borrow(), Session::SignedIn { .. })
    }

    /// Follows the engine's session on every connection. A dropped
    /// connection changes nothing: the next one says where it stands.
    fn follow(self: Rc<Self>) {
        let weak = Rc::downgrade(&self);
        engine::get().on_connect(move |rpc| {
            let Some(auth) = weak.upgrade() else { return };
            let heard = Rc::downgrade(&auth);
            let watch = rpc.watch("AuthStatus", json!({}), move |item| {
                if let Some(auth) = heard.upgrade() {
                    auth.heard(session_from(&item));
                }
            });
            *auth.watch.borrow_mut() = Some(watch);
        });
    }

    fn heard(&self, session: Session) {
        let was_signed_in = self.signed_in();
        let changed = *self.session.borrow() != session;
        if changed {
            // A new session or organization: its bearer is another.
            self.access.borrow_mut().take();
        }
        let now_signed_in = matches!(session, Session::SignedIn { .. });
        let now_set_up = matches!(session, Session::SignedIn { org: Some(_), .. });
        *self.session.borrow_mut() = session;
        // Setup finished (an organization chosen): what waited for it asks
        // again (`after_setup`).
        if now_set_up {
            for waiter in std::mem::take(&mut *self.set_up_waiters.borrow_mut()) {
                waiter.send(());
            }
        }
        if now_signed_in {
            self.failure.set(None);
        } else if was_signed_in && self.failure.get().is_none() && !self.signed_out_here.get() {
            // Signed out, not by asking here: the session was refused.
            eprintln!("zeron-zgx: account: the session ended");
            self.failure.set(Some("expired"));
        }
        engine::get().session(now_signed_in);
        if changed {
            self.publish();
        }
    }

    /// The launch's wait for the engine: none in time, signed out (the
    /// engine's session shows as soon as it answers).
    async fn restore_deadline(self: Rc<Self>) {
        zgx::timer::sleep(RESTORE_WAIT).await;
        if *self.session.borrow() == Session::Unknown {
            eprintln!("zeron-zgx: account: no answer from the engine yet; shown signed out");
            self.gave_up.set(true);
            self.publish();
        }
    }

    fn next_generation(&self) -> u64 {
        self.generation.set(self.generation.get() + 1);
        self.generation.get()
    }

    /// The engine's sign-in page, opened, then the engine's session awaited.
    async fn sign_in(self: Rc<Self>, create: bool) -> Result<Value, Outcome> {
        if self.signed_in() {
            return Ok(self.value());
        }
        let generation = self.next_generation();
        self.waiting.set(true);
        self.failure.set(None);
        self.publish();
        let result = self.browser_sign_in(create, generation).await;
        if self.generation.get() != generation {
            // Cancelled or signed out meanwhile: that already said so.
            return Err("cancelled");
        }
        self.waiting.set(false);
        match result {
            Ok(()) => {
                self.signed_out_here.set(false);
                self.publish();
                eprintln!("zeron-zgx: account: signed in");
                Ok(self.value())
            }
            Err(outcome) => {
                eprintln!("zeron-zgx: account: sign-in didn't finish ({outcome})");
                self.failure.set(Some(outcome));
                self.publish();
                Err(outcome)
            }
        }
    }

    /// `create` opens on making an account and asks for it afresh (a
    /// browser still signed into someone would otherwise hand back that
    /// account, unasked); signing in may reuse it, unless someone signed out
    /// here.
    async fn browser_sign_in(&self, create: bool, generation: u64) -> Result<(), Outcome> {
        let params = if create {
            json!({ "screenHint": "sign-up", "fresh": true })
        } else {
            json!({ "screenHint": "sign-in", "fresh": self.signed_out_here.get() })
        };
        let answer = engine::get()
            .call("SignIn", params)
            .await
            .map_err(|error| {
                eprintln!("zeron-zgx: account: the engine didn't start a sign-in: {error}");
                engine::outcome(&error)
            })?;
        // Empty: the engine has no sign-in to do (development).
        if let Some(url) = answer["url"].as_str().filter(|url| !url.is_empty()) {
            if !url.starts_with("https://") {
                return Err("failed");
            }
            if let Err(error) = open_sign_in(url) {
                eprintln!("zeron-zgx: account: the browser didn't open: {error}");
                return Err("failed");
            }
        }
        let until = Instant::now() + SIGN_IN_WAIT;
        loop {
            if self.generation.get() != generation {
                return Err("cancelled");
            }
            if self.signed_in() {
                return Ok(());
            }
            if Instant::now() >= until {
                return Err("timed_out");
            }
            zgx::timer::sleep(Duration::from_millis(200)).await;
        }
    }

    fn cancel(&self) {
        if !self.waiting.get() {
            return;
        }
        self.next_generation();
        self.waiting.set(false);
        self.failure.set(Some("cancelled"));
        self.publish();
    }

    /// Signed out: the engine's session ended (the engine forgets it), as
    /// asked here.
    async fn sign_out(&self) {
        self.next_generation();
        self.waiting.set(false);
        self.signed_out_here.set(true);
        self.access.borrow_mut().take();
        if let Err(error) = engine::get().call("SignOut", json!({})).await {
            eprintln!("zeron-zgx: account: the engine didn't sign out: {error}");
        }
        *self.session.borrow_mut() = Session::SignedOut;
        self.failure.set(None);
        engine::get().session(false);
        self.publish();
        eprintln!("zeron-zgx: account: signed out");
    }

    /// The engine's bearer, kept until it nears its end.
    async fn bearer(&self) -> Result<String, Outcome> {
        if let Some(access) = &*self.access.borrow()
            && access.expires - now() > TOKEN_SLACK
        {
            return Ok(access.token.clone());
        }
        let answer = engine::get()
            .call("AccessToken", json!({}))
            .await
            .map_err(|error| engine::outcome(&error))?;
        let token = answer["token"].as_str().ok_or("failed")?.to_string();
        // No expiry said: used once, asked for again next time.
        let expires = answer["expiresAt"].as_i64().unwrap_or_else(now);
        *self.access.borrow_mut() = Some(Access {
            token: token.clone(),
            expires,
        });
        Ok(token)
    }

    /// A call to the edge as the account: asked again once with a fresh
    /// bearer if the token was refused.
    async fn authed(
        &self,
        method: &'static str,
        path: String,
        body: Option<Json>,
    ) -> Result<(u16, Json), Outcome> {
        let url = format!("{}{path}", self.endpoint.edge);
        for attempt in 0..2 {
            let bearer = self.bearer().await?;
            let (url, body) = (url.clone(), body.clone());
            let answer = off_thread(move || request(method, &url, Some(&bearer), body))
                .await
                .unwrap_or(Http::Offline);
            match answer {
                Http::Ok(json) => return Ok((200, json)),
                Http::Status(401, _) if attempt == 0 => {
                    self.access.borrow_mut().take();
                }
                Http::Status(401, _) => return Err("signed_out"),
                Http::Status(status, json) => return Ok((status, json)),
                Http::Offline => return Err("offline"),
            }
        }
        Err("failed")
    }

    /// `authed`, with a body of raw bytes.
    async fn authed_bytes(
        &self,
        method: &'static str,
        path: String,
        bytes: Vec<u8>,
        content_type: &'static str,
    ) -> Result<(u16, Json), Outcome> {
        let url = format!("{}{path}", self.endpoint.edge);
        let bytes = std::sync::Arc::new(bytes);
        for attempt in 0..2 {
            let bearer = self.bearer().await?;
            let (url, bytes) = (url.clone(), bytes.clone());
            let answer = off_thread(move || request_bytes(method, &url, &bearer, &bytes, content_type))
                .await
                .unwrap_or(Http::Offline);
            match answer {
                Http::Ok(json) => return Ok((200, json)),
                Http::Status(401, _) if attempt == 0 => {
                    self.access.borrow_mut().take();
                }
                Http::Status(401, _) => return Err("signed_out"),
                Http::Status(status, json) => return Ok((status, json)),
                Http::Offline => return Err("offline"),
            }
        }
        Err("failed")
    }

    async fn username(&self) -> Result<Value, Outcome> {
        match self.authed("GET", "/username".into(), None).await? {
            (200, body) => Ok(body["username"].as_str().map_or(Value::Nil, Value::from)),
            _ => Err("failed"),
        }
    }

    /// The onboarding pages the account has finished (the edge's
    /// `GET /onboarding`): their ids, a list.
    async fn onboarding(&self) -> Result<Value, Outcome> {
        match self.authed("GET", "/onboarding".into(), None).await? {
            (200, body) => Ok(done_ids(&body)),
            _ => Err("failed"),
        }
    }

    /// Records onboarding pages `ids` as finished (`POST /onboarding`):
    /// every finished id, as GET answers.
    async fn onboarded(&self, ids: Vec<String>) -> Result<Value, Outcome> {
        let body = json!({ "done": ids });
        match self.authed("POST", "/onboarding".into(), Some(body)).await? {
            (200, body) => Ok(done_ids(&body)),
            (400, _) => Err("invalid"),
            _ => Err("failed"),
        }
    }

    async fn check_username(&self, name: String) -> Result<Value, Outcome> {
        let path = format!("/username/check?username={}", encode(&name));
        match self.authed("GET", path, None).await? {
            (200, body) => Ok(Value::record([
                ("available", Value::Bool(body["available"].as_bool() == Some(true))),
                ("reason", body["reason"].as_str().map_or(Value::Nil, Value::from)),
            ])),
            (429, _) => Err("rate_limited"),
            _ => Err("failed"),
        }
    }

    async fn claim_username(&self, name: String) -> Result<Value, Outcome> {
        let body = json!({ "username": name });
        match self.authed("POST", "/username".into(), Some(body)).await? {
            (200, card) => Ok(card_value(&card)),
            (status, body) => Err(match (status, body["error"].as_str()) {
                (400, _) => "invalid",
                (403, Some("reserved_username")) => "reserved",
                (409, Some("username_taken")) => "taken",
                (409, Some("already_claimed")) => "already_claimed",
                (409, Some("not_ready")) => "not_ready",
                (429, _) => "rate_limited",
                _ => "failed",
            }),
        }
    }

    /// The organizations the account is in (the edge's `GET /auth/orgs`):
    /// `{ id, organization_id, name, role }` each (`role` "admin" for the
    /// account's own).
    async fn orgs(&self) -> Result<Value, Outcome> {
        match self.authed("GET", "/auth/orgs".into(), None).await? {
            (200, body) => {
                let orgs: Vec<Value> = body["orgs"]
                    .as_array()
                    .map(|orgs| {
                        orgs.iter()
                            .map(|o| {
                                let str = |key: &str| o[key].as_str().map_or(Value::Nil, Value::from);
                                // (A slug, or WorkOS's `{ slug }`.)
                                let role = o["role"].as_str().or_else(|| o["role"]["slug"].as_str());
                                Value::record([
                                    ("id", str("id")),
                                    ("organization_id", str("organizationId")),
                                    ("name", str("name")),
                                    ("role", role.map_or(Value::Nil, Value::from)),
                                ])
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                Ok(Value::List(orgs.into()))
            }
            _ => Err("failed"),
        }
    }

    /// A new organization, the account its first (admin) member: its
    /// `organization_id`. An account makes one organization of its own: one
    /// that already has it is answered with that one (`existing`), to work
    /// in, not an error. Working in it is `choose_org`'s.
    async fn create_org(&self, name: String) -> Result<Value, Outcome> {
        let body = json!({ "name": name });
        match self.authed("POST", "/auth/orgs".into(), Some(body)).await? {
            (200, body) => Ok(Value::record([(
                "organization_id",
                body["organizationId"].as_str().map_or(Value::Nil, Value::from),
            )])),
            (409, body) if body["error"].as_str() == Some("org_exists") => match body["organizationId"].as_str() {
                Some(id) => Ok(Value::record([
                    ("organization_id", Value::from(id)),
                    ("existing", Value::Bool(true)),
                ])),
                None => Err("failed"),
            },
            (400, _) => Err("invalid"),
            (429, _) => Err("rate_limited"),
            _ => Err("failed"),
        }
    }

    /// Works in organization `id` from now on: the engine's session scoped
    /// to it (`SelectOrg`), so its sync, Cloud and GitHub are that
    /// organization's. Refused or failed, the session stays as it was.
    async fn choose_org(&self, id: String) -> Result<Value, Outcome> {
        match engine::get()
            .call("SelectOrg", json!({ "organizationId": id }))
            .await
        {
            Ok(_) => {
                self.access.borrow_mut().take();
                Ok(Value::record([("organization_id", Value::from(id.as_str()))]))
            }
            Err(error) => {
                let lower = error.to_ascii_lowercase();
                if lower.contains("could not switch") || lower.contains("no longer be a member") {
                    Err("refused")
                } else {
                    Err(engine::outcome(&error))
                }
            }
        }
    }

    /// Invites `emails` to organization `id` as members (the edge's
    /// `POST /auth/orgs/{id}/invitations`; WorkOS sends the emails):
    /// `{ sent = { email }, failed = { { email, error } } }`, an address's
    /// error invalid, exists or failed. Only its admins can.
    async fn invite(&self, id: String, emails: Vec<String>) -> Result<Value, Outcome> {
        let body = json!({ "emails": emails });
        match self.authed("POST", format!("/auth/orgs/{}/invitations", encode(&id)), Some(body)).await? {
            (200, body) => {
                let sent: Vec<Value> = body["sent"]
                    .as_array()
                    .map(|l| l.iter().filter_map(|e| e.as_str()).map(Value::from).collect())
                    .unwrap_or_default();
                let failed: Vec<Value> = body["failed"]
                    .as_array()
                    .map(|l| {
                        l.iter()
                            .map(|f| {
                                let str = |key: &str| f[key].as_str().map_or(Value::Nil, Value::from);
                                Value::record([("email", str("email")), ("error", str("error"))])
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                Ok(Value::record([
                    ("sent", Value::List(sent.into())),
                    ("failed", Value::List(failed.into())),
                ]))
            }
            (400, _) => Err("invalid"),
            (403, _) => Err("refused"),
            (429, _) => Err("rate_limited"),
            _ => Err("failed"),
        }
    }

    /// Anyone's card: public, no account needed.
    async fn card(&self, name: String) -> Result<Value, Outcome> {
        let url = format!("{}/cards/{}", self.endpoint.edge, encode(&name));
        match off_thread(move || request("GET", &url, None, None))
            .await
            .unwrap_or(Http::Offline)
        {
            Http::Ok(card) => Ok(card_value(&card)),
            Http::Status(404, _) => Ok(Value::Nil),
            Http::Status(..) => Err("failed"),
            Http::Offline => Err("offline"),
        }
    }
}

/// The engine's `AuthStatus` (`{state: signedOut | needsOrganization |
/// signedIn, user?, orgId?}`) as a session. Choosing an organization counts
/// as signed in: setup's next page is where one is chosen.
fn session_from(item: &Json) -> Session {
    let user = || {
        let u = &item["user"];
        Some(User {
            id: u["id"].as_str()?.into(),
            email: u["email"].as_str()?.into(),
            // The engine keeps the whole name; setup greets by the first.
            first_name: u["name"]
                .as_str()
                .and_then(|name| name.split_whitespace().next())
                .map(Into::into),
        })
    };
    match item["state"].as_str() {
        Some("signedIn") | Some("needsOrganization") => match user() {
            Some(user) => Session::SignedIn {
                user,
                org: item["orgId"].as_str().map(Into::into),
            },
            None => Session::SignedOut,
        },
        _ => Session::SignedOut,
    }
}

/// A card as scripts take it (the edge's `PublicCard`).
fn card_value(card: &Json) -> Value {
    let str = |key: &str| card[key].as_str().map_or(Value::Nil, Value::from);
    let num = |key: &str| card[key].as_f64().map_or(Value::Nil, Value::from);
    let badges: Vec<Value> = card["badges"]
        .as_array()
        .map(|badges| {
            badges
                .iter()
                .map(|b| {
                    Value::record([
                        ("id", b["id"].as_str().map_or(Value::Nil, Value::from)),
                        ("name", b["name"].as_str().map_or(Value::Nil, Value::from)),
                        ("icon", b["icon"].as_str().map_or(Value::Nil, Value::from)),
                    ])
                })
                .collect()
        })
        .unwrap_or_default();
    Value::record([
        ("username", str("username")),
        ("first_name", str("firstName")),
        ("number", num("userNumber")),
        ("joined_on", str("joinedOn")),
        ("badges", Value::List(badges.into())),
        ("scene", num("scene")),
        ("holo", str("holo")),
        ("sky", num("sky")),
    ])
}

/// Opens a page the engine handed us in the browser. `ZERON_ZGX_PRIVATE_SIGNIN=1`
/// (development) opens it in a private Chrome window instead, so the
/// browser's own sessions (a Google account it signs into by itself) can't
/// pick the account: every sign-in asks.
pub fn open_sign_in(url: &str) -> std::io::Result<()> {
    if no_browser(url) {
        return Ok(());
    }
    if cfg!(target_os = "macos") && std::env::var_os("ZERON_ZGX_PRIVATE_SIGNIN").is_some() {
        return std::process::Command::new("/usr/bin/open")
            .args(["-na", "Google Chrome", "--args", "--incognito", url])
            .spawn()
            .map(|_| ());
    }
    open::that_detached(url)
}

/// `ZERON_ZGX_NO_BROWSER=1` (development: trying a flow without a browser
/// window opening on this Mac): the page isn't opened, only its host said.
/// Set to a file's path, the page goes in that file (owner-only), for a
/// test rig's own browser to open.
pub fn no_browser(url: &str) -> bool {
    let Some(setting) = crate::dev::smoke_switch("ZERON_ZGX_NO_BROWSER") else {
        return false;
    };
    let host = url.split("://").nth(1).and_then(|rest| rest.split(['/', '?']).next()).unwrap_or("");
    eprintln!("zeron-zgx: (not opened: a page on {host})");
    if setting != "1" {
        #[cfg(unix)]
        {
            use std::{io::Write, os::unix::fs::OpenOptionsExt};
            let written = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&setting)
                .and_then(|mut file| file.write_all(url.as_bytes()));
            if let Err(error) = written {
                eprintln!("zeron-zgx: the page couldn't be written for the rig: {error}");
            }
        }
    }
    true
}

/// `{ done: { id: time } }`'s ids, as a list.
fn done_ids(body: &Json) -> Value {
    let ids: Vec<Value> = body["done"]
        .as_object()
        .map(|done| done.keys().map(|id| Value::from(id.as_str())).collect())
        .unwrap_or_default();
    Value::List(ids.into())
}

pub fn text(value: &Value) -> String {
    match value {
        Value::Str(s) => s.to_string(),
        _ => String::new(),
    }
}

// -- the edge ---------------------------------------------------------------

enum Http {
    Ok(Json),
    Status(u16, Json),
    Offline,
}

/// One request, blocking: for a worker thread.
fn request(method: &str, url: &str, bearer: Option<&str>, body: Option<Json>) -> Http {
    let agent = ureq::AgentBuilder::new()
        .timeout(HTTP_TIMEOUT)
        .user_agent("zeron-zgx")
        .build();
    let mut req = agent.request(method, url).set("accept", "application/json");
    if let Some(token) = bearer {
        req = req.set("authorization", &format!("Bearer {token}"));
    }
    let answer = match body {
        Some(body) => req.send_json(body),
        None => req.call(),
    };
    match answer {
        Ok(res) => Http::Ok(res.into_json().unwrap_or(Json::Null)),
        Err(ureq::Error::Status(status, res)) => Http::Status(status, res.into_json().unwrap_or(Json::Null)),
        Err(ureq::Error::Transport(_)) => Http::Offline,
    }
}

/// One request with a body of raw bytes, blocking: for a worker thread.
fn request_bytes(method: &str, url: &str, bearer: &str, bytes: &[u8], content_type: &str) -> Http {
    let agent = ureq::AgentBuilder::new()
        .timeout(HTTP_TIMEOUT)
        .user_agent("zeron-zgx")
        .build();
    let answer = agent
        .request(method, url)
        .set("accept", "application/json")
        .set("authorization", &format!("Bearer {bearer}"))
        .set("content-type", content_type)
        .send_bytes(bytes);
    match answer {
        Ok(res) => Http::Ok(res.into_json().unwrap_or(Json::Null)),
        Err(ureq::Error::Status(status, res)) => Http::Status(status, res.into_json().unwrap_or(Json::Null)),
        Err(ureq::Error::Transport(_)) => Http::Offline,
    }
}

/// `work` on a thread of its own, its answer awaited on the main thread
/// (None only if it panicked).
async fn off_thread<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (tx, mut rx) = zgx::task::latest();
    std::thread::spawn(move || {
        tx.send(work());
    });
    rx.recv().await
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A future run to its end on this thread (its waker unparks it).
    fn block_on<T>(future: impl std::future::Future<Output = T>) -> T {
        struct Unpark(std::thread::Thread);
        impl std::task::Wake for Unpark {
            fn wake(self: std::sync::Arc<Self>) {
                self.0.unpark();
            }
        }
        let waker = std::task::Waker::from(std::sync::Arc::new(Unpark(std::thread::current())));
        let mut cx = std::task::Context::from_waker(&waker);
        let mut future = std::pin::pin!(future);
        loop {
            if let std::task::Poll::Ready(v) = future.as_mut().poll(&mut cx) {
                return v;
            }
            std::thread::park_timeout(std::time::Duration::from_millis(50));
        }
    }

    /// While the account's setup isn't finished the engine answers template
    /// (and plugin) calls `setup_incomplete`: the call waits for setup, then
    /// asks again, and gets its answer; any other error is the answer.
    #[test]
    fn a_call_answered_setup_incomplete_asks_again_once_set_up() {
        let asked = Cell::new(0);
        let waited = Cell::new(0);
        let got = block_on(retry_after(
            || {
                asked.set(asked.get() + 1);
                let n = asked.get();
                async move { if n == 1 { Err::<&str, _>("capability: setup_incomplete".to_string()) } else { Ok("templates") } }
            },
            || {
                waited.set(waited.get() + 1);
                async {}
            },
        ));
        assert_eq!((got, asked.get(), waited.get()), (Ok("templates"), 2, 1));
        // Another error isn't retried.
        asked.set(0);
        let got = block_on(retry_after(
            || {
                asked.set(asked.get() + 1);
                async { Err::<(), _>("offline".to_string()) }
            },
            || async {},
        ));
        assert_eq!((got, asked.get()), (Err("offline".to_string()), 1));
        // Setup never finishing: given up on after a few, the error kept.
        asked.set(0);
        let got = block_on(retry_after(
            || {
                asked.set(asked.get() + 1);
                async { Err::<(), _>(SETUP_INCOMPLETE.to_string()) }
            },
            || async {},
        ));
        assert!(got.is_err_and(|e| setup_incomplete(&e)) && asked.get() == SETUP_RETRIES + 1);
    }

    /// `set_up` resolves once AuthStatus names an organization.
    #[test]
    fn set_up_resolves_when_an_organization_is_chosen() {
        let auth = AUTH.with(Rc::clone);
        auth.heard(session_from(&json!({ "state": "needsOrganization", "user": { "id": "u", "email": "a@b.c" } })));
        assert!(!is_set_up());
        let (tell, mut heard) = zgx::task::latest::<()>();
        auth.set_up_waiters.borrow_mut().push(tell);
        auth.heard(session_from(&json!({ "state": "signedIn", "orgId": "org_1", "user": { "id": "u", "email": "a@b.c" } })));
        assert!(is_set_up());
        assert_eq!(block_on(heard.recv()), Some(()));
        assert!(auth.set_up_waiters.borrow().is_empty());
        block_on(set_up());
    }

    #[test]
    fn the_engines_auth_status_reads_as_a_session() {
        assert!(matches!(session_from(&json!({ "state": "signedOut" })), Session::SignedOut));
        let user = json!({ "id": "u1", "email": "a@example.com", "name": "Ada Lovelace" });
        match session_from(&json!({ "state": "signedIn", "user": user, "orgId": "org_1" })) {
            Session::SignedIn { user, org } => {
                assert_eq!(user.first_name.as_deref(), Some("Ada"));
                assert_eq!(org.as_deref(), Some("org_1"));
            }
            _ => panic!("signed in"),
        }
        // Choosing an organization is signed in, with none yet.
        assert!(matches!(
            session_from(&json!({ "state": "needsOrganization", "user": user })),
            Session::SignedIn { org: None, .. }
        ));
        // A user the engine didn't send can't be signed in.
        assert!(matches!(session_from(&json!({ "state": "signedIn" })), Session::SignedOut));
    }
}
