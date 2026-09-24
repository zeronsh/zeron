import { methods } from "@zeron/engine-client";
import { mintId } from "./id";

/**
 * Attachments — staging, upload, and read-back for the composer and the
 * transcript. A direct port of `crates/ui/src/attachments.rs` (the desktop's
 * composer/use-attachments.ts + transcript-attachment-cache.ts) with no code
 * shared — same wire, same behavior, web idioms.
 *
 * - **Text transport**: `withAttachments` appends the refs trailer to the
 *   prompt; `parseUserMessageImages` reverses it for the transcript's own
 *   bubbles.
 * - **Staging**: `stageFile` / `stageBytes` for picker, drop, paste, and
 *   clipboard reads. The bytes live in `StagedAttachment.bytes`; the local
 *   `objectUrl` is optional cache.
 * - **Upload**: `uploadAttachments` chunks the bytes via `UploadChunk` and
 *   finalizes with `UploadCommit`. Per-attachment progress + retry ladder
 *   mirror the desktop (`state.ts uploadAttachment`).
 * - **Read-back**: `readAttachmentImage` loops `ReadAttachmentChunk` until
 *   `done`. The hook lives in `../state/attachment-cache.ts`.
 */

const ATTACHED_HEADER = "Attached images (local files — open them to view):";
const ATTACHED_HEADER_LOOSE = "attached images (local files";

// ---------------------------------------------------------------------------
// Limits and constants (port of attachments.rs MAX_/UPLOAD_/READ_)
// ---------------------------------------------------------------------------

/** One-attachment size cap (24 MB) — matches the engine's jail. */
export const MAX_ATTACHMENT_BYTES = 24 * 1024 * 1024;

// ---------------------------------------------------------------------------
// Staged-strip geometry (composer.rs:288-296) — the pill height depends on
// the strip's wrap math, so the height function lives with the attachments
// (the strip's own rendering is components/attachments/attachment-strip.tsx).
// ---------------------------------------------------------------------------

/** `STRIP_THUMB` (composer.rs:288) — the staged thumbnail's edge. */
export const STRIP_THUMB = 56;
/** `STRIP_GAP` (composer.rs:289). */
export const STRIP_GAP = 8;
/** `STRIP_PAD_TOP` (composer.rs:290). */
export const STRIP_PAD_TOP = 12;
/** `STRIP_PAD_X` (composer.rs:291) — the strip's per-side inset. */
export const STRIP_PAD_X = 16;

/**
 * `attachment_strip_height` (composer.rs:296): the wrap strip's height for
 * `count` staged thumbnails at an `innerWidth` pill content width. Mirrors
 * flex-wrap: as many 56px thumbs per row as fit with 8px gaps inside the 16px
 * side insets.
 */
export function attachmentStripHeight(count: number, innerWidth: number): number {
  if (count === 0) {
    return 0;
  }
  const usable = Math.max(innerWidth - 2 * STRIP_PAD_X, STRIP_THUMB);
  const perRow = Math.max(Math.floor((usable + STRIP_GAP) / (STRIP_THUMB + STRIP_GAP)), 1);
  const rows = Math.ceil(count / perRow);
  return STRIP_PAD_TOP + rows * STRIP_THUMB + (rows - 1) * STRIP_GAP;
}

/** Base64 chars per `UploadChunk`. Sized against the relay's hard ceiling
 *  (Cloudflare caps WebSocket at 1 MiB, JSON envelope + uleb header add
 *  ~150 bytes). A multiple of 4 so each slice is independently decodable. */
export const UPLOAD_CHUNK_B64_CHARS = 680_000;

/** Safety cap on the read-back loop (a stuck offset or runaway host stops
 *  here, mirroring `state.ts MAX_ATTACHMENT_READ_CHUNKS`). */
const MAX_READ_CHUNKS = 1_000;

/** Per-call deadlines (state.ts uploadAttachment): a stalled-but-open link
 *  never fails an RPC on its own, so every call races a timer. */
const FIRST_CHUNK_TIMEOUT_MS = 90_000;
const CHUNK_TIMEOUT_MS = 30_000;
const COMMIT_TIMEOUT_MS = 150_000;
const READ_CHUNK_TIMEOUT_MS = 20_000;

const UPLOAD_CONCURRENCY = 3;

/** Whole-attachment deadline. Scales with chunk count, capped (a flapping
 *  link that succeeds on retry 2-of-3 never trips the consecutive-failure
 *  abort — bound the WHOLE send instead). */
function attachmentDeadlineMs(nChunks: number): number {
  return Math.min(120_000 + 15_000 * nChunks, 900_000);
}

// ---------------------------------------------------------------------------
// Format detection
// ---------------------------------------------------------------------------

/** The intersection of browser-decodable image types and the engine's
 *  `mime_by_ext` read-back jail. */
export type AttachmentFormat = "png" | "jpg" | "gif" | "webp" | "svg" | "bmp" | "tif";

const EXT_TO_FORMAT: Record<string, AttachmentFormat> = {
  png: "png",
  jpg: "jpg",
  jpeg: "jpg",
  gif: "gif",
  webp: "webp",
  svg: "svg",
  bmp: "bmp",
  tif: "tif",
  tiff: "tif",
};

/** The MIME type the engine will return when we read this back. */
export function formatToMime(format: AttachmentFormat): string {
  switch (format) {
    case "png":
      return "image/png";
    case "jpg":
      return "image/jpeg";
    case "gif":
      return "image/gif";
    case "webp":
      return "image/webp";
    case "svg":
      return "image/svg+xml";
    case "bmp":
      return "image/bmp";
    case "tif":
      return "image/tiff";
  }
}

/** Detect an image format from a file name's extension. `null` for
 *  unsupported types (the composer quietly skips them — same as the desktop). */
export function formatByName(name: string): AttachmentFormat | null {
  const dot = name.lastIndexOf(".");
  if (dot < 0 || dot === name.length - 1) {
    return null;
  }
  const ext = name.slice(dot + 1).toLowerCase();
  return EXT_TO_FORMAT[ext] ?? null;
}

/** Detect an image format from raw bytes (magic-number sniff). Browsers'
 *  built-in decoders accept the result of these heuristics. `null` when
 *  unrecognised. */
export function formatByBytes(bytes: Uint8Array): AttachmentFormat | null {
  if (bytes.length >= 8 &&
    bytes[0] === 0x89 && bytes[1] === 0x50 && bytes[2] === 0x4e && bytes[3] === 0x47 &&
    bytes[4] === 0x0d && bytes[5] === 0x0a && bytes[6] === 0x1a && bytes[7] === 0x0a) {
    return "png";
  }
  if (bytes.length >= 3 && bytes[0] === 0xff && bytes[1] === 0xd8 && bytes[2] === 0xff) {
    return "jpg";
  }
  if (bytes.length >= 6 &&
    (bytes[0] === 0x47 && bytes[1] === 0x49 && bytes[2] === 0x46 && bytes[3] === 0x38) &&
    (bytes[4] === 0x37 || bytes[4] === 0x39) && bytes[5] === 0x61) {
    return "gif";
  }
  if (bytes.length >= 12 &&
    bytes[0] === 0x52 && bytes[1] === 0x49 && bytes[2] === 0x46 && bytes[3] === 0x46 &&
    bytes[8] === 0x57 && bytes[9] === 0x45 && bytes[10] === 0x42 && bytes[11] === 0x50) {
    return "webp";
  }
  if (bytes.length >= 4 &&
    bytes[0] === 0x42 && bytes[1] === 0x4d) {
    return "bmp";
  }
  if (bytes.length >= 4 &&
    (bytes[0] === 0x49 && bytes[1] === 0x49 && bytes[2] === 0x2a && bytes[3] === 0x00) ||
    (bytes[0] === 0x4d && bytes[1] === 0x4d && bytes[2] === 0x00 && bytes[3] === 0x2a)) {
    return "tif";
  }
  return null;
}

/** The full filename extension for a format. */
function formatExtension(format: AttachmentFormat): string {
  switch (format) {
    case "jpg":
      return "jpg";
    case "tif":
      return "tif";
    default:
      return format;
  }
}

/** Pasted screenshots and dropped bare files often arrive without an
 *  extension; agents sniff images by extension so we add one. */
export function ensureExtension(name: string, format: AttachmentFormat): string {
  const dot = name.lastIndexOf(".");
  if (dot > 0 && dot < name.length - 1) {
    const stem = name.slice(0, dot);
    const ext = name.slice(dot + 1);
    if (
      stem.length > 0 &&
      ext.length >= 2 &&
      ext.length <= 5 &&
      ext.toLowerCase().split("").every((ch) => (ch >= "a" && ch <= "z") || (ch >= "A" && ch <= "Z") || (ch >= "0" && ch <= "9"))
    ) {
      return name;
    }
  }
  return `${name}.${formatExtension(format)}`;
}

/** A safe default name when the source didn't carry one. */
function defaultName(): string {
  return "image";
}

// ---------------------------------------------------------------------------
// Staged attachment
// ---------------------------------------------------------------------------

/** An attachment the user has dropped/picked into the composer. Bytes are
 *  kept locally until the user sends (or removes); the upload step copies
 *  them onto the chat's host device. */
export interface StagedAttachment {
  /** Client-minted id, stable across renders; used as the row key in the
   *  strip and as the upload identity (`uploadId`) for the chunked RPC. */
  readonly id: string;
  /** Filename sent to the engine. Carries a valid extension so agents
   *  sniff correctly (see [`ensureExtension`]). */
  readonly name: string;
  /** Detected or sniffed format; drives the MIME used for read-back. */
  readonly format: AttachmentFormat;
  /** The raw bytes. Never mutated by the strip — `removeAttachment` drops
   *  the reference. */
  readonly bytes: Uint8Array;
  /** A tiny data URL for the thumbnail (cheap to render, decoded inline).
   *  Constructed once at staging time. */
  readonly previewUrl: string;
  /** Best-effort natural size (px). `null` when the browser can't decode
   *  the bytes (e.g. an SVG that depends on an external font). */
  readonly naturalSize: { width: number; height: number } | null;
}

/** Stage bytes with a guessed format (e.g. clipboard paste). */
export function stageBytes(name: string | null, bytes: Uint8Array): StagedAttachment {
  const detected = formatByBytes(bytes);
  if (detected === null) {
    throw new Error("Only image files can be attached (PNG, JPEG, GIF, WebP, SVG, BMP, or TIFF).");
  }
  if (bytes.byteLength > MAX_ATTACHMENT_BYTES) {
    throw new Error(`${name ?? defaultName()} is too large (24 MB max).`);
  }
  const baseName = name && name.length > 0 ? name : defaultName();
  return finalizeStage(baseName, detected, bytes);
}

/** Stage a picked file. `File` is the browser `File` (subclass of Blob);
 *  we keep the bytes raw for upload and a data-URL thumbnail for the strip. */
export function stageFile(file: { readonly name: string; readonly size: number; arrayBuffer(): Promise<ArrayBuffer> }): Promise<StagedAttachment> {
  if (file.size > MAX_ATTACHMENT_BYTES) {
    return Promise.reject(new Error(`${file.name} is too large (24 MB max).`));
  }
  return file
    .arrayBuffer()
    .then((buffer) => new Uint8Array(buffer))
    .then((bytes) => {
      const fromName = formatByName(file.name);
      const fromBytes = formatByBytes(bytes);
      const format = fromName ?? fromBytes;
      if (format === null) {
        throw new Error(`${file.name} is not a supported image.`);
      }
      return finalizeStage(file.name, format, bytes);
    });
}

/** Build the staged attachment (the bytes pass the size gate by the time
 *  we get here). */
function finalizeStage(rawName: string, format: AttachmentFormat, bytes: Uint8Array): StagedAttachment {
  const name = ensureExtension(rawName, format);
  const previewUrl = bytesToDataUrl(bytes, formatToMime(format));
  return {
    id: mintId(),
    name,
    format,
    bytes,
    previewUrl,
    naturalSize: null,
  };
}

/** Compose a data URL the strip can hand straight to an `<img src=…>`. */
function bytesToDataUrl(bytes: Uint8Array, mime: string): string {
  return `data:${mime};base64,${bytesToBase64(bytes)}`;
}

/** Minimal base64 encoder (no dependency; `btoa` is unavailable in jsdom
 *  tests, and `Uint8Array.toBase64` is too new to lean on). */
function base64Encode(binary: string): string {
  const chars = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
  let out = "";
  let i = 0;
  for (; i + 2 < binary.length; i += 3) {
    const a = binary.charCodeAt(i);
    const b = binary.charCodeAt(i + 1);
    const c = binary.charCodeAt(i + 2);
    out += chars[a >> 2];
    out += chars[((a & 3) << 4) | (b >> 4)];
    out += chars[((b & 15) << 2) | (c >> 6)];
    out += chars[c & 63];
  }
  if (i < binary.length) {
    const a = binary.charCodeAt(i);
    out += chars[a >> 2];
    if (i + 1 < binary.length) {
      const b = binary.charCodeAt(i + 1);
      out += chars[((a & 3) << 4) | (b >> 4)];
      out += chars[(b & 15) << 2];
      out += "=";
    } else {
      out += chars[(a & 3) << 4];
      out += "==";
    }
  }
  return out;
}

/** Decode a base64 string back to bytes (used by `readAttachmentImage`). */
function base64Decode(b64: string): Uint8Array {
  if (typeof atob === "function") {
    const binary = atob(b64);
    const out = new Uint8Array(binary.length);
    for (let ix = 0; ix < binary.length; ix++) {
      out[ix] = binary.charCodeAt(ix);
    }
    return out;
  }
  // Node < 16 fallback (rare; tests should ship a polyfill via `setupFiles`).
  const chars = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
  const lookup = new Int8Array(256).fill(-1);
  for (let i = 0; i < chars.length; i++) {
    lookup[chars.charCodeAt(i)] = i;
  }
  const cleaned = b64.replace(/=+$/g, "");
  const out = new Uint8Array(Math.floor((cleaned.length * 3) / 4));
  let ox = 0;
  let cursor = 0;
  for (; cursor + 3 < cleaned.length; cursor += 4) {
    const a = lookup[cleaned.charCodeAt(cursor)]!;
    const b = lookup[cleaned.charCodeAt(cursor + 1)]!;
    const c = lookup[cleaned.charCodeAt(cursor + 2)]!;
    const d = lookup[cleaned.charCodeAt(cursor + 3)]!;
    out[ox++] = (a << 2) | (b >> 4);
    out[ox++] = ((b & 15) << 4) | (c >> 2);
    out[ox++] = ((c & 3) << 6) | d;
  }
  if (cursor + 1 < cleaned.length) {
    const a = lookup[cleaned.charCodeAt(cursor)]!;
    const b = lookup[cleaned.charCodeAt(cursor + 1)]!;
    out[ox++] = (a << 2) | (b >> 4);
  }
  if (cursor + 2 < cleaned.length) {
    const b = lookup[cleaned.charCodeAt(cursor + 1)]!;
    const c = lookup[cleaned.charCodeAt(cursor + 2)]!;
    out[ox++] = ((b & 15) << 4) | (c >> 2);
  }
  return out.subarray(0, ox);
}

// ---------------------------------------------------------------------------
// Text transport (withAttachments / parseUserMessageImages)
// ---------------------------------------------------------------------------

/** The placeholder text used for image-only sends (the body the harness
 *  sees when the user attaches without typing anything). */
export const ATTACHMENT_ONLY_TEXT = "See the attached image(s).";

/** Append the attachment-refs trailer to a prompt so the host's harness
 *  can find the files locally. The trailer is the only thing that
 *  persists in the chat doc — the bytes live on the chat's host device. */
export function withAttachments(text: string, paths: readonly string[]): string {
  if (paths.length === 0) {
    return text;
  }
  const body = text.length === 0 ? ATTACHMENT_ONLY_TEXT : text;
  const refs = paths.map((path) => `- ${path}`).join("\n");
  return `${body}\n\n${ATTACHED_HEADER}\n${refs}`;
}

/** Find the refs trailer byte offsets. `null` when no trailer is present
 *  (the message predates attachments or is a plain-text turn). */
function findRefsMarker(content: string): { bodyEnd: number; refsStart: number } | null {
  const lower = content.toLowerCase();
  const needle = `\n\n${ATTACHED_HEADER_LOOSE}`;
  let from = 0;
  while (from < lower.length) {
    const found = lower.indexOf(needle, from);
    if (found < 0) {
      return null;
    }
    const lineStart = found + 2;
    const nl = content.indexOf("\n", lineStart);
    const lineEnd = nl < 0 ? content.length : nl;
    const line = content.slice(lineStart, lineEnd).replace(/\r$/, "");
    if (line.endsWith("):")) {
      const refsStart = Math.min(lineEnd + 1, content.length);
      return { bodyEnd: found, refsStart };
    }
    from = lineStart;
  }
  return null;
}

/** A user-message attachment ref parsed back from a bubble's text. */
export interface UserImageAttachment {
  /** Stable id (`{ix}:{path}`) — keeps React row keys stable across renders. */
  readonly id: string;
  /** The path persisted in the doc; on the host device this is the absolute
   *  file path the harness opens. */
  readonly path: string;
  /** The filename component. */
  readonly name: string;
}

/** What `parseUserMessageImages` returns — the visible prompt plus the
 *  parsed attachment refs (empty when the message predates attachments). */
export interface ParsedUserMessage {
  readonly text: string;
  readonly attachments: readonly UserImageAttachment[];
}

/** Split a user message's text into the visible prompt and the attachment
 *  refs (the desktop's `parseUserMessageImages` port). Empty attachments
 *  when no trailer is found. */
export function parseUserMessageImages(content: string): ParsedUserMessage {
  const marker = findRefsMarker(content);
  if (marker === null) {
    return { text: content, attachments: [] };
  }
  const body = content.slice(0, marker.bodyEnd).replace(/\s+$/, "");
  const refsText = content.slice(marker.refsStart);
  const attachments: UserImageAttachment[] = refsText
    .split("\n")
    .map((line) => line.trimStart())
    .filter((line): line is string => line.startsWith("- ") && line.length > 2)
    .map((line) => line.slice(2).trim())
    .filter((path) => path.length > 0)
    .map((path, ix) => ({
      id: `${ix}:${path}`,
      name: nameFromPath(path),
      path,
    }));
  if (attachments.length === 0) {
    return { text: content, attachments: [] };
  }
  const visibleText = body === ATTACHMENT_ONLY_TEXT ? "" : body;
  return { text: visibleText, attachments };
}

/** The text the rail/sidebar surfaces for a user message. For image-only
 *  sends, this collapses to a short summary like "Attached image" /
 *  "3 attached images" — the desktop's `userMessageRailText` port. */
export function userMessageRailText(content: string): string {
  const parsed = parseUserMessageImages(content);
  if (parsed.text.trim().length > 0) {
    return parsed.text;
  }
  switch (parsed.attachments.length) {
    case 0:
      return content;
    case 1:
      return "Attached image";
    default:
      return `${parsed.attachments.length} attached images`;
  }
}

function nameFromPath(path: string): string {
  const ix = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  const name = ix >= 0 ? path.slice(ix + 1) : path;
  return name.length > 0 ? name : "image";
}

// ---------------------------------------------------------------------------
// Upload — chunked, retried, progress-tracked (port of state.ts uploadAttachment)
// ---------------------------------------------------------------------------

/** A progress reporter: receives `(uploadedBytes, totalBytes)` on every
 *  committed chunk. Optional so callers can ignore it. */
export type UploadProgress = (uploadedBytes: number, totalBytes: number) => void;

/**
 * The upload-path failure the composer surfaces with its own verbatim copy
 * (composer.rs:6358-6374): `"Couldn't upload the attachment — the device may
 * be offline."` The desktop's OTHER upload failure string — `"Couldn't stage
 * the attachment locally."` (composer.rs:6323-6335) — belongs to the
 * queued-flow's LOCAL-engine stage, which the web's single-engine model never
 * takes (there is no local engine in a browser); only the remote-host path
 * exists here.
 */
export class AttachmentUploadError extends Error {
  constructor(cause: unknown) {
    super("Couldn't upload the attachment — the device may be offline.");
    this.name = "AttachmentUploadError";
    this.cause = cause;
  }
}

/** What one upload produced: the durable path on the host (the same path
 *  the transcript's read-back uses) plus the wire's `transfers` entry. */
export interface UploadedAttachment {
  readonly uploadId: string;
  readonly fileName: string;
  readonly path: string;
}

/** Upload a batch of staged attachments to the chat's host device. The
 *  caller mints each `uploadId` (we use the staged id so the wire-level
 *  identity matches the strip's row key). Sequential per-attachment so
 *  a failed upload fails the whole send with a clear error. */
export async function uploadAttachments(
  client: { call(method: string, params: unknown): Promise<unknown> },
  attachments: readonly StagedAttachment[],
  progress: UploadProgress | null,
): Promise<readonly UploadedAttachment[]> {
  if (attachments.length === 0) {
    return [];
  }
  const totalBytes = attachments.reduce((sum, att) => sum + att.bytes.byteLength, 0);
  let uploadedSoFar = 0;
  progress?.(0, totalBytes);

  const out: UploadedAttachment[] = [];
  for (const att of attachments) {
    try {
      const path = await uploadOne(client, att, (delta) => {
        progress?.(uploadedSoFar + delta, totalBytes);
      });
      out.push({ uploadId: att.id, fileName: att.name, path });
      uploadedSoFar += att.bytes.byteLength;
    } catch (error) {
      // The composer surfaces this with the desktop's verbatim copy
      // (composer.rs:6371-6373); the raw cause rides along for logs.
      throw new AttachmentUploadError(error);
    }
  }
  progress?.(totalBytes, totalBytes);
  return out;
}

/** Chunked upload of one attachment. Base64 the bytes, fan out up to
 *  [`UPLOAD_CONCURRENCY`] `UploadChunk` calls, then `UploadCommit`. Each
 *  call races a per-deadline timer; transient blips retry once. */
async function uploadOne(
  client: { call(method: string, params: unknown): Promise<unknown> },
  attachment: StagedAttachment,
  onChunk: (deltaBytes: number) => void,
): Promise<string> {
  const b64 = bytesToBase64(attachment.bytes);
  const ranges = chunkRanges(b64.length);
  const totalBytes = attachment.bytes.byteLength;
  const uploadId = attachment.id;

  const overallTimer = setTimeout(() => {}, 0); // ensure we have a timer handle for clearTimeout
  clearTimeout(overallTimer);
  const deadlineMs = attachmentDeadlineMs(ranges.length);
  const deadline = new Promise<never>((_, reject) =>
    setTimeout(
      () => reject(new Error(`attachment upload exceeded ${Math.round(deadlineMs / 1000)}s`)),
      deadlineMs,
    ),
  );

  const upload = (async () => {
    let uploaded = 0;
    let nextSeq = 0;
    const queue: Promise<void>[] = [];
    const inFlight = new Map<number, Promise<void>>();

    const launch = (): void => {
      while (inFlight.size < UPLOAD_CONCURRENCY && nextSeq < ranges.length) {
        const seq = nextSeq++;
        const range = ranges[seq]!;
        const chunkB64 = b64.slice(range.start, range.end);
        const binaryBytes = Math.floor(((range.end - range.start) * 3) / 4);
        const isFirstWindow = seq < UPLOAD_CONCURRENCY;
        const call = sendChunk(client, uploadId, seq, chunkB64, isFirstWindow).then(() => {
          uploaded += binaryBytes;
          // Cap rounding so the final reported value matches totalBytes.
          if (seq === ranges.length - 1 && uploaded > totalBytes) {
            uploaded = totalBytes;
          }
          onChunk(uploaded);
        });
        inFlight.set(seq, call.finally(() => inFlight.delete(seq)));
        queue.push(inFlight.get(seq)!);
      }
    };

    launch();
    while (inFlight.size > 0 || nextSeq < ranges.length) {
      if (inFlight.size === 0 && nextSeq < ranges.length) {
        launch();
      }
      await Promise.race(Array.from(inFlight.values()));
    }

    const reply = (await callWithTimeout(
      client,
      methods.UPLOAD_COMMIT,
      { uploadId, fileName: attachment.name },
      COMMIT_TIMEOUT_MS,
    )) as { path?: unknown };
    const path = typeof reply.path === "string" ? reply.path : null;
    if (path === null) {
      throw new Error("upload commit returned no path");
    }
    return path;
  })();

  return Promise.race([upload, deadline]);
}

/** One `UploadChunk` call with its per-call timeout. Transient blips
 *  (timeout, transport) retry up to 2 times — 3 attempts total, like the
 *  desktop's per-chunk loop (attachments.rs:388-417) — staggered by
 *  `50ms * attempt * (seq + 1)` so parallel chunks that failed together don't
 *  re-collide in lockstep. `seq` slots are idempotent engine-side, so a blind
 *  re-send is safe. */
async function sendChunk(
  client: { call(method: string, params: unknown): Promise<unknown> },
  uploadId: string,
  seq: number,
  data: string,
  isFirstWindow: boolean,
): Promise<void> {
  const timeoutMs = isFirstWindow ? FIRST_CHUNK_TIMEOUT_MS : CHUNK_TIMEOUT_MS;
  const params = { uploadId, seq, data };
  let attempt = 0;
  for (;;) {
    try {
      await callWithTimeout(client, methods.UPLOAD_CHUNK, params, timeoutMs);
      return;
    } catch (error) {
      if (attempt >= 2) {
        throw error;
      }
      attempt += 1;
      const delay = 50 * attempt * (seq + 1);
      await new Promise((resolve) => setTimeout(resolve, delay));
    }
  }
}

/** `(seq, b64 byte-range)` chunks for one file's base64 (`chunk_ranges`,
 *  attachments.rs:330-344): contiguous, non-overlapping
 *  `UPLOAD_CHUNK_B64_CHARS`-sized tiles. An empty file still yields one empty
 *  chunk (the commit RPC needs the uploadId staged). */
export function chunkRanges(b64Len: number): Array<{ start: number; end: number }> {
  const ranges: Array<{ start: number; end: number }> = [];
  let start = 0;
  while (start < b64Len) {
    const end = Math.min(start + UPLOAD_CHUNK_B64_CHARS, b64Len);
    ranges.push({ start, end });
    start = end;
  }
  if (ranges.length === 0) {
    ranges.push({ start: 0, end: 0 });
  }
  return ranges;
}

/** Encode bytes to a base64 string. The platform's `btoa` accepts a
 *  binary string; chunked to avoid `String.fromCharCode.apply` overflow on
 *  multi-MB buffers. Falls back to the chunked string-encoder when `btoa`
 *  isn't available (older runtimes — Node 16+ has it globally). */
function bytesToBase64(bytes: Uint8Array): string {
  if (typeof btoa === "function") {
    const CHUNK = 0x8000;
    let binary = "";
    for (let ix = 0; ix < bytes.byteLength; ix += CHUNK) {
      const slice = bytes.subarray(ix, Math.min(ix + CHUNK, bytes.byteLength));
      binary += String.fromCharCode.apply(null, Array.from(slice) as number[]);
    }
    return btoa(binary);
  }
  return base64Encode(bytesToBinaryString(bytes));
}

function bytesToBinaryString(bytes: Uint8Array): string {
  const CHUNK = 0x8000;
  let out = "";
  for (let ix = 0; ix < bytes.byteLength; ix += CHUNK) {
    const slice = bytes.subarray(ix, Math.min(ix + CHUNK, bytes.byteLength));
    out += String.fromCharCode.apply(null, Array.from(slice) as number[]);
  }
  return out;
}

/** Race a JSON call against a wall-clock deadline. Mirrors the desktop's
 *  `call_with_timeout` (state.ts uploadAttachment). */
async function callWithTimeout(
  client: { call(method: string, params: unknown): Promise<unknown> },
  method: string,
  params: unknown,
  timeoutMs: number,
): Promise<unknown> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const timeout = new Promise<never>((_, reject) => {
    timer = setTimeout(() => reject(new Error(`${method} timed out`)), timeoutMs);
  });
  try {
    return await Promise.race([client.call(method, params), timeout]);
  } finally {
    if (timer !== undefined) {
      clearTimeout(timer);
    }
  }
}

// ---------------------------------------------------------------------------
// Read-back (port of state.ts readAttachmentImage)
// ---------------------------------------------------------------------------

/** Read-back result: the decoded bytes plus the name the host reported. */
export interface DecodedAttachmentImage {
  readonly name: string;
  readonly mime: string;
  readonly bytes: Uint8Array;
}

/** Loop `ReadAttachmentChunk` until the host signals `done`, accumulating
 *  base64. Returns `null` on any timeout / RPC error / malformed chunk /
 *  stuck offset / over-cap loop (defensive, never infinite). */
export async function readAttachmentImage(
  client: { call(method: string, params: unknown): Promise<unknown> },
  path: string,
): Promise<DecodedAttachmentImage | null> {
  let name = "";
  let mime = "application/octet-stream";
  let b64 = "";
  let offset = 0;
  for (let ix = 0; ix < MAX_READ_CHUNKS; ix++) {
    let reply: { name?: unknown; mimeType?: unknown; data?: unknown; nextOffset?: unknown; done?: unknown };
    try {
      reply = (await callWithTimeout(
        client,
        methods.READ_ATTACHMENT_CHUNK,
        { path, offset },
        READ_CHUNK_TIMEOUT_MS,
      )) as typeof reply;
    } catch {
      return null;
    }
    if (
      typeof reply.data !== "string" ||
      typeof reply.done !== "boolean" ||
      typeof reply.nextOffset !== "number"
    ) {
      return null;
    }
    if (typeof reply.name === "string") {
      name = reply.name;
    }
    if (typeof reply.mimeType === "string") {
      mime = reply.mimeType;
    }
    b64 += reply.data;
    if (reply.done) {
      break;
    }
    if (reply.nextOffset <= offset) {
      return null;
    }
    offset = reply.nextOffset;
  }
  if (b64.length === 0) {
    return null;
  }
  let bytes: Uint8Array;
  try {
    bytes = base64Decode(b64);
  } catch {
    return null;
  }
  return {
    name: name.length > 0 ? name : nameFromPath(path),
    mime,
    bytes,
  };
}

/** Bytes → data URL (used to render a thumbnail after a successful read). */
export function bytesToImageDataUrl(mime: string, bytes: Uint8Array): string {
  return bytesToDataUrl(bytes, mime);
}