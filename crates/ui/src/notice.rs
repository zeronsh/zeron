//! Shared notice chip — the tinted failure card the composer strip and the
//! transcript converged on (zeron composer.tsx `Notice` / chat-view.tsx
//! `ErrorChip`).

use gpui::{AnyElement, Div, FontWeight, SharedString, div, prelude::*, px};

use crate::theme::Theme;

/// The chip's header icon treatment, which also picks its metrics: the bare
/// triangle reads lighter and rem-scales with the composer's text; the tile
/// anchors the transcript's block chip at fixed metrics.
pub enum NoticeChipIcon {
    /// Bare 14px triangle in the label color; 12px inset/radius, rem-scaled
    /// 12px text on a 16px line height (the composer's inline notice).
    Plain,
    /// 20px accent-washed tile holding a 12px triangle; 10px inset/radius,
    /// fixed 12px text (the transcript's ErrorChip).
    Tile,
}

/// The failure notice both surfaces render (zeron composer.tsx `Notice`,
/// chat-view.tsx `ErrorChip`): a tinted rounded chip — `border
/// <accent>/[0.16]` over a `<accent>/[0.05]` wash (the transcript's tile:
/// a borderless 0.1 wash), never a bare stroke — with a header row (DangerTriangle + medium label, a
/// tiny copy button pinned to its top-right corner — failure payloads are
/// meant to be pasted, not screenshotted) and the message below. The message WRAPS instead of truncating: failure payloads carry
/// exit statuses and stderr, and a one-line ellipsis was exactly what made
/// zeronsh/comet#95 undiagnosable from the screenshot. `warning` picks the
/// amber palette (amber-400/amber-200) over the default red
/// (red-400/red-300). Callers chain their own chrome: the composer its
/// id/dismiss click, the transcript `w_full().overflow_hidden()`.
pub fn notice_chip(
    theme: &Theme,
    warning: bool,
    label: &'static str,
    message: impl Into<SharedString>,
    icon: NoticeChipIcon,
) -> Div {
    notice_chip_with_action(theme, warning, label, message, icon, None)
}

/// [`notice_chip`] with an action under the message (the transcript's
/// "Log in").
pub fn notice_chip_with_action(
    theme: &Theme,
    warning: bool,
    label: &'static str,
    message: impl Into<SharedString>,
    icon: NoticeChipIcon,
    action: Option<AnyElement>,
) -> Div {
    let (accent, muted) = if warning {
        (theme.warning, theme.warning_muted)
    } else {
        (theme.danger, theme.danger_muted)
    };
    let message = message.into();
    let copy_message = message.clone();
    let tile = matches!(icon, NoticeChipIcon::Tile);
    // The transcript's block chip reads as an alert: a stronger red wash,
    // the message in the lighter red.
    let wash = if tile { 0.1 } else { 0.05 };
    let frame = |inset: f32| {
        div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .rounded(px(inset))
            // The transcript's alert is borderless: the wash alone.
            .when(!tile, |chip| {
                chip.border_1().border_color(accent.opacity(0.16))
            })
            .bg(accent.opacity(wash))
            .px(px(inset))
            .py(px(8.0))
    };
    let chip = if tile {
        frame(10.0).text_size(px(12.0))
    } else {
        frame(12.0)
            .text_size(crate::typography::ui_rems(12.0))
            .line_height(px(16.0))
            .text_color(muted.opacity(0.9))
    };
    let header_icon: AnyElement = if tile {
        div()
            .flex_none()
            .size(px(20.0))
            .rounded(px(6.0))
            .bg(accent.opacity(0.12))
            .flex()
            .items_center()
            .justify_center()
            .child(
                crate::icons::icon(crate::icons::DANGER_TRIANGLE)
                    .size(px(12.0))
                    .text_color(muted.opacity(0.8)),
            )
            .into_any_element()
    } else {
        crate::icons::icon(crate::icons::DANGER_TRIANGLE)
            .size(px(14.0))
            .text_color(muted.opacity(0.9))
            .into_any_element()
    };
    // The tile pins its own label/message colors; the plain icon inherits the
    // chip-wide muted accent.
    let copy = div()
        .id("notice-copy")
        .flex_none()
        .size(px(20.0))
        .rounded(px(6.0))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover(move |s| s.bg(accent.opacity(0.12)))
        // The composer chip dismisses on click; copying must
        // not take the notice with it.
        .on_click(move |_, _, cx| {
            cx.stop_propagation();
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(copy_message.to_string()));
        })
        .tooltip(crate::settings::widgets::text_tooltip("Copy message"))
        .child(
            crate::icons::icon(crate::icons::COPY)
                .size(px(12.0))
                .text_color(muted.opacity(0.8)),
        );
    let header = div()
        .flex()
        .items_center()
        .gap(px(8.0))
        .child(header_icon)
        .child(
            div()
                .font_weight(FontWeight::MEDIUM)
                .when(tile, |label| label.text_color(muted.opacity(0.8)))
                .child(label),
        )
        .child(div().flex_1());
    let message = div()
        .min_w_0()
        .w_full()
        .when(tile, |message| message.text_color(muted))
        .child(message);
    // Copy pinned to the header's top-right corner; an action goes under
    // the message.
    chip.child(header.child(copy))
        .child(message)
        .when_some(action, |chip, action| {
            chip.child(div().pt(px(2.0)).flex().child(action))
        })
}
