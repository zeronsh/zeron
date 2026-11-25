//! Chats their provider's usage limit stopped, on this host: the wait for
//! the reset and the resume at it ([`zeron_proto::usage_limit`]).
//!
//! - A turn that ends on the limit leaves `Session::limited` on its row
//!   (`sessions`, `session_limits.rs`). Here it is completed: the account
//!   slot it ran on and, without a reset time in the error's own words, the
//!   reset from that account's usage probe (else re-checked every
//!   [`RECHECK_SECS`]); a limit right after a resume waits at least the
//!   attempt's backoff, and past [`MAX_ATTEMPTS`] in a row the chat is left
//!   to the person.
//! - When its orchestrator allows it ([`LimitPolicy::switch_account`]), the
//!   chat continues on another account instead (`account_switch`), and
//!   switches back at a turn boundary after the reset. (What a chat is to
//!   orchestration is read only while Orchestrators is on; off, every chat
//!   is a plain one: no switch, no paused cards, no Brain dump.)
//! - When the reset passes (and the account's fresh probe, if it has one,
//!   agrees), each waiting chat is sent [`RESUMED_AFTER_LIMIT`] once,
//!   orchestrators first, then crew, then the rest, [`STAGGER_MIN_SECS`] to
//!   [`STAGGER_MAX_SECS`] apart. Never a thread of a paused card (it waits
//!   on), an archived chat or the Brain dump (their limit is dropped).
//! - The limit is cleared when the resumed turn starts.
//!
//! Each limit is marked beside the run journal (`{chat}.limited`), so a
//! restart re-arms its wait. One task, started with the engine whatever
//! Orchestrators is ([`crate::resumes`]): it sleeps until the earliest reset
//! or resume due, or a limited row changes.

use std::collections::HashMap;

use chrono::{DateTime, Duration as TimeDelta, Utc};
use zeron_proto::usage_limit::{
    LimitPolicy, RECHECK_SECS, RESUMED_AFTER_LIMIT, STAGGER_MAX_SECS, STAGGER_MIN_SECS, SessionLimit, SwitchedAccount,
};
use zeron_proto::{AgentAccount, AgentAccountsSnapshot, HarnessId, Session, SessionStatus};

use crate::orchestration::ChatRole;
use crate::resumes::Resumes;
use crate::rpc::EngineRpc;
use crate::usage_limit_text::{MAX_ATTEMPTS, check_backoff};

/// A usage probe at most this old says whether a limit has reset.
const PROBE_FRESH_SECS: i64 = 10 * 60;
/// A resume sent that no turn followed in this long: the wait is over, the
/// board sees the failed turn.
const RESUME_STALE_SECS: i64 = 30 * 60;
/// The longest the task sleeps (a cap; it wakes on every limited row change).
const MAX_SLEEP_SECS: i64 = 60 * 60;
/// How often a thread of a paused card is looked at again.
const PAUSED_RECHECK_SECS: i64 = 5 * 60;

/// What the task keeps of each limited chat here.
#[derive(Debug, Default)]
pub(crate) struct LimitBook {
    chats: HashMap<String, Entry>,
    /// The last resume scheduled: the next one goes a stagger after it.
    last_fire: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Default)]
struct Entry {
    /// The limit this entry is for (a new limit is a new entry).
    since: Option<DateTime<Utc>>,
    /// Account, reset and switch filled in.
    completed: bool,
    /// When its resume goes, once the reset passed.
    fire_at: Option<DateTime<Utc>>,
    /// When its resume was sent (once per limit).
    resumed_at: Option<DateTime<Utc>>,
    /// Looked at again no sooner than this (a paused card's thread).
    hold_until: Option<DateTime<Utc>>,
    /// A switched chat's last turn boundary seen.
    boundary: Option<Boundary>,
}

type Boundary = (SessionStatus, Option<String>, Option<DateTime<Utc>>);

fn boundary(row: &Session) -> Boundary {
    (row.status, row.last_completed_turn.clone(), row.last_turn_outcome.map(|o| o.finished_at))
}

/// When a waiting limit is due: its reset, else a re-check.
fn due_at(limit: &SessionLimit) -> DateTime<Utc> {
    limit.until.unwrap_or(limit.since + TimeDelta::seconds(RECHECK_SECS))
}

impl LimitBook {
    /// When the task should look again, at most [`MAX_SLEEP_SECS`] on.
    pub(crate) fn next_wake(&self, rows: &[Session], now: DateTime<Utc>) -> DateTime<Utc> {
        let cap = now + TimeDelta::seconds(MAX_SLEEP_SECS);
        let mut next = cap;
        for row in rows {
            let (Some(limit), Some(entry)) = (row.limited.as_ref(), self.chats.get(&row.chat_id)) else { continue };
            let at = if let Some(sent) = entry.resumed_at {
                sent + TimeDelta::seconds(RESUME_STALE_SECS)
            } else if let Some(fire) = entry.fire_at {
                fire
            } else {
                due_at(limit).max(entry.hold_until.unwrap_or(now))
            };
            next = next.min(at);
        }
        next.max(now + TimeDelta::seconds(1)).min(cap)
    }

    /// When `chat`'s resume goes, once scheduled; for tests.
    #[cfg(test)]
    pub(crate) fn fire_at(&self, chat: &str) -> Option<DateTime<Utc>> {
        self.chats.get(chat).and_then(|e| e.fire_at)
    }

    /// Chats with a resume sent, for tests.
    #[cfg(test)]
    pub(crate) fn resumed(&self) -> Vec<String> {
        let mut chats: Vec<String> =
            self.chats.iter().filter(|(_, e)| e.resumed_at.is_some()).map(|(c, _)| c.clone()).collect();
        chats.sort();
        chats
    }
}

/// The account `limit`'s chat ran on in `snapshot`: its slot when known,
/// else its harness's active one.
fn account_of<'a>(snapshot: Option<&'a AgentAccountsSnapshot>, limit: &SessionLimit) -> Option<&'a AgentAccount> {
    let accounts = &snapshot?.accounts;
    match &limit.account_slot {
        Some(slot) => accounts.iter().find(|a| &a.id == slot),
        None => {
            let harness = HarnessId::from_slug(&limit.provider)?;
            accounts.iter().find(|a| a.harness == harness && a.active)
        }
    }
}

/// When `account`'s exhausted usage windows reset, as its probe says (the
/// latest: usage resumes only when every exhausted window has). `None`: none
/// is exhausted, or it doesn't say when.
pub(crate) fn exhausted_reset(account: &AgentAccount, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    account
        .usage_windows
        .iter()
        .filter(|w| w.used_fraction >= 0.995)
        .filter_map(|w| w.resets_at)
        .filter(|at| *at > now)
        .max()
}

/// When `account`'s window that `window` names resets ("5-hour": its 5h
/// window, "weekly": its week), exhausted or not: the best guess left.
fn named_reset(account: &AgentAccount, window: Option<&str>, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let names: &[&str] = match window? {
        "5-hour" => &["5h", "5-hour", "five", "session"],
        w if w.starts_with("weekly") => &["week", "7d", "7-day", "seven"],
        _ => return None,
    };
    account
        .usage_windows
        .iter()
        .filter(|w| names.iter().any(|n| w.label.to_lowercase().contains(n)))
        .filter_map(|w| w.resets_at)
        .find(|at| *at > now)
}

/// Whether `account`'s probe is fresh at `now` and taken after `since`.
fn fresh_probe(account: &AgentAccount, since: DateTime<Utc>, now: DateTime<Utc>) -> bool {
    account
        .usage_fetched_at
        .and_then(DateTime::from_timestamp_millis)
        .is_some_and(|at| at >= since && now - at <= TimeDelta::seconds(PROBE_FRESH_SECS))
}

/// `limit` completed from what this host knows at `now`: the account it
/// ran on (`snapshot`), its reset (the error's words, else the account's
/// probe), and the backoff of a limit right after a resume.
pub(crate) fn complete(mut limit: SessionLimit, snapshot: Option<&AgentAccountsSnapshot>, now: DateTime<Utc>) -> SessionLimit {
    let account = account_of(snapshot, &limit);
    if let Some(account) = account {
        limit.account_slot.get_or_insert_with(|| account.id.clone());
        if limit.account_label.is_none() {
            limit.account_label = account.email.clone().or_else(|| account.display_name.clone());
        }
    }
    if limit.until.is_none() {
        limit.until = account.and_then(|a| exhausted_reset(a, now).or_else(|| named_reset(a, limit.window.as_deref(), now)));
    }
    if limit.attempt > 0 {
        let least = limit.since + check_backoff(limit.attempt);
        limit.until = Some(limit.until.map_or(least, |until| until.max(least)));
    }
    limit
}

/// The spacing between two resumes, with jitter.
fn stagger(chat: &str, now: DateTime<Utc>) -> TimeDelta {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    chat.hash(&mut h);
    now.timestamp_nanos_opt().unwrap_or_default().hash(&mut h);
    let span = (STAGGER_MAX_SECS - STAGGER_MIN_SECS + 1) as u64;
    TimeDelta::seconds(STAGGER_MIN_SECS + (h.finish() % span) as i64)
}

/// Test seam: the chats the switch hook was asked for.
#[cfg(test)]
pub(crate) static SWITCH_ASKED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

impl EngineRpc {
    /// What `chat`'s orchestrator (its own, or its crew's) says to do on a
    /// limit; off for any other chat.
    pub(crate) fn on_limit_policy(&self, chat: &str) -> LimitPolicy {
        let Some(orch) = self.active_orchestration() else { return LimitPolicy::default() };
        let role = orch.role_of(chat);
        let Some(id) = role.orchestrator_id() else { return LimitPolicy::default() };
        orch.orchestrator(id).filter(|o| !o.archived).map(|o| o.crew.on_limit).unwrap_or_default()
    }

    async fn limit_switch(&self, chat: &str, limit: &SessionLimit) -> Option<SwitchedAccount> {
        #[cfg(test)]
        SWITCH_ASKED.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(chat.to_owned());
        self.switch_account_for_limit(chat, limit).await
    }

    /// Re-arm every limit marked before the engine restarted.
    pub(crate) fn restore_limits(&self, book: &mut LimitBook) -> Vec<String> {
        let mut restored = Vec::new();
        for mark in self.sessions_ref().limit_marks() {
            if self.sessions_ref().restore_limited(&mark) {
                book.chats.insert(
                    mark.chat_id.clone(),
                    Entry {
                        since: Some(mark.limit.since),
                        completed: true,
                        resumed_at: mark.resumed_at,
                        ..Entry::default()
                    },
                );
                restored.push(mark.chat_id);
            }
        }
        if !restored.is_empty() {
            tracing::info!(?restored, "usage limits re-armed after the restart");
        }
        restored
    }

    /// Drop `chat`'s limit (the wait is over without a resume).
    fn drop_limit(&self, chat: &str, since: DateTime<Utc>) {
        self.sessions_ref().update_limited(
            chat,
            |l| {
                if l.as_ref().is_some_and(|l| l.since == since) {
                    *l = None;
                }
            },
            None,
        );
    }

    /// One pass over the limited chats here at `now`: limits completed (and
    /// switched, where allowed), switched chats switched back, due chats'
    /// resumes scheduled and sent. The chats resumed.
    pub(crate) async fn limits_pass(&self, book: &mut LimitBook, now: DateTime<Utc>) -> Vec<String> {
        let rows = self.sessions_ref().limited_rows();
        book.chats.retain(|chat, _| rows.iter().any(|r| &r.chat_id == chat));
        let snapshot = self.agent_accounts_ref().watch_snapshots().borrow().clone();
        // (What orchestration says of these chats: nothing while it's off.)
        let orch = self.active_orchestration();
        let state = orch.map(|o| o.state()).unwrap_or_default();
        // (rank, since, chat): due for a resume, not yet scheduled.
        let mut due: Vec<(u8, DateTime<Utc>, String)> = Vec::new();
        for row in &rows {
            let Some(mut limit) = row.limited.clone() else { continue };
            let chat = row.chat_id.as_str();
            let entry = book.chats.entry(chat.to_owned()).or_default();
            if entry.since != Some(limit.since) {
                *entry = Entry { since: Some(limit.since), ..Entry::default() };
            }
            if !entry.completed {
                entry.completed = true;
                if limit.attempt >= MAX_ATTEMPTS {
                    tracing::info!(chat, attempt = limit.attempt, "usage limit: resumes keep hitting it, left to the person");
                    self.drop_limit(chat, limit.since);
                    continue;
                }
                limit = complete(limit, snapshot.as_ref(), now);
                if limit.waiting()
                    && self.on_limit_policy(chat).switch_account
                    && let Some(to) = self.limit_switch(chat, &limit).await
                {
                    tracing::info!(chat, to = %to.account_slot, "usage limit: continues on another account");
                    limit.switched_to = Some(to);
                }
                let since = limit.since;
                let completed = limit.clone();
                self.sessions_ref().update_limited(
                    chat,
                    |l| {
                        if l.as_ref().is_some_and(|l| l.since == since) {
                            *l = Some(completed);
                        }
                    },
                    None,
                );
                let Some(entry) = book.chats.get_mut(chat) else { continue };
                if !limit.waiting() {
                    // Its current turn on the other account isn't a boundary.
                    entry.boundary = Some(boundary(row));
                    continue;
                }
            }
            let Some(entry) = book.chats.get_mut(chat) else { continue };
            // Running on another account: back at a turn boundary after the
            // reset.
            if !limit.waiting() {
                let at_boundary = matches!(row.status, SessionStatus::Idle | SessionStatus::Errored)
                    && row.waiting.is_none()
                    && row.running_subagents == 0;
                let key = boundary(row);
                if at_boundary && due_at(&limit) <= now && entry.boundary.as_ref() != Some(&key) {
                    entry.boundary = Some(key);
                    if self.switch_back_after_reset(chat, &limit).await {
                        tracing::info!(chat, "usage limit: switched back to its own account");
                        self.drop_limit(chat, limit.since);
                    }
                }
                continue;
            }
            if let Some(sent) = entry.resumed_at {
                if now - sent >= TimeDelta::seconds(RESUME_STALE_SECS) && !self.sessions_ref().turn_in_flight(chat) {
                    tracing::warn!(chat, "usage limit: its resume never started a turn; the wait is over");
                    self.drop_limit(chat, limit.since);
                }
                continue;
            }
            if entry.fire_at.is_some() || entry.hold_until.is_some_and(|h| h > now) || due_at(&limit) > now {
                continue;
            }
            // Due. The account's fresh probe may say it hasn't reset yet.
            if let Some(account) = account_of(snapshot.as_ref(), &limit)
                && fresh_probe(account, limit.since, now)
                && let Some(later) = exhausted_reset(account, now)
            {
                let since = limit.since;
                self.sessions_ref().update_limited(
                    chat,
                    |l| {
                        if let Some(l) = l.as_mut().filter(|l| l.since == since) {
                            l.until = Some(later);
                        }
                    },
                    None,
                );
                continue;
            }
            // Not resumed: a thread of a paused card (it waits on), an
            // archived chat, the Brain dump (their wait ends).
            let paused = state.items.iter().any(|i| !i.removed && i.paused() && i.threads.iter().any(|t| t == chat));
            if paused {
                entry.hold_until = Some(now + TimeDelta::seconds(PAUSED_RECHECK_SECS));
                continue;
            }
            let role = orch.map_or(ChatRole::Plain, |o| o.role_of(chat));
            let gone = self.workspace_host().chat(chat).ok().flatten().is_none_or(|c| c.archived);
            if gone || matches!(role, ChatRole::BrainDump { .. }) {
                self.drop_limit(chat, limit.since);
                continue;
            }
            if self.sessions_ref().turn_in_flight(chat) {
                continue;
            }
            let rank = match role {
                ChatRole::Orchestrator { .. } => 0,
                ChatRole::Crew { .. } => 1,
                _ => 2,
            };
            due.push((rank, limit.since, chat.to_owned()));
        }
        // Orchestrators first, then crew, then the rest; spread out.
        due.sort();
        for (_, _, chat) in &due {
            let at = match book.last_fire {
                Some(last) => (last + stagger(chat, now)).max(now),
                None => now,
            };
            book.last_fire = Some(at);
            if let Some(entry) = book.chats.get_mut(chat) {
                entry.fire_at = Some(at);
            }
        }
        let mut firing: Vec<(DateTime<Utc>, String)> = book
            .chats
            .iter()
            .filter(|(_, e)| e.resumed_at.is_none() && e.fire_at.is_some_and(|at| at <= now))
            .map(|(c, e)| (e.fire_at.unwrap_or(now), c.clone()))
            .collect();
        firing.sort();
        let mut resumed = Vec::new();
        // A chat a restart resume just sent its "[Resumed]" (its cut-off turn,
        // its wait, its subagents, a cut-off send) is left a moment: that
        // turn clears its limit as it starts, and one message is enough.
        let just_resumed = self.resumes().resumed_since(now, TimeDelta::seconds(crate::restart_resume::RESUMED_GRACE_SECS));
        for (_, chat) in firing {
            if !rows.iter().any(|r| r.chat_id == chat && r.limited.as_ref().is_some_and(SessionLimit::waiting)) {
                continue;
            }
            if just_resumed.contains(&chat) {
                continue;
            }
            let injected = zeron_doc::Injected {
                chat_id: Some(chat.clone()),
                ..zeron_doc::Injected::of(zeron_doc::injected::LIMIT_RESET)
            };
            match crate::orchestration::ops::deliver(self, &chat, RESUMED_AFTER_LIMIT, None, crate::orchestration::ops::When::Next, Some(injected)) {
                Ok(_) => {
                    if let Some(entry) = book.chats.get_mut(&chat) {
                        entry.resumed_at = Some(now);
                    }
                    // (Its mark says so: a restart doesn't send it again.)
                    self.sessions_ref().update_limited(&chat, |_| {}, Some(now));
                    tracing::info!(chat = %chat, "usage limit reset: resumed");
                    resumed.push(chat);
                }
                Err(error) => {
                    tracing::warn!(%error, chat = %chat, "usage limit reset: the resume wasn't sent");
                    if let Some(entry) = book.chats.get_mut(&chat) {
                        entry.fire_at = None;
                        entry.hold_until = Some(now + TimeDelta::seconds(RECHECK_SECS));
                    }
                }
            }
        }
        resumed
    }
}

/// What of the rows here the task reads: each limited row and its turn.
fn signature(rows: &[Session]) -> Vec<(String, SessionLimit, Boundary)> {
    rows.iter().filter_map(|r| Some((r.chat_id.clone(), r.limited.clone()?, boundary(r)))).collect()
}

/// The task: re-arm the marked limits, then a pass whenever a limited row
/// changes or the next reset or resume falls due.
pub(crate) fn start(resumes: Resumes) {
    // (Tests drive the passes themselves, at the times they choose.)
    if cfg!(test) {
        return;
    }
    tokio::spawn(async move {
        let Some(rpc) = resumes.rpc() else { return };
        let mut sessions_rx = rpc.sessions_ref().watch_sessions();
        let mut book = LimitBook::default();
        rpc.restore_limits(&mut book);
        drop(rpc);
        let mut seen = signature(&sessions_rx.borrow_and_update());
        loop {
            let Some(rpc) = resumes.rpc() else { break };
            let now = Utc::now();
            rpc.limits_pass(&mut book, now).await;
            let wake = book.next_wake(&rpc.sessions_ref().limited_rows(), now);
            drop(rpc);
            let sleep = tokio::time::sleep((wake - now).to_std().unwrap_or_default());
            tokio::pin!(sleep);
            loop {
                tokio::select! {
                    _ = &mut sleep => break,
                    changed = sessions_rx.changed() => {
                        if changed.is_err() {
                            return;
                        }
                        let now_seen = signature(&sessions_rx.borrow_and_update());
                        if now_seen != seen {
                            seen = now_seen;
                            break;
                        }
                    }
                }
            }
        }
    });
}

#[cfg(test)]
#[path = "usage_limits_tests.rs"]
mod tests;
