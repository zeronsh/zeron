//! Frontend-agnostic view logic: the derivations every viewport needs and none
//! of them should own — sort orders, staleness gating, sidebar grouping, the
//! boot gate, relative times.
//!
//! This lives in `proto` rather than in the viewport crate so the rules stay
//! pure and independently testable: the same workspace doc must produce the
//! same row order on every surface, and there is exactly one implementation
//! and one test suite per rule.
//!
//! Everything in this module is pure. `chat_indicator` (the status derivation
//! these gate on) is in [`crate::entities`].

use chrono::{DateTime, Utc};

use crate::{AuthState, Chat, ChatIndicator, Session, SessionStatus, Space, WorkspaceScope};

// ---------------------------------------------------------------------------
// Connection + status
// ---------------------------------------------------------------------------

/// Viewport ⇄ engine connection lifecycle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionStatus {
    Connecting,
    Ready,
    Failed(String),
}

/// What a chat's status dot / working indicator should show right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Indicator {
    None,
    Working,
    AwaitingInput,
    Errored,
}

/// A `Working`/`AwaitingInput` session older than this is treated as dead — a
/// crashed backend must never show an eternal "Working" (feature-inventory
/// §1.12). Engines heartbeat sessions well inside this window.
pub const SESSION_STALE_MS: i64 = 45_000;

/// Staleness-checked indicator for a session row. Pure.
pub fn effective_indicator(session: Option<&Session>, now: DateTime<Utc>) -> Indicator {
    let Some(session) = session else {
        return Indicator::None;
    };
    match session.status {
        SessionStatus::Idle => Indicator::None,
        SessionStatus::Errored => Indicator::Errored,
        SessionStatus::Working | SessionStatus::AwaitingInput => {
            let age_ms = now
                .signed_duration_since(session.updated_at)
                .num_milliseconds();
            if age_ms > SESSION_STALE_MS {
                Indicator::None
            } else if session.status == SessionStatus::Working {
                Indicator::Working
            } else {
                Indicator::AwaitingInput
            }
        }
    }
}

/// The full display status for a chat row / tab dot: live states win, then the
/// synced seen marker decides completed-vs-idle. Staleness gating rides on
/// [`effective_indicator`]; the derivation itself is [`crate::chat_indicator`].
pub fn display_status(chat: &Chat, session: Option<&Session>, now: DateTime<Utc>) -> ChatIndicator {
    let live = session.filter(|s| effective_indicator(Some(s), now) != Indicator::None);
    crate::chat_indicator(chat, live)
}

/// Attention bucket for the sidebar's Active list — lower is more urgent.
pub fn attention_rank(status: ChatIndicator) -> u8 {
    match status {
        ChatIndicator::AwaitingInput => 0,
        ChatIndicator::Errored => 1,
        ChatIndicator::Working => 2,
        ChatIndicator::Completed => 3,
        ChatIndicator::Idle => 4,
    }
}

// ---------------------------------------------------------------------------
// Sort orders
// ---------------------------------------------------------------------------

/// Active-list order: pure recency (`last_message_at` desc, `created_at`
/// fallback), id tiebreak so the sort is total. Deliberately NOT
/// attention-bucketed: status drives the DOT, never the position — bucketing
/// meant that merely OPENING a completed session (completed → seen → idle)
/// dropped its row under the pointer (user report: "their position in the
/// scrollbar changes"). Matches the old sidebar, which rendered chats in
/// recency order and let the dots carry urgency; [`attention_rank`] still
/// aggregates the space rows' urgency dot.
pub fn sort_active(rows: &mut Vec<(ChatIndicator, &Chat)>) {
    rows.sort_by(|(_, a), (_, b)| {
        let ka = a.last_message_at.unwrap_or(a.created_at);
        let kb = b.last_message_at.unwrap_or(b.created_at);
        kb.cmp(&ka).then_with(|| a.id.cmp(&b.id))
    });
}

/// Session-tab order for a space: creation order (activity never reorders
/// tabs), id tiebreak. Pure.
pub fn sort_tabs(chats: &mut [&Chat]) {
    chats.sort_by(|a, b| {
        a.created_at
            .cmp(&b.created_at)
            .then_with(|| a.id.cmp(&b.id))
    });
}

/// Spaces list order: creation order, id tiebreak — total and stable across
/// devices. Pure.
pub fn sort_spaces(spaces: &mut [Space]) {
    spaces.sort_by(|a, b| {
        a.created_at
            .cmp(&b.created_at)
            .then_with(|| a.id.cmp(&b.id))
    });
}

/// Sidebar order: `last_message_at` desc, falling back to `created_at`; ties
/// break by `created_at` desc then id so the sort is total and stable across
/// devices. Pure.
pub fn sort_chats(chats: &mut [Chat]) {
    chats.sort_by(|a, b| {
        let ka = a.last_message_at.unwrap_or(a.created_at);
        let kb = b.last_message_at.unwrap_or(b.created_at);
        kb.cmp(&ka)
            .then_with(|| b.created_at.cmp(&a.created_at))
            .then_with(|| a.id.cmp(&b.id))
    });
}

// ---------------------------------------------------------------------------
// Boot gate
// ---------------------------------------------------------------------------

/// The app gate (zeron's App.tsx phases). Pure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GatePhase {
    /// Booting / probing — splash covers this.
    Loading,
    /// Engine unreachable and embedding failed.
    Failed(String),
    /// Engine up, but signed out — show the sign-in card.
    SignIn,
    /// Signed in but no organization selected — "Create your workspace".
    OrgGate,
    /// Render the shell.
    Ready,
}

/// Missing scope is treated as synced. Current engines always publish
/// [`WorkspaceScope`] before becoming ready, while old daemons are deliberately
/// kept behind the account gate instead of being mistaken for local runtimes.
pub fn gate_phase(
    connection: &ConnectionStatus,
    workspace_scope: Option<WorkspaceScope>,
    auth: Option<&AuthState>,
) -> GatePhase {
    match connection {
        ConnectionStatus::Connecting => GatePhase::Loading,
        ConnectionStatus::Failed(err) => GatePhase::Failed(err.clone()),
        ConnectionStatus::Ready => match workspace_scope.unwrap_or(WorkspaceScope::Synced) {
            WorkspaceScope::Local | WorkspaceScope::Development => GatePhase::Ready,
            WorkspaceScope::Synced => match auth {
                Some(AuthState::NeedsOrganization { .. }) => GatePhase::OrgGate,
                Some(AuthState::SignedIn { .. }) => GatePhase::Ready,
                Some(AuthState::SignedOut) | None => GatePhase::SignIn,
            },
        },
    }
}

#[cfg(test)]
mod gate_tests {
    use super::*;
    use crate::UserProfile;

    fn user() -> UserProfile {
        UserProfile {
            id: "user-1".into(),
            email: "user@example.com".into(),
            name: None,
        }
    }

    #[test]
    fn workspace_scope_controls_the_auth_gate() {
        assert_eq!(
            gate_phase(
                &ConnectionStatus::Ready,
                Some(WorkspaceScope::Local),
                Some(&AuthState::SignedOut),
            ),
            GatePhase::Ready
        );
        assert_eq!(
            gate_phase(
                &ConnectionStatus::Ready,
                Some(WorkspaceScope::Synced),
                Some(&AuthState::SignedOut),
            ),
            GatePhase::SignIn
        );
        assert_eq!(
            gate_phase(
                &ConnectionStatus::Ready,
                Some(WorkspaceScope::Synced),
                Some(&AuthState::NeedsOrganization { user: user() }),
            ),
            GatePhase::OrgGate
        );
    }

    #[test]
    fn development_and_local_never_use_the_workos_gate() {
        for scope in [WorkspaceScope::Local, WorkspaceScope::Development] {
            for auth in [
                AuthState::SignedOut,
                AuthState::NeedsOrganization { user: user() },
                AuthState::SignedIn {
                    user: user(),
                    org_id: Some("org-1".into()),
                },
            ] {
                assert_eq!(
                    gate_phase(&ConnectionStatus::Ready, Some(scope), Some(&auth)),
                    GatePhase::Ready
                );
            }
        }
    }

    #[test]
    fn missing_scope_falls_back_to_a_synced_gate() {
        assert_eq!(
            gate_phase(&ConnectionStatus::Ready, None, None),
            GatePhase::SignIn
        );
    }
}

/// Parse an `AuthStatus` frame tolerantly. The engine currently serializes its
/// own enum (`{"_tag": "SignedIn", ...}`) while the proto type expects
/// `{"state": "signedIn", ...}` — accept both so either side can converge
/// without breaking a viewport.
pub fn parse_auth_state(value: &serde_json::Value) -> Option<AuthState> {
    if let Ok(state) = serde_json::from_value::<AuthState>(value.clone()) {
        return Some(state);
    }
    let tag = value.get("_tag").and_then(|t| t.as_str())?;
    let user = || -> Option<crate::UserProfile> {
        let u = value.get("user")?;
        Some(crate::UserProfile {
            id: u.get("id")?.as_str()?.to_string(),
            email: u.get("email")?.as_str()?.to_string(),
            name: u.get("name").and_then(|n| n.as_str()).map(str::to_string),
        })
    };
    match tag {
        "SignedOut" => Some(AuthState::SignedOut),
        "NeedsOrganization" => Some(AuthState::NeedsOrganization { user: user()? }),
        "SignedIn" => Some(AuthState::SignedIn {
            user: user()?,
            org_id: value
                .get("orgId")
                .and_then(|v| v.as_str())
                .map(str::to_string),
        }),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Sidebar grouping
// ---------------------------------------------------------------------------

/// One grouped-by-project sidebar section.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatGroup<'a> {
    pub label: String,
    pub chats: Vec<&'a Chat>,
}

/// Project label for a chat: the basename of its cwd, or "No project".
pub fn project_label(cwd: Option<&str>) -> String {
    let Some(cwd) = cwd.map(str::trim).filter(|c| !c.is_empty()) else {
        return "No project".to_string();
    };
    if matches!(cwd, "~" | "~/") {
        return "No project".to_string();
    }
    std::path::Path::new(cwd.trim_end_matches(['/', '\\']))
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| cwd.to_string())
}

/// Group chats by project label, preserving the incoming (recency) order both
/// for groups (by their most recent chat) and rows within a group. Pure.
pub fn group_chats<'a>(chats: impl IntoIterator<Item = &'a Chat>) -> Vec<ChatGroup<'a>> {
    let mut groups: Vec<ChatGroup<'a>> = Vec::new();
    for chat in chats {
        let label = project_label(chat.cwd.as_deref());
        match groups.iter_mut().find(|g| g.label == label) {
            Some(group) => group.chats.push(chat),
            None => groups.push(ChatGroup {
                label,
                chats: vec![chat],
            }),
        }
    }
    groups
}

/// Compact relative time ("now", "5m", "3h", "2d", "1w", …) — no "ago" suffix;
/// port of zeron's `formatTimeAgo`.
pub fn format_time_ago(then: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let s = now.signed_duration_since(then).num_seconds().max(0);
    // Under a minute reads as "now" — otherwise 45–59s floors to a bare "0m".
    if s < 60 {
        return "now".to_string();
    }
    let m = s / 60;
    if m < 60 {
        return format!("{m}m");
    }
    let h = m / 60;
    if h < 24 {
        return format!("{h}h");
    }
    let d = h / 24;
    if d < 7 {
        return format!("{d}d");
    }
    let w = d / 7;
    if w < 5 {
        return format!("{w}w");
    }
    let mo = d / 30;
    if mo < 12 {
        return format!("{mo}mo");
    }
    format!("{}y", d / 365)
}

/// Session-row sub-line, "project · branch" (zeron `chatLocation`): the repo
/// checkout identity. Either part may be missing; empty when both are.
pub fn chat_location(chat: &Chat) -> Option<String> {
    let project = chat
        .cwd
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .map(|c| project_label(Some(c)));
    let reference = chat
        .branch
        .as_deref()
        .map(str::trim)
        .filter(|b| !b.is_empty());
    match (project, reference) {
        (Some(p), Some(r)) => Some(format!("{p} · {r}")),
        (Some(p), None) => Some(p),
        (None, Some(r)) => Some(r.to_string()),
        (None, None) => None,
    }
}

// ---------------------------------------------------------------------------
// Tool summaries (pure)
// ---------------------------------------------------------------------------

/// Collapse model-generated text onto ONE line for single-line surfaces (tool
/// chips, titles, previews): newlines, tabs and runs of whitespace become
/// single spaces, trimmed.
///
/// Both viewports need this for the same reason from opposite directions — gpui
/// breaks on a literal `\n` before its ellipsis logic, and a terminal cell grid
/// would take an embedded newline as a cursor move.
pub fn single_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("{n} {one}")
    } else {
        format!("{n} {many}")
    }
}

/// Per-kind chip label + one-line detail. Labels match zeron's `describeTool`
/// (tool-chip.tsx) exactly, so the two viewports name a tool identically.
pub fn tool_chip_content(call: &crate::ToolCall) -> (&'static str, String) {
    let (label, detail) = tool_chip_content_raw(call);
    (label, single_line(&detail))
}

fn tool_chip_content_raw(call: &crate::ToolCall) -> (&'static str, String) {
    use crate::ToolCall;
    match call {
        ToolCall::Exec { command } => ("Run", command.clone()),
        ToolCall::ReadFile { path } => ("Read", path.clone()),
        ToolCall::WriteFile { path, .. } => ("Write", path.clone()),
        ToolCall::EditFile { path, .. } => ("Edit", path.clone()),
        ToolCall::ApplyPatch { path } => {
            ("Patch", path.clone().unwrap_or_else(|| "workspace".into()))
        }
        ToolCall::Search { pattern, path } => (
            "Search",
            match path {
                Some(path) => format!("{pattern} in {path}"),
                None => pattern.clone(),
            },
        ),
        ToolCall::Glob { pattern } => ("Glob", pattern.clone()),
        ToolCall::WebFetch { url, .. } => ("Fetch", url.clone()),
        ToolCall::WebSearch { query } => ("Web", query.clone()),
        ToolCall::Todo { items } => {
            let done = items.iter().filter(|i| i.done).count();
            ("Todo", format!("{done}/{} done", items.len()))
        }
        ToolCall::Mcp { server, tool, .. } => ("MCP", format!("{server} · {tool}")),
        // Subagent spawns decode as Unknown named "Agent[: <description>]"
        // (every native driver's convention): label them "Agent" with the
        // description as the detail — "Tool · Agent: scan repo" read as two
        // labels fighting.
        ToolCall::Unknown { name, .. } => match name.strip_prefix("Agent: ") {
            Some(description) => ("Agent", description.to_owned()),
            None if name == "Agent" => ("Agent", String::new()),
            None => ("Tool", name.clone()),
        },
    }
}

/// The ToolGroup summary line — "Ran 3 commands · edited 2 files".
///
/// Takes `(call, is_error)` pairs so each viewport can keep its own row model;
/// the summary itself is one implementation for both.
pub fn tool_group_summary(tools: &[(crate::ToolCall, bool)]) -> String {
    use crate::ToolCall;
    let mut commands = 0usize;
    let mut edited: Vec<&str> = Vec::new();
    let mut reads = 0usize;
    let mut searches = 0usize;
    let mut fetches = 0usize;
    let mut todos = 0usize;
    let mut other = 0usize;
    let mut failed = 0usize;
    for (call, is_error) in tools {
        if *is_error {
            failed += 1;
        }
        match call {
            ToolCall::Exec { .. } => commands += 1,
            ToolCall::WriteFile { path, .. } | ToolCall::EditFile { path, .. } => {
                if !edited.contains(&path.as_str()) {
                    edited.push(path);
                }
            }
            ToolCall::ApplyPatch { path } => {
                let p = path.as_deref().unwrap_or("patch");
                if !edited.contains(&p) {
                    edited.push(p);
                }
            }
            ToolCall::ReadFile { .. } => reads += 1,
            ToolCall::Search { .. } | ToolCall::Glob { .. } | ToolCall::WebSearch { .. } => {
                searches += 1
            }
            ToolCall::WebFetch { .. } => fetches += 1,
            ToolCall::Todo { .. } => todos += 1,
            ToolCall::Mcp { .. } | ToolCall::Unknown { .. } => other += 1,
        }
    }
    let mut segments: Vec<String> = Vec::new();
    if commands > 0 {
        segments.push(format!("ran {}", plural(commands, "command", "commands")));
    }
    if !edited.is_empty() {
        segments.push(format!("edited {}", plural(edited.len(), "file", "files")));
    }
    if reads > 0 {
        segments.push(format!("read {}", plural(reads, "file", "files")));
    }
    if searches > 0 {
        segments.push(format!("searched {}", plural(searches, "time", "times")));
    }
    if fetches > 0 {
        segments.push(format!("fetched {}", plural(fetches, "page", "pages")));
    }
    if todos > 0 {
        segments.push("updated todos".to_string());
    }
    if other > 0 {
        segments.push(format!("called {}", plural(other, "tool", "tools")));
    }
    if segments.is_empty() {
        segments.push(plural(tools.len(), "tool", "tools"));
    }
    if failed > 0 {
        segments.push(format!("{failed} failed"));
    }
    let mut summary = segments.join(" · ");
    // Capitalize the first segment only (zeron's style).
    if let Some(first) = summary.get(0..1) {
        let upper = first.to_uppercase();
        summary.replace_range(0..1, &upper);
    }
    summary
}

// ---------------------------------------------------------------------------
// `zeron chat` exec recognition (pure)
// ---------------------------------------------------------------------------

/// Which `zeron chat` verb an exec'd command line invokes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZeronChatVerb {
    Spawn,
    Tell,
    Wait,
    Output,
}

/// A recognized `zeron chat spawn|tell|wait|output` invocation: the verb plus
/// its positional chat arguments (flags and their values removed). The UI
/// renders these as agent chips rather than ordinary commands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZeronChatCommand {
    pub verb: ZeronChatVerb,
    /// Positional arguments after flag stripping — chat ids, prefixes or
    /// titles (`wait` can take several; `spawn` normally carries none).
    pub targets: Vec<String>,
    /// Display label the caller asked for: `--title`, else the inline
    /// `--prompt` text (`--prompt-file` paths are NOT titles — skipped).
    pub label: Option<String>,
    /// The `--title` value alone — the spawned chat carries it verbatim, so
    /// a spawn chip whose output never persisted can still find its child by
    /// name (`label` may hold prompt text instead, which never matches).
    pub title: Option<String>,
    /// The `--harness` value, when given — the spawn chip shows its brand
    /// mark even before the child chat resolves.
    pub harness: Option<String>,
    /// The `--model` value, when given — the spawn chip's model trail before
    /// the child chat resolves.
    pub model: Option<String>,
}

/// Split one shell line into words: single/double quotes and backslash
/// escapes are honored, operators (`&&`, `;`, `|`, `>`) stay their own
/// tokens. Not a full shell grammar — enough to recognize the CLI's own
/// call shapes.
fn shell_words(line: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut has_word = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {
                if has_word {
                    words.push(std::mem::take(&mut word));
                    has_word = false;
                }
            }
            '\'' => {
                has_word = true;
                for c in chars.by_ref() {
                    if c == '\'' {
                        break;
                    }
                    word.push(c);
                }
            }
            '"' => {
                has_word = true;
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' => {
                            if let Some(c) = chars.next() {
                                word.push(c);
                            }
                        }
                        _ => word.push(c),
                    }
                }
            }
            '\\' => {
                has_word = true;
                if let Some(c) = chars.next() {
                    word.push(c);
                }
            }
            '&' if chars.peek() == Some(&'&') => {
                chars.next();
                if has_word {
                    words.push(std::mem::take(&mut word));
                    has_word = false;
                }
                words.push("&&".into());
            }
            '|' if chars.peek() == Some(&'|') => {
                chars.next();
                if has_word {
                    words.push(std::mem::take(&mut word));
                    has_word = false;
                }
                words.push("||".into());
            }
            ';' | '|' | '>' => {
                if has_word {
                    words.push(std::mem::take(&mut word));
                    has_word = false;
                }
                words.push(c.to_string());
            }
            _ => {
                has_word = true;
                word.push(c);
            }
        }
    }
    if has_word {
        words.push(word);
    }
    words
}

/// Is this word the `zeron` binary: bare `zeron`, `$ZERON_CLI` (any quoting
/// already stripped), or an absolute path ending in `/zeron`.
fn is_zeron_binary(word: &str) -> bool {
    word == "zeron" || word == "$ZERON_CLI" || (word.starts_with('/') && word.ends_with("/zeron"))
}

/// Long flags that consume the NEXT word as their value (`--flag value`;
/// `--flag=value` needs no skip). Every other `--flag` is boolean.
const ZERON_CHAT_VALUE_FLAGS: &[&str] = &[
    "--prompt",
    "--prompt-file",
    "--message-file",
    "--timeout",
    "--harness",
    "--model",
    "--reasoning",
    "--title",
    "--project",
    "--device",
    "--base",
    "--parent",
    "--cwd",
    "--mode",
    "--request",
    "--limit",
];

/// Recognize an exec'd `zeron chat` orchestration command. Tolerates the
/// realistic wrappers agents emit — a `cd … &&` prefix, env assignments,
/// `env`, `bash|sh -c '…'` — but never a command that merely CONTAINS the
/// words (`echo zeron chat spawn`, `grep "zeron chat"`).
pub fn zeron_chat_command(command: &str) -> Option<ZeronChatCommand> {
    let words = shell_words(command);
    zeron_chat_command_words(&words)
}

fn zeron_chat_command_words(words: &[String]) -> Option<ZeronChatCommand> {
    let mut at = 0;
    // Leading `cd <dir> &&` segments and `NAME=value` env assignments
    // (optionally behind `env`) precede the real command.
    loop {
        match words.get(at).map(String::as_str) {
            Some("cd") => {
                at += 2; // `cd` and its target
                match words.get(at).map(String::as_str) {
                    Some("&&") | Some(";") => at += 1,
                    _ => return None,
                }
            }
            Some("env") => at += 1,
            Some(word)
                if word.split_once('=').is_some_and(|(name, _)| {
                    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                }) =>
            {
                at += 1;
            }
            _ => break,
        }
    }
    // `bash -lc '…'`, `sh -c "…"` and friends: the flagged shell re-parses
    // its one command argument.
    if matches!(
        words.get(at).map(String::as_str),
        Some("bash" | "sh" | "zsh")
    ) {
        let mut ix = at + 1;
        while let Some(flag) = words.get(ix) {
            if flag.starts_with('-') && !flag.contains('c') {
                ix += 1;
            } else {
                break;
            }
        }
        if words
            .get(ix)
            .is_some_and(|flag| flag.starts_with('-') && flag.contains('c'))
            && let Some(inner) = words.get(ix + 1)
        {
            return zeron_chat_command(inner);
        }
        return None;
    }
    if !words.get(at).is_some_and(|w| is_zeron_binary(w)) {
        return None;
    }
    if words.get(at + 1).map(String::as_str) != Some("chat") {
        return None;
    }
    let verb = match words.get(at + 2).map(String::as_str) {
        Some("spawn") => ZeronChatVerb::Spawn,
        Some("tell") => ZeronChatVerb::Tell,
        Some("wait") => ZeronChatVerb::Wait,
        // `read` is the CLI's documented alias of `output` — same chip.
        Some("output") | Some("read") => ZeronChatVerb::Output,
        _ => return None,
    };
    let mut targets = Vec::new();
    let mut label = None;
    let mut title = None;
    let mut harness = None;
    let mut model = None;
    let mut ix = at + 3;
    while let Some(word) = words.get(ix) {
        ix += 1;
        match word.as_str() {
            // A later pipeline/chain is a different command; stop there.
            "&&" | "||" | ";" | "|" | ">" => break,
            w if w.starts_with("--") => {
                let (flag, inline) = match w.split_once('=') {
                    Some((flag, value)) => (flag, Some(value.to_owned())),
                    None => (w, None),
                };
                let value = inline.or_else(|| {
                    ZERON_CHAT_VALUE_FLAGS
                        .contains(&flag)
                        .then(|| words.get(ix).cloned())
                        .flatten()
                        .inspect(|_| ix += 1)
                });
                match flag {
                    "--title" => {
                        if title.is_none() {
                            title = value.clone();
                        }
                        if label.is_none() {
                            label = value;
                        }
                    }
                    "--prompt" => {
                        if label.is_none() {
                            label = value;
                        }
                    }
                    "--harness" if harness.is_none() => {
                        harness = value;
                    }
                    "--model" if model.is_none() => {
                        model = value;
                    }
                    _ => {}
                }
            }
            w if w.starts_with('-') && w.len() > 1 => {}
            _ => {
                // `tell`/`output` take ONE chat then free text; `wait` takes
                // several; `spawn` takes none (its prompt rides a flag).
                let takes_many = verb == ZeronChatVerb::Wait;
                if takes_many || targets.is_empty() {
                    targets.push(word.clone());
                }
            }
        }
    }
    Some(ZeronChatCommand {
        verb,
        targets,
        label,
        title,
        harness,
        model,
    })
}

/// The status-dot palette, as oklch triples (L, C, H°).
///
/// Colors live here rather than in the viewport because the *meaning* of a
/// dot is part of the protocol, not the presentation — a given status must
/// read the same on every surface. `zeron-ui` has the oklch→sRGB math.
pub mod dot {
    /// Running. Pink, not amber: the harsh yellow read as a warning, and running
    /// is routine (user request).
    pub const WORKING: (f32, f32, f32) = (0.718, 0.202, 349.761);
    /// Asking a question. Indigo — must read differently from "busy" at a glance.
    pub const AWAITING: (f32, f32, f32) = (0.673, 0.182, 276.935);
    /// Errored. Red-400.
    pub const ERRORED: (f32, f32, f32) = (0.704, 0.191, 22.216);
    /// Finished but unseen. Emerald — reads as "ready for you".
    pub const COMPLETED: (f32, f32, f32) = (0.765, 0.177, 163.223);
}

// ---------------------------------------------------------------------------
// Checkout selection (new sessions)
// ---------------------------------------------------------------------------

/// Where a new session runs (t3code's env-mode: `local | worktree`).
///
/// "Current worktree" is deliberately **not** a third mode — it is `Local` when
/// the picked ref already happens to be materialized as a worktree, in which
/// case the session reuses that checkout's path. Modelling it as three states
/// would let the UI hold a combination the engine cannot honour.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CheckoutKind {
    /// The space's own folder — or the picked ref's existing worktree.
    #[default]
    Local,
    /// A fresh isolated worktree created off the picked base ref on send.
    NewWorktree,
}

/// The resolved on-send checkout action.
#[derive(Debug, Clone, PartialEq)]
pub enum CheckoutPlan {
    /// Run in the space folder as-is. `branch` is the checkout's branch (the
    /// picked or current ref), carried onto `createChat` so the session names
    /// it from the first frame; `None` = refs never loaded.
    CurrentCheckout { branch: Option<String> },
    /// Reuse the picked ref's existing worktree (a cwd override; no git).
    ReuseWorktree { path: String, branch: String },
    /// `CreateWorktree` off `base` on send (the engine mints a `zeron/<name>`
    /// branch). `base: None` = refs never loaded — send falls back to the space
    /// folder rather than failing.
    NewWorktree { base: Option<String> },
}

/// Resolve the on-send action from the mode and the picked ref.
pub fn checkout_plan(kind: CheckoutKind, picked: Option<&crate::RepoRef>) -> CheckoutPlan {
    let name = picked.map(|r| r.name.clone());
    match kind {
        CheckoutKind::NewWorktree => CheckoutPlan::NewWorktree { base: name },
        CheckoutKind::Local => match picked.and_then(|r| r.worktree_path.clone()) {
            Some(path) => CheckoutPlan::ReuseWorktree {
                path,
                branch: name.unwrap_or_default(),
            },
            None => CheckoutPlan::CurrentCheckout { branch: name },
        },
    }
}

/// Label of the checkout-kind trigger (t3code `resolveEnvModeLabel`).
pub fn checkout_label(kind: CheckoutKind, picked: Option<&crate::RepoRef>) -> &'static str {
    match kind {
        CheckoutKind::NewWorktree => "New worktree",
        CheckoutKind::Local => {
            if picked.is_some_and(|r| r.worktree_path.is_some()) {
                "Current worktree"
            } else {
                "Current checkout"
            }
        }
    }
}

#[cfg(test)]
mod checkout_tests {
    use super::*;
    use crate::RepoRef;

    fn plain(name: &str) -> RepoRef {
        RepoRef {
            name: name.into(),
            current: false,
            worktree_path: None,
        }
    }

    fn materialized(name: &str, path: &str) -> RepoRef {
        RepoRef {
            name: name.into(),
            current: false,
            worktree_path: Some(path.into()),
        }
    }

    #[test]
    fn local_resolves_by_whether_the_ref_has_a_worktree() {
        // The same mode means two different things depending on the ref — which
        // is exactly why "current worktree" is not its own state.
        assert_eq!(
            checkout_plan(CheckoutKind::Local, Some(&plain("main"))),
            CheckoutPlan::CurrentCheckout {
                branch: Some("main".into())
            }
        );
        assert_eq!(
            checkout_plan(CheckoutKind::Local, Some(&materialized("feat", "/wt/feat"))),
            CheckoutPlan::ReuseWorktree {
                path: "/wt/feat".into(),
                branch: "feat".into()
            }
        );
        // No ref picked at all is still the space folder — with no branch to
        // stamp until refs load.
        assert_eq!(
            checkout_plan(CheckoutKind::Local, None),
            CheckoutPlan::CurrentCheckout { branch: None }
        );
    }

    #[test]
    fn new_worktree_carries_its_base_and_tolerates_none() {
        assert_eq!(
            checkout_plan(CheckoutKind::NewWorktree, Some(&plain("main"))),
            CheckoutPlan::NewWorktree {
                base: Some("main".into())
            }
        );
        // Refs never loaded: send falls back to the space folder rather than
        // failing, so the base is allowed to be absent.
        assert_eq!(
            checkout_plan(CheckoutKind::NewWorktree, None),
            CheckoutPlan::NewWorktree { base: None }
        );
    }

    #[test]
    fn labels_say_which_of_the_three_outcomes_you_will_get() {
        assert_eq!(
            checkout_label(CheckoutKind::Local, Some(&plain("main"))),
            "Current checkout"
        );
        assert_eq!(
            checkout_label(CheckoutKind::Local, Some(&materialized("f", "/wt/f"))),
            "Current worktree"
        );
        assert_eq!(
            checkout_label(CheckoutKind::NewWorktree, Some(&plain("main"))),
            "New worktree"
        );
    }
}

#[cfg(test)]
mod zeron_chat_command_tests {
    use super::*;

    fn cmd(s: &str) -> Option<ZeronChatCommand> {
        zeron_chat_command(s)
    }

    const CHILD: &str = "3f6b2a18-9c4d-4e5f-8a7b-1c2d3e4f5a6b";

    #[test]
    fn spawn_forms_all_classify() {
        assert_eq!(
            cmd("zeron chat spawn --prompt \"scan the repo\" --harness claude-code"),
            Some(ZeronChatCommand {
                verb: ZeronChatVerb::Spawn,
                targets: vec![],
                label: Some("scan the repo".into()),
                title: None,
                harness: Some("claude-code".into()),
                model: None
            })
        );
        assert_eq!(
            cmd("zeron chat spawn --prompt-file /tmp/brief.md --model opus --wait --timeout 20m"),
            Some(ZeronChatCommand {
                verb: ZeronChatVerb::Spawn,
                targets: vec![],
                label: None,
                title: None,
                harness: None,
                model: Some("opus".into())
            })
        );
        assert_eq!(
            cmd("$ZERON_CLI chat spawn --prompt hi --worktree --base main"),
            Some(ZeronChatCommand {
                verb: ZeronChatVerb::Spawn,
                targets: vec![],
                label: Some("hi".into()),
                title: None,
                harness: None,
                model: None
            })
        );
        assert_eq!(
            cmd("\"$ZERON_CLI\" chat tell 3f6b2a18 \"done?\""),
            Some(ZeronChatCommand {
                verb: ZeronChatVerb::Tell,
                targets: vec!["3f6b2a18".into()],
                label: None,
                title: None,
                harness: None,
                model: None
            })
        );
        assert_eq!(
            cmd("/home/u/.zeron/bin/zeron chat output --json 3f6b2a18"),
            Some(ZeronChatCommand {
                verb: ZeronChatVerb::Output,
                targets: vec!["3f6b2a18".into()],
                label: None,
                title: None,
                harness: None,
                model: None
            })
        );
        // `read` is `output`'s alias — the same chip; `log`/`transcript`/
        // `history`/`show`/`list` stay ordinary commands.
        assert_eq!(
            cmd("zeron chat read 3f6b2a18"),
            Some(ZeronChatCommand {
                verb: ZeronChatVerb::Output,
                targets: vec!["3f6b2a18".into()],
                label: None,
                title: None,
                harness: None,
                model: None
            })
        );
        for alias in ["log", "transcript", "history", "show", "list"] {
            assert_eq!(
                cmd(&format!("zeron chat {alias} 3f6b2a18")),
                None,
                "{alias}"
            );
        }
        // `--title` is remembered apart from the label fallback so a spawn
        // chip can find its child chat by name later.
        assert_eq!(
            cmd("zeron chat spawn --title \"Audit the parser\" --prompt go"),
            Some(ZeronChatCommand {
                verb: ZeronChatVerb::Spawn,
                targets: vec![],
                label: Some("Audit the parser".into()),
                title: Some("Audit the parser".into()),
                harness: None,
                model: None
            })
        );
    }

    #[test]
    fn prefixes_and_wrappers_still_classify() {
        assert_eq!(
            cmd("cd /repo && zeron chat tell main --message-file ./note.md"),
            Some(ZeronChatCommand {
                verb: ZeronChatVerb::Tell,
                targets: vec!["main".into()],
                label: None,
                title: None,
                harness: None,
                model: None
            })
        );
        assert_eq!(
            cmd("ZERON_CHAT_ID=abc zeron chat wait 3f6b2a18 --timeout 90s"),
            Some(ZeronChatCommand {
                verb: ZeronChatVerb::Wait,
                targets: vec!["3f6b2a18".into()],
                label: None,
                title: None,
                harness: None,
                model: None
            })
        );
        assert_eq!(
            cmd("env FOO=1 bash -lc 'zeron chat wait a b --any'"),
            Some(ZeronChatCommand {
                verb: ZeronChatVerb::Wait,
                targets: vec!["a".into(), "b".into()],
                label: None,
                title: None,
                harness: None,
                model: None
            })
        );
        assert_eq!(
            cmd("sh -c \"zeron chat output 'My title'\""),
            Some(ZeronChatCommand {
                verb: ZeronChatVerb::Output,
                targets: vec!["My title".into()],
                label: None,
                title: None,
                harness: None,
                model: None
            })
        );
        // A chained tail command does not become a target.
        assert_eq!(
            cmd("zeron chat spawn --prompt go && echo done"),
            Some(ZeronChatCommand {
                verb: ZeronChatVerb::Spawn,
                targets: vec![],
                label: Some("go".into()),
                title: None,
                harness: None,
                model: None
            })
        );
    }

    #[test]
    fn flags_never_become_targets() {
        // `--mode steer` and friends consume their values; booleans don't.
        // `tell`'s free-text message is not a target.
        assert_eq!(
            cmd("zeron chat tell 3f6b2a18 --mode steer hello there"),
            Some(ZeronChatCommand {
                verb: ZeronChatVerb::Tell,
                targets: vec!["3f6b2a18".into()],
                label: None,
                title: None,
                harness: None,
                model: None
            })
        );
        assert_eq!(
            cmd("zeron chat wait --any a --timeout=5m b"),
            Some(ZeronChatCommand {
                verb: ZeronChatVerb::Wait,
                targets: vec!["a".into(), "b".into()],
                label: None,
                title: None,
                harness: None,
                model: None
            })
        );
        assert_eq!(
            cmd(&format!("zeron chat tell {CHILD} --wait --timeout 1h")),
            Some(ZeronChatCommand {
                verb: ZeronChatVerb::Tell,
                targets: vec![CHILD.into()],
                label: None,
                title: None,
                harness: None,
                model: None
            })
        );
        // `self` is a real target.
        assert_eq!(
            cmd("zeron chat tell self hi"),
            Some(ZeronChatCommand {
                verb: ZeronChatVerb::Tell,
                targets: vec!["self".into()],
                label: None,
                title: None,
                harness: None,
                model: None
            })
        );
    }

    #[test]
    fn read_aliases_to_output() {
        // The CLI's `read` is the same verb as `output`: the transcript must
        // render it as the Read chip, not a generic exec row.
        assert_eq!(
            cmd(&format!("zeron chat read {CHILD}")),
            Some(ZeronChatCommand {
                verb: ZeronChatVerb::Output,
                targets: vec![CHILD.into()],
                label: None,
                title: None,
                harness: None,
                model: None
            })
        );
        assert_eq!(
            cmd("zeron chat read 'My title' --tail 20"),
            Some(ZeronChatCommand {
                verb: ZeronChatVerb::Output,
                targets: vec!["My title".into()],
                label: None,
                title: None,
                harness: None,
                model: None
            })
        );
        // The near-miss verbs stay ordinary commands.
        for verb in ["log", "transcript", "history", "show", "list"] {
            assert_eq!(
                cmd(&format!("zeron chat {verb} {CHILD}")),
                None,
                "verb {verb} must not classify"
            );
        }
    }

    #[test]
    fn negatives() {
        // Merely containing the words is not an invocation.
        assert_eq!(cmd("echo zeron chat spawn --prompt hi"), None);
        assert_eq!(cmd("grep \"zeron chat\" log.txt"), None);
        assert_eq!(cmd("zeronchat spawn x"), None);
        assert_eq!(cmd("zeron chat list --json"), None);
        assert_eq!(cmd("zeron mcp"), None);
        assert_eq!(cmd("zeron"), None);
        // Relative paths are not the CLI shim.
        assert_eq!(cmd("./zeron chat spawn --prompt hi"), None);
        assert_eq!(cmd("../bin/zeron chat spawn x"), None);
        // A bare `cd` prefix must join the real command with an operator.
        assert_eq!(cmd("cd /repo zeron chat spawn x"), None);
        assert_eq!(cmd("cd zeron chat spawn x"), None);
        // The flag value is not a verb: `--model spawn` can't spoof one.
        assert_eq!(cmd("zeron chat --model spawn"), None);
        assert_eq!(cmd(""), None);
    }
}
