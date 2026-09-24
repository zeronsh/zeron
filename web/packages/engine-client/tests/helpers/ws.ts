import { WebSocket as NodeWebSocket } from "ws";
import type { EngineClient, EngineStatus } from "../../src/client";
import type { SocketClose, WebSocketFactory, WsSocket } from "../../src/socket";

export interface TrackedFactory {
  factory: WebSocketFactory;
  /** Every client-side socket the factory created, oldest first. */
  sockets: NodeWebSocket[];
  /** Timestamps of each dial, aligned with `sockets`. */
  dialedAt: number[];
}

/** A `ws`-backed factory that keeps the raw sockets for `terminate()` (hard-drop simulation). */
export function trackedFactory(): TrackedFactory {
  const sockets: NodeWebSocket[] = [];
  const dialedAt: number[] = [];
  const factory: WebSocketFactory = (url) => {
    dialedAt.push(Date.now());
    const socket = new NodeWebSocket(url);
    sockets.push(socket);
    return new WsAdapter(socket);
  };
  return { factory, sockets, dialedAt };
}

class WsAdapter implements WsSocket {
  readonly #socket: NodeWebSocket;

  constructor(socket: NodeWebSocket) {
    this.#socket = socket;
  }

  send(data: string): void {
    this.#socket.send(data);
  }

  close(code?: number, reason?: string): void {
    this.#socket.close(code, reason);
  }

  addEventListener(type: "open", listener: () => void): void;
  addEventListener(type: "message", listener: (event: { data: unknown }) => void): void;
  addEventListener(type: "close", listener: (event: SocketClose) => void): void;
  addEventListener(type: "error", listener: () => void): void;
  addEventListener(
    type: "open" | "message" | "close" | "error",
    listener: (() => void) | ((event: { data: unknown }) => void) | ((event: SocketClose) => void),
  ): void {
    switch (type) {
      case "open":
        this.#socket.on("open", () => (listener as () => void)());
        break;
      case "message":
        this.#socket.on("message", (data: unknown) => {
          const text =
            typeof data === "string" ? data : Buffer.from(data as Uint8Array).toString("utf8");
          (listener as (event: { data: unknown }) => void)({ data: text });
        });
        break;
      case "close":
        this.#socket.on("close", (code: number, reason: Buffer) => {
          (listener as (event: SocketClose) => void)({
            code,
            reason: reason.toString("utf8"),
            wasClean: code === 1000,
          });
        });
        break;
      case "error":
        this.#socket.on("error", () => (listener as () => void)());
        break;
    }
  }
}

export function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

export async function waitUntil(
  predicate: () => boolean,
  timeoutMs = 5_000,
  message = "condition",
): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (!predicate()) {
    if (Date.now() > deadline) {
      throw new Error(`timed out waiting for ${message}`);
    }
    await delay(10);
  }
}

export async function statusWhen(
  client: EngineClient,
  predicate: (status: EngineStatus) => boolean,
  timeoutMs = 5_000,
): Promise<EngineStatus> {
  const current = client.status;
  if (current !== null && predicate(current)) {
    return current;
  }
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      unsubscribe();
      reject(new Error(`timed out waiting for status: ${JSON.stringify(client.status)}`));
    }, timeoutMs);
    const unsubscribe = client.onStatus((status) => {
      if (predicate(status)) {
        clearTimeout(timer);
        unsubscribe();
        resolve(status);
      }
    });
  });
}
