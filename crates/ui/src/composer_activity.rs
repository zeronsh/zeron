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

#[derive(Clone)]
pub(super) struct GoalOverride {
    expected: Option<ToolCall>,
    baseline: Option<ToolCall>,
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
        "active" => "In progress",
        "paused" => "Paused",
        "blocked" => "Needs input",
        "usageLimited" => "Usage limit",
        "budgetLimited" => "Budget reached",
        "complete" => "Complete",
        _ => "Goal",
    }
}

fn goal_status_color(status: &str, theme: &Theme) -> gpui::Hsla {
    match status {
        "active" => theme.accent,
        "blocked" | "usageLimited" | "budgetLimited" => theme.warning,
        "complete" => theme.success,
        _ => theme.text_muted,
    }
}

fn goal_history_visible(harness: Option<HarnessId>) -> bool {
    harness == Some(HarnessId::Codex)
}

fn same_goal_lifecycle(expected: &ToolCall, actual: &ToolCall) -> bool {
    matches!(
        (expected, actual),
        (
            ToolCall::Goal {
                objective: expected_objective,
                status: expected_status,
                ..
            },
            ToolCall::Goal {
                objective: actual_objective,
                status: actual_status,
                ..
            }
        ) if expected_objective == actual_objective && expected_status == actual_status
    )
}

fn same_goal_snapshot(expected: &Option<ToolCall>, actual: &Option<ToolCall>) -> bool {
    match (expected, actual) {
        (Some(expected), Some(actual)) => same_goal_lifecycle(expected, actual),
        (None, None) => true,
        (None, Some(ToolCall::Goal { status, .. }))
        | (Some(ToolCall::Goal { status, .. }), None) => status == "cleared",
        _ => false,
    }
}

fn goal_meta(status: &str, tokens_used: u64, token_budget: Option<u64>) -> SharedString {
    match token_budget {
        Some(budget) => format!(
            "{} · {} / {} tokens",
            goal_label(status),
            tokens_used,
            budget
        )
        .into(),
        None => goal_label(status).into(),
    }
}

fn activity_icon_button(
    id: &'static str,
    focus: FocusHandle,
    label: &'static str,
    glyph: &'static str,
    enabled: bool,
    disabled_label: Option<&'static str>,
    danger: bool,
    theme: &Theme,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
    on_key: impl Fn(&KeyDownEvent, &mut Window, &mut App) + 'static,
) -> gpui::AnyElement {
    let accent = theme.accent;
    let hover = if danger { theme.danger } else { theme.text };
    let tooltip = if enabled {
        label
    } else {
        disabled_label.unwrap_or(label)
    };
    div()
        .id(id)
        .track_focus(&focus)
        .role(Role::Button)
        .aria_label(label)
        .size(px(28.0))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.0))
        .text_color(theme.text_muted)
        .when(enabled, |el| {
            el.cursor_pointer()
                .tab_index(0)
                .hover(move |s| s.bg(crate::theme::ink(0.07)).text_color(hover))
                .focus_visible(move |s| s.bg(accent.opacity(0.16)).text_color(accent))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(move |event, window, cx| {
                    cx.stop_propagation();
                    on_click(event, window, cx);
                })
                .on_key_down(move |event, window, cx| {
                    if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                        cx.stop_propagation();
                        on_key(event, window, cx);
                    }
                })
        })
        .when(!enabled, |el| el.opacity(0.36))
        .tooltip(move |_, cx| cx.new(|_| ActivitySummaryTooltip(tooltip.into())).into())
        .tooltip_show_delay(std::time::Duration::from_millis(350))
        .child(
            crate::icons::icon(glyph)
                .size(px(15.0))
                .text_color(theme.text_muted),
        )
        .into_any_element()
}

fn goal_controls_allowed(
    harness: Option<HarnessId>,
    local_transport: bool,
    host_transport: bool,
    goal_advertised: bool,
) -> bool {
    harness == Some(HarnessId::Codex) && local_transport && host_transport && goal_advertised
}

fn goal_from_reply(reply: &serde_json::Value) -> Result<Option<ToolCall>, String> {
    let Some(goal) = reply.get("goal") else {
        return Err("the host returned no goal state".into());
    };
    if goal.is_null() {
        return Ok(None);
    }
    let objective = goal["objective"]
        .as_str()
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| "the host returned an invalid goal".to_string())?;
    Ok(Some(ToolCall::Goal {
        objective: objective.into(),
        status: goal["status"].as_str().unwrap_or("active").into(),
        tokens_used: goal["tokensUsed"].as_u64().unwrap_or_default(),
        token_budget: goal["tokenBudget"].as_u64(),
    }))
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
    fn goal_controls_available(&self, chat_id: &str, cx: &App) -> bool {
        let state = self.state.read(cx);
        let harness = state
            .selected_chat_row()
            .and_then(|chat| chat.config.as_ref())
            .map(|config| config.harness);
        let local_transport = state
            .engine()
            .is_some_and(|engine| engine.engine_info().supports(capabilities::GOAL_ACTIONS_V1));
        let host_transport = state.chat_host_supports(chat_id, capabilities::GOAL_ACTIONS_V1);
        let advertised = self
            .available_modes(cx)
            .is_some_and(|option| option.choices.iter().any(|choice| choice.id == "goal"));
        goal_controls_allowed(harness, local_transport, host_transport, advertised)
    }

    fn begin_goal_edit(
        &mut self,
        chat_id: String,
        objective: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.goal_pending.contains_key(&chat_id) {
            return;
        }
        self.goal_edit_chat = Some(chat_id);
        self.goal_input
            .update(cx, |input, cx| input.set_text(objective, cx));
        window.focus(&self.goal_input.focus_handle(cx), cx);
        cx.notify();
    }

    fn cancel_goal_edit(&mut self, cx: &mut Context<Self>) {
        self.goal_edit_chat = None;
        self.goal_input
            .update(cx, |input, cx| input.set_text("", cx));
        cx.notify();
    }

    pub(super) fn submit_goal_edit(&mut self, cx: &mut Context<Self>) {
        let Some(chat_id) = self.goal_edit_chat.clone() else {
            return;
        };
        let objective = self.goal_input.read(cx).text().trim().to_owned();
        if objective.is_empty() {
            self.failure = Some("Goal objective cannot be empty".into());
            self.failure_key = Some(chat_id);
            cx.notify();
            return;
        }
        self.set_goal("edit", Some(objective), cx);
    }

    fn set_goal(
        &mut self,
        action: &'static str,
        objective: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let state = self.state.read(cx);
        let Some(chat_id) = state.selected_chat.clone() else {
            return;
        };
        let Some(engine) = state.engine().cloned() else {
            return;
        };
        let Some(target_device_id) = state.selected_chat_row().map(|chat| chat.device_id.clone())
        else {
            return;
        };
        let baseline = activity(&state.transcript).goal.cloned();
        if self.goal_pending.contains_key(&chat_id) || !self.goal_controls_available(&chat_id, cx) {
            return;
        }
        self.goal_pending.insert(chat_id.clone(), action.into());
        cx.notify();
        let mut params = serde_json::json!({
            "chatId": chat_id,
            "targetDeviceId": target_device_id,
            "action": action,
        });
        if let Some(objective) = objective {
            params["objective"] = objective.into();
        }
        cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(methods::SET_GOAL, params)
                .await
                .map_err(|error| error.to_string())
                .and_then(|reply| goal_from_reply(&reply));
            this.update(cx, |composer, cx| {
                composer.goal_pending.remove(&chat_id);
                match result {
                    Ok(goal) => {
                        composer.goal_overrides.insert(
                            chat_id.clone(),
                            GoalOverride {
                                expected: goal,
                                baseline,
                            },
                        );
                        if composer.goal_edit_chat.as_deref() == Some(chat_id.as_str()) {
                            composer.goal_edit_chat = None;
                            composer
                                .goal_input
                                .update(cx, |input, cx| input.set_text("", cx));
                        }
                    }
                    Err(error) => {
                        composer.failure = Some(format!("Goal update failed: {error}").into());
                        composer.failure_key = Some(chat_id.clone());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn render_agent_activity(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let chat_id = self.state.read(cx).selected_chat.clone()?;
        if self.activity_chat.as_ref() != Some(&chat_id) {
            self.activity_chat = Some(chat_id.clone());
            self.activity_expanded = false;
            self.activity_height = 0.0;
            self.activity_motion = ActivityMotion::default();
            self.activity_scroll.set_offset(point(px(0.0), px(0.0)));
            self.activity_plan = None;
            self.goal_edit_chat = None;
        }
        let has_question = self.wizard.is_some();
        let (has_queue, is_codex, plan, todos, transcript_goal) = {
            let state = self.state.read(cx);
            let latest = activity(&state.transcript);
            (
                !state.queue.is_empty(),
                goal_history_visible(
                    state
                        .selected_chat_row()
                        .and_then(|chat| chat.config.as_ref())
                        .map(|config| config.harness),
                ),
                latest
                    .plan
                    .filter(|text| !text.trim().is_empty())
                    .map(str::to_owned),
                latest
                    .todos
                    .as_deref()
                    .filter(|items| !items.is_empty())
                    .map(<[zeron_proto::TodoItem]>::to_vec),
                latest.goal.cloned(),
            )
        };
        // Goal state is Codex-native. A retained Codex transcript may remain
        // after changing the chat harness; never present that history as a
        // live goal owned by another provider.
        let goal = if !is_codex {
            self.goal_overrides.remove(&chat_id);
            self.goal_edit_chat = None;
            None
        } else if let Some(optimistic) = self.goal_overrides.get(&chat_id).cloned() {
            let settled = same_goal_snapshot(&optimistic.expected, &transcript_goal);
            let provider_advanced = !same_goal_snapshot(&optimistic.baseline, &transcript_goal);
            if settled || provider_advanced {
                self.goal_overrides.remove(&chat_id);
                transcript_goal.clone().filter(
                    |goal| !matches!(goal, ToolCall::Goal { status, .. } if status == "cleared"),
                )
            } else {
                optimistic.expected
            }
        } else {
            transcript_goal.clone().filter(
                |goal| !matches!(goal, ToolCall::Goal { status, .. } if status == "cleared"),
            )
        };
        if plan.is_none() && todos.is_none() && goal.is_none() {
            return None;
        }
        let theme = Theme::of(cx).for_popup();
        let mut rows = div().flex().flex_col().gap(px(4.0)).px(px(8.0)).py(px(6.0));
        let mut details = div().relative().flex().flex_col().gap(px(12.0)).p(px(8.0));
        if let Some(ToolCall::Goal {
            objective,
            status,
            tokens_used,
            token_budget,
        }) = goal.as_ref()
        {
            let editing = self.goal_edit_chat.as_deref() == Some(chat_id.as_str());
            let edit_input_height = self
                .goal_input
                .read(cx)
                .measured_content_height()
                .clamp(18.0, 54.0);
            if editing {
                self.goal_input.update(cx, |input, cx| {
                    if input.viewport_height != Some(edit_input_height)
                        || input.settled_viewport_height != Some(edit_input_height)
                    {
                        input.viewport_height = Some(edit_input_height);
                        input.settled_viewport_height = Some(edit_input_height);
                        input.resizing = false;
                        input.overflow_top_padding = 0.0;
                        cx.notify();
                    }
                });
            }
            let pending = self.goal_pending.get(&chat_id).cloned();
            let controls_available = self.goal_controls_available(&chat_id, cx);
            let enabled = controls_available && pending.is_none();
            let disabled_hint = if pending.is_some() {
                "Goal update in progress"
            } else {
                "Goal controls require native Goal support on the chat host"
            };
            let dot_color = if pending.is_some() {
                theme.accent
            } else {
                goal_status_color(status, &theme)
            };
            let meta: SharedString = pending
                .as_ref()
                .map(|_| SharedString::from("Updating…"))
                .unwrap_or_else(|| goal_meta(status, *tokens_used, *token_budget));
            let mut row = div()
                .id("goal-activity-row")
                .min_h(px(34.0))
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(div().size(px(7.0)).rounded_full().flex_none().bg(dot_color));
            if editing {
                row = row.child(
                    div()
                        .h(px(edit_input_height + 12.0))
                        .flex_1()
                        .min_w_0()
                        .rounded(px(7.0))
                        .border_1()
                        .border_color(theme.accent.opacity(0.56))
                        .bg(crate::theme::ink(0.035))
                        .px(px(7.0))
                        .overflow_hidden()
                        .child(self.goal_input.clone()),
                );
                row = row
                    .child(activity_icon_button(
                        "goal-save",
                        self.goal_action_focuses["goal-save"].clone(),
                        if pending.is_some() {
                            "Saving…"
                        } else {
                            "Save goal"
                        },
                        crate::icons::CHECK,
                        enabled,
                        Some(disabled_hint),
                        false,
                        &theme,
                        cx.listener(|this, _, _, cx| this.submit_goal_edit(cx)),
                        cx.listener(|this, _, _, cx| this.submit_goal_edit(cx)),
                    ))
                    .child(activity_icon_button(
                        "goal-cancel",
                        self.goal_action_focuses["goal-cancel"].clone(),
                        "Cancel edit",
                        crate::icons::CLOSE,
                        pending.is_none(),
                        pending.is_some().then_some(disabled_hint),
                        false,
                        &theme,
                        cx.listener(|this, _, _, cx| this.cancel_goal_edit(cx)),
                        cx.listener(|this, _, _, cx| this.cancel_goal_edit(cx)),
                    ));
            } else {
                let objective_tooltip: SharedString = objective.clone().into();
                row = row.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(1.0))
                        .child(
                            div()
                                .id("goal-objective")
                                .truncate()
                                .text_size(crate::typography::ui_rems(12.0))
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(theme.text)
                                .tooltip(move |_, cx| {
                                    cx.new(|_| ActivitySummaryTooltip(objective_tooltip.clone()))
                                        .into()
                                })
                                .tooltip_show_delay(std::time::Duration::from_millis(500))
                                .child(SharedString::from(objective.clone())),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_size(crate::typography::ui_rems(10.5))
                                .text_color(theme.text_muted)
                                .child(meta),
                        ),
                );
                let lifecycle = if status == "active" {
                    ("Pause goal", crate::icons::PAUSE, "pause")
                } else {
                    ("Resume goal", crate::icons::PLAY, "resume")
                };
                row = row
                    .child(activity_icon_button(
                        "goal-lifecycle",
                        self.goal_action_focuses["goal-lifecycle"].clone(),
                        lifecycle.0,
                        lifecycle.1,
                        enabled,
                        Some(disabled_hint),
                        false,
                        &theme,
                        cx.listener(move |this, _, _, cx| this.set_goal(lifecycle.2, None, cx)),
                        cx.listener(move |this, _, _, cx| this.set_goal(lifecycle.2, None, cx)),
                    ))
                    .child(activity_icon_button(
                        "goal-edit",
                        self.goal_action_focuses["goal-edit"].clone(),
                        "Edit goal",
                        crate::icons::PEN,
                        enabled,
                        Some(disabled_hint),
                        false,
                        &theme,
                        {
                            let chat_id = chat_id.clone();
                            let objective = objective.clone();
                            cx.listener(move |this, _, window, cx| {
                                this.begin_goal_edit(chat_id.clone(), objective.clone(), window, cx)
                            })
                        },
                        {
                            let chat_id = chat_id.clone();
                            let objective = objective.clone();
                            cx.listener(move |this, _, window, cx| {
                                this.begin_goal_edit(chat_id.clone(), objective.clone(), window, cx)
                            })
                        },
                    ))
                    .child(activity_icon_button(
                        "goal-clear",
                        self.goal_action_focuses["goal-clear"].clone(),
                        "Delete goal",
                        crate::icons::TRASH_BIN_MINIMALISTIC,
                        enabled,
                        Some(disabled_hint),
                        true,
                        &theme,
                        cx.listener(|this, _, _, cx| this.set_goal("clear", None, cx)),
                        cx.listener(|this, _, _, cx| this.set_goal("clear", None, cx)),
                    ));
            }
            rows = rows.child(row);
        }
        // A collapsed plan may stream thousands of tokens. Parse only while
        // visible (or finishing its exit), without reparsing on scroll/hover.
        if let Some(text) = plan
            .as_deref()
            .filter(|_| self.activity_expanded || self.activity_motion.from > 0.5)
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
        if let Some(items) = todos.as_deref() {
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
        let has_details = plan.is_some() || todos.is_some();
        if has_details {
            let summary = [
                plan.is_some().then(|| "Plan".to_owned()),
                todos.as_deref().map(task_summary),
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
                    .rounded(px(7.0))
                    .border_1()
                    .border_color(crate::theme::hairline(0.0))
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
                    .focus_visible(move |s| {
                        s.bg(theme.accent.opacity(0.08))
                            .border_color(theme.accent.opacity(0.62))
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

#[cfg(feature = "appshots-fixture")]
impl Composer {
    /// Deterministically focuses a rendered production goal control for the
    /// native fixture. Activation still travels through its real keyboard
    /// handler and SetGoal RPC path.
    pub fn fixture_focus_goal_control(
        &self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(focus) = self.goal_action_focuses.get(id) else {
            return false;
        };
        window.focus(focus, cx);
        cx.notify();
        true
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
    fn retained_goal_history_is_visible_only_for_codex() {
        assert!(goal_history_visible(Some(HarnessId::Codex)));
        for harness in [
            HarnessId::ClaudeCode,
            HarnessId::Cursor,
            HarnessId::Devin,
            HarnessId::Grok,
            HarnessId::Hermes,
            HarnessId::Pi,
            HarnessId::Opencode,
            HarnessId::Antigravity,
            HarnessId::Mock,
        ] {
            assert!(!goal_history_visible(Some(harness)), "{harness:?}");
        }
        assert!(!goal_history_visible(None));
    }

    #[test]
    fn goal_controls_require_codex_both_transports_and_advertised_goal_mode() {
        assert!(goal_controls_allowed(
            Some(HarnessId::Codex),
            true,
            true,
            true
        ));
        for unavailable in [
            (Some(HarnessId::ClaudeCode), true, true, true),
            (Some(HarnessId::Codex), false, true, true),
            (Some(HarnessId::Codex), true, false, true),
            (Some(HarnessId::Codex), true, true, false),
            (None, true, true, true),
        ] {
            assert!(!goal_controls_allowed(
                unavailable.0,
                unavailable.1,
                unavailable.2,
                unavailable.3
            ));
        }
    }

    #[test]
    fn goal_rpc_reply_preserves_lifecycle_and_clear_state() {
        let active = serde_json::json!({
            "goal": {
                "objective": "Ship the compact activity panel",
                "status": "active",
                "tokensUsed": 42,
                "tokenBudget": 1000
            }
        });
        assert_eq!(
            goal_from_reply(&active).unwrap(),
            Some(ToolCall::Goal {
                objective: "Ship the compact activity panel".into(),
                status: "active".into(),
                tokens_used: 42,
                token_budget: Some(1000),
            })
        );
        let cleared = serde_json::json!({ "goal": null });
        assert_eq!(goal_from_reply(&cleared).unwrap(), None);
    }

    #[test]
    fn optimistic_goal_yields_to_a_newer_provider_lifecycle() {
        let goal = |status: &str, tokens_used| {
            Some(ToolCall::Goal {
                objective: "Ship it".into(),
                status: status.into(),
                tokens_used,
                token_budget: Some(1_000),
            })
        };
        let baseline = goal("active", 10);
        let expected = goal("paused", 10);
        let token_refresh = goal("active", 20);
        let newer_blocked = goal("blocked", 20);

        assert!(same_goal_snapshot(&baseline, &token_refresh));
        assert!(!same_goal_snapshot(&expected, &token_refresh));
        assert!(!same_goal_snapshot(&baseline, &newer_blocked));
        assert!(!same_goal_snapshot(&expected, &newer_blocked));
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
