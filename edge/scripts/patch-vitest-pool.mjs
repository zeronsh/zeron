import { readFile, writeFile } from "node:fs/promises";

const path = new URL("../node_modules/@cloudflare/vitest-pool-workers/dist/worker/lib/cloudflare/test-internal.mjs", import.meta.url);
const marker = "// Zeron: reject pre-run probes without touching Vitest state.";
const before = 'for (const key of WORKER_ENTRYPOINT_KEYS) Wrapper.prototype[key] = async function(thing) {\n\t\tconst { mainPath, entrypointValue } = await getWorkerEntrypointExport(this.env, entrypoint);';
const after = `for (const key of WORKER_ENTRYPOINT_KEYS) Wrapper.prototype[key] = async function(thing) {\n\t\t${marker}\n\t\tif (typeof __vitest_worker__ !== "object") {\n\t\t\tif (key === "fetch") return new Response("Worker test runner is initializing", { status: 503 });\n\t\t\tthrow new Error("Worker event received before the Vitest runner initialized");\n\t\t}\n\t\tconst { mainPath, entrypointValue } = await getWorkerEntrypointExport(this.env, entrypoint);`;

let source;
try {
  source = await readFile(path, "utf8");
} catch (error) {
  if (error?.code === "ENOENT" && (process.env.npm_config_omit ?? "").split(",").includes("dev")) process.exit(0);
  throw error;
}

if (source.includes(after)) {
  // Already patched with the exact expected guard.
} else if (source.includes(marker) || !source.includes(before)) {
  throw new Error("Unsupported @cloudflare/vitest-pool-workers layout; update the Zeron runner patch");
} else {
  await writeFile(path, source.replace(before, after));
}
