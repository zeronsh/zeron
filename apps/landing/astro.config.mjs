// @ts-check
import cloudflare from "@astrojs/cloudflare";
import { cacheCloudflare } from "@astrojs/cloudflare/cache";
import { defineConfig, fontProviders } from "astro/config";

// zeron.sh is prerendered wherever it can be; the routes that need a server
// (the plugin store, its account and admin pages) say `prerender = false`
// and run on the Worker (wrangler.jsonc), which reads the store from the
// edge through its EDGE binding.
//
// `astro dev` stands the fake edge (fake-edge/) up beside the site under the
// edge's name, so the binding finds it.
const command = process.argv.slice(2).find((arg) => !arg.startsWith("-"));
const fakeEdge = command === "dev";

/**
 * Routes for making the store, never built for zeron.sh: the fake's
 * switches (the queue as a picture needs it, the edge, the fake itself),
 * under /_dev in `astro dev` alone.
 * @type {import("astro").AstroIntegration}
 */
const devPages = {
  name: "zeron:dev-pages",
  hooks: {
    "astro:config:setup": ({ command, injectRoute }) => {
      if (command !== "dev") return;
      injectRoute({ pattern: "/_dev/queue", entrypoint: "./src/dev/queue.ts", prerender: false });
      injectRoute({ pattern: "/_dev/edge", entrypoint: "./src/dev/edge.ts", prerender: false });
      injectRoute({ pattern: "/_dev/fake", entrypoint: "./src/dev/fake.ts", prerender: false });
    },
    // (The site's CSP lists the build's own styles; `astro dev` injects its styles inline, unlisted, so a
    // browser draws every page bare. Said on start, with the way to see the site as it ships.)
    "astro:server:start": ({ logger }) => {
      logger.warn("A browser shows these pages unstyled: the site's CSP blocks the styles `astro dev` injects. To see the site as it ships, with the fake store: npm run preview:fake");
    },
  },
};

/**
 * The built 404 page (an address nothing answers) is a file the Worker
 * never renders, so its headers are the built files' own: the site's
 * security headers and its policy, appended to _headers (the adapter adds
 * its own rule beside them). Its bytes stay as built.
 * @type {import("astro").AstroIntegration}
 */
const notFoundHeaders = {
  name: "zeron:not-found-headers",
  hooks: {
    "astro:build:done": async ({ dir }) => {
      const { readFile, writeFile } = await import("node:fs/promises");
      const { SECURITY_HEADERS, policyFor } = await import("./src/lib/store/policy.mjs");
      const { hashes } = JSON.parse(await readFile(new URL("./src/lib/store/inline-styles.json", import.meta.url), "utf8"));
      const file = new URL("./_headers", dir);
      const before = await readFile(file, "utf8").catch(() => "");
      const lines = [...Object.entries(SECURITY_HEADERS).map(([k, v]) => `  ${k}: ${v}`), `  content-security-policy: ${policyFor(hashes)}`];
      const rules = ["/404.html", "/404"].map((path) => `${path}\n${lines.join("\n")}`).join("\n");
      await writeFile(file, `${before}${before && !before.endsWith("\n") ? "\n" : ""}${rules}\n`);
    },
  },
};

/** The landing page's components with scripts of their own (their chunks' names). */
const LANDING_SCRIPTS = /(^|\/)(Base|CopyCommand|Downloads|GitHubStars|Yours)\.astro_astro_type_script_/;

export default defineConfig({
  site: "https://zeron.sh",
  trailingSlash: "never",
  // /privacy is served from privacy.html, the URL the app and stores link to.
  build: { format: "file" },
  devToolbar: { enabled: false },
  integrations: [devPages, notFoundHeaders],
  adapter: cloudflare({
    // Pictures are optimized at build time, as before the adapter.
    imageService: "compile",
    auxiliaryWorkers: fakeEdge ? [{ configPath: "./fake-edge/wrangler.jsonc" }] : [],
  }),
  // The site keeps no sessions of its own: who is signed in is the edge's.
  session: false,
  // Public store pages are kept by the Worker cache for as long as they say
  // (Astro.cache in the route); anything that doesn't say is never kept.
  cache: { provider: cacheCloudflare() },
  // A form from another site is refused by the middleware, as Astro's own
  // check would (lib/store/security.ts isCrossSiteForm): Astro's runs only
  // where a page is, so under /admin it would say which pages are there.
  security: { checkOrigin: false },
  vite: {
    // The store's pages allow scripts from the site's own files only (no
    // inline script), so a script is a file unless it is one of the
    // landing page's, which keep the build's default (small ones inlined)
    // and so stay byte for byte as they were. Pictures keep the default.
    build: { assetsInlineLimit: (file) => (/\.m?js$/.test(file) && !LANDING_SCRIPTS.test(file) ? false : undefined) },
    plugins: [
      {
        name: "zeron:css-target",
        enforce: "post",
        configEnvironment(name, config) {
          // The pages' CSS is collected from the server build. Without the
          // adapter that build's CSS target was esnext; the Cloudflare plugin
          // would make it es2024, which adds prefixes and changes every
          // stylesheet. Kept as it was.
          if (name === "ssr" || name === "prerender") config.build = { ...config.build, cssTarget: "esnext" };
        },
      },
    ],
  },
  fonts: [
    {
      provider: fontProviders.local(),
      name: "Geist",
      cssVariable: "--font-sans",
      fallbacks: ["system-ui", "sans-serif"],
      options: { variants: [{ src: ["./src/assets/fonts/geist.woff2"], weight: "100 900", style: "normal" }] },
    },
    {
      provider: fontProviders.local(),
      name: "Geist Mono",
      cssVariable: "--font-mono",
      fallbacks: ["ui-monospace", "monospace"],
      options: { variants: [{ src: ["./src/assets/fonts/geist-mono.woff2"], weight: "100 900", style: "normal" }] },
    },
  ],
});
