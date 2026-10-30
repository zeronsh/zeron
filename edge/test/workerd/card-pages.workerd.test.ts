import { SELF, env } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import { PUBLIC_KEYS } from "../../../apps/card-web/site/pages.mjs";
import { imageKey } from "../../src/card-image";
import { PAGE_CSP, PLAYER_CSP } from "../../src/card-routes";
import { pngChunkTypes } from "../../src/png";
import { signWorkOsPayload } from "../../src/user-directory";
import type { PublicCard } from "../../src/user-directory";

const SECRET = "workos_whsec_test";
const JOINED = "2026-10-01T10:00:00.000Z";
const USER = "user_01PAGESTEST0000000000001";
const EMAIL = "ada.pages@example.com";

const webhook = async (event: string, data: Record<string, unknown>) => {
  const payload = JSON.stringify({ id: `event_${crypto.randomUUID()}`, event, data, created_at: new Date().toISOString() });
  const t = Date.now();
  const v1 = await signWorkOsPayload(SECRET, t, payload);
  return SELF.fetch("https://edge.test/workos/webhook", {
    method: "POST",
    headers: { "content-type": "application/json", "workos-signature": `t=${t}, v1=${v1}` },
    body: payload
  });
};

/** A member with a username: signed up through the webhook (first and last
 * name, an email), then claimed as `ada-pages` with dev auth. */
const member = async (): Promise<PublicCard> => {
  await webhook("user.created", {
    object: "user",
    id: USER,
    email: EMAIL,
    first_name: "Ada",
    last_name: "Lovelace",
    profile_picture_url: "https://workoscdn.com/images/v1/abc",
    email_verified: true,
    created_at: JOINED,
    updated_at: JOINED
  });
  const claimed = await SELF.fetch("https://edge.test/username", {
    method: "POST",
    headers: { authorization: `Bearer ${USER}`, "content-type": "application/json" },
    body: JSON.stringify({ username: "ada-pages" })
  });
  expect([200, 409]).toContain(claimed.status);
  const read = await SELF.fetch("https://edge.test/cards/ada-pages");
  expect(read.status).toBe(200);
  return (await read.json()) as PublicCard;
};

/** The `#card` data block of a page, parsed. */
const dataBlock = (html: string): Record<string, unknown> => {
  const m = /<script type="application\/json" id="card">(.*?)<\/script>/s.exec(html);
  if (!m) throw new Error("no data block");
  return JSON.parse(m[1]!) as Record<string, unknown>;
};

/** Nothing a page serves may name what the public card doesn't: the last
 * name, the email, the user id, a picture URL. */
const PRIVATE = ["Lovelace", EMAIL, USER, "workoscdn", "profile_picture", "email_verified"];
const scan = (text: string) => {
  for (const word of PRIVATE) expect(text, `found ${word}`).not.toContain(word);
};

describe("member card pages", () => {
  it("serves the share page with the Player Card tags, the player, and only the public card", async () => {
    const card = await member();
    expect(Object.keys(card)).toEqual([...PUBLIC_KEYS]);
    expect(card.firstName).toBe("Ada");

    const page = await SELF.fetch("https://edge.test/@ada-pages");
    expect(page.status).toBe(200);
    expect(page.headers.get("content-type")).toBe("text/html; charset=utf-8");
    expect(page.headers.get("content-security-policy")).toBe(PAGE_CSP);
    expect(page.headers.get("x-frame-options")).toBeNull();
    expect(page.headers.get("cache-control")).toBe("public, max-age=60");
    const html = await page.text();
    expect(html).toContain('<meta name="twitter:card" content="player" />');
    expect(html).toContain('<meta name="twitter:player" content="https://edge.test/@ada-pages/player" />');
    expect(html).toContain('<meta name="twitter:image" content="https://edge.test/@ada-pages/card.png" />');
    expect(html).toContain('<meta property="og:image" content="https://edge.test/@ada-pages/card.png" />');
    expect(html).toContain('<script type="module" src="https://edge.test/card/card-page.js"></script>');
    expect(html).toContain("<title>Ada — Zeron member #");
    expect(html).not.toContain("noindex");
    expect(dataBlock(html)).toEqual(card);
    scan(html);

    const player = await SELF.fetch("https://edge.test/@Ada-Pages/player");
    expect(player.status).toBe(200);
    expect(player.headers.get("content-security-policy")).toBe(PLAYER_CSP);
    expect(player.headers.get("content-security-policy")).toContain("frame-ancestors 'self' https://x.com https://*.x.com https://twitter.com https://*.twitter.com");
    expect(player.headers.get("x-frame-options")).toBeNull();
    const playerHtml = await player.text();
    expect(dataBlock(playerHtml)).toEqual(card);
    expect(playerHtml).not.toContain("noindex");
    scan(playerHtml);

    scan(JSON.stringify(card));
  });

  it("serves the picture from R2 once drawn, and a brief 404 before", async () => {
    const card = await member();
    const before = await SELF.fetch("https://edge.test/@ada-pages/card.png");
    expect(before.status).toBe(404);
    expect(before.headers.get("cache-control")).toBe("public, max-age=60");

    // A picture with a text chunk, as a browser might write one: served as
    // stored (the drawing path strips chunks before storing).
    const bytes = new Uint8Array([137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130]);
    await env.CARD_IMAGES!.put(await imageKey(card), bytes, { httpMetadata: { contentType: "image/png" } });
    const after = await SELF.fetch("https://edge.test/@ada-pages/card.png");
    expect(after.status).toBe(200);
    expect(after.headers.get("content-type")).toBe("image/png");
    expect(after.headers.get("cache-control")).toBe("public, max-age=3600");
    const body = new Uint8Array(await after.arrayBuffer());
    expect(body).toEqual(bytes);
    expect(pngChunkTypes(body)).toEqual(["IEND"]);
  });

  it("answers a name no one holds, and anything else under a name, with a 404", async () => {
    const missing = await SELF.fetch("https://edge.test/@nobody-here");
    expect(missing.status).toBe(404);
    expect(await missing.text()).toContain("No card here");
    expect((await SELF.fetch("https://edge.test/@nobody-here/player")).status).toBe(404);
    expect((await SELF.fetch("https://edge.test/@nobody-here/card.png")).status).toBe(404);
    await member();
    expect((await SELF.fetch("https://edge.test/@ada-pages/other")).status).toBe(404);
    expect((await SELF.fetch("https://edge.test/@ada-pages/player/more")).status).toBe(404);
  });
});
