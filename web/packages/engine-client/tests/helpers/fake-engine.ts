import { createServer, type Server as HttpServer } from "node:http";
import { WebSocket, WebSocketServer } from "ws";
import type { ServerFrame } from "../../src/codec";

export interface ClientCallFrame {
  id: number;
  method: string;
  params?: unknown;
  cancel?: boolean;
}

export interface Reply {
  /** The request's frame id, so scripts can forge raw/batched frames. */
  readonly id: number;
  ok(value: unknown): void;
  error(message: string): void;
  /** The stream readiness ack (`{ok: {stream: true}}`). */
  ack(): void;
  item(value: unknown): void;
  done(): void;
  /** Send several newline-joined frames (or arbitrary text) as ONE message. */
  raw(text: string): void;
}

export interface FakeConnection {
  readonly socket: WebSocket;
  /** Every parsed client frame, in arrival order. */
  readonly frames: ClientCallFrame[];
  /** Frame ids of `{id, cancel: true}` frames received. */
  readonly cancels: number[];
  readonly streams: Map<string, number>;
  authed: boolean;
  pushItem(method: string, value: unknown): void;
  finishStream(method: string): void;
}

export interface FakeEngineOptions {
  deviceId?: string;
  /** Return false to close the connection with 4401 "invalid credential". */
  auth?: (credential: string) => boolean;
}

type CallHandler = (params: unknown, reply: Reply) => void;
type StreamHandler = (reply: Reply) => void;

/**
 * A scripted stand-in engine: same wire contract as the real listener
 * (first-frame Auth, 4401 refusal, ndjson frames), with per-test handlers
 * for calls and streams, upgrade refusal counting, and recorders for every
 * frame it sees.
 */
export class FakeEngine {
  readonly calls: Record<string, CallHandler> = {};
  readonly streams: Record<string, StreamHandler> = {};
  /** Answer for EngineInfo; change per test to script identity mismatches. */
  engineInfo: () => Record<string, unknown> = () => ({
    deviceId: this.deviceId,
    workspaceScope: "local",
    capabilities: ["message-queue-v1", "web-client"],
  });
  auth: (credential: string) => boolean = () => true;
  /** How many upcoming upgrades to refuse with HTTP 401 before accepting. */
  refuseUpgrades = 0;

  readonly connections: FakeConnection[] = [];
  readonly authFrames: string[] = [];
  engineInfoCalls = 0;

  readonly #deviceId: string;
  #http: HttpServer | null = null;
  #wss: WebSocketServer | null = null;
  #port = 0;

  constructor(options: FakeEngineOptions = {}) {
    this.#deviceId = options.deviceId ?? "fake-device-1";
    if (options.auth !== undefined) {
      this.auth = options.auth;
    }
    this.calls["Echo"] = (params, reply) => {
      reply.ok(params);
    };
  }

  get deviceId(): string {
    return this.#deviceId;
  }

  get endpoint(): string {
    return `ws://127.0.0.1:${this.#port}`;
  }

  listen(): Promise<void> {
    return new Promise((resolve, reject) => {
      const http = createServer((request, response) => {
        response.destroy();
      });
      const wss = new WebSocketServer({ noServer: true });
      http.on("upgrade", (request, socket, head) => {
        if (this.refuseUpgrades > 0) {
          this.refuseUpgrades -= 1;
          socket.write("HTTP/1.1 401 Unauthorized\r\nConnection: close\r\nContent-Length: 0\r\n\r\n");
          socket.destroy();
          return;
        }
        wss.handleUpgrade(request, socket, head, (ws) => this.#attach(ws));
      });
      http.on("error", reject);
      http.listen(0, "127.0.0.1", () => {
        this.#http = http;
        this.#wss = wss;
        this.#port = http.address() !== null && typeof http.address() === "object" ? (http.address() as { port: number }).port : 0;
        resolve();
      });
    });
  }

  close(): Promise<void> {
    const wss = this.#wss;
    const http = this.#http;
    this.#wss = null;
    this.#http = null;
    if (wss === null || http === null) {
      return Promise.resolve();
    }
    for (const client of wss.clients) {
      client.terminate();
    }
    return new Promise((resolve) => {
      wss.close(() => {
        http.close(() => resolve());
      });
    });
  }

  #attach(socket: WebSocket): void {
    const connection: FakeConnection = {
      socket,
      frames: [],
      cancels: [],
      streams: new Map(),
      authed: false,
      pushItem: (method: string, value: unknown) => {
        const id = connection.streams.get(method);
        if (id !== undefined) {
          socket.send(JSON.stringify({ id, item: value }));
        }
      },
      finishStream: (method: string) => {
        const id = connection.streams.get(method);
        if (id !== undefined) {
          connection.streams.delete(method);
          socket.send(JSON.stringify({ id, done: true }));
        }
      },
    };
    this.connections.push(connection);
    let authed = false;
    socket.on("message", (data: unknown, isBinary: boolean) => {
      if (isBinary) {
        return;
      }
      const text = typeof data === "string" ? data : Buffer.from(data as Uint8Array).toString("utf8");
      if (!authed) {
        this.authFrames.push(text);
        let credential: unknown;
        try {
          const parsed: unknown = JSON.parse(text.split("\n").map((line) => line.trim()).find((line) => line.length > 0) ?? "");
          credential =
            typeof parsed === "object" && parsed !== null
              ? (parsed as Record<string, unknown>).auth
              : undefined;
        } catch {
          credential = undefined;
        }
        if (typeof credential !== "string" || !this.auth(credential)) {
          socket.close(4401, "invalid credential");
          return;
        }
        authed = true;
        connection.authed = true;
        return;
      }
      for (const line of text.split("\n")) {
        const trimmed = line.trim();
        if (trimmed.length === 0) {
          continue;
        }
        let frame: ClientCallFrame;
        try {
          frame = JSON.parse(trimmed) as ClientCallFrame;
        } catch {
          continue;
        }
        connection.frames.push(frame);
        if (frame.cancel === true) {
          connection.cancels.push(frame.id);
          for (const [method, id] of connection.streams) {
            if (id === frame.id) {
              connection.streams.delete(method);
            }
          }
          continue;
        }
        if (typeof frame.method !== "string") {
          continue;
        }
        this.#dispatch(connection, frame);
      }
    });
  }

  #dispatch(connection: FakeConnection, frame: ClientCallFrame): void {
    const { socket } = connection;
    const sendFrame = (frameValue: ServerFrame): void => {
      socket.send(JSON.stringify(frameValue));
    };
    const reply: Reply = {
      id: frame.id,
      ok: (value: unknown) => sendFrame({ id: frame.id, ok: value }),
      error: (message: string) => sendFrame({ id: frame.id, err: message }),
      ack: () => sendFrame({ id: frame.id, ok: { stream: true } }),
      item: (value: unknown) => sendFrame({ id: frame.id, item: value }),
      done: () => sendFrame({ id: frame.id, done: true }),
      raw: (text: string) => socket.send(text),
    };
    const method = frame.method;
    if (method === "EngineInfo") {
      this.engineInfoCalls += 1;
      reply.ok(this.engineInfo());
      return;
    }
    const stream = this.streams[method];
    if (stream !== undefined) {
      connection.streams.set(method, frame.id);
      stream(reply);
      return;
    }
    const call = this.calls[method];
    if (call !== undefined) {
      call(frame.params, reply);
      return;
    }
    reply.error(`unknown method: ${method}`);
  }
}
