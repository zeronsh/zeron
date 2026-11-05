//! The app-facing pipes: tool calls (`ServePluginTools` / `CallPluginTool` /
//! `PluginToolResult`) and the device channel (`WatchPluginChannel`). Each
//! has one subscriber at a time: the latest wins and an older one's stream
//! ends.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt;
use futures::stream::BoxStream;
use tokio::sync::{mpsc, oneshot};

pub const TOOL_TIMEOUT: Duration = Duration::from_secs(30);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// One subscriber slot, latest wins.
#[derive(Default)]
pub struct Slot {
    current: Mutex<Option<(u64, mpsc::UnboundedSender<serde_json::Value>)>>,
    next: AtomicU64,
}

impl Slot {
    pub fn is_open(&self) -> bool {
        lock(&self.current)
            .as_ref()
            .is_some_and(|(_, tx)| !tx.is_closed())
    }

    /// Push to the subscriber; false when there's none.
    pub fn push(&self, value: serde_json::Value) -> bool {
        lock(&self.current)
            .as_ref()
            .is_some_and(|(_, tx)| tx.send(value).is_ok())
    }

    /// A new subscriber's stream. `on_close` runs when it drops while still
    /// the current one (not when a newer subscriber replaced it).
    pub fn subscribe(
        self: &Arc<Self>,
        on_close: impl Fn() + Send + Sync + 'static,
    ) -> BoxStream<'static, serde_json::Value> {
        let (tx, rx) = mpsc::unbounded_channel();
        let generation = self.next.fetch_add(1, Ordering::SeqCst) + 1;
        // Replacing drops the older sender: its stream ends.
        *lock(&self.current) = Some((generation, tx));
        struct Guard {
            slot: Arc<Slot>,
            generation: u64,
            on_close: Box<dyn Fn() + Send + Sync>,
        }
        impl Drop for Guard {
            fn drop(&mut self) {
                let was_current = {
                    let mut current = lock(&self.slot.current);
                    let mine = current.as_ref().is_some_and(|(g, _)| *g == self.generation);
                    if mine {
                        *current = None;
                    }
                    mine
                };
                // A replaced subscriber leaves the newer one's state alone.
                if was_current {
                    (self.on_close)();
                }
            }
        }
        let guard = Guard {
            slot: self.clone(),
            generation,
            on_close: Box::new(on_close),
        };
        futures::stream::unfold((rx, guard), |(mut rx, guard)| async move {
            rx.recv().await.map(|value| (value, (rx, guard)))
        })
        .boxed()
    }
}

/// What the app answered for one call.
#[derive(Debug, Clone, PartialEq)]
pub enum ToolAnswer {
    Ok(serde_json::Value),
    Err(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum CallError {
    /// Nobody serving, or the server went away mid-call.
    NotOpen,
    TimedOut,
    Failed(String),
}

/// Calls waiting for the app's answer.
#[derive(Default)]
pub struct ToolBroker {
    pub serve: Arc<Slot>,
    pending: Mutex<HashMap<String, oneshot::Sender<ToolAnswer>>>,
}

impl ToolBroker {
    /// The server went away: every waiting call fails now rather than at its
    /// timeout.
    pub fn drop_pending(&self) {
        lock(&self.pending).clear();
    }

    pub async fn call(
        &self,
        plugin: &str,
        tool: &str,
        args: serde_json::Value,
        chat_id: Option<&str>,
        timeout: Duration,
    ) -> Result<serde_json::Value, CallError> {
        if !self.serve.is_open() {
            return Err(CallError::NotOpen);
        }
        let call_id = crate::new_id();
        let (tx, rx) = oneshot::channel();
        lock(&self.pending).insert(call_id.clone(), tx);
        let mut item =
            serde_json::json!({ "callId": call_id, "plugin": plugin, "tool": tool, "args": args });
        if let Some(chat_id) = chat_id {
            item["chatId"] = serde_json::json!(chat_id);
        }
        if !self.serve.push(item) {
            lock(&self.pending).remove(&call_id);
            return Err(CallError::NotOpen);
        }
        let answer = tokio::time::timeout(timeout, rx).await;
        lock(&self.pending).remove(&call_id);
        match answer {
            Err(_) => Err(CallError::TimedOut),
            Ok(Err(_)) => Err(CallError::NotOpen),
            Ok(Ok(ToolAnswer::Ok(value))) => Ok(value),
            Ok(Ok(ToolAnswer::Err(error))) => Err(CallError::Failed(error)),
        }
    }

    /// The app's answer; an unknown (late, finished) call id is ignored.
    pub fn answer(&self, call_id: &str, answer: ToolAnswer) -> bool {
        match lock(&self.pending).remove(call_id) {
            Some(tx) => tx.send(answer).is_ok(),
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn plugins_broker_round_trip_timeout_and_latest_server_wins() {
        let broker = Arc::new(ToolBroker::default());
        // Nobody serving.
        assert_eq!(
            broker
                .call("focus", "count", serde_json::json!({}), None, TOOL_TIMEOUT)
                .await,
            Err(CallError::NotOpen)
        );
        let mut old = broker.serve.subscribe(|| {});
        let mut server = broker.serve.subscribe(|| {});
        assert!(old.next().await.is_none(), "an older server's stream ends");
        let answering = broker.clone();
        let app = tokio::spawn(async move {
            let call = server.next().await.unwrap();
            assert_eq!(call["tool"], "count");
            assert_eq!(call["chatId"], "chat-1");
            let id = call["callId"].as_str().unwrap().to_owned();
            answering.answer(&id, ToolAnswer::Ok(serde_json::json!({ "words": 2 })));
            let call = server.next().await.unwrap();
            let id = call["callId"].as_str().unwrap().to_owned();
            answering.answer(&id, ToolAnswer::Err("no text".into()));
            // Third call: never answered.
            let _ = server.next().await.unwrap();
            server
        });
        let value = broker
            .call(
                "focus",
                "count",
                serde_json::json!({ "text": "a b" }),
                Some("chat-1"),
                TOOL_TIMEOUT,
            )
            .await
            .unwrap();
        assert_eq!(value, serde_json::json!({ "words": 2 }));
        assert_eq!(
            broker
                .call("focus", "count", serde_json::json!({}), None, TOOL_TIMEOUT)
                .await,
            Err(CallError::Failed("no text".into()))
        );
        assert_eq!(
            broker
                .call(
                    "focus",
                    "count",
                    serde_json::json!({}),
                    None,
                    Duration::from_millis(50)
                )
                .await,
            Err(CallError::TimedOut)
        );
        let server = app.await.unwrap();
        assert!(broker.serve.is_open());
        drop(server);
        assert!(!broker.serve.is_open(), "nothing serves once it drops");
        assert!(!broker.answer("stale", ToolAnswer::Ok(serde_json::Value::Null)));
    }

    #[tokio::test]
    async fn plugins_broker_server_going_away_fails_waiting_calls() {
        let broker = Arc::new(ToolBroker::default());
        let closer = broker.clone();
        let mut server = broker.serve.subscribe(move || closer.drop_pending());
        let call = {
            let broker = broker.clone();
            tokio::spawn(async move {
                broker
                    .call("p", "t", serde_json::json!({}), None, TOOL_TIMEOUT)
                    .await
            })
        };
        let _ = server.next().await.unwrap();
        drop(server);
        assert_eq!(call.await.unwrap(), Err(CallError::NotOpen));
    }
}
