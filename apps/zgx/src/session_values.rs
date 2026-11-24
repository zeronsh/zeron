//! Parts of a chat's session row as scripts see them (src/sessions.rs): what
//! its agent is doing, how far through its todo list it is, and the usage
//! limit that stopped it. Snake_case keys, times in Unix seconds, a field that
//! isn't there nil.

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value as Json;
use zeron_proto::orchestration::{ChatActivity, TodoProgress};
use zeron_proto::usage_limit::SessionLimit;
use zgx_script::Value;

/// What a chat's agent is doing: `{ kind, label, since }`.
pub(crate) fn activity_value(a: &ChatActivity) -> Value {
    Value::record([("kind", word(&a.kind)), ("label", Value::from(a.label.as_str())), ("since", secs(&a.since))])
}

/// How far through its todo list a chat is: `{ done, total, current? }`.
pub(crate) fn todo_value(t: &TodoProgress) -> Value {
    Value::record([
        ("done", Value::from(f64::from(t.done))),
        ("total", Value::from(f64::from(t.total))),
        ("current", opt_str(t.current.as_deref())),
    ])
}

/// A chat its provider's usage limit stopped:
/// `{ provider, account_slot?, account_label?, window?, until_at?, since,
/// attempt, switched_to? = { account_slot, account_label, at, harness?,
/// model? } }` (`until_at`, `until` on the wire, nil while unknown: `until`
/// is a Luau keyword).
pub(crate) fn limited_value(l: &SessionLimit) -> Value {
    let switched = l.switched_to.as_ref().map_or(Value::Nil, |s| {
        Value::record([
            ("account_slot", Value::from(s.account_slot.as_str())),
            ("account_label", Value::from(s.account_label.as_str())),
            ("at", secs(&s.at)),
            ("harness", opt_str(s.harness.as_deref())),
            ("model", opt_str(s.model.as_deref())),
        ])
    });
    Value::record([
        ("provider", Value::from(l.provider.as_str())),
        ("account_slot", opt_str(l.account_slot.as_deref())),
        ("account_label", opt_str(l.account_label.as_deref())),
        ("window", opt_str(l.window.as_deref())),
        ("until_at", l.until.as_ref().map_or(Value::Nil, secs)),
        ("since", secs(&l.since)),
        ("attempt", Value::from(f64::from(l.attempt))),
        ("switched_to", switched),
    ])
}

/// An enum's wire word in snake_case (`runningTests` → `running_tests`).
fn word<T: Serialize>(v: &T) -> Value {
    match serde_json::to_value(v) {
        Ok(Json::String(s)) => {
            let mut out = String::with_capacity(s.len() + 4);
            for c in s.chars() {
                if c.is_ascii_uppercase() {
                    out.push('_');
                    out.push(c.to_ascii_lowercase());
                } else {
                    out.push(c);
                }
            }
            Value::from(out.as_str())
        }
        _ => Value::Nil,
    }
}

fn secs(t: &DateTime<Utc>) -> Value {
    Value::from(t.timestamp() as f64)
}

fn opt_str(v: Option<&str>) -> Value {
    v.map_or(Value::Nil, Value::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const NOW: i64 = 1_794_470_400;

    fn iso(secs: i64) -> String {
        DateTime::<Utc>::from_timestamp(secs, 0).unwrap().to_rfc3339()
    }

    fn text(v: Option<&Value>) -> Option<String> {
        v.map(crate::auth::text)
    }

    fn num(v: Option<&Value>) -> Option<f64> {
        v.and_then(Value::as_f64)
    }

    fn nil(v: Option<&Value>) -> bool {
        v.is_none_or(|v| matches!(v, Value::Nil))
    }

    #[test]
    fn activity_and_todo_read_as_session_rows_carry_them() {
        let a: ChatActivity =
            serde_json::from_value(json!({ "kind": "testing", "label": "cargo test", "since": iso(NOW - 48) })).unwrap();
        let v = activity_value(&a);
        assert_eq!(text(v.field("kind")).as_deref(), Some("testing"));
        assert_eq!(text(v.field("label")).as_deref(), Some("cargo test"));
        assert_eq!(num(v.field("since")), Some((NOW - 48) as f64));
        let t = todo_value(&TodoProgress { done: 3, total: 3, current: None });
        assert_eq!((num(t.field("done")), num(t.field("total"))), (Some(3.), Some(3.)));
        assert!(nil(t.field("current")));
    }

    #[test]
    fn a_limited_chat_reaches_scripts_snake_cased() {
        let waiting: SessionLimit = serde_json::from_value(json!({
            "provider": "claude", "accountLabel": "me@work.dev", "window": "5-hour",
            "until": iso(NOW + 1800), "since": iso(NOW - 60), "attempt": 1
        }))
        .unwrap();
        let v = limited_value(&waiting);
        assert_eq!(text(v.field("provider")).as_deref(), Some("claude"));
        assert_eq!(text(v.field("account_label")).as_deref(), Some("me@work.dev"));
        assert_eq!(num(v.field("until_at")), Some((NOW + 1800) as f64));
        assert_eq!(num(v.field("since")), Some((NOW - 60) as f64));
        assert_eq!(num(v.field("attempt")), Some(1.0));
        assert!(nil(v.field("switched_to")));
        assert!(nil(v.field("account_slot")));

        let switched: SessionLimit = serde_json::from_value(json!({
            "provider": "claude", "since": iso(NOW - 60),
            "switchedTo": { "accountSlot": "s2", "accountLabel": "me@home.dev", "at": iso(NOW - 30), "model": "opus" }
        }))
        .unwrap();
        let v = limited_value(&switched);
        assert!(nil(v.field("until_at")));
        let to = v.field("switched_to").unwrap();
        assert_eq!(text(to.field("account_slot")).as_deref(), Some("s2"));
        assert_eq!(text(to.field("account_label")).as_deref(), Some("me@home.dev"));
        assert_eq!(num(to.field("at")), Some((NOW - 30) as f64));
        assert_eq!(text(to.field("model")).as_deref(), Some("opus"));
        assert!(nil(to.field("harness")));
    }
}
