import { useEffect, useRef } from "react";
import { ChatList } from "./chat-list";
import { SidebarViewMenu, SpaceFilter } from "./space-filter";
import { NewChatListener } from "./new-chat-button";
import { ArchivedSection } from "./archived-section";
import { SidebarNotice } from "./sidebar-notice";
import { AccountRow } from "./account-row";
import { ConnectionPill } from "./connection-pill";
import { UpdateStrip } from "./update-strip";
import { AddSpacePalette } from "./add-space-palette";
import { CommandPalette } from "./command-palette";

/**
 * The sidebar's column — the desktop's `render_chat_sidebar`: the space
 * filter header (the trigger plus the view-options button in one row), the
 * global active-chat list, the archived shelf, the notice strip, the
 * connection line, the update strip, and the user menu pinned to the bottom.
 *
 * The filter row sits ABOVE the scroll region (the desktop pins it there so
 * its dropdown floats unclipped by the list's overflow — and so the edge
 * fade never touches it); the list region inside it is the desktop's
 * `edge_faded` scope, a quadratic ramp per overflow edge, gated live from
 * the scroll offset with a 1px dead-zone (`edge_fade.rs`).
 *
 * New-chat creation is NOT here: the desktop's titlebar owns that action in
 * both sidebar states, so the `+` lives in `Titlebar` and this column only
 * lists. Not keyed by engine either — the sidebar reads the fleet-merged
 * snapshot and persists across engine switches (ticket 43).
 *
 * The add-space palette mounts here as a sibling (like `SidebarNotice`) —
 * headless while closed; the spaces menu's "New project…" row (ticket 10)
 * and the `Mod+Shift+N` binding open it through `addSpaceStore`. The
 * command palette (ticket 16) mounts beside it — `Mod+K` toggles it
 * through `commandPaletteStore`.
 */
export function SidebarBody() {
  return (
    <>
      <NewChatListener />
      {/* `render_spaces_filter`'s container row: the trigger and the
          view-options button over a 4px gap, 8px inline padding. */}
      <div className="space-filter">
        <SpaceFilter />
        <SidebarViewMenu />
      </div>
      <SidebarScroll>
        <ChatList />
        <ArchivedSection />
      </SidebarScroll>
      <ConnectionPill />
      <SidebarNotice />
      <UpdateStrip />
      <AccountRow />
      <AddSpacePalette />
      <CommandPalette />
    </>
  );
}

/**
 * The scroll region and its edge fade (`edge_fade.rs`): the wrapper carries
 * the quadratic mask, gated per edge by `--rb-sidebar-fade-top`/`-bottom`
 * (0/1), refreshed from the live scroll offset on every scroll frame and
 * whenever the content resizes. Both start closed — an unscrolled list
 * never dims its first row.
 */
function SidebarScroll({ children }: { children: React.ReactNode }) {
  const wrapRef = useRef<HTMLDivElement | null>(null);
  const scrollRef = useRef<HTMLElement | null>(null);
  useEffect(() => {
    const wrap = wrapRef.current;
    const scroller = scrollRef.current;
    if (wrap === null || scroller === null) {
      return;
    }
    let raf = 0;
    const apply = (): void => {
      raf = 0;
      // The 1px dead-zone: an edge fades only when there is more than a
      // pixel of content past it.
      const top = scroller.scrollTop > 1.0;
      const bottom = scroller.scrollTop < scroller.scrollHeight - scroller.clientHeight - 1.0;
      wrap.style.setProperty("--rb-sidebar-fade-top", top ? "1" : "0");
      wrap.style.setProperty("--rb-sidebar-fade-bottom", bottom ? "1" : "0");
    };
    const schedule = (): void => {
      if (raf === 0) {
        raf = requestAnimationFrame(apply);
      }
    };
    const observer = new ResizeObserver(schedule);
    observer.observe(scroller);
    scroller.addEventListener("scroll", schedule, { passive: true });
    apply();
    return () => {
      observer.disconnect();
      scroller.removeEventListener("scroll", schedule);
      if (raf !== 0) {
        cancelAnimationFrame(raf);
      }
    };
  }, []);

  return (
    <div className="sidebar-scroll" ref={wrapRef}>
      <nav className="sidebar-list" aria-label="Chats" ref={scrollRef}>
        {children}
      </nav>
    </div>
  );
}
