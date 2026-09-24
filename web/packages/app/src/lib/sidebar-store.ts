import type { StorageLike } from "./engine-store";
import { projectSidebarPinChange, retainKnownPins, type SidebarPinChange } from "./sidebar-pins";
import {
  assignSidebarSection,
  createSidebarSection,
  deleteSidebarSection,
  renameSidebarSection,
  setSidebarSectionCollapsed,
  validSectionName,
} from "./sidebar-sections";
import {
  UiSettingsStore,
  uiSettings,
  type SidebarOrganization,
  type SidebarSection,
  type SidebarSort,
  type UiSettings,
} from "../state/ui-settings";

/**
 * Browser-side sidebar UI state — the web peer of the desktop's
 * `settings.space_filter` / `settings.last_space_id` (ui-settings.json).
 * The space filter is the sidebar's space switcher AND the new-chat flow's
 * target: a chat created under a filter lands in that space; under "All
 * projects" it lands in the last selected (then first) space, or project-
 * less when the engine has no spaces. `archivedOpen` mirrors the desktop's
 * in-memory disclosure flag and is deliberately not persisted.
 *
 * The five sidebar view options (`sidebarOrganization`/`sidebarSort`/the
 * three Show toggles, settings.rs:507-544) ride along read-only here —
 * ticket 10's view menu writes them; the sidebar only reads.
 *
 * Persistence is the consolidated `state/ui-settings.ts` store — this class
 * owns the sidebar's *view* of it plus the one flag that never reaches
 * storage, not a `localStorage` key of its own.
 */

export interface SidebarState {
  /** The space the sidebar filters on; null = "All projects". */
  readonly spaceFilter: string | null;
  /** The last explicitly picked space — the new-chat fallback under "All". */
  readonly lastSpaceId: string | null;
  /** The archived shelf's disclosure (in-memory, like the desktop). */
  readonly archivedOpen: boolean;
  /**
   * The pinned section's disclosure — `Shell::pinned_open`: OPEN by default,
   * session-transient (in-memory, like the desktop's Archived shelf).
   */
  readonly pinnedOpen: boolean;
  /**
   * Device-local pinned sessions per workspace profile, in visual order
   * (`UiSettings::sidebar_pinned_session_ids_by_profile`; ui-settings, never
   * synced). Callers resolve the active bucket(s) off the fleet registry.
   */
  readonly pinnedByProfile: Readonly<Record<string, readonly string[]>>;
  /**
   * Custom sidebar sections per workspace profile, in order (upstream
   * 86249cf0's `sidebar_sections_by_profile`): device-local, profile-
   * isolated, never synced. Empty sections are retained so a "Drop sessions
   * here" target survives a reload.
   */
  readonly sectionsByProfile: Readonly<Record<string, readonly SidebarSection[]>>;
  /**
   * The create-section dialog's disclosure (in-memory, like the archived
   * shelf): `open_section_dialog(None, ..)` from the view menu's Create
   * Section row. The dialog itself renders in the chat list.
   */
  readonly sectionDialogOpen: boolean;
  /** ByDevice buckets the list under per-device disclosures; InOneList is flat. */
  readonly organization: SidebarOrganization;
  /** The comparator the active list, jump order, and archived shelf share. */
  readonly sort: SidebarSort;
  /** Upstream 78e9e6ae's display toggles — the view menu writes them. */
  readonly compact: boolean;
  readonly showProjectIcon: boolean;
  readonly showProjectLabel: boolean;
  readonly showHarness: boolean;
  readonly showBranch: boolean;
  readonly showPullRequest: boolean;
}

export interface SidebarStoreOptions {
  /** The settings store to read through; defaults to the app's singleton. */
  readonly settings?: UiSettingsStore;
  /** Convenience for tests: a settings store over this storage. */
  readonly storage?: StorageLike;
}

export class SidebarStore {
  readonly #settings: UiSettingsStore;
  #archivedOpen = false;
  // `Shell::pinned_open`: pins are visible by default (a pin the section
  // hides would be pointless), and the flag never reaches storage.
  #pinnedOpen = true;
  // `Shell::section_dialog` — in-memory like the archived shelf.
  #sectionDialogOpen = false;
  #state: SidebarState;
  readonly #listeners = new Set<() => void>();

  constructor(options: SidebarStoreOptions = {}) {
    this.#settings =
      options.settings ?? (options.storage === undefined ? uiSettings : new UiSettingsStore({ storage: options.storage }));
    this.#state = this.#project(this.#settings.getSnapshot());
    // Settings can move from elsewhere (a settings page, another view onto the
    // same fields) — re-project, and stay quiet when this slice did not move.
    this.#settings.subscribe(() => {
      this.#emit(this.#project(this.#settings.getSnapshot()));
    });
  }

  getSnapshot(): SidebarState {
    return this.#state;
  }

  subscribe(listener: () => void): () => void {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  }

  /**
   * Set the sidebar's space filter (null = All projects). A picked space
   * also becomes the last selected space — the new-chat target under "All".
   */
  setSpaceFilter(spaceId: string | null): void {
    if (spaceId === this.#state.spaceFilter) {
      return;
    }
    this.#settings.update(
      { spaceFilter: spaceId, lastSpaceId: spaceId ?? this.#state.lastSpaceId },
      "immediate",
    );
  }

  setArchivedOpen(open: boolean): void {
    if (open === this.#archivedOpen) {
      return;
    }
    this.#archivedOpen = open;
    this.#emit(this.#project(this.#settings.getSnapshot()));
  }

  /** `Shell::pinned_open`'s toggle: in-memory only, a no-op notifies nobody. */
  setPinnedOpen(open: boolean): void {
    if (open === this.#pinnedOpen) {
      return;
    }
    this.#pinnedOpen = open;
    this.#emit(this.#project(this.#settings.getSnapshot()));
  }

  /** `open_section_dialog(None, ..)` from the view menu's Create Section row. */
  openSectionDialog(): void {
    if (this.#sectionDialogOpen) {
      return;
    }
    this.#sectionDialogOpen = true;
    this.#emit(this.#project(this.#settings.getSnapshot()));
  }

  /** `section_dialog = None` — the dialog's own cancel/submit path. */
  closeSectionDialog(): void {
    if (!this.#sectionDialogOpen) {
      return;
    }
    this.#sectionDialogOpen = false;
    this.#emit(this.#project(this.#settings.getSnapshot()));
  }

  /**
   * `Shell::set_chat_pinned` (68306a17): one per-item intent — a Pin anchored
   * after the current last pin, or an Unpin — projected onto the profile's
   * bucket. An emptied bucket drops out of the map. A null profile key is
   * the desktop's "identity not ready" early return; a no-op writes nothing
   * (and notifies nobody).
   */
  setChatPinned(profileKey: string | null, chatId: string, pinned: boolean): void {
    if (profileKey === null) {
      return;
    }
    const current = this.#settings.getSnapshot().sidebarPinnedSessionIdsByProfile;
    const bucket = current[profileKey] ?? [];
    const sections = this.#sectionsFor(profileKey);
    // `active_sidebar_pins`: the pins the sidebar DRAWS exclude section
    // members (membership is exclusive in display) — the no-op check reads
    // the displayed set, like the desktop's.
    const displayed = bucket.filter(
      (id) => !sections.some((section) => section.sessionIds.includes(id)),
    );
    if (displayed.includes(chatId) === pinned) {
      return;
    }
    if (pinned && bucket.includes(chatId)) {
      // A chat whose pin already sits in the bucket but is claimed by a
      // section: re-pinning reclaims it for the pin (`set_chat_pinned`'s
      // assign-None early return — the pin write itself is a no-op).
      if (sections.some((section) => section.sessionIds.includes(chatId))) {
        this.#settings.update(
          { sidebarSectionsByProfile: this.#assignedSections(profileKey, chatId, null) },
          "immediate",
        );
      }
      return;
    }
    const change: SidebarPinChange = pinned
      ? {
          action: "pin",
          sessionId: chatId,
          after: bucket.length > 0 ? (bucket[bucket.length - 1] ?? null) : null,
          before: null,
        }
      : { action: "unpin", sessionId: chatId };
    const next = projectSidebarPinChange(bucket, change);
    const map: Record<string, readonly string[]> = { ...current };
    if (next.length === 0) {
      delete map[profileKey];
    } else {
      map[profileKey] = next;
    }
    // Membership is exclusive with pins: the local pin write lands in the
    // same `update`, so the chat leaves any custom section at the same
    // moment (the desktop's Local-scope `assign_sidebar_section(chat, None)`).
    const patch: Partial<UiSettings> = pinned
      ? {
          sidebarPinnedSessionIdsByProfile: map,
          sidebarSectionsByProfile: this.#assignedSections(profileKey, chatId, null),
        }
      : { sidebarPinnedSessionIdsByProfile: map };
    this.#settings.update(patch, "immediate");
  }

  // ── Custom sections (upstream 86249cf0, local-only) ─────────────────────

  /**
   * `submit_section_dialog`'s create arm: a trimmed, non-empty name (≤120
   * chars) appends an empty open section under the profile. Returns the
   * created section's id, or null when the name is invalid / the identity
   * is not ready.
   */
  createSection(profileKey: string | null, rawName: string): string | null {
    const name = validSectionName(rawName);
    if (profileKey === null || name === null) {
      return null;
    }
    const sections = this.#sectionsFor(profileKey);
    const id = sectionId();
    this.#settings.update(
      { sidebarSectionsByProfile: withSections(this.#settings.getSnapshot().sidebarSectionsByProfile, profileKey, createSidebarSection(sections, id, name)) },
      "immediate",
    );
    return id;
  }

  /** `submit_section_dialog`'s rename arm: a missing id is a no-op. */
  renameSection(profileKey: string | null, sectionId: string, rawName: string): boolean {
    const name = validSectionName(rawName);
    if (profileKey === null || name === null) {
      return false;
    }
    const sections = this.#sectionsFor(profileKey);
    if (!sections.some((section) => section.id === sectionId)) {
      return false;
    }
    this.#settings.update(
      { sidebarSectionsByProfile: withSections(this.#settings.getSnapshot().sidebarSectionsByProfile, profileKey, renameSidebarSection(sections, sectionId, name)) },
      "immediate",
    );
    return true;
  }

  /** `delete_sidebar_section`: the sessions survive in the regular groups. */
  deleteSection(profileKey: string | null, sectionId: string): void {
    if (profileKey === null) {
      return;
    }
    this.#settings.update(
      { sidebarSectionsByProfile: withSections(this.#settings.getSnapshot().sidebarSectionsByProfile, profileKey, deleteSidebarSection(this.#sectionsFor(profileKey), sectionId)) },
      "immediate",
    );
  }

  /** The section header's disclosure toggle (persisted, unlike the groups). */
  setSectionCollapsed(profileKey: string | null, sectionId: string, collapsed: boolean): void {
    if (profileKey === null) {
      return;
    }
    this.#settings.update(
      { sidebarSectionsByProfile: withSections(this.#settings.getSnapshot().sidebarSectionsByProfile, profileKey, setSidebarSectionCollapsed(this.#sectionsFor(profileKey), sectionId, collapsed)) },
      "immediate",
    );
  }

  /**
   * `assign_sidebar_section`: exclusive membership. Target null clears it
   * (a drop into Pinned or Regular); a target that vanished is refused.
   */
  assignSidebarSection(profileKey: string | null, chatId: string, target: string | null): void {
    if (profileKey === null) {
      return;
    }
    this.#settings.update(
      {
        sidebarSectionsByProfile: this.#assignedSections(profileKey, chatId, target),
      },
      "immediate",
    );
  }

  #sectionsFor(profileKey: string): readonly SidebarSection[] {
    return this.#settings.getSnapshot().sidebarSectionsByProfile[profileKey] ?? [];
  }

  #assignedSections(
    profileKey: string,
    chatId: string,
    target: string | null,
  ): Readonly<Record<string, readonly SidebarSection[]>> {
    return withSections(
      this.#settings.getSnapshot().sidebarSectionsByProfile,
      profileKey,
      assignSidebarSection(this.#sectionsFor(profileKey), chatId, target),
    );
  }

  /**
   * Settle the buckets after a committed drag reorder
   * (`commit_pinned_session_drag`): the input is `commitVisiblePinReorder`'s
   * output; emptied buckets drop out. A no-op writes nothing.
   */
  replacePinsByProfile(pinnedByProfile: Readonly<Record<string, readonly string[]>>): void {
    const clean: Record<string, readonly string[]> = {};
    for (const [key, ids] of Object.entries(pinnedByProfile)) {
      if (ids.length > 0) {
        clean[key] = ids;
      }
    }
    if (pinMapsEqual(this.#settings.getSnapshot().sidebarPinnedSessionIdsByProfile, clean)) {
      return;
    }
    this.#settings.update({ sidebarPinnedSessionIdsByProfile: clean }, "immediate");
  }

  /**
   * `retain_known_pins` on the desktop's synced-chats tick, per ACTIVE
   * profile: another profile's absent chats are not deletions. Archived ids
   * survive (unarchiving restores the pin); only a chat the loaded list
   * confirms deleted loses its pin. A no-op notifies nobody.
   */
  pruneUnknownPins(profileKeys: readonly string[], knownChatIds: ReadonlySet<string>): void {
    const current = this.#settings.getSnapshot().sidebarPinnedSessionIdsByProfile;
    let map: Record<string, readonly string[]> | null = null;
    for (const key of profileKeys) {
      const bucket = current[key];
      if (bucket === undefined) {
        continue;
      }
      const next = retainKnownPins([...bucket], knownChatIds);
      if (next !== null) {
        map ??= { ...current };
        if (next.length === 0) {
          delete map[key];
        } else {
          map[key] = next;
        }
      }
    }
    if (map !== null) {
      this.#settings.update({ sidebarPinnedSessionIdsByProfile: map }, "immediate");
    }
  }

  #project(settings: UiSettings): SidebarState {
    return {
      spaceFilter: settings.spaceFilter,
      lastSpaceId: settings.lastSpaceId,
      archivedOpen: this.#archivedOpen,
      pinnedOpen: this.#pinnedOpen,
      pinnedByProfile: settings.sidebarPinnedSessionIdsByProfile,
      sectionsByProfile: settings.sidebarSectionsByProfile,
      sectionDialogOpen: this.#sectionDialogOpen,
      organization: settings.sidebarOrganization,
      sort: settings.sidebarSort,
      compact: settings.sidebarCompact,
      showProjectIcon: settings.sidebarShowProjectIcon,
      showProjectLabel: settings.sidebarShowProjectLabel,
      showHarness: settings.sidebarShowHarness,
      showBranch: settings.sidebarShowBranch,
      showPullRequest: settings.sidebarShowPullRequest,
    };
  }

  #emit(state: SidebarState): void {
    if (
      state.spaceFilter === this.#state.spaceFilter &&
      state.lastSpaceId === this.#state.lastSpaceId &&
      state.archivedOpen === this.#state.archivedOpen &&
      state.pinnedOpen === this.#state.pinnedOpen &&
      // Healed snapshots allocate fresh containers per write — compare contents.
      pinMapsEqual(state.pinnedByProfile, this.#state.pinnedByProfile) &&
      sectionMapsEqual(state.sectionsByProfile, this.#state.sectionsByProfile) &&
      state.sectionDialogOpen === this.#state.sectionDialogOpen &&
      state.organization === this.#state.organization &&
      state.sort === this.#state.sort &&
      state.compact === this.#state.compact &&
      state.showProjectIcon === this.#state.showProjectIcon &&
      state.showProjectLabel === this.#state.showProjectLabel &&
      state.showHarness === this.#state.showHarness &&
      state.showBranch === this.#state.showBranch &&
      state.showPullRequest === this.#state.showPullRequest
    ) {
      return;
    }
    this.#state = state;
    for (const listener of this.#listeners) {
      listener();
    }
  }
}

/** Content comparison for per-profile pin maps (healed snapshots reallocate). */
function pinMapsEqual(
  left: Readonly<Record<string, readonly string[]>>,
  right: Readonly<Record<string, readonly string[]>>,
): boolean {
  const leftKeys = Object.keys(left);
  const rightKeys = Object.keys(right);
  return (
    leftKeys.length === rightKeys.length &&
    leftKeys.every((key) => {
      const leftBucket = left[key]!;
      const rightBucket = right[key];
      return (
        rightBucket !== undefined &&
        rightBucket.length === leftBucket.length &&
        leftBucket.every((id, ix) => id === rightBucket[ix])
      );
    })
  );
}

/** Write one profile's section list back into the map (empty list drops out). */
function withSections(
  current: Readonly<Record<string, readonly SidebarSection[]>>,
  profileKey: string,
  sections: readonly SidebarSection[],
): Readonly<Record<string, readonly SidebarSection[]>> {
  const map: Record<string, readonly SidebarSection[]> = { ...current };
  if (sections.length === 0) {
    delete map[profileKey];
  } else {
    map[profileKey] = sections;
  }
  return map;
}

/** Content comparison for per-profile section maps (order matters). */
function sectionMapsEqual(
  left: Readonly<Record<string, readonly SidebarSection[]>>,
  right: Readonly<Record<string, readonly SidebarSection[]>>,
): boolean {
  const leftKeys = Object.keys(left);
  const rightKeys = Object.keys(right);
  return (
    leftKeys.length === rightKeys.length &&
    leftKeys.every((key) => {
      const leftList = left[key]!;
      const rightList = right[key];
      return (
        rightList !== undefined &&
        rightList.length === leftList.length &&
        leftList.every((section, ix) => {
          const other = rightList[ix]!;
          return (
            section.id === other.id &&
            section.name === other.name &&
            section.collapsed === other.collapsed &&
            section.sessionIds.length === other.sessionIds.length &&
            section.sessionIds.every((id, ix2) => id === other.sessionIds[ix2])
          );
        })
      );
    })
  );
}

/** `uuid::Uuid::new_v4`'s web peer (crypto.randomUUID when present). */
function sectionId(): string {
  const crypto = (globalThis as { crypto?: { randomUUID?: () => string } }).crypto;
  if (crypto?.randomUUID !== undefined) {
    return crypto.randomUUID();
  }
  return `section-${Math.random().toString(36).slice(2)}${Date.now().toString(36)}`;
}
