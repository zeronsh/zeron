import { defineConfig } from "vitest/config";

export default defineConfig({
  // (tests/edge runs the site in front of the real edge: vitest.edge.config.ts.)
  test: { include: ["src/**/*.test.ts", "tests/**/*.test.{ts,js}"], exclude: ["tests/edge/**", "node_modules/**"] },
});
