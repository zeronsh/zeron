//! Long pastes as attachments: the composer stages a paste past
//! [`MIN_CHARS`] as a chip instead of flooding the input, the sent prompt
//! carries each one as a trailing `<pasted-text>` block the agent reads, and
//! [`extract_badge`] turns those blocks back into a chip in the transcript.

use gpui::{div, prelude::*, px};

/// Shorter pastes go into the input as typed text.
pub const MIN_CHARS: usize = 1_500;

const OPEN: &str = "\n\n<pasted-text>\n";
const CLOSE: &str = "\n</pasted-text>";
/// The words a prompt carries when the pastes are all there is.
const ONLY_TEXT: &str = "See the pasted text below.";

/// One label for every paste in a message, on the composer's chip and the
/// sent message's alike.
pub fn label(blocks: &[impl AsRef<str>]) -> String {
    match blocks {
        [one] => format!(
            "Pasted text · {} chars",
            crate::context_usage::with_separators(one.as_ref().chars().count() as u64)
        ),
        many => format!("{} pasted texts", many.len()),
    }
}

/// The pastes' chip, drawn like the composer's other chips.
pub fn chip(label: &str, theme: &crate::theme::Theme) -> gpui::Div {
    div()
        .flex_none()
        .text_size(px(crate::composer::INPUT_TEXT_SIZE))
        .line_height(px(crate::badges::BADGE_HEIGHT))
        .text_color(theme.text)
        .child(crate::composer::chip_pill(
            crate::composer::ChipKind::Paste,
            label,
            theme,
        ))
}

pub fn is_long(text: &str) -> bool {
    text.chars().nth(MIN_CHARS - 1).is_some()
}

/// `text` with every paste appended, in order. Folded before review comments,
/// whose block must stay last.
pub fn with_pasted(text: &str, pasted: &[String]) -> String {
    if pasted.is_empty() {
        return text.to_string();
    }
    let mut out = if text.trim().is_empty() {
        ONLY_TEXT.to_string()
    } else {
        text.to_string()
    };
    for paste in pasted {
        out.push_str(OPEN);
        out.push_str(paste);
        out.push_str(CLOSE);
    }
    out
}

/// [`crate::badges::Extractor`] for trailing paste blocks.
pub fn extract_badge(text: &str) -> Option<(String, crate::badges::MessageBadge)> {
    let mut rest = text;
    let mut blocks = Vec::new();
    while let Some(body) = rest.strip_suffix(CLOSE) {
        let Some(at) = body.rfind(OPEN) else {
            break;
        };
        blocks.push(&body[at + OPEN.len()..]);
        rest = &body[..at];
    }
    if blocks.is_empty() {
        return None;
    }
    blocks.reverse();
    let rest = if rest == ONLY_TEXT { "" } else { rest };
    Some((
        rest.to_string(),
        crate::badges::MessageBadge {
            icon: crate::icons::DOCUMENT,
            label: label(&blocks).into(),
            details: Vec::new(),
            full: blocks.join("\n\n").into(),
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pastes_round_trip_into_one_chip() {
        let staged = ["x".repeat(12_400), "second\nblock".to_string()];
        let sent = with_pasted("summarize these", &staged);
        assert!(sent.starts_with("summarize these\n\n<pasted-text>\n"));
        let (rest, chip) = extract_badge(&sent).unwrap();
        assert_eq!(rest, "summarize these");
        assert_eq!(chip.label, "2 pasted texts");
        assert_eq!(chip.full, staged.join("\n\n"));
        assert_eq!(label(&staged[..1]), "Pasted text · 12,400 chars");
    }

    #[test]
    fn a_paste_alone_is_a_message_and_shows_no_filler() {
        let sent = with_pasted("  ", &["only this".into()]);
        let (rest, chip) = extract_badge(&sent).unwrap();
        assert_eq!(rest, "");
        assert_eq!(chip.label, "Pasted text · 9 chars");
    }

    #[test]
    fn review_comments_still_trail_the_pastes() {
        let comment = crate::comments::ReviewComment::file("src/a.rs", 3, "fix this");
        let sent = crate::comments::with_comments(&with_pasted("go", &["log".into()]), &[comment]);
        let (badges_text, badges) = crate::badges::split(&sent);
        assert_eq!(badges_text, "go");
        assert_eq!(badges.len(), 2);
    }

    #[test]
    fn plain_messages_and_quoted_tags_are_left_alone() {
        assert!(extract_badge("no pastes here").is_none());
        assert!(extract_badge("talking about </pasted-text>").is_none());
        assert!(!is_long(&"y".repeat(MIN_CHARS - 1)));
        assert!(is_long(&"y".repeat(MIN_CHARS)));
    }
}
