import { describe, expect, test } from "vitest";
import { changedFiles, diffRows, fileTree, languageOf, manifestChanges, readText } from "./review.ts";

const f = (path: string, sha = "1", binary = false) => ({ path, bytes: 1, sha256: sha, binary });

describe("the file tree", () => {
  test("folders first, then files, each by name", () => {
    const tree = fileTree([f("z.md"), f("shots/b.png"), f("a.luau"), f("shots/a.png"), f("lib/x/y.luau")]);
    expect(tree.map((n) => n.name)).toEqual(["lib", "shots", "a.luau", "z.md"]);
    expect(tree[1]!.children.map((n) => n.path)).toEqual(["shots/a.png", "shots/b.png"]);
    expect(tree[0]!.children[0]!.children[0]!.path).toBe("lib/x/y.luau");
  });
});

describe("what changed", () => {
  test("added, removed and changed files, by path", () => {
    const now = [f("a", "1"), f("b", "2"), f("d", "4")];
    const base = [f("a", "1"), f("b", "x"), f("c", "3")];
    expect(changedFiles(now, base).map((c) => [c.path, c.change])).toEqual([["b", "modified"], ["c", "removed"], ["d", "added"]]);
  });

  test("manifest lists change item by item, risky additions called out, reach first", () => {
    const before = { name: "A", permissions: ["ui.add", "network"], network: { hosts: ["a.example"] }, signature: { sig: "x" }, version: "1.0.0" };
    const now = { name: "B", permissions: ["ui.add", "commands", "sessions.read"], network: { hosts: ["a.example", "b.example"] }, signature: { sig: "y" }, version: "1.1.0" };
    const out = manifestChanges(now, before);
    expect(out.map((c) => c.key)).toEqual(["network", "permissions", "name"]);
    const permissions = out.find((c) => c.key === "permissions")!;
    expect(permissions.added).toEqual([{ item: "commands", risky: true }, { item: "sessions.read", risky: false }]);
    expect(permissions.removed).toEqual([{ item: "network", risky: false }]);
    expect(out.find((c) => c.key === "network")!.added).toEqual([{ item: "hosts: b.example", risky: true }]);
    const name = out.find((c) => c.key === "name")!;
    expect(name.added).toBeNull();
    expect([name.before, name.after]).toEqual(['"A"', '"B"']);
  });

  test("the same object in another key order is no change", () => {
    expect(manifestChanges({ ui: { change: ["a"], replace: [] } }, { ui: { replace: [], change: ["a"] } })).toEqual([]);
  });

  test("lines, numbered before and after", () => {
    const rows = diffRows("a\nb\nc\n", "a\nB\nc\nd\n");
    expect(rows[0]).toEqual({ kind: "hunk", text: "@@ -1,3 +1,4 @@" });
    expect(rows.slice(1)).toEqual([
      { kind: "context", text: "a", old: 1, new: 1 },
      { kind: "removed", text: "b", old: 2 },
      { kind: "added", text: "B", new: 2 },
      { kind: "context", text: "c", old: 3, new: 3 },
      { kind: "added", text: "d", new: 4 },
    ]);
  });
});

describe("reading a file", () => {
  test("text, binary, too big", async () => {
    expect(await readText(new Response("héllo"))).toEqual({ text: "héllo" });
    expect(await readText(new Response(new Uint8Array([0xff, 0xfe, 0x00])))).toEqual({ binary: true });
    expect(await readText(new Response("a\u0000b"))).toEqual({ binary: true });
    expect(await readText(new Response("x".repeat(20)), 10)).toEqual({ tooBig: true });
    expect(await readText(new Response("x", { status: 404 }))).toBeNull();
  });

  test("languages by extension", () => {
    expect(languageOf("init.luau")).toBe("lua");
    expect(languageOf("zeron-plugin.json")).toBe("json");
    expect(languageOf("bin/tool")).toBeNull();
    expect(languageOf("x.constructor")).toBeNull();
  });
});
