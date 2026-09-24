// @vitest-environment jsdom

/**
 * The shared stacked notice chip (lib/notice-chip.ts +
 * components/notice-chip.tsx) — the web peer of the desktop's
 * `notice.rs::notice_chip`: header row (icon + label + copy button pinned
 * top-right) over the wrapping message; the composer dismisses on click and
 * copying must not take the notice with it. No JSX (createElement),
 * per-file jsdom pragma only.
 */

import { act, createElement } from "react";
import { createRoot } from "react-dom/client";
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";

const actEnvironment = globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean };
beforeAll(() => {
  actEnvironment.IS_REACT_ACT_ENVIRONMENT = true;
});
afterAll(() => {
  delete actEnvironment.IS_REACT_ACT_ENVIRONMENT;
});

import {
  OFFLINE_NOTICE,
  noticeChipModel,
  noticeLabelForTone,
  noticeToneForMessage,
} from "../src/lib/notice-chip";
import { NoticeChip } from "../src/components/notice-chip";

describe("notice chip model", () => {
  it("reads the offline-ish composer failure as a warning", () => {
    expect(noticeToneForMessage(OFFLINE_NOTICE)).toBe("warning");
    expect(noticeToneForMessage("Couldn't send that message")).toBe("danger");
    expect(noticeLabelForTone("warning")).toBe("Warning");
    expect(noticeLabelForTone("danger")).toBe("Error");
  });

  it("picks the header-icon metrics from the variant", () => {
    const plain = noticeChipModel({
      tone: "danger",
      variant: "plain",
      label: "Error",
      message: "x",
    });
    expect(plain.iconSize).toBe(14);
    const tile = noticeChipModel({ tone: "danger", variant: "tile", label: "Error", message: "x" });
    expect(tile.iconSize).toBe(12);
  });
});

describe("NoticeChip", () => {
  let host: HTMLDivElement | null = null;
  let root: ReturnType<typeof createRoot> | null = null;

  afterEach(() => {
    if (root !== null) {
      act(() => root!.unmount());
      root = null;
    }
    host?.remove();
    host = null;
    vi.restoreAllMocks();
  });

  function mount(element: React.ReactElement): HTMLDivElement {
    host = document.createElement("div");
    document.body.appendChild(host);
    root = createRoot(host);
    act(() => root!.render(element));
    return host;
  }

  it("stacks the header row over the wrapping message", () => {
    const el = mount(
      createElement(NoticeChip, {
        tone: "danger",
        variant: "tile",
        label: "Error",
        message: "the agent exited with status 1\nand a long stderr tail",
      }),
    );
    const chip = el.querySelector(".notice-chip");
    expect(chip).not.toBeNull();
    expect(chip!.classList.contains("notice-chip-tile")).toBe(true);
    expect(el.querySelector(".notice-chip-label")!.textContent).toBe("Error");
    expect(el.querySelector(".notice-chip-text")!.textContent).toContain("status 1");
    // The tile variant wraps its triangle in the washed tile.
    expect(el.querySelector(".notice-chip-tile-icon")).not.toBeNull();
    expect(el.querySelector(".notice-chip-copy")).not.toBeNull();
  });

  it("marks the warning tone and the composer's dismiss wiring", () => {
    const dismiss = vi.fn();
    const el = mount(
      createElement(NoticeChip, {
        tone: noticeToneForMessage(OFFLINE_NOTICE),
        variant: "plain",
        label: noticeLabelForTone(noticeToneForMessage(OFFLINE_NOTICE)),
        message: OFFLINE_NOTICE,
        id: "composer-failure",
        onClick: dismiss,
      }),
    );
    const chip = el.querySelector("#composer-failure")!;
    expect(chip.classList.contains("notice-chip-warning")).toBe(true);
    expect(chip.classList.contains("notice-chip-dismissible")).toBe(true);
    expect(el.querySelector(".notice-chip-label")!.textContent).toBe("Warning");
    act(() => {
      chip.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    expect(dismiss).toHaveBeenCalledTimes(1);
  });

  it("copies the message without dismissing the chip", () => {
    const dismiss = vi.fn();
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText },
      configurable: true,
    });
    const el = mount(
      createElement(NoticeChip, {
        tone: "danger",
        variant: "plain",
        label: "Error",
        message: "spawn failed: ENOENT",
        onClick: dismiss,
      }),
    );
    const copy = el.querySelector(".notice-chip-copy")!;
    act(() => {
      copy.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    expect(writeText).toHaveBeenCalledWith("spawn failed: ENOENT");
    expect(dismiss).not.toHaveBeenCalled();
  });
});
