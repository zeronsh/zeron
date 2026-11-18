/**
 * Staff roles: the edge's own record of who is a Zeron admin
 * (`user_roles` in user-directory.ts). A WorkOS token only says who someone
 * is. WorkOS roles are per organization and everyone is `admin` of their own
 * personal organization, so nothing here ever reads a WorkOS role or claim.
 *
 * Operator routes (ADMIN_TOKEN, before the bearer gate):
 *   GET  /admin/roles[?all=1]                      → { roles: RoleRow[] }  (revoked ones with all=1)
 *   POST /admin/roles/grant  { userId, role, by, note? } → { granted: true | false (held already) }
 *   POST /admin/roles/revoke { userId, role, by }        → { revoked: true | false (didn't hold it) }
 *
 * `by` names the operator (ADMIN_TOKEN is shared, so the token can't). Only a
 * user the directory has, live, can be granted a role: a mistyped id is a
 * 404, not a phantom admin. Revoking takes effect on the next request; a
 * grant shortens the account's web sessions at once (web-session.ts).
 */
import { adminAuthorized } from "./cloud/billing";
import type { Env } from "./env";
import { json, jsonError, readJsonBody } from "./http";
import { ROLES, isRole, userDirectoryStub, type Role } from "./user-directory";

export { ROLES, isRole, type Role };

/** Whether `userId` holds `role` at the edge, now. False when the directory
 * isn't set up. */
export const hasRole = async (env: Env, userId: string, role: Role): Promise<boolean> => {
  const directory = userDirectoryStub(env);
  return directory ? directory.hasRole(userId, role) : false;
};

const USER_ID_RE = /^user_[A-Za-z0-9]{1,64}$/;
const MAX_BODY = 4096;

/** A trimmed string of 1–`max` characters, or undefined. */
const text = (value: unknown, max: number): string | undefined => {
  if (typeof value !== "string") return undefined;
  const trimmed = value.trim();
  return trimmed.length >= 1 && trimmed.length <= max ? trimmed : undefined;
};

/** `/admin/roles*`, or undefined when it isn't one. */
export const handleRoleAdminRoute = async (request: Request, env: Env, url: URL): Promise<Response | undefined> => {
  const action = url.pathname === "/admin/roles" ? "" : url.pathname.startsWith("/admin/roles/") ? url.pathname.slice(13) : undefined;
  if (action === undefined) return undefined;
  if (!env.ADMIN_TOKEN) return json({ error: "not_found" }, 404);
  if (!(await adminAuthorized(request, env.ADMIN_TOKEN))) return jsonError(401, "unauthenticated", "Admin token required.");
  const directory = userDirectoryStub(env);
  if (!directory) return jsonError(503, "unavailable", "The user directory isn't set up on this server.");
  if (action === "") {
    if (request.method !== "GET") return jsonError(405, "method_not_allowed", "GET only.");
    return json({ roles: await directory.listRoles(url.searchParams.get("all") === "1") });
  }
  if (action !== "grant" && action !== "revoke") return jsonError(404, "not_found", "Not found.");
  if (request.method !== "POST") return jsonError(405, "method_not_allowed", "POST only.");
  const body = (await readJsonBody(request, MAX_BODY)) as Record<string, unknown> | undefined;
  const userId = body?.userId;
  const role = body?.role;
  const by = text(body?.by, 80);
  const note = body?.note === undefined || body.note === null ? null : text(body.note, 500);
  if (typeof userId !== "string" || !USER_ID_RE.test(userId) || !isRole(role) || !by || note === undefined) {
    return jsonError(400, "bad_request", `Expected {userId, role (${ROLES.join(", ")}), by (who you are)${action === "grant" ? ", note?" : ""}}.`);
  }
  if (action === "revoke") {
    const revoked = await directory.revokeRole(userId, role, by);
    console.log("roles: revoked", role, revoked ? "held" : "not held");
    return json({ revoked });
  }
  switch (await directory.grantRole(userId, role, by, note)) {
    case "granted":
      console.log("roles: granted", role);
      return json({ granted: true });
    case "held":
      return json({ granted: false });
    case "unknown_user":
      return jsonError(404, "unknown_user", "That user isn't in the directory.");
    case "deleted_user":
      return jsonError(409, "deleted_user", "That account has been deleted.");
  }
};
