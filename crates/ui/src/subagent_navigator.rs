//! The agent rail: the strip of a session's agents, under the composer.
//!
//! `main` is the first row; every subagent the session spawned follows it,
//! folded into one group row per declared agent type once a type has more than
//! one member (a folder you must open to find a single row is pure friction).
//! ↓ from the composer enters the rail, ↑/↓ walk it, →/Enter descend into a
//! group — or into an agent, to reach the agents *it* spawned — and ←/Esc climb
//! back out. Enter on a row renders that agent's transcript in the conversation
//! column, exactly as the session's own transcript renders.
//!
//! The rail holds open every subagent doc feed it needs (the scope path plus
//! the focused agent) and nothing else, through the refcounted feeds on
//! [`AppState`] it shares with the right pane's subagent tabs — so a doc can
//! never stay pinned in the engine's LRU with nothing on screen to show for
//! it, and a tab closing never blanks the column the rail is showing. The
//! shell owns only the transcript view for whatever the rail focused.

use std::collections::HashMap;

use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement,
    KeyDownEvent, Render, SharedString, Subscription, Window, div, prelude::*, px,
};
use zeron_doc::{MessagePart, SessionMessageEntry, SubagentStatus};
use zeron_proto::ToolCall;

use crate::motion;
use crate::state::AppState;
use crate::theme::Theme;

/// Row height. The composer's picker chips are 32px; the rail is quieter chrome
/// below the pill, so its rows sit a notch tighter.
const ROW_HEIGHT: f32 = 28.0;
const ROW_GAP: f32 = 2.0;
/// Rows shown before the list scrolls. Past this the rail starts eating the
/// conversation it belongs to.
const MAX_VISIBLE_ROWS: usize = 5;
/// The scope breadcrumb, shown only while the rail is inside a group or agent.
const CRUMB_HEIGHT: f32 = 18.0;

/// Which transcript the conversation column renders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentTarget {
    /// The session itself.
    Main,
    Agent {
        doc_id: SharedString,
        title: SharedString,
        /// The subagent finished: its transcript is a frozen blob, so the view
        /// reads top-down instead of following a streaming end.
        frozen: bool,
    },
}

impl AgentTarget {
    pub fn doc_id(&self) -> Option<&SharedString> {
        match self {
            AgentTarget::Main => None,
            AgentTarget::Agent { doc_id, .. } => Some(doc_id),
        }
    }
}

/// What the shell listens for.
#[derive(Debug, Clone)]
pub enum NavigatorEvent {
    /// Render this agent's transcript in the conversation column.
    Focus(AgentTarget),
    /// Give the keyboard back to the composer input (↑ off the first row, or
    /// Esc at the root scope).
    ReturnToComposer,
    /// A printable key arrived while the rail held focus. Typing is never a
    /// rail gesture, so the character goes where it was meant to go instead of
    /// vanishing into a list that has no use for it.
    TypeIntoComposer(String),
}

// ---------------------------------------------------------------------------
// Pure model
// ---------------------------------------------------------------------------

/// One spawn chip, read off a transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSpawn {
    pub doc_id: SharedString,
    pub title: SharedString,
    /// The agent type the spawn declared (`subagent_type`) — the rail's folder
    /// name. `None` when the spawn didn't name one; those never group, because
    /// a guessed grouping is worse than a flat list.
    pub kind: Option<SharedString>,
    pub status: SubagentStatus,
}

impl AgentSpawn {
    fn frozen(&self) -> bool {
        matches!(self.status, SubagentStatus::Done | SubagentStatus::Failed)
    }

    fn target(&self) -> AgentTarget {
        AgentTarget::Agent {
            doc_id: self.doc_id.clone(),
            title: self.title.clone(),
            frozen: self.frozen(),
        }
    }
}

/// Every spawn chip in `entries`, in transcript order. A chip counts only once
/// the engine has stamped its subagent doc id — before that there is nothing to
/// open.
pub fn spawns(entries: &[SessionMessageEntry]) -> Vec<AgentSpawn> {
    let mut out = Vec::new();
    for entry in entries {
        for part in &entry.parts {
            let MessagePart::Tool {
                call,
                subagent_ref: Some(doc_id),
                subagent_status,
                ..
            } = part
            else {
                continue;
            };
            // The same genus gate the doc fold uses: a ref may only ever ride a
            // spawn call, so a driver keying bug cannot put an ordinary tool
            // chip in the rail (and open a doc that was never created).
            if !call.is_subagent_spawn() {
                continue;
            }
            out.push(AgentSpawn {
                doc_id: doc_id.as_str().into(),
                title: crate::transcript::subagent_tab_title(call),
                kind: spawn_kind(call),
                status: subagent_status.unwrap_or(SubagentStatus::Running),
            });
        }
    }
    out
}

/// The spawn's declared agent type. `sanitize_tool_call` keeps this key on the
/// doc precisely so the client can name the child without the prompt.
fn spawn_kind(call: &ToolCall) -> Option<SharedString> {
    let input = match call {
        ToolCall::Unknown { input, .. } | ToolCall::Mcp { input, .. } => input.as_ref()?,
        _ => return None,
    };
    let kind = input.get("subagent_type")?.as_str()?.trim();
    (!kind.is_empty()).then(|| kind.into())
}

/// A rendered row at the current scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    /// The session's own transcript. Only ever the first row of the root scope.
    Main,
    /// A folder over every spawn of one declared type.
    Group {
        kind: SharedString,
        count: usize,
        running: usize,
    },
    /// A spawn, by index into the scope's spawn list.
    Agent(usize),
}

/// Fold a scope's spawns into rows: a group takes the position of its first
/// member, so a fan-out reads as one line without the rail losing spawn order.
/// A type with a single member stays flat.
pub fn rows(spawns: &[AgentSpawn], with_main: bool) -> Vec<Row> {
    // Count first, so the grouping decision is settled before any row is built
    // — one pass over the spawns rather than a scan per spawn.
    let mut counts: HashMap<&str, (usize, usize)> = HashMap::with_capacity(spawns.len());
    for spawn in spawns {
        if let Some(kind) = &spawn.kind {
            let slot = counts.entry(kind.as_ref()).or_insert((0, 0));
            slot.0 += 1;
            if spawn.status == SubagentStatus::Running {
                slot.1 += 1;
            }
        }
    }
    let mut out = Vec::with_capacity(spawns.len() + 1);
    if with_main {
        out.push(Row::Main);
    }
    let mut emitted: Vec<&str> = Vec::with_capacity(counts.len());
    for (ix, spawn) in spawns.iter().enumerate() {
        let group = spawn.kind.as_ref().and_then(|kind| {
            counts
                .get(kind.as_ref())
                .filter(|(count, _)| *count > 1)
                .map(|counts| (kind, *counts))
        });
        match group {
            Some((kind, (count, running))) => {
                if emitted.contains(&kind.as_ref()) {
                    continue;
                }
                emitted.push(kind.as_ref());
                out.push(Row::Group {
                    kind: kind.clone(),
                    count,
                    running,
                });
            }
            None => out.push(Row::Agent(ix)),
        }
    }
    out
}

/// Where the rail currently is.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Scope {
    /// The session: `main` plus its own spawns.
    Root,
    /// One agent type's members.
    Group(SharedString),
    /// The agents spawned by one agent.
    Agent {
        doc_id: SharedString,
        title: SharedString,
    },
}

/// The character a keystroke would have typed, or `None` for anything that is
/// a command rather than text: a shortcut (cmd/ctrl/fn), or a control key whose
/// `key_char` is a bare escape sequence.
pub fn typed_char(event: &KeyDownEvent) -> Option<String> {
    let modifiers = event.keystroke.modifiers;
    if modifiers.platform || modifiers.control || modifiers.function {
        return None;
    }
    let text = event.keystroke.key_char.as_deref()?;
    (!text.is_empty() && !text.chars().any(char::is_control)).then(|| text.to_owned())
}

/// Cursor step. `None` means the rail is done with the keyboard — ↑ off the
/// first row hands it back to the composer.
pub fn step(cursor: usize, count: usize, delta: isize) -> Option<usize> {
    if count == 0 {
        return None;
    }
    let next = cursor as isize + delta;
    (next >= 0).then(|| (next as usize).min(count - 1))
}

// ---------------------------------------------------------------------------
// View
// ---------------------------------------------------------------------------

pub struct SubagentNavigator {
    state: Entity<AppState>,
    focus_handle: FocusHandle,
    scroll: gpui::ScrollHandle,
    /// Always non-empty; `scope_stack[0]` is [`Scope::Root`].
    scope_stack: Vec<Scope>,
    /// One cursor per stack depth, so climbing back out lands on the row you
    /// descended from.
    cursor_stack: Vec<usize>,
    /// The transcript the conversation column is showing.
    focused: AgentTarget,
    /// Doc feeds this rail holds open, in the order they were taken.
    feeds: Vec<SharedString>,
    /// The current scope's spawn list, memoized behind
    /// [`AppState::transcript_revision`].
    cache: Option<(u64, SharedString, Vec<AgentSpawn>)>,
    /// The chat the rail belongs to; a change resets it.
    chat_key: String,
    _observe: Subscription,
}

impl EventEmitter<NavigatorEvent> for SubagentNavigator {}

impl Focusable for SubagentNavigator {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl SubagentNavigator {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let chat_key = state.read(cx).composer_key();
        let observe = cx.observe(&state, |this: &mut Self, _, cx| this.on_state_changed(cx));
        Self {
            state,
            focus_handle: cx.focus_handle(),
            scroll: gpui::ScrollHandle::new(),
            scope_stack: vec![Scope::Root],
            cursor_stack: vec![0],
            focused: AgentTarget::Main,
            feeds: Vec::new(),
            cache: None,
            chat_key,
            _observe: observe,
        }
    }

    /// The transcript the conversation column should render.
    pub fn focused(&self) -> &AgentTarget {
        &self.focused
    }

    /// ↓ from the composer: land the cursor on `main` and take the keyboard.
    /// A rail with no agents refuses, so the key keeps its editor meaning.
    pub fn enter_from_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.rows(cx).is_empty() {
            return false;
        }
        self.set_cursor(0, cx);
        window.focus(&self.focus_handle, cx);
        true
    }

    /// Focus an agent from outside the rail (a spawn chip's click). The rail
    /// follows: that agent's row becomes the cursor.
    pub fn focus_target(&mut self, target: AgentTarget, cx: &mut Context<Self>) {
        self.focus(target, cx);
        self.sync_cursor_to_focus(cx);
    }

    // ---- scope + rows ----

    fn scope(&self) -> &Scope {
        self.scope_stack.last().expect("root scope is never popped")
    }

    fn cursor(&self) -> usize {
        *self.cursor_stack.last().expect("one cursor per scope")
    }

    /// The doc whose spawns the current scope lists. Empty = the session's own
    /// transcript.
    fn source_doc(&self) -> SharedString {
        match self.scope() {
            Scope::Root | Scope::Group(_) => SharedString::default(),
            Scope::Agent { doc_id, .. } => doc_id.clone(),
        }
    }

    /// The current scope's spawn list, memoized behind the docs revision: a live
    /// run commits ~8 times a second and the rail renders on every one.
    fn spawns(&mut self, cx: &App) -> &[AgentSpawn] {
        let state = self.state.read(cx);
        let revision = state.transcript_revision;
        let source = self.source_doc();
        let hit = self
            .cache
            .as_ref()
            .is_some_and(|(rev, key, _)| *rev == revision && *key == source);
        if !hit {
            let entries = if source.is_empty() {
                state.transcript.as_slice()
            } else {
                state.sub_transcript(&source)
            };
            self.cache = Some((revision, source, spawns(entries)));
        }
        &self.cache.as_ref().expect("filled above").2
    }

    fn rows(&mut self, cx: &App) -> Vec<Row> {
        let scope = self.scope().clone();
        let spawns = self.spawns(cx);
        match scope {
            // Inside a group the rows ARE its members: folding again would
            // collapse them straight back into the group you just opened.
            Scope::Group(kind) => spawns
                .iter()
                .enumerate()
                .filter(|(_, spawn)| spawn.kind.as_ref() == Some(&kind))
                .map(|(ix, _)| Row::Agent(ix))
                .collect(),
            Scope::Root => rows(spawns, true),
            Scope::Agent { .. } => rows(spawns, false),
        }
    }

    fn set_cursor(&mut self, cursor: usize, cx: &mut Context<Self>) {
        if let Some(slot) = self.cursor_stack.last_mut() {
            *slot = cursor;
        }
        self.scroll.scroll_to_item(cursor);
        cx.notify();
    }

    /// Park the cursor on the focused agent's row when this scope has one, so
    /// the highlight and the rendered transcript agree.
    fn sync_cursor_to_focus(&mut self, cx: &mut Context<Self>) {
        let Some(doc_id) = self.focused.doc_id().cloned() else {
            if matches!(self.scope(), Scope::Root) {
                self.set_cursor(0, cx);
            }
            return;
        };
        let rows = self.rows(cx);
        let at = rows.iter().position(|row| match row {
            Row::Agent(ix) => self
                .cache
                .as_ref()
                .and_then(|(_, _, spawns)| spawns.get(*ix))
                .is_some_and(|spawn| spawn.doc_id == doc_id),
            _ => false,
        });
        if let Some(at) = at {
            self.set_cursor(at, cx);
        }
    }

    // ---- navigation ----

    /// Enter: open what the cursor is on.
    fn activate(&mut self, cx: &mut Context<Self>) {
        let rows = self.rows(cx);
        let Some(row) = rows.get(self.cursor()).cloned() else {
            return;
        };
        match row {
            Row::Main => self.focus(AgentTarget::Main, cx),
            Row::Group { kind, .. } => self.push_scope(Scope::Group(kind), cx),
            Row::Agent(ix) => {
                if let Some(target) = self.spawns(cx).get(ix).map(AgentSpawn::target) {
                    self.focus(target, cx);
                }
            }
        }
    }

    /// →: one level deeper. On a group that is its members; on an agent it is
    /// the agents that agent spawned, which also focuses it — its doc has to be
    /// fed before the rail can list anything inside it.
    fn descend(&mut self, cx: &mut Context<Self>) {
        let rows = self.rows(cx);
        let Some(row) = rows.get(self.cursor()).cloned() else {
            return;
        };
        match row {
            Row::Main => {}
            Row::Group { kind, .. } => self.push_scope(Scope::Group(kind), cx),
            Row::Agent(ix) => {
                let Some(spawn) = self.spawns(cx).get(ix).cloned() else {
                    return;
                };
                self.focus(spawn.target(), cx);
                self.push_scope(
                    Scope::Agent {
                        doc_id: spawn.doc_id,
                        title: spawn.title,
                    },
                    cx,
                );
            }
        }
    }

    /// ← / Esc: back out one level, or hand the keyboard back at the root.
    fn ascend(&mut self, cx: &mut Context<Self>) {
        if self.scope_stack.len() <= 1 {
            cx.emit(NavigatorEvent::ReturnToComposer);
            return;
        }
        self.scope_stack.pop();
        self.cursor_stack.pop();
        self.cache = None;
        self.retain_feeds(cx);
        self.scroll.scroll_to_item(self.cursor());
        cx.notify();
    }

    fn push_scope(&mut self, scope: Scope, cx: &mut Context<Self>) {
        if *self.scope() == scope {
            return;
        }
        self.scope_stack.push(scope);
        self.cursor_stack.push(0);
        self.cache = None;
        self.retain_feeds(cx);
        self.scroll.scroll_to_item(0);
        cx.notify();
    }

    fn focus(&mut self, target: AgentTarget, cx: &mut Context<Self>) {
        if self.focused == target {
            return;
        }
        self.focused = target;
        self.retain_feeds(cx);
        cx.emit(NavigatorEvent::Focus(self.focused.clone()));
        cx.notify();
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        // Bare navigation keys are unbound outside the composer's key context,
        // so they reach here unconsumed (gpui runs matched bindings first).
        let key = event.keystroke.key.as_str();
        match key {
            "up" | "down" => {
                let count = self.rows(cx).len();
                let delta = if key == "up" { -1 } else { 1 };
                match step(self.cursor(), count, delta) {
                    Some(next) => self.set_cursor(next, cx),
                    // Off the top of the list: back to typing.
                    None => cx.emit(NavigatorEvent::ReturnToComposer),
                }
            }
            "right" => self.descend(cx),
            "left" | "escape" => self.ascend(cx),
            "enter" => self.activate(cx),
            _ => {
                // A shortcut chord is not the rail's to consume.
                let Some(text) = typed_char(event) else {
                    return;
                };
                cx.emit(NavigatorEvent::TypeIntoComposer(text));
            }
        }
        cx.stop_propagation();
    }

    // ---- doc feeds ----

    /// Hold open the feeds the scope path and the focus need, release the
    /// rest. The set is tiny and recomputed whole: a feed left running keeps
    /// its doc pinned in the engine's LRU forever. The blob for a frozen
    /// agent is keyed by the SESSION's chat id even for a nested agent — the
    /// engine uploads every sidecar under the run's chat.
    fn retain_feeds(&mut self, cx: &mut Context<Self>) {
        let mut wanted: Vec<(SharedString, bool)> = Vec::with_capacity(self.scope_stack.len() + 1);
        for scope in &self.scope_stack {
            if let Scope::Agent { doc_id, .. } = scope {
                // A scope's own doc is watched live: it may still be spawning
                // the children the rail is listing.
                wanted.push((doc_id.clone(), false));
            }
        }
        if let AgentTarget::Agent { doc_id, frozen, .. } = &self.focused
            && !wanted.iter().any(|(id, _)| id == doc_id)
        {
            wanted.push((doc_id.clone(), *frozen));
        }
        let chat_id = self
            .state
            .read(cx)
            .selected_chat
            .clone()
            .unwrap_or_default();
        let state = self.state.clone();
        self.feeds.retain(|doc_id| {
            let keep = wanted.iter().any(|(id, _)| id == doc_id);
            if !keep {
                state.update(cx, |state, _| state.close_subagent_feed(doc_id));
            }
            keep
        });
        for (doc_id, frozen) in wanted {
            if self.feeds.contains(&doc_id) {
                continue;
            }
            state.update(cx, |state, cx| {
                state.open_subagent_feed(&chat_id, doc_id.to_string(), frozen, cx)
            });
            self.feeds.push(doc_id);
        }
    }

    /// A chat switch is a different set of agents entirely: drop the scope, the
    /// feeds and the focus, and put the column back on the session.
    fn on_state_changed(&mut self, cx: &mut Context<Self>) {
        let key = self.state.read(cx).composer_key();
        if key == self.chat_key {
            cx.notify();
            return;
        }
        self.chat_key = key;
        self.scope_stack.truncate(1);
        self.cursor_stack.truncate(1);
        self.cursor_stack[0] = 0;
        self.cache = None;
        let was_agent = self.focused != AgentTarget::Main;
        self.focused = AgentTarget::Main;
        self.retain_feeds(cx);
        if was_agent {
            cx.emit(NavigatorEvent::Focus(AgentTarget::Main));
        }
        cx.notify();
    }
}

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

impl Render for SubagentNavigator {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        // One row build per frame: `rows` reads the memoized spawn list, and
        // everything below reads the SAME Vec rather than rebuilding it.
        let rows = self.rows(cx);
        if rows.is_empty() && self.scope_stack.len() == 1 {
            return div();
        }
        // The list can shrink under the cursor (a doc resubscribe replays fewer
        // rows); never highlight past the end.
        let cursor = self.cursor().min(rows.len().saturating_sub(1));
        if let Some(slot) = self.cursor_stack.last_mut() {
            *slot = cursor;
        }
        let focus_handle = self.focus_handle.clone();
        let keyboard = focus_handle.is_focused(window);
        let scroll = self.scroll.clone();
        let crumb = self.render_crumb(&theme, cx);
        let shown = rows.len().min(MAX_VISIBLE_ROWS);
        let list_height = shown as f32 * ROW_HEIGHT + shown.saturating_sub(1) as f32 * ROW_GAP;
        let focused_doc = self.focused.doc_id().cloned();
        // A group wears the focus mark when the agent on screen is one of its
        // members — the accent must not vanish just because the rail climbed
        // back out to the folder.
        let spawns = self.spawns(cx);
        let focused_kind = focused_doc.as_ref().and_then(|doc| {
            spawns
                .iter()
                .find(|spawn| &spawn.doc_id == doc)
                .and_then(|spawn| spawn.kind.clone())
        });
        let list = div()
            .id("agent-rail-rows")
            .flex()
            .flex_col()
            .gap(px(ROW_GAP))
            .max_h(px(list_height))
            .overflow_y_scroll()
            .track_scroll(&scroll)
            .children(rows.iter().enumerate().map(|(ix, row)| {
                agent_row(
                    ix,
                    row,
                    spawns,
                    cursor,
                    keyboard,
                    focused_doc.as_ref(),
                    focused_kind.as_ref(),
                    &theme,
                    cx,
                )
            }));

        div()
            .key_context("AgentRail")
            .track_focus(&focus_handle)
            .w_full()
            .flex()
            .flex_col()
            .gap(px(2.0))
            // Same trick as the branch toolbar above it: bleed half the
            // composer container's bottom padding so the rail sits in even air
            // instead of pressed against the window edge.
            .mt(px(2.0))
            .mb(px(-8.0))
            .px(px(2.0))
            .on_key_down(
                cx.listener(|this, event: &KeyDownEvent, _, cx| this.on_key_down(event, cx)),
            )
            .children(crumb)
            .child(list)
    }
}

impl SubagentNavigator {
    /// The scope line, shown only below the root: which folder or agent you are
    /// inside, and the way back out.
    fn render_crumb(&mut self, theme: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let label: SharedString = match self.scope() {
            Scope::Root => return None,
            Scope::Group(kind) => kind.clone(),
            Scope::Agent { title, .. } => title.clone(),
        };
        Some(
            div()
                .id("agent-rail-crumb")
                .h(px(CRUMB_HEIGHT))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(4.0))
                .px(px(6.0))
                .cursor_pointer()
                .text_size(crate::typography::ui_rems(11.0))
                .text_color(theme.text_faint)
                .on_click(cx.listener(|this, _, _, cx| this.ascend(cx)))
                .child(
                    crate::icons::icon(crate::icons::ALT_ARROW_LEFT)
                        .size(px(10.0))
                        .text_color(theme.text_faint),
                )
                .child(div().min_w_0().truncate().child(label))
                .into_any_element(),
        )
    }
}

/// One rail row. A free function, not a method: the spawn slice it reads is
/// borrowed out of the navigator's memo, so the row cannot also hold `&self` —
/// and cloning the list once a frame to dodge that is exactly the per-frame
/// allocation this rail must not have.
#[allow(clippy::too_many_arguments)]
fn agent_row(
    ix: usize,
    row: &Row,
    spawns: &[AgentSpawn],
    cursor: usize,
    keyboard: bool,
    focused_doc: Option<&SharedString>,
    focused_kind: Option<&SharedString>,
    theme: &Theme,
    cx: &mut Context<SubagentNavigator>,
) -> AnyElement {
    let spawn = match row {
        Row::Agent(at) => match spawns.get(*at) {
            Some(spawn) => Some(spawn),
            None => return gpui::Empty.into_any_element(),
        },
        _ => None,
    };
    let (label, icon_path) = match row {
        Row::Main => (SharedString::from("main"), crate::icons::CHAT_ROUND_LINE),
        Row::Group { kind, .. } => (kind.clone(), crate::icons::FOLDER),
        Row::Agent(_) => (
            spawn.expect("agent row has a spawn").title.clone(),
            crate::icons::BOT,
        ),
    };
    // Two independent states share a row: FOCUSED is the transcript the column
    // shows (the accent tile), the cursor is where the keyboard is (the wash).
    // They coincide most of the time and must still read apart when they don't.
    let focused = match row {
        Row::Main => focused_doc.is_none(),
        // A folder inherits the mark from whichever member is on screen.
        Row::Group { kind, .. } => focused_kind == Some(kind),
        Row::Agent(_) => spawn.map(|s| &s.doc_id) == focused_doc,
    };
    let at_cursor = ix == cursor;
    let hover_key = format!("agent-rail-{ix}");
    let rest = if at_cursor && keyboard {
        crate::theme::wash(0.10)
    } else {
        gpui::transparent_black()
    };
    let hovered = if at_cursor && keyboard {
        crate::theme::wash(0.12)
    } else {
        crate::theme::wash(0.06)
    };
    let (tile_bg, tile_fg) = if focused {
        (theme.accent_wash, theme.accent)
    } else {
        (crate::theme::ink(0.08), theme.text_muted)
    };
    let failed = spawn.is_some_and(|s| s.status == SubagentStatus::Failed);
    let text_color = if failed {
        theme.danger
    } else if focused || at_cursor {
        theme.text
    } else {
        theme.text_muted
    };
    let running = match row {
        Row::Main => false,
        Row::Group { running, .. } => *running > 0,
        Row::Agent(_) => spawn.is_some_and(|s| s.status == SubagentStatus::Running),
    };
    let trailing: Option<AnyElement> = match row {
        Row::Group { count, .. } => Some(
            div()
                .flex_none()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(4.0))
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(theme.text_faint)
                        .child(SharedString::from(count.to_string())),
                )
                .child(
                    crate::icons::icon(crate::icons::ALT_ARROW_RIGHT)
                        .size(px(11.0))
                        .text_color(theme.text_faint),
                )
                .into_any_element(),
        ),
        _ => None,
    };
    let clicked = row.clone();
    div()
        .id(("agent-rail-row", ix))
        .h(px(ROW_HEIGHT))
        .w_full()
        .flex_none()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .px(px(6.0))
        .rounded(px(8.0))
        .cursor_pointer()
        .bg(motion::hover_blend(&hover_key, rest, hovered))
        .on_hover(motion::hover_listener(hover_key))
        .on_click(cx.listener(move |this, _, _, cx| {
            this.set_cursor(ix, cx);
            match &clicked {
                Row::Main => this.focus(AgentTarget::Main, cx),
                Row::Group { kind, .. } => this.push_scope(Scope::Group(kind.clone()), cx),
                Row::Agent(at) => {
                    if let Some(target) = this.spawns(cx).get(*at).map(AgentSpawn::target) {
                        this.focus(target, cx);
                    }
                }
            }
        }))
        .child(
            // The transcript chip's icon tile, to the pixel — the rail lists
            // the same things those chips announce.
            div()
                .size(px(18.0))
                .flex_none()
                .rounded(px(5.0))
                .bg(tile_bg)
                .flex()
                .items_center()
                .justify_center()
                .child(
                    crate::icons::icon(icon_path)
                        .size(px(12.0))
                        .text_color(if failed { theme.danger } else { tile_fg }),
                ),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(crate::typography::ui_rems(13.0))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(text_color)
                .child(label),
        )
        .when(running, |el| {
            el.child(div().flex_none().child(crate::loaders::mini_glyph_spinner(
                format!("agent-rail-spin-{ix}"),
                2.0,
                theme.glyph,
                cx.entity_id(),
                cx,
            )))
        })
        .children(trailing)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeron_doc::MessageRole;

    fn tool(id: &str, name: &str, doc: Option<&str>, kind: Option<&str>) -> MessagePart {
        MessagePart::Tool {
            id: id.into(),
            call: ToolCall::Unknown {
                name: name.into(),
                input: kind.map(|kind| serde_json::json!({ "subagent_type": kind })),
            },
            is_error: false,
            resolved: true,
            output: None,
            diff: None,
            output_ref: None,
            output_bytes: None,
            diff_ref: None,
            diff_stats: None,
            subagent_ref: doc.map(str::to_owned),
            subagent_status: doc.map(|_| SubagentStatus::Done),
            subagent_tail: None,
        }
    }

    fn entry(parts: Vec<MessagePart>) -> SessionMessageEntry {
        SessionMessageEntry {
            id: "e1".into(),
            role: MessageRole::Assistant,
            parts,
            created_at: 0,
            device_id: "d".into(),
            status: None,
            continuation_of: None,
        }
    }

    #[test]
    fn only_stamped_spawn_chips_reach_the_rail() {
        let found = spawns(&[entry(vec![
            // A mis-keyed ref on an ordinary chip must not become a row: the
            // doc behind it was never created.
            tool("t1", "Run", Some("chat--sub--x"), None),
            // A spawn the engine hasn't stamped yet has nothing to open.
            tool("t2", "Agent: pending", None, Some("Explore")),
            tool(
                "t3",
                "Agent: read the diff",
                Some("chat--sub--a"),
                Some("Explore"),
            ),
        ])]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].doc_id.as_ref(), "chat--sub--a");
        assert_eq!(found[0].title.as_ref(), "read the diff");
        assert_eq!(found[0].kind.as_deref(), Some("Explore"));
        assert!(found[0].frozen());
    }

    fn spawn(doc: &str, kind: Option<&str>, status: SubagentStatus) -> AgentSpawn {
        AgentSpawn {
            doc_id: doc.into(),
            title: doc.into(),
            kind: kind.map(SharedString::from),
            status,
        }
    }

    #[test]
    fn a_type_folds_into_a_group_only_once_it_has_two_members() {
        let list = vec![
            spawn("a", Some("Explore"), SubagentStatus::Running),
            spawn("b", Some("Plan"), SubagentStatus::Done),
            spawn("c", Some("Explore"), SubagentStatus::Done),
            spawn("d", None, SubagentStatus::Running),
        ];
        // `Explore` folds (two members, one still running) and keeps its first
        // member's position; the lone `Plan` and the untyped spawn stay flat.
        assert_eq!(
            rows(&list, true),
            vec![
                Row::Main,
                Row::Group {
                    kind: "Explore".into(),
                    count: 2,
                    running: 1,
                },
                Row::Agent(1),
                Row::Agent(3),
            ]
        );
        // Only the root scope carries `main`; the rest of the fold is identical.
        assert_eq!(rows(&list, false), rows(&list, true)[1..]);
    }

    #[test]
    fn the_cursor_clamps_at_the_bottom_and_leaves_at_the_top() {
        assert_eq!(step(0, 3, 1), Some(1));
        assert_eq!(step(2, 3, 1), Some(2));
        assert_eq!(step(1, 3, -1), Some(0));
        // Off the first row: the rail is done, the composer takes over.
        assert_eq!(step(0, 3, -1), None);
        assert_eq!(step(0, 0, 1), None);
    }
}
