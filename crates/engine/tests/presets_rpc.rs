//! Agent presets over RPC: list (user, project, imported), upsert, delete.

mod support;

use std::sync::Arc;

use serde_json::{Value, json};
use support::*;
use zeron_rpc::methods;

#[tokio::test]
async fn presets_are_listed_per_project_saved_and_deleted() {
    let dir = tempfile::tempdir().unwrap();
    let handler: Handler = Arc::new(|_, _, _, _| {});
    let env = assemble(&dir.path().join("data"), Default::default(), handler);
    let client = zeron_rpc::memory_client(env.core.rpc_service());
    let call = |method: &'static str, params: Value| {
        let client = &client;
        async move { client.call_as::<Value>(method, params).await }
    };
    let ids = |value: &Value| -> Vec<String> {
        value["presets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| format!("{}:{}", p["id"].as_str().unwrap(), p["source"].as_str().unwrap()))
            .collect()
    };

    assert_eq!(call(methods::LIST_PRESETS, json!({})).await.unwrap()["presets"], json!([]));

    let saved = call(
        methods::UPSERT_PRESET,
        json!({ "preset": {
            "id": "", "name": "Fast fixer", "harness": "claude-code",
            "policy": { "mode": "auto" }, "instructions": "Keep diffs small."
        }}),
    )
    .await
    .unwrap();
    assert_eq!(saved["preset"]["id"], "fast-fixer");
    assert_eq!(saved["preset"]["policy"]["mode"], "auto");

    let nameless = call(
        methods::UPSERT_PRESET,
        json!({ "preset": { "id": "x", "name": "  ", "harness": "codex" } }),
    )
    .await;
    assert!(nameless.is_err(), "a preset needs a name");

    // A project's own presets appear when the project's folder is asked about.
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(project.path().join(".zeron/agents")).unwrap();
    std::fs::write(
        project.path().join(".zeron/agents/reviewer.md"),
        "---\nname: Reviewer\nharness: codex\nmode: ask\n---\nOnly report.\n",
    )
    .unwrap();
    let inside = call(
        methods::LIST_PRESETS,
        json!({ "path": project.path().display().to_string() }),
    )
    .await
    .unwrap();
    assert_eq!(ids(&inside), ["reviewer:project", "fast-fixer:user"]);
    let outside = call(methods::LIST_PRESETS, json!({})).await.unwrap();
    assert_eq!(ids(&outside), ["fast-fixer:user"]);

    let deleted = call(methods::DELETE_PRESET, json!({ "id": "fast-fixer" })).await.unwrap();
    assert_eq!(deleted["deleted"], true);
    let again = call(methods::DELETE_PRESET, json!({ "id": "fast-fixer" })).await.unwrap();
    assert_eq!(again["deleted"], false);
    assert_eq!(call(methods::LIST_PRESETS, json!({})).await.unwrap()["presets"], json!([]));
}
