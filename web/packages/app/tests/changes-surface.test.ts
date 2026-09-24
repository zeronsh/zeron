import { describe, expect, it } from "vitest";
import { createElement } from "react";
import { renderToString } from "react-dom/server";
import { ChangesSurface, ChangesToolbar, ChangesScopeSelector, ChangesScopeMenuRows } from "../src/routes/changes-page";
import { changesSurfaceStore, ChangesSurfaceStore } from "../src/state/changes-surface";

/**
 * A render-path smoke for the Changes surface: the smoke ENGINE fixture has
 * no git checkout, so a Diffs tab cannot be minted in the browser harness
 * (the picker's git gate). Server-rendering the two trees exercises the
 * mount path — imports, hook wiring, the store binding — beyond what the
 * pure-logic suite covers. The node suite cannot dispatch DOM events, so
 * the selector's pick is driven at the store the rows call and re-rendered
 * through the toolbar that reads it.
 */
describe("Changes surface render smoke", () => {
  it("renders the toolbar with the one scope selector trigger and the trailing tools", () => {
    const html = renderToString(createElement(ChangesToolbar, { chatId: "c1", surfaceId: "d1" }));
    // ONE trigger where the three chips sat (ticket 62): the active scope's
    // label + the chevron, not the other scopes' labels.
    expect(html).toContain('id="changes-scope-trigger"');
    expect(html).toContain("Working tree");
    expect(html).not.toContain("Branch changes");
    expect(html).not.toContain("Latest turn");
    expect(html).toContain('id="changes-split"');
    expect(html).toContain('id="changes-wrap"');
    expect(html).toContain('id="changes-fold-all"');
  });

  it("opens the selector's popover with the three desktop rows, the active one marked", () => {
    // Base UI's portal renders nothing under `renderToString`, so the open
    // state is asserted on the trigger (aria-expanded) and the rows through
    // the composed menu unit the card carries.
    const selector = renderToString(
      createElement(ChangesScopeSelector, {
        chatId: "c2",
        surfaceId: "d2",
        scope: "workingTree",
        open: true,
        onOpenChange: () => {},
      }),
    );
    expect(selector).toContain('id="changes-scope-trigger"');
    expect(selector).toContain('aria-expanded="true"');
    expect(selector).not.toContain("Branch changes");
    const rows = renderToString(
      createElement(ChangesScopeMenuRows, { chatId: "c2", surfaceId: "d2", scope: "workingTree", onPick: () => {} }),
    );
    expect(rows).toContain("Working tree");
    expect(rows).toContain("Branch changes");
    expect(rows).toContain("Latest turn");
    // One selected row (the wash + aria-selected) carrying the check mark.
    expect(rows.match(/menu-row-selected/g)?.length).toBe(1);
    expect(rows).toContain('aria-selected="true"');
    expect(rows).toContain("changes-scope-check");
  });

  it("a row pick switches the scope through the unchanged setScope store and the trigger follows", () => {
    // The rows' onClick is setScope over DIFF_SCOPE_CHIPS plus the close
    // callback; driving that store write and re-rendering the toolbar (which
    // reads the store through useChangesSurface) covers the pick's
    // observable outcome — the node suite cannot dispatch DOM events.
    changesSurfaceStore.setScope("c3", "d3", "branch");
    expect(changesSurfaceStore.snapshotFor("c3", "d3").scope).toBe("branch");
    const closed = renderToString(createElement(ChangesToolbar, { chatId: "c3", surfaceId: "d3" }));
    expect(closed).toContain("Branch changes");
    expect(closed).not.toContain("Working tree");
    const rows = renderToString(
      createElement(ChangesScopeMenuRows, { chatId: "c3", surfaceId: "d3", scope: "branch", onPick: () => {} }),
    );
    expect(rows).toContain('aria-selected="true"');
    expect(rows).toContain("changes-scope-check");
    changesSurfaceStore.dispose("c3", "d3");
  });

  it("renders the surface's no-engine gate without a session", () => {
    const html = renderToString(createElement(ChangesSurface, { chatId: "c1", surfaceId: "d1" }));
    expect(html).toContain("Pair an engine to view its changes.");
  });
});

/**
 * A commit-pinned tab (`Changes::for_commit`, ticket 27's click target): the
 * scope lands on `commit` with the pinned sha and never moves off it — there
 * is no scope row to take it back.
 */
describe("commit-pinned Changes surface state", () => {
  it("pins the commit scope and ignores later scope switches", () => {
    const store = new ChangesSurfaceStore();
    store.pinCommit("c1", "d27", "896e31f0abcd");
    const pinned = store.snapshotFor("c1", "d27");
    expect(pinned.scope).toBe("commit");
    expect(pinned.commitSha).toBe("896e31f0abcd");

    store.setScope("c1", "d27", "branch");
    store.setScope("c1", "d27", "workingTree");
    const after = store.snapshotFor("c1", "d27");
    expect(after.scope).toBe("commit");
    expect(after.commitSha).toBe("896e31f0abcd");

    store.dispose("c1", "d27");
    expect(store.snapshotFor("c1", "d27").scope).toBe("workingTree");
  });
});

