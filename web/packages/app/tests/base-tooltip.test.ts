// @vitest-environment jsdom

/**
 * Ticket 18 — `base/tooltip.tsx`'s virtual-anchor extension: the label
 * tooltip family gains the mode the composer's mention tooltip needs —
 * `anchor` (a `virtualAnchorAt(x, y)` point instead of the trigger part),
 * a controlled `open` (no trigger part arms the delay), and `popupRef`
 * (the caller's own hover hit-test reads the live popup rect). The
 * trigger mode's contract is untouched: the label opens after the delay
 * on hover and closes on leave, the popup rides the body portal, and the
 * positioner carries the menu tier (`rb-tooltip-positioner`).
 *
 * The mounted idiom follows nested-menu.test.ts: no JSX, per-file jsdom
 * pragma, Base UI tooltips in jsdom behind the matchMedia /
 * ResizeObserver stubs.
 */

import { act, createElement, useState, type Ref } from "react";
import { createRoot } from "react-dom/client";
import { afterAll, afterEach, beforeAll, describe, expect, it } from "vitest";
import { RbTooltip, RbTooltipTrigger } from "../src/components/base/tooltip";
import { Tooltip } from "../src/components/ui/Tooltip";
import { virtualAnchorAt } from "../src/components/base/positioning";

// ── jsdom gaps the mounted popups hit ───────────────────────────────────────

let phoneMode = false;

beforeAll(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  window.matchMedia = ((query: string) => ({
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
});

afterAll(() => {
  delete (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT;
});

const unmounts: Array<() => void> = [];

afterEach(() => {
  while (unmounts.length > 0) {
    unmounts.pop()!();
  }
  document.body.replaceChildren();
});

/** The shared pop state — the virtual-anchor consumer's seam. */
interface Mounted {
  readonly container: HTMLDivElement;
  setOpen(next: boolean): void;
  unmount(): void;
}

function mountVirtualAnchor(label: string, popupRef?: Ref<HTMLDivElement>): Mounted {
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  function Host() {
    const [open, setOpenState] = useState(true);
    (globalThis as { __setOpen?: (next: boolean) => void }).__setOpen = setOpenState;
    return createElement(RbTooltip, {
      label,
      open,
      anchor: virtualAnchorAt(120, 40),
      placement: { side: "top", align: "start", sideOffset: 1 },
      popupClassName: "mention-tooltip",
      popupRef,
    });
  }
  act(() => {
    root.render(createElement(Host));
  });
  const handle: Mounted = {
    container,
    setOpen(next: boolean) {
      act(() => {
        (globalThis as { __setOpen?: (next: boolean) => void }).__setOpen!(next);
      });
    },
    unmount() {
      act(() => {
        root.unmount();
      });
      container.remove();
    },
  };
  unmounts.push(() => handle.unmount());
  return handle;
}

describe("RbTooltip virtual-anchor mode (ticket 18)", () => {
  it("a controlled open renders the label in the body portal with the positioner's menu tier", () => {
    const handle = mountVirtualAnchor("src/lib.rs");
    const popup = document.querySelector<HTMLElement>(".rb-tooltip-popup.mention-tooltip");
    expect(popup).not.toBeNull();
    expect(popup!.textContent).toBe("src/lib.rs");
    // The portal: the popup paints at the document body, never inside the
    // host container — the composer pill's clip box cannot swallow it.
    expect(popup!.closest("body")).toBe(document.body);
    expect(handle.container.contains(popup!)).toBe(false);
    // The positioner carries the shared menu tier (app.css's
    // `.rb-tooltip-positioner`); the popup itself no longer sets one.
    expect(document.querySelector(".rb-tooltip-positioner")).not.toBeNull();
  });

  it("closing through the controlled seam unmounts the popup — the mount-while-open contract", () => {
    const handle = mountVirtualAnchor("src/lib.rs");
    expect(document.querySelector(".rb-tooltip-popup")).not.toBeNull();
    handle.setOpen(false);
    expect(document.querySelector(".rb-tooltip-popup")).toBeNull();
  });

  it("popupRef receives the live popup element — the consumer's hover hit-test seam", () => {
    const ref: { current: HTMLDivElement | null } = { current: null };
    mountVirtualAnchor("src/lib.rs", ref);
    // The ref lands the popup element itself (the mention tooltip's
    // pointer-inside-the-popup check reads its rect).
    expect(ref.current).not.toBeNull();
    expect(ref.current!.classList.contains("mention-tooltip")).toBe(true);
  });
});

describe("RbTooltip trigger mode (regression)", () => {
  it("the label opens after the delay on hover and closes on leave", async () => {
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);
    act(() => {
      root.render(
        createElement(
          RbTooltip,
          { label: "Add panel surface" },
          createElement(RbTooltipTrigger, {
            delay: 350,
            render: createElement("button", { type: "button", className: "strip-add" }, "+"),
          }),
        ),
      );
    });
    unmounts.push(() => {
      act(() => {
        root.unmount();
      });
      container.remove();
    });

    const trigger = container.querySelector<HTMLElement>(".strip-add")!;
    // Base UI's tooltip trigger is mouse-only + rest-delay: the native
    // `mouseenter` (bound straight on the trigger by the hover hook) resets
    // the move gate, and the OPEN arms from the React `onMouseMove` prop's
    // rest timer — the jsdom recipe is a bubbling mouseenter (dispatched at
    // the element, so the hook's own listener fires too) plus a bubbling
    // mousemove (React's delegated root needs the bubble), then the rest
    // window. The close rides the hook's native non-bubbling `mouseleave`.
    const enter = (): void => {
      trigger.dispatchEvent(new MouseEvent("mouseenter", { bubbles: true }));
      trigger.dispatchEvent(new MouseEvent("mousemove", { bubbles: true }));
    };
    const leave = (): void => {
      trigger.dispatchEvent(new MouseEvent("mouseleave"));
    };
    // Under the 350ms rest delay nothing shows (the pointer rests for less
    // than the delay — the arm fires nothing).
    // Under the 350ms delay nothing shows (a quick pass arms nothing).
    await act(async () => {
      enter();
    });
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 200));
    });
    expect(document.querySelector(".rb-tooltip-popup")).toBeNull();

    // At the delay the label shows, portaled to the body.
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 250));
    });
    const popup = document.querySelector<HTMLElement>(".rb-tooltip-popup");
    expect(popup).not.toBeNull();
    expect(popup!.textContent).toBe("Add panel surface");
    expect(container.contains(popup!)).toBe(false);

    // Leave dismisses the label (the family's 0ms close delay).
    await act(async () => {
      leave();
    });
    expect(document.querySelector(".rb-tooltip-popup")).toBeNull();
  });

  it("ui/Tooltip forwards the virtual-anchor trio — the app layer's exposure", () => {
    const ref: { current: HTMLDivElement | null } = { current: null };
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);
    act(() => {
      root.render(
        createElement(Tooltip, {
          label: "src/lib.rs",
          open: true,
          anchor: virtualAnchorAt(10, 10),
          popupClassName: "mention-tooltip",
          popupRef: ref,
        }),
      );
    });
    unmounts.push(() => {
      act(() => {
        root.unmount();
      });
      container.remove();
    });
    const popup = document.querySelector<HTMLElement>(".rb-tooltip-popup.mention-tooltip");
    expect(popup).not.toBeNull();
    // No trigger part renders in virtual-anchor mode — the label rides the
    // controlled open alone.
    expect(container.querySelector("button")).toBeNull();
    expect(ref.current).toBe(popup);
    expect(popup!.textContent).toBe("src/lib.rs");
  });
});
