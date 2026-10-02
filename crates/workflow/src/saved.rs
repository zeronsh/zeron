//! The saved-workflow file format: a Starlark script that starts with a
//! frontmatter comment block.
//!
//! ```text
//! # zeron-workflow
//! # name: pr-review
//! # description: Review the pending changes with three reviewers and confirm findings
//! # when_to_use: When the user asks for a thorough multi-agent review
//! # args:
//! #   base: {type: string, default: "main", description: "Branch to diff against"}
//! #   deep: {type: bool, default: false}
//! #   ticket: {type: int, required: true}
//!
//! def main(args):
//!     ...
//! ```
//!
//! Comments keep the file a valid script — the editor highlights it, the
//! engine runs the **whole file** (so a run's script hash pins the frontmatter
//! too, and line numbers in diagnostics are file lines).
//!
//! # Grammar
//!
//! * The block is the leading run of lines that start with `#`; it ends at the
//!   first line that does not (end it with a blank line — a plain comment
//!   directly under it would be read as frontmatter). The first line is
//!   exactly `# zeron-workflow`.
//! * After the `#` comes one optional space, then either nothing (a spacer),
//!   a top-level `key: value` (no indent), or — under `args:` — an indented
//!   `argname: {flow mapping}`.
//! * Top-level keys: `name` (optional; must equal the file name), `description`
//!   (required), `when_to_use`, `args`. Values run to the end of the line and
//!   are plain text (no quoting, no continuation lines). Unknown or repeated
//!   keys are errors.
//! * An argument is `{type: T, required: true, default: V, description: "…"}`,
//!   keys in any order, each at most once. `type` is a bare word
//!   (`string int number bool json`); `default` and `description` are JSON
//!   values (strings double-quoted); `required` is `true` or `false`. A
//!   required argument has no default; a default must have the argument's type.
//! * Errors carry `path:line:col message` and are **all** reported, not only
//!   the first.
//!
//! Parsing is total over arbitrary text: bounded sizes, no allocation driven by
//! the input beyond the bounds, no panics (tested with hostile input).

use serde_json::Value;
use zeron_proto::saved_workflow::*;

use crate::diagnostic::Diagnostic;
use crate::limits::MAX_SCRIPT_BYTES;

/// The first line of every saved workflow.
pub const MARK: &str = "# zeron-workflow";
/// The frontmatter block as a whole.
pub const MAX_FRONTMATTER_BYTES: usize = 16 * 1024;
pub const MAX_FRONTMATTER_LINES: usize = 200;

/// What a saved file declares about itself.
#[derive(Debug, Clone, PartialEq)]
pub struct SavedMeta {
    pub name: Option<String>,
    pub description: String,
    pub when_to_use: Option<String>,
    pub args: Vec<SavedArg>,
}

/// A parsed file: its metadata and where the block ended.
#[derive(Debug, Clone, PartialEq)]
pub struct SavedFile {
    pub meta: SavedMeta,
    /// Lines the frontmatter block occupies (the script proper starts after).
    pub frontmatter_lines: usize,
}

/// A bundled workflow (read-only, shown under "Built-in").
#[derive(Debug, Clone, Copy)]
pub struct Builtin {
    pub name: &'static str,
    pub source: &'static str,
}

const BUILTINS: &[Builtin] = &[
    Builtin {
        name: "pr-review",
        source: include_str!("../builtin/pr-review.star"),
    },
    Builtin {
        name: "fix-until-green",
        source: include_str!("../builtin/fix-until-green.star"),
    },
    Builtin {
        name: "repo-audit",
        source: include_str!("../builtin/repo-audit.star"),
    },
];

/// The workflows that ship with the app.
pub fn builtins() -> &'static [Builtin] {
    BUILTINS
}

fn strip_bom(text: &str) -> &str {
    text.strip_prefix('\u{feff}').unwrap_or(text)
}

/// Does the file open with the frontmatter mark?
pub fn has_frontmatter(text: &str) -> bool {
    strip_bom(text)
        .lines()
        .next()
        .is_some_and(|l| l.trim_end() == MARK)
}

/// Lines of the frontmatter block (the leading run of `#` lines), 0 when the
/// file does not open with the mark.
fn block_len(text: &str) -> usize {
    if !has_frontmatter(text) {
        return 0;
    }
    strip_bom(text)
        .lines()
        .take_while(|l| l.starts_with('#'))
        .count()
}

/// The script without its frontmatter block and the blank lines after it.
/// A file without the mark is returned unchanged.
pub fn strip_frontmatter(text: &str) -> &str {
    let text = strip_bom(text);
    let n = block_len(text);
    if n == 0 {
        return text;
    }
    let mut rest = text;
    for _ in 0..n {
        match rest.find('\n') {
            Some(i) => rest = &rest[i + 1..],
            None => return "",
        }
    }
    rest.trim_start_matches(['\n', '\r'])
}

struct Errors<'a> {
    label: &'a str,
    list: Vec<Diagnostic>,
}

impl Errors<'_> {
    fn push(&mut self, line: usize, col: usize, message: impl Into<String>) {
        if self.list.len() < 50 {
            self.list.push(Diagnostic::new(
                self.label,
                line as u32,
                col as u32,
                message,
            ));
        }
    }
}

/// A cursor over one line's content with 1-based column tracking.
struct Cur<'a> {
    s: &'a str,
    pos: usize,
    /// Column (1-based, in characters) of `s[0]` in the file line.
    col0: usize,
}

impl<'a> Cur<'a> {
    fn col(&self) -> usize {
        self.col0 + self.s[..self.pos].chars().count()
    }

    fn rest(&self) -> &'a str {
        &self.s[self.pos..]
    }

    fn skip_ws(&mut self) {
        while self.rest().starts_with(' ') {
            self.pos += 1;
        }
    }

    fn eat(&mut self, c: char) -> bool {
        if self.rest().starts_with(c) {
            self.pos += c.len_utf8();
            true
        } else {
            false
        }
    }

    fn ident(&mut self) -> &'a str {
        let rest = self.rest();
        let n = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(rest.len());
        self.pos += n;
        &rest[..n]
    }
}

fn control_free(text: &str) -> bool {
    !text.chars().any(char::is_control)
}

/// Parse the frontmatter of `text`. `label` names the file in diagnostics;
/// `expected_name` is the file stem — a `name:` key that disagrees with it is
/// an error (the file name is the identity).
pub fn parse(
    label: &str,
    expected_name: Option<&str>,
    text: &str,
) -> Result<SavedFile, Vec<Diagnostic>> {
    let mut e = Errors {
        label,
        list: Vec::new(),
    };
    if text.len() > MAX_SCRIPT_BYTES {
        return Err(vec![Diagnostic::unpositioned(
            label,
            format!("the file is larger than {} KB", MAX_SCRIPT_BYTES / 1024),
        )]);
    }
    let text = strip_bom(text);
    if !has_frontmatter(text) {
        return Err(vec![Diagnostic::new(
            label,
            1,
            1,
            format!(
                "a saved workflow starts with the line `{MARK}` and a frontmatter comment block"
            ),
        )]);
    }
    let lines: Vec<&str> = text
        .lines()
        .take(MAX_FRONTMATTER_LINES + 1)
        .take_while(|l| l.starts_with('#'))
        .collect();
    if lines.len() > MAX_FRONTMATTER_LINES {
        e.push(
            MAX_FRONTMATTER_LINES + 1,
            1,
            format!("the frontmatter is longer than {MAX_FRONTMATTER_LINES} lines"),
        );
        return Err(e.list);
    }
    let bytes: usize = lines.iter().map(|l| l.len() + 1).sum();
    if bytes > MAX_FRONTMATTER_BYTES {
        e.push(
            1,
            1,
            format!(
                "the frontmatter is larger than {} KB",
                MAX_FRONTMATTER_BYTES / 1024
            ),
        );
        return Err(e.list);
    }

    let mut name: Option<(usize, String)> = None;
    let mut description: Option<(usize, String)> = None;
    let mut when: Option<(usize, String)> = None;
    let mut args_line: Option<usize> = None;
    let mut args: Vec<SavedArg> = Vec::new();
    let mut in_args = false;

    for (i, raw) in lines.iter().enumerate().skip(1) {
        let line_no = i + 1;
        let mut consumed = 1; // the '#'
        let mut content = &raw[1..];
        if let Some(r) = content.strip_prefix(' ') {
            content = r;
            consumed += 1;
        }
        let content = content.trim_end();
        if content.is_empty() {
            continue;
        }
        if content.starts_with('\t') || content.trim_start_matches(' ').starts_with('\t') {
            e.push(line_no, consumed + 1, "indent with spaces, not tabs");
            continue;
        }
        let indent = content.len() - content.trim_start_matches(' ').len();
        let body = &content[indent..];
        let col = consumed + indent + 1;
        if !control_free(body) {
            e.push(line_no, col, "control characters are not allowed");
            continue;
        }
        if indent == 0 {
            let Some((key, value)) = body.split_once(':') else {
                e.push(
                    line_no,
                    col,
                    "expected `key: value` (end the frontmatter with a blank line before ordinary comments)",
                );
                continue;
            };
            let value_col = col + key.chars().count() + 1;
            let value = value.trim();
            let slot = match key {
                "name" => Some(&mut name),
                "description" => Some(&mut description),
                "when_to_use" => Some(&mut when),
                "args" => None,
                other => {
                    e.push(
                        line_no,
                        col,
                        format!(
                            "unknown key `{other}` (expected name, description, when_to_use or args)"
                        ),
                    );
                    in_args = false;
                    continue;
                }
            };
            match slot {
                Some(slot) => {
                    in_args = false;
                    if let Some((first, _)) = slot {
                        e.push(
                            line_no,
                            col,
                            format!("duplicate key `{key}` (first at line {first})"),
                        );
                    } else if value.is_empty() {
                        e.push(line_no, value_col, format!("`{key}` has no value"));
                    } else {
                        *slot = Some((line_no, value.to_owned()));
                    }
                }
                None => {
                    if let Some(first) = args_line {
                        e.push(
                            line_no,
                            col,
                            format!("duplicate key `args` (first at line {first})"),
                        );
                    }
                    args_line.get_or_insert(line_no);
                    if !(value.is_empty() || value == "{}") {
                        e.push(
                            line_no,
                            value_col,
                            "`args:` takes no inline value; list the arguments on the indented lines below it",
                        );
                    }
                    in_args = true;
                }
            }
            continue;
        }
        if !in_args {
            e.push(
                line_no,
                col,
                "an indented line is only allowed under `args:`",
            );
            continue;
        }
        let mut cur = Cur {
            s: body,
            pos: 0,
            col0: col,
        };
        if let Some(arg) = parse_arg_line(&mut cur, line_no, &mut e) {
            if args.iter().any(|a| a.name == arg.name) {
                e.push(line_no, col, format!("duplicate argument `{}`", arg.name));
            } else if args.len() >= SAVED_MAX_ARGS {
                e.push(
                    line_no,
                    col,
                    format!("a workflow declares at most {SAVED_MAX_ARGS} arguments"),
                );
            } else {
                args.push(arg);
            }
        }
    }

    // Top-level checks.
    if let Some((ln, n)) = &name {
        if let Err(msg) = valid_saved_name(n) {
            e.push(*ln, 9, msg);
        } else if let Some(expected) = expected_name
            && expected != n
        {
            e.push(
                *ln,
                9,
                format!("`name: {n}` does not match the file name `{expected}.star`"),
            );
        }
    }
    match &description {
        None => e.push(1, 1, "missing required `description:`"),
        Some((ln, d)) => {
            if d.chars().count() > SAVED_MAX_DESCRIPTION_CHARS {
                e.push(
                    *ln,
                    1,
                    format!(
                        "`description` is longer than {SAVED_MAX_DESCRIPTION_CHARS} characters"
                    ),
                );
            }
        }
    }
    if let Some((ln, w)) = &when
        && w.chars().count() > SAVED_MAX_WHEN_TO_USE_CHARS
    {
        e.push(
            *ln,
            1,
            format!("`when_to_use` is longer than {SAVED_MAX_WHEN_TO_USE_CHARS} characters"),
        );
    }

    if !e.list.is_empty() {
        return Err(e.list);
    }
    Ok(SavedFile {
        meta: SavedMeta {
            name: name.map(|(_, n)| n),
            description: description.map(|(_, d)| d).unwrap_or_default(),
            when_to_use: when.map(|(_, w)| w),
            args,
        },
        frontmatter_lines: lines.len(),
    })
}

/// `argname: {type: …, …}`.
fn parse_arg_line(cur: &mut Cur<'_>, line: usize, e: &mut Errors<'_>) -> Option<SavedArg> {
    let name_col = cur.col();
    let name = cur.ident();
    if name.is_empty() {
        e.push(line, name_col, "expected an argument name");
        return None;
    }
    if let Err(msg) = valid_arg_name(name) {
        e.push(line, name_col, msg);
        return None;
    }
    cur.skip_ws();
    if !cur.eat(':') {
        e.push(
            line,
            cur.col(),
            format!("expected `:` after the argument name `{name}`"),
        );
        return None;
    }
    cur.skip_ws();
    let brace_col = cur.col();
    if !cur.eat('{') {
        e.push(
            line,
            brace_col,
            format!("expected `{{type: …}}` after `{name}:`"),
        );
        return None;
    }

    let mut ty: Option<SavedArgType> = None;
    let mut required: Option<bool> = None;
    let mut default: Option<Value> = None;
    let mut description: Option<String> = None;
    let mut seen: Vec<&str> = Vec::new();
    let mut ok = true;
    cur.skip_ws();
    if !cur.rest().starts_with('}') {
        loop {
            cur.skip_ws();
            let key_col = cur.col();
            let key = cur.ident();
            if key.is_empty() {
                e.push(
                    line,
                    key_col,
                    "expected a key (type, required, default or description)",
                );
                return None;
            }
            if !matches!(key, "type" | "required" | "default" | "description") {
                e.push(
                    line,
                    key_col,
                    format!(
                        "unknown key `{key}` (expected type, required, default or description)"
                    ),
                );
                ok = false;
            } else if seen.contains(&key) {
                e.push(line, key_col, format!("duplicate key `{key}`"));
                ok = false;
            }
            seen.push(key);
            cur.skip_ws();
            if !cur.eat(':') {
                e.push(line, cur.col(), format!("expected `:` after `{key}`"));
                return None;
            }
            cur.skip_ws();
            let value_col = cur.col();
            if key == "type" {
                let word = if cur.rest().starts_with('"') {
                    match read_json(cur, line, e) {
                        Some(Value::String(s)) => s,
                        Some(_) => {
                            e.push(line, value_col, "`type` must be a word such as `string`");
                            return None;
                        }
                        None => return None,
                    }
                } else {
                    cur.ident().to_owned()
                };
                match SavedArgType::parse(&word) {
                    Some(t) => ty = Some(t),
                    None => {
                        let hint = match word.as_str() {
                            "boolean" => " (use `bool`)",
                            "integer" => " (use `int`)",
                            "str" | "text" => " (use `string`)",
                            "float" | "double" => " (use `number`)",
                            _ => "",
                        };
                        e.push(
                            line,
                            value_col,
                            format!("unknown type `{word}`{hint}; expected string, int, number, bool or json"),
                        );
                        ok = false;
                    }
                }
            } else {
                let Some(value) = read_json(cur, line, e) else {
                    return None;
                };
                match key {
                    "required" => match value {
                        Value::Bool(b) => required = Some(b),
                        _ => {
                            e.push(line, value_col, "`required` must be true or false");
                            ok = false;
                        }
                    },
                    "description" => match value {
                        Value::String(s)
                            if control_free(&s)
                                && s.chars().count() <= SAVED_MAX_ARG_DESCRIPTION_CHARS =>
                        {
                            description = Some(s);
                        }
                        Value::String(_) => {
                            e.push(
                                line,
                                value_col,
                                format!("`description` must be one line of at most {SAVED_MAX_ARG_DESCRIPTION_CHARS} characters"),
                            );
                            ok = false;
                        }
                        _ => {
                            e.push(
                                line,
                                value_col,
                                "`description` must be a double-quoted string",
                            );
                            ok = false;
                        }
                    },
                    "default" => default = Some(value),
                    _ => {}
                }
            }
            cur.skip_ws();
            if cur.eat(',') {
                cur.skip_ws();
                if cur.rest().starts_with('}') {
                    e.push(line, cur.col(), "a trailing `,` is not allowed");
                    return None;
                }
                continue;
            }
            break;
        }
    }
    let close_col = cur.col();
    if !cur.eat('}') {
        e.push(line, close_col, "expected `,` or `}`");
        return None;
    }
    cur.skip_ws();
    if !cur.rest().is_empty() {
        e.push(line, cur.col(), "unexpected text after `}`");
        return None;
    }
    let Some(ty) = ty else {
        if ok {
            e.push(line, brace_col, format!("argument `{name}` needs a `type`"));
        }
        return None;
    };
    let arg = SavedArg {
        name: name.to_owned(),
        ty,
        required: required.unwrap_or(false),
        default,
        description,
    };
    if !ok {
        return None;
    }
    if arg.required && arg.default.is_some() {
        e.push(
            line,
            brace_col,
            format!("argument `{name}` is required, so it cannot have a default"),
        );
        return None;
    }
    if let Some(d) = &arg.default {
        // A default must have the type (and the size limits) of the argument.
        if d.is_null() && arg.ty != SavedArgType::Json {
            e.push(
                line,
                brace_col,
                format!("argument `{name}`: a default of null is only allowed for json"),
            );
            return None;
        }
        if let Err(msg) = arg.check(d) {
            e.push(line, brace_col, format!("{msg} (default value)"));
            return None;
        }
    }
    Some(arg)
}

/// One JSON value at the cursor (strings, numbers, booleans, null, arrays,
/// objects); advances past it.
fn read_json(cur: &mut Cur<'_>, line: usize, e: &mut Errors<'_>) -> Option<Value> {
    let col = cur.col();
    let rest = cur.rest();
    let mut stream = serde_json::Deserializer::from_str(rest).into_iter::<Value>();
    match stream.next() {
        Some(Ok(v)) => {
            cur.pos += stream.byte_offset();
            Some(v)
        }
        Some(Err(err)) => {
            let hint = if rest.starts_with(|c: char| c.is_ascii_alphabetic())
                && !rest.starts_with("true")
                && !rest.starts_with("false")
                && !rest.starts_with("null")
            {
                " (write strings in double quotes)"
            } else {
                ""
            };
            // serde_json's own position is relative to the slice; fold it in.
            let extra = if err.line() == 1 {
                err.column().saturating_sub(1)
            } else {
                0
            };
            let msg = err.to_string();
            let msg = msg.split(" at line ").next().unwrap_or(&msg);
            e.push(line, col + extra, format!("invalid value: {msg}{hint}"));
            None
        }
        None => {
            e.push(line, col, "expected a value");
            None
        }
    }
}

// ── writing ───────────────────────────────────────────────────────────────

/// Check what `render` is about to write (the same rules `parse` applies).
pub fn validate_meta(meta: &SavedMeta) -> Result<(), String> {
    let name = meta.name.as_deref().ok_or("the workflow needs a name")?;
    valid_saved_name(name)?;
    for (what, text, max) in [
        (
            "description",
            Some(meta.description.as_str()),
            SAVED_MAX_DESCRIPTION_CHARS,
        ),
        (
            "when_to_use",
            meta.when_to_use.as_deref(),
            SAVED_MAX_WHEN_TO_USE_CHARS,
        ),
    ] {
        let Some(text) = text else { continue };
        if text.trim().is_empty() && what == "description" {
            return Err("the description is empty".into());
        }
        if !control_free(text) {
            return Err(format!(
                "`{what}` must be a single line without control characters"
            ));
        }
        if text.trim() != text {
            return Err(format!("`{what}` has leading or trailing spaces"));
        }
        if text.chars().count() > max {
            return Err(format!("`{what}` is longer than {max} characters"));
        }
    }
    if meta.args.len() > SAVED_MAX_ARGS {
        return Err(format!("at most {SAVED_MAX_ARGS} arguments"));
    }
    let mut seen = std::collections::HashSet::new();
    for a in &meta.args {
        valid_arg_name(&a.name)?;
        if !seen.insert(a.name.as_str()) {
            return Err(format!("duplicate argument `{}`", a.name));
        }
        if a.required && a.default.is_some() {
            return Err(format!(
                "argument `{}` is required, so it cannot have a default",
                a.name
            ));
        }
        if let Some(d) = &a.default {
            if d.is_null() && a.ty != SavedArgType::Json {
                return Err(format!(
                    "argument `{}`: null default is only for json",
                    a.name
                ));
            }
            a.check(d)?;
        }
        if let Some(desc) = &a.description
            && (!control_free(desc) || desc.chars().count() > SAVED_MAX_ARG_DESCRIPTION_CHARS)
        {
            return Err(format!(
                "argument `{}`: the description must be one line of at most {SAVED_MAX_ARG_DESCRIPTION_CHARS} characters",
                a.name
            ));
        }
    }
    Ok(())
}

/// The frontmatter block for `meta` (always ends with a blank line). Key order
/// is fixed so saving twice writes the same bytes (no phantom git diffs).
pub fn render_frontmatter(meta: &SavedMeta) -> Result<String, String> {
    validate_meta(meta)?;
    let json = |v: &Value| serde_json::to_string(v).map_err(|e| e.to_string());
    let mut out = String::new();
    out.push_str(MARK);
    out.push('\n');
    if let Some(name) = &meta.name {
        out.push_str(&format!("# name: {name}\n"));
    }
    out.push_str(&format!("# description: {}\n", meta.description));
    if let Some(w) = &meta.when_to_use {
        out.push_str(&format!("# when_to_use: {w}\n"));
    }
    if !meta.args.is_empty() {
        out.push_str("# args:\n");
        for a in &meta.args {
            let mut fields = vec![format!("type: {}", a.ty.as_str())];
            if a.required {
                fields.push("required: true".into());
            }
            if let Some(d) = &a.default {
                fields.push(format!("default: {}", json(d)?));
            }
            if let Some(desc) = &a.description {
                fields.push(format!(
                    "description: {}",
                    json(&Value::String(desc.clone()))?
                ));
            }
            out.push_str(&format!("#   {}: {{{}}}\n", a.name, fields.join(", ")));
        }
    }
    out.push('\n');
    Ok(out)
}

/// The whole file: frontmatter for `meta`, then `script` — whose own
/// frontmatter, if it has one, is replaced (the caller's metadata wins).
pub fn render(meta: &SavedMeta, script: &str) -> Result<String, String> {
    let head = render_frontmatter(meta)?;
    let body = strip_frontmatter(script).trim_end();
    let mut out = head;
    out.push_str(body);
    out.push('\n');
    if out.len() > MAX_SCRIPT_BYTES {
        return Err(format!(
            "the file would be larger than {} KB",
            MAX_SCRIPT_BYTES / 1024
        ));
    }
    // The writer and the parser must agree; never write a file we cannot read.
    let back = parse("saved.star", meta.name.as_deref(), &out)
        .map_err(|d| crate::diagnostic::render(&d))?;
    if back.meta != *meta {
        return Err("the metadata does not survive a round trip through the file format".into());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const GOOD: &str = r#"# zeron-workflow
# name: pr-review
# description: Review the pending changes with three reviewers and confirm findings
# when_to_use: When the user asks for a thorough multi-agent review
# args:
#   base: {type: string, default: "main", description: "Branch to diff against"}
#   deep: {type: bool, default: false}
#   ticket: {type: int, required: true}
#   extra: {type: json}

def main(args):
    phase("x")
    return agent("a").ask("hi").result().value
"#;

    fn errors(text: &str) -> Vec<String> {
        parse("f.star", Some("pr-review"), text)
            .unwrap_err()
            .iter()
            .map(ToString::to_string)
            .collect()
    }

    #[test]
    fn the_documented_example_parses() {
        let file = parse("pr-review.star", Some("pr-review"), GOOD).unwrap();
        assert_eq!(file.frontmatter_lines, 9);
        let m = file.meta;
        assert_eq!(m.name.as_deref(), Some("pr-review"));
        assert!(m.description.starts_with("Review the pending"));
        assert!(m.when_to_use.unwrap().starts_with("When the user"));
        assert_eq!(m.args.len(), 4);
        assert_eq!(m.args[0].default, Some(json!("main")));
        assert_eq!(
            m.args[0].description.as_deref(),
            Some("Branch to diff against")
        );
        assert_eq!(m.args[1].ty, SavedArgType::Bool);
        assert!(m.args[2].required && m.args[2].default.is_none());
        assert_eq!(m.args[3].ty, SavedArgType::Json);
    }

    #[test]
    fn the_whole_file_still_analyses_as_a_script() {
        let analysis = crate::analyze("pr-review.star", GOOD).unwrap();
        assert_eq!(analysis.graph.phase_names(), ["x"]);
    }

    #[test]
    fn description_alone_is_enough_and_name_is_optional() {
        let f = parse(
            "a.star",
            Some("a"),
            "# zeron-workflow\n# description: d\n\ndef main(args):\n    pass\n",
        )
        .unwrap();
        assert!(f.meta.name.is_none() && f.meta.args.is_empty());
        assert_eq!(f.frontmatter_lines, 2);
    }

    #[test]
    fn crlf_and_bom_are_accepted() {
        let text = "\u{feff}# zeron-workflow\r\n# description: d\r\n# args:\r\n#   n: {type: int, default: 3}\r\n\r\nx = 1\r\n";
        let f = parse("a.star", None, text).unwrap();
        assert_eq!(f.meta.args[0].default, Some(json!(3)));
    }

    #[test]
    fn missing_mark_is_a_clear_error() {
        let e = errors("def main(args):\n    pass\n");
        assert_eq!(e.len(), 1);
        assert!(
            e[0].starts_with("f.star:1:1 a saved workflow starts with the line `# zeron-workflow`"),
            "{e:?}"
        );
        // Near misses are not the mark.
        for text in [
            "#zeron-workflow\n# description: d\n",
            "  # zeron-workflow\n",
            "# zeron-workflow2\n# description: d\n",
            "\n# zeron-workflow\n",
        ] {
            assert!(!has_frontmatter(text), "{text:?}");
        }
    }

    #[test]
    fn errors_are_all_reported_with_line_and_column() {
        let text = "# zeron-workflow\n# bogus: 1\n# description: d\n# description: again\n# args:\n#   a: {type: nope}\n#   b: {type: int, default: \"x\"}\n#   c: {type: string, wat: 1}\n\n";
        let e = errors(text);
        assert!(
            e.iter()
                .any(|m| m.starts_with("f.star:2:3 unknown key `bogus`")),
            "{e:?}"
        );
        assert!(
            e.iter()
                .any(|m| m.starts_with("f.star:4:3 duplicate key `description` (first at line 3)")),
            "{e:?}"
        );
        assert!(
            e.iter()
                .any(|m| m.contains("f.star:6:") && m.contains("unknown type `nope`")),
            "{e:?}"
        );
        assert!(
            e.iter()
                .any(|m| m.contains("f.star:7:") && m.contains("expected an int")),
            "{e:?}"
        );
        assert!(
            e.iter()
                .any(|m| m.contains("f.star:8:") && m.contains("unknown key `wat`")),
            "{e:?}"
        );
        assert!(e.len() >= 5);
    }

    #[test]
    fn columns_point_at_the_offending_token() {
        let text = "# zeron-workflow\n# description: d\n# args:\n#   n: {type: int, bogus: 1}\n";
        let e = errors(text);
        // "#   n: {type: int, bogus: 1}": `bogus` starts at char 20 (1-based).
        assert_eq!(
            e[0],
            "f.star:4:20 unknown key `bogus` (expected type, required, default or description)"
        );
    }

    #[test]
    fn duplicate_arguments_and_keys_are_rejected() {
        let e = errors(
            "# zeron-workflow\n# description: d\n# args:\n#   n: {type: int}\n#   n: {type: bool}\n",
        );
        assert!(
            e.iter().any(|m| m.contains("duplicate argument `n`")),
            "{e:?}"
        );
        let e =
            errors("# zeron-workflow\n# description: d\n# args:\n#   n: {type: int, type: bool}\n");
        assert!(
            e.iter().any(|m| m.contains("duplicate key `type`")),
            "{e:?}"
        );
        let e = errors("# zeron-workflow\n# description: d\n# args:\n# args:\n");
        assert!(
            e.iter().any(|m| m.contains("duplicate key `args`")),
            "{e:?}"
        );
        let e =
            errors("# zeron-workflow\n# name: pr-review\n# name: pr-review\n# description: d\n");
        assert!(
            e.iter().any(|m| m.contains("duplicate key `name`")),
            "{e:?}"
        );
    }

    #[test]
    fn required_and_defaults_are_checked() {
        let e = errors(
            "# zeron-workflow\n# description: d\n# args:\n#   n: {type: int, required: true, default: 3}\n",
        );
        assert!(
            e[0].contains("required, so it cannot have a default"),
            "{e:?}"
        );
        let e = errors(
            "# zeron-workflow\n# description: d\n# args:\n#   n: {type: bool, default: \"no\"}\n",
        );
        assert!(e[0].contains("expected a bool"), "{e:?}");
        let e = errors(
            "# zeron-workflow\n# description: d\n# args:\n#   n: {type: string, default: null}\n",
        );
        assert!(e[0].contains("null"), "{e:?}");
        let ok = parse(
            "f.star",
            None,
            "# zeron-workflow\n# description: d\n# args:\n#   n: {type: json, default: null}\n",
        )
        .unwrap();
        assert_eq!(ok.meta.args[0].default, Some(Value::Null));
        let e = errors(
            "# zeron-workflow\n# description: d\n# args:\n#   n: {type: string, required: \"yes\"}\n",
        );
        assert!(e[0].contains("`required` must be true or false"), "{e:?}");
        let e = errors("# zeron-workflow\n# description: d\n# args:\n#   n: {required: true}\n");
        assert!(e[0].contains("needs a `type`"), "{e:?}");
    }

    #[test]
    fn type_names_have_hints() {
        let e = errors("# zeron-workflow\n# description: d\n# args:\n#   n: {type: boolean}\n");
        assert!(e[0].contains("(use `bool`)"), "{e:?}");
        let e = errors("# zeron-workflow\n# description: d\n# args:\n#   n: {type: integer}\n");
        assert!(e[0].contains("(use `int`)"), "{e:?}");
    }

    #[test]
    fn unquoted_strings_get_a_hint() {
        let e = errors(
            "# zeron-workflow\n# description: d\n# args:\n#   n: {type: string, default: main}\n",
        );
        assert!(e[0].contains("write strings in double quotes"), "{e:?}");
    }

    #[test]
    fn malformed_mappings_are_errors_not_panics() {
        for line in [
            "n:",
            "n: {",
            "n: {type",
            "n: {type:",
            "n: {type: int",
            "n: {type: int,}",
            "n: {type: int} trailing",
            "n: type: int",
            "n: {,}",
            "n: {type int}",
            "n {type: int}",
            ": {type: int}",
            "n: [type]",
            "n: {type: \"in",
            "n: {type: int, default: [1, 2}",
            "n: {type: json, default: {\"a\": }}",
            "n: {type: 3}",
            "n: {\"type\": \"int\"}",
        ] {
            let text = format!("# zeron-workflow\n# description: d\n# args:\n#   {line}\n");
            let r = parse("f.star", None, &text);
            assert!(r.is_err(), "{line:?} should be rejected");
            for d in r.unwrap_err() {
                assert!(d.line >= 1 && d.col >= 1, "{line:?}: {d}");
            }
        }
    }

    #[test]
    fn a_plain_comment_under_the_block_is_reported_helpfully() {
        let e = errors(
            "# zeron-workflow\n# description: d\n# Helpers below\ndef main(args):\n    pass\n",
        );
        assert!(
            e[0].contains("expected `key: value`") && e[0].contains("blank line"),
            "{e:?}"
        );
        // With the blank line it is an ordinary comment.
        assert!(
            parse(
                "f.star",
                None,
                "# zeron-workflow\n# description: d\n\n# Helpers below\ndef main(args):\n    pass\n"
            )
            .is_ok()
        );
    }

    #[test]
    fn indentation_rules() {
        let e = errors("# zeron-workflow\n# description: d\n#   n: {type: int}\n");
        assert!(e[0].contains("only allowed under `args:`"), "{e:?}");
        let e = errors("# zeron-workflow\n# description: d\n# args:\n#\tn: {type: int}\n");
        assert!(e[0].contains("tabs"), "{e:?}");
        // A key after the args block ends it.
        let e = errors(
            "# zeron-workflow\n# args:\n#   n: {type: int}\n# when_to_use: w\n#   m: {type: int}\n# description: d\n",
        );
        assert!(
            e.iter()
                .any(|m| m.contains("f.star:5:") && m.contains("only allowed under `args:`")),
            "{e:?}"
        );
        // Any indentation depth under args works.
        assert!(parse("f.star", None, "# zeron-workflow\n# description: d\n# args:\n#  n: {type: int}\n#      m: {type: int}\n").is_ok());
    }

    #[test]
    fn name_must_match_the_file_and_be_a_slug() {
        let e = errors("# zeron-workflow\n# name: other\n# description: d\n");
        assert!(
            e[0].contains("does not match the file name `pr-review.star`"),
            "{e:?}"
        );
        let e = errors("# zeron-workflow\n# name: ../../etc/passwd\n# description: d\n");
        assert!(e[0].contains("must be lowercase letters"), "{e:?}");
        let e = errors("# zeron-workflow\n# name: Bad Name\n# description: d\n");
        assert!(e[0].contains("must be lowercase letters"), "{e:?}");
    }

    #[test]
    fn description_is_required_and_bounded() {
        let e = errors("# zeron-workflow\n# name: pr-review\n");
        assert!(
            e.iter()
                .any(|m| m.contains("missing required `description:`")),
            "{e:?}"
        );
        let long = "x".repeat(SAVED_MAX_DESCRIPTION_CHARS + 1);
        let e = errors(&format!("# zeron-workflow\n# description: {long}\n"));
        assert!(e[0].contains("longer than 300"), "{e:?}");
        let e = errors("# zeron-workflow\n# description:\n");
        assert!(
            e.iter().any(|m| m.contains("`description` has no value")),
            "{e:?}"
        );
        // Control characters never get through.
        let e = errors("# zeron-workflow\n# description: bad\u{7}bell\n");
        assert!(e.iter().any(|m| m.contains("control characters")), "{e:?}");
    }

    #[test]
    fn hostile_input_is_bounded_and_never_panics() {
        // Huge file.
        let big = format!(
            "# zeron-workflow\n# description: d\n\n{}",
            "x = 1\n".repeat(60_000)
        );
        assert!(big.len() > MAX_SCRIPT_BYTES);
        assert!(
            parse("f.star", None, &big).unwrap_err()[0]
                .message
                .contains("larger than 256 KB")
        );
        // Endless frontmatter.
        let long = format!("# zeron-workflow\n# description: d\n{}", "#\n".repeat(5000));
        assert!(
            parse("f.star", None, &long).unwrap_err()[0]
                .message
                .contains("longer than 200 lines")
        );
        // One enormous line.
        let wide = format!(
            "# zeron-workflow\n# description: d\n# args:\n#   n: {{type: string, default: \"{}\"}}\n",
            "a".repeat(40_000)
        );
        assert!(
            parse("f.star", None, &wide).unwrap_err()[0]
                .message
                .contains("frontmatter is larger than 16 KB")
        );
        // Deeply nested JSON default (serde_json's recursion limit trips, no stack overflow).
        let deep = format!(
            "# zeron-workflow\n# description: d\n# args:\n#   n: {{type: json, default: {}{}}}\n",
            "[".repeat(5000),
            "]".repeat(5000)
        );
        assert!(parse("f.star", None, &deep).is_err());
        // Many arguments.
        let many: String = (0..40)
            .map(|i| format!("#   a{i}: {{type: int}}\n"))
            .collect();
        let e = errors(&format!(
            "# zeron-workflow\n# description: d\n# args:\n{many}"
        ));
        assert!(
            e.iter().any(|m| m.contains("at most 32 arguments")),
            "{e:?}"
        );
        // Garbage bytes, odd unicode, lone marks.
        for text in [
            "",
            "#",
            "# zeron-workflow",
            "# zeron-workflow\n#",
            "\u{0}\u{1}",
            "# zeron-workflow\n# \u{202e}description: d\n",
            "# zeron-workflow\n# 描述: x\n# description: ü\n#   é: {type: int}\n",
        ] {
            let _ = parse("f.star", None, text);
        }
        // Error lists are capped.
        let spam: String = (0..150).map(|i| format!("# k{i}: v\n")).collect();
        let e = errors(&format!("# zeron-workflow\n{spam}"));
        assert!(e.len() <= 50);
    }

    #[test]
    fn unicode_columns_count_characters() {
        let text = "# zeron-workflow\n# description: d\n# args:\n#   é: {type: int}\n";
        let e = parse("f.star", None, text).unwrap_err();
        assert_eq!(e[0].col, 5, "{:?}", e[0]);
    }

    #[test]
    fn strip_frontmatter_leaves_the_script() {
        assert_eq!(
            strip_frontmatter(GOOD).lines().next(),
            Some("def main(args):")
        );
        assert_eq!(strip_frontmatter("def main(args):\n"), "def main(args):\n");
        assert_eq!(
            strip_frontmatter("# zeron-workflow\n# description: d\n"),
            ""
        );
        assert_eq!(strip_frontmatter("# zeron-workflow"), "");
    }

    fn sample_meta() -> SavedMeta {
        SavedMeta {
            name: Some("pr-review".into()),
            description: "Review: with \"quotes\" and ünïcode".into(),
            when_to_use: Some("When asked".into()),
            args: vec![
                SavedArg {
                    name: "base".into(),
                    ty: SavedArgType::String,
                    required: false,
                    default: Some(json!("ma\"in\n")),
                    description: Some("Branch: to \"diff\"".into()),
                },
                SavedArg {
                    name: "n".into(),
                    ty: SavedArgType::Number,
                    required: false,
                    default: Some(json!(2.5)),
                    description: None,
                },
                SavedArg {
                    name: "ticket".into(),
                    ty: SavedArgType::Int,
                    required: true,
                    default: None,
                    description: None,
                },
                SavedArg {
                    name: "opts".into(),
                    ty: SavedArgType::Json,
                    required: false,
                    default: Some(json!({"a": [1, null, "x}"]})),
                    description: Some("a, b: {c}".into()),
                },
            ],
        }
    }

    #[test]
    fn render_round_trips_awkward_values() {
        let meta = sample_meta();
        let file = render(&meta, "def main(args):\n    return 1\n").unwrap();
        let back = parse("pr-review.star", Some("pr-review"), &file).unwrap();
        assert_eq!(back.meta, meta);
        assert!(file.ends_with("def main(args):\n    return 1\n"));
        assert!(file.contains("\n\ndef main"), "a blank line ends the block");
        // Byte-stable: saving the saved file again changes nothing.
        let again = render(&back.meta, &file).unwrap();
        assert_eq!(again, file);
    }

    #[test]
    fn render_replaces_an_existing_frontmatter() {
        let mut meta = sample_meta();
        meta.description = "new".into();
        let file = render(&meta, GOOD).unwrap();
        assert_eq!(file.matches(MARK).count(), 1);
        assert!(file.contains("# description: new"));
        assert!(!file.contains("three reviewers"));
        assert!(file.contains("def main(args):"));
    }

    #[test]
    fn render_refuses_what_parse_would_refuse() {
        let mut m = sample_meta();
        m.name = Some("../x".into());
        assert!(render(&m, "x = 1").is_err());
        let mut m = sample_meta();
        m.description = "two\nlines".into();
        assert!(render(&m, "x = 1").is_err());
        let mut m = sample_meta();
        m.description = "  ".into();
        assert!(render(&m, "x = 1").is_err());
        let mut m = sample_meta();
        m.args[2].default = Some(json!(1));
        assert!(render(&m, "x = 1").is_err());
        let mut m = sample_meta();
        m.args[1].default = Some(json!("x"));
        assert!(render(&m, "x = 1").is_err());
        let mut m = sample_meta();
        m.args.push(m.args[0].clone());
        assert!(render(&m, "x = 1").is_err());
        let mut m = sample_meta();
        m.name = None;
        assert!(render(&m, "x = 1").is_err());
        let mut m = sample_meta();
        m.args[0].name = "bad-name".into();
        assert!(render(&m, "x = 1").is_err());
        let huge = "x = 1\n".repeat(60_000);
        assert!(render(&sample_meta(), &huge).is_err());
    }

    #[test]
    fn builtins_are_valid_saved_workflows() {
        for b in builtins() {
            let file = parse(&format!("{}.star", b.name), Some(b.name), b.source)
                .unwrap_or_else(|d| panic!("{}: {}", b.name, crate::diagnostic::render(&d)));
            assert_eq!(file.meta.name.as_deref(), Some(b.name));
            assert!(!file.meta.description.is_empty());
            crate::analyze(&format!("{}.star", b.name), b.source)
                .unwrap_or_else(|d| panic!("{}: {}", b.name, crate::diagnostic::render(&d)));
        }
    }
}
