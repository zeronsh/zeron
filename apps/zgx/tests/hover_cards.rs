//! Every node tooltip is framed once, by ds/hover_card, in the menus' own
//! surface.
//!
//! A `tooltip = …` that builds a node builds it through the hover card
//! (`HoverCard.Card`, `HoverCard.Tip`, or what is made of them: `Tooltip`,
//! `context_usage.Card`). The hover card's frame is the menus' glass
//! (`glass.style(…, "menu")`, as ds/menu's panels are), so a hover card and
//! a picker's menu are one material; the files built on it paint no fill,
//! edge, shadow or blur of their own round it. (zgx b75d8e74f finds a popup's
//! card through its clear layer: no second rim, AppKit's material under it.
//! ui/tests/composer_place.test.luau checks a mounted card: its layer hugs
//! it, one frame inside.)

use std::path::{Path, PathBuf};

fn ui_dir() -> PathBuf {
    std::env::var_os("ZERON_ZGX_TEST_UI")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui"))
}

/// What a tooltip may be built with: the hover card, and what is made of it.
const FRAMED: &[&str] = &["HoverCard.Card", "HoverCard.Tip", "Tooltip", "context_usage.Card"];

/// The files built on the hover card's frame: none paints a surface of its
/// own round it (a fill, an edge, a shadow, a blur, glass).
const ON_FRAME: &[&str] = &["ds/tooltip.luau", "ds/context_usage.luau"];

fn luau_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            // (Generated types, and the tests' own fixtures of what's wrong.)
            if name != ".zgx" && name != "tests" {
                luau_files(&path, out);
            }
        } else if name.ends_with(".luau") && name != "files_snapshot.luau" {
            out.push(path);
        }
    }
}

/// The code of a line, its comment cut.
fn code(line: &str) -> &str {
    match line.find("--") {
        Some(at) => &line[..at],
        None => line,
    }
}

/// The node constructors in `rhs` (a dotted name followed by `{`, or by `(`
/// where it is `ui.…` or capitalised): `Some(name)` for each.
fn constructors(rhs: &str) -> Vec<String> {
    let bytes = rhs.as_bytes();
    let mut out = Vec::new();
    for (at, c) in rhs.char_indices() {
        if c != '{' && c != '(' {
            continue;
        }
        let mut end = at;
        while end > 0 && bytes[end - 1] == b' ' {
            end -= 1;
        }
        let mut start = end;
        while start > 0 && (bytes[start - 1].is_ascii_alphanumeric() || bytes[start - 1] == b'_' || bytes[start - 1] == b'.') {
            start -= 1;
        }
        let name = &rhs[start..end];
        if name.is_empty() {
            continue;
        }
        let last = name.rsplit('.').next().unwrap_or(name);
        let capital = last.chars().next().is_some_and(|c| c.is_ascii_uppercase());
        let node = name.starts_with("ui.")
            && !matches!(last, "t" | "shortcut_text" | "raw" | "computed" | "state");
        let glass = name.starts_with("glass.") || name.starts_with("Glass.") || name.starts_with("Menu.");
        if (c == '{' && (capital || node || glass)) || (c == '(' && (capital || node || glass)) {
            out.push(name.to_owned());
        }
    }
    out
}

#[test]
fn every_node_tooltip_is_a_hover_card() {
    let root = ui_dir();
    let mut files = Vec::new();
    luau_files(&root, &mut files);
    assert!(!files.is_empty(), "no scripts under {}", root.display());
    let mut wrong = Vec::new();
    for path in &files {
        let Ok(text) = std::fs::read_to_string(path) else { continue };
        for (n, line) in text.lines().enumerate() {
            let line = code(line);
            let Some(at) = line.find("tooltip = ") else { continue };
            let rhs = &line[at + "tooltip = ".len()..];
            for name in constructors(rhs) {
                let short = name.strip_prefix("ds.").unwrap_or(&name);
                if !FRAMED.contains(&short) {
                    wrong.push(format!(
                        "{}:{}: `tooltip = {}` builds its node with `{name}`: frame it with ds/hover_card (HoverCard.Card or HoverCard.Tip)",
                        path.strip_prefix(&root).unwrap_or(path).display(),
                        n + 1,
                        rhs.trim()
                    ));
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn a_hover_card_wears_the_menus_surface_and_nothing_else_frames_it() {
    let root = ui_dir();
    let mut wrong = Vec::new();
    // The frame: the menus' glass, as ds/menu's panels take it.
    let frame = std::fs::read_to_string(root.join("ds/hover_card.luau")).expect("ds/hover_card.luau");
    let code: String = frame.lines().map(code).collect::<Vec<_>>().join("\n");
    if !code.contains("glass.style(") || !code.contains("\"menu\"") {
        wrong.push("ds/hover_card.luau: its frame isn't the menus' glass (`glass.style(props, \"menu\")`)".to_owned());
    }
    for needle in ["bg =", "border =", "shadow =", "backdrop_blur =", "rounded ="] {
        for (n, line) in frame.lines().enumerate() {
            if code_has(line, needle) {
                wrong.push(format!("ds/hover_card.luau:{}: `{}`: the frame is the menus' glass alone", n + 1, line.trim()));
            }
        }
    }
    // What's built on it adds no surface of its own.
    for file in ON_FRAME {
        let path = root.join(file);
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        for (n, line) in text.lines().enumerate() {
            for needle in ["shadow =", "backdrop_blur =", "drop_shadow =", "glass.", "border ="] {
                if code_has(line, needle) {
                    wrong.push(format!("{file}:{}: `{}`", n + 1, line.trim()));
                }
            }
        }
    }
    assert!(wrong.is_empty(), "a hover card is the menus' surface, framed once:\n{}", wrong.join("\n"));
}

/// Whether `line`'s code (its comment cut) sets `needle`.
fn code_has(line: &str, needle: &str) -> bool {
    code(line).contains(needle)
}
