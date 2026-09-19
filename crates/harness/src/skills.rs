//! Host-side discovery for providers without a typed skills catalog. Only
//! configured skill roots are traversed; bodies stay on the host until invoked.
use crate::HarnessError;
use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
};
use zeron_proto::{HarnessId, invocation::Skill};

pub(crate) async fn discover(harness: HarnessId, cwd: &Path) -> Result<Vec<Skill>, HarnessError> {
    let cwd = cwd.to_path_buf();
    let home = crate::executable::home_or_current_dir();
    tokio::task::spawn_blocking(move || discover_at(harness, &cwd, &home))
        .await
        .map_err(|error| HarnessError::Protocol(error.to_string()))?
}

/// ACP has no standard skill catalog. Pi labels its skill commands explicitly;
/// other agents can bind a discovered skill only to a command they advertised.
pub(crate) fn attach_advertised_commands(
    harness: HarnessId,
    skills: &mut Vec<Skill>,
    commands: &[zeron_proto::SlashCommand],
) {
    use zeron_proto::invocation::SkillCommand;
    for command in commands {
        if !zeron_proto::invocation::valid_skill_command_name(&command.name) {
            continue;
        }
        let name = if harness == HarnessId::Pi {
            let Some(name) = command.name.strip_prefix("skill:") else {
                continue;
            };
            name
        } else {
            command.name.as_str()
        };
        if let Some(skill) = skills.iter_mut().find(|skill| skill.name == name) {
            skill.command = Some(SkillCommand {
                name: command.name.clone(),
                harness,
            });
        } else if harness == HarnessId::Pi && !name.is_empty() {
            skills.push(Skill {
                name: name.into(),
                path: format!("harness-skill:pi:{name}"),
                description: command.description.clone(),
                enabled: true,
                command: Some(SkillCommand {
                    name: command.name.clone(),
                    harness,
                }),
            });
        }
    }
}

fn project_dirs(harness: HarnessId) -> &'static [&'static str] {
    match harness {
        HarnessId::ClaudeCode => &[".agents/skills", ".claude/commands", ".claude/skills"],
        HarnessId::Cursor => &[
            ".claude/skills",
            ".codex/skills",
            ".agents/skills",
            ".cursor/skills",
        ],
        HarnessId::Opencode => &[".claude/skills", ".agents/skills", ".opencode/skills"],
        HarnessId::Grok => &[".agents/skills", ".claude/skills", ".grok/skills"],
        HarnessId::Hermes => &[".agents/skills"],
        HarnessId::Pi => &[".agents/skills", ".pi/skills"],
        HarnessId::Devin => &[".agents/skills"],
        HarnessId::Antigravity => &[".agents/skills", ".gemini/skills"],
        HarnessId::Codex => &[".agents/skills", ".codex/skills"],
        HarnessId::Mock => &[],
    }
}

fn discover_at(harness: HarnessId, cwd: &Path, home: &Path) -> Result<Vec<Skill>, HarnessError> {
    let mut roots: Vec<(PathBuf, String)> = project_dirs(harness)
        .iter()
        .map(|dir| (home.join(dir), String::new()))
        .collect();
    match harness {
        HarnessId::Antigravity => {
            roots.extend(
                crate::acp::antigravity_skill_dirs()
                    .into_iter()
                    .map(|path| (path, String::new())),
            );
        }
        HarnessId::Pi => roots.push((
            std::env::var_os("PI_CODING_AGENT_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".pi/agent"))
                .join("skills"),
            String::new(),
        )),
        HarnessId::Hermes => roots.push((
            std::env::var_os("HERMES_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".hermes"))
                .join("skills"),
            String::new(),
        )),
        HarnessId::Opencode => roots.push((
            std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".config"))
                .join("opencode/skills"),
            String::new(),
        )),
        HarnessId::ClaudeCode => {
            let config = std::env::var_os("CLAUDE_CONFIG_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".claude"));
            roots.push((config.join("commands"), String::new()));
            roots.push((config.join("skills"), String::new()));
            // Only installed plugin locations; never crawl caches or marketplaces.
            if let Ok(bytes) = std::fs::read(config.join("plugins/installed_plugins.json")) {
                if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                    if let Some(plugins) = value.get("plugins").and_then(|v| v.as_object()) {
                        for (id, installs) in plugins {
                            let namespace = id.split('@').next().unwrap_or(id);
                            for install in installs.as_array().into_iter().flatten() {
                                if let Some(project) =
                                    install.get("projectPath").and_then(|v| v.as_str())
                                {
                                    if !cwd.starts_with(project) {
                                        continue;
                                    }
                                }
                                if let Some(path) =
                                    install.get("installPath").and_then(|v| v.as_str())
                                {
                                    roots.push((
                                        Path::new(path).join("skills"),
                                        format!("{namespace}:"),
                                    ));
                                    roots.push((
                                        Path::new(path).join("commands"),
                                        format!("{namespace}:"),
                                    ));
                                }
                            }
                        }
                    }
                }
            }
        }
        _ => {}
    }
    // Project scope stops at the nearest worktree root; nested directories
    // override ancestors, and project skills override global skills by name.
    let mut ancestors = Vec::new();
    for dir in cwd.ancestors() {
        ancestors.push(dir);
        if dir.join(".git").exists() || dir == home {
            break;
        }
    }
    for dir in ancestors.into_iter().rev() {
        for suffix in project_dirs(harness) {
            roots.push((dir.join(suffix), String::new()));
        }
    }
    let mut found = BTreeMap::new();
    for (root, namespace) in roots {
        scan_root(&root, &namespace, harness, &mut found)?;
    }
    Ok(found.into_values().collect())
}

fn scan_root(
    root: &Path,
    namespace: &str,
    harness: HarnessId,
    found: &mut BTreeMap<String, Skill>,
) -> Result<(), HarnessError> {
    let mut pending = vec![(root.to_path_buf(), 0usize)];
    let mut seen = HashSet::new();
    let mut entries = 0;
    while let Some((dir, depth)) = pending.pop() {
        if entries >= 4096 {
            break;
        }
        if depth > 12 {
            continue;
        }
        let canonical = match std::fs::canonicalize(&dir) {
            Ok(path) => path,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        if !seen.insert(canonical) {
            continue;
        }
        let mut children = std::fs::read_dir(&dir)?.collect::<Result<Vec<_>, _>>()?;
        children.sort_by_key(|entry| entry.file_name());
        for entry in children {
            entries += 1;
            let path = entry.path();
            if path.is_dir() {
                pending.push((path, depth + 1));
                continue;
            }
            let legacy_commands = root.file_name().is_some_and(|name| name == "commands");
            let flat = legacy_commands
                || (matches!(harness, HarnessId::Opencode | HarnessId::Pi) && depth == 0);
            if entry.file_name() != "SKILL.md"
                && !(flat && path.extension().is_some_and(|ext| ext == "md"))
            {
                continue;
            }
            if let Some(mut skill) = read_skill(&path)? {
                if legacy_commands {
                    let relative = path.strip_prefix(root).unwrap().with_extension("");
                    skill.name = relative.to_string_lossy().replace(['/', '\\'], ":");
                }
                skill.name = format!("{namespace}{}", skill.name);
                if zeron_proto::invocation::valid_invocation_name(&skill.name) {
                    found.insert(skill.name.clone(), skill);
                }
            }
        }
    }
    Ok(())
}

fn read_skill(path: &Path) -> Result<Option<Skill>, HarnessError> {
    use std::io::Read;
    // Bound reads even if the file grows between metadata and read.
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(256 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 256 * 1024 {
        return Ok(None);
    }
    // A malformed skill must not hide every other completion. Decode only
    // after checking the byte bound, which can end inside a UTF-8 character.
    let Ok(text) = std::str::from_utf8(&bytes) else {
        return Ok(None);
    };
    let mut lines = text.lines();
    let metadata = if lines.next().is_some_and(|line| line.trim() == "---") {
        let mut yaml = String::new();
        let mut closed = false;
        for line in lines {
            if line.trim() == "---" {
                closed = true;
                break;
            }
            yaml.push_str(line);
            yaml.push('\n');
        }
        if !closed {
            return Ok(None);
        }
        match serde_yaml_ng::from_str::<serde_json::Value>(&yaml) {
            Ok(value) => value,
            Err(_) => return Ok(None),
        }
    } else {
        serde_json::Value::Null
    };
    let fallback = if path.file_name().is_some_and(|name| name == "SKILL.md") {
        path.parent().and_then(Path::file_name)
    } else {
        path.file_stem()
    }
    .and_then(|name| name.to_str())
    .unwrap_or_default();
    let name = metadata
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or(fallback);
    let path = path.to_string_lossy();
    if !zeron_proto::invocation::valid_invocation_name(name)
        || !zeron_proto::invocation::valid_skill_path(&path)
    {
        return Ok(None);
    }
    Ok(Some(Skill {
        name: name.into(),
        path: path.into_owned(),
        description: metadata
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim()
            .into(),
        enabled: metadata
            .get("user-invocable")
            .and_then(|v| v.as_bool())
            .unwrap_or(true),
        command: None,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn write(root: &Path, relative: &str, body: &str) -> PathBuf {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn acp_native_skill_commands_remain_distinct_from_builtin_commands() {
        let commands = vec![
            zeron_proto::SlashCommand {
                name: "skill:review".into(),
                description: "Review".into(),
                input_hint: None,
            },
            zeron_proto::SlashCommand {
                name: "compact".into(),
                description: String::new(),
                input_hint: None,
            },
        ];
        let mut skills = vec![];
        attach_advertised_commands(HarnessId::Pi, &mut skills, &commands);
        assert_eq!(skills.len(), 1);
        let skill = &skills[0];
        let invocation = zeron_proto::invocation::Invocation::Skill {
            name: skill.name.clone(),
            path: skill.path.clone(),
            command: skill.command.clone(),
        };
        assert_eq!(
            zeron_proto::invocation::harness_prompt(
                &format!("{} arguments", invocation.link()),
                HarnessId::Pi
            ),
            "/skill:review arguments"
        );
        for harness in [HarnessId::Devin, HarnessId::Grok, HarnessId::Hermes] {
            let mut skills = vec![Skill {
                name: "review".into(),
                path: "/repo/SKILL.md".into(),
                description: String::new(),
                enabled: true,
                command: None,
            }];
            let command = zeron_proto::SlashCommand {
                name: "review".into(),
                description: String::new(),
                input_hint: None,
            };
            attach_advertised_commands(harness, &mut skills, &[command]);
            assert_eq!(skills[0].command.as_ref().unwrap().harness, harness);
        }
    }

    #[test]
    fn advertised_commands_preserve_valid_skill_links() {
        use zeron_proto::invocation::{Invocation, invocation_links};
        let commands = [
            "skill:",
            "skill:two words",
            "skill:bad\nname",
            "skill:bad\0name",
            "skill:review[ui]",
            "skill:审查-é:ui.v2_test",
        ]
        .into_iter()
        .map(|name| zeron_proto::SlashCommand {
            name: name.into(),
            description: String::new(),
            input_hint: None,
        })
        .collect::<Vec<_>>();
        let mut skills = vec![];
        attach_advertised_commands(HarnessId::Pi, &mut skills, &commands);
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "审查-é:ui.v2_test");
        let skill = skills.pop().unwrap();
        let invocation = Invocation::Skill {
            name: skill.name,
            path: skill.path,
            command: skill.command,
        };
        assert_eq!(invocation_links(&invocation.link())[0].1, invocation);
    }

    #[test]
    fn discovery_rejects_invalid_names_and_control_paths() {
        let temp = tempfile::tempdir().unwrap();
        for name in [
            "two words",
            " padded",
            "padded ",
            "tab\tname",
            "non\u{a0}breaking",
        ] {
            let path = write(
                temp.path(),
                "invalid/SKILL.md",
                &format!(
                    "---\nname: {}\n---\nBody",
                    serde_json::to_string(name).unwrap()
                ),
            );
            assert!(read_skill(&path).unwrap().is_none(), "{name:?}");
        }
        #[cfg(unix)]
        {
            let path = write(
                temp.path(),
                "bad\npath/SKILL.md",
                "---\nname: valid\n---\nBody",
            );
            assert!(read_skill(&path).unwrap().is_none());
        }
        let path = write(
            temp.path(),
            "é skill/SKILL.md",
            "---\nname: '审查[ui]`'\n---\nBody",
        );
        let skill = read_skill(&path).unwrap().unwrap();
        assert_eq!(skill.name, "审查[ui]`");
        assert_eq!(skill.path, path.to_str().unwrap());
    }

    #[test]
    fn derived_legacy_names_and_plugin_namespaces_are_validated() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("commands");
        write(&root, "bad folder/review.md", "---\nname: valid\n---\nBody");
        write(&root, "good/review.md", "Body");
        let mut found = BTreeMap::new();
        scan_root(&root, "", HarnessId::ClaudeCode, &mut found).unwrap();
        assert_eq!(
            found.keys().map(String::as_str).collect::<Vec<_>>(),
            ["good:review"]
        );
        found.clear();
        scan_root(&root, "bad namespace:", HarnessId::ClaudeCode, &mut found).unwrap();
        assert!(found.is_empty());
        scan_root(&root, "é-plugin:", HarnessId::ClaudeCode, &mut found).unwrap();
        assert_eq!(
            found.keys().map(String::as_str).collect::<Vec<_>>(),
            ["é-plugin:good:review"]
        );
    }

    #[test]
    fn every_production_harness_can_discover_shared_project_skills() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        write(
            &repo,
            ".agents/skills/portable/SKILL.md",
            "---\nname: portable\ndescription: Shared skill\n---\nBody",
        );
        for harness in [
            HarnessId::ClaudeCode,
            HarnessId::Codex,
            HarnessId::Cursor,
            HarnessId::Devin,
            HarnessId::Grok,
            HarnessId::Hermes,
            HarnessId::Pi,
            HarnessId::Antigravity,
            HarnessId::Opencode,
        ] {
            let skills = discover_at(harness, &repo, &home).unwrap();
            assert!(
                skills.iter().any(|skill| skill.name == "portable"),
                "{harness:?}"
            );
        }
    }

    #[test]
    fn malformed_or_oversized_skill_files_do_not_hide_valid_completions() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("skills");
        write(&root, "valid/SKILL.md", "---\nname: valid\n---\nBody");
        let invalid = write(&root, "invalid/SKILL.md", "");
        std::fs::write(invalid, [0xff, 0xfe]).unwrap();
        // The bounded read ends partway through this last character.
        let oversized = format!("{}é", "x".repeat(256 * 1024));
        write(&root, "oversized/SKILL.md", &oversized);
        for harness in [
            HarnessId::ClaudeCode,
            HarnessId::Codex,
            HarnessId::Cursor,
            HarnessId::Devin,
            HarnessId::Grok,
            HarnessId::Hermes,
            HarnessId::Pi,
            HarnessId::Antigravity,
            HarnessId::Opencode,
        ] {
            let mut found = BTreeMap::new();
            scan_root(&root, "", harness, &mut found).unwrap();
            assert_eq!(
                found.keys().map(String::as_str).collect::<Vec<_>>(),
                ["valid"],
                "{harness:?}"
            );
        }
    }

    #[test]
    fn provider_roots_precedence_and_user_invocability_are_preserved() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        write(
            &home,
            ".agents/skills/review/SKILL.md",
            "---\nname: review\ndescription: Global\n---\nBody",
        );
        write(
            &repo,
            ".agents/skills/review/SKILL.md",
            "---\nname: review\ndescription: Project\n---\nBody",
        );
        for (harness, native) in [
            (HarnessId::ClaudeCode, ".claude"),
            (HarnessId::Cursor, ".cursor"),
            (HarnessId::Grok, ".grok"),
            (HarnessId::Pi, ".pi"),
            (HarnessId::Opencode, ".opencode"),
        ] {
            let native_path = write(
                &repo,
                &format!("{native}/skills/review/SKILL.md"),
                "---\nname: review\ndescription: >-\n  Native multiline\n  description\ndisable-model-invocation: true\n---\nBody",
            );
            let skills = discover_at(harness, &repo, &home).unwrap();
            let skill = skills.iter().find(|skill| skill.name == "review").unwrap();
            // Directory traversal uses native separators; the fixture path
            // may contain forward slashes on Windows. Compare path identity.
            assert_eq!(Path::new(&skill.path), native_path.as_path());
            assert_eq!(skill.description, "Native multiline description");
            assert!(skill.enabled, "model invocation is not user invocation");
        }
        write(
            &repo,
            ".agents/skills/hidden/SKILL.md",
            "---\nname: hidden\nuser-invocable: false\n---\nBody",
        );
        write(
            &repo,
            ".agents/skills/broken/SKILL.md",
            "---\nname: [broken\n---\nBody",
        );
        let skills = discover_at(HarnessId::Devin, &repo, &home).unwrap();
        assert_eq!(skills.len(), 2);
        assert!(
            !skills
                .iter()
                .find(|skill| skill.name == "hidden")
                .unwrap()
                .enabled
        );
        assert_eq!(
            skills
                .iter()
                .find(|skill| skill.name == "review")
                .unwrap()
                .description,
            "Project"
        );
    }

    #[test]
    fn nested_workspace_stops_at_git_boundary_and_legacy_commands_keep_namespaces() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        let nested = repo.join("packages/web");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(&nested).unwrap();
        write(temp.path(), ".agents/skills/outside/SKILL.md", "Body");
        write(&repo, ".agents/skills/root/SKILL.md", "Body");
        write(
            &nested,
            ".claude/commands/team/review.md",
            "---\ndescription: Legacy\n---\nBody",
        );
        let skills =
            discover_at(HarnessId::ClaudeCode, &nested, &temp.path().join("home")).unwrap();
        assert!(skills.iter().any(|skill| skill.name == "root"));
        assert!(skills.iter().any(|skill| skill.name == "team:review"));
        assert!(!skills.iter().any(|skill| skill.name == "outside"));
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_skill_directories_are_supported_without_following_cycles() {
        let temp = tempfile::tempdir().unwrap();
        let path = write(
            temp.path(),
            "source/review/SKILL.md",
            "---\nname: review\n---\nBody",
        );
        let root = temp.path().join("skills");
        std::fs::create_dir(&root).unwrap();
        std::os::unix::fs::symlink(path.parent().unwrap(), root.join("review")).unwrap();
        std::os::unix::fs::symlink(&root, root.join("cycle")).unwrap();
        let mut found = BTreeMap::new();
        scan_root(&root, "plugin:", HarnessId::ClaudeCode, &mut found).unwrap();
        assert_eq!(found.len(), 1);
        assert!(found.contains_key("plugin:review"));
    }
}
