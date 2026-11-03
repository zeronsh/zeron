/**
 * `/onboarding`: which onboarding pages the signed-in user has finished, so
 * the app shows only the ones left (one-off pages too, like a release's
 * "what's new"). The user is always the bearer's; runner tokens never get
 * here (runner-policy.ts names no `/onboarding`).
 *
 *   GET  /onboarding                    → { done: { [pageId]: isoTime } }
 *   POST /onboarding { done: string[] } → the same, after recording them; a
 *                                         page already done keeps its first time
 *
 * Page ids match `PAGE_ID_RE`; at most `MAX_PAGES_PER_REQUEST` a request and
 * `MAX_ONBOARDING_PAGES` stored a user, else 400 `{ error: "invalid" }`.
 * Stored in the user directory (user-directory.ts, `onboarding_done`).
 */
import type { Verified } from "./auth";
import type { Env } from "./env";
import { json, jsonError, readJsonBody } from "./http";
import { userDirectoryStub } from "./user-directory";

export const PAGE_ID_RE = /^[a-z0-9][a-z0-9-]{0,63}$/;
export const MAX_PAGES_PER_REQUEST = 50;
/** 50 ids of 64 chars, quoted, with room to spare. */
const MAX_BODY = 8 * 1024;

const invalid = (): Response => json({ error: "invalid" }, 400);

export const handleOnboardingRoute = async (
  request: Request,
  env: Env,
  auth: Verified,
  url: URL
): Promise<Response | undefined> => {
  if (url.pathname !== "/onboarding") return undefined;
  const directory = userDirectoryStub(env);
  if (!directory) return jsonError(503, "unavailable", "The user directory isn't set up on this server.");
  if (request.method === "GET") return json({ done: await directory.onboarding(auth.userId) });
  if (request.method !== "POST") return jsonError(405, "method_not_allowed", "GET or POST only.");
  const done = ((await readJsonBody(request, MAX_BODY)) as { done?: unknown } | undefined)?.done;
  if (
    !Array.isArray(done) ||
    done.length > MAX_PAGES_PER_REQUEST ||
    !done.every((id) => typeof id === "string" && PAGE_ID_RE.test(id))
  ) {
    return invalid();
  }
  const recorded = await directory.markOnboarding(auth.userId, done as string[]);
  return recorded ? json({ done: recorded }) : invalid();
};
