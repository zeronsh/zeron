import { generateKeyPairSync } from "node:crypto";
import { kCurrentWorker } from "miniflare";
import { defineConfig } from "vitest/config";
import { cloudflareTest } from "@cloudflare/vitest-pool-workers";
import { createBoatFake } from "./test/workerd/boat-fake.ts";
import { createStripeFake } from "./test/workerd/stripe-fake.ts";

// Runtime-real test tier: runs inside actual workerd via
// @cloudflare/vitest-pool-workers, against a real SQLite-backed Durable
// Object, so platform limits like the ~2MB SQLITE_TOOBIG row cap (the
// 2026-08-05 whale sync freeze) are the runtime's own, not FakeSql constants.
// `npm run test:workerd`.
//
// loro-wasm does NOT work in this tier: the pool's test runner evaluates
// modules where wasm codegen is disallowed, while a real worker compiles the
// base64-inlined module at startup (deployed edge and `wrangler dev` are
// fine). loro-on-workerd coverage lives in the wrangler-dev scripts
// (scripts/whale-check.mjs, scripts/fold-check.mjs).
//
// Cloud tier: real CloudAccount/CloudIndex DOs and real Workflows
// (Miniflare's engine), with Boat replaced by an in-process fake reached as
// the outbound service (every fetch to https://boat.test), the VAULT
// binding pointed at a recording stub entrypoint in the fixture, and a
// throwaway ES256 pair so runner JWTs are real.
const runnerKeys = generateKeyPairSync("ec", { namedCurve: "P-256" });

/** Boat and Stripe fakes behind Miniflare's one outbound service. */
const outbound = () => {
  const boat = createBoatFake();
  const stripe = createStripeFake();
  return (request: Parameters<typeof boat>[0]) =>
    new URL(request.url).hostname === "stripe.test" ? stripe(request) : boat(request);
};

export default defineConfig({
  plugins: [
    cloudflareTest({
      main: "./test/workerd/fixture.ts",
      miniflare: {
        compatibilityDate: "2026-07-01",
        r2Buckets: ["BLOBS"],
        durableObjects: {
          DEVICE_ROOMS: { className: "DeviceRoom", useSQLite: true },
          TEST_LOG: { className: "TestLogRoom", useSQLite: true },
          CHAT_ROOMS: { className: "ChatRoom", useSQLite: true },
          PREVIEW_ROOMS: { className: "PreviewRoom", useSQLite: true },
          REGISTRY_ROOMS: { className: "RegistryRoom", useSQLite: true },
          CLOUD_ACCOUNTS: { className: "CloudAccount", useSQLite: true },
          CLOUD_INDEX: { className: "CloudIndex", useSQLite: true }
        },
        workflows: {
          CLOUD_PROVISION: { name: "test-cloud-provision", className: "ProvisionWorkflow" },
          CLOUD_WAKE: { name: "test-cloud-wake", className: "WakeWorkflow" },
          CLOUD_SLEEP: { name: "test-cloud-sleep", className: "SleepWorkflow" },
          CLOUD_DELETE: { name: "test-cloud-delete", className: "DeleteWorkflow" }
        },
        serviceBindings: {
          VAULT: { name: kCurrentWorker, entrypoint: "VaultStub" }
        },
        r2Buckets: ["BLOBS"],
        bindings: {
          AUTH_MODE: "dev",
          WORKOS_CLIENT_ID: "test-only",
          BOAT_API_KEY: "test-boat-key",
          BOAT_API_BASE: "https://boat.test/api/v1",
          SANDBOX_PROVIDER: "boat",
          CLOUD_TTL_SECONDS: "7200",
          CLOUD_IDLE_MINUTES: "20",
          CLOUD_STARTING_CREDITS: "1000",
          STRIPE_SECRET_KEY: "sk_test_fake",
          STRIPE_WEBHOOK_SECRET: "whsec_test_fake",
          STRIPE_PRICE_BASE: "price_base",
          STRIPE_PRICE_PRO: "price_pro",
          STRIPE_PRICE_MAX: "price_max",
          STRIPE_PRICE_CREDITS: "price_credits",
          STRIPE_API_URL: "https://stripe.test",
          CLOUD_MAX_AWAKE: "2",
          ADMIN_TOKEN: "test-admin-token",
          RUNNER_JWT_PRIVATE_KEY: JSON.stringify(runnerKeys.privateKey.export({ format: "jwk" })),
          RUNNER_JWT_PUBLIC_KEY: JSON.stringify(runnerKeys.publicKey.export({ format: "jwk" }))
        },
        outboundService: outbound()
      }
    })
  ],
  resolve: {
    // Mirror wrangler.jsonc: workerd cannot fetch loro's WASM by URL; the
    // base64 entry inlines it.
    alias: { "loro-crdt": "loro-crdt/base64" }
  },
  test: {
    include: ["test/workerd/**/*.test.ts"],
    // Cloud lifecycle tests drive real Workflows end to end.
    testTimeout: 30_000
  }
});
