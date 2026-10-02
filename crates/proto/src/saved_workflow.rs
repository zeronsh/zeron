//! Saved (reusable) workflows: the wire types and the argument contract.
//!
//! A saved workflow is a Starlark file with a frontmatter comment block
//! (`docs/workflows.md` → "Saved workflows"). The *file format* — parsing and
//! writing the frontmatter — lives in `zeron-workflow` (engine side only).
//! What every client needs lives here, with no interpreter: the scopes, the
//! declared arguments, the list/detail shapes, and the one place the argument
//! rules are written down ([`validate_args`], [`SavedArg::parse_text`]) so the
//! engine, the desktop launcher, the `/workflow` slash command and mobile all
//! accept and reject the same values.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::WorkflowGraph;

/// Longest saved-workflow name (it is the file stem).
pub const SAVED_MAX_NAME_CHARS: usize = 64;
/// One-line description / when-to-use in the frontmatter.
pub const SAVED_MAX_DESCRIPTION_CHARS: usize = 300;
pub const SAVED_MAX_WHEN_TO_USE_CHARS: usize = 600;
pub const SAVED_MAX_ARGS: usize = 32;
pub const SAVED_MAX_ARG_NAME_CHARS: usize = 32;
pub const SAVED_MAX_ARG_DESCRIPTION_CHARS: usize = 200;
/// A string argument (or default).
pub const SAVED_MAX_ARG_STRING_BYTES: usize = 16 * 1024;
/// A json argument (or default), compact-encoded.
pub const SAVED_MAX_ARG_JSON_BYTES: usize = 16 * 1024;
/// All arguments of one run, compact-encoded.
pub const SAVED_MAX_ARGS_BYTES: usize = 64 * 1024;

/// Where a saved workflow lives. A project's file shadows a global one of the
/// same name, which shadows a built-in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SavedScope {
    /// `<project>/.zeron/workflows/<name>.star`, only in that project.
    Project,
    /// `~/.zeron/workflows/<name>.star` on the device, every project.
    Global,
    /// Shipped with the app; read-only.
    Builtin,
}

impl SavedScope {
    pub fn as_str(self) -> &'static str {
        match self {
            SavedScope::Project => "project",
            SavedScope::Global => "global",
            SavedScope::Builtin => "builtin",
        }
    }

    /// Settings group title.
    pub fn label(self) -> &'static str {
        match self {
            SavedScope::Project => "Project",
            SavedScope::Global => "Global",
            SavedScope::Builtin => "Built-in",
        }
    }

    /// Resolution order: lower wins (project, then global, then built-in).
    pub fn precedence(self) -> u8 {
        match self {
            SavedScope::Project => 0,
            SavedScope::Global => 1,
            SavedScope::Builtin => 2,
        }
    }

    /// Scopes a user or agent can write to.
    pub fn writable(self) -> bool {
        self != SavedScope::Builtin
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "project" => Some(SavedScope::Project),
            "global" => Some(SavedScope::Global),
            "builtin" | "built-in" => Some(SavedScope::Builtin),
            _ => None,
        }
    }
}

impl std::fmt::Display for SavedScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The type of one declared argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SavedArgType {
    String,
    /// A whole number (|n| ≤ 2^53, so every client round-trips it exactly).
    Int,
    Number,
    Bool,
    /// Any JSON value, unchecked.
    Json,
}

impl SavedArgType {
    pub const ALL: [SavedArgType; 5] = [
        SavedArgType::String,
        SavedArgType::Int,
        SavedArgType::Number,
        SavedArgType::Bool,
        SavedArgType::Json,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            SavedArgType::String => "string",
            SavedArgType::Int => "int",
            SavedArgType::Number => "number",
            SavedArgType::Bool => "bool",
            SavedArgType::Json => "json",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.as_str() == text)
    }
}

/// Largest integer every JSON consumer (f64) holds exactly.
const MAX_SAFE_INT: i64 = 1 << 53;

/// One declared argument of a saved workflow.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedArg {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: SavedArgType,
    /// The run cannot start without it. Mutually exclusive with `default`.
    #[serde(default)]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// `a valid argument name`: an identifier, so scripts can write `args.get("x")`
/// and slash commands `x=1`.
pub fn valid_arg_name(name: &str) -> Result<(), String> {
    let mut chars = name.chars();
    let ok = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !ok || name.len() > SAVED_MAX_ARG_NAME_CHARS {
        return Err(format!(
            "argument name {name:?} must be letters, digits and `_` (not starting with a digit), at most {SAVED_MAX_ARG_NAME_CHARS} characters"
        ));
    }
    Ok(())
}

fn describe(value: &Value) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Bool(b) => format!("bool {b}"),
        Value::Number(n) => format!("number {n}"),
        Value::String(s) => {
            let shown: String = s.chars().take(24).collect();
            if s.chars().count() > 24 {
                format!("string \"{shown}…\"")
            } else {
                format!("string \"{shown}\"")
            }
        }
        Value::Array(_) => "an array".into(),
        Value::Object(_) => "an object".into(),
    }
}

impl SavedArg {
    /// Does `value` have this argument's type? (`json` accepts anything,
    /// `null` included.)
    pub fn check(&self, value: &Value) -> Result<(), String> {
        let bad = |expected: &str| {
            Err(format!(
                "argument '{}': expected {expected}, got {}",
                self.name,
                describe(value)
            ))
        };
        match self.ty {
            SavedArgType::String => match value {
                Value::String(s) if s.len() > SAVED_MAX_ARG_STRING_BYTES => Err(format!(
                    "argument '{}': text is longer than {} KB",
                    self.name,
                    SAVED_MAX_ARG_STRING_BYTES / 1024
                )),
                Value::String(_) => Ok(()),
                _ => bad("a string"),
            },
            SavedArgType::Int => match value.as_i64() {
                Some(n) if n.abs() <= MAX_SAFE_INT => Ok(()),
                Some(_) => Err(format!(
                    "argument '{}': integer is outside ±2^53",
                    self.name
                )),
                None => bad("an int (a whole number)"),
            },
            SavedArgType::Number => match value.as_f64() {
                Some(n) if n.is_finite() && value.is_number() => Ok(()),
                _ => bad("a number"),
            },
            SavedArgType::Bool => match value {
                Value::Bool(_) => Ok(()),
                _ => bad("a bool (true or false)"),
            },
            SavedArgType::Json => {
                let size = serde_json::to_string(value).map_or(usize::MAX, |s| s.len());
                if size > SAVED_MAX_ARG_JSON_BYTES {
                    Err(format!(
                        "argument '{}': json is larger than {} KB",
                        self.name,
                        SAVED_MAX_ARG_JSON_BYTES / 1024
                    ))
                } else {
                    Ok(())
                }
            }
        }
    }

    /// Text typed by a person (a form field, `name=value` on the slash
    /// command) → a value of this type. Strings are taken verbatim; the other
    /// types are lenient about case and spacing, never about meaning.
    pub fn parse_text(&self, text: &str) -> Result<Value, String> {
        let value = match self.ty {
            SavedArgType::String => Value::String(text.to_owned()),
            SavedArgType::Int => {
                let n: i64 = text.trim().parse().map_err(|_| {
                    format!(
                        "argument '{}': {:?} is not a whole number",
                        self.name,
                        text.trim()
                    )
                })?;
                Value::from(n)
            }
            SavedArgType::Number => {
                let n: f64 = text
                    .trim()
                    .parse()
                    .ok()
                    .filter(|n: &f64| n.is_finite())
                    .ok_or_else(|| {
                        format!(
                            "argument '{}': {:?} is not a number",
                            self.name,
                            text.trim()
                        )
                    })?;
                // Keep `3` an integer-looking number, like the frontmatter does.
                if n.fract() == 0.0 && n.abs() < MAX_SAFE_INT as f64 {
                    Value::from(n as i64)
                } else {
                    Value::from(n)
                }
            }
            SavedArgType::Bool => match text.trim().to_ascii_lowercase().as_str() {
                "true" | "yes" | "on" | "1" => Value::Bool(true),
                "false" | "no" | "off" | "0" => Value::Bool(false),
                other => {
                    return Err(format!(
                        "argument '{}': {other:?} is not true or false",
                        self.name
                    ));
                }
            },
            SavedArgType::Json => serde_json::from_str(text.trim())
                .map_err(|e| format!("argument '{}': not valid JSON ({e})", self.name))?,
        };
        self.check(&value)?;
        Ok(value)
    }

    /// The value as the form shows it: strings verbatim, the rest compact.
    pub fn display_value(value: &Value) -> String {
        match value {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        }
    }
}

/// Validate the arguments a caller passed against a workflow's declaration and
/// return the complete set the script will receive (defaults filled, absent
/// optionals left out).
///
/// ALL problems are collected, not just the first: a model (or a person) that
/// is told "missing `pr`, and `prNumber` is not an argument" fixes both at
/// once. A workflow without declarations rejects any argument — it cannot read
/// it, and dropping it silently would let the caller believe it took effect.
pub fn validate_args(specs: &[SavedArg], provided: &Value) -> Result<Value, Vec<String>> {
    let given = match provided {
        Value::Null => Map::new(),
        Value::Object(m) => m.clone(),
        _ => return Err(vec!["`args` must be an object".into()]),
    };
    let mut errors = Vec::new();
    let declared = || {
        specs
            .iter()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    for key in given.keys() {
        if specs.iter().any(|s| &s.name == key) {
            continue;
        }
        errors.push(if specs.is_empty() {
            format!("unknown argument '{key}': this workflow declares no arguments")
        } else {
            format!("unknown argument '{key}' (declared: {})", declared())
        });
    }
    let mut out = Map::new();
    for spec in specs {
        // `null` means "not given" for every type but json, where it is data.
        let supplied = given
            .get(&spec.name)
            .filter(|v| !(v.is_null() && spec.ty != SavedArgType::Json));
        match supplied {
            Some(value) => match spec.check(value) {
                Ok(()) => {
                    out.insert(spec.name.clone(), value.clone());
                }
                Err(e) => errors.push(e),
            },
            None => match &spec.default {
                Some(default) => {
                    out.insert(spec.name.clone(), default.clone());
                }
                None if spec.required => {
                    errors.push(format!("missing required argument '{}'", spec.name));
                }
                None => {}
            },
        }
    }
    let size = serde_json::to_string(&out).map_or(usize::MAX, |s| s.len());
    if errors.is_empty() && size > SAVED_MAX_ARGS_BYTES {
        errors.push(format!(
            "the arguments are larger than {} KB",
            SAVED_MAX_ARGS_BYTES / 1024
        ));
    }
    if errors.is_empty() {
        Ok(Value::Object(out))
    } else {
        Err(errors)
    }
}

const WINDOWS_RESERVED: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// A saved workflow's name is its file stem, so it must be a plain slug:
/// lowercase letters, digits, `-` and `_`, starting with a letter or digit.
/// That alphabet **is** the path-traversal defence — no separators, no dots,
/// no spaces — and it stays valid on every filesystem (device names such as
/// `con` are refused for Windows' sake).
pub fn valid_saved_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("the workflow name is empty".into());
    }
    if name.chars().count() > SAVED_MAX_NAME_CHARS {
        return Err(format!(
            "the workflow name is longer than {SAVED_MAX_NAME_CHARS} characters"
        ));
    }
    let mut chars = name.chars();
    let first_ok = chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    if !first_ok
        || !chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
    {
        return Err(format!(
            "workflow name {name:?} must be lowercase letters, digits, `-` and `_`, starting with a letter or digit (like `pr-review`)"
        ));
    }
    if WINDOWS_RESERVED.contains(&name) {
        return Err(format!("{name:?} is a reserved device name on Windows"));
    }
    Ok(())
}

/// Turn a free-form title into a candidate name (`"PR review!"` → `pr-review`).
/// May return an empty string; the result still needs [`valid_saved_name`].
pub fn slug_for_name(title: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in title.trim().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            dash = false;
        } else if !out.is_empty() && !dash {
            out.push('-');
            dash = true;
        }
    }
    let out = out.trim_matches('-').to_owned();
    out.chars()
        .take(SAVED_MAX_NAME_CHARS)
        .collect::<String>()
        .trim_end_matches('-')
        .to_owned()
}

// ── list / detail ─────────────────────────────────────────────────────────

/// One saved workflow as a list shows it (no script).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedWorkflowSummary {
    pub name: String,
    pub scope: SavedScope,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when_to_use: Option<String>,
    #[serde(default)]
    pub args: Vec<SavedArg>,
    /// Absolute path of the file on the host (`None` for built-ins).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The project this file belongs to (project scope).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_root: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space_id: Option<String>,
    /// File modification time (ms since the epoch).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified_at: Option<i64>,
    /// A workflow of this name in a higher scope wins; this one is hidden
    /// in the context the list was asked for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shadowed_by: Option<SavedScope>,
    /// Lower scopes this one hides.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shadows: Vec<SavedScope>,
}

impl SavedWorkflowSummary {
    /// `base, deep?, rounds=3`: required bare, optional with `?`, defaulted
    /// with their default.
    pub fn args_line(&self) -> String {
        self.args
            .iter()
            .map(|a| match (&a.default, a.required) {
                (Some(d), _) => format!("{}={}", a.name, SavedArg::display_value(d)),
                (None, true) => a.name.clone(),
                (None, false) => format!("{}?", a.name),
            })
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// A file in a workflows folder that cannot be used (the list never fails on
/// one bad file; it names it).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedWorkflowInvalid {
    pub path: String,
    pub scope: SavedScope,
    /// `path:line:col message` lines (or one sentence).
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_root: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SavedWorkflowList {
    #[serde(default)]
    pub workflows: Vec<SavedWorkflowSummary>,
    #[serde(default)]
    pub invalid: Vec<SavedWorkflowInvalid>,
    /// Where this device keeps its global workflows (shown in Settings).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub global_dir: Option<String>,
}

/// One workflow with its script and the analysis the approval would show.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedWorkflowDetail {
    #[serde(flatten)]
    pub summary: SavedWorkflowSummary,
    /// The whole file, frontmatter included.
    pub script: String,
    /// Phases, agents and literal commands (`None` when analysis failed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graph: Option<WorkflowGraph>,
    /// Analysis problems (`path:line:col message`), when the script is broken.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<String>,
}

/// Which saved workflow a run came from (approval payload; the run header
/// carries the same two fields flat).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedRunRef {
    pub name: String,
    pub scope: SavedScope,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn arg(name: &str, ty: SavedArgType) -> SavedArg {
        SavedArg {
            name: name.into(),
            ty,
            required: false,
            default: None,
            description: None,
        }
    }

    #[test]
    fn names_are_slugs_and_nothing_else() {
        for ok in [
            "pr-review",
            "a",
            "fix_until_green",
            "v2",
            "0day",
            &"a".repeat(64),
        ] {
            assert!(valid_saved_name(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "PR-review",
            "../etc/passwd",
            "a/b",
            "a\\b",
            "..",
            ".hidden",
            "-lead",
            "_lead",
            "with space",
            "tab\t",
            "nul\0",
            "dot.star",
            "ünï",
            "con",
            "nul",
            "com1",
            &"a".repeat(65),
        ] {
            assert!(valid_saved_name(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn slug_for_name_is_a_best_effort_candidate() {
        assert_eq!(slug_for_name("PR review!"), "pr-review");
        assert_eq!(slug_for_name("  Fix until   green "), "fix-until-green");
        assert_eq!(slug_for_name("***"), "");
        assert!(valid_saved_name(&slug_for_name(&"x ".repeat(100))).is_ok());
    }

    #[test]
    fn arg_names_are_identifiers() {
        assert!(valid_arg_name("base").is_ok());
        assert!(valid_arg_name("_x1").is_ok());
        for bad in ["", "1x", "a-b", "a b", "a.b", &"a".repeat(33)] {
            assert!(valid_arg_name(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn typed_checks_accept_and_reject() {
        let int = arg("n", SavedArgType::Int);
        assert!(int.check(&json!(3)).is_ok());
        assert!(int.check(&json!(-3)).is_ok());
        assert!(int.check(&json!(3.5)).is_err());
        assert!(
            int.check(&json!(3.0)).is_err(),
            "3.0 is a float on the wire"
        );
        assert!(int.check(&json!("3")).is_err());
        assert!(int.check(&json!(1u64 << 60)).is_err());
        let num = arg("x", SavedArgType::Number);
        assert!(num.check(&json!(3)).is_ok());
        assert!(num.check(&json!(3.5)).is_ok());
        assert!(num.check(&json!("3.5")).is_err());
        let b = arg("b", SavedArgType::Bool);
        assert!(b.check(&json!(true)).is_ok());
        assert!(b.check(&json!(1)).is_err());
        let j = arg("j", SavedArgType::Json);
        for v in [json!(null), json!([1, {"a": 2}]), json!("s")] {
            assert!(j.check(&v).is_ok());
        }
        assert!(j.check(&json!("x".repeat(20_000))).is_err());
        let s = arg("s", SavedArgType::String);
        assert!(s.check(&json!("ok")).is_ok());
        assert!(s.check(&json!(null)).is_err());
        assert!(
            s.check(&json!("x".repeat(SAVED_MAX_ARG_STRING_BYTES + 1)))
                .is_err()
        );
    }

    #[test]
    fn text_parsing_is_lenient_about_form_and_strict_about_meaning() {
        let int = arg("n", SavedArgType::Int);
        assert_eq!(int.parse_text(" 42 ").unwrap(), json!(42));
        assert!(int.parse_text("4.2").is_err());
        assert!(int.parse_text("").is_err());
        let num = arg("x", SavedArgType::Number);
        assert_eq!(num.parse_text("2").unwrap(), json!(2));
        assert_eq!(num.parse_text("2.5").unwrap(), json!(2.5));
        assert!(num.parse_text("NaN").is_err());
        assert!(num.parse_text("inf").is_err());
        let b = arg("b", SavedArgType::Bool);
        for t in ["true", "Yes", "ON", "1"] {
            assert_eq!(b.parse_text(t).unwrap(), json!(true), "{t}");
        }
        for t in ["false", "no", "Off", "0"] {
            assert_eq!(b.parse_text(t).unwrap(), json!(false), "{t}");
        }
        assert!(b.parse_text("maybe").is_err());
        let j = arg("j", SavedArgType::Json);
        assert_eq!(j.parse_text(r#" {"a": [1]} "#).unwrap(), json!({"a": [1]}));
        assert!(j.parse_text("{oops").is_err());
        let s = arg("s", SavedArgType::String);
        assert_eq!(
            s.parse_text("  keep  spaces ").unwrap(),
            json!("  keep  spaces ")
        );
    }

    fn pr_review() -> Vec<SavedArg> {
        vec![
            SavedArg {
                default: Some(json!("main")),
                ..arg("base", SavedArgType::String)
            },
            SavedArg {
                default: Some(json!(false)),
                ..arg("deep", SavedArgType::Bool)
            },
            SavedArg {
                required: true,
                ..arg("ticket", SavedArgType::Int)
            },
            arg("notes", SavedArgType::Json),
        ]
    }

    #[test]
    fn validation_fills_defaults_and_omits_absent_optionals() {
        let out = validate_args(&pr_review(), &json!({"ticket": 7})).unwrap();
        assert_eq!(out, json!({"base": "main", "deep": false, "ticket": 7}));
        let out = validate_args(
            &pr_review(),
            &json!({"ticket": 7, "base": "dev", "deep": true, "notes": null}),
        )
        .unwrap();
        assert_eq!(
            out,
            json!({"base": "dev", "deep": true, "ticket": 7, "notes": null}),
            "null is data for json"
        );
    }

    #[test]
    fn null_means_absent_for_typed_arguments() {
        let out = validate_args(&pr_review(), &json!({"ticket": 7, "base": null})).unwrap();
        assert_eq!(out["base"], "main");
        let err = validate_args(&pr_review(), &json!({"ticket": null})).unwrap_err();
        assert_eq!(err, ["missing required argument 'ticket'"]);
    }

    #[test]
    fn validation_collects_every_problem() {
        let err = validate_args(
            &pr_review(),
            &json!({"base": 3, "deep": "yes", "extra": 1, "other": 2}),
        )
        .unwrap_err();
        assert_eq!(err.len(), 5, "{err:?}");
        assert!(err.iter().any(|e| e.contains("unknown argument 'extra'")
            && e.contains("declared: base, deep, ticket, notes")));
        assert!(err.iter().any(|e| e.contains("unknown argument 'other'")));
        assert!(
            err.iter()
                .any(|e| e.contains("argument 'base': expected a string, got number 3"))
        );
        assert!(
            err.iter()
                .any(|e| e.contains("argument 'deep': expected a bool"))
        );
        assert!(
            err.iter()
                .any(|e| e == "missing required argument 'ticket'")
        );
    }

    #[test]
    fn a_workflow_without_arguments_rejects_any() {
        let err = validate_args(&[], &json!({"x": 1})).unwrap_err();
        assert_eq!(
            err,
            ["unknown argument 'x': this workflow declares no arguments"]
        );
        assert_eq!(validate_args(&[], &json!({})).unwrap(), json!({}));
        assert_eq!(validate_args(&[], &Value::Null).unwrap(), json!({}));
    }

    #[test]
    fn args_must_be_an_object_and_stay_small() {
        assert!(validate_args(&[], &json!([1])).is_err());
        let specs: Vec<_> = (0..5)
            .map(|i| arg(&format!("a{i}"), SavedArgType::Json))
            .collect();
        let big = json!("x".repeat(SAVED_MAX_ARG_JSON_BYTES - 10));
        let provided = json!({"a0": big, "a1": big, "a2": big, "a3": big, "a4": big});
        let err = validate_args(&specs, &provided).unwrap_err();
        assert!(err[0].contains("larger than 64 KB"), "{err:?}");
    }

    #[test]
    fn summaries_describe_their_arguments() {
        let s = SavedWorkflowSummary {
            name: "pr-review".into(),
            scope: SavedScope::Global,
            description: "d".into(),
            when_to_use: None,
            args: pr_review(),
            path: None,
            project_root: None,
            space_id: None,
            modified_at: None,
            shadowed_by: None,
            shadows: vec![],
        };
        assert_eq!(s.args_line(), "base=main, deep=false, ticket, notes?");
    }

    #[test]
    fn wire_names_are_stable() {
        assert_eq!(
            serde_json::to_value(SavedScope::Builtin).unwrap(),
            "builtin"
        );
        assert_eq!(serde_json::to_value(SavedArgType::Bool).unwrap(), "bool");
        let a = serde_json::to_value(&pr_review()[0]).unwrap();
        assert_eq!(
            a,
            json!({"name": "base", "type": "string", "required": false, "default": "main"})
        );
        // An older/foreign producer may omit every optional field.
        let back: SavedArg = serde_json::from_value(json!({"name": "x", "type": "json"})).unwrap();
        assert!(!back.required && back.default.is_none());
        assert_eq!(SavedScope::parse("built-in"), Some(SavedScope::Builtin));
        assert!(SavedScope::Project.precedence() < SavedScope::Global.precedence());
    }
}
