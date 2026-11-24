// The categories' icons are the app's own: each the very SVG the app draws
// (apps/zgx/ui/ds/icon_svgs.luau, generated from its icon set), not a
// look-alike.
import { readFileSync } from "node:fs";
import { describe, expect, test } from "vitest";
import icons from "../../data/icons.json";
import { CATEGORIES } from "./nav.ts";

const app = readFileSync(new URL("../../../../zgx/ui/ds/icon_svgs.luau", import.meta.url), "utf8");
const appIcon = (name: string) => new RegExp(`\\["${name}"\\] = \\[==\\[([\\s\\S]*?)\\]==\\]`).exec(app)?.[1];

describe("the categories' icons are the app's", () => {
  test.each(CATEGORIES.map((item) => [item.label, item.icon]))("%s: %s", (_, name) => {
    expect(appIcon(name), `${name} is one of the app's`).toBeDefined();
    expect(icons[name as keyof typeof icons]).toBe(appIcon(name));
  });
});
