/**
 * `RbSwitch` — the settings toggle row's switch on Base UI's Switch
 * (Notifications ×6, Agents enable, Files autosave/word-wrap, Shortcuts
 * escape-behavior — ticket 29). The desktop's `toggle_switch` geometry is
 * already ported as the `.toggle` recipe (32×18 track, 14px thumb, 2px
 * inset, `HOVER_FADE` slide); this wrapper renders Base UI's parts onto
 * those exact classes so the CSS carries over verbatim.
 *
 * The parity rules, encoded ONCE:
 * - **Classes:** the Root wears `.toggle` (plus `rb-switch` for the
 *   state-keyed rules), the Thumb `.toggle-thumb`. The checked state moves
 *   the thumb and fills the track through `[data-checked]` CSS — the
 *   hand-rolled `.toggle-on` class juggling is gone, the geometry is not.
 * - **Controlled:** `checked` + `onCheckedChange` (Base UI's change event
 *   carries the reason); `defaultChecked` for the uncontrolled rows.
 * - Switch is not an overlay: no overlayKeyboard registration, no
 *   positioning, no motion hooks.
 */

import type { ComponentProps } from "react";
import { Switch, type SwitchRootProps } from "@base-ui/react/switch";

export interface RbSwitchProps extends Omit<SwitchRootProps, "children" | "className"> {
  /** Extra classes on the track beyond `.toggle rb-switch`. */
  readonly className?: string;
  /** Extra classes on the thumb beyond `.toggle-thumb`. */
  readonly thumbClassName?: string;
  /** The thumb's children (an icon, if a surface ever needs one). */
  readonly thumbChildren?: React.ReactNode;
}

/** `RbSwitch` — the 32×18 toggle_switch on Base UI's parts. */
export function RbSwitch(props: RbSwitchProps) {
  const { className, thumbClassName, thumbChildren, ...rest } = props;
  return (
    <Switch.Root className={`toggle rb-switch ${className ?? ""}`} {...rest}>
      <Switch.Thumb className={`toggle-thumb ${thumbClassName ?? ""}`}>{thumbChildren}</Switch.Thumb>
    </Switch.Root>
  );
}

/** The raw thumb part for consumers that compose their own track. */
export function RbSwitchThumb(props: ComponentProps<typeof Switch.Thumb>) {
  const { className, ...rest } = props;
  return <Switch.Thumb className={`toggle-thumb ${className ?? ""}`} {...rest} />;
}
