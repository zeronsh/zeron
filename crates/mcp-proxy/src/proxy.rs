use crate::{
    cache::{self, Entry},
    config::{Config, Mode},
    process::Process,
    rpc::{self, Remap},
};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    io::{self, Write},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader},
    sync::mpsc,
    time::Instant,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Cold,
    ServingCold,
    Starting,
    Warm,
    Failed,
}
enum Event {
    Client(Value),
    Server(u64, Value),
    ClientClosed,
    ServerClosed(u64, String),
}
struct Listing {
    method: &'static str,
    field: &'static str,
    items: Vec<Value>,
    cursors: HashSet<String>,
}
struct Capture {
    entry: Entry,
    before: Option<Entry>,
    lists: VecDeque<(&'static str, &'static str)>,
    active: Option<(String, Listing)>,
    dirty: bool,
    at: Instant,
}
struct Proxy {
    config: Config,
    resolved: Option<crate::npx::Resolution>,
    state: State,
    process: Option<Process>,
    child_tx: Option<mpsc::Sender<Value>>,
    events: mpsc::Sender<Event>,
    output: mpsc::Sender<Value>,
    generation: u64,
    next_id: u64,
    initialize: Option<Value>,
    initialize_id: Option<Value>,
    key: String,
    cache: Option<Entry>,
    catalog_valid: bool,
    capture: Option<Capture>,
    discovery: bool,
    discovery_at: u64,
    replay: HashMap<String, Value>,
    stop_after_capture: bool,
    passthrough: bool,
    queue: VecDeque<Value>,
    remap: Remap,
    server_requests: HashSet<String>,
    logging: Option<Value>,
    subscriptions: HashMap<String, Value>,
    roots_changed: bool,
    failed: Option<(Instant, String)>,
    started: Instant,
    last_activity: Instant,
    last_progress: Option<Instant>,
    stderr: Arc<Mutex<Vec<u8>>>,
}
fn send(tx: &mpsc::Sender<Value>, value: Value) -> io::Result<()> {
    tx.try_send(value).map_err(|e| {
        io::Error::new(
            io::ErrorKind::BrokenPipe,
            format!("MCP output unavailable: {e}"),
        )
    })
}
fn read_lines<R: AsyncRead + Unpin + Send + 'static>(
    reader: R,
    tx: mpsc::Sender<Event>,
    generation: Option<u64>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut lines = BufReader::new(reader).lines();
        loop {
            match lines.next_line().await {
                Ok(Some(line)) if line.trim().is_empty() => continue,
                Ok(Some(line)) => match serde_json::from_str::<Value>(&line) {
                    Ok(value) if value.is_object() => {
                        let event = match generation {
                            Some(g) => Event::Server(g, value),
                            None => Event::Client(value),
                        };
                        if tx.send(event).await.is_err() {
                            break;
                        }
                    }
                    _ if generation.is_none() => {
                        if tx
                            .send(Event::Client(json!({"invalid":true})))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    _ => {
                        let _ = tx
                            .send(Event::ServerClosed(
                                generation.unwrap(),
                                "invalid JSON-RPC from server".into(),
                            ))
                            .await;
                        break;
                    }
                },
                result => {
                    let reason = match result {
                        Err(e) => e.to_string(),
                        _ => "server stdout closed".into(),
                    };
                    let _ = tx
                        .send(match generation {
                            Some(g) => Event::ServerClosed(g, reason),
                            None => Event::ClientClosed,
                        })
                        .await;
                    break;
                }
            }
        }
    })
}
impl Proxy {
    fn event(&self, event: &str, fields: Value) {
        let mut v = json!({"event":event,"server":self.config.server,"at":cache::now_ms()});
        if let Some(fields) = fields.as_object() {
            v.as_object_mut().unwrap().extend(fields.clone());
        }
        let mut line = v.to_string();
        // Lifecycle data is bounded separately from the child's arbitrary diagnostics.
        if line.len() >= 4095 {
            line = json!({"event":event,"server":self.config.server.chars().take(100).collect::<String>(),"reason":"event exceeded log limit"}).to_string();
        }
        line.push('\n');
        let _ = std::io::stderr().write_all(line.as_bytes());
        if self.config.run_dir.is_some() {
            let dir = self.config.data_dir.join("logs");
            if std::fs::create_dir_all(&dir).is_ok() {
                if let Ok(mut f) = cache::private_options()
                    .append(true)
                    .create(true)
                    .open(dir.join("mcp-proxy.jsonl"))
                {
                    let _ = f.write_all(line.as_bytes());
                }
            }
        }
    }
    fn client_out(&self, value: Value) -> io::Result<()> {
        send(&self.output, value)
    }
    fn child_out(&self, value: Value) -> io::Result<()> {
        send(
            self.child_tx
                .as_ref()
                .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "server not running"))?,
            value,
        )
    }
    fn internal_id(&mut self) -> String {
        self.next_id += 1;
        format!("zp:{}", self.next_id)
    }
    fn spawn(&mut self, reason: &str) -> io::Result<()> {
        if self.process.is_some() {
            return Ok(());
        }
        self.started = Instant::now();
        self.last_activity = self.started;
        self.stderr.lock().unwrap().clear();
        let (command, args, path) = match &self.resolved {
            Some(r) => (
                r.command.to_string_lossy().into_owned(),
                r.args.clone(),
                Some(r.path.clone()),
            ),
            None => (self.config.command.clone(), self.config.args.clone(), None),
        };
        let mut process = Process::spawn(&self.config, &command, &args, path.as_deref())?;
        self.generation += 1;
        let g = self.generation;
        process.tasks.push(read_lines(
            process.child.stdout.take().unwrap(),
            self.events.clone(),
            Some(g),
        ));
        process.stderr_task(self.stderr.clone());
        let mut stdin = process.stdin.take().unwrap();
        let (tx, mut rx) = mpsc::channel::<Value>(256);
        let events = self.events.clone();
        process.tasks.push(tokio::spawn(async move {
            while let Some(value) = rx.recv().await {
                if let Err(e) = stdin.write_all(format!("{value}\n").as_bytes()).await {
                    let _ = events.send(Event::ServerClosed(g, e.to_string())).await;
                    break;
                }
                if stdin.flush().await.is_err() {
                    break;
                }
            }
            let _ = stdin.shutdown().await;
        }));
        self.child_tx = Some(tx);
        self.process = Some(process);
        self.state = if self.passthrough {
            State::Warm
        } else {
            State::Starting
        };
        self.event(
            "start",
            json!({"ms":self.started.elapsed().as_millis(),"reason":reason}),
        );
        if !self.passthrough {
            self.child_out(rpc::request(
                "zp:init",
                "initialize",
                self.initialize.clone().unwrap_or(json!({})),
            ))?;
        }
        Ok(())
    }
    async fn stop(&mut self, reason: &str) {
        // Dropping the writer channel closes child stdin before group termination.
        self.child_tx.take();
        if let Some(mut process) = self.process.take() {
            // The first task reads stdout, the last writes stdin. Abort the latter to
            // close its owned pipe even if a stalled peer stopped reading.
            if let Some(writer) = process.tasks.pop() {
                writer.abort();
                let _ = writer.await;
            }
            process.stop().await;
        }
        self.state = State::ServingCold;
        self.server_requests.clear();
        self.capture = None;
        self.replay.clear();
        self.event("stop", json!({"reason":reason}));
    }
    async fn fail(&mut self, reason: String) -> io::Result<()> {
        if self.discovery && self.config.launch_identity.is_some() {
            let _ = crate::discovery::record_discovery(
                &self.config,
                crate::discovery::Discovery::Failed,
            );
            if let Some(id) = self.initialize_id.take() {
                // No response has reached the client yet. Restart eagerly and
                // replay its original initialize, then forward normal traffic.
                self.queue.push_front(rpc::request(
                    id,
                    "initialize",
                    self.initialize.clone().unwrap_or(json!({})),
                ));
                self.stop("discovery-fallback").await;
                self.discovery = false;
                self.passthrough = true;
                self.failed = None;
                if self.spawn("legacy-fallback").is_ok() {
                    return self.flush();
                }
            }
        }
        let startup = self.state == State::Starting
            || self.discovery
            || self.started.elapsed() < Duration::from_secs(10);
        let tail = String::from_utf8_lossy(&self.stderr.lock().unwrap()).into_owned();
        let detail = if tail.trim().is_empty() {
            reason
        } else {
            format!("{reason}: {}", tail.trim())
        };
        let detail: String = detail
            .chars()
            .filter(|c| !c.is_control())
            .take(300)
            .collect();
        let message = format!(
            "MCP server {:?} failed to start: {detail}",
            self.config.server
        );
        let mut pending: Vec<_> = self.queue.drain(..).collect();
        pending.extend(self.remap.pending.drain().map(|(_, v)| v));
        if let Some(id) = self.initialize_id.take() {
            pending.push(rpc::request(id, "initialize", json!({})));
        }
        for request in pending {
            if let Some(response) = rpc::failure(&request, &message) {
                self.client_out(response)?;
            }
        }
        if self.discovery && !self.key.is_empty() {
            let mut entry = Entry::new(self.key.clone(), self.config.server.clone(), Value::Null);
            entry.discovery_failed = true;
            entry.cached_at = self.discovery_at;
            self.save(&entry);
        }
        self.event("fail", json!({"reason":detail}));
        self.stop("failure").await;
        if startup {
            self.failed = Some((Instant::now(), detail));
            self.state = State::Failed;
        }
        Ok(())
    }
    async fn capture_failed(&mut self, reason: String) -> io::Result<()> {
        if self.discovery {
            return self.fail(reason).await;
        }
        self.capture = None;
        self.catalog_valid = false;
        let _ = cache::invalidate(&self.config.data_dir, &self.key);
        self.event("cache", json!({"status":"refresh-failed","reason":reason.chars().take(300).collect::<String>()}));
        Ok(())
    }
    fn save(&self, entry: &Entry) {
        if let Err(error) = cache::save(&self.config.data_dir, entry) {
            self.event(
                "cache",
                json!({"status":"write-failed","reason":error.to_string()}),
            );
        }
    }
    fn available_idle(&self) -> bool {
        self.replay.is_empty()
            && self.remap.pending.is_empty()
            && self.server_requests.is_empty()
            && self.subscriptions.is_empty()
            && self.queue.is_empty()
            && self
                .last_progress
                .is_none_or(|t| t.elapsed() >= Duration::from_secs(30))
    }
    fn local(&mut self, message: &Value) -> io::Result<bool> {
        let method = message["method"].as_str().unwrap_or("");
        let Some(id) = message.get("id").cloned() else {
            return Ok(false);
        };
        let result = match method {
            "ping" => Some(json!({})),
            "logging/setLevel" => {
                self.logging = Some(message["params"].clone());
                Some(json!({}))
            }
            _ if self.catalog_valid => self.cache.as_ref().and_then(|e| e.listing(method)),
            _ => None,
        };
        if let Some(result) = result {
            self.client_out(rpc::ok(id, result))?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
    async fn client(&mut self, mut message: Value) -> io::Result<()> {
        if message.get("invalid").is_some() || message["jsonrpc"] != "2.0" {
            return self.client_out(rpc::error(Value::Null, -32600, "invalid JSON-RPC request"));
        }
        let method = message["method"].as_str().unwrap_or("").to_owned();
        if self.config.selftest {
            return self.selftest(message);
        }
        if method.is_empty() {
            if self.server_requests.remove(&message["id"].to_string()) && self.process.is_some() {
                self.last_activity = Instant::now();
                self.child_out(message)?;
            }
            return Ok(());
        }
        if method == "notifications/cancelled" {
            let id = &message["params"]["requestId"];
            let before = self.queue.len();
            self.queue.retain(|v| &v["id"] != id);
            if before != self.queue.len() {
                return Ok(());
            }
            if let Some(mapped) = self.remap.cancel(id) {
                message["params"]["requestId"] = json!(mapped);
                self.child_out(message)?;
            }
            return Ok(());
        }
        if method == "initialize" && self.initialize.is_none() && !self.passthrough {
            self.initialize = Some(message["params"].clone());
            let command =
                cache::executable(&self.config.command, std::env::var_os("PATH").as_deref());
            let (command, bin, npx) = match &self.resolved {
                Some(r) => (r.command.clone(), r.bin.clone(), r.identity()),
                None => (command.clone(), command, Value::Null),
            };
            self.key = cache::key(&self.config, &message["params"], &command, &bin, npx);
            let disk = cache::read(&self.config.data_dir, &self.key);
            if disk
                .as_ref()
                .is_some_and(|e| cache::channel_capability(&e.initialize) == Some(true))
            {
                let _ = crate::discovery::record_discovery(
                    &self.config,
                    crate::discovery::Discovery::Channel,
                );
            }
            if disk.as_ref().is_some_and(|e| {
                e.discovery_failed || cache::channel_capability(&e.initialize) != Some(false)
            }) {
                self.passthrough = true;
                self.event("cache", json!({"status":"discovery-failed-passthrough"}));
            } else if let Some(entry) =
                disk.filter(|e| e.valid(&self.key, &message["params"]["protocolVersion"]))
            {
                self.cache = Some(entry);
                self.state = State::ServingCold;
                self.event("cache", json!({"status":"hit"}));
                return self.client_out(rpc::ok(
                    message["id"].clone(),
                    cache::advertise(self.cache.as_ref().unwrap().initialize.clone()),
                ));
            } else {
                self.discovery = true;
                self.discovery_at = cache::now_ms();
                self.stop_after_capture = self.config.mode == Mode::Lazy;
                self.initialize_id = message.get("id").cloned();
                self.event("cache", json!({"status":"miss"}));
                if let Err(e) = self.spawn("discovery") {
                    self.fail(e.to_string()).await?;
                }
                return Ok(());
            }
        }
        if !self.passthrough {
            if method == "notifications/initialized" {
                return Ok(());
            }
            if method == "notifications/roots/list_changed" {
                self.roots_changed = true;
                if self.state == State::Warm {
                    self.child_out(message)?;
                    self.roots_changed = false;
                }
                return Ok(());
            }
            if self.state != State::Warm && self.local(&message)? {
                return Ok(());
            }
            if self.initialize.is_none() {
                return self.client_out(rpc::error(
                    message["id"].clone(),
                    -32000,
                    "initialize required",
                ));
            }
        }
        if let Some((at, reason)) = &self.failed {
            if at.elapsed() < self.config.backoff {
                let msg = format!(
                    "MCP server {:?} failed {}s ago: {}",
                    self.config.server,
                    at.elapsed().as_secs(),
                    reason
                );
                if let Some(response) = rpc::failure(&message, &msg) {
                    self.client_out(response)?;
                }
                return Ok(());
            }
            self.failed = None;
        }
        self.last_activity = Instant::now();
        if method == "notifications/progress" {
            self.last_progress = Some(Instant::now());
        }
        if self.state == State::Warm {
            self.forward(message)?;
        } else {
            if self.queue.len() >= 256 {
                return self.client_out(rpc::error(
                    message["id"].clone(),
                    -32000,
                    "MCP startup queue full",
                ));
            }
            self.queue.push_back(message);
            if self.process.is_none() {
                if let Err(e) = self.spawn("request") {
                    self.fail(e.to_string()).await?;
                }
                if self.passthrough && self.process.is_some() {
                    self.flush()?;
                }
            }
        }
        Ok(())
    }
    fn forward(&mut self, message: Value) -> io::Result<()> {
        if message["method"] == "logging/setLevel" {
            self.logging = Some(message["params"].clone());
        }
        let message = self.remap.outgoing(message);
        self.child_out(message)
    }
    fn flush(&mut self) -> io::Result<()> {
        while let Some(message) = self.queue.pop_front() {
            if !self.passthrough && self.local(&message)? {
                continue;
            }
            self.forward(message)?;
        }
        Ok(())
    }
    fn capture(&mut self, initialize: Value) -> io::Result<()> {
        let mut lists = VecDeque::new();
        if initialize["capabilities"].get("tools").is_some() {
            lists.push_back(("tools/list", "tools"));
        }
        if initialize["capabilities"].get("prompts").is_some() {
            lists.push_back(("prompts/list", "prompts"));
        }
        if initialize["capabilities"].get("resources").is_some() {
            lists.push_back(("resources/list", "resources"));
            lists.push_back(("resources/templates/list", "resourceTemplates"));
        }
        self.capture = Some(Capture {
            entry: Entry::new(self.key.clone(), self.config.server.clone(), initialize),
            before: self.cache.clone(),
            lists,
            active: None,
            dirty: false,
            at: Instant::now(),
        });
        self.next_list()
    }
    fn next_list(&mut self) -> io::Result<()> {
        let Some((method, field)) = self.capture.as_mut().and_then(|c| c.lists.pop_front()) else {
            return self.finish_capture();
        };
        let id = self.internal_id();
        self.capture.as_mut().unwrap().active = Some((
            id.clone(),
            Listing {
                method,
                field,
                items: Vec::new(),
                cursors: HashSet::new(),
            },
        ));
        self.child_out(rpc::request(id, method, json!({})))
    }
    fn finish_capture(&mut self) -> io::Result<()> {
        let Some(capture) = self.capture.take() else {
            return Ok(());
        };
        if capture.dirty {
            return self.capture(capture.entry.initialize);
        }
        let entry = capture.entry;
        let _ = crate::discovery::record_discovery(
            &self.config,
            crate::discovery::Discovery::NonChannel,
        );
        self.save(&entry);
        self.event(
            "cache",
            json!({"status":"refresh","ms":self.started.elapsed().as_millis()}),
        );
        let mut changed = Vec::new();
        if let Some(before) = capture.before {
            if cache::advertise(before.initialize) != cache::advertise(entry.initialize.clone()) {
                changed.extend(["tools", "prompts", "resources"]);
            }
            if before.tools != entry.tools {
                changed.push("tools");
            }
            if before.prompts != entry.prompts {
                changed.push("prompts");
            }
            if before.resources != entry.resources
                || before.resource_templates != entry.resource_templates
            {
                changed.push("resources");
            }
        }
        self.cache = Some(entry);
        self.catalog_valid = true;
        self.discovery = false;
        if let Some(id) = self.initialize_id.take() {
            self.client_out(rpc::ok(
                id,
                cache::advertise(self.cache.as_ref().unwrap().initialize.clone()),
            ))?;
        }
        changed.sort();
        changed.dedup();
        for cap in changed {
            self.client_out(rpc::notification(
                &format!("notifications/{cap}/list_changed"),
                json!({}),
            ))?;
        }
        self.flush()
    }
    async fn server(&mut self, message: Value) -> io::Result<()> {
        let method = message["method"].as_str().unwrap_or("");
        if !method.is_empty() {
            if let Some(id) = message.get("id") {
                self.server_requests.insert(id.to_string());
            }
            if method == "notifications/progress" {
                self.last_progress = Some(Instant::now());
            }
            if method == "notifications/cancelled" {
                let id = &message["params"]["requestId"];
                if self.server_requests.remove(&id.to_string()) {
                    return self.client_out(message);
                }
                if let Some(original) = id.as_str().and_then(|id| self.remap.pending.get(id)) {
                    let mut message = message.clone();
                    message["params"]["requestId"] = original["id"].clone();
                    return self.client_out(message);
                }
                if id == "zp:init"
                    || id.as_str().is_some_and(|id| self.replay.contains_key(id))
                    || self
                        .capture
                        .as_ref()
                        .and_then(|c| c.active.as_ref())
                        .is_some_and(|(active, _)| id == active)
                {
                    return Ok(());
                }
                self.server_requests.remove(&id.to_string());
            }
            if !self.passthrough
                && [
                    "notifications/tools/list_changed",
                    "notifications/prompts/list_changed",
                    "notifications/resources/list_changed",
                ]
                .contains(&method)
            {
                self.catalog_valid = false;
                let _ = cache::invalidate(&self.config.data_dir, &self.key);
                if let Some(c) = &mut self.capture {
                    c.dirty = true;
                } else if let Some(entry) = &self.cache {
                    self.capture(entry.initialize.clone())?;
                }
            }
            return self.client_out(message);
        }
        if !self.passthrough && message["id"] == "zp:init" {
            if message.get("error").is_some() {
                return self.fail(format!("initialize: {}", message["error"])).await;
            }
            let result = message["result"].clone();
            if !result.is_object() {
                return self.fail("initialize result is not an object".into()).await;
            }
            let Some(channel) = cache::channel_capability(&result) else {
                return self
                    .fail("initialize channel capabilities unknown".into())
                    .await;
            };
            if channel {
                self.stop_after_capture = false;
                let _ = crate::discovery::record_discovery(
                    &self.config,
                    crate::discovery::Discovery::Channel,
                );
                let entry =
                    Entry::new(self.key.clone(), self.config.server.clone(), result.clone());
                self.save(&entry);
                if let Some(id) = self.initialize_id.take() {
                    // Keep the very same server alive. Let Claude send initialized
                    // itself and forward every channel notification unchanged.
                    self.discovery = false;
                    self.state = State::Warm;
                    self.passthrough = true;
                    self.client_out(rpc::ok(id, result))?;
                    return self.flush();
                }
            }
            self.child_out(rpc::notification("notifications/initialized", json!({})))?;
            if let Some(params) = self.logging.clone() {
                let id = self.internal_id();
                self.replay.insert(
                    id.clone(),
                    rpc::request(id.clone(), "logging/setLevel", params.clone()),
                );
                self.child_out(rpc::request(id, "logging/setLevel", params))?;
            }
            for params in self.subscriptions.values().cloned().collect::<Vec<_>>() {
                let id = self.internal_id();
                self.replay.insert(
                    id.clone(),
                    rpc::request(id.clone(), "resources/subscribe", params.clone()),
                );
                self.child_out(rpc::request(id, "resources/subscribe", params))?;
            }
            if self.roots_changed {
                self.child_out(rpc::notification(
                    "notifications/roots/list_changed",
                    json!({}),
                ))?;
                self.roots_changed = false;
            }
            self.state = State::Warm;
            self.failed = None;
            if channel {
                self.discovery = false;
                self.passthrough = true;
                return self.flush();
            }
            self.capture(result)?;
            // Non-discovery starts needn't delay calls for metadata refresh.
            if !self.discovery {
                self.flush()?;
            }
            return Ok(());
        }
        if self
            .capture
            .as_ref()
            .and_then(|c| c.active.as_ref())
            .is_some_and(|(id, _)| message["id"] == *id)
        {
            if message.get("error").is_some() {
                return self
                    .capture_failed(format!("metadata capture: {}", message["error"]))
                    .await;
            }
            let capture = self.capture.as_mut().unwrap();
            let (id, list) = capture.active.as_mut().unwrap();
            let result = &message["result"];
            let Some(items) = result[list.field].as_array() else {
                return self.capture_failed("invalid metadata listing".into()).await;
            };
            list.items.extend(items.iter().cloned());
            if list.field == "tools" {
                if let Some(ttl) = result["ttlMs"].as_u64() {
                    capture.entry.ttl_ms =
                        Some(capture.entry.ttl_ms.map_or(ttl, |old| old.min(ttl)));
                }
                if let Some(scope) = result["cacheScope"].as_str() {
                    if capture.entry.cache_scope.as_deref() != Some("private") {
                        capture.entry.cache_scope = Some(scope.into());
                    }
                }
            }
            if let Some(cursor) = result["nextCursor"].as_str().filter(|s| !s.is_empty()) {
                if !list.cursors.insert(cursor.into()) || list.cursors.len() > 10000 {
                    return self.capture_failed("metadata pagination loop".into()).await;
                }
                let request = rpc::request(id.clone(), list.method, json!({"cursor":cursor}));
                return self.child_out(request);
            }
            let (_, list) = capture.active.take().unwrap();
            match list.field {
                "tools" => capture.entry.tools = list.items,
                "prompts" => capture.entry.prompts = Some(list.items),
                "resources" => capture.entry.resources = Some(list.items),
                _ => capture.entry.resource_templates = Some(list.items),
            }
            return self.next_list();
        }
        if let Some(id) = message["id"].as_str() {
            if let Some(request) = self.replay.remove(id) {
                if message.get("error").is_some() && request["method"] == "resources/subscribe" {
                    if let Some(uri) = request["params"]["uri"].as_str() {
                        self.subscriptions.remove(uri);
                    }
                }
                return Ok(());
            }
        }
        if let Some((response, request)) = self.remap.response(message) {
            self.last_activity = Instant::now();
            if response.get("error").is_none() {
                if request["method"] == "resources/subscribe" {
                    if let Some(uri) = request["params"]["uri"].as_str() {
                        self.subscriptions
                            .insert(uri.into(), request["params"].clone());
                    }
                }
                if request["method"] == "resources/unsubscribe" {
                    if let Some(uri) = request["params"]["uri"].as_str() {
                        self.subscriptions.remove(uri);
                    }
                }
            }
            self.client_out(response)?;
        }
        // Unknown replies include replay IDs; proxy-originated IDs never escape.
        Ok(())
    }
    fn selftest(&self, message: Value) -> io::Result<()> {
        let Some(id) = message.get("id").cloned() else {
            return Ok(());
        };
        let result = match message["method"].as_str().unwrap_or("") {
            "initialize" => {
                json!({"protocolVersion":message["params"]["protocolVersion"],"serverInfo":{"name":"zeron-mcp-proxy-selftest","version":"1"},"capabilities":{"tools":{}}})
            }
            "tools/list" => {
                json!({"tools":[{"name":"zeron_proxy_selftest","description":"Return ok","inputSchema":{"type":"object"}}]})
            }
            "tools/call" => json!({"content":[{"type":"text","text":"ok"}]}),
            "ping" => json!({}),
            _ => return self.client_out(rpc::error(id, -32601, "unknown selftest method")),
        };
        self.client_out(rpc::ok(id, result))
    }
}

pub async fn run(config: Config) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    if unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let (tx, mut rx) = mpsc::channel(256);
    let (output, mut out_rx) = mpsc::channel::<Value>(256);
    let reader = read_lines(tokio::io::stdin(), tx.clone(), None);
    let close = tx.clone();
    let writer = tokio::spawn(async move {
        let mut stdout = tokio::io::stdout();
        while let Some(value) = out_rx.recv().await {
            if stdout
                .write_all(format!("{value}\n").as_bytes())
                .await
                .is_err()
                || stdout.flush().await.is_err()
            {
                let _ = close.send(Event::ClientClosed).await;
                break;
            }
        }
    });
    let resolved = if config.no_npx_resolve || config.selftest {
        None
    } else {
        crate::npx::resolve(&config.command, &config.args, &config.data_dir).await
    };
    let passthrough = config.mode == Mode::KeepAlive;
    let mut proxy = Proxy {
        config,
        resolved,
        state: State::Cold,
        process: None,
        child_tx: None,
        events: tx,
        output,
        generation: 0,
        next_id: 0,
        initialize: None,
        initialize_id: None,
        key: String::new(),
        cache: None,
        catalog_valid: true,
        capture: None,
        discovery: false,
        discovery_at: 0,
        replay: HashMap::new(),
        stop_after_capture: false,
        passthrough,
        queue: VecDeque::new(),
        remap: Remap::default(),
        server_requests: HashSet::new(),
        logging: None,
        subscriptions: HashMap::new(),
        roots_changed: false,
        failed: None,
        started: Instant::now(),
        last_activity: Instant::now(),
        last_progress: None,
        stderr: Arc::new(Mutex::new(Vec::new())),
    };
    #[cfg(unix)]
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    #[cfg(unix)]
    let mut hup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())?;
    let mut timer = tokio::time::interval(proxy.config.check_interval);
    timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut result = Ok(());
    let mut reason = "eof";
    if passthrough && !proxy.config.selftest {
        if let Err(e) = proxy.spawn("keep-alive") {
            proxy.fail(e.to_string()).await?;
        }
    }
    loop {
        let start_deadline =
            if proxy.state == State::Starting || (proxy.discovery && proxy.process.is_some()) {
                proxy.started + proxy.config.start_timeout
            } else if let Some(capture) = &proxy.capture {
                capture.at + proxy.config.start_timeout
            } else {
                Instant::now() + Duration::from_secs(86400)
            };
        tokio::select! {
            event = rx.recv() => {
                let action = match event {
                    Some(Event::Client(v)) => proxy.client(v).await,
                    Some(Event::Server(g,v)) if g == proxy.generation && proxy.process.is_some() => proxy.server(v).await,
                    Some(Event::ServerClosed(g,reason)) if g == proxy.generation && proxy.process.is_some() => {
                        let status = proxy.process.as_mut().unwrap().child.try_wait().ok().flatten();
                        proxy.fail(format!("exited ({}) during the call",status.map(|s|s.to_string()).unwrap_or(reason))).await
                    }
                    Some(Event::ClientClosed) | None => break,
                    _ => Ok(()),
                };
                if let Err(e) = action { result = Err(e); break; }
            }
            status = async {
                match proxy.process.as_mut() { Some(p) => p.child.wait().await, None => std::future::pending().await }
            } => {
                if let Err(e) = proxy.fail(format!("exited ({}) during the call",status.map(|s|s.to_string()).unwrap_or_else(|e|e.to_string()))).await { result = Err(e); break; }
            }
            _ = timer.tick() => {
                if let Some(p) = &mut proxy.process { p.discover_watchdogs(); }
                if proxy.state == State::Warm && !proxy.passthrough && proxy.config.mode == Mode::Lazy && proxy.capture.is_none() && proxy.available_idle()
                    && (proxy.stop_after_capture || (!proxy.config.idle.is_zero() && proxy.last_activity.elapsed() >= proxy.config.idle)) {
                    proxy.stop_after_capture = false; proxy.stop("idle").await;
                }
            }
            _ = tokio::time::sleep_until(start_deadline) => {
                let timeout = if proxy.state == State::Starting || proxy.discovery { proxy.fail("initialize/discovery timeout (60s default)".into()).await }
                    else { proxy.capture_failed("metadata refresh timeout".into()).await };
                if let Err(e) = timeout { result = Err(e); break; }
            }
            _ = tokio::signal::ctrl_c() => { reason = "signal"; break; }
            _ = async {
                #[cfg(unix)] { tokio::select! { _ = term.recv() => (), _ = hup.recv() => () } }
                #[cfg(not(unix))] { std::future::pending::<()>().await; }
            } => { reason = "signal"; break; }
        }
        if proxy.stop_after_capture
            && proxy.state == State::Warm
            && proxy.capture.is_none()
            && proxy.available_idle()
        {
            proxy.stop_after_capture = false;
            proxy.stop("idle").await;
        }
    }
    proxy.stop(reason).await;
    reader.abort();
    drop(proxy);
    let _ = tokio::time::timeout(Duration::from_secs(1), writer).await;
    result
}
