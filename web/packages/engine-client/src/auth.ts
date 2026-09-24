import { RpcError } from "./rpc-error";

export interface SignInConfig {
  /** `"dev"`: the bearer is a local user id. `"workos"`: AuthKit flow. */
  readonly mode: "dev" | "workos";
  /** The AuthKit authorize URL when `mode` is `"workos"`; `null` in dev. */
  readonly authorizeUrl: string | null;
}

export interface SignInTokens {
  readonly accessToken: string;
  readonly refreshToken: string | null;
  readonly userId: string;
  readonly email: string | null;
}

export interface AuthFetchOptions {
  readonly fetch?: typeof fetch;
  readonly timeoutMs?: number;
}

const DEFAULT_TIMEOUT_MS = 15_000;

/**
 * Fetch the engine's pre-auth sign-in configuration (`GET /auth/config`):
 * dev mode signs in with a local user id; WorkOS mode redirects the
 * browser to the authorize URL and exchanges the pasted code.
 */
export async function fetchSignInConfig(
  baseUrl: string,
  options: AuthFetchOptions = {},
): Promise<SignInConfig> {
  const fetcher = options.fetch ?? fetch;
  const url = `${baseUrl.replace(/\/+$/, "")}/auth/config`;
  let response: Response;
  try {
    response = await fetcher(url, {
      redirect: "error",
      signal: AbortSignal.timeout(options.timeoutMs ?? DEFAULT_TIMEOUT_MS),
    });
  } catch {
    throw new RpcError("transport", "Could not reach that engine. Check the address and that the engine is running.");
  }
  if (!response.ok) {
    throw new RpcError("transport", `Sign-in endpoint returned HTTP ${response.status}`);
  }
  let config: unknown;
  try {
    config = await response.json();
  } catch {
    throw new RpcError("bad-reply", "Invalid sign-in configuration");
  }
  const record = config as Record<string, unknown> | null;
  const mode = record?.mode;
  if (mode !== "dev" && mode !== "workos") {
    throw new RpcError("bad-reply", "Invalid sign-in configuration");
  }
  const authorizeUrl = record?.authorizeUrl;
  if (mode === "workos" && typeof authorizeUrl !== "string") {
    throw new RpcError("bad-reply", "Sign-in configuration is missing the authorize URL");
  }
  return { mode, authorizeUrl: mode === "workos" ? (authorizeUrl as string) : null };
}

/**
 * Exchange a pasted WorkOS sign-in code at the engine's proxied
 * `POST /auth/exchange`. The engine forwards the code to the edge (the
 * WorkOS API key lives only there) and passes the tokens through, so the
 * browser never needs CORS on the edge.
 */
export async function exchangeSignInCode(
  baseUrl: string,
  code: string,
  options: AuthFetchOptions = {},
): Promise<SignInTokens> {
  const fetcher = options.fetch ?? fetch;
  const url = `${baseUrl.replace(/\/+$/, "")}/auth/exchange`;
  let response: Response;
  try {
    response = await fetcher(url, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ code }),
      redirect: "error",
      signal: AbortSignal.timeout(options.timeoutMs ?? DEFAULT_TIMEOUT_MS),
    });
  } catch {
    throw new RpcError("transport", "Could not reach the engine's sign-in endpoint");
  }
  if (response.status === 401) {
    throw new RpcError("failed", "That sign-in code did not work — it may have expired or already have been used.");
  }
  if (response.status === 501) {
    throw new RpcError("failed", "This engine is in development sign-in mode; no code exchange is needed.");
  }
  if (!response.ok) {
    throw new RpcError("transport", `Sign-in endpoint returned HTTP ${response.status}`);
  }
  let tokens: unknown;
  try {
    tokens = await response.json();
  } catch {
    throw new RpcError("bad-reply", "Invalid sign-in response");
  }
  const record = tokens as Record<string, unknown> | null;
  const user = record?.user as Record<string, unknown> | null | undefined;
  if (
    typeof record?.accessToken !== "string" ||
    typeof user?.id !== "string" ||
    (record?.refreshToken !== undefined && record?.refreshToken !== null && typeof record.refreshToken !== "string")
  ) {
    throw new RpcError("bad-reply", "Invalid sign-in response");
  }
  return {
    accessToken: record.accessToken,
    refreshToken: typeof record.refreshToken === "string" ? record.refreshToken : null,
    userId: user.id,
    email: typeof user.email === "string" ? user.email : null,
  };
}

/**
 * Normalize an engine address the way the fleet store keys engines:
 * `http(s)://host[:port]` with no path, query, or fragment.
 */
export function parseEngineUrl(input: string): { baseUrl: string } {
  let url: URL;
  try {
    url = new URL(input.trim());
  } catch {
    throw new RpcError("transport", "Engine address must be a http:// or https:// URL");
  }
  if ((url.protocol !== "http:" && url.protocol !== "https:") || url.hostname.length === 0) {
    throw new RpcError("transport", "Engine address must be a http:// or https:// URL");
  }
  if (url.username.length > 0 || url.password.length > 0 || url.search.length > 0 || url.hash.length > 0) {
    throw new RpcError("transport", "Engine address must not carry user information, a query, or a fragment");
  }
  if (url.pathname !== "/" && url.pathname !== "") {
    throw new RpcError("transport", "Engine address must be a bare origin, not a path");
  }
  return { baseUrl: `${url.protocol}//${url.host}` };
}
