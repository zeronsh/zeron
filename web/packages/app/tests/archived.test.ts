import { describe, expect, it } from "vitest";
import type { Chat, Device, Space } from "@zeron/proto";
import { archivedChats, chatLocation } from "../src/lib/archived";
import { archivedChatRows, archivedRows } from "../src/lib/view";
import { archivedRightSlot } from "../src/components/archived-section";

const NOW = 1_800_000_000_000;

function chat(fields: Partial<Chat>): Chat {
  return {
    id: "chat",
    deviceId: "dev-1",
    title: null,
    archived: true,
    cwd: null,
    branch: null,
    checkoutId: null,
    sourceContext: null,
    config: null,
    lastMessagePreview: null,
    lastMessageAt: new Date(NOW - 3_600_000).toISOString(),
    createdAt: new Date(NOW - 86_400_000).toISOString(),
    harnessSessionId: null,
    harnessSessionCwd: null,
    spaceId: null,
    lastSeenAt: null,
    roomGen: null,
    ...fields,
  };
}

function device(id: string, name: string): Device {
  return { id, name, platform: "windows", lastSeenAt: null, createdAt: null };
}

describe("archivedChats (archived.rs:15-17, page-shaped)", () => {
  it("shows every archived chat, unscoped by the sidebar's space filter", () => {
    const rows = archivedChats(
      [
        chat({ id: "a", archived: false }),
        chat({ id: "b", spaceId: "space-1" }),
        chat({ id: "c", spaceId: "space-2" }),
      ],
      [],
      NOW,
    );
    // The shelf (archivedRows) would scope to one space's rows; the page
    // shows both — same comparator, no filter.
    expect(rows.map((row) => row.chat.id)).toEqual(["b", "c"]);
    expect(archivedRows([chat({ id: "b", spaceId: "space-1" }), chat({ id: "c", spaceId: "space-2" })], "space-1", NOW).map((r) => r.chat.id)).toEqual(["b"]);
  });

  it("carries the fuller per-row content the page renders", () => {
    const rows = archivedChats(
      [chat({ id: "b", title: "  Fix the parser  ", cwd: "/home/u/repo", branch: "main" })],
      [device("dev-1", "Studio desktop")],
      NOW,
    );
    expect(rows[0]!.title).toBe("  Fix the parser  ");
    expect(rows[0]!.device).toBe("Studio desktop");
    expect(rows[0]!.location).toBe("repo · main");
    expect(rows[0]!.timeAgo).toBe("1h");
  });

  it("falls back to Untitled session and omits unknown devices entirely", () => {
    const rows = archivedChats([chat({ title: null })], [], NOW);
    expect(rows[0]!.title).toBe("Untitled session");
    expect(rows[0]!.device).toBe(null);
    expect(rows[0]!.location).toBe(null);
  });

  it("sorts in the sidebar's recency order (lastUpdated)", () => {
    const older = chat({ id: "older", lastMessageAt: new Date(NOW - 10 * 3_600_000).toISOString() });
    const newer = chat({ id: "newer", lastMessageAt: new Date(NOW - 1_000).toISOString() });
    expect(archivedChats([older, newer], [], NOW).map((row) => row.chat.id)).toEqual(["newer", "older"]);
    expect(archivedChats([older, newer], [], NOW, "created").map((row) => row.chat.id)).toEqual(["newer", "older"]);
  });
});

describe("chatLocation (proto view.rs:252-270)", () => {
  it("joins project and branch, either alone, or neither", () => {
    expect(chatLocation(chat({ cwd: "/home/u/repo", branch: "main" }))).toBe("repo · main");
    expect(chatLocation(chat({ cwd: "/home/u/repo" }))).toBe("repo");
    expect(chatLocation(chat({ branch: "main" }))).toBe("main");
    expect(chatLocation(chat({}))).toBe(null);
    expect(chatLocation(chat({ cwd: "~", branch: "  " }))).toBe(null);
  });
});

describe("archivedRightSlot (spaces.rs:1669-1712)", () => {
  it("archivedRowRightSlotRendersExactlyOneChild", () => {
    // Rest — and every non-hover state, since focus never pins the pill
    // and touch never fires hover — renders the time-ago, only.
    expect(archivedRightSlot(false)).toBe("time");
    // Pointer within the row, pill included: the Unarchive pill, only.
    expect(archivedRightSlot(true)).toBe("pill");
    // Exactly one child at every instant: the choice is always one of
    // the two slots, never both.
    for (const hovered of [false, true]) {
      expect(["time", "pill"]).toContain(archivedRightSlot(hovered));
    }
  });
});

/*
 * The archived shelf's SHARED row data (upstream dfd2fc0c's
 * `sidebar_chat_data`): `archivedChatRows` derives the same `ChatRow`
 * the active list draws — project @ device folder, branch/PR metadata —
 * so the shelf shares layout and metadata in every sidebar mode.
 */
describe("archivedChatRows (sidebar_chat_data)", () => {
  function space(id: string, path: string, name: string | null = null): Space {
    return {
      id,
      deviceId: "dev-1",
      path,
      name,
      gitDetected: false,
      gitCheckedAt: null,
      checkoutId: null,
      createdAt: "2026-01-01T00:00:00Z",
    };
  }

  it("derives the active rows' shared metadata for archived chats", () => {
    const rows = archivedChatRows(
      [chat({ id: "b", spaceId: "space-1", title: "Fix the parser" })],
      [space("space-1", "/repos/fieldnotes")],
      null,
      [],
      NOW,
      [device("dev-1", "Studio desktop")],
    );
    expect(rows).toHaveLength(1);
    const row = rows[0]!;
    expect(row.project).toBe("fieldnotes");
    expect(row.projectPath).toBe("/repos/fieldnotes");
    expect(row.folder).toBe("fieldnotes @ Studio desktop");
    expect(row.deviceId).toBe("dev-1");
    expect(row.deviceName).toBe("Studio desktop");
  });

  it("respects the space filter and the show toggles", () => {
    const chats = [
      chat({ id: "in-space", spaceId: "space-1" }),
      chat({ id: "other-space", spaceId: "space-2" }),
      chat({ id: "home", spaceId: null }),
      chat({ id: "active", archived: false }),
    ];
    const rows = archivedChatRows(
      chats,
      [space("space-1", "/repos/one"), space("space-2", "/repos/two")],
      "space-1",
      [],
      NOW,
      [],
    );
    expect(rows.map((row) => row.chat.id)).toEqual(["in-space"]);
    // Show toggles clear the shared metadata the way they do for the
    // active list.
    const toggled = archivedChatRows(
      [chat({ id: "in-space", spaceId: "space-1" })],
      [space("space-1", "/repos/one")],
      null,
      [],
      NOW,
      [],
      { showBranch: false, showPullRequest: false, showHarness: false },
    );
    expect(toggled[0]!.branch).toBe(null);
    expect(toggled[0]!.changeRequest).toBe(null);
    expect(toggled[0]!.harness).toBe(null);
  });

  it("never hides a dangling-space row — it reads as the ? project", () => {
    const rows = archivedChatRows(
      [chat({ id: "dangling", spaceId: "gone" })],
      [],
      null,
      [],
      NOW,
      [],
    );
    expect(rows).toHaveLength(1);
    expect(rows[0]!.project).toBe("?");
    expect(rows[0]!.projectPath).toBe(null);
  });
});
