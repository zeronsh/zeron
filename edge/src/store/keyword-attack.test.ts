// The identity lane's probe: keywords the store lists site-wide (GET /store/v1/keywords) are held to both rules (367b7fbd7), kept as written.
import { expect, it } from "vitest";
import { checkListing } from "./listing";
import { isText } from "./checks";
import { parseJsonText, type JsonObject } from "./json";
import { readManifest } from "./manifest";
const files = new Map(Object.entries({ "init.luau": "x", "README.md": "# Hi", "icon.svg": "<svg/>" }).map(([k, s]) => [k, new TextEncoder().encode(s)]));
const fields = (keywords: string[]) =>
  checkListing(readManifest(parseJsonText(JSON.stringify({ id: "focus", name: "Focus", version: "1.0.0", entry: "init.luau", description: "Focus.", icon: "icon.svg", categories: ["productivity"], keywords })) as JsonObject), files, isText);
for (const k of ["verified", "official", "zeron-official", "by-zeron", "zeron-approved", "zeron-team"]) {
  it(`refuses the keyword ${k}`, () => {
    const { problems, warnings } = fields([k]);
    expect([...problems, ...warnings].some((p) => p.field === "keywords")).toBe(true);
  });
}
