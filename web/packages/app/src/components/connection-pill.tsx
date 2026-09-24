import { useEffect, useState } from "react";
import { useEngineSession } from "../state/session-provider";
import { useEngineStatus } from "../state/hooks";
import { useConnectionState } from "./connection-state";

/**
 * The sidebar's connection line — the desktop's `render_connection_pill`.
 *
 * Nothing renders while healthy: the pill only exists during a real outage.
 * No surface and no border (v0.2.12 feedback) — a bare spinner beside a
 * faint 11px caption while reconnecting, a 5px warning dot when the browser
 * says the OS is offline (the desktop's `ConnectivityState::Offline`, with
 * the same "sends are saved" string). The transport error itself belongs
 * in logs, not the sidebar, so only the state's label shows here; the main
 * card's banner carries the detail and the re-pair affordance, and a raw
 * "Attempt N" never reaches this copy.
 */
export function ConnectionPill() {
  const session = useEngineSession();
  const status = useEngineStatus(session);
  const state = useConnectionState(status);
  const [osOffline, setOsOffline] = useState(() => !navigator.onLine);

  useEffect(() => {
    const update = (): void => setOsOffline(!navigator.onLine);
    window.addEventListener("online", update);
    window.addEventListener("offline", update);
    return () => {
      window.removeEventListener("online", update);
      window.removeEventListener("offline", update);
    };
  }, []);

  if (session === null || (state.className === "conn-connected" && !osOffline)) {
    return null;
  }
  if (osOffline) {
    return (
      <div className="connection-pill" role="status">
        <span className="dot dot-offline" />
        <span className="connection-pill-label">Offline — sends are saved</span>
      </div>
    );
  }
  const spinning = state.className === "conn-connecting" || state.className === "conn-reconnecting";
  return (
    <div className="connection-pill" role="status">
      {spinning ? <span className="mono-spinner" /> : <span className={`dot ${state.dot}`} />}
      <span className="connection-pill-label">{state.label}</span>
    </div>
  );
}
