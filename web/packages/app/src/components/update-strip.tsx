import { useEffect, useSyncExternalStore } from "react";
import type { UpdateStatus } from "@zeron/proto";
import type { EngineClient } from "@zeron/engine-client";
import { useEngineSession } from "../state/session-provider";
import { uiSettings } from "../state/ui-settings";

/**
 * The update strip — the desktop's `render_update_strip` (`shell.rs:5035`).
 *
 * The web build is never a desktop install, so only the ADVISORY branch
 * exists: ``Update available — v{latest} · run `zeron update` ``. Clicking
 * dismisses that version (persisted through the settings store). The
 * `UpdateFlow` union keeps the desktop's download/stage/relaunch branches in
 * the type so they can be added later without reshaping the component.
 */

/** `zeron_rpc::UPDATE_STATUS` — the engine's update-facts stream. */
const UPDATE_STATUS = "UpdateStatus";

/**
 * The update lifecycle a DESKTOP install drives. The web only ever renders
 * the advisory label, but the union is the desktop's shape.
 */
export type UpdateFlow =
  | { kind: "idle" }
  | { kind: "downloading" }
  | { kind: "ready" }
  | { kind: "failed"; message: string };

export function updateStripLabel(
  flow: UpdateFlow,
  latestVersion: string,
  desktopInstall: boolean,
): { label: string; clickable: boolean } {
  if (!desktopInstall) {
    return { label: `Update available — v${latestVersion} · run \`zeron update\``, clickable: true };
  }
  switch (flow.kind) {
    case "idle":
      return { label: `Update available — v${latestVersion}`, clickable: true };
    case "downloading":
      return { label: `Downloading v${latestVersion}…`, clickable: false };
    case "ready":
      return { label: "Update ready — restart to apply", clickable: true };
    case "failed":
      return { label: `Update failed: ${flow.message}`, clickable: true };
  }
}

/**
 * One engine's `UpdateStatus` stream, shared: the strip subscribes wherever
 * it mounts and the store re-attaches per client. The engine replays the
 * current status on subscribe, so the strip appears the moment a newer
 * release is known.
 */
class UpdateStatusStore {
  #status: UpdateStatus | null = null;
  #handle: { cancel(): void } | null = null;
  readonly #listeners = new Set<() => void>();

  subscribe(listener: () => void): () => void {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  }

  getSnapshot(): UpdateStatus | null {
    return this.#status;
  }

  attach(client: EngineClient): void {
    if (this.#handle !== null) {
      return;
    }
    this.#handle = client.watch<UpdateStatus>(
      UPDATE_STATUS,
      {},
      {
        onItem: (status) => {
          this.#status = status;
          for (const listener of this.#listeners) {
            listener();
          }
        },
        onEnd: () => {},
      },
    );
  }

  detach(): void {
    this.#handle?.cancel();
    this.#handle = null;
    if (this.#status !== null) {
      this.#status = null;
      for (const listener of this.#listeners) {
        listener();
      }
    }
  }
}

export const updateStatusStore = new UpdateStatusStore();

/** The strip's read of the engine's status, as one value. */
function useUpdateStatus(): UpdateStatus | null {
  const session = useEngineSession();
  useEffect(() => {
    if (session === null) {
      return;
    }
    updateStatusStore.attach(session.client);
    return () => updateStatusStore.detach();
  }, [session]);
  return useSyncExternalStore(
    (listener) => updateStatusStore.subscribe(listener),
    () => updateStatusStore.getSnapshot(),
  );
}

export function UpdateStrip() {
  const status = useUpdateStatus();
  const dismissed = useSyncExternalStore(
    (listener) => uiSettings.subscribe(listener),
    () => uiSettings.getSnapshot().dismissedUpdateVersion,
  );
  if (status === null || !status.updateAvailable) {
    return null;
  }
  const latest = status.latestVersion ?? null;
  if (latest === null || latest === dismissed) {
    return null;
  }
  const { label } = updateStripLabel({ kind: "idle" }, latest, false);
  return (
    <button
      type="button"
      id="update-strip"
      className="update-strip"
      title="Dismiss this version"
      onClick={() => uiSettings.update({ dismissedUpdateVersion: latest }, "immediate")}
    >
      <span className="update-strip-label">{label}</span>
    </button>
  );
}
