//! Hermes's models by the names its own catalog gives them.
//!
//! Hermes's ACP adapter labels a model with its id (`claude-opus-5-5`), but
//! keeps the models.dev catalog it reads context windows from beside its
//! config (`$HERMES_HOME/models_dev_cache.json`), and that names each model
//! as its maker does ("Claude Opus 5.5"). A model it doesn't list keeps its
//! id. Zeron only reads the file; Hermes refreshes it.
use serde::Deserialize;
use std::{collections::HashMap, path::Path};
use zeron_proto::Model;

#[derive(Deserialize)]
struct Provider {
    #[serde(default)]
    models: HashMap<String, Entry>,
}

#[derive(Deserialize)]
struct Entry {
    #[serde(default)]
    name: Option<String>,
}

/// The models.dev provider a Hermes provider's models are listed under, as
/// Hermes maps them (`agent/models_dev.py`); any other is its own id.
fn catalog_provider(hermes: &str) -> &str {
    match hermes {
        "openai-codex" | "openai-api" => "openai",
        "novita" => "novita-ai",
        "kimi" | "kimi-coding" | "kimi-coding-cn" | "moonshot" => "kimi-for-coding",
        "minimax-oauth" => "minimax",
        "qwen-oauth" => "alibaba",
        "ai-gateway" => "vercel",
        "opencode-zen" => "opencode",
        "kilocode" => "kilo",
        "fireworks" => "fireworks-ai",
        "gemini" => "google",
        "xai-oauth" => "xai",
        "meta-ai" => "meta",
        other => other,
    }
}

/// Renames each Hermes model whose label is still its bare id (the provider
/// already split off) after the catalog in Hermes's home, if there is one.
pub(super) fn name_models(models: &mut [Model]) {
    let home = crate::model_context::root(
        "HERMES_HOME",
        crate::executable::home_or_current_dir().join(".hermes"),
    );
    name_models_from(models, &home.join("models_dev_cache.json"));
}

fn name_models_from(models: &mut [Model], catalog: &Path) {
    // (Several MB of JSON: read only when some model needs a name.)
    if !models.iter().any(|m| named_by_id(m).is_some()) {
        return;
    }
    let Ok(bytes) = std::fs::read(catalog) else {
        return;
    };
    let Ok(providers) = serde_json::from_slice::<HashMap<String, Provider>>(&bytes) else {
        tracing::debug!("Hermes's model catalog didn't parse; models keep their ids");
        return;
    };
    for model in models {
        let Some((provider, id)) = named_by_id(model) else {
            continue;
        };
        let Some(listed) = providers.get(catalog_provider(&provider)) else {
            continue;
        };
        // Hermes writes some versions with a dot (`claude-fable-5.1`) that
        // the catalog keys with a dash.
        let name = [id.clone(), id.replace('.', "-")]
            .iter()
            .find_map(|key| listed.models.get(key)?.name.clone())
            .map(|n| n.trim().to_owned())
            .filter(|n| !n.is_empty());
        if let Some(name) = name {
            model.label = name;
        }
    }
}

/// `(provider, model id)` of a model labelled by its id alone.
fn named_by_id(model: &Model) -> Option<(String, String)> {
    let provider = model.provider.as_ref()?;
    let (_, id) = model.id.split_once(':')?;
    (model.label == id).then(|| (provider.id.clone(), id.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(id: &str, label: &str, provider: &str) -> Model {
        Model {
            id: id.into(),
            label: label.into(),
            provider: Some(zeron_proto::ModelProvider {
                id: provider.into(),
                name: provider.into(),
            }),
            description: None,
            reasoning_levels: vec![],
            options: vec![],
            default: false,
        }
    }

    /// Hermes's own catalog names its models; one it doesn't list keeps
    /// its id, and a label Hermes wrote itself is left as it is.
    #[test]
    fn hermes_models_read_as_its_catalog_names_them() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = dir.path().join("models_dev_cache.json");
        // The shape of a real models.dev cache, trimmed.
        std::fs::write(
            &catalog,
            r#"{
              "anthropic": {"id": "anthropic", "name": "Anthropic", "models": {
                "claude-opus-5-5": {"id": "claude-opus-5-5", "name": "Claude Opus 5.5", "reasoning": true},
                "claude-fable-5-1": {"id": "claude-fable-5-1", "name": "Claude Fable 5.1"}
              }},
              "openai": {"id": "openai", "models": {
                "gpt-6-luna": {"id": "gpt-6-luna", "name": "GPT-6 Luna"}
              }}
            }"#,
        )
        .unwrap();
        let mut models = vec![
            model("anthropic:claude-opus-5-5", "claude-opus-5-5", "anthropic"),
            model(
                "anthropic:claude-fable-5.1",
                "claude-fable-5.1",
                "anthropic",
            ),
            model("openai-codex:gpt-6-luna", "gpt-6-luna", "openai-codex"),
            model(
                "openai-codex:gpt-6-luna-900k",
                "gpt-6-luna-900k",
                "openai-codex",
            ),
            model("ollama:llama3", "llama3", "ollama"),
            model("anthropic:claude-opus-5-5", "My Opus", "anthropic"),
        ];
        name_models_from(&mut models, &catalog);
        let labels: Vec<_> = models.iter().map(|m| m.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Claude Opus 5.5",
                "Claude Fable 5.1",
                "GPT-6 Luna",
                "gpt-6-luna-900k",
                "llama3",
                "My Opus"
            ]
        );
        // No catalog, or one that doesn't parse: ids, and no error.
        let mut models = vec![model(
            "anthropic:claude-opus-5-5",
            "claude-opus-5-5",
            "anthropic",
        )];
        name_models_from(&mut models, &dir.path().join("missing.json"));
        std::fs::write(&catalog, "not json").unwrap();
        name_models_from(&mut models, &catalog);
        assert_eq!(models[0].label, "claude-opus-5-5");
    }
}
