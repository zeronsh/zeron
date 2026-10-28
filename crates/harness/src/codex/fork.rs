//! Administrative app-server client: no thread/start or turn/start is permitted here.
use super::*;
use crate::{NativeForkControls, NativeForkError};
use zeron_proto::{NativeForkAvailability, NativeForkBoundary, NativeForkPoint, NativeForkResult};

impl CodexHarness {
    pub(super) async fn fork_support(&self) -> NativeForkAvailability {
        let probe = async {
            let exe = self.resolve_executable().map_err(|e| e.to_string())?;
            let meta = std::fs::metadata(&exe).map_err(|e| e.to_string())?;
            let key = format!("{}:{:?}:{}", exe.display(), meta.modified(), meta.len());
            static CACHE: std::sync::OnceLock<tokio::sync::Mutex<HashMap<String, bool>>> =
                std::sync::OnceLock::new();
            let mut cache = CACHE.get_or_init(Default::default).lock().await;
            if let Some(ok) = cache.get(&key) {
                return Ok(*ok);
            }
            let dir =
                crate::scratch::ScratchDir::new("codex-fork-schema").map_err(|e| e.to_string())?;
            let mut cmd = Command::new(&exe);
            crate::compose_child_path(&mut cmd, &exe);
            cmd.args(["app-server", "generate-json-schema", "--out"])
                .arg(dir.path())
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .kill_on_drop(true);
            let mut child = cmd.spawn().map_err(|e| e.to_string())?;
            let result = tokio::time::timeout(Duration::from_secs(15), child.wait()).await;
            let _ = child.start_kill();
            let _ = child.wait().await;
            let success = result
                .ok()
                .and_then(Result::ok)
                .is_some_and(|s| s.success());
            let schema = std::fs::read(dir.path().join("v2/ThreadForkParams.json"))
                .ok()
                .and_then(|b| serde_json::from_slice::<Value>(&b).ok());
            let supported = success
                && schema.as_ref().is_some_and(|s| {
                    s.pointer("/properties/lastTurnId").is_some()
                        && s.pointer("/properties/threadId").is_some()
                });
            cache.insert(key, supported);
            Ok::<_, String>(supported)
        }
        .await;
        match probe {
            Ok(true) => NativeForkAvailability::available(),
            _ => NativeForkAvailability::unavailable(
                "This Codex executable has no verified lastTurnId fork contract",
            ),
        }
    }

    pub(super) async fn fork_at(
        &self,
        point: &NativeForkPoint,
        controls: NativeForkControls,
    ) -> Result<NativeForkResult, NativeForkError> {
        let reject = |e: HarnessError| NativeForkError::Rejected(e.to_string());
        point.validate().map_err(NativeForkError::Rejected)?;
        let NativeForkBoundary::AppServerTurn { turn_id } = &point.boundary else {
            return Err(NativeForkError::Rejected("Expected a Codex turn".into()));
        };
        if !self.fork_support().await.available {
            return Err(NativeForkError::Rejected(
                "Codex lastTurnId support is unavailable".into(),
            ));
        }
        let exe = self.resolve_executable().map_err(reject)?;
        let mut cmd = Command::new(&exe);
        crate::compose_child_path(&mut cmd, &exe);
        cmd.arg("app-server")
            .current_dir(&point.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let mut child = cmd.spawn().map_err(HarnessError::Io).map_err(reject)?;
        let (client, mut incoming) =
            RpcClient::new(child.stdin.take().unwrap(), child.stdout.take().unwrap());
        let drain_client = client.clone();
        let drain = tokio::spawn(async move {
            while let Some(frame) = incoming.recv().await {
                if let Incoming::Request { id, .. } = frame {
                    drain_client.respond_error(
                        &id,
                        -32601,
                        "No interactive requests during native fork",
                    );
                }
            }
        });
        let mut creating = false;
        let operation = async {
            client.request("initialize", json!({"clientInfo":{"name":"zeron-native", "version":env!("CARGO_PKG_VERSION")}, "capabilities":{"experimentalApi":true}})).await.map_err(reject)?;
            client.notify("initialized", None);
            let source = client
                .request(
                    "thread/read",
                    json!({"threadId":point.source_session_id, "includeTurns":true}),
                )
                .await
                .map_err(reject)?;
            let turns = source["thread"]["turns"].as_array().ok_or_else(|| {
                NativeForkError::Rejected("Source turn history is unavailable".into())
            })?;
            let index = turns
                .iter()
                .position(|turn| turn["id"].as_str() == Some(turn_id))
                .ok_or_else(|| NativeForkError::Rejected("Native turn no longer exists".into()))?;
            if turns[index]["status"] != "completed" {
                return Err(NativeForkError::Rejected(
                    "This reply is not a complete provider turn".into(),
                ));
            }
            creating = true;
            let response = client.request("thread/fork", json!({"threadId":point.source_session_id,"lastTurnId":turn_id,"ephemeral":false,"cwd":point.cwd})).await
                .map_err(|e| NativeForkError::Indeterminate(e.to_string()))?;
            let id = response["thread"]["id"]
                .as_str()
                .filter(|s| !s.is_empty() && *s != point.source_session_id)
                .ok_or_else(|| {
                    NativeForkError::Indeterminate(
                        "Provider returned no independent thread ID".into(),
                    )
                })?;
            let child = client
                .request("thread/read", json!({"threadId":id,"includeTurns":true}))
                .await
                .map_err(|e| NativeForkError::Indeterminate(e.to_string()))?;
            if child["thread"]["turns"].as_array().map(Vec::as_slice) != Some(&turns[..=index]) {
                return Err(NativeForkError::Indeterminate(
                    "Provider did not preserve the exact selected turn prefix".into(),
                ));
            }
            Ok(NativeForkResult {
                session_id: id.into(),
                cwd: point.cwd.clone(),
            })
        };
        let outcome = tokio::select! {
            result = tokio::time::timeout(controls.timeout, operation) => result.ok(),
            _ = controls.interrupt.cancelled() => None,
        };
        shutdown_child(&mut child, self.kill_grace).await;
        drain.abort();
        let _ = drain.await;
        drop(controls.execution_lease);
        outcome.unwrap_or_else(|| {
            Err(if creating {
                NativeForkError::Indeterminate("Fork cancelled or timed out".into())
            } else {
                NativeForkError::Rejected("Fork cancelled or timed out before creation".into())
            })
        })
    }
}
