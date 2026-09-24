// @vitest-environment jsdom

/**
 * Ticket 01 — the portaled nested menu family (`ui/NestedMenu.tsx` on
 * `base/positioning.ts`'s nested preset): the placement numbers
 * (`nested_menu`, popover.rs:584-612 — side, top-align, the
 * CARD_INSET+ANCHOR_GAP card-to-card gap; `anchored_menu_right`,
 * popover.rs:526-549), the desktop arm's BODY portal (the shared
 * bug-2/5b/9 root cause — an inline absolute submenu clipped by an
 * `overflow: hidden` ancestor), the flyout's membership in the parent
 * popup tree (presses inside it never dismiss the parent card; Escape
 * closes the nested menu alone — popover.rs:586-589's "shares the
 * parent's interaction surface"), and the phone arm's drill-down
 * (ticket 15's in-place expansion under a back header, never a side
 * flyout). The mounted idiom follows composer-reasoning.test.ts: no JSX,
 * per-file jsdom pragma, Base UI popovers in jsdom behind matchMedia /
 * ResizeObserver / scrollIntoView stubs.
 */

import { act, createElement, useState } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import {
  anchorHelperPlacement,
  ANCHOR_GAP,
  CARD_INSET,
  nestedMenuPlacement,
  NO_FLIP_COLLISION_AVOIDANCE,
  noFlipPositionerProps,
  SNAP_MARGIN,
} from "../src/components/base/positioning";
import { createRbPopoverHandle, RbPopover, RbPopoverTrigger } from "../src/components/base/popover";
import { NestedMenu } from "../src/components/ui/NestedMenu";

// ── jsdom gaps the mounted cards hit (composer-reasoning.test.ts's set) ──────

/** The useIsPhone answer for every mount in this file (PHONE_QUERY match). */
let phoneMode = false;

beforeAll(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  window.matchMedia = ((query: string) => ({
    // `(max-width: 768px)` matches in phone mode; `(min-width: 769px)` in
    // desktop mode — the exact pair `state/media.ts` derives from the one
    // breakpoint, so the two queries can never disagree.
    matches: query.startsWith("(max-width") === phoneMode,
    media: query,
    onchange: null,
    addEventListener: () => {},
    removeEventListener: () => {},
    addListener: () => {},
    removeListener: () => {},
    dispatchEvent: () => false,
  })) as unknown as typeof window.matchMedia;
  globalThis.ResizeObserver = class {
    observe(): void {}
    unobserve(): void {}
    disconnect(): void {}
  };
  if (typeof Element.prototype.scrollIntoView !== "function") {
    Element.prototype.scrollIntoView = () => {};
  }
  if (typeof globalThis.requestAnimationFrame !== "function") {
    globalThis.requestAnimationFrame = ((callback: FrameRequestCallback) => {
      callback(0);
      return 0;
    }) as typeof requestAnimationFrame;
  }
});

afterAll(() => {
  delete (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT;
});

// ── The mounted NestedMenu harness ───────────────────────────────────────────

interface MountedNested {
  /** The clipping host the row renders inside (the `.sidebar`/`.popover-card` stand-in). */
  readonly container: HTMLDivElement;
  /** Every onOpenChange the surface saw, as [open, reason] pairs. */
  readonly events: Array<[boolean, string]>;
  row(): HTMLElement | null;
  drill(): HTMLElement | null;
  /** The portaled flyout card — queried on the document, not the container. */
  popup(): HTMLElement | null;
  pressEscape(): void;
  unmount(): void;
}

const mounted: Array<() => void> = [];

afterEach(() => {
  while (mounted.length > 0) {
    mounted.pop()!();
  }
});

function mountNested(options?: { phone?: boolean; open?: boolean }): MountedNested {
  phoneMode = options?.phone ?? false;
  const container = document.createElement("div");
  container.className = "clipping-host";
  document.body.appendChild(container);
  const root = createRoot(container);
  const events: Array<[boolean, string]> = [];

  function Host() {
    const [open, setOpenState] = useState(options?.open ?? false);
    return createElement(
      // The clipping ancestor: everything absolutely positioned inside it
      // paints nothing (bug 2's `.sidebar`, bug 5b/9's `.popover-card`).
      "div",
      { className: "clip-box", style: { overflow: "hidden", position: "relative" } },
      createElement(NestedMenu, {
        open,
        onOpenChange: (next: boolean, details: { reason: string }) => {
          events.push([next, details.reason]);
          setOpenState(next);
        },
        trigger: createElement("button", { type: "button", className: "group-row" }, "Reasoning"),
        label: "Reasoning",
        ariaLabel: "Reasoning choices",
        children: createElement("button", { type: "button", className: "choice-row" }, "High"),
      }),
    );
  }

  act(() => {
    root.render(createElement(Host));
  });
  const unmount = (): void => {
    act(() => {
      root.unmount();
    });
    container.remove();
  };
  mounted.push(unmount);
  return {
    container,
    events,
    row: () => container.querySelector<HTMLElement>(".group-row"),
    drill: () => container.querySelector<HTMLElement>(".rb-submenu-drill"),
    // No `cardClassName` is passed by this harness, so the query also locks
    // the component's own `.popover-card` default (the glass card).
    popup: () => document.querySelector<HTMLElement>(".rb-popover-popup"),
    pressEscape: () => {
      act(() => {
        document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
      });
    },
    unmount,
  };
}

/** A real press pair on `target`: pointerdown (marks the press) then click. */
function press(target: HTMLElement): void {
  act(() => {
    target.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true, button: 0 }));
    target.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, detail: 1 }));
  });
}

// ── The placement presets (pure, popover.rs citations) ──────────────────────

describe("nestedMenuPlacement (popover.rs:584-612)", () => {
  it("opens beside the row on the chosen side, top-aligned, at the card-to-card gap", () => {
    // The anchor point sits CARD_INSET (4) + ANCHOR_GAP (6) beyond the row's
    // edge — the row is inset 4px inside the parent card, so the visual gap
    // between the two cards reads 6px, the desktop's `-(CARD_INSET + 6.0)`.
    expect(CARD_INSET).toBe(4);
    expect(nestedMenuPlacement("right")).toEqual({ side: "right", align: "start", sideOffset: 10 });
    expect(nestedMenuPlacement("left")).toEqual({ side: "left", align: "start", sideOffset: 10 });
  });

  it("rides the clamp-only preset: chosen side never flips, 8px window gutter, fixed", () => {
    const props = noFlipPositionerProps(nestedMenuPlacement("right"));
    expect(props.collisionAvoidance).toEqual(NO_FLIP_COLLISION_AVOIDANCE);
    expect(props.collisionPadding).toBe(SNAP_MARGIN);
    expect(props.positionMethod).toBe("fixed");
    expect(props.side).toBe("right");
    expect(props.sideOffset).toBe(10);
  });
});

describe("anchorRight (popover.rs:526-549)", () => {
  it("pins the card's top-left at the trigger's top-right + 6 — the user-menu shape", () => {
    expect(anchorHelperPlacement("anchorRight")).toEqual({ side: "right", align: "start" });
    expect(noFlipPositionerProps(anchorHelperPlacement("anchorRight")).sideOffset).toBe(ANCHOR_GAP);
  });
});

// ── The desktop arm: the portaled flyout ─────────────────────────────────────

describe("NestedMenu desktop arm", () => {
  it("portals the flyout to the body, escaping the clipping ancestor", () => {
    const handle = mountNested({ open: true });
    const popup = handle.popup();
    expect(popup).not.toBeNull();
    // The portal is the fix: the card paints in the body, not inside the
    // overflow:hidden host — bug 2/5b/9's invisible-inline-absolute class.
    expect(handle.container.contains(popup!)).toBe(false);
    expect(document.body.contains(popup!)).toBe(true);
    expect(popup!.getAttribute("role")).toBe("menu");
    expect(popup!.getAttribute("aria-label")).toBe("Reasoning choices");
    // `frosted_menu`'s default: no `cardClassName` was passed, so the card
    // IS the shared glass card, and it rides the shared menu tier
    // (`.rb-popover-positioner`, `--rb-z-menu` — desktop `priority(2)`'s
    // DOM-order equivalent, since the nested portal mounts after the
    // parent's).
    expect(popup!.classList.contains("popover-card")).toBe(true);
    expect(popup!.closest(".rb-popover-positioner")).not.toBeNull();
    // The flyout takes no focus by default (`initialFocus: false`): the
    // parent card's focused element keeps the cursor walk (ticket 07/08's
    // keyboard model rides on this).
    expect(document.activeElement).toBe(document.body);
  });

  it("opens through the row's press (trigger-press) and closes on Escape", () => {
    const handle = mountNested({ open: false });
    expect(handle.popup()).toBeNull();
    press(handle.row()!);
    expect(handle.events).toContainEqual([true, "trigger-press"]);
    expect(handle.popup()).not.toBeNull();
    // Base UI's Trigger adoption: the row carries the expanded state.
    expect(handle.row()!.getAttribute("aria-expanded")).toBe("true");
    handle.pressEscape();
    expect(handle.events).toContainEqual([false, "escape-key"]);
    expect(handle.popup()).toBeNull();
    expect(handle.row()!.getAttribute("aria-expanded")).toBe("false");
  });

  it("dismisses on an outside press", () => {
    // The outside target must exist BEFORE the popover opens: Base UI marks
    // the pre-existing outside elements `data-base-ui-inert` when the popup
    // opens, and a target injected after open reads as a third-party
    // element the dismissal pass deliberately ignores. Real app DOM exists
    // before any popover opens, so this is the faithful shape.
    const outside = document.createElement("button");
    document.body.appendChild(outside);
    const handle = mountNested({ open: true });
    press(outside);
    outside.remove();
    expect(handle.events).toContainEqual([false, "outside-press"]);
  });

  it("opens on hover after the intent delay (trigger-hover), the corridor owning the close", () => {
    // The desktop's nested menus open on hover intent (pickers.rs:4095-4132,
    // popover/hover_intent.rs). Base UI encodes the same contract: a 100ms
    // rest window before the open, then safePolygon + a 100ms close grace
    // so a pointer crossing the 6px card gap never drops the flyout. Only
    // setTimeout is faked — the positioner's rAF stays real.
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    try {
      const handle = mountNested({ open: false });
      act(() => {
        handle.row()!.dispatchEvent(new MouseEvent("mouseenter"));
        handle.row()!.dispatchEvent(new MouseEvent("mousemove", { bubbles: true }));
      });
      // Still inside the rest window — nothing opened yet.
      expect(handle.popup()).toBeNull();
      act(() => {
        vi.advanceTimersByTime(120);
      });
      expect(handle.events).toContainEqual([true, "trigger-hover"]);
      expect(handle.popup()).not.toBeNull();
      // Leaving the row arms the corridor: after the close grace the flyout
      // dismisses with the hover reason (a pointer racing the 6px gap stays
      // open — safePolygon's geometry, Base UI's own tested layer).
      act(() => {
        handle.row()!.dispatchEvent(new MouseEvent("mouseleave", { clientX: 999, clientY: 999 }));
      });
      act(() => {
        vi.advanceTimersByTime(120);
      });
      expect(handle.events).toContainEqual([false, "trigger-hover"]);
      expect(handle.popup()).toBeNull();
    } finally {
      vi.useRealTimers();
    }
  });
});

// ── The desktop arm inside a real parent popover: the tree contract ─────────

describe("NestedMenu inside a parent popover (popover.rs:586-589 — shares the surface)", () => {
  interface MountedParent {
    readonly container: HTMLDivElement;
    readonly nestedEvents: Array<[boolean, string]>;
    readonly parentEvents: Array<[boolean, string]>;
    /** An outside target that exists BEFORE the popovers open (a real press reads as one). */
    readonly outside: HTMLButtonElement;
    nestedPopup(): HTMLElement | null;
    parentPopup(): HTMLElement | null;
    choice(): HTMLElement | null;
    pressEscape(): void;
    unmount(): void;
  }

  function mountWithParent(): MountedParent {
    phoneMode = false;
    // Pre-exists the open (see the desktop arm's outside-press test): the
    // inert pass marks it, so presses on it are real outside presses.
    const outside = document.createElement("button");
    document.body.appendChild(outside);
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);
    const nestedEvents: Array<[boolean, string]> = [];
    const parentEvents: Array<[boolean, string]> = [];

    function Host() {
      const [parentOpen, setParentOpen] = useState(true);
      const [nestedOpen, setNestedOpen] = useState(true);
      const [handle] = useState(() => createRbPopoverHandle());
      return createElement(
        "div",
        null,
        createElement(RbPopoverTrigger, {
          handle,
          render: createElement("button", { type: "button", className: "parent-chip" }, "Chip"),
        }),
        createElement(RbPopover, {
          handle,
          open: parentOpen,
          onOpenChange: (next: boolean, details: { reason: string }) => {
            parentEvents.push([next, details.reason]);
            setParentOpen(next);
          },
          placement: "anchorBelow",
          cardClassName: "popover-card test-parent-card",
          initialFocus: false,
          children: createElement(NestedMenu, {
            open: nestedOpen,
            onOpenChange: (next: boolean, details: { reason: string }) => {
              nestedEvents.push([next, details.reason]);
              setNestedOpen(next);
            },
            trigger: createElement("button", { type: "button", className: "group-row" }, "Group"),
            label: "Group",
            ariaLabel: "Group choices",
            cardClassName: "popover-card test-nested-card",
            children: createElement("button", { type: "button", className: "choice-row" }, "Choice"),
          }),
        }),
      );
    }

    act(() => {
      root.render(createElement(Host));
    });
    const unmount = (): void => {
      act(() => {
        root.unmount();
      });
      container.remove();
    };
    mounted.push(unmount);
    return {
      container,
      nestedEvents,
      parentEvents,
      outside,
      nestedPopup: () => document.querySelector<HTMLElement>(".rb-popover-popup.test-nested-card"),
      parentPopup: () => document.querySelector<HTMLElement>(".rb-popover-popup.test-parent-card"),
      choice: () => document.querySelector<HTMLElement>(".rb-submenu-choice, .choice-row"),
      pressEscape: () => {
        act(() => {
          document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
        });
      },
      unmount,
    };
  }

  it("the flyout escapes the parent card's clip box — portaled beside, not inside it", () => {
    const handle = mountWithParent();
    const parent = handle.parentPopup();
    const nested = handle.nestedPopup();
    expect(parent).not.toBeNull();
    expect(nested).not.toBeNull();
    // Bug 5b's root cause inverted: the submenu is NOT a child of the
    // parent card (whose overflow:hidden would clip it) — both ride the
    // body portal.
    expect(parent!.contains(nested!)).toBe(false);
    expect(document.body.contains(nested!)).toBe(true);
  });

  it("presses inside the flyout never dismiss the parent card", () => {
    const handle = mountWithParent();
    const choice = handle.choice();
    expect(choice).not.toBeNull();
    press(choice!);
    expect(handle.parentEvents.filter(([open]) => !open)).toEqual([]);
    expect(handle.nestedEvents.filter(([open]) => !open)).toEqual([]);
    // The counterfactual, so the above is not vacuous: a real outside press
    // on the pre-existing target DOES dismiss the parent card.
    press(handle.outside);
    expect(handle.parentEvents).toContainEqual([false, "outside-press"]);
  });

  it("Escape closes the nested menu alone, leaving the parent card open", () => {
    const handle = mountWithParent();
    handle.pressEscape();
    expect(handle.nestedEvents).toContainEqual([false, "escape-key"]);
    expect(handle.parentEvents.filter(([open]) => !open)).toEqual([]);
    expect(handle.nestedPopup()).toBeNull();
    expect(handle.parentPopup()).not.toBeNull();
  });
});

// ── The phone arm: the drill-down (ticket 15's mobile surface rule) ──────────

describe("NestedMenu phone arm", () => {
  it("expands the choices in place under a back header — never a portal flyout", () => {
    const handle = mountNested({ phone: true, open: true });
    // No Base UI popup mounts at all: the phone arm renders plain DOM.
    expect(document.querySelector(".rb-popover-popup")).toBeNull();
    const drill = handle.drill();
    expect(drill).not.toBeNull();
    expect(handle.container.contains(drill!)).toBe(true);
    expect(drill!.getAttribute("role")).toBe("group");
    // The back affordance: the altArrowLeft mark plus the group's label.
    const header = drill!.querySelector<HTMLElement>(".rb-submenu-drill-header");
    expect(header).not.toBeNull();
    expect(header!.textContent).toContain("Reasoning");
    expect(drill!.querySelector(".rb-submenu-drill-body .choice-row")).not.toBeNull();
    // The row carries the expanded state (the desktop arm's data-popup-open
    // stand-in).
    expect(handle.row()!.getAttribute("aria-expanded")).toBe("true");
  });

  it("the row's press toggles the drill through onOpenChange(trigger-press)", () => {
    const handle = mountNested({ phone: true, open: false });
    expect(handle.drill()).toBeNull();
    press(handle.row()!);
    expect(handle.events).toContainEqual([true, "trigger-press"]);
    expect(handle.drill()).not.toBeNull();
  });

  it("the back header closes the drill (close-press)", () => {
    const handle = mountNested({ phone: true, open: true });
    const header = handle.drill()!.querySelector<HTMLElement>(".rb-submenu-drill-header");
    press(header!);
    expect(handle.events).toContainEqual([false, "close-press"]);
    expect(handle.drill()).toBeNull();
    expect(handle.row()!.getAttribute("aria-expanded")).toBe("false");
  });
});
