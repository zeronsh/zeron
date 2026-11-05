//! The ACP registry's agents, for Settings → Providers → Add an agent: what
//! the engine lists for the device Providers shows (`ListAcpRegistry`),
//! and adding, updating and removing one there. The engine fetches the
//! registry, installs and launches; this only carries its answers.
//!
//! What scripts see, `acp_registry`:
//!   { state = "looking" | "ready" | "unavailable", offline, target?,
//!     agents = { { id, harness, name, version, about, by, license?,
//!                  source?, website?, kind?, readiness, host?, checksum?,
//!                  built_in?, added?, adding, delisted, icon? } } }
//! (`kind` "binary" | "npx" | "uvx", none without a build for the device;
//! `readiness` "ready" | "needs-node" | "needs-uv" | "no-build"; `checksum`
//! true / false for a binary, nil for a package; `icon` the registry's
//! one-ink SVG.) Each agent carries all it shows, keyed by `id`.
//!
//!   acp_registry.load(refresh)   asks again (an action; `refresh` skips the
//!                                engine's five-minute copy)
//!   acp_registry.add(id)         adds it to the device shown (minutes), or
//!                                updates it to the registry's version;
//!                                rejects with the engine's reason
//!   acp_registry.remove(id)      removes it from that device
//!   acp_registry.env_get(id)     its environment there (`KEY=VALUE` a line)
//!   acp_registry.env_set({ id, env })  kept there, owner-only, never synced
//!   acp_registry.add_own({ name, command, env })  one of your own there

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use serde_json::{Value as Json, json};
use zgx_desktop::WindowContext;
use zgx_script::{ScriptHost, Shared, Value};

use crate::auth::text;
use crate::engine;

#[derive(Clone, Copy, PartialEq)]
enum Look {
    Looking,
    Ready,
    Unavailable,
}

struct Registry {
    shared: RefCell<Vec<Shared>>,
    look: Cell<Look>,
    offline: Cell<bool>,
    agents: RefCell<Vec<Json>>,
    /// Being added from here (the engine says so too, once asked again).
    adding: RefCell<Vec<String>>,
    /// The device the list is of (none: this Mac).
    target: RefCell<Option<String>>,
    loading: Cell<u64>,
}

thread_local! {
    static REGISTRY: Rc<Registry> = Rc::new(Registry {
        shared: RefCell::default(),
        look: Cell::new(Look::Looking),
        offline: Cell::new(false),
        agents: RefCell::default(),
        adding: RefCell::default(),
        target: RefCell::default(),
        loading: Cell::new(0),
    });
}

pub fn install(_cx: &mut WindowContext, host: &ScriptHost) {
    let registry = REGISTRY.with(Rc::clone);
    registry.shared.borrow_mut().push(host.shared("acp_registry", registry.value()));

    let r = registry.clone();
    host.action("acp_registry.load", move |refresh| {
        let r = r.clone();
        let refresh = matches!(refresh, Value::Bool(true));
        engine::get().spawn(async move { r.load(refresh).await });
    });
    let r = registry.clone();
    host.async_action("acp_registry.add", move |id| {
        let r = r.clone();
        async move { crate::plugins::preview::refuse("acp_registry.add")?; r.add(text(&id)).await }
    });
    let r = registry.clone();
    host.async_action("acp_registry.remove", move |id| {
        let r = r.clone();
        async move { r.remove(text(&id)).await }
    });
    let r = registry.clone();
    host.async_action("acp_registry.env_get", move |id| {
        let params = r.addressed(json!({ "id": text(&id) }));
        async move {
            match engine::get().call("GetAcpAgentEnv", params).await {
                Ok(reply) => Ok(Value::from(reply["env"].as_str().unwrap_or_default())),
                Err(error) => Err(reason(&error)),
            }
        }
    });
    let r = registry.clone();
    host.async_action("acp_registry.env_set", move |args| {
        let id = args.field("id").map(text).unwrap_or_default();
        let env = args.field("env").map(text).unwrap_or_default();
        let params = r.addressed(json!({ "id": id, "env": env }));
        async move { crate::plugins::preview::refuse("acp_registry.env_set")?;
            match engine::get().call("SetAcpAgentEnv", params).await {
                Ok(reply) => Ok(Value::from(reply["env"].as_str().unwrap_or_default())),
                Err(error) => Err(reason(&error)),
            }
        }
    });
    let r = registry;
    host.async_action("acp_registry.add_own", move |args| {
        let r = r.clone();
        let field = |k: &str| args.field(k).map(text).unwrap_or_default();
        let params = json!({ "name": field("name"), "command": field("command"), "env": field("env") });
        async move { crate::plugins::preview::refuse("acp_registry.add_own")?; r.add_own(params).await }
    });
}

/// Who wrote it: the registry's authors, an address left as its name part.
fn by(entry: &Json) -> String {
    let authors: Vec<String> = entry["authors"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Json::as_str)
        .map(|a| a.split('@').next().unwrap_or(a).trim().to_string())
        .filter(|a| !a.is_empty())
        .collect();
    authors.join(", ")
}

/// A repository as the page names it: `github.com/owner/repo`.
fn source(entry: &Json) -> Option<String> {
    let url = entry["repository"].as_str()?;
    let rest = url.strip_prefix("https://")?;
    Some(rest.trim_end_matches('/').trim_end_matches(".git").to_string())
}

fn opt(value: Option<&str>) -> Value {
    value.filter(|v| !v.is_empty()).map_or(Value::Nil, Value::from)
}

impl Registry {
    fn value(&self) -> Value {
        let state = match self.look.get() {
            Look::Looking => "looking",
            Look::Ready => "ready",
            Look::Unavailable => "unavailable",
        };
        let adding = self.adding.borrow();
        let agents: Vec<Value> = self
            .agents
            .borrow()
            .iter()
            .map(|a| {
                let id = a["id"].as_str().unwrap_or_default();
                Value::record([
                    ("id", Value::from(id)),
                    ("harness", Value::from(a["harness"].as_str().unwrap_or_default())),
                    ("name", Value::from(a["name"].as_str().unwrap_or(id))),
                    ("version", Value::from(a["version"].as_str().unwrap_or_default())),
                    ("about", Value::from(a["description"].as_str().unwrap_or_default())),
                    ("by", Value::from(by(a).as_str())),
                    ("license", opt(a["license"].as_str())),
                    ("source", source(a).map_or(Value::Nil, |s| Value::from(s.as_str()))),
                    ("source_url", opt(a["repository"].as_str())),
                    ("website", opt(a["website"].as_str())),
                    ("kind", opt(a["kind"].as_str())),
                    ("readiness", Value::from(a["readiness"].as_str().unwrap_or("ready"))),
                    ("host", opt(a["host"].as_str())),
                    ("checksum", a["checksum"].as_bool().map_or(Value::Nil, Value::Bool)),
                    ("built_in", opt(a["builtIn"].as_str())),
                    ("added", opt(a["added"].as_str())),
                    (
                        "adding",
                        Value::Bool(a["adding"].as_bool().unwrap_or(false) || adding.iter().any(|x| x == id)),
                    ),
                    ("delisted", Value::Bool(a["delisted"].as_bool().unwrap_or(false))),
                    ("own", Value::Bool(a["own"].as_bool().unwrap_or(false))),
                    ("icon", opt(a["icon"].as_str())),
                ])
            })
            .collect();
        let target = self.target.borrow().as_deref().map_or(Value::Nil, Value::from);
        Value::record([
            ("state", Value::from(state)),
            ("offline", Value::Bool(self.offline.get())),
            ("agents", Value::List(agents.into())),
            ("target", target),
        ])
    }

    fn publish(&self) {
        let value = self.value();
        for shared in self.shared.borrow().iter() {
            shared.set(value.clone());
        }
    }

    /// The device Providers shows (its agents' target).
    fn addressed(&self, params: Json) -> Json {
        crate::agents::addressed_to_shown(params)
    }

    async fn load(self: Rc<Self>, refresh: bool) {
        let run = self.loading.get() + 1;
        self.loading.set(run);
        let target = crate::agents::shown_target();
        // Another device's list: what was shown goes.
        if *self.target.borrow() != target {
            *self.target.borrow_mut() = target;
            self.agents.borrow_mut().clear();
            self.look.set(Look::Looking);
            self.publish();
        }
        let params = self.addressed(json!({ "refresh": refresh }));
        match engine::get().call("ListAcpRegistry", params).await {
            Ok(reply) if self.loading.get() == run => {
                *self.agents.borrow_mut() = reply["agents"].as_array().cloned().unwrap_or_default();
                crate::agents::remember_listing_icons(&self.agents.borrow());
                self.offline.set(reply["offline"].as_bool().unwrap_or(false));
                self.look.set(Look::Ready);
                self.publish();
            }
            Ok(_) => {}
            Err(error) => {
                eprintln!("zeron-zgx: acp registry: the list didn't come: {error}");
                if self.loading.get() == run && self.look.get() != Look::Ready {
                    self.look.set(Look::Unavailable);
                    self.publish();
                }
            }
        }
    }

    /// Takes one agent's fresh listing in place.
    fn take(&self, agent: &Json) {
        let Some(id) = agent["id"].as_str() else { return };
        let mut agents = self.agents.borrow_mut();
        match agents.iter_mut().find(|a| a["id"].as_str() == Some(id)) {
            Some(slot) => *slot = agent.clone(),
            None => agents.push(agent.clone()),
        }
    }

    async fn add(self: Rc<Self>, id: String) -> Result<Value, String> {
        if self.adding.borrow().contains(&id) {
            return Err("already adding".into());
        }
        self.adding.borrow_mut().push(id.clone());
        self.publish();
        let result = engine::get()
            .call("AddAcpAgent", self.addressed(json!({ "id": id })))
            .await;
        self.adding.borrow_mut().retain(|a| *a != id);
        match result {
            Ok(reply) => {
                self.take(&reply["agent"]);
                self.publish();
                // The agent is a harness now: its card, and its models.
                crate::agents::reload();
                Ok(Value::Nil)
            }
            Err(error) => {
                self.publish();
                eprintln!("zeron-zgx: acp registry: {id} wasn't added: {error}");
                Err(reason(&error))
            }
        }
    }

    async fn add_own(self: Rc<Self>, params: Json) -> Result<Value, String> {
        match engine::get().call("AddOwnAgent", self.addressed(params)).await {
            Ok(reply) => {
                self.take(&reply["agent"]);
                self.publish();
                crate::agents::reload();
                Ok(Value::from(reply["agent"]["harness"].as_str().unwrap_or_default()))
            }
            Err(error) => {
                eprintln!("zeron-zgx: acp registry: your agent wasn't added: {error}");
                Err(reason(&error))
            }
        }
    }

    async fn remove(self: Rc<Self>, id: String) -> Result<Value, String> {
        match engine::get()
            .call("RemoveAcpAgent", self.addressed(json!({ "id": id })))
            .await
        {
            Ok(_) => {
                if let Some(agent) = self
                    .agents
                    .borrow_mut()
                    .iter_mut()
                    .find(|a| a["id"].as_str() == Some(id.as_str()))
                {
                    agent["added"] = Json::Null;
                }
                // (One no longer listed, or your own, goes with it.)
                self.agents.borrow_mut().retain(|a| {
                    let gone = a["delisted"].as_bool().unwrap_or(false) || a["own"].as_bool().unwrap_or(false);
                    !gone || a["id"] != id.as_str()
                });
                self.publish();
                crate::agents::reload();
                Ok(Value::Nil)
            }
            Err(error) => {
                eprintln!("zeron-zgx: acp registry: {id} wasn't removed: {error}");
                Err(reason(&error))
            }
        }
    }
}

/// What the page says of a failure: offline, else the engine's own words
/// (its errors here are written to be read: "the download doesn't match the
/// registry's checksum", "Node.js (npm), to install …").
fn reason(error: &str) -> String {
    match engine::outcome(error) {
        "offline" => "offline".into(),
        _ => error
            .trim_start_matches("rpc error: ")
            .trim_start_matches("install failed: ")
            .trim()
            .trim_end_matches('.')
            .to_string(),
    }
}
