import { describe, expect, it } from "vitest";
import { layout } from "@zeron/theme";
import { applyAppearanceToDocument, inkCssVars } from "../src/theme";
import { DEFAULT_APPEARANCE } from "../src/lib/appearance-store";

/**
 * A stand-in for `document.documentElement`: the test environment is `node`,
 * and `applyAppearanceToDocument` only ever touches the inline style, the
 * dataset, and `colorScheme`.
 */
function stubRoot(): { vars: Map<string, string>; element: HTMLElement } {
  const vars = new Map<string, string>();
  const element = {
    style: {
      colorScheme: "",
      setProperty(name: string, value: string) {
        vars.set(name, value);
      },
    },
    dataset: {} as Record<string, string>,
  };
  return { vars, element: element as unknown as HTMLElement };
}

function applied(appearance: "dark" | "light"): Map<string, string> {
  const { vars, element } = stubRoot();
  applyAppearanceToDocument({ ...DEFAULT_APPEARANCE, mode: appearance }, appearance, element);
  return vars;
}

describe("appearance custom properties", () => {
  it("emits the color roles the desktop authors by hand", () => {
    const dark = applied("dark");
    const light = applied("light");
    // crates/ui/src/theme.rs: text_dim, surface_raised_hover, danger_strong.
    expect(dark.get("--rb-text-dim")).toBe("#989898");
    expect(light.get("--rb-text-dim")).toBe("#636363");
    expect(dark.get("--rb-raised-hover")).toBe("#2b2b2b");
    expect(light.get("--rb-raised-hover")).toBe("#dedede");
    expect(dark.get("--rb-danger-strong")).toBe("#c74b47");
    expect(light.get("--rb-danger-strong")).toBe("#be1022");
  });

  it("branches the derived alphas by appearance", () => {
    const dark = applied("dark");
    const light = applied("light");
    // glass_selected_bg()/card_selected_bg(): wash(0.11) dark, wash(0.06) light.
    expect(dark.get("--rb-selected-wash-alpha")).toBe("0.11");
    expect(light.get("--rb-selected-wash-alpha")).toBe("0.06");
    // band(): always black, 0.16 dark / 0.045 light.
    expect(dark.get("--rb-band-alpha")).toBe("0.16");
    expect(light.get("--rb-band-alpha")).toBe("0.045");
    // scrim(): always black, SCRIM_ALPHA_DARK 0.6 dark / 0.32 light.
    expect(dark.get("--rb-scrim-alpha")).toBe("0.6");
    expect(light.get("--rb-scrim-alpha")).toBe("0.32");
    // The alphas the artifact already carried are untouched by this pass.
    expect(dark.get("--rb-glass-overlay-alpha")).toBe(String(layout.glass.overlayAlphaDark));
    expect(light.get("--rb-glass-overlay-alpha")).toBe(String(layout.glass.overlayAlphaLight));
  });

  it("flattens the input glass into an opaque plate (ticket 79)", () => {
    const dark = applied("dark");
    const light = applied("light");
    // crates/ui/src/theme.rs:968-980 `input_glass_bg()` = flatten(input, bg):
    // zeron-dark authors input #343438b8 (a = 184/255) over background
    // #060606 — per-channel source-over, alpha 1: r/g =
    // round(52·a + 6·(1−a)) = 39, b = round(56·a + 6·(1−a)) = 42. The
    // former color-mix(82%) interpolated alpha (~0.77) and stayed
    // see-through; the plate is opaque. Light input is already #ffffff.
    expect(dark.get("--rb-input-plate")).toBe("#27272a");
    expect(light.get("--rb-input-plate")).toBe("#ffffff");
  });

  it("keeps the neutral ladders tone-flipped", () => {
    expect(inkCssVars("dark")["--rb-wash"]).toBe("235 235 235");
    expect(inkCssVars("light")["--rb-wash"]).toBe("26 26 26");
  });
});
