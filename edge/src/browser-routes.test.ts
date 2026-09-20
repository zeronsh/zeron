import { describe, expect, it } from "vitest";
import { deviceRoomOnline, previewOrigin, rewritePreviewLocation } from "./browser-routes";

describe("browser preview origin and redirects", () => {
  it("only permits HTTPS preview origins for the Secure host-only capability cookie", () => {
    expect(previewOrigin({ BROWSER_PREVIEW_ORIGIN: "https://preview.example/" })?.toString()).toBe("https://preview.example/");
    for (const value of ["http://localhost:8787/", "https://preview.example/path", "https://preview.example/?x=1"]) {
      expect(previewOrigin({ BROWSER_PREVIEW_ORIGIN: value })).toBeUndefined();
    }
  });


  it("keeps controlled localhost aliases inside the capability origin", () => {
    const current = new URL("https://p-ticket.preview.example/app/");
    expect(rewritePreviewLocation("http://device.project.localhost:5173/path?q=1#part", current)).toBe("https://p-ticket.preview.example/path?q=1#part");
    expect(rewritePreviewLocation("https://example.com/path?q=1#part", current)).toBe("https://example.com/path?q=1#part");
  });
});


describe("browser device discovery", () => {
  it("treats an individual Durable Object status failure as offline", async () => {
    const failing = { fetch: async () => { throw new Error("stale room"); } } as unknown as DurableObjectStub;
    const online = { fetch: async () => new Response(JSON.stringify({ hostConnected: true })) } as unknown as DurableObjectStub;
    await expect(deviceRoomOnline(failing, "owner")).resolves.toBe(false);
    await expect(deviceRoomOnline(online, "owner")).resolves.toBe(true);
  });
});
