use serde_json::Value;
use zeron_proto::{Model, ReasoningLevel};

pub(super) fn models(value: &Value) -> Vec<Model> {
    value["models"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|row| row["disabled"] != true)
        .filter_map(|row| {
            let id = row["id"].as_str()?.to_owned();
            Some(Model {
                label: row["displayName"].as_str().unwrap_or(&id).to_owned(),
                id,
                description: None,
                reasoning_levels: row["supportedReasoningEfforts"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|effort| match effort.as_str()? {
                        "minimal" => Some(ReasoningLevel::Minimal),
                        "low" => Some(ReasoningLevel::Low),
                        "medium" => Some(ReasoningLevel::Medium),
                        "high" => Some(ReasoningLevel::High),
                        "xhigh" => Some(ReasoningLevel::XHigh),
                        "max" => Some(ReasoningLevel::Max),
                        _ => None,
                    })
                    .collect(),
                options: vec![],
            })
        })
        .collect()
}
