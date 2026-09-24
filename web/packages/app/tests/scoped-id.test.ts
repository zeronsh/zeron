import { describe, expect, it } from "vitest";
import { RpcError } from "@zeron/engine-client";
import { SCOPED_ID_PREFIX, encodeScopedId, isScopedId, parseScopedId } from "@zeron/engine-client";

/**
 * Ticket 31's id codec — the port of `engine_registry/tests.rs::
 * scoped_identity_codec_is_collision_safe` (tests.rs:39-59): round-trips
 * for awkward raw ids under two engine keys, cross-key inequality, and
 * malformed payloads rejecting. The web scopes EVERY id uniformly (no
 * local engine to leave unscoped), so the unscoped half of the desktop's
 * matrix maps to `parse` returning the default-engine marker.
 */

const RAW_IDS = ["same-id", "engine:v1:invalid", "folder/??"];

describe("ScopedId", () => {
  it("scopedIdentityCodecIsCollisionSafe", () => {
    const remote = "http://remote:27699";
    const other = "http://other:27699";
    for (const raw of RAW_IDS) {
      for (const key of [remote, other]) {
        const encoded = encodeScopedId(key, raw);
        // The wire format: prefix + unpadded base64url of [engineKey, rawId].
        expect(encoded.startsWith(SCOPED_ID_PREFIX)).toBe(true);
        expect(encoded.slice(SCOPED_ID_PREFIX.length)).not.toMatch(/[+/=]/u);
        expect(parseScopedId(encoded)).toEqual({ engine: key, rawId: raw });
      }
      // An id scoped under one engine must never collide with the same raw
      // bytes scoped under another.
      expect(encodeScopedId(remote, raw)).not.toBe(encodeScopedId(other, raw));
      // A raw id that already carries the prefix must not collide with a
      // DIFFERENT engine's scoping of it either.
      expect(encodeScopedId(remote, "engine:v1:literal-id")).not.toBe(
        encodeScopedId(other, "engine:v1:literal-id"),
      );
    }
    // Unscoped ids parse as the default-engine marker and keep their bytes.
    expect(parseScopedId("same-id")).toEqual({ engine: null, rawId: "same-id" });
    expect(isScopedId("same-id")).toBe(false);
    expect(isScopedId(encodeScopedId(remote, "same-id"))).toBe(true);
    // Malformed payloads reject exactly as the desktop's parse errors do.
    expect(() => parseScopedId("engine:v1:bad")).toThrow(RpcError);
    expect(() => parseScopedId("engine:v1:!not-base64url!")).toThrow(RpcError);
    expect(() => parseScopedId(`${SCOPED_ID_PREFIX}${btoa("[]")}`)).toThrow(RpcError);
    expect(() => parseScopedId(`${SCOPED_ID_PREFIX}${btoa(JSON.stringify(["", "raw"]))}`)).toThrow(
      /Missing engine identity/u,
    );
  });
});
