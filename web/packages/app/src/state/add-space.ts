import { useSyncExternalStore } from "react";
import type {
  Device,
  DriveEntry,
  DriveListing,
  FolderEntry,
  FolderListing,
  PrepareSpacePathReply,
  Space,
} from "@zeron/proto";
import { methods, encodeScopedId } from "@zeron/engine-client";
import { classifyKey, menuStep } from "../lib/picker-search";
import {
  addSpaceCompletion,
  browserRows,
  childPath,
  deviceRows,
  filteredFolders,
  isStaleResponse,
  locationRows,
  manualPathQuery,
  parentPath,
  segmentTarget,
  typedPathTarget,
  type LocationRowEntry,
  type StaleGuard,
} from "../lib/add-space";
import type { EngineSession } from "./engine-session";
import { mintId } from "../lib/id";
import { commandPaletteStore } from "./command-palette";
import { sidebarStore } from "./sidebar";
import { uiSettings } from "./ui-settings";

/**
 * The add-space palette's state machine — the web port of the desktop's
 * `AddSpaceFlow` (`crates/ui/src/shell/spaces.rs:186-231`) plus its whole
 * action surface: open/close, the Devices → Locations → Folders step
 * ladder with its breadcrumbs and back navigation, the search-edit
 * decision tree, the keyboard handler, manual-path prepare, and submit
 * with its optimistic space row.
 *
 * The mount lifecycle rides the Base UI dialog (`RbDialogGlass` in the
 * palette component): `open` is the dialog's open flag, and the exit
 * window — open=false while the layer still paints its `[data-closed]`
 * fade — ends when the component reports `onOpenChangeComplete(false)`
 * (`unmounted()`), which drops the flow. The flow object itself is
 * immutable and swapped on every mutation, so the external-store snapshot
 * stays referentially honest.
 *
 * `addSpaceStore.open()` is the hook other tickets call: ticket 10's
 * spaces-menu "New project…" row and ticket 12's `Mod+K` binding both
 * open this surface (the desktop's `open_add_space`).
 */

/** The palette's mount phases, as the component and CSS read them. */
export type AddSpaceStatus = "closed" | "open" | "closing";

/** New project's step ladder: devices, then locations, then folders. */
export type AddSpaceStep = "devices" | "locations" | "folders";

export type AddSpaceListing =
  | "idle"
  | "loading"
  | { error: string }
  | { path: string; entries: FolderEntry[]; truncated: boolean };

/** `SpacePath` as the palette stores it (`prepare_manual_space`). */
export interface AddSpaceManualPath {
  readonly path: string;
  readonly exists: boolean;
  readonly gitDetected: boolean;
}

/** The single flow state object, carrying the current step's own state. */
export interface AddSpaceFlow {
  /** Stamped once per open; responses from a prior open are dropped. */
  readonly identity: string;
  /** Bumped on every browse/device-switch; drops superseded in-flight work. */
  readonly revision: number;
  /** The step the card renders: devices, then locations, then folders. */
  readonly step: AddSpaceStep;
  /** The chosen location (the Folders step's browse root). */
  readonly location: LocationRowEntry | null;
  /** The selected device; null on the Devices step. */
  readonly deviceId: string | null;
  /** The requested listing path; null = "home, not yet resolved". */
  readonly browserPath: string | null;
  /** The device's resolved home — what the location crumb folds over. */
  readonly home: string | null;
  readonly query: string;
  /** A leading `.` in the query reveals dotfiles (and reloads the folder). */
  readonly hiddenQuery: boolean;
  readonly listing: AddSpaceListing;
  /** The selected device's mounted drives; empty on failure — no error UI. */
  readonly drives: readonly DriveEntry[];
  /** True while the Locations step's drive load is in flight. */
  readonly drivesLoading: boolean;
  /** Keyboard highlight within the current step's FILTERED rows. */
  readonly active: number;
  readonly manualPath: AddSpaceManualPath | null;
  readonly submitBusy: boolean;
  /** The footer's error line. */
  readonly error: string | null;
  /** Best-effort git seed for the current browser path. */
  readonly browserRepo: boolean;
}

export interface AddSpaceSnapshot {
  readonly status: AddSpaceStatus;
  readonly flow: AddSpaceFlow | null;
  /**
   * Optimistic space rows minted by a submit still on the wire (the
   * desktop's `AppState.spaces` echo). Ticket 10's spaces menu merges them
   * by id; a failed createSpace rolls the row back here.
   */
  readonly pendingSpaces: readonly Space[];
}

/** What the mounted palette component supplies each session. */
export interface AddSpaceContext {
  readonly session: EngineSession | null;
  /** Route to the blank canvas — the desktop's `Route::Chat` landing. */
  readonly goToCanvas: () => void;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export class AddSpaceStore {
  /** The dialog's open flag — `false` through the exit window. */
  #open = false;
  /** False only once the exit has drained (the old "closed" state). */
  #mounted = false;
  #flow: AddSpaceFlow | null = null;
  #pending: Space[] = [];
  #context: AddSpaceContext | null = null;
  #manualInFlight = false;
  #submitInFlight = false;
  #snapshot: AddSpaceSnapshot = { status: "closed", flow: null, pendingSpaces: [] };
  readonly #listeners = new Set<() => void>();

  getSnapshot(): AddSpaceSnapshot {
    return this.#snapshot;
  }

  subscribe(listener: () => void): () => void {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  }

  /**
   * `open_add_space` (spaces.rs): mint a fresh identity and land on the
   * Devices step — no device is picked, nothing loads. Devices stream in
   * live from the watch snapshot, so a deviceless open simply renders an
   * empty device list (the old ticket-43 wait died with the auto-pick:
   * there is no pick to wait for anymore).
   */
  open(): void {
    // The desktop's `open_add_space` clears the command palette first —
    // Mod+Shift+N (the New project binding) must not stack two cards.
    commandPaletteStore.close();
    this.#manualInFlight = false;
    this.#submitInFlight = false;
    this.#pending = [];
    this.#flow = {
      identity: mintId(),
      revision: 0,
      step: "devices",
      location: null,
      deviceId: null,
      browserPath: null,
      home: null,
      query: "",
      hiddenQuery: false,
      listing: "idle",
      drives: [],
      drivesLoading: false,
      active: 0,
      manualPath: null,
      submitBusy: false,
      error: null,
      browserRepo: false,
    };
    this.#open = true;
    this.#mounted = true;
    this.#commit();
  }

  /** Every close path funnels here: the exit window, then the drain. */
  close(): void {
    if (this.#open) {
      this.#open = false;
      this.#commit();
    }
  }

  /**
   * The exit drained (`RbDialogGlass`'s `onOpenChangeComplete(false)`): the
   * layer is gone, so the flow goes with it.
   */
  unmounted(): void {
    if (!this.#mounted) {
      return;
    }
    this.#open = false;
    this.#mounted = false;
    this.#flow = null;
    this.#manualInFlight = false;
    this.#submitInFlight = false;
    this.#commit();
  }

  /**
   * Hard close for a host that is unmounting (an engine switch remounts the
   * sidebar): there is nothing left to paint, so no exit window either.
   */
  forceClose(): void {
    this.close();
    this.unmounted();
  }

  /** The component's session binding — re-called on engine switches. */
  attach(context: AddSpaceContext): void {
    this.#context = context;
  }

  // ── Search edits (the Edited decision tree) ───────────────────────────

  /**
   * A keystroke landed in the search input. Every edit resets the
   * highlight to the first row; on the Folders step the desktop's
   * decision tree runs in order — slash-descend, then the manual-path
   * fork, then the plain-filter branch with its dotfile reload. On the
   * Devices/Locations steps the query only filters the pick-list.
   */
  setQuery(text: string): void {
    const flow = this.#aliveFlow();
    if (flow === null || flow.query === text) {
      return;
    }
    if (this.#slashDescend(text)) {
      return;
    }
    let next: AddSpaceFlow = { ...flow, query: text, active: 0 };
    if (flow.step !== "folders") {
      this.#flow = next;
      this.#commit();
      return;
    }
    if (this.#manualInFlight && !this.#submitInFlight) {
      next = { ...next, submitBusy: false };
    }
    this.#manualInFlight = false;
    next = { ...next, revision: next.revision + 1, manualPath: null, error: null };
    this.#flow = next;
    if (manualPathQuery(text)) {
      this.#prepareManual(false, false);
      return;
    }
    const showHidden = text.startsWith(".");
    const reload = showHidden !== next.hiddenQuery;
    this.#flow = { ...next, hiddenQuery: showHidden };
    if (reload) {
      this.#loadFolders(this.#flow.browserPath);
      return;
    }
    this.#commit();
  }

  /**
   * `add_space_slash_descend` (spaces.rs:2080-2129): a trailing `/` on a
   * typed full path jumps there directly; on a folder-naming query it
   * resolves the segment against the listing (exact case, exact
   * case-insensitive, unique prefix) and descends. Returns whether it
   * fired — descending clears the query.
   */
  #slashDescend(text: string): boolean {
    const flow = this.#flow;
    // Slash navigation only applies to folders, never device/location search.
    if (flow === null || flow.step !== "folders") {
      return false;
    }
    if (text.endsWith("/") && (text.startsWith("/") || text.startsWith("~"))) {
      const target = typedPathTarget(text, flow.home);
      if (target === null) {
        return false;
      }
      this.#descend(target, false);
      return true;
    }
    if (!text.endsWith("/")) {
      return false;
    }
    const query = text.slice(0, -1);
    if (query.length === 0 || query.includes("/")) {
      return false;
    }
    const listing = this.#readyListing();
    if (listing === null) {
      return false;
    }
    const dirs = browserRows(listing.entries);
    const names = dirs.map((entry) => entry.name);
    const ix = segmentTarget(names, query);
    if (ix === null) {
      return false;
    }
    const entry = dirs[ix] as FolderEntry;
    this.#descend(childPath(listing.path, entry.name), entry.isRepo);
    return true;
  }

  // ── Browsing ───────────────────────────────────────────────────────────

  /**
   * `add_space_pick_device`: selecting a device advances to its Locations.
   * The pick retires the old device's in-flight probes and manual-path
   * state, clears the query, and loads the drives.
   */
  pickDevice(deviceId: string): void {
    const flow = this.#aliveFlow();
    if (flow === null) {
      return;
    }
    this.#manualInFlight = false;
    this.#flow = {
      ...flow,
      step: "locations",
      location: null,
      revision: flow.revision + 1,
      manualPath: null,
      hiddenQuery: false,
      deviceId,
      listing: "idle",
      drives: [],
      drivesLoading: true,
      browserPath: null,
      home: null,
      browserRepo: false,
      active: 0,
      query: "",
      error: null,
    };
    this.#commit();
    this.#loadDrives();
  }

  /** `add_space_goto_location`: selecting a location advances to its
   *  folders — the Folders step browses the drive's mount (or home). */
  gotoLocation(name: string, path: string | null): void {
    const flow = this.#aliveFlow();
    if (flow === null) {
      return;
    }
    this.#flow = {
      ...flow,
      step: "folders",
      location: { name, path },
      browserRepo: false,
      query: "",
    };
    this.#loadFolders(path);
  }

  /**
   * `add_space_back_to`: the breadcrumb/← retreat. Drops the browse state
   * and the location; backing out to Devices also drops the device, its
   * drives, and the resolved home.
   */
  backTo(step: AddSpaceStep): void {
    const flow = this.#aliveFlow();
    if (flow === null) {
      return;
    }
    this.#manualInFlight = false;
    this.#flow = {
      ...flow,
      step,
      revision: flow.revision + 1,
      location: null,
      listing: "idle",
      browserPath: null,
      browserRepo: false,
      active: 0,
      error: null,
      query: "",
      ...(step === "devices" ? { deviceId: null, drives: [], drivesLoading: false, home: null } : {}),
    };
    this.#commit();
  }

  /** `add_space_open_active`: → / Enter — on Devices/Locations, open the
   *  highlighted row's step; on Folders, open the highlighted folder, or
   *  resolve a typed path when nothing matches. */
  openActive(): void {
    const flow = this.#aliveFlow();
    if (flow === null) {
      return;
    }
    if (flow.step === "devices") {
      const device = this.#deviceRows()[flow.active];
      if (device !== undefined) {
        this.pickDevice(device.id);
      }
      return;
    }
    if (flow.step === "locations") {
      const location = this.#locationRows()[flow.active];
      if (location !== undefined) {
        this.gotoLocation(location.name, location.path);
      }
      return;
    }
    if (manualPathQuery(flow.query)) {
      this.#prepareManual(false, true);
      return;
    }
    const listing = this.#readyListing();
    if (listing === null) {
      return;
    }
    const rows = filteredFolders(listing.entries, flow.query);
    if (rows.length === 0) {
      if (flow.query.startsWith("/") || flow.query.startsWith("~")) {
        const target = typedPathTarget(flow.query, flow.home);
        if (target !== null) {
          this.#descend(target, false);
        }
      }
      return;
    }
    const entry = rows[flow.active];
    if (entry === undefined) {
      return;
    }
    this.#descend(childPath(listing.path, entry.name), entry.isRepo);
  }

  /**
   * `add_space_go_up`: ←, and ⌫ on an empty query. Back traverses folders
   * (parent directory), then locations, then devices; standing on the
   * location's root (or with nothing loaded) retreats to Locations.
   */
  goUp(): void {
    const flow = this.#aliveFlow();
    if (flow === null) {
      return;
    }
    if (flow.step === "devices") {
      return;
    }
    if (flow.step === "locations") {
      this.backTo("devices");
      return;
    }
    const listing = this.#readyListing();
    const root = flow.location?.path ?? flow.home;
    const atRoot = listing !== null && root !== null && listing.path === root;
    const parent = listing !== null && !atRoot ? parentPath(listing.path) : null;
    if (parent !== null) {
      this.#descend(parent, false);
      return;
    }
    this.backTo("locations");
  }

  /** `add_space_accept_completion` (spaces.rs:2158-2166): ⇥ fills the query
   *  with the previewed folder's full name; descending stays on `/`/⏎. */
  acceptCompletion(): void {
    const flow = this.#aliveFlow();
    if (flow === null) {
      return;
    }
    const listing = this.#readyListing();
    if (listing === null) {
      return;
    }
    const rows = filteredFolders(listing.entries, flow.query);
    const completion = addSpaceCompletion(rows, flow.active, flow.query);
    if (completion === null) {
      return;
    }
    this.#flow = { ...flow, query: completion.name };
    this.#commit();
  }

  /** A folder row click (mouse path): descend into it. */
  descend(full: string, isRepo: boolean): void {
    this.#descend(full, isRepo);
  }

  /**
   * The folder-level Retry chip: reload at the current browser path.
   * Only the Folders step renders it — a path reload presumes a device.
   */
  retryLoad(): void {
    const flow = this.#aliveFlow();
    if (flow === null) {
      return;
    }
    this.#loadFolders(flow.browserPath);
  }

  /**
   * `add_space_key`: the palette's keyboard map, bubbling from the focused
   * search input. Returns whether the key was consumed (the caller
   * prevents the browser default then).
   */
  keyDown(event: KeyboardEvent): boolean {
    const flow = this.#aliveFlow();
    if (flow === null) {
      return false;
    }
    // ←/→ act on the current step's rows, not the text caret; ⇥ completes
    // — all three unbound in the desktop's "PaletteSearch" context so
    // they bubble here.
    switch (event.key) {
      case "ArrowRight":
        this.openActive();
        return true;
      case "ArrowLeft":
        this.goUp();
        return true;
      case "Tab":
        this.acceptCompletion();
        return true;
      default:
        break;
    }
    const key = classifyKey(event.key, event.metaKey, event.ctrlKey);
    switch (key) {
      case "escape":
        this.close();
        return true;
      case "up":
      case "down": {
        const count =
          flow.step === "devices"
            ? this.#deviceRows().length
            : flow.step === "locations"
              ? this.#locationRows().length
              : (() => {
                  const listing = this.#readyListing();
                  return listing === null ? 0 : filteredFolders(listing.entries, flow.query).length;
                })();
        const delta = key === "up" ? -1 : 1;
        const next = menuStep(flow.active, count, delta);
        this.#flow = { ...flow, active: next ?? 0 };
        this.#commit();
        return true;
      }
      case "enter":
        this.openActive();
        return true;
      case "mod-enter":
        // ⌘⏎ adds the folder open in the breadcrumbs — the Folders step.
        if (flow.step === "folders") {
          this.submit();
        }
        return true;
      case "backspace":
        if (flow.query.length === 0) {
          this.goUp();
          return true;
        }
        return false;
      default:
        return false;
    }
  }

  // ── Submit ─────────────────────────────────────────────────────────────

  /**
   * `submit_add_space`: ⌘⏎ on the Folders step. A typed path re-prepares
   * with create when the manual probe said it does not exist; a browsed
   * folder goes straight to the create.
   */
  submit(): void {
    const flow = this.#aliveFlow();
    if (flow === null || flow.step !== "folders") {
      return;
    }
    if (manualPathQuery(flow.query)) {
      const create = flow.manualPath !== null && !flow.manualPath.exists;
      this.#prepareManual(create, true);
      return;
    }
    this.#submitBrowsed();
  }

  /**
   * `submit_browsed_space` (spaces.rs:2326-2437): same (device, folder)
   * already has a space → just land in it; otherwise mint a client id,
   * echo the row optimistically, and roll back with the engine's error
   * string inline if the create fails.
   */
  #submitBrowsed(): void {
    const flow = this.#aliveFlow();
    const session = this.#session();
    if (
      flow === null ||
      session === null ||
      flow.submitBusy ||
      flow.step !== "folders" ||
      flow.deviceId === null
    ) {
      return;
    }
    const listing = this.#readyListing();
    if (listing === null) {
      return;
    }
    const path = listing.path;
    const deviceId = flow.deviceId;
    const gitDetected = flow.browserRepo;
    const identity = flow.identity;
    const existing = session.cache
      .getSnapshot()
      .spaces.rows.find((row) => row.deviceId === deviceId && row.path === path);
    if (existing !== undefined) {
      this.#land(this.#scope(existing.id));
      return;
    }
    const spaceId = mintId();
    this.#pending = [
      ...this.#pending,
      {
        // The optimistic row lives in the MERGED (scoped) sidebar view —
        // its ids are scoped to the routed engine so the confirming watch
        // frame replaces the twin by id (ticket 31).
        id: this.#scope(spaceId),
        deviceId: this.#scope(deviceId),
        path,
        name: null,
        gitDetected,
        gitCheckedAt: null,
        checkoutId: null,
        createdAt: new Date().toISOString(),
      },
    ];
    this.#submitInFlight = true;
    this.#flow = { ...flow, submitBusy: true, error: null };
    this.#commit();
    void session.client
      .call(methods.MUTATE, { op: "createSpace", spaceId, deviceId, path, gitDetected })
      .then(() => {
        this.#submitInFlight = false;
        // The optimistic row STAYS — the watch frame replaces it by id.
        if (this.#aliveFlow()?.identity === identity) {
          this.#land(this.#scope(spaceId));
        } else {
          this.#commit();
        }
      })
      .catch((error: unknown) => {
        this.#submitInFlight = false;
        this.#pending = this.#pending.filter((row) => row.id !== spaceId);
        const current = this.#aliveFlow();
        if (current !== null && current.identity === identity) {
          this.#flow = { ...current, submitBusy: false, error: errorMessage(error) };
        }
        this.#commit();
      });
  }

  /**
   * `land_in_space` (spaces.rs:653-668): close, route to the blank canvas,
   * and make the new space the new-chat target — "All" stays "All" but
   * remembers the space; an explicit project filter follows it.
   */
  #land(spaceId: string): void {
    const filter = sidebarStore.getSnapshot().spaceFilter;
    if (filter !== null) {
      sidebarStore.setSpaceFilter(spaceId);
    } else {
      uiSettings.update({ lastSpaceId: spaceId }, "immediate");
    }
    this.#context?.goToCanvas();
    this.close();
  }

  // ── Loads ──────────────────────────────────────────────────────────────

  /**
   * `load_space_folders`: ListFolders on the flow's device (targeted when
   * remote). The step machine's state advances FIRST — with no route to
   * the device the Folders step still leaves the skeleton for the error
   * row, never a forever-skeleton. Guards the response with the
   * identity/device check plus the browser path, the hidden-query flag,
   * and "the search has since become a manual path".
   */
  #loadFolders(path: string | null): void {
    const flow = this.#flow;
    if (flow === null) {
      return;
    }
    const session = this.#session();
    const deviceId = flow.deviceId;
    const request: StaleGuard = { identity: flow.identity, revision: null, deviceId };
    const query = flow.query;
    const hiddenQuery = query.startsWith(".");
    this.#manualInFlight = false;
    this.#flow = {
      ...flow,
      revision: flow.revision + 1,
      manualPath: null,
      hiddenQuery,
      browserPath: path,
      listing: "loading",
      active: 0,
    };
    if (session === null || deviceId === null) {
      this.#flow = { ...this.#flow, listing: { error: "Device is not connected" } };
      this.#commit();
      return;
    }
    this.#commit();
    const params: Record<string, unknown> = { query };
    if (path !== null) {
      params.path = path;
    }
    // Only target remote devices — local calls skip the relay.
    if (this.#localDeviceId() !== deviceId) {
      params.targetDeviceId = deviceId;
    }
    void session.client
      .call<FolderListing>(methods.LIST_FOLDERS, params)
      .then((listing) => {
        const current = this.#guard(request, path, hiddenQuery);
        if (current === null) {
          return;
        }
        this.#flow = {
          ...current,
          // A pathless browse resolved home — remember it for the crumbs.
          home: path === null ? listing.path : current.home,
          listing: {
            path: listing.path,
            entries: listing.entries,
            truncated: listing.truncated,
          },
        };
        this.#commit();
      })
      .catch((error: unknown) => {
        const current = this.#guard(request, path, hiddenQuery);
        if (current === null) {
          return;
        }
        this.#flow = { ...current, listing: { error: errorMessage(error) } };
        this.#commit();
      });
  }

  /**
   * `load_space_drives` (spaces.rs:1960-2007): ListDrives, best-effort —
   * failures stay silent, the Locations section just stays at Home.
   */
  #loadDrives(): void {
    const flow = this.#flow;
    const session = this.#session();
    if (flow === null || session === null || flow.deviceId === null) {
      if (flow !== null && flow.drivesLoading) {
        this.#flow = { ...flow, drivesLoading: false };
        this.#commit();
      }
      return;
    }
    const request: StaleGuard = { identity: flow.identity, revision: null, deviceId: flow.deviceId };
    const params: Record<string, unknown> = {};
    if (this.#localDeviceId() !== flow.deviceId) {
      params.targetDeviceId = flow.deviceId;
    }
    void session.client
      .call<DriveListing>(methods.LIST_DRIVES, params)
      .then((listing) => {
        const current = this.#guard(request, null, null);
        if (current === null) {
          return;
        }
        this.#flow = { ...current, drives: listing.drives, drivesLoading: false };
        this.#commit();
      })
      .catch(() => {
        const current = this.#guard(request, null, null);
        if (current === null) {
          return;
        }
        this.#flow = { ...current, drives: [], drivesLoading: false };
        this.#commit();
      });
  }

  /**
   * `prepare_manual_space` (spaces.rs:2256-2306): probe — and optionally
   * create — a typed path on the OWNING device. A missing folder is never
   * silently created on plain Enter; only the ⌘⏎ path passes create.
   */
  #prepareManual(create: boolean, submit: boolean): void {
    const flow = this.#aliveFlow();
    const session = this.#session();
    if (flow === null || session === null || flow.submitBusy || flow.deviceId === null) {
      return;
    }
    const request: StaleGuard = { identity: flow.identity, revision: flow.revision, deviceId: flow.deviceId };
    const path = flow.query.trim();
    this.#manualInFlight = true;
    this.#flow = { ...flow, submitBusy: submit, error: null };
    this.#commit();
    void session.client
      .call<PrepareSpacePathReply>(methods.PREPARE_SPACE_PATH, {
        path,
        createIfMissing: create,
        // Required here, unlike the two loads: path syntax resolves on the
        // owning device, never assumed local.
        targetDeviceId: flow.deviceId,
      })
      .then((result) => {
        this.#manualInFlight = false;
        const current = this.#guard(request, null, null);
        if (current === null) {
          return;
        }
        const add = submit && result.exists;
        this.#flow = {
          ...current,
          submitBusy: false,
          manualPath: {
            path: result.path,
            exists: result.exists,
            gitDetected: result.gitDetected,
          },
          ...(add
            ? {
                listing: { path: result.path, entries: [], truncated: false },
                browserRepo: result.gitDetected,
              }
            : {}),
        };
        this.#commit();
        if (add) {
          this.#submitBrowsed();
        }
      })
      .catch((error: unknown) => {
        this.#manualInFlight = false;
        const current = this.#guard(request, null, null);
        if (current === null) {
          return;
        }
        this.#flow = { ...current, submitBusy: false, error: errorMessage(error) };
        this.#commit();
      });
  }

  // ── Internals ──────────────────────────────────────────────────────────

  /** The flow, but only while genuinely open — closing reads as gone. */
  #aliveFlow(): AddSpaceFlow | null {
    return this.#open ? this.#flow : null;
  }

  #session(): EngineSession | null {
    return this.#context?.session ?? null;
  }

  /** Scope an id to the routed engine — the merged sidebar's id namespace. */
  #scope(id: string): string {
    const session = this.#session();
    return session === null ? id : encodeScopedId(session.engine.baseUrl, id);
  }

  #devices(): readonly Device[] {
    return this.#session()?.cache.getSnapshot().devices.rows ?? [];
  }

  /** The Devices step's filtered rows (`add_space_devices`). */
  #deviceRows(): Device[] {
    const flow = this.#flow;
    if (flow === null) {
      return [];
    }
    return deviceRows(this.#devices(), flow.query);
  }

  /** The Locations step's filtered rows (`add_space_locations`). */
  #locationRows(): LocationRowEntry[] {
    const flow = this.#flow;
    if (flow === null) {
      return [];
    }
    return locationRows(flow.drives, flow.query);
  }

  #localDeviceId(): string | null {
    return this.#session()?.client.engineInfo?.deviceId ?? null;
  }

  #readyListing(): { path: string; entries: FolderEntry[]; truncated: boolean } | null {
    const listing = this.#flow?.listing;
    if (typeof listing === "string" || listing === undefined || listing === null) {
      return null;
    }
    if (!("entries" in listing)) {
      return null;
    }
    return listing;
  }

  /**
   * The shared response guard: alive (open, same identity era), then
   * `is_stale`, then — for folder loads — the browser path, the hidden
   * flag, and "the search became a manual path".
   */
  #guard(request: StaleGuard, path: string | null, hiddenQuery: boolean | null): AddSpaceFlow | null {
    const current = this.#aliveFlow();
    if (current === null || isStaleResponse(current, request)) {
      return null;
    }
    if (path !== null || hiddenQuery !== null) {
      if (current.browserPath !== path) {
        return null;
      }
      if (hiddenQuery !== null && current.hiddenQuery !== hiddenQuery) {
        return null;
      }
      if (manualPathQuery(current.query)) {
        return null;
      }
    }
    return current;
  }

  #descend(full: string, isRepo: boolean): void {
    const flow = this.#flow;
    if (flow === null) {
      return;
    }
    this.#flow = { ...flow, browserRepo: isRepo, query: "" };
    this.#loadFolders(full);
  }

  #commit(): void {
    this.#snapshot = {
      status: !this.#mounted ? "closed" : this.#open ? "open" : "closing",
      flow: this.#flow,
      pendingSpaces: this.#pending,
    };
    for (const listener of [...this.#listeners]) {
      listener();
    }
  }
}

/**
 * The palette's module-level singleton — same shape as `sidebarStore` /
 * `fleetStore`. `open()`/`close()` are the hooks tickets 10 and 12 call.
 */
export const addSpaceStore = new AddSpaceStore();

/**
 * The fixed `mod-k` binding's toggle (`shell.rs:7832-7839`): the palette
 * mounted → close it; otherwise open it. Ticket 12's shell subscribes the
 * shortcut bus's `add-space-palette` event to this.
 */
export function toggleAddSpace(): void {
  if (addSpaceStore.getSnapshot().flow !== null) {
    addSpaceStore.close();
  } else {
    addSpaceStore.open();
  }
}

const subscribe = (listener: () => void) => addSpaceStore.subscribe(listener);
const getSnapshot = () => addSpaceStore.getSnapshot();

/** The mount phases plus the flow (null only once fully closed). */
export function useAddSpaceSnapshot(): AddSpaceSnapshot {
  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
}

/** The flow while the palette is mounted (open or closing); null when closed. */
export function useAddSpace(): AddSpaceFlow | null {
  return useAddSpaceSnapshot().flow;
}

/**
 * The optimistic space rows, for the spaces menu to merge by id (ticket 10):
 * a row appears here when its create is still on the wire and is replaced
 * by the watch frame's confirmed row — same id — once it lands. The array
 * reference only changes when the pending set itself changes, so this hook
 * does not re-render on every palette keystroke.
 */
export function usePendingSpaces(): readonly Space[] {
  const pending = (): readonly Space[] => addSpaceStore.getSnapshot().pendingSpaces;
  return useSyncExternalStore(subscribe, pending, pending);
}
