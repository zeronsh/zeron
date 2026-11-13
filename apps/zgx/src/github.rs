//! GitHub for Cloud agents, for the scripts: the account's GitHub
//! connection (held by the engine's vault) and GitHub's device flow to make
//! one, as the shipping app's Settings → Cloud runs them (crates/ui/src/
//! settings/cloud.rs). Cloud agents work on machines of their own; GitHub
//! is how they clone and push, so a connection counts once it's allowed on
//! the account's Cloud. Cloud is part of every signed-in account: one not
//! set up yet is set up here (`CloudEnable`, which starts no machine), as
//! the shipping app does.
//!
//! What scripts see, `github`:
//!   { phase = "checking" | "unavailable" | "idle" | "starting" | "waiting"
//!            | "failed" | "connected",
//!     code?, account?, repos = { "owner/name" }, failure?,
//!     accounts = { { slot, login, avatar?, status = "connected"
//!                    | "needs_reconnect" } },
//!     already? }
//! (`failure` is what went wrong, said for the screen: a code that expired,
//! turned down, or GitHub out of reach. `accounts`: every login connected for
//! Cloud, empty from an engine or vault from before several logins, which
//! leaves `account` to name the one there is. `already`: the login just
//! approved was connected before, so nothing was added.)
//! and call:
//!   github.load (an action)       where the account stands
//!   github.connect (an action)    asks GitHub for a code, then waits for it
//!                                 to be approved there
//!   github.cancel (an action)     stops waiting
//!   github.open (an action)       GitHub's page for entering the code
//!   github.choose (an action)     GitHub's page for the repositories Cloud
//!                                 can open
//!   github.disconnect (an action) GitHub no longer used by Cloud
//!   github.forget ({ slot })      one login disconnected, the others kept

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde_json::{Value as Json, json};
use zgx_desktop::WindowContext;
use zgx_script::{ScriptHost, Shared, Value};

use crate::engine;

#[derive(Clone, Copy, PartialEq)]
enum Phase {
    Checking,
    Unavailable,
    Idle,
    Starting,
    Waiting,
    Failed,
    Connected,
}

/// A GitHub login connected for Cloud.
#[derive(Clone, Debug, PartialEq)]
struct Account {
    slot: String,
    login: String,
    avatar: Option<String>,
    connected: bool,
    devices: Vec<String>,
}

/// The logins from `GithubAccounts`.
fn accounts_from(answer: &Json) -> Vec<Account> {
    answer["accounts"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|a| {
            Some(Account {
                slot: a["slot"].as_str()?.into(),
                login: a["account"].as_str().unwrap_or_default().into(),
                avatar: a["avatarUrl"].as_str().filter(|u| u.starts_with("https://")).map(Into::into),
                connected: a["status"].as_str() != Some("needsReconnect"),
                devices: a["authorizedDevices"]
                    .as_array()
                    .map(|d| d.iter().filter_map(|d| d.as_str().map(Into::into)).collect())
                    .unwrap_or_default(),
            })
        })
        .collect()
}

/// Whether GitHub works for Cloud: some login does. Without a list (an engine
/// or vault from before several logins), the one connection the status names.
fn usable(accounts: &[Account], connection: Option<&Json>, cloud: &str) -> bool {
    if accounts.is_empty() {
        return connection.is_some_and(|c| {
            c["status"].as_str() != Some("needsReconnect")
                && c["authorizedDevices"]
                    .as_array()
                    .is_some_and(|d| d.iter().any(|d| d.as_str() == Some(cloud)))
        });
    }
    accounts.iter().any(|a| a.connected && a.devices.iter().any(|d| d == cloud))
}

struct GitHub {
    shared: RefCell<Vec<Shared>>,
    phase: Cell<Phase>,
    code: RefCell<Option<String>>,
    verification: RefCell<Option<String>>,
    account: RefCell<Option<String>>,
    accounts: RefCell<Vec<Account>>,
    /// The login just approved, when it was connected already.
    already: RefCell<Option<String>>,
    repos: RefCell<Vec<String>>,
    failure: RefCell<Option<String>>,
    /// The logical Cloud device, once Cloud is on.
    cloud: RefCell<Option<String>>,
    /// The vault's last answer.
    vault: RefCell<Json>,
    /// Moves on every load, connect and cancel: an older one stops.
    generation: Cell<u64>,
}

thread_local! {
    static GITHUB: Rc<GitHub> = Rc::new(GitHub::new());
}

pub fn install(_cx: &mut WindowContext, host: &ScriptHost) {
    let github = GITHUB.with(Rc::clone);
    github.shared.borrow_mut().push(host.shared("github", github.value()));

    let g = github.clone();
    host.action("github.load", move |_| {
        let g = g.clone();
        engine::get().spawn(async move { g.load().await });
    });
    let g = github.clone();
    host.action("github.connect", move |_| {
        let g = g.clone();
        engine::get().spawn(async move { g.connect().await });
    });
    let g = github.clone();
    host.action("github.cancel", move |_| {
        if g.phase.get() == Phase::Waiting || g.phase.get() == Phase::Starting {
            g.next_generation();
            let usable = usable(&g.accounts.borrow(), None, &g.cloud.borrow().clone().unwrap_or_default());
            g.set(if usable { Phase::Connected } else { Phase::Idle });
        }
    });
    let g = github.clone();
    host.action("github.open", move |_| {
        if let Some(url) = g.verification.borrow().as_deref() {
            open_page(url);
        }
    });
    let g = github.clone();
    host.action("github.choose", move |_| {
        let vault = g.vault.borrow();
        if let Some(url) = vault["githubInstallUrl"].as_str() {
            open_page(url);
        }
    });
    let g = github.clone();
    host.action("github.disconnect", move |_| {
        start_disconnect(g.clone());
    });
    let g = github;
    host.action("github.forget", move |args| {
        if let Some(slot) = args.field("slot").map(crate::auth::text).filter(|s| !s.is_empty()) {
            start_forget(g.clone(), slot);
        }
    });
}

/// One login disconnected, unless an unkept preview asks, or a connection is
/// under way (it holds the state of this): whether it started.
fn start_forget(g: Rc<GitHub>, slot: String) -> bool {
    if matches!(g.phase.get(), Phase::Starting | Phase::Waiting) || crate::plugins::preview::refuse_aloud("github.forget") {
        return false;
    }
    engine::get().spawn(async move { g.forget(slot).await });
    true
}

/// GitHub disconnected, unless an unkept preview asks: whether it started.
fn start_disconnect(g: Rc<GitHub>) -> bool {
    if crate::plugins::preview::refuse_aloud("github.disconnect") {
        return false;
    }
    engine::get().spawn(async move { g.disconnect().await });
    true
}

pub(crate) fn open_page(url: &str) {
    if url.starts_with("https://")
        && !url.chars().any(char::is_control)
        && !crate::auth::no_browser(url)
        && let Err(error) = open::that_detached(url)
    {
        eprintln!("zeron-zgx: github: the browser didn't open: {error}");
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

impl GitHub {
    fn new() -> Self {
        Self {
            shared: RefCell::default(),
            phase: Cell::new(Phase::Checking),
            code: RefCell::default(),
            verification: RefCell::default(),
            account: RefCell::default(),
            accounts: RefCell::default(),
            already: RefCell::default(),
            repos: RefCell::default(),
            failure: RefCell::default(),
            cloud: RefCell::default(),
            vault: RefCell::new(Json::Null),
            generation: Cell::new(0),
        }
    }

    fn value(&self) -> Value {
        let phase = match self.phase.get() {
            Phase::Checking => "checking",
            Phase::Unavailable => "unavailable",
            Phase::Idle => "idle",
            Phase::Starting => "starting",
            Phase::Waiting => "waiting",
            Phase::Failed => "failed",
            Phase::Connected => "connected",
        };
        let text = |v: &RefCell<Option<String>>| v.borrow().as_deref().map_or(Value::Nil, Value::from);
        let repos: Vec<Value> = self.repos.borrow().iter().map(|r| Value::from(r.as_str())).collect();
        let accounts: Vec<Value> = self
            .accounts
            .borrow()
            .iter()
            .map(|a| {
                Value::record([
                    ("slot", Value::from(a.slot.as_str())),
                    ("login", Value::from(a.login.as_str())),
                    ("avatar", a.avatar.as_deref().map_or(Value::Nil, Value::from)),
                    ("status", Value::from(if a.connected { "connected" } else { "needs_reconnect" })),
                ])
            })
            .collect();
        Value::record([
            ("phase", Value::from(phase)),
            ("code", text(&self.code)),
            ("account", text(&self.account)),
            ("accounts", Value::List(accounts.into())),
            ("already", text(&self.already)),
            ("repos", Value::List(repos.into())),
            ("failure", text(&self.failure)),
        ])
    }

    fn publish(&self) {
        let value = self.value();
        for shared in self.shared.borrow().iter() {
            shared.set(value.clone());
        }
    }

    fn set(&self, phase: Phase) {
        self.phase.set(phase);
        self.publish();
    }

    fn fail(&self, why: &str) {
        *self.failure.borrow_mut() = Some(why.into());
        self.set(Phase::Failed);
    }

    fn next_generation(&self) -> u64 {
        self.generation.set(self.generation.get() + 1);
        self.generation.get()
    }

    /// Cloud's status (set up if it isn't yet), then the vault's GitHub.
    async fn load(self: Rc<Self>) {
        let generation = self.next_generation();
        if !matches!(self.phase.get(), Phase::Connected | Phase::Idle) {
            self.set(Phase::Checking);
        }
        let engine = engine::get();
        let mut status = match engine.call("CloudStatus", json!({})).await {
            Ok(status) => status,
            Err(error) => {
                eprintln!("zeron-zgx: github: Cloud's status unavailable: {error}");
                if self.generation.get() == generation {
                    self.unavailable("Zeron can't be reached right now. Skip for now.");
                }
                return;
            }
        };
        if self.generation.get() != generation {
            return;
        }
        if status["available"].as_bool() != Some(true) {
            self.unavailable("Cloud isn't available for this account yet. Skip for now.");
            return;
        }
        if status["state"].as_str() == Some("off") {
            eprintln!("zeron-zgx: github: setting up Cloud for the account");
            match engine.call("CloudEnable", json!({})).await {
                Ok(enabled) => status = enabled,
                Err(error) => {
                    eprintln!("zeron-zgx: github: Cloud didn't set up: {error}");
                    if self.generation.get() == generation {
                        self.unavailable("Cloud couldn't be set up. Skip for now.");
                    }
                    return;
                }
            }
        }
        let enabled = !matches!(status["state"].as_str(), Some("off" | "deleting"));
        let Some(cloud) = status["deviceId"].as_str().filter(|_| enabled) else {
            self.unavailable("Cloud is still setting up. Skip for now.");
            return;
        };
        *self.cloud.borrow_mut() = Some(cloud.into());
        self.refresh_vault(generation).await;
    }

    fn unavailable(&self, why: &str) {
        *self.failure.borrow_mut() = Some(why.into());
        self.set(Phase::Unavailable);
    }

    /// The vault's GitHub: connected for Cloud (its logins and repositories),
    /// or not.
    async fn refresh_vault(&self, generation: u64) {
        let vault = match engine::get().call("VaultStatus", json!({})).await {
            Ok(vault) => vault,
            Err(error) => {
                eprintln!("zeron-zgx: github: the vault didn't answer: {error}");
                if self.generation.get() == generation {
                    self.unavailable("Zeron can't be reached right now. Skip for now.");
                }
                return;
            }
        };
        if self.generation.get() != generation {
            return;
        }
        let cloud = self.cloud.borrow().clone().unwrap_or_default();
        let connection = vault["connections"]
            .as_array()
            .and_then(|c| c.iter().find(|c| c["provider"].as_str() == Some("github")))
            .cloned();
        *self.vault.borrow_mut() = vault;
        // (An engine or vault from before several logins doesn't answer: the
        // connection in the status stands for its one.)
        let accounts = match engine::get().call("GithubAccounts", json!({})).await {
            Ok(answer) => accounts_from(&answer),
            Err(error) => {
                eprintln!("zeron-zgx: github: no list of logins: {error}");
                Vec::new()
            }
        };
        if self.generation.get() != generation {
            return;
        }
        let connected = usable(&accounts, connection.as_ref(), &cloud);
        *self.accounts.borrow_mut() = accounts;
        if !connected {
            *self.account.borrow_mut() = None;
            self.repos.borrow_mut().clear();
            if self.phase.get() != Phase::Failed {
                self.set(Phase::Idle);
            }
            return;
        }
        *self.account.borrow_mut() = connection.and_then(|c| c["account"].as_str().map(Into::into));
        *self.failure.borrow_mut() = None;
        self.set(Phase::Connected);
        // The repositories it can see, newest push first (the vault calls
        // GitHub).
        match engine::get().call("ListGithubRepos", json!({})).await {
            Ok(answer) if self.generation.get() == generation => {
                let repos = answer["repos"]
                    .as_array()
                    .map(|r| r.iter().filter_map(|r| r["fullName"].as_str().map(Into::into)).collect())
                    .unwrap_or_default();
                *self.repos.borrow_mut() = repos;
                self.publish();
            }
            Ok(_) => {}
            Err(error) => eprintln!("zeron-zgx: github: repositories unavailable: {error}"),
        }
    }

    /// The device flow: a code from GitHub, shown; GitHub asked after until
    /// it's approved, turned down or expired.
    async fn connect(self: Rc<Self>) {
        let generation = self.next_generation();
        let Some(cloud) = self.cloud.borrow().clone() else {
            self.fail("Cloud isn't set up for this account yet.");
            return;
        };
        *self.failure.borrow_mut() = None;
        *self.code.borrow_mut() = None;
        *self.already.borrow_mut() = None;
        self.set(Phase::Starting);
        // Every device already allowed keeps its access; Cloud is added.
        let mut devices: Vec<String> = self.vault.borrow()["connections"]
            .as_array()
            .and_then(|c| c.iter().find(|c| c["provider"].as_str() == Some("github")))
            .and_then(|c| c["authorizedDevices"].as_array())
            .map(|d| d.iter().filter_map(|d| d.as_str().map(Into::into)).collect())
            .unwrap_or_default();
        if !devices.contains(&cloud) {
            devices.push(cloud);
        }
        let engine = engine::get();
        let flow = match engine
            .call("GithubConnectStart", json!({ "authorizedDevices": devices }))
            .await
        {
            Ok(flow) => flow,
            Err(error) => {
                eprintln!("zeron-zgx: github: no code from GitHub: {error}");
                if self.generation.get() == generation {
                    self.fail("GitHub couldn't be reached. Try again.");
                }
                return;
            }
        };
        if self.generation.get() != generation {
            return;
        }
        let (Some(flow_id), Some(code), Some(url)) = (
            flow["flowId"].as_str(),
            flow["userCode"].as_str(),
            flow["verificationUri"].as_str(),
        ) else {
            self.fail("GitHub couldn't be reached. Try again.");
            return;
        };
        let flow_id = flow_id.to_string();
        let interval = Duration::from_secs(flow["intervalSecs"].as_u64().unwrap_or(5).max(1));
        let expires_at = flow["expiresAt"].as_i64().unwrap_or(0);
        *self.code.borrow_mut() = Some(code.into());
        *self.verification.borrow_mut() = Some(url.into());
        self.set(Phase::Waiting);
        let mut failures = 0;
        loop {
            zgx::timer::sleep(interval).await;
            if self.generation.get() != generation {
                return;
            }
            if expires_at > 0 && now_ms() > expires_at {
                self.fail("The code expired before it was approved.");
                return;
            }
            match engine.call("GithubConnectPoll", json!({ "flowId": flow_id })).await {
                Ok(progress) => match progress["state"].as_str() {
                    Some("connected") => {
                        *self.code.borrow_mut() = None;
                        if let Some(account) = progress["account"].as_str() {
                            *self.account.borrow_mut() = Some(account.into());
                            if progress["alreadyConnected"].as_bool() == Some(true) {
                                *self.already.borrow_mut() = Some(account.into());
                            }
                        }
                        self.set(Phase::Connected);
                        self.refresh_vault(generation).await;
                        return;
                    }
                    Some("failed") => {
                        eprintln!(
                            "zeron-zgx: github: not connected: {}",
                            progress["error"].as_str().unwrap_or("no reason given")
                        );
                        if self.generation.get() == generation {
                            self.fail("GitHub didn't approve the connection. Try again.");
                        }
                        return;
                    }
                    _ => failures = 0,
                },
                // Ride out a blip; give up when GitHub stays out of reach.
                Err(error) => {
                    failures += 1;
                    if failures >= 3 {
                        eprintln!("zeron-zgx: github: lost the connection's progress: {error}");
                        if self.generation.get() == generation {
                            self.fail("GitHub couldn't be reached. Try again.");
                        }
                        return;
                    }
                }
            }
        }
    }

    /// One login disconnected; the others carry on. Its chats stay on it and
    /// say so until it's connected again.
    async fn forget(self: Rc<Self>, slot: String) {
        let generation = self.next_generation();
        *self.already.borrow_mut() = None;
        match engine::get().call("GithubAccountForget", json!({ "slot": slot })).await {
            Ok(_) => self.refresh_vault(generation).await,
            Err(error) => {
                eprintln!("zeron-zgx: github: didn't disconnect a login: {error}");
                if self.generation.get() == generation {
                    self.publish();
                }
            }
        }
    }

    /// GitHub no longer used by Cloud: forgotten, unless other devices still
    /// use it, which keep it.
    async fn disconnect(self: Rc<Self>) {
        let generation = self.next_generation();
        let cloud = self.cloud.borrow().clone().unwrap_or_default();
        let others: Vec<String> = self.vault.borrow()["connections"]
            .as_array()
            .and_then(|c| c.iter().find(|c| c["provider"].as_str() == Some("github")))
            .and_then(|c| c["authorizedDevices"].as_array())
            .map(|d| {
                d.iter()
                    .filter_map(|d| d.as_str())
                    .filter(|d| *d != cloud)
                    .map(Into::into)
                    .collect()
            })
            .unwrap_or_default();
        let (method, params) = if others.is_empty() {
            ("VaultDisconnect", json!({ "provider": "github" }))
        } else {
            ("VaultAuthorize", json!({ "provider": "github", "authorizedDevices": others }))
        };
        match engine::get().call(method, params).await {
            Ok(_) => self.refresh_vault(generation).await,
            Err(error) => {
                eprintln!("zeron-zgx: github: didn't disconnect: {error}");
                if self.generation.get() == generation {
                    self.publish();
                }
            }
        }
    }
}

#[cfg(test)]
mod disconnect_tests {
    use super::*;
    use crate::plugins::preview;

    /// An unkept preview can't disconnect GitHub; otherwise it goes ahead.
    #[test]
    fn a_preview_can_t_disconnect_github() {
        let g = GITHUB.with(Rc::clone);
        preview::set(Some(preview::Preview { id: "t".into(), name: "T".into() }));
        assert!(!start_disconnect(g.clone()));
        preview::set(None);
        assert!(start_disconnect(g));
    }

    #[test]
    fn logins_are_read_from_the_engine() {
        let accounts = accounts_from(&json!({ "accounts": [
            { "slot": "a1", "account": "work-me", "status": "connected", "authorizedDevices": ["cloud-1"],
              "avatarUrl": "https://github.com/work-me.png?size=64" },
            { "slot": "a2", "account": "home-me", "status": "needsReconnect", "avatarUrl": "http://not-this" },
            { "account": "no-slot" },
        ] }));
        assert_eq!(accounts.len(), 2);
        assert_eq!(accounts[0].login, "work-me");
        assert_eq!(accounts[0].avatar.as_deref(), Some("https://github.com/work-me.png?size=64"));
        assert!(accounts[0].connected);
        assert_eq!(accounts[1].avatar, None);
        assert!(!accounts[1].connected);
    }

    #[test]
    fn github_works_while_any_login_does() {
        let account = |connected: bool, device: &str| Account {
            slot: "s".into(),
            login: "l".into(),
            avatar: None,
            connected,
            devices: vec![device.into()],
        };
        assert!(usable(&[account(false, "cloud"), account(true, "cloud")], None, "cloud"));
        assert!(!usable(&[account(false, "cloud")], None, "cloud"));
        assert!(!usable(&[account(true, "laptop")], None, "cloud"));
        // Without a list, the one connection the status names.
        let one = json!({ "status": "connected", "authorizedDevices": ["cloud"] });
        assert!(usable(&[], Some(&one), "cloud"));
        assert!(!usable(&[], Some(&one), "other"));
        let dead = json!({ "status": "needsReconnect", "authorizedDevices": ["cloud"] });
        assert!(!usable(&[], Some(&dead), "cloud"));
        assert!(!usable(&[], None, "cloud"));
    }
}
