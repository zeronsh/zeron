import { describe, expect, it } from "vitest";
import { SESSION_HASH_HEADER, SESSION_ROOM_HEADER, SESSION_STORE_HEADER } from "./browser-sessions";
import { AUTH_USER_HEADER, ROOM_KIND_HEADER } from "./env";
import { forwardedHeaders } from "./forwarded-headers";

describe("forwardedHeaders", () => {
  it("replaces all Worker-controlled trust headers", () => {
    const headers = forwardedHeaders(
      new Request("https://edge.test/device/id/status", {
        headers: {
          [AUTH_USER_HEADER]: "spoofed-user",
          [ROOM_KIND_HEADER]: "workspace",
          [SESSION_HASH_HEADER]: "spoofed-session",
          [SESSION_ROOM_HEADER]: "spoofed-room",
          [SESSION_STORE_HEADER]: "1",
          range: "bytes=0-10"
        }
      }),
      "verified-user"
    );

    expect(headers.get(AUTH_USER_HEADER)).toBe("verified-user");
    expect(headers.get(ROOM_KIND_HEADER)).toBeNull();
    expect(headers.get(SESSION_HASH_HEADER)).toBeNull();
    expect(headers.get(SESSION_ROOM_HEADER)).toBeNull();
    expect(headers.get(SESSION_STORE_HEADER)).toBeNull();
    expect(headers.get("range")).toBe("bytes=0-10");
  });

  it("sets room kind only after the caller authorizes a workspace forward", () => {
    const headers = forwardedHeaders(new Request("https://edge.test/workspace/org/ws"), "verified-user", "workspace");
    expect(headers.get(ROOM_KIND_HEADER)).toBe("workspace");
  });
});
