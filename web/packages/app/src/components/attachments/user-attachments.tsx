import { useEffect, useState } from "react";
import type { EngineClient } from "@zeron/engine-client";
import {
  attachmentCacheKey,
  beginAttachmentLoad,
  loadAttachment,
  protectAttachments,
  useAttachmentImage,
  useUploadProgressPercent,
  type AttachmentImageSnapshot,
} from "../../state/attachment-cache";
import { bytesToImageDataUrl, type UserImageAttachment } from "../../lib/attachments";
import { Lightbox } from "../lightbox";
import { GlyphSpinner } from "../glyph-spinner";

/**
 * The user-row's attachment strip — the web port of the desktop's own
 * user-bubble 112×80 thumbnail strip (`transcript.rs::render_user_attachments`,
 * :4878). Wraps right-aligned under the user bubble; loads go through the
 * module-level cache in `../../state/attachment-cache.ts` so a re-render of
 * the same row never re-fetches, and the cache seeds after a successful send
 * (composer.tsx) so the user's own bubble never round-trips.
 *
 * A thumbnail whose ref is still pending (`pending://…` or legacy
 * `pending/…`) draws the sending overlay — a dark pulsing scrim with the
 * whole-send upload-progress ring (or the glyph spinner while the percent is
 * unknown) over the locally-seeded image, exactly the desktop's echo
 * treatment (transcript.rs:5118-5149).
 *
 * The eviction shield: every mounted strip registers its keys with
 * `protectAttachments` (ref-counted, so duplicate paths in one message
 * survive), which is the web peer of the desktop's per-row-sync
 * `protect_attachments` call — the set is always exactly what is on screen.
 */

interface UserAttachmentsProps {
  readonly client: EngineClient;
  readonly deviceId: string | null;
  readonly attachments: readonly UserImageAttachment[];
}

/**
 * Ref-counted registry of the (deviceId, path) keys currently on screen —
 * virtualization unmounts off-screen rows, so the mounted set tracks the
 * visible set without a transcript-side sync pass.
 */
const mountedKeys = new Map<string, number>();

function remountProtection(): void {
  const keys = new Set(mountedKeys.keys());
  protectAttachments(keys);
}

export function UserAttachments({ client, deviceId, attachments }: UserAttachmentsProps) {
  const [preview, setPreview] = useState<{ name: string; src: string } | null>(null);

  // The eviction shield (attachments.rs:639-654): replace the protected set
  // with exactly this strip's keys while it is on screen.
  useEffect(() => {
    if (deviceId === null) {
      return;
    }
    const keys = attachments.map((att) => attachmentCacheKey(deviceId, att.path));
    for (const key of keys) {
      mountedKeys.set(key, (mountedKeys.get(key) ?? 0) + 1);
    }
    remountProtection();
    return () => {
      for (const key of keys) {
        const count = (mountedKeys.get(key) ?? 1) - 1;
        if (count <= 0) {
          mountedKeys.delete(key);
        } else {
          mountedKeys.set(key, count);
        }
      }
      remountProtection();
    };
  }, [deviceId, attachments]);

  if (attachments.length === 0) {
    return null;
  }
  return (
    <div className="user-attachments">
      {attachments.map((att) => (
        <UserAttachmentThumb
          key={att.id}
          client={client}
          deviceId={deviceId}
          attachment={att}
          onPreview={setPreview}
        />
      ))}
      {preview !== null && (
        <Lightbox name={preview.name} src={preview.src} onClose={() => setPreview(null)} />
      )}
    </div>
  );
}

function UserAttachmentThumb({
  client,
  deviceId,
  attachment,
  onPreview,
}: {
  client: EngineClient;
  deviceId: string | null;
  attachment: UserImageAttachment;
  onPreview: (preview: { name: string; src: string } | null) => void;
}) {
  // We don't know the deviceId for older chats, and a single-bubble render
  // with no device is harmless — the row paints a skeleton, the load is
  // skipped, the thumbnail never lands. In practice the engine always
  // includes the deviceId. (The desktop tries the chat's HOST device id then
  // the local one, in order; the web's single-engine model only ever has the
  // one engine device id — a documented simplification, not a silent fork.)
  const snapshot = useAttachmentImage(deviceId, attachment.path);

  // A ref that is still crossing the wire: the queued flow's `pending://`
  // (bytes ship engine-side after the send) or the legacy echo's synthetic
  // `pending/` (transcript.rs:4903-4917).
  const sending =
    attachment.path.startsWith("pending://") || attachment.path.startsWith("pending/");

  useEffect(() => {
    if (deviceId === null) {
      return;
    }
    if (snapshot.state !== "loading") {
      return;
    }
    void loadAttachment(client, deviceId, attachment.path);
  }, [client, deviceId, attachment.path, snapshot.state]);

  // The cache's retry backoff computes `retryIn`, but nothing re-triggers a
  // load when it elapses without a re-render — this row owns its own
  // re-attempt (the desktop's cache-level scheduled retry, attachments.rs:578).
  const retryIn = snapshot.state === "error" ? snapshot.retryIn : 0;
  useEffect(() => {
    if (deviceId === null || retryIn <= 0) {
      return;
    }
    const timer = setTimeout(() => {
      void loadAttachment(client, deviceId, attachment.path);
    }, retryIn);
    return () => clearTimeout(timer);
  }, [client, deviceId, attachment.path, retryIn]);

  return (
    <button
      type="button"
      className="user-attachments-thumb"
      data-state={snapshot.state === "loaded" ? "loaded" : snapshot.state}
      aria-label={snapshot.state === "loaded" ? `Preview ${attachment.name}` : attachment.name}
      onClick={() => {
        if (snapshot.state === "loaded" && snapshot.image !== null) {
          onPreview({
            name: snapshot.image.name,
            src: bytesToImageDataUrl(snapshot.image.mime, snapshot.image.bytes),
          });
        }
      }}
    >
      <ThumbContent snapshot={snapshot} sending={sending} />
    </button>
  );
}

function ThumbContent({
  snapshot,
  sending,
}: {
  snapshot: AttachmentImageSnapshot;
  sending: boolean;
}) {
  if (snapshot.state === "loaded" && snapshot.image !== null) {
    return (
      <>
        {/* Explicit 110×78 dims (112 − the 1px borders) with the img's own
            7px radius: percent heights let a tall photo grow past the frame
            (transcript.rs:5102-5117). */}
        <img
          className="user-attachments-thumb-img"
          src={bytesToImageDataUrl(snapshot.image.mime, snapshot.image.bytes)}
          alt=""
          draggable={false}
        />
        {sending && <SendingOverlay />}
      </>
    );
  }
  if (snapshot.state === "error") {
    // The dashed "missing" thumb — no glyph, no label (transcript.rs:5153).
    return null;
  }
  // Loading: the pulsing skeleton (same wash as the popover skeletons) —
  // the button's own border/bg pair carries it; no content.
  return null;
}

/**
 * The sending overlay (transcript.rs:5118-5149): a dark scrim pulsing on the
 * ZERON_PULSE wave over the loaded image, with the whole-send upload
 * progress ring when a percent is known and the glyph spinner otherwise.
 */
function SendingOverlay() {
  // Percent sources, in order (transcript.rs:5917-5929): this attachment's
  // own relay transfer by its `pending://<uploadId>` ref (the web has no
  // transfer stream — no first source), then the send-wide
  // `upload_progress_percent`. Neither → the indeterminate spinner.
  const percent = useUploadProgressPercent();
  return (
    <span className="user-attachments-thumb-sending" aria-hidden="true">
      {percent !== null ? <UploadProgressRing percent={percent} /> : <GlyphSpinner size={12} />}
    </span>
  );
}

/**
 * `loaders::upload_progress_ring` (loaders.rs:283-333): a 34px ring, 2.5px
 * stroke, faint white track plus a bright arc growing clockwise from 12
 * o'clock, percent centered. Fixed white-on-dim palette — the scrim behind
 * it makes it read in both themes.
 */
function UploadProgressRing({ percent }: { percent: number }) {
  const diameter = 34;
  const stroke = 2.5;
  const radius = diameter / 2 - stroke;
  const clamped = Math.min(Math.max(percent, 0), 100);
  const sweep = (clamped / 100) * 2 * Math.PI * radius;
  return (
    <span className="upload-progress-ring">
      <svg
        className="upload-progress-ring-svg"
        width={diameter}
        height={diameter}
        viewBox={`0 0 ${diameter} ${diameter}`}
        aria-hidden="true"
      >
        {/* The track: a faint full circle. */}
        <circle
          className="upload-progress-ring-track"
          cx={diameter / 2}
          cy={diameter / 2}
          r={radius}
          fill="none"
          strokeWidth={stroke}
        />
        {/* The arc: `sweep` of the circumference, clockwise from 12 o'clock
            (rotate −90° puts the dash origin at the top). */}
        <circle
          className="upload-progress-ring-arc"
          cx={diameter / 2}
          cy={diameter / 2}
          r={radius}
          fill="none"
          strokeWidth={stroke}
          strokeDasharray={`${sweep} ${2 * Math.PI * radius - sweep}`}
          transform={`rotate(-90 ${diameter / 2} ${diameter / 2})`}
        />
      </svg>
      <span className="upload-progress-ring-label">{clamped}%</span>
    </span>
  );
}

/** Test-only: a no-side-effect way to claim the load slot. */
export const __beginLoadForTests = beginAttachmentLoad;
