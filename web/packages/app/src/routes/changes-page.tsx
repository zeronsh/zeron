import { useCallback, useEffect, useMemo, useState, useSyncExternalStore } from "react";
import { Icon, type IconName } from "@zeron/icons";
import { useEngineSession } from "../state/session-provider";
import { useEngineStatus, useNow } from "../state/hooks";
import { useFleet, useFleetSnapshot } from "../state/fleet";
import { ChangesStore, type ChangesSnapshot } from "../state/changes-store";
import { ChangeRequestStore, type ChangeRequestTarget, changeRequestForChat } from "../state/change-requests-store";
import { changesSurfaceStore, useChangesSurface } from "../state/changes-surface";
import { reviewCommentStore, useReviewComments } from "../state/review-comments";
import { rightPaneStore } from "../state/right-pane";
import { chatPageRow } from "../lib/view";
import {
  cleanMessage,
  defaultBaseRef,
  diffPhase,
  DIFF_SCOPE_CHIPS,
  DIFF_SCOPE_LABELS,
  scopeLabel,
  type DiffScope,
  type FileFold,
} from "../lib/diff";
import { DiffView, useParsedDiff, type DiffReviewWiring } from "../components/diff-view";
import { useResolvedAppearance } from "../state/appearance";
import { CommentAdder } from "../components/review-comments/comment-adder";
import { ChangeRequestBadge } from "../components/change-request-badge";
import { MatrixSpinner } from "../components/glyph-spinner";
import { Tooltip, TOOLTIP_VIEW_OPTIONS_MS } from "../components/ui/Tooltip";
import { PickerCard } from "../components/ui/PickerCard";
import { MenuRowNav } from "../components/ui/MenuRows";
import type { ChangeRequestSummary } from "@zeron/proto";

/**
 * The right pane's Changes surface — the web peer of the desktop's Changes
 * tab (`crates/ui/src/changes.rs`). The pane is chat-scoped chrome with no
 * URL of its own; a diff tab's scope, base, layout, wrap, and folds live in
 * the per-surface store (`state/changes-surface.ts`), shared with the
 * toolbar the host renders above this body.
 *
 * The toolbar row (scope selector, the branch → base selector, split/wrap/
 * fold-all) is the desktop's `render_header_controls`; the banner row below
 * it (scope label, +N/−N, the "Partial snapshot" chip) is
 * `render_header_strip`. The scope selector is the desktop's trigger +
 * popover port (ticket 62 overturned ticket 22's sanctioned three-chip
 * deviation, user report 2026-09-20 #7); the base selector stays a native
 * `<select>` — the remaining documented, accepted web deviation (research
 * §5), not a bug to fix here.
 *
 * The CR card is reactive: the surface subscribes a
 * `WatchCheckoutChangeRequest` per the chat's `(device, cwd, branch)` tuple
 * and derives the visible summary. When no change request exists the card is
 * absent. It is a documented web-only page-level addition — the desktop
 * never shows a CR inside the Changes tab; its badge lives in the sidebar
 * row and the composer footer, where the web also renders it.
 */

export function ChangesSurface({ chatId, surfaceId }: { chatId: string; surfaceId: string }) {
  const surface = useChangesSurface(chatId, surfaceId);
  // A commit-pinned tab (`Changes::for_commit`) mounts with its sha; the
  // scope never moves off it for the tab's whole life.
  const meta = rightPaneStore.diffMetaOf(surfaceId);
  const pinnedSha = meta !== null && meta.flavor === "commit" ? meta.commitSha ?? null : null;
  useEffect(() => {
    if (pinnedSha !== null) {
      changesSurfaceStore.pinCommit(chatId, surfaceId, pinnedSha);
    }
  }, [chatId, surfaceId, pinnedSha]);
  return (
    <ChangesBody
      chatId={chatId}
      surfaceId={surfaceId}
      scope={surface.scope}
      requestedBase={surface.baseRef}
      commitSha={surface.commitSha}
      layout={surface.layout}
      wrap={surface.wrap}
      folds={surface.folds}
      scrollEpoch={surface.scrollEpoch}
    />
  );
}

/**
 * A commit-pinned tab's toolbar row (`render_header_controls`' commit arm,
 * changes.rs:3646-3684): a fixed identity chip — mono short sha + the
 * subject — instead of the scope dropdown; split/wrap/fold-all trail. The
 * pin never changes, so there is nothing to pick.
 */
export function CommitDiffToolbar({ chatId, surfaceId }: { chatId: string; surfaceId: string }) {
  const surface = useChangesSurface(chatId, surfaceId);
  const meta = rightPaneStore.diffMetaOf(surfaceId);
  const sha = meta?.commitSha ?? "";
  const subject = meta?.subject ?? "";
  return (
    <div className="surface-toolbar changes-toolbar" role="toolbar" aria-label="Commit diff">
      <span className="changes-commit-sha mono" title={sha}>
        {sha.slice(0, 7)}
      </span>
      <span className="changes-commit-subject">{subject}</span>
      <span className="changes-toolbar-spring" />
      <div className="changes-tools">
        <HeaderToggle
          id="changes-split"
          icon="splitColumns"
          label="Split view"
          active={surface.layout === "split"}
          onClick={() => changesSurfaceStore.toggleLayout(chatId, surfaceId)}
        />
        <Tooltip label="Wrap long lines" delay={TOOLTIP_VIEW_OPTIONS_MS}
          trigger={
            <HeaderToggle
              id="changes-wrap"
              icon="wrapText"
              label="Wrap long lines"
              active={surface.wrap}
              onClick={() => changesSurfaceStore.toggleWrap(chatId, surfaceId)}
            />
          }
        />
        <HeaderToggle
          id="changes-fold-all"
          icon="foldVertical"
          label="Collapse all files"
          active={false}
          onClick={() => changesSurfaceStore.toggleCollapseAll(chatId, surfaceId)}
        />
      </div>
    </div>
  );
}

/**
 * The Diff surface's toolbar row — what the host renders above the body
 * through the registry. Controls mutate the same per-surface store the body
 * reads, so a scope switch or a fold lands in both trees at once.
 */
export function ChangesToolbar({ chatId, surfaceId }: { chatId: string; surfaceId: string }) {
  const surface = useChangesSurface(chatId, surfaceId);
  const [scopeOpen, setScopeOpen] = useState(false);
  return (
    <div className="surface-toolbar changes-toolbar" role="toolbar" aria-label="Diff options">
      <ChangesScopeSelector
        chatId={chatId}
        surfaceId={surfaceId}
        scope={surface.scope}
        open={scopeOpen}
        onOpenChange={setScopeOpen}
      />
      {surface.scope === "branch" ? (
        <BasePicker
          branches={surface.branches}
          branch={surface.branch}
          current={surface.baseRef}
          onChange={(next) => changesSurfaceStore.setBaseRef(chatId, surfaceId, next)}
        />
      ) : null}
      <span className="changes-toolbar-spring" />
      <div className="changes-tools">
        <HeaderToggle
          id="changes-split"
          icon="splitColumns"
          label="Split view"
          active={surface.layout === "split"}
          onClick={() => changesSurfaceStore.toggleLayout(chatId, surfaceId)}
        />
        <Tooltip label="Wrap long lines" delay={TOOLTIP_VIEW_OPTIONS_MS}
          trigger={
            <HeaderToggle
              id="changes-wrap"
              icon="wrapText"
              label="Wrap long lines"
              active={surface.wrap}
              onClick={() => changesSurfaceStore.toggleWrap(chatId, surfaceId)}
            />
          }
        />
        <HeaderToggle
          id="changes-fold-all"
          icon="foldVertical"
          label="Collapse all files"
          active={false}
          onClick={() => changesSurfaceStore.toggleCollapseAll(chatId, surfaceId)}
        />
      </div>
    </div>
  );
}

/**
 * The scope menu's rows (`render_scope_menu`, changes.rs:3845-3871): one
 * `MenuRowNav` per `DiffScope::ALL` on the menu family's 2px rhythm, the
 * active scope's row wearing the selected wash plus the trailing check,
 * every pick calling the unchanged `setScope` and closing through the
 * caller. A unit so the selector composes it into the card and the node
 * test suite can render the rows directly (Base UI's portal renders
 * nothing under `renderToString`).
 */
export function ChangesScopeMenuRows({
  chatId,
  surfaceId,
  scope,
  onPick,
}: {
  readonly chatId: string;
  readonly surfaceId: string;
  readonly scope: DiffScope;
  readonly onPick: () => void;
}) {
  return (
    <div className="changes-scope-menu">
      {DIFF_SCOPE_CHIPS.map((option) => (
        <MenuRowNav
          key={option}
          fadeKey={`changes-scope-row-${option}`}
          selected={scope === option}
          onClick={() => {
            changesSurfaceStore.setScope(chatId, surfaceId, option);
            onPick();
          }}
        >
          <span className="menu-row-label">{DIFF_SCOPE_LABELS[option]}</span>
          {scope === option ? <Icon name="check" size={12} className="changes-scope-check" /> : null}
        </MenuRowNav>
      ))}
    </div>
  );
}

/**
 * The scope selector (`render_header_controls`' scope trigger + menu,
 * changes.rs:3699-3744 / 3845-3871): ONE 24px trigger — the current scope's
 * label plus the 12px chevron — where ticket 22's three chips sat. The
 * `PickerCard` opens 10px below the trigger at the desktop's 180px and
 * carries the rows unit; ticket 62 (user report 2026-09-20 #7) replaced the
 * sanctioned three-chip deviation with this port.
 */
export function ChangesScopeSelector({
  chatId,
  surfaceId,
  scope,
  open,
  onOpenChange,
}: {
  readonly chatId: string;
  readonly surfaceId: string;
  readonly scope: DiffScope;
  /** Controlled open — the toolbar owns the state; every dismissal lands there. */
  readonly open: boolean;
  readonly onOpenChange: (open: boolean) => void;
}) {
  return (
    <PickerCard
      open={open}
      onOpenChange={onOpenChange}
      placement="anchorBelowGap"
      gap={10}
      width={180}
      role="menu"
      ariaLabel="Diff scope"
      overlaySource="changes-scope"
      initialFocus={false}
      trigger={
        <button
          type="button"
          id="changes-scope-trigger"
          className="changes-scope-trigger"
          aria-haspopup="menu"
          aria-expanded={open}
        >
          <span className="changes-scope-label">{DIFF_SCOPE_LABELS[scope]}</span>
          <Icon name="altArrowDown" size={12} className="changes-scope-caret" />
        </button>
      }
    >
      <ChangesScopeMenuRows
        chatId={chatId}
        surfaceId={surfaceId}
        scope={scope}
        onPick={() => onOpenChange(false)}
      />
    </PickerCard>
  );
}

/**
 * A 24×24 icon button on the CONTROL_GAP/CONTROL_RADIUS contract
 * (`header_toggle`, changes.rs:3544-3597): a latched toggle holds a flat
 * `wash(0.14)` with no hover blend; an unlatched one blends `wash(0.0)` →
 * `wash(0.14)` over HOVER_FADE. Icon 14px — `text` when active, else
 * `text_muted` at 0.7.
 */
function HeaderToggle({
  id,
  icon,
  label,
  active,
  onClick,
}: {
  readonly id: string;
  readonly icon: IconName;
  readonly label: string;
  readonly active: boolean;
  readonly onClick: () => void;
}) {
  return (
    <button
      type="button"
      id={id}
      className={`changes-tool ${active ? "changes-tool-active" : ""}`}
      onClick={onClick}
      aria-pressed={active}
      aria-label={label}
    >
      <Icon name={icon} size={14} />
    </button>
  );
}

interface ChangesBodyProps {
  readonly chatId: string;
  readonly surfaceId: string;
  readonly scope: DiffScope;
  readonly requestedBase: string | null;
  readonly commitSha: string | null;
  readonly layout: "unified" | "split";
  readonly wrap: boolean;
  readonly folds: ReadonlyMap<string, FileFold>;
  readonly scrollEpoch: number;
}

const NO_CHANGES: ChangesSnapshot = {
  working: [],
  scoped: null,
  branches: [],
  watchLoaded: false,
  resolvedForChat: null,
  phase: "preparing",
  error: null,
  scopedError: null,
  generation: 0,
};

function ChangesBody({ chatId, surfaceId, scope, requestedBase, commitSha, layout, wrap, folds, scrollEpoch }: ChangesBodyProps) {
  const session = useEngineSession();
  const fleet = useFleet();
  const paired = fleet.engines.length > 0;
  const status = useEngineStatus(session);
  // The MERGED fleet snapshot: the chat row lookup by its scoped id spans
  // every engine; the diff store runs on the routed session's client.
  const snapshot = useFleetSnapshot();
  const now = useNow(10_000);
  // File headers resolve polychrome icons — dark picks the `dark/` tree.
  const appearance = useResolvedAppearance();

  const deviceId = status?.state === "connected" ? status.info.deviceId : null;
  const chat = chatPageRow(chatId, snapshot.chats.rows, snapshot.spaces.rows, snapshot.statuses.rows, now)?.chat ?? null;
  const branch = chat?.branch ?? null;
  const checkoutId = chat?.checkoutId ?? null;
  const cwd = chat?.cwd ?? null;

  const [store, setStore] = useState<ChangesStore | null>(null);
  useEffect(() => {
    if (session === null || deviceId === null) {
      setStore(null);
      return;
    }
    const created = new ChangesStore(session.client, {
      checkoutId,
      deviceId,
      cwd,
      chatId,
    });
    setStore(created);
    return () => {
      created.dispose();
      setStore((current) => (current === created ? null : current));
    };
  }, [session, deviceId, cwd, checkoutId, chatId]);

  const subscribeChanges = useCallback(
    (listener: () => void) => (store === null ? () => {} : store.subscribe(listener)),
    [store],
  );
  const readChanges = useCallback(() => store?.getSnapshot() ?? NO_CHANGES, [store]);
  const changes = useSyncExternalStore(subscribeChanges, readChanges, readChanges);

  // The chat's branch context, mirrored for the toolbar's base picker.
  const branches = changes.branches;
  useEffect(() => {
    changesSurfaceStore.setChatContext(chatId, surfaceId, { branch, branches });
  }, [chatId, surfaceId, branch, branches]);

  const crStore = useMemo(() => {
    if (session === null || deviceId === null) {
      return null;
    }
    // The target IS the local device here — `watchParams` omits
    // `targetDeviceId` for it, matching the desktop byte-for-byte.
    return new ChangeRequestStore(session.client, { localDeviceId: deviceId });
  }, [session, deviceId]);

  useEffect(() => () => {
    crStore?.dispose();
  }, [crStore]);

  const subscribeCr = useCallback(
    (listener: () => void) => (crStore === null ? () => {} : crStore.subscribe(listener)),
    [crStore],
  );
  const readCr = useCallback(() => crStore?.getSnapshot(), [crStore]);
  const crSnap = useSyncExternalStore(subscribeCr, readCr, readCr);

  useEffect(() => {
    if (crStore === null || branch === null || cwd === null || deviceId === null) {
      return;
    }
    const trimmed = branch.trim();
    if (trimmed.length === 0) {
      crStore.setTargets([]);
      return;
    }
    const targets: ChangeRequestTarget[] = [
      { deviceId, cwd, branch: trimmed, checkoutId },
    ];
    crStore.setTargets(targets);
  }, [crStore, branch, cwd, deviceId, checkoutId]);

  useEffect(() => {
    if (store === null) {
      return;
    }
    const base = scope === "branch" ? requestedBase : null;
    const sha = scope === "commit" ? commitSha : null;
    store.setScope(scope, base, sha);
  }, [store, scope, requestedBase, commitSha]);

  const activeDiff = useMemo(() => {
    if (scope === "workingTree") {
      return changes.resolvedForChat;
    }
    return changes.scoped?.diff ?? null;
  }, [changes, scope]);

  const files = useParsedDiff(activeDiff);
  useEffect(() => {
    changesSurfaceStore.setFiles(files);
  }, [files]);

  // ── Ticket 23: staged review comments for this chat ─────────────────────
  // The staged set drives the diff's comment rows; the open draft (with its
  // live body) interleaves after its anchor line. The comment being edited
  // is excluded — its card row becomes the draft row (staged_comments,
  // changes.rs:2587-2597).
  const review = useReviewComments(chatId);
  const diffDraft = review.diffDraft;
  const visibleComments = useMemo(
    () =>
      review.comments.filter(
        (comment) => comment.source.kind === "diff" && comment.id !== diffDraft?.editingId,
      ),
    [review.comments, diffDraft?.editingId],
  );
  // The store's own object identity keeps the row-list memo stable between
  // unrelated re-renders.
  const reviewDraft = diffDraft;
  // The fold heights must include comment cards (body_height_with reads the
  // staged set at toggle time, changes.rs:2324-2329).
  useEffect(() => {
    changesSurfaceStore.setComments(visibleComments, reviewDraft);
  }, [visibleComments, reviewDraft]);
  const reviewWiring: DiffReviewWiring = {
    comments: visibleComments,
    draft: reviewDraft,
    onDraftBody: (body) => reviewCommentStore.setDiffDraftBody(chatId, body),
    onDraftCancel: () => reviewCommentStore.cancelDiffDraft(chatId),
    onDraftCommit: () => reviewCommentStore.commitDiffDraft(chatId),
    onCardEdit: (id) => reviewCommentStore.editDiffComment(chatId, id),
    onCardRemove: (id) => reviewCommentStore.removeComment(chatId, id),
  };
  const renderAdder = useCallback(
    (info: { readonly path: string; readonly side: "old" | "new"; readonly lineNo: number }) => {
      // The adder needs the file's pre-rename path for the comment's anchor
      // (open_draft → old_path_of, changes.rs:2757).
      const oldPath = files.find((file) => file.path === info.path)?.oldPath ?? null;
      return (
        <CommentAdder
          onOpen={() =>
            reviewCommentStore.openDiffDraft(chatId, {
              path: info.path,
              side: info.side,
              line: info.lineNo,
              oldPath,
            })
          }
        />
      );
    },
    [chatId, files],
  );

  const error = changes.error;
  // Scoped-fetch failures replace the content area; the two known engine
  // messages are remapped to friendly copy, everything else stays raw in
  // the warning tone (`render`, changes.rs:4785-4821).
  const scopedError = scope !== "workingTree" ? changes.scopedError : null;
  const scopedNotice =
    scopedError === null
      ? null
      : scopedError.includes("no turn recorded")
        ? { message: "No turn recorded yet — send a message first", warn: false }
        : scopedError.includes("unknown method")
          ? { message: "This chat's device is running an older Zeron — update it to view branch and turn diffs", warn: false }
          : { message: scopedError, warn: true };

  const crSummary: ChangeRequestSummary | null = useMemo(() => {
    if (crSnap === null || crSnap === undefined || deviceId === null || cwd === null || branch === null) {
      return null;
    }
    const target: ChangeRequestTarget = { deviceId, cwd, branch, checkoutId };
    return changeRequestForChat(crSnap.snapshots, target);
  }, [crSnap, deviceId, cwd, branch, checkoutId]);

  const fileCount = files.length;
  const additions = activeDiff?.additions ?? 0;
  const deletions = activeDiff?.deletions ?? 0;
  const baseForLabel = scope === "branch" ? changes.scoped?.baseRef ?? requestedBase : null;

  if (session === null || !paired) {
    return (
      <div className="changes-page changes-page-surface">
        <p className="changes-empty">Pair an engine to view its changes.</p>
      </div>
    );
  }
  if (!snapshot.chats.loaded) {
    return (
      <div className="changes-page changes-page-surface">
        <p className="changes-empty">Loading…</p>
      </div>
    );
  }
  if (chat === null) {
    return (
      <div className="changes-page changes-page-surface">
        <p className="changes-empty">That chat isn&apos;t on this engine.</p>
      </div>
    );
  }

  // The chat row resolved above; the phase mirrors the desktop's
  // `diff_phase(active_diff)` — preparing while the active capture is
  // pending, clean when it is empty, list otherwise.
  const phase = diffPhase(activeDiff);
  const crUnsupported = crSnap != null && !crSnap.supported;

  return (
    <div className="changes-page changes-page-surface">
      {error !== null ? (
        <div className="changes-error-banner" role="alert">
          {error}
        </div>
      ) : null}
      {scopedNotice !== null ? (
        <div
          className={`changes-scoped-error ${scopedNotice.warn ? "changes-scoped-error-warn" : ""}`}
          role={scopedNotice.warn ? "alert" : "status"}
        >
          {scopedNotice.message}
        </div>
      ) : (
        <>
          {phase === "list" ? (
            <div className="changes-banner">
              <span className="changes-banner-label">{scopeLabel({ scope, count: fileCount, base: baseForLabel })}</span>
              <span className="diff-file-add mono" aria-hidden>{`+${additions}`}</span>
              <span className="diff-file-del mono" aria-hidden>{`−${deletions}`}</span>
              <span className="changes-banner-spring" />
              {activeDiff !== null && activeDiff.truncated ? (
                <span className="changes-banner-warn">Partial snapshot</span>
              ) : null}
            </div>
          ) : null}

          {crUnsupported ? (
            <div className="changes-cr-card changes-cr-card-disabled" role="status">
              <span className="changes-cr-card-label">Change requests unavailable on this engine</span>
            </div>
          ) : crSummary !== null ? (
            <div className="changes-cr-card" role="status">
              <span className="changes-cr-card-label">Open change request</span>
              <ChangeRequestBadge summary={crSummary} />
              <span className="changes-cr-card-base mono">
                {crSummary.baseRef} ← {crSummary.headRef}
              </span>
            </div>
          ) : null}
          {/*
            No PR yet → no card. The desktop's badge appears only once a change
            request exists; it has no create affordance to mirror.
          */}

          <div className="changes-body">
            {phase === "preparing" ? (
              <div className="changes-empty changes-preparing" role="status">
                {/* `gradient_spinner("changes-preparing", cell 3.0)` (changes.rs:4831) → a 15px box. */}
                <MatrixSpinner size={15} />
                <span>Preparing diff…</span>
              </div>
            ) : phase === "clean" ? (
              <p className="changes-empty">{cleanMessage(scope, baseForLabel)}</p>
            ) : (
              <DiffView
                files={files}
                appearance={appearance}
                layout={layout}
                wrap={wrap}
                folds={folds}
                onToggleFold={(path) => changesSurfaceStore.toggleFold(chatId, surfaceId, path)}
                scrollEpoch={scrollEpoch}
                renderAdder={scope === "commit" ? undefined : renderAdder}
                review={reviewWiring}
              />
            )}
          </div>
        </>
      )}
    </div>
  );
}

/**
 * The base selector for the branch scope — a native `<select>`, kept as the
 * documented web idiom (research §5 "Searchable ref (base) picker"), with
 * the desktop's `{branch} → {base}` relationship made visible by the
 * prefixed mono branch label (`render_ref_selector`, changes.rs:3875-3948).
 */
function BasePicker({
  branches,
  branch,
  current,
  onChange,
}: {
  readonly branches: readonly string[];
  readonly branch: string | null;
  readonly current: string | null;
  readonly onChange: (next: string) => void;
}) {
  const defaultBase = useMemo(() => defaultBaseRef(branches, branch), [branches, branch]);
  const value = current ?? defaultBase ?? "";
  if (branches.length === 0) {
    return (
      <div className="changes-base">
        <span className="changes-base-loading">Loading branches…</span>
      </div>
    );
  }
  return (
    <div className="changes-base">
      <span className="changes-base-branch mono" title={branch ?? undefined}>
        {branch ?? "HEAD"}
      </span>
      <span className="changes-base-arrow" aria-hidden>
        <Icon name="arrowRight" size={12} />
      </span>
      <select
        className="changes-base-picker"
        aria-label="Compare against"
        value={value}
        onChange={(event) => onChange(event.target.value)}
      >
        {branches.map((candidate) => (
          <option key={candidate} value={candidate}>
            {candidate}
          </option>
        ))}
      </select>
    </div>
  );
}
