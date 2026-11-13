import { LoroDoc } from "loro-crdt";
import { describe, expect, it } from "vitest";
import { decodeVersion, versionIncludes } from "./chat-relay";

describe("reading a Loro version vector", () => {
  const version = (doc: LoroDoc) => decodeVersion(doc.oplogVersion().encode())!;

  it("reads what Loro writes, and compares", () => {
    const a = new LoroDoc();
    a.setPeerId(7n);
    a.getText("t").insert(0, "hello");
    a.commit();
    const early = version(a);
    expect(early.get(7n)).toBe(5);
    a.getText("t").insert(0, "more");
    a.commit();
    const late = version(a);
    const b = new LoroDoc();
    b.setPeerId(18164352131159876902n);
    b.getText("t").insert(0, "x");
    b.commit();
    expect(version(b).get(18164352131159876902n)).toBe(1);
    expect(versionIncludes(late, early)).toBe(true);
    expect(versionIncludes(early, late)).toBe(false);
    expect(versionIncludes(late, version(b))).toBe(false);
  });

  it("an empty doc's version is empty", () => {
    expect(version(new LoroDoc()).size).toBe(0);
  });

  it("anything else isn't a version", () => {
    expect(decodeVersion(new Uint8Array([1]))).toBeNull();
    expect(decodeVersion(new Uint8Array([0, 0]))).toBeNull();
  });
});
