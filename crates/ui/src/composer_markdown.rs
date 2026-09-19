//! Source-aware Markdown editing helpers. The draft is always plain Markdown.
use pulldown_cmark::{Event, Options, Parser, Tag};
use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Face {
    Bold,
    Italic,
    Code,
    Strikethrough,
}

pub fn faces(text: &str) -> Vec<(Range<usize>, Face)> {
    if text.len() > 128 * 1024 {
        return Vec::new();
    }
    Parser::new_ext(
        text,
        Options::ENABLE_TASKLISTS | Options::ENABLE_STRIKETHROUGH,
    )
    .into_offset_iter()
    .filter_map(|(event, range)| {
        let face = match event {
            Event::Start(Tag::Strong | Tag::Heading { .. }) => Face::Bold,
            Event::Start(Tag::Strikethrough) => Face::Strikethrough,
            Event::Start(Tag::Emphasis) => Face::Italic,
            Event::Start(Tag::CodeBlock(_)) | Event::Code(_) => Face::Code,
            _ => return None,
        };
        Some((range, face))
    })
    .collect()
}

/// Highlight fenced code with its own grammar. Running the Markdown grammar
/// over the whole draft colors fence bodies as strings, including identifiers.
/// Keep prose and fence markers neutral, and retain exact source byte offsets.
pub fn syntax_spans(text: &str) -> Vec<zeron_syntax::HighlightSpan> {
    if text.len() > 128 * 1024 {
        return Vec::new();
    }
    let mut language = None;
    let mut body = String::new();
    let mut segments = Vec::new();
    let mut result = Vec::new();
    for (event, range) in Parser::new(text).into_offset_iter() {
        match event {
            Event::Start(Tag::CodeBlock(pulldown_cmark::CodeBlockKind::Fenced(info))) => {
                language = info.split_whitespace().next().map(str::to_owned);
                body.clear();
                segments.clear();
            }
            Event::Text(_) if language.is_some() => {
                // Nested fences emit separate text events with quote/list
                // prefixes removed. Keep their exact source mapping, but parse
                // the complete body so multiline strings/comments retain state.
                let start = body.len();
                body.push_str(&text[range.clone()]);
                segments.push((start..body.len(), range));
            }
            Event::End(pulldown_cmark::TagEnd::CodeBlock) => {
                if let Some(language) = language.take() {
                    highlight_code_body(&body, &language, &segments, &mut result);
                }
                if result.len() >= 16_000 {
                    break;
                }
            }
            _ => {}
        }
    }
    result
}

fn highlight_code_body(
    source: &str,
    language: &str,
    segments: &[(Range<usize>, Range<usize>)],
    result: &mut Vec<zeron_syntax::HighlightSpan>,
) {
    let Ok(document) = zeron_syntax::highlight_with_limits(
        zeron_syntax::HighlightRequest {
            source,
            path: None,
            fence_tag: Some(language),
        },
        zeron_syntax::HighlightLimits {
            max_source_bytes: 128 * 1024,
            max_spans: 16_000 - result.len(),
        },
        None,
    ) else {
        return;
    };
    let mut offset = 0;
    for (line, spans) in source.split('\n').zip(document.lines) {
        for span in spans {
            let start = offset + span.range.start;
            let end = offset + span.range.end;
            let first = segments.partition_point(|(body, _)| body.end <= start);
            for (body, raw) in segments[first..]
                .iter()
                .take_while(|(body, _)| body.start < end)
            {
                if result.len() >= 16_000 {
                    return;
                }
                result.push(zeron_syntax::HighlightSpan {
                    range: raw.start + start.max(body.start) - body.start
                        ..raw.start + end.min(body.end) - body.start,
                    kind: span.kind,
                });
            }
        }
        offset += line.len() + 1;
    }
}

pub fn in_code(text: &str, cursor: usize) -> bool {
    if cursor > text.len() || !text.is_char_boundary(cursor) {
        return false;
    }
    let mut parsed_code = Vec::new();
    let mut inline_start = None;
    let mut quote_depth = 0;
    for (event, range) in Parser::new_ext(
        text,
        Options::ENABLE_TASKLISTS | Options::ENABLE_STRIKETHROUGH,
    )
    .into_offset_iter()
    {
        match event {
            Event::Start(Tag::BlockQuote(_)) => quote_depth += 1,
            Event::End(pulldown_cmark::TagEnd::BlockQuote(_)) => quote_depth -= 1,
            Event::Start(Tag::Paragraph | Tag::Heading { .. } | Tag::Item)
                if range.start <= cursor && cursor <= range.end =>
            {
                inline_start = Some(range.start);
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                if range.start <= cursor && cursor < range.end {
                    return true;
                }
                if range.end == cursor {
                    match kind {
                        pulldown_cmark::CodeBlockKind::Indented => return true,
                        pulldown_cmark::CodeBlockKind::Fenced(_) => {
                            let source = &text[range.clone()];
                            let mut lines = source.lines();
                            let opening = lines.next().unwrap_or_default().trim_start();
                            let delimiter = opening.chars().next().unwrap_or('`');
                            let count = opening.chars().take_while(|c| *c == delimiter).count();
                            let closed = lines.last().is_some_and(|line| {
                                // Offset ranges retain container prefixes on
                                // subsequent lines. Remove only quote markers
                                // belonging to the parsed enclosing containers.
                                let mut line = line.trim();
                                for _ in 0..quote_depth {
                                    if let Some(rest) = line.strip_prefix('>') {
                                        line = rest.trim_start();
                                    }
                                }
                                line.len() >= count && line.chars().all(|c| c == delimiter)
                            });
                            if !closed {
                                return true;
                            }
                        }
                    }
                }
                parsed_code.push(range);
            }
            Event::Text(_) if range.end == cursor => {
                // A fence-looking final line can still be literal code (for
                // example with four spaces of extra indentation). The parser's
                // body event is authoritative over the closing-line heuristic.
                if parsed_code
                    .last()
                    .is_some_and(|code: &Range<usize>| code.contains(&range.start))
                {
                    return true;
                }
            }
            Event::Code(_) => {
                if range.start <= cursor && cursor < range.end {
                    return true;
                }
                parsed_code.push(range);
            }
            _ => {}
        }
    }
    // Pulldown intentionally leaves unfinished inline spans as prose. Match
    // delimiter RUNS, skipping valid parsed spans (which can contain backticks
    // of another length), rather than counting individual backticks.
    let before = &text[..cursor];
    // Inline code cannot cross a paragraph/heading boundary. Whitespace-only
    // blank lines also terminate unfinished spans, including in CRLF drafts.
    let mut at = inline_start.unwrap_or_else(|| {
        let mut offset = 0;
        let mut start = 0;
        for line in before.split_inclusive('\n') {
            offset += line.len();
            if line.trim_matches([' ', '\t', '\r', '\n']).is_empty() {
                start = offset;
            }
        }
        start
    });
    let bytes = before.as_bytes();
    let mut delimiter = None;
    let mut code_index = 0;
    while at < bytes.len() {
        while parsed_code
            .get(code_index)
            .is_some_and(|range| range.end <= at)
        {
            code_index += 1;
        }
        if let Some(range) = parsed_code
            .get(code_index)
            .filter(|range| range.contains(&at))
        {
            at = range.end;
        } else if bytes[at] == b'\\' && delimiter.is_none() {
            at += 2;
        } else if bytes[at] == b'`' {
            let count = bytes[at..].iter().take_while(|b| **b == b'`').count();
            if delimiter == Some(count) {
                delimiter = None;
            } else if delimiter.is_none() {
                delimiter = Some(count);
            }
            at += count;
        } else {
            at += 1;
        }
    }
    delimiter.is_some()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListPrefix {
    /// Start of list indentation, after any literal quote-container prefix.
    pub indent_start: usize,
    /// Absolute byte offset of the list marker within the source line.
    pub indent: usize,
    pub end: usize,
    pub next: String,
    pub bullet: Option<usize>,
}

fn is_thematic_break(line: &str) -> bool {
    let marks: Vec<_> = line
        .chars()
        .filter(|c| !matches!(c, ' ' | '\t' | '\r'))
        .collect();
    marks.len() >= 3 && matches!(marks[0], '-' | '*' | '_') && marks.iter().all(|c| *c == marks[0])
}

/// Preserve each quote marker and its optional separator exactly as authored.
/// Four leading spaces belong to indented code, not another quote container.
fn quote_prefix_end(line: &str) -> usize {
    let mut end = 0;
    loop {
        let rest = &line[end..];
        let spaces = rest.bytes().take_while(|byte| *byte == b' ').count();
        if spaces > 3 || rest.as_bytes().get(spaces) != Some(&b'>') {
            break;
        }
        end += spaces + 1;
        if matches!(line.as_bytes().get(end), Some(b' ' | b'\t')) {
            end += 1;
        }
    }
    end
}

pub fn list_prefix(line: &str) -> Option<ListPrefix> {
    let line = line.strip_suffix('\r').unwrap_or(line);
    let indent_start = quote_prefix_end(line);
    let content = &line[indent_start..];
    if is_thematic_break(content) {
        return None;
    }
    let indent = line.len() - content.trim_start_matches([' ', '\t']).len();
    let rest = &line[indent..];
    let (marker_end, next_marker, bullet) = if rest.starts_with(['-', '*', '+']) {
        (1, rest[..1].to_string(), Some(indent))
    } else {
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 || digits > 9 {
            return None;
        }
        let delimiter = rest.get(digits..digits + 1)?;
        if delimiter != "." && delimiter != ")" {
            return None;
        }
        let number: u32 = rest[..digits].parse().ok()?;
        (digits + 1, format!("{}{delimiter}", number + 1), None)
    };
    let after_marker = &rest[marker_end..];
    if !after_marker.is_empty() && !after_marker.starts_with([' ', '\t']) {
        return None;
    }
    let padding = after_marker.len() - after_marker.trim_start_matches([' ', '\t']).len();
    let marker_len = marker_end + padding;
    let task_source = &rest[marker_len..];
    let task = ["[ ]", "[x]", "[X]"]
        .iter()
        .any(|marker| task_source.starts_with(marker))
        && task_source
            .get(3..)
            .is_some_and(|tail| tail.is_empty() || tail.starts_with([' ', '\t']));
    let task_padding = if task {
        let tail = &task_source[3..];
        tail.len() - tail.trim_start_matches([' ', '\t']).len()
    } else {
        0
    };
    let separator = if padding == 0 {
        " "
    } else {
        &rest[marker_end..marker_len]
    };
    Some(ListPrefix {
        indent_start,
        indent,
        end: indent + marker_len + if task { 3 + task_padding } else { 0 },
        next: format!(
            "{}{next_marker}{separator}{}",
            &line[..indent],
            if task { "[ ] " } else { "" }
        ),
        bullet,
    })
}

pub fn newline_edit(text: &str, cursor: usize) -> Option<(Range<usize>, String)> {
    if cursor > text.len() || !text.is_char_boundary(cursor) || in_code(text, cursor) {
        return None;
    }
    let start = text[..cursor].rfind('\n').map_or(0, |i| i + 1);
    let end = text[cursor..].find('\n').map_or(text.len(), |i| cursor + i);
    let raw_line = &text[start..end];
    let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
    let line_end = start + line.len();
    let prefix = list_prefix(line)?;
    if Parser::new(text).into_offset_iter().any(|(event, range)| {
        matches!(event, Event::Start(Tag::Heading { .. }))
            && range.contains(&(start + prefix.indent))
    }) {
        return None;
    }
    if cursor < start + prefix.end {
        return None;
    }
    if line[prefix.end..].trim().is_empty() {
        // Leaving an empty nested item outdents one level first.
        let retained =
            prefix.indent_start + (prefix.indent - prefix.indent_start).saturating_sub(2);
        return Some((start..line_end, line[..retained].to_string()));
    }
    let newline = if raw_line.ends_with('\r') {
        "\r\n"
    } else {
        "\n"
    };
    Some((cursor..cursor, format!("{newline}{}", prefix.next)))
}

/// The parser has already established that this is a heading. Only ATX
/// markers collapse: setext underlines retain their source line and geometry.
fn decorate_atx_heading(source: &str, offset: usize, edits: &mut Vec<(Range<usize>, String)>) {
    let line = source
        .split('\n')
        .next()
        .unwrap_or_default()
        .trim_end_matches('\r');
    let indent = line.len() - line.trim_start_matches([' ', '\t']).len();
    let hashes = line[indent..]
        .bytes()
        .take_while(|byte| *byte == b'#')
        .count();
    if !(1..=6).contains(&hashes) {
        return;
    }
    let after_marker = indent + hashes;
    if after_marker < line.len() && !line[after_marker..].starts_with([' ', '\t']) {
        return;
    }
    let content_start = line.len() - line[after_marker..].trim_start_matches([' ', '\t']).len();
    edits.push((offset + indent..offset + content_start, String::new()));
    let trimmed = line.trim_end_matches([' ', '\t']);
    let closing = trimmed
        .bytes()
        .rev()
        .take_while(|byte| *byte == b'#')
        .count();
    let closing_start = trimmed.len() - closing;
    if closing > 0
        && closing_start > 0
        && matches!(line.as_bytes()[closing_start - 1], b' ' | b'\t')
    {
        let whitespace_start = line[..closing_start].trim_end_matches([' ', '\t']).len();
        let start = whitespace_start.max(content_start);
        if start < line.len() {
            edits.push((offset + start..offset + line.len(), String::new()));
        }
    }
}

/// Hidden delimiters outside the active logical line, and typographic bullets.
/// Each replacement retains a source range for caret/IME mapping.
pub fn decorations(text: &str, active: Range<usize>) -> Vec<(Range<usize>, String)> {
    if text.len() > 128 * 1024 {
        return Vec::new();
    }
    let mut edits = Vec::new();
    let mut tasks = Vec::new();
    let mut heading_ranges = Vec::new();
    let code_ranges: Vec<_> = faces(text)
        .into_iter()
        .filter_map(|(range, face)| (face == Face::Code).then_some(range))
        .collect();
    for (event, range) in Parser::new_ext(
        text,
        Options::ENABLE_TASKLISTS | Options::ENABLE_STRIKETHROUGH,
    )
    .into_offset_iter()
    {
        if matches!(event, Event::Start(Tag::Heading { .. })) {
            heading_ranges.push(range.clone());
        }
        if range.start <= active.end && range.end > active.start {
            continue;
        }
        if let Event::TaskListMarker(checked) = event {
            tasks.push((range.clone(), checked));
        }
        let source = &text[range.clone()];
        if matches!(event, Event::Start(Tag::Heading { .. })) {
            decorate_atx_heading(source, range.start, &mut edits);
        }
        let n = match event {
            Event::Start(Tag::Strong) => 2,
            Event::Start(Tag::Emphasis) => 1,
            Event::Start(Tag::Strikethrough) => source.bytes().take_while(|b| *b == b'~').count(),
            Event::Code(_) => source.bytes().take_while(|b| *b == b'`').count(),
            _ => continue,
        };
        if n > 0 && range.len() > 2 * n {
            edits.push((range.start..range.start + n, String::new()));
            edits.push((range.end - n..range.end, String::new()));
        }
    }
    let mut at = 0;
    for line in text.split('\n') {
        if !is_thematic_break(line) {
            if let Some(prefix) = list_prefix(line) {
                let marker = at + prefix.indent;
                let code_index = code_ranges.partition_point(|range| range.end <= marker);
                // A single '-' can be a setext underline, not a list item.
                // Preserve parsed heading source regardless of active line.
                let heading_index = heading_ranges.partition_point(|range| range.end <= marker);
                let in_heading = heading_ranges
                    .get(heading_index)
                    .is_some_and(|range| range.contains(&marker));
                if !in_heading
                    && !code_ranges
                        .get(code_index)
                        .is_some_and(|range| range.contains(&marker))
                {
                    let task_index = tasks.partition_point(|(range, _)| range.start < marker);
                    let task = tasks
                        .get(task_index)
                        .filter(|(range, _)| range.end <= at + prefix.end);
                    if let Some((range, checked)) = task {
                        edits.push((
                            prefix.bullet.map_or(range.start, |bullet| at + bullet)..range.end,
                            if *checked { "☑" } else { "☐" }.into(),
                        ));
                    } else if let Some(bullet) = prefix.bullet {
                        // Bullets retain a one-character source mapping, so they
                        // stay editable without flashing back to raw markers
                        // whenever the caret enters their line.
                        edits.push((at + bullet..at + bullet + 1, "•".into()));
                    }
                }
            }
        }
        at += line.len() + 1;
    }
    edits.sort_by_key(|(r, _)| r.start);
    edits
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decorations_preserve_rules_code_and_nested_emphasis() {
        for rule in ["- - -", "* * *", "___"] {
            assert!(list_prefix(rule).is_none());
            assert!(
                decorations(&format!("{rule}\nactive"), rule.len() + 1..rule.len() + 7).is_empty()
            );
        }
        let text = "```inline```\nactive";
        assert_eq!(
            decorations(text, 13..text.len()),
            vec![(0..3, String::new()), (9..12, String::new())]
        );
        let text = "***both*** and **bold _nested_**\nactive";
        let edits = decorations(text, text.rfind('\n').unwrap() + 1..text.len());
        for pair in edits.windows(2) {
            assert!(pair[0].0.end <= pair[1].0.start);
        }
        let mut rendered = text.to_string();
        for (range, replacement) in edits.into_iter().rev() {
            rendered.replace_range(range, &replacement);
        }
        assert_eq!(rendered, "both and bold nested\nactive");
        assert_eq!(newline_edit("café", 4), None);
        assert_eq!(newline_edit("short", 100), None);
    }

    #[test]
    fn code_boundaries_and_delimiter_runs() {
        for text in [
            "    - shell-command",
            "    $skill",
            "``code $",
            "````rust\ncode\n```",
        ] {
            assert!(in_code(text, text.len()), "{text:?}");
        }
        for text in [
            "``literal ` backtick`` $skill",
            "```rust\ncode\n```",
            "`code` $skill",
        ] {
            assert!(!in_code(text, text.len()), "{text:?}");
        }
        assert_eq!(newline_edit("    - shell-command", 19), None);
    }

    #[test]
    fn unfinished_inline_code_stops_at_markdown_block_boundaries() {
        for text in [
            "`unfinished\n \n$skill",
            "`unfinished\r\n\t\r\n@file",
            "`unfinished\n# heading $skill",
            "`unfinished\n\n- @file",
            "`unfinished\n \n",
        ] {
            assert!(!in_code(text, text.len()), "{text:?}");
        }
        for text in [
            "`unfinished\nsoft line $skill",
            "# heading `unfinished $skill",
        ] {
            assert!(in_code(text, text.len()), "{text:?}");
        }
    }

    #[test]
    fn injected_colors_use_source_offsets_after_unicode_and_mentions() {
        let text = "héllo [file](zeron-file:src/main.rs)\n```rust\nfn main() { let café = 42; }\n```\n```python\ndef hello(): pass\n```";
        let spans = syntax_spans(text);
        assert!(spans.iter().any(|span| &text[span.range.clone()] == "fn"
            && span.kind == zeron_syntax::HighlightKind::Keyword));
        assert!(spans.iter().any(|span| &text[span.range.clone()] == "def"
            && span.kind == zeron_syntax::HighlightKind::Keyword));
        assert!(syntax_spans(&"x".repeat(128 * 1024 + 1)).is_empty());
        assert!(
            spans
                .iter()
                .all(|span| !text[span.range.clone()].contains("```"))
        );
        assert!(syntax_spans("Normal `inline code` and **bold**").is_empty());
        let rust = "```rust\nlet name = \"literal\";\n```";
        let highlighted = syntax_spans(rust);
        assert!(
            highlighted
                .iter()
                .filter(|span| span.kind == zeron_syntax::HighlightKind::String)
                .all(|span| !rust[span.range.clone()].contains("name"))
        );
    }

    #[test]
    fn nested_fences_preserve_multiline_language_state_and_source_offsets() {
        for (language, lines, token, kind) in [
            (
                "rust",
                ["/* open", "café middle", "close */"],
                "café middle",
                zeron_syntax::HighlightKind::Comment,
            ),
            (
                "python",
                ["value = \"\"\"open", "café middle", "close\"\"\""],
                "café middle",
                zeron_syntax::HighlightKind::String,
            ),
        ] {
            for (opening, prefix) in [("", "> "), ("- item\n", "  ")] {
                let text = format!(
                    "{opening}{prefix}```{language}\n{prefix}{}\n{prefix}{}\n{prefix}{}\n{prefix}```",
                    lines[0], lines[1], lines[2]
                );
                let spans = syntax_spans(&text);
                assert!(
                    spans
                        .iter()
                        .any(|span| span.kind == kind && text[span.range.clone()].contains(token)),
                    "{text:?}: {spans:?}"
                );
                for span in &spans {
                    assert!(
                        text.is_char_boundary(span.range.start)
                            && text.is_char_boundary(span.range.end)
                    );
                    assert!(!text[span.range.clone()].contains('>'));
                    assert!(!text[span.range.clone()].contains("```"));
                }
            }
        }
    }

    #[test]
    fn dense_markdown_decorations_remain_bounded_and_ordered() {
        let text = "- **é**\n".repeat(10_000);
        assert!(text.len() < 128 * 1024);
        let started = std::time::Instant::now();
        let edits = decorations(&text, text.len()..text.len());
        eprintln!("10,000 decorated lines: {:?}", started.elapsed());
        assert_eq!(edits.len(), 30_000);
        assert!(
            edits
                .windows(2)
                .all(|pair| pair[0].0.end <= pair[1].0.start)
        );
        assert!(edits.iter().all(
            |(range, _)| text.is_char_boundary(range.start) && text.is_char_boundary(range.end)
        ));
        let tasks = "- [x] task\n".repeat(10_000);
        let edits = decorations(&tasks, tasks.len()..tasks.len());
        assert_eq!(edits.len(), 10_000);
        assert!(edits.iter().all(|(_, replacement)| replacement == "☑"));
        assert!(decorations(&"- **é**\n".repeat(20_000), 0..0).is_empty());
    }

    #[test]
    fn list_continuation_exit_and_code() {
        assert_eq!(newline_edit("9. item", 7), Some((7..7, "\n10. ".into())));
        assert_eq!(
            newline_edit("  - [x] done", 12),
            Some((12..12, "\n  - [ ] ".into()))
        );
        assert_eq!(newline_edit("- ", 2), Some((0..2, "".into())));
        assert_eq!(newline_edit("```\n- item", 10), None);
        assert!(in_code("say `/$", 7));
        assert!(!in_code("say \\` $", 8));
    }
    #[test]
    fn bullets_render_on_active_and_inactive_lines() {
        let text = "-\n- \n-";
        let edits = decorations(text, 5..6);
        assert_eq!(
            edits,
            vec![(0..1, "•".into()), (2..3, "•".into()), (5..6, "•".into())]
        );
        assert_eq!(newline_edit("-", 1), Some((0..1, String::new())));
        assert_eq!(decorations("- item", 0..6), vec![(0..1, "•".into())]);
        assert!(list_prefix("-word").is_none());
        assert!(decorations("```\n-\n```", 0..3).is_empty());
    }

    fn rendered(text: &str, active: Range<usize>) -> String {
        let mut result = text.to_owned();
        let edits = decorations(text, active);
        for pair in edits.windows(2) {
            assert!(pair[0].0.end <= pair[1].0.start);
        }
        for (range, replacement) in edits.into_iter().rev() {
            result.replace_range(range, &replacement);
        }
        result
    }

    #[test]
    fn bullets_do_not_depend_on_the_caret_line() {
        let text = "-\n- \n-";
        for active in [0..1, 2..4, 5..6] {
            assert_eq!(rendered(text, active), "•\n• \n•");
        }
        assert_eq!(
            rendered("* first\n+ second\n- third", 0..7),
            "• first\n• second\n• third"
        );
    }

    #[test]
    fn list_whitespace_and_empty_task_items_continue_consistently() {
        for (text, next) in [
            ("-\titem", "\n-\t"),
            ("12)\titem", "\n13)\t"),
            ("+   item", "\n+   "),
            ("- [X]\tdone", "\n- [ ] "),
            ("1. [x] done", "\n2. [ ] "),
        ] {
            assert_eq!(
                newline_edit(text, text.len()),
                Some((text.len()..text.len(), next.into())),
                "{text:?}"
            );
        }
        for text in ["- [ ]", "* [x] ", "1. [X]\t", "1."] {
            assert_eq!(
                newline_edit(text, text.len()),
                Some((0..text.len(), String::new())),
                "{text:?}"
            );
        }
        for text in ["-word", "1.word", "- [x]word"] {
            if let Some(prefix) = list_prefix(text) {
                assert_eq!(prefix.end, 2);
                assert_eq!(prefix.next, "- ");
            }
        }
    }

    #[test]
    fn checkbox_projection_matches_markdown_and_retains_editable_active_source() {
        let text = "- [x] done\n- [ ] pending\n- [x]word\n1. [X] ordered\nactive";
        assert_eq!(
            rendered(text, text.len() - 6..text.len()),
            "☑ done\n☐ pending\n• [x]word\n1. ☑ ordered\nactive"
        );
        assert_eq!(rendered("- [x] done", 0..10), "• [x] done");
        assert_eq!(rendered("-\t[x] done\nactive", 12..18), "☑ done\nactive");
    }

    #[test]
    fn indented_and_nested_code_preserve_literal_list_markers() {
        for text in [
            "    - shell\n\nactive",
            "    - [x] literal\n\nactive",
            "- item\n\n      - shell\n\nactive",
            "```sh\n- [x] literal\n```\nactive",
        ] {
            let active = text.rfind("active").unwrap();
            let result = rendered(text, active..text.len());
            assert!(
                result.contains("- shell") || result.contains("- [x] literal"),
                "{result:?}"
            );
        }
        assert!(!is_thematic_break("-\u{a0}-\u{a0}-"));
    }

    #[test]
    fn headings_use_bold_faces_and_hide_only_inactive_atx_markers() {
        for heading in ["# Café", "  ### Café ###", "###### Café\t##"] {
            let text = format!("{heading}\nactive");
            let active = heading.len() + 1..text.len();
            let result = rendered(&text, active);
            assert_eq!(result.trim_start(), "Café\nactive", "{heading:?}");
            assert_eq!(rendered(&text, 0..heading.len()), text);
            assert!(
                faces(&text).iter().any(
                    |(range, face)| *face == Face::Bold && text[range.clone()].contains("Café")
                )
            );
        }
        for heading in ["# Café###", "# Café \\###", "# **Café** ###"] {
            let text = format!("{heading}\nactive");
            let result = rendered(&text, heading.len() + 1..text.len());
            assert!(result.contains("Café"));
            if heading != "# **Café** ###" {
                assert!(result.contains("###"));
            } else {
                assert_eq!(result, "Café\nactive");
            }
        }
        for (text, expected) in [
            ("> ## Café ##\n\nactive", "> Café\n\nactive"),
            ("- ## Café ##\n\nactive", "• Café\n\nactive"),
            ("# ###\nactive", "\nactive"),
            ("# Café ###\r\nactive", "Café\r\nactive"),
        ] {
            assert_eq!(
                rendered(text, text.rfind("active").unwrap()..text.len()),
                expected
            );
        }
        let setext = "Café\n====\nactive";
        assert_eq!(rendered(setext, 11..setext.len()), setext);
        assert!(faces(setext).iter().any(|(_, face)| *face == Face::Bold));
        for literal in [
            "---\nactive",
            "    # code\n\nactive",
            "```\n# code\n```\nactive",
            "####### literal\nactive",
        ] {
            assert!(!faces(literal).iter().any(|(_, face)| *face == Face::Bold));
        }
    }

    #[test]
    fn strikethrough_hides_delimiters_and_preserves_nested_unicode_source() {
        let text = "~~café **bold**~~ and ~~_italic_~~\nactive";
        assert_eq!(
            rendered(text, text.rfind('\n').unwrap() + 1..text.len()),
            "café bold and italic\nactive"
        );
        assert_eq!(rendered(text, 0..text.find('\n').unwrap()), text);
        assert_eq!(
            faces(text)
                .iter()
                .filter(|(_, face)| *face == Face::Strikethrough)
                .count(),
            2
        );
        for literal in ["\\~\\~literal\\~\\~", "`~~code~~`", "```\n~~code~~\n```"] {
            assert!(
                !faces(literal)
                    .iter()
                    .any(|(_, face)| *face == Face::Strikethrough)
            );
        }
    }

    #[test]
    fn setext_underlines_are_never_projected_as_bullets() {
        for underline in ["-", " -", "  -", "---", "==="] {
            let text = format!("Title\n{underline}\n\nactive");
            for active in [0..5, 6..6 + underline.len(), text.len() - 6..text.len()] {
                assert_eq!(rendered(&text, active), text, "{underline:?}");
            }
            assert_eq!(newline_edit(&text, 6 + underline.len()), None);
        }
        assert_eq!(rendered("-\nactive", 2..8), "•\nactive");
    }

    #[test]
    fn crlf_lists_preserve_line_endings_and_empty_item_semantics() {
        assert_eq!(rendered("-\r\n\r\nactive", 5..11), "•\r\n\r\nactive");
        for line in ["-", "- ", "- [ ]", "1. [x]"] {
            let text = format!("{line}\r\nnext");
            assert_eq!(
                newline_edit(&text, line.len()),
                Some((0..line.len(), String::new()))
            );
        }
        for (line, next) in [
            ("- item", "- "),
            ("- [x] done", "- [ ] "),
            ("1. item", "2. "),
        ] {
            let text = format!("{line}\r\nnext");
            assert_eq!(
                newline_edit(&text, line.len()),
                Some((line.len()..line.len(), format!("\r\n{next}")))
            );
        }
    }

    #[test]
    fn quoted_fence_closure_and_tight_list_inline_scopes() {
        for (text, code) in [
            ("> ```rust\n> fn x() {}\n> ```", false),
            ("> > ```rust\n> > fn x() {}\n> > ```", false),
            ("> ```rust\n> fn x() {}", true),
            ("> > ````rust\n> > fn x() {}\n> > ```", true),
            ("```rust\nfn x() {}\n    ```", true),
            ("> ```rust\n> fn x() {}\n>     ```", true),
            ("- ```rust\n  fn x() {}\n      ```", true),
            ("- `unfinished\n- $skill", false),
            ("- `unfinished\n  - @file", false),
            ("- `unfinished\n  soft $skill", true),
        ] {
            assert_eq!(in_code(text, text.len()), code, "{text:?}");
        }
    }

    #[test]
    fn quoted_lists_project_only_their_list_markers() {
        let text =
            "> - first\n> - [x] done\n> > + [ ] nested\n>> 2. [x] ordered\n>  - child\n\nactive";
        assert_eq!(
            rendered(text, text.rfind("active").unwrap()..text.len()),
            "> • first\n> ☑ done\n> > ☐ nested\n>> 2. ☑ ordered\n>  • child\n\nactive"
        );
        assert_eq!(rendered("> - [x] done", 0..12), "> • [x] done");
        assert_eq!(
            rendered(">\t- item\r\n\r\nactive", 14..20),
            ">\t• item\r\n\r\nactive"
        );
        for text in [
            "> ---\n\nactive",
            "> Title\n> -\n\nactive",
            ">     - literal\n\nactive",
            "> ```sh\n> - [x] literal\n> ```\n\nactive",
            "    > - literal\n\nactive",
            "\\> - literal\n\nactive",
        ] {
            assert_eq!(
                rendered(text, text.rfind("active").unwrap()..text.len()),
                text,
                "{text:?}"
            );
        }
    }

    #[test]
    fn quoted_list_continuation_preserves_containers_and_empty_exit() {
        for (text, next) in [
            ("> - item", "\n> - "),
            ("> > - [x] done", "\n> > - [ ] "),
            (">> 9) item", "\n>> 10) "),
            ("  > - item", "\n  > - "),
            (">   - child", "\n>   - "),
            (">\t- item", "\n>\t- "),
        ] {
            assert_eq!(
                newline_edit(text, text.len()),
                Some((text.len()..text.len(), next.into())),
                "{text:?}"
            );
        }
        for (text, retained) in [
            ("> - ", "> "),
            (">> -", ">> "),
            ("> > - [ ]", "> > "),
            ("> 2. [x] ", "> "),
            (">   - ", "> "),
            (">     - ", ">   "),
        ] {
            // Provide a parent list so four-space nested indentation remains
            // list content rather than an indented code block.
            let source = format!("> - parent\n>\n{text}");
            let start = source.len() - text.len();
            assert_eq!(
                newline_edit(&source, source.len()),
                Some((start..source.len(), retained.into())),
                "{text:?}"
            );
        }
        let text = "> - [x] done\r\n> next";
        let cursor = text.find('\r').unwrap();
        assert_eq!(
            newline_edit(text, cursor),
            Some((cursor..cursor, "\r\n> - [ ] ".into()))
        );
        let empty = "> - [ ]\r\n> next";
        let cursor = empty.find('\r').unwrap();
        assert_eq!(newline_edit(empty, cursor), Some((0..cursor, "> ".into())));
        for text in [
            "> ---",
            "> Title\n> -",
            ">     - literal",
            "> ```\n> - literal",
            "    > - literal",
        ] {
            assert_eq!(newline_edit(text, text.len()), None, "{text:?}");
        }
        let prefix = list_prefix("> >   - [x] task").unwrap();
        assert_eq!((prefix.indent_start, prefix.indent, prefix.end), (4, 6, 12));
    }

    #[test]
    fn decoration_keeps_active_syntax_editable() {
        let text = "**bold**\n- item\nactive";
        assert!(decorations(text, 0..8).iter().all(|(r, _)| r.start >= 9));
        let edits = decorations(text, 16..22);
        assert!(edits.contains(&(0..2, String::new())));
        assert!(edits.contains(&(9..10, "•".into())));
    }
}
