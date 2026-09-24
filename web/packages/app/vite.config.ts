import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { resolve } from "node:path";

const edge = process.env.ZERON_DEV_EDGE ?? "http://127.0.0.1:27641";

export default defineConfig({
  plugins: [react()],
  build: {
    rollupOptions: {
      input: {
        index: resolve(__dirname, "index.html"),
        pair: resolve(__dirname, "pair.html"),
      },
    },
  },
  server: {
    // The user's dev domain tunnels here (dev.embedez.com → :3000), and the
    // browser sees the app same-origin with the edge's browser API, so the
    // WorkOS session cookie travels with every request.
    allowedHosts: ["dev.embedez.com", "localhost", "127.0.0.1"],
    proxy: {
      // The edge Worker's browser API — session, login, callback, devices —
      // and the cookie-authenticated device relay WebSocket, proxied to the
      // local staging edge (`wrangler dev`, see scripts/browser-staging.mjs
      // upstream). Same-origin from the browser's point of view.
      // `changeOrigin` rewrites the proxied Host to the loopback upstream:
      // the Worker gates its dev paths on `loopback(url)` and rejects other
      // Hosts, while the browser-side Origin header still validates against
      // WORKOS_BROWSER_ORIGIN. Upstream's browser-dev-proxy.mjs does the same.
      "/api/browser": { target: edge, ws: true, changeOrigin: true },
    },
  },
  // `vite preview` serves the built bundle for the same dev domain: the
  // unbundled dev server pays the tunnel's per-module latency on ~500
  // modules and reads as a slow first load, while the preview server pairs
  // the optimized bundle with the same proxy and host allowlist.
  preview: {
    allowedHosts: ["dev.embedez.com", "localhost", "127.0.0.1"],
    proxy: {
      "/api/browser": { target: edge, ws: true, changeOrigin: true },
    },
  },

});
