//! Account rotation: a provider's saved accounts take turns across new
//! chats (main, side and worker), so usage spreads instead of draining one
//! subscription. The engine owns it — per device, per provider, off by
//! default — and assigns each new chat an account it keeps, moving it to
//! the next healthy account if that one hits its limit mid-turn; this
//! module is the view of that state shared by Settings → Providers and the
//! composer's usage card. Pure apart from the element helpers at the end.

use chrono::{DateTime, Local, TimeZone};
use gpui::{AnyElement, IntoElement, ParentElement, SharedString, Styled, div};
use zeron_proto::{
    AgentAccount, AgentAccountRotation, AgentAccountsSnapshot, AgentAuthKind, HarnessId,
};

use crate::theme::Theme;

/// The settings row's title.
pub const TITLE: &str = "Rotate accounts across chats";
/// The settings row's one-line description.
pub const DESCRIPTION: &str = "New chats take turns across your saved accounts, and a chat \
     that hits a limit moves to the next account.";
/// The usage card's shorter row, under the accounts it rotates.
pub const CARD_TITLE: &str = "Take turns across chats";
pub const CARD_DESCRIPTION: &str = "New chats start on the next account; limits move them on.";

/// The engine's setter: `{harness, enabled}` → the fresh snapshot.
pub const SET_METHOD: &str = zeron_rpc::methods::SET_AGENT_ACCOUNT_ROTATION;

/// One provider's rotation on one device, as the engine reports it.
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderRotation {
    pub harness: HarnessId,
    /// The user's setting.
    pub enabled: bool,
    /// The harness can run chats on separate accounts at all.
    pub supported: bool,
    /// In effect now: enabled, supported, two usable accounts.
    pub active: bool,
    /// The account the next new chat starts on, while active.
    pub next: Option<String>,
    /// The engine's reason it's unsupported or not in effect.
    pub note: Option<String>,
    pub accounts: Vec<AccountRotation>,
}

/// One account's part in its provider's rotation.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AccountRotation {
    pub account_id: String,
    pub in_rotation: bool,
    /// Chats on this account with a turn running right now.
    pub live_chats: u32,
    /// Why new chats skip it for now, as the engine reports it.
    pub skipped: Option<Skip>,
}

/// A participating account rotation passes over for now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skip {
    /// The engine's display reason ("Usage limit", "Signed out").
    pub reason: String,
    /// Epoch millis it's expected back (its limit resets).
    pub until: Option<i64>,
}

impl Skip {
    /// "usage limit until 3:40 PM", "signed out". Pure.
    pub fn describe(&self, now: DateTime<Local>) -> String {
        let reason = lowercase_first(&self.reason);
        match self.until.and_then(|until| format_until(until, now)) {
            Some(until) => format!("{reason} until {until}"),
            None => reason,
        }
    }
}

fn lowercase_first(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// When a skipped account is back: its time today ("3:40 PM"), "tomorrow
/// at 3:40 PM", or further out just the date ("Nov 3"). `None` for a
/// moment that can't be shown. Pure.
pub fn format_until(until_ms: i64, now: DateTime<Local>) -> Option<String> {
    let at = Local.timestamp_millis_opt(until_ms).single()?;
    let time = at.format("%-I:%M %p").to_string();
    let days = (at.date_naive() - now.date_naive()).num_days();
    Some(match days {
        ..=0 => time,
        1 => format!("tomorrow at {time}"),
        _ => at.format("%b %-d").to_string(),
    })
}

/// `harness`'s rotation in `snapshot`, with each of its accounts' part;
/// `None` from an engine that predates rotation. Pure.
pub fn provider_rotation(
    snapshot: &AgentAccountsSnapshot,
    harness: HarnessId,
) -> Option<ProviderRotation> {
    let rotation = snapshot.rotation_for(harness)?;
    let accounts = snapshot
        .accounts
        .iter()
        .filter(|a| a.harness == harness)
        .map(|account| {
            let in_rotation = rotation.accounts.iter().any(|id| *id == account.id);
            AccountRotation {
                account_id: account.id.clone(),
                in_rotation,
                live_chats: rotation.running_count(&account.id) as u32,
                skipped: rotation
                    .skipped
                    .iter()
                    .find(|skip| skip.account_id == account.id)
                    .map(|skip| Skip {
                        reason: skip.reason.clone(),
                        until: skip.until,
                    }),
            }
        })
        .collect();
    Some(ProviderRotation {
        harness,
        enabled: rotation.enabled,
        supported: rotation.supported,
        active: rotation.active,
        next: rotation.next_account_id.clone(),
        note: rotation.note.clone(),
        accounts,
    })
}

/// The optimistic half of a flip: the setting changes at once and, when
/// it can, takes effect; the engine's reply settles `active` and `next`.
/// Pure.
pub fn mark_enabled(snapshot: &mut AgentAccountsSnapshot, harness: HarnessId, enabled: bool) {
    if let Some(rotation) = snapshot.rotation.iter_mut().find(|r| r.harness == harness) {
        rotation.enabled = enabled;
        rotation.active = enabled && rotation.supported && rotation.accounts.len() >= 2;
        if !rotation.active {
            rotation.next_account_id = None;
        } else if rotation.next_account_id.is_none() {
            rotation.next_account_id = rotation.accounts.first().cloned();
        }
    }
}

/// Put `harness`'s rotation back as it was before a refused flip, leaving
/// every other provider's (and the accounts) as they are now. Pure.
pub fn restore_rotation(
    snapshot: &mut AgentAccountsSnapshot,
    harness: HarnessId,
    previous: Option<AgentAccountRotation>,
) {
    snapshot.rotation.retain(|r| r.harness != harness);
    snapshot.rotation.extend(previous);
}

/// The setter's params (the `targetDeviceId` passthrough is the caller's).
pub fn set_params(harness: HarnessId, enabled: bool) -> serde_json::Value {
    serde_json::json!({ "harness": harness, "enabled": enabled })
}

/// The account a rotated chat runs on, and the one a fallback moved it
/// off (a limit, a sign-out, a removal).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatAccount {
    /// The agent it runs (the account's), for the row's mark.
    pub harness: HarnessId,
    pub label: String,
    pub moved_from: Option<String>,
    /// The engine's account of the move ("Usage limit on alice@x, moved to
    /// bob@x").
    pub moved_reason: Option<String>,
}

impl ChatAccount {
    /// The row's hover line: why it's on this account.
    pub fn tooltip(&self) -> String {
        match (&self.moved_reason, &self.moved_from) {
            (Some(reason), _) => reason.clone(),
            (None, Some(from)) => format!("Moved here from {from}"),
            (None, None) => "Account rotation started this chat on this account".into(),
        }
    }
}

fn account_label(account: &AgentAccount) -> String {
    account
        .email
        .clone()
        .or_else(|| account.display_name.clone())
        .unwrap_or_else(|| "Unknown account".into())
}

/// The account a rotated chat runs on; `None` for a chat rotation didn't
/// assign. Pure.
pub fn chat_account(snapshot: &AgentAccountsSnapshot, chat_id: &str) -> Option<ChatAccount> {
    let account = snapshot.chat_account(chat_id)?;
    let pinned = snapshot
        .rotation_for(account.harness)
        .and_then(|rotation| rotation.chats.iter().find(|c| c.chat_id == chat_id));
    let moved_reason = pinned.and_then(|chat| chat.moved_reason.clone());
    let moved_from = pinned
        .and_then(|chat| chat.previous_account_id.as_deref())
        .map(|previous| {
            snapshot
                .accounts
                .iter()
                .find(|a| a.harness == account.harness && a.id == previous)
                .map_or_else(|| "a removed account".into(), account_label)
        });
    Some(ChatAccount {
        harness: account.harness,
        label: account_label(account),
        moved_from,
        moved_reason,
    })
}

impl ProviderRotation {
    pub fn account(&self, id: &str) -> Option<&AccountRotation> {
        self.accounts.iter().find(|a| a.account_id == id)
    }
}

/// Whether the toggle can be flipped, and if not, why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Availability {
    Available,
    /// The harness can't give each chat its own login.
    Unsupported,
    /// Fewer than two accounts to take turns between.
    NeedsTwoAccounts,
}

/// Accounts that could take a turn: subscription logins Zeron can switch
/// to (API keys never rotate). Pure.
pub fn usable_accounts<'a>(accounts: impl IntoIterator<Item = &'a AgentAccount>) -> usize {
    accounts
        .into_iter()
        .filter(|a| a.switchable && a.auth_kind != Some(AgentAuthKind::ApiKey))
        .count()
}

/// Pure. An already-on rotation stays flippable (off) whatever changed.
pub fn availability(rotation: Option<&ProviderRotation>, usable: usize) -> Availability {
    match rotation {
        Some(rotation) if rotation.enabled => Availability::Available,
        Some(rotation) if !rotation.supported => Availability::Unsupported,
        None => Availability::Unsupported,
        _ if usable < 2 => Availability::NeedsTwoAccounts,
        _ => Availability::Available,
    }
}

/// The toggle's caption when it isn't the plain description: why it can't
/// be flipped, or why an enabled rotation isn't in effect — the engine's
/// reason first. Pure.
pub fn caption(
    harness: HarnessId,
    rotation: Option<&ProviderRotation>,
    availability: Availability,
) -> Option<String> {
    let note = rotation.and_then(|r| r.note.clone());
    match availability {
        Availability::Available if rotation.is_some_and(|r| r.enabled && !r.active) => {
            note.or_else(|| unavailable_reason(harness, Availability::NeedsTwoAccounts))
        }
        Availability::Available => None,
        other => note.or_else(|| unavailable_reason(harness, other)),
    }
}

/// The disabled toggle's own explanation. Pure.
pub fn unavailable_reason(harness: HarnessId, availability: Availability) -> Option<String> {
    match availability {
        Availability::Available => None,
        Availability::Unsupported => Some(format!(
            "{} can't run chats on separate accounts yet.",
            zeron_proto::providers::provider_name(harness)
        )),
        Availability::NeedsTwoAccounts => Some("Add a second account to take turns.".into()),
    }
}

/// Whether the settings switch shows: wherever the engine reports
/// rotation, once there's something to rotate — a supported agent with an
/// account (the caption asks for a second), or any agent with two (the
/// caption says why it can't). Pure.
pub fn shows_switch(rotation: Option<&ProviderRotation>, accounts: usize, usable: usize) -> bool {
    match rotation {
        None => false,
        Some(rotation) if rotation.supported || rotation.enabled => accounts > 0,
        Some(_) => usable >= 2,
    }
}

/// One mark on an account row while its provider rotates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Marker {
    /// The next new chat starts here.
    Next,
    /// Every account is at its limit: the next chat still starts here (the
    /// one with the most headroom).
    NextAllAtLimit,
    /// Chats running on it right now.
    Chats(u32),
    /// Sitting out, and why.
    Skipped(Skip),
    /// Not taking turns (e.g. an unreadable login).
    Out,
}

impl Marker {
    /// Pure, given `now` (a skip's "until" is local time).
    pub fn label(&self, now: DateTime<Local>) -> String {
        match self {
            Marker::Next => "Next".into(),
            Marker::NextAllAtLimit => "Next — all at limit".into(),
            Marker::Chats(1) => "1 chat running".into(),
            Marker::Chats(n) => format!("{n} chats running"),
            Marker::Skipped(skip) if skip.reason.is_empty() => "Skipped".into(),
            Marker::Skipped(skip) => format!("Skipped — {}", skip.describe(now)),
            Marker::Out => "Not in rotation".into(),
        }
    }
}

/// An account's marks, in reading order; nothing while rotation isn't in
/// effect. Pure.
pub fn markers(rotation: Option<&ProviderRotation>, account_id: &str) -> Vec<Marker> {
    let Some(rotation) = rotation.filter(|r| r.active) else {
        return Vec::new();
    };
    let Some(account) = rotation.account(account_id) else {
        return Vec::new();
    };
    let next = rotation.next.as_deref() == Some(account_id);
    let mut marks = Vec::new();
    match &account.skipped {
        // Everyone's at a limit, and the engine still picks this one.
        Some(_) if next => marks.push(Marker::NextAllAtLimit),
        Some(skip) => marks.push(Marker::Skipped(skip.clone())),
        None if !account.in_rotation => marks.push(Marker::Out),
        None if next => marks.push(Marker::Next),
        None => {}
    }
    if account.live_chats > 0 {
        marks.push(Marker::Chats(account.live_chats));
    }
    marks
}

/// The device's live account reads "In use" — or, while chats take turns
/// and it only starts the ones rotation leaves alone, "Default". Pure.
pub fn active_label(rotation: Option<&ProviderRotation>) -> &'static str {
    if rotation.is_some_and(|r| r.active) {
        "Default"
    } else {
        "In use"
    }
}

/// The marks as meta-line fragments: "Next" in the accent, a skip in the
/// warning tone, counts muted like the rest of the line.
pub fn marker_fragments(markers: &[Marker], theme: &Theme) -> Vec<AnyElement> {
    let now = Local::now();
    markers
        .iter()
        .map(|marker| {
            let color = match marker {
                Marker::Next => theme.accent,
                Marker::NextAllAtLimit | Marker::Skipped(_) => theme.warning,
                Marker::Chats(_) | Marker::Out => theme.text_muted,
            };
            div()
                .text_color(color)
                .child(SharedString::from(marker.label(now)))
                .into_any_element()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rotation(enabled: bool, supported: bool) -> ProviderRotation {
        ProviderRotation {
            harness: HarnessId::ClaudeCode,
            enabled,
            supported,
            active: enabled && supported,
            next: Some("b".into()),
            note: None,
            accounts: vec![
                AccountRotation {
                    account_id: "a".into(),
                    in_rotation: true,
                    live_chats: 2,
                    skipped: None,
                },
                AccountRotation {
                    account_id: "b".into(),
                    in_rotation: true,
                    live_chats: 0,
                    skipped: None,
                },
                AccountRotation {
                    account_id: "c".into(),
                    in_rotation: true,
                    live_chats: 1,
                    skipped: Some(Skip {
                        reason: "Usage limit".into(),
                        until: None,
                    }),
                },
                AccountRotation {
                    account_id: "d".into(),
                    in_rotation: false,
                    ..Default::default()
                },
            ],
        }
    }

    #[test]
    fn the_toggle_needs_support_and_two_accounts_but_always_turns_off() {
        let off = rotation(false, true);
        assert_eq!(availability(Some(&off), 2), Availability::Available);
        assert_eq!(availability(Some(&off), 1), Availability::NeedsTwoAccounts);
        assert_eq!(
            availability(Some(&rotation(false, false)), 3),
            Availability::Unsupported
        );
        // An engine without rotation reads as unsupported.
        assert_eq!(availability(None, 3), Availability::Unsupported);
        // Already on: it can always be turned off, even down to one account.
        assert_eq!(
            availability(Some(&rotation(true, true)), 1),
            Availability::Available
        );
        assert_eq!(
            unavailable_reason(HarnessId::Codex, Availability::Unsupported).as_deref(),
            Some("Codex can't run chats on separate accounts yet.")
        );
        assert_eq!(
            unavailable_reason(HarnessId::Codex, Availability::NeedsTwoAccounts).as_deref(),
            Some("Add a second account to take turns.")
        );
        assert!(unavailable_reason(HarnessId::Codex, Availability::Available).is_none());
        // Driven by what the engine reports, not by a list of agents.
        assert!(!shows_switch(None, 3, 3));
        assert!(shows_switch(Some(&off), 1, 1));
        assert!(!shows_switch(Some(&off), 0, 0));
        assert!(!shows_switch(Some(&rotation(false, false)), 1, 1));
        assert!(shows_switch(Some(&rotation(false, false)), 2, 2));
        // The engine's reason wins over the fallback caption.
        let mut noted = rotation(false, false);
        noted.note = Some("Needs the local proxy".into());
        assert_eq!(
            caption(HarnessId::Grok, Some(&noted), Availability::Unsupported).as_deref(),
            Some("Needs the local proxy")
        );
        let mut waiting = rotation(true, true);
        waiting.active = false;
        waiting.note = Some("Needs at least two signed-in accounts".into());
        assert_eq!(
            caption(HarnessId::Codex, Some(&waiting), Availability::Available).as_deref(),
            Some("Needs at least two signed-in accounts")
        );
        assert!(caption(HarnessId::Codex, Some(&off), Availability::Available).is_none());
    }

    #[test]
    fn rows_say_next_how_many_chats_and_why_one_sits_out() {
        let on = rotation(true, true);
        assert_eq!(markers(Some(&on), "a"), [Marker::Chats(2)]);
        assert_eq!(markers(Some(&on), "b"), [Marker::Next]);
        let limited = Skip {
            reason: "Usage limit".into(),
            until: None,
        };
        assert_eq!(
            markers(Some(&on), "c"),
            [Marker::Skipped(limited.clone()), Marker::Chats(1)]
        );
        assert_eq!(markers(Some(&on), "d"), [Marker::Out]);
        assert!(markers(Some(&on), "unknown").is_empty());
        // Off: the rows stay as they were.
        assert!(markers(Some(&rotation(false, true)), "b").is_empty());
        assert!(markers(None, "b").is_empty());
        let now = Local::now();
        assert_eq!(Marker::Chats(1).label(now), "1 chat running");
        assert_eq!(Marker::Chats(3).label(now), "3 chats running");

        // Every account at its limit: the one the engine still picks
        // reads as next, the rest as skipped.
        let mut exhausted = on.clone();
        for account in &mut exhausted.accounts {
            account.skipped = Some(limited.clone());
        }
        assert_eq!(markers(Some(&exhausted), "b"), [Marker::NextAllAtLimit]);
        assert_eq!(
            markers(Some(&exhausted), "a"),
            [Marker::Skipped(limited.clone()), Marker::Chats(2)]
        );
        assert_eq!(Marker::NextAllAtLimit.label(now), "Next — all at limit");
        assert_eq!(active_label(Some(&on)), "Default");
        assert_eq!(active_label(None), "In use");
    }

    #[test]
    fn skips_read_the_engines_reason_and_when_the_account_is_back() {
        let now = Local.with_ymd_and_hms(2026, 11, 1, 10, 0, 0).unwrap();
        let at = |day: u32, hour: u32, minute: u32| {
            Local
                .with_ymd_and_hms(2026, 11, day, hour, minute, 0)
                .unwrap()
                .timestamp_millis()
        };
        assert_eq!(format_until(at(1, 15, 40), now).as_deref(), Some("3:40 PM"));
        assert_eq!(
            format_until(at(2, 9, 5), now).as_deref(),
            Some("tomorrow at 9:05 AM")
        );
        assert_eq!(format_until(at(4, 9, 5), now).as_deref(), Some("Nov 4"));
        let label = |reason: &str, until: Option<i64>| {
            Marker::Skipped(Skip {
                reason: reason.into(),
                until,
            })
            .label(now)
        };
        assert_eq!(
            label("Usage limit", Some(at(1, 15, 40))),
            "Skipped — usage limit until 3:40 PM"
        );
        assert_eq!(label("Signed out", None), "Skipped — signed out");

        // From the snapshot: the engine's skip list, not a guess from usage.
        let account = |id: &str, used: f32| -> AgentAccount {
            serde_json::from_value(serde_json::json!({
                "id": id, "harness": "codex", "active": false, "switchable": true,
                "usageWindows": [{ "label": "Session", "usedFraction": used }],
            }))
            .unwrap()
        };
        let snapshot = AgentAccountsSnapshot {
            // "a" is nearly full but not skipped; "b" is skipped by a
            // provider's limit reply the usage meters don't show yet.
            accounts: vec![account("a", 0.99), account("b", 0.1)],
            warnings: Vec::new(),
            rotation: vec![AgentAccountRotation {
                harness: HarnessId::Codex,
                supported: true,
                enabled: true,
                active: true,
                accounts: vec!["a".into(), "b".into()],
                next_account_id: Some("a".into()),
                chats: Vec::new(),
                note: None,
                skipped: vec![zeron_proto::AgentAccountSkip {
                    account_id: "b".into(),
                    reason: "Usage limit".into(),
                    until: Some(at(1, 15, 40)),
                }],
            }],
        };
        let rotation = provider_rotation(&snapshot, HarnessId::Codex).unwrap();
        assert_eq!(markers(Some(&rotation), "a"), [Marker::Next]);
        assert_eq!(
            markers(Some(&rotation), "b")[0].label(now),
            "Skipped — usage limit until 3:40 PM"
        );
    }

    #[test]
    fn a_refused_flip_puts_back_only_its_providers_rotation() {
        let entry = |harness, enabled| AgentAccountRotation {
            harness,
            supported: true,
            enabled,
            active: enabled,
            accounts: vec!["a".into(), "b".into()],
            next_account_id: None,
            chats: Vec::new(),
            note: None,
            skipped: Vec::new(),
        };
        let mut snapshot = AgentAccountsSnapshot {
            accounts: Vec::new(),
            warnings: Vec::new(),
            rotation: vec![
                entry(HarnessId::Codex, false),
                entry(HarnessId::ClaudeCode, false),
            ],
        };
        let before = snapshot.rotation_for(HarnessId::Codex).cloned();
        mark_enabled(&mut snapshot, HarnessId::Codex, true);
        // Meanwhile Claude's flip landed.
        mark_enabled(&mut snapshot, HarnessId::ClaudeCode, true);
        restore_rotation(&mut snapshot, HarnessId::Codex, before);
        assert!(!snapshot.rotation_for(HarnessId::Codex).unwrap().enabled);
        assert!(
            snapshot
                .rotation_for(HarnessId::ClaudeCode)
                .unwrap()
                .enabled
        );
    }

    #[test]
    fn a_chat_shows_its_account_and_where_a_limit_moved_it_from() {
        let account = |id: &str, email: &str| -> AgentAccount {
            serde_json::from_value(serde_json::json!({
                "id": id, "harness": "codex", "email": email,
                "active": false, "switchable": true,
            }))
            .unwrap()
        };
        let chat =
            |chat: &str, account: &str, previous: Option<&str>| zeron_proto::AgentAccountChat {
                chat_id: chat.into(),
                account_id: account.into(),
                assigned_at: 0,
                used_at: 0,
                previous_account_id: previous.map(str::to_string),
                moved_reason: None,
                running: false,
            };
        let snapshot = AgentAccountsSnapshot {
            accounts: vec![
                account("a", "alice@example.com"),
                account("b", "bob@example.com"),
            ],
            warnings: Vec::new(),
            rotation: vec![zeron_proto::AgentAccountRotation {
                harness: HarnessId::Codex,
                supported: true,
                enabled: true,
                active: true,
                accounts: vec!["a".into(), "b".into()],
                next_account_id: Some("a".into()),
                chats: vec![
                    chat("fresh", "a", None),
                    chat("moved", "b", Some("a")),
                    chat("orphan", "b", Some("gone")),
                ],
                note: None,
                skipped: Vec::new(),
            }],
        };
        let fresh = chat_account(&snapshot, "fresh").unwrap();
        assert_eq!(fresh.label, "alice@example.com");
        assert_eq!(fresh.harness, HarnessId::Codex);
        assert!(fresh.moved_from.is_none());
        let moved = chat_account(&snapshot, "moved").unwrap();
        assert_eq!(moved.label, "bob@example.com");
        assert_eq!(moved.tooltip(), "Moved here from alice@example.com");
        // The engine's own account of the move wins.
        let mut explained = snapshot.clone();
        explained.rotation[0].chats[1].moved_reason =
            Some("Usage limit on alice@example.com, moved to bob@example.com".into());
        assert_eq!(
            chat_account(&explained, "moved").unwrap().tooltip(),
            "Usage limit on alice@example.com, moved to bob@example.com"
        );
        assert_eq!(
            chat_account(&snapshot, "orphan")
                .unwrap()
                .moved_from
                .as_deref(),
            Some("a removed account")
        );
        assert!(chat_account(&snapshot, "unrotated").is_none());

        // A row counts its chats running now, not every chat it carries.
        let idle = provider_rotation(&snapshot, HarnessId::Codex).unwrap();
        assert_eq!(idle.account("b").unwrap().live_chats, 0);
        let mut busy = snapshot.clone();
        busy.rotation[0].chats[1].running = true;
        let busy = provider_rotation(&busy, HarnessId::Codex).unwrap();
        assert_eq!(busy.account("a").unwrap().live_chats, 0);
        assert_eq!(busy.account("b").unwrap().live_chats, 1);
        assert_eq!(markers(Some(&busy), "b"), [Marker::Chats(1)]);
    }
}
