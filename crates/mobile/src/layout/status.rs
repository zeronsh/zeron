//! The status cards: goal strip, todo strip, workflow run, and the compact
//! marker rows goal / workflow machinery leaves in the transcript. Wording,
//! counts and fold rules come from `zeron_proto::{goal_view, todo_view,
//! workflow_view}` (the desktop's own view models); this file only decides
//! how they sit on a phone: which parts are rows, what a tap does.
//!
//! State a person toggles (open / closed, "show more", which artifact is
//! open) lives in [`UiState`], owned by the row builder and fed back by
//! `TranscriptView::act`. It is in memory only, like the desktop's.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use zeron_markdown::parser::parse_full;
use zeron_proto::goal_view::{self, MarkerKind, Tone as GoalTone};
use zeron_proto::todo_view::{self, TodoPanelState, TodoRow, TodoSummary};
use zeron_proto::workflow_view::{
    self, CardModel, Light, PaneModel, PillState, Tone as WfTone,
};
use zeron_proto::{
    ArtifactKind, Goal, GoalEventKind, GoalStatus, TodoItem, TodoStatus, VerdictOutcome,
    WorkflowEventMarker, WorkflowRun, WorkflowStatus,
};

use super::cards::{Act, Btn, Card, CardStyles, ChipSpec, Glyph, Header, Item, Line};
use super::display::ColorRole;
use super::markdown::{Ctx, prepare_block};

/// Reports listed before "Show all".
pub(crate) const REPORTS_SHOWN: usize = 3;
/// Lines of an artifact a phone fetches and draws (a table page, a file head).
pub(crate) const PREVIEW_LINES: usize = 120;
/// Longest result drawn, collapsed and open.
const RESULT_LINES: usize = 8;
const RESULT_LINES_OPEN: usize = 40;

/// What the artifact an open run shows looks like right now.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ArtifactView {
    Loading,
    Failed(String),
    /// The page, as markdown (`artifact_view::to_markdown`).
    Ready { title: String, markdown: Arc<String>, total: u64, truncated: bool },
}

/// Per-chat presentation state of the cards. In memory, like the desktop's.
#[derive(Debug, Default, Clone)]
pub(crate) struct UiState {
    /// `wf:<run>`, `goal`, `todo` → explicit open choice.
    pub open: HashMap<String, bool>,
    pub actor_pages: HashMap<String, usize>,
    pub reports_all: HashSet<String>,
    pub result_open: HashSet<String>,
    /// run id → artifact id being previewed.
    pub artifact: HashMap<String, String>,
    pub artifacts: HashMap<(String, String), ArtifactView>,
    /// A command the host could not take, shown on the card until the next
    /// success (`run id` / `goal`).
    pub failures: HashMap<String, String>,
    pub todo: TodoPanelState,
}

impl UiState {
    /// A stable fingerprint of what one card's rendering reads from here.
    pub fn run_sig(&self, run_id: &str) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.open.get(&format!("wf:{run_id}")).hash(&mut h);
        self.actor_pages.get(run_id).hash(&mut h);
        self.reports_all.contains(run_id).hash(&mut h);
        self.result_open.contains(run_id).hash(&mut h);
        self.failures.get(run_id).hash(&mut h);
        if let Some(id) = self.artifact.get(run_id) {
            id.hash(&mut h);
            match self.artifacts.get(&(run_id.to_owned(), id.clone())) {
                None => 0u8.hash(&mut h),
                Some(ArtifactView::Loading) => 1u8.hash(&mut h),
                Some(ArtifactView::Failed(m)) => (2u8, m).hash(&mut h),
                Some(ArtifactView::Ready { markdown, .. }) => (3u8, markdown.len(), markdown.as_str()).hash(&mut h),
            }
        }
        h.finish()
    }
}

// MARK: - Colors

fn goal_tone(t: GoalTone) -> ColorRole {
    match t {
        GoalTone::Accent => ColorRole::Accent,
        GoalTone::Success => ColorRole::Success,
        GoalTone::Warning => ColorRole::Warning,
        GoalTone::Muted => ColorRole::TextSecondary,
    }
}

fn wf_tone(t: WfTone) -> ColorRole {
    match t {
        WfTone::Accent => ColorRole::Accent,
        WfTone::Success => ColorRole::Success,
        WfTone::Warning => ColorRole::Warning,
        WfTone::Danger => ColorRole::Danger,
        WfTone::Muted => ColorRole::TextSecondary,
    }
}

fn light_glyph(l: Light) -> Glyph {
    match l {
        Light::Pending => Glyph::Ring(ColorRole::TextTertiary),
        Light::Running => Glyph::Dot(ColorRole::Accent),
        Light::Done => Glyph::Dot(ColorRole::Success),
        Light::Failed => Glyph::Dot(ColorRole::Danger),
        Light::Stopped => Glyph::Dot(ColorRole::TextTertiary),
    }
}

fn pill_glyph(s: PillState) -> Glyph {
    match s {
        PillState::Pending => Glyph::Ring(ColorRole::TextTertiary),
        PillState::Running => Glyph::Spinner,
        PillState::Asking => Glyph::Icon("questionmark.bubble", ColorRole::Warning),
        PillState::Done => Glyph::Icon("checkmark", ColorRole::Success),
        PillState::Failed => Glyph::Icon("xmark", ColorRole::Danger),
        PillState::Cancelled => Glyph::Icon("minus", ColorRole::TextTertiary),
    }
}

fn kind_icon(k: ArtifactKind) -> &'static str {
    match k {
        ArtifactKind::Markdown => "doc.text",
        ArtifactKind::Table => "tablecells",
        ArtifactKind::Metrics => "chart.bar",
        ArtifactKind::File => "doc",
    }
}

fn kind_word(k: ArtifactKind) -> &'static str {
    match k {
        ArtifactKind::Markdown => "document",
        ArtifactKind::Table => "table",
        ArtifactKind::Metrics => "metrics",
        ArtifactKind::File => "file",
    }
}

fn one_line(s: &str) -> String {
    zeron_proto::view::one_line(s)
}

// MARK: - Goal strip

/// The goal's fingerprint for row versions: everything the strip draws.
pub(crate) fn goal_sig(goal: &Goal, now_ms: i64, open: bool, failure: Option<&str>) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (&goal.id, goal.status as u8, goal.iteration, goal.max_rounds, goal.extensions).hash(&mut h);
    (goal.tokens_used, goal.verifier_tokens_used, goal.updated_at, goal.verdicts.len()).hash(&mut h);
    goal_view::elapsed_seconds(goal, now_ms).hash(&mut h);
    goal.objective.hash(&mut h);
    goal.reason.as_ref().map(|r| (&r.message, r.kind as u8)).hash(&mut h);
    open.hash(&mut h);
    failure.hash(&mut h);
    h.finish()
}

pub(crate) fn goal_card(ctx: &mut Ctx, goal: &Goal, now_ms: i64, open: bool, failure: Option<&str>) -> (Card, String) {
    let st = CardStyles::new(ctx);
    let (chip_word, chip_tone) = goal_view::chip(goal);
    let (headline, _) = goal_view::headline(goal);
    let rounds = format!("R{} · {}", goal.iteration.max(1), zeron_proto::view::format_elapsed(goal_view::elapsed_seconds(goal, now_ms) as i64));
    let button = match goal.status {
        GoalStatus::Active | GoalStatus::Verifying => Some(("pause.fill", ColorRole::TextSecondary, Act::new("goal.pause", "Pause goal"))),
        GoalStatus::Paused | GoalStatus::BudgetLimited => Some(("play.fill", ColorRole::Accent, Act::new("goal.resume", "Resume goal"))),
        GoalStatus::Complete => None,
    };
    let mut items = vec![Item::Header(Header {
        icon: Glyph::Icon("target", goal_tone(chip_tone)),
        title: st.title(ctx, "Goal", ColorRole::Text),
        pill: Some(st.tiny(ctx, chip_word, goal_tone(chip_tone))),
        trail: Some(st.one(ctx, &rounds, ColorRole::TextTertiary)),
        sub: Some(st.text(ctx, &one_line(&headline), ColorRole::TextSecondary)),
        sub_lines: if open { 3 } else { 2 },
        button,
        chevron: Some(open),
        act: Some(Act::new("goal.toggle", if open { "Collapse goal" } else { "Expand goal" })),
        spinner: goal.status.is_running(),
    })];
    if open {
        items.push(Item::Space(8.0));
        items.push(Item::Text { text: st.text(ctx, goal.objective.trim(), ColorRole::Text), lines: 10, lead: Glyph::None });
        items.push(Item::Space(6.0));
        items.push(Item::Text { text: st.text(ctx, &goal_view::meta_line(goal, now_ms), ColorRole::TextTertiary), lines: 3, lead: Glyph::None });
        if let Some(v) = goal.verdicts.last() {
            let (word, color) = match v.outcome {
                VerdictOutcome::Pass => ("passed", ColorRole::Success),
                VerdictOutcome::NotSatisfied => ("not satisfied", ColorRole::TextSecondary),
                VerdictOutcome::Failed => ("could not run", ColorRole::Warning),
            };
            let mut text = format!("Verifier, round {}: {word}", v.iteration);
            if !v.reason.trim().is_empty() {
                text.push_str(&format!(" — {}", one_line(&v.reason)));
            }
            items.push(Item::Space(6.0));
            items.push(Item::Text { text: st.text(ctx, &text, color), lines: 4, lead: Glyph::None });
        }
        if let Some(f) = failure {
            items.push(Item::Space(6.0));
            items.push(Item::Text { text: st.text(ctx, &one_line(f), ColorRole::Danger), lines: 3, lead: Glyph::Icon("exclamationmark.triangle", ColorRole::Danger) });
        }
        items.push(Item::Space(10.0));
        let mut buttons = Vec::new();
        match goal.status {
            GoalStatus::Active | GoalStatus::Verifying => {
                buttons.push(Btn { label: st.label(ctx, "Pause", ColorRole::Text), act: Act::new("goal.pause", "Pause goal") });
            }
            GoalStatus::Paused | GoalStatus::BudgetLimited => {
                buttons.push(Btn { label: st.label(ctx, "Resume", ColorRole::Accent), act: Act::new("goal.resume", "Resume goal") });
            }
            GoalStatus::Complete => {}
        }
        buttons.push(Btn { label: st.label(ctx, "Clear", ColorRole::Danger), act: Act::new("goal.clear", "Clear goal") });
        items.push(Item::Buttons(buttons));
    } else if let Some(f) = failure {
        items.push(Item::Space(6.0));
        items.push(Item::Text { text: st.text(ctx, &one_line(f), ColorRole::Danger), lines: 2, lead: Glyph::Icon("exclamationmark.triangle", ColorRole::Danger) });
    }
    let copy = format!("Goal {} · {}", chip_word.to_lowercase(), one_line(&goal.summary_title));
    (Card { items }, copy)
}

// MARK: - Todo strip

pub(crate) fn todo_sig(items: &[TodoItem], panel: &TodoPanelState, working: bool) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for i in items {
        (&i.text, i.status() as u8).hash(&mut h);
    }
    (panel.expanded, panel.show_earlier, panel.show_later, working).hash(&mut h);
    h.finish()
}

/// `working`: a turn is live, so the current item spins (a stopped run shows
/// a still ring instead of looking busy).
pub(crate) fn todo_card(ctx: &mut Ctx, items: &[TodoItem], panel: &TodoPanelState, working: bool) -> (Card, String) {
    let st = CardStyles::new(ctx);
    let summary = TodoSummary::of(items);
    let finished = summary.finished();
    let open = panel.is_expanded(finished);
    let title = if finished {
        format!("Todo · {}/{} · All done", summary.done, summary.total)
    } else {
        format!("Todo · {}/{}", summary.done, summary.total)
    };
    let headline = summary.headline().map(|ix| one_line(&items[ix].text));
    let mut out = vec![Item::Header(Header {
        icon: Glyph::Icon("tool-checklist", ColorRole::TextSecondary),
        title: st.title(ctx, &title, ColorRole::Text),
        pill: None,
        trail: None,
        sub: (!open).then_some(headline.clone()).flatten().map(|h| st.one(ctx, &h, ColorRole::TextSecondary)),
        sub_lines: 1,
        button: finished.then(|| ("xmark", ColorRole::TextTertiary, Act::new("todo.dismiss", "Dismiss the list"))),
        chevron: Some(open),
        act: Some(Act::new("todo.toggle", if open { "Collapse the list" } else { "Expand the list" })),
        spinner: false,
    })];
    if open {
        out.push(Item::Space(4.0));
        for row in todo_view::rows(items, panel.show_earlier, panel.show_later) {
            match row {
                TodoRow::Item(ix) => {
                    let item = &items[ix];
                    let (glyph, color) = match item.status() {
                        TodoStatus::Completed => (Glyph::Icon("checkmark", ColorRole::Success), ColorRole::TextTertiary),
                        TodoStatus::InProgress if working => (Glyph::Spinner, ColorRole::Text),
                        TodoStatus::InProgress => (Glyph::Ring(ColorRole::Accent), ColorRole::Text),
                        TodoStatus::Pending => (Glyph::Ring(ColorRole::TextTertiary), ColorRole::TextSecondary),
                    };
                    out.push(Item::Line(Line {
                        glyph,
                        title: st.text(ctx, &one_line(&item.text), color),
                        title_lines: 2,
                        trail: None,
                        sub: None,
                        sub_lines: 0,
                        act: None,
                        indent: 0.0,
                    }));
                }
                TodoRow::Fold { side, count, open } => {
                    let (word, payload) = match side {
                        todo_view::FoldSide::Earlier => ("earlier", "todo.earlier"),
                        todo_view::FoldSide::Later => ("later", "todo.later"),
                    };
                    let label = if open { format!("Hide {count} {word}") } else { format!("{count} {word}") };
                    out.push(Item::Line(Line {
                        glyph: Glyph::Icon(if open { "chevron.up" } else { "chevron.down" }, ColorRole::TextTertiary),
                        title: st.one(ctx, &label, ColorRole::TextTertiary),
                        title_lines: 1,
                        trail: None,
                        sub: None,
                        sub_lines: 0,
                        act: Some(Act::new(payload, label.clone())),
                        indent: 0.0,
                    }));
                }
            }
        }
    }
    let copy = match headline {
        Some(h) => format!("{title} · {h}"),
        None => title,
    };
    (Card { items: out }, copy)
}

// MARK: - Workflow run

/// Everything a run card draws that is not in the run itself.
pub(crate) struct RunContext<'a> {
    pub ui: &'a UiState,
    pub open: bool,
}

pub(crate) fn workflow_card(ctx: &mut Ctx, run: &WorkflowRun, rc: &RunContext) -> (Card, String) {
    let st = CardStyles::new(ctx);
    let run_id = run.header.run_id.as_str();
    let pages = rc.ui.actor_pages.get(run_id).copied().unwrap_or(1);
    let pane = PaneModel::build(run, &|_| 0, pages);
    let card = &pane.card;
    let tone = wf_tone(card.tone);
    let open = rc.open;

    let mut sub = card.counts.clone();
    let failed = pane.actors.iter().filter(|a| a.state == PillState::Failed).count();
    if failed > 0 {
        sub.push_str(&format!(" · {failed} failed"));
    }
    let title = format!("{} · {}", card.kind_word, card.name);
    let mut items = vec![Item::Header(Header {
        icon: Glyph::Icon("arrow.triangle.branch", tone),
        title: st.title(ctx, &title, ColorRole::Text),
        pill: None,
        trail: None,
        sub: (!sub.is_empty()).then(|| st.one(ctx, &sub, ColorRole::TextSecondary)),
        sub_lines: 1,
        button: None,
        chevron: Some(open),
        act: Some(Act::new(format!("wf.toggle:{run_id}"), if open { "Collapse the workflow" } else { "Expand the workflow" })),
        spinner: card.status == WorkflowStatus::Running,
    })];

    // The phase rail.
    if !card.stations.is_empty() {
        items.push(Item::Space(8.0));
        let chips = card
            .stations
            .iter()
            .map(|s| {
                let name = if s.base.parallel_with_prev { format!("∥ {}", s.base.name) } else { s.base.name.clone() };
                ChipSpec {
                    lead: light_glyph(s.base.light),
                    label: st.label(ctx, &name, ColorRole::Text),
                    trail: s.base.fraction().map(|f| st.tiny(ctx, &f, ColorRole::TextTertiary)),
                    act: None,
                    selected: false,
                }
            })
            .collect();
        items.push(Item::Chips(chips));
    }

    // Waiting for the person: visible even when the card is folded.
    if card.questions > 0 && !open {
        items.push(Item::Space(8.0));
        let word = if card.questions == 1 { "1 agent is waiting for your answer" } else { &format!("{} agents are waiting for your answer", card.questions) };
        items.push(Item::Text { text: st.text(ctx, word, ColorRole::Warning), lines: 2, lead: Glyph::Icon("questionmark.bubble", ColorRole::Warning) });
    }
    for n in &card.notices {
        items.push(Item::Space(6.0));
        let (lead, color) = match n.tone {
            WfTone::Danger => (Glyph::Icon("exclamationmark.triangle", ColorRole::Danger), ColorRole::Danger),
            WfTone::Warning => (Glyph::Icon("exclamationmark.triangle", ColorRole::Warning), ColorRole::TextSecondary),
            _ => (Glyph::None, ColorRole::TextTertiary),
        };
        items.push(Item::Text { text: st.text(ctx, &n.text, color), lines: 3, lead });
    }
    if let Some(f) = rc.ui.failures.get(run_id) {
        items.push(Item::Space(6.0));
        items.push(Item::Text { text: st.text(ctx, &one_line(f), ColorRole::Danger), lines: 3, lead: Glyph::Icon("exclamationmark.triangle", ColorRole::Danger) });
    }

    if !open {
        // Folded: artifact capsules (they open the card on the document).
        if !card.chips.is_empty() {
            items.push(Item::Space(8.0));
            let mut chips: Vec<ChipSpec> = card
                .chips
                .iter()
                .map(|c| ChipSpec {
                    lead: Glyph::Icon(kind_icon(c.kind), ColorRole::TextSecondary),
                    label: st.label(ctx, &c.label, ColorRole::Text),
                    trail: None,
                    act: Some(Act::new(format!("wf.artifact:{run_id}:{}", c.id), format!("Open {}", c.title))),
                    selected: false,
                })
                .collect();
            if card.chips_more > 0 {
                chips.push(ChipSpec {
                    lead: Glyph::None,
                    label: st.label(ctx, &format!("+{}", card.chips_more), ColorRole::TextSecondary),
                    trail: None,
                    act: Some(Act::new(format!("wf.toggle:{run_id}"), "Show all artifacts")),
                    selected: false,
                });
            }
            items.push(Item::Chips(chips));
        }
        let copy = card.summary();
        return (Card { items }, copy);
    }

    // Open: what the run is waiting on, who is doing what, what it made.
    for q in pane.questions.iter().take(3) {
        items.push(Item::Space(10.0));
        let text = format!("{}: {}", q.actor, one_line(&q.question));
        items.push(Item::Text { text: st.text(ctx, &text, ColorRole::Text), lines: 4, lead: Glyph::Icon("questionmark.bubble", ColorRole::Warning) });
        if !q.context.trim().is_empty() {
            items.push(Item::Space(2.0));
            items.push(Item::Text { text: st.text(ctx, &zeron_proto::truncate_chars(one_line(&q.context).as_str(), 160), ColorRole::TextTertiary), lines: 3, lead: Glyph::None });
        }
    }
    if !pane.questions.is_empty() {
        items.push(Item::Space(4.0));
        items.push(Item::Text { text: st.text(ctx, "Answer in the box at the bottom of the chat.", ColorRole::TextTertiary), lines: 2, lead: Glyph::None });
    }

    if pane.actors_total > 0 {
        items.push(Item::Space(12.0));
        items.push(Item::Text { text: st.label(ctx, &format!("Agents · {}", pane.actors_total), ColorRole::TextSecondary), lines: 1, lead: Glyph::None });
        for a in &pane.actors {
            let act = a
                .child_chat_id
                .as_ref()
                .map(|id| Act::new(format!("chat:{id}"), format!("Open {}'s chat", a.name)));
            items.push(Item::Line(Line {
                glyph: pill_glyph(a.state),
                title: st.label(ctx, &a.name, ColorRole::Text),
                title_lines: 1,
                trail: Some(st.one(ctx, a.state.word(), ColorRole::TextSecondary)),
                sub: a.activity.as_ref().map(|t| st.one(ctx, t, ColorRole::TextFaint)),
                sub_lines: 1,
                act,
                indent: 0.0,
            }));
        }
        let shown = pane.actors.len() as u32;
        if shown < pane.actors_total {
            let more = pane.actors_total - shown;
            let label = format!("Show {} more", more.min(workflow_view::PANE_ACTORS_PER_PAGE as u32));
            items.push(Item::Line(Line {
                glyph: Glyph::Icon("chevron.down", ColorRole::TextTertiary),
                title: st.one(ctx, &label, ColorRole::TextTertiary),
                title_lines: 1,
                trail: Some(st.one(ctx, &format!("{more} not shown"), ColorRole::TextFaint)),
                sub: None,
                sub_lines: 0,
                act: Some(Act::new(format!("wf.actors:{run_id}"), label.clone())),
                indent: 0.0,
            }));
        }
    }

    if !pane.artifacts.is_empty() {
        items.push(Item::Space(12.0));
        items.push(Item::Text { text: st.label(ctx, &format!("Artifacts · {}", pane.artifacts.len()), ColorRole::TextSecondary), lines: 1, lead: Glyph::None });
        let open_id = rc.ui.artifact.get(run_id);
        for a in &pane.artifacts {
            let selected = open_id == Some(&a.chip.id);
            items.push(Item::Line(Line {
                glyph: Glyph::Icon(kind_icon(a.chip.kind), if selected { ColorRole::Accent } else { ColorRole::TextSecondary }),
                title: st.label(ctx, &a.chip.title, ColorRole::Text),
                title_lines: 1,
                trail: Some(st.one(ctx, &format!("{} · {}", kind_word(a.chip.kind), workflow_view::format_bytes(a.bytes)), ColorRole::TextTertiary)),
                sub: None,
                sub_lines: 0,
                act: Some(Act::new(format!("wf.artifact:{run_id}:{}", a.chip.id), format!("{} {}", if selected { "Hide" } else { "Open" }, a.chip.title))),
                indent: 0.0,
            }));
            if selected {
                match rc.ui.artifacts.get(&(run_id.to_owned(), a.chip.id.clone())) {
                    None | Some(ArtifactView::Loading) => {
                        items.push(Item::Text { text: st.text(ctx, "Loading…", ColorRole::TextTertiary), lines: 1, lead: Glyph::Spinner });
                    }
                    Some(ArtifactView::Failed(msg)) => {
                        items.push(Item::Text { text: st.text(ctx, &one_line(msg), ColorRole::Danger), lines: 3, lead: Glyph::Icon("exclamationmark.triangle", ColorRole::Danger) });
                        items.push(Item::Line(Line {
                            glyph: Glyph::Icon("arrow.clockwise", ColorRole::Accent),
                            title: st.label(ctx, "Try again", ColorRole::Accent),
                            title_lines: 1,
                            trail: None,
                            sub: None,
                            sub_lines: 0,
                            act: Some(Act::new(format!("wf.artifact-retry:{run_id}:{}", a.chip.id), "Try loading the artifact again")),
                            indent: 0.0,
                        }));
                    }
                    Some(ArtifactView::Ready { markdown, total, truncated, .. }) => {
                        items.push(Item::Space(4.0));
                        let tree = parse_full(markdown);
                        let blocks = tree.blocks.iter().map(|b| prepare_block(ctx, &b.block, 0, false)).collect();
                        items.push(Item::Blocks(blocks));
                        if *truncated {
                            items.push(Item::Space(4.0));
                            items.push(Item::Text {
                                text: st.text(ctx, &format!("Showing the first part of {}.", workflow_view::format_bytes(*total)), ColorRole::TextTertiary),
                                lines: 2,
                                lead: Glyph::None,
                            });
                        }
                    }
                }
                items.push(Item::Space(4.0));
            }
        }
    }

    if let Some(result) = &pane.result_preview {
        items.push(Item::Space(12.0));
        items.push(Item::Text { text: st.label(ctx, "Result", ColorRole::TextSecondary), lines: 1, lead: Glyph::None });
        let open_result = rc.ui.result_open.contains(run_id);
        let text = zeron_proto::truncate_chars(result.trim(), if open_result { 3000 } else { 900 });
        let long = result.trim().chars().count() > 280 || result.trim().lines().count() > RESULT_LINES;
        items.push(Item::Text { text: st.text(ctx, &text, ColorRole::Text), lines: if open_result { RESULT_LINES_OPEN } else { RESULT_LINES }, lead: Glyph::None });
        if long || pane.result_truncated {
            let label = if open_result { "Show less" } else { "Show more" };
            items.push(Item::Line(Line {
                glyph: Glyph::Icon(if open_result { "chevron.up" } else { "chevron.down" }, ColorRole::TextTertiary),
                title: st.one(ctx, label, ColorRole::TextTertiary),
                title_lines: 1,
                trail: None,
                sub: None,
                sub_lines: 0,
                act: Some(Act::new(format!("wf.result:{run_id}"), label)),
                indent: 0.0,
            }));
        }
    }

    if !pane.reports.is_empty() {
        items.push(Item::Space(12.0));
        items.push(Item::Text { text: st.label(ctx, &format!("Reports · {}", pane.reports.len()), ColorRole::TextSecondary), lines: 1, lead: Glyph::None });
        let all = rc.ui.reports_all.contains(run_id);
        let shown = if all { pane.reports.len() } else { pane.reports.len().min(REPORTS_SHOWN) };
        for r in pane.reports.iter().take(shown) {
            items.push(Item::Space(4.0));
            items.push(Item::Text { text: st.text(ctx, &one_line(&r.text), ColorRole::TextSecondary), lines: if all { 6 } else { 3 }, lead: Glyph::None });
        }
        if pane.reports.len() > REPORTS_SHOWN {
            let label = if all { "Show fewer reports".to_owned() } else { format!("Show {} more", pane.reports.len() - shown) };
            items.push(Item::Line(Line {
                glyph: Glyph::Icon(if all { "chevron.up" } else { "chevron.down" }, ColorRole::TextTertiary),
                title: st.one(ctx, &label, ColorRole::TextTertiary),
                title_lines: 1,
                trail: None,
                sub: None,
                sub_lines: 0,
                act: Some(Act::new(format!("wf.reports:{run_id}"), label.clone())),
                indent: 0.0,
            }));
        }
    }

    let mut meta = Vec::new();
    if !card.meta.is_empty() {
        meta.push(card.meta.clone());
    }
    if let Some(c) = &pane.concurrency
        && card.status == WorkflowStatus::Running
    {
        meta.push(c.clone());
    }
    for (i, line) in meta.iter().enumerate() {
        items.push(Item::Space(if i == 0 { 12.0 } else { 2.0 }));
        items.push(Item::Text { text: st.text(ctx, line, ColorRole::TextTertiary), lines: 2, lead: Glyph::None });
    }

    if card.can_stop || card.can_resume {
        items.push(Item::Space(12.0));
        let btn = if card.can_stop {
            Btn { label: st.label(ctx, "Stop workflow", ColorRole::Danger), act: Act::new(format!("wf.stop:{run_id}"), "Stop the workflow") }
        } else {
            Btn { label: st.label(ctx, "Resume workflow", ColorRole::Accent), act: Act::new(format!("wf.resume:{run_id}"), "Resume the workflow") }
        };
        items.push(Item::Buttons(vec![btn]));
    }
    (Card { items }, card.summary())
}

/// Whether a run's card starts open. Folded: an open run is a screenful, and
/// what needs the person (a waiting agent, a failure) shows on the folded card.
pub(crate) fn default_open(_newest: bool) -> bool {
    false
}

/// A card's version is what it draws: the model, the open choice and the
/// local UI state, so a row is remeasured exactly when one of them changed.
pub(crate) fn run_sig(run: &WorkflowRun, ui: &UiState, open: bool) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let pages = ui.actor_pages.get(&run.header.run_id).copied().unwrap_or(1);
    CardModel::build(run, false).fingerprint(open).hash(&mut h);
    // Actors and reports are not part of the card model but are drawn open.
    if open {
        run.actors.iter().map(|a| (&a.name, a.status as u8, a.asks, a.failed_asks)).collect::<Vec<_>>().hash(&mut h);
        run.nodes
            .iter()
            .map(|n| (n.order, n.phase as u8, n.outcome.map(|o| o as u8), n.turn, n.tool_calls, n.last_tool.as_deref()))
            .collect::<Vec<_>>()
            .hash(&mut h);
        run.reports.iter().map(|r| (r.index, r.text.len())).collect::<Vec<_>>().hash(&mut h);
        run.artifacts.iter().map(|a| (&a.id, a.version, a.bytes)).collect::<Vec<_>>().hash(&mut h);
        run.pending_questions.iter().map(|q| (&q.qid, q.question.len())).collect::<Vec<_>>().hash(&mut h);
        run.header.result_preview.hash(&mut h);
        pages.hash(&mut h);
    }
    ui.run_sig(&run.header.run_id).hash(&mut h);
    h.finish()
}

// MARK: - Markers

/// How a marker row reads: icon, tone, one label and an optional detail line.
pub(crate) struct MarkerLook {
    pub icon: &'static str,
    pub color: ColorRole,
    pub label: String,
    pub detail: Option<String>,
}

pub(crate) fn marker_look(marker: &goal_view::GoalMarker) -> MarkerLook {
    let icon = match marker.kind {
        MarkerKind::Workflow(WorkflowEventMarker::Completed) | MarkerKind::WorkflowMessage(WorkflowStatus::Completed) => "checkmark.circle",
        MarkerKind::Workflow(WorkflowEventMarker::Errored | WorkflowEventMarker::Denied) | MarkerKind::WorkflowMessage(WorkflowStatus::Errored) => "exclamationmark.triangle",
        MarkerKind::Workflow(WorkflowEventMarker::Stopped) | MarkerKind::WorkflowMessage(WorkflowStatus::Stopped) => "pause.circle",
        MarkerKind::Workflow(_) | MarkerKind::WorkflowMessage(_) => "arrow.triangle.branch",
        MarkerKind::Event(GoalEventKind::Complete) => "checkmark.circle",
        MarkerKind::Event(GoalEventKind::Paused) => "pause.circle",
        MarkerKind::Event(GoalEventKind::BudgetLimited | GoalEventKind::VerifierFailed) => "exclamationmark.triangle",
        MarkerKind::Event(GoalEventKind::NotSatisfied) => "arrow.right.circle",
        _ => "target",
    };
    MarkerLook { icon, color: goal_tone(marker.tone()), label: marker.label(), detail: marker.detail_line() }
}
