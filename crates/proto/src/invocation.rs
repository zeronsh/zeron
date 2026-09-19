//! Durable composer references. Only canonical links created by completion are
//! decoded; ordinary slash/dollar text remains ordinary prompt text.
use serde::{Deserialize, Serialize};
use std::ops::Range;

pub const INVOCATION_SCHEME: &str = "zeron-invoke:";

fn escape_label(label: &str) -> String {
    label
        .replace('\\', "\\\\")
        .replace('[', "\\[")
        .replace(']', "\\]")
        .replace('`', "\\`")
}

/// Native catalog entries may have no local skill file (for example plugin-provided commands).
pub fn native_skill_identity(path: &str) -> bool {
    path.starts_with("opencode-skill:") || path.starts_with("harness-skill:")
}

/// Catalog names must survive canonical link decoding without changing identity.
pub fn valid_invocation_name(name: &str) -> bool {
    !name.is_empty() && !name.chars().any(|c| c.is_control() || c.is_whitespace())
}

/// Spaces and Unicode are valid in local paths and native skill identities.
pub fn valid_skill_path(path: &str) -> bool {
    !path.is_empty() && !path.chars().any(char::is_control)
}

/// Advertised native commands use the same grammar as selected skill commands.
pub fn valid_skill_command_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | ':' | '.'))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillCommand {
    pub name: String,
    pub harness: crate::HarnessId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Skill {
    pub name: String,
    pub path: String,
    pub description: String,
    pub enabled: bool,
    /// A provider-advertised slash invocation for this skill, when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<SkillCommand>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Invocation {
    Command {
        name: String,
    },
    Skill {
        name: String,
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        command: Option<SkillCommand>,
    },
}

impl Invocation {
    pub fn name(&self) -> &str {
        match self {
            Self::Command { name } | Self::Skill { name, .. } => name,
        }
    }
    pub fn prefix(&self) -> char {
        match self {
            Self::Command { .. } => '/',
            Self::Skill { .. } => '$',
        }
    }
    pub fn detail(&self) -> String {
        match self {
            Self::Command { name } => format!("/{name}"),
            Self::Skill { path, .. } => path.clone(),
        }
    }
    pub fn link(&self) -> String {
        let json = serde_json::to_vec(self).expect("invocation serializes");
        let payload: String = json.iter().map(|b| format!("{b:02x}")).collect();
        let label = escape_label(self.name());
        format!("[{}{label}]({INVOCATION_SCHEME}{payload})", self.prefix())
    }
    /// Skills retain their selected identity as an ordinary Markdown file
    /// reference. Commands stay in place so provider prefix semantics survive.
    pub fn prompt_text(&self) -> String {
        match self {
            Self::Command { name } => format!("/{name}"),
            Self::Skill { name, path, .. } => {
                if native_skill_identity(path) {
                    return format!("{name}");
                }
                let path: String = path
                    .bytes()
                    .map(|b| {
                        if b.is_ascii_alphanumeric() || b"/-._~:".contains(&b) {
                            (b as char).to_string()
                        } else {
                            format!("%{b:02X}")
                        }
                    })
                    .collect();
                format!("[${}]({path})", escape_label(name))
            }
        }
    }
}

pub fn invocation_links(text: &str) -> Vec<(Range<usize>, Invocation)> {
    if !text.contains(INVOCATION_SCHEME) {
        return Vec::new();
    }
    let mut links = Vec::new();
    let mut image_depth = 0;
    for (event, range) in pulldown_cmark::Parser::new(text).into_offset_iter() {
        match &event {
            pulldown_cmark::Event::Start(pulldown_cmark::Tag::Image { .. }) => {
                image_depth += 1;
            }
            pulldown_cmark::Event::End(pulldown_cmark::TagEnd::Image) => {
                image_depth -= 1;
            }
            _ => {}
        }
        // Links in image alt text are descriptions, never active selections.
        if image_depth > 0 {
            continue;
        }
        let pulldown_cmark::Event::Start(pulldown_cmark::Tag::Link { dest_url, .. }) = event else {
            continue;
        };
        let Some(hex) = dest_url.strip_prefix(INVOCATION_SCHEME) else {
            continue;
        };
        let (start, end) = (range.start, range.end);
        if hex.len() % 2 != 0 || !hex.is_ascii() {
            continue;
        }
        let bytes: Option<Vec<u8>> = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
            .collect();
        let Some(invocation) = bytes.and_then(|b| serde_json::from_slice::<Invocation>(&b).ok())
        else {
            continue;
        };
        if !valid_invocation_name(invocation.name()) {
            continue;
        }
        if let Invocation::Skill { path, command, .. } = &invocation {
            if !valid_skill_path(path)
                || command
                    .as_ref()
                    .is_some_and(|command| !valid_skill_command_name(&command.name))
            {
                continue;
            }
        }
        let canonical = invocation.link();
        // Older transcripts did not escape label backticks. Accept an old
        // token only when Markdown still identifies the complete link.
        let source = &text[start..end];
        if (canonical == source || canonical.replace("\\`", "`") == source)
            && links
                .last()
                .is_none_or(|(r, _): &(Range<usize>, Invocation)| r.end <= start)
        {
            links.push((start..end, invocation));
        }
    }
    links
}

/// Reject a provider-native skill when its canonical identity has no portable
/// file behind it. File-backed skills remain usable as readable references on
/// another harness; native-only catalog entries cannot be represented there.
pub fn validate_harness_invocations(text: &str, harness: crate::HarnessId) -> Result<(), String> {
    for (_, invocation) in invocation_links(text) {
        if let Invocation::Skill {
            name,
            path,
            command: Some(command),
        } = invocation
            && native_skill_identity(&path)
            && command.harness != harness
        {
            return Err(format!(
                "Skill ${name} is native to {:?} and cannot be sent to {:?}",
                command.harness, harness
            ));
        }
    }
    Ok(())
}

/// Keep selected skill identity intact until Codex builds native input blocks.
/// Other providers receive readable Markdown and their advertised command text.
pub fn harness_prompt(text: &str, harness: crate::HarnessId) -> String {
    let text = crate::file_mentions::file_mention_prompt(text);
    if harness == crate::HarnessId::Codex {
        return text;
    }
    let mut result = String::new();
    let mut at = 0;
    for (range, invocation) in invocation_links(&text) {
        result.push_str(&text[at..range.start]);
        match &invocation {
            Invocation::Skill {
                command: Some(command),
                ..
            } if command.harness == harness
                && text[..range.start]
                    .trim_matches([' ', '\t', '\r', '\n'])
                    .is_empty() =>
            {
                // The catalog supplies the provider's canonical command, regardless
                // of whether the user selected the skill through $ or /.
                result.push_str(&format!("/{}", command.name));
            }
            Invocation::Skill {
                path,
                command: Some(command),
                ..
            } if native_skill_identity(path) && command.harness == harness => {
                result.push_str(&format!("Use the skill /{}", command.name));
            }
            Invocation::Skill { .. } => {
                // Inline invocations and file-only providers need explicit intent;
                // a dollar token by itself has no universal provider semantics.
                result.push_str("Use the skill ");
                result.push_str(&invocation.prompt_text());
            }
            _ => result.push_str(&invocation.prompt_text()),
        }
        at = range.end;
    }
    result.push_str(&text[at..]);
    result
}

pub fn invocation_prompt(text: &str) -> String {
    let mut result = String::new();
    let mut at = 0;
    for (range, invocation) in invocation_links(text) {
        result.push_str(&text[at..range.start]);
        result.push_str(&invocation.prompt_text());
        at = range.end;
    }
    result.push_str(&text[at..]);
    result
}

/// Native commands allow leading blank lines and paragraph indentation, but
/// indented code and non-ASCII whitespace remain ordinary prompt text.
pub fn leading_command(text: &str) -> Option<(&str, &str)> {
    let trimmed = text.trim_start_matches([' ', '\t', '\r', '\n']);
    let indent = text[..text.len() - trimmed.len()]
        .rsplit(['\n', '\r'])
        .next()
        .unwrap_or_default();
    if indent.contains('\t') || indent.len() >= 4 {
        return None;
    }
    let rest = trimmed.strip_prefix('/')?;
    let mut parts = rest.splitn(2, char::is_whitespace);
    let name = parts.next().filter(|name| !name.is_empty())?;
    Some((name, parts.next().unwrap_or_default().trim()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const HARNESSES: [crate::HarnessId; 9] = [
        crate::HarnessId::ClaudeCode,
        crate::HarnessId::Codex,
        crate::HarnessId::Cursor,
        crate::HarnessId::Devin,
        crate::HarnessId::Grok,
        crate::HarnessId::Hermes,
        crate::HarnessId::Pi,
        crate::HarnessId::Antigravity,
        crate::HarnessId::Opencode,
    ];

    #[test]
    fn leading_commands_respect_markdown_indentation() {
        for text in [
            "/review tests",
            "  /review tests",
            "\n   /review tests",
            " \t\n/review tests",
        ] {
            assert_eq!(leading_command(text), Some(("review", "tests")), "{text:?}");
        }
        for text in [
            "    /review",
            "\t/review",
            "\n    /review",
            "\u{a0}/review",
            "Please /review",
            "`/review`",
            "\\/review",
            "/",
        ] {
            assert_eq!(leading_command(text), None, "{text:?}");
        }
    }

    #[test]
    fn delivery_preserves_skill_names_and_paths_with_markdown_punctuation() {
        let skill = Invocation::Skill {
            name: r"review[ui]\draft".into(),
            path: "/repo/é [draft](1)/SKILL.md".into(),
            command: None,
        };
        assert_eq!(invocation_links(&skill.link())[0].1, skill);
        for harness in HARNESSES {
            let delivered = harness_prompt(&format!("Please {} now", skill.link()), harness);
            let delivered = invocation_prompt(&delivered);
            let mut labels = String::new();
            let mut destinations = Vec::new();
            for event in pulldown_cmark::Parser::new(&delivered) {
                match event {
                    pulldown_cmark::Event::Start(pulldown_cmark::Tag::Link {
                        dest_url, ..
                    }) => {
                        destinations.push(dest_url.to_string());
                    }
                    pulldown_cmark::Event::Text(text) => labels.push_str(&text),
                    _ => {}
                }
            }
            assert_eq!(
                destinations,
                ["/repo/%C3%A9%20%5Bdraft%5D%281%29/SKILL.md"],
                "{harness:?}"
            );
            assert!(
                labels.contains(&format!("${}", skill.name())),
                "{harness:?}: {labels}"
            );
        }
    }

    #[test]
    fn literal_markdown_examples_never_activate_skills_for_any_harness() {
        let skill = Invocation::Skill {
            name: "review".into(),
            path: "/repo/SKILL.md".into(),
            command: None,
        }
        .link();
        for literal in [
            format!("`{skill}`"),
            format!("``{skill}``"),
            format!("```md\n{skill}\n```"),
            format!("~~~md\n{skill}\n~~~"),
            format!("    {skill}"),
            format!("\\{skill}"),
            format!("<!-- {skill} -->"),
            format!("<pre>\n{skill}\n</pre>"),
            format!("![example {skill}](example.png)"),
        ] {
            assert!(invocation_links(&literal).is_empty(), "{literal}");
            for harness in HARNESSES {
                assert_eq!(harness_prompt(&literal, harness), literal, "{harness:?}");
            }
        }
    }
    #[test]
    fn selected_native_skills_route_by_provider_and_keep_arguments() {
        for harness in [crate::HarnessId::ClaudeCode, crate::HarnessId::Opencode] {
            let skill = Invocation::Skill {
                name: "plugin:review".into(),
                path: "/repo/SKILL.md".into(),
                command: Some(SkillCommand {
                    name: "plugin:review".into(),
                    harness,
                }),
            };
            let raw = format!("{} inspect tests", skill.link());
            assert_eq!(
                harness_prompt(&raw, harness),
                "/plugin:review inspect tests"
            );
            assert!(harness_prompt(&raw, crate::HarnessId::Cursor).starts_with("Use the skill "));
            assert!(
                harness_prompt(&format!("Please {}", skill.link()), harness)
                    .contains("Use the skill ")
            );
            assert_eq!(invocation_links(&raw)[0].1, skill);
        }
        for harness in [
            crate::HarnessId::Cursor,
            crate::HarnessId::Devin,
            crate::HarnessId::Grok,
            crate::HarnessId::Hermes,
            crate::HarnessId::Pi,
        ] {
            let skill = Invocation::Skill {
                name: "review".into(),
                path: "/repo/SKILL.md".into(),
                command: None,
            };
            assert_eq!(
                harness_prompt(&skill.link(), harness),
                "Use the skill [$review](/repo/SKILL.md)"
            );
        }
    }

    #[test]
    fn native_only_skill_commands_cannot_cross_harnesses() {
        let skill = Invocation::Skill {
            name: "plugin:review".into(),
            path: "opencode-skill:plugin:review".into(),
            command: Some(SkillCommand {
                name: "plugin:review".into(),
                harness: crate::HarnessId::Opencode,
            }),
        };
        let raw = format!("{} inspect tests", skill.link());
        for harness in HARNESSES {
            let result = validate_harness_invocations(&raw, harness);
            if harness == crate::HarnessId::Opencode {
                assert!(result.is_ok(), "{harness:?}: {result:?}");
            } else {
                let error = result.expect_err("foreign native skill must be rejected");
                assert!(error.contains("plugin:review"), "{harness:?}: {error}");
            }
        }
    }

    #[test]
    fn file_backed_skill_commands_keep_the_cross_harness_fallback() {
        let skill = Invocation::Skill {
            name: "review".into(),
            path: "/repo/with space/SKILL.md".into(),
            command: Some(SkillCommand {
                name: "plugin:review".into(),
                harness: crate::HarnessId::Opencode,
            }),
        };
        let raw = format!("{} inspect tests", skill.link());
        for harness in HARNESSES {
            validate_harness_invocations(&raw, harness).unwrap();
            let delivered = harness_prompt(&raw, harness);
            if harness == crate::HarnessId::Opencode {
                assert_eq!(delivered, "/plugin:review inspect tests");
            } else if harness == crate::HarnessId::Codex {
                assert_eq!(invocation_links(&delivered)[0].1, skill);
            } else {
                assert_eq!(
                    delivered, "Use the skill [$review](/repo/with%20space/SKILL.md) inspect tests",
                    "{harness:?}"
                );
            }
        }
    }

    #[test]
    fn native_skill_prefixes_follow_the_same_whitespace_rules_as_commands() {
        for harness in HARNESSES
            .into_iter()
            .filter(|harness| *harness != crate::HarnessId::Codex)
        {
            let skill = Invocation::Skill {
                name: "review".into(),
                path: "/repo/SKILL.md".into(),
                command: Some(SkillCommand {
                    name: "review".into(),
                    harness,
                }),
            };
            for prefix in ["\u{a0}", "\u{2003}"] {
                let raw = format!("{prefix}{} tests", skill.link());
                assert_eq!(
                    harness_prompt(&raw, harness),
                    format!("{prefix}Use the skill [$review](/repo/SKILL.md) tests"),
                    "{harness:?}: non-command prefixes must retain the selected file identity",
                );
            }
            for prefix in ["", "  ", "\n   "] {
                let raw = format!("{prefix}{} tests", skill.link());
                let prompt = harness_prompt(&raw, harness);
                assert_eq!(
                    leading_command(&prompt),
                    Some(("review", "tests")),
                    "{harness:?}"
                );
            }
        }
    }

    #[test]
    fn delivery_preserves_native_skill_identity_only_for_codex() {
        let skill = Invocation::Skill {
            command: None,
            name: "review".into(),
            path: "/repo/with space/SKILL.md".into(),
        };
        let raw = format!(
            "Use {} on {}",
            skill.link(),
            crate::file_mentions::local_file_link("src/lib.rs", false)
        );
        let codex = harness_prompt(&raw, crate::HarnessId::Codex);
        assert_eq!(invocation_links(&codex)[0].1, skill);
        assert!(codex.ends_with("[lib.rs](src/lib.rs)"));
        for harness in [crate::HarnessId::ClaudeCode, crate::HarnessId::Opencode] {
            let prompt = harness_prompt(&raw, harness);
            assert_eq!(
                prompt,
                "Use Use the skill [$review](/repo/with%20space/SKILL.md) on [lib.rs](src/lib.rs)"
            );
        }
    }

    #[test]
    fn legacy_backtick_labels_remain_recognizable() {
        let skill = Invocation::Skill {
            name: "review`ui".into(),
            path: "/repo/SKILL.md".into(),
            command: None,
        };
        let legacy = skill.link().replace("\\`", "`");
        assert_eq!(invocation_links(&legacy)[0].1, skill);
        let file = crate::file_mentions::local_file_link("src/a`b.rs", false);
        let legacy_file = file.replace("\\`", "`");
        assert_eq!(
            crate::file_mentions::file_mention_links(&legacy_file)[0].path,
            "src/a`b.rs"
        );
    }

    #[test]
    fn canonical_labels_cannot_open_markdown_code_spans_across_chips() {
        let skill = Invocation::Skill {
            name: "review`ui".into(),
            path: "/repo/SKILL.md".into(),
            command: None,
        };
        let file = crate::file_mentions::local_file_link("src/a`b.rs", false);
        let raw = format!("{} then {file} then {}", skill.link(), skill.link());
        assert_eq!(invocation_links(&raw).len(), 2);
        assert_eq!(crate::file_mentions::file_mention_links(&raw).len(), 1);
        for harness in HARNESSES {
            let delivered = harness_prompt(&raw, harness);
            if harness == crate::HarnessId::Codex {
                assert_eq!(invocation_links(&delivered).len(), 2);
            } else {
                assert!(
                    !delivered.contains(INVOCATION_SCHEME),
                    "{harness:?}: {delivered}"
                );
            }
            assert!(!delivered.contains(crate::file_mentions::FILE_MENTION_SCHEME));
        }
    }

    #[test]
    fn mixed_canonical_labels_preserve_all_ascii_punctuation() {
        for punctuation in (b'!'..=b'~').filter(u8::is_ascii_punctuation) {
            if punctuation == b'\\' || punctuation == b'/' {
                continue; // A separator is not part of a local basename.
            }
            let punctuation = punctuation as char;
            let skill = Invocation::Skill {
                name: format!("review{punctuation}ui"),
                path: "/repo/SKILL.md".into(),
                command: None,
            };
            let path = format!("src/a{punctuation}b.rs");
            let file = crate::file_mentions::local_file_link(&path, false);
            let raw = format!("{} then {file} then {}", skill.link(), skill.link());
            assert_eq!(invocation_links(&raw).len(), 2, "punctuation {punctuation}");
            let files = crate::file_mentions::file_mention_links(&raw);
            assert_eq!(files.len(), 1, "punctuation {punctuation}");
            assert_eq!(files[0].path, path);
        }
    }

    #[test]
    fn durable_references_round_trip_without_reordering() {
        let command = Invocation::Command {
            name: "compact".into(),
        };
        let skill = Invocation::Skill {
            command: None,
            name: "review".into(),
            path: "/repo/a b/SKILL.md".into(),
        };
        let raw = format!("first {} then {} finally", command.link(), skill.link());
        assert_eq!(invocation_links(&raw).len(), 2);
        assert_eq!(
            invocation_prompt(&raw),
            "first /compact then [$review](/repo/a%20b/SKILL.md) finally"
        );
        assert_eq!(invocation_prompt("/$not-a-chip"), "/$not-a-chip");
        assert!(invocation_links("[x](zeron-invoke:bad)").is_empty());
        for code in [
            format!("`{}`", command.link()),
            format!("```\n{}\n```", command.link()),
            format!("\\{}", command.link()),
        ] {
            assert!(invocation_links(&code).is_empty());
            assert_eq!(invocation_prompt(&code), code);
        }
    }
}
