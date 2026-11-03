//! What agents are told of Zeron's interface, to make a template of it: the
//! catalog of screens (each module, its file, what it draws and the elements
//! it names: `#id`s and `.class`es), written into Zeron's own copy as
//! `.catalog.json` (`live`). The engine reads it for `describe_app` and
//! `create_template`.
//!
//! The elements are read from each screen's source: zgx's live report
//! (`host.elements`, behind `describe_elements`) says what the running
//! window has and where plugins can place, but not which file made it, and
//! a template is edited file by file.

use std::{collections::BTreeSet, path::Path};

/// `screens/shell/sidebar.luau` → `@app/screens/shell/sidebar`;
/// `screens/chat/init.luau` → `@app/screens/chat`.
pub fn module_of(rel: &str) -> String {
    let stem = rel.strip_suffix(".luau").unwrap_or(rel);
    let stem = stem.strip_suffix("/init").unwrap_or(stem);
    format!("@app/{stem}")
}

/// A module's name for a list: "Shell › Sidebar".
pub fn title(module: &str) -> String {
    let stem = module.strip_prefix("@app/screens/").unwrap_or(module);
    stem.split('/')
        .map(|part| {
            let words = part.replace('_', " ");
            let mut chars = words.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" › ")
}

/// Every screen of the main window under `ui/screens` (but onboarding's,
/// its own window), with what it draws and the elements it names.
pub fn catalog(ui: &Path) -> serde_json::Value {
    let mut screens = Vec::new();
    for (rel, path) in super::live::files(&ui.join("screens"), |rel| rel.ends_with(".luau")) {
        let Ok(source) = std::fs::read_to_string(&path) else { continue };
        let rel = format!("screens/{rel}");
        let module = module_of(&rel);
        if module.starts_with("@app/screens/onboarding") {
            continue;
        }
        screens.push(serde_json::json!({
            "module": module,
            "file": rel,
            "title": title(&module),
            "description": describe(&source),
            "elements": elements(&source),
        }));
    }
    serde_json::json!({ "screens": screens })
}

/// The elements a module's source names, as selectors: each `id = "…"` as
/// `#id`, each class in a `class = "…"` (every string on that line, for a
/// class chosen by a condition) as `.class`. A part filled in as it runs
/// (`` `plugin-{id}` ``) shows as `*`. Ids first, then classes, each sorted.
pub fn elements(source: &str) -> Vec<String> {
    let mut ids = BTreeSet::new();
    let mut classes = BTreeSet::new();
    for line in source.lines() {
        let code = line.split("--").next().unwrap_or("");
        if let Some(at) = prop_at(code, "id") {
            if let Some(first) = strings(&code[at..]).into_iter().next() {
                ids.insert(format!("#{first}"));
            }
        }
        if let Some(at) = prop_at(code, "class") {
            for text in strings(&code[at..]) {
                classes.extend(text.split_whitespace().map(|c| format!(".{c}")));
            }
        }
    }
    ids.into_iter().chain(classes).collect()
}

/// Where `name = ` starts as a property (not `ids =`, `my_id =`, `id ==`),
/// past the `=`.
fn prop_at(code: &str, name: &str) -> Option<usize> {
    let pattern = format!("{name} = ");
    let mut from = 0;
    while let Some(i) = code[from..].find(&pattern) {
        let at = from + i;
        let before = code[..at].chars().next_back();
        let plain = before.is_none_or(|c| !(c.is_alphanumeric() || c == '_' || c == '.'));
        if plain {
            return Some(at + pattern.len());
        }
        from = at + pattern.len();
    }
    None
}

/// The string literals in `code` ("…", '…', or `…` with each `{…}` as `*`),
/// stopping at the first thing that isn't one, `if`/`then`/`else` or a
/// condition between them.
fn strings(code: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = code.char_indices().peekable();
    while let Some((_, c)) = chars.next() {
        match c {
            '"' | '\'' | '`' => {
                let mut text = String::new();
                let mut depth = 0;
                for (_, ch) in chars.by_ref() {
                    if c == '`' && ch == '{' {
                        depth += 1;
                        if depth == 1 {
                            text.push('*');
                        }
                    } else if c == '`' && ch == '}' && depth > 0 {
                        depth -= 1;
                    } else if depth == 0 && ch == c {
                        break;
                    } else if depth == 0 {
                        text.push(ch);
                    }
                }
                if !text.is_empty() && text.chars().all(|ch| ch.is_alphanumeric() || "-_ *".contains(ch)) {
                    out.push(text);
                }
            }
            ',' | '}' | ')' | ';' => break,
            _ => {}
        }
    }
    out
}

/// What a screen draws, from its module's opening comment: its first
/// sentence, or as much as fits a line.
pub fn describe(source: &str) -> String {
    let mut words = String::new();
    for line in source.lines() {
        let Some(text) = line.trim().strip_prefix("--") else { break };
        let text = text.trim();
        if text.is_empty() {
            if words.is_empty() {
                continue;
            }
            break;
        }
        if !words.is_empty() {
            words.push(' ');
        }
        words.push_str(text);
    }
    let end = words.find(". ").map(|i| i + 1).unwrap_or(words.len());
    let first: String = words[..end].chars().take(160).collect();
    first.trim_end_matches(':').to_string()
}

/// `value` into `path`, unless it holds it already: whether it wrote.
pub fn write_if_changed(path: &Path, value: &serde_json::Value) -> bool {
    let bytes = serde_json::to_vec_pretty(value).unwrap_or_default();
    super::live::put(path, &bytes).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_name_the_modules_they_are() {
        assert_eq!(module_of("screens/shell/sidebar.luau"), "@app/screens/shell/sidebar");
        assert_eq!(module_of("screens/chat/init.luau"), "@app/screens/chat");
        assert_eq!(title("@app/screens/shell/title_bar"), "Shell › Title bar");
    }

    #[test]
    fn the_catalog_says_what_each_screen_draws_and_the_elements_it_names() {
        let ui = Path::new(env!("CARGO_MANIFEST_DIR")).join("ui");
        let catalog = catalog(&ui);
        let screens = catalog["screens"].as_array().unwrap();
        let bar = screens.iter().find(|s| s["module"] == "@app/screens/shell/title_bar").unwrap();
        assert_eq!(bar["file"], "screens/shell/title_bar.luau");
        assert!(!bar["description"].as_str().unwrap().is_empty());
        let named = |s: &serde_json::Value| -> Vec<String> {
            s["elements"].as_array().unwrap().iter().map(|e| e.as_str().unwrap().to_string()).collect()
        };
        assert!(named(bar).contains(&"#titlebar".to_string()) && named(bar).contains(&"#titlebar-actions".to_string()));
        assert!(screens.iter().all(|s| s.get("hooks").is_none()));
        assert!(screens.iter().all(|s| !s["module"].as_str().unwrap().starts_with("@app/screens/onboarding")));
        assert_eq!(
            elements(concat!(
                "ui.row { id = \"bar\", class = \"row wide\" }\n",
                "x.id = \"not-a-prop\"\nlocal ids = { \"no\" }\n",
                "-- id = \"commented\"\n",
                "ui.div { class = if on then \"tab file-tab\" else \"tab\", id = `row-{t.id}` }\n",
                "ui.div { class = `plugin-tab plugin-{name}` }\n",
                "{ id = s.id }\n",
            )),
            vec!["#bar", "#row-*", ".file-tab", ".plugin-*", ".plugin-tab", ".row", ".tab", ".wide"]
        );
        assert_eq!(
            describe("-- The main window's sidebar: rows. More words here.\n-- And on.\n\nlocal x = 1"),
            "The main window's sidebar: rows."
        );
    }
}
