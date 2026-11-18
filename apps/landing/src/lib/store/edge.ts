// The edge, as the site's server routes reach it: through the EDGE service
// binding, never over the internet, so nothing here holds a secret. Public
// reads carry no cookie (what they return is the same for everyone, which is
// what lets the pages that show them be cached); the signed-in calls carry
// the visitor's cookie, and writes their Origin and the session's CSRF token,
// which the edge checks.
import type {
  AdminPlugin,
  AuditPage,
  CatalogPage,
  Category,
  DeployKey,
  EdgeError,
  ErrorWord,
  KeywordCount,
  MyPlugins,
  PluginDetail,
  QueuePage,
  QueueState,
  ReportsPage,
  Reservation,
  Sort,
  Stats,
  Submission,
  VersionInfo,
  VersionsPage,
  WebSession,
} from "./types.ts";
import { ADMIN_PAGE } from "./paging.ts";

export type Result<T> = { ok: true; data: T } | { ok: false; status: number; error: ErrorWord; message?: string };

const ERROR_WORDS = new Set<string>([
  "not_found", "signed_out", "forbidden", "bad_request", "bad_csrf", "rate_limited", "suspended",
  "needs_username", "id_taken", "id_reserved", "not_semver", "version_exists", "not_newer", "bad_package",
  "bad_signature", "key_not_allowed", "missing_readme", "missing_listing", "too_large", "conflict", "invalid_state",
  "unauthenticated", "bad_cursor", "bad_path", "site_id", "invalid_scope", "not_owner", "not_verified",
  "bad_listing", "claims_zeron",
]);

/** A binding: what `env.EDGE` is, and what a test hands in instead. */
export interface EdgeBinding {
  fetch(input: Request): Promise<Response>;
}

export interface CatalogQuery {
  q?: string;
  category?: string;
  keyword?: string;
  sort?: Sort;
  cursor?: string;
  limit?: number;
  featured?: boolean;
  /** Only plugins whose newest version is verified (amendment 45: a filter, never a sort). */
  verified?: boolean;
  /** One publisher's plugins, by username. */
  author?: string;
}

/** One visitor's view of the edge: built per request from what they sent. */
export class Edge {
  /**
   * Cookies the edge set while answering (a session rotates as it is
   * used): the middleware passes each back to the browser with the page.
   */
  readonly setCookies: string[] = [];
  readonly #binding: EdgeBinding;
  readonly #origin: string;
  readonly #cookie: string | null;
  #session: Promise<WebSession | null> | undefined;
  readonly #from: string | null;

  /**
   * `request` is the visitor's: its origin is the host the edge is asked on
   * (as a browser would), and its cookie and Origin go with signed-in calls.
   */
  constructor(binding: EdgeBinding, request: Request) {
    this.#binding = binding;
    this.#origin = new URL(request.url).origin;
    this.#cookie = request.headers.get("cookie");
    this.#from = request.headers.get("origin");
  }

  // ── Public ────────────────────────────────────────────────────────────

  catalog(query: CatalogQuery): Promise<Result<CatalogPage>> {
    const params = new URLSearchParams();
    if (query.q) params.set("q", query.q);
    if (query.category) params.set("category", query.category);
    if (query.keyword) params.set("keyword", query.keyword);
    if (query.sort) params.set("sort", query.sort);
    if (query.cursor) params.set("cursor", query.cursor);
    if (query.limit) params.set("limit", String(query.limit));
    if (query.featured) params.set("featured", "true");
    if (query.verified) params.set("verified", "1");
    if (query.author) params.set("author", query.author);
    const search = params.size ? `?${params}` : "";
    return this.#json(`/store/v1/plugins${search}`);
  }

  plugin(id: string): Promise<Result<PluginDetail>> {
    return this.#json(`/store/v1/plugins/${segment(id)}`);
  }

  /** A page of a plugin's versions, listed and yanked, newest first (amendment 45: every one can be seen and installed). */
  versions(id: string, cursor?: string, limit?: number): Promise<Result<VersionsPage>> {
    const params = new URLSearchParams();
    if (cursor) params.set("cursor", cursor);
    if (limit) params.set("limit", String(limit));
    return this.#json(`/store/v1/plugins/${segment(id)}/versions${params.size ? `?${params}` : ""}`);
  }

  /** One version of a plugin, listed or yanked. */
  version(id: string, version: string): Promise<Result<VersionInfo>> {
    return this.#json(`/store/v1/plugins/${segment(id)}/versions/${segment(version)}`);
  }

  /** The store's totals: plugins listed, and how many of them are verified. */
  stats(): Promise<Result<Stats>> {
    return this.#json(`/store/v1/stats`);
  }

  /** The latest Zeron released (the edge's /releases/latest.txt), or null when it can't be had. */
  async latestRelease(): Promise<string | null> {
    try {
      const response = await this.#send(this.#request("/releases/latest.txt", { method: "GET" }));
      if (!response.ok) return null;
      const version = (await response.text()).trim();
      return /^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$/.test(version) ? version : null;
    } catch {
      return null;
    }
  }

  /** The categories, each with its count; with `top` (1 to 4), its most installed plugins too. */
  categories(top?: 1 | 2 | 3 | 4): Promise<Result<{ categories: Category[] }>> {
    return this.#json(`/store/v1/categories${top ? `?top=${top}` : ""}`);
  }

  /** The keywords listed plugins carry (in a category, if one is named), most used first. */
  keywords(query: { category?: string; limit?: number } = {}): Promise<Result<{ keywords: KeywordCount[] }>> {
    const params = new URLSearchParams();
    if (query.category) params.set("category", query.category);
    if (query.limit) params.set("limit", String(query.limit));
    return this.#json(`/store/v1/keywords${params.size ? `?${params}` : ""}`);
  }

  /**
   * A file of a version's package, as the edge answers (the caller decides
   * what of it goes to the browser). `signedIn` sends the visitor's cookie:
   * a version taken down or removed is its owner's and staff's only.
   */
  async file(id: string, version: string, path: string, signedIn: boolean): Promise<Response> {
    let url: string;
    try {
      url = `/store/v1/plugins/${segment(id)}/files/${segment(version)}/${path.split("/").map(segment).join("/")}`;
    } catch {
      // (A path no package can hold is a file that isn't there.)
      return new Response(null, { status: 404 });
    }
    // (An edge that can't be reached is a file that can't be had, never a failed page.)
    return this.#send(this.#request(url, { method: "GET", signedIn })).catch(() => new Response(null, { status: 502 }));
  }

  // ── Signed in ─────────────────────────────────────────────────────────

  /** The visitor's session, or null when they aren't signed in. */
  session(): Promise<WebSession | null> {
    // (Read once a request: the admin gate and the page ask the same thing.)
    this.#session ??= this.#cookie ? this.#json<WebSession>("/auth/web/session", { signedIn: true }).then((result) => (result.ok ? result.data : null)) : Promise.resolve(null);
    return this.#session;
  }

  myPlugins(): Promise<Result<MyPlugins>> {
    return this.#json("/store/v1/me/plugins", { signedIn: true });
  }

  /** The account's deploy keys (amendment 44: the account's, each with its scope). */
  keys(): Promise<Result<{ keys: DeployKey[] }>> {
    return this.#json("/store/v1/me/keys", { signedIn: true });
  }

  versionAction(id: string, version: string, action: "yank" | "unyank", csrf: string): Promise<Result<unknown>> {
    return this.#json(`/store/v1/plugins/${segment(id)}/versions/${segment(version)}/${action}`, { method: "POST", csrf, body: {} });
  }

  revokeKey(key: string, csrf: string): Promise<Result<unknown>> {
    return this.#json(`/store/v1/me/keys/${segment(key)}`, { method: "DELETE", csrf });
  }

  report(id: string, body: { reason: string; text: string }, csrf: string): Promise<Result<unknown>> {
    return this.#json(`/store/v1/plugins/${segment(id)}/reports`, { method: "POST", csrf, body });
  }

  // ── Admin ─────────────────────────────────────────────────────────────

  /** The versions to verify (undecided, the default), or those verified, or declined, or all: oldest first. */
  queue(state: QueueState, cursor?: string): Promise<Result<QueuePage>> {
    const params = new URLSearchParams({ state, limit: String(ADMIN_PAGE) });
    if (cursor) params.set("cursor", cursor);
    return this.#json(`/store/v1/admin/queue?${params}`, { signedIn: true });
  }

  /** One version, as staff review it. */
  submission(id: string, version: string): Promise<Result<Submission>> {
    return this.#json(`/store/v1/admin/versions/${segment(id)}/${segment(version)}`, { signedIn: true });
  }

  adminPlugin(id: string): Promise<Result<AdminPlugin>> {
    return this.#json(`/store/v1/admin/plugins/${segment(id)}`, { signedIn: true });
  }

  reports(status: string, cursor?: string): Promise<Result<ReportsPage>> {
    const params = new URLSearchParams({ status, limit: String(ADMIN_PAGE) });
    if (cursor) params.set("cursor", cursor);
    return this.#json(`/store/v1/admin/reports?${params}`, { signedIn: true });
  }

  audit(cursor?: string, plugin?: string): Promise<Result<AuditPage>> {
    const params = new URLSearchParams({ limit: String(ADMIN_PAGE) });
    if (cursor) params.set("cursor", cursor);
    if (plugin) params.set("plugin", plugin);
    return this.#json(`/store/v1/admin/audit?${params}`, { signedIn: true });
  }

  reservations(): Promise<Result<{ reservations: Reservation[] }>> {
    return this.#json("/store/v1/admin/reservations", { signedIn: true });
  }

  /** Free a reserved id (its reservation goes; a claim never used is released). */
  releaseReservation(id: string, csrf: string): Promise<Result<unknown>> {
    return this.#json(`/store/v1/admin/reservations/${segment(id)}`, { method: "DELETE", csrf });
  }

  /** An admin's decision or action: `path` under /store/v1/admin. */
  admin(path: string, body: Record<string, unknown>, csrf: string): Promise<Result<unknown>> {
    return this.#json(`/store/v1/admin/${path}`, { method: "POST", csrf, body });
  }

  // ── The wire ──────────────────────────────────────────────────────────

  #request(path: string, options: { method: string; signedIn?: boolean; csrf?: string; body?: unknown }): Request {
    const headers = new Headers({ accept: "application/json" });
    const signedIn = options.signedIn || options.csrf !== undefined;
    if (signedIn && this.#cookie) headers.set("cookie", this.#cookie);
    if (options.csrf !== undefined) {
      headers.set("x-csrf-token", options.csrf);
      // The visitor's own Origin (Astro has already refused a cross-site
      // form); the edge checks it again.
      if (this.#from) headers.set("origin", this.#from);
    }
    let body: string | undefined;
    if (options.body !== undefined) {
      headers.set("content-type", "application/json");
      body = JSON.stringify(options.body);
    }
    return new Request(new URL(path, this.#origin), { method: options.method, headers, body, redirect: "manual" });
  }

  /** A request to the edge; any cookie it sets is kept for the browser. */
  async #send(request: Request): Promise<Response> {
    const response = await this.#binding.fetch(request);
    // (Only a signed-in call carries a cookie, so only one can rotate it.)
    if (request.headers.has("cookie")) this.setCookies.push(...response.headers.getSetCookie());
    return response;
  }

  async #json<T>(path: string, options: { method?: string; signedIn?: boolean; csrf?: string; body?: unknown } = {}): Promise<Result<T>> {
    let response: Response;
    try {
      response = await this.#send(this.#request(path, { method: "GET", ...options }));
    } catch {
      return { ok: false, status: 502, error: "unknown" };
    }
    let payload: unknown = null;
    try {
      payload = response.status === 204 ? {} : await response.json();
    } catch {
      payload = null;
    }
    if (response.ok && payload !== null) return { ok: true, data: payload as T };
    return failure(response.status, payload);
  }
}

function failure(status: number, payload: unknown): { ok: false; status: number; error: ErrorWord; message?: string } {
  const body = (payload ?? {}) as Partial<EdgeError>;
  const word = typeof body.error === "string" && ERROR_WORDS.has(body.error) ? (body.error as ErrorWord) : status === 404 ? "not_found" : status === 401 ? "signed_out" : status === 403 ? "forbidden" : status === 429 ? "rate_limited" : "unknown";
  return { ok: false, status: status || 502, error: word, message: typeof body.message === "string" ? body.message : undefined };
}

/**
 * One path segment, encoded, so an id or a version can never reach another
 * route. encodeURIComponent leaves "." alone, and the URL parser would
 * resolve "." and ".." away, so they are refused (callers check their input
 * first, names.ts, and this is the last line). A name like "%2e" is safe:
 * encoded it is "%252e", which no parser takes for a dot.
 */
function segment(value: string): string {
  // (Encoded, "%2e" is "%252e": a name, not a dot. Only real dot segments are refused.)
  if (value === "" || value === "." || value === "..") throw new RangeError("not a path segment");
  return encodeURIComponent(value);
}
