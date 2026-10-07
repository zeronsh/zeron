//! The desktop's sidebar pins, mirrored onto the phone.
//!
//! A desktop on a local (signed-out) profile keeps its pins only in its own
//! settings file (`ui-settings.json`, `sidebarPinnedSessionIdsByProfile`
//! under `"local"`; see `crates/ui/src/settings.rs`): its engine has no pin
//! records (`WatchSidebarPreferences` answers `initialized: false`), so
//! before this the phone never saw a single desktop pin. A synced profile
//! keeps them in the engine's registry, which `WatchSidebarPreferences`
//! does serve.
//!
//! Desktop pins are merged into the phone's own (still phone-editable)
//! pins against the desktop list mirrored last time: pins added on the
//! desktop are pinned, pins the desktop dropped are unpinned, pins made
//! only on the phone stay. Nothing is written back to the desktop.

use std::collections::HashSet;

use zeron_doc::RegistryDoc;
use zeron_proto::SidebarPinChange;

/// Where the desktop keeps `ui-settings.json` (`apps/zeron/src/paths.rs`):
/// `%LOCALAPPDATA%\Zeron` on Windows, `~/.zeron` elsewhere. `cmd /c`
/// works whether the machine's SSH shell is cmd or PowerShell.
pub(crate) fn settings_command(platform: Option<&str>) -> &'static str {
    match platform {
        Some(p) if p.eq_ignore_ascii_case("windows") => {
            r#"cmd /c type "%LOCALAPPDATA%\Zeron\ui-settings.json""#
        }
        _ => r#"cat "$HOME/.zeron/ui-settings.json""#,
    }
}

/// The local profile's pins in a desktop `ui-settings.json`, in sidebar
/// order. `None` when the text isn't a settings file (then nothing is
/// touched); no pins recorded is an empty list.
pub(crate) fn parse_ui_settings(text: &str) -> Option<Vec<String>> {
    let text = text.trim_start_matches('\u{feff}');
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let settings = value.as_object()?;
    Some(ids(settings
        .get("sidebarPinnedSessionIdsByProfile")
        .and_then(|by| by.get("local"))))
}

/// The pins in a `WatchSidebarPreferences` frame, or `None` when the engine
/// holds none of its own (`initialized: false`: a local profile).
pub(crate) fn parse_engine_preferences(value: &serde_json::Value) -> Option<Vec<String>> {
    value
        .get("initialized")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
        .then(|| ids(value.get("pinnedSessionIds")))
}

fn ids(value: Option<&serde_json::Value>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for id in value.and_then(|v| v.as_array()).into_iter().flatten() {
        if let Some(id) = id.as_str()
            && !id.is_empty()
            && !out.iter().any(|o| o == id)
        {
            out.push(id.to_owned());
        }
    }
    out
}

/// The phone's pins after merging the desktop's: the desktop's (those the
/// phone knows) in desktop order, then the phone's own, minus any the
/// desktop unpinned since `last` (the desktop list merged last time).
pub(crate) fn merge_pins(
    desktop: &[String],
    last: Option<&[String]>,
    phone: &[String],
    known: &HashSet<String>,
) -> Vec<String> {
    let dropped: HashSet<&String> = last
        .unwrap_or_default()
        .iter()
        .filter(|id| !desktop.contains(id))
        .collect();
    let mut target: Vec<String> = Vec::new();
    for id in desktop.iter().filter(|id| known.contains(*id)) {
        if !target.contains(id) {
            target.push(id.clone());
        }
    }
    for id in phone {
        if !dropped.contains(id) && !target.contains(id) {
            target.push(id.clone());
        }
    }
    target
}

/// Make the registry's pins exactly `target`, in that order, touching only
/// what differs.
pub(crate) fn apply_pins(doc: &mut RegistryDoc, target: &[String]) {
    let current = doc
        .sidebar_preferences()
        .map(|p| p.pinned_session_ids)
        .unwrap_or_default();
    for id in current.iter().filter(|id| !target.contains(id)) {
        let _ = doc.change_sidebar_pin(&SidebarPinChange::Unpin {
            session_id: id.clone(),
        });
    }
    let remaining: Vec<&String> = current.iter().filter(|id| target.contains(id)).collect();
    if remaining.len() == target.len() && remaining.iter().zip(target).all(|(a, b)| *a == b) {
        return;
    }
    let mut after: Option<String> = None;
    for id in target {
        let pinned = doc
            .change_sidebar_pin(&SidebarPinChange::Pin {
                session_id: id.clone(),
                after: after.clone(),
                before: None,
            })
            .is_ok();
        if pinned {
            after = Some(id.clone());
        }
    }
}

/// The desktop list merged last time, per machine (`engine` device id).
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct Mirrored {
    pub engine: String,
    pub pins: Vec<String>,
}

pub(crate) const MIRRORED_FILE: &str = "direct-desktop-pins.json";

pub(crate) fn load_mirrored(dir: &std::path::Path, engine: &str) -> Option<Vec<String>> {
    let text = std::fs::read_to_string(dir.join(MIRRORED_FILE)).ok()?;
    let mirrored: Mirrored = serde_json::from_str(&text).ok()?;
    (mirrored.engine == engine).then_some(mirrored.pins)
}

pub(crate) fn save_mirrored(dir: &std::path::Path, engine: &str, pins: &[String]) {
    let mirrored = Mirrored {
        engine: engine.to_owned(),
        pins: pins.to_vec(),
    };
    if let Ok(text) = serde_json::to_string(&mirrored) {
        let _ = std::fs::create_dir_all(dir);
        let tmp = dir.join(format!("{MIRRORED_FILE}.tmp"));
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, dir.join(MIRRORED_FILE));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn the_local_profile_pins_are_read_from_a_desktop_settings_file() {
        // Shaped like Villa's (2026-10-02), with a BOM as `type` may print.
        let text = "\u{feff}{\"theme\":\"dark\",\"sidebarPinnedSessionIdsByProfile\":{\"local\":[\"bf41c941-9aa7-40ad-8a6c-9a6d4e4d2ca7\",\"c6b03354-e4a3-45e2-9b42-44e461c10959\"],\"remote:x\":[\"zz\"]},\"sidebarSectionsByProfile\":{\"local\":[]}}";
        assert_eq!(
            parse_ui_settings(text).unwrap(),
            v(&[
                "bf41c941-9aa7-40ad-8a6c-9a6d4e4d2ca7",
                "c6b03354-e4a3-45e2-9b42-44e461c10959"
            ])
        );
        assert_eq!(parse_ui_settings("{\"theme\":\"dark\"}").unwrap(), v(&[]));
        assert_eq!(
            parse_ui_settings("The system cannot find the path specified."),
            None
        );
        assert_eq!(parse_ui_settings("[1,2]"), None);
    }

    #[test]
    fn engine_preferences_count_only_when_the_engine_holds_pins() {
        let local = serde_json::json!({
            "revision": 0, "synced": false, "initialized": false, "pinnedSessionIds": []
        });
        assert_eq!(parse_engine_preferences(&local), None);
        let synced = serde_json::json!({ "initialized": true, "pinnedSessionIds": ["a", "b"] });
        assert_eq!(parse_engine_preferences(&synced), Some(v(&["a", "b"])));
    }

    #[test]
    fn desktop_pins_merge_with_the_phones_own() {
        let known: HashSet<String> = v(&["a", "b", "c", "d", "p"]).into_iter().collect();
        // First time: the desktop's pins first, the phone's own kept.
        assert_eq!(
            merge_pins(&v(&["a", "b"]), None, &v(&["p"]), &known),
            v(&["a", "b", "p"])
        );
        // The desktop unpinned b and pinned c: b goes, c comes, p stays.
        assert_eq!(
            merge_pins(
                &v(&["a", "c"]),
                Some(&v(&["a", "b"])),
                &v(&["a", "b", "p"]),
                &known
            ),
            v(&["a", "c", "p"])
        );
        // A pin the phone doesn't know (yet) is skipped; duplicates once.
        assert_eq!(
            merge_pins(&v(&["x", "a", "a"]), None, &v(&["a"]), &known),
            v(&["a"])
        );
        // The phone unpinned a desktop pin that the desktop still has: the
        // desktop list changed (c added), so it is pinned again.
        assert_eq!(
            merge_pins(&v(&["a", "c"]), Some(&v(&["a"])), &v(&[]), &known),
            v(&["a", "c"])
        );
    }

    #[test]
    fn the_merged_list_is_written_as_pins_in_order() {
        let mut doc = RegistryDoc::new("android-test".to_owned());
        for id in ["a", "b", "c", "p"] {
            doc.upsert_chat(&chat(id)).unwrap();
        }
        doc.reconcile_sidebar_pins(true).unwrap();
        apply_pins(&mut doc, &v(&["p"]));
        apply_pins(&mut doc, &v(&["a", "b", "p"]));
        assert_eq!(pins(&doc), v(&["a", "b", "p"]));
        apply_pins(&mut doc, &v(&["c", "a", "p"]));
        assert_eq!(pins(&doc), v(&["c", "a", "p"]));
        apply_pins(&mut doc, &v(&[]));
        assert_eq!(pins(&doc), v(&[]));
    }

    fn pins(doc: &RegistryDoc) -> Vec<String> {
        doc.sidebar_preferences().unwrap().pinned_session_ids
    }

    fn chat(id: &str) -> zeron_proto::Chat {
        serde_json::from_value(serde_json::json!({
            "id": id, "spaceId": "s", "deviceId": "pc", "title": id, "archived": false,
            "createdAt": "2026-10-01T00:00:00Z", "updatedAt": "2026-10-01T00:00:00Z"
        }))
        .unwrap()
    }
}
