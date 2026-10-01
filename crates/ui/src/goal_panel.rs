//! Goal mode in the composer dock: the goal tray, `/goal`, and the transcript
//! markers.
//!
//! `/goal <objective>` keeps a chat working until an independent verifier chat
//! judges the objective met (`docs/goal-mode.md`). The goal itself is
//! replicated with the chat's doc; this file renders it. It sits in the same
//! tray stack as the todo list and the queue, one step narrower than whatever
//! is below it.
//!
//! The first half is pure — parsing `/goal`, what the header says, how rounds
//! and their todo items are derived from the transcript — and unit-tested
//! without a window. The second half is gpui rendering on [`Composer`].

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use gpui::{
    AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled, Window, div, prelude::*, px,
};

use zeron_doc::{MessagePart, MessageRole, SessionMessageEntry};
use zeron_proto::{
    GOAL_OBJECTIVE_MAX_CHARS, Goal, GoalCommand, GoalEventKind, GoalLimits,
    GoalStatus, MessageOrigin, TodoStatus, ToolCall, VerdictOutcome,
};

use crate::composer::{Composer, QUEUE_COMPOSER_OVERLAP};
use crate::icons::{self, icon};
use crate::motion::{self, AnimationExt as _};
use crate::theme::Theme;

// ---------------------------------------------------------------------------
// `/goal` input
// ---------------------------------------------------------------------------

/// What a typed `/goal …` line asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GoalInput {
    /// Bare `/goal`: show and focus the tray.
    Show,
    Set {
        objective: String,
        replace: bool,
    },
    Pause,
    Resume,
    Clear,
}

/// Parse a composer line. `None` = not a goal command (an ordinary message);
/// `Some(Err)` = a goal command that cannot be sent, with the reason to show.
///
/// Only a line that *starts* with `/goal` counts — leading indentation is how
/// the composer lets people send a slash-prefixed line literally. The control
/// words (`pause`, `resume`, `clear`) must be the whole argument, so
/// `/goal pause the deploy script` is an objective, not a pause.
pub(crate) fn parse_goal_input(text: &str) -> Option<Result<GoalInput, String>> {
    let rest = text.strip_prefix("/goal")?;
    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
        return None; // `/goals`, `/goalpost`
    }
    let arg = rest.trim();
    let lower = arg.to_lowercase();
    Some(match lower.as_str() {
        "" => Ok(GoalInput::Show),
        "pause" => Ok(GoalInput::Pause),
        "resume" => Ok(GoalInput::Resume),
        "clear" => Ok(GoalInput::Clear),
        "replace" => Err("Add the new objective after `replace`.".into()),
        _ => {
            let (objective, replace) = match lower
                .strip_prefix("replace")
                .filter(|r| r.starts_with(char::is_whitespace))
            {
                Some(_) => (arg["replace".len()..].trim(), true),
                None => (arg, false),
            };
            let chars = objective.chars().count();
            if chars > GOAL_OBJECTIVE_MAX_CHARS {
                Err(format!(
                    "The objective is {chars} characters; the limit is {GOAL_OBJECTIVE_MAX_CHARS}."
                ))
            } else {
                Ok(GoalInput::Set {
                    objective: objective.to_owned(),
                    replace,
                })
            }
        }
    })
}

/// `QueueCommand` params for a goal mutation.
pub(crate) fn goal_params(chat_id: &str, command: &GoalCommand) -> serde_json::Value {
    serde_json::json!({
        "chatId": chat_id,
        "command": { "kind": "goal", "command": command },
    })
}

impl GoalInput {
    pub(crate) fn command(&self) -> Option<GoalCommand> {
        Some(match self {
            GoalInput::Show => return None,
            GoalInput::Set { objective, replace } => GoalCommand::Set {
                objective: objective.clone(),
                limits: GoalLimits::default(),
                replace: *replace,
            },
            GoalInput::Pause => GoalCommand::Pause,
            GoalInput::Resume => GoalCommand::Resume,
            GoalInput::Clear => GoalCommand::Clear,
        })
    }
}

// ---------------------------------------------------------------------------
// Presentation model
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tone {
    Accent,
    Success,
    Warning,
    Muted,
}

/// The status chip: label and tone.
pub(crate) fn chip(goal: &Goal) -> (&'static str, Tone) {
    match goal.status {
        GoalStatus::Active => ("Active", Tone::Accent),
        GoalStatus::Verifying => ("Verifying", Tone::Accent),
        GoalStatus::Paused => ("Paused", Tone::Muted),
        GoalStatus::Complete => ("Complete", Tone::Success),
        GoalStatus::BudgetLimited => ("Budget reached", Tone::Warning),
    }
}

/// Seconds of goal activity, including the phase in progress.
pub(crate) fn elapsed_seconds(goal: &Goal, now_ms: i64) -> u64 {
    let running = match (&goal.pending, goal.status.is_running()) {
        (Some(p), true) => (now_ms.saturating_sub(p.started_at).max(0) / 1000) as u64,
        _ => 0,
    };
    goal.time_used_seconds.saturating_add(running)
}

pub(crate) fn format_tokens(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => format!("{:.1}k", n as f64 / 1_000.0).replace(".0k", "k"),
        _ => format!("{:.1}M", n as f64 / 1_000_000.0).replace(".0M", "M"),
    }
}

/// `Round 3 of 25 · 12.4k tokens (verifier 1.1k) · 4m 12s`, with the caps that
/// are set.
pub(crate) fn meta_line(goal: &Goal, now_ms: i64) -> String {
    let mut parts = vec![format!(
        "Round {} of {}",
        goal.iteration.max(1),
        goal.effective_max_rounds()
    )];
    let tokens = match goal.effective_token_budget() {
        Some(cap) => format!(
            "{} of {} tokens",
            format_tokens(goal.total_tokens()),
            format_tokens(cap)
        ),
        None => format!("{} tokens", format_tokens(goal.total_tokens())),
    };
    parts.push(if goal.verifier_tokens_used > 0 {
        format!(
            "{tokens} (verifier {})",
            format_tokens(goal.verifier_tokens_used)
        )
    } else {
        tokens
    });
    let elapsed = crate::transcript::format_elapsed(elapsed_seconds(goal, now_ms) as i64);
    parts.push(match goal.effective_time_budget_seconds() {
        Some(cap) => format!(
            "{elapsed} of {}",
            crate::transcript::format_elapsed(cap as i64)
        ),
        None => elapsed,
    });
    parts.join(" · ")
}

/// Where the collapsed header's single line of text comes from.
pub(crate) fn headline(goal: &Goal) -> (String, Tone) {
    match (&goal.status, &goal.reason) {
        (GoalStatus::Complete, _) => ("Verified".into(), Tone::Success),
        (GoalStatus::Paused, Some(r)) => (r.message.clone(), Tone::Muted),
        (GoalStatus::BudgetLimited, Some(r)) => (r.message.clone(), Tone::Warning),
        _ => (
            format!(
                "Round {}: {}",
                goal.iteration.max(1),
                goal.round_title(goal.iteration.max(1))
            ),
            Tone::Muted,
        ),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RoundState {
    Passed,
    NotSatisfied,
    VerifierFailed,
    /// The agent is working on it.
    Working,
    Verifying,
    /// Stopped before a verdict (pause, limit, interrupt).
    Stopped,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RoundView {
    pub number: u32,
    pub title: String,
    pub state: RoundState,
    pub reason: Option<String>,
    pub verifier_chat_id: Option<String>,
    /// Todo items that first appeared during this round, with their latest
    /// known status.
    pub todos: Vec<(String, TodoStatus)>,
}

/// The goal's rounds, oldest first. A round starts at the user entry carrying
/// its [`MessageOrigin::Goal`]; the todo items it lists are those whose text
/// had not been seen in any earlier list.
pub(crate) fn rounds(goal: &Goal, entries: &[SessionMessageEntry]) -> Vec<RoundView> {
    let mut starts: HashMap<u32, usize> = HashMap::new();
    for (ix, entry) in entries.iter().enumerate() {
        if let Some(MessageOrigin::Goal { goal_id, round, .. }) = &entry.origin
            && *goal_id == goal.id
        {
            starts.entry(*round).or_insert(ix);
        }
    }
    let mut seen: HashSet<String> = HashSet::new();
    let mut latest: HashMap<String, TodoStatus> = HashMap::new();
    let mut first_in_round: HashMap<u32, Vec<String>> = HashMap::new();
    let mut current: u32 = 0;
    let boundary: HashMap<usize, u32> = starts.iter().map(|(r, ix)| (*ix, *r)).collect();
    for (ix, entry) in entries.iter().enumerate() {
        if let Some(round) = boundary.get(&ix) {
            current = *round;
        }
        if entry.role != MessageRole::Assistant {
            continue;
        }
        for part in &entry.parts {
            if let MessagePart::Tool {
                call: ToolCall::Todo { items },
                ..
            } = part
            {
                for item in items {
                    latest.insert(item.text.clone(), item.status());
                    if seen.insert(item.text.clone()) && current > 0 {
                        first_in_round
                            .entry(current)
                            .or_default()
                            .push(item.text.clone());
                    }
                }
            }
        }
    }
    (1..=goal.iteration)
        .map(|number| {
            let verdict = goal.verdicts.iter().rev().find(|v| v.iteration == number);
            let state = match verdict.map(|v| v.outcome) {
                Some(VerdictOutcome::Pass) => RoundState::Passed,
                Some(VerdictOutcome::NotSatisfied) => RoundState::NotSatisfied,
                Some(VerdictOutcome::Failed) => RoundState::VerifierFailed,
                None if number == goal.iteration => match goal.status {
                    GoalStatus::Active => RoundState::Working,
                    GoalStatus::Verifying => RoundState::Verifying,
                    _ => RoundState::Stopped,
                },
                None => RoundState::Stopped,
            };
            RoundView {
                number,
                title: goal.round_title(number),
                state,
                reason: verdict.map(|v| v.reason.clone()).filter(|r| !r.is_empty()),
                verifier_chat_id: verdict.and_then(|v| v.verifier_chat_id.clone()),
                todos: first_in_round
                    .get(&number)
                    .map(|texts| {
                        texts
                            .iter()
                            .map(|t| {
                                (
                                    t.clone(),
                                    latest.get(t).copied().unwrap_or(TodoStatus::Pending),
                                )
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
            }
        })
        .collect()
}

/// A transcript marker: either a round's prompt or a lifecycle event.
#[derive(Debug, Clone, PartialEq)]
pub struct GoalMarker {
    pub kind: MarkerKind,
    pub round: u32,
    pub title: String,
    pub detail: String,
    pub verifier_chat_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkerKind {
    /// A round's prompt, sent by the controller.
    Round,
    Event(GoalEventKind),
}

/// The marker an entry stands for, when it is goal machinery rather than a
/// message: a controller-sent round prompt (a user entry) or a lifecycle event
/// (a system entry).
pub(crate) fn goal_marker(entry: &SessionMessageEntry) -> Option<GoalMarker> {
    match entry.origin.as_ref()? {
        MessageOrigin::Goal { round, title, .. } if entry.role == MessageRole::User => {
            Some(GoalMarker {
                kind: MarkerKind::Round,
                round: *round,
                title: title.clone(),
                detail: String::new(),
                verifier_chat_id: None,
            })
        }
        MessageOrigin::GoalEvent {
            event,
            round,
            title,
            detail,
            verifier_chat_id,
            ..
        } => Some(GoalMarker {
            kind: MarkerKind::Event(*event),
            round: *round,
            title: title.clone(),
            detail: detail.clone(),
            verifier_chat_id: verifier_chat_id.clone(),
        }),
        _ => None,
    }
}

impl GoalMarker {
    /// The one-line label: `Goal · round 3: Add the missing test`.
    pub(crate) fn label(&self) -> String {
        use GoalEventKind::*;
        match self.kind {
            MarkerKind::Round => format!("Goal · round {}: {}", self.round, one_line(&self.title)),
            MarkerKind::Event(Set) => format!("Goal set · {}", one_line(&self.title)),
            MarkerKind::Event(Resumed) => "Goal resumed".into(),
            MarkerKind::Event(Paused) => "Goal paused".into(),
            MarkerKind::Event(Cleared) => "Goal cleared".into(),
            MarkerKind::Event(NotSatisfied) => {
                format!("Verifier · round {} not satisfied", self.round)
            }
            MarkerKind::Event(Complete) => format!("Goal complete · {} rounds", self.round),
            MarkerKind::Event(BudgetLimited) => "Goal stopped at its limit".into(),
            MarkerKind::Event(VerifierFailed) => {
                format!("Verifier · round {} failed", self.round)
            }
        }
    }

    pub(crate) fn tone(&self) -> Tone {
        use GoalEventKind::*;
        match self.kind {
            MarkerKind::Event(Complete) => Tone::Success,
            MarkerKind::Event(BudgetLimited | VerifierFailed) => Tone::Warning,
            MarkerKind::Round | MarkerKind::Event(Set | Resumed) => Tone::Accent,
            _ => Tone::Muted,
        }
    }

    /// The second line (verdict reason, pause message, next action).
    pub(crate) fn detail_line(&self) -> Option<String> {
        let detail = one_line(&self.detail);
        let text = match self.kind {
            MarkerKind::Event(GoalEventKind::NotSatisfied) if !self.title.is_empty() => {
                if detail.is_empty() {
                    format!("Next: {}", one_line(&self.title))
                } else {
                    format!("{detail} — next: {}", one_line(&self.title))
                }
            }
            _ => detail,
        };
        (!text.is_empty()).then_some(text)
    }
}

fn one_line(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    zeron_proto::truncate_chars(&flat, 220)
}

/// Per-chat presentation state. In memory only, like the todo tray's.
#[derive(Debug, Default, Clone)]
pub(crate) struct GoalPanelState {
    pub expanded: bool,
    /// Rounds whose detail block is open.
    pub open_rounds: HashSet<u32>,
    /// Bumped on every toggle so the body's fade-in replays.
    pub epoch: u32,
    /// What the list was last scrolled to the end for (rounds, verdicts,
    /// status); it follows the newest round when it opens and whenever a
    /// round or verdict lands.
    pub followed: Option<(usize, usize, GoalStatus)>,
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

const HEADER_HEIGHT: f32 = 32.0;
const ROW_PAD_X: f32 = 8.0;
const ROW_RADIUS: f32 = 8.0;
const TEXT_SIZE: f32 = 12.5;
const TEXT_LINE: f32 = 17.0;
const GLYPH_SLOT: f32 = 14.0;
const ACTION_SIZE: f32 = 24.0;
const SIDE_INSET: f32 = 16.0;
/// Space kept clear under the expanded list. Every tray tucks
/// `QUEUE_COMPOSER_OVERLAP` behind whatever follows it (the todo or queue tray,
/// or the composer); without this clearance the last round row would sit under
/// that edge.
pub(crate) const BODY_BOTTOM_CLEARANCE: f32 = 6.0 + QUEUE_COMPOSER_OVERLAP;

/// Share of the window height the expanded goal list may take. The todo and
/// queue trays each cap at 0.3; the goal tray gives way as trays stack below
/// it, so a short window keeps room for the transcript and every list scrolls
/// inside its own cap instead of pushing another tray out of view.
pub(crate) fn body_height_fraction(trays_below: usize) -> f32 {
    match trays_below {
        0 => 0.32,
        1 => 0.22,
        _ => 0.18,
    }
}

fn tone_color(tone: Tone, theme: &Theme) -> gpui::Hsla {
    match tone {
        Tone::Accent => theme.accent,
        Tone::Success => theme.success,
        Tone::Warning => theme.warning,
        Tone::Muted => theme.text_muted,
    }
}

impl Composer {
    /// The goal tray, or `None` when the selected chat has no goal.
    /// `below`: how many trays (todo, queue) stack beneath this one.
    pub(crate) fn render_goal_panel(
        &mut self,
        below: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let (chat_id, goal, entries_len) = {
            let state = self.state.read(cx);
            let chat_id = state.selected_chat.clone()?;
            (chat_id, state.goal.clone()?, state.transcript.len())
        };
        let _ = entries_len;
        let running = goal.status.is_running();
        if running {
            self.ensure_goal_ticker(cx);
        }
        let now_ms = chrono::Utc::now().timestamp_millis();
        let panel = self.goal_panels.entry(chat_id.clone()).or_default().clone();
        let theme = Theme::of(cx).clone();
        let live = self.run_live(cx);
        let view = cx.entity_id();

        let header = self.goal_header(&chat_id, &goal, &panel, now_ms, &theme, cx);
        let surface = crate::queue::queue_panel_surface(&theme).child(header);
        let surface = if panel.expanded {
            let round_views = {
                let state = self.state.read(cx);
                rounds(&goal, &state.transcript)
            };
            let signature = (round_views.len(), goal.verdicts.len(), goal.status);
            if panel.followed != Some(signature) {
                self.goal_scroll.scroll_to_bottom();
                if let Some(state) = self.goal_panels.get_mut(&chat_id) {
                    state.followed = Some(signature);
                }
            }
            let body = self.goal_body(
                &chat_id,
                &goal,
                &panel,
                &round_views,
                now_ms,
                live,
                &theme,
                view,
                cx,
            );
            let rows = crate::edge_fade::edge_faded(
                Theme::TRANSCRIPT_FADE_BAND,
                true,
                true,
                div()
                    .id("goal-panel-body")
                    .max_h(window.viewport_size().height * body_height_fraction(below))
                    .overflow_y_scroll()
                    .track_scroll(&self.goal_scroll)
                    .child(body),
            )
            .fade_overflow_y(&self.goal_scroll)
            .outset_bottom(TEXT_SIZE);
            surface.child(div().mt(px(2.0)).pb(px(6.0)).child(rows).with_animation(
                SharedString::from(format!("goal-body-{}", panel.epoch)),
                motion::FADE_QUICK.animation(),
                |el, t| el.opacity(t),
            ))
        } else {
            surface
        };
        let inset = SIDE_INSET * (1 + below) as f32;
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

    /// One repaint a second while a goal runs, so the elapsed ticker moves.
    /// Skipped under reduced motion (which also covers a backgrounded window
    /// when "pause animations in background" is on): the time then refreshes
    /// whenever something else repaints. Parks itself when the goal stops.
    fn ensure_goal_ticker(&mut self, cx: &mut Context<Self>) {
        if self.goal_ticker.is_some() {
            return;
        }
        self.goal_ticker = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let keep = this
                    .update(cx, |composer, cx| {
                        let running = composer
                            .state
                            .read(cx)
                            .goal
                            .as_ref()
                            .is_some_and(|g| g.status.is_running());
                        if !running {
                            composer.goal_ticker = None;
                            return false;
                        }
                        if !cx.reduce_motion() {
                            cx.notify();
                        }
                        true
                    })
                    .unwrap_or(false);
                if !keep {
                    break;
                }
            }
        }));
    }

    #[allow(clippy::too_many_arguments)]
    fn goal_header(
        &self,
        chat_id: &str,
        goal: &Goal,
        panel: &GoalPanelState,
        now_ms: i64,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let accent = theme.accent;
        let label = if panel.expanded {
            "Collapse goal"
        } else {
            "Expand goal"
        };
        let (chip_label, chip_tone) = chip(goal);
        let chip_color = tone_color(chip_tone, theme);
        let (headline_text, headline_tone) = headline(goal);
        let toggle_chat = chat_id.to_owned();
        let elapsed = crate::transcript::format_elapsed(elapsed_seconds(goal, now_ms) as i64);
        let toggle = div()
            .id("goal-panel-toggle")
            .role(gpui::Role::Button)
            .aria_label(label)
            .flex_1()
            .min_w_0()
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
                let panel = this.goal_panels.entry(toggle_chat.clone()).or_default();
                panel.expanded = !panel.expanded;
                panel.epoch = panel.epoch.wrapping_add(1);
                panel.followed = None;
                cx.notify();
            }))
            .tooltip(crate::settings::widgets::text_tooltip(label))
            .tooltip_show_delay(Duration::from_millis(350))
            .child(
                icon(icons::GOAL)
                    .size(px(14.0))
                    .text_color(tone_color(chip_tone, theme)),
            )
            .child(
                div()
                    .flex_none()
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_size(px(TEXT_SIZE))
                    .text_color(theme.text)
                    .child("Goal"),
            )
            .child(
                div()
                    .flex_none()
                    .px(px(6.0))
                    .h(px(16.0))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .bg(chip_color.opacity(0.14))
                    .text_size(px(10.5))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(chip_color)
                    .child(chip_label),
            )
            .when(!panel.expanded, |el| {
                el.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(px(TEXT_SIZE))
                        .text_color(tone_color(headline_tone, theme))
                        .child(SharedString::from(headline_text)),
                )
            })
            .when(panel.expanded, |el| el.child(div().flex_1()))
            .child(
                div()
                    .flex_none()
                    .text_size(px(11.5))
                    .text_color(theme.text_faint)
                    .child(SharedString::from(format!(
                        "R{} · {elapsed}",
                        goal.iteration.max(1)
                    ))),
            )
            .child(
                icon(if panel.expanded {
                    icons::ALT_ARROW_DOWN
                } else {
                    icons::ALT_ARROW_UP
                })
                .size(px(13.0))
                .text_color(theme.text_muted.opacity(0.7)),
            );

        let mut row = div().flex().items_center().gap(px(2.0)).child(toggle);
        match goal.status {
            GoalStatus::Active | GoalStatus::Verifying => {
                row = row.child(self.goal_action(
                    "goal-pause",
                    icons::PAUSE,
                    "Pause goal",
                    chat_id,
                    GoalCommand::Pause,
                    theme,
                    cx,
                ));
            }
            GoalStatus::Paused | GoalStatus::BudgetLimited => {
                row = row.child(self.goal_action(
                    "goal-resume",
                    icons::ACTION_PLAY,
                    "Resume goal",
                    chat_id,
                    GoalCommand::Resume,
                    theme,
                    cx,
                ));
            }
            GoalStatus::Complete => {}
        }
        row = row.child(self.goal_action(
            "goal-clear",
            if goal.status == GoalStatus::Complete {
                icons::CLOSE
            } else {
                icons::TRASH_BIN_MINIMALISTIC
            },
            if goal.status == GoalStatus::Complete {
                "Dismiss goal"
            } else {
                "Clear goal"
            },
            chat_id,
            GoalCommand::Clear,
            theme,
            cx,
        ));
        row.into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn goal_action(
        &self,
        id: &'static str,
        glyph: &'static str,
        tooltip: &'static str,
        chat_id: &str,
        command: GoalCommand,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let accent = theme.accent;
        let chat_id = chat_id.to_owned();
        div()
            .id(id)
            .role(gpui::Role::Button)
            .aria_label(tooltip)
            .size(px(ACTION_SIZE))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(5.0))
            .cursor_pointer()
            .hover(|s| s.bg(theme.element_hover))
            .tab_index(0)
            .focus_visible(move |s| s.bg(accent.opacity(0.18)))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.send_goal_command(chat_id.clone(), command.clone(), cx);
            }))
            .tooltip(crate::settings::widgets::text_tooltip(tooltip))
            .tooltip_show_delay(Duration::from_millis(350))
            .child(
                icon(glyph)
                    .size(px(12.0))
                    .text_color(theme.text_muted.opacity(0.85)),
            )
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn goal_body(
        &self,
        chat_id: &str,
        goal: &Goal,
        panel: &GoalPanelState,
        round_views: &[RoundView],
        now_ms: i64,
        live: bool,
        theme: &Theme,
        view: gpui::EntityId,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let reason = goal.reason.as_ref().map(|r| {
            let tone = match goal.status {
                GoalStatus::Complete => Tone::Success,
                GoalStatus::BudgetLimited => Tone::Warning,
                _ => Tone::Muted,
            };
            (r.message.clone(), tone)
        });
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .px(px(ROW_PAD_X))
                    .py(px(4.0))
                    .text_size(px(TEXT_SIZE))
                    .line_height(px(TEXT_LINE))
                    .text_color(theme.text)
                    .child(SharedString::from(goal.objective.clone())),
            )
            .child(
                div()
                    .px(px(ROW_PAD_X))
                    .pb(px(4.0))
                    .text_size(px(11.5))
                    .text_color(theme.text_faint)
                    .child(SharedString::from(meta_line(goal, now_ms))),
            )
            .when_some(reason, |el, (message, tone)| {
                el.child(
                    div()
                        .px(px(ROW_PAD_X))
                        .pb(px(6.0))
                        .text_size(px(11.5))
                        .line_height(px(16.0))
                        .text_color(tone_color(tone, theme))
                        .child(SharedString::from(message)),
                )
            })
            .children(round_views.iter().map(|round| {
                self.goal_round_row(chat_id, goal, panel, round, live, theme, view, cx)
            }))
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn goal_round_row(
        &self,
        chat_id: &str,
        goal: &Goal,
        panel: &GoalPanelState,
        round: &RoundView,
        live: bool,
        theme: &Theme,
        view: gpui::EntityId,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let accent = theme.accent;
        let open = panel.open_rounds.contains(&round.number);
        let has_detail =
            round.reason.is_some() || !round.todos.is_empty() || round.verifier_chat_id.is_some();
        let number = round.number;
        let toggle_chat = chat_id.to_owned();
        let outcome_label = match round.state {
            RoundState::Passed => "passed",
            RoundState::NotSatisfied => "not satisfied",
            RoundState::VerifierFailed => "verifier failed",
            RoundState::Working => "working",
            RoundState::Verifying => "verifying",
            RoundState::Stopped => "stopped",
        };
        let glyph: AnyElement = match round.state {
            RoundState::Passed => icon(icons::CHECK)
                .size(px(12.0))
                .text_color(theme.success)
                .into_any_element(),
            RoundState::NotSatisfied => icon(icons::ARROW_RIGHT)
                .size(px(12.0))
                .text_color(theme.text_faint)
                .into_any_element(),
            RoundState::VerifierFailed => icon(icons::DANGER_TRIANGLE)
                .size(px(12.0))
                .text_color(theme.warning)
                .into_any_element(),
            RoundState::Working | RoundState::Verifying if live || goal.status.is_running() => {
                crate::loaders::mini_glyph_spinner(
                    format!("goal-round-{number}"),
                    2.5,
                    theme.glyph,
                    view,
                    cx,
                )
                .into_any_element()
            }
            _ => div()
                .size(px(10.0))
                .rounded_full()
                .border_1()
                .border_color(theme.text_faint.opacity(0.7))
                .into_any_element(),
        };
        let header = div()
            .id(SharedString::from(format!("goal-round-{number}")))
            .role(gpui::Role::Button)
            .aria_label(format!("Round {number}: {}", round.title))
            .min_h(px(26.0))
            .px(px(ROW_PAD_X))
            .py(px(4.0))
            .rounded(px(6.0))
            .flex()
            .items_start()
            .gap(px(8.0))
            .when(has_detail, |el| {
                el.cursor_pointer()
                    .hover(|s| s.bg(theme.element_hover))
                    .tab_index(0)
                    .focus_visible(move |s| s.bg(accent.opacity(0.14)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let panel = this.goal_panels.entry(toggle_chat.clone()).or_default();
                        if !panel.open_rounds.insert(number) {
                            panel.open_rounds.remove(&number);
                        }
                        cx.notify();
                    }))
            })
            .child(
                div()
                    .w(px(GLYPH_SLOT))
                    .h(px(TEXT_LINE))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(glyph),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(TEXT_SIZE))
                    .line_height(px(TEXT_LINE))
                    .text_color(match round.state {
                        RoundState::Working | RoundState::Verifying => theme.text,
                        _ => theme.text_muted,
                    })
                    .child(SharedString::from(format!("{number}. {}", round.title))),
            )
            .child(
                div()
                    .flex_none()
                    .h(px(TEXT_LINE))
                    .flex()
                    .items_center()
                    .text_size(px(11.0))
                    .text_color(theme.text_faint)
                    .child(outcome_label),
            );
        let detail = (open && has_detail).then(|| {
            div()
                .pl(px(ROW_PAD_X + GLYPH_SLOT + 8.0))
                .pr(px(ROW_PAD_X))
                .pb(px(6.0))
                .flex()
                .flex_col()
                .gap(px(3.0))
                .when_some(round.reason.clone(), |el, reason| {
                    el.child(
                        div()
                            .text_size(px(11.5))
                            .line_height(px(16.0))
                            .text_color(theme.text_muted)
                            .child(SharedString::from(reason)),
                    )
                })
                .children(round.todos.iter().enumerate().map(|(ix, (text, status))| {
                    div()
                        .flex()
                        .items_start()
                        .gap(px(6.0))
                        .child(
                            div()
                                .w(px(12.0))
                                .h(px(16.0))
                                .flex_none()
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(todo_dot(*status, theme)),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("goal-todo-{number}-{ix}")))
                                .flex_1()
                                .min_w_0()
                                .text_size(px(11.5))
                                .line_height(px(16.0))
                                .text_color(match status {
                                    TodoStatus::Completed => theme.text_faint,
                                    _ => theme.text_muted,
                                })
                                .child(SharedString::from(text.clone())),
                        )
                }))
                .when_some(round.verifier_chat_id.clone(), |el, verifier| {
                    el.child(
                        div()
                            .id(SharedString::from(format!("goal-verifier-{number}")))
                            .role(gpui::Role::Button)
                            .aria_label("Open the verifier chat")
                            .cursor_pointer()
                            .text_size(px(11.5))
                            .text_color(accent)
                            .hover(|s| s.underline())
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.emit(crate::composer::ComposerEvent::OpenChat {
                                    chat_id: verifier.clone(),
                                });
                            }))
                            .child("Open verifier chat"),
                    )
                })
        });
        div()
            .flex()
            .flex_col()
            .child(header)
            .when_some(detail, |el, detail| el.child(detail))
            .into_any_element()
    }

    /// Send a goal mutation to the chat's host through the command plane.
    pub(crate) fn send_goal_command(
        &mut self,
        chat_id: String,
        command: GoalCommand,
        cx: &mut Context<Self>,
    ) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            self.failure = Some("Engine not connected".into());
            self.failure_key = None;
            cx.notify();
            return;
        };
        if !engine
            .engine_info()
            .supports(zeron_proto::capabilities::GOAL_MODE_V1)
            || !self
                .state
                .read(cx)
                .chat_host_supports(&chat_id, zeron_proto::capabilities::GOAL_MODE_V1)
        {
            self.failure = Some("Update Zeron on the chat's device to use goals.".into());
            self.failure_key = Some(chat_id);
            cx.notify();
            return;
        }
        let params = goal_params(&chat_id, &command);
        cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(zeron_rpc::methods::QUEUE_COMMAND, params)
                .await;
            if let Err(err) = result {
                this.update(cx, |composer, cx| {
                    composer.failure = Some(format!("Goal command failed: {err}").into());
                    composer.failure_key = Some(chat_id);
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    /// Handle a typed `/goal …` line. Returns whether the line was consumed.
    pub(crate) fn run_goal_input(&mut self, text: &str, cx: &mut Context<Self>) -> bool {
        let Some(parsed) = parse_goal_input(text) else {
            return false;
        };
        let Some(chat_id) = self.state.read(cx).selected_chat.clone() else {
            self.failure = Some("Start a conversation first, then set its goal.".into());
            self.failure_key = None;
            cx.notify();
            return true;
        };
        match parsed {
            Err(message) => {
                self.failure = Some(message.into());
                self.failure_key = Some(chat_id);
            }
            Ok(GoalInput::Show) => self.show_goal_panel(&chat_id, cx),
            Ok(input) => {
                if let Some(command) = input.command() {
                    // An existing running or paused goal: `/goal <text>` is a
                    // replacement the user must ask for with `replace`.
                    let blocked = matches!(&input, GoalInput::Set { replace: false, .. })
                        && self.state.read(cx).goal.as_ref().is_some_and(|g| {
                            !matches!(g.status, GoalStatus::Complete | GoalStatus::BudgetLimited)
                        });
                    if blocked {
                        self.failure = Some(
                            "This chat already has a goal. Use /goal replace <objective>, or /goal clear first."
                                .into(),
                        );
                        self.failure_key = Some(chat_id);
                    } else {
                        self.send_goal_command(chat_id, command, cx);
                    }
                }
            }
        }
        self.input.update(cx, |input, cx| input.set_text("", cx));
        cx.notify();
        true
    }

    /// `/goal` with no argument: open the tray (or say there is no goal).
    pub(crate) fn show_goal_panel(&mut self, chat_id: &str, cx: &mut Context<Self>) {
        if self.state.read(cx).goal.is_some() {
            let panel = self.goal_panels.entry(chat_id.to_owned()).or_default();
            panel.expanded = true;
            panel.epoch = panel.epoch.wrapping_add(1);
            panel.followed = None;
        } else {
            self.failure =
                Some("This chat has no goal. Type /goal followed by an objective.".into());
            self.failure_key = Some(chat_id.to_owned());
        }
        cx.notify();
    }
}

fn todo_dot(status: TodoStatus, theme: &Theme) -> AnyElement {
    match status {
        TodoStatus::Completed => icon(icons::CHECK)
            .size(px(10.0))
            .text_color(theme.success)
            .into_any_element(),
        TodoStatus::InProgress => div()
            .size(px(8.0))
            .rounded_full()
            .border_1()
            .border_color(theme.accent)
            .into_any_element(),
        TodoStatus::Pending => div()
            .size(px(8.0))
            .rounded_full()
            .border_1()
            .border_color(theme.text_faint.opacity(0.7))
            .into_any_element(),
    }
}

/// The transcript marker element: a compact one- or two-line row.
pub(crate) fn marker_element(marker: &GoalMarker, theme: &Theme) -> AnyElement {
    let color = tone_color(marker.tone(), theme);
    let glyph = match marker.kind {
        MarkerKind::Event(GoalEventKind::Complete) => icons::CHECK,
        MarkerKind::Event(GoalEventKind::Paused) => icons::PAUSE,
        MarkerKind::Event(GoalEventKind::BudgetLimited | GoalEventKind::VerifierFailed) => {
            icons::DANGER_TRIANGLE
        }
        MarkerKind::Event(GoalEventKind::NotSatisfied) => icons::ARROW_RIGHT,
        _ => icons::GOAL,
    };
    let label = SharedString::from(marker.label());
    div()
        .py(px(8.0))
        .w_full()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(2.0))
        .child(
            div()
                .w_full()
                .min_w_0()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(div().flex_1().min_w_0().h(px(1.0)).bg(theme.border))
                .child(icon(glyph).size(px(12.0)).text_color(color))
                .child(
                    div()
                        .flex_none()
                        .max_w(px(560.0))
                        .truncate()
                        .text_size(crate::typography::ui_rems(12.0))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(color)
                        .child(label),
                )
                .child(div().flex_1().min_w_0().h(px(1.0)).bg(theme.border)),
        )
        .when_some(marker.detail_line(), |el, detail| {
            el.child(
                div()
                    .w_full()
                    .min_w_0()
                    .px(px(24.0))
                    .text_center()
                    .text_size(crate::typography::ui_rems(11.5))
                    .line_height(px(16.0))
                    .text_color(theme.text_faint)
                    .child(SharedString::from(detail)),
            )
        })
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeron_proto::{GoalReasonKind, GoalVerdict, TodoItem};

    fn goal() -> Goal {
        Goal::new("g1", "Ship the thing", &GoalLimits::default(), 0).unwrap()
    }

    #[test]
    fn stacked_trays_split_the_height_and_clear_the_overlap() {
        // The goal list never takes more than the todo/queue cap, shrinks as
        // trays stack under it, and the three together stay under 4/5 of the
        // window (each list scrolls inside its own cap).
        assert!(body_height_fraction(0) <= 0.35);
        assert!(body_height_fraction(1) < body_height_fraction(0));
        assert!(body_height_fraction(2) < body_height_fraction(1));
        assert!(body_height_fraction(2) + 0.3 + 0.3 < 0.8);
        // The last row clears the edge the next tray tucks over.
        assert!(BODY_BOTTOM_CLEARANCE > QUEUE_COMPOSER_OVERLAP);
    }

    #[test]
    fn goal_lines_parse() {
        assert_eq!(parse_goal_input("/goal"), Some(Ok(GoalInput::Show)));
        assert_eq!(parse_goal_input("/goal  "), Some(Ok(GoalInput::Show)));
        assert_eq!(parse_goal_input("/goal pause"), Some(Ok(GoalInput::Pause)));
        assert_eq!(
            parse_goal_input("/goal Resume"),
            Some(Ok(GoalInput::Resume))
        );
        assert_eq!(parse_goal_input("/goal clear"), Some(Ok(GoalInput::Clear)));
        assert_eq!(
            parse_goal_input("/goal fix the build"),
            Some(Ok(GoalInput::Set {
                objective: "fix the build".into(),
                replace: false
            }))
        );
        assert_eq!(
            parse_goal_input("/goal replace ship it\nsecond line"),
            Some(Ok(GoalInput::Set {
                objective: "ship it\nsecond line".into(),
                replace: true
            }))
        );
    }

    #[test]
    fn control_words_must_be_the_whole_argument() {
        // An objective that merely starts with a control word is an objective.
        assert_eq!(
            parse_goal_input("/goal pause the deploy script"),
            Some(Ok(GoalInput::Set {
                objective: "pause the deploy script".into(),
                replace: false
            }))
        );
        assert_eq!(
            parse_goal_input("/goal replacement strategy"),
            Some(Ok(GoalInput::Set {
                objective: "replacement strategy".into(),
                replace: false
            }))
        );
        assert!(matches!(parse_goal_input("/goal replace"), Some(Err(_))));
    }

    #[test]
    fn only_a_leading_goal_command_counts() {
        assert_eq!(parse_goal_input("hello /goal x"), None);
        assert_eq!(
            parse_goal_input(" /goal x"),
            None,
            "indentation sends literally"
        );
        assert_eq!(parse_goal_input("/goals"), None);
        assert_eq!(parse_goal_input("/goalpost"), None);
        assert_eq!(parse_goal_input("/compact"), None);
    }

    #[test]
    fn oversized_objectives_are_refused_before_sending() {
        let long = format!("/goal {}", "x".repeat(GOAL_OBJECTIVE_MAX_CHARS + 1));
        assert!(matches!(parse_goal_input(&long), Some(Err(m)) if m.contains("limit")));
        let ok = format!("/goal {}", "é".repeat(GOAL_OBJECTIVE_MAX_CHARS));
        assert!(matches!(parse_goal_input(&ok), Some(Ok(_))));
    }

    #[test]
    fn commands_and_params_have_the_wire_shape_the_engine_reads() {
        let input = parse_goal_input("/goal replace x").unwrap().unwrap();
        let params = goal_params("chat", &input.command().unwrap());
        assert_eq!(params["chatId"], "chat");
        assert_eq!(params["command"]["kind"], "goal");
        assert_eq!(params["command"]["command"]["action"], "set");
        assert_eq!(params["command"]["command"]["replace"], true);
        assert!(GoalInput::Show.command().is_none());
    }

    #[test]
    fn status_chips_and_headlines() {
        let mut g = goal();
        assert_eq!(chip(&g), ("Active", Tone::Accent));
        g.iteration = 2;
        g.push_verdict(GoalVerdict {
            iteration: 1,
            outcome: VerdictOutcome::NotSatisfied,
            reason: "no".into(),
            next_action: Some("Add the test".into()),
            at: 1,
            verifier_chat_id: None,
            verifier_tokens: None,
            verifier_ms: None,
        });
        assert_eq!(headline(&g).0, "Round 2: Add the test");
        g.stop(
            GoalStatus::BudgetLimited,
            GoalReasonKind::MaxRounds,
            "Reached the round limit",
        );
        assert_eq!(chip(&g), ("Budget reached", Tone::Warning));
        assert_eq!(
            headline(&g),
            ("Reached the round limit".into(), Tone::Warning)
        );
        g.status = GoalStatus::Complete;
        assert_eq!(chip(&g).0, "Complete");
        assert_eq!(headline(&g).0, "Verified");
    }

    #[test]
    fn elapsed_adds_the_running_phase_only_while_running() {
        let mut g = goal();
        g.time_used_seconds = 60;
        g.pending = Some(zeron_proto::GoalPending {
            kind: zeron_proto::GoalPendingKind::Turn,
            round: 1,
            message_id: "m".into(),
            started_at: 10_000,
        });
        assert_eq!(elapsed_seconds(&g, 25_000), 75);
        g.status = GoalStatus::Paused;
        assert_eq!(elapsed_seconds(&g, 25_000), 60);
    }

    #[test]
    fn token_and_meta_formatting() {
        assert_eq!(format_tokens(999), "999");
        assert_eq!(format_tokens(12_400), "12.4k");
        assert_eq!(format_tokens(50_000), "50k");
        assert_eq!(format_tokens(2_100_000), "2.1M");
        let mut g = goal();
        g.iteration = 3;
        g.tokens_used = 12_000;
        g.verifier_tokens_used = 1_000;
        g.token_budget = Some(50_000);
        g.time_used_seconds = 252;
        assert_eq!(
            meta_line(&g, 0),
            "Round 3 of 25 · 13k of 50k tokens (verifier 1k) · 4m 12s"
        );
    }

    fn user_round(goal: &Goal, round: u32) -> SessionMessageEntry {
        SessionMessageEntry {
            origin: Some(MessageOrigin::Goal {
                goal_id: goal.id.clone(),
                round,
                title: format!("round {round}"),
            }),
            id: goal.round_message_id(round),
            role: MessageRole::User,
            parts: vec![MessagePart::Text {
                id: "t".into(),
                text: "go".into(),
            }],
            created_at: 0,
            device_id: "d".into(),
            status: None,
            continuation_of: None,
            duration_ms: None,
        }
    }

    fn assistant_todo(id: &str, list: Vec<TodoItem>) -> SessionMessageEntry {
        SessionMessageEntry {
            origin: None,
            id: id.into(),
            role: MessageRole::Assistant,
            parts: vec![MessagePart::Tool {
                id: id.into(),
                call: ToolCall::Todo { items: list },
                is_error: false,
                resolved: true,
                output: None,
                diff: None,
                output_ref: None,
                output_bytes: None,
                diff_ref: None,
                diff_stats: None,
                subagent_ref: None,
                subagent_status: None,
                subagent_tail: None,
            }],
            created_at: 0,
            device_id: "d".into(),
            status: Some(zeron_doc::MessageStatus::Complete),
            continuation_of: None,
            duration_ms: None,
        }
    }

    #[test]
    fn rounds_list_the_todos_that_first_appeared_in_each() {
        let mut g = goal();
        g.iteration = 2;
        g.push_verdict(GoalVerdict {
            iteration: 1,
            outcome: VerdictOutcome::NotSatisfied,
            reason: "tests missing".into(),
            next_action: Some("Write tests".into()),
            at: 1,
            verifier_chat_id: Some("v1".into()),
            verifier_tokens: None,
            verifier_ms: None,
        });
        let entries = vec![
            user_round(&g, 1),
            assistant_todo(
                "a1",
                vec![
                    TodoItem::new("Implement", TodoStatus::InProgress),
                    TodoItem::new("Document", TodoStatus::Pending),
                ],
            ),
            user_round(&g, 2),
            assistant_todo(
                "a2",
                vec![
                    TodoItem::new("Implement", TodoStatus::Completed),
                    TodoItem::new("Document", TodoStatus::Completed),
                    TodoItem::new("Write tests", TodoStatus::InProgress),
                ],
            ),
        ];
        let views = rounds(&g, &entries);
        assert_eq!(views.len(), 2);
        assert_eq!(views[0].title, "Ship the thing");
        assert_eq!(views[0].state, RoundState::NotSatisfied);
        assert_eq!(views[0].reason.as_deref(), Some("tests missing"));
        assert_eq!(views[0].verifier_chat_id.as_deref(), Some("v1"));
        // Round 1's items carry their LATEST status; round 2 only its new one.
        assert_eq!(
            views[0].todos,
            vec![
                ("Implement".to_string(), TodoStatus::Completed),
                ("Document".to_string(), TodoStatus::Completed)
            ]
        );
        assert_eq!(views[1].title, "Write tests");
        assert_eq!(views[1].state, RoundState::Working);
        assert_eq!(
            views[1].todos,
            vec![("Write tests".to_string(), TodoStatus::InProgress)]
        );
    }

    #[test]
    fn rounds_ignore_other_goals_and_missing_boundaries() {
        let mut g = goal();
        g.iteration = 1;
        let mut other = goal();
        other.id = "other".into();
        let entries = vec![
            user_round(&other, 1),
            assistant_todo("a", vec![TodoItem::new("x", TodoStatus::Pending)]),
        ];
        let views = rounds(&g, &entries);
        assert_eq!(views.len(), 1);
        assert!(
            views[0].todos.is_empty(),
            "no boundary for this goal's round"
        );
        g.status = GoalStatus::Paused;
        assert_eq!(rounds(&g, &[])[0].state, RoundState::Stopped);
        g.status = GoalStatus::Verifying;
        assert_eq!(rounds(&g, &[])[0].state, RoundState::Verifying);
    }

    #[test]
    fn markers_replace_machine_origin_entries() {
        let g = goal();
        let round = goal_marker(&user_round(&g, 3)).unwrap();
        assert_eq!(round.label(), "Goal · round 3: round 3");
        // A human message is never a marker, whatever it says.
        let mut plain = user_round(&g, 1);
        plain.origin = None;
        assert!(goal_marker(&plain).is_none());

        let event = SessionMessageEntry {
            origin: Some(MessageOrigin::GoalEvent {
                goal_id: g.id.clone(),
                event: GoalEventKind::NotSatisfied,
                round: 2,
                title: "Add the test".into(),
                detail: "tests are missing".into(),
                verifier_chat_id: Some("v".into()),
            }),
            role: MessageRole::System,
            ..user_round(&g, 1)
        };
        let marker = goal_marker(&event).unwrap();
        assert_eq!(marker.label(), "Verifier · round 2 not satisfied");
        assert_eq!(
            marker.detail_line().as_deref(),
            Some("tests are missing — next: Add the test")
        );
        assert_eq!(marker.tone(), Tone::Muted);
        let done = GoalMarker {
            kind: MarkerKind::Event(GoalEventKind::Complete),
            round: 4,
            title: String::new(),
            detail: String::new(),
            verifier_chat_id: None,
        };
        assert_eq!(done.label(), "Goal complete · 4 rounds");
        assert_eq!(done.tone(), Tone::Success);
        assert_eq!(done.detail_line(), None);
    }

    #[test]
    fn marker_text_is_flattened_and_bounded() {
        let marker = GoalMarker {
            kind: MarkerKind::Round,
            round: 1,
            title: format!("line one\n{}", "word ".repeat(100)),
            detail: String::new(),
            verifier_chat_id: None,
        };
        let label = marker.label();
        assert!(!label.contains('\n'));
        assert!(label.chars().count() < 260);
    }
}

#[cfg(test)]
mod composer_tests {
    use super::*;
    use crate::state::AppState;

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

    fn goal(status: GoalStatus) -> Goal {
        let mut goal = Goal::new("g", "Ship the thing", &GoalLimits::default(), 0).unwrap();
        goal.iteration = 1;
        goal.status = status;
        goal
    }

    fn show(state: &gpui::Entity<AppState>, goal: Option<Goal>, cx: &mut gpui::TestAppContext) {
        state.update(cx, |state, _| {
            state.selected_chat = Some("a".into());
            state.goal = goal;
        });
    }

    fn rendered(handle: &gpui::WindowHandle<Composer>, cx: &mut gpui::TestAppContext) -> bool {
        handle
            .update(cx, |composer, window, cx| {
                composer.render_goal_panel(0, window, cx).is_some()
            })
            .unwrap()
    }

    #[gpui::test]
    fn the_tray_exists_only_while_the_chat_has_a_goal(cx: &mut gpui::TestAppContext) {
        let (_dir, state, handle) = window(cx);
        show(&state, None, cx);
        assert!(!rendered(&handle, cx));
        for status in [
            GoalStatus::Active,
            GoalStatus::Verifying,
            GoalStatus::Paused,
            GoalStatus::Complete,
            GoalStatus::BudgetLimited,
        ] {
            show(&state, Some(goal(status)), cx);
            assert!(rendered(&handle, cx), "{status:?}");
        }
        show(&state, None, cx);
        assert!(!rendered(&handle, cx));
    }

    #[gpui::test]
    fn goal_lines_are_consumed_and_a_bare_goal_opens_the_tray(cx: &mut gpui::TestAppContext) {
        let (_dir, state, handle) = window(cx);
        show(&state, Some(goal(GoalStatus::Active)), cx);
        // Ordinary text is not ours.
        let consumed = handle
            .update(cx, |c, _, cx| c.run_goal_input("hello", cx))
            .unwrap();
        assert!(!consumed);
        // `/goal` shows the tray, expanded.
        let consumed = handle
            .update(cx, |c, _, cx| c.run_goal_input("/goal", cx))
            .unwrap();
        assert!(consumed);
        handle
            .read_with(cx, |c, _| assert!(c.goal_panels["a"].expanded))
            .unwrap();
        // Without a goal it says so instead of sending anything.
        show(&state, None, cx);
        handle
            .update(cx, |c, _, cx| {
                c.goal_panels.clear();
                assert!(c.run_goal_input("/goal", cx));
            })
            .unwrap();
        handle
            .read_with(cx, |c, _| {
                assert!(c.failure.as_ref().is_some_and(|f| f.contains("no goal")));
            })
            .unwrap();
    }

    #[gpui::test]
    fn replacing_an_active_goal_needs_the_replace_word(cx: &mut gpui::TestAppContext) {
        let (_dir, state, handle) = window(cx);
        show(&state, Some(goal(GoalStatus::Active)), cx);
        handle
            .update(cx, |c, _, cx| {
                assert!(c.run_goal_input("/goal something else", cx))
            })
            .unwrap();
        handle
            .read_with(cx, |c, _| {
                assert!(c.failure.as_ref().is_some_and(|f| f.contains("replace")));
            })
            .unwrap();
    }
}
