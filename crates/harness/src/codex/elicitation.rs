//! MCP elicitations (`mcpServer/elicitation/request`): an MCP server asking
//! the person for input in the middle of its tool call, whatever the
//! approval policy. A flat form of choices, yes/no and short text becomes
//! questions for the input bridge; any other shape is declined — a valid
//! answer the server handles, where a JSON-RPC error failed its tool call.

use serde_json::{Map, Value, json};
use zeron_proto::{UserInputAnswer, UserInputQuestion};

enum Kind {
    Text,
    Number {
        integer: bool,
    },
    Boolean,
    /// One of `values`, shown as the matching `labels`.
    Choice {
        values: Vec<String>,
        labels: Vec<String>,
    },
    /// Any of them.
    Choices {
        values: Vec<String>,
        labels: Vec<String>,
    },
}

struct Field {
    key: String,
    required: bool,
    kind: Kind,
    default: Option<Value>,
    question_id: String,
}

/// A form the input bridge can ask; [`Form::response`] turns the answers
/// into the elicitation's reply.
pub(super) struct Form {
    fields: Vec<Field>,
    pub(super) questions: Vec<UserInputQuestion>,
    /// An empty form: a plain confirmation, asked as Accept / Decline.
    confirm_id: Option<String>,
}

pub(super) fn decline() -> Value {
    json!({ "action": "decline" })
}

/// `None` for anything but a flat `form` of supported fields (URL and
/// device-verification modes, nested or free-form schemas).
pub(super) fn form(params: &Value) -> Option<Form> {
    match params.get("mode").and_then(Value::as_str) {
        Some("form") | None => {}
        Some(_) => return None,
    }
    let schema = params.get("requestedSchema")?;
    let properties = match schema.get("properties") {
        Some(Value::Object(properties)) => properties.clone(),
        None | Some(Value::Null) => Map::new(),
        Some(_) => return None,
    };
    let required: Vec<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let server = params
        .get("serverName")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .unwrap_or("MCP server");
    let message = params
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    let header = format!("{server} asks");

    if properties.is_empty() {
        let question = UserInputQuestion {
            id: super::new_message_id(),
            header,
            question: if message.is_empty() {
                format!("{server} asks to continue. Accept?")
            } else {
                message.to_owned()
            },
            options: vec!["Accept".into(), "Decline".into()],
            prefill: None,
            multiline: false,
            multi_select: false,
        };
        return Some(Form {
            fields: Vec::new(),
            confirm_id: Some(question.id.clone()),
            questions: vec![question],
        });
    }

    let mut fields = Vec::new();
    let mut questions = Vec::new();
    for (index, (key, field)) in properties.iter().enumerate() {
        let kind = kind(field)?;
        let title = field
            .get("title")
            .and_then(Value::as_str)
            .filter(|t| !t.is_empty())
            .unwrap_or(key);
        let description = field
            .get("description")
            .and_then(Value::as_str)
            .filter(|d| !d.is_empty());
        let mut text = description.map_or_else(|| title.to_owned(), str::to_owned);
        if index == 0 && !message.is_empty() {
            text = format!("{message}\n\n{text}");
        }
        let default = field.get("default").filter(|d| !d.is_null()).cloned();
        let (options, multi_select) = match &kind {
            Kind::Boolean => (vec!["Yes".into(), "No".into()], false),
            Kind::Choice { labels, .. } => (labels.clone(), false),
            Kind::Choices { labels, .. } => (labels.clone(), true),
            Kind::Text | Kind::Number { .. } => (Vec::new(), false),
        };
        let question = UserInputQuestion {
            id: super::new_message_id(),
            header: header.clone(),
            question: text,
            options,
            prefill: match (&kind, &default) {
                (Kind::Text | Kind::Number { .. }, Some(Value::String(s))) => Some(s.clone()),
                (Kind::Number { .. }, Some(Value::Number(n))) => Some(n.to_string()),
                _ => None,
            },
            multiline: false,
            multi_select,
        };
        fields.push(Field {
            key: key.clone(),
            required: required.contains(&key.as_str()),
            kind,
            default,
            question_id: question.id.clone(),
        });
        questions.push(question);
    }
    Some(Form {
        fields,
        questions,
        confirm_id: None,
    })
}

fn kind(field: &Value) -> Option<Kind> {
    let options = |field: &Value| -> Option<(Vec<String>, Vec<String>)> {
        if let Some(values) = field.get("enum").and_then(Value::as_array) {
            let values: Vec<String> = values
                .iter()
                .map(|v| v.as_str().map(str::to_owned))
                .collect::<Option<_>>()?;
            let labels = field
                .get("enumNames")
                .and_then(Value::as_array)
                .and_then(|names| {
                    names
                        .iter()
                        .map(|n| n.as_str().map(str::to_owned))
                        .collect::<Option<Vec<_>>>()
                })
                .filter(|names| names.len() == values.len())
                .unwrap_or_else(|| values.clone());
            return Some((values, labels));
        }
        let titled = field
            .get("oneOf")
            .or_else(|| field.get("anyOf"))
            .and_then(Value::as_array)?;
        titled
            .iter()
            .map(|option| {
                let value = option.get("const")?.as_str()?.to_owned();
                let label = option
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or(&value)
                    .to_owned();
                Some((value, label))
            })
            .collect::<Option<Vec<_>>>()
            .map(|pairs| pairs.into_iter().unzip())
    };
    Some(match field.get("type").and_then(Value::as_str)? {
        "boolean" => Kind::Boolean,
        "number" => Kind::Number { integer: false },
        "integer" => Kind::Number { integer: true },
        "string" => match options(field) {
            Some((values, labels)) if !values.is_empty() => Kind::Choice { values, labels },
            Some(_) => return None,
            None => Kind::Text,
        },
        "array" => {
            let (values, labels) = options(field.get("items")?)?;
            if values.is_empty() {
                return None;
            }
            Kind::Choices { values, labels }
        }
        _ => return None,
    })
}

impl Form {
    /// The elicitation reply for `answers`. No answer at all (the person
    /// went away), a missing required field or an unreadable one declines.
    pub(super) fn response(&self, answers: &[UserInputAnswer]) -> Value {
        let labels = |id: &str| -> Option<&[String]> {
            answers
                .iter()
                .find(|a| a.question_id == id)
                .map(|a| a.labels.as_slice())
        };
        if let Some(id) = &self.confirm_id {
            let accepted =
                labels(id).is_some_and(|l| l.iter().any(|l| l.eq_ignore_ascii_case("accept")));
            return if accepted {
                json!({ "action": "accept", "content": {} })
            } else {
                decline()
            };
        }
        if answers.is_empty() {
            return decline();
        }
        let mut content = Map::new();
        for field in &self.fields {
            let given: Vec<&str> = labels(&field.question_id)
                .unwrap_or_default()
                .iter()
                .map(|l| l.trim())
                .filter(|l| !l.is_empty())
                .collect();
            let value = match (&field.kind, given.first()) {
                (_, None) => field.default.clone(),
                (Kind::Text, Some(text)) => Some(Value::String((*text).to_owned())),
                (Kind::Number { integer: true }, Some(text)) => match text.parse::<i64>() {
                    Ok(n) => Some(n.into()),
                    Err(_) => return decline(),
                },
                (Kind::Number { integer: false }, Some(text)) => {
                    match text
                        .parse::<f64>()
                        .ok()
                        .and_then(serde_json::Number::from_f64)
                    {
                        Some(n) => Some(Value::Number(n)),
                        None => return decline(),
                    }
                }
                (Kind::Boolean, Some(text)) => Some(Value::Bool(text.eq_ignore_ascii_case("yes"))),
                (Kind::Choice { values, labels }, Some(text)) => {
                    match labels.iter().position(|l| l == text) {
                        Some(at) => Some(Value::String(values[at].clone())),
                        None => return decline(),
                    }
                }
                (Kind::Choices { values, labels }, Some(_)) => {
                    let mut chosen = Vec::new();
                    for text in &given {
                        match labels.iter().position(|l| l == text) {
                            Some(at) => chosen.push(Value::String(values[at].clone())),
                            None => return decline(),
                        }
                    }
                    Some(Value::Array(chosen))
                }
            };
            match value {
                Some(value) => {
                    content.insert(field.key.clone(), value);
                }
                None if field.required => return decline(),
                None => {}
            }
        }
        json!({ "action": "accept", "content": content })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(form: &Form, labels: &[&[&str]]) -> Vec<UserInputAnswer> {
        form.questions
            .iter()
            .zip(labels)
            .map(|(q, l)| UserInputAnswer {
                question_id: q.id.clone(),
                labels: l.iter().map(|s| (*s).to_owned()).collect(),
            })
            .collect()
    }

    #[test]
    fn flat_forms_round_trip_typed_content() {
        let params = json!({
            "serverName": "deploy", "threadId": "t", "mode": "form",
            "message": "Pick a target",
            "requestedSchema": {"type": "object", "required": ["env", "count"], "properties": {
                "env": {"type": "string", "oneOf": [{"const": "prod", "title": "Production"}, {"const": "dev", "title": "Development"}]},
                "count": {"type": "integer", "title": "Replicas"},
                "force": {"type": "boolean"},
                "tags": {"type": "array", "items": {"type": "string", "enum": ["a", "b"]}},
                "note": {"type": "string", "default": "none"}
            }}
        });
        let form = form(&params).expect("supported form");
        assert_eq!(form.questions.len(), 5);
        // The server's message leads the first question.
        assert!(form.questions[0].question.starts_with("Pick a target\n\n"));
        assert!(
            form.questions[1..]
                .iter()
                .all(|q| !q.question.contains("Pick a target"))
        );
        let by_key = |key: &str| {
            form.fields
                .iter()
                .position(|f| f.key == key)
                .map(|at| form.questions[at].id.clone())
                .unwrap()
        };
        let answers = vec![
            UserInputAnswer {
                question_id: by_key("env"),
                labels: vec!["Development".into()],
            },
            UserInputAnswer {
                question_id: by_key("count"),
                labels: vec![" 3 ".into()],
            },
            UserInputAnswer {
                question_id: by_key("force"),
                labels: vec!["Yes".into()],
            },
            UserInputAnswer {
                question_id: by_key("tags"),
                labels: vec!["a".into(), "b".into()],
            },
        ];
        assert_eq!(
            form.response(&answers),
            json!({"action": "accept", "content": {
                "env": "dev", "count": 3, "force": true, "tags": ["a", "b"], "note": "none"
            }})
        );
        // A required field left empty, or an unreadable number, declines.
        assert_eq!(form.response(&answers[..1]), decline());
        let mut bad = answers.clone();
        bad[1].labels = vec!["three".into()];
        assert_eq!(form.response(&bad), decline());
        // Nobody answered.
        assert_eq!(form.response(&[]), decline());
    }

    #[test]
    fn empty_forms_are_confirmations() {
        let form = form(
            &json!({"serverName": "s", "message": "Proceed?", "mode": "form",
            "requestedSchema": {"type": "object", "properties": {}}}),
        )
        .unwrap();
        assert_eq!(form.questions[0].options, vec!["Accept", "Decline"]);
        assert_eq!(form.questions[0].question, "Proceed?");
        assert_eq!(
            form.response(&answer(&form, &[&["Accept"]])),
            json!({"action": "accept", "content": {}})
        );
        assert_eq!(form.response(&answer(&form, &[&["Decline"]])), decline());
        assert_eq!(form.response(&[]), decline());
    }

    #[test]
    fn unsupported_modes_and_schemas_are_not_asked() {
        assert!(
            form(&json!({"mode": "url", "url": "https://x", "message": "m", "elicitationId": "e"}))
                .is_none()
        );
        assert!(form(&json!({"mode": "openai/userVerification", "challenge": "c"})).is_none());
        assert!(
            form(
                &json!({"mode": "form", "message": "m", "requestedSchema": {"type": "object",
            "properties": {"nested": {"type": "object"}}}})
            )
            .is_none()
        );
    }
}
