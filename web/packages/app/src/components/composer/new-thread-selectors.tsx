import { useMemo, useState, useSyncExternalStore } from "react";
import type { Device, RepoRef, Space } from "@zeron/proto";
import { encodeScopedId } from "@zeron/engine-client";
import { useEngineSession } from "../../state/session-provider";
import { useNow } from "../../state/hooks";
import { useFleetSnapshot } from "../../state/fleet";
import { composerDefaults } from "../../lib/composer-draft";
import { spacesSorted } from "../../lib/view";
import { useSidebar } from "../../state/sidebar";
import { CheckoutChip, DeviceChip, ProjectChip, RefChip, type CheckoutKind } from "../composer-footer";

/**
 * The new-thread canvas's target rows — the desktop's
 * `pickers.rs::render_new_thread_target_selectors` (2426-2498, the floating
 * 20px row above the pill: device + project chips) and
 * `render_new_thread_git_selectors` (2502-2566, the footer slot's Layer A:
 * checkout + ref chips).
 *
 * The chips and their popovers are ticket 10's (exported from
 * `../composer-footer.tsx`); this file owns the ROWS that place them and the
 * canvas TARGET they read: the remembered device/project/no-project picks
 * (`composerDefaults` — the web peer of `restore_composer_target`,
 * state.rs:1283-1302) resolved through `effective_device_id`
 * (state.rs:1314-1320): the picked project's host when one is selected,
 * else the explicit device pick, else this device.
 */

/** `useSyncExternalStore` plumbing for the composer-defaults store. */
const subscribeDefaults = (listener: () => void) => composerDefaults.subscribe(listener);
const getDefaults = () => composerDefaults.getSnapshot();

/** The new-chat canvas's resolved run target. */
export interface NewThreadTarget {
  readonly devices: readonly Device[];
  readonly spaces: readonly Space[];
  readonly ownDeviceId: string | null;
  /** The picked space row, or null ("no project" / nothing remembered). */
  readonly space: Space | null;
  /** The device that runs the agents for this target. */
  readonly effectiveDevice: Device | null;
  readonly effectiveDeviceId: string | null;
  /**
   * Catalogs and refs come from the device that RUNS the agents — the
   * space's device when it differs from the connected engine's own.
   */
  readonly targetDeviceId: string | null;
}

/**
 * Resolve the canvas target: the remembered picks projected onto the live
 * device/space rows. The remembered project wins; with nothing remembered,
 * the SIDEBAR's space pick (the filter, else the last space) stands in —
 * the desktop's `selected_space` is one field shared by the sidebar filter
 * and the canvas (`land_in_space` routes here after creating one).
 * `effective_device_id` (state.rs:1314-1320): the space's host, else the
 * device pick, else the connected engine's own device.
 */
export function useNewThreadTarget(): NewThreadTarget {
  const session = useEngineSession();
  // The MERGED fleet snapshot: the canvas's device/space pickers span every
  // engine's scoped rows; the composer's calls go through the routed
  // session (the picked space's engine, else the active engine).
  const snapshot = useFleetSnapshot();
  const defaults = useSyncExternalStore(subscribeDefaults, getDefaults, getDefaults);
  const sidebar = useSidebar();

  return useMemo(() => {
    const devices = snapshot?.devices.rows ?? EMPTY_DEVICES;
    const spaces = spacesSorted(snapshot?.spaces.rows ?? EMPTY_SPACES);
    // `restore_composer_target`: the pick survives only while the row does.
    const fallback = sidebar.spaceFilter ?? sidebar.lastSpaceId;
    const projectId = defaults.noProject ? null : (defaults.project ?? fallback);
    const space = projectId === null ? null : spaces.find((row) => row.id === projectId) ?? null;
    // The routed engine's own device, SCOPED to match the merged rows.
    const ownRawDeviceId = session?.client.engineInfo?.deviceId ?? null;
    const own =
      session !== null && ownRawDeviceId !== null
        ? encodeScopedId(session.engine.baseUrl, ownRawDeviceId)
        : null;
    const effectiveDeviceId = space?.deviceId ?? defaults.device ?? own;
    const effectiveDevice = devices.find((device) => device.id === effectiveDeviceId) ?? null;
    const targetDeviceId =
      space !== null && own !== null && space.deviceId !== own ? space.deviceId : null;
    return { devices, spaces, ownDeviceId: own, space, effectiveDevice, effectiveDeviceId, targetDeviceId };
    // `defaults` is a cached snapshot object; the memo keys on its identity,
    // which changes only when a pick lands.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [snapshot?.devices.rows, snapshot?.spaces.rows, defaults, ownDeviceKey(session), sidebar.spaceFilter, sidebar.lastSpaceId]);
}

/** The routed session's own (scoped) device id as a memo key. */
function ownDeviceKey(session: ReturnType<typeof useEngineSession>): string | null {
  const deviceId = session?.client.engineInfo?.deviceId ?? null;
  return session !== null && deviceId !== null
    ? encodeScopedId(session.engine.baseUrl, deviceId)
    : null;
}

const EMPTY_DEVICES: readonly Device[] = [];
const EMPTY_SPACES: readonly Space[] = [];

/**
 * `render_new_thread_target_selectors` (pickers.rs:2426-2498): a `flex_none`
 * row, gap 4, with two footer chips in order — the DEVICE chip (monitor
 * icon, label = device name or "This device", warning tint when offline,
 * popover 224) and the PROJECT chip (folder icon, label = space display
 * name or "No project", popover 280, right-aligned by the row's
 * justify-end).
 */
export function NewThreadTargetSelectors() {
  const target = useNewThreadTarget();
  const now = useNow(30_000);

  return (
    <div className="new-thread-target-selectors">
      <DeviceChip
        devices={target.devices}
        effectiveDevice={target.effectiveDevice}
        ownDeviceId={target.ownDeviceId}
        now={now}
        fallbackLabel="This device"
      />
      <ProjectChip
        spaces={target.spaces}
        currentSpaceId={target.space?.id ?? null}
        fallbackLabel="No project"
      />
    </div>
  );
}

/**
 * `render_new_thread_git_selectors` (pickers.rs:2502-2566): the footer
 * slot's Layer A — a `w_full min_w_0` flex row, gap 4, with the
 * checkout-kind chip (popover 224) and the branch chip (popover 320).
 * Renders NOTHING when the target space has no git (the row collapses
 * with it — the slot's height is `SESSION_FOOTER_HEIGHT × bottom_slot`).
 */
export function NewThreadGitSelectors() {
  const session = useEngineSession();
  const target = useNewThreadTarget();
  const space = target.space;
  // Draft picks for the git row — refs are fixed once the chat runs.
  const [draftBranch, setDraftBranch] = useState<string | null>(null);
  const [checkout, setCheckout] = useState<CheckoutKind>("local");
  const [refs, setRefs] = useState<readonly RepoRef[]>([]);

  if (space === null || !space.gitDetected) {
    return null;
  }
  const repoPath = space.path;
  const picked = draftBranch;
  const pickedRefHasWorktree =
    picked !== null &&
    refs.some((row) => row.name === picked && row.worktreePath !== null && row.worktreePath !== undefined);

  return (
    <div className="new-thread-git-selectors">
      <CheckoutChip
        checkout={checkout}
        pickedRefHasWorktree={pickedRefHasWorktree}
        onPick={(kind) => {
          setCheckout(kind);
          // Picking Local from NewWorktree with a non-current plain ref
          // picked drops the branch override — the current branch takes
          // over (pickers.rs:1359-1373).
          if (
            kind === "local" &&
            checkout === "newWorktree" &&
            !pickedRefHasWorktree &&
            picked !== null &&
            !refs.some((row) => row.name === picked && row.current)
          ) {
            setDraftBranch(null);
          }
        }}
      />
      <RefChip
        session={session}
        repoPath={repoPath}
        currentBranch={null}
        draftBranch={draftBranch}
        checkout={checkout}
        targetDeviceId={target.targetDeviceId}
        canPick={session !== null}
        onPick={(name) => setDraftBranch(name)}
        onRefs={setRefs}
      />
    </div>
  );
}
