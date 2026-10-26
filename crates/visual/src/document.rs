//! Visuals embedded in Markdown documents as ```` ```zeron-visual ```` fences.
//!
//! Uses the Markdown preview's parser so `zeron visual check` finds the same
//! scripts, seeds them identically, and maps errors back to document lines.

/// The fence language that marks a visual (case-insensitive).
pub const FENCE_LANGUAGE: &str = "zeron-visual";

pub fn is_fence_language(language: &str) -> bool {
    language.eq_ignore_ascii_case(FENCE_LANGUAGE)
}

/// One visual fence in a document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fence {
    /// The 1-based line of the opening fence: script line `n` sits on
    /// document line `line + n`.
    pub line: usize,
    pub code: String,
    /// Its position among fences with identical code (see
    /// [`crate::seed_for_document`]).
    pub occurrence: usize,
}

/// Every visual fence in `markdown`, in document order, with the same code
/// as the Markdown preview. Other fences, HTML, and indented code are handled
/// by the shared Markdown parser.
pub fn fences(markdown: &str) -> Vec<Fence> {
    let mut seen = std::collections::HashMap::<String, usize>::new();
    zeron_markdown::parser::fenced_code_blocks(markdown)
        .into_iter()
        .filter(|fence| fence.language.as_deref().is_some_and(is_fence_language))
        .map(|fence| {
            let occurrence = seen.entry(fence.code.clone()).or_default();
            let visual = Fence {
                line: fence.line,
                code: fence.code,
                occurrence: *occurrence,
            };
            *occurrence += 1;
            visual
        })
        .collect()
}

/// Where a script error points in the document: the line of its first
/// `visual:N:` position (the fence line when it has none) and the message
/// without that position. A Luau traceback only lends its position: the
/// message stops where the traceback starts.
pub fn locate(message: &str, fence_line: usize) -> (usize, String) {
    let (head, traceback) = message
        .split_once("\nstack traceback:")
        .unwrap_or((message, ""));
    if let Some((at, len, line)) = position(head) {
        let after = head[at + len..].trim_start();
        return (fence_line + line, format!("{}{after}", &head[..at]));
    }
    let line = position(traceback).map_or(0, |(_, _, line)| line);
    (fence_line + line, head.trim_end().to_owned())
}

/// The first `visual:N:` in `text`: its offset, length, and N.
fn position(text: &str) -> Option<(usize, usize, usize)> {
    let mut search = 0;
    while let Some(found) = text[search..].find("visual:") {
        let at = search + found;
        let rest = &text[at + "visual:".len()..];
        let digits = rest.chars().take_while(char::is_ascii_digit).count();
        if digits > 0
            && rest[digits..].starts_with(':')
            && let Ok(line) = rest[..digits].parse::<usize>()
        {
            return Some((at, "visual:".len() + digits + 1, line));
        }
        search = at + "visual:".len();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_backtick_and_tilde_fences_with_their_lines() {
        let doc = "# Doc\n\n```zeron-visual\nlocal a = 1\nreturn a\n```\n\ntext\n\n~~~ Zeron-Visual extra\nline 1\n~~~\n";
        let found = fences(doc);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].line, 3);
        assert_eq!(found[0].code, "local a = 1\nreturn a");
        assert_eq!(found[1].line, 10);
        assert_eq!(found[1].code, "line 1");
    }

    #[test]
    fn other_fences_hide_what_they_contain() {
        let doc = "````markdown\n```zeron-visual\nnot a visual\n```\n````\n```lua\nx\n```\n";
        assert!(fences(doc).is_empty());
    }

    #[test]
    fn nested_fences_lose_their_container_indentation() {
        let doc = "- item\n\n  ```zeron-visual\n  local a = 1\n    nested()\n  ```\n\n> ```zeron-visual\n> quoted()\n> ```\n";
        let found = fences(doc);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].line, 3);
        assert_eq!(found[0].code, "local a = 1\n  nested()");
        assert_eq!(found[1].line, 8);
        assert_eq!(found[1].code, "quoted()");
    }

    #[test]
    fn list_markers_on_the_fence_line_are_containers() {
        let doc = "1. ```zeron-visual\n   one()\n   ```\n";
        let found = fences(doc);
        assert_eq!(found[0].code, "one()");
    }

    #[test]
    fn tab_indented_list_fences_are_visuals() {
        let found = fences("- item\n\n\t```zeron-visual\n\treturn ui.nope {}\n\t```\n");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].line, 3);
        assert_eq!(found[0].code, "return ui.nope {}");
    }

    #[test]
    fn html_and_indented_code_do_not_open_visuals() {
        for source in [
            "<!--\n```zeron-visual\nreturn nil\n```\n-->\n",
            "<div>\n```zeron-visual\nreturn nil\n```\n</div>\n",
            "    ```zeron-visual\n    return nil\n    ```\n",
        ] {
            assert!(fences(source).is_empty(), "{source:?}");
        }
    }

    #[test]
    fn a_list_ending_also_ends_its_unclosed_fence() {
        let found = fences("- ```zeron-visual\n  first()\n\n```zeron-visual\nsecond()\n```\n");
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].line, 1);
        assert_eq!(found[0].code, "first()\n");
        assert_eq!(found[1].line, 4);
        assert_eq!(found[1].code, "second()");
    }

    #[test]
    fn crlf_blank_lines_and_unclosed_fences() {
        let doc = "```zeron-visual\r\n\r\nfirst()\r\n```\r\n```zeron-visual\nlast()";
        let found = fences(doc);
        assert_eq!(found[0].code, "\nfirst()");
        assert_eq!(found[1].code, "last()");
        assert_eq!(found[1].line, 5);
    }

    #[test]
    fn identical_fences_count_occurrences() {
        let doc = "```zeron-visual\nx\n```\n```zeron-visual\ny\n```\n```zeron-visual\nx\n```\n";
        let found = fences(doc);
        let occurrences: Vec<usize> = found.iter().map(|f| f.occurrence).collect();
        assert_eq!(occurrences, [0, 0, 1]);
    }

    #[test]
    fn errors_map_to_document_lines() {
        assert_eq!(
            locate("visual:3: attempt to call a nil value", 10),
            (13, "attempt to call a nil value".into())
        );
        assert_eq!(
            locate("after selecting tab 'b': visual:5: boom", 2),
            (7, "after selecting tab 'b': boom".into())
        );
        assert_eq!(
            locate("the script used more than 64 MB", 4),
            (4, "the script used more than 64 MB".into())
        );
        assert_eq!(locate("visual:x: odd", 1), (1, "visual:x: odd".into()));
        assert_eq!(
            locate(
                "attempt to call a nil value\nstack traceback:\n\t[C]: in ?\n\tvisual:4: in function <visual:1>",
                31
            ),
            (35, "attempt to call a nil value".into())
        );
    }
}
