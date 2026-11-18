// Close a report: dismissed, or acted on, with a note. (Not destructive:
// no question first.)
import type { APIRoute } from "astro";
import { done, field, flashFor, formOf, tokenMatches } from "../../../lib/store/forms.ts";
import { adminOrResume } from "../../../lib/store/resume.ts";

export const prerender = false;

export const POST: APIRoute = async (context) => {
  const { report } = context.params;
  if (!report || !/^[\w-]{1,64}$/.test(report)) return new Response(null, { status: 404 });
  // (Signed out while the page was open: back to the reports, the note as typed.)
  const session = await adminOrResume(context, { path: "/admin/reports", fields: ["report", "note"], form: (form) => form.set("report", report) });
  if (session instanceof Response) return session;
  const form = await formOf(context.request);
  if (!form || !tokenMatches(form, session)) return done(context, "expired", "/admin/reports");
  const resolution = field(form, "resolution");
  if (resolution !== "dismissed" && resolution !== "actioned") return done(context, "bad_input", "/admin/reports");
  const result = await context.locals.edge.admin(`reports/${encodeURIComponent(report)}/resolve`, { resolution, note: field(form, "note", 500) }, session.csrf);
  return done(context, flashFor(result, "report_resolved"), "/admin/reports");
};
