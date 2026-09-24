import { RpcError } from "@zeron/engine-client";

/**
 * Sign-in failures as user-facing sentences — the one error vocabulary the
 * sign-in surfaces share (the `/pair` landing page and Settings →
 * Devices).
 */
export function describeSignInError(error: unknown): string {
  if (error instanceof RpcError) {
    if (error.kind === "failed") {
      return "That sign-in code did not work — it may have expired or already have been used.";
    }
    if (error.kind === "transport") {
      return "Could not reach that engine. Check the address and that the engine is running.";
    }
    return error.message;
  }
  if (error instanceof Error && error.message.length > 0) {
    // The store's configuration-error refusal, and any storage failure.
    return error.message;
  }
  return "Sign-in failed.";
}
