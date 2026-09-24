export type RpcErrorKind =
  | "unknown-method"
  | "failed"
  | "bad-reply"
  | "transport"
  | "timeout"
  | "closed"
  | "parked";

/** A failed call or stream; `kind` drives re-pair and degrade UX, not the message. */
export class RpcError extends Error {
  readonly kind: RpcErrorKind;
  /** Set when `kind` is "unknown-method": the method the engine lacks. */
  readonly method?: string;

  constructor(kind: RpcErrorKind, message: string, method?: string) {
    super(message);
    this.name = "RpcError";
    this.kind = kind;
    this.method = method;
  }
}

const UNKNOWN_METHOD_PREFIX = "unknown method: ";

/** Classify a wire `{err}` string the way the Rust client does (crates/rpc/src/client.rs). */
export function wireError(message: string): RpcError {
  if (message.startsWith(UNKNOWN_METHOD_PREFIX)) {
    return new RpcError("unknown-method", message, message.slice(UNKNOWN_METHOD_PREFIX.length));
  }
  return new RpcError("failed", message);
}
