//! Changes to the MCP servers that agents ask for (Zeron MCP's
//! `request_mcp_change`), waiting for the person.
//!
//! An agent's Zeron tools run without asking in every chat, and a server
//! added here reaches every agent on every device (the vault syncs it): so
//! an agent never changes the servers itself. It asks; the request waits in
//! this engine (never synced, never over the peer relay) and shows in this
//! device's app (Settings → MCP, and a notice in the window's corner), which
//! alone answers it (`ResolveMcpRequest`: local only, not an agent's). A
//! request carries no secret: a server that signs in does so in the
//! person's browser once they allow it, and one that takes a key is theirs
//! to add in Settings → MCP. Allowed, it's done the way the page does it.

use std::time::Duration;

use zeron_proto::{
    McpAuthKind, McpChange, McpRequestStatus, McpRequestView, McpTransport,
    RequestMcpChangeParams, UpsertMcpServerRequest,
};

use super::{McpServers, lock, requested_account_label, strip_mcp_prefix};
use crate::{new_id, now_ms};

/// At most this many wait at once: an agent looping on a request can't
/// bury the person in them.
const MAX_PENDING: usize = 8;
/// How long an answered request stays readable for the agent that asked
/// (`GetMcpRequest`); a pending one waits as long as the engine runs.
const KEEP_ANSWERED: Duration = Duration::from_secs(30 * 60);
/// What a request may say of itself, at most.
const MAX_REASON: usize = 300;
const MAX_NAME: usize = 80;
const MAX_ARG: usize = 1_000;
const MAX_ARGS: usize = 64;

impl McpServers {
    /// An agent's request: checked, then waiting for the person (the same
    /// one waiting already, when it asks again).
    pub async fn request_change(
        &self,
        params: RequestMcpChangeParams,
    ) -> Result<McpRequestView, String> {
        // Someone to ask: a Zeron window following this engine's servers.
        // With none (a Cloud machine, a headless server), a request would
        // wait for no one, so the agent is told whom to ask instead.
        if self.windows.load(std::sync::atomic::Ordering::Acquire) == 0 {
            return Err(
                "No Zeron window is open on this device to ask the person in. Ask them to add or \
                 change it in Settings → MCP on their computer (it syncs here), or with \
                 `zeron mcp` in a terminal."
                    .into(),
            );
        }
        let title = self.describe(&params.change)?;
        let reason = params
            .reason
            .map(|r| r.trim().chars().take(MAX_REASON).collect::<String>())
            .filter(|r| !r.is_empty());
        let view = {
            let mut requests = lock(&self.requests);
            forget_answered(&mut requests);
            if let Some(same) = requests
                .iter()
                .find(|r| r.status == McpRequestStatus::Pending && r.change == params.change)
            {
                return Ok(same.clone());
            }
            let waiting = requests
                .iter()
                .filter(|r| r.status == McpRequestStatus::Pending)
                .count();
            if waiting >= MAX_PENDING {
                return Err(format!(
                    "{waiting} changes already wait for the person: ask them to answer those in \
                     Settings → MCP first"
                ));
            }
            let view = McpRequestView {
                id: new_id(),
                change: params.change,
                reason,
                chat_id: params.chat_id.filter(|c| !c.trim().is_empty()),
                title,
                status: McpRequestStatus::Pending,
                server_id: None,
                error: None,
                created_at: now_ms(),
            };
            requests.push(view.clone());
            view
        };
        tracing::info!(request = %view.id, title = %view.title, "mcp: an agent asks for a change");
        self.publish_snapshot();
        Ok(view)
    }

    /// A request as it stands, for the agent that asked.
    pub fn get_request(&self, id: &str) -> Result<McpRequestView, String> {
        let mut requests = lock(&self.requests);
        forget_answered(&mut requests);
        requests
            .iter()
            .find(|r| r.id == id)
            .cloned()
            .ok_or_else(|| format!("no MCP request '{id}' (answered over half an hour ago, or never made)"))
    }

    /// The person's answer (this device's app alone): declined, or allowed
    /// and done as the page does it.
    pub async fn resolve_request(&self, id: &str, approve: bool) -> Result<McpRequestView, String> {
        let change = {
            let requests = lock(&self.requests);
            let request = requests
                .iter()
                .find(|r| r.id == id)
                .ok_or_else(|| format!("no MCP request '{id}'"))?;
            // Answered, or an answer (a second click, another window) under
            // way: never done twice.
            if request.status != McpRequestStatus::Pending || !lock(&self.answering).insert(id.to_owned()) {
                return Err("that request was already answered".into());
            }
            request.change.clone()
        };
        // (Given up with the answer, should its asker go away before it's
        // done: the request then waits as it did.)
        struct Answering<'a>(&'a McpServers, &'a str);
        impl Drop for Answering<'_> {
            fn drop(&mut self) {
                lock(&self.0.answering).remove(self.1);
            }
        }
        let _answering = Answering(self, id);
        let outcome = if approve {
            Some(self.apply(&change).await)
        } else {
            None
        };
        let view = {
            let mut requests = lock(&self.requests);
            let request = requests
                .iter_mut()
                .find(|r| r.id == id)
                .ok_or_else(|| format!("no MCP request '{id}'"))?;
            match outcome {
                None => request.status = McpRequestStatus::Declined,
                Some(Ok(server_id)) => {
                    request.status = McpRequestStatus::Approved;
                    request.server_id = Some(server_id);
                }
                Some(Err(error)) => {
                    request.status = McpRequestStatus::Failed;
                    request.error = Some(error);
                }
            }
            // (Answered now: kept from here for `KEEP_ANSWERED`.)
            request.created_at = now_ms();
            request.clone()
        };
        self.publish_snapshot();
        Ok(view)
    }

    /// The change done: the server it made or changed.
    async fn apply(&self, change: &McpChange) -> Result<String, String> {
        match change {
            McpChange::Add { .. } => {
                let req = self.upsert_for(change)?;
                let before: std::collections::HashSet<String> =
                    self.snapshot().servers.into_iter().map(|s| s.id).collect();
                let after = self.upsert_server(req).await?;
                after
                    .servers
                    .into_iter()
                    .map(|s| s.id)
                    .find(|id| !before.contains(id))
                    .ok_or_else(|| "the server was saved, but isn't listed".to_owned())
            }
            McpChange::Remove { server } => {
                let id = self.find_server(server)?;
                self.delete_server(&id).await?;
                Ok(id)
            }
            McpChange::SetEnabled { server, enabled } => {
                let id = self.find_server(server)?;
                self.set_server_enabled(&id, *enabled).await?;
                Ok(id)
            }
        }
    }

    /// What a request is to, in words, once it's checked: a preset that
    /// exists, a remote server on https that signs in (or needs nothing), a
    /// command (shown to the person as it is), an account name that is one;
    /// a server to remove or switch that's here.
    fn describe(&self, change: &McpChange) -> Result<String, String> {
        match change {
            McpChange::Add {
                preset,
                name,
                url,
                auth_kind,
                command,
                args,
                account_label,
            } => {
                if let Some(name) = name.as_deref().filter(|n| !n.trim().is_empty()) {
                    server_name(Some(name))?;
                }
                let label = requested_account_label(&UpsertMcpServerRequest {
                    account_label: account_label.clone(),
                    ..Default::default()
                })?
                .flatten();
                let base = match (preset.as_deref(), url.as_deref(), command.as_deref()) {
                    (Some(preset), None, None) => self
                        .presets
                        .iter()
                        .find(|p| p.id == preset)
                        .map(|p| name.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| p.name.clone()))
                        .ok_or_else(|| {
                            let known: Vec<&str> = self.presets.iter().map(|p| p.id.as_str()).collect();
                            format!("no MCP preset '{preset}' (presets: {})", known.join(", "))
                        })?,
                    (None, Some(url), None) => {
                        let parsed = reqwest::Url::parse(url.trim())
                            .map_err(|e| format!("invalid URL '{url}': {e}"))?;
                        if parsed.scheme() != "https" {
                            return Err("a remote MCP server's URL must use https://".into());
                        }
                        match auth_kind.unwrap_or(McpAuthKind::OAuth) {
                            McpAuthKind::OAuth | McpAuthKind::None => {}
                            McpAuthKind::Bearer | McpAuthKind::Headers => {
                                return Err(
                                    "a server that takes a key or headers is the person's to add, \
                                     in Settings → MCP: ask them to"
                                        .into(),
                                );
                            }
                        }
                        server_name(name.as_deref())?
                    }
                    (None, None, Some(command)) => {
                        if command.trim().is_empty() {
                            return Err("give the command to run".into());
                        }
                        if args.len() > MAX_ARGS || args.iter().any(|a| a.len() > MAX_ARG) {
                            return Err("too many or too long arguments".into());
                        }
                        server_name(name.as_deref())?
                    }
                    _ => {
                        return Err(
                            "give exactly one of preset, url (a remote server) or command (one run \
                             on this device)"
                                .into(),
                        );
                    }
                };
                Ok(match label {
                    Some(account) => format!("{base} · {account}"),
                    None => base,
                })
            }
            McpChange::Remove { server } | McpChange::SetEnabled { server, .. } => {
                let id = self.find_server(server)?;
                let view = self
                    .snapshot()
                    .servers
                    .into_iter()
                    .find(|s| s.id == id)
                    .ok_or_else(|| format!("no MCP server '{server}'"))?;
                Ok(match view.account_label {
                    Some(account) => format!("{} · {account}", view.name),
                    None => view.name,
                })
            }
        }
    }

    /// The upsert an allowed `Add` makes: a preset's address and sign-in, or
    /// what was asked; kept in the vault where there is one (a command
    /// stays on this device).
    fn upsert_for(&self, change: &McpChange) -> Result<UpsertMcpServerRequest, String> {
        let McpChange::Add {
            preset,
            name,
            url,
            auth_kind,
            command,
            args,
            account_label,
        } = change
        else {
            return Err("not an addition".into());
        };
        let mut req = UpsertMcpServerRequest {
            account_label: account_label.clone(),
            enabled: Some(true),
            ..Default::default()
        };
        if let Some(preset) = preset {
            let p = self
                .presets
                .iter()
                .find(|p| &p.id == preset)
                .ok_or_else(|| format!("no MCP preset '{preset}'"))?;
            req.name = name.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| p.name.clone());
            req.preset = Some(p.id.clone());
            req.transport = p.transport;
            req.url = Some(p.url.clone());
            req.auth_kind = p.auth_kind;
        } else if let Some(url) = url {
            req.name = server_name(name.as_deref())?;
            req.transport = McpTransport::Http;
            req.url = Some(url.trim().to_owned());
            req.auth_kind = auth_kind.unwrap_or(McpAuthKind::OAuth);
        } else if let Some(command) = command {
            req.name = server_name(name.as_deref())?;
            req.transport = McpTransport::Stdio;
            req.command = Some(command.trim().to_owned());
            req.args = args.clone();
            req.auth_kind = McpAuthKind::None;
            req.local_only = true;
        }
        Ok(req)
    }

    /// A server by its id or the name agents know it by.
    fn find_server(&self, key: &str) -> Result<String, String> {
        let key = strip_mcp_prefix(key.trim());
        let servers = self.snapshot().servers;
        servers
            .iter()
            .find(|s| s.id == key)
            .or_else(|| servers.iter().find(|s| s.agent_name.eq_ignore_ascii_case(key)))
            .map(|s| s.id.clone())
            .ok_or_else(|| {
                let known: Vec<&str> = servers.iter().map(|s| s.agent_name.as_str()).collect();
                format!("no MCP server '{key}' (servers: {})", known.join(", "))
            })
    }
}

/// A server's name as asked: one line, not too long.
fn server_name(name: Option<&str>) -> Result<String, String> {
    let name = name.map(str::trim).unwrap_or_default();
    if name.is_empty() {
        return Err("give the server a name".into());
    }
    if name.chars().count() > MAX_NAME || name.chars().any(char::is_control) {
        return Err(format!("a server's name is one line of at most {MAX_NAME} characters"));
    }
    Ok(name.to_owned())
}

/// Answered requests older than [`KEEP_ANSWERED`] go.
fn forget_answered(requests: &mut Vec<McpRequestView>) {
    let horizon = now_ms() - KEEP_ANSWERED.as_millis() as i64;
    requests.retain(|r| r.status == McpRequestStatus::Pending || r.created_at >= horizon);
}

#[cfg(test)]
mod tests {
    use zeron_proto::{McpChange, McpRequestStatus, RequestMcpChangeParams};

    use super::MAX_PENDING;
    use crate::mcp_servers::McpServers;
    use crate::profile::EngineProfile;

    fn manager() -> (tempfile::TempDir, std::sync::Arc<McpServers>) {
        let dir = tempfile::tempdir().unwrap();
        let profile = EngineProfile::local(dir.path()).unwrap();
        let mgr = McpServers::new(&profile, dir.path(), "dev-local", None);
        (dir, mgr)
    }

    /// A Zeron window following the servers: someone to ask.
    fn window(mgr: &McpServers) -> futures::stream::BoxStream<'static, serde_json::Value> {
        mgr.watch_stream(zeron_rpc::Caller::Local)
    }

    fn ask(change: McpChange) -> RequestMcpChangeParams {
        RequestMcpChangeParams {
            change,
            reason: Some("  to look up the docs  ".into()),
            chat_id: Some("chat-1".into()),
        }
    }

    fn add_preset(preset: &str, account: Option<&str>) -> McpChange {
        McpChange::Add {
            preset: Some(preset.into()),
            name: None,
            url: None,
            auth_kind: None,
            command: None,
            args: Vec::new(),
            account_label: account.map(Into::into),
        }
    }

    /// Two windows' answers at once: done by one.
    #[tokio::test]
    async fn a_request_is_done_once_however_many_answer() {
        let (_dir, mgr) = manager();
        let _window = window(&mgr);
        let asked = mgr.request_change(ask(add_preset("context7", None))).await.unwrap();
        let (a, b) = tokio::join!(mgr.resolve_request(&asked.id, true), mgr.resolve_request(&asked.id, true));
        assert_eq!([a.is_ok(), b.is_ok()].iter().filter(|ok| **ok).count(), 1);
        assert_eq!(mgr.snapshot().servers.len(), 1);
        // One given up before it's done leaves it waiting.
        let asked = mgr.request_change(ask(add_preset("linear", None))).await.unwrap();
        drop(mgr.resolve_request(&asked.id, true));
        assert!(mgr.resolve_request(&asked.id, false).await.is_ok());
    }

    /// Only this device's window can answer, so only one is someone to ask;
    /// and what another device or an agent reads holds no key.
    #[tokio::test]
    async fn another_device_is_no_one_to_ask_and_reads_no_key() {
        use futures::StreamExt;
        use zeron_proto::{McpAuthKind, McpTransport, UpsertMcpServerRequest};
        let (_dir, mgr) = manager();
        mgr.upsert_server(UpsertMcpServerRequest {
            name: "Keyed".into(),
            transport: McpTransport::Http,
            url: Some("https://tools.example/s/path-key/mcp?key=query-key".into()),
            auth_kind: McpAuthKind::None,
            ..Default::default()
        })
        .await
        .unwrap();
        mgr.upsert_server(UpsertMcpServerRequest {
            name: "Command".into(),
            transport: McpTransport::Stdio,
            command: Some("npx".into()),
            args: vec!["--token=arg-key".into()],
            ..Default::default()
        })
        .await
        .unwrap();
        let peer = zeron_rpc::Caller::Peer { device: None };
        let mut theirs = mgr.watch_stream(peer.clone());
        theirs.next().await.unwrap();
        assert!(mgr.request_change(ask(add_preset("linear", None))).await.is_err(), "a peer isn't a window");
        let ours = window(&mgr);
        mgr.request_change(ask(add_preset("linear", None))).await.unwrap();
        for caller in [peer, zeron_rpc::Caller::Agent { chat: None }] {
            let seen = crate::mcp_servers::shown_to(&caller, mgr.snapshot());
            let text = serde_json::to_string(&seen).unwrap();
            assert!(!text.contains("-key"), "{caller:?}: {text}");
            assert!(text.contains("https://tools.example"), "{text}");
            // (The agent that asked reads how it's answered.)
            assert_eq!(seen.requests.is_empty(), matches!(caller, zeron_rpc::Caller::Peer { .. }));
        }
        let streamed = theirs.next().await.unwrap().to_string();
        assert!(!streamed.contains("-key") && !streamed.contains("\"requests\""), "{streamed}");
        drop(ours);
        assert!(mgr.request_change(ask(add_preset("notion", None))).await.is_err(), "its window closed");
    }

    /// An agent's request waits, changing nothing, until the person answers;
    /// allowed, it's done; declined, nothing happens. Asking again while it
    /// waits is the same request.
    #[tokio::test]
    async fn requests_wait_for_the_person_and_are_done_as_answered() {
        let (_dir, mgr) = manager();
        let _window = window(&mgr);
        let asked = mgr.request_change(ask(add_preset("context7", None))).await.unwrap();
        assert_eq!(asked.status, McpRequestStatus::Pending);
        assert_eq!(asked.title, "Context7");
        assert_eq!(asked.reason.as_deref(), Some("to look up the docs"));
        assert!(mgr.snapshot().servers.is_empty(), "nothing changes while it waits");
        assert_eq!(mgr.snapshot().requests.len(), 1);
        let again = mgr.request_change(ask(add_preset("context7", None))).await.unwrap();
        assert_eq!(again.id, asked.id);

        let done = mgr.resolve_request(&asked.id, true).await.unwrap();
        assert_eq!(done.status, McpRequestStatus::Approved);
        let id = done.server_id.clone().unwrap();
        assert_eq!(mgr.snapshot().servers[0].id, id);
        assert_eq!(mgr.get_request(&asked.id).unwrap().status, McpRequestStatus::Approved);
        assert!(mgr.resolve_request(&asked.id, true).await.is_err(), "answered once");

        // Switched off, then removed, by the name agents know it by.
        let off = mgr
            .request_change(ask(McpChange::SetEnabled { server: "context7".into(), enabled: false }))
            .await
            .unwrap();
        assert_eq!(off.title, "Context7");
        mgr.resolve_request(&off.id, true).await.unwrap();
        assert!(!mgr.snapshot().servers[0].enabled);

        let remove = mgr
            .request_change(ask(McpChange::Remove { server: "context7".into() }))
            .await
            .unwrap();
        let declined = mgr.resolve_request(&remove.id, false).await.unwrap();
        assert_eq!(declined.status, McpRequestStatus::Declined);
        assert_eq!(mgr.snapshot().servers.len(), 1, "declined: still there");
        let remove = mgr
            .request_change(ask(McpChange::Remove { server: "context7".into() }))
            .await
            .unwrap();
        mgr.resolve_request(&remove.id, true).await.unwrap();
        assert!(mgr.snapshot().servers.is_empty());
    }

    /// What a request may not be: a server that takes a key, one off https,
    /// one asked two ways, a preset that isn't, a server that isn't here.
    #[tokio::test]
    async fn requests_that_cant_be_asked_are_refused() {
        let (_dir, mgr) = manager();
        let _window = window(&mgr);
        let custom = |url: &str, auth: Option<zeron_proto::McpAuthKind>| McpChange::Add {
            preset: None,
            name: Some("Acme".into()),
            url: Some(url.into()),
            auth_kind: auth,
            command: None,
            args: Vec::new(),
            account_label: None,
        };
        for change in [
            custom("https://mcp.acme.test/mcp", Some(zeron_proto::McpAuthKind::Bearer)),
            custom("http://mcp.acme.test/mcp", None),
            add_preset("no-such-preset", None),
            add_preset("linear", Some("two\nlines")),
            McpChange::Add {
                preset: Some("linear".into()),
                name: None,
                url: Some("https://mcp.acme.test/mcp".into()),
                auth_kind: None,
                command: None,
                args: Vec::new(),
                account_label: None,
            },
            McpChange::Remove { server: "nothing".into() },
        ] {
            assert!(mgr.request_change(ask(change.clone())).await.is_err(), "{change:?}");
        }
        assert!(mgr.snapshot().requests.is_empty());

        // A custom one that signs in, and a command here, may be asked.
        let remote = mgr
            .request_change(ask(custom("https://mcp.acme.test/mcp", None)))
            .await
            .unwrap();
        assert_eq!(remote.title, "Acme");
        let local = mgr
            .request_change(ask(McpChange::Add {
                preset: None,
                name: Some("Files".into()),
                url: None,
                auth_kind: None,
                command: Some("npx".into()),
                args: vec!["-y".into(), "@modelcontextprotocol/server-filesystem".into()],
                account_label: Some("Docs".into()),
            }))
            .await
            .unwrap();
        assert_eq!(local.title, "Files · Docs");
    }

    /// At most `MAX_PENDING` wait at once.
    #[tokio::test]
    async fn only_so_many_wait_at_once() {
        let (_dir, mgr) = manager();
        let _window = window(&mgr);
        for n in 0..MAX_PENDING {
            mgr.request_change(ask(add_preset("linear", Some(&format!("Account {n}")))))
                .await
                .unwrap();
        }
        let err = mgr
            .request_change(ask(add_preset("linear", Some("One more"))))
            .await
            .unwrap_err();
        assert!(err.contains("wait for the person"), "{err}");
    }

    /// With no Zeron window here (a Cloud machine, a headless server), a
    /// request would wait for no one: the agent is told whom to ask.
    #[tokio::test]
    async fn with_no_window_to_ask_in_the_agent_is_told_whom_to_ask() {
        let (_dir, mgr) = manager();
        let err = mgr.request_change(ask(add_preset("context7", None))).await.unwrap_err();
        assert!(err.contains("Settings → MCP"), "{err}");
        assert!(mgr.snapshot().requests.is_empty());
    }
}
