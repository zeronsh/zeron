//! Adding a project, as the shipping app does (crates/ui shell/spaces.rs
//! `AddSpaceFlow`): a folder on one of your devices, browsed through the
//! engine (no folder dialog, this Mac's included), or a repository cloned
//! into one; and removing one. Each call names the device it's for and
//! whether that's another one (`remote`), which the engine reaches through
//! the relay (`targetDeviceId`).
//!
//!   projects.folders { device, remote, path? }   `{ path, entries = { { name,
//!                                                is_dir, is_repo } }, truncated }`
//!                                                (no `path`: its home)
//!   projects.drives { device, remote }           `{ { name, path } }`
//!   projects.search_home { device, remote, query } `{ matches = { { path,
//!                                                relative, is_repo } }, indexing }`
//!                                                (an empty query warms it)
//!   projects.release_home { device, remote }     its home search let go
//!   projects.clone { device, remote, url, parent } the repository's `{ path,
//!                                                name }`, cloned under `parent`
//!   projects.create { device, path, git, repo? } the new project's id
//!   projects.delete id                           gone, its sessions with it
//!
//! A project of repositories from anywhere on its device, and the device's
//! repositories (`capabilities::COMPOSED_PROJECTS_V1`):
//!
//!   projects.repositories { device, remote, path } the project's repositories:
//!                                                `{ repo, truncated, list = { { path,
//!                                                branch?, empty?, location?, missing?,
//!                                                github? } } }`
//!   projects.compose { device, remote, name, repositories = { { path, name? } },
//!                      parent? }                 its new folder's `{ path }`
//!   projects.set_repositories { device, remote, path, repositories }
//!                                                what it includes now, listed
//!   projects.local { device, remote, refresh? }  `{ list = { { path, name, branch?,
//!                                                empty?, github?, touched?, kept? } },
//!                                                parent, publish, truncated }`
//!   projects.new_repository { device, remote, name, parent?, empty?, publish?,
//!                      private? }               `{ repository, warning? }`
//!   projects.forget_repository { device, remote, path }
//!   projects.add_repository { device, remote, path } `{ path, name }`
//!
//! A refusal comes back as the engine's own words, for the flow to say.

use serde_json::{Value as Json, json};
use zgx_script::{ScriptHost, Value};

use crate::{auth::text, engine};

pub fn install(host: &ScriptHost) {
    host.async_action("projects.folders", |arg| async move {
        let mut params = json!({});
        if let Some(path) = field(&arg, "path") {
            params["path"] = Json::from(path);
        }
        let found = call("ListFolders", params, &arg).await?;
        let entries: Vec<Json> = found["entries"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|e| {
                json!({
                    "name": e["name"],
                    "is_dir": e["isDir"].as_bool().unwrap_or(false),
                    "is_repo": e["isRepo"].as_bool().unwrap_or(false),
                })
            })
            .collect();
        Ok(value(&json!({ "path": found["path"], "entries": entries, "truncated": found["truncated"].as_bool().unwrap_or(false) })))
    });
    host.async_action("projects.drives", |arg| async move {
        let found = call("ListDrives", json!({}), &arg).await?;
        Ok(value(&found["drives"]))
    });
    host.async_action("projects.search_home", |arg| async move {
        let query = field(&arg, "query").unwrap_or_default();
        let found = call("SearchHomeFolders", json!({ "query": query, "limit": 20 }), &arg).await?;
        let matches: Vec<Json> = found["matches"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|m| json!({ "path": m["path"], "relative": m["relative"], "is_repo": m["isRepo"].as_bool().unwrap_or(false) }))
            .collect();
        Ok(value(&json!({ "matches": matches, "indexing": found["indexing"].as_bool().unwrap_or(false) })))
    });
    host.async_action("projects.release_home", |arg| async move {
        let _ = call("ReleaseHomeFolderSearch", json!({}), &arg).await;
        Ok(Value::Nil)
    });
    host.async_action("projects.clone", |arg| { let allowed = crate::plugins::preview::refuse("projects.clone"); async move { allowed?;
        let url = field(&arg, "url").ok_or("Type the repository's address")?;
        let parent = field(&arg, "parent").ok_or("Choose where to clone it")?;
        let repo = call("CloneRepo", json!({ "url": url, "parent": parent }), &arg).await?;
        Ok(value(&json!({ "path": repo["path"], "name": repo["name"] })))
    } });
    host.async_action("projects.create", |arg| async move {
        let device = field(&arg, "device").ok_or("Choose a device")?;
        let path = field(&arg, "path").ok_or("Choose a folder")?;
        let id = crate::sessions::new_id();
        let mut params = json!({
            "op": "createSpace",
            "spaceId": id,
            "deviceId": device,
            "path": path,
            "gitDetected": arg.field("git").is_some_and(Value::truthy),
        });
        if let Some(repo) = field(&arg, "repo") {
            params["githubRepo"] = Json::from(repo);
        }
        if let Some(name) = field(&arg, "name") {
            params["name"] = Json::from(name);
        }
        engine::get().call("Mutate", params).await?;
        Ok(Value::from(id.as_str()))
    });
    host.async_action("projects.repositories", |arg| async move {
        let path = field(&arg, "path").ok_or("Choose a project")?;
        let found = call("ListProjectRepositories", json!({ "path": path }), &arg).await?;
        Ok(value(&project_repositories(&found)))
    });
    host.async_action("projects.compose", |arg| { let allowed = crate::plugins::preview::refuse("projects.compose"); async move {
        allowed?;
        let name = field(&arg, "name").ok_or("Name the project")?;
        let mut params = json!({ "name": name, "repositories": members(&arg) });
        if let Some(parent) = field(&arg, "parent") {
            params["parent"] = Json::from(parent);
        }
        let made = call("ComposeProject", params, &arg).await?;
        Ok(value(&json!({ "path": made["path"] })))
    } });
    host.async_action("projects.set_repositories", |arg| { let allowed = crate::plugins::preview::refuse("projects.set_repositories"); async move {
        allowed?;
        let path = field(&arg, "path").ok_or("Choose a project")?;
        let found = call(
            "SetProjectRepositories",
            json!({ "path": path, "repositories": members(&arg) }),
            &arg,
        )
        .await?;
        Ok(value(&project_repositories(&found)))
    } });
    host.async_action("projects.local", |arg| async move {
        let refresh = arg.field("refresh").is_some_and(Value::truthy);
        let found = call("ListLocalRepositories", json!({ "refresh": refresh }), &arg).await?;
        let list: Vec<Json> = found["repositories"]
            .as_array()
            .into_iter()
            .flatten()
            .map(local_repository)
            .collect();
        Ok(value(&json!({
            "list": list,
            "parent": found["defaultParent"],
            "home": found["home"],
            "publish": found["canPublish"].as_bool().unwrap_or(false),
            "truncated": found["truncated"].as_bool().unwrap_or(false),
        })))
    });
    host.async_action("projects.new_repository", |arg| { let allowed = crate::plugins::preview::refuse("projects.new_repository"); async move {
        allowed?;
        let name = field(&arg, "name").ok_or("Name the repository")?;
        let mut params = json!({ "name": name, "empty": arg.field("empty").is_some_and(Value::truthy) });
        if let Some(parent) = field(&arg, "parent") {
            params["parent"] = Json::from(parent);
        }
        if arg.field("publish").is_some_and(Value::truthy) {
            params["publish"] = json!({ "private": !arg.field("private").is_some_and(|v| !v.truthy()) });
        }
        let made = call("CreateLocalRepository", params, &arg).await?;
        Ok(value(&json!({ "repository": local_repository(&made["repository"]), "warning": made["warning"] })))
    } });
    host.async_action("projects.forget_repository", |arg| { let allowed = crate::plugins::preview::refuse("projects.forget_repository"); async move {
        allowed?;
        let path = field(&arg, "path").ok_or("Choose a repository")?;
        call("ForgetLocalRepository", json!({ "path": path }), &arg).await?;
        Ok(Value::Nil)
    } });
    host.async_action("projects.add_repository", |arg| async move {
        let path = field(&arg, "path").ok_or("Choose a repository")?;
        let repo = call("AddRepo", json!({ "path": path }), &arg).await?;
        Ok(value(
            &json!({ "path": repo["path"], "name": repo["name"] }),
        ))
    });
    host.async_action("projects.delete", |id| { let allowed = crate::plugins::preview::refuse("projects.delete"); async move {
        allowed?;
        let id = text(&id);
        if id.is_empty() {
            return Err("no project".into());
        }
        engine::get()
            .call("Mutate", json!({ "op": "deleteSpace", "spaceId": id }))
            .await?;
        Ok(Value::Nil)
    } });
}

/// `method` on the device `arg` names: through the relay when it's another.
async fn call(method: &str, mut params: Json, arg: &Value) -> Result<Json, String> {
    if arg.field("remote").is_some_and(Value::truthy)
        && let Some(device) = field(arg, "device")
    {
        params["targetDeviceId"] = Json::from(device);
    }
    engine::get().call(method, params).await
}

/// The repositories asked for (`{ { path, name? } }`, or plain paths), as
/// `ProjectMember`s.
fn members(arg: &Value) -> Json {
    let asked: Json = arg
        .field("repositories")
        .map(|v| serde_json::from_str(&v.to_json()).unwrap_or(Json::Null))
        .unwrap_or(Json::Null);
    let list: Vec<Json> = asked
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| match m {
            Json::String(path) => Some(json!({ "path": path })),
            Json::Object(_) => {
                let path = m["path"].as_str().filter(|p| !p.trim().is_empty())?;
                let mut member = json!({ "path": path });
                if let Some(name) = m["name"].as_str().filter(|n| !n.trim().is_empty()) {
                    member["name"] = Json::from(name);
                }
                Some(member)
            }
            _ => None,
        })
        .collect();
    Json::Array(list)
}

/// `ListProjectRepositories`' answer, as the flows read it.
pub(crate) fn project_repositories(found: &Json) -> Json {
    let list: Vec<Json> = found["repositories"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|r| r["linked"].as_bool() != Some(true))
        .filter_map(|r| {
            let path = r["path"].as_str().filter(|p| !p.is_empty())?;
            let mut out = json!({ "path": path });
            for (from, to) in [
                ("branch", "branch"),
                ("location", "location"),
                ("github", "github"),
            ] {
                if let Some(text) = r[from].as_str() {
                    out[to] = Json::from(text);
                }
            }
            if r["hasCommits"].as_bool() != Some(true) {
                out["empty"] = Json::Bool(true);
            }
            if r["missing"].as_bool() == Some(true) {
                out["missing"] = Json::Bool(true);
            }
            Some(out)
        })
        .collect();
    json!({
        "repo": found["isRepo"].as_bool() == Some(true),
        "truncated": found["truncated"].as_bool() == Some(true),
        "list": list,
    })
}

/// One of `ListLocalRepositories`' repositories, as the flows read it.
fn local_repository(r: &Json) -> Json {
    let mut out = json!({ "path": r["path"], "name": r["name"] });
    for (from, to) in [("branch", "branch"), ("github", "github")] {
        if let Some(text) = r[from].as_str() {
            out[to] = Json::from(text);
        }
    }
    if let Some(at) = r["touchedAt"].as_i64() {
        out["touched"] = Json::from(at);
    }
    if r["hasCommits"].as_bool() != Some(true) {
        out["empty"] = Json::Bool(true);
    }
    if r["kept"].as_bool() == Some(true) {
        out["kept"] = Json::Bool(true);
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
    /// A remote device's calls go through the relay, this one's don't.
    #[test]
    fn another_devices_calls_name_it() {
        let arg = |remote: bool| {
            zgx_script::Value::from_json(&format!(r#"{{"device":"box","remote":{remote}}}"#))
                .unwrap()
        };
        let target = |arg: &zgx_script::Value| {
            let mut params = serde_json::json!({});
            if arg.field("remote").is_some_and(zgx_script::Value::truthy)
                && let Some(device) = super::field(arg, "device")
            {
                params["targetDeviceId"] = serde_json::Value::from(device);
            }
            params
        };
        assert_eq!(target(&arg(true))["targetDeviceId"], "box");
        assert!(target(&arg(false)).get("targetDeviceId").is_none());
    }
}
