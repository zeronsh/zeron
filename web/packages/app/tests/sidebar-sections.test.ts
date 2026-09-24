import { describe, expect, it } from "vitest";
import type { StorageLike } from "../src/lib/engine-store";
import { SidebarStore } from "../src/lib/sidebar-store";
import { healSidebarSectionsByProfile, type SidebarSection } from "../src/state/ui-settings";
import {
  activeSidebarSections,
  assignSidebarSection,
  createSidebarSection,
  deleteSidebarSection,
  renameSidebarSection,
  sectionMembership,
  sectionRows,
  setSidebarSectionCollapsed,
  SIDEBAR_SECTION_NAME_MAX,
  validSectionName,
} from "../src/lib/sidebar-sections";
import { customSectionKeyedHeight } from "../src/components/sidebar-sections";

function memoryStorage(): StorageLike {
  const map = new Map<string, string>();
  return {
    getItem: (key) => (map.has(key) ? map.get(key)! : null),
    setItem: (key, value) => void map.set(key, value),
    removeItem: (key) => void map.delete(key),
  };
}

/*
 * Custom sidebar sections — the web peer of the desktop's
 * `shell/sidebar_sections.rs` tests (upstream 86249cf0, ported local-only:
 * NO sync). Names follow the Rust tests where the seam matches; the
 * archive-all path lives in the component (per-session engine Mutates) and
 * is not unit-testable here — the Rust suite covers it.
 */

function section(id: string, name: string, sessionIds: readonly string[] = [], collapsed = false): SidebarSection {
  return { id, name, sessionIds, collapsed };
}

describe("sidebar-sections (pure rules)", () => {
  it("valid_section_name_trims_rejects_empty_and_over_120_chars", () => {
    expect(validSectionName("  Work in progress  ")).toBe("Work in progress");
    expect(validSectionName("   ")).toBe(null);
    expect(validSectionName("")).toBe(null);
    expect(validSectionName("x".repeat(SIDEBAR_SECTION_NAME_MAX))).toBe("x".repeat(SIDEBAR_SECTION_NAME_MAX));
    expect(validSectionName("x".repeat(SIDEBAR_SECTION_NAME_MAX + 1))).toBe(null);
  });

  it("create_appends_an_empty_open_section", () => {
    const sections = createSidebarSection([section("a", "A", ["c1"])], "b", "B");
    expect(sections).toEqual([section("a", "A", ["c1"]), section("b", "B")]);
  });

  it("rename_and_delete_target_their_id_and_delete_keeps_the_chats", () => {
    const sections = [section("a", "A", ["c1"]), section("b", "B", ["c2"])];
    expect(renameSidebarSection(sections, "a", "Renamed")).toEqual([
      section("a", "Renamed", ["c1"]),
      section("b", "B", ["c2"]),
    ]);
    // Deleting the section never deletes sessions — membership dies with it.
    expect(deleteSidebarSection(sections, "a")).toEqual([section("b", "B", ["c2"])]);
  });

  it("assign_is_exclusive_and_un_collapses_the_target_and_refuses_a_vanished_target", () => {
    const sections = [section("a", "A", ["c1"], true), section("b", "B", ["c2"])];
    // Moving c1 to b: it leaves a, joins b, and b un-collapses to reveal it.
    expect(assignSidebarSection(sections, "c1", "b")).toEqual([
      section("a", "A", [], true),
      section("b", "B", ["c2", "c1"], false),
    ]);
    // Target null clears membership everywhere.
    expect(assignSidebarSection(sections, "c2", null)).toEqual([
      section("a", "A", ["c1"], true),
      section("b", "B", []),
    ]);
    // A target that no longer exists is refused whole.
    expect(assignSidebarSection(sections, "c1", "gone")).toEqual(sections);
  });

  it("set_collapsed_flips_only_its_section", () => {
    const sections = [section("a", "A"), section("b", "B")];
    expect(setSidebarSectionCollapsed(sections, "b", true)).toEqual([
      section("a", "A"),
      section("b", "B", [], true),
    ]);
  });

  it("active_sections_read_the_profile_bucket_and_null_means_none", () => {
    const byProfile = { local: [section("a", "A")], "synced:d1": [section("z", "Z")] };
    expect(activeSidebarSections(byProfile, "local")).toEqual([section("a", "A")]);
    expect(activeSidebarSections(byProfile, "unknown")).toEqual([]);
    expect(activeSidebarSections(byProfile, null)).toEqual([]);
  });

  it("section_membership_finds_the_claiming_section_first_wins", () => {
    const sections = [section("a", "A", ["c1"]), section("b", "B", ["c2", "c1"])];
    expect(sectionMembership(sections, "c2")).toBe("b");
    expect(sectionMembership(sections, "c9")).toBe(null);
  });

  it("section_rows_split_claimed_from_remaining_without_duplicates", () => {
    const sections = [section("a", "A", ["c1", "gone"]), section("b", "B", ["c3", "c1"])];
    const rows = chatRows(["c1", "c2", "c3"]);
    const { groups, remaining } = sectionRows(sections, rows);
    // c1 lands in the FIRST claiming section; c3 in b; c2 stays regular; a
    // vanished member holds no row; a collapsed section still claims.
    expect(groups.map((group) => group.section.id)).toEqual(["a", "b"]);
    expect(groups[0]!.rows.map((row) => row.chat.id)).toEqual(["c1"]);
    expect(groups[1]!.rows.map((row) => row.chat.id)).toEqual(["c3"]);
    expect(remaining.map((row) => row.chat.id)).toEqual(["c2"]);
  });

  it("custom_section_keyed_height_is_the_band_header_and_open_body", () => {
    const collapsed = customSectionKeyedHeight(section("a", "A", [], true), [], true, true);
    expect(collapsed).toBe(12 + 28);
    // An empty open body is the inset + the 40px "Drop sessions here".
    const empty = customSectionKeyedHeight(section("a", "A"), [], false, false);
    expect(empty).toBe(12 + 28 + 4 + 40);
    // One compact row (29px) + inset, no gaps.
    const one = customSectionKeyedHeight(section("a", "A"), chatRows(["c1"]), true, true);
    expect(one).toBe(12 + 28 + 4 + 29);
  });
});

describe("SidebarStore sections", () => {
  it("sections_dialog_persistence_deletion_and_profile_isolation", () => {
    const storage = memoryStorage();
    const store = new SidebarStore({ storage });
    // The dialog's open state is in-memory like the archived shelf.
    store.openSectionDialog();
    expect(store.getSnapshot().sectionDialogOpen).toBe(true);
    store.closeSectionDialog();
    expect(store.getSnapshot().sectionDialogOpen).toBe(false);
    expect(new SidebarStore({ storage }).getSnapshot().sectionDialogOpen).toBe(false);

    // Create: trimmed names, invalid ones refused, ids unique.
    const a = store.createSection("local", "  Research  ");
    expect(a).not.toBe(null);
    expect(store.createSection("local", "   ")).toBe(null);
    expect(store.createSection("local", "x".repeat(SIDEBAR_SECTION_NAME_MAX + 1))).toBe(null);
    expect(store.createSection(null, "Nowhere")).toBe(null);
    const b = store.createSection("local", "WIP");
    expect(a).not.toBe(b);
    expect(store.getSnapshot().sectionsByProfile.local!.map((s) => [s.name, s.sessionIds, s.collapsed])).toEqual([
      ["Research", [], false],
      ["WIP", [], false],
    ]);

    // Rename: a missing id is a no-op; invalid names refused.
    expect(store.renameSection("local", a!, "Deep Work")).toBe(true);
    expect(store.renameSection("local", "gone", "X")).toBe(false);
    expect(store.renameSection("local", a!, "  ")).toBe(false);
    expect(store.getSnapshot().sectionsByProfile.local![0]!.name).toBe("Deep Work");

    // Delete: the section goes, its sessions were never stored here anyway.
    store.deleteSection("local", b!);
    expect(store.getSnapshot().sectionsByProfile.local).toHaveLength(1);

    // Profile isolation: another profile's sections never leak.
    store.createSection("synced:d1", "Remote");
    expect(store.getSnapshot().sectionsByProfile["synced:d1"]![0]!.name).toBe("Remote");
    store.deleteSection("local", a!);
    expect(store.getSnapshot().sectionsByProfile.local).toBeUndefined();
    expect(store.getSnapshot().sectionsByProfile["synced:d1"]).toHaveLength(1);

    // Persistence: a fresh store over the same storage reloads the map.
    const reloaded = new SidebarStore({ storage }).getSnapshot().sectionsByProfile;
    expect(reloaded["synced:d1"]).toHaveLength(1);
    expect(reloaded["synced:d1"]![0]!.name).toBe("Remote");
    expect(reloaded["synced:d1"]![0]!.sessionIds).toEqual([]);
  });

  it("sections_transfer_between_pins_sections_and_regular_without_duplicates", () => {
    const store = new SidebarStore({ storage: memoryStorage() });
    const s1 = store.createSection("local", "A")!;
    const s2 = store.createSection("local", "B")!;

    // Pin a chat: it pins AND leaves any section at the same write.
    store.setChatPinned("local", "p1", true);
    store.assignSidebarSection("local", "p1", s1);
    store.setChatPinned("local", "p2", true);
    expect(store.getSnapshot().sectionsByProfile.local![0]!.sessionIds).toEqual(["p1"]);
    store.setChatPinned("local", "p2", true);
    expect(store.getSnapshot().pinnedByProfile.local).toEqual(["p1", "p2"]);

    // Pinning a claimed chat clears membership (exclusive in display).
    store.setChatPinned("local", "p1", true);
    expect(store.getSnapshot().sectionsByProfile.local![0]!.sessionIds).toEqual([]);
    expect(store.getSnapshot().pinnedByProfile.local).toEqual(["p1", "p2"]);

    // Assign moves membership exclusively between sections, no duplicates.
    store.assignSidebarSection("local", "p1", s2);
    expect(store.getSnapshot().sectionsByProfile.local!.map((s) => s.sessionIds)).toEqual([[], ["p1"]]);
    store.assignSidebarSection("local", "p1", s2);
    expect(store.getSnapshot().sectionsByProfile.local!.map((s) => s.sessionIds)).toEqual([[], ["p1"]]);

    // Assign to null (a drop into Pinned or Regular) clears membership.
    store.assignSidebarSection("local", "p1", null);
    expect(store.getSnapshot().sectionsByProfile.local!.map((s) => s.sessionIds)).toEqual([[], []]);

    // Unpinning never touches sections.
    store.assignSidebarSection("local", "p2", s1);
    store.setChatPinned("local", "p2", false);
    expect(store.getSnapshot().sectionsByProfile.local![0]!.sessionIds).toEqual(["p2"]);
  });

  it("sections_pin_settle_keeps_membership_for_hidden_pins_and_repin_reclaims", () => {
    // The web port of `sections_pin_settle_keeps_newer_moves_and_clears_
    // committed_pins`, adapted to the local synchronous seam: the store IS
    // the settle — a pin write lands and clears membership immediately; a
    // claimed pin stays hidden but keeps its bucket slot until it is
    // re-pinned (reclaim) or the section releases it.
    const store = new SidebarStore({ storage: memoryStorage() });
    const s1 = store.createSection("local", "A")!;
    // A raw pin assigned to a section without unpinning (the component's
    // drop path unpins first; the store's assign never touches pins).
    store.setChatPinned("local", "p1", true);
    store.assignSidebarSection("local", "p1", s1);
    expect(store.getSnapshot().pinnedByProfile.local).toEqual(["p1"]);
    expect(store.getSnapshot().sectionsByProfile.local![0]!.sessionIds).toEqual(["p1"]);

    // A hidden pin (raw bucket + claimed) unpinned is a no-op on the
    // DISPLAYED pins — the desktop's active_sidebar_pins early return.
    store.setChatPinned("local", "p1", false);
    expect(store.getSnapshot().pinnedByProfile.local).toEqual(["p1"]);
    expect(store.getSnapshot().sectionsByProfile.local![0]!.sessionIds).toEqual(["p1"]);

    // Re-pinning the claimed chat reclaims it: membership clears, the pin
    // write itself was already settled.
    store.setChatPinned("local", "p1", true);
    expect(store.getSnapshot().pinnedByProfile.local).toEqual(["p1"]);
    expect(store.getSnapshot().sectionsByProfile.local![0]!.sessionIds).toEqual([]);
  });
});

describe("healSidebarSectionsByProfile", () => {
  it("keeps_valid_sections_drops_junk_and_retains_empty_sections", () => {
    const healed = healSidebarSectionsByProfile({
      local: [
        { id: "a", name: "A", sessionIds: ["c1"], collapsed: true },
        { id: "", name: "No id" },
        { id: "b", name: "" },
        { id: "c", name: "C", sessionIds: "not-a-list" },
        { id: "a", name: "Duplicate id" },
        "junk",
        { id: "d", name: "Empty but valid" },
      ],
      "": [section("x", "X")],
      other: "not-a-list",
    });
    // Empty sections are RETAINED (the "Drop sessions here" target survives
    // a reload); junk heals out one by one; junk session lists heal empty.
    expect(healed).toEqual({
      local: [
        { id: "a", name: "A", sessionIds: ["c1"], collapsed: true },
        { id: "c", name: "C", sessionIds: [], collapsed: false },
        { id: "d", name: "Empty but valid", sessionIds: [], collapsed: false },
      ],
    });
  });

  it("non_objects_heal_to_the_empty_map", () => {
    expect(healSidebarSectionsByProfile(null)).toEqual({});
    expect(healSidebarSectionsByProfile([])).toEqual({});
    expect(healSidebarSectionsByProfile("x")).toEqual({});
  });
});

// ---- helpers ----

function chatRows(ids: readonly string[]): { chat: { id: string }; branch: null; changeRequest: null }[] {
  return ids.map((id) => ({ chat: { id }, branch: null, changeRequest: null }));
}
