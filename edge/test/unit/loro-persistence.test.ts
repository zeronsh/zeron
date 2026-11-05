import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { LoroDoc } from "loro-crdt";
// Test-only pin: rolling upgrades must still interoperate with old readers.
import { LoroDoc as LegacyDoc } from "loro-crdt-legacy";

const fixture = (name: string) => readFileSync(new URL(
  `../../../crates/doc/tests/fixtures/loro-1.13.7/${name}`, import.meta.url
));
const updated = () => JSON.parse(fixture("updated.json").toString());

describe("Loro persistence across the 1.13.7 → 1.17 upgrade", () => {
  for (const snapshot of ["snapshot.bin", "shallow.bin"]) {
    it(`merges a legacy update after loading ${snapshot} and persists it for old readers`, () => {
      const modern = new LoroDoc();
      const legacy = new LegacyDoc();
      const cold = new LoroDoc();
      try {
        modern.import(fixture(snapshot));
        expect(modern.toJSON()).toEqual(JSON.parse(fixture("initial.json").toString()));
        modern.getMap("metadata").set("modern", true);
        modern.commit();
        modern.import(fixture("update.bin"));
        const expected = updated();
        expected.metadata.modern = true;
        expect(modern.toJSON()).toEqual(expected);
        // A downgraded client reads the new snapshot, edits, and persists it;
        // the upgraded client must still restore and replay idempotently.
        legacy.import(modern.export({ mode: "snapshot" }));
        expect(legacy.toJSON()).toEqual(expected);
        legacy.getMap("metadata").set("afterRestart", true);
        legacy.commit();
        cold.import(legacy.export({ mode: "snapshot" }));
        cold.import(fixture("update.bin"));
        expected.metadata.afterRestart = true;
        expect(cold.toJSON()).toEqual(expected);
      } finally {
        modern.free(); legacy.free(); cold.free();
      }
    });
  }

  it("exchanges concurrent updates in both directions", () => {
    const modern = new LoroDoc();
    const legacy = new LegacyDoc();
    try {
      modern.import(fixture("snapshot.bin"));
      legacy.import(fixture("snapshot.bin"));
      const modernVersion = modern.version();
      const legacyVersion = legacy.version();
      modern.getText("text").insert(0, "new ");
      modern.getMovableList("queue").move(0, 1);
      modern.commit();
      legacy.getText("text").insert(0, "old ");
      legacy.getMap("metadata").set("concurrent", true);
      legacy.commit();
      const modernUpdate = modern.export({ mode: "update", from: modernVersion });
      const legacyUpdate = legacy.export({ mode: "update", from: legacyVersion });
      modern.import(legacyUpdate);
      legacy.import(modernUpdate);
      expect(modern.toJSON()).toEqual(legacy.toJSON());
      expect(modern.getText("text").toString()).toContain("Hello 🌍");
      expect(modern.getText("text").toString()).toContain("new ");
      expect(modern.getText("text").toString()).toContain("old ");
    } finally {
      modern.free(); legacy.free();
    }
  });

  it("keeps a pending legacy update when its snapshot arrives", () => {
    const doc = new LoroDoc();
    try {
      expect(doc.import(fixture("update.bin")).pending).toBeTruthy();
      doc.import(fixture("snapshot.bin"));
      expect(doc.toJSON()).toEqual(updated());
    } finally { doc.free(); }
  });

  it("rejects truncated snapshots without poisoning the document", () => {
    const snapshot = fixture("snapshot.bin");
    for (const length of [1, 10, Math.floor(snapshot.length / 2), snapshot.length - 1]) {
      const doc = new LoroDoc();
      try {
        doc.getText("local").insert(0, "keep");
        doc.commit();
        const before = doc.toJSON();
        expect(() => doc.import(snapshot.subarray(0, length))).toThrow();
        expect(doc.toJSON()).toEqual(before);
        doc.getText("local").insert(4, " usable");
        doc.commit();
        expect(() => doc.export({ mode: "snapshot" })).not.toThrow();
      } finally { doc.free(); }
    }
  });

  it("bounds WASM memory during streaming with an export every tick", () => {
    execFileSync(process.execPath, [fileURLToPath(new URL(
      "../../scripts/loro-streaming-memory.cjs", import.meta.url
    ))], { timeout: 30_000, stdio: "pipe" });
  }, 35_000);
});
