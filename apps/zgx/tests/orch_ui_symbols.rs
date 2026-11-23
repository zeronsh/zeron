//! No symbol typed as text between words in the orchestrators' and the
//! Control room's UI: no "·" in a string their code builds, nor in the
//! words their locale keys hold. What sits between two things is spacing,
//! or an icon (as the composer's place row has it); a sentence's own
//! punctuation is fine.
//!
//! Over every `ds/orch_*` (the Board, its cards and the Graph with the
//! rest), `screens/control/` and `screens/orchestrator/`, in ui/ or in the
//! Orchestrators plugin's app/ (plugins-builtin/orchestrators, which keeps
//! ui/'s layout), and the `control.`, `orch.` and `ds.orch.` keys of every
//! locale. (The usage
//! limit's `ds.limit.` words keep a separator between their facts that no
//! one sees: `limit.parts` and the Graph's card lay them out apart.)

use std::path::{Path, PathBuf};

const SYMBOL: char = '·';

fn ui_dir() -> PathBuf {
    std::env::var_os("ZERON_ZGX_TEST_UI")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui"))
}

/// The Orchestrators plugin's app-side modules, laid out as in ui/.
fn plugin_app_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("plugins-builtin/orchestrators/app")
}

fn luau_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            luau_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "luau") {
            out.push(path);
        }
    }
}

fn in_scope(rel: &str) -> bool {
    if rel.starts_with("screens/control/") || rel.starts_with("screens/orchestrator/") {
        return true;
    }
    rel.starts_with("ds/orch_")
}

/// The code of a line, its comment cut.
fn code(line: &str) -> &str {
    match line.find("--") {
        Some(at) => &line[..at],
        None => line,
    }
}

fn key_in_scope(key: &str) -> bool {
    ["control.", "orch.", "ds.orch."].iter().any(|p| key.starts_with(p))
}

fn walk(prefix: &str, value: &serde_json::Value, file: &str, found: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                let key = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") };
                walk(&key, v, file, found);
            }
        }
        serde_json::Value::String(s) if s.contains(SYMBOL) && key_in_scope(prefix) => {
            found.push(format!("{file}: {prefix} = {s:?}"));
        }
        _ => {}
    }
}

#[test]
fn no_dot_typed_between_words_in_the_orchestrators_ui() {
    let ui = ui_dir();
    let mut found = Vec::new();
    let mut scanned = 0;
    for (root, shown) in [(ui.clone(), "ui"), (plugin_app_dir(), "plugins-builtin/orchestrators/app")] {
        let mut files = Vec::new();
        luau_files(&root.join("ds"), &mut files);
        luau_files(&root.join("screens"), &mut files);
        for path in &files {
            let rel = path.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/");
            if !in_scope(&rel) {
                continue;
            }
            scanned += 1;
            let text = std::fs::read_to_string(path).unwrap();
            for (n, line) in text.lines().enumerate() {
                if code(line).contains(SYMBOL) {
                    found.push(format!("{shown}/{rel}:{}: {}", n + 1, line.trim()));
                }
            }
        }
    }
    // (A scope that found no file would pass on nothing.)
    assert!(scanned > 20, "only {scanned} files in scope under {} and {}", ui.display(), plugin_app_dir().display());
    let mut locales = 0;
    for entry in std::fs::read_dir(ui.join("locales")).unwrap().flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        locales += 1;
        let value: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        walk("", &value, &entry.file_name().to_string_lossy(), &mut found);
    }
    assert!(locales >= 2);
    assert!(found.is_empty(), "\"{SYMBOL}\" typed as text; use spacing or an icon:\n{}", found.join("\n"));
}
