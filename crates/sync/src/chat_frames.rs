//! chat2 wire frames — Rust twin of `edge/src/chat-frames.ts` (the DO's
//! codec). Binary WS frames: `[type u8][headerLen u32 LE][header JSON][payload]`.
//! Headers are tiny JSON; payloads are opaque bytes (Loro updates, checkpoint
//! frontiers, presence ephemera). Cross-language contract — the layout tests
//! here pin the same vectors as the TS suite; change both together.

use serde::{Deserialize, Serialize};

/// Frame type bytes (shared client/server space; mirror `FRAME` in TS).
pub mod frame_type {
    pub const HELLO: u8 = 0x01;
    pub const STATE: u8 = 0x02;
    pub const ROWS_REQ: u8 = 0x03;
    pub const ROW: u8 = 0x04;
    pub const ROWS_DONE: u8 = 0x05;
    pub const PUSH: u8 = 0x06;
    pub const ACK: u8 = 0x07;
    pub const PRESENCE: u8 = 0x08;
    pub const PROBE: u8 = 0x09;
    pub const PROBE_OK: u8 = 0x0a;
    pub const ERROR: u8 = 0x0b;
    // chat3 transcript plane (docs/chat3-sync.md).
    pub const ITEM_PUT: u8 = 0x10;
    pub const LIVE: u8 = 0x11;
    pub const ITEM: u8 = 0x12;
    pub const ITEMS_DONE: u8 = 0x13;
}

/// Headers are ids + a few integers; anything bigger is a peer bug.
pub const MAX_HEADER_BYTES: usize = 4096;

#[derive(Debug, Clone, PartialEq)]
pub struct WireFrame {
    pub kind: u8,
    pub header: serde_json::Value,
    pub payload: Vec<u8>,
}

pub fn encode(kind: u8, header: &impl Serialize, payload: &[u8]) -> Vec<u8> {
    let header = serde_json::to_vec(header).expect("frame header serializes");
    let mut out = Vec::with_capacity(5 + header.len() + payload.len());
    out.push(kind);
    out.extend_from_slice(&(header.len() as u32).to_le_bytes());
    out.extend_from_slice(&header);
    out.extend_from_slice(payload);
    out
}

/// `None` = malformed. Unknown type bytes are NOT rejected here (unlike the
/// DO): the client tolerates future server frame types by skipping them.
pub fn decode(bytes: &[u8]) -> Option<WireFrame> {
    if bytes.len() < 5 {
        return None;
    }
    let kind = bytes[0];
    let header_len = u32::from_le_bytes(bytes[1..5].try_into().ok()?) as usize;
    if header_len > MAX_HEADER_BYTES || 5 + header_len > bytes.len() {
        return None;
    }
    let header: serde_json::Value = serde_json::from_slice(&bytes[5..5 + header_len]).ok()?;
    if !header.is_object() {
        return None;
    }
    Some(WireFrame {
        kind,
        header,
        payload: bytes[5 + header_len..].to_vec(),
    })
}

// ── typed headers (parse via `serde_json::from_value(frame.header)`) ────────

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HelloHeader<'a> {
    pub cursor: u64,
    pub device: &'a str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RowsReqHeader {
    pub after: u64,
    pub exclude_own: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PushHeader<'a> {
    pub batch_id: &'a str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PresenceOutHeader {
    pub at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StateHeader {
    pub head_seq: u64,
    pub seq_floor: u64,
    pub checkpoint_seq: u64,
    pub checkpoint_size: u64,
    #[serde(default)]
    pub row_count: u64,
    #[serde(default)]
    pub row_bytes: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RowHeader {
    pub seq: u64,
    pub device: String,
    pub batch_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RowsDoneHeader {
    pub head_seq: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AckHeader {
    pub batch_id: String,
    pub seq: u64,
    #[serde(default)]
    pub dup: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PresenceInHeader {
    pub device: String,
    pub at: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeOkHeader {
    pub head_seq: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ErrorHeader {
    pub code: String,
    #[serde(default)]
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pins_the_wire_layout() {
        // Must match the TS vector: [type][headerLen u32 LE][header][payload].
        let frame = encode(
            frame_type::PUSH,
            &serde_json::json!({"batchId": "b1"}),
            &[9, 8, 7],
        );
        assert_eq!(frame[0], frame_type::PUSH);
        let header = br#"{"batchId":"b1"}"#;
        assert_eq!(&frame[1..5], &(header.len() as u32).to_le_bytes());
        assert_eq!(&frame[5..5 + header.len()], header);
        assert_eq!(&frame[5 + header.len()..], &[9, 8, 7]);
    }

    #[test]
    fn round_trips_and_rejects_malformed() {
        let payload = vec![1u8; 1000];
        let frame = encode(frame_type::ROW, &serde_json::json!({"seq": 7}), &payload);
        let decoded = decode(&frame).unwrap();
        assert_eq!(decoded.kind, frame_type::ROW);
        assert_eq!(decoded.header["seq"], 7);
        assert_eq!(decoded.payload, payload);

        assert!(decode(&[]).is_none());
        assert!(decode(&[frame_type::HELLO]).is_none());
        // Header length past the buffer.
        let mut truncated = encode(frame_type::HELLO, &serde_json::json!({}), &[]);
        truncated[1..5].copy_from_slice(&9999u32.to_le_bytes());
        assert!(decode(&truncated).is_none());
        // Non-object header.
        let arr = encode(frame_type::HELLO, &serde_json::json!([1]), &[]);
        assert!(decode(&arr).is_none());
        // Oversized header.
        let fat = encode(
            frame_type::HELLO,
            &serde_json::json!({"pad": "x".repeat(MAX_HEADER_BYTES)}),
            &[],
        );
        assert!(decode(&fat).is_none());
    }

    #[test]
    fn typed_headers_parse_server_shapes() {
        let state: StateHeader = serde_json::from_value(serde_json::json!({
            "headSeq": 10, "seqFloor": 3, "checkpointSeq": 3,
            "checkpointSize": 160_000, "rowCount": 7, "rowBytes": 14_000
        }))
        .unwrap();
        assert_eq!(state.head_seq, 10);
        assert_eq!(state.checkpoint_seq, 3);
        let ack: AckHeader =
            serde_json::from_value(serde_json::json!({"batchId": "b", "seq": 4})).unwrap();
        assert!(!ack.dup);
    }
}
