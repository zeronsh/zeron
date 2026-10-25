import { SELF, env } from "cloudflare:test";
import { describe, expect, it } from "vitest";

describe("release artifacts (zeron.sh/releases/*)", () => {
  it("caches artifacts for good, pointers briefly, and honors an upload's own policy", async () => {
    await env.RELEASES.put("zeron-1.2.3-linux-x86_64.tar.gz", "tarball");
    await env.RELEASES.put("latest.txt", "1.2.3");
    // The Android APK's stable name: replaced by every build.
    await env.RELEASES.put("zeron-android-arm64.apk", "apk", {
      httpMetadata: { contentType: "application/vnd.android.package-archive", cacheControl: "no-cache" }
    });

    const tarball = await SELF.fetch("https://test/releases/zeron-1.2.3-linux-x86_64.tar.gz");
    expect(tarball.headers.get("cache-control")).toBe("public, max-age=86400, immutable");
    expect(tarball.headers.get("content-type")).toBe("application/octet-stream");
    expect(await tarball.text()).toBe("tarball");

    const latest = await SELF.fetch("https://test/releases/latest.txt");
    expect(latest.headers.get("cache-control")).toBe("public, max-age=60");
    await latest.arrayBuffer();

    const apk = await SELF.fetch("https://test/releases/zeron-android-arm64.apk", { method: "HEAD" });
    expect(apk.status).toBe(200);
    expect(apk.headers.get("cache-control")).toBe("no-cache");
    expect(apk.headers.get("content-type")).toBe("application/vnd.android.package-archive");
  });
});
