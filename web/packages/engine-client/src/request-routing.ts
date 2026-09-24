import { MUTATE } from "./methods";
import { RpcError } from "./rpc-error";
import { isScopedId, parseScopedId } from "./scoped-id";

/**
 * Request routing at the engine boundary — the port of
 * `request_routing.rs::wire_params` (`crates/ui/src/request_routing.rs:36-94`).
 *
 * Before a call or subscribe leaves for one specific engine, every
 * `ScopedId`-shaped envelope field is decoded back to that engine's raw id;
 * a scoped id belonging to a DIFFERENT engine rejects the request
 * client-side (`"Request identity belongs to another engine"`) — it is never
 * sent over the wire. `targetDeviceId` is always decoded then stripped:
 * routing ends at the socket; no engine forwards a request on to another
 * engine or device. Only RPC envelope identity fields cross this boundary —
 * text, paths, model options, tools, and user JSON stay byte-for-byte.
 */

/** The envelope fields that carry engine-scoped identities. */
const IDENTITY_FIELDS = [
  "chatId",
  "spaceId",
  "deviceId",
  "checkoutId",
  "expectedCheckoutId",
  "docId",
  "parentChatId",
  "targetDeviceId",
] as const;

function decodeField(owner: string, object: Record<string, unknown>, field: string): void {
  const value = object[field];
  if (typeof value === "string") {
    const parsed = parseScopedId(value);
    if (isScopedId(value) && parsed.engine !== null && parsed.engine !== owner) {
      throw new RpcError("failed", "Request identity belongs to another engine");
    }
    object[field] = parsed.rawId;
  }
}

function fields(owner: string, object: Record<string, unknown>): void {
  for (const field of IDENTITY_FIELDS) {
    if (object[field] !== undefined) {
      decodeField(owner, object, field);
    }
  }
  // Routing ends at the socket; no engine forwards this request.
  delete object.targetDeviceId;
}

/**
 * Wire-prepare `params` for a call to `owner`: decode every identity field
 * (top level, inside `target`, and `Mutate`'s `id`), reject foreign scoped
 * ids, and strip `targetDeviceId`. Decodes the params object in place (the
 * desktop mutates its `serde_json::Value` the same way) and returns it.
 */
export function wireParams(owner: string, method: string, params: unknown): unknown {
  if (typeof params !== "object" || params === null || Array.isArray(params)) {
    return params;
  }
  const object = params as Record<string, unknown>;
  fields(owner, object);
  if (typeof object.target === "object" && object.target !== null && !Array.isArray(object.target)) {
    fields(owner, object.target as Record<string, unknown>);
  }
  if (method === MUTATE && typeof object.id === "string") {
    decodeField(owner, object, "id");
  }
  return params;
}
