use super::{normalize::string, rpc::Client};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::{sync::oneshot, task::JoinSet};
use zeron_proto::{UserInputAnswer, UserInputQuestion};
pub(super) type Input =
    dyn Fn(Vec<UserInputQuestion>) -> oneshot::Receiver<Vec<UserInputAnswer>> + Send + Sync;
#[derive(Default)]
pub(super) struct Dialogs {
    pub input: Option<Arc<Input>>,
    tasks: JoinSet<()>,
}
impl Dialogs {
    pub fn request(&mut self, client: Client, frame: &Value) {
        let method = string(frame, "method").to_owned();
        if !matches!(method.as_str(), "select" | "confirm" | "input" | "editor") {
            return;
        }
        while self.tasks.try_join_next().is_some() {}
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
            header: "Pi".into(),
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
        self.tasks.spawn(async move {
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
    }
    pub fn cancel(&mut self) {
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
        let transport = super::super::rpc::Transport::new(writer, reader);
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
}
