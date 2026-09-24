import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { RightPaneStore } from "../src/state/right-pane";
import { PANEL_TOGGLE_SLOTS } from "../src/state/layout";

/**
 * Ticket 10 — the Files toggle on the web titlebar, mirroring the desktop's
 * `toggle-files-panel` (tabs.rs:366-384 over files_panel.rs:234-245): the
 * custom FILE_TREE icon immediately LEFT of the pane toggle, aria-label
 * "Hide/Show files panel", and `bg(wash(0.09))` while the docked explorer
 * portion is open. The suite runs in vitest's node environment with no DOM
 * and no mount harness (nothing in the repo renders components in tests),
 * so the contract is pinned in the three layers the ticket ships: the store
 * semantics the button drives (toggle opens/docks the pane alone, the
 * active input `filesOpen` flips with it), the titlebar/app-shell source
 * wiring, and the stylesheet's active wash — the same layers
 * tests/identity-freeze.test.ts pins with `readFileSync`.
 */

const css = readFileSync(new URL("../src/styles/app.css", import.meta.url), "utf8");
const titlebar = readFileSync(
  new URL("../src/components/titlebar.tsx", import.meta.url),
  "utf8",
);
const appShell = readFileSync(
  new URL("../src/components/app-shell.tsx", import.meta.url),
  "utf8",
);

describe("the store half — the toggle the button drives", () => {
  it("the toggle opens/docks the explorer portion alone, then closes just it", () => {
    // `toggle_files_panel` (files_panel.rs:234-245): opening docks the
    // explorer portion without opening the surface host; a second press
    // undocks exactly that portion.
    const store = new RightPaneStore(null);
    store.toggleFilesPanel("chat-1");
    expect(store.stateFor("chat-1")).toMatchObject({ open: false, filesOpen: true });
    store.toggleFilesPanel("chat-1");
    expect(store.stateFor("chat-1")).toMatchObject({ open: false, filesOpen: false });
  });

  it("the active input tracks filesOpen across the pane toggle, which never touches it", () => {
    // The button's active state reads exactly `pane.filesOpen`; the PANE
    // toggle drives only the surface host (fe45a1cd), so the Files button
    // keeps its wash through a pane open/close round trip.
    const store = new RightPaneStore(null);
    store.toggleFilesPanel("chat-1");
    const active = (): boolean => store.stateFor("chat-1").filesOpen;
    expect(active()).toBe(true);
    store.toggle("chat-1");
    expect(active()).toBe(true);
    store.toggle("chat-1");
    expect(active()).toBe(true);
    store.close("chat-1");
    expect(active()).toBe(true);
    store.toggleFilesPanel("chat-1");
    expect(active()).toBe(false);
  });
});

describe("the titlebar half — the button itself (tabs.rs:366-384)", () => {
  it("renders the fileTree icon LEFT of the pane toggle, gated by its own handler", () => {
    // The two fixed right-edge anchors in order: Files first, then the
    // pane toggle — the desktop's trailing strip order.
    const files = titlebar.indexOf("icon=\"fileTree\"");
    const pane = titlebar.indexOf("icon=\"sidebarMinimalistic\"");
    expect(files).toBeGreaterThan(-1);
    expect(pane).toBeGreaterThan(files);
    // The button mounts only where the shell hands it a handler (the
    // desktop hides the whole trailing group on the canvas).
    expect(titlebar).toMatch(/\{onToggleFiles != null && \(\s*<HeaderIconButton/);
  });

  it("the aria-label flips with filesOpen — the desktop's Hide/Show pair", () => {
    expect(titlebar).toMatch(
      /label=\{filesOpen \? "Hide files panel" : "Show files panel"\}/,
    );
  });

  it("the active wash is driven by filesOpen through the data-active attribute", () => {
    expect(titlebar).toMatch(/onClick=\{onToggleFiles\}\s*active=\{filesOpen\}/);
    expect(titlebar).toMatch(/data-active=\{active \? "1" : undefined\}/);
  });
});

describe("the wiring — app-shell hands the button the store (the toggle's only caller)", () => {
  it("wires onToggleFiles to rightPaneStore.toggleFilesPanel(paneChatId)", () => {
    expect(appShell).toMatch(
      /onToggleFiles=\{hasPane \? \(\) => rightPaneStore\.toggleFilesPanel\(paneChatId\) : null\}/,
    );
  });

  it("passes the live filesOpen flag, gated on pane ownership like the pane controls", () => {
    expect(appShell).toMatch(/filesOpen=\{hasPane && pane\.filesOpen\}/);
  });
});

describe("the stylesheet half — the active wash and the band budget", () => {
  it("the active header button paints wash(0.09), after the hover rule so it holds on hover", () => {
    const active = css.match(/\.header-icon-button\[data-active="1"\]\s*\{[^}]*\}/)?.[0];
    expect(active, ".header-icon-button[data-active=\"1\"] rule must exist").toBeDefined();
    expect(active).toMatch(/background:\s*rgb\(var\(--rb-wash\) \/ 0\.09\);/);
    // Only the background changes — the glyph stays muted like the desktop.
    expect(active).not.toMatch(/color:/);
    // The desktop's `.when(files_panel_open, …)` applies AFTER the hover
    // blend, so the active wash wins on hover too — the rule must sit
    // after the hover rule for the same effect.
    expect(css.indexOf(".header-icon-button[data-active")).toBeGreaterThan(
      css.indexOf(".header-icon-button:hover"),
    );
  });

  it("the toggle-slots token is the desktop's PANEL_TOGGLE_SLOTS pair (56px)", () => {
    const token = css.match(/--rb-titlebar-toggle-slots:\s*(\d+(?:\.\d+)?)px;/)?.[1];
    expect(token, "--rb-titlebar-toggle-slots must resolve to px").toBeDefined();
    expect(Number(token)).toBe(PANEL_TOGGLE_SLOTS);
    expect(PANEL_TOGGLE_SLOTS).toBe(56);
  });

  it("the pane band's inner width budgets the pair, not one slot", () => {
    const inner = css.match(/\.titlebar-pane-band-inner\s*\{[^}]*\}/)?.[0];
    expect(inner).toBeDefined();
    expect(inner).toMatch(
      /calc\(var\(--rb-pane-open\) - var\(--rb-titlebar-edge-inset\) - var\(--rb-titlebar-toggle-slots\)\)/,
    );
    expect(inner).not.toMatch(/- 28px\)/);
  });
});
