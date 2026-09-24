import { useLayoutEffect, useRef, useState } from "react";
import type { ReactNode } from "react";
import { motion } from "@zeron/theme";
import { evalWidthTween } from "../state/layout";
import { useIsPhone, usePrefersReducedMotion } from "../state/media";
import { resolvedActive, type ChatPaneState } from "../state/right-pane";
import { renderRightSurface, surfaceEntry } from "./surface-registry";
import { RightTabStrip } from "./right-tab-strip";

/**
 * The right pane — the desktop's surface host (`render_right_pane`).
 *
 * It is a flush, left-bordered glass panel beside the conversation column, not
 * an inset card, and it is a sibling of that column at the SHELL level (the
 * desktop's root row is `[sidebar][conversation][right pane]`). That placement
 * is what lets its surface and its seam hairline run the full window height,
 * behind the overlaid titlebar, the way `render_right_pane` does; only its
 * body pads down past the titlebar band. Its tabs live up in that band (see
 * `RightTabStrip`), and its drag seam is a shell child too, because this
 * column clips.
 *
 * Geometry is `right_pane_container` (`shell.rs:3826`): the OUTER width rides
 * the 200ms resize curve while an inner, right-anchored child holds the wider
 * endpoint's width for the duration. The surface therefore slides out from the
 * window's edge at a fixed layout width instead of reflowing through every
 * intermediate one — the same clip-don't-squeeze trick the sidebar uses,
 * mirrored. The column stays mounted at width 0 while closed, because a CSS
 * width transition has nothing to animate from if the element is absent.
 *
 * EXCEPT on a takeover flip: `right_takeover_content_tween` (shell.rs:3833-3838)
 * gives the inner child the SAME tween endpoints as the column, so the
 * surface's layout width tracks the animating width per frame — the pane
 * really is changing to a different width there, and holding one end would
 * leave the surface laid out wrong for the whole 200ms (and snap at the end).
 * A rAF loop ports `eval_tween` per frame, the pane-seam bounce's pattern.
 *
 * The pane's edge bounce (`eval_resize_edge_bounce`) adds its offset through
 * the `--rb-pane-edge-offset` var the shell composes in — the desktop's
 * `+ edge_offset` on the container width, driven by the seam.
 *
 * At phone widths (ticket 52) the column's FORM is a right-side overlay
 * drawer — the mirror of the left sidebar drawer — while this component's
 * surface bookkeeping is unchanged: the width glide and the inner's held
 * width are desktop concerns the phone CSS simply outweighs (`width: 100%
 * !important` beats the inline writes), the surfaces still stay mounted
 * through the close, and the tabs that lived in the titlebar band render
 * in a 38px strip header INSIDE the drawer instead (the band renders no
 * pane tabs at phone). The drawer's CSS pads that whole content below the
 * titlebar band (web-bugs 14): the bar is a transparent, hit-testable
 * overlay ABOVE the drawer, so the strip header starts at the bar's
 * bottom edge — visible AND clickable — while the bar's own cluster keeps
 * its documented clickability to close what it opened.
 */

/** `motion::RESIZE` — the same 200ms the stylesheet transitions on. */
const RESIZE_MS = motion.specs.find((spec) => spec.name === "resize")?.durationMs ?? 200;

/** `motion::MENU_IN` — the phone drawer slide's 140ms (app.css's `.right-pane` phone block). */
const MENU_IN_MS = motion.specs.find((spec) => spec.name === "menuIn")?.durationMs ?? 140;

export function RightPane({
  chatId,
  pane,
  openWidth,
  glide,
}: {
  chatId: string;
  pane: ChatPaneState;
  /** What the pane resolves to when open — its width, and its content's. */
  openWidth: number;
  /** Owned by the shell, which needs the same glide for the conversation. */
  glide: PaneGlide;
}) {
  const active = resolvedActive(pane);
  const closing = !pane.open;
  const phone = useIsPhone();
  const asideRef = useRef<HTMLElement | null>(null);
  // Ticket 72: an expanded close slides away at the width the user was
  // viewing — the logical flags already reset, this holds only the width.
  const holdExpandedClose = usePhoneExpandedCloseHold(chatId, phone, pane.open, pane.expanded, asideRef);
  // The File family stays unmounted throughout the closing animation after
  // its resources are suspended (`shell.rs:6455-6459`); everything else
  // renders until the glide finishes.
  const filesWhileClosing = closing && active.kind === "file";
  const entry = surfaceEntry(active.kind);
  const ctx = { chatId };
  const innerRef = useRef<HTMLDivElement | null>(null);

  let content: ReactNode = null;
  if (!filesWhileClosing) {
    const body = renderRightSurface(active, ctx);
    const toolbar = entry?.toolbar?.(active, ctx);
    content =
      toolbar === undefined ? body : (
        <>
          {toolbar}
          <div className="right-pane-surface">{body}</div>
        </>
      );
  }

  // `right_takeover_content_tween`: while a takeover glide runs, drive the
  // inner width per frame on the tween's own clock. The first write lands
  // before paint (a layout effect), so the surface never renders at a
  // shrink-to-fit width; the loop's final write is `to`, which is exactly
  // where the settled inline style takes over. Keyed on the tween object's
  // identity, which only changes when a glide arms or clears.
  const tween = glide.tween;
  useLayoutEffect(() => {
    if (tween === null) {
      return;
    }
    const inner = innerRef.current;
    if (inner === null) {
      return;
    }
    if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
      // The CSS has already snapped the column; the content must not trail.
      inner.style.width = `${tween.to}px`;
      return;
    }
    const { from, to } = tween;
    const started = performance.now();
    let raf = 0;
    const write = (elapsed: number): void => {
      inner.style.width = `${evalWidthTween(from, to, elapsed)}px`;
    };
    write(0);
    const tick = (now: number): void => {
      const elapsed = now - started;
      write(elapsed);
      if (elapsed < RESIZE_MS) {
        raf = requestAnimationFrame(tick);
      }
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [tween]);

  // The chat-switch snap (shell.rs:1837-1862): when the pane KEY (the owning
  // chat) changed in this commit, the width lands at the destination's flags
  // immediately — the desktop clears `right_tween` & co ("snap, no tween —
  // the panels belong to the destination chat"). The flag drops on the next
  // commit (`data-pane-snap` in app.css kills the width transition for that
  // one commit only), so the pane's OWN same-chat open/close/takeover glides
  // are untouched: by the time a toggle changes the width, the ref below has
  // already caught up and the transition is back.
  const previousPaneKeyRef = useRef(chatId);
  const paneKeyChanged = previousPaneKeyRef.current !== chatId;
  useLayoutEffect(() => {
    previousPaneKeyRef.current = chatId;
  });

  return (
    <aside
      ref={asideRef}
      className={`right-pane ${pane.expanded ? "right-pane-expanded" : ""}${holdExpandedClose ? " right-pane-closing-expanded" : ""}`}
      data-pane-snap={paneKeyChanged ? "1" : "0"}
      // At phone the drawer's width is CSS-owned (`min(30rem, 88vw)`, and
      // `100vw` expanded) — the inline column width is the desktop glide's
      // input and is skipped so it cannot fight the drawer rule.
      style={phone ? undefined : { width: `calc(${pane.open ? openWidth : 0}px + var(--rb-pane-edge-offset, 0px))` }}
      aria-label="Panel"
      aria-hidden={!pane.open}
    >
      {/*
        The phone strip header (§2.2): the tabs that the titlebar band
        carries at desktop widths render inside the drawer at phone, above
        the pane body. The drawer's CSS pad owns the titlebar clearance —
        this row starts at the bar's bottom edge, never inside its
        hit-testable band (web-bugs 14). Mounted through the close glide
        like everything else — `aria-hidden` hides the tree while the
        drawer is shut.
      */}
      {phone && (
        <div className="right-pane-strip-header">
          <RightTabStrip chatId={chatId} pane={pane} />
        </div>
      )}
      {/*
        A takeover glide drives this width per frame (the effect above); any
        other glide holds the wider endpoint's width; `null` only ever means
        "the tween owns it", never "follow the column" — the inner is
        absolutely positioned and shrink-to-fits without a width.
      */}
      <div
        ref={innerRef}
        className="right-pane-inner"
        style={glide.content === null ? undefined : { width: glide.content }}
      >
        {/*
          Surfaces stay mounted through the closing glide and leave with it —
          unmounting on the first frame would empty the panel before it moves.
        */}
        {glide.mounted && <div className="right-pane-body">{content}</div>}
      </div>
    </aside>
  );
}

/** An armed width tween's endpoints — the `from`/`to` `eval_tween` lerps. */
export interface PaneGlideTween {
  readonly from: number;
  readonly to: number;
}

/**
 * The desktop's `right_panel_content_width`, as a hook — and the two cases it
 * deliberately treats differently:
 *
 * - **Open / close** (`toggle_right_pane`) leaves
 *   `right_takeover_content_tween` unset, so the content width is
 *   `stable_panel_content_width` — the LARGER endpoint, held for the glide.
 *   The surface keeps its geometry and is revealed or clipped away rather than
 *   reflowing through every intermediate width. It also stays mounted for the
 *   duration, so a close animates instead of blinking out.
 * - **Takeover** (`toggle_right_pane_expand`) sets that tween to the same
 *   endpoints as the column, so the content TRACKS the animating width. The
 *   pane really is changing to a different width here, and holding one end
 *   would leave the surface laid out wrong for the whole 200ms.
 *
 * Drags are excluded by construction: they change neither flag, and must track
 * the pointer exactly rather than lag behind a held width.
 *
 * `gliding` is the tween-in-flight flag the shell needs for the seam guard:
 * the right resize handle is unmounted while a glide runs (`shell.rs:7943`),
 * because takeover derives its width from the viewport and a manual drag
 * would fight the target.
 */
export function usePaneGlide(
  open: boolean,
  expanded: boolean,
  openWidth: number,
): { mounted: boolean; content: number | null; gliding: boolean; tween: PaneGlideTween | null } {
  const [glide, setGlide] = useState<{ held: number | null; tween: PaneGlideTween | null } | null>(null);
  const previous = useRef({ open, expanded, openWidth });

  // A layout effect, not a passive one: arming in the flip's own commit means
  // the first painted frame of the glide already carries the held (or
  // tweened) content width — arming a frame late painted the new endpoint
  // for one frame and then yanked it back.
  useLayoutEffect(() => {
    const was = previous.current;
    previous.current = { open, expanded, openWidth };
    if (was.open === open && was.expanded === expanded) {
      // A drag (or a window resize): retarget with no glide and no hold.
      return;
    }
    // Takeover tracks; an open/close — including closing out of takeover,
    // which resets `expanded` — holds the wider endpoint
    // (`toggle_right_pane` never arms the content tween).
    const takeoverFlip = was.open && open && was.expanded !== expanded;
    const held = takeoverFlip ? null : Math.max(was.open ? was.openWidth : 0, open ? openWidth : 0);
    const tween = takeoverFlip
      ? { from: was.open ? was.openWidth : 0, to: open ? openWidth : 0 }
      : null;
    setGlide({ held, tween });
    const timer = window.setTimeout(() => setGlide(null), RESIZE_MS);
    return () => window.clearTimeout(timer);
  }, [open, expanded, openWidth]);

  if (glide === null) {
    return { mounted: open, content: openWidth, gliding: false, tween: null };
  }
  // Mid-glide the surface is mounted whichever way the column is moving.
  return {
    mounted: true,
    content: glide.held === null ? null : Math.max(glide.held, openWidth),
    gliding: true,
    tween: glide.tween,
  };
}

export type PaneGlide = ReturnType<typeof usePaneGlide>;

/**
 * Ticket 72 — the phone drawer's expanded-close width hold.
 *
 * The phone drawer slides shut on a 140ms transform transition, but
 * `toggle`/`close` reset `open:false, expanded:false` in the SAME commit
 * that starts the slide (right-pane.ts:250-254,303-304), and the aside's
 * width class read the logical flag directly — so an expanded close
 * narrowed 100vw → `min(30rem, 88vw)` on the slide's first frame. The
 * desktop's close holds stable inner geometry while its outer width glides
 * (`shell.rs:3850-3875`, and the takeover reset at `:1987-1995`); this hold
 * is the phone drawer's corresponding presentation-continuity contract.
 *
 * The hold is deliberately NOT pane state: the logical flags still clear
 * immediately (a reopen lands in normal mode — §5), and nothing here is
 * persisted. It is a transient, component-local presentation value keyed to
 * the pane owner, armed only on the expanded-open → closed edge, released
 * by the drawer's own transform `transitionend` (or a deterministic
 * fallback), and canceled by reopen, owner switch, breakpoint flip, or
 * reduced motion. The two decision functions below are the pure core the
 * node-environment tests drive (`tests/phone-pane-close.test.ts`); the
 * hook wires them to commits and the DOM settle events.
 */

/** The inputs the hold decision reads: owner, width layer, logical flags. */
export interface PhonePaneSnapshot {
  /** The pane's owner — the chat (or panel key) the drawer belongs to. */
  readonly owner: string;
  /** The phone width layer (`useIsPhone`'s arm). */
  readonly phone: boolean;
  readonly open: boolean;
  readonly expanded: boolean;
}

/**
 * Arm iff this commit is the expanded-open → closed edge at phone width:
 * the same owner on both sides, the phone layer on both sides, and the
 * logical flags going open+expanded → shut. Every close entry point funnels
 * through the store, so this one edge covers toggle, Escape and backdrop
 * alike. A normal-width close never arms (no width would change), and
 * reduced motion never arms — the CSS snaps the transform
 * (`transition: none`), so there are no frames to hold through and a hold
 * would only be a stale 100vw closed state.
 */
export function armsExpandedCloseHold(
  was: PhonePaneSnapshot,
  now: PhonePaneSnapshot,
  reducedMotion: boolean,
): boolean {
  return (
    !reducedMotion &&
    was.phone &&
    now.phone &&
    was.owner === now.owner &&
    was.open &&
    was.expanded &&
    !now.open
  );
}

/**
 * Release iff the held close can no longer settle as captured: the owner
 * changed (another chat's pane state now drives the drawer — the
 * destination's state wins), the viewport left the phone layer (the
 * desktop width/glide rules take over), or the pane reopened (a reopen is
 * a fresh open in normal mode — a stale completion must not narrow or
 * hide it). A still-closed commit on the same owner releases nothing: the
 * hold ends with the slide, not with a stray re-render.
 */
export function releasesExpandedCloseHold(holdOwner: string, now: PhonePaneSnapshot): boolean {
  return holdOwner !== now.owner || !now.phone || now.open;
}

/**
 * The hold as a hook: `true` while the aside must keep the expanded
 * presentation width (`right-pane-closing-expanded`) although the logical
 * `expanded` has already cleared. Phone-only by construction — the class
 * has no rule outside the `@media (max-width: 768px)` block regardless.
 */
export function usePhoneExpandedCloseHold(
  chatId: string,
  phone: boolean,
  open: boolean,
  expanded: boolean,
  asideRef: { readonly current: HTMLElement | null },
): boolean {
  const [holdOwner, setHoldOwner] = useState<string | null>(null);
  const reducedMotion = usePrefersReducedMotion();
  const previous = useRef<PhonePaneSnapshot>({ owner: chatId, phone, open, expanded });
  const now: PhonePaneSnapshot = { owner: chatId, phone, open, expanded };

  // Arm/release on commit — a layout effect so the close's first painted
  // frame already carries the held width; arming a frame late would paint
  // the narrowed width once and then yank it back.
  useLayoutEffect(() => {
    const was = previous.current;
    previous.current = now;
    setHoldOwner((current) => {
      if (current !== null && releasesExpandedCloseHold(current, now)) {
        return null;
      }
      if (current === null && armsExpandedCloseHold(was, now, reducedMotion)) {
        return now.owner;
      }
      return current;
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [chatId, phone, open, expanded]);

  // While armed, the hold ends with the drawer's own transform transition —
  // guarded to the aside's `transform` so a child's transitionend bubbling
  // up cannot settle it early — with deterministic releases for everything
  // the event cannot cover: `transitioncancel`, and a fallback clock just
  // past the slide for any silent miss. Reopen, owner switch and breakpoint
  // flip release on the commit above; unmount runs this effect's cleanup,
  // so a routed-away hold dies with the component.
  useLayoutEffect(() => {
    if (holdOwner === null) {
      return;
    }
    const aside = asideRef.current;
    const release = (): void => setHoldOwner(null);
    const onTransitionSettle = (event: TransitionEvent): void => {
      if (event.target === aside && event.propertyName === "transform") {
        release();
      }
    };
    aside?.addEventListener("transitionend", onTransitionSettle);
    aside?.addEventListener("transitioncancel", onTransitionSettle);
    const fallback = window.setTimeout(release, MENU_IN_MS + 100);
    return () => {
      aside?.removeEventListener("transitionend", onTransitionSettle);
      aside?.removeEventListener("transitioncancel", onTransitionSettle);
      window.clearTimeout(fallback);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [holdOwner]);

  // A reduced-motion flip mid-close (the CSS kill ends the transition
  // without an end event): the shared hook re-renders on the flip, and the
  // hold must not survive it.
  useLayoutEffect(() => {
    if (holdOwner !== null && reducedMotion) {
      setHoldOwner(null);
    }
  }, [holdOwner, reducedMotion]);

  // Derived, not raw state: even between a releasing commit's render and
  // its layout effect, the class can never describe a state the release
  // rule already condemns.
  return holdOwner !== null && !releasesExpandedCloseHold(holdOwner, now);
}
