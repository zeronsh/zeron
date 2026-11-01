#![cfg(unix)]
use serde_json::json;
use std::os::unix::fs::PermissionsExt;
use zeron_harness::{ClaudeHarness, Harness};
use zeron_proto::ReasoningLevel;

fn fixture(response: serde_json::Value) -> (tempfile::TempDir, ClaudeHarness) {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("claude");
    std::fs::write(&script, include_str!("fixtures/fake-claude-models.py")).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(dir.path().join("response.json"), response.to_string()).unwrap();
    (dir, ClaudeHarness::new().with_executable(script))
}

#[tokio::test]
async fn initialize_is_shared_and_curated_metadata_survives_the_live_union() {
    let (dir, harness) = fixture(json!({"subtype":"success", "response":{
        "commands":[{"name":"review", "description":"Review"}],
        "models":[
            {"value":"default", "resolvedModel":"claude-opus-5-5[1m]", "displayName":"Default (recommended)", "supportedEffortLevels":["low","medium","high","xhigh","max"]},
            {"value":"opus[1m]", "resolvedModel":"claude-opus-5-5[1m]", "displayName":"Opus"},
            {"value":"fable[1m]", "resolvedModel":"claude-fable-5-1", "displayName":"Fable", "description":"CLI description", "supportedEffortLevels":["low"]},
            {"value":"gateway/new", "displayName":"Gateway model", "description":"Gateway description", "supportedEffortLevels":["low","xhigh","future","xhigh","max"]},
            {"value":"sonnet", "displayName":"Unresolved alias"}
        ]
    }}));
    let (catalog, commands) = tokio::join!(harness.model_catalog(true), harness.commands());
    let catalog = catalog.unwrap();
    assert_eq!(catalog.source, "live");
    assert_eq!(commands.unwrap()[0].name, "review");
    assert_eq!(catalog.models[0].id, "claude-opus-5-5[1m]");
    assert_eq!(catalog.models[1].id, "claude-opus-5-5");
    for curated in zeron_harness::claude::catalog::static_models() {
        assert_eq!(
            catalog.models.iter().find(|m| m.id == curated.id),
            Some(&curated)
        );
    }
    let new = catalog
        .models
        .iter()
        .find(|m| m.id == "gateway/new")
        .unwrap();
    assert_eq!(new.label, "Gateway model");
    assert_eq!(new.description.as_deref(), Some("Gateway description"));
    assert_eq!(
        new.reasoning_levels,
        [
            ReasoningLevel::Low,
            ReasoningLevel::XHigh,
            ReasoningLevel::Max,
            ReasoningLevel::Ultracode,
            ReasoningLevel::Ultrathink
        ]
    );
    // Explicit gateway settings from the environment remain selectable, but
    // initialize must not introduce or promote an unresolved alias.
    for alias in ["default", "opus[1m]", "fable[1m]", "sonnet"] {
        assert_eq!(
            catalog.models.iter().filter(|m| m.id == alias).count(),
            harness
                .fallback_models()
                .iter()
                .filter(|m| m.id == alias)
                .count()
        );
    }
    harness.commands().await.unwrap();
    harness.model_catalog(true).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("calls")).unwrap(),
        "initialize\n"
    );
}

#[tokio::test]
async fn failed_or_logged_out_initialize_falls_back_to_curated_catalog() {
    for error in ["transport failed", "not logged in: authentication required"] {
        let (_dir, harness) = fixture(json!({"subtype":"error","error":error}));
        assert_eq!(harness.models().await.unwrap(), harness.fallback_models());
        assert!(harness.model_catalog(false).await.is_err());
    }
}

#[tokio::test]
async fn default_uses_the_named_rows_live_model_name_in_either_order() {
    for aliases in [["default", "sonnet"], ["sonnet", "default"]] {
        let (_dir, harness) = fixture(json!({"subtype":"success", "response":{
            "models": aliases.map(|alias| json!({
                "value":alias, "resolvedModel":"claude-sonnet-5-5",
                "displayName":if alias == "default" { "Default (recommended)" } else { "Sonnet" },
                "description":if alias == "default" {
                    "Use the default model (currently Sonnet 5.5) · $2/$10 per Mtok"
                } else { "Sonnet 5.5 · Efficient for routine tasks · $2/$10 per Mtok" },
                "supportedEffortLevels":["low","medium","high","xhigh","max"]
            }))
        }}));
        let catalog = harness.model_catalog(true).await.unwrap();
        let sonnet = &catalog.models[0];
        assert_eq!(sonnet.id, "claude-sonnet-5-5");
        assert_eq!(sonnet.label, "Sonnet 5.5");
        assert_eq!(
            sonnet.description.as_deref(),
            Some("Sonnet 5.5 · Efficient for routine tasks · $2/$10 per Mtok")
        );
        assert_eq!(
            catalog.models.iter().filter(|m| m.id == sonnet.id).count(),
            1
        );
        assert!(sonnet.reasoning_levels.contains(&ReasoningLevel::XHigh));
        // Discovery supplies this name; the fallback must not know the new model.
        assert!(
            !zeron_harness::claude::catalog::static_models()
                .iter()
                .any(|m| m.id == sonnet.id)
        );
    }
}

#[tokio::test]
async fn live_names_do_not_depend_on_known_families_versions_or_id_formats() {
    for (id, name, description, expected) in [
        (
            "provider/opaque-deployment",
            "Nebula",
            "Nebula 17.3 · Provider description",
            "Nebula 17.3",
        ),
        (
            "gateway/custom",
            "Custom gateway model",
            "Gateway description",
            "Custom gateway model",
        ),
        (
            "opaque-model-without-prose",
            "Nebula",
            "Nebula 18.1",
            "Nebula 18.1",
        ),
        (
            "next-generation",
            "New Model Preview",
            "Provider description",
            "New Model Preview",
        ),
        (
            "claude-sonnet-6-2[1m]",
            "Sonnet (1M context)",
            "Sonnet 6.2 · Provider description",
            "Sonnet 6.2",
        ),
    ] {
        let (_dir, harness) = fixture(json!({"subtype":"success", "response":{
            "models":[
                {"value":"default", "resolvedModel":id, "displayName":"Default (recommended)"},
                {"value":"new-alias", "resolvedModel":id, "displayName":name, "description":description}
            ]
        }}));
        let catalog = harness.model_catalog(true).await.unwrap();
        assert_eq!(catalog.models[0].id, id);
        assert_eq!(catalog.models[0].label, expected);
        assert_eq!(catalog.models[0].description.as_deref(), Some(description));
    }
}

#[tokio::test]
async fn unnamed_default_uses_its_concrete_id_without_guessing_the_model_name() {
    let (_dir, harness) = fixture(json!({"subtype":"success", "response":{
        "models":[
            {"value":"default", "resolvedModel":"gateway/custom", "displayName":"Default (recommended)"},
            {"value":"gateway/other", "displayName":"Custom gateway model"}
        ]
    }}));
    let catalog = harness.model_catalog(true).await.unwrap();
    assert_eq!(catalog.models[0].id, "gateway/custom");
    assert_eq!(catalog.models[0].label, "gateway/custom");
    assert_eq!(catalog.models.last().unwrap().label, "Custom gateway model");
}
