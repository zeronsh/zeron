//! `mcp-proxy`: Zeron's MCP tools on stdio for an agent that only speaks
//! stdio MCP, forwarded to the engine's own HTTP endpoint ([`crate::http`]).
//!
//! It is what an agent's run starts in place of a whole `zeron mcp` server:
//! no async runtime, no engine client, no state beyond the calls in flight —
//! std only, a thread per call — so it stays a couple of MB resident. Each
//! line from the agent is POSTed on its own connection with the run's token
//! (`ZERON_MCP_TOKEN`) to `ZERON_MCP_URL`; whatever comes back (one JSON
//! object, or an SSE stream of progress and then the response) is written
//! back as lines. Once the agent has initialized, a `subscriptions/listen`
//! stream brings tool-list changes, as the stdio server sent them.
//!
//! A call the endpoint can't take (the engine restarted and the run's token
//! went with it) is answered with an error, never left hanging.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

/// The modern revision the change stream is opened with.
const LISTEN_VERSION: &str = "2026-07-28";

#[derive(Clone)]
struct Endpoint {
    host: String,
    port: u16,
    path: String,
    token: String,
}

impl Endpoint {
    fn from_env() -> anyhow::Result<Self> {
        let url = std::env::var(zeron_proto::McpServer::URL_ENV)
            .map_err(|_| anyhow::anyhow!("{} is not set", zeron_proto::McpServer::URL_ENV))?;
        let token = std::env::var(zeron_proto::McpServer::TOKEN_ENV)
            .map_err(|_| anyhow::anyhow!("{} is not set", zeron_proto::McpServer::TOKEN_ENV))?;
        Self::parse(&url, token)
    }

    /// `http://host:port/path` — loopback only.
    fn parse(url: &str, token: String) -> anyhow::Result<Self> {
        let rest = url
            .strip_prefix("http://")
            .ok_or_else(|| anyhow::anyhow!("not an http:// URL: {url}"))?;
        let (authority, path) = match rest.find('/') {
            Some(slash) => (&rest[..slash], &rest[slash..]),
            None => (rest, "/"),
        };
        let (host, port) = authority
            .rsplit_once(':')
            .ok_or_else(|| anyhow::anyhow!("no port in {url}"))?;
        if host != "127.0.0.1" && host != "localhost" {
            anyhow::bail!("refusing a non-loopback MCP endpoint: {url}");
        }
        Ok(Self {
            host: host.to_owned(),
            port: port.parse()?,
            path: path.to_owned(),
            token,
        })
    }

    /// POST `body`; the response's status, content type and body reader.
    fn post(&self, body: &str, extra: &[(&str, String)]) -> std::io::Result<Reply> {
        let mut stream = TcpStream::connect((self.host.as_str(), self.port))?;
        let _ = stream.set_nodelay(true);
        let mut head = format!(
            "POST {} HTTP/1.1\r\nHost: {}:{}\r\nAuthorization: Bearer {}\r\n\
             Content-Type: application/json\r\nAccept: application/json, text/event-stream\r\n\
             Content-Length: {}\r\nConnection: close\r\n",
            self.path,
            self.host,
            self.port,
            self.token,
            body.len()
        );
        for (name, value) in extra {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        head.push_str("\r\n");
        stream.write_all(head.as_bytes())?;
        stream.write_all(body.as_bytes())?;
        stream.flush()?;
        read_reply(stream)
    }
}

struct Reply {
    status: u16,
    event_stream: bool,
    body: Box<dyn BufRead + Send>,
}

fn read_reply(stream: TcpStream) -> std::io::Result<Reply> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let status = line
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .ok_or_else(|| std::io::Error::other(format!("bad status line: {line:?}")))?;
    let mut length: Option<u64> = None;
    let mut chunked = false;
    let mut event_stream = false;
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        let header = line.trim_end();
        if header.is_empty() {
            break;
        }
        let Some((name, value)) = header.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match name.trim().to_ascii_lowercase().as_str() {
            "content-length" => length = value.parse().ok(),
            "transfer-encoding" => chunked = value.eq_ignore_ascii_case("chunked"),
            "content-type" => event_stream = value.starts_with("text/event-stream"),
            _ => {}
        }
    }
    let body: Box<dyn BufRead + Send> = if chunked {
        Box::new(BufReader::new(Chunked { inner: reader, left: 0, done: false }))
    } else if let Some(length) = length {
        Box::new(BufReader::new(reader.take(length)))
    } else {
        Box::new(reader)
    };
    Ok(Reply { status, event_stream, body })
}

/// A `Transfer-Encoding: chunked` body, decoded.
struct Chunked<R> {
    inner: R,
    left: usize,
    done: bool,
}

impl<R: BufRead> Read for Chunked<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.done {
            return Ok(0);
        }
        if self.left == 0 {
            let mut size = String::new();
            // The CRLF ending the previous chunk, then the size line.
            loop {
                size.clear();
                if self.inner.read_line(&mut size)? == 0 {
                    self.done = true;
                    return Ok(0);
                }
                if !size.trim().is_empty() {
                    break;
                }
            }
            let hex = size.trim().split(';').next().unwrap_or("0");
            self.left = usize::from_str_radix(hex, 16)
                .map_err(|_| std::io::Error::other(format!("bad chunk size: {size:?}")))?;
            if self.left == 0 {
                self.done = true;
                return Ok(0);
            }
        }
        let want = buf.len().min(self.left);
        let read = self.inner.read(&mut buf[..want])?;
        if read == 0 {
            self.done = true;
        }
        self.left -= read;
        Ok(read)
    }
}

/// Every JSON-RPC message in a reply, in order, to `each`.
fn messages(reply: Reply, mut each: impl FnMut(Value)) -> std::io::Result<()> {
    let mut body = reply.body;
    if !reply.event_stream {
        let mut text = String::new();
        body.read_to_string(&mut text)?;
        if let Ok(message) = serde_json::from_str::<Value>(text.trim()) {
            each(message);
        }
        return Ok(());
    }
    // SSE: `data:` lines make an event; a blank line ends it; `:` lines are
    // comments (keep-alives).
    let mut data = String::new();
    let mut line = String::new();
    loop {
        line.clear();
        if body.read_line(&mut line)? == 0 {
            return Ok(());
        }
        let text = line.trim_end_matches(['\r', '\n']);
        if text.is_empty() {
            if !data.is_empty() {
                if let Ok(message) = serde_json::from_str::<Value>(&data) {
                    each(message);
                }
                data.clear();
            }
        } else if let Some(value) = text.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(value.strip_prefix(' ').unwrap_or(value));
        }
    }
}

type Out = Arc<Mutex<std::io::Stdout>>;

fn write_line(out: &Out, message: &Value) {
    let mut out = out.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let _ = writeln!(out, "{message}");
    let _ = out.flush();
}

fn failure(id: Value, message: String) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32603, "message": message } })
}

/// Forward one request; its reply (or an error) goes to `out`.
fn forward_request(endpoint: &Endpoint, out: &Out, id: Value, line: &str) {
    let mut answered = false;
    let result = endpoint.post(line, &[]).and_then(|reply| {
        let status = reply.status;
        messages(reply, |message| {
            if message.get("id") == Some(&id) && message.get("method").is_none() {
                answered = true;
            }
            write_line(out, &message);
        })
        .map(|()| status)
    });
    if !answered {
        let why = match result {
            Ok(401) => "the Zeron engine no longer knows this agent's run (it restarted?)".to_owned(),
            Ok(status) => format!("the Zeron engine answered HTTP {status}"),
            Err(err) => format!("the Zeron engine can't be reached: {err}"),
        };
        write_line(out, &failure(id, why));
    }
}

/// Tool-list changes, while the agent runs: a `subscriptions/listen`
/// stream, reopened when it drops, until the endpoint refuses the token.
fn listen_for_changes(endpoint: Endpoint, out: Out) {
    let body = json!({
        "jsonrpc": "2.0",
        "id": "zeron-proxy-listen",
        "method": "subscriptions/listen",
        "params": {
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": LISTEN_VERSION,
                "io.modelcontextprotocol/clientInfo": { "name": "zeron-mcp-proxy", "version": env!("CARGO_PKG_VERSION") },
                "io.modelcontextprotocol/clientCapabilities": {},
            },
            "notifications": { "toolsListChanged": true },
        },
    })
    .to_string();
    let headers = [
        ("MCP-Protocol-Version", LISTEN_VERSION.to_owned()),
        ("Mcp-Method", "subscriptions/listen".to_owned()),
    ];
    let mut backoff = Duration::from_secs(1);
    loop {
        match endpoint.post(&body, &headers) {
            Ok(reply) if reply.status == 200 => {
                backoff = Duration::from_secs(1);
                let _ = messages(reply, |message| {
                    if message.get("method").and_then(Value::as_str)
                        == Some("notifications/tools/list_changed")
                    {
                        // As the stdio server sent it: no subscription tag.
                        write_line(
                            &out,
                            &json!({ "jsonrpc": "2.0", "method": "notifications/tools/list_changed" }),
                        );
                    }
                });
            }
            // The run is over (or never was): nothing more will change.
            Ok(reply) if reply.status == 401 || reply.status == 403 => return,
            _ => {}
        }
        std::thread::sleep(backoff);
        backoff = (backoff * 2).min(Duration::from_secs(30));
    }
}

/// Run until stdin closes.
pub fn run() -> anyhow::Result<()> {
    let endpoint = Endpoint::from_env()?;
    let out: Out = Arc::new(Mutex::new(std::io::stdout()));
    let mut listening = false;
    let stdin = std::io::stdin();
    let mut line = String::new();
    let mut input = stdin.lock();
    loop {
        line.clear();
        match input.read_line(&mut line) {
            Ok(0) => return Ok(()),
            Ok(_) => {}
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err.into()),
        }
        let text = line.trim();
        if text.is_empty() {
            continue;
        }
        let Ok(message) = serde_json::from_str::<Value>(text) else {
            write_line(
                &out,
                &json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": "parse error" } }),
            );
            continue;
        };
        let method = message.get("method").and_then(Value::as_str).map(str::to_owned);
        let id = message.get("id").cloned();
        let endpoint = endpoint.clone();
        let body = text.to_owned();
        match (id, method) {
            (Some(id), Some(_)) => {
                let out = out.clone();
                std::thread::spawn(move || forward_request(&endpoint, &out, id, &body));
            }
            (None, Some(method)) => {
                if method == "notifications/initialized" && !listening {
                    listening = true;
                    let out = out.clone();
                    let listen = endpoint.clone();
                    std::thread::spawn(move || listen_for_changes(listen, out));
                }
                // Cancels and the like: the endpoint answers 202.
                std::thread::spawn(move || {
                    let _ = endpoint.post(&body, &[]).map(|reply| messages(reply, |_| {}));
                });
            }
            // A response to a server request: the endpoint sends none.
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_loopback_http_endpoint_is_taken() {
        let ok = Endpoint::parse("http://127.0.0.1:4321/mcp", "t".into()).unwrap();
        assert_eq!((ok.host.as_str(), ok.port, ok.path.as_str()), ("127.0.0.1", 4321, "/mcp"));
        assert!(Endpoint::parse("http://example.com:80/mcp", "t".into()).is_err());
        assert!(Endpoint::parse("https://127.0.0.1:1/mcp", "t".into()).is_err());
    }

    #[test]
    fn chunked_sse_decodes_into_messages_in_order() {
        let sse = "event: message\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\"}\n\n: keep-alive\n\nevent: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n\n";
        let mut wire = String::new();
        for piece in [&sse[..10], &sse[10..70], &sse[70..]] {
            wire.push_str(&format!("{:x}\r\n{piece}\r\n", piece.len()));
        }
        wire.push_str("0\r\n\r\n");
        let reader = BufReader::new(Chunked {
            inner: BufReader::new(std::io::Cursor::new(wire.into_bytes())),
            left: 0,
            done: false,
        });
        let mut seen = Vec::new();
        messages(
            Reply { status: 200, event_stream: true, body: Box::new(reader) },
            |message| seen.push(message),
        )
        .unwrap();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0]["method"], "notifications/progress");
        assert_eq!(seen[1]["id"], 1);
    }
}
