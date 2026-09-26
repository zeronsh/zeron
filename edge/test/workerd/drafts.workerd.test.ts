import { SELF } from "cloudflare:test";
import { expect, it } from "vitest";

it("stores large immutable draft content independently of the registry and isolates users", async () => {
  const id = crypto.randomUUID();
  const path = `https://edge/draft-content/org/${id}`;
  const headers = { authorization: "Bearer alice@org" };
  const body = JSON.stringify({ prompt: "Long prompt ".repeat(10000) });
  expect((await SELF.fetch(path, { method: "PUT", headers, body })).status).toBe(200);
  expect((await SELF.fetch(path, { method: "PUT", headers, body })).status).toBe(200);
  expect((await SELF.fetch(path, { method: "PUT", headers, body: "different" })).status).toBe(409);
  expect(new TextDecoder().decode(await (await SELF.fetch(path, { headers })).arrayBuffer())).toBe(body);
  expect((await SELF.fetch(path, { headers: { authorization: "Bearer bob@org" } })).status).toBe(404);
  expect((await SELF.fetch(path, { headers: { authorization: "Bearer alice@other" } })).status).toBe(403);
});
