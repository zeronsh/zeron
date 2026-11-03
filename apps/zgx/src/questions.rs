//! What agents ask the person without stopping for it (the engine's
//! `ask_question`): open questions, each on its chat, kept in the
//! workspace's registry, so every device shows them (`WatchQuestions`). An
//! open question doesn't put its chat in "waiting on you": the agent keeps
//! working, and the engine sends it the answer when one comes.
//!
//! What scripts see, `questions`:
//!   { state = "loading" | "ready" | "unsupported",
//!     list = { { id, chat, question, options = { string }, context?,
//!                status = "pending" | "answered" | "dismissed",
//!                answer?, created, resolved? } } }
//! oldest first: every open one, and each chat's last few settled ones
//! (a withdrawn one isn't listed). `created` and `resolved` are Unix
//! seconds.
//!
//! and call, each rejecting with the engine's words (or "offline"):
//!   questions.answer({ id, answer })   the agent is sent the answer
//!   questions.dismiss(id)              closed unanswered; the agent is told

use std::{cell::RefCell, collections::HashMap, rc::Rc};

use serde_json::{Value as Json, json};
use zgx_desktop::WindowContext;
use zgx_script::{ScriptHost, Shared, Value};

use crate::auth::text;
use crate::engine::{self, rpc::Watch};

/// How many settled questions each chat keeps showing (collapsed).
const SETTLED_KEPT: usize = 5;

#[derive(Default)]
struct Questions {
    shared: RefCell<Vec<(u64, Shared)>>,
    next_window: std::cell::Cell<u64>,
    watching: std::cell::Cell<bool>,
    state: RefCell<&'static str>,
    list: RefCell<Json>,
    watch: RefCell<Option<Watch>>,
}

thread_local! {
    static QUESTIONS: Rc<Questions> = Rc::new(Questions { state: RefCell::new("loading"), ..Default::default() });
}

pub fn install(cx: &mut WindowContext, host: &ScriptHost) {
    let q = QUESTIONS.with(Rc::clone);
    let window = q.next_window.get();
    q.next_window.set(window + 1);
    q.shared.borrow_mut().push((window, host.shared("questions", q.value())));
    let me = Rc::downgrade(&q);
    cx.on_closed(move || {
        if let Some(q) = me.upgrade() {
            q.shared.borrow_mut().retain(|(w, _)| *w != window);
        }
    });
    if !q.watching.replace(true) {
        let weak = Rc::downgrade(&q);
        engine::get().on_connect(move |rpc| {
            let Some(q) = weak.upgrade() else { return };
            let (heard, ended) = (Rc::downgrade(&q), Rc::downgrade(&q));
            let watch = rpc.watch_ending(
                "WatchQuestions",
                json!({}),
                move |item| {
                    if let Some(q) = heard.upgrade() {
                        *q.state.borrow_mut() = "ready";
                        *q.list.borrow_mut() = item;
                        q.publish();
                    }
                },
                move |why| {
                    // An engine from before questions: nothing to show.
                    if let Some(q) = ended.upgrade() {
                        if unknown_method(&why) {
                            *q.state.borrow_mut() = "unsupported";
                            q.publish();
                        }
                    }
                },
            );
            *q.watch.borrow_mut() = Some(watch);
        });
    }
    host.async_action("questions.answer", |args| async move {
        let id = args.field("id").map(text).unwrap_or_default();
        let answer = args.field("answer").map(text).unwrap_or_default();
        if id.is_empty() || answer.trim().is_empty() {
            return Err("empty".to_string());
        }
        let params = json!({ "questionId": id, "answer": answer.trim() });
        engine::get().call("AnswerQuestion", params).await.map(|_| Value::Nil)
    });
    host.async_action("questions.dismiss", |id| async move {
        let params = json!({ "questionId": text(&id) });
        engine::get().call("DismissQuestion", params).await.map(|_| Value::Nil)
    });
}

fn unknown_method(why: &str) -> bool {
    let lower = why.to_ascii_lowercase();
    lower.contains("unknown method") || lower.contains("unknownmethod")
}

impl Questions {
    fn publish(&self) {
        let value = self.value();
        for (_, shared) in self.shared.borrow().iter() {
            shared.set(value.clone());
        }
    }

    fn value(&self) -> Value {
        let list = for_script(&self.list.borrow());
        let json = json!({ "state": *self.state.borrow(), "list": list });
        Value::from_json(&json.to_string()).unwrap_or(Value::Nil)
    }
}

/// The engine's questions in the scripts' words: open ones, and each
/// chat's last [`SETTLED_KEPT`] settled ones, oldest first.
fn for_script(all: &Json) -> Vec<Json> {
    let mut rows: Vec<(i64, Json)> = Vec::new();
    for q in all.as_array().into_iter().flatten() {
        let (Some(id), Some(chat)) = (q["id"].as_str(), q["chatId"].as_str()) else { continue };
        let status = q["status"].as_str().unwrap_or("pending");
        if !matches!(status, "pending" | "answered" | "dismissed") {
            continue;
        }
        let created = q["createdAt"].as_str().and_then(crate::sessions::unix_seconds).unwrap_or(0);
        let mut row = json!({
            "id": id,
            "chat": chat,
            "question": q["question"].as_str().unwrap_or_default(),
            "options": q["options"].as_array().map(|o| o.iter().filter_map(Json::as_str).collect::<Vec<_>>()).unwrap_or_default(),
            "status": status,
            "created": created,
        });
        if let Some(context) = q["context"].as_str().filter(|c| !c.trim().is_empty()) {
            row["context"] = json!(context);
        }
        if let Some(answer) = q["answer"].as_str() {
            row["answer"] = json!(answer);
        }
        if let Some(at) = q["resolvedAt"].as_str().and_then(crate::sessions::unix_seconds) {
            row["resolved"] = json!(at);
        }
        rows.push((created, row));
    }
    rows.sort_by_key(|(at, _)| *at);
    // Each chat's settled ones, newest kept.
    let mut settled: HashMap<String, usize> = HashMap::new();
    let mut keep = vec![true; rows.len()];
    for (i, (_, row)) in rows.iter().enumerate().rev() {
        if row["status"] != "pending" {
            let n = settled.entry(row["chat"].as_str().unwrap_or_default().to_owned()).or_default();
            *n += 1;
            keep[i] = *n <= SETTLED_KEPT;
        }
    }
    rows.into_iter().zip(keep).filter(|(_, k)| *k).map(|((_, row), _)| row).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(id: &str, chat: &str, status: &str, at: &str) -> Json {
        json!({ "id": id, "chatId": chat, "question": "Which?", "options": ["A", "B"], "context": null,
                "blocking": false, "status": status, "answer": if status == "answered" { json!("A") } else { Json::Null },
                "createdAt": at, "resolvedAt": null, "resolvedBy": null, "deliveredAt": null })
    }

    #[test]
    fn questions_reach_the_scripts_oldest_first_without_withdrawn_ones() {
        let out = for_script(&json!([
            q("q2", "c1", "pending", "2026-11-03T10:05:00Z"),
            q("q1", "c1", "answered", "2026-11-03T10:00:00Z"),
            q("q3", "c2", "cancelled", "2026-11-03T10:06:00Z"),
        ]));
        let ids: Vec<&str> = out.iter().map(|r| r["id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["q1", "q2"]);
        assert_eq!(out[0]["answer"], "A");
        assert_eq!(out[1]["options"], json!(["A", "B"]));
        assert_eq!(out[1]["chat"], "c1");
        assert!(out[1].get("context").is_none());
    }

    #[test]
    fn each_chat_keeps_only_its_last_few_settled_questions() {
        let mut all = vec![q("open", "c1", "pending", "2026-11-03T09:00:00Z")];
        for i in 0..8 {
            all.push(q(&format!("s{i}"), "c1", "answered", &format!("2026-11-03T10:0{i}:00Z")));
        }
        let out = for_script(&Json::Array(all));
        let ids: Vec<&str> = out.iter().map(|r| r["id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["open", "s3", "s4", "s5", "s6", "s7"]);
    }

    #[test]
    fn an_engine_from_before_questions_is_told_apart() {
        assert!(unknown_method("unknown method: WatchQuestions"));
        assert!(!unknown_method("closed"));
    }
}
