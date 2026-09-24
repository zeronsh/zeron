import { afterEach, describe, expect, it, vi } from "vitest";
import type { Chat, Device, Space } from "@zeron/proto";
import { actionsFor, matchesQuery, paletteChats } from "../src/lib/command-palette";
import {
  commandPaletteStore,
  toggleCommandPalette,
} from "../src/state/command-palette";
import { addSpaceStore } from "../src/state/add-space";

/**
 * Ports of the desktop's command palette tests
 * (`crates/ui/src/shell/command_palette.rs` test module) plus the store's
 * open/close/activate lifecycle: `action_search_hides_empty_section_and_
 * preserves_order`, `search_matches_words_across_chat_metadata`, the
 * global chat-history search (title/project/device/branch/PR targets,
 * archived included, the 30-row cap after sorting), and the activation
 * intents (chat rows carry the chat id they launch — upstream 62c52329's
 * argument fix, web shape).
 */

function chat(partial: Partial<Chat> & { readonly id: string }): Chat {
  return {
    deviceId: "local",
    spaceId: "project",
    title: null,
    archived: false,
    createdAt: "2026-09-01T00:00:00Z",
    lastMessageAt: null,
    lastSeenAt: null,
    config: null,
    sourceContext: null,
    ...partial,
  } as Chat;
}

describe("action_search_hides_empty_section_and_preserves_order (command_palette.rs)", () => {
  it("an empty query lists every action in order, dark resolving the theme action to light", () => {
    expect(actionsFor("", true).map((action) => action.id)).toEqual([
      "new-chat",
      "new-project",
      "settings",
      "theme",
    ]);
    expect(actionsFor("", true).at(-1)?.theme).toBe("light");
  });

  it("matching actions filter in place; a miss hides the section", () => {
    expect(actionsFor("new", true).map((action) => action.id)).toEqual(["new-chat", "new-project"]);
    expect(actionsFor("settings", true).map((action) => action.id)).toEqual(["settings"]);
    expect(actionsFor("deployment", true)).toEqual([]);
  });
});

describe("theme_action_targets_the_opposite_resolved_appearance (b4dd24d7)", () => {
  it("dark resolves to the light theme; light resolves to dark", () => {
    expect(actionsFor("theme", true).map((action) => action.theme)).toEqual(["light"]);
    expect(actionsFor("theme", false).map((action) => action.theme)).toEqual(["dark"]);
    expect(actionsFor("light", true).map((action) => action.theme)).toEqual(["light"]);
    expect(actionsFor("dark", false).map((action) => action.theme)).toEqual(["dark"]);
  });
});

describe("search_matches_words_across_chat_metadata (command_palette.rs)", () => {
  it("every word must appear, case-insensitively", () => {
    expect(matchesQuery("mac auth", "Fix authentication Zeron @ MacBook main")).toBe(true);
    expect(matchesQuery("  ", "Any chat")).toBe(true);
    expect(matchesQuery("mac windows", "Zeron @ MacBook")).toBe(false);
  });
});

describe("paletteChats (command_entries)", () => {
  const spaces: Space[] = [
    { id: "project", deviceId: "local", path: "/tmp/fieldnotes", name: null } as unknown as Space,
  ];
  const devices: Device[] = [
    { id: "local", name: "This device", platform: "windows", lastSeenAt: null } as unknown as Device,
  ];
  const chats: Chat[] = [
    chat({ id: "a", title: "Fix authentication redirects" }),
    chat({ id: "b", title: "Add deployment status", deviceId: "remote" }),
    chat({ id: "c", title: "Untouched", archived: true }),
  ];

  it("searches title, project, device, and branch metadata", () => {
    const rows = paletteChats({
      chats,
      spaces,
      statuses: [],
      devices,
      changeRequests: new Map(),
      now: Date.parse("2026-09-22T00:00:00Z"),
      query: "fieldnotes auth",
    });
    // "b" has no space and the "remote" device is unknown; only "a" matches.
    expect(rows.map((row) => row.chat.id)).toEqual(["a"]);
    expect(rows[0]!.folder).toBe("fieldnotes @ This device");
  });

  it("archived chats remain searchable", () => {
    const rows = paletteChats({
      chats,
      spaces,
      statuses: [],
      devices,
      changeRequests: new Map(),
      now: Date.parse("2026-09-22T00:00:00Z"),
      query: "untouched",
    });
    expect(rows.map((row) => row.chat.id)).toEqual(["c"]);
    expect(rows[0]!.archived).toBe(true);
  });

  it("a PR's number, title, and refs are search targets", () => {
    const changeRequests = new Map([
      [
        "a",
        {
          number: 421,
          title: "Palette polish",
          headRef: "wing/palette",
          baseRef: "main",
          state: "open" as const,
          url: "https://example.test/pr/421",
          provider: "github",
        },
      ],
    ]);
    const byNumber = paletteChats({
      chats,
      spaces,
      statuses: [],
      devices,
      changeRequests,
      now: 0,
      query: "421 palette",
    });
    expect(byNumber.map((row) => row.chat.id)).toEqual(["a"]);
    expect(byNumber[0]!.changeRequest?.number).toBe(421);
  });

  it("caps the history at 30 matching chats after sorting", () => {
    const many: Chat[] = [];
    for (let ix = 0; ix < 50; ix += 1) {
      const seconds = String(ix).padStart(2, "0");
      many.push(chat({ id: `row-${ix}`, title: `Chat ${ix}`, createdAt: `2026-09-01T00:00:${seconds}Z` }));
    }
    const rows = paletteChats({
      chats: many,
      spaces,
      statuses: [],
      devices,
      changeRequests: new Map(),
      now: 0,
      query: "",
    });
    expect(rows).toHaveLength(30);
    // lastUpdated sort: newest first.
    expect(rows[0]!.chat.id).toBe("row-49");
  });

  it("an unknown project reads as ~ (no space id) or ? (dangling id)", () => {
    const rows = paletteChats({
      chats: [
        chat({ id: "home", spaceId: null }),
        chat({ id: "dangling", spaceId: "missing" }),
      ],
      spaces,
      statuses: [],
      devices,
      changeRequests: new Map(),
      now: 0,
      query: "",
    });
    const byId = new Map(rows.map((row) => [row.chat.id, row.project]));
    expect(byId.get("home")).toBe("~");
    expect(byId.get("dangling")).toBe("?");
  });
});

describe("CommandPaletteStore (toggle/close/activate)", () => {
  afterEach(() => {
    commandPaletteStore.forceClose();
    addSpaceStore.forceClose();
  });

  it("toggle opens, then closes; open replaces the add-space palette", () => {
    addSpaceStore.open();
    toggleCommandPalette();
    expect(commandPaletteStore.getSnapshot().status).toBe("open");
    // The add-space card left the open state (its exit window may still be
    // fading — the web's close drains through `[data-closed]`).
    expect(addSpaceStore.getSnapshot().status).not.toBe("open");
    toggleCommandPalette();
    expect(commandPaletteStore.getSnapshot().status).toBe("closing");
  });

  it("a search edit resets the highlight; move wraps at both ends", () => {
    commandPaletteStore.open();
    commandPaletteStore.setQuery("new");
    expect(commandPaletteStore.getSnapshot().active).toBe(0);
    commandPaletteStore.move(1, 3);
    commandPaletteStore.move(1, 3);
    expect(commandPaletteStore.getSnapshot().active).toBe(2);
    commandPaletteStore.move(1, 3);
    expect(commandPaletteStore.getSnapshot().active).toBe(0);
    commandPaletteStore.move(-1, 3);
    expect(commandPaletteStore.getSnapshot().active).toBe(2);
  });

  it("unmounted drops the query and highlight", () => {
    commandPaletteStore.open();
    commandPaletteStore.setQuery("auth");
    commandPaletteStore.move(1, 3);
    commandPaletteStore.close();
    commandPaletteStore.unmounted();
    expect(commandPaletteStore.getSnapshot()).toEqual({
      status: "closed",
      query: "",
      active: 0,
    });
  });

  it("chat entries carry the chat id they launch (62c52329, web shape)", () => {
    const openChat = vi.fn();
    commandPaletteStore.attach({
      session: null,
      goToCanvas: () => {},
      openChat,
      openSettings: () => {},
    });
    commandPaletteStore.open();
    commandPaletteStore.activateEntry({ kind: "chat", chatId: "chat-7" });
    expect(openChat).toHaveBeenCalledWith("chat-7");
    expect(commandPaletteStore.getSnapshot().status).not.toBe("open");
  });

  it("New project closes the palette and opens the add-space flow", () => {
    commandPaletteStore.open();
    commandPaletteStore.activateEntry({ kind: "new-project" });
    expect(commandPaletteStore.getSnapshot().status).not.toBe("open");
    expect(addSpaceStore.getSnapshot().status).toBe("open");
  });

  it("New chat and Open settings route through the attached context", () => {
    const goToCanvas = vi.fn();
    const openSettings = vi.fn();
    commandPaletteStore.attach({
      session: null,
      goToCanvas,
      openChat: () => {},
      openSettings,
    });
    commandPaletteStore.open();
    commandPaletteStore.activateEntry({ kind: "new-chat" });
    commandPaletteStore.open();
    commandPaletteStore.activateEntry({ kind: "settings" });
    expect(goToCanvas).toHaveBeenCalledTimes(1);
    expect(openSettings).toHaveBeenCalledTimes(1);
  });

  it("the theme action keeps the palette open and switches the mode (b4dd24d7)", () => {
    const setTheme = vi.fn();
    commandPaletteStore.open();
    commandPaletteStore.activateEntry({ kind: "theme", setTheme });
    expect(setTheme).toHaveBeenCalledTimes(1);
    // The palette stays open so the action updates to its next state.
    expect(commandPaletteStore.getSnapshot().status).toBe("open");
  });
});
