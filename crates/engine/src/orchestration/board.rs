//! The board's places, kept true.
//!
//! `derive.rs` works out where one card is from facts. This gathers those
//! facts for the cards a change touched, applies what comes out, and runs
//! it when a fact changes:
//!
//! - **Facts** ([`EngineRpc::derive_cards`]): the card's item and row, the
//!   asks about it, its threads as `board_checks::Thread::of` reads them
//!   (this engine's own runs for the chats it hosts, a host's synced row
//!   while its heartbeat is fresh, progress rows, usage-limit holds, the
//!   resume after a restart), its orchestrator's turn, its route, its PRs
//!   as the app last saw them, and for a Merged finish line the engine's
//!   own `git merge-base --is-ancestor`.
//! - **Apply**: the card's [`BoardCard`] row (place, status, the move's
//!   number, where it came from and why, a timeline line), the old column
//!   on its item for readers from before places, the engine's own asks
//!   raised or resolved, and one message per orchestrator for its stalled
//!   cards.
//! - **When**: in the same step as the op that changed a fact (the
//!   person's and the agents' ops call [`EngineRpc::derive_cards`]
//!   themselves); a moment after a session, ask, card or route row changes
//!   here or arrives from another device ([`start`]); when a timed fact
//!   runs out; and every minute over the cards not closed. Only the cards a
//!   change touches are worked out.
//!
//! A board is kept by the engine hosting its orchestrator, else its first
//! thread, else the device of the dump it came from ([`keeper_of`]): only
//! that engine works it out on its own and acts for it (records a closing,
//! raises and resolves the engine's asks, wakes its orchestrator). Any
//! engine works a card out right after an op it ran on it and writes its
//! place when that takes no act ([`Scope`]); what does is left to the
//! keeper, which works the card out when the op's rows reach it.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Mutex;

use chrono::{DateTime, Duration as TimeDelta, Utc};
use serde_json::{Value, json};
use zeron_proto::orchestration::{
    AnswerVia, Ask, AskKind, AskOption, AskOrigin, AskState, BoardCard, CardChip, CardEvent, CardStatus, CheckWhy, Checking,
    ChipKind, CloseHow, Closed, CommKind, EventKind, FinishKind, LinkKind, MoveCause, Mover, OrchestratorKind, Place,
    Progress, ReportState, Stall, Word, WorkItem, WorkLink, WorkState, board::place_of_old, prompt_text::one_line,
};
use zeron_proto::{Chat, Session, SessionStatus};
use zeron_rpc::RpcError;

use super::Orchestration;
use super::board_checks::{Pr, RESUMED_GRACE_SECS, Thread, prs_of};
use super::comms::YOU;
use super::derive::{self, Act, Derived, Facts, Owner, Raise, RouteFact, ThreadFact, Why, derive};
use crate::rpc::EngineRpc;

/// How long after a change the touched cards are worked out (a burst, one
/// pass).
const SETTLE: std::time::Duration = std::time::Duration::from_millis(150);
/// How often every card not closed is worked out anyway (a fact that
/// changed without a row changing: a heartbeat going stale, a progress row
/// growing old).
const EVERY: std::time::Duration = std::time::Duration::from_secs(60);
/// How long a Merged card's `git` answer is kept.
const MERGE_CHECK_SECS: i64 = 20;

/// What the engine keeps between passes.
#[derive(Default)]
pub(crate) struct Board {
    cards: Mutex<HashSet<String>>,
    chats: Mutex<HashSet<String>>,
    notify: tokio::sync::Notify,
    /// When each card is due again though nothing changed.
    rechecks: Mutex<HashMap<String, DateTime<Utc>>>,
    /// Each Merged card's last `git` answer.
    merges: Mutex<MergeChecks>,
}

/// card → when `git` was last asked about its branch, and what it said.
type MergeChecks = HashMap<String, (DateTime<Utc>, Option<String>)>;

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Board {
    /// Card `id` is worked out a moment from now.
    pub(crate) fn touch(&self, id: &str) {
        lock(&self.cards).insert(id.to_owned());
        self.notify.notify_one();
    }

    /// The cards `chat` is a thread or the orchestrator of are worked out a
    /// moment from now.
    pub(crate) fn touch_chat(&self, chat: &str) {
        lock(&self.chats).insert(chat.to_owned());
        self.notify.notify_one();
    }

    fn take(&self) -> (HashSet<String>, HashSet<String>) {
        (std::mem::take(&mut *lock(&self.cards)), std::mem::take(&mut *lock(&self.chats)))
    }

    /// The cards due at `now`, taken.
    fn due(&self, now: DateTime<Utc>) -> Vec<String> {
        let mut rechecks = lock(&self.rechecks);
        let due: Vec<String> = rechecks.iter().filter(|(_, at)| **at <= now).map(|(id, _)| id.clone()).collect();
        for id in &due {
            rechecks.remove(id);
        }
        due
    }

    fn next_due(&self) -> Option<DateTime<Utc>> {
        lock(&self.rechecks).values().min().copied()
    }

    fn set_due(&self, id: &str, at: Option<DateTime<Utc>>) {
        let mut rechecks = lock(&self.rechecks);
        match at {
            Some(at) => {
                rechecks.insert(id.to_owned(), at);
            }
            None => {
                rechecks.remove(id);
            }
        }
    }
}

/// Which of the cards given a pass works out, and what it may do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scope {
    /// Right after an op this engine ran on them: every card given. One
    /// this engine doesn't keep gets its place only when that takes no act.
    Op,
    /// On its own (a change seen, a timer, the sweep): the cards this
    /// engine keeps.
    Kept,
    /// Every card given, as their keeper (a dry run over copied data, the
    /// tests' pretend other devices).
    #[cfg_attr(not(test), allow(dead_code))]
    All,
}

/// A card's move, as a pass reports it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Moved {
    pub item: String,
    pub title: String,
    pub from: Option<Place>,
    pub to: Place,
    pub status: CardStatus,
    pub reason: String,
    pub by: Mover,
    pub seq: u64,
}

impl std::fmt::Display for Moved {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} \"{}\": {} -> {} ({:?}): {}",
            self.item,
            one_line(&self.title, 60),
            self.from.map_or("-", Place::as_str),
            self.to.as_str(),
            self.status,
            self.reason
        )
    }
}

/// What one card's facts are read from, copied under the registry lock.
struct Rows {
    item: WorkItem,
    row: Option<BoardCard>,
    asks: Vec<Ask>,
    owner: Option<zeron_proto::orchestration::Orchestrator>,
    waits_on: Vec<String>,
    /// Its route: state, review line, target chat, the dump's device.
    route: Option<(zeron_proto::orchestration::RouteState, Option<String>, Option<String>, String)>,
    /// Each of its threads that another open card has too: when the one of
    /// those last given work was (`None`: none of them was yet), and which.
    shared: HashMap<String, (Option<DateTime<Utc>>, String)>,
    pr_ask: Option<Ask>,
    progress: HashMap<String, Progress>,
    /// Its threads' newest reports (their crew rows').
    reports: Vec<(String, zeron_proto::orchestration::ReportHead)>,
}

/// When a card was last touched (made, given a thread, given work): a
/// message to a thread on several open cards is for the one touched last.
fn stamp(item: &WorkItem, row: Option<&BoardCard>) -> DateTime<Utc> {
    [row.and_then(|r| r.work_at), item.threads_at, Some(item.created_at)].into_iter().flatten().max().unwrap_or(item.created_at)
}

/// Whether a thread at work is at work for another card than this one: a
/// thread on several open cards works for the one last given work. A card
/// never given work has it only when no other open card does (a card
/// planned for a busy thread isn't what that thread is doing).
fn at_work_elsewhere(mine: Option<DateTime<Utc>>, id: &str, other: Option<&(Option<DateTime<Utc>>, String)>) -> bool {
    match (mine, other) {
        (_, None) => false,
        (None, Some(_)) => true,
        (Some(_), Some((None, _))) => false,
        (Some(mine), Some((Some(theirs), other))) => (*theirs, other.as_str()) > (mine, id),
    }
}

/// The id of the ask the engine keeps for an orchestrator's open PRs
/// (`None`: the cards with no orchestrator).
pub(crate) fn pr_ask_id(orchestrator: Option<&str>) -> String {
    format!("prs-{}", orchestrator.unwrap_or("loose"))
}

/// A card's chats: its threads, else (none left) the chats it links: a
/// thread the crew archive took off it works on it again when messaged.
fn chats_of(item: &WorkItem) -> Vec<String> {
    if !item.threads.is_empty() {
        return item.threads.clone();
    }
    item.links.iter().filter(|l| l.kind == LinkKind::Chat).map(|l| l.target.clone()).collect()
}

fn rows_of(doc: &zeron_doc::RegistryDoc, id: &str) -> Option<Rows> {
    let item = doc.work_item(id).filter(|i| !i.removed)?;
    let row = doc.board_card(id);
    let threads = item.threads.clone();
    let asks = doc.asks_where(|a| {
        a.item_id.as_deref() == Some(id) || (a.item_id.is_none() && threads.contains(&a.from_chat))
    });
    let owner = item.orchestrator_id.as_deref().and_then(|o| doc.orchestrator(o));
    let waits_on = item
        .depends_on
        .iter()
        .filter(|dep| !doc.board_card(dep).is_some_and(|c| c.closed.is_some()))
        .filter_map(|dep| doc.work_item(dep).filter(|i| !i.removed))
        // (A dependency from before places: done by its old column.)
        .filter(|i| doc.board_card(&i.id).is_some() || !matches!(i.state, WorkState::Done | WorkState::Dropped))
        .map(|i| i.title)
        .collect();
    let route = item.source.as_ref().and_then(|s| {
        let dump = doc.dump(&s.dump_id)?;
        let r = dump.routes.iter().find(|r| r.id == s.route_id)?;
        Some((r.state, r.review.clone(), super::watchers::target_chat(&r.target).map(str::to_owned), dump.device_id.clone()))
    });
    let mut shared: HashMap<String, (Option<DateTime<Utc>>, String)> = HashMap::new();
    if !threads.is_empty() {
        for other in doc.work_items_where(|i| i.id != id && !i.removed && i.threads.iter().any(|t| threads.contains(t))) {
            let other_row = doc.board_card(&other.id);
            let closed = match &other_row {
                Some(r) => r.closed.is_some(),
                None => matches!(other.state, WorkState::Done | WorkState::Dropped),
            };
            if closed {
                continue;
            }
            let at = other_row.as_ref().and_then(|r| r.work_at);
            for t in other.threads.iter().filter(|t| threads.contains(t)) {
                let newer = shared.get(t).is_none_or(|(have, _)| at > *have);
                if newer {
                    shared.insert(t.clone(), (at, other.id.clone()));
                }
            }
        }
    }
    let pr_ask = doc.ask(&pr_ask_id(item.orchestrator_id.as_deref()));
    let mut progress = HashMap::new();
    for chat in chats_of(&item).iter().chain(item.orchestrator_id.iter()) {
        if let Some(p) = doc.progress(chat) {
            progress.insert(chat.clone(), p);
        }
    }
    let reports = own_reports(doc, &item, |t| shared.contains_key(t));
    Some(Rows { item, row, asks, owner, waits_on, route, shared, pr_ask, progress, reports })
}

/// The newest reports of `item`'s threads that are about it. A crew row
/// keeps only its thread's last report, with no card on it, and a thread
/// serves many cards over time: the report counts for the card that is the
/// thread's own, or, when its row names none, for the one open card it is
/// on (`shared`: its threads that another open card has too); and only a
/// report made since the card was.
fn own_reports(
    doc: &zeron_doc::RegistryDoc,
    item: &WorkItem,
    shared: impl Fn(&str) -> bool,
) -> Vec<(String, zeron_proto::orchestration::ReportHead)> {
    item.threads
        .iter()
        .filter_map(|t| doc.crew_member(t))
        .filter(|m| match m.item_id.as_deref() {
            Some(own) => own == item.id,
            None => !shared(&m.chat_id),
        })
        .filter_map(|m| m.last_report.filter(|r| r.at >= item.created_at).map(|r| (m.chat_id, r)))
        .collect()
}

/// Whether another open card has thread `chat` too.
fn on_another_open_card(doc: &zeron_doc::RegistryDoc, id: &str, chat: &str) -> bool {
    doc.work_items_where(|i| i.id != id && !i.removed && i.threads.iter().any(|t| t == chat)).into_iter().any(|other| {
        match doc.board_card(&other.id) {
            Some(row) => row.closed.is_none(),
            None => !matches!(other.state, WorkState::Done | WorkState::Dropped),
        }
    })
}

/// Who keeps a card's board.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Keeper {
    /// This device.
    Here,
    /// Another device that works places out.
    Device(String),
    /// Its hosts are all from before places: nobody works it out on its
    /// own (its own engine keeps its old column as it always did, and a
    /// reader shows it by that column once it differs from its row's:
    /// `zeron_proto::orchestration::board::place_of`).
    Older,
    /// It has no orchestrator, thread or dump: it only changes by an op,
    /// and whichever engine runs the op works it out.
    Anyone,
}

/// Who keeps `item`'s board: the first of its orchestrator's host, its
/// first thread's, and the device of the dump it came from, that works
/// places out (`places`: whether a device does).
pub(crate) fn keeper_of(
    item: &WorkItem,
    dump_device: Option<&str>,
    chats: &HashMap<String, Chat>,
    here: &str,
    places: impl Fn(&str) -> bool,
) -> Keeper {
    let hosts: Vec<&str> = [item.orchestrator_id.as_deref(), item.threads.first().map(String::as_str)]
        .into_iter()
        .flatten()
        .filter_map(|c| chats.get(c).map(|c| c.device_id.as_str()))
        .chain(dump_device)
        .collect();
    if hosts.is_empty() {
        return Keeper::Anyone;
    }
    match hosts.into_iter().find(|d| *d == here || places(d)) {
        Some(d) if d == here => Keeper::Here,
        Some(d) => Keeper::Device(d.to_owned()),
        None => Keeper::Older,
    }
}

/// A card's row from before places: its old column, mapped (the proposal's
/// table), with the facts that column implied.
pub(crate) fn seed_row(
    item: &WorkItem,
    asks: &[Ask],
    prs: &[Pr],
    reports: &[(String, zeron_proto::orchestration::ReportHead)],
    now: DateTime<Utc>,
) -> BoardCard {
    let mut row = BoardCard::new(&item.id, now);
    let open_ask = asks.iter().any(|a| a.state == AskState::Open);
    let open_pr = prs.iter().any(|p| p.open != Some(false));
    let (place, status) = place_of_old(item, open_ask || open_pr);
    row.place = place;
    row.status = status;
    row.cause = MoveCause::Mapped;
    row.since = match item.state {
        WorkState::Done | WorkState::Dropped => item.done_at.unwrap_or(item.updated_at),
        _ => item.moved_at.unwrap_or(item.created_at),
    };
    row.status_since = row.since;
    // Its thread's last word, as its crew row kept it.
    row.word = reports
        .iter()
        .filter(|(_, r)| matches!(r.state, ReportState::Done | ReportState::Blocked | ReportState::Failed))
        .max_by_key(|(_, r)| r.at)
        .map(|(from, r)| Word { state: r.state, at: r.at, from: from.clone(), summary: r.summary.clone(), report: Some(r.id.clone()) });
    let started = || item.threads_at.or(item.moved_at).or(Some(item.created_at)).filter(|_| !item.threads.is_empty());
    match item.state {
        WorkState::Inbox => row.held = true,
        // Up next with work behind it (a thread on it, or one archived off
        // it): parked by its orchestrator, which is held now. Queued for a
        // crew slot, or never started: planned.
        WorkState::Planned => {
            let worked = !item.threads.is_empty() || item.links.iter().any(|l| l.kind == LinkKind::Chat);
            if worked && item.waiting.is_none() {
                row.held = true;
                row.status = CardStatus::Held;
            }
        }
        WorkState::Other => {}
        WorkState::Active | WorkState::Blocked => row.work_at = started(),
        WorkState::Review => {
            row.work_at = started();
            if open_pr {
                row.finish.line = FinishKind::Pr;
            } else if !open_ask {
                row.closed =
                    Some(Closed { at: item.approved_at.unwrap_or(item.updated_at), by: Mover::Engine, how: CloseHow::Mapped, note: None });
            }
        }
        WorkState::Done => {
            row.closed = Some(Closed {
                at: item.done_at.unwrap_or(item.updated_at),
                by: if item.approved_at.is_some() { Mover::You } else { Mover::Engine },
                how: if item.approved_at.is_some() { CloseHow::Accepted } else { CloseHow::Mapped },
                note: None,
            });
        }
        WorkState::Dropped => {
            row.closed = Some(Closed { at: item.updated_at, by: Mover::Engine, how: CloseHow::Dropped, note: None });
        }
    }
    // (Nothing from before places shows as new.)
    if row.closed.is_some() {
        row.seen_at = Some(now);
    }
    row
}

/// The chip a card carries.
fn chip_of(item: &WorkItem, row: &BoardCard) -> Option<CardChip> {
    let pr = || {
        let link = item.links.iter().find(|l| l.kind == LinkKind::Pr)?;
        let number = link.target.trim_end_matches('/').rsplit('/').next().filter(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()));
        let label = match number {
            Some(n) => format!("PR #{n}"),
            None => "PR".to_owned(),
        };
        Some(CardChip { kind: ChipKind::Pr, label, target: Some(link.target.clone()) })
    };
    match &row.closed {
        Some(c) => match c.how {
            CloseHow::Merged => Some(CardChip {
                kind: ChipKind::Merged,
                label: match &c.note {
                    Some(sha) => format!("merged {sha}"),
                    None => "merged".into(),
                },
                target: c.note.clone(),
            }),
            CloseHow::Pr => pr(),
            CloseHow::Report => Some(CardChip { kind: ChipKind::Report, label: "report".into(), target: None }),
            CloseHow::Accepted => Some(CardChip { kind: ChipKind::Accepted, label: "approved".into(), target: None }),
            CloseHow::Closed => Some(CardChip { kind: ChipKind::Closed, label: "closed".into(), target: None }),
            CloseHow::Dropped => Some(CardChip { kind: ChipKind::Dropped, label: "dropped".into(), target: None }),
            CloseHow::Marked | CloseHow::Mapped | CloseHow::Other => pr(),
        },
        None => pr(),
    }
}

/// A move's reason, what it was, and who made it.
fn move_words(from: Option<Place>, d: &Derived, actor: Mover) -> (String, MoveCause, Mover) {
    let answered = actor == Mover::You && matches!(from, Some(Place::NeedsYou | Place::Snoozed));
    let reopened = matches!(from, Some(Place::Done | Place::Dropped));
    let (reason, cause, by): (String, MoveCause, Mover) = match &d.why {
        Why::Closed { how, by, note } => match how {
            CloseHow::Report => (
                match note {
                    Some(said) => format!("Reported done: {said}"),
                    None => "Its work is done".into(),
                },
                MoveCause::Reported,
                *by,
            ),
            CloseHow::Merged => (
                match note {
                    Some(sha) => format!("Merged in {sha}"),
                    None => "Merged".into(),
                },
                MoveCause::Merged,
                *by,
            ),
            CloseHow::Pr => ("Its PR is merged or closed".into(), MoveCause::PrClosed, *by),
            CloseHow::Accepted => ("You approved it".into(), MoveCause::Accepted, Mover::You),
            CloseHow::Marked => ("You marked it done".into(), MoveCause::Marked, Mover::You),
            CloseHow::Closed => (
                match note {
                    Some(why) => format!("Closed: {why}"),
                    None => "Closed by its orchestrator".into(),
                },
                MoveCause::Closed,
                *by,
            ),
            CloseHow::Dropped => (
                match (by, note) {
                    (Mover::You, _) => "You dropped it".into(),
                    (_, Some(why)) => format!("Dropped: {why}"),
                    _ => "Dropped".into(),
                },
                MoveCause::Dropped,
                *by,
            ),
            CloseHow::Mapped | CloseHow::Other => ("Was done".into(), MoveCause::Mapped, *by),
        },
        Why::Asked(question) => (format!("Asked you: {question}"), MoveCause::Asked, Mover::Engine),
        Why::AwaitingInput => ("Waiting for your answer in its thread".into(), MoveCause::Asked, Mover::Engine),
        Why::Failed => ("Its thread stopped on an error".into(), MoveCause::Failed, Mover::Engine),
        Why::StallAsked => ("Nothing was moving on it".into(), MoveCause::Stalled, Mover::Engine),
        Why::Review => ("Ready for you to look at".into(), MoveCause::Asked, Mover::Engine),
        Why::ThreadLive | Why::WorkSent if answered => ("You answered".into(), MoveCause::Answered, Mover::You),
        Why::ThreadLive if reopened => ("Reopened: its thread is working again".into(), MoveCause::Reopened, Mover::Engine),
        Why::ThreadLive if matches!(from, Some(Place::Planned) | None) => ("Thread started".into(), MoveCause::Started, Mover::Engine),
        Why::ThreadLive => ("Its thread is working again".into(), MoveCause::Working, Mover::Engine),
        Why::WorkSent if reopened => ("Reopened: more work was sent".into(), MoveCause::Reopened, Mover::Engine),
        Why::WorkSent => ("Work sent to its thread".into(), MoveCause::Started, Mover::Engine),
        Why::Resuming => ("Resuming after a restart".into(), MoveCause::Working, Mover::Engine),
        Why::OwnerChecking(why) => match why {
            CheckWhy::Report => ("Its thread reported".into(), MoveCause::Finished, Mover::Engine),
            CheckWhy::Answer if actor == Mover::You => ("You answered".into(), MoveCause::Answered, Mover::You),
            CheckWhy::Answer => ("Its ask was answered".into(), MoveCause::Answered, Mover::Engine),
            CheckWhy::CrewAsk => ("Its thread asked its orchestrator".into(), MoveCause::Finished, Mover::Engine),
            CheckWhy::Stall => ("Nothing was moving: its orchestrator is checking".into(), MoveCause::Stalled, Mover::Engine),
            CheckWhy::Created | CheckWhy::Other => ("Its orchestrator is on it".into(), MoveCause::Created, Mover::Engine),
        },
        Why::Settling => ("Thread finished".into(), MoveCause::Finished, Mover::Engine),
        Why::Snoozed => (
            match d.until {
                Some(until) => format!("Snoozed until {}", local_time(until)),
                None => "Snoozed".into(),
            },
            MoveCause::Snoozed,
            Mover::You,
        ),
        Why::Paused => ("Paused by you".into(), MoveCause::Paused, Mover::You),
        Why::After(dep) => (format!("Waiting on {}", one_line(dep, 80)), MoveCause::Waiting, Mover::Engine),
        Why::Held => ("Held for later".into(), MoveCause::Held, Mover::Engine),
        Why::NotStarted if from.is_none() => ("Planned".into(), MoveCause::Created, Mover::Engine),
        Why::NotStarted => ("Not started".into(), MoveCause::Created, Mover::Engine),
    };
    // What the person's own op led to is theirs.
    let by = if actor == Mover::Engine { by } else { actor };
    (reason, cause, by)
}

/// A time the person reads, in their own zone and the app's way of
/// writing it (`Nov 23 9:00 AM`): never UTC.
pub(crate) fn local_time(at: DateTime<Utc>) -> String {
    at.with_timezone(&chrono::Local).format("%b %-d %-I:%M %p").to_string()
}

/// A card's column from before places, as it was called.
fn old_column(state: WorkState, paused: bool) -> &'static str {
    match state {
        WorkState::Inbox => "Later",
        WorkState::Planned => "Up next",
        WorkState::Active => "Now",
        WorkState::Blocked if paused => "Paused",
        WorkState::Blocked => "Blocked",
        WorkState::Review => "Review",
        WorkState::Done => "Done",
        WorkState::Dropped => "Dropped",
        WorkState::Other => "another column",
    }
}

/// A timeline line.
fn event(at: DateTime<Utc>, kind: EventKind, text: impl Into<String>, about: Option<&str>) -> CardEvent {
    CardEvent { at, kind, text: text.into(), by: None, from: None, to: None, seq: None, about: about.map(str::to_owned) }
}

/// `git` in `repo`, its trimmed stdout when it succeeded.
fn git(repo: &str, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// Whether `branch`'s tip is contained in `into` in `repo` (the commit,
/// short), and carries work of the card's own: a commit made since the card
/// was (`since`). A branch nobody committed to is contained in its target
/// from the start.
pub(crate) fn merged_in(repo: &str, branch: &str, into: &str, since: DateTime<Utc>) -> Option<String> {
    let tip = git(repo, &["rev-parse", "--verify", "--quiet", &format!("{branch}^{{commit}}")])?;
    git(repo, &["rev-parse", "--verify", "--quiet", &format!("{into}^{{commit}}")])?;
    let committed: i64 = git(repo, &["log", "-1", "--format=%ct", &tip])?.parse().ok()?;
    if committed < since.timestamp() {
        return None;
    }
    git(repo, &["merge-base", "--is-ancestor", &tip, into])?;
    git(repo, &["rev-parse", "--short", &tip])
}

impl EngineRpc {
    /// Where a Merged card's branch stands: `Some(commit)` once it landed.
    /// Asked of `git` here, in its thread's folder when this device hosts
    /// it, else its orchestrator's (worktrees of one repository share their
    /// branches). Kept [`MERGE_CHECK_SECS`].
    fn merged_fact(&self, rows: &Rows, row: &BoardCard, chats: &HashMap<String, Chat>, now: DateTime<Utc>) -> Option<String> {
        let board = self.orchestration().board();
        if let Some((at, known)) = lock(&board.merges).get(&rows.item.id)
            && now - *at < TimeDelta::seconds(MERGE_CHECK_SECS)
        {
            return known.clone();
        }
        let here = self.orchestration().device_id();
        let item = &rows.item;
        let thread = item.threads.iter().filter_map(|t| chats.get(t)).find(|c| c.device_id == here);
        let owner = item.orchestrator_id.as_deref().and_then(|o| chats.get(o)).filter(|c| c.device_id == here);
        let folder = |c: &Chat| {
            c.cwd.clone().or_else(|| c.space_id.as_deref().and_then(|s| self.workspace_host().space(s).ok().flatten()).map(|s| s.path))
        };
        let branch = row
            .finish
            .branch
            .clone()
            .or_else(|| item.threads.iter().filter_map(|t| chats.get(t)).find_map(|c| c.branch.clone()))
            .or_else(|| item.links.iter().find(|l| l.kind == LinkKind::Branch).map(|l| l.target.clone()));
        let found = (|| {
            let branch = branch?;
            let repo = thread.and_then(folder).or_else(|| owner.and_then(folder))?;
            // Its target: named, else the branch its project's own checkout
            // (its orchestrator's folder) is on.
            let into = row.finish.into.clone().or_else(|| {
                let project = owner.and_then(folder)?;
                git(&project, &["symbolic-ref", "--quiet", "--short", "HEAD"])
            })?;
            if into == branch {
                return None;
            }
            merged_in(&repo, &branch, &into, item.created_at)
        })();
        lock(&board.merges).insert(item.id.clone(), (now, found.clone()));
        found
    }

    /// Cards `ids`, worked out right after an op this engine ran on them
    /// ([`Scope::Op`]). `actor`: who did it (the person's own op makes the
    /// move theirs). The cards that moved.
    pub(crate) fn derive_cards(&self, ids: &[String], actor: Mover, now: DateTime<Utc>) -> Vec<Moved> {
        self.derive_scoped(ids, actor, now, Scope::Op)
    }

    /// Work out cards `ids` at `now` and apply what comes out. The cards
    /// that moved.
    pub(crate) fn derive_scoped(&self, ids: &[String], actor: Mover, now: DateTime<Utc>, scope: Scope) -> Vec<Moved> {
        // Never while the engine stops: its shutdown interrupts every turn,
        // which is nobody's word on any card.
        if ids.is_empty() || self.sessions_ref().shutting_down() {
            return Vec::new();
        }
        let orch = self.orchestration().clone();
        let here = orch.device_id().to_owned();
        let all: Vec<Rows> = orch.workspace().read_orchestration_with(|doc| ids.iter().filter_map(|id| rows_of(doc, id)).collect());
        if all.is_empty() {
            return Vec::new();
        }
        // The chats these cards read: their threads and orchestrators.
        let mut wanted: HashSet<&str> = HashSet::new();
        let linked: Vec<Vec<String>> = all.iter().map(|r| chats_of(&r.item)).collect();
        for (r, chats) in all.iter().zip(&linked) {
            wanted.extend(chats.iter().map(String::as_str));
            wanted.extend(r.item.orchestrator_id.as_deref());
        }
        let chats: HashMap<String, Chat> = wanted
            .iter()
            .filter_map(|c| self.workspace_host().chat(c).ok().flatten().map(|chat| ((*c).to_owned(), chat)))
            .collect();
        let mut sessions: HashMap<String, Session> = orch
            .workspace()
            .watch_session_rows()
            .borrow()
            .iter()
            .filter(|s| wanted.contains(s.chat_id.as_str()))
            .map(|s| (s.chat_id.clone(), s.clone()))
            .collect();
        // (This engine's own runs are the truth for the chats it hosts.)
        let mut local: HashSet<String> = HashSet::new();
        for s in self.sessions_ref().watch_sessions().borrow().iter().filter(|s| wanted.contains(s.chat_id.as_str())) {
            local.insert(s.chat_id.clone());
            sessions.insert(s.chat_id.clone(), s.clone());
        }
        let resumed = self.resumes().resumed_since(now, TimeDelta::seconds(RESUMED_GRACE_SECS));
        let known_prs = self.known_change_requests();
        // The other devices that work places out.
        let with_places: HashSet<String> = orch
            .workspace()
            .read_devices()
            .unwrap_or_default()
            .into_iter()
            .filter(|d| d.supports(zeron_proto::capabilities::ORCHESTRATION_PLACES_V1))
            .map(|d| d.id)
            .collect();

        let mut moved = Vec::new();
        // orchestrator → its cards nothing moves on.
        let mut wakes: HashMap<String, Vec<(String, String, String)>> = HashMap::new();
        let mut pr_groups: HashSet<Option<String>> = HashSet::new();
        for (rows, linked) in all.iter().zip(&linked) {
            let item = &rows.item;
            let keeper = keeper_of(item, rows.route.as_ref().map(|r| r.3.as_str()), &chats, &here, |d| with_places.contains(d));
            // (An op on a card nobody keeps, or whose hosts are all from
            // before places, is worked out by the engine that ran it: no
            // other will, and its old column must follow for those hosts.)
            let keeps = matches!(
                (scope, &keeper),
                (Scope::All, _) | (_, Keeper::Here) | (Scope::Op, Keeper::Anyone | Keeper::Older)
            );
            if !keeps && scope == Scope::Kept {
                continue;
            }
            // Its PRs: its links, and the one its finish line names.
            let mut with_pr = item.clone();
            if let Some(url) = rows.row.as_ref().and_then(|r| r.finish.pr.clone())
                && !with_pr.links.iter().any(|l| l.kind == LinkKind::Pr && l.target == url)
            {
                with_pr.links.push(WorkLink { kind: LinkKind::Pr, label: "PR".into(), target: url });
            }
            let prs = prs_of(&with_pr, &known_prs);
            let seeded = rows.row.is_none();
            let row = rows.row.clone().unwrap_or_else(|| seed_row(item, &rows.asks, &prs, &rows.reports, now));

            let threads: Vec<ThreadFact> = linked
                .iter()
                .map(|t| {
                    let session = sessions.get(t);
                    let progress = rows.progress.get(t);
                    let standing = Thread::of(chats.get(t), session, progress, local.contains(t), now);
                    let live = standing.live();
                    let turn = session.is_some_and(|s| matches!(s.status, SessionStatus::Working | SessionStatus::AwaitingInput));
                    let limited = session
                        .filter(|_| live && !turn)
                        .and_then(|s| s.limited.as_ref())
                        .filter(|l| l.waiting())
                        .map(|l| l.until);
                    let waiting = session.filter(|_| live && !turn && limited.is_none()).and_then(|s| {
                        match (s.waiting.as_ref().and_then(|w| w.on.first()), s.running_subagents) {
                            (Some(on), _) => Some(one_line(&on.label, 60)),
                            (None, 1) => Some("1 subagent".to_owned()),
                            (None, n) if n > 1 => Some(format!("{n} subagents")),
                            _ if s.waiting.is_some() => Some(String::new()),
                            _ => None,
                        }
                    });
                    ThreadFact {
                        id: t.clone(),
                        standing,
                        waiting,
                        awaiting_input: live && session.is_some_and(|s| s.status == SessionStatus::AwaitingInput),
                        limited,
                        active_at: Thread::active_at(session, progress),
                        ended_at: session.filter(|_| !turn).and_then(|s| s.last_turn_outcome.map(|o| o.finished_at)),
                        elsewhere: at_work_elsewhere(row.work_at, &item.id, rows.shared.get(t)),
                        resuming: !live && resumed.contains(t),
                    }
                })
                // (A linked chat counts only while it works: it isn't the
                // card's thread any more.)
                .filter(|t| !item.threads.is_empty() || t.standing.live())
                .collect();
            let owner = rows.owner.as_ref().map(|o| {
                let chat = chats.get(&o.id);
                let session = sessions.get(&o.id);
                let standing = Thread::of(chat, session, rows.progress.get(&o.id), local.contains(&o.id), now);
                let turn = session.is_some_and(|s| matches!(s.status, SessionStatus::Working | SessionStatus::AwaitingInput));
                Owner {
                    wakeable: o.kind == OrchestratorKind::Standard && !o.archived && !o.paused && chat.is_some_and(|c| !c.archived),
                    live: standing.live(),
                    started_at: session.and_then(|s| s.started_at),
                    ended_at: session.filter(|_| !turn).and_then(|s| s.last_turn_outcome.map(|o| o.finished_at)),
                }
            });
            let route = rows.route.as_ref().map(|(state, review, target, _)| RouteFact {
                state: *state,
                review: review.clone(),
                ends_it: match target.as_deref() {
                    Some(t) if item.threads.is_empty() => item.orchestrator_id.as_deref() == Some(t),
                    Some(t) => item.threads.len() == 1 && item.threads[0] == t,
                    None => false,
                },
            });
            // Asked of git only once nothing works on it.
            let at_work = threads.iter().any(|t| t.standing.live() && !t.elsewhere);
            let merged = (row.finish.line == FinishKind::Merged && row.closed.is_none() && !at_work && keeps)
                .then(|| self.merged_fact(rows, &row, &chats, now))
                .flatten();
            // An ask its thread raised on no card is the card's the thread is
            // on now (one thread, several cards: one of them asks).
            let asks: Vec<&Ask> = rows
                .asks
                .iter()
                .filter(|a| a.item_id.is_some() || !at_work_elsewhere(row.work_at, &item.id, rows.shared.get(&a.from_chat)))
                .collect();
            let facts = Facts {
                item,
                card: &row,
                threads,
                asks,
                owner,
                waits_on: rows.waits_on.clone(),
                route,
                prs,
                merged,
                pr_ask: rows.pr_ask.as_ref(),
            };
            let derived = derive(&facts, now);
            // (What a stalled card's orchestrator is told of it.)
            let said = facts.card.word.as_ref().map_or_else(
                || "its thread ended without a report".to_owned(),
                |w| format!("its thread last reported {}: {}", w.state.as_str(), one_line(&w.summary, 100)),
            );
            drop(facts);
            // Acts are the keeper's: a card this engine doesn't keep is left
            // to it (it works the card out when this op's rows reach it).
            if !keeps && !derived.acts.is_empty() {
                continue;
            }
            if derived.acts.contains(&Act::JoinPrAsk) || row.finish.line == FinishKind::Pr {
                pr_groups.insert(item.orchestrator_id.clone());
            }
            let applied = self.apply_derived(rows, row, seeded, &derived, actor, now);
            orch.board().set_due(&item.id, derived.recheck.filter(|_| keeps));
            if let Some(m) = applied.moved {
                moved.push(m);
            }
            if applied.wake
                && let Some(o) = item.orchestrator_id.clone()
            {
                wakes.entry(o).or_default().push((item.id.clone(), item.title.clone(), said));
            }
            for (ask, from, question) in applied.raised {
                orch.comms().say(CommKind::Ask, &from, YOU, &question);
                tracing::info!(card = %item.id, %ask, "board: the engine asked the person about a card");
            }
            if !applied.release.is_empty()
                && let Some(rpc) = orch.rpc()
            {
                let threads = applied.release;
                tokio::spawn(async move {
                    for t in threads {
                        if let Err(error) = rpc.doc_host_ref().release_queue(&t).await {
                            tracing::warn!(%error, thread = %t, "board: held messages weren't released");
                        }
                    }
                });
            }
        }
        for group in pr_groups {
            self.sync_pr_ask(group.as_deref(), now);
        }
        for (orchestrator, cards) in wakes {
            let ids: Vec<&str> = cards.iter().map(|(id, _, _)| id.as_str()).collect();
            let injected = zeron_doc::Injected::board_check(ids.iter().copied());
            if let Err(error) = super::ops::deliver(self, &orchestrator, &stall_text(&cards), None, super::ops::When::Next, Some(injected)) {
                tracing::warn!(%error, %orchestrator, "board: couldn't wake the orchestrator for its stalled cards");
            }
        }
        if !moved.is_empty() {
            tracing::info!(moved = %moved.iter().map(ToString::to_string).collect::<Vec<_>>().join("; "), "board: cards moved");
        }
        moved
    }

    /// `derived` applied to one card (under the registry lock): its row, the
    /// old column on its item, the engine's own asks.
    fn apply_derived(&self, rows: &Rows, seed: BoardCard, seeded: bool, derived: &Derived, actor: Mover, now: DateTime<Utc>) -> Applied {
        let id = rows.item.id.clone();
        let from_chat = rows.item.orchestrator_id.clone().or_else(|| rows.item.threads.first().cloned());
        self.orchestration().workspace().write_orchestration(|doc| {
            let mut applied = Applied::default();
            let Some(mut item) = doc.work_item(&id).filter(|i| !i.removed) else { return applied };
            // (The row as it is now: an op may have written a fact since.)
            let mut row = doc.board_card(&id).unwrap_or(seed);
            let before = (!seeded).then(|| row.clone());
            let item_before = item.clone();
            let mut new_asks: Vec<String> = Vec::new();
            for act in &derived.acts {
                match act {
                    Act::Reopen => {
                        row.closed = None;
                        row.seen_at = None;
                    }
                    Act::Close { how, note } => {
                        row.closed = Some(Closed { at: now, by: Mover::Engine, how: *how, note: note.clone() });
                    }
                    Act::ClearChecking => row.checking = None,
                    Act::StallBegin { wake } => {
                        row.stall = Some(Stall { since: now, woken_at: wake.then_some(now), settled_at: None, ask: None });
                        if *wake {
                            row.checking = Some(Checking { since: now, why: CheckWhy::Stall });
                            applied.wake = true;
                        }
                    }
                    Act::StallSettled { at } => {
                        if let Some(s) = row.stall.as_mut() {
                            s.settled_at = Some(*at);
                        }
                    }
                    Act::ClearStall => row.stall = None,
                    Act::Unpause => {
                        item.paused_at = None;
                        applied.release.extend(item.threads.iter().cloned());
                    }
                    Act::Hold => row.held = true,
                    Act::DropThreads(gone) => {
                        for t in gone {
                            item.threads.retain(|x| x != t);
                            if !item.links.iter().any(|l| l.kind == LinkKind::Chat && l.target == *t) {
                                item.links.push(WorkLink { kind: LinkKind::Chat, label: "Archived thread".into(), target: t.clone() });
                            }
                        }
                    }
                    Act::Raise(raise) => {
                        let Some(from) = from_chat.clone() else { continue };
                        let (question, options, context) = derive::raised_question(&item, raise);
                        let ask = Ask {
                            id: uuid::Uuid::new_v4().simple().to_string(),
                            from_chat: from.clone(),
                            orchestrator_id: item.orchestrator_id.clone(),
                            item_id: Some(id.clone()),
                            kind: match raise {
                                Raise::Review(_) => AskKind::Approval,
                                _ => AskKind::Decision,
                            },
                            question: question.clone(),
                            options: options
                                .into_iter()
                                .map(|(label, detail)| AskOption { detail: detail.into(), ..AskOption::new(label) })
                                .collect(),
                            urgent: false,
                            state: AskState::Open,
                            answer: None,
                            created_at: now,
                            answered_at: None,
                            dump_id: None,
                            route_id: None,
                            quiet: false,
                            origin: match raise {
                                Raise::Failed => AskOrigin::BoardCheck,
                                Raise::Stalled => AskOrigin::Stall,
                                Raise::Review(_) => AskOrigin::Review,
                            },
                            turn_id: None,
                            updated_at: None,
                            escalated_at: None,
                            answered_via: None,
                            images: Vec::new(),
                            recommendation: None,
                            links: item.links.iter().filter(|l| matches!(l.kind, LinkKind::Pr | LinkKind::Branch)).cloned().collect(),
                            context,
                            deferred_until: None,
                        };
                        doc.upsert_ask(&ask);
                        if *raise == Raise::Stalled
                            && let Some(s) = row.stall.as_mut()
                        {
                            s.ask = Some(ask.id.clone());
                        }
                        new_asks.push(ask.id.clone());
                        applied.raised.push((ask.id, from, question));
                    }
                    Act::JoinPrAsk => {}
                    Act::Resolve { ask, why } => {
                        if let Some(mut a) = doc.ask(ask).filter(|a| a.state == AskState::Open) {
                            a.state = AskState::Answered;
                            a.answer = Some(format!("resolved: {why}"));
                            a.answered_at = Some(now);
                            a.answered_via = Some(AnswerVia::Resolved);
                            a.deferred_until = None;
                            doc.upsert_ask(&a);
                        }
                    }
                }
            }
            // Its asks: what the timeline says of the ones new and gone.
            let mut asks = new_asks;
            asks.extend(derived.asks.iter().cloned());
            let mut asked: Vec<(String, String)> = Vec::new();
            let mut answered: Vec<(String, String)> = Vec::new();
            // Closed by the person's answer to the engine's own question
            // (its Drop it, Close it, Looks good): the move says it, once.
            let closed_by_you = matches!(derived.why, Why::Closed { by: Mover::You, .. });
            if !seeded {
                for new in asks.iter().filter(|a| !row.asks.contains(a)) {
                    if let Some(a) = doc.ask(new) {
                        asked.push((format!("Asked you: {}", one_line(&a.question, 110)), new.clone()));
                    }
                }
                for gone in row.asks.iter().filter(|a| !asks.contains(a)) {
                    let Some(a) = doc.ask(gone) else { continue };
                    if closed_by_you && a.origin.engine() && a.answered_via == Some(AnswerVia::Person) {
                        continue;
                    }
                    let said = one_line(a.answer.as_deref().unwrap_or(""), 90);
                    let text = match (a.state, a.answered_via) {
                        (AskState::Dismissed, _) => "You dismissed its question".to_owned(),
                        (_, Some(AnswerVia::Resolved)) => continue,
                        (_, Some(AnswerVia::Orchestrator)) => format!("Its orchestrator answered: {said}"),
                        (_, Some(AnswerVia::Thread | AnswerVia::Action)) => "Answered in its thread".to_owned(),
                        (AskState::Open, _) => continue,
                        _ => format!("You answered: {said}"),
                    };
                    answered.push((text, gone.clone()));
                }
            }
            row.asks = asks;
            // Its place.
            let was = row.place;
            if seeded {
                // From before places: mapped, not moved. Nothing to animate,
                // nothing shown as new, and its time the one it had (a card
                // found done was done when its thread said so).
                row.place = derived.place;
                row.status = derived.status;
                if let Some(c) = row.closed.as_mut().filter(|c| c.at == now) {
                    c.at = row.word.as_ref().map_or(row.since, |w| w.at.max(row.since));
                }
                if let Some(c) = &row.closed {
                    row.since = c.at;
                }
                row.status_since = row.since;
                row.reason = format!("Was in {}", old_column(item.state, item.paused()));
                row.seen_at = Some(now);
                let mut line = event(row.since, EventKind::Move, row.reason.clone(), None);
                line.to = Some(derived.place);
                row.note_event(line);
            } else if was != derived.place {
                // A paused card the person resumed: theirs, and said so.
                let resumed = row.status == CardStatus::Paused
                    && derived.status != CardStatus::Paused
                    && actor == Mover::You
                    && !matches!(derived.why, Why::Closed { .. });
                let (mut reason, cause, by) = if resumed {
                    ("You resumed it".to_owned(), MoveCause::Resumed, Mover::You)
                } else {
                    move_words(Some(was), derived, actor)
                };
                // The move says what was asked or answered: no second line.
                match cause {
                    MoveCause::Asked if !asked.is_empty() => reason = asked.remove(0).0,
                    MoveCause::Answered if !answered.is_empty() => reason = answered.remove(0).0,
                    _ => {}
                }
                row.seq += 1;
                row.from = Some(was);
                row.place = derived.place;
                row.since = now;
                row.reason = reason.clone();
                row.cause = cause;
                row.by = by;
                // (A move the person made needs no dot.)
                if by == Mover::You {
                    row.seen_at = Some(now);
                }
                let mut line = event(now, EventKind::Move, reason.clone(), None);
                line.by = Some(by);
                line.from = Some(was);
                line.to = Some(derived.place);
                line.seq = Some(row.seq);
                row.note_event(line);
                applied.moved = Some(Moved {
                    item: id.clone(),
                    title: item.title.clone(),
                    from: Some(was),
                    to: derived.place,
                    status: derived.status,
                    reason,
                    by,
                    seq: row.seq,
                });
            }
            for (text, about) in asked {
                row.note_event(event(now, EventKind::Ask, text, Some(&about)));
            }
            for (text, about) in answered {
                row.note_event(event(now, EventKind::Answer, text, Some(&about)));
            }
            if row.status != derived.status {
                row.status = derived.status;
                row.status_since = now;
            }
            row.note = derived.note.clone();
            row.until = derived.until;
            row.chip = chip_of(&item, &row);
            if before.as_ref() != Some(&row) {
                row.updated_at = now;
                doc.upsert_board_card(&row);
            }
            // The column a reader from before places shows it in.
            let old = row.old_state();
            if item.state != old {
                if old == WorkState::Done {
                    item.done_at = Some(row.closed.as_ref().map_or(now, |c| c.at));
                } else if item.state == WorkState::Done {
                    item.done_at = None;
                }
                item.state = old;
            }
            // (The old check's notes mean nothing now.)
            item.check = None;
            if item.state != WorkState::Planned {
                item.waiting = None;
            }
            if item != item_before {
                item.updated_at = now;
                doc.upsert_work_item(&item);
            }
            applied
        })
    }

    /// The ask the engine keeps for `orchestrator`'s cards whose PR is open
    /// and whose work rests: one for all of them (opened, kept up to date,
    /// closed when none is left), which the person may put off.
    fn sync_pr_ask(&self, orchestrator: Option<&str>, now: DateTime<Utc>) {
        let orch = self.orchestration();
        let id = pr_ask_id(orchestrator);
        let raised = orch.workspace().write_orchestration(|doc| {
            let waiting: Vec<(WorkItem, BoardCard)> = doc
                .work_items_where(|i| !i.removed && i.orchestrator_id.as_deref() == orchestrator)
                .into_iter()
                .filter_map(|i| doc.board_card(&i.id).map(|c| (i, c)))
                .filter(|(_, c)| {
                    c.finish.line == FinishKind::Pr
                        && c.closed.is_none()
                        && c.status == CardStatus::Review
                        && matches!(c.place, Place::NeedsYou | Place::Snoozed)
                })
                .collect();
            let current = doc.ask(&id);
            if waiting.is_empty() {
                if let Some(mut a) = current.filter(|a| a.state == AskState::Open) {
                    a.state = AskState::Answered;
                    a.answer = Some("resolved: no PR is waiting".into());
                    a.answered_at = Some(now);
                    a.answered_via = Some(AnswerVia::Resolved);
                    a.deferred_until = None;
                    doc.upsert_ask(&a);
                }
                return None;
            }
            let from = orchestrator.map(str::to_owned).or_else(|| waiting.iter().find_map(|(i, _)| i.threads.first().cloned()))?;
            let question = match waiting.len() {
                1 => format!("1 PR to review: {}", one_line(&waiting[0].0.title, 80)),
                n => format!("{n} PRs to review"),
            };
            let context: String =
                waiting.iter().map(|(i, _)| format!("- {}", one_line(&i.title, 100))).collect::<Vec<_>>().join("\n");
            let links: Vec<WorkLink> =
                waiting.iter().flat_map(|(i, _)| i.links.iter().filter(|l| l.kind == LinkKind::Pr).cloned()).take(20).collect();
            let open = current.as_ref().is_some_and(|a| a.state == AskState::Open);
            let mut ask = match current {
                Some(a) if open => a,
                // Opened anew (its last round was answered).
                other => Ask {
                    id: id.clone(),
                    from_chat: from.clone(),
                    orchestrator_id: orchestrator.map(str::to_owned),
                    item_id: None,
                    kind: AskKind::Approval,
                    question: String::new(),
                    options: Vec::new(),
                    urgent: false,
                    state: AskState::Open,
                    answer: None,
                    created_at: now,
                    answered_at: None,
                    dump_id: None,
                    route_id: None,
                    quiet: other.is_some(),
                    origin: AskOrigin::Review,
                    turn_id: None,
                    updated_at: None,
                    escalated_at: None,
                    answered_via: None,
                    images: Vec::new(),
                    recommendation: None,
                    links: Vec::new(),
                    context: None,
                    deferred_until: None,
                },
            };
            if ask.question == question && ask.context.as_deref() == Some(context.as_str()) && ask.links == links {
                return None;
            }
            let fresh = !open;
            if open {
                ask.updated_at = Some(now);
            }
            ask.question = question.clone();
            ask.context = Some(context);
            ask.links = links;
            doc.upsert_ask(&ask);
            fresh.then_some((from, question))
        });
        if let Some((from, question)) = raised {
            orch.comms().say(CommKind::Ask, &from, YOU, &question);
        }
    }

    /// The cards `chat` is a thread of (or, with a turn of its own on them,
    /// the orchestrator of).
    pub(crate) fn cards_of_chat(&self, chat: &str) -> Vec<String> {
        self.orchestration().workspace().read_orchestration_with(|doc| {
            let mut ids: Vec<String> = doc
                .work_items_where(|i| {
                    !i.removed
                        && (i.threads.iter().any(|t| t == chat)
                            || i.links.iter().any(|l| l.kind == LinkKind::Chat && l.target == chat)
                            || i.orchestrator_id.as_deref() == Some(chat))
                })
                .into_iter()
                // Its orchestrator's turn matters only to the cards that turn
                // is on (or is awaited on), and to the ones it works itself.
                .filter(|i| {
                    i.orchestrator_id.as_deref() != Some(chat)
                        || i.threads.iter().any(|t| t == chat)
                        || doc.board_card(&i.id).is_none_or(|c| {
                            c.closed.is_none() && (c.checking.is_some() || c.stall.is_some() || i.threads.is_empty())
                        })
                })
                .map(|i| i.id)
                .collect();
            ids.dedup();
            ids
        })
    }

    /// One pass: the cards touched since the last, the ones due, and (on
    /// `sweep`) every card not closed that this engine keeps. The moves.
    pub(crate) fn board_pass(&self, sweep: bool, now: DateTime<Utc>) -> Vec<Moved> {
        let board = self.orchestration().board();
        let (mut cards, chats) = board.take();
        for chat in chats {
            cards.extend(self.cards_of_chat(&chat));
        }
        cards.extend(board.due(now));
        if sweep {
            cards.extend(self.orchestration().workspace().read_orchestration_with(|doc| {
                let closed: HashSet<String> = doc.board_cards_where(|c| c.closed.is_some()).into_iter().map(|c| c.id).collect();
                doc.work_items_where(|i| !i.removed && !closed.contains(&i.id)).into_iter().map(|i| i.id).collect::<Vec<_>>()
            }));
        }
        let mut ids: Vec<String> = cards.into_iter().collect();
        ids.sort();
        self.derive_scoped(&ids, Mover::Engine, now, Scope::Kept)
    }

    /// Every card not removed, worked out as its keeper ([`Scope::All`]).
    #[cfg(test)]
    pub(crate) fn board_pass_all(&self, now: DateTime<Utc>) -> Vec<Moved> {
        let ids: Vec<String> = self
            .orchestration()
            .workspace()
            .read_orchestration_with(|doc| doc.work_items_where(|i| !i.removed).into_iter().map(|i| i.id).collect());
        self.derive_scoped(&ids, Mover::Engine, now, Scope::All)
    }

    /// Cards `ids`, worked out as their keeper ([`Scope::All`]).
    #[cfg(test)]
    pub(crate) fn derive_all(&self, ids: &[String], now: DateTime<Utc>) -> Vec<Moved> {
        self.derive_scoped(ids, Mover::Engine, now, Scope::All)
    }

    /// Work was sent to `chat` at `now` (its first prompt, a message from
    /// its orchestrator or another agent, an answer, a resume): the card
    /// the message is for knows. The card: of the open ones `chat` is on,
    /// the one touched last (made, given the thread, given work: a card
    /// made for a thread and then messaged is that message's); with none
    /// open, the newest closed one (which reopens, when an agent's word
    /// closed it).
    pub(crate) fn work_sent(&self, chat: &str, now: DateTime<Utc>) {
        self.work_sent_on(chat, None, now);
    }

    /// [`Self::work_sent`], for a message that says which card it is about
    /// (`named`: a comment on a card, changes asked on it, its resume): that
    /// card, when `chat` is on it.
    pub(crate) fn work_sent_on(&self, chat: &str, named: Option<&str>, now: DateTime<Utc>) {
        // (No card is written while Orchestrators is off.)
        let Some(orch) = self.active_orchestration() else { return };
        let card = orch.workspace().write_orchestration(|doc| {
            let on: Vec<(WorkItem, Option<BoardCard>)> = doc
                .work_items_where(|i| !i.removed && i.threads.iter().any(|t| t == chat))
                .into_iter()
                .map(|i| {
                    let row = doc.board_card(&i.id);
                    (i, row)
                })
                .collect();
            let closed = |(i, row): &(WorkItem, Option<BoardCard>)| match row {
                Some(r) => r.closed.is_some(),
                None => matches!(i.state, WorkState::Done | WorkState::Dropped),
            };
            let newest = |list: Vec<&(WorkItem, Option<BoardCard>)>| {
                list.into_iter().max_by_key(|(i, row)| (stamp(i, row.as_ref()), i.id.clone())).map(|(i, _)| i.id.clone())
            };
            let pick = named
                .filter(|id| on.iter().any(|(i, _)| i.id == *id))
                .map(str::to_owned)
                .or_else(|| newest(on.iter().filter(|c| !closed(c)).collect()))
                .or_else(|| newest(on.iter().collect()))?;
            // (A card from before places gets its row when it's worked out.)
            if let Some(mut row) = doc.board_card(&pick) {
                row.work_at = Some(now);
                row.updated_at = now;
                doc.upsert_board_card(&row);
            }
            Some(pick)
        });
        // (Worked out by the op that sent it, when it's one of the board's;
        // else a moment from now.)
        if let Some(card) = card {
            orch.board().touch(&card);
        }
    }

    /// Change card `id`'s row (made from its old column when it has none
    /// yet). `false`: no such card.
    pub(crate) fn with_card(&self, id: &str, now: DateTime<Utc>, f: impl FnOnce(&mut BoardCard, &mut WorkItem)) -> bool {
        let known = self.known_change_requests();
        self.orchestration().workspace().write_orchestration(|doc| {
            let Some(mut item) = doc.work_item(id).filter(|i| !i.removed) else { return false };
            let mut row = doc.board_card(id).unwrap_or_else(|| {
                let threads = item.threads.clone();
                let asks = doc.asks_where(|a| {
                    a.item_id.as_deref() == Some(id) || (a.item_id.is_none() && threads.contains(&a.from_chat))
                });
                seed_row(&item, &asks, &prs_of(&item, &known), &own_reports(doc, &item, |t| on_another_open_card(doc, id, t)), now)
            });
            let item_before = item.clone();
            f(&mut row, &mut item);
            row.updated_at = now;
            doc.upsert_board_card(&row);
            if item != item_before {
                item.updated_at = now;
                doc.upsert_work_item(&item);
            }
            true
        })
    }

    /// Whether card `id` waits for the person to look at its result (an
    /// approval approves it; anything else asks for changes).
    pub(crate) fn card_in_review(&self, id: &str) -> bool {
        let Some(item) = self.orchestration().item(id) else { return false };
        let (place, status) = zeron_proto::orchestration::board::place_of(&item, self.board_card(id).as_ref(), true);
        status == CardStatus::Review && matches!(place, Place::NeedsYou | Place::Snoozed)
    }

    /// A card's row (`None`: none yet, or no such card).
    pub(crate) fn board_card(&self, id: &str) -> Option<BoardCard> {
        self.orchestration().workspace().read_orchestration_with(|doc| doc.board_card(id))
    }

    /// Whether a thread is at work on card `id` (working, held on its own
    /// work, or with work on its way).
    pub(crate) fn card_at_work(&self, id: &str, now: DateTime<Utc>) -> bool {
        let Some(item) = self.orchestration().item(id) else { return false };
        let in_flight = self
            .board_card(id)
            .and_then(|c| c.work_at)
            .is_some_and(|sent| now - sent < TimeDelta::seconds(derive::IN_FLIGHT_SECS));
        in_flight || item.threads.iter().any(|t| self.sessions_ref().turn_in_flight(t) || super::ops::chat_status(self, t) == SessionStatus::Working)
    }

    /// The person or an orchestrator closes card `id` (see
    /// [`OrchestrateOp::CloseItem`]).
    ///
    /// [`OrchestrateOp::CloseItem`]: zeron_proto::orchestration::OrchestrateOp::CloseItem
    pub(crate) fn close_item(&self, id: &str, by: Mover, reason: Option<String>, dropped: bool, now: DateTime<Utc>) -> Result<Value, RpcError> {
        let orch = self.orchestration();
        let card = orch.item(id).ok_or_else(|| RpcError::Failed(format!("No board item {id}")))?;
        let reason = reason.map(|r| one_line(&r, 200)).filter(|r| !r.is_empty());
        if by != Mover::You {
            if reason.is_none() {
                return Err(RpcError::BadParams("Say why it's closed (`reason`)".into()));
            }
            if card.flow.is_some() {
                return Err(RpcError::Failed(
                    "That card is a step of its flow's run: it closes when its step reports done".into(),
                ));
            }
            if card.paused() {
                return Err(RpcError::Failed("That card is paused by the person: it stays until they resume it".into()));
            }
            if self.card_at_work(id, now) {
                return Err(RpcError::Failed(
                    "A thread is working on that card: it closes when its work ends (stop the thread first if the work isn't needed)".into(),
                ));
            }
        }
        let how = match (dropped, by) {
            (true, _) => CloseHow::Dropped,
            (false, Mover::You) => CloseHow::Marked,
            (false, _) => CloseHow::Closed,
        };
        self.with_card(id, now, |row, item| {
            row.closed = Some(Closed { at: now, by, how, note: reason.clone() });
            row.held = false;
            // (Done or dropped, it's no longer paused.)
            item.paused_at = None;
        });
        if by == Mover::You {
            // Closed by the person, its questions are settled.
            self.dismiss_asks_of(id, if dropped { "the card was dropped" } else { "the card was marked done" }, now);
            let to = card.orchestrator_id.as_deref().unwrap_or(YOU);
            orch.comms().say(CommKind::Message, YOU, to, &format!("{}: {}", if dropped { "Dropped" } else { "Done" }, one_line(&card.title, 80)));
        }
        self.derive_cards(&[id.to_owned()], by, now);
        Ok(json!({ "id": id }))
    }

    /// Card `id`'s open asks an agent raised, dismissed (`why`).
    fn dismiss_asks_of(&self, id: &str, why: &str, now: DateTime<Utc>) {
        self.orchestration().workspace().write_orchestration(|doc| {
            for mut a in doc.asks_where(|a| a.state == AskState::Open && a.item_id.as_deref() == Some(id) && !a.origin.engine()) {
                a.state = AskState::Dismissed;
                a.answer = Some(format!("resolved: {why}"));
                a.answered_at = Some(now);
                a.answered_via = Some(AnswerVia::Resolved);
                doc.upsert_ask(&a);
            }
        });
    }

    /// The person accepts card `id` ("Looks good", or an older app's
    /// Approve): done.
    pub(crate) fn accept_item(&self, id: &str, now: DateTime<Utc>) -> Result<Value, RpcError> {
        let orch = self.orchestration();
        let card = orch.item(id).ok_or_else(|| RpcError::Failed(format!("No board item {id}")))?;
        self.with_card(id, now, |row, item| {
            row.closed = Some(Closed { at: now, by: Mover::You, how: CloseHow::Accepted, note: None });
            row.held = false;
            item.approved_at = Some(now);
            item.paused_at = None;
        });
        let to = card.threads.first().cloned().or(card.orchestrator_id.clone()).unwrap_or_else(|| id.to_owned());
        orch.comms().say(CommKind::Answer, YOU, &to, &format!("Approved: {}", one_line(&card.title, 80)));
        self.derive_cards(&[id.to_owned()], Mover::You, now);
        Ok(json!({ "id": id }))
    }

    /// The person reopens closed card `id`: it goes where its facts put it.
    pub(crate) fn reopen_item(&self, id: &str, now: DateTime<Utc>) -> Result<Value, RpcError> {
        if !self.with_card(id, now, |row, item| {
            row.closed = None;
            // (Its thread's old word doesn't close it again.)
            row.word = None;
            row.stall = None;
            item.approved_at = None;
        }) {
            return Err(RpcError::Failed(format!("No board item {id}")));
        }
        self.derive_cards(&[id.to_owned()], Mover::You, now);
        Ok(json!({ "id": id }))
    }

    /// The person keeps card `id` for later, or lets it go.
    pub(crate) fn hold_item(&self, id: &str, held: bool, now: DateTime<Utc>) -> Result<Value, RpcError> {
        if !self.with_card(id, now, |row, _| {
            row.held = held;
            if !held {
                row.stall = None;
            }
        }) {
            return Err(RpcError::Failed(format!("No board item {id}")));
        }
        self.derive_cards(&[id.to_owned()], Mover::You, now);
        Ok(json!({ "id": id }))
    }

    /// The person opened card `id`.
    pub(crate) fn seen_item(&self, id: &str, now: DateTime<Utc>) -> Result<Value, RpcError> {
        let seen = self.orchestration().workspace().write_orchestration(|doc| {
            let Some(mut row) = doc.board_card(id) else { return false };
            row.seen_at = Some(now);
            doc.upsert_board_card(&row);
            true
        });
        if !seen && self.orchestration().item(id).is_none() {
            return Err(RpcError::Failed(format!("No board item {id}")));
        }
        Ok(json!({ "id": id }))
    }

    /// The person answered one of the engine's own asks on a card with
    /// `picked` (an option's label) or `text`: the engine does it itself.
    /// `true` when it did (nothing is delivered to the asker then).
    pub(crate) fn answer_engine_ask(&self, ask: &Ask, picked: Option<&str>, text: &str, now: DateTime<Utc>) -> Result<bool, RpcError> {
        let Some(id) = ask.item_id.as_deref().filter(|_| ask.origin.engine()) else { return Ok(false) };
        let Some(card) = self.orchestration().item(id) else { return Ok(false) };
        let thread = card
            .threads
            .iter()
            .find(|t| self.workspace_host().chat(t).ok().flatten().is_some_and(|c| !c.archived))
            .cloned();
        let title = one_line(&card.title, 80);
        match picked {
            Some(derive::DROP) => {
                self.close_item(id, Mover::You, None, true, now)?;
            }
            Some(derive::CLOSE) => {
                self.close_item(id, Mover::You, None, false, now)?;
            }
            Some(derive::LOOKS_GOOD) => {
                self.accept_item(id, now)?;
            }
            Some(label @ (derive::RETRY | derive::CONTINUE)) => {
                let Some(to) = thread.or(card.orchestrator_id.clone()) else { return Ok(false) };
                let message = if label == derive::RETRY {
                    format!("[Retry] The person asked to run \"{title}\" again: carry on where it stopped.")
                } else {
                    format!("[Continue] The person wants \"{title}\" carried on: go on from where you stopped, and report when it's done.")
                };
                // (Carried on, it's no longer left alone.)
                self.with_card(id, now, |row, _| row.held = false);
                super::ops::deliver(self, &to, &message, None, super::ops::When::Next, Some(zeron_doc::Injected::resumed(id)))?;
                self.orchestration().comms().say(CommKind::Answer, YOU, &to, label);
                // (To its orchestrator: its turn on the card.)
                if Some(&to) == card.orchestrator_id.as_ref() {
                    self.with_card(id, now, |row, _| row.checking = Some(Checking { since: now, why: CheckWhy::Answer }));
                }
                self.derive_cards(&[id.to_owned()], Mover::You, now);
            }
            // Words, on something to look at: what to change.
            None if ask.origin == AskOrigin::Review && !text.trim().is_empty() => {
                self.request_changes(id, text.trim())?;
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
}

/// What applying a card's place left to do outside the registry lock.
#[derive(Default)]
struct Applied {
    moved: Option<Moved>,
    /// Its orchestrator is woken for it.
    wake: bool,
    /// Asks raised: (id, from, question).
    raised: Vec<(String, String, String)>,
    /// Threads whose held messages go now.
    release: Vec<String>,
}

/// What a stalled card's orchestrator is told, once.
pub(crate) fn stall_text(cards: &[(String, String, String)]) -> String {
    let mut out = String::from(
        "[Board] Nothing is moving on these cards of yours. For each, do one: send its thread on (`send_message`), ask the person what only they can decide (`ask`, on the card), or close it saying why (`item_close`). Any still unmoved 2 minutes after your turn goes to the person as a question.",
    );
    for (id, title, said) in cards {
        out.push_str(&format!("\n- \"{}\" ({}): {}", one_line(title, 80), one_line(id, 40), one_line(said, 160)));
    }
    out
}

/// A crew report reached its orchestrator's host (`doc_host/crew_inbox.rs`,
/// on the engine that keeps the card): its card's timeline says so; a done,
/// blocked or failed one is the thread's word; one that wakes the
/// orchestrator puts its turn on the card.
pub(crate) fn on_report(workspace: &crate::workspace_host::WorkspaceHost, report: &zeron_proto::orchestration::Report) {
    let Some(id) = report.item_id.as_deref() else { return };
    workspace.write_orchestration(|doc| {
        // (A card from before places gets its row, and this word from its
        // crew row, when it's first worked out.)
        let Some(mut row) = doc.board_card(id) else { return };
        if row.events.iter().any(|e| e.kind == EventKind::Report && e.about.as_deref() == Some(report.id.as_str())) {
            return;
        }
        let Some(item) = doc.work_item(id).filter(|i| !i.removed) else { return };
        // Whose word it is: the card's own thread's. A thread may report on
        // any card of its orchestrator's board (which puts it on that card,
        // as its guest), but only one the person, its orchestrator or the
        // engine put there says where the card stands: its own item, or a
        // card it was on before it spoke and never joined by a report. A
        // flow step's card is its run's to end, never a report's.
        let from = report.from_chat.as_str();
        let its_item = doc.crew_member(from).is_some_and(|m| m.item_id.as_deref() == Some(id));
        let on_it = item.threads.iter().any(|t| t == from);
        let before = on_it && item.threads_at.is_none_or(|at| at < report.at);
        // (Where the report was filed says so, in the write that puts the
        // thread on the card. An engine from before places doesn't: there
        // the thread came onto the card as it spoke.)
        if !its_item && on_it && !before && joined_as_it_spoke(&item, report.at) && !row.guests.iter().any(|g| g == from) {
            row.guests.push(from.to_owned());
        }
        let own = its_item || (before && !row.guests.iter().any(|g| g == from));
        let speaks = own && item.flow.is_none();
        if speaks && matches!(report.state, ReportState::Done | ReportState::Blocked | ReportState::Failed) {
            row.word = Some(Word {
                state: report.state,
                at: report.at,
                from: report.from_chat.clone(),
                summary: report.summary.clone(),
                report: Some(report.id.clone()),
            });
        }
        if own {
            row.heard_at = Some(row.heard_at.map_or(report.at, |h| h.max(report.at)));
        }
        if report.state.wakes() && row.closed.is_none() {
            row.checking = Some(Checking { since: report.at, why: CheckWhy::Report });
        }
        let said = match report.state {
            ReportState::Done => "Reported done",
            ReportState::Blocked => "Reported blocked",
            ReportState::Failed => "Reported failed",
            ReportState::Message => "Told its orchestrator",
            _ => "Reported",
        };
        row.note_event(event(report.at, EventKind::Report, format!("{said}: {}", report.summary), Some(&report.id)));
        row.updated_at = report.at;
        doc.upsert_board_card(&row);
    });
}

/// How long after a report is made its filing puts its thread on the card
/// it names (the same step, on one clock).
const JOINED_WITHIN_SECS: i64 = 5;

/// Whether `item`'s threads last changed as a report made at `at` was filed:
/// the report's own thread joining it.
fn joined_as_it_spoke(item: &WorkItem, at: DateTime<Utc>) -> bool {
    item.threads_at.is_some_and(|joined| joined >= at && joined - at <= chrono::Duration::seconds(JOINED_WITHIN_SECS))
}

/// A report from `chat` has put it on card `id`, in this same write: unless
/// the card is its own item, it is the card's guest from here on, and what
/// it reports never says where the card stands ([`on_report`]). (A card
/// from before places has no row yet: its report is read as it was then,
/// when its row is made.)
pub(crate) fn joined_by_report(doc: &mut zeron_doc::RegistryDoc, id: &str, chat: &str, at: DateTime<Utc>) {
    if doc.crew_member(chat).is_some_and(|m| m.item_id.as_deref() == Some(id)) {
        return;
    }
    let Some(mut row) = doc.board_card(id) else { return };
    if !row.guests.iter().any(|g| g == chat) {
        row.guests.push(chat.to_owned());
        row.updated_at = at;
        doc.upsert_board_card(&row);
    }
}

/// What of a session a card's place reads.
type Sig = (SessionStatus, Option<String>, Option<(zeron_proto::DoneStatus, DateTime<Utc>)>, bool, Option<DateTime<Utc>>);

fn sig(s: &Session) -> Sig {
    (
        s.status,
        s.last_completed_turn.clone(),
        s.last_turn_outcome.map(|o| (o.status, o.finished_at)),
        s.waiting.is_some() || s.running_subagents > 0,
        s.limited.as_ref().map(|l| l.since),
    )
}

/// The chats of `sessions` that changed in a way a card's place reads.
fn changed_chats(seen: &mut HashMap<String, Sig>, sessions: &[Session]) -> Vec<String> {
    let mut changed = Vec::new();
    for s in sessions {
        let now = sig(s);
        if seen.get(&s.chat_id) != Some(&now) {
            seen.insert(s.chat_id.clone(), now);
            changed.push(s.chat_id.clone());
        }
    }
    changed
}

fn mark(h: impl Hash) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    h.hash(&mut hasher);
    hasher.finish()
}

/// What the rows said that a place reads, when last looked at: a row whose
/// mark changed touches its card (an ask, its card or its asker's cards).
/// Only the kinds that changed are walked, and nothing is copied for a row
/// that didn't.
#[derive(Default)]
struct Marks {
    /// Each kind's version when it was last walked.
    versions: [u64; 4],
    items: HashMap<String, u64>,
    cards: HashMap<String, u64>,
    asks: HashMap<String, u64>,
    /// item → its route's mark.
    routes: HashMap<String, u64>,
}

impl Marks {
    /// The cards and chats touched by what changed since the last look.
    fn changed(&mut self, orch: &Orchestration) -> (Vec<String>, Vec<String>) {
        let (mut cards, mut chats): (Vec<String>, Vec<String>) = Default::default();
        let dumps_changed = orch.workspace().read_orchestration_with(|doc| {
            use zeron_doc::registry::{KIND_ASKS, KIND_BOARD_CARDS, KIND_DUMPS, KIND_WORK_ITEMS};
            let now = [KIND_WORK_ITEMS, KIND_BOARD_CARDS, KIND_ASKS, KIND_DUMPS].map(|k| doc.kind_version(k));
            if now[0] != self.versions[0] {
                cards.extend(doc.changed_work_items(
                    &mut self.items,
                    |i| mark((&i.threads, i.paused_at, i.removed, &i.depends_on, &i.orchestrator_id, i.links.len(), i.threads_at)),
                    |i| i.id.clone(),
                ));
            }
            // (Only the facts: what the engine worked out isn't news to it.)
            if now[1] != self.versions[1] {
                cards.extend(doc.changed_board_cards(
                    &mut self.cards,
                    |c| {
                        mark((
                            c.closed.as_ref().map(|x| x.at),
                            c.held,
                            c.work_at,
                            c.word.as_ref().map(|w| w.at),
                            c.heard_at,
                            c.checking.as_ref().map(|x| x.since),
                            c.finish.line,
                            &c.finish.branch,
                            &c.finish.into,
                            &c.finish.pr,
                        ))
                    },
                    |c| c.id.clone(),
                ));
            }
            if now[2] != self.versions[2] {
                for (item, asker) in doc.changed_asks(
                    &mut self.asks,
                    |a| mark((std::mem::discriminant(&a.state), a.deferred_until, a.urgent)),
                    |a| (a.item_id.clone(), a.from_chat.clone()),
                ) {
                    match item {
                        Some(item) => cards.push(item),
                        None => chats.push(asker),
                    }
                }
            }
            let dumps_changed = now[3] != self.versions[3];
            self.versions = now;
            dumps_changed
        });
        if dumps_changed {
            orch.scan(|state| {
                for r in state.dumps.iter().flat_map(|d| &d.routes) {
                    let Some(item) = r.item_id.as_deref() else { continue };
                    let now = mark((std::mem::discriminant(&r.state), r.review.is_some()));
                    if self.routes.get(item) != Some(&now) {
                        self.routes.insert(item.to_owned(), now);
                        cards.push(item.to_owned());
                    }
                }
            });
        }
        (cards, chats)
    }
}

/// The board's loop: after the start-up pass (the chats a stop left at work
/// resumed, then every card worked out: `restart_resume.rs`), a pass a moment after every
/// change, when a card falls due, and every [`EVERY`].
pub(crate) fn start(orchestration: Orchestration) {
    // What changed: session rows (here and synced) by chat; cards, asks and
    // routes by their marks.
    let watch = orchestration.clone();
    orchestration.spawn(async move {
        let mut state_rx = watch.subscribe();
        let mut rows_rx = watch.workspace().watch_session_rows();
        let Some(rpc) = watch.rpc() else { return };
        let mut sessions_rx = rpc.sessions_ref().watch_sessions();
        drop(rpc);
        let (mut seen_rows, mut seen_here): (HashMap<String, Sig>, HashMap<String, Sig>) = Default::default();
        // (What is there at the start is the start-up pass's.)
        let mut marks = Marks::default();
        marks.changed(&watch);
        loop {
            tokio::select! {
                c = state_rx.changed() => {
                    if c.is_err() { break }
                    let perf_t = std::time::Instant::now();
                    let (cards, chats) = marks.changed(&watch);
                    for id in &cards {
                        watch.board().touch(id);
                    }
                    for chat in &chats {
                        watch.board().touch_chat(chat);
                    }
                    tracing::debug!(target: "perf", what = "board_marks", cards = cards.len() as u64, us = perf_t.elapsed().as_micros() as u64);
                }
                c = rows_rx.changed() => {
                    if c.is_err() { break }
                    let changed = changed_chats(&mut seen_rows, &rows_rx.borrow_and_update());
                    for chat in changed {
                        watch.board().touch_chat(&chat);
                    }
                }
                c = sessions_rx.changed() => {
                    if c.is_err() { break }
                    let changed = changed_chats(&mut seen_here, &sessions_rx.borrow_and_update());
                    for chat in changed {
                        watch.board().touch_chat(&chat);
                    }
                }
            }
        }
    });
    let run = orchestration.clone();
    run.spawn(async move {
        // The start-up pass is the engine's start-up resume's
        // (`restart_resume.rs`); turned on after it, it's had here.
        if let Some(rpc) = orchestration.rpc() {
            rpc.resumes().started_up().await;
        }
        if orchestration.take_start_up()
            && let Some(rpc) = orchestration.rpc()
        {
            let (resumed, moved) = rpc.start_up_pass_on().await;
            tracing::info!(?resumed, cards = moved.len(), "board: start-up pass (turned on)");
        }
        let mut tick = tokio::time::interval(EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        tick.tick().await;
        loop {
            let due = orchestration.board().next_due().map(|at| {
                let wait = (at - Utc::now()).to_std().unwrap_or_default();
                tokio::time::Instant::now() + wait
            });
            let sweep = tokio::select! {
                _ = tick.tick() => true,
                _ = orchestration.board().notify.notified() => {
                    tokio::time::sleep(SETTLE).await;
                    false
                }
                _ = async { match due { Some(at) => tokio::time::sleep_until(at).await, None => std::future::pending().await } } => false,
            };
            if let Some(rpc) = orchestration.rpc() {
                let perf_t = std::time::Instant::now();
                // (A send the stop cut off is found when its chat's commands
                // are next drained: its chat is resumed then.)
                if sweep {
                    rpc.resume_cut_off_sends();
                    rpc.hold_paused_queues();
                }
                let moved = rpc.board_pass(sweep, Utc::now());
                tracing::debug!(target: "perf", what = "board_pass", sweep, moved = moved.len() as u64, us = perf_t.elapsed().as_micros() as u64);
            }
        }
    });
}

#[cfg(test)]
#[path = "board_tests.rs"]
mod tests;
