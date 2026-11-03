//! Non-blocking questions and waits: the tools' defaults, what they ask
//! the engine for, and that the blocking forms still block and time out.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures::StreamExt;
use serde_json::{Value, json};
use zeron_rpc::{RpcError, RpcReply, RpcService, memory_client, methods};

use super::{Tools, catalog};
use crate::zeron::{Origin, Zeron};
use std::sync::Arc;

/// One chat ("Beta") whose session is `session`, a two-message transcript,
/// and a question store; every write is recorded.
struct Engine {
    session: Mutex<Value>,
    questions: Mutex<Vec<Value>>,
    /// Answer every blocking question this long after it's asked.
    answer_after: Option<Duration>,
    writes: Mutex<Vec<(String, Value)>>,
}

impl Engine {
    fn new(status: &str) -> Arc<Self> {
        Self::with(status, None)
    }

    fn with(status: &str, answer_after: Option<Duration>) -> Arc<Self> {
        Arc::new(Self {
            session: Mutex::new(json!([{
                "chatId": "chat-beta-2", "deviceId": "dev-local", "status": status,
                "startedAt": null, "updatedAt": chrono::Utc::now(),
                "lastCompletedTurn": "turn-1",
            }])),
            questions: Mutex::default(),
            answer_after,
            writes: Mutex::default(),
        })
    }

    fn writes(&self, method: &str) -> Vec<Value> {
        self.writes
            .lock()
            .unwrap()
            .iter()
            .filter(|(m, _)| m == method)
            .map(|(_, p)| p.clone())
            .collect()
    }
}

fn once(item: Value) -> RpcReply {
    RpcReply::Stream(futures::stream::iter(vec![item]).boxed())
}

#[async_trait]
impl RpcService for Engine {
    async fn handle(&self, method: &str, params: Value) -> Result<RpcReply, RpcError> {
        self.writes
            .lock()
            .unwrap()
            .push((method.to_owned(), params.clone()));
        Ok(match method {
            methods::WATCH_CHATS => once(json!([{
                "id": "chat-alpha-1", "deviceId": "dev-local", "title": "Alpha",
                "archived": false, "createdAt": "2026-09-01T00:00:00Z",
                "config": { "harness": "claude-code", "sandbox": "workspace-write" }
            }, {
                "id": "chat-beta-2", "deviceId": "dev-local", "title": "Beta",
                "archived": false, "createdAt": "2026-09-02T00:00:00Z",
                "config": { "harness": "claude-code", "sandbox": "workspace-write" }
            }])),
            methods::WATCH_SPACES => once(json!([])),
            methods::WATCH_SESSIONS => once(self.session.lock().unwrap().clone()),
            methods::LIST_HARNESSES => RpcReply::Value(json!([{
                "id": "claude-code", "name": "Claude Code", "supportsSteering": true,
                "steeringMode": "step-boundary", "installed": true
            }])),
            methods::LIST_AGENT_ACCOUNTS => return Err(RpcError::UnknownMethod(method.into())),
            methods::WATCH_DOC_MESSAGES => once(json!({ "reset": [
                { "id": "u1", "role": "user", "createdAt": 1, "deviceId": "dev-local",
                  "parts": [{ "kind": "text", "id": "t", "text": "hi" }] },
                { "id": "a1", "role": "assistant", "createdAt": 2, "deviceId": "dev-local",
                  "status": "complete",
                  "parts": [{ "kind": "text", "id": "t", "text": "hello back" }] }
            ]})),
            methods::QUEUE_COMMAND | methods::QUEUE_MESSAGE => {
                RpcReply::Value(json!({ "commandId": "cmd-1", "id": "m-1" }))
            }
            methods::NOTIFY_TURN_END => {
                RpcReply::Value(json!({ "watchId": "w-1", "watching": true }))
            }
            methods::ASK_QUESTION => {
                let question = json!({
                    "id": "q-1", "chatId": params["chatId"], "question": params["question"],
                    "options": params["options"], "blocking": params["blocking"],
                    "status": "pending", "answer": null, "createdAt": "2026-11-03T00:00:00Z",
                });
                self.questions.lock().unwrap().push(question.clone());
                RpcReply::Value(question)
            }
            methods::WATCH_QUESTIONS => {
                let now: Vec<Value> = self.questions.lock().unwrap().clone();
                let Some(after) = self.answer_after else {
                    // Never answered: the first snapshot, then silence.
                    return Ok(RpcReply::Stream(
                        futures::stream::iter(vec![json!(now)])
                            .chain(futures::stream::pending())
                            .boxed(),
                    ));
                };
                let answered: Vec<Value> = now
                    .iter()
                    .map(|q| {
                        let mut q = q.clone();
                        q["status"] = json!("answered");
                        q["answer"] = json!("main");
                        q
                    })
                    .collect();
                RpcReply::Stream(
                    futures::stream::iter(vec![json!(now)])
                        .chain(futures::stream::once(async move {
                            tokio::time::sleep(after).await;
                            json!(answered)
                        }))
                        .chain(futures::stream::pending())
                        .boxed(),
                )
            }
            methods::RELEASE_QUESTION | methods::CANCEL_QUESTION => {
                let questions = self.questions.lock().unwrap();
                let q = questions
                    .iter()
                    .find(|q| q["id"] == params["questionId"])
                    .cloned()
                    .unwrap_or(Value::Null);
                RpcReply::Value(q)
            }
            other => return Err(RpcError::UnknownMethod(other.into())),
        })
    }
}

fn tools_in(engine: &Arc<Engine>, chat: Option<&str>) -> Tools {
    let origin = Origin {
        chat_id: chat.map(str::to_owned),
        ..Default::default()
    };
    Tools::new(Arc::new(Zeron::with_client(
        memory_client(engine.clone()),
        origin,
    )))
}

fn schema(tool: &str) -> Value {
    catalog()
        .into_iter()
        .find(|t| t.name == tool)
        .unwrap_or_else(|| panic!("no tool {tool}"))
        .input_schema
}

#[test]
fn every_wait_defaults_to_non_blocking() {
    let ask = schema("ask_question");
    assert_eq!(ask["properties"]["blocking"]["default"], false);
    assert_eq!(ask["required"], json!(["question"]));
    assert_eq!(ask["properties"]["options"]["type"], "array");
    assert_eq!(
        schema("wait_for_turn")["properties"]["blocking"]["default"],
        false
    );
    for tool in ["send_message", "create_chat"] {
        let props = &schema(tool)["properties"];
        assert_eq!(props["wait"]["default"], false, "{tool}");
        assert_eq!(props["notify"]["default"], false, "{tool}");
    }
    // The batch tools carry the same per-request arguments.
    let batch = schema("send_messages");
    assert_eq!(
        batch["properties"]["requests"]["items"]["properties"]["notify"]["default"],
        false
    );
    for tool in ["get_question", "cancel_question"] {
        assert_eq!(schema(tool)["required"], json!(["question_id"]), "{tool}");
    }
    assert!(schema("list_questions")["properties"]["include_resolved"].is_object());
}

#[tokio::test]
async fn a_question_returns_at_once_as_pending() {
    let engine = Engine::new("working");
    let tools = tools_in(&engine, Some("chat-alpha-1"));
    let started = Instant::now();
    let asked = tools
        .call(
            "ask_question",
            json!({ "question": "Which branch?", "options": ["main", "dev"] }),
        )
        .await
        .unwrap();
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(asked["question_id"], "q-1");
    assert_eq!(asked["status"], "pending");
    let sent = engine.writes(methods::ASK_QUESTION);
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["chatId"], "chat-alpha-1");
    assert_eq!(sent[0]["blocking"], false);
    assert_eq!(sent[0]["options"], json!(["main", "dev"]));
    assert!(
        engine.writes(methods::WATCH_QUESTIONS).is_empty(),
        "nothing waits"
    );
}

#[tokio::test]
async fn a_blocking_question_returns_its_answer() {
    let engine = Engine::with("working", Some(Duration::from_millis(100)));
    let tools = tools_in(&engine, Some("chat-alpha-1"));
    let answered = tools
        .call(
            "ask_question",
            json!({ "question": "Which branch?", "blocking": true }),
        )
        .await
        .unwrap();
    assert_eq!(answered["status"], "answered");
    assert_eq!(answered["answer"], "main");
    assert_eq!(engine.writes(methods::ASK_QUESTION)[0]["blocking"], true);
    assert!(engine.writes(methods::RELEASE_QUESTION).is_empty());
}

#[tokio::test]
async fn a_blocking_question_times_out_and_lets_go_of_the_question() {
    let engine = Engine::new("working");
    let tools = tools_in(&engine, Some("chat-alpha-1"));
    let started = Instant::now();
    let waited = tools
        .call(
            "ask_question",
            json!({ "question": "Which branch?", "blocking": true, "timeout_secs": 1 }),
        )
        .await
        .unwrap();
    assert!(started.elapsed() >= Duration::from_millis(900));
    assert_eq!(waited["status"], "pending");
    assert_eq!(waited["timedOut"], true);
    // From now on an answer is delivered as a message.
    let released = engine.writes(methods::RELEASE_QUESTION);
    assert_eq!(
        released,
        vec![json!({ "questionId": "q-1", "deliver": false, "chatId": "chat-alpha-1" })]
    );
}

#[tokio::test]
async fn a_cancelled_blocking_question_is_released_for_delivery() {
    let engine = Engine::new("working");
    let tools = Arc::new(tools_in(&engine, Some("chat-alpha-1")));
    let call = {
        let tools = tools.clone();
        tokio::spawn(async move {
            tools
                .call(
                    "ask_question",
                    json!({ "question": "Ship?", "blocking": true }),
                )
                .await
        })
    };
    tokio::time::sleep(Duration::from_millis(200)).await;
    call.abort();
    let _ = call.await;
    for _ in 0..50 {
        if !engine.writes(methods::RELEASE_QUESTION).is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(
        engine.writes(methods::RELEASE_QUESTION),
        vec![json!({ "questionId": "q-1", "deliver": true, "chatId": "chat-alpha-1" })]
    );
}

#[tokio::test]
async fn questions_need_a_chat_to_come_back_to() {
    let engine = Engine::new("idle");
    let error = tools_in(&engine, None)
        .call("ask_question", json!({ "question": "Ship?" }))
        .await
        .unwrap_err();
    assert!(
        error.contains("from an agent Zeron launched in a chat"),
        "{error}"
    );
}

#[tokio::test]
async fn waiting_on_a_working_chat_returns_at_once_and_watches() {
    let engine = Engine::new("working");
    let tools = tools_in(&engine, Some("chat-alpha-1"));
    let started = Instant::now();
    let waited = tools
        .call("wait_for_turn", json!({ "chat": "Beta" }))
        .await
        .unwrap();
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(waited["watching"], true);
    assert_eq!(waited["status"], "working");
    let watch = &engine.writes(methods::NOTIFY_TURN_END)[0];
    assert_eq!(watch["chatId"], "chat-beta-2");
    assert_eq!(watch["watcherChatId"], "chat-alpha-1");
    assert_eq!(watch["expectTurn"], false);
    assert_eq!(watch["baseline"]["status"], "working");
}

#[tokio::test]
async fn waiting_on_a_finished_turn_answers_now() {
    let engine = Engine::new("idle");
    let tools = tools_in(&engine, Some("chat-alpha-1"));
    let waited = tools
        .call("wait_for_turn", json!({ "chat": "Beta" }))
        .await
        .unwrap();
    assert_eq!(waited["watching"], false);
    assert_eq!(waited["turn"]["outcome"], "completed");
    assert_eq!(waited["turn"]["replies"][0]["text"], "hello back");
    assert!(engine.writes(methods::NOTIFY_TURN_END).is_empty());
}

#[tokio::test]
async fn a_blocking_wait_still_blocks_and_times_out() {
    let engine = Engine::new("working");
    let tools = tools_in(&engine, Some("chat-alpha-1"));
    let started = Instant::now();
    let waited = tools
        .call(
            "wait_for_turn",
            json!({ "chat": "Beta", "blocking": true, "timeout_secs": 1 }),
        )
        .await
        .unwrap();
    assert!(started.elapsed() >= Duration::from_millis(900));
    assert_eq!(waited["turn"]["outcome"], "timedOut");
    assert!(engine.writes(methods::NOTIFY_TURN_END).is_empty());
}

#[tokio::test]
async fn send_with_notify_watches_from_the_send() {
    let engine = Engine::new("idle");
    let tools = tools_in(&engine, Some("chat-alpha-1"));
    let sent = tools
        .call(
            "send_message",
            json!({ "chat": "Beta", "text": "go", "notify": true }),
        )
        .await
        .unwrap();
    assert_eq!(sent["notify"]["watching"], true);
    let watch = &engine.writes(methods::NOTIFY_TURN_END)[0];
    assert_eq!(watch["chatId"], "chat-beta-2");
    assert_eq!(watch["expectTurn"], true, "a send in flight");
    assert_eq!(watch["baseline"]["lastCompletedTurn"], "turn-1");
    assert_eq!(watch["startMessageIds"], json!(["u1", "a1"]));
    // A later non-blocking wait still knows the send is in flight.
    let waited = tools
        .call("wait_for_turn", json!({ "chat": "Beta" }))
        .await
        .unwrap();
    assert_eq!(waited["watching"], true);
    assert_eq!(
        engine.writes(methods::NOTIFY_TURN_END)[1]["expectTurn"],
        true
    );

    let both = tools
        .call(
            "send_message",
            json!({ "chat": "Beta", "text": "go", "notify": true, "wait": true }),
        )
        .await
        .unwrap_err();
    assert!(both.contains("exclusive"), "{both}");
}

#[tokio::test]
async fn without_a_chat_of_its_own_a_wait_blocks() {
    // Nowhere to deliver a notification: the old blocking behavior.
    let engine = Engine::new("idle");
    let waited = tools_in(&engine, None)
        .call(
            "wait_for_turn",
            json!({ "chat": "Beta", "timeout_secs": 2 }),
        )
        .await
        .unwrap();
    assert_eq!(waited["turn"]["outcome"], "completed");
    assert!(engine.writes(methods::NOTIFY_TURN_END).is_empty());
}
