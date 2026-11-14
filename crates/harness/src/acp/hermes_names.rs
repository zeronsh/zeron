//! Hermes's models by the names its own catalog gives them.
//!
//! Hermes's ACP adapter labels a model with its id (`claude-opus-5-5`), but
//! keeps the models.dev catalog it reads context windows from beside its
//! config (`$HERMES_HOME/models_dev_cache.json`), and that names each model
//! as its maker does ("Claude Opus 5.5"). A model it doesn't list keeps its
//! id. Zeron only reads the file; Hermes refreshes it.
use serde::de::{DeserializeSeed, Deserializer, IgnoredAny, MapAccess, Visitor};
use std::{
    collections::{HashMap, HashSet},
    fmt,
    path::Path,
};
use zeron_proto::Model;

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

/// Hermes's large-context picker variants (`openai-codex:gpt-6-luna-900k`)
/// as a Context Window choice on their base model rather than rows of
/// their own. Hermes adds `-900k` after each Codex model that has one, a
/// Hermes-side name for the same model with the 900K window; the base runs
/// with Codex's 272K (`hermes_cli/codex_models.py`). A run that picks 900K
/// sends the variant's id (`pick_model_value`). A variant whose base isn't
/// listed stays its own row.
pub(super) fn fold_context_variants(models: &mut Vec<Model>) {
    const SUFFIX: &str = "-900k";
    let ids: HashSet<String> = models.iter().map(|m| m.id.clone()).collect();
    let folded: HashSet<String> = ids
        .iter()
        .filter(|id| {
            id.strip_suffix(SUFFIX)
                .is_some_and(|base| ids.contains(base))
        })
        .cloned()
        .collect();
    if folded.is_empty() {
        return;
    }
    models.retain(|m| !folded.contains(&m.id));
    for model in models.iter_mut() {
        if folded.contains(&format!("{}{SUFFIX}", model.id))
            && !model.options.iter().any(|o| o.id == "contextWindow")
        {
            model.options.push(zeron_proto::ModelOption {
                id: "contextWindow".into(),
                label: "Context Window".into(),
                choices: vec![
                    zeron_proto::ModelOptionChoice {
                        id: "272k".into(),
                        label: "272K".into(),
                    },
                    zeron_proto::ModelOptionChoice {
                        id: "900k".into(),
                        label: "900K".into(),
                    },
                ],
                default_choice: "272k".into(),
            });
        }
    }
}

/// Renames each Hermes model whose label is still its bare id (the provider
/// already split off) after the catalog in Hermes's home, if there is one.
/// Blocking file work: call off the async runtime.
pub(super) fn name_models(models: &mut [Model]) {
    let home = crate::model_context::root(
        "HERMES_HOME",
        crate::executable::home_or_current_dir().join(".hermes"),
    );
    name_models_from(models, &home.join("models_dev_cache.json"));
}

/// The most of Hermes's catalog read (it's a few MB).
const MAX_CATALOG_BYTES: u64 = 64 << 20;

/// Catalog key → name, for the models asked about, by catalog provider.
type Names = HashMap<String, HashMap<String, String>>;

fn name_models_from(models: &mut [Model], catalog: &Path) {
    // The keys each model may be listed under: its id, and the id with
    // dots as dashes (Hermes writes `claude-fable-5.1`; the catalog keys
    // `claude-fable-5-1`).
    let mut wanted: HashMap<String, HashSet<String>> = HashMap::new();
    for (provider, id) in models.iter().filter_map(named_by_id) {
        let keys = wanted
            .entry(catalog_provider(provider).to_owned())
            .or_default();
        keys.insert(id.replace('.', "-"));
        keys.insert(id.to_owned());
    }
    if wanted.is_empty() {
        return;
    }
    // Streamed through a small buffer and kept only where asked for: the
    // file is several MB of every provider's models, and holding it (or a
    // map of it) would cost far more than the few names wanted.
    // Only a regular file of a sane size: a FIFO (or a device) there would
    // block the read forever, a runaway file would be parsed for minutes.
    let Ok(meta) = std::fs::metadata(catalog) else {
        return;
    };
    if !meta.is_file() || meta.len() > MAX_CATALOG_BYTES {
        tracing::debug!(
            "Hermes's model catalog isn't a file of a usable size; models keep their ids"
        );
        return;
    }
    let Ok(file) = std::fs::File::open(catalog) else {
        return;
    };
    let mut reader =
        serde_json::Deserializer::from_reader(std::io::BufReader::with_capacity(64 * 1024, file));
    let names = match (Catalog { wanted: &wanted }).deserialize(&mut reader) {
        Ok(names) => names,
        Err(error) => {
            tracing::debug!(%error, "Hermes's model catalog didn't parse; models keep their ids");
            return;
        }
    };
    for model in models.iter_mut() {
        let Some((provider, id)) = named_by_id(model) else {
            continue;
        };
        let Some(listed) = names.get(catalog_provider(provider)) else {
            continue;
        };
        let name = listed
            .get(id)
            .or_else(|| listed.get(&id.replace('.', "-")))
            .cloned();
        if let Some(name) = name {
            model.label = name;
        }
    }
}

/// The catalog's top level: provider id → provider, reading only the
/// providers asked for and skipping the rest without keeping anything.
struct Catalog<'a> {
    wanted: &'a HashMap<String, HashSet<String>>,
}

impl<'de> DeserializeSeed<'de> for Catalog<'_> {
    type Value = Names;
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Names, D::Error> {
        d.deserialize_map(self)
    }
}

impl<'de> Visitor<'de> for Catalog<'_> {
    type Value = Names;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a models.dev catalog")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Names, A::Error> {
        let mut names = Names::new();
        while let Some(provider) = map.next_key_seed(pick(|k| self.wanted.contains_key(k)))? {
            match provider.and_then(|p| Some((self.wanted.get(&p)?, p))) {
                Some((keys, provider)) => {
                    let found = map.next_value_seed(Provider { keys })?;
                    if !found.is_empty() {
                        names.insert(provider, found);
                    }
                }
                None => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(names)
    }
}

/// A map key, kept (allocated) only if `.0` wants it: the catalog's
/// thousands of other keys are only compared, in the parser's own buffer.
struct Pick<F>(F);

/// (A function, so a closure's `&str` is taken as any lifetime.)
fn pick<F: Fn(&str) -> bool>(wants: F) -> Pick<F> {
    Pick(wants)
}

impl<'de, F: Fn(&str) -> bool> DeserializeSeed<'de> for Pick<F> {
    type Value = Option<String>;
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Option<String>, D::Error> {
        d.deserialize_str(self)
    }
}

impl<'de, F: Fn(&str) -> bool> Visitor<'de> for Pick<F> {
    type Value = Option<String>;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a key")
    }
    fn visit_str<E>(self, key: &str) -> Result<Option<String>, E> {
        Ok((self.0)(key).then(|| key.to_owned()))
    }
}

/// A provider: its `models`, the rest skipped.
struct Provider<'a> {
    keys: &'a HashSet<String>,
}

impl<'de> DeserializeSeed<'de> for Provider<'_> {
    type Value = HashMap<String, String>;
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        d.deserialize_map(self)
    }
}

impl<'de> Visitor<'de> for Provider<'_> {
    type Value = HashMap<String, String>;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a models.dev provider")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut found = HashMap::new();
        while let Some(field) = map.next_key_seed(pick(|k| k == "models"))? {
            if field.is_some() {
                found = map.next_value_seed(Models { keys: self.keys })?;
            } else {
                map.next_value::<IgnoredAny>()?;
            }
        }
        Ok(found)
    }
}

/// A provider's models: the name of each asked for, the rest skipped.
struct Models<'a> {
    keys: &'a HashSet<String>,
}

impl<'de> DeserializeSeed<'de> for Models<'_> {
    type Value = HashMap<String, String>;
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        d.deserialize_map(self)
    }
}

impl<'de> Visitor<'de> for Models<'_> {
    type Value = HashMap<String, String>;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a provider's models")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        #[derive(serde::Deserialize)]
        struct Entry {
            #[serde(default)]
            name: Option<String>,
        }
        let mut found = HashMap::new();
        while let Some(key) = map.next_key_seed(pick(|k| self.keys.contains(k)))? {
            if let Some(key) = key {
                let name = map
                    .next_value::<Entry>()?
                    .name
                    .map(|n| n.trim().to_owned())
                    .filter(|n| !n.is_empty());
                if let Some(name) = name {
                    found.insert(key, name);
                }
            } else {
                map.next_value::<IgnoredAny>()?;
            }
        }
        Ok(found)
    }
}

/// `(provider, model id)` of a model labelled by its id alone.
fn named_by_id(model: &Model) -> Option<(&str, &str)> {
    let provider = model.provider.as_ref()?;
    let (_, id) = model.id.split_once(':')?;
    (model.label == id).then_some((provider.id.as_str(), id))
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

    /// Hermes's `-900k` rows fold into their base's Context Window; one
    /// without a listed base stays.
    #[test]
    fn hermes_large_context_variants_are_a_context_window_choice() {
        let mut models = vec![
            model("openai-codex:gpt-6-luna", "gpt-6-luna", "openai-codex"),
            model(
                "openai-codex:gpt-6-luna-900k",
                "gpt-6-luna-900k",
                "openai-codex",
            ),
            model(
                "openai-codex:gpt-6.1-terra",
                "gpt-6.1-terra",
                "openai-codex",
            ),
            model("openai-codex:gpt-9-900k", "gpt-9-900k", "openai-codex"),
        ];
        fold_context_variants(&mut models);
        let ids: Vec<_> = models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "openai-codex:gpt-6-luna",
                "openai-codex:gpt-6.1-terra",
                "openai-codex:gpt-9-900k"
            ]
        );
        let window = models[0]
            .options
            .iter()
            .find(|o| o.id == "contextWindow")
            .unwrap();
        assert_eq!(window.default_choice, "272k");
        assert_eq!(
            window
                .choices
                .iter()
                .map(|c| c.label.as_str())
                .collect::<Vec<_>>(),
            ["272K", "900K"]
        );
        assert!(models[1].options.is_empty());
        // Picking 900K sends the variant; the default sends the model.
        let available = ["openai-codex:gpt-6-luna", "openai-codex:gpt-6-luna-900k"];
        assert_eq!(
            super::super::pick_model_value("openai-codex:gpt-6-luna", &available, Some("900k"))
                .as_deref(),
            Some("openai-codex:gpt-6-luna-900k")
        );
        assert_eq!(
            super::super::pick_model_value("openai-codex:gpt-6-luna", &available, Some("272k"))
                .as_deref(),
            Some("openai-codex:gpt-6-luna")
        );
        // A chat that saved the variant itself still runs it.
        assert_eq!(
            super::super::pick_model_value("openai-codex:gpt-6-luna-900k", &available, None)
                .as_deref(),
            Some("openai-codex:gpt-6-luna-900k")
        );
    }

    /// A catalog that isn't a regular file of a usable size is passed over
    /// at once: a FIFO would block a read forever.
    #[cfg(unix)]
    #[test]
    fn a_catalog_that_isnt_a_plain_file_of_a_usable_size_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("models_dev_cache.json");
        let made = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap();
        assert!(made.success());
        let mut models = vec![model(
            "anthropic:claude-opus-5-5",
            "claude-opus-5-5",
            "anthropic",
        )];
        let started = std::time::Instant::now();
        name_models_from(&mut models, &fifo);
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        assert_eq!(models[0].label, "claude-opus-5-5");
        // Past the size cap (sparse: no disk spent): skipped unread.
        let big = dir.path().join("big.json");
        let file = std::fs::File::create(&big).unwrap();
        file.set_len(MAX_CATALOG_BYTES + 1).unwrap();
        name_models_from(&mut models, &big);
        assert_eq!(models[0].label, "claude-opus-5-5");
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
