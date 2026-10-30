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
      page: "https://zeron.sh/@ada",
      player: "https://zeron.sh/@ada/player",
      image: "https://zeron.sh/@ada/card.png",
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
      '<meta name="twitter:player" content="https://zeron.sh/@ada/player" />',
      '<meta name="twitter:player:width" content="480" />',
      '<meta name="twitter:player:height" content="480" />',
      '<meta name="twitter:image" content="https://zeron.sh/@ada/card.png" />',
      '<meta property="og:image" content="https://zeron.sh/@ada/card.png" />',
      '<meta property="og:url" content="https://zeron.sh/@ada" />',
      '<link rel="canonical" href="https://zeron.sh/@ada" />',
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
