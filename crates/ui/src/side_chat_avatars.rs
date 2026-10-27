//! The side chats a chat started, drawn as a stack of overlapping provider
//! marks: up to three, then a "+N" disc for the rest. The titlebar shows the
//! ones still at work this way, and the transcript's "Started side chats"
//! line folds to it until opened.
//!
//! A mark wears a small status dot only when its chat has something to say:
//! asking a question (the accent, as everywhere a chat waits on you), failed,
//! or finished. Working is the stack's resting state and carries no dot.

use gpui::{Hsla, ParentElement, SharedString, Styled, div, prelude::*, px};
use zeron_proto::{ChatIndicator, HarnessId};

use crate::icons::icon;
use crate::theme::Theme;

/// Marks a stack draws before the rest fold into "+N".
pub const SHOWN: usize = 3;

/// One side chat in a stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Avatar {
    pub harness: Option<HarnessId>,
    pub status: ChatIndicator,
}

/// The status dot a mark wears, if any.
fn dot(status: ChatIndicator, theme: &Theme) -> Option<Hsla> {
    match status {
        ChatIndicator::AwaitingInput => Some(theme.accent),
        ChatIndicator::Errored => Some(theme.danger),
        ChatIndicator::Completed => Some(theme.success),
        ChatIndicator::Working | ChatIndicator::Idle => None,
    }
}

/// `avatars` as overlapping discs `size` across, on a surface of color
/// `ground` (each disc's rim is cut from it, so overlaps read as layers).
/// The discs sit at explicit offsets in a box sized to hold them, so the
/// stack reserves exactly the width it draws.
pub fn stack(avatars: &[Avatar], size: f32, ground: Hsla, theme: &Theme) -> gpui::Div {
    let step = size - (size * 0.32).round();
    let shown = if avatars.len() > SHOWN {
        SHOWN - 1
    } else {
        avatars.len()
    };
    let rest = avatars.len() - shown;
    let count = shown + usize::from(rest > 0);
    let width = size + step * count.saturating_sub(1) as f32;
    let mut row = div().relative().flex_none().w(px(width)).h(px(size));
    for (ix, avatar) in avatars[..shown].iter().enumerate() {
        row = row.child(
            disc(size, ground, theme)
                .absolute()
                .top_0()
                .left(px(step * ix as f32))
                .child(mark(avatar.harness, size, theme))
                .children(dot(avatar.status, theme).map(|color| status_dot(size, color, ground))),
        );
    }
    if rest > 0 {
        row = row.child(
            disc(size, ground, theme)
                .absolute()
                .top_0()
                .left(px(step * shown as f32))
                .text_size(px((size * 0.5).round()))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(theme.text_muted)
                .child(SharedString::from(format!("+{rest}"))),
        );
    }
    row
}

fn disc(size: f32, ground: Hsla, theme: &Theme) -> gpui::Div {
    div()
        .flex_none()
        .size(px(size))
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(theme.surface_raised)
        .border(px(1.5))
        .border_color(ground)
        .shadow(vec![gpui::BoxShadow {
            color: crate::theme::ink(0.10),
            offset: gpui::point(px(0.0), px(0.0)),
            blur_radius: px(0.0),
            spread_radius: px(0.5),
            inset: false,
        }])
}

fn mark(harness: Option<HarnessId>, size: f32, theme: &Theme) -> gpui::Svg {
    let (path, tint) = match harness {
        Some(harness) => crate::pickers::harness_brand_icon(harness),
        None => (crate::icons::CHAT_ROUND_LINE, None),
    };
    icon(path)
        .size(px((size * 0.52).round()))
        .text_color(tint.unwrap_or(theme.text))
}

fn status_dot(size: f32, color: Hsla, ground: Hsla) -> gpui::Div {
    let dot = (size * 0.4).round().max(6.0);
    div()
        .absolute()
        .right(px(-2.0))
        .bottom(px(-2.0))
        .size(px(dot))
        .rounded_full()
        .bg(color)
        .border(px(1.5))
        .border_color(ground)
}

/// "2 working · 1 needs input": what a stack of `statuses` is doing, for a
/// tooltip or a line beside it.
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
}
