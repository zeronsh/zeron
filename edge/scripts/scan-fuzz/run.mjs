#!/usr/bin/env node
/**
 * The package scanner (src/store/scan.ts, read through package.ts
 * `inspectPackageJson`) against serde_json itself, input by input: whether
 * each is JSON at all, and as the engine reads a package from it, its
 * `format`, canonical `manifest` and every decoded file.
 *
 *   node scripts/scan-fuzz/run.mjs [--seed N] [--count N] [--valid]
 *   node scripts/scan-fuzz/run.mjs --write-corpus     (refresh the fixed corpus npm test runs)
 *
 * `gen.ts` makes the inputs: hand-written edges (escaped keys, duplicate
 * members, every number class at the range's edge, depth 125–130, bad
 * UTF-8), then `count` generated packages and as many byte-mutated ones.
 * `reference/` is the answer key: serde_json at the engine's version and
 * features (float_roundtrip), and base64 0.22. Needs cargo. Run it with a
 * few seeds after any change to scan.ts or json.ts; every difference is a
 * bug on one side.
 *
 * The fixed corpus (test/fixtures/scan-corpus.*, made with --write-corpus:
 * seed 7, 1,500 generated) runs in `npm test` (src/store/scan-corpus.test.ts)
 * without cargo.
 */
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { isDeepStrictEqual } from "node:util";
import { build } from "esbuild";

const here = dirname(fileURLToPath(import.meta.url));
const edge = resolve(here, "../..");
const arg = (name, fallback) => {
  const i = process.argv.indexOf(name);
  return i >= 0 ? process.argv[i + 1] : fallback;
};
const writeCorpus = process.argv.includes("--write-corpus");
const seed = writeCorpus ? "7" : arg("--seed", "1");
const count = writeCorpus ? "1500" : arg("--count", "20000");
const valid = process.argv.includes("--valid");

const work = mkdtempSync(join(tmpdir(), "scan-fuzz-"));
try {
  const bundle = async (entry, out) => {
    await build({ entryPoints: [entry], outfile: out, bundle: true, platform: "node", format: "esm", logLevel: "error" });
    return out;
  };
  const records = join(work, "records.bin");
  execFileSync("node", [await bundle(join(here, "gen.ts"), join(work, "gen.mjs")), records, seed, count, ...(valid ? ["valid"] : [])], { stdio: "inherit" });

  const target = process.env.CARGO_TARGET_DIR ?? join(work, "target");
  execFileSync("cargo", ["build", "--release", "--quiet", "--manifest-path", join(here, "reference/Cargo.toml")], { stdio: "inherit", env: { ...process.env, CARGO_TARGET_DIR: target } });
  const expected = execFileSync(join(target, "release/scan-fuzz-reference"), [records], { maxBuffer: 1 << 30 }).toString();

  const driver = join(work, "driver.ts");
  writeFileSync(
    driver,
    `import { readFileSync } from "node:fs";
     import { inspectPackageJson } from ${JSON.stringify(join(edge, "src/store/package.ts"))};
     const data = readFileSync(process.argv[2]);
     const out = [];
     for (let at = 0; at < data.length; ) {
       const n = data.readUInt32LE(at);
       const r = inspectPackageJson(new Uint8Array(data.subarray(at + 4, at + 4 + n)));
       at += 4 + n;
       const hex = (b) => Buffer.from(b).toString("hex");
       out.push(JSON.stringify(r.pkg ? { doc: r.doc, pkg: { format: r.pkg.format, manifest: r.pkg.manifest, files: r.pkg.files && r.pkg.files.map(([k, v]) => [k, v && hex(v)]) } } : r));
     }
     process.stdout.write(out.join("\\n") + "\\n");`
  );
  const actual = execFileSync("node", [await bundle(driver, join(work, "driver.mjs")), records], { maxBuffer: 1 << 30 }).toString();

  const want = expected.trim().split("\n").map((l) => JSON.parse(l));
  const got = actual.trim().split("\n").map((l) => JSON.parse(l));
  if (want.length !== got.length) throw new Error(`${want.length} answers from serde_json, ${got.length} from the scanner`);
  const data = readFileSync(records);
  let at = 0;
  let differ = 0;
  for (let i = 0; i < want.length; i++) {
    const n = data.readUInt32LE(at);
    const input = data.subarray(at + 4, at + 4 + n);
    at += 4 + n;
    if (!isDeepStrictEqual(want[i], got[i])) {
      if (differ++ < 10) console.log(`differs #${i}: ${JSON.stringify(input.toString("latin1").slice(0, 200))}\n  serde_json: ${JSON.stringify(want[i]).slice(0, 300)}\n  scanner:    ${JSON.stringify(got[i]).slice(0, 300)}`);
    }
  }
  console.log(`${want.length} inputs, ${differ} differences (seed ${seed})`);
  if (writeCorpus && differ === 0) {
    writeFileSync(join(edge, "test/fixtures/scan-corpus.bin"), data);
    writeFileSync(join(edge, "test/fixtures/scan-corpus.jsonl"), expected);
    console.log("wrote test/fixtures/scan-corpus.{bin,jsonl}");
  }
  process.exit(differ ? 1 : 0);
} finally {
  rmSync(work, { recursive: true, force: true });
}
