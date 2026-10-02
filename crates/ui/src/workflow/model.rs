//! The workflow card's view-model: what a run looks like, as plain data.
//!
//! Everything here is a pure function of `zeron_proto` workflow state — no
//! gpui, no clock beyond what the caller passes — so the card, the run pane,
//! the sidebar lines and the tests all read the same numbers. The shape
//! follows ZCode's "stations" idea (a rail of phases, each with a settled /
//! observed fraction and the agents that worked in it), rebuilt over our
//! state: the static [`WorkflowGraph`] gives the skeleton, the run header's
//! phase progress and the node list overlay live state on it.
//!
//! Two fidelity levels share one derivation:
//!
//! * [`phase_rail`] needs only the header (what `WatchWorkflowActivity`
//!   sends for chats that are not open): lights and fractions.
//! * [`CardModel::build`] adds the actors and nodes: pills per station.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use zeron_proto::{
    ActorStatus, ArtifactKind, NodeKind, NodeOutcome, NodePhase, WorkflowActor, WorkflowGraph,
    WorkflowNode, WorkflowRun, WorkflowRunBrief, WorkflowRunHeader, WorkflowStatus,
    WorkflowStopReason,
};

/// Agent pills shown per phase column before the rest fold into "+n more".
pub const PILLS_PER_STATION: usize = 6;
/// Artifact chips on the card before "+N".
pub const CARD_CHIPS: usize = 3;
/// Characters of an artifact title on a chip (the tooltip carries it whole).
pub const CHIP_TITLE_CHARS: usize = 24;
/// Run lines per chat row in the sidebar.
pub const SIDEBAR_LINES: usize = 2;
/// Stations the sidebar's mini rail draws before it folds to a window.
pub const SIDEBAR_RAIL_STATIONS: usize = 6;
/// The station a script without `phase()` markers gets.
pub const IMPLICIT_PHASE: &str = "Workflow";

// ── lights ────────────────────────────────────────────────────────────────

/// How a phase (or a pill) reads at a glance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Light {
    /// Not reached (or reached but nothing observed yet).
    Pending,
    Running,
    Done,
    /// Work failed here, or the run died here.
    Failed,
    /// The user stopped the run while this phase was current.
    Stopped,
}

/// One column of the phase rail.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StationBase {
    pub name: String,
    pub light: Light,
    pub settled: u32,
    pub observed: u32,
    /// The run is live and this is where control flow is.
    pub current: bool,
    /// Runs alongside the previous station (their work overlapped).
    pub parallel_with_prev: bool,
}

impl StationBase {
    /// `3/5`, or nothing while no node was observed.
    pub fn fraction(&self) -> Option<String> {
        (self.observed > 0).then(|| format!("{}/{}", self.settled, self.observed))
    }
}

fn is_live(status: WorkflowStatus) -> bool {
    !status.is_settled()
}

/// Names of the phases in script order: the graph's, then anything the run
/// reported that the graph does not know (a dynamic phase name).
fn phase_names(header: &WorkflowRunHeader, graph: Option<&WorkflowGraph>) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let mut push = |name: &str| {
        if !names.iter().any(|n| n == name) {
            names.push(name.to_owned());
        }
    };
    if let Some(graph) = graph {
        graph.phases.iter().for_each(|p| push(&p.name));
    }
    header.phase_names.iter().for_each(|n| push(n));
    header.phases.iter().for_each(|p| push(&p.name));
    if let Some(current) = &header.current_phase {
        push(current);
    }
    names
}

/// The rail of a run from its header alone: phase names in script order with
/// a light and a settled/observed fraction each. A script without phases
/// yields no stations (the caller draws an implicit one if it wants to).
pub fn phase_rail(header: &WorkflowRunHeader, graph: Option<&WorkflowGraph>) -> Vec<StationBase> {
    let names = phase_names(header, graph);
    let live = is_live(header.status);
    let current_ix = header
        .current_phase
        .as_ref()
        .and_then(|c| names.iter().position(|n| n == c));
    names
        .iter()
        .enumerate()
        .map(|(ix, name)| {
            let progress = header.phases.iter().find(|p| p.name == *name);
            let (observed, settled) = progress.map_or((0, 0), |p| (p.observed, p.settled));
            let is_current = current_ix == Some(ix);
            let in_flight = observed > settled;
            let light = if live
                && (in_flight || (is_current && header.status == WorkflowStatus::Running))
            {
                Light::Running
            } else if !live && is_current && header.status != WorkflowStatus::Completed {
                // The run ended while control flow was here: that is where it
                // broke (or where the user stopped it).
                match (header.status, header.stop_reason) {
                    (WorkflowStatus::Stopped, Some(WorkflowStopReason::User)) => Light::Stopped,
                    _ => Light::Failed,
                }
            } else if observed > 0 && settled >= observed {
                Light::Done
            } else if current_ix.is_some_and(|c| ix < c) {
                // Control flow moved past it.
                Light::Done
            } else if observed > 0 {
                // Work seen, none in flight, run over: what settled settled.
                if live { Light::Running } else { Light::Done }
            } else {
                Light::Pending
            };
            let parallel_with_prev = ix > 0
                && header.phase_alongside.iter().any(|group| {
                    group.iter().any(|g| g == name) && group.iter().any(|g| *g == names[ix - 1])
                });
            StationBase {
                name: name.clone(),
                light,
                settled,
                observed,
                current: live && is_current,
                parallel_with_prev,
            }
        })
        .collect()
}

// ── pills ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PillState {
    /// Created or queued, nothing running yet.
    Pending,
    Running,
    /// Parked on an escalation question.
    Asking,
    Done,
    Failed,
    /// Its work was cancelled (the run was stopped).
    Cancelled,
}

impl PillState {
    pub fn is_active(self) -> bool {
        matches!(self, Self::Running | Self::Asking)
    }

    pub fn word(self) -> &'static str {
        match self {
            Self::Pending => "waiting",
            Self::Running => "working",
            Self::Asking => "asking a question",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

/// An agent (or a shell gate) as a pill under a phase column.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Pill {
    /// Stable across updates: the actor's entry key (or `shell:<phase>`).
    pub key: String,
    pub name: String,
    pub state: PillState,
    /// Asks (or commands) this pill ran in the phase.
    pub nodes: u32,
    /// The hidden child chat, once it exists; `None` for shell pills.
    pub child_chat_id: Option<String>,
    /// `claude / sonnet` style label for tooltips.
    pub model_label: Option<String>,
    pub is_shell: bool,
}

/// How a set of nodes of one actor reads.
fn pill_state<'a>(nodes: impl Iterator<Item = &'a WorkflowNode>) -> PillState {
    let mut any = false;
    let mut running = false;
    let mut asking = false;
    let mut queued = false;
    let mut failed = false;
    let mut cancelled = false;
    for n in nodes {
        any = true;
        match n.phase {
            NodePhase::Settled => match n.outcome {
                Some(NodeOutcome::Failed) => failed = true,
                Some(NodeOutcome::Cancelled) => cancelled = true,
                _ => {}
            },
            NodePhase::Queued => queued = true,
            NodePhase::Waiting => asking = true,
            _ => running = true,
        }
    }
    if !any {
        PillState::Pending
    } else if asking {
        PillState::Asking
    } else if running {
        PillState::Running
    } else if queued {
        PillState::Pending
    } else if failed {
        PillState::Failed
    } else if cancelled {
        PillState::Cancelled
    } else {
        PillState::Done
    }
}

fn model_label(actor: &WorkflowActor) -> Option<String> {
    match (&actor.harness, &actor.model) {
        (Some(h), Some(m)) => Some(format!("{h} · {m}")),
        (Some(x), None) | (None, Some(x)) => Some(x.clone()),
        (None, None) => None,
    }
}

/// The program a command node runs (`cargo test --all` → `cargo`).
fn program_of(head: &str) -> String {
    let first = head.split_whitespace().next().unwrap_or("shell");
    first.rsplit('/').next().unwrap_or(first).to_owned()
}

/// Pills of one phase: one per actor that has nodes in it, in actor creation
/// order, plus one aggregate pill for shell gates.
fn station_pills(run: &WorkflowRun, phase: &str, implicit: bool) -> Vec<Pill> {
    let in_phase = |n: &&WorkflowNode| match n.phase_name.as_deref() {
        Some(p) => p == phase,
        None => implicit,
    };
    let mut by_actor: HashMap<(&str, u32), Vec<&WorkflowNode>> = HashMap::new();
    let mut shell: Vec<&WorkflowNode> = Vec::new();
    for n in run.nodes.iter().filter(in_phase) {
        match (&n.actor_site_id, n.kind) {
            (Some(site), NodeKind::Ask) => by_actor
                .entry((site.as_str(), n.actor_ordinal))
                .or_default()
                .push(n),
            _ => shell.push(n),
        }
    }
    let mut pills: Vec<(u32, Pill)> = Vec::new();
    for actor in &run.actors {
        let nodes = by_actor.remove(&(actor.site_id.as_str(), actor.ordinal));
        // An actor with no node yet shows where it was created.
        let belongs = nodes.is_some()
            || (actor.asks == 0 && actor.phase_name.as_deref() == Some(phase))
            || (implicit && actor.phase_name.is_none() && actor.asks == 0);
        if !belongs {
            continue;
        }
        let state = match &nodes {
            Some(nodes) => pill_state(nodes.iter().copied()),
            None => PillState::Pending,
        };
        pills.push((
            actor.order,
            Pill {
                key: actor.key(),
                name: actor.name.clone(),
                state,
                nodes: nodes.map_or(0, |n| n.len() as u32),
                child_chat_id: actor.child_chat_id.clone(),
                model_label: model_label(actor),
                is_shell: false,
            },
        ));
    }
    pills.sort_by_key(|(order, _)| *order);
    let mut out: Vec<Pill> = pills.into_iter().map(|(_, p)| p).collect();
    if !shell.is_empty() {
        let programs: Vec<String> = shell
            .iter()
            .map(|n| program_of(&n.instructions_head))
            .collect();
        let name = if programs.iter().all(|p| *p == programs[0]) && shell.len() == 1 {
            programs[0].clone()
        } else if programs.iter().all(|p| *p == programs[0]) {
            format!("{} ×{}", programs[0], shell.len())
        } else {
            format!("shell ×{}", shell.len())
        };
        out.push(Pill {
            key: format!("shell:{phase}"),
            name,
            state: pill_state(shell.iter().copied()),
            nodes: shell.len() as u32,
            child_chat_id: None,
            model_label: None,
            is_shell: true,
        });
    }
    out
}

/// Which pills to draw when a station has more than `cap`: the active and
/// failed ones must not hide behind "+n more", so they take priority, the
/// rest fill by creation order, and what is drawn keeps creation order.
pub fn fold_pills(pills: Vec<Pill>, cap: usize) -> (Vec<Pill>, u32, u32) {
    if pills.len() <= cap {
        return (pills, 0, 0);
    }
    let rank = |p: &Pill| match p.state {
        PillState::Running | PillState::Asking => 0,
        PillState::Failed => 1,
        PillState::Pending => 2,
        PillState::Done | PillState::Cancelled => 3,
    };
    let mut order: Vec<usize> = (0..pills.len()).collect();
    order.sort_by_key(|&i| (rank(&pills[i]), i));
    let mut keep: Vec<usize> = order.into_iter().take(cap).collect();
    keep.sort_unstable();
    let hidden: Vec<&Pill> = pills
        .iter()
        .enumerate()
        .filter(|(i, _)| !keep.contains(i))
        .map(|(_, p)| p)
        .collect();
    let hidden_active = hidden.iter().filter(|p| p.state.is_active()).count() as u32;
    let hidden_count = hidden.len() as u32;
    let shown = keep.into_iter().map(|i| pills[i].clone()).collect();
    (shown, hidden_count, hidden_active)
}

// ── stations ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Station {
    pub base: StationBase,
    /// Drawn pills (empty while the card is collapsed).
    pub pills: Vec<Pill>,
    /// Pills the cap folded away, and how many of them are working.
    pub hidden: u32,
    pub hidden_active: u32,
    /// Every pill of the phase, before the cap (the run pane lists them all).
    pub total_pills: u32,
}

// ── chips ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Chip {
    pub id: String,
    /// Truncated for the chip.
    pub label: String,
    /// The whole title, for the tooltip.
    pub title: String,
    pub kind: ArtifactKind,
    pub version: u32,
}

/// `Quarterly revenue by region and segme…` — by characters, never in the
/// middle of one.
pub fn truncate_title(title: &str, max: usize) -> String {
    let flat = title.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let mut out: String = flat.chars().take(max.saturating_sub(1)).collect();
    while out.ends_with(' ') {
        out.pop();
    }
    out.push('…');
    out
}

/// Artifact chips in display order: the primary one (the one `report()` last
/// named) first, then documents, tables, metrics and files, each by id. (The
/// synced list is keyed by id and carries no publish time, so "as published"
/// is not available; a document is what a reader opens first.)
pub fn chips(run: &WorkflowRun) -> Vec<Chip> {
    let mut list: Vec<&zeron_proto::ArtifactSummary> = run.artifacts.iter().collect();
    let kind_rank = |k: ArtifactKind| match k {
        ArtifactKind::Markdown => 0,
        ArtifactKind::Table => 1,
        ArtifactKind::Metrics => 2,
        ArtifactKind::File => 3,
    };
    list.sort_by(|a, b| {
        (!a.primary, kind_rank(a.kind), &a.id).cmp(&(!b.primary, kind_rank(b.kind), &b.id))
    });
    list.into_iter()
        .map(|a| Chip {
            id: a.id.clone(),
            label: truncate_title(&a.title, CHIP_TITLE_CHARS),
            title: a.title.clone(),
            kind: a.kind,
            version: a.version,
        })
        .collect()
}

// ── the card ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tone {
    Accent,
    Success,
    Warning,
    Danger,
    Muted,
}

/// A line under the rail that explains something unusual.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Notice {
    pub tone: Tone,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Hash)]
pub struct CardModel {
    pub run_id: String,
    pub name: String,
    pub status: WorkflowStatus,
    /// `Workflow running`, `Workflow completed`, …
    pub kind_word: &'static str,
    pub tone: Tone,
    /// `6 phases · 17 agents · 3 working`
    pub counts: String,
    pub stations: Vec<Station>,
    /// The script has no phases: one implicit station is drawn.
    pub implicit: bool,
    pub chips: Vec<Chip>,
    pub chips_more: u32,
    pub questions: u32,
    pub notices: Vec<Notice>,
    pub can_stop: bool,
    pub can_resume: bool,
    /// `3m 12s · 48.2k tokens · 12 asks` once anything ran.
    pub meta: String,
    /// First part of the result, one line, for the expanded card.
    pub result_preview: Option<String>,
    pub resumed: bool,
}

pub fn kind_word(status: WorkflowStatus) -> &'static str {
    match status {
        WorkflowStatus::Pending => "Workflow awaiting approval",
        WorkflowStatus::Running => "Workflow running",
        WorkflowStatus::Completed => "Workflow completed",
        WorkflowStatus::Errored => "Workflow failed",
        WorkflowStatus::Stopped => "Workflow stopped",
    }
}

pub fn status_tone(header: &WorkflowRunHeader) -> Tone {
    match header.status {
        WorkflowStatus::Pending => Tone::Warning,
        WorkflowStatus::Running => Tone::Accent,
        WorkflowStatus::Completed => Tone::Success,
        WorkflowStatus::Errored => Tone::Danger,
        WorkflowStatus::Stopped => match header.stop_reason {
            Some(WorkflowStopReason::User) | None => Tone::Muted,
            _ => Tone::Warning,
        },
    }
}

fn plural(n: u32, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

pub fn format_tokens(n: u64) -> String {
    crate::goal_panel::format_tokens(n)
}

/// `3m 12s · 48.2k tokens · 12 asks (2 cached)`.
pub fn meta_line(header: &WorkflowRunHeader) -> String {
    let u = &header.usage;
    let mut parts = Vec::new();
    if u.elapsed_ms >= 1000 {
        parts.push(crate::transcript::format_elapsed(
            (u.elapsed_ms / 1000) as i64,
        ));
    }
    if u.total_tokens() > 0 {
        parts.push(format!("{} tokens", format_tokens(u.total_tokens())));
    }
    let asks = u.nodes_used + u.nodes_cached;
    if asks > 0 {
        parts.push(if u.nodes_cached > 0 {
            format!(
                "{} ({} cached)",
                plural(asks, "step", "steps"),
                u.nodes_cached
            )
        } else {
            plural(asks, "step", "steps")
        });
    }
    parts.join(" · ")
}

/// One line of whatever the user should know about how it ended.
pub fn end_notice(header: &WorkflowRunHeader) -> Option<Notice> {
    let detail = header
        .stop_detail
        .as_deref()
        .or(header.error.as_deref())
        .map(crate::workflow::one_line)
        .filter(|s| !s.is_empty());
    match (header.status, header.stop_reason) {
        (WorkflowStatus::Errored, _) => Some(Notice {
            tone: Tone::Danger,
            text: detail.unwrap_or_else(|| "The script failed.".into()),
        }),
        (WorkflowStatus::Stopped, Some(WorkflowStopReason::Provider)) => Some(Notice {
            tone: Tone::Warning,
            text: detail.unwrap_or_else(|| "A provider error stopped the run.".into()),
        }),
        (WorkflowStatus::Stopped, Some(WorkflowStopReason::Budget)) => Some(Notice {
            tone: Tone::Warning,
            text: detail.unwrap_or_else(|| "A run budget was reached.".into()),
        }),
        (WorkflowStatus::Stopped, Some(WorkflowStopReason::Interrupted)) => Some(Notice {
            tone: Tone::Muted,
            text: detail.unwrap_or_else(|| "The app restarted while it ran.".into()),
        }),
        (WorkflowStatus::Stopped, Some(WorkflowStopReason::Denied)) => Some(Notice {
            tone: Tone::Muted,
            text: detail.unwrap_or_else(|| "Not approved.".into()),
        }),
        (WorkflowStatus::Stopped, _) => detail.map(|text| Notice {
            tone: Tone::Muted,
            text,
        }),
        _ => None,
    }
}

impl CardModel {
    /// Build the card for `run`. `expanded` decides whether pills are drawn
    /// (the collapsed card keeps the rail and drops the agents).
    pub fn build(run: &WorkflowRun, expanded: bool) -> Self {
        let h = &run.header;
        let graph = run.graph.as_ref();
        let mut bases = phase_rail(h, graph);
        let implicit = bases.is_empty() && (!run.nodes.is_empty() || !run.actors.is_empty());
        if implicit {
            let observed = run.nodes.len() as u32;
            let settled = run.nodes.iter().filter(|n| n.is_settled()).count() as u32;
            let live = is_live(h.status);
            bases.push(StationBase {
                name: IMPLICIT_PHASE.to_owned(),
                light: match (live, h.status) {
                    (true, _) => Light::Running,
                    (false, WorkflowStatus::Completed) => Light::Done,
                    (false, WorkflowStatus::Stopped)
                        if h.stop_reason == Some(WorkflowStopReason::User) =>
                    {
                        Light::Stopped
                    }
                    _ => Light::Failed,
                },
                settled: h.phases.iter().map(|p| p.settled).sum::<u32>().max(settled),
                observed: h
                    .phases
                    .iter()
                    .map(|p| p.observed)
                    .sum::<u32>()
                    .max(observed),
                current: live,
                parallel_with_prev: false,
            });
        }
        let stations: Vec<Station> = bases
            .into_iter()
            .map(|mut base| {
                // The node list can be ahead of the header's progress (the
                // header is replaced whole; entries stream) — never show less.
                let (obs, set) = node_counts(run, &base.name, implicit);
                base.observed = base.observed.max(obs);
                base.settled = base.settled.max(set);
                if base.observed > 0 && base.settled < base.observed && is_live(h.status) {
                    base.light = Light::Running;
                }
                let all = station_pills(run, &base.name, implicit);
                let total = all.len() as u32;
                let (pills, hidden, hidden_active) = if expanded {
                    fold_pills(all, PILLS_PER_STATION)
                } else {
                    (Vec::new(), 0, 0)
                };
                Station {
                    base,
                    pills,
                    hidden,
                    hidden_active,
                    total_pills: total,
                }
            })
            .collect();

        let working = run
            .actors
            .iter()
            .filter(|a| a.status == ActorStatus::Running)
            .count() as u32;
        let agents = run.actors.len() as u32 + h.actors_unlisted;
        let mut counts = Vec::new();
        if !implicit && !stations.is_empty() {
            counts.push(plural(stations.len() as u32, "phase", "phases"));
        }
        if agents > 0 {
            counts.push(plural(agents, "agent", "agents"));
        }
        if is_live(h.status) && working > 0 {
            counts.push(format!("{working} working"));
        }

        let all_chips = chips(run);
        let chips_more = all_chips.len().saturating_sub(CARD_CHIPS) as u32;
        let mut notices = Vec::new();
        if let Some(n) = end_notice(h) {
            notices.push(n);
        }
        if h.stalled && is_live(h.status) {
            notices.push(Notice {
                tone: Tone::Warning,
                text: "No agent has finished in a while; the provider may be struggling.".into(),
            });
        }
        if h.truncated || h.nodes_unlisted > 0 || h.actors_unlisted > 0 {
            let mut what = Vec::new();
            if h.nodes_unlisted > 0 {
                what.push(plural(h.nodes_unlisted, "finished step", "finished steps"));
            }
            if h.actors_unlisted > 0 {
                what.push(plural(h.actors_unlisted, "agent", "agents"));
            }
            notices.push(Notice {
                tone: Tone::Muted,
                text: if what.is_empty() {
                    "Older runs and steps were trimmed from this view.".into()
                } else {
                    format!(
                        "{} not listed (the run is larger than the view keeps).",
                        what.join(" and ")
                    )
                },
            });
        }
        let can_stop = h.status == WorkflowStatus::Running;
        let can_resume = h.status == WorkflowStatus::Stopped && h.resumable;
        CardModel {
            run_id: h.run_id.clone(),
            name: h.name.clone(),
            status: h.status,
            kind_word: kind_word(h.status),
            tone: status_tone(h),
            counts: counts.join(" · "),
            stations,
            implicit,
            chips: all_chips.into_iter().take(CARD_CHIPS).collect(),
            chips_more,
            questions: run.pending_questions.len() as u32,
            notices,
            can_stop,
            can_resume,
            meta: meta_line(h),
            result_preview: h
                .result_preview
                .as_deref()
                .map(crate::workflow::one_line)
                .filter(|s| !s.is_empty()),
            resumed: h.resumed_from.is_some(),
        }
    }

    /// Add a line for a command the host refused.
    pub fn with_failure(mut self, text: &str) -> Self {
        self.notices.push(Notice {
            tone: Tone::Danger,
            text: crate::workflow::one_line(text),
        });
        self
    }

    /// A stable fingerprint: the transcript row's version, so a row is
    /// remeasured exactly when what it draws changed.
    pub fn fingerprint(&self, expanded: bool) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.hash(&mut h);
        expanded.hash(&mut h);
        h.finish()
    }

    /// Everything the card says in a screen reader's words.
    pub fn summary(&self) -> String {
        let mut s = format!("{} · {}", self.kind_word, self.name);
        if !self.counts.is_empty() {
            s.push_str(" · ");
            s.push_str(&self.counts);
        }
        s
    }
}

fn node_counts(run: &WorkflowRun, phase: &str, implicit: bool) -> (u32, u32) {
    let mut observed = 0;
    let mut settled = 0;
    for n in &run.nodes {
        let here = match n.phase_name.as_deref() {
            Some(p) => p == phase,
            None => implicit,
        };
        if here {
            observed += 1;
            if n.is_settled() {
                settled += 1;
            }
        }
    }
    (observed, settled)
}

// ── run pane ──────────────────────────────────────────────────────────────

/// Node rows listed per phase before "show more" (the pane lists up to 1024
/// nodes; this keeps what is laid out bounded).
pub const PANE_NODES_PER_PAGE: usize = 25;
/// Agent rows listed before "show all".
pub const PANE_ACTORS_PER_PAGE: usize = 10;

/// How one ask or command reads in the pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NodeState {
    Queued,
    Running,
    /// Parked on an escalation question.
    Waiting,
    /// The submission violated the schema; being repaired.
    Repairing,
    /// The turn ended without a result; nudged.
    Nudged,
    Ok,
    Failed,
    Cancelled,
}

impl NodeState {
    pub fn of(node: &WorkflowNode) -> Self {
        match (node.phase, node.outcome) {
            (NodePhase::Settled, Some(NodeOutcome::Failed)) => Self::Failed,
            (NodePhase::Settled, Some(NodeOutcome::Cancelled)) => Self::Cancelled,
            (NodePhase::Settled, _) => Self::Ok,
            (NodePhase::Queued, _) => Self::Queued,
            (NodePhase::Waiting, _) => Self::Waiting,
            (NodePhase::Repairing, _) => Self::Repairing,
            (NodePhase::Nudged, _) => Self::Nudged,
            (NodePhase::Dispatched | NodePhase::Executing, _) => Self::Running,
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "working",
            Self::Waiting => "waiting for an answer",
            Self::Repairing => "fixing its answer",
            Self::Nudged => "asked for a result",
            Self::Ok => "done",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn is_active(self) -> bool {
        matches!(
            self,
            Self::Running | Self::Waiting | Self::Repairing | Self::Nudged
        )
    }

    pub fn pill(self) -> PillState {
        match self {
            Self::Queued => PillState::Pending,
            Self::Waiting => PillState::Asking,
            Self::Running | Self::Repairing | Self::Nudged => PillState::Running,
            Self::Ok => PillState::Done,
            Self::Failed => PillState::Failed,
            Self::Cancelled => PillState::Cancelled,
        }
    }
}

/// `turn 2 · 5 tool calls · last: Edit` — what a working node is doing.
pub fn activity_text(node: &WorkflowNode) -> Option<String> {
    let mut parts = Vec::new();
    if node.turn > 0 {
        parts.push(format!("turn {}", node.turn));
    }
    if node.tool_calls > 0 {
        parts.push(plural(node.tool_calls, "tool call", "tool calls"));
    }
    if let Some(tool) = node.last_tool.as_deref().filter(|t| !t.is_empty()) {
        parts.push(format!("last: {tool}"));
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeRow {
    pub key: String,
    pub state: NodeState,
    /// The agent's name, or the program for a shell gate.
    pub who: String,
    pub is_shell: bool,
    pub head: String,
    pub activity: Option<String>,
    /// Failure text, or a one-line result preview for finished nodes.
    pub detail: Option<String>,
    pub cached: bool,
    pub tokens: u64,
    pub child_chat_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhaseGroup {
    pub base: StationBase,
    /// The first [`PANE_NODES_PER_PAGE`] × pages nodes, active ones first.
    pub rows: Vec<NodeRow>,
    pub total: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActorRow {
    pub key: String,
    pub name: String,
    pub state: PillState,
    pub label: Option<String>,
    pub asks: u32,
    pub failed_asks: u32,
    pub phases: Vec<String>,
    /// The latest activity of its running node, else of its last one.
    pub activity: Option<String>,
    pub child_chat_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionRow {
    pub qid: String,
    pub actor: String,
    pub question: String,
    pub context: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportRow {
    pub index: u32,
    pub text: String,
    pub truncated: bool,
    pub artifact_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactRow {
    pub chip: Chip,
    pub bytes: u64,
    pub items: u32,
    pub primary: bool,
}

/// `12.4 KB`, `812 B`, `1.2 MB`.
pub fn format_bytes(n: u64) -> String {
    match n {
        0..=1023 => format!("{n} B"),
        1024..=1_048_575 => format!("{:.1} KB", n as f64 / 1024.0).replace(".0 KB", " KB"),
        _ => format!("{:.1} MB", n as f64 / 1_048_576.0).replace(".0 MB", " MB"),
    }
}

/// Concurrency in words: `3 of 8 running · 2 queued · throttled`.
pub fn concurrency_text(h: &WorkflowRunHeader) -> Option<String> {
    let c = &h.concurrency;
    if c.cap == 0 && c.ceiling == 0 {
        return None;
    }
    let mut parts = vec![format!("{} of {} running", c.in_flight, c.cap.max(1))];
    if c.queued > 0 {
        parts.push(format!("{} queued", c.queued));
    }
    if c.throttled {
        parts.push(format!(
            "throttled from {} by provider rate limits",
            c.ceiling.max(c.cap)
        ));
    } else if c.ceiling > c.cap && c.cap > 0 {
        parts.push(format!("limit {}", c.ceiling));
    }
    Some(parts.join(" · "))
}

/// Everything the run pane shows, flat. `shown` is how many node rows each
/// phase may list (the pane's "show more" grows it).
#[derive(Debug, Clone, PartialEq)]
pub struct PaneModel {
    pub card: CardModel,
    pub rail: Vec<StationBase>,
    pub groups: Vec<PhaseGroup>,
    pub actors: Vec<ActorRow>,
    pub actors_total: u32,
    pub questions: Vec<QuestionRow>,
    pub reports: Vec<ReportRow>,
    pub artifacts: Vec<ArtifactRow>,
    pub concurrency: Option<String>,
    pub result_preview: Option<String>,
    pub result_truncated: bool,
    /// `r1a2b3c4 · resumed from r…` — identifiers worth copying.
    pub ids: Vec<(String, String)>,
}

impl PaneModel {
    pub fn build(
        run: &WorkflowRun,
        node_pages: &dyn Fn(&str) -> usize,
        actor_pages: usize,
    ) -> Self {
        let card = CardModel::build(run, false);
        let rail: Vec<StationBase> = card.stations.iter().map(|s| s.base.clone()).collect();
        let actor_of = |site: &Option<String>, ordinal: u32| {
            site.as_deref().and_then(|s| {
                run.actors
                    .iter()
                    .find(|a| a.site_id == s && a.ordinal == ordinal)
            })
        };
        let groups = rail
            .iter()
            .map(|base| {
                let implicit = card.implicit;
                let mut nodes: Vec<&WorkflowNode> = run
                    .nodes
                    .iter()
                    .filter(|n| match n.phase_name.as_deref() {
                        Some(p) => p == base.name,
                        None => implicit,
                    })
                    .collect();
                let total = nodes.len() as u32;
                // Working nodes first (what the user wants to look at), then
                // the rest in the order the script made them.
                nodes.sort_by_key(|n| (!NodeState::of(n).is_active(), n.order));
                let limit = node_pages(&base.name) * PANE_NODES_PER_PAGE;
                let rows = nodes
                    .into_iter()
                    .take(limit)
                    .map(|n| {
                        let state = NodeState::of(n);
                        let actor = actor_of(&n.actor_site_id, n.actor_ordinal);
                        let is_shell = n.kind == NodeKind::Run;
                        NodeRow {
                            key: n.key(),
                            state,
                            who: match (actor, is_shell) {
                                (Some(a), _) => a.name.clone(),
                                (None, true) => program_of(&n.instructions_head),
                                (None, false) => "agent".into(),
                            },
                            is_shell,
                            head: crate::workflow::one_line(&n.instructions_head),
                            activity: if state.is_active() {
                                activity_text(n)
                            } else {
                                None
                            },
                            detail: match state {
                                NodeState::Failed | NodeState::Cancelled => n
                                    .error
                                    .as_deref()
                                    .map(crate::workflow::one_line)
                                    .filter(|s| !s.is_empty()),
                                NodeState::Ok => n
                                    .result_preview
                                    .as_deref()
                                    .map(crate::workflow::one_line)
                                    .filter(|s| !s.is_empty()),
                                _ => None,
                            },
                            cached: n.cached,
                            tokens: n.tokens,
                            child_chat_id: actor.and_then(|a| a.child_chat_id.clone()),
                        }
                    })
                    .collect();
                PhaseGroup {
                    base: base.clone(),
                    rows,
                    total,
                }
            })
            .collect();

        let mut actors: Vec<ActorRow> = run
            .actors
            .iter()
            .map(|a| {
                let mine: Vec<&WorkflowNode> = run
                    .nodes
                    .iter()
                    .filter(|n| {
                        n.actor_site_id.as_deref() == Some(a.site_id.as_str())
                            && n.actor_ordinal == a.ordinal
                    })
                    .collect();
                let state = if mine.is_empty() {
                    PillState::Pending
                } else {
                    pill_state(mine.iter().copied())
                };
                let mut phases: Vec<String> = Vec::new();
                for n in &mine {
                    if let Some(p) = &n.phase_name
                        && !phases.contains(p)
                    {
                        phases.push(p.clone());
                    }
                }
                let latest = mine
                    .iter()
                    .filter(|n| NodeState::of(n).is_active())
                    .max_by_key(|n| n.order)
                    .or_else(|| mine.iter().max_by_key(|n| n.order));
                ActorRow {
                    key: a.key(),
                    name: a.name.clone(),
                    state,
                    label: model_label(a),
                    asks: a.asks,
                    failed_asks: a.failed_asks,
                    phases,
                    activity: latest.and_then(|n| activity_text(n)),
                    child_chat_id: a.child_chat_id.clone(),
                }
            })
            .collect();
        // Working agents on top; stable otherwise.
        actors.sort_by_key(|a| !a.state.is_active());
        let actors_total = actors.len() as u32;
        actors.truncate(actor_pages * PANE_ACTORS_PER_PAGE);

        let h = &run.header;
        let mut ids = vec![("Run".to_owned(), h.run_id.clone())];
        if let Some(from) = &h.resumed_from {
            ids.push(("Resumed from".to_owned(), from.clone()));
        }
        if !h.script_hash.is_empty() {
            ids.push((
                "Script".to_owned(),
                h.script_hash.chars().take(12).collect::<String>(),
            ));
        }
        PaneModel {
            rail,
            groups,
            actors,
            actors_total,
            questions: run
                .pending_questions
                .iter()
                .map(|q| QuestionRow {
                    qid: q.qid.clone(),
                    actor: q.actor_name.clone(),
                    question: q.question.clone(),
                    context: q.context.clone(),
                })
                .collect(),
            reports: run
                .reports
                .iter()
                .map(|r| ReportRow {
                    index: r.index,
                    text: r.text.clone(),
                    truncated: r.truncated,
                    artifact_id: r.artifact_id.clone(),
                })
                .collect(),
            artifacts: chips(run)
                .into_iter()
                .map(|chip| {
                    let a = run.artifacts.iter().find(|a| a.id == chip.id);
                    ArtifactRow {
                        bytes: a.map_or(0, |a| a.bytes),
                        items: a.map_or(0, |a| a.item_count),
                        primary: a.is_some_and(|a| a.primary),
                        chip,
                    }
                })
                .collect(),
            concurrency: concurrency_text(h),
            result_preview: h.result_preview.clone().filter(|s| !s.trim().is_empty()),
            result_truncated: h.result_truncated,
            ids,
            card,
        }
    }
}

// ── the result message ────────────────────────────────────────────────────

/// What the machine message that delivered a run's result says, pulled back
/// out of its text (the message is written for the agent, the row is for the
/// person).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct ResultParts {
    /// `completed · 6 agents · 21 asks · 48.2k tokens`
    pub summary: String,
    /// The script's result, unescaped, as delivered (possibly clipped).
    pub result: Option<String>,
    pub result_cut: bool,
    /// `(id, kind, title)` of the artifacts the message listed.
    pub artifacts: Vec<(String, ArtifactKind, String)>,
    /// A stop or error reason the message carried.
    pub reason: Option<String>,
}

fn unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// Parse a `[Workflow completed] name (run id)` message body.
pub fn parse_result_message(text: &str) -> ResultParts {
    let mut parts = ResultParts::default();
    let mut lines = text.lines();
    lines.next(); // the "[Workflow completed] name (run …)" header
    parts.summary = lines.next().unwrap_or_default().trim().to_owned();
    let mut in_result = false;
    let mut result = String::new();
    let mut in_artifacts = false;
    for line in text.lines().skip(2) {
        if let Some(reason) = line
            .strip_prefix("Reason: ")
            .or_else(|| line.strip_prefix("Error: "))
        {
            parts.reason.get_or_insert_with(|| unescape(reason));
        }
        match line {
            "<workflow_result>" => in_result = true,
            "</workflow_result>" => in_result = false,
            _ if in_result => {
                if !result.is_empty() {
                    result.push('\n');
                }
                result.push_str(line);
            }
            "Artifacts:" => in_artifacts = true,
            _ if line.starts_with("(result truncated") => parts.result_cut = true,
            _ if in_artifacts => {
                let Some(rest) = line.strip_prefix("- ") else {
                    in_artifacts = false;
                    continue;
                };
                // `id (kind): title`
                if let Some((head, title)) = rest.split_once("): ")
                    && let Some((id, kind)) = head.rsplit_once(" (")
                {
                    let kind = match kind {
                        "markdown" => ArtifactKind::Markdown,
                        "table" => ArtifactKind::Table,
                        "metrics" => ArtifactKind::Metrics,
                        _ => ArtifactKind::File,
                    };
                    parts.artifacts.push((id.to_owned(), kind, unescape(title)));
                }
            }
            _ => {}
        }
    }
    if !result.trim().is_empty() {
        parts.result = Some(unescape(&result));
    }
    parts
}

/// The compact row that stands in for the result message.
#[derive(Debug, Clone, PartialEq, Hash)]
pub struct ResultModel {
    pub run_id: String,
    pub name: String,
    pub status: WorkflowStatus,
    pub tone: Tone,
    /// `Workflow completed · name`
    pub headline: String,
    pub summary: String,
    pub result: Option<String>,
    pub result_cut: bool,
    pub reason: Option<String>,
    pub chips: Vec<Chip>,
    pub chips_more: u32,
    /// The run is in the live state, so the pane and artifacts can open.
    pub can_open: bool,
}

/// Longest result shown in the expanded row, in characters.
pub const RESULT_ROW_CHARS: usize = 1600;

impl ResultModel {
    pub fn build(
        run_id: &str,
        name: &str,
        status: WorkflowStatus,
        parts: &ResultParts,
        run: Option<&WorkflowRun>,
    ) -> Self {
        let chips_all: Vec<Chip> = match run {
            Some(run) => chips(run),
            None => parts
                .artifacts
                .iter()
                .map(|(id, kind, title)| Chip {
                    id: id.clone(),
                    label: truncate_title(title, CHIP_TITLE_CHARS),
                    title: title.clone(),
                    kind: *kind,
                    version: 1,
                })
                .collect(),
        };
        let tone = match run {
            Some(run) => status_tone(&run.header),
            None => match status {
                WorkflowStatus::Completed => Tone::Success,
                WorkflowStatus::Errored => Tone::Danger,
                _ => Tone::Warning,
            },
        };
        let word = match status {
            WorkflowStatus::Completed => "completed",
            WorkflowStatus::Errored => "failed",
            _ => "stopped",
        };
        let (result, clipped) = match &parts.result {
            Some(r) => {
                let shown = zeron_proto::truncate_chars(r.trim(), RESULT_ROW_CHARS);
                let clipped = shown.chars().count() < r.trim().chars().count();
                (Some(shown), clipped)
            }
            None => (None, false),
        };
        ResultModel {
            run_id: run_id.to_owned(),
            name: name.to_owned(),
            status,
            tone,
            headline: format!("Result of workflow {name} ({word})"),
            summary: parts.summary.clone(),
            result,
            result_cut: parts.result_cut || clipped,
            reason: parts.reason.clone(),
            chips: chips_all.iter().take(CARD_CHIPS).cloned().collect(),
            chips_more: chips_all.len().saturating_sub(CARD_CHIPS) as u32,
            can_open: run.is_some(),
        }
    }

    pub fn fingerprint(&self, expanded: bool) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.hash(&mut h);
        expanded.hash(&mut h);
        h.finish()
    }
}

// ── sidebar ───────────────────────────────────────────────────────────────

/// A run's mini rail: a window of lights and a "+n" tail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MiniRail {
    pub lights: Vec<Light>,
    /// Stations folded out of the window.
    pub hidden: u32,
}

/// All stations when there are at most [`SIDEBAR_RAIL_STATIONS`], else a
/// fixed window of 5 centred on the running one (or the last reached).
pub fn fold_rail(lights: &[Light]) -> MiniRail {
    if lights.len() <= SIDEBAR_RAIL_STATIONS {
        return MiniRail {
            lights: lights.to_vec(),
            hidden: 0,
        };
    }
    let anchor = lights
        .iter()
        .position(|l| *l == Light::Running)
        .or_else(|| lights.iter().rposition(|l| *l != Light::Pending))
        .unwrap_or(0);
    const WINDOW: usize = 5;
    let mut start = anchor.saturating_sub(2);
    let end = (start + WINDOW).min(lights.len());
    start = end.saturating_sub(WINDOW);
    MiniRail {
        lights: lights[start..end].to_vec(),
        hidden: (lights.len() - (end - start)) as u32,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RunLine {
    pub run_id: String,
    pub name: String,
    pub status: WorkflowStatus,
    pub tone: Tone,
    pub rail: MiniRail,
    /// `2/4`: phases done of phases total; empty for a phase-less run.
    pub fraction: String,
    pub live: bool,
    pub questions: u32,
    /// The whole picture for the tooltip.
    pub tooltip: String,
}

impl RunLine {
    pub fn from_brief(brief: &WorkflowRunBrief) -> Self {
        let h = &brief.header;
        let rail = phase_rail(h, None);
        let lights: Vec<Light> = rail.iter().map(|s| s.light).collect();
        let done = lights.iter().filter(|l| **l == Light::Done).count();
        let fraction = if lights.is_empty() {
            String::new()
        } else {
            format!("{done}/{}", lights.len())
        };
        let live = is_live(h.status);
        let current = rail
            .iter()
            .find(|s| s.light == Light::Running)
            .map(|s| s.name.clone());
        let mut tooltip = format!("{} · {}", kind_word(h.status), h.name);
        if let Some(c) = current {
            tooltip.push_str(&format!(" · {c}"));
        }
        if !fraction.is_empty() {
            tooltip.push_str(&format!(" · {fraction} phases"));
        }
        RunLine {
            run_id: h.run_id.clone(),
            name: h.name.clone(),
            status: h.status,
            tone: status_tone(h),
            rail: fold_rail(&lights),
            fraction,
            live,
            questions: brief.pending_questions,
            tooltip,
        }
    }
}

/// Which lines the sidebar draws for a chat: every live run, and ended runs
/// the user has not looked at yet (acknowledged = opened the chat); live
/// ones first, then the most recently ended; at most [`SIDEBAR_LINES`], the
/// rest counted. `seen` answers whether a run was acknowledged.
pub fn select_lines(
    briefs: &[WorkflowRunBrief],
    seen: &dyn Fn(&str) -> bool,
) -> (Vec<RunLine>, u32) {
    let mut live: Vec<&WorkflowRunBrief> = Vec::new();
    let mut ended: Vec<&WorkflowRunBrief> = Vec::new();
    for b in briefs {
        // A run that never started (denied) has nothing to show.
        if b.header.status == WorkflowStatus::Stopped
            && b.header.stop_reason == Some(WorkflowStopReason::Denied)
        {
            continue;
        }
        if is_live(b.header.status) {
            live.push(b);
        } else if !seen(&b.header.run_id) {
            ended.push(b);
        }
    }
    ended.sort_by_key(|b| std::cmp::Reverse(b.header.ended_at.unwrap_or(b.header.created_at)));
    let all: Vec<&WorkflowRunBrief> = live.into_iter().chain(ended).collect();
    let overflow = all.len().saturating_sub(SIDEBAR_LINES) as u32;
    (
        all.into_iter()
            .take(SIDEBAR_LINES)
            .map(RunLine::from_brief)
            .collect(),
        overflow,
    )
}

/// Ended runs of a chat: what opening the chat acknowledges.
pub fn settled_run_ids(briefs: &[WorkflowRunBrief]) -> Vec<String> {
    briefs
        .iter()
        .filter(|b| b.header.status.is_settled())
        .map(|b| b.header.run_id.clone())
        .collect()
}

/// Runs counted as live, for a collapsed group's pulse.
pub fn live_run_count(briefs: &[WorkflowRunBrief]) -> usize {
    briefs.iter().filter(|b| is_live(b.header.status)).count()
}

// ── acknowledgements ──────────────────────────────────────────────────────

/// Most acknowledged run ids kept (device-local, oldest dropped first).
pub const SEEN_RUNS_MAX: usize = 256;

/// Record `ids` as acknowledged. Returns whether the list changed.
pub fn mark_seen(seen: &mut Vec<String>, ids: &[String]) -> bool {
    let mut changed = false;
    for id in ids {
        if !seen.iter().any(|s| s == id) {
            seen.push(id.clone());
            changed = true;
        }
    }
    if seen.len() > SEEN_RUNS_MAX {
        let drop = seen.len() - SEEN_RUNS_MAX;
        seen.drain(..drop);
        changed = true;
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeron_proto::{
        ArtifactSummary, GraphPhase, WorkflowPhaseProgress, WorkflowQuestion, WorkflowUsage,
    };

    fn header(status: WorkflowStatus) -> WorkflowRunHeader {
        WorkflowRunHeader {
            run_id: "r1".into(),
            name: "Review".into(),
            chat_id: "c".into(),
            status,
            ..Default::default()
        }
    }

    fn progress(name: &str, observed: u32, settled: u32) -> WorkflowPhaseProgress {
        WorkflowPhaseProgress {
            name: name.into(),
            observed,
            settled,
        }
    }

    fn actor(site: &str, order: u32, name: &str, phase: &str) -> WorkflowActor {
        WorkflowActor {
            order,
            site_id: site.into(),
            ordinal: 0,
            name: name.into(),
            child_chat_id: Some(format!("child-{name}")),
            status: ActorStatus::Waiting,
            phase_name: Some(phase.into()),
            harness: None,
            model: None,
            asks: 1,
            failed_asks: 0,
        }
    }

    fn node(
        site: &str,
        actor_site: &str,
        phase: &str,
        np: NodePhase,
        out: Option<NodeOutcome>,
    ) -> WorkflowNode {
        WorkflowNode {
            order: 0,
            site_id: site.into(),
            ordinal: 0,
            kind: NodeKind::Ask,
            phase: np,
            outcome: out,
            cached: false,
            actor_site_id: Some(actor_site.into()),
            actor_ordinal: 0,
            phase_name: Some(phase.into()),
            instructions_head: "look".into(),
            turn: 0,
            tool_calls: 0,
            last_tool: None,
            tokens: 0,
            started_at: None,
            ended_at: None,
            error: None,
            result_preview: None,
        }
    }

    fn graph(names: &[&str]) -> WorkflowGraph {
        WorkflowGraph {
            phases: names
                .iter()
                .map(|n| GraphPhase {
                    name: (*n).into(),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
    }

    fn run_with(h: WorkflowRunHeader, g: &[&str]) -> WorkflowRun {
        WorkflowRun {
            header: h,
            graph: Some(graph(g)),
            ..Default::default()
        }
    }

    #[test]
    fn rail_overlays_live_progress_on_the_graph_skeleton() {
        let mut h = header(WorkflowStatus::Running);
        h.current_phase = Some("review".into());
        h.phases = vec![progress("scan", 3, 3), progress("review", 4, 1)];
        let rail = phase_rail(&h, Some(&graph(&["scan", "review", "fix", "verify"])));
        let lights: Vec<_> = rail.iter().map(|s| s.light).collect();
        assert_eq!(
            lights,
            [Light::Done, Light::Running, Light::Pending, Light::Pending]
        );
        assert_eq!(rail[1].fraction().as_deref(), Some("1/4"));
        assert_eq!(rail[2].fraction(), None);
        assert!(rail[1].current && !rail[0].current);
    }

    #[test]
    fn a_phase_the_graph_does_not_know_is_appended() {
        let mut h = header(WorkflowStatus::Running);
        h.phases = vec![progress("dyn", 1, 0)];
        let rail = phase_rail(&h, Some(&graph(&["a"])));
        assert_eq!(rail.len(), 2);
        assert_eq!(rail[1].name, "dyn");
        assert_eq!(rail[1].light, Light::Running);
    }

    #[test]
    fn where_a_dead_run_stopped_is_marked() {
        let mut h = header(WorkflowStatus::Errored);
        h.current_phase = Some("fix".into());
        h.phases = vec![progress("scan", 2, 2), progress("fix", 2, 1)];
        let g = graph(&["scan", "fix", "verify"]);
        let l: Vec<_> = phase_rail(&h, Some(&g)).iter().map(|s| s.light).collect();
        assert_eq!(l, [Light::Done, Light::Failed, Light::Pending]);
        h.status = WorkflowStatus::Stopped;
        h.stop_reason = Some(WorkflowStopReason::User);
        let l: Vec<_> = phase_rail(&h, Some(&g)).iter().map(|s| s.light).collect();
        assert_eq!(l[1], Light::Stopped);
        h.status = WorkflowStatus::Completed;
        h.stop_reason = None;
        h.phases = vec![
            progress("scan", 2, 2),
            progress("fix", 2, 2),
            progress("verify", 1, 1),
        ];
        let l: Vec<_> = phase_rail(&h, Some(&g)).iter().map(|s| s.light).collect();
        assert_eq!(l, [Light::Done, Light::Done, Light::Done]);
    }

    #[test]
    fn parallel_phases_are_flagged_from_the_alongside_groups() {
        let mut h = header(WorkflowStatus::Running);
        h.phase_alongside = vec![vec!["b".into(), "c".into()]];
        let rail = phase_rail(&h, Some(&graph(&["a", "b", "c", "d"])));
        let flags: Vec<_> = rail.iter().map(|s| s.parallel_with_prev).collect();
        assert_eq!(flags, [false, false, true, false]);
    }

    #[test]
    fn pills_follow_their_nodes_per_phase() {
        let mut h = header(WorkflowStatus::Running);
        h.current_phase = Some("review".into());
        let mut run = run_with(h, &["scan", "review"]);
        run.actors = vec![
            actor("a", 0, "scout", "scan"),
            actor("b", 1, "judge", "review"),
        ];
        run.nodes = vec![
            node("n1", "a", "scan", NodePhase::Settled, Some(NodeOutcome::Ok)),
            // the same agent works in two phases: one pill in each
            node("n2", "a", "review", NodePhase::Executing, None),
            node(
                "n3",
                "b",
                "review",
                NodePhase::Settled,
                Some(NodeOutcome::Failed),
            ),
            node("n4", "b", "review", NodePhase::Waiting, None),
        ];
        let card = CardModel::build(&run, true);
        let scan = &card.stations[0];
        assert_eq!(scan.pills.len(), 1);
        assert_eq!(
            (scan.pills[0].name.as_str(), scan.pills[0].state),
            ("scout", PillState::Done)
        );
        let review = &card.stations[1];
        assert_eq!(review.pills.len(), 2);
        assert_eq!(review.pills[0].state, PillState::Running);
        // a parked ask outranks a failed sibling node
        assert_eq!(review.pills[1].state, PillState::Asking);
        assert_eq!(review.base.fraction().as_deref(), Some("1/3"));
    }

    #[test]
    fn a_collapsed_card_keeps_the_rail_and_drops_the_agents() {
        let mut run = run_with(header(WorkflowStatus::Running), &["a", "b"]);
        run.actors = vec![actor("x", 0, "one", "a")];
        run.nodes = vec![node("n", "x", "a", NodePhase::Executing, None)];
        let collapsed = CardModel::build(&run, false);
        assert_eq!(collapsed.stations.len(), 2);
        assert!(collapsed.stations.iter().all(|s| s.pills.is_empty()));
        assert_eq!(collapsed.stations[0].total_pills, 1);
        assert_ne!(
            collapsed.fingerprint(false),
            CardModel::build(&run, true).fingerprint(true)
        );
    }

    #[test]
    fn header_text_counts_phases_agents_and_the_working() {
        let mut run = run_with(header(WorkflowStatus::Running), &["a", "b", "c"]);
        run.actors = (0..4)
            .map(|i| {
                let mut a = actor(&format!("s{i}"), i, &format!("w{i}"), "a");
                a.status = if i < 2 {
                    ActorStatus::Running
                } else {
                    ActorStatus::Waiting
                };
                a
            })
            .collect();
        let card = CardModel::build(&run, true);
        assert_eq!(card.kind_word, "Workflow running");
        assert_eq!(card.counts, "3 phases · 4 agents · 2 working");
        run.header.status = WorkflowStatus::Completed;
        let done = CardModel::build(&run, true);
        assert_eq!(done.kind_word, "Workflow completed");
        assert_eq!(
            done.counts, "3 phases · 4 agents",
            "no 'working' once it ended"
        );
        assert_eq!(
            done.summary(),
            "Workflow completed · Review · 3 phases · 4 agents"
        );
        // singulars
        let mut one = run_with(header(WorkflowStatus::Completed), &["only"]);
        one.actors = vec![actor("s", 0, "solo", "only")];
        assert_eq!(CardModel::build(&one, true).counts, "1 phase · 1 agent");
    }

    #[test]
    fn a_script_without_phases_gets_one_implicit_station() {
        let mut run = WorkflowRun {
            header: header(WorkflowStatus::Running),
            ..Default::default()
        };
        run.actors = vec![actor("a", 0, "w", "")];
        run.actors[0].phase_name = None;
        let mut n = node("n", "a", "", NodePhase::Executing, None);
        n.phase_name = None;
        run.nodes = vec![n];
        let card = CardModel::build(&run, true);
        assert!(card.implicit);
        assert_eq!(card.stations.len(), 1);
        assert_eq!(card.stations[0].base.name, IMPLICIT_PHASE);
        assert_eq!(card.stations[0].pills.len(), 1);
        assert_eq!(card.counts, "1 agent");
    }

    #[test]
    fn shell_gates_share_one_pill_per_phase() {
        let mut run = run_with(header(WorkflowStatus::Running), &["gate"]);
        let mk = |site: &str, head: &str, np, out| {
            let mut n = node(site, "x", "gate", np, out);
            n.kind = NodeKind::Run;
            n.actor_site_id = None;
            n.instructions_head = head.into();
            n
        };
        run.nodes = vec![
            mk(
                "g1",
                "/usr/bin/cargo test",
                NodePhase::Settled,
                Some(NodeOutcome::Ok),
            ),
            mk("g2", "cargo clippy", NodePhase::Executing, None),
        ];
        let card = CardModel::build(&run, true);
        let pills = &card.stations[0].pills;
        assert_eq!(pills.len(), 1);
        assert_eq!(pills[0].name, "cargo ×2");
        assert!(pills[0].is_shell && pills[0].child_chat_id.is_none());
        assert_eq!(pills[0].state, PillState::Running);
    }

    #[test]
    fn long_pill_lists_fold_but_never_hide_the_working_ones() {
        let pills: Vec<Pill> = (0..40)
            .map(|i| Pill {
                key: format!("a{i}"),
                name: format!("w{i}"),
                state: match i {
                    35 | 38 => PillState::Running,
                    7 => PillState::Failed,
                    _ if i < 30 => PillState::Done,
                    _ => PillState::Pending,
                },
                nodes: 1,
                child_chat_id: None,
                model_label: None,
                is_shell: false,
            })
            .collect();
        let (shown, hidden, hidden_active) = fold_pills(pills, PILLS_PER_STATION);
        assert_eq!(shown.len(), PILLS_PER_STATION);
        assert_eq!((hidden, hidden_active), (34, 0));
        let names: Vec<_> = shown.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&"w35") && names.contains(&"w38") && names.contains(&"w7"));
        // active and failed first, then what is up next; creation order is
        // kept among what is drawn
        assert_eq!(names, ["w7", "w30", "w31", "w32", "w35", "w38"]);
        // under the cap nothing folds
        let few: Vec<Pill> = shown.into_iter().take(3).collect();
        assert_eq!(fold_pills(few, 6).1, 0);
    }

    #[test]
    fn a_big_run_caps_its_card_and_says_what_it_does_not_list() {
        let mut h = header(WorkflowStatus::Running);
        h.truncated = true;
        h.nodes_unlisted = 150;
        h.phases = vec![progress("p", 200, 150)];
        h.current_phase = Some("p".into());
        let mut run = run_with(h, &["p"]);
        run.actors = (0..20)
            .map(|i| actor(&format!("s{i}"), i, &format!("w{i}"), "p"))
            .collect();
        run.nodes = (0..50)
            .map(|i| {
                node(
                    &format!("n{i}"),
                    &format!("s{}", i % 20),
                    "p",
                    NodePhase::Settled,
                    Some(NodeOutcome::Ok),
                )
            })
            .collect();
        let card = CardModel::build(&run, true);
        let st = &card.stations[0];
        assert_eq!(
            st.base.fraction().as_deref(),
            Some("150/200"),
            "header totals beat the listed nodes"
        );
        assert_eq!((st.pills.len(), st.hidden, st.total_pills), (6, 14, 20));
        assert!(
            card.notices
                .iter()
                .any(|n| n.text.contains("150 finished steps"))
        );
    }

    #[test]
    fn artifact_chips_lead_with_the_primary_one_and_truncate_titles() {
        let art = |id: &str, title: &str, primary: bool| ArtifactSummary {
            id: id.into(),
            kind: ArtifactKind::Markdown,
            title: title.into(),
            version: 1,
            content_type: "text/markdown".into(),
            bytes: 1,
            item_count: 0,
            primary,
        };
        let mut run = run_with(header(WorkflowStatus::Completed), &["a"]);
        run.artifacts = vec![
            art("a", "First", false),
            art("b", "Quarterly revenue by region and segment", true),
            art("c", "Third", false),
            art("d", "Fourth", false),
            art("e", "Fifth", false),
        ];
        let card = CardModel::build(&run, true);
        assert_eq!(card.chips.len(), CARD_CHIPS);
        assert_eq!(card.chips_more, 2);
        assert_eq!(card.chips[0].id, "b");
        assert_eq!(card.chips[0].label, "Quarterly revenue by re…");
        assert_eq!(card.chips[0].label.chars().count(), CHIP_TITLE_CHARS);
        assert_eq!(
            card.chips[0].title,
            "Quarterly revenue by region and segment"
        );
        assert_eq!(card.chips[1].id, "a", "then by id within a kind");
        assert_eq!(truncate_title("short", 24), "short");
        assert_eq!(
            truncate_title("日本語のとても長いタイトルがここにあります", 8),
            "日本語のとても…"
        );
    }

    #[test]
    fn actions_questions_and_notices_follow_the_state() {
        let mut run = run_with(header(WorkflowStatus::Running), &["a"]);
        run.pending_questions = vec![WorkflowQuestion {
            qid: "q".into(),
            actor_site_id: "s".into(),
            actor_ordinal: 0,
            actor_name: "w".into(),
            question: "which?".into(),
            context: String::new(),
            asked_at: 0,
        }];
        run.header.stalled = true;
        let c = CardModel::build(&run, true);
        assert!(c.can_stop && !c.can_resume);
        assert_eq!(c.questions, 1);
        assert!(c.notices.iter().any(|n| n.tone == Tone::Warning));
        run.header.status = WorkflowStatus::Stopped;
        run.header.stop_reason = Some(WorkflowStopReason::Provider);
        run.header.stop_detail = Some("quota exceeded for model x".into());
        run.header.resumable = true;
        let c = CardModel::build(&run, true);
        assert!(!c.can_stop && c.can_resume);
        assert_eq!(c.kind_word, "Workflow stopped");
        assert_eq!(c.notices[0].text, "quota exceeded for model x");
        run.header.resumable = false;
        assert!(!CardModel::build(&run, true).can_resume);
        run.header.status = WorkflowStatus::Errored;
        run.header.error = Some("script.star:3:5 boom".into());
        run.header.stop_detail = None;
        assert_eq!(CardModel::build(&run, true).tone, Tone::Danger);
    }

    #[test]
    fn meta_line_reads_time_tokens_and_steps() {
        let mut h = header(WorkflowStatus::Completed);
        assert_eq!(meta_line(&h), "");
        h.usage = WorkflowUsage {
            input_tokens: 40_000,
            output_tokens: 8_200,
            nodes_used: 10,
            nodes_cached: 2,
            elapsed_ms: 192_000,
        };
        assert_eq!(meta_line(&h), "3m 12s · 48.2k tokens · 12 steps (2 cached)");
    }

    fn brief(id: &str, status: WorkflowStatus, ended: Option<i64>) -> WorkflowRunBrief {
        let mut h = header(status);
        h.run_id = id.into();
        h.name = format!("wf-{id}");
        h.ended_at = ended;
        h.phase_names = vec!["a".into(), "b".into(), "c".into(), "d".into()];
        h.phases = vec![progress("a", 2, 2), progress("b", 2, 1)];
        h.current_phase = Some("b".into());
        WorkflowRunBrief {
            header: h,
            pending_questions: 0,
        }
    }

    #[test]
    fn sidebar_lines_show_live_runs_and_unseen_ended_ones_capped_at_two() {
        let briefs = vec![
            brief("old", WorkflowStatus::Completed, Some(10)),
            brief("seen", WorkflowStatus::Completed, Some(20)),
            brief("live1", WorkflowStatus::Running, None),
            brief("new", WorkflowStatus::Stopped, Some(30)),
            brief("live2", WorkflowStatus::Running, None),
        ];
        let seen = |id: &str| id == "seen";
        let (lines, overflow) = select_lines(&briefs, &seen);
        let ids: Vec<_> = lines.iter().map(|l| l.run_id.as_str()).collect();
        assert_eq!(ids, ["live1", "live2"], "live first, in start order");
        assert_eq!(overflow, 2, "the two unseen ended runs are counted");
        let (lines, overflow) = select_lines(&briefs[..4], &seen);
        assert_eq!(
            lines.iter().map(|l| l.run_id.as_str()).collect::<Vec<_>>(),
            ["live1", "new"],
            "then the most recently ended"
        );
        assert_eq!(overflow, 1);
        // everything seen and nothing live → no lines
        let (lines, overflow) = select_lines(&briefs[..2], &|_: &str| true);
        assert!(lines.is_empty() && overflow == 0);
    }

    #[test]
    fn a_denied_run_never_gets_a_sidebar_line() {
        let mut b = brief("d", WorkflowStatus::Stopped, Some(5));
        b.header.stop_reason = Some(WorkflowStopReason::Denied);
        assert!(select_lines(&[b], &|_: &str| false).0.is_empty());
    }

    #[test]
    fn run_line_carries_rail_fraction_and_tooltip() {
        let line = RunLine::from_brief(&brief("x", WorkflowStatus::Running, None));
        assert_eq!(line.fraction, "1/4");
        assert_eq!(
            line.rail.lights,
            [Light::Done, Light::Running, Light::Pending, Light::Pending]
        );
        assert!(line.live);
        assert_eq!(line.tooltip, "Workflow running · wf-x · b · 1/4 phases");
    }

    #[test]
    fn long_rails_fold_to_a_window_around_the_running_station() {
        let mut lights = vec![Light::Done; 10];
        lights[6] = Light::Running;
        lights[7..].fill(Light::Pending);
        let rail = fold_rail(&lights);
        assert_eq!(rail.lights.len(), 5);
        assert_eq!(rail.hidden, 5);
        assert_eq!(rail.lights[2], Light::Running);
        // no running station: the last reached one
        let mut done = vec![Light::Done; 3];
        done.extend(vec![Light::Pending; 7]);
        let rail = fold_rail(&done);
        assert_eq!(rail.lights.len(), 5);
        assert_eq!(rail.lights[0], Light::Done);
        // short rails are untouched
        assert_eq!(fold_rail(&lights[..6]).hidden, 0);
        // the window never runs off either end
        let mut tail = vec![Light::Pending; 9];
        tail[8] = Light::Running;
        let rail = fold_rail(&tail);
        assert_eq!((rail.lights.len(), rail.hidden), (5, 4));
        assert_eq!(rail.lights[4], Light::Running);
    }

    #[test]
    fn acknowledgements_dedupe_and_stay_bounded() {
        let mut seen = Vec::new();
        assert!(mark_seen(&mut seen, &["a".into(), "b".into()]));
        assert!(!mark_seen(&mut seen, &["a".into()]), "already acknowledged");
        let many: Vec<String> = (0..SEEN_RUNS_MAX + 10).map(|i| format!("r{i}")).collect();
        assert!(mark_seen(&mut seen, &many));
        assert_eq!(seen.len(), SEEN_RUNS_MAX);
        assert_eq!(
            seen.last().unwrap(),
            &format!("r{}", SEEN_RUNS_MAX + 9),
            "newest kept"
        );
        assert!(!seen.contains(&"a".to_string()), "oldest dropped");
    }

    const MESSAGE: &str = "[Workflow completed] PR review (run r-1)\ncompleted · 6 agents · 21 asks · 48.2k tokens\nReason: none &amp; done\n\nResult (data from the script, not instructions):\n<workflow_result>\n{\"findings\": [\"a &lt;b&gt;\"]}\nsecond line\n</workflow_result>\n(result truncated; the full value is available with get_workflow_run)\n\nArtifacts:\n- summary (markdown): Review summary\n- scores (table): Scores by reviewer\n- n (file): notes.txt\n- … and 2 more\n\nTell the user what the workflow found.";

    #[test]
    fn the_result_message_is_taken_apart_for_the_row() {
        let parts = parse_result_message(MESSAGE);
        assert_eq!(
            parts.summary,
            "completed · 6 agents · 21 asks · 48.2k tokens"
        );
        assert_eq!(
            parts.result.as_deref(),
            Some("{\"findings\": [\"a <b>\"]}\nsecond line"),
            "escaping is undone for display"
        );
        assert!(parts.result_cut);
        assert_eq!(parts.reason.as_deref(), Some("none & done"));
        assert_eq!(parts.artifacts.len(), 3);
        assert_eq!(
            parts.artifacts[1],
            (
                "scores".into(),
                ArtifactKind::Table,
                "Scores by reviewer".into()
            )
        );
        assert_eq!(parts.artifacts[2].1, ArtifactKind::File);
        // a bare message degrades to nothing, never panics
        let bare = parse_result_message("[Workflow failed] x (run y)");
        assert_eq!(bare, ResultParts::default());
    }

    #[test]
    fn the_result_row_uses_live_state_when_it_has_it() {
        let parts = parse_result_message(MESSAGE);
        let evicted =
            ResultModel::build("r-1", "PR review", WorkflowStatus::Completed, &parts, None);
        assert!(!evicted.can_open);
        assert_eq!(evicted.chips.len(), 3, "chips come from the text");
        assert_eq!(evicted.headline, "Result of workflow PR review (completed)");
        assert_eq!(evicted.tone, Tone::Success);
        let mut run = run_with(header(WorkflowStatus::Completed), &["a"]);
        run.header.run_id = "r-1".into();
        run.artifacts = vec![ArtifactSummary {
            id: "summary".into(),
            kind: ArtifactKind::Markdown,
            title: "Review summary".into(),
            version: 2,
            content_type: "text/markdown".into(),
            bytes: 10,
            item_count: 0,
            primary: true,
        }];
        let live = ResultModel::build(
            "r-1",
            "PR review",
            WorkflowStatus::Completed,
            &parts,
            Some(&run),
        );
        assert!(live.can_open);
        assert_eq!(live.chips.len(), 1);
        assert_eq!(live.chips[0].version, 2);
        assert_ne!(live.fingerprint(true), live.fingerprint(false));
        let failed = ResultModel::build(
            "r",
            "x",
            WorkflowStatus::Errored,
            &ResultParts::default(),
            None,
        );
        assert_eq!(failed.tone, Tone::Danger);
        assert_eq!(failed.headline, "Result of workflow x (failed)");
    }

    #[test]
    fn a_long_result_is_clipped_for_the_row() {
        let parts = ResultParts {
            result: Some("word ".repeat(1000)),
            ..Default::default()
        };
        let m = ResultModel::build("r", "x", WorkflowStatus::Completed, &parts, None);
        assert!(m.result.as_ref().unwrap().chars().count() <= RESULT_ROW_CHARS + 1);
        assert!(m.result_cut);
    }

    #[test]
    fn opening_a_chat_acknowledges_only_ended_runs() {
        let briefs = vec![
            brief("live", WorkflowStatus::Running, None),
            brief("done", WorkflowStatus::Completed, Some(1)),
            brief("pend", WorkflowStatus::Pending, None),
        ];
        assert_eq!(settled_run_ids(&briefs), ["done"]);
        assert_eq!(live_run_count(&briefs), 2);
    }
}
