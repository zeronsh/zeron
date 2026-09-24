import { describe, expect, it } from "vitest";
import type { Chat, Space } from "@zeron/proto";
import type { ChatStatus } from "@zeron/engine-client";
import {
  attentionRank,
  chatIndicator,
  chatListRows,
  chatPageRow,
  displayStatus,
  effectiveIndicator,
  mergePendingSpaces,
  mostUrgent,
  projectLabel,
  sortRows,
  spaceDisplayName,
  statusWord,
  timeAgo,
  unseen,
  type ChatRow,
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

describe("unseen", () => {
  it("is true when the last message is newer than the seen marker", () => {
    expect(unseen(chat({ lastMessageAt: "2026-09-16T11:00:00Z", lastSeenAt: "2026-09-16T10:00:00Z" }))).toBe(true);
  });

  it("is true with activity and no seen marker", () => {
    expect(unseen(chat({ lastMessageAt: "2026-09-16T11:00:00Z" }))).toBe(true);
  });

  it("is false without activity or once seen", () => {
    expect(unseen(chat({}))).toBe(false);
    expect(unseen(chat({ lastMessageAt: "2026-09-16T11:00:00Z", lastSeenAt: "2026-09-16T11:00:00Z" }))).toBe(false);
    expect(unseen(chat({ lastMessageAt: "2026-09-16T11:00:00Z", lastSeenAt: "2026-09-16T11:30:00Z" }))).toBe(false);
  });
});

describe("effectiveIndicator", () => {
  it("carries working and awaiting input inside the staleness window", () => {
    expect(effectiveIndicator(status({ status: "working" }), NOW)).toBe("working");
    expect(effectiveIndicator(status({ status: "awaitingInput" }), NOW)).toBe("awaitingInput");
  });

  it("treats a session older than 45s as dead", () => {
    const stale = status({ status: "working", updatedAt: "2026-09-16T11:59:00Z" });
    expect(effectiveIndicator(stale, NOW)).toBe("none");
    expect(effectiveIndicator(status({ status: "awaitingInput", updatedAt: "2026-09-16T11:59:00Z" }), NOW)).toBe("none");
  });

  it("keeps errored and drops idle", () => {
    expect(effectiveIndicator(status({ status: "errored", updatedAt: "2026-09-16T08:00:00Z" }), NOW)).toBe("errored");
    expect(effectiveIndicator(status({ status: "idle" }), NOW)).toBe("none");
    expect(effectiveIndicator(undefined, NOW)).toBe("none");
  });
});

describe("chatIndicator", () => {
  it("prefers the live states", () => {
    expect(chatIndicator(chat({}), status({ status: "working" }))).toBe("working");
    expect(chatIndicator(chat({}), status({ status: "awaitingInput" }))).toBe("awaitingInput");
  });

  it("reads errored only while unseen, then falls back to completed", () => {
    const errored = status({ status: "errored" });
    expect(chatIndicator(chat({ lastMessageAt: "2026-09-16T11:00:00Z" }), errored)).toBe("errored");
    expect(chatIndicator(chat({ lastMessageAt: "2026-09-16T11:00:00Z", lastSeenAt: "2026-09-16T11:30:00Z" }), errored)).toBe("idle");
  });

  it("maps unseen to completed and everything else to idle", () => {
    expect(chatIndicator(chat({ lastMessageAt: "2026-09-16T11:00:00Z" }), undefined)).toBe("completed");
    expect(chatIndicator(chat({ lastSeenAt: "2026-09-16T11:00:00Z" }), undefined)).toBe("idle");
  });
});

describe("displayStatus", () => {
  it("drops a stale working session back to the seen-marker derivation", () => {
    const staleWorking = chat({ lastMessageAt: "2026-09-16T11:00:00Z" });
    const session = status({ status: "working", updatedAt: "2026-09-16T11:00:00Z" });
    expect(displayStatus(staleWorking, session, NOW)).toBe("completed");
  });

  it("keeps a fresh working session working", () => {
    expect(displayStatus(chat({}), status({ status: "working" }), NOW)).toBe("working");
  });
});

describe("attentionRank", () => {
  it("matches the desktop buckets — lower is more urgent", () => {
    expect(attentionRank("awaitingInput")).toBeLessThan(attentionRank("errored"));
    expect(attentionRank("errored")).toBeLessThan(attentionRank("working"));
    expect(attentionRank("working")).toBeLessThan(attentionRank("completed"));
    expect(attentionRank("completed")).toBeLessThan(attentionRank("idle"));
  });

  it("mostUrgent picks the min-rank status", () => {
    expect(mostUrgent(["idle", "completed", "working"])).toBe("working");
    expect(mostUrgent(["idle", "errored", "awaitingInput"])).toBe("awaitingInput");
    expect(mostUrgent(["idle", "idle"])).toBe("idle");
    expect(mostUrgent([])).toBe(null);
  });
});

describe("statusWord", () => {
  it("mirrors the desktop corner words", () => {
    expect(statusWord("working")).toBe("Working");
    expect(statusWord("awaitingInput")).toBe("Input");
    expect(statusWord("errored")).toBe("Failed");
    expect(statusWord("completed")).toBe("Done");
    expect(statusWord("idle")).toBe(null);
  });
});

describe("sortRows", () => {
  it("orders by lastMessageAt desc with createdAt fallback and id tiebreak", () => {
    const rows = [
      { chat: chat({ id: "a", createdAt: "2026-09-16T09:00:00Z" }) },
      { chat: chat({ id: "b", createdAt: "2026-09-16T10:00:00Z" }) },
      { chat: chat({ id: "c", createdAt: "2026-09-16T08:00:00Z", lastMessageAt: "2026-09-16T11:30:00Z" }) },
    ];
    expect(sortRows(rows).map((row) => row.chat.id)).toEqual(["c", "b", "a"]);
  });

  it("breaks full ties by id ascending", () => {
    const rows = [
      { chat: chat({ id: "z", createdAt: "2026-09-16T09:00:00Z" }) },
      { chat: chat({ id: "a", createdAt: "2026-09-16T09:00:00Z" }) },
    ];
    expect(sortRows(rows).map((row) => row.chat.id)).toEqual(["a", "z"]);
  });
});

describe("timeAgo", () => {
  it("buckets like the desktop's format_time_ago", () => {
    expect(timeAgo("2026-09-16T11:59:40Z", NOW)).toBe("now");
    expect(timeAgo("2026-09-16T11:55:00Z", NOW)).toBe("5m");
    expect(timeAgo("2026-09-16T09:00:00Z", NOW)).toBe("3h");
    expect(timeAgo("2026-09-14T12:00:00Z", NOW)).toBe("2d");
    expect(timeAgo("2026-09-09T12:00:00Z", NOW)).toBe("1w");
    expect(timeAgo("2026-08-19T12:00:00Z", NOW)).toBe("4w");
    expect(timeAgo("2026-08-01T12:00:00Z", NOW)).toBe("1mo");
    expect(timeAgo("2024-09-16T12:00:00Z", NOW)).toBe("2y");
  });

  it("clamps future timestamps to now", () => {
    expect(timeAgo("2026-09-16T12:00:30Z", NOW)).toBe("now");
  });
});

describe("projectLabel and spaceDisplayName", () => {
  it("labels from the cwd basename, treating home as unlabeled", () => {
    expect(projectLabel("C:\\dev\\zeron")).toBe("zeron");
    expect(projectLabel("/home/me/project")).toBe("project");
    expect(projectLabel("~")).toBe(null);
    expect(projectLabel(null)).toBe(null);
  });

  it("prefers the rename, then the folder basename", () => {
    expect(spaceDisplayName({ id: "s", deviceId: "d", path: "/srv/app", name: "App", gitDetected: false, createdAt: "2026-01-01T00:00:00Z" })).toBe("App");
    expect(spaceDisplayName({ id: "s", deviceId: "d", path: "/srv/app", name: null, gitDetected: false, createdAt: "2026-01-01T00:00:00Z" })).toBe("app");
  });
});

describe("mergePendingSpaces", () => {
  const space = (id: string, path: string, name: string | null): Space => ({
    id,
    deviceId: "device-1",
    path,
    name,
    gitDetected: false,
    createdAt: "2026-01-01T00:00:00Z",
  });

  it("passes the confirmed rows through untouched with nothing pending", () => {
    const rows = [space("a", "/srv/a", "Alpha"), space("b", "/srv/b", null)];
    expect(mergePendingSpaces(rows, [])).toBe(rows);
  });

  it("appends optimistic rows the watch frame has not confirmed yet", () => {
    const rows = [space("a", "/srv/a", "Alpha")];
    const pending = [space("p1", "/srv/new", null)];
    const merged = mergePendingSpaces(rows, pending);
    expect(merged).toHaveLength(2);
    expect(merged.map((row) => row.id)).toEqual(["a", "p1"]);
  });

  it("drops an optimistic row once its id is confirmed — never a duplicate", () => {
    const confirmed = [space("p1", "/srv/new", "Renamed")];
    const pending = [space("p1", "/srv/new", null), space("p2", "/srv/other", null)];
    const merged = mergePendingSpaces(confirmed, pending);
    // The confirmed row wins; only the still-unconfirmed sibling survives.
    expect(merged.map((row) => row.id)).toEqual(["p1", "p2"]);
    expect(merged[0]!.name).toBe("Renamed");
  });
});

describe("chatListRows", () => {
  const space = (id: string, name: string | null): Space => ({
    id,
    deviceId: "device-1",
    path: `/srv/${id}`,
    name,
    gitDetected: false,
    createdAt: "2026-01-01T00:00:00Z",
  });

  it("streams live statuses onto the rows and keeps recency order", () => {
    const chats = [
      chat({ id: "fresh", lastMessageAt: "2026-09-16T11:00:00Z" }),
      chat({ id: "busy", lastMessageAt: "2026-09-16T11:30:00Z" }),
      chat({ id: "old", createdAt: "2026-09-16T09:00:00Z" }),
    ];
    const statuses = [
      status({ chatId: "busy", status: "working", updatedAt: "2026-09-16T11:59:30Z" }),
      status({ chatId: "fresh", status: "awaitingInput", updatedAt: "2026-09-16T11:59:30Z" }),
    ];
    const rows = chatListRows(chats, [], statuses, NOW);
    expect(rows.map((row) => row.chat.id)).toEqual(["busy", "fresh", "old"]);
    expect(rows.map((row) => row.status)).toEqual(["working", "awaitingInput", "idle"]);
  });

  it("hides archived chats and chats of unknown spaces, keeps project-less ones", () => {
    const chats = [
      chat({ id: "gone", archived: true }),
      chat({ id: "dangling", spaceId: "missing" }),
      chat({ id: "loose", spaceId: null }),
      chat({ id: "spaced", spaceId: "s1" }),
    ];
    const rows = chatListRows(chats, [space("s1", "Engine work")], [], NOW);
    expect(rows.map((row) => row.chat.id)).toEqual(["loose", "spaced"]);
    expect(rows[1]!.project).toBe("Engine work");
  });

  it("labels project-less rows ~ and stamps branches from the source context", () => {
    const rows = chatListRows(
      [
        chat({
          id: "a",
          cwd: "/home/me/zeron",
          branch: "legacy-scalar",
          sourceContext: { checkoutId: "c1", repoRoot: "/home/me/zeron", cwd: "/home/me/zeron", branch: "feat/web", observedAt: "2026-09-16T10:00:00Z" },
        }),
        chat({ id: "b", cwd: "~", branch: null }),
      ],
      [],
      [],
      NOW,
    );
    // Project-less sessions read as "~" (spaces.rs:1387), and only the
    // conversation-owned source context's branch counts — the legacy scalar
    // cannot prove a worktree has not switched since it was written.
    expect(rows[0]!.project).toBe("~");
    expect(rows[0]!.branch).toBe("feat/web");
    expect(rows[1]!.project).toBe("~");
    expect(rows[1]!.branch).toBe(null);
  });

  it("marks unseen chats completed and seen ones idle", () => {
    const rows = chatListRows(
      [chat({ id: "unseen", lastMessageAt: "2026-09-16T11:00:00Z" }), chat({ id: "seen", lastMessageAt: "2026-09-16T11:00:00Z", lastSeenAt: "2026-09-16T11:30:00Z" })],
      [],
      [],
      NOW,
    ) as ChatRow[];
    expect(rows.find((row) => row.chat.id === "unseen")!.status).toBe("completed");
    expect(rows.find((row) => row.chat.id === "seen")!.status).toBe("idle");
  });
});

describe("danglingSpaceChatsHiddenOnlyWhenSpacesAreLoaded (ticket 43)", () => {
  const space = (id: string, name: string | null): Space => ({
    id,
    deviceId: "device-1",
    path: `/srv/${id}`,
    name,
    gitDetected: false,
    createdAt: "2026-01-01T00:00:00Z",
  });

  it("danglingSpaceChatsHiddenOnlyWhenSpacesAreLoaded", () => {
    const chats = [chat({ id: "lagging", spaceId: "s1" }), chat({ id: "loose", spaceId: null })];

    // Spaces frame still out (loading, or errored before any frame): the
    // space-attached row renders with the "?" label — a lagging or failed
    // spaces stream must never blank space-attached chats.
    const unresolved = chatListRows(chats, [], [], NOW, [], { spacesLoaded: false });
    expect(unresolved.map((row) => row.chat.id)).toEqual(["lagging", "loose"]);
    expect(unresolved[0]!.project).toBe("?");
    expect(unresolved[0]!.folder).toBe("?");
    expect(unresolved[1]!.project).toBe("~");

    // The frame lands: the label resolves to the space's display name.
    const landed = chatListRows(chats, [space("s1", "Engine work")], [], NOW, [], { spacesLoaded: true });
    expect(landed.map((row) => row.chat.id)).toEqual(["lagging", "loose"]);
    expect(landed[0]!.project).toBe("Engine work");

    // Loaded and truly missing: the row hides — the desktop's
    // merged-registry rule (state.rs:1438).
    const gone = chatListRows(
      [chat({ id: "gone", spaceId: "nope" }), chat({ id: "kept", spaceId: "s1" })],
      [space("s1", "Alpha")],
      [],
      NOW,
      [],
      { spacesLoaded: true },
    );
    expect(gone.map((row) => row.chat.id)).toEqual(["kept"]);
    expect(gone[0]!.project).toBe("Alpha");
  });

  it("an errored spaces stream keeps the rows up with the ? label", () => {
    // errored-before-loaded: the RowSet's loaded flag stays false — the
    // same unresolved branch as the lag, so the invariant holds.
    const rows = chatListRows([chat({ id: "dangling", spaceId: "x" })], [], [], NOW, [], { spacesLoaded: false });
    expect(rows.map((row) => row.chat.id)).toEqual(["dangling"]);
    expect(rows[0]!.project).toBe("?");
  });

  it("chatPageRow applies the same gate as the list", () => {
    const chats = [chat({ id: "lagging", spaceId: "s1" })];
    const unresolved = chatPageRow("lagging", chats, [], [], NOW, [], undefined, false);
    expect(unresolved!.project).toBe("?");
    const landed = chatPageRow("lagging", chats, [space("s1", "Engine work")], [], NOW, [], undefined, true);
    expect(landed!.project).toBe("Engine work");
    const hidden = chatPageRow("lagging", chats, [], [], NOW, [], undefined, true);
    expect(hidden).toBe(undefined);
  });
});
