//! Native agent activity in the same glass tray used above the composer.
use super::*;
use zeron_proto::ToolCall;

struct ActivitySummaryTooltip(SharedString);

impl Render for ActivitySummaryTooltip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        crate::frost::frosted(
            6.0,
            crate::frost::MENU_BLUR,
            div()
                .max_w(px(360.0))
                .px(px(8.0))
                .py(px(5.0))
                .rounded(px(6.0))
                .border_1()
                .border_color(theme.border)
                .bg(crate::popover::surface_bg(theme))
                .text_size(crate::typography::ui_rems(10.5))
                .text_color(theme.text_muted)
                .child(self.0.clone()),
        )
    }
}

#[derive(Default)]
struct Activity<'a> {
    plan: Option<&'a str>,
    todos: Option<std::borrow::Cow<'a, [zeron_proto::TodoItem]>>,
    goal: Option<&'a ToolCall>,
}

/// Independent snapshots: an empty list clears tasks without resurrecting an
/// older list. Failed writes never replace the last successful snapshot.
fn activity(transcript: &[SessionMessageEntry]) -> Activity<'_> {
    let mut latest = Activity::default();
    let mut patches = Vec::new();
    for part in transcript
        .iter()
        .rev()
        .filter(|entry| entry.role == MessageRole::Assistant)
        .flat_map(|entry| entry.parts.iter().rev())
    {
        if let MessagePart::Tool {
            call,
            is_error: false,
            resolved,
            ..
        } = part
        {
            match call {
                ToolCall::Plan { text } if latest.plan.is_none() => latest.plan = Some(text),
                ToolCall::Todo { items } if *resolved && latest.todos.is_none() => {
                    latest.todos = Some(std::borrow::Cow::Borrowed(items))
                }
                ToolCall::TodoPatch { .. } if *resolved && latest.todos.is_none() => {
                    patches.push(call)
                }
                ToolCall::Goal { .. } if latest.goal.is_none() => latest.goal = Some(call),
                _ => {}
            }
        }
        if latest.plan.is_some() && latest.todos.is_some() && latest.goal.is_some() {
            break;
        }
    }
    if !patches.is_empty() {
        let items = latest
            .todos
            .get_or_insert_with(|| std::borrow::Cow::Owned(Vec::new()))
            .to_mut();
        for patch in patches.into_iter().rev() {
            if let ToolCall::TodoPatch {
                task_id,
                text,
                status,
            } = patch
            {
                if status.as_deref() == Some("deleted") {
                    items.retain(|item| item.id.as_ref() != Some(task_id));
                    continue;
                }
                if !items.iter().any(|item| item.id.as_ref() == Some(task_id)) {
                    let Some(text) = text else {
                        continue;
                    };
                    items.push(zeron_proto::TodoItem {
                        id: Some(task_id.clone()),
                        text: text.clone(),
                        done: false,
                        status: None,
                    });
                }
                let item = items
                    .iter_mut()
                    .find(|item| item.id.as_ref() == Some(task_id))
                    .unwrap();
                if let Some(text) = text {
                    item.text = text.clone();
                }
                if let Some(status) = status {
                    item.done = status == "completed";
                    item.status = zeron_proto::TodoStatus::from_wire(Some(status));
                }
            }
        }
    }
    latest
}

#[derive(Default)]
pub(super) struct ActivityMotion {
    from: f32,
    to: f32,
    started: Option<std::time::Instant>,
}
impl ActivityMotion {
    fn sample(&mut self, target: f32, now: std::time::Instant, reduced: bool) -> (f32, bool) {
        let current = self.started.map_or(self.to, |start| {
            let t = (now.duration_since(start).as_secs_f32() / 0.18).clamp(0.0, 1.0);
            motion::lerp(self.from, self.to, motion::EASE_OUT.eval(t))
        });
        if reduced || self.started.is_none() {
            self.from = target;
            self.to = target;
            self.started = Some(now);
            return (target, false);
        }
        if (target - self.to).abs() > 0.5 {
            self.from = current;
            self.to = target;
            self.started = Some(now);
        }
        let moving = (current - self.to).abs() > 0.5;
        if !moving {
            self.from = self.to;
        }
        (if moving { current } else { self.to }, moving)
    }
}

fn goal_label(status: &str) -> &'static str {
    match status {
        "active" => "Goal in progress",
        "paused" => "Goal paused",
        "blocked" => "Goal needs input",
        "usageLimited" => "Goal paused · usage limit",
        "budgetLimited" => "Goal paused · budget reached",
        "complete" => "Goal complete",
        _ => "Goal",
    }
}

fn task_summary(items: &[zeron_proto::TodoItem]) -> String {
    use zeron_proto::TodoStatus;
    let done = items
        .iter()
        .filter(|item| item.state() == TodoStatus::Completed)
        .count();
    let cancelled = items
        .iter()
        .filter(|item| item.state() == TodoStatus::Cancelled)
        .count();
    if cancelled == items.len() && !items.is_empty() {
        return "Tasks cancelled".into();
    }
    if done + cancelled == items.len() && !items.is_empty() {
        return if cancelled == 0 {
            "All tasks complete".into()
        } else {
            format!("{done} complete · {cancelled} cancelled")
        };
    }
    let mut summary = format!("Tasks · {done}/{} complete", items.len());
    for (state, label) in [
        (TodoStatus::InProgress, "in progress"),
        (TodoStatus::Blocked, "blocked"),
        (TodoStatus::Cancelled, "cancelled"),
    ] {
        let count = items.iter().filter(|item| item.state() == state).count();
        if count > 0 {
            summary.push_str(&format!(" · {count} {label}"));
        }
    }
    summary
}

impl Composer {
    pub(super) fn render_agent_activity(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let state = self.state.read(cx);
        let chat = state.selected_chat.as_ref()?;
        if self.activity_chat.as_ref() != Some(chat) {
            self.activity_chat = Some(chat.clone());
            self.activity_expanded = false;
            self.activity_height = 0.0;
            self.activity_motion = ActivityMotion::default();
            self.activity_scroll.set_offset(point(px(0.0), px(0.0)));
            self.activity_plan = None;
        }
        let has_question = self.wizard.is_some();
        let has_queue = !state.queue.is_empty();
        let latest = activity(&state.transcript);
        let plan = latest.plan.filter(|text| !text.trim().is_empty());
        let todos = latest.todos.as_deref().filter(|items| !items.is_empty());
        let goal = latest
            .goal
            .filter(|goal| !matches!(goal, ToolCall::Goal { status, .. } if status == "cleared"));
        if plan.is_none() && todos.is_none() && goal.is_none() {
            return None;
        }
        let theme = Theme::of(cx).for_popup();
        let mut rows = div().flex().flex_col().gap(px(6.0)).p(px(8.0));
        let mut details = div().relative().flex().flex_col().gap(px(12.0)).p(px(8.0));
        if let Some(ToolCall::Goal {
            objective,
            status,
            tokens_used,
            token_budget,
        }) = goal
        {
            if status != "cleared" {
                let label = goal_label(status);
                details = details.child(
                    div()
                        .text_size(crate::typography::ui_rems(12.0))
                        .text_color(theme.text)
                        .child(label)
                        .child(div().child(SharedString::from(objective.clone())))
                        .when_some(*token_budget, |el, budget| {
                            el.child(div().text_color(theme.text_muted).child(SharedString::from(
                                format!("{tokens_used} / {budget} tokens"),
                            )))
                        }),
                );
            }
        }
        // A collapsed plan may stream thousands of tokens. Parse only while
        // visible (or finishing its exit), without reparsing on scroll/hover.
        if let Some(text) =
            plan.filter(|_| self.activity_expanded || self.activity_motion.from > 0.5)
        {
            if self
                .activity_plan
                .as_ref()
                .is_none_or(|(previous, _)| previous != text)
            {
                self.activity_plan = Some((text.to_owned(), crate::markdown::parse_full(text)));
            }
            let opts = crate::markdown::render::RenderOptions::settled("composer-plan".into());
            details = details.child(crate::markdown::render::render_tree(
                &self.activity_plan.as_ref().unwrap().1,
                &opts,
                &theme,
                window,
                &|_| None,
            ));
        }
        if let Some(items) = todos {
            let mut list = div().flex().flex_col().gap(px(6.0));
            for (index, item) in items.iter().enumerate() {
                use zeron_proto::TodoStatus;
                let (glyph, label) = match item.state() {
                    TodoStatus::Completed => ("✓", "Complete"),
                    TodoStatus::InProgress => ("◉", "In progress"),
                    TodoStatus::Cancelled => ("−", "Cancelled"),
                    TodoStatus::Blocked => ("!", "Blocked"),
                    TodoStatus::Pending => ("○", "Pending"),
                };
                let muted = matches!(item.state(), TodoStatus::Completed | TodoStatus::Cancelled);
                list = list.child(
                    div()
                        .id(("activity-task", index))
                        .flex()
                        .items_start()
                        .gap(px(8.0))
                        .text_size(crate::typography::ui_rems(12.0))
                        .text_color(if muted { theme.text_muted } else { theme.text })
                        .child(div().w(px(14.0)).flex_none().child(glyph))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(SharedString::from(item.text.clone()))
                                .child(
                                    div()
                                        .text_size(crate::typography::ui_rems(10.0))
                                        .text_color(theme.text_muted)
                                        .child(label),
                                ),
                        ),
                );
            }
            details = details.child(list);
        }
        let has_details = plan.is_some() || todos.is_some() || goal.is_some();
        if has_details {
            let summary = [
                goal.map(|goal| match goal {
                    ToolCall::Goal { status, .. } => goal_label(status).to_owned(),
                    _ => unreachable!(),
                }),
                todos.map(task_summary),
                (plan.is_some() && todos.is_none()).then(|| "Plan".to_owned()),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ");
            let summary_tooltip: SharedString = summary.clone().into();
            let expanded = self.activity_expanded;
            let focus = self.activity_focus.clone();
            rows = rows.child(
                div()
                    .id("activity-disclosure")
                    .role(Role::Button)
                    .track_focus(&focus)
                    .tab_index(0)
                    .aria_label(SharedString::from(format!(
                        "{summary} · {} details",
                        if expanded { "Hide" } else { "Show" }
                    )))
                    .aria_expanded(expanded)
                    .min_h(px(28.0))
                    .px(px(6.0))
                    .rounded(px(8.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .cursor_pointer()
                    .tooltip(move |_, cx| {
                        cx.new(|_| ActivitySummaryTooltip(summary_tooltip.clone()))
                            .into()
                    })
                    .tooltip_show_delay(std::time::Duration::from_millis(500))
                    .hover(|s| s.bg(crate::theme::ink(0.05)))
                    .when(focus.is_focused(window), |el| {
                        el.shadow(vec![gpui::BoxShadow {
                            color: theme.accent,
                            offset: point(px(0.0), px(0.0)),
                            blur_radius: px(0.0),
                            spread_radius: px(2.0),
                            inset: true,
                        }])
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.activity_expanded = !this.activity_expanded;
                        cx.notify();
                    }))
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                        if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                            this.activity_expanded = !this.activity_expanded;
                            cx.notify();
                            cx.stop_propagation();
                        } else if event.keystroke.key == "escape" && this.activity_expanded {
                            this.activity_expanded = false;
                            cx.notify();
                            cx.stop_propagation();
                        }
                    }))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(crate::typography::ui_rems(12.0))
                            .child(SharedString::from(summary)),
                    )
                    .child(
                        crate::icons::icon(if expanded {
                            crate::icons::ALT_ARROW_UP
                        } else {
                            crate::icons::ALT_ARROW_DOWN
                        })
                        .size(px(14.0))
                        .text_color(theme.text_muted),
                    ),
            );
        }
        let entity = cx.entity().downgrade();
        details = details.child(
            gpui::canvas(
                move |bounds, _, cx| {
                    let _ = entity.update(cx, |this, cx| {
                        let height = f32::from(bounds.size.height);
                        if (height - this.activity_height).abs() > 0.5 {
                            this.activity_height = height;
                            cx.notify();
                        }
                    });
                },
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0(),
        );
        // Other trays take priority; expanded context remains available within
        // a shared budget so the answer and queue controls stay reachable.
        let limit = composer_tray_limits(
            f32::from(window.viewport_size().height),
            has_question,
            has_queue,
        )
        .activity;
        let target = if self.activity_expanded && has_details {
            self.activity_height.min(limit)
        } else {
            0.0
        };
        let (height, moving) =
            self.activity_motion
                .sample(target, std::time::Instant::now(), cx.reduce_motion());
        if moving {
            window.request_animation_frame();
        }
        let scroll = crate::edge_fade::edge_faded(
            Theme::TRANSCRIPT_FADE_BAND,
            true,
            true,
            div()
                .id("agent-activity-rows")
                .max_h(px(limit))
                .overflow_y_scroll()
                .track_scroll(&self.activity_scroll)
                .child(details),
        )
        .fade_overflow_y(&self.activity_scroll);
        let body = div().h(px(height)).overflow_hidden().child(scroll);
        Some(
            div()
                .mx(px(QUEUE_SIDE_INSET))
                .mb(px(-(Theme::SPACE_SM + QUEUE_COMPOSER_OVERLAP)))
                .child(crate::frost::frosted(
                    crate::queue::PANEL_RADIUS,
                    crate::frost::MENU_BLUR,
                    crate::queue::queue_panel_surface(&theme)
                        .child(rows)
                        .child(body),
                ))
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeron_proto::{TodoItem, TodoStatus};
    fn task(status: TodoStatus) -> TodoItem {
        TodoItem {
            id: None,
            text: "Task".into(),
            done: status == TodoStatus::Completed,
            status: Some(status),
        }
    }
    fn entry(calls: Vec<(ToolCall, bool, bool)>) -> SessionMessageEntry {
        SessionMessageEntry { id: "entry".into(), role: MessageRole::Assistant, created_at: 0, device_id: "device".into(), status: None, continuation_of: None,
            parts: calls.into_iter().enumerate().map(|(index, (call, error, resolved))| serde_json::from_value(serde_json::json!({
                "kind":"tool", "id": index.to_string(), "call":call, "isError":error, "resolved":resolved
            })).unwrap()).collect() }
    }
    #[test]
    fn activity_fixture_states_parse_and_clear_independently() {
        let cases: serde_json::Value = serde_json::from_str(include_str!(
            "../../../scripts/fixtures/composer-activity.json"
        ))
        .unwrap();
        for case in cases.as_array().unwrap() {
            let calls: Vec<ToolCall> = serde_json::from_value(case["calls"].clone()).unwrap();
            let entries = [entry(
                calls.into_iter().map(|call| (call, false, true)).collect(),
            )];
            let latest = activity(&entries);
            match case["name"].as_str().unwrap() {
                "empty" => assert!(
                    latest.plan.is_none() && latest.todos.is_none() && latest.goal.is_none()
                ),
                "cleared-tasks" => assert!(latest.todos.unwrap().is_empty()),
                "combined" => assert!(
                    latest.plan.is_some() && latest.todos.is_some() && latest.goal.is_some()
                ),
                _ => assert!(
                    latest.plan.is_some() || latest.todos.is_some() || latest.goal.is_some()
                ),
            }
        }
    }

    #[test]
    fn plans_and_tasks_coexist_and_failed_or_pending_writes_do_not_replace_tasks() {
        let entries = vec![entry(vec![
            (
                ToolCall::Todo {
                    items: vec![task(TodoStatus::InProgress)],
                },
                false,
                true,
            ),
            (
                ToolCall::Plan {
                    text: "Plan".into(),
                },
                false,
                false,
            ),
            (
                ToolCall::Todo {
                    items: vec![task(TodoStatus::Completed)],
                },
                true,
                true,
            ),
            (ToolCall::Todo { items: vec![] }, false, false),
        ])];
        let latest = activity(&entries);
        assert_eq!(latest.plan, Some("Plan"));
        assert_eq!(latest.todos.unwrap()[0].state(), TodoStatus::InProgress);
    }
    #[test]
    fn empty_snapshots_clear_without_resurrecting_history() {
        let entries = vec![entry(vec![
            (
                ToolCall::Todo {
                    items: vec![task(TodoStatus::Pending)],
                },
                false,
                true,
            ),
            (ToolCall::Todo { items: vec![] }, false, true),
            (ToolCall::Plan { text: "".into() }, false, true),
        ])];
        let latest = activity(&entries);
        assert!(latest.todos.unwrap().is_empty());
        assert_eq!(latest.plan, Some(""));
    }
    #[test]
    fn resumed_task_patches_reconstruct_ids_without_process_memory() {
        let mut entries = vec![
            entry(vec![
                (
                    ToolCall::TodoPatch {
                        task_id: "1".into(),
                        text: Some("Build".into()),
                        status: Some("pending".into()),
                    },
                    false,
                    true,
                ),
                (
                    ToolCall::TodoPatch {
                        task_id: "2".into(),
                        text: Some("Test".into()),
                        status: Some("pending".into()),
                    },
                    false,
                    true,
                ),
            ]),
            entry(vec![(
                ToolCall::TodoPatch {
                    task_id: "1".into(),
                    text: None,
                    status: Some("completed".into()),
                },
                false,
                true,
            )]),
        ];
        let latest = activity(&entries);
        let items = latest.todos.unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].state(), TodoStatus::Completed);
        assert_eq!(items[1].text, "Test");
        entries.push(entry(vec![(
            ToolCall::TodoPatch {
                task_id: "1".into(),
                text: None,
                status: Some("deleted".into()),
            },
            false,
            true,
        )]));
        assert_eq!(activity(&entries).todos.unwrap().len(), 1);
    }

    #[test]
    fn cancellation_never_claims_all_tasks_completed() {
        assert_eq!(
            task_summary(&[task(TodoStatus::Cancelled)]),
            "Tasks cancelled"
        );
        assert_eq!(
            task_summary(&[task(TodoStatus::Completed), task(TodoStatus::Cancelled)]),
            "1 complete · 1 cancelled"
        );
        assert_eq!(
            task_summary(&[task(TodoStatus::Completed)]),
            "All tasks complete"
        );
    }
    #[test]
    fn reveal_retargets_continuously_and_reduced_motion_settles() {
        let now = std::time::Instant::now();
        let mut motion = ActivityMotion::default();
        assert_eq!(motion.sample(0.0, now, false), (0.0, false));
        assert_eq!(motion.sample(160.0, now, false), (0.0, true));
        let later = now + Duration::from_millis(90);
        let (mid, _) = motion.sample(160.0, later, false);
        assert!(mid > 0.0 && mid < 160.0);
        assert_eq!(motion.sample(0.0, later, false).0, mid);
        assert_eq!(motion.sample(0.0, later, true), (0.0, false));
    }
}
