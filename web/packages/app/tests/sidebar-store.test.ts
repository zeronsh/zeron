import { describe, expect, it } from "vitest";
import type { StorageLike } from "../src/lib/engine-store";
import { SidebarStore } from "../src/lib/sidebar-store";
import { UiSettingsStore } from "../src/state/ui-settings";

function memoryStorage(): StorageLike {
  const map = new Map<string, string>();
  return {
    getItem: (key) => (map.has(key) ? map.get(key)! : null),
    setItem: (key, value) => void map.set(key, value),
    removeItem: (key) => void map.delete(key),
  };
}

describe("SidebarStore", () => {
  it("starts unfiltered with the archived shelf closed and pins open", () => {
    const store = new SidebarStore({ storage: memoryStorage() });
    expect(store.getSnapshot()).toEqual({
      spaceFilter: null,
      lastSpaceId: null,
      archivedOpen: false,
      // `Shell::pinned_open`: pins are visible by default (a hidden pin
      // would be pointless), session-transient like the archived shelf.
      pinnedOpen: true,
      pinnedByProfile: {},
      // Custom sections (upstream 86249cf0): device-local, profile-isolated,
      // never synchronized; the create-section dialog is in-memory.
      sectionsByProfile: {},
      sectionDialogOpen: false,
      // The view options ride along at their desktop defaults
      // (settings.rs) — ticket 10's menu writes them. Compact mode defaults
      // ON (upstream ffaa3102).
      organization: "inOneList",
      sort: "lastUpdated",
      compact: true,
      showProjectIcon: true,
      showProjectLabel: true,
      showHarness: true,
      showBranch: true,
      showPullRequest: true,
    });
  });

  it("projects the sidebar display toggles and re-projects on writes (upstream 78e9e6ae)", () => {
    const storage = memoryStorage();
    // The view menu writes through uiSettings directly; the sidebar store
    // re-projects its slice off the same settings instance.
    const settings = new UiSettingsStore({ storage });
    const store = new SidebarStore({ settings });
    settings.updateImmediate({
      sidebarCompact: false,
      sidebarShowProjectLabel: false,
      sidebarOrganization: "byProject",
    });
    expect(store.getSnapshot().compact).toBe(false);
    expect(store.getSnapshot().showProjectLabel).toBe(false);
    expect(store.getSnapshot().organization).toBe("byProject");
    // And the projection survives a reload over the same storage.
    const second = new SidebarStore({ storage });
    expect(second.getSnapshot().compact).toBe(false);
    expect(second.getSnapshot().showProjectLabel).toBe(false);
    expect(second.getSnapshot().organization).toBe("byProject");
  });

  it("persists the filter and the last selected space across reloads", () => {
    const storage = memoryStorage();
    const first = new SidebarStore({ storage });
    first.setSpaceFilter("space-1");
    first.setSpaceFilter("space-2");
    first.setSpaceFilter(null);
    const second = new SidebarStore({ storage });
    // "All projects" is the filter; the last picked space survives as the
    // new-chat fallback, exactly like the desktop's last_space_id.
    expect(second.getSnapshot().spaceFilter).toBe(null);
    expect(second.getSnapshot().lastSpaceId).toBe("space-2");
  });

  it("keeps the archived disclosure in memory only", () => {
    const storage = memoryStorage();
    const first = new SidebarStore({ storage });
    first.setArchivedOpen(true);
    expect(first.getSnapshot().archivedOpen).toBe(true);
    expect(new SidebarStore({ storage }).getSnapshot().archivedOpen).toBe(false);
  });

  it("keeps the pinned disclosure in memory only, open by default", () => {
    const storage = memoryStorage();
    const first = new SidebarStore({ storage });
    first.setChatPinned("local", "a", true);
    first.setPinnedOpen(false);
    expect(first.getSnapshot().pinnedOpen).toBe(false);
    // `Shell::pinned_open` never reaches storage: a fresh store over the
    // same storage re-expands (and the pins themselves survive).
    const second = new SidebarStore({ storage });
    expect(second.getSnapshot().pinnedOpen).toBe(true);
    expect(second.getSnapshot().pinnedByProfile).toEqual({ local: ["a"] });
  });

  it("ignores corrupted legacy state without destroying it", () => {
    // Storage moved to the consolidated ui-settings key; the legacy key is a
    // one-time migration source now, so a corrupt one heals to the default and
    // is left exactly where it is for a rollback to find.
    const storage = memoryStorage();
    storage.setItem("zeron.sidebar.v1", "{not json");
    expect(new SidebarStore({ storage }).getSnapshot().spaceFilter).toBe(null);
    expect(storage.getItem("zeron.sidebar.v1")).toBe("{not json");
  });

  it("notifies subscribers on actual changes only", () => {
    const store = new SidebarStore({ storage: memoryStorage() });
    let fired = 0;
    store.subscribe(() => {
      fired += 1;
    });
    store.setSpaceFilter("space-1");
    store.setSpaceFilter("space-1");
    store.setArchivedOpen(true);
    store.setArchivedOpen(true);
    store.setPinnedOpen(false);
    store.setPinnedOpen(false);
    expect(fired).toBe(3);
  });

  it("set_chat_pinned: pins append in click order under their profile, unpins leave the rest", () => {
    const store = new SidebarStore({ storage: memoryStorage() });
    store.setChatPinned("local", "a", true);
    store.setChatPinned("local", "b", true);
    store.setChatPinned("synced:device-1", "s1", true);
    store.setChatPinned("local", "c", true);
    expect(store.getSnapshot().pinnedByProfile).toEqual({
      local: ["a", "b", "c"],
      "synced:device-1": ["s1"],
    });
    store.setChatPinned("local", "b", false);
    expect(store.getSnapshot().pinnedByProfile).toEqual({
      local: ["a", "c"],
      "synced:device-1": ["s1"],
    });
    // The last unpin drops the bucket from the map.
    store.setChatPinned("synced:device-1", "s1", false);
    expect(store.getSnapshot().pinnedByProfile).toEqual({ local: ["a", "c"] });
  });

  it("pin no-ops write nothing and notify nobody — including a null profile key", () => {
    const store = new SidebarStore({ storage: memoryStorage() });
    store.setChatPinned("local", "a", true);
    let fired = 0;
    store.subscribe(() => {
      fired += 1;
    });
    store.setChatPinned("local", "a", true);
    store.setChatPinned("local", "ghost", false);
    // "Identity not ready": the desktop's early return.
    store.setChatPinned(null, "b", true);
    store.replacePinsByProfile({ local: ["a"] });
    expect(fired).toBe(0);
    expect(store.getSnapshot().pinnedByProfile).toEqual({ local: ["a"] });
  });

  it("local_synced_local_switch_restores_each_profiles_pins", () => {
    const storage = memoryStorage();
    const first = new SidebarStore({ storage });
    first.setChatPinned("local", "a", true);
    first.setChatPinned("local", "b", true);
    first.replacePinsByProfile({ local: ["b", "a"], "synced:device-1": ["s1"] });
    const second = new SidebarStore({ storage });
    expect(second.getSnapshot().pinnedByProfile).toEqual({ local: ["b", "a"], "synced:device-1": ["s1"] });
  });

  it("pin_cleanup_for_one_profile_leaves_other_profiles_untouched", () => {
    const store = new SidebarStore({ storage: memoryStorage() });
    store.setChatPinned("local", "active", true);
    store.setChatPinned("local", "archived", true);
    store.setChatPinned("local", "deleted", true);
    store.setChatPinned("synced:device-1", "synced-pin", true);
    let fired = 0;
    store.subscribe(() => {
      fired += 1;
    });
    // Only the active profiles are judged; "synced:device-1" is not among
    // them, so its pin is nobody's deletion.
    store.pruneUnknownPins(["local"], new Set(["active", "archived"]));
    expect(store.getSnapshot().pinnedByProfile).toEqual({
      local: ["active", "archived"],
      "synced:device-1": ["synced-pin"],
    });
    expect(fired).toBe(1);
    store.pruneUnknownPins(["local"], new Set(["active", "archived"]));
    expect(fired).toBe(1);
    // A bucket pruned to empty drops out of the map.
    store.pruneUnknownPins(["synced:device-1"], new Set(["active"]));
    expect(store.getSnapshot().pinnedByProfile).toEqual({ local: ["active", "archived"] });
  });
});
