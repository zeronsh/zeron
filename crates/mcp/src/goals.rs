//! Goal tools: `get_goal`, `set_goal`, `pause_goal`, `resume_goal`,
//! `clear_goal` (see docs/goal-mode.md).
//!
//! Mutations travel the command plane (`QueueCommand` with a `goal` command),
//! so they reach the chat's host wherever it runs. There is deliberately no
//! tool that completes a goal — only the verifier child chat can — and an
//! agent can neither pause, resume, nor clear the goal that is verifying *it*:
//! stopping that loop is the user's decision, not a way out of verification.

use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};
use zeron_doc::SessionCommandPayload;
use zeron_proto::{Chat, Goal, GoalCommand, GoalLimits, GoalStatus};

use crate::tools::{ToolDef, Tools};

/// How long a goal tool waits for the host to apply its command before it
/// reports the command as queued only.
const APPLY_WAIT: Duration = Duration::from_secs(6);
const APPLY_POLL: Duration = Duration::from_millis(250);

pub(crate) fn catalog() -> Vec<ToolDef> {
    let chat = json!({
        "type": "string",
        "description": "Chat id, unique id prefix, or exact title. Defaults to the chat you are speaking from."
    });
    vec![
        ToolDef {
            name: "get_goal",
            description: "A chat's goal: objective, status (active, verifying, paused, complete, budgetLimited), round, token/time use, the verifier's recent verdicts, and why it stopped, if it did. null when the chat has no goal.",
            input_schema: json!({ "type": "object", "properties": { "chat": chat } }),
        },
        ToolDef {
            name: "set_goal",
            description: "Give a chat a goal: it keeps working turn after turn until an independent verifier chat judges the objective met (or a limit is reached). Safe defaults: 25 rounds, no token or time cap. Fails if the chat already has an unfinished goal unless replace=true. You cannot replace the goal verifying your own chat. A goal can only be completed by its verifier — there is no tool to mark it done.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "objective": { "type": "string", "description": "What must be true when the goal is met, written so it can be checked against the workspace. At most 4000 characters." },
                    "chat": chat,
                    "max_rounds": { "type": "integer", "minimum": 1, "maximum": 200, "description": "Round cap (default 25)." },
                    "token_budget": { "type": "integer", "minimum": 1, "description": "Optional cap on tokens spent by the agent and the verifier." },
                    "time_budget_seconds": { "type": "integer", "minimum": 1, "description": "Optional cap on seconds of goal activity." },
                    "replace": { "type": "boolean", "default": false }
                },
                "required": ["objective"]
            }),
        },
        ToolDef {
            name: "pause_goal",
            description: "Pause another chat's goal (it stops after its current turn). Not allowed on your own chat.",
            input_schema: json!({ "type": "object", "properties": { "chat": chat }, "required": ["chat"] }),
        },
        ToolDef {
            name: "resume_goal",
            description: "Resume another chat's paused or limit-stopped goal; resuming past a limit grants another allowance. Not allowed on your own chat.",
            input_schema: json!({ "type": "object", "properties": { "chat": chat }, "required": ["chat"] }),
        },
        ToolDef {
            name: "clear_goal",
            description: "Remove another chat's goal. Not allowed on your own chat.",
            input_schema: json!({ "type": "object", "properties": { "chat": chat }, "required": ["chat"] }),
        },
    ]
}

#[derive(Deserialize, Default)]
pub(crate) struct GoalChatArgs {
    chat: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct SetGoalArgs {
    objective: String,
    chat: Option<String>,
    max_rounds: Option<u32>,
    token_budget: Option<u64>,
    time_budget_seconds: Option<u64>,
    #[serde(default)]
    replace: bool,
}

/// The goal as the tools report it.
pub(crate) fn goal_json(goal: &Goal) -> Value {
    let mut value = serde_json::to_value(goal).unwrap_or(Value::Null);
    if let Some(object) = value.as_object_mut() {
        // The ledger is controller plumbing; the budget view is what callers want.
        object.remove("pending");
        object.insert("tokensTotal".into(), json!(goal.total_tokens()));
        object.insert(
            "maxRoundsEffective".into(),
            json!(goal.effective_max_rounds()),
        );
    }
    value
}

impl Tools {
    /// Resolve the target chat; the bool says it is this server's own chat.
    async fn goal_target(&self, chat: Option<&str>) -> anyhow::Result<(Chat, bool)> {
        let own_id = self.zeron.origin().chat_id.clone();
        let key = chat
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .map(str::to_owned)
            .or_else(|| own_id.clone());
        let Some(key) = key else {
            anyhow::bail!("no chat: pass `chat` (this server is not running inside a chat)");
        };
        let chat = self.zeron.resolve_chat(&key).await?;
        let own = own_id.as_deref() == Some(chat.id.as_str());
        Ok((chat, own))
    }

    pub(crate) async fn get_goal(&self, args: GoalChatArgs) -> anyhow::Result<Value> {
        let (chat, _) = self.goal_target(args.chat.as_deref()).await?;
        let goal = self.zeron.goal(&chat.id).await?;
        Ok(json!({
            "chatId": chat.id,
            "title": chat.title,
            "goal": goal.as_ref().map(goal_json),
        }))
    }

    pub(crate) async fn set_goal(&self, args: SetGoalArgs) -> anyhow::Result<Value> {
        let (chat, own) = self.goal_target(args.chat.as_deref()).await?;
        let objective = args.objective.trim().to_owned();
        if args.replace && own {
            anyhow::bail!(
                "you cannot replace the goal that is verifying your own chat; ask the user"
            );
        }
        let command = GoalCommand::Set {
            objective: objective.clone(),
            limits: GoalLimits {
                max_rounds: args.max_rounds,
                token_budget: args.token_budget,
                time_budget_seconds: args.time_budget_seconds,
            },
            replace: args.replace,
        };
        let before = self.zeron.goal(&chat.id).await.ok().flatten();
        let command_id = self.queue_goal(&chat, command).await?;
        let applied = self
            .await_goal(&chat, |goal| {
                goal.objective == objective && Some(&goal.id) != before.as_ref().map(|g| &g.id)
            })
            .await;
        Ok(match applied {
            Some(goal) => {
                json!({ "chatId": chat.id, "commandId": command_id, "goal": goal_json(&goal) })
            }
            None => json!({
                "chatId": chat.id,
                "commandId": command_id,
                "queued": true,
                "note": "The host has not applied it yet. If this chat already has an unfinished goal, set replace=true (or clear it first); if its host device is offline, it applies when the host is back."
            }),
        })
    }

    pub(crate) async fn pause_goal(&self, args: GoalChatArgs) -> anyhow::Result<Value> {
        self.goal_transition(args, GoalCommand::Pause, |g| g.status == GoalStatus::Paused)
            .await
    }

    pub(crate) async fn resume_goal(&self, args: GoalChatArgs) -> anyhow::Result<Value> {
        self.goal_transition(args, GoalCommand::Resume, |g| g.status.is_running())
            .await
    }

    pub(crate) async fn clear_goal(&self, args: GoalChatArgs) -> anyhow::Result<Value> {
        let (chat, own) = self.goal_target(args.chat.as_deref()).await?;
        refuse_own(own, "clear")?;
        let command_id = self.queue_goal(&chat, GoalCommand::Clear).await?;
        let cleared = self.await_cleared(&chat).await;
        Ok(json!({ "chatId": chat.id, "commandId": command_id, "cleared": cleared }))
    }

    async fn goal_transition(
        &self,
        args: GoalChatArgs,
        command: GoalCommand,
        done: impl Fn(&Goal) -> bool,
    ) -> anyhow::Result<Value> {
        let (chat, own) = self.goal_target(args.chat.as_deref()).await?;
        let verb = match command {
            GoalCommand::Pause => "pause",
            GoalCommand::Resume => "resume",
            _ => "change",
        };
        refuse_own(own, verb)?;
        let command_id = self.queue_goal(&chat, command).await?;
        let goal = self.await_goal(&chat, done).await;
        Ok(json!({
            "chatId": chat.id,
            "commandId": command_id,
            "goal": goal.as_ref().map(goal_json),
            "applied": goal.is_some(),
        }))
    }

    async fn queue_goal(&self, chat: &Chat, command: GoalCommand) -> anyhow::Result<String> {
        self.zeron
            .queue_command(&chat.id, &SessionCommandPayload::Goal { command })
            .await
    }

    /// Poll the chat's goal until `done` holds (or the wait runs out).
    async fn await_goal(&self, chat: &Chat, done: impl Fn(&Goal) -> bool) -> Option<Goal> {
        let deadline = tokio::time::Instant::now() + APPLY_WAIT;
        loop {
            if let Ok(Some(goal)) = self.zeron.goal(&chat.id).await
                && done(&goal)
            {
                return Some(goal);
            }
            if tokio::time::Instant::now() >= deadline {
                return None;
            }
            tokio::time::sleep(APPLY_POLL).await;
        }
    }

    async fn await_cleared(&self, chat: &Chat) -> bool {
        let deadline = tokio::time::Instant::now() + APPLY_WAIT;
        loop {
            if matches!(self.zeron.goal(&chat.id).await, Ok(None)) {
                return true;
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(APPLY_POLL).await;
        }
    }
}

fn refuse_own(own: bool, verb: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        !own,
        "you cannot {verb} the goal that is verifying your own chat — only the user can. \
         Finish the work; the verifier decides when it is done."
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_tool_can_complete_a_goal() {
        let names: Vec<&str> = catalog().iter().map(|t| t.name).collect();
        assert_eq!(
            names,
            [
                "get_goal",
                "set_goal",
                "pause_goal",
                "resume_goal",
                "clear_goal"
            ]
        );
        // The commands the tools can emit: none marks a goal complete.
        for def in catalog() {
            assert!(!def.name.contains("complete") && !def.name.contains("done"));
            assert_eq!(def.input_schema["type"], "object");
        }
    }

    #[test]
    fn goal_json_hides_the_ledger_and_adds_totals() {
        let mut goal = Goal::new("g", "obj", &GoalLimits::default(), 0).unwrap();
        goal.tokens_used = 10;
        goal.verifier_tokens_used = 5;
        goal.pending = Some(zeron_proto::GoalPending {
            kind: zeron_proto::GoalPendingKind::Turn,
            round: 1,
            message_id: "m".into(),
            started_at: 0,
        });
        let json = goal_json(&goal);
        assert!(json.get("pending").is_none());
        assert_eq!(json["tokensTotal"], 15);
        assert_eq!(json["maxRoundsEffective"], 25);
        assert_eq!(json["status"], "active");
    }
}
