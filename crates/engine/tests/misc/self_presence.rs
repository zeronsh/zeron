//! 2026-11-11 03:55, work-laptop: every socket reset at once. The registry
//! room rejoined and the heartbeat kept going, but the Mac's own
//! `lastSeenAt` (what list_devices and the app show for this device) froze
//! at the rejoin instant: the room never echoes a device's beat back to the
//! socket that sent it, so the self entry only ever came from a hello. The
//! mock room now behaves as the DO does (no echo), and can drop every
//! socket at once.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use zeron_doc::RegistryDoc;
use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_proto::HarnessId;
use zeron_sync::registry::mock_server::MockRegistryServer;
use zeron_sync::{RegistryClient, RegistryEvent, RegistryTuning};

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

fn doc(device: &str) -> Arc<Mutex<RegistryDoc>> {
    Arc::new(Mutex::new(RegistryDoc::new(device)))
}

fn assemble(dir: &std::path::Path, device_id: &str) -> EngineCore {
    std::fs::create_dir_all(dir).expect("data dir");
    std::fs::write(dir.join("device-id"), device_id).expect("device id");
    EngineCore::assemble(dir, Arc::new(HarnessRegistry::new()), HarnessId::Mock, None)
        .expect("engine assembles")
}

/// The client: after every socket drops, peers see a fresh beat without the
/// host's next tick, and the sender's own view of its beat moves too.
#[tokio::test]
async fn presence_resumes_after_every_socket_drops_at_once() {
    let server = MockRegistryServer::start().await;
    let a = RegistryClient::connect(&server.url(), doc("dev-a"), "dev-a")
        .await
        .expect("a connects");
    let b = RegistryClient::connect(&server.url(), doc("dev-b"), "dev-b")
        .await
        .expect("b connects");
    a.set_presence(1_000);
    wait_for(|| b.presence().get("dev-a") == Some(&1_000), "b to hear a").await;
    wait_for(|| a.own_presence() == Some(1_000), "a's own beat").await;
    // The room never sends a device its own beat.
    assert_eq!(a.presence().get("dev-a"), None);

    let before = now_ms();
    let mut events = a.events();
    server.drop_all_connections();
    tokio::time::timeout(Duration::from_secs(10), async {
        while !matches!(events.recv().await, Ok(RegistryEvent::Disconnected)) {}
    })
    .await
    .expect("a sees the drop");
    wait_for(
        || server.presence_of("dev-a").is_some_and(|at| at >= before),
        "the room to hold a fresh beat from a",
    )
    .await;
    wait_for(
        || b.presence().get("dev-a").is_some_and(|at| *at >= before),
        "b to hear a again",
    )
    .await;
    wait_for(
        || a.own_presence().is_some_and(|at| at >= before),
        "a's own beat to move past the drop",
    )
    .await;

    // The steady heartbeat keeps moving the self view afterwards.
    let later = now_ms() + 60_000;
    a.set_presence(later);
    wait_for(|| a.own_presence() == Some(later), "a's next beat").await;
    wait_for(|| b.presence().get("dev-a") == Some(&later), "b to hear it").await;
    a.shutdown().await;
    b.shutdown().await;
}

async fn rejoined(events: &mut tokio::sync::broadcast::Receiver<RegistryEvent>) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !matches!(events.recv().await, Ok(RegistryEvent::Disconnected)) {}
        while !matches!(events.recv().await, Ok(RegistryEvent::Connected)) {}
    })
    .await
    .expect("dropped and rejoined");
}

/// The phone stops beating in the background on purpose (each beat wakes
/// the radio). A rejoin after a network change must not beat for it and
/// show it online; a host still beating does beat at once on its rejoin.
#[tokio::test]
async fn a_client_that_stopped_beating_stays_quiet_after_a_rejoin() {
    let server = MockRegistryServer::start().await;
    let url = Arc::new(zeron_sync::StaticUrl(server.url()));
    let short = RegistryTuning {
        rejoin_beat_window: Duration::from_millis(300),
        ..RegistryTuning::default()
    };
    let phone = RegistryClient::connect_via_tuned(url.clone(), doc("dev-phone"), "dev-phone", short)
        .await
        .expect("phone connects");
    let laptop = RegistryClient::connect_via_tuned(url, doc("dev-laptop"), "dev-laptop", RegistryTuning::default())
        .await
        .expect("laptop connects");
    phone.set_presence(1_000);
    laptop.set_presence(2_000);
    wait_for(
        || server.presence_of("dev-phone") == Some(1_000) && server.presence_of("dev-laptop") == Some(2_000),
        "both first beats",
    )
    .await;
    // The phone goes to the background: no beat for longer than its window.
    tokio::time::sleep(Duration::from_millis(600)).await;

    let before = now_ms();
    let (mut phone_events, mut laptop_events) = (phone.events(), laptop.events());
    server.drop_all_connections();
    rejoined(&mut phone_events).await;
    rejoined(&mut laptop_events).await;
    wait_for(
        || server.presence_of("dev-laptop").is_some_and(|at| at >= before),
        "the laptop's rejoin beat",
    )
    .await;
    // Time for a rejoin beat from the phone, were one coming.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(server.presence_of("dev-phone"), Some(1_000), "the backgrounded phone stays quiet");
    assert_eq!(phone.own_presence(), Some(1_000));
    phone.shutdown().await;
    laptop.shutdown().await;
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
