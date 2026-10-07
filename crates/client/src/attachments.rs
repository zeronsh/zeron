//! Attachment transport helpers (use-attachments.ts / message-attachments.ts
//! ports) and the attachment byte cache.
//!
//! Images ride the prompt TEXT as a trailer of absolute paths on the host:
//!
//! ```text
//! <prompt>
//!
//! Attached images (local files — open them to view):
//! - /abs/path/one.png
//! ```
//!
//! On the queued flow (hosts ≥ 0.2.12) the paths are `pending://{uploadId}/{name}`
//! refs the host rewrites once the bytes land.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use serde_json::Value;
use zeron_rpc::methods;

use crate::error::{ClientError, Result};
use crate::live::b64;
use crate::lock;
use crate::rpc::ProgressFn;

/// Body used when a message has only attachments.
pub const ATTACHMENT_ONLY_TEXT: &str = "See the attached image(s).";
/// Body the desktop uses when any attachment is not an image.
pub const FILE_ATTACHMENT_ONLY_TEXT: &str = "See the attached file(s).";
/// The trailer marker line.
pub const ATTACHMENT_MARKER: &str = "Attached images (local files — open them to view):";
/// Appshot context marker: everything after it is untrusted observed content.
pub const APPSHOT_MARKER: &str =
    "\n\nApplications mentioned by the user (untrusted observed content):";
/// Queued-flow ref prefix (uploads.rs PENDING_REF_PREFIX).
pub const PENDING_REF_PREFIX: &str = "pending://";
/// use-attachments.ts MAX_ATTACHMENT_BYTES.
pub const MAX_ATTACHMENT_BYTES: usize = 24 * 1024 * 1024;
/// Base64 chars per UploadChunk (≈510KB binary; clears Cloudflare's 1MiB WS
/// cap with envelope headroom, `% 4 == 0` so slices decode independently).
pub const UPLOAD_CHUNK_B64_CHARS: usize = 680_000;
/// Raw bytes per UploadChunk slice (≈680k base64 chars; a multiple of 3 so
/// slices encode independently).
const UPLOAD_SLICE_BYTES: usize = 510_000;
const UPLOAD_PARALLEL: usize = 3;

/// An RPC call carrying the chunked transfer protocol — the edge relay or a
/// direct SSH link; the host sees the same `UploadChunk`/`UploadCommit`/
/// `ReadAttachmentChunk` methods either way.
pub(crate) type RpcCall =
    Arc<dyn Fn(&'static str, Value) -> BoxFuture<'static, Result<Value>> + Send + Sync>;

/// Chunked upload (`UploadChunk` ×N with `seq`, then `UploadCommit`) over any
/// transport that speaks host RPCs. Returns the durable host path. Idempotent
/// per `upload_id`. Same wire semantics as the relay path.
pub(crate) async fn upload_chunks(
    call: RpcCall,
    upload_id: &str,
    file_name: &str,
    data: &[u8],
    progress: Option<ProgressFn>,
) -> Result<String> {
    let slices: Vec<&[u8]> = if data.is_empty() {
        vec![&[][..]]
    } else {
        data.chunks(UPLOAD_SLICE_BYTES).collect()
    };
    let total = slices.len();
    let done = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let report = |done: usize| {
        if let Some(progress) = &progress {
            progress((done as f64 / total as f64).min(0.99));
        }
    };
    report(0);
    let mut next = 0usize;
    while next < total {
        let batch: Vec<(usize, &[u8])> = slices
            .iter()
            .enumerate()
            .skip(next)
            .take(UPLOAD_PARALLEL)
            .map(|(i, s)| (i, *s))
            .collect();
        next += batch.len();
        let calls = batch.into_iter().map(|(seq, slice)| {
            let done = done.clone();
            let call = call.clone();
            let report = &report;
            async move {
                let params =
                    serde_json::json!({ "uploadId": upload_id, "seq": seq, "data": b64::encode(slice) });
                let mut last = None;
                for attempt in 0..3u64 {
                    if attempt > 0 {
                        tokio::time::sleep(std::time::Duration::from_millis(
                            50 * attempt * (seq as u64 + 1),
                        ))
                        .await;
                    }
                    match call(methods::UPLOAD_CHUNK, params.clone()).await {
                        Ok(_) => {
                            let now =
                                done.fetch_add(1, std::sync::atomic::Ordering::AcqRel) + 1;
                            report(now);
                            return Ok(());
                        }
                        Err(
                            err @ (ClientError::Unsupported(_) | ClientError::HostError(_)),
                        ) => return Err(err),
                        Err(err) => last = Some(err),
                    }
                }
                Err(last.unwrap_or(ClientError::HostUnavailable("upload".into())))
            }
        });
        for result in futures::future::join_all(calls).await {
            result?;
        }
    }
    let reply = call(
        methods::UPLOAD_COMMIT,
        serde_json::json!({ "uploadId": upload_id, "fileName": file_name }),
    )
    .await?;
    if let Some(progress) = &progress {
        progress(1.0);
    }
    reply
        .get("path")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| ClientError::HostError("UploadCommit returned no path".into()))
}

/// `ReadAttachmentChunk` until done, over any transport that speaks host RPCs.
pub(crate) async fn read_chunks(call: RpcCall, path: &str) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut offset: u64 = 0;
    for _ in 0..1000 {
        let reply = call(
            methods::READ_ATTACHMENT_CHUNK,
            serde_json::json!({ "path": path, "offset": offset }),
        )
        .await?;
        let data = reply
            .get("data")
            .and_then(Value::as_str)
            .unwrap_or_default();
        out.extend(
            b64::decode(data)
                .ok_or_else(|| ClientError::HostError("bad attachment chunk".into()))?,
        );
        if reply.get("done").and_then(Value::as_bool).unwrap_or(true) {
            return Ok(out);
        }
        let next = reply
            .get("nextOffset")
            .and_then(Value::as_u64)
            .unwrap_or(offset);
        if next <= offset {
            return Err(ClientError::HostError(
                "attachment read made no progress".into(),
            ));
        }
        offset = next;
    }
    Err(ClientError::HostError("attachment too large".into()))
}

/// Append the attachment trailer (no-op without paths).
pub fn with_attachments(text: &str, paths: &[String]) -> String {
    if paths.is_empty() {
        return text.to_owned();
    }
    let body = if text.trim().is_empty() {
        ATTACHMENT_ONLY_TEXT
    } else {
        text
    };
    let refs = paths
        .iter()
        .map(|p| format!("- {p}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!("{body}\n\n{ATTACHMENT_MARKER}\n{refs}")
}

/// The host's upload-name rule (uploads.rs `sanitize`): basename, anything
/// outside `[A-Za-z0-9._-]` → `_`, last 80 chars, `upload` when empty. A
/// `pending://` ref must name exactly what the escort commits.
pub fn upload_file_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let cleaned: String = base
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let tail: String = cleaned
        .chars()
        .rev()
        .take(80)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    if tail.is_empty() {
        "upload".to_owned()
    } else {
        tail
    }
}

pub fn pending_ref(upload_id: &str, name: &str) -> String {
    format!("{PENDING_REF_PREFIX}{upload_id}/{name}")
}

/// `(upload_id, file_name)` of a pending ref.
pub fn parse_pending_ref(reference: &str) -> Option<(&str, &str)> {
    let body = reference.strip_prefix(PENDING_REF_PREFIX)?;
    let (id, name) = body.split_once('/')?;
    (!id.is_empty() && !name.is_empty()).then_some((id, name))
}

/// Upload ids a command still needs landed — `Run` lists refs in
/// `attachments`, `Steer` embeds `- pending://…` lines (mirrors the
/// engine's `command_transfers`/`pending_refs_in`).
pub(crate) fn command_pending_uploads(
    entry: &zeron_doc::SessionCommandEntry,
) -> Vec<String> {
    let refs: Vec<String> = match &entry.payload {
        zeron_doc::SessionCommandPayload::Run { request, .. } => request
            .attachments
            .iter()
            .filter(|p| p.starts_with(PENDING_REF_PREFIX))
            .cloned()
            .collect(),
        zeron_doc::SessionCommandPayload::Steer { prompt, .. } => prompt
            .lines()
            .filter_map(|line| {
                let path = line.trim_start().strip_prefix("- ")?.trim();
                path.starts_with(PENDING_REF_PREFIX)
                    .then(|| path.to_string())
            })
            .collect(),
        _ => Vec::new(),
    };
    refs.iter()
        .filter_map(|r| parse_pending_ref(r).map(|(id, _)| id.to_owned()))
        .collect()
}

/// Source labels for one Appshot capture (observed text never surfaces).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppshotLabel {
    pub app_name: String,
    pub window_title: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserImage {
    pub path: String,
    pub name: String,
    pub appshot: Option<AppshotLabel>,
}

/// A user message split into its visible prompt and attachment refs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedUserMessage {
    pub text: String,
    pub images: Vec<UserImage>,
}

/// The visible part of a prompt (drops the Appshot context).
pub fn visible_text(text: &str) -> &str {
    match text.find(APPSHOT_MARKER) {
        Some(at) => text[..at].trim(),
        None => text,
    }
}

fn attribute(tag: &str, name: &str) -> Option<String> {
    let needle = format!("{name}=\"");
    let start = tag.find(&needle)? + needle.len();
    let end = tag[start..].find('"')? + start;
    Some(
        tag[start..end]
            .replace("&quot;", "\"")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&amp;", "&"),
    )
}

fn appshot_labels(text: &str) -> HashMap<String, AppshotLabel> {
    let Some(at) = text.find(APPSHOT_MARKER) else {
        return HashMap::new();
    };
    let mut out = HashMap::new();
    let mut rest = &text[at + APPSHOT_MARKER.len()..];
    while let Some(open) = rest.find("<appshot ") {
        let Some(close) = rest[open..].find('>') else {
            break;
        };
        let tag = &rest[open..open + close];
        if let (Some(image), Some(app)) = (attribute(tag, "image"), attribute(tag, "app"))
            && !app.trim().is_empty()
        {
            out.insert(
                image,
                AppshotLabel {
                    app_name: app,
                    window_title: attribute(tag, "window-title").filter(|t| !t.is_empty()),
                },
            );
        }
        rest = &rest[open + close..];
    }
    out
}

/// message-attachments.ts `parseUserMessageImages`.
pub fn parse_user_message(content: &str) -> ParsedUserMessage {
    let labels = appshot_labels(content);
    let lines: Vec<&str> = content.split('\n').collect();
    let marker = (1..lines.len()).find(|&ix| {
        let line = lines[ix].trim();
        lines[ix - 1].trim().is_empty()
            && line
                .to_lowercase()
                .starts_with("attached images (local files")
            && line.ends_with("):")
    });
    let Some(marker) = marker else {
        return ParsedUserMessage {
            text: visible_text(content).to_owned(),
            images: Vec::new(),
        };
    };
    let images: Vec<UserImage> = lines[marker + 1..]
        .iter()
        .filter_map(|line| {
            let path = line.trim().strip_prefix("- ")?.trim();
            (!path.is_empty()).then(|| UserImage {
                path: path.to_owned(),
                name: path
                    .rsplit('/')
                    .next()
                    .filter(|n| !n.is_empty())
                    .unwrap_or("image")
                    .to_owned(),
                appshot: labels.get(path).cloned(),
            })
        })
        .collect();
    if images.is_empty() {
        return ParsedUserMessage {
            text: visible_text(content).to_owned(),
            images,
        };
    }
    let body = lines[..marker - 1].join("\n");
    let visible = visible_text(body.trim()).to_owned();
    ParsedUserMessage {
        text: if visible == ATTACHMENT_ONLY_TEXT || visible == FILE_ATTACHMENT_ONLY_TEXT {
            String::new()
        } else {
            visible
        },
        images,
    }
}

/// Queue rows show user text only — never the transport trailer.
pub fn queue_visible_text(text: &str, attachments: &[String]) -> String {
    if attachments.is_empty() {
        return visible_text(text).to_owned();
    }
    let parsed = parse_user_message(text);
    if parsed.text.is_empty() {
        ATTACHMENT_ONLY_TEXT.to_owned()
    } else {
        parsed.text
    }
}

/// Byte-budgeted LRU for attachment reads (`(device, path)` → bytes).
pub(crate) struct AttachmentCache {
    budget: usize,
    inner: Mutex<CacheInner>,
}

#[derive(Default)]
struct CacheInner {
    bytes: usize,
    map: HashMap<(String, String), Arc<Vec<u8>>>,
    order: VecDeque<(String, String)>,
}

impl AttachmentCache {
    pub(crate) fn new(budget: usize) -> Self {
        Self {
            budget,
            inner: Mutex::new(CacheInner::default()),
        }
    }

    pub(crate) fn get(&self, device_id: &str, path: &str) -> Option<Arc<Vec<u8>>> {
        let mut inner = lock(&self.inner);
        let key = (device_id.to_owned(), path.to_owned());
        let hit = inner.map.get(&key).cloned()?;
        if let Some(at) = inner.order.iter().position(|k| *k == key) {
            inner.order.remove(at);
        }
        inner.order.push_back(key);
        Some(hit)
    }

    pub(crate) fn put(&self, device_id: &str, path: &str, data: Arc<Vec<u8>>) {
        let mut inner = lock(&self.inner);
        let key = (device_id.to_owned(), path.to_owned());
        if let Some(old) = inner.map.insert(key.clone(), data.clone()) {
            inner.bytes -= old.len();
            if let Some(at) = inner.order.iter().position(|k| *k == key) {
                inner.order.remove(at);
            }
        }
        inner.bytes += data.len();
        inner.order.push_back(key);
        while inner.bytes > self.budget && inner.order.len() > 1 {
            let Some(oldest) = inner.order.pop_front() else {
                break;
            };
            if let Some(evicted) = inner.map.remove(&oldest) {
                inner.bytes -= evicted.len();
            }
        }
    }

    pub(crate) fn clear(&self) {
        *lock(&self.inner) = CacheInner::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trailer_round_trips() {
        let paths = vec!["/tmp/a.png".to_owned(), pending_ref("u1", "b.png")];
        let text = with_attachments("look", &paths);
        let parsed = parse_user_message(&text);
        assert_eq!(parsed.text, "look");
        assert_eq!(parsed.images.len(), 2);
        assert_eq!(parsed.images[1].name, "b.png");
        assert_eq!(parse_pending_ref(&paths[1]), Some(("u1", "b.png")));
        let only = with_attachments("", &paths[..1]);
        assert_eq!(parse_user_message(&only).text, "");
        assert_eq!(queue_visible_text(&only, &paths[..1]), ATTACHMENT_ONLY_TEXT);
        let files = format!(
            "{FILE_ATTACHMENT_ONLY_TEXT}\n\n{ATTACHMENT_MARKER}\n- /tmp/ab12cd34-notes.zip"
        );
        assert_eq!(parse_user_message(&files).text, "");
    }

    #[test]
    fn command_pending_uploads_reads_run_and_steer_refs() {
        use zeron_doc::{SessionCommandEntry, SessionCommandPayload, SessionCommandStatus};
        let entry = |payload| SessionCommandEntry {
            id: "c".into(),
            payload,
            issued_by: "me".into(),
            issued_at: 0,
            based_on: None,
            expires_at: None,
            status: SessionCommandStatus::Pending,
            resolution: None,
        };
        let run = entry(SessionCommandPayload::Run {
            request: zeron_proto::RunRequest {
                prompt: "p".into(),
                harness: None,
                model: None,
                reasoning: None,
                model_options: Default::default(),
                cwd: "~".into(),
                sandbox: zeron_proto::SandboxLevel::WorkspaceWrite,
                auto_approve: true,
                resume: None,
                attachments: vec!["/abs/a.png".into(), pending_ref("u1", "b.png")],
                worktree: None,
                mcp: None,
            },
            message_id: "m".into(),
        });
        assert_eq!(command_pending_uploads(&run), ["u1"]);
        let steer = entry(SessionCommandPayload::Steer {
            prompt: "go\n- pending://u2/c.png\n- /abs/d.png".into(),
            message_id: None,
        });
        assert_eq!(command_pending_uploads(&steer), ["u2"]);
        assert!(
            command_pending_uploads(&entry(SessionCommandPayload::Interrupt {})).is_empty()
        );
    }

    #[test]
    fn appshot_context_stays_hidden() {
        let text = format!(
            "Compare.{APPSHOT_MARKER}\n<appshot app=\"Notes\" window-title=\"Plan\" image=\"/a.png\">SECRET</appshot>"
        );
        let full = with_attachments(&text, &["/a.png".to_owned()]);
        let parsed = parse_user_message(&full);
        assert_eq!(parsed.text, "Compare.");
        assert_eq!(parsed.images[0].appshot.as_ref().unwrap().app_name, "Notes");
        assert!(!parsed.text.contains("SECRET"));
    }

    #[test]
    fn cache_evicts_oldest_over_budget() {
        let cache = AttachmentCache::new(10);
        cache.put("d", "a", Arc::new(vec![0; 6]));
        cache.put("d", "b", Arc::new(vec![0; 6]));
        assert!(cache.get("d", "a").is_none());
        assert!(cache.get("d", "b").is_some());
    }

    /// A scripted RPC endpoint: records (method, params) calls and serves
    /// canned replies — the upload/commit/read flow is transport-agnostic,
    /// so a closure stands in for the relay AND the direct link.
    fn scripted(
        calls: Arc<Mutex<Vec<(&'static str, Value)>>>,
        answer: impl Fn(&'static str, &Value) -> Value + Send + Sync + 'static,
    ) -> RpcCall {
        let answer = Arc::new(answer);
        Arc::new(move |method, params| {
            let calls = calls.clone();
            let answer = answer.clone();
            Box::pin(async move {
                lock(&calls).push((method, params.clone()));
                Ok(answer(method, &params))
            })
        })
    }

    #[tokio::test]
    async fn upload_chunks_slices_and_commits() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let seen = calls.clone();
        let call = scripted(calls, |method, _| match method {
            methods::UPLOAD_CHUNK => serde_json::json!({ "ok": true }),
            methods::UPLOAD_COMMIT => serde_json::json!({ "path": "/u/x.png" }),
            _ => panic!("unexpected {method}"),
        });
        let data = vec![7u8; UPLOAD_SLICE_BYTES + 10]; // two slices
        let path = upload_chunks(call, "up1", "x.png", &data, None)
            .await
            .unwrap();
        assert_eq!(path, "/u/x.png");
        let calls = lock(&seen);
        assert_eq!(calls.len(), 3); // 2 chunks + commit
        let mut seqs: Vec<u64> = calls
            .iter()
            .filter(|(m, _)| *m == methods::UPLOAD_CHUNK)
            .map(|(_, p)| p["seq"].as_u64().unwrap())
            .collect();
        seqs.sort();
        assert_eq!(seqs, [0, 1]);
        // Slice 0 round-trips the full slice; slice 1 the tail.
        let first = calls
            .iter()
            .find(|(m, p)| *m == methods::UPLOAD_CHUNK && p["seq"] == 0)
            .unwrap()
            .1["data"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(b64::decode(&first).unwrap().len(), UPLOAD_SLICE_BYTES);
        let last = calls.last().unwrap();
        assert_eq!(last.0, methods::UPLOAD_COMMIT);
        assert_eq!(last.1["uploadId"], "up1");
        assert_eq!(last.1["fileName"], "x.png");
    }

    #[tokio::test]
    async fn upload_chunks_empty_payload_commits_one_slice() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let seen = calls.clone();
        let call = scripted(calls, |method, _| match method {
            methods::UPLOAD_CHUNK => serde_json::json!({ "ok": true }),
            methods::UPLOAD_COMMIT => serde_json::json!({ "path": "/u/x.png" }),
            _ => panic!("unexpected {method}"),
        });
        // Empty payload still produces exactly one chunk + commit.
        let path = upload_chunks(call, "up2", "x.png", &[], None).await.unwrap();
        assert_eq!(path, "/u/x.png");
        assert_eq!(lock(&seen).len(), 2);
    }

    #[tokio::test]
    async fn read_chunks_walks_offsets_until_done() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let payload = b64::encode(b"hello");
        let call = scripted(calls, move |method, params| {
            assert_eq!(method, methods::READ_ATTACHMENT_CHUNK);
            if params["offset"] == 0 {
                serde_json::json!({ "data": payload, "nextOffset": 5, "done": false })
            } else {
                serde_json::json!({ "data": b64::encode(b" world"), "done": true })
            }
        });
        let bytes = read_chunks(call, "/a.png").await.unwrap();
        assert_eq!(bytes, b"hello world");
    }
}
