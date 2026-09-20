import assert from "node:assert/strict";
import { readFile, readdir } from "node:fs/promises";
import { fileURLToPath } from "node:url";

// Bundle the real entrypoints: Vitest's module runner cannot initialize Loro's
// WASM. All bindings and requests here stay local; no Cloudflare login is needed.
process.env.WRANGLER_SEND_METRICS = "false";
const { unstable_startWorker } = await import("wrangler");
process.chdir(fileURLToPath(new URL("..", import.meta.url)));
const dist = new URL("../../apps/web/dist/", import.meta.url);
const html = await readFile(new URL("index.html", dist), "utf8");
const assets = (await readdir(dist)).filter(name => /^(?:comet-web-[a-f0-9]+(?:_bg)?|[a-f0-9]+-zeron-browser-initializer)\.(js|wasm)$/.test(name));
assert.ok(assets.some(name => name.endsWith(".js")), "build the JS bundle first");
assert.ok(assets.some(name => name.endsWith(".wasm")), "build the WASM bundle first");
const origin = "web.zeron.sh";
const start = (hostname, entrypoint, missingOrigin = false) => unstable_startWorker({
  config: "wrangler.jsonc",
  ...(entrypoint ? { entrypoint } : {}),
  ...(missingOrigin ? { bindings: { WORKOS_BROWSER_ORIGIN: { type: "plain_text", value: "" } } } : {}),
  dev: {
    remote: false, persist: false, watch: false, inspector: false,
    server: { hostname: "127.0.0.1", port: 0 },
    // Wrangler rewrites incoming URLs to this origin, without contacting it.
    origin: { hostname, secure: true },
    outboundService: () => { throw new Error("Unexpected outbound network request"); }
  }
});
const request = (worker, path, init = {}) => worker.fetch(`http://localhost${path}`, {
  ...init, redirect: "manual", signal: AbortSignal.timeout(15_000)
});
let checks = 0;
async function sameBackend(actualWorker, baseline, path, init = {}) {
  const actual = await request(actualWorker, path, init);
  const expected = await request(baseline, path, init);
  assert.equal(actual.status, expected.status, path);
  assert.equal(actual.headers.get("content-type"), expected.headers.get("content-type"), path);
  assert.equal(actual.headers.get("location"), expected.headers.get("location"), path);
  assert.equal(await actual.text(), await expected.text(), path);
  checks++;
}
async function withWorkers(hostname, missingOrigin, check) {
  const production = await start(hostname, undefined, missingOrigin);
  let baseline;
  try {
    baseline = await start(hostname, "src/index.ts", missingOrigin);
    await check(production, baseline);
  } finally {
    await baseline?.dispose();
    await production.dispose();
  }
}
await withWorkers(origin, false, async (production, baseline) => {
  for (const method of ["GET", "HEAD"]) {
    for (const path of ["/", ...assets.map(name => `/${name}`)]) {
      const response = await request(production, path, { method });
      assert.equal(response.status, 200, `${method} ${path}`);
      for (const [header, value] of Object.entries({
        "cross-origin-opener-policy": "same-origin",
        "cross-origin-embedder-policy": "require-corp",
        "cross-origin-resource-policy": "same-origin"
      })) assert.equal(response.headers.get(header), value, path);
      // Hashed bundle files never change, so they cache forever; the shell must revalidate.
      if (path === "/") assert.doesNotMatch(response.headers.get("cache-control") ?? "", /immutable/, path);
      else assert.match(response.headers.get("cache-control") ?? "", /max-age=31536000.*immutable/, path);
      assert.match(response.headers.get("content-type") ?? "", path.endsWith(".wasm") ? /application\/wasm/ : path.endsWith(".js") ? /javascript/ : /text\/html/);
      const body = Buffer.from(await response.arrayBuffer());
      assert.deepEqual(body, method === "HEAD" ? Buffer.alloc(0) : path === "/" ? Buffer.from(html) : await readFile(new URL(path.slice(1), dist)), path);
      checks++;
    }
    // Cloudflare's real asset service canonicalizes index.html to /.
    const index = await request(production, "/index.html", { method });
    assert.equal(index.status, 307);
    assert.equal(index.headers.get("location"), "/");
    await index.arrayBuffer();
    checks++;
  }
  for (const path of ["/health", "/api/browser/session", "/api/nonexistent", "/auth/cli/callback", "/device/d/ws", "/install.sh", "/missing.js", "/api/comet-web-a123.js"]) {
    await sameBackend(production, baseline, path, { headers: { "sec-fetch-mode": "navigate" } });
  }
  await sameBackend(production, baseline, "/", { method: "POST" });
  await sameBackend(production, baseline, "/", { headers: { upgrade: "websocket" } });
});
for (const hostname of ["edge.zeron.sh", "preview.edge.zeron.sh", "ticket.preview.edge.zeron.sh", "edge.comet.zeron.sh", "web.zeron.sh.evil.test"]) {
  await withWorkers(hostname, false, async (production, baseline) => {
    for (const path of ["/", "/index.html", ...assets.map(name => `/${name}`)]) {
      await sameBackend(production, baseline, path);
    }
  });
}
await withWorkers(origin, true, async (production, baseline) => {
  await sameBackend(production, baseline, "/");
});
console.log(`Production routing passed: ${checks} real local Worker checks (assets, origins, backend routes, writes, upgrades, missing origin).`);
