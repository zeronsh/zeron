// What a form did, said once: a word's own message, and no version in it but a version.
import type { AstroCookies } from "astro";
import { describe, expect, test } from "vitest";
import { restoredSaid } from "./admin.ts";
import { FLASH, setFlash, takeFlash, type Said } from "./flash.ts";

const jar = (value?: string) => {
  let kept = value;
  return {
    get: () => (kept === undefined ? undefined : { value: kept }),
    set: (_: string, v: string) => void (kept = v),
    delete: () => void (kept = undefined),
  } as unknown as AstroCookies;
};
const said = (what: Said) => {
  const cookies = jar();
  setFlash(cookies, what);
  return [takeFlash(cookies)?.message, takeFlash(cookies)];
};

describe("a flash", () => {
  test("a word's message, once", () => {
    for (const [word, { message }] of Object.entries(FLASH)) if (typeof message === "string") expect(said(word as keyof typeof FLASH)).toEqual([message, null]);
  });

  test("the versions a message names, and nothing that isn't one", () => {
    expect(said(["version_replaced", "1.2.0-beta.1+build.7", "1.3.0"])).toEqual(["Restored, not verified. 1.3.0 is newer, so 1.2.0-beta.1+build.7 takes no new installs.", null]);
    for (const value of ["", "nothing", "toString", "version_replaced", "version_replaced~1.2.0", "version_replaced~1.2.0~", "version_replaced~1.2.0~<b>", "version_replaced~call us~1.3.0", "version_replaced~1.2.0~1.3.0~1.4.0", `version_replaced~1.2.0~1.3.0-${"a".repeat(40)}`, "verified~1.2.0~1.3.0"])
      expect(takeFlash(jar(value)), value).toBeNull();
  });
});

describe("what a restore says, from the edge's answer", () => {
  test("listed again", () => {
    expect(restoredSaid("1.3.0", { version: { version: "1.3.0", status: "listed", yanked: false, yanked_by: null } })).toBe("version_restored");
  });

  test("superseded at once by a newer one, named", () => {
    expect(restoredSaid("1.2.0", { version: { version: "1.2.0", status: "yanked", yanked: true, yanked_by: "superseded", superseded_by: "1.3.0" } })).toEqual(["version_replaced", "1.2.0", "1.3.0"]);
  });

  test("an answer that says neither: only that it's restored", () => {
    for (const answer of [null, undefined, "", {}, { version: null }, { version: { status: "yanked" } }, { version: { status: "yanked", yanked_by: "superseded", superseded_by: "<b>" } }, { status: "listed" }]) expect(restoredSaid("1.2.0", answer)).toBe("version_back");
  });
});
