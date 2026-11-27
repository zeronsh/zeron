/**
 * Outside agents in a device's room (docs/mcp.md, "Outside agents";
 * mcp-oauth.ts is the Worker's side). The room keeps what an outside agent's
 * access to this device is made of on the edge — the person's asks, the
 * authorization codes, the access tokens — and relays the agent's MCP
 * requests to its host as `mcp` frames. Only the room sends those (it drops
 * any a client sends; the host believes only frames without `from`), and
 * only the host answers them.
 *
 * - An ask (`POST /mcp/ask`, from the consent page) goes to the host and
 *   waits for the person's answer there. The device makes the grant id; the
 *   room records the grant only once the device answers with it, and an ask
 *   gets its one code only after that.
 * - A code is single use and short-lived: any redemption burns it, and
 *   redeeming one again revokes the tokens it gave. A token lasts hours, for
 *   its user, this device, its grant and `mcp:agent` alone. Codes and tokens
 *   are stored as SHA-256 digests, never as themselves.
 * - A call (`POST /mcp/call`) needs a live token and a grant the room holds,
 *   and goes to the live host or nowhere: never queued. The device answers
 *   401 for a grant it no longer keeps, and the room drops it then.
 * - Joining, the host lists the grants it keeps and the room drops the rest;
 *   a revoke on the device drops one at once.
 *
 * Internal only: the Worker reaches these paths (mcp-oauth.ts); its routes
 * never forward a client's path here.
 */
import { b64urlEncode, constantTimeEqual, sha256B64url } from "./cloud/crypto";
import type { DeviceFrameHeader } from "./device-room";

export const MCP_KIND = "mcp";
export const MCP_SCOPE = "mcp:agent";

/** How long an ask waits for the device's answer. */
export const ASK_TTL_MS = 5 * 60_000;
/** How long an authorization code may be redeemed. */
export const CODE_TTL_MS = 2 * 60_000;
/** How long an access token works. No refresh: the agent signs in again. */
export const TOKEN_TTL_MS = 8 * 3_600_000;
/** Asks waiting at once in one room. */
export const MAX_WAITING_ASKS = 5;
/** Grants one device holds. */
export const MAX_GRANTS = 32;
/** Live tokens per grant; past this the oldest go. */
export const MAX_TOKENS_PER_GRANT = 8;
/** Calls relayed at once. */
export const MAX_CALLS = 16;
/** How long a call waits for the device to start answering. */
export const HEAD_TIMEOUT_MS = 30_000;

/** A grant id as a device makes one. */
const GRANT_RE = /^[A-Za-z0-9_-]{16,64}$/;
/** RFC 7636 §4.1. */
const VERIFIER_RE = /^[A-Za-z0-9._~-]{43,128}$/;
/** Where the call's headers stop: what the device's endpoint reads. */
const CALL_HEADERS = ["accept", "content-type", "mcp-protocol-version", "mcp-method", "mcp-name"] as const;

const CODE_PREFIX = "zmcpc1";
const TOKEN_PREFIX = "zmcp1";
const SECRET_RE = /^[A-Za-z0-9_-]{43}$/;
const DEVICE_RE = /^[A-Za-z0-9_-]{1,128}$/;

/** `prefix.device.secret`: the device the room is, so the Worker can route. */
const mint = (prefix: string, device: string): string =>
  `${prefix}.${device}.${b64urlEncode(crypto.getRandomValues(new Uint8Array(32)))}`;

const deviceIn = (prefix: string, value: unknown): string | undefined => {
  if (typeof value !== "string") return undefined;
  const parts = value.split(".");
  if (parts.length !== 3 || parts[0] !== prefix || !DEVICE_RE.test(parts[1]!) || !SECRET_RE.test(parts[2]!)) return undefined;
  return parts[1];
};

/** The device an authorization code is for, or undefined for no code of ours. */
export const codeDevice = (code: unknown): string | undefined => deviceIn(CODE_PREFIX, code);
/** The device an access token is for, or undefined for no token of ours. */
export const tokenDevice = (token: unknown): string | undefined => deviceIn(TOKEN_PREFIX, token);

const digest = (value: string): Promise<string> => sha256B64url(value);

interface Head {
  status: number;
  headers: Record<string, string>;
}

interface PendingCall {
  grant: string;
  host: WebSocket;
  headed: (head: Head | undefined) => void;
  controller?: ReadableStreamDefaultController<Uint8Array>;
  buffered: Uint8Array[];
  ended: boolean;
}

/** What the consent page sends for an ask. */
export interface AskRequest {
  readonly clientId: string;
  readonly clientName: string;
  readonly redirectUri: string;
  readonly codeChallenge: string;
  readonly state: string | null;
}

/** An ask as the waiting page reads it. */
export type AskStatus =
  | { readonly status: "waiting" | "issued" | "expired" }
  | { readonly status: "denied"; readonly redirectUri: string; readonly state: string | null }
  | { readonly status: "allowed"; readonly redirectUri: string; readonly state: string | null; readonly code: string };

const json = (value: unknown, status = 200): Response =>
  new Response(JSON.stringify(value), { status, headers: { "content-type": "application/json" } });

/** A JSON-RPC error for `body`'s request (the device's own refusals' shape). */
const rpcError = (body: string, status: number, message: string): Response => {
  let id: unknown = null;
  try {
    const parsed = JSON.parse(body) as { id?: unknown };
    if (typeof parsed.id === "string" || typeof parsed.id === "number") id = parsed.id;
  } catch {
    /* no id to answer */
  }
  return json({ jsonrpc: "2.0", id, error: { code: -32000, message } }, status);
};

export class McpRelay {
  private readonly calls = new Map<string, PendingCall>();
  private readonly encoder = new TextEncoder();

  constructor(
    private readonly sql: SqlStorage,
    private readonly liveHost: () => WebSocket | undefined,
    private readonly deliver: (ws: WebSocket, header: DeviceFrameHeader, payload: Uint8Array) => void
  ) {
    sql.exec(`CREATE TABLE IF NOT EXISTS mcp_asks (
      id TEXT PRIMARY KEY, user_id TEXT NOT NULL, client_id TEXT NOT NULL, client_name TEXT NOT NULL,
      redirect_uri TEXT NOT NULL, code_challenge TEXT NOT NULL, state TEXT, status TEXT NOT NULL,
      grant_id TEXT, expires_at INTEGER NOT NULL)`);
    sql.exec(`CREATE TABLE IF NOT EXISTS mcp_grants (
      id TEXT PRIMARY KEY, user_id TEXT NOT NULL, client_id TEXT NOT NULL, created_at INTEGER NOT NULL)`);
    sql.exec(`CREATE TABLE IF NOT EXISTS mcp_codes (
      hash TEXT PRIMARY KEY, user_id TEXT NOT NULL, client_id TEXT NOT NULL, redirect_uri TEXT NOT NULL,
      code_challenge TEXT NOT NULL, grant_id TEXT NOT NULL, expires_at INTEGER NOT NULL, used INTEGER NOT NULL DEFAULT 0)`);
    sql.exec(`CREATE TABLE IF NOT EXISTS mcp_tokens (
      hash TEXT PRIMARY KEY, user_id TEXT NOT NULL, grant_id TEXT NOT NULL, client_id TEXT NOT NULL,
      scope TEXT NOT NULL, code_hash TEXT NOT NULL, created_at INTEGER NOT NULL, expires_at INTEGER NOT NULL)`);
  }

  private prune(now: number): void {
    this.sql.exec("DELETE FROM mcp_asks WHERE expires_at <= ?", now);
    this.sql.exec("DELETE FROM mcp_codes WHERE expires_at <= ?", now);
    this.sql.exec("DELETE FROM mcp_tokens WHERE expires_at <= ?", now);
  }

  /** Drop `grant` and everything that reached it. */
  private dropGrant(grant: string): void {
    this.sql.exec("DELETE FROM mcp_tokens WHERE grant_id = ?", grant);
    this.sql.exec("DELETE FROM mcp_codes WHERE grant_id = ?", grant);
    this.sql.exec("DELETE FROM mcp_grants WHERE id = ?", grant);
  }

  private send(host: WebSocket, stream: string, message: unknown): void {
    this.deliver(host, { s: stream, k: MCP_KIND }, this.encoder.encode(JSON.stringify(message)));
  }

  private askFrame(host: WebSocket, id: string, clientName: string, redirectUri: string): void {
    this.send(host, id, { t: "ask", client: clientName, redirectHost: new URL(redirectUri).host });
  }

  /** `/mcp/*`. `userId` is the Worker's stamp (the consent page's
   * signed-in person); the code and token paths carry their own proof. */
  async fetch(request: Request, url: URL, userId: string | null, owner: string | undefined): Promise<Response> {
    const now = Date.now();
    this.prune(now);
    if (url.pathname === "/mcp/ask") {
      if (!userId || !owner || owner !== userId) return json({ error: "forbidden" }, 403);
      if (request.method === "POST") return this.openAsk((await request.json()) as AskRequest, userId, now);
      if (request.method === "GET") {
        return json(await this.readAsk(url.searchParams.get("id") ?? "", url.searchParams.get("device") ?? "", userId, now));
      }
    }
    if (url.pathname === "/mcp/token" && request.method === "POST") {
      const body = (await request.json()) as { code?: string; device?: string; clientId?: string; redirectUri?: string; codeVerifier?: string };
      const issued = await this.redeem(body, owner, now);
      return issued ? json(issued) : json({ error: "invalid_grant" }, 400);
    }
    if (url.pathname === "/mcp/call" && request.method === "POST") return this.call(request, owner, now);
    return json({ error: "not_found" }, 404);
  }

  private openAsk(ask: AskRequest, userId: string, now: number): Response {
    const host = this.liveHost();
    if (!host) return json({ error: "offline" }, 409);
    const waiting = this.sql.exec("SELECT COUNT(*) AS n FROM mcp_asks WHERE status = 'waiting'").one().n as number;
    if (waiting >= MAX_WAITING_ASKS) return json({ error: "busy" }, 429);
    const id = crypto.randomUUID();
    this.sql.exec(
      `INSERT INTO mcp_asks (id, user_id, client_id, client_name, redirect_uri, code_challenge, state, status, expires_at)
       VALUES (?, ?, ?, ?, ?, ?, ?, 'waiting', ?)`,
      id,
      userId,
      ask.clientId,
      ask.clientName,
      ask.redirectUri,
      ask.codeChallenge,
      ask.state,
      now + ASK_TTL_MS
    );
    this.askFrame(host, id, ask.clientName, ask.redirectUri);
    return json({ ask: id });
  }

  /** An ask's state; an allowed one's code is minted here, once. */
  private async readAsk(id: string, device: string, userId: string, now: number): Promise<AskStatus> {
    const [row] = this.sql.exec("SELECT * FROM mcp_asks WHERE id = ? AND user_id = ?", id, userId).toArray();
    if (!row) return { status: "expired" };
    const redirectUri = row.redirect_uri as string;
    const state = (row.state as string | null) ?? null;
    if (row.status === "denied") return { status: "denied", redirectUri, state };
    if (row.status !== "allowed") return { status: row.status === "issued" ? "issued" : "waiting" };
    const grant = row.grant_id as string;
    if (!DEVICE_RE.test(device) || this.sql.exec("SELECT 1 FROM mcp_grants WHERE id = ?", grant).toArray().length === 0) {
      return { status: "expired" };
    }
    // Claimed before the first await, so two polls can't both mint.
    const claimed = this.sql.exec("UPDATE mcp_asks SET status = 'issued' WHERE id = ? AND status = 'allowed' RETURNING id", id);
    if (claimed.toArray().length === 0) return { status: "issued" };
    const code = mint(CODE_PREFIX, device);
    this.sql.exec(
      `INSERT INTO mcp_codes (hash, user_id, client_id, redirect_uri, code_challenge, grant_id, expires_at)
       VALUES (?, ?, ?, ?, ?, ?, ?)`,
      await digest(code),
      userId,
      row.client_id,
      redirectUri,
      row.code_challenge,
      grant,
      now + CODE_TTL_MS
    );
    return { status: "allowed", redirectUri, state, code };
  }

  private async redeem(
    body: { code?: string; device?: string; clientId?: string; redirectUri?: string; codeVerifier?: string },
    owner: string | undefined,
    now: number
  ): Promise<{ token: string; expiresIn: number } | undefined> {
    if (typeof body.code !== "string" || typeof body.device !== "string" || codeDevice(body.code) !== body.device) return undefined;
    const hash = await digest(body.code);
    const verified = typeof body.codeVerifier === "string" && VERIFIER_RE.test(body.codeVerifier) ? await digest(body.codeVerifier) : "";
    const token = mint(TOKEN_PREFIX, body.device);
    const tokenHash = await digest(token);
    // No await from here on, so a redemption racing this one sees it used.
    const [row] = this.sql.exec("SELECT * FROM mcp_codes WHERE hash = ?", hash).toArray();
    if (!row) return undefined;
    // Burnt by any attempt; a second one means the code leaked, so what
    // the first gave goes too (RFC 6749 §4.1.2).
    if (row.used) {
      this.sql.exec("DELETE FROM mcp_tokens WHERE code_hash = ?", hash);
      return undefined;
    }
    this.sql.exec("UPDATE mcp_codes SET used = 1 WHERE hash = ?", hash);
    if ((row.expires_at as number) <= now || row.client_id !== body.clientId || row.redirect_uri !== body.redirectUri) {
      return undefined;
    }
    if (!verified || !constantTimeEqual(verified, row.code_challenge as string)) return undefined;
    const grant = row.grant_id as string;
    if (!owner || owner !== row.user_id || this.sql.exec("SELECT 1 FROM mcp_grants WHERE id = ?", grant).toArray().length === 0) {
      return undefined;
    }
    this.sql.exec(
      `INSERT INTO mcp_tokens (hash, user_id, grant_id, client_id, scope, code_hash, created_at, expires_at)
       VALUES (?, ?, ?, ?, ?, ?, ?, ?)`,
      tokenHash,
      row.user_id,
      grant,
      row.client_id,
      MCP_SCOPE,
      hash,
      now,
      now + TOKEN_TTL_MS
    );
    this.sql.exec(
      `DELETE FROM mcp_tokens WHERE grant_id = ? AND hash NOT IN
         (SELECT hash FROM mcp_tokens WHERE grant_id = ? ORDER BY created_at DESC LIMIT ?)`,
      grant,
      grant,
      MAX_TOKENS_PER_GRANT
    );
    return { token, expiresIn: Math.floor(TOKEN_TTL_MS / 1000) };
  }

  private async call(request: Request, owner: string | undefined, now: number): Promise<Response> {
    const bearer = request.headers.get("authorization")?.replace(/^Bearer /, "") ?? "";
    const body = await request.text();
    const [token] = tokenDevice(bearer)
      ? this.sql.exec("SELECT user_id, grant_id, scope, expires_at FROM mcp_tokens WHERE hash = ?", await digest(bearer)).toArray()
      : [];
    if (!token || (token.expires_at as number) <= now || token.scope !== MCP_SCOPE || !owner || owner !== token.user_id) {
      return json({ error: "invalid_token" }, 401);
    }
    const grant = token.grant_id as string;
    if (this.sql.exec("SELECT 1 FROM mcp_grants WHERE id = ?", grant).toArray().length === 0) {
      return json({ error: "invalid_token" }, 401);
    }
    const host = this.liveHost();
    if (!host) return rpcError(body, 503, "device offline: open Zeron on it and try again");
    if (this.calls.size >= MAX_CALLS) return rpcError(body, 503, "too many calls at once on this device");

    const headers: Record<string, string> = {};
    for (const name of CALL_HEADERS) {
      const value = request.headers.get(name);
      if (value !== null) headers[name] = value;
    }
    const stream = crypto.randomUUID();
    const head = new Promise<Head | undefined>((headed) => {
      this.calls.set(stream, { grant, host, headed, buffered: [], ended: false });
    });
    this.send(host, stream, { t: "call", grant, headers, body });
    let timer: ReturnType<typeof setTimeout> | undefined;
    const answered = await Promise.race([
      head,
      new Promise<undefined>((late) => {
        timer = setTimeout(() => late(undefined), HEAD_TIMEOUT_MS);
      })
    ]);
    clearTimeout(timer);
    const pending = this.calls.get(stream);
    if (!answered || !pending) {
      this.calls.delete(stream);
      if (this.liveHost() === host) this.send(host, stream, { t: "cancel" });
      return rpcError(body, 503, "the device didn't answer");
    }
    if (answered.status === 401) {
      // The device doesn't keep this grant (revoked there, or never made).
      this.calls.delete(stream);
      this.dropGrant(grant);
      return json({ error: "invalid_token" }, 401);
    }
    const relay = this;
    const out = new ReadableStream<Uint8Array>({
      start(controller) {
        pending.controller = controller;
        for (const chunk of pending.buffered) controller.enqueue(chunk);
        pending.buffered = [];
        if (pending.ended) {
          controller.close();
          relay.calls.delete(stream);
        }
      },
      cancel() {
        // The agent closed the stream: the 2026 cancel.
        relay.calls.delete(stream);
        if (relay.liveHost() === host) relay.send(host, stream, { t: "cancel" });
      }
    });
    const contentType = answered.headers["content-type"];
    return new Response(out, {
      status: answered.status,
      headers: {
        ...(contentType ? { "content-type": contentType } : {}),
        "cache-control": "no-store"
      }
    });
  }

  /** An `mcp` frame from the host. */
  hostFrame(header: DeviceFrameHeader, payload: Uint8Array): void {
    let frame: Record<string, unknown>;
    try {
      frame = JSON.parse(new TextDecoder().decode(payload)) as Record<string, unknown>;
    } catch {
      return;
    }
    const call = this.calls.get(header.s);
    switch (frame.t) {
      case "answer":
        this.answer(header.s, frame);
        return;
      case "grants": {
        const kept = new Set(Array.isArray(frame.grants) ? frame.grants.filter((g): g is string => typeof g === "string") : []);
        for (const row of this.sql.exec("SELECT id FROM mcp_grants").toArray()) {
          if (!kept.has(row.id as string)) this.dropGrant(row.id as string);
        }
        return;
      }
      case "revoked":
        if (typeof frame.grant === "string") this.dropGrant(frame.grant);
        return;
      case "head": {
        if (!call) return;
        const status = typeof frame.status === "number" && frame.status >= 200 && frame.status <= 599 ? frame.status : 502;
        const sent = (frame.headers ?? {}) as Record<string, unknown>;
        const contentType = typeof sent["content-type"] === "string" ? sent["content-type"] : undefined;
        call.headed({ status, headers: contentType ? { "content-type": contentType } : {} });
        return;
      }
      case "data": {
        if (!call || typeof frame.data !== "string") return;
        const chunk = this.encoder.encode(frame.data);
        if (call.controller) {
          try {
            call.controller.enqueue(chunk);
          } catch {
            this.calls.delete(header.s);
          }
        } else call.buffered.push(chunk);
        return;
      }
      case "end": {
        if (!call) return;
        if (call.controller) {
          try {
            call.controller.close();
          } catch {
            /* already closed */
          }
          this.calls.delete(header.s);
        } else call.ended = true;
        return;
      }
    }
  }

  /** The device's answer to ask `id`: the grant it now keeps, or a no. */
  private answer(id: string, frame: Record<string, unknown>): void {
    const [row] = this.sql.exec("SELECT * FROM mcp_asks WHERE id = ? AND status = 'waiting'", id).toArray();
    if (!row || (row.expires_at as number) <= Date.now()) return;
    const grant = frame.grant;
    const taken =
      typeof grant === "string" && this.sql.exec("SELECT 1 FROM mcp_grants WHERE id = ?", grant).toArray().length > 0;
    const full = (this.sql.exec("SELECT COUNT(*) AS n FROM mcp_grants").one().n as number) >= MAX_GRANTS;
    if (frame.denied === true || typeof grant !== "string" || !GRANT_RE.test(grant) || taken || full) {
      this.sql.exec("UPDATE mcp_asks SET status = 'denied' WHERE id = ?", id);
      return;
    }
    this.sql.exec(
      "INSERT INTO mcp_grants (id, user_id, client_id, created_at) VALUES (?, ?, ?, ?)",
      grant,
      row.user_id,
      row.client_id,
      Date.now()
    );
    this.sql.exec("UPDATE mcp_asks SET status = 'allowed', grant_id = ? WHERE id = ?", grant, id);
  }

  /** A host joined: the asks still waiting are its to answer. */
  hostJoined(host: WebSocket): void {
    this.prune(Date.now());
    for (const row of this.sql.exec("SELECT id, client_name, redirect_uri FROM mcp_asks WHERE status = 'waiting'").toArray()) {
      this.askFrame(host, row.id as string, row.client_name as string, row.redirect_uri as string);
    }
  }

  /** `host` went away: what it was answering ends (nothing is retried). */
  hostGone(host: WebSocket): void {
    for (const [stream, call] of this.calls) {
      if (call.host !== host) continue;
      this.calls.delete(stream);
      call.headed(undefined);
      try {
        call.controller?.close();
      } catch {
        /* already closed */
      }
    }
  }
}
