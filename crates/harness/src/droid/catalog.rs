//! Preserve Factory's catalog ids and effort ladders, including router/fast rows.
use super::normalize::string;
use serde_json::Value;
use std::collections::HashSet;
use zeron_proto::{Model, ReasoningLevel};

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
