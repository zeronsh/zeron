// Close a report: dismissed, or acted on, with a note. (Not destructive:
// no question first.)
import type { APIRoute } from "astro";
import { done, field, flashFor, formOf, tokenMatches } from "../../../lib/store/forms.ts";
import { adminSession } from "../../../lib/store/session.ts";

export const prerender = false;

export const POST: APIRoute = async (context) => {
  const { report } = context.params;
  const session = await adminSession(context);
  if (!session || !report || !/^[\w-]{1,64}$/.test(report)) return new Response(null, { status: 404 });
  const form = await formOf(context.request);
  if (!form || !tokenMatches(form, session)) return done(context, "expired", "/admin/reports");
  const resolution = field(form, "resolution");
  if (resolution !== "dismissed" && resolution !== "actioned") return done(context, "bad_input", "/admin/reports");
  const result = await context.locals.edge.admin(`reports/${encodeURIComponent(report)}/resolve`, { resolution, note: field(form, "note", 500) }, session.csrf);
  return done(context, flashFor(result, "report_resolved"), "/admin/reports");
};
