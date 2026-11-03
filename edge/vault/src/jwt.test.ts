import { describe, expect, it } from "vitest";
import { decodeJwtPayload, jwtExpMs } from "./jwt";
import { toBase64Url, utf8 } from "./encoding";
import { fakeJwt } from "../test/support";

describe("JWT reads", () => {
  it("reads exp in milliseconds", () => {
    expect(jwtExpMs(fakeJwt({ exp: 1_800_000_000 }))).toBe(1_800_000_000_000);
  });

  it("treats a missing or bogus exp as absent", () => {
    expect(jwtExpMs(fakeJwt({ sub: "x" }))).toBeUndefined();
    expect(jwtExpMs(fakeJwt({ exp: "soon" }))).toBeUndefined();
    expect(jwtExpMs(fakeJwt({ exp: -5 }))).toBeUndefined();
  });

  it("rejects non-JWTs", () => {
    expect(decodeJwtPayload("opaque-token")).toBeUndefined();
    expect(decodeJwtPayload("a.b")).toBeUndefined();
    expect(decodeJwtPayload("a.!!!.c")).toBeUndefined();
    expect(decodeJwtPayload(`a.${toBase64Url(utf8("[1,2]"))}.c`)).toBeUndefined();
    expect(jwtExpMs("sk-ant-api03-key")).toBeUndefined();
  });

  it("decodes unicode claims", () => {
    expect(decodeJwtPayload(fakeJwt({ name: "Zoë" }))?.name).toBe("Zoë");
  });
});
