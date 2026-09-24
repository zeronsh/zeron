import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";

/**
 * The transcript's bottom fade band, guarded at the artifact level like the
 * sidebar's (sidebar-fade.test.ts). The band is a 1px NO-OP veil until the
 * desktop's underlay layout is ported (ticket 18/06): the desktop's band
 * exists to hide content that scrolls UNDER translucent chrome
 * (shell.rs:6025-6072), but the web transcript and the bottom stack are flex
 * SIBLINGS — nothing ever covers transcript pixels — so a live band faded
 * the newest ~79px of content (the just-sent bubble, the streaming tail) to
 * alpha 0 over bare background. An unclamped band value here is the ghost
 * zone coming back.
 */

const css = readFileSync(new URL("../src/styles/app.css", import.meta.url), "utf8");

function transcriptBlock(): string {
  const block = css.match(/\.transcript\s*\{[^}]*\}/)?.[0];
  expect(block).toBeDefined();
  return block!;
}

describe("transcript bottom fade band", () => {
  it("clamps the band to the 1px no-op veil", () => {
    // Not the old `max(bottom_stack − status_strip, 1px)` — a bare 1px, so
    // only the final pixel of the scroller fades. The underlay port
    // (ticket 18/06) is what re-earns a real band.
    expect(transcriptBlock()).toMatch(/--rb-bottom-band:\s*1px;/);
  });

  it("disables scroll anchoring on the scroller", () => {
    // gpui's list never adjusts scrollTop behind the app's back (its scroll
    // handler fires from wheel/touch only, transcript.rs:3131-3133) and the
    // web scroller implements its own anchor preservation
    // (captureAnchor + writePreserving). A browser-anchored adjustment
    // fires a scroll event the stick controller cannot tell from user
    // input — a phantom pin break.
    expect(transcriptBlock()).toMatch(/overflow-anchor:\s*none;/);
  });

  it("keeps the mask stops wired to the band variable", () => {
    const block = transcriptBlock();
    // The bottom gradient stops still reference the (now clamped) variable:
    // the mask geometry survives intact for the underlay port.
    expect(block).toMatch(/calc\(100% - var\(--rb-bottom-band\)\)/);
    expect(block).toMatch(/rgba\(0, 0, 0, 0\) 100%/);
  });
});
