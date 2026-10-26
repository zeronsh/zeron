//! chat3 transcript plane wire codec and transport seam (docs/chat3-sync.md).
//!
//! The plane rides the chat2 frame layout (`chat_frames`): stored rows are
//! `item` frames, open parts are `live` frames, a page or feed ends with
//! `itemsDone`. Host upserts go up as a length-prefixed run of `itemPut`
//! frames in one `POST /items` (the JSON reply is the ack); pages and the
//! change feed come back the same way from `GET /items`, the `/rows` body
//! encoding. Live frames ride the chat's existing socket.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, Weak};

use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;
use zeron_doc::SessionDoc;
use zeron_doc::transcript_plane::{Item, ItemKey, Live, LiveOp, Reader, ReceivedItem};

use crate::SyncError;
use crate::chat_frames::{self as wire, frame_type};

/// Rows per page: about a screen and a half of a busy turn.
pub const PAGE_LIMIT: usize = 200;

/// `GET/POST /chat3/{id}/items` and `GET/PUT /chat3/{id}/outputs/{part}`.
/// Engine and app clients implement it over their own HTTP stacks.
pub trait TranscriptTransport: Send + Sync + 'static {
    /// POST a length-prefixed `itemPut` body; returns the JSON ack.
    fn post_items(&self, body: Vec<u8>) -> BoxFuture<'static, Result<String, SyncError>>;
    /// GET with a query (`limit=…&before=…` or `since=…`); returns the body.
    fn fetch_items(&self, query: String) -> BoxFuture<'static, Result<Vec<u8>, SyncError>>;
    fn put_output(&self, part: String, bytes: Vec<u8>) -> BoxFuture<'static, Result<(), SyncError>>;
    /// `None` = the room has no output for this part (404).
    fn get_output(&self, part: String) -> BoxFuture<'static, Result<Option<Vec<u8>>, SyncError>>;
}

#[derive(Serialize)]
struct ItemPutHeader<'a> {
    ord: f64,
    idx: i64,
    entry: &'a str,
}

#[derive(Deserialize)]
struct ItemHeader {
    ord: f64,
    idx: i64,
    entry: String,
    rev: u64,
}

#[derive(Serialize, Deserialize)]
struct LiveHeader {
    entry: String,
    ord: f64,
    idx: i64,
    op: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    at: Option<usize>,
    /// A `part` frame's entry header row (JSON object), for readers that
    /// haven't received the stored one yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    head: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemsDone {
    pub head_rev: u64,
    #[serde(default)]
    pub more: bool,
}

/// One `GET /items` body, decoded.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ItemsResponse {
    pub items: Vec<ReceivedItem>,
    pub live: Vec<Live>,
    /// `None` when the server truncated the body at its cap: apply what
    /// arrived and ask again from the new head.
    pub done: Option<ItemsDone>,
}

/// The ack of one `POST /items`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PostAck {
    pub head_rev: u64,
    pub results: Vec<PostResult>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PostResult {
    #[serde(default)]
    pub ord: Option<f64>,
    #[serde(default)]
    pub idx: Option<i64>,
    #[serde(default)]
    pub rev: Option<u64>,
    /// False for an identical rewrite the room skipped.
    #[serde(default)]
    pub changed: Option<bool>,
    #[serde(default)]
    pub error: Option<String>,
}

/// u32-LE length-prefixed frames (the `/rows` body encoding).
pub fn length_prefixed(frames: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::with_capacity(frames.iter().map(|f| f.len() + 4).sum());
    for frame in frames {
        out.extend_from_slice(&(frame.len() as u32).to_le_bytes());
        out.extend_from_slice(frame);
    }
    out
}

pub fn split_length_prefixed(body: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut off = 0;
    while off + 4 <= body.len() {
        let len = u32::from_le_bytes(body[off..off + 4].try_into().unwrap_or_default()) as usize;
        off += 4;
        if off + len > body.len() {
            break;
        }
        out.push(&body[off..off + len]);
        off += len;
    }
    out
}

pub fn encode_item_puts(items: &[Item]) -> Vec<u8> {
    let frames: Vec<Vec<u8>> = items
        .iter()
        .map(|item| {
            wire::encode(
                frame_type::ITEM_PUT,
                &ItemPutHeader {
                    ord: item.key.ord,
                    idx: item.key.idx,
                    entry: &item.entry,
                },
                &item.json,
            )
        })
        .collect();
    length_prefixed(&frames)
}

pub fn encode_live(live: &Live) -> Vec<u8> {
    let (op, at, payload) = match &live.op {
        LiveOp::Part(json) => ("part", None, json.clone()),
        LiveOp::Append { at, text } => ("append", Some(*at), text.as_bytes().to_vec()),
    };
    wire::encode(
        frame_type::LIVE,
        &LiveHeader {
            entry: live.entry.clone(),
            ord: live.key.ord,
            idx: live.key.idx,
            op: op.into(),
            at,
            head: live.header.as_ref().and_then(|h| serde_json::from_slice(h).ok()),
        },
        &payload,
    )
}

pub fn decode_live(header: &serde_json::Value, payload: &[u8]) -> Option<Live> {
    let header: LiveHeader = serde_json::from_value(header.clone()).ok()?;
    let op = match header.op.as_str() {
        "part" => LiveOp::Part(payload.to_vec()),
        "append" => LiveOp::Append {
            at: header.at?,
            text: String::from_utf8(payload.to_vec()).ok()?,
        },
        _ => return None,
    };
    Some(Live {
        key: ItemKey {
            ord: header.ord,
            idx: header.idx,
        },
        entry: header.entry,
        op,
        header: header.head.and_then(|h| serde_json::to_vec(&h).ok()),
    })
}

pub fn decode_item(header: &serde_json::Value, payload: &[u8]) -> Option<ReceivedItem> {
    let header: ItemHeader = serde_json::from_value(header.clone()).ok()?;
    Some(ReceivedItem {
        key: ItemKey {
            ord: header.ord,
            idx: header.idx,
        },
        entry: header.entry,
        rev: header.rev,
        json: payload.to_vec(),
    })
}

pub fn decode_items_body(body: &[u8]) -> ItemsResponse {
    let mut out = ItemsResponse::default();
    for raw in split_length_prefixed(body) {
        let Some(frame) = wire::decode(raw) else { continue };
        match frame.kind {
            frame_type::ITEM => {
                if let Some(item) = decode_item(&frame.header, &frame.payload) {
                    out.items.push(item);
                }
            }
            frame_type::LIVE => {
                if let Some(live) = decode_live(&frame.header, &frame.payload) {
                    out.live.push(live);
                }
            }
            frame_type::ITEMS_DONE => {
                out.done = serde_json::from_value(frame.header).ok();
            }
            _ => {}
        }
    }
    out
}

/// Query for the newest page before `before` (or the end).
pub fn page_query(before: Option<ItemKey>, limit: usize) -> String {
    match before {
        Some(key) => format!("limit={limit}&before={}", key.cursor()),
        None => format!("limit={limit}"),
    }
}

pub fn feed_query(since: u64) -> String {
    format!("since={since}")
}

/// The ref a gen-3 tool part's `outputRef` carries: `chat3:{chatId}/{partId}`.
pub fn output_ref(chat_id: &str, part_id: &str) -> String {
    format!("chat3:{chat_id}/{part_id}")
}

/// Split a `chat3:` output ref back into `(chatId, partId)`.
pub fn parse_output_ref(reference: &str) -> Option<(&str, &str)> {
    let rest = reference.strip_prefix("chat3:")?;
    let (chat, part) = rest.split_once('/')?;
    (!chat.is_empty() && !part.is_empty()).then_some((chat, part))
}

/// The edge's part id charset (`PART_RE` in chat-transcript.ts).
pub fn valid_part_id(part: &str) -> bool {
    !part.is_empty()
        && part.len() <= 200
        && part.bytes().all(|b| b.is_ascii_alphanumeric() || b"_.:#~-".contains(&b))
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn doc_error(err: zeron_doc::DocError) -> SyncError {
    SyncError::Protocol(format!("transcript apply: {err}"))
}

/// The reader half of a gen-3 chat, shared by the engine and the app client:
/// pages the newest history in, follows the change feed, applies relayed
/// rows and live frames, pages older history on request.
///
/// Callers hold the doc WEAKLY across awaits (a strong ref reads as a live
/// writer to the engine's LRU) and pass the lock their transcript publisher
/// reads under, so a publish never sees a half-applied page.
pub struct ReaderPlane {
    reader: Mutex<Reader>,
    /// A relayed row skipped revs: run the change feed.
    pub catch_up: Notify,
    reading: tokio::sync::Mutex<()>,
    import_lock: Arc<Mutex<()>>,
}

impl ReaderPlane {
    pub fn new(import_lock: Arc<Mutex<()>>) -> Self {
        Self {
            reader: Mutex::new(Reader::default()),
            catch_up: Notify::new(),
            reading: tokio::sync::Mutex::new(()),
            import_lock,
        }
    }

    /// A transcript frame relayed on the chat's socket. Returns true when
    /// the doc changed.
    pub fn apply_frame(&self, doc: &SessionDoc, kind: u8, header: &serde_json::Value, payload: &[u8]) -> bool {
        match kind {
            frame_type::ITEM => {
                let Some(item) = decode_item(header, payload) else {
                    return false;
                };
                let _import = lock(&self.import_lock);
                let head = Reader::cursor(doc).head_rev;
                if item.rev > head + 1 {
                    // Rows between were missed (a relay outran the feed): the
                    // feed fetches them all, this one included.
                    self.catch_up.notify_one();
                    return false;
                }
                lock(&self.reader)
                    .apply_feed(doc, &[item], None, false)
                    .map(|n| n > 0)
                    .unwrap_or(false)
            }
            frame_type::LIVE => {
                let Some(live) = decode_live(header, payload) else {
                    return false;
                };
                let _import = lock(&self.import_lock);
                let applied = lock(&self.reader).apply_live(doc, &live).unwrap_or(false);
                if !applied && matches!(live.op, LiveOp::Append { .. }) {
                    // An append that doesn't line up: a frame was missed (or
                    // this viewer joined mid-part). The room replays the
                    // open part's frames on every read, so a feed catch-up
                    // rebuilds it.
                    self.catch_up.notify_one();
                }
                applied
            }
            _ => false,
        }
    }

    /// The newest page for a transcript that never held one, else the change
    /// feed from its last rev. Safe to call repeatedly.
    pub async fn catch_up(&self, doc: &Weak<SessionDoc>, transport: &dyn TranscriptTransport) -> Result<(), SyncError> {
        let _reading = self.reading.lock().await;
        let cursor = {
            let Some(doc) = doc.upgrade() else { return Ok(()) };
            Reader::cursor(&doc)
        };
        if cursor.head_rev == 0 && cursor.oldest.is_none() {
            let body = transport.fetch_items(page_query(None, PAGE_LIMIT)).await?;
            let response = decode_items_body(&body);
            let Some(doc) = doc.upgrade() else { return Ok(()) };
            let max_rev = response.items.iter().map(|i| i.rev).max().unwrap_or(0);
            let (more, head) = response.done.map_or((true, max_rev), |d| (d.more, d.head_rev));
            let _import = lock(&self.import_lock);
            let mut reader = lock(&self.reader);
            reader.apply_page(&doc, &response.items, more, head, false).map_err(doc_error)?;
            for live in &response.live {
                let _ = reader.apply_live(&doc, live);
            }
            if response.done.is_some() {
                return Ok(());
            }
        }
        self.feed_locked(doc, transport, |_| true).await.map(|_| ())
    }

    /// Follow the change feed to the head, applying only rows `keep` accepts
    /// (a host keeps rows it did not write). Returns the keys applied.
    pub async fn feed(
        &self,
        doc: &Weak<SessionDoc>,
        transport: &dyn TranscriptTransport,
        keep: impl Fn(&ItemKey) -> bool,
    ) -> Result<HashSet<(u64, i64)>, SyncError> {
        let _reading = self.reading.lock().await;
        self.feed_locked(doc, transport, keep).await
    }

    async fn feed_locked(
        &self,
        doc: &Weak<SessionDoc>,
        transport: &dyn TranscriptTransport,
        keep: impl Fn(&ItemKey) -> bool,
    ) -> Result<HashSet<(u64, i64)>, SyncError> {
        let mut applied = HashSet::new();
        loop {
            let since = {
                let Some(doc) = doc.upgrade() else { return Ok(applied) };
                Reader::cursor(&doc).head_rev
            };
            let body = transport.fetch_items(feed_query(since)).await?;
            let response = decode_items_body(&body);
            let Some(doc) = doc.upgrade() else { return Ok(applied) };
            let max_rev = response.items.iter().map(|i| i.rev).max().unwrap_or(since);
            let rows: Vec<_> = response.items.iter().filter(|i| keep(&i.key)).cloned().collect();
            applied.extend(rows.iter().map(|i| i.key.bits()));
            let head = response.done.map(|d| d.head_rev).unwrap_or(max_rev);
            let _import = lock(&self.import_lock);
            let mut reader = lock(&self.reader);
            reader.apply_feed(&doc, &rows, Some(head), true).map_err(doc_error)?;
            for live in &response.live {
                let _ = reader.apply_live(&doc, live);
            }
            if response.done.is_some() || max_rev <= since {
                return Ok(applied);
            }
        }
    }

    /// The page before the oldest row held. Returns whether older rows remain.
    pub async fn load_earlier(&self, doc: &Weak<SessionDoc>, transport: &dyn TranscriptTransport) -> Result<bool, SyncError> {
        let _reading = self.reading.lock().await;
        let cursor = {
            let Some(doc) = doc.upgrade() else { return Ok(false) };
            Reader::cursor(&doc)
        };
        if !cursor.more_before {
            return Ok(false);
        }
        let body = transport.fetch_items(page_query(cursor.oldest, PAGE_LIMIT)).await?;
        let response = decode_items_body(&body);
        let Some(doc) = doc.upgrade() else { return Ok(false) };
        let more = response.done.map_or(true, |d| d.more);
        let _import = lock(&self.import_lock);
        lock(&self.reader)
            .apply_page(&doc, &response.items, more, 0, true)
            .map_err(doc_error)?;
        Ok(Reader::cursor(&doc).more_before)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn item_puts_round_trip_through_the_page_decoder_shape() {
        let item = Item {
            key: ItemKey { ord: 1.5, idx: 3 },
            entry: "e1".into(),
            json: br#"{"kind":"text","text":"hi"}"#.to_vec(),
        };
        let body = encode_item_puts(std::slice::from_ref(&item));
        let frames = split_length_prefixed(&body);
        assert_eq!(frames.len(), 1);
        let frame = wire::decode(frames[0]).unwrap();
        assert_eq!(frame.kind, frame_type::ITEM_PUT);
        assert_eq!(frame.header, serde_json::json!({"ord": 1.5, "idx": 3, "entry": "e1"}));
        assert_eq!(frame.payload, item.json);
    }

    #[test]
    fn live_frames_round_trip() {
        for op in [
            LiveOp::Part(br#"{"kind":"tool"}"#.to_vec()),
            LiveOp::Append {
                at: 7,
                text: "wörld".into(),
            },
        ] {
            let live = Live {
                key: ItemKey { ord: 2.0, idx: 0 },
                entry: "e".into(),
                op,
                header: Some(br#"{"id":"e","role":"assistant"}"#.to_vec()),
            };
            let frame = wire::decode(&encode_live(&live)).unwrap();
            assert_eq!(frame.kind, frame_type::LIVE);
            assert_eq!(decode_live(&frame.header, &frame.payload), Some(live));
        }
    }

    #[test]
    fn an_items_body_decodes_rows_live_and_done() {
        let item = wire::encode(
            frame_type::ITEM,
            &serde_json::json!({"ord": 1.0, "idx": -1, "entry": "e", "rev": 9}),
            b"{}",
        );
        let live = encode_live(&Live {
            key: ItemKey { ord: 1.0, idx: 0 },
            entry: "e".into(),
            op: LiveOp::Append {
                at: 0,
                text: "x".into(),
            },
            header: None,
        });
        let done = wire::encode(frame_type::ITEMS_DONE, &serde_json::json!({"headRev": 9, "more": true}), &[]);
        let body = length_prefixed(&[item, live, done]);
        let response = decode_items_body(&body);
        assert_eq!(response.items.len(), 1);
        assert_eq!(response.items[0].rev, 9);
        assert_eq!(response.live.len(), 1);
        assert_eq!(response.done, Some(ItemsDone { head_rev: 9, more: true }));
        // Truncated at the cap: no done frame.
        let truncated = decode_items_body(&body[..body.len() - 10]);
        assert_eq!(truncated.done, None);
    }

    #[test]
    fn an_append_that_does_not_line_up_asks_for_a_catch_up() {
        let doc = SessionDoc::init_split("chat").unwrap();
        let plane = ReaderPlane::new(Arc::new(Mutex::new(())));
        let frame = wire::decode(&encode_live(&Live {
            key: ItemKey { ord: 1.0, idx: 0 },
            entry: "e1".into(),
            op: LiveOp::Append { at: 10, text: "late".into() },
            header: None,
        }))
        .unwrap();
        assert!(!plane.apply_frame(&doc, frame.kind, &frame.header, &frame.payload));
        let runtime = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
        runtime.block_on(async {
            tokio::time::timeout(std::time::Duration::from_millis(100), plane.catch_up.notified())
                .await
                .expect("a missed append requests the feed");
        });
    }

    #[test]
    fn output_refs_name_their_room() {
        let reference = output_ref("chat-1", "toolu_01");
        assert_eq!(parse_output_ref(&reference), Some(("chat-1", "toolu_01")));
        assert_eq!(parse_output_ref("chat-1/toolu_01"), None);
        assert!(valid_part_id("toolu_01:abc"));
        assert!(!valid_part_id("a/b"));
    }

    #[test]
    fn page_cursors_use_the_rooms_key_syntax() {
        assert_eq!(page_query(Some(ItemKey { ord: 3.0, idx: -1 }), 50), "limit=50&before=3:-1");
        assert_eq!(page_query(Some(ItemKey { ord: 1.5, idx: 4 }), 50), "limit=50&before=1.5:4");
        assert_eq!(page_query(None, 200), "limit=200");
    }
}
