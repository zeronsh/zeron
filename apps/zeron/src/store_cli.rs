//! `zeron plugin check|publish|status|key|yank|unyank`: the plugin
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
use zeron_engine::plugins::{PLUGINS_DIR, listing, manifest};
use zeron_rpc::methods;

/// `println!`, the line [`listing::escaped`]: words from a plugin or the
/// store can't start a line of their own, colour one, or reorder it.
macro_rules! out {
    ($($t:tt)*) => { println!("{}", listing::escaped(&format!($($t)*))) };
}

/// `eprintln!`, escaped as [`out!`].
macro_rules! note {
    ($($t:tt)*) => { eprintln!("{}", listing::escaped(&format!($($t)*))) };
}

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

/// (Its line [`listing::escaped`]: it may carry a plugin's or the store's words.)
fn exit(code: i32, message: impl Into<String>) -> anyhow::Error {
    Exit { code, message: listing::escaped(&message.into()) }.into()
}

/// The exit code for a failure's word (`"<word>: <words>"`).
pub fn code(word: &str) -> i32 {
    match word {
        "not_semver" | "version_exists" | "not_newer" | "missing_readme" | "missing_listing" | "bad_listing" | "bad_package" | "bad_manifest"
        | "too_large" | "too_big" | "id_reserved" | "id_too_similar" | "claims_zeron" | "changed" | "not_a_package" | "unsigned" | "tampered" | "bad_file" => RULES,
        // (A request already made, or changed on its way: sending it again
        // is the fix.)
        "store_unreachable" | "rate_limited" | "store_error" | "unavailable" | "body_mismatch" | "replayed_request" => UNREACHABLE,
        // (The runner's clock is more than five minutes out.)
        "no_manifest" | "invalid" | "not_a_secret" | "damaged" | "stale_request" => SETUP,
        _ => REFUSED,
    }
}

/// What to do about each word the store answers a deploy key's request
/// with, in one plain line (the store's own words are for its logs).
fn plain_line(word: &str) -> Option<&'static str> {
    Some(match word {
        "replayed_request" => "The store had this exact request already. Run it again.",
        "stale_request" => "This machine's clock is more than five minutes off. Set it right, then run it again.",
        "key_revoked" => "That deploy key was revoked. Make a new one (`zeron plugin key create <id>`) and put its secret in ZERON_DEPLOY_KEY.",
        "key_expired" => "That deploy key has expired. Make a new one (`zeron plugin key create <id>`) and put its secret in ZERON_DEPLOY_KEY.",
        "invalid_signature" => "The store couldn't check this request's signature. Copy the deploy key's secret again from where it was shown.",
        "key_not_allowed" => "That deploy key can't do this: it publishes its own plugin and reads its status, nothing else.",
        "unauthenticated" => "The store doesn't know that deploy key, or nobody is signed in. Make a key with `zeron plugin key create <id>`.",
        "too_many_keys" => "This plugin has as many live deploy keys as it can (10). Revoke one first (`zeron plugin key revoke <id> <key>`).",
        "body_mismatch" => "What reached the store isn't what was signed (it changed on the way). Run it again.",
        _ => return None,
    })
}

/// A failure from the store or the engine, as an exit.
fn failed(error: &str) -> anyhow::Error {
    let word = install::reason_of(error).unwrap_or("");
    let message = plain_line(word).map_or_else(|| error.to_owned(), |line| format!("{word}: {line}"));
    exit(if word.is_empty() { REFUSED } else { code(word) }, message)
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
        out!("{} {} meets the store's rules.", prepared.manifest.id, prepared.manifest.version);
        return Ok(None);
    }
    let needs_store = matches!(command, PluginCommand::Publish { .. } | PluginCommand::Status { .. });
    if !needs_store {
        return Ok(Some(command));
    }
    let Some(key) = deploy_key()? else { return Ok(Some(command)) };
    let key = Arc::new(key);
    let edge = zeron_engine::plugins::store::client::edge_base(&zeron_cli::edge_url_from_env()).map_err(|why| exit(SETUP, why))?;
    let store = StoreClient::new(&edge);
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
                out!("{what} is ready: {} files, {}, signed with deploy key {}. Not sent (--dry-run).", prepared.files.len(), size(raw.len() as u64), key.public());
                return Ok(None);
            }
            let reply = store.publish(&prepared.manifest.id, raw, &caller).await.map_err(|e| failed(&e))?;
            submitted(&what, &reply, &store.page_url(&prepared.manifest.id));
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
        _ => "back in the store",
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
        note!("warning: {line}");
    }
    for line in publish::lines(&prepared.problems) {
        note!("error: {line}");
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

/// A version published: live at once, verified only once a reviewer has
/// read it (amendment 45), and where its page is.
fn submitted(what: &str, reply: &Value, page: &str) {
    let mark = if reply["verified"] == true { "verified" } else { "not yet verified" };
    out!("Published {what}: live now, {mark}. Anyone can install it from {page}");
    // Older versions nobody had verified take no new installs now.
    let version = reply["version"].as_str().unwrap_or("this version");
    for older in reply["superseded"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        out!("{}", listing::escaped(&format!("{older} no longer takes new installs: {version} replaced it before it was verified")));
    }
}

/// `zeron plugin status`'s lines: each plugin, then each version's state.
fn print_status(reply: &Value, only: Option<&str>) {
    for line in status_lines(reply, only) {
        println!("{line}");
    }
}

/// [`print_status`]'s lines, each [`listing::escaped`] (the store's words).
fn status_lines(reply: &Value, only: Option<&str>) -> Vec<String> {
    let plugins: Vec<&Value> = reply["plugins"].as_array().into_iter().flatten().filter(|p| only.is_none_or(|id| p["id"] == id)).collect();
    let mut lines = Vec::new();
    if plugins.is_empty() {
        lines.push(if only.is_some() { "That plugin isn't yours in the store." } else { "You have no plugins in the store yet." }.to_owned());
    }
    for plugin in plugins {
        lines.push(listing::escaped(plugin["id"].as_str().unwrap_or("?")));
        for version in plugin["versions"].as_array().into_iter().flatten() {
            let mark = if version["verified"] == true { "verified" } else { "not verified" };
            let mut line = format!("  {}  {}  {mark}", version["version"].as_str().unwrap_or("?"), version["status"].as_str().unwrap_or("?"));
            // Yanked because a newer version came out before anyone verified it.
            if version["yanked_by"] == "superseded" {
                let by = version["superseded"]["by"].as_str().unwrap_or("a newer version");
                line.push_str(&format!(": superseded by {by}"));
            }
            // A reviewer declined to verify it (yours alone to read).
            let review = &version["review"];
            if review["decision"] == "declined" {
                let words: Vec<&str> = [review["reason"].as_str(), review["message"].as_str()].into_iter().flatten().filter(|w| !w.is_empty()).collect();
                line.push_str(&format!(": declined{}", if words.is_empty() { String::new() } else { format!(": {}", words.join(": ")) }));
            }
            lines.push(listing::escaped(&line));
        }
    }
    lines
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
                    note!("{kind}: {}{at}: {}", p["rule"].as_str().unwrap_or("?"), p["words"].as_str().unwrap_or(""));
                }
            }
            let what = format!("{id} {}", prepared["version"].as_str().unwrap_or(""));
            if prepared["ok"] != true {
                let n = prepared["problems"].as_array().map_or(0, Vec::len);
                return Err(exit(RULES, format!("{what} doesn't meet the store's rules ({n} problem{}): fix and try again", if n == 1 { "" } else { "s" })));
            }
            if dry_run {
                out!("{what} is ready to publish. Not sent (--dry-run).");
                return Ok(());
            }
            let reply = call(methods::PUBLISH_PLUGIN, json!({ "id": id, "hash": prepared["hash"], "ticket": prepared["ticket"] })).await?;
            answered(&reply)?;
            submitted(&what, &reply, reply["url"].as_str().unwrap_or_default());
        }
        PluginCommand::Status { id } => print_status(&call(methods::STORE_MY_PLUGINS, json!({})).await?, id.as_deref()),
        PluginCommand::Yank { id, version } => action(client, &id, &version, "yank").await?,
        PluginCommand::Unyank { id, version } => action(client, &id, &version, "unyank").await?,
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
    out!("{}", acted(id, version, action, &reply));
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
        KeyCommand::Create { name, plugins, all, expires_in_days } => {
            let scope = if all { json!({ "all": true }) } else { json!({ "plugins": plugins }) };
            let what = if all { "all of your plugins".to_owned() } else { plugins.join(", ") };
            let mut params = json!({ "name": name, "scope": scope });
            if let Some(days) = expires_in_days {
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as i64;
                params["expires"] = json!(now + i64::from(days) * 24 * 60 * 60 * 1000);
            }
            let mut reply = call(methods::CREATE_DEPLOY_KEY, params.clone()).await?;
            if reply["error"] == "step_up_required" {
                // A new key needs a fresh sign-in begun for it (each key its
                // own): asked for, then waited on.
                let url = call(methods::STEP_UP, json!({ "for": "deploy_key" })).await?;
                eprintln!("Making a deploy key needs a fresh sign-in. Open this to confirm it's you:\n\n  {}\n", listing::escaped(url["url"].as_str().unwrap_or("")));
                let started = std::time::Instant::now();
                while reply["error"] == "step_up_required" && started.elapsed() < STEP_UP_WAIT {
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    reply = call(methods::CREATE_DEPLOY_KEY, params.clone()).await?;
                }
            }
            answered(&reply)?;
            let public = reply["key"]["key"].as_str().unwrap_or("");
            out!("Made deploy key \"{name}\" for {what}{}.", if public.is_empty() { String::new() } else { format!(" ({public})") });
            println!("Its secret, shown this once (Zeron keeps no copy):\n");
            println!("  {}\n", reply["secret"].as_str().unwrap_or(""));
            println!("Add it to your CI's secrets as {}. `zeron plugin publish <folder>` reads it from there.", deploy_key::ENV);
        }
        KeyCommand::List => {
            let reply = call(methods::LIST_DEPLOY_KEYS, json!({})).await?;
            let keys = reply["keys"].as_array().cloned().unwrap_or_default();
            if keys.is_empty() {
                out!("You have no deploy keys. Make one with `zeron plugin key create --name <what it's for> --plugins <id,…>` (or `--all`).");
            }
            for k in keys {
                out!("{}", key_line(&k));
            }
        }
        KeyCommand::Revoke { key } => {
            call(methods::REVOKE_DEPLOY_KEY, json!({ "key": key })).await?;
            out!("Revoked {key}: it publishes nothing from now on.");
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
    )
}

/// One deploy key, as `zeron plugin key list` says it: its key, name, what
/// it publishes, and whether it's revoked.
fn key_line(key: &Value) -> String {
    let scope = if key["scope"]["all"] == true {
        "all of your plugins".to_owned()
    } else {
        key["scope"]["plugins"].as_array().into_iter().flatten().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")
    };
    let mut line = format!("{}  {}  {scope}", key["key"].as_str().unwrap_or("?"), key["name"].as_str().unwrap_or(""));
    if key["revoked_at"].is_number() {
        line.push_str("  revoked");
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_failure_word_has_its_exit_code() {
        assert_eq!(code("not_semver"), RULES);
        assert_eq!(code("version_exists"), RULES);
        assert_eq!(code("claims_zeron"), RULES);
        assert_eq!(code("id_too_similar"), RULES);
        assert_eq!(code("key_not_allowed"), REFUSED);
        assert_eq!(code("needs_username"), REFUSED);
        assert_eq!(code("signed_out"), REFUSED);
        assert_eq!(code("store_unreachable"), UNREACHABLE);
        assert_eq!(code("rate_limited"), UNREACHABLE);
        assert_eq!(code("something_new"), REFUSED);
        assert_eq!(code("key_revoked"), REFUSED);
        assert_eq!(code("key_expired"), REFUSED);
        assert_eq!(code("too_many_keys"), REFUSED);
        assert_eq!(code("stale_request"), SETUP);
        assert_eq!(code("replayed_request"), UNREACHABLE);
        // A 401 or 403 with no body: refused, not "run it again".
        assert_eq!(code("unauthenticated"), REFUSED);
        assert_eq!(code("forbidden"), REFUSED);
        // Each signed-request word reads as one plain line, word first.
        for word in ["replayed_request", "stale_request", "key_revoked", "key_expired", "invalid_signature", "key_not_allowed", "unauthenticated", "too_many_keys", "body_mismatch"] {
            let said = failed(&format!("{word}: the store's own words")).to_string();
            assert!(said.starts_with(&format!("{word}: ")) && !said.contains('\n') && !said.contains("the store's own words"), "{said}");
        }
    }

    /// No control or format character a plugin or the store sent reaches a
    /// terminal or a CI log raw.
    const HOSTILE: &str = "a\nerror: fake line\u{1b}[2J\u{202e}";

    fn raw_free(line: &str) -> bool {
        !line.chars().any(listing::unseen_char)
    }

    #[test]
    fn a_manifest_s_id_is_never_printed_raw() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = json!({ "id": HOSTILE, "name": "Focus", "version": "1.0.0", "entry": "init.luau", "permissions": [] });
        std::fs::write(dir.path().join("zeron-plugin.json"), serde_json::to_vec(&manifest).unwrap()).unwrap();
        std::fs::write(dir.path().join("init.luau"), "return {}").unwrap();
        let prepared = read(dir.path()).unwrap();
        let line = exit(RULES, problems_line(&prepared)).to_string();
        assert!(raw_free(&line) && line.contains("\\u{a}") && line.contains("\\u{1b}"), "{line}");
        for line in publish::lines(&prepared.problems).iter().map(|l| listing::escaped(l)) {
            assert!(raw_free(&line), "{line}");
        }
        assert!(publish::lines(&prepared.problems).iter().any(|l| l.contains('\n')), "(the engine's words do carry it: the CLI escapes them)");
    }

    #[test]
    fn the_store_s_words_in_status_are_never_printed_raw() {
        let reply = json!({ "plugins": [{ "id": HOSTILE, "versions": [
            { "version": "1.0.0\u{1b}[31m", "status": "listed", "verified": false, "review": { "decided": true, "decision": "declined", "reason": HOSTILE } },
            { "version": "1.1.0", "status": "listed", "verified": false, "review": { "decided": true, "decision": "declined", "message": "fine\r\nerror: forged" } },
            { "version": "1.2.0", "status": "listed", "verified": true, "review": { "decided": true, "decision": "verified" } },
            { "version": "1.3.0", "status": "yanked", "verified": false, "yanked_by": "superseded", "superseded": { "at": 1, "by": "1.4.0\u{1b}[2J" } },
        ] }] });
        let lines = status_lines(&reply, None);
        assert_eq!(lines.len(), 5, "{lines:?}");
        assert!(lines.iter().all(|l| raw_free(l)), "{lines:?}");
        assert!(lines[1].contains("not verified: declined: a\\u{a}error: fake line\\u{1b}[2J\\u{202e}"), "{lines:?}");
        assert!(lines[2].ends_with("fine\\u{d}\\u{a}error: forged"), "{lines:?}");
        assert_eq!(lines[3], "  1.2.0  listed  verified");
        assert_eq!(lines[4], "  1.3.0  yanked  not verified: superseded by 1.4.0\\u{1b}[2J");
    }
}
