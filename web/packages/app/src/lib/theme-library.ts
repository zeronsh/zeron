import {
  colorContrast,
  findVariant,
  isHexColor,
  themeVariants,
  type Appearance,
  type ThemeFamily,
  type ThemeVariant,
} from "@zeron/theme";
import type { StorageLike } from "./engine-store";

/**
 * The web custom-theme library — the peer of the desktop's
 * `crates/theme/src/library.rs` (`CustomThemeLibrary`) as scoped by ticket 28
 * §2.11: snapshot import only. Link/Reload/Reveal need a durable, linkable OS
 * path a browser does not have, so they stay absent (the ticket's §5 "Do
 * not").
 *
 * Scope cuts vs the desktop, deliberate:
 * - **Native family JSON, not VS Code packages.** The desktop's
 *   `theme_library::compile` rides `zeron_theme::vscode` (~1600 lines of
 *   token-color mapping). The web accepts a Zeron-family JSON file
 *   (`{ name, variants: [...] }` — the same shape `ThemeFamily` serializes
 *   to, i.e. the desktop's editable-file format); that keeps the whole
 *   dialog/select/review/manage pipeline real without porting the compiler.
 * - **`localStorage`, not a data dir.** The desktop persists
 *   `theme-library.json` next to `ui-settings.json`; the browser equivalent
 *   is one sibling key (`zeron.theme-library.v1`) holding the compiled
 *   families — JSON, like the settings store, so the load stays synchronous.
 *   Binary assets (the new-thread background image) are the ones that go to
 *   IndexedDB.
 * - **Per-entry warnings do not exist.** They come from linked-source
 *   reloads, which are desktop-only.
 *
 * The registry overlay (`registryVariants`/`findVariantAnywhere`/
 * `variantsForAppearanceAll`) mirrors `ThemeRegistry::active()` — builtins
 * first, custom families appended — so imported variants appear in the
 * appearance selectors and survive a reload.
 */

// ---------------------------------------------------------------------------
// Constants (library.rs module level; the byte cap is web-adjusted)
// ---------------------------------------------------------------------------

export const THEME_LIBRARY_STORAGE_KEY = "zeron.theme-library.v1";

/** library.rs `MAX_LIBRARY_ENTRIES`. */
export const MAX_LIBRARY_ENTRIES = 256;
/** library.rs `MAX_LIBRARY_VARIANTS`. */
export const MAX_LIBRARY_VARIANTS = 1024;
/**
 * library.rs caps the durable file at 16 MiB; `localStorage` quotas sit
 * around 5 MiB per origin and are shared with the settings store, so the web
 * cap is a quarter of the desktop's.
 */
export const MAX_LIBRARY_BYTES = 4 * 1024 * 1024;

// ---------------------------------------------------------------------------
// The model
// ---------------------------------------------------------------------------

/** The import mapping/validation log (vscode.rs `ImportReport`, web shape). */
export interface ThemeImportReport {
  /** How many leaf roles came straight from the file. */
  readonly mappings: number;
  /** `"Adjusted · …"` lines (remapping; native files produce none). */
  readonly adjustments: readonly string[];
  /** `"Fallback · …"` lines (roles filled from the builtin base). */
  readonly fallbacks: readonly string[];
  readonly warnings: readonly string[];
  /** Validation lines, the desktop's `"Validation {category} {severity} · {message}"`. */
  readonly validation: readonly string[];
  /** `"Unsupported · …"` lines (unknown top-level keys). */
  readonly dropped: readonly string[];
}

/** One variant the compiler could not build (vscode.rs failures). */
export interface ThemeCompilationFailure {
  readonly name: string;
  readonly message: string;
}

/** A parsed source, ready to install (vscode.rs `SourceCompilation`). */
export interface ThemeCompilation {
  readonly family: ThemeFamily;
  readonly reports: ReadonlyMap<string, ThemeImportReport>;
  readonly failures: readonly ThemeCompilationFailure[];
  /** The file the import dialog read (the snapshot's provenance). */
  readonly fileName: string | null;
}

/** One installed library entry (library.rs `CustomThemeEntry`, web shape). */
export interface ThemeLibraryEntry {
  readonly id: string;
  readonly name: string;
  readonly family: ThemeFamily;
  readonly reports: ReadonlyMap<string, ThemeImportReport>;
  readonly selectedVariantIds: readonly string[];
  /** `CustomThemeSource::ImportedSnapshot.imported_from`. */
  readonly importedFrom: string | null;
}

// ---------------------------------------------------------------------------
// Pure helpers (appearance.rs:404-460, ported verbatim)
// ---------------------------------------------------------------------------

/** `source_name` (appearance.rs:428-439). The package.json branch is dead on
 *  the web — a file input gives a bare name, no parent directory — but the
 *  shape stays so the day a folder picker exists the rule is already here. */
export function sourceName(path: string): string {
  const segments = path.split(/[\\/]/).filter((segment) => segment.length > 0);
  const fileName = segments[segments.length - 1] ?? path;
  const base = fileName === "package.json" ? segments[segments.length - 2] ?? fileName : fileName;
  const stem = fileStem(base);
  return stem === "" ? "Custom theme" : stem;
}

/** Rust `Path::file_stem`: the name minus the final extension; a leading dot
 *  is part of the name, not an extension separator. */
function fileStem(name: string): string {
  const dot = name.lastIndexOf(".");
  if (dot <= 0) {
    return name;
  }
  return name.slice(0, dot);
}

/** `slug` (appearance.rs:441-460): lowercase, keep ASCII alphanumerics,
 *  collapse other runs to one interior `-`, empty → "theme". */
export function slug(value: string): string {
  let result = "";
  let separator = false;
  for (const character of value.toLowerCase()) {
    if (character >= "a" && character <= "z") {
      if (separator && result.length > 0) {
        result += "-";
      }
      result += character;
      separator = false;
    } else if (character >= "0" && character <= "9") {
      if (separator && result.length > 0) {
        result += "-";
      }
      result += character;
      separator = false;
    } else {
      separator = true;
    }
  }
  return result === "" ? "theme" : result;
}

/** library.rs `unique_id`: the base, or `{base}-{n}` past the first clash. */
export function uniqueId(base: string, existing: readonly string[]): string {
  if (!existing.includes(base)) {
    return base;
  }
  for (let suffix = 2; ; suffix += 1) {
    const candidate = `${base}-${suffix}`;
    if (!existing.includes(candidate)) {
      return candidate;
    }
  }
}

/** library.rs `rekey_family`: rewrite family+variant ids under a new entry id. */
export function rekeyFamily(family: ThemeFamily, newId: string): ThemeFamily {
  const oldId = family.id;
  return {
    ...family,
    id: newId,
    variants: family.variants.map((variant) => ({
      ...variant,
      familyId: newId,
      id: variant.id.startsWith(oldId)
        ? `${newId}${variant.id.slice(oldId.length)}`
        : `${newId}-${variant.id}`,
    })),
  };
}

/** `"{n} mapped · {n} adjusted · {n} inferred/fallback · {n} unsupported · {n} warnings · {n} validation"` (appearance.rs:868-877). */
export function reportSummary(report: ThemeImportReport): string {
  return (
    `${report.mappings} mapped · ${report.adjustments.length} adjusted · ` +
    `${report.fallbacks.length} inferred/fallback · ${report.dropped.length} unsupported · ` +
    `${report.warnings.length} warnings · ${report.validation.length} validation`
  );
}

/** The report panel's lines after the summary (appearance.rs:879-929's order). */
export function reportLines(report: ThemeImportReport): readonly string[] {
  return [
    ...report.adjustments.map((line) => `Adjusted · ${line}`),
    ...report.fallbacks.map((line) => `Fallback · ${line}`),
    ...report.warnings.map((line) => `Warning · ${line}`),
    ...report.validation,
    ...report.dropped.map((line) => `Unsupported · ${line}`),
  ];
}

// ---------------------------------------------------------------------------
// Source parsing (the web's compile_source)
// ---------------------------------------------------------------------------

/**
 * Compile a Zeron-family JSON file into an installable family: every
 * variant is validated (appearance, object shape), missing roles are filled
 * from the builtin base of that appearance (each fill a "Fallback" report
 * line), unknown keys are surfaced as warnings/unsupported lines, and the
 * desktop's non-structural contrast checks run as validation lines.
 * Structural problems (empty/duplicate ids, a family with no valid variant)
 * throw — the dialog renders the message as its error strip.
 */
export function parseThemeSource(text: string, familyId: string, familyName: string, fileName: string | null): ThemeCompilation {
  let parsed: unknown;
  try {
    parsed = JSON.parse(text) as unknown;
  } catch {
    throw new Error("Could not read the theme file as JSON.");
  }
  if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
    throw new Error("The theme file must be a JSON object.");
  }
  const raw = parsed as Record<string, unknown>;
  const rawVariants = raw.variants;
  if (!Array.isArray(rawVariants) || rawVariants.length === 0) {
    throw new Error('The theme file must carry a non-empty "variants" array.');
  }

  const variants: ThemeVariant[] = [];
  const failures: ThemeCompilationFailure[] = [];
  const reports = new Map<string, ThemeImportReport>();
  const usedIds = new Set<string>();
  for (let index = 0; index < rawVariants.length; index += 1) {
    const fallbackName = `Variant ${index + 1}`;
    const candidate = rawVariants[index];
    if (typeof candidate !== "object" || candidate === null || Array.isArray(candidate)) {
      failures.push({ name: fallbackName, message: "the variant entry must be an object" });
      continue;
    }
    const compiled = compileVariant(candidate as Record<string, unknown>, familyId, familyName, usedIds, fileName ?? familyId);
    if (compiled === null) {
      const name = typeof (candidate as Record<string, unknown>).name === "string"
        ? ((candidate as Record<string, unknown>).name as string)
        : fallbackName;
      failures.push({ name, message: 'appearance must be "dark" or "light"' });
      continue;
    }
    variants.push(compiled.variant);
    reports.set(compiled.variant.id, compiled.report);
  }
  if (variants.length === 0) {
    throw new Error("No variant in the theme file could be compiled.");
  }
  return {
    family: { id: familyId, name: familyName, variants },
    reports,
    failures,
    fileName,
  };
}

function compileVariant(
  raw: Record<string, unknown>,
  familyId: string,
  familyName: string,
  usedIds: Set<string>,
  sourceUrl: string,
): { variant: ThemeVariant; report: ThemeImportReport } | null {
  if (raw.appearance !== "dark" && raw.appearance !== "light") {
    return null;
  }
  const appearance: Appearance = raw.appearance;
  const base = findVariant(appearance === "dark" ? "zeron-dark" : "zeron-light");
  if (base === undefined) {
    throw new Error("the builtin registry is missing its base variant");
  }

  let mappings = 0;
  const fallbacks: string[] = [];
  const warnings: string[] = [];
  const validation: string[] = [];
  const dropped: string[] = [];

  const name = typeof raw.name === "string" && raw.name.trim() !== "" ? raw.name : `${familyName} ${appearance === "dark" ? "Dark" : "Light"}`;
  let id = typeof raw.id === "string" && raw.id.trim() !== "" ? raw.id : `${familyId}-${appearance}`;
  if (usedIds.has(id)) {
    validation.push(`Validation Structural Error · variant id "${id}" is duplicated; renumbered`);
    id = uniqueId(id, [...usedIds]);
  }
  usedIds.add(id);

  const fill = (source: unknown, baseValue: string, path: string): { value: string; authored: boolean } => {
    if (typeof source === "string" && isHexColor(source)) {
      return { value: source, authored: true };
    }
    if (typeof source === "string") {
      warnings.push(`"${path}" is not a #rrggbb color; filled from the builtin`);
    }
    fallbacks.push(`${path} filled from the ${base.id} builtin`);
    return { value: baseValue, authored: false };
  };

  const colors: Record<string, string> = {};
  const baseColors = { ...base.colors };
  for (const key of Object.keys(baseColors)) {
    const filled = fill((raw.colors as Record<string, unknown> | undefined)?.[key], baseColors[key as keyof typeof baseColors], `colors.${key}`);
    colors[key] = filled.value;
    if (filled.authored) {
      mappings += 1;
    }
  }
  if (raw.colors !== undefined && typeof raw.colors === "object" && raw.colors !== null) {
    for (const key of Object.keys(raw.colors)) {
      if (!(key in base.colors)) {
        dropped.push(`colors.${key}`);
      }
    }
  }

  const accentRaw = (raw.accent as Record<string, unknown> | undefined) ?? {};
  const baseAccent = base.accent as unknown as Record<string, unknown>;
  const accent: Record<string, unknown> = {};
  for (const key of Object.keys(baseAccent)) {
    if (key === "glyph") {
      const rawGlyph = accentRaw.glyph;
      if (Array.isArray(rawGlyph) && rawGlyph.length === 3 && rawGlyph.every((slot) => typeof slot === "string" && isHexColor(slot))) {
        accent.glyph = rawGlyph as unknown;
        mappings += 3;
      } else {
        fallbacks.push(`accent.glyph filled from the ${base.id} builtin`);
        accent.glyph = baseAccent.glyph;
      }
    } else {
      const filled = fill(accentRaw[key], baseAccent[key] as string, `accent.${key}`);
      accent[key] = filled.value;
      if (filled.authored) {
        mappings += 1;
      }
    }
  }

  const syntaxRaw = (raw.syntax as Record<string, unknown> | undefined) ?? {};
  const syntax: Record<string, string> = {};
  const baseSyntax = { ...base.syntax };
  for (const key of Object.keys(baseSyntax)) {
    const filled = fill(syntaxRaw[key], baseSyntax[key]!, `syntax.${key}`);
    syntax[key] = filled.value;
    if (filled.authored) {
      mappings += 1;
    }
  }

  const terminalRaw = (raw.terminal as Record<string, unknown> | undefined) ?? {};
  const baseTerminal = { ...base.terminal } as unknown as Record<string, string>;
  const terminal: Record<string, unknown> = {};
  for (const key of ["background", "foreground", "selection"] as const) {
    const filled = fill(terminalRaw[key], baseTerminal[key]!, `terminal.${key}`);
    terminal[key] = filled.value;
    if (filled.authored) {
      mappings += 1;
    }
  }
  const rawAnsi = terminalRaw.ansi;
  if (Array.isArray(rawAnsi) && rawAnsi.length === 16 && rawAnsi.every((slot) => typeof slot === "string" && isHexColor(slot))) {
    terminal.ansi = rawAnsi;
    mappings += 16;
  } else {
    fallbacks.push(`terminal.ansi filled from the ${base.id} builtin`);
    terminal.ansi = base.terminal.ansi;
  }

  const recommendedSurfaceTreatment =
    raw.recommendedSurfaceTreatment === "frosted" || raw.recommendedSurfaceTreatment === "opaque"
      ? raw.recommendedSurfaceTreatment
      : base.recommendedSurfaceTreatment;

  for (const key of Object.keys(raw)) {
    if (!["id", "name", "appearance", "colors", "accent", "syntax", "terminal", "recommendedSurfaceTreatment"].includes(key)) {
      dropped.push(key);
    }
  }

  // The desktop's contrast checks (ThemeRegistry::validate, non-blocking).
  const check = (role: string, foreground: string, background: string, minimum: number): void => {
    const ratio = colorContrast(foreground, background);
    if (ratio !== null && ratio < minimum) {
      validation.push(`Validation Contrast Error · ${role} contrast is ${ratio.toFixed(2)}:1; expected ${minimum.toFixed(1)}:1`);
    }
  };
  check("text", colors.text!, colors.background!, 4.5);
  check("muted text", colors.textMuted!, colors.background!, 4.5);
  check("accent", accent.primary as string, colors.background!, 3);
  check("on-accent", accent.on as string, accent.strong as string, 4.5);
  check("terminal foreground", terminal.foreground as string, terminal.background as string, 4.5);

  const variant: ThemeVariant = {
    id,
    familyId,
    name,
    appearance,
    recommendedSurfaceTreatment,
    colors: colors as unknown as ThemeVariant["colors"],
    accent: accent as unknown as ThemeVariant["accent"],
    syntax,
    terminal: terminal as unknown as ThemeVariant["terminal"],
    source: {
      format: "zeron-family",
      url: sourceUrl,
      revision: "local",
      license: "User supplied",
      assetHash: "sha256:imported",
    },
  };
  return { variant, report: { mappings, adjustments: [], fallbacks, warnings, validation, dropped } };
}

/** Structural (blocking) validation (ThemeRegistry::validate + is_blocking). */
export function validationErrors(family: ThemeFamily): readonly string[] {
  const issues: string[] = [];
  const ids = new Set<string>();
  for (const variant of family.variants) {
    if (variant.id.trim() === "" || !ids.add(variant.id)) {
      issues.push(`${variant.id}: variant id must be unique`);
    }
  }
  return issues;
}

// ---------------------------------------------------------------------------
// The store
// ---------------------------------------------------------------------------

export interface ThemeLibraryStoreOptions {
  readonly storage?: StorageLike;
}

function memoryStorage(): StorageLike {
  const map = new Map<string, string>();
  return {
    getItem: (key) => (map.has(key) ? map.get(key)! : null),
    setItem: (key, value) => void map.set(key, value),
    removeItem: (key) => void map.delete(key),
  };
}

function defaultStorage(): StorageLike {
  try {
    const candidate = (globalThis as { localStorage?: StorageLike }).localStorage;
    return candidate ?? memoryStorage();
  } catch {
    return memoryStorage();
  }
}

/**
 * The durable library: `install`/`duplicate`/`remove` mutate, save, and
 * notify — the desktop's `persist_and_activate` shape (every mutation writes
 * the whole library back and re-derives the runtime registry).
 */
export class ThemeLibraryStore {
  readonly #storage: StorageLike;
  #entries: readonly ThemeLibraryEntry[] = [];
  #loadWarning: string | null = null;
  readonly #listeners = new Set<() => void>();

  constructor(options: ThemeLibraryStoreOptions = {}) {
    this.#storage = options.storage ?? defaultStorage();
    let source: string | null = null;
    try {
      source = this.#storage.getItem(THEME_LIBRARY_STORAGE_KEY);
    } catch {
      source = null;
    }
    if (source === null) {
      return;
    }
    try {
      this.#entries = healEntries(JSON.parse(source) as unknown);
    } catch (error) {
      // The desktop keeps the last known good library and surfaces the load
      // failure; an unreadable key heals to empty with the same warning.
      this.#entries = [];
      this.#loadWarning = error instanceof Error ? error.message : String(error);
    }
  }

  // Arrow-function properties (the UiSettingsStore pattern): React's
  // useSyncExternalStore calls these as detached function references, so
  // method syntax would lose `this`.
  getSnapshot = (): readonly ThemeLibraryEntry[] => this.#entries;

  /** The load-time failure, rendered verbatim under the font block. */
  getLoadWarning = (): string | null => this.#loadWarning;

  subscribe = (listener: () => void): (() => void) => {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  };

  /** `finish_import` (appearance.rs:404-425) + `library.install`. */
  install(compilation: ThemeCompilation, selectedVariantIds: readonly string[]): string {
    if (selectedVariantIds.length === 0) {
      throw new Error("Select at least one variant to import.");
    }
    const selected = new Set(selectedVariantIds);
    let family: ThemeFamily = {
      ...compilation.family,
      variants: compilation.family.variants.filter((variant) => selected.has(variant.id)),
    };
    if (family.variants.length === 0) {
      throw new Error("select at least one successfully compiled variant");
    }
    const errors = validationErrors(family);
    if (errors.length > 0) {
      throw new Error(`theme validation failed: ${errors.join("; ")}`);
    }
    const entryId = uniqueId(family.id, this.#entries.map((entry) => entry.id));
    const oldIds = family.variants.map((variant) => variant.id);
    if (entryId !== family.id) {
      family = rekeyFamily(family, entryId);
    }
    const reports = new Map<string, ThemeImportReport>();
    oldIds.forEach((oldId, ix) => {
      const report = compilation.reports.get(oldId);
      if (report !== undefined) {
        reports.set(family.variants[ix]!.id, report);
      }
    });
    this.#push({
      id: entryId,
      name: family.name,
      family,
      reports,
      selectedVariantIds: family.variants.map((variant) => variant.id),
      importedFrom: compilation.fileName,
    });
    return entryId;
  }

  /** library.rs `duplicate_as_snapshot` (the web's only duplicate flavor). */
  duplicate(id: string): string {
    const original = this.#entries.find((entry) => entry.id === id);
    if (original === undefined) {
      throw new Error(`unknown custom theme \`${id}\``);
    }
    const newId = uniqueId(`${original.id}-copy`, this.#entries.map((entry) => entry.id));
    const oldIds = original.family.variants.map((variant) => variant.id);
    const family = rekeyFamily({ ...original.family, name: `${original.name} Copy` }, newId);
    const reports = new Map<string, ThemeImportReport>();
    oldIds.forEach((oldId, ix) => {
      const report = original.reports.get(oldId);
      if (report !== undefined) {
        reports.set(family.variants[ix]!.id, report);
      }
    });
    this.#push({
      id: newId,
      name: `${original.name} Copy`,
      family,
      reports,
      selectedVariantIds: family.variants.map((variant) => variant.id),
      importedFrom: original.importedFrom,
    });
    return newId;
  }

  /** library.rs `remove`. */
  remove(id: string): boolean {
    const before = this.#entries.length;
    this.#entries = this.#entries.filter((entry) => entry.id !== id);
    if (this.#entries.length === before) {
      return false;
    }
    this.#save();
    this.#notify();
    return true;
  }

  #push(entry: ThemeLibraryEntry): void {
    const entries = [...this.#entries, entry];
    if (entries.length > MAX_LIBRARY_ENTRIES) {
      throw new Error(`custom theme library is limited to ${MAX_LIBRARY_ENTRIES} entries`);
    }
    const variants = entries.reduce((count, item) => count + item.family.variants.length, 0);
    if (variants > MAX_LIBRARY_VARIANTS) {
      throw new Error(`custom theme library is limited to ${MAX_LIBRARY_VARIANTS} variants`);
    }
    this.#entries = entries;
    this.#save();
    this.#notify();
  }

  #save(): void {
    const serialized = JSON.stringify({
      version: 1,
      entries: this.#entries.map(serializeEntry),
    });
    if (serialized.length > MAX_LIBRARY_BYTES) {
      throw new Error(`custom theme library exceeds the ${MAX_LIBRARY_BYTES}-byte limit`);
    }
    try {
      this.#storage.setItem(THEME_LIBRARY_STORAGE_KEY, serialized);
    } catch (error) {
      throw new Error("could not save the custom theme library", { cause: error });
    }
  }

  #notify(): void {
    for (const listener of this.#listeners) {
      listener();
    }
  }
}

function serializeEntry(entry: ThemeLibraryEntry): Record<string, unknown> {
  return {
    id: entry.id,
    name: entry.name,
    importedFrom: entry.importedFrom,
    selectedVariantIds: [...entry.selectedVariantIds],
    family: entry.family,
    reports: Object.fromEntries([...entry.reports].map(([id, report]) => [id, report])),
  };
}

/** Per-entry healing on load: keep well-shaped entries, drop the rest. */
function healEntries(value: unknown): readonly ThemeLibraryEntry[] {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new Error("the custom theme library is not a JSON object");
  }
  const raw = value as Record<string, unknown>;
  if (!Array.isArray(raw.entries)) {
    throw new Error('the custom theme library has no "entries" array');
  }
  const entries: ThemeLibraryEntry[] = [];
  for (const candidate of raw.entries) {
    if (typeof candidate !== "object" || candidate === null) {
      continue;
    }
    const entry = candidate as Record<string, unknown>;
    const family = entry.family;
    if (
      typeof entry.id !== "string" ||
      typeof entry.name !== "string" ||
      typeof family !== "object" ||
      family === null ||
      !Array.isArray((family as Record<string, unknown>).variants)
    ) {
      continue;
    }
    const reports = new Map<string, ThemeImportReport>();
    if (typeof entry.reports === "object" && entry.reports !== null && !Array.isArray(entry.reports)) {
      for (const [id, report] of Object.entries(entry.reports)) {
        if (typeof report === "object" && report !== null && !Array.isArray(report)) {
          reports.set(id, report as unknown as ThemeImportReport);
        }
      }
    }
    entries.push({
      id: entry.id,
      name: entry.name,
      family: family as ThemeFamily,
      reports,
      selectedVariantIds: Array.isArray(entry.selectedVariantIds)
        ? entry.selectedVariantIds.filter((id): id is string => typeof id === "string")
        : [],
      importedFrom: typeof entry.importedFrom === "string" ? entry.importedFrom : null,
    });
  }
  return entries;
}

// ---------------------------------------------------------------------------
// The registry overlay (ThemeRegistry::active / replace_custom_families)
// ---------------------------------------------------------------------------

/** The one library for this page load; the overlay reads it by default. */
export const themeLibrary = new ThemeLibraryStore();

/** Builtins plus installed custom families, registry order (registry.active()). */
export function registryVariants(library: ThemeLibraryStore = themeLibrary): readonly ThemeVariant[] {
  return [...themeVariants, ...library.getSnapshot().flatMap((entry) => entry.family.variants)];
}

/** `registry.variant(id)` over the same builtin+custom union. */
export function findVariantAnywhere(id: string, library: ThemeLibraryStore = themeLibrary): ThemeVariant | undefined {
  return registryVariants(library).find((variant) => variant.id === id);
}

/** `registry.variants_for(appearance)` over the same union. */
export function variantsForAppearanceAll(
  appearance: Appearance,
  library: ThemeLibraryStore = themeLibrary,
): readonly ThemeVariant[] {
  return registryVariants(library).filter((variant) => variant.appearance === appearance);
}
