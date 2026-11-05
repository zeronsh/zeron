import { describe, expect, it } from "vitest";
import { PUBLIC_KEYS, escapeHtml, nameOf, publicCard, renderPlayer, renderShare, urlsFor } from "../../../apps/card-web/site/pages.mjs";
import { pngChunkTypes, stripPng } from "../../src/png";

const card = {
  username: "ada",
  firstName: "Ada",
  userNumber: 182,
  joinedOn: "2026-10-01",
  badges: [{ id: "cloud-tester", name: "Cloud Tester", icon: null }],
  scene: 3,
  holo: "glyph",
  sky: 3389156178
};

/** The `#card` data block of a page, parsed. */
const dataBlock = (html: string): Record<string, unknown> => {
  const m = /<script type="application\/json" id="card">(.*?)<\/script>/s.exec(html);
  if (!m) throw new Error("no data block");
  return JSON.parse(m[1]!) as Record<string, unknown>;
};

describe("card pages", () => {
  const urls = urlsFor(card, "https://zeron.sh");

  it("names the player, the picture and the assets from the site", () => {
    expect(urls).toEqual({
      page: "https://zeron.sh/c/ada",
      player: "https://zeron.sh/c/ada/player",
      image: "https://zeron.sh/c/ada/card.png",
      json: "https://zeron.sh/cards/ada",
      script: "https://zeron.sh/card/card.js",
      page_script: "https://zeron.sh/card/card-page.js",
      style: "https://zeron.sh/card/card.css",
      manifest: "https://zeron.sh/card/art/manifest.json"
    });
  });

  it("embeds exactly the public card, keys in the edge's order, and nothing more", () => {
    const leaky = { ...card, lastName: "Lovelace", email: "ada@example.com", userId: "user_01X", badges: [{ ...card.badges[0], grantedAt: 1, note: "secret" }] };
    for (const html of [renderShare(leaky, urls), renderPlayer(leaky, urls)]) {
      const data = dataBlock(html);
      expect(Object.keys(data)).toEqual([...PUBLIC_KEYS]);
      expect(Object.keys((data.badges as object[])[0]!)).toEqual(["id", "name", "icon"]);
      for (const word of ["Lovelace", "ada@example.com", "user_01X", "grantedAt", "secret"]) expect(html).not.toContain(word);
    }
  });

  it("shows the first name only, and the handle when there is none", () => {
    const html = renderShare(card, urls, { site: "@zeron" });
    expect(html).toContain('<meta name="twitter:title" content="Ada — Zeron member #182" />');
    expect(html).toContain('<meta name="twitter:site" content="@zeron" />');
    expect(nameOf({ ...card, firstName: null })).toBe("@ada");
    expect(renderPlayer({ ...card, firstName: null }, urls)).toContain("<title>@ada — Zeron</title>");
  });

  it("carries the Player Card and Open Graph tags with absolute URLs, and no noindex", () => {
    const html = renderShare(card, urls);
    for (const tag of [
      '<meta name="twitter:card" content="player" />',
      '<meta name="twitter:player" content="https://zeron.sh/c/ada/player" />',
      '<meta name="twitter:player:width" content="480" />',
      '<meta name="twitter:player:height" content="480" />',
      '<meta name="twitter:image" content="https://zeron.sh/c/ada/card.png" />',
      '<meta property="og:image" content="https://zeron.sh/c/ada/card.png" />',
      '<meta property="og:url" content="https://zeron.sh/c/ada" />',
      '<link rel="canonical" href="https://zeron.sh/c/ada" />',
      '<script type="module" src="https://zeron.sh/card/card-page.js"></script>'
    ]) {
      expect(html).toContain(tag);
    }
    expect(html).not.toContain("noindex");
    expect(renderPlayer(card, urls)).not.toContain("noindex");
  });

  it("escapes member-written text and keeps the data block closed", () => {
    const hostile = { ...card, firstName: `</script><script>alert(1)</script>"'<b>`, badges: [{ id: "x", name: "<img src=x onerror=alert(1)>", icon: null }] };
    for (const html of [renderShare(hostile, urls), renderPlayer(hostile, urls)]) {
      expect(html).not.toContain("<script>alert");
      expect(html).not.toContain("<img src=x");
      expect(html).toContain("\\u003c/script>");
      expect(dataBlock(html).firstName).toBe(hostile.firstName);
    }
    expect(escapeHtml(`<a href="x">'&'</a>`)).toBe("&lt;a href=&quot;x&quot;&gt;&#39;&amp;&#39;&lt;/a&gt;");
  });

  it("reduces a card to its public fields", () => {
    expect(publicCard({ ...card, extra: 1 } as typeof card)).toEqual(card);
  });
});

describe("png stripping", () => {
  const chunk = (type: string, data: Uint8Array): Uint8Array => {
    const out = new Uint8Array(12 + data.length);
    new DataView(out.buffer).setUint32(0, data.length);
    out.set([...type].map((c) => c.charCodeAt(0)), 4);
    out.set(data, 8);
    return out;
  };
  const join = (...parts: Uint8Array[]): Uint8Array => {
    const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
    let o = 0;
    for (const p of parts) {
      out.set(p, o);
      o += p.length;
    }
    return out;
  };
  const signature = new Uint8Array([137, 80, 78, 71, 13, 10, 26, 10]);

  it("keeps the picture and drops every other chunk", () => {
    const png = join(signature, chunk("IHDR", new Uint8Array(13)), chunk("tEXt", new Uint8Array([65, 0, 66])), chunk("eXIf", new Uint8Array(4)), chunk("IDAT", new Uint8Array(3)), chunk("tIME", new Uint8Array(7)), chunk("IEND", new Uint8Array(0)));
    expect(pngChunkTypes(png)).toEqual(["IHDR", "tEXt", "eXIf", "IDAT", "tIME", "IEND"]);
    expect(pngChunkTypes(stripPng(png))).toEqual(["IHDR", "IDAT", "IEND"]);
    expect(() => stripPng(new Uint8Array([1, 2, 3]))).toThrow("not a PNG");
  });
});

describe("the card's badges", async () => {
  const { layoutCard, BADGE_STYLES } = await import("../../../apps/card-web/site/card.js");
  const { readFileSync } = await import("node:fs");
  const mock = JSON.parse(readFileSync(new URL("../../../apps/card-web/mock/members.json", import.meta.url), "utf8")) as {
    members: (typeof card & { badges: { id: string; name: string; icon: null }[] })[];
  };
  type Box = { x: number; y: number; w: number; h: number; border?: number; fill?: unknown };
  type Run = { text: string; x: number; baseline: number };
  /** The badge pills (20 high) and their labels, left to right. */
  const pills = (member: object) => {
    const { boxes, runs } = layoutCard(member) as { boxes: Box[]; runs: Run[] };
    const row = boxes.filter((b) => b.h === 20).sort((a, b) => a.x - b.x);
    const labels = runs.filter((r) => row.some((b) => r.x > b.x && r.x < b.x + b.w && r.baseline > b.y && r.baseline < b.y + b.h));
    return { row, labels: labels.sort((a, b) => a.x - b.x).map((r) => r.text) };
  };

  it("draws V3 BETA as an outline pill like Cloud Tester's, in the read's order", () => {
    expect(BADGE_STYLES).toMatchObject({ "cloud-tester": "outline", "v3-beta": "outline" });
    const graham = mock.members.find((m) => m.username === "graham")!;
    expect(graham.badges.map((b) => b.id)).toEqual(["cloud-tester", "v3-beta"]);
    const both = pills(graham);
    expect(both.labels).toEqual(["CLOUD TESTER", "V3 BETA"]);
    expect(both.row).toHaveLength(2);
    for (const pill of both.row) expect(pill).toMatchObject({ border: 1 });
    for (const pill of both.row) expect(pill.fill).toBeUndefined();
    // One row, 4 apart; V3 BETA's pill fits its seven characters.
    expect(both.row[1]!.y).toBe(both.row[0]!.y);
    expect(both.row[1]!.x).toBe(both.row[0]!.x + both.row[0]!.w + 4);
    expect(both.row[1]!.w).toBe(8 + 7 * (0.6 * 10 + 1) + 8 + 2);

    const ada = mock.members.find((m) => m.username === "ada")!;
    expect(pills(ada)).toMatchObject({ labels: ["V3 BETA"] });
    expect(pills(ada).row[0]).toMatchObject({ border: 1 });
  });

  it("draws a badge it has no style for as an outline pill, as the native app does", () => {
    const graham = mock.members.find((m) => m.username === "graham")!;
    const { row, labels } = pills({ ...graham, badges: [{ id: "early-tester", name: "Early Tester", icon: null }] });
    expect(labels).toEqual(["EARLY TESTER"]);
    expect(row[0]).toMatchObject({ border: 1 });
    expect(row[0]!.fill).toBeUndefined();
  });

  it("draws Maintainer as a dark (ink) pill, first, as the edge serves it", () => {
    expect(BADGE_STYLES).toMatchObject({ maintainer: "ink" });
    const graham = mock.members.find((m) => m.username === "graham")!;
    const { row, labels } = pills({
      ...graham,
      badges: [{ id: "maintainer", name: "Maintainer", icon: null }, ...graham.badges]
    });
    expect(labels).toEqual(["MAINTAINER", "CLOUD TESTER", "V3 BETA"]);
    // The ink pill's fill, no border; the others stay outline pills.
    expect(row[0]).toMatchObject({ fill: [0x16, 0x16, 0x16, 0x80] });
    expect(row[0]!.border).toBeUndefined();
    for (const pill of row.slice(1)) expect(pill).toMatchObject({ border: 1 });
  });

  it("gives every mock member's badges in one order, as the edge serves them", () => {
    const order = ["cloud-tester", "v3-beta"];
    for (const m of mock.members) {
      const ids = m.badges.map((b) => b.id);
      expect(ids).toEqual(order.filter((id) => ids.includes(id)));
      for (const b of m.badges) expect(Object.keys(b)).toEqual(["id", "name", "icon"]);
      expect(pills(m).labels).toEqual(m.badges.map((b) => b.name.toUpperCase()));
    }
    expect(mock.members.filter((m) => m.badges.some((b) => b.id === "v3-beta")).map((m) => m.badges.find((b) => b.id === "v3-beta"))).toContainEqual({
      id: "v3-beta",
      name: "V3 BETA",
      icon: null
    });
  });
});
