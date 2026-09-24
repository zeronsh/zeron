import { describe, expect, it } from "vitest";
import { findVariant } from "@zeron/theme";
import {
  findVariantAnywhere,
  parseThemeSource,
  rekeyFamily,
  reportLines,
  reportSummary,
  ThemeLibraryStore,
  uniqueId,
  variantsForAppearanceAll,
  type ThemeCompilation,
} from "../src/lib/theme-library";
import type { StorageLike } from "../src/lib/engine-store";

/**
 * The web custom-theme library — each describe named after the
 * `crates/theme/src/library.rs` behavior it mirrors (install/rekey/remove/
 * duplicate-as-snapshot) plus the parse pipeline ticket 28 specs (§2.11).
 */

function memoryStorage(): StorageLike & { dump(): Map<string, string> } {
  const map = new Map<string, string>();
  return {
    getItem: (key) => (map.has(key) ? map.get(key)! : null),
    setItem: (key, value) => void map.set(key, value),
    removeItem: (key) => void map.delete(key),
    dump: () => map,
  };
}

const SUNSET_SOURCE = JSON.stringify({
  name: "Sunset",
  variants: [
    {
      id: "sunset-light",
      name: "Sunset Light",
      appearance: "light",
      colors: { background: "#fffdf5", text: "#1c1917" },
    },
    { id: "sunset-dark", appearance: "dark", colors: { background: "#171310" } },
  ],
});

function sunsetCompilation(fileName: string | null = "sunset-theme.json"): ThemeCompilation {
  return parseThemeSource(SUNSET_SOURCE, "custom-sunset", "Sunset", fileName);
}

describe("parseThemeSource", () => {
  it("compiles every valid variant and fills un-authored roles from the builtin", () => {
    const compilation = sunsetCompilation();
    expect(compilation.family.variants).toHaveLength(2);
    expect(compilation.failures).toEqual([]);

    const light = compilation.family.variants[0]!;
    expect(light.colors.background).toBe("#fffdf5");
    // An un-authored role takes the same-appearance builtin's value.
    expect(light.colors.border).toBe(findVariant("zeron-light")!.colors.border);
    expect(light.appearance).toBe("light");

    const dark = compilation.family.variants[1]!;
    expect(dark.name).toBe("Sunset Dark");
    expect(dark.source.url).toBe("sunset-theme.json");

    // The fills land in the report as fallback lines.
    const report = compilation.reports.get("sunset-dark")!;
    expect(report.fallbacks.length).toBeGreaterThan(0);
    expect(reportSummary(report)).toContain("inferred/fallback");
    expect(reportLines(report)[0]).toContain("Fallback · ");
  });

  it("fails variants with an unusable appearance and throws when none compile", () => {
    const oneBad = parseThemeSource(
      JSON.stringify({
        name: "Mixed",
        variants: [
          { appearance: "sepia", name: "Broken" },
          { appearance: "dark", colors: { background: "#111111" } },
        ],
      }),
      "custom-mixed",
      "Mixed",
      null,
    );
    expect(oneBad.failures).toHaveLength(1);
    expect(oneBad.failures[0]!.message).toContain('appearance must be "dark" or "light"');
    expect(oneBad.family.variants).toHaveLength(1);

    expect(() =>
      parseThemeSource(JSON.stringify({ name: "Bad", variants: [{ appearance: "sepia" }] }), "custom-bad", "Bad", null),
    ).toThrow("No variant in the theme file could be compiled.");
  });

  it("rejects non-JSON and shapeless sources with the dialog's error text", () => {
    expect(() => parseThemeSource("{not json", "custom-x", "X", null)).toThrow("Could not read the theme file as JSON.");
    expect(() => parseThemeSource("[]", "custom-x", "X", null)).toThrow("The theme file must be a JSON object.");
    expect(() => parseThemeSource("{}", "custom-x", "X", null)).toThrow(
      'The theme file must carry a non-empty "variants" array.',
    );
  });

  it("renumbers duplicate variant ids and flags them as validation", () => {
    const compilation = parseThemeSource(
      JSON.stringify({
        name: "Twins",
        variants: [
          { id: "twins-dark", appearance: "dark" },
          { id: "twins-dark", appearance: "dark" },
        ],
      }),
      "custom-twins",
      "Twins",
      null,
    );
    expect(compilation.family.variants.map((variant) => variant.id)).toEqual(["twins-dark", "twins-dark-2"]);
    expect(compilation.reports.get("twins-dark-2")!.validation[0]).toContain("duplicated; renumbered");
  });

  it("flags low-contrast authoring the way ThemeRegistry::validate does", () => {
    const compilation = parseThemeSource(
      JSON.stringify({
        name: "Fog",
        variants: [
          {
            id: "fog-dark",
            appearance: "dark",
            colors: { background: "#0a0a0a", text: "#101010" },
          },
        ],
      }),
      "custom-fog",
      "Fog",
      null,
    );
    const report = compilation.reports.get("fog-dark")!;
    expect(report.validation.join("\n")).toContain("text contrast is");
    expect(report.validation.join("\n")).toContain("Validation Contrast Error");
  });
});

describe("ThemeLibraryStore", () => {
  it("installs a selection, persists through storage, and reloads", () => {
    const storage = memoryStorage();
    const store = new ThemeLibraryStore({ storage });
    const compilation = sunsetCompilation();
    const entryId = store.install(compilation, ["sunset-dark"]);

    expect(store.getSnapshot()).toHaveLength(1);
    expect(store.getSnapshot()[0]!.family.variants.map((variant) => variant.id)).toEqual(["sunset-dark"]);
    expect(store.getSnapshot()[0]!.name).toBe("Sunset");
    expect(entryId).toBe("custom-sunset");

    const reloaded = new ThemeLibraryStore({ storage });
    expect(reloaded.getSnapshot()[0]!.id).toBe("custom-sunset");
    expect(reloaded.getLoadWarning()).toBe(null);
  });

  it("rejects an empty selection with finish_import's message", () => {
    const store = new ThemeLibraryStore({ storage: memoryStorage() });
    expect(() => store.install(sunsetCompilation(), [])).toThrow("Select at least one variant to import.");
  });

  it("rekeys a second import of the same family id (unique_id + rekey_family)", () => {
    const store = new ThemeLibraryStore({ storage: memoryStorage() });
    const compilation = sunsetCompilation();
    store.install(compilation, ["sunset-light", "sunset-dark"]);
    const second = store.install(sunsetCompilation(), ["sunset-dark"]);

    expect(second).toBe("custom-sunset-2");
    const entry = store.getSnapshot().find((candidate) => candidate.id === second)!;
    expect(entry.family.id).toBe("custom-sunset-2");
    // rekey_family: strip the old family prefix, else `{new_id}-{variant_id}`
    // — the file's ids carry no family prefix, so they compose.
    expect(entry.family.variants[0]!.id).toBe("custom-sunset-2-sunset-dark");
    expect(entry.family.variants[0]!.familyId).toBe("custom-sunset-2");
    // The report follows the rekeyed id.
    expect(entry.reports.has("custom-sunset-2-sunset-dark")).toBe(true);
  });

  it("duplicate copies the family and remove drops it", () => {
    const store = new ThemeLibraryStore({ storage: memoryStorage() });
    store.install(sunsetCompilation(), ["sunset-light", "sunset-dark"]);
    const duplicateId = store.duplicate("custom-sunset");

    const duplicate = store.getSnapshot().find((entry) => entry.id === duplicateId)!;
    expect(duplicate.name).toBe("Sunset Copy");
    expect(duplicate.family.variants.map((variant) => variant.id)).toEqual([
      "custom-sunset-copy-sunset-light",
      "custom-sunset-copy-sunset-dark",
    ]);

    expect(store.remove("custom-sunset")).toBe(true);
    expect(store.getSnapshot()).toHaveLength(1);
    expect(store.remove("custom-sunset")).toBe(false);
  });

  it("merges custom variants into the registry overlay after the builtins", () => {
    const storage = memoryStorage();
    const store = new ThemeLibraryStore({ storage });
    store.install(sunsetCompilation(), ["sunset-dark"]);

    expect(findVariantAnywhere("sunset-dark", store)).toBeDefined();
    expect(findVariantAnywhere("zeron-dark", store)!.id).toBe("zeron-dark");
    const dark = variantsForAppearanceAll("dark", store);
    expect(dark.map((variant) => variant.id)).toContain("sunset-dark");
    // Builtins first, custom appended (ThemeRegistry::active's order).
    expect(dark[0]!.id).toBe("zeron-dark");
    expect(dark[dark.length - 1]!.id).toBe("sunset-dark");
  });

  it("surfaces an unreadable library as a load warning and heals to empty", () => {
    const storage = memoryStorage();
    storage.setItem("zeron.theme-library.v1", "{corrupt");
    const store = new ThemeLibraryStore({ storage });
    expect(store.getSnapshot()).toEqual([]);
    expect(store.getLoadWarning()).not.toBe(null);
  });
});

describe("pure helpers", () => {
  it("uniqueId appends a numeric suffix past the first clash", () => {
    expect(uniqueId("base", [])).toBe("base");
    expect(uniqueId("base", ["base"])).toBe("base-2");
    expect(uniqueId("base", ["base", "base-2"])).toBe("base-3");
  });

  it("rekeyFamily rewrites ids under a new family id", () => {
    const family = {
      id: "old",
      name: "Old",
      variants: [
        { id: "old-light", familyId: "old" },
        { id: "unrelated", familyId: "old" },
      ],
    } as unknown as Parameters<typeof rekeyFamily>[0];
    const rekeyed = rekeyFamily(family, "new");
    expect(rekeyed.id).toBe("new");
    expect(rekeyed.variants.map((variant) => variant.id)).toEqual(["new-light", "new-unrelated"]);
    expect(rekeyed.variants.every((variant) => variant.familyId === "new")).toBe(true);
  });
});
