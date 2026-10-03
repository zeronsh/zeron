//! The explorer's footer: two collapsible sections docked under the file
//! tree — **Subagents** (the spawn chips of the active chat's transcript,
//! with their live status) and **Chats** (the side chats hanging off the
//! active chat: forks, and chats an agent spawned through the Zeron MCP
//! server). Rows borrow the left sidebar's compact session row — 29px, status
//! glyph, title, time — minus the harness, project and device icons, which
//! say nothing here (every row shares the parent's context). Clicking a row
//! opens it in the right pane's surface host; the Chats header carries "+"
//! and fork beside its caret; the section chrome animates with the same
//! collapse motion as the sidebar's disclosures.
//!
//! The footer has a fixed height budget that the open sections share and
//! scroll inside, and like the sidebar's Archived shelf each shows ten rows
//! before a "Show N more" row pages by ten.
//!
//! An empty section stays collapsed and opens when its first row arrives.
//! A collapse the user chooses is a preference shared by every explorer
//! (see [`CollapsedSections`]), not state of one chat's explorer.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use chrono::{DateTime, Utc};
use gpui::{
    Animation, AnimationExt as _, AnyElement, Context, Entity, EntityId, MouseButton, ScrollHandle,
    SharedString, div, prelude::*, px,
};
use zeron_doc::{MessagePart, SubagentStatus};
use zeron_proto::{Chat, ChatIndicator};

use crate::composer::ComposerInput;
use crate::icons::{self, icon};
use crate::state::AppState;
use crate::theme::Theme;
use crate::{loaders, motion};

use super::{FilesEvent, FilesSurface};

const SECTION_HEADER_HEIGHT: f32 = 28.0;
const SECTION_BODY_INSET: f32 = 4.0;
const ROW_HEIGHT: f32 = 29.0;
const ROW_GAP: f32 = 2.0;
/// Fade band under a section list's edges (the sidebar's treatment, scaled
/// to the shorter lists).
const LIST_FADE_BAND: f32 = 16.0;
/// Hover group of a section header (reveals its actions).
const HEADER_GROUP: &str = "files-section-header";
/// Rows a section shows before "Show more" pages it, and the page size —
/// the sidebar's Archived shelf numbers.
const INITIAL_ROWS: usize = 10;
const PAGE_ROWS: usize = 10;
const FOOTER_PAD_TOP: f32 = 4.0;
const FOOTER_PAD_BOTTOM: f32 = 6.0;
/// The footer's height budget; shorter content shrinks the footer to fit.
const FOOTER_HEIGHT: f32 = 510.0;
const TWEEN_GRACE: std::time::Duration = std::time::Duration::from_millis(120);

/// Which footer section a motion or toggle addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum Section {
    Subagents,
    Chats,
}

/// The user's collapse choice per section, persisted in settings and shared
/// by every explorer. It never opens an empty section.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CollapsedSections {
    pub subagents: bool,
    pub chats: bool,
}

impl CollapsedSections {
    fn get(self, section: Section) -> bool {
        match section {
            Section::Subagents => self.subagents,
            Section::Chats => self.chats,
        }
    }

    fn set(&mut self, section: Section, collapsed: bool) {
        match section {
            Section::Subagents => self.subagents = collapsed,
            Section::Chats => self.chats = collapsed,
        }
    }
}

impl Section {
    fn key(self) -> &'static str {
        match self {
            Section::Subagents => "subagents",
            Section::Chats => "chats",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Section::Subagents => "Subagents",
            Section::Chats => "Chats",
        }
    }
}

/// One in-flight open/close of a section body (the sidebar's
/// `SidebarDisclosureMotion`, kept local so the explorer owns its own
/// epochs). A re-toggle mid-flight picks up from the painted height.
#[derive(Debug, Clone, Copy)]
struct DisclosureMotion {
    epoch: u64,
    from: f32,
    to: f32,
    started: std::time::Instant,
}

impl DisclosureMotion {
    fn current(self) -> f32 {
        let total = motion::COLLAPSE.total().as_secs_f32();
        let raw = if total > 0.0 {
            self.started.elapsed().as_secs_f32() / total
        } else {
            1.0
        };
        motion::lerp(self.from, self.to, motion::COLLAPSE.progress(raw))
    }

    fn animating(self) -> bool {
        self.started.elapsed() < motion::COLLAPSE.total() + TWEEN_GRACE
    }
}

/// The footer's rows, derived from app state once per state change rather
/// than on every render.
#[derive(Debug, Default)]
struct SectionRows {
    subagents: Vec<SubagentRow>,
    chats: Vec<ChildChatRow>,
}

impl SectionRows {
    fn count(&self, section: Section) -> usize {
        match section {
            Section::Subagents => self.subagents.len(),
            Section::Chats => self.chats.len(),
        }
    }
}

/// Footer state on the explorer surface.
#[derive(Debug)]
pub(super) struct ExplorerSections {
    collapsed: CollapsedSections,
    /// Open state and body height as last painted. When a row arrival or
    /// departure changes whether a section is open, the body animates from
    /// here exactly as a click would.
    painted: HashMap<Section, (bool, f32)>,
    motion: HashMap<Section, DisclosureMotion>,
    /// Rows revealed per section ("Show more" pages this up).
    shown: HashMap<Section, usize>,
    /// One scroll handle per section list, so the edge fades can read
    /// overflow at paint time.
    scroll: HashMap<Section, ScrollHandle>,
    rows: Option<SectionRows>,
    /// Hash of what the footer would draw, so the state observer only
    /// re-renders the explorer when a section's contents actually changed —
    /// not on every streamed transcript delta.
    fingerprint: u64,
    /// The side chat whose title the shell is editing in place, and its
    /// field — drawn over that row's title.
    chat_rename: Option<(String, Entity<ComposerInput>)>,
}

impl Default for ExplorerSections {
    fn default() -> Self {
        Self {
            collapsed: CollapsedSections::default(),
            painted: HashMap::new(),
            motion: HashMap::new(),
            shown: HashMap::new(),
            scroll: [
                (Section::Subagents, ScrollHandle::new()),
                (Section::Chats, ScrollHandle::new()),
            ]
            .into_iter()
            .collect(),
            rows: None,
            fingerprint: 0,
            chat_rename: None,
        }
    }
}

impl ExplorerSections {
    /// Open when the user hasn't collapsed it and there is something to show.
    fn is_open(&self, section: Section, count: usize) -> bool {
        count > 0 && !self.collapsed.get(section)
    }

    fn shown(&self, section: Section) -> usize {
        self.shown
            .get(&section)
            .copied()
            .unwrap_or(INITIAL_ROWS)
            .max(INITIAL_ROWS)
    }

    /// Animate a section body from where it is painted now to `target`.
    fn animate(&mut self, section: Section, resting: f32, target: f32) {
        let previous = self.motion.get(&section).copied();
        let from = previous
            .filter(|m| m.animating())
            .map(DisclosureMotion::current)
            .unwrap_or(resting);
        let epoch = previous.map_or(1, |m| m.epoch + 1);
        self.motion.insert(
            section,
            DisclosureMotion {
                epoch,
                from,
                to: target,
                started: std::time::Instant::now(),
            },
        );
    }

    /// Record this frame's open state and body height, animating when the
    /// open state changed without a click (a first row, a last row gone).
    fn paint(&mut self, section: Section, open: bool, height: f32) {
        let target = if open { height } else { 0.0 };
        if let Some((was_open, painted)) = self.painted.get(&section).copied()
            && was_open != open
        {
            self.animate(section, painted, target);
        }
        self.painted.insert(section, (open, target));
    }

    fn scroll(&self, section: Section) -> ScrollHandle {
        self.scroll
            .get(&section)
            .cloned()
            .unwrap_or_else(ScrollHandle::new)
    }

    fn live_motion(&self, section: Section) -> Option<DisclosureMotion> {
        self.motion.get(&section).copied().filter(|m| m.animating())
    }
}

/// A spawn chip of the active transcript, as the footer lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SubagentRow {
    pub doc_id: String,
    pub title: SharedString,
    pub status: Option<SubagentStatus>,
    /// When the latest turn that spawned (or steered) it was written — the
    /// closest thing a subagent has to a last-updated time.
    pub spawned_at: DateTime<Utc>,
}

impl SubagentRow {
    /// A settled subagent is frozen: the tab reads its blob first.
    pub fn frozen(&self) -> bool {
        matches!(
            self.status,
            Some(SubagentStatus::Done) | Some(SubagentStatus::Failed)
        )
    }

    fn indicator(&self) -> ChatIndicator {
        match self.status {
            Some(SubagentStatus::Running) => ChatIndicator::Working,
            Some(SubagentStatus::Done) => ChatIndicator::Completed,
            Some(SubagentStatus::Failed) => ChatIndicator::Errored,
            None => ChatIndicator::Idle,
        }
    }
}

/// The active chat's subagents, one row per subagent doc: running ones first,
/// longest-running leading, then the settled ones most recently updated first
/// (later spawns lead within one turn).
/// Only genuine spawn chips with a stamped doc ref qualify — the chip IS the
/// index (there is no listing endpoint), and a stray ref on a non-Agent tool
/// must not surface as a phantom subagent.
pub(super) fn subagent_rows(state: &AppState, chat_id: &str) -> Vec<SubagentRow> {
    if state.selected_chat.as_deref() != Some(chat_id) {
        return Vec::new();
    }
    let mut rows: Vec<SubagentRow> = Vec::new();
    for entry in &state.transcript {
        let spawned_at =
            DateTime::<Utc>::from_timestamp_millis(entry.created_at).unwrap_or_else(Utc::now);
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
            if !call.is_subagent_spawn() {
                continue;
            }
            let row = SubagentRow {
                doc_id: doc_id.clone(),
                title: crate::transcript::subagent_tab_title(call),
                status: *subagent_status,
                spawned_at,
            };
            match rows.iter_mut().find(|r| r.doc_id == row.doc_id) {
                // A reopened (steered) subagent updates its row in place.
                Some(existing) => *existing = row,
                None => rows.push(row),
            }
        }
    }
    // Stable sort over the reversed spawn order: ties keep the later spawn
    // on top.
    rows.reverse();
    rows.sort_by_key(|row| std::cmp::Reverse(row.spawned_at));
    // Running subagents lead, longest-running first: one started long ago is
    // buried under everything spawned since, and is the one to find and steer.
    // Oldest-first also keeps the group still as new subagents join at its
    // foot. The settled tail keeps the newest-first order above.
    let (mut running, settled): (Vec<_>, Vec<_>) = rows
        .into_iter()
        .partition(|row| row.status == Some(SubagentStatus::Running));
    // Ties keep spawn order, like the group's oldest-first order.
    running.reverse();
    running.sort_by_key(|row| row.spawned_at);
    running.extend(settled);
    running
}

/// A side chat of the active chat, as the footer lists it.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct ChildChatRow {
    pub chat_id: String,
    pub title: SharedString,
    pub status: ChatIndicator,
    pub time_ago: SharedString,
    /// The chat's linked pull request, drawn as the sidebar's badge.
    pub change_request: Option<zeron_proto::ChangeRequestSummary>,
    activity: DateTime<Utc>,
}

/// The live (unarchived) children of `chat_id`, most recent activity first —
/// the same order the sidebar's Sessions list keeps.
pub(super) fn child_chat_rows(
    state: &AppState,
    chat_id: &str,
    now: DateTime<Utc>,
) -> Vec<ChildChatRow> {
    let mut rows: Vec<ChildChatRow> = state
        .chats
        .iter()
        .filter(|chat| !chat.archived && chat.parent_chat_id.as_deref() == Some(chat_id))
        .map(|chat| {
            let activity = chat.last_message_at.unwrap_or(chat.created_at);
            ChildChatRow {
                chat_id: chat.id.clone(),
                title: child_chat_title(chat).into(),
                status: state.display_status_for(chat, now),
                time_ago: zeron_proto::view::format_time_ago(activity, now).into(),
                change_request: state.change_request_for_chat(chat).cloned(),
                activity,
            }
        })
        .collect();
    rows.sort_by_key(|row| std::cmp::Reverse(row.activity));
    rows
}

/// A side chat titles itself on its first turn; until then the preview or a
/// placeholder stands in.
pub(super) fn child_chat_title(chat: &Chat) -> String {
    chat.title
        .clone()
        .or_else(|| chat.last_message_preview.clone())
        .unwrap_or_else(|| "New side chat".into())
}

/// What the footer would draw for these rows, hashed.
fn fingerprint(subagents: &[SubagentRow], chats: &[ChildChatRow]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for row in subagents {
        row.doc_id.hash(&mut hasher);
        row.title.as_ref().hash(&mut hasher);
        (row.status.map(|s| s as u8)).hash(&mut hasher);
    }
    0xC0FFEEu64.hash(&mut hasher);
    for row in chats {
        row.chat_id.hash(&mut hasher);
        row.title.as_ref().hash(&mut hasher);
        (row.status as u8).hash(&mut hasher);
        row.time_ago.as_ref().hash(&mut hasher);
        row.change_request
            .as_ref()
            .map(|pr| (pr.number, pr.state as u8))
            .hash(&mut hasher);
    }
    hasher.finish()
}

/// The height a section body wants for `count` rows with `shown` revealed:
/// inset, the visible rows, and a "Show more" row while more remain. An
/// empty section is collapsed and wants nothing.
pub(super) fn content_height(count: usize, shown: usize) -> f32 {
    if count == 0 {
        return 0.0;
    }
    let visible = count.min(shown);
    let more = if count > shown { 1 } else { 0 };
    let slots = visible + more;
    // Sized to its rows: one side chat takes one row, not a reserved block.
    SECTION_BODY_INSET + slots as f32 * ROW_HEIGHT + slots.saturating_sub(1) as f32 * ROW_GAP
}

/// Split the footer's body budget between two open sections: each may take
/// what it wants, and a short one hands its slack to the other. Closed
/// sections get 0. Pure.
pub(super) fn body_budget(budget: f32, wants: [f32; 2], open: [bool; 2]) -> [f32; 2] {
    let budget = budget.max(0.0);
    let want = |i: usize| if open[i] { wants[i] } else { 0.0 };
    let (a, b) = (want(0), want(1));
    if a + b <= budget {
        return [a, b];
    }
    let half = budget / 2.0;
    let first = a.min(budget - b.min(half));
    let second = b.min(budget - first);
    [first, second]
}

/// The footer's chrome outside the bodies: padding and the two headers.
fn chrome_height() -> f32 {
    FOOTER_PAD_TOP + FOOTER_PAD_BOTTOM + 2.0 * SECTION_HEADER_HEIGHT
}

impl FilesSurface {
    /// Show (or clear) the shell's inline rename of one of the Chats rows.
    pub(crate) fn set_chat_rename(
        &mut self,
        rename: Option<(String, Entity<ComposerInput>)>,
        cx: &mut Context<Self>,
    ) {
        let key = |rename: &Option<(String, Entity<ComposerInput>)>| {
            rename
                .as_ref()
                .map(|(id, input)| (id.clone(), input.entity_id()))
        };
        if key(&self.sections.chat_rename) != key(&rename) {
            if let Some((chat_id, _)) = &rename {
                self.reveal_chat_row(chat_id, cx);
            }
            self.sections.chat_rename = rename;
            cx.notify();
        }
    }

    /// Open the Chats section, page it, and scroll its list so `chat_id`'s
    /// row shows — the inline rename's field must never sit on a hidden row.
    fn reveal_chat_row(&mut self, chat_id: &str, cx: &mut Context<Self>) {
        let rows = child_chat_rows(self.state.read(cx), &self.chat_id, Utc::now());
        let Some(index) = rows.iter().position(|row| row.chat_id == chat_id) else {
            return;
        };
        // Opening the section here is the user's choice too; its next paint
        // runs the opening motion.
        if self.sections.collapsed.chats {
            self.sections.collapsed.set(Section::Chats, false);
            cx.emit(FilesEvent::SectionsCollapsedChanged(
                self.sections.collapsed,
            ));
        }
        // Page in the same steps "Show more" takes.
        if index >= self.sections.shown(Section::Chats) {
            self.sections.shown.insert(
                Section::Chats,
                INITIAL_ROWS + (index + 1 - INITIAL_ROWS).div_ceil(PAGE_ROWS) * PAGE_ROWS,
            );
        }
        self.sections.scroll(Section::Chats).scroll_to_item(index);
    }

    /// Re-derive the footer rows from app state; re-render only when what
    /// the footer draws changed.
    pub(super) fn refresh_sections(&mut self, cx: &mut Context<Self>) {
        let rows = {
            let state = self.state.read(cx);
            SectionRows {
                subagents: subagent_rows(state, &self.chat_id),
                chats: child_chat_rows(state, &self.chat_id, Utc::now()),
            }
        };
        let fingerprint = fingerprint(&rows.subagents, &rows.chats);
        let changed = fingerprint != self.sections.fingerprint || self.sections.rows.is_none();
        self.sections.fingerprint = fingerprint;
        self.sections.rows = Some(rows);
        if changed {
            cx.notify();
        }
    }

    /// Apply the shared collapse preference. A click already animated the
    /// explorer that made it; the others follow on their next paint.
    pub fn set_collapsed_sections(&mut self, collapsed: CollapsedSections, cx: &mut Context<Self>) {
        if self.sections.collapsed != collapsed {
            self.sections.collapsed = collapsed;
            cx.notify();
        }
    }

    pub(super) fn render_sections(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        if self.sections.rows.is_none() {
            self.refresh_sections(cx);
        }
        let rows = self.sections.rows.take().unwrap_or_default();
        let counts = [rows.count(Section::Subagents), rows.count(Section::Chats)];
        let wants = [
            content_height(counts[0], self.sections.shown(Section::Subagents)),
            content_height(counts[1], self.sections.shown(Section::Chats)),
        ];
        let open = [
            self.sections.is_open(Section::Subagents, counts[0]),
            self.sections.is_open(Section::Chats, counts[1]),
        ];
        let budget = FOOTER_HEIGHT - chrome_height();
        let heights = body_budget(budget, wants, open);
        self.sections.paint(Section::Subagents, open[0], heights[0]);
        self.sections.paint(Section::Chats, open[1], heights[1]);
        let (subagents, chats) = (&rows.subagents, &rows.chats);
        let view = cx.entity_id();
        let subagent_body = self.render_subagent_rows(subagents, view, theme, cx);
        let chat_body = self.render_chat_rows(chats, theme, cx);
        let chats_actions = self.render_chats_header_actions(counts[1] == 0, theme, cx);
        let footer = div()
            .id("files-sections")
            .relative()
            .flex_none()
            .w_full()
            .flex()
            .flex_col()
            .px(px(6.0))
            .pt(px(FOOTER_PAD_TOP))
            .pb(px(FOOTER_PAD_BOTTOM))
            .child(self.render_section(
                Section::Subagents,
                subagents.len(),
                wants[0],
                heights[0],
                None,
                subagent_body,
                theme,
                cx,
            ))
            .child(self.render_section(
                Section::Chats,
                chats.len(),
                wants[1],
                heights[1],
                Some(chats_actions),
                chat_body,
                theme,
                cx,
            ))
            .into_any_element();
        self.sections.rows = Some(rows);
        footer
    }

    /// "+" and fork beside the Chats caret: a fresh side chat of the active
    /// chat, or a fork of it through its latest completed response.
    fn render_chats_header_actions(
        &self,
        empty: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // Hidden until the header is hovered, like the sidebar's row menus:
        // the caret is the resting state, the actions appear on approach.
        // An empty section has no caret and no rows, so its actions are the
        // only way in and stay visible.
        div()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(2.0))
            .when(!empty, |actions| {
                actions
                    .opacity(0.0)
                    .group_hover(HEADER_GROUP, |s| s.opacity(1.0))
            })
            .child(
                header_action(
                    "files-sections-new-chat",
                    icons::PLUS,
                    "New side chat",
                    theme,
                )
                .on_click(cx.listener(|_, _, _, cx| {
                    cx.stop_propagation();
                    cx.emit(FilesEvent::NewChildChat);
                })),
            )
            .child(
                header_action(
                    "files-sections-fork",
                    icons::GIT_BRANCH,
                    "Fork this chat",
                    theme,
                )
                .on_click(cx.listener(|_, _, _, cx| {
                    cx.stop_propagation();
                    cx.emit(FilesEvent::ForkChat);
                })),
            )
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_section(
        &mut self,
        section: Section,
        count: usize,
        wanted: f32,
        height: f32,
        actions: Option<AnyElement>,
        body: AnyElement,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open = self.sections.is_open(section, count);
        let empty = count == 0;
        // The collapsed header carries the count; open, the rows speak.
        let label: SharedString = if open || count == 0 {
            section.label().into()
        } else {
            format!("{} ({count})", section.label()).into()
        };
        // Toggling animates between 0 and the height this section gets from
        // the budget (the body scrolls inside it); a closed section reopens
        // to what it wants within the whole budget.
        let full = if open {
            height
        } else {
            wanted.min(FOOTER_HEIGHT - chrome_height()).max(0.0)
        };
        let header = div()
            .id(SharedString::from(format!(
                "files-section-{}",
                section.key()
            )))
            .group(HEADER_GROUP)
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .h(px(SECTION_HEADER_HEIGHT))
            .pl(px(Theme::SPACE_SM))
            .pr(px(4.0))
            .rounded(px(6.0))
            // An empty section has nothing to disclose: no caret, no toggle.
            .when(!empty, |header| {
                header
                    .role(gpui::Role::Button)
                    .aria_label(SharedString::from(format!(
                        "{} {}",
                        if open { "Collapse" } else { "Expand" },
                        section.label()
                    )))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        // Open → close runs from the painted body height to 0,
                        // and back up to what the budget allows.
                        let (resting, target) = if open { (full, 0.0) } else { (0.0, full) };
                        this.sections.animate(section, resting, target);
                        this.sections.painted.insert(section, (!open, target));
                        this.sections.collapsed.set(section, open);
                        cx.emit(FilesEvent::SectionsCollapsedChanged(
                            this.sections.collapsed,
                        ));
                        cx.notify();
                    }))
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(crate::typography::ui_rems(12.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(theme.text_muted.opacity(0.5))
                    .child(label),
            )
            .children(actions)
            .when(!empty, |header| {
                header.child(self.render_chevron(section, open, theme))
            });
        div()
            .flex_none()
            .flex()
            .flex_col()
            .child(header)
            .child(self.render_disclosure_body(section, open, height, body))
            .into_any_element()
    }

    fn render_chevron(&self, section: Section, open: bool, theme: &Theme) -> AnyElement {
        let chevron = icon(icons::ALT_ARROW_RIGHT)
            .size(px(12.0))
            .text_color(theme.text_muted.opacity(0.5));
        let frame = div()
            .flex_none()
            .size(px(20.0))
            .flex()
            .items_center()
            .justify_center();
        if let Some(tween) = self.sections.live_motion(section) {
            let denominator = tween.from.max(tween.to).max(1.0);
            let from = (tween.from / denominator).clamp(0.0, 1.0);
            let to = (tween.to / denominator).clamp(0.0, 1.0);
            frame
                .child(chevron.with_animation(
                    SharedString::from(format!(
                        "files-section-chevron-{}-{}",
                        section.key(),
                        tween.epoch
                    )),
                    collapse_animation(),
                    move |el, t| {
                        let reveal = motion::lerp(from, to, t);
                        el.with_transformation(gpui::Transformation::rotate(gpui::percentage(
                            reveal * 0.25,
                        )))
                    },
                ))
                .into_any_element()
        } else {
            let resting = if open { 0.25 } else { 0.0 };
            frame
                .child(
                    chevron.with_transformation(gpui::Transformation::rotate(gpui::percentage(
                        resting,
                    ))),
                )
                .into_any_element()
        }
    }

    fn render_disclosure_body(
        &self,
        section: Section,
        open: bool,
        height: f32,
        content: AnyElement,
    ) -> AnyElement {
        let target = if open { height } else { 0.0 };
        let frame = div().w_full().flex_none().overflow_hidden().child(content);
        let Some(tween) = self.sections.live_motion(section) else {
            return frame.h(px(target)).into_any_element();
        };
        let denominator = tween.from.max(tween.to).max(1.0);
        frame
            .with_animation(
                SharedString::from(format!(
                    "files-section-body-{}-{}",
                    section.key(),
                    tween.epoch
                )),
                collapse_animation(),
                move |el, t| {
                    let height = motion::lerp(tween.from, tween.to, t);
                    let reveal = (height / denominator).clamp(0.0, 1.0);
                    el.h(px(height))
                        .opacity(0.35 + 0.65 * reveal)
                        .relative()
                        .top(px(-3.0 * (1.0 - reveal)))
                },
            )
            .into_any_element()
    }

    fn render_show_more(
        &self,
        section: Section,
        remaining: usize,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .id(SharedString::from(format!(
                "files-section-{}-more",
                section.key()
            )))
            .role(gpui::Role::Button)
            .flex_none()
            .h(px(ROW_HEIGHT))
            .flex()
            .items_center()
            .px(px(Theme::SPACE_SM))
            .rounded(px(8.0))
            .cursor_pointer()
            .text_size(crate::typography::ui_rems(12.0))
            .text_color(theme.text_muted.opacity(0.7))
            .hover(|s| s.bg(theme.glass_hover()).text_color(theme.text_muted))
            .child(format!("Show {} more", remaining.min(PAGE_ROWS)))
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                let shown = this.sections.shown(section) + PAGE_ROWS;
                this.sections.shown.insert(section, shown);
                cx.notify();
            }))
            .into_any_element()
    }

    fn render_subagent_rows(
        &self,
        rows: &[SubagentRow],
        view: EntityId,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if rows.is_empty() {
            return div().into_any_element();
        }
        let now = Utc::now();
        let shown = self.sections.shown(Section::Subagents);
        let scroll = self.sections.scroll(Section::Subagents);
        let mut list = row_list("files-subagent-rows", &scroll);
        for row in rows.iter().take(shown) {
            let glyph = status_glyph(
                format!("files-subagent-{}", row.doc_id),
                row.indicator(),
                view,
                theme,
                cx,
            );
            let open = row.clone();
            list = list.child(
                compact_row(format!("files-subagent-{}", row.doc_id), theme)
                    .aria_label(SharedString::from(format!("Open subagent {}", row.title)))
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.stop_propagation();
                        cx.emit(FilesEvent::OpenSubagent {
                            doc_id: open.doc_id.clone(),
                            title: open.title.to_string(),
                            frozen: open.frozen(),
                        });
                    }))
                    .child(glyph)
                    .child(row_title(
                        format!("files-subagent-title-{}", row.doc_id),
                        row.title.clone(),
                    ))
                    .child(time_ago_label(
                        zeron_proto::view::format_time_ago(row.spawned_at, now).into(),
                        theme,
                    )),
            );
        }
        if rows.len() > shown {
            list = list.child(self.render_show_more(
                Section::Subagents,
                rows.len() - shown,
                theme,
                cx,
            ));
        }
        faded_list(list, &scroll)
    }

    fn render_chat_rows(
        &self,
        rows: &[ChildChatRow],
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if rows.is_empty() {
            return div().into_any_element();
        }
        let view = cx.entity_id();
        let shown = self.sections.shown(Section::Chats);
        let scroll = self.sections.scroll(Section::Chats);
        let mut list = row_list("files-chat-rows", &scroll);
        for row in rows.iter().take(shown) {
            let glyph = status_glyph(
                format!("files-chat-{}", row.chat_id),
                row.status,
                view,
                theme,
                cx,
            );
            let open_id = row.chat_id.clone();
            let menu_id = row.chat_id.clone();
            let rename_input = self
                .sections
                .chat_rename
                .as_ref()
                .filter(|(id, _)| *id == row.chat_id)
                .map(|(_, input)| input.clone());
            let title = match rename_input {
                Some(input) => crate::shell::chat_title_editor(
                    format!("files-chat-title-editor-{}", row.chat_id).into(),
                    input,
                    theme,
                ),
                None => row_title(
                    format!("files-chat-title-{}", row.chat_id),
                    row.title.clone(),
                )
                .into_any_element(),
            };
            list = list.child(
                compact_row(format!("files-chat-{}", row.chat_id), theme)
                    .aria_label(SharedString::from(format!("Open side chat {}", row.title)))
                    .on_click(cx.listener(move |_, event: &gpui::ClickEvent, _, cx| {
                        cx.stop_propagation();
                        // The first click opened the tab; the second edits
                        // the title in place.
                        if event.click_count() >= 2 {
                            cx.emit(FilesEvent::RenameChildChat(open_id.clone()));
                        } else {
                            cx.emit(FilesEvent::OpenChildChat(open_id.clone()));
                        }
                    }))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |_, event: &gpui::MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            cx.emit(FilesEvent::ChildChatContextMenu {
                                chat_id: menu_id.clone(),
                                position: event.position,
                            });
                        }),
                    )
                    .child(glyph)
                    .child(title)
                    .children(row.change_request.clone().map(|summary| {
                        crate::change_requests::pull_request_badge(
                            format!("files-chat-pr-{}", row.chat_id).into(),
                            summary,
                            crate::change_requests::ChangeRequestBadgeSurface::Sidebar,
                            theme,
                        )
                    }))
                    .child(time_ago_label(row.time_ago.clone(), theme)),
            );
        }
        if rows.len() > shown {
            list = list.child(self.render_show_more(Section::Chats, rows.len() - shown, theme, cx));
        }
        faded_list(list, &scroll)
    }
}

fn collapse_animation() -> Animation {
    motion::COLLAPSE.animation()
}

/// A 20px icon button in a section header, sized to sit beside the caret.
fn header_action(
    id: &'static str,
    icon_path: &'static str,
    label: &'static str,
    theme: &Theme,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .role(gpui::Role::Button)
        .aria_label(label)
        .flex_none()
        .size(px(20.0))
        .rounded(px(5.0))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover(|s| s.bg(crate::theme::wash(0.09)))
        .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
        .tooltip(crate::settings::widgets::text_tooltip(label))
        // Icons take their color on the element itself, never inherited.
        .child(
            icon(icon_path)
                .size(px(13.0))
                .text_color(theme.text_muted.opacity(0.85)),
        )
}

/// The scrolling column an open section's rows live in; the body frame
/// sets the height, the list fills it and reports its overflow through
/// `scroll` for the edge fades.
fn row_list(id: &'static str, scroll: &ScrollHandle) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .size_full()
        .flex()
        .flex_col()
        .gap(px(ROW_GAP))
        .pt(px(SECTION_BODY_INSET))
        .overflow_y_scroll()
        .track_scroll(scroll)
}

/// A section list under the sidebar's overflow fades: rows dissolve at the
/// edges while there is more to scroll to, plain when everything fits.
fn faded_list(list: gpui::Stateful<gpui::Div>, scroll: &ScrollHandle) -> AnyElement {
    crate::edge_fade::edge_faded(
        LIST_FADE_BAND,
        true,
        true,
        div().relative().size_full().child(list),
    )
    .fade_overflow_y(scroll)
    .into_any_element()
}

/// The sidebar's compact session row, stripped to status + title (+ time):
/// 29px, 8px radius, the glass hover wash, 13px title on a 17px line.
fn compact_row(id: String, theme: &Theme) -> gpui::Stateful<gpui::Div> {
    div()
        .id(SharedString::from(id))
        .role(gpui::Role::Button)
        .flex_none()
        .h(px(ROW_HEIGHT))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(4.0))
        .rounded(px(8.0))
        .px(px(Theme::SPACE_SM))
        .cursor_pointer()
        .text_color(theme.text.opacity(0.8))
        .hover(|s| s.bg(theme.glass_hover()).text_color(theme.text))
}

/// The compact row's trailing time, 11px in the faded subline color.
fn time_ago_label(text: SharedString, theme: &Theme) -> gpui::Div {
    div()
        .flex_none()
        .text_size(crate::typography::ui_rems(11.0))
        .text_color(theme.text_muted.opacity(0.5))
        .child(text)
}

/// The sidebar's fading label: overflow dissolves at the right edge instead
/// of an ellipsis.
fn row_title(id: String, title: SharedString) -> impl IntoElement {
    crate::shell::sidebar_faded_label(
        id.into(),
        true,
        div()
            .text_size(crate::typography::ui_rems(13.0))
            .line_height(px(17.0))
            .child(title),
    )
}

/// The compact row's 13px status slot: Working animates the glyph spinner,
/// Completed wears the check, the rest a 6px dot in the status color.
fn status_glyph(
    key: String,
    status: ChatIndicator,
    view: EntityId,
    theme: &Theme,
    cx: &mut gpui::App,
) -> AnyElement {
    let color = crate::shell::spaces::status_dot_color(status, theme);
    let glyph: AnyElement = match status {
        ChatIndicator::Completed => icon(icons::CHECK)
            .size(px(11.0))
            .flex_none()
            .text_color(color)
            .into_any_element(),
        ChatIndicator::Working => {
            loaders::mini_glyph_spinner(format!("{key}-working"), 2.0, theme.glyph, view, cx)
                .into_any_element()
        }
        _ => div()
            .size(px(6.0))
            .flex_none()
            .rounded_full()
            .bg(color)
            .into_any_element(),
    };
    div()
        .flex_none()
        .size(px(13.0))
        .flex()
        .items_center()
        .justify_center()
        .child(glyph)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeron_doc::{MessageRole, MessageStatus, SessionMessageEntry};
    use zeron_proto::ToolCall;

    fn chat(id: &str, parent: Option<&str>, minutes_ago: i64) -> Chat {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "deviceId": "dev",
            "archived": false,
            "createdAt": Utc::now() - chrono::Duration::minutes(minutes_ago),
            "parentChatId": parent,
        }))
        .unwrap()
    }

    fn spawn(
        id: &str,
        name: &str,
        doc: Option<&str>,
        status: Option<SubagentStatus>,
    ) -> MessagePart {
        MessagePart::Tool {
            id: id.into(),
            call: ToolCall::Unknown {
                name: name.into(),
                input: Some(serde_json::json!({ "description": "verify the marker pipeline" })),
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
            subagent_status: status,
            subagent_tail: None,
        }
    }

    fn entry(parts: Vec<MessagePart>) -> SessionMessageEntry {
        SessionMessageEntry {
            duration_ms: None,
            id: "e1".into(),
            role: MessageRole::Assistant,
            parts,
            created_at: (Utc::now() - chrono::Duration::minutes(3)).timestamp_millis(),
            device_id: "dev".into(),
            status: Some(MessageStatus::Complete),
            continuation_of: None,
        }
    }

    #[test]
    fn subagent_rows_list_only_stamped_spawn_chips_of_the_selected_chat() {
        let mut state = AppState::new();
        state.selected_chat = Some("main".into());
        state.transcript = vec![entry(vec![
            spawn(
                "t1",
                "Agent: verify",
                Some("main--sub--t1"),
                Some(SubagentStatus::Running),
            ),
            // No doc ref yet: the engine stamps it asynchronously.
            spawn("t2", "Agent: later", None, None),
            // A stray ref on a non-spawn tool never surfaces.
            spawn(
                "t3",
                "Read",
                Some("main--sub--t3"),
                Some(SubagentStatus::Done),
            ),
            spawn(
                "t4",
                "Agent: done",
                Some("main--sub--t4"),
                Some(SubagentStatus::Done),
            ),
        ])];
        let rows = subagent_rows(&state, "main");
        assert_eq!(
            rows.iter().map(|r| r.doc_id.as_str()).collect::<Vec<_>>(),
            ["main--sub--t1", "main--sub--t4"]
        );
        // The bare task, genus stripped — the same title the tab wears.
        assert_eq!(rows[0].title.as_ref(), "verify");
        assert!(rows[1].frozen());
        assert!(!rows[0].frozen());
        // Spawn time comes from the turn that carried the chip.
        assert!((Utc::now() - rows[0].spawned_at).num_minutes() >= 2);
        // Another chat's explorer sees nothing of this transcript.
        assert!(subagent_rows(&state, "other").is_empty());
    }

    #[test]
    fn subagent_rows_are_most_recently_updated_first() {
        let at = |id: &str, minutes_ago: i64, parts| SessionMessageEntry {
            id: id.into(),
            created_at: (Utc::now() - chrono::Duration::minutes(minutes_ago)).timestamp_millis(),
            ..entry(parts)
        };
        let mut state = AppState::new();
        state.selected_chat = Some("main".into());
        state.transcript = vec![
            at(
                "e1",
                30,
                vec![
                    spawn(
                        "a",
                        "Agent: a",
                        Some("main--sub--a"),
                        Some(SubagentStatus::Done),
                    ),
                    spawn(
                        "b",
                        "Agent: b",
                        Some("main--sub--b"),
                        Some(SubagentStatus::Done),
                    ),
                ],
            ),
            at(
                "e2",
                20,
                vec![spawn(
                    "c",
                    "Agent: c",
                    Some("main--sub--c"),
                    Some(SubagentStatus::Done),
                )],
            ),
            // `a` is steered again: its row moves up with the newer turn.
            at(
                "e3",
                10,
                vec![spawn(
                    "a",
                    "Agent: a",
                    Some("main--sub--a"),
                    Some(SubagentStatus::Running),
                )],
            ),
        ];
        let rows = subagent_rows(&state, "main");
        assert_eq!(
            rows.iter().map(|r| r.doc_id.as_str()).collect::<Vec<_>>(),
            ["main--sub--a", "main--sub--c", "main--sub--b"]
        );
        assert_eq!(rows[0].status, Some(SubagentStatus::Running));
    }

    #[test]
    fn running_subagents_lead_longest_running_first() {
        let at = |id: &str, minutes_ago: i64, parts| SessionMessageEntry {
            id: id.into(),
            created_at: (Utc::now() - chrono::Duration::minutes(minutes_ago)).timestamp_millis(),
            ..entry(parts)
        };
        let sub = |id: &str, status| {
            spawn(
                id,
                &format!("Agent: {id}"),
                Some(&format!("main--sub--{id}")),
                Some(status),
            )
        };
        let mut state = AppState::new();
        state.selected_chat = Some("main".into());
        state.transcript = vec![
            at("e1", 90, vec![sub("old-run", SubagentStatus::Running)]),
            at("e2", 60, vec![sub("old-done", SubagentStatus::Done)]),
            at("e3", 30, vec![sub("mid-run", SubagentStatus::Running)]),
            at("e4", 20, vec![sub("mid-fail", SubagentStatus::Failed)]),
            at("e5", 10, vec![sub("new-done", SubagentStatus::Done)]),
            at("e6", 5, vec![sub("new-run", SubagentStatus::Running)]),
        ];
        let ids: Vec<_> = subagent_rows(&state, "main")
            .into_iter()
            .map(|r| r.doc_id)
            .collect();
        // Running first, longest-running leading; the settled tail keeps the
        // newest-first order it always had.
        assert_eq!(
            ids,
            [
                "main--sub--old-run",
                "main--sub--mid-run",
                "main--sub--new-run",
                "main--sub--new-done",
                "main--sub--mid-fail",
                "main--sub--old-done",
            ]
        );
    }

    #[test]
    fn child_chat_rows_are_live_children_newest_first() {
        let mut state = AppState::new();
        let mut archived = chat("old", Some("main"), 1);
        archived.archived = true;
        let mut titled = chat("b", Some("main"), 30);
        titled.title = Some("Investigate caching".into());
        state.apply_chats(vec![
            chat("main", None, 60),
            chat("a", Some("main"), 5),
            titled,
            chat("unrelated", Some("elsewhere"), 2),
            archived,
        ]);
        let rows = child_chat_rows(&state, "main", Utc::now());
        assert_eq!(
            rows.iter().map(|r| r.chat_id.as_str()).collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert_eq!(rows[0].title.as_ref(), "New side chat");
        assert_eq!(rows[1].title.as_ref(), "Investigate caching");
        assert_eq!(rows[0].status, ChatIndicator::Idle);
    }

    #[gpui::test]
    fn double_clicked_side_chat_asks_for_an_inline_rename(cx: &mut gpui::TestAppContext) {
        use gpui::{AppContext as _, MouseDownEvent, MouseUpEvent};

        let (files, cx) = super::super::test_support::setup(cx);
        files.update(cx, |files, cx| {
            // Notify like any state change, so the footer re-derives its rows.
            files.state.update(cx, |state, cx| {
                let mut side = chat("side", Some("chat"), 1);
                side.title = Some("Side work".into());
                state.chats.push(side);
                cx.notify();
            });
        });
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear();
        });
        // The first row opens the section with its disclosure motion; land
        // it so the row is fully on screen.
        files.update(cx, |files, _| files.sections.motion.clear());
        cx.update(|window, cx| window.draw(cx).clear());
        let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let recorded = events.clone();
        let _sub = cx.update(|_, cx| {
            cx.subscribe(&files, move |_, event, _| {
                recorded.borrow_mut().push(event.clone())
            })
        });
        let row = cx.debug_bounds("files-chat-title-side").unwrap();
        for click_count in [1, 2] {
            cx.simulate_event(MouseDownEvent {
                button: MouseButton::Left,
                position: row.center(),
                click_count,
                ..Default::default()
            });
            cx.simulate_event(MouseUpEvent {
                button: MouseButton::Left,
                position: row.center(),
                click_count,
                ..Default::default()
            });
        }
        cx.run_until_parked();
        {
            let events = events.borrow();
            assert!(matches!(&events[..], [
                FilesEvent::OpenChildChat(open),
                FilesEvent::RenameChildChat(rename),
            ] if open == "side" && rename == "side"));
        }

        // The shell's field replaces the row title while the edit is open.
        let input = cx.update(|_, cx| cx.new(|cx| ComposerInput::new("Session title", cx)));
        files.update(cx, |files, cx| {
            files.set_chat_rename(Some(("side".into(), input)), cx)
        });
        cx.update(|window, cx| window.draw(cx).clear());
        assert!(cx.debug_bounds("files-chat-title-editor-side").is_some());
        assert!(cx.debug_bounds("files-chat-title-side").is_none());
        files.update(cx, |files, cx| files.set_chat_rename(None, cx));
        cx.update(|window, cx| window.draw(cx).clear());
        assert!(cx.debug_bounds("files-chat-title-side").is_some());
    }

    #[gpui::test]
    fn inline_rename_reveals_a_side_chat_in_a_collapsed_paged_section(
        cx: &mut gpui::TestAppContext,
    ) {
        use gpui::AppContext as _;

        let (files, cx) = super::super::test_support::setup(cx);
        files.update(cx, |files, cx| {
            files.state.update(cx, |state, cx| {
                for ix in 0..12 {
                    state
                        .chats
                        .push(chat(&format!("side-{ix:02}"), Some("chat"), ix));
                }
                cx.notify();
            });
            files.sections.collapsed.set(Section::Chats, true);
            assert!(!files.sections.is_open(Section::Chats, 12));
        });
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear();
        });

        // Newest first: side-11 is the twelfth row, past the first page.
        let input = cx.update(|_, cx| cx.new(|cx| ComposerInput::new("Session title", cx)));
        files.update(cx, |files, cx| {
            files.set_chat_rename(Some(("side-11".into(), input)), cx)
        });
        cx.update(|window, cx| window.draw(cx).clear());
        files.read_with(cx, |files, _| {
            assert!(files.sections.is_open(Section::Chats, 12));
            assert_eq!(
                files.sections.shown(Section::Chats),
                INITIAL_ROWS + PAGE_ROWS
            );
            // The reveal became the section's opening motion.
            assert_eq!(files.sections.motion[&Section::Chats].from, 0.0);
        });
        assert!(cx.debug_bounds("files-chat-title-editor-side-11").is_some());
    }

    #[test]
    fn fingerprint_tracks_membership_and_status() {
        let mut state = AppState::new();
        let now = Utc::now();
        let print = |state: &AppState| {
            fingerprint(
                &subagent_rows(state, "main"),
                &child_chat_rows(state, "main", now),
            )
        };
        state.apply_chats(vec![chat("main", None, 60)]);
        let empty = print(&state);
        state.apply_chats(vec![chat("main", None, 60), chat("a", Some("main"), 5)]);
        let one = print(&state);
        assert_ne!(empty, one);
        assert_eq!(one, print(&state));
    }

    #[test]
    fn content_height_pages_at_ten_rows_and_counts_the_show_more_row() {
        // A short list is exactly as tall as its rows.
        let one = content_height(1, INITIAL_ROWS);
        assert_eq!(one, SECTION_BODY_INSET + ROW_HEIGHT);
        let ten = content_height(10, INITIAL_ROWS);
        // Eleven rows: ten visible plus the "Show more" slot.
        let eleven = content_height(11, INITIAL_ROWS);
        assert_eq!(eleven - ten, ROW_HEIGHT + ROW_GAP);
        // Paging once reveals up to 20 rows before the next "Show more".
        let paged = content_height(40, INITIAL_ROWS + PAGE_ROWS);
        assert_eq!(
            paged,
            SECTION_BODY_INSET + 21.0 * ROW_HEIGHT + 20.0 * ROW_GAP
        );
        // An empty section is collapsed and wants nothing.
        assert_eq!(content_height(0, INITIAL_ROWS), 0.0);
    }

    #[test]
    fn empty_sections_stay_collapsed_and_open_with_their_first_row() {
        let mut sections = ExplorerSections::default();
        assert!(!sections.is_open(Section::Chats, 0));
        sections.paint(Section::Chats, false, 0.0);
        assert!(sections.live_motion(Section::Chats).is_none());
        // The first row opens the section with the disclosure motion.
        assert!(sections.is_open(Section::Chats, 1));
        let one_row = content_height(1, INITIAL_ROWS);
        sections.paint(Section::Chats, true, one_row);
        let opening = sections.live_motion(Section::Chats).unwrap();
        assert_eq!((opening.from, opening.to), (0.0, one_row));
        // Repainting the same state starts nothing new.
        sections.paint(Section::Chats, true, one_row);
        assert_eq!(
            sections.live_motion(Section::Chats).unwrap().epoch,
            opening.epoch
        );
        // A collapse preference outlives rows arriving.
        sections.collapsed.set(Section::Subagents, true);
        assert!(!sections.is_open(Section::Subagents, 3));
        assert!(sections.is_open(Section::Chats, 3));
    }

    #[test]
    fn body_budget_shares_the_footer_and_hands_slack_across() {
        // Both fit: each takes what it wants.
        assert_eq!(
            body_budget(300.0, [100.0, 100.0], [true, true]),
            [100.0, 100.0]
        );
        // Closed sections take nothing.
        assert_eq!(
            body_budget(300.0, [100.0, 100.0], [false, true]),
            [0.0, 100.0]
        );
        // Both oversubscribed: an even split.
        assert_eq!(
            body_budget(200.0, [500.0, 500.0], [true, true]),
            [100.0, 100.0]
        );
        // A short second section hands its slack to the first.
        assert_eq!(
            body_budget(200.0, [500.0, 40.0], [true, true]),
            [160.0, 40.0]
        );
        // A short first section hands its slack to the second.
        assert_eq!(
            body_budget(200.0, [40.0, 500.0], [true, true]),
            [40.0, 160.0]
        );
        assert_eq!(body_budget(-5.0, [40.0, 500.0], [true, true]), [0.0, 0.0]);
    }
}
