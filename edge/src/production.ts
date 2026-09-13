import edge from "./index";
import type { Env } from "./env";

// Preserve the existing exported classes and hence production DO identities.
export { SessionRoom, DeviceRoom, RegistryRoom, ChatRoom, PreviewRoom, BrowserSessionStore } from "./index";

type ProductionEnv = Env & { ASSETS: Fetcher };

export default {
  async fetch(request: Request, env: ProductionEnv): Promise<Response> {
    const url = new URL(request.url);
    // Only Trunk's entry point and hashed bundles are static routes. Never let
    // asset routing mask an API, native WebSocket, installer, or preview path.
    const staticPath = url.pathname === "/" || url.pathname === "/index.html"
      || /^\/comet-web-[a-f0-9]+(?:_bg)?\.(?:js|wasm)$/.test(url.pathname);
    if (url.origin === env.WORKOS_BROWSER_ORIGIN && staticPath
      && (request.method === "GET" || request.method === "HEAD")
      && !request.headers.has("upgrade")) {
      return env.ASSETS.fetch(request);
    }
    return edge.fetch(request, env);
  }
};
