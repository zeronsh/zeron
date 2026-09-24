import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Icon } from "@zeron/icons";
import { encodeScopedId, methods } from "@zeron/engine-client";
import type { ChangeRequestSummary, ContextUsage, Device, RepoRef, Space } from "@zeron/proto";
import { useEngineSession } from "../state/session-provider";
import { useNow } from "../state/hooks";
import { useFleetSnapshot } from "../state/fleet";
import { deviceOnline, spaceDisplayName, spacesSorted } from "../lib/view";
import { filterIndices } from "../lib/picker-search";
import { addSpaceStore } from "../state/add-space";
import { composerDefaults, rememberNoProject, rememberTarget } from "../lib/composer-draft";
import { sidebarStore } from "../state/sidebar";
import { ContextUsageIndicator, hasWindow } from "./context-usage";
import { ChangeRequestBadge } from "./change-request-badge";
import { FooterChip, FooterLabel } from "./ui/Chip";
import { PickerSearchField, useCursorList } from "./ui/CursorList";
import { MenuRowNav } from "./ui/MenuRows";
import { PickerCard } from "./ui/PickerCard";
import { ErrorRow, SkeletonRows } from "./ui/Skeleton";

/**
 * The session footer row — the desktop's `workspace_footer_row`
 * (`pickers.rs:2341-2422`, SESSION_FOOTER_HEIGHT 24). Ticket 15 slots this
 * row into the 24px footer slot's LAYER B: the new-thread canvas's Layer A
 * (checkout + ref chips for the run target) lives in
 * `./composer/new-thread-selectors.tsx`, and the composer renders the slot
 * with the two absolutely-inset layers cross-faded by the dock's
 * `selectors`/`footer` channels — never both mounted at once
 * (`route_chrome_opacities` guarantees one is exactly 0).
 *
 * While a chat has neither a persisted `ChatConfig` nor a stamped branch
 * (the web's closest analogue of the desktop's draft canvas), the row
 * carries the four draft chips: device and project (the run target, writing
 * the remembered defaults) and checkout kind + ref (the git target,
 * `SwitchRef` executing against the space folder for a plain non-current
 * ref). Once the chat is committed, the same slots become read-only
 * `FooterLabel`s: a git ref is fixed at creation, so the desktop never
 * offers a picker there. The trailing cluster (change-request badge + usage
 * indicator) belongs to both variants; the row's geometry is ticket 13's.
 *
 * The four chip components are exported: the new-thread selector rows mount
 * the SAME chips (ticket 10 owns their cards; the rows only place them).
 *
 * Each chip's card is one `PickerCard` over the base `RbPopover` layer (the
 * trigger's `trigger-press` reason replaces the old noteTriggerPress dance;
 * pressing another chip dismisses the first popover and opens that chip's
 * own — the four-chip switching behavior). All four register the
 * `composer-pickers` overlayKeyboard source while open, keeping session-nav
 * shortcuts quiet under any of them — the desktop's
 * `composer.pickers().is_open()` covers the footer pickers too
 * (shell.rs:3681-3683).
 */

/** `MAX_REF_ROWS` (pickers.rs) — the ref list's cap, surfaced as "Showing X of Y". */
const MAX_REF_ROWS = 300;

/** The checkout-kind pair shared by the footer's draft row and the canvas's git selectors. */
export type CheckoutKind = "local" | "newWorktree";

export interface ComposerFooterProps {
  readonly chat: {
    readonly id: string;
    readonly branch: string | null;
    readonly config: unknown;
    readonly spaceId?: string | null;
    readonly cwd: string | null;
  };
  readonly crSummary: ChangeRequestSummary | null;
  readonly contextUsage: ContextUsage | null;
}

export function ComposerFooter({ chat, crSummary, contextUsage }: ComposerFooterProps) {
  const session = useEngineSession();
  // The MERGED fleet snapshot: the footer's device/space lookups read the
  // scoped rows of every engine; the RPCs below go through the routed
  // session's client (the chat's owning engine).
  const snapshot = useFleetSnapshot();
  const now = useNow(30_000);

  const devices = snapshot?.devices.rows ?? EMPTY_DEVICES;
  const spaces = useMemo(() => spacesSorted(snapshot?.spaces.rows ?? EMPTY_SPACES), [snapshot?.spaces.rows]);
  const space =
    chat.spaceId === null || chat.spaceId === undefined
      ? null
      : spaces.find((row) => row.id === chat.spaceId) ?? null;
  // The routed engine's own device, SCOPED — it must compare against the
  // merged rows' scoped device ids.
  const ownRawDeviceId = session?.client.engineInfo?.deviceId ?? null;
  const ownDeviceId =
    session !== null && ownRawDeviceId !== null
      ? encodeScopedId(session.engine.baseUrl, ownRawDeviceId)
      : null;
  const effectiveDeviceId = space?.deviceId ?? ownDeviceId;
  const effectiveDevice = devices.find((device) => device.id === effectiveDeviceId) ?? null;
  // Catalogs and refs come from the device that RUNS the agents — the
  // space's device when it differs from the connected engine's own.
  const targetDeviceId =
    space !== null && ownDeviceId !== null && space.deviceId !== ownDeviceId ? space.deviceId : null;

  const committed = chat.config !== null || chat.branch !== null;
  // Draft picks for the git row (refs are fixed once the chat runs).
  const [draftBranch, setDraftBranch] = useState<string | null>(null);
  const [checkout, setCheckout] = useState<CheckoutKind>("local");
  // The loaded refs, lifted so the checkout chip can read "Current worktree"
  // off the picked ref (`checkout_label`, pickers.rs:1280-1304).
  const [refs, setRefs] = useState<readonly RepoRef[]>([]);

  const spacePath = space?.path ?? null;
  const gitDetected = space?.gitDetected ?? false;
  const canSwitch = !committed && spacePath !== null && session !== null;
  const picked = draftBranch ?? chat.branch;
  const pickedRefHasWorktree =
    picked !== null && refs.some((row) => row.name === picked && row.worktreePath !== null && row.worktreePath !== undefined);

  // `render_footer`'s established-chat branch (pickers.rs:2571-2660): the
  // checkout-kind label reads the space row — "Worktree" when the chat's
  // cwd differs from the space path, else "Local checkout" — and the whole
  // label row renders only when the chat's space has git detected. The
  // trailing cluster (spring, CR badge, usage) belongs to both variants.
  const chatCwd = typeof chat.cwd === "string" ? chat.cwd : null;
  const isWorktree = chatCwd !== null && spacePath !== null && chatCwd !== spacePath;

  return (
    <div className={`composer-footer ${committed ? "" : "composer-footer-draft"}`}>
      {/*
        Layer B's row (the 24px slot wrapper itself is the composer's — see
        composer.tsx; this element is the absolutely-inset session layer's
        content).
      */}
      {committed ? (
        gitDetected ? (
          <>
            <FooterLabel
              icon={isWorktree ? "folderWithFiles" : "folder"}
              label={isWorktree ? "Worktree" : "Local checkout"}
            />
            <FooterLabel icon="gitBranch" label={chat.branch ?? "No ref"} />
          </>
        ) : null
      ) : (
        <>
          <DeviceChip devices={devices} effectiveDevice={effectiveDevice} ownDeviceId={ownDeviceId} now={now} />
          <ProjectChip spaces={spaces} currentSpaceId={space?.id ?? null} />
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
            repoPath={spacePath}
            currentBranch={chat.branch}
            draftBranch={draftBranch}
            checkout={checkout}
            targetDeviceId={targetDeviceId}
            canPick={canSwitch}
            onPick={(name) => setDraftBranch(name)}
            onRefs={setRefs}
          />
        </>
      )}
      <span className="footer-spring" />
      {crSummary !== null && <ChangeRequestBadge summary={crSummary} />}
      {hasWindow(contextUsage) && <ContextUsageIndicator usage={contextUsage} />}
    </div>
  );
}

const EMPTY_DEVICES: readonly Device[] = [];
const EMPTY_SPACES: readonly Space[] = [];

// ---------------------------------------------------------------------------
// The device popover (pickers.rs:1928-2006) — width 224
// ---------------------------------------------------------------------------

export interface DeviceChipProps {
  readonly devices: readonly Device[];
  readonly effectiveDevice: Device | null;
  readonly ownDeviceId: string | null;
  readonly now: number;
  /** The label with no device row — "Select device" in the footer, "This device" on the canvas (pickers.rs:2426). */
  readonly fallbackLabel?: string;
}

export function DeviceChip({
  devices,
  effectiveDevice,
  ownDeviceId,
  now,
  fallbackLabel = "Select device",
}: DeviceChipProps) {
  const [open, setOpen] = useState(false);

  // Device order: this device first, then by lowercased name, then by id.
  const rows = useMemo(() => {
    return [...devices].sort((a, b) => {
      const aLocal = a.id === ownDeviceId ? 0 : 1;
      const bLocal = b.id === ownDeviceId ? 0 : 1;
      if (aLocal !== bLocal) {
        return aLocal - bLocal;
      }
      const byName = a.name.toLowerCase().localeCompare(b.name.toLowerCase());
      return byName !== 0 ? byName : a.id.localeCompare(b.id);
    });
  }, [devices, ownDeviceId]);

  const label = effectiveDevice?.name ?? fallbackLabel;
  const offline = effectiveDevice !== null && !deviceOnline(effectiveDevice, now);

  return (
    <PickerCard
      open={open}
      onOpenChange={setOpen}
      placement="anchorAbove"
      role="dialog"
      ariaLabel="Devices"
      width={224}
      overlaySource="composer-pickers"
      trigger={
        <FooterChip
          id="picker-device"
          icon="monitor"
          label={label}
          open={open}
          offline={offline}
          title={label}
        />
      }
    >
      <DeviceCard
        open={open}
        onClose={() => setOpen(false)}
        rows={rows}
        ownDeviceId={ownDeviceId}
        effectiveDeviceId={effectiveDevice?.id ?? null}
        now={now}
      />
    </PickerCard>
  );
}

function DeviceCard({
  open,
  onClose,
  rows,
  ownDeviceId,
  effectiveDeviceId,
  now,
}: {
  readonly open: boolean;
  readonly onClose: () => void;
  readonly rows: readonly Device[];
  readonly ownDeviceId: string | null;
  readonly effectiveDeviceId: string | null;
  readonly now: number;
}) {
  const [query, setQuery] = useState("");
  const inputRef = useRef<HTMLInputElement | null>(null);

  const names = rows.map((device) => device.name);
  const filtered = filterIndices(query, names).map((ix) => rows[ix]!);

  function pick(device: Device): void {
    const snapshot = composerDefaults.getSnapshot();
    rememberTarget(device.id, snapshot.project, snapshot.noProject);
    onClose();
  }

  const { cursor, setCursor, onKeyDown } = useCursorList({
    enabled: open,
    count: filtered.length,
    onActivate: (ix) => {
      const device = filtered[ix];
      if (device !== undefined) {
        pick(device);
      }
    },
  });

  useEffect(() => {
    if (open) {
      setQuery("");
      // Device → the effective device's index, else 0.
      const target = rows.findIndex((device) => device.id === effectiveDeviceId);
      setCursor(target < 0 ? 0 : target);
      inputRef.current?.focus();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  return (
    <div className="picker-key-frame" onKeyDown={onKeyDown}>
      <PickerSearchField
        inputRef={inputRef}
        value={query}
        onQuery={(value) => {
          setQuery(value);
          setCursor(0);
        }}
        placeholder="Search devices…"
        ariaLabel="Search devices"
      />
      {filtered.length === 0 ? (
        <div className="picker-empty-note">No devices match.</div>
      ) : (
        <div className="picker-list">
          {filtered.map((device, ix) => (
            <MenuRowNav
              key={device.id}
              fadeKey={device.id}
              highlighted={ix === cursor}
              selected={device.id === effectiveDeviceId}
              onClick={() => pick(device)}
            >
              <span className="menu-row-label">{device.name}</span>
              {device.id === ownDeviceId && <span className="picker-row-tag">You</span>}
              {!deviceOnline(device, now) && <Icon name="wifiOff" size={12} className="picker-row-offline" />}
            </MenuRowNav>
          ))}
        </div>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// The project popover (pickers.rs:2012-2124) — width 280
// ---------------------------------------------------------------------------

export interface ProjectChipProps {
  readonly spaces: readonly Space[];
  readonly currentSpaceId: string | null;
  /** The label with no project — "All projects" in the footer, "No project" on the canvas (pickers.rs:2453). */
  readonly fallbackLabel?: string;
}

export function ProjectChip({ spaces, currentSpaceId, fallbackLabel = "All projects" }: ProjectChipProps) {
  const [open, setOpen] = useState(false);

  const pickedSpace = currentSpaceId === null ? null : spaces.find((space) => space.id === currentSpaceId) ?? null;
  const label = pickedSpace === null ? fallbackLabel : spaceDisplayName(pickedSpace);

  return (
    <PickerCard
      open={open}
      onOpenChange={setOpen}
      placement="anchorAboveEnd"
      role="dialog"
      ariaLabel="Project"
      width={280}
      overlaySource="composer-pickers"
      trigger={<FooterChip id="picker-project" icon="folder" label={label} open={open} title={label} />}
    >
      <ProjectCard open={open} onClose={() => setOpen(false)} spaces={spaces} currentSpaceId={currentSpaceId} />
    </PickerCard>
  );
}

function ProjectCard({
  open,
  onClose,
  spaces,
  currentSpaceId,
}: {
  readonly open: boolean;
  readonly onClose: () => void;
  readonly spaces: readonly Space[];
  readonly currentSpaceId: string | null;
}) {
  const [query, setQuery] = useState("");
  const inputRef = useRef<HTMLInputElement | null>(null);

  const labels = spaces.map((space) => spaceDisplayName(space));
  const filtered = filterIndices(query, labels).map((ix) => spaces[ix]!);

  function pickSpace(space: Space): void {
    const snapshot = composerDefaults.getSnapshot();
    rememberTarget(snapshot.device, space.id, false);
    onClose();
  }

  function pickNoProject(): void {
    const snapshot = composerDefaults.getSnapshot();
    // The no-project pick ALSO takes the sidebar's space filter (§2.4,
    // shell.rs:1767-1774): a retained project filter would hide the
    // projectless session's first send from the active list.
    rememberNoProject(snapshot.device, sidebarStore);
    onClose();
  }

  const { cursor, setCursor, onKeyDown } = useCursorList({
    enabled: open,
    // Spaces + the trailing "Don't work in a project" row (§2.5).
    count: filtered.length + 1,
    onActivate: (ix) => {
      if (ix < filtered.length) {
        pickSpace(filtered[ix]!);
      } else {
        pickNoProject();
      }
    },
  });

  useEffect(() => {
    if (open) {
      setQuery("");
      // Space → the current space's index; the trailing "opt-out" row when
      // the draft has no project; NO_ACTIVE_ROW means 0 on the first Down.
      const target =
        currentSpaceId === null
          ? spaces.length
          : spaces.findIndex((space) => space.id === currentSpaceId);
      setCursor(target < 0 ? 0 : target);
      inputRef.current?.focus();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  return (
    <div className="picker-key-frame" onKeyDown={onKeyDown}>
      <PickerSearchField
        inputRef={inputRef}
        value={query}
        onQuery={(value) => {
          setQuery(value);
          setCursor(0);
        }}
        placeholder="Search projects…"
        ariaLabel="Search projects"
      />
      {filtered.length === 0 ? (
        <div className="picker-empty-note">
          {query.trim().length > 0 ? "No projects match." : "No projects on this device."}
        </div>
      ) : (
        <div className="picker-list">
          {filtered.map((space, ix) => (
            <MenuRowNav
              key={space.id}
              fadeKey={space.id}
              highlighted={ix === cursor}
              selected={space.id === currentSpaceId}
              onClick={() => pickSpace(space)}
            >
              <span className="menu-row-label">{spaceDisplayName(space)}</span>
            </MenuRowNav>
          ))}
        </div>
      )}
      {/* A one-off local divider (pickers.rs:2113-2120), not a shared primitive. */}
      <div className="picker-divider" />
      <MenuRowNav
        fadeKey="new-project"
        onClick={() => {
          // Close this popover, THEN open the add-space palette
          // (pickers.rs:2115-2121 / §2.4.3 — ticket 11's `addSpaceStore`
          // owns the surface).
          onClose();
          addSpaceStore.open();
        }}
      >
        <Icon name="plus" size={12} className="picker-row-icon" />
        <span className="menu-row-label">New project…</span>
      </MenuRowNav>
      <MenuRowNav
        fadeKey="no-project"
        highlighted={cursor === filtered.length}
        selected={currentSpaceId === null}
        onClick={pickNoProject}
      >
        <Icon name="close" size={12} className="picker-row-icon" />
        <span className="menu-row-label">Don&apos;t work in a project</span>
      </MenuRowNav>
    </div>
  );
}

// ---------------------------------------------------------------------------
// The checkout-kind popover (pickers.rs:3073-3131) — width 224, two rows
// ---------------------------------------------------------------------------

export interface CheckoutChipProps {
  readonly checkout: CheckoutKind;
  readonly pickedRefHasWorktree: boolean;
  readonly onPick: (kind: CheckoutKind) => void;
}

export function CheckoutChip({ checkout, pickedRefHasWorktree, onPick }: CheckoutChipProps) {
  const [open, setOpen] = useState(false);

  // `checkout_label` (pickers.rs:1280-1304): "New worktree" |
  // "Current worktree" when the picked ref has an existing worktree, else
  // "Current checkout".
  const label =
    checkout === "newWorktree" ? "New worktree" : pickedRefHasWorktree ? "Current worktree" : "Current checkout";

  return (
    <PickerCard
      open={open}
      onOpenChange={setOpen}
      placement="anchorAbove"
      role="dialog"
      ariaLabel="Checkout kind"
      width={224}
      overlaySource="composer-pickers"
      // No search input here — the card never moved focus on open, and the
      // default would land it on the first row; `false` keeps focus put.
      initialFocus={false}
      trigger={
        <FooterChip
          id="picker-checkout"
          icon={checkout === "newWorktree" || pickedRefHasWorktree ? "folderWithFiles" : "folder"}
          label={label}
          open={open}
          title={label}
        />
      }
    >
      <CheckoutCard open={open} onClose={() => setOpen(false)} checkout={checkout} onPick={onPick} />
    </PickerCard>
  );
}

function CheckoutCard({
  open,
  onClose,
  checkout,
  onPick,
}: {
  readonly open: boolean;
  readonly onClose: () => void;
  readonly checkout: CheckoutKind;
  readonly onPick: (kind: CheckoutKind) => void;
}) {
  function pick(kind: CheckoutKind): void {
    onPick(kind);
    onClose();
  }

  // Enter picks the highlighted kind; ↑/↓ walk the two rows (toggle step 5:
  // Checkout anchors on 0 or 1 — index 0 is `local`, and the cursor starts
  // on the committed kind's row).
  const { cursor, setCursor, onKeyDown } = useCursorList({
    enabled: open,
    count: 2,
    initial: checkout === "local" ? 0 : 1,
    onActivate: (ix) => pick(ix === 0 ? "local" : "newWorktree"),
  });

  return (
    <div className="picker-list picker-list-plain" onKeyDown={onKeyDown}>
      <MenuRowNav
        fadeKey="local"
        highlighted={cursor === 0 && checkout !== "local"}
        selected={checkout === "local"}
        onMouseEnter={() => setCursor(0)}
        onClick={() => pick("local")}
      >
        <Icon name="folder" size={14} className="picker-row-icon-muted" />
        <span className="menu-row-label">Current checkout</span>
      </MenuRowNav>
      <MenuRowNav
        fadeKey="newWorktree"
        highlighted={cursor === 1 && checkout !== "newWorktree"}
        selected={checkout === "newWorktree"}
        onMouseEnter={() => setCursor(1)}
        onClick={() => pick("newWorktree")}
      >
        <Icon name="folderWithFiles" size={14} className="picker-row-icon-muted" />
        <span className="menu-row-label">New worktree</span>
      </MenuRowNav>
    </div>
  );
}

// ---------------------------------------------------------------------------
// The ref (branch) popover (pickers.rs:2939-3069) — width 320
// ---------------------------------------------------------------------------

interface RefsState {
  readonly rows: readonly RepoRef[];
  readonly loading: boolean;
  readonly error: string | null;
}

export interface RefChipProps {
  readonly session: ReturnType<typeof useEngineSession>;
  readonly repoPath: string | null;
  readonly currentBranch: string | null;
  readonly draftBranch: string | null;
  readonly checkout: CheckoutKind;
  readonly targetDeviceId: string | null;
  readonly canPick: boolean;
  readonly onPick: (name: string) => void;
  readonly onRefs: (rows: readonly RepoRef[]) => void;
}

export function RefChip({
  session,
  repoPath,
  currentBranch,
  draftBranch,
  checkout,
  targetDeviceId,
  canPick,
  onPick,
  onRefs,
}: RefChipProps) {
  const [open, setOpen] = useState(false);
  const [refs, setRefs] = useState<RefsState>({ rows: [], loading: false, error: null });
  const [switching, setSwitching] = useState<string | null>(null);
  const [switchError, setSwitchError] = useState<string | null>(null);

  const loadRefs = useCallback(
    async (force: boolean): Promise<void> => {
      if (session === null || repoPath === null) {
        return;
      }
      if (refs.loading || (refs.rows.length > 0 && !force)) {
        return;
      }
      setRefs((current) => ({ ...current, loading: true, error: null }));
      try {
        const params: Record<string, unknown> = { repoPath };
        if (targetDeviceId !== null) {
          params.targetDeviceId = targetDeviceId;
        }
        const rows = await session.client.call<RepoRef[]>(methods.LIST_REFS, params);
        const list = Array.isArray(rows) ? rows : [];
        setRefs({ rows: list, loading: false, error: null });
        onRefs(list);
      } catch (error) {
        setRefs({ rows: [], loading: false, error: error instanceof Error ? error.message : String(error) });
        onRefs([]);
      }
    },
    [session, repoPath, refs.loading, refs.rows.length, targetDeviceId, onRefs],
  );

  // Every open force-reloads refs and clears any stale switch error
  // (toggle steps 7-8).
  useEffect(() => {
    if (open) {
      setSwitchError(null);
      void loadRefs(true);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  const picked = draftBranch ?? currentBranch;
  const label = refLabel(picked, checkout);

  async function pickRef(row: RepoRef): Promise<void> {
    // Refs are fixed at creation: a committed chat never moves.
    if (!canPick) {
      return;
    }
    if (row.worktreePath !== null && row.worktreePath !== undefined) {
      // Reuse the ref's existing worktree ("Current worktree").
      onPick(row.name);
      setOpen(false);
      return;
    }
    if (checkout === "newWorktree" || row.current) {
      onPick(row.name);
      setOpen(false);
      return;
    }
    // Local mode + a plain non-current ref: CHECK OUT the space folder via
    // SwitchRef; success records the pick, closes, and force-refreshes
    // refs; failure keeps the popover open with git's verbatim message.
    // One switch at a time (pickers.rs:1309-1357).
    if (session === null || repoPath === null || switching !== null) {
      return;
    }
    setSwitching(row.name);
    setSwitchError(null);
    try {
      const params: Record<string, unknown> = { repoPath, refName: row.name };
      if (targetDeviceId !== null) {
        params.targetDeviceId = targetDeviceId;
      }
      await session.client.call(methods.SWITCH_REF, params);
      onPick(row.name);
      setOpen(false);
      void loadRefs(true);
    } catch (error) {
      setSwitchError(error instanceof Error ? error.message : String(error));
    } finally {
      setSwitching(null);
    }
  }

  return (
    <PickerCard
      open={open}
      onOpenChange={setOpen}
      placement="anchorAbove"
      role="dialog"
      ariaLabel="Ref"
      width={320}
      overlaySource="composer-pickers"
      trigger={<FooterChip id="picker-branch" icon="gitBranch" label={label} open={open} title={label} />}
    >
      <BranchCard
        open={open}
        onClose={() => setOpen(false)}
        refs={refs}
        repoPath={repoPath}
        switching={switching}
        switchError={switchError}
        picked={picked}
        onRetry={() => void loadRefs(true)}
        onPick={(row) => void pickRef(row)}
      />
    </PickerCard>
  );
}

function BranchCard({
  open,
  onClose,
  refs,
  repoPath,
  switching,
  switchError,
  picked,
  onRetry,
  onPick,
}: {
  readonly open: boolean;
  readonly onClose: () => void;
  readonly refs: RefsState;
  readonly repoPath: string | null;
  readonly switching: string | null;
  readonly switchError: string | null;
  readonly picked: string | null;
  readonly onRetry: () => void;
  readonly onPick: (row: RepoRef) => void;
}) {
  const [query, setQuery] = useState("");
  const inputRef = useRef<HTMLInputElement | null>(null);
  const listRef = useRef<HTMLDivElement | null>(null);

  const filtered = useMemo(() => {
    const names = refs.rows.map((row) => row.name);
    return filterIndices(query, names).map((ix) => refs.rows[ix]!).slice(0, MAX_REF_ROWS);
  }, [refs.rows, query]);

  const count = Math.min(refs.rows.length, MAX_REF_ROWS);

  const { cursor, setCursor, onKeyDown } = useCursorList({
    enabled: open,
    count,
    onActivate: (ix) => {
      const row = filtered[ix];
      if (row !== undefined) {
        onPick(row);
      }
    },
  });

  // The walk spans the CAPPED row count while the rendered list is the
  // FILTERED one, so this card keeps its own scroll effect — keyed on the
  // rendered length, not the walk count (the hook's `listRef` effect
  // assumes the two are equal, as they are on every other surface).
  useEffect(() => {
    const row = listRef.current?.querySelector<HTMLElement>(`[data-ref-index="${cursor}"]`);
    row?.scrollIntoView({ block: "nearest" });
  }, [cursor, filtered.length]);

  useEffect(() => {
    if (open) {
      setQuery("");
      // Branch → the current ref's row, capped to 299 (toggle step 5).
      const target = picked === null ? 0 : filtered.findIndex((row) => row.name === picked);
      setCursor(Math.min(target < 0 ? 0 : target, Math.max(0, MAX_REF_ROWS - 1)));
      inputRef.current?.focus();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  return (
    <div className="picker-key-frame" onKeyDown={onKeyDown}>
      <PickerSearchField
        inputRef={inputRef}
        value={query}
        onQuery={(value) => {
          setQuery(value);
          setCursor(0);
        }}
        placeholder="Search refs…"
        ariaLabel="Search refs"
      />
      {repoPath === null ? (
        <div className="picker-empty-note">No project selected</div>
      ) : refs.loading ? (
        <div id="branch-skeleton">
          <SkeletonRows count={4} />
        </div>
      ) : refs.error !== null ? (
        <ErrorRow message={refs.error} onRetry={onRetry} />
      ) : filtered.length === 0 ? (
        <div className="picker-empty-note">No refs found.</div>
      ) : (
        <div className="picker-list" ref={listRef}>
          {filtered.map((row, ix) => (
            <MenuRowNav
              key={row.name}
              fadeKey={row.name}
              data-ref-index={ix}
              highlighted={ix === cursor}
              selected={picked === row.name}
              onClick={() => onPick(row)}
            >
              <span className="menu-row-label">{row.name}</span>
              {switching === row.name && <span className="picker-row-switching">switching…</span>}
              {row.current ? (
                <span className="picker-row-tag">current</span>
              ) : row.worktreePath !== null && row.worktreePath !== undefined ? (
                <span className="picker-row-tag">worktree</span>
              ) : null}
            </MenuRowNav>
          ))}
        </div>
      )}
      {switchError !== null && (
        <div className="picker-trailing-error" role="alert">
          {switchError}
        </div>
      )}
      {refs.rows.length > MAX_REF_ROWS && (
        <div className="picker-trailing-note">
          {`Showing ${Math.min(refs.rows.length, MAX_REF_ROWS)} of ${refs.rows.length} refs`}
        </div>
      )}
    </div>
  );
}

/** `ref_label` (pickers.rs:1748): "Select ref" | "From {name}" | the bare name. */
function refLabel(picked: string | null, checkout: CheckoutKind): string {
  if (picked === null) {
    return "Select ref";
  }
  return checkout === "newWorktree" ? `From ${picked}` : picked;
}
