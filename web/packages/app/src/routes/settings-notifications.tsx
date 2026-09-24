import { RowTile } from "../components/settings-widgets";
import type { IconName } from "@zeron/icons";
import { RbSwitch } from "../components/base/switch";
import { uiSettings, useUiSettings } from "../state/ui-settings";
import { requestNotificationPermission } from "../lib/notifications";

/**
 * Notifications settings (desktop settings/notifications.rs parity): the
 * sound master + its three event rows, and the desktop-banner master + its
 * background-only row. Dependent rows render dimmed and inert while their
 * master is off (still named switches in the a11y tree, with the
 * "Unavailable while its parent setting is off" description); every flip
 * persists the COMPLETE set immediately — not a delta write. Ticket 30 owns
 * wiring these into actual chimes and banners.
 */

interface NotificationRow {
  readonly key:
    | "soundEnabled"
    | "soundCompletionEnabled"
    | "soundInputEnabled"
    | "soundAttentionEnabled"
    | "notificationsEnabled"
    | "notificationsBackgroundOnly";
  readonly title: string;
  readonly tile: IconName;
  readonly description: string;
  /** The master this row depends on; null = always interactive. */
  readonly master: "soundEnabled" | "notificationsEnabled" | null;
}

const ROWS: readonly NotificationRow[] = [
  {
    key: "soundEnabled",
    title: "Session sounds",
    tile: "volumeLoud",
    description: "Allow sounds for the selected session events below.",
    master: null,
  },
  {
    key: "soundCompletionEnabled",
    title: "Task completed",
    tile: "check",
    description: "Play a sound when an agent finishes a run.",
    master: "soundEnabled",
  },
  {
    key: "soundInputEnabled",
    title: "Input required",
    tile: "chatRoundLine",
    description: "Play a sound when an agent needs your response.",
    master: "soundEnabled",
  },
  {
    key: "soundAttentionEnabled",
    title: "Errors and disconnections",
    tile: "dangerTriangle",
    description: "Play a sound when a run fails or the connection remains unavailable.",
    master: "soundEnabled",
  },
  {
    key: "notificationsEnabled",
    title: "Desktop notifications",
    tile: "bell",
    description:
      "Show a system banner on the same events, so pings reach you while Zeron is in the background.",
    master: null,
  },
  {
    key: "notificationsBackgroundOnly",
    title: "Only when in the background",
    tile: "monitor",
    description: "Skip the banner while a Zeron window is focused.",
    master: "notificationsEnabled",
  },
];

export function NotificationsSettingsPage() {
  const settings = useUiSettings();

  function toggle(key: NotificationRow["key"]) {
    // The full set persists on every flip (NotificationsEvent::Changed).
    const patch = {
      soundEnabled: settings.soundEnabled,
      soundCompletionEnabled: settings.soundCompletionEnabled,
      soundInputEnabled: settings.soundInputEnabled,
      soundAttentionEnabled: settings.soundAttentionEnabled,
      notificationsEnabled: settings.notificationsEnabled,
      notificationsBackgroundOnly: settings.notificationsBackgroundOnly,
    };
    patch[key] = !settings[key];
    uiSettings.updateImmediate(patch);
    if (key === "notificationsEnabled" && patch.notificationsEnabled) {
      void requestNotificationPermission();
    }
  }

  return (
    <div className="settings-page">
      <h1 className="settings-title">Notifications</h1>
      <p className="settings-subtitle">
        Choose which session events can play a sound, and when desktop notifications appear.
      </p>

      <section className="settings-card">
        {ROWS.map((row) => {
          const enabled = settings[row.key];
          const masterOff = row.master !== null && !settings[row.master];
          return (
            <div key={row.key} className={`settings-row ${masterOff ? "settings-row-dim" : ""}`}>
              <RowTile icon={row.tile} />
              <div className="settings-row-main">
                <span className="settings-row-title">{row.title}</span>
                <span className="settings-row-meta">{row.description}</span>
              </div>
              {masterOff ? (
                <span
                  className={`toggle rb-switch-inert ${enabled ? "toggle-on" : ""}`}
                  role="switch"
                  aria-checked={enabled}
                  aria-label={row.title}
                  aria-description="Unavailable while its parent setting is off"
                >
                  <span className="toggle-thumb" />
                </span>
              ) : (
                <RbSwitch checked={enabled} onCheckedChange={() => toggle(row.key)} aria-label={row.title} />
              )}
            </div>
          );
        })}
      </section>
    </div>
  );
}
