//! Models come from `omp models --json`: the same set as RPC
//! `get_available_models`, with each model's thinking levels, and without a
//! `set_model` round trip per model.
use crate::HarnessError;
use serde_json::Value;
use std::{path::Path, time::Duration};
use zeron_proto::{Model, ModelOption, ModelOptionChoice, ReasoningLevel, SlashCommand};

pub(super) const THINKING_OPTION: &str = "omp_thinking";

pub(super) async fn models(
    exe: &Path,
    agent_dir: Option<&Path>,
) -> Result<Vec<Model>, HarnessError> {
    let mut cmd = crate::process::Command::new(exe);
    crate::process::owned::configure(&mut cmd);
    crate::compose_child_path(&mut cmd, exe);
    if let Some(dir) = agent_dir {
        cmd.env("PI_CODING_AGENT_DIR", dir);
        for key in super::sessions::PROFILE_VARS {
            cmd.env_remove(key);
        }
    }
    cmd.args(["models", "--json"])
        .stdin(crate::process::Stdio::null())
        .stdout(crate::process::Stdio::piped())
        .stderr(crate::process::Stdio::piped())
        .kill_on_drop(true);
    let mut child = crate::process::owned::Child::new(cmd.spawn()?);
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let listing = async {
        use tokio::io::AsyncReadExt;
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let (mut stdout, mut stderr) = (stdout.take(64 * 1024 * 1024), stderr.take(65536));
        let (read_out, read_err) =
            tokio::join!(stdout.read_to_end(&mut out), stderr.read_to_end(&mut err),);
        read_out?;
        read_err?;
        Ok::<_, HarnessError>((child.wait().await?, out, err))
    };
    let (status, out, err) = match tokio::time::timeout(Duration::from_secs(60), listing).await {
        Ok(result) => result?,
        Err(_) => {
            child.shutdown(Duration::from_secs(1)).await;
            return Err(HarnessError::Protocol(
                "Oh My Pi model listing timed out".into(),
            ));
        }
    };
    if !status.success() {
        return Err(HarnessError::Protocol(format!(
            "omp models --json failed ({status}): {}",
            String::from_utf8_lossy(&err).trim()
        )));
    }
    let data: Value = serde_json::from_slice(&out)
        .map_err(|e| HarnessError::Protocol(format!("Invalid omp models JSON: {e}")))?;
    Ok(parse_models(&data))
}
fn parse_models(data: &Value) -> Vec<Model> {
    data["models"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|m| m["kind"].as_str().is_none_or(|kind| kind == "chat"))
        .filter_map(|m| {
            let provider = m["provider"].as_str()?;
            let id = m["id"].as_str()?;
            let reasoning_levels: Vec<ReasoningLevel> = m["thinking"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| serde_json::from_value(v.clone()).ok())
                .collect();
            Some(Model {
                id: format!("{provider}/{id}"),
                label: m["name"].as_str().unwrap_or(id).into(),
                description: Some(provider.into()),
                // OMP accepts `off` for every reasoning model.
                options: if m["reasoning"] == true {
                    thinking_option()
                } else {
                    vec![]
                },
                reasoning_levels,
            })
        })
        .collect()
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
/// OMP's command catalog (`get_available_commands` or an
/// `available_commands_update` frame). Builtins such as `/compact` arrive as
/// prompts and OMP runs them itself, so no Zeron-side builtins are added.
pub(super) fn commands(data: &Value) -> Vec<SlashCommand> {
    data["commands"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| {
            Some(SlashCommand {
                name: v["name"].as_str()?.into(),
                description: v["description"].as_str().unwrap_or("").into(),
                input_hint: v["input"]["hint"].as_str().map(str::to_owned),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn models_keep_provider_ids_levels_and_the_thinking_toggle() {
        let models = parse_models(&json!({"models":[
            {"provider":"anthropic","id":"claude-fable-5-1","name":"Claude Fable 5.1","kind":"chat","reasoning":true,"thinking":["low","medium","high","xhigh","max"]},
            {"provider":"anthropic","id":"claude-3-haiku","kind":"chat","reasoning":false,"thinking":null},
            {"provider":"openai","id":"gpt-image","kind":"image","reasoning":false}
        ]}));
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "anthropic/claude-fable-5-1");
        assert_eq!(models[0].label, "Claude Fable 5.1");
        assert_eq!(
            models[0].reasoning_levels,
            vec![
                ReasoningLevel::Low,
                ReasoningLevel::Medium,
                ReasoningLevel::High,
                ReasoningLevel::XHigh,
                ReasoningLevel::Max
            ]
        );
        assert_eq!(models[0].options[0].id, THINKING_OPTION);
        assert_eq!(models[1].label, "claude-3-haiku");
        assert!(models[1].reasoning_levels.is_empty() && models[1].options.is_empty());
    }

    #[test]
    fn commands_carry_input_hints() {
        let commands = commands(&json!({"commands":[
            {"name":"compact","source":"builtin","description":"Compact context","input":{"hint":"[instructions]"}},
            {"name":"skill:review","source":"skill"}
        ]}));
        assert_eq!(commands[0].input_hint.as_deref(), Some("[instructions]"));
        assert_eq!(commands[1].name, "skill:review");
        assert_eq!(commands[1].description, "");
    }
}
