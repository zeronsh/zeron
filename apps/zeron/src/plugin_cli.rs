//! `zeron plugin …`: make, sign, check and list plugins through the running
//! Zeron (the same local connection `zeron mcp` uses). `zeron template …`
//! does the same for templates (whole replacements of Zeron's interface).

use serde_json::{Value, json};
use zeron_cli::{PluginCommand, TemplateCommand};
use zeron_rpc::methods;

async fn connect(
    ipc_port: u16,
    data_dir: &std::path::Path,
) -> anyhow::Result<zeron_rpc::RpcClient> {
    zeron_rpc::connect_ipc(ipc_port, Some(data_dir))
        .await
        .map_err(|_| {
            anyhow::anyhow!("Zeron isn't running here. Open it, or start it with `zeron headless`.")
        })
}

async fn call(
    client: &zeron_rpc::RpcClient,
    method: &'static str,
    params: Value,
) -> anyhow::Result<Value> {
    client
        .call(method, params)
        .await
        .map_err(|e| anyhow::anyhow!("{}", plain(e)))
}

pub async fn run_template(
    command: TemplateCommand,
    ipc_port: u16,
    data_dir: &std::path::Path,
) -> anyhow::Result<()> {
    use zeron_engine::plugins::{TEMPLATES_DIR, templates};
    // The escape hatch: putting Zeron's own back needs no running Zeron and
    // nothing set up; the app picks up `.active` when it next starts.
    let client = match connect(ipc_port, data_dir).await {
        Ok(client) => client,
        Err(_) if matches!(&command, TemplateCommand::Use { id } if id.trim() == templates::ZERON) =>
        {
            templates::use_template(&data_dir.join(TEMPLATES_DIR), templates::ZERON, None)
                .map_err(anyhow::Error::msg)?;
            println!("Using Zeron's own interface.");
            return Ok(());
        }
        Err(error) => return Err(error),
    };
    match command {
        TemplateCommand::List => {
            let list = call(&client, methods::LIST_TEMPLATES, json!({})).await?;
            for item in list["templates"].as_array().cloned().unwrap_or_default() {
                println!("{}", template_line(&item));
            }
            println!("Folder: {}", str_of(&list, "dir"));
        }
        TemplateCommand::Use { id } => {
            let reply = call(&client, methods::USE_TEMPLATE, json!({ "id": id })).await?;
            match str_of(&reply, "active") {
                templates::ZERON => println!("Using Zeron's own interface."),
                active => println!("Using {active}."),
            }
        }
        TemplateCommand::Create { id, name, from } => {
            let mut params = json!({ "id": id, "name": name.unwrap_or_else(|| id.clone()) });
            if let Some(from) = from {
                params["from"] = json!(from);
            }
            let reply = call(&client, methods::CREATE_TEMPLATE, params).await?;
            let files = reply["files"].as_array().map_or(0, Vec::len);
            println!(
                "Made {} in {} ({files} file{}).",
                str_of(&reply, "id"),
                str_of(&reply, "dir"),
                if files == 1 { "" } else { "s" }
            );
            println!("Use it with `zeron template use {}`.", str_of(&reply, "id"));
        }
        TemplateCommand::Status { id } => {
            let reply = call(&client, methods::TEMPLATE_STATUS, json!({ "id": id })).await?;
            let files = reply["files"].as_u64().unwrap_or(0);
            let inherits = reply["inherits"].as_u64().unwrap_or(0);
            println!(
                "{}{}: {files} file{}, {inherits} inherited from Zeron.",
                str_of(&reply, "id"),
                if reply["active"] == true {
                    " (in use)"
                } else {
                    ""
                },
                if files == 1 { "" } else { "s" },
            );
            if let Some(error) = reply["root_error"].as_str() {
                println!("Broken: {error}");
            }
            if let Some(errors) = reply["file_errors"].as_object() {
                for (rel, error) in errors {
                    println!(
                        "Error in {rel} (Zeron's own shows instead): {}",
                        error.as_str().unwrap_or("")
                    );
                }
            }
            for rel in reply["out_of_date"].as_array().cloned().unwrap_or_default() {
                println!(
                    "Out of date: {} (Zeron's own has changed since it was copied)",
                    rel.as_str().unwrap_or("")
                );
            }
        }
        TemplateCommand::Delete { id } => {
            let reply = call(&client, methods::DELETE_TEMPLATE, json!({ "id": id })).await?;
            println!("Deleted {}.", str_of(&reply, "deleted"));
            if str_of(&reply, "active") == templates::ZERON {
                println!("Using Zeron's own interface.");
            }
        }
    }
    Ok(())
}

/// `midnight  Midnight 1.0.0  in use  errors (ds/tokens.luau: …)`.
fn template_line(item: &Value) -> String {
    let mut line = format!(
        "{}  {} {}",
        str_of(item, "id"),
        str_of(item, "name"),
        str_of(item, "version")
    )
    .trim_end()
    .to_owned();
    if item["active"] == true {
        line.push_str("  in use");
    }
    let status = str_of(item, "status");
    line.push_str(&format!("  {}", status.replace('_', " ")));
    if let Some(error) = item["error"].as_str() {
        line.push_str(&format!(" ({error})"));
    }
    line
}

pub async fn run(
    command: PluginCommand,
    ipc_port: u16,
    data_dir: &std::path::Path,
) -> anyhow::Result<()> {
    let client = connect(ipc_port, data_dir).await?;
    let call = |method: &'static str, params: Value| call(&client, method, params);
    match command {
        PluginCommand::New {
            id,
            name,
            permissions,
        } => {
            let item = call(
                methods::NEW_PLUGIN,
                json!({ "id": id, "name": name.unwrap_or_else(|| id.clone()), "permissions": permissions }),
            )
            .await?;
            println!("Made {} in {}.", str_of(&item, "id"), str_of(&item, "dir"));
            println!("{}", line(&item));
        }
        PluginCommand::Sign { target } => {
            let path = std::path::Path::new(&target);
            let params = if target.contains('/') || path.is_dir() && !looks_like_id(&target) {
                let absolute = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
                json!({ "path": absolute.display().to_string() })
            } else {
                json!({ "id": target })
            };
            let reply = call(methods::SIGN_PLUGIN, params).await?;
            let files = reply["files"].as_u64().unwrap_or(0);
            let name = reply["id"].as_str().map(str::to_owned).unwrap_or(target);
            println!(
                "Signed {name} as {} ({files} file{}).",
                author(&reply["author"]),
                if files == 1 { "" } else { "s" }
            );
        }
        PluginCommand::Verify { path } => {
            let absolute = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone().into());
            let reply = call(
                methods::VERIFY_PLUGIN,
                json!({ "path": absolute.display().to_string() }),
            )
            .await?;
            let status = str_of(&reply, "status");
            let by = author(&reply["author"]);
            match status {
                "ok" => println!("Signed by {by}. Its files match."),
                "unsigned" => println!("It isn't signed."),
                "tampered" => println!(
                    "{}.",
                    reply["author"]["reason"]
                        .as_str()
                        .unwrap_or("Its files don't match its signature")
                ),
                _ => println!("It's broken: {}", str_of(&reply, "error")),
            }
            if let Some(reason) = reply["author"]["reason"].as_str()
                && status == "ok"
            {
                println!("{reason}.");
            }
            if status != "ok" {
                std::process::exit(1);
            }
        }
        PluginCommand::List => {
            let list = call(methods::LIST_PLUGINS, json!({})).await?;
            let plugins = list["plugins"].as_array().cloned().unwrap_or_default();
            if plugins.is_empty() {
                println!("No plugins yet. Make one with `zeron plugin new <id>`.");
            }
            for item in &plugins {
                println!("{}", line(item));
            }
        }
        PluginCommand::On { id } => switch(&client, &id, true).await?,
        PluginCommand::Off { id } => switch(&client, &id, false).await?,
        PluginCommand::Export { id, to } => {
            // `to` is already a full path (the parser made it one).
            let mut params = json!({ "id": id });
            if let Some(to) = to {
                params["to"] = json!(to.display().to_string());
            }
            let reply = call(methods::EXPORT_PLUGIN, params).await?;
            println!(
                "Saved {id} to {} ({}).",
                str_of(&reply, "path"),
                size(reply["bytes"].as_u64().unwrap_or(0))
            );
            println!("Share that file: anyone installs it from the file or a link to it.");
        }
    }
    Ok(())
}

/// `zeron plugin on|off <id>`: the switch set, then the list read back and
/// what's in effect said. Fails (non-zero) with the engine's words when it's
/// refused, and when what's in effect isn't what was asked.
async fn switch(client: &zeron_rpc::RpcClient, id: &str, on: bool) -> anyhow::Result<()> {
    let id = id.trim();
    let reply = call(client, methods::SET_PLUGIN_OFF, json!({ "id": id, "off": !on }))
        .await
        .map_err(|error| match error.to_string() {
            // Zeron's own are known once a window has told the engine where
            // they are.
            message if message.starts_with("There's no plugin") => anyhow::anyhow!(
                "{message}. Zeron's own plugins are known once Zeron's window has opened."
            ),
            message => anyhow::anyhow!("{message}"),
        })?;
    let list = call(client, methods::LIST_PLUGINS, json!({})).await?;
    let item = find(&list, id).unwrap_or(&reply);
    let said = switched(item);
    if str_of(item, "status") != if on { "on" } else { "off" } {
        anyhow::bail!("{said}");
    }
    println!("{said}");
    Ok(())
}

/// Plugin `id` in a `ListPlugins` reply: Zeron's own first, as the engine
/// reads its switch (a folder of yours under its id is broken anyway).
fn find<'a>(list: &'a Value, id: &str) -> Option<&'a Value> {
    let plugins = list["plugins"].as_array()?;
    let mut matching = plugins.iter().filter(|p| str_of(p, "id") == id);
    let first = matching.clone().next();
    matching.find(|p| p["builtin"] == true).or(first)
}

/// `Orchestrators is on for your whole account.`, `Focus is off on this
/// device.`, or what it is instead (`Focus is waiting for you to allow it.`).
fn switched(item: &Value) -> String {
    let name = match str_of(item, "name") {
        "" => str_of(item, "id"),
        name => name,
    };
    let where_ = if str_of(item, "scope") == "account" {
        "for your whole account"
    } else {
        "on this device"
    };
    match str_of(item, "status") {
        status @ ("on" | "off") => format!("{name} is {status} {where_}."),
        "pending" => format!("{name} is waiting for you to allow it."),
        "tampered" => format!("{name} has changed since you allowed it."),
        _ => match item["error"].as_str() {
            Some(error) => format!("{name} is broken: {error}"),
            None => format!("{name} is broken."),
        },
    }
}

/// `512 bytes`, `12.3 KB`, `4.1 MB`.
fn size(bytes: u64) -> String {
    match bytes {
        1 => "1 byte".into(),
        0..1024 => format!("{bytes} bytes"),
        1024..1_048_576 => format!("{:.1} KB", bytes as f64 / 1024.0),
        _ => format!("{:.1} MB", bytes as f64 / 1_048_576.0),
    }
}

fn looks_like_id(text: &str) -> bool {
    !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn str_of<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or("")
}

/// `you`, `Zeron`, `@ada`, or `someone unknown`.
fn author(author: &Value) -> String {
    match str_of(author, "kind") {
        "you" => "you".into(),
        "zeron" => "Zeron".into(),
        "verified" => match author["username"].as_str() {
            Some(name) => format!("@{name}"),
            None => "a verified author".into(),
        },
        _ => "someone unknown".into(),
    }
}

/// `focus 1.2.0  by @ada  on`, `orchestrators 1.0.0  by Zeron  off  account`.
fn line(item: &Value) -> String {
    let status = str_of(item, "status");
    let mut line = format!(
        "{} {}  by {}  {}",
        str_of(item, "id"),
        str_of(item, "version"),
        author(&item["author"]),
        if status == "tampered" {
            "changed since you allowed it"
        } else {
            status
        }
    );
    // One switch for every device of the account, not this one's.
    if str_of(item, "scope") == "account" {
        line.push_str("  account");
    }
    if let Some(ask) = item["ask"]
        .as_array()
        .filter(|a| !a.is_empty() && status == "pending")
    {
        let ask: Vec<&str> = ask.iter().filter_map(Value::as_str).collect();
        line.push_str(&format!("  (asks for {})", ask.join(", ")));
    }
    if let Some(error) = item["error"].as_str() {
        line.push_str(&format!("  ({error})"));
    }
    line
}

/// The engine's message, without transport wrapping.
fn plain(error: zeron_rpc::RpcError) -> String {
    match error {
        zeron_rpc::RpcError::Failed(message) => message,
        zeron_rpc::RpcError::UnknownMethod(_) => {
            "This Zeron is too old for plugins. Update it first.".into()
        }
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{find, line, size, switched};
    use serde_json::json;

    fn orchestrators(status: &str) -> serde_json::Value {
        json!({
            "id": "orchestrators", "name": "Orchestrators", "version": "1.0.0",
            "author": { "kind": "zeron" }, "status": status, "builtin": true,
            "scope": "account", "default_off": true,
        })
    }

    #[test]
    fn a_switch_says_what_is_in_effect_and_where() {
        assert_eq!(switched(&orchestrators("on")), "Orchestrators is on for your whole account.");
        assert_eq!(switched(&orchestrators("off")), "Orchestrators is off for your whole account.");
        let git = json!({ "id": "git", "name": "Git", "status": "off", "builtin": true });
        assert_eq!(switched(&git), "Git is off on this device.");
        let focus = json!({ "id": "focus", "name": "Focus", "status": "pending" });
        assert_eq!(switched(&focus), "Focus is waiting for you to allow it.");
        let broken = json!({ "id": "x", "status": "broken", "error": "Its entry file init.luau is missing" });
        assert_eq!(switched(&broken), "x is broken: Its entry file init.luau is missing");
    }

    #[test]
    fn the_list_says_on_or_off_and_an_account_switch() {
        assert_eq!(line(&orchestrators("on")), "orchestrators 1.0.0  by Zeron  on  account");
        let focus = json!({ "id": "focus", "version": "1.2.0", "author": { "kind": "verified", "username": "ada" }, "status": "off" });
        assert_eq!(line(&focus), "focus 1.2.0  by @ada  off");
    }

    /// A folder of yours under a built-in's id is listed too (broken): the
    /// switch read back is Zeron's own.
    #[test]
    fn the_switch_read_back_is_zeron_s_own() {
        let list = json!({ "plugins": [
            { "id": "orchestrators", "status": "broken", "author": { "kind": "you" } },
            orchestrators("on"),
            { "id": "focus", "status": "on" },
        ] });
        assert_eq!(find(&list, "orchestrators").unwrap()["status"], "on");
        assert_eq!(find(&list, "focus").unwrap()["status"], "on");
        assert!(find(&list, "nope").is_none());
    }

    #[test]
    fn sizes_read_in_plain_words() {
        assert_eq!(size(1), "1 byte");
        assert_eq!(size(512), "512 bytes");
        assert_eq!(size(12_595), "12.3 KB");
        assert_eq!(size(4_299_161), "4.1 MB");
    }
}
