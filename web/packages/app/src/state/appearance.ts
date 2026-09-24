import { useSyncExternalStore } from "react";
import type { Appearance } from "@zeron/theme";
import type { NewThreadBackgroundEffect, NewThreadComposerBackground } from "./ui-settings";
import { AppearanceStore, type AppearancePreferences, resolveAppearance } from "../lib/appearance-store";
import { applyAppearanceToDocument, applyConversationWidthToDocument, applyTypographyToDocument } from "../theme";
import { Readiness, resolveNewThreadBackground } from "../lib/new-thread-background";
import { prepareNewThreadBackgroundEffects } from "../lib/new-thread-background-effects";
import { uiSettings, type UiSettingsStore } from "./ui-settings";

/**
 * The browser-scoped appearance store singleton plus its live application:
 * `initAppearance` installs the stored preferences on the document root
 * before first paint (no dark-to-light flash), then re-applies on every
 * preference change and every OS light/dark move while the mode is
 * `system` — the web peer of the desktop's `appearance::observe_window`.
 */
export const appearanceStore = new AppearanceStore();

const subscribeStore = (listener: () => void) => appearanceStore.subscribe(listener);
const getSnapshot = () => appearanceStore.getSnapshot();

export function useAppearance(): AppearancePreferences {
  return useSyncExternalStore(subscribeStore, getSnapshot, getSnapshot);
}

const DARK_QUERY = "(prefers-color-scheme: dark)";

function darkMedia(): { matches: boolean; addEventListener?: (type: string, listener: () => void) => void; removeEventListener?: (type: string, listener: () => void) => void } | null {
  const matchMedia = (
    globalThis as {
      matchMedia?: (query: string) => {
        matches: boolean;
        addEventListener?: (type: string, listener: () => void) => void;
        removeEventListener?: (type: string, listener: () => void) => void;
      };
    }
  ).matchMedia;
  return matchMedia === undefined ? null : matchMedia(DARK_QUERY);
}

/** The OS light/dark state, reactive while any reader is mounted. */
export function useSystemAppearance(): Appearance {
  const subscribe = (listener: () => void) => {
    const media = darkMedia();
    media?.addEventListener?.("change", listener);
    return () => media?.removeEventListener?.("change", listener);
  };
  return useSyncExternalStore(subscribe, systemAppearance, systemAppearance);
}

/**
 * The appearance that actually paints: the stored mode resolved against the
 * OS state. Surfaces that need to pick appearance-dependent assets (the
 * file-type icons' `dark/` tree) read this, not the raw preference.
 */
export function useResolvedAppearance(): Appearance {
  const preferences = useAppearance();
  const system = useSystemAppearance();
  return resolveAppearance(preferences.mode, system);
}

function systemAppearance(): Appearance {
  return darkMedia()?.matches === false ? "light" : "dark";
}

// ---------------------------------------------------------------------------
// The new-thread background (ticket 15, settings.rs:86-122; ticket 35's
// shell-scoped artwork + readiness, shell.rs:5846-5895)
// ---------------------------------------------------------------------------

/** The resolved new-thread hero artwork plus the settings-store effect. */
export interface NewThreadArtwork {
  /** The decoded image to paint, or null while resolving / nothing installed. */
  readonly url: string | null;
  /** The artwork's identity — the readiness fade restarts when it changes. */
  readonly id: string | number | null;
  readonly effect: NewThreadBackgroundEffect;
  /**
   * The store-owned readiness clock's past-arrival flag — the hero wrapper's
   * `data-ready`: false while a cold artwork's 120 ms fade has yet to start,
   * true from the first frame on. A warm artwork (the same id across a route
   * change) mounts ready and never re-fades.
   */
  readonly ready: boolean;
}

/** Construction seams for the store's tests (settings, resolve, clock, frames). */
export interface NewThreadArtworkStoreOptions {
  readonly settings?: UiSettingsStore;
  /** The artwork resolution; defaults to `resolveNewThreadBackground`. */
  readonly resolve?: (setting: NewThreadComposerBackground | null) => Promise<string>;
  /** The prewarm (decode + effect raster); tests record the calls. */
  readonly prewarm?: (effect: NewThreadBackgroundEffect, appearance: Appearance, url: string) => void;
  /** The monotonic clock the readiness fade reads. */
  readonly nowMs?: () => number;
  /** The frame scheduler — rAF while fading in the browser. */
  readonly schedule?: (callback: () => void) => void;
  /** `prefers-reduced-motion` at evaluation time. */
  readonly reducedMotion?: () => boolean;
}

/** `prefers-reduced-motion` right now (the store reads it per evaluation). */
function prefersReducedMotionNow(): boolean {
  return (
    typeof window !== "undefined" &&
    typeof window.matchMedia === "function" &&
    window.matchMedia("(prefers-reduced-motion: reduce)").matches
  );
}

/** rAF while fading (shell.rs:5884-5886); environments without rAF never advance. */
function defaultFrameSchedule(callback: () => void): void {
  if (typeof requestAnimationFrame === "function") {
    requestAnimationFrame(callback);
  }
}

/**
 * The default prewarm — decode + raster only (effects.rs:292-302's
 * `prepare`, hero-geometry-free, safe on BOTH routes): the effect raster's
 * memoized job, plus the raw artwork's decode for the `none` effect, whose
 * hero paints the image itself. A warm decode is what keeps a remounting
 * hero from painting empty canvases.
 */
function defaultPrewarm(effect: NewThreadBackgroundEffect, appearance: Appearance, url: string): void {
  void prepareNewThreadBackgroundEffects(effect, appearance, url);
  if (typeof Image === "function") {
    const element = new Image();
    element.src = url;
    void element.decode().catch(() => {});
  }
}

/**
 * The shell-scoped new-thread artwork source — the web peer of the desktop's
 * shell-owned artwork and `Shell::new_thread_artwork_ready` (shell.rs:1032,
 * 5846-5895): the resolution, the effect, and the 120 ms readiness clock
 * live HERE, at module scope, so they survive every route change and every
 * hero unmount. The desktop resolves (and prewarms) on BOTH routes — "not
 * contingent on a hero measurement or a navigation gesture" — and the
 * Readiness restarts only when the image id changes, so a remounting hero
 * finds warm artwork and never re-fades (ticket 35, gaps G17/G18).
 */
export class NewThreadArtworkStore {
  readonly #settings: UiSettingsStore;
  readonly #resolveArtwork: (setting: NewThreadComposerBackground | null) => Promise<string>;
  readonly #prewarmArtwork: (effect: NewThreadBackgroundEffect, appearance: Appearance, url: string) => void;
  readonly #nowMs: () => number;
  readonly #schedule: (callback: () => void) => void;
  readonly #reducedMotion: () => boolean;
  readonly #listeners = new Set<() => void>();
  /** shell.rs:1032 — the shell-owned readiness clock (effects.rs:11-33). */
  readonly #readiness = new Readiness();
  #url: string | null = null;
  #effect: NewThreadBackgroundEffect;
  #settingKey: string | null | undefined;
  #resolveToken = 0;
  #framePending = false;
  #snapshot: NewThreadArtwork;

  constructor(options: NewThreadArtworkStoreOptions = {}) {
    this.#settings = options.settings ?? uiSettings;
    this.#resolveArtwork = options.resolve ?? ((setting) => resolveNewThreadBackground(setting));
    this.#prewarmArtwork = options.prewarm ?? defaultPrewarm;
    this.#nowMs = options.nowMs ?? (() => performance.now());
    this.#schedule = options.schedule ?? defaultFrameSchedule;
    this.#reducedMotion = options.reducedMotion ?? prefersReducedMotionNow;
    const initial = this.#settings.getSnapshot();
    this.#effect = initial.newThreadBackgroundEffect;
    this.#snapshot = { url: null, id: null, effect: this.#effect, ready: false };
    this.#settings.subscribe(this.#onSettings);
    // Resolve + prewarm from boot, on BOTH routes (shell.rs:5846-5859).
    this.#onSettings();
  }

  getSnapshot = (): NewThreadArtwork => this.#snapshot;

  subscribe = (listener: () => void): (() => void) => {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  };

  /**
   * The readiness clock's current value — the desktop's `artwork_opacity`
   * input (shell.rs:5860-5864). Tests and the one-fade assertion read it;
   * the hero consumes the `ready` flip, whose CSS transition is the visible
   * ramp.
   */
  readinessValue(): number {
    return this.#readiness.opacity(this.#url, this.#reducedMotion(), this.#nowMs());
  }

  /**
   * A settings write: the effect publishes immediately (the readiness is
   * id-keyed, never effect-keyed — an effect flip does not re-fade) and
   * re-prewarms; a changed setting re-resolves. A replace that writes the
   * same path/name keeps the old URL — the desktop's path-keyed artwork
   * cache behaves the same.
   */
  #onSettings = (): void => {
    const snapshot = this.#settings.getSnapshot();
    const setting = snapshot.newThreadComposerBackground;
    const effect = snapshot.newThreadBackgroundEffect;
    if (effect !== this.#effect) {
      this.#effect = effect;
      this.#evaluate();
      this.#prewarm();
    }
    const settingKey = setting === null ? null : `${setting.path}\u0000${setting.name}`;
    if (settingKey !== this.#settingKey) {
      this.#settingKey = settingKey;
      this.#resolve(setting);
    }
  };

  #resolve(setting: NewThreadComposerBackground | null): void {
    const token = ++this.#resolveToken;
    void this.#resolveArtwork(setting).then(
      (url) => {
        if (token !== this.#resolveToken) {
          return; // A newer setting superseded this resolve.
        }
        if (url !== this.#url) {
          this.#url = url;
          // The artwork's arrival: the readiness clock restarts on the new id
          // (shell.rs:5860-5864) and the prewarm runs (5846-5859).
          this.#evaluate();
          this.#prewarm();
        }
      },
      () => {
        // A failed resolve keeps the previous artwork painted.
      },
    );
  }

  #prewarm(): void {
    if (this.#url === null) {
      return;
    }
    this.#prewarmArtwork(
      this.#effect,
      resolveAppearance(appearanceStore.getSnapshot().mode, systemAppearance()),
      this.#url,
    );
  }

  /**
   * Evaluate the clock and publish when an observable field changed. The
   * frame loop keeps evaluating while the artwork is resolved and its
   * opacity is under 1 (shell.rs:5884-5886): the ramp rides the store's own
   * frame source, never a hero remount.
   */
  #evaluate(): void {
    const value = this.#readiness.opacity(this.#url, this.#reducedMotion(), this.#nowMs());
    const ready = this.#url !== null && value > 0;
    const next: NewThreadArtwork = { url: this.#url, id: this.#url, effect: this.#effect, ready };
    if (
      next.url !== this.#snapshot.url ||
      next.id !== this.#snapshot.id ||
      next.effect !== this.#snapshot.effect ||
      next.ready !== this.#snapshot.ready
    ) {
      this.#snapshot = next;
      for (const listener of this.#listeners) {
        listener();
      }
    }
    if (this.#url !== null && value < 1) {
      this.#scheduleFrame();
    }
  }

  #scheduleFrame(): void {
    if (this.#framePending) {
      return;
    }
    this.#framePending = true;
    this.#schedule(() => {
      this.#framePending = false;
      this.#evaluate();
    });
  }
}

/** The one artwork source for this page load — shell scope (shell.rs:1032). */
export const newThreadArtworkStore = new NewThreadArtworkStore();

const subscribeArtwork = (listener: () => void) => newThreadArtworkStore.subscribe(listener);
const getArtworkSnapshot = () => newThreadArtworkStore.getSnapshot();

/**
 * Read the shell-scoped new-thread artwork (ticket 35): a thin subscription
 * to `newThreadArtworkStore`. The url/id/effect — and the readiness clock
 * beside them — survive route changes and hero unmounts, so a remounting
 * `NewThreadCanvas` picks up the warm snapshot instead of re-resolving from
 * `url = null` (the route-change double flash's second act). Resolution
 * runs through the shared resolver (ticket 48's
 * `resolveActiveNewThreadBackground`, reached via the
 * `resolveNewThreadBackground` wrapper).
 */
export function useNewThreadBackground(): NewThreadArtwork {
  return useSyncExternalStore(subscribeArtwork, getArtworkSnapshot, getArtworkSnapshot);
}

/** Non-hook read for code outside React (the id keys the readiness clock). */
export function currentNewThreadBackgroundSetting(): NewThreadComposerBackground | null {
  return uiSettings.getSnapshot().newThreadComposerBackground;
}

/** Boot wiring for main.tsx; returns a teardown (unused by the app shell). */
export function initAppearance(): () => void {
  const apply = () => applyAppearanceToDocument(appearanceStore.getSnapshot(), systemAppearance());
  apply();
  // The interface font/size (ticket 28): applied before first paint with the
  // theme, then re-applied on every settings write — a discrete choice, so
  // any snapshot change carries it.
  const applyTypography = () => {
    const settings = uiSettings.getSnapshot();
    applyTypographyToDocument({
      uiFontFamily: settings.uiFontFamily,
      uiFontSize: settings.uiFontSize,
      codeFontFamily: settings.codeFontFamily,
      codeFontSize: settings.codeFontSize,
    });
    // The transcript column cap rides the same write path: a live drag
    // reflow must land with the store's snapshot, not on the next paint.
    applyConversationWidthToDocument(settings.transcriptWidth);
  };
  applyTypography();
  const unsubscribe = appearanceStore.subscribe(apply);
  const unsubscribeTypography = uiSettings.subscribe(applyTypography);
  const media = darkMedia();
  media?.addEventListener?.("change", apply);
  return () => {
    unsubscribe();
    unsubscribeTypography();
    media?.removeEventListener?.("change", apply);
  };
}
