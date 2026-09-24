import { useState } from "react";
import type { ReactElement } from "react";
import { Icon } from "@zeron/icons";
import { engineConnection, settingsEngineLabel } from "../lib/settings-engine";
import { setActiveDevice, useFleet, useFleetRegistry } from "../state/fleet";
import { PickerCard } from "./ui/PickerCard";
import { MenuRow } from "./ui/MenuRows";

/**
 * `SettingsEngineIndicator` — the `Engine {host}` pill the
 * engine-addressing settings pages (Remote access, Agents, Accounts)
 * carry beside their subtitle whenever more than one engine is paired.
 * The desktop needs nothing like it: its settings address the local
 * engine by construction (`remote_access.rs:37-39`). The web's settings
 * pages follow `fleet.active` — the last-paired engine, invisible and
 * unswitchable before this ticket — so the pill names the engine and
 * its popover lists every paired engine with its connection dot and a
 * check on the active one.
 *
 * Picking a row calls `setActiveDevice` — the store's first UI
 * caller for that write. Changing the active engine re-routes
 * `useEngineSession()` on `/settings/*` routes, so the page's data
 * reloads against the chosen engine (the pages already reset on session
 * identity change). The popover itself is the shared anchored-menu
 * family (`PickerCard`), so it rides the MENU_IN/MENU_OUT motion
 * catalog and honors `prefers-reduced-motion` like every other menu.
 */
export function SettingsEngineIndicator() {
  const fleet = useFleet();
  const registry = useFleetRegistry();
  const [open, setOpen] = useState(false);
  const label = settingsEngineLabel(fleet);
  if (label === null) {
    return null;
  }
  const byKey = new Map(registry.engines.map((engine) => [engine.key, engine]));

  const trigger: ReactElement = (
    <button
      type="button"
      className={`settings-engine-indicator ${open ? "settings-engine-indicator-open" : ""}`}
      aria-haspopup="menu"
      aria-expanded={open}
    >
      <span className="settings-engine-indicator-label">{`Engine ${label}`}</span>
      <Icon name="sortVertical" size={12} className="settings-engine-indicator-caret" />
    </button>
  );

  return (
    <PickerCard
      open={open}
      onOpenChange={setOpen}
      placement="anchorBelow"
      cardClassName="popover-card settings-engine-menu"
      role="menu"
      ariaLabel="Engines"
      width={220}
      initialFocus={false}
      overlaySource="settings-engine-indicator"
      trigger={trigger}
    >
      {fleet.engines.map((engine) => {
        const isActive = engine.baseUrl === fleet.active;
        return (
          <MenuRow
            key={engine.baseUrl}
            fadeKey={`settings-engine-${engine.baseUrl}`}
            selected={isActive}
            onClick={() => {
              setActiveDevice(engine.baseUrl);
              setOpen(false);
            }}
          >
            <span className={`dot ${engineConnection(byKey.get(engine.baseUrl) ?? null).dot}`} />
            <span className="settings-engine-row-host">{engine.label}</span>
            {isActive && <Icon name="check" size={12} className="settings-engine-row-check" />}
          </MenuRow>
        );
      })}
    </PickerCard>
  );
}
