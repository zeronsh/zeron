import { Outlet } from "@tanstack/react-router";

/**
 * The settings route shell — just the scrolling section outlet. The section
 * nav is the SIDEBAR's content on `/settings/*` (the desktop's
 * `render_settings_nav` swap, shell.rs:993-1008 — see
 * `components/settings-nav.tsx`); the main column carries only the pages,
 * padded below the overlaid titlebar like every other route.
 */
export function SettingsLayout() {
  return (
    <div className="settings-scroll">
      <Outlet />
    </div>
  );
}
