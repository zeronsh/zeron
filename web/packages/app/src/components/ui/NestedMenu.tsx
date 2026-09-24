/**
 * `NestedMenu` — the portaled nested submenu (ticket 01): the web peer of
 * desktop `popover::nested_menu` (popover.rs:584-612) for the
 * cursor-keyboard surfaces (`PickerCard` cards, plain relative hosts). A
 * row that opens its own choices — the view menu's groups (ticket 07),
 * the model picker's settings tray (ticket 08) — renders through here
 * instead of an inline absolute div that an `overflow: hidden` ancestor
 * clips away (research bugs 5b/9/2's shared root cause: `.popover-card`
 * and `.sidebar` clip everything their inline-absolute children paint).
 *
 * TWO arms, resolved HERE (the consumer never branches — ticket 15's
 * mobile surface rule):
 * - **Desktop:** the choices portal to `document.body` through the nested
 *   `RbPopover` (Base UI Popover, `modal: false`, the no-flip clamp-only
 *   preset) — `nestedMenuPlacement` pins the card beside the trigger row
 *   (right by default; `side: "left"` when the row sits near the window's
 *   right edge), top-aligned, 6px of card-to-card gap, riding the menu
 *   z-tier through `.rb-popover-positioner`. The portal is what escapes
 *   the parent card's clip box. Base UI keeps the flyout linked to the
 *   parent popup tree, which encodes popover.rs:586-589's contract ("a
 *   nested menu shares the parent's interaction surface") structurally:
 *   presses inside the flyout never dismiss the parent card, and Escape
 *   closes the nested menu alone (`pickers.rs`'s close-just-the-nested
 *   behavior), with no hand-rolled guards.
 * - **Phone (≤768px, `useIsPhone`):** no portal, no flyout — the choices
 *   expand IN PLACE inside the enclosing drawer sheet under a header row
 *   carrying the back affordance (altArrowLeft + `label`). The trigger
 *   row's press toggles the block through the same `onOpenChange` seam
 *   the desktop arm rides; the header's press closes it.
 *
 * The rest of the contract is `RbPopover`'s, verbatim: controlled `open`,
 * the `trigger-press`/`outside-press`/`escape-key` reason set, the
 * `[data-open]`/`[data-closed]` exit window, `overlaySource`, the
 * escape/focus split, the `focus-out` veto. Keyboard stays the consumer's
 * cursor model (`ui/CursorList.tsx`): the flyout takes no focus by
 * default (`initialFocus: false`), so the parent card's focused element
 * keeps receiving the walk while the nested card is open; `onKeyDown`
 * covers a surface that focuses the flyout itself. N-level nesting works
 * — each level portals and links to its own parent the same way.
 */

import {
  cloneElement,
  useState,
  type CSSProperties,
  type HTMLAttributes,
  type KeyboardEvent as ReactKeyboardEvent,
  type MouseEvent as ReactMouseEvent,
  type ReactElement,
  type ReactNode,
} from "react";
import { Icon } from "@zeron/icons";
import {
  createRbPopoverHandle,
  nestedMenuPlacement,
  RbPopover,
  RbPopoverTrigger,
  type NestedMenuSide,
  type RbPopoverProps,
} from "../base/popover";
import { useIsPhone } from "../../state/media";

/**
 * Base UI's own submenu hover preset (MenuSubmenuTrigger's default): the
 * row opens its flyout 100ms after the pointer settles on it.
 */
const NESTED_HOVER_OPEN_MS = 100;
/**
 * The corridor grace: a pointer crossing the 6px trigger→flyout gap must
 * not drop the flyout mid-transit (the desktop's hover-intent corridor,
 * popover/hover_intent.rs, as Base UI's close delay).
 */
const NESTED_HOVER_CLOSE_MS = 100;

export interface NestedMenuProps {
  /** Controlled open — the caller owns every open/close transition. */
  readonly open: boolean;
  /**
   * Base UI's Popover change event, verbatim on the desktop arm (reasons
   * `trigger-press`, `trigger-hover`, `outside-press`, `escape-key`, …).
   * The phone arm synthesizes `trigger-press` (row press) and
   * `close-press` (back header) with the same shape — consumers read only
   * the open flag, the shared-wrappers convention
   * (`base/responsive-surface.tsx`).
   */
  readonly onOpenChange: RbPopoverProps["onOpenChange"];
  /**
   * The trigger row element, adopted via `RbPopoverTrigger`'s `render` on
   * desktop (its className/handlers/children/ref stay its own; the press
   * toggles) and composed with the toggle press on phone.
   */
  readonly trigger: ReactElement;
  /**
   * The group's name — the phone drill-down header's title (and the
   * drill region's accessible name fallback). Falls back to `ariaLabel`.
   */
  readonly label?: ReactNode;
  /**
   * The flyout's `menu_heading` (popover.rs:771-795) — both of the
   * desktop's `nested_menu` consumers head their flyouts with the group's
   * name (pickers.rs:4036's traits tray, spaces.rs:2496's view menu).
   * Desktop arm only: the phone drill's header already carries the label,
   * so the arm would otherwise say it twice.
   */
  readonly heading?: ReactNode;
  /**
   * The side the desktop flyout opens on — `nested_menu`'s `left: bool`
   * (popover.rs:591-595): callers choose the side that has room; default
   * right.
   */
  readonly side?: NestedMenuSide;
  /**
   * Also open the flyout on hover (desktop arm; default `true` — the
   * desktop's nested menus open on hover intent). `hoverDelayMs`/
   * `closeDelayMs` tune the intent window and the corridor grace.
   */
  readonly openOnHover?: boolean;
  /** The hover-open delay in ms; requires `openOnHover`. Default 100. */
  readonly hoverDelayMs?: number;
  /** The hover-close grace in ms (the gap crossing). Default 100. */
  readonly closeDelayMs?: number;
  /**
   * Whether the adopted trigger row is a native button — `false` for div
   * rows (Base UI's Trigger must not force button semantics onto them);
   * default `true`.
   */
  readonly nativeButton?: boolean;
  /** The flyout card's class list beyond the base `.popover-card`. */
  readonly cardClassName?: string;
  /** The flyout's ARIA role; default `"menu"`. */
  readonly role?: string;
  /** The flyout/drill region's accessible name. */
  readonly ariaLabel?: string;
  /** The flyout's width — folded into the popup's inline style (desktop). */
  readonly width?: number;
  /** Inline style on the flyout card beyond `width` (maxHeight…). */
  readonly style?: CSSProperties;
  /**
   * Registers this name on the `overlayKeyboard` registry while the flyout
   * is open — usually OMITTED: the enclosing card already claims the
   * scope, and the flyout only exists while that card is open.
   */
  readonly overlaySource?: string;
  /**
   * Per-surface focus-on-open — default `false` (keep focus put): the
   * cursor keyboard model lives on the parent card's focused element.
   */
  readonly initialFocus?: RbPopoverProps["initialFocus"];
  /** Where Escape returns focus (the desktop's escape split). */
  readonly escapeFocusTarget?: RbPopoverProps["escapeFocusTarget"];
  /** The desktop's `motion::speed_scale`; rescales the exit duration. */
  readonly motionSpeed?: number;
  /**
   * The flyout card frame's key handler — for a surface that focuses the
   * flyout itself; the cursor model otherwise stays on the parent card.
   */
  readonly onKeyDown?: (event: ReactKeyboardEvent<HTMLDivElement>) => void;
  readonly children: ReactNode;
}

/**
 * The phone arm's stand-in change event: no Base UI pipeline runs there,
 * so the row press and the back header synthesize the reason the consumer
 * would see from the equivalent desktop paths. Consumers read only the
 * open flag (the convention the responsive wrappers share); the cast
 * mirrors `drawerOnOpenChange`'s narrowing.
 */
function phoneChangeDetails(
  reason: "trigger-press" | "close-press",
  event?: Event,
): Parameters<RbPopoverProps["onOpenChange"]>[1] {
  return {
    reason,
    event: event ?? null,
    trigger: undefined,
    cancel() {},
    isCanceled: false,
    allowPropagation() {},
    isPropagationAllowed: false,
    preventUnmountOnClose() {},
  } as Parameters<RbPopoverProps["onOpenChange"]>[1];
}

/** `NestedMenu` — the nested submenu with both arms resolved in one unit. */
export function NestedMenu(props: NestedMenuProps) {
  const isPhone = useIsPhone();
  if (isPhone) {
    return <NestedMenuPhone {...props} />;
  }
  return <NestedMenuDesktop {...props} />;
}

/** The desktop arm: the row adopted as the flyout's trigger, the card portaled beside it. */
function NestedMenuDesktop(props: NestedMenuProps) {
  const [handle] = useState(() => createRbPopoverHandle());
  const style: CSSProperties | undefined =
    props.width === undefined ? props.style : { ...props.style, width: props.width };
  return (
    <>
      <RbPopoverTrigger
        handle={handle}
        render={props.trigger}
        nativeButton={props.nativeButton}
        openOnHover={props.openOnHover ?? true}
        delay={props.hoverDelayMs ?? NESTED_HOVER_OPEN_MS}
        closeDelay={props.closeDelayMs ?? NESTED_HOVER_CLOSE_MS}
      />
      <RbPopover
        handle={handle}
        open={props.open}
        onOpenChange={props.onOpenChange}
        placement={nestedMenuPlacement(props.side ?? "right")}
        cardClassName={props.cardClassName}
        role={props.role ?? "menu"}
        ariaLabel={props.ariaLabel}
        style={style}
        overlaySource={props.overlaySource}
        initialFocus={props.initialFocus ?? false}
        escapeFocusTarget={props.escapeFocusTarget}
        motionSpeed={props.motionSpeed}
        onKeyDown={props.onKeyDown}
      >
        {props.heading}
        {props.children}
      </RbPopover>
    </>
  );
}

/** The row-patch shape the phone arm composes onto the adopted trigger element. */
type AdoptedRowProps = HTMLAttributes<HTMLElement>;

/**
 * The phone arm: the drill-down. The row renders in place with the toggle
 * press composed onto its own handlers (the desktop arm gets the same
 * toggle from Base UI's Trigger); while open, the choices expand under a
 * back-affordance header — never a side flyout (ticket 15).
 */
function NestedMenuPhone(props: NestedMenuProps) {
  const row = cloneElement(props.trigger as ReactElement<AdoptedRowProps>, {
    "aria-expanded": props.open,
    onClick: (event: ReactMouseEvent<HTMLElement>) => {
      const own = (props.trigger.props as { onClick?: (event: ReactMouseEvent<HTMLElement>) => void })
        .onClick;
      own?.(event);
      if (!event.defaultPrevented) {
        props.onOpenChange(!props.open, phoneChangeDetails("trigger-press", event.nativeEvent));
      }
    },
  });
  if (!props.open) {
    return row;
  }
  const title = props.label ?? props.ariaLabel;
  const drillName = props.ariaLabel ?? (typeof props.label === "string" ? props.label : undefined);
  return (
    <>
      {row}
      <div className="rb-submenu-drill" role="group" aria-label={drillName}>
        <button
          type="button"
          className="menu-row rb-submenu-drill-header"
          aria-label={typeof title === "string" ? `Back to ${title}` : "Back"}
          onClick={(event) => {
            if (!event.defaultPrevented) {
              props.onOpenChange(false, phoneChangeDetails("close-press", event.nativeEvent));
            }
          }}
        >
          <Icon name="altArrowLeft" size={12} className="rb-submenu-drill-back" />
          {title !== undefined && <span className="rb-submenu-drill-title">{title}</span>}
        </button>
        <div className="rb-submenu-drill-body">{props.children}</div>
      </div>
    </>
  );
}
