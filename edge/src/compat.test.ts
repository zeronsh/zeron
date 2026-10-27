import { describe, expect, it } from "vitest";
import { clientProtocol, registrySearch, syncGate, UPDATE_HEADER } from "./compat";

const request = (path: string, headers: Record<string, string> = {}) => new Request(`https://edge${path}`, { headers });
const gate = (min: string | undefined, path: string, headers: Record<string, string> = {}) => {
  const r = request(path, headers);
  return syncGate({ MIN_CLIENT_PROTOCOL: min }, r, new URL(r.url));
};

describe("client protocol", () => {
  it("reads the query, then the header, and treats anything else as 0", () => {
    expect(clientProtocol(request("/registry/o/ws?proto=3"))).toBe(3);
    expect(clientProtocol(request("/chat3/c/items", { "x-zeron-proto": "2" }))).toBe(2);
    expect(clientProtocol(request("/chat2/c/ws"))).toBe(0);
    expect(clientProtocol(request("/chat2/c/ws?proto=1.5"))).toBe(0);
    expect(clientProtocol(request("/chat2/c/ws?proto=-1"))).toBe(0);
  });

  it("stamps the caller's protocol on registry forwards", () => {
    const r = request("/registry/o/rows?since=4&beat=1", { "x-zeron-proto": "1" });
    expect(registrySearch(r, new URL(r.url))).toBe("?since=4&beat=1&proto=1");
    expect(registrySearch(r, new URL(r.url), "device=d")).toBe("?device=d&proto=1");
  });
});

describe("minimum protocol", () => {
  it("lets everyone through until the operator sets one", () => {
    expect(gate(undefined, "/registry/o/ws")).toBeUndefined();
    expect(gate("0", "/chat2/c/ws")).toBeUndefined();
  });

  it("refuses older clients on the sync routes only, as an app-wide update", async () => {
    const refused = gate("2", "/chat3/c/items", { "x-zeron-proto": "1" })!;
    expect(refused.status).toBe(426);
    expect(refused.headers.get(UPDATE_HEADER)).toBe("app");
    expect(((await refused.json()) as { error: string }).error).toBe("update_required");
    expect(gate("2", "/registry/o/ws")!.status, "a build from before protocols").toBe(426);
    expect(gate("2", "/registry/o/ws?proto=2")).toBeUndefined();
    expect(gate("2", "/cloud/o/sessions")).toBeUndefined();
  });
});
