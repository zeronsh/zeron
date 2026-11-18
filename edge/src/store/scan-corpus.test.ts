/**
 * The package scanner against serde_json on a fixed corpus: 3,175 inputs
 * (hand-written edges, generated packages, byte mutations; depth 125–130
 * among them) with serde_json's own answers, made by
 * scripts/scan-fuzz/run.mjs --write-corpus. The script runs many more,
 * against serde_json live, for whoever changes scan.ts or json.ts.
 */
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { isDeepStrictEqual } from "node:util";
import { describe, expect, it } from "vitest";
import { inspectPackageJson } from "./package";

const here = dirname(fileURLToPath(import.meta.url));
const data = readFileSync(resolve(here, "../../test/fixtures/scan-corpus.bin"));
const expected = readFileSync(resolve(here, "../../test/fixtures/scan-corpus.jsonl"), "utf8").trim().split("\n");

describe("the package scanner reads every corpus input as serde_json does", () => {
  it(`${expected.length} inputs`, () => {
    const differences: string[] = [];
    let at = 0;
    for (const [i, line] of expected.entries()) {
      const n = data.readUInt32LE(at);
      const input = new Uint8Array(data.subarray(at + 4, at + 4 + n));
      at += 4 + n;
      const r = inspectPackageJson(input);
      const got = r.pkg
        ? { doc: r.doc, pkg: { format: r.pkg.format, manifest: r.pkg.manifest, files: r.pkg.files && r.pkg.files.map(([k, v]) => [k, v && Buffer.from(v).toString("hex")]) } }
        : r;
      if (!isDeepStrictEqual(JSON.parse(line), got)) differences.push(`#${i} ${Buffer.from(input).toString("latin1").slice(0, 120)}`);
    }
    expect(at).toBe(data.length);
    expect(differences).toEqual([]);
  });
});
