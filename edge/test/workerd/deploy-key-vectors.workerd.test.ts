/**
 * The engine's deploy-key vectors (docs/plugin-package-vectors.json), end to
 * end at the edge: the key registered to an account with the vector's
 * scope, then its exact requests (method, path, headers, body), the publish
 * and the signed read of `GET /store/v1/me/plugins`, each accepted once as
 * that account's deploy key, on the edge's clock at the vector's moment.
 * (A v2 proof binds a publisher id no test account has, so its message and
 * signature are the unit test's; here the key is bound directly.)
 */
import { SELF, env } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import vectors from "../../../docs/plugin-package-vectors.json";
import type { Env } from "../../src/env";
import { readBody } from "../../src/store/identity/body";
import { verifySignedRequest } from "../../src/store/identity/signed-request";
import { stepUpHash } from "../../src/step-up";
import { directory, person, setOwner, stepUp } from "./identity-helpers";

const v = vectors.deploy_key;
const bodyOf = (b64: string) => Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));

/** The vector's request exactly: its method, path, headers and body. */
const vectorRequest = (r: typeof v.request | typeof v.get) =>
  new Request(`https://edge.test${r.path}`, {
    method: r.method,
    headers: r.headers,
    body: r.method === "GET" ? undefined : bodyOf(r.body_b64)
  });

const verify = (request: Request, at: number) => verifySignedRequest(request, env as unknown as Env, new URL(request.url), at);

describe("the engine's deploy-key vector at the edge", () => {
  it("registers with its proof, then publishes and reads with its exact requests, each once", async () => {
    const owner = await person();
    await setOwner("focus", owner);
    const scope = v.proofs.find((p) => "plugins" in p.scope)!.scope as { plugins: string[] };
    expect(await directory().registerDeployKey(owner, v.public, "vector", null, scope, await stepUpHash(await stepUp(owner)))).toBe("registered");

    for (const r of [v.request, v.get]) {
      const request = vectorRequest(r);
      expect(await verify(request, r.ts), r.method).toEqual({ userId: owner, username: null, key: v.public, name: "vector", scope, createdAt: expect.any(Number) });
      expect(await readBody(request)).toEqual(bodyOf(r.body_b64));
      // Once only, and never outside the clock skew.
      const replayed = (await verify(vectorRequest(r), r.ts)) as Response;
      expect(((await replayed.json()) as { error: string }).error).toBe("replayed_request");
      const stale = (await verify(vectorRequest(r), r.ts + v.max_skew_ms + 1)) as Response;
      expect(((await stale.json()) as { error: string }).error).toBe("stale_request");
    }
  });
});
