//! Where a plugin's requests from the window may go: `ui.fetch` and remote
//! pictures, and `ui.socket` the day this app has a connector for it. The
//! rule is plugin-guard's `net`, the one the engine holds a plugin's service
//! to, so the two can't drift apart: public addresses, or the hosts its
//! manifest names (`"network": { "hosts": ["localhost:8080"] }`). This
//! device, your network and instance metadata are refused otherwise.
//!
//! zgx asks of every request: its URL and each redirect's (`url`), every
//! address a name resolves to, and an IP literal's (`address`), and every
//! header (`header`), before anything connects. `caller` is the plugin that
//! asked; `None` is the app's own scripts, which aren't held to this. What a
//! plugin may reach comes from the engine's list, whose consent covers its
//! hosts: nothing in a plugin's VM reaches this table.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, RwLock};

use zeron_plugin_guard::net::Policy;
use zgx_script::{FetchPolicy, Url};

/// Each loaded plugin's rules, by id. Its requests run off the window's
/// thread (zgx-run's), so it's shared across threads.
#[derive(Clone, Default)]
pub struct Network {
    plugins: Arc<RwLock<HashMap<String, Result<Policy, String>>>>,
}

impl Network {
    /// These plugins (each with its manifest's `network.hosts`) are the ones
    /// loaded now. A list the engine would have refused reaches nothing.
    pub fn set(&self, plugins: impl IntoIterator<Item = (String, Vec<String>)>) {
        let rules = plugins.into_iter().map(|(id, hosts)| {
            let rules = Policy::new(&hosts);
            (id, rules)
        });
        *self.plugins.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = rules.collect();
    }

    /// What `f` says of `plugin`'s rules. One not loaded (withdrawn while a
    /// request ran) reaches nothing.
    fn asking(&self, plugin: &str, f: impl FnOnce(&Policy) -> Result<(), String>) -> Result<(), String> {
        let plugins = self.plugins.read().unwrap_or_else(|poisoned| poisoned.into_inner());
        match plugins.get(plugin) {
            Some(Ok(rules)) => f(rules),
            Some(Err(why)) => Err(why.clone()),
            None => Err(format!("{plugin} isn't on, so it can't use the network")),
        }
    }
}

/// The port `url` goes to, written or its scheme's.
fn port_of(url: &Url) -> Option<u16> {
    url.port().or(match url.scheme() {
        "http" | "ws" => Some(80),
        "https" | "wss" => Some(443),
        _ => None,
    })
}

/// How a manifest names what `url` goes to: its host and the port it wrote.
fn entry_for(url: &Url) -> String {
    let host = match url.host().contains(':') {
        true => format!("[{}]", url.host()),
        false => url.host().to_string(),
    };
    match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host,
    }
}

impl FetchPolicy for Network {
    fn url(&self, caller: Option<&str>, url: &Url) -> Result<(), String> {
        let Some(plugin) = caller else { return Ok(()) };
        let Some(port) = port_of(url) else {
            return Err(format!("{plugin} reaches http, https, ws and wss addresses only"));
        };
        self.asking(plugin, |rules| {
            rules
                .host(url.host(), port)
                .map_err(|why| format!("{why}. To reach it, add \"{}\" to \"network\": {{ \"hosts\": [] }} in its manifest", entry_for(url)))
        })
    }

    fn address(&self, caller: Option<&str>, host: &str, address: IpAddr) -> Result<(), String> {
        let Some(plugin) = caller else { return Ok(()) };
        // (`::ffff:127.0.0.1` reaches this device as `127.0.0.1` does.)
        self.asking(plugin, |rules| rules.resolved(host, address.to_canonical()))
    }

    fn header(&self, caller: Option<&str>, host: &str, name: &str) -> Result<(), String> {
        let Some(plugin) = caller else { return Ok(()) };
        self.asking(plugin, |rules| rules.header_to(host, name))
    }
}
