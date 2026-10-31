/**
 * A fake WorkOS API for the workerd tier (Miniflare outbound, host
 * `api.workos.com`): organizations and organization memberships, enough for
 * `POST /auth/orgs` and `GET /auth/orgs`.
 *
 * Test controls (plain HTTP from inside the tests):
 *   GET  https://api.workos.com/__state        {orgs, memberships}
 *   POST https://api.workos.com/__fail         {path, times}: the next `times`
 *                                              POSTs to `path` answer 500
 *   POST https://api.workos.com/__memberships  {user_id, organization_id, role}
 *                                              seeds an active membership
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
}

export const createWorkOsFake = () => {
  const orgs: Org[] = [];
  const memberships: Membership[] = [];
  const failures = new Map<string, number>();
  let next = 0;
  const reply = (status: number, body: unknown) =>
    new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
  const addMembership = (userId: string, organizationId: string, role: string): Membership => {
    const membership = { id: `om_fake${++next}`, user_id: userId, organization_id: organizationId, role, status: "active" };
    memberships.push(membership);
    return membership;
  };
  return async (request: Request): Promise<Response> => {
    const url = new URL(request.url);
    if (url.pathname === "/__state") return reply(200, { orgs, memberships });
    const body = request.method === "POST" ? ((await request.json()) as Record<string, string | number>) : {};
    if (url.pathname === "/__fail") {
      failures.set(body.path as string, body.times as number);
      return reply(200, {});
    }
    if (url.pathname === "/__memberships") {
      return reply(200, addMembership(body.user_id as string, body.organization_id as string, body.role as string));
    }
    const failing = failures.get(url.pathname) ?? 0;
    if (request.method === "POST" && failing > 0) {
      failures.set(url.pathname, failing - 1);
      return reply(500, { message: "fake WorkOS failure" });
    }
    if (request.method === "POST" && url.pathname === "/organizations") {
      const org = { id: `org_fake${++next}`, name: body.name as string };
      orgs.push(org);
      return reply(201, { object: "organization", ...org });
    }
    if (url.pathname === "/user_management/organization_memberships") {
      if (request.method === "POST") {
        const membership = addMembership(body.user_id as string, body.organization_id as string, (body.role_slug as string) ?? "member");
        return reply(201, { object: "organization_membership", ...membership, role: { slug: membership.role } });
      }
      const userId = url.searchParams.get("user_id");
      const data = memberships
        .filter((m) => m.user_id === userId && m.status === "active")
        .map((m) => ({
          object: "organization_membership",
          ...m,
          organization_name: orgs.find((o) => o.id === m.organization_id)?.name ?? null,
          role: { slug: m.role },
          roles: [{ slug: m.role }]
        }));
      return reply(200, { data, list_metadata: { before: null, after: null } });
    }
    return reply(404, { message: `fake WorkOS has no ${request.method} ${url.pathname}` });
  };
};
