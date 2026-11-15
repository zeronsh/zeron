//! Live probe: OpenCode 2.x's sign-ins as Zeron's Providers page handles
//! them. Lists them (never a secret), then — with `--round-trip <integration>`
//! — stores a dummy key credential for that integration, finds it, and
//! removes it again. Pick an integration you don't use: storing makes it
//! that integration's active credential until it is removed.
//!
//!     cargo run -p zeron-harness --example opencode_credential_probe -- [--round-trip 302ai]

use zeron_harness::OpencodeHarness;

#[tokio::main]
async fn main() {
    let harness = OpencodeHarness::new();
    let list = harness.native_credentials().await.expect("credentials");
    for credential in &list {
        println!(
            "{} ({}) label={:?} kind={} active={}",
            credential.integration,
            credential.integration_name.as_deref().unwrap_or("?"),
            credential.label,
            credential.kind,
            credential.active
        );
    }
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() != Some("--round-trip") {
        return;
    }
    let integration = args.next().expect("an integration id");
    harness
        .add_native_credential(
            &integration,
            "zeron probe",
            serde_json::json!({ "type": "key", "key": "zeron-probe-not-a-key" }),
        )
        .await
        .expect("stored");
    let stored = harness
        .native_credentials()
        .await
        .expect("credentials")
        .into_iter()
        .find(|c| c.integration == integration && c.label.as_deref() == Some("zeron probe"))
        .expect("the stored credential is listed");
    println!("stored: {} active={}", stored.id, stored.active);
    harness
        .activate_native_credential(&stored.id)
        .await
        .expect("activated");
    harness
        .remove_native_credential(&stored.id)
        .await
        .expect("removed");
    let left = harness
        .native_credentials()
        .await
        .expect("credentials")
        .into_iter()
        .any(|c| c.id == stored.id);
    println!("removed: {}", !left);
}
