import { useCallback } from "react";
import { Icon } from "@zeron/icons";

import { noticeChipModel, type NoticeChipTone, type NoticeChipVariant } from "../lib/notice-chip";

/**
 * The shared stacked notice chip (desktop `notice.rs::notice_chip`): header
 * row (DangerTriangle + medium label + copy button pinned top-right) over the
 * wrapping message body. The composer chains its own chrome (id, dismiss
 * click); the transcript mounts the tile variant with no dismiss.
 *
 * The copy button stops propagation: the composer chip dismisses on click
 * and copying must not take the notice with it.
 */
export function NoticeChip({
  tone,
  variant,
  label,
  message,
  id,
  role,
  onClick,
}: {
  readonly tone: NoticeChipTone;
  readonly variant: NoticeChipVariant;
  readonly label: string;
  readonly message: string;
  readonly id?: string;
  readonly role?: string;
  readonly onClick?: () => void;
}) {
  const model = noticeChipModel({ tone, variant, label, message });
  const copy = useCallback(
    (event: React.MouseEvent) => {
      event.stopPropagation();
      void navigator.clipboard?.writeText(message);
    },
    [message],
  );
  const classes = [
    "notice-chip",
    `notice-chip-${variant}`,
    tone === "warning" ? "notice-chip-warning" : "",
    onClick ? "notice-chip-dismissible" : "",
  ]
    .filter((c) => c !== "")
    .join(" ");
  return (
    <div className={classes} id={id} role={role} onClick={onClick}>
      <div className="notice-chip-header">
        {variant === "tile" ? (
          <span className="notice-chip-tile-icon" aria-hidden>
            <Icon name="dangerTriangle" size={model.iconSize} />
          </span>
        ) : (
          <Icon name="dangerTriangle" size={model.iconSize} className="notice-chip-icon" />
        )}
        <span className="notice-chip-label">{label}</span>
        <button
          type="button"
          className="notice-chip-copy"
          aria-label="Copy message"
          onClick={copy}
        >
          <Icon name="copy" size={12} />
        </button>
      </div>
      <div className="notice-chip-text">{message}</div>
    </div>
  );
}
