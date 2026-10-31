//! Minimal newline-delimited MCP stdio client for offline benchmark tools.
use std::process::Stdio;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

pub struct Client {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    next_id: u64,
    timeout: Duration,
}

impl Client {
    /// The caller supplies a fully isolated command and environment.
    pub fn start(command: &mut Command, timeout: Duration) -> Result<Self> {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .context("start MCP server")?;
        let input = child.stdin.take().context("MCP stdin")?;
        let output = BufReader::new(child.stdout.take().context("MCP stdout")?);
        Ok(Self {
            child,
            input,
            output,
            next_id: 1,
            timeout,
        })
    }

    async fn send(&mut self, value: &Value) -> Result<()> {
        let mut bytes = serde_json::to_vec(value)?;
        bytes.push(b'\n');
        self.input.write_all(&bytes).await?;
        self.input.flush().await?;
        Ok(())
    }

    pub async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"jsonrpc":"2.0", "id":id, "method":method, "params":params}))
            .await?;
        let response = tokio::time::timeout(self.timeout, async {
            loop {
                let mut line = String::new();
                ensure!(
                    self.output.read_line(&mut line).await? != 0,
                    "MCP server closed stdout"
                );
                ensure!(line.len() <= 4 * 1024 * 1024, "MCP response too large");
                let frame: Value = serde_json::from_str(&line).context("invalid MCP JSON")?;
                if frame["id"].as_u64() != Some(id) {
                    continue;
                }
                if !frame["error"].is_null() {
                    bail!("MCP {method} failed: {}", frame["error"]);
                }
                return frame
                    .get("result")
                    .cloned()
                    .context("MCP response missing result");
            }
        })
        .await
        .context("MCP response timed out")??;
        Ok(response)
    }

    pub async fn initialize(&mut self) -> Result<Value> {
        let result = self.request("initialize", json!({"protocolVersion":"2024-11-05", "capabilities":{}, "clientInfo":{"name":"zeron-bench", "version":"1"}})).await?;
        self.send(&json!({"jsonrpc":"2.0", "method":"notifications/initialized"}))
            .await?;
        Ok(result)
    }

    pub async fn timed_call(&mut self, name: &str, arguments: Value) -> Result<(Value, f64)> {
        let start = Instant::now();
        let result = self
            .request("tools/call", json!({"name": name, "arguments": arguments}))
            .await?;
        ensure!(result["isError"] != true, "MCP tool returned an error");
        Ok((result, start.elapsed().as_secs_f64() * 1000.0))
    }

    pub async fn stop(mut self) -> Result<()> {
        self.child.start_kill().ok();
        self.child.wait().await?;
        Ok(())
    }
}
