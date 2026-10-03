import { build } from "esbuild";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";

await build({
  entryPoints: [fileURLToPath(new URL("worker.mjs", import.meta.url))],
  outfile: fileURLToPath(new URL("../../../apps/ios/Zeron/NativeCodex/NativeShellWorker.js", import.meta.url)),
  bundle: true,
  platform: "browser",
  format: "iife",
  target: "safari18",
  minify: true,
  legalComments: "eof",
  banner: { js: "/*! just-bash (Apache-2.0)\n" + await readFile(new URL("node_modules/just-bash/LICENSE", import.meta.url), "utf8") + "\n*/" },
  alias: { "node:zlib": fileURLToPath(new URL("zlib-unavailable.mjs", import.meta.url)) },
});
