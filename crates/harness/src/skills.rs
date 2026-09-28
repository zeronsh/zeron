//! Host-side discovery for providers without a typed skills catalog. Only
//! configured skill roots are traversed; bodies stay on the host until invoked.
use crate::HarnessError;
use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
};
use zeron_proto::{HarnessId, invocation::Skill};

const MAX_DISCOVERY_ENTRIES: usize = 4096;

/// Commands and skill metadata both need the provider's advertised catalog.
/// Share an overlapping probe, but refresh on a later open so provider account,
/// plugin and workspace changes are not hidden behind a persistent host cache.
#[derive(Default)]
pub(crate) struct CommandDiscovery {
    latest:
        tokio::sync::Mutex<Option<(PathBuf, std::time::Instant, Vec<zeron_proto::SlashCommand>)>>,
}

impl CommandDiscovery {
    pub(crate) async fn get(
        &self,
        cwd: &Path,
        discover: impl std::future::Future<
            Output = Result<Vec<zeron_proto::SlashCommand>, HarnessError>,
        >,
    ) -> Result<Vec<zeron_proto::SlashCommand>, HarnessError> {
        let requested = std::time::Instant::now();
        let mut latest = self.latest.lock().await;
        if let Some((root, completed, commands)) = latest.as_ref()
            && root == cwd
            && *completed >= requested
        {
            return Ok(commands.clone());
        }
        let commands = discover.await?;
        *latest = Some((cwd.to_owned(), std::time::Instant::now(), commands.clone()));
        Ok(commands)
    }
}

pub(crate) async fn discover(harness: HarnessId, cwd: &Path) -> Result<Vec<Skill>, HarnessError> {
    let cwd = cwd.to_path_buf();
    let home = crate::executable::home_or_current_dir();
    tokio::task::spawn_blocking(move || discover_at(harness, &cwd, &home))
        .await
        .map_err(|error| HarnessError::Protocol(error.to_string()))?
}

/// Shared Agent Skills do not establish a provider-native command identity.
pub(crate) fn is_shared_skill(path: &str) -> bool {
    Path::new(path).ancestors().any(|dir| {
        dir.file_name().is_some_and(|name| name == "skills")
            && dir
                .parent()
                .and_then(Path::file_name)
                .is_some_and(|name| name == ".agents")
    })
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
            // A shared skill named `compact` is not evidence that the
            // provider's built-in /compact invokes that file. Pi's explicit
            // skill: namespace supplies the classification other ACP catalogs lack.
            if harness != HarnessId::Pi && is_shared_skill(&skill.path) {
                continue;
            }
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
        HarnessId::Omp => &[".agents/skills", ".omp/skills"],
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
        HarnessId::Omp => roots.push((home.join(".omp/agent/skills"), String::new())),
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
    let mut remaining = MAX_DISCOVERY_ENTRIES;
    for (root, namespace) in roots {
        if remaining == 0 {
            break;
        }
        scan_root(&root, &namespace, harness, &mut found, &mut remaining)?;
    }
    Ok(found.into_values().collect())
}

fn scan_root(
    root: &Path,
    namespace: &str,
    harness: HarnessId,
    found: &mut BTreeMap<String, Skill>,
    remaining: &mut usize,
) -> Result<(), HarnessError> {
    let mut pending = vec![(root.to_path_buf(), 0usize)];
    let mut seen = HashSet::new();
    while let Some((dir, depth)) = pending.pop() {
        if *remaining == 0 {
            break;
        }
        if depth > 12 {
            continue;
        }
        let canonical = match std::fs::canonicalize(&dir) {
            Ok(path) => path,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                tracing::warn!(path = %dir.display(), %error, "skipping inaccessible skill directory");
                continue;
            }
        };
        if !seen.insert(canonical) {
            continue;
        }
        let children = match std::fs::read_dir(&dir) {
            Ok(children) => children,
            Err(error) => {
                tracing::warn!(path = %dir.display(), %error, "skipping unreadable skill directory");
                continue;
            }
        };
        let mut children: Vec<_> = children
            // Charge directory entries before filtering, allocation and sorting.
            // Failed entries also consume work; all roots share this budget.
            .take(*remaining)
            .inspect(|_| *remaining -= 1)
            .filter_map(|entry| match entry {
                Ok(entry) => Some(entry),
                Err(error) => {
                    tracing::warn!(path = %dir.display(), %error, "skipping unreadable skill directory entry");
                    None
                }
            })
            .collect();
        children.sort_by_key(|entry| entry.file_name());
        for entry in children {
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
            // One stale symlink or unreadable file must not discard the rest
            // of the global and project catalog.
            let skill = match read_skill(&path) {
                Ok(skill) => skill,
                Err(error) => {
                    tracing::warn!(path = %path.display(), %error, "skipping unreadable skill");
                    continue;
                }
            };
            if let Some(mut skill) = skill {
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
    // Keep regular-file symlinks working, but never open known devices/pipes.
    if !std::fs::metadata(path)?.is_file() {
        return Ok(None);
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // A regular file can be replaced with a FIFO after the metadata check.
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Ok(None);
    }
    // Bound reads even if the file grows between metadata and read.
    let mut bytes = Vec::new();
    file.take(256 * 1024 + 1).read_to_end(&mut bytes)?;
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

    #[tokio::test]
    async fn command_discovery_shares_overlap_but_refreshes_later_and_other_roots() {
        let discovery = CommandDiscovery::default();
        let probes = std::cell::Cell::new(0);
        let probe = || async {
            probes.set(probes.get() + 1);
            tokio::task::yield_now().await;
            Ok(vec![zeron_proto::SlashCommand {
                name: format!("probe-{}", probes.get()),
                description: String::new(),
                input_hint: None,
            }])
        };
        let root = Path::new("/workspace");
        let (commands, skills) =
            tokio::join!(discovery.get(root, probe()), discovery.get(root, probe()),);
        assert_eq!(commands.unwrap()[0].name, skills.unwrap()[0].name);
        assert_eq!(
            probes.get(),
            1,
            "overlapping callers share one provider process"
        );
        assert_eq!(
            discovery.get(root, probe()).await.unwrap()[0].name,
            "probe-2"
        );
        let (a, b) = tokio::join!(
            discovery.get(root, probe()),
            discovery.get(Path::new("/other-workspace"), probe()),
        );
        assert_ne!(a.unwrap()[0].name, b.unwrap()[0].name);
        assert_eq!(probes.get(), 4);
        assert!(
            discovery
                .get(root, async { Err(HarnessError::Protocol("retry".into())) })
                .await
                .is_err()
        );
        assert_eq!(
            discovery.get(root, probe()).await.unwrap()[0].name,
            "probe-5"
        );
    }

    fn write(root: &Path, relative: &str, body: &str) -> PathBuf {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn discovery_caps_large_directories_across_roots() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        write(&home, ".agents/skills/global.md", "Global");
        for index in 0..4200 {
            write(&repo, &format!(".pi/skills/skill-{index}.md"), "Body");
        }
        let skills = discover_at(HarnessId::Pi, &repo, &home).unwrap();
        assert_eq!(skills.len(), MAX_DISCOVERY_ENTRIES);
        assert!(skills.iter().any(|skill| skill.name == "global"));

        // Directories count too; pending nested work cannot restart the budget.
        let mut found = BTreeMap::new();
        let mut remaining = 1;
        scan_root(&repo, "", HarnessId::Pi, &mut found, &mut remaining).unwrap();
        assert_eq!(remaining, 0);
        assert!(found.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn discovery_skips_special_files_without_blocking() {
        const CHILD: &str = "ZERON_SKILL_SPECIAL_FILE_TEST";
        if std::env::var_os(CHILD).is_none() {
            // Isolate the probe so a regression cannot strand the test runner.
            let mut child = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "skills::tests::discovery_skips_special_files_without_blocking",
                    "--nocapture",
                ])
                .env(CHILD, "1")
                .spawn()
                .unwrap();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                if let Some(status) = child.try_wait().unwrap() {
                    assert!(status.success());
                    return;
                }
                if std::time::Instant::now() >= deadline {
                    child.kill().unwrap();
                    child.wait().unwrap();
                    panic!("skill discovery blocked on a special file");
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
        use std::os::unix::{fs::symlink, net::UnixListener};
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let valid = write(root, "valid.md", "Body");
        symlink(valid, root.join("linked.md")).unwrap();
        let fifo = root.join("pipe.md");
        let name = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        symlink(fifo, root.join("pipe-link.md")).unwrap();
        symlink("/dev/zero", root.join("device.md")).unwrap();
        let _socket = UnixListener::bind(root.join("socket.md")).unwrap();
        let mut found = BTreeMap::new();
        let mut remaining = MAX_DISCOVERY_ENTRIES;
        scan_root(root, "", HarnessId::Pi, &mut found, &mut remaining).unwrap();
        assert_eq!(
            found.keys().map(String::as_str).collect::<Vec<_>>(),
            ["linked", "valid"]
        );
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
    fn shared_acp_skills_do_not_alias_builtin_commands() {
        for harness in [
            HarnessId::Devin,
            HarnessId::Grok,
            HarnessId::Hermes,
            HarnessId::Antigravity,
            HarnessId::Pi,
        ] {
            let mut skills = vec![Skill {
                name: "compact".into(),
                path: "/repo/.agents/skills/compact/SKILL.md".into(),
                description: "Compact JSON fixtures".into(),
                enabled: true,
                command: None,
            }];
            let command = |name: &str| zeron_proto::SlashCommand {
                name: name.into(),
                description: String::new(),
                input_hint: None,
            };
            attach_advertised_commands(harness, &mut skills, &[command("compact")]);
            assert!(skills[0].command.is_none(), "{harness:?}");
            if harness == HarnessId::Pi {
                attach_advertised_commands(harness, &mut skills, &[command("skill:compact")]);
                assert_eq!(skills[0].command.as_ref().unwrap().name, "skill:compact");
            }
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
        let mut remaining = MAX_DISCOVERY_ENTRIES;
        scan_root(&root, "", HarnessId::ClaudeCode, &mut found, &mut remaining).unwrap();
        assert_eq!(
            found.keys().map(String::as_str).collect::<Vec<_>>(),
            ["good:review"]
        );
        found.clear();
        scan_root(
            &root,
            "bad namespace:",
            HarnessId::ClaudeCode,
            &mut found,
            &mut remaining,
        )
        .unwrap();
        assert!(found.is_empty());
        scan_root(
            &root,
            "é-plugin:",
            HarnessId::ClaudeCode,
            &mut found,
            &mut remaining,
        )
        .unwrap();
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
            let mut remaining = MAX_DISCOVERY_ENTRIES;
            scan_root(&root, "", harness, &mut found, &mut remaining).unwrap();
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
            (HarnessId::Omp, ".omp"),
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
    fn unreadable_directories_do_not_hide_other_skill_roots() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        write(&home, ".agents/skills/global/SKILL.md", "Global");
        write(&repo, ".agents/skills/project/SKILL.md", "Project");
        // Exercise both an inaccessible root and a nested directory; the
        // provider's other roots must survive.
        for denied in [
            home.join(".cursor/skills"),
            home.join(".agents/skills/private"),
        ] {
            std::fs::create_dir_all(&denied).unwrap();
            std::fs::set_permissions(&denied, std::fs::Permissions::from_mode(0)).unwrap();
            let result = discover_at(HarnessId::Cursor, &repo, &home);
            std::fs::set_permissions(&denied, std::fs::Permissions::from_mode(0o700)).unwrap();
            let skills = result.unwrap();
            assert!(skills.iter().any(|skill| skill.name == "global"));
            assert!(skills.iter().any(|skill| skill.name == "project"));
        }
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_skill_files_do_not_hide_global_or_project_skills() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        write(
            &home,
            ".agents/skills/global/SKILL.md",
            "Global instructions",
        );
        write(
            &repo,
            ".agents/skills/project/SKILL.md",
            "Project instructions",
        );

        let missing = home.join(".agents/skills/missing");
        std::fs::create_dir_all(&missing).unwrap();
        symlink(temp.path().join("gone.md"), missing.join("SKILL.md")).unwrap();
        let cycle = repo.join(".agents/skills/cycle");
        std::fs::create_dir_all(&cycle).unwrap();
        symlink("SKILL.md", cycle.join("SKILL.md")).unwrap();

        let denied = write(&repo, ".agents/skills/denied/SKILL.md", "Private");
        std::fs::set_permissions(&denied, std::fs::Permissions::from_mode(0)).unwrap();
        // Root can read mode-000 files; missing and cyclic links still exercise
        // per-file I/O failures on privileged test runners.
        let denied_is_readable = std::fs::File::open(&denied).is_ok();
        let result = discover_at(HarnessId::Cursor, &repo, &home);
        std::fs::set_permissions(&denied, std::fs::Permissions::from_mode(0o600)).unwrap();
        let skills = result.unwrap();
        let names: Vec<_> = skills.iter().map(|skill| skill.name.as_str()).collect();
        assert!(names.contains(&"global"));
        assert!(names.contains(&"project"));
        assert!(!names.contains(&"missing"));
        assert!(!names.contains(&"cycle"));
        assert_eq!(names.contains(&"denied"), denied_is_readable);
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
        let mut remaining = MAX_DISCOVERY_ENTRIES;
        scan_root(
            &root,
            "plugin:",
            HarnessId::ClaudeCode,
            &mut found,
            &mut remaining,
        )
        .unwrap();
        assert_eq!(found.len(), 1);
        assert!(found.contains_key("plugin:review"));
    }
}
