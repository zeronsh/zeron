//! Compatibility with content references emitted by agent skills.
//!
//! Keep transport-specific spelling here, not in a frontend or a harness:
//! skills can emit the same text through different providers. This module
//! never touches the filesystem. The original message remains authoritative.
use std::ops::Range;

use pulldown_cmark::{Event, Parser, Tag};

const CITATION: &str = ":codex-file-citation{";
const VISUALIZE: &str = "\u{e200}visualize\u{e202}";
const VISUALIZE_END: &str = "\u{e201}";
const PLACEHOLDER: &str = "\u{e000}zeron-content-reference\u{e001}";
const MAX_REFERENCE_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileReference {
    pub path: String,
}

impl FileReference {
    pub fn label(&self) -> &str {
        self.path.rsplit(['/', '\\']).next().unwrap_or(&self.path)
    }
}

/// A directive's path is literal, whereas a Markdown destination is a URI:
/// protect percent signs and location punctuation from a second interpretation.
pub fn file_destination(path: &str) -> String {
    use std::fmt::Write;
    let mut destination = String::from("file://");
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/') {
            destination.push(byte as char);
        } else {
            write!(&mut destination, "%{byte:02X}").unwrap();
        }
    }
    destination
}

/// A host-independent request. File IO and document execution belong to the
/// receiving host; a Markdown parser only describes what may be previewed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HtmlPreview {
    pub path: String,
    pub wide: bool,
    pub compatibility: HtmlCompatibility,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HtmlCompatibility {
    None,
    CodexVisualize,
}

pub fn html_preview(text: &str) -> Option<HtmlPreview> {
    let payload = text.strip_prefix(VISUALIZE)?.strip_suffix(VISUALIZE_END)?;
    if payload.len() > MAX_REFERENCE_BYTES {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(payload).ok()?;
    let object = value.as_object()?;
    if object
        .keys()
        .any(|key| !matches!(key.as_str(), "path" | "mode"))
    {
        return None;
    }
    let path = object.get("path")?.as_str()?;
    let mode = object.get("mode").map(|v| v.as_str()).unwrap_or(Some(""))?;
    if !matches!(mode, "" | "wide")
        || !absolute_path(path)
        || path.chars().any(char::is_control)
        || !path
            .rsplit('.')
            .next()
            .is_some_and(|ext| matches!(ext.to_ascii_lowercase().as_str(), "html" | "htm"))
    {
        return None;
    }
    Some(HtmlPreview {
        path: path.into(),
        wide: mode == "wide",
        compatibility: HtmlCompatibility::CodexVisualize,
    })
}

fn absolute_path(path: &str) -> bool {
    path.starts_with('/')
        || (path.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
            && path.as_bytes().get(1) == Some(&b':')
            && path
                .as_bytes()
                .get(2)
                .is_some_and(|c| matches!(c, b'/' | b'\\')))
}

/// Decode one complete directive. Unknown attributes are preserved in the
/// original message but do not grant extra behavior (e.g. page navigation).
pub fn file_reference(text: &str) -> Option<FileReference> {
    let mut rest = text.strip_prefix(CITATION)?.strip_suffix('}')?.trim();
    let mut path = None;
    let mut purpose = None;
    while !rest.is_empty() {
        let end = rest.find(|c: char| !c.is_ascii_alphanumeric() && c != '_')?;
        let key = &rest[..end];
        if key.is_empty() {
            return None;
        }
        rest = rest[end..].trim_start().strip_prefix('=')?.trim_start();
        let quote = rest.chars().next()?;
        if !matches!(quote, '"' | '\'') {
            return None;
        }
        rest = &rest[1..];
        let mut value = String::new();
        let mut chars = rest.char_indices();
        let consumed = loop {
            let (at, c) = chars.next()?;
            if c == quote {
                break at + 1;
            }
            if c == '\\' {
                let (_, escaped) = chars.next()?;
                if escaped != quote && escaped != '\\' {
                    return None;
                }
                value.push(escaped);
            } else if c.is_control() {
                return None;
            } else {
                value.push(c);
            }
        };
        match key {
            "path" if path.is_none() => path = Some(value),
            "purpose" if purpose.is_none() && matches!(value.as_str(), "source" | "output") => {
                purpose = Some(value);
            }
            "path" | "purpose" => return None,
            _ => {}
        }
        rest = &rest[consumed..];
        if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
            return None;
        }
        rest = rest.trim_start();
    }
    let path = path?;
    (absolute_path(&path) && !path.ends_with(['/', '\\']) && path.len() > 1)
        .then_some(FileReference { path })
}

struct Replacement {
    source: Range<usize>,
    shown: Range<usize>,
}

/// Opaque text tokens protect payloads from CommonMark punctuation. Unlike
/// code delimiters, adjacent tokens cannot fuse with each other or nearby code.
pub(crate) struct Rewritten<'a> {
    pub text: String,
    source: &'a str,
    replacements: Vec<Replacement>,
    placeholder: String,
}

impl Rewritten<'_> {
    pub fn original_range(&self, range: Range<usize>) -> Range<usize> {
        self.original(range.start, false)..self.original(range.end, true)
    }

    fn original(&self, at: usize, end: bool) -> usize {
        let mut delta = 0isize;
        for replacement in &self.replacements {
            if at <= replacement.shown.start {
                break;
            }
            if at < replacement.shown.end {
                return if end {
                    replacement.source.end
                } else {
                    replacement.source.start
                };
            }
            delta = replacement.source.end as isize - replacement.shown.end as isize;
        }
        at.saturating_add_signed(delta)
    }

    pub fn expand<'a>(
        &self,
        event: Event<'a>,
        range: Range<usize>,
        mut emit: impl FnMut(Event<'a>, Range<usize>),
    ) {
        let Event::Text(text) = event else {
            emit(event, range);
            return;
        };
        let first = self
            .replacements
            .partition_point(|r| r.shown.start < range.start);
        let replacements = &self.replacements[first..];
        if replacements.first().is_none_or(|r| r.shown.end > range.end) {
            emit(Event::Text(text), range);
            return;
        }
        let mut remaining = text.as_ref();
        let mut start = range.start;
        for replacement in replacements.iter().take_while(|r| r.shown.end <= range.end) {
            let Some(at) = remaining.find(&self.placeholder) else {
                break;
            };
            if at > 0 {
                emit(
                    Event::Text(remaining[..at].to_owned().into()),
                    start..replacement.shown.start,
                );
            }
            emit(
                Event::InlineHtml(self.source[replacement.source.clone()].to_owned().into()),
                replacement.shown.clone(),
            );
            remaining = &remaining[at + self.placeholder.len()..];
            start = replacement.shown.end;
        }
        if !remaining.is_empty() {
            emit(Event::Text(remaining.to_owned().into()), start..range.end);
        }
    }
}

pub(crate) fn rewrite(source: &str) -> Option<Rewritten<'_>> {
    if !source.contains(CITATION) && !source.contains(VISUALIZE) {
        return None;
    }
    // Ask the same CommonMark parser which regions are literal. In particular,
    // an unclosed code fence must stay literal throughout streaming.
    let protected: Vec<_> = Parser::new_ext(source, super::parser::options())
        .into_offset_iter()
        .filter_map(|(event, range)| match event {
            Event::Code(_)
            | Event::Html(_)
            | Event::InlineHtml(_)
            | Event::Start(
                Tag::CodeBlock(_) | Tag::HtmlBlock | Tag::Link { .. } | Tag::Image { .. },
            ) => Some(range),
            _ => None,
        })
        .collect();
    let mut text = String::new();
    let mut placeholder = PLACEHOLDER.to_owned();
    while source.contains(&placeholder) {
        placeholder.push('\u{e001}');
    }
    let mut replacements = Vec::new();
    let mut copied = 0;
    let mut at = 0;
    while let Some(relative) = source[at..].find(['\u{e200}', ':']) {
        let start = at + relative;
        let visualize = source[start..].starts_with(VISUALIZE);
        let prefix = if visualize { VISUALIZE } else { CITATION };
        at = start + source[start..].chars().next().unwrap().len_utf8();
        if !source[start..].starts_with(prefix) {
            continue;
        }
        at = start + prefix.len();
        if protected.iter().any(|r| r.contains(&start))
            || source[..start]
                .bytes()
                .rev()
                .take_while(|b| *b == b'\\')
                .count()
                % 2
                != 0
        {
            continue;
        }
        let mut quoted = None;
        let mut escaped = false;
        let end = if visualize {
            let bytes = &source.as_bytes()[at..];
            bytes[..bytes.len().min(MAX_REFERENCE_BYTES + VISUALIZE_END.len())]
                .windows(VISUALIZE_END.len())
                .position(|window| window == VISUALIZE_END.as_bytes())
                .map(|n| at + n + VISUALIZE_END.len())
        } else {
            source[at..]
                .char_indices()
                .take_while(|(i, _)| *i < MAX_REFERENCE_BYTES)
                .find_map(|(i, c)| {
                    if escaped {
                        escaped = false;
                        return None;
                    }
                    if c == '\\' && quoted.is_some() {
                        escaped = true;
                        return None;
                    }
                    if quoted == Some(c) {
                        quoted = None;
                    } else if quoted.is_none() && matches!(c, '"' | '\'') {
                        quoted = Some(c);
                    } else if quoted.is_none() && c == '}' {
                        return Some(at + i + 1);
                    }
                    None
                })
        };
        let Some(end) = end else { continue };
        // Inline references cannot cross a paragraph boundary during streaming.
        if source[start..end].contains(['\n', '\r']) {
            continue;
        }
        if file_reference(&source[start..end]).is_none()
            && html_preview(&source[start..end]).is_none()
        {
            continue;
        }
        text.push_str(&source[copied..start]);
        let shown_start = text.len();
        text.push_str(&placeholder);
        replacements.push(Replacement {
            source: start..end,
            shown: shown_start..text.len(),
        });
        copied = end;
        at = end;
    }
    if replacements.is_empty() {
        return None;
    }
    text.push_str(&source[copied..]);
    Some(Rewritten {
        text,
        source,
        replacements,
        placeholder,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::{Block, IncrementalParser, parse_full};

    const VISUAL: &str =
        "\u{e200}visualize\u{e202}{\"path\":\"/tmp/方案 [a].html\",\"mode\":\"wide\"}\u{e201}";

    #[test]
    fn visualize_is_typed_only_when_complete_and_outside_examples() {
        let request = html_preview(VISUAL).unwrap();
        assert_eq!(request.path, "/tmp/方案 [a].html");
        assert!(request.wide);
        assert_eq!(links(VISUAL), [VISUAL]);
        assert_eq!(links(&format!("{VISUAL}{VISUAL}")), [VISUAL, VISUAL]);
        assert!(rewrite(&VISUAL.replace(",", ",\n\n")).is_none());
        for source in [
            format!("`{VISUAL}`"),
            format!("```html\n{VISUAL}\n```"),
            format!("\\{VISUAL}"),
            VISUAL.replace("wide", "unknown"),
            VISUAL.replace(".html", ".pdf"),
            VISUAL.replace("/tmp/", "../"),
            VISUAL.replace(VISUALIZE_END, ""),
        ] {
            assert!(rewrite(&source).is_none(), "{source}");
        }
        let source =
            format!("前文\n\n{VISUAL}\n\n- [x] 后续 :codex-file-citation{{path=\"/tmp/a.pdf\"}}\n");
        let mut parser = IncrementalParser::new();
        for (i, c) in source.char_indices() {
            let end = i + c.len_utf8();
            parser.set_text(&source[..end]);
            assert_eq!(parser.tree(), &parse_full(&source[..end]), "{end}");
        }
        let tree = parse_full(&source);
        assert_eq!(
            &source[tree.blocks[2].range.clone()],
            "- [x] 后续 :codex-file-citation{path=\"/tmp/a.pdf\"}\n"
        );
    }

    fn links(source: &str) -> Vec<String> {
        let tree = parse_full(source);
        let Block::Paragraph { runs } = &tree.blocks[0].block else {
            panic!("paragraph")
        };
        runs.iter().filter_map(|r| r.style.link.clone()).collect()
    }

    #[test]
    fn citations_keep_literal_paths_and_readable_labels() {
        let source = r#"看过 :codex-file-citation{path="/tmp/空 格/a_*[x]`{y}.pdf" purpose="source"} 和 :codex-file-citation{purpose="output" path="/tmp/b.pdf"}。"#;
        assert_eq!(
            links(source),
            [
                file_destination("/tmp/空 格/a_*[x]`{y}.pdf"),
                "file:///tmp/b.pdf".into()
            ]
        );
        let tree = parse_full(source);
        let Block::Paragraph { runs } = &tree.blocks[0].block else {
            panic!()
        };
        assert_eq!(runs[1].style.file_label.as_deref(), Some("a_*[x]`{y}.pdf"));
        assert_eq!(runs.last().unwrap().text, "。");
    }

    #[test]
    fn examples_and_invalid_directives_are_not_links() {
        let citation = r#":codex-file-citation{path="/tmp/a.pdf" purpose="source"}"#;
        for source in [
            format!("`{citation}`"),
            format!("\\{citation}"),
            format!("[{citation}](https://example.com)"),
            citation.replace("/tmp/a.pdf", "javascript:alert(1)"),
            citation.replace("source", "bad"),
        ] {
            assert!(
                !links(&source).contains(&"file:///tmp/a.pdf".into()),
                "{source}"
            );
        }
        assert!(rewrite(&format!("```text\n{citation}")).is_none());
        assert!(rewrite(&format!("    {citation}")).is_none());
    }

    #[test]
    fn citation_paths_are_literal_not_line_anchors_or_uri_escapes() {
        let source = r#":codex-file-citation{path="/tmp/a%20#L4.pdf" purpose="source"}"#;
        assert_eq!(links(source), ["file:///tmp/a%2520%23L4.pdf"]);
        let tree = parse_full(source);
        let Block::Paragraph { runs } = &tree.blocks[0].block else {
            panic!()
        };
        assert_eq!(runs[0].text, "/tmp/a%20#L4.pdf");
    }

    #[test]
    fn adjacent_citations_and_code_keep_their_own_boundaries() {
        let reference = r#":codex-file-citation{path="/tmp/a.pdf"}"#;
        for source in [
            format!("{reference}{reference}"),
            format!("`code`{reference}{reference}`after`"),
            format!("&amp;{reference}**{reference}**"),
            format!("{PLACEHOLDER} {reference}{reference}"),
        ] {
            assert_eq!(
                links(&source),
                ["file:///tmp/a.pdf", "file:///tmp/a.pdf"],
                "{source}"
            );
            let mut parser = IncrementalParser::new();
            for (i, c) in source.char_indices() {
                let end = i + c.len_utf8();
                parser.set_text(&source[..end]);
                assert_eq!(parser.tree(), &parse_full(&source[..end]), "{end}");
            }
        }
        let multiline = ":codex-file-citation{path=\"/tmp/a.pdf\"\n\npurpose=\"source\"}";
        assert!(rewrite(multiline).is_none());
    }

    #[test]
    fn streaming_matches_full_parse_at_every_character_boundary() {
        let source = "**资料** :codex-file-citation{path=\"/tmp/资料.pdf\" purpose=\"source\"}\n\n- [ ] 后续\n";
        let mut parser = IncrementalParser::new();
        for (i, c) in source.char_indices() {
            let end = i + c.len_utf8();
            parser.set_text(&source[..end]);
            assert_eq!(parser.tree(), &parse_full(&source[..end]), "{end}");
        }
        let tree = parse_full(source);
        let Block::List { items, .. } = &tree.blocks[1].block else {
            panic!()
        };
        let Block::Paragraph { runs } = &items[0][0] else {
            panic!()
        };
        let task = runs[0].style.task.as_ref().unwrap();
        assert_eq!(&source[task.range.clone()], "[ ]");
    }
}
