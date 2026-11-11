//! 2026-11-11 03:55, work-laptop: every socket reset at once. The registry
//! room rejoined and the heartbeat kept going, but the Mac's own
//! `lastSeenAt` (what list_devices and the app show for this device) froze
//! at the rejoin instant: the room never echoes a device's beat back to the
//! socket that sent it, so the self entry only ever came from a hello. The
//! mock room now behaves as the DO does (no echo), and can drop every
//! socket at once.

use std::sync::Arc;
use std::time::Duration;

use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_proto::HarnessId;
use zeron_sync::registry::mock_server::MockRegistryServer;

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

async fn wait_for(mut predicate: impl FnMut() -> bool, what: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !predicate() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn assemble(dir: &std::path::Path, device_id: &str) -> EngineCore {
    std::fs::create_dir_all(dir).expect("data dir");
    std::fs::write(dir.join("device-id"), device_id).expect("device id");
    EngineCore::assemble(dir, Arc::new(HarnessRegistry::new()), HarnessId::Mock, None)
        .expect("engine assembles")
}

/// The engine: its own device row's `lastSeenAt` moves past the drop, here
/// and on its peer.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn own_last_seen_moves_after_every_socket_drops() {
    let dir_a = tempfile::tempdir().unwrap();
    let dir_b = tempfile::tempdir().unwrap();
    let a = assemble(dir_a.path(), "dev-a");
    let b = assemble(dir_b.path(), "dev-b");
    let room = MockRegistryServer::start().await;
    a.workspace.connect_registry_url(&room.url());
    b.workspace.connect_registry_url(&room.url());
    let seen = |core: &EngineCore, id: &str| -> Option<i64> {
        core.workspace
            .watch_devices()
            .borrow()
            .iter()
            .find(|d| d.id == id)
            .and_then(|d| d.last_seen_at)
            .map(|at| at.timestamp_millis())
    };
    wait_for(
        || a.workspace.connected() && b.workspace.connected(),
        "both joined",
    )
    .await;
    wait_for(|| room.presence_of("dev-a").is_some(), "a's first beat").await;
    wait_for(|| seen(&b, "dev-a").is_some(), "b to know a's row").await;

    let before = now_ms();
    room.drop_all_connections();
    wait_for(
        || seen(&a, "dev-a").is_some_and(|at| at >= before),
        "a's own lastSeenAt to move past the drop",
    )
    .await;
    wait_for(
        || seen(&b, "dev-a").is_some_and(|at| at >= before),
        "b to see a's heartbeat again after the drop",
    )
    .await;
}
