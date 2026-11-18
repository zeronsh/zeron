/**
 * `GET /.well-known/zeron` — what a Zeron app needs to know about a server
 * before anyone signs in to it (docs/self-hosting.md).
 *
 * A person who points an app at a self-hosted edge types its address and
 * nothing else: the app reads this document to learn that the address is a
 * Zeron server at all, and which WorkOS client its sign-in goes through. The
 * client id is public (it is in every authorize URL and in the JWKS path), so
 * publishing it costs nothing; the API key never leaves the Worker.
 *
 * Versioned by `zeron`: a client that reads a version it doesn't know says
 * the server needs a newer app, rather than guessing.
 */
import type { Env } from "./env";

export const DISCOVERY_PATH = "/.well-known/zeron";

/** The document's format version. Bump only for a change old apps would
 * misread; adding an optional field is not one. */
export const DISCOVERY_VERSION = 1;

export type DiscoveryAuth =
  /** Sign-in through this WorkOS AuthKit client. */
  | { readonly mode: "workos"; readonly workosClientId: string }
  /** Development edge: the bearer is the user id. Apps refuse it. */
  | { readonly mode: "dev" };

export interface Discovery {
  readonly zeron: typeof DISCOVERY_VERSION;
  /** What the operator calls this server (`SERVER_NAME`), when set. */
  readonly name?: string;
  readonly auth: DiscoveryAuth;
}

/** The longest name shown; the apps cut a longer one the same way
 * (crates/proto/src/server.rs `tidy_name`). */
export const NAME_MAX = 64;

/** `SERVER_NAME` as the apps and the join page show it: one line (no
 * direction overrides to turn the text around), at most `NAME_MAX`
 * characters, an ellipsis past that. Undefined when nothing's left. */
export const tidyName = (raw: string | undefined): string | undefined => {
  const words = (raw ?? "")
    .replace(/[\u200E\u200F\u202A-\u202E\u2066-\u2069]/g, "")
    .split(/[\s\p{Cc}]+/u)
    .filter(Boolean);
  if (words.length === 0) return undefined;
  const chars = [...words.join(" ")];
  return chars.length > NAME_MAX ? `${chars.slice(0, NAME_MAX).join("").trimEnd()}…` : chars.join("");
};

/** The discovery document, or which settings keep sign-in from working. */
export const discovery = (env: Env): { ok: true; document: Discovery } | { ok: false; missing: string[] } => {
  const name = tidyName(env.SERVER_NAME);
  if (env.AUTH_MODE === "dev") return { ok: true, document: { zeron: DISCOVERY_VERSION, ...(name ? { name } : {}), auth: { mode: "dev" } } };
  const clientId = env.WORKOS_CLIENT_ID?.trim() ?? "";
  const missing = [
    ...(clientId ? [] : ["WORKOS_CLIENT_ID"]),
    ...(env.WORKOS_API_KEY ? [] : ["WORKOS_API_KEY"])
  ];
  if (missing.length > 0) return { ok: false, missing };
  return {
    ok: true,
    document: { zeron: DISCOVERY_VERSION, ...(name ? { name } : {}), auth: { mode: "workos", workosClientId: clientId } }
  };
};

/** Answer the discovery route; undefined for any other request. */
export const discoveryRoute = (request: Request, env: Env, url: URL): Response | undefined => {
  if (url.pathname !== DISCOVERY_PATH && url.pathname !== `${DISCOVERY_PATH}/`) return undefined;
  if (request.method !== "GET" && request.method !== "HEAD") return undefined;
  const found = discovery(env);
  // A sign-in that can't work is the operator's to fix: say which settings
  // are missing (names only) rather than hand apps a client that fails later.
  const body = found.ok
    ? found.document
    : { zeron: DISCOVERY_VERSION, error: "sign_in_not_configured", missing: found.missing };
  return new Response(request.method === "HEAD" ? null : JSON.stringify(body), {
    status: found.ok ? 200 : 503,
    headers: {
      "content-type": "application/json",
      // Public facts that change only with a deploy.
      "cache-control": found.ok ? "public, max-age=300" : "no-store",
      "access-control-allow-origin": "*"
    }
  });
};
