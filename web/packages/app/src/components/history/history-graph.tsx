import { useMemo } from "react";
import { useAppearance, useSystemAppearance } from "../../state/appearance";
import {
  graphColor,
  laneX,
  HISTORY_ROW_HEIGHT,
  HISTORY_GRAPH_ROW_OVERLAP,
  type GraphGeometry,
  type GraphRow,
} from "../../lib/git-history";

/**
 * The History graph's SVG lane canvas — the port of the desktop's
 * `graph_paths` `canvas()` (history.rs:3311-3530). One absolutely
 * positioned `<svg>` behind the row list; one `<path>` per lane COLOR
 * (grouping every segment that color owns). The browser's own
 * `overflow: hidden` clipping replaces the desktop's manual culling loop,
 * and CSS transitions on `stroke`/`stroke-width` replace its 2-pass
 * hover-focus dimming.
 *
 * The palette (`history.rs:3312-3319`): the 6 theme roles, cycled by
 * `colorId % 6`, each through `graphColor` (HSL saturation × 0.72).
 */

/** The lane palette's `--rb-*` role tokens, in desktop order. */
const PALETTE_TOKENS = [
  "--rb-accent",
  "--rb-activity",
  "--rb-success",
  "--rb-warning",
  "--rb-danger",
  "--rb-text-muted",
] as const;

/**
 * Read the 6-color lane palette off the installed variant. Re-read on any
 * appearance change (the same contract `terminal/theme.ts` uses). Outside a
 * document (SSR) the tokens resolve at paint time instead.
 */
export function useGraphPalette(): readonly string[] {
  const appearance = useAppearance();
  const system = useSystemAppearance();
  return useMemo(() => {
    if (typeof document === "undefined") {
      return PALETTE_TOKENS.map((token) => `var(${token})`);
    }
    const style = getComputedStyle(document.documentElement);
    const colors = PALETTE_TOKENS.map((token) => style.getPropertyValue(token).trim());
    return colors.map((color) => (color.length > 0 ? graphColor(color) : `var(${PALETTE_TOKENS[0]})`));
  }, [appearance, system]);
}

interface GraphPathsProps {
  readonly rows: readonly GraphRow[];
  readonly geometry: GraphGeometry;
  /** The hovered lane color, or null — drives the focus crossfade. */
  readonly hoveredColorId: number | null;
  /** Branch-tips view never paints the compact rail (false adjacencies). */
  readonly railMode: boolean;
  readonly palette: readonly string[];
  readonly contentHeight: number;
}

/**
 * The lane canvas — every visible row's segments as SVG paths, grouped per
 * colorId. In compact rail mode each row instead paints one vertical
 * segment down the rail x from its center to the next row's.
 */
export function HistoryGraph({
  rows,
  geometry,
  hoveredColorId,
  railMode,
  palette,
  contentHeight,
}: GraphPathsProps) {
  const paths = useMemo(() => {
    const byColor = new Map<number, string[]>();
    const push = (colorId: number, d: string): void => {
      const bucket = byColor.get(colorId);
      if (bucket === undefined) {
        byColor.set(colorId, [d]);
      } else {
        bucket.push(d);
      }
    };

    if (geometry.compact && railMode) {
      const railX = laneX(geometry, 0);
      for (let index = 0; index < rows.length; index += 1) {
        const row = rows[index]!;
        const y = index * HISTORY_ROW_HEIGHT;
        const endY =
          index + 1 < rows.length ? (index + 1) * HISTORY_ROW_HEIGHT + HISTORY_ROW_HEIGHT / 2 : y + HISTORY_ROW_HEIGHT;
        push(row.nodeColorId, `M ${railX} ${y + HISTORY_ROW_HEIGHT / 2} L ${railX} ${endY}`);
      }
    } else {
      const middle = HISTORY_ROW_HEIGHT / 2;
      for (let index = 0; index < rows.length; index += 1) {
        const row = rows[index]!;
        const y = index * HISTORY_ROW_HEIGHT;
        for (const segment of row.segments) {
          const fromX = laneX(geometry, segment.fromLane);
          const toX = laneX(geometry, segment.toLane);
          switch (segment.shape) {
            case "incoming":
              push(
                segment.colorId,
                `M ${fromX} ${y - HISTORY_GRAPH_ROW_OVERLAP} C ${fromX} ${y + middle * 0.55} ${toX} ${y + middle * 0.55} ${toX} ${y + middle}`,
              );
              break;
            case "outgoing":
              push(
                segment.colorId,
                `M ${fromX} ${y + middle} C ${fromX} ${y + middle * 1.45} ${toX} ${y + middle * 1.45} ${toX} ${y + HISTORY_ROW_HEIGHT + HISTORY_GRAPH_ROW_OVERLAP}`,
              );
              break;
            case "through":
              if (segment.fromLane === segment.toLane) {
                push(segment.colorId, `M ${fromX} ${y - HISTORY_GRAPH_ROW_OVERLAP} L ${toX} ${y + HISTORY_ROW_HEIGHT + HISTORY_GRAPH_ROW_OVERLAP}`);
              } else {
                push(
                  segment.colorId,
                  `M ${fromX} ${y - HISTORY_GRAPH_ROW_OVERLAP} C ${fromX} ${y + middle} ${toX} ${y + middle} ${toX} ${y + HISTORY_ROW_HEIGHT + HISTORY_GRAPH_ROW_OVERLAP}`,
                );
              }
              break;
          }
        }
      }
    }
    return [...byColor.entries()].map(([colorId, segments]) => ({
      colorId,
      d: segments.join(" "),
    }));
  }, [rows, geometry, railMode]);

  return (
    <svg
      className="history-graph"
      width={geometry.width}
      height={Math.max(contentHeight, 1)}
      viewBox={`0 0 ${geometry.width} ${Math.max(contentHeight, 1)}`}
      aria-hidden="true"
      focusable="false"
    >
      {paths.map(({ colorId, d }) => {
        const color = palette[colorId % palette.length] ?? palette[0]!;
        const focused = hoveredColorId === colorId;
        const dimmed = hoveredColorId !== null && !focused;
        return (
          <path
            key={colorId}
            className={`history-graph-line ${focused ? "history-graph-line-focused" : ""}`}
            d={d}
            style={{
              stroke: dimmed ? `color-mix(in srgb, ${color} 24%, var(--rb-bg))` : color,
              strokeWidth: focused ? 2.25 : 1.5,
            }}
          />
        );
      })}
    </svg>
  );
}
