//! Small gpui pieces shared by the workflow surfaces: lamps, status marks,
//! icon buttons, chips. Colors come from the theme only.

use std::time::Duration;

use gpui::{
    AnyElement, Hsla, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, div, px,
};
use zeron_proto::{ArtifactKind, WorkflowStatus};

use super::model::{Light, PillState, Tone};
use crate::icons::{self, icon};
use crate::theme::Theme;

pub(crate) fn tone_color(tone: Tone, theme: &Theme) -> Hsla {
    match tone {
        Tone::Accent => theme.accent,
        Tone::Success => theme.success,
        Tone::Warning => theme.warning,
        Tone::Danger => theme.danger,
        Tone::Muted => theme.text_muted,
    }
}

pub(crate) fn light_color(light: Light, theme: &Theme) -> Hsla {
    match light {
        Light::Pending => theme.text_faint,
        Light::Running => theme.accent,
        Light::Done => theme.success,
        Light::Failed => theme.danger,
        Light::Stopped => theme.text_muted,
    }
}

/// The phase lamp: hollow while pending, solid otherwise.
pub(crate) fn lamp(light: Light, size: f32, theme: &Theme) -> gpui::Div {
    let color = light_color(light, theme);
    let base = div().flex_none().size(px(size)).rounded_full();
    match light {
        Light::Pending => base.border_1().border_color(color.opacity(0.7)),
        _ => base.bg(color),
    }
}

pub(crate) fn kind_icon(kind: ArtifactKind) -> &'static str {
    match kind {
        ArtifactKind::Markdown => icons::FILE_MARKDOWN,
        ArtifactKind::Table => icons::TABLE,
        ArtifactKind::Metrics => icons::CHART_BARS,
        ArtifactKind::File => icons::DOCUMENT,
    }
}

pub(crate) fn kind_word(kind: ArtifactKind) -> &'static str {
    match kind {
        ArtifactKind::Markdown => "Document",
        ArtifactKind::Table => "Table",
        ArtifactKind::Metrics => "Metrics",
        ArtifactKind::File => "File",
    }
}

/// The run's status glyph: a spinner while it works (and motion allows),
/// otherwise a still glyph.
pub(crate) fn status_glyph(
    status: WorkflowStatus,
    tone: Tone,
    key: SharedString,
    theme: &Theme,
    view: gpui::EntityId,
    cx: &mut gpui::App,
) -> AnyElement {
    let color = tone_color(tone, theme);
    match status {
        WorkflowStatus::Running => {
            crate::loaders::mini_glyph_spinner(key, 2.5, theme.glyph, view, cx).into_any_element()
        }
        WorkflowStatus::Pending => icon(icons::CLOCK_CIRCLE)
            .size(px(14.0))
            .text_color(color)
            .into_any_element(),
        WorkflowStatus::Completed => icon(icons::CHECK)
            .size(px(14.0))
            .text_color(color)
            .into_any_element(),
        WorkflowStatus::Errored => icon(icons::DANGER_TRIANGLE)
            .size(px(14.0))
            .text_color(color)
            .into_any_element(),
        WorkflowStatus::Stopped => icon(icons::PAUSE)
            .size(px(14.0))
            .text_color(color)
            .into_any_element(),
    }
}

/// An agent pill's state mark, in a fixed 12px slot so names align.
pub(crate) fn pill_mark(
    state: PillState,
    key: SharedString,
    theme: &Theme,
    view: gpui::EntityId,
    cx: &mut gpui::App,
) -> AnyElement {
    let slot = div()
        .flex_none()
        .size(px(12.0))
        .flex()
        .items_center()
        .justify_center();
    match state {
        PillState::Running => slot
            .child(crate::loaders::mini_glyph_spinner(
                key,
                2.0,
                theme.glyph,
                view,
                cx,
            ))
            .into_any_element(),
        PillState::Asking => slot
            .child(
                icon(icons::CHAT_ROUND_LINE)
                    .size(px(12.0))
                    .text_color(theme.warning),
            )
            .into_any_element(),
        PillState::Done => slot
            .child(
                icon(icons::CHECK)
                    .size(px(12.0))
                    .text_color(theme.text_muted),
            )
            .into_any_element(),
        PillState::Failed => slot
            .child(
                icon(icons::CLOSE_CIRCLE)
                    .size(px(12.0))
                    .text_color(theme.danger),
            )
            .into_any_element(),
        PillState::Cancelled => slot
            .child(
                icon(icons::PAUSE)
                    .size(px(11.0))
                    .text_color(theme.text_faint),
            )
            .into_any_element(),
        PillState::Pending => slot
            .child(
                div()
                    .size(px(8.0))
                    .rounded_full()
                    .border_1()
                    .border_color(theme.text_faint.opacity(0.7)),
            )
            .into_any_element(),
    }
}

/// A quiet icon-only button with a tooltip (every icon-only control in the
/// app carries one).
pub(crate) fn icon_button(
    id: impl Into<gpui::ElementId>,
    glyph: &'static str,
    tooltip: &'static str,
    theme: &Theme,
    on_click: impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    let accent = theme.accent;
    div()
        .id(id)
        .role(gpui::Role::Button)
        .aria_label(tooltip)
        .size(px(24.0))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(5.0))
        .cursor_pointer()
        .hover(|s| s.bg(crate::theme::ink(0.07)))
        .tab_index(0)
        .focus_visible(move |s| s.bg(accent.opacity(0.18)))
        // The card around it toggles on a click; a button must not.
        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(move |event, window, cx| {
            cx.stop_propagation();
            on_click(event, window, cx);
        })
        .tooltip(crate::settings::widgets::text_tooltip(tooltip))
        .tooltip_show_delay(Duration::from_millis(350))
        .child(
            icon(glyph)
                .size(px(13.0))
                .text_color(theme.text_muted.opacity(0.9)),
        )
}
