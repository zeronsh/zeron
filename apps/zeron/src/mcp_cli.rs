//! `zeron mcp …`: the MCP servers Zeron gives agents, managed through the
//! running Zeron (the same local connection `zeron plugin …` uses) as
//! Settings → MCP manages them: list them, add one (one of Zeron's, a remote
//! server, one this device runs), sign in and out, name an account, turn one
//! on or off, remove it. Plain `zeron mcp` is the Zeron MCP server itself
//! (`crate::run_mcp`).
//!
//! A server is named by its id, the name agents see it by (`linear_work`),
//! or its service and account (`linear/Work`): [`resolve`].

use std::collections::HashSet;
use std::io::{IsTerminal as _, Write as _};
use std::time::{Duration, Instant};

use serde::{Serialize, de::DeserializeOwned};
use serde_json::json;
use zeron_cli::{McpAdd, McpAuth, McpClientArgs, McpCommand};
use zeron_proto::{
    CancelMcpAuthRequest, DeleteMcpServerRequest, DisconnectMcpAuthRequest, MCP_CLIENT_REQUIRED,
    McpAuthFlowStatus, McpAuthKind, McpPreset, McpServerStatus, McpServerView, McpServersSnapshot,
    McpStorage, McpTransport, PollMcpAuthRequest, PollMcpAuthResponse, SetMcpServerEnabledRequest,
    StartMcpAuthRequest, StartMcpAuthResponse, UpsertMcpServerRequest,
};
use zeron_rpc::methods;

/// How often a sign-in in the browser is asked after (as Settings → MCP does).
const AUTH_POLL_INTERVAL: Duration = Duration::from_secs(1);
/// How long a sign-in is waited for: the engine gives up on one after five
/// minutes (its `AUTH_FLOW_TIMEOUT`), this a little later should that
/// answer never come.
const AUTH_WAIT: Duration = Duration::from_secs(5 * 60 + 30);

pub async fn run(
    command: McpCommand,
    ipc_port: u16,
    data_dir: &std::path::Path,
) -> anyhow::Result<()> {
    let mcp = Mcp {
        client: crate::plugin_cli::connect(ipc_port, data_dir).await?,
    };
    match command {
        McpCommand::List { json } => {
            let snapshot = mcp.snapshot().await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&snapshot)?);
            } else {
                print!("{}", list_text(&snapshot));
            }
        }
        McpCommand::Add(add) => mcp.add(add).await?,
        McpCommand::SignIn { server, client } => {
            let app = OwnApp::from_args(&client)?;
            let snapshot = mcp.snapshot().await?;
            let server = resolve(&snapshot.servers, &server).map_err(anyhow::Error::msg)?;
            mcp.sign_in(server, &app, &snapshot.client_redirect_uri)
                .await?;
        }
        McpCommand::SignOut { server } => {
            let snapshot = mcp.snapshot().await?;
            let server = resolve(&snapshot.servers, &server).map_err(anyhow::Error::msg)?;
            if server.transport == McpTransport::Stdio || server.auth_kind == McpAuthKind::None {
                anyhow::bail!("{} has no sign-in to sign out of.", full_name(server));
            }
            let _: McpServersSnapshot = mcp
                .call(
                    methods::DISCONNECT_MCP_AUTH,
                    &DisconnectMcpAuthRequest {
                        id: server.id.clone(),
                    },
                )
                .await?;
            println!("Signed out of {}.", full_name(server));
            if server.auth_kind == McpAuthKind::OAuth {
                println!("Sign in again with `zeron mcp sign-in {}`.", handle(server));
            }
        }
        McpCommand::Rename { server, account } => {
            let snapshot = mcp.snapshot().await?;
            let server = resolve(&snapshot.servers, &server).map_err(anyhow::Error::msg)?;
            let account = account.trim();
            if let Some(taken) = account_taken(&snapshot.servers, server, account) {
                anyhow::bail!(
                    "{} already has an account named {taken}.",
                    service_name(&snapshot, server)
                );
            }
            // Everything it has, as it has it, and the new name: what isn't
            // given (its sign-in, its environment) the engine keeps.
            let request = UpsertMcpServerRequest {
                id: Some(server.id.clone()),
                name: server.name.clone(),
                preset: server.preset.clone(),
                transport: server.transport,
                url: server.url.clone(),
                command: server.command.clone(),
                args: server.args.clone(),
                auth_kind: server.auth_kind,
                local_only: server.storage == McpStorage::Local,
                account_label: Some(account.to_owned()),
                ..Default::default()
            };
            let after: McpServersSnapshot = mcp.call(methods::UPSERT_MCP_SERVER, &request).await?;
            let renamed = after
                .servers
                .iter()
                .find(|s| s.id == server.id)
                .unwrap_or(server);
            if account.is_empty() {
                println!("{} has no account name now.", full_name(renamed));
            } else {
                println!("Renamed it {}.", full_name(renamed));
            }
            println!("Agents see it as {}.", agent_name(renamed));
        }
        McpCommand::On { server } => mcp.set_enabled(&server, true).await?,
        McpCommand::Off { server } => mcp.set_enabled(&server, false).await?,
        McpCommand::Remove { server, yes } => {
            let snapshot = mcp.snapshot().await?;
            let server = resolve(&snapshot.servers, &server).map_err(anyhow::Error::msg)?;
            let name = full_name(server);
            if !yes && !confirm_remove(server)? {
                println!("Kept {name}.");
                return Ok(());
            }
            let _: McpServersSnapshot = mcp
                .call(
                    methods::DELETE_MCP_SERVER,
                    &DeleteMcpServerRequest {
                        id: server.id.clone(),
                    },
                )
                .await?;
            println!("Removed {name}.");
        }
    }
    Ok(())
}

struct Mcp {
    client: zeron_rpc::RpcClient,
}

impl Mcp {
    async fn call<T: DeserializeOwned>(
        &self,
        method: &'static str,
        params: &impl Serialize,
    ) -> anyhow::Result<T> {
        let reply = self
            .client
            .call(method, serde_json::to_value(params)?)
            .await
            .map_err(|e| anyhow::anyhow!("{}", plain(e)))?;
        Ok(serde_json::from_value(reply)?)
    }

    /// The servers as they are now: the vault's listed again (another device
    /// may have added or changed one), else what's known when it's offline.
    async fn snapshot(&self) -> anyhow::Result<McpServersSnapshot> {
        self.call(methods::REFRESH_MCP_SERVERS, &json!({})).await
    }

    async fn add(&self, add: McpAdd) -> anyhow::Result<()> {
        let app = OwnApp::from_args(&add.client)?;
        let snapshot = self.snapshot().await?;
        let account = add
            .account
            .as_deref()
            .map(str::trim)
            .filter(|a| !a.is_empty())
            .map(str::to_owned);
        let mut request = UpsertMcpServerRequest {
            enabled: Some(true),
            // Kept in Zeron Vault when there is one (on every device), as
            // Settings → MCP adds it; a stdio server is always this device's.
            local_only: !snapshot.vault_available,
            account_label: account.clone(),
            oauth_client_id: app.id.clone(),
            oauth_client_secret: app.secret.clone(),
            ..Default::default()
        };
        if let Some(id) = add.preset.as_deref() {
            let preset = find_preset(&snapshot, id)?;
            if preset.client_required && app.id.is_none() {
                let retry = format!("zeron mcp add {}", preset.id);
                anyhow::bail!(
                    "{}",
                    client_required(&preset.name, &snapshot.client_redirect_uri, &retry)
                );
            }
            request.name = preset.name.clone();
            request.preset = Some(preset.id.clone());
            request.transport = preset.transport;
            request.url = Some(preset.url.clone());
            request.auth_kind = preset.auth_kind;
            match preset.auth_kind {
                McpAuthKind::Bearer => {
                    request.bearer_token =
                        Some(token_from_env(add.token_env.as_deref(), &preset.name)?);
                }
                McpAuthKind::Headers => {
                    let token = token_from_env(add.token_env.as_deref(), &preset.name)?;
                    let header = preset
                        .header_name
                        .clone()
                        .unwrap_or_else(|| "Authorization".into());
                    request.headers = [(header, token)].into();
                }
                McpAuthKind::OAuth | McpAuthKind::None => {
                    if add.token_env.is_some() {
                        anyhow::bail!("{} doesn't sign in with a token.", preset.name);
                    }
                }
            }
        } else if let Some(url) = add.url {
            request.name = add.name.clone().unwrap_or_default();
            request.transport = McpTransport::Http;
            request.url = Some(url);
            request.auth_kind = match (add.auth, add.token_env.is_some()) {
                (None | Some(McpAuth::Bearer), true) => McpAuthKind::Bearer,
                (Some(McpAuth::Bearer), false) => anyhow::bail!(
                    "A server that signs in with a token needs it: --token-env names the environment variable that holds it."
                ),
                (Some(_), true) => anyhow::bail!("--token-env is for --auth bearer."),
                (None | Some(McpAuth::Oauth), false) => McpAuthKind::OAuth,
                (Some(McpAuth::None), false) => McpAuthKind::None,
            };
            if request.auth_kind == McpAuthKind::Bearer {
                request.bearer_token =
                    Some(token_from_env(add.token_env.as_deref(), &request.name)?);
            }
        } else if let Some(command) = add.command {
            request.name = add.name.clone().unwrap_or_default();
            request.transport = McpTransport::Stdio;
            request.command = Some(command);
            request.args = add.args;
            request.auth_kind = McpAuthKind::None;
            request.local_only = true;
            for var in &add.env {
                request.env.insert(var.clone(), passed_env(var)?);
            }
        } else {
            // (The parser takes exactly one of the three.)
            anyhow::bail!("Name a server to add: one of Zeron's, --url or --command.");
        }
        if request.auth_kind != McpAuthKind::OAuth && app.id.is_some() {
            anyhow::bail!("--client-id is for a server that signs in with OAuth.");
        }

        // Another account of a service already here takes a name of its
        // own (what tells them apart, to agents too), and is kept where the
        // first one is.
        let service = request_service_key(&request);
        let accounts: Vec<&McpServerView> = snapshot
            .servers
            .iter()
            .filter(|s| service_key(s) == service)
            .collect();
        if let Some(first) = accounts.first() {
            let Some(account) = account.as_deref() else {
                let also = if first.auth_kind == McpAuthKind::OAuth {
                    format!(
                        ", or sign in to it with `zeron mcp sign-in {}`",
                        handle(first)
                    )
                } else {
                    String::new()
                };
                anyhow::bail!(
                    "{} is added already. Add another account of it with --account <name>{also}.",
                    service_name(&snapshot, first)
                );
            };
            if let Some(taken) = accounts
                .iter()
                .find_map(|s| s.account_label.as_deref().filter(|l| same_text(l, account)))
            {
                anyhow::bail!(
                    "{} already has an account named {taken}.",
                    service_name(&snapshot, first)
                );
            }
            if request.transport != McpTransport::Stdio {
                request.local_only = first.storage == McpStorage::Local;
            }
        }

        let before: HashSet<&str> = snapshot.servers.iter().map(|s| s.id.as_str()).collect();
        let after: McpServersSnapshot = self.call(methods::UPSERT_MCP_SERVER, &request).await?;
        let made = after
            .servers
            .iter()
            .find(|s| !before.contains(s.id.as_str()))
            .ok_or_else(|| anyhow::anyhow!("Zeron didn't add it (it isn't in the list after)."))?;
        println!("Added {}, kept {}.", full_name(made), kept_in(made));
        if made.auth_kind == McpAuthKind::OAuth && made.transport != McpTransport::Stdio {
            println!();
            self.sign_in(made, &app, &after.client_redirect_uri).await
        } else {
            println!("Agents see it as {}.", agent_name(made));
            Ok(())
        }
    }

    /// Sign in to `server` in the browser and wait for it to finish.
    async fn sign_in(
        &self,
        server: &McpServerView,
        app: &OwnApp,
        redirect_uri: &str,
    ) -> anyhow::Result<()> {
        let name = full_name(server);
        match (server.transport, server.auth_kind) {
            (McpTransport::Stdio, _) => {
                anyhow::bail!("{name} runs on this device: there's nothing to sign in to.")
            }
            (_, McpAuthKind::None) => anyhow::bail!("{name} needs no sign-in."),
            (_, McpAuthKind::Bearer | McpAuthKind::Headers) => anyhow::bail!(
                "{name} signs in with a token, not in the browser. To change its token, remove it and add it again with --token-env."
            ),
            (_, McpAuthKind::OAuth) => {}
        }
        let request = StartMcpAuthRequest {
            id: server.id.clone(),
            client_id: app.id.clone(),
            client_secret: app.secret.clone(),
            scopes: Vec::new(),
        };
        let started: StartMcpAuthResponse = match self.call(methods::START_MCP_AUTH, &request).await
        {
            Ok(started) => started,
            Err(error) if error.to_string().contains(MCP_CLIENT_REQUIRED) => {
                let retry = format!("zeron mcp sign-in {}", handle(server));
                anyhow::bail!("{}", client_required(&name, redirect_uri, &retry))
            }
            Err(error) => return Err(error),
        };
        if !openable_auth_url(&started.auth_url) {
            self.cancel(&started.flow_id).await;
            anyhow::bail!("Zeron's sign-in page for {name} isn't a web address to open.");
        }
        println!(
            "Sign in to {name} in your browser:\n\n  {}\n",
            started.auth_url
        );
        if open_in_browser(&started.auth_url) {
            println!("(It's opened in your browser.)");
        } else {
            let port = started.callback_port;
            println!(
                "Open it in a browser on this computer: it comes back to 127.0.0.1:{port}. From another computer, forward that port first (`ssh -L {port}:127.0.0.1:{port} …`)."
            );
        }
        println!("Waiting for you to finish (Ctrl-C cancels)…");
        self.wait_for(&started.flow_id).await?;
        let after = self.snapshot().await?;
        let server = after
            .servers
            .iter()
            .find(|s| s.id == server.id)
            .unwrap_or(server);
        println!(
            "Signed in to {}. Agents see it as {}.",
            full_name(server),
            agent_name(server)
        );
        Ok(())
    }

    /// Wait for sign-in flow `flow_id` to finish: Ctrl-C cancels it.
    async fn wait_for(&self, flow_id: &str) -> anyhow::Result<()> {
        let deadline = Instant::now() + AUTH_WAIT;
        let ctrl_c = tokio::signal::ctrl_c();
        tokio::pin!(ctrl_c);
        loop {
            tokio::select! {
                _ = &mut ctrl_c => {
                    self.cancel(flow_id).await;
                    anyhow::bail!("Sign-in cancelled.");
                }
                _ = tokio::time::sleep(AUTH_POLL_INTERVAL) => {}
            }
            if Instant::now() >= deadline {
                self.cancel(flow_id).await;
                anyhow::bail!("Sign-in timed out: nothing came back from the browser.");
            }
            let poll: PollMcpAuthResponse = self
                .call(
                    methods::POLL_MCP_AUTH,
                    &PollMcpAuthRequest {
                        flow_id: flow_id.to_owned(),
                    },
                )
                .await?;
            match poll.status {
                McpAuthFlowStatus::Pending => {}
                McpAuthFlowStatus::Completed => return Ok(()),
                McpAuthFlowStatus::Cancelled => anyhow::bail!("Sign-in was cancelled."),
                McpAuthFlowStatus::Failed => anyhow::bail!(
                    "Sign-in failed: {}",
                    poll.error
                        .as_deref()
                        .map(str::trim)
                        .filter(|e| !e.is_empty())
                        .unwrap_or("the server didn't say why")
                ),
            }
        }
    }

    async fn cancel(&self, flow_id: &str) {
        let _: anyhow::Result<serde_json::Value> = self
            .call(
                methods::CANCEL_MCP_AUTH,
                &CancelMcpAuthRequest {
                    flow_id: flow_id.to_owned(),
                },
            )
            .await;
    }

    async fn set_enabled(&self, query: &str, enabled: bool) -> anyhow::Result<()> {
        let snapshot = self.snapshot().await?;
        let server = resolve(&snapshot.servers, query).map_err(anyhow::Error::msg)?;
        let after: McpServersSnapshot = self
            .call(
                methods::SET_MCP_SERVER_ENABLED,
                &SetMcpServerEnabledRequest {
                    id: server.id.clone(),
                    enabled,
                },
            )
            .await?;
        let server = after
            .servers
            .iter()
            .find(|s| s.id == server.id)
            .unwrap_or(server);
        if enabled {
            println!(
                "{} is on: agents get it as {}.",
                full_name(server),
                agent_name(server)
            );
        } else {
            println!(
                "{} is off: agents don't get it. `zeron mcp on {}` turns it back on.",
                full_name(server),
                handle(server)
            );
        }
        Ok(())
    }
}

/// The person's own OAuth app for a sign-in, if they gave one: its secret
/// read from the environment variable they named.
#[derive(Default)]
struct OwnApp {
    id: Option<String>,
    secret: Option<String>,
}

impl OwnApp {
    fn from_args(args: &McpClientArgs) -> anyhow::Result<Self> {
        let secret = match args.client_secret_env.as_deref() {
            Some(var) => Some(secret_env(var, "its OAuth app's client secret")?),
            None => None,
        };
        Ok(Self {
            id: args.client_id.clone(),
            secret,
        })
    }
}

/// Asks before removing `server` (on a terminal; anywhere else it takes
/// `--yes`).
fn confirm_remove(server: &McpServerView) -> anyhow::Result<bool> {
    let name = full_name(server);
    if !std::io::stdin().is_terminal() {
        anyhow::bail!("Removing {name} takes a yes: run it again with --yes.");
    }
    let from = match server.storage {
        McpStorage::Vault => "from Zeron Vault, on every device",
        McpStorage::Local => "from this device",
    };
    print!(
        "Remove {name} {from}? Agents lose {}, and its sign-in goes with it. [y/N] ",
        agent_name(server)
    );
    std::io::stdout().flush()?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    Ok(matches!(answer.trim().to_lowercase().as_str(), "y" | "yes"))
}

/// A bearer token from the environment variable named (never the command
/// line, where other processes and the shell's history see it).
fn token_from_env(var: Option<&str>, name: &str) -> anyhow::Result<String> {
    match var {
        Some(var) => secret_env(var, &format!("{name}'s token")),
        None => anyhow::bail!(
            "{name} signs in with a token: --token-env names the environment variable that holds it."
        ),
    }
}

/// A secret's value from environment variable `var`: set, and not blank.
fn secret_env(var: &str, what: &str) -> anyhow::Result<String> {
    let value = passed_env(var)?;
    match value.trim() {
        "" => anyhow::bail!("{var} is empty: it should hold {what}."),
        value => Ok(value.to_owned()),
    }
}

/// Environment variable `var`'s value, as this shell has it.
fn passed_env(var: &str) -> anyhow::Result<String> {
    if var.is_empty() || var.contains(['=', '\0']) {
        anyhow::bail!("{var:?} isn't an environment variable's name.");
    }
    std::env::var(var).map_err(|error| match error {
        std::env::VarError::NotPresent => anyhow::anyhow!("{var} isn't set in this shell."),
        std::env::VarError::NotUnicode(_) => anyhow::anyhow!("{var} isn't text."),
    })
}

/// What to say when a server signs in only with an OAuth app of the
/// person's own (the engine answered [`MCP_CLIENT_REQUIRED`]).
fn client_required(name: &str, redirect_uri: &str, retry: &str) -> String {
    format!(
        "{name} signs in only with an OAuth app of your own: it can't register Zeron's by itself.\n\
         Make one in its developer settings, allowing the redirect URL\n\n  {redirect_uri}\n\n\
         then run\n\n  {retry} --client-id <ID> --client-secret-env <VAR>\n\n\
         (<VAR> names the environment variable holding its client secret; leave it out for an app without one)."
    )
}

fn find_preset<'a>(snapshot: &'a McpServersSnapshot, id: &str) -> anyhow::Result<&'a McpPreset> {
    let id = id.trim();
    snapshot
        .presets
        .iter()
        .find(|p| p.id.eq_ignore_ascii_case(id))
        .ok_or_else(|| {
            let ids: Vec<&str> = snapshot.presets.iter().map(|p| p.id.as_str()).collect();
            anyhow::anyhow!(
                "Zeron has no server {id:?}. Its servers: {}. Add another by its address with --url, or one this device runs with --command.",
                ids.join(", ")
            )
        })
}

/// The server `query` names, among `servers`: its id, the name agents see
/// it by, or `service/account` (a preset's id or a server's name, and its
/// account's name), whichever case. More than one is an error naming them,
/// as is none.
fn resolve<'a>(servers: &'a [McpServerView], query: &str) -> Result<&'a McpServerView, String> {
    let query = query.trim();
    let bare = query.strip_prefix("mcp:").unwrap_or(query);
    if bare.is_empty() {
        return Err(
            "Name a server: its id, the name agents see it by, or service/account (`zeron mcp list` names them)."
                .into(),
        );
    }
    let split = bare.split_once('/');
    let found: Vec<&McpServerView> = servers
        .iter()
        .filter(|s| {
            s.id.eq_ignore_ascii_case(bare)
                || (!s.agent_name.is_empty() && s.agent_name.eq_ignore_ascii_case(bare))
                || split.is_some_and(|(service, account)| {
                    is_service(s, service)
                        && s.account_label
                            .as_deref()
                            .is_some_and(|l| same_text(l, account))
                })
        })
        .collect();
    match found[..] {
        [one] => Ok(one),
        [] => {
            let service = split.map_or(bare, |(service, _)| service);
            let accounts: Vec<String> = servers
                .iter()
                .filter(|s| is_service(s, service))
                .map(|s| format!("\n  {}", describe(s)))
                .collect();
            if accounts.is_empty() {
                Err(format!(
                    "No MCP server is named {query}. `zeron mcp list` names them."
                ))
            } else {
                Err(format!(
                    "No MCP server is named {query}. Did you mean one of these?{}",
                    accounts.concat()
                ))
            }
        }
        _ => Err(format!(
            "{query} could be any of these: name it by its id.{}",
            found
                .iter()
                .map(|s| format!("\n  {}", describe(s)))
                .collect::<String>()
        )),
    }
}

/// Whether `server` is of service `service`: a preset's id, or a custom
/// server's name.
fn is_service(server: &McpServerView, service: &str) -> bool {
    server
        .preset
        .as_deref()
        .is_some_and(|p| p.eq_ignore_ascii_case(service.trim()))
        || same_text(&server.name, service)
}

/// `linear/Work  (agents see it as linear_work; id linear)`.
fn describe(server: &McpServerView) -> String {
    let service = server
        .preset
        .as_deref()
        .filter(|p| !p.is_empty())
        .unwrap_or(&server.name);
    let reference = match server.account_label.as_deref() {
        Some(label) => format!("{service}/{label}"),
        None => full_name(server),
    };
    format!(
        "{reference}  (agents see it as {}; id {})",
        agent_name(server),
        server.id
    )
}

fn same_text(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

/// The service a server is an account of: its preset's, else its name's (two
/// custom servers of one name are two accounts of it), as the engine groups
/// them for agents' names.
fn service_key(server: &McpServerView) -> String {
    server
        .preset
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .unwrap_or(&server.name)
        .trim()
        .to_lowercase()
}

fn request_service_key(request: &UpsertMcpServerRequest) -> String {
    request
        .preset
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .unwrap_or(&request.name)
        .trim()
        .to_lowercase()
}

/// Another account of `server`'s service already named `account` (its
/// name as it has it), if one is: two of a service can't share a name.
fn account_taken(
    servers: &[McpServerView],
    server: &McpServerView,
    account: &str,
) -> Option<String> {
    if account.trim().is_empty() {
        return None;
    }
    let key = service_key(server);
    servers
        .iter()
        .filter(|s| s.id != server.id && service_key(s) == key)
        .find_map(|s| s.account_label.as_deref().filter(|l| same_text(l, account)))
        .map(str::to_owned)
}

/// `Linear (Work)`, or `Linear` for an account without a name.
fn full_name(server: &McpServerView) -> String {
    match server
        .account_label
        .as_deref()
        .map(str::trim)
        .filter(|l| !l.is_empty())
    {
        Some(label) => format!("{} ({label})", server.name),
        None => server.name.clone(),
    }
}

/// What to type to name it again: the name agents see it by, else its id.
fn handle(server: &McpServerView) -> &str {
    if server.agent_name.is_empty() {
        &server.id
    } else {
        &server.agent_name
    }
}

fn agent_name(server: &McpServerView) -> &str {
    if server.agent_name.is_empty() {
        "(no name yet)"
    } else {
        &server.agent_name
    }
}

/// The service's name: its preset's, else the server's own.
fn service_name(snapshot: &McpServersSnapshot, server: &McpServerView) -> String {
    server
        .preset
        .as_deref()
        .and_then(|id| snapshot.presets.iter().find(|p| p.id == id))
        .map_or_else(|| server.name.clone(), |p| p.name.clone())
}

fn kept_in(server: &McpServerView) -> &'static str {
    match server.storage {
        McpStorage::Vault => "in Zeron Vault",
        McpStorage::Local => "on this device",
    }
}

fn status_text(server: &McpServerView) -> &'static str {
    if !server.enabled {
        return "off";
    }
    match server.status {
        McpServerStatus::Connected => "connected",
        McpServerStatus::NeedsAuth => "not signed in",
        McpServerStatus::Authorizing => "signing in",
        McpServerStatus::ReauthRequired => "sign in again",
        McpServerStatus::Error => "error",
        McpServerStatus::Disabled => "off",
    }
}

/// `zeron mcp list`: a table of the servers, a service's accounts together
/// (the service named on its first), then the presets not added yet.
fn list_text(snapshot: &McpServersSnapshot) -> String {
    let mut out = String::new();
    if snapshot.servers.is_empty() {
        out.push_str("No MCP servers yet.\n");
    } else {
        let mut groups: Vec<(String, Vec<&McpServerView>)> = Vec::new();
        for server in &snapshot.servers {
            let key = service_key(server);
            match groups.iter_mut().find(|(k, _)| *k == key) {
                Some((_, members)) => members.push(server),
                None => groups.push((key, vec![server])),
            }
        }
        let header =
            ["SERVICE", "ACCOUNT", "AGENT NAME", "STATUS", "KEPT", "ID"].map(str::to_owned);
        let mut rows: Vec<([String; 6], Option<String>)> = vec![(header, None)];
        for (_, members) in &groups {
            for (place, server) in members.iter().enumerate() {
                let service = if place == 0 {
                    service_name(snapshot, server)
                } else {
                    String::new()
                };
                let error = server
                    .last_error
                    .as_deref()
                    .filter(|e| {
                        server.enabled
                            && server.status == McpServerStatus::Error
                            && !e.trim().is_empty()
                    })
                    .map(|e| e.trim().to_owned());
                rows.push((
                    [
                        service,
                        server.account_label.clone().unwrap_or_else(|| "-".into()),
                        agent_name(server).to_owned(),
                        status_text(server).to_owned(),
                        match server.storage {
                            McpStorage::Vault => "Zeron Vault".into(),
                            McpStorage::Local => "this device".into(),
                        },
                        server.id.clone(),
                    ],
                    error,
                ));
            }
        }
        let mut widths = [0usize; 6];
        for (cells, _) in &rows {
            for (width, cell) in widths.iter_mut().zip(cells) {
                *width = (*width).max(cell.chars().count());
            }
        }
        for (cells, error) in &rows {
            let mut line = String::new();
            for (i, (cell, width)) in cells.iter().zip(widths).enumerate() {
                if i + 1 == cells.len() {
                    line.push_str(cell);
                } else {
                    line.push_str(cell);
                    line.push_str(&" ".repeat(width - cell.chars().count() + 2));
                }
            }
            out.push_str(line.trim_end());
            out.push('\n');
            if let Some(error) = error {
                out.push_str(&format!("{}{error}\n", " ".repeat(widths[0] + 2)));
            }
        }
    }
    let added: HashSet<&str> = snapshot
        .servers
        .iter()
        .filter_map(|s| s.preset.as_deref())
        .collect();
    let to_add: Vec<&McpPreset> = snapshot
        .presets
        .iter()
        .filter(|p| !added.contains(p.id.as_str()))
        .collect();
    if !to_add.is_empty() {
        out.push_str("\nAdd with `zeron mcp add <id>`:\n");
        let width = to_add
            .iter()
            .map(|p| p.id.chars().count())
            .max()
            .unwrap_or(0);
        for preset in to_add {
            let own_app = if preset.client_required {
                " (with an OAuth app of your own)"
            } else {
                ""
            };
            out.push_str(&format!(
                "  {:width$}  {}: {}{own_app}\n",
                preset.id, preset.name, preset.description
            ));
        }
    }
    if !snapshot.vault_available {
        out.push_str("\nZeron Vault isn't available here (sign in to Zeron for it): servers are kept on this device.\n");
    }
    out
}

/// A sign-in page to open: `https://`, or plain `http://` only to this
/// computer, and nothing a terminal or a shell would take for more.
fn openable_auth_url(url: &str) -> bool {
    if url.is_empty() || url.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return false;
    }
    if let Some(rest) = url.strip_prefix("https://") {
        return !rest.is_empty();
    }
    ["http://127.0.0.1", "http://localhost"]
        .iter()
        .any(|prefix| {
            url.strip_prefix(prefix)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with([':', '/', '?']))
        })
}

/// Opens the sign-in page in this computer's browser, if it has one to
/// open: on Linux only with a desktop (over SSH, `xdg-open` might start a
/// terminal browser that takes this terminal).
fn open_in_browser(url: &str) -> bool {
    if cfg!(target_os = "linux")
        && std::env::var_os("DISPLAY").is_none()
        && std::env::var_os("WAYLAND_DISPLAY").is_none()
    {
        return false;
    }
    open::that_detached(url).is_ok()
}

/// The engine's message, without transport wrapping.
fn plain(error: zeron_rpc::RpcError) -> String {
    match error {
        zeron_rpc::RpcError::Failed(message) => message,
        zeron_rpc::RpcError::UnknownMethod(_) => {
            "This Zeron is too old to manage MCP servers from the command line. Update it first."
                .into()
        }
        zeron_rpc::RpcError::Closed => "Zeron closed the connection (did it quit?).".into(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(
        id: &str,
        preset: Option<&str>,
        name: &str,
        label: Option<&str>,
        agent: &str,
    ) -> McpServerView {
        McpServerView {
            id: id.into(),
            name: name.into(),
            preset: preset.map(Into::into),
            account_label: label.map(Into::into),
            agent_name: agent.into(),
            enabled: true,
            ..Default::default()
        }
    }

    fn servers() -> Vec<McpServerView> {
        vec![
            server(
                "linear",
                Some("linear"),
                "Linear",
                Some("Work"),
                "linear_work",
            ),
            server(
                "linear-2",
                Some("linear"),
                "Linear",
                Some("Personal"),
                "linear_personal",
            ),
            server("context7", Some("context7"), "Context7", None, "context7"),
            server("files", None, "Files", None, "files"),
        ]
    }

    fn id(found: Result<&McpServerView, String>) -> String {
        found
            .map(|s| s.id.clone())
            .unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn a_server_is_found_by_its_id_agent_name_or_service_and_account() {
        let servers = servers();
        assert_eq!(id(resolve(&servers, "linear-2")), "linear-2");
        assert_eq!(id(resolve(&servers, "mcp:linear-2")), "linear-2");
        assert_eq!(id(resolve(&servers, "linear_personal")), "linear-2");
        assert_eq!(id(resolve(&servers, "LINEAR_PERSONAL")), "linear-2");
        assert_eq!(id(resolve(&servers, "linear/Personal")), "linear-2");
        assert_eq!(id(resolve(&servers, "Linear/personal")), "linear-2");
        assert_eq!(id(resolve(&servers, " linear/work ")), "linear");
        // An id is one server's whichever account it is.
        assert_eq!(id(resolve(&servers, "linear")), "linear");
        assert_eq!(id(resolve(&servers, "context7")), "context7");
        // A custom server's service is its name.
        assert_eq!(id(resolve(&servers, "files")), "files");
    }

    #[test]
    fn an_ambiguous_name_names_the_servers_it_could_be() {
        let mut servers = servers();
        // An id that's another server's agent name.
        servers.push(server(
            "linear_work",
            None,
            "Linear Work",
            None,
            "linear_work_2",
        ));
        let error = resolve(&servers, "linear_work").unwrap_err();
        assert!(error.contains("could be any of these"), "{error}");
        assert!(
            error.contains("linear/Work  (agents see it as linear_work; id linear)"),
            "{error}"
        );
        assert!(
            error.contains("Linear Work  (agents see it as linear_work_2; id linear_work)"),
            "{error}"
        );
        // Its other names are still one server's.
        assert_eq!(id(resolve(&servers, "linear_work_2")), "linear_work");
        assert_eq!(id(resolve(&servers, "linear/work")), "linear");
    }

    #[test]
    fn no_match_suggests_the_service_s_accounts() {
        let mut servers = servers();
        servers.retain(|s| s.id != "linear");
        servers.push(server(
            "linear-3",
            Some("linear"),
            "Linear",
            None,
            "linear_2",
        ));
        let error = resolve(&servers, "linear").unwrap_err();
        assert!(error.contains("Did you mean one of these?"), "{error}");
        assert!(error.contains("linear/Personal"), "{error}");
        assert!(
            error.contains("Linear  (agents see it as linear_2; id linear-3)"),
            "{error}"
        );
        let error = resolve(&servers, "linear/Home").unwrap_err();
        assert!(error.contains("linear/Personal"), "{error}");
        let error = resolve(&servers, "github").unwrap_err();
        assert_eq!(
            error,
            "No MCP server is named github. `zeron mcp list` names them."
        );
        assert!(resolve(&servers, "  ").is_err());
        assert!(resolve(&[], "linear").is_err());
    }

    #[test]
    fn two_accounts_of_a_service_cant_share_a_name() {
        let servers = servers();
        let work = &servers[0];
        assert_eq!(
            account_taken(&servers, work, "personal").as_deref(),
            Some("Personal")
        );
        // Its own name, a new one, or none, are fine.
        assert_eq!(account_taken(&servers, work, "work"), None);
        assert_eq!(account_taken(&servers, work, "Home"), None);
        assert_eq!(account_taken(&servers, work, ""), None);
        // Another service's account names are its own.
        assert_eq!(account_taken(&servers, &servers[2], "Work"), None);
    }

    #[test]
    fn the_list_groups_a_service_s_accounts_and_offers_what_s_left() {
        let mut servers = servers();
        servers[0].status = McpServerStatus::Connected;
        servers[0].storage = McpStorage::Vault;
        servers[1].status = McpServerStatus::Error;
        servers[1].last_error = Some("token expired".into());
        servers[2].enabled = false;
        servers[3].status = McpServerStatus::NeedsAuth;
        let preset = |id: &str, name: &str, client_required| McpPreset {
            id: id.into(),
            name: name.into(),
            description: format!("{name} things."),
            client_required,
            ..Default::default()
        };
        let snapshot = McpServersSnapshot {
            vault_available: true,
            presets: vec![
                preset("linear", "Linear", false),
                preset("notion", "Notion", false),
                preset("context7", "Context7", false),
                preset("slack", "Slack", true),
            ],
            servers,
            ..Default::default()
        };
        assert_eq!(
            list_text(&snapshot),
            "\
SERVICE   ACCOUNT   AGENT NAME       STATUS         KEPT         ID
Linear    Work      linear_work      connected      Zeron Vault  linear
          Personal  linear_personal  error          this device  linear-2
          token expired
Context7  -         context7         off            this device  context7
Files     -         files            not signed in  this device  files

Add with `zeron mcp add <id>`:
  notion  Notion: Notion things.
  slack   Slack: Slack things. (with an OAuth app of your own)
"
        );
        let empty = McpServersSnapshot::default();
        assert_eq!(
            list_text(&empty),
            "No MCP servers yet.\n\nZeron Vault isn't available here (sign in to Zeron for it): servers are kept on this device.\n"
        );
    }

    #[test]
    fn only_web_sign_in_pages_are_opened() {
        assert!(openable_auth_url("https://mcp.linear.app/authorize?x=1"));
        assert!(openable_auth_url("http://127.0.0.1:38417/callback"));
        assert!(openable_auth_url("http://localhost/x"));
        assert!(!openable_auth_url("https://"));
        assert!(!openable_auth_url("http://example.com"));
        assert!(!openable_auth_url("http://localhost.evil.com"));
        assert!(!openable_auth_url("file:///etc/passwd"));
        assert!(!openable_auth_url("https://x.dev/\u{1b}[2J"));
        assert!(!openable_auth_url("https://x.dev/ a"));
    }

    #[test]
    fn a_secret_comes_from_the_environment_never_blank() {
        // (A name no other test sets.)
        let var = "ZERON_MCP_CLI_TEST_SECRET";
        assert!(
            secret_env(var, "a token")
                .unwrap_err()
                .to_string()
                .contains("isn't set")
        );
        assert!(passed_env("A=B").is_err());
        assert!(passed_env("").is_err());
        assert!(
            token_from_env(None, "Example")
                .unwrap_err()
                .to_string()
                .contains("--token-env")
        );
    }
}
