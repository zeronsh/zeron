import assert from "node:assert/strict";

// Read-only deployment checks; never logs in or mutates remote state.
const origin = new URL(process.argv[2] ?? "http://127.0.0.1:27641");
const request = (path, init = {}) => fetch(new URL(path, origin), {
  ...init, redirect: "manual", signal: AbortSignal.timeout(15_000)
});
const page = await request("/");
assert.equal(page.status, 200);
assert.match(page.headers.get("content-type") ?? "", /text\/html/);
for (const [header, value] of Object.entries({
  "cross-origin-opener-policy": "same-origin",
  "cross-origin-embedder-policy": "require-corp",
  "cross-origin-resource-policy": "same-origin"
})) assert.equal(page.headers.get(header), value, header);
const html = await page.text();
// Read asset references, not JS identifiers such as window.wasmBindings.
const assets = [...new Set([...html.matchAll(/\bhref=["']([^"']+\.(?:js|wasm))["']/g)].map(match => match[1]))];
assert.ok(assets.some(path => path.endsWith(".wasm")), "WASM preload must be present");
assert.ok(assets.some(path => path.endsWith(".js")), "JS module preload must be present");
for (const asset of assets) {
  const response = await request(asset, { method: "HEAD" });
  assert.equal(response.status, 200, asset);
  assert.match(response.headers.get("content-type") ?? "", asset.endsWith(".wasm") ? /application\/wasm/ : /javascript/);
  assert.equal(response.headers.get("cross-origin-embedder-policy"), "require-corp");
}
const health = await request("/health", { headers: { "sec-fetch-mode": "navigate" } });
assert.equal(health.status, 200);
assert.deepEqual(await health.json(), { ok: true, auth: "workos" });
for (const path of ["/api/browser/session", "/api/nonexistent", "/auth/nonexistent", "/device/nonexistent/status"]) {
  const response = await request(path, { headers: { "sec-fetch-mode": "navigate" } });
  assert.match(response.headers.get("content-type") ?? "", /application\/json/, path);
  assert.ok(!response.headers.has("set-cookie"), `${path}: must not establish a session`);
  if (path === "/api/browser/session") {
    assert.equal(response.status, 200, path);
    assert.deepEqual(await response.json(), { authenticated: false });
  } else {
    assert.equal(response.status, 401, path);
    assert.deepEqual(await response.json(), { error: "unauthenticated" });
  }
}
console.log(`Staging static assets, isolation headers, WorkOS mode, and API routing passed: ${origin.origin}`);
