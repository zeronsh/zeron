import { defineConfig } from "vitest/config";

// Pure-logic unit tests (Node, FakeSql). The workerd tier lives in
// vitest.workerd.config.ts and must not be picked up here: its tests import
// `cloudflare:test`, which only resolves inside the workers pool.
// test/unit holds Node tests that need Node APIs (e.g. running the Cloud
// install script under bash) — outside src/ so the Workers typecheck never
// sees `node:*` imports.
export default defineConfig({
  test: {
    include: ["src/**/*.test.ts", "test/unit/**/*.test.ts"]
  }
});
