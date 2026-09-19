//! Process-wide ownership of the engine and its application subscriptions.
//!
//! Windows are consumers. Only this owner bootstraps or shuts down the engine;
//! a daemon remains independent of the lifetime of the graphical application.

use gpui::{App, AppContext, Entity, Global};

use crate::state::{AppState, EngineBootConfig};

pub(crate) struct AppRuntime {
    pub state: Entity<AppState>,
    pub boot: EngineBootConfig,
}

impl Global for AppRuntime {}

pub(crate) fn init(boot: EngineBootConfig, cx: &mut App) -> Entity<AppState> {
    cx.set_global(crate::chat_store::ChatStore::default());
    let state = cx.new(|_| {
        let mut state = AppState::new();
        state.data_dir = Some(boot.data_dir.clone());
        state
    });
    cx.set_global(AppRuntime {
        state: state.clone(),
        boot,
    });
    crate::notification_service::init(state.clone(), cx);
    let owner = state.clone();
    cx.on_app_quit(move |cx| {
        crate::settings::flush(cx);
        let shutdown = owner
            .read(cx)
            .engine()
            .cloned()
            .map(|engine| gpui_tokio::Tokio::spawn(cx, async move { engine.shutdown().await }));
        async move {
            if let Some(task) = shutdown {
                let _ = task.await;
            }
        }
    })
    .detach();
    state
}

/// All retry and profile-transition paths resolve the same application owner.
/// Fixtures can still bootstrap an isolated state without installing globals.
pub(crate) fn owner_or(state: Entity<AppState>, cx: &App) -> Entity<AppState> {
    cx.try_global::<AppRuntime>()
        .map(|runtime| runtime.state.clone())
        .unwrap_or(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn consumers_do_not_replace_the_runtime_owner(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let owner = cx.new(|_| AppState::new());
            let consumer = cx.new(|_| AppState::new());
            cx.set_global(AppRuntime {
                state: owner.clone(),
                boot: EngineBootConfig {
                    data_dir: std::path::PathBuf::from("unused"),
                    ipc_port: 0,
                    edge_url: String::new(),
                    edge_token: None,
                    org_id: None,
                    workos_client_id: None,
                    default_harness: crate::HarnessId::Mock,
                },
            });
            assert_eq!(owner_or(consumer.clone(), cx), owner);
            drop(consumer);
            assert_eq!(cx.global::<AppRuntime>().state, owner);
        });
    }
}
