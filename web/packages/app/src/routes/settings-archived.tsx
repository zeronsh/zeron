import { useState } from "react";
import { Icon } from "@zeron/icons";
import { useEngineSession } from "../state/session-provider";
import { useNow, useWatchSnapshot } from "../state/hooks";
import { setChatArchived } from "../lib/chat-actions";
import { archivedChats } from "../lib/archived";
import { useUiSettings } from "../state/ui-settings";

/**
 * The full-page Archived sessions list (desktop settings/archived.rs) — a
 * different surface from the sidebar's shelf (`components/archived-section.tsx`,
 * which stays untouched): every archived chat across devices, unscoped by
 * the sidebar's space filter, with the fuller per-row content (device ·
 * location meta, the hover-reveal Unarchive pill) the shelf omits.
 */

export function ArchivedSettingsPage() {
  const session = useEngineSession();
  const client = session?.client ?? null;
  const snapshot = useWatchSnapshot(session);
  const settings = useUiSettings();
  const now = useNow(10_000);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const chats = snapshot?.chats;
  const devices = snapshot?.devices.rows ?? [];
  const rows =
    chats !== undefined && chats.error === null && chats.loaded
      ? archivedChats(chats.rows, devices, now, settings.sidebarSort)
      : [];

  function unarchive(chatId: string) {
    if (client === null || busy !== null) {
      return;
    }
    setBusy(chatId);
    setError(null);
    void (async () => {
      try {
        await setChatArchived(client, chatId, false);
      } catch (cause) {
        setError(`Unarchive failed: ${cause instanceof Error ? cause.message : String(cause)}`);
      } finally {
        setBusy(null);
      }
    })();
  }

  const count = rows.length;

  return (
    <div className="settings-page">
      <h1 className="settings-title">
        Archived sessions{count > 0 && <span className="settings-title-count">{count}</span>}
      </h1>
      <p className="settings-subtitle">
        Hidden from the sidebar, never deleted. Unarchiving puts a session back on its device.
      </p>

      {error !== null && (
        <p className="error-strip" role="alert" onClick={() => setError(null)}>
          {error}
        </p>
      )}

      {count === 0 ? (
        <div className="archived-empty" aria-live="polite">
          <Icon name="archiveMinimalistic" size={28} className="archived-empty-icon" />
          <p className="archived-empty-title">Nothing archived</p>
          <p className="archived-empty-hint">Right-click a session in the sidebar to archive it.</p>
        </div>
      ) : (
        <div className="archived-page-rows">
          {rows.map((row) => {
            const isBusy = busy === row.chat.id;
            return (
              <div key={row.chat.id} className="archived-page-row">
                <div className="archived-tile" aria-hidden="true">
                  <Icon name="archiveMinimalistic" size={16} className="archived-tile-icon" />
                </div>
                <div className="archived-row-main">
                  <div className="archived-row-title-line">
                    <span className="archived-row-title">{row.title}</span>
                    <span className="archived-row-time">{row.timeAgo}</span>
                  </div>
                  {(row.device !== null || row.location !== null) && (
                    <div className="archived-row-meta">
                      {row.device !== null && <span>{row.device}</span>}
                      {row.device !== null && row.location !== null && (
                        <span className="archived-row-meta-dot" aria-hidden="true">·</span>
                      )}
                      {row.location !== null && <span className="archived-row-location">{row.location}</span>}
                    </div>
                  )}
                </div>
                <button
                  type="button"
                  className={`archived-unarchive ${isBusy ? "archived-unarchive-busy" : ""}`}
                  onClick={() => unarchive(row.chat.id)}
                >
                  <Icon name="archiveUpMinimalistic" size={14} />
                  {isBusy ? "Unarchiving…" : "Unarchive"}
                </button>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
