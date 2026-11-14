//! Preserve Factory's catalog ids and effort ladders, including router/fast rows.
use super::normalize::string;
use serde_json::{Value, json};
use std::collections::HashSet;
use zeron_proto::{Model, ReasoningLevel};

/// The effort a title runs `model` at: the one asked for where its ladder
/// has it, else the ladder's lowest. A model with no known ladder keeps the
/// session's `effective` effort.
pub(super) fn title_effort(
    model: &str,
    requested: Option<ReasoningLevel>,
    effective: &Value,
    models: &[Model],
) -> Value {
    let levels = models
        .iter()
        .find(|m| m.id == model)
        .map(|m| m.reasoning_levels.as_slice())
        .unwrap_or_default();
    requested
        .filter(|level| levels.contains(level))
        .or_else(|| levels.iter().min().copied())
        .map_or_else(|| effective.clone(), |level| json!(level))
}

pub(super) fn models(result: &Value) -> Vec<Model> {
    let mut seen = HashSet::new();
    result["models"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| {
            let id = string(row, "id");
            if id.is_empty() || row["disabled"] == true || !seen.insert(id.to_owned()) {
                return None;
            }
            let label = row["displayName"]
                .as_str()
                .filter(|s| !s.is_empty())
                .unwrap_or(id);
            let mut levels = vec![];
            for level in row["supportedReasoningEfforts"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                let level = match level {
                    "minimal" => ReasoningLevel::Minimal,
                    "low" => ReasoningLevel::Low,
                    "medium" => ReasoningLevel::Medium,
                    "high" => ReasoningLevel::High,
                    "xhigh" => ReasoningLevel::XHigh,
                    "max" => ReasoningLevel::Max,
                    _ => continue,
                };
                if !levels.contains(&level) {
                    levels.push(level);
                }
            }
            Some(Model {
                id: id.into(),
                label: label.into(),
                description: row["description"].as_str().map(str::to_owned),
                reasoning_levels: levels,
                options: vec![],
                provider: None,
                default: false,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn droid_title_effort_uses_cached_ladder_or_effective_effort() {
        use ReasoningLevel::{High, Low, Medium, Minimal};
        let model = |id: &str, levels: &[ReasoningLevel]| Model {
            id: id.into(),
            label: String::new(),
            description: None,
            reasoning_levels: levels.to_vec(),
            options: vec![],
            provider: None,
            default: false,
        };
        let models = [model("ladder", &[High, Medium, Low]), model("empty", &[])];
        for (model, requested, expected) in [
            ("ladder", Some(Medium), "medium"),
            ("ladder", Some(Minimal), "low"),
            ("ladder", None, "low"),
            ("empty", Some(Minimal), "high"),
            ("unknown", Some(Minimal), "high"),
        ] {
            assert_eq!(
                title_effort(model, requested, &json!("high"), &models),
                json!(expected)
            );
        }
    }
    #[test]
    fn droid_catalog_keeps_native_ids_and_drops_none_and_disabled() {
        let models = models(&serde_json::json!({"models":[
            {"id":"auto","displayName":"Auto Model","supportedReasoningEfforts":["none"]},
            {"id":"native-fast","supportedReasoningEfforts":["off","minimal","low","medium","high","xhigh","max","dynamic"]},
            {"id":"disabled","disabled":true},
            {"id":"native-fast"}
        ]}));
        assert_eq!(
            models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["auto", "native-fast"]
        );
        assert!(models[0].reasoning_levels.is_empty());
        assert_eq!(
            models[1].reasoning_levels,
            [
                ReasoningLevel::Minimal,
                ReasoningLevel::Low,
                ReasoningLevel::Medium,
                ReasoningLevel::High,
                ReasoningLevel::XHigh,
                ReasoningLevel::Max
            ]
        );
    }
}
