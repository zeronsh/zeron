//! Source-aware Markdown editing helpers. The draft is always plain Markdown.
//! The editing rules live in `zeron_markdown::live`, shared with the iOS
//! notes screen; code highlighting stays here with the syntax crate.
use pulldown_cmark::{Event, Parser, Tag};
use std::ops::Range;

pub use zeron_markdown::live::*;

/// Highlight fenced code with its own grammar. Running the Markdown grammar
/// over the whole draft colors fence bodies as strings, including identifiers.
/// Keep prose and fence markers neutral, and retain exact source byte offsets.
pub fn syntax_spans(text: &str) -> Vec<zeron_syntax::HighlightSpan> {
    if text.len() > LIVE_MAX_BYTES {
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
            max_source_bytes: LIVE_MAX_BYTES,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn injected_colors_use_source_offsets_after_unicode_and_mentions() {
        let text = "héllo [file](zeron-file:src/main.rs)\n```rust\nfn main() { let café = 42; }\n```\n```python\ndef hello(): pass\n```";
        let spans = syntax_spans(text);
        assert!(spans.iter().any(|span| &text[span.range.clone()] == "fn"
            && span.kind == zeron_syntax::HighlightKind::Keyword));
        assert!(spans.iter().any(|span| &text[span.range.clone()] == "def"
            && span.kind == zeron_syntax::HighlightKind::Keyword));
        assert!(syntax_spans(&"x".repeat(LIVE_MAX_BYTES + 1)).is_empty());
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
}
