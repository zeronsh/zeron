// Who is signed in, for the list's foot on a public page (scripts/app.ts):
// a public page is the same for everyone, so it asks after it has loaded.
// Always 200 (signed out is an answer, not an error), never kept by anyone.
import type { APIRoute } from "astro";

export const prerender = false;

export const GET: APIRoute = async ({ locals }) => {
  const session = await locals.edge.session();
  const body = session
    ? { signed_in: true, name: session.username ? `@${session.username}` : (session.user.name ?? "You"), staff: session.roles.includes("admin") }
    : { signed_in: false };
  return new Response(JSON.stringify(body), { headers: { "content-type": "application/json", "cache-control": "private, no-store", "x-robots-tag": "noindex" } });
};
