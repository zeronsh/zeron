/**
 * The shared notice-chip model — the web peer of the desktop's
 * `crates/ui/src/notice.rs` (`notice_chip`). The transcript ErrorChip and the
 * composer failure notice converged on one stacked chip: a header row (icon +
 * medium label + copy button pinned top-right — failure payloads are meant to
 * be pasted, not screenshotted) over the WRAPPING message body. A one-line
 * ellipsis was exactly what made zeronsh/comet#95 undiagnosable from a
 * screenshot.
 */

/** `warning` picks the amber palette over the default red. */
export type NoticeChipTone = "danger" | "warning";

/**
 * The header-icon treatment, which also picks the chip's metrics: `plain` is
 * the composer's inline notice (bare 14px triangle, 12px radius); `tile` is
 * the transcript's ErrorChip (20px washed tile holding a 12px triangle, 10px
 * radius).
 */
export type NoticeChipVariant = "plain" | "tile";

export interface NoticeChipModel {
  readonly tone: NoticeChipTone;
  readonly variant: NoticeChipVariant;
  readonly label: string;
  readonly message: string;
  /** Triangle edge length in px: 12 inside the tile, 14 bare. */
  readonly iconSize: 12 | 14;
}

/** The composer's offline-ish failure reads as a warning, not an error. */
export const OFFLINE_NOTICE = "Engine not connected";

/** Tone for a composer failure message (offline → amber). */
export function noticeToneForMessage(message: string): NoticeChipTone {
  return message === OFFLINE_NOTICE ? "warning" : "danger";
}

/** The label the header row carries for a tone: "Warning" / "Error". */
export function noticeLabelForTone(tone: NoticeChipTone): "Warning" | "Error" {
  return tone === "warning" ? "Warning" : "Error";
}

export function noticeChipModel(input: {
  readonly tone: NoticeChipTone;
  readonly variant: NoticeChipVariant;
  readonly label: string;
  readonly message: string;
}): NoticeChipModel {
  return {
    tone: input.tone,
    variant: input.variant,
    label: input.label,
    message: input.message,
    iconSize: input.variant === "tile" ? 12 : 14,
  };
}
