import { Outlet, useRouterState } from "@tanstack/react-router";
import { Link } from "@tanstack/react-router";
import { EngineSessionProvider, useEngineRetry, useEngineSession } from "../state/session-provider";
import { useFleet, useFleetRegistry } from "../state/fleet";
import { useEngineStatus } from "../state/hooks";
import { GateCard } from "../components/gate-card";
import { useConnectionState } from "../components/connection-state";

/**
 * The root route: the engine session provider, then the gate/page split —
 * the desktop's `GatePhase` (`shell.rs:238-240`), generalized to the fleet:
 *
 * - **Ready** — the page, wrapped in the one keyed `fade_in("phase-app")`
 *   entrance: 500ms `EASE_OUT_EXPO`, opacity 0→1 with a 4px rise, replayed
 *   whenever the app recovers from a gate. ANY engine connecting (or the
 *   offline cache seeding rows) ends the Loading phase — the desktop's
 *   `Ready` persists through registry-level reconnects the same way, and
 *   remote engines' outages are per-entry badges, never a gate.
 * - **Failed** — EVERY engine parked (revoked credential / identity
 *   changed): nothing can connect, so the gate card owns the screen. A
 *   single parked engine while others are live is NOT a gate — its rows
 *   keep rendering from cache and the shell banner carries the state.
 * - **Loading** — the first dial is in flight and nothing has connected or
 *   seeded yet: a plain empty root, no overlay (the boot splash is a
 *   deliberate open question and is NOT built here).
 *
 * `/pair` always shows the page: it is the one route that can FIX a dead
 * pairing, so the gate must never cover it.
 */
export function RootLayout() {
  return (
    <EngineSessionProvider>
      <GateAndPage />
    </EngineSessionProvider>
  );
}

function GateAndPage() {
  const session = useEngineSession();
  const status = useEngineStatus(session);
  const state = useConnectionState(status);
  const fleet = useFleet();
  const registry = useFleetRegistry();
  const retry = useEngineRetry();
  const onPair = useRouterState({ select: (s) => s.location.pathname === "/pair" });

  const engines = registry.engines;
  const paired = fleet.engines.length > 0;
  const allOff = paired && engines.length > 0 && engines.every((engine) => engine.state === "off");
  const anythingLive =
    engines.some((engine) => engine.state === "connected" || engine.chats.loaded || engine.spaces.loaded);

  const phase = paired && !onPair ? (allOff ? "failed" : anythingLive ? "ready" : "loading") : "ready";

  if (phase === "loading") {
    // GatePhase::Loading — the desktop renders ONLY the root (plus a splash
    // this port deliberately defers): nothing to see, nothing to click.
    return <div className="gate-loading" />;
  }
  if (phase === "failed") {
    const error = engines.find((engine) => engine.state === "off" && engine.lastError !== null)?.lastError
      ?? engines[0]?.lastError
      ?? state.detail
      ?? state.label;
    return (
      <GateCard error={error ?? "Engine connection failed"} onRetry={retry}>
        {/*
          Web-only escape hatch, flagged in the ticket's Comments: a parked
          credential cannot be fixed by retrying, and the gate covers every
          route that could re-pair — without this link the dead session
          would strand the browser.
        */}
        <Link to="/pair" className="gate-pair-link">
          Pair again
        </Link>
      </GateCard>
    );
  }
  return (
    <div className="page-fade" key={phase}>
      <Outlet />
    </div>
  );
}
