use super::*;
use crate::{NativeForkControls, NativeForkError};
use std::collections::HashSet;
use std::sync::{
    Mutex, OnceLock, Weak,
    atomic::{AtomicBool, Ordering},
};
use zeron_proto::{NativeForkAvailability, NativeForkBoundary, NativeForkPoint, NativeForkResult};

#[derive(Default)]
pub(super) struct Admission {
    pub gate: Arc<tokio::sync::Mutex<()>>,
    pub active: AtomicBool,
}
pub(super) fn admission(server: &Server, cwd: Option<&str>, session: &str) -> Arc<Admission> {
    static GATES: OnceLock<Mutex<HashMap<String, Weak<Admission>>>> = OnceLock::new();
    let mut gates = GATES
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let key = format!(
        "{}\0{}\0{session}",
        if server.child.is_some() {
            "managed"
        } else {
            &server.base
        },
        cwd.unwrap_or_default()
    );
    if let Some(gate) = gates.get(&key).and_then(Weak::upgrade) {
        return gate;
    }
    gates.retain(|_, v| v.strong_count() > 0);
    let gate = Arc::new(Admission::default());
    gates.insert(key, Arc::downgrade(&gate));
    gate
}
fn canonical(message: &Value) -> Value {
    let mut result = message.clone();
    // Remove only storage identities; preserve provider tool IDs and all payloads.
    let clean = |value: &mut Value| {
        if let Some(map) = value.as_object_mut() {
            for key in ["id", "sessionID", "messageID", "parentID"] {
                map.remove(key);
            }
        }
    };
    clean(&mut result);
    if let Some(info) = result.get_mut("info") {
        clean(info);
    }
    if let Some(parts) = result.get_mut("parts").and_then(Value::as_array_mut) {
        for part in parts {
            clean(part);
        }
    }
    result
}
fn info(message: &Value) -> &Value {
    message.get("info").unwrap_or(message)
}
fn message_id(message: &Value) -> Option<&str> {
    info(message)["id"].as_str()
}

impl Server {
    async fn fork_messages(&self, session: &str, cwd: &str) -> Result<Vec<Value>, HarnessError> {
        let v2 = self.protocol().await == Protocol::V2;
        let path = format!("{}/session/{session}/message", if v2 { "/api" } else { "" });
        let mut cursor = None::<String>;
        let mut seen = HashSet::new();
        let mut messages = Vec::new();
        loop {
            let mut req = self
                .request(reqwest::Method::GET, &path)
                .timeout(CALL_TIMEOUT);
            if v2 {
                req = req.query(&[("limit", "100")]);
                // V2 cursors carry their ordering; combining order with cursor
                // is rejected even on the final (empty) page.
                if cursor.is_none() {
                    req = req.query(&[("order", "asc")]);
                }
            }
            if let Some(cursor) = &cursor {
                req = req.query(&[("cursor", cursor)]);
            }
            let response = self
                .scoped(req, Some(cwd))
                .await
                .send()
                .await
                .map_err(|e| HarnessError::Protocol(e.to_string()))?;
            if !response.status().is_success() {
                return Err(HarnessError::Protocol(format!(
                    "Native transcript is unavailable (HTTP {})",
                    response.status()
                )));
            }
            let page: Value = response
                .json()
                .await
                .map_err(|e| HarnessError::Protocol(e.to_string()))?;
            let values = if v2 { &page["data"] } else { &page };
            let items = values.as_array().ok_or_else(|| {
                HarnessError::Protocol("Unexpected native transcript contract".into())
            })?;
            for message in items {
                let id = message_id(message)
                    .ok_or_else(|| HarnessError::Protocol("Native message has no ID".into()))?;
                if !seen.insert(id.to_owned()) {
                    return Err(HarnessError::Protocol(
                        "Native transcript order is ambiguous".into(),
                    ));
                }
                messages.push(message.clone());
            }
            let next = if v2 {
                page.pointer("/cursor/next")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
            } else {
                None
            };
            if next.is_none() {
                break;
            }
            if next == cursor.as_deref() || messages.len() > 100_000 {
                return Err(HarnessError::Protocol(
                    "Native transcript pagination is ambiguous".into(),
                ));
            }
            cursor = next.map(str::to_owned);
        }
        Ok(messages)
    }

    async fn fork_verified(
        &self,
        point: &NativeForkPoint,
        source_idle: bool,
    ) -> Result<NativeForkResult, NativeForkError> {
        let reject = |e: HarnessError| NativeForkError::Rejected(e.to_string());
        let NativeForkBoundary::OpenCodeReply {
            assistant_message_id,
        } = &point.boundary
        else {
            return Err(NativeForkError::Rejected(
                "Expected an OpenCode reply".into(),
            ));
        };
        let admission = admission(self, Some(&point.cwd), &point.source_session_id);
        let _guard = admission.gate.lock().await;
        let source = self
            .fork_messages(&point.source_session_id, &point.cwd)
            .await
            .map_err(reject)?;
        let index = source
            .iter()
            .position(|m| message_id(m) == Some(assistant_message_id))
            .ok_or_else(|| NativeForkError::Rejected("Native reply no longer exists".into()))?;
        let reply = info(&source[index]);
        if reply["role"] != "assistant" && reply["type"] != "assistant"
            || reply
                .pointer("/time/completed")
                .and_then(Value::as_u64)
                .is_none()
        {
            return Err(NativeForkError::Rejected(
                "This reply is not a complete provider turn".into(),
            ));
        }
        let next = source.get(index + 1).and_then(message_id);
        if next.is_none()
            && (!source_idle
                || admission.active.load(Ordering::Acquire)
                || self
                    .session_running(&point.source_session_id, Some(&point.cwd))
                    .await
                    .map_err(reject)?)
        {
            return Err(NativeForkError::Rejected(
                "SourceBusy: the final native reply is still changing".into(),
            ));
        }
        let path = format!(
            "{}/session/{}/fork",
            if self.protocol().await == Protocol::V2 {
                "/api"
            } else {
                ""
            },
            point.source_session_id
        );
        // The verified 2.0.11 executable's /openapi.json names this exclusive
        // boundary `before`; v1 calls it messageID. Unknown versions are gated.
        let body = next.map_or_else(
            || json!({}),
            |id| {
                if self.protocol.get() == Some(&Protocol::V2) {
                    json!({"before":id})
                } else {
                    json!({"messageID":id})
                }
            },
        );
        let result = self
            .post_json(&path, Some(&point.cwd), &body)
            .await
            .map_err(|e| NativeForkError::Indeterminate(e.to_string()))?;
        let result = unwrap_data(result);
        let id = result["id"]
            .as_str()
            .filter(|id| !id.is_empty() && *id != point.source_session_id)
            .ok_or_else(|| {
                NativeForkError::Indeterminate("Provider returned no independent session ID".into())
            })?;
        let child = self
            .fork_messages(id, &point.cwd)
            .await
            .map_err(|e| NativeForkError::Indeterminate(e.to_string()))?;
        if child.len() != index + 1
            || child
                .iter()
                .zip(&source)
                .any(|(a, b)| canonical(a) != canonical(b))
        {
            let mismatch = child
                .iter()
                .zip(&source)
                .position(|(a, b)| canonical(a) != canonical(b));
            let fields = mismatch
                .map(|index| {
                    let a = canonical(&source[index]);
                    let b = canonical(&child[index]);
                    a.as_object()
                        .into_iter()
                        .flat_map(|m| m.keys())
                        .filter(|key| a.get(*key) != b.get(*key))
                        .cloned()
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            return Err(NativeForkError::Indeterminate(format!(
                "Provider did not preserve the selected native prefix (expected {} messages, received {}; mismatch {:?}, fields {:?})",
                index + 1,
                child.len(),
                mismatch,
                fields
            )));
        }
        Ok(NativeForkResult {
            session_id: id.into(),
            cwd: point.cwd.clone(),
        })
    }
}

impl OpencodeHarness {
    pub(super) async fn fork_support(&self, cwd: &std::path::Path) -> NativeForkAvailability {
        match self.server(cwd.to_str(), None).await {
            Ok(mut server) => {
                let _ = server.protocol().await;
                let supported = server
                    .version
                    .get()
                    .and_then(|v| v.number)
                    .is_some_and(|v| matches!(v, (1, 18, 33) | (2, 0, 11)));
                server.shutdown(self.kill_grace).await;
                if supported {
                    NativeForkAvailability::available()
                } else {
                    NativeForkAvailability::unavailable(
                        "This OpenCode version has no verified native fork contract",
                    )
                }
            }
            Err(_) => NativeForkAvailability::unavailable("OpenCode server is unavailable"),
        }
    }
    pub(super) async fn fork_at(
        &self,
        point: &NativeForkPoint,
        controls: NativeForkControls,
    ) -> Result<NativeForkResult, NativeForkError> {
        point.validate().map_err(NativeForkError::Rejected)?;
        let mut server = self
            .server(Some(&point.cwd), None)
            .await
            .map_err(|e| NativeForkError::Rejected(e.to_string()))?;
        let _ = server.protocol().await;
        if !server
            .version
            .get()
            .and_then(|v| v.number)
            .is_some_and(|v| matches!(v, (1, 18, 33) | (2, 0, 11)))
        {
            server.shutdown(self.kill_grace).await;
            return Err(NativeForkError::Rejected(
                "This OpenCode version has no verified native fork contract".into(),
            ));
        }
        let result = tokio::select! {
            result = tokio::time::timeout(controls.timeout, server.fork_verified(point, controls.source_idle)) => result.unwrap_or_else(|_| Err(NativeForkError::Indeterminate("OpenCode fork timed out".into()))),
            _ = controls.interrupt.cancelled() => Err(NativeForkError::Indeterminate("OpenCode fork cancelled".into())),
        };
        server.shutdown(self.kill_grace).await;
        drop(controls.execution_lease);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn native_fork_verifies_exclusive_boundary_on_both_wires() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        for v2 in [false, true] {
            for (overcopy, tail) in [(false, false), (true, false), (false, true)] {
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                let base = format!("http://{}", listener.local_addr().unwrap());
                let traffic = Arc::new(std::sync::Mutex::new(Vec::new()));
                let captured = traffic.clone();
                let server = tokio::spawn(async move {
                    loop {
                        let (mut stream, _) = listener.accept().await.unwrap();
                        let mut data = vec![0u8; 16384];
                        let count = stream.read(&mut data).await.unwrap();
                        let request = String::from_utf8_lossy(&data[..count]).to_string();
                        captured.lock().unwrap().push(request.clone());
                        let path = request.split_whitespace().nth(1).unwrap();
                        let message = |id: &str, role: &str, text: &str| {
                            if v2 {
                                json!({"id":id,"type":role,"time":{"created":1,"completed":2},"content":[{"type":"text","text":text}]})
                            } else {
                                json!({"info":{"id":id,"role":role,"time":{"created":1,"completed":2}},"parts":[{"id":format!("p{id}"),"messageID":id,"type":"text","text":text}]})
                            }
                        };
                        let mut source = vec![
                            message("u1", "user", "initial"),
                            message("a1", "assistant", "answer"),
                            message("u2", "user", "later secret"),
                        ];
                        if tail {
                            source.pop();
                        }
                        let response = if request.starts_with("POST") {
                            assert!(path.split('?').next().unwrap().ends_with("/fork"));
                            assert_eq!(
                                request.contains(if v2 {
                                    "\"before\":\"u2\""
                                } else {
                                    "\"messageID\":\"u2\""
                                }),
                                !tail
                            );
                            json!({"id":"child"})
                        } else if path.split('?').next().unwrap().ends_with("/status")
                            || path.split('?').next().unwrap().ends_with("/active")
                        {
                            json!({})
                        } else if path.contains("/source/message") {
                            json!(source)
                        } else if path.contains("/child/message") {
                            let mut child = source[..if overcopy { 3 } else { 2 }].to_vec();
                            for (index, m) in child.iter_mut().enumerate() {
                                if v2 {
                                    m["id"] = json!(format!("remapped{index}"));
                                } else {
                                    m["info"]["id"] = json!(format!("remapped{index}"));
                                    m["parts"][0]["id"] = json!(format!("newpart{index}"));
                                    m["parts"][0]["messageID"] = json!(format!("remapped{index}"));
                                }
                            }
                            json!(child)
                        } else {
                            panic!("unexpected request {path}")
                        };
                        let paged = v2 && path.contains("/message");
                        let response = if paged && path.contains("cursor=") {
                            assert!(!path.contains("order="));
                            json!([])
                        } else {
                            response
                        };
                        let body = if v2 {
                            json!({"data":response,"cursor":{"next":if paged && !path.contains("cursor=") { Some("next-page") } else { None }}})
                        } else {
                            response
                        }
                        .to_string();
                        stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
                    }
                });
                let client = Server::attached(base);
                client
                    .protocol
                    .set(if v2 { Protocol::V2 } else { Protocol::V1 })
                    .unwrap();
                let point = NativeForkPoint {
                    format_version: 1,
                    harness: HarnessId::Opencode,
                    source_device_id: "host".into(),
                    source_session_id: "source".into(),
                    cwd: "/project".into(),
                    boundary: NativeForkBoundary::OpenCodeReply {
                        assistant_message_id: "a1".into(),
                    },
                };
                let result = client.fork_verified(&point, tail).await;
                assert_eq!(result.is_ok(), !overcopy, "{result:?}");
                assert_eq!(
                    traffic.lock().unwrap().len(),
                    3 + usize::from(tail) + if v2 { 2 } else { 0 }
                );
                let before = traffic
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|r| r.starts_with("POST"))
                    .count();
                let mut invalid = point.clone();
                invalid.boundary = NativeForkBoundary::OpenCodeReply {
                    assistant_message_id: "missing".into(),
                };
                assert!(matches!(
                    client.fork_verified(&invalid, true).await,
                    Err(NativeForkError::Rejected(_))
                ));
                if tail {
                    assert!(matches!(
                        client.fork_verified(&point, false).await,
                        Err(NativeForkError::Rejected(_))
                    ));
                    let admission = admission(&client, Some(&point.cwd), &point.source_session_id);
                    admission.active.store(true, Ordering::Release);
                    assert!(matches!(
                        client.fork_verified(&point, true).await,
                        Err(NativeForkError::Rejected(_))
                    ));
                    admission.active.store(false, Ordering::Release);
                }
                assert_eq!(
                    traffic
                        .lock()
                        .unwrap()
                        .iter()
                        .filter(|r| r.starts_with("POST"))
                        .count(),
                    before
                );
                server.abort();
            }
        }
    }
}
