import { expect, test } from "vitest";
import { hadSession, resumeAddress } from "./resume.ts";

const form = (fields: Record<string, string>) => {
  const f = new FormData();
  for (const [k, v] of Object.entries(fields)) f.set(k, v);
  return f;
};

test("only the named fields come back, in the page's address", () => {
  expect(resumeAddress("/admin/x/1.0.0/reject", form({ reason: "listing", message: "Say why.", csrf: "secret" }), ["reason", "message"])).toBe(
    "/admin/x/1.0.0/reject?reason=listing&message=Say+why.",
  );
  expect(resumeAddress("/a", null, ["reason"])).toBe("/a");
});

test("what doesn't fit through sign-in is cut, by whole characters, and said", () => {
  const long = "é".repeat(2000);
  const address = resumeAddress("/admin/x/1.0.0/reject", form({ reason: "listing", message: long }), ["reason", "message"]);
  expect(address.length).toBeLessThanOrEqual(2048);
  const params = new URL(address, "https://zeron.sh").searchParams;
  expect(params.get("cut")).toBe("1");
  expect(params.get("reason")).toBe("listing");
  expect(long.startsWith(params.get("message")!)).toBe(true);
});

test("a session cookie, by the edge's name", () => {
  const request = (cookie: string) => new Request("https://zeron.sh/", { headers: { cookie } });
  expect(hadSession(request("a=1; __Host-zeron_session=x"))).toBe(true);
  expect(hadSession(request("__Host-zeron_flash=approved"))).toBe(false);
});
