/**
 * Message badges — `crates/ui/src/badges.rs::render` (:67-101) and the hover
 * card `BadgeCard` (:103-183), ported 1:1.
 *
 * The pill: 24px tall, radius 8, `ink(0.06)` plate, 12px MEDIUM muted text,
 * 12px icon at 70% — a stand-in for context the prompt folded in as text.
 * The hover card opens after the 280ms family delay (badges.rs:64) through
 * `PickerCard`'s hover-open shape (the content-card rule, base/tooltip.tsx):
 * a 320px frosted card with one row per comment — accent bar, mono location,
 * optional `L`/`R` tag pill, body at 12/16.
 */

import { useState } from "react";
import type { ReactNode } from "react";
import { Icon } from "@zeron/icons";
import { PickerCard } from "./ui/PickerCard";
import type { BadgeDetail, MessageBadge } from "../lib/badges";

/** `HOVER_DELAY` (badges.rs:64) — the tooltip family's 280ms. */
const HOVER_DELAY_MS = 280;

/** `BADGE_HEIGHT` (badges.rs:58) — the composer sizes its strip arithmetically. */
export const BADGE_HEIGHT = 24;

/**
 * The pills for one user row, inside ticket 18's strip container
 * (`badges::render`'s callers). `null` when the prompt carried nothing.
 */
export function MessageBadges({ badges }: { badges: readonly MessageBadge[] }): ReactNode {
  if (badges.length === 0) {
    return null;
  }
  return (
    <div className="user-badges">
      {badges.map((badge, bix) => (
        <BadgePill key={bix} badge={badge} />
      ))}
    </div>
  );
}

/**
 * One pill. Empty `details` means the label says everything — no card, no
 * hover chrome (badges.rs:91-100).
 */
export function BadgePill({ badge }: { badge: MessageBadge }) {
  const hasDetails = badge.details.length > 0;
  const [open, setOpen] = useState(false);
  const pill = (
    <div className="badge-pill">
      <Icon name={badge.icon} size={12} className="badge-pill-icon" />
      <span>{badge.label}</span>
    </div>
  );
  if (!hasDetails) {
    return pill;
  }
  return (
    <PickerCard
      open={open}
      onOpenChange={setOpen}
      placement={{ side: "top", align: "center" }}
      cardClassName="popover-card badge-card"
      ariaLabel={badge.label}
      width={320}
      openOnHover
      hoverDelayMs={HOVER_DELAY_MS}
      trigger={pill}
    >
      <BadgeCard details={badge.details} />
    </PickerCard>
  );
}

/**
 * The hover card (badges.rs:103-183): `popover_card w(320) p(6) flex flex_col
 * gap(4)`, frosted — the frosted wrap is load-bearing on the desktop because
 * it gives the card its own scene layer; the web's equivalent is the CSS
 * stacking context `.popover-card` already carries.
 */
export function BadgeCard({ details }: { details: readonly BadgeDetail[] }) {
  return (
    <div className="badge-card">
      {details.map((detail, ix) => (
        <BadgeCardRow key={ix} detail={detail} />
      ))}
    </div>
  );
}

/** One card row (badges.rs:108-164). */
function BadgeCardRow({ detail }: { detail: BadgeDetail }) {
  return (
    <div className="badge-card-row">
      <span className="badge-card-bar" aria-hidden />
      <div className="badge-card-column">
        <div className="badge-card-location">
          <span className="badge-card-path">{detail.location}</span>
          {detail.tag !== null && <span className="badge-card-tag">{detail.tag}</span>}
        </div>
        <div className="badge-card-body">{detail.body}</div>
      </div>
    </div>
  );
}
