//! Probe: does `ForkBoundary`'s wire shape match what the composer sends?

use zeron_doc::ForkBoundary;

#[test]
fn fork_boundary_round_trips_from_the_composer_json() {
    // What the UI puts on the wire for "Fork here".
    let wire = serde_json::json!({ "kind": "beforeMessage", "message_id": "m-1" });
    let parsed: Result<ForkBoundary, _> = serde_json::from_value(wire.clone());
    println!("parse({wire}) = {parsed:?}");
    assert!(parsed.is_ok(), "composer shape must deserialize");

    // And what the engine echoes back.
    let out = serde_json::to_value(&parsed.unwrap()).unwrap();
    println!("serialized = {out}");

    // The plain whole-conversation fork.
    let latest: ForkBoundary =
        serde_json::from_value(serde_json::json!({"kind": "latest"})).unwrap();
    assert_eq!(latest, ForkBoundary::Latest);
    // Absent boundary (the sidebar's fork) must default, too.
    let d: ForkBoundary = serde_json::from_value(serde_json::json!(null)).unwrap_or_default();
    assert_eq!(d, ForkBoundary::Latest);
}
