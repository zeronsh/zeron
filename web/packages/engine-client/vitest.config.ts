import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    projects: [
      {
        test: {
          name: "unit",
          environment: "node",
          include: ["tests/codec.test.ts", "tests/fake-server.test.ts", "tests/watch-cache.test.ts"],
        },
      },
      {
        test: {
          name: "conformance",
          environment: "node",
          include: ["tests/conformance.test.ts"],
          // The suite builds the conformance engine example on first run.
          hookTimeout: 600_000,
          testTimeout: 120_000,
        },
      },
      {
        test: {
          name: "smoke",
          environment: "node",
          include: ["tests/web-smoke.test.ts"],
          // The browser end-to-end smoke (ticket 18): exercises pair +
          // watch + send against the seeded web_smoke engine. Builds the
          // example on first run, same build budget as the conformance
          // suite.
          hookTimeout: 600_000,
          testTimeout: 120_000,
        },
      },
    ],
  },
});
