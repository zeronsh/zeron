/**
 * Minimal WorkOS User Management REST client — the fetch-based port of the
 * old apps/server `WorkOsAuth` service (which used @workos-inc/node; the
 * Worker keeps it SDK-free). This is the one place that holds the WorkOS
 * **API key** (a Worker secret). Device backends build the public authorize
 * URL themselves and delegate the secret-bearing steps here, so the key never
 * lands on a device.
 *
 * Without WORKOS_API_KEY configured the routes answer 501; in dev mode
 * backends use their userId as the bearer and never call these.
 */
import type { Env } from "./env";

const API = "https://api.workos.com";

/** Thrown for rejected WorkOS calls; routes map it to 401 (same as the old
 * server's WorkOsAuthFailed), or, for the account/org settings calls, by
 * `status` and `code` (the WorkOS HTTP status and error code). */
export class WorkOsAuthFailed extends Error {
  constructor(
    message: string,
    readonly status?: number,
    readonly code?: string
  ) {
    super(message);
  }
}

export interface ExchangeResult {
  readonly user: {
    readonly id: string;
    readonly email: string;
    readonly firstName: string | null;
    readonly lastName: string | null;
  };
  readonly accessToken: string;
  readonly refreshToken: string;
}

export interface RefreshResult {
  readonly accessToken: string;
  readonly refreshToken: string;
}

export interface OrgMembership {
  readonly id: string;
  readonly organizationId: string;
  readonly name: string;
  /** Role slug, e.g. `admin`; null when WorkOS names none. */
  readonly role: string | null;
}

interface WireUser {
  id: string;
  email: string;
  first_name: string | null;
  last_name: string | null;
}

interface WireAuthResponse {
  user: WireUser;
  access_token: string;
  refresh_token: string;
}

interface WireMembership {
  id: string;
  organization_id: string;
  organization_name?: string | null;
  role?: { slug?: string | null } | null;
}

const failed = async (res: Response): Promise<never> => {
  let message = "authentication failed";
  let code: string | undefined;
  try {
    const body = (await res.json()) as { message?: string; error_description?: string; error?: string; code?: unknown };
    message = body.message ?? body.error_description ?? body.error ?? message;
    if (typeof body.code === "string") code = body.code;
  } catch {
    /* non-JSON error body */
  }
  throw new WorkOsAuthFailed(message, res.status, code);
};

const post = async (apiKey: string, path: string, body: unknown): Promise<Response> =>
  fetch(`${API}${path}`, {
    method: "POST",
    headers: {
      authorization: `Bearer ${apiKey}`,
      "content-type": "application/json"
    },
    body: JSON.stringify(body)
  });

/** `authenticateWithCode`: WorkOS code → tokens + user. */
export const exchange = async (env: Env, apiKey: string, code: string): Promise<ExchangeResult> => {
  const res = await fetch(`${API}/user_management/authenticate`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      client_id: env.WORKOS_CLIENT_ID,
      client_secret: apiKey,
      grant_type: "authorization_code",
      code
    })
  });
  if (!res.ok) return failed(res);
  const r = (await res.json()) as WireAuthResponse;
  return {
    user: {
      id: r.user.id,
      email: r.user.email,
      firstName: r.user.first_name,
      lastName: r.user.last_name
    },
    accessToken: r.access_token,
    refreshToken: r.refresh_token
  };
};

/** `authenticateWithRefreshToken`; passing `organizationId` scopes the session
 * to that org (the next access token carries `org_id`). */
export const refresh = async (
  env: Env,
  apiKey: string,
  refreshToken: string,
  organizationId?: string
): Promise<RefreshResult> => {
  const res = await fetch(`${API}/user_management/authenticate`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      client_id: env.WORKOS_CLIENT_ID,
      client_secret: apiKey,
      grant_type: "refresh_token",
      refresh_token: refreshToken,
      ...(organizationId ? { organization_id: organizationId } : {})
    })
  });
  if (!res.ok) return failed(res);
  const r = (await res.json()) as WireAuthResponse;
  return { accessToken: r.access_token, refreshToken: r.refresh_token };
};

/** The user's active organization memberships. */
export const listOrgs = async (apiKey: string, userId: string): Promise<OrgMembership[]> => {
  const params = new URLSearchParams({ user_id: userId, statuses: "active", limit: "100" });
  const res = await fetch(`${API}/user_management/organization_memberships?${params}`, {
    headers: { authorization: `Bearer ${apiKey}` }
  });
  if (!res.ok) return failed(res);
  const r = (await res.json()) as { data: WireMembership[] };
  return r.data.map((m) => ({
    id: m.id,
    organizationId: m.organization_id,
    name: m.organization_name ?? m.organization_id,
    role: m.role?.slug ?? null
  }));
};

/** Create an organization and make the user its first (admin) member. */
export const createOrg = async (
  apiKey: string,
  userId: string,
  name: string
): Promise<{ organizationId: string }> => {
  const orgRes = await post(apiKey, "/organizations", { name });
  if (!orgRes.ok) return failed(orgRes);
  const org = (await orgRes.json()) as { id: string };
  // The creator administers their workspace. Role slugs are per-environment
  // config, so fall back to the default role if "admin" doesn't exist rather
  // than failing the whole onboarding.
  const withRole = await post(apiKey, "/user_management/organization_memberships", {
    user_id: userId,
    organization_id: org.id,
    role_slug: "admin"
  });
  if (!withRole.ok) {
    const fallback = await post(apiKey, "/user_management/organization_memberships", {
      user_id: userId,
      organization_id: org.id
    });
    if (!fallback.ok) return failed(fallback);
  }
  return { organizationId: org.id };
};

/** How one invitation went: `exists` = already a member or already invited. */
export type InvitationOutcome = "sent" | "invalid" | "exists" | "failed";

/** Invite `email` to the org as a member; WorkOS sends the email. Never
 * throws, and never logs the address. */
export const sendInvitation = async (
  apiKey: string,
  invitation: { readonly email: string; readonly organizationId: string; readonly inviterUserId: string }
): Promise<InvitationOutcome> => {
  const send = (roleSlug?: string) =>
    post(apiKey, "/user_management/invitations", {
      email: invitation.email,
      organization_id: invitation.organizationId,
      inviter_user_id: invitation.inviterUserId,
      ...(roleSlug ? { role_slug: roleSlug } : {})
    });
  try {
    let res = await send("member");
    let code = res.ok ? undefined : await errorCode(res);
    // Role slugs are per-environment config (as in createOrg): without a
    // "member" role, the default role is the member one.
    if (code === "invalid_role") {
      res = await send();
      code = res.ok ? undefined : await errorCode(res);
    }
    if (res.ok) return "sent";
    if (code === "invalid_request_parameters") return "invalid";
    if (code === "email_already_invited_to_organization" || code === "user_already_organization_member") return "exists";
    console.warn("workos: invitation failed", res.status, code ?? "no code");
    return "failed";
  } catch (e) {
    console.warn("workos: invitation failed", e instanceof Error ? e.name : "error");
    return "failed";
  }
};

const errorCode = async (res: Response): Promise<string | undefined> => {
  try {
    const body = (await res.json()) as { code?: unknown };
    return typeof body.code === "string" ? body.code : undefined;
  } catch {
    return undefined;
  }
};

// ---------------------------------------------------------------------------
// Account and org settings (auth-routes.ts: /auth/me, /auth/orgs/{id}/…).
// Every failure throws WorkOsAuthFailed with the WorkOS status and code;
// nothing here logs, so no name, address or id reaches the tail.
// ---------------------------------------------------------------------------

/** The fields of a WorkOS user object the settings routes use (wire names;
 * the same shape the directory's webhook upsert takes). */
export interface WireProfile {
  readonly id: string;
  readonly email: string;
  readonly first_name: string | null;
  readonly last_name: string | null;
  readonly profile_picture_url: string | null;
  readonly created_at: string;
  readonly updated_at: string;
}

export interface OrgMember {
  readonly membershipId: string;
  readonly userId: string;
  readonly role: string | null;
  readonly status: string;
  /** ISO 8601, when the membership was created. */
  readonly createdAt: string;
}

export interface MembershipRef {
  readonly membershipId: string;
  readonly organizationId: string;
  readonly userId: string;
}

export interface WireInvitation {
  readonly id: string;
  readonly email: string;
  /** `pending`, `accepted`, `expired` or `revoked`. */
  readonly state: string;
  readonly organization_id: string | null;
  readonly expires_at: string;
  readonly created_at: string;
}

/** Rows a settings list reads at most (members, invitations). */
export const SETTINGS_LIST_CAP = 200;

const call = (apiKey: string, method: string, path: string, body?: unknown): Promise<Response> =>
  fetch(`${API}${path}`, {
    method,
    headers: {
      authorization: `Bearer ${apiKey}`,
      ...(body === undefined ? {} : { "content-type": "application/json" })
    },
    ...(body === undefined ? {} : { body: JSON.stringify(body) })
  });

const ok = async <T>(res: Response): Promise<T> => {
  if (!res.ok) return failed(res);
  return (await res.json()) as T;
};

/** Read a WorkOS list to its end or `SETTINGS_LIST_CAP` rows. */
const listAll = async <T>(apiKey: string, path: string, params: Record<string, string>): Promise<T[]> => {
  const rows: T[] = [];
  let after: string | null = null;
  do {
    const query = new URLSearchParams({ ...params, limit: "100", ...(after ? { after } : {}) });
    const page: { data: T[]; list_metadata?: { after?: string | null } } = await ok(await call(apiKey, "GET", `${path}?${query}`));
    rows.push(...page.data);
    after = page.list_metadata?.after ?? null;
  } while (after && rows.length < SETTINGS_LIST_CAP);
  return rows.slice(0, SETTINGS_LIST_CAP);
};

const profileOf = (u: WireProfile): WireProfile => ({
  id: u.id,
  email: u.email,
  first_name: u.first_name ?? null,
  last_name: u.last_name ?? null,
  profile_picture_url: u.profile_picture_url ?? null,
  created_at: u.created_at,
  updated_at: u.updated_at
});

export const getUser = async (apiKey: string, userId: string): Promise<WireProfile> =>
  profileOf(await ok<WireProfile>(await call(apiKey, "GET", `/user_management/users/${encodeURIComponent(userId)}`)));

export const updateUserName = async (
  apiKey: string,
  userId: string,
  firstName: string,
  lastName: string
): Promise<WireProfile> =>
  profileOf(
    await ok<WireProfile>(
      await call(apiKey, "PUT", `/user_management/users/${encodeURIComponent(userId)}`, {
        first_name: firstName,
        last_name: lastName
      })
    )
  );

/** Delete a user for good (Settings → Profile → Delete account). One that's
 * already gone counts as deleted. */
export const deleteUser = async (apiKey: string, userId: string): Promise<void> => {
  const res = await call(apiKey, "DELETE", `/user_management/users/${encodeURIComponent(userId)}`);
  if (!res.ok && res.status !== 404) await failed(res);
};

/** An org's active memberships, oldest first (at most `SETTINGS_LIST_CAP`). */
export const listOrgMembers = async (apiKey: string, organizationId: string): Promise<OrgMember[]> => {
  const rows = await listAll<WireMembership & { user_id: string; status: string; created_at: string }>(
    apiKey,
    "/user_management/organization_memberships",
    { organization_id: organizationId, statuses: "active", order: "asc" }
  );
  return rows.map((m) => ({
    membershipId: m.id,
    userId: m.user_id,
    role: m.role?.slug ?? null,
    status: m.status,
    createdAt: m.created_at
  }));
};

/** One membership; undefined when WorkOS has no such id. */
export const getMembership = async (apiKey: string, membershipId: string): Promise<MembershipRef | undefined> => {
  const res = await call(apiKey, "GET", `/user_management/organization_memberships/${encodeURIComponent(membershipId)}`);
  if (res.status === 404) return undefined;
  const m = await ok<{ id: string; organization_id: string; user_id: string }>(res);
  return { membershipId: m.id, organizationId: m.organization_id, userId: m.user_id };
};

export const removeMembership = async (apiKey: string, membershipId: string): Promise<void> => {
  const res = await call(apiKey, "DELETE", `/user_management/organization_memberships/${encodeURIComponent(membershipId)}`);
  if (!res.ok) return failed(res);
};

export const setMembershipRole = async (apiKey: string, membershipId: string, roleSlug: string): Promise<void> => {
  const res = await call(apiKey, "PUT", `/user_management/organization_memberships/${encodeURIComponent(membershipId)}`, {
    role_slug: roleSlug
  });
  if (!res.ok) return failed(res);
};

/** Every invitation to the org, newest first (at most `SETTINGS_LIST_CAP`). */
export const listInvitations = async (apiKey: string, organizationId: string): Promise<WireInvitation[]> =>
  listAll<WireInvitation>(apiKey, "/user_management/invitations", { organization_id: organizationId, order: "desc" });

/** One invitation; undefined when WorkOS has no such id. */
export const getInvitation = async (apiKey: string, invitationId: string): Promise<WireInvitation | undefined> => {
  const res = await call(apiKey, "GET", `/user_management/invitations/${encodeURIComponent(invitationId)}`);
  if (res.status === 404) return undefined;
  return ok<WireInvitation>(res);
};

export const revokeInvitation = async (apiKey: string, invitationId: string): Promise<void> => {
  const res = await call(apiKey, "POST", `/user_management/invitations/${encodeURIComponent(invitationId)}/revoke`, {});
  if (!res.ok) return failed(res);
};

/** WorkOS re-sends the invitation's email; `unsupported` when this WorkOS
 * has no resend endpoint (404). */
export const resendInvitation = async (apiKey: string, invitationId: string): Promise<"sent" | "unsupported"> => {
  const res = await call(apiKey, "POST", `/user_management/invitations/${encodeURIComponent(invitationId)}/resend`, {});
  if (res.status === 404) return "unsupported";
  if (!res.ok) return failed(res);
  return "sent";
};

/** Delete an org for good (with its memberships and invitations). One that's
 * already gone counts as deleted. */
export const deleteOrg = async (apiKey: string, organizationId: string): Promise<void> => {
  const res = await call(apiKey, "DELETE", `/organizations/${encodeURIComponent(organizationId)}`);
  if (!res.ok && res.status !== 404) await failed(res);
};

export const renameOrg = async (
  apiKey: string,
  organizationId: string,
  name: string
): Promise<{ organizationId: string; name: string }> => {
  const org = await ok<{ id: string; name: string }>(
    await call(apiKey, "PUT", `/organizations/${encodeURIComponent(organizationId)}`, { name })
  );
  return { organizationId: org.id, name: org.name };
};
