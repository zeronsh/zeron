import { useEffect, useState } from "react";
import type { ReactNode } from "react";
import type { FetchToolBlobReply, SessionMessageEntry } from "@zeron/proto";
import { methods } from "@zeron/engine-client";
import type { IconName } from "@zeron/icons";
import { rightPaneStore, type RightSurface } from "../state/right-pane";
import { useEngineSession } from "../state/session-provider";
import { useEngineStatus } from "../state/hooks";
import { TranscriptStore } from "../state/transcript-store";
import { ChangesSurface, ChangesToolbar, CommitDiffToolbar } from "../routes/changes-page";
import { HistoryPane } from "./history/history-pane";
import { HistoryToolbar } from "./history/history-toolbar";
import { FileSurface } from "./files/file-viewer";
import { TerminalDock } from "../terminal/terminal-dock";
import { paneTerminalStore } from "../terminal/store";
import { SurfacePicker } from "./surface-picker";
import { TranscriptView } from "./transcript";
import type { SubagentOpen } from "./tool-group";

/**
 * The right pane's surface registry — the seam between the pane HOST (this
 * ticket) and the surface BODIES (Changes 22, Files 24/25, Terminal 26,
 * History 27, subagent transcript 19). Later tickets replace an entry's
 * `render` through `registerRightSurface` without touching the host.
 *
 * Contextual chrome (title, detail, icon, dirty) reads the backing entities
 * out of `state/right-pane.ts` — the peer of the desktop's
 * `right_surface_rows` walking `file_surfaces` / `diffs` / `subagent_tabs`.
 */

export interface SurfaceContext {
  readonly chatId: string;
}

export interface RightSurfaceEntry {
  readonly kind: RightSurface["kind"];
  readonly title: (s: RightSurface, ctx: SurfaceContext) => string;
  readonly detail?: (s: RightSurface, ctx: SurfaceContext) => string | null;
  readonly icon: (s: RightSurface, ctx: SurfaceContext) => IconName;
  readonly isDirty?: (s: RightSurface, ctx: SurfaceContext) => boolean;
  /** Default true; the picker is the one unclosable surface. */
  readonly isClosable?: (s: RightSurface) => boolean;
  /** The `surface_chrome::toolbar` row above the body (Diff surfaces). */
  readonly toolbar?: (s: RightSurface, ctx: SurfaceContext) => ReactNode;
  readonly render: (s: RightSurface, ctx: SurfaceContext) => ReactNode;
}

const entries = new Map<RightSurface["kind"], RightSurfaceEntry>();

/** Register (or replace) a surface kind's entry. */
export function registerRightSurface(entry: RightSurfaceEntry): void {
  entries.set(entry.kind, entry);
}

export function surfaceEntry(kind: RightSurface["kind"]): RightSurfaceEntry | undefined {
  return entries.get(kind);
}

/*
 * Boot wiring: the pane's terminal host is injected here rather than imported
 * by `state/right-pane.ts` (that module must stay loadable in the node test
 * environment, and xterm must not come with it). The pane store's version
 * bumps — a shell's OSC title changing, a tab exiting — re-render the pane's
 * chips through the right-pane store's notify, the desktop's TitleChanged
 * fan-out's peer.
 */
rightPaneStore.setTerminalSource(paneTerminalStore);
paneTerminalStore.subscribe(() => {
  rightPaneStore.notify();
});

/** The backing facts for a surface, straight from the entity maps. */
function facts(surface: RightSurface, ctx: SurfaceContext) {
  return rightPaneStore.describe(surface, ctx.chatId);
}

function titleOf(fallback: string): (s: RightSurface, ctx: SurfaceContext) => string {
  return (s, ctx) => facts(s, ctx)?.title ?? fallback;
}

/**
 * The pane's content for a surface — `render_right_pane`'s match. A surface
 * whose backing entity is gone renders the picker (`… else the picker`).
 */
export function renderRightSurface(surface: RightSurface, ctx: SurfaceContext): ReactNode {
  if (surface.kind !== "picker" && facts(surface, ctx) === null) {
    return <SurfacePicker chatId={ctx.chatId} />;
  }
  return entries.get(surface.kind)?.render(surface, ctx) ?? null;
}

/**
 * `surface_chrome::toolbar` (§3.27): the 38 px border-box row a Diff surface
 * mounts above its body. Its controls — the scope selector, ref selector,
 * split/wrap/fold-all — are ticket 22's `ChangesToolbar`, reading and
 * mutating the same per-surface state store the body renders from.
 */

/**
 * The Terminal surface — the pane's embedded panel (`RightSurface::Terminal`
 * rendering the shared `right_terminal` panel with `select_tab_by_key`).
 * Each surface chip addresses ONE terminal tab: the id is the tab's key,
 * minted together by `addTerminalSurface`/`openTabFor`.
 */
function TerminalSurface({ surfaceId, chatId }: { surfaceId: string; chatId: string }) {
  return <TerminalDock store={paneTerminalStore} chatId={chatId} docked tabKey={surfaceId} />;
}

/**
 * A subagent's transcript, as a right-pane tab — the desktop's
 * `add_subagent_surface` (shell.rs:2682) + `Transcript::for_doc`. A LIVE
 * subagent watches its doc directly; a FROZEN one (done/failed) tries the
 * `{chatId}/{docId}` uploaded snapshot blob first and falls back to the
 * live watch on any failure. The transcript rides the subagent override
 * (alignTop: top-aligned, top-only fade, no rail, no own-turn runway).
 * Spawn chips inside it open their own nested tabs through the same
 * registrar, keyed to the pane's chat.
 */
function SubagentSurface({ surfaceId, chatId }: { surfaceId: string; chatId: string }) {
  const session = useEngineSession();
  const status = useEngineStatus(session);
  const meta = rightPaneStore.subagentSurfaceOf(surfaceId);
  const client = session?.client ?? null;
  const deviceId = status !== null && status.state === "connected" ? status.info.deviceId : null;
  const docId = meta?.docId ?? null;
  const frozen = meta?.frozen ?? false;
  const blobChatId = meta?.chatId ?? null;

  const [store, setStore] = useState<TranscriptStore | null>(null);
  useEffect(() => {
    if (client === null || docId === null) {
      return;
    }
    const created = new TranscriptStore(client, docId, frozen ? { follow: false } : undefined);
    setStore(created);
    return () => {
      created.dispose();
      setStore((current) => (current === created ? null : current));
    };
  }, [client, docId, frozen]);

  // The frozen snapshot fetch — a best-effort blob read; ANY failure falls
  // back to the live doc watch (`resubscribe` arms it).
  useEffect(() => {
    if (!frozen || client === null || docId === null || blobChatId === null || store === null) {
      return;
    }
    let cancelled = false;
    client
      .call<FetchToolBlobReply>(methods.FETCH_TOOL_BLOB, { blobRef: `${blobChatId}/${docId}` })
      .then((reply) => {
        if (cancelled) {
          return;
        }
        const entries = parseSnapshotEntries(reply.text);
        if (entries !== null) {
          store.seedEntries(entries);
        } else {
          store.resubscribe();
        }
      })
      .catch(() => {
        if (!cancelled) {
          store.resubscribe();
        }
      });
    return () => {
      cancelled = true;
    };
  }, [frozen, client, docId, blobChatId, store]);

  if (client === null || store === null) {
    return null;
  }
  const onOpenSubagent = (payload: SubagentOpen): void => {
    rightPaneStore.addSubagentSurface(chatId, payload);
  };
  return (
    <TranscriptView client={client} docId={store.docId} deviceId={deviceId} store={store} alignTop onOpenSubagent={onOpenSubagent} />
  );
}

/** Parse a frozen snapshot blob — a JSON array of transcript entries. */
function parseSnapshotEntries(text: string): SessionMessageEntry[] | null {
  try {
    const parsed: unknown = JSON.parse(text);
    if (!Array.isArray(parsed)) {
      return null;
    }
    return parsed.filter(
      (entry): entry is SessionMessageEntry =>
        typeof entry === "object" && entry !== null && "id" in entry && "parts" in entry,
    );
  } catch {
    return null;
  }
}

let registered = false;

/** The stub + real bodies this ticket wires; later tickets re-register. */
function registerDefaults(): void {
  if (registered) {
    return;
  }
  registered = true;

  registerRightSurface({
    kind: "picker",
    title: () => "Picker",
    icon: () => "plus",
    isClosable: () => false,
    render: (_s, ctx) => <SurfacePicker chatId={ctx.chatId} />,
  });

  registerRightSurface({
    kind: "file",
    title: titleOf("File"),
    detail: (s, ctx) => facts(s, ctx)?.detail ?? null,
    // The tab strip's IconName slot is monochrome by design; the
    // polychrome file-type icon lives in the surface's breadcrumb toolbar
    // (`FileIcon`, ticket 24's manifest).
    icon: () => "document",
    render: (s, ctx) => (s.kind === "file" ? <FileSurface chatId={ctx.chatId} surfaceId={s.id} /> : null),
  });

  registerRightSurface({
    kind: "diff",
    title: titleOf("Diffs"),
    // `git-branch` when that `Changes` `is_history()`, else `list`.
    icon: (s, ctx) => (facts(s, ctx)?.isHistory === true ? "gitBranch" : "list"),
    toolbar: (s, ctx) => {
      if (s.kind !== "diff") {
        return null;
      }
      const meta = rightPaneStore.diffMetaOf(s.id);
      if (meta !== null && meta.flavor === "history") {
        return <HistoryToolbar chatId={ctx.chatId} surfaceId={s.id} />;
      }
      if (meta !== null && meta.flavor === "commit") {
        return <CommitDiffToolbar chatId={ctx.chatId} surfaceId={s.id} />;
      }
      return <ChangesToolbar chatId={ctx.chatId} surfaceId={s.id} />;
    },
    render: (s, ctx) => {
      if (s.kind !== "diff") {
        return null;
      }
      const meta = rightPaneStore.diffMetaOf(s.id);
      if (meta !== null && meta.flavor === "history") {
        return <HistoryPane chatId={ctx.chatId} surfaceId={s.id} />;
      }
      return <ChangesSurface chatId={ctx.chatId} surfaceId={s.id} />;
    },
  });

  registerRightSurface({
    kind: "terminal",
    title: titleOf("Terminal"),
    icon: () => "terminal",
    render: (s, ctx) => (s.kind === "terminal" ? <TerminalSurface surfaceId={s.id} chatId={ctx.chatId} /> : null),
  });

  registerRightSurface({
    kind: "subagent",
    title: titleOf("Subagent"),
    icon: () => "bot",
    render: (s, ctx) => (s.kind === "subagent" ? <SubagentSurface surfaceId={s.id} chatId={ctx.chatId} /> : null),
  });
}

registerDefaults();
