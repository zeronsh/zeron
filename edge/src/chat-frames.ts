/**
 * chat2 wire frames (docs/chat2-sync.md workstream B).
 *
 * Binary WS frames: `[type u8][headerLen u32 LE][header JSON utf8][payload]`.
 * Headers are tiny JSON (ids, seqs); payloads are opaque byte blobs (Loro
 * updates, checkpoint frontiers, presence ephemera) that only CLIENTS parse —
 * the DO relays them untouched. Binary because base64'ing update bytes costs
 * 33% on the wire, which matters at the 1.2 Mbps links the whale incident
 * surfaced; no loro-protocol because the server owns no CRDT semantics.
 *
 * Shared shape across Rust (crates/sync) and Swift clients — change it only
 * with cross-language test vectors (registry precedent).
 */

import { textDecoder, textEncoder } from "./blobs";

/** Frame type bytes. Client→server and server→client share one space. */
export const FRAME = {
  // client → server
  /** `{cursor, device}` — first frame on a socket; answered by `state`. */
  hello: 0x01,
  /** `{after, excludeOwn}` — request backfill rows with `seq > after`;
   * `excludeOwn` skips the sender device's own writes (reconnect path). */
  rowsReq: 0x03,
  /** `{batchId}` + payload = one opaque Loro update. */
  push: 0x06,
  /** `{at}` + opaque payload — relayed verbatim to live peers, never stored. */
  presence: 0x08,
  /** `{}` — liveness probe; answered by `probeOk` from the DO itself (unlike
   * the runtime-answered ping/pong pair, this proves the DO runs). */
  probe: 0x09,

  // server → client
  /** `{headSeq, seqFloor, checkpointSeq, checkpointSize, rowCount, rowBytes}`
   * + payload = checkpoint frontier bytes (empty when no checkpoint). The
   * client compares the frontier against its local doc: included → rows-only
   * catch-up; not included → `GET /checkpoint` first. */
  state: 0x02,
  /** `{seq, device, batchId}` + payload = update bytes. Sent during backfill
   * (after `rowsReq`) and as the live relay of other devices' pushes. */
  row: 0x04,
  /** `{headSeq}` — backfill complete; subsequent `row` frames are live. */
  rowsDone: 0x05,
  /** `{batchId, seq, dup}` — push accepted (`dup` = batchId replay, no-op). */
  ack: 0x07,
  /** `{headSeq}` — probe answer. */
  probeOk: 0x0a,
  /** `{code, message}` — recoverable rejection (socket stays open). */
  error: 0x0b,

  // relay layer (docs/design/relay-layer.md): only with CHAT_RELAY_LAYER on,
  // and only to sockets whose `hello` said `relay: 1`.
  /** `{}` + update, to live peers; `{device, queued?, batchId?}` from them.
   * Never stored. `queued` marks a row held in the offline queue. */
  relay: 0x0c,
  /** `{hello?}` + version vector. `hello` asks peers to answer if it's behind. */
  announce: 0x0d,
  /** `{to, xfer?, offset?}` + version vector. Forwarded to `to` only. */
  syncReq: 0x0e,
  /** `{to, xfer, offset, total}` + a fragment of one update blob. Forwarded to `to` only. */
  syncData: 0x0f,
  /** `{batchId}` + update, held in the offline queue until the host has it. */
  enqueue: 0x10,
  /** `{batchId, dup}` — the queued row is durable. */
  queued: 0x11,
  /** `{}` — read the offline queue, answered by `relay` frames and `queueDone`. */
  queueReq: 0x12,
  /** `{count}` — end of the queue backlog. */
  queueDone: 0x13,
  /** `{batchIds}` — the host has saved these. Delete them. */
  delivered: 0x14,
  /** `{device}` — a reader here can't take relay frames: the host must store. */
  legacyPeer: 0x15
} as const;

/** Frames that exist only with the relay layer on. With it off they're answered
 * exactly as an unknown type byte is. */
export const RELAY_FRAME_TYPES: ReadonlySet<number> = new Set([
  FRAME.relay,
  FRAME.announce,
  FRAME.syncReq,
  FRAME.syncData,
  FRAME.enqueue,
  FRAME.queued,
  FRAME.queueReq,
  FRAME.queueDone,
  FRAME.delivered,
  FRAME.legacyPeer
]);

export type FrameType = (typeof FRAME)[keyof typeof FRAME];

export interface Frame {
  type: FrameType;
  header: Record<string, unknown>;
  payload: Uint8Array;
}

/** Headers are ids + a few integers; anything bigger is a client bug. */
export const MAX_HEADER_BYTES = 4096;

const KNOWN_TYPES = new Set<number>(Object.values(FRAME));

export const encodeFrame = (
  type: FrameType,
  header: Record<string, unknown>,
  payload?: Uint8Array
): Uint8Array => {
  const headerBytes = textEncoder.encode(JSON.stringify(header));
  const payloadLen = payload?.length ?? 0;
  const out = new Uint8Array(5 + headerBytes.length + payloadLen);
  out[0] = type;
  new DataView(out.buffer).setUint32(1, headerBytes.length, true);
  out.set(headerBytes, 5);
  if (payload) out.set(payload, 5 + headerBytes.length);
  return out;
};

/** `undefined` = malformed (unknown type, bad length, junk JSON). The DO
 * answers malformed frames with an `error` frame, not a close — one corrupt
 * frame from a flaky link must not cost a reconnect cycle. */
export const decodeFrame = (bytes: Uint8Array): Frame | undefined => {
  if (bytes.length < 5) return undefined;
  const type = bytes[0]!;
  if (!KNOWN_TYPES.has(type)) return undefined;
  const headerLen = new DataView(bytes.buffer, bytes.byteOffset).getUint32(1, true);
  if (headerLen > MAX_HEADER_BYTES || 5 + headerLen > bytes.length) return undefined;
  let header: unknown;
  try {
    header = JSON.parse(textDecoder.decode(bytes.subarray(5, 5 + headerLen)));
  } catch {
    return undefined;
  }
  if (typeof header !== "object" || header === null || Array.isArray(header)) return undefined;
  return {
    type: type as FrameType,
    header: header as Record<string, unknown>,
    payload: bytes.subarray(5 + headerLen)
  };
};
