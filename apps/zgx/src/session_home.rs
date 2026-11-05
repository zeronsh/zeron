//! The panel's home for a session (ui/screens/panel/home): the project's
//! run actions, as the shipping app's session card has them (crates/ui shell/session_info.rs, shell/actions_ui.rs),
//! through the engine's calls, on the device the session runs on.
//!
//!   home.actions { space, device? }               the project's actions:
//!                                                 { actions = { { id, name,
//!                                                 command, icon, setup } },
//!                                                 importable = { { name,
//!                                                 command, icon, setup } },
//!                                                 issue? }
//!   home.save_action { space, device?, id?, name, command, icon, setup }
//!                                                 added (no id) or changed:
//!                                                 the actions, as above
//!   home.delete_action { space, device?, id }     gone: the actions, as above
//!   home.run_action { space, chat, device?, id }  run in a terminal of the
//!                                                 chat's: its terminal's id
//!
//! An engine from before actions answers "unsupported" (the home then
//! leaves the section out); any other refusal is the engine's own words.
//! (The branch's totals are the Git plugin's: core asks nothing of git.)

use serde_json::{Map, Value as Json, json};
use zgx_script::{ScriptHost, Value};

use crate::{auth::text, engine};

pub fn install(host: &ScriptHost) {
    host.async_action("home.actions", |arg| async move {
        let space = field(&arg, "space").ok_or("no project")?;
        let snapshot = call("ListProjectActions", json!({ "spaceId": space }), &arg).await?;
        Ok(value(&snapshot_for_script(&snapshot)))
    });
    host.async_action("home.save_action", |arg| async move { crate::plugins::preview::refuse("home.save_action")?;
        let space = field(&arg, "space").ok_or("no project")?;
        let draft = json!({
            "name": field(&arg, "name").unwrap_or_default(),
            "command": field(&arg, "command").unwrap_or_default(),
            "icon": field(&arg, "icon").unwrap_or_else(|| "play".into()),
            "runOnWorktreeCreate": arg.field("setup").is_some_and(Value::truthy),
        });
        let params = json!({
            "spaceId": space,
            "actionId": field(&arg, "id").map_or(Json::Null, Json::from),
            "action": draft,
        });
        let snapshot = call("UpsertProjectAction", params, &arg).await?;
        Ok(value(&snapshot_for_script(&snapshot)))
    });
    host.async_action("home.delete_action", |arg| async move {
        let space = field(&arg, "space").ok_or("no project")?;
        let id = field(&arg, "id").ok_or("no action")?;
        let snapshot = call("DeleteProjectAction", json!({ "spaceId": space, "actionId": id }), &arg).await?;
        Ok(value(&snapshot_for_script(&snapshot)))
    });
    host.async_action("home.run_action", |arg| async move { crate::plugins::preview::refuse("home.run_action")?;
        let space = field(&arg, "space").ok_or("no project")?;
        let chat = field(&arg, "chat").ok_or("no session")?;
        let id = field(&arg, "id").ok_or("no action")?;
        // (The shipping app's size: the terminal's tab resizes it as it shows.)
        let params = json!({ "spaceId": space, "chatId": chat, "actionId": id, "cols": 80, "rows": 24 });
        let run = call("RunProjectAction", params, &arg).await?;
        let terminal = run["terminal"]["id"].as_str().ok_or("The action's terminal didn't open.")?;
        Ok(Value::from(terminal))
    });
}

/// `method` on the session's device (through the relay when it's another).
async fn call(method: &str, mut params: Json, arg: &Value) -> Result<Json, String> {
    if let Some(device) = crate::files::route_to(field(arg, "device")).await {
        params["targetDeviceId"] = Json::from(device);
    }
    engine::get().call(method, params).await.map_err(said)
}

/// An engine from before the call: "unsupported"; else its own words.
fn said(error: String) -> String {
    let lower = error.to_ascii_lowercase();
    if lower.contains("unknown method") || lower.contains("unknownmethod") {
        "unsupported".into()
    } else {
        error
    }
}

/// The engine's snapshot in the script's words (snake case, `setup` for
/// "run on worktree creation").
fn snapshot_for_script(snapshot: &Json) -> Json {
    let action = |a: &Json, with_id: bool| {
        let mut out = Map::new();
        if with_id {
            out.insert("id".into(), a["id"].clone());
        }
        out.insert("name".into(), a["name"].clone());
        out.insert("command".into(), a["command"].clone());
        out.insert("icon".into(), a["icon"].clone());
        out.insert("setup".into(), Json::Bool(a["runOnWorktreeCreate"].as_bool().unwrap_or(false)));
        Json::Object(out)
    };
    let list = |key: &str, with_id: bool| -> Vec<Json> { snapshot[key].as_array().into_iter().flatten().map(|a| action(a, with_id)).collect() };
    let mut out = json!({
        "actions": list("actions", true),
        "importable": list("importableActions", false),
    });
    if let Some(issue) = snapshot["projectFileIssue"].as_str() {
        out["issue"] = Json::from(issue);
    }
    out
}

fn field(arg: &Value, key: &str) -> Option<String> {
    arg.field(key).map(text).filter(|s| !s.trim().is_empty())
}

fn value(json: &Json) -> Value {
    Value::from_json(&json.to_string()).unwrap_or(Value::Nil)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_snapshot_reaches_the_script_in_its_words() {
        let snapshot = json!({
            "spaceId": "s",
            "actions": [{ "id": "a1", "name": "Dev", "command": "npm run dev", "icon": "play", "runOnWorktreeCreate": true }],
            "importableActions": [{ "name": "Test", "command": "cargo test", "icon": "test", "runOnWorktreeCreate": false }],
            "projectFileIssue": "zeron.json: bad",
        });
        let out = snapshot_for_script(&snapshot);
        assert_eq!(out["actions"][0]["id"], "a1");
        assert_eq!(out["actions"][0]["setup"], true);
        assert!(out["importable"][0].get("id").is_none());
        assert_eq!(out["importable"][0]["icon"], "test");
        assert_eq!(out["issue"], "zeron.json: bad");
        assert!(snapshot_for_script(&json!({ "actions": [], "importableActions": [] })).get("issue").is_none());
    }

    #[test]
    fn an_engine_from_before_actions_says_unsupported() {
        assert_eq!(said("Unknown method: ListProjectActions".into()), "unsupported");
        assert_eq!(said("project not found".into()), "project not found");
    }
}
