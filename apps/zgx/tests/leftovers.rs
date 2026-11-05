//! No debugging left behind: what a session of poking at a bug leaves in the
//! source, found before it ships (README, "Patterns").
//!
//! - In `ui/` (every `.luau`, tests included):
//!   - a `print(` whose message isn't a `zeron-zgx: …` log line (the one
//!     form the app logs in);
//!   - a call of a bare `warn(` or `log(` the file doesn't define itself;
//!   - a state, persisted value or shared value named like a probe (`probe`,
//!     `debug`, `dbg`, `tmp`, `temp`, `scratch` in its name);
//!   - a `TEMP`, `XXX`, `DEBUG` or `REMOVE ME` marker in a comment.
//! - In `src/` (outside tests):
//!   - an `eprintln!`, `println!`, `eprint!` or `print!` that isn't a
//!     `zeron-zgx: …` log line (or one of the few other known prefixes);
//!   - a `dbg!` anywhere, tests included;
//!   - the same comment markers.
//!
//! Text only: comments and string literals are told from code, so a demo
//! file's sample source or a comment quoting `print(` doesn't count. Runs in
//! well under a second.

use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Every file under `dir` with extension `ext`.
fn files(dir: &Path, ext: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut dirs = vec![dir.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|e| e == ext) {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// A file split into code (string contents and comments blanked, so offsets
/// and line numbers still match the source) and its comments.
struct Source {
    text: String,
    code: Vec<u8>,
    comments: Vec<(usize, String)>,
}

impl Source {
    fn line(&self, at: usize) -> usize {
        self.text.as_bytes()[..at].iter().filter(|&&b| b == b'\n').count() + 1
    }

    /// The string literal starting at or after `at` (skipping whitespace),
    /// without its quotes, if the next thing there is one.
    fn literal_after(&self, at: usize) -> Option<&str> {
        let bytes = self.text.as_bytes();
        let mut i = at;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let quote = *bytes.get(i)?;
        if !matches!(quote, b'"' | b'\'' | b'`') {
            return None;
        }
        let start = i + 1;
        let mut j = start;
        while j < bytes.len() && bytes[j] != quote && bytes[j] != b'\n' {
            j += if bytes[j] == b'\\' { 2 } else { 1 };
        }
        self.text.get(start..j.min(bytes.len()))
    }

    /// Where `needle` starts in the code, each time it does, as a whole
    /// call: not preceded by part of a longer name (or by `.` or `:` when
    /// `bare`).
    fn calls(&self, needle: &str, bare: bool) -> Vec<usize> {
        let code = &self.code;
        let n = needle.as_bytes();
        let mut out = Vec::new();
        if code.len() < n.len() {
            return out;
        }
        for at in 0..=code.len() - n.len() {
            if &code[at..at + n.len()] != n {
                continue;
            }
            if at > 0 {
                let before = code[at - 1];
                if before.is_ascii_alphanumeric() || before == b'_' || (bare && matches!(before, b'.' | b':')) {
                    continue;
                }
            }
            out.push(at);
        }
        out
    }
}

/// Blank `text[from..to]` in `code`, newlines kept.
fn blank(code: &mut [u8], from: usize, to: usize) {
    for b in &mut code[from..to] {
        if *b != b'\n' {
            *b = b' ';
        }
    }
}

/// `[[`, `[=[`, … at `i`: the level (count of `=`), if it opens a long bracket.
fn long_open(b: &[u8], i: usize) -> Option<usize> {
    if b.get(i) != Some(&b'[') {
        return None;
    }
    let mut j = i + 1;
    while b.get(j) == Some(&b'=') {
        j += 1;
    }
    (b.get(j) == Some(&b'[')).then_some(j - i - 1)
}

/// The end (one past) of a long bracket of `level` whose body starts at `i`.
fn long_close(b: &[u8], mut i: usize, level: usize) -> usize {
    while i < b.len() {
        if b[i] == b']' {
            let mut j = i + 1;
            while j < b.len() && b[j] == b'=' {
                j += 1;
            }
            if j - i - 1 == level && b.get(j) == Some(&b']') {
                return j + 1;
            }
        }
        i += 1;
    }
    b.len()
}

fn luau(text: String) -> Source {
    let b = text.as_bytes();
    let mut code = b.to_vec();
    let mut comments = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'-' && b.get(i + 1) == Some(&b'-') {
            let end = if let Some(level) = long_open(b, i + 2) {
                long_close(b, i + 4 + level, level)
            } else {
                b[i..].iter().position(|&c| c == b'\n').map_or(b.len(), |n| i + n)
            };
            comments.push((i, text[i + 2..end].to_owned()));
            blank(&mut code, i, end);
            i = end;
        } else if let Some(level) = long_open(b, i) {
            let end = long_close(b, i + 2 + level, level);
            blank(&mut code, i + 1, end.saturating_sub(1).max(i + 1));
            i = end;
        } else if matches!(b[i], b'"' | b'\'' | b'`') {
            let quote = b[i];
            let mut j = i + 1;
            while j < b.len() && b[j] != quote && b[j] != b'\n' {
                j += if b[j] == b'\\' { 2 } else { 1 };
            }
            let end = j.min(b.len());
            blank(&mut code, i + 1, end);
            i = end + 1;
        } else {
            i += 1;
        }
    }
    Source { text, code, comments }
}

fn rust(text: String) -> Source {
    let b = text.as_bytes();
    let mut code = b.to_vec();
    let mut comments = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'/' && b.get(i + 1) == Some(&b'/') {
            let end = b[i..].iter().position(|&c| c == b'\n').map_or(b.len(), |n| i + n);
            comments.push((i, text[i + 2..end].to_owned()));
            blank(&mut code, i, end);
            i = end;
        } else if b[i] == b'/' && b.get(i + 1) == Some(&b'*') {
            let end = text[i + 2..].find("*/").map_or(b.len(), |n| i + 2 + n + 2);
            comments.push((i, text[i + 2..end.saturating_sub(2).max(i + 2)].to_owned()));
            blank(&mut code, i, end);
            i = end;
        } else if b[i] == b'r' && matches!(b.get(i + 1), Some(b'"' | b'#')) && (i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_')) {
            // A raw string: r"…", r#"…"#, …
            let mut j = i + 1;
            while b.get(j) == Some(&b'#') {
                j += 1;
            }
            if b.get(j) != Some(&b'"') {
                i += 1;
                continue;
            }
            let hashes = j - i - 1;
            let close = format!("\"{}", "#".repeat(hashes));
            let end = text[j + 1..].find(&close).map_or(b.len(), |n| j + 1 + n + close.len());
            blank(&mut code, j + 1, end.saturating_sub(close.len()).max(j + 1));
            i = end;
        } else if b[i] == b'"' {
            let mut j = i + 1;
            while j < b.len() && b[j] != b'"' {
                j += if b[j] == b'\\' { 2 } else { 1 };
            }
            let end = j.min(b.len());
            blank(&mut code, i + 1, end);
            i = end + 1;
        } else if b[i] == b'\'' {
            // A char literal ('x', '\n', '{'), not a lifetime ('a).
            let len = if b.get(i + 1) == Some(&b'\\') {
                b[i + 2..].iter().position(|&c| c == b'\'').map(|n| n + 3)
            } else {
                let ch = text[i + 1..].chars().next().map_or(1, char::len_utf8);
                (b.get(i + 1 + ch) == Some(&b'\'')).then_some(ch + 2)
            };
            match len {
                Some(len) => {
                    blank(&mut code, i + 1, i + len - 1);
                    i += len;
                }
                None => i += 1,
            }
        } else {
            i += 1;
        }
    }
    Source { text, code, comments }
}

/// Where a `#[cfg(test)] mod … { … }` runs in `src`, as byte ranges.
fn test_modules(src: &Source) -> Vec<(usize, usize)> {
    let code = &src.code;
    let mut out = Vec::new();
    for at in src.calls("#[cfg(test)]", false) {
        let after = &src.code[at + 12..];
        let skip = after.iter().take_while(|b| b.is_ascii_whitespace()).count();
        let rest = &after[skip..];
        let is_mod = rest.starts_with(b"mod ") || rest.starts_with(b"pub mod ") || rest.starts_with(b"pub(crate) mod ");
        let Some(open) = is_mod.then(|| rest.iter().position(|&b| b == b'{' || b == b';')).flatten() else { continue };
        if rest[open] == b';' {
            continue;
        }
        let start = at + 12 + skip + open;
        let mut depth = 0usize;
        let mut end = code.len();
        for (k, &c) in code[start..].iter().enumerate() {
            match c {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = start + k + 1;
                        break;
                    }
                }
                _ => {}
            }
        }
        out.push((at, end));
    }
    out
}

/// A comment marking something to take out again.
fn marker(comment: &str) -> Option<&'static str> {
    let words: Vec<&str> = comment.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).collect();
    for word in ["TEMP", "XXX", "DEBUG"] {
        if words.contains(&word) {
            return Some(word);
        }
    }
    (comment.contains("REMOVE ME") || comment.to_ascii_lowercase().contains("remove before")).then_some("REMOVE ME")
}

/// The app's one log form, and the other prefixes known to be logs.
const LOG_PREFIXES: &[&str] = &["zeron-zgx: ", "terminal-bridge: "];

/// Source files whose output is the point (dev tools printing results).
const PRINTERS: &[&str] = &["src/audit_shots.rs"];

/// Prints that are what a person sees, by file: the terminal bridge's
/// stdout is the terminal itself (its notices in the pane).
const SHOWN: &[(&str, &str)] = &[("src/terminal.rs", "print!(")];

/// Words a probe's state is named with.
fn probe_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    if ["probe", "debug", "dbg"].iter().any(|w| lower.contains(w)) {
        return true;
    }
    lower.split(|c: char| !c.is_ascii_alphanumeric()).any(|w| matches!(w, "tmp" | "temp" | "scratch" | "xxx"))
}

fn rel(path: &Path) -> String {
    path.strip_prefix(root()).unwrap_or(path).display().to_string()
}

fn ui_findings() -> Vec<String> {
    let mut out = Vec::new();
    for path in files(&root().join("ui"), "luau") {
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let src = luau(text);
        let file = rel(&path);
        let code = String::from_utf8_lossy(&src.code).into_owned();

        for at in src.calls("print(", true) {
            let message = src.literal_after(at + 6).unwrap_or("");
            if !LOG_PREFIXES.iter().any(|p| message.starts_with(p)) {
                out.push(format!("{file}:{}: print( that isn't a `zeron-zgx: …` log line", src.line(at)));
            }
        }
        for name in ["warn", "log"] {
            let defined = code.contains(&format!("local function {name}(")) || code.contains(&format!("local {name} ="));
            if defined {
                continue;
            }
            for at in src.calls(&format!("{name}("), true) {
                out.push(format!("{file}:{}: a bare {name}( call", src.line(at)));
            }
        }
        for kind in ["ui.state(", "ui.persist(", "ui.shared("] {
            for at in src.calls(kind, false) {
                if let Some(name) = src.literal_after(at + kind.len()).filter(|n| probe_name(n)) {
                    out.push(format!("{file}:{}: {kind}\"{name}\"…) is named like a probe", src.line(at)));
                }
            }
        }
        for (at, comment) in &src.comments {
            if let Some(word) = marker(comment) {
                out.push(format!("{file}:{}: a {word} marker", src.line(*at)));
            }
        }
    }
    out
}

fn src_findings() -> Vec<String> {
    let mut out = Vec::new();
    for path in files(&root().join("src"), "rs") {
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let src = rust(text);
        let file = rel(&path);
        let test_file = file.ends_with("_tests.rs");
        let tests = test_modules(&src);
        let in_test = |at: usize| test_file || tests.iter().any(|&(a, b)| a <= at && at < b);

        for at in src.calls("dbg!(", false) {
            out.push(format!("{file}:{}: a dbg!", src.line(at)));
        }
        if !PRINTERS.contains(&file.as_str()) {
            for mac in ["eprintln!(", "println!(", "eprint!(", "print!("] {
                if SHOWN.contains(&(file.as_str(), mac)) {
                    continue;
                }
                for at in src.calls(mac, false) {
                    if in_test(at) {
                        continue;
                    }
                    let message = src.literal_after(at + mac.len()).unwrap_or("");
                    if !LOG_PREFIXES.iter().any(|p| message.starts_with(p)) {
                        out.push(format!("{file}:{}: {} that isn't a `zeron-zgx: …` log line", src.line(at), &mac[..mac.len() - 1]));
                    }
                }
            }
        }
        for (at, comment) in &src.comments {
            if let Some(word) = marker(comment) {
                out.push(format!("{file}:{}: a {word} marker", src.line(*at)));
            }
        }
    }
    out
}

#[test]
fn no_debugging_left_in_ui() {
    let found = ui_findings();
    assert!(found.is_empty(), "{} debugging leftovers in ui/:\n{}", found.len(), found.join("\n"));
}

#[test]
fn no_debugging_left_in_src() {
    let found = src_findings();
    assert!(found.is_empty(), "{} debugging leftovers in src/:\n{}", found.len(), found.join("\n"));
}

/// The scanner itself: what it must catch and what it must let be.
#[test]
fn the_scanner_tells_code_from_text() {
    let s = luau(
        "local x = ui.state(\"findprobe_log\", 0)\n\
         print(`[x] {x}`)\n\
         print(`zeron-zgx: fine {x}`)\n\
         local demo = [[print(1) -- TEMP]]\n\
         local q = \"print(2)\"\n\
         -- quoting print( in prose is fine\n\
         --[[ XXX: take out ]]\n\
         t:warn(1)\n"
            .to_owned(),
    );
    let prints: Vec<usize> = s.calls("print(", true).iter().map(|&a| s.line(a)).collect();
    assert_eq!(prints, vec![2, 3]);
    assert_eq!(s.literal_after(s.calls("print(", true)[1] + 6), Some("zeron-zgx: fine {x}"));
    assert_eq!(s.calls("warn(", true), Vec::<usize>::new());
    assert!(probe_name(s.literal_after(s.calls("ui.state(", false)[0] + 9).unwrap()));
    assert!(!probe_name("templates_tab"));
    let marks: Vec<_> = s.comments.iter().filter_map(|(_, c)| marker(c)).collect();
    assert_eq!(marks, vec!["XXX"]);

    let r = rust(
        "fn a() { eprintln!(\"zeron-zgx: ok\"); let c = '{'; let s = r#\"dbg!(x)\"#; }\n\
         #[cfg(test)]\n\
         mod tests { fn b() { eprintln!(\"loose\"); } }\n\
         fn c() { println!(\"loose\"); }\n"
            .to_owned(),
    );
    assert!(r.calls("dbg!(", false).is_empty());
    let tests = test_modules(&r);
    let loose: Vec<usize> = r
        .calls("println!(", false)
        .into_iter()
        .filter(|&at| !tests.iter().any(|&(a, b)| a <= at && at < b))
        .map(|at| r.line(at))
        .collect();
    assert_eq!(loose, vec![4]);
}
