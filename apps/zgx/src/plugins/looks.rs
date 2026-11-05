//! A template's look: the colours its preview is drawn in on the Templates
//! page (a swatch of each appearance it has, light then dark), read from
//! the files that colour the window, as the live overlay lays them:
//!
//! - its `ds/themes/zeron-{light,dark}.json`, where it has its own (a whole
//!   file over Zeron's: what the window loads for that appearance);
//! - its `theme.luau`, a palette for an appearance worn over that theme
//!   (ds/palettes): the template's own theme, worn where you haven't chosen
//!   one (src/themes.rs).
//!
//! An appearance with neither is Zeron's. A template that changes none shows
//! Zeron's own (no look); one that changes one appearance shows that one
//! alone. Zeron's own look is its own theme files (`.zeron`).
//!
//! Files are read through the host, once per change: each is cached by its
//! stamp (size and time written), so the page's list, published often,
//! reads nothing new until a file is saved.
//!
//! ```text
//! look   { stamp, light?: Scheme, dark?: Scheme }
//! Scheme { theme?: { type, extends, tokens }, palette?: Palette }
//! ```

use std::{
    cell::RefCell,
    collections::HashMap,
    path::{Path, PathBuf},
};

use serde_json::{Map, Value as Json, json};

use super::live;

/// The file a template's own theme is in, at its root.
pub const THEME_FILE: &str = "theme.luau";

const SCHEMES: [&str; 2] = ["light", "dark"];

/// Parsed files by path, each with the stamp it was read at.
#[derive(Default)]
pub struct Cache {
    files: RefCell<HashMap<PathBuf, (String, Json)>>,
}

impl Cache {
    /// `path` parsed by `parse` (Null when it's absent or doesn't parse),
    /// and its stamp: read again only once the file has changed.
    fn read(&self, path: &Path, parse: fn(&str) -> Option<Json>) -> (String, Json) {
        let stamp = live::file_stamp(path);
        if let Some((at, value)) = self.files.borrow().get(path)
            && *at == stamp
        {
            return (stamp, value.clone());
        }
        let value = std::fs::read_to_string(path).ok().and_then(|text| parse(&text)).unwrap_or(Json::Null);
        self.files.borrow_mut().insert(path.to_path_buf(), (stamp.clone(), value.clone()));
        (stamp, value)
    }

    /// Template `id`'s own theme: its `theme.luau` read as data (`{ name?,
    /// light?, dark? }`), or Null for none, or a file that isn't data.
    pub fn theme_file(&self, templates: &Path, id: &str) -> Json {
        self.read(&templates.join(id).join(THEME_FILE), zeron_theme::luau_data::parse).1
    }

    /// Template `id`'s look (Zeron's own for `live::BUILTIN`), or Null when
    /// it changes no colour of Zeron's.
    pub fn look(&self, templates: &Path, id: &str) -> Json {
        let zeron = templates.join(live::ZERON);
        let own = id == live::BUILTIN;
        let dir = if own { zeron.clone() } else { templates.join(id) };
        let mut stamps = Vec::new();
        let (stamp, palettes) = if own { (String::new(), Json::Null) } else { self.read(&dir.join(THEME_FILE), zeron_theme::luau_data::parse) };
        stamps.push(stamp);
        let mut out = Map::new();
        for scheme in SCHEMES {
            let rel = format!("ds/themes/zeron-{scheme}.json");
            let mut entry = Map::new();
            let (stamp, theme) = self.read(&dir.join(&rel), json5_theme);
            stamps.push(stamp);
            // A template's copy of Zeron's file as it is changes nothing.
            let changed = own || {
                let (_, zeron_theme) = self.read(&zeron.join(&rel), json5_theme);
                theme != zeron_theme
            };
            if theme.is_object() && changed {
                entry.insert("theme".into(), theme);
            }
            if palettes[scheme].is_object() {
                entry.insert("palette".into(), palettes[scheme].clone());
            }
            if !entry.is_empty() {
                out.insert(scheme.into(), Json::Object(entry));
            }
        }
        if out.is_empty() {
            return Json::Null;
        }
        out.insert("stamp".into(), json!(live::hash(stamps.join("|").as_bytes())));
        Json::Object(out)
    }
}

/// A zgx theme file (JSON with comments): `{ type, extends, tokens }`.
fn json5_theme(text: &str) -> Option<Json> {
    let file: Json = json5::from_str(text).ok()?;
    let tokens = file.get("tokens").filter(|t| t.is_object())?.clone();
    Some(json!({
        "type": file.get("type").cloned().unwrap_or(Json::Null),
        "extends": file.get("extends").cloned().unwrap_or(Json::Null),
        "tokens": tokens,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zgx-looks-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(dir: &Path, rel: &str, text: &str) {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    const DARK: &str = "{\n\t// Zeron dark.\n\t\"name\": \"zeron-dark\", \"type\": \"dark\", \"extends\": \"zgx-dark\",\n\t\"tokens\": { \"accent\": \"#9277b0\", \"zeron\": { \"window\": \"#0e0e10\" }, },\n}";
    const LIGHT: &str = "{ \"name\": \"zeron-light\", \"type\": \"light\", \"extends\": \"zgx-light\", \"tokens\": { \"accent\": \"#7a55c4\" } }";

    /// A templates folder with Zeron's own themes.
    fn setup(name: &str) -> PathBuf {
        let t = dir(name).join("templates");
        write(&t, ".zeron/ds/themes/zeron-dark.json", DARK);
        write(&t, ".zeron/ds/themes/zeron-light.json", LIGHT);
        write(&t, ".zeron/theme.luau", "-- Zeron's own.\nreturn nil\n");
        t
    }

    #[test]
    fn zeron_s_own_look_is_its_two_themes() {
        let t = setup("own");
        let look = Cache::default().look(&t, live::BUILTIN);
        assert_eq!(look["dark"]["theme"]["tokens"]["accent"], "#9277b0", "comments and trailing commas read");
        assert_eq!(look["dark"]["theme"]["extends"], "zgx-dark");
        assert_eq!(look["light"]["theme"]["tokens"]["accent"], "#7a55c4");
        assert!(look["dark"].get("palette").is_none() && look["stamp"].is_string());
    }

    #[test]
    fn a_template_s_look_is_what_it_changes() {
        let t = setup("changes");
        let cache = Cache::default();
        // Both appearances, its own files.
        write(&t, "pink/zeron-template.json", "{}");
        write(&t, "pink/ds/themes/zeron-dark.json", &DARK.replace("#9277b0", "#ff3e9a"));
        write(&t, "pink/ds/themes/zeron-light.json", &LIGHT.replace("#7a55c4", "#d6247a"));
        let pink = cache.look(&t, "pink");
        assert_eq!((pink["dark"]["theme"]["tokens"]["accent"].as_str(), pink["light"]["theme"]["tokens"]["accent"].as_str()), (Some("#ff3e9a"), Some("#d6247a")));
        // Only dark: the light one is Zeron's, and left out.
        write(&t, "night/zeron-template.json", "{}");
        write(&t, "night/ds/themes/zeron-dark.json", &DARK.replace("#9277b0", "#22ccaa"));
        let night = cache.look(&t, "night");
        assert!(night["dark"].is_object() && night.get("light").is_none());
        // None: a copy of Zeron's files changes nothing.
        write(&t, "copy/zeron-template.json", "{}");
        write(&t, "copy/ds/themes/zeron-dark.json", DARK);
        write(&t, "copy/screens/x.luau", "-- a screen");
        assert!(cache.look(&t, "copy").is_null());
        write(&t, "bare/zeron-template.json", "{}");
        assert!(cache.look(&t, "bare").is_null());
        // A palette (a template's own theme.luau).
        write(&t, "ink/zeron-template.json", "{}");
        write(&t, "ink/theme.luau", "return {\n\tname = \"Ink\",\n\tlight = { background = \"#d5d6db\" },\n\tdark = { background = \"#1a1b26\", ansi = { \"#000000\", \"#111111\" } },\n}\n");
        let ink = cache.look(&t, "ink");
        assert_eq!((ink["light"]["palette"]["background"].as_str(), ink["dark"]["palette"]["background"].as_str()), (Some("#d5d6db"), Some("#1a1b26")));
        assert_eq!(ink["dark"]["palette"]["ansi"].as_array().unwrap().len(), 2);
        assert!(ink["dark"].get("theme").is_none());
        assert_eq!(cache.theme_file(&t, "ink")["name"], "Ink");
        assert!(cache.theme_file(&t, "copy").is_null());
    }

    #[test]
    fn a_file_is_read_again_only_once_it_changes() {
        let t = setup("cache");
        write(&t, "pink/zeron-template.json", "{}");
        write(&t, "pink/ds/themes/zeron-dark.json", &DARK.replace("#9277b0", "#ff3e9a"));
        let cache = Cache::default();
        let first = cache.look(&t, "pink");
        // Changed behind the cache's back without its stamp moving: the
        // cache still answers (nothing read).
        let path = t.join("pink/ds/themes/zeron-dark.json");
        let at = std::fs::metadata(&path).unwrap().modified().unwrap();
        std::fs::write(&path, DARK.replace("#9277b0", "#ff3e9b")).unwrap();
        std::fs::File::options().write(true).open(&path).unwrap().set_modified(at).unwrap();
        assert_eq!(cache.look(&t, "pink"), first, "same stamp: from the cache");
        // Saved (a new time): read again, with a new stamp.
        std::thread::sleep(std::time::Duration::from_millis(5));
        std::fs::write(&path, DARK.replace("#9277b0", "#ff3e9c")).unwrap();
        let again = cache.look(&t, "pink");
        assert_eq!(again["dark"]["theme"]["tokens"]["accent"], "#ff3e9c");
        assert_ne!(again["stamp"], first["stamp"]);
    }

    /// Development only: each template's look in `LOOKS_DIR` (a templates
    /// folder, read only): which appearances it changes, and its accents.
    #[test]
    #[ignore = "run on purpose: reads a real templates folder"]
    fn print_the_looks_of_a_templates_folder() {
        let t = PathBuf::from(std::env::var("LOOKS_DIR").expect("LOOKS_DIR: a templates folder"));
        let cache = Cache::default();
        for i in live::installed(&t) {
            let look = cache.look(&t, &i.id);
            let accent = |s: &str| {
                let e = &look[s];
                e["theme"]["tokens"]["accent"].as_str().or(e["palette"]["accent"].as_str()).map(|a| format!("{s} {a}"))
            };
            let said: Vec<String> = SCHEMES.iter().filter_map(|s| accent(s)).collect();
            println!("{}: {}", i.id, if look.is_null() { "Zeron's own".to_string() } else { said.join(", ") });
        }
    }
}
