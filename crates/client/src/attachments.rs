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

use crate::lock;

/// Body used when a message has only attachments.
pub const ATTACHMENT_ONLY_TEXT: &str = "See the attached image(s).";
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
        text: if visible == ATTACHMENT_ONLY_TEXT {
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
}
