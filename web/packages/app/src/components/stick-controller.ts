import type { ChatArrivalWindow } from "../lib/chat-arrival";
import {
  AT_BOTTOM_PX,
  GLIDE_MAX_VIEWPORTS,
  SPRING_FRAME_MS,
  SPRING_MAX_CATCHUP_FRAMES,
  SPRING_SETTLE_GRACE_MS,
  StickSpring,
  jumpButtonShown,
  jumpVisibility,
  shouldAnchorLiveStream,
  shouldBreakPin,
  shouldRestick,
} from "../lib/stick-spring";
import { GlideTimeline, GLIDE_FRAME_MS, SCROLL_GLIDE_CURVE, SCROLL_GLIDE_MS } from "../lib/rail";
import {
  OWN_SEND_GLIDE_RETAIN,
  OWN_SEND_GLIDE_SNAP_PX,
  OWN_SEND_SCROLL_SLACK_PX,
  OWN_SEND_TOP_INSET_PX,
  type OwnTurnAnchor,
} from "../lib/transcript";
import { cubicBezierEval } from "../state/layout";

/**
 * The scroller's row geometry, read at frame time (the virtualizer keeps the
 * ref fresh per render): where the own-turn anchor row sits in content space,
 * whether the reply's natural content has already filled the reservation, and
 * the row-top prefix sums the rail glide anchors against.
 */
export interface OwnTurnGeometry {
  readonly anchor: { readonly top: number; readonly ix: number } | null;
  readonly filled: boolean;
  /** Row-top positions (the virtualizer's prefix sums), in scroll content space. */
  readonly positions: readonly number[];
  /**
   * True while the row list is mid-replay (the store not loaded, or its
   * replay pending — a resubscribe/reconnect window). A missing anchor in
   * that window is transient: the runway waits, never retires.
   */
  readonly transient: boolean;
}

export interface StickControllerOptions {
  onJumpVisibility: (shown: boolean) => void;
  /** The runway installed/retired/re-armed — the virtualizer recomputes its floor. */
  onOwnTurnChange?: () => void;
  /** User scroll input (not ours) — the surface cancels its hold/anim state. */
  onUserInput?: () => void;
  /**
   * Explicit navigation began (ticket 71): a fold toggle, the rail glide,
   * the selection auto-scroll, or a tool-fold click handed the viewport to
   * navigation. The surface cancels its tool-fold compensation here — the
   * desktop's `begin_scroll_navigation` clears its compensations the same
   * way.
   */
  onNavigation?: () => void;
  reducedMotion?: MediaQueryList | null;
  /**
   * The chat-switch arrival window (ticket 58): while it is armed, `kick`
   * writes the end directly instead of arming the spring — a switch's
   * arrival is atomic (the desktop's `select_chat`, state.rs:1740-1792),
   * so the estimate→measure settle cascade must never read as motion.
   */
  arrival?: ChatArrivalWindow | null;
}

/**
 * The DOM driver for the stick-to-bottom spring and the own-turn runway —
 * the web peer of the desktop transcript's `handle_scroll`/`step_spring`/
 * `step_own_turn`/`engage_pin` (crates/ui/src/transcript.rs):
 *
 * - Escape: a scroll the controller didn't write that moves AWAY from the
 *   bottom breaks the pin (wheel, touch, keys, scrollbar drag all surface as
 *   scroll events; content growth never fires one). A no-op scroll attempt
 *   at the end keeps the pin, exactly like the desktop's distance check.
 * - Re-stick: arriving at the bottom, or moving toward it inside the 70px
 *   band, re-engages the pin with a glide (`shouldRestick` direction guard).
 * - Teleport: jumps longer than 2.5 viewports snap to within that range and
 *   glide the rest; `prefers-reduced-motion` snaps instead of gliding.
 * - Live anchor: a live stream resting at the end hard-anchors instantly as
 *   its measured height grows (`shouldAnchorLiveStream`).
 * - Settle grace: 500ms after landing the spring state parks; a layout kick
 *   inside the grace reuses it.
 * - Own-turn runway: `onOwnSend` installs an anchor whose entry glide eases
 *   the prompt to `OWN_SEND_TOP_INSET_PX` below the viewport top (row 0
 *   excepted — its own top gap already carries the titlebar chrome) and whose
 *   positioned hold re-asserts that spot after every layout. Wheel input
 *   releases the hold; the reservation itself is the virtualizer's floor and
 *   survives until the reply fills it, the prompt disappears, or the chat is
 *   left and revisited.
 */
export class StickController {
  #el: HTMLElement | null = null;
  #spring = new StickSpring();
  #streaming = false;
  #pinned = true;
  #raf = 0;
  #lastTick: number | null = null;
  #settledAt: number | null = null;
  #kick = false;
  /** Our own scrollTop writes, so the scroll handler can tell ours from the user's. */
  #expected: number | null = null;
  /** The scrollHeight at the time of our write — a browser clamp from a
   *  layout shrink (virtualizer estimate drift) is still ours. */
  #expectedHeight = 0;
  #prevDistance = 0;
  #jumpShown = false;
  #geometry: (() => OwnTurnGeometry) | null = null;
  #ownTurn: OwnTurnAnchor | null = null;
  #ownTurnLastTick: number | null = null;
  /** The rail glide's 16ms timer — one glide owns the scroll-task slot. */
  #glideTimer: number | 0 = 0;
  readonly #onJumpVisibility: (shown: boolean) => void;
  readonly #onOwnTurnChange: () => void;
  readonly #onUserInput: () => void;
  readonly #onNavigation: () => void;
  readonly #reduced: MediaQueryList | null;
  readonly #arrival: ChatArrivalWindow | null;

  constructor(options: StickControllerOptions) {
    this.#onJumpVisibility = options.onJumpVisibility;
    this.#onOwnTurnChange = options.onOwnTurnChange ?? (() => {});
    this.#onUserInput = options.onUserInput ?? (() => {});
    this.#onNavigation = options.onNavigation ?? (() => {});
    this.#arrival = options.arrival ?? null;
    this.#reduced =
      options.reducedMotion ??
      (typeof globalThis.matchMedia === "function" ? globalThis.matchMedia("(prefers-reduced-motion: reduce)") : null);
  }

  get pinned(): boolean {
    return this.#pinned;
  }

  /** The live own-turn anchor (read-only view; the reservation's floor keys off it). */
  get ownTurn(): OwnTurnAnchor | null {
    return this.#ownTurn;
  }

  /** The runway currently owns the viewport (gliding or holding). */
  get ownTurnHeld(): boolean {
    return this.#ownTurn?.held === true;
  }

  attach(el: HTMLElement): void {
    this.detach();
    this.#el = el;
    this.#pinned = true;
    this.#ownTurn = null;
    this.#ownTurnLastTick = null;
    this.#prevDistance = this.#distance();
    this.#jumpShown = false;
    el.addEventListener("scroll", this.#onScroll, { passive: true });
  }

  detach(): void {
    const el = this.#el;
    if (el !== null) {
      el.removeEventListener("scroll", this.#onScroll);
    }
    this.#el = null;
    if (this.#raf !== 0) {
      cancelAnimationFrame(this.#raf);
      this.#raf = 0;
    }
    this.#stopGlide();
  }

  setStreaming(streaming: boolean): void {
    this.#streaming = streaming;
  }

  /** The virtualizer's live geometry (anchor row position, reservation fill). */
  setGeometry(provider: () => OwnTurnGeometry): void {
    this.#geometry = provider;
  }

  /** Content or viewport resized: one observation frame (desktop wake_spring). */
  kick(): void {
    if (this.#pinned && this.#arrival?.isArrival(performance.now())) {
      // Ticket 58 — a chat switch's arrival is atomic: while the arrival
      // window is armed, the settle cascade's measurement batches must not
      // arm the spring against their drift (the visible "scrolling down").
      // Write the end directly (`snapToEnd`'s shape) and leave the spring
      // parked; the window is the scroller's ONE arrival predicate.
      this.#spring.reset();
      this.#lastTick = null;
      this.#settledAt = null;
      this.#write(this.#maxScroll());
      this.#prevDistance = this.#distance();
      return;
    }
    if (
      this.#settledAt !== null &&
      this.#lastTick !== null &&
      performance.now() - this.#settledAt >= SPRING_SETTLE_GRACE_MS
    ) {
      this.#spring.reset();
      this.#lastTick = null;
    }
    this.#settledAt = null;
    // Refresh the escape baseline (the desktop's own-turn stepper discipline,
    // transcript.rs:3534-3540): only the NEXT scroll's own delta may register
    // as user intent. Without this, content growth between two user scrolls
    // accumulates into the second scroll's reading — a wheel DOWN toward the
    // bottom after growth read as "scrolled away" and silently released the
    // pin, dropping the stream below the fold. The scroll-anchoring class of
    // phantom (the browser adjusting scrollTop when content above resizes)
    // dies on the same baseline: its distance does not grow.
    this.#prevDistance = this.#distance();
    this.#kick = true;
    this.#schedule();
  }

  /** Snap to the end without motion (initial load, chat switch). */
  snapToEnd(): void {
    const el = this.#el;
    if (el === null) {
      return;
    }
    this.#pinned = true;
    this.#spring.reset();
    this.#write(this.#maxScroll());
    this.#prevDistance = 0;
    this.#setJumpShown(false);
  }

  /**
   * Apply a saved viewport after a populated replay (`restore_pending_viewport`):
   * the concrete scroll position, the released runway (the reservation without
   * the automatic hold — revisiting must not follow new output to the bottom),
   * and the saved distance's jump-button state.
   */
  restoreViewport(scrollTop: number, ownTurn: OwnTurnAnchor | null, distanceFromBottom: number): void {
    const el = this.#el;
    if (el === null) {
      return;
    }
    this.#pinned = false;
    this.#spring.reset();
    this.#lastTick = null;
    this.#settledAt = null;
    this.#ownTurn = ownTurn;
    this.#ownTurnLastTick = null;
    this.#onOwnTurnChange();
    this.#write(Math.max(0, Math.min(scrollTop, this.#maxScroll())));
    this.#prevDistance = this.#distance();
    this.#setJumpShown(jumpVisibility(false, distanceFromBottom));
  }

  /**
   * `on_own_send` (transcript.rs:3323): reserve the reply's space below a
   * locally-sent prompt — EVERY send, not just the first. Replacing a
   * previous anchor starts a new glide.
   */
  onOwnSend(chatId: string, messageId: string): void {
    const geometry = this.#geometry?.() ?? null;
    const seenPrompt = geometry?.anchor !== null && geometry?.anchor !== undefined;
    this.#ownTurn = {
      chatId,
      messageId,
      held: true,
      positioned: false,
      seenPrompt,
    };
    this.#ownTurnLastTick = null;
    this.#pinned = false;
    this.#spring.reset();
    this.#lastTick = null;
    this.#settledAt = null;
    this.#setJumpShown(false);
    this.#onOwnTurnChange();
    this.#schedule();
  }

  /**
   * `begin_scroll_navigation` (transcript.rs:3062): hand viewport ownership to
   * explicit navigation (a fold toggle, the selection auto-scroll) before it
   * moves the list — the own-turn hold stands down, the pin drops, and any
   * running rail glide yields (the desktop's one scroll-task slot).
   */
  beginScrollNavigation(): void {
    this.#stopGlide();
    this.releaseOwnTurnHold();
    this.#pinned = false;
    this.#spring.reset();
    this.#lastTick = null;
    this.#settledAt = null;
    this.#kick = false;
    // The surface's viewport compensations stand down (ticket 71): a tool
    // fold compensator arming AFTER this call is the next owner.
    this.#onNavigation();
  }

  /** Cancel a running glide (the scroll-task slot clears; the next
   *  navigation owns the viewport). */
  #stopGlide(): void {
    if (this.#glideTimer !== 0) {
      window.clearInterval(this.#glideTimer);
      this.#glideTimer = 0;
    }
  }

  /**
   * `scroll_to_row` (rail.rs:251-408), the DOM port: glide the list so `row`
   * sits at the viewport top over `SCROLL_GLIDE` (500ms `EASE_IN_OUT`) at a
   * 16ms cadence, each frame consuming a [`GlideTimeline`] fraction of the
   * CURRENT remaining distance — a height re-estimate mid-flight just
   * re-enters the same timeline. Reduced motion jumps. The target row's top
   * is re-read from the live geometry every frame (the virtualizer's prefix
   * sums correct as rows measure).
   */
  scrollToRow(row: number): void {
    const el = this.#el;
    if (el === null) {
      return;
    }
    this.beginScrollNavigation();
    const target = (): number | null => {
      const positions = this.#geometry?.().positions;
      const top = positions?.[row];
      return top === undefined ? null : top;
    };
    const first = target();
    if (first === null) {
      return;
    }
    if (this.#reduced?.matches) {
      this.#write(first);
      return;
    }
    this.#stopGlide();
    const started = performance.now();
    const timeline = new GlideTimeline();
    // The desktop runs (total/16) + 90 frames, then lands exactly (rail.rs:267).
    const frames = Math.ceil(SCROLL_GLIDE_MS / GLIDE_FRAME_MS) + 90;
    let frame = 0;
    this.#glideTimer = window.setInterval(() => {
      frame++;
      const raw = Math.min(Math.max((performance.now() - started) / SCROLL_GLIDE_MS, 0), 1);
      const eased = cubicBezierEval(SCROLL_GLIDE_CURVE, raw);
      const frac = timeline.step(eased);
      const here = el.scrollTop;
      const goal = target() ?? here;
      if (raw >= 1 || frame >= frames) {
        this.#stopGlide();
        this.#write(goal);
        return;
      }
      this.#write(here + frac * (goal - here));
    }, GLIDE_FRAME_MS);
  }

  /** The hold stands down; the reservation (the virtualizer's floor) stays. */
  releaseOwnTurnHold(): void {
    if (this.#ownTurn?.held === true) {
      this.#ownTurn = { ...this.#ownTurn, held: false };
      this.#ownTurnLastTick = null;
    } else {
      this.#ownTurnLastTick = null;
    }
  }

  /**
   * Re-engage the bottom pin with a glide; long jumps teleport first. With a
   * live runway, "bottom" IS the held position (the reservation makes
   * prompt-at-top and pad-bottom the same place): re-arm the hold and glide
   * back instead of destroying the runway — only navigating away and back
   * clears it (`jump_to_bottom`, transcript.rs:3748).
   */
  jumpToBottom(options: { anchorExpanded?: boolean } = {}): void {
    if (this.#el === null) {
      return;
    }
    // An expanded prompt can be taller than the viewport; jumping should
    // reveal the reply below it, so the hold releases and the pin engages.
    if (this.#ownTurn !== null && options.anchorExpanded === true) {
      this.releaseOwnTurnHold();
      this.engagePin();
      return;
    }
    if (this.#ownTurn !== null) {
      this.#ownTurn = { ...this.#ownTurn, held: true, positioned: false };
      this.#ownTurnLastTick = null;
      this.#pinned = false;
      this.#spring.reset();
      this.#setJumpShown(false);
      this.#schedule();
      return;
    }
    this.engagePin();
  }

  /** `engage_pin` (transcript.rs:3782) — pin + hide the pill + wake the spring. */
  engagePin(): void {
    const el = this.#el;
    if (el === null) {
      return;
    }
    this.#pinned = true;
    this.#setJumpShown(false);
    if (this.#reduced?.matches) {
      this.#write(this.#maxScroll());
      this.#prevDistance = 0;
      return;
    }
    const viewport = el.clientHeight;
    const distance = this.#distance();
    const glideMax = GLIDE_MAX_VIEWPORTS * viewport;
    if (viewport > 0 && distance > glideMax) {
      this.#write(this.#maxScroll() - glideMax);
    }
    this.kick();
  }

  /**
   * A viewport-preserving scroll write from the virtualizer's anchor restore:
   * marked as ours so the scroll handler never reads it as a user escape.
   */
  writePreserving(scrollTop: number): void {
    this.#write(scrollTop);
  }

  /** Current distance from the end in px. */
  #distance(): number {
    const el = this.#el;
    return el === null ? 0 : Math.max(0, el.scrollHeight - el.clientHeight - el.scrollTop);
  }

  #maxScroll(): number {
    const el = this.#el;
    return el === null ? 0 : Math.max(0, el.scrollHeight - el.clientHeight);
  }

  #write(scrollTop: number): void {
    const el = this.#el;
    if (el === null) {
      return;
    }
    this.#expected = scrollTop;
    this.#expectedHeight = el.scrollHeight;
    el.scrollTop = scrollTop;
  }

  /** `own_send_inset` (transcript.rs:3439): row 0's own gap carries the chrome. */
  static ownSendInset(anchorIx: number): number {
    return anchorIx === 0 ? 0 : OWN_SEND_TOP_INSET_PX;
  }

  #onScroll = (): void => {
    const el = this.#el;
    if (el === null) {
      return;
    }
    const distance = this.#distance();
    // "Ours" also covers a write the browser CLAMPED: the virtualizer's
    // estimate drift can shrink the DOM's height under our own write (rows
    // re-measure, the spacer recomputes), and the clamp's landing position
    // must never read as user input. Two discriminators: a clamp to the
    // scroll end, or a content height that shrank since the write — a user
    // scroll never changes the scrollHeight.
    const ours =
      this.#expected !== null &&
      (Math.abs(el.scrollTop - this.#expected) <= 1.5 ||
        (this.#expected > el.scrollTop && el.scrollTop >= this.#maxScroll() - 0.5) ||
        (this.#expected > el.scrollTop && el.scrollHeight < this.#expectedHeight - 0.5));
    this.#expected = null;
    if (ours) {
      this.#prevDistance = distance;
      return;
    }
    this.#onUserInput();
    if (this.#ownTurn !== null) {
      // Input owns the viewport immediately: the hold releases (the
      // reservation stays as scrollable space), the pin drops, and reaching
      // the end preserves normal tail-follow intent. Re-sticking at a short
      // turn's actual hold re-arms the runway; an off-screen prompt belongs
      // to an overflowing reply (transcript.rs:3140-3177).
      const releasedHeld = this.#ownTurn.held;
      this.releaseOwnTurnHold();
      this.#pinned = false;
      this.#spring.reset();
      this.#lastTick = null;
      const previous = this.#prevDistance;
      this.#prevDistance = distance;
      this.#pinned = distance <= AT_BOTTOM_PX || shouldRestick(distance, previous);
      const geometry = this.#geometry?.() ?? null;
      const anchor = geometry?.anchor ?? null;
      const atHold =
        anchor !== null &&
        anchor.top - el.scrollTop >= StickController.ownSendInset(anchor.ix) - OWN_SEND_SCROLL_SLACK_PX - 2;
      if (!releasedHeld && atHold && shouldRestick(distance, previous)) {
        this.#ownTurn = { ...this.#ownTurn, held: true, positioned: false };
        this.#ownTurnLastTick = null;
        this.#pinned = false;
        this.#schedule();
      }
      if (this.#pinned) {
        this.kick();
      }
      this.#setJumpShown(jumpVisibility(this.#jumpShown, distance) && this.#ownTurn?.held !== true);
      return;
    }
    if (this.#pinned) {
      // User input moving away from the bottom breaks the pin
      // (`shouldBreakPin`, transcript.rs:3182-3189). Content growth never
      // lands here on the desktop — it doesn't fire the scroll handler — and
      // the baseline refresh in kick() keeps that true here.
      if (shouldBreakPin(distance, this.#prevDistance)) {
        this.#pinned = false;
        this.#spring.reset();
        this.#lastTick = null;
        this.#settledAt = null;
      }
    } else if (distance <= AT_BOTTOM_PX || shouldRestick(distance, this.#prevDistance)) {
      // Arriving at the bottom, or returning toward it inside the band,
      // re-engages the pin with a glide.
      this.#pinned = true;
      this.kick();
    }
    this.#prevDistance = distance;
    // `handle_scroll`'s pinned gate (transcript.rs:3198): never flash the
    // pill while the bottom spring settles near the end. No own turn is
    // live in this branch (the one above owns the held suppression).
    this.#setJumpShown(jumpButtonShown(this.#jumpShown, distance, this.#pinned, false));
  };

  #setJumpShown(shown: boolean): void {
    if (shown !== this.#jumpShown) {
      this.#jumpShown = shown;
      this.#onJumpVisibility(shown);
    }
  }

  #schedule(): void {
    if (this.#raf === 0 && this.#el !== null) {
      this.#raf = requestAnimationFrame(this.#tick);
    }
  }

  #tick = (): void => {
    this.#raf = 0;
    const el = this.#el;
    if (el === null) {
      return;
    }
    // The own-turn stepper first: while a runway owns the viewport the spring
    // has nothing to chase (the reservation makes the hold the bottom).
    const owned = this.#ownTurn !== null ? this.#stepOwnTurn(el) : false;
    if (!owned && this.#pinned) {
      this.#stepSpring(el);
    }
  };

  #stepSpring(el: HTMLElement): void {
    const now = performance.now();
    if (this.#settledAt !== null && now - this.#settledAt >= SPRING_SETTLE_GRACE_MS) {
      this.#spring.reset();
      this.#lastTick = null;
      this.#settledAt = null;
    }
    const frames =
      this.#lastTick === null
        ? 1
        : Math.min((now - this.#lastTick) / SPRING_FRAME_MS, SPRING_MAX_CATCHUP_FRAMES);
    this.#lastTick = now;
    this.#kick = false;

    const target = this.#maxScroll();
    let distance = target - el.scrollTop;

    // A live stream resting at the end keeps the end anchored instantly.
    if (shouldAnchorLiveStream(true, distance, this.#streaming)) {
      if (distance > 0) {
        this.#write(target);
        distance = 0;
      }
      this.#settledAt ??= now;
      this.#prevDistance = 0;
      this.#setJumpShown(jumpVisibility(this.#jumpShown, 0));
      return;
    }

    if (this.#reduced?.matches) {
      if (distance > 0) {
        this.#write(target);
      }
      this.#prevDistance = 0;
      return;
    }

    // Long jumps teleport to within glide range first (mugen springToBottom).
    const viewport = el.clientHeight;
    const glideMax = GLIDE_MAX_VIEWPORTS * viewport;
    if (viewport > 0 && distance > glideMax) {
      this.#write(target - glideMax);
      distance = glideMax;
    }
    const pos = target - distance;
    const next = this.#spring.step(pos, target, frames);
    if (next > pos) {
      this.#write(next);
    }
    const remaining = Math.max(0, target - next);
    this.#prevDistance = remaining;
    this.#setJumpShown(jumpVisibility(this.#jumpShown, remaining));
    if (remaining <= 0.5) {
      // Land on the end exactly; remeasured rows must not restart the glide.
      if (el.scrollTop !== target) {
        this.#write(target);
      }
      this.#settledAt ??= now;
    } else {
      this.#settledAt = null;
    }
    // Keep driving while moving; a settled spring wakes on the next kick.
    if (next > pos || StickSpring.needsFrame(remaining)) {
      this.#schedule();
    }
  }

  /**
   * `step_own_turn` (transcript.rs:3529-3734): advance the prompt glide or
   * hand a filled reservation to tail-follow. Returns true while the runway
   * owns the viewport (the spring stands down for the frame).
   */
  #stepOwnTurn(el: HTMLElement): boolean {
    const ownTurn = this.#ownTurn;
    if (ownTurn === null) {
      return false;
    }
    // Layout moves the bottom too (pad refinement, streaming growth): refresh
    // the wheel handler's escape baseline every frame so only a WHEEL's own
    // delta registers as user intent.
    this.#prevDistance = this.#distance();
    const geometry = this.#geometry?.() ?? null;
    const anchor = geometry?.anchor ?? null;
    if (anchor === null) {
      // The desktop waits one notification for the optimistic echo
      // (transcript.rs:3541-3543) — and a row list that is momentarily
      // replaying (a resubscribe or reconnect window; rows are never
      // emptied, but the geometry can lag a frame) is the same wait: keep
      // the runway and schedule the next frame. Only a prompt absent from a
      // POPULATED frame is terminal (a failed echo or a removed entry).
      if (ownTurn.seenPrompt && geometry?.transient !== true) {
        this.#retireOwnTurn();
        return false;
      }
      this.#schedule();
      return true;
    }
    if (ownTurn.messageId !== this.#ownTurn?.messageId || !ownTurn.seenPrompt) {
      this.#ownTurn = { ...ownTurn, seenPrompt: true };
    }
    const viewportHeight = el.clientHeight;
    if (viewportHeight <= 0) {
      this.#schedule();
      return true;
    }
    if (geometry?.filled === true) {
      // The reply's natural content fills the reservation: retire the runway
      // and hand the viewport to the ordinary bottom spring.
      const held = this.#ownTurn?.held === true;
      this.#retireOwnTurn();
      if (held || this.#pinned || this.#distance() <= AT_BOTTOM_PX) {
        this.engagePin();
      }
      return false;
    }
    if (!this.#ownTurn!.held) {
      // Released: the reservation stays as plain scrollable space; the
      // ordinary escape/restick rules apply.
      return false;
    }
    const inset = StickController.ownSendInset(anchor.ix);
    const target = anchor.top - inset;
    if (this.#ownTurn!.positioned) {
      // Landed: re-assert the prompt's position after every layout.
      // ONE-SIDED: only upward drift (the row below the hold) is corrected;
      // the slack under the hold is legal resting space (transcript.rs:3587-3653).
      const err = anchor.top - el.scrollTop - inset;
      const atScrollEnd = el.scrollTop >= this.#maxScroll() - 0.5;
      const moved = !atScrollEnd && (err > 0.5 || err < -(OWN_SEND_SCROLL_SLACK_PX + 2));
      if (moved) {
        // Correct with the entry glide's ease, not a snap — the only in-band
        // escapes are one-frame commit transients; an eased return reads as
        // native rubber-banding.
        const now = performance.now();
        const frames =
          this.#ownTurnLastTick === null
            ? 1
            : Math.min((now - this.#ownTurnLastTick) / SPRING_FRAME_MS, SPRING_MAX_CATCHUP_FRAMES);
        this.#ownTurnLastTick = now;
        const ease = 1 - OWN_SEND_GLIDE_RETAIN ** frames;
        if (Math.abs(err) <= OWN_SEND_GLIDE_SNAP_PX) {
          this.#write(el.scrollTop + err);
          this.#ownTurnLastTick = null;
        } else {
          this.#write(el.scrollTop + err * ease);
        }
        this.#schedule();
      } else {
        this.#ownTurnLastTick = null;
      }
      return true;
    }
    // ---- entry glide ------------------------------------------------------
    const now = performance.now();
    const frames =
      this.#ownTurnLastTick === null
        ? 1
        : Math.min((now - this.#ownTurnLastTick) / SPRING_FRAME_MS, SPRING_MAX_CATCHUP_FRAMES);
    this.#ownTurnLastTick = now;
    const ease = 1 - OWN_SEND_GLIDE_RETAIN ** frames;
    let err = target - el.scrollTop;
    const glideMax = GLIDE_MAX_VIEWPORTS * viewportHeight;
    if (err > glideMax) {
      this.#write(el.scrollTop + err - glideMax);
      err = glideMax;
    }
    const land = (): void => {
      this.#write(target);
      this.#ownTurnLastTick = null;
      if (this.#ownTurn !== null) {
        this.#ownTurn = { ...this.#ownTurn, positioned: true };
      }
    };
    if (this.#reduced?.matches) {
      land();
    } else if (err <= OWN_SEND_GLIDE_SNAP_PX && err >= -(OWN_SEND_SCROLL_SLACK_PX + 2)) {
      // At the hold — or resting in the slack under it: land WITHOUT pulling
      // the view up. Only a still-above position gets the snap.
      if (err > 0.5) {
        land();
      } else if (this.#ownTurn !== null) {
        this.#ownTurn = { ...this.#ownTurn, positioned: true };
      }
      this.#ownTurnLastTick = null;
    } else {
      this.#write(el.scrollTop + err * ease);
      // `own_turn_glide_crossed`: never glide PAST the prompt — provisional
      // row heights can overshoot the unmeasured reservation's bottom. A
      // CLAMPED write (the scroll range ends short of the target) is the
      // same verdict: the hold's floor IS the resting place.
      if (el.scrollTop > target || (el.scrollTop >= this.#maxScroll() - 0.5 && el.scrollTop < target)) {
        land();
      } else {
        this.#schedule();
      }
    }
    this.#prevDistance = this.#distance();
    return true;
  }

  /** Retire the runway (filled, or the prompt disappeared terminally). */
  #retireOwnTurn(): void {
    this.#ownTurn = null;
    this.#ownTurnLastTick = null;
    this.#prevDistance = this.#distance();
    this.#setJumpShown(jumpVisibility(this.#jumpShown, this.#prevDistance));
    this.#onOwnTurnChange();
  }
}
