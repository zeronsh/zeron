//! Browser coordination between MCP agent tools and the headed UI.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use serde_json::Value;
use tokio::sync::{broadcast, oneshot, Mutex};

#[derive(Clone)]
pub struct BrowserService {
    commands_tx: broadcast::Sender<Value>,
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<Result<Value, String>>>>>,
    last_state: Arc<Mutex<Value>>,
}

impl Default for BrowserService {
    fn default() -> Self {
        let (tx, _) = broadcast::channel(32);
        Self {
            commands_tx: tx,
            pending: Arc::new(Mutex::new(HashMap::new())),
            last_state: Arc::new(Mutex::new(serde_json::json!({
                "open": false,
                "url": null,
                "title": "",
                "loading": false,
                "consoleLogs": []
            }))),
        }
    }
}

impl BrowserService {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn subscribe_commands(&self) -> broadcast::Receiver<Value> {
        self.commands_tx.subscribe()
    }

    pub async fn update_state(&self, state: Value) {
        if let Some(reply_to) = state.get("replyTo").and_then(|v| v.as_str()) {
            let mut pending = self.pending.lock().await;
            if let Some(tx) = pending.remove(reply_to) {
                let res = if let Some(err) = state.get("error").and_then(|v| v.as_str()) {
                    Err(err.to_string())
                } else {
                    Ok(state.get("result").cloned().unwrap_or(Value::Null))
                };
                let _ = tx.send(res);
            }
        }
        if state.get("replyTo").is_none() {
            let mut st = self.last_state.lock().await;
            *st = state;
        }
    }

    pub async fn get_state(&self) -> Value {
        self.last_state.lock().await.clone()
    }

    pub async fn execute_command(&self, mut command: Value) -> Result<Value, String> {
        if self.commands_tx.receiver_count() == 0 {
            return Err("No active Zeron browser window connected. Please open a browser tab in the sidebar.".into());
        }

        let id = uuid::Uuid::new_v4().to_string();
        if let Some(obj) = command.as_object_mut() {
            obj.insert("id".to_string(), Value::String(id.clone()));
        }

        let (reply_tx, reply_rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().await;
            pending.insert(id.clone(), reply_tx);
        }

        if let Err(_) = self.commands_tx.send(command) {
            let mut pending = self.pending.lock().await;
            pending.remove(&id);
            return Err("Failed to deliver command to browser window".into());
        }

        match tokio::time::timeout(Duration::from_secs(15), reply_rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => {
                let mut pending = self.pending.lock().await;
                pending.remove(&id);
                Err("Browser command failed or channel closed".into())
            }
            Err(_) => {
                let mut pending = self.pending.lock().await;
                pending.remove(&id);
                Err("Browser command timed out after 15 seconds".into())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn execute_command_fails_fast_when_no_ui_listeners() {
        let service = BrowserService::new();
        let res = service
            .execute_command(serde_json::json!({ "action": "navigate", "url": "https://example.com" }))
            .await;
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("No active Zeron browser window connected"));
    }

    #[tokio::test]
    async fn execute_command_delivers_command_and_resolves_reply() {
        let service = BrowserService::new();
        let mut rx = service.subscribe_commands();

        let s = service.clone();
        let handle = tokio::spawn(async move {
            s.execute_command(serde_json::json!({ "action": "navigate", "url": "https://example.com" }))
                .await
        });

        let received = rx.recv().await.expect("broadcast receive");
        let id = received["id"].as_str().expect("has command id");
        assert_eq!(received["action"], "navigate");
        assert_eq!(received["url"], "https://example.com");

        service
            .update_state(serde_json::json!({
                "replyTo": id,
                "result": { "navigated": true }
            }))
            .await;

        let res = handle.await.unwrap().expect("command success");
        assert_eq!(res["navigated"], true);
    }

    #[tokio::test]
    async fn execute_command_propagates_ui_error() {
        let service = BrowserService::new();
        let mut rx = service.subscribe_commands();

        let s = service.clone();
        let handle = tokio::spawn(async move {
            s.execute_command(serde_json::json!({ "action": "click", "selector": "#missing" }))
                .await
        });

        let received = rx.recv().await.expect("broadcast receive");
        let id = received["id"].as_str().expect("has command id");

        service
            .update_state(serde_json::json!({
                "replyTo": id,
                "error": "Element not found"
            }))
            .await;

        let res = handle.await.unwrap();
        assert_eq!(res.unwrap_err(), "Element not found");
    }

    #[tokio::test]
    async fn update_and_get_last_state() {
        let service = BrowserService::new();
        assert_eq!(service.get_state().await["open"], false);

        service
            .update_state(serde_json::json!({
                "open": true,
                "url": "http://localhost:3000",
                "title": "Home",
                "loading": false,
                "consoleLogs": []
            }))
            .await;

        let st = service.get_state().await;
        assert_eq!(st["open"], true);
        assert_eq!(st["url"], "http://localhost:3000");
        assert_eq!(st["title"], "Home");
    }
}
