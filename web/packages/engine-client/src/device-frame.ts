/**
 * DeviceRoom wire framing for the browser client — a TS port of the Rust
 * codec (crates/rpc/src/device_frame.rs, shared by native relays and PR
 * #319's browser client).
 *
 * A frame is `uleb128(header_len) || UTF-8 JSON header || payload`. The
 * Worker routes on `to`/`from`; a browser client sends only `{s,k}`.
 */

import type { SocketClose, WsSocket } from "./socket";

export const RELAY_KIND = " relay";
export const RPC_KIND = "rpc";
/** End-to-end proof that the relay can reach the remote engine host. */
export const ECHO_KIND = "echo";
/** Durable Object hibernation-safe text keepalive request and response. */
export const PING_TEXT = "ping";
export const PONG_TEXT = "pong";
/** The browser transport sends both keepalive forms at this cadence. */
export const PING_INTERVAL_MS = 10_000;
/** Client-to-edge half-open transport deadline. */
export const SILENCE_LEASE_MS = 25_000;
/** Edge pongs alone do not prove the edge-to-host leg; require host echo. */
export const ECHO_DEADLINE_MS = 20_000;
/** Inbound frame ceiling (PR #319's connection.rs). */
export const MAX_FRAME = 8 * 1024 * 1024;
/** Cloudflare's per-WebSocket-message ceiling for encoded relay frames. */
export const MAX_OUTBOUND_FRAME = 1024 * 1024;
/** WebSocket high-water mark; a single relay frame may exceed it. */
export const MAX_BUFFERED = 256 * 1024;

export interface DeviceFrameHeader {
  readonly s: string;
  readonly k: string;
  readonly to?: string;
  readonly from?: string;
}

function headerBytes(header: DeviceFrameHeader): Uint8Array {
  return new TextEncoder().encode(JSON.stringify(header));
}

/** Encode one relay frame. Throws only on a header that cannot serialize. */
export function encodeDeviceFrame(header: DeviceFrameHeader, payload: Uint8Array): Uint8Array {
  const json = headerBytes(header);
  const output = new Uint8Array(5 + json.length + payload.length);
  let length = json.length;
  let offset = 0;
  for (;;) {
    let byte = length & 0x7f;
    length >>>= 7;
    if (length !== 0) {
      byte |= 0x80;
    }
    output[offset++] = byte;
    if (length === 0) {
      break;
    }
  }
  output.set(json, offset);
  output.set(payload, offset + json.length);
  return output.slice(0, offset + json.length + payload.length);
}

/** Decode one relay frame; throws on truncated or malformed input. */
export function decodeDeviceFrame(bytes: Uint8Array): { header: DeviceFrameHeader; payload: Uint8Array } {
  let offset = 0;
  let length = 0;
  let shift = 0;
  for (;;) {
    if (offset >= bytes.length) {
      throw new Error("device frame: truncated uleb128");
    }
    const byte = bytes[offset++]!;
    if (shift >= 32 || (shift === 28 && (byte & 0x70) !== 0)) {
      throw new Error("device frame: uleb128 overflow");
    }
    length |= (byte & 0x7f) * 2 ** shift;
    if ((byte & 0x80) === 0) {
      break;
    }
    shift += 7;
  }
  const end = offset + length;
  if (end > bytes.length) {
    throw new Error("device frame: truncated header");
  }
  const headerText = new TextDecoder().decode(bytes.subarray(offset, end));
  const header = JSON.parse(headerText) as DeviceFrameHeader;
  if (typeof header?.s !== "string" || typeof header?.k !== "string") {
    throw new Error("device frame: bad header JSON");
  }
  return { header, payload: bytes.slice(end) };
}

/**
 * The browser's DeviceRoom socket: a same-origin, cookie-authenticated
 * WebSocket (`/api/browser/device/{id}/ws`) carrying RPC text inside
 * DeviceFrame binary envelopes, with ping/pong/echo liveness and
 * buffered-amount backpressure — the TS twin of PR #319's
 * `apps/web/src/rpc/connection.rs` browser transport.
 *
 * RPC text is the same protocol the direct engine listener speaks minus the
 * first-frame `Auth` envelope: the edge validated the browser session at
 * the upgrade and injected the owner identity into the DeviceRoom.
 */
export class RelaySocket implements WsSocket {
  readonly #socket: WebSocket;
  readonly #listeners = {
    open: new Set<() => void>(),
    message: new Set<(event: { data: unknown }) => void>(),
    close: new Set<(event: SocketClose) => void>(),
    error: new Set<() => void>(),
  };
  #closed = false;
  #lastRx = Date.now();
  #lastEcho = Date.now();
  readonly #heartbeat: ReturnType<typeof setInterval>;
  #drainTimer: ReturnType<typeof setInterval> | undefined;

  constructor(url: string) {
    this.#socket = new WebSocket(url);
    this.#socket.binaryType = "arraybuffer";
    this.#socket.addEventListener("open", () => {
      this.#touch();
      this.#lastEcho = Date.now();
      for (const listener of this.#listeners.open) {
        listener();
      }
    });
    this.#socket.addEventListener("message", (event: MessageEvent) => {
      if (!this.#accept(event.data)) {
        this.#protocolFailure();
      }
    });
    this.#socket.addEventListener("close", (event: CloseEvent) => {
      this.#finish();
      const close: SocketClose = {
        code: event.code,
        reason: event.reason,
        wasClean: event.wasClean,
      };
      for (const listener of this.#listeners.close) {
        listener(close);
      }
    });
    this.#socket.addEventListener("error", () => {
      for (const listener of this.#listeners.error) {
        listener();
      }
    });
    this.#heartbeat = setInterval(() => this.#tick(), PING_INTERVAL_MS);
  }

  send(data: string): void {
    const frame = encodeDeviceFrame({ s: RPC_KIND, k: RPC_KIND }, new TextEncoder().encode(data));
    if (frame.length > MAX_OUTBOUND_FRAME) {
      this.#protocolFailure();
      return;
    }
    // Backpressure mirrors the Rust client: a frame may exceed the
    // high-water mark itself; only the already-buffered amount controls
    // draining. Wait asynchronously — `send` stays synchronous per the
    // client's socket contract, frames drain in order.
    if (this.#socket.bufferedAmount > MAX_BUFFERED) {
      this.#queueFrame(frame);
      return;
    }
    this.#socket.send(frame);
  }

  close(code?: number, reason?: string): void {
    this.#finish();
    if (code === undefined) {
      this.#socket.close();
    } else {
      this.#socket.close(code, reason);
    }
  }

  addEventListener(type: "open", listener: () => void): void;
  addEventListener(type: "message", listener: (event: { data: unknown }) => void): void;
  addEventListener(type: "close", listener: (event: SocketClose) => void): void;
  addEventListener(type: "error", listener: () => void): void;
  addEventListener(type: "open" | "message" | "close" | "error", listener: never | ((event: never) => void)): void {
    this.#listeners[type as "open"].add(listener as () => void);
  }

  /** Frame acceptance: pong text, echo frames, and RPC frames only. */
  #accept(data: unknown): boolean {
    if (typeof data === "string") {
      if (data === PONG_TEXT) {
        this.#touch();
        return true;
      }
      return false;
    }
    if (!(data instanceof ArrayBuffer) || data.byteLength > MAX_FRAME) {
      return false;
    }
    let decoded: { header: DeviceFrameHeader; payload: Uint8Array };
    try {
      decoded = decodeDeviceFrame(new Uint8Array(data));
    } catch {
      return false;
    }
    if (decoded.header.k === ECHO_KIND) {
      this.#echoed();
      return true;
    }
    if (decoded.header.k === RPC_KIND) {
      this.#echoed();
      const text = new TextDecoder().decode(decoded.payload);
      for (const listener of this.#listeners.message) {
        listener({ data: text });
      }
      return true;
    }
    return false;
  }

  #tick(): void {
    if (this.#closed || this.#socket.readyState !== WebSocket.OPEN) {
      return;
    }
    const now = Date.now();
    if (now - this.#lastRx > SILENCE_LEASE_MS || now - this.#lastEcho > ECHO_DEADLINE_MS) {
      this.close(1000, "relay liveness expired");
      return;
    }
    this.#socket.send(PING_TEXT);
    this.#socket.send(encodeDeviceFrame({ s: ECHO_KIND, k: ECHO_KIND }, new Uint8Array(0)));
  }

  #queueFrame(frame: Uint8Array): void {
    if (this.#drainTimer !== undefined) {
      return;
    }
    const queue: Uint8Array[] = [frame];
    this.#drainTimer = setInterval(() => {
      if (this.#closed) {
        clearInterval(this.#drainTimer);
        this.#drainTimer = undefined;
        return;
      }
      while (this.#socket.bufferedAmount <= MAX_BUFFERED && queue.length > 0) {
        const next = queue.shift()!;
        if (next.length > MAX_OUTBOUND_FRAME) {
          this.#protocolFailure();
          return;
        }
        this.#socket.send(next);
      }
      if (queue.length === 0) {
        clearInterval(this.#drainTimer!);
        this.#drainTimer = undefined;
      }
    }, 16);
  }

  #protocolFailure(): void {
    this.close(1000, "relay protocol failure");
  }

  #touch(): void {
    this.#lastRx = Date.now();
  }

  #echoed(): void {
    this.#touch();
    this.#lastEcho = Date.now();
  }

  #finish(): void {
    if (this.#closed) {
      return;
    }
    this.#closed = true;
    clearInterval(this.#heartbeat);
    if (this.#drainTimer !== undefined) {
      clearInterval(this.#drainTimer);
      this.#drainTimer = undefined;
    }
  }
}
