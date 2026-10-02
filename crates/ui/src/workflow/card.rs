//! The workflow card in the transcript, and the compact result row.
//!
//! Both are plain functions of a pure model ([`CardModel`]) and an
//! [`ActionSink`]: they draw, and describe what a click wants; the
//! transcript carries it out. Nothing here owns state or timers — the only
//! animation is the shared mini spinner, which already follows the global
//! reduce-motion / pause-in-background rules.

use std::time::Duration;

use super::model::{CardModel, Chip, Notice, Pill, PillState, Station};
use super::widgets::{
    icon_button, kind_icon, lamp, light_color, pill_mark, status_glyph, tone_color,
};
use super::{ActionSink, WorkflowAction};
use crate::icons::{self, icon};
use crate::theme::Theme;
use crate::typography::ui_rems;
use gpui::{
    AnyElement, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, div, prelude::*, px,
};

const COLUMN_BASIS: f32 = 120.0;
const COLUMN_MAX: f32 = 220.0;
const PILL_HEIGHT: f32 = 22.0;

/// Mouse-down on a control must not start the card's own click.
fn swallow(el: gpui::Stateful<gpui::Div>) -> gpui::Stateful<gpui::Div> {
    el.on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
}

fn tip(
    label: impl Into<SharedString>,
) -> impl Fn(&mut gpui::Window, &mut gpui::App) -> gpui::AnyView {
    crate::settings::widgets::text_tooltip(label)
}

/// The card. `expanded` shows the agents under each phase; collapsed keeps
/// the rail, the artifact chips and the notices.
pub fn card_element(
    model: &CardModel,
    expanded: bool,
    row_id: &SharedString,
    theme: &Theme,
    sink: &ActionSink,
    view: gpui::EntityId,
    cx: &mut gpui::App,
) -> AnyElement {
    let accent = theme.accent;
    let run_id = model.run_id.clone();
    let toggle_label = if expanded {
        "Collapse workflow"
    } else {
        "Expand workflow"
    };
    let toggle_sink = sink.clone();
    let toggle_id = run_id.clone();

    let header = header_row(model, expanded, row_id, theme, sink, view, cx);
    let rail = rail(model, expanded, row_id, theme, sink, view, cx);

    div()
        .py(px(6.0))
        .w_full()
        .min_w_0()
        .child(
            div()
                .id(SharedString::from(format!("{row_id}-card")))
                .role(gpui::Role::Button)
                .aria_label(SharedString::from(format!(
                    "{} · {toggle_label}",
                    model.summary()
                )))
                .w_full()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .px(px(12.0))
                .py(px(8.0))
                .rounded(px(10.0))
                .border_1()
                .border_color(crate::theme::hairline(0.08))
                .bg(crate::theme::ink(0.03))
                .cursor_pointer()
                .tab_index(0)
                .focus_visible(move |s| s.border_color(accent))
                .on_click(move |_, window, cx| {
                    toggle_sink(WorkflowAction::ToggleCard(toggle_id.clone()), window, cx)
                })
                .child(header)
                .when_some(rail, |el, rail| el.child(rail))
                .when(!model.chips.is_empty(), |el| {
                    el.child(chip_row(model, theme, sink))
                })
                .children(
                    model
                        .notices
                        .iter()
                        .map(|notice| notice_line(notice, theme)),
                )
                .when(expanded, |el| el.child(footer(model, theme))),
        )
        .into_any_element()
}

fn header_row(
    model: &CardModel,
    expanded: bool,
    row_id: &SharedString,
    theme: &Theme,
    sink: &ActionSink,
    view: gpui::EntityId,
    cx: &mut gpui::App,
) -> AnyElement {
    let tone = tone_color(model.tone, theme);
    let run_id = model.run_id.clone();
    let mut row = div()
        .w_full()
        .min_w_0()
        .h(px(24.0))
        .flex()
        .items_center()
        .gap(px(8.0))
        .child(
            div()
                .flex_none()
                .w(px(14.0))
                .flex()
                .justify_center()
                .child(status_glyph(
                    model.status,
                    model.tone,
                    SharedString::from(format!("{row_id}-status")),
                    theme,
                    view,
                    cx,
                )),
        )
        .child(
            div()
                .flex_none()
                .text_size(ui_rems(12.5))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(tone)
                .child(model.kind_word),
        )
        .child(
            div()
                .min_w(px(40.0))
                // Barely gives way: the counts after it shrink first.
                .flex_shrink(0.1)
                .truncate()
                .text_size(ui_rems(12.5))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(theme.text)
                .child(SharedString::from(format!("· {}", model.name))),
        )
        .when(!model.counts.is_empty(), |el| {
            el.child(
                div()
                    .min_w_0()
                    // Gives way first: the name says more than the counts.
                    .flex_shrink(4.0)
                    .truncate()
                    .text_size(ui_rems(12.0))
                    .text_color(theme.text_faint)
                    .child(SharedString::from(format!("· {}", model.counts))),
            )
        })
        .child(div().flex_1());

    if model.questions > 0 {
        let sink = sink.clone();
        let open = run_id.clone();
        let warning = theme.warning;
        let label = if model.questions == 1 {
            "1 question".to_owned()
        } else {
            format!("{} questions", model.questions)
        };
        row = row.child(swallow(
            div()
                .id(SharedString::from(format!("{row_id}-questions")))
                .role(gpui::Role::Button)
                .aria_label(SharedString::from(format!("{label} waiting. Open the run")))
                .flex_none()
                .h(px(20.0))
                .px(px(7.0))
                .rounded_full()
                .flex()
                .items_center()
                .gap(px(4.0))
                .bg(warning.opacity(0.14))
                .text_size(ui_rems(11.0))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(warning)
                .cursor_pointer()
                .tab_index(0)
                .hover(move |s| s.bg(warning.opacity(0.22)))
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    sink(
                        WorkflowAction::OpenRun {
                            run_id: open.clone(),
                            landing: None,
                        },
                        window,
                        cx,
                    );
                })
                .tooltip(tip(
                    "An agent is waiting for an answer. Open the run to reply.",
                ))
                .tooltip_show_delay(Duration::from_millis(350))
                .child(
                    icon(icons::CHAT_ROUND_LINE)
                        .size(px(11.0))
                        .text_color(warning),
                )
                .child(SharedString::from(label)),
        ));
    }
    if model.can_stop {
        let sink = sink.clone();
        let id = run_id.clone();
        row = row.child(icon_button(
            SharedString::from(format!("{row_id}-stop")),
            icons::STOP,
            "Stop workflow (it can be resumed)",
            theme,
            move |_, window, cx| sink(WorkflowAction::Stop { run_id: id.clone() }, window, cx),
        ));
    }
    if model.can_resume {
        let sink = sink.clone();
        let id = run_id.clone();
        let accent = theme.accent;
        row = row.child(swallow(
            div()
                .id(SharedString::from(format!("{row_id}-resume")))
                .role(gpui::Role::Button)
                .aria_label("Resume workflow")
                .flex_none()
                .h(px(24.0))
                .px(px(8.0))
                .rounded(px(6.0))
                .flex()
                .items_center()
                .gap(px(5.0))
                .text_size(ui_rems(12.0))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(theme.text)
                .border_1()
                .border_color(crate::theme::hairline(0.14))
                .cursor_pointer()
                .tab_index(0)
                .hover(|s| s.bg(crate::theme::ink(0.07)))
                .focus_visible(move |s| s.border_color(accent))
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    sink(WorkflowAction::Resume { run_id: id.clone() }, window, cx)
                })
                .tooltip(tip(
                    "Continue from the journal; finished steps are not repeated",
                ))
                .tooltip_show_delay(Duration::from_millis(350))
                .child(
                    icon(icons::RESTART)
                        .size(px(12.0))
                        .text_color(theme.text_muted),
                )
                .child("Resume"),
        ));
    }
    {
        let sink = sink.clone();
        let id = run_id;
        row = row.child(icon_button(
            SharedString::from(format!("{row_id}-open")),
            icons::EXPAND_ARROWS,
            "Open the run",
            theme,
            move |_, window, cx| {
                sink(
                    WorkflowAction::OpenRun {
                        run_id: id.clone(),
                        landing: None,
                    },
                    window,
                    cx,
                )
            },
        ));
    }
    row = row.child(
        icon(if expanded {
            icons::ALT_ARROW_UP
        } else {
            icons::ALT_ARROW_DOWN
        })
        .size(px(13.0))
        .text_color(theme.text_muted.opacity(0.7)),
    );
    row.into_any_element()
}

/// The phase rail: one column per phase, wrapping when the card is narrow.
fn rail(
    model: &CardModel,
    expanded: bool,
    row_id: &SharedString,
    theme: &Theme,
    sink: &ActionSink,
    view: gpui::EntityId,
    cx: &mut gpui::App,
) -> Option<AnyElement> {
    if model.stations.is_empty() {
        return None;
    }
    let columns: Vec<AnyElement> = model
        .stations
        .iter()
        .enumerate()
        .map(|(ix, station)| column(ix, station, model, expanded, row_id, theme, sink, view, cx))
        .collect();
    Some(
        div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_row()
            .flex_wrap()
            .gap_x(px(10.0))
            .gap_y(px(8.0))
            .children(columns)
            .into_any_element(),
    )
}

#[allow(clippy::too_many_arguments)]
fn column(
    ix: usize,
    station: &Station,
    model: &CardModel,
    expanded: bool,
    row_id: &SharedString,
    theme: &Theme,
    sink: &ActionSink,
    view: gpui::EntityId,
    cx: &mut gpui::App,
) -> AnyElement {
    let base = &station.base;
    let color = light_color(base.light, theme);
    let pending = matches!(base.light, super::model::Light::Pending);
    let name_color = if pending {
        theme.text_faint
    } else {
        theme.text
    };
    let fraction = base.fraction();
    let live_fraction = matches!(base.light, super::model::Light::Running);
    let title = if base.parallel_with_prev {
        format!("{} runs alongside the previous phase", base.name)
    } else {
        base.name.clone()
    };
    div()
        .id(SharedString::from(format!("{row_id}-col{ix}")))
        .flex_1()
        .flex_basis(px(COLUMN_BASIS))
        .min_w(px(104.0))
        .max_w(px(COLUMN_MAX))
        .flex()
        .flex_col()
        .gap(px(4.0))
        // The rail itself: a segment per phase, lit by its state. Parallel
        // phases share the accent so they read as one band.
        .child(div().h(px(2.0)).w_full().rounded_full().bg(if pending {
            theme.border
        } else {
            color.opacity(if live_fraction { 0.95 } else { 0.55 })
        }))
        .child(
            div()
                .w_full()
                .h(px(18.0))
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(lamp(base.light, 7.0, theme))
                .when(base.parallel_with_prev, |el| {
                    el.child(
                        div()
                            .flex_none()
                            .text_size(ui_rems(11.0))
                            .text_color(theme.text_faint)
                            .child("∥"),
                    )
                })
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(ui_rems(12.0))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(name_color)
                        .child(SharedString::from(base.name.clone())),
                )
                .when_some(fraction, |el, text| {
                    el.child(
                        div()
                            .flex_none()
                            .text_size(ui_rems(11.0))
                            .text_color(if live_fraction {
                                theme.text_muted
                            } else {
                                theme.text_faint
                            })
                            .child(SharedString::from(text)),
                    )
                }),
        )
        .tooltip(tip(title))
        .tooltip_show_delay(Duration::from_millis(500))
        .when(expanded, |el| {
            el.children(
                station
                    .pills
                    .iter()
                    .enumerate()
                    .map(|(pix, pill)| pill_row(ix, pix, pill, row_id, theme, sink, view, cx)),
            )
            .when(station.hidden > 0, |el| {
                let sink = sink.clone();
                let run_id = model.run_id.clone();
                let landing = base.name.clone();
                let label = if station.hidden_active > 0 {
                    format!(
                        "+{} more · {} working",
                        station.hidden, station.hidden_active
                    )
                } else {
                    format!("+{} more", station.hidden)
                };
                el.child(swallow(
                    div()
                        .id(SharedString::from(format!("{row_id}-col{ix}-more")))
                        .role(gpui::Role::Button)
                        .aria_label(SharedString::from(format!(
                            "{label}. Open the run to see all agents in {}",
                            base.name
                        )))
                        .h(px(PILL_HEIGHT))
                        .px(px(6.0))
                        .rounded(px(5.0))
                        .flex()
                        .items_center()
                        .text_size(ui_rems(11.0))
                        .text_color(theme.text_faint)
                        .cursor_pointer()
                        .tab_index(0)
                        .hover(|s| s.bg(crate::theme::ink(0.05)))
                        .on_click(move |_, window, cx| {
                            cx.stop_propagation();
                            sink(
                                WorkflowAction::OpenRun {
                                    run_id: run_id.clone(),
                                    landing: Some(landing.clone()),
                                },
                                window,
                                cx,
                            );
                        })
                        .child(SharedString::from(label)),
                ))
            })
        })
        .into_any_element()
}

#[allow(clippy::too_many_arguments)]
fn pill_row(
    col: usize,
    ix: usize,
    pill: &Pill,
    row_id: &SharedString,
    theme: &Theme,
    sink: &ActionSink,
    view: gpui::EntityId,
    cx: &mut gpui::App,
) -> AnyElement {
    let name_color = match pill.state {
        PillState::Pending => theme.text_faint,
        PillState::Done | PillState::Cancelled => theme.text_muted,
        _ => theme.text,
    };
    let mut tooltip = format!("{} · {}", pill.name, pill.state.word());
    if let Some(model) = &pill.model_label {
        tooltip.push_str(&format!(" · {model}"));
    }
    if pill.nodes > 1 {
        tooltip.push_str(&format!(" · {} steps", pill.nodes));
    }
    let mark = pill_mark(
        pill.state,
        SharedString::from(format!("{row_id}-p{col}-{ix}")),
        theme,
        view,
        cx,
    );
    let base = div()
        .id(SharedString::from(format!("{row_id}-p{col}-{ix}-pill")))
        .h(px(PILL_HEIGHT))
        .w_full()
        .min_w_0()
        .px(px(6.0))
        .rounded(px(5.0))
        .flex()
        .items_center()
        .gap(px(7.0))
        .child(mark)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(ui_rems(12.0))
                .text_color(name_color)
                .child(SharedString::from(pill.name.clone())),
        );
    match &pill.child_chat_id {
        Some(child) => {
            let sink = sink.clone();
            let child = child.clone();
            let title = pill.name.clone();
            let accent = theme.accent;
            swallow(base)
                .role(gpui::Role::Button)
                .aria_label(SharedString::from(format!("Open {}'s chat", pill.name)))
                .cursor_pointer()
                .tab_index(0)
                .hover(|s| s.bg(crate::theme::ink(0.06)))
                .focus_visible(move |s| s.bg(accent.opacity(0.16)))
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    sink(
                        WorkflowAction::OpenActor {
                            child_chat_id: child.clone(),
                            title: title.clone(),
                        },
                        window,
                        cx,
                    );
                })
                .tooltip(tip(format!("{tooltip} — open its chat")))
                .tooltip_show_delay(Duration::from_millis(350))
                .into_any_element()
        }
        None => base
            .tooltip(tip(tooltip))
            .tooltip_show_delay(Duration::from_millis(350))
            .into_any_element(),
    }
}

fn chip_row(model: &CardModel, theme: &Theme, sink: &ActionSink) -> AnyElement {
    let run_id = model.run_id.clone();
    div()
        .w_full()
        .min_w_0()
        .flex()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap(px(6.0))
        .children(
            model
                .chips
                .iter()
                .map(|chip| artifact_chip(&run_id, chip, theme, sink)),
        )
        .when(model.chips_more > 0, |el| {
            let sink = sink.clone();
            let id = run_id.clone();
            el.child(swallow(
                div()
                    .id(SharedString::from(format!("{}-chips-more", model.run_id)))
                    .role(gpui::Role::Button)
                    .aria_label(SharedString::from(format!(
                        "{} more artifacts. Open the run",
                        model.chips_more
                    )))
                    .h(px(24.0))
                    .px(px(8.0))
                    .rounded(px(7.0))
                    .flex()
                    .items_center()
                    .text_size(ui_rems(11.5))
                    .text_color(theme.text_muted)
                    .cursor_pointer()
                    .tab_index(0)
                    .hover(|s| s.bg(crate::theme::ink(0.06)))
                    .on_click(move |_, window, cx| {
                        cx.stop_propagation();
                        sink(
                            WorkflowAction::OpenRun {
                                run_id: id.clone(),
                                landing: None,
                            },
                            window,
                            cx,
                        );
                    })
                    .child(SharedString::from(format!("+{}", model.chips_more))),
            ))
        })
        .into_any_element()
}

pub(crate) fn artifact_chip(
    run_id: &str,
    chip: &Chip,
    theme: &Theme,
    sink: &ActionSink,
) -> AnyElement {
    let sink = sink.clone();
    let run = run_id.to_owned();
    let artifact = chip.id.clone();
    let accent = theme.accent;
    let tooltip = if chip.version > 1 {
        format!("{} (version {})", chip.title, chip.version)
    } else {
        chip.title.clone()
    };
    swallow(
        div()
            .id(SharedString::from(format!("{run_id}-chip-{}", chip.id)))
            .role(gpui::Role::Button)
            .aria_label(SharedString::from(format!("Open artifact {}", chip.title)))
            .h(px(24.0))
            .max_w(px(220.0))
            .px(px(8.0))
            .rounded(px(7.0))
            .flex()
            .items_center()
            .gap(px(6.0))
            .border_1()
            .border_color(crate::theme::hairline(0.1))
            .cursor_pointer()
            .tab_index(0)
            .hover(|s| s.bg(crate::theme::ink(0.06)))
            .focus_visible(move |s| s.border_color(accent))
            .on_click(move |_, window, cx| {
                cx.stop_propagation();
                sink(
                    WorkflowAction::OpenArtifact {
                        run_id: run.clone(),
                        artifact_id: artifact.clone(),
                    },
                    window,
                    cx,
                );
            })
            .tooltip(tip(tooltip))
            .tooltip_show_delay(Duration::from_millis(350))
            .child(
                icon(kind_icon(chip.kind))
                    .size(px(12.0))
                    .text_color(theme.text_muted),
            )
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(ui_rems(11.5))
                    .text_color(theme.text)
                    .child(SharedString::from(chip.label.clone())),
            ),
    )
    .into_any_element()
}

fn notice_line(notice: &Notice, theme: &Theme) -> AnyElement {
    div()
        .w_full()
        .min_w_0()
        .text_size(ui_rems(11.5))
        .line_height(px(16.0))
        .text_color(match notice.tone {
            super::model::Tone::Muted => theme.text_faint,
            tone => tone_color(tone, theme),
        })
        .child(SharedString::from(notice.text.clone()))
        .into_any_element()
}

/// The expanded card's last line: totals and, once it ended, the result.
fn footer(model: &CardModel, theme: &Theme) -> AnyElement {
    let ended = model.status.is_settled();
    div()
        .w_full()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(2.0))
        .when(!model.meta.is_empty(), |el| {
            el.child(
                div()
                    .text_size(ui_rems(11.5))
                    .text_color(theme.text_faint)
                    .child(SharedString::from(model.meta.clone())),
            )
        })
        .when(ended, |el| {
            el.when_some(model.result_preview.clone(), |el, preview| {
                el.child(
                    div()
                        .w_full()
                        .truncate()
                        .text_size(ui_rems(11.5))
                        .text_color(theme.text_muted)
                        .child(SharedString::from(format!("Result: {preview}"))),
                )
            })
        })
        .into_any_element()
}

/// The compact row that replaces the machine message delivering a run's
/// result to the agent: status and summary on one line, a result preview and
/// the artifact chips when opened, and a way into the run.
pub fn result_element(
    model: &super::model::ResultModel,
    expanded: bool,
    row_id: &SharedString,
    theme: &Theme,
    sink: &ActionSink,
    view: gpui::EntityId,
    cx: &mut gpui::App,
) -> AnyElement {
    let accent = theme.accent;
    let tone = tone_color(model.tone, theme);
    let toggle_sink = sink.clone();
    let toggle_id = model.run_id.clone();
    let label = if expanded {
        "Collapse the workflow result"
    } else {
        "Expand the workflow result"
    };
    let header = div()
        .w_full()
        .min_w_0()
        .h(px(24.0))
        .flex()
        .items_center()
        .gap(px(8.0))
        .child(
            div()
                .flex_none()
                .w(px(14.0))
                .flex()
                .justify_center()
                .child(status_glyph(
                    model.status,
                    model.tone,
                    SharedString::from(format!("{row_id}-status")),
                    theme,
                    view,
                    cx,
                )),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(ui_rems(12.0))
                .text_color(tone)
                .font_weight(gpui::FontWeight::MEDIUM)
                .child(SharedString::from(format!(
                    "{} sent to the agent",
                    model.headline
                ))),
        )
        .when(model.can_open, |el| {
            let sink = sink.clone();
            let id = model.run_id.clone();
            el.child(icon_button(
                SharedString::from(format!("{row_id}-open")),
                icons::EXPAND_ARROWS,
                "Open the run",
                theme,
                move |_, window, cx| {
                    sink(
                        WorkflowAction::OpenRun {
                            run_id: id.clone(),
                            landing: None,
                        },
                        window,
                        cx,
                    )
                },
            ))
        })
        .child(
            icon(if expanded {
                icons::ALT_ARROW_UP
            } else {
                icons::ALT_ARROW_DOWN
            })
            .size(px(13.0))
            .text_color(theme.text_muted.opacity(0.7)),
        );
    div()
        .py(px(4.0))
        .w_full()
        .min_w_0()
        .child(
            div()
                .id(SharedString::from(format!("{row_id}-result")))
                .role(gpui::Role::Button)
                .aria_label(SharedString::from(format!("{} · {label}", model.headline)))
                .w_full()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(6.0))
                .px(px(12.0))
                .py(px(5.0))
                .rounded(px(10.0))
                .border_1()
                .border_color(crate::theme::hairline(0.07))
                .cursor_pointer()
                .tab_index(0)
                .hover(|s| s.bg(crate::theme::ink(0.025)))
                .focus_visible(move |s| s.border_color(accent))
                .on_click(move |_, window, cx| {
                    toggle_sink(WorkflowAction::ToggleResult(toggle_id.clone()), window, cx)
                })
                .child(header)
                .when(!model.summary.is_empty(), |el| {
                    el.child(
                        div()
                            .w_full()
                            .min_w_0()
                            .when(!expanded, |el| el.truncate())
                            .text_size(ui_rems(11.5))
                            .line_height(px(16.0))
                            .text_color(theme.text_muted)
                            .child(SharedString::from(model.summary.clone())),
                    )
                })
                .when(expanded, |el| {
                    el.when_some(model.reason.clone(), |el, reason| {
                        el.child(
                            div()
                                .text_size(ui_rems(11.5))
                                .line_height(px(16.0))
                                .text_color(tone)
                                .child(SharedString::from(reason)),
                        )
                    })
                    .when_some(model.result.clone(), |el, result| {
                        el.child(
                            div()
                                .w_full()
                                .px(px(10.0))
                                .py(px(8.0))
                                .rounded(px(7.0))
                                .bg(crate::theme::ink(0.04))
                                .font_family(theme.font_mono.clone())
                                .text_size(px(theme.code_font_size - 1.0))
                                .line_height(px(17.0))
                                .text_color(theme.text)
                                .child(SharedString::from(result)),
                        )
                        .when(model.result_cut, |el| {
                            el.child(
                                div()
                                    .text_size(ui_rems(11.0))
                                    .text_color(theme.text_faint)
                                    .child("The result is longer than this preview."),
                            )
                        })
                    })
                })
                .when(!model.chips.is_empty() && expanded, |el| {
                    el.child(
                        div()
                            .w_full()
                            .flex()
                            .flex_row()
                            .flex_wrap()
                            .items_center()
                            .gap(px(6.0))
                            .children(model.chips.iter().map(|chip| {
                                if model.can_open {
                                    artifact_chip(&model.run_id, chip, theme, sink)
                                } else {
                                    static_chip(chip, theme)
                                }
                            }))
                            .when(model.chips_more > 0, |el| {
                                el.child(
                                    div()
                                        .text_size(ui_rems(11.5))
                                        .text_color(theme.text_muted)
                                        .child(SharedString::from(format!(
                                            "+{}",
                                            model.chips_more
                                        ))),
                                )
                            }),
                    )
                }),
        )
        .into_any_element()
}

/// A chip for an artifact whose run is no longer tracked: it names it, it
/// cannot open it.
fn static_chip(chip: &Chip, theme: &Theme) -> AnyElement {
    div()
        .h(px(24.0))
        .max_w(px(220.0))
        .px(px(8.0))
        .rounded(px(7.0))
        .flex()
        .items_center()
        .gap(px(6.0))
        .border_1()
        .border_color(crate::theme::hairline(0.07))
        .child(
            icon(kind_icon(chip.kind))
                .size(px(12.0))
                .text_color(theme.text_faint),
        )
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_size(ui_rems(11.5))
                .text_color(theme.text_muted)
                .child(SharedString::from(chip.label.clone())),
        )
        .into_any_element()
}
