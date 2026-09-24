import { useState } from "react";
import type { ReactElement } from "react";
import { Icon } from "@zeron/icons";
import { PickerCard } from "./PickerCard";
import { MenuHeading, MenuRow } from "./MenuRows";
import { platformGlyph } from "../../lib/devices";

/**
 * `render_device_switcher` (accounts.rs:270-410 / harnesses.rs:430-581) —
 * the page-header device picker both Accounts and Agents (Settings → Agents)
 * carry, duplicated per page on the desktop. Web shape: data + callbacks in
 * (the devices list, the local id, the effective target), DOM out; the
 * caller owns the retarget. The 220px popup, the trigger geometry
 * (16px glyph · 12.5px Medium label · 6px presence dot · 14px chevron), and
 * the row order (registration time, then id) are the desktop's verbatim.
 */

export interface DeviceSwitcherProps {
  /** Every registered device, in registry order (the caller sorts). */
  readonly devices: readonly { id: string; name: string; platform: string }[];
  /** The connected engine's own device id; null before the first EngineInfo. */
  readonly localDeviceId: string | null;
  /** The page's explicit target device; null = this device (no passthrough). */
  readonly target: string | null;
  /** Retarget the page (set_target_device). `null` returns to the local device. */
  readonly onTargetChange: (target: string | null) => void;
}

export function DeviceSwitcher(props: DeviceSwitcherProps) {
  const [open, setOpen] = useState(false);
  const effective = props.target ?? props.localDeviceId;
  const selected = props.devices.find((device) => device.id === effective) ?? null;
  const triggerGlyph = platformGlyph(selected?.platform ?? "macos");
  const triggerLabel = selected?.name ?? "This device";
  const isLocal = effective !== null && effective === props.localDeviceId;

  const trigger: ReactElement = (
    <button
      type="button"
      className={`device-switcher-trigger ${open ? "device-switcher-trigger-open" : ""}`}
      aria-haspopup="menu"
      aria-expanded={open}
      aria-label={`Target device: ${triggerLabel}`}
    >
      <Icon name={triggerGlyph} size={16} className="device-switcher-glyph" />
      <span className="device-switcher-label">{triggerLabel}</span>
      <span className={`device-switcher-dot ${isLocal ? "device-switcher-dot-local" : ""}`} />
      <Icon name="sortVertical" size={14} className="device-switcher-caret" />
    </button>
  );

  return (
    <PickerCard
      open={open}
      onOpenChange={setOpen}
      placement="anchorBelow"
      cardClassName="popover-card device-switcher-menu"
      role="menu"
      ariaLabel="Devices"
      width={220}
      initialFocus={false}
      overlaySource="device-switcher"
      trigger={trigger}
    >
      <MenuHeading>Devices</MenuHeading>
      {props.devices.map((device, ix) => {
        const isActive = device.id === effective;
        const deviceIsLocal = device.id === props.localDeviceId;
        return (
          <MenuRow
            key={device.id}
            fadeKey={`device-row-${ix}`}
            selected={isActive}
            onClick={() => {
              // Local device = no passthrough (calls stay direct).
              props.onTargetChange(deviceIsLocal ? null : device.id);
              setOpen(false);
            }}
          >
            <Icon name={platformGlyph(device.platform)} size={16} className="device-switcher-glyph" />
            <span className="device-switcher-row-name">{device.name}</span>
            {deviceIsLocal && <span className="device-switcher-row-you">You</span>}
            <span className={`device-switcher-dot ${deviceIsLocal ? "device-switcher-dot-local" : ""}`} />
          </MenuRow>
        );
      })}
    </PickerCard>
  );
}
