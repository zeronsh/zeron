//! `model/list` → [`Model`]s, and the reasoning tiers both ways.
use serde_json::Value;
use zeron_proto::{Model, ReasoningLevel};

use super::normalize::string;

/// Muse's tier for a Zeron reasoning level (its closed vocabulary also has
/// `none`, which no Zeron level selects). Levels Muse has no tier for
/// (`ultracode`, `ultrathink`) leave the model's default.
pub(super) fn effort(level: Option<ReasoningLevel>) -> Option<&'static str> {
    Some(match level? {
        ReasoningLevel::Minimal => "minimal",
        ReasoningLevel::Low => "low",
        ReasoningLevel::Medium => "medium",
        ReasoningLevel::High => "high",
        ReasoningLevel::XHigh => "xhigh",
        ReasoningLevel::Max => "max",
        ReasoningLevel::Ultra => "ultra",
        ReasoningLevel::Ultracode | ReasoningLevel::Ultrathink => return None,
    })
}

fn level(tier: &str) -> Option<ReasoningLevel> {
    Some(match tier {
        "minimal" => ReasoningLevel::Minimal,
        "low" => ReasoningLevel::Low,
        "medium" => ReasoningLevel::Medium,
        "high" => ReasoningLevel::High,
        "xhigh" => ReasoningLevel::XHigh,
        "max" => ReasoningLevel::Max,
        "ultra" => ReasoningLevel::Ultra,
        _ => return None,
    })
}

/// The tiers older hosts imply when a row reports none (`"unknown"`).
const FALLBACK_TIERS: &[&str] = &["low", "medium", "high", "xhigh", "max"];

/// Meta's rows from a `model/list` result, the catalog default first.
pub(super) fn models(result: &Value) -> Vec<Model> {
    let mut seen = std::collections::HashSet::new();
    let mut models: Vec<(bool, Model)> = result["models"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|row| matches!(row["providerId"].as_str(), None | Some("meta")))
        .filter_map(|row| {
            let id = string(row, "modelId").trim();
            if id.is_empty() || !seen.insert(id.to_string()) {
                return None;
            }
            let label = string(row, "displayLabel").trim();
            let label = if label.is_empty() || (label == id && id.starts_with("muse-")) {
                pretty(id)
            } else {
                label.to_string()
            };
            let tiers: Vec<&str> = match row["variants"].as_array() {
                Some(tiers) => tiers.iter().filter_map(Value::as_str).collect(),
                None => FALLBACK_TIERS.to_vec(),
            };
            Some((
                row["isDefault"] == true,
                Model {
                    id: id.into(),
                    label,
                    description: row["description"]
                        .as_str()
                        .filter(|d| !d.trim().is_empty())
                        .map(str::to_owned),
                    reasoning_levels: tiers.into_iter().filter_map(level).collect(),
                    options: vec![],
                },
            ))
        })
        .collect();
    // Stable: the default first, the host's order (newest first) after it.
    models.sort_by_key(|(default, _)| !*default);
    models.into_iter().map(|(_, model)| model).collect()
}

/// `muse-spark-1.2` → `Muse Spark 1.2`.
fn pretty(id: &str) -> String {
    id.split('-')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) if first.is_ascii_lowercase() => {
                    first.to_ascii_uppercase().to_string() + chars.as_str()
                }
                _ => word.to_string(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn meta_rows_map_with_their_tiers_default_first() {
        let models = models(&json!({"providerId":"meta","models":[
            {"modelId":"muse-spark-1.3","displayLabel":"muse-spark-1.3","providerId":"meta","isDefault":false,
             "variants":["low","medium","high"],"description":null},
            {"modelId":"muse-spark-1.2","displayLabel":"Muse Spark","providerId":"meta","isDefault":true,
             "variants":"unknown","description":"Fast"},
            {"modelId":"other","displayLabel":"Other","providerId":"elsewhere","isDefault":false},
            {"modelId":"muse-spark-1.3","displayLabel":"dup","providerId":"meta","isDefault":false}
        ]}));
        assert_eq!(
            models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["muse-spark-1.2", "muse-spark-1.3"]
        );
        assert_eq!(models[0].description.as_deref(), Some("Fast"));
        // `"unknown"` variants fall back to the common ladder.
        assert_eq!(models[0].reasoning_levels.len(), 5);
        assert_eq!(models[1].label, "Muse Spark 1.3");
        assert_eq!(
            models[1].reasoning_levels,
            [
                ReasoningLevel::Low,
                ReasoningLevel::Medium,
                ReasoningLevel::High
            ]
        );
    }

    #[test]
    fn levels_round_trip_to_tiers() {
        for tier in ["minimal", "low", "medium", "high", "xhigh", "max", "ultra"] {
            assert_eq!(effort(level(tier)), Some(tier));
        }
        assert_eq!(effort(Some(ReasoningLevel::Ultrathink)), None);
        assert_eq!(effort(None), None);
    }
}
