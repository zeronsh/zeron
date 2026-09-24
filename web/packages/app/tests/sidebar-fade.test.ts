import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import {
  labelFadeInset,
  labelFadeOutset,
  SIDEBAR_LABEL_FADE_BAND,
} from "../src/components/sidebar-faded-label";

/**
 * The sidebar edge-fade mask, guarded at the artifact level: the shipped
 * stops must be the COMPLEMENT blend `1 − gate × (1 − ramp)`, not the naive
 * `gate × ramp`. The inverted form is the bug that masked the whole sidebar
 * out at rest (both gates 0 → every stop 0): a CSS mask paints at the
 * gradient's alpha, and `edge_fade.rs:184-202` paints fully opaque when no
 * edge is active — the gate removes the FADE, never the content.
 *
 * The label fade (upstream 01b705fe) rides the same artifact: the mask's
 * ramp width must be the measured `--rb-label-fade-inset` defaulting to 0
 * (a fitting label never dims), and the onset curve is
 * `label_fade_outset`'s, ported 1:1 from edge_fade.rs.
 */

const css = readFileSync(new URL("../src/styles/app.css", import.meta.url), "utf8");

function maskStops(): string[] {
  const block = css.match(/\.sidebar-scroll\s*\{[^}]*\}/)?.[0];
  expect(block).toBeDefined();
  const mask = block!.match(/mask-image:\s*linear-gradient\(([^;]+)\);/)?.[1];
  expect(mask).toBeDefined();
  // Split on the commas BETWEEN stops (a comma ahead of the next `rgba(`),
  // never the ones inside a color's `rgba(0, 0, 0, …)`.
  return mask!.replace(/^\s*to bottom\s*,\s*/, "").split(/,\s*(?=rgba\()/).map((stop) => stop.trim());
}

/** The quadratic ramp values per band (edge_fade.rs: (distance / band)²). */
const RAMPS = [0, 0.0625, 0.25, 0.5625, 1] as const;

/** The complement: gate 0 → 1 (fully visible), gate 1 → the ramp itself. */
function stopAlpha(gate: number, ramp: number): number {
  return 1 - gate * (1 - ramp);
}

describe("sidebar edge-fade mask", () => {
  it("blends each gated stop between opaque and the fade ramp", () => {
    const stops = maskStops();
    expect(stops).toHaveLength(10);

    const gatePattern = /rgba\(0,\s*0,\s*0,\s*calc\(1 - var\(--rb-sidebar-fade-(top|bottom)\) \* ([\d.]+)\)\)/;
    for (const stop of stops) {
      const gated = stop.match(gatePattern);
      if (gated === null) {
        // The band-adjacent mid stops are always opaque (ramp 1 → 1 − G·0).
        expect(stop).toMatch(/rgba\(0,\s*0,\s*0,\s*1\)/);
        continue;
      }
      const [, , multiplier] = gated;
      expect(RAMPS).toContain(1 - Number(multiplier));
    }
  });

  it("keeps both bands' ramp order and the 24px band edges", () => {
    const stops = maskStops();
    // The quadratic ramp reads top-down 0 → 1 on both bands (mirrored for
    // bottom), so the complement multipliers run 1 → 0.9375 → 0.75 → 0.4375.
    expect(stops[0]).toMatch(/fade-top\)\s*\*\s*1\)\)\s*0px$/);
    expect(stops[1]).toMatch(/fade-top\)\s*\*\s*0\.9375\)\)\s*6px$/);
    expect(stops[2]).toMatch(/fade-top\)\s*\*\s*0\.75\)\)\s*12px$/);
    expect(stops[3]).toMatch(/fade-top\)\s*\*\s*0\.4375\)\)\s*18px$/);
    expect(stops[4]).toMatch(/rgba\(0,\s*0,\s*0,\s*1\)\s*24px$/);
    expect(stops[5]).toMatch(/rgba\(0,\s*0,\s*0,\s*1\)\s*calc\(100% - 24px\)$/);
    expect(stops[6]).toMatch(/fade-bottom\)\s*\*\s*0\.4375\)\)\s*calc\(100% - 18px\)$/);
    expect(stops[7]).toMatch(/fade-bottom\)\s*\*\s*0\.75\)\)\s*calc\(100% - 12px\)$/);
    expect(stops[8]).toMatch(/fade-bottom\)\s*\*\s*0\.9375\)\)\s*calc\(100% - 6px\)$/);
    expect(stops[9]).toMatch(/fade-bottom\)\s*\*\s*1\)\)\s*100%$/);
  });

  it("resolves to the desktop's states: opaque at rest, the ramp when gated", () => {
    // Rest (gate 0): every stop fully opaque — nothing is masked out.
    for (const ramp of RAMPS) {
      expect(stopAlpha(0, ramp)).toBe(1);
    }
    // Scrolled past the edge (gate 1): exactly the quadratic ramp.
    for (const ramp of RAMPS) {
      expect(stopAlpha(1, ramp)).toBeCloseTo(ramp, 10);
    }
  });

  it("ships both gates defaulted to 0", () => {
    const block = css.match(/\.sidebar-scroll\s*\{[^}]*\}/)![0];
    expect(block).toMatch(/--rb-sidebar-fade-top:\s*0;/);
    expect(block).toMatch(/--rb-sidebar-fade-bottom:\s*0;/);
  });
});

describe("sidebar label fade (upstream 01b705fe)", () => {
  it("label_fade_enters_continuously_and_settles_at_one_band", () => {
    // `label_fade_enters_continuously_and_settles_at_one_band`
    // (edge_fade.rs): no overflow keeps the whole band out of the label;
    // a band or more of overflow settles it fully in.
    const band = SIDEBAR_LABEL_FADE_BAND;
    expect(band).toBe(20);
    expect(labelFadeOutset(0, band)).toBe(band);
    expect(labelFadeOutset(band, band)).toBe(0);
    expect(labelFadeOutset(100, band)).toBe(0);
    // The visible ramp is the complement — 0 while the label fits.
    expect(labelFadeInset(0, band)).toBe(0);
    expect(labelFadeInset(band, band)).toBe(band);
    // Simulate resizing in 0.1 px increments across the old hard cutoff:
    // the eased edge alpha never jumps (a fraction-too-wide label must not
    // suddenly dim its final characters).
    let previous = 1;
    for (let step = 0; step <= 400; step += 1) {
      const overflow = step / 10;
      const edgeAlpha = (labelFadeOutset(overflow, band) / band) ** 2;
      expect(edgeAlpha).toBeLessThanOrEqual(previous);
      expect(previous - edgeAlpha).toBeLessThan(0.02);
      previous = edgeAlpha;
    }
  });

  it("masks the last band through --rb-label-fade-inset, defaulted to 0", () => {
    const block = css.match(/\.sidebar-label-fade\s*\{[^}]*\}/)?.[0];
    expect(block).toBeDefined();
    // The ramp sits at the RIGHT edge, gated by the measured inset var —
    // 0 default means a fitting label paints fully.
    expect(block).toMatch(/mask-image:\s*linear-gradient\(/);
    expect(block).toMatch(/to right/);
    expect(block).toMatch(/#000 calc\(100% - var\(--rb-label-fade-inset, 0px\)\)/);
    expect(block).toMatch(/transparent 100%/);
    // The clipped content stays laid out nowrap behind the mask.
    const inner = css.match(/\.sidebar-label-fade-inner\s*\{[^}]*\}/)?.[0];
    expect(inner).toBeDefined();
    expect(inner).toMatch(/white-space:\s*nowrap;/);
  });

  it("replaces the ellipsis on every faded sidebar label", () => {
    // The desktop swapped truncation for the fade (shell.rs/spaces.rs);
    // a leftover ellipsis would fight the mask.
    for (const label of [
      ".chat-row-folder",
      ".chat-row-title",
      ".chat-row-branch",
      ".arch-row-title",
      ".sidebar-disclosure-label",
      ".space-filter-name",
      ".space-filter-tag",
    ]) {
      const block = css.match(new RegExp(`${label.replace(".", "\\.")}\\s*\\{[^}]*\\}`))?.[0];
      expect(block, `${label} rule`).toBeDefined();
      expect(block, `${label} keeps no ellipsis`).not.toMatch(/text-overflow:\s*ellipsis;/);
      expect(block, `${label} keeps no nowrap`).not.toMatch(/white-space:\s*nowrap;/);
    }
  });
});
