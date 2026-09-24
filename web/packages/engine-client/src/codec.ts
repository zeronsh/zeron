/**
 * The ndjson frame codec for the engine control plane.
 *
 * One JSON object per line; a transport may batch several frames into one
 * text message (the engine's listener also reads the first frame's first
 * line only — see `encodeAuthEnvelope`). Malformed lines are dropped, never
 * fatal: the Rust reader does exactly the same (crates/rpc/src/client.rs).
 */

export interface CallFrame {
  readonly id: number;
  readonly method: string;
  readonly params?: unknown;
}

export interface CancelFrame {
  readonly id: number;
  readonly cancel: true;
}

export type ClientFrame = CallFrame | CancelFrame;

/** A server-originated frame; exactly one of `ok` / `err` / `item` / `done` is meaningful. */
export interface ServerFrame {
  readonly id: number;
  readonly ok?: unknown;
  readonly err?: string;
  readonly item?: unknown;
  readonly done?: boolean;
}

export interface DecodedMessage {
  readonly frames: ServerFrame[];
  /** Lines that were not a valid server frame; dropped, not fatal. */
  readonly malformed: number;
}

/**
 * The first-frame `Auth` envelope. It must be the entire first message: the
 * engine's listener authenticates the first non-empty line of the first text
 * frame and discards the rest of that message — never batch anything with it.
 */
export function encodeAuthEnvelope(credential: string): string {
  return JSON.stringify({ auth: credential });
}

/**
 * Serialize a client frame. Key order mirrors the Rust serde struct order
 * (`id`, `method`, `params`, `cancel`, with null/absent fields skipped) so
 * frames are byte-identical to what the native client sends.
 */
export function encodeClientFrame(frame: ClientFrame): string {
  if ("cancel" in frame) {
    return JSON.stringify({ id: frame.id, cancel: true });
  }
  const encoded: Record<string, unknown> = { id: frame.id, method: frame.method };
  if (frame.params !== undefined && frame.params !== null) {
    encoded.params = frame.params;
  }
  return JSON.stringify(encoded);
}

/**
 * Split one text message into server frames. Blank lines are skipped,
 * unparseable or wrongly-shaped lines are counted as malformed and dropped.
 */
export function decodeServerMessage(text: string): DecodedMessage {
  const frames: ServerFrame[] = [];
  let malformed = 0;
  for (const line of text.split("\n")) {
    const trimmed = line.trim();
    if (trimmed.length === 0) {
      continue;
    }
    let value: unknown;
    try {
      value = JSON.parse(trimmed);
    } catch {
      malformed += 1;
      continue;
    }
    const frame = asServerFrame(value);
    if (frame === null) {
      malformed += 1;
      continue;
    }
    frames.push(frame);
  }
  return { frames, malformed };
}

function asServerFrame(value: unknown): ServerFrame | null {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    return null;
  }
  const record = value as Record<string, unknown>;
  const { id, ok, err, item, done } = record;
  if (typeof id !== "number" || !Number.isInteger(id) || id < 0) {
    return null;
  }
  if (err !== undefined && typeof err !== "string") {
    return null;
  }
  if (done !== undefined && typeof done !== "boolean") {
    return null;
  }
  return { id, ok, err, item, done };
}

/**
 * The stream readiness ack: `{id, ok: {stream: true}}`. `WatchCheckoutChange
 * Request` sends it before its first item — it is an ack, not data. Any other
 * `ok` on a stream id does not occur on this wire today.
 */
export function isStreamAck(frame: ServerFrame): boolean {
  if (typeof frame.ok !== "object" || frame.ok === null || Array.isArray(frame.ok)) {
    return false;
  }
  return (frame.ok as Record<string, unknown>).stream === true;
}
