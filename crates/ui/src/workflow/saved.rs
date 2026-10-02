//! Saved workflows in the desktop UI: the pure half.
//!
//! Everything a person types or reads about a saved workflow is decided here,
//! without a window: the launcher's args form (parsing, defaults, per-field
//! errors), the `/workflow name key=value …` command line, name resolution,
//! the Settings list's grouping and notes, run-history rows, and the "Save as
//! workflow…" dialog's validation. The argument rules themselves are
//! `zeron_proto::saved_workflow` (shared with the engine, which re-checks).

use std::collections::BTreeMap;

use serde_json::{Map, Value};
use zeron_proto::{
    SavedArg, SavedArgType, SavedScope, SavedWorkflowInvalid, SavedWorkflowList,
    SavedWorkflowSummary, Space, WorkflowRunHeader, WorkflowStatus, slug_for_name,
    valid_saved_name, validate_args,
};

use super::model::{Tone, format_tokens};

// ── the launcher's args form ──────────────────────────────────────────────

/// One editable argument. Booleans are a switch, everything else is text.
#[derive(Debug, Clone, PartialEq)]
pub struct ArgField {
    pub spec: SavedArg,
    pub text: String,
    pub on: bool,
}

impl ArgField {
    pub fn is_toggle(&self) -> bool {
        self.spec.ty == SavedArgType::Bool
    }

    /// A json value is edited in a taller box.
    pub fn is_multiline(&self) -> bool {
        self.spec.ty == SavedArgType::Json
    }

    /// Shown in an empty box: what leaving it empty means.
    pub fn placeholder(&self) -> String {
        match (&self.spec.default, self.spec.required) {
            (Some(d), _) => format!("Default: {}", SavedArg::display_value(d)),
            (None, true) => "Required".into(),
            (None, false) => "Optional".into(),
        }
    }

    /// `string`, `int`, … the type chip.
    pub fn type_label(&self) -> &'static str {
        self.spec.ty.as_str()
    }
}

/// What is wrong with a form, per field and overall.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FormErrors {
    pub fields: BTreeMap<usize, String>,
    pub general: Vec<String>,
}

impl FormErrors {
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty() && self.general.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ArgsForm {
    pub fields: Vec<ArgField>,
}

fn initial_text(spec: &SavedArg, value: Option<&Value>) -> String {
    match value {
        None => String::new(),
        Some(v) => match spec.ty {
            // Objects and lists read better laid out; scalars stay one line.
            SavedArgType::Json if v.is_object() || v.is_array() => {
                serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string())
            }
            _ => SavedArg::display_value(v),
        },
    }
}

impl ArgsForm {
    /// A form for `specs`. `given` (an object) overrides the defaults — the
    /// values `/workflow` already parsed, or the ones of the run being re-run.
    pub fn new(specs: &[SavedArg], given: &Value) -> Self {
        let given = given.as_object();
        let fields = specs
            .iter()
            .map(|spec| {
                let value = given
                    .and_then(|g| g.get(&spec.name))
                    .or(spec.default.as_ref());
                ArgField {
                    on: value.and_then(Value::as_bool).unwrap_or(false),
                    text: if spec.ty == SavedArgType::Bool {
                        String::new()
                    } else {
                        initial_text(spec, value)
                    },
                    spec: spec.clone(),
                }
            })
            .collect();
        Self { fields }
    }

    pub fn set_text(&mut self, index: usize, text: impl Into<String>) {
        if let Some(f) = self.fields.get_mut(index) {
            f.text = text.into();
        }
    }

    pub fn toggle(&mut self, index: usize) {
        if let Some(f) = self.fields.get_mut(index) {
            f.on = !f.on;
        }
    }

    /// The arguments to send, or every problem found. An empty box means "not
    /// given": the default applies (the engine fills it in); an empty box with
    /// no default is an error only when the argument is required.
    pub fn collect(&self) -> Result<Value, FormErrors> {
        let mut errors = FormErrors::default();
        let mut out = Map::new();
        for (ix, f) in self.fields.iter().enumerate() {
            if f.is_toggle() {
                out.insert(f.spec.name.clone(), Value::Bool(f.on));
                continue;
            }
            if f.text.trim().is_empty() {
                if f.spec.required && f.spec.default.is_none() {
                    errors.fields.insert(ix, "Required".into());
                }
                continue;
            }
            match f.spec.parse_text(&f.text) {
                Ok(v) => {
                    out.insert(f.spec.name.clone(), v);
                }
                Err(_) => {
                    errors.fields.insert(
                        ix,
                        match f.spec.ty {
                            SavedArgType::Int => "Enter a whole number".into(),
                            SavedArgType::Number => "Enter a number".into(),
                            SavedArgType::Json => json_problem(&f.text),
                            _ => "Not a valid value".into(),
                        },
                    );
                }
            }
        }
        if !errors.is_empty() {
            return Err(errors);
        }
        let specs: Vec<SavedArg> = self.fields.iter().map(|f| f.spec.clone()).collect();
        match validate_args(&specs, &Value::Object(out.clone())) {
            Ok(_) => Ok(Value::Object(out)),
            Err(messages) => {
                errors.general = messages;
                Err(errors)
            }
        }
    }
}

/// `Not valid JSON: expected value at line 2, column 5`.
fn json_problem(text: &str) -> String {
    match serde_json::from_str::<Value>(text.trim()) {
        Err(e) => format!("Not valid JSON: {e}"),
        // A valid value the argument rules still refuse (too large).
        Ok(_) => "Too large".into(),
    }
}

// ── `/workflow name key=value …` ──────────────────────────────────────────

/// What a typed `/workflow …` line asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowInput {
    /// Bare `/workflow`: say what is available.
    List,
    Run {
        name: String,
        /// `key=value` as typed; `None` is a bare `key` (a switch).
        assignments: Vec<(String, Option<String>)>,
    },
}

/// Parse a composer line. `None`: not a `/workflow` command (an ordinary
/// message). `Some(Err)`: a workflow command that cannot be understood.
///
/// Like `/goal`, only a line that *starts* with the command counts, and the
/// command must be the whole first word (`/workflows` is something else).
/// Values may be quoted (`'…'` or `"…"`, with `\"` and `\\` in the latter).
pub fn parse_workflow_input(text: &str) -> Option<Result<WorkflowInput, String>> {
    let rest = text.strip_prefix("/workflow")?;
    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let words = match split_words(rest) {
        Ok(w) => w,
        Err(e) => return Some(Err(e)),
    };
    let mut words = words.into_iter();
    let Some(name) = words.next() else {
        return Some(Ok(WorkflowInput::List));
    };
    if name.contains('=') {
        return Some(Err(
            "Start with the workflow's name: /workflow <name> key=value …".into(),
        ));
    }
    let assignments = words
        .map(|w| match w.split_once('=') {
            Some((k, v)) => (k.to_owned(), Some(v.to_owned())),
            None => (w, None),
        })
        .collect();
    Some(Ok(WorkflowInput::Run { name, assignments }))
}

/// Shell-ish words: whitespace separates, quotes group (also inside a word:
/// `opts='{"a": 1}'`).
fn split_words(text: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut started = false;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {
                if started {
                    words.push(std::mem::take(&mut cur));
                    started = false;
                }
            }
            '\'' => {
                started = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(c) => cur.push(c),
                        None => return Err("A quote is not closed.".into()),
                    }
                }
            }
            '"' => {
                started = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(e @ ('"' | '\\')) => cur.push(e),
                            Some(other) => {
                                cur.push('\\');
                                cur.push(other);
                            }
                            None => return Err("A quote is not closed.".into()),
                        },
                        Some(c) => cur.push(c),
                        None => return Err("A quote is not closed.".into()),
                    }
                }
            }
            c => {
                started = true;
                cur.push(c);
            }
        }
    }
    if started {
        words.push(cur);
    }
    Ok(words)
}

/// The saved workflow a typed name means: exact, else a unique prefix. Only
/// the ones that win in the chat's project are candidates.
pub fn pick_workflow<'a>(
    name: &str,
    list: &'a [SavedWorkflowSummary],
) -> Result<&'a SavedWorkflowSummary, String> {
    let live: Vec<&SavedWorkflowSummary> =
        list.iter().filter(|w| w.shadowed_by.is_none()).collect();
    if let Some(exact) = live.iter().find(|w| w.name == name) {
        return Ok(exact);
    }
    let prefixed: Vec<&&SavedWorkflowSummary> =
        live.iter().filter(|w| w.name.starts_with(name)).collect();
    match prefixed.as_slice() {
        [one] => Ok(**one),
        [] => Err(if live.is_empty() {
            format!("No saved workflow named {name:?}. Nothing is saved yet.")
        } else {
            format!(
                "No saved workflow named {name:?}. Saved: {}.",
                names_line(&live)
            )
        }),
        many => Err(format!(
            "{name:?} could be {}. Type more of the name.",
            many.iter()
                .map(|w| w.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

fn names_line(list: &[&SavedWorkflowSummary]) -> String {
    let mut names: Vec<&str> = list.iter().map(|w| w.name.as_str()).collect();
    let more = names.len().saturating_sub(8);
    names.truncate(8);
    let mut s = names.join(", ");
    if more > 0 {
        s.push_str(&format!(", +{more} more"));
    }
    s
}

/// The arguments a command line supplied, typed, and what is still missing.
#[derive(Debug, Clone, PartialEq)]
pub struct Bound {
    /// Typed values, only the ones given.
    pub args: Value,
    /// Required arguments with no value and no default.
    pub missing: Vec<String>,
}

/// Type the `key=value` words against `wf`'s declarations. Every problem is
/// reported at once.
pub fn bind_assignments(
    wf: &SavedWorkflowSummary,
    assignments: &[(String, Option<String>)],
) -> Result<Bound, Vec<String>> {
    let mut errors = Vec::new();
    let mut args = Map::new();
    for (key, raw) in assignments {
        let Some(spec) = wf.args.iter().find(|a| &a.name == key) else {
            errors.push(if wf.args.is_empty() {
                format!("{} takes no arguments (got {key:?}).", wf.name)
            } else {
                format!(
                    "{:?} is not an argument of {}. It takes: {}.",
                    key,
                    wf.name,
                    wf.args
                        .iter()
                        .map(|a| a.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            });
            continue;
        };
        if args.contains_key(key) {
            errors.push(format!("{key} is given twice."));
            continue;
        }
        let text = match (raw, spec.ty) {
            (Some(v), _) => v.as_str(),
            // A bare `deep` switches a bool on.
            (None, SavedArgType::Bool) => "true",
            (None, _) => {
                errors.push(format!("{key} needs a value: {key}=…"));
                continue;
            }
        };
        match spec.parse_text(text) {
            Ok(v) => {
                args.insert(key.clone(), v);
            }
            Err(e) => errors.push(e),
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    let missing = wf
        .args
        .iter()
        .filter(|a| a.required && a.default.is_none() && !args.contains_key(&a.name))
        .map(|a| a.name.clone())
        .collect();
    Ok(Bound {
        args: Value::Object(args),
        missing,
    })
}

/// One line for the composer's notice: what a bare `/workflow` says.
pub fn list_hint(list: &[SavedWorkflowSummary]) -> String {
    let live: Vec<&SavedWorkflowSummary> =
        list.iter().filter(|w| w.shadowed_by.is_none()).collect();
    if live.is_empty() {
        return "No saved workflows yet. Save one from a run's card, or ask the agent.".into();
    }
    format!(
        "Run one with /workflow <name> key=value …  Saved: {}.",
        names_line(&live)
    )
}

// ── the Settings list ─────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GroupKind {
    Global,
    Project { space_id: Option<String> },
    Builtin,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub summary: SavedWorkflowSummary,
    /// `base=main, deep=false, ticket`
    pub args_line: String,
    /// "Overrides the global workflow", "Hidden by api, web", …
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    pub kind: GroupKind,
    pub title: String,
    /// Where it lives: `~/.zeron/workflows`, a project path.
    pub subtitle: Option<String>,
    pub rows: Vec<Row>,
    /// Files in the folder that cannot be used, with the reason.
    pub invalid: Vec<SavedWorkflowInvalid>,
}

/// Identity of a row across refreshes.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WorkflowKey {
    pub scope: SavedScope,
    pub name: String,
    pub project_root: Option<String>,
}

impl WorkflowKey {
    pub fn of(w: &SavedWorkflowSummary) -> Self {
        Self {
            scope: w.scope,
            name: w.name.clone(),
            project_root: w.project_root.clone(),
        }
    }
}

fn project_title(root: &str, space_id: Option<&str>, spaces: &[Space]) -> String {
    spaces
        .iter()
        .find(|s| space_id.is_some_and(|id| s.id == id) || s.path == root)
        .map(|s| s.display_name().to_owned())
        .unwrap_or_else(|| {
            root.trim_end_matches(['/', '\\'])
                .rsplit(['/', '\\'])
                .next()
                .filter(|s| !s.is_empty())
                .unwrap_or(root)
                .to_owned()
        })
}

/// Global first, then each project that has something to show, then the
/// built-ins; each group sorted by name.
pub fn group(list: &SavedWorkflowList, spaces: &[Space]) -> Vec<Group> {
    let row = |w: &SavedWorkflowSummary, note: Option<String>| Row {
        args_line: w.args_line(),
        note,
        summary: w.clone(),
    };
    let overridden_in = |w: &SavedWorkflowSummary| -> Vec<String> {
        list.workflows
            .iter()
            .filter(|p| p.scope == SavedScope::Project && p.name == w.name)
            .map(|p| {
                project_title(
                    p.project_root.as_deref().unwrap_or_default(),
                    p.space_id.as_deref(),
                    spaces,
                )
            })
            .collect()
    };
    let mut groups = Vec::new();

    let mut global_rows: Vec<&SavedWorkflowSummary> = list
        .workflows
        .iter()
        .filter(|w| w.scope == SavedScope::Global)
        .collect();
    global_rows.sort_by(|a, b| a.name.cmp(&b.name));
    groups.push(Group {
        kind: GroupKind::Global,
        title: "Global".into(),
        subtitle: Some(
            list.global_dir
                .clone()
                .unwrap_or_else(|| "every project".to_owned()),
        ),
        rows: global_rows
            .into_iter()
            .map(|w| {
                let mut notes = Vec::new();
                if w.shadows.contains(&SavedScope::Builtin) {
                    notes.push("Overrides the built-in workflow".to_owned());
                }
                let hidden = overridden_in(w);
                if !hidden.is_empty() {
                    notes.push(format!("Hidden in {}", hidden.join(", ")));
                }
                row(w, (!notes.is_empty()).then(|| notes.join(" · ")))
            })
            .collect(),
        invalid: list
            .invalid
            .iter()
            .filter(|i| i.scope == SavedScope::Global)
            .cloned()
            .collect(),
    });

    // Projects, keyed by root, ordered by title.
    let mut roots: Vec<(String, Option<String>)> = Vec::new();
    for (root, space) in list
        .workflows
        .iter()
        .filter(|w| w.scope == SavedScope::Project)
        .map(|w| (w.project_root.clone(), w.space_id.clone()))
        .chain(
            list.invalid
                .iter()
                .filter(|i| i.scope == SavedScope::Project)
                .map(|i| (i.project_root.clone(), i.space_id.clone())),
        )
    {
        let Some(root) = root else { continue };
        if !roots.iter().any(|(r, _)| *r == root) {
            roots.push((root, space));
        }
    }
    let mut projects: Vec<Group> = roots
        .into_iter()
        .map(|(root, space)| {
            let mut rows: Vec<&SavedWorkflowSummary> = list
                .workflows
                .iter()
                .filter(|w| {
                    w.scope == SavedScope::Project && w.project_root.as_deref() == Some(&root)
                })
                .collect();
            rows.sort_by(|a, b| a.name.cmp(&b.name));
            Group {
                kind: GroupKind::Project {
                    space_id: space.clone(),
                },
                title: project_title(&root, space.as_deref(), spaces),
                subtitle: Some(root.clone()),
                rows: rows
                    .into_iter()
                    .map(|w| {
                        let note = match w.shadows.as_slice() {
                            [] => None,
                            [SavedScope::Builtin] => Some("Overrides the built-in workflow".into()),
                            [SavedScope::Global] => Some("Overrides the global workflow".into()),
                            _ => Some("Overrides the global and built-in workflows".into()),
                        };
                        row(w, note)
                    })
                    .collect(),
                invalid: list
                    .invalid
                    .iter()
                    .filter(|i| {
                        i.scope == SavedScope::Project && i.project_root.as_deref() == Some(&root)
                    })
                    .cloned()
                    .collect(),
            }
        })
        .collect();
    projects.sort_by_key(|g| g.title.to_lowercase());
    groups.extend(projects);

    let mut built: Vec<&SavedWorkflowSummary> = list
        .workflows
        .iter()
        .filter(|w| w.scope == SavedScope::Builtin)
        .collect();
    built.sort_by(|a, b| a.name.cmp(&b.name));
    if !built.is_empty() {
        groups.push(Group {
            kind: GroupKind::Builtin,
            title: "Built-in".into(),
            subtitle: Some("ships with Zeron, read-only".into()),
            rows: built
                .into_iter()
                .map(|w| {
                    let note = overridden_by_global(w, list);
                    row(w, note)
                })
                .collect(),
            invalid: Vec::new(),
        });
    }
    groups
}

fn overridden_by_global(w: &SavedWorkflowSummary, list: &SavedWorkflowList) -> Option<String> {
    list.workflows
        .iter()
        .any(|o| o.scope == SavedScope::Global && o.name == w.name)
        .then(|| "Overridden by a global workflow of the same name".to_owned())
}

/// The file name of an unusable file, for the list.
pub fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

// ── run history ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct RunRow {
    pub run_id: String,
    pub chat_id: String,
    pub status: &'static str,
    pub tone: Tone,
    /// `3m ago`, `just now`
    pub when: String,
    /// `3m 12s · 48.2k tokens · 12 steps`
    pub detail: String,
    pub resumed: bool,
}

pub fn status_word(h: &WorkflowRunHeader) -> (&'static str, Tone) {
    let tone = super::model::status_tone(h);
    let word = match h.status {
        WorkflowStatus::Pending => "Awaiting approval",
        WorkflowStatus::Running => "Running",
        WorkflowStatus::Completed => "Completed",
        WorkflowStatus::Errored => "Failed",
        WorkflowStatus::Stopped => match h.stop_reason {
            Some(zeron_proto::WorkflowStopReason::Denied) => "Denied",
            _ => "Stopped",
        },
    };
    (word, tone)
}

pub fn run_rows(headers: &[WorkflowRunHeader], now: chrono::DateTime<chrono::Utc>) -> Vec<RunRow> {
    headers
        .iter()
        .map(|h| {
            let (status, tone) = status_word(h);
            let at = chrono::DateTime::from_timestamp_millis(h.created_at).unwrap_or(now);
            RunRow {
                run_id: h.run_id.clone(),
                chat_id: h.chat_id.clone(),
                status,
                tone,
                when: match crate::state::format_time_ago(at, now).as_str() {
                    "now" => "just now".to_owned(),
                    ago => format!("{ago} ago"),
                },
                detail: super::model::meta_line(h),
                resumed: h.resumed_from.is_some(),
            }
        })
        .collect()
}

/// `1.2k` style token count (re-exported for the pane header).
pub fn tokens(n: u64) -> String {
    format_tokens(n)
}

// ── "Save as workflow…" ───────────────────────────────────────────────────

/// Argument declarations inferred from the values a run was started with, so a
/// saved copy of an ad-hoc run keeps them as defaults.
pub fn infer_args(args: &Value) -> Vec<SavedArg> {
    let Some(map) = args.as_object() else {
        return Vec::new();
    };
    map.iter()
        .filter(|(name, _)| zeron_proto::valid_arg_name(name).is_ok())
        .map(|(name, value)| {
            let ty = match value {
                Value::String(_) => SavedArgType::String,
                Value::Bool(_) => SavedArgType::Bool,
                Value::Number(n) if n.as_i64().is_some() => SavedArgType::Int,
                Value::Number(_) => SavedArgType::Number,
                _ => SavedArgType::Json,
            };
            let arg = SavedArg {
                name: name.clone(),
                ty,
                required: false,
                default: Some(value.clone()),
                description: None,
            };
            // A default the rules refuse (too large) is dropped, not saved broken.
            if arg.check(value).is_ok() {
                arg
            } else {
                SavedArg {
                    default: None,
                    ..arg
                }
            }
        })
        .collect()
}

/// The dialog's fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveForm {
    pub name: String,
    pub description: String,
    pub when_to_use: String,
    pub scope: SavedScope,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SaveErrors {
    pub name: Option<String>,
    pub description: Option<String>,
    pub when_to_use: Option<String>,
    pub scope: Option<String>,
}

impl SaveErrors {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

impl SaveForm {
    /// Start from the run's title: `PR review!` → `pr-review`.
    pub fn for_run(run_name: &str, has_project: bool) -> Self {
        let slug = slug_for_name(run_name);
        Self {
            name: if valid_saved_name(&slug).is_ok() {
                slug
            } else {
                String::new()
            },
            description: String::new(),
            when_to_use: String::new(),
            scope: if has_project {
                SavedScope::Project
            } else {
                SavedScope::Global
            },
        }
    }

    pub fn validate(&self, has_project: bool) -> Result<(), SaveErrors> {
        let mut e = SaveErrors::default();
        if let Err(msg) = valid_saved_name(self.name.trim()) {
            e.name = Some(if self.name.trim().is_empty() {
                "Give it a name, like pr-review".to_owned()
            } else {
                short_name_problem(&msg)
            });
        }
        let description = self.description.trim();
        if description.is_empty() {
            e.description = Some("Say in a line what it does".into());
        } else if description.chars().count() > zeron_proto::SAVED_MAX_DESCRIPTION_CHARS {
            e.description = Some(format!(
                "At most {} characters",
                zeron_proto::SAVED_MAX_DESCRIPTION_CHARS
            ));
        } else if description.contains('\n') {
            e.description = Some("One line, please".into());
        }
        if self.when_to_use.chars().count() > zeron_proto::SAVED_MAX_WHEN_TO_USE_CHARS {
            e.when_to_use = Some(format!(
                "At most {} characters",
                zeron_proto::SAVED_MAX_WHEN_TO_USE_CHARS
            ));
        } else if self.when_to_use.contains('\n') {
            e.when_to_use = Some("One line, please".into());
        }
        if self.scope == SavedScope::Project && !has_project {
            e.scope = Some("This chat has no project folder".into());
        }
        if e.is_empty() { Ok(()) } else { Err(e) }
    }
}

fn short_name_problem(msg: &str) -> String {
    if msg.contains("reserved") {
        "That name is reserved".into()
    } else if msg.contains("longer than") {
        "Too long (64 characters at most)".into()
    } else {
        "Lowercase letters, digits, - and _".into()
    }
}

/// How the scope reads in the UI.
pub fn scope_label(scope: SavedScope) -> &'static str {
    scope.label()
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

    fn specs() -> Vec<SavedArg> {
        vec![
            SavedArg {
                default: Some(json!("main")),
                description: Some("Branch".into()),
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
            arg("ratio", SavedArgType::Number),
            SavedArg {
                default: Some(json!({"a": [1, 2]})),
                ..arg("opts", SavedArgType::Json)
            },
        ]
    }

    fn summary(name: &str, scope: SavedScope, args: Vec<SavedArg>) -> SavedWorkflowSummary {
        SavedWorkflowSummary {
            name: name.into(),
            scope,
            description: format!("{name} does it"),
            when_to_use: None,
            args,
            path: Some(format!("/x/{name}.star")),
            project_root: None,
            space_id: None,
            modified_at: None,
            shadowed_by: None,
            shadows: vec![],
        }
    }

    // ── form ──

    #[test]
    fn a_form_starts_from_defaults_and_marks_what_is_required() {
        let form = ArgsForm::new(&specs(), &Value::Null);
        assert_eq!(form.fields[0].text, "main");
        assert!(!form.fields[1].on && form.fields[1].is_toggle());
        assert_eq!(form.fields[2].text, "");
        assert_eq!(form.fields[2].placeholder(), "Required");
        assert_eq!(form.fields[3].placeholder(), "Optional");
        assert_eq!(form.fields[0].placeholder(), "Default: main");
        assert!(form.fields[4].is_multiline());
        assert!(
            form.fields[4].text.contains("\"a\""),
            "pretty json: {}",
            form.fields[4].text
        );
        assert_eq!(form.fields[2].type_label(), "int");
    }

    #[test]
    fn given_values_beat_defaults() {
        let form = ArgsForm::new(&specs(), &json!({"base": "dev", "deep": true, "ticket": 7}));
        assert_eq!(form.fields[0].text, "dev");
        assert!(form.fields[1].on);
        assert_eq!(form.fields[2].text, "7");
    }

    #[test]
    fn collect_types_every_field_and_omits_empty_optionals() {
        let mut form = ArgsForm::new(&specs(), &Value::Null);
        form.set_text(2, " 42 ");
        form.toggle(1);
        form.set_text(3, "");
        let out = form.collect().unwrap();
        assert_eq!(
            out,
            json!({"base": "main", "deep": true, "ticket": 42, "opts": {"a": [1, 2]}})
        );
        // Clearing a defaulted box leaves it out: the default applies.
        form.set_text(0, "");
        assert!(form.collect().unwrap().get("base").is_none());
    }

    #[test]
    fn collect_reports_every_problem_at_once() {
        let mut form = ArgsForm::new(&specs(), &Value::Null);
        form.set_text(2, "");
        form.set_text(3, "abc");
        form.set_text(4, "{oops");
        let e = form.collect().unwrap_err();
        assert_eq!(e.fields[&2], "Required");
        assert_eq!(e.fields[&3], "Enter a number");
        assert!(e.fields[&4].starts_with("Not valid JSON"), "{:?}", e.fields);
        assert!(e.general.is_empty());
        form.set_text(2, "1.5");
        assert_eq!(
            form.collect().unwrap_err().fields[&2],
            "Enter a whole number"
        );
    }

    #[test]
    fn collect_applies_the_shared_rules_to_what_the_fields_cannot_see() {
        let mut form = ArgsForm::new(&[arg("s", SavedArgType::String)], &Value::Null);
        form.set_text(0, "x".repeat(20_000));
        let e = form.collect().unwrap_err();
        assert!(e.fields.contains_key(&0) || !e.general.is_empty(), "{e:?}");
        // No arguments at all is fine.
        assert_eq!(
            ArgsForm::new(&[], &Value::Null).collect().unwrap(),
            json!({})
        );
    }

    // ── slash command ──

    fn parsed(text: &str) -> WorkflowInput {
        parse_workflow_input(text).unwrap().unwrap()
    }

    #[test]
    fn only_a_workflow_command_line_is_one() {
        assert_eq!(parse_workflow_input("hello"), None);
        assert_eq!(parse_workflow_input("/workflows"), None);
        assert_eq!(parse_workflow_input("/workflowx a"), None);
        assert_eq!(
            parse_workflow_input(" /workflow a"),
            None,
            "indentation sends it literally"
        );
        assert_eq!(parsed("/workflow"), WorkflowInput::List);
        assert_eq!(parsed("/workflow   "), WorkflowInput::List);
    }

    #[test]
    fn name_and_assignments_with_quoting() {
        assert_eq!(
            parsed("/workflow pr-review base=dev deep"),
            WorkflowInput::Run {
                name: "pr-review".into(),
                assignments: vec![("base".into(), Some("dev".into())), ("deep".into(), None)],
            }
        );
        let WorkflowInput::Run { assignments, .. } = parsed(
            r#"/workflow x note="two words" opts='{"a": [1, 2]}' path=a=b q="say \"hi\" \\ ok" empty="""#,
        ) else {
            panic!()
        };
        assert_eq!(assignments[0], ("note".into(), Some("two words".into())));
        assert_eq!(
            assignments[1],
            ("opts".into(), Some(r#"{"a": [1, 2]}"#.into()))
        );
        assert_eq!(assignments[2], ("path".into(), Some("a=b".into())));
        assert_eq!(
            assignments[3],
            ("q".into(), Some(r#"say "hi" \ ok"#.into()))
        );
        assert_eq!(assignments[4], ("empty".into(), Some(String::new())));
        assert!(
            parse_workflow_input("/workflow x a='open")
                .unwrap()
                .is_err()
        );
        assert!(
            parse_workflow_input("/workflow x a=\"open")
                .unwrap()
                .is_err()
        );
        assert!(
            parse_workflow_input("/workflow base=dev").unwrap().is_err(),
            "the name comes first"
        );
    }

    #[test]
    fn names_resolve_exactly_then_by_unique_prefix_and_only_winners_count() {
        let mut hidden = summary("pr-review", SavedScope::Builtin, vec![]);
        hidden.shadowed_by = Some(SavedScope::Project);
        let list = vec![
            hidden,
            summary("pr-review", SavedScope::Project, vec![]),
            summary("pr-notes", SavedScope::Global, vec![]),
            summary("repo-audit", SavedScope::Builtin, vec![]),
        ];
        assert_eq!(
            pick_workflow("pr-review", &list).unwrap().scope,
            SavedScope::Project
        );
        assert_eq!(pick_workflow("repo", &list).unwrap().name, "repo-audit");
        let e = pick_workflow("pr", &list).unwrap_err();
        assert!(e.contains("pr-review") && e.contains("pr-notes"), "{e}");
        let e = pick_workflow("nope", &list).unwrap_err();
        assert!(e.contains("Saved: pr-review, pr-notes, repo-audit."), "{e}");
        assert!(
            pick_workflow("x", &[])
                .unwrap_err()
                .contains("Nothing is saved yet")
        );
    }

    #[test]
    fn assignments_are_typed_against_the_declaration() {
        let wf = summary("pr-review", SavedScope::Global, specs());
        let b = bind_assignments(
            &wf,
            &[
                ("ticket".into(), Some("12".into())),
                ("deep".into(), None),
                ("base".into(), Some("dev".into())),
                ("opts".into(), Some(r#"{"x": 1}"#.into())),
            ],
        )
        .unwrap();
        assert_eq!(
            b.args,
            json!({"ticket": 12, "deep": true, "base": "dev", "opts": {"x": 1}})
        );
        assert!(b.missing.is_empty());
        // A required argument left out opens the launcher instead of failing.
        let b = bind_assignments(&wf, &[("base".into(), Some("dev".into()))]).unwrap();
        assert_eq!(b.missing, ["ticket"]);
    }

    #[test]
    fn assignment_errors_are_all_reported() {
        let wf = summary("pr-review", SavedScope::Global, specs());
        let e = bind_assignments(
            &wf,
            &[
                ("nope".into(), Some("1".into())),
                ("ticket".into(), Some("x".into())),
                ("base".into(), None),
                ("base".into(), Some("a".into())),
                ("ratio".into(), Some("2".into())),
                ("ratio".into(), Some("3".into())),
            ],
        )
        .unwrap_err();
        assert_eq!(e.len(), 4, "{e:?}");
        assert!(
            e[0].contains("not an argument of pr-review") && e[0].contains("base, deep, ticket"),
            "{e:?}"
        );
        assert!(e.iter().any(|m| m.contains("not a whole number")));
        assert!(e.iter().any(|m| m.contains("base needs a value")));
        assert!(e.iter().any(|m| m.contains("ratio is given twice")));
        let none = summary("plain", SavedScope::Global, vec![]);
        let e = bind_assignments(&none, &[("x".into(), Some("1".into()))]).unwrap_err();
        assert_eq!(e, ["plain takes no arguments (got \"x\")."]);
    }

    #[test]
    fn the_bare_command_says_what_is_available() {
        assert!(list_hint(&[]).contains("No saved workflows yet"));
        let list = vec![
            summary("a", SavedScope::Global, vec![]),
            summary("b", SavedScope::Global, vec![]),
        ];
        assert_eq!(
            list_hint(&list),
            "Run one with /workflow <name> key=value …  Saved: a, b."
        );
        let many: Vec<_> = (0..12)
            .map(|i| summary(&format!("w{i:02}"), SavedScope::Global, vec![]))
            .collect();
        assert!(list_hint(&many).contains("+4 more"));
    }

    // ── list grouping ──

    fn space(id: &str, path: &str, name: Option<&str>) -> Space {
        Space {
            id: id.into(),
            device_id: "d".into(),
            path: path.into(),
            name: name.map(str::to_owned),
            git_detected: true,
            git_checked_at: None,
            checkout_id: None,
            created_at: chrono::Utc::now(),
        }
    }

    fn project_wf(
        name: &str,
        root: &str,
        space: &str,
        shadows: Vec<SavedScope>,
    ) -> SavedWorkflowSummary {
        let mut w = summary(name, SavedScope::Project, vec![]);
        w.project_root = Some(root.into());
        w.space_id = Some(space.into());
        w.shadows = shadows;
        w
    }

    #[test]
    fn groups_are_global_then_projects_then_builtin_with_notes() {
        let mut global = summary("pr-review", SavedScope::Global, vec![]);
        global.shadows = vec![SavedScope::Builtin];
        let list = SavedWorkflowList {
            global_dir: Some("~/.zeron/workflows".into()),
            workflows: vec![
                summary("pr-review", SavedScope::Builtin, vec![]),
                summary("repo-audit", SavedScope::Builtin, vec![]),
                global,
                summary("alpha", SavedScope::Global, vec![]),
                project_wf(
                    "pr-review",
                    "/p/web",
                    "s-web",
                    vec![SavedScope::Global, SavedScope::Builtin],
                ),
                project_wf("zeta", "/p/api", "s-api", vec![]),
                project_wf("beta", "/p/api", "s-api", vec![]),
            ],
            invalid: vec![SavedWorkflowInvalid {
                path: "/p/web/.zeron/workflows/bad.star".into(),
                scope: SavedScope::Project,
                reason: "bad.star:2:3 unknown key `x`".into(),
                project_root: Some("/p/web".into()),
                space_id: Some("s-web".into()),
            }],
        };
        let spaces = [
            space("s-web", "/p/web", Some("Web app")),
            space("s-api", "/p/api", None),
        ];
        let groups = group(&list, &spaces);
        let titles: Vec<_> = groups.iter().map(|g| g.title.as_str()).collect();
        assert_eq!(titles, ["Global", "api", "Web app", "Built-in"]);
        assert_eq!(groups[0].subtitle.as_deref(), Some("~/.zeron/workflows"));
        let names = |g: &Group| {
            g.rows
                .iter()
                .map(|r| r.summary.name.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(names(&groups[0]), ["alpha", "pr-review"]);
        assert_eq!(names(&groups[1]), ["beta", "zeta"]);
        assert_eq!(names(&groups[3]), ["pr-review", "repo-audit"]);
        // Notes say who hides whom.
        assert_eq!(
            groups[0].rows[1].note.as_deref(),
            Some("Overrides the built-in workflow · Hidden in Web app")
        );
        assert_eq!(
            groups[2].rows[0].note.as_deref(),
            Some("Overrides the global and built-in workflows")
        );
        assert_eq!(
            groups[3].rows[0].note.as_deref(),
            Some("Overridden by a global workflow of the same name")
        );
        assert!(groups[3].rows[1].note.is_none());
        // The unusable file shows up in its project.
        assert_eq!(groups[2].invalid.len(), 1);
        assert!(groups[1].invalid.is_empty());
        assert!(matches!(groups[1].kind, GroupKind::Project { .. }));
    }

    #[test]
    fn an_empty_list_still_offers_the_global_group_and_unknown_projects_use_their_folder_name() {
        let groups = group(&SavedWorkflowList::default(), &[]);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].title, "Global");
        assert!(groups[0].rows.is_empty());
        assert_eq!(groups[0].subtitle.as_deref(), Some("every project"));
        let list = SavedWorkflowList {
            workflows: vec![project_wf("x", "/home/me/code/thing/", "gone", vec![])],
            ..Default::default()
        };
        assert_eq!(group(&list, &[])[1].title, "thing");
    }

    #[test]
    fn keys_identify_a_row_across_refreshes() {
        let a = project_wf("x", "/p/a", "s", vec![]);
        let b = project_wf("x", "/p/b", "s2", vec![]);
        assert_ne!(WorkflowKey::of(&a), WorkflowKey::of(&b));
        assert_eq!(WorkflowKey::of(&a), WorkflowKey::of(&a.clone()));
        assert_eq!(file_name("/x/y/z.star"), "z.star");
        assert_eq!(file_name("C:\\x\\z.star"), "z.star");
    }

    // ── run history ──

    #[test]
    fn run_rows_say_how_each_run_ended() {
        let now = chrono::Utc::now();
        let mut done = WorkflowRunHeader {
            run_id: "r1".into(),
            chat_id: "c".into(),
            status: WorkflowStatus::Completed,
            created_at: now.timestamp_millis() - 3 * 60_000,
            ..Default::default()
        };
        done.usage.elapsed_ms = 192_000;
        done.usage.input_tokens = 48_000;
        done.usage.nodes_used = 12;
        let denied = WorkflowRunHeader {
            run_id: "r2".into(),
            status: WorkflowStatus::Stopped,
            stop_reason: Some(zeron_proto::WorkflowStopReason::Denied),
            created_at: now.timestamp_millis(),
            ..Default::default()
        };
        let failed = WorkflowRunHeader {
            status: WorkflowStatus::Errored,
            resumed_from: Some("r0".into()),
            ..Default::default()
        };
        let rows = run_rows(&[done, denied, failed], now);
        assert_eq!(rows[0].status, "Completed");
        assert_eq!(rows[0].tone, Tone::Success);
        assert!(rows[0].detail.starts_with("3m 12s"), "{}", rows[0].detail);
        assert!(rows[0].detail.contains("tokens") && rows[0].detail.contains("12 steps"));
        assert_eq!(rows[1].status, "Denied");
        assert_eq!(rows[2].status, "Failed");
        assert!(rows[2].resumed);
        assert_eq!(rows[0].when, "3m ago");
        assert_eq!(rows[1].when, "just now");
    }

    // ── save dialog ──

    #[test]
    fn the_save_dialog_suggests_a_name_and_validates() {
        let form = SaveForm::for_run("PR review!", true);
        assert_eq!(form.name, "pr-review");
        assert_eq!(form.scope, SavedScope::Project);
        assert_eq!(SaveForm::for_run("***", false).name, "");
        assert_eq!(SaveForm::for_run("x", false).scope, SavedScope::Global);
        let mut form = form;
        let e = form.validate(true).unwrap_err();
        assert!(e.description.is_some() && e.name.is_none());
        form.description = "Reviews the diff".into();
        assert!(form.validate(true).is_ok());
        form.name = "Bad Name".into();
        assert_eq!(
            form.validate(true).unwrap_err().name.as_deref(),
            Some("Lowercase letters, digits, - and _")
        );
        form.name = String::new();
        assert!(
            form.validate(true)
                .unwrap_err()
                .name
                .unwrap()
                .contains("Give it a name")
        );
        form.name = "con".into();
        assert!(
            form.validate(true)
                .unwrap_err()
                .name
                .unwrap()
                .contains("reserved")
        );
        form.name = "ok".into();
        form.description = "two\nlines".into();
        assert!(form.validate(true).unwrap_err().description.is_some());
        form.description = "x".repeat(301);
        assert!(
            form.validate(true)
                .unwrap_err()
                .description
                .unwrap()
                .contains("300")
        );
        form.description = "ok".into();
        form.when_to_use = "y".repeat(601);
        assert!(form.validate(true).unwrap_err().when_to_use.is_some());
        form.when_to_use.clear();
        form.scope = SavedScope::Project;
        assert!(form.validate(false).unwrap_err().scope.is_some());
        form.scope = SavedScope::Global;
        assert!(form.validate(false).is_ok());
    }

    #[test]
    fn a_save_keeps_the_arguments_a_run_was_started_with() {
        let inferred = infer_args(&json!({
            "base": "main", "n": 3, "ratio": 2.5, "deep": true, "opts": {"a": 1}, "bad-name": 1,
        }));
        let by = |n: &str| inferred.iter().find(|a| a.name == n).unwrap();
        assert_eq!(by("base").ty, SavedArgType::String);
        assert_eq!(by("n").ty, SavedArgType::Int);
        assert_eq!(by("ratio").ty, SavedArgType::Number);
        assert_eq!(by("deep").ty, SavedArgType::Bool);
        assert_eq!(by("opts").ty, SavedArgType::Json);
        assert_eq!(by("n").default, Some(json!(3)));
        assert!(!inferred.iter().any(|a| a.name == "bad-name"));
        assert!(infer_args(&Value::Null).is_empty());
        let huge = infer_args(&json!({"s": "x".repeat(20_000)}));
        assert_eq!(
            huge[0].default, None,
            "an oversize default is dropped, not saved broken"
        );
    }
}
