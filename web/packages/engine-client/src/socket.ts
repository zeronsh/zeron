export interface SocketClose {
  readonly code: number;
  readonly reason: string;
  readonly wasClean: boolean;
}

/**
 * The socket surface the client uses — the DOM WebSocket's shape, so the
 * browser default needs no adapter. Tests wrap the `ws` library's client,
 * which can also `terminate()` to simulate a hard network drop.
 */
export interface WsSocket {
  send(data: string): void;
  close(code?: number, reason?: string): void;
  addEventListener(type: "open", listener: () => void): void;
  addEventListener(type: "message", listener: (event: { data: unknown }) => void): void;
  addEventListener(type: "close", listener: (event: SocketClose) => void): void;
  addEventListener(type: "error", listener: () => void): void;
}

export type WebSocketFactory = (url: string) => WsSocket;

/** The browser's native WebSocket. */
export const browserWebSocket: WebSocketFactory = (url) => new WebSocket(url);
