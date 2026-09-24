import { describe, expect, it } from "vitest";
import type { Chat, Space } from "@zeron/proto";
import type { ChatStatus } from "@zeron/engine-client";
import {
  archivedRows,
  chatPageRow,
  healedSpaceFilter,
  singleLine,
  spacesSorted,
} from "../src/lib/view";

const NOW = Date.parse("2026-09-16T12:00:00Z");

function chat(fields: Partial<Chat>): Chat {
  return {
    id: "chat",
    deviceId: "device-1",
    title: null,
    archived: false,
    cwd: null,
    branch: null,
    checkoutId: null,
    config: null,
    lastMessagePreview: null,
    lastMessageAt: null,
    createdAt: "2026-09-16T10:00:00Z",
    ...fields,
  };
}

function space(id: string, name: string | null): Space {
  return {
    id,
    deviceId: "device-1",
    path: `/srv/${id}`,
    name,
    gitDetected: false,
    createdAt: "2026-01-01T00:00:00Z",
  };
}

function status(fields: Partial<ChatStatus>): ChatStatus {
  return {
    chatId: "chat",
    deviceId: "device-1",
    status: "working",
    startedAt: null,
    updatedAt: "2026-09-16T11:59:30Z",
    lastCompletedTurn: null,
    ...fields,
  };
}

describe("singleLine", () => {
  it("collapses all whitespace runs like the desktop", () => {
    expect(singleLine("a\nb")).toBe("a b");
    expect(singleLine("  a\t\t b \r\n c  ")).toBe("a b c");
    expect(singleLine("plain")).toBe("plain");
    expect(singleLine("")).toBe("");
    expect(singleLine("\n\n")).toBe("");
  });
});

describe("spacesSorted", () => {
  it("orders case-insensitively by display name with an id tiebreak", () => {
    const spaces = [space("b", "Beta"), space("a", null), space("c", "alpha"), space("d", "Alpha")];
    // display names: Beta, b (basename of /srv/b... no — id "a" → path /srv/a → "a"), alpha, Alpha
    const sorted = spacesSorted(spaces);
    expect(sorted.map((row) => row.id)).toEqual(["a", "c", "d", "b"]);
  });

  it("breaks display-name ties by id", () => {
    const spaces = [space("z", "Same"), space("y", "Same")];
    expect(spacesSorted(spaces).map((row) => row.id)).toEqual(["y", "z"]);
  });
});

describe("healedSpaceFilter", () => {
  it("keeps a live filter and heals a dangling one to All projects", () => {
    const spaces = [space("s1", null)];
    expect(healedSpaceFilter("s1", spaces)).toBe("s1");
    expect(healedSpaceFilter("gone", spaces)).toBe(null);
    expect(healedSpaceFilter(null, spaces)).toBe(null);
  });
});

describe("archivedRows", () => {
  it("keeps only archived chats, under the filter scoped to that space", () => {
    const chats = [
      chat({ id: "live", spaceId: "s1" }),
      chat({ id: "arch", archived: true, spaceId: "s1" }),
      chat({ id: "arch-other", archived: true, spaceId: "s2" }),
      chat({ id: "arch-loose", archived: true, spaceId: null }),
    ];
    expect(archivedRows(chats, null, NOW).map((row) => row.chat.id).sort()).toEqual(["arch", "arch-loose", "arch-other"]);
    expect(archivedRows(chats, "s2", NOW).map((row) => row.chat.id)).toEqual(["arch-other"]);
  });

  it("orders by recency with the createdAt and id tiebreaks (sort_chats)", () => {
    const chats = [
      chat({ id: "b", archived: true, createdAt: "2026-09-16T09:00:00Z" }),
      chat({ id: "a", archived: true, createdAt: "2026-09-16T09:00:00Z" }),
      chat({ id: "fresh", archived: true, lastMessageAt: "2026-09-16T11:00:00Z" }),
      chat({ id: "newer-created", archived: true, createdAt: "2026-09-16T10:00:00Z" }),
    ];
    expect(archivedRows(chats, null, NOW).map((row) => row.chat.id)).toEqual(["fresh", "newer-created", "a", "b"]);
  });

  it("single-lines titles, falls back to New session, and stamps time-ago", () => {
    const rows = archivedRows(
      [
        chat({ id: "t", archived: true, title: "  multi\nline\ttitle  " }),
        chat({ id: "u", archived: true, title: null, createdAt: "2026-09-16T11:00:00Z" }),
      ],
      null,
      NOW,
    );
    const titled = rows.find((row) => row.chat.id === "t")!;
    expect(titled.title).toBe("multi line title");
    const untitled = rows.find((row) => row.chat.id === "u")!;
    expect(untitled.title).toBe("New session");
    expect(untitled.timeAgo).toBe("1h");
  });
});

describe("chatPageRow", () => {
  it("finds any chat by id, archived included", () => {
    const chats = [
      chat({ id: "live", title: "Live" }),
      chat({ id: "arch", title: "Old", archived: true, spaceId: "s1" }),
    ];
    const spaces = [space("s1", "Engine work")];
    const row = chatPageRow("arch", chats, spaces, [], NOW);
    expect(row?.chat.title).toBe("Old");
    expect(row?.chat.archived).toBe(true);
    expect(row?.project).toBe("Engine work");
  });

  it("returns undefined for unknown ids and dangling spaces", () => {
    const chats = [chat({ id: "dangling", spaceId: "gone" })];
    expect(chatPageRow("missing", chats, [], [], NOW)).toBe(undefined);
    expect(chatPageRow("dangling", chats, [], [], NOW)).toBe(undefined);
  });

  it("derives the live status like the sidebar rows", () => {
    const chats = [chat({ id: "busy" })];
    const statuses = [status({ chatId: "busy", status: "working", updatedAt: "2026-09-16T11:59:30Z" })];
    expect(chatPageRow("busy", chats, [], statuses, NOW)?.status).toBe("working");
  });
});
