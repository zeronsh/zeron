import { defineConfig } from "vitest/config";

// Pure-logic unit tests (Node): the AES-GCM envelope and KEK rotation, JWT
// reads, provider response classification. The workerd tier lives
// in vitest.workerd.config.ts and must not be picked up here: its tests import
// `cloudflare:test`, which only resolves inside the workers pool.
export default defineConfig({
  test: {
    include: ["src/**/*.test.ts"]
  }
});
