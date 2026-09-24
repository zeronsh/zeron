/**
 * Shared pointer intent for nested menus — the web peer of the desktop's
 * `popover/hover_intent.rs` (upstream zeron f9563394). Feed trigger
 * enter/move events into `enter`/`moved`: open immediately on `"open"`, or
 * arm the caller's 300ms timer on `"defer"` (the deferred callback must
 * re-check that the source submenu is still open and this target is still
 * pending). Call `leave` on trigger exit and `cancel` on clicks/keyboard/
 * dismissal; `containsPointer` guards the safe trigger→child corridor for
 * outside-movement dismissal. Works for children on either side.
 *
 * The class itself stays synchronous and pure — the timer belongs to the
 * component, so tests drive the decision lattice without fake clocks.
 */

export type HoverAction = "none" | "open" | "defer";

/** The hover-intent grace period (`hover_intent.rs`'s 300ms timer). */
export const HOVER_INTENT_GRACE_MS = 300;

export interface Point {
  readonly x: number;
  readonly y: number;
}

export interface Bounds {
  readonly left: number;
  readonly top: number;
  readonly right: number;
  readonly bottom: number;
}

export class HoverIntent<K> {
  #origin: Point | null = null;
  #pending: K | null = null;
  #pointer: Point | null = null;

  cancel(): void {
    this.#pending = null;
    this.#pointer = null;
  }

  reset(): void {
    this.cancel();
    this.#origin = null;
  }

  recordOrigin(pointer: Point): void {
    this.#origin = pointer;
  }

  pending(): K | null {
    return this.#pending;
  }

  #towardChild(pointer: Point, bounds: Bounds | null, left: boolean): boolean {
    return this.#origin !== null && bounds !== null && corridor(this.#origin, pointer, bounds, left);
  }

  enter(current: K | null, target: K, pointer: Point, bounds: Bounds | null, left: boolean): HoverAction {
    this.cancel();
    if (current === target) {
      this.recordOrigin(pointer);
      return "none";
    }
    if (current !== null && this.#towardChild(pointer, bounds, left)) {
      this.#pending = target;
      this.#pointer = pointer;
      return "defer";
    }
    return "open";
  }

  moved(current: K | null, target: K, pointer: Point, bounds: Bounds | null, left: boolean): HoverAction {
    if (current === target) {
      this.recordOrigin(pointer);
      return "none";
    }
    if (this.#pending === target) {
      const previous = this.#pointer;
      const forward =
        previous === null ? 0 : (pointer.x - previous.x) * (left ? -1 : 1);
      if (!this.#towardChild(pointer, bounds, left) || forward <= -2) {
        this.cancel();
        return "open";
      }
      if (forward >= 2) {
        // Renew grace while progressing toward the child; a pause switches.
        return this.enter(current, target, pointer, bounds, left);
      }
    }
    return "none";
  }

  leave(target: K): void {
    if (this.#pending === target) {
      this.cancel();
    }
  }

  containsPointer(trigger: Bounds, child: Bounds | null, pointer: Point, left: boolean): boolean {
    if (
      pointer.x >= trigger.left &&
      pointer.x <= trigger.right &&
      pointer.y >= trigger.top &&
      pointer.y <= trigger.bottom
    ) {
      this.recordOrigin(pointer);
      return true;
    }
    if (child === null) {
      return true;
    }
    return (
      (pointer.x >= child.left &&
        pointer.x <= child.right &&
        pointer.y >= child.top &&
        pointer.y <= child.bottom) ||
      this.#towardChild(pointer, child, left)
    );
  }
}

/**
 * The triangle from the active trigger's last pointer position to the near
 * child edge (`hover_intent.rs::corridor`). Diagonal travel toward the
 * submenu is safe; unrelated space is not.
 */
export function corridor(origin: Point, pointer: Point, submenu: Bounds, onLeft: boolean): boolean {
  const edge = onLeft ? submenu.right : submenu.left;
  const direction = onLeft ? -1 : 1;
  const distance = (edge - origin.x) * direction;
  const advance = (pointer.x - origin.x) * direction;
  if (distance <= 0 || advance <= 0 || advance > distance + 8) {
    return false;
  }
  const fraction = Math.min(advance / distance, 1);
  const top = origin.y + (submenu.top - 8 - origin.y) * fraction;
  const bottom = origin.y + (submenu.bottom + 8 - origin.y) * fraction;
  return pointer.y >= top && pointer.y <= bottom;
}
