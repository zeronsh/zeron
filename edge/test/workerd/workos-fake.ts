/**
 * A fake WorkOS API for the workerd tier (Miniflare outbound, host
 * `api.workos.com`): users, organizations, organization memberships and
 * invitations, enough for the /auth/orgs, invitation and settings routes.
 *
 * Test controls (plain HTTP from inside the tests):
 *   GET  https://api.workos.com/__state        {orgs, users, memberships, invitations,
 *                                              deletedOrgs}
 *   POST https://api.workos.com/__fail         {path, times, status?, code?, method?}:
 *                                              the next `times` calls to `path`
 *                                              (by `method`, or any) answer
 *                                              `status` (500)
 *   POST https://api.workos.com/__memberships  {user_id, organization_id, role,
 *                                              email?} seeds an active membership
 *   POST https://api.workos.com/__users        {id, email, first_name?, last_name?,
 *                                              profile_picture_url?} seeds a user
 *   POST https://api.workos.com/__invitations  {email, organization_id, state,
 *                                              expires_at} seeds an invitation
 */
import { Response, type Request } from "miniflare";

interface Org {
  id: string;
  name: string;
}

interface Membership {
  id: string;
  user_id: string;
  organization_id: string;
  role: string;
  status: string;
  /** The member's address, for "already a member" on invitations. */
  email?: string;
  created_at: string;
}

interface Invitation {
  id: string;
  email: string;
  organization_id: string;
  inviter_user_id: string | null;
  role_slug: string | null;
  state: string;
  expires_at: string;
  created_at: string;
  /** How many times its email went out. */
  sends: number;
}

interface User {
  id: string;
  email: string;
  first_name: string | null;
  last_name: string | null;
  profile_picture_url: string | null;
  created_at: string;
  updated_at: string;
}

const WEEK_MS = 7 * 86_400_000;

export const createWorkOsFake = () => {
  const orgs: Org[] = [];
  const memberships: Membership[] = [];
  const invitations: Invitation[] = [];
  const users: User[] = [];
  /** Orgs deleted (DELETE /organizations/{id}), in order. */
  const deletedOrgs: string[] = [];
  const failures = new Map<string, { times: number; status: number; code?: string; method?: string }>();
  let next = 0;
  /** Strictly increasing, so a later write always has a later `updated_at`. */
  let clock = Date.parse("2026-01-01T00:00:00Z");
  const now = () => new Date((clock += 1000)).toISOString();
  const reply = (status: number, body: unknown) =>
    new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
  const addMembership = (userId: string, organizationId: string, role: string, email?: string): Membership => {
    const membership = {
      id: `om_fake${++next}`,
      user_id: userId,
      organization_id: organizationId,
      role,
      status: "active",
      email,
      created_at: now()
    };
    memberships.push(membership);
    return membership;
  };
  const wireMembership = (m: Membership) => ({
    object: "organization_membership",
    ...m,
    organization_name: orgs.find((o) => o.id === m.organization_id)?.name ?? null,
    role: { slug: m.role },
    roles: [{ slug: m.role }]
  });
  const wireInvitation = ({ sends: _sends, ...i }: Invitation) => ({ object: "invitation", ...i });
  /** WorkOS list pages: `limit` rows after the `after` id. */
  const page = <T extends { id: string }>(rows: T[], url: URL) => {
    const limit = Number(url.searchParams.get("limit") ?? "10");
    const after = url.searchParams.get("after");
    const start = after ? rows.findIndex((r) => r.id === after) + 1 : 0;
    const data = rows.slice(start, start + limit);
    return { data, list_metadata: { before: null, after: start + limit < rows.length ? data[data.length - 1]!.id : null } };
  };
  return async (request: Request): Promise<Response> => {
    const url = new URL(request.url);
    if (url.pathname === "/__state") return reply(200, { orgs, users, memberships, invitations, deletedOrgs });
    const body =
      request.method === "POST" || request.method === "PUT" ? ((await request.json()) as Record<string, string | number>) : {};
    if (url.pathname === "/__fail") {
      failures.set(body.path as string, {
        times: body.times as number,
        status: (body.status as number | undefined) ?? 500,
        code: body.code as string | undefined,
        method: body.method as string | undefined
      });
      return reply(200, {});
    }
    if (url.pathname === "/__users") {
      const at = now();
      const user: User = {
        id: body.id as string,
        email: body.email as string,
        first_name: (body.first_name as string | undefined) ?? null,
        last_name: (body.last_name as string | undefined) ?? null,
        profile_picture_url: (body.profile_picture_url as string | undefined) ?? null,
        created_at: at,
        updated_at: at
      };
      users.push(user);
      return reply(200, user);
    }
    if (url.pathname === "/__invitations") {
      const invitation: Invitation = {
        id: `invitation_fake${++next}`,
        email: body.email as string,
        organization_id: body.organization_id as string,
        inviter_user_id: null,
        role_slug: "member",
        state: body.state as string,
        expires_at: body.expires_at as string,
        created_at: now(),
        sends: 1
      };
      invitations.push(invitation);
      return reply(200, wireInvitation(invitation));
    }
    if (url.pathname === "/__memberships") {
      return reply(
        200,
        addMembership(body.user_id as string, body.organization_id as string, body.role as string, body.email as string | undefined)
      );
    }
    const failing = failures.get(url.pathname);
    if (failing && failing.times > 0 && (!failing.method || failing.method === request.method)) {
      failing.times -= 1;
      return reply(failing.status, { message: "fake WorkOS failure", ...(failing.code ? { code: failing.code } : {}) });
    }
    const userMatch = /^\/user_management\/users\/([^/]+)$/.exec(url.pathname);
    if (userMatch) {
      const user = users.find((u) => u.id === decodeURIComponent(userMatch[1]!));
      if (!user) return reply(404, { code: "entity_not_found", message: "User not found." });
      if (request.method === "DELETE") {
        // Gone, and their memberships with them.
        users.splice(users.indexOf(user), 1);
        for (let i = memberships.length - 1; i >= 0; i--) if (memberships[i]!.user_id === user.id) memberships.splice(i, 1);
        return new Response(null, { status: 202 });
      }
      if (request.method === "PUT") {
        if (typeof body.first_name === "string") user.first_name = body.first_name;
        if (typeof body.last_name === "string") user.last_name = body.last_name;
        user.updated_at = now();
      }
      return reply(200, { object: "user", ...user });
    }
    const membershipMatch = /^\/user_management\/organization_memberships\/([^/]+)$/.exec(url.pathname);
    if (membershipMatch) {
      const index = memberships.findIndex((m) => m.id === decodeURIComponent(membershipMatch[1]!));
      if (index < 0) return reply(404, { code: "entity_not_found", message: "Membership not found." });
      const membership = memberships[index]!;
      if (request.method === "DELETE") {
        memberships.splice(index, 1);
        return new Response(null, { status: 202 });
      }
      if (request.method === "PUT") {
        if (body.role_slug !== "admin" && body.role_slug !== "member") {
          return reply(422, { code: "invalid_role", message: "Role does not exist." });
        }
        membership.role = body.role_slug as string;
      }
      return reply(200, wireMembership(membership));
    }
    const invitationMatch = /^\/user_management\/invitations\/([^/]+)(?:\/(revoke|resend))?$/.exec(url.pathname);
    if (invitationMatch) {
      const invitation = invitations.find((i) => i.id === decodeURIComponent(invitationMatch[1]!));
      if (!invitation) return reply(404, { code: "entity_not_found", message: "Invitation not found." });
      if (request.method === "POST" && invitationMatch[2] === "revoke") invitation.state = "revoked";
      if (request.method === "POST" && invitationMatch[2] === "resend") {
        invitation.sends += 1;
        invitation.state = "pending";
        invitation.expires_at = new Date(Date.now() + WEEK_MS).toISOString();
      }
      return reply(200, wireInvitation(invitation));
    }
    const orgMatch = /^\/organizations\/([^/]+)$/.exec(url.pathname);
    if (orgMatch && request.method === "DELETE") {
      // Gone, with its memberships and invitations.
      const orgId = decodeURIComponent(orgMatch[1]!);
      // (Tests seed memberships into orgs the fake never made: those count.)
      const index = orgs.findIndex((o) => o.id === orgId);
      const known = index >= 0 || memberships.some((m) => m.organization_id === orgId);
      if (!known) return reply(404, { code: "entity_not_found", message: "Organization not found." });
      if (index >= 0) orgs.splice(index, 1);
      deletedOrgs.push(orgId);
      for (let i = memberships.length - 1; i >= 0; i--) if (memberships[i]!.organization_id === orgId) memberships.splice(i, 1);
      for (let i = invitations.length - 1; i >= 0; i--) if (invitations[i]!.organization_id === orgId) invitations.splice(i, 1);
      return new Response(null, { status: 202 });
    }
    if (orgMatch && request.method === "PUT") {
      const orgId = decodeURIComponent(orgMatch[1]!);
      let org = orgs.find((o) => o.id === orgId);
      if (!org) {
        org = { id: orgId, name: "" };
        orgs.push(org);
      }
      org.name = body.name as string;
      return reply(200, { object: "organization", ...org });
    }
    if (request.method === "POST" && url.pathname === "/user_management/authenticate") {
      // A code is `code:<userId>` for a seeded user; a refresh token is
      // `rt:<userId>`. The access token is the user id (the edge's dev mode).
      const grant = body.grant_type as string;
      const raw = String(grant === "authorization_code" ? body.code : body.refresh_token);
      const prefix = grant === "authorization_code" ? "code:" : "rt:";
      const user = raw.startsWith(prefix) ? users.find((u) => u.id === raw.slice(prefix.length)) : undefined;
      if (!user) return reply(400, { error: "invalid_grant", error_description: "Invalid code." });
      return reply(200, { user: { object: "user", ...user }, access_token: user.id, refresh_token: `rt:${user.id}` });
    }
    if (request.method === "POST" && url.pathname === "/organizations") {
      const org = { id: `org_fake${++next}`, name: body.name as string };
      orgs.push(org);
      return reply(201, { object: "organization", ...org });
    }
    if (request.method === "POST" && url.pathname === "/user_management/invitations") {
      const email = String(body.email);
      const org = body.organization_id as string;
      // Stricter than the edge's own check (no empty domain labels), so a
      // test can reach this answer.
      if (!/^[^\s@]+@[^\s@.]+(\.[^\s@.]+)+$/.test(email)) {
        return reply(422, { code: "invalid_request_parameters", message: "Validation failed" });
      }
      const same = (a: string | undefined) => a?.toLowerCase() === email.toLowerCase();
      if (memberships.some((m) => m.organization_id === org && same(m.email))) {
        return reply(400, { code: "user_already_organization_member", message: "User already a member of organization." });
      }
      if (invitations.some((i) => i.organization_id === org && same(i.email) && i.state === "pending")) {
        return reply(400, { code: "email_already_invited_to_organization", message: "Email already invited to organization." });
      }
      const invitation: Invitation = {
        id: `invitation_fake${++next}`,
        email,
        organization_id: org,
        inviter_user_id: (body.inviter_user_id as string) ?? null,
        role_slug: (body.role_slug as string) ?? null,
        state: "pending",
        expires_at: new Date(Date.now() + WEEK_MS).toISOString(),
        created_at: now(),
        sends: 1
      };
      invitations.push(invitation);
      return reply(201, wireInvitation(invitation));
    }
    if (request.method === "GET" && url.pathname === "/user_management/invitations") {
      const org = url.searchParams.get("organization_id");
      return reply(200, page(invitations.filter((i) => i.organization_id === org).reverse(), url));
    }
    if (url.pathname === "/user_management/organization_memberships") {
      if (request.method === "POST") {
        const membership = addMembership(body.user_id as string, body.organization_id as string, (body.role_slug as string) ?? "member");
        return reply(201, { object: "organization_membership", ...membership, role: { slug: membership.role } });
      }
      const userId = url.searchParams.get("user_id");
      const orgId = url.searchParams.get("organization_id");
      const rows = memberships
        .filter((m) => (userId === null || m.user_id === userId) && (orgId === null || m.organization_id === orgId))
        .filter((m) => m.status === "active");
      return reply(200, page(rows.map(wireMembership), url));
    }
    return reply(404, { message: `fake WorkOS has no ${request.method} ${url.pathname}` });
  };
};
