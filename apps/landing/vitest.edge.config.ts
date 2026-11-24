import { defineConfig } from "vitest/config";

// The site in front of the real edge worker (tests/edge): the edge's own
// bundle in Miniflare, with the store's D1 and R2 and a WorkOS fake
// (scripts/plugin-store-e2e/edge.mjs). Needs the edge's node_modules.
// `npm run test:edge`.
export default defineConfig({
  test: { include: ["tests/edge/**/*.test.ts"], testTimeout: 60_000, hookTimeout: 180_000, fileParallelism: false },
});
