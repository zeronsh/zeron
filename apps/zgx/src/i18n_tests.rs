//! Zeron's words (ui/locales) stay whole: every key a script names with a
//! literal `ui.t("…")` (or the design system's `locale.t`/`locale.s`) is in English (the source) and in every translation,
//! each translation names the same arguments as its English (`{n}`, a
//! plural's `n`), and every message parses. `zgx lint` checks the same for
//! one locale at a time as it lints; this holds them all at once, without
//! the zgx CLI. tools/i18n.py `check` says the same, and which keys are
//! still English.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

fn ui() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui")
}

/// A locale file's messages by their dotted keys.
fn catalog(name: &str) -> BTreeMap<String, String> {
    let path = ui().join("locales").join(name);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let json: serde_json::Value = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{name}: {e}"));
    let mut out = BTreeMap::new();
    fn walk(prefix: &str, v: &serde_json::Value, out: &mut BTreeMap<String, String>) {
        match v {
            serde_json::Value::Object(map) => {
                for (k, v) in map {
                    let key = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") };
                    walk(&key, v, out);
                }
            }
            serde_json::Value::String(s) => {
                out.insert(prefix.to_string(), s.clone());
            }
            other => panic!("{prefix}: a message is a string, not {other}"),
        }
    }
    walk("", &json, &mut out);
    out
}

/// The locales beside English (a pseudo-locale made for a layout check,
/// en-XA, is left out: it's made from English, not kept).
fn translations() -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(ui().join("locales"))
        .unwrap()
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.ends_with(".json") && n != "en.json" && !n.starts_with("en-XA"))
        .collect();
    names.sort();
    names
}

/// The arguments a message names (`{name}`, `{n, plural, …}`), through its
/// plural and select branches; an error where it doesn't parse. ICU's
/// quoting: `'{'` is a literal brace, `''` an apostrophe.
fn arguments(message: &str) -> Result<BTreeSet<String>, String> {
    let chars: Vec<char> = message.chars().collect();
    let mut names = BTreeSet::new();
    let mut i = 0;
    text(&chars, &mut i, 0, &mut names)?;
    if i < chars.len() {
        return Err(format!("an unmatched }} at {i}"));
    }
    Ok(names)
}

fn text(c: &[char], i: &mut usize, depth: usize, names: &mut BTreeSet<String>) -> Result<(), String> {
    while *i < c.len() {
        match c[*i] {
            '{' => argument(c, i, names)?,
            '}' if depth > 0 => return Ok(()),
            '}' => return Err(format!("an unmatched }} at {i}")),
            '\'' if c.get(*i + 1) == Some(&'\'') => *i += 2,
            '\'' if matches!(c.get(*i + 1), Some('{' | '}' | '#' | '|')) => {
                *i += 1;
                while *i < c.len() && c[*i] != '\'' {
                    *i += 1;
                }
                *i += 1;
            }
            _ => *i += 1,
        }
    }
    if depth > 0 { Err("a branch never closes".into()) } else { Ok(()) }
}

fn word(c: &[char], i: &mut usize) -> String {
    while *i < c.len() && c[*i].is_whitespace() {
        *i += 1;
    }
    let start = *i;
    while *i < c.len() && (c[*i].is_alphanumeric() || matches!(c[*i], '_' | '.' | '=' | '-')) {
        *i += 1;
    }
    let w: String = c[start..*i].iter().collect();
    while *i < c.len() && c[*i].is_whitespace() {
        *i += 1;
    }
    w
}

fn argument(c: &[char], i: &mut usize, names: &mut BTreeSet<String>) -> Result<(), String> {
    *i += 1; // {
    let name = word(c, i);
    if name.is_empty() {
        return Err(format!("an argument without a name at {i}"));
    }
    names.insert(name);
    match c.get(*i) {
        Some('}') => {
            *i += 1;
            return Ok(());
        }
        Some(',') => *i += 1,
        _ => return Err(format!("expected , or }} after an argument at {i}")),
    }
    let kind = word(c, i);
    if matches!(kind.as_str(), "plural" | "select" | "selectordinal") {
        if c.get(*i) != Some(&',') {
            return Err(format!("expected , after {kind}"));
        }
        *i += 1;
        let mut branches = 0;
        loop {
            let key = word(c, i);
            if key.is_empty() {
                break;
            }
            if c.get(*i) != Some(&'{') {
                return Err(format!("expected {{ after `{key}`"));
            }
            *i += 1;
            text(c, i, 1, names)?;
            *i += 1; // }
            branches += 1;
        }
        if branches == 0 || c.get(*i) != Some(&'}') {
            return Err(format!("{kind} doesn't close"));
        }
        *i += 1;
        return Ok(());
    }
    // number, date: their style, up to the brace.
    while *i < c.len() && c[*i] != '}' {
        *i += 1;
    }
    if *i >= c.len() {
        return Err(format!("{kind} doesn't close"));
    }
    *i += 1;
    Ok(())
}

/// Each literal `ui.t("key"` in the scripts (not in comments), with where.
fn keys_named() -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    fn walk(dir: &Path, out: &mut BTreeMap<String, String>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if path.is_dir() {
                if name != ".zgx" && name != "locales" {
                    walk(&path, out);
                }
            // (The Files demo's snapshot is data: other files' text, `ui.t`
            // calls in it included, embedded as strings.)
            } else if name.ends_with(".luau") && name != "files_snapshot.luau" {
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                for (n, line) in text.lines().enumerate() {
                    let code = match line.find("--") {
                        Some(at) => &line[..at],
                        None => line,
                    };
                    // `ui.t(` in screens, `locale.t(` / `locale.s(` in the
                    // design system (ds/locale.luau).
                    let mut rest = code;
                    while let Some((at, len)) = ["ui.t(", "locale.t(", "locale.s("]
                        .iter()
                        .filter_map(|p| rest.find(p).map(|at| (at, p.len())))
                        .min()
                    {
                        rest = &rest[at + len..];
                        let trimmed = rest.trim_start();
                        let Some(quote) = trimmed.chars().next().filter(|q| matches!(q, '"' | '\'')) else {
                            continue;
                        };
                        let body = &trimmed[1..];
                        if let Some(end) = body.find(quote) {
                            let key = &body[..end];
                            out.entry(key.to_string())
                                .or_insert_with(|| format!("{}:{}", path.strip_prefix(ui()).unwrap_or(&path).display(), n + 1));
                        }
                    }
                }
            }
        }
    }
    walk(&ui(), &mut out);
    out
}

#[test]
fn every_message_parses() {
    let mut bad = Vec::new();
    for name in std::iter::once("en.json".to_string()).chain(translations()) {
        for (key, message) in catalog(&name) {
            if let Err(e) = arguments(&message) {
                bad.push(format!("{name}: {key}: {e}: {message:?}"));
            }
        }
    }
    assert!(bad.is_empty(), "messages that don't parse:\n{}", bad.join("\n"));
}

#[test]
fn every_key_the_scripts_name_is_in_every_locale() {
    let en = catalog("en.json");
    let named = keys_named();
    assert!(!named.is_empty(), "no ui.t keys found under ui/");
    let mut missing = Vec::new();
    for (key, at) in &named {
        if !en.contains_key(key) {
            missing.push(format!("en.json lacks {key} ({at})"));
        }
    }
    for name in translations() {
        let other = catalog(&name);
        for key in en.keys() {
            if !other.contains_key(key) {
                missing.push(format!("{name} lacks {key}"));
            }
        }
        for key in other.keys() {
            if !en.contains_key(key) {
                missing.push(format!("{name} has {key}, which en.json doesn't"));
            }
        }
    }
    assert!(missing.is_empty(), "{} keys out of step:\n{}", missing.len(), missing.join("\n"));
}

#[test]
fn translations_name_the_arguments_english_does() {
    let en = catalog("en.json");
    let mut bad = Vec::new();
    for name in translations() {
        for (key, message) in catalog(&name) {
            let (Some(source), Ok(theirs)) = (en.get(&key), arguments(&message)) else { continue };
            let Ok(ours) = arguments(source) else { continue };
            if ours != theirs {
                bad.push(format!("{name}: {key}: {theirs:?}, English names {ours:?}"));
            }
        }
    }
    assert!(bad.is_empty(), "placeholders that don't match English:\n{}", bad.join("\n"));
}

#[test]
fn the_argument_reader_knows_icu() {
    let names = |m: &str| arguments(m).unwrap().into_iter().collect::<Vec<_>>();
    assert_eq!(names("Delete {name}?"), ["name"]);
    assert_eq!(names("{n, plural, one {# file} other {# files in {dir}}}"), ["dir", "n"]);
    assert_eq!(names("{m, select, 1 {Jan} other {}} {day}"), ["day", "m"]);
    assert_eq!(names("Don't use '{'braces'}'"), Vec::<String>::new());
    assert!(arguments("{n, plural, one {# file}").is_err());
    assert!(arguments("oops }").is_err());
}
