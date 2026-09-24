import { useEffect } from "react";
import { useNavigate } from "@tanstack/react-router";
import { useFleet, useFleetRegistry } from "../state/fleet";

/**
 * The connect gate. Nothing here is manual: the fleet module's boot already
 * checked the browser session and redirected to WorkOS when signed out, so
 * this page is what a signed-in visitor sees while the edge reports no
 * connected engines. Start `zeron` on a machine signed in to the same
 * WorkOS account and the device joins the fleet — this returns home.
 */
export function PairPage() {
  const fleet = useFleet();
  const registry = useFleetRegistry();
  const navigate = useNavigate();
  const live = registry.engines.length > 0;

  useEffect(() => {
    if (live) {
      void navigate({ to: "/", replace: true });
    }
  }, [live, navigate]);

  return (
    <main className="pair-page">
      <h1>Connecting</h1>
      {fleet.configurationError !== null ? (
        <p className="form-error">{fleet.configurationError}</p>
      ) : (
        <p className="pair-status">
          Waiting for your engines. Start <code>zeron</code> on a machine signed in to the same
          WorkOS account — it appears here automatically.
        </p>
      )}
    </main>
  );
}
