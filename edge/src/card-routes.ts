/**
 * Member cards over HTTP. A card needs a username, and a username is claimed
 * once: there is no self-service rename, and operators fix mistakes through
 * `/admin/usernames` (user-directory.ts).
 *
 * Public, routed before the bearer gate:
 *   GET /cards/:username  — a card as JSON, CORS-open
 *   GET /@:username       — a card's page (`zeron.sh/@*` routes here)
 *
 * Signed in, routed after the gate, so a Cloud runner's token is refused
 * (runner-policy.ts): the user id is always the token's `sub`, never input.
 *   GET  /username                  — the caller's username, or null
 *   GET  /username/check?username=  — {available, reason?: invalid|reserved|taken}
 *   POST /username {username}       — claim; a repeat of the same claim succeeds
 */
import type { Verified } from "./auth";
import { CARD_TTL_SECONDS, MISSING_CARD_TTL_SECONDS, cacheCard, cachedCard, forgetCard } from "./card-cache";
import { escapeHtml } from "./auth-routes";
import { CARD_SCENES } from "./card";
import type { Env } from "./env";
import { json, jsonError, readJsonBody } from "./http";
import { foldUsername, isReservedUsername } from "./username";
import { userDirectoryStub, type PublicCard } from "./user-directory";

const MAX_CLAIM_BODY = 1024;

const PAGE_PATH = /^\/(?:@|%40)/i;
const PAGE_NAME = /^\/(?:@|%40)([^/]*)\/?$/i;

const unavailable = (): Response => jsonError(503, "unavailable", "The user directory isn't set up on this server.");

/**
 * The page at `/@username`. For now the card's fields as plain text; this is
 * the seam for `apps/card-web`, which replaces the markup and reads the same
 * fields from the `#card` data block. Member-written text lands in this page,
 * on the landing site's origin: it's escaped, and no script may run.
 */
const cardPage = (card: PublicCard): Response => {
  const name = escapeHtml(card.firstName);
  const data = JSON.stringify(card).replaceAll("<", "\\u003c");
  return page(
    200,
    CARD_TTL_SECONDS,
    `${name} — Zeron`,
    `<h1>${name}</h1>
<p>@${escapeHtml(card.username)} · #${card.userNumber} · joined ${escapeHtml(card.joinedOn)}</p>
<script type="application/json" id="card">${data}</script>`
  );
};

const page = (status: number, ttl: number, title: string, body: string): Response =>
  new Response(
    `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8" />
<meta name="viewport" content="width=device-width, initial-scale=1" />
<meta name="robots" content="noindex" />
<title>${title}</title>
<style>
  body { margin: 0; min-height: 100vh; display: grid; place-items: center; text-align: center;
         background: #0a0a0a; color: #ededed; font: 15px/1.6 ui-sans-serif, system-ui, sans-serif; }
  h1 { font-size: 1.05rem; font-weight: 600; margin: 0 0 0.25rem; }
  p { color: #a1a1a1; margin: 0; }
</style>
</head>
<body><main>${body}</main></body>
</html>`,
    {
      status,
      headers: {
        "content-type": "text/html; charset=utf-8",
        "cache-control": `public, max-age=${ttl}`,
        "content-security-policy": "default-src 'none'; style-src 'unsafe-inline'",
        "x-content-type-options": "nosniff"
      }
    }
  );

const cardJson = (card: PublicCard | null): Response =>
  new Response(JSON.stringify(card ?? { error: "not_found", message: "No one has this username." }), {
    status: card ? 200 : 404,
    headers: {
      "content-type": "application/json",
      "cache-control": `public, max-age=${card ? CARD_TTL_SECONDS : MISSING_CARD_TTL_SECONDS}`,
      "access-control-allow-origin": "*"
    }
  });

/** `/cards/:username` and `/@:username`; undefined when neither. */
export const handlePublicCardRoute = async (
  request: Request,
  env: Env,
  ctx: ExecutionContext,
  url: URL
): Promise<Response | undefined> => {
  const parts = url.pathname.split("/").filter(Boolean);
  const isPage = PAGE_PATH.test(url.pathname);
  if (!isPage && parts[0] !== "cards") return undefined;
  if (request.method !== "GET" && request.method !== "HEAD") return jsonError(405, "method_not_allowed", "GET only.");

  const username = foldUsername(isPage ? PAGE_NAME.exec(url.pathname)?.[1] : parts.length === 2 ? parts[1] : undefined);
  let card: PublicCard | null = null;
  if (username) {
    const cached = await cachedCard(username);
    if (cached !== undefined) {
      card = cached;
    } else {
      const directory = userDirectoryStub(env);
      if (!directory) return unavailable();
      card = (await directory.card(username)) ?? null;
      ctx.waitUntil(cacheCard(username, card));
    }
  }
  if (!isPage) return cardJson(card);
  if (card) return cardPage(card);
  return page(404, MISSING_CARD_TTL_SECONDS, "No card here — Zeron", "<h1>No card here</h1><p>No one has this username.</p>");
};

/** `/username[/check]`, past the bearer gate; undefined when neither. */
export const handleUsernameRoute = async (
  request: Request,
  env: Env,
  auth: Verified,
  url: URL
): Promise<Response | undefined> => {
  const parts = url.pathname.split("/").filter(Boolean);
  if (parts[0] !== "username") return undefined;
  const directory = userDirectoryStub(env);
  if (!directory) return unavailable();

  if (parts[1] === "check" && parts.length === 2) {
    if (request.method !== "GET") return jsonError(405, "method_not_allowed", "GET only.");
    const username = foldUsername(url.searchParams.get("username"));
    if (!username) return json({ available: false, reason: "invalid" });
    if (isReservedUsername(username)) return json({ available: false, reason: "reserved" });
    const available = await directory.checkUsername(auth.userId, username);
    if (available === undefined) return tooManyAttempts();
    return json(available ? { available } : { available, reason: "taken" });
  }

  if (parts.length !== 1) return undefined;
  if (request.method === "GET") return json({ username: await directory.username(auth.userId) });
  if (request.method !== "POST") return jsonError(405, "method_not_allowed", "GET or POST only.");

  const body = await readJsonBody(request, MAX_CLAIM_BODY);
  const username = foldUsername((body as { username?: unknown } | undefined)?.username);
  if (!username) {
    return jsonError(400, "invalid_username", "Use 3–20 letters, digits, _ or -, starting with a letter or digit.");
  }
  if (isReservedUsername(username)) return jsonError(403, "reserved_username", "That username is reserved.");
  const claimed = await directory.claimUsername(auth.userId, username, CARD_SCENES);
  switch (claimed.error) {
    case undefined:
      // A cached "no one has this" would hide the new card.
      await forgetCard(username);
      return json(claimed.card);
    case "rate_limited":
      return tooManyAttempts();
    case "already_claimed":
      return jsonError(409, "already_claimed", "You already have a username, and it can't be changed.");
    case "username_taken":
    case "username_retired":
      return jsonError(409, "username_taken", "That username is taken.");
    case "unknown_user":
      // Sign-up and its webhook race; the client can retry.
      return jsonError(409, "not_ready", "Your account is still being set up. Try again in a moment.");
    case "deleted_user":
      return jsonError(403, "deleted_user", "This account has been deleted.");
  }
};

const tooManyAttempts = (): Response =>
  new Response(JSON.stringify({ error: "rate_limited", message: "Too many tries. Wait a minute." }), {
    status: 429,
    headers: { "content-type": "application/json", "retry-after": "60" }
  });
