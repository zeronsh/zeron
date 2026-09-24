import { describe, expect, it } from "vitest";
import { findVariant } from "@zeron/theme";
import {
  accentHelper,
  accentSwatchColor,
  AppearanceStore,
  CODE_FONT_CHOICES,
  DEFAULT_APPEARANCE,
  effectiveCodeFontFamily,
  effectiveTerminalFontFamily,
  effectiveUiFontFamily,
  fontSizePxLabel,
  MONO_FONT_SIZES,
  nearestMonoFontSize,
  resolveAppearance,
  resolveSurfaceTreatment,
  resolveVariantId,
  stepFont,
  SURFACE_PREFERENCES,
  surfaceHelper,
  TERMINAL_FONT_CHOICES,
  UI_FONT_CHOICES,
  variantChoices,
} from "../src/lib/appearance-store";
import {
  codeBlockLineHeight,
  codeBlockTextSize,
  diffLineHeight,
  diffTextSize,
  editorTextSize,
  PREVIEW_LINE_HEIGHT_BASELINE,
  previewLineHeight,
  previewTextSize,
} from "../src/lib/typography";
import { CODE_FONT_SIZE_DEFAULT, FONT_SIZE_MAX, FONT_SIZE_MIN } from "../src/state/ui-settings";
import { sourceName, slug } from "../src/lib/theme-library";
import {
  backgroundRowState,
  BACKGROUND_UNSUPPORTED_MESSAGE,
  DEFAULT_NEW_THREAD_BACKGROUND_URL,
  installNewThreadBackground,
  NEW_THREAD_BACKGROUND_IDB_PATH,
  removeNewThreadBackground,
  resolveActiveNewThreadBackground,
  resolveInstalledBackground,
} from "../src/lib/new-thread-background";
import { memoryBackgroundBlobStore } from "../src/lib/background-blob-store";
import { UiSettingsStore } from "../src/state/ui-settings";
import type { StorageLike } from "../src/lib/engine-store";

function memoryStorage(): StorageLike & { dump(): Map<string, string> } {
  const map = new Map<string, string>();
  return {
    getItem: (key) => (map.has(key) ? map.get(key)! : null),
    setItem: (key, value) => void map.set(key, value),
    removeItem: (key) => void map.delete(key),
    dump: () => map,
  };
}

describe("AppearanceStore", () => {
  it("defaults to the desktop's ThemeSelection default", () => {
    const store = new AppearanceStore({ storage: memoryStorage() });
    expect(store.getSnapshot()).toEqual(DEFAULT_APPEARANCE);
    expect(store.getSnapshot().mode).toBe("system");
    expect(store.getSnapshot().darkVariant).toBe("zeron-dark");
    expect(store.getSnapshot().lightVariant).toBe("zeron-light");
  });

  it("persists every change and reloads through storage", () => {
    const storage = memoryStorage();
    const store = new AppearanceStore({ storage });
    store.setMode("light");
    store.setVariant("light", "github-light");
    store.setVariant("dark", "nord");
    store.setAccent("pink");
    store.setSurface("opaque");
    const reloaded = new AppearanceStore({ storage });
    expect(reloaded.getSnapshot()).toEqual({
      mode: "light",
      lightVariant: "github-light",
      darkVariant: "nord",
      accent: "pink",
      surface: "opaque",
    });
  });

  it("notifies subscribers on actual changes only", () => {
    const store = new AppearanceStore({ storage: memoryStorage() });
    let fired = 0;
    const unsubscribe = store.subscribe(() => {
      fired += 1;
    });
    store.setMode("dark");
    store.setMode("dark");
    store.setAccent("cyan");
    unsubscribe();
    store.setSurface("opaque");
    expect(fired).toBe(2);
  });

  it("refuses a variant authored for the other appearance", () => {
    const store = new AppearanceStore({ storage: memoryStorage() });
    store.setVariant("light", "nord"); // nord is dark-only
    expect(store.getSnapshot().lightVariant).toBe("zeron-light");
    store.setVariant("dark", "nord");
    expect(store.getSnapshot().darkVariant).toBe("nord");
  });

  it("ignores corrupted legacy state without destroying it", () => {
    // Storage moved to the consolidated ui-settings key; the legacy key is a
    // one-time migration source now, so a corrupt one heals to the defaults
    // and is left exactly where it is for a rollback to find.
    const storage = memoryStorage();
    storage.setItem("zeron.appearance.v1", "{not json");
    expect(new AppearanceStore({ storage }).getSnapshot()).toEqual(DEFAULT_APPEARANCE);
    expect(storage.getItem("zeron.appearance.v1")).toBe("{not json");
  });

  it("migrates the legacy appearance key on first load", () => {
    const storage = memoryStorage();
    storage.setItem(
      "zeron.appearance.v1",
      JSON.stringify({
        version: 1,
        mode: "dark",
        lightVariant: "github-light",
        darkVariant: "nord",
        accent: "pink",
        surface: "opaque",
      }),
    );
    expect(new AppearanceStore({ storage }).getSnapshot()).toEqual({
      mode: "dark",
      lightVariant: "github-light",
      darkVariant: "nord",
      accent: "pink",
      surface: "opaque",
    });
    expect(storage.getItem("zeron.ui-settings.v1")).not.toBe(null);
  });

  it("heals a persisted frosted choice to the explicit opaque one", () => {
    // Frosted was removed by product decision; a stored one must not fall
    // back to the theme default the user had deliberately moved off of.
    const storage = memoryStorage();
    storage.setItem(
      "zeron.ui-settings.v1",
      JSON.stringify({ ...DEFAULT_APPEARANCE, surface: "frosted" }),
    );
    expect(new AppearanceStore({ storage }).getSnapshot().surface).toBe("opaque");
  });

  it("falls back per-field when persisted values are unknown", () => {
    const storage = memoryStorage();
    storage.setItem(
      "zeron.appearance.v1",
      JSON.stringify({
        version: 1,
        mode: "sepia",
        lightVariant: "nord",
        darkVariant: "ghost-variant",
        accent: "octarine",
        surface: "mirror",
      }),
    );
    expect(new AppearanceStore({ storage }).getSnapshot()).toEqual(DEFAULT_APPEARANCE);
  });
});

describe("appearance resolution", () => {
  it("combines the mode with the OS state (appearance.rs resolve)", () => {
    expect(resolveAppearance("system", "dark")).toBe("dark");
    expect(resolveAppearance("system", "light")).toBe("light");
    expect(resolveAppearance("light", "dark")).toBe("light");
    expect(resolveAppearance("dark", "light")).toBe("dark");
  });

  it("keeps the light and dark variants independent", () => {
    const preferences = { ...DEFAULT_APPEARANCE, lightVariant: "github-light", darkVariant: "dracula" };
    expect(resolveVariantId(preferences, "light")).toBe("github-light");
    expect(resolveVariantId(preferences, "dark")).toBe("dracula");
  });

  /*
   * Product decision (2026-09-17): the web never frosts — the resolution is
   * forced opaque regardless of the stored preference or the variant's
   * recommendation (both default themes recommend frosted). A deliberate
   * deviation from the desktop's SurfacePreference::resolve.
   */
  it("forces opaque surfaces for every preference, recommendation notwithstanding", () => {
    const frosted = findVariant("zeron-dark"); // recommended: frosted
    expect(frosted).toBeDefined();
    expect(frosted!.recommendedSurfaceTreatment).toBe("frosted");
    expect(SURFACE_PREFERENCES).toEqual(["themeDefault", "opaque"]);
    for (const surface of SURFACE_PREFERENCES) {
      expect(resolveSurfaceTreatment()).toBe("opaque");
      expect(surface).not.toBe("frosted");
    }
  });
});

describe("variantChoices", () => {
  it("offers both appearances in registry order", () => {
    // Desktop appearance.rs test: the registry has 10 light / 20 dark builtins.
    expect(variantChoices("light")).toHaveLength(10);
    expect(variantChoices("dark")).toHaveLength(20);
    expect(variantChoices("light").every((variant) => variant.appearance === "light")).toBe(true);
    expect(variantChoices("dark").every((variant) => variant.appearance === "dark")).toBe(true);
    expect(variantChoices("dark")[0]!.id).toBe("zeron-dark");
  });
});

describe("helper copy and swatches", () => {
  it("mirrors the desktop's accent helper text", () => {
    expect(accentHelper("themeDefault")).toBe("Theme default · Uses the palette's intended color.");
    expect(accentHelper("pink")).toBe("Pink · Controls, glyphs, selections, code, and activity.");
  });

  it("mirrors the desktop's surface helper text", () => {
    expect(surfaceHelper("themeDefault", "opaque")).toBe("Uses this theme's opaque default.");
    expect(surfaceHelper("opaque", "opaque")).toBe("Solid surfaces for every theme.");
  });

  it("picks the swatch color for the resolved appearance", () => {
    const dark = findVariant("zeron-dark")!;
    const light = findVariant("zeron-light")!;
    expect(accentSwatchColor("themeDefault", dark)).toBe(dark.accent.primary);
    expect(accentSwatchColor("pink", dark)).toBe("#f472b6");
    expect(accentSwatchColor("pink", light)).toBe("#be185d");
  });
});

describe("interface font (ticket 28)", () => {
  it("font_keyboard_navigation_stops_at_edges_and_skips_unavailable", () => {
    // The desktop's step_font clamps at both ends — stepping past the first
    // or last available choice is a no-op, never a wrap.
    expect(stepFont("geist", -1)).toBe("geist");
    expect(stepFont("geist", -3)).toBe("geist");
    expect(stepFont("system", 1)).toBe("system");
    expect(stepFont("system", 4)).toBe("system");
    expect(stepFont("geist", 1)).toBe("geistMono");
    expect(stepFont("geistMono", -1)).toBe("geist");
    expect(stepFont("geistMono", 1)).toBe("system");
    // A delta's magnitude collapses to one step (delta.signum()).
    expect(stepFont("geist", 2)).toBe("geistMono");
    // Zero and unknown values stay put.
    expect(stepFont("geist", 0)).toBe("geist");
    expect(stepFont("geistMono", 0)).toBe("geistMono");
  });

  it("resolves an installed-family request to the first available choice", () => {
    // resolve_effective's web form: no OS probe, so `installed:*` resolves
    // to Geist — and the error strip ("This font could not be loaded…")
    // is the surface that reports it.
    expect(effectiveUiFontFamily("geist")).toBe("geist");
    expect(effectiveUiFontFamily("geistMono")).toBe("geistMono");
    expect(effectiveUiFontFamily("system")).toBe("system");
    expect(effectiveUiFontFamily("installed:Inter")).toBe("geist");
    expect(effectiveUiFontFamily("inter")).toBe("geist");
  });
});

describe("terminal and code fonts (upstream #374)", () => {
  it("narrows the terminal catalog to the fixed-width choices", () => {
    // `only_the_terminal_catalog_is_narrowed_to_fixed_width`: no OS advance
    // probe exists on the web, so the bundled monospace is the one choice.
    expect(TERMINAL_FONT_CHOICES).toEqual(["geistMono"]);
    expect(CODE_FONT_CHOICES).toEqual(UI_FONT_CHOICES);
  });

  it("resolves a proportional terminal request to Geist Mono", () => {
    // `persisted_proportional_terminal_family_falls_back`.
    expect(effectiveTerminalFontFamily("geistMono")).toBe("geistMono");
    expect(effectiveTerminalFontFamily("geist")).toBe("geistMono");
    expect(effectiveTerminalFontFamily("system")).toBe("geistMono");
    expect(effectiveTerminalFontFamily("installed:Inter")).toBe("geistMono");
  });

  it("keeps the whole catalog for code, falling back to Geist Mono", () => {
    expect(effectiveCodeFontFamily("geist")).toBe("geist");
    expect(effectiveCodeFontFamily("geistMono")).toBe("geistMono");
    expect(effectiveCodeFontFamily("system")).toBe("system");
    expect(effectiveCodeFontFamily("installed:Inter")).toBe("geistMono");
  });

  it("mono_size_ladder_keeps_both_defaults_exactly_reachable", () => {
    expect(MONO_FONT_SIZES).toContain(13);
    expect(MONO_FONT_SIZES).toContain(12.5);
    // Off-ladder values (older settings, hand edits) snap, never drop.
    expect(nearestMonoFontSize(12.4)).toBe(12.5);
    expect(nearestMonoFontSize(13.4)).toBe(13);
    expect(nearestMonoFontSize(100)).toBe(20);
  });

  it("labels every rung in pixels, whole and fractional", () => {
    expect(MONO_FONT_SIZES.map(fontSizePxLabel)).toEqual([
      "10 px",
      "11 px",
      "12 px",
      "12.5 px",
      "13 px",
      "14 px",
      "15 px",
      "16 px",
      "18 px",
      "20 px",
    ]);
  });
});

describe("code surface scaling (lib/typography.ts)", () => {
  it("reproduces every surface's historical size at the default", () => {
    // 12.5 markdown code / 12.0 diff / 13.0 editor / 11.5 preview, exactly.
    expect(codeBlockTextSize(CODE_FONT_SIZE_DEFAULT)).toBe(12.5);
    expect(codeBlockLineHeight(CODE_FONT_SIZE_DEFAULT)).toBe(18);
    expect(diffTextSize(CODE_FONT_SIZE_DEFAULT)).toBe(12);
    expect(diffLineHeight(CODE_FONT_SIZE_DEFAULT)).toBe(21);
    expect(editorTextSize(CODE_FONT_SIZE_DEFAULT)).toBe(13);
    expect(previewTextSize(CODE_FONT_SIZE_DEFAULT)).toBe(11.5);
    expect(previewLineHeight(CODE_FONT_SIZE_DEFAULT)).toBe(20);
  });

  it("scaled sizes keep their proportions and stay clamped", () => {
    // `scaled_diff_sizes_keep_their_proportions_and_stay_clamped`.
    expect(diffTextSize(25)).toBe(24);
    expect(diffLineHeight(25)).toBe(42);
    expect(diffTextSize(FONT_SIZE_MAX)).toBeLessThanOrEqual(FONT_SIZE_MAX);
    expect(editorTextSize(4)).toBe(FONT_SIZE_MIN);
    expect(previewLineHeight(FONT_SIZE_MIN)).toBe(PREVIEW_LINE_HEIGHT_BASELINE);
  });
});

describe("theme import names (appearance.rs:404-460)", () => {
  it("sourceNameFallsBackToCustomTheme", () => {
    expect(sourceName("sunset-theme.json")).toBe("sunset-theme");
    expect(sourceName("My Theme.tar.gz")).toBe("My Theme.tar");
    expect(sourceName("no-extension")).toBe("no-extension");
    // A bare ".hidden" name keeps its leading dot (Rust file_stem).
    expect(sourceName(".hidden")).toBe(".hidden");
    // The package.json-parent-dir branch is dead on the web (a file input
    // gives a bare name); the stem is what remains.
    expect(sourceName("package.json")).toBe("package");
    expect(sourceName("")).toBe("Custom theme");
  });

  it("slugCollapsesNonAlphanumericRunsAndNeverEmpty", () => {
    expect(slug("My Cool Theme!")).toBe("my-cool-theme");
    expect(slug("  --Zeron--  ")).toBe("zeron");
    expect(slug("Tokyo Night 2")).toBe("tokyo-night-2");
    // Leading separators never produce a leading dash; interior runs
    // collapse to one.
    expect(slug("-abc")).toBe("abc");
    expect(slug("a!!b??c")).toBe("a-b-c");
    // Non-ASCII letters are separators, not slugs.
    expect(slug("Ünicode Ünicode")).toBe("nicode-nicode");
    expect(slug("!!!")).toBe("theme");
    expect(slug("")).toBe("theme");
  });
});

describe("new-thread background install/remove (settings.rs:305-386)", () => {
  it("backgroundInstallRejectsUndecodableImage", async () => {
    const settings = new UiSettingsStore({ storage: memoryStorage() });
    const blobs = memoryBackgroundBlobStore();
    // An SVG decodes as an attachment but never as a background — the
    // candidate is rejected before anything is staged.
    const file = new File(['<svg xmlns="http://www.w3.org/2000/svg"></svg>'], "bad.svg", {
      type: "image/svg+xml",
    });
    const error = await installNewThreadBackground(file, { settings, blobs });
    expect(error).toBe(BACKGROUND_UNSUPPORTED_MESSAGE);
    expect(settings.getSnapshot().newThreadComposerBackground).toBe(null);
    expect(blobs.snapshot()).toBe(null);
  });

  it("backgroundRemoveClearsFieldAndRetiresResource", async () => {
    const settings = new UiSettingsStore({ storage: memoryStorage() });
    const blobs = memoryBackgroundBlobStore();
    settings.updateImmediate({
      newThreadComposerBackground: { path: NEW_THREAD_BACKGROUND_IDB_PATH, name: "wall.png" },
    });
    await blobs.put(new Blob(["bytes"], { type: "image/png" }));
    expect(await blobs.url()).not.toBe(null);

    const error = await removeNewThreadBackground({ settings, blobs });
    expect(error).toBe(null);
    expect(settings.getSnapshot().newThreadComposerBackground).toBe(null);
    expect(blobs.snapshot()).toBe(null);
    expect(await blobs.url()).toBe(null);
    // Removing again is a no-op success (settings.rs:368-370).
    expect(await removeNewThreadBackground({ settings, blobs })).toBe(null);
  });

  it("resolves an installed background only while its resource lives", async () => {
    const settings = new UiSettingsStore({ storage: memoryStorage() });
    const blobs = memoryBackgroundBlobStore();
    settings.updateImmediate({
      newThreadComposerBackground: { path: NEW_THREAD_BACKGROUND_IDB_PATH, name: "wall.png" },
    });
    // Nothing staged yet: the row reads "Image unavailable", not the default.
    expect(await resolveInstalledBackground(settings.getSnapshot().newThreadComposerBackground, blobs)).toBe(null);
    await blobs.put(new Blob(["bytes"], { type: "image/png" }));
    expect(await resolveInstalledBackground(settings.getSnapshot().newThreadComposerBackground, blobs)).not.toBe(null);
    // Not installed at all resolves null too.
    expect(await resolveInstalledBackground(null, blobs)).toBe(null);
  });
});

describe("settings-page background row resolution (ticket 48)", () => {
  it("recognizes the default as selected and opens the effect gate", async () => {
    const settings = new UiSettingsStore({ storage: memoryStorage() });
    const blobs = memoryBackgroundBlobStore();
    // Fresh install: nothing stored — the row resolves the bundled default
    // the same way the canvas painter already does.
    const stored = settings.getSnapshot().newThreadComposerBackground;
    expect(stored).toBe(null);
    const resolved = await resolveActiveNewThreadBackground(
      stored,
      DEFAULT_NEW_THREAD_BACKGROUND_URL,
      blobs,
    );
    expect(resolved).toEqual({
      url: DEFAULT_NEW_THREAD_BACKGROUND_URL,
      name: "Zeron",
      isDefault: true,
    });
    const row = backgroundRowState(stored, resolved);
    expect(row.installed).toBe(true);
    // The effect row's gate: the default keeps it open.
    expect(row.available).toBe(true);
    expect(row.meta).toEqual(["Zeron", "Softened automatically on frosted themes."]);
  });

  it("keeps the user row unchanged and Image unavailable distinct", async () => {
    const settings = new UiSettingsStore({ storage: memoryStorage() });
    const blobs = memoryBackgroundBlobStore();
    settings.updateImmediate({
      newThreadComposerBackground: { path: NEW_THREAD_BACKGROUND_IDB_PATH, name: "wall.png" },
    });
    const stored = settings.getSnapshot().newThreadComposerBackground;
    await blobs.put(new Blob(["bytes"], { type: "image/png" }));
    const resolved = await resolveActiveNewThreadBackground(stored, DEFAULT_NEW_THREAD_BACKGROUND_URL, blobs);
    expect(resolved?.name).toBe("wall.png");
    expect(resolved?.isDefault).toBe(false);
    expect(backgroundRowState(stored, resolved).meta).toEqual([
      "wall.png",
      "Softened automatically on frosted themes.",
    ]);
    // Stored but broken: "Image unavailable", the effect gate closed.
    await blobs.delete();
    const broken = await resolveActiveNewThreadBackground(stored, DEFAULT_NEW_THREAD_BACKGROUND_URL, blobs);
    expect(broken).toBe(null);
    const row = backgroundRowState(stored, broken);
    expect(row.installed).toBe(true);
    expect(row.available).toBe(false);
    expect(row.meta).toEqual(["Image unavailable", "Choose a replacement or remove it."]);
  });

  it("falls back to the empty row only when nothing resolves", () => {
    const row = backgroundRowState(null, null);
    expect(row.installed).toBe(false);
    expect(row.available).toBe(false);
    expect(row.meta).toEqual(["Add an image behind the composer on empty new threads."]);
  });
});
