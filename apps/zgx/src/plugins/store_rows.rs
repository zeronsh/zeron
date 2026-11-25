//! What the Templates page shows of templates from the store: each one's
//! record as it was installed (`.imported.json`, here on the disk), and what
//! only the engine can say of it now (`ListTemplates`): whether the store
//! verified it and its files are still that record's, whether it was changed
//! here, and the update found for it. The engine hashes the files each time
//! it's asked; nothing of that is worked out, or kept, here.

use std::time::{Duration, Instant};

use serde_json::{Value as Json, json};

/// How long one that wasn't answered is left before it's asked again.
const AGAIN: Duration = Duration::from_secs(30);

/// When the engine is asked of the templates (`ListTemplates`, which reads
/// their files): as what's here moves (`of`, the folders' records and what
/// waits), or when it's plainly wanted (the page opens, updates looked for);
/// never again for the same. A look is every two seconds where there's
/// nothing to watch with.
#[derive(Default)]
pub struct Asks {
    /// What the last answer was of.
    answered: Option<Json>,
    /// What's on its way, and whether it's wanted again by now.
    asking: Option<Json>,
    again: bool,
    failed: Option<Instant>,
}

impl Asks {
    /// Whether to ask now, of what's here (`of`).
    pub fn due(&mut self, of: &Json, now: Instant) -> bool {
        if let Some(asking) = &self.asking {
            self.again |= asking != of;
            return false;
        }
        if self.answered.as_ref() == Some(of) || self.failed.is_some_and(|at| now.duration_since(at) < AGAIN) {
            return false;
        }
        self.asking = Some(of.clone());
        true
    }

    /// Wanted, whatever was answered.
    pub fn want(&mut self) {
        (self.answered, self.failed, self.again) = (None, None, self.asking.is_some());
    }

    /// The answer came, or didn't: whether it's wanted again at once.
    pub fn done(&mut self, answered: bool, now: Instant) -> bool {
        let asked = self.asking.take();
        let again = std::mem::take(&mut self.again);
        self.failed = (!answered).then_some(now);
        self.answered = if answered && !again { asked } else { None };
        answered && again
    }
}

/// Of `source`, what the page may have of where a template came from: never a
/// local path; from the store, its id there and who published it, and the
/// engine's word on it (`row`, its row for the same install).
pub fn source(source: &Json, row: Option<&Json>) -> Json {
    let mut out = json!({});
    let mut take = |from: &Json, key: &str| {
        if !from[key].is_null() {
            out[key] = from[key].clone();
        }
    };
    for key in ["kind", "url", "ref"] {
        take(source, key);
    }
    if source["kind"] == "store" {
        // (As the engine keeps it, and as it shows it.)
        let record = if source["store"].is_object() { &source["store"] } else { source };
        for key in ["id", "publisher"] {
            take(record, key);
        }
        // (Verified is said by the engine alone: the record's own word is of
        // when it was installed.)
        if let Some(said) = row.map(|row| &row["imported"]["source"]) {
            for key in ["verified", "verified_at", "verified_alternative", "replaced"] {
                take(said, key);
            }
        }
    }
    out
}

/// The engine's row for template `id` as it's installed here (`imported`): the
/// same package, or its word is of another.
pub fn row<'a>(said: &'a Json, id: &str, imported: &Json) -> Option<&'a Json> {
    let hash = imported["hash"].as_str().filter(|h| !h.is_empty())?;
    said["templates"].as_array()?.iter().find(|row| row["id"] == id && row["imported"]["hash"] == hash)
}

/// Which updates the store's templates take on this device, as the plugins'
/// are (`plugins.store_updates`): the device's choice, and each one's own by
/// its id in the store. Null before the engine says.
pub fn choices(said: &Json) -> Json {
    let taken = |v: &Json| v.as_str().filter(|v| matches!(*v, "newest" | "verified")).map(str::to_owned);
    let Some(device) = taken(&said["prefs"]["device"]) else { return Json::Null };
    let own: serde_json::Map<String, Json> = said["templates"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| Some((row["imported"]["source"]["id"].as_str()?.to_owned(), json!(taken(&row["prefer"])?))))
        .collect();
    json!({ "device": device, "plugins": own })
}

/// What the engine says of package `name` waiting, beyond what's on the disk.
pub fn waiting(said: &Json, pending: &mut Json) {
    const ENGINES: [&str; 5] = ["store", "update_of", "replaces_edits", "new_owner", "also_installed"];
    // (Its own file's word on any of it is nothing.)
    if let Some(pending) = pending.as_object_mut() {
        pending.retain(|key, _| !ENGINES.contains(&key.as_str()));
    }
    let name = pending["pending"].clone();
    let Some(item) = said["pending"].as_array().and_then(|all| all.iter().find(|p| p["pending"] == name && p["hash"] == pending["hash"])) else { return };
    for key in ENGINES {
        if !item[key].is_null() {
            pending[key] = item[key].clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> Json {
        json!({ "key": null, "hash": "h1", "version": "1.2.0", "source": { "kind": "store", "path": "/Users/me/x", "store": { "id": "midnight", "publisher": "ada", "publisher_id": "u_1", "hash": "c", "verified": true } } })
    }

    #[test]
    fn a_store_template_s_row_is_its_record_and_the_engine_s_word_on_it() {
        let imported = record();
        // Before the engine has said: whose it is, and no mark.
        assert_eq!(source(&imported["source"], None), json!({ "kind": "store", "id": "midnight", "publisher": "ada" }));
        let said = json!({ "templates": [
            { "id": "midnight-2", "imported": { "hash": "h1", "source": { "kind": "store", "id": "midnight", "verified": true, "verified_at": 5 } }, "edited": false },
            { "id": "other", "imported": { "hash": "h1", "source": { "verified": true } } },
        ] });
        let row_ = row(&said, "midnight-2", &imported).unwrap();
        assert_eq!(source(&imported["source"], Some(row_)), json!({ "kind": "store", "id": "midnight", "publisher": "ada", "verified": true, "verified_at": 5 }));
        // Of another install of it, or none of this name: not this one's.
        assert!(row(&said, "midnight-2", &json!({ "hash": "h2" })).is_none());
        assert!(row(&said, "midnight-2", &json!({})).is_none());
        assert!(row(&said, "gone", &imported).is_none());
        assert!(row(&Json::Null, "midnight-2", &imported).is_none());
    }

    #[test]
    fn the_engine_is_asked_as_what_s_here_moves_and_never_for_the_same() {
        let (mut asks, t) = (Asks::default(), Instant::now());
        let (one, two) = (json!(["one"]), json!(["two"]));
        assert!(asks.due(&one, t));
        assert!(!asks.due(&one, t), "on its way");
        assert!(!asks.done(true, t));
        for s in 0..100 {
            assert!(!asks.due(&one, t + Duration::from_secs(2 * s)), "a look a moment, nothing moved");
        }
        assert!(asks.due(&two, t), "moved");
        // Moved again while it was on its way: asked once more.
        assert!(!asks.due(&one, t));
        assert!(asks.done(true, t));
        assert!(asks.due(&one, t));
        assert!(!asks.done(true, t));
        // The page opens, or updates were looked for.
        asks.want();
        assert!(asks.due(&one, t));
        asks.want();
        assert!(asks.done(true, t), "wanted as one was on its way");
        assert!(asks.due(&one, t) && !asks.done(true, t));
        // Not answered: again, not at once.
        assert!(asks.due(&two, t) && !asks.done(false, t));
        assert!(!asks.due(&two, t + Duration::from_secs(2)));
        assert!(asks.due(&two, t + AGAIN));
        asks.done(false, t);
        asks.want();
        assert!(asks.due(&two, t), "wanted: at once");
    }

    #[test]
    fn its_updates_are_the_device_s_choice_or_its_own() {
        assert_eq!(choices(&json!({ "templates": [] })), Json::Null);
        let said = json!({ "prefs": { "device": "newest" }, "templates": [
            { "id": "midnight-2", "prefer": "verified", "imported": { "source": { "kind": "store", "id": "midnight" } } },
            { "id": "dusk", "prefer": "all", "imported": { "source": { "kind": "store", "id": "dusk" } } },
            { "id": "mine" },
        ] });
        assert_eq!(choices(&said), json!({ "device": "newest", "plugins": { "midnight": "verified" } }));
    }

    #[test]
    fn nothing_else_gains_a_word_and_no_path_is_shown() {
        let file = json!({ "kind": "file", "path": "/Users/me/x.zeron-template", "verified": true, "id": "zeron" });
        assert_eq!(source(&file, Some(&json!({ "imported": { "source": { "verified": true } } }))), json!({ "kind": "file" }));
        let link = json!({ "kind": "link", "url": "https://x", "ref": "v1" });
        assert_eq!(source(&link, None), link);
    }

    #[test]
    fn a_waiting_package_has_the_engine_s_word_of_that_package_alone() {
        let said = json!({ "pending": [{ "pending": "midnight-1", "hash": "h1", "store": { "id": "midnight" }, "replaces_edits": true, "key": "other" }] });
        let mut same = json!({ "pending": "midnight-1", "hash": "h1", "key": "k" });
        waiting(&said, &mut same);
        assert_eq!(same, json!({ "pending": "midnight-1", "hash": "h1", "key": "k", "store": { "id": "midnight" }, "replaces_edits": true }));
        let mut other = json!({ "pending": "midnight-1", "hash": "h2", "store": { "id": "midnight", "verified": true }, "update_of": "x", "also_installed": [] });
        waiting(&said, &mut other);
        assert_eq!(other, json!({ "pending": "midnight-1", "hash": "h2" }), "what its own file says of the store isn't the store's word");
    }
}
