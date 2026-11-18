/**
 * The engine's deploy-key vector (docs/plugin-package-vectors.json), end to
 * end at the edge: its key registered with its own proof, then its exact
 * request (bytes, path, timestamp, Authorization header) accepted as a
 * deploy key's publish of its plugin, on the edge's clock at that moment.
 */
import { SELF, env } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import vectors from "../../../docs/plugin-package-vectors.json";
import type { Env } from "../../src/env";
import { readBody } from "../../src/store/identity/body";
import { verifySignedRequest } from "../../src/store/identity/signed-request";
import { person, setOwner, stepUp } from "./identity-helpers";

const v = vectors.deploy_key;
const body = Uint8Array.from(atob(v.request.body_b64), (c) => c.charCodeAt(0));

const vectorRequest = () =>
  new Request(`https://edge.test${v.request.path}`, {
    method: v.request.method,
    headers: { authorization: v.request.authorization, "x-zeron-content-sha256": v.request.body_sha256 },
    body
  });

describe("the engine's deploy-key vector at the edge", () => {
  it("registers with its proof and publishes with its exact request", async () => {
    const owner = await person();
    await setOwner("focus", owner);
    const registered = await SELF.fetch("https://edge.test/store/v1/plugins/focus/keys", {
      method: "POST",
      headers: { authorization: `Bearer ${owner}`, "content-type": "application/json" },
      body: JSON.stringify({ key: v.public, name: "vector", proof: v.proof, stepUp: await stepUp(owner) })
    });
    expect(registered.status).toBe(201);

    const request = vectorRequest();
    const caller = await verifySignedRequest(request, env as unknown as Env, new URL(request.url), v.request.ts);
    expect(caller).toEqual({ userId: owner, username: null, key: v.public, pluginId: "focus" });
    expect(await readBody(request)).toEqual(body);
    // Once only, and never outside the clock skew.
    const again = vectorRequest();
    const replayed = (await verifySignedRequest(again, env as unknown as Env, new URL(again.url), v.request.ts)) as Response;
    expect(((await replayed.json()) as { error: string }).error).toBe("replayed_request");
    const late = vectorRequest();
    const stale = (await verifySignedRequest(late, env as unknown as Env, new URL(late.url), v.request.ts + v.max_skew_ms + 1)) as Response;
    expect(((await stale.json()) as { error: string }).error).toBe("stale_request");
  });
});
