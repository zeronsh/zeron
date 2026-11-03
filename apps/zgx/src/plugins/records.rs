//! What each plugin may read of Zeron, kept for it alone as
//! `plugin.<id>.records`: the window gives plugins ids (`session = <id>`
//! where a session is), and a plugin's SDK finds the record by its id here.
//! A record holds only the fields the plugin was allowed. Those it wasn't
//! are absent, never an error, and no record of the app's own ever reaches a
//! plugin as it is.
//!
//! | grant | what it adds |
//! |---|---|
//! | `sessions.read` | each session's title, status, project, device, times, its git branch and whether it's in a worktree of its own |
//! | `messages.read` | each message's role and text, in the open chat |
//! | `profile.read` | your id, name, username and avatar |
//! | `account.email` | your email, unless you hide email addresses |
//! | `files.read` | file paths (`files_read`; the SDK gives an id of the plugin's own without) |
//! | `messages.read`, or "ui.change" on a reply's blocks | a code block's source and a formula's TeX (`blocks_read`) |
//!
//! What an element gives plugins of a file or a block is a token
//! (`tokens`), found through the plugin's own `plugin.<id>.file` and
//! `plugin.<id>.block` as these say.

use serde_json::{Map, Value as Json, json};

/// What records are made from, as the window has it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sources {
    /// The sessions bridge's (`sessions`): `{ chats = { … } }`.
    pub sessions: Json,
    /// The open chat's messages (`chat.messages`).
    pub messages: Json,
    /// You, as the app composes it (`plugins.profile`): `{ id, name,
    /// username, avatar, email }`.
    pub profile: Json,
    /// Whether email addresses are hidden (Settings → General).
    pub hide_emails: bool,
}

fn pick(from: &Json, keys: &[&str]) -> Json {
    let mut out = Map::new();
    for k in keys {
        if let Some(v) = from.get(*k).filter(|v| !v.is_null()) {
            out.insert((*k).to_string(), v.clone());
        }
    }
    Json::Object(out)
}

/// The records for a plugin allowed `permissions`.
pub fn records(permissions: &[String], sources: &Sources) -> Json {
    let may = |p: &str| permissions.iter().any(|x| x == p);
    let mut out = Map::new();
    if may("sessions.read") {
        let mut sessions = Map::new();
        // Each project's folder: a session elsewhere is in a worktree of its
        // own (said as yes or no; the path is files.read's).
        let folder = |project: &Json| {
            sources.sessions["projects"].as_array().into_iter().flatten().find(|p| p["id"] == *project).and_then(|p| p["path"].as_str())
        };
        for chat in sources.sessions["chats"].as_array().into_iter().flatten() {
            if let Some(id) = chat["id"].as_str() {
                let mut record = pick(chat, &["title", "status", "project", "device", "updated", "created", "branch", "source_branch"]);
                if let (Some(cwd), Some(path)) = (chat["cwd"].as_str(), folder(&chat["project"])) {
                    record["worktree"] = json!(cwd.trim_end_matches('/') != path.trim_end_matches('/'));
                }
                // Its folder, with "files.read" (a path is a file's path).
                if may("files.read")
                    && let Some(cwd) = chat["cwd"].as_str()
                {
                    record["cwd"] = json!(cwd);
                }
                sessions.insert(id.to_string(), record);
            }
        }
        out.insert("sessions".into(), Json::Object(sessions));
        // Each project by its name (never its path).
        let mut projects = Map::new();
        for project in sources.sessions["projects"].as_array().into_iter().flatten() {
            if let Some(id) = project["id"].as_str() {
                let mut record = pick(project, &["name", "device"]);
                if may("files.read")
                    && let Some(path) = project["path"].as_str()
                {
                    record["path"] = json!(path);
                }
                projects.insert(id.to_string(), record);
            }
        }
        out.insert("projects".into(), Json::Object(projects));
    }
    if may("messages.read") {
        let mut messages = Map::new();
        for message in sources.messages.as_array().into_iter().flatten() {
            if let Some(id) = message["id"].as_str() {
                messages.insert(id.to_string(), pick(message, &["role", "text"]));
            }
        }
        out.insert("messages".into(), Json::Object(messages));
    }
    let mut profile = Map::new();
    if may("profile.read")
        && let Json::Object(fields) = pick(&sources.profile, &["id", "name", "username", "avatar"])
    {
        profile.extend(fields);
    }
    if may("account.email")
        && !sources.hide_emails
        && let Some(email) = sources.profile["email"].as_str()
    {
        profile.insert("email".into(), json!(email));
    }
    out.insert("profile".into(), Json::Object(profile));
    if may("files.read") {
        out.insert("files_read".into(), json!(true));
    }
    if may("messages.read") {
        out.insert("blocks_read".into(), json!(true));
    }
    Json::Object(out)
}

/// The records for a plugin allowed `permissions`, with these "ui.change"
/// targets: a plugin allowed to change how a reply's code blocks or
/// formulas show (`.md-code…`, `.md-math`) reads what they hold, as it
/// draws them.
pub fn records_with(permissions: &[String], change: &[String], sources: &Sources) -> Json {
    let mut out = records(permissions, sources);
    let changes_blocks = permissions.iter().any(|p| p == "ui.change")
        && change.iter().any(|t| {
            let last = t.rsplit(">>").next().unwrap_or(t).trim();
            last.starts_with(".md-code") || last.starts_with(".md-math")
        });
    if changes_blocks {
        out["blocks_read"] = json!(true);
    }
    out
}

/// Whether any of `permissions` reads the open chat's messages (so the
/// window's messages are followed only for a plugin that may read them).
pub fn reads_messages(permissions: &[String]) -> bool {
    permissions.iter().any(|p| p == "messages.read")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sources(hide: bool) -> Sources {
        Sources {
            sessions: json!({
                "chats": [{ "id": "s1", "title": "Fix the build", "status": "working", "project": "p1", "device": "d1", "updated": 5, "cwd": "/secret/path/.worktrees/fix", "branch": "fix/build" }],
                "projects": [{ "id": "p1", "path": "/secret/path", "name": "Atlas", "device": "d1" }],
            }),
            messages: json!([{ "id": "m1", "role": "user", "text": "hello", "attachments": [] }]),
            profile: json!({ "id": "u1", "name": "Ada", "username": "ada", "email": "ada@example.com" }),
            hide_emails: hide,
        }
    }

    fn p(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn nothing_allowed_reads_nothing() {
        assert_eq!(records(&[], &sources(false)), json!({ "profile": {} }));
    }

    #[test]
    fn each_grant_adds_its_fields_and_only_those() {
        let r = records(&p(&["sessions.read"]), &sources(false));
        assert_eq!(
            r["sessions"]["s1"],
            json!({ "title": "Fix the build", "status": "working", "project": "p1", "device": "d1", "updated": 5, "branch": "fix/build", "worktree": true })
        );
        assert!(r["sessions"]["s1"].get("cwd").is_none(), "a path isn't a session's to give");
        assert_eq!(r["projects"]["p1"], json!({ "name": "Atlas", "device": "d1" }), "a project's name, not its path");
        assert!(r.get("messages").is_none());
        // With "files.read", folders: a session's and its project's.
        let r = records(&p(&["sessions.read", "files.read"]), &sources(false));
        assert_eq!((r["sessions"]["s1"]["cwd"].as_str(), r["projects"]["p1"]["path"].as_str()), (Some("/secret/path/.worktrees/fix"), Some("/secret/path")));
        let r = records(&p(&["messages.read"]), &sources(false));
        assert_eq!(r["messages"]["m1"], json!({ "role": "user", "text": "hello" }));
        let r = records(&p(&["profile.read"]), &sources(false));
        assert_eq!(r["profile"], json!({ "id": "u1", "name": "Ada", "username": "ada" }));
        let r = records(&p(&["files.read"]), &sources(false));
        assert_eq!(r["files_read"], true);
    }

    #[test]
    fn email_is_its_own_grant_and_hiding_emails_keeps_it() {
        let r = records(&p(&["profile.read", "account.email"]), &sources(false));
        assert_eq!(r["profile"]["email"], "ada@example.com");
        let hidden = records(&p(&["profile.read", "account.email"]), &sources(true));
        assert!(hidden["profile"].get("email").is_none());
        let no_grant = records(&p(&["profile.read"]), &sources(false));
        assert!(no_grant["profile"].get("email").is_none());
    }
}
