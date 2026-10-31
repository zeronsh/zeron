//! Plan-usage cards at the foot of the chat sidebar: one per enabled provider
//! that reports usage, showing the live account's meters. The meters are the
//! composer ring popover's, stretched to the card width, and the data is the same
//! [`AccountsSnapshotCache`], so a switch or probe anywhere shows up here.
use std::collections::HashMap;

use gpui::{Context, Entity, IntoElement, Render, Subscription, Task, Window, div, prelude::*, px};
use zeron_engine::registry::HarnessDescriptor;
use zeron_proto::{AgentAccount, AgentAccountsSnapshot, HarnessId};
use zeron_rpc::methods;

use crate::accounts_resource::{AccountsResource, target_params, usage_account};
use crate::icons::icon;
use crate::pickers::{HarnessCatalogChanged, harness_brand_icon, offered_harnesses};
use crate::settings::accounts::{
    self, AccountsSnapshotCache, provider_name, render_sidebar_usage_meter, reports_usage, signs_in,
};
use crate::settings::widgets;
use crate::state::AppState;
use crate::theme::Theme;

/// Windows per card — the composer popover's cap, so a card stays compact.
const MAX_METERS: usize = 2;

/// The providers to show cards for, in Settings order: offered on the host
/// (enabled and installed) and able to report usage. Pure.
pub fn usage_providers(catalog: &[HarnessDescriptor]) -> Vec<HarnessId> {
    let offered: Vec<HarnessId> = offered_harnesses(catalog).iter().map(|d| d.id).collect();
    accounts::PROVIDERS
        .iter()
        .map(|(harness, _, _)| *harness)
        .filter(|harness| {
            offered.contains(harness) && signs_in(*harness) && reports_usage(*harness)
        })
        .collect()
}

/// The accounts to draw a card for: each provider's live account, when it has
/// usage windows to show. Pure.
pub fn card_accounts<'a>(
    snapshot: &'a AgentAccountsSnapshot,
    providers: &[HarnessId],
) -> Vec<&'a AgentAccount> {
    providers
        .iter()
        .filter_map(|harness| usage_account(snapshot, *harness))
        .collect()
}

/// Normalize local queries to the shared cache's `None` key.
fn sidebar_target(state: &AppState) -> Option<String> {
    state
        .selected_chat_row()
        .map(|chat| chat.device_id.clone())
        .filter(|device| !device.is_empty())
        .or_else(|| state.effective_device_id())
        .filter(|device| state.local_device_id.as_ref() != Some(device))
}

fn cached_usage_label(fetched_at: Option<i64>, now: i64) -> String {
    match fetched_at {
        Some(at) => {
            let minutes = now.saturating_sub(at).max(0) / 60_000;
            if minutes == 0 {
                "Cached · updated just now".into()
            } else if minutes < 60 {
                format!("Cached · updated {minutes}m ago")
            } else if minutes < 1440 {
                format!("Cached · updated {}h ago", minutes / 60)
            } else {
                format!("Cached · updated {}d ago", minutes / 1440)
            }
        }
        None => "Cached · update time unknown".into(),
    }
}

/// Loads the selected host's catalog and accounts through the device relay.
pub struct SidebarUsage {
    state: Entity<AppState>,
    target: Option<String>,
    generation: u64,
    /// Retain each host's catalog so cached accounts remain visible offline.
    providers: HashMap<Option<String>, Vec<HarnessId>>,
    requested: bool,
    catalog_task: Option<Task<()>>,
    resource: Entity<AccountsResource>,
    _resource: Subscription,
    _cache: Subscription,
    _catalog: Subscription,
    _state: Subscription,
}

impl SidebarUsage {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let resource = AccountsResource::for_target(state.clone(), None, cx);
        let observation = cx.observe(&resource, |usage: &mut Self, resource, cx| {
            if !resource.read(cx).refreshing {
                usage.load_catalog(cx);
            }
            cx.notify();
        });
        Self {
            _state: cx.observe(&state, |_, _, cx| cx.notify()),
            state,
            target: None,
            generation: 0,
            providers: HashMap::new(),
            requested: false,
            catalog_task: None,
            resource,
            _resource: observation,
            // Settings → Accounts and the composer ring write the same cache.
            _cache: cx.observe_global::<AccountsSnapshotCache>(|_, cx| cx.notify()),
            // A Settings → Providers toggle changes which cards belong.
            _catalog: cx.observe_global::<HarnessCatalogChanged>(|usage, cx| {
                usage.load_catalog(cx);
            }),
        }
    }

    /// Follow the same host as the composer, including an open chat's host.
    fn ensure_loaded(&mut self, cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        let target = sidebar_target(state);
        if self.target != target {
            self.target = target;
            self.generation += 1;
            self.requested = false;
            self.catalog_task = None;
            self.resource =
                AccountsResource::for_target(self.state.clone(), self.target.clone(), cx);
            self._resource = cx.observe(&self.resource, |usage, resource, cx| {
                if !resource.read(cx).refreshing {
                    usage.load_catalog(cx);
                }
                cx.notify();
            });
        }
        self.resource
            .update(cx, |resource, cx| resource.ensure_loaded(cx));
        if self.requested || self.state.read(cx).engine().is_none() {
            return;
        }
        self.requested = true;
        self.load_catalog(cx);
    }

    fn load_catalog(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let target = self.target.clone();
        let generation = self.generation;
        self.catalog_task = Some(cx.spawn(async move |this, cx| {
            let list = engine
                .client()
                .call(
                    methods::LIST_HARNESSES,
                    target_params(&target, serde_json::json!({})),
                )
                .await
                .ok()
                .and_then(|value| serde_json::from_value::<Vec<HarnessDescriptor>>(value).ok());
            if let Some(list) = list {
                this.update(cx, |usage, cx| {
                    if usage.generation != generation {
                        return;
                    }
                    usage.providers.insert(target, usage_providers(&list));
                    cx.notify();
                })
                .ok();
            }
        }));
    }

    fn card(
        account: &AgentAccount,
        theme: &Theme,
        now: chrono::DateTime<chrono::Utc>,
        unavailable: bool,
    ) -> gpui::Stateful<gpui::Div> {
        let (mark, tint) = harness_brand_icon(account.harness);
        let windows = &account.usage_windows;
        let provider = accounts::login_options(account.harness)
            .into_iter()
            .find(|option| {
                option.provider.is_some() && option.provider == account.provider.as_deref()
            });
        let name = match provider {
            Some(provider) => format!("{} · {}", provider_name(account.harness), provider.label),
            None => provider_name(account.harness).to_string(),
        };
        let mut title = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .child(
                icon(mark)
                    .size(px(13.0))
                    .text_color(tint.unwrap_or(theme.text_muted)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(crate::typography::ui_rems(11.5))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(theme.text)
                    .child(name),
            );
        if let Some(plan) = account.plan_label.clone() {
            title = title.child(
                div()
                    .flex_none()
                    .text_size(crate::typography::ui_rems(11.0))
                    .text_color(theme.text_muted)
                    .child(plan),
            );
        }
        div()
            .id(("sidebar-usage-card", account.harness as usize))
            .px(px(Theme::SPACE_SM))
            .py(px(6.0))
            .rounded(px(Theme::CONTROL_RADIUS))
            .bg(theme.wash(0.05))
            .flex()
            .flex_col()
            .gap(px(2.0))
            .tooltip(widgets::text_tooltip(accounts::usage_resets(windows, now)))
            .child(title)
            .children(
                windows
                    .iter()
                    .take(MAX_METERS)
                    .map(|window| render_sidebar_usage_meter(window, theme)),
            )
            .when(unavailable || account.usage_error.is_some(), |el| {
                el.child(
                    div()
                        .text_size(crate::typography::ui_rems(10.0))
                        .text_color(theme.text_muted)
                        .child(cached_usage_label(
                            account.usage_fetched_at,
                            now.timestamp_millis(),
                        )),
                )
            })
    }
}

impl Render for SidebarUsage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_loaded(cx);
        let theme = Theme::of(cx).clone();
        let now = chrono::Utc::now();
        let unavailable = self.resource.read(cx).error.is_some()
            || self
                .target
                .as_deref()
                .is_some_and(|target| !self.state.read(cx).device_online(target, now));
        let cards: Vec<gpui::Stateful<gpui::Div>> = match (
            self.providers.get(&self.target),
            cx.try_global::<AccountsSnapshotCache>()
                .and_then(|cache| cache.0.get(&self.target)),
        ) {
            (Some(providers), Some(snapshot)) => card_accounts(snapshot, providers)
                .into_iter()
                .map(|account| Self::card(account, &theme, now, unavailable))
                .collect(),
            _ => Vec::new(),
        };
        // Nothing to show takes no room: the padding rides with the cards.
        div()
            .id("sidebar-usage")
            .debug_selector(|| "sidebar-usage".into())
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .when(!cards.is_empty(), |el| {
                el.px(px(Theme::SPACE_SM)).pt(px(Theme::SPACE_SM))
            })
            .children(cards)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_follows_selected_host_and_open_chat() {
        let mut state = AppState::new();
        state.local_device_id = Some("viewer".into());
        assert_eq!(sidebar_target(&state), None);

        state.selected_device = Some("home".into());
        assert_eq!(sidebar_target(&state).as_deref(), Some("home"));
        let params = target_params(
            &sidebar_target(&state),
            serde_json::json!({"forceUsage": true}),
        );
        assert_eq!(
            params,
            serde_json::json!({"targetDeviceId": "home", "forceUsage": true})
        );

        state.chats = vec![
            serde_json::from_value(serde_json::json!({
                "id": "remote-chat",
                "deviceId": "other-host",
                "archived": false,
                "createdAt": "2026-10-25T12:00:00Z",
            }))
            .unwrap(),
        ];
        state.selected_chat = Some("remote-chat".into());
        assert_eq!(sidebar_target(&state).as_deref(), Some("other-host"));

        state.chats[0].device_id = "viewer".into();
        assert_eq!(sidebar_target(&state), None);
        assert_eq!(
            target_params(&sidebar_target(&state), serde_json::json!({})),
            serde_json::json!({})
        );

        state.selected_chat = None;
        assert_eq!(sidebar_target(&state).as_deref(), Some("home"));
    }

    #[test]
    fn cached_readings_show_age_even_without_a_probe_timestamp() {
        assert_eq!(cached_usage_label(None, 0), "Cached · update time unknown");
        assert_eq!(
            cached_usage_label(Some(60_000), 0),
            "Cached · updated just now"
        );
        assert_eq!(
            cached_usage_label(Some(0), 120_000),
            "Cached · updated 2m ago"
        );
        assert_eq!(
            cached_usage_label(Some(0), 7_200_000),
            "Cached · updated 2h ago"
        );
        assert_eq!(
            cached_usage_label(Some(0), 172_800_000),
            "Cached · updated 2d ago"
        );
    }

    fn descriptor(id: HarnessId, installed: bool, enabled: Option<bool>) -> HarnessDescriptor {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "name": "x",
            "supportsSteering": false,
            "steeringMode": "turn-boundary",
            "reasoningLevels": [],
            "installed": installed,
            "enabled": enabled,
        }))
        .unwrap()
    }

    fn account(harness: HarnessId, active: bool, windows: usize) -> AgentAccount {
        serde_json::from_value(serde_json::json!({
            "id": format!("{harness:?}-{active}"),
            "harness": harness,
            "email": null,
            "planLabel": null,
            "active": active,
            "switchable": true,
            "usageWindows": (0..windows)
                .map(|_| serde_json::json!({ "label": "5h", "usedFraction": 0.4, "resetsAt": null }))
                .collect::<Vec<_>>(),
        }))
        .unwrap()
    }

    #[test]
    fn providers_are_enabled_installed_and_report_usage() {
        let catalog = vec![
            descriptor(HarnessId::Codex, true, Some(true)),
            descriptor(HarnessId::ClaudeCode, true, Some(true)),
            descriptor(HarnessId::Cursor, true, Some(false)),
            descriptor(HarnessId::Grok, false, Some(true)),
            // Enabled and installed, but reports no plan usage.
            descriptor(HarnessId::Antigravity, true, Some(true)),
        ];
        // Settings order, not catalog order.
        assert_eq!(
            usage_providers(&catalog),
            vec![HarnessId::ClaudeCode, HarnessId::Codex]
        );
    }

    #[test]
    fn cards_show_live_accounts_that_have_usage() {
        let snapshot = AgentAccountsSnapshot {
            accounts: vec![
                account(HarnessId::ClaudeCode, true, 2),
                account(HarnessId::Codex, false, 2),
                account(HarnessId::Codex, true, 0),
                account(HarnessId::Cursor, true, 1),
            ],
            warnings: Vec::new(),
        };
        let providers = [HarnessId::ClaudeCode, HarnessId::Codex];
        let cards = card_accounts(&snapshot, &providers);
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].harness, HarnessId::ClaudeCode);
    }

    #[test]
    fn cards_skip_active_provider_groups_without_readings() {
        let mut empty = account(HarnessId::Opencode, true, 0);
        empty.id = "copilot".into();
        let mut measured = account(HarnessId::Opencode, true, 1);
        measured.id = "openai".into();
        let snapshot = AgentAccountsSnapshot {
            accounts: vec![empty, measured],
            warnings: vec![],
        };
        let cards = card_accounts(&snapshot, &[HarnessId::Opencode]);
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].id, "openai");
    }

    #[test]
    fn cards_choose_highest_usage_and_break_ties_independently_of_row_order() {
        let mut low = account(HarnessId::Opencode, true, 1);
        low.id = "a".into();
        let mut high = low.clone();
        high.id = "b".into();
        high.usage_windows[0].used_fraction = 0.9;
        let mut snapshot = AgentAccountsSnapshot {
            accounts: vec![low, high],
            warnings: vec![],
        };
        assert_eq!(card_accounts(&snapshot, &[HarnessId::Opencode])[0].id, "b");
        snapshot.accounts.reverse();
        assert_eq!(card_accounts(&snapshot, &[HarnessId::Opencode])[0].id, "b");
        snapshot.accounts[1].usage_windows[0].used_fraction = 0.9;
        assert_eq!(card_accounts(&snapshot, &[HarnessId::Opencode])[0].id, "a");
        snapshot.accounts.reverse();
        assert_eq!(card_accounts(&snapshot, &[HarnessId::Opencode])[0].id, "a");
    }
}
