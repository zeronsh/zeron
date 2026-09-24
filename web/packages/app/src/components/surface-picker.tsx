import { useMemo } from "react";
import { Icon, type IconName } from "@zeron/icons";
import { useEngineSession } from "../state/session-provider";
import { useFleetSnapshot } from "../state/fleet";
import { chatPageRow } from "../lib/view";
import { rightPaneStore } from "../state/right-pane";

/**
 * The right pane's empty state — the desktop's `render_surface_picker`
 * (`shell.rs:6594-6675`): a compact vertical list (the old two-card grid
 * clipped in narrow panes). Each row mints a tab; the file explorer is a
 * docked panel opened from its own toggle, not a central surface
 * (`2012bf29`).
 */

/** One add-surface affordance — shared by the picker and the `+` menu. */
export interface SurfaceChoice {
  readonly id: string;
  readonly icon: IconName;
  /** Verbatim from the Rust (`"Diffs"`, not `"Diff"`). */
  readonly label: string;
  readonly open: (chatId: string) => void;
  /** `space_git_detected()` gates the git surfaces on the desktop. */
  readonly needsGit: boolean;
}

/**
 * The picker's rows in desktop order (`shell.rs:6638-6672`) minus `Browser`
 * (desktop-only: no embedded web view on web) and `Files` (the docked
 * explorer column's own toggle owns it now). The `+` menu's rows are the
 * same list (`shell.rs:7136-7143`).
 */
export const SURFACE_CHOICES: readonly SurfaceChoice[] = [
  {
    id: "surface-card-terminal",
    icon: "terminal",
    label: "Terminal",
    needsGit: false,
    open: (chatId) => rightPaneStore.addTerminalSurface(chatId),
  },
  {
    id: "surface-card-diffs",
    icon: "list",
    label: "Diffs",
    needsGit: true,
    open: (chatId) => rightPaneStore.addDiffSurface(chatId, "diff"),
  },
  {
    id: "surface-card-history",
    icon: "gitBranch",
    label: "History",
    needsGit: true,
    open: (chatId) => rightPaneStore.addDiffSurface(chatId, "history"),
  },
];

/**
 * `space_git_detected()` — the desktop asks the space's checkout whether it
 * is a git repo. The web's honest proxy is the chat's resolved branch: the
 * engine only reports one for a git checkout. (A dedicated space query, if
 * one lands, replaces this without touching the callers.)
 */
export function useGitDetected(chatId: string): boolean {
  const session = useEngineSession();
  const snapshot = useFleetSnapshot();
  return useMemo(() => {
    if (!snapshot.chats.loaded) {
      return false;
    }
    const row = chatPageRow(
      chatId,
      snapshot.chats.rows,
      snapshot.spaces.rows,
      snapshot.statuses.rows,
      Date.now(),
      snapshot.devices.rows,
    );
    return (row?.chat.branch ?? null) !== null;
  }, [snapshot, chatId]);
}

/** The choices a given chat may open right now. */
export function surfaceChoices(gitDetected: boolean): readonly SurfaceChoice[] {
  return SURFACE_CHOICES.filter((choice) => !choice.needsGit || gitDetected);
}

export function SurfacePicker({ chatId }: { chatId: string }) {
  const gitDetected = useGitDetected(chatId);
  return (
    <div className="surface-picker" aria-label="Panel surfaces">
      <div className="surface-picker-list">
        {surfaceChoices(gitDetected).map((choice) => (
          <SurfacePickerRow key={choice.id} choice={choice} chatId={chatId} />
        ))}
      </div>
    </div>
  );
}

function SurfacePickerRow({ choice, chatId }: { choice: SurfaceChoice; chatId: string }) {
  return (
    <button
      type="button"
      id={choice.id}
      className="surface-picker-row"
      onClick={() => choice.open(chatId)}
    >
      <Icon name={choice.icon} size={15} />
      <span className="surface-picker-label">{choice.label}</span>
    </button>
  );
}

/** A centred muted placeholder body — real bodies land in tickets 22/24/25/26/27. */
export function SurfaceStubBody({ label }: { label: string }) {
  return (
    <div className="right-surface-stub" aria-label={label}>
      …
    </div>
  );
}
