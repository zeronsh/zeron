//! Publishes this device didn't make (amendment 45). A version is public as
//! soon as it's published, so every publish of the account's plugins and
//! templates that wasn't sent from here raises a notice on the account's
//! devices: which version, and whether another device or which deploy token
//! sent it. A device reads them from the owner's view of its versions
//! (`/me/plugins` and `/me/templates`, each looked over on its own: `via`,
//! `key`, `deploy_token`), keeps each until the app has shown it, and raises
//! one once per version.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::client::Kind;

/// How far back, by the store's clock, a device remembers which publishes
/// it has looked over (older ones are behind [`Seen::before`]).
pub const KEEP_MS: i64 = 30 * 24 * 60 * 60 * 1000;

/// At most this many notices wait for the app; the oldest go first.
pub const MAX_WAITING: usize = 20;

/// Which publishes a device has looked over, by the store's clock (never
/// the device's, which can be off).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Seen {
    /// Whose (the publisher id): another account signed in starts afresh.
    #[serde(default)]
    pub account: String,
    /// Every publish at or before this was looked over.
    pub before: i64,
    /// Publishes after `before` looked over: `id@version` → `published_at`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub versions: BTreeMap<String, i64>,
}

/// A publish not made on this device.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PublishNotice {
    /// `id@version`: what the app acknowledges it by.
    pub id: String,
    /// Its id, a plugin's or a template's (one namespace).
    pub plugin: String,
    /// It's a template's.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub template: bool,
    pub version: String,
    /// The plugin's name in the store, when it gave one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// `device` (another of the account's devices) or `deploy_token`.
    pub via: String,
    /// The deploy token's name, when one sent it and the store still names it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_name: Option<String>,
    /// When the store took it (ms).
    pub at: i64,
}

impl PublishNotice {
    /// In a sentence, for a banner or a host that doesn't know `via`.
    pub fn words(&self) -> String {
        let what = format!("{} {}", self.name.as_deref().unwrap_or(&self.plugin), self.version);
        match (self.via.as_str(), &self.token_name) {
            ("deploy_token", Some(token)) => format!("{what} was published with your deploy token \"{token}\". It's live now. If that wasn't you, revoke the token and yank the version."),
            ("deploy_token", None) => format!("{what} was published with one of your deploy tokens. It's live now. If that wasn't you, revoke the token and yank the version."),
            _ => format!("{what} was published from another of your devices. It's live now. If that wasn't you, yank it and sign that device out."),
        }
    }

    /// As the plugin list carries it (`store_notices`).
    pub fn shown(&self) -> Value {
        json!({
            "id": self.id,
            "kind": if self.template { "template" } else { "plugin" },
            "plugin": self.plugin,
            "version": self.version,
            "name": self.name,
            "via": self.via,
            "token_name": self.token_name,
            "at": self.at,
            "words": self.words(),
        })
    }
}

/// Look over `mine` (`/me/plugins`' answer, or `/me/templates`' for that
/// `kind`) after `seen`: what's been looked over now, and a notice for each
/// publish after `seen` that this device (`own_key`, its signing key) didn't send. The first look notices
/// nothing: what was there before a device looked isn't news to it. A
/// version another account published (a plugin transferred in, by its
/// former owner: its `publisher_id` isn't the account's) isn't noticed.
/// `clean` makes the store's words safe to keep and show.
pub fn look(kind: Kind, seen: Option<&Seen>, mine: &Value, own_key: &str, clean: impl Fn(&str) -> String) -> (Seen, Vec<PublishNotice>) {
    let account = mine["publisher_id"].as_str();
    let versions = mine[kind.segment()].as_array().into_iter().flatten().flat_map(|item| {
        let id = item["id"].as_str().unwrap_or_default();
        let name = item["name"].as_str();
        item["versions"].as_array().into_iter().flatten().filter_map(move |v| {
            let (version, at) = (v["version"].as_str()?, v["published_at"].as_i64()?);
            (!id.is_empty()).then_some((id, name, version, at, v))
        })
    });
    let Some(seen) = seen else {
        let before = versions.map(|(.., at, _)| at).max().unwrap_or(0);
        return (Seen { before, ..Seen::default() }, Vec::new());
    };
    let mut seen = seen.clone();
    let mut notices = Vec::new();
    for (id, name, version, at, v) in versions {
        let key = format!("{id}@{version}");
        if at <= seen.before || seen.versions.contains_key(&key) {
            continue;
        }
        seen.versions.insert(key.clone(), at);
        if let (Some(account), Some(by)) = (account, v["publisher_id"].as_str())
            && by != account
        {
            continue;
        }
        let via = v["via"].as_str().unwrap_or_default();
        let from_here = via == "device" && v["key"].as_str() == Some(own_key);
        if from_here {
            continue;
        }
        notices.push(PublishNotice {
            id: key,
            plugin: id.to_owned(),
            template: kind == Kind::Template,
            version: clean(version),
            name: name.map(&clean).filter(|n| !n.is_empty()),
            via: if via == "deploy_token" { "deploy_token" } else { "device" }.to_owned(),
            token_name: v["deploy_token"]["name"].as_str().map(&clean).filter(|n| !n.is_empty()),
            at,
        });
    }
    // Remembered for 30 days of the store's clock; older is behind `before`.
    let newest = seen.versions.values().copied().max().unwrap_or(seen.before).max(seen.before);
    let cutoff = newest - KEEP_MS;
    if cutoff > seen.before {
        seen.before = cutoff;
        seen.versions.retain(|_, at| *at > cutoff);
    }
    notices.sort_by_key(|n| n.at);
    (seen, notices)
}

/// `waiting` with `new` added, at most [`MAX_WAITING`], oldest first.
pub fn add(waiting: &mut Vec<PublishNotice>, new: Vec<PublishNotice>) {
    for notice in new {
        if !waiting.iter().any(|w| w.id == notice.id) {
            waiting.push(notice);
        }
    }
    let excess = waiting.len().saturating_sub(MAX_WAITING);
    waiting.drain(..excess);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mine(versions: &[(&str, &str, i64, &str, &str, Option<&str>)]) -> Value {
        let mut plugins: BTreeMap<&str, Vec<Value>> = BTreeMap::new();
        for (id, version, at, via, key, token_name) in versions {
            plugins.entry(id).or_default().push(json!({
                "version": version, "published_at": at, "via": via, "key": if *via == "deploy_token" { Value::Null } else { json!(key) },
                "deploy_token": token_name.map(|name| json!({ "id": "dt_1", "short": "Ab12Cd34", "name": name })),
            }));
        }
        json!({ "plugins": plugins.into_iter().map(|(id, versions)| json!({ "id": id, "name": "Focus", "versions": versions })).collect::<Vec<_>>() })
    }

    fn same(text: &str) -> String {
        text.to_owned()
    }

    #[test]
    fn the_first_look_notices_nothing_and_later_publishes_not_from_here_are_noticed_once() {
        let (seen, notices) = look(Kind::Plugin, None, &mine(&[("focus", "1.0.0", 100, "device", "OTHER", None)]), "MINE", same);
        assert!(notices.is_empty());
        assert_eq!(seen.before, 100);

        let now = mine(&[
            ("focus", "1.0.0", 100, "device", "OTHER", None),
            ("focus", "1.1.0", 200, "device", "MINE", None),
            ("focus", "1.2.0", 300, "device", "OTHER", None),
            ("focus", "1.3.0", 400, "deploy_token", "", Some("ci")),
        ]);
        let (seen, notices) = look(Kind::Plugin, Some(&seen), &now, "MINE", same);
        assert_eq!(notices.iter().map(|n| n.id.as_str()).collect::<Vec<_>>(), ["focus@1.2.0", "focus@1.3.0"]);
        assert_eq!(notices[0].via, "device");
        assert!(notices[0].words().contains("another of your devices"), "{}", notices[0].words());
        assert_eq!((notices[1].via.as_str(), notices[1].token_name.as_deref()), ("deploy_token", Some("ci")));
        assert!(notices[1].words().starts_with("Focus 1.3.0 was published with your deploy token \"ci\""), "{}", notices[1].words());

        // Once per version.
        let (seen, again) = look(Kind::Plugin, Some(&seen), &now, "MINE", same);
        assert!(again.is_empty());

        // A template's the same, said to be one; of a kind, only that's looked at.
        let with = json!({ "publisher_id": "pub_a", "templates": [{ "id": "midnight", "name": "Midnight", "versions": [
            { "version": "1.0.0", "published_at": 500, "via": "device", "key": "MINE" },
            { "version": "1.1.0", "published_at": 600, "via": "deploy_token", "key": null, "deploy_token": { "name": "ci" } },
        ] }] });
        assert!(look(Kind::Plugin, Some(&seen), &with, "MINE", same).1.is_empty());
        let (seen, notices) = look(Kind::Template, Some(&Seen::default()), &with, "MINE", same);
        assert_eq!(notices.iter().map(|n| (n.id.as_str(), n.template)).collect::<Vec<_>>(), [("midnight@1.1.0", true)]);
        assert_eq!((notices[0].shown()["kind"].as_str(), notices[0].shown()["plugin"].as_str()), (Some("template"), Some("midnight")));
        assert!(look(Kind::Template, Some(&seen), &with, "MINE", same).1.is_empty());
        // One kept by an earlier Zeron, which only knew of plugins.
        let old: PublishNotice = serde_json::from_value(json!({ "id": "focus@1.0.0", "plugin": "focus", "version": "1.0.0", "via": "device", "at": 1 })).unwrap();
        assert_eq!(old.shown()["kind"], "plugin");
    }

    #[test]
    fn a_version_another_account_published_is_no_notice() {
        let (seen, _) = look(Kind::Plugin, None, &json!({ "publisher_id": "pub_a", "plugins": [] }), "MINE", same);
        let mut now = mine(&[("moved", "1.0.0", 100, "device", "OTHER", None), ("moved", "1.1.0", 200, "device", "OTHER", None)]);
        now["publisher_id"] = json!("pub_a");
        now["plugins"][0]["versions"][0]["publisher_id"] = json!("pub_former");
        now["plugins"][0]["versions"][1]["publisher_id"] = json!("pub_a");
        let (_, notices) = look(Kind::Plugin, Some(&seen), &now, "MINE", same);
        assert_eq!(notices.iter().map(|n| n.id.as_str()).collect::<Vec<_>>(), ["moved@1.1.0"]);
    }

    #[test]
    fn a_device_remembers_thirty_days_of_the_store_s_clock() {
        let seen = Seen { versions: BTreeMap::from([("focus@1.0.0".to_owned(), 10)]), ..Seen::default() };
        let later = 10 + KEEP_MS + 5;
        let (seen, notices) = look(Kind::Plugin, Some(&seen), &mine(&[("focus", "1.0.0", 10, "device", "OTHER", None), ("focus", "2.0.0", later, "device", "OTHER", None)]), "MINE", same);
        assert_eq!(notices.len(), 1);
        assert_eq!(seen.before, 15);
        assert_eq!(seen.versions.keys().collect::<Vec<_>>(), ["focus@2.0.0"]);
    }

    #[test]
    fn waiting_notices_are_kept_unique_and_few() {
        let notice = |n: i64| PublishNotice { id: format!("p@1.0.{n}"), plugin: "p".into(), template: false, version: format!("1.0.{n}"), name: None, via: "device".into(), token_name: None, at: n };
        let mut waiting = vec![notice(0)];
        add(&mut waiting, (0..25).map(notice).collect());
        assert_eq!(waiting.len(), MAX_WAITING);
        assert_eq!(waiting.first().unwrap().at, 5);
    }
}
