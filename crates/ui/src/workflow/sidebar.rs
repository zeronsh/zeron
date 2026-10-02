//! Run lines under a chat's sidebar row: a live run's name with a compact
//! phase rail and `2/4`, and the same for runs that ended since the user last
//! opened the chat. At most two lines; the rest are counted.
//!
//! The lines are pure paint over [`RunLine`]s — no timers, no animation (a
//! sidebar of long-running chats must stay still).

use std::rc::Rc;
use std::time::Duration;

use gpui::{
    AnyElement, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, prelude::*, px,
};

use super::model::{Light, RunLine, Tone};
use super::widgets::{light_color, tone_color};
use crate::theme::Theme;
use crate::typography::ui_rems;
use zeron_proto::WorkflowStatus;

/// What a clicked run line asks for: open that run (its id).
pub type OpenRun = Rc<dyn Fn(String, &mut Window, &mut gpui::App)>;

pub const LINE_HEIGHT: f32 = 17.0;
const TOP_PAD: f32 = 2.0;

/// Height the strip adds under a row for `lines` lines.
pub fn strip_height(lines: usize) -> f32 {
    if lines == 0 {
        0.0
    } else {
        TOP_PAD + LINE_HEIGHT * lines as f32
    }
}

fn dots(line: &RunLine, theme: &Theme) -> gpui::Div {
    let mut row = div().flex_none().flex().items_center().gap(px(3.0));
    for light in &line.rail.lights {
        let color = light_color(*light, theme);
        row = row.child(match light {
            Light::Pending => div()
                .size(px(5.0))
                .rounded_full()
                .border_1()
                .border_color(color.opacity(0.6)),
            _ => div().size(px(5.0)).rounded_full().bg(color),
        });
    }
    if line.rail.hidden > 0 {
        row = row.child(
            div()
                .text_size(ui_rems(9.5))
                .text_color(theme.text_faint)
                .child(SharedString::from(format!("+{}", line.rail.hidden))),
        );
    }
    row
}

/// The strip. `on_open` receives the run id of a clicked line.
pub fn run_lines(lines: &[RunLine], overflow: u32, theme: &Theme, on_open: OpenRun) -> AnyElement {
    let n = lines.len();
    div()
        .w_full()
        .pt(px(TOP_PAD))
        .pl(px(Theme::SPACE_SM + 14.0))
        .pr(px(Theme::SPACE_SM))
        .flex()
        .flex_col()
        .children(lines.iter().enumerate().map(|(ix, line)| {
            let accent = theme.accent;
            let run_id = line.run_id.clone();
            let open = on_open.clone();
            let live = line.live;
            let name_color = match (live, line.tone) {
                (true, _) => theme.text_muted,
                (false, Tone::Success) => theme.text_muted,
                (false, tone) => tone_color(tone, theme),
            };
            let tail = if ix + 1 == n && overflow > 0 {
                Some(format!("+{overflow} more"))
            } else {
                None
            };
            let ended_mark = match line.status {
                WorkflowStatus::Errored => Some(("failed", theme.danger)),
                WorkflowStatus::Stopped => Some(("stopped", theme.text_faint)),
                _ => None,
            };
            div()
                .id(SharedString::from(format!("sidebar-run-{}", line.run_id)))
                .role(gpui::Role::Button)
                .aria_label(SharedString::from(line.tooltip.clone()))
                .h(px(LINE_HEIGHT))
                .w_full()
                .min_w_0()
                .flex()
                .items_center()
                .gap(px(6.0))
                .rounded(px(4.0))
                .cursor_pointer()
                .tab_index(0)
                .hover(|s| s.bg(crate::theme::ink(0.05)))
                .focus_visible(move |s| s.bg(accent.opacity(0.16)))
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    open(run_id.clone(), window, cx)
                })
                .tooltip(crate::settings::widgets::text_tooltip(line.tooltip.clone()))
                .tooltip_show_delay(Duration::from_millis(500))
                .child(dots(line, theme))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(ui_rems(10.5))
                        .text_color(name_color)
                        .child(SharedString::from(line.name.clone())),
                )
                .when(line.questions > 0, |el| {
                    el.child(
                        div()
                            .flex_none()
                            .text_size(ui_rems(10.0))
                            .text_color(theme.warning)
                            .child(SharedString::from(format!("{}?", line.questions))),
                    )
                })
                .when_some(ended_mark, |el, (word, color)| {
                    el.child(
                        div()
                            .flex_none()
                            .text_size(ui_rems(10.0))
                            .text_color(color)
                            .child(word),
                    )
                })
                .when(!line.fraction.is_empty(), |el| {
                    el.child(
                        div()
                            .flex_none()
                            .text_size(ui_rems(10.0))
                            .text_color(theme.text_faint)
                            .child(SharedString::from(line.fraction.clone())),
                    )
                })
                .when_some(tail, |el, tail| {
                    el.child(
                        div()
                            .flex_none()
                            .text_size(ui_rems(10.0))
                            .text_color(theme.text_faint)
                            .child(SharedString::from(tail)),
                    )
                })
        }))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_strip_grows_by_a_line_each() {
        assert_eq!(strip_height(0), 0.0);
        assert_eq!(strip_height(1), 19.0);
        assert_eq!(strip_height(2), 36.0);
    }
}
