use super::{Process, normalize::string};
use crate::HarnessError;
use serde_json::{Value, json};
use zeron_proto::{Model, ModelOption, ModelOptionChoice, ReasoningLevel, SlashCommand};

pub(super) fn levels(data: &Value) -> Vec<ReasoningLevel> {
    data["levels"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| serde_json::from_value(v.clone()).ok())
        .collect()
}
pub(super) async fn models(process: &mut Process) -> Result<Vec<Model>, HarnessError> {
    let mut backlog = vec![];
    let data = process
        .query(json!({"type":"get_available_models"}), &mut backlog)
        .await?;
    let mut result = vec![];
    for model in data["models"].as_array().into_iter().flatten() {
        let provider = string(model, "provider");
        let id = string(model, "id");
        process
            .query(
                json!({"type":"set_model","provider":provider,"modelId":id}),
                &mut backlog,
            )
            .await?;
        let supported = process
            .query(
                json!({"type":"get_available_thinking_levels"}),
                &mut backlog,
            )
            .await?;
        result.push(Model {
            id: format!("{provider}/{id}"),
            label: model["name"].as_str().unwrap_or(id).into(),
            description: Some(provider.into()),
            reasoning_levels: levels(&supported),
            options: thinking_option(&supported),
        });
        backlog.clear();
    }
    Ok(result)
}
fn thinking_option(data: &Value) -> Vec<ModelOption> {
    if !data["levels"]
        .as_array()
        .is_some_and(|v| v.iter().any(|s| s == "off"))
    {
        return vec![];
    }
    vec![ModelOption {
        id: "pi_thinking".into(),
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
pub(super) fn commands(data: &Value) -> Vec<SlashCommand> {
    let mut commands: Vec<_> = data["commands"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| {
            let name = v["name"].as_str()?;
            Some(SlashCommand {
                name: name.into(),
                description: string(v, "description").into(),
                input_hint: None,
            })
        })
        .collect();
    for (name, description) in [
        ("compact", "Compact context"),
        ("session", "Session statistics"),
        ("name", "Set session name"),
        ("export", "Export session to HTML"),
        ("autocompact", "Toggle automatic compaction"),
        ("steering", "Steering queue mode"),
        ("follow-up", "Follow-up queue mode"),
    ] {
        if !commands.iter().any(|c| c.name == name) {
            commands.push(SlashCommand {
                name: name.into(),
                description: description.into(),
                input_hint: None,
            });
        }
    }
    commands
}
pub(super) fn builtin(text: &str, auto: bool) -> Option<Value> {
    let text = text.trim();
    let (name, args) = text.split_once(char::is_whitespace).unwrap_or((text, ""));
    let args = args.trim();
    Some(match name {
        "/compact" => json!({"type":"compact","customInstructions":args}),
        "/session" => json!({"type":"get_session_stats"}),
        "/name" => json!({"type":"set_session_name","name":args}),
        "/export" => {
            if args.is_empty() {
                json!({"type":"export_html"})
            } else {
                json!({"type":"export_html","outputPath":args})
            }
        }
        "/autocompact" => {
            json!({"type":"set_auto_compaction","enabled":match args {"on"=>true,"off"=>false,_=>!auto}})
        }
        "/steering" if !args.is_empty() => json!({"type":"set_steering_mode","mode":args}),
        "/follow-up" if !args.is_empty() => json!({"type":"set_follow_up_mode","mode":args}),
        "/steering" | "/follow-up" => json!({"type":"get_state"}),
        _ => return None,
    })
}
