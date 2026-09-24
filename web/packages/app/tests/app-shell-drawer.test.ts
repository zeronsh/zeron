import { describe, expect, it } from "vitest";
import { shouldCloseDrawer } from "../src/components/app-shell";

/**
 * Ticket 55 — the phone drawer's close-on-navigate rule (research M7(b)).
 * The suite runs in vitest's node environment with no DOM and no mount
 * harness (nothing in the repo renders components in tests), so the
 * research's app-shell-level "sidebarOpen flips false under a mocked 375px
 * matchMedia" case is landed as the ticket §3 pure-helper extraction: the
 * rule `AppShell`'s effect consults, with `isPhone` standing in for the
 * mocked `(max-width: 768px)` matchMedia (the toggle's own breakpoint —
 * the width read stays inside the effect body, at navigation time). The
 * wiring itself — deps `[pathname]`, the matchMedia string, the
 * `setSidebarOpen(false)` write — is typechecked by `tsc --noEmit` in
 * `pnpm -r build`.
 *
 * Desktop widths never close: the sidebar is a persistent column there
 * ("there is nothing to port", M7(b)) — the desktop-width cases below
 * assert that branch of THIS rule; no desktop test names map to it.
 */
describe("shouldCloseDrawer (M7(b) close-on-navigate)", () => {
  it("drawer closes on pathname change at phone widths", () => {
    // A 375px viewport: the drawer is open and the navigation changed the
    // pathname — the one condition the effect keys on.
    expect(shouldCloseDrawer(true, true, true)).toBe(true);
    // The rule is conjunctive: drop any input and the drawer stays open.
    expect(shouldCloseDrawer(true, true, false)).toBe(false);
    expect(shouldCloseDrawer(true, false, true)).toBe(false);
    expect(shouldCloseDrawer(false, true, true)).toBe(false);
  });

  it("drawer stays open on pathname change at desktop widths", () => {
    // ≥769px: matchMedia false — the sidebar column persists and selecting
    // a chat or settings page never closes anything (desktop untouched).
    expect(shouldCloseDrawer(true, true, false)).toBe(false);
    expect(shouldCloseDrawer(false, true, false)).toBe(false);
    expect(shouldCloseDrawer(true, false, false)).toBe(false);
    expect(shouldCloseDrawer(false, false, false)).toBe(false);
  });

  it("no close when the drawer is already closed", () => {
    // The effect's early return: nothing to close, whatever the width.
    expect(shouldCloseDrawer(true, false, true)).toBe(false);
    expect(shouldCloseDrawer(true, false, false)).toBe(false);
    expect(shouldCloseDrawer(false, false, true)).toBe(false);
  });

  it("same-path navigation does not refire the effect", () => {
    // The recorded known limitation, asserted as such: re-notifying the
    // same pathname (tapping the row of the already-active chat or
    // section) leaves the drawer open — the effect's `[pathname]` deps
    // compare equal, and the previous-pathname ref keeps even a refire
    // from reading as a navigation, so the rule never sees a pathname
    // change. The backdrop, the cluster toggle, and Escape still close it.
    expect(shouldCloseDrawer(false, true, true)).toBe(false);
    expect(shouldCloseDrawer(false, true, false)).toBe(false);
    expect(shouldCloseDrawer(false, false, true)).toBe(false);
  });
});
