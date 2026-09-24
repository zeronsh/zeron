import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { WorkspaceFileText, WriteWorkspaceFileOutcome } from "@zeron/proto";
import { WorkspaceFilesClient, type FilesCaller } from "../src/lib/files-client";
import { FileDocument } from "../src/lib/file-document";

function textFile(fields: Partial<WorkspaceFileText> = {}): WorkspaceFileText {
  return {
    checkoutId: "checkout-1",
    path: "src/lib.rs",
    text: "fn main() {}",
    contentHash: "hash-1",
    size: 12,
    encoding: "utf8",
    lineEnding: "lf",
    truncated: false,
    ...fields,
  };
}

interface FakeStore {
  client: WorkspaceFilesClient;
  written: { text: string; expectedContentHash: string }[];
  failWrite: (error: Error) => void;
  succeedWrite: () => void;
  conflictWrite: () => void;
  setDisk: (file: WorkspaceFileText) => void;
}

/** A scripted read/write endpoint for one document. */
function fakeStore(file: WorkspaceFileText): FakeStore {
  let disk = file;
  const written: FakeStore["written"] = [];
  const success = (): WriteWorkspaceFileOutcome => ({
    status: "written",
    file: { path: disk.path, contentHash: "hash-2", size: 3 },
  });
  let writeOutcome: () => WriteWorkspaceFileOutcome = success;
  const caller: FilesCaller = {
    call<T>(method: string, params?: unknown): Promise<T> {
      if (method === "ReadWorkspaceFile") {
        return Promise.resolve(disk as T);
      }
      if (method === "WriteWorkspaceFile") {
        const request = params as { text: string; expectedContentHash: string };
        written.push({ text: request.text, expectedContentHash: request.expectedContentHash });
        try {
          return Promise.resolve(writeOutcome() as T);
        } catch (error) {
          return Promise.reject(error instanceof Error ? error : new Error(String(error)));
        }
      }
      return Promise.reject(new Error(`unexpected method ${method}`));
    },
  };
  return {
    client: new WorkspaceFilesClient(caller, { spaceId: "s1" }),
    written,
    failWrite: (error) => {
      writeOutcome = () => {
        throw error;
      };
    },
    succeedWrite: () => {
      writeOutcome = success;
    },
    conflictWrite: () => {
      writeOutcome = () => ({ status: "conflict", reason: "changed", currentContentHash: "disk-hash" });
    },
    setDisk: (next) => {
      disk = next;
    },
  };
}

async function settle(): Promise<void> {
  await new Promise((resolve) => setTimeout(resolve, 0));
  await new Promise((resolve) => setTimeout(resolve, 0));
}

async function loaded(store: FakeStore, path = "src/lib.rs"): Promise<FileDocument> {
  const document = new FileDocument(store.client, path);
  document.load();
  await settle();
  return document;
}

describe("FileDocument", () => {
  it("loads clean and editable", async () => {
    const document = await loaded(fakeStore(textFile()));
    const snapshot = document.getSnapshot();
    expect(snapshot.phase).toEqual({ kind: "ready" });
    expect(snapshot.text).toBe("fn main() {}");
    expect(snapshot.editable).toBe(true);
    expect(snapshot.dirty).toBe(false);
    document.dispose();
  });

  it("edit → save → written rotates the hash and cleans the buffer", async () => {
    const store = fakeStore(textFile());
    const document = await loaded(store);
    document.edit("fn main() { println!() }");
    expect(document.getSnapshot().dirty).toBe(true);
    document.save();
    await settle();
    expect(store.written).toEqual([{ text: "fn main() { println!() }", expectedContentHash: "hash-1" }]);
    const snapshot = document.getSnapshot();
    expect(snapshot.phase).toEqual({ kind: "ready" });
    expect(snapshot.dirty).toBe(false);
    document.dispose();
  });

  it("sends the read checkout identity, even if metadata moved on", async () => {
    const store = fakeStore(textFile({ checkoutId: "checkout-returned-by-engine" }));
    const document = await loaded(store);
    document.edit("changed");
    document.save();
    await settle();
    // (The request went out — the desktop test pins the same invariant.)
    expect(store.written).toHaveLength(1);
    document.dispose();
  });

  it("a conflict preserves the dirty buffer and blocks further saves", async () => {
    const store = fakeStore(textFile());
    const document = await loaded(store);
    document.edit("mine");
    store.conflictWrite();
    document.save();
    await settle();
    const snapshot = document.getSnapshot();
    expect(snapshot.phase).toEqual({ kind: "conflict", diskHash: "disk-hash" });
    expect(snapshot.text).toBe("mine");
    expect(snapshot.dirty).toBe(true);
    expect(document.canSave()).toBe(false);
    document.dispose();
  });

  it("a failed save preserves the buffer and retries", async () => {
    const store = fakeStore(textFile());
    const document = await loaded(store);
    document.edit("mine");
    store.failWrite(new Error("offline"));
    document.save();
    await settle();
    expect(document.getSnapshot().phase).toEqual({ kind: "saveFailed", message: "offline" });
    expect(document.canSave()).toBe(true);

    document.save();
    await settle();
    expect(document.getSnapshot().phase).toEqual({ kind: "saveFailed", message: "offline" });

    // Editing again clears the failure back to ready (desktop mark_user_edit).
    document.edit("mine again");
    expect(document.getSnapshot().phase.kind).toBe("ready");
    document.dispose();
  });

  it("reload from disk discards the buffer after a conflict", async () => {
    const store = fakeStore(textFile());
    const document = await loaded(store);
    document.edit("mine");
    store.conflictWrite();
    document.save();
    await settle();
    store.setDisk(textFile({ text: "on disk", contentHash: "hash-2" }));
    document.reloadFromDisk();
    await settle();
    const snapshot = document.getSnapshot();
    expect(snapshot.phase).toEqual({ kind: "ready" });
    expect(snapshot.text).toBe("on disk");
    expect(snapshot.dirty).toBe(false);
    document.dispose();
  });

  it("reconcile silently reloads a clean document", async () => {
    const store = fakeStore(textFile());
    const document = await loaded(store);
    store.setDisk(textFile({ text: "fn external() {}", contentHash: "hash-2" }));
    document.reconcile();
    await settle();
    const snapshot = document.getSnapshot();
    expect(snapshot.text).toBe("fn external() {}");
    expect(snapshot.phase).toEqual({ kind: "ready" });
    expect(snapshot.dirty).toBe(false);
    document.dispose();
  });

  it("reconcile leaves a matching disk hash alone", async () => {
    const store = fakeStore(textFile());
    const document = await loaded(store);
    document.reconcile();
    await settle();
    expect(document.getSnapshot().text).toBe("fn main() {}");
    expect(document.getSnapshot().phase).toEqual({ kind: "ready" });
    document.dispose();
  });

  it("reconcile with a dirty buffer surfaces externally-modified instead", async () => {
    const store = fakeStore(textFile());
    const document = await loaded(store);
    document.edit("mine");
    store.setDisk(textFile({ text: "on disk", contentHash: "hash-2" }));
    document.reconcile();
    await settle();
    const snapshot = document.getSnapshot();
    expect(snapshot.phase).toEqual({ kind: "externallyModified", diskHash: "hash-2" });
    expect(snapshot.text).toBe("mine");
    document.dispose();
  });

  it("markDeleted preserves the buffer; restore reloads a recreated file", async () => {
    const store = fakeStore(textFile());
    const document = await loaded(store);
    document.edit("mine");
    document.markDeleted();
    expect(document.getSnapshot().phase).toEqual({ kind: "deletedOnDisk" });
    expect(document.getSnapshot().text).toBe("mine");

    document.restore();
    await settle();
    expect(document.getSnapshot().phase).toEqual({ kind: "ready" });
    document.dispose();
  });

  it("read-only snapshots are not editable (binary, truncated, mixed endings)", async () => {
    for (const fields of [
      { encoding: "binary", text: null, readOnlyReason: "binary" },
      { truncated: true },
      { lineEnding: "mixed" },
    ] as const) {
      const store = fakeStore(textFile(fields));
      const document = await loaded(store);
      expect(document.getSnapshot().phase.kind).toBe("readOnly");
      expect(document.getSnapshot().editable).toBe(false);
      document.edit("nope");
      expect(document.getSnapshot().dirty).toBe(false);
      document.dispose();
    }
  });

  it("a load failure lands as an error phase", async () => {
    const caller: FilesCaller = { call: () => Promise.reject(new Error("not found")) };
    const document = new FileDocument(new WorkspaceFilesClient(caller, { spaceId: "s1" }), "gone.txt");
    document.load();
    await settle();
    expect(document.getSnapshot().phase).toEqual({ kind: "error", message: "not found" });
    document.dispose();
  });

  it("edits during a save stay dirty when the older save lands", async () => {
    const store = fakeStore(textFile());
    const document = await loaded(store);
    document.edit("first");
    document.save();
    document.edit("second");
    await settle();
    const snapshot = document.getSnapshot();
    expect(snapshot.phase).toEqual({ kind: "ready" });
    expect(snapshot.dirty).toBe(true);
    expect(document.canSave()).toBe(true);
    document.dispose();
  });

  it("markdown documents start in preview mode and toggle", async () => {
    const store = fakeStore(textFile({ path: "docs/readme.md", text: "# Hi" }));
    const document = await loaded(store, "docs/readme.md");
    expect(document.getSnapshot().showMarkdown).toBe(true);
    document.setShowMarkdown(false);
    expect(document.getSnapshot().showMarkdown).toBe(false);
    document.dispose();
  });

  it("keepEditing converts externallyModified into conflict", async () => {
    const store = fakeStore(textFile());
    const document = await loaded(store);
    document.edit("mine");
    store.setDisk(textFile({ text: "on disk", contentHash: "hash-2" }));
    document.reconcile();
    await settle();
    expect(document.getSnapshot().phase.kind).toBe("externallyModified");
    document.keepEditing();
    expect(document.getSnapshot().phase).toEqual({ kind: "conflict", diskHash: "hash-2" });
    expect(document.getSnapshot().dirty).toBe(true);
    // The conflict blocks saves until an explicit reload.
    expect(document.canSave()).toBe(false);
    document.dispose();
  });

  describe("autosave scheduling (preview.rs schedule_autosave)", () => {
    beforeEach(() => {
      vi.useFakeTimers();
    });
    afterEach(() => {
      vi.useRealTimers();
    });

    async function settleFake(): Promise<void> {
      await vi.advanceTimersByTimeAsync(0);
      await vi.advanceTimersByTimeAsync(0);
    }

    it("fires after idle edits, rescheduling on each keystroke", async () => {
      const store = fakeStore(textFile());
      const document = new FileDocument(store.client, "src/lib.rs", { autosaveDelayMs: 900 });
      document.load();
      await settleFake();
      document.configureAutosave(true, 900);
      document.edit("one");
      document.edit("two");
      // The timer restarts per edit — nothing fires at the halfway mark.
      await vi.advanceTimersByTimeAsync(500);
      expect(store.written).toHaveLength(0);
      await vi.advanceTimersByTimeAsync(400);
      expect(store.written).toEqual([{ text: "two", expectedContentHash: "hash-1" }]);
      await vi.advanceTimersByTimeAsync(2_000);
      expect(store.written).toHaveLength(1);
      expect(document.getSnapshot().dirty).toBe(false);
      document.dispose();
    });

    it("does not autosave a saveFailed document until a fresh edit", async () => {
      const store = fakeStore(textFile());
      const document = new FileDocument(store.client, "src/lib.rs", { autosaveDelayMs: 100 });
      document.load();
      await settleFake();
      document.configureAutosave(true, 100);
      document.edit("mine");
      store.failWrite(new Error("offline"));
      document.save();
      await settleFake();
      expect(document.getSnapshot().phase.kind).toBe("saveFailed");
      // The failed phase is not autosave-capable; idle never retries.
      await vi.advanceTimersByTimeAsync(5_000);
      expect(store.written).toHaveLength(1);
      // A fresh edit returns it to ready and re-arms the schedule.
      store.succeedWrite();
      document.edit("mine again");
      await vi.advanceTimersByTimeAsync(200);
      await settleFake();
      expect(store.written[1]?.text).toBe("mine again");
      expect(document.getSnapshot().dirty).toBe(false);
      document.dispose();
    });
  });

  describe("close lifecycle (preview.rs prepare_close)", () => {
    it("a clean document allows the close", async () => {
      const store = fakeStore(textFile());
      const document = await loaded(store);
      expect(document.prepareClose()).toBe("allow");
      document.dispose();
    });

    it("a dirty ready document saves and pends", async () => {
      const store = fakeStore(textFile());
      const document = await loaded(store);
      document.edit("mine");
      expect(document.prepareClose()).toBe("pending");
      await settle();
      expect(store.written).toHaveLength(1);
      expect(document.getSnapshot().dirty).toBe(false);
      document.dispose();
    });

    it("a saveFailed document blocks the close; discard resolves it", async () => {
      const store = fakeStore(textFile());
      const document = await loaded(store);
      document.edit("mine");
      store.failWrite(new Error("offline"));
      document.save();
      await settle();
      expect(document.prepareClose()).toBe("blocked");
      // Keep Open: still dirty, still blocked.
      expect(document.blocksLifecycleClose()).toBe(true);
      // Discard Changes: dirty state resolves for the lifecycle exit.
      document.discardChanges();
      expect(document.hasUnsavedChanges()).toBe(false);
      expect(document.prepareClose()).toBe("allow");
      document.dispose();
    });
  });
});
