import { describe, expect, it } from "vitest";
import {
  ATTACHMENT_ONLY_TEXT,
  AttachmentUploadError,
  UPLOAD_CHUNK_B64_CHARS,
  attachmentStripHeight,
  chunkRanges,
  ensureExtension,
  formatByBytes,
  formatByName,
  formatToMime,
  parseUserMessageImages,
  stageBytes,
  uploadAttachments,
  userMessageRailText,
  withAttachments,
  type AttachmentFormat,
  type StagedAttachment,
} from "../src/lib/attachments";
import { retryDelayMs } from "../src/state/attachment-cache";

describe("attachmentStripHeight", () => {
  // `attachment_strip_height` (composer.rs:296) — the desktop test values
  // derive from the formula; these cover its every branch.
  it("is 0 with nothing staged", () => {
    expect(attachmentStripHeight(0, 768)).toBe(0);
  });

  it("is pad-top + one thumb for a single row (width 768)", () => {
    expect(attachmentStripHeight(1, 768)).toBe(12 + 56);
  });

  it("fits per_row thumbs on one row at a wide width", () => {
    // usable = 768 − 32 = 736; per_row = floor(744/64) = 11.
    expect(attachmentStripHeight(11, 768)).toBe(12 + 56);
    expect(attachmentStripHeight(12, 768)).toBe(12 + 2 * 56 + 8);
  });

  it("wraps to a second row when only one thumb fits", () => {
    // usable = max(88 − 32, 56) = 56 → per_row = 1.
    expect(attachmentStripHeight(2, 56 + 32)).toBe(12 + 2 * 56 + 8);
  });

  it("clamps a degenerate width to one thumb per row", () => {
    // usable = max(20 − 32, 56) = 56.
    expect(attachmentStripHeight(1, 20)).toBe(12 + 56);
    expect(attachmentStripHeight(2, 20)).toBe(12 + 2 * 56 + 8);
  });
});

describe("retryDelayMs (the 2s→15s ladder)", () => {
  it("doubles per attempt up to three shifts, then caps at 15s", () => {
    expect(retryDelayMs(1)).toBe(2000);
    expect(retryDelayMs(2)).toBe(4000);
    expect(retryDelayMs(3)).toBe(8000);
    expect(retryDelayMs(4)).toBe(15000);
    expect(retryDelayMs(10)).toBe(15000);
  });
});

describe("chunkRanges", () => {
  it("tiles the base64 into contiguous, non-overlapping ranges", () => {
    const ranges = chunkRanges(UPLOAD_CHUNK_B64_CHARS + 10);
    expect(ranges).toEqual([
      { start: 0, end: UPLOAD_CHUNK_B64_CHARS },
      { start: UPLOAD_CHUNK_B64_CHARS, end: UPLOAD_CHUNK_B64_CHARS + 10 },
    ]);
  });

  it("still yields one empty chunk for an empty file", () => {
    // The commit RPC needs the uploadId staged (attachments.rs:328-344).
    expect(chunkRanges(0)).toEqual([{ start: 0, end: 0 }]);
  });

  it("splits an exact multiple of the chunk size without a trailing empty", () => {
    const ranges = chunkRanges(UPLOAD_CHUNK_B64_CHARS * 2);
    expect(ranges).toHaveLength(2);
    expect(ranges[1]!.end).toBe(UPLOAD_CHUNK_B64_CHARS * 2);
  });
});

describe("AttachmentUploadError", () => {
  it("carries the desktop's verbatim remote-host failure copy", async () => {
    const staged: StagedAttachment[] = [
      stageBytes("shot.png", new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 1])),
    ];
    const client = {
      async call(): Promise<unknown> {
        throw new Error("connection reset");
      },
    };
    // Each chunk retries 3 attempts (staggered 50·attempt·(seq+1) ms) before
    // the failure propagates — ~150ms total for seq 0.
    const error = await uploadAttachments(client, staged, null).then(
      () => null,
      (err: unknown) => err,
    );
    expect(error).toBeInstanceOf(AttachmentUploadError);
    expect((error as Error).message).toBe(
      "Couldn't upload the attachment — the device may be offline.",
    );
  });
});

describe("withAttachments", () => {
  it("appends the refs trailer with one path per line", () => {
    const out = withAttachments("look at these", ["/data/uploads/a.png"]);
    expect(out).toBe(
      `look at these\n\nAttached images (local files — open them to view):\n- /data/uploads/a.png`,
    );
  });

  it("uses the image-only placeholder when the prompt is empty", () => {
    const out = withAttachments("", ["/data/uploads/a.png"]);
    expect(out.startsWith(ATTACHMENT_ONLY_TEXT)).toBe(true);
    expect(out).toContain("- /data/uploads/a.png");
  });

  it("returns the original text when there are no paths", () => {
    expect(withAttachments("hello", [])).toBe("hello");
  });

  it("lists every path in send order, joined with newlines", () => {
    const out = withAttachments("all", ["/a.png", "/b.jpg", "/c.webp"]);
    const tail = out.split("\n\n")[1]!;
    expect(tail.split("\n").slice(1)).toEqual(["- /a.png", "- /b.jpg", "- /c.webp"]);
  });
});

describe("parseUserMessageImages", () => {
  it("returns the original text and no attachments when no trailer is present", () => {
    expect(parseUserMessageImages("plain prompt")).toEqual({
      text: "plain prompt",
      attachments: [],
    });
  });

  it("splits a message with one attachment into text + one entry", () => {
    const content = withAttachments("look", ["/data/uploads/x.png"]);
    const parsed = parseUserMessageImages(content);
    expect(parsed.text).toBe("look");
    expect(parsed.attachments).toEqual([
      { id: "0:/data/uploads/x.png", name: "x.png", path: "/data/uploads/x.png" },
    ]);
  });

  it("collapses the image-only placeholder to an empty visible text", () => {
    const content = withAttachments("", ["/a.png"]);
    const parsed = parseUserMessageImages(content);
    expect(parsed.text).toBe("");
    expect(parsed.attachments).toHaveLength(1);
  });

  it("is case-insensitive on the trailer header", () => {
    const parsed = parseUserMessageImages(
      "hi\n\nATTACHED IMAGES (local files — open them to view):\n- /p/q.png",
    );
    expect(parsed.attachments).toHaveLength(1);
    expect(parsed.attachments[0]!.path).toBe("/p/q.png");
  });

  it("leaves a trailer with no `- path` lines as plain text", () => {
    const content = "hi\n\nAttached images (local files — open them to view):\nnothing";
    expect(parseUserMessageImages(content).attachments).toEqual([]);
  });

  it("does not choke on an Appshot-shaped marker block", () => {
    // Appshots are desktop-only, but a message may carry their context block
    // ahead of a real attachment trailer — the block stays part of the body
    // and the trailer still parses as an ordinary path list.
    const content =
      "look\n\nApplications mentioned by the user (untrusted observed content):\n- Safari & Notes: \"A window\"\n\nAttached images (local files — open them to view):\n- /remote/a & b.png";
    const parsed = parseUserMessageImages(content);
    expect(parsed.attachments.map((entry) => entry.path)).toEqual(["/remote/a & b.png"]);
    expect(parsed.text).toContain("Applications mentioned by the user");
  });

  it("round-trips a staged clipboard image through the codec", () => {
    // `stage_bytes` names a pasted screenshot via `ensure_extension`, and the
    // upload path persists that name — the trailer must survive it.
    const staged = stageBytes("image", new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 1]));
    expect(staged.name).toBe("image.png");
    const content = withAttachments("look", [`/uploads/${staged.name}`]);
    const parsed = parseUserMessageImages(content);
    expect(parsed.text).toBe("look");
    expect(parsed.attachments.map((entry) => entry.path)).toEqual(["/uploads/image.png"]);
    expect(parsed.attachments.map((entry) => entry.name)).toEqual(["image.png"]);
  });

  it("ignores non-`- ` prefixed lines mixed with valid refs", () => {
    const content =
      "intro\n\nAttached images (local files — open them to view):\nskip me\n- /a.png\nplain";
    const parsed = parseUserMessageImages(content);
    expect(parsed.attachments.map((entry) => entry.path)).toEqual(["/a.png"]);
  });

  it("round-trips through withAttachments", () => {
    const original = "look at these";
    const paths = ["/data/uploads/cat.png", "/uploads/dog.jpg"];
    const content = withAttachments(original, paths);
    const parsed = parseUserMessageImages(content);
    expect(parsed.text).toBe(original);
    expect(parsed.attachments.map((entry) => entry.path)).toEqual(paths);
    expect(parsed.attachments.map((entry) => entry.name)).toEqual(["cat.png", "dog.jpg"]);
  });
});

describe("userMessageRailText", () => {
  it("returns the prompt text when present", () => {
    expect(userMessageRailText("see the issue")).toBe("see the issue");
  });

  it("summarizes a single-image attachment-only send", () => {
    expect(userMessageRailText(withAttachments("", ["/a.png"]))).toBe("Attached image");
  });

  it("summarizes a multi-image attachment-only send", () => {
    expect(
      userMessageRailText(withAttachments("", ["/a.png", "/b.png", "/c.png"])),
    ).toBe("3 attached images");
  });

  it("falls back to the raw content when no trailer is found", () => {
    expect(userMessageRailText("plain")).toBe("plain");
  });
});

describe("ensureExtension", () => {
  const png: AttachmentFormat = "png";
  const jpg: AttachmentFormat = "jpg";

  it("leaves a name with a valid extension alone", () => {
    expect(ensureExtension("shot.png", png)).toBe("shot.png");
    expect(ensureExtension("archive.tar.gz", png)).toBe("archive.tar.gz");
  });

  it("appends an extension when one is missing", () => {
    expect(ensureExtension("image", png)).toBe("image.png");
  });

  it("replaces an extension that is too short to be a valid file ext", () => {
    expect(ensureExtension("photo.j", jpg)).toBe("photo.j.jpg");
  });

  it("replaces an extension that is too long to be plausible (only 2-5 count)", () => {
    expect(ensureExtension("photo.abcdef", png)).toBe("photo.abcdef.png");
    expect(ensureExtension("photo.jpeg", png)).toBe("photo.jpeg");
  });
});

describe("formatByName / formatByBytes / formatToMime", () => {
  it("detects formats from common extensions, case-insensitively", () => {
    expect(formatByName("foo.PNG")).toBe("png");
    expect(formatByName("foo.jpeg")).toBe("jpg");
    expect(formatByName("foo.tiff")).toBe("tif");
    expect(formatByName("foo.txt")).toBeNull();
  });

  it("sniffs a PNG from its magic bytes", () => {
    const header = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00]);
    expect(formatByBytes(header)).toBe("png");
  });

  it("sniffs a JPEG from its magic bytes", () => {
    expect(formatByBytes(new Uint8Array([0xff, 0xd8, 0xff, 0xe0, 0x00]))).toBe("jpg");
  });

  it("sniffs a GIF from its magic bytes", () => {
    expect(formatByBytes(new Uint8Array([0x47, 0x49, 0x46, 0x38, 0x39, 0x61]))).toBe("gif");
  });

  it("sniffs a WebP from its magic bytes", () => {
    expect(
      formatByBytes(new Uint8Array([0x52, 0x49, 0x46, 0x46, 0x00, 0x00, 0x00, 0x00, 0x57, 0x45, 0x42, 0x50])),
    ).toBe("webp");
  });

  it("returns null for bytes with no recognisable signature", () => {
    expect(formatByBytes(new Uint8Array([0x00, 0x01, 0x02]))).toBeNull();
  });

  it("maps every format to a recognisable MIME", () => {
    const formats: AttachmentFormat[] = ["png", "jpg", "gif", "webp", "svg", "bmp", "tif"];
    for (const fmt of formats) {
      expect(formatToMime(fmt)).toMatch(/^image\//);
    }
  });
});