//! Processes the chat's agent left running in the background, docked above
//! the composer.
//!
//! A dev server, a test watcher, a `tail -f`: an agent starts them and moves
//! on, and they keep running after its turn ends. The host's runtime reports
//! the live set on the chat's session row (`Session::background_tasks`, kept
//! fresh by the engine's heartbeat), so this tray shows what is running on any
//! device, whether or not the chat is open there. A row says what runs and
//! for how long; opening it shows the tail of its output, and its stop button
//! asks the host to end that one process (`StopBackgroundTask`). A process
//! leaves the tray when the agent reports it gone, never on the click alone.
//!
//! It sits at the top of the composer's tray stack (above the checklist and
//! the queue), one step narrower than the tray below it. Collapsed by default:
//! it is ambient state, not something to read on every turn.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use gpui::{
    AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled, Window, div, prelude::*, px,
};

use zeron_proto::{BackgroundTask, BackgroundTaskOutput};
use zeron_rpc::methods;

use crate::composer::{Composer, QUEUE_COMPOSER_OVERLAP};
use crate::icons::{self, icon};
use crate::motion::{self, AnimationExt as _};
use crate::theme::Theme;

/// What the collapsed header lists: each process's command, oldest first.
pub(crate) fn headline(tasks: &[BackgroundTask]) -> String {
    tasks
        .iter()
        .map(|task| first_line(&task.command))
        .collect::<Vec<_>>()
        .join("  ·  ")
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or_default().trim()
}

/// How long a process has run, at the precision its age deserves.
pub(crate) fn uptime(task: &BackgroundTask, now: chrono::DateTime<chrono::Utc>) -> String {
    crate::transcript::format_elapsed(now.signed_duration_since(task.started_at).num_seconds())
}

/// The output's last lines, at most `max`, and whether earlier ones exist.
pub(crate) fn output_lines(output: &BackgroundTaskOutput, max: usize) -> (Vec<&str>, bool) {
    let lines: Vec<&str> = output.text.lines().collect();
    let skip = lines.len().saturating_sub(max);
    (lines[skip..].to_vec(), output.truncated || skip > 0)
}

/// Per-chat presentation state, in memory for the app run (like the todo
/// tray's).
#[derive(Debug, Default)]
pub(crate) struct BackgroundTrayState {
    expanded: bool,
    /// The process whose output is open.
    open: Option<String>,
    /// Stops asked of the host, until the process leaves the list.
    stopping: HashSet<String>,
    /// The last output read per process.
    outputs: HashMap<String, BackgroundTaskOutput>,
    /// A stop that failed, shown under the list until the next action.
    failure: Option<SharedString>,
    /// Bumped on every toggle so the list's fade-in replays.
    epoch: u32,
}

impl BackgroundTrayState {
    /// Forget what belonged to processes that have ended.
    pub fn observe(&mut self, tasks: &[BackgroundTask]) {
        let listed = |id: &String| tasks.iter().any(|t| t.id == *id);
        self.stopping.retain(listed);
        self.outputs.retain(|id, _| listed(id));
        if self.open.as_ref().is_some_and(|id| !listed(id)) {
            self.open = None;
        }
    }

    pub fn is_expanded(&self) -> bool {
        self.expanded
    }

    pub fn toggle(&mut self) {
        self.expanded = !self.expanded;
        self.failure = None;
        self.epoch = self.epoch.wrapping_add(1);
    }

    /// Open `id`'s output, or close it when it is the one open.
    pub fn toggle_output(&mut self, id: &str) {
        self.open = if self.open.as_deref() == Some(id) {
            None
        } else {
            Some(id.to_owned())
        };
    }

    pub fn open_output(&self) -> Option<&str> {
        self.open.as_deref()
    }

    pub fn is_stopping(&self, id: &str) -> bool {
        self.stopping.contains(id)
    }
}

/// The selected chat's background processes, and the device hosting them.
struct Listed {
    chat_id: String,
    host: String,
    tasks: Vec<BackgroundTask>,
    /// The host can stop a process and read its output, and this viewport's
    /// engine can reach it. Otherwise the rows only say what runs.
    actions: bool,
}

const HEADER_HEIGHT: f32 = 32.0;
const ROW_HEIGHT: f32 = 30.0;
const ROW_PAD_X: f32 = 8.0;
const ROW_RADIUS: f32 = crate::queue::ROW_RADIUS;
const TEXT_SIZE: f32 = 12.5;
const MONO_SIZE: f32 = 12.0;
const GLYPH_SLOT: f32 = 14.0;
const ACTION_SIZE: f32 = 22.0;
/// How far each tray steps in from the one below it (the todo tray's step).
const SIDE_INSET: f32 = 16.0;
/// Output lines an open row shows; earlier ones scroll.
const OUTPUT_LINES: usize = 200;
const OUTPUT_LINE: f32 = 16.0;
const OUTPUT_VISIBLE: usize = 8;
/// How often an expanded tray re-reads its clock and its open output.
const TICK: Duration = Duration::from_secs(1);

impl Composer {
    fn selected_background_tasks(&self, cx: &gpui::App) -> Option<Listed> {
        let state = self.state.read(cx);
        let chat_id = state.selected_chat.clone()?;
        let tasks =
            zeron_proto::view::background_tasks(state.session_for(&chat_id), chrono::Utc::now());
        if tasks.is_empty() {
            return None;
        }
        let host = state.selected_chat_row()?.device_id.clone();
        let capability = zeron_proto::capabilities::BACKGROUND_TASKS_V1;
        // A remote host's requests go through this viewport's own engine,
        // which forwards only the methods it knows.
        let actions = state.device_supports(&host, capability)
            && state
                .engine()
                .is_none_or(|engine| engine.engine_info().supports(capability));
        Some(Listed {
            chat_id,
            host,
            tasks: tasks.to_vec(),
            actions,
        })
    }

    /// The background tray, or `None` when the chat's agent runs nothing in
    /// the background. `trays_below`: how many trays stack between it and the
    /// composer (it steps in once per tray).
    pub(crate) fn render_background_tray(
        &mut self,
        trays_below: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let Some(listed) = self.selected_background_tasks(cx) else {
            self.background_tick = None;
            return None;
        };
        let tray = self
            .background_trays
            .entry(listed.chat_id.clone())
            .or_default();
        tray.observe(&listed.tasks);
        let expanded = tray.is_expanded();
        let epoch = tray.epoch;
        let failure = tray.failure.clone();
        // The clock and the open output only show while expanded; a
        // collapsed tray costs no frames however long the processes run.
        if expanded {
            self.ensure_background_tick(cx);
        } else {
            self.background_tick = None;
        }

        let theme = Theme::of(cx).clone();
        let header = self.background_header(&listed, expanded, &theme, cx);
        let surface = crate::queue::queue_panel_surface(&theme, cx.has_active_drag()).child(header);
        let surface = if expanded {
            let now = chrono::Utc::now();
            let rows = listed
                .tasks
                .iter()
                .map(|task| self.background_row(&listed, task, now, &theme, cx))
                .collect::<Vec<_>>();
            let list = crate::edge_fade::edge_faded(
                Theme::TRANSCRIPT_FADE_BAND,
                true,
                true,
                div()
                    .id("background-tray-rows")
                    .max_h(window.viewport_size().height * 0.35)
                    .overflow_y_scroll()
                    .track_scroll(&self.background_scroll)
                    .child(div().flex().flex_col().children(rows)),
            )
            .fade_overflow_y(&self.background_scroll);
            surface.child(
                div()
                    .mt(px(2.0))
                    .pb(px(6.0))
                    .child(list)
                    .when_some(failure, |el, failure| {
                        el.child(
                            div()
                                .px(px(ROW_PAD_X))
                                .pt(px(4.0))
                                .text_size(px(11.5))
                                .text_color(theme.danger_muted)
                                .child(failure),
                        )
                    })
                    .with_animation(
                        SharedString::from(format!("background-rows-{epoch}")),
                        motion::FADE_QUICK.animation(),
                        |el, t| el.opacity(t),
                    ),
            )
        } else {
            surface
        };

        let inset = SIDE_INSET * (trays_below + 1) as f32;
        Some(
            div()
                .mx(px(inset))
                .mb(px(-(Theme::SPACE_SM + QUEUE_COMPOSER_OVERLAP)))
                .child(crate::frost::frosted(
                    crate::queue::PANEL_RADIUS,
                    crate::frost::MENU_BLUR,
                    surface,
                ))
                .into_any_element(),
        )
    }

    /// `Background · 3` and, collapsed, what runs. One button that toggles
    /// the list.
    fn background_header(
        &self,
        listed: &Listed,
        expanded: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let accent = theme.accent;
        let count = listed.tasks.len();
        let label = if expanded {
            "Collapse background processes"
        } else {
            "Show background processes"
        };
        let chat_id = listed.chat_id.clone();
        div()
            .id("background-tray-toggle")
            .role(gpui::Role::Button)
            .aria_label(format!("{label}: {count} running"))
            .h(px(HEADER_HEIGHT))
            .px(px(ROW_PAD_X))
            .rounded(px(ROW_RADIUS))
            .flex()
            .items_center()
            .gap(px(8.0))
            .cursor_pointer()
            .hover(|s| s.bg(theme.element_hover))
            .tab_index(0)
            .focus_visible(move |s| s.bg(accent.opacity(0.14)))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.background_trays
                    .entry(chat_id.clone())
                    .or_default()
                    .toggle();
                cx.notify();
            }))
            .tooltip(crate::settings::widgets::text_tooltip(label))
            .tooltip_show_delay(Duration::from_millis(350))
            .child(
                div()
                    .w(px(GLYPH_SLOT))
                    .flex_none()
                    .flex()
                    .justify_center()
                    .child(icon(icons::TERMINAL).size(px(14.0)).text_color(theme.busy)),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_baseline()
                    .gap(px(6.0))
                    .text_size(px(TEXT_SIZE))
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(theme.text)
                            .child("Background"),
                    )
                    .child(
                        div()
                            .text_color(theme.text_faint)
                            .child(SharedString::from(count.to_string())),
                    ),
            )
            .when(!expanded, |el| {
                el.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_family(theme.font_mono.clone())
                        .text_size(px(MONO_SIZE))
                        .text_color(theme.text_muted)
                        .child(SharedString::from(headline(&listed.tasks))),
                )
            })
            .when(expanded, |el| el.child(div().flex_1()))
            .child(
                icon(if expanded {
                    icons::ALT_ARROW_DOWN
                } else {
                    icons::ALT_ARROW_UP
                })
                .size(px(13.0))
                .text_color(theme.text_muted.opacity(0.7)),
            )
            .into_any_element()
    }

    /// One process: what runs, the agent's word for it, how long it has run,
    /// and its stop button; open, the tail of its output beneath.
    fn background_row(
        &self,
        listed: &Listed,
        task: &BackgroundTask,
        now: chrono::DateTime<chrono::Utc>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let accent = theme.accent;
        let tray = self.background_trays.get(&listed.chat_id);
        let stopping = tray.is_some_and(|t| t.is_stopping(&task.id));
        let open = tray.and_then(BackgroundTrayState::open_output) == Some(task.id.as_str());
        let output = tray.and_then(|t| t.outputs.get(&task.id)).cloned();
        let command = first_line(&task.command).to_owned();
        let row_id = SharedString::from(format!("background-row-{}", task.id));

        let toggle_chat = listed.chat_id.clone();
        let toggle_id = task.id.clone();
        let toggle_host = listed.host.clone();
        let row = div()
            .id(row_id)
            .h(px(ROW_HEIGHT))
            .px(px(ROW_PAD_X))
            .rounded(px(ROW_RADIUS))
            .flex()
            .items_center()
            .gap(px(8.0))
            .when(open, |el| el.bg(theme.element_hover.opacity(0.6)))
            .when(listed.actions, |el| {
                el.role(gpui::Role::Button)
                    .aria_label(format!(
                        "{} output of {command}",
                        if open { "Hide" } else { "Show" }
                    ))
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.element_hover))
                    .tab_index(0)
                    .focus_visible(move |s| s.bg(accent.opacity(0.14)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let tray = this
                            .background_trays
                            .entry(toggle_chat.clone())
                            .or_default();
                        tray.toggle_output(&toggle_id);
                        tray.failure = None;
                        if tray.open_output().is_some() {
                            this.read_background_output(&toggle_chat, &toggle_host, &toggle_id, cx);
                        }
                        cx.notify();
                    }))
            })
            .child(
                div()
                    .w(px(GLYPH_SLOT))
                    .flex_none()
                    .flex()
                    .justify_center()
                    .child(div().size(px(6.0)).rounded_full().bg(if stopping {
                        theme.text_faint
                    } else {
                        theme.busy
                    })),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_baseline()
                    .gap(px(10.0))
                    .child(
                        div()
                            .min_w_0()
                            .flex_shrink(1.0)
                            .truncate()
                            .font_family(theme.font_mono.clone())
                            .text_size(px(MONO_SIZE))
                            .text_color(if stopping {
                                theme.text_faint
                            } else {
                                theme.text
                            })
                            .child(SharedString::from(command.clone())),
                    )
                    .when_some(task.description.clone(), |el, description| {
                        el.child(
                            div()
                                .min_w(px(48.0))
                                .flex_shrink_0()
                                .max_w(px(220.0))
                                .truncate()
                                .text_size(px(11.5))
                                .text_color(theme.text_faint)
                                .child(SharedString::from(description)),
                        )
                    }),
            )
            .child(
                div()
                    .flex_none()
                    .font_family(theme.font_mono.clone())
                    .text_size(px(11.0))
                    .text_color(theme.text_faint)
                    .child(SharedString::from(if stopping {
                        "Stopping…".to_owned()
                    } else {
                        uptime(task, now)
                    })),
            )
            .when(listed.actions, |el| {
                el.child(self.background_stop_button(listed, task, stopping, theme, cx))
            });

        let mut column = div().flex().flex_col().child(row);
        if open && listed.actions {
            column = column.child(background_output_view(
                &task.id,
                output.as_ref(),
                &self.background_output_scroll,
                theme,
            ));
        }
        column.into_any_element()
    }

    fn background_stop_button(
        &self,
        listed: &Listed,
        task: &BackgroundTask,
        stopping: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let accent = theme.accent;
        let danger = theme.danger;
        let command = first_line(&task.command).to_owned();
        let chat_id = listed.chat_id.clone();
        let host = listed.host.clone();
        let id = task.id.clone();
        div()
            .id(SharedString::from(format!("background-stop-{}", task.id)))
            .role(gpui::Role::Button)
            .aria_label(format!("Stop {command}"))
            .size(px(ACTION_SIZE))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(5.0))
            .when(!stopping, |el| {
                el.cursor_pointer()
                    .hover(move |s| s.bg(danger.opacity(0.14)))
                    .tab_index(0)
                    .focus_visible(move |s| s.bg(accent.opacity(0.18)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        // The row's own click opens output; this is a stop.
                        cx.stop_propagation();
                        this.stop_background_task(&chat_id, &host, &id, cx);
                    }))
                    .tooltip(crate::settings::widgets::text_tooltip("Stop process"))
                    .tooltip_show_delay(Duration::from_millis(350))
            })
            .child(icon(icons::STOP).size(px(10.0)).text_color(if stopping {
                theme.text_faint.opacity(0.5)
            } else {
                theme.text_muted
            }))
            .into_any_element()
    }

    /// Ask the chat's host to stop one process. It stays listed (dimmed,
    /// "Stopping…") until the agent reports it gone.
    pub(crate) fn stop_background_task(
        &mut self,
        chat_id: &str,
        host: &str,
        id: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let tray = self.background_trays.entry(chat_id.to_owned()).or_default();
        if !tray.stopping.insert(id.to_owned()) {
            return;
        }
        tray.failure = None;
        cx.notify();
        let params = serde_json::json!({
            "chatId": chat_id,
            "taskId": id,
            "targetDeviceId": host,
        });
        let (chat_id, id) = (chat_id.to_owned(), id.to_owned());
        cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(methods::STOP_BACKGROUND_TASK, params)
                .await;
            let stopping =
                matches!(&result, Ok(reply) if reply["stopping"].as_bool() == Some(true));
            if stopping {
                return;
            }
            if let Err(err) = &result {
                tracing::warn!(error = %err, "background process stop failed");
            }
            let _ = this.update(cx, |composer, cx| {
                let tray = composer.background_trays.entry(chat_id).or_default();
                tray.stopping.remove(&id);
                // Not listed by the host any more: it ended on its own, and
                // the next session row drops it. Anything else failed.
                if result.is_err() {
                    tray.failure = Some("Couldn't stop that process".into());
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn read_background_output(
        &mut self,
        chat_id: &str,
        host: &str,
        id: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let params = serde_json::json!({
            "chatId": chat_id,
            "taskId": id,
            "targetDeviceId": host,
        });
        let (chat_id, id) = (chat_id.to_owned(), id.to_owned());
        cx.spawn(async move |this, cx| {
            let Ok(reply) = engine
                .client()
                .call(methods::READ_BACKGROUND_TASK_OUTPUT, params)
                .await
            else {
                return;
            };
            let Ok(Some(output)) = serde_json::from_value::<Option<BackgroundTaskOutput>>(reply)
            else {
                return;
            };
            let _ = this.update(cx, |composer, cx| {
                let tray = composer.background_trays.entry(chat_id).or_default();
                if tray.outputs.get(&id) == Some(&output) {
                    return;
                }
                let follow = tray.outputs.get(&id).is_none_or(|_| {
                    let scroll = &composer.background_output_scroll;
                    scroll.max_offset().y - (-scroll.offset().y) < px(4.0)
                });
                tray.outputs.insert(id, output);
                if follow {
                    composer.background_output_scroll.scroll_to_bottom();
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// While the tray is expanded: once a second, re-render for the clocks
    /// and re-read the open output. Dropped (and so stopped) when the tray
    /// collapses or empties.
    fn ensure_background_tick(&mut self, cx: &mut Context<Self>) {
        if self.background_tick.is_some() {
            return;
        }
        self.background_tick = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(TICK).await;
                let alive = this.update(cx, |composer, cx| {
                    if let Some(listed) = composer.selected_background_tasks(cx)
                        && let Some(open) = composer
                            .background_trays
                            .get(&listed.chat_id)
                            .and_then(|t| t.open_output().map(str::to_owned))
                    {
                        composer.read_background_output(&listed.chat_id, &listed.host, &open, cx);
                    }
                    cx.notify();
                });
                if alive.is_err() {
                    break;
                }
            }
        }));
    }
}

/// The open row's output: its last lines in the terminal face, following the
/// end as it grows (unless scrolled up).
fn background_output_view(
    id: &str,
    output: Option<&BackgroundTaskOutput>,
    scroll: &gpui::ScrollHandle,
    theme: &Theme,
) -> AnyElement {
    let body = match output {
        None => div()
            .text_color(theme.text_faint)
            .child("Reading output…")
            .into_any_element(),
        Some(output) if output.text.trim().is_empty() => div()
            .text_color(theme.text_faint)
            .child("No output yet")
            .into_any_element(),
        Some(output) => {
            let (lines, earlier) = output_lines(output, OUTPUT_LINES);
            div()
                .flex()
                .flex_col()
                .when(earlier, |el| {
                    el.child(
                        div()
                            .pb(px(2.0))
                            .text_color(theme.text_faint)
                            .child("Earlier output not shown"),
                    )
                })
                .children(lines.into_iter().map(|line| {
                    div()
                        .min_h(px(OUTPUT_LINE))
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(SharedString::from(line.to_owned()))
                }))
                .into_any_element()
        }
    };
    div()
        .mx(px(ROW_PAD_X))
        .mt(px(2.0))
        .mb(px(6.0))
        .ml(px(ROW_PAD_X + GLYPH_SLOT + 8.0))
        .rounded(px(8.0))
        .bg(theme.bg.opacity(0.45))
        .border_1()
        .border_color(theme.border.opacity(0.6))
        // Padding outside the scroll area: scrolled to the end, it shows
        // whole lines rather than a sliver of one under the top edge.
        .px(px(10.0))
        .py(px(8.0))
        .child(
            div()
                .id(SharedString::from(format!("background-output-{id}")))
                .max_h(px(OUTPUT_LINE * OUTPUT_VISIBLE as f32))
                .overflow_y_scroll()
                .track_scroll(scroll)
                .font_family(theme.font_mono.clone())
                .text_size(px(11.0))
                .line_height(px(OUTPUT_LINE))
                .text_color(theme.text_muted)
                .child(body),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: &str, command: &str, ago: i64) -> BackgroundTask {
        BackgroundTask {
            id: id.into(),
            command: command.into(),
            description: None,
            started_at: chrono::Utc::now() - chrono::Duration::seconds(ago),
        }
    }

    #[test]
    fn the_headline_lists_each_command_once() {
        let tasks = [
            task("a", "pnpm dev", 60),
            task("b", "cargo watch -x test\nsecond line", 5),
        ];
        assert_eq!(headline(&tasks), "pnpm dev  ·  cargo watch -x test");
    }

    #[test]
    fn uptime_reads_compactly() {
        let now = chrono::Utc::now();
        let mut t = task("a", "x", 0);
        t.started_at = now - chrono::Duration::seconds(754);
        assert_eq!(uptime(&t, now), "12m 34s");
        t.started_at = now - chrono::Duration::seconds(3 * 3600 + 120);
        assert_eq!(uptime(&t, now), "3h 2m");
    }

    #[test]
    fn output_keeps_its_last_lines() {
        let output = BackgroundTaskOutput {
            text: (0..10)
                .map(|i| format!("line {i}"))
                .collect::<Vec<_>>()
                .join("\n"),
            truncated: false,
        };
        let (lines, earlier) = output_lines(&output, 3);
        assert_eq!(lines, ["line 7", "line 8", "line 9"]);
        assert!(earlier);
        let (lines, earlier) = output_lines(&output, 20);
        assert_eq!(lines.len(), 10);
        assert!(!earlier);
    }

    #[test]
    fn state_forgets_ended_processes() {
        let mut state = BackgroundTrayState::default();
        state.toggle_output("a");
        state.stopping.insert("a".into());
        state
            .outputs
            .insert("a".into(), BackgroundTaskOutput::default());
        state.observe(&[task("b", "x", 0)]);
        assert_eq!(state.open_output(), None);
        assert!(!state.is_stopping("a"));
        assert!(state.outputs.is_empty());
    }

    use crate::state::AppState;
    use gpui::AppContext as _;

    fn window(
        cx: &mut gpui::TestAppContext,
    ) -> (
        tempfile::TempDir,
        gpui::Entity<AppState>,
        gpui::WindowHandle<Composer>,
    ) {
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::dark());
            crate::app_menus::init(cx);
            crate::history::init(
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                cx,
            );
            crate::settings::init(crate::settings::UiSettings::default(), dir.path(), cx);
        });
        let state = cx.new(|_| AppState::new());
        let handle = {
            let state = state.clone();
            cx.add_window(move |_, cx| Composer::new(state, cx))
        };
        (dir, state, handle)
    }

    fn chat(id: &str) -> zeron_proto::Chat {
        zeron_proto::Chat {
            id: id.into(),
            device_id: "host".into(),
            title: None,
            archived: false,
            cwd: None,
            branch: None,
            checkout_id: None,
            source_context: None,
            config: None,
            last_message_preview: None,
            last_message_at: None,
            created_at: chrono::Utc::now(),
            harness_session_id: None,
            harness_session_cwd: None,
            harness_session_harness: None,
            parent_chat_id: None,
            space_id: None,
            last_seen_at: None,
            room_gen: None,
        }
    }

    /// Select `chat` with its host's session row carrying `tasks`, updated
    /// `age` seconds ago, on a host advertising `capable`.
    fn show(
        state: &gpui::Entity<AppState>,
        chat_id: &str,
        tasks: Vec<BackgroundTask>,
        age: i64,
        capable: bool,
        cx: &mut gpui::TestAppContext,
    ) {
        state.update(cx, |state, _| {
            state.chats = vec![chat(chat_id)];
            state.devices = vec![zeron_proto::Device {
                id: "host".into(),
                name: "Host".into(),
                platform: "linux".into(),
                last_seen_at: Some(chrono::Utc::now()),
                created_at: None,
                version: None,
                cursor_sdk_version: None,
                capabilities: if capable {
                    vec![zeron_proto::capabilities::BACKGROUND_TASKS_V1.into()]
                } else {
                    Vec::new()
                },
            }];
            state.sessions = vec![zeron_proto::Session {
                last_completed_turn: None,
                chat_id: chat_id.into(),
                device_id: "host".into(),
                status: zeron_proto::SessionStatus::Idle,
                started_at: None,
                updated_at: chrono::Utc::now() - chrono::Duration::seconds(age),
                last_turn_outcome: None,
                terminal_activity: None,
                running_subagents: 0,
                background_tasks: tasks,
            }];
            state.selected_chat = Some(chat_id.into());
        });
    }

    fn listed(
        handle: &gpui::WindowHandle<Composer>,
        cx: &mut gpui::TestAppContext,
    ) -> Option<(usize, bool)> {
        handle
            .update(cx, |composer, window, cx| {
                composer.render_background_tray(0, window, cx)?;
                composer
                    .selected_background_tasks(cx)
                    .map(|l| (l.tasks.len(), l.actions))
            })
            .unwrap()
    }

    #[gpui::test]
    fn the_tray_shows_only_live_background_processes(cx: &mut gpui::TestAppContext) {
        let (_dir, state, handle) = window(cx);
        show(&state, "a", Vec::new(), 0, true, cx);
        assert_eq!(listed(&handle, cx), None);
        show(&state, "a", vec![task("p", "pnpm dev", 30)], 0, true, cx);
        assert_eq!(listed(&handle, cx), Some((1, true)));
        // A row nobody has refreshed belongs to an engine that took its
        // processes with it.
        let stale = zeron_proto::view::SESSION_STALE_MS / 1000 + 5;
        show(
            &state,
            "a",
            vec![task("p", "pnpm dev", 30)],
            stale,
            true,
            cx,
        );
        assert_eq!(listed(&handle, cx), None);
    }

    #[gpui::test]
    fn a_host_without_the_capability_only_lists(cx: &mut gpui::TestAppContext) {
        let (_dir, state, handle) = window(cx);
        show(&state, "a", vec![task("p", "pnpm dev", 30)], 0, false, cx);
        assert_eq!(listed(&handle, cx), Some((1, false)));
    }

    #[gpui::test]
    fn the_tray_is_collapsed_until_opened_and_remembers_per_chat(cx: &mut gpui::TestAppContext) {
        let (_dir, state, handle) = window(cx);
        show(&state, "a", vec![task("p", "pnpm dev", 30)], 0, true, cx);
        assert!(listed(&handle, cx).is_some());
        handle
            .update(cx, |composer, _, _| {
                assert!(!composer.background_trays["a"].is_expanded());
                composer.background_trays.get_mut("a").unwrap().toggle();
            })
            .unwrap();
        // Rendering expanded runs the clock; collapsing stops it.
        assert!(listed(&handle, cx).is_some());
        handle
            .update(cx, |composer, _, _| {
                assert!(composer.background_tick.is_some());
                composer.background_trays.get_mut("a").unwrap().toggle();
            })
            .unwrap();
        assert!(listed(&handle, cx).is_some());
        handle
            .read_with(cx, |composer, _| {
                assert!(composer.background_tick.is_none())
            })
            .unwrap();
        show(&state, "b", vec![task("q", "cargo watch", 5)], 0, true, cx);
        assert!(listed(&handle, cx).is_some());
        handle
            .read_with(cx, |composer, _| {
                assert!(!composer.background_trays["b"].is_expanded());
            })
            .unwrap();
    }

    #[test]
    fn a_row_toggles_its_own_output() {
        let mut state = BackgroundTrayState::default();
        state.toggle_output("a");
        assert_eq!(state.open_output(), Some("a"));
        state.toggle_output("b");
        assert_eq!(state.open_output(), Some("b"));
        state.toggle_output("b");
        assert_eq!(state.open_output(), None);
    }
}
