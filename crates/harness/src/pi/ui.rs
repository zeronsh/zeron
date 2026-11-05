use super::{normalize::string, rpc::Client};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex, PoisonError};
use tokio::{sync::oneshot, task::JoinSet};
use zeron_proto::{UserInputAnswer, UserInputQuestion};
pub(super) type Input =
    dyn Fn(Vec<UserInputQuestion>) -> oneshot::Receiver<Vec<UserInputAnswer>> + Send + Sync;
#[derive(Default)]
pub(super) struct Dialogs {
    pub input: Option<Arc<Input>>,
    tasks: JoinSet<()>,
    /// Dialog ids Pi still waits on; whoever removes an id answers it, once.
    open: Arc<Mutex<Vec<Value>>>,
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
        // Pi's `timeout` is in milliseconds; 0 means none, and a negative one
        // has already expired, so the dialog is over before it is shown. One
        // too large for a Duration is as good as none.
        let timeout = match frame["timeout"].as_f64() {
            Some(ms) if ms < 0.0 => {
                let _ =
                    client.send(json!({"type":"extension_ui_response","id":id,"cancelled":true}));
                return;
            }
            Some(ms) if ms > 0.0 => std::time::Duration::try_from_secs_f64(ms / 1000.0).ok(),
            _ => None,
        };
        let question_id = question.id.clone();
        let answer = (input)(vec![question]);
        self.lock().push(id.clone());
        let open = Arc::clone(&self.open);
        self.tasks.spawn(async move {
            let deadline = async {
                match timeout {
                    Some(timeout) => tokio::time::sleep(timeout).await,
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
            let mut open = open.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(index) = open.iter().position(|pending| *pending == id) {
                open.remove(index);
                let _ = client.send(response);
            }
        });
    }
    /// Close every open dialog. The process may live on (a turn ended, an
    /// interrupt), and an extension awaiting its dialog would wait forever,
    /// so each is answered `cancelled` first.
    pub fn cancel(&mut self, client: &Client) {
        self.tasks.abort_all();
        for id in std::mem::take(&mut *self.lock()) {
            let _ = client.send(json!({"type":"extension_ui_response","id":id,"cancelled":true}));
        }
    }
    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Value>> {
        self.open.lock().unwrap_or_else(PoisonError::into_inner)
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
        dialogs.cancel(&transport.client);
        tokio::time::timeout(std::time::Duration::from_secs(1), sender.closed())
            .await
            .unwrap();
        let reply = tokio::time::timeout(std::time::Duration::from_secs(1), lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&reply).unwrap(),
            json!({"type":"extension_ui_response", "id":"stopped", "cancelled":true})
        );
    }

    #[tokio::test]
    async fn zero_timeout_waits_and_negative_timeout_is_already_over() {
        let (writer, peer) = tokio::io::duplex(4096);
        let (_peer_writer, reader) = tokio::io::duplex(4096);
        let transport = super::super::rpc::Transport::new(writer, reader);
        let (bridge, mut requests) = tokio::sync::mpsc::unbounded_channel();
        let mut dialogs = Dialogs {
            input: Some(Arc::new(move |_| {
                let (tx, rx) = oneshot::channel();
                bridge.send(tx).unwrap();
                rx
            })),
            ..Default::default()
        };
        let mut lines = BufReader::new(peer).lines();
        dialogs.request(
            transport.client.clone(),
            &json!({"id":"expired", "method":"confirm", "timeout":-1}),
        );
        let reply = tokio::time::timeout(std::time::Duration::from_secs(1), lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&reply).unwrap(),
            json!({"type":"extension_ui_response", "id":"expired", "cancelled":true})
        );
        assert!(requests.try_recv().is_err(), "an expired dialog was shown");
        dialogs.request(
            transport.client.clone(),
            &json!({"id":"open", "method":"confirm", "timeout":0}),
        );
        let sender = requests.recv().await.unwrap();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), lines.next_line())
                .await
                .is_err(),
            "a zero timeout closed the dialog at once"
        );
        sender
            .send(vec![UserInputAnswer {
                question_id: String::new(),
                labels: vec![],
            }])
            .ok();
        // Answered (with no matching label) only once the user replied.
        let reply = tokio::time::timeout(std::time::Duration::from_secs(1), lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(serde_json::from_str::<Value>(&reply).unwrap()["id"], "open");
    }

    #[tokio::test]
    async fn a_timeout_too_large_for_a_duration_waits_like_none() {
        let (writer, peer) = tokio::io::duplex(4096);
        let (_peer_writer, reader) = tokio::io::duplex(4096);
        let transport = super::super::rpc::Transport::new(writer, reader);
        let (bridge, mut requests) = tokio::sync::mpsc::unbounded_channel();
        let mut dialogs = Dialogs {
            input: Some(Arc::new(move |_| {
                let (tx, rx) = oneshot::channel();
                bridge.send(tx).unwrap();
                rx
            })),
            ..Default::default()
        };
        let mut lines = BufReader::new(peer).lines();
        dialogs.request(
            transport.client.clone(),
            &json!({"id":"huge", "method":"confirm", "timeout":1e300}),
        );
        let sender = requests.recv().await.unwrap();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), lines.next_line())
                .await
                .is_err(),
            "a huge timeout closed the dialog at once"
        );
        sender
            .send(vec![UserInputAnswer {
                question_id: String::new(),
                labels: vec![],
            }])
            .ok();
        let reply = tokio::time::timeout(std::time::Duration::from_secs(1), lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(serde_json::from_str::<Value>(&reply).unwrap()["id"], "huge");
    }
}
