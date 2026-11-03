//! A template's look: the colours its preview is drawn in on the Templates
//! page (a swatch of each appearance it has, light then dark), read from
//! the files that colour the window, as the live overlay lays them:
//!
//! - its `ds/themes/zeron-{light,dark}.json`, where it has its own (a whole
//!   file over Zeron's: what the window loads for that appearance);
//! - its `theme.luau` (src/plugins/theme_templates.rs), a palette for an
//!   appearance worn over that theme (ds/palettes).
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

use super::{live, theme_templates};

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

    /// Template `id`'s look (Zeron's own for `live::BUILTIN`), or Null when
    /// it changes no colour of Zeron's.
    pub fn look(&self, templates: &Path, id: &str) -> Json {
        let zeron = templates.join(live::ZERON);
        let own = id == live::BUILTIN;
        let dir = if own { zeron.clone() } else { templates.join(id) };
        let mut stamps = Vec::new();
        let (stamp, palettes) = if own { (String::new(), Json::Null) } else { self.read(&dir.join(theme_templates::FILE), luau_literal) };
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

/// A `theme.luau` read as data: `return` and a Luau table literal of
/// strings, numbers, booleans and tables (what theme_templates writes, and
/// a palette written by hand). Anything else (code) isn't a look: None.
pub fn luau_literal(text: &str) -> Option<Json> {
    let mut p = Parser { s: text.as_bytes(), at: 0 };
    p.space();
    if !p.word("return") {
        return None;
    }
    let value = p.value()?;
    p.space();
    (p.at == p.s.len()).then_some(value)
}

struct Parser<'a> {
    s: &'a [u8],
    at: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.at).copied()
    }

    /// Past white space and `--` comments (a line's, or a `--[[ ]]` block).
    fn space(&mut self) {
        loop {
            while self.peek().is_some_and(|c| c.is_ascii_whitespace()) {
                self.at += 1;
            }
            if self.s[self.at..].starts_with(b"--[[") {
                match self.s[self.at..].windows(2).position(|w| w == b"]]") {
                    Some(end) => self.at += end + 2,
                    None => self.at = self.s.len(),
                }
            } else if self.s[self.at..].starts_with(b"--") {
                while self.peek().is_some_and(|c| c != b'\n') {
                    self.at += 1;
                }
            } else {
                return;
            }
        }
    }

    fn word(&mut self, word: &str) -> bool {
        self.space();
        let end = self.at + word.len();
        let next = self.s.get(end).copied();
        if self.s[self.at..].starts_with(word.as_bytes()) && !next.is_some_and(|c| c.is_ascii_alphanumeric() || c == b'_') {
            self.at = end;
            return true;
        }
        false
    }

    fn name(&mut self) -> Option<String> {
        self.space();
        let start = self.at;
        while self.peek().is_some_and(|c| c.is_ascii_alphanumeric() || c == b'_') {
            self.at += 1;
        }
        (self.at > start && !self.s[start].is_ascii_digit()).then(|| String::from_utf8_lossy(&self.s[start..self.at]).into_owned())
    }

    fn string(&mut self) -> Option<String> {
        self.space();
        let quote = self.peek().filter(|c| *c == b'"' || *c == b'\'')?;
        self.at += 1;
        let mut out = Vec::new();
        loop {
            let c = self.peek()?;
            self.at += 1;
            match c {
                b'\\' => {
                    let e = self.peek()?;
                    self.at += 1;
                    out.push(match e {
                        b'n' => b'\n',
                        b't' => b'\t',
                        other => other,
                    });
                }
                c if c == quote => return String::from_utf8(out).ok(),
                b'\n' => return None,
                c => out.push(c),
            }
        }
    }

    fn value(&mut self) -> Option<Json> {
        self.space();
        match self.peek()? {
            b'"' | b'\'' => self.string().map(Json::String),
            b'{' => self.table(),
            c if c == b'-' || c.is_ascii_digit() => {
                let start = self.at;
                self.at += 1;
                while self.peek().is_some_and(|c| c.is_ascii_digit() || c == b'.') {
                    self.at += 1;
                }
                std::str::from_utf8(&self.s[start..self.at]).ok()?.parse::<f64>().ok().and_then(|n| serde_json::Number::from_f64(n).map(Json::Number))
            }
            _ => {
                if self.word("true") {
                    Some(Json::Bool(true))
                } else if self.word("false") {
                    Some(Json::Bool(false))
                } else if self.word("nil") {
                    Some(Json::Null)
                } else {
                    None
                }
            }
        }
    }

    /// `{ name = v, ["key"] = v, v, … }`: an object when it has a key, else
    /// a list.
    fn table(&mut self) -> Option<Json> {
        self.at += 1;
        let mut map = Map::new();
        let mut list = Vec::new();
        loop {
            self.space();
            match self.peek()? {
                b'}' => {
                    self.at += 1;
                    break;
                }
                b'[' => {
                    self.at += 1;
                    let key = self.string()?;
                    self.space();
                    (self.peek()? == b']').then_some(())?;
                    self.at += 1;
                    self.equals()?;
                    map.insert(key, self.value()?);
                }
                _ => {
                    let save = self.at;
                    match self.name() {
                        Some(key) if !matches!(key.as_str(), "true" | "false" | "nil") && self.equals().is_some() => {
                            map.insert(key, self.value()?);
                        }
                        _ => {
                            self.at = save;
                            list.push(self.value()?);
                        }
                    }
                }
            }
            self.space();
            if matches!(self.peek(), Some(b',' | b';')) {
                self.at += 1;
            }
        }
        match (map.is_empty(), list.is_empty()) {
            (_, true) => Some(Json::Object(map)),
            (true, false) => Some(Json::Array(list)),
            (false, false) => None,
        }
    }

    fn equals(&mut self) -> Option<()> {
        self.space();
        (self.peek()? == b'=' && self.s.get(self.at + 1) != Some(&b'=')).then(|| self.at += 1)
    }
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
        // A palette (an official template's theme.luau, as written).
        let id = theme_templates::install(&t, "tokyo-night").unwrap();
        let tokyo = cache.look(&t, &id);
        assert_eq!((tokyo["light"]["palette"]["background"].as_str(), tokyo["dark"]["palette"]["background"].as_str()), (Some("#d5d6db"), Some("#1a1b26")));
        assert_eq!(tokyo["dark"]["palette"]["ansi"].as_array().unwrap().len(), 16);
        assert!(tokyo["dark"].get("theme").is_none());
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

    #[test]
    fn a_theme_file_reads_as_data_and_code_does_not() {
        let v = luau_literal("-- a palette\nreturn {\n\tname = \"Mine\", -- its name\n\tdark = { text = 'a\\'b', syntax = { \"#1\", \"#2\" }, n = 1.5, on = true },\n\t[\"light\"] = nil,\n}\n").unwrap();
        assert_eq!(v["name"], "Mine");
        assert_eq!(v["dark"]["text"], "a'b");
        assert_eq!(v["dark"]["syntax"], json!(["#1", "#2"]));
        assert_eq!(v["dark"]["n"], 1.5);
        assert_eq!(luau_literal("return nil\n"), Some(Json::Null));
        assert!(luau_literal("local x = 1\nreturn { a = x }").is_none());
        assert!(luau_literal("return require(\"@ds/thing\")").is_none());
        assert!(luau_literal("return { a = \"x\" } extra").is_none());
    }
}
