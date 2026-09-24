import { afterEach, describe, expect, it, vi } from "vitest";
import {
  IMAGE_CACHE_BUDGET_BYTES,
  __loadedBytesForTests,
  __resetAttachmentCacheForTests,
  attachmentCacheKey,
  beginAttachmentLoad,
  beginUploadProgress,
  endUploadProgress,
  getAttachmentSnapshot,
  loadAttachment,
  protectAttachments,
  seedAttachment,
  setUploadProgress,
  storeAttachmentError,
  subscribeAttachment,
  uploadProgressPercent,
} from "../src/state/attachment-cache";

afterEach(() => {
  __resetAttachmentCacheForTests();
});

const FAKE_IMAGE = { name: "a.png", mime: "image/png", bytes: new Uint8Array([1, 2, 3]) };

describe("attachment-cache snapshot lifecycle", () => {
  it("begins in the loading state until a load settles", () => {
    const snap = getAttachmentSnapshot("dev-1", "/host/a.png");
    expect(snap.state).toBe("loading");
    expect(snap.image).toBeNull();
  });

  it("seeds a loaded entry and observes it via subscription", () => {
    const calls: number[] = [];
    const sub = subscribeAttachment("dev-1", "/host/a.png", () => calls.push(1));
    expect(getAttachmentSnapshot("dev-1", "/host/a.png").state).toBe("loading");

    seedAttachment("dev-1", "/host/a.png", FAKE_IMAGE);
    expect(calls).toEqual([1]);
    const snap = getAttachmentSnapshot("dev-1", "/host/a.png");
    expect(snap.state).toBe("loaded");
    expect(snap.image).toEqual(FAKE_IMAGE);
    sub();
  });

  it("hides a load that was already claimed", () => {
    expect(beginAttachmentLoad("dev-1", "/host/a.png")).toBe(true);
    expect(beginAttachmentLoad("dev-1", "/host/a.png")).toBe(false);
  });

  it("transitions to error with a 2-second retry hint", () => {
    // Pretend a load was kicked.
    expect(beginAttachmentLoad("dev-1", "/host/a.png")).toBe(true);
    storeAttachmentError("dev-1", "/host/a.png");
    const snap = getAttachmentSnapshot("dev-1", "/host/a.png");
    expect(snap.state).toBe("error");
    expect(snap.retryIn).toBeGreaterThan(0);
    expect(snap.retryIn).toBeLessThanOrEqual(2000);
  });

  it("the 2s→15s retry ladder is bounded", async () => {
    expect(beginAttachmentLoad("dev-1", "/host/a.png")).toBe(true);
    // 1 error → ~2s wait
    storeAttachmentError("dev-1", "/host/a.png");
    const first = getAttachmentSnapshot("dev-1", "/host/a.png");
    expect(first.retryIn).toBeLessThanOrEqual(2000);

    // A later error after the wait elapsed ~ 4s, capped at 15s
    storeAttachmentError("dev-1", "/host/a.png");
    storeAttachmentError("dev-1", "/host/a.png");
    const later = getAttachmentSnapshot("dev-1", "/host/a.png");
    expect(later.retryIn).toBeLessThanOrEqual(15_000);
  });

  it("loadAttachment resolves with the read result when the call succeeds", async () => {
    // Build a tiny client mock that returns a single chunk with done=true.
    const client = {
      async call(method: string, params: { offset?: number }): Promise<unknown> {
        if (method === "ReadAttachmentChunk") {
          return {
            name: "shot.png",
            mimeType: "image/png",
            data: "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=",
            nextOffset: params.offset ?? 0,
            done: true,
          };
        }
        throw new Error(`unexpected method: ${method}`);
      },
    };

    await loadAttachment(client, "dev-1", "/host/shot.png");
    const snap = getAttachmentSnapshot("dev-1", "/host/shot.png");
    expect(snap.state).toBe("loaded");
    expect(snap.image?.name).toBe("shot.png");
    expect(snap.image?.bytes.byteLength).toBeGreaterThan(0);
  });

  it("loadAttachment marks the entry as errored when the call throws", async () => {
    const client = {
      async call(): Promise<unknown> {
        throw new Error("rpc failure");
      },
    };
    await loadAttachment(client, "dev-1", "/host/missing.png");
    const snap = getAttachmentSnapshot("dev-1", "/host/missing.png");
    expect(snap.state).toBe("error");
  });

  it("loadAttachment returns silently when the load was already claimed", async () => {
    const callMock = vi.fn(async (): Promise<unknown> => ({}));
    const client = { call: callMock };
    expect(beginAttachmentLoad("dev-1", "/host/a.png")).toBe(true);
    await loadAttachment(client, "dev-1", "/host/a.png");
    expect(callMock).not.toHaveBeenCalled();
  });
});

describe("attachment-cache unsubscribe", () => {
  it("drops the listener when the unsubscribe function is called", () => {
    let count = 0;
    const sub = subscribeAttachment("dev-1", "/host/x.png", () => {
      count += 1;
    });
    seedAttachment("dev-1", "/host/x.png", FAKE_IMAGE);
    expect(count).toBe(1);
    sub();
    seedAttachment("dev-1", "/host/x.png", FAKE_IMAGE);
    expect(count).toBe(1);
  });
});

// ---------------------------------------------------------------------------
// LRU / byte-budget eviction (`ImageCache::insert_loaded`,
// attachments.rs:599-631) + the protected set (:639-654).
// ---------------------------------------------------------------------------

const MI = 1024 * 1024;

function bigImage(bytes: number): { name: string; mime: string; bytes: Uint8Array } {
  return { name: "big.png", mime: "image/png", bytes: new Uint8Array(bytes) };
}

describe("attachment-cache eviction", () => {
  it("stays quiet under the 64 MiB budget", () => {
    seedAttachment("dev-1", "/a.png", bigImage(23 * MI));
    seedAttachment("dev-1", "/b.png", bigImage(23 * MI));
    expect(__loadedBytesForTests()).toBe(46 * MI);
    expect(getAttachmentSnapshot("dev-1", "/a.png").state).toBe("loaded");
    expect(getAttachmentSnapshot("dev-1", "/b.png").state).toBe("loaded");
  });

  it("evicts the globally-oldest entry once the budget is exceeded", () => {
    seedAttachment("dev-1", "/a.png", bigImage(23 * MI));
    seedAttachment("dev-1", "/b.png", bigImage(23 * MI));
    // A read bumps A's LRU tick past B's insert, so B is the oldest.
    expect(getAttachmentSnapshot("dev-1", "/a.png").state).toBe("loaded");
    seedAttachment("dev-1", "/c.png", bigImage(23 * MI));
    expect(__loadedBytesForTests()).toBe(46 * MI);
    expect(getAttachmentSnapshot("dev-1", "/b.png").state).toBe("loading");
    expect(getAttachmentSnapshot("dev-1", "/a.png").state).toBe("loaded");
    expect(getAttachmentSnapshot("dev-1", "/c.png").state).toBe("loaded");
  });

  it("never evicts the just-inserted key", () => {
    seedAttachment("dev-1", "/a.png", bigImage(40 * MI));
    // Inserting the SAME key again replaces it — the old bytes leave the
    // budget and the new entry cannot evict itself.
    seedAttachment("dev-1", "/a.png", bigImage(40 * MI));
    expect(__loadedBytesForTests()).toBe(40 * MI);
    expect(getAttachmentSnapshot("dev-1", "/a.png").state).toBe("loaded");
  });

  it("shields protected keys from eviction, replacing the set wholesale", () => {
    seedAttachment("dev-1", "/a.png", bigImage(23 * MI));
    seedAttachment("dev-1", "/b.png", bigImage(23 * MI));
    const keys = new Set([attachmentCacheKey("dev-1", "/b.png")]);
    protectAttachments(keys);
    seedAttachment("dev-1", "/c.png", bigImage(23 * MI));
    expect(__loadedBytesForTests()).toBe(46 * MI);
    expect(getAttachmentSnapshot("dev-1", "/a.png").state).toBe("loading");
    expect(getAttachmentSnapshot("dev-1", "/b.png").state).toBe("loaded");
    expect(getAttachmentSnapshot("dev-1", "/c.png").state).toBe("loaded");

    // A wholesale replace drops the old shield: B becomes evictable again.
    protectAttachments(new Set());
    seedAttachment("dev-1", "/d.png", bigImage(23 * MI));
    expect(__loadedBytesForTests()).toBe(46 * MI);
    expect(getAttachmentSnapshot("dev-1", "/b.png").state).toBe("loading");
    expect(getAttachmentSnapshot("dev-1", "/d.png").state).toBe("loaded");
  });

  it("stops evicting when everything left is protected", () => {
    seedAttachment("dev-1", "/a.png", bigImage(40 * MI));
    protectAttachments(new Set([attachmentCacheKey("dev-1", "/a.png")]));
    seedAttachment("dev-1", "/b.png", bigImage(40 * MI));
    // Nothing evictable: the budget overruns but both entries stay.
    expect(__loadedBytesForTests()).toBe(80 * MI);
    expect(getAttachmentSnapshot("dev-1", "/a.png").state).toBe("loaded");
    expect(getAttachmentSnapshot("dev-1", "/b.png").state).toBe("loaded");
    expect(80 * MI).toBeGreaterThan(IMAGE_CACHE_BUDGET_BYTES);
  });
});

// ---------------------------------------------------------------------------
// Send-wide upload progress (`begin_upload_progress` / `upload_progress_percent`)
// ---------------------------------------------------------------------------

describe("attachment-cache upload progress", () => {
  it("reports null when nothing is uploading", () => {
    expect(uploadProgressPercent()).toBeNull();
  });

  it("tracks the whole-send percent and clamps to 0..100", () => {
    beginUploadProgress(200);
    expect(uploadProgressPercent()).toBe(0);
    setUploadProgress(100);
    expect(uploadProgressPercent()).toBe(50);
    setUploadProgress(500);
    expect(uploadProgressPercent()).toBe(100);
    endUploadProgress();
    expect(uploadProgressPercent()).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// Error-snapshot stability (`useSyncExternalStore` needs a stable identity)
// ---------------------------------------------------------------------------

describe("attachment-cache error snapshot stability", () => {
  it("returns the same object while the retry bucket holds", () => {
    expect(beginAttachmentLoad("dev-1", "/host/x.png")).toBe(true);
    storeAttachmentError("dev-1", "/host/x.png");
    const first = getAttachmentSnapshot("dev-1", "/host/x.png");
    const second = getAttachmentSnapshot("dev-1", "/host/x.png");
    expect(first).toBe(second);
    expect(first.state).toBe("error");
    expect(first.retryIn).toBeGreaterThan(0);
  });
});