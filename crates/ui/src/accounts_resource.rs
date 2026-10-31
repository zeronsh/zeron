//! One refresh lifecycle per watched host, shared by every accounts presentation.
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use gpui::{App, AppContext as _, Context, Entity, EntityId, Global, Task, WeakEntity};
use zeron_proto::{AgentAccount, AgentAccountsSnapshot, HarnessId};
use zeron_rpc::methods;

use crate::state::AppState;

/// Last snapshots survive view lifetimes; optimistic account changes do not
/// alter the refresh outcome owned by the watched resource.
#[derive(Default)]
pub(crate) struct AccountsSnapshotCache(pub(crate) HashMap<Option<String>, AgentAccountsSnapshot>);
impl Global for AccountsSnapshotCache {}

const FORCE_MIN_INTERVAL: Duration = Duration::from_secs(30);
const POLL_INTERVAL: Duration = Duration::from_secs(5 * 60);

#[derive(Default)]
struct Resources(HashMap<(EntityId, Option<String>), WeakEntity<AccountsResource>>);
impl Global for Resources {}

/// Explicit actions refresh even inside the background probe throttle (for
/// example, a newly completed login must immediately appear in Settings).
#[derive(Clone, Copy)]
pub(crate) enum Refresh {
    Background,
    Explicit,
    Accounts,
}

pub(crate) fn target_params(
    target: &Option<String>,
    mut params: serde_json::Value,
) -> serde_json::Value {
    if let Some(target) = target {
        params["targetDeviceId"] = serde_json::json!(target);
    }
    params
}

/// The binding limit: the most-used window of the account. Pure.
pub fn used_fraction(account: &AgentAccount) -> Option<f32> {
    account
        .usage_windows
        .iter()
        .map(|window| window.used_fraction.clamp(0.0, 1.0))
        .reduce(f32::max)
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

/// One sidebar card per harness: the active account with the most constrained
/// reported window. Stable id ordering breaks ties independently of RPC order.
pub(crate) fn usage_account(
    snapshot: &AgentAccountsSnapshot,
    harness: HarnessId,
) -> Option<&AgentAccount> {
    snapshot
        .accounts
        .iter()
        .filter(|account| {
            account.harness == harness && account.active && !account.usage_windows.is_empty()
        })
        .max_by(|a, b| {
            used_fraction(a)
                .unwrap_or_default()
                .total_cmp(&used_fraction(b).unwrap_or_default())
                .then_with(|| b.id.cmp(&a.id))
        })
}

pub(crate) struct AccountsResource {
    state: Entity<AppState>,
    target: Option<String>,
    requested: bool,
    last_forced: Option<Instant>,
    pub(crate) refreshing: bool,
    pub(crate) error: Option<String>,
    load: Option<Task<()>>,
    pending: Option<Refresh>,
    _poll: Task<()>,
}

impl AccountsResource {
    pub(crate) fn for_target(
        state: Entity<AppState>,
        target: Option<String>,
        cx: &mut App,
    ) -> Entity<Self> {
        let key = (state.entity_id(), target.clone());
        if let Some(resource) = cx
            .default_global::<Resources>()
            .0
            .get(&key)
            .and_then(WeakEntity::upgrade)
        {
            return resource;
        }
        let resource = cx.new(|cx: &mut Context<Self>| Self {
            state,
            target,
            requested: false,
            last_forced: None,
            refreshing: false,
            error: None,
            load: None,
            pending: None,
            _poll: cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(POLL_INTERVAL).await;
                    if this
                        .update(cx, |resource, cx| resource.refresh(Refresh::Background, cx))
                        .is_err()
                    {
                        break;
                    }
                }
            }),
        });
        let resources = &mut cx.default_global::<Resources>().0;
        resources.retain(|_, resource| resource.upgrade().is_some());
        resources.insert(key, resource.downgrade());
        resource
    }

    pub(crate) fn ensure_loaded(&mut self, cx: &mut Context<Self>) {
        if !self.requested {
            self.refresh(Refresh::Background, cx);
        }
    }

    pub(crate) fn refresh(&mut self, mode: Refresh, cx: &mut Context<Self>) {
        if self.refreshing {
            // A login or account action may finish while an older list is in
            // flight. Re-list afterwards rather than losing the new account.
            if !matches!(mode, Refresh::Background) {
                self.pending = Some(match self.pending {
                    Some(Refresh::Explicit) => Refresh::Explicit,
                    _ => mode,
                });
            }
            return;
        }
        if matches!(mode, Refresh::Background)
            && self
                .last_forced
                .is_some_and(|at| at.elapsed() < FORCE_MIN_INTERVAL)
        {
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            if self.error.as_deref() != Some("Engine not connected") {
                self.error = Some("Engine not connected".into());
                cx.notify();
            }
            return;
        };
        let force_usage = !matches!(mode, Refresh::Accounts);
        if force_usage {
            self.last_forced = Some(Instant::now());
        }
        self.requested = true;
        self.refreshing = true;
        let target = self.target.clone();
        let paint_first = force_usage
            && cx
                .try_global::<AccountsSnapshotCache>()
                .is_none_or(|cache| !cache.0.contains_key(&target));
        self.load = Some(cx.spawn(async move |this, cx| {
            let fetch = |force_usage| {
                let engine = engine.clone();
                let params = target_params(&target, serde_json::json!({"forceUsage": force_usage}));
                async move {
                    engine
                        .client()
                        .call(methods::LIST_AGENT_ACCOUNTS, params)
                        .await
                        .map_err(|err| err.to_string())
                        .and_then(|value| {
                            serde_json::from_value::<AgentAccountsSnapshot>(value)
                                .map_err(|err| err.to_string())
                        })
                }
            };
            if paint_first && let Ok(snapshot) = fetch(false).await {
                this.update(cx, |_, cx| {
                    cx.default_global::<AccountsSnapshotCache>()
                        .0
                        .insert(target.clone(), snapshot);
                    cx.notify();
                })
                .ok();
            }
            let result = fetch(force_usage).await;
            this.update(cx, |resource, cx| {
                resource.complete(result, cx);
                if let Some(mode) = resource.pending.take() {
                    // Do not replace (and cancel) the task that is finishing.
                    let resource = cx.entity().downgrade();
                    cx.defer(move |cx| {
                        resource
                            .update(cx, |resource, cx| resource.refresh(mode, cx))
                            .ok();
                    });
                }
            })
            .ok();
        }));
        cx.notify();
    }

    fn complete(&mut self, result: Result<AgentAccountsSnapshot, String>, cx: &mut Context<Self>) {
        self.refreshing = false;
        match result {
            Ok(snapshot) => {
                cx.default_global::<AccountsSnapshotCache>()
                    .0
                    .insert(self.target.clone(), snapshot);
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(label: &str) -> AgentAccountsSnapshot {
        AgentAccountsSnapshot {
            accounts: vec![],
            warnings: vec![zeron_proto::AgentAccountWarning {
                harness: HarnessId::Codex,
                message: label.into(),
            }],
        }
    }

    #[gpui::test]
    fn views_share_a_resource_and_only_watched_targets_keep_polling(cx: &mut gpui::TestAppContext) {
        let weak = cx.update(|cx| {
            let state = cx.new(|_| AppState::new());
            let sidebar = AccountsResource::for_target(state.clone(), None, cx);
            let settings = AccountsResource::for_target(state.clone(), None, cx);
            assert_eq!(sidebar.entity_id(), settings.entity_id());
            let remote = AccountsResource::for_target(state.clone(), Some("remote".into()), cx);
            assert_ne!(sidebar.entity_id(), remote.entity_id());
            sidebar.update(cx, |resource, cx| {
                resource.complete(Ok(snapshot("cached")), cx)
            });
            let weak = sidebar.downgrade();
            drop(sidebar);
            assert!(weak.upgrade().is_some());
            drop(settings);
            assert_eq!(
                cx.global::<AccountsSnapshotCache>().0[&None].warnings[0].message,
                "cached"
            );
            weak
        });
        cx.run_until_parked();
        assert!(
            weak.upgrade().is_none(),
            "the registry and poll task must not retain an unwatched resource"
        );
    }

    #[gpui::test]
    fn successful_external_refresh_clears_only_its_targets_failure(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let state = cx.new(|_| AppState::new());
            let sidebar = AccountsResource::for_target(state.clone(), None, cx);
            sidebar.update(cx, |resource, cx| {
                resource.complete(Ok(snapshot("old")), cx)
            });
            sidebar.update(cx, |resource, cx| {
                resource.complete(Err("offline".into()), cx)
            });
            assert_eq!(
                cx.global::<AccountsSnapshotCache>().0[&None].warnings[0].message,
                "old",
                "a failed refresh keeps the painted snapshot"
            );

            // Optimistic account changes are not successful usage probes.
            cx.default_global::<AccountsSnapshotCache>()
                .0
                .insert(None, snapshot("optimistic"));
            assert_eq!(sidebar.read(cx).error.as_deref(), Some("offline"));
            let other = AccountsResource::for_target(state.clone(), Some("other".into()), cx);
            other.update(cx, |resource, cx| {
                resource.complete(Ok(snapshot("other")), cx)
            });
            assert_eq!(sidebar.read(cx).error.as_deref(), Some("offline"));

            // Settings and the sidebar share both the result and its freshness.
            let settings = AccountsResource::for_target(state, None, cx);
            settings.update(cx, |resource, cx| {
                resource.complete(Ok(snapshot("fresh")), cx)
            });
            assert!(sidebar.read(cx).error.is_none());
            assert_eq!(
                cx.global::<AccountsSnapshotCache>().0[&None].warnings[0].message,
                "fresh"
            );
            assert_eq!(
                cx.global::<AccountsSnapshotCache>().0[&Some("other".into())].warnings[0].message,
                "other"
            );
        });
    }

    #[gpui::test]
    fn shared_requests_paint_cached_usage_then_probe_and_preserve_a_pending_account_reload(
        cx: &mut gpui::TestAppContext,
    ) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _guard = runtime.enter();
        let (out, mut requests) = tokio::sync::mpsc::channel::<String>(16);
        let (replies, inbound) = tokio::sync::mpsc::channel::<String>(16);
        let resource = cx.update(|cx| {
            let state = cx.new(|_| {
                let mut state = AppState::new();
                state.set_test_engine(crate::state::EngineHandle::from_test_client(
                    zeron_rpc::RpcClient::new(out, inbound),
                ));
                state
            });
            let sidebar = AccountsResource::for_target(state.clone(), Some("remote".into()), cx);
            let composer = AccountsResource::for_target(state, Some("remote".into()), cx);
            sidebar.update(cx, |resource, cx| resource.ensure_loaded(cx));
            composer.update(cx, |resource, cx| resource.ensure_loaded(cx));
            sidebar
        });
        for (index, forced) in [false, true, false].into_iter().enumerate() {
            cx.run_until_parked();
            let request: zeron_rpc::ClientFrame =
                serde_json::from_str(&requests.try_recv().expect("one shared list request"))
                    .unwrap();
            assert_eq!(
                request.method.as_deref(),
                Some(methods::LIST_AGENT_ACCOUNTS)
            );
            assert_eq!(
                request.params,
                serde_json::json!({"targetDeviceId":"remote", "forceUsage":forced})
            );
            assert!(
                requests.try_recv().is_err(),
                "views must share the in-flight list"
            );
            if index == 1 {
                // A completed account action needs a new list even if a usage
                // probe was already running. Background observers only join it.
                resource.update(cx, |resource, cx| {
                    resource.refresh(Refresh::Accounts, cx);
                    resource.refresh(Refresh::Background, cx);
                });
            }
            let reply = zeron_rpc::ServerFrame {
                id: request.id,
                ok: Some(serde_json::to_value(snapshot(&format!("reply-{index}"))).unwrap()),
                ..Default::default()
            };
            runtime.block_on(async {
                replies
                    .send(serde_json::to_string(&reply).unwrap())
                    .await
                    .unwrap();
                while replies.capacity() < replies.max_capacity() {
                    tokio::task::yield_now().await;
                }
            });
            cx.run_until_parked();
            cx.update(|cx| {
                assert_eq!(
                    cx.global::<AccountsSnapshotCache>().0[&Some("remote".into())].warnings[0]
                        .message,
                    format!("reply-{index}")
                );
            });
        }
        resource.update(cx, |resource, cx| resource.refresh(Refresh::Background, cx));
        cx.run_until_parked();
        assert!(
            requests.try_recv().is_err(),
            "the shared probe throttle applies across views"
        );
        cx.update(|cx| assert!(!resource.read(cx).refreshing));
    }
}
