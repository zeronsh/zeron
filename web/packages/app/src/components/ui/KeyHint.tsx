/**
 * The key-cap family — ports of `popover.rs:841-940`: `key_cap`
 * (`:841-852`), `key_hint_label` (`:855-860`), `key_hint` (`:864-878`),
 * `key_hint_text` (`:882-896`), `key_hint_pair` (`:900-926`), and
 * `kbd_hint` (`:929-940`). Footer legends and in-row accelerator chips.
 * Geometry lives in `styles/app.css` next to each class.
 */

import type { ReactNode } from "react";

/** `key_cap` (`popover.rs:841-852`) — one 22px cap holding arbitrary children. */
export function KeyCap(props: { children: ReactNode }) {
  return <span className="key-cap">{props.children}</span>;
}

/** `key_hint_label` (`popover.rs:855-860`) — the tiny verb after a key-cap. */
export function KeyHintLabel(props: { children: ReactNode }) {
  return <span className="key-hint-label">{props.children}</span>;
}

/** `key_hint` (`popover.rs:864-878`) — a footer legend: cap + tiny verb.
 * The cap content (an icon) is the caller's. */
export function KeyHint(props: { cap: ReactNode; label: ReactNode }) {
  return (
    <span className="key-hint">
      <KeyCap>{props.cap}</KeyCap>
      <KeyHintLabel>{props.label}</KeyHintLabel>
    </span>
  );
}

/** `key_hint_text` (`popover.rs:882-896`) — a cap holding a WORD ("tab",
 * "esc") in the monospace font. */
export function KeyHintText(props: { cap: string; label: ReactNode }) {
  return (
    <span className="key-hint">
      <KeyCap>
        <span className="key-cap-word">{props.cap}</span>
      </KeyCap>
      <KeyHintLabel>{props.label}</KeyHintLabel>
    </span>
  );
}

/** `key_hint_pair` (`popover.rs:900-926`) — a cap holding two glyphs split
 * by a hairline divider. */
export function KeyHintPair(props: { first: ReactNode; second: ReactNode; label: ReactNode }) {
  return (
    <span className="key-hint">
      <KeyCap>
        <span className="key-cap-icon">{props.first}</span>
        <span className="key-cap-divider" />
        <span className="key-cap-icon">{props.second}</span>
      </KeyCap>
      <KeyHintLabel>{props.label}</KeyHintLabel>
    </span>
  );
}

/** `kbd_hint` (`popover.rs:929-940`) — the muted accelerator chip inside
 * menu rows ("⌘1"-style). */
export function KbdHint(props: { children: ReactNode }) {
  return <span className="kbd-hint">{props.children}</span>;
}
