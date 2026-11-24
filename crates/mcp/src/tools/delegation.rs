//! Background delegation, for an agent whose host has it on
//! ([`Delegation`]): its subagent chats are recorded as tasks
//! (`RegisterDelegatedTask`) that wake it when they settle, so nothing here
//! blocks unless it asks to. It works with its own children; another chat only
//! when it says the person asked, or that nothing else will do.

use serde::Deserialize;
use serde_json::{Value, json};
use zeron_proto::{AccessMode, Chat, ChildKind, DelegatedStatus, DelegatedTask, DelegatedTaskView, DelegationMode};
use zeron_rpc::methods;

use super::{Delivered, ToolDef, Tools};
use crate::zeron::{Delegation, short};

pub(super) const TASK_STATUS: &str = "task_status";
pub(super) const CANCEL_TASK: &str = "cancel_task";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum Mode {
    Async,
    Wait,
}

/// When results wake the parent, as an agent says it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum Wake {
    #[default]
    Each,
    All,
}

impl Wake {
    fn wire(self) -> zeron_proto::CompletionWake {
        match self {
            Wake::Each => zeron_proto::CompletionWake::Always,
            Wake::All => zeron_proto::CompletionWake::SettledOnly,
        }
    }
}

#[derive(Deserialize, Default)]
pub(super) struct TaskStatusArgs {
    task: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct CancelTaskArgs {
    task: String,
}

/// A task this call started.
pub(super) struct Started {
    task_id: String,
}

impl Started {
    pub(super) fn describe(&self, result: &mut Value, wait: bool) {
        result["taskId"] = json!(self.task_id);
        if !wait {
            result["status"] = json!("running");
            result["note"] = json!(
                "Running in the background. Carry on, or end your turn: this chat then reads as waiting, and you're woken with the result. Don't poll or wait for it."
            );
        }
    }
}

/// The broadest of `modes` no wider than `ceiling`. The ceiling itself when
/// nothing is (or an older engine lists none): the caller then says what it
/// can't run.
pub(super) fn inherited_access(ceiling: AccessMode, modes: &[AccessMode]) -> AccessMode {
    if ceiling.is_full_access() {
        return ceiling;
    }
    let rank = |mode: &AccessMode| AccessMode::WITH_AUTO.iter().position(|m| m == mode).unwrap_or(0);
    modes
        .iter()
        .filter(|mode| rank(mode) <= rank(&ceiling))
        .max_by_key(|mode| rank(mode))
        .copied()
        .unwrap_or(ceiling)
}

const CREATE_CHAT: &str = "Delegate work to a child of your chat (kind subagent, the default): any connected provider (see list_harnesses), whichever you run on, on this device or another of the person's. Put the whole task in `prompt`. It runs in the background: this returns a taskId at once, you carry on or end your turn, your chat then reads as waiting, and Zeron wakes you with the result. Don't wait or poll. `mode: \"wait\"` blocks instead: only for something short you can't go on without. For several at once use create_chats. Delegation is one level deep: a chat that has a parent cannot create chats. kind side is a side conversation for the person; kind chat is a standalone session with no parent, on any device: only when the person asks for one, or the work has to live outside this chat. With cloud, the chat runs on a Cloud machine of its own (any built-in agent), from a Cloud chat or a project with a GitHub repository. Select a project and/or device; catalogs are validated on that host.";
const CREATE_CHATS: &str = "Delegate to several children at once (create_chat's arguments per request, each with its prompt). All run together in the background and wake you as they settle (`wake: \"all\"` on each: once, when every one has). End your turn when you have nothing else to do. Results preserve request order and include per-request errors; successful requests are not rolled back.";
const SEND_MESSAGE: &str = "Send one of your child chats more work or a correction. Messages are attributed to your chat. A child that's idle starts a turn, one that's working reads it at its next step, and either way its result wakes you (mode async, the default: returns at once). mode 'wait' blocks for the reply instead; 'queue' holds it until the child's turn ends. Any other chat needs peer: true.";
const SEND_MESSAGES: &str = "Send messages to several of your child chats at once (send_message's arguments per request). Results preserve request order and include per-request errors; successful sends are not rolled back. Do not include dependent messages to the same chat.";
const WAIT_FOR_TURN: &str = "Block until a chat's turn settles: completed, awaitingInput, interrupted, errored, or timedOut. Your children's results come to you without it: end your turn and you're woken. Use it only when the person asked you to wait, or for a chat that isn't your child. Native pendingInput can be answered with respond_to_input; terminalActivity input must be handled in its terminal. Stale/offline activity is not completion.";

/// The catalog as an agent of `delegation` is offered it.
pub(super) fn adapt(tools: &mut Vec<ToolDef>, delegation: Delegation) {
    if delegation == Delegation::Off {
        return;
    }
    let parent = delegation == Delegation::Parent;
    let wake = json!({ "type": "string", "enum": ["each", "all"], "default": "each", "description": "When the result wakes you: each as it settles (with whatever else settled by then), or once all the tasks this turn started have." });
    let peer = json!({ "type": "boolean", "default": false, "description": "For a chat that isn't your child (nor your parent): set only when the person asked for this, when you're answering a message that chat sent you, or when there is no other way." });
    for tool in tools.iter_mut() {
        let properties = match tool.name {
            "create_chats" | "send_messages" => &mut tool.input_schema["properties"]["requests"]["items"]["properties"],
            _ => &mut tool.input_schema["properties"],
        };
        match tool.name {
            "create_chat" | "create_chats" if parent => {
                properties["mode"] = json!({ "type": "string", "enum": ["async", "wait"], "default": "async", "description": "With prompt, for a subagent: async returns a taskId at once and wakes you with the result; wait blocks until the first turn finishes and returns the reply." });
                properties["wake"] = wake.clone();
                if let Some(properties) = properties.as_object_mut() {
                    properties.remove("wait");
                }
                tool.description = if tool.name == "create_chat" { CREATE_CHAT } else { CREATE_CHATS };
            }
            "send_message" | "send_messages" => {
                properties["peer"] = peer.clone();
                if parent {
                    properties["mode"] = json!({ "type": "string", "enum": ["async", "wait", "run", "steer", "queue"], "default": "async" });
                    properties["wake"] = wake.clone();
                    if let Some(properties) = properties.as_object_mut() {
                        properties.remove("wait");
                    }
                    tool.description = if tool.name == "send_message" { SEND_MESSAGE } else { SEND_MESSAGES };
                }
            }
            "interrupt_chat" | "respond_to_input" | "archive_chat" => properties["peer"] = peer.clone(),
            "wait_for_turn" if parent => tool.description = WAIT_FOR_TURN,
            _ => {}
        }
    }
    if parent {
        tools.push(ToolDef {
            name: TASK_STATUS,
            description: "The work you delegated: each task's child chat, device, agent and status (queued, running, awaitingInput, completed, failed, cancelled), and whether its device can be reached now. You don't need it to get results: they wake you.",
            input_schema: json!({ "type": "object", "properties": {
                "task": { "type": "string", "description": "One task's id. Omit for all, newest first." }
            } }),
        });
        tools.push(ToolDef {
            name: CANCEL_TASK,
            description: "Take back a task you delegated: stops its child chat's turn, and its result no longer wakes you.",
            input_schema: json!({ "type": "object", "properties": {
                "task": { "type": "string", "description": "The task's id." }
            }, "required": ["task"] }),
        });
    }
}

impl Tools {
    fn origin_chat(&self) -> Option<&str> {
        self.zeron.origin().chat_id.as_deref()
    }

    /// Whether `chat` is a child this agent delegates to in the background.
    pub(super) fn is_delegate(&self, chat: &Chat) -> bool {
        self.delegation() == Delegation::Parent
            && chat.parent_chat_id.is_some()
            && chat.parent_chat_id.as_deref() == self.origin_chat()
            && chat.child_kind == Some(ChildKind::Subagent)
    }

    /// An agent works with its children (and answers its parent). Another
    /// chat is somebody else's to run: only when it says why it must.
    pub(super) async fn ensure_within_reach(&self, chat: &Chat, peer: bool) -> anyhow::Result<()> {
        if peer || self.delegation() == Delegation::Off {
            return Ok(());
        }
        let Some(origin) = self.origin_chat() else {
            return Ok(());
        };
        if chat.id == origin || chat.parent_chat_id.as_deref() == Some(origin) {
            return Ok(());
        }
        if self
            .zeron
            .resolve_chat(origin)
            .await
            .ok()
            .and_then(|own| own.parent_chat_id)
            .is_some_and(|parent| parent == chat.id)
        {
            return Ok(());
        }
        anyhow::bail!(
            "{} isn't your child chat. Work with the chats you created (create_chat starts one). Set peer: true only if the person asked you to act on it, you're answering a message it sent you, or there is no other way.",
            chat.title.as_deref().map(str::trim).filter(|t| !t.is_empty()).unwrap_or(short(&chat.id)),
        )
    }

    /// Record a message about to be sent to a child, so its turn's end comes
    /// back here. `None` for a chat that isn't one.
    pub(super) async fn delegate(
        &self,
        chat: &Chat,
        message_id: &str,
        wait: bool,
        wake: Wake,
        title: Option<&str>,
    ) -> anyhow::Result<Option<Started>> {
        if !self.is_delegate(chat) {
            return Ok(None);
        }
        let task: DelegatedTask = serde_json::from_value(
            self.zeron
                .call(
                    methods::REGISTER_DELEGATED_TASK,
                    json!({
                        "chatId": self.origin_chat(),
                        "childChatId": chat.id,
                        "messageId": message_id,
                        "mode": if wait { DelegationMode::Wait } else { DelegationMode::Async },
                        "completionWake": wake.wire(),
                        "title": title,
                    }),
                )
                .await?,
        )?;
        Ok(Some(Started { task_id: task.task_id }))
    }

    async fn update_task(&self, task_id: &str, action: &str, status: Option<DelegatedStatus>) -> anyhow::Result<bool> {
        let reply = self
            .zeron
            .call(
                methods::UPDATE_DELEGATED_TASK,
                json!({ "chatId": self.origin_chat(), "taskId": task_id, "action": action, "status": status }),
            )
            .await?;
        Ok(reply["ok"] == true)
    }

    /// A message that didn't go leaves no task behind.
    pub(super) async fn sent_or_discard(
        &self,
        task: Option<&Started>,
        sent: anyhow::Result<Delivered>,
    ) -> anyhow::Result<Delivered> {
        if let (Err(_), Some(task)) = (&sent, task)
            && let Err(error) = self.update_task(&task.task_id, "discard", None).await
        {
            tracing::warn!(%error, "a task whose message wasn't sent couldn't be discarded");
        }
        sent
    }

    /// How a blocking call ended: the result it returns is the parent's
    /// already; one it gave up on comes as any background task's does.
    pub(super) async fn after_wait(&self, task: &Started, turn: Option<&Value>, result: &mut Value) {
        let updated = match turn.and_then(settled) {
            Some(status) => self.update_task(&task.task_id, "acknowledge", Some(status)).await,
            None => {
                result["note"] = json!(
                    "Not finished: it goes on in the background, and you're woken with the result once it is."
                );
                self.update_task(&task.task_id, "async", None).await
            }
        };
        if let Err(error) = updated {
            tracing::warn!(%error, "a delegated task couldn't be updated");
        }
    }

    /// `wait_for_turn` returned a child's result: it isn't sent again.
    pub(super) async fn acknowledge_turn(&self, chat: &Chat, turn: &Value) {
        let Some(status) = settled(turn).filter(|_| self.is_delegate(chat)) else {
            return;
        };
        let Ok(tasks) = self.delegated().await else {
            return;
        };
        for task in tasks.iter().filter(|t| t.task.child_chat_id == chat.id && t.task.holds_parent()) {
            if let Err(error) = self.update_task(&task.task.task_id, "acknowledge", Some(status)).await {
                tracing::warn!(%error, "a delegated task couldn't be updated");
            }
        }
    }

    async fn delegated(&self) -> anyhow::Result<Vec<DelegatedTaskView>> {
        Ok(serde_json::from_value(
            self.zeron
                .call(methods::LIST_DELEGATED_TASKS, json!({ "chatId": self.origin_chat() }))
                .await?,
        )?)
    }

    pub(super) async fn task_status(&self, args: TaskStatusArgs) -> anyhow::Result<Value> {
        let mut tasks = self.delegated().await?;
        if let Some(id) = args.task.as_deref() {
            tasks.retain(|t| t.task.task_id == id);
            anyhow::ensure!(!tasks.is_empty(), "no task {id:?} of this chat");
        }
        tasks.reverse();
        Ok(json!({
            "tasks": tasks.iter().map(|t| json!({
                "taskId": t.task.task_id,
                "chatId": t.task.child_chat_id,
                "deviceId": t.task.target_device_id,
                "title": t.task.title,
                "harness": t.task.harness,
                "model": t.task.model,
                "status": t.task.status,
                "mode": t.task.mode,
                "delivery": t.task.delivery_state,
                "stopped": t.cohort == zeron_proto::CohortDisposition::Stopped,
                "unreachable": t.unreachable,
                "startedAt": t.task.created_at,
                "settledAt": t.task.settled_at,
            })).collect::<Vec<_>>()
        }))
    }

    pub(super) async fn cancel_task(&self, args: CancelTaskArgs) -> anyhow::Result<Value> {
        anyhow::ensure!(
            self.update_task(&args.task, "cancel", None).await?,
            "no task {:?} of this chat",
            args.task
        );
        Ok(json!({ "taskId": args.task, "status": "cancelled" }))
    }
}

/// How a waited-for turn ended; `None` while it hasn't.
fn settled(turn: &Value) -> Option<DelegatedStatus> {
    match turn["outcome"].as_str()? {
        "completed" => Some(DelegatedStatus::Completed),
        "errored" => Some(DelegatedStatus::Failed),
        "interrupted" => Some(DelegatedStatus::Cancelled),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inherited_access_steps_down_to_what_the_agent_has() {
        use AccessMode::*;
        assert_eq!(inherited_access(FullAccess, &[Supervised]), FullAccess);
        assert_eq!(inherited_access(Auto, &[Supervised, AutoAcceptEdits, FullAccess]), AutoAcceptEdits);
        assert_eq!(inherited_access(Auto, &[Supervised, Auto, FullAccess]), Auto);
        assert_eq!(inherited_access(AutoAcceptEdits, &[Supervised, FullAccess]), Supervised);
        // Nothing it has is narrow enough: never widened (the caller refuses).
        assert_eq!(inherited_access(Supervised, &[FullAccess]), Supervised);
        assert_eq!(inherited_access(Supervised, &[]), Supervised);
    }
}
