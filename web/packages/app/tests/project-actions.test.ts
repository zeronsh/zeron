import { describe, expect, it } from "vitest";
import type { ProjectAction, ProjectActionDraft, ProjectActionsSnapshot, TerminalSession } from "@zeron/proto";
import {
  ACTION_LABEL_MIN_TITLEBAR_WIDTH,
  MAX_PROJECT_ACTION_NAME_CHARS,
  ProjectActionsStore,
  actionIconName,
  canRun,
  draftFromAction,
  preferredAction,
  projectActionContext,
  projectActionParams,
  showActionLabel,
  statusSnapshot,
  validateActionDraft,
  type ProjectActionContext,
  type ProjectActionTerminals,
  type ProjectActionsKey,
} from "../src/lib/project-actions";

/**
 * The Actions controller, against the desktop's (`crates/ui/src/
 * project_actions.rs` + the flows of `shell/actions_ui.rs`): preferred
 * selection, the b1484015 label cutoff, generation-guarded loads,
 * mutation gating (a8aa4967), the retryable unavailable surface
 * (f4365134), the run flow's terminal attach/failure, and the
 * engine-local routing context (ticket 24: the owning engine's client
 * plus a `targetDeviceId` hint the wire boundary strips).
 */

function action(id: string, setup = false): ProjectAction {
  return { id, name: id, command: id, icon: "play", runOnWorktreeCreate: setup };
}

function snapshot(actions: ProjectAction[], spaceId = "space-1"): ProjectActionsSnapshot {
  return { spaceId, actions, importableActions: [], projectFileIssue: null };
}

const KEY: ProjectActionsKey = { engineKey: "local", spaceId: "space-1" };

/** Drain the store's promise chains (call → then/catch → accept → bump). */
async function flush(rounds = 6): Promise<void> {
  for (let round = 0; round < rounds; round += 1) {
    await Promise.resolve();
  }
}

describe("preferred selection and the responsive cutoff", () => {
  const actions = [action("setup", true), action("dev")];

  it("prefers the saved choice, else the first non-setup, else the first", () => {
    expect(preferredAction(actions, "setup")!.id).toBe("setup");
    expect(preferredAction(actions, "gone")!.id).toBe("dev");
    expect(preferredAction(actions, null)!.id).toBe("dev");
    expect(preferredAction([action("setup", true)], null)!.id).toBe("setup");
    expect(preferredAction([], null)).toBeNull();
  });

  it("shows the label only when the titlebar has room (b1484015)", () => {
    expect(showActionLabel(ACTION_LABEL_MIN_TITLEBAR_WIDTH - 1)).toBe(false);
    expect(showActionLabel(ACTION_LABEL_MIN_TITLEBAR_WIDTH)).toBe(true);
    expect(showActionLabel(0)).toBe(false);
  });

  it("maps wire icons to the icon set's action glyphs", () => {
    expect(actionIconName("play")).toBe("actionPlay");
    expect(actionIconName("configure")).toBe("actionConfigure");
    expect(actionIconName("debug")).toBe("actionDebug");
  });

  it("draft_from_action echoes the editable fields", () => {
    const draft: ProjectActionDraft = draftFromAction(action("dev", true));
    expect(draft).toEqual({ name: "dev", command: "dev", icon: "play", runOnWorktreeCreate: true });
  });
});

describe("validateActionDraft (save_project_action's pre-flight)", () => {
  it("trims and enforces the engine's name/command rules", () => {
    expect(validateActionDraft(" Dev ", " pnpm dev ")).toBeNull();
    expect(validateActionDraft("   ", "cmd")).toBe("Action name is required");
    expect(validateActionDraft("a".repeat(MAX_PROJECT_ACTION_NAME_CHARS + 1), "cmd")).toContain("80 characters");
    expect(validateActionDraft("Dev", "")).toBe("Action command is required");
    expect(validateActionDraft("Dev", "x".repeat(16 * 1024 + 1))).toContain("16384 bytes");
  });
});

describe("projectActionContext (engine-local routing, ticket 24)", () => {
  const space = { id: "space-1", deviceId: "engine:v1:dev-b", path: "/b", name: null, checkoutId: null, gitDetected: true, createdAt: "2026-01-01T00:00:00Z" };
  const chat = { id: "chat-1", deviceId: "engine:v1:dev-b", title: null, archived: false, cwd: "/b", branch: null, checkoutId: null, sourceContext: null, config: null, lastMessagePreview: null, lastMessageAt: null, createdAt: "2026-01-01T00:00:00Z", spaceId: "space-1" };
  const client = {
    call<T>(_method: string, _params?: unknown): Promise<T> {
      return Promise.resolve(snapshot([]) as T);
    },
  };

  it("resolves the chat-owning space with a targetDeviceId hint when the device is remote", () => {
    const context = projectActionContext({
      chat,
      spaces: [space],
      engineKey: "b-url",
      client,
      localDeviceId: "engine:v1:dev-a",
    });
    expect(context).not.toBeNull();
    expect(context!.key).toEqual({ engineKey: "b-url", spaceId: "space-1" });
    expect(context!.chatId).toBe("chat-1");
    expect(context!.targetDeviceId).toBe("engine:v1:dev-b");
  });

  it("no hint when the chat's device is the local device", () => {
    const context = projectActionContext({
      chat,
      spaces: [space],
      engineKey: "b-url",
      client,
      localDeviceId: "engine:v1:dev-b",
    });
    expect(context!.targetDeviceId).toBeNull();
  });

  it("requires the chat, its space, and the client; the space must be the chat device's own", () => {
    expect(projectActionContext({ chat: null, spaces: [space], engineKey: "b", client, localDeviceId: null })).toBeNull();
    expect(projectActionContext({ chat, spaces: [], engineKey: "b", client, localDeviceId: null })).toBeNull();
    expect(projectActionContext({ chat, spaces: [space], engineKey: "b", client: null, localDeviceId: null })).toBeNull();
    const foreign = { ...space, deviceId: "engine:v1:dev-c" };
    expect(projectActionContext({ chat, spaces: [foreign], engineKey: "b", client, localDeviceId: null })).toBeNull();
  });

  it("projectActionParams merges only the routing hint", () => {
    expect(projectActionParams({ spaceId: "s" }, null)).toEqual({ spaceId: "s" });
    expect(projectActionParams({ spaceId: "s" }, "dev-b")).toEqual({ spaceId: "s", targetDeviceId: "dev-b" });
  });
});

/** A scripted RPC fake — the owning engine's client. */
class FakeClient {
  readonly calls: { method: string; params: Record<string, unknown> }[] = [];
  readonly #scripted = new Map<string, Promise<unknown>>();
  #handler: ((method: string, params: Record<string, unknown>) => Promise<unknown>) | null = null;

  /** One scripted outcome per method name (resolved promises replay). */
  replyTo(method: string, outcome: Promise<unknown>): void {
    this.#scripted.set(method, outcome);
  }

  /** A full dispatch handler, consulted after the scripted map misses. */
  replyWith(handler: (method: string, params: Record<string, unknown>) => Promise<unknown>): void {
    this.#handler = handler;
  }

  call<T>(method: string, params: unknown = {}): Promise<T> {
    const record = params as Record<string, unknown>;
    this.calls.push({ method, params: record });
    const scripted = this.#scripted.get(method);
    if (scripted !== undefined) {
      return scripted as Promise<T>;
    }
    if (this.#handler !== null) {
      return this.#handler(method, record) as Promise<T>;
    }
    return Promise.resolve(snapshot([]) as T);
  }
}

function contextOf(client: FakeClient, key: ProjectActionsKey = KEY): ProjectActionContext {
  return { key, chatId: "chat-1", client, targetDeviceId: null };
}

/** The terminal drawer fake — reserve/attach/fail recorded in order. */
class FakeTerminals implements ProjectActionTerminals {
  readonly reserved: { chatId: string; title: string }[] = [];
  readonly attached: { chatId: string; key: string; session: TerminalSession }[] = [];
  readonly failed: { chatId: string; key: string; message: string }[] = [];
  #failAttach = false;

  reserveTabForChat(chatId: string, title: string): string | null {
    this.reserved.push({ chatId, title });
    return `tab-${this.reserved.length}`;
  }

  attachReservedSession(chatId: string, key: string, session: TerminalSession): boolean {
    if (this.#failAttach) {
      return false;
    }
    this.attached.push({ chatId, key, session });
    return true;
  }

  failReservedTab(chatId: string, key: string, message: string): void {
    this.failed.push({ chatId, key, message });
  }

  failNextAttach(): void {
    this.#failAttach = true;
  }
}

describe("ProjectActionsStore loads", () => {
  it("loads on ensure and accepts the newest reply", async () => {
    const client = new FakeClient();
    const store = new ProjectActionsStore();
    store.ensure(contextOf(client));
    expect(store.activeStatus()?.kind).toBe("loading");
    expect(client.calls[0]).toEqual({ method: "ListProjectActions", params: { spaceId: "space-1" } });
    await flush();
    expect(store.activeStatus()).toEqual({ kind: "ready", snapshot: snapshot([]) });
    expect(canRun(store.activeStatus()!)).toBe(true);
  });

  it("a late reply for a superseded project never replaces the active one", async () => {
    const client = new FakeClient();
    const store = new ProjectActionsStore();
    const first = contextOf(client, { engineKey: "local", spaceId: "one" });
    const second = contextOf(client, { engineKey: "local", spaceId: "two" });
    const gate: { release: ((value: unknown) => void) | null } = { release: null };
    client.replyWith((method, params) => {
      if ((params.spaceId as string) === "one") {
        return new Promise((resolve) => {
          gate.release = resolve;
        });
      }
      return Promise.resolve(snapshot([], "two"));
    });
    store.ensure(first);
    store.ensure(second);
    await flush();
    expect(store.activeStatus()).toEqual({ kind: "ready", snapshot: snapshot([], "two") });
    gate.release?.(snapshot([action("stale")], "one"));
    await flush();
    expect(store.activeStatus()).toEqual({ kind: "ready", snapshot: snapshot([], "two") });
  });

  it("UnknownMethod hides (version skew); transport errors keep the last snapshot and stay retryable", async () => {
    const client = new FakeClient();
    const store = new ProjectActionsStore();
    const context = contextOf(client);
    client.replyWith(() => Promise.reject(new Error("unknown method: ListProjectActions")));
    store.ensure(context);
    await flush();
    expect(store.activeStatus()?.kind).toBe("unsupported");

    // A later load recovering (an explicit refresh, like the desktop's
    // retry) caches the snapshot; a transport failure after that keeps it.
    client.replyWith(() => Promise.resolve(snapshot([action("dev")])));
    store.refresh(context);
    await flush();
    expect(statusSnapshot(store.activeStatus()!)!.actions.map((a) => a.id)).toEqual(["dev"]);

    store.markUnavailable(KEY, "engine b is not connected");
    const status = store.activeStatus()!;
    expect(status.kind).toBe("unavailable");
    expect(statusSnapshot(status)!.actions.map((a) => a.id)).toEqual(["dev"]);
    expect(canRun(status)).toBe(false);
  });

  it("a first-load transport failure keeps a visible retry surface (f4365134)", async () => {
    const client = new FakeClient();
    const store = new ProjectActionsStore();
    client.replyTo("ListProjectActions", Promise.reject(new Error("engine remote is not connected")));
    store.ensure(contextOf(client));
    await flush();
    expect(statusSnapshot(store.activeStatus()!)).toBeNull();
    const visible = store.visibleSnapshot();
    expect(visible).toEqual({ spaceId: "space-1", actions: [], importableActions: [], projectFileIssue: null });
  });

  it("ensure is idempotent while a snapshot is cached", async () => {
    const client = new FakeClient();
    const store = new ProjectActionsStore();
    store.ensure(contextOf(client));
    await flush();
    const calls = client.calls.length;
    store.ensure(contextOf(client));
    expect(client.calls.length).toBe(calls);
  });
});

describe("ProjectActionsStore mutations (a8aa4967 gating)", () => {
  it("save upserts through the cache and closes the editor; a validation error stays in the dialog", async () => {
    const client = new FakeClient();
    const store = new ProjectActionsStore();
    client.replyTo("ListProjectActions", Promise.resolve(snapshot([])));
    store.ensure(contextOf(client));
    await flush();
    store.openEditor(KEY, { action: null, import: null });
    store.updateEditor({ name: "Dev", command: "pnpm dev" });
    client.replyTo("UpsertProjectAction", Promise.resolve(snapshot([action("dev")])));
    store.save(contextOf(client), () => {});
    expect(store.activeStatus()?.kind).toBe("saving");
    await flush();
    await flush();
    expect(store.editor).toBeNull();
    expect(statusSnapshot(store.activeStatus()!)!.actions.map((a) => a.id)).toEqual(["dev"]);

    store.openEditor(KEY, { action: null, import: null });
    store.updateEditor({ name: "", command: "x" });
    store.save(contextOf(client), () => {});
    await flush();
    expect(store.editor?.error).toBe("Action name is required");
  });

  it("a stale mutation reply cannot land after a newer mutation began", async () => {
    const client = new FakeClient();
    const store = new ProjectActionsStore();
    const context = contextOf(client);
    client.replyTo("ListProjectActions", Promise.resolve(snapshot([action("dev")])));
    store.ensure(context);
    await flush();
    store.openEditor(KEY, { action: action("dev"), import: null });
    store.updateEditor({ name: "Dev2", command: "pnpm dev2" });

    const gate: { release: ((value: ProjectActionsSnapshot) => void) | null } = { release: null };
    client.replyWith((method) => {
      if (method === "UpsertProjectAction") {
        if (gate.release === null) {
          return new Promise((resolve) => {
            gate.release = resolve;
          });
        }
        return Promise.resolve(snapshot([action("dev3")]));
      }
      return Promise.resolve(snapshot([action("dev")]));
    });

    // The first save's reply is still pending when the editor is closed,
    // reopened and saved again — the newer mutation supersedes it.
    store.save(context, () => {});
    store.closeEditor();
    store.openEditor(KEY, { action: action("dev"), import: null });
    store.updateEditor({ name: "Dev3", command: "pnpm dev3" });
    store.save(context, () => {});
    await flush();
    expect(statusSnapshot(store.activeStatus()!)!.actions[0]!.id).toBe("dev3");
    gate.release?.(snapshot([action("STALE")]));
    await flush();
    expect(statusSnapshot(store.activeStatus()!)!.actions[0]!.id).toBe("dev3");
  });

  it("activating a different project invalidates the in-flight mutation", async () => {
    const client = new FakeClient();
    const store = new ProjectActionsStore();
    const context = contextOf(client);
    client.replyTo("ListProjectActions", Promise.resolve(snapshot([action("dev")])));
    store.ensure(context);
    await flush();
    store.openEditor(KEY, { action: action("dev"), import: null });
    store.updateEditor({ name: "Dev2", command: "pnpm dev2" });
    client.replyTo("UpsertProjectAction", Promise.resolve(snapshot([action("dev2")])));
    store.save(context, () => {});
    store.activate({ engineKey: "local", spaceId: "other" });
    await flush();
    await flush();
    // The stale upsert reply lands nowhere: the old project's cache entry
    // still holds its ready snapshot, and the new key has no state at all.
    expect(store.activeStatus()).toBeNull();
    store.activate(KEY);
    expect(statusSnapshot(store.activeStatus()!)!.actions.map((a) => a.id)).toEqual(["dev"]);
  });

  it("delete removes the action and clears the preferred id through the callback", async () => {
    const client = new FakeClient();
    const store = new ProjectActionsStore();
    const context = contextOf(client);
    client.replyTo("ListProjectActions", Promise.resolve(snapshot([action("dev")])));
    store.ensure(context);
    await flush();
    store.openEditor(KEY, { action: action("dev"), import: null });
    const cleared: [string, string][] = [];
    client.replyTo("DeleteProjectAction", Promise.resolve(snapshot([])));
    store.remove(context, { onPreferredCleared: (spaceId, actionId) => cleared.push([spaceId, actionId]) });
    await flush();
    await flush();
    expect(store.editor).toBeNull();
    expect(cleared).toEqual([["space-1", "dev"]]);
    expect(statusSnapshot(store.activeStatus()!)!.actions).toEqual([]);
  });
});

describe("ProjectActionsStore run flow", () => {
  const session: TerminalSession = { id: "run-1", cwd: "/b", shell: "bash" };

  it("reserves the drawer tab, runs on the owning engine, attaches the returned PTY", async () => {
    const client = new FakeClient();
    const store = new ProjectActionsStore();
    const terminals = new FakeTerminals();
    const context = contextOf(client);
    client.replyTo("ListProjectActions", Promise.resolve(snapshot([action("dev")])));
    store.ensure(context);
    await flush();

    const started: [string, string][] = [];
    client.replyTo("RunProjectAction", Promise.resolve({ actionId: "dev", actionName: "dev", terminal: session }));
    await store.run(context, action("dev"), terminals, {
      onRunStarted: (spaceId, actionId) => started.push([spaceId, actionId]),
    });

    expect(terminals.reserved).toEqual([{ chatId: "chat-1", title: "dev" }]);
    expect(terminals.attached).toEqual([{ chatId: "chat-1", key: "tab-1", session }]);
    expect(terminals.failed).toEqual([]);
    expect(started).toEqual([["space-1", "dev"]]);
    expect(store.lastRunActionId).toBe("dev");
    expect(client.calls.find((call) => call.method === "RunProjectAction")?.params).toEqual({
      spaceId: "space-1",
      chatId: "chat-1",
      actionId: "dev",
      cols: 80,
      rows: 24,
    });
    // A successful run closes the menu.
    store.toggleMenu(context);
    client.replyTo("ListProjectActions", Promise.resolve(snapshot([action("dev")])));
    await store.run(context, action("dev"), terminals, { onRunStarted: () => {} });
    expect(store.menuOpen).toBe(false);
  });

  it("a failed run fails the reserved tab and marks the control unavailable", async () => {
    const client = new FakeClient();
    const store = new ProjectActionsStore();
    const terminals = new FakeTerminals();
    const context = contextOf(client);
    client.replyTo("ListProjectActions", Promise.resolve(snapshot([action("dev")])));
    store.ensure(context);
    await flush();

    client.replyTo("RunProjectAction", Promise.reject(new Error("Project chat not found")));
    await store.run(context, action("dev"), terminals, { onRunStarted: () => {} });
    expect(terminals.reserved).toHaveLength(1);
    expect(terminals.attached).toEqual([]);
    expect(terminals.failed).toEqual([{ chatId: "chat-1", key: "tab-1", message: "Project chat not found" }]);
    expect(store.activeStatus()?.kind).toBe("unavailable");
  });

  it("a tab closed mid-run releases the engine-opened PTY", async () => {
    const client = new FakeClient();
    const store = new ProjectActionsStore();
    const terminals = new FakeTerminals();
    terminals.failNextAttach();
    const context = contextOf(client);
    client.replyTo("ListProjectActions", Promise.resolve(snapshot([action("dev")])));
    store.ensure(context);
    await flush();

    client.replyTo("RunProjectAction", Promise.resolve({ actionId: "dev", actionName: "dev", terminal: session }));
    await store.run(context, action("dev"), terminals, { onRunStarted: () => {} });
    expect(terminals.attached).toEqual([]);
    const close = client.calls.find((call) => call.method === "CloseTerminal");
    expect(close?.params).toEqual({ terminalId: "run-1" });
  });
});

describe("editor lifecycle", () => {
  it("opens from an action, an import, or blank; typing patches the draft; the menu closes", async () => {
    const client = new FakeClient();
    const store = new ProjectActionsStore();
    const context = contextOf(client);
    store.toggleMenu(context);
    store.openEditor(KEY, { action: action("dev", true), import: null });
    expect(store.menuOpen).toBe(false);
    expect(store.editor).toMatchObject({ name: "dev", command: "dev", icon: "play", runOnWorktreeCreate: true, actionId: "dev" });

    store.openEditor(KEY, { action: null, import: { name: "Lint", command: "pnpm lint", icon: "lint", runOnWorktreeCreate: false } });
    expect(store.editor).toMatchObject({ name: "Lint", actionId: null });

    store.openEditor(KEY, { action: null, import: null });
    expect(store.editor).toMatchObject({ name: "", command: "", icon: "play" });

    store.updateEditor({ name: "Typed" });
    expect(store.editor?.name).toBe("Typed");
    store.setConfirmDelete(true);
    expect(store.editor?.confirmDelete).toBe(true);
    store.closeEditor();
    expect(store.editor).toBeNull();
  });

  it("activate resets the menu, the editor and the cache entry state", async () => {
    const client = new FakeClient();
    const store = new ProjectActionsStore();
    const context = contextOf(client);
    client.replyTo("ListProjectActions", Promise.resolve(snapshot([action("dev")])));
    store.ensure(context);
    await flush();
    store.openEditor(KEY, { action: action("dev"), import: null });
    store.toggleMenu(context);
    store.activate({ engineKey: "local", spaceId: "two" });
    expect(store.editor).toBeNull();
    expect(store.menuOpen).toBe(false);
    // The new key has no cached state until its own load begins.
    expect(store.activeStatus()).toBeNull();
    // The cached snapshot survives a re-activate for its key.
    store.activate(KEY);
    expect(store.activeStatus()).toEqual({ kind: "ready", snapshot: snapshot([action("dev")]) });
  });
});
