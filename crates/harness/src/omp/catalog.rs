//! omp's model catalog, thinking levels and slash commands.
use super::normalize::string;
use serde_json::Value;
use zeron_proto::{
    Model, ModelOption, ModelOptionChoice, ReasoningLevel, RunRequest, SlashCommand,
};

/// The model option that turns thinking off (omp's `off` level), beside the
/// reasoning level that otherwise picks it.
pub(super) const THINKING_OPTION: &str = "omp_thinking";

/// omp's thinking levels, lowest first (`ThinkingLevel` without `inherit`).
const LEVELS: [&str; 7] = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];

/// `get_available_models` (and the session's current model, the default) as
/// Zeron's catalog. A model's id is omp's selection key `provider/id`.
pub(super) fn models(available: &Value, current: &Value) -> Vec<Model> {
    let default = format!("{}/{}", string(current, "provider"), string(current, "id"));
    available["models"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|m| !string(m, "provider").is_empty() && !string(m, "id").is_empty())
        .map(|m| {
            let id = format!("{}/{}", string(m, "provider"), string(m, "id"));
            let levels = reasoning_levels(m);
            Model {
                default: id == default,
                label: m["name"]
                    .as_str()
                    .filter(|n| !n.is_empty())
                    .unwrap_or(string(m, "id"))
                    .into(),
                description: Some(string(m, "provider").into()),
                options: if m["reasoning"] == true {
                    thinking_option()
                } else {
                    vec![]
                },
                reasoning_levels: levels,
                id,
            }
        })
        .collect()
}

/// A model's effort ladder: `thinking.efforts` (omp 18), else every level
/// for a reasoning model.
fn reasoning_levels(model: &Value) -> Vec<ReasoningLevel> {
    if model["reasoning"] != true {
        return vec![];
    }
    let efforts: Vec<&str> = match model["thinking"]["efforts"].as_array() {
        Some(efforts) => efforts.iter().filter_map(Value::as_str).collect(),
        None => LEVELS[1..].to_vec(),
    };
    efforts.into_iter().filter_map(level).collect()
}

fn level(name: &str) -> Option<ReasoningLevel> {
    Some(match name {
        "minimal" => ReasoningLevel::Minimal,
        "low" => ReasoningLevel::Low,
        "medium" => ReasoningLevel::Medium,
        "high" => ReasoningLevel::High,
        "xhigh" => ReasoningLevel::XHigh,
        "max" => ReasoningLevel::Max,
        _ => return None,
    })
}

fn thinking_option() -> Vec<ModelOption> {
    vec![ModelOption {
        id: THINKING_OPTION.into(),
        label: "Thinking".into(),
        default_choice: "auto".into(),
        choices: vec![
            ModelOptionChoice {
                id: "auto".into(),
                label: "Use reasoning level".into(),
            },
            ModelOptionChoice {
                id: "off".into(),
                label: "Off".into(),
            },
        ],
    }]
}

/// Whether `request` picks a thinking level (an effort, or thinking off).
pub(super) fn thinking_selected(request: &RunRequest) -> bool {
    request.reasoning.is_some() || thinking_off(request)
}

fn thinking_off(request: &RunRequest) -> bool {
    request
        .model_options
        .get(THINKING_OPTION)
        .is_some_and(|v| v == "off")
}

/// The level to set for `request` among the `supported` ones
/// (`get_available_thinking_levels`): the one asked for, else the nearest
/// effort below it (above, when none is below). Zeron's levels above omp's
/// top (ultra, ultrathink…) ask for omp's `max`.
pub(super) fn thinking_level(request: &RunRequest, supported: &Value) -> Option<&'static str> {
    let wanted = if thinking_off(request) {
        "off"
    } else {
        match request.reasoning? {
            ReasoningLevel::Minimal => "minimal",
            ReasoningLevel::Low => "low",
            ReasoningLevel::Medium => "medium",
            ReasoningLevel::High => "high",
            ReasoningLevel::XHigh => "xhigh",
            ReasoningLevel::Max
            | ReasoningLevel::Ultra
            | ReasoningLevel::Ultracode
            | ReasoningLevel::Ultrathink => "max",
        }
    };
    let supported: Vec<&str> = supported["levels"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    // Off is asked for exactly; a model that cannot turn it off keeps its own.
    if wanted == "off" {
        return supported.contains(&"off").then_some("off");
    }
    // Some thinking was asked for: never off, the nearest effort below, else
    // the lowest above.
    let rank = LEVELS.iter().position(|l| *l == wanted)?;
    LEVELS[1..=rank]
        .iter()
        .rev()
        .chain(LEVELS[rank + 1..].iter())
        .find(|l| supported.contains(l))
        .copied()
}

/// omp's command list (`available_commands_update`, `get_available_commands`)
/// as Zeron's. omp runs its own slash commands from a prompt (local ones
/// answer without a model turn), so every one is offered as omp lists it.
pub(super) fn commands(data: &Value) -> Vec<SlashCommand> {
    data["commands"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| {
            let name = c["name"].as_str().filter(|n| !n.is_empty())?;
            Some(SlashCommand {
                name: name.into(),
                description: string(c, "description").into(),
                input_hint: c["input"]["hint"]
                    .as_str()
                    .filter(|h| !h.is_empty())
                    .map(str::to_owned),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn request(reasoning: Option<ReasoningLevel>, off: bool) -> RunRequest {
        let mut options = serde_json::Map::new();
        if off {
            options.insert(THINKING_OPTION.into(), json!("off"));
        }
        RunRequest {
            reasoning,
            model_options: options,
            ..crate::omp::tests_support::request("/tmp")
        }
    }

    #[test]
    fn models_carry_provider_ids_effort_ladders_and_the_default() {
        // Trimmed from omp 18.20.0's get_available_models / get_state.
        let available = json!({"models":[
            {"id":"gpt-5.6-luna","name":"GPT-5.6 Luna","provider":"openai-codex","reasoning":true,
             "thinking":{"mode":"effort","efforts":["low","medium","high","xhigh","max"]}},
            {"id":"local","name":"","provider":"ollama","reasoning":false},
            {"id":"","provider":"broken"}]});
        let models = models(
            &available,
            &json!({"id":"gpt-5.6-luna","provider":"openai-codex"}),
        );
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "openai-codex/gpt-5.6-luna");
        assert_eq!(models[0].label, "GPT-5.6 Luna");
        assert!(models[0].default);
        assert_eq!(
            models[0].reasoning_levels,
            [
                ReasoningLevel::Low,
                ReasoningLevel::Medium,
                ReasoningLevel::High,
                ReasoningLevel::XHigh,
                ReasoningLevel::Max
            ]
        );
        assert_eq!(models[0].options[0].id, THINKING_OPTION);
        assert_eq!(models[1].label, "local");
        assert!(
            models[1].reasoning_levels.is_empty()
                && models[1].options.is_empty()
                && !models[1].default
        );
    }

    #[test]
    fn thinking_levels_clamp_down_and_off_is_exact() {
        let supported = json!({"levels":["off","low","medium","high","xhigh","max"]});
        assert_eq!(
            thinking_level(&request(Some(ReasoningLevel::High), false), &supported),
            Some("high")
        );
        // No minimal here: the lowest effort above it, never off.
        assert_eq!(
            thinking_level(&request(Some(ReasoningLevel::Minimal), false), &supported),
            Some("low")
        );
        assert_eq!(
            thinking_level(
                &request(Some(ReasoningLevel::Ultrathink), false),
                &supported
            ),
            Some("max")
        );
        assert_eq!(
            thinking_level(&request(Some(ReasoningLevel::Low), true), &supported),
            Some("off")
        );
        assert_eq!(thinking_level(&request(None, false), &supported), None);
        let narrow = json!({"levels":["off","low","medium"]});
        assert_eq!(
            thinking_level(&request(Some(ReasoningLevel::Max), false), &narrow),
            Some("medium")
        );
        assert_eq!(
            thinking_level(&request(None, true), &json!({"levels":["medium"]})),
            None
        );
    }

    #[test]
    fn commands_keep_their_input_hints() {
        // Trimmed from omp 18.20.0's available_commands_update.
        let commands = commands(&json!({"commands":[
            {"name":"compact","description":"Compact context","source":"builtin"},
            {"name":"security","description":"Scans","input":{"hint":"<plan|scan>"},"source":"builtin"},
            {"name":"skill:frontend-design","description":"Design","source":"skill"},
            {"description":"nameless"}]}));
        assert_eq!(commands.len(), 3);
        assert_eq!(commands[1].input_hint.as_deref(), Some("<plan|scan>"));
        assert_eq!(commands[2].name, "skill:frontend-design");
    }
}
