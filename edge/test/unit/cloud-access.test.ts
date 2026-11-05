import { describe, expect, it } from "vitest";
import { cloudOffered } from "../../src/cloud/routes";

const me = { userId: "user_me", orgId: "org_me" };

describe("CLOUD_ACCESS", () => {
  it("offers Cloud to nobody when unset, except in local dev", () => {
    expect(cloudOffered({ AUTH_MODE: "workos" }, me)).toBe(false);
    expect(cloudOffered({ AUTH_MODE: "workos", CLOUD_ACCESS: "  " }, me)).toBe(false);
    expect(cloudOffered({ AUTH_MODE: "dev" }, me)).toBe(true);
  });

  it("offers Cloud to everyone, or to the listed users and orgs", () => {
    expect(cloudOffered({ AUTH_MODE: "workos", CLOUD_ACCESS: "everyone" }, me)).toBe(true);
    expect(cloudOffered({ AUTH_MODE: "workos", CLOUD_ACCESS: "user_a, user_me" }, me)).toBe(true);
    expect(cloudOffered({ AUTH_MODE: "workos", CLOUD_ACCESS: "user_a org_me" }, me)).toBe(true);
    expect(cloudOffered({ AUTH_MODE: "workos", CLOUD_ACCESS: "user_a,org_b" }, me)).toBe(false);
    expect(cloudOffered({ AUTH_MODE: "workos", CLOUD_ACCESS: "user_a" }, { userId: "user_me" })).toBe(false);
  });
});
