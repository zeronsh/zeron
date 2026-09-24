import { useEffect, useRef, useState } from "react";
import { Icon } from "@zeron/icons";
import { methods } from "@zeron/engine-client";
import type { Device } from "@zeron/proto";
import { useEngineSession } from "../state/session-provider";
import { useFleet } from "../state/fleet";
import { useEngineStatus, useNow, useWatchSnapshot } from "../state/hooks";
import {
  BtnGhost,
  BtnPrimary,
  Dialog,
  DialogCard,
  DialogField,
  DialogTitle,
} from "../components/ui/Dialog";
import {
  lastSeenOnline,
  formatLastSeenAt,
  platformGlyph,
  platformLabel,
  presenceDot,
  shortId,
  type EngineConnection,
} from "../lib/devices";

/**
 * Devices settings (desktop settings/devices.rs parity): the device registry
 * of the connected engine — one row per device that has paired with it, with
 * the platform tile's corner presence dot, the meta line (platform · version
 * · connection · last seen · added · the click-to-copy id chip), Rename (via
 * the Mutate renameDevice op) and the connect card that opens the sign-in
 * flow at `/pair` — the page's one entry point for adding engines.
 *
 * One row per engine, period: the legacy "Engines" card (the pairing-era
 * drawer row ticket 45 folded in above the device rows) listed the fleet's
 * engines again, so every engine appeared twice — under the WorkOS
 * browser-session fleet the two lists describe the same engines, and the
 * device rows are the registry of record. They come from the connected
 * engine's WatchDevices; the "engine-backed" row is the one whose id
 * matches the connected engine's own device (its presence is the live
 * connection state), a row matching a parked fleet engine is
 * engine-backed-off; every other row falls back to the last-seen window.
 */

interface RenameDialog {
  readonly deviceId: string;
  readonly name: string;
}

export function DevicesSettingsPage() {
  const session = useEngineSession();
  const client = session?.client ?? null;
  const status = useEngineStatus(session);
  const fleet = useFleet();
  const snapshot = useWatchSnapshot(session);
  const now = useNow(15_000);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState<string | null>(null);
  const [rename, setRename] = useState<RenameDialog | null>(null);
  const copyTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  // A new engine (a successful sign-in) rebuilds the session; reset the page.
  useEffect(() => {
    setError(null);
    setCopied(null);
    setRename(null);
  }, [session]);

  useEffect(() => {
    return () => {
      if (copyTimer.current !== null) {
        clearTimeout(copyTimer.current);
      }
    };
  }, []);

  const devices = snapshot?.devices.rows ?? [];
  const localDeviceId = session?.client.engineInfo?.deviceId ?? null;

  /** The live engine connection behind a row, or null (last-seen fallback). */
  function rowConnection(deviceId: string): EngineConnection | null {
    if (deviceId === localDeviceId) {
      if (status === null) {
        return null;
      }
      switch (status.state) {
        case "connected":
          return "connected";
        case "connecting":
        case "reconnecting":
          return "reconnecting";
        default:
          return "off";
      }
    }
    // A row backed by a parked engine this client knows: engine-backed, off.
    const parked = fleet.engines.find(
      (engine) => engine.deviceId === deviceId && engine.baseUrl !== fleet.active,
    );
    return parked === undefined ? null : "off";
  }

  /** The fleet engine a non-local engine-backed row forgets from, if any. */


  /** The dialog closes FIRST, then the trimmed name decides whether an RPC fires (submit_rename's silent-swallow quirk). */
  function submitRename(name: string) {
    const deviceId = rename?.deviceId;
    setRename(null);
    const trimmed = name.trim();
    if (deviceId === undefined || trimmed.length === 0 || client === null) {
      return;
    }
    void (async () => {
      try {
        await client.call(methods.MUTATE, { op: "renameDevice", deviceId, name: trimmed });
      } catch (cause) {
        setError(`Rename failed: ${cause instanceof Error ? cause.message : String(cause)}`);
      }
    })();
  }

  function copyId(deviceId: string) {
    void navigator.clipboard.writeText(deviceId).catch(() => {});
    setCopied(deviceId);
    if (copyTimer.current !== null) {
      clearTimeout(copyTimer.current);
    }
    copyTimer.current = setTimeout(() => setCopied(null), 1500);
  }

  const count = devices.length;

  return (
    <div className="settings-page">
      <h1 className="settings-title">
        Devices{count > 0 && <span className="settings-title-count">{count}</span>}
      </h1>
      <p className="settings-subtitle">Connect and manage engines.</p>

      {error !== null && (
        <p className="error-strip" role="alert" onClick={() => setError(null)}>
          {error}
        </p>
      )}

      <section className="settings-card settings-pairing-box">
        <p className="settings-pairing-hint">
          Engines appear here automatically: sign in with the same WorkOS account you use for{" "}
          <code>zeron login</code> on the engine, and every connected device joins this list.
        </p>
      </section>

      <section className="settings-card">
        {count === 0 ? (
          <p className="settings-empty settings-empty-devices">No devices registered</p>
        ) : (
          devices.map((device, ix) => (
            <DeviceRow
              key={device.id}
              device={device}
              first={ix === 0}
              isLocal={device.id === localDeviceId}
              connection={rowConnection(device.id)}
              online={lastSeenOnline(device.lastSeenAt, now)}
              copied={copied === device.id}
              now={now}
              onCopyId={() => copyId(device.id)}
              onRename={() => setRename({ deviceId: device.id, name: device.name })}
            />
          ))
        )}
      </section>

      {rename !== null && (
        <RenameDeviceDialog dialog={rename} onCancel={() => setRename(null)} onSubmit={submitRename} />
      )}
    </div>
  );
}

function DeviceRow(props: {
  readonly device: Device;
  readonly first: boolean;
  readonly isLocal: boolean;
  readonly connection: EngineConnection | null;
  readonly online: boolean;
  readonly copied: boolean;
  readonly now: number;
  readonly onCopyId: () => void;
  readonly onRename: () => void;
}) {
  const device = props.device;
  const dot = presenceDot(props.connection, props.online);
  const version = device.version !== null && device.version !== undefined && device.version.length > 0
    ? device.version
    : null;
  return (
    <div className="settings-row device-row">
      <div className="row-tile device-tile" aria-hidden="true">
        <Icon name={platformGlyph(device.platform)} size={16} className="row-tile-icon" />
        <span className={`presence-dot presence-${dot}`} />
      </div>
      <div className="settings-row-main">
        <span className="settings-row-title">{device.name}</span>
        <span className="settings-meta-line">
          {platformLabel(device.platform)}
          {version !== null && (
            <>
              <span className="settings-meta-dot" aria-hidden="true">·</span>
              {`v${version}`}
            </>
          )}
          {props.connection !== null && (
            <>
              <span className="settings-meta-dot" aria-hidden="true">·</span>
              {props.connection === "connected"
                ? "Connected"
                : props.connection === "reconnecting"
                  ? "Reconnecting"
                  : "Off"}
            </>
          )}
          {!props.online && (
            <>
              <span className="settings-meta-dot" aria-hidden="true">·</span>
              {`Last seen ${formatLastSeenAt(device.lastSeenAt, props.now)}`}
            </>
          )}
          {device.createdAt !== null && device.createdAt !== undefined && (
            <>
              <span className="settings-meta-dot" aria-hidden="true">·</span>
              {`Added ${formatLastSeenAt(device.createdAt, props.now)}`}
            </>
          )}
          <span className="settings-meta-dot" aria-hidden="true">·</span>
          <button
            type="button"
            className={`id-chip ${props.copied ? "id-chip-copied" : ""}`}
            onClick={props.onCopyId}
            aria-label={`Copy device id ${device.id}`}
          >
            {props.copied ? "Copied" : shortId(device.id)}
          </button>
        </span>
      </div>
      {props.isLocal && <span className="badge">This device</span>}
      <button type="button" className="btn btn-ghost device-rename" onClick={props.onRename}>
        <Icon name="pen" size={14} />
        Rename
      </button>
    </div>
  );
}

/**
 * The rename dialog (devices.rs render_rename_dialog): the shared
 * `ui/Dialog` family — `RbDialog`'s scrim swallows presses without
 * dismissing (its `disablePointerDismissal` carries the old hand-roll's
 * parity quirk for free) and the phone arm is the family's bottom sheet
 * (the old fixed-centered card squashed at ≤768px). Cancel, Rename, and
 * Enter close it, as before; Escape now cancels too — the one gained
 * path, matching the shared dialogs (documented deviation). Exported
 * for the mounted family test (tests/settings-dialogs.test.ts).
 */
export function RenameDeviceDialog(props: {
  readonly dialog: RenameDialog;
  readonly onCancel: () => void;
  readonly onSubmit: (name: string) => void;
}) {
  const [name, setName] = useState(props.dialog.name);
  const inputRef = useRef<HTMLInputElement | null>(null);
  return (
    <Dialog ariaLabel="Rename device" onClose={props.onCancel} initialFocus={inputRef}>
      <DialogCard>
        <DialogTitle>Rename device</DialogTitle>
        <form
          className="dialog-form-rows"
          onSubmit={(event) => {
            event.preventDefault();
            props.onSubmit(name);
          }}
        >
          <DialogField>
            <input
              ref={inputRef}
              type="text"
              placeholder="Device name"
              value={name}
              onChange={(event) => setName(event.target.value)}
              autoComplete="off"
              aria-label="Device name"
            />
          </DialogField>
          <div className="dialog-actions-row">
            <BtnGhost type="button" onClick={props.onCancel}>
              Cancel
            </BtnGhost>
            <BtnPrimary type="submit">Rename</BtnPrimary>
          </div>
        </form>
      </DialogCard>
    </Dialog>
  );
}
