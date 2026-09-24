import type { ReactNode } from "react";

/**
 * The engine-failure gate — the desktop's `render_gate_card` +
 * `grid_backdrop` (`shell.rs:7223-7365`).
 *
 * `Failed(error)` renders the full-window card: the theme background, the
 * faint 44px grid fading toward the edges, the error copy at 14px muted, and
 * a bordered "Retry" button (hover takes the standard glass wash). The
 * content enters on the 500ms `FADE_IN` (EASE_OUT_EXPO, 4px rise), keyed per
 * phase on the desktop so every gate swap replays it.
 */

/**
 * The zeron `.bg-grid` port: 44px hairlines at `hairline(0.035)`. The
 * desktop approximates the original radial mask with four edge gradients
 * because gpui has no `mask-image`; the web uses the real radial mask.
 */
export function GridBackdrop() {
  return <div className="grid-backdrop" aria-hidden />;
}

export function GateCard({
  error,
  onRetry,
  children,
}: {
  error: string;
  onRetry: () => void;
  /** A secondary, web-only escape hatch under the Retry (e.g. Pair again). */
  children?: ReactNode;
}) {
  return (
    <div className="gate-card" role="alertdialog" aria-label="Engine unavailable">
      <GridBackdrop />
      <div className="gate-card-center">
        <div className="gate-card-content">
          <p className="gate-card-error">{error}</p>
          <button type="button" id="retry-engine" className="gate-retry" onClick={onRetry}>
            Retry
          </button>
          {children}
        </div>
      </div>
    </div>
  );
}
