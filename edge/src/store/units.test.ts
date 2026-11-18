import { describe, expect, it } from "vitest";
import { decodeCursor, encodeCursor, ftsQuery, searchTerms } from "./search";
import { consentItem, fullControl, heldToItsOwnRules } from "./consent";
import { formatFloat, parseJsonText } from "./json";
import type { JsonObject } from "./json";
import { changelogSection, readListing } from "./listing";
import { readManifest } from "./manifest";
import { decodeBase64 } from "./package";
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
    for (const bad of ["", ">= 0.4.0", "0.4", "^0.4", "x", ">=0.4.0,<0.6.0", "0.4.0 - 0.5.0", ">=v0.4.0", "~>1.0.0", "||", ">=0.4.0 ||", "1.0.0\t2.0.0", "a".repeat(300)]) {
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
    expect(changelogSection(log, "1.2.0")).toBe("### Added\n- a thing");
    expect(changelogSection(log, "1.1.0")).toBe("- older");
    expect(changelogSection(log, "1.10.0")).toBe("- not 1.1.0");
    expect(changelogSection(log, "1.0.0")).toBeUndefined();
    expect(changelogSection("## 2.0.0-beta.1\nbeta\n## 2.0.0\nfinal", "2.0.0")).toBe("final");
  });
});

describe("store listing", () => {
  const files = (extra: Record<string, string> = {}) =>
    new Map(Object.entries({ "init.luau": "x", "README.md": "# Hi", "icon.svg": "<svg/>", ...extra }).map(([k, s]) => [k, new TextEncoder().encode(s)]));
  const manifest = (fields: Record<string, unknown>) =>
    readManifest(parseJsonText(JSON.stringify({ id: "focus", name: "Focus", version: "1.0.0", entry: "init.luau", description: "Focus.", icon: "icon.svg", categories: ["productivity"], ...fields })) as JsonObject);

  it("reads a complete listing and refuses an incomplete one", () => {
    const ok = readListing(manifest({ keywords: ["timer", "deep work"], homepage: "https://example.com", engines: { zeron: ">=0.4.0" } }), files());
    expect(ok).toMatchObject({ summary: "Focus.", icon: "icon.svg", categories: ["productivity"], keywords: ["timer", "deep work"], engines: ">=0.4.0", readme: "# Hi", notes: null });
    expect(readListing(manifest({ icon: "../icon.svg" }), files())).toMatchObject({ error: "missing_listing" });
    expect(readListing(manifest({ icon: "big.svg" }), files({ "big.svg": `<svg>${"x".repeat(33 * 1024)}</svg>` }))).toMatchObject({ error: "missing_listing" });
    expect(readListing(manifest({ keywords: Array.from({ length: 9 }, (_, i) => `k${i}`) }), files())).toMatchObject({ error: "missing_listing" });
    expect(readListing(manifest({ keywords: [" padded"] }), files())).toMatchObject({ error: "missing_listing" });
    expect(readListing(manifest({ categories: ["git", "git"] }), files())).toMatchObject({ error: "missing_listing" });
    expect(readListing(manifest({ repository: "https://user:pw@example.com" }), files())).toMatchObject({ error: "missing_listing" });
    expect(readListing(manifest({ description: "two\nlines" }), files())).toMatchObject({ error: "missing_listing" });
    expect(readListing(manifest({}), new Map([...files()].map(([k, b]) => [k, k === "README.md" ? new Uint8Array([0xff, 0xfe]) : b])))).toMatchObject({ error: "missing_readme" });
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
    expect(decodeCursor(c, "f1")).toEqual({ key: -3.25, seq: 42 });
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
