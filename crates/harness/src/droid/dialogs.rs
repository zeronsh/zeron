//! The runner owns pending native ids. Removing an id answers it exactly once,
//! even when stop, terminal notifications and a late UI answer overlap.
use std::collections::HashMap;

use futures::{StreamExt, future::BoxFuture, stream::FuturesUnordered};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use zeron_proto::{AccessMode, UserInputAnswer, UserInputQuestion};

use super::{Output, normalize::string};
use crate::{
    approval::{self, Action},
    jsonrpc::RpcClient,
};

/// A server request the person has been asked about.
struct Open {
    id: Value,
    /// What it is answered with when the turn ends first.
    cancelled: Value,
}

#[derive(Default)]
pub(super) struct Dialogs {
    open: HashMap<String, Open>,
    answers: FuturesUnordered<BoxFuture<'static, (String, Value)>>,
}

impl Dialogs {
    pub async fn ask(
        &mut self,
        tx: &mpsc::Sender<Output>,
        id: Value,
        questions: Vec<UserInputQuestion>,
        response: impl FnOnce(Vec<UserInputAnswer>) -> Value + Send + 'static,
        cancelled: Value,
    ) {
        let key = id.to_string();
        if self.open.contains_key(&key) {
            return;
        }
        let (reply, receiver) = oneshot::channel();
        self.open.insert(
            key.clone(),
            Open {
                id,
                cancelled: cancelled.clone(),
            },
        );
        let _ = tx.send(Output::Input { questions, reply }).await;
        self.answers.push(Box::pin(async move {
            let result = match receiver.await {
                Ok(answers) => response(answers.await.unwrap_or_default()),
                Err(_) => cancelled,
            };
            (key, result)
        }));
    }

    pub fn has_pending(&self) -> bool {
        !self.answers.is_empty()
    }

    pub async fn next(&mut self) -> Option<(String, Value)> {
        self.answers.next().await
    }

    pub fn answer(&mut self, client: &RpcClient, key: &str, result: Value) {
        if let Some(open) = self.open.remove(key) {
            client.respond(&open.id, result);
        }
    }

    pub fn cancel(&mut self, client: &RpcClient) {
        self.answers.clear();
        for (_, open) in self.open.drain() {
            client.respond(&open.id, open.cancelled);
        }
    }
}

/// Answers a server request nobody can be asked about: no turn is running,
/// or the one that was is being stopped.
pub(super) fn refuse(client: &RpcClient, id: &Value, method: &str) {
    match method {
        "droid.request_permission" => client.respond(id, cancel_permission()),
        "droid.ask_user" => client.respond(id, cancel_questions()),
        _ => client.respond_error(id, -32601, "Method not found"),
    }
}

pub(super) fn cancel_permission() -> Value {
    json!({"selectedOption":"cancel"})
}

pub(super) fn cancel_questions() -> Value {
    json!({"cancelled":true,"answers":[]})
}

pub(super) fn offered(params: &Value, option: &str) -> bool {
    params["options"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|v| v.as_str() == Some(option) || v["value"].as_str() == Some(option))
}

pub(super) fn access(request: &zeron_proto::RunRequest) -> AccessMode {
    if request.auto_approve {
        AccessMode::FullAccess
    } else {
        request.access
    }
}

pub(super) fn autonomy(access: AccessMode) -> &'static str {
    match access {
        AccessMode::Supervised => "off",
        AccessMode::AutoAcceptEdits => "medium",
        AccessMode::FullAccess => "high",
    }
}

/// The option that leaves Spec mode at `access`'s autonomy.
pub(super) fn proceed(access: AccessMode) -> &'static str {
    match access {
        AccessMode::Supervised => "proceed_once",
        AccessMode::AutoAcceptEdits => "proceed_auto_run_medium",
        AccessMode::FullAccess => "proceed_auto_run_high",
    }
}

pub(super) fn spec(params: &Value) -> Option<&Value> {
    params["toolUses"].as_array()?.iter().find(|item| {
        string(item, "confirmationType") == "exit_spec_mode"
            || string(&item["toolUse"], "name") == "ExitSpecMode"
    })
}

pub(super) fn plan(item: &Value) -> Option<String> {
    let details = &item["details"];
    let input = &item["toolUse"]["input"];
    let plan = details["plan"]
        .as_str()
        .or_else(|| input["plan"].as_str())
        .filter(|plan| !plan.trim().is_empty())?;
    let title = details["title"]
        .as_str()
        .or_else(|| input["title"].as_str())
        .unwrap_or("");
    let text = if title.trim().is_empty() {
        plan.to_owned()
    } else {
        format!("# {title}\n\n{plan}")
    };
    (text.len() <= 64 * 1024).then_some(text)
}

fn action(item: &Value) -> Action {
    if matches!(
        string(&item["toolUse"], "name"),
        "Edit" | "Create" | "ApplyPatch" | "MultiEdit"
    ) || matches!(string(item, "confirmationType"), "edit" | "create")
    {
        Action::Edit
    } else {
        Action::Other
    }
}

pub(super) fn permission_question(params: &Value, access: AccessMode) -> Option<UserInputQuestion> {
    let items: Vec<_> = params["toolUses"]
        .as_array()
        .into_iter()
        .flatten()
        .collect();
    if !items.is_empty()
        && items
            .iter()
            .all(|item| approval::allowed_without_asking(access, action(item)))
    {
        return None;
    }
    if items.len() == 1 {
        let item = items[0];
        let tool = &item["toolUse"];
        let input = &tool["input"];
        return Some(match string(tool, "name") {
            "Execute" => approval::command("Droid", string(input, "command")),
            _ if action(item) == Action::Edit => {
                let path = input["file_path"]
                    .as_str()
                    .or_else(|| item["details"]["filePath"].as_str())
                    .unwrap_or("");
                approval::edit("Droid", &[path])
            }
            name => approval::tool("Droid", name, None),
        });
    }
    let names = items
        .iter()
        .map(|item| string(&item["toolUse"], "name"))
        .collect::<Vec<_>>()
        .join(", ");
    Some(approval::tool("Droid", &names, None))
}

pub(super) fn questions(params: &Value) -> Vec<UserInputQuestion> {
    params["questions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|q| {
            let index = q["index"].as_u64()?;
            let text = q["question"].as_str()?;
            Some(UserInputQuestion {
                id: index.to_string(),
                header: string(q, "topic").into(),
                question: text.into(),
                options: q["options"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|s| s.as_str().map(str::to_owned))
                    .collect(),
                multi_select: q["multiSelect"].as_bool().unwrap_or(false),
                prefill: None,
                multiline: false,
            })
        })
        .collect()
}

pub(super) fn question_response(
    questions: &[UserInputQuestion],
    answers: &[UserInputAnswer],
) -> Value {
    let native: Option<Vec<Value>> = questions
        .iter()
        .map(|question| {
            let answer = answers.iter().find(|a| a.question_id == question.id)?;
            let labels: Vec<_> = answer
                .labels
                .iter()
                .map(|label| label.trim())
                .filter(|label| !label.is_empty())
                .collect();
            (!labels.is_empty()).then(|| {
                json!({
                    "index": question.id.parse::<u64>().unwrap_or_default(),
                    "question": question.question,
                    "answer": labels.join(","),
                })
            })
        })
        .collect();
    native.map_or_else(cancel_questions, |answers| json!({ "answers": answers }))
}
