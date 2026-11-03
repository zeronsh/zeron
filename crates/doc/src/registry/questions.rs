//! `questions` rows: what agents asked the person without stopping
//! ([`zeron_proto::questions`]). Any device writes them, field by field LWW
//! like every row: the asking chat's engine creates one, whichever device the
//! person answers on resolves it.

use super::*;
use zeron_proto::{AgentQuestion, QuestionStatus};

pub const KIND_QUESTIONS: &str = "questions";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawQuestion {
    id: String,
    chat_id: String,
    question: String,
    #[serde(default)]
    options: Vec<String>,
    #[serde(default)]
    context: Option<String>,
    #[serde(default)]
    blocking: bool,
    #[serde(default)]
    status: QuestionStatus,
    #[serde(default)]
    answer: Option<String>,
    #[serde(default)]
    created_at: Option<i64>,
    #[serde(default)]
    resolved_at: Option<i64>,
    #[serde(default)]
    resolved_by: Option<String>,
    #[serde(default)]
    delivered_at: Option<i64>,
}

fn at(ms: Option<i64>) -> Option<DateTime<Utc>> {
    ms.and_then(DateTime::from_timestamp_millis)
}

impl From<RawQuestion> for AgentQuestion {
    fn from(raw: RawQuestion) -> Self {
        AgentQuestion {
            id: raw.id,
            chat_id: raw.chat_id,
            question: raw.question,
            options: raw.options,
            context: raw.context,
            blocking: raw.blocking,
            status: raw.status,
            answer: raw.answer,
            created_at: at(raw.created_at).unwrap_or(DateTime::UNIX_EPOCH),
            resolved_at: at(raw.resolved_at),
            resolved_by: raw.resolved_by,
            delivered_at: at(raw.delivered_at),
        }
    }
}

impl RegistryDoc {
    /// Write a new question row (all of it).
    pub fn upsert_question(&mut self, q: &AgentQuestion) -> Result<(), DocError> {
        let set = fields([
            ("id", json!(q.id)),
            ("chatId", json!(q.chat_id)),
            ("question", json!(q.question)),
            ("options", json!(q.options)),
            ("context", opt_str(q.context.as_deref())),
            ("blocking", json!(q.blocking)),
            ("status", serde_json::to_value(q.status)?),
            ("answer", opt_str(q.answer.as_deref())),
            ("createdAt", json!(q.created_at.timestamp_millis())),
            ("resolvedAt", opt_ms(q.resolved_at)),
            ("resolvedBy", opt_str(q.resolved_by.as_deref())),
            ("deliveredAt", opt_ms(q.delivered_at)),
        ]);
        // Checked here, not left to the room: an oversized op is refused
        // with its whole batch, which then wedges every write behind it.
        let probe = RowOp {
            kind: KIND_QUESTIONS.into(),
            id: q.id.clone(),
            op: OpKind::Upsert,
            set: Some(set.clone()),
            hlc: encode_hlc(0, 0, self.device_id()),
            clocks: None,
        };
        if !fits_op_budget(&probe) {
            return Err(DocError::Schema("question too large".into()));
        }
        self.write(KIND_QUESTIONS, &q.id, OpKind::Upsert, set);
        Ok(())
    }

    pub fn question(&self, id: &str) -> Option<AgentQuestion> {
        self.overlay_row(KIND_QUESTIONS, id)
            .and_then(|row| row_to::<RawQuestion>(&row))
            .map(AgentQuestion::from)
    }

    /// Every question, oldest first.
    pub fn read_questions(&self) -> Vec<AgentQuestion> {
        let mut out: Vec<AgentQuestion> = self
            .read_kind::<RawQuestion>(KIND_QUESTIONS)
            .into_iter()
            .map(AgentQuestion::from)
            .collect();
        out.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
        out
    }

    /// Close an open question. `None` when there's no such question; the
    /// question as it stands, unchanged, when it was no longer open.
    pub fn resolve_question(
        &mut self,
        id: &str,
        status: QuestionStatus,
        answer: Option<&str>,
        at: DateTime<Utc>,
    ) -> Option<(AgentQuestion, bool)> {
        let current = self.question(id)?;
        if !current.status.is_open() || status.is_open() {
            return Some((current, false));
        }
        let device = self.device_id().to_owned();
        self.write(
            KIND_QUESTIONS,
            id,
            OpKind::Update,
            fields([
                (
                    "status",
                    serde_json::to_value(status).unwrap_or(Value::Null),
                ),
                ("answer", opt_str(answer)),
                ("resolvedAt", json!(at.timestamp_millis())),
                ("resolvedBy", json!(device)),
            ]),
        );
        Some((self.question(id)?, true))
    }

    /// Stamp the answer as delivered into the asking chat.
    pub fn set_question_delivered(&mut self, id: &str, at: DateTime<Utc>) -> bool {
        if !self.row_exists(KIND_QUESTIONS, id) {
            return false;
        }
        self.write(
            KIND_QUESTIONS,
            id,
            OpKind::Update,
            fields([("deliveredAt", json!(at.timestamp_millis()))]),
        );
        true
    }

    /// The agent stopped waiting inside its tool call: from now on an answer
    /// is delivered as a message.
    pub fn set_question_blocking(&mut self, id: &str, blocking: bool) -> bool {
        if !self.row_exists(KIND_QUESTIONS, id) {
            return false;
        }
        self.write(
            KIND_QUESTIONS,
            id,
            OpKind::Update,
            fields([("blocking", json!(blocking))]),
        );
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn question(id: &str) -> AgentQuestion {
        AgentQuestion {
            id: id.into(),
            chat_id: "chat-1".into(),
            question: "Which branch?".into(),
            options: vec!["main".into(), "dev".into()],
            context: Some("ctx".into()),
            blocking: false,
            status: QuestionStatus::Pending,
            answer: None,
            created_at: DateTime::from_timestamp_millis(1_000).unwrap(),
            resolved_at: None,
            resolved_by: None,
            delivered_at: None,
        }
    }

    #[test]
    fn questions_round_trip_and_resolve_once() {
        let mut doc = RegistryDoc::new("dev-a");
        doc.set_local_only(true);
        doc.upsert_question(&question("q-1")).unwrap();
        assert_eq!(doc.read_questions(), vec![question("q-1")]);
        let now = Utc::now();
        let (answered, changed) = doc
            .resolve_question("q-1", QuestionStatus::Answered, Some("main"), now)
            .unwrap();
        assert!(changed);
        assert_eq!(answered.status, QuestionStatus::Answered);
        assert_eq!(answered.answer.as_deref(), Some("main"));
        assert_eq!(answered.resolved_by.as_deref(), Some("dev-a"));
        // Closed questions stay closed.
        let (again, changed) = doc
            .resolve_question("q-1", QuestionStatus::Dismissed, None, now)
            .unwrap();
        assert!(!changed);
        assert_eq!(again.status, QuestionStatus::Answered);
        assert!(
            doc.resolve_question("nope", QuestionStatus::Answered, None, now)
                .is_none()
        );
        assert!(doc.set_question_delivered("q-1", now));
        assert!(doc.question("q-1").unwrap().delivered_at.is_some());
    }

    #[test]
    fn questions_sync_to_another_replica_as_ops() {
        let mut a = RegistryDoc::new("dev-a");
        a.upsert_question(&question("q-2")).unwrap();
        let pushed = a.take_pushable();
        assert_eq!(pushed.len(), 1);
        assert_eq!(pushed[0].ops[0].kind, KIND_QUESTIONS);
        // The server stores the row; another device reads it from there.
        let (row, _) = apply_op(None, &pushed[0].ops[0]);
        let mut b = RegistryDoc::new("dev-b");
        assert!(b.apply_rows(1, vec![row.unwrap()]));
        assert_eq!(b.read_questions(), vec![question("q-2")]);
        let (answered, changed) = b
            .resolve_question("q-2", QuestionStatus::Answered, Some("dev"), Utc::now())
            .unwrap();
        assert!(changed && answered.resolved_by.as_deref() == Some("dev-b"));
    }

    #[test]
    fn an_oversized_question_is_refused_before_it_can_wedge_the_queue() {
        let mut doc = RegistryDoc::new("dev-a");
        let mut q = question("q-3");
        q.question = "x".repeat(20_000);
        assert!(doc.upsert_question(&q).is_err());
        assert!(doc.read_questions().is_empty());
    }
}
