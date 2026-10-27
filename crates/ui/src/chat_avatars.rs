//! The chats working for a task, drawn as a row of provider squircles: up to
//! three, or two and a "+N" tile for the rest (the session card's Tasks lane
//! and the transcript's task card).
//!
//! A tile wears a small status dot only when its chat needs someone: asking
//! a question (the accent, as everywhere a chat waits on you) or failed.

use gpui::{Hsla, ParentElement, SharedString, Styled, div, px};
use zeron_proto::{ChatIndicator, HarnessId};

use crate::icons::icon;
use crate::theme::Theme;

/// Tiles a row draws; past this, the last one counts the rest.
pub const SHOWN: usize = 3;

/// One chat in a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Avatar {
    pub harness: Option<HarnessId>,
    pub status: ChatIndicator,
}

/// The status dot a tile wears, if any.
fn dot(status: ChatIndicator, theme: &Theme) -> Option<Hsla> {
    match status {
        ChatIndicator::AwaitingInput => Some(theme.accent),
        ChatIndicator::Errored => Some(theme.danger),
        _ => None,
    }
}

/// How many tiles `count` chats draw, and how many of them are chats (the
/// rest fold into a "+N" tile).
pub fn layout(count: usize) -> (usize, usize) {
    if count > SHOWN {
        (SHOWN, SHOWN - 1)
    } else {
        (count, count)
    }
}

/// The corner radius that reads as a squircle at `size`.
pub fn radius(size: f32) -> f32 {
    (size * 0.3).round()
}

/// A provider mark's size inside a tile `size` across: even, so it centers
/// on whole pixels.
pub fn mark_size(size: f32) -> f32 {
    (size * 0.31).round() * 2.0
}

/// `avatars` as squircles `size` across, side by side. `ground` is the
/// surface beneath, which rings a status dot.
pub fn stack(avatars: &[Avatar], size: f32, ground: Hsla, theme: &Theme) -> gpui::Div {
    let (_, shown) = layout(avatars.len());
    let rest = avatars.len() - shown;
    let mut row = div()
        .flex_none()
        .flex()
        .items_center()
        .gap(px((size / 6.0).round()));
    for avatar in &avatars[..shown] {
        row = row.child(
            tile(size, theme)
                .relative()
                .child(mark(avatar.harness, size, theme))
                .children(dot(avatar.status, theme).map(|color| status_dot(size, color, ground))),
        );
    }
    if rest > 0 {
        row = row.child(
            tile(size, theme)
                .text_size(px((size * 0.5).round()))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(theme.text_muted)
                .child(SharedString::from(format!("+{rest}"))),
        );
    }
    row
}

/// One squircle tile `size` across, its content centered.
pub fn tile(size: f32, _theme: &Theme) -> gpui::Div {
    div()
        .flex_none()
        .size(px(size))
        .rounded(px(radius(size)))
        .flex()
        .items_center()
        .justify_center()
        .bg(crate::theme::ink(0.07))
}

/// `harness`'s mark at its size for a tile `size` across.
pub fn mark(harness: Option<HarnessId>, size: f32, theme: &Theme) -> gpui::Svg {
    let (path, tint) = match harness {
        Some(harness) => crate::pickers::harness_brand_icon(harness),
        None => (crate::icons::CHAT_ROUND_LINE, None),
    };
    icon(path)
        .flex_none()
        .size(px(mark_size(size)))
        .text_color(tint.unwrap_or(theme.text_muted))
}

fn status_dot(size: f32, color: Hsla, ground: Hsla) -> gpui::Div {
    let dot = (size * 0.36).round().max(6.0);
    div()
        .absolute()
        .right(px(-2.0))
        .bottom(px(-2.0))
        .size(px(dot))
        .rounded_full()
        .bg(color)
        .border_1()
        .border_color(ground)
}

/// "1 needs input · 2 working · 1 done": what a row of `statuses` is doing,
/// for a tooltip or a line beside it.
pub fn summary(statuses: impl IntoIterator<Item = ChatIndicator>) -> String {
    let mut counts = [0usize; 4];
    for status in statuses {
        let slot = match status {
            ChatIndicator::AwaitingInput => 0,
            ChatIndicator::Errored => 1,
            ChatIndicator::Working => 2,
            ChatIndicator::Completed | ChatIndicator::Idle => 3,
        };
        counts[slot] += 1;
    }
    let labels = ["needs input", "failed", "working", "done"];
    let parts: Vec<String> = counts
        .iter()
        .zip(labels)
        .filter(|(count, _)| **count > 0)
        .map(|(count, label)| {
            if label == "needs input" && *count > 1 {
                format!("{count} need input")
            } else {
                format!("{count} {label}")
            }
        })
        .collect();
    parts.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_leads_with_what_needs_you() {
        use ChatIndicator::*;
        assert_eq!(
            summary([Working, AwaitingInput, Working, Completed]),
            "1 needs input · 2 working · 1 done"
        );
        assert_eq!(summary([AwaitingInput, AwaitingInput]), "2 need input");
        assert_eq!(summary([]), "");
    }

    #[test]
    fn four_or_more_fold_the_third_tile_into_a_count() {
        assert_eq!(layout(0), (0, 0));
        assert_eq!(layout(3), (3, 3));
        // Four: two marks and "+2".
        assert_eq!(layout(4), (3, 2));
        assert_eq!(layout(9), (3, 2));
    }

    #[test]
    fn marks_center_on_whole_pixels() {
        for size in [14.0, 16.0, 18.0, 20.0, 22.0] {
            let mark = mark_size(size);
            assert_eq!(mark % 2.0, 0.0, "size {size}");
            assert_eq!((size - mark) % 2.0, 0.0, "size {size}");
        }
    }
}
