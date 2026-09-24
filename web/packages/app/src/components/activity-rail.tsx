import { Icon } from "@zeron/icons";
import { toolIconName, type ToolItem } from "../lib/transcript";
import {
  ACTIVITY_GUTTER_WIDTH,
  ACTIVITY_ICON_LEFT,
  ACTIVITY_ICON_SIZE,
  railPath,
  toolConnectorParts,
} from "../lib/tool-motion";

/**
 * The activity rail — the task tree's gutter, the desktop's `activity_rail`
 * (transcript.rs:7227). One 48px column per chip row: an SVG canvas whose
 * single path unions the incoming trunk and the elbow branch (non-zero fill,
 * exactly the desktop's one-fill union — stroke tessellation would
 * double-blend the fork), plus the tool's glyph at the branch tip.
 *
 * The previous row draws the first leg of a new arrival to its lower
 * boundary; this row then continues down, rounds the elbow, and finally
 * reveals the icon (`opacity(branchReveal)`).
 */

export interface ActivityRailProps {
  readonly tool: ToolItem;
  /** Row *ix* > 0 — the incoming trunk has a predecessor to continue from. */
  readonly hasPredecessor: boolean;
  /** Another row follows — the trunk continues past this row's bend. */
  readonly continues: boolean;
  /** `tool_connector_reveal_progress` for this row. */
  readonly connectorReveal: number;
  /** The NEXT row's connector progress, driving this row's continuation. */
  readonly continuationReveal: number;
  /** The row's compact height — the elbow's band (TOOL_TREE_ROW_HEIGHT). */
  readonly bendRowHeight: number;
  /** The row's FULL height (an expanded detail body stretches the trunk). */
  readonly canvasHeight: number;
}

export function ActivityRail({
  tool,
  hasPredecessor,
  continues,
  connectorReveal,
  continuationReveal,
  bendRowHeight,
  canvasHeight,
}: ActivityRailProps) {
  const { branch } = toolConnectorParts(connectorReveal, hasPredecessor);
  const d = railPath({ bendRowHeight, canvasHeight, hasPredecessor, continues, connectorReveal, continuationReveal });
  return (
    <div className="activity-rail" aria-hidden>
      <svg
        className="activity-rail-canvas"
        width={ACTIVITY_GUTTER_WIDTH}
        height={canvasHeight}
        viewBox={`0 0 ${ACTIVITY_GUTTER_WIDTH} ${canvasHeight}`}
      >
        {d !== null && <path className="activity-rail-path" d={d} fillRule="nonzero" />}
      </svg>
      <span
        className={`activity-rail-glyph ${tool.isError ? "activity-rail-glyph-error" : ""}`}
        style={{ left: ACTIVITY_ICON_LEFT, top: bendRowHeight / 2 - ACTIVITY_ICON_SIZE / 2, opacity: branch }}
      >
        <Icon
          name={tool.isThought ? "chatRoundLine" : toolIconName(tool.call)}
          size={ACTIVITY_ICON_SIZE}
        />
      </span>
    </div>
  );
}
