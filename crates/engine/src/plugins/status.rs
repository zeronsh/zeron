//! The engine's per-plugin state (`.state.json`) and the status decision
//! table (contract "Engine state").
//!
//! Own plugins get no automatic decision: one asking for nothing is `on` at
//! once; one asking for anything waits for Review, and so does every new
//! permission after that (`native` included), whoever made it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::{Deserialize, Serialize};

pub const STATE_FILE: &str = ".state.json";

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    pub own: BTreeSet<String>,
    #[serde(default)]
    pub decisions: BTreeMap<String, Decision>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Decision {
    pub allowed: bool,
    /// The signing key the decision was made for; `null` for an unknown author.
    #[serde(default)]
    pub author_key: Option<String>,
    #[serde(default)]
    pub permissions: Vec<String>,
    /// Only when `author_key` is null, or for a decline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    #[serde(default)]
    pub off: bool,
}

impl State {
    pub fn load(plugins_dir: &Path) -> Self {
        std::fs::read(plugins_dir.join(STATE_FILE))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, plugins_dir: &Path) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(self).unwrap_or_default();
        crate::runner::write_private_atomic(&plugins_dir.join(STATE_FILE), &bytes)
            .map_err(|_| "Couldn't save your plugin choices".to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Author {
    You,
    /// One of Zeron's own, shipped inside the app (`SetBuiltinPlugins`).
    Zeron,
    Verified {
        #[serde(skip_serializing_if = "Option::is_none")]
        username: Option<String>,
    },
    Unknown {
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
}

impl Author {
    pub fn unknown(reason: &str) -> Self {
        Author::Unknown {
            reason: Some(reason.to_owned()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    On,
    Off,
    Pending,
    Tampered,
    Broken,
}

/// What the table needs to know about a plugin right now.
pub struct Facts<'a> {
    pub broken: bool,
    pub asked: &'a [String],
    pub author: &'a Author,
    /// The key its signature checks out with, if it does.
    pub key: Option<&'a str>,
    pub hash: &'a str,
}

impl Facts<'_> {
    /// The key a decision is recorded against: none for an unknown author.
    pub fn decision_key(&self) -> Option<String> {
        match self.author {
            Author::Unknown { .. } => None,
            _ => self.key.map(str::to_owned),
        }
    }

    /// The decision DecidePlugin records now.
    pub fn record(&self, allow: bool) -> Decision {
        let author_key = self.decision_key();
        Decision {
            content_hash: (author_key.is_none() || !allow).then(|| self.hash.to_owned()),
            author_key,
            allowed: allow,
            permissions: sorted(self.asked),
            off: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub status: Status,
    pub allowed: Vec<String>,
    pub ask: Vec<String>,
}

fn sorted(list: &[String]) -> Vec<String> {
    list.iter()
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

pub fn decide(facts: &Facts<'_>, decision: Option<&Decision>) -> Verdict {
    let asked: BTreeSet<String> = facts.asked.iter().cloned().collect();
    let all = || asked.iter().cloned().collect::<Vec<_>>();
    let verdict = |status, allowed: Vec<String>, ask: Vec<String>| Verdict {
        status,
        allowed,
        ask,
    };
    let on_or_off = |d: &Decision| {
        verdict(
            if d.off { Status::Off } else { Status::On },
            sorted(&d.permissions),
            Vec::new(),
        )
    };
    let missing = |d: &Decision| -> Vec<String> {
        let allowed: BTreeSet<&String> = d.permissions.iter().collect();
        asked
            .iter()
            .filter(|p| !allowed.contains(p))
            .cloned()
            .collect()
    };

    if facts.broken {
        let allowed = decision
            .filter(|d| d.allowed)
            .map(|d| sorted(&d.permissions))
            .unwrap_or_default();
        return verdict(Status::Broken, allowed, Vec::new());
    }

    // Declined: stays off until its key, hash or permissions change.
    if let Some(d) = decision.filter(|d| !d.allowed) {
        let unchanged = d.author_key == facts.decision_key()
            && d.permissions.iter().cloned().collect::<BTreeSet<_>>() == asked
            && d.content_hash
                .as_deref()
                .is_none_or(|hash| hash == facts.hash);
        let status = if unchanged {
            Status::Off
        } else {
            Status::Pending
        };
        return verdict(status, Vec::new(), all());
    }

    match facts.author {
        // Zeron's own: allowed what it asks as it ships, on unless turned
        // off (`scan::builtin_entry` settles it; this is its default).
        Author::Zeron => verdict(
            if decision.is_some_and(|d| d.off) { Status::Off } else { Status::On },
            all(),
            Vec::new(),
        ),
        // Matched by id: an own plugin keeps its key through re-signs.
        Author::You => match decision {
            Some(d) if missing(d).is_empty() => on_or_off(d),
            Some(d) => verdict(Status::Pending, sorted(&d.permissions), missing(d)),
            None if asked.is_empty() => verdict(Status::On, Vec::new(), Vec::new()),
            None => verdict(Status::Pending, Vec::new(), all()),
        },
        Author::Verified { .. } => match decision {
            Some(d) if d.author_key.as_deref() == facts.key => {
                if missing(d).is_empty() {
                    on_or_off(d)
                } else {
                    verdict(Status::Pending, sorted(&d.permissions), missing(d))
                }
            }
            // Someone else's now.
            Some(_) | None => verdict(Status::Pending, Vec::new(), all()),
        },
        Author::Unknown { .. } => match decision {
            // Its hash moves only with an update its own key signed
            // (install.rs), which may ask for more than was allowed.
            Some(d) if d.author_key.is_none() && d.content_hash.as_deref() == Some(facts.hash) => {
                if missing(d).is_empty() {
                    on_or_off(d)
                } else {
                    verdict(Status::Pending, sorted(&d.permissions), missing(d))
                }
            }
            // Allowed, then changed under the user.
            Some(d) => verdict(Status::Tampered, sorted(&d.permissions), all()),
            None => verdict(Status::Pending, Vec::new(), all()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn perms(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn verified() -> Author {
        Author::Verified {
            username: Some("ada".into()),
        }
    }

    fn facts<'a>(
        asked: &'a [String],
        author: &'a Author,
        key: Option<&'a str>,
        hash: &'a str,
    ) -> Facts<'a> {
        Facts {
            broken: false,
            asked,
            author,
            key,
            hash,
        }
    }

    fn allowed(list: &[&str], key: Option<&str>, hash: Option<&str>) -> Decision {
        Decision {
            allowed: true,
            author_key: key.map(str::to_owned),
            permissions: perms(list),
            content_hash: hash.map(str::to_owned),
            off: false,
        }
    }

    #[test]
    fn plugins_status_broken_wins() {
        let asked = perms(&["ui.add"]);
        let mut f = facts(&asked, &Author::You, Some("K"), "h");
        f.broken = true;
        let v = decide(&f, Some(&allowed(&["ui.add"], Some("K"), None)));
        assert_eq!(v.status, Status::Broken);
        assert!(v.ask.is_empty());
    }

    #[test]
    fn plugins_status_own_without_permissions_is_on_at_once() {
        let asked = perms(&[]);
        let v = decide(&facts(&asked, &Author::You, Some("K"), "h"), None);
        assert_eq!(
            v,
            Verdict {
                status: Status::On,
                allowed: vec![],
                ask: vec![]
            }
        );
    }

    #[test]
    fn plugins_status_own_with_permissions_asks_for_all() {
        let asked = perms(&["ui.add", "sessions.read"]);
        let v = decide(&facts(&asked, &Author::You, Some("K"), "h"), None);
        assert_eq!(v.status, Status::Pending);
        assert_eq!(v.ask, perms(&["sessions.read", "ui.add"]));
    }

    #[test]
    fn plugins_status_own_allowed_loads_silently_then_asks_on_any_new_permission() {
        let decision = allowed(&["ui.add"], Some("K"), None);
        let same = perms(&["ui.add"]);
        // Re-signed with a new hash: still on.
        assert_eq!(
            decide(
                &facts(&same, &Author::You, Some("K"), "h2"),
                Some(&decision)
            )
            .status,
            Status::On
        );
        // Fewer permissions: on.
        let fewer = perms(&[]);
        assert_eq!(
            decide(
                &facts(&fewer, &Author::You, Some("K"), "h3"),
                Some(&decision)
            )
            .status,
            Status::On
        );
        // A new permission — even for your own plugin.
        let more = perms(&["ui.add", "network"]);
        let v = decide(
            &facts(&more, &Author::You, Some("K"), "h4"),
            Some(&decision),
        );
        assert_eq!(v.status, Status::Pending);
        assert_eq!(v.ask, perms(&["network"]));
        assert_eq!(v.allowed, perms(&["ui.add"]));
        // Off stays off.
        let mut off = decision.clone();
        off.off = true;
        assert_eq!(
            decide(&facts(&same, &Author::You, Some("K"), "h"), Some(&off)).status,
            Status::Off
        );
    }

    #[test]
    fn plugins_status_native_is_never_auto_allowed() {
        let asked = perms(&["native"]);
        // Own, no decision: asks.
        let v = decide(&facts(&asked, &Author::You, Some("K"), "h"), None);
        assert_eq!(
            (v.status, v.ask.clone()),
            (Status::Pending, perms(&["native"]))
        );
        // Own, allowed without native: still asks for native.
        let v = decide(
            &facts(&asked, &Author::You, Some("K"), "h"),
            Some(&allowed(&["ui.add"], Some("K"), None)),
        );
        assert_eq!((v.status, v.ask), (Status::Pending, perms(&["native"])));
        // Allowed explicitly: on.
        let v = decide(
            &facts(&asked, &Author::You, Some("K"), "h"),
            Some(&allowed(&["native"], Some("K"), None)),
        );
        assert_eq!(v.status, Status::On);
    }

    #[test]
    fn plugins_status_own_matched_by_id_not_key() {
        let asked = perms(&["ui.add"]);
        let decision = allowed(&["ui.add"], Some("OLD"), None);
        assert_eq!(
            decide(
                &facts(&asked, &Author::You, Some("NEW"), "h"),
                Some(&decision)
            )
            .status,
            Status::On
        );
    }

    #[test]
    fn plugins_status_verified_same_author_new_version_loads_silently() {
        let asked = perms(&["ui.add"]);
        let decision = allowed(&["ui.add", "tools"], Some("K"), None);
        let a = verified();
        assert_eq!(
            decide(&facts(&asked, &a, Some("K"), "v2"), Some(&decision)).status,
            Status::On
        );
        let mut off = decision.clone();
        off.off = true;
        assert_eq!(
            decide(&facts(&asked, &a, Some("K"), "v2"), Some(&off)).status,
            Status::Off
        );
    }

    #[test]
    fn plugins_status_verified_new_permission_asks() {
        let asked = perms(&["ui.add", "network"]);
        let a = verified();
        let v = decide(
            &facts(&asked, &a, Some("K"), "v2"),
            Some(&allowed(&["ui.add"], Some("K"), None)),
        );
        assert_eq!((v.status, v.ask), (Status::Pending, perms(&["network"])));
    }

    #[test]
    fn plugins_status_different_author_asks() {
        let asked = perms(&["ui.add"]);
        let a = verified();
        let v = decide(
            &facts(&asked, &a, Some("OTHER"), "v2"),
            Some(&allowed(&["ui.add"], Some("K"), None)),
        );
        assert_eq!((v.status, v.ask), (Status::Pending, perms(&["ui.add"])));
        // And with no decision at all.
        assert_eq!(
            decide(&facts(&asked, &a, Some("K"), "v2"), None).status,
            Status::Pending
        );
    }

    #[test]
    fn plugins_status_unknown_author() {
        let asked = perms(&["ui.add"]);
        let a = Author::unknown("x");
        // No decision.
        assert_eq!(
            decide(&facts(&asked, &a, None, "h"), None).status,
            Status::Pending
        );
        // Allowed for this exact content.
        let decision = allowed(&["ui.add"], None, Some("h"));
        assert_eq!(
            decide(&facts(&asked, &a, None, "h"), Some(&decision)).status,
            Status::On
        );
        // Unsigned and changed.
        let v = decide(&facts(&asked, &a, None, "h2"), Some(&decision));
        assert_eq!(v.status, Status::Tampered);
        assert_eq!(v.ask, perms(&["ui.add"]));
        // Signed by someone when allowed, unknown now.
        let signed = allowed(&["ui.add"], Some("K"), None);
        assert_eq!(
            decide(&facts(&asked, &a, Some("K"), "h"), Some(&signed)).status,
            Status::Tampered
        );
    }

    #[test]
    fn plugins_status_declined_stays_off_until_anything_changes() {
        let asked = perms(&["ui.add"]);
        let a = verified();
        let f = facts(&asked, &a, Some("K"), "h");
        let declined = f.record(false);
        assert!(!declined.allowed);
        assert_eq!(declined.content_hash.as_deref(), Some("h"));
        let v = decide(&f, Some(&declined));
        assert_eq!((v.status, v.ask), (Status::Off, perms(&["ui.add"])));
        assert_eq!(
            decide(&facts(&asked, &a, Some("K"), "h2"), Some(&declined)).status,
            Status::Pending
        );
        assert_eq!(
            decide(&facts(&asked, &a, Some("K2"), "h"), Some(&declined)).status,
            Status::Pending
        );
        let more = perms(&["ui.add", "tools"]);
        assert_eq!(
            decide(&facts(&more, &a, Some("K"), "h"), Some(&declined)).status,
            Status::Pending
        );
        // Unknown authors decline by hash.
        let u = Author::unknown("x");
        let f = facts(&asked, &u, Some("K"), "h");
        let declined = f.record(false);
        assert_eq!(declined.author_key, None);
        assert_eq!(decide(&f, Some(&declined)).status, Status::Off);
    }

    #[test]
    fn plugins_status_record_shapes() {
        let asked = perms(&["tools", "ui.add"]);
        let a = verified();
        let d = facts(&asked, &a, Some("K"), "h").record(true);
        assert_eq!(d.author_key.as_deref(), Some("K"));
        assert_eq!(d.content_hash, None);
        let u = Author::unknown("x");
        let d = facts(&asked, &u, Some("K"), "h").record(true);
        assert_eq!(d.author_key, None);
        assert_eq!(d.content_hash.as_deref(), Some("h"));
        let json = serde_json::to_value(&d).unwrap();
        assert_eq!(json["author_key"], serde_json::Value::Null);
    }

    #[test]
    fn plugins_state_file_round_trip_is_private() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = State::default();
        state.own.insert("compact-sidebar".into());
        state
            .decisions
            .insert("focus".into(), allowed(&["ui.add"], Some("K"), None));
        state.save(dir.path()).unwrap();
        assert_eq!(State::load(dir.path()), state);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.path().join(STATE_FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }
}
