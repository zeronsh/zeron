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

export default defineConfig({
  site: "https://zeron.sh",
  trailingSlash: "never",
  // /privacy is served from privacy.html, the URL the app and stores link to.
  build: { format: "file" },
  devToolbar: { enabled: false },
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
  security: { checkOrigin: true },
  // The pages' CSS is collected from the server build. Without the adapter
  // that build's CSS target was esnext; the Cloudflare plugin would make it
  // es2024, which adds prefixes and changes every stylesheet. Kept as it was.
  vite: {
    plugins: [
      {
        name: "zeron:css-target",
        enforce: "post",
        configEnvironment: (name) => (name === "ssr" || name === "prerender" ? { build: { cssTarget: "esnext" } } : undefined),
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
