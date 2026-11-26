// @ts-check
import { defineConfig, fontProviders } from "astro/config";

// zeron.sh is prerendered and deployed as static assets (wrangler.jsonc).
// When a route needs a server — sign-in, the plugin store — add
// @astrojs/cloudflare as the adapter and mark that route `prerender = false`;
// the rest of the site stays static.
export default defineConfig({
  site: "https://zeron.sh",
  trailingSlash: "never",
  // /privacy is served from privacy.html, the URL the app and stores link to.
  build: { format: "file" },
  devToolbar: { enabled: false },
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
