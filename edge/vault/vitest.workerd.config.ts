import { defineConfig } from "vitest/config";
import { cloudflareTest } from "@cloudflare/vitest-pool-workers";

// Runtime-real tier: the real VaultApi entrypoint and both SQLite-backed
// Durable Objects inside workerd (@cloudflare/vitest-pool-workers), built from
// wrangler.jsonc itself so the bindings/migrations under test are the ones
// that deploy. Secrets are fixed test values: a current KEK plus a previous
// one, so the rotation path (open under previous → re-seal under current) is
// exercised. Upstream providers are stubbed per test with a spy on the global
// `fetch` (the DOs share the test isolate). `npm run test:workerd`.
const TEST_KEK = "emVyb24tdmF1bHQtd29ya2VyZC10ZXN0LWtlay0zMmI="; // "zeron-vault-workerd-test-kek-32b"
const TEST_KEK_PREVIOUS = "emVyb24tdmF1bHQtcHJldmlvdXMtdGVzdC1rZWstMzI="; // "zeron-vault-previous-test-kek-32"

const pem = (label: string, der: ArrayBuffer | Uint8Array): string => {
  const bytes = der instanceof Uint8Array ? der : new Uint8Array(der);
  const body = btoa(String.fromCharCode(...bytes)).replace(/.{64}/g, "$&\n");
  return `-----BEGIN ${label}-----\n${body}\n-----END ${label}-----\n`;
};

/**
 * A throwaway GitHub App key per run, handed to the vault the way GitHub
 * downloads it (PKCS#1), so the PKCS#1 → PKCS#8 wrap is exercised too. Tests
 * verify App JWTs against the public half.
 */
const githubAppKey = async () => {
  const pair = (await crypto.subtle.generateKey(
    { name: "RSASSA-PKCS1-v1_5", modulusLength: 2048, publicExponent: new Uint8Array([1, 0, 1]), hash: "SHA-256" },
    true,
    ["sign", "verify"]
  )) as CryptoKeyPair;
  const pkcs8 = new Uint8Array((await crypto.subtle.exportKey("pkcs8", pair.privateKey)) as ArrayBuffer);
  // PrivateKeyInfo = 30 82 LL LL | 02 01 00 | AlgorithmIdentifier (15) | 04 82 LL LL | RSAPrivateKey
  if (pkcs8[0] !== 0x30 || pkcs8[1] !== 0x82 || pkcs8[22] !== 0x04 || pkcs8[23] !== 0x82) {
    throw new Error("unexpected PKCS#8 layout for the test GitHub App key");
  }
  return {
    privateKey: pem("RSA PRIVATE KEY", pkcs8.subarray(26)),
    publicKey: pem("PUBLIC KEY", (await crypto.subtle.exportKey("spki", pair.publicKey)) as ArrayBuffer)
  };
};

export default defineConfig(async () => {
  const appKey = await githubAppKey();
  return {
    plugins: [
      cloudflareTest({
        wrangler: { configPath: "./wrangler.jsonc" },
        miniflare: {
          bindings: {
            VAULT_KEK: TEST_KEK,
            VAULT_KEK_PREVIOUS: TEST_KEK_PREVIOUS,
            GITHUB_APP_CLIENT_ID: "Iv1.testclient",
            GITHUB_APP_CLIENT_SECRET: "test-client-secret",
            GITHUB_APP_PRIVATE_KEY: appKey.privateKey,
            GITHUB_APP_SLUG: "zeron-test",
            TEST_GITHUB_APP_PUBLIC_KEY: appKey.publicKey
          }
        }
      })
    ],
    test: {
      include: ["test/workerd/**/*.test.ts"]
    }
  };
});
