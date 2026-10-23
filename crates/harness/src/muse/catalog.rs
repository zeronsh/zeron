//! `model/list` rows → Zeron models. MSP rows carry their selectable
//! reasoning tiers in `variants`; `unknown` means the catalog declared no
//! tier list, so the harness-level ladder applies.

use serde_json::Value;
use zeron_proto::{Model, ReasoningLevel};

pub(super) fn parse_models(result: &Value) -> Vec<Model> {
    let Some(rows) = result.get("models").and_then(Value::as_array) else {
        return Vec::new();
    };
    rows.iter()
        .filter_map(|row| {
            let id = row.get("modelId").and_then(Value::as_str)?.to_owned();
            if id.is_empty() {
                return None;
            }
            let label = row
                .get("displayLabel")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .unwrap_or(&id)
                .to_owned();
            let mut description = row
                .get("description")
                .and_then(Value::as_str)
                .map(str::to_owned);
            if description.is_none()
                && let Some(limit) = row.get("contextLimit").and_then(Value::as_u64)
            {
                description = Some(format!("{}k context", (limit + 500) / 1000));
            }
            Some(Model {
                id,
                label,
                description,
                reasoning_levels: reasoning_levels(row),
                options: Vec::new(),
            })
        })
        .collect()
}

fn reasoning_levels(row: &Value) -> Vec<ReasoningLevel> {
    // `variants` is either the string "unknown" or an object holding the
    // ordered tier list; tolerate an array spelling too.
    let tiers: Vec<&str> = match row.get("variants") {
        Some(Value::Array(list)) => list.iter().filter_map(Value::as_str).collect(),
        Some(Value::Object(map)) => ["tiers", "levels", "variants"]
            .iter()
            .find_map(|key| map.get(*key).and_then(Value::as_array))
            .map(|list| list.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    tiers.iter().filter_map(|tier| parse_level(tier)).collect()
}

pub(super) fn parse_level(tier: &str) -> Option<ReasoningLevel> {
    Some(match tier {
        "none" | "minimal" => ReasoningLevel::Minimal,
        "low" => ReasoningLevel::Low,
        "medium" => ReasoningLevel::Medium,
        "high" => ReasoningLevel::High,
        "xhigh" => ReasoningLevel::XHigh,
        "max" => ReasoningLevel::Max,
        "ultra" => ReasoningLevel::Ultra,
        _ => return None,
    })
}

/// The MSP `reasoningEffort` spelling for a Zeron level.
pub(super) fn effort_arg(level: ReasoningLevel) -> &'static str {
    match level {
        ReasoningLevel::Minimal => "minimal",
        ReasoningLevel::Low => "low",
        ReasoningLevel::Medium => "medium",
        ReasoningLevel::High => "high",
        ReasoningLevel::XHigh => "xhigh",
        ReasoningLevel::Max => "max",
        ReasoningLevel::Ultra | ReasoningLevel::Ultracode => "ultra",
        ReasoningLevel::Ultrathink => "high",
    }
}

pub(super) fn fallback_models() -> Vec<Model> {
    vec![Model {
        id: "default".into(),
        label: "Muse default".into(),
        description: Some("Model configured in Muse".into()),
        reasoning_levels: vec![
            ReasoningLevel::Low,
            ReasoningLevel::Medium,
            ReasoningLevel::High,
            ReasoningLevel::XHigh,
            ReasoningLevel::Max,
            ReasoningLevel::Ultra,
        ],
        options: vec![],
    }]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn live_rows_parse_with_context_description() {
        let models = parse_models(&json!({"models": [
            {"modelId": "muse-wonder-1.0", "displayLabel": "Wonder", "description": null, "contextLimit": 1007997, "variants": "unknown"},
            {"modelId": "", "displayLabel": "skip"},
        ]}));
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].label, "Wonder");
        assert_eq!(models[0].description.as_deref(), Some("1008k context"));
        assert!(models[0].reasoning_levels.is_empty());
    }
}
