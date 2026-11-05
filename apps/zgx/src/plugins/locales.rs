//! A plugin's own words: `locales/<locale>.json` in its folder (`en.json`
//! its source, as Zeron's ui/locales), each flattened to dotted keys and
//! published to the plugin alone as `plugin.<id>.locales`
//! (`{ en = { ["greeting.title"] = "…" }, ["zh-Hans"] = { … } }`). The SDK's
//! `zeron.t` / `zeron.s` (plugins-sdk/zeron.luau) show them in the locale
//! the app shows, falling back to the plugin's English, then the key.
//!
//! zgx keeps one catalog per host (the app's: Zeron's own words, which the
//! lent design system shows), so a plugin's words are read here and
//! formatted by the SDK, not by `ui.t`. A plugin with no `locales` folder
//! gets an empty table and loses nothing.

use std::path::Path;

use serde_json::{Map, Value as Json};
use zgx_script::ScriptHost;

/// The most a plugin's locale files may hold, all together: words, not
/// data.
const MOST_BYTES: u64 = 1 << 20;

/// `dir`'s `locales/*.json`, each flattened, by locale (the file's stem):
/// empty where there are none. A file that isn't JSON, or a value that
/// isn't text, is left out and said.
pub fn read(dir: &Path) -> Json {
    let mut out = Map::new();
    let Ok(entries) = std::fs::read_dir(dir.join("locales")) else {
        return Json::Object(out);
    };
    let mut paths: Vec<_> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    paths.sort();
    let mut total = 0u64;
    for path in paths {
        let Some(locale) = path.file_stem().and_then(|s| s.to_str()).map(str::to_string) else { continue };
        total += std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        if total > MOST_BYTES {
            eprintln!("zeron-zgx: {}: a plugin's locales hold more than 1 MB; the rest are left out", path.display());
            break;
        }
        let json: Json = match std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|t| serde_json::from_str(&t).map_err(|e| e.to_string())) {
            Ok(json) => json,
            Err(error) => {
                eprintln!("zeron-zgx: {}: {error}", path.display());
                continue;
            }
        };
        let mut flat = Map::new();
        flatten("", &json, &mut flat);
        out.insert(locale, Json::Object(flat));
    }
    Json::Object(out)
}

fn flatten(prefix: &str, v: &Json, out: &mut Map<String, Json>) {
    match v {
        Json::Object(map) => {
            for (k, v) in map {
                let key = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") };
                flatten(&key, v, out);
            }
        }
        Json::String(_) => {
            out.insert(prefix.to_string(), v.clone());
        }
        _ => {}
    }
}

/// Publishes plugin `id`'s words, read from its folder `dir`, to it.
pub fn publish(host: &ScriptHost, id: &str, dir: &Path) {
    let words = super::json_value(&read(dir));
    host.shared(&format!("plugin.{id}.locales"), words.clone()).set(words);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plugins_locales_flatten_by_locale_and_none_is_empty() {
        let dir = std::env::temp_dir().join(format!("zgx-plugin-locales-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(read(&dir), serde_json::json!({}));
        std::fs::create_dir_all(dir.join("locales")).unwrap();
        std::fs::write(dir.join("locales/en.json"), r#"{ "timer": { "start": "Start", "left": "{n, plural, one {# minute} other {# minutes}} left" } }"#).unwrap();
        std::fs::write(dir.join("locales/zh-Hans.json"), r#"{ "timer": { "start": "开始" } }"#).unwrap();
        std::fs::write(dir.join("locales/broken.json"), "{ nope").unwrap();
        let words = read(&dir);
        assert_eq!(words["en"]["timer.start"], "Start");
        assert_eq!(words["zh-Hans"]["timer.start"], "开始");
        assert!(words.get("broken").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
