//! `zeron plugin check|publish|status|token|yank|unyank`: the plugin
//! store from the command line.
//!
//! Two ways, one model (the plan's "Who may publish"):
//! - **In CI**, with a deploy token in `ZERON_DEPLOY_TOKEN` (never an
//!   argument, so it stays out of process lists and shell history): no Zeron
//!   running, no sign-in. The folder is checked, packed (unsigned: the store
//!   records the version as the token's) and sent with the token as its
//!   bearer (`store::client`).
//! - **On your machine**, without it: through the running Zeron, your own
//!   plugin signed with this device's key and sent with your session
//!   (`PreparePluginPublish`, then `PublishPlugin` with the ticket it gave).
//!
//! `check` needs neither. Failures say one line each and exit with a code a
//! CI job can act on ([`code`]).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Value, json};
use zeron_cli::{PluginCommand, TokenCommand};
use zeron_engine::plugins::install;
use zeron_engine::plugins::store::client::{Caller, StoreClient};
use zeron_engine::plugins::store::deploy_token::{self, DeployToken};
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
/// Usage or set-up: no such folder, no token where one is needed, a damaged token.
pub const SETUP: i32 = 2;
/// The store refused the token or the account (not allowed, signed out,
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
        | "too_large" | "too_big" | "id_reserved" | "id_too_similar" | "changed" | "not_a_package" | "unsigned" | "tampered" | "bad_file" => RULES,
        "store_unreachable" | "rate_limited" | "store_error" | "unavailable" => UNREACHABLE,
        "no_manifest" | "invalid" | "not_a_token" | "damaged" => SETUP,
        _ => REFUSED,
    }
}

/// What to do about each word the store answers a deploy token's request
/// with, in one plain line (the store's own words are for its logs).
fn plain_line(word: &str) -> Option<&'static str> {
    Some(match word {
        "invalid_token" => "The store won't publish this with that deploy token: it's unknown, revoked or expired, isn't for this plugin, or this is the plugin's first version (that comes from your machine). Check `zeron plugin token list`, or make a new one (`zeron plugin token create`) and put it in ZERON_DEPLOY_TOKEN.",
        "token_not_allowed" => "A deploy token can't do this: it publishes the plugins it was made for, and reads their status, nothing else.",
        "unauthenticated" => "The store doesn't know who's asking: no deploy token reached it, and nobody is signed in.",
        "too_many_tokens" => "Your account has as many live deploy tokens as it can. Revoke one first (`zeron plugin token revoke <id>`).",
        _ => return None,
    })
}

/// A failure from the store or the engine, as an exit.
fn failed(error: &str) -> anyhow::Error {
    let word = install::reason_of(error).unwrap_or("");
    let message = plain_line(word).map_or_else(|| error.to_owned(), |line| format!("{word}: {line}"));
    exit(if word.is_empty() { REFUSED } else { code(word) }, message)
}

/// The deploy token in `ZERON_DEPLOY_TOKEN`, when one is set (an empty
/// variable is none: CI systems set a missing secret to ""). One that isn't
/// a token, or is mistyped, stops here, before anything is sent (and the
/// line never quotes it).
fn deploy_token() -> anyhow::Result<Option<DeployToken>> {
    match std::env::var(deploy_token::ENV) {
        Ok(text) if !text.trim().is_empty() => DeployToken::parse(&text).map(Some).map_err(|why| {
            let word = if why == deploy_token::DAMAGED { "damaged" } else { "not_a_token" };
            exit(SETUP, format!("{word}: {}: {why}", deploy_token::ENV))
        }),
        _ => Ok(None),
    }
}

/// Run `command` here when it needs no running Zeron: `check` always, and
/// with `ZERON_DEPLOY_TOKEN` set, `publish` and `status`: all a deploy token
/// may do, so a leaked one can add an unverified version, which review
/// reads, and nothing else. `Ok(Some(command))` hands it back for the
/// running Zeron and its signed-in session.
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
    let Some(token) = deploy_token()? else { return Ok(Some(command)) };
    let token = Arc::new(token);
    let edge = zeron_engine::plugins::store::client::edge_base(&zeron_cli::edge_url_from_env()).map_err(|why| exit(SETUP, why))?;
    let store = StoreClient::new(&edge);
    let caller = Caller::DeployToken(token.clone());
    match command {
        PluginCommand::Publish { target, dry_run } => {
            let mut prepared = read(&folder(&target, data_dir)?)?;
            // Newer than what the store has (a deploy token reads its
            // plugins' state), asked only of a plugin that meets the rules here; the
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
            let raw = prepared.pack_unsigned().map_err(|e| failed(&e))?;
            let what = format!("{} {}", prepared.manifest.id, prepared.manifest.version);
            if dry_run {
                out!("{what} is ready: {} files, {}, to publish with deploy token {}. Not sent (--dry-run).", prepared.files.len(), size(raw.len() as u64), token.short());
                return Ok(None);
            }
            let reply = store.publish(&prepared.manifest.id, raw, &caller).await.map_err(|(e, _)| failed(&e))?;
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
        PluginCommand::Token { .. } => "token",
        _ => "publish",
    };
    if what != "publish" && std::env::var(deploy_token::ENV).is_ok_and(|s| !s.trim().is_empty()) {
        format!(
            "`zeron plugin {what}` needs Zeron running here and signed in: a deploy token only publishes and reads status ({} can't do this)",
            deploy_token::ENV
        )
    } else if what == "publish" {
        format!("Zeron isn't running here. From CI, set {} to a deploy token (`zeron plugin token create`).", deploy_token::ENV)
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
        PluginCommand::Token { command } => token(client, command).await?,
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
                "{target} isn't in Zeron's plugins folder: a folder anywhere publishes with a deploy token (set {}; make one with `zeron plugin token create`)",
                deploy_token::ENV
            ),
        )),
    }
}

/// How long `token create` waits for the sign-in it asked for.
const STEP_UP_WAIT: std::time::Duration = std::time::Duration::from_secs(5 * 60);

async fn token(client: &zeron_rpc::RpcClient, command: TokenCommand) -> anyhow::Result<()> {
    let call = |method: &'static str, params: Value| ask(client, method, params);
    match command {
        TokenCommand::Create { name, plugins, all, expires_in_days } => {
            let scope = if all { json!({ "all": true }) } else { json!({ "plugins": plugins }) };
            let what = if all { "all of your plugins".to_owned() } else { plugins.join(", ") };
            let mut params = json!({ "name": name, "scope": scope });
            if let Some(days) = expires_in_days {
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as i64;
                params["expires"] = json!(now + i64::from(days) * 24 * 60 * 60 * 1000);
            }
            let mut reply = call(methods::CREATE_DEPLOY_TOKEN, params.clone()).await?;
            if reply["error"] == "step_up_required" {
                // A new token needs a fresh sign-in begun for it (each token
                // its own): asked for, then waited on.
                let url = call(methods::STEP_UP, json!({ "for": "deploy_token" })).await?;
                eprintln!("Making a deploy token needs a fresh sign-in. Open this to confirm it's you:\n\n  {}\n", listing::escaped(url["url"].as_str().unwrap_or("")));
                let started = std::time::Instant::now();
                while reply["error"] == "step_up_required" && started.elapsed() < STEP_UP_WAIT {
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    reply = call(methods::CREATE_DEPLOY_TOKEN, params.clone()).await?;
                }
            }
            answered(&reply)?;
            // (Read as a token is: only that is ever printed whole.)
            let secret = reply["secret"].as_str().and_then(|t| DeployToken::parse(t).ok()).ok_or_else(|| exit(REFUSED, "store_error: The store's answer had no token in it that could be read: revoke it with `zeron plugin token list` and make another"))?;
            out!("Made deploy token \"{name}\" ({}) for {what}.", secret.short());
            println!("Here it is. It won't be shown again, and neither Zeron nor the store keeps it:\n");
            println!("  {}\n", secret.as_str());
            println!("Add it to your CI's secrets as {}. `zeron plugin publish <folder>` reads it from there.", deploy_token::ENV);
        }
        TokenCommand::List => {
            let reply = call(methods::LIST_DEPLOY_TOKENS, json!({})).await?;
            let tokens = reply["tokens"].as_array().cloned().unwrap_or_default();
            if tokens.is_empty() {
                out!("You have no deploy tokens. Make one with `zeron plugin token create --name <what it's for> --plugins <id,…>` (or `--all`).");
            }
            for t in tokens {
                out!("{}", token_line(&t));
            }
        }
        TokenCommand::Revoke { id } => {
            call(methods::REVOKE_DEPLOY_TOKEN, json!({ "id": id })).await?;
            out!("Revoked {id}: it publishes nothing from now on.");
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
            | PluginCommand::Token { .. }
            | PluginCommand::Yank { .. }
            | PluginCommand::Unyank { .. }
    )
}

/// One deploy token, as `zeron plugin token list` says it: its id (what
/// `revoke` takes), its short id (its first characters, as the store names
/// it), name, what it publishes, and whether it's revoked.
fn token_line(token: &Value) -> String {
    let scope = if token["scope"]["all"] == true {
        "all of your plugins".to_owned()
    } else {
        token["scope"]["plugins"].as_array().into_iter().flatten().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")
    };
    let mut line = format!(
        "{}  {}  {}  {scope}",
        token["id"].as_str().unwrap_or("?"),
        token["short"].as_str().unwrap_or("?"),
        token["name"].as_str().unwrap_or("")
    );
    if token["revoked_at"].is_number() {
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
        assert_eq!(code("id_too_similar"), RULES);
        assert_eq!(code("token_not_allowed"), REFUSED);
        assert_eq!(code("invalid_token"), REFUSED);
        assert_eq!(code("needs_username"), REFUSED);
        assert_eq!(code("signed_out"), REFUSED);
        assert_eq!(code("store_unreachable"), UNREACHABLE);
        assert_eq!(code("rate_limited"), UNREACHABLE);
        assert_eq!(code("something_new"), REFUSED);
        assert_eq!(code("too_many_tokens"), REFUSED);
        assert_eq!(code("not_a_token"), SETUP);
        assert_eq!(code("damaged"), SETUP);
        // A 401 or 403 with no body: refused, not "run it again".
        assert_eq!(code("unauthenticated"), REFUSED);
        assert_eq!(code("forbidden"), REFUSED);
        // Each token word reads as one plain line, word first.
        for word in ["invalid_token", "token_not_allowed", "unauthenticated", "too_many_tokens"] {
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
