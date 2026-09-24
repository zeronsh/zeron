import { describe, expect, it } from "vitest";
import {
  MONOGRAM_PALETTE,
  fnv1a,
  monogramCss,
  monogramLetter,
  monogramTone,
} from "../src/lib/monogram";

/*
 * The curated monogram palette, against `crates/ui/src/shell/
 * project_icon.rs`'s `MONOGRAM_PALETTE`/`monogram` (upstream 1ec74e40 →
 * 2f40dfad): the same eight tones, the same FNV-1a selection, the same
 * 8%/85% (24%/full active) alphas.
 */

describe("MONOGRAM_PALETTE", () => {
  it("carries the eight curated (dark, light) tone pairs in the desktop's order", () => {
    expect(MONOGRAM_PALETTE).toEqual([
      [0x94a3b8, 0x475569], // slate
      [0x93c5fd, 0x2563eb], // blue
      [0xc4b5fd, 0x7c3aed], // violet
      [0xfda4af, 0xbe123c], // rose
      [0xfcd34d, 0xa16207], // amber
      [0x6ee7b7, 0x047857], // emerald
      [0x5eead4, 0x0f766e], // teal
      [0xfdba74, 0xc2410c], // orange
    ]);
  });
});

describe("fnv1a", () => {
  it("matches the FNV-1a 32-bit reference vectors", () => {
    // https://datatracker.ietf.org/doc/html/draft-eastlake-fnv: FNV-1a 32
    // of "" , "a", "foobar".
    expect(fnv1a("")).toBe(0x811c9dc5);
    expect(fnv1a("a")).toBe(0xe40c292c);
    expect(fnv1a("foobar")).toBe(0xbf9cf968);
  });
});

describe("monogramTone", () => {
  it("selects a stable palette entry per project path", () => {
    const pick = (path: string): readonly [number, number] => {
      const tone = monogramTone(path);
      return [tone.dark, tone.light];
    };
    expect(pick("/repos/fieldnotes")).toEqual(
      MONOGRAM_PALETTE[fnv1a("/repos/fieldnotes") % MONOGRAM_PALETTE.length]!,
    );
    // The same path always selects the same entry — the desktop's "projects
    // retain their assigned color across processes".
    expect(pick("/repos/fieldnotes")).toEqual(pick("/repos/fieldnotes"));
    // Different paths distribute over the palette.
    const picks = new Set(
      ["/a", "/b", "/c", "/d", "/e", "/f", "/g", "/h", "/i", "/j"].map((p) =>
        fnv1a(p) % MONOGRAM_PALETTE.length,
      ),
    );
    expect(picks.size).toBeGreaterThan(1);
  });
});

describe("monogramLetter", () => {
  it("takes the project's first character, uppercased, with a ? fallback", () => {
    expect(monogramLetter("fieldnotes")).toBe("F");
    expect(monogramLetter("  zeron ")).toBe("Z");
    expect(monogramLetter("")).toBe("?");
    expect(monogramLetter("   ")).toBe("?");
  });
});

describe("monogramCss", () => {
  it("composites the tone with the desktop's opacity arithmetic", () => {
    const tone = { dark: 0x94a3b8, light: 0x475569 };
    expect(monogramCss(tone, "light", 0.08)).toBe("rgb(71 85 105 / 0.08)");
    expect(monogramCss(tone, "dark", 0.85)).toBe("rgb(148 163 184 / 0.85)");
    expect(monogramCss(tone, "dark", 1)).toBe("rgb(148 163 184 / 1)");
  });
});
