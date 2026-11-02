//! The composer footer's ring cluster. The plan-usage ring shows how much of
//! the active account's rate limit the session's harness has used, beside
//! the context ring. Clicking it opens the harness's accounts — each with its
//! usage meters — and clicking one switches to it, the same `ActivateAgentAccount` Settings →
//! Accounts runs. Both views share [`AccountsSnapshotCache`], so a switch in
//! either shows up in the other.
use std::time::{Duration, Instant};

use gpui::{
    Context, Entity, IntoElement, Render, SharedString, Subscription, Task, Window, div,
    prelude::*, px,
};
use zeron_proto::{AgentAccount, AgentAccountsSnapshot, HarnessId};
use zeron_rpc::methods;

use crate::account_rotation;
use crate::popover;
use crate::settings::accounts::{
    self, AccountsSnapshotCache, UsageLevel, render_usage_meter, reports_usage, signs_in,
    usage_color, usage_level,
};
use crate::settings::widgets;
use crate::state::AppState;
use crate::theme::Theme;

/// Forced probes hit the provider; the engine throttles them too, but the
/// ring re-probes on every hover, so don't even ask more often than this.
const FORCE_MIN_INTERVAL: Duration = Duration::from_secs(30);
/// Background re-probe while a composer is alive: usage moves as turns run.
const POLL_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// The binding limit: the most-used window of the account. Pure.
pub fn used_fraction(account: &AgentAccount) -> Option<f32> {
    account
        .usage_windows
        .iter()
        .map(|window| window.used_fraction.clamp(0.0, 1.0))
        .reduce(f32::max)
}

/// The account a chat runs on: the one rotation started it on, else the
/// harness's live account. Pure.
pub fn chat_account<'a>(
    snapshot: &'a AgentAccountsSnapshot,
    harness: HarnessId,
    chat: Option<&str>,
) -> Option<&'a AgentAccount> {
    chat.and_then(|chat| snapshot.chat_account(chat))
        .filter(|account| account.harness == harness)
        .or_else(|| active_account(snapshot, harness))
}

/// The harness's live account, when it reports usage. Pure.
pub fn active_account(
    snapshot: &AgentAccountsSnapshot,
    harness: HarnessId,
) -> Option<&AgentAccount> {
    snapshot
        .accounts
        .iter()
        .find(|account| account.harness == harness && account.active)
}

/// Loads (and switches) the accounts of the composer's target device.
pub struct AccountUsage {
    state: Entity<AppState>,
    /// `None` = this device, as in [`AccountsSnapshotCache`].
    target: Option<String>,
    harness: Option<HarnessId>,
    /// The composer's chat, whose rotated account the ring reads.
    chat: Option<String>,
    /// Its turn is running: a start or an end may move it to an account.
    running: bool,
    /// Once any list has been asked for the current target.
    loaded: bool,
    last_forced: Option<Instant>,
    error: Option<SharedString>,
    load_task: Option<Task<()>>,
    /// A cache-only re-list (no probe) after the chat or its turn changed.
    refresh_task: Option<Task<()>>,
    action_task: Option<Task<()>>,
    rotation_task: Option<Task<()>>,
    popup: popover::Popup<FooterCard>,
    _poll: Task<()>,
    _cache: Subscription,
    _state: Subscription,
}

impl AccountUsage {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let poll = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL_INTERVAL).await;
                if this
                    .update(cx, |usage, cx| {
                        if usage.harness.is_some() {
                            usage.load(true, cx);
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
            _state: cx.observe(&state, |_, _, cx| cx.notify()),
            state,
            target: None,
            harness: None,
            chat: None,
            running: false,
            loaded: false,
            last_forced: None,
            error: None,
            load_task: None,
            refresh_task: None,
            action_task: None,
            rotation_task: None,
            popup: popover::Popup::default(),
            _poll: poll,
            // Settings → Accounts writes the same cache.
            _cache: cx.observe_global::<AccountsSnapshotCache>(|_, cx| cx.notify()),
        }
    }

    /// Point at the session's harness, device, chat and whether its turn
    /// runs; loads on first sight of a device. Rotation picks a new chat's
    /// account on its first run and may move it when a limit hits, so a
    /// switched chat — or, while rotation is in effect, a turn starting or
    /// ending — re-reads the engine's cached list (no usage probe). Cheap
    /// enough to call every render.
    pub fn track(
        &mut self,
        harness: Option<HarnessId>,
        target: Option<String>,
        chat: Option<String>,
        running: bool,
        cx: &mut Context<Self>,
    ) {
        let harness = harness.filter(|h| signs_in(*h) && reports_usage(*h));
        self.harness = harness;
        let retargeted = self.target != target;
        if retargeted {
            self.target = target;
            self.loaded = false;
            self.last_forced = None;
            self.error = None;
        }
        let rechatted = self.chat != chat;
        let turn_moved = self.running != running;
        self.chat = chat;
        self.running = running;
        if rechatted {
            self.error = None;
        }
        let Some(harness) = harness else {
            return;
        };
        if !self.loaded {
            self.loaded = true;
            self.load(true, cx);
        } else if rechatted || (turn_moved && self.rotation_active(harness, cx)) {
            self.refresh(cx);
        }
    }

    fn rotation_active(&self, harness: HarnessId, cx: &gpui::App) -> bool {
        self.snapshot(cx)
            .and_then(|snapshot| snapshot.rotation_for(harness))
            .is_some_and(|rotation| rotation.active)
    }

    /// Re-read the engine's list without probing usage: which account each
    /// rotated chat is on moves with turns, the meters don't need to.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let key = self.target.clone();
        let params = self.params(serde_json::json!({ "forceUsage": false }));
        self.refresh_task = Some(cx.spawn(async move |this, cx| {
            let snapshot = engine
                .client()
                .call(methods::LIST_AGENT_ACCOUNTS, params)
                .await
                .ok()
                .and_then(|value| serde_json::from_value::<AgentAccountsSnapshot>(value).ok());
            if let Some(snapshot) = snapshot {
                this.update(cx, |_, cx| {
                    cx.default_global::<AccountsSnapshotCache>()
                        .0
                        .insert(key, snapshot);
                })
                .ok();
            }
        }));
    }

    fn snapshot<'a>(&self, cx: &'a gpui::App) -> Option<&'a AgentAccountsSnapshot> {
        cx.try_global::<AccountsSnapshotCache>()?
            .0
            .get(&self.target)
    }

    fn params(&self, mut value: serde_json::Value) -> serde_json::Value {
        if let (Some(target), Some(object)) = (&self.target, value.as_object_mut()) {
            object.insert("targetDeviceId".into(), serde_json::json!(target));
        }
        value
    }

    /// Plain list first when nothing is cached (the engine's persisted usage
    /// paints at once), then the forced probe replaces it.
    fn load(&mut self, force_usage: bool, cx: &mut Context<Self>) {
        if force_usage {
            if self
                .last_forced
                .is_some_and(|at| at.elapsed() < FORCE_MIN_INTERVAL)
            {
                return;
            }
            self.last_forced = Some(Instant::now());
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let key = self.target.clone();
        let paint_first = force_usage && self.snapshot(cx).is_none();
        let plain = self.params(serde_json::json!({ "forceUsage": false }));
        let params = self.params(serde_json::json!({ "forceUsage": force_usage }));
        self.load_task = Some(cx.spawn(async move |this, cx| {
            let fetch = |params: serde_json::Value| {
                let engine = engine.clone();
                async move {
                    engine
                        .client()
                        .call(methods::LIST_AGENT_ACCOUNTS, params)
                        .await
                        .ok()
                        .and_then(|value| {
                            serde_json::from_value::<AgentAccountsSnapshot>(value).ok()
                        })
                }
            };
            let store = |snapshot: AgentAccountsSnapshot, cx: &mut gpui::AsyncApp| {
                let key = key.clone();
                this.update(cx, |_, cx| {
                    cx.default_global::<AccountsSnapshotCache>()
                        .0
                        .insert(key, snapshot);
                })
                .ok();
            };
            if paint_first && let Some(snapshot) = fetch(plain).await {
                store(snapshot, cx);
            }
            if let Some(snapshot) = fetch(params).await {
                store(snapshot, cx);
            }
        }));
    }

    /// Switch optimistically, like Settings → Accounts: the rows flip at once
    /// and the engine's reply replaces them; a refusal restores the list.
    fn switch(&mut self, account: &AgentAccount, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let key = self.target.clone();
        let cache = &mut cx.default_global::<AccountsSnapshotCache>().0;
        let previous = cache.get(&key).cloned();
        if let Some(snapshot) = cache.get_mut(&key) {
            // Per-provider agents keep one live login per provider: only the
            // switched-to row's group changes.
            accounts::mark_switched(snapshot, account);
        }
        self.error = None;
        let params = self.params(serde_json::json!({
            "id": account.id,
            "accountId": account.id,
            "harness": account.harness,
        }));
        self.action_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(methods::ACTIVATE_AGENT_ACCOUNT, params)
                .await;
            this.update(cx, |usage, cx| {
                match result.map(serde_json::from_value::<AgentAccountsSnapshot>) {
                    Ok(Ok(snapshot)) => {
                        cx.default_global::<AccountsSnapshotCache>()
                            .0
                            .insert(key, snapshot);
                    }
                    Ok(Err(_)) => usage.load(false, cx),
                    Err(err) => {
                        if let Some(previous) = previous {
                            cx.default_global::<AccountsSnapshotCache>()
                                .0
                                .insert(key, previous);
                        }
                        usage.error = Some(err.to_string().into());
                    }
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// The card's rotation switch, for a provider with two accounts to take
    /// turns between (or one already rotating, so it can be turned off).
    fn rotation_row(
        &self,
        rotation: Option<&account_rotation::ProviderRotation>,
        rows: &[AgentAccount],
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Stateful<gpui::Div>> {
        let rotation = rotation?;
        let usable = account_rotation::usable_accounts(rows);
        let availability = account_rotation::availability(Some(rotation), usable);
        if availability != account_rotation::Availability::Available {
            return None;
        }
        // On but not in effect (down to one usable account): say why.
        let caption = account_rotation::caption(rotation.harness, Some(rotation), availability)
            .unwrap_or_else(|| account_rotation::CARD_DESCRIPTION.to_string());
        let harness = rotation.harness;
        let on = rotation.enabled;
        let busy = self.rotation_task.is_some();
        Some(
            popover::menu_row(theme, false, "account-usage-rotation")
                .id("account-usage-rotation")
                .role(gpui::Role::Switch)
                .aria_label(account_rotation::TITLE)
                .aria_toggled(if on {
                    gpui::Toggled::True
                } else {
                    gpui::Toggled::False
                })
                .when(busy, |row| row.opacity(0.7).cursor_default())
                .when(!busy, |row| {
                    row.on_click(cx.listener(move |usage, _, _, cx| {
                        usage.set_rotation(harness, !on, cx);
                    }))
                })
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(div().truncate().child(account_rotation::CARD_TITLE))
                        .child(
                            div()
                                .text_size(crate::typography::ui_rems(11.0))
                                .text_color(theme.text_muted)
                                .child(SharedString::from(caption)),
                        ),
                )
                .child(widgets::toggle_switch(
                    theme,
                    on,
                    "account-usage-rotation-switch",
                )),
        )
    }

    /// Flip the provider's rotation, optimistically like a switch: the card
    /// changes at once, the engine's list replaces it, a refusal restores it.
    fn set_rotation(&mut self, harness: HarnessId, enabled: bool, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let key = self.target.clone();
        let cache = &mut cx.default_global::<AccountsSnapshotCache>().0;
        // Only this provider's rotation is put back on a refusal: anything
        // else (a list, another provider's flip) may land meanwhile.
        let previous = cache
            .get(&key)
            .and_then(|snapshot| snapshot.rotation_for(harness).cloned());
        if let Some(snapshot) = cache.get_mut(&key) {
            account_rotation::mark_enabled(snapshot, harness, enabled);
        }
        self.error = None;
        let params = self.params(account_rotation::set_params(harness, enabled));
        self.rotation_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(account_rotation::SET_METHOD, params)
                .await;
            this.update(cx, |usage, cx| {
                usage.rotation_task = None;
                match result.map(serde_json::from_value::<AgentAccountsSnapshot>) {
                    Ok(Ok(snapshot)) => {
                        cx.default_global::<AccountsSnapshotCache>()
                            .0
                            .insert(key, snapshot);
                    }
                    Ok(Err(_)) => usage.load(false, cx),
                    Err(err) => {
                        if let Some(snapshot) =
                            cx.default_global::<AccountsSnapshotCache>().0.get_mut(&key)
                        {
                            account_rotation::restore_rotation(snapshot, harness, previous);
                        }
                        // Said on the card only while it still shows that device.
                        if usage.target == key {
                            usage.error = Some(err.to_string().into());
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// The ring's reading: the chat's account's most-used window — the one
    /// rotation started it on, else the live one — when it has usage.
    fn fraction(&self, cx: &gpui::App) -> Option<f32> {
        used_fraction(chat_account(
            self.snapshot(cx)?,
            self.harness?,
            self.chat.as_deref(),
        )?)
    }

    /// A trigger click: open this ring's popover, or close it when the
    /// press found it open (the card's mouse-down-out already began that
    /// close — see `Popup::note_trigger_press`).
    fn toggle(&mut self, card: FooterCard, cx: &mut Context<Self>) {
        if self.popup.take_press_was_open() || self.popup.as_open() == Some(&card) {
            self.dismiss(cx);
            return;
        }
        if card == FooterCard::Accounts {
            // Opening the card is the moment someone cares: re-probe.
            self.load(true, cx);
        }
        self.popup.open(card);
        cx.notify();
    }

    fn dismiss(&mut self, cx: &mut Context<Self>) {
        if self.popup.begin_close() {
            popover::reap_popup(cx, |usage: &mut Self| &mut usage.popup);
        }
        cx.notify();
    }

    /// A footer ring that opens `card` above itself, right-aligned like the
    /// model picker.
    fn trigger(
        &self,
        chip: gpui::Stateful<gpui::Div>,
        card: FooterCard,
        content: impl FnOnce(&Self, &mut Context<Self>) -> gpui::Div,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let chip = chip
            .relative()
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |usage, _, _, _| {
                    usage
                        .popup
                        .note_trigger_press_matching(|open| *open == card);
                }),
            )
            .on_click(cx.listener(move |usage, _, _, cx| usage.toggle(card, cx)));
        if self.popup.get() != Some(&card) {
            return chip.into_any_element();
        }
        let content = content(self, cx)
            .on_mouse_down_out(cx.listener(|usage, _, _, cx| usage.dismiss(cx)))
            .into_any_element();
        chip.child(popover::anchored_menu_above_end(
            card.id(),
            content,
            self.popup.closing_since(),
        ))
        .into_any_element()
    }

    fn accounts_card(&self, cx: &mut Context<Self>) -> gpui::Div {
        let theme = &Theme::of(cx).for_popup();
        let harness = self.harness;
        let now = chrono::Utc::now();
        let rows: Vec<AgentAccount> = match (harness, self.snapshot(cx)) {
            (Some(harness), Some(snapshot)) => accounts::provider_accounts(snapshot, harness)
                .into_iter()
                .cloned()
                .collect(),
            _ => Vec::new(),
        };
        let rotation = match (harness, self.snapshot(cx)) {
            (Some(harness), Some(snapshot)) => {
                account_rotation::provider_rotation(snapshot, harness)
            }
            _ => None,
        };
        let rotation_row = self.rotation_row(rotation.as_ref(), &rows, theme, cx);
        // The account rotation started this chat on, marked among the rows.
        let this_chat = self
            .chat
            .as_deref()
            .zip(self.snapshot(cx))
            .and_then(|(chat, snapshot)| {
                let id = snapshot.chat_account(chat)?.id.clone();
                let moved = account_rotation::chat_account(snapshot, chat)?
                    .moved_from
                    .is_some();
                Some((id, moved))
            });
        let title = format!(
            "{} accounts",
            harness.map_or("Agent", accounts::provider_name)
        );
        popover::popover_card(theme)
            .w(px(400.0))
            .flex()
            .flex_col()
            .child(popover::menu_heading(theme, &title))
            .children(rows.iter().enumerate().map(|(ix, account)| {
                let email: SharedString = account
                    .email
                    .clone()
                    .or_else(|| account.display_name.clone())
                    .unwrap_or_else(|| "Unknown account".into())
                    .into();
                let can_switch = !account.active && account.switchable;
                let mut meta: Vec<gpui::AnyElement> = account
                    .plan_label
                    .iter()
                    .map(|plan| {
                        div()
                            .child(SharedString::from(plan.clone()))
                            .into_any_element()
                    })
                    .collect();
                let marks = account_rotation::markers(rotation.as_ref(), &account.id);
                if let Some((_, moved)) = this_chat.as_ref().filter(|(id, _)| *id == account.id) {
                    meta.push(
                        div()
                            .text_color(theme.accent)
                            .child(SharedString::from(if *moved {
                                "This chat · moved here"
                            } else {
                                "This chat"
                            }))
                            .into_any_element(),
                    );
                }
                if account.active {
                    meta.push(
                        div()
                            .text_color(theme.accent)
                            .child(SharedString::from(account_rotation::active_label(
                                rotation.as_ref(),
                            )))
                            .into_any_element(),
                    );
                } else if marks.is_empty()
                    && let Some(reason) = account
                        .usage_error
                        .clone()
                        .filter(|_| account.usage_windows.is_empty())
                {
                    meta.push(div().child(SharedString::from(reason)).into_any_element());
                }
                meta.extend(account_rotation::marker_fragments(&marks, theme));
                let switch_to = account.clone();
                popover::menu_row(theme, account.active, format!("account-usage-row-{ix}"))
                    .id(("account-usage-row", ix))
                    .when(!can_switch, |row| row.cursor_default())
                    .when(can_switch, |row| {
                        row.on_click(cx.listener(move |usage, _, _, cx| {
                            usage.switch(&switch_to, cx);
                        }))
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(12.5))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .child(email),
                            )
                            .when(!meta.is_empty(), |el| {
                                el.child(crate::settings::widgets::meta_line(theme, meta))
                            }),
                    )
                    .child(
                        div()
                            .id(("account-usage-meters", ix))
                            .flex_none()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .when(!account.usage_windows.is_empty(), |meters| {
                                meters.tooltip(widgets::text_tooltip(accounts::usage_resets(
                                    &account.usage_windows,
                                    now,
                                )))
                            })
                            .children(
                                account
                                    .usage_windows
                                    .iter()
                                    .take(2)
                                    .map(|window| render_usage_meter(window, theme)),
                            ),
                    )
            }))
            .when_some(rotation_row, |card, row| {
                card.child(popover::menu_separator()).child(row)
            })
            .children(self.error.clone().map(|error| {
                div()
                    .px(px(8.0))
                    .py(px(4.0))
                    .text_size(px(12.0))
                    .text_color(theme.danger)
                    .child(error)
            }))
    }
}

/// Which footer ring's popover is up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FooterCard {
    Accounts,
    Context,
}

impl FooterCard {
    fn id(self) -> &'static str {
        match self {
            FooterCard::Accounts => "account-usage-menu",
            FooterCard::Context => "context-usage-menu",
        }
    }
}

/// The footer's ring cluster: account usage (accent arc, so the two read
/// apart), then context occupancy. Each opens its popover on click.
impl Render for AccountUsage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let context = self.state.read(cx).context_usage;
        let account = self.fraction(cx).map(|fraction| {
            let level = usage_level(fraction);
            let chip = crate::context_usage::ring_chip(
                "account-usage",
                fraction,
                usage_color(level, &theme),
                match level {
                    UsageLevel::Normal => theme.text_muted,
                    _ => usage_color(level, &theme),
                },
                format!("{}%", (fraction * 100.0).round() as u32),
                self.popup.get() == Some(&FooterCard::Accounts),
                &theme,
            );
            self.trigger(
                chip,
                FooterCard::Accounts,
                |usage, cx| usage.accounts_card(cx),
                cx,
            )
        });
        let context = crate::context_usage::has_window(context).then(|| {
            let chip = crate::context_usage::chip(
                context,
                self.popup.get() == Some(&FooterCard::Context),
                &theme,
            );
            self.trigger(
                chip,
                FooterCard::Context,
                move |_, cx| crate::context_usage::card(context, &Theme::of(cx).for_popup()),
                cx,
            )
        });
        div()
            .flex()
            .items_center()
            .gap(px(4.0))
            .children(account)
            .children(context)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeron_proto::AgentUsageWindow;

    fn account(harness: HarnessId, active: bool, used: &[f32]) -> AgentAccount {
        serde_json::from_value(serde_json::json!({
            "id": format!("{harness:?}-{active}"),
            "harness": harness,
            "email": null,
            "planLabel": null,
            "active": active,
            "switchable": true,
            "usageWindows": used
                .iter()
                .map(|fraction| AgentUsageWindow {
                    label: "5h".into(),
                    used_fraction: *fraction,
                    resets_at: None,
                })
                .collect::<Vec<_>>(),
        }))
        .unwrap()
    }

    #[test]
    fn ring_shows_the_most_used_window() {
        assert_eq!(used_fraction(&account(HarnessId::Codex, true, &[])), None);
        assert_eq!(
            used_fraction(&account(HarnessId::Codex, true, &[0.12, 0.64])),
            Some(0.64)
        );
        assert_eq!(
            used_fraction(&account(HarnessId::Codex, true, &[1.4])),
            Some(1.0)
        );
    }

    #[test]
    fn active_account_is_scoped_to_the_harness() {
        let snapshot = AgentAccountsSnapshot {
            accounts: vec![
                account(HarnessId::ClaudeCode, true, &[0.3]),
                account(HarnessId::Codex, false, &[0.1]),
                account(HarnessId::Codex, true, &[0.2]),
            ],
            warnings: Vec::new(),
            rotation: Vec::new(),
        };
        assert_eq!(
            active_account(&snapshot, HarnessId::Codex).map(|a| a.id.as_str()),
            Some("Codex-true")
        );
        assert!(active_account(&snapshot, HarnessId::Cursor).is_none());
    }

    fn rotation(harness: HarnessId, enabled: bool) -> zeron_proto::AgentAccountRotation {
        zeron_proto::AgentAccountRotation {
            harness,
            supported: true,
            enabled,
            active: enabled,
            accounts: vec![format!("{harness:?}-true"), format!("{harness:?}-false")],
            next_account_id: None,
            chats: Vec::new(),
            note: None,
            skipped: Vec::new(),
        }
    }

    fn pinned(chat: &str, account: &str) -> zeron_proto::AgentAccountChat {
        zeron_proto::AgentAccountChat {
            chat_id: chat.into(),
            account_id: account.into(),
            assigned_at: 0,
            used_at: 0,
            previous_account_id: None,
            moved_reason: None,
            running: false,
        }
    }

    /// Two Codex and two Claude accounts; Codex rotates and has put chat
    /// "c1" on its saved (not live) account.
    fn rotating() -> AgentAccountsSnapshot {
        let mut codex = rotation(HarnessId::Codex, true);
        codex.chats = vec![pinned("c1", "Codex-false")];
        AgentAccountsSnapshot {
            accounts: vec![
                account(HarnessId::ClaudeCode, true, &[0.3]),
                account(HarnessId::ClaudeCode, false, &[0.9]),
                account(HarnessId::Codex, true, &[0.2]),
                account(HarnessId::Codex, false, &[0.7]),
            ],
            warnings: Vec::new(),
            rotation: vec![codex, rotation(HarnessId::ClaudeCode, false)],
        }
    }

    #[test]
    fn the_ring_reads_the_account_rotation_put_the_chat_on() {
        let snapshot = rotating();
        let read = |harness, chat: Option<&str>| {
            chat_account(&snapshot, harness, chat).map(|a| a.id.as_str())
        };
        // The rotated chat: its own account, not the live one.
        assert_eq!(read(HarnessId::Codex, Some("c1")), Some("Codex-false"));
        // A chat rotation didn't place, or none: the live account.
        assert_eq!(read(HarnessId::Codex, Some("other")), Some("Codex-true"));
        assert_eq!(read(HarnessId::Codex, None), Some("Codex-true"));
        // The composer switched agents: the pin belongs to another one.
        assert_eq!(
            read(HarnessId::ClaudeCode, Some("c1")),
            Some("ClaudeCode-true")
        );
        assert_eq!(
            used_fraction(chat_account(&snapshot, HarnessId::Codex, Some("c1")).unwrap()),
            Some(0.7)
        );
    }

    type Calls = std::sync::Arc<std::sync::Mutex<Vec<(String, serde_json::Value)>>>;

    /// Lists answer [`rotating`]; flips are refused.
    fn engine(calls: Calls) -> std::sync::Arc<crate::pull_request_test_support::ScriptedRpc> {
        crate::pull_request_test_support::ScriptedRpc::new(move |method, params| {
            calls.lock().unwrap().push((method.clone(), params));
            async move {
                match method.as_str() {
                    methods::LIST_AGENT_ACCOUNTS => zeron_rpc::RpcReply::value(&rotating()),
                    account_rotation::SET_METHOD => {
                        Err(zeron_rpc::RpcError::Failed("rotation refused".into()))
                    }
                    other => panic!("unexpected request {other}"),
                }
            }
        })
    }

    fn cached(cx: &mut gpui::TestAppContext, key: Option<String>) -> AgentAccountsSnapshot {
        cx.update(|cx| {
            cx.global::<AccountsSnapshotCache>()
                .0
                .get(&key)
                .cloned()
                .unwrap()
        })
    }

    /// A refused flip puts back only its own provider's rotation (another
    /// provider's flip that landed meanwhile stays), on the device the card
    /// targets, and the card says why.
    #[gpui::test]
    fn a_refused_flip_reverts_only_its_provider_on_its_device(cx: &mut gpui::TestAppContext) {
        use crate::pull_request_test_support as fixture;
        use gpui::AppContext as _;
        let runtime = fixture::runtime();
        let _guard = runtime.enter();
        fixture::init(cx);
        let calls = Calls::default();
        let rpc = engine(calls.clone());
        let state = cx.update(|cx| fixture::state(cx, Some(rpc.client())));
        let usage = cx.new(|cx| AccountUsage::new(state, cx));
        let remote = Some("remote".to_string());
        let mut off = rotating();
        off.rotation[0].enabled = false;
        off.rotation[0].active = false;
        cx.update(|cx| {
            cx.default_global::<AccountsSnapshotCache>()
                .0
                .insert(remote.clone(), off);
        });
        usage.update(cx, |usage, cx| {
            usage.target = remote.clone();
            usage.loaded = true;
            usage.harness = Some(HarnessId::Codex);
            usage.set_rotation(HarnessId::Codex, true, cx);
        });
        // Optimistic: on at once.
        assert!(
            cached(cx, remote.clone())
                .rotation_for(HarnessId::Codex)
                .unwrap()
                .enabled
        );
        // Meanwhile Claude's rotation is turned on elsewhere.
        cx.update(|cx| {
            let cache = &mut cx.default_global::<AccountsSnapshotCache>().0;
            account_rotation::mark_enabled(
                cache.get_mut(&remote).unwrap(),
                HarnessId::ClaudeCode,
                true,
            );
        });
        rpc.settle(cx, &runtime, |cx| {
            usage.read_with(cx, |usage, _| usage.rotation_task.is_none())
        });
        let after = cached(cx, remote.clone());
        assert!(!after.rotation_for(HarnessId::Codex).unwrap().enabled);
        assert!(after.rotation_for(HarnessId::ClaudeCode).unwrap().enabled);
        usage.read_with(cx, |usage, _| {
            assert!(
                usage
                    .error
                    .as_ref()
                    .is_some_and(|error| error.contains("rotation refused"))
            );
        });
        let calls = calls.lock().unwrap();
        let (method, params) = calls.last().unwrap();
        assert_eq!(method, account_rotation::SET_METHOD);
        assert_eq!(params["targetDeviceId"], "remote");
        assert_eq!(params["harness"], "codex");
        assert_eq!(params["enabled"], true);
    }

    /// The chat's account shows up promptly: switching chats re-reads the
    /// engine's cached list, as does a turn starting or ending while
    /// rotation is in effect — never with a usage probe.
    #[gpui::test]
    fn chat_switches_and_turns_reread_the_cached_list(cx: &mut gpui::TestAppContext) {
        use crate::pull_request_test_support as fixture;
        use gpui::AppContext as _;
        let runtime = fixture::runtime();
        let _guard = runtime.enter();
        fixture::init(cx);
        let calls = Calls::default();
        let rpc = engine(calls.clone());
        let state = cx.update(|cx| fixture::state(cx, Some(rpc.client())));
        let usage = cx.new(|cx| AccountUsage::new(state, cx));
        cx.update(|cx| {
            cx.default_global::<AccountsSnapshotCache>()
                .0
                .insert(None, rotating());
        });
        let lists = |calls: &Calls| {
            calls
                .lock()
                .unwrap()
                .iter()
                .filter(|(method, _)| method == methods::LIST_AGENT_ACCOUNTS)
                .map(|(_, params)| params["forceUsage"].as_bool().unwrap())
                .collect::<Vec<_>>()
        };
        let track = |cx: &mut gpui::TestAppContext, chat: &str, running: bool| {
            usage.update(cx, |usage, cx| {
                usage.track(Some(HarnessId::Codex), None, Some(chat.into()), running, cx)
            });
        };
        usage.update(cx, |usage, _| usage.loaded = true);
        // A different chat: one cache-only list.
        track(cx, "c1", false);
        rpc.settle(cx, &runtime, |_| rpc.completed() == 1);
        assert_eq!(lists(&calls), [false]);
        // Same chat, same turn state: nothing.
        track(cx, "c1", false);
        cx.run_until_parked();
        assert_eq!(lists(&calls).len(), 1);
        // Its turn starts, then ends: Codex rotates, so each re-reads.
        track(cx, "c1", true);
        rpc.settle(cx, &runtime, |_| rpc.completed() == 2);
        track(cx, "c1", false);
        rpc.settle(cx, &runtime, |_| rpc.completed() == 3);
        assert_eq!(lists(&calls), [false, false, false]);
        // Claude doesn't rotate: its turns don't.
        usage.update(cx, |usage, cx| {
            usage.track(
                Some(HarnessId::ClaudeCode),
                None,
                Some("c1".into()),
                true,
                cx,
            )
        });
        cx.run_until_parked();
        assert_eq!(lists(&calls).len(), 3);
    }
}
