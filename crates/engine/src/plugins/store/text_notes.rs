//! What the stores show as typed: a README's or CHANGELOG's raw HTML and
//! footnotes, and on the website display math. Both stores draw one subset
//! of Markdown, and an author learns
//! what falls outside it from the publish check (advice, never a failure),
//! not from their own page.
//!
//! A linear scan, line by line, outside code (fenced blocks, indented
//! blocks, inline spans): no Markdown parser runs over an author's text.

use crate::plugins::listing::Problem;

/// A README or CHANGELOG with raw HTML (`<p align="center">`, `<details>`,
/// `<img>`, `<br>`, a comment): shown as the characters typed.
pub const RAW_HTML: &str = "raw_html";
/// One with a footnote (`[^1]`, or its `[^1]: …`): shown as typed.
pub const FOOTNOTE: &str = "footnote";
/// One with display math (a `$$` line opening a block, or a fence whose
/// info string is `math`): the website shows it as typed, Zeron draws it.
/// (Inline `$…$` is never noted: prices and shell variables read alike.)
pub const MATH: &str = "math";

/// Elements a README might use, as a tag's name is matched (lowercase):
/// prose with a `<` (`a < b`, `<Ctrl>`) names none of them.
const ELEMENTS: &[&str] = &[
    "a",
    "abbr",
    "article",
    "aside",
    "audio",
    "b",
    "big",
    "blockquote",
    "br",
    "button",
    "caption",
    "center",
    "cite",
    "code",
    "col",
    "colgroup",
    "dd",
    "del",
    "details",
    "div",
    "dl",
    "dt",
    "em",
    "figcaption",
    "figure",
    "font",
    "footer",
    "form",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "hr",
    "i",
    "iframe",
    "img",
    "input",
    "ins",
    "kbd",
    "label",
    "li",
    "link",
    "main",
    "mark",
    "meta",
    "nav",
    "ol",
    "p",
    "picture",
    "pre",
    "q",
    "s",
    "samp",
    "script",
    "section",
    "small",
    "source",
    "span",
    "strike",
    "strong",
    "style",
    "sub",
    "summary",
    "sup",
    "svg",
    "table",
    "tbody",
    "td",
    "tfoot",
    "th",
    "thead",
    "tr",
    "tt",
    "u",
    "ul",
    "var",
    "video",
    "wbr",
];

/// `file`'s notes for `text`: one each for raw HTML, footnotes and display
/// math, naming the first line it was seen on.
pub fn notes(file: &str, text: &str) -> Vec<Problem> {
    let mut html: Option<usize> = None;
    let mut footnote: Option<usize> = None;
    let mut math: Option<usize> = None;
    let mut fence: Option<(u8, usize)> = None;
    for (index, line) in text.lines().enumerate() {
        let number = index + 1;
        let indent = line.len() - line.trim_start_matches(' ').len();
        let body = line.trim_start_matches(' ');
        // Fenced code: an opening run of 3+ ` or ~, closed by a run of the
        // same character at least as long.
        if indent < 4
            && let Some((mark, run)) = fence_run(body)
        {
            match fence {
                None => {
                    if math.is_none() && body[run..].trim().eq_ignore_ascii_case("math") {
                        math = Some(number);
                    }
                    fence = Some((mark, run));
                    continue;
                }
                Some((open, len))
                    if open == mark && run >= len && body[run..].trim().is_empty() =>
                {
                    fence = None;
                    continue;
                }
                Some(_) => {}
            }
        }
        if fence.is_some() || indent >= 4 || line.starts_with('\t') {
            continue;
        }
        let prose = outside_inline_code(body);
        if html.is_none() && has_html(&prose) {
            html = Some(number);
        }
        if footnote.is_none() && has_footnote(&prose) {
            footnote = Some(number);
        }
        // (A `$$` in a code span is blanked out of `prose` already.)
        if math.is_none() && prose.trim_start().starts_with("$$") {
            math = Some(number);
        }
        if html.is_some() && footnote.is_some() && math.is_some() {
            break;
        }
    }
    let note = |rule: &'static str, line: usize, words: String| Problem {
        rule,
        field: None,
        path: Some(file.to_owned()),
        line: Some(line),
        words,
    };
    let as_text = |line: usize, what: &str, instead: &str| {
        format!("{file} line {line} has {what}; the store shows it as text. {instead}")
    };
    let mut out = Vec::new();
    if let Some(line) = html {
        out.push(note(
            RAW_HTML,
            line,
            as_text(line, "HTML", "Use Markdown for pictures and headings."),
        ));
    }
    if let Some(line) = footnote {
        out.push(note(
            FOOTNOTE,
            line,
            as_text(line, "a footnote", "Write the note into the text instead."),
        ));
    }
    if let Some(line) = math {
        out.push(note(
            MATH,
            line,
            format!("{file} line {line} has math; the store's website shows it as typed (Zeron itself draws it)."),
        ));
    }
    out
}

/// A code fence's character and run length, if `line` opens or closes one.
fn fence_run(line: &str) -> Option<(u8, usize)> {
    let mark = *line.as_bytes().first()?;
    if mark != b'`' && mark != b'~' {
        return None;
    }
    let run = line.bytes().take_while(|b| *b == mark).count();
    // (A backtick fence's info string has no backtick.)
    (run >= 3 && !(mark == b'`' && line[run..].contains('`'))).then_some((mark, run))
}

/// `line` with each inline code span (a run of backticks to the next run
/// of the same length) blanked out; an unclosed run is text.
fn outside_inline_code(line: &str) -> String {
    let bytes = line.as_bytes();
    let mut out = String::with_capacity(line.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'`' {
            let next = line[i..].find('`').map_or(line.len(), |at| i + at);
            out.push_str(&line[i..next]);
            i = next;
            continue;
        }
        let run = bytes[i..].iter().take_while(|b| **b == b'`').count();
        let mut j = i + run;
        let mut closed = None;
        while j < bytes.len() {
            if bytes[j] == b'`' {
                let other = bytes[j..].iter().take_while(|b| **b == b'`').count();
                if other == run {
                    closed = Some(j + other);
                    break;
                }
                j += other;
            } else {
                j += 1;
            }
        }
        match closed {
            Some(end) => {
                out.push(' ');
                i = end;
            }
            None => {
                out.push_str(&line[i..i + run]);
                i += run;
            }
        }
    }
    out
}

/// Whether `text` opens or closes an element a README might use (its tag
/// closed on the line), or holds an HTML comment.
fn has_html(text: &str) -> bool {
    let bytes = text.as_bytes();
    (0..bytes.len())
        .any(|i| bytes[i] == b'<' && (text[i..].starts_with("<!--") || tag_at(&text[i + 1..])))
}

/// After a `<`: `/`? a known element's name, then `>` or `/>`, or attributes
/// (each a name then `=`, or a lone name before `>` or `/>`) up to `>`.
fn tag_at(rest: &str) -> bool {
    let rest = rest.strip_prefix('/').unwrap_or(rest);
    let name_len = rest
        .bytes()
        .take_while(|b| b.is_ascii_alphanumeric())
        .count();
    if name_len == 0 || !ELEMENTS.contains(&rest[..name_len].to_ascii_lowercase().as_str()) {
        return false;
    }
    let after = &rest[name_len..];
    if after.starts_with('>') || after.starts_with("/>") {
        return true;
    }
    if !after.starts_with([' ', '\t']) {
        return false;
    }
    // Its first attribute: a name, then `=`, `>` or `/>` (`<details open>`,
    // `<img src="x">`), never a word of prose (`a<b and c>d`).
    let attrs = after.trim_start();
    if attrs.starts_with('>') || attrs.starts_with("/>") {
        return true;
    }
    let attr_len = attrs
        .bytes()
        .take_while(|b| b.is_ascii_alphanumeric() || *b == b'-' || *b == b':')
        .count();
    attr_len > 0 && {
        let next = attrs[attr_len..].trim_start();
        (next.starts_with('=') || next.starts_with('>') || next.starts_with("/>"))
            && attrs.contains('>')
    }
}

/// Whether `text` holds a footnote reference or definition: `[^label]`,
/// its label at least one character, no space or `]` in it.
fn has_footnote(text: &str) -> bool {
    text.match_indices("[^").any(|(at, _)| {
        let label: &str = &text[at + 2..];
        let len = label.find(']').unwrap_or(0);
        len > 0 && !label[..len].contains(char::is_whitespace)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn said(text: &str) -> Vec<(&'static str, String)> {
        notes("README.md", text)
            .into_iter()
            .map(|p| (p.rule, p.words))
            .collect()
    }

    #[test]
    fn raw_html_is_noted_once_at_its_first_line() {
        let text =
            "# Focus\n\nWhat you're on.\n<p align=\"center\"><img src=\"shot.png\"></p>\n<br>\n";
        assert_eq!(
            said(text),
            [(RAW_HTML, "README.md line 4 has HTML; the store shows it as text. Use Markdown for pictures and headings.".to_owned())]
        );
        for html in [
            "<details>",
            "<details open>",
            "</details>",
            "<br/>",
            "<br />",
            "<img src='a.png' alt=\"A\">",
            "<!-- hidden -->",
            "<SUMMARY>Click</SUMMARY>",
            "x <kbd>Ctrl</kbd> y",
        ] {
            assert_eq!(
                said(html).first().map(|(rule, _)| *rule),
                Some(RAW_HTML),
                "{html}"
            );
        }
    }

    #[test]
    fn footnotes_are_noted_once_at_their_first_line() {
        assert_eq!(
            said("# Focus\n\nIt's fast.[^1]\n\n[^1]: Mostly.\n"),
            [(FOOTNOTE, "README.md line 3 has a footnote; the store shows it as text. Write the note into the text instead.".to_owned())]
        );
        assert_eq!(said("[^note]: A definition alone.").len(), 1);
        // Both, each once.
        let both = said("a[^1] b <br>\n<hr>\n[^2]\n");
        assert_eq!(
            both.iter().map(|(rule, _)| *rule).collect::<Vec<_>>(),
            [RAW_HTML, FOOTNOTE]
        );
    }

    #[test]
    fn display_math_is_noted_once_at_its_first_line() {
        let math = |text: &str| {
            said(text)
                .into_iter()
                .filter(|(rule, _)| *rule == MATH)
                .map(|(_, words)| words)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            math("# Focus\n\nIt sums:\n\n$$\nx^2 + y^2\n$$\n\n$$ a = b $$\n"),
            ["README.md line 5 has math; the store's website shows it as typed (Zeron itself draws it).".to_owned()]
        );
        assert_eq!(
            math("Area:\n\n```math\n\\pi r^2\n```\n"),
            ["README.md line 3 has math; the store's website shows it as typed (Zeron itself draws it).".to_owned()]
        );
        assert_eq!(math("~~~ Math\nx\n~~~").len(), 1);
        // Never inline dollars, nor `$$` in code.
        for text in [
            "It costs $5 and $10.",
            "Set $HOME, then run `echo $PATH`.",
            "Totals: $$ are on us.",
            "```\n$$\nx\n$$\n```",
            "Write `$$` around it.",
            "    $$ indented code",
            "```js\nconst math = 1;\n```",
        ] {
            assert_eq!(math(text), Vec::<String>::new(), "{text:?}");
        }
    }

    #[test]
    fn a_less_than_in_prose_or_in_code_is_neither() {
        for text in [
            "If a < b, it waits.",
            "a<b and c>d",
            "Press <Ctrl> and <Shift>.",
            "Arrows: <- and ->, and 3 <= 4.",
            "Mail <ada@example.com> or see <https://example.com>.",
            "Use `<details>` for that, or ``a `<br>` b``.",
            "```html\n<p align=\"center\">\n<br>\n```",
            "~~~~\n<details>\n```\n<br>\n~~~~",
            "    <div>indented code</div>",
            "\t<div>tab-indented code</div>",
            "An array: x[^1 0] and a caret [^ ].",
            "`[^1]` in code",
            "<customtag> isn't HTML anyone draws",
        ] {
            assert_eq!(said(text), [], "{text:?}");
        }
        // Past a closed fence, it's read again.
        assert_eq!(
            said("```\n<br>\n```\n<br>")
                .first()
                .map(|(_, w)| w.clone())
                .unwrap_or_default(),
            "README.md line 4 has HTML; the store shows it as text. Use Markdown for pictures and headings."
        );
        // An unclosed backtick is text.
        assert_eq!(said("a ` <br> b").len(), 1);
    }
}
