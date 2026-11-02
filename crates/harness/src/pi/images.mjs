// Per-run image limits for Pi. No global settings or session images change.
//
// Pi bounds each image (2000 px, 4.5 MB) but not the request: every image in
// history is resent with every request, so a long screenshot session reaches
// a provider or gateway body limit (HTTP 413, which Pi neither retries nor
// compacts away) and can never complete another request. Three layers:
//
// 1. Re-encode large tool-result images (usually PNG to JPEG, same pixels, so
//    screen coordinates stay valid) before they enter history.
// 2. Keep each request's images within a budget derived from the model's
//    declared limits; older images are sent as text. The cut moves in quanta,
//    so the provider prompt cache breaks once per quantum, not per request.
// 3. When a provider still rejects the body size, halve the budget, record it
//    in the session (a new Pi process resumes each run) and retry once.
//
// Static namespace import: Pi's loader aliases it to the running Pi (dynamic
// import() bypasses that), and a missing export on an older Pi is undefined.
import * as piAgent from "@earendil-works/pi-coding-agent";

/** Re-encoding target; Pi's own normalization then leaves the image alone. */
const RESIZE = { maxWidth: 2000, maxHeight: 2000, maxBytes: 512 * 1024, jpegQuality: 80 };

/** Budget without a declared model limit: ~100 screenshots, under a 32 MB body cap with text. */
const DEFAULT_REQUEST = { maxBytes: 20 * 1024 * 1024, maxImages: 80 };
/** Share of a declared `maxRequestBytes` that images may use; text and JSON need the rest. */
const IMAGE_SHARE = 0.6;
const MIN_SCALE = 1 / 8;
const SCALE_ENTRY = "zeron-image-budget";

/** Provider or gateway rejections of the request size or image volume. */
const TOO_LARGE =
  /(?:^|status(?: code)?:?\s*)413\b|request_too_large|request (?:body |entity )?too large|payload too large|failed to buffer the request body|media_budget_exceeded|many-image requests/i;

const resizeImage = typeof piAgent.resizeImage === "function" ? piAgent.resizeImage : null;

export async function compactImages(content, resizeImage, options = RESIZE) {
  let changed = false;
  const out = [];
  for (const block of content) {
    if (block?.type !== "image" || typeof block.data !== "string" || block.data.length <= options.maxBytes) {
      out.push(block);
      continue;
    }
    let result = null;
    try {
      result = await resizeImage(Buffer.from(block.data, "base64"), block.mimeType, options);
    } catch {}
    // Keep the original when the backend is unavailable or nothing got smaller.
    if (!result || result.data.length >= block.data.length) {
      out.push(block);
      continue;
    }
    out.push({ ...block, data: result.data, mimeType: result.mimeType });
    if (result.width !== result.originalWidth || result.height !== result.originalHeight) {
      const scale = (result.originalWidth / result.width).toFixed(2);
      out.push({
        type: "text",
        text: `[Image: original ${result.originalWidth}x${result.originalHeight}, displayed at ${result.width}x${result.height}. Multiply coordinates by ${scale} to map to original image.]`,
      });
    }
    changed = true;
  }
  return changed ? out : null;
}

/** Request image limits for `model`, scaled down after body-size rejections. */
export function requestLimits(model, scale = 1) {
  const declared = model?.inputLimits ?? {};
  const fromBody = declared.maxRequestBytes ? Math.floor(declared.maxRequestBytes * IMAGE_SHARE) : Infinity;
  const fromCount = declared.images?.maxPerRequest ?? Infinity;
  const maxBytes = Math.max(1, Math.floor(Math.min(DEFAULT_REQUEST.maxBytes, fromBody) * scale));
  const maxImages = Math.max(1, Math.floor(Math.min(DEFAULT_REQUEST.maxImages, fromCount) * scale));
  return {
    maxBytes,
    maxImages,
    byteQuantum: Math.max(1, Math.floor(maxBytes * 0.4)),
    countQuantum: Math.max(1, Math.floor(maxImages * 0.25)),
  };
}

/** How many of the oldest images to drop so the rest fit `limits`. */
export function dropCount(sizes, limits) {
  const total = sizes.reduce((sum, n) => sum + n, 0);
  const excessCount = Math.max(0, sizes.length - limits.maxImages);
  const excessBytes = Math.max(0, total - limits.maxBytes);
  if (excessCount === 0 && excessBytes === 0) return 0;
  // Round up to whole quanta so the cut stays put across many requests.
  const removeCount = Math.ceil(excessCount / limits.countQuantum) * limits.countQuantum;
  const removeBytes = Math.ceil(excessBytes / limits.byteQuantum) * limits.byteQuantum;
  let count = 0;
  let removed = 0;
  while (count < sizes.length && (count < removeCount || removed < removeBytes)) {
    removed += sizes[count];
    count += 1;
  }
  return count;
}

const isImage = block => block?.type === "image" && typeof block.data === "string";
const imageHolder = message => message?.role !== "assistant" && Array.isArray(message?.content);

/** Tool call id to the file it read, to tell the model where to look again. */
function readSources(messages) {
  const sources = new Map();
  for (const message of messages) {
    if (message?.role !== "assistant" || !Array.isArray(message.content)) continue;
    for (const block of message.content) {
      const args = block?.type === "toolCall" ? block.arguments : null;
      const path = args?.path ?? args?.file ?? args?.files?.[0]?.file;
      if (typeof path === "string") sources.set(block.id, path);
    }
  }
  return sources;
}

function placeholder(path) {
  const from = path ? ` from ${path}` : "";
  return `[Earlier image${from} omitted to keep the request within provider size limits. Read the file or take a new screenshot if you need it again.]`;
}

/** Messages with the oldest images replaced by text, or null if all fit. */
export function offloadImages(messages, limits) {
  const sizes = [];
  for (const message of messages) {
    if (imageHolder(message)) for (const block of message.content) if (isImage(block)) sizes.push(block.data.length);
  }
  let drop = dropCount(sizes, limits);
  if (drop === 0) return null;
  const sources = readSources(messages);
  return messages.map(message => {
    if (drop === 0 || !imageHolder(message) || !message.content.some(isImage)) return message;
    const content = message.content.map(block => {
      if (drop === 0 || !isImage(block)) return block;
      drop -= 1;
      return { type: "text", text: placeholder(sources.get(message.toolCallId)) };
    });
    return { ...message, content };
  });
}

/** The last recorded budget scale on this session branch. */
function storedScale(sessionManager) {
  let scale = 1;
  for (const entry of sessionManager?.getBranch?.() ?? []) {
    if (entry?.type === "custom" && entry.customType === SCALE_ENTRY && entry.data?.scale > 0) scale = entry.data.scale;
  }
  return Math.max(MIN_SCALE, Math.min(1, scale));
}

export default function (pi) {
  let scale = null;
  let shrunk = false;
  const current = ctx => (scale ??= storedScale(ctx?.sessionManager));

  pi.on("session_start", (_event, ctx) => {
    scale = storedScale(ctx?.sessionManager);
  });
  pi.on("tool_result", async (event, ctx) => {
    if (!resizeImage || !event.content?.some(block => isImage(block) && block.data.length > RESIZE.maxBytes)) return;
    // A model or user that sets its own image limits gets Pi's handling instead.
    if (ctx?.model?.inputLimits?.images?.resize || pi.getSettings?.()?.images?.autoResize === false) return;
    const content = await compactImages(event.content, resizeImage);
    if (!content) return;
    // Replacing content alone would drop structured output.
    return event.structuredContent === undefined ? { content } : { content, structuredContent: event.structuredContent };
  });
  pi.on("context", (event, ctx) => {
    const messages = offloadImages(event.messages, requestLimits(ctx?.model, current(ctx)));
    return messages ? { messages } : undefined;
  });
  pi.on("message_end", (event, ctx) => {
    const message = event.message;
    if (message?.role !== "assistant" || message.stopReason !== "error" || !TOO_LARGE.test(message.errorMessage ?? "")) return;
    const before = current(ctx);
    if (before <= MIN_SCALE) return;
    scale = Math.max(MIN_SCALE, before / 2);
    shrunk = true;
    pi.appendEntry(SCALE_ENTRY, { scale });
  });
  pi.on("agent_before_settle", event => {
    // One retry per halving: the floor bounds the loop.
    if (!shrunk) return;
    shrunk = false;
    if (event.outcome === "error" && event.context?.canContinue) return { continue: true };
  });
}
