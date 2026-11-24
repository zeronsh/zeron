// The Worker's two entrypoints (wrangler.jsonc `exports`). Cloudflare's
// Worker cache answers before a Worker runs, keyed on the address alone,
// never on cookies: a page kept for everyone would be handed to someone
// signed in, signed-out bar and all. So the front door, uncached, runs for
// every request and only routes: a request with a session cookie goes to the
// site past the cache (`no-store`: not looked up, not kept), where the edge
// says who it is and the page is drawn for them, never kept
// (lib/store/http.ts publicPage); any other goes through the cache to the
// site, today's page for everyone. The front door reads no session and asks
// no one: whose the cookie is, if anyone's, is the site's to ask the edge.
import { handle } from "@astrojs/cloudflare/handler";
import { WorkerEntrypoint } from "cloudflare:workers";
import { hasSessionCookie } from "./lib/store/session-cookie.ts";

// (The runtime's types are kept out of this project, which the browser's scripts share: src/env.d.ts. Only
// what is used here: the request's context and its loopback to this Worker's own entrypoints.)
type Context = Parameters<typeof handle>[2];
interface FrontDoorContext {
  exports: { Site: { fetch(request: Request, init?: { cache?: "no-store" }): Promise<Response> } };
}

/** The site (Astro), behind the Worker cache. */
export class Site extends WorkerEntrypoint<Env> {
  override fetch(request: Request): Promise<Response> {
    return handle(request, this.env, this.ctx as Context) as Promise<Response>;
  }
}

export default {
  fetch(request: Request, _env: Env, ctx: FrontDoorContext): Promise<Response> {
    const site = ctx.exports.Site;
    return hasSessionCookie(request) ? site.fetch(request, { cache: "no-store" }) : site.fetch(request);
  },
};
