//! Agent presets on this device (docs/agent-presets.md): the user's own in
//! `{data_dir}/presets.json`, a project's in `<workspace>/.zeron/agents/*.md`,
//! and a project's Claude Code subagents (`.claude/agents/*.md`) offered
//! read-only. A project preset beats a user preset of the same id inside that
//! project; an imported one never overrides either.
//!
//! A project file is YAML-style frontmatter, then the instructions:
//!
//! ```text
//! ---
//! name: Reviewer
//! description: Reviews a diff and reports; never edits.
//! harness: codex
//! model: gpt-5
//! reasoning: high
//! mode: ask
//! sandbox: read-only
//! tools: read_chat, get_chat
//! may_spawn: false
//! ---
//! Only report what you find. Never edit a file.
//! ```
//!
//! Frontmatter is flat `key: value` lines (a list is comma separated or in
//! `[brackets]`); keys this build doesn't know are ignored.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use serde::{Deserialize, Serialize};
use zeron_proto::{
    AgentPreset, HarnessId, PermissionMode, PresetFallback, PresetSource, PresetTools,
    ReasoningLevel, SandboxMode,
};

pub const USER_PRESETS_FILE: &str = "presets.json";
pub const PROJECT_PRESETS_DIR: &str = ".zeron/agents";
pub const IMPORTED_PRESETS_DIR: &str = ".claude/agents";
/// A preset file bigger than this is ignored, so a stray file can't bloat
/// every system prompt.
const MAX_FILE_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct PresetsFile {
    presets: Vec<AgentPreset>,
}

/// The user's presets, read from disk on every use (the file is tiny), so a
/// hand edit applies at once.
#[derive(Debug, Default)]
pub struct Presets {
    /// `None` (bare tests) = no user presets and nothing persisted.
    user_path: Option<PathBuf>,
    write: Mutex<()>,
}

impl Presets {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            user_path: Some(data_dir.join(USER_PRESETS_FILE)),
            write: Mutex::new(()),
        }
    }

    pub fn user(&self) -> Vec<AgentPreset> {
        self.user_path
            .as_deref()
            .map(read_user)
            .unwrap_or_default()
    }

    /// Create or replace a user preset (by id).
    pub fn upsert(&self, mut preset: AgentPreset) -> Result<AgentPreset, String> {
        preset.name = preset.name.trim().to_string();
        if preset.name.is_empty() {
            return Err("A preset needs a name.".into());
        }
        if preset.id.trim().is_empty() {
            preset.id = zeron_proto::preset::slug(&preset.name);
        }
        if preset.id.is_empty() || preset.id != zeron_proto::preset::slug(&preset.id) {
            return Err("A preset's id can only use letters, digits and dashes.".into());
        }
        preset.source = PresetSource::User;
        let Some(path) = self.user_path.as_deref() else {
            return Ok(preset);
        };
        let _guard = self.write.lock().unwrap_or_else(PoisonError::into_inner);
        let mut file = PresetsFile {
            presets: read_user(path),
        };
        match file.presets.iter_mut().find(|p| p.id == preset.id) {
            Some(existing) => *existing = preset.clone(),
            None => file.presets.push(preset.clone()),
        }
        write_user(path, &file).map_err(|e| e.to_string())?;
        Ok(preset)
    }

    /// Returns whether a preset with that id was there.
    pub fn delete(&self, id: &str) -> Result<bool, String> {
        let Some(path) = self.user_path.as_deref() else {
            return Ok(false);
        };
        let _guard = self.write.lock().unwrap_or_else(PoisonError::into_inner);
        let mut file = PresetsFile {
            presets: read_user(path),
        };
        let before = file.presets.len();
        file.presets.retain(|p| p.id != id);
        if file.presets.len() == before {
            return Ok(false);
        }
        write_user(path, &file).map_err(|e| e.to_string())?;
        Ok(true)
    }

    /// Every preset offered in `workspace` (or just the user's): the
    /// project's own first, then its imported Claude Code subagents, then the
    /// user's, with a project preset replacing a user preset of the same id.
    pub fn list(&self, workspace: Option<&Path>) -> Vec<AgentPreset> {
        let mut out: Vec<AgentPreset> = Vec::new();
        if let Some(workspace) = workspace {
            out.extend(read_dir(
                &workspace.join(PROJECT_PRESETS_DIR),
                PresetSource::Project,
            ));
        }
        let mut imported = workspace
            .map(|w| read_dir(&w.join(IMPORTED_PRESETS_DIR), PresetSource::Imported))
            .unwrap_or_default();
        let user = self.user();
        let taken = |out: &[AgentPreset], id: &str| out.iter().any(|p| p.id == id);
        // Imported ones only take ids nothing else uses.
        imported.retain(|p| !user.iter().any(|u| u.id == p.id));
        for preset in imported.into_iter().chain(user) {
            if !taken(&out, &preset.id) {
                out.push(preset);
            }
        }
        out
    }

    /// A preset by id, or by name ignoring case.
    pub fn find(&self, workspace: Option<&Path>, key: &str) -> Option<AgentPreset> {
        let key = key.trim();
        let all = self.list(workspace);
        all.iter()
            .find(|p| p.id == key)
            .or_else(|| all.iter().find(|p| p.name.eq_ignore_ascii_case(key)))
            .cloned()
    }
}

fn read_user(path: &Path) -> Vec<AgentPreset> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    match serde_json::from_str::<PresetsFile>(&text) {
        Ok(file) => file
            .presets
            .into_iter()
            .map(|p| AgentPreset {
                source: PresetSource::User,
                ..p
            })
            .collect(),
        Err(err) => {
            tracing::warn!(path = %path.display(), error = %err, "presets unreadable; ignoring");
            Vec::new()
        }
    }
}

fn write_user(path: &Path, file: &PresetsFile) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_vec_pretty(file).map_err(std::io::Error::other)?;
    let tmp = path.with_extension(format!("json.{}.tmp", uuid::Uuid::new_v4()));
    std::fs::write(&tmp, json)
        .and_then(|()| std::fs::rename(&tmp, path))
        .inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })
}

fn read_dir(dir: &Path, source: PresetSource) -> Vec<AgentPreset> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "md"))
        .collect();
    files.sort();
    files
        .into_iter()
        .filter_map(|path| {
            let meta = std::fs::metadata(&path).ok()?;
            if !meta.is_file() || meta.len() > MAX_FILE_BYTES {
                return None;
            }
            let text = std::fs::read_to_string(&path).ok()?;
            let stem = path.file_stem()?.to_str()?;
            parse_markdown(&text, stem, source)
        })
        .collect()
}

/// A preset from a markdown file's text; `None` when it has no usable
/// frontmatter or no name. `stem` (the file name) is the default id.
pub fn parse_markdown(text: &str, stem: &str, source: PresetSource) -> Option<AgentPreset> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let rest = text.strip_prefix("---")?.trim_start_matches(['\r', ' ', '\t']);
    let rest = rest.strip_prefix('\n')?;
    let end = rest
        .lines()
        .scan(0usize, |at, line| {
            let start = *at;
            *at += line.len() + 1;
            Some((start, line))
        })
        .find(|(_, line)| line.trim_end() == "---")?;
    let (front, body) = (&rest[..end.0], rest[end.0 + 4..].trim());

    let mut fields: Vec<(String, String)> = Vec::new();
    for line in front.lines() {
        let line = line.trim_end();
        if line.trim_start().starts_with('#') || line.trim().is_empty() {
            continue;
        }
        if let Some((key, value)) = line.split_once(':')
            && !key.starts_with(char::is_whitespace)
        {
            fields.push((key.trim().to_ascii_lowercase().replace('-', "_"), unquote(value)));
        }
    }
    let get = |key: &str| {
        fields
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
            .filter(|v| !v.is_empty())
    };

    let name = get("name")?;
    let mut preset = AgentPreset::new(name, HarnessId::ClaudeCode);
    preset.id = zeron_proto::preset::slug(stem);
    if preset.id.is_empty() {
        preset.id = zeron_proto::preset::slug(name);
    }
    if preset.id.is_empty() {
        return None;
    }
    preset.source = source;
    preset.description = get("description").unwrap_or_default().to_string();
    preset.icon = get("icon").map(str::to_owned);
    preset.color = get("color").map(str::to_owned);
    preset.model = get("model").map(str::to_owned);
    if !body.is_empty() {
        preset.instructions = Some(body.to_string());
    }
    if source == PresetSource::Imported {
        // A Claude Code subagent file: its own tool names are Claude's, not
        // Zeron's, so only the identity, model and prompt carry over.
        return Some(preset);
    }
    if let Some(harness) = get("harness").and_then(harness_from) {
        preset.harness = harness;
    }
    preset.reasoning = get("reasoning").and_then(|v| enum_from::<ReasoningLevel>(v));
    if let Some(mode) = get("mode").and_then(mode_from) {
        preset.policy.mode = mode;
    }
    if let Some(sandbox) = get("sandbox").and_then(sandbox_from) {
        preset.policy.sandbox = sandbox;
    }
    if let Some(network) = get("network").and_then(bool_from) {
        preset.policy.network = network;
    }
    preset.worktree = get("worktree").and_then(bool_from).unwrap_or(false);
    preset.may_spawn = get("may_spawn").and_then(bool_from).unwrap_or(true);
    preset.tools = PresetTools {
        allow: get("tools").map(list_from).unwrap_or_default(),
        deny: get("deny").map(list_from).unwrap_or_default(),
    };
    preset.fallbacks = get("fallbacks")
        .map(list_from)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|entry| {
            let (harness, model) = match entry.split_once('/') {
                Some((h, m)) => (h, Some(m.trim().to_string())),
                None => (entry.as_str(), None),
            };
            Some(PresetFallback {
                harness: harness_from(harness.trim())?,
                model: model.filter(|m| !m.is_empty()),
            })
        })
        .collect();
    Some(preset)
}

fn unquote(value: &str) -> String {
    let value = value.trim();
    for quote in ['"', '\''] {
        if let Some(inner) = value
            .strip_prefix(quote)
            .and_then(|v| v.strip_suffix(quote))
        {
            return inner.to_string();
        }
    }
    value.to_string()
}

fn list_from(value: &str) -> Vec<String> {
    value
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .split(',')
        .map(unquote)
        .filter(|item| !item.is_empty())
        .collect()
}

fn bool_from(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" => Some(true),
        "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn normalized(value: &str) -> String {
    value
        .trim()
        .chars()
        .filter(|c| !matches!(c, '-' | '_' | ' '))
        .collect::<String>()
        .to_ascii_lowercase()
}

fn mode_from(value: &str) -> Option<PermissionMode> {
    match normalized(value).as_str() {
        "bypass" | "bypasspermissions" => Some(PermissionMode::Bypass),
        "auto" => Some(PermissionMode::Auto),
        "acceptedits" => Some(PermissionMode::AcceptEdits),
        "ask" => Some(PermissionMode::Ask),
        "plan" => Some(PermissionMode::Plan),
        _ => None,
    }
}

fn sandbox_from(value: &str) -> Option<SandboxMode> {
    match normalized(value).as_str() {
        "off" | "none" | "nosandbox" => Some(SandboxMode::Off),
        "workspacewrite" | "workspace" => Some(SandboxMode::WorkspaceWrite),
        "readonly" => Some(SandboxMode::ReadOnly),
        _ => None,
    }
}

fn enum_from<T: serde::de::DeserializeOwned>(value: &str) -> Option<T> {
    serde_json::from_value(serde_json::Value::String(
        value.trim().to_ascii_lowercase(),
    ))
    .ok()
}

fn harness_from(value: &str) -> Option<HarnessId> {
    enum_from(&value.trim().replace('_', "-"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const REVIEWER: &str = "---\nname: Reviewer\ndescription: \"Reviews a diff; never edits.\"\nharness: codex\nmodel: gpt-5\nreasoning: high\nmode: accept-edits\nsandbox: read-only\nnetwork: false\ntools: [read_chat, get_chat]\ndeny: get_chat\nmay_spawn: no\nworktree: yes\nfallbacks: claude-code/sonnet, opencode\nunknown_key: ignored\n---\n\nOnly report what you find.\nNever edit a file.\n";

    #[test]
    fn a_project_file_becomes_a_preset() {
        let p = parse_markdown(REVIEWER, "code-reviewer", PresetSource::Project).unwrap();
        assert_eq!((p.id.as_str(), p.name.as_str()), ("code-reviewer", "Reviewer"));
        assert_eq!(p.description, "Reviews a diff; never edits.");
        assert_eq!(p.harness, HarnessId::Codex);
        assert_eq!(p.model.as_deref(), Some("gpt-5"));
        assert_eq!(p.reasoning, Some(ReasoningLevel::High));
        assert_eq!(p.policy.mode, PermissionMode::AcceptEdits);
        assert_eq!(p.policy.sandbox, SandboxMode::ReadOnly);
        assert!(!p.policy.network);
        assert_eq!(p.tools.allow, vec!["read_chat", "get_chat"]);
        assert_eq!(p.tools.deny, vec!["get_chat"]);
        assert!(!p.may_spawn && p.worktree);
        assert_eq!(
            p.fallbacks,
            vec![
                PresetFallback {
                    harness: HarnessId::ClaudeCode,
                    model: Some("sonnet".into())
                },
                PresetFallback {
                    harness: HarnessId::Opencode,
                    model: None
                },
            ]
        );
        assert_eq!(
            p.instructions.as_deref(),
            Some("Only report what you find.\nNever edit a file.")
        );
        assert_eq!(p.source, PresetSource::Project);
    }

    #[test]
    fn files_without_usable_frontmatter_are_not_presets() {
        assert!(parse_markdown("# just notes", "n", PresetSource::Project).is_none());
        assert!(parse_markdown("---\nname: x\nno end", "n", PresetSource::Project).is_none());
        assert!(parse_markdown("---\ndescription: no name\n---\nbody", "n", PresetSource::Project).is_none());
    }

    #[test]
    fn a_claude_subagent_keeps_its_identity_model_and_prompt_only() {
        let text = "---\nname: debugger\ndescription: Debugs failures\ntools: Read, Grep, Bash\nmodel: sonnet\n---\nYou are an expert debugger.\n";
        let p = parse_markdown(text, "debugger", PresetSource::Imported).unwrap();
        assert_eq!(p.harness, HarnessId::ClaudeCode);
        assert_eq!(p.model.as_deref(), Some("sonnet"));
        assert!(p.tools.is_empty(), "Claude's tool names aren't Zeron's");
        assert_eq!(p.instructions.as_deref(), Some("You are an expert debugger."));
        assert_eq!(p.source, PresetSource::Imported);
    }

    #[test]
    fn user_presets_round_trip_and_validate() {
        let data = tempfile::tempdir().unwrap();
        let presets = Presets::new(data.path());
        assert!(presets.upsert(AgentPreset::new("  ", HarnessId::Codex)).is_err());
        let mut bad = AgentPreset::new("Fixer", HarnessId::Codex);
        bad.id = "Not A Slug".into();
        assert!(presets.upsert(bad).is_err());
        let saved = presets
            .upsert(AgentPreset::new("Fast fixer", HarnessId::ClaudeCode))
            .unwrap();
        assert_eq!(saved.id, "fast-fixer");
        let mut edited = saved.clone();
        edited.policy.mode = PermissionMode::Auto;
        presets.upsert(edited).unwrap();
        let all = presets.user();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].policy.mode, PermissionMode::Auto);
        assert!(presets.delete("fast-fixer").unwrap());
        assert!(!presets.delete("fast-fixer").unwrap());
        assert!(presets.user().is_empty());
    }

    #[test]
    fn a_project_preset_wins_inside_the_project_and_imports_never_override() {
        let data = tempfile::tempdir().unwrap();
        let ws = tempfile::tempdir().unwrap();
        let presets = Presets::new(data.path());
        let mut mine = AgentPreset::new("Reviewer", HarnessId::Codex);
        mine.policy.mode = PermissionMode::Ask;
        presets.upsert(mine).unwrap();
        presets
            .upsert(AgentPreset::new("Debugger", HarnessId::Codex))
            .unwrap();
        let project = ws.path().join(PROJECT_PRESETS_DIR);
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join("reviewer.md"), REVIEWER).unwrap();
        let imported = ws.path().join(IMPORTED_PRESETS_DIR);
        std::fs::create_dir_all(&imported).unwrap();
        std::fs::write(
            imported.join("debugger.md"),
            "---\nname: debugger\n---\nImported prompt",
        )
        .unwrap();
        std::fs::write(imported.join("tester.md"), "---\nname: tester\n---\nRun tests").unwrap();

        let inside: Vec<_> = presets
            .list(Some(ws.path()))
            .into_iter()
            .map(|p| (p.id, p.source))
            .collect();
        assert_eq!(
            inside,
            vec![
                ("reviewer".to_string(), PresetSource::Project),
                ("tester".to_string(), PresetSource::Imported),
                ("debugger".to_string(), PresetSource::User),
            ],
            "the user's debugger beats the imported one"
        );
        // Outside the project only the user's presets exist.
        let outside: Vec<_> = presets.list(None).into_iter().map(|p| p.id).collect();
        assert_eq!(outside, vec!["reviewer", "debugger"]);
        // Lookup by id or name.
        assert_eq!(
            presets.find(Some(ws.path()), "REVIEWER").unwrap().source,
            PresetSource::Project
        );
        assert!(presets.find(None, "nope").is_none());
    }
}
