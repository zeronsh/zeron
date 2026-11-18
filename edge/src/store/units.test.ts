import { describe, expect, it } from "vitest";
import { decodeCursor, encodeCursor, ftsQuery, searchTerms } from "./search";
import { isText, readmePictures } from "./checks";
import { consentItem, fullControl, heldToItsOwnRules } from "./consent";
import { formatFloat, parseJsonText } from "./json";
import type { JsonObject } from "./json";
import { changelogNotes, checkListing, listingOf } from "./listing";
import { readManifest } from "./manifest";
import { PAYLOAD_PREFIX, decodeBase64, inspectPackageJson, sha256Hex, signaturePayload, verifyPackage } from "./package";
import { comparePrecedence, parseRange, parseVersion, preKey, precedenceText, satisfies } from "./semver";

const v = (text: string) => {
  const parsed = parseVersion(text);
  if (!parsed) throw new Error(text);
  return parsed;
};

describe("store versions", () => {
  it("holds the store's limits on top of semver's", () => {
    expect(parseVersion("9007199254740991.0.0")?.major).toBe(Number.MAX_SAFE_INTEGER);
    expect(parseVersion("9007199254740992.0.0")).toBeUndefined();
    expect(parseVersion(`1.0.0-${"a".repeat(122)}`)).toBeDefined();
    expect(parseVersion(`1.0.0-${"a".repeat(123)}`)).toBeUndefined();
    expect(precedenceText(v("1.2.3-rc.1+build.5"))).toBe("1.2.3-rc.1");
    expect(comparePrecedence(v("1.0.0+a"), v("1.0.0+b"))).toBe(0);
  });

  it("sorts pre-releases by text exactly as by precedence", () => {
    const ordered = ["1.0.0-0", "1.0.0-1", "1.0.0-2", "1.0.0-9", "1.0.0-10", "1.0.0-100", "1.0.0--", "1.0.0-a", "1.0.0-a.0", "1.0.0-a.1", "1.0.0-a.b", "1.0.0-a-b", "1.0.0-alpha", "1.0.0-alpha.1", "1.0.0-alpha.beta", "1.0.0-b", "1.0.0-beta.2", "1.0.0-beta.11", "1.0.0-rc.1", "1.0.0-z"];
    const keys = ordered.map((t) => preKey(v(t).pre));
    for (let i = 1; i < ordered.length; i++) {
      expect(comparePrecedence(v(ordered[i - 1]!), v(ordered[i]!)), `${ordered[i - 1]} < ${ordered[i]}`).toBe(-1);
      expect(keys[i - 1]! < keys[i]!, `key ${ordered[i - 1]} < ${ordered[i]}`).toBe(true);
    }
    expect(preKey([])).toBe("");
  });

  it("reads engines.zeron as the npm-style subset", () => {
    const yes = (range: string, version: string) => expect(satisfies(v(version), parseRange(range)!), `${version} in ${range}`).toBe(true);
    const no = (range: string, version: string) => expect(satisfies(v(version), parseRange(range)!), `${version} not in ${range}`).toBe(false);
    yes(">=0.4.0", "0.4.0");
    yes(">=0.4.0", "1.0.0");
    no(">=0.4.0", "0.3.9");
    no(">=0.4.0", "0.4.0-beta.1");
    yes("^1.2.3", "1.9.0");
    no("^1.2.3", "2.0.0");
    no("^1.2.3", "2.0.0-0");
    yes("^0.2.3", "0.2.9");
    no("^0.2.3", "0.3.0");
    yes("^0.0.3", "0.0.3");
    no("^0.0.3", "0.0.4");
    yes("~1.2.3", "1.2.9");
    no("~1.2.3", "1.3.0");
    yes("~0.0.3", "0.0.9");
    yes("1.2.3", "1.2.3+build");
    no("1.2.3", "1.2.4");
    yes("=1.2.3", "1.2.3");
    yes(">=0.4.0 <0.6.0", "0.5.9");
    no(">=0.4.0 <0.6.0", "0.6.0");
    yes(">=0.4.0 <0.6.0 || >=1.0.0", "1.2.0");
    yes("<0.4.0||>=1.0.0", "0.3.0");
    yes("*", "0.0.1");
    yes(">1.0.0-beta.1", "1.0.0-beta.2");
    yes(">= 0.4.0 <0.6.0", "0.5.0");
    yes("<0.3.0 || *", "9.9.9");
    for (const bad of ["", " ", ">=", "0.4", "^0.4", "x", ">=0.4.0,<0.6.0", "0.4.0 - 0.5.0", ">=v0.4.0", "~>1.0.0", "||", ">=0.4.0 ||", "1.0.0\t2.0.0", "* 1.0.0", ">=*", "a".repeat(300)]) {
      expect(parseRange(bad), bad).toBeUndefined();
    }
  });
});

describe("store JSON", () => {
  it("prints floats as serde_json does", () => {
    const cases: [number, string][] = [
      [1, "1.0"],
      [-0, "-0.0"],
      [0.1, "0.1"],
      [100, "100.0"],
      [1e15, "1000000000000000.0"],
      [1e16, "1e+16"],
      [1.5e16, "1.5e+16"],
      [1e-5, "0.00001"],
      [1e-6, "1e-6"],
      [1.25e-7, "1.25e-7"],
      [123.456, "123.456"],
      [5e-324, "5e-324"],
      [1.7976931348623157e308, "1.7976931348623157e+308"]
    ];
    for (const [value, text] of cases) expect(formatFloat(value), String(value)).toBe(text);
  });
});

describe("store base64", () => {
  it("decodes only canonical standard base64", () => {
    expect(decodeBase64("", true)).toEqual(new Uint8Array(0));
    expect(decodeBase64("QQ==", true)).toEqual(new Uint8Array([65]));
    expect(decodeBase64("QUI=", true)).toEqual(new Uint8Array([65, 66]));
    for (const bad of ["QQ", "QQ=", "QR==", "Q===", "====", "QU I=", "QUI=\n", "-_8=", "QQ==QQ=="]) expect(decodeBase64(bad, true), bad).toBeUndefined();
    expect(decodeBase64("-_8", false)).toEqual(new Uint8Array([251, 255]));
    expect(decodeBase64("-_8=", false)).toBeUndefined();
  });
});

describe("store changelog", () => {
  it("finds the section a version names", () => {
    const log = "# Changelog\n\n## [Unreleased]\n- soon\n\n## [1.2.0] - 2026-11-01\n### Added\n- a thing\n\n## v1.1.0\n- older\n\n## 1.10.0\n- not 1.1.0\n";
    expect(changelogNotes(log, "1.2.0")).toBe("### Added\n- a thing");
    expect(changelogNotes(log, "1.1.0")).toBe("- older");
    expect(changelogNotes(log, "1.10.0")).toBe("- not 1.1.0");
    expect(changelogNotes(log, "1.0.0")).toBeUndefined();
    expect(changelogNotes("## 2.0.0-beta.1\nbeta\n## 2.0.0\r\nfinal\r\n", "2.0.0")).toBe("final");
  });
});

describe("store listing", () => {
  const files = (extra: Record<string, string> = {}) =>
    new Map(Object.entries({ "init.luau": "x", "README.md": "# Hi", "icon.svg": "<svg/>", ...extra }).map(([k, s]) => [k, new TextEncoder().encode(s)]));
  const manifest = (fields: Record<string, unknown>) =>
    readManifest(parseJsonText(JSON.stringify({ id: "focus", name: "Focus", version: "1.0.0", entry: "init.luau", description: "Focus.", icon: "icon.svg", categories: ["productivity"], ...fields })) as JsonObject);
  const rules = (fields: Record<string, unknown>, extra?: Record<string, string>) => checkListing(manifest(fields), files(extra), isText).problems.map((p) => p.rule);

  it("reads a complete listing, and the store's own size limits on top", () => {
    expect(rules({ keywords: ["timer", "deep work"], homepage: "https://example.com/", engines: { zeron: ">=0.4.0" } })).toEqual([]);
    const ok = listingOf(manifest({ keywords: ["timer"], engines: { zeron: ">=0.4.0" } }), files());
    expect(ok).toMatchObject({ summary: "Focus.", icon: "icon.svg", categories: ["productivity"], keywords: ["timer"], engines: ">=0.4.0", readme: "# Hi", notes: null });
    expect(rules({}, { "README.md": `# Big\n${"x".repeat(512 * 1024)}` })).toEqual(["bad_listing"]);
    expect(rules({}, { "CHANGELOG.md": `## 1.0.0\n${"x".repeat(512 * 1024)}` })).toEqual(["too_large"]);
    expect(rules({ x: "y".repeat(256 * 1024) })).toEqual(["too_large"]);
    expect(rules({ icon: "../icon.svg" })).toEqual(["bad_listing"]);
    expect(rules({ icon: "big.svg" }, { "big.svg": `<svg>${"x".repeat(33 * 1024)}</svg>` })).toEqual(["bad_listing"]);
    expect(rules({ repository: "https://user@example.com" })).toEqual(["bad_listing"]);
    expect(rules({ description: undefined, icon: undefined })).toEqual(["missing_listing", "missing_listing"]);
  });

  it("holds amendment 31's rules: folded paths, unseen characters, exact URLs", () => {
    const problems = (fields: Record<string, unknown>, extra?: Record<string, string>) => checkListing(manifest(fields), files(extra), isText).problems.map((p) => [p.rule, p.field, p.path]);
    expect(problems({}, { "INIT.luau": "y" })).toEqual([["bad_listing", "files", "init.luau"]]);
    expect(problems({}, { "caf\u00e9.txt": "a", "cafe\u0301.txt": "b" })).toEqual([["bad_listing", "files", "caf\u00e9.txt"]]);
    expect(problems({}, { "Zeron-Plugin.json": "{}" })).toEqual([["bad_listing", "files", "Zeron-Plugin.json"]]);
    expect(problems({}, { "a/b.txt": "a", "a/b.txt.": "b" }).length).toBe(1);
    expect(problems({ name: "Focus\u202e" })).toEqual([["bad_listing", "name", null]]);
    expect(problems({ description: "Fo\u200bcus." })).toEqual([["bad_listing", "description", null]]);
    expect(problems({ description: "Tab\there." })).toEqual([["bad_listing", "description", null]]);
    expect(problems({ homepage: "https://example.com/a b" })).toEqual([["bad_listing", "homepage", null]]);
    expect(problems({ repository: "https:\\\\evil.com" })).toEqual([["bad_listing", "repository", null]]);
    expect(problems({ homepage: "https://user:pw@example.com/" })).toEqual([["bad_listing", "homepage", null]]);
    // What every author writes is taken, and kept as the parser spells it.
    expect(problems({ homepage: "https://example.com", repository: "https://Example.com/x/../y" })).toEqual([]);
    expect(listingOf(manifest({ homepage: "https://example.com", repository: "https://Example.com/x/../y" }), files())).toMatchObject({
      homepage: "https://example.com/",
      repository: "https://example.com/y"
    });
    expect(problems({ homepage: "https://example.com/focus?x=1#y" })).toEqual([]);
  });
});

describe("store search", () => {
  it("makes every word a quoted prefix term and nothing else", () => {
    expect(searchTerms('Git "review" OR near(x) *')).toEqual(["git", "review", "or", "near", "x"]);
    expect(ftsQuery(searchTerms("Café-Tools"))).toBe('"café"* "tools"*');
    expect(searchTerms("!!! ***")).toEqual([]);
    expect(searchTerms("a b c d e f g h i j")).toHaveLength(8);
  });

  it("keeps a cursor to the list it came from", () => {
    const c = encodeCursor("f1", -3.25, 42);
    expect(decodeCursor(c, "f1")).toEqual({ key: -3.25, seq: 42, dir: "next" });
    expect(decodeCursor(encodeCursor("f1", 1, 2, "prev"), "f1")).toEqual({ key: 1, seq: 2, dir: "prev" });
    expect(decodeCursor(c, "f2")).toBeUndefined();
    for (const bad of ["", "!!!", "e30", encodeCursor("f1", Number.NaN, 1), "a".repeat(600)]) expect(decodeCursor(bad, "f1"), bad).toBeUndefined();
  });
});

describe("store consent item", () => {
  it("rates programs and hosts as the engine does", () => {
    expect(fullControl("python3.12") && fullControl("Bash.exe") && !fullControl("jq")).toBe(true);
    expect(heldToItsOwnRules("Git.exe") && heldToItsOwnRules("gh")).toBe(true);
    const m = readManifest(
      parseJsonText(
        JSON.stringify({
          id: "x1", name: "X", version: "1.0.0", entry: "init.luau",
          permissions: ["commands", "network", "tools"],
          commands: ["gh", "python3", "jq", "some-tool"],
          network: { hosts: ["localhost:8080", "api.example.com", "*.example.org", "box.local"] },
          tools: [{ name: "count", description: "Counts." }]
        })
      ) as JsonObject
    );
    expect(m.problem).toBeUndefined();
    expect(consentItem(m)).toEqual({
      asks: ["commands", "network", "tools"],
      ui: { change: [], replace: [] },
      commands: ["gh", "python3", "jq", "some-tool"],
      commands_anything: ["some-tool"],
      commands_full_control: ["python3"],
      network_hosts: ["localhost:8080", "api.example.com", "*.example.org", "box.local"],
      network_local: ["localhost:8080", "box.local"],
      network_named: ["api.example.com"],
      tools: [{ name: "count", description: "Counts." }]
    });
  });
});

describe("store package fast path", () => {
  const encoder = new TextEncoder();
  const b64url = (b: Uint8Array) => Buffer.from(b).toString("base64url");
  /** A signed package whose JSON text is written by `write`, so its
   * strings can be spelled any way JSON allows. */
  const signed = async (files: Record<string, Uint8Array>, extra: Record<string, string>, write: (data: Record<string, string>, manifest: string) => string) => {
    const pair = (await crypto.subtle.generateKey({ name: "Ed25519" }, true, ["sign", "verify"])) as CryptoKeyPair;
    const key = b64url(new Uint8Array((await crypto.subtle.exportKey("raw", pair.publicKey)) as ArrayBuffer));
    const hashes = Object.fromEntries(await Promise.all(Object.entries(files).map(async ([p, b]) => [p, await sha256Hex(b)] as const)));
    const manifest = { id: "fast", name: "Fast", version: "1.0.0", entry: "init.luau", ...extra };
    const payload = signaturePayload(PAYLOAD_PREFIX, parseJsonText(JSON.stringify(manifest)) as JsonObject, key, new Map(Object.entries(hashes)));
    const sig = b64url(new Uint8Array(await crypto.subtle.sign({ name: "Ed25519" }, pair.privateKey, encoder.encode(payload))));
    const data = Object.fromEntries(Object.entries(files).map(([p, b]) => [p, Buffer.from(b).toString("base64")]));
    return encoder.encode(write(data, JSON.stringify({ ...manifest, signature: { key, files: hashes, sig } })));
  };
  const plain = (data: Record<string, string>, manifest: string) => JSON.stringify({ format: "zeron-plugin:v1", manifest: JSON.parse(manifest), files: data });
  const long = new Uint8Array(300).map((_, i) => (i * 7) & 0xff);

  it("reads long file data in place, and long manifest text as text", async () => {
    const pkg = await signed({ "init.luau": encoder.encode("return 1"), "data.bin": long }, { description: "d".repeat(200) }, plain);
    const v = await verifyPackage(pkg);
    expect(v.files.get("data.bin")).toEqual(long);
    expect(v.manifest.description).toBe("d".repeat(200));
  });

  it("still unescapes long data written with escapes, as serde_json does", async () => {
    const pkg = await signed({ "init.luau": encoder.encode("return 1"), "data.bin": long }, {}, (data, manifest) =>
      plain(data, manifest).replace(data["data.bin"]!, data["data.bin"]!.replaceAll("/", "\\/").replace(/^./, (c) => `\\u00${c.charCodeAt(0).toString(16)}`))
    );
    expect((await verifyPackage(pkg)).files.get("data.bin")).toEqual(long);
  });

  it("refuses long data that isn't canonical base64", async () => {
    const pkg = await signed({ "init.luau": encoder.encode("return 1"), "data.bin": long.subarray(0, 100) }, {}, (data, manifest) =>
      plain({ ...data, "data.bin": data["data.bin"]!.replace(/.=$/, "R=").replace(/==$/, "R=") }, manifest)
    );
    await expect(verifyPackage(pkg)).rejects.toMatchObject({ code: "bad_file" });
    const nonAscii = await signed({ "init.luau": encoder.encode("return 1"), "data.bin": long }, {}, (data, manifest) =>
      plain({ ...data, "data.bin": `${data["data.bin"]!.slice(0, -4)}é===` }, manifest)
    );
    await expect(verifyPackage(nonAscii)).rejects.toMatchObject({ code: "bad_file" });
  });

  it("tells text from binary in place", () => {
    const b = (...bytes: number[]) => new Uint8Array(bytes);
    expect(isText(encoder.encode("plain, ünïcödé, 😀, 中文"))).toBe(true);
    expect(isText(b())).toBe(true);
    for (const bad of [b(0), b(0xc0, 0x80), b(0xe0, 0x80, 0x80), b(0xed, 0xa0, 0x80), b(0xf4, 0x90, 0x80, 0x80), b(0xe4, 0xb8), b(0x80), b(0xff)]) {
      expect(isText(bad), [...bad].join(",")).toBe(false);
    }
  });
});

describe("store package depth (serde_json: 127 containers at most)", () => {
  const encoder = new TextEncoder();
  const b64url = (b: Uint8Array) => Buffer.from(b).toString("base64url");
  const nest = (n: number) => "[".repeat(n) + "]".repeat(n);
  /** A signed package with `x` (nested `inTop` deep) at the top level, or
   * inside the manifest (`inManifest`), or as a file's data (`inFile`).
   * Keys are written in sorted order, so the JSON text is the canonical form. */
  const build = async (where: "top" | "manifest" | "file", arrays: number) => {
    const pair = (await crypto.subtle.generateKey({ name: "Ed25519" }, true, ["sign", "verify"])) as CryptoKeyPair;
    const key = b64url(new Uint8Array((await crypto.subtle.exportKey("raw", pair.publicKey)) as ArrayBuffer));
    const init = encoder.encode("return 1");
    const files: Record<string, string> = { "init.luau": await sha256Hex(init) };
    const x = where === "manifest" ? `,"x":${nest(arrays)}` : "";
    const fields = (sig: string) =>
      `{"entry":"init.luau","id":"deep","name":"Deep","signature":{"files":${JSON.stringify(files)},"key":"${key}"${sig}},"version":"1.0.0"${x}}`;
    const sig = b64url(new Uint8Array(await crypto.subtle.sign({ name: "Ed25519" }, pair.privateKey, encoder.encode(PAYLOAD_PREFIX + fields("")))));
    const fileData = `"init.luau":"${Buffer.from(init).toString("base64")}"` + (where === "file" ? `,"z.bin":${nest(arrays)}` : "");
    const top = where === "top" ? `,"x":${nest(arrays)}` : "";
    return encoder.encode(`{"files":{${fileData}},"format":"zeron-plugin:v1","manifest":${fields(`,"sig":"${sig}"`)}${top}}`);
  };
  const verdict = async (raw: Uint8Array) => {
    try {
      await verifyPackage(raw);
      return "ok";
    } catch (e) {
      return (e as { code: string }).code;
    }
  };

  it("takes 127 open at once and refuses 128 and 129, wherever they are", async () => {
    // Containers open at the deepest point: the top level (1), then where `x` sits.
    for (const [where, around, below] of [["top", 1, "ok"], ["manifest", 2, "ok"], ["file", 2, "bad_file"]] as const) {
      for (const total of [127, 128, 129]) {
        const got = await verdict(await build(where, total - around));
        expect(got, `${where} at ${total}`).toBe(total <= 127 ? below : "not_a_package");
      }
    }
  });

  it("is the coordinator's minimal case: a member nested 127 deep inside the top-level object", async () => {
    const raw = encoder.encode(`{"x":${nest(127)}}`);
    expect(inspectPackageJson(raw).doc).toBe(false);
    expect(inspectPackageJson(encoder.encode(`{"x":${nest(126)}}`)).doc).toBe(true);
  });
});

describe("store README pictures", () => {
  it("finds the relative ones, in linear time", () => {
    expect(readmePictures("# Hi\n![a](shots/one.png) ![b](./x/../two.jpg?v=1#top) ![c](https://e.com/x.png) ![d](//cdn/x.png) ![e](<a%20b.png>)\n![no\n](n.png)")).toEqual([
      "shots/one.png",
      "two.jpg",
      "a b.png"
    ]);
    const hostile = "![".repeat(200_000);
    const started = performance.now();
    expect(readmePictures(hostile)).toEqual([]);
    expect(performance.now() - started).toBeLessThan(500);
  });
});
