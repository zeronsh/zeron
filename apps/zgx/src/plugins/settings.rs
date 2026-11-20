//! Each plugin's settings: declared in its manifest, drawn by Zeron (a page
//! of its own in Settings, under Plugins, while it's on), kept for it alone.
//!
//! A manifest lists them under `settings`, each `{ id, kind, label,
//! description?, default?, section? }` and, by kind:
//!
//! - `switch`: on or off (`default` a boolean, else off);
//! - `choice`: one of `options` (`[{ value, label }]`, values text), drawn
//!   as a `style` of `"select"` (the default) or `"segmented"`;
//! - `text`: words (`placeholder?`), at most [`MAX_TEXT`] characters;
//! - `number`: `min?`, `max?`, `step?`, `unit?` (said after the field);
//! - `shortcut`: a key combination as Zeron writes them (`ctrl+alt+space`),
//!   or none (`""`);
//! - `folder`: a folder you choose (its absolute path), or none (`""`);
//! - `permission` (Zeron's own plugins alone): a macOS permission row,
//!   `{ kind, permission }` with `permission` one of [`PERMISSIONS`]. Zeron
//!   says everything on it (its words, whether it's granted, the way to
//!   grant it); it has no value. Anyone else's is left out.
//! - `wallpaper`: the row that uses the plugin's drawing as a new chat's
//!   wallpaper, `{ kind, section? }`, for a plugin that replaces a new chat's
//!   background (`#new-chat-background` in its `ui.change`; anyone else's
//!   is left out). Zeron says everything on it and keeps what's chosen (the
//!   app's: one plugin's wallpaper at a time, with Zeron's effect over it);
//!   it has no value of the plugin's.
//!
//! A choice's option may say what it does (`description`), said in the
//! setting's place while it's chosen. The manifest's top-level
//! `settings_description` (plain text, [`MAX_PAGE_DESCRIPTION`] characters
//! at most) is said under the page's title in Zeron's generic line's place.
//!
//! A plugin may report a status for any of its settings
//! (`plugin.<id>.setting_status { id, badge, tone, text? }`, the SDK's
//! `zeron.set_status`): a short badge in a fixed tone ([`TONES`]) and words
//! in the setting's description's place, plain text, drawn by Zeron beside
//! the control. Held while the plugin is on; it goes when it's off.
//!
//! What you set is kept in `<data>/plugins/.storage/<id>.settings.json`,
//! and only what you set: a setting you never touched follows the
//! manifest's default, so a default the plugin changes reaches you. A value
//! that no longer fits its setting (an option dropped, a kind changed) is
//! ignored for its default. Removing the plugin removes the file (the
//! engine's `RemovePlugin`, and the app's Remove).
//!
//! The plugin reads its values (with defaults) from `plugin.<id>.settings`,
//! its own and read only; only Zeron's page sets them (`plugins.setting_set`,
//! an action no plugin reaches).

use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
};

use serde_json::{Map, Value as Json, json};

/// The most settings a plugin declares (the rest are left out).
pub const MAX_SETTINGS: usize = 40;
/// The most options a choice has.
pub const MAX_OPTIONS: usize = 24;
/// The most characters a text setting holds.
pub const MAX_TEXT: usize = 2000;
const MAX_LABEL: usize = 80;
const MAX_DESCRIPTION: usize = 400;
const KINDS: &[&str] = &["switch", "choice", "text", "number", "shortcut", "folder", "permission", "wallpaper"];
/// The place a plugin replaces to draw a new chat's wallpaper.
pub const WALLPAPER_PLACE: &str = "#new-chat-background";

/// Whether a row of `kind` is Zeron's own words, with no value of the
/// plugin's (a permission's, the wallpaper's).
fn zeron_s(kind: &str) -> bool {
    kind == "permission" || kind == "wallpaper"
}
/// The permissions a `permission` row can be.
pub const PERMISSIONS: &[&str] = &["screen-recording", "accessibility"];
/// A status's tones.
pub const TONES: &[&str] = &["neutral", "success", "warning", "danger"];
const MAX_BADGE: usize = 24;
const MAX_STATUS_TEXT: usize = 200;
/// The most characters a page's own description says.
pub const MAX_PAGE_DESCRIPTION: usize = 200;

/// What a manifest says under its page's title (`settings_description`),
/// plain, or none.
pub fn page_description(manifest: &Json) -> Option<String> {
    let text = plain(manifest["settings_description"].as_str()?, MAX_PAGE_DESCRIPTION);
    (!text.is_empty()).then_some(text)
}

/// One declared setting, as the page draws it (its manifest's fields,
/// checked and filled in: a default that fits, a choice's style).
#[derive(Clone, Debug, PartialEq)]
pub struct Setting {
    pub id: String,
    pub kind: String,
    pub default: Json,
    /// Everything the page reads (label, description, options, …).
    pub json: Json,
}

fn short(value: Option<&Json>, most: usize) -> Option<String> {
    let s = value?.as_str()?.trim();
    (!s.is_empty()).then(|| s.chars().take(most).collect())
}

fn number(value: Option<&Json>) -> Option<f64> {
    value?.as_f64().filter(|n| n.is_finite())
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// A key combination as Zeron writes them: modifiers then a key, `+`
/// between, at least one of Control, Option or Command held.
pub fn valid_shortcut(combo: &str) -> bool {
    let parts: Vec<&str> = combo.split('+').collect();
    let Some((key, mods)) = parts.split_last() else { return false };
    let known = ["ctrl", "alt", "shift", "cmd"];
    let held = mods.iter().any(|m| matches!(*m, "ctrl" | "alt" | "cmd"));
    let key_ok = !key.is_empty() && key.len() <= 10 && key.chars().all(|c| c.is_ascii_alphanumeric()) && !known.contains(key);
    held && key_ok && mods.iter().all(|m| known.contains(m)) && combo.len() <= 40
}

/// `value` as `setting` holds it, or none where it doesn't fit.
pub fn fit(setting: &Setting, value: &Json) -> Option<Json> {
    let j = &setting.json;
    match setting.kind.as_str() {
        "switch" => value.as_bool().map(Json::Bool),
        "choice" => {
            let v = value.as_str()?;
            j["options"].as_array()?.iter().any(|o| o["value"] == v).then(|| json!(v))
        }
        "text" => value.as_str().map(|s| json!(s.chars().take(MAX_TEXT).collect::<String>())),
        "number" => {
            let mut n = number(Some(value))?;
            if let Some(min) = number(j.get("min")) {
                n = n.max(min);
            }
            if let Some(max) = number(j.get("max")) {
                n = n.min(max);
            }
            Some(json!(n))
        }
        "shortcut" => value.as_str().filter(|s| s.is_empty() || valid_shortcut(s)).map(|s| json!(s)),
        "folder" => value.as_str().filter(|s| s.is_empty() || (Path::new(s).is_absolute() && s.len() <= 4096)).map(|s| json!(s)),
        _ => None,
    }
}

/// The settings `manifest` declares, each checked: one that isn't a
/// setting (no id, an unknown kind, a choice with no options, an id used
/// already, a permission row of a plugin not Zeron's own: `builtin`) is
/// left out, and said once on stderr.
pub fn declared(plugin: &str, manifest: &Json, builtin: bool) -> Vec<Setting> {
    let mut out: Vec<Setting> = Vec::new();
    let Some(list) = manifest["settings"].as_array() else { return out };
    for raw in list.iter().take(MAX_SETTINGS) {
        let skip = |why: &str| eprintln!("zeron-zgx: plugin {plugin}: a setting left out: {why}");
        // A permission row's id is its permission, unless it says another.
        let id = raw["id"].as_str().or(match raw["kind"].as_str() {
            Some("permission") => raw["permission"].as_str(),
            Some("wallpaper") => Some("wallpaper"),
            _ => None,
        });
        let Some(id) = id.filter(|id| valid_id(id)) else {
            skip("its id must be letters, digits, - _ or .");
            continue;
        };
        if out.iter().any(|s| s.id == id) {
            skip(&format!("{id} is declared twice"));
            continue;
        }
        let Some(kind) = raw["kind"].as_str().filter(|k| KINDS.contains(k)) else {
            skip(&format!("{id}'s kind must be one of {}", KINDS.join(", ")));
            continue;
        };
        if kind == "wallpaper" {
            let replaces = manifest["ui"]["change"].as_array().is_some_and(|l| l.iter().any(|s| s.as_str() == Some(WALLPAPER_PLACE)));
            if !replaces {
                skip(&format!("{id}: a wallpaper row needs \"{WALLPAPER_PLACE}\" in its ui.change"));
                continue;
            }
            if out.iter().any(|s| s.kind == "wallpaper") {
                skip(&format!("{id}: a plugin has one wallpaper row"));
                continue;
            }
            // Nothing of the plugin's own on it: Zeron says it all.
            let mut json = json!({ "id": id, "kind": kind });
            if let Some(s) = short(raw.get("section"), MAX_LABEL) {
                json["section"] = json!(s);
            }
            out.push(Setting { id: id.to_string(), kind: kind.to_string(), default: Json::Null, json });
            continue;
        }
        if kind == "permission" {
            if !builtin {
                skip(&format!("{id}: a permission row is Zeron's own plugins' alone"));
                continue;
            }
            let Some(permission) = raw["permission"].as_str().filter(|p| PERMISSIONS.contains(p)) else {
                skip(&format!("{id}'s permission must be one of {}", PERMISSIONS.join(", ")));
                continue;
            };
            // Nothing of the plugin's own on it: Zeron says it all.
            let mut json = json!({ "id": id, "kind": kind, "permission": permission });
            if let Some(s) = short(raw.get("section"), MAX_LABEL) {
                json["section"] = json!(s);
            }
            out.push(Setting { id: id.to_string(), kind: kind.to_string(), default: Json::Null, json });
            continue;
        }
        let label = short(raw.get("label"), MAX_LABEL).unwrap_or_else(|| id.to_string());
        let mut json = json!({ "id": id, "kind": kind, "label": label });
        if let Some(d) = short(raw.get("description"), MAX_DESCRIPTION) {
            json["description"] = json!(d);
        }
        if let Some(s) = short(raw.get("section"), MAX_LABEL) {
            json["section"] = json!(s);
        }
        match kind {
            "choice" => {
                let options: Vec<Json> = raw["options"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|o| {
                        let value = o["value"].as_str().filter(|v| !v.is_empty() && v.len() <= 200)?;
                        let label = short(o.get("label"), MAX_LABEL).unwrap_or_else(|| value.to_string());
                        let mut option = json!({ "value": value, "label": label });
                        if let Some(d) = short(o.get("description"), MAX_DESCRIPTION) {
                            option["description"] = json!(d);
                        }
                        Some(option)
                    })
                    .take(MAX_OPTIONS)
                    .collect();
                if options.is_empty() {
                    skip(&format!("{id} is a choice with no options"));
                    continue;
                }
                json["options"] = Json::Array(options);
                json["style"] = json!(if raw["style"] == "segmented" { "segmented" } else { "select" });
            }
            "text" => {
                if let Some(p) = short(raw.get("placeholder"), MAX_LABEL) {
                    json["placeholder"] = json!(p);
                }
            }
            "number" => {
                for key in ["min", "max", "step"] {
                    if let Some(n) = number(raw.get(key)) {
                        json[key] = json!(n);
                    }
                }
                if let (Some(min), Some(max)) = (number(json.get("min")), number(json.get("max")))
                    && min > max
                {
                    skip(&format!("{id}'s min is over its max"));
                    continue;
                }
                if let Some(u) = short(raw.get("unit"), 24) {
                    json["unit"] = json!(u);
                }
            }
            _ => {}
        }
        let mut setting = Setting { id: id.to_string(), kind: kind.to_string(), default: Json::Null, json };
        // Its default: the manifest's where it fits, else the kind's own
        // (off, the first option, nothing, the least it takes).
        let fallback = match kind {
            "switch" => json!(false),
            "choice" => setting.json["options"][0]["value"].clone(),
            // (Nought, within its bounds.)
            "number" => fit(&setting, &json!(0.0)).unwrap_or(json!(0.0)),
            _ => json!(""),
        };
        let default = raw.get("default").and_then(|d| fit(&setting, d));
        if raw.get("default").is_some_and(|d| !d.is_null()) && default.is_none() {
            skip(&format!("{id}'s default doesn't fit it; it starts from {fallback}"));
        }
        setting.default = default.unwrap_or(fallback);
        setting.json["default"] = setting.default.clone();
        out.push(setting);
    }
    out
}

/// `declared`'s values: what was set where it still fits, else its default
/// (a permission row has none).
pub fn values(declared: &[Setting], stored: &Map<String, Json>) -> Map<String, Json> {
    declared
        .iter()
        .filter(|s| !zeron_s(&s.kind))
        .map(|s| (s.id.clone(), stored.get(&s.id).and_then(|v| fit(s, v)).unwrap_or_else(|| s.default.clone())))
        .collect()
}

/// One line of plain text, at most `most` characters: control characters
/// (new lines among them) become spaces.
fn plain(text: &str, most: usize) -> String {
    let flat: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let words: Vec<&str> = flat.split_whitespace().collect();
    let cut: String = words.join(" ").chars().take(most).collect();
    cut.trim_end().to_string()
}

/// A status a plugin reports for `setting`, as Zeron draws it: `{ badge,
/// tone, text? }` (none: cleared, an empty badge and text clear it too),
/// or why not.
pub fn status(setting: &Setting, asked: &Json) -> Result<Option<Json>, String> {
    if zeron_s(&setting.kind) {
        return Err(format!("A {} row's status is Zeron's", setting.kind));
    }
    if asked.is_null() {
        return Ok(None);
    }
    let badge = plain(asked["badge"].as_str().unwrap_or_default(), MAX_BADGE);
    let text = plain(asked["text"].as_str().unwrap_or_default(), MAX_STATUS_TEXT);
    if badge.is_empty() && text.is_empty() {
        return Ok(None);
    }
    let tone = asked["tone"].as_str().unwrap_or("neutral");
    if !TONES.contains(&tone) {
        return Err(format!("A status's tone is one of {}", TONES.join(", ")));
    }
    let mut out = json!({ "tone": tone });
    if !badge.is_empty() {
        out["badge"] = json!(badge);
    }
    if !text.is_empty() {
        out["text"] = json!(text);
    }
    Ok(Some(out))
}

/// Where plugin `id`'s settings are kept, under `dir` (`.storage`).
pub fn path_in(dir: &Path, id: &str) -> Option<PathBuf> {
    let safe = !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    safe.then(|| dir.join(format!("{id}.settings.json")))
}

/// `<data>/plugins/.storage`, where each plugin's storage is kept.
pub fn storage_dir() -> Option<PathBuf> {
    crate::engine::data_dir().map(|d| d.join("plugins").join(".storage"))
}

fn load(dir: &Path, id: &str) -> Map<String, Json> {
    path_in(dir, id)
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str::<Json>(&t).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

fn save(dir: &Path, id: &str, held: &Map<String, Json>) -> Result<(), String> {
    let path = path_in(dir, id).ok_or("there's nowhere to keep it")?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    if held.is_empty() {
        let _ = std::fs::remove_file(&path);
        return Ok(());
    }
    let text = serde_json::to_string(held).map_err(|e| e.to_string())?;
    let aside = path.with_extension("json.new");
    std::fs::write(&aside, text).map_err(|e| e.to_string())?;
    std::fs::rename(&aside, &path).map_err(|e| e.to_string())
}

/// Plugin `id`'s settings let go of (it was removed).
pub fn forget_in(dir: &Path, id: &str) {
    if let Some(path) = path_in(dir, id) {
        let _ = std::fs::remove_file(path);
    }
}

/// One plugin's settings as the window holds them.
#[derive(Clone, Debug, Default)]
struct Held {
    name: String,
    /// What it was read as (the engine's content hash): read again only as
    /// it changes.
    hash: String,
    declared: Vec<Setting>,
    /// What its page says under its title (`settings_description`).
    description: Option<String>,
    stored: Map<String, Json>,
    /// What the plugin reports of its settings, by id (`status`): held
    /// while it's on.
    status: Map<String, Json>,
}

/// Every plugin's settings: the plugins on now, what each declares, and
/// what you set.
#[derive(Default)]
pub struct Book {
    dir: Option<PathBuf>,
    held: BTreeMap<String, Held>,
}

/// A plugin on now, as `Book::sync` follows it.
#[derive(Clone, Debug, Default)]
pub struct On {
    pub id: String,
    pub name: String,
    /// What it was read as (the engine's content hash).
    pub hash: String,
    /// One of Zeron's own (shipped inside the app): it may declare
    /// permission rows.
    pub builtin: bool,
}

/// What `sync` found, for the window to publish.
#[derive(Debug, Default, PartialEq)]
pub struct Changed {
    /// The list the Settings window draws changed.
    pub list: bool,
    /// These plugins' values changed (each one's `plugin.<id>.settings`).
    pub values: Vec<String>,
}

impl Book {
    /// Settings kept under `dir` (`.storage`; none: nowhere, nothing kept).
    pub fn new(dir: Option<PathBuf>) -> Self {
        Self { dir, held: BTreeMap::new() }
    }

    /// Follows the plugins on now (`(id, name, hash)`), returning what
    /// changed; `read` gives one's manifest (from its snapshot), asked only
    /// as its hash changes. A plugin no longer on goes from the list; its
    /// values stay on disk for when it's back.
    pub fn sync(&mut self, on: &[On], read: impl Fn(&str) -> Option<Json>) -> Changed {
        let mut changed = Changed::default();
        let keep: HashMap<&str, ()> = on.iter().map(|o| (o.id.as_str(), ())).collect();
        let before = self.held.len();
        self.held.retain(|id, _| keep.contains_key(id.as_str()));
        changed.list |= self.held.len() != before;
        for On { id, name, hash, builtin } in on {
            let same = self.held.get(id).is_some_and(|h| h.hash == *hash && h.name == *name);
            if same {
                continue;
            }
            let manifest = read(id);
            let declared = manifest.as_ref().map(|m| declared(id, m, *builtin)).unwrap_or_default();
            let description = manifest.as_ref().and_then(page_description);
            let was = self.held.get(id).map(|h| values(&h.declared, &h.stored));
            let stored = match self.held.get(id) {
                Some(h) => h.stored.clone(),
                None => self.dir.as_deref().map(|d| load(d, id)).unwrap_or_default(),
            };
            let had = self.held.get(id).map(|h| (h.name.clone(), h.declared.clone(), h.description.clone()));
            // What it reported of settings it still declares.
            let mut status = self.held.get(id).map(|h| h.status.clone()).unwrap_or_default();
            status.retain(|k, _| declared.iter().any(|s| s.id == *k && !zeron_s(&s.kind)));
            let held = Held { name: name.clone(), hash: hash.clone(), declared, description, stored, status };
            if had.as_ref() != Some(&(held.name.clone(), held.declared.clone(), held.description.clone())) {
                changed.list = true;
            }
            if was.as_ref() != Some(&values(&held.declared, &held.stored)) {
                changed.values.push(id.clone());
            }
            self.held.insert(id.clone(), held);
        }
        changed
    }

    /// Plugin `id`'s values, defaults filled in (none declared: empty).
    pub fn values(&self, id: &str) -> Map<String, Json> {
        self.held.get(id).map(|h| values(&h.declared, &h.stored)).unwrap_or_default()
    }

    /// The values plugin `id` starts from before it's synced (its manifest
    /// read now, what you set read from disk).
    pub fn values_from(&self, id: &str, manifest: Option<&Json>) -> Map<String, Json> {
        if self.held.contains_key(id) {
            return self.values(id);
        }
        let declared = manifest.map(|m| declared(id, m, false)).unwrap_or_default();
        let stored = self.dir.as_deref().map(|d| load(d, id)).unwrap_or_default();
        values(&declared, &stored)
    }

    /// Sets plugin `plugin`'s setting `id` (null: back to its default),
    /// kept on disk at once; says why not.
    pub fn set(&mut self, plugin: &str, id: &str, value: &Json) -> Result<(), String> {
        let held = self.held.get_mut(plugin).ok_or("That plugin isn't on")?;
        let setting = held.declared.iter().find(|s| s.id == id).ok_or("It has no such setting")?;
        let mut next = held.stored.clone();
        if value.is_null() {
            next.remove(id);
        } else {
            next.insert(id.to_string(), fit(setting, value).ok_or("That doesn't fit the setting")?);
        }
        if let Some(dir) = &self.dir {
            save(dir, plugin, &next)?;
        }
        held.stored = next;
        Ok(())
    }

    /// Plugin `plugin` reports a status for its setting `id` (null, or an
    /// empty one: cleared); says why not. Whether anything changed.
    pub fn set_status(&mut self, plugin: &str, id: &str, asked: &Json) -> Result<bool, String> {
        let held = self.held.get_mut(plugin).ok_or("That plugin isn't on")?;
        let setting = held.declared.iter().find(|s| s.id == id).ok_or("It has no such setting")?;
        let next = status(setting, asked)?;
        let was = held.status.get(id).cloned();
        match &next {
            Some(v) => held.status.insert(id.to_string(), v.clone()),
            None => held.status.remove(id),
        };
        Ok(was != next)
    }

    /// Plugin `id`'s settings let go of, on disk too (it was removed).
    pub fn forget(&mut self, id: &str) {
        self.held.remove(id);
        if let Some(dir) = &self.dir {
            forget_in(dir, id);
        }
    }

    /// The plugins on with settings, by name (then id), each with what it
    /// declares and its values: what Settings draws a page of each from.
    pub fn list(&self) -> Json {
        let mut list: Vec<(&String, &Held)> = self.held.iter().filter(|(_, h)| !h.declared.is_empty()).collect();
        list.sort_by(|a, b| a.1.name.to_lowercase().cmp(&b.1.name.to_lowercase()).then_with(|| a.0.cmp(b.0)));
        Json::Array(
            list.into_iter()
                .map(|(id, h)| {
                    json!({
                        "plugin": id,
                        "name": h.name,
                        "settings": h.declared.iter().map(|s| s.json.clone()).collect::<Vec<_>>(),
                        "values": values(&h.declared, &h.stored),
                        "status": h.status,
                        "description": h.description,
                    })
                })
                .collect(),
        )
    }
}

/// A plugin's manifest, from its snapshot (what the engine checked).
pub fn manifest_in(snapshot: &Path) -> Option<Json> {
    let text = std::fs::read_to_string(snapshot.join("zeron-plugin.json")).ok()?;
    serde_json::from_str(&text).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> Json {
        json!({
            "id": "demo", "name": "Demo", "version": "1", "entry": "init.luau",
            "settings": [
                { "id": "on", "kind": "switch", "label": "Greet", "default": true },
                { "id": "tone", "kind": "choice", "label": "Tone", "style": "segmented",
                  "options": [ { "value": "warm", "label": "Warm" }, { "value": "dry", "label": "Dry" } ], "default": "warm" },
                { "id": "greeting", "kind": "text", "label": "Greeting", "default": "Hello" },
                { "id": "every", "kind": "number", "label": "Every", "min": 30, "max": 600, "unit": "seconds", "default": 60 },
                { "id": "keys", "kind": "shortcut", "label": "Shortcut", "default": "ctrl+alt+g" },
                { "id": "out", "kind": "folder", "label": "Save to" },
                { "id": "bad", "kind": "colour", "label": "Nope" },
                { "id": "empty", "kind": "choice", "label": "No options" },
                { "id": "on", "kind": "switch", "label": "Twice" },
            ],
        })
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zgx-settings-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// `book` following `list` (`(id, name, hash, manifest)`).
    fn sync(book: &mut Book, list: &[(&str, &str, &str, Json)]) -> Changed {
        let on: Vec<On> = list.iter().map(|(i, n, h, _)| On { id: i.to_string(), name: n.to_string(), hash: h.to_string(), builtin: false }).collect();
        book.sync(&on, |id| list.iter().find(|(i, ..)| *i == id).map(|(.., m)| m.clone()))
    }

    #[test]
    fn a_manifest_declares_its_settings_each_checked() {
        let d = declared("demo", &manifest(), false);
        let ids: Vec<&str> = d.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["on", "tone", "greeting", "every", "keys", "out"], "unknown kinds, empty choices and repeats are left out");
        assert_eq!(d[1].json["style"], "segmented");
        assert_eq!(d[3].json["unit"], "seconds");
        assert_eq!(d[5].default, json!(""), "a folder starts with none");
        // A default that doesn't fit falls back to the kind's own.
        let m = json!({ "settings": [{ "id": "n", "kind": "number", "min": 5, "default": "six" }, { "id": "c", "kind": "choice", "options": [{ "value": "a" }], "default": "z" }] });
        let d = declared("x", &m, false);
        assert_eq!((d[0].default.clone(), d[1].default.clone()), (json!(5.0), json!("a")));
        assert_eq!(d[1].json["options"][0]["label"], "a", "an option's label defaults to its value");
    }

    #[test]
    fn values_fit_their_setting_or_fall_back() {
        let d = declared("demo", &manifest(), false);
        let tone = &d[1];
        assert_eq!(fit(tone, &json!("dry")), Some(json!("dry")));
        assert_eq!(fit(tone, &json!("loud")), None);
        assert_eq!(fit(&d[3], &json!(9000)), Some(json!(600.0)), "a number is kept within its bounds");
        assert_eq!(fit(&d[4], &json!("cmd+shift+k")), Some(json!("cmd+shift+k")));
        assert_eq!(fit(&d[4], &json!("shift+k")), None, "a shortcut holds Control, Option or Command");
        assert_eq!(fit(&d[4], &json!("")), Some(json!("")), "or is none");
        assert_eq!(fit(&d[5], &json!("relative/path")), None);
        // A folder is absolute as this platform reads it.
        let pictures = if cfg!(windows) { r"C:\Users\me\Pictures" } else { "/Users/me/Pictures" };
        assert_eq!(fit(&d[5], &json!(pictures)), Some(json!(pictures)));
        if cfg!(windows) {
            assert_eq!(fit(&d[5], &json!("/Users/me/Pictures")), None, "rooted, but on no drive");
        }
        assert_eq!(fit(&d[0], &json!("yes")), None);
    }

    #[test]
    fn a_plugin_s_page_comes_and_goes_with_it_and_its_values_stay() {
        let dir = scratch("comes-goes");
        let mut book = Book::new(Some(dir.clone()));
        let changed = sync(&mut book, &[("demo", "Demo", "h1", manifest())]);
        assert!(changed.list && changed.values == ["demo"]);
        assert_eq!(book.list().as_array().unwrap().len(), 1);
        book.set("demo", "greeting", &json!("Hi")).unwrap();
        // Turned off: its page goes; what you set stays.
        let changed = sync(&mut book, &[]);
        assert!(changed.list);
        assert_eq!(book.list(), json!([]));
        assert!(book.set("demo", "greeting", &json!("x")).is_err(), "one that's off isn't set");
        // Back on: as you left it.
        sync(&mut book, &[("demo", "Demo", "h1", manifest())]);
        assert_eq!(book.values("demo")["greeting"], "Hi");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn what_you_set_survives_a_restart_and_only_that_is_kept() {
        let dir = scratch("restart");
        let mut book = Book::new(Some(dir.clone()));
        sync(&mut book, &[("demo", "Demo", "h1", manifest())]);
        book.set("demo", "tone", &json!("dry")).unwrap();
        assert!(book.set("demo", "tone", &json!("loud")).is_err());
        let kept: Json = serde_json::from_str(&std::fs::read_to_string(dir.join("demo.settings.json")).unwrap()).unwrap();
        assert_eq!(kept, json!({ "tone": "dry" }), "defaults aren't written down");
        // A new run.
        let mut again = Book::new(Some(dir.clone()));
        assert_eq!(again.values_from("demo", Some(&manifest()))["tone"], "dry");
        sync(&mut again, &[("demo", "Demo", "h1", manifest())]);
        assert_eq!(again.values("demo")["tone"], "dry");
        // Back to its default: nothing kept, the file gone.
        again.set("demo", "tone", &Json::Null).unwrap();
        assert_eq!(again.values("demo")["tone"], "warm");
        assert!(!dir.join("demo.settings.json").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_changed_default_reaches_whoever_never_set_it() {
        let dir = scratch("defaults");
        let mut book = Book::new(Some(dir.clone()));
        sync(&mut book, &[("demo", "Demo", "h1", manifest())]);
        book.set("demo", "tone", &json!("dry")).unwrap();
        let mut m = manifest();
        m["settings"][2]["default"] = json!("Howdy");
        m["settings"][1]["default"] = json!("warm");
        let changed = sync(&mut book, &[("demo", "Demo", "h2", m.clone())]);
        assert_eq!(changed.values, ["demo"]);
        let v = book.values("demo");
        assert_eq!((v["greeting"].as_str(), v["tone"].as_str()), (Some("Howdy"), Some("dry")), "the new default, what you set kept");
        // An option dropped: what you'd chosen goes back to the default.
        m["settings"][1]["options"] = json!([{ "value": "warm" }]);
        sync(&mut book, &[("demo", "Demo", "h3", m)]);
        assert_eq!(book.values("demo")["tone"], "warm");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn each_plugin_s_are_its_own_and_go_when_it_s_removed() {
        let dir = scratch("own");
        let mut book = Book::new(Some(dir.clone()));
        let mut other = manifest();
        other["name"] = json!("Another");
        sync(&mut book, &[("demo", "Demo", "h1", manifest()), ("another", "Another", "h1", other)]);
        book.set("demo", "greeting", &json!("Mine")).unwrap();
        assert_eq!(book.values("another")["greeting"], "Hello");
        let names: Vec<String> = book.list().as_array().unwrap().iter().map(|p| p["name"].as_str().unwrap().to_string()).collect();
        assert_eq!(names, ["Another", "Demo"], "by name");
        book.forget("demo");
        assert!(!dir.join("demo.settings.json").exists());
        assert!(path_in(&dir, "../etc").is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_plugin_with_no_settings_has_no_page() {
        let mut book = Book::new(None);
        sync(&mut book, &[("plain", "Plain", "h", json!({ "id": "plain" }))]);
        assert_eq!(book.list(), json!([]));
        assert!(book.values("plain").is_empty());
    }

    #[test]
    fn an_option_may_say_what_it_does() {
        let m = json!({ "settings": [{ "id": "dest", "kind": "choice", "options": [
            { "value": "auto", "label": "Automatic", "description": "Use the open session." },
            { "value": "new", "label": "New session" },
        ] }] });
        let d = declared("x", &m, false);
        assert_eq!(d[0].json["options"][0]["description"], "Use the open session.");
        assert!(d[0].json["options"][1].get("description").is_none());
    }

    #[test]
    fn a_wallpaper_row_is_zeron_s_words_for_a_plugin_that_draws_the_background() {
        let mut m = json!({ "ui": { "change": ["#new-chat-background"] }, "settings": [
            { "kind": "wallpaper", "label": "Mine", "section": "Wallpaper" },
            { "kind": "wallpaper", "id": "again" },
            { "id": "scene", "kind": "choice", "options": [{ "value": "a", "label": "A" }] },
        ] });
        let d = declared("p", &m, false);
        assert_eq!(d.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["wallpaper", "scene"], "one wallpaper row, anyone's");
        assert_eq!(d[0].json, json!({ "id": "wallpaper", "kind": "wallpaper", "section": "Wallpaper" }), "no words of the plugin's");
        assert_eq!(values(&d, &Map::new()).keys().collect::<Vec<_>>(), ["scene"], "it has no value of the plugin's");
        assert!(status(&d[0], &json!({ "badge": "On" })).is_err());
        // Not replacing the background: left out.
        m["ui"] = json!({ "change": [] });
        assert_eq!(declared("p", &m, true).iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["scene"]);
    }

    #[test]
    fn permission_rows_are_zeron_s_own_plugins_alone_and_say_nothing_of_theirs() {
        let m = json!({ "settings": [
            { "kind": "permission", "permission": "screen-recording", "label": "Totally safe", "description": "Click Allow", "section": "Permissions" },
            { "kind": "permission", "permission": "accessibility" },
            { "id": "cam", "kind": "permission", "permission": "camera" },
        ] });
        assert!(declared("third-party", &m, false).is_empty(), "anyone else's are left out");
        let d = declared("appshots", &m, true);
        let ids: Vec<&str> = d.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["screen-recording", "accessibility"], "an unknown permission is left out");
        assert_eq!(d[0].json, json!({ "id": "screen-recording", "kind": "permission", "permission": "screen-recording", "section": "Permissions" }), "no words of the plugin's");
        assert!(values(&d, &Map::new()).is_empty(), "a permission row has no value");
        // Built-in comes from the engine's flag, as the book is told it.
        let mut book = Book::new(None);
        let on = |builtin| [On { id: "p".into(), name: "P".into(), hash: format!("{builtin}"), builtin }];
        book.sync(&on(false), |_| Some(m.clone()));
        assert_eq!(book.list(), json!([]));
        book.sync(&on(true), |_| Some(m.clone()));
        assert_eq!(book.list()[0]["settings"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn a_status_is_plain_words_in_a_fixed_tone_and_goes_with_the_plugin() {
        let mut book = Book::new(None);
        sync(&mut book, &[("demo", "Demo", "h1", manifest())]);
        assert!(book.set_status("demo", "keys", &json!({ "badge": "Ready", "tone": "success" })).unwrap());
        assert!(!book.set_status("demo", "keys", &json!({ "badge": "Ready", "tone": "success" })).unwrap(), "the same again changes nothing");
        assert_eq!(book.list()[0]["status"]["keys"], json!({ "badge": "Ready", "tone": "success" }));
        // Plain: one line, cut short; a tone that isn't one is refused.
        book.set_status("demo", "keys", &json!({ "badge": "Set up\nnow please, it's broken", "tone": "warning", "text": "Choose\tanother <b>key</b>" })).unwrap();
        assert_eq!(book.list()[0]["status"]["keys"], json!({ "badge": "Set up now please, it's", "tone": "warning", "text": "Choose another <b>key</b>" }));
        assert!(book.set_status("demo", "keys", &json!({ "badge": "x", "tone": "blink" })).is_err());
        assert!(book.set_status("demo", "nope", &json!({ "badge": "x" })).is_err());
        // Cleared by null, or by nothing to say.
        book.set_status("demo", "keys", &Json::Null).unwrap();
        assert!(book.list()[0]["status"].as_object().unwrap().is_empty());
        book.set_status("demo", "tone", &json!({ "badge": "Beta" })).unwrap();
        assert_eq!(book.list()[0]["status"]["tone"]["tone"], "neutral");
        book.set_status("demo", "tone", &json!({ "badge": "", "text": "  " })).unwrap();
        assert!(book.list()[0]["status"].get("tone").is_none());
        // Off: gone. On again: none until it says so again.
        book.set_status("demo", "keys", &json!({ "badge": "Ready", "tone": "success" })).unwrap();
        sync(&mut book, &[]);
        assert!(book.set_status("demo", "keys", &json!({ "badge": "Ready" })).is_err(), "one that's off reports nothing");
        sync(&mut book, &[("demo", "Demo", "h1", manifest())]);
        assert!(book.list()[0]["status"].as_object().unwrap().is_empty());
    }

    /// Appshots' page today (ui/screens/settings/appshots.luau), written as
    /// a manifest's settings (not wired in): every row declarable, as one of
    /// Zeron's own plugins.
    #[test]
    fn appshots_page_is_declarable() {
        let m: Json = serde_json::from_str(r#"{"settings_description": "Set up once, one permission at a time. Screen Recording captures the window; Accessibility optionally adds off-screen application text.", "settings": [{"id": "enabled", "kind": "switch", "section": "Capture", "label": "Capture Appshots", "description": "Captures are staged for review and never sent automatically.", "default": false}, {"id": "sound", "kind": "switch", "section": "Capture", "label": "Capture sound", "description": "Play a sound when an Appshot is ready.", "default": true}, {"id": "shortcut", "kind": "shortcut", "section": "Capture", "label": "Global shortcut", "description": "The shortcut works while another application has focus.", "default": "ctrl+alt+space"}, {"id": "destination", "kind": "choice", "style": "select", "section": "Capture", "label": "Destination", "default": "automatic", "options": [{"value": "automatic", "label": "Automatic", "description": "Use the open session, or the new-session composer when no session is open."}, {"value": "last", "label": "Last session", "description": "Use the open session, or return to the last session used for an Appshot."}, {"value": "new", "label": "New session", "description": "Stage captures in a new-session composer, keeping existing drafts intact."}]}, {"kind": "permission", "permission": "screen-recording", "section": "Permissions"}, {"kind": "permission", "permission": "accessibility", "section": "Permissions"}]}"#).unwrap();
        let d = declared("appshots", &m, true);
        let ids: Vec<&str> = d.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["enabled", "sound", "shortcut", "destination", "screen-recording", "accessibility"], "nothing left out");
        assert_eq!(page_description(&m).as_deref(), Some("Set up once, one permission at a time. Screen Recording captures the window; Accessibility optionally adds off-screen application text."), "its intro, whole");
        let mut book = Book::new(None);
        book.sync(&[On { id: "appshots".into(), name: "Appshots".into(), hash: "h".into(), builtin: true }], |_| Some(m.clone()));
        let v = book.values("appshots");
        assert_eq!((v["enabled"].clone(), v["sound"].clone(), v["shortcut"].clone(), v["destination"].clone()), (json!(false), json!(true), json!("ctrl+alt+space"), json!("automatic")));
        // The shortcut's registration, as Appshots would report it.
        assert!(book.set_status("appshots", "shortcut", &json!({ "badge": "Set up", "tone": "warning", "text": "This shortcut is unavailable. Choose a different key combination." })).unwrap());
        assert!(book.set_status("appshots", "screen-recording", &json!({ "badge": "Ready" })).is_err(), "a permission's status is Zeron's");
    }

    #[test]
    fn a_page_may_say_what_it_is_in_plain_words() {
        let mut m = manifest();
        assert_eq!(page_description(&m), None, "none: Zeron's own line");
        m["settings_description"] = json!("Set up once,\n\tone permission at a time.   ");
        assert_eq!(page_description(&m).as_deref(), Some("Set up once, one permission at a time."));
        m["settings_description"] = json!("x".repeat(500));
        assert_eq!(page_description(&m).map(|d| d.chars().count()), Some(MAX_PAGE_DESCRIPTION));
        m["settings_description"] = json!(" \n ");
        assert_eq!(page_description(&m), None);
        // Published with the page, and followed as an update changes it.
        let mut book = Book::new(None);
        m["settings_description"] = json!("Greets you.");
        sync(&mut book, &[("demo", "Demo", "h1", m.clone())]);
        assert_eq!(book.list()[0]["description"], "Greets you.");
        m["settings_description"] = json!("Greets you warmly.");
        assert!(sync(&mut book, &[("demo", "Demo", "h2", m)]).list);
        assert_eq!(book.list()[0]["description"], "Greets you warmly.");
    }
}
