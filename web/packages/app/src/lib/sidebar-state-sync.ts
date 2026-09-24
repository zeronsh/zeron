import { encodeScopedId, parseScopedId, methods } from "@zeron/engine-client";
import type { EngineClient, WatchHandle } from "@zeron/engine-client";
import type { SidebarStateSnapshot } from "@zeron/proto";
import type { SidebarSection, UiSettingsStore } from "../state/ui-settings";

/**
 * The engine-side sidebar state bridge — the web peer of the desktop's
 * `shell/sidebar_state_sync.rs` (ticket 11). Upstream zeron synced pins and
 * sections through the cloud registry; zeron removed that layer on purpose
 * (ADR 0004). The state now lives engine-side on the project-actions
 * precedent: the engine owning a profile persists `sidebar-state.json` and
 * fans changes out over `WatchSidebarState`, while `localStorage`
 * (`zeron.ui-settings.v1`) stays as the offline cache and optimistic
 * write-ahead. Writes flow through `SetSidebarPins` / `SetSidebarSections`
 * — ordered-list replaces that reply with the fresh snapshot — and
 * conflicts resolve last-write-wins: a surface with a pending local write
 * never adopts a frame (our write is newer), and the push lands after.
 *
 * The web is multi-engine where the desktop is single-engine, so each
 * attached engine keeps its own slice (ticket 11: "each engine holds its
 * own slice; no cross-engine merge"): the wire bucket for engine E carries
 * only ids scoped to E (raw engine ids on the wire, `ScopedId`s in the
 * cache), and E's frames reconcile only E's ids inside the merged
 * per-profile bucket the sidebar renders. Frames from one engine never
 * clobber another engine's ids, and adopting a frame never re-pushes —
 * only local user mutations (and the load-time prune) mark surfaces dirty.
 *
 * Routing matches the existing surface: requests ride one specific
 * engine's `EngineClient`, where `wireParams` strips any
 * `targetDeviceId` before the frame leaves — no relay, the engine is the
 * destination.
 */

/** How long an ended `WatchSidebarState` waits before re-subscribing. */
const RESUBSCRIBE_MS = 2_000;

/** The client surface a bridge needs — `Pick<EngineClient, "call" | "watch">`. */
export type SidebarStateClient = Pick<EngineClient, "call" | "watch">;

/** The wire shape of one custom section (`zeron_proto::SidebarSection`). */
interface WireSidebarSection {
  readonly id: string;
  readonly name: string;
  readonly sessionIds: readonly string[];
  readonly collapsed: boolean;
}

/** One engine's bridge: the watch, the dirty flags, the last-known state. */
interface Bridge {
  readonly key: string;
  readonly client: SidebarStateClient;
  profileKey: string | null;
  watch: WatchHandle | null;
  retryTimer: ReturnType<typeof setTimeout> | null;
  /** Invalidates stale retry timers and late frames after detach. */
  epoch: number;
  /** The engine's own state is unknown until the first frame (or reply). */
  gotFrame: boolean;
  /** The engine's last-reported bucket for its profile — raw engine ids. */
  lastPins: readonly string[] | null;
  lastSections: readonly WireSidebarSection[] | null;
  /** The section ids the engine's last frame listed (position reconcile). */
  lastSectionIds: ReadonlySet<string>;
  dirtyPins: boolean;
  dirtySections: boolean;
  writeRunning: boolean;
  /** An engine too old to know the method never recovers — local-only. */
  unsupported: boolean;
  /** The connection generation the last item arrived on. */
  generation: number;
}

/**
 * Replace one engine's OWN items inside a merged list: positions the engine
 * previously held take the arriving items in arrival order (a dropped id
 * loses its slot), every foreign item keeps its position and relative
 * order, and leftover arrivals append. Idempotent for a repeated identical
 * frame, and never lets one engine's frame move another engine's ids —
 * the "no cross-engine merge" rule expressed as one walk.
 */
export function reconcileOwnedIds(
  current: readonly string[],
  next: readonly string[],
  owner: (id: string) => boolean,
): string[] {
  const out: string[] = [];
  const seen = new Set<string>();
  let take = 0;
  for (const id of current) {
    if (owner(id)) {
      if (take < next.length) {
        const arrival = next[take]!;
        if (!seen.has(arrival)) {
          seen.add(arrival);
          out.push(arrival);
        }
        take += 1;
      }
      // The engine dropped this id: the slot disappears.
      continue;
    }
    if (!seen.has(id)) {
      seen.add(id);
      out.push(id);
    }
  }
  for (; take < next.length; take += 1) {
    const arrival = next[take]!;
    if (!seen.has(arrival)) {
      seen.add(arrival);
      out.push(arrival);
    }
  }
  return out;
}

/** Content comparison for ordered id lists. */
function idsEqual(left: readonly string[], right: readonly string[]): boolean {
  return left.length === right.length && left.every((id, ix) => id === right[ix]);
}

/** Content comparison for per-profile pin maps. */
function pinMapsEqual(
  left: Readonly<Record<string, readonly string[]>>,
  right: Readonly<Record<string, readonly string[]>>,
): boolean {
  const leftKeys = Object.keys(left);
  if (leftKeys.length !== Object.keys(right).length) {
    return false;
  }
  return leftKeys.every((key) => {
    const bucket = right[key];
    return bucket !== undefined && idsEqual(left[key]!, bucket);
  });
}

/** Content comparison for per-profile section maps (order matters). */
function sectionMapsEqual(
  left: Readonly<Record<string, readonly SidebarSection[]>>,
  right: Readonly<Record<string, readonly SidebarSection[]>>,
): boolean {
  const leftKeys = Object.keys(left);
  if (leftKeys.length !== Object.keys(right).length) {
    return false;
  }
  return leftKeys.every((key) => sectionsEqual(left[key] ?? [], right[key] ?? []));
}

function sectionsEqual(left: readonly SidebarSection[], right: readonly SidebarSection[]): boolean {
  return (
    left.length === right.length &&
    left.every((section, ix) => {
      const other = right[ix]!;
      return (
        section.id === other.id &&
        section.name === other.name &&
        section.collapsed === other.collapsed &&
        idsEqual(section.sessionIds, other.sessionIds)
      );
    })
  );
}

function wireSectionsEqual(
  left: readonly WireSidebarSection[],
  right: readonly WireSidebarSection[],
): boolean {
  return (
    left.length === right.length &&
    left.every((section, ix) => {
      const other = right[ix]!;
      return (
        section.id === other.id &&
        section.name === other.name &&
        section.collapsed === other.collapsed &&
        idsEqual(section.sessionIds, other.sessionIds)
      );
    })
  );
}

/** Parse a scoped id without throwing (a malformed id owns nothing). */
function ownerOf(id: string): string | null {
  if (!id.startsWith("engine:v1:")) {
    return null;
  }
  try {
    return parseScopedId(id).engine;
  } catch {
    return null;
  }
}

/**
 * The per-engine sidebar state bridge over the settings store
 * (`localStorage` offline cache). Attach one bridge per paired engine; the
 * class owns the `WatchSidebarState` subscription, the frame adoption, and
 * the single-flight write-through for every attached engine.
 */
export class SidebarStateSync {
  readonly #settings: UiSettingsStore;
  readonly #bridges = new Map<string, Bridge>();
  /** Re-entrancy guard: adoption writes to the cache never mark surfaces dirty. */
  #applying = 0;
  #lastPins: Readonly<Record<string, readonly string[]>> = {};
  #lastSections: Readonly<Record<string, readonly SidebarSection[]>> = {};
  #disposed = false;
  readonly #unsubscribeSettings: () => void;

  constructor(settings: UiSettingsStore) {
    this.#settings = settings;
    const snapshot = settings.getSnapshot();
    this.#lastPins = snapshot.sidebarPinnedSessionIdsByProfile;
    this.#lastSections = snapshot.sidebarSectionsByProfile;
    this.#unsubscribeSettings = settings.subscribe(() => this.#onSettingsChanged());
  }

  /** The engine keys currently bridged (the registry wiring's detach loop). */
  attachedKeys(): readonly string[] {
    return [...this.#bridges.keys()];
  }

  /**
   * Bridge one engine. A re-attach with the same client and profile key is
   * a no-op; a changed profile key (the engine's identity moved) restarts
   * the bridge so its bucket filtering follows the new key.
   */
  attach(engineKey: string, client: SidebarStateClient, profileKey: string | null): void {
    if (this.#disposed) {
      return;
    }
    const existing = this.#bridges.get(engineKey);
    if (existing !== undefined) {
      if (existing.client === client && existing.profileKey === profileKey) {
        return;
      }
      this.detach(engineKey);
    }
    const bridge: Bridge = {
      key: engineKey,
      client,
      profileKey,
      watch: null,
      retryTimer: null,
      epoch: 0,
      gotFrame: false,
      lastPins: null,
      lastSections: null,
      lastSectionIds: new Set(),
      dirtyPins: false,
      dirtySections: false,
      writeRunning: false,
      unsupported: false,
      generation: 0,
    };
    this.#bridges.set(engineKey, bridge);
    if (profileKey !== null) {
      this.#registerWatch(bridge);
    }
  }

  /** Stop bridging one engine; its cached writes stay for a later re-attach. */
  detach(engineKey: string): void {
    const bridge = this.#bridges.get(engineKey);
    if (bridge === undefined) {
      return;
    }
    this.#bridges.delete(engineKey);
    bridge.epoch += 1;
    bridge.watch?.cancel();
    bridge.watch = null;
    if (bridge.retryTimer !== null) {
      clearTimeout(bridge.retryTimer);
      bridge.retryTimer = null;
    }
  }

  /** Tear the whole bridge down (tests). */
  dispose(): void {
    this.#disposed = true;
    for (const key of [...this.#bridges.keys()]) {
      this.detach(key);
    }
    this.#unsubscribeSettings();
  }

  // ── The watch ────────────────────────────────────────────────────────

  #registerWatch(bridge: Bridge): void {
    if (this.#disposed || bridge.unsupported || bridge.profileKey === null) {
      return;
    }
    bridge.epoch += 1;
    const epoch = bridge.epoch;
    bridge.watch = bridge.client.watch<SidebarStateSnapshot>(
      methods.WATCH_SIDEBAR_STATE,
      {},
      {
        onItem: (snapshot, context) => {
          if (this.#bridges.get(bridge.key) !== bridge || bridge.epoch !== epoch) {
            return;
          }
          // A fresh stream instance (reconnect): cached offline writes retry
          // now, BEFORE this frame applies — LWW: ours is newer.
          if (context.generation !== bridge.generation) {
            bridge.generation = context.generation;
            this.#push(bridge);
          }
          this.#applyFrame(bridge, snapshot);
        },
        onEnd: (error) => {
          if (this.#bridges.get(bridge.key) !== bridge || bridge.epoch !== epoch) {
            return;
          }
          bridge.watch = null;
          if (error !== undefined && error.kind === "unknown-method") {
            // An engine from before this surface: stay in local-only mode.
            bridge.unsupported = true;
            return;
          }
          // The stream ended (engine restart, mid-connection error): the
          // client re-subscribes on reconnect, but a clean end needs our own
          // retry — same backoff shape as the desktop's watch loop.
          bridge.retryTimer = setTimeout(() => {
            bridge.retryTimer = null;
            if (this.#bridges.get(bridge.key) === bridge && bridge.epoch === epoch) {
              this.#registerWatch(bridge);
            }
          }, RESUBSCRIBE_MS);
        },
      },
      // `WatchSidebarState` sends no readiness ack — items only (the
      // SubscribeTerminal shape), so the ack barrier is disabled.
      { ackTimeoutMs: 0 },
    );
  }

  // ── Frames ───────────────────────────────────────────────────────────

  #applyFrame(bridge: Bridge, snapshot: unknown): void {
    if (
      typeof snapshot !== "object" ||
      snapshot === null ||
      typeof (snapshot as SidebarStateSnapshot).pinsByProfile !== "object" ||
      typeof (snapshot as SidebarStateSnapshot).sectionsByProfile !== "object"
    ) {
      return;
    }
    const profileKey = bridge.profileKey;
    if (profileKey === null) {
      return;
    }
    const wire = snapshot as SidebarStateSnapshot;
    const enginePins = wire.pinsByProfile?.[profileKey] ?? null;
    const engineSections = wire.sectionsByProfile?.[profileKey] ?? null;
    const first = !bridge.gotFrame;
    // Once any frame has landed, the engine's state is known (an absent
    // bucket is an empty list, not a mystery): `last*` null only before it.
    bridge.gotFrame = true;
    bridge.lastPins = enginePins ?? [];
    bridge.lastSections = engineSections ?? [];

    const current = this.#settings.getSnapshot();
    const pins = current.sidebarPinnedSessionIdsByProfile;
    const sections = current.sidebarSectionsByProfile;
    // The position reconcile for the NEXT frame walks these ids.
    bridge.lastSectionIds = new Set((engineSections ?? []).map((section) => section.id));

    if (first) {
      // One-time import: the engine has no bucket for a surface while the
      // offline cache does (pre-ticket state, or authoring that never
      // landed) — push the local value; last write wins.
      if (
        (enginePins === null || enginePins.length === 0) &&
        (pins[profileKey] ?? []).some((id) => ownerOf(id) === bridge.key)
      ) {
        bridge.dirtyPins = true;
      }
      if (engineSections === null && (sections[profileKey] ?? []).length > 0) {
        bridge.dirtySections = true;
      }
      if (bridge.dirtyPins || bridge.dirtySections) {
        this.#push(bridge);
        return;
      }
    }

    if (!bridge.dirtyPins) {
      const scoped = (enginePins ?? []).map((id) => encodeScopedId(bridge.key, id));
      const merged = reconcileOwnedIds(
        pins[profileKey] ?? [],
        scoped,
        (id) => ownerOf(id) === bridge.key,
      );
      if (!idsEqual(merged, pins[profileKey] ?? [])) {
        this.#adopt({ sidebarPinnedSessionIdsByProfile: withBucket(pins, profileKey, merged) });
      }
    }
    if (!bridge.dirtySections) {
      const merged = this.#reconcileSections(
        bridge,
        engineSections ?? [],
        sections[profileKey] ?? [],
      );
      if (!sectionsEqual(merged, sections[profileKey] ?? [])) {
        this.#adopt({ sidebarSectionsByProfile: withBucket(sections, profileKey, merged) });
      }
    }
  }

  /**
   * One engine's section list over the merged bucket: positions the engine
   * previously listed take its arriving sections (a dropped section loses
   * its slot), foreign sections keep theirs, and an arriving id that
   * already sits in the merged list merges its members in (both engines
   * hold the same section shells with their own members). Names and
   * disclosure state follow the most recently arrived frame.
   */
  #reconcileSections(
    bridge: Bridge,
    frame: readonly WireSidebarSection[],
    current: readonly SidebarSection[],
  ): SidebarSection[] {
    const owner = (id: string) => ownerOf(id) === bridge.key;
    const arriving: SidebarSection[] = frame.map((section) => ({
      id: section.id,
      name: section.name,
      collapsed: section.collapsed,
      sessionIds: section.sessionIds.map((id) => encodeScopedId(bridge.key, id)),
    }));
    const result: SidebarSection[] = [];
    const index = new Map<string, number>();
    const upsert = (section: SidebarSection): void => {
      const at = index.get(section.id);
      if (at === undefined) {
        index.set(section.id, result.length);
        result.push({ ...section });
        return;
      }
      const previous = result[at]!;
      result[at] = {
        ...section,
        sessionIds: reconcileOwnedIds(previous.sessionIds, section.sessionIds, owner),
      };
    };
    let take = 0;
    for (const section of current) {
      if (bridge.lastSectionIds.has(section.id)) {
        if (take < arriving.length) {
          upsert(arriving[take]!);
          take += 1;
        }
        // The engine dropped this section: the slot disappears.
        continue;
      }
      upsert(section);
    }
    for (; take < arriving.length; take += 1) {
      upsert(arriving[take]!);
    }
    return result;
  }

  // ── Local writes ─────────────────────────────────────────────────────

  #onSettingsChanged(): void {
    if (this.#applying > 0 || this.#disposed) {
      // An adoption (ours): frames never mark surfaces dirty.
      return;
    }
    const snapshot = this.#settings.getSnapshot();
    const pins = snapshot.sidebarPinnedSessionIdsByProfile;
    const sections = snapshot.sidebarSectionsByProfile;
    const pinsChanged = !pinMapsEqual(pins, this.#lastPins);
    const sectionsChanged = !sectionMapsEqual(sections, this.#lastSections);
    this.#lastPins = pins;
    this.#lastSections = sections;
    if (!pinsChanged && !sectionsChanged) {
      return;
    }
    for (const bridge of this.#bridges.values()) {
      bridge.dirtyPins ||= pinsChanged;
      bridge.dirtySections ||= sectionsChanged;
      this.#push(bridge);
    }
  }

  /** The engine bucket's own ids, raw, in merged order — the wire payload. */
  #ownRawIds(bridge: Bridge, profileKey: string): string[] {
    const bucket = this.#settings.getSnapshot().sidebarPinnedSessionIdsByProfile[profileKey] ?? [];
    const out: string[] = [];
    for (const id of bucket) {
      if (ownerOf(id) === bridge.key) {
        out.push(parseScopedId(id).rawId);
      }
    }
    return out;
  }

  /** The merged section list projected onto one engine's members. */
  #ownWireSections(bridge: Bridge, profileKey: string): WireSidebarSection[] {
    const sections = this.#settings.getSnapshot().sidebarSectionsByProfile[profileKey] ?? [];
    return sections.map((section) => ({
      id: section.id,
      name: section.name,
      collapsed: section.collapsed,
      sessionIds: section.sessionIds
        .filter((id) => ownerOf(id) === bridge.key)
        .map((id) => parseScopedId(id).rawId),
    }));
  }

  #push(bridge: Bridge): void {
    if (this.#disposed || bridge.writeRunning || bridge.profileKey === null || bridge.unsupported) {
      return;
    }
    if (!bridge.dirtyPins && !bridge.dirtySections) {
      return;
    }
    bridge.writeRunning = true;
    void this.#writeLoop(bridge);
  }

  /**
   * The single-flight write-through: send each dirty surface as an
   * ordered-list replace, apply the fresh-snapshot reply, and clear the
   * surface only when the sent value is still current (a mid-flight edit
   * stays dirty and is re-sent on the next pass). A transport failure
   * leaves the surface dirty — the offline cache keeps serving reads, and
   * the next frame (or local mutation) retries the push.
   */
  async #writeLoop(bridge: Bridge): Promise<void> {
    let failed = false;
    try {
      while (this.#bridges.get(bridge.key) === bridge && bridge.profileKey !== null) {
        const profileKey = bridge.profileKey;
        if (bridge.dirtyPins) {
          const sessionIds = this.#ownRawIds(bridge, profileKey);
          if (bridge.gotFrame && idsEqual(sessionIds, bridge.lastPins ?? [])) {
            // The engine already holds exactly this list — the reply would
            // echo it back; skip the round trip.
            bridge.dirtyPins = false;
          } else {
            const reply = await bridge.client.call<SidebarStateSnapshot>(methods.SET_SIDEBAR_PINS, {
              profileKey,
              sessionIds,
            });
            this.#applyFrame(bridge, reply);
            if (idsEqual(sessionIds, this.#ownRawIds(bridge, profileKey))) {
              bridge.dirtyPins = false;
            }
          }
        }
        if (this.#bridges.get(bridge.key) !== bridge) {
          break;
        }
        if (bridge.dirtySections) {
          const sections = this.#ownWireSections(bridge, profileKey);
          if (bridge.gotFrame && wireSectionsEqual(sections, bridge.lastSections ?? [])) {
            bridge.dirtySections = false;
          } else {
            const reply = await bridge.client.call<SidebarStateSnapshot>(
              methods.SET_SIDEBAR_SECTIONS,
              { profileKey, sections },
            );
            this.#applyFrame(bridge, reply);
            if (wireSectionsEqual(sections, this.#ownWireSections(bridge, profileKey))) {
              bridge.dirtySections = false;
            }
          }
        }
        if (!bridge.dirtyPins && !bridge.dirtySections) {
          break;
        }
        // A mid-flight edit changed the payload: re-read and send the fresh
        // value. Yield first so the settings listeners settle.
        await Promise.resolve();
      }
    } catch {
      // Offline or transport failure: the cache stays authoritative for
      // reads (the offline fallback), the surface stays dirty, and the
      // retry rides the next frame, mutation, or re-attach — never a busy
      // loop.
      failed = true;
    } finally {
      bridge.writeRunning = false;
      if (
        !failed &&
        !this.#disposed &&
        this.#bridges.get(bridge.key) === bridge &&
        (bridge.dirtyPins || bridge.dirtySections)
      ) {
        // A mutation landed while the loop was exiting — carry it. (A
        // failed call is NOT retried here: that path waits for a trigger.)
        this.#push(bridge);
      }
    }
  }

  /** An adoption write into the offline cache — never marks surfaces dirty. */
  #adopt(
    patch:
      | { sidebarPinnedSessionIdsByProfile: Readonly<Record<string, readonly string[]>> }
      | { sidebarSectionsByProfile: Readonly<Record<string, readonly SidebarSection[]>> },
  ): void {
    this.#applying += 1;
    try {
      this.#settings.update(patch, "immediate");
      const snapshot = this.#settings.getSnapshot();
      this.#lastPins = snapshot.sidebarPinnedSessionIdsByProfile;
      this.#lastSections = snapshot.sidebarSectionsByProfile;
    } finally {
      this.#applying -= 1;
    }
  }
}

/** Write one profile's bucket back into the map (an empty list drops out). */
function withBucket<T>(
  current: Readonly<Record<string, readonly T[]>>,
  profileKey: string,
  bucket: readonly T[],
): Record<string, readonly T[]> {
  const map: Record<string, readonly T[]> = { ...current };
  if (bucket.length === 0) {
    delete map[profileKey];
  } else {
    map[profileKey] = bucket;
  }
  return map;
}
