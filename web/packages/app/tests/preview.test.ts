import { describe, expect, it } from "vitest";
import {
  isLoopbackHost,
  previewEmptyCopy,
  previewProxyReachable,
  previewRowLabel,
  previewSubtitle,
  previewUrl,
} from "../src/lib/preview";

describe("previewUrl", () => {
  it("matches the wire PreviewService::url shape — hostname at the proxy port", () => {
    expect(previewUrl({ hostname: "macbook.my-app.localhost" }, 7331)).toBe(
      "http://macbook.my-app.localhost:7331",
    );
  });

  it("keeps the URL stable when a restarted dev server moves ports", () => {
    // Stability is engine-side (the catalog persists names across restarts);
    // the web side must derive the URL from the stable hostname, never from
    // the ephemeral service port.
    const before = previewUrl({ hostname: "macbook.my-app.localhost" }, 7331);
    const after = previewUrl({ hostname: "macbook.my-app.localhost" }, 7331);
    expect(after).toBe(before);
  });
});

describe("previewRowLabel", () => {
  it("is the bare local port for local chats, device-qualified for remote ones", () => {
    const service = { deviceName: "MacBook", port: 5173 };
    expect(previewRowLabel(service, false)).toBe("localhost:5173");
    expect(previewRowLabel(service, true)).toBe("MacBook · localhost:5173");
  });
});

describe("previewSubtitle and previewEmptyCopy", () => {
  it("mirror the desktop wording", () => {
    expect(previewSubtitle(false)).toBe("Running locally");
    expect(previewSubtitle(true)).toBe("Running on your device");
    expect(previewEmptyCopy(true, false)).toBe("Looking for dev servers…");
    expect(previewEmptyCopy(false, false)).toContain("Start a dev server in this project.");
    expect(previewEmptyCopy(false, true)).toContain("on your other device");
  });
});

describe("isLoopbackHost", () => {
  it("accepts the loopback family", () => {
    expect(isLoopbackHost("localhost")).toBe(true);
    expect(isLoopbackHost("macbook.my-app.localhost")).toBe(true);
    expect(isLoopbackHost("127.0.0.1")).toBe(true);
    expect(isLoopbackHost("127.0.1.20")).toBe(true);
    expect(isLoopbackHost("::1")).toBe(true);
    expect(isLoopbackHost("[::1]")).toBe(true);
  });

  it("rejects LAN, loopback-looking, and public names", () => {
    expect(isLoopbackHost("192.168.1.20")).toBe(false);
    expect(isLoopbackHost("10.0.0.5")).toBe(false);
    expect(isLoopbackHost("128.0.0.1")).toBe(false);
    expect(isLoopbackHost("localhost.evil.test")).toBe(false);
    expect(isLoopbackHost("engine.example.com")).toBe(false);
    expect(isLoopbackHost("")).toBe(false);
  });
});

describe("previewProxyReachable", () => {
  it("is true exactly when the engine endpoint is loopback from the browser", () => {
    expect(previewProxyReachable("http://127.0.0.1:27655")).toBe(true);
    expect(previewProxyReachable("http://localhost:27655")).toBe(true);
    expect(previewProxyReachable("http://[::1]:27655")).toBe(true);
    expect(previewProxyReachable("http://192.168.1.20:27655")).toBe(false);
    expect(previewProxyReachable("https://engine.example.com")).toBe(false);
    expect(previewProxyReachable("not a url")).toBe(false);
  });
});
