//! omp's `extension_ui_request` sub-protocol, answered through Zeron's input
//! requests. Each dialog is answered once (or cancelled); a request omp
//! withdraws (`method: "cancel"`) closes the question without an answer.
//!
//! - Tool approvals arrive as a `select` titled `Allow tool: <name>` with an
//!   approve-once choice and `Deny` (omp's `approvalChoiceLabels`). They are
//!   asked the way every Zeron driver asks (crate::approval: yes/no); yes
//!   approves this call once, anything else denies.
//! - `select`/`confirm`/`input`/`editor` are one question each; `ask` (an
//!   extension's multi-question dialog, after `set_ask_dialog`) is several.
//! - The ask tool's "Other (type your own)" choice is followed by an `input`
//!   with the same title; text typed as the answer to the select picks that
//!   choice and answers the input with it.
//! - Presentation methods (`notify`, `setStatus`, `setWidget`, `setTitle`,
//!   `set_editor_text`, `open_url`) need no answer; the runner shows
//!   notifications that warn or fail.
use crate::pi::rpc::Client;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, PoisonError},
};
use tokio::{sync::oneshot, task::JoinSet};
use zeron_proto::{UserInputAnswer, UserInputQuestion};

pub(super) type Input =
    dyn Fn(Vec<UserInputQuestion>) -> oneshot::Receiver<Vec<UserInputAnswer>> + Send + Sync;

/// The header on every question omp asks that is not an approval.
const HEADER: &str = "omp";
/// omp's approve-this-call labels (`approvalOnceLabel`).
const APPROVE_ONCE: [&str; 2] = ["Approve", "Approve Once"];
const DENY: &str = "Deny";

#[derive(Default)]
pub(super) struct Dialogs {
    pub input: Option<Arc<Input>>,
    tasks: JoinSet<()>,
    /// Dialog ids omp still waits on, with the task answering each; whoever
    /// removes an id answers it, once.
    open: Arc<Mutex<HashMap<String, Option<tokio::task::AbortHandle>>>>,
    /// Text typed for an "Other" choice, waiting for omp's follow-up `input`
    /// (by title).
    other: Arc<Mutex<Option<(String, String)>>>,
}

/// What kind of dialog a request is.
enum Kind {
    Approval { once: String },
    Select,
    Confirm,
    Text { editor: bool },
    Ask,
}

impl Dialogs {
    /// Whether `frame` asks for an answer (the rest are presentation only).
    pub fn interactive(frame: &Value) -> bool {
        matches!(
            frame["method"].as_str(),
            Some("select" | "confirm" | "input" | "editor" | "ask" | "cancel")
        )
    }

    pub fn request(&mut self, client: Client, frame: &Value) {
        while self.tasks.try_join_next().is_some() {}
        let method = frame["method"].as_str().unwrap_or("");
        let Some(id) = frame["id"].as_str().map(str::to_owned) else {
            return;
        };
        if method == "cancel" {
            // omp withdrew the request it names: close the question, and
            // answer nothing (omp has settled it already).
            if let Some(target) = frame["targetId"].as_str()
                && let Some(Some(task)) = self.lock().remove(target)
            {
                task.abort();
            }
            return;
        }
        let options = strings(&frame["options"]);
        let title = frame["title"].as_str().unwrap_or("").to_owned();
        let kind = match method {
            "select" => match approval_once(&title, &options) {
                Some(once) => Kind::Approval { once },
                None => Kind::Select,
            },
            "confirm" => Kind::Confirm,
            "input" => Kind::Text { editor: false },
            "editor" => Kind::Text { editor: true },
            "ask" => Kind::Ask,
            _ => return,
        };
        // The text an "Other" choice was picked with answers its follow-up.
        if method == "input" {
            let typed = {
                let mut other = self.other.lock().unwrap_or_else(PoisonError::into_inner);
                match other.take() {
                    Some((for_title, text)) if for_title == title => Some(text),
                    kept => {
                        *other = kept;
                        None
                    }
                }
            };
            if let Some(text) = typed {
                let _ = client.send(json!({"type":"extension_ui_response","id":id,"value":text}));
                return;
            }
        }
        let Some(input) = self.input.clone() else {
            // Nobody to ask: an approval is refused, anything else declined.
            let _ = client.send(match kind {
                Kind::Approval { .. } => {
                    json!({"type":"extension_ui_response","id":id,"value":DENY})
                }
                _ => cancelled(&id, false),
            });
            return;
        };
        // omp's `timeout` is in milliseconds; a host enforces it and answers
        // `timedOut`. Zero (or none) waits; negative has already expired.
        let timeout = match frame["timeout"].as_f64() {
            Some(ms) if ms < 0.0 => {
                let _ = client.send(cancelled(&id, true));
                return;
            }
            Some(ms) if ms > 0.0 => std::time::Duration::try_from_secs_f64(ms / 1000.0).ok(),
            _ => None,
        };
        let questions = questions(&kind, frame, &title, &options);
        let answers = (input)(questions.clone());
        // Open before the task runs: it may answer at once.
        self.lock().insert(id.clone(), None);
        let open = Arc::clone(&self.open);
        let other = Arc::clone(&self.other);
        let key = id.clone();
        let task = self.tasks.spawn(async move {
            let deadline = async {
                match timeout {
                    Some(timeout) => tokio::time::sleep(timeout).await,
                    None => std::future::pending().await,
                }
            };
            let (answers, timed_out) = tokio::select! {
                result = answers => (result.unwrap_or_default(), false),
                _ = deadline => (vec![], true),
            };
            let response = if timed_out {
                cancelled(&key, true)
            } else {
                respond(&kind, &key, &questions, &answers, &title, &options, &other)
            };
            let mut open = open.lock().unwrap_or_else(PoisonError::into_inner);
            if open.remove(&key).is_some() {
                let _ = client.send(response);
            }
        });
        if let Some(slot) = self.lock().get_mut(&id) {
            *slot = Some(task);
        }
    }

    /// Close every open dialog. The process may live on (a turn ended, a
    /// stop), and omp would wait on a dialog forever: each is answered
    /// `cancelled` first.
    pub fn cancel(&mut self, client: &Client) {
        let open: Vec<(String, Option<tokio::task::AbortHandle>)> = self.lock().drain().collect();
        for (id, task) in open {
            if let Some(task) = task {
                task.abort();
            }
            let _ = client.send(cancelled(&id, false));
        }
        self.tasks.abort_all();
        *self.other.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Option<tokio::task::AbortHandle>>> {
        self.open.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn cancelled(id: &str, timed_out: bool) -> Value {
    let mut response = json!({"type":"extension_ui_response","id":id,"cancelled":true});
    if timed_out {
        response["timedOut"] = json!(true);
    }
    response
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect()
}

/// The approve-once label of an approval prompt, when `title` and `options`
/// are one: `Allow tool: …` first, `Deny` last, an approve-once choice.
fn approval_once(title: &str, options: &[String]) -> Option<String> {
    if !title.starts_with("Allow tool: ") || options.last().map(String::as_str) != Some(DENY) {
        return None;
    }
    options
        .iter()
        .find(|o| APPROVE_ONCE.contains(&o.as_str()))
        .cloned()
}

/// The questions a dialog asks the person.
fn questions(
    kind: &Kind,
    frame: &Value,
    title: &str,
    options: &[String],
) -> Vec<UserInputQuestion> {
    let question = |text: String, options: Vec<String>| UserInputQuestion {
        id: uuid::Uuid::new_v4().to_string(),
        header: HEADER.into(),
        question: text,
        options,
        multi_select: false,
        prefill: None,
        multiline: false,
    };
    match kind {
        Kind::Approval { .. } => vec![approval_question(title)],
        Kind::Select => vec![question(title.into(), options.to_vec())],
        Kind::Confirm => {
            let message = frame["message"].as_str().unwrap_or("");
            let text = if message.is_empty() {
                title.into()
            } else {
                format!("{title}\n{message}")
            };
            vec![question(text, vec!["Yes".into(), "No".into()])]
        }
        Kind::Text { editor } => vec![UserInputQuestion {
            prefill: frame["prefill"].as_str().map(str::to_owned),
            multiline: *editor,
            ..question(title.into(), vec![])
        }],
        Kind::Ask => frame["questions"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|q| UserInputQuestion {
                id: q["id"].as_str().unwrap_or_default().to_owned(),
                header: q["header"]
                    .as_str()
                    .filter(|h| !h.is_empty())
                    .unwrap_or(HEADER)
                    .to_owned(),
                question: q["question"].as_str().unwrap_or_default().to_owned(),
                options: q["options"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|o| o["label"].as_str().map(str::to_owned))
                    .collect(),
                multi_select: q["multi"] == true,
                prefill: None,
                multiline: false,
            })
            .collect(),
    }
}

/// An approval as Zeron asks every agent's: a command, a file change, or a
/// tool, read off omp's prompt (`Allow tool: <name>` and its detail rows,
/// such as `Command: …` or `Path: …`).
fn approval_question(title: &str) -> UserInputQuestion {
    let mut lines = title.lines();
    let tool = lines
        .next()
        .and_then(|l| l.strip_prefix("Allow tool: "))
        .unwrap_or("a tool")
        .trim();
    let details: Vec<&str> = lines.map(str::trim).filter(|l| !l.is_empty()).collect();
    let field = |label: &str| {
        details
            .iter()
            .find_map(|line| line.strip_prefix(label).map(str::trim))
            .filter(|v| !v.is_empty())
    };
    match tool {
        "bash" => crate::approval::command("omp", field("Command:").unwrap_or("")),
        "edit" | "write" | "delete" | "move" | "ast_edit" => match field("Path:") {
            Some(path) => crate::approval::edit("omp", &[path]),
            None => crate::approval::edit("omp", &[]),
        },
        _ => crate::approval::tool(
            "omp",
            tool,
            (!details.is_empty()).then(|| details.join("\n")).as_deref(),
        ),
    }
}

/// omp's answer to a dialog from the person's.
fn respond(
    kind: &Kind,
    id: &str,
    questions: &[UserInputQuestion],
    answers: &[UserInputAnswer],
    title: &str,
    options: &[String],
    other: &Mutex<Option<(String, String)>>,
) -> Value {
    let labels = |question: &UserInputQuestion| -> Vec<String> {
        answers
            .iter()
            .find(|a| a.question_id == question.id)
            .map(|a| a.labels.clone())
            .unwrap_or_default()
    };
    let first = questions.first().map(labels).unwrap_or_default();
    let value = |value: &str| json!({"type":"extension_ui_response","id":id,"value":value});
    match kind {
        Kind::Approval { once } => {
            if questions
                .first()
                .is_some_and(|q| crate::approval::approved(q, answers))
            {
                value(once)
            } else {
                value(DENY)
            }
        }
        Kind::Select => match first.first() {
            Some(pick) if options.contains(pick) => value(pick),
            Some(text) if !text.trim().is_empty() => {
                // Free text: an "Other" choice takes it, through omp's follow-up input.
                match options.iter().find(|o| o.starts_with("Other")) {
                    Some(choice) => {
                        *other.lock().unwrap_or_else(PoisonError::into_inner) =
                            Some((title.to_owned(), text.clone()));
                        value(choice)
                    }
                    None => cancelled(id, false),
                }
            }
            _ => cancelled(id, false),
        },
        Kind::Confirm => match first.first().map(String::as_str) {
            Some("Yes") => json!({"type":"extension_ui_response","id":id,"confirmed":true}),
            Some("No") => json!({"type":"extension_ui_response","id":id,"confirmed":false}),
            _ => cancelled(id, false),
        },
        Kind::Text { .. } => match first.first() {
            Some(text) => value(text),
            None => cancelled(id, false),
        },
        Kind::Ask => {
            // Nothing answered at all: the questions were closed.
            if answers.is_empty() {
                return cancelled(id, false);
            }
            let answers: Vec<Value> = questions
                .iter()
                .map(|question| {
                    let (picked, typed): (Vec<String>, Vec<String>) = labels(question)
                        .into_iter()
                        .partition(|label| question.options.contains(label));
                    let mut answer = json!({"id": question.id, "selectedOptions": picked});
                    let typed = typed.join("\n");
                    if !typed.trim().is_empty() {
                        answer["customInput"] = json!(typed);
                    }
                    answer
                })
                .collect();
            json!({"type":"extension_ui_response","id":id,"answers":answers})
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, BufReader};

    struct Rig {
        client: Client,
        _transport: crate::pi::rpc::Transport,
        replies: tokio::io::Lines<BufReader<tokio::io::DuplexStream>>,
        asked: tokio::sync::mpsc::UnboundedReceiver<(
            Vec<UserInputQuestion>,
            oneshot::Sender<Vec<UserInputAnswer>>,
        )>,
        dialogs: Dialogs,
    }

    fn rig() -> Rig {
        let (writer, peer) = tokio::io::duplex(1 << 16);
        let (_peer_writer, reader) = tokio::io::duplex(1 << 16);
        let transport = crate::pi::rpc::Transport::named(writer, reader, "omp");
        let (bridge, asked) = tokio::sync::mpsc::unbounded_channel();
        let dialogs = Dialogs {
            input: Some(Arc::new(move |questions| {
                let (tx, rx) = oneshot::channel();
                bridge.send((questions, tx)).unwrap();
                rx
            })),
            ..Default::default()
        };
        Rig {
            client: transport.client.clone(),
            _transport: transport,
            replies: BufReader::new(peer).lines(),
            asked,
            dialogs,
        }
    }

    impl Rig {
        async fn reply(&mut self) -> Value {
            let line =
                tokio::time::timeout(std::time::Duration::from_secs(2), self.replies.next_line())
                    .await
                    .expect("a reply")
                    .unwrap()
                    .unwrap();
            serde_json::from_str(&line).unwrap()
        }
        fn request(&mut self, frame: Value) {
            self.dialogs.request(self.client.clone(), &frame);
        }
        async fn ask(
            &mut self,
        ) -> (
            Vec<UserInputQuestion>,
            oneshot::Sender<Vec<UserInputAnswer>>,
        ) {
            self.asked.recv().await.unwrap()
        }
    }

    fn answer(question: &UserInputQuestion, labels: &[&str]) -> Vec<UserInputAnswer> {
        vec![UserInputAnswer {
            question_id: question.id.clone(),
            labels: labels.iter().map(|s| s.to_string()).collect(),
        }]
    }

    /// Recorded from omp 18.20.0 (`--approval-mode always-ask`).
    fn approval() -> Value {
        json!({"type":"extension_ui_request","id":"a1","method":"select",
               "title":"Allow tool: bash\nCommand: echo zeron-probe",
               "options":["Approve","Approve bash for this session","Deny"]})
    }

    #[tokio::test]
    async fn approvals_ask_yes_no_and_answer_approve_once_or_deny() {
        let mut rig = rig();
        rig.request(approval());
        let (questions, tx) = rig.ask().await;
        assert_eq!(questions[0].header, "Approve command");
        assert!(
            questions[0].question.contains("`echo zeron-probe`"),
            "{}",
            questions[0].question
        );
        assert_eq!(questions[0].options, ["Yes", "No"]);
        tx.send(answer(&questions[0], &["Yes"])).unwrap();
        assert_eq!(
            rig.reply().await,
            json!({"type":"extension_ui_response","id":"a1","value":"Approve"})
        );
        rig.request(approval());
        let (questions, tx) = rig.ask().await;
        tx.send(answer(&questions[0], &["sure, why not"])).unwrap();
        assert_eq!(rig.reply().await["value"], "Deny");
        // Closed unanswered: denied, never approved silently.
        rig.request(approval());
        let (_, tx) = rig.ask().await;
        drop(tx);
        assert_eq!(rig.reply().await["value"], "Deny");
        // With nobody to ask, refused without asking.
        rig.dialogs.input = None;
        rig.request(approval());
        assert_eq!(rig.reply().await["value"], "Deny");
        assert!(rig.asked.try_recv().is_err());
    }

    #[test]
    fn approval_questions_name_files_and_tools() {
        let edit = approval_question("Allow tool: write\nPath: src/a.rs");
        assert_eq!(edit.header, "Approve file change");
        assert!(edit.question.contains("src/a.rs"));
        let mcp = approval_question("Allow tool: mcp__db__drop\nOrigin: MCP server tool");
        assert!(mcp.question.contains("mcp__db__drop"), "{}", mcp.question);
        // Not an approval: an extension's own select that merely says Deny.
        assert_eq!(
            approval_once("Pick one", &["Approve".into(), "Deny".into()]),
            None
        );
        assert_eq!(
            approval_once(
                "Allow tool: x",
                &[
                    "Approve Once".into(),
                    "Approve x for this session".into(),
                    "Deny".into()
                ]
            ),
            Some("Approve Once".into())
        );
    }

    #[tokio::test]
    async fn ask_tool_other_choice_carries_typed_text_into_its_follow_up_input() {
        let mut rig = rig();
        // Recorded from omp 18.20.0's ask tool in rpc-ui.
        rig.request(json!({"type":"extension_ui_request","id":"s1","method":"select","title":"What is your favorite color?",
            "options":["Red","Blue","Other (type your own)"],"initialIndex":0,"selectionMarker":"radio"}));
        let (questions, tx) = rig.ask().await;
        assert_eq!(questions[0].header, "omp");
        tx.send(answer(&questions[0], &["teal"])).unwrap();
        assert_eq!(rig.reply().await["value"], "Other (type your own)");
        rig.request(json!({"type":"extension_ui_request","id":"i1","method":"input","title":"What is your favorite color?"}));
        assert_eq!(
            rig.reply().await,
            json!({"type":"extension_ui_response","id":"i1","value":"teal"})
        );
        assert!(
            rig.asked.try_recv().is_err(),
            "the follow-up was not asked again"
        );
    }

    #[tokio::test]
    async fn ask_dialogs_answer_each_question_and_cancel_when_closed() {
        let mut rig = rig();
        let frame = json!({"type":"extension_ui_request","id":"k","method":"ask","questions":[
            {"id":"q1","question":"Which?","options":[{"label":"A"},{"label":"B"}],"multi":true},
            {"id":"q2","question":"Why?","header":"Reason","options":[]}]});
        rig.request(frame.clone());
        let (questions, tx) = rig.ask().await;
        assert!(questions[0].multi_select);
        assert_eq!(questions[1].header, "Reason");
        tx.send(vec![
            UserInputAnswer {
                question_id: "q1".into(),
                labels: vec!["A".into(), "B".into()],
            },
            UserInputAnswer {
                question_id: "q2".into(),
                labels: vec!["because".into()],
            },
        ])
        .unwrap();
        assert_eq!(
            rig.reply().await,
            json!({"type":"extension_ui_response","id":"k","answers":[
                {"id":"q1","selectedOptions":["A","B"]},
                {"id":"q2","selectedOptions":[],"customInput":"because"}]})
        );
        rig.request(frame);
        let (_, tx) = rig.ask().await;
        tx.send(vec![]).unwrap();
        assert_eq!(rig.reply().await["cancelled"], true);
    }

    #[tokio::test]
    async fn omp_cancel_closes_the_question_and_timeouts_report_timed_out() {
        let mut rig = rig();
        rig.request(json!({"type":"extension_ui_request","id":"c1","method":"confirm","title":"Go?","message":"really"}));
        let (questions, mut tx) = rig.ask().await;
        assert_eq!(questions[0].question, "Go?\nreally");
        rig.request(
            json!({"type":"extension_ui_request","id":"x","method":"cancel","targetId":"c1"}),
        );
        tokio::time::timeout(std::time::Duration::from_secs(2), tx.closed())
            .await
            .unwrap();
        rig.request(json!({"type":"extension_ui_request","id":"t1","method":"input","title":"Name","timeout":20}));
        let (_, _tx) = rig.ask().await;
        assert_eq!(
            rig.reply().await,
            json!({"type":"extension_ui_response","id":"t1","cancelled":true,"timedOut":true})
        );
        // The cancelled confirm was never answered.
        rig.request(
            json!({"type":"extension_ui_request","id":"c2","method":"confirm","title":"Again?"}),
        );
        let (questions, tx) = rig.ask().await;
        tx.send(answer(&questions[0], &["No"])).unwrap();
        assert_eq!(
            rig.reply().await,
            json!({"type":"extension_ui_response","id":"c2","confirmed":false})
        );
    }

    #[tokio::test]
    async fn cancel_answers_every_open_dialog_once() {
        let mut rig = rig();
        rig.request(json!({"type":"extension_ui_request","id":"e1","method":"editor","title":"Edit","prefill":"x"}));
        let (questions, tx) = rig.ask().await;
        assert!(questions[0].multiline);
        assert_eq!(questions[0].prefill.as_deref(), Some("x"));
        let client = rig.client.clone();
        rig.dialogs.cancel(&client);
        assert_eq!(
            rig.reply().await,
            json!({"type":"extension_ui_response","id":"e1","cancelled":true})
        );
        let _ = tx.send(answer(&questions[0], &["late"]));
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(100),
                rig.replies.next_line()
            )
            .await
            .is_err(),
            "answered twice"
        );
    }
}
