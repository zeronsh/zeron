import { describe, expect, it } from "vitest";
import type { Device, DriveEntry, FolderEntry } from "@zeron/proto";
import { methods } from "@zeron/engine-client";
import type { EngineSession } from "../src/state/engine-session";
import { addSpaceStore, toggleAddSpace } from "../src/state/add-space";
import {
  addSpaceCompletion,
  breadcrumbs,
  browserRows,
  childPath,
  completionPrefixLen,
  deviceRows,
  filteredFolders,
  highlightRanges,
  isStaleResponse,
  locationRows,
  manualPathQuery,
  parentPath,
  pathUnder,
  segmentTarget,
  typedPathTarget,
} from "../src/lib/add-space";

/**
 * Ports of the desktop's picker tests (`crates/ui/src/pickers.rs` test
 * module) plus the add-space derivations from `spaces.rs` the palette
 * renders from: `folder_paths_and_breadcrumbs`, `completion_prefix_lengths`,
 * `segment_target_resolution`,
 * `typed_path_target_expands_absolute_and_home_paths`, the `is_stale` rule
 * (`spaces.rs:238-247`), the step-row filters (`add_space_devices` /
 * `add_space_locations`), the match ranges (`search_match_ranges`,
 * popover.rs), and the device-first flow test
 * `devices_locations_folders_and_back_clear_stale_state`.
 */

describe("folder_paths_and_breadcrumbs (pickers.rs)", () => {
  it("parentPath climbs and stops at the root", () => {
    expect(parentPath("/home/w/dev")).toBe("/home/w");
    expect(parentPath("/home")).toBe("/");
    expect(parentPath("/home/")).toBe("/");
    expect(parentPath("/")).toBe(null);
    expect(parentPath("")).toBe(null);
  });

  it("childPath joins without doubling the separator", () => {
    expect(childPath("/home", "w")).toBe("/home/w");
    expect(childPath("/", "home")).toBe("/home");
  });

  it("breadcrumbs walk root-first, accumulating the full path", () => {
    const crumbs = breadcrumbs("/home/w/dev");
    expect(crumbs.map(([label]) => label)).toEqual(["/", "home", "w", "dev"]);
    expect(crumbs[2]![1]).toBe("/home/w");
    expect(breadcrumbs("/")).toHaveLength(1);
  });
});

describe("completion_prefix_lengths (pickers.rs)", () => {
  it("is case-insensitive and indexes into the name", () => {
    expect(completionPrefixLen("Documents", "doc")).toBe(3);
    expect("Documents".slice(3)).toBe("uments");
    expect(completionPrefixLen("zeron", "zeron")).toBe(5);
    expect(completionPrefixLen("zeron", "")).toBe(0);
    expect(completionPrefixLen("zeron", "dev")).toBe(null);
    // Longer than the name → not a prefix.
    expect(completionPrefixLen("dev", "devel")).toBe(null);
  });

  it("multibyte names slice on a code-point boundary", () => {
    const len = completionPrefixLen("héllo", "hé");
    expect(len).not.toBe(null);
    expect("héllo".slice(len as number)).toBe("llo");
  });
});

describe("segment_target_resolution (pickers.rs)", () => {
  const names = ["github", "GitHub", "worktree"];

  it("exact casing beats the earlier case-insensitive sibling", () => {
    expect(segmentTarget(names, "GitHub")).toBe(1);
    expect(segmentTarget(names, "github")).toBe(0);
  });

  it("case-insensitive exact still lands without an exact-cased hit", () => {
    expect(segmentTarget(names, "WORKTREE")).toBe(2);
  });

  it("unique prefix descends; an ambiguous one keeps the slash honest", () => {
    expect(segmentTarget(names, "work")).toBe(2);
    expect(segmentTarget(names, "g")).toBe(null);
    expect(segmentTarget(names, "x")).toBe(null);
  });
});

describe("typed_path_target_expands_absolute_and_home_paths (pickers.rs)", () => {
  const home = "/home/wing";

  it("absolute paths trim their trailing slash and need no home", () => {
    expect(typedPathTarget("/disk2/", home)).toBe("/disk2");
    expect(typedPathTarget("/disk2/projects", home)).toBe("/disk2/projects");
    expect(typedPathTarget("/", home)).toBe("/");
    expect(typedPathTarget("/disk2", null)).toBe("/disk2");
  });

  it("home-relative paths expand against home", () => {
    expect(typedPathTarget("~", home)).toBe("/home/wing");
    expect(typedPathTarget("~/", home)).toBe("/home/wing");
    expect(typedPathTarget("~/github/", home)).toBe("/home/wing/github");
  });

  it("~x is a folder name; ~ cannot expand before home is known", () => {
    expect(typedPathTarget("~x", home)).toBe(null);
    expect(typedPathTarget("src", home)).toBe(null);
    expect(typedPathTarget("~/github", null)).toBe(null);
  });
});

describe("manual_path_query (spaces.rs:254-260)", () => {
  it("recognizes every typed-path shape, trimmed", () => {
    expect(manualPathQuery("/disk2")).toBe(true);
    expect(manualPathQuery("~/x")).toBe(true);
    expect(manualPathQuery("~")).toBe(true);
    expect(manualPathQuery("\\\\server")).toBe(true);
    expect(manualPathQuery("C:\\Users")).toBe(true);
    expect(manualPathQuery("  /disk2 ")).toBe(true);
    expect(manualPathQuery("zeron")).toBe(false);
    expect(manualPathQuery(".hidden")).toBe(false);
  });
});

describe("path_under (spaces.rs:497-500)", () => {
  it("is segment-aware — a sibling prefix is not a parent", () => {
    expect(pathUnder("/media/a", "/media")).toBe(true);
    expect(pathUnder("/media", "/media")).toBe(true);
    expect(pathUnder("/media/ab", "/media/a")).toBe(false);
    expect(pathUnder("/media/a", "/media/ab")).toBe(false);
    expect(pathUnder("/anything", "/")).toBe(true);
    expect(pathUnder("/anything", "")).toBe(true);
  });
});

const entry = (name: string, isDir: boolean, isRepo = false): FolderEntry => ({
  name,
  isDir,
  isRepo,
});

describe("browserRows + filteredFolders + completion (spaces.rs)", () => {
  const entries = [
    entry("dev", true, true),
    entry("notes.txt", false),
    entry("Documents", true),
    entry(".config", true),
    entry("github", true),
  ];

  it("browserRows keeps directories only", () => {
    expect(browserRows(entries).map((row) => row.name)).toEqual(["dev", "Documents", ".config", "github"]);
  });

  it("filtering ranks prefix matches first and hides dotfiles by default", () => {
    const rows = filteredFolders(entries, "");
    expect(rows.map((row) => row.name)).toEqual(["dev", "Documents", "github"]);
  });

  it("a leading dot reveals the dotfiles (client-side insurance)", () => {
    const rows = filteredFolders(entries, ".");
    expect(rows.map((row) => row.name)).toEqual([".config"]);
  });

  it("substring matches come after prefix matches", () => {
    const rows = filteredFolders(entries, "d");
    expect(rows.map((row) => row.name)).toEqual(["dev", "Documents"]);
  });

  it("completion prefers the highlighted row, else the first prefix match", () => {
    const rows = filteredFolders(entries, "");
    expect(addSpaceCompletion(rows, 0, "de")).toEqual({ name: "dev", suffix: "v" });
    expect(addSpaceCompletion(rows, 1, "de")).toEqual({ name: "dev", suffix: "v" });
    expect(addSpaceCompletion(rows, 0, "doc")).toEqual({ name: "Documents", suffix: "uments" });
    // An already-complete match previews nothing; an empty query neither.
    expect(addSpaceCompletion(rows, 0, "dev")).toBe(null);
    expect(addSpaceCompletion(rows, 0, "")).toBe(null);
    expect(addSpaceCompletion(rows, 0, "zzz")).toBe(null);
  });
});

describe("is_stale (spaces.rs:238-247)", () => {
  const flow = { identity: "id-1", revision: 3, deviceId: "device-a" };

  it("drops responses from another open, a superseded browse, or another device", () => {
    expect(isStaleResponse(flow, { identity: "id-1", revision: null, deviceId: "device-a" })).toBe(false);
    expect(isStaleResponse(flow, { identity: "id-2", revision: null, deviceId: "device-a" })).toBe(true);
    expect(isStaleResponse(flow, { identity: "id-1", revision: 4, deviceId: "device-a" })).toBe(true);
    expect(isStaleResponse(flow, { identity: "id-1", revision: null, deviceId: "device-b" })).toBe(true);
    // A null revision (path-keyed loads) never trips the revision check.
    expect(isStaleResponse(flow, { identity: "id-1", revision: 99, deviceId: "device-a" })).toBe(true);
    expect(isStaleResponse({ ...flow, revision: 99 }, { identity: "id-1", revision: 99, deviceId: "device-a" })).toBe(false);
  });
});

const drive = (name: string, path: string): DriveEntry => ({ name, path });

describe("step rows: deviceRows + locationRows (spaces.rs)", () => {
  const devices: Device[] = [
    { id: "d-local", name: "Studio", platform: "macos", lastSeenAt: null, createdAt: null },
    { id: "d-remote", name: "Server", platform: "linux", lastSeenAt: null, createdAt: null },
  ];

  it("deviceRows filters and ranks by name, preserving the device row", () => {
    expect(deviceRows(devices, "").map((row) => row.id)).toEqual(["d-local", "d-remote"]);
    expect(deviceRows(devices, "server").map((row) => row.id)).toEqual(["d-remote"]);
    expect(deviceRows(devices, "zzz")).toEqual([]);
  });

  it("locationRows always leads with Home, then the mounted drives", () => {
    const drives = [drive("Projects", "/projects"), drive("System", "/")];
    expect(locationRows(drives, "")).toEqual([
      { name: "Home", path: null },
      { name: "Projects", path: "/projects" },
      { name: "System", path: "/" },
    ]);
    expect(locationRows(drives, "proj")).toEqual([{ name: "Projects", path: "/projects" }]);
    // A failed drive load leaves the list at Home only — no error UI.
    expect(locationRows([], "")).toEqual([{ name: "Home", path: null }]);
  });
});

describe("highlightRanges (popover.rs search_match_ranges)", () => {
  it("inline matches keep adjacent word boundaries", () => {
    const text = "fieldnotes/fix-authentication-redirects";
    const ranges = highlightRanges(text, "authentication");
    expect(ranges).toHaveLength(1);
    expect(text.slice(0, ranges[0]!.start)).toBe("fieldnotes/fix-");
    expect(text.slice(ranges[0]!.start, ranges[0]!.end)).toBe("authentication");
    expect(text.slice(ranges[0]!.end)).toBe("-redirects");
  });

  it("highlights repeated case-insensitive and overlapping words", () => {
    expect(highlightRanges("New chat, new project", "NEW")).toEqual([
      { start: 0, end: 3 },
      { start: 10, end: 13 },
    ]);
    expect(highlightRanges("authentication", "auth authentication")).toEqual([{ start: 0, end: 14 }]);
    expect(highlightRanges("New chat", "  ")).toEqual([]);
    expect(highlightRanges("New chat", "settings")).toEqual([]);
  });

  it("preserves original unicode boundaries after lowercase expansion", () => {
    // UTF-16 code units here — the desktop's ranges are UTF-8 bytes, the
    // spans are the same characters.
    expect(highlightRanges("İstanbul café", "i CAFÉ")).toEqual([
      { start: 0, end: 1 },
      { start: 9, end: 13 },
    ]);
    expect(highlightRanges("🚀 CAFÉ", "café")).toEqual([{ start: 3, end: 7 }]);
  });
});

describe("addSpaceStore + toggleAddSpace (shell.rs)", () => {
  /**
   * The fixed `mod-k` binding's toggle, against the headless store (no
   * session attached: `open()` lands on the Devices step and fires no
   * loads). The exit window ends when the mounted palette reports Base
   * UI's `onOpenChangeComplete(false)` — `unmounted()` here stands in for
   * that callback.
   */
  const reaped = (): void => {
    addSpaceStore.unmounted();
  };

  it("a closed palette opens on the Devices step; a mounted one closes", () => {
    expect(addSpaceStore.getSnapshot().status).toBe("closed");
    expect(addSpaceStore.getSnapshot().flow).toBe(null);

    toggleAddSpace();
    expect(addSpaceStore.getSnapshot().status).toBe("open");
    const flow = addSpaceStore.getSnapshot().flow;
    expect(flow).not.toBe(null);
    expect(flow!.step).toBe("devices");
    expect(flow!.deviceId).toBe(null);

    // Mounted → the same chord closes it (the flow lives through the exit
    // window so the card can paint its way out).
    toggleAddSpace();
    expect(addSpaceStore.getSnapshot().status).toBe("closing");
    reaped();
    expect(addSpaceStore.getSnapshot().status).toBe("closed");
    expect(addSpaceStore.getSnapshot().flow).toBe(null);
  });

  it("the chord re-opens once the close has fully drained", () => {
    addSpaceStore.open();
    addSpaceStore.close();
    reaped();
    toggleAddSpace();
    expect(addSpaceStore.getSnapshot().status).toBe("open");
    // Leave the singleton closed for whichever test runs next.
    addSpaceStore.close();
    reaped();
    expect(addSpaceStore.getSnapshot().status).toBe("closed");
  });
});

describe("device-first New project flow (spaces.rs project_flow_tests)", () => {
  /**
   * The fake routed session: a fixed device list, a recording client whose
   * calls resolve instantly (ListFolders echoes the requested path), and an
   * optional local device id. The store only reads
   * `cache.getSnapshot().devices.rows`, `client.engineInfo`, and
   * `client.call`, so that is the whole surface.
   */
  function fakeSession(
    devices: Device[],
    calls: Array<{ method: string; params: Record<string, unknown> }>,
    localDeviceId: string | null = null,
  ): EngineSession {
    return {
      engine: { baseUrl: "https://engine.test", credential: "cred" },
      client: {
        engineInfo: { deviceId: localDeviceId },
        call: (method: string, params: Record<string, unknown>): Promise<unknown> => {
          calls.push({ method, params });
          if (method === methods.LIST_FOLDERS) {
            const path = typeof params.path === "string" ? params.path : "/home/studio";
            return Promise.resolve({ path, entries: [], truncated: false });
          }
          if (method === methods.LIST_DRIVES) {
            return Promise.resolve({ drives: [{ name: "Projects", path: "/projects" }] });
          }
          return Promise.resolve({});
        },
      },
      cache: {
        getSnapshot: () => ({
          devices: { rows: devices, loaded: devices.length > 0, error: null },
        }),
      },
    } as unknown as EngineSession;
  }

  const flush = (): Promise<void> => new Promise((resolve) => setTimeout(resolve, 0));

  const device = (id: string, name: string): Device => ({
    id,
    name,
    platform: id === "d-remote" ? "linux" : "macos",
    lastSeenAt: null,
    createdAt: null,
  });

  const cleanup = (): void => {
    addSpaceStore.close();
    addSpaceStore.unmounted();
  };

  it("devices → locations → folders and back clear stale state", async () => {
    try {
      const devices = [device("d-local", "Studio"), device("d-remote", "Server")];
      const calls: Array<{ method: string; params: Record<string, unknown> }> = [];
      addSpaceStore.attach({ session: fakeSession(devices, calls, "d-local"), goToCanvas: () => {} });

      // The Devices step: no pick, no loads — the query filters the list.
      addSpaceStore.open();
      let flow = addSpaceStore.getSnapshot().flow!;
      expect(flow.step).toBe("devices");
      expect(flow.deviceId).toBe(null);
      expect(calls).toEqual([]);

      addSpaceStore.setQuery("server");
      expect(deviceRows(devices, addSpaceStore.getSnapshot().flow!.query)).toHaveLength(1);
      addSpaceStore.openActive();
      flow = addSpaceStore.getSnapshot().flow!;
      expect(flow.step).toBe("locations");
      expect(flow.deviceId).toBe("d-remote");
      expect(flow.query).toBe("");
      await flush();
      flow = addSpaceStore.getSnapshot().flow!;
      expect(flow.drives).toEqual([{ name: "Projects", path: "/projects" }]);
      expect(flow.drivesLoading).toBe(false);
      expect(calls.map((call) => call.method)).toEqual([methods.LIST_DRIVES]);

      addSpaceStore.setQuery("projects");
      addSpaceStore.openActive();
      flow = addSpaceStore.getSnapshot().flow!;
      expect(flow.step).toBe("folders");
      expect(flow.location).toEqual({ name: "Projects", path: "/projects" });
      await flush();
      flow = addSpaceStore.getSnapshot().flow!;
      expect(flow.browserPath).toBe("/projects");
      expect(flow.listing).toEqual({ path: "/projects", entries: [], truncated: false });
      expect(calls.map((call) => call.method)).toEqual([methods.LIST_DRIVES, methods.LIST_FOLDERS]);

      // ← on the location's root retreats to Locations; the listing and
      // the location crumb state clear.
      addSpaceStore.goUp();
      flow = addSpaceStore.getSnapshot().flow!;
      expect(flow.step).toBe("locations");
      expect(flow.location).toBe(null);
      expect(flow.listing).toBe("idle");
      expect(flow.query).toBe("");

      // ← again retreats to Devices: the device, its drives and the
      // resolved home go with it.
      addSpaceStore.goUp();
      flow = addSpaceStore.getSnapshot().flow!;
      expect(flow.step).toBe("devices");
      expect(flow.deviceId).toBe(null);
      expect(flow.drives).toEqual([]);
      expect(flow.home).toBe(null);
      expect(flow.query).toBe("");

      // Slash navigation only applies to folders, never device search.
      addSpaceStore.setQuery("/projects/");
      flow = addSpaceStore.getSnapshot().flow!;
      expect(flow.step).toBe("devices");
      expect(flow.query).toBe("/projects/");
    } finally {
      cleanup();
    }
  });

  it("a folders browse descends and the parent climb stops at the location root", async () => {
    try {
      const devices = [device("d-local", "Studio")];
      const calls: Array<{ method: string; params: Record<string, unknown> }> = [];
      addSpaceStore.attach({ session: fakeSession(devices, calls, "d-local"), goToCanvas: () => {} });

      addSpaceStore.open();
      addSpaceStore.pickDevice("d-local");
      addSpaceStore.gotoLocation("Projects", "/projects");
      await flush();
      // Descend one level in; ← climbs back to the location root…
      addSpaceStore.descend("/projects/zeron", false);
      await flush();
      let flow = addSpaceStore.getSnapshot().flow!;
      expect(flow.listing).toEqual({ path: "/projects/zeron", entries: [], truncated: false });
      addSpaceStore.goUp();
      await flush();
      flow = addSpaceStore.getSnapshot().flow!;
      expect(flow.step).toBe("folders");
      expect(flow.listing).toEqual({ path: "/projects", entries: [], truncated: false });
      // …and ← on the root retreats to Locations instead of climbing past it.
      addSpaceStore.goUp();
      expect(addSpaceStore.getSnapshot().flow!.step).toBe("locations");
    } finally {
      cleanup();
    }
  });

  it("a deviceless folders load surfaces the error row instead of a forever-skeleton", async () => {
    try {
      // No session attached: the routed engine is gone mid-flow.
      addSpaceStore.attach({ session: null, goToCanvas: () => {} });
      addSpaceStore.open();
      addSpaceStore.pickDevice("d-gone");
      addSpaceStore.gotoLocation("Home", null);
      await flush();
      const flow = addSpaceStore.getSnapshot().flow!;
      expect(flow.step).toBe("folders");
      expect(flow.listing).toEqual({ error: "Device is not connected" });
    } finally {
      cleanup();
    }
  });
});
