/**
 * Custom sidebar sections — the web peer of the desktop's
 * `shell/sidebar_sections.rs` (upstream zeron 86249cf0, ported local-only:
 * NO sync). Sections are a device-local, presentation-only preference
 * persisted in `ui-settings.ts` under the active workspace profile, exactly
 * like the pin buckets; the pure CRUD, membership, grouping, and drop rules
 * live here so the components stay thin and the unit tests mirror the Rust
 * `sidebar_sections.rs` tests one-for-one.
 */
import type { SidebarSection } from "../state/ui-settings";

/** `settings.rs::SidebarSection`'s name bound (submit_section_dialog). */
export const SIDEBAR_SECTION_NAME_MAX = 120;

/** `sidebar_sections.rs::active_sidebar_sections` — the profile's list. */
export function activeSidebarSections(
  byProfile: Readonly<Record<string, readonly SidebarSection[]>>,
  profileKey: string | null,
): readonly SidebarSection[] {
  if (profileKey === null) {
    return [];
  }
  return byProfile[profileKey] ?? [];
}

/** `submit_section_dialog`'s name rule: trimmed, non-empty, ≤120 chars. */
export function validSectionName(raw: string): string | null {
  const name = raw.trim();
  if (name.length === 0 || [...name].length > SIDEBAR_SECTION_NAME_MAX) {
    return null;
  }
  return name;
}

/** `submit_section_dialog`'s create arm: append an empty open section. */
export function createSidebarSection(
  sections: readonly SidebarSection[],
  id: string,
  name: string,
): SidebarSection[] {
  return [...sections, { id, name, sessionIds: [], collapsed: false }];
}

/** `submit_section_dialog`'s rename arm: a missing id is a no-op. */
export function renameSidebarSection(
  sections: readonly SidebarSection[],
  id: string,
  name: string,
): SidebarSection[] {
  return sections.map((section) =>
    section.id === id ? { ...section, name } : section,
  );
}

/** `delete_sidebar_section`: membership dies with the section, never chats. */
export function deleteSidebarSection(
  sections: readonly SidebarSection[],
  id: string,
): SidebarSection[] {
  return sections.filter((section) => section.id !== id);
}

/** The section header's disclosure toggle (`render_custom_sidebar_section`). */
export function setSidebarSectionCollapsed(
  sections: readonly SidebarSection[],
  id: string,
  collapsed: boolean,
): SidebarSection[] {
  return sections.map((section) =>
    section.id === id ? { ...section, collapsed } : section,
  );
}

/**
 * `assign_sidebar_section`: exclusive membership — the chat leaves every
 * section, joins the target (which un-collapses to reveal it), and target
 * `null` means "regular" (membership cleared). A target that no longer
 * exists is the desktop's early return: the assignment is refused whole.
 */
export function assignSidebarSection(
  sections: readonly SidebarSection[],
  chatId: string,
  target: string | null,
): SidebarSection[] {
  if (target !== null && !sections.some((section) => section.id === target)) {
    return [...sections];
  }
  return sections.map((section) => {
    if (section.id === target) {
      const sessionIds = section.sessionIds.filter((id) => id !== chatId);
      sessionIds.push(chatId);
      return { ...section, sessionIds, collapsed: false };
    }
    return { ...section, sessionIds: section.sessionIds.filter((id) => id !== chatId) };
  });
}

/** The section that currently claims the chat, if any. */
export function sectionMembership(
  sections: readonly SidebarSection[],
  chatId: string,
): string | null {
  for (const section of sections) {
    if (section.sessionIds.includes(chatId)) {
      return section.id;
    }
  }
  return null;
}

/**
 * `render_active_rows`' custom groups: one bucket per section IN ORDER
 * holding the claimed rows (whatever the section lists, filtered to the
 * rows that exist), plus the remaining unclaimed rows for the regular
 * groups. Collapsed sections still claim their rows — they render as the
 * header alone but their members never fall through to the regular list.
 * A chat listed by two sections lands in the first (the healed store keeps
 * ids unique within a section; cross-section duplicates resolve first-wins).
 */
export function sectionRows<T extends { chat: { id: string } }>(
  sections: readonly SidebarSection[],
  rows: readonly T[],
): { groups: { section: SidebarSection; rows: T[] }[]; remaining: T[] } {
  const claimed = new Map<string, T[]>();
  for (const section of sections) {
    claimed.set(section.id, []);
  }
  const remaining: T[] = [];
  for (const row of rows) {
    let placed = false;
    for (const section of sections) {
      if (section.sessionIds.includes(row.chat.id)) {
        claimed.get(section.id)!.push(row);
        placed = true;
        break;
      }
    }
    if (!placed) {
      remaining.push(row);
    }
  }
  return {
    groups: sections.map((section) => ({
      section,
      rows: claimed.get(section.id) ?? [],
    })),
    remaining,
  };
}
