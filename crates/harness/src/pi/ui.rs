use super::{normalize::string, rpc::Client};
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc};
use tokio::{
    sync::oneshot,
    task::{AbortHandle, JoinSet},
};
use zeron_proto::{UserInputAnswer, UserInputQuestion};
pub(crate) type Input =
    dyn Fn(Vec<UserInputQuestion>) -> oneshot::Receiver<Vec<UserInputAnswer>> + Send + Sync;
pub(crate) struct Dialogs {
    label: &'static str,
    pub input: Option<Arc<Input>>,
    tasks: JoinSet<()>,
    open: HashMap<String, AbortHandle>,
}
impl Default for Dialogs {
    fn default() -> Self {
        Self::new("Pi")
    }
}
impl Dialogs {
    pub fn new(label: &'static str) -> Self {
        Self {
            label,
            input: None,
            tasks: JoinSet::new(),
            open: HashMap::new(),
        }
    }
    pub fn request(&mut self, client: Client, frame: &Value) {
        let method = string(frame, "method").to_owned();
        if method == "cancel" {
            // OMP already resolved the dialog (timeout or abort); close it unanswered.
            if let Some(task) = self.open.remove(string(frame, "targetId")) {
                task.abort();
            }
            return;
        }
        if !matches!(method.as_str(), "select" | "confirm" | "input" | "editor") {
            return;
        }
        while self.tasks.try_join_next().is_some() {}
        self.open.retain(|_, task| !task.is_finished());
        let id = frame["id"].clone();
        let Some(input) = &self.input else {
            let _ = client.send(json!({"type":"extension_ui_response","id":id,"cancelled":true}));
            return;
        };
        let options = if method == "confirm" {
            vec!["Yes".into(), "No".into()]
        } else {
            frame["options"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        };
        let title = string(frame, "title");
        let message = string(frame, "message");
        let question = UserInputQuestion {
            id: uuid::Uuid::new_v4().to_string(),
            header: self.label.into(),
            question: if message.is_empty() {
                title.into()
            } else {
                format!("{title}\n{message}")
            },
            options: options.clone(),
            multi_select: false,
            prefill: frame["prefill"].as_str().map(str::to_owned),
            multiline: method == "editor",
        };
        let question_id = question.id.clone();
        let answer = (input)(vec![question]);
        let timeout = frame["timeout"].as_u64();
        let key = string(frame, "id").to_owned();
        let task = self.tasks.spawn(async move {
            let deadline = async {
                match timeout {
                    Some(ms) => tokio::time::sleep(std::time::Duration::from_millis(ms)).await,
                    None => std::future::pending().await,
                }
            };
            let answers =
                tokio::select! { result=answer=>result.unwrap_or_default(),_=deadline=>vec![] };
            let value = answers
                .iter()
                .find(|a| a.question_id == question_id)
                .and_then(|a| a.labels.first());
            let mut response = json!({"type":"extension_ui_response","id":id});
            match (method.as_str(), value) {
                ("confirm", Some(value)) if options.contains(value) => {
                    response["confirmed"] = json!(value == "Yes")
                }
                ("select", Some(value)) if options.contains(value) => {
                    response["value"] = json!(value)
                }
                ("input" | "editor", Some(value)) => response["value"] = json!(value),
                _ => response["cancelled"] = json!(true),
            }
            let _ = client.send(response);
        });
        self.open.insert(key, task);
    }
    pub fn cancel(&mut self) {
        self.open.clear();
        self.tasks.abort_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, BufReader};

    #[tokio::test]
    async fn timeout_cancels_instead_of_selecting_and_drops_the_input_receiver() {
        let (writer, peer) = tokio::io::duplex(4096);
        let (_peer_writer, reader) = tokio::io::duplex(4096);
        let transport = super::super::rpc::Transport::new("Pi", writer, reader);
        let (bridge, mut requests) = tokio::sync::mpsc::unbounded_channel();
        let mut dialogs = Dialogs::default();
        dialogs.input = Some(Arc::new(move |_| {
            let (tx, rx) = oneshot::channel();
            bridge.send(tx).unwrap();
            rx
        }));
        dialogs.request(
            transport.client.clone(),
            &json!({
                "id":"expired", "method":"select", "options":["first"], "timeout":10
            }),
        );
        let mut sender = requests.recv().await.unwrap();
        let mut lines = BufReader::new(peer).lines();
        let reply = tokio::time::timeout(std::time::Duration::from_secs(1), lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&reply).unwrap(),
            json!({
                "type":"extension_ui_response", "id":"expired", "cancelled":true
            })
        );
        sender.closed().await;
        dialogs.request(
            transport.client.clone(),
            &json!({"id":"stopped", "method":"input"}),
        );
        let mut sender = requests.recv().await.unwrap();
        dialogs.cancel();
        tokio::time::timeout(std::time::Duration::from_secs(1), sender.closed())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn a_cancel_request_closes_only_its_target_dialog() {
        let (writer, _peer) = tokio::io::duplex(4096);
        let (_peer_writer, reader) = tokio::io::duplex(4096);
        let transport = super::super::rpc::Transport::new("Oh My Pi", writer, reader);
        let (bridge, mut requests) = tokio::sync::mpsc::unbounded_channel();
        let mut dialogs = Dialogs::new("Oh My Pi");
        dialogs.input = Some(Arc::new(move |questions: Vec<UserInputQuestion>| {
            assert_eq!(questions[0].header, "Oh My Pi");
            let (tx, rx) = oneshot::channel();
            bridge.send(tx).unwrap();
            rx
        }));
        for id in ["kept", "closed"] {
            dialogs.request(
                transport.client.clone(),
                &json!({"id":id, "method":"input"}),
            );
        }
        let kept = requests.recv().await.unwrap();
        let mut closed = requests.recv().await.unwrap();
        dialogs.request(
            transport.client.clone(),
            &json!({"method":"cancel", "targetId":"closed"}),
        );
        tokio::time::timeout(std::time::Duration::from_secs(1), closed.closed())
            .await
            .unwrap();
        assert!(!kept.is_closed());
    }
}
