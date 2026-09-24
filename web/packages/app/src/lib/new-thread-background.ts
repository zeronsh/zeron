/**
 * The new-thread background hero's pure geometry — the web port of
 * `crates/ui/src/shell.rs:694-914` (`new_thread_background`,
 * `new_thread_background_height`, `new_thread_background_opacity`) and
 * `crates/ui/src/new_thread_background_mask.rs` (the two-pass feathered
 * cutout) plus `new_thread_background_effects.rs::Readiness` (the 120 ms
 * artwork fade-in).
 *
 * Everything is expressed in the shared window space, exactly like the
 * desktop's `Bounds<Pixels>`: the hero rect and the composer's measured
 * surface rect are both viewport coordinates, so the mask geometry ports
 * 1:1 (`tests/new-thread-background.test.ts` mirrors the mask module's
 * tests). The component converts to hero-local pixels only when it emits
 * CSS.
 *
 * The cutout hole is the desktop's exact per-pixel shader (the gpui
 * `ImageAlphaMask` fragment shader over `mask.rs`'s geometry): a hard 8 px
 * transparent margin around the cleared rounded rect, then a one-sided
 * smoothstep dome over `clamp(hero height × 0.52, 120, 280)`, composed with
 * the shared bottom fade by `min` inside ONE mask — never by multiplying
 * two element masks (mid-ramp `a·b < min(a,b)`, so multiplication darkens
 * the hole's edges near the hero's bottom).
 *
 * This module references no theme roles at all: the mask multiplies source
 * alpha and nothing else, so the artwork resolves into the real canvas
 * (translucent themes included) with no theme-coloured overlay bleaching or
 * darkening it.
 */

import type { SurfaceTreatment } from "@zeron/theme";
import type { UiSettingsStore } from "../state/ui-settings";
import { clamp } from "./new-thread-background-effects";
import { idbBackgroundBlobStore, type BackgroundBlobStore } from "./background-blob-store";

// ---------------------------------------------------------------------------
// Constants (shell.rs:694-699, mask.rs:8)
// ---------------------------------------------------------------------------

/** Frosted themes show the hero at 0.84; opaque themes at 1.0. */
export const NEW_THREAD_BACKGROUND_FROSTED_OPACITY = 0.84;
/** The hero covers the top 72% of the viewport… */
export const NEW_THREAD_BACKGROUND_VIEWPORT_RATIO = 0.72;
/** …but never more than 760px. */
export const NEW_THREAD_BACKGROUND_MAX_HEIGHT = 760;
/** The reveal pass's opacity (mask.rs:8): half-strength artwork softening the cutout's contrast. */
export const CUTOUT_REVEAL_OPACITY = 0.5;
/** The pill's corner radius — `COMPOSER_RADIUS` (composer.rs:62). */
export const HERO_MASK_RADIUS = 26;
/** Hard transparency margin around the composer's rounded rect (mask.rs:41). */
export const HERO_MASK_CLEARANCE = 8;
/** The reveal pass's feather — 1px, a no-op ramp. */
export const REVEAL_FEATHER = 1;

/** A rect in the shared window space. */
export interface Rect {
  readonly x: number;
  readonly y: number;
  readonly width: number;
  readonly height: number;
}

// ---------------------------------------------------------------------------
// Geometry (shell.rs:844-855)
// ---------------------------------------------------------------------------

/** `new_thread_background_opacity` (shell.rs:844-850). */
export function newThreadBackgroundOpacity(isFrost: boolean): number {
  return isFrost ? NEW_THREAD_BACKGROUND_FROSTED_OPACITY : 1;
}

/**
 * The hero ELEMENT's opacity (shell.rs:880, 5860-5864, 5893):
 * `(1 − dissolve) × artwork_readiness × new_thread_background_opacity(is_frost)`
 * — the multiplier is 0.84 under a resolved frosted surface, 1.0 under an
 * opaque one. The component splits the product across two elements — the
 * root carries `(1 − dissolve) × multiplier`, the readiness wrapper the
 * fade — which composes back to exactly this because nested CSS opacities
 * multiply.
 */
export function newThreadBackgroundElementOpacity(
  dissolve: number,
  readiness: number,
  surface: SurfaceTreatment,
): number {
  const settled = 1 - clamp(dissolve, 0, 1);
  return settled * readiness * newThreadBackgroundOpacity(surface === "frosted");
}

/** `new_thread_background_height` (shell.rs:852-855): `min(max(vh,0)·0.72, 760)`. */
export function newThreadBackgroundHeight(viewportHeight: number): number {
  return clamp(
    viewportHeight * NEW_THREAD_BACKGROUND_VIEWPORT_RATIO,
    0,
    NEW_THREAD_BACKGROUND_MAX_HEIGHT,
  );
}

// ---------------------------------------------------------------------------
// The mask geometry (new_thread_background_mask.rs:12-47)
// ---------------------------------------------------------------------------

/** The per-pass mask parameters, ported field for field. */
export interface HeroMaskGeometry {
  /** The mask's own rect (the cleared hole for the cutout pass; parked below the image for the reveal). */
  readonly bounds: Rect;
  readonly radius: number;
  readonly feather: number;
  readonly clearance: number;
  /** Shared by both passes: `(hero.bottom, hero height)`. */
  readonly bottomFade: { readonly end: number; readonly height: number };
}

function bottom(rect: Rect): number {
  return rect.y + rect.height;
}

function rectEquals(a: Rect, b: Rect): boolean {
  return a.x === b.x && a.y === b.y && a.width === b.width && a.height === b.height;
}

/**
 * `mask(hero, composer, cutout)` (mask.rs:12-47). The cutout pass clears a
 * rounded-rect hole at the composer's live bounds — bounds = the "cleared"
 * rect (composer origin/width, bottom extended to the hero's bottom so a
 * taller image must not fade back in beneath the composer's rounded lower
 * edge), `radius = COMPOSER_RADIUS`, `feather = clamp(hero height × 0.52,
 * 120, 280)`, `clearance = 8`. The reveal pass parks its exclusion rect
 * entirely below the image so it has only the shared bottom fade.
 */
export function heroMaskGeometry(hero: Rect, composer: Rect, cutout: boolean): HeroMaskGeometry {
  const height = hero.height;
  // Keep the cleared area open through the hero's bottom.
  const cleared: Rect = {
    x: composer.x,
    y: composer.y,
    width: composer.width,
    height: Math.max(bottom(composer), bottom(hero)) - composer.y,
  };
  const parked: Rect = {
    x: hero.x,
    y: bottom(hero) + 1,
    width: hero.width,
    height: hero.height,
  };
  return {
    bounds: cutout ? cleared : parked,
    radius: cutout ? HERO_MASK_RADIUS : 0,
    feather: cutout ? clamp(height * 0.52, 120, 280) : REVEAL_FEATHER,
    clearance: cutout ? HERO_MASK_CLEARANCE : 0,
    // Start fading at the image's top, rather than holding full opacity
    // through its first 40% and compressing the transition near the bottom.
    // Both passes use the full height, independently of the softer cutout.
    bottomFade: { end: bottom(hero), height: Math.max(height, 1) },
  };
}

// ---------------------------------------------------------------------------
// The cutout ramp (the gpui ImageAlphaMask shader over mask.rs's geometry)
// ---------------------------------------------------------------------------

/**
 * `smoothstep(0, edge, value)` — the GLSL smoothstep the shader applies to
 * both the hole ramp and the bottom fade: `t²(3 − 2t)` over the clamped
 * `t = value/edge`. Zero for `value ≤ 0`, 0.5 at `edge/2`, exactly 1 at
 * `value ≥ edge` (compact support).
 */
function smoothstep(value: number, edge: number): number {
  const t = clamp(value / edge, 0, 1);
  return t * t * (3 - 2 * t);
}

/**
 * The hole ramp — `image_mask_alpha`'s first term (shaders.wgsl): the
 * rounded-rect SDF of the mask's own bounds,
 * `q = |p − center| − half_size + radius`,
 * `distance = length(max(q, 0)) + min(max(q.x, q.y), 0) − radius`
 * (with the radius clamped to half the shorter side, exactly like
 * `ImageAlphaMask::scale`), then
 * `smoothstep(0, feather, distance − clearance)` — alpha is exactly 0 for
 * `d ≤ clearance` (the hard transparent margin), 0.5 at
 * `d = clearance + feather/2`, and 1 at `d ≥ clearance + feather`.
 */
export function cutoutHoleAlpha(mask: HeroMaskGeometry, x: number, y: number): number {
  if (mask.feather <= 0) {
    return 1;
  }
  const bounds = mask.bounds;
  const radius = Math.min(
    Math.max(mask.radius, 0),
    Math.max(bounds.width, 0) * 0.5,
    Math.max(bounds.height, 0) * 0.5,
  );
  const halfWidth = bounds.width * 0.5;
  const halfHeight = bounds.height * 0.5;
  const qx = Math.abs(x - (bounds.x + halfWidth)) - halfWidth + radius;
  const qy = Math.abs(y - (bounds.y + halfHeight)) - halfHeight + radius;
  const distance = Math.hypot(Math.max(qx, 0), Math.max(qy, 0)) + Math.min(Math.max(qx, qy), 0) - radius;
  return smoothstep(distance - mask.clearance, mask.feather);
}

/**
 * The shared bottom fade — the shader's second term:
 * `smoothstep(0, bottom_feather, bottom_y − y)` across the hero's full
 * height, on BOTH passes (mask.rs:42-46).
 */
export function cutoutBottomFadeAlpha(mask: HeroMaskGeometry, y: number): number {
  const feather = mask.bottomFade.height;
  if (feather <= 0) {
    return 1;
  }
  return smoothstep(mask.bottomFade.end - y, feather);
}

/**
 * The desktop's whole mask shader in one pure function (shaders.wgsl
 * `image_mask_alpha`): `alpha = min(hole, fade)` — the `min` happens inside
 * ONE mask, so a pixel where both terms are mid-ramp keeps the brighter of
 * the two (multiplying two element masks would darken it to their product).
 * A non-positive feather disables the entire mask, exactly like the shader's
 * early-out.
 */
export function cutoutMaskAlpha(mask: HeroMaskGeometry, x: number, y: number): number {
  if (mask.feather <= 0) {
    return 1;
  }
  return Math.min(cutoutHoleAlpha(mask, x, y), cutoutBottomFadeAlpha(mask, y));
}

/**
 * The shader's per-pixel grid over the hero's own raster: every raster pixel
 * `(x, y)` evaluates `cutoutMaskAlpha` at its window-space center
 * `(hero.x + (x + 0.5)/scale, hero.y + (y + 0.5)/scale)` — the web peer of
 * the fragment shader evaluating the scaled mask at device-pixel positions
 * (the ramp is scale-invariant, so `scale` is the canvas backing store's
 * device-pixel ratio). Rows and columns outside the hole's ramp band are
 * filled with the row-constant fade (the hole is exactly 1 there), which
 * keeps the per-pixel smoothstep work inside the dome.
 */
export function cutoutMaskRaster(
  hero: Rect,
  composer: Rect,
  cutout: boolean,
  width: number,
  height: number,
  scale = 1,
): Float32Array {
  const mask = heroMaskGeometry(hero, composer, cutout);
  const out = new Float32Array(width * height);
  // The hole can only dip below 1 within `feather + clearance` of the mask's
  // bounds (the SDF's sub-level set of a rounded rect is its dilation by
  // that distance); a pixel outside the band evaluates to exactly the row's
  // fade, so only the band runs the per-pixel smoothstep.
  const reach = mask.feather + mask.clearance + 1;
  const bounds = mask.bounds;
  const bandLeft = Math.max(0, Math.floor((bounds.x - reach - hero.x) * scale));
  const bandRight = Math.min(width, Math.ceil((bounds.x + bounds.width + reach - hero.x) * scale));
  const bandTop = Math.max(0, Math.floor((bounds.y - reach - hero.y) * scale));
  const bandBottom = Math.min(height, Math.ceil((bounds.y + bounds.height + reach - hero.y) * scale));
  for (let y = 0; y < height; y++) {
    const windowY = hero.y + (y + 0.5) / scale;
    const fade = cutoutBottomFadeAlpha(mask, windowY);
    out.fill(fade, y * width, (y + 1) * width);
    if (y < bandTop || y >= bandBottom) {
      continue;
    }
    for (let x = bandLeft; x < bandRight; x++) {
      out[y * width + x] = Math.min(
        cutoutHoleAlpha(mask, hero.x + (x + 0.5) / scale, windowY),
        fade,
      );
    }
  }
  return out;
}

/**
 * The shared bottom-fade gradient stops, with smoothstep values at the
 * quarter points (t=.25 → .156, .5 → .5, .75 → .844) — the ticket's CSS
 * mapping of `alpha *= smoothstep(0, hero_height, hero.bottom − y)` across
 * the hero's entire height, on BOTH passes.
 */
export const BOTTOM_FADE_GRADIENT =
  "linear-gradient(to bottom, rgba(0,0,0,1) 0%, rgba(0,0,0,0.156) 25%, rgba(0,0,0,0.5) 50%, rgba(0,0,0,0.844) 75%, rgba(0,0,0,0) 100%)";

/** Rect equality for the hero's re-measure guards. */
export { rectEquals };

// ---------------------------------------------------------------------------
// Artwork readiness (new_thread_background_effects.rs::Readiness, :11-32)
// ---------------------------------------------------------------------------

/**
 * The artwork's 120 ms fade-in, restarting only when the image id changes
 * (the same artwork does not re-fade); reduced motion snaps to 1. Exactly
 * 0.5 at 60 ms (`smoothstep(0.5)`), asserted by the desktop's test at
 * effects.rs:328-330.
 */
export class Readiness {
  #id: string | number | null = null;
  #startMs = 0;

  opacity(imageId: string | number | null, reduced: boolean, nowMs: number): number {
    if (imageId === null) {
      this.#id = null;
      return 0;
    }
    if (this.#id !== imageId) {
      this.#id = imageId;
      this.#startMs = nowMs;
    }
    if (reduced) {
      return 1;
    }
    const t = clamp((nowMs - this.#startMs) / 120, 0, 1);
    return t * t * (3 - 2 * t);
  }
}

// ---------------------------------------------------------------------------
// Artwork resolution (settings.rs:62-93 + the decode contract)
// ---------------------------------------------------------------------------

/** The bundled fallback's public URL — served from the app bundle. */
export const DEFAULT_NEW_THREAD_BACKGROUND_URL = "/backgrounds/default-new-thread-background.png";

/**
 * A background is only "available" when one is installed AND its file still
 * exists (shell.rs:5848-5859 resolves setting-path or default). The web's
 * managed copy lives in IndexedDB (`lib/background-blob-store.ts`); the
 * setting's `path` names it as `idb:new-thread-composer-background`, and an
 * entry only counts when it actually decodes — `createImageBitmap` accepts
 * only what really decodes, which is the decode-by-sniffing contract (SVG is
 * allowed as an ATTACHMENT but must be rejected as a background;
 * `createImageBitmap` rejects SVG blobs).
 */
export async function decodeBackgroundBlob(blob: Blob): Promise<boolean> {
  try {
    await createImageBitmap(blob);
    return true;
  } catch {
    return false;
  }
}

/**
 * Resolve the artwork to paint: the setting's managed blob if it decodes,
 * else the bundled default. A `path` that is not the managed key is fetched
 * as a URL (a same-session object URL from a pre-managed install) so legacy
 * stored paths keep resolving until they are replaced. Thin wrapper over
 * [`resolveActiveNewThreadBackground`] — the painter and the Appearance row
 * share that one resolution; on a broken stored entry this wrapper keeps
 * painting the default (the page-vs-painter split is existing behavior).
 *
 * The default blob store is the module-level singleton
 * (`idbBackgroundBlobStore`), so every resolution — this one, the
 * Appearance row's, the install/remove staging — shares ONE `cachedUrl`:
 * the resolved URL (hence the artwork's identity) is stable across mounts,
 * and replacing the blob retires the old URL exactly once (ticket 35).
 */
export async function resolveNewThreadBackground(
  setting: { readonly path: string; readonly name: string } | null,
  defaultUrl: string = DEFAULT_NEW_THREAD_BACKGROUND_URL,
  blobs: BackgroundBlobStore = idbBackgroundBlobStore(),
): Promise<string> {
  return (await resolveActiveNewThreadBackground(setting, defaultUrl, blobs))?.url ?? defaultUrl;
}

/** The background that actually renders (ticket 48's resolved selection). */
export interface ResolvedNewThreadBackground {
  /** The artwork that will paint. */
  readonly url: string;
  /** Display name: the stored name, or "Zeron" for the default. */
  readonly name: string;
  /** True when nothing is stored and the bundled default resolved. */
  readonly isDefault: boolean;
}

/**
 * The web peer of the desktop's `settings::active_new_thread_background`:
 * the stored entry while its blob still resolves, else the bundled default.
 * A stored entry that no longer resolves returns null — the Appearance
 * row's "Image unavailable" state — while the painter separately falls back
 * to the default through `resolveNewThreadBackground`. Callers gating UI on
 * "is a background active" must use this, never the raw persisted field.
 */
export async function resolveActiveNewThreadBackground(
  setting: { readonly path: string; readonly name: string } | null,
  defaultUrl: string = DEFAULT_NEW_THREAD_BACKGROUND_URL,
  blobs: BackgroundBlobStore = idbBackgroundBlobStore(),
): Promise<ResolvedNewThreadBackground | null> {
  if (setting === null) {
    return { url: defaultUrl, name: "Zeron", isDefault: true };
  }
  const installed = await resolveInstalledBackground(setting, blobs);
  if (installed === null) {
    return null;
  }
  return { url: installed, name: setting.name, isDefault: false };
}

/**
 * The installed background's URL, or null when nothing is installed or the
 * stored entry no longer decodes — the Appearance row's "Image unavailable"
 * state and the effect row's gate. Binds the singleton blob store by
 * default (see `resolveNewThreadBackground`).
 */
export async function resolveInstalledBackground(
  setting: { readonly path: string; readonly name: string } | null,
  blobs: BackgroundBlobStore = idbBackgroundBlobStore(),
): Promise<string | null> {
  if (setting === null) {
    return null;
  }
  if (setting.path === NEW_THREAD_BACKGROUND_IDB_PATH) {
    try {
      return await blobs.url();
    } catch {
      return null;
    }
  }
  try {
    const response = await fetch(setting.path);
    if (response.ok && (await decodeBackgroundBlob(await response.blob()))) {
      return setting.path;
    }
  } catch {
    // Not installed (or unreachable) — the caller falls back.
  }
  return null;
}

/**
 * The Appearance page's background row state (ticket 48's truth table, the
 * web peer of the desktop's `background_row_state`): the resolved value
 * drives the row — thumbnail, meta name, and the effect row's gate — while
 * the stored one preserves the "Image unavailable" distinction.
 */
export interface BackgroundRowState {
  /** "Replace image" + "Remove" actions (the bundled default counts). */
  readonly installed: boolean;
  /** The artwork tile and effect row show exactly when artwork resolves. */
  readonly available: boolean;
  /** The meta fragments under the row title. */
  readonly meta: readonly string[];
}

export function backgroundRowState(
  stored: { readonly path: string; readonly name: string } | null,
  resolved: ResolvedNewThreadBackground | null,
): BackgroundRowState {
  if (resolved !== null) {
    return {
      installed: true,
      available: true,
      meta: [resolved.name, "Softened automatically on frosted themes."],
    };
  }
  if (stored !== null) {
    return {
      installed: true,
      available: false,
      meta: ["Image unavailable", "Choose a replacement or remove it."],
    };
  }
  return {
    installed: false,
    available: false,
    meta: ["Add an image behind the composer on empty new threads."],
  };
}

// ---------------------------------------------------------------------------
// Install / remove (settings.rs:305-386)
// ---------------------------------------------------------------------------

/** settings.rs:309-313 — the decode rejection's copy, verbatim. */
export const BACKGROUND_UNSUPPORTED_MESSAGE =
  "This background image is unsupported or damaged. Choose a valid image such as PNG or JPEG.";
/** settings.rs:332/344 — the persistence failure's copy, verbatim. */
export const BACKGROUND_SAVE_MESSAGE = "Unable to save the image. Check folder permissions and try again.";
/** settings.rs:371 — the removal failure's copy, verbatim. */
export const BACKGROUND_REMOVE_MESSAGE = "Unable to remove the image. Check folder permissions and try again.";

/** The settings field's `path` while the managed IndexedDB copy backs it. */
export const NEW_THREAD_BACKGROUND_IDB_PATH = "idb:new-thread-composer-background";

/** The stores an install/remove touches; injectable for tests. */
export interface BackgroundDeps {
  readonly settings: UiSettingsStore;
  readonly blobs: BackgroundBlobStore;
}

/**
 * `install_new_thread_composer_background` (settings.rs:305-362): reject a
 * file that does not decode (never persist the candidate), stage the managed
 * copy, then flip the setting — and on any staging failure leave the
 * previous background exactly as it was. The web's staging IS the blob put,
 * so the desktop's save-then-retire ordering collapses to: decode → put →
 * point (the settings write itself is atomic in `UiSettingsStore`).
 */
export async function installNewThreadBackground(file: File, deps: BackgroundDeps): Promise<string | null> {
  if (!(await decodeBackgroundBlob(file))) {
    return BACKGROUND_UNSUPPORTED_MESSAGE;
  }
  try {
    await deps.blobs.put(file);
  } catch {
    return BACKGROUND_SAVE_MESSAGE;
  }
  deps.settings.updateImmediate({
    newThreadComposerBackground: { path: NEW_THREAD_BACKGROUND_IDB_PATH, name: file.name },
  });
  return null;
}

/**
 * `remove_new_thread_composer_background` (settings.rs:364-386): clear the
 * field first, retire the managed blob after — a failed retirement leaves a
 * stray invisible blob, never a setting pointing at a deleted image.
 */
export async function removeNewThreadBackground(deps: BackgroundDeps): Promise<string | null> {
  if (deps.settings.getSnapshot().newThreadComposerBackground === null) {
    return null;
  }
  deps.settings.updateImmediate({ newThreadComposerBackground: null });
  try {
    await deps.blobs.delete();
  } catch {
    return BACKGROUND_REMOVE_MESSAGE;
  }
  return null;
}
