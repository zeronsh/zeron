//! Attachment bytes in transit to a Cloud machine
//! (`/blob/{chatId}/attachments/{uploadId}`, edge/src/attachment-blobs.ts;
//! docs/design/cloud-attachments.md).
//!
//! A peer link can't reach a sleeping machine, so for a Cloud-hosted chat the
//! sender stores the bytes at the edge FIRST, and only then writes the message.
//! The machine downloads them when it wakes. Until the upload lands, the send
//! waits in a durable local outbox ([`crate::DocsStore::hold_send`]), which is
//! why this module also carries the shapes the desktop engine and the mobile
//! core share: both hold sends in the same table.

use std::time::Duration;

use futures::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeron_doc::{QueuedMessage, SessionCommandPayload};

/// The edge's cap (attachment-blobs.ts) and the engine's assembled-upload cap
/// (uploads.rs `MAX_BYTES`).
pub const MAX_STORED_ATTACHMENT_BYTES: u64 = 32 * 1024 * 1024;

/// Carries the object's hex SHA-256 both ways.
pub const SHA256_HEADER: &str = "x-zeron-sha256";

/// A held send never stops trying on its own: only its person removes it. After
/// this long it tries less often ([`held_backoff_cap`]).
pub const HELD_SEND_SLOW_AFTER_MS: i64 = zeron_doc::CLOUD_COMMAND_TTL_MS;
/// The longest wait between tries for a send older than [`HELD_SEND_SLOW_AFTER_MS`].
pub const HELD_SEND_SLOW_BACKOFF: Duration = Duration::from_secs(15 * 60);

/// The longest wait between a held send's tries: `normal` for its first week,
/// then [`HELD_SEND_SLOW_BACKOFF`]. A network or system wake still cuts a wait short.
pub fn held_backoff_cap(age_ms: i64, normal: Duration) -> Duration {
    if age_ms > HELD_SEND_SLOW_AFTER_MS {
        HELD_SEND_SLOW_BACKOFF.max(normal)
    } else {
        normal
    }
}

/// What a send says when the store is full. It frees up as machines collect what
/// they're sent.
pub const STORE_FULL: &str =
    "too many files are waiting for Cloud machines to collect them; retry once one has";

const STREAM_CHUNK: usize = 256 * 1024;
/// Generous for a slow uplink: 2 minutes plus 16 KB/s (a full-size file gets
/// about 36 minutes). The shared clients' short timeouts are for small bodies.
fn transfer_timeout(bytes: u64) -> Duration {
    Duration::from_secs(120 + bytes / (16 * 1024))
}
const FETCH_TIMEOUT: Duration = Duration::from_secs(45 * 60);
const SHORT_TIMEOUT: Duration = Duration::from_secs(30);

/// One file of a held send: the bytes are already staged on this device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HeldUpload {
    pub upload_id: String,
    pub file_name: String,
}

/// What goes into the chat once the bytes are stored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum HeldBody {
    /// A Run or Steer; its command id is the outbox row's id.
    Command { payload: SessionCommandPayload },
    /// A queue row (the row's own id is the outbox row's id).
    Row { item: QueuedMessage },
}

/// A send waiting in the outbox.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HeldPayload {
    pub body: HeldBody,
    pub uploads: Vec<HeldUpload>,
    /// The host's version isn't known, so it may not fetch from the store:
    /// also push the bytes over the peer link once the message is out.
    #[serde(default)]
    pub also_peer: bool,
}

impl HeldPayload {
    /// The transcript message this send becomes, if it names one.
    pub fn message_id(&self) -> Option<&str> {
        match &self.body {
            HeldBody::Command {
                payload: SessionCommandPayload::Run { message_id, .. },
            } => Some(message_id),
            HeldBody::Command {
                payload:
                    SessionCommandPayload::Steer {
                        message_id: Some(message_id),
                        ..
                    },
            } => Some(message_id),
            _ => None,
        }
    }

    /// The prompt as the transcript shows it (with its attachment trailer).
    pub fn prompt(&self) -> &str {
        match &self.body {
            HeldBody::Command {
                payload: SessionCommandPayload::Run { request, .. },
            } => &request.prompt,
            HeldBody::Command {
                payload: SessionCommandPayload::Steer { prompt, .. },
            } => prompt,
            HeldBody::Command { .. } => "",
            HeldBody::Row { item } => &item.text,
        }
    }
}

/// One row of the outbox.
#[derive(Debug, Clone, PartialEq)]
pub struct HeldSend {
    pub id: String,
    pub chat_id: String,
    pub payload: HeldPayload,
    pub created_at: i64,
    /// Why it stopped trying. Only the user's retry clears it.
    pub failure: Option<String>,
}

/// Why storing a file didn't work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PutError {
    /// Network, token, or edge trouble: try again.
    Transient(String),
    /// The edge refused for good; the message says why.
    Permanent(String),
    /// This edge has no attachment store yet. The peer path still works.
    Unsupported,
}

impl std::fmt::Display for PutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transient(why) | Self::Permanent(why) => f.write_str(why),
            Self::Unsupported => f.write_str("the edge has no attachment store"),
        }
    }
}

/// What the store holds for a ref.
#[derive(Debug, PartialEq, Eq)]
pub enum Fetched {
    Bytes(Vec<u8>),
    /// Nothing stored (or not this caller's to read): the bytes, if any, are
    /// coming some other way.
    Absent,
}

pub fn blob_url(edge: &str, chat_id: &str, upload_id: &str) -> String {
    format!(
        "{}/blob/{chat_id}/attachments/{upload_id}",
        edge.trim_end_matches('/')
    )
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn put_status_error(status: u16) -> Option<PutError> {
    match status {
        200..=299 => None,
        // The route doesn't exist on this edge (an older deployment).
        404 | 405 => Some(PutError::Unsupported),
        413 => Some(PutError::Permanent(
            "the file is too large to send to this machine".into(),
        )),
        507 => Some(PutError::Permanent(STORE_FULL.into())),
        400 | 403 | 411 => Some(PutError::Permanent(format!(
            "the file was refused (HTTP {status})"
        ))),
        // 401 (a token about to refresh), 408, 429, 5xx.
        _ => Some(PutError::Transient(format!("upload HTTP {status}"))),
    }
}

/// Store one file. `progress` hears the bytes handed to the socket.
pub async fn put(
    http: &reqwest::Client,
    url: &str,
    bearer: &str,
    bytes: Vec<u8>,
    progress: impl Fn(u64) + Send + Sync + 'static,
) -> Result<(), PutError> {
    let total = bytes.len() as u64;
    if total > MAX_STORED_ATTACHMENT_BYTES {
        return Err(PutError::Permanent(
            "the file is too large to send to this machine".into(),
        ));
    }
    let sha256 = sha256_hex(&bytes);
    let mut sent = 0u64;
    let chunks = (0..bytes.len().div_ceil(STREAM_CHUNK)).map(move |ix| {
        let end = ((ix + 1) * STREAM_CHUNK).min(bytes.len());
        bytes[ix * STREAM_CHUNK..end].to_vec()
    });
    let body = futures::stream::iter(chunks).map(move |chunk| {
        sent += chunk.len() as u64;
        progress(sent);
        Ok::<_, std::convert::Infallible>(chunk)
    });
    let response = http
        .put(url)
        .bearer_auth(bearer)
        .header(reqwest::header::CONTENT_LENGTH, total)
        .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
        .header(SHA256_HEADER, sha256)
        .timeout(transfer_timeout(total))
        .body(reqwest::Body::wrap_stream(body))
        .send()
        .await
        .map_err(|err| PutError::Transient(format!("upload failed: {}", err.without_url())))?;
    match put_status_error(response.status().as_u16()) {
        None => Ok(()),
        Some(err) => Err(err),
    }
}

/// Download one file, checked against the checksum it was stored with.
/// `Err` is worth retrying; [`Fetched::Absent`] is an answer.
pub async fn get(http: &reqwest::Client, url: &str, bearer: &str) -> Result<Fetched, String> {
    let response = http
        .get(url)
        .bearer_auth(bearer)
        .timeout(FETCH_TIMEOUT)
        .send()
        .await
        .map_err(|err| format!("download failed: {}", err.without_url()))?;
    let status = response.status().as_u16();
    // 403: this machine isn't the chat's host, or doesn't know it yet. The
    // gate never caches a refusal, so the next look gets a fresh answer.
    if matches!(status, 403..=405) {
        return Ok(Fetched::Absent);
    }
    if !response.status().is_success() {
        return Err(format!("download HTTP {status}"));
    }
    let expected = response
        .headers()
        .get(SHA256_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_ascii_lowercase);
    let body = response
        .bytes()
        .await
        .map_err(|err| format!("download body failed: {}", err.without_url()))?;
    if let Some(expected) = expected
        && sha256_hex(&body) != expected
    {
        return Err("downloaded bytes don't match their checksum".into());
    }
    Ok(Fetched::Bytes(body.to_vec()))
}

/// Best-effort: the bucket's lifecycle rule catches a delete that never lands.
pub async fn delete(http: &reqwest::Client, url: &str, bearer: &str) -> Result<(), String> {
    let response = http
        .delete(url)
        .bearer_auth(bearer)
        .timeout(SHORT_TIMEOUT)
        .send()
        .await
        .map_err(|err| format!("delete failed: {}", err.without_url()))?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(format!("delete HTTP {}", response.status().as_u16()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_a_known_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn put_statuses_say_whether_to_retry_or_fall_back() {
        assert_eq!(put_status_error(200), None);
        assert_eq!(put_status_error(404), Some(PutError::Unsupported));
        assert_eq!(put_status_error(405), Some(PutError::Unsupported));
        assert!(matches!(
            put_status_error(413),
            Some(PutError::Permanent(_))
        ));
        assert_eq!(
            put_status_error(507),
            Some(PutError::Permanent(STORE_FULL.into()))
        );
        assert!(matches!(
            put_status_error(403),
            Some(PutError::Permanent(_))
        ));
        assert!(matches!(
            put_status_error(401),
            Some(PutError::Transient(_))
        ));
        assert!(matches!(
            put_status_error(429),
            Some(PutError::Transient(_))
        ));
        assert!(matches!(
            put_status_error(503),
            Some(PutError::Transient(_))
        ));
    }

    #[test]
    fn an_old_held_send_tries_less_often_but_never_stops() {
        let normal = Duration::from_secs(30);
        assert_eq!(held_backoff_cap(0, normal), normal);
        assert_eq!(held_backoff_cap(HELD_SEND_SLOW_AFTER_MS, normal), normal);
        assert_eq!(
            held_backoff_cap(HELD_SEND_SLOW_AFTER_MS + 1, normal),
            HELD_SEND_SLOW_BACKOFF
        );
        assert_eq!(held_backoff_cap(i64::MAX, normal), HELD_SEND_SLOW_BACKOFF);
    }

    #[test]
    fn urls_name_the_chat_and_upload() {
        assert_eq!(
            blob_url("https://edge.test/", "c1", "u1"),
            "https://edge.test/blob/c1/attachments/u1"
        );
    }

    #[test]
    fn a_held_payload_round_trips_and_names_its_message() {
        let payload = HeldPayload {
            body: HeldBody::Command {
                payload: SessionCommandPayload::Steer {
                    prompt: "look".into(),
                    message_id: Some("m1".into()),
                },
            },
            uploads: vec![HeldUpload {
                upload_id: "u1".into(),
                file_name: "a.png".into(),
            }],
            also_peer: false,
        };
        let json = serde_json::to_value(&payload).unwrap();
        assert_eq!(json["uploads"][0]["uploadId"], "u1");
        let read: HeldPayload = serde_json::from_value(json).unwrap();
        assert_eq!(read, payload);
        assert_eq!(read.message_id(), Some("m1"));
        assert_eq!(read.prompt(), "look");
    }
}
