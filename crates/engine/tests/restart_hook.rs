//! The version-change hook (`zeron_update::set_before_restart`) prepares
//! the live engine's store, and keeps no engine alive once it is shut down.
//! (A binary of its own: the hook is process-wide, the newest one wins.)

use zeron_engine::{Engine, EngineConfig, HarnessId};

#[tokio::test]
async fn the_restart_hook_reaches_the_live_engine_and_holds_none_after() {
    let dir = tempfile::tempdir().unwrap();
    let config = EngineConfig {
        data_dir: dir.path().to_path_buf(),
        edge_url: "http://127.0.0.1:1".into(),
        edge_token: None,
        ipc_port: 0,
        default_harness: HarnessId::Mock,
        org_id: None,
        workos_client_id: Some("client_test".into()),
    };
    let auth = Engine::build_auth(&config).await;
    let scope = Engine::initial_workspace_scope(&auth);
    let profile = Engine::resolve_profile(&config, &auth, scope)
        .unwrap()
        .expect("local profile is ready without auth");
    let runtime = Engine::assemble_runtime(&config, auth, profile)
        .await
        .unwrap();
    let store = runtime.core().doc_host.store().clone();
    let retired = runtime.core().doc_host.retirement_probe();

    assert!(!store.snapshot_deltas_stopped());
    zeron_update::run_before_restart();
    assert!(
        store.snapshot_deltas_stopped(),
        "the hook prepared the running engine's store"
    );

    runtime.shutdown().await;
    drop(runtime);
    for _ in 0..500 {
        if retired() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(retired(), "the hook (or anything else) kept the engine alive");
    // With the engine gone there is nothing to prepare, and nothing breaks.
    zeron_update::run_before_restart();
}
