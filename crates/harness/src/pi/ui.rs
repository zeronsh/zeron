use super::{normalize::string, rpc::Client};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, PoisonError},
};
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
    /// Dialog ids Pi still waits on; whoever removes an id answers it, once.
    open: Arc<Mutex<Vec<Value>>>,
    /// Each open dialog's task by id, so OMP's `cancel` can close just that one.
    aborts: HashMap<String, AbortHandle>,
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
            open: Arc::default(),
            aborts: HashMap::new(),
        }
    }
    pub fn request(&mut self, client: Client, frame: &Value) {
        let method = string(frame, "method").to_owned();
        if method == "cancel" {
            // OMP already resolved the dialog (timeout or abort); close it unanswered.
            let target = string(frame, "targetId");
            self.lock()
                .retain(|pending| pending.as_str() != Some(target));
            if let Some(task) = self.aborts.remove(target) {
                task.abort();
            }
            return;
        }
        if !matches!(method.as_str(), "select" | "confirm" | "input" | "editor") {
            return;
        }
        while self.tasks.try_join_next().is_some() {}
        self.aborts.retain(|_, task| !task.is_finished());
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
        let key = string(frame, "id").to_owned();
        let task = self.tasks.spawn(async move {
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
        self.aborts.insert(key, task);
    }
    /// Close every open dialog. The process may live on (a turn ended, an
    /// interrupt), and an extension awaiting its dialog would wait forever,
    /// so each is answered `cancelled` first.
    pub fn cancel(&mut self, client: &Client) {
        self.aborts.clear();
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
        let transport = super::super::rpc::Transport::new("Pi", writer, reader);
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
        let transport = super::super::rpc::Transport::new("Pi", writer, reader);
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
