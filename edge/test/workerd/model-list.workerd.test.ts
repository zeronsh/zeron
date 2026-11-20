import { SELF } from "cloudflare:test";
import { describe, expect, it } from "vitest";

describe("hosted model list (/models/v1.json) in the worker", () => {
  it("is routed before sign-in: no bearer, and no list set here ⇒ 404, not 401", async () => {
    const response = await SELF.fetch("https://test/models/v1.json");
    expect(response.status).toBe(404);
    expect(await response.json()).toEqual({ error: "not_found" });
  });
});
