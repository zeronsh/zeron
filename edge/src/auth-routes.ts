/**
 * The /auth/* HTTP surface absorbed from zeron's apps/server:
 *
 *  - POST /auth/exchange     — WorkOS code → tokens (see `workos.ts`).
 *  - POST /auth/refresh      — WorkOS refresh → fresh tokens (org-scopable).
 *  - GET  /auth/orgs         — the caller's active org memberships.
 *  - POST /auth/orgs         — create an org + first (admin) membership; one
 *                              per user (409 `org_exists` after that).
 *  - POST /auth/orgs/{id}/invitations — an org admin invites addresses as
 *                              members; WorkOS sends the emails.
 *  - GET|PATCH|DELETE /auth/me, PUT|DELETE /auth/me/photo, GET /auth/avatars/{key},
 *    PUT /auth/me/images, DELETE /auth/me/images/{key}, and the org settings routes (members, invitations,
 *    rename): see "Account and org settings" below.
 *  - GET  /auth/cli/callback — headless sign-in: shows a paste-able code.
 *
 * Exchange/refresh/callback run BEFORE the bearer gate (the caller has no
 * access token yet); the org routes verify the bearer themselves — the user
 * id is ALWAYS the token's `sub`, never request input: users manage their own
 * memberships and no one else's. Error mapping matches the old server: bad
 * body 400, missing bearer 401, WorkOS-off 501, rejected exchange/refresh 401.
 */
import { bearerFromRequest, verifyToken } from "./auth";
import { forgetCard } from "./card-cache";
import type { Env } from "./env";
import { cleanImage } from "./image-check";
import { nameProblem, orgNameProblem } from "./names";
import { MAX_LIVE_STEP_UPS, STEP_UP_TTL_MS, newStepUpToken, stepUpHash, stepUpRequired } from "./step-up";
import { userDirectoryStub, type MemberProfile } from "./user-directory";
import { deleteUserBlobs } from "./user-images";
import {
  WorkOsAuthFailed,
  createOrg,
  deleteOrg,
  deleteUser,
  exchange,
  getInvitation,
  getMembership,
  getUser,
  listInvitations,
  listOrgMembers,
  listOrgs,
  refresh,
  removeMembership,
  renameOrg,
  resendInvitation,
  revokeInvitation,
  sendInvitation,
  setMembershipRole,
  updateUserName,
  type MembershipRef,
  type OrgMember,
  type WireInvitation,
  type WireProfile
} from "./workos";

const json = (value: unknown, status = 200): Response =>
  new Response(JSON.stringify(value), {
    status,
    headers: { "content-type": "application/json" }
  });

const notConfigured = (): Response => json({ error: "workos not configured" }, 501);

/** The caller already created `organizationId` (null: it's being created by
 * another request right now). */
const orgExists = (organizationId: string | null): Response => json({ error: "org_exists", organizationId }, 409);

const authFailed = (e: unknown): Response =>
  json({ error: e instanceof WorkOsAuthFailed ? e.message : "authentication failed" }, 401);

const bodyJson = async <T>(request: Request): Promise<T | undefined> => {
  try {
    return (await request.json()) as T;
  } catch {
    return undefined;
  }
};

/** A new step-up ticket for `userId`, or undefined when the directory can't
 * take one (logged without the user). */
const mintStepUp = async (env: Env, userId: string): Promise<{ token: string; expiresAt: number } | undefined> => {
  const directory = userDirectoryStub(env);
  if (!directory) return undefined;
  const token = newStepUpToken();
  try {
    const expiresAt = await directory.issueStepUp(userId, (await stepUpHash(token))!, STEP_UP_TTL_MS, MAX_LIVE_STEP_UPS);
    return { token, expiresAt };
  } catch (e) {
    console.warn("auth/exchange: minting a step-up failed", e instanceof Error ? e.name : "error");
    return undefined;
  }
};

/** Handle an /auth/* route; undefined means "not an auth route". */
export const handleAuthRoute = async (
  request: Request,
  env: Env,
  url: URL
): Promise<Response | undefined> => {
  const parts = url.pathname.split("/").filter(Boolean);
  if (parts[0] !== "auth") return undefined;
  const apiKey = env.WORKOS_API_KEY;

  if (parts[1] === "exchange" && parts.length === 2 && request.method === "POST") {
    if (!apiKey) return notConfigured();
    const body = await bodyJson<{ code?: string }>(request);
    if (typeof body?.code !== "string") return json({ error: "missing code" }, 400);
    let exchanged: Awaited<ReturnType<typeof exchange>>;
    try {
      exchanged = await exchange(env, apiKey, body.code);
    } catch (e) {
      return authFailed(e);
    }
    // A code comes only from an interactive sign-in: the one moment that
    // earns a step-up ticket (step-up.ts). Sign-in itself never fails on it.
    const stepUp = await mintStepUp(env, exchanged.user.id);
    return json(stepUp ? { ...exchanged, stepUp } : exchanged);
  }

  if (parts[1] === "refresh" && parts.length === 2 && request.method === "POST") {
    if (!apiKey) return notConfigured();
    const body = await bodyJson<{ refreshToken?: string; organizationId?: string }>(request);
    if (typeof body?.refreshToken !== "string") return json({ error: "missing refreshToken" }, 400);
    if (body.organizationId !== undefined && typeof body.organizationId !== "string") {
      return json({ error: "missing refreshToken" }, 400);
    }
    try {
      return json(await refresh(env, apiKey, body.refreshToken, body.organizationId));
    } catch (e) {
      // Identify repeat offenders: a client with a rotated-out session
      // retries every 30s forever and is otherwise anonymous in the tail
      // (the Worker outcome is "ok" — only the 401 body says it failed).
      // The token fingerprint is safe: single-use, and this one is dead.
      console.warn(
        "auth/refresh failed",
        request.headers.get("cf-connecting-ip") ?? "unknown-ip",
        `token:${body.refreshToken.slice(0, 6)}…len${body.refreshToken.length}`,
        e instanceof WorkOsAuthFailed ? e.message : String(e)
      );
      return authFailed(e);
    }
  }

  if (parts[1] === "orgs" && parts.length === 2) {
    if (!apiKey) return notConfigured();
    const token = bearerFromRequest(request);
    const caller = token ? await verifyToken(env, token) : undefined;
    if (!caller) return json({ error: "invalid or missing bearer token" }, 401);
    // A Cloud device's runner token acts for its user inside the sandbox, but
    // never manages memberships (runner-policy.ts covers the post-bearer routes).
    if (caller.kind === "runner") return json({ error: "forbidden" }, 403);
    if (request.method === "GET") {
      try {
        return json({ orgs: await listOrgs(apiKey, caller.userId) });
      } catch (e) {
        return authFailed(e);
      }
    }
    if (request.method === "POST") {
      const body = await bodyJson<{ name?: string }>(request);
      if (typeof body?.name !== "string") return json({ error: "missing name" }, 400);
      const trimmed = body.name.trim();
      if (trimmed.length === 0 || trimmed.length > 80) {
        return json({ error: "name must be 1-80 characters" }, 400);
      }
      const problem = orgNameProblem(trimmed);
      if (problem) return invalidName(problem);
      // Each user creates one org; invitations to others are unaffected. The
      // claim is taken before WorkOS is called, so concurrent requests (a
      // double-submit, a retry) can't both create.
      const directory = userDirectoryStub(env);
      const claim = directory ? await directory.claimOrgCreation(caller.userId) : undefined;
      if (claim && "organizationId" in claim) return orgExists(claim.organizationId);
      const release = async () => {
        if (claim) await directory!.releaseOrgCreation(caller.userId, claim.claimId);
      };
      let created: { organizationId: string };
      try {
        // An admin membership counts as having created one: creators from
        // before the claims, and a create that died after WorkOS made the org.
        const admin = (await listOrgs(apiKey, caller.userId)).find((m) => m.role === "admin");
        if (admin) {
          await release();
          return orgExists(admin.organizationId);
        }
        created = await createOrg(apiKey, caller.userId, trimmed);
      } catch (e) {
        await release();
        return authFailed(e);
      }
      if (claim) {
        // Unrecorded, the claim goes stale and the admin membership still counts.
        await directory!
          .finishOrgCreation(caller.userId, claim.claimId, created.organizationId)
          .catch((e: unknown) => console.error("auth/orgs: recording a creation failed", String(e)));
      }
      return json(created);
    }
  }

  if (parts[1] === "orgs" && parts.length === 4 && parts[3] === "invitations" && request.method === "POST") {
    if (!apiKey) return notConfigured();
    const token = bearerFromRequest(request);
    const caller = token ? await verifyToken(env, token) : undefined;
    if (!caller) return json({ error: "invalid or missing bearer token" }, 401);
    if (caller.kind === "runner") return json({ error: "forbidden" }, 403);
    return invite(request, env, apiKey, caller.userId, parts[2]!);
  }

  // A photo someone chose for themselves: public, by its opaque key.
  if (parts[1] === "avatars" && parts.length === 3 && request.method === "GET") {
    return avatar(env, parts[2]!);
  }

  if (parts[1] === "me" && parts.length === 2 && ["GET", "PATCH", "DELETE"].includes(request.method)) {
    if (!apiKey) return notConfigured();
    const caller = await settingsCaller(request, env);
    if (caller instanceof Response) return caller;
    if (request.method === "GET") return getMe(env, url, apiKey, caller.userId);
    if (request.method === "PATCH") return patchMe(request, env, url, apiKey, caller.userId);
    return deleteMe(request, env, apiKey, caller.userId);
  }

  if (parts[1] === "me" && parts.length === 3 && parts[2] === "photo" && (request.method === "PUT" || request.method === "DELETE")) {
    if (!apiKey) return notConfigured();
    const caller = await settingsCaller(request, env);
    if (caller instanceof Response) return caller;
    return request.method === "PUT"
      ? putPhoto(request, env, url, apiKey, caller.userId)
      : setPhoto(env, url, apiKey, caller.userId, null);
  }

  // An image of the caller's for something of theirs (a project's icon):
  // stored under a key of its own, served as a photo is, and deleted by them.
  if (parts[1] === "me" && parts[2] === "images" && ((parts.length === 3 && request.method === "PUT") || (parts.length === 4 && request.method === "DELETE"))) {
    const caller = await settingsCaller(request, env);
    if (caller instanceof Response) return caller;
    return request.method === "PUT" ? putImage(request, env, url, caller.userId) : deleteImage(env, caller.userId, parts[3]!);
  }

  const orgRoute = parts[1] === "orgs" ? orgSettingsRoute(parts, request.method) : undefined;
  if (orgRoute) {
    if (!apiKey) return notConfigured();
    const caller = await settingsCaller(request, env);
    if (caller instanceof Response) return caller;
    return orgRoute({ request, env, apiKey, userId: caller.userId, orgId: parts[2]!, itemId: parts[4], url });
  }

  if (parts[1] === "cli" && parts[2] === "callback" && request.method === "GET") {
    return cliCallback(url);
  }

  return undefined;
};

// ---------------------------------------------------------------------------
// Account and org settings
// ---------------------------------------------------------------------------
//
//  GET    /auth/me                                   → Me
//  PATCH  /auth/me {name?, title?}                   → Me   (title "" or null clears it)
//  DELETE /auth/me {confirm: <email>, stepUp}        → {ok} (403 step_up_required without a
//                                                     fresh sign-in's ticket, step-up.ts;
//                                                     409 last_admin: the only admin of an
//                                                     org with others; 409 busy)
//  PUT    /auth/me/photo  <image/png|jpeg, ≤1 MB>    → Me   (checked and stripped, image-check.ts)
//  DELETE /auth/me/photo                             → Me
//  GET    /auth/avatars/{key}                        → the photo (public, immutable)
//  PUT    /auth/me/images <image/png|jpeg, ≤1 MB>    → {url}  (an image of yours, e.g. a
//                                                     project's icon, served as a photo is;
//                                                     checked and stripped; 409 too_many_images,
//                                                     413 image_quota, 429 rate_limited)
//  DELETE /auth/me/images/{key}                      → {ok}   (yours only; 404 otherwise)
//  PATCH  /auth/orgs/{org} {name}                    → {organizationId, name}  (admin)
//  GET    /auth/orgs/{org}/members                   → {members}               (member)
//  PATCH  /auth/orgs/{org}/members/{membership} {role} → {ok}                  (admin)
//  DELETE /auth/orgs/{org}/members/{membership}      → {ok}                    (admin)
//  GET    /auth/orgs/{org}/invitations               → {invitations}           (admin)
//  DELETE /auth/orgs/{org}/invitations/{invitation}  → {ok}                    (admin)
//  POST   /auth/orgs/{org}/invitations/{invitation}/resend → {ok}              (admin)
//
// The caller is the token's `sub`; WorkOS user ids never leave the edge (a
// member is named by their membership id). Nothing here logs a name, address
// or id: WorkOS failures log their status and error code only.

/** Longest name a person or org may have, after trimming. */
export const MAX_NAME_LENGTH = 80;

/** How long an expired invitation stays in the list. */
const EXPIRED_INVITATION_DAYS = 30;

/** The verified user, or the 401/403 to answer. Runner tokens never manage
 * accounts or orgs. */
const settingsCaller = async (request: Request, env: Env) => {
  const token = bearerFromRequest(request);
  const caller = token ? await verifyToken(env, token) : undefined;
  if (!caller) return json({ error: "invalid or missing bearer token" }, 401);
  if (caller.kind === "runner") return json({ error: "forbidden" }, 403);
  return caller;
};

/** A failed WorkOS call: 400 `invalid` when WorkOS rejected the input,
 * otherwise 502 `upstream`. Logs the status and code, nothing else. */
const upstream = (e: unknown, route: string): Response => {
  if (e instanceof WorkOsAuthFailed) {
    if (e.status === 422 || e.code === "invalid_request_parameters") return json({ error: "invalid" }, 400);
    console.warn(`auth/${route}: workos failed`, e.status ?? "no status", e.code ?? "no code");
  } else {
    console.warn(`auth/${route}: workos failed`, e instanceof Error ? e.name : "error");
  }
  return json({ error: "upstream" }, 502);
};

const forbidden = (): Response => json({ error: "forbidden" }, 403);
const notFound = (): Response => json({ error: "not_found" }, 404);
const invalid = (): Response => json({ error: "invalid" }, 400);
/** A name names.ts refuses: it would go out in WorkOS's emails. */
const invalidName = (reason: string): Response => json({ error: "invalid_name", reason }, 400);
/** Someone else is changing this org's roles right now: try again. */
const busy = (): Response => json({ error: "busy" }, 409);

/** A trimmed name of 1-80 characters from `{ name }`, or undefined. */
const nameFrom = (body: unknown): string | undefined => {
  const name = (body as { name?: unknown } | undefined)?.name;
  if (typeof name !== "string") return undefined;
  const trimmed = name.trim();
  return trimmed.length >= 1 && trimmed.length <= MAX_NAME_LENGTH ? trimmed : undefined;
};

/** First word, and the rest (empty for a one-word name). */
export const splitName = (name: string): { firstName: string; lastName: string } => {
  const match = /^(\S+)\s+([\s\S]*)$/.exec(name.trim());
  return match ? { firstName: match[1]!, lastName: match[2]!.trim() } : { firstName: name.trim(), lastName: "" };
};

const fullName = (first: string | null, last: string | null): string => [first, last].filter(Boolean).join(" ");

/** The largest photo taken, in bytes (zgx sends 512×512). */
export const MAX_PHOTO_BYTES = 1_000_000;
/** Longest title, after trimming. */
export const MAX_TITLE_LENGTH = 80;
const PHOTO_KEY = /^[0-9a-f]{32}$/;
const PHOTO_CACHE = "public, max-age=31536000, immutable";
const photoObject = (key: string): string => `avatars/${key}`;
const photoUrl = (url: URL, key: string): string => `${url.origin}/auth/avatars/${key}`;

/** What the edge keeps of the caller that WorkOS doesn't. */
interface Own {
  readonly title: string | null;
  readonly photoKey: string | null;
}

const ownProfile = async (env: Env, userId: string): Promise<Own> => {
  const directory = userDirectoryStub(env);
  if (!directory) return { title: null, photoKey: null };
  try {
    const found = (await directory.profiles([userId]))[userId];
    return { title: found?.title ?? null, photoKey: found?.photoKey ?? null };
  } catch (e) {
    console.warn("auth/me: directory read failed", e instanceof Error ? e.name : "error");
    return { title: null, photoKey: null };
  }
};

const me = (u: WireProfile, own: Own, url: URL) => ({
  firstName: u.first_name,
  lastName: u.last_name,
  email: u.email,
  name: fullName(u.first_name, u.last_name),
  title: own.title,
  // Their own photo, else the one their sign-in gave.
  avatar: own.photoKey ? photoUrl(url, own.photoKey) : u.profile_picture_url
});

const getMe = async (env: Env, url: URL, apiKey: string, userId: string): Promise<Response> => {
  let user: WireProfile;
  try {
    user = await getUser(apiKey, userId);
  } catch (e) {
    return upstream(e, "me");
  }
  return json(me(user, await ownProfile(env, userId), url));
};

/** A title from a body's `title`: trimmed, null to clear, undefined if it
 * isn't one (not a string, or longer than 80). */
const titleFrom = (value: unknown): string | null | undefined => {
  if (value === null) return null;
  if (typeof value !== "string") return undefined;
  const trimmed = value.trim();
  if (trimmed.length > MAX_TITLE_LENGTH) return undefined;
  return trimmed === "" ? null : trimmed;
};

/** `write` against the directory's row for the caller, recording them from
 * WorkOS first when their webhook hasn't landed yet. Undefined when there's
 * no directory, or it won't take them (deleted). */
const withRecorded = async <T>(
  env: Env,
  apiKey: string,
  userId: string,
  write: (directory: NonNullable<ReturnType<typeof userDirectoryStub>>) => Promise<T | undefined>
): Promise<T | undefined> => {
  const directory = userDirectoryStub(env);
  if (!directory) return undefined;
  const first = await write(directory);
  if (first !== undefined) return first;
  await directory.upsert(await getUser(apiKey, userId));
  return write(directory);
};

const unavailable = (): Response => json({ error: "unavailable" }, 503);

/** The caller's own photo: stored under a new random key (so the URL is never
 * reused and caches forever), the one it replaces deleted. */
const putPhoto = async (request: Request, env: Env, url: URL, apiKey: string, userId: string): Promise<Response> => {
  const body = await imageBody(request);
  if (body instanceof Response) return body;
  const [type, bytes] = body;
  const key = newPhotoKey();
  await env.BLOBS.put(photoObject(key), bytes, { httpMetadata: { contentType: type, cacheControl: PHOTO_CACHE } });
  const answer = await setPhoto(env, url, apiKey, userId, key);
  // Not taken: the new object goes too.
  if (answer.status !== 200) await env.BLOBS.delete(photoObject(key));
  return answer;
};

/** The PNG or JPEG of at most `MAX_PHOTO_BYTES` sent with `request`, checked
 * and rebuilt without its metadata (image-check.ts), as `[type, bytes]`, or
 * the answer refusing it. */
const imageBody = async (request: Request): Promise<[string, Uint8Array] | Response> => {
  const type = request.headers.get("content-type")?.split(";")[0]?.trim().toLowerCase() ?? "";
  if (type !== "image/png" && type !== "image/jpeg") return invalid();
  if (Number(request.headers.get("content-length") ?? "0") > MAX_PHOTO_BYTES) return json({ error: "too_large" }, 413);
  const bytes = new Uint8Array(await request.arrayBuffer());
  if (bytes.length > MAX_PHOTO_BYTES) return json({ error: "too_large" }, 413);
  const clean = cleanImage(type, bytes);
  if (!clean) return invalid();
  return [type, clean];
};

const newPhotoKey = (): string => [...crypto.getRandomValues(new Uint8Array(16))].map((b) => b.toString(16).padStart(2, "0")).join("");

/** An image of the caller's, not their photo (a project's icon): kept beside
 * the photos under a new random key, marked as theirs, its public URL
 * answered. What it's for is the caller's to record (the project's row). */
const putImage = async (request: Request, env: Env, url: URL, userId: string): Promise<Response> => {
  const body = await imageBody(request);
  if (body instanceof Response) return body;
  const [type, bytes] = body;
  const key = newPhotoKey();
  // Counted against the owner's quota first (user-directory.ts `reserveImage`),
  // so it's theirs to account for, and goes with their account.
  const directory = userDirectoryStub(env);
  if (!directory) return unavailable();
  switch (await directory.reserveImage(userId, key, bytes.length)) {
    case "ok":
      break;
    case "too_many":
      return json({ error: "too_many_images" }, 409);
    case "too_large":
      return json({ error: "image_quota" }, 413);
    case "rate_limited":
      return json({ error: "rate_limited" }, 429);
    case "deleted_user":
      return forbidden();
  }
  try {
    await env.BLOBS.put(photoObject(key), bytes, {
      httpMetadata: { contentType: type, cacheControl: PHOTO_CACHE },
      customMetadata: { owner: userId, kind: "image" }
    });
  } catch (e) {
    await directory.dropImage(userId, key);
    throw e;
  }
  return json({ key, url: photoUrl(url, key) });
};

/** One of the caller's images gone (one replaced). Only an image they put:
 * anyone else's, or a photo, answers 404 as a missing one does. */
const deleteImage = async (env: Env, userId: string, key: string): Promise<Response> => {
  if (!PHOTO_KEY.test(key)) return notFound();
  const object = await env.BLOBS.head(photoObject(key));
  if (!object || object.customMetadata?.owner !== userId || object.customMetadata?.kind !== "image") return notFound();
  await env.BLOBS.delete(photoObject(key));
  await userDirectoryStub(env)?.dropImage(userId, key);
  return json({ ok: true });
};

/** Point the caller's photo at `key` (null: none), deleting the one replaced. */
const setPhoto = async (env: Env, url: URL, apiKey: string, userId: string, key: string | null): Promise<Response> => {
  let user: WireProfile;
  let set: { readonly replaced: string | null } | undefined;
  try {
    user = await getUser(apiKey, userId);
    set = await withRecorded(env, apiKey, userId, (d) => d.setPhoto(userId, key));
  } catch (e) {
    return upstream(e, "me/photo");
  }
  if (!set) return unavailable();
  if (set.replaced && set.replaced !== key) await env.BLOBS.delete(photoObject(set.replaced));
  return json(me(user, { ...(await ownProfile(env, userId)), photoKey: key }, url));
};

/** A photo by its key, as anyone may see it. */
const avatar = async (env: Env, key: string): Promise<Response> => {
  if (!PHOTO_KEY.test(key)) return notFound();
  const object = await env.BLOBS.get(photoObject(key));
  if (!object) return notFound();
  return new Response(object.body, {
    headers: {
      "content-type": object.httpMetadata?.contentType ?? "image/png",
      "cache-control": PHOTO_CACHE,
      "x-content-type-options": "nosniff",
      // Only ever a picture: nothing it holds runs, opened directly or not.
      "content-security-policy": "default-src 'none'; sandbox"
    }
  });
};

/** Whether `confirm` is `email` typed back (case and spaces aside). */
const confirms = (confirm: string, email: string | null | undefined): boolean =>
  !!email && confirm.trim().toLowerCase() === email.trim().toLowerCase();

/** Delete the caller's account, once they've typed their address. Refused
 * while they're the only admin of an org others are in (someone must be
 * left to run it). An org they're the only member of goes with them (no one
 * is left to run it, or to be let into it by its invitations). WorkOS takes
 * their memberships and sessions, so every device's next refresh is refused
 * and it signs out; the directory tombstones them now rather than at the
 * webhook, their username retires for good, and their photo goes.
 *
 * Safe to retry: each step is one that's already done counts as done, and a
 * retry after WorkOS has deleted them (an answer lost on the way back)
 * finishes the edge's part. */
const deleteMe = async (request: Request, env: Env, apiKey: string, userId: string): Promise<Response> => {
  const body = await bodyJson<{ confirm?: unknown; stepUp?: unknown }>(request);
  const confirm = body?.confirm;
  // Nothing is asked of WorkOS without the address typed.
  if (typeof confirm !== "string" || confirm.trim() === "") return invalid();
  let user: WireProfile;
  try {
    user = await getUser(apiKey, userId);
  } catch (e) {
    if (e instanceof WorkOsAuthFailed && e.status === 404) return finishDeleted(env, userId, confirm);
    return upstream(e, "me");
  }
  if (!confirms(confirm, user.email)) return invalid();
  // A bearer alone never deletes an account: a fresh sign-in's ticket
  // (step-up.ts) does. Checked here, used up by the tombstone.
  const directory = userDirectoryStub(env);
  if (!directory) return unavailable();
  if (!(await directory.hasStepUp(userId, await stepUpHash(body?.stepUp)))) return stepUpRequired();
  let orgs: Awaited<ReturnType<typeof listOrgs>>;
  try {
    orgs = await listOrgs(apiKey, userId);
  } catch (e) {
    return upstream(e, "me");
  }
  // Where they're an admin, their deletion takes the org's role lock: two
  // admins deleting themselves at once can't both see the other stay.
  const locked = await lockOrgs(env, orgs.filter((o) => o.role === "admin").map((o) => o.organizationId));
  if (!locked) return busy();
  try {
    const alone: string[] = [];
    try {
      for (const org of orgs) {
        const others = (await listOrgMembers(apiKey, org.organizationId)).filter((m) => m.userId !== userId);
        if (others.length === 0) {
          alone.push(org.organizationId);
        } else if (org.role === "admin" && !others.some((m) => m.role === "admin")) {
          return json({ error: "last_admin", organization: org.name }, 409);
        }
      }
    } catch (e) {
      return upstream(e, "me");
    }
    let photoKey: string | null = null;
    try {
      photoKey = await directory.photoKey(userId);
    } catch {
      // (Its object stays behind: unreachable, its key unknown.)
    }
    try {
      // Their own orgs first: once the user is gone, which they were is too.
      for (const organizationId of alone) await deleteOrg(apiKey, organizationId);
      await deleteUser(apiKey, userId);
    } catch (e) {
      return upstream(e, "me");
    }
    await tombstone(env, user, photoKey);
    return json({ ok: true });
  } finally {
    await locked.release();
  }
};

/** Take the role locks of `orgIds` (sorted, so two callers can't each hold
 * half), or none: undefined when one is held by someone else. */
const lockOrgs = async (env: Env, orgIds: readonly string[]): Promise<{ release: () => Promise<void> } | undefined> => {
  const directory = userDirectoryStub(env);
  const noop = { release: async () => {} };
  if (!directory || orgIds.length === 0) return noop;
  const holder = crypto.randomUUID();
  const taken: string[] = [];
  const release = async () => {
    for (const orgId of taken) await directory.unlockOrg(orgId, holder).catch(() => undefined);
  };
  for (const orgId of [...new Set(orgIds)].sort()) {
    if (!(await directory.lockOrg(orgId, holder))) {
      await release();
      return undefined;
    }
    taken.push(orgId);
  }
  return { release };
};

/** The directory's part of a deletion: tombstoned (username retired, card
 * forgotten, step-up tickets gone) and the photo's and images' objects gone.
 * Failures are logged and left: the `user.deleted` webhook lands the
 * tombstone too, and a retry finishes it. */
const tombstone = async (env: Env, user: WireProfile, photoKey: string | null): Promise<void> => {
  const directory = userDirectoryStub(env);
  let images: string[] = [];
  if (directory) {
    try {
      images = await directory.takeImages(user.id);
      const username = await directory.remove(user, new Date().toISOString());
      if (username) await forgetCard(username);
    } catch (e) {
      console.warn("auth/me: directory tombstone failed", e instanceof Error ? e.name : "error");
    }
  }
  await deleteUserBlobs(env, user.id, photoKey, images);
};

/** A delete asked again after WorkOS already deleted the caller: the
 * directory's part finished if it hadn't been (their address, still on
 * record, typed back), else nothing left to do. */
const finishDeleted = async (env: Env, userId: string, confirm: string): Promise<Response> => {
  const directory = userDirectoryStub(env);
  if (!directory) return json({ ok: true });
  let live: MemberProfile | undefined;
  let photoKey: string | null = null;
  try {
    live = (await directory.profiles([userId]))[userId];
    photoKey = await directory.photoKey(userId);
  } catch (e) {
    console.warn("auth/me: directory read failed", e instanceof Error ? e.name : "error");
    return unavailable();
  }
  if (live) {
    if (live.email && !confirms(confirm, live.email)) return invalid();
    const now = new Date().toISOString();
    await tombstone(
      env,
      { id: userId, email: "", first_name: null, last_name: null, profile_picture_url: null, created_at: now, updated_at: now },
      photoKey
    );
  } else {
    let images: string[] = [];
    try {
      images = await directory.takeImages(userId);
    } catch {
      // (The scan below still finds them.)
    }
    await deleteUserBlobs(env, userId, photoKey, images);
  }
  return json({ ok: true });
};

/** Rename the caller in WorkOS, then in the directory right away (the same
 * upsert the `user.updated` webhook makes, so the later delivery is a no-op)
 * so their member card follows without waiting for it; and set their title,
 * which only the directory keeps. Either, or both. */
const patchMe = async (request: Request, env: Env, url: URL, apiKey: string, userId: string): Promise<Response> => {
  const body = await bodyJson<Record<string, unknown>>(request);
  const hasName = !!body && typeof body === "object" && "name" in body;
  const hasTitle = !!body && typeof body === "object" && "title" in body;
  if (!hasName && !hasTitle) return invalid();
  const name = hasName ? nameFrom(body) : undefined;
  if (hasName && !name) return invalid();
  const problem = name ? nameProblem(name) : undefined;
  if (problem) return invalidName(problem);
  const title = hasTitle ? titleFrom(body!.title) : undefined;
  if (hasTitle && title === undefined) return invalid();
  let user: WireProfile;
  try {
    if (name) {
      const { firstName, lastName } = splitName(name);
      user = await updateUserName(apiKey, userId, firstName, lastName);
    } else {
      user = await getUser(apiKey, userId);
    }
    if (hasTitle) {
      const set = await withRecorded(env, apiKey, userId, async (d) => ((await d.setTitle(userId, title ?? null)) ? true : undefined));
      if (!set) return unavailable();
    }
  } catch (e) {
    return upstream(e, "me");
  }
  if (!name) return json(me(user, await ownProfile(env, userId), url));
  const directory = userDirectoryStub(env);
  if (directory) {
    try {
      if (await directory.upsert(user)) {
        const username = await directory.username(userId);
        if (username) await forgetCard(username);
      }
    } catch (e) {
      // The webhook still lands it.
      console.warn("auth/me: directory update failed", e instanceof Error ? e.name : "error");
    }
  }
  return json(me(user, await ownProfile(env, userId), url));
};

interface OrgContext {
  readonly request: Request;
  readonly env: Env;
  readonly apiKey: string;
  readonly userId: string;
  readonly orgId: string;
  /** The membership or invitation id, on routes that name one. */
  readonly itemId: string | undefined;  /** The request's URL: where photos are served from. */
  readonly url: URL;
}

type OrgHandler = (ctx: OrgContext) => Promise<Response>;

/** The settings route for an /auth/orgs/{org}/… path, if it is one. Leaves
 * `GET|POST /auth/orgs` and `POST /auth/orgs/{org}/invitations` alone. */
const orgSettingsRoute = (parts: readonly string[], method: string): OrgHandler | undefined => {
  const [, , , kind, , action] = parts;
  if (parts.length === 3 && method === "PATCH") return patchOrg;
  if (kind === "members") {
    if (parts.length === 4 && method === "GET") return listMembers;
    if (parts.length === 5 && method === "PATCH") return patchMember;
    if (parts.length === 5 && method === "DELETE") return removeMember;
  }
  if (kind === "invitations") {
    if (parts.length === 4 && method === "GET") return listOrgInvitations;
    if (parts.length === 5 && method === "DELETE") return revokeOrgInvitation;
    if (parts.length === 6 && action === "resend" && method === "POST") return resendOrgInvitation;
  }
  return undefined;
};

/** The caller's membership in the org (admin only when `admin`), or the
 * 403 to answer. A failed membership read maps like the other org routes. */
const gate = async (ctx: OrgContext, admin: boolean): Promise<Response | undefined> => {
  try {
    const membership = (await listOrgs(ctx.apiKey, ctx.userId)).find((m) => m.organizationId === ctx.orgId);
    if (!membership || (admin && membership.role !== "admin")) return forbidden();
    return undefined;
  } catch (e) {
    return authFailed(e);
  }
};

const patchOrg: OrgHandler = async (ctx) => {
  const name = nameFrom(await bodyJson<unknown>(ctx.request));
  if (!name) return invalid();
  const problem = orgNameProblem(name);
  if (problem) return invalidName(problem);
  const refused = await gate(ctx, true);
  if (refused) return refused;
  try {
    return json(await renameOrg(ctx.apiKey, ctx.orgId, name));
  } catch (e) {
    return upstream(e, "orgs");
  }
};

/** Members with their profiles: the directory's where it has them (one DO
 * call), WorkOS's for the rest. */
const listMembers: OrgHandler = async (ctx) => {
  const refused = await gate(ctx, false);
  if (refused) return refused;
  let members: OrgMember[];
  try {
    members = await listOrgMembers(ctx.apiKey, ctx.orgId);
  } catch (e) {
    return upstream(e, "members");
  }
  const directory = userDirectoryStub(ctx.env);
  let known: Record<string, MemberProfile> = {};
  if (directory) {
    try {
      known = await directory.profiles(members.map((m) => m.userId));
    } catch (e) {
      console.warn("auth/members: directory read failed", e instanceof Error ? e.name : "error");
    }
  }
  const missing = [...new Set(members.map((m) => m.userId).filter((id) => !known[id]))];
  const fetched = await Promise.allSettled(missing.map((id) => getUser(ctx.apiKey, id)));
  const fromWorkOs = new Map<string, MemberProfile>();
  fetched.forEach((result, i) => {
    if (result.status === "fulfilled") {
      const u = result.value;
      fromWorkOs.set(missing[i]!, {
        email: u.email,
        firstName: u.first_name,
        lastName: u.last_name,
        profilePictureUrl: u.profile_picture_url,
        username: null,
        title: null,
        photoKey: null
      });
    }
  });
  if (fetched.some((r) => r.status === "rejected")) console.warn("auth/members: some profiles unavailable");
  return json({
    // Field by field: no WorkOS user id goes out.
    members: members.map((m) => {
      const profile = known[m.userId] ?? fromWorkOs.get(m.userId);
      return {
        id: m.membershipId,
        name: (profile && fullName(profile.firstName, profile.lastName)) || null,
        email: profile?.email ?? null,
        role: m.role,
        you: m.userId === ctx.userId,
        avatar: profile?.photoKey ? photoUrl(ctx.url, profile.photoKey) : (profile?.profilePictureUrl ?? null),
        title: profile?.title ?? null,
        username: profile?.username ?? null,
        joinedAt: m.createdAt
      };
    })
  });
};

/** The org's membership named by the path, or the 404/400 to answer.
 * Someone else's: no admin changes their own membership. */
const otherMembership = async (ctx: OrgContext): Promise<Response | undefined> => {
  let membership: MembershipRef | undefined;
  try {
    membership = await getMembership(ctx.apiKey, ctx.itemId!);
  } catch (e) {
    return upstream(e, "members");
  }
  if (!membership || membership.organizationId !== ctx.orgId) return notFound();
  if (membership.userId === ctx.userId) return json({ error: "self" }, 400);
  return undefined;
};

/**
 * Run a role change or removal under the org's role lock, re-checking that
 * the caller is still an admin inside it. Two admins demoting (or removing)
 * each other at once would each pass a check made before the other's change
 * and leave the org with no admin. Under the lock, the second one finds it's
 * no longer an admin (403). The answer is 409 `busy` while another change
 * holds the lock. `last_admin` (409) also guards the change itself: it never
 * leaves the org without an active admin.
 */
const adminChange = async (ctx: OrgContext, demotes: boolean, change: () => Promise<void>): Promise<Response> => {
  const locked = await lockOrgs(ctx.env, [ctx.orgId]);
  if (!locked) return busy();
  try {
    const refused = (await gate(ctx, true)) ?? (await otherMembership(ctx));
    if (refused) return refused;
    if (demotes) {
      let members: OrgMember[];
      try {
        members = await listOrgMembers(ctx.apiKey, ctx.orgId);
      } catch (e) {
        return upstream(e, "members");
      }
      const left = members.filter((m) => m.role === "admin" && m.membershipId !== ctx.itemId);
      if (left.length === 0) return json({ error: "last_admin" }, 409);
    }
    try {
      await change();
    } catch (e) {
      if (e instanceof WorkOsAuthFailed && e.code === "invalid_role") return json({ error: "invalid_role" }, 400);
      return upstream(e, "members");
    }
    return json({ ok: true });
  } finally {
    await locked.release();
  }
};

const removeMember: OrgHandler = (ctx) => adminChange(ctx, true, () => removeMembership(ctx.apiKey, ctx.itemId!));

const ROLES = new Set(["admin", "member"]);

const patchMember: OrgHandler = async (ctx) => {
  const role = (await bodyJson<{ role?: unknown }>(ctx.request))?.role;
  if (typeof role !== "string" || !ROLES.has(role)) return invalid();
  return adminChange(ctx, role !== "admin", () => setMembershipRole(ctx.apiKey, ctx.itemId!, role));
};

/** Pending invitations, and those expired in the last 30 days, newest first.
 * A pending one past its expiry reads as expired. */
const listOrgInvitations: OrgHandler = async (ctx) => {
  const refused = await gate(ctx, true);
  if (refused) return refused;
  let all: WireInvitation[];
  try {
    all = await listInvitations(ctx.apiKey, ctx.orgId);
  } catch (e) {
    return upstream(e, "invitations");
  }
  const now = Date.now();
  const cutoff = now - EXPIRED_INVITATION_DAYS * 86_400_000;
  const invitations = all.flatMap((i) => {
    if (i.organization_id !== ctx.orgId) return [];
    const expiresAt = Date.parse(i.expires_at);
    const state = i.state === "pending" && expiresAt <= now ? "expired" : i.state;
    if (state !== "pending" && !(state === "expired" && expiresAt >= cutoff)) return [];
    return [{ id: i.id, email: i.email, state: state as "pending" | "expired", expiresAt: i.expires_at, createdAt: i.created_at }];
  });
  return json({ invitations });
};

/** The org's invitation named by the path, or the 404 to answer. */
const orgInvitation = async (ctx: OrgContext): Promise<WireInvitation | Response> => {
  try {
    const invitation = await getInvitation(ctx.apiKey, ctx.itemId!);
    return invitation && invitation.organization_id === ctx.orgId ? invitation : notFound();
  } catch (e) {
    return upstream(e, "invitations");
  }
};

/** Revoke a pending (or expired) invitation; one already accepted or revoked
 * has nothing left to revoke. */
const revokeOrgInvitation: OrgHandler = async (ctx) => {
  const refused = await gate(ctx, true);
  if (refused) return refused;
  const invitation = await orgInvitation(ctx);
  if (invitation instanceof Response) return invitation;
  if (invitation.state === "pending" || invitation.state === "expired") {
    try {
      await revokeInvitation(ctx.apiKey, invitation.id);
    } catch (e) {
      return upstream(e, "invitations");
    }
  }
  return json({ ok: true });
};

/** Send a pending or expired invitation's email again, against the same
 * hourly quota as new invitations. Where WorkOS can't resend, the old one is
 * revoked and a new one goes to the same address. */
const resendOrgInvitation: OrgHandler = async (ctx) => {
  const refused = await gate(ctx, true);
  if (refused) return refused;
  const invitation = await orgInvitation(ctx);
  if (invitation instanceof Response) return invitation;
  if (invitation.state !== "pending" && invitation.state !== "expired") return json({ error: "not_pending" }, 409);
  const directory = userDirectoryStub(ctx.env);
  if (directory && !(await directory.takeInviteQuota(ctx.userId, 1))) return json({ error: "rate_limited" }, 429);
  try {
    if ((await resendInvitation(ctx.apiKey, invitation.id)) === "sent") return json({ ok: true });
    if (invitation.state === "pending") await revokeInvitation(ctx.apiKey, invitation.id);
  } catch (e) {
    return upstream(e, "invitations");
  }
  const outcome = await sendInvitation(ctx.apiKey, {
    email: invitation.email,
    organizationId: ctx.orgId,
    inviterUserId: ctx.userId
  });
  if (outcome === "sent") return json({ ok: true });
  if (outcome === "exists") return json({ error: "exists" }, 409);
  return json({ error: "upstream" }, 502);
};

// ---------------------------------------------------------------------------
// Invitations
// ---------------------------------------------------------------------------

/** Addresses one request may invite, after dedupe. */
export const MAX_INVITES_PER_REQUEST = 10;

const EMAIL_RE = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;

/** The body's addresses, trimmed and deduped case-insensitively (first
 * spelling kept); undefined unless it's `{ emails: string[] }` with 1 to
 * `MAX_INVITES_PER_REQUEST`. */
const inviteEmails = (body: unknown): string[] | undefined => {
  const emails = (body as { emails?: unknown } | undefined)?.emails;
  if (!Array.isArray(emails) || emails.length > 100 || !emails.every((e) => typeof e === "string")) return undefined;
  const seen = new Map<string, string>();
  for (const raw of emails as string[]) {
    const email = raw.trim();
    if (email && !seen.has(email.toLowerCase())) seen.set(email.toLowerCase(), email);
  }
  return seen.size >= 1 && seen.size <= MAX_INVITES_PER_REQUEST ? [...seen.values()] : undefined;
};

/** An admin of `organizationId` invites addresses as members. One address
 * failing doesn't fail the rest. Addresses are never logged. */
const invite = async (
  request: Request,
  env: Env,
  apiKey: string,
  userId: string,
  organizationId: string
): Promise<Response> => {
  const emails = inviteEmails(await bodyJson<unknown>(request));
  if (!emails) return json({ error: "invalid" }, 400);
  try {
    const memberships = await listOrgs(apiKey, userId);
    if (!memberships.some((m) => m.organizationId === organizationId && m.role === "admin")) {
      return json({ error: "forbidden" }, 403);
    }
    // (The org's name goes out in WorkOS's email. names.ts checked it when it
    // was set; names from before that are grandfathered.)
  } catch (e) {
    return authFailed(e);
  }
  const directory = userDirectoryStub(env);
  if (directory && !(await directory.takeInviteQuota(userId, emails.length))) {
    return json({ error: "rate_limited" }, 429);
  }
  const outcomes = await Promise.all(
    emails.map((email) =>
      EMAIL_RE.test(email) && email.length <= 254
        ? sendInvitation(apiKey, { email, organizationId, inviterUserId: userId })
        : ("invalid" as const)
    )
  );
  return json({
    sent: emails.filter((_, i) => outcomes[i] === "sent"),
    failed: emails.flatMap((email, i) => (outcomes[i] === "sent" ? [] : [{ email, error: outcomes[i] }]))
  });
};

// ---------------------------------------------------------------------------
// Headless sign-in callback
// ---------------------------------------------------------------------------

/** Query params land verbatim in the page — escape them. (WorkOS codes/states
 * are URL-safe tokens, but this URL accepts anything.) */
export const escapeHtml = (s: string): string =>
  s
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#39;");

const cliPage = (body: string): string => `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8" />
<meta name="viewport" content="width=device-width, initial-scale=1" />
<meta name="robots" content="noindex" />
<title>Zeron — sign in</title>
<style>
  body { margin: 0; min-height: 100vh; display: grid; place-items: center;
         background: #0a0a0a; color: #ededed;
         font: 15px/1.6 ui-sans-serif, system-ui, sans-serif; }
  main { max-width: 34rem; padding: 2rem; text-align: center; }
  h1 { font-size: 1.05rem; font-weight: 600; margin: 0 0 0.75rem; }
  p { color: #a1a1a1; margin: 0.25rem 0; }
  code#paste { display: block; margin: 1.25rem 0 0.75rem; padding: 0.9rem 1rem;
         background: #171717; border: 1px solid #2e2e2e; border-radius: 8px;
         font: 13px/1.5 ui-monospace, monospace; word-break: break-all;
         user-select: all; cursor: pointer; }
  button { margin-top: 0.25rem; padding: 0.45rem 1rem; border-radius: 8px;
         border: 1px solid #2e2e2e; background: #ededed; color: #0a0a0a;
         font: 500 13px ui-sans-serif, system-ui, sans-serif; cursor: pointer; }
</style>
</head>
<body><main>${body}</main></body>
</html>`;

const html = (body: string, status = 200): Response =>
  new Response(body, { status, headers: { "content-type": "text/html; charset=utf-8" } });

/**
 * The hosted OAuth callback for headless (paste-code) sign-in. Registered as a
 * WorkOS redirect URI; it does NOT exchange the code — it renders `state.code`
 * for the user to paste into the device that started the flow (`zeron login`),
 * where the exchange runs so the tokens land on that machine. The state half
 * must match the pending sign-in there, so the paste is CSRF-checked at the
 * same point the loopback flow is.
 */
const cliCallback = (url: URL): Response => {
  const code = url.searchParams.get("code");
  const state = url.searchParams.get("state");
  const denied = url.searchParams.get("error");
  if (denied || !code || !state) {
    const detail = denied
      ? `Sign-in was not completed (${escapeHtml(denied)}).`
      : "This link is missing its sign-in code.";
    return html(
      cliPage(`<h1>Sign-in failed</h1><p>${detail}</p><p>Start again from your terminal.</p>`),
      400
    );
  }
  const paste = `${escapeHtml(state)}.${escapeHtml(code)}`;
  return html(
    cliPage(
      `<h1>Almost there</h1>
<p>Paste this code into the terminal that asked for it:</p>
<code id="paste">${paste}</code>
<button onclick="navigator.clipboard.writeText(document.getElementById('paste').textContent).then(()=>{this.textContent='Copied'})">Copy code</button>
<p style="margin-top:1rem;font-size:13px">This code expires in a few minutes and only works on the device that started sign-in.</p>`
    )
  );
};
