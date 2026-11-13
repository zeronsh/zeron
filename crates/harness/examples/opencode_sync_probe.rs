//! Live probe, read-only: what the engine's OpenCode sync sees on the
//! machine's background service — every top-level session, the newest one's
//! history as agent events, then the change feed for a few seconds. Prints
//! counts and kinds, never message text.
//!
//!     cargo run -p zeron-harness --example opencode_sync_probe -- [seconds]

use zeron_harness::{NativeMessageKind, OpencodeHarness};

#[tokio::main]
async fn main() {
    let seconds: u64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(5);
    let harness = OpencodeHarness::new();
    let sessions = harness.native_sessions().await.expect("sessions");
    let archived = sessions.iter().filter(|s| s.archived).count();
    let busy = sessions.iter().filter(|s| s.busy).count();
    let directories: std::collections::BTreeSet<_> =
        sessions.iter().map(|s| s.directory.as_str()).collect();
    println!(
        "sessions: {} ({archived} archived, {busy} busy) in {} directories",
        sessions.len(),
        directories.len()
    );
    if let Some(newest) = sessions.iter().max_by_key(|s| s.updated_at_ms) {
        let history = harness.native_history(newest).await.expect("history");
        let (mut users, mut events) = (0, 0);
        for message in &history {
            match &message.kind {
                NativeMessageKind::User { .. } => users += 1,
                NativeMessageKind::Assistant { events: e } => events += e.len(),
            }
        }
        println!(
            "newest session: {} messages ({users} prompts, {events} agent events)",
            history.len()
        );
    }
    let mut feed = harness.follow_native().await.expect("feed");
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(seconds);
    while let Ok(Some(change)) = tokio::time::timeout_at(deadline, feed.recv()).await {
        println!("change: {change:?}");
    }
}
