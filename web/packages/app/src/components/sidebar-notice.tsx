import { sidebarNotice, useSidebarNotice } from "../state/notice";

/** The inline mutation-failure strip; click dismisses (shell.rs sidebar_notice). */
export function SidebarNotice() {
  const notice = useSidebarNotice();
  if (notice === null) {
    return null;
  }
  return (
    <button type="button" className="sidebar-notice" onClick={() => sidebarNotice.clear()}>
      {notice}
    </button>
  );
}
