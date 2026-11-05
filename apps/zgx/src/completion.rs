//! What the composer's `/` and `$` offer: an agent's slash commands and
//! skills for a chat's (or project's) folder on its device, as the shipping
//! composer finds them (crates/ui/src/composer.rs: `ListCommands` and
//! `ListSkills`, merged by `invocation_candidates`), each with the canonical
//! reference a pick puts in the prompt (`zeron_proto::invocation`'s
//! `Invocation::link`), which the engine reads at send.
//!
//!   chat.invocations { harness, target = { chat? | space?, device?, path? } }
//!       { rows = { { kind = "command" | "skill", name, label, detail, link } },
//!         warning? }     or an error when neither list could be had
//!
//! Commands come first in the agent's order, then its enabled skills; a
//! command that is a skill's own alias is left to the skill. Names and paths
//! the canonical reference couldn't carry are left out.

use std::{cell::RefCell, future::Future, pin::Pin, rc::Rc, task::Poll};

use serde_json::{Map, Value as Json, json};
use zgx_script::{ScriptHost, Value};

use crate::engine;

pub fn install(host: &ScriptHost) {
    let local: Rc<RefCell<Option<String>>> = Rc::default();
    host.async_action("chat.invocations", move |arg| {
        let local = local.clone();
        async move { invocations(arg, local).await }
    });
}

fn text(value: Option<&Value>) -> String {
    value.map(crate::auth::text).unwrap_or_default()
}

/// This engine's device (a target there needs no `targetDeviceId`).
async fn local_device(cache: &RefCell<Option<String>>) -> Option<String> {
    if let Some(id) = cache.borrow().clone() {
        return Some(id);
    }
    let info = engine::get().call("EngineInfo", json!({})).await.ok()?;
    let id = info["deviceId"].as_str()?.to_string();
    *cache.borrow_mut() = Some(id.clone());
    Some(id)
}

async fn invocations(arg: Value, local: Rc<RefCell<Option<String>>>) -> Result<Value, String> {
    let harness = text(arg.field("harness"));
    if harness.is_empty() {
        return Err("no agent".into());
    }
    let target = arg.field("target").ok_or("no target")?;
    let mut params = Map::new();
    params.insert("harness".into(), Json::String(harness));
    let chat = text(target.field("chat"));
    let space = text(target.field("space"));
    let path = text(target.field("path"));
    if !chat.is_empty() {
        params.insert("chatId".into(), Json::String(chat));
    } else if !space.is_empty() {
        params.insert("spaceId".into(), Json::String(space));
    } else if !path.is_empty() {
        params.insert("path".into(), Json::String(path));
    } else {
        return Err("no session or project".into());
    }
    let device = text(target.field("device"));
    if !device.is_empty() && local_device(&local).await.is_none_or(|here| here != device) {
        params.insert("targetDeviceId".into(), Json::String(device));
    }
    let params = Json::Object(params);
    let (commands, skills) = join(
        engine::get().call("ListCommands", params.clone()),
        engine::get().call("ListSkills", params),
    )
    .await;
    let (rows, warning) = merge(commands, skills)?;
    let mut fields = vec![("rows", Value::List(rows.into_iter().map(row_value).collect::<Vec<_>>().into()))];
    if let Some(warning) = warning {
        fields.push(("warning", Value::from(warning.as_str())));
    }
    Ok(Value::record(fields))
}

/// Both futures, run together.
async fn join<A, B>(a: impl Future<Output = A>, b: impl Future<Output = B>) -> (A, B) {
    let mut a: Pin<Box<dyn Future<Output = A> + '_>> = Box::pin(a);
    let mut b: Pin<Box<dyn Future<Output = B> + '_>> = Box::pin(b);
    let (mut ra, mut rb) = (None, None);
    std::future::poll_fn(move |cx| {
        if ra.is_none()
            && let Poll::Ready(v) = a.as_mut().poll(cx)
        {
            ra = Some(v);
        }
        if rb.is_none()
            && let Poll::Ready(v) = b.as_mut().poll(cx)
        {
            rb = Some(v);
        }
        if ra.is_some() && rb.is_some() {
            Poll::Ready((ra.take().unwrap(), rb.take().unwrap()))
        } else {
            Poll::Pending
        }
    })
    .await
}

#[derive(Debug, Clone, PartialEq)]
struct Row {
    kind: &'static str,
    name: String,
    label: String,
    detail: String,
    link: String,
}

fn row_value(row: Row) -> Value {
    Value::record([
        ("kind", Value::from(row.kind)),
        ("name", Value::from(row.name.as_str())),
        ("label", Value::from(row.label.as_str())),
        ("detail", Value::from(row.detail.as_str())),
        ("link", Value::from(row.link.as_str())),
    ])
}

/// The shipping composer's `merge_invocation_results`: both lists, or the one
/// that came with a word about the other, or why neither came.
fn merge(commands: Result<Json, String>, skills: Result<Json, String>) -> Result<(Vec<Row>, Option<String>), String> {
    let commands = commands.map(|v| v.as_array().cloned().unwrap_or_default());
    // (`null`: the agent has no skills to list.)
    let skills = skills.map(|v| v.as_array().cloned());
    match (commands, skills) {
        (Ok(commands), Ok(skills)) => Ok((candidates(&commands, &skills.unwrap_or_default()), None)),
        (Ok(commands), Err(_)) if !commands.is_empty() => {
            Ok((candidates(&commands, &[]), Some("Skills couldn't be listed".into())))
        }
        (Err(_), Ok(Some(skills))) if skills.iter().any(|s| s["enabled"].as_bool() == Some(true)) => {
            Ok((candidates(&[], &skills), Some("Commands couldn't be listed".into())))
        }
        (Err(error), _) | (_, Err(error)) => Err(error),
    }
}

/// `zeron_proto::invocation`'s name and path rules: what the canonical
/// reference carries unchanged.
fn valid_name(name: &str) -> bool {
    !name.is_empty() && !name.chars().any(|c| c.is_control() || c.is_whitespace())
}
fn valid_path(path: &str) -> bool {
    !path.is_empty() && !path.chars().any(char::is_control)
}
fn valid_command_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | ':' | '.'))
}
fn native_skill(path: &str) -> bool {
    path.starts_with("opencode-skill:") || path.starts_with("harness-skill:")
}

/// A skill's name as people read it: its last `:` part, its words
/// capitalized (`code-review` → "Code Review").
fn display_name(name: &str) -> String {
    name.rsplit(':')
        .next()
        .unwrap_or(name)
        .split(|c: char| c == '-' || c == '_' || c.is_whitespace())
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut chars = w.chars();
            let first = chars.next().unwrap();
            first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// `Invocation::link`: `[/name](zeron-invoke:<hex of its JSON>)`.
fn link(prefix: char, name: &str, invocation: &Json) -> String {
    let bytes = serde_json::to_vec(invocation).unwrap_or_default();
    let payload: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    let label = name.replace('\\', "\\\\").replace('[', "\\[").replace(']', "\\]").replace('`', "\\`");
    format!("[{prefix}{label}](zeron-invoke:{payload})")
}

/// The shipping composer's `invocation_candidates`.
fn candidates(commands: &[Json], skills: &[Json]) -> Vec<Row> {
    let skills: Vec<&Json> = skills
        .iter()
        .filter(|s| {
            valid_name(s["name"].as_str().unwrap_or(""))
                && valid_path(s["path"].as_str().unwrap_or(""))
                && s["command"].as_object().is_none_or(|c| valid_command_name(c.get("name").and_then(Json::as_str).unwrap_or("")))
        })
        .collect();
    let aliases: std::collections::HashSet<&str> =
        skills.iter().filter_map(|s| s["command"]["name"].as_str()).collect();
    let mut rows = Vec::new();
    for c in commands {
        let name = c["name"].as_str().unwrap_or("");
        if !valid_name(name) || aliases.contains(name) {
            continue;
        }
        let mut detail = c["description"].as_str().unwrap_or("").to_string();
        if let Some(hint) = c["inputHint"].as_str().filter(|h| !h.is_empty()) {
            detail = if detail.is_empty() { format!("<{hint}>") } else { format!("{detail} · <{hint}>") };
        }
        rows.push(Row {
            kind: "command",
            name: name.to_string(),
            label: format!("/{name}"),
            detail,
            link: link('/', name, &json!({ "kind": "command", "name": name })),
        });
    }
    for s in skills {
        if s["enabled"].as_bool() != Some(true) {
            continue;
        }
        let name = s["name"].as_str().unwrap_or("");
        let path = s["path"].as_str().unwrap_or("");
        let description = s["description"].as_str().unwrap_or("");
        let mut invocation = json!({ "kind": "skill", "name": name, "path": path });
        if s["command"].is_object() {
            invocation["command"] = s["command"].clone();
        }
        rows.push(Row {
            kind: "skill",
            name: name.to_string(),
            label: display_name(name),
            detail: if native_skill(path) || description.is_empty() {
                description.to_string()
            } else {
                format!("{description} — {path}")
            },
            link: link('$', name, &invocation),
        });
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(link: &str) -> Json {
        let hex = link.split("zeron-invoke:").nth(1).unwrap().trim_end_matches(')');
        let bytes: Vec<u8> = (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap()).collect();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[test]
    fn commands_then_enabled_skills_a_skills_alias_left_to_it() {
        let commands = vec![
            json!({ "name": "review", "description": "Review the diff", "inputHint": "branch" }),
            json!({ "name": "deploy", "description": "" }),
            json!({ "name": "bad name", "description": "spaces" }),
        ];
        let skills = vec![
            json!({ "name": "code-review", "path": "/s/code-review/SKILL.md", "description": "Careful review", "enabled": true, "command": { "name": "deploy", "harness": "claude-code" } }),
            json!({ "name": "off", "path": "/s/off/SKILL.md", "description": "", "enabled": false }),
            json!({ "name": "native", "path": "harness-skill:native", "description": "Built in", "enabled": true }),
        ];
        let rows = candidates(&commands, &skills);
        let names: Vec<_> = rows.iter().map(|r| (r.kind, r.label.as_str())).collect();
        assert_eq!(names, vec![("command", "/review"), ("skill", "Code Review"), ("skill", "Native")]);
        assert_eq!(rows[0].detail, "Review the diff · <branch>");
        assert_eq!(rows[1].detail, "Careful review — /s/code-review/SKILL.md");
        assert_eq!(rows[2].detail, "Built in");
    }

    #[test]
    fn a_pick_carries_the_canonical_reference_the_engine_reads() {
        let rows = candidates(
            &[json!({ "name": "review" })],
            &[json!({ "name": "a[b]", "path": "/s/x.md", "description": "", "enabled": true })],
        );
        assert!(rows[0].link.starts_with("[/review](zeron-invoke:"));
        assert_eq!(decode(&rows[0].link), json!({ "kind": "command", "name": "review" }));
        assert!(rows[1].link.starts_with("[$a\\[b\\]](zeron-invoke:"));
        assert_eq!(decode(&rows[1].link), json!({ "kind": "skill", "name": "a[b]", "path": "/s/x.md" }));
    }

    /// Real lists from a live engine, when `ZGX_LIVE_INVOCATIONS` names a
    /// folder of `ListCommands-<harness>.json` and `ListSkills-<harness>.json`
    /// (an isolated engine's answers): every row merges, and every link reads
    /// back as the invocation it was made from.
    #[test]
    fn live_lists_merge_and_their_links_read_back() {
        let Some(dir) = std::env::var_os("ZGX_LIVE_INVOCATIONS") else {
            return;
        };
        let dir = std::path::PathBuf::from(dir);
        for harness in ["claude-code", "codex"] {
            let read = |m: &str| -> Json {
                serde_json::from_slice(&std::fs::read(dir.join(format!("{m}-{harness}.json"))).unwrap()).unwrap()
            };
            let (rows, warning) = merge(Ok(read("ListCommands")), Ok(read("ListSkills"))).unwrap();
            assert!(warning.is_none());
            let commands = rows.iter().filter(|r| r.kind == "command").count();
            let skills = rows.iter().filter(|r| r.kind == "skill").count();
            eprintln!("{harness}: {commands} commands, {skills} skills; first {:?}", rows.first().map(|r| (&r.label, &r.detail)));
            assert!(commands + skills > 0, "{harness} listed nothing");
            for r in &rows {
                let back = decode(&r.link);
                assert_eq!(back["name"], json!(r.name), "{}", r.link);
                assert_eq!(back["kind"], json!(r.kind));
            }
        }
    }

    #[test]
    fn one_list_failing_still_shows_the_other() {
        let commands = Ok(json!([{ "name": "review" }]));
        let (rows, warning) = merge(commands, Err("down".into())).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(warning.as_deref(), Some("Skills couldn't be listed"));
        assert!(merge(Err("down".into()), Ok(Json::Null)).is_err());
        let (rows, warning) = merge(Ok(json!([])), Ok(Json::Null)).unwrap();
        assert!(rows.is_empty() && warning.is_none());
    }
}
