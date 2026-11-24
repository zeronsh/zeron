//! What the Templates page shows of templates from the store: each one's
//! record as it was installed (`.imported.json`, here on the disk), and what
//! only the engine can say of it now (`ListTemplates`): whether the store
//! verified it and its files are still that record's, whether it was changed
//! here, and the update found for it. The engine hashes the files each time
//! it's asked; nothing of that is worked out, or kept, here.

use serde_json::{Value as Json, json};

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
    let name = pending["pending"].clone();
    let Some(item) = said["pending"].as_array().and_then(|all| all.iter().find(|p| p["pending"] == name && p["hash"] == pending["hash"])) else { return };
    for key in ["store", "update_of", "replaces_edits", "new_owner", "also_installed"] {
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
        let mut other = json!({ "pending": "midnight-1", "hash": "h2" });
        waiting(&said, &mut other);
        assert!(other["store"].is_null());
    }
}
