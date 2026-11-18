// The Worker's bindings (wrangler.jsonc), for the server routes: only what
// the site calls on them, with the web's own Request and Response, so the
// Workers runtime's globals stay out of the browser scripts that share
// this project.
interface ServiceBinding {
  fetch(request: Request): Promise<Response>;
}

interface Env {
  /** The site's built files (dist/client). */
  ASSETS: ServiceBinding;
  /** The edge worker: the store, and the web sign-in session. */
  EDGE: ServiceBinding;
  /** The site's own origin (https://zeron.sh): the store's paths on any other name of this Worker go there. */
  SITE_ORIGIN?: string;
}

declare module "cloudflare:workers" {
  export const env: Env;
}

declare namespace App {
  interface Locals {
    /** The visitor's view of the edge (src/middleware.ts). */
    edge: import("./lib/store/edge.ts").Edge;
  }
}
