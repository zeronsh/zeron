//! `zeron plugin …`: make, sign, check and list plugins through the running
//! Zeron (the same local connection `zeron mcp` uses). `zeron template …`
//! does the same for templates (whole replacements of Zeron's interface).

use clap::Subcommand;
use serde_json::{Value, json};
use zeron_rpc::methods;

#[derive(Subcommand)]
pub enum PluginCommand {
    /// Make a new plugin in Zeron's plugins folder, signed as you.
    New {
        /// Its id: lowercase letters, digits and dashes.
        id: String,
        /// Its name (defaults to the id).
        #[arg(long)]
        name: Option<String>,
        /// A permission it asks for (repeat for more).
        #[arg(long = "permission", value_name = "P")]
        permissions: Vec<String>,
    },
    /// Sign a plugin as you: an id in the plugins folder, or a folder's path.
    Sign { target: String },
    /// Check a plugin folder's signature and who made it.
    Verify { path: String },
    /// List the plugins Zeron knows.
    List,
}

#[derive(Subcommand)]
pub enum TemplateCommand {
    /// List the templates on this device, Zeron's own first.
    List,
    /// Run Zeron's window with a template. `zeron template use zeron` puts
    /// Zeron's own back, even with Zeron closed.
    Use { id: String },
    /// Make a new template: a full copy of Zeron's interface to rewrite.
    Create {
        /// Its id: lowercase letters, digits and dashes.
        id: String,
        /// Its name (defaults to the id).
        #[arg(long)]
        name: Option<String>,
        /// A template to start from (defaults to Zeron's own).
        #[arg(long)]
        from: Option<String>,
    },
    /// Show a template's errors, out-of-date files and what it inherits.
    Status { id: String },
    /// Delete a template (it's set aside in the templates folder's .removed).
    Delete { id: String },
}

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
            templates::use_template(&data_dir.join(TEMPLATES_DIR), templates::ZERON)
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
    }
    Ok(())
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

/// `you`, `@ada`, or `someone unknown`.
fn author(author: &Value) -> String {
    match str_of(author, "kind") {
        "you" => "you".into(),
        "verified" => match author["username"].as_str() {
            Some(name) => format!("@{name}"),
            None => "a verified author".into(),
        },
        _ => "someone unknown".into(),
    }
}

/// `focus 1.2.0  by @ada  on`.
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
