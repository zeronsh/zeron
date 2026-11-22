//! Where each card is on the board.
//!
//! A card's place is never set: the engine that keeps its board works it
//! out from facts (its threads, its asks, its finish line, what the person
//! did) whenever one changes, and writes the result here, in the card's
//! [`BoardCard`] row (registry kind `boardCards`, keyed by the item's id).
//! The row also keeps the facts only the engine records (the finish line,
//! how it closed, its thread's last word) and the card's timeline.
//!
//! The row is its own kind, not fields of [`WorkItem`], because a build
//! from before it writes a `WorkItem` back without the fields it doesn't
//! know. [`WorkItem::state`] still gets a value an older reader understands
//! ([`BoardCard::old_state`]); nothing reads it back.
//!
//! [`WorkItem`]: super::WorkItem
//! [`WorkItem::state`]: super::WorkItem::state

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::{ReportState, WorkItem, WorkState};

/// Where a card is: one of the board's three columns or their drawers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Place {
    /// Needs you: an open ask to the person.
    NeedsYou,
    /// Needs you's drawer: every ask on it is put off, and nobody works on
    /// it meanwhile.
    Snoozed,
    /// Running: an agent is provably on it.
    Running,
    /// Running's drawer: not started, paused by the person, or waiting on
    /// another card.
    Planned,
    /// Done: it crossed its finish line (today's in the column, the rest in
    /// its Earlier drawer; the app splits them by [`BoardCard::since`]).
    Done,
    /// Off the board, kept in Earlier's history.
    Dropped,
    #[serde(other)]
    Other,
}

impl Place {
    /// The wire word.
    pub fn as_str(self) -> &'static str {
        match self {
            Place::NeedsYou => "needsYou",
            Place::Snoozed => "snoozed",
            Place::Running => "running",
            Place::Planned => "planned",
            Place::Done => "done",
            Place::Dropped => "dropped",
            Place::Other => "other",
        }
    }

    /// As the person reads it.
    pub fn label(self) -> &'static str {
        match self {
            Place::NeedsYou => "Needs you",
            Place::Snoozed => "Snoozed",
            Place::Running => "Running",
            Place::Planned => "Planned",
            Place::Done => "Done",
            Place::Dropped => "Dropped",
            Place::Other => "Other",
        }
    }

    /// Closed: nothing more happens on it by itself.
    pub fn closed(self) -> bool {
        matches!(self, Place::Done | Place::Dropped)
    }
}

/// What a card says under its title: words only, never a column of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CardStatus {
    // Needs you (and Snoozed, which keeps the word of the ask put off).
    /// An agent asked the person something.
    Asked,
    /// Its thread stopped on an error: retry or drop?
    Failed,
    /// Something to look at: an open PR, or a result to accept.
    Review,
    /// Nothing is moving on it: continue, close or drop?
    Stalled,
    // Running.
    Working,
    /// Its thread waits on its own background work or subagents.
    Waiting,
    /// Its thread waits for its usage limit to reset ([`BoardCard::until`]).
    Limited,
    /// Its orchestrator is looking at what it reported or what the person
    /// answered, or the engine is checking where it stands.
    Checking,
    /// A message to its thread is on its way, or queued behind what the
    /// thread is doing now.
    Queued,
    // Planned.
    /// Starts when a slot frees.
    Next,
    /// Kept for later: never starts on its own.
    Held,
    /// Paused by the person.
    Paused,
    /// Waits on another card.
    After,
    // Closed.
    Done,
    Dropped,
    #[serde(other)]
    Other,
}

impl CardStatus {
    /// As the card says it.
    pub fn label(self) -> &'static str {
        match self {
            CardStatus::Asked => "Waiting for you",
            CardStatus::Failed => "Failed",
            CardStatus::Review => "Review",
            CardStatus::Stalled => "Nothing is moving",
            CardStatus::Working => "Working",
            CardStatus::Waiting => "Waiting on tests",
            CardStatus::Limited => "Waiting for usage limit",
            CardStatus::Checking => "Orchestrator checking",
            CardStatus::Queued => "Queued",
            CardStatus::Next => "Up next",
            CardStatus::Held => "Held",
            CardStatus::Paused => "Paused by you",
            CardStatus::After => "Waiting on another task",
            CardStatus::Done => "Done",
            CardStatus::Dropped => "Dropped",
            CardStatus::Other => "",
        }
    }
}

/// Who made a move happen.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Mover {
    /// The person (the only moves that toast).
    You,
    /// The engine, from what it saw.
    #[default]
    Engine,
    /// An agent's act (it closed the card with a reason).
    Agent,
    #[serde(other)]
    Other,
}

/// What a move was, for the timeline's glyph and the animation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MoveCause {
    /// The card was made.
    Created,
    /// Work started on it or was sent to it.
    Started,
    /// Its thread works on it.
    Working,
    /// Its thread's turn ended.
    Finished,
    /// Something asked the person.
    Asked,
    /// Its ask was answered or dismissed.
    Answered,
    /// Its ask was put off.
    Snoozed,
    /// Its put-off ask came back.
    Returned,
    /// Its thread stopped on an error.
    Failed,
    /// Nothing was moving on it.
    Stalled,
    Paused,
    Resumed,
    /// Held for later, or let go again.
    Held,
    /// It waits on another card.
    Waiting,
    /// Its thread reported done.
    Reported,
    /// Its branch landed.
    Merged,
    /// Its PR merged or closed.
    PrClosed,
    /// The person accepted it.
    Accepted,
    /// The person marked it done.
    Marked,
    /// Its orchestrator closed it with a reason.
    Closed,
    Dropped,
    /// It came back from closed.
    Reopened,
    /// Mapped from the column it had before places.
    #[default]
    Mapped,
    #[serde(other)]
    Other,
}

/// What ends a card: chosen when it's made, checked by the engine.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FinishKind {
    /// Its thread reports done, and its orchestrator's turn on that report
    /// ends without sending more work or asking. (A thread that reports to
    /// nobody: its turn completes with nothing left.)
    #[default]
    Report,
    /// Its branch's tip is contained in its target branch.
    Merged,
    /// Its PR is merged or closed.
    Pr,
    /// The person says it looks good.
    Accept,
    #[serde(other)]
    Other,
}

impl FinishKind {
    pub fn as_str(self) -> &'static str {
        match self {
            FinishKind::Report => "report",
            FinishKind::Merged => "merged",
            FinishKind::Pr => "pr",
            FinishKind::Accept => "accept",
            FinishKind::Other => "other",
        }
    }
}

/// A card's finish line.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Finish {
    #[serde(default)]
    pub line: FinishKind,
    /// [`FinishKind::Merged`]: the card's branch (absent: its thread's own,
    /// else its branch link).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// [`FinishKind::Merged`]: the branch it lands on (absent: the branch
    /// its project's own checkout is on).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub into: Option<String>,
    /// [`FinishKind::Pr`]: its PR's URL (absent: the card's PR links).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr: Option<String>,
}

impl Finish {
    pub fn of(line: FinishKind) -> Self {
        Finish { line, ..Default::default() }
    }

    /// "Done when", as the side panel says it.
    pub fn done_when(&self) -> String {
        match self.line {
            FinishKind::Merged => match self.into.as_deref() {
                Some(into) => format!("Merged into {into}"),
                None => "Its branch is merged".into(),
            },
            FinishKind::Pr => "Its PR is merged or closed".into(),
            FinishKind::Accept => "You say it looks good".into(),
            FinishKind::Report | FinishKind::Other => "Its thread reports done".into(),
        }
    }
}

/// How a card closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CloseHow {
    /// Its thread's done report stood.
    Report,
    /// Its branch landed ([`Closed::note`]: the commit, short).
    Merged,
    /// Its PR merged or closed.
    Pr,
    /// The person accepted it.
    Accepted,
    /// The person marked it done.
    Marked,
    /// Its orchestrator closed it ([`Closed::note`]: why).
    Closed,
    /// Taken off the plan.
    Dropped,
    /// It was Done (or in Review with nothing to review) before places.
    Mapped,
    #[serde(other)]
    Other,
}

/// A card's closing: recorded once, kept until it's reopened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Closed {
    pub at: DateTime<Utc>,
    pub by: Mover,
    pub how: CloseHow,
    /// The reason, the commit, the PR's end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl Closed {
    pub fn dropped(&self) -> bool {
        self.how == CloseHow::Dropped
    }
}

/// The last thing a card's thread reported that says where it stands:
/// done, blocked or failed (progress and plain messages don't replace it).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Word {
    pub state: ReportState,
    pub at: DateTime<Utc>,
    /// The thread that said it.
    pub from: String,
    pub summary: String,
    /// The report's id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<String>,
}

/// Why a card's orchestrator has a turn on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CheckWhy {
    /// Its thread reported.
    Report,
    /// The person answered its ask.
    Answer,
    /// Its thread asked something its orchestrator may answer.
    CrewAsk,
    /// The engine found nothing moving on it and woke its orchestrator.
    Stall,
    /// It was made inside the orchestrator's turn, or routed to it.
    Created,
    #[serde(other)]
    Other,
}

/// A card's orchestrator has a turn on it (running, or about to be woken).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Checking {
    pub since: DateTime<Utc>,
    pub why: CheckWhy,
}

/// The engine's own check of a card nothing moves on: its orchestrator
/// woken once, then the person asked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stall {
    /// When nothing was first found on it.
    pub since: DateTime<Utc>,
    /// When its orchestrator was woken for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub woken_at: Option<DateTime<Utc>>,
    /// When that turn ended (or the wake was given up on).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settled_at: Option<DateTime<Utc>>,
    /// The ask raised for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ask: Option<String>,
}

/// What a timeline line is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EventKind {
    /// The card moved ([`CardEvent::to`]).
    Move,
    /// Its thread reported.
    Report,
    /// Something asked the person.
    Ask,
    /// An ask was answered.
    Answer,
    /// A thread was put on it.
    Thread,
    /// A note of the engine's (work sent, its orchestrator woken).
    Note,
    #[serde(other)]
    Other,
}

/// One line of a card's "What happened", written by the engine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CardEvent {
    pub at: DateTime<Utc>,
    pub kind: EventKind,
    /// One line.
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<Mover>,
    /// A move: the place it left and the one it came to, and its number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<Place>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<Place>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
    /// What it's about: the ask, the report, the thread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub about: Option<String>,
}

/// The most a card's timeline keeps (the oldest go first).
pub const CARD_EVENTS_MAX: usize = 40;
/// The most one timeline line keeps.
pub const CARD_EVENT_TEXT_MAX: usize = 140;

/// What a chip on a card is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ChipKind {
    Merged,
    Pr,
    Report,
    Accepted,
    Closed,
    Dropped,
    #[serde(other)]
    Other,
}

/// The one chip a card may carry at its foot (`merged e99f817`, `PR #1503`,
/// `report`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CardChip {
    pub kind: ChipKind,
    pub label: String,
    /// Where it leads: a URL, a commit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
}

/// A card's place and what the engine keeps about it. Synced (registry kind
/// `boardCards`, its id the item's).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardCard {
    /// Its [`WorkItem`]'s id.
    pub id: String,

    // ── Worked out by the engine ────────────────────────────────────────────
    pub place: Place,
    pub status: CardStatus,
    /// A few words with the status: what its thread waits on, the card it
    /// waits on, why it's queued.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// When the status ends by itself: a usage limit's reset, a put-off
    /// ask's return.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<DateTime<Utc>>,
    /// When it came to this place.
    pub since: DateTime<Utc>,
    /// When its status last changed (the card's live timer counts from it).
    pub status_since: DateTime<Utc>,
    /// Counts its moves: a reader that sees it rise animates the move
    /// (`from` to `place`, for `reason`).
    #[serde(default)]
    pub seq: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<Place>,
    /// Why it moved, as the timeline says it ("Thread finished", "You
    /// approved it", "Merged in e99f817").
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
    #[serde(default)]
    pub cause: MoveCause,
    #[serde(default)]
    pub by: Mover,
    /// Its open asks, the one to answer first first (put-off ones last).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub asks: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chip: Option<CardChip>,

    // ── Facts the engine records ────────────────────────────────────────────
    #[serde(default)]
    pub finish: Finish,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed: Option<Closed>,
    /// Kept for later (the old Later column): never starts on its own.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub held: bool,
    /// When work was last started on it or sent to one of its threads (by
    /// its orchestrator, the engine, or the person from the board).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub word: Option<Word>,
    /// Its threads that put themselves on it by reporting on it (a thread
    /// may report on any card of its orchestrator's board). They show on it
    /// and their work counts as work on it, but what they report never says
    /// where it stands: that is its own threads', put on it by the person,
    /// its orchestrator or the engine.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub guests: Vec<String>,
    /// When its thread was last heard from other than by its session: a
    /// report of any kind, an outcome of its route. Work sent before that
    /// has reached it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heard_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checking: Option<Checking>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stall: Option<Stall>,
    /// When the person last opened it (a move the engine made after that
    /// shows a dot).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seen_at: Option<DateTime<Utc>>,
    /// "What happened", oldest first, at most [`CARD_EVENTS_MAX`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<CardEvent>,
    pub updated_at: DateTime<Utc>,
}

impl BoardCard {
    /// A card's row before anything is known of it.
    pub fn new(id: &str, now: DateTime<Utc>) -> Self {
        BoardCard {
            id: id.to_owned(),
            place: Place::Planned,
            status: CardStatus::Next,
            note: None,
            until: None,
            since: now,
            status_since: now,
            seq: 0,
            from: None,
            reason: String::new(),
            cause: MoveCause::Created,
            by: Mover::Engine,
            asks: Vec::new(),
            chip: None,
            finish: Finish::default(),
            closed: None,
            held: false,
            work_at: None,
            word: None,
            guests: Vec::new(),
            heard_at: None,
            checking: None,
            stall: None,
            seen_at: None,
            events: Vec::new(),
            updated_at: now,
        }
    }

    /// The engine moved it since the person last opened it.
    pub fn unseen(&self) -> bool {
        self.by != Mover::You && self.seen_at.is_none_or(|seen| seen < self.since)
    }

    /// Add a timeline line (the oldest dropped past [`CARD_EVENTS_MAX`]).
    pub fn note_event(&mut self, mut event: CardEvent) {
        event.text = super::prompt_text::one_line(&event.text, CARD_EVENT_TEXT_MAX);
        self.events.push(event);
        if self.events.len() > CARD_EVENTS_MAX {
            let over = self.events.len() - CARD_EVENTS_MAX;
            self.events.drain(..over);
        }
    }

    /// The column a build from before places shows it in. Written to
    /// [`WorkItem::state`] with every move; never read back.
    pub fn old_state(&self) -> WorkState {
        old_state(self.place, self.status)
    }
}

/// [`BoardCard::old_state`], from the place and status alone.
pub fn old_state(place: Place, status: CardStatus) -> WorkState {
    match (place, status) {
        (Place::Done, _) => WorkState::Done,
        (Place::Dropped, _) => WorkState::Dropped,
        (Place::Running, _) => WorkState::Active,
        // Something to look at: the old Review column (Approve works there).
        (Place::NeedsYou | Place::Snoozed, CardStatus::Review) => WorkState::Review,
        (Place::NeedsYou | Place::Snoozed, _) => WorkState::Blocked,
        // (A paused card was Blocked with `pausedAt`.)
        (Place::Planned, CardStatus::Paused) => WorkState::Blocked,
        (Place::Planned, CardStatus::Held) => WorkState::Inbox,
        (Place::Planned, _) => WorkState::Planned,
        (Place::Other, _) => WorkState::Other,
    }
}

/// Where a card with no [`BoardCard`] row shows (its board's engine predates
/// places, or hasn't worked it out yet): the old column, mapped.
/// `needs_you`: it has an open ask or an open PR.
pub fn place_of_old(item: &WorkItem, needs_you: bool) -> (Place, CardStatus) {
    match item.state {
        WorkState::Inbox => (Place::Planned, CardStatus::Held),
        WorkState::Planned => (Place::Planned, CardStatus::Next),
        WorkState::Active => (Place::Running, CardStatus::Working),
        WorkState::Blocked if item.paused_at.is_some() => (Place::Planned, CardStatus::Paused),
        WorkState::Blocked => (Place::NeedsYou, CardStatus::Asked),
        WorkState::Review if needs_you => (Place::NeedsYou, CardStatus::Review),
        WorkState::Review | WorkState::Done => (Place::Done, CardStatus::Done),
        WorkState::Dropped => (Place::Dropped, CardStatus::Dropped),
        WorkState::Other => (Place::Planned, CardStatus::Next),
    }
}

/// Where a card shows: by its row, when its engine works places out and
/// nothing older moved it since; else by its old column
/// ([`place_of_old`]). An engine that works places out writes the old
/// column with every move ([`BoardCard::old_state`]), so a row whose old
/// column isn't the item's was overtaken by an older writer.
pub fn place_of(item: &WorkItem, row: Option<&BoardCard>, needs_you: bool) -> (Place, CardStatus) {
    match row {
        Some(row) if row.old_state() == item.state => (row.place, row.status),
        _ => place_of_old(item, needs_you),
    }
}

impl super::frames::Keyed for BoardCard {
    fn key(&self) -> &str {
        &self.id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_place_maps_to_a_column_an_older_reader_knows() {
        use CardStatus as S;
        use Place as P;
        assert_eq!(old_state(P::Running, S::Working), WorkState::Active);
        assert_eq!(old_state(P::Running, S::Checking), WorkState::Active);
        assert_eq!(old_state(P::NeedsYou, S::Asked), WorkState::Blocked);
        assert_eq!(old_state(P::NeedsYou, S::Failed), WorkState::Blocked);
        assert_eq!(old_state(P::NeedsYou, S::Review), WorkState::Review);
        assert_eq!(old_state(P::Snoozed, S::Asked), WorkState::Blocked);
        assert_eq!(old_state(P::Planned, S::Next), WorkState::Planned);
        assert_eq!(old_state(P::Planned, S::After), WorkState::Planned);
        assert_eq!(old_state(P::Planned, S::Held), WorkState::Inbox);
        assert_eq!(old_state(P::Planned, S::Paused), WorkState::Blocked);
        assert_eq!(old_state(P::Done, S::Done), WorkState::Done);
        assert_eq!(old_state(P::Dropped, S::Dropped), WorkState::Dropped);
    }

    #[test]
    fn a_row_an_older_writer_overtook_gives_way_to_the_old_column() {
        let now: DateTime<Utc> = "2026-11-22T00:00:00Z".parse().unwrap();
        let mut item: WorkItem = serde_json::from_value(serde_json::json!({
            "id": "i1", "title": "t", "state": "active", "priority": "normal", "createdAt": now, "updatedAt": now,
        }))
        .unwrap();
        let mut row = BoardCard::new("i1", now);
        row.place = Place::Running;
        row.status = CardStatus::Checking;
        assert_eq!(place_of(&item, Some(&row), false), (Place::Running, CardStatus::Checking));
        assert_eq!(place_of(&item, None, false), (Place::Running, CardStatus::Working));
        // An older engine moved it to Done since: its row is behind.
        item.state = WorkState::Done;
        assert_eq!(place_of(&item, Some(&row), false), (Place::Done, CardStatus::Done));
    }

    #[test]
    fn a_newer_peers_words_cost_no_row() {
        let row: BoardCard = serde_json::from_value(serde_json::json!({
            "id": "i1", "place": "orbit", "status": "spinning", "cause": "gravity", "by": "moon",
            "since": "2026-11-22T00:00:00Z", "statusSince": "2026-11-22T00:00:00Z",
            "updatedAt": "2026-11-22T00:00:00Z", "finish": { "line": "vibes" },
        }))
        .unwrap();
        assert_eq!(row.place, Place::Other);
        assert_eq!(row.status, CardStatus::Other);
        assert_eq!(row.cause, MoveCause::Other);
        assert_eq!(row.by, Mover::Other);
        assert_eq!(row.finish.line, FinishKind::Other);
    }

    #[test]
    fn the_timeline_keeps_its_newest_lines() {
        let now: DateTime<Utc> = "2026-11-22T00:00:00Z".parse().unwrap();
        let mut card = BoardCard::new("i1", now);
        for n in 0..CARD_EVENTS_MAX + 5 {
            card.note_event(CardEvent {
                at: now,
                kind: EventKind::Note,
                text: format!("line {n}\nsecond line"),
                by: None,
                from: None,
                to: None,
                seq: None,
                about: None,
            });
        }
        assert_eq!(card.events.len(), CARD_EVENTS_MAX);
        assert_eq!(card.events[0].text, "line 5 second line");
    }
}
