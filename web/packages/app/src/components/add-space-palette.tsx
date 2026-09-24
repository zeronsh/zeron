import { useEffect, useRef } from "react";
import type { ReactNode, RefObject } from "react";
import { useNavigate } from "@tanstack/react-router";
import { Icon, type IconName } from "@zeron/icons";
import type { Device, FolderEntry } from "@zeron/proto";
import { useEngineSession } from "../state/session-provider";
import { useNow, useWatchSnapshot } from "../state/hooks";
import { deviceOnline } from "../lib/view";
import {
  addSpaceCompletion,
  breadcrumbs,
  childPath,
  deviceRows,
  filteredFolders,
  highlightRanges,
  locationRows,
  pathUnder,
  type LocationRowEntry,
} from "../lib/add-space";
import {
  addSpaceStore,
  useAddSpaceSnapshot,
  type AddSpaceFlow,
  type AddSpaceStep,
} from "../state/add-space";
import { isMacPlatform } from "../state/shortcuts";
import { ESCAPE_PRIORITY, registerEscapeSurface } from "../state/escape";
import { RbDialogGlass } from "./base/dialog";
import { KeyCap, KeyHintPair, KeyHintText } from "./ui/KeyHint";
import { MenuRowNav } from "./ui/MenuRows";
import { ErrorRow, SkeletonRows } from "./ui/Skeleton";

/**
 * The add-space palette — the "New project" surface, port of the desktop's
 * `render_add_space_overlay` (`spaces.rs`): a 600px command-palette card
 * (its own 14px radius, deliberately NOT the generic 12px) centered on the
 * lighter 0.35 `modal_glass` scrim. New project navigates a step ladder —
 * Devices → Locations → Folders — with a back button and breadcrumb trail
 * over the list, and the Add action living in the footer on the Folders
 * step. One search input filters whichever step is showing.
 *
 * The mount lifecycle rides `RbDialogGlass`: the store's `open` flag drives
 * the dialog, scrim presses and the escape ladder close through
 * `addSpaceStore.close()`, and the 100ms `[data-closed]` layer fade IS the
 * exit window — `unmounted()` fires when Base UI's animation-aware unmount
 * drains, dropping the flow. Headless while closed — the state machine
 * lives in `state/add-space.ts` (`addSpaceStore`); ticket 10's spaces-menu
 * row and ticket 12's `Mod+K` binding call `open()`. Escape resolves on
 * the shell's capture ladder at the reserved `addSpace` priority, so one
 * keystroke can never reach two handlers.
 */

/**
 * The Devices-page platform mapping (settings::devices) — LAPTOP for
 * macos/darwin, GLOBAL for web, SMARTPHONE for ios/android, MONITOR
 * otherwise.
 */
export function devicePlatformIcon(platform: string): IconName {
  switch (platform) {
    case "macos":
    case "darwin":
      return "laptop";
    case "web":
      return "global";
    case "ios":
    case "android":
      return "smartphone";
    default:
      return "monitor";
  }
}

/** The search input's placeholder tracks the step (spaces.rs). */
const PLACEHOLDER: Record<AddSpaceStep, string> = {
  devices: "Search devices…",
  locations: "Search locations…",
  folders: "Search folders…",
};

/** The folders listing, when the flow is on the Folders step and ready. */
function readyListing(flow: AddSpaceFlow): { path: string; entries: FolderEntry[]; truncated: boolean } | null {
  return typeof flow.listing === "object" && "entries" in flow.listing ? flow.listing : null;
}

export function AddSpacePalette() {
  const session = useEngineSession();
  const snapshot = useWatchSnapshot(session);
  const now = useNow(30_000);
  const state = useAddSpaceSnapshot();
  const navigate = useNavigate();
  const inputRef = useRef<HTMLInputElement | null>(null);
  const listRef = useRef<HTMLDivElement | null>(null);

  // The store is module-level; the mounted card is its window onto the
  // active session, re-attached on engine switches WITHOUT closing — the
  // shell never keys the sidebar tree to engines, so an open palette
  // survives a switch (ticket 43). The canvas hop rides a ref so the
  // binding only re-runs when the session does.
  const goToCanvasRef = useRef(() => {
    void navigate({ to: "/" });
  });
  goToCanvasRef.current = () => {
    void navigate({ to: "/" });
  };
  useEffect(() => {
    addSpaceStore.attach({
      session,
      goToCanvas: () => {
        goToCanvasRef.current();
      },
    });
  }, [session]);
  // Only a true host unmount force-closes — nothing is left to paint, so
  // no exit window either. A session change re-runs the effect above; it
  // is not an unmount, and an open flow stays open.
  useEffect(() => () => addSpaceStore.forceClose(), []);

  // The shell's Escape ladder owns Escape at the reserved addSpace
  // priority — one capture-phase handler, so the focused input's own
  // keydown never sees the key (no double close).
  useEffect(() => {
    if (state.status === "closed") {
      return;
    }
    return registerEscapeSurface(ESCAPE_PRIORITY.addSpace, () => {
      if (state.status === "open") {
        addSpaceStore.close();
      }
      // Consumed either way — closing still counts; a second Escape in the
      // exit window must not fall through to the chat interrupt.
      return true;
    });
  }, [state.status]);

  // `focus_pending`: the search input takes focus on open.
  useEffect(() => {
    if (state.status === "open") {
      inputRef.current?.focus({ preventScroll: true });
    }
  }, [state.status]);

  const flow = state.flow;
  if (state.status === "closed" || flow === null) {
    return null;
  }

  const devices = snapshot?.devices.rows ?? [];
  const device = flow.deviceId !== null ? devices.find((row) => row.id === flow.deviceId) ?? null : null;

  // The scrim press is Base UI's dismissal now (modal Dialog, pointer
  // dismissal on — the `modal_glass` contract); `overlayOpen` holds the
  // keyboard claim through the exit window (the scrim is still up while the
  // card fades).
  return (
    <RbDialogGlass
      open={state.status === "open"}
      onOpenChange={(next) => {
        if (!next) {
          addSpaceStore.close();
        }
      }}
      onOpenChangeComplete={(next) => {
        if (!next) {
          addSpaceStore.unmounted();
        }
      }}
      ariaLabel="New project"
      overlaySource="add-space"
      // The component renders through the exit window ("closing"), so the
      // claim holds until the layer is truly gone — a jump firing under a
      // still-visible scrim would strand it (ticket 11's comment).
      overlayOpen
      backdropClassName="add-space-backdrop"
      cardClassName="add-space-frost"
    >
      <div className="add-space-card">
        <Header flow={flow} inputRef={inputRef} />
        <Crumbs flow={flow} device={device} />
        <Results flow={flow} devices={devices} now={now} listRef={listRef} />
        {flow.error !== null && <div className="add-space-error">{flow.error}</div>}
        <Footer flow={flow} />
      </div>
    </RbDialogGlass>
  );
}

// ── Header: search icon · input · esc ────────────────────────────────────

/**
 * The ⌘K bar (spaces.rs): the palette search glyph, the search input with
 * the ⇥ ghost suffix, and the esc hint. The query filters whichever step
 * is showing; completion previews only exist on the Folders step.
 */
function Header(props: {
  readonly flow: AddSpaceFlow;
  readonly inputRef: RefObject<HTMLInputElement | null>;
}) {
  const { flow, inputRef } = props;
  const listing = flow.step === "folders" ? readyListing(flow) : null;
  const rows = listing !== null ? filteredFolders(listing.entries, flow.query) : [];
  const completion = listing !== null ? addSpaceCompletion(rows, flow.active, flow.query) : null;
  return (
    <div className="add-space-header">
      <span className="add-space-search-icon" aria-hidden>
        <Icon name="magnifer" size={16} />
      </span>
      <div className="add-space-search">
        {completion !== null && (
          <span className="add-space-ghost" aria-hidden>
            <span className="add-space-ghost-query">{flow.query}</span>
            <span className="add-space-ghost-suffix">{completion.suffix}</span>
          </span>
        )}
        <input
          ref={inputRef}
          type="text"
          value={flow.query}
          placeholder={PLACEHOLDER[flow.step]}
          spellCheck={false}
          autoComplete="off"
          autoCorrect="off"
          onChange={(event) => {
            addSpaceStore.setQuery(event.target.value);
          }}
          onKeyDown={(event) => {
            // The desktop's "PaletteSearch" context leaves the navigation
            // keys unbound so they bubble to the card's handler.
            if (addSpaceStore.keyDown(event.nativeEvent)) {
              event.preventDefault();
            }
          }}
        />
      </div>
      <KeyHintText cap="esc" label="" />
    </div>
  );
}

// ── Breadcrumbs: back button + the step trail ────────────────────────────

/**
 * Back (zeron has no command palette to return to yet — ticket 16 — so
 * it closes the flow outright), then the trail: "New project" → the device
 * → the location → the browsed folder segments. The current step's crumb
 * is settled, not clickable; ancestors retreat through `back_to` /
 * `goto_location` / `descend`.
 */
function Crumbs(props: { readonly flow: AddSpaceFlow; readonly device: Device | null }) {
  const { flow, device } = props;
  const listing = flow.step === "folders" ? readyListing(flow) : null;
  const location = flow.location;
  const root = location?.path ?? flow.home;
  const atRoot = listing === null || (root !== null && listing.path === root);
  const segments = listing !== null ? breadcrumbs(listing.path) : [];
  return (
    <div className="add-space-crumbs">
      <button
        type="button"
        className="add-space-back"
        aria-label="Close"
        onClick={() => {
          addSpaceStore.close();
        }}
      >
        <Icon name="arrowLeft" size={16} />
      </button>
      <span className="add-space-crumbs-divider" aria-hidden />
      <nav className="add-space-trail" aria-label="New project">
        <Crumb
          label="New project"
          current={flow.step === "devices"}
          onClick={() => {
            addSpaceStore.backTo("devices");
          }}
        />
        {device !== null && (
          <CrumbSegment
            label={device.name}
            icon={devicePlatformIcon(device.platform)}
            current={flow.step === "locations"}
            onClick={() => {
              addSpaceStore.backTo("locations");
            }}
          />
        )}
        {location !== null && (
          <CrumbSegment
            label={location.name}
            icon={location.path === null ? "home" : "hardDrive"}
            current={atRoot}
            onClick={() => {
              addSpaceStore.gotoLocation(location.name, location.path);
            }}
          />
        )}
        {location !== null &&
          segments.map(([label, full]) => {
            // Segments the location crumb already stands for fold away.
            if (root !== null && pathUnder(root, full)) {
              return null;
            }
            return (
              <CrumbSegment
                key={full}
                label={label}
                icon="folder"
                current={full === listing?.path}
                onClick={() => {
                  addSpaceStore.descend(full, false);
                }}
              />
            );
          })}
      </nav>
    </div>
  );
}

function Crumb(props: {
  readonly label: string;
  readonly icon?: IconName;
  readonly current: boolean;
  readonly onClick: () => void;
}) {
  return (
    <button
      type="button"
      className={`add-space-crumb ${props.current ? "add-space-crumb-current" : ""}`}
      onClick={props.onClick}
    >
      {props.icon !== undefined && <Icon name={props.icon} size={14} className="add-space-crumb-icon" />}
      <span className="add-space-crumb-label">{props.label}</span>
    </button>
  );
}

/** A chevron-kept-with-destination trail segment. */
function CrumbSegment(props: {
  readonly label: string;
  readonly icon: IconName;
  readonly current: boolean;
  readonly onClick: () => void;
}) {
  return (
    <span className="add-space-crumb-pair">
      <Icon name="altArrowRight" size={12} className="add-space-crumb-sep" aria-hidden />
      <Crumb label={props.label} icon={props.icon} current={props.current} onClick={props.onClick} />
    </span>
  );
}

// ── Results: the current step's rows ─────────────────────────────────────

/** Match-highlighted row label (popover.rs `search_highlight`). */
function Highlighted(props: { readonly text: string; readonly query: string }) {
  const ranges = highlightRanges(props.text, props.query);
  if (ranges.length === 0) {
    return <span className="add-space-row-label">{props.text}</span>;
  }
  const parts: ReactNode[] = [];
  let at = 0;
  ranges.forEach((range, ix) => {
    if (range.start > at) {
      parts.push(props.text.slice(at, range.start));
    }
    parts.push(
      <span key={ix} className="add-space-hl">
        {props.text.slice(range.start, range.end)}
      </span>,
    );
    at = range.end;
  });
  if (at < props.text.length) {
    parts.push(props.text.slice(at));
  }
  return <span className="add-space-row-label">{parts}</span>;
}

function Results(props: {
  readonly flow: AddSpaceFlow;
  readonly devices: readonly Device[];
  readonly now: number;
  readonly listRef: RefObject<HTMLDivElement | null>;
}) {
  const { flow, devices, now, listRef } = props;
  const listing = readyListing(flow);
  const loadError = typeof flow.listing === "object" && "error" in flow.listing ? flow.listing.error : null;
  const listingPath = flow.step === "folders" ? (listing?.path ?? null) : null;

  // Every step/browse starts the list at the top (the desktop resets
  // `list_scroll`), and keyboard navigation keeps the highlighted row in
  // view (`scroll_to_item` — the rows are the list's direct children).
  useEffect(() => {
    listRef.current?.scrollTo({ top: 0 });
  }, [flow.step, flow.deviceId, flow.location, listingPath, listRef]);
  useEffect(() => {
    const row = listRef.current?.children.item(flow.active);
    row?.scrollIntoView({ block: "nearest" });
  }, [flow.active, listRef]);

  if (flow.step === "devices") {
    const rows = deviceRows(devices, flow.query);
    return (
      <div className="add-space-list-wrap">
        <div className="add-space-list" ref={listRef}>
          {rows.length === 0 && <div className="add-space-list-empty">No devices found</div>}
          {rows.map((device, ix) => (
            <MenuRowNav
              key={device.id}
              fadeKey={`add-space-device-${ix}`}
              highlighted={ix === flow.active}
              onClick={() => {
                addSpaceStore.pickDevice(device.id);
              }}
            >
              <Icon name={devicePlatformIcon(device.platform)} size={15} className="add-space-row-icon" />
              <Highlighted text={device.name} query={flow.query} />
              <span className="add-space-row-rest" />
              <span
                className={`add-space-presence ${deviceOnline(device, now) ? "add-space-presence-online" : ""}`}
              />
            </MenuRowNav>
          ))}
        </div>
      </div>
    );
  }

  if (flow.step === "locations") {
    const rows = locationRows(flow.drives, flow.query);
    return (
      <div className="add-space-list-wrap">
        <div className="add-space-list" ref={listRef}>
          {rows.length === 0 && <div className="add-space-list-empty">No locations found</div>}
          {rows.map((location, ix) => (
            <LocationRow
              key={location.path ?? "home"}
              location={location}
              query={flow.query}
              highlighted={ix === flow.active}
            />
          ))}
          {flow.drivesLoading && <div className="add-space-locations-loading">Loading locations…</div>}
        </div>
      </div>
    );
  }

  // Folders.
  if (listing === null && loadError === null) {
    return (
      <div className="add-space-list-state">
        <SkeletonRows count={6} />
      </div>
    );
  }
  if (loadError !== null) {
    return (
      <div className="add-space-list-error">
        <ErrorRow
          message={loadError}
          onRetry={() => {
            addSpaceStore.retryLoad();
          }}
        />
      </div>
    );
  }
  const rows = filteredFolders(listing?.entries ?? [], flow.query);
  if (rows.length === 0) {
    return (
      <div className="add-space-list-empty">
        {flow.query.length === 0 ? "No folders here" : "No folders match"}
      </div>
    );
  }
  const base = listing?.path ?? "";
  return (
    <div className="add-space-list-wrap">
      <div className="add-space-list" ref={listRef}>
        {rows.map((entry, ix) => (
          <MenuRowNav
            key={entry.name}
            fadeKey={`add-space-folder-${ix}`}
            highlighted={ix === flow.active}
            onClick={() => {
              addSpaceStore.descend(childPath(base, entry.name), entry.isRepo);
            }}
          >
            <Icon name="folder" size={15} className="add-space-row-icon" />
            <Highlighted text={entry.name} query={flow.query} />
            <span className="add-space-row-rest" />
            {entry.isRepo && <Icon name="gitBranch" size={13} className="add-space-repo-icon" />}
          </MenuRowNav>
        ))}
      </div>
    </div>
  );
}

function LocationRow(props: {
  readonly location: LocationRowEntry;
  readonly query: string;
  readonly highlighted: boolean;
}) {
  const { location, query, highlighted } = props;
  return (
    <MenuRowNav
      fadeKey={`add-space-location-${location.name}`}
      highlighted={highlighted}
      onClick={() => {
        addSpaceStore.gotoLocation(location.name, location.path);
      }}
    >
      <Icon
        name={location.path === null ? "home" : "hardDrive"}
        size={15}
        className="add-space-row-icon"
      />
      <Highlighted text={location.name} query={query} />
    </MenuRowNav>
  );
}

// ── Footer: the key-hint legend + the Add action (Folders step) ──────────

function Footer(props: { readonly flow: AddSpaceFlow }) {
  const { flow } = props;
  const manualMissing = flow.manualPath !== null && !flow.manualPath.exists;
  const listing = readyListing(flow);
  const dim = flow.submitBusy || (listing === null && flow.manualPath === null);
  return (
    <div className="add-space-footer">
      <KeyHintPair first={<Icon name="arrowUp" />} second={<Icon name="arrowDown" />} label="Navigate" />
      <KeyHintText cap="↵" label="Open" />
      <KeyHintText cap="esc" label="Close" />
      <span className="add-space-footer-rest" />
      {flow.step === "folders" && (
        <button
          type="button"
          className="add-space-submit"
          data-rb-dim={dim ? "" : undefined}
          onClick={() => {
            addSpaceStore.submit();
          }}
        >
          {flow.submitBusy ? (
            <span>Adding…</span>
          ) : (
            <>
              <span>{manualMissing ? "Create and add" : "Add project"}</span>
              <KeyCap>
                <span className="key-cap-word">{isMacPlatform() ? "⌘↵" : "Ctrl↵"}</span>
              </KeyCap>
            </>
          )}
        </button>
      )}
    </div>
  );
}
