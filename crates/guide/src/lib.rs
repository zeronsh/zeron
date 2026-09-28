//! zeron-guide — the instructions, `zeron guide` chapters and the staged
//! skill bundle injected into chat runs so agents can orchestrate through
//! the `zeron` CLI. No dependencies; all content is compiled in via
//! `include_str!`.

/// Short system instructions stamped onto every chat run (the
/// `system_instructions` block on harnesses without a system-prompt
/// channel).
pub fn instructions() -> &'static str {
    include_str!("../content/instructions.md")
}

/// One `zeron guide <name>` chapter.
pub struct Chapter {
    pub name: &'static str,
    /// One line shown in [`overview`]'s chapter list.
    pub summary: &'static str,
    pub body: &'static str,
}

static CHAPTERS: &[Chapter] = &[
    Chapter {
        name: "chats",
        summary: "spawn, message, wait on, read and manage chats",
        body: include_str!("../content/chats.md"),
    },
    Chapter {
        name: "environments",
        summary: "projects, devices, worktrees and spawn limits",
        body: include_str!("../content/environments.md"),
    },
    Chapter {
        name: "harnesses",
        summary: "harness and model selection and inheritance",
        body: include_str!("../content/harnesses.md"),
    },
    Chapter {
        name: "notifications",
        summary: "what the parent is told about its children, and when",
        body: include_str!("../content/notifications.md"),
    },
    Chapter {
        name: "mentions",
        summary: "@chat:<id> live links",
        body: include_str!("../content/mentions.md"),
    },
    Chapter {
        name: "json",
        summary: "--json output and exit codes",
        body: include_str!("../content/json.md"),
    },
];

pub fn chapters() -> &'static [Chapter] {
    CHAPTERS
}

pub fn chapter(name: &str) -> Option<&'static Chapter> {
    CHAPTERS.iter().find(|c| c.name == name)
}

/// What bare `zeron guide` prints: a short intro plus the chapter list.
pub fn overview() -> String {
    let mut out = String::from(
        "Zeron lets a chat spawn, message and read child chats through the `zeron` CLI.\n\nChapters:\n",
    );
    for chapter in CHAPTERS {
        out.push_str(&format!("  {:<15} {}\n", chapter.name, chapter.summary));
    }
    out.push_str("\nRun `zeron guide <chapter>` for details.");
    out
}

/// One file of the staged skill bundle, `path` relative to the bundle root.
pub struct BundleFile {
    pub path: &'static str,
    pub contents: &'static str,
}

/// The plugin manifest, generated so its version always matches the crate.
const PLUGIN_JSON: &str = concat!(
    r#"{"name":"zeron","version":""#,
    env!("CARGO_PKG_VERSION"),
    r#"","description":"Zeron orchestration skills","author":{"name":"Zeron"},"skills":"./skills"}"#
);

/// The staged skill bundle, laid out as a Claude Code plugin:
/// `{bundle}/.claude-plugin/plugin.json` + `{bundle}/skills/<name>/SKILL.md`.
pub fn bundle_files() -> &'static [BundleFile] {
    BUNDLE_FILES
}

static BUNDLE_FILES: &[BundleFile] = &[
    BundleFile {
        path: ".claude-plugin/plugin.json",
        contents: PLUGIN_JSON,
    },
    BundleFile {
        path: "skills/zeron-cli/SKILL.md",
        contents: include_str!("../content/skills/zeron-cli/SKILL.md"),
    },
    BundleFile {
        path: "skills/zeron-cli/references/commands.md",
        contents: include_str!("../content/skills/zeron-cli/references/commands.md"),
    },
    BundleFile {
        path: "skills/zeron-cli/references/patterns.md",
        contents: include_str!("../content/skills/zeron-cli/references/patterns.md"),
    },
];

/// A skill inside the bundle, for harnesses that can only list skills as
/// text. `path` is the SKILL.md relative to the bundle root; `description`
/// is identical to the SKILL.md frontmatter.
pub struct SkillInfo {
    pub name: &'static str,
    pub description: &'static str,
    pub path: &'static str,
}

pub fn skills() -> &'static [SkillInfo] {
    &[SkillInfo {
        name: "zeron-cli",
        description: "Use when you need to delegate work to child chats, run agents in parallel, or coordinate with and read other chats in Zeron.",
        path: "skills/zeron-cli/SKILL.md",
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instructions_is_the_injected_block() {
        assert_eq!(
            instructions(),
            "You are working inside Zeron, a native control plane for coding agents. The `zeron` CLI is available when you need Zeron context or orchestration.\n\n- Prefer bare `zeron` on PATH; `$ZERON_CLI` is the absolute binary.\n- Run `zeron chat show self` to see your own chat, project and harness.\n- Run `zeron guide` for concepts and `zeron guide <chapter>` for command details.\n- Use `zeron chat ...` to spawn, message, wait for and read other chats. Do not spawn chats or message other chats unless the user has explicitly asked you to.\n- After spawning, let child chats work. Zeron notifies you when a child finishes, fails, is interrupted or needs help; do not poll with sleeps or repeated status reads. Use `zeron chat wait` only when you need a result before continuing.\n- Reference a chat as `@chat:<full-id>` so Zeron renders it as a live link. Do not construct URLs for chats.\n- Use Markdown links for files and URLs you want the user to open."
        );
    }

    #[test]
    fn every_chapter_is_non_empty_and_listed_in_overview() {
        let overview = overview();
        assert!(overview.contains("zeron guide <chapter>"));
        for chapter in chapters() {
            assert!(!chapter.body.trim().is_empty(), "{}", chapter.name);
            assert!(!chapter.summary.is_empty(), "{}", chapter.name);
            assert!(overview.contains(chapter.name), "{}", chapter.name);
            assert_eq!(
                super::chapter(chapter.name).map(|c| c.name),
                Some(chapter.name)
            );
        }
        assert!(super::chapter("nope").is_none());
    }

    #[test]
    fn skill_info_matches_the_skill_frontmatter() {
        let file = bundle_files()
            .iter()
            .find(|f| f.path == "skills/zeron-cli/SKILL.md")
            .expect("bundle carries the SKILL.md");
        let frontmatter = file
            .contents
            .strip_prefix("---\n")
            .and_then(|c| c.split("\n---\n").next())
            .expect("SKILL.md has YAML frontmatter");
        for skill in skills() {
            assert!(frontmatter.contains(&format!("name: {}", skill.name)));
            assert!(frontmatter.contains(&format!("description: {}", skill.description)));
            assert!(bundle_files().iter().any(|f| f.path == skill.path));
        }
    }

    #[test]
    fn bundle_contains_every_linked_reference() {
        let skill = bundle_files()
            .iter()
            .find(|f| f.path == "skills/zeron-cli/SKILL.md")
            .unwrap();
        // Links like [x](references/commands.md) resolve under the skill dir.
        for cap in skill.contents.split("](references/").skip(1) {
            let rel = cap.split(')').next().unwrap();
            let path = format!("skills/zeron-cli/references/{rel}");
            assert!(
                bundle_files().iter().any(|f| f.path == path),
                "missing {path}"
            );
        }
    }

    #[test]
    fn plugin_manifest_is_valid_json_named_zeron() {
        let manifest = bundle_files()
            .iter()
            .find(|f| f.path == ".claude-plugin/plugin.json")
            .expect("bundle carries the plugin manifest");
        let json: serde_json::Value = serde_json::from_str(manifest.contents).unwrap();
        assert_eq!(json["name"], "zeron");
        assert_eq!(json["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(json["skills"], "./skills");
    }
}
