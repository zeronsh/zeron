//! `zeron plugin check|publish|status|key|yank|unyank|withdraw`: the plugin
//! store from the command line.
//!
//! Two ways, one model (the plan's "Who may publish"):
//! - **In CI**, with a deploy key's secret in `ZERON_DEPLOY_KEY` (never an
//!   argument, so it stays out of process lists and shell history): no Zeron
//!   running, no sign-in. The folder is checked, signed with the deploy key
//!   and sent, the request signed with it too (`store::client`).
//! - **On your machine**, without it: through the running Zeron, your own
//!   plugin signed with this device's key and sent with your session
//!   (`PreparePluginPublish`, then `PublishPlugin` with the ticket it gave).
//!
//! `check` needs neither. Failures say one line each and exit with a code a
//! CI job can act on ([`code`]).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Value, json};
use zeron_cli::{KeyCommand, PluginCommand};
use zeron_engine::plugins::install;
use zeron_engine::plugins::store::client::{Caller, StoreClient};
use zeron_engine::plugins::store::deploy_key::{self, DeployKey};
use zeron_engine::plugins::store::publish::{self, Prepared};
use zeron_engine::plugins::{PLUGINS_DIR, manifest};
use zeron_rpc::methods;

/// A command that ends with an exit code of its own, and one line saying why.
#[derive(Debug)]
pub struct Exit {
    pub code: i32,
    pub message: String,
}

impl std::fmt::Display for Exit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Exit {}

/// The plugin doesn't meet the store's rules (or the store said so).
pub const RULES: i32 = 1;
/// Usage or set-up: no such folder, no key where one is needed, a damaged key.
pub const SETUP: i32 = 2;
/// The store refused the key or the account (not allowed, signed out,
/// someone else's id, no username yet, suspended).
pub const REFUSED: i32 = 3;
/// The store can't be reached, or is busy: try again later.
pub const UNREACHABLE: i32 = 4;

fn exit(code: i32, message: impl Into<String>) -> anyhow::Error {
    Exit { code, message: message.into() }.into()
}

/// The exit code for a failure's word (`"<word>: <words>"`).
pub fn code(word: &str) -> i32 {
    match word {
        "not_semver" | "version_exists" | "not_newer" | "missing_readme" | "missing_listing" | "bad_listing" | "bad_package" | "bad_manifest"
        | "too_large" | "too_big" | "id_reserved" | "changed" | "not_a_package" | "unsigned" | "tampered" | "bad_file" => RULES,
        // (A request already made, or changed on its way: sending it again
        // is the fix.)
        "store_unreachable" | "rate_limited" | "store_error" | "unavailable" | "body_mismatch" | "replayed_request" => UNREACHABLE,
        // (The runner's clock is more than five minutes out.)
        "no_manifest" | "invalid" | "not_a_secret" | "damaged" | "stale_request" => SETUP,
        _ => REFUSED,
    }
}

/// A failure from the store or the engine, as an exit.
fn failed(error: &str) -> anyhow::Error {
    let word = install::reason_of(error).unwrap_or("");
    exit(if word.is_empty() { REFUSED } else { code(word) }, error.to_owned())
}

/// The deploy key in `ZERON_DEPLOY_KEY`, when one is set (an empty variable
/// is none: CI systems set a missing secret to "").
fn deploy_key() -> anyhow::Result<Option<DeployKey>> {
    match std::env::var(deploy_key::ENV) {
        Ok(secret) if !secret.trim().is_empty() => DeployKey::from_secret(&secret)
            .map(Some)
            .map_err(|why| exit(SETUP, format!("{}: {why}", deploy_key::ENV))),
        _ => Ok(None),
    }
}

/// Run `command` here when it needs no running Zeron: `check` always, and
/// with `ZERON_DEPLOY_KEY` set, `publish` and `status`: all a deploy key may
/// do (contract amendment 6), so a leaked one can add a pending version,
/// which review stops, and nothing else. `Ok(Some(command))` hands it back
/// for the running Zeron and its signed-in session.
pub async fn standalone(command: PluginCommand, data_dir: &Path) -> anyhow::Result<Option<PluginCommand>> {
    if let PluginCommand::Check { target } = &command {
        let prepared = read(&folder(target, data_dir)?)?;
        say(&prepared);
        if !prepared.ok() {
            return Err(exit(RULES, problems_line(&prepared)));
        }
        println!("{} {} meets the store's rules.", prepared.manifest.id, prepared.manifest.version);
        return Ok(None);
    }
    let needs_store = matches!(command, PluginCommand::Publish { .. } | PluginCommand::Status { .. });
    if !needs_store {
        return Ok(Some(command));
    }
    let Some(key) = deploy_key()? else { return Ok(Some(command)) };
    let key = Arc::new(key);
    let store = StoreClient::new(&zeron_cli::edge_url_from_env());
    let caller = Caller::DeployKey(key.clone());
    match command {
        PluginCommand::Publish { target, dry_run } => {
            let mut prepared = read(&folder(&target, data_dir)?)?;
            // Newer than what the store has (a deploy key reads its plugin's
            // state), asked only of a plugin that meets the rules here; the
            // store has the last word if it can't be asked.
            let asked = if prepared.ok() { Some(store.my_plugins(&caller).await) } else { None };
            match asked.unwrap_or_else(|| Err(String::new())) {
                Ok(mine) => {
                    let versions = mine["plugins"].as_array().into_iter().flatten().find(|p| p["id"] == prepared.manifest.id.as_str()).map(|p| p["versions"].clone());
                    if let Some(Value::Array(versions)) = versions
                        && let Some(problem) = publish::newer_problem(&prepared.manifest.version, &versions)
                    {
                        prepared.problems.push(problem);
                    }
                }
                Err(error) if !error.is_empty() => {
                    eprintln!("note: couldn't ask the store which versions it has: it has the last word on whether this one is newer")
                }
                Err(_) => {}
            }
            say(&prepared);
            if !prepared.ok() {
                return Err(exit(RULES, problems_line(&prepared)));
            }
            let raw = prepared.pack(key.signing_key()).map_err(|e| failed(&e))?;
            let what = format!("{} {}", prepared.manifest.id, prepared.manifest.version);
            if dry_run {
                println!("{what} is ready: {} files, {}, signed with deploy key {}. Not sent (--dry-run).", prepared.files.len(), size(raw.len() as u64), key.public());
                return Ok(None);
            }
            let reply = store.publish(&prepared.manifest.id, raw, &caller).await.map_err(|e| failed(&e))?;
            submitted(&what, &reply);
        }
        PluginCommand::Status { id } => print_status(&store.my_plugins(&caller).await.map_err(|e| failed(&e))?, id.as_deref()),
        _ => unreachable!("only publish and status get here"),
    }
    Ok(None)
}

/// Why `command` can't run without the running Zeron, when it can't: one
/// line for a CI log.
pub fn needs_zeron(command: &PluginCommand) -> String {
    let what = match command {
        PluginCommand::Yank { .. } => "yank",
        PluginCommand::Unyank { .. } => "unyank",
        PluginCommand::Withdraw { .. } => "withdraw",
        PluginCommand::Key { .. } => "key",
        _ => "publish",
    };
    if what != "publish" && std::env::var(deploy_key::ENV).is_ok_and(|s| !s.trim().is_empty()) {
        format!(
            "`zeron plugin {what}` needs Zeron running here and signed in: a deploy key only publishes and reads status ({} can't do this)",
            deploy_key::ENV
        )
    } else if what == "publish" {
        format!("Zeron isn't running here. From CI, set {} to a deploy key's secret (`zeron plugin key create`).", deploy_key::ENV)
    } else {
        format!("`zeron plugin {what}` needs Zeron running here and signed in. Open it, or start it with `zeron headless`.")
    }
}

fn acted(id: &str, version: &str, action: &str, reply: &Value) -> String {
    let done = match action {
        "yank" => "yanked: no new installs",
        "unyank" => "back in the store",
        _ => "withdrawn from review",
    };
    match reply["status"].as_str() {
        Some(status) => format!("{id} {version} {done} ({status})."),
        None => format!("{id} {version} {done}."),
    }
}

/// The plugin's folder: `target` as a path, or the id of a folder in
/// Zeron's plugins folder.
fn folder(target: &str, data_dir: &Path) -> anyhow::Result<PathBuf> {
    let path = Path::new(target);
    if path.is_dir() {
        return Ok(path.to_path_buf());
    }
    if manifest::check_id(target).is_ok() {
        let own = data_dir.join(PLUGINS_DIR).join(target);
        if own.is_dir() {
            return Ok(own);
        }
    }
    Err(exit(SETUP, format!("{target}: there's no plugin folder there")))
}

fn read(dir: &Path) -> anyhow::Result<Prepared> {
    publish::read(dir).map_err(|e| exit(if install::reason_of(&e) == Some("no_manifest") { SETUP } else { RULES }, e))
}

/// Every problem and warning, one line each, on stderr.
fn say(prepared: &Prepared) {
    for line in publish::lines(&prepared.warnings) {
        eprintln!("warning: {line}");
    }
    for line in publish::lines(&prepared.problems) {
        eprintln!("error: {line}");
    }
}

fn problems_line(prepared: &Prepared) -> String {
    let n = prepared.problems.len();
    format!(
        "{} {} doesn't meet the store's rules ({n} problem{}): fix {} and try again",
        prepared.manifest.id,
        prepared.manifest.version,
        if n == 1 { "" } else { "s" },
        if n == 1 { "it" } else { "them" }
    )
}

fn submitted(what: &str, reply: &Value) {
    let status = reply["status"].as_str().unwrap_or("pending");
    println!("Submitted {what}: {status}. It's reviewed before anyone else sees it; your own devices are offered it now.");
}

/// `zeron plugin status`'s lines: each plugin, then each version's state.
fn print_status(reply: &Value, only: Option<&str>) {
    let plugins: Vec<&Value> = reply["plugins"].as_array().into_iter().flatten().filter(|p| only.is_none_or(|id| p["id"] == id)).collect();
    if plugins.is_empty() {
        println!("{}", if only.is_some() { "That plugin isn't yours in the store." } else { "You have no plugins in the store yet." });
    }
    for plugin in plugins {
        println!("{}", plugin["id"].as_str().unwrap_or("?"));
        for version in plugin["versions"].as_array().into_iter().flatten() {
            let mut line = format!("  {}  {}", version["version"].as_str().unwrap_or("?"), version["status"].as_str().unwrap_or("?"));
            if let Some(reason) = version["reason"].as_str().or(version["message"].as_str()).filter(|r| !r.is_empty()) {
                line.push_str(&format!(": {reason}"));
            }
            println!("{line}");
        }
    }
}

/// `512 bytes`, `12.3 KB`, `4.1 MB`.
fn size(bytes: u64) -> String {
    match bytes {
        0..1024 => format!("{bytes} bytes"),
        1024..1_048_576 => format!("{:.1} KB", bytes as f64 / 1024.0),
        _ => format!("{:.1} MB", bytes as f64 / 1_048_576.0),
    }
}

// ── Through the running Zeron ───────────────────────────────────────────────

/// `method` asked of the running Zeron: its failure as an exit.
async fn ask(client: &zeron_rpc::RpcClient, method: &'static str, params: Value) -> anyhow::Result<Value> {
    client.call(method, params).await.map_err(|error| match error {
        zeron_rpc::RpcError::Failed(message) => failed(&message),
        zeron_rpc::RpcError::UnknownMethod(_) => exit(SETUP, "This Zeron is too old for the store. Update it first."),
        other => exit(UNREACHABLE, other.to_string()),
    })
}

/// The store's commands through the running Zeron (with your session).
pub async fn through_zeron(command: PluginCommand, data_dir: &Path, client: &zeron_rpc::RpcClient) -> anyhow::Result<()> {
    let call = |method: &'static str, params: Value| ask(client, method, params);
    match command {
        PluginCommand::Publish { target, dry_run } => {
            let id = own_id(&target, data_dir)?;
            let prepared = call(methods::PREPARE_PLUGIN_PUBLISH, json!({ "id": id })).await?;
            for (kind, key) in [("warning", "warnings"), ("error", "problems")] {
                for p in prepared[key].as_array().into_iter().flatten() {
                    let at = p["path"].as_str().or(p["field"].as_str()).map(|a| format!(" ({a})")).unwrap_or_default();
                    eprintln!("{kind}: {}{at}: {}", p["rule"].as_str().unwrap_or("?"), p["words"].as_str().unwrap_or(""));
                }
            }
            let what = format!("{id} {}", prepared["version"].as_str().unwrap_or(""));
            if prepared["ok"] != true {
                let n = prepared["problems"].as_array().map_or(0, Vec::len);
                return Err(exit(RULES, format!("{what} doesn't meet the store's rules ({n} problem{}): fix and try again", if n == 1 { "" } else { "s" })));
            }
            if dry_run {
                println!("{what} is ready to publish. Not sent (--dry-run).");
                return Ok(());
            }
            let reply = call(methods::PUBLISH_PLUGIN, json!({ "id": id, "hash": prepared["hash"], "ticket": prepared["ticket"] })).await?;
            answered(&reply)?;
            submitted(&what, &reply);
        }
        PluginCommand::Status { id } => print_status(&call(methods::STORE_MY_PLUGINS, json!({})).await?, id.as_deref()),
        PluginCommand::Yank { id, version } => action(client, &id, &version, "yank").await?,
        PluginCommand::Unyank { id, version } => action(client, &id, &version, "unyank").await?,
        PluginCommand::Withdraw { id, version } => action(client, &id, &version, "withdraw").await?,
        PluginCommand::Key { command } => key(client, command).await?,
        _ => unreachable!("only the store's commands get here"),
    }
    Ok(())
}

/// `{ok: false, error, message}` as an exit.
fn answered(reply: &Value) -> anyhow::Result<()> {
    if reply["ok"] == false {
        let word = reply["error"].as_str().unwrap_or("failed");
        return Err(exit(code(word), format!("{word}: {}", reply["message"].as_str().unwrap_or(""))));
    }
    Ok(())
}

async fn action(client: &zeron_rpc::RpcClient, id: &str, version: &str, action: &str) -> anyhow::Result<()> {
    let reply = ask(client, methods::STORE_VERSION_ACTION, json!({ "id": id, "version": version, "action": action })).await?;
    answered(&reply)?;
    println!("{}", acted(id, version, action, &reply));
    Ok(())
}

/// The id of your plugin `target` names: an id, or its folder in Zeron's
/// plugins folder.
fn own_id(target: &str, data_dir: &Path) -> anyhow::Result<String> {
    if manifest::check_id(target).is_ok() && !Path::new(target).is_dir() {
        return Ok(target.to_owned());
    }
    let canonical = std::fs::canonicalize(target).map_err(|_| exit(SETUP, format!("{target}: there's no plugin folder there")))?;
    let plugins = std::fs::canonicalize(data_dir.join(PLUGINS_DIR)).ok();
    match canonical.file_name().map(|n| n.to_string_lossy().into_owned()) {
        Some(id) if canonical.parent() == plugins.as_deref() => Ok(id),
        _ => Err(exit(
            SETUP,
            format!(
                "{target} isn't in Zeron's plugins folder: a folder anywhere publishes with a deploy key (set {}; make one with `zeron plugin key create`)",
                deploy_key::ENV
            ),
        )),
    }
}

/// How long `key create` waits for the sign-in it asked for.
const STEP_UP_WAIT: std::time::Duration = std::time::Duration::from_secs(5 * 60);

async fn key(client: &zeron_rpc::RpcClient, command: KeyCommand) -> anyhow::Result<()> {
    let call = |method: &'static str, params: Value| ask(client, method, params);
    match command {
        KeyCommand::Create { id, name, expires_in_days } => {
            let mut params = json!({ "id": id, "name": name });
            if let Some(days) = expires_in_days {
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as i64;
                params["expires"] = json!(now + i64::from(days) * 24 * 60 * 60 * 1000);
            }
            let mut reply = call(methods::CREATE_DEPLOY_KEY, params.clone()).await?;
            if reply["error"] == "step_up_required" {
                // A new key needs a fresh sign-in: asked for, then waited on.
                let url = call(methods::STEP_UP, json!({})).await?;
                eprintln!("Making a deploy key needs a fresh sign-in. Open this to confirm it's you:\n\n  {}\n", url["url"].as_str().unwrap_or(""));
                let started = std::time::Instant::now();
                while reply["error"] == "step_up_required" && started.elapsed() < STEP_UP_WAIT {
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    reply = call(methods::CREATE_DEPLOY_KEY, params.clone()).await?;
                }
            }
            answered(&reply)?;
            let public = reply["key"]["key"].as_str().unwrap_or("");
            println!("Made deploy key \"{name}\" for {id}{}.", if public.is_empty() { String::new() } else { format!(" ({public})") });
            println!("Its secret, shown this once (Zeron keeps no copy):\n");
            println!("  {}\n", reply["secret"].as_str().unwrap_or(""));
            println!("Add it to your CI's secrets as {}. `zeron plugin publish <folder>` reads it from there.", deploy_key::ENV);
        }
        KeyCommand::List { id } => {
            let reply = call(methods::LIST_DEPLOY_KEYS, json!({ "id": id })).await?;
            let keys = reply["keys"].as_array().cloned().unwrap_or_default();
            if keys.is_empty() {
                println!("{id} has no deploy keys. Make one with `zeron plugin key create {id} --name <what it's for>`.");
            }
            for k in keys {
                let mut line = format!("{}  {}", k["key"].as_str().unwrap_or("?"), k["name"].as_str().unwrap_or(""));
                if k["revokedAt"].is_number() || k["revoked_at"].is_number() {
                    line.push_str("  revoked");
                }
                println!("{line}");
            }
        }
        KeyCommand::Revoke { id, key } => {
            call(methods::REVOKE_DEPLOY_KEY, json!({ "id": id, "key": key })).await?;
            println!("Revoked {key}: it publishes nothing for {id} from now on.");
        }
    }
    Ok(())
}

/// Whether `command` is one of the store's (for the running Zeron).
pub fn is_store_command(command: &PluginCommand) -> bool {
    matches!(
        command,
        PluginCommand::Publish { .. }
            | PluginCommand::Status { .. }
            | PluginCommand::Key { .. }
            | PluginCommand::Yank { .. }
            | PluginCommand::Unyank { .. }
            | PluginCommand::Withdraw { .. }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_failure_word_has_its_exit_code() {
        assert_eq!(code("not_semver"), RULES);
        assert_eq!(code("version_exists"), RULES);
        assert_eq!(code("key_not_allowed"), REFUSED);
        assert_eq!(code("needs_username"), REFUSED);
        assert_eq!(code("signed_out"), REFUSED);
        assert_eq!(code("store_unreachable"), UNREACHABLE);
        assert_eq!(code("rate_limited"), UNREACHABLE);
        assert_eq!(code("something_new"), REFUSED);
    }
}
