/**
 * Version compatibility (docs/compat.md).
 *
 * Every client from protocol 1 on announces its protocol: `proto=` on socket
 * URLs, the `x-zeron-proto` header on HTTP. Builds from before that announce
 * nothing and count as protocol 0.
 *
 * - The registry hides protocol ≥ 1 devices' presence from protocol-0
 *   sockets, so those builds see newer devices as offline (and their peer
 *   links never dial them).
 * - A refusal is HTTP 426 with `x-zeron-update-required: app|room`. Clients
 *   from protocol 1 on show "update Zeron" for it instead of a connection
 *   error and stop redialing at their normal cadence. Protocol-0 builds just
 *   fail to connect.
 * - `MIN_CLIENT_PROTOCOL` (a Worker var, default 0) refuses older clients on
 *   every sync route (`app`). A room that moved to a generation older builds
 *   can't read refuses everything (`room`) — a chat3 chat's `chat2/{id}`.
 */

export const UPDATE_HEADER = "x-zeron-update-required";
export const PROTO_HEADER = "x-zeron-proto";

/** The caller's announced protocol (0 when absent or malformed). */
export const clientProtocol = (request: Request, url: URL = new URL(request.url)): number => {
  const raw = url.searchParams.get("proto") ?? request.headers.get(PROTO_HEADER) ?? "";
  const value = Number(raw);
  return raw !== "" && Number.isInteger(value) && value >= 0 && value < 1_000_000 ? value : 0;
};

/** The lowest protocol the sync routes accept. */
export const minClientProtocol = (env: { MIN_CLIENT_PROTOCOL?: string }): number => {
  const value = Number(env.MIN_CLIENT_PROTOCOL ?? "0");
  return Number.isInteger(value) && value > 0 ? value : 0;
};

export const updateRequired = (scope: "app" | "room", message: string): Response =>
  new Response(JSON.stringify({ error: "update_required", scope, message }), {
    status: 426,
    headers: { "content-type": "application/json", [UPDATE_HEADER]: scope }
  });

/** The operator's minimum-protocol gate on the sync routes (`registry`,
 * `chat2`, `chat3`): a refusal, or undefined to carry on. */
export const syncGate = (env: { MIN_CLIENT_PROTOCOL?: string }, request: Request, url: URL): Response | undefined => {
  const route = url.pathname.split("/").filter(Boolean)[0] ?? "";
  if (!["registry", "chat2", "chat3"].includes(route)) return undefined;
  if (clientProtocol(request, url) >= minClientProtocol(env)) return undefined;
  return updateRequired("app", "This version of Zeron is no longer supported. Update to keep syncing.");
};

/** The `?search` a registry forward carries: the caller's own query with its
 * protocol stamped (from the query or the header), which decides whose
 * presence it sees. */
export const registrySearch = (request: Request, url: URL, base: string = url.search): string => {
  const search = new URLSearchParams(base);
  search.set("proto", String(clientProtocol(request, url)));
  return `?${search}`;
};

/** Close code for sockets of a room that just started refusing them. */
export const UPDATE_CLOSE_CODE = 4426;
