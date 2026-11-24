// The built site as it deploys (its Worker on workerd, its files from
// dist/client) against the fake edge: what each route answers, to whom,
// with which headers, and that nothing a plugin's author writes runs.
import { readFileSync } from "node:fs";
import { afterAll, beforeAll, describe, expect, test } from "vitest";
import { hasInlineScript, rawElements } from "../src/lib/store/security.ts";
import { fileUrl } from "../src/lib/store/urls.ts";

/** A page runs only the site's own script files: no inline script, no handler, no javascript: link. */
function inert(body: string) {
  for (const e of rawElements(body)) if (e.tag === "script") expect(e.attributes).toMatch(/\bsrc="\/_astro\/|type="application\/ld\+json"/);
  const markup = body.replace(/="[^"]*"/g, '=""');
  expect(markup).not.toMatch(/<[a-z][^>]*\son[a-z]+=/i);
  expect(body).not.toMatch(/(href|src)="\s*(javascript|data|vbscript):/i);
}
import { startSite, type Site } from "./harness.ts";

let site: Site;
beforeAll(async () => {
  site = await startSite();
}, 60_000);
afterAll(() => site?.dispose());

const html = async (path: string, cookie?: string) => {
  const response = await site.fetch(path, cookie ? { headers: { cookie } } : {});
  return { response, body: await response.text() };
};

/** A form post from the site's own pages, as a browser sends one. */
const post = (path: string, cookie: string, fields: Record<string, string>, origin = "https://zeron.sh") =>
  site.fetch(path, { method: "POST", headers: { cookie, origin, "content-type": "application/x-www-form-urlencoded" }, body: new URLSearchParams(fields).toString() });

/** The one-shot message a response set, as a cookie to send back. */
const flashOf = (response: Response) => response.headers.getSetCookie().find((c) => c.startsWith("__Host-zeron_flash="))?.split(";")[0] ?? "";

/** A version's state on its author's page, as its row's badge says it. */
/** A version's row on one of your plugins' pages. */
const rowOf = (body: string, version: string) => body.split('<li class="vr').find((r) => r.includes(`<b class="mono">${version}</b>`)) ?? "";
/** Where a version stands in its row: its status pill, or Listed when it has none (amendment 45). */
const stateOf = (body: string, version: string) => (rowOf(body, version) ? (/<span class="state [a-z]+" data-kind="status">([^<]+)<\/span>/.exec(rowOf(body, version))?.[1] ?? "Listed") : undefined);
/** A version's second badge in its row: whether a reviewer verified it (amendment 45), when it's listed or yanked. */
const verifiedOf = (body: string, version: string) => /data-kind="verified">(?:<svg[^]*?<\/svg>)?([^<]+)<\/span>/.exec(rowOf(body, version))?.[1];

/** The session's CSRF token, as a page carries it in its forms. */
const tokenOf = (body: string) => /name="csrf" value="([^"]+)"/.exec(body)?.[1] ?? "";

/** The config the build baked in (dist/server/wrangler.json): production's, or staging's when built with CLOUDFLARE_ENV=staging. */
const BAKED = JSON.parse(readFileSync(new URL("../dist/server/wrangler.json", import.meta.url), "utf8"));
const STAGING = process.env.CLOUDFLARE_ENV === "staging";

describe("the deploy config", () => {
  test("production is the zeron.sh worker reading the production edge; staging, its own worker reading the staging edge", () => {
    const config = BAKED;
    if (STAGING) {
      // (npm run deploy:staging: its own worker, which holds only its own domain and so can never take zeron.sh.)
      expect(config.name).toBe("zeron-landing-staging");
      expect(config.routes).toEqual([{ pattern: "site-staging.zeron.sh", custom_domain: true }]);
      expect(config.services).toEqual([{ binding: "EDGE", service: "zeron-edge-staging" }]);
      expect(config.vars).toEqual({ SITE_ORIGIN: "https://site-staging.zeron.sh" });
    } else {
      expect(config.name).toBe("comet-landing");
      expect(config.routes.map((r: { pattern: string }) => r.pattern)).toEqual(["zeron.sh", "comet.zeron.sh"]);
      expect(config.services).toEqual([{ binding: "EDGE", service: "comet-native-edge" }]);
      expect(config.vars).toEqual({ SITE_ORIGIN: "https://zeron.sh" });
    }
    expect(config.assets.binding).toBe("ASSETS");
    expect(config.assets.not_found_handling).toBeUndefined();
    expect(config.cache).toEqual({ enabled: true });
    expect(config.limits).toEqual({ cpu_ms: 3000 });
  });
});

describe("the site's own pages", () => {
  test("prerendered pages are files, served without the server's headers", async () => {
    for (const path of ["/", "/privacy"]) {
      const { response, body } = await html(path);
      expect(response.status, path).toBe(200);
      expect(body).toBe(readFileSync(new URL(`../dist/client${path === "/" ? "/index" : path}.html`, import.meta.url), "utf8"));
      // (The Worker's middleware adds a CSP to what it renders; a file has none.)
      expect(response.headers.get("content-security-policy"), path).toBeNull();
    }
  });

  test("an unknown path is the site's 404 page, with 404", async () => {
    const { response, body } = await html("/no/such/page");
    expect(response.status).toBe(404);
    expect(body).toBe(readFileSync(new URL("../dist/client/404.html", import.meta.url), "utf8"));
  });

  test("robots and the sitemaps", async () => {
    expect((await html("/robots.txt")).body).toContain("Sitemap: https://zeron.sh/sitemap.xml");
    expect((await html("/sitemap.xml")).body).toContain("<loc>https://zeron.sh/plugins/sitemap.xml</loc>");
    const plugins = await html("/plugins/sitemap.xml");
    expect(plugins.response.headers.get("content-type")).toContain("application/xml");
    expect(plugins.body).toContain("<loc>https://zeron.sh/plugins/focus-timer</loc>");
    expect(plugins.body).toContain("<loc>https://zeron.sh/plugins?category=git</loc>");
    // (Nothing of it listed: every version taken down.)
    expect(plugins.body).not.toContain("clipboard-sync");
  });
});

describe("the store's front (/plugins)", () => {
  test("public, cached for a minute at most, no inline script, a strict CSP", async () => {
    const { response, body } = await html("/plugins");
    expect(response.status).toBe(200);
    expect(response.headers.get("cache-control")).toBe("public, max-age=30");
    expect(response.headers.get("cloudflare-cdn-cache-control")).toBe("public, max-age=30, stale-while-revalidate=30");
    const csp = response.headers.get("content-security-policy")!;
    expect(csp).toMatch(/script-src 'self';/);
    expect(csp).not.toContain("unsafe");
    expect(response.headers.get("x-frame-options")).toBe("DENY");
    expect(hasInlineScript(body)).toBe(false);
    expect(body).toContain("Focus timer");
    expect(body).toContain('<h2 id="featured">Featured</h2>');
  });

});

describe("All plugins (/plugins/all)", () => {
  /** The plugins a list's page shows, in order. */
  const listed = (body: string) => [...body.matchAll(/<a class="card res h\d+" href="\/plugins\/([^"]+)"/g)].map((m) => m[1]);

  test("public, cached for a minute at most, a strict CSP; the columns, a row a plugin", async () => {
    const { response, body } = await html("/plugins/all");
    expect(response.status).toBe(200);
    expect(response.headers.get("cache-control")).toBe("public, max-age=30");
    expect(response.headers.get("cloudflare-cdn-cache-control")).toBe("public, max-age=30, stale-while-revalidate=30");
    expect(response.headers.get("content-security-policy")).toMatch(/script-src 'self';/);
    expect(hasInlineScript(body)).toBe(false);
    inert(body);
    // A card a plugin, each on its own hue; the facets say where you are.
    expect(listed(body)).toHaveLength(24);
    expect(body).toMatch(/<a href="\/plugins\/all" aria-current="page"><span>All<\/span><span class="num">\d+<\/span><\/a>/);
    expect(body).toMatch(/<a href="\/plugins\/all" aria-current="true"><i aria-hidden="true"><\/i>Everything<\/a>/);
  });

  test("Next and Previous are real addresses that carry the cursor, and come back to the same list", async () => {
    const first = await html("/plugins/all");
    const next = /href="([^"]+)" rel="next"/.exec(first.body)?.[1]?.replaceAll("&amp;", "&");
    expect(next).toMatch(/^\/plugins\/all\?cursor=/);
    expect(first.body).toMatch(/<span class="btn sec sm" aria-disabled="true">.*?Previous<\/span>/);
    const second = await html(next!);
    expect(second.response.status).toBe(200);
    expect(listed(second.body)).not.toEqual(listed(first.body));
    const previous = /href="([^"]+)" rel="prev"/.exec(second.body)?.[1]?.replaceAll("&amp;", "&");
    expect(previous).toMatch(/^\/plugins\/all/);
    expect(listed((await html(previous!)).body)).toEqual(listed(first.body));
  });

  test("a card's foot says what the list is sorted by; one not verified says so instead", async () => {
    const foot = (body: string) => [...body.matchAll(/<span class="count">(Updated|Published|Verified) ([^<]+)<\/span>/g)].map((m) => [m[1], m[2]]);
    expect(new Set(foot((await html("/plugins/all?sort=updated")).body).map(([w]) => w))).toEqual(new Set(["Updated"]));
    expect((await html("/plugins/all")).body).toMatch(/<span class="state no">[\d.]+(-[\w.]+)? not verified yet<\/span>/);
    // Newest: the day each was first listed (amendment 39), the newest first.
    const newest = foot((await html("/plugins/all?sort=new")).body);
    expect(new Set(newest.map(([w]) => w))).toEqual(new Set(["Published"]));
    const days = newest.map(([, d]) => Date.parse(`${d}${/\d{4}/.test(d!) ? "" : `, ${new Date().getUTCFullYear()}`}`));
    expect(days.length).toBeGreaterThan(1);
    expect(days).toEqual([...days].sort((a, b) => b - a));
    // Newly verified (Discover's shelf): by when Zeron verified each, said.
    const verified = foot((await html("/plugins/all?sort=verified")).body);
    expect(verified.length).toBeGreaterThan(1);
    expect(verified.every(([w]) => w === "Verified")).toBe(true);
  });

  test("the pager: Previous, where you are of how many, Next", async () => {
    const first = await html("/plugins/all");
    const total = Number(/<b class="num">([\d,]+)<\/b> plugins/.exec(first.body)![1]!.replace(",", ""));
    expect(first.body).toContain(`<span class="num">1–24 of ${total}</span>`);
    const next = /href="([^"]+)" rel="next"/.exec(first.body)![1]!.replaceAll("&amp;", "&");
    expect(next).toMatch(/&page=2$/);
    expect((await html(next)).body).toMatch(new RegExp(`<span class="num">25–\\d+ of ${total}</span>`));
  });

  test("the sort is a row of real addresses, the chosen one marked; a search adds Best match", async () => {
    const { body } = await html("/plugins/all");
    expect(body).toMatch(/<a href="\/plugins\/all" aria-current="page">Most installed<\/a><a href="\/plugins\/all\?sort=updated">Recently updated<\/a><a href="\/plugins\/all\?sort=new">Newest<\/a>/);
    const search = await html("/plugins/all?q=timer");
    expect(search.body).toMatch(/aria-current="page">Best match<\/a>/);
  });

  test("search, category, keyword; the count when the list says it exactly", async () => {
    const timer = await html("/plugins/all?q=timer");
    expect(listed(timer.body)).toContain("focus-timer");
    expect(timer.body).toMatch(/<h1[^>]*>Results for “timer”<\/h1>/);
    // The count, always (amendment 38's total).
    expect(timer.body).toMatch(/<p id="results"><b class="num">\d+<\/b> plugins?<\/p>/);
    // (The field keeps the words.)
    expect(timer.body).toContain('name="q" value="timer"');
    const empty = await html("/plugins/all?q=zzzzqqq");
    expect(empty.body).toContain("Nothing matches “zzzzqqq”");
    expect(empty.body).toContain('href="/plugins/all">Clear search</a>');
    expect(empty.body).toContain('<b class="num">0</b> plugins');
    const git = await html("/plugins/all?category=git");
    expect(listed(git.body)).toContain("pr-radar");
    expect(listed(git.body)).not.toContain("token-meter");
    expect(git.body).toContain('<link rel="canonical" href="https://zeron.sh/plugins/all?category=git">');
    expect(git.body).toMatch(/<a href="\/plugins\/all\?category=git" aria-current="page"><span>Git<\/span><span class="num">\d+<\/span><\/a>/);
    // The category's keywords, each a link that narrows it; the chosen one marked, and a link that lets it go.
    expect(git.body).toMatch(/<a class="tag" href="\/plugins\/all\?category=git&amp;keyword=[a-z]+" title="[a-z]+">/);
    const kw = await html("/plugins/all?category=git&keyword=github");
    expect(kw.body).toContain('<a class="tag" href="/plugins/all?category=git" title="github" aria-current="true">github</a>');
    expect(listed((await html("/plugins/all?keyword=github")).body)).toContain("pr-radar");
  });

  test("an address that says more than it means is sent to the plain one; /plugins?… is All plugins'", async () => {
    for (const [asked, plain] of [["/plugins/all?sort=popular&utm_source=x", "/plugins/all"], ["/plugins?q=timer", "/plugins/all?q=timer"], ["/plugins?category=git", "/plugins/all?category=git"]]) {
      const response = await site.fetch(asked!);
      expect(response.status, asked).toBe(301);
      expect(response.headers.get("location"), asked).toBe(plain);
    }
  });

  test("search results stay out of search engines; the list and its categories don't", async () => {
    expect((await html("/plugins/all?q=timer")).body).toContain('<meta name="robots" content="noindex, nofollow">');
    expect((await html("/plugins/all")).body).not.toContain('name="robots"');
    expect((await html("/plugins/all?category=git")).body).not.toContain('name="robots"');
  });
});

describe("a plugin's page", () => {
  test("its README, what it asks for, Install in Zeron", async () => {
    const { response, body } = await html("/plugins/pr-radar");
    expect(response.status).toBe(200);
    expect(response.headers.get("cloudflare-cdn-cache-control")).toBe("public, max-age=30, stale-while-revalidate=30");
    expect(body).toContain('href="zeron://open/plugin/pr-radar" data-install>');
    expect(body).toContain('<h3 id="user-content-what-it-does">What it does</h3>');
    expect(body).toContain('"@type":"SoftwareApplication"');
    expect(hasInlineScript(body)).toBe(false);
    // All of what it asks for, on its own tab, in the app's words; the risky last, in the danger's ink.
    const asks = (await html("/plugins/pr-radar/asks")).body;
    expect(asks).toContain("Run gh, git on your computer as you");
    expect(asks).toContain("Install it only if you trust @kai.");
    expect(asks).toMatch(/<li class="ar risky">/);
  });

  test("the page's rules: one primary button, its own colour, its pictures, the README under the page, its parts as pages", async () => {
    const { body } = await html("/plugins/focus-timer");
    // One action, one primary button; what else is said, in words.
    expect(body.match(/class="btn pri/g)).toHaveLength(1);
    expect(body).toContain(">Nothing happened?</button>");
    // On its own hue; its pictures a strip on it, one at a time.
    expect(body).toMatch(/<main id="main" class="h\d+">/);
    expect(body.match(/<a class="slide"/g)).toHaveLength(2);
    // The README doesn't say the name again, and nothing in it outweighs the title.
    // (One heading names it; drawn its first letter's side bearing left: lib/store/bearing.ts.)
    expect(body.match(/<h[1-6][^>]*>(<span class="name">)?Focus timer/g)).toEqual(['<h1 class="lsb-5"><span class="name">Focus timer']);
    expect(body).not.toMatch(/<article class="prose"[^>]*>[^]*?<h1/);
    // Its parts are pages of their own, each a real address; Report is in the rail.
    expect(body).toMatch(/<a href="\/plugins\/focus-timer" aria-current="page">Overview<\/a><a href="\/plugins\/focus-timer\/versions">Versions<span class="num">\d+<\/span><\/a><a href="\/plugins\/focus-timer\/asks">What it asks for<span class="num">\d+<\/span><\/a>/);
    expect(body).toContain('<a href="/plugins/focus-timer/report">');
    // Every version verified: the seal by the name, no amber note.
    expect(body).toContain('<span class="state ok">');
    expect(body).not.toContain('<p class="note">');
  });

  test("its newest not verified: the amber note, the newest verified to install instead; none verified: said so", async () => {
    const worst = (await html("/plugins/worst-case-listing-at-every-limit-xyz12")).body;
    expect(worst).toContain('<span class="state no">Not verified yet</span>');
    expect(worst).toMatch(/<p class="note"><b>1\.5\.0 isn&#39;t verified yet\. 1\.0\.0 is\.<\/b>/);
    const none = (await html("/plugins/voice-memos")).body;
    expect(none).toContain('<p class="note"><b>No version of this plugin is verified yet.</b> Zeron asks before it installs it.</p>');
  });

  test("a hostile plugin's words are only words", async () => {
    const { body } = await html("/plugins/xss-probe");
    // (Tags only: an attribute's value may hold "<script>" as inert text.)
    const markup = body.replace(/="[^"]*"/g, '=""');
    // (Every script the page's own: its data block, or a file of the site's.)
    for (const tag of body.replace(/="[^"]*"/g, (v) => (/^="(application\/ld\+json|\/_astro\/[^"]*)"$/.test(v) ? v : '=""')).matchAll(/<script\b[^>]*>/g)) expect(tag[0]).toMatch(/type="application\/ld\+json"|src="\/_astro\//);
    expect(body).toContain('<script type="application/ld+json">');
    expect(markup).not.toMatch(/<img[^>]+onerror/i);
    expect(markup).not.toMatch(/<svg[^>]+onload/i);
    expect(body).not.toMatch(/href="(javascript|data|vbscript):/i);
    expect(body).not.toMatch(/<(iframe|form action="https)/i);
    expect(body).toContain("&lt;img src=x onerror=&quot;alert(&#39;name&#39;)&quot;&gt; Probe");
    // The structured data can't close its own script.
    const ld = /<script type="application\/ld\+json">([^]*?)<\/script>/.exec(body)![1]!;
    expect(ld).not.toContain("</");
    expect(JSON.parse(ld).name).toContain("<img");
  });

  test("a name with a right-to-left override can't turn the page's words around", async () => {
    const { body } = await html("/plugins/xss-probe");
    const title = /<title>([^<]*)<\/title>/.exec(body)![1]!;
    expect(title).not.toMatch(/[\u202a-\u202e\u2066-\u2069]/);
    expect(title).toContain("a plugin for Zeron");
    expect(/<meta name="description" content="([^"]*)"/.exec(body)![1]).not.toMatch(/[\u202a-\u202e\u2066-\u2069]/);
    // (In the page, it stays as written, in an element of its own.)
    expect(body).toMatch(/<h1><span class="name">[^<]*\u202eevil<\/span>/);
  });

  test("an author's links are followed only as parsed https addresses", async () => {
    const { body } = await html("/plugins/focus-timer");
    expect(body).toContain('href="https://example.com/focus-timer" rel="nofollow noopener ugc"');
    expect(body).toContain('href="https://github.com/ada/focus-timer" rel="nofollow noopener ugc"');
  });

  test("a 404 the Worker draws (a plugin, a version) carries the site's CSP and security headers, and is never kept; the site's 404 file keeps its own", async () => {
    for (const path of ["/plugins/no-such-plugin", "/plugins/Bad_Id", "/plugins/focus-timer/versions/9.9.9"]) {
      const { response, body } = await html(path);
      expect(response.status, path).toBe(404);
      expect(body.length, path).toBeGreaterThan(0);
      expect(response.headers.get("content-security-policy"), path).toMatch(/^default-src 'none'; script-src 'self'; style-src 'self' /);
      expect(response.headers.get("x-content-type-options"), path).toBe("nosniff");
      expect(response.headers.get("cache-control"), path).toMatch(/no-store/);
      inert(body);
    }
    // (An address with nothing at all, and someone else's plugin among yours, are the site's 404 page as a file, with the file's headers.)
    for (const [path, cookie] of [["/no/such/page", undefined], ["/account/plugins/pr-radar", await site.signIn("user_ada")]] as const) {
      const missing = await html(path, cookie);
      expect(missing.response.status, path).toBe(404);
      expect(missing.body, path).toBe(readFileSync(new URL("../dist/client/404.html", import.meta.url), "utf8"));
    }
  });

  test("unknown, unlisted and malformed ids are one 404, in the shell, saying nothing of why", async () => {
    const pages = await Promise.all(["/plugins/no-such-plugin", "/plugins/clipboard-sync", "/plugins/crypto-ticker", "/plugins/Bad_Id", "/plugins/a%2Fb"].map(async (path) => ({ path, ...(await html(path)) })));
    for (const { path, response, body } of pages) {
      expect(response.status, path).toBe(404);
      expect(body, path).toContain("No plugin is listed here");
      expect(response.headers.get("x-robots-tag") ?? body, path).toMatch(/noindex/);
    }
    // (Each the same but for its own address: its canonical, and where sign-in comes back to.)
    const same = (body: string) => body.replace(/(rel="canonical" href|property="og:url" content)="[^"]*"/g, "").replace(/return_to=[^"]*/g, "");
    expect(new Set(pages.map((p) => same(p.body))).size).toBe(1);
  });

  test("a query on a plugin's page goes to its one address", async () => {
    const response = await site.fetch("/plugins/focus-timer?x=1");
    expect(response.status).toBe(301);
    expect(response.headers.get("location")).toBe("/plugins/focus-timer");
  });
});

describe("package files", () => {
  test("a listed version's picture: typed by its name, sandboxed, a minute in shared caches", async () => {
    const response = await site.fetch("/plugins/focus-timer/files/1.2.0/icon.svg");
    expect(response.status).toBe(200);
    expect(response.headers.get("content-type")).toBe("image/svg+xml");
    expect(response.headers.get("content-security-policy")).toContain("sandbox");
    expect(response.headers.get("x-content-type-options")).toBe("nosniff");
    expect(response.headers.get("cloudflare-cdn-cache-control")).toBe("public, max-age=60");
    expect(response.headers.get("cache-control")).toBe("public, max-age=60");
  });

  test("pictures that aren't what they say are still only pictures: typed by name, sandboxed, never sniffed", async () => {
    for (const [file, type] of [["script.svg", "image/svg+xml"]]) {
      const response = await site.fetch(`/plugins/theme-sync/files/1.0.0/shots/${file}`);
      expect(response.status, file).toBe(200);
      expect(response.headers.get("content-type"), file).toBe(type);
      expect(response.headers.get("x-content-type-options"), file).toBe("nosniff");
      expect(response.headers.get("content-disposition"), file).toBe("inline");
      const csp = response.headers.get("content-security-policy") ?? "";
      expect(csp, file).toContain("sandbox");
      expect(csp, file).toContain("default-src 'none'");
      expect(csp, file).not.toContain("script");
      expect(response.headers.get("cross-origin-resource-policy"), file).toBe("same-origin");
    }
    // A file the edge gives only as a download (an HTML page named .png) is never shown inline from here,
    // and every answer of this route, a refusal too, carries the sandbox.
    const attached = await site.fetch("/plugins/theme-sync/files/1.0.0/shots/page.png");
    expect(attached.status).toBe(404);
    expect(await attached.text()).toBe("");
    expect(attached.headers.get("content-security-policy")).toContain("sandbox");
    expect(attached.headers.get("x-content-type-options")).toBe("nosniff");
    // Anything else in a package is never served from here, whatever it's called.
    for (const file of ["polyglot.gif", "page.html", "x.js", "x.htm", "x.xml", "x.svgz", "x.json", "x.webp", "x"]) {
      expect((await site.fetch(`/plugins/theme-sync/files/1.0.0/shots/${file}`)).status, file).toBe(404);
    }
  });

  test("a path with spaces, # and % is a file's, encoded part by part", async () => {
    const response = await site.fetch(fileUrl("xss-probe", "1.0.0", "shots/a b#1 100%.svg"));
    expect(response.status).toBe(200);
    expect(response.headers.get("content-type")).toBe("image/svg+xml");
  });

  test("one address per file, page and sitemap: a query or a doubled slash goes to the plain one", async () => {
    for (const [asked, plain] of [["/plugins/focus-timer/files/1.2.0/icon.svg?v=2", "/plugins/focus-timer/files/1.2.0/icon.svg"], ["/plugins/sitemap.xml?n=1", "/plugins/sitemap.xml"], ["/plugins//focus-timer", "/plugins/focus-timer"], ["https://zeron.sh//plugins/focus-timer", "/plugins/focus-timer"]]) {
      const response = await site.fetch(asked);
      expect(response.status, asked).toBe(301);
      expect(response.headers.get("location"), asked).toBe(plain);
    }
  });

  test("a name that is an encoded dot is a name, not a climb: the edge's 404, sandboxed", async () => {
    const response = await site.fetch("/plugins/focus-timer/files/1.2.0/%252e%252e/icon.svg");
    expect(response.status).toBe(404);
    expect(response.headers.get("content-security-policy")).toContain("sandbox");
    expect(response.headers.get("x-content-type-options")).toBe("nosniff");
  });

  test("only pictures pass, and no path climbs out", async () => {
    for (const path of ["/plugins/focus-timer/files/1.2.0/init.luau", "/plugins/focus-timer/files/1.2.0/README.md", "/plugins/focus-timer/files/1.2.0/..%2f..%2fsecret.png", "/plugins/focus-timer/files/v1/icon.svg"]) {
      expect((await site.fetch(path)).status, path).toBe(404);
    }
  });

  test("a version taken down: nobody's but its author's and the admins', and kept by no one; one waiting for a reviewer is everyone's", async () => {
    // (Amendment 45: a version is listed at once, verified or not.)
    expect((await site.fetch("/plugins/focus-timer/files/1.3.0-beta.1/icon.svg")).status).toBe(200);
    const path = "/plugins/worst-case-listing-at-every-limit-xyz12/files/1.2.0/icon.svg";
    expect((await site.fetch(path)).status).toBe(404);
    expect((await site.fetch(path, { headers: { cookie: await site.signIn("user_kai") } })).status).toBe(404);
    for (const user of ["user_ada", "user_grace"]) {
      const response = await site.fetch(path, { headers: { cookie: await site.signIn(user) } });
      expect(response.status, user).toBe(200);
      expect(response.headers.get("cache-control"), user).toBe("private, no-store");
      expect(response.headers.get("cloudflare-cdn-cache-control") ?? "", user).not.toMatch(/public/);
    }
  });
});

describe("public pages: the same for everyone signed out (a shared cache keeps them); drawn for you, and never kept, signed in", () => {
  const PUBLIC = ["/plugins", "/plugins/all?category=git", "/plugins/all", "/plugins/focus-timer", "/plugins/session-badges", "/plugins/focus-timer/versions", "/plugins/publishing"];
  /** The bar's own part: who it says you are, and what it offers. */
  const bar = (body: string) => body.slice(body.indexOf('<header class="top"'), body.indexOf("</header>") + 9);

  test("signed out, a page is the same for everyone, sets no cookie, and a shared cache may keep it; the bar waits for the script", async () => {
    for (const path of PUBLIC) {
      // (Drawn once first: a README still being drawn for the first visitor is shown as its text to one who comes meanwhile.)
      await html(path);
      // (No cookie, or ones that aren't a session's: one page.)
      const asked = await Promise.all([undefined, "__Host-zeron_flash=report_sent", "theme=dark"].map((cookie) => site.fetch(path, cookie ? { headers: { cookie } } : {})));
      const bodies = await Promise.all(asked.map((r) => r.text()));
      expect(new Set(bodies).size, path).toBe(1);
      for (const r of asked) {
        expect(r.headers.get("cache-control"), path).toMatch(/^public, max-age=\d+$/);
        expect(r.headers.get("cloudflare-cdn-cache-control") ?? "", path).toMatch(/public/);
      }
      expect(bar(bodies[0]!), path).toContain('hidden data-me');
      expect(bar(bodies[0]!), path).not.toMatch(/@ada|@grace/);
    }
    expect((await site.fetch("/plugins")).headers.get("set-cookie")).toBeNull();
  });

  test("signed in, the page comes with you in the bar (Review for staff), and is yours alone: private, never kept", async () => {
    for (const [user, handle, staff] of [["user_ada", "@ada", false], ["user_grace", "@grace", true]] as const) {
      const cookie = await site.signIn(user);
      for (const path of PUBLIC) {
        const response = await site.fetch(path, { headers: { cookie } });
        const body = await response.text();
        expect(response.status, path).toBe(200);
        expect(response.headers.get("cache-control"), `${path} as ${handle}`).toBe("private, no-store");
        expect(response.headers.get("cloudflare-cdn-cache-control") ?? "", path).not.toMatch(/public/);
        // (Drawn: the name in the bar, no Sign in, the menu's Sign out with the session's token; Review only for staff.)
        const top = bar(body);
        expect(top, `${path} as ${handle}`).toContain(`>${handle}<`);
        expect(top, path).not.toContain("hidden data-me");
        expect(top, path).not.toContain(">Sign in<");
        expect(body, path).toMatch(/<form method="post" action="\/auth\/web\/logout" data-signout>\s*<input type="hidden" name="csrf" value="[^"]+"/);
        expect(top.includes('href="/admin/plugins"'), `${path}: Review for ${handle}`).toBe(staff);
      }
    }
  });

  test("a session cookie that's no one's: the signed-out bar, drawn, and still never kept", async () => {
    const response = await site.fetch("/plugins", { headers: { cookie: "__Host-zeron_session=nobody-has-this" } });
    const body = await response.text();
    expect(response.headers.get("cache-control")).toBe("private, no-store");
    expect(bar(body)).toContain(">Sign in<");
    expect(bar(body)).not.toContain("data-me");
  });
});

describe("signed-in pages", () => {
  test("every staff page's crumbs start at Review, the store's word for the queue's page", async () => {
    const grace = await site.signIn("user_grace");
    for (const path of ["/admin/plugins/focus-timer", "/admin/plugins/focus-timer/1.2.0", "/admin/plugins/focus-timer/actions/suspend", "/admin/plugins/session-badges/1.2.1/decline"]) {
      const { body } = await html(path, grace);
      const crumbs = /<nav class="crumbs" aria-label="Where this is">([\s\S]*?)<\/nav>/.exec(body)?.[1] ?? "";
      expect(crumbs, path).toMatch(/^\s*<a href="\/admin\/plugins">Review<\/a>/);
      expect(crumbs, path).not.toContain("Queue");
    }
  });

  test("signed out, Your plugins, one of them, its questions and Deploy tokens say so and offer sign-in, here; never a redirect", async () => {
    for (const path of ["/account", "/account/tokens", "/account/tokens/new", "/account/plugins/focus-timer", "/account/plugins/focus-timer/versions/1.2.0/yank"]) {
      const { response, body } = await html(path);
      expect(response.status, path).toBe(200);
      expect(response.headers.get("cache-control"), path).toBe("private, no-store");
      expect(body, path).toContain(`<a class="btn pri" href="/auth/web/login?return_to=${encodeURIComponent(path)}">Sign in</a>`);
      expect(body, path).toMatch(/<h2>Sign in to see (the plugins you publish|your deploy tokens)<\/h2>/);
    }
  });

  test("signed out, the pages that act send you to sign in and back (a path here, never anywhere else)", async () => {
    for (const path of ["/plugins/focus-timer/report"]) {
      const response = await site.fetch(path);
      expect(response.status, path).toBe(303);
      expect(response.headers.get("location"), path).toBe(`/auth/web/login?return_to=${encodeURIComponent(path)}`);
    }
  });

  test("yours: never cached, never indexed", async () => {
    const cookie = await site.signIn("user_ada");
    const { response, body } = await html("/account", cookie);
    expect(response.status).toBe(200);
    expect(response.headers.get("cache-control")).toBe("private, no-store");
    expect(response.headers.get("x-robots-tag")).toBe("noindex, nofollow");
    expect(response.headers.get("cloudflare-cdn-cache-control") ?? "").not.toMatch(/public/);
    expect(body).toContain("Session badges");
    expect(hasInlineScript(body)).toBe(false);
  });

  test("a session the edge rotates goes back to the browser, and that page is kept by no one", async () => {
    // (The edge rotates a staff session as it's used.)
    const { response } = await html("/admin/plugins", await site.signIn("user_grace"));
    expect(response.headers.getSetCookie().join("\n")).toMatch(/__Host-zeron_session=[\w-]+; Path=\/; Secure; HttpOnly; SameSite=Lax/);
    expect(response.headers.get("cache-control")).toBe("private, no-store");
    expect(response.headers.get("cloudflare-cdn-cache-control") ?? "").not.toMatch(/public/);
  });

  test("where each plugin's newest version stands: verified, not yet, the reviewer's note, taken down and why, removed", async () => {
    const { body } = await html("/account", await site.signIn("user_ada"));
    const row = (id: string) => body.slice(body.indexOf(`<div class="id">${id}</div>`), body.indexOf("</li>", body.indexOf(`<div class="id">${id}</div>`)));
    expect(row("linear-issues")).toMatch(/<span class="state ok">.*?Verified<\/span>/);
    expect(row("linear-issues")).toMatch(/<small>Verified on [A-Z][a-z]{2} \d{1,2}<\/small>/);
    expect(row("first-light")).toContain('<span class="state no">Not verified yet</span>');
    expect(row("first-light")).toContain("Nothing verified yet: agents can&#39;t install it until one is.");
    // (Not verified: the reviewer's note, in the app's words, to its author only.)
    expect(row("rough-draft")).toContain('<span class="state bad">Not verified</span>');
    expect(row("rough-draft")).toContain('<small class="why">The reviewer&#39;s note: “The README is one line.');
    expect(row("clipboard-sync")).toContain('<span class="state bad">Taken down</span>');
    expect(row("clipboard-sync")).toMatch(/“It sent every copied password to its sync host in plain text\.” Off since [A-Z][a-z]{2} \d{1,2}\./);
    expect(row("old-sketch")).toContain('<span class="state bad">Removed</span>');
    // (One whose newest isn't verified says which is, and since when.)
    expect(row("session-badges")).toMatch(/1\.1\.1 is the newest verified, since [A-Z][a-z]{2} \d{1,2}/);
    // Where it was published from: a deploy key by its name, or one of your devices.
    expect(body).toContain("One of your devices");
  });

  test("an id with nothing that can be installed says it goes back after 90 days", async () => {
    const ada = await site.signIn("user_ada");
    expect((await html("/account/plugins/clipboard-sync", ada)).body).toContain("Its id stays yours for 90 days after your last version");
    // (A version declined is listed still: its id is in use.)
    expect((await html("/account/plugins/rough-draft", ada)).body).not.toContain("90 days");
  });

  test("someone else's plugin is the 404 page", async () => {
    const { response } = await html("/account/plugins/focus-timer", await site.signIn("user_kai"));
    expect(response.status).toBe(404);
  });
});

describe("who is signed in, for a public page's list (account/me.json)", () => {
  test("always 200, never kept; signed out, a member, staff", async () => {
    const answers: [string | undefined, Record<string, unknown>][] = [
      [undefined, { signed_in: false }],
      [await site.signIn("user_ada"), { signed_in: true, name: "@ada", staff: false, csrf: expect.any(String) }],
      [await site.signIn("user_grace"), { signed_in: true, name: "@grace", staff: true, csrf: expect.any(String) }],
    ];
    for (const [cookie, expected] of answers) {
      const response = await site.fetch("/account/me.json", cookie ? { headers: { cookie } } : {});
      expect(response.status).toBe(200);
      expect(response.headers.get("cache-control")).toBe("private, no-store");
      expect(await response.json()).toEqual(expected);
    }
  });
});

describe("the admin pages can't be told from pages that don't exist", () => {
  const PATHS = ["/admin/reservations", "/admin/reservations/linear", "/admin/plugins", "/admin/plugins/session-badges", "/admin/plugins/session-badges/1.2.1", "/admin/plugins/session-badges/1.2.1/verify", "/admin/plugins/session-badges/actions/suspend", "/admin/reports", "/admin/audit"];

  test("signed out, or signed in without the role: the site's 404, byte for byte", async () => {
    const notFound = readFileSync(new URL("../dist/client/404.html", import.meta.url), "utf8");
    const unknown = await site.fetch("/admin/nothing-here");
    const ada = await site.signIn("user_ada");
    for (const path of PATHS) {
      for (const cookie of [undefined, ada]) {
        const { response, body } = await html(path, cookie);
        expect(response.status, `${path} ${cookie ? "ada" : "anyone"}`).toBe(404);
        expect(body, path).toBe(notFound);
        // Every header, not only the body: nothing says "this one exists".
        const headersOf = (r: Response) => JSON.stringify([...r.headers].filter(([k]) => k !== "date" && k !== "cf-cache-status"));
        expect(headersOf(response), path).toBe(headersOf(unknown));
        expect(response.headers.get("location"), path).toBeNull();
      }
    }
  });

  test("an admin who opens an address no staff page has sees the site's 404 page, not an empty one", async () => {
    const grace = await site.signIn("user_grace");
    for (const path of ["/admin/nothing-here", "/admin/plugins/no-such-plugin", "/admin/nothing-here"]) {
      const response = await site.fetch(path, { headers: { cookie: grace } });
      expect(response.status, path).toBe(404);
      expect(await response.text(), path).toBe(readFileSync(new URL("../dist/client/404.html", import.meta.url), "utf8"));
      // (An admin's answer, theirs alone: kept by no cache.)
      expect(response.headers.get("cache-control"), path).toBe("private, no-store");
      expect(response.headers.get("cloudflare-cdn-cache-control") ?? "", path).not.toMatch(/public/);
    }
    // A fresh admin session whose first use is such an address: its answer kept by no one, and any
    // session the edge rotated meanwhile goes back on it (secure.test.ts holds the cookie's way through).
    const fresh = await site.signIn("user_grace");
    const first = await site.fetch("/admin/nothing-here", { headers: { cookie: fresh } });
    expect(first.status).toBe(404);
    expect(first.headers.get("cache-control")).toBe("private, no-store");
    for (const cookie of first.headers.getSetCookie()) expect(cookie).toMatch(/^__Host-zeron_session=[\w-]+; Path=\/; Secure; HttpOnly; SameSite=Lax/);
  });

  test("an admin sees them, never cached, never indexed", async () => {
    const grace = await site.signIn("user_grace");
    for (const path of PATHS) {
      const { response } = await html(path, grace);
      expect(response.status, path).toBe(200);
      expect(response.headers.get("cache-control"), path).toBe("private, no-store");
      expect(response.headers.get("x-robots-tag"), path).toBe("noindex, nofollow");
    }
  });

  /** A response's every header but the clock's. */
  const headersOf = (r: Response) => JSON.stringify([...r.headers].filter(([k]) => k !== "date" && k !== "cf-cache-status"));
  /** Pages that are there, beside addresses that aren't (an action no page has among them). */
  const PAIRS: [string, string][] = [
    ["/admin/reservations", "/admin/nothing-here"],
    ["/admin/plugins/session-badges/1.2.1/verify", "/admin/plugins/session-badges/1.2.1/bogus"],
    ["/admin/plugins/session-badges/actions/suspend", "/admin/plugins/session-badges/actions/bogus"],
    ["/admin/reports/1", "/admin/reports/1/more"],
  ];

  test("a form from another site: the same 404 whether a page is there or not", async () => {
    const notFound = readFileSync(new URL("../dist/client/404.html", import.meta.url), "utf8");
    for (const pair of PAIRS) {
      const answers = await Promise.all(pair.map((path) => post(path, "", { csrf: "x" }, "https://evil.example")));
      for (const [i, response] of answers.entries()) {
        expect(response.status, pair[i]).toBe(404);
        expect(await response.text(), pair[i]).toBe(notFound);
      }
      expect(headersOf(answers[0]!), pair[0]).toBe(headersOf(answers[1]!));
    }
  });

  test("a form posted with a session that ended: the same trip to sign in, whether a page is there or not", async () => {
    for (const pair of PAIRS) {
      const answers = await Promise.all(pair.map((path) => post(path, "__Host-zeron_session=ended", { csrf: "x", reason: "Looks wrong" })));
      for (const [i, response] of answers.entries()) {
        expect(response.status, pair[i]).toBe(303);
        // (What was typed comes back; the stale token doesn't.)
        expect(response.headers.get("location"), pair[i]).toBe(`/auth/web/login?${new URLSearchParams({ return_to: `${pair[i]}?reason=Looks+wrong` })}`);
      }
      expect(headersOf(answers[0]!).replace(/return_to=[^"]*/, ""), pair[0]).toBe(headersOf(answers[1]!).replace(/return_to=[^"]*/, ""));
    }
  });

  test("a doubled slash: the 404 whether a page is there or not; an admin is sent on", async () => {
    const unknown = await site.fetch("/admin/nothing-here");
    for (const path of ["/admin//plugins", "/admin//nothing-here", "//admin/audit", "/admin"]) {
      const response = await site.fetch(path);
      expect(response.status, path).toBe(404);
      expect(headersOf(response), path).toBe(headersOf(unknown));
    }
    const grace = await site.signIn("user_grace");
    const sent = await site.fetch("/admin//plugins", { headers: { cookie: grace } });
    expect(sent.status).toBe(301);
    expect(sent.headers.get("location")).toBe("/admin/plugins");
  });

  test("an admin: a form from another site is refused; an address no page has is the 404", async () => {
    const grace = await site.signIn("user_grace");
    expect((await post("/admin/reservations", grace, { csrf: "x", id: "a-b" }, "https://evil.example")).status).toBe(403);
    expect((await site.fetch("/admin/nothing-here", { headers: { cookie: grace } })).status).toBe(404);
  });

  test("a report closed after signing in again comes back to its row, the note as typed", async () => {
    const grace = await site.signIn("user_grace");
    const back = await site.fetch("/admin/reports/1?note=Checked+it", { headers: { cookie: grace } });
    expect(back.status).toBe(303);
    expect(back.headers.get("location")).toBe("/admin/reports?report=1&note=Checked+it");
  });

  test("a post from a non-admin does nothing and says nothing", async () => {
    const ada = await site.signIn("user_ada");
    const page = await html("/account", ada);
    const response = await post("/admin/plugins/voice-memos/0.1.0/verify", ada, { csrf: tokenOf(page.body) });
    expect(response.status).toBe(404);
    expect((await html("/plugins/voice-memos")).body).toContain(">Not verified yet<");
  });

  test("the public never reads a decline: a version a reviewer didn't verify is 'not verified yet' to anyone but its author and staff", async () => {
    const page = (await html("/plugins/rough-draft")).body;
    expect(page).toContain(">Not verified yet<");
    expect(page).not.toContain(">Not verified<");
    expect(page).not.toContain("The README is one line.");
    expect((await html("/account/plugins/rough-draft", await site.signIn("user_ada"))).body).toContain(">Not verified<");
  });

  test("the app's link can name a version: 'Install 1.0.0 instead' beside an unverified newest, and a version's own page installs that version", async () => {
    const page = (await html("/plugins/worst-case-listing-at-every-limit-xyz12")).body;
    // (The newest by the plain link.)
    expect(page).toContain('href="zeron://open/plugin/worst-case-listing-at-every-limit-xyz12" data-install>');
    // (The amber note: the app's sentences, its link the one that names the verified version.)
    expect(page).toContain('<b>1.5.0 isn&#39;t verified yet. 1.0.0 is.</b> <a href="zeron://open/plugin/worst-case-listing-at-every-limit-xyz12?version=1.0.0">Install 1.0.0 instead</a>, verified');
    const older = (await html("/plugins/worst-case-listing-at-every-limit-xyz12/versions/1.0.0")).body;
    expect(older).toContain('href="zeron://open/plugin/worst-case-listing-at-every-limit-xyz12?version=1.0.0" data-install>');
    expect(older).toMatch(/data-install>[^]*?Install 1\.0\.0\s*<\/a>/);
    // (Its newest's page: the plain link.)
    expect((await html("/plugins/worst-case-listing-at-every-limit-xyz12/versions/1.5.0")).body).toContain('href="zeron://open/plugin/worst-case-listing-at-every-limit-xyz12" data-install>');
    // On Versions, each listed version's Install names it; one that takes no new installs can't be.
    const versions = (await html("/plugins/worst-case-listing-at-every-limit-xyz12/versions")).body;
    expect(versions).toContain('href="zeron://open/plugin/worst-case-listing-at-every-limit-xyz12?version=1.0.0">Install 1.0.0</a>');
    expect(versions.match(/<span class="off">Can't be installed<\/span>/g)).toHaveLength(2);
  });

  test("a version's page says only what the store's record names: a crafted address puts no words in its mouth", async () => {
    const crafted = "1.0.0-verified.by.zeron.call.us";
    const path = `/plugins/focus-timer/versions/${crafted}`;
    // (Its words: the page's text and title, not the addresses it links to, which are this one.)
    const words = (body: string) => body.replace(/="[^"]*"/g, "");
    try {
      // (An edge that answers with another version's record: the version isn't in the store.)
      await site.fake("loose", { on: "1" });
      const other = await html(path);
      expect(other.response.status).toBe(404);
      expect(words(other.body)).not.toContain("call.us");
      await site.fake("loose");
      // (A store that doesn't answer: its page, naming no version.)
      await site.fake("down", { on: "1" });
      const down = await html(path);
      expect(down.response.status).toBe(503);
      expect(words(down.body)).not.toContain("call.us");
    } finally {
      await site.fake("loose");
      await site.fake("down");
    }
    expect((await html(path)).response.status).toBe(404);
  });

  test("a version with a build, \"+\" and all, is found by its address: its page and its files", async () => {
    const built = "1.2.0+build.7";
    try {
      await site.fake("rename", { id: "focus-timer", from: "1.2.0", to: built });
      const cookie = await site.signIn("user_kai");
      const page = await html(`/plugins/focus-timer/versions/${encodeURIComponent(built)}`, cookie);
      expect(page.response.status).toBe(200);
      expect(page.body).toContain(built);
      const icon = await site.fetch(`/plugins/focus-timer/files/${encodeURIComponent(built)}/icon.svg`);
      expect(icon.status).toBe(200);
      await icon.arrayBuffer();
    } finally {
      await site.fake("rename", { id: "focus-timer", from: built, to: "1.2.0" });
    }
  });

  test("a version the link can't name is never named on its button: Open in Zeron, and the app chooses", async () => {
    // (33 characters and a "+": the app's link carries 32 at most; the "+" also comes through the address as %2B.)
    const long = "1.0.0-rc.1+build.20261118.0123456";
    try {
      await site.fake("rename", { id: "worst-case-listing-at-every-limit-xyz12", from: "1.0.0", to: long });
      const cookie = await site.signIn("user_kai");
      const page = (await html("/plugins/worst-case-listing-at-every-limit-xyz12", cookie)).body;
      expect(page).not.toContain(`Install ${long} instead`);
      expect(page).not.toContain("?version=");
      // (Its own page: no version is that long, so no address names one: the site's 404.)
      expect((await html(`/plugins/worst-case-listing-at-every-limit-xyz12/versions/${encodeURIComponent(long)}`, cookie)).response.status).toBe(404);
      const versions = (await html("/plugins/worst-case-listing-at-every-limit-xyz12/versions", cookie)).body;
      expect(versions).toContain(`href="zeron://open/plugin/worst-case-listing-at-every-limit-xyz12">Open in Zeron<span class="sr-only"> for ${long}</span></a>`);
    } finally {
      await site.fake("rename", { id: "worst-case-listing-at-every-limit-xyz12", from: long, to: "1.0.0" });
    }
  });
});

describe("forms", () => {
  test("a post from another site is refused before anything runs", async () => {
    const ada = await site.signIn("user_ada");
    const page = await html("/account/plugins/session-badges", ada);
    const response = await post("/account/plugins/session-badges/versions/1.2.1/yank", ada, { csrf: tokenOf(page.body) }, "https://evil.example");
    expect(response.status).toBe(403);
    expect(stateOf((await html("/account/plugins/session-badges", ada)).body, "1.2.1")).toBe("Listed");
  });

  test("a post without the session's token changes nothing", async () => {
    const ada = await site.signIn("user_ada");
    const response = await post("/account/plugins/session-badges/versions/1.2.1/yank", ada, { csrf: "not-the-token" });
    expect(response.status).toBe(303);
    const after = await html("/account/plugins/session-badges", `${ada}; ${flashOf(response)}`);
    expect(after.body).toContain("The page was open too long. Try again.");
    expect(stateOf(after.body, "1.2.1")).toBe("Listed");
  });

  test("a confirm page names the plugin in the app's words; someone else's plugin is the 404", async () => {
    const ada = await site.signIn("user_ada");
    const ask = await html("/account/plugins/focus-timer/versions/1.2.0/yank", ada);
    expect(ask.response.status).toBe(200);
    expect(ask.body).toContain("Stop new installs of Focus timer 1.2.0?");
    expect(ask.body).toContain("Those who have it keep it working.");
    const kai = await site.signIn("user_kai");
    expect((await html("/account/plugins/focus-timer/versions/1.2.0/yank", kai)).response.status).toBe(404);
  });

  test("an author with nothing sent is told how to send the first", async () => {
    const { body } = await html("/account", await site.signIn("user_noor"));
    expect(body).toContain("<h2>Nothing published yet</h2>");
    expect(body).toContain('data-copy="zeron plugin publish"');
    expect(body).toContain('<a href="/account" aria-current="page">Your plugins<span class="num">0</span></a>');
  });

  test("one plugin: where each version stands and whether it's verified; a version's one action in its column", async () => {
    const { body } = await html("/account/plugins/worst-case-listing-at-every-limit-xyz12", await site.signIn("user_ada"));
    const states = Object.fromEntries(["1.5.0", "1.4.0", "1.3.0", "1.2.0", "1.1.0", "1.0.0"].map((v) => [v, stateOf(body, v)]));
    // (Amendment 45: 1.4.0, declined, was superseded by 1.5.0; 1.2.0 taken down, 1.3.0 removed, 1.1.0 yanked by its author.)
    expect(states).toEqual({ "1.5.0": "Listed", "1.4.0": "No new installs", "1.3.0": "Removed", "1.2.0": "Taken down", "1.1.0": "No new installs", "1.0.0": "Listed" });
    expect(verifiedOf(body, "1.5.0")).toBe("Not verified yet");
    expect(verifiedOf(body, "1.4.0")).toBe("Not verified");
    expect(verifiedOf(body, "1.0.0")).toBe("Verified");
    // A superseded version says why it takes no new installs, and can't be allowed again by its author.
    expect(body).toContain("Replaced by 1.5.0 before it was verified. Those who have it keep it working; verifying it would allow installs again.");
    const actions = [...body.matchAll(/data-dialog="(yank|unyank)-([\d_]+)"/g)].map((m) => `${m[1]} ${m[2]}`);
    expect(actions).toEqual(["yank 1_5_0", "unyank 1_1_0", "yank 1_0_0"]);
    // A reviewer's words in full, never cut; staff's reason for a take-down beside it.
    expect(body).toContain("Until then this one stays listed, not verified.”");
    expect(body).toContain("Taken down: it sent the open chat&#39;s text to a host its README doesn&#39;t name.");
    // Said once: taken down already says it was turned off on the devices that had it.
    expect(body.match(/turned off on the devices that had it/g)).toHaveLength(1);
    // Whether it's verified always first, beside the version; where it stands after it; one kind of button for each action.
    for (const r of body.split('<li class="vr').slice(1)) if (r.includes('data-kind="status"') && r.includes('data-kind="verified"')) expect(r.indexOf('data-kind="verified"')).toBeLessThan(r.indexOf('data-kind="status"'));
    expect([...body.matchAll(/<a class="([^"]+)" href="\/account\/plugins\/[^"]+\/(?:yank|unyank)"/g)].map((m) => m[1])).toEqual(["btn sec sm", "btn sec sm", "btn sec sm"]);
    // Its tokens are the account's: a way to them.
    expect(body).toContain('<a href="/account/tokens">deploy tokens</a>');
  });

  test("in public, a version that takes no new installs says why: replaced before it was verified, or stopped by its author", async () => {
    const superseded = await html("/plugins/worst-case-listing-at-every-limit-xyz12/versions/1.4.0");
    expect(superseded.body).toContain("Replaced by 1.5.0 before it was verified. Those who have it keep it working.");
    // (It can't be installed, and says so in place of Install.)
    expect(superseded.body).toContain('<span class="btn pri lg" aria-disabled="true">Can\'t be installed</span>');
    expect(superseded.body).not.toContain("data-install");
    // (Never the reviewer's reason: that is its author's to read.)
    expect(superseded.body).not.toContain("Until then this one stays listed");
    expect((await html("/plugins/worst-case-listing-at-every-limit-xyz12/versions/1.1.0")).body).toContain("Its author stopped new installs of it.");
  });

  test("the account's deploy tokens: each with what it can publish, never cut; its short id; the revoked and ended apart", async () => {
    const { body } = await html("/account/tokens", await site.signIn("user_ada"));
    // Twelve live, each with Revoke; five revoked or ended, in their group.
    expect(body.match(/<a class="btn ghost sm" href="\/account\/tokens\/tok_[0-9A-Za-z]{16}">Revoke/g)).toHaveLength(12);
    // (Three named in full; all twenty of Ada's by name, counted, every one on request.)
    expect(body).toMatch(/<div class="names">[^<,]+, [^<,]+, [^<,]+<\/div><small>3 plugins<\/small>/);
    expect(body).toContain('<div class="count">20 plugins</div>');
    expect(body).toContain('<span class="tile s more">+17</span>');
    expect(body).toMatch(/<details class="gone"><summary>Revoked and ended tokens · 5/);
    expect(body).toMatch(/<span class="state quiet">Ended [A-Z][a-z]{2} \d{1,2}<\/span>/);
    // Each by its short id, the store's: never more of a token, which this page never has.
    const ids = [...body.matchAll(/<div class="id">([^<]*)<\/div>/g)].map((m) => m[1]);
    expect(ids).toHaveLength(17);
    for (const id of ids) expect(id).toMatch(/^[0-9A-Za-z]{8}$/);
    expect(body).toContain('<div class="id">xJVb81Wo</div>');
    expect(body).not.toMatch(/zdt_[0-9A-Za-z]/);
    // All of yours; one to three named in full; past three, three tiles, "+n", and every one on request.
    expect(body).toMatch(/All your plugins<\/div><small>\d+ today, and any you publish later<\/small>/);
    // (A name with no spaces breaks inside itself, evenly; every other word only at its spaces. "on" stays with the plugin it names.)
    const long = "Averyveryverylongpluginnamewithoutanyspacesatallthatkeepsgoing64";
    expect(body).toContain(`<div class="names">Focus\u00a0timer and <span class="lw">${long}</span></div><small>2 plugins</small>`);
    // (A short name is kept whole: names break between one another.)
    expect(body).toContain('<div class="names">Agent\u00a0relay, Quick\u00a0links, Theme\u00a0sync</div>');
    expect(body).toContain(`<small>on\u00a0<span class="lw">${long}</span></small>`);
    expect(body).toContain("<small>on\u00a0Focus timer</small>");
    // (Past three: the count, and "Show all" under it in the same column; the list in the same scope.)
    const many = body.slice(body.indexOf("GitHub Actions, the monorepo"), body.indexOf("</ul>", body.indexOf('<ul class="plist"')));
    expect(many).toContain('<div class="scope many">');
    expect(many).toContain('<span class="tile s more">+11</span>');
    expect(many).toMatch(/<div class="count">14 plugins<\/div><details class="every"><summary>Show all 14/);
    const list = body.slice(body.indexOf('<ul class="plist"'), body.indexOf("</ul>", body.indexOf('<ul class="plist"')));
    expect(list.match(/<li class="pl">/g)).toHaveLength(14);
    expect(body).not.toMatch(/text-overflow|…<\/div><small>\d+ plugins?/);
    // Ending within 14 days: the warning's pill, in days.
    expect(body).toMatch(/<span class="state no">Ends in \d+ days?<\/span>/);
    // New token: a link to its page, which the script opens in place as the dialog holding the same form.
    expect(body).toContain('<a class="btn pri" href="/account/tokens/new" data-dialog="new-token">');
    expect(body).toMatch(/<dialog class="question" id="new-token" aria-labelledby="new-token-title" closedby="any">\s*<h2 id="new-token-title">New deploy token<\/h2>\s*<form method="post" action="\/account\/tokens\/new" class="tokform">/);
  });

  test("Deploy keys' old addresses lead to Deploy tokens for good", async () => {
    for (const path of ["/account/keys", "/account/keys/k5Yh3xQ2b8VtYwGm1rPz0nLc4dEa6sJu9oKi7fBg2Wq"]) {
      const response = await site.fetch(path);
      expect(response.status, path).toBe(301);
      expect(response.headers.get("location"), path).toBe("/account/tokens");
    }
  });

  test("a token for fifty plugins, the most a token may name: three tiles, +47, and all fifty, by name, on request", async () => {
    try {
      await site.fake("many", { on: "1" });
      const { body } = await html("/account/tokens", await site.signIn("user_ada"));
      const every = body.slice(body.indexOf(">Every one<"), body.indexOf("</ul>", body.indexOf(">Every one<")));
      expect(every).toContain('<div class="count">50 plugins</div>');
      expect(every).toContain('<span class="tile s more">+47</span>');
      expect(every.match(/<li class="pl">/g)).toHaveLength(50);
      // (Every one a plugin of hers, by its name: never an id in its place.)
      expect(every).not.toMatch(/<li class="pl">.*?<span>[a-z0-9-]+<\/span><\/li>/);
    } finally {
      await site.fake("many");
    }
    expect((await html("/account/tokens", await site.signIn("user_ada"))).body).toContain('<div class="count">20 plugins</div>');
  });

  test("revoking a token asks first, then the token stops", async () => {
    try {
      const ada = await site.signIn("user_ada");
      const id = "tok_08ctRRy5a3tUlC6k";
      const ask = await html(`/account/tokens/${id}`, ada);
      expect(ask.body).toContain("Revoke GitHub Actions?");
      const response = await post(`/account/tokens/${id}`, ada, { csrf: tokenOf(ask.body) });
      expect(response.headers.get("location")).toBe("/account/tokens");
      expect(flashOf(response)).toBe("__Host-zeron_flash=key_revoked");
      expect((await html(`/account/tokens/${id}`, ada)).response.status).toBe(404);
      // (Not a token's id: the site's 404, never asked of the edge.)
      expect((await html("/account/tokens/k5Yh3xQ2b8VtYwGm1rPz0nLc4dEa6sJu9oKi7fBg2Wq", ada)).response.status).toBe(404);
    } finally {
      await site.fake("tokens");
    }
  });

  describe("making a deploy token", () => {
    /** The token a page shows, every place it's in it. */
    const tokensIn = (text: string) => [...text.matchAll(/zdt_[0-9A-Za-z]+/g)].map((m) => m[0]);
    /** Signed in this moment (a fresh sign-in: its ticket unspent), or as a session whose ticket is spent or old. */
    const signedIn = async (user: string, fresh = true) => {
      if (fresh) return site.signIn(user);
      const response = await site.fetch(`/auth/web/callback?as=${user}&fresh=0&return_to=/`);
      return response.headers.get("set-cookie")!.split(";")[0]!;
    };
    const make = async (cookie: string, fields: Record<string, string> | [string, string][]) => {
      const form = await html("/account/tokens/new", cookie);
      const body = new URLSearchParams(fields);
      body.set("csrf", tokenOf(form.body));
      const response = await site.fetch("/account/tokens/new", { method: "POST", headers: { cookie, origin: "https://zeron.sh", "content-type": "application/x-www-form-urlencoded" }, body: body.toString() });
      return { response, body: response.status === 303 ? "" : await response.text() };
    };

    test("the form, then the token from the edge's answer, once: in the page's words, never an address, a field or anything kept", async () => {
      try {
        const ada = await signedIn("user_ada");
        const form = await html("/account/tokens/new", ada);
        expect(form.response.status).toBe(200);
        expect(form.response.headers.get("cache-control")).toBe("private, no-store");
        // Name, Ends, Can publish; fresh from signing in, nothing to say about signing in again.
        expect(form.body).toContain('<input id="page-token-name" class="input" type="text" name="name" value placeholder="GitHub Actions" maxlength="64" required autocomplete="off" spellcheck="false">');
        expect([...form.body.matchAll(/<option value="([^"]+)"( selected)?>([^<]+)<\/option>/g)].map((m) => `${m[1]}${m[2] ?? ""} ${m[3]}`)).toEqual(["30 In 30 days", "90 selected In 90 days", "365 In a year", "never Never"]);
        expect(form.body).toContain('<input type="radio" name="scope" value="all" checked>');
        expect(form.body.match(/<input type="checkbox" name="plugins" value="[a-z0-9-]+">/g)).toHaveLength(20);
        expect(form.body).not.toContain("Zeron will ask you to sign in again first.");

        const made = await make(ada, [["name", "Release CI"], ["ends", "30"], ["scope", "chosen"], ["plugins", "focus-timer"], ["plugins", "theme-sync"]]);
        expect(made.response.status).toBe(200);
        expect(made.response.headers.get("cache-control")).toBe("private, no-store");
        expect(made.response.headers.get("referrer-policy")).toBe("no-referrer");
        expect(made.response.headers.getSetCookie().join("\n")).not.toMatch(/zdt_/);
        expect(made.body).toContain("<h1 id=\"made-title\"");
        expect(made.body).toContain("Copy your token now");
        expect(made.body).toContain("Put it in your CI&#39;s secrets as ZERON_DEPLOY_TOKEN.");
        // Once, in the code block, as the store makes one: never in an attribute, a field, or a script's data.
        const [token, ...more] = tokensIn(made.body);
        expect(more).toEqual([]);
        expect(token).toMatch(/^zdt_[0-9A-Za-z]{49}$/);
        const { readToken } = await import("../fake-edge/token.ts");
        expect(readToken(token!).ok).toBe(true);
        expect(made.body).toContain(`<code id="the-token" translate="no">${token}</code>`);
        expect(made.body).toContain('<button type="button" data-copy-of="the-token" aria-label="Copy the token">');
        expect(made.body).not.toMatch(/="[^"]*zdt_/);
        expect(made.body).not.toMatch(/<input[^>]*zdt_|<textarea/);
        // What it is: its name and short id (the token's first characters), what it publishes, when it ends; then done.
        const short = token!.slice(4, 12);
        expect(made.body).toContain(`<dd>Release CI <span class="id">${short}</span></dd>`);
        expect(made.body).toContain("<dd>Focus timer, Theme sync</dd>");
        expect(made.body).toContain('<a class="btn pri" href="/account/tokens" data-secret-done>I&#39;ve copied it</a>');

        // The list has it by name and short id, and never the token.
        const list = await html("/account/tokens", ada);
        expect(list.body).toContain(`<div class="id">${short}</div>`);
        expect(list.body).toContain(">Release CI<");
        expect(tokensIn(list.body)).toEqual([]);
        expect(list.body.match(/<a class="btn ghost sm" href="\/account\/tokens\/tok_/g)).toHaveLength(13);

        // The sign-in's ticket is spent: another token signs in again first, and comes back to the form as it was, with no token anywhere in the address.
        const again = await make(ada, [["name", "Second"], ["ends", "never"], ["scope", "all"]]);
        expect(again.response.status).toBe(303);
        const location = again.response.headers.get("location")!;
        expect(location).toBe(`/auth/web/login?${new URLSearchParams({ return_to: "/account/tokens/new?again=1&name=Second&ends=never&scope=all" })}`);
        expect(tokensIn(decodeURIComponent(location))).toEqual([]);
        expect((await html("/account/tokens", ada)).body.match(/<a class="btn ghost sm" href="\/account\/tokens\/tok_/g)).toHaveLength(13);
      } finally {
        await site.fake("tokens");
      }
    });

    test("without a fresh sign-in: the form says so, and the post signs in again first, then comes back to the form filled in", async () => {
      try {
        const stale = await signedIn("user_ada", false);
        const form = await html("/account/tokens/new", stale);
        expect(form.body).toContain('<p class="note">Zeron will ask you to sign in again first.</p>');
        const list = await html("/account/tokens", stale);
        expect(list.body).toContain('<p class="note">Zeron will ask you to sign in again first.</p>');
        const sent = await make(stale, [["name", "Nightly"], ["ends", "365"], ["scope", "chosen"], ["plugins", "focus-timer"], ["plugins", "weather"]]);
        expect(sent.response.status).toBe(303);
        const back = "/account/tokens/new?again=1&name=Nightly&ends=365&scope=chosen&plugins=focus-timer%2Cweather";
        expect(sent.response.headers.get("location")).toBe(`/auth/web/login?${new URLSearchParams({ return_to: back })}`);
        // (Nothing was made.)
        expect((await html("/account/tokens", stale)).body.match(/<a class="btn ghost sm" href="\/account\/tokens\/tok_/g)).toHaveLength(12);
        // Signed in again, the form as it was, and a word that it's ready.
        const fresh = await signedIn("user_ada");
        const filled = await html(back, fresh);
        expect(filled.body).toContain('name="name" value="Nightly"');
        expect(filled.body).toContain('<option value="365" selected>');
        expect(filled.body).toContain('<input type="radio" name="scope" value="chosen" checked>');
        expect(filled.body).toContain('<input type="checkbox" name="plugins" value="focus-timer" checked>');
        expect(filled.body).toContain('<input type="checkbox" name="plugins" value="weather" checked>');
        expect(filled.body.match(/name="plugins" value="[a-z0-9-]+" checked/g)).toHaveLength(2);
        expect(filled.body).toContain('<p class="note ok">You signed in again. Create the token to finish.</p>');
      } finally {
        await site.fake("tokens");
      }
    });

    test("the store's refusals, each said beside what it's about, the form kept as it was", async () => {
      try {
        // (The form's own: a name, and a plugin when choosing; never asked of the store.)
        const noName = await make(await signedIn("user_ada"), [["name", " "], ["ends", "90"], ["scope", "all"]]);
        expect(noName.response.status).toBe(400);
        expect(noName.body).toContain('aria-invalid="true" aria-describedby="page-token-said"');
        expect(noName.body).toContain('<p class="said" id="page-token-said">Name the token in 1 to 64 characters.</p>');
        const noneChosen = await make(await signedIn("user_ada"), [["name", "CI"], ["ends", "90"], ["scope", "chosen"]]);
        expect(noneChosen.response.status).toBe(400);
        expect(noneChosen.body).toContain("Choose at least one plugin it can publish.");
        // (Someone else's plugin: not_owner, by name.)
        const theirs = await make(await signedIn("user_ada"), [["name", "CI"], ["ends", "90"], ["scope", "chosen"], ["plugins", "pr-radar"]]);
        expect(theirs.response.status).toBe(403);
        expect(theirs.body).toContain("You don&#39;t own pr-radar any more. Choose again.");
        expect(theirs.body).toContain('name="name" value="CI"');
        // Twenty live, the most: the next is refused, and says what to do.
        for (let i = 0; i < 8; i++) expect((await make(await signedIn("user_ada"), [["name", `CI ${i}`], ["ends", "30"], ["scope", "all"]])).response.status).toBe(200);
        const full = await make(await signedIn("user_ada"), [["name", "One more"], ["ends", "30"], ["scope", "all"]]);
        expect(full.response.status).toBe(409);
        expect(full.body).toContain('<p class="said" role="alert">You have 20 deploy tokens, the most an account can have. Revoke one to make another.</p>');
        expect(tokensIn(full.body)).toEqual([]);
        // Ten a day: the eleventh waits for tomorrow.
        for (let i = 0; i < 10; i++) expect((await make(await signedIn("user_noor"), [["name", `N ${i}`], ["ends", "30"], ["scope", "all"]])).response.status).toBe(200);
        const slow = await make(await signedIn("user_noor"), [["name", "N 10"], ["ends", "30"], ["scope", "all"]]);
        expect(slow.response.status).toBe(429);
        expect(slow.body).toContain("You&#39;ve made 10 deploy tokens today. You can make more tomorrow.");
      } finally {
        await site.fake("tokens");
      }
    });

    test("a post from elsewhere, or without the page's token, makes nothing", async () => {
      const ada = await signedIn("user_ada");
      const fields = { name: "Forged", ends: "30", scope: "all" };
      const forged = await site.fetch("/account/tokens/new", { method: "POST", headers: { cookie: ada, origin: "https://evil.example", "content-type": "application/x-www-form-urlencoded" }, body: new URLSearchParams({ ...fields, csrf: tokenOf((await html("/account/tokens/new", ada)).body) }).toString() });
      expect(forged.status).toBe(403);
      const noCsrf = await post("/account/tokens/new", ada, fields);
      expect(noCsrf.status).toBe(303);
      expect(flashOf(noCsrf)).toBe("__Host-zeron_flash=expired");
      expect((await html("/account/tokens", ada)).body.match(/<a class="btn ghost sm" href="\/account\/tokens\/tok_/g)).toHaveLength(12);
    });
  });

  test("a confirm page asks first; the post does it once and says so once", async () => {
    const ada = await site.signIn("user_ada");
    const ask = await html("/account/plugins/session-badges/versions/1.2.1/yank", ada);
    expect(ask.response.status).toBe(200);
    expect(ask.body).toContain("Stop new installs of Session badges 1.2.1?");
    const response = await post("/account/plugins/session-badges/versions/1.2.1/yank", ada, { csrf: tokenOf(ask.body) });
    expect(response.status).toBe(303);
    expect(response.headers.get("location")).toBe("/account/plugins/session-badges");
    expect(response.headers.getSetCookie().find((c) => c.startsWith("__Host-zeron_flash="))).toMatch(/^__Host-zeron_flash=yanked; .*Secure/);
    const cookie = `${ada}; ${flashOf(response)}`;
    const after = await html("/account/plugins/session-badges", cookie);
    expect(after.body).toContain("New installs stopped.");
    expect(stateOf(after.body, "1.2.1")).toBe("No new installs");
    // (Withdraw is gone with amendment 45: nothing waits to be listed.)
    expect((await html("/account/plugins/session-badges/versions/1.2.1/withdraw", ada)).response.status).toBe(404);
    // (Said once: the page that shows it clears it.)
    expect(after.response.headers.getSetCookie().join("\n")).toMatch(/__Host-zeron_flash=[^;]*;.*(Max-Age=0|Expires=Thu, 01 Jan 1970)/);
  });

  test("a first version is listed at once, not verified; an admin verifies it and the public page says so", async () => {
    const grace = await site.signIn("user_grace");
    expect((await html("/plugins/voice-memos")).body).toContain("<b>No version of this plugin is verified yet.</b>");
    const page = await html("/admin/plugins/voice-memos/0.1.0", grace);
    const response = await post("/admin/plugins/voice-memos/0.1.0/verify", grace, { csrf: tokenOf(page.body) });
    expect(response.status).toBe(303);
    expect(response.headers.get("set-cookie")).toMatch(/zeron_flash=verified/);
    const after = (await html("/plugins/voice-memos", await site.signIn("user_kai"))).body;
    expect(after).not.toContain("No version of this plugin is verified yet.");
    expect(after).toContain(">Verified<");
    expect((await html("/admin/audit", grace)).body).toMatch(/Verified\s*<a href="\/admin\/plugins\/voice-memos\/0\.1\.0"/);
  });

  test("an admin whose session ended keeps what they typed through signing in again", async () => {
    const expired = "__Host-zeron_session=ended-long-ago";
    const fields = { csrf: "stale", reason: "listing", message: "Say what <it> does, & why." };
    const response = await post("/admin/plugins/focus-timer/1.3.0-beta.1/decline", expired, fields);
    expect(response.status).toBe(303);
    const location = new URL(response.headers.get("location")!, "https://zeron.sh");
    expect(location.pathname).toBe("/auth/web/login");
    const back = location.searchParams.get("return_to")!;
    expect(back).toBe("/admin/plugins/focus-timer/1.3.0-beta.1/decline?reason=listing&message=Say+what+%3Cit%3E+does%2C+%26+why.");
    // Signed in again, the question comes back with the answer as typed (and nothing was done meanwhile).
    const page = await html(back, await site.signIn("user_grace"));
    expect(page.body).toContain(">Say what &lt;it&gt; does, &amp; why.</textarea>");
    expect(page.body).toMatch(/<option value="listing" selected/);
    expect((await html("/admin/plugins/focus-timer/1.3.0-beta.1", await site.signIn("user_grace"))).body).toContain("Live, not read yet");
  });

  test("a non-admin's post, signed in or not, is the 404 page and nothing more", async () => {
    const notFound = readFileSync(new URL("../dist/client/404.html", import.meta.url), "utf8");
    for (const cookie of [await site.signIn("user_ada"), "theme=dark"]) {
      for (const path of ["/admin/plugins/focus-timer/1.3.0-beta.1/decline", "/admin/reports/rep_1", "/admin/reservations"]) {
        const response = await post(path, cookie, { csrf: "x", reason: "listing", message: "m", resolution: "dismissed", id: "abc" });
        expect(response.status, path).toBe(404);
        expect(response.headers.get("location"), path).toBeNull();
        expect(await response.text(), path).toBe(notFound);
      }
    }
  });

  test("what an author wrote shows as words on every admin view: tools, notes, file names, contents, diffs", async () => {
    const grace = await site.signIn("user_grace");
    const here = "/admin/plugins/xss-probe/1.1.0";
    const overview = await html(here, grace);
    inert(overview.body);
    expect(overview.body).toContain("&lt;/p&gt;&lt;script&gt;alert(&#39;tool&#39;)&lt;/script&gt;&lt;img src=x onerror=alert(&#39;tool&#39;)&gt;");
    expect(overview.body).toContain("What the author says changed");
    const files = await html(`${here}?view=files&path=${encodeURIComponent("docs/<b>notes</b> #1 100%.md")}`, grace);
    inert(files.body);
    expect(files.body).toContain("&lt;b&gt;notes&lt;/b&gt; #1 100%.md");
    // (The file's text, highlighted: its words are there, as text.)
    expect(files.body).toContain("alert('file')");
    const changes = await html(`${here}?view=changes`, grace);
    inert(changes.body);
    expect(changes.body).toMatch(/&lt;\/code&gt;&lt;\/pre&gt;&lt;script&gt;alert\(('|&#39;)diff('|&#39;)\)/);
    const readme = await html(`${here}?view=readme`, grace);
    inert(readme.body);
    // The public page's notes too.
    inert((await html("/plugins/xss-probe")).body);
  });

  test("no file can hide from the reviewer: each is shown as itself, or said to be missing, never another in its place", async () => {
    const grace = await site.signIn("user_grace");
    const here = "/admin/plugins/xss-probe/1.1.0";
    const view = (path: string) => html(`${here}?view=files&path=${encodeURIComponent(path)}`, grace);
    const long = await view(`src/${"x".repeat(600)}.lua`);
    expect(long.response.status).toBe(200);
    expect(long.body).toContain("the long one");
    const dotted = await view("lib/%2e%2e/x.lua");
    expect(dotted.response.status).toBe(200);
    expect(dotted.body).toContain("the dotted one");
    const missing = await view("no/such/file.lua");
    expect(missing.body).toContain("This version has no file at that path.");
    expect(missing.body).not.toMatch(/"entry"|&quot;entry&quot;/);
    // An SVG is shown as a picture and as its source: what it draws, and what runs.
    const svg = await view("shots/a b#1 100%.svg");
    expect(svg.body).toContain(`src="${fileUrl("xss-probe", "1.1.0", "shots/a b#1 100%.svg")}"`);
    expect(svg.body).toMatch(/rect/);
    expect(svg.body).toContain("9277b0");
    // Every file of the version, changed or not, can be compared.
    expect((await html(`${here}?view=changes`, grace)).response.status).toBe(200);
  });

  test("a decision's page says what is decided, and says so when the visit came from elsewhere", async () => {
    const grace = await site.signIn("user_grace");
    const plain = await html("/admin/plugins/session-badges/1.2.1/verify", grace);
    expect(plain.body).toContain("Session badges");
    expect(plain.body).toContain("its newest verified version");
    expect(plain.body).toContain("Read it first");
    expect(plain.body).not.toContain("You followed a link from somewhere else");
    const lured = await site.fetch("/admin/plugins/session-badges/1.2.1/verify", { headers: { cookie: grace, "sec-fetch-site": "cross-site" } });
    expect(await lured.text()).toContain("You followed a link from somewhere else");
  });

  test("reserving an id for an account, and freeing it", async () => {
    const grace = await site.signIn("user_grace");
    const page = await html("/admin/reservations", grace);
    expect(page.body).toContain("zeron-tools");
    const csrf = tokenOf(page.body);
    const reserved = await post("/admin/reservations", grace, { csrf, id: "Kai-Tools", to_username: "@kai", note: "Asked by email." });
    expect(flashOf(reserved)).toBe("__Host-zeron_flash=reserved");
    expect((await html("/admin/reservations", grace)).body).toMatch(/<code[^>]*>kai-tools<\/code>[^]*?@kai[^]*?Asked by email\./);
    // An id in use moves by a transfer, and the page says so.
    expect(flashOf(await post("/admin/reservations", grace, { csrf, id: "focus-timer" }))).toBe("__Host-zeron_flash=id_in_use");
    expect(flashOf(await post("/admin/reservations", grace, { csrf, id: "x" }))).toBe("__Host-zeron_flash=bad_input");
    // The site's own page words: nobody can be granted one, and the page says why, beside the form too.
    for (const fields of [{ id: "all" }, { id: "publishing", to_username: "@kai" }] as Record<string, string>[]) {
      expect(flashOf(await post("/admin/reservations", grace, { csrf, ...fields }))).toBe("__Host-zeron_flash=site_id");
    }
    const after = await html("/admin/reservations", grace);
    expect(after.body).not.toMatch(/<code[^>]*>(all|publishing)<\/code>/);
    expect(after.body).toContain("The site's own page words can't be reserved: no plugin can have them.");
    expect(flashOf(await post("/admin/reservations/kai-tools", grace, { csrf }))).toBe("__Host-zeron_flash=reservation_released");
    expect((await html("/admin/reservations", grace)).body).not.toContain("kai-tools");
    expect((await html("/admin/audit", grace)).body).toMatch(/Freed\s*<a href="\/admin\/reservations"/);
  });

  test("not verifying needs a reason and words for the author; the version stays listed", async () => {
    const grace = await site.signIn("user_grace");
    const page = await html("/admin/plugins/focus-timer/1.3.0-beta.1", grace);
    const csrf = tokenOf(page.body);
    const missing = await post("/admin/plugins/focus-timer/1.3.0-beta.1/decline", grace, { csrf, reason: "listing" });
    expect(missing.headers.get("set-cookie")).toMatch(/zeron_flash=bad_input/);
    const done = await post("/admin/plugins/focus-timer/1.3.0-beta.1/decline", grace, { csrf, reason: "listing", message: "Say what <b>it</b> does." });
    expect(done.headers.get("set-cookie")).toMatch(/zeron_flash=declined/);
    expect((await site.fetch("/plugins/focus-timer/files/1.3.0-beta.1/icon.svg")).status).toBe(200);
    const author = await html("/account/plugins/focus-timer", await site.signIn("user_ada"));
    expect(author.body).toContain("Say what &lt;b&gt;it&lt;/b&gt; does.");
  });

  test("a report needs a known reason, reaches the reviewers, and thanks you", async () => {
    const kai = await site.signIn("user_kai");
    const page = await html("/plugins/theme-sync/report", kai);
    expect(page.response.headers.get("cache-control")).toBe("private, no-store");
    const csrf = tokenOf(page.body);
    expect((await post("/plugins/theme-sync/report", kai, { csrf, reason: "bogus", text: "x" })).headers.get("set-cookie")).toMatch(/bad_input/);
    const sent = await post("/plugins/theme-sync/report", kai, { csrf, reason: "broken", text: "It <script>breaks</script> the sidebar." });
    expect((await html("/plugins/theme-sync/report", `${kai}; ${flashOf(sent)}`)).body).toContain("Thanks for telling us");
    const reports = await html("/admin/reports", await site.signIn("user_grace"));
    expect(reports.body).toContain("It &lt;script&gt;breaks&lt;/script&gt; the sidebar.");
  });

  test("signing out without the token is refused in the store's word, and the session lives", async () => {
    const lin = await site.signIn("user_lin");
    const refused = await post("/auth/web/logout", lin, {});
    expect(refused.status).toBe(403);
    const said = (await refused.json()) as Record<string, unknown>;
    expect(said).toMatchObject({ error: "bad_csrf", message: expect.any(String) });
    expect(said).not.toHaveProperty("reason");
    expect((await site.fetch("/account", { headers: { cookie: lin } })).status).toBe(200);
  });

  test("signing out is a form post with the token; the session ends", async () => {
    const lin = await site.signIn("user_lin");
    const page = await html("/account", lin);
    expect(page.body).toContain("Claim a username first:");
    const response = await post("/auth/web/logout", lin, { csrf: tokenOf(page.body) });
    expect(response.status).toBe(303);
    // (As the edge does: always the front page, absolute.)
    expect(response.headers.get("location")).toBe("https://zeron.sh/");
    // (Signed out now: the page offers sign-in again.)
    expect((await html("/account", lin)).body).toContain("<h2>Sign in to see the plugins you publish</h2>");
  });
});

describe("Discover", () => {
  test("the store in three numbers, from /stats; every category with its line, its count and a few of its plugins", async () => {
    const { body } = await html("/plugins");
    expect(body).toMatch(/<dt>Plugins<\/dt><dd class="num">\d+<\/dd>/);
    expect(body).toMatch(/<dt>Verified<\/dt><dd class="num">\d+ <svg class="seal s"/);
    expect(body).toMatch(/<dt>New this week<\/dt><dd class="num">\d+<\/dd>/);
    const index = body.slice(body.indexOf('<ul class="index">'), body.indexOf("</ul>", body.indexOf('<ul class="index">')));
    expect(index.match(/<a class="idx" href="\/plugins\/all\?category=/g)).toHaveLength(8);
    expect(index).toContain("<h3>Developer tools</h3><p>Docs, databases, secrets and builds</p>");
    // (Its top plugins, at most three tiles, each on its own hue.)
    for (const tiles of index.matchAll(/<div class="tiles" aria-hidden="true">(.*?)<\/div>/gs)) expect(tiles[1]!.match(/class="tile s h\d+"/g)!.length).toBeLessThanOrEqual(3);
  });

  test("Featured: each on its own colour, its first picture, or its tile alone, large, when it ships none", async () => {
    const withPictures = (await html("/plugins")).body;
    const featured = withPictures.slice(withPictures.indexOf('<h2 id="featured">'), withPictures.indexOf("</section>", withPictures.indexOf('<h2 id="featured">')));
    expect(featured.match(/<a class="card f-(big|small) h\d+"/g)).toHaveLength(3);
    expect(featured.match(/<img class="shot"/g)).toHaveLength(3);
    try {
      await site.fake("bare", { id: "pr-radar" });
      // (A minute's cache: another address of the same page, a fresh one.)
      const { body } = await html("/plugins", await site.signIn("user_kai"));
      const bare = body.slice(body.indexOf('<h2 id="featured">'), body.indexOf("</section>", body.indexOf('<h2 id="featured">')));
      expect(bare.match(/<img class="shot"/g)).toHaveLength(2);
      expect(bare).toMatch(/<span class="alone"><span class="tile xl h\d+"/);
    } finally {
      await site.fake("bare");
    }
  });

  test("its lists: See all, each a real link to the list it summarises; Just published only what isn't verified yet", async () => {
    const { body } = await html("/plugins");
    expect([...body.matchAll(/<a class="more" href="([^"]+)">See all/g)].map((m) => m[1])).toEqual(["/plugins/all", "/plugins/all?sort=verified", "/plugins/all?sort=new"]);
    const fresh = body.slice(body.indexOf('<ul class="fresh">'), body.indexOf("</ul>", body.indexOf('<ul class="fresh">')));
    expect(fresh.match(/<li>/g)!.length).toBeGreaterThan(0);
    expect(fresh.match(/<li>/g)!.length).toBe(fresh.match(/<span class="state no">Not verified yet<\/span>/g)!.length);
    expect(fresh).not.toContain('class="seal');
  });

  test("an empty store says so; a store that didn't answer says so, and isn't kept", async () => {
    try {
      await site.fake("empty", { on: "1" });
      const { body } = await html("/plugins", await site.signIn("user_mo"));
      expect(body).toContain("<h2>No plugins yet</h2>");
      expect(body).not.toContain('<h2 id="featured">');
    } finally {
      await site.fake("empty");
    }
    try {
      await site.fake("down", { on: "1" });
      const { response, body } = await html("/plugins", await site.signIn("user_lin"));
      expect(response.status).toBe(503);
      expect(body).toContain("The plugin store didn&#39;t answer");
    } finally {
      await site.fake("down");
    }
  });
});

describe("the staff's side: the queue, a submission, the decision", () => {
  /** The rows of a table, each its cells' text. */

  /** The queue's rows (each a version), as the look draws them. */
  const queueRows = (body: string) => body.slice(body.indexOf('<div class="card q">'), body.indexOf("</main>")).split('<div class="qr">').slice(1);

  test("the queue: reported first, then the longest waiting, its count on its tab; each row what it asks for against its newest verified version", async () => {
    const grace = await site.signIn("user_grace");
    const { body } = await html("/admin/plugins", grace);
    const rows = queueRows(body);
    const total = Number(body.match(/aria-current="page">Waiting<span class="num">(\d+)<\/span>/)![1]);
    expect(rows).toHaveLength(total);
    // (Amendment 45: plugins with open reports first, then the oldest.)
    const reported = rows.map((r) => /<span class="state bad">\d+ reports?<\/span>/.test(r));
    expect(reported.some(Boolean)).toBe(true);
    expect(reported).toEqual([...reported].sort((a, b) => Number(b) - Number(a)));
    for (const part of [rows.filter((_, i) => reported[i]), rows.filter((_, i) => !reported[i])]) {
      const sent = part.map((r) => Date.parse(r.match(/<time datetime="([^"]+)"/)![1]!));
      expect(sent).toEqual([...sent].sort((a, b) => a - b));
    }
    // An update that asks for more: each new ask marked "+", a far-reaching one in the danger's ink.
    const worst = rows.find((r) => r.includes("/admin/plugins/worst-case-listing-at-every-limit-xyz12/"))!;
    expect(worst).toContain("<span>Asks for more than 1.1.0</span>");
    expect(worst).toMatch(/<li class="add risky">[^<]+<span class="sr-only"> \(new\)<\/span><\/li>/);
    expect(worst).toMatch(/<li class="add">[^<]+<span class="sr-only"> \(new\)<\/span><\/li>/);
    // Nothing new, but a host swapped inside an ask it had.
    const xss = rows.find((r) => r.includes("/admin/plugins/xss-probe/"))!;
    expect(xss).toContain("<span>What it asks for changed inside since 1.0.0</span>");
    // A first version: everything it asks for, none of it marked new against nothing.
    const first = rows.find((r) => r.includes("/admin/plugins/first-light/"))!;
    expect(first).toContain("<span>Nothing verified yet</span>");
    expect(first).toMatch(/<ul aria-label="What it asks for"><li>[^<]+<\/li>/);
    expect(first).not.toContain('class="add');
    // Each says how many have the plugin, beside who sent it.
    for (const r of rows) expect(r).toMatch(/<div class="by">by [^<]+ · [\d,]+ installs?/);
    // Every row opens its version, its link naming the version too.
    for (const r of rows) expect(r).toMatch(/<a href="\/admin\/plugins\/[a-z0-9-]+\/[^"]+">.*?<span class="sr-only"> [^<]+<\/span><\/a>/);
    // (A name's controls that turn text around are dropped: "Probe \u202Eevil" reads as written, never around its version.)
    expect(body).not.toContain("\u202E");
    // The staff pages sit beside the lists as tabs: Reports with how many are open.
    expect(body).toMatch(/<a href="\/admin\/reports">Reports<span class="num">\d+<\/span><\/a>/);
    expect(body).toContain('<a href="/admin/audit">Log</a>');
  });

  test("Taken down: versions taken down and not restored, the latest first, a tab of its own", async () => {
    const { body } = await html("/admin/plugins?state=taken_down", await site.signIn("user_grace"));
    expect(body).toMatch(/<a href="\/admin\/plugins\?state=taken_down" aria-current="page">Taken down<\/a>/);
    const rows = queueRows(body);
    expect(rows.length).toBeGreaterThan(0);
    expect(rows.some((r) => r.includes("/admin/plugins/worst-case-listing-at-every-limit-xyz12/1.2.0"))).toBe(true);
  });

  test("an empty queue says so; forty fit one page, with no pager", async () => {
    const grace = await site.signIn("user_grace");
    try {
      await site.queueAs("empty");
      const empty = await html("/admin/plugins", grace);
      expect(empty.body).toContain("Nothing waiting");
      expect(empty.body).toMatch(/aria-current="page">Waiting<span class="num">0<\/span>/);
      await site.queueAs(40);
      const forty = await html("/admin/plugins", grace);
      expect(queueRows(forty.body)).toHaveLength(40);
      expect(forty.body).not.toContain('class="pager"');
    } finally {
      await site.queueAs(null);
    }
  });

  test("a version to verify: what it asks for against the newest verified version, what moved first, the risky in the danger's ink", async () => {
    const { body } = await html("/admin/plugins/worst-case-listing-at-every-limit-xyz12/1.5.0", await site.signIn("user_grace"));
    expect(body).toContain("Compared with 1.1.0, the newest verified version: 2 new asks, 2 asks changed inside, its MCP server, its service and a setting that grants changed.");
    const rows = body.slice(body.indexOf('<ul class="asklist">'), body.indexOf('id="said"')).split('<li class="ar').slice(1);
    const titles = rows.map((r) => r.match(/<div class="t">([^<]+)/)![1]!.trim());
    expect(titles.slice(0, 4)).toEqual(["Send you notifications", "Connect to these places", "Run python3, git, jq, bash on your computer as you", "Send messages to your agents as you"]);
    expect(rows[0]).toMatch(/^ new"/);
    expect(rows[0]).toContain('<span class="state no">New in 1.5.0</span>');
    expect(rows[2]).toMatch(/^ risky new"/);
    expect(rows[2]).toContain('<span class="state no">Changed</span>');
    expect(rows[2]).toMatch(/<li class="add"><code>bash<\/code><span class="sr-only"> \(new\)<\/span><\/li>/);
    expect(rows[1]).toMatch(/<li class="del"><code>\*/);
    // Then what it binds past its asks (amendment 41), risky, its MCP server and service whole, before and after.
    expect(titles.slice(4, 7)).toEqual(["Its MCP server changed", "Its service changed", "A setting that grants access changed"]);
    expect(rows.slice(4, 7).every((r) => r.startsWith(" risky new"))).toBe(true);
    expect(rows[4]).toMatch(/<dt>Before<\/dt><dd><code>node server\.js<\/code><code>API_URL=https:\/\/api\.[^<]+<\/code><\/dd>/);
    expect(rows[4]).toContain("<code>node server.js --listen 0.0.0.0:7777</code>");
    expect(rows[5]).toContain("<dd><code>services/sync.luau</code></dd>");
    // Then what is as before, quieter.
    expect(rows.slice(7).every((r) => / same"/.test(r.slice(0, 20)) && r.includes('<span class="same">Same as 1.1.0</span>'))).toBe(true);
  });

  test("the decision: Mark verified, the one primary, asks once with what there is to weigh; Don't verify asks for a reason and words, showing the author's view; Take down and Remove ask why; a verified version's are only those two", async () => {
    const grace = await site.signIn("user_grace");
    const railOf = (body: string) => body.slice(body.indexOf('<aside class="rail rv-rail"'), body.indexOf("</aside>", body.indexOf('<aside class="rail rv-rail"')));
    const pending = (await html("/admin/plugins/worst-case-listing-at-every-limit-xyz12/1.5.0", grace)).body;
    const rail = railOf(pending);
    expect(rail.match(/btn pri/g)).toHaveLength(1);
    expect(rail).toMatch(/<a class="btn pri" href="\/admin\/plugins\/worst-case-listing-at-every-limit-xyz12\/1\.5\.0\/verify" data-dialog="approve">.*?Mark 1\.5\.0 verified<\/a>/s);
    const ask = pending.slice(pending.indexOf('<dialog class="question sm" id="approve"'), pending.indexOf("</dialog>", pending.indexOf('id="approve"')));
    expect(ask).toContain("Everyone installing it sees it&#39;s verified");
    expect(ask).toContain('<p class="weigh-line bad">2 new asks · 2 asks changed · Its MCP server, its service and a setting that grants changed · 1 check to look at</p>');
    // (Verify has the focus: a clean update is a click, then Enter.)
    expect(ask).toContain('<button class="btn pri" type="submit" autofocus>Verify</button>');
    // The author: publishing since, and their plugins; never how many of theirs were or weren't verified (Wing's ruling).
    expect(rail).toMatch(/<dt>Publishing since<\/dt><dd>[A-Z][a-z]{2} \d{1,2}, \d{4}<\/dd>/);
    expect(rail).toMatch(/<dt>Plugins<\/dt><dd>\d+(, (all verified|verified|\d+ with a verified version))?<\/dd>/);
    expect(rail).not.toMatch(/\d+ verified ·|not verified<\/dd>/);
    expect(rail).toContain('data-dialog="reject">Don\'t verify, with a note…</a>');
    // (Listed already: a harmful or misplaced one is taken down or removed whether or not it's decided.)
    expect(rail).toContain('data-dialog="take-down">Take this version down…</a>');
    expect(rail).toContain('data-dialog="remove">Remove it…</a>');
    expect(pending).toContain('<select id="reject-reason" name="reason" required>');
    expect(pending).toContain('<option value="" disabled selected>Choose a reason</option>');
    expect(pending).toContain("What @ada reads");
    // (The manifest as signed: the first of its files.)
    expect((await html("/admin/plugins/worst-case-listing-at-every-limit-xyz12/1.5.0?view=files", grace)).body).toContain('<p class="file-head"><code>zeron-plugin.json</code>');
    const verified = (await html("/admin/plugins/focus-timer/1.2.0", grace)).body;
    const verifiedRail = railOf(verified);
    expect(verifiedRail).toContain('data-dialog="take-down">Take this version down…</a>');
    expect(verifiedRail).toContain('data-dialog="remove">Remove it…</a>');
    expect(verifiedRail).not.toContain("/verify");
    expect(verifiedRail).not.toContain("/decline");
    // Without the script, the same question as a page, the author's view on it too.
    const page = await html("/admin/plugins/worst-case-listing-at-every-limit-xyz12/1.5.0/decline", grace);
    expect(page.body).toContain("Don&#39;t verify Averyveryverylongpluginnamewithoutanyspacesatallthatkeepsgoing64 1.5.0?");
    expect(page.body).toContain("What @ada reads");
  });

  test("a version taken down is restored by staff, and says so", async () => {
    const grace = await site.signIn("user_grace");
    const before = (await html("/admin/plugins/worst-case-listing-at-every-limit-xyz12/1.2.0", grace)).body;
    const rail = before.slice(before.indexOf('<aside class="rail rv-rail"'), before.indexOf("</aside>", before.indexOf('<aside class="rail rv-rail"')));
    expect(rail).toContain("/1.2.0/restore");
    expect(rail).not.toContain("/verify");
    const response = await post("/admin/plugins/worst-case-listing-at-every-limit-xyz12/1.2.0/restore", grace, { csrf: tokenOf(before), reason: "Its author fixed the host." });
    expect(response.headers.get("location")).toBe("/admin/plugins/worst-case-listing-at-every-limit-xyz12");
    expect(response.headers.get("set-cookie")).toMatch(/zeron_flash=version_restored/);
  });
});

describe("the review of the web store: what it found, held", () => {
  const notFound = () => readFileSync(new URL("../dist/client/404.html", import.meta.url), "utf8");
  const headersOf = (r: Response) => JSON.stringify([...r.headers].filter(([k]) => k !== "date" && k !== "cf-cache-status"));
  const timed = async (path: string, cookie?: string) => {
    const started = performance.now();
    const page = await html(path, cookie);
    return { ...page, ms: performance.now() - started };
  };

  test("words built to be slow to draw, at the store's caps, can't hold a plugin's page: it answers within the bound; what would write too much is its text, said so", async () => {
    const first = await timed("/plugins/slow-words");
    expect(first.response.status).toBe(200);
    expect(first.ms).toBeLessThan(4_000);
    // (The README, a new list on every line, would keep more than its bound; the notes are drawn.)
    expect(first.body).toContain(`<p class="as-text">Shown as plain text: it's too long to lay out.</p>`);
    // (Its versions' notes are drawn on its Versions, within the bound too.)
    const notes = await timed("/plugins/slow-words/versions");
    expect(notes.response.status).toBe(200);
    expect(notes.ms).toBeLessThan(4_000);
    expect(notes.body).toContain("<blockquote>");
    // (Kept, so the next visit draws nothing again.)
    const again = await timed("/plugins/slow-words");
    expect(again.ms).toBeLessThan(4_000);
  });

  test("…nor hide a submission from its reviewer: every view answers, and says, loudly, what couldn't be drawn", async () => {
    const grace = await site.signIn("user_grace");
    for (const view of ["", "?view=files", "?view=changes"]) {
      const { response, body, ms } = await timed(`/admin/plugins/slow-words/1.2.0${view}`, grace);
      expect(response.status, view).toBe(200);
      expect(ms, view).toBeLessThan(4_000);
      // (Its README past what it may keep; its notes, a table filling 64 cells a row, past what it may write.)
      expect(body, view).toMatch(/<li class="fail">.*?This README is too long to lay out: it(?:'|&#39;)s shown as plain text\. That alone is a reason not to verify it\./s);
      expect(body, view).toMatch(/<li class="fail">.*?Its notes for 1\.2\.0 are too long to lay out/s);
    }
    // (Verify's line counts them among what failed.)
    expect((await html("/admin/plugins/slow-words/1.2.0", grace)).body).toMatch(/<p class="weigh-line bad">[^<]*2 checks failed/);
  });

  test("a file built to break the highlighter is shown as text, and the reviewer told, in Files and in Changes", async () => {
    const grace = await site.signIn("user_grace");
    const file = await html(`/admin/plugins/slow-words/1.2.0?view=files&path=${encodeURIComponent("src/nest.js")}`, grace);
    expect(file.response.status).toBe(200);
    expect(file.body).toContain("This file couldn't be highlighted.");
    const changes = await html("/admin/plugins/slow-words/1.2.0?view=changes", grace);
    expect(changes.response.status).toBe(200);
    expect(changes.body).toContain("Some lines couldn't be highlighted.");
  });

  test("a staff address written another way is the same 404 as one that isn't there; an admin is sent to its one form", async () => {
    const unknown = await site.fetch("/admin/nothing-here");
    const ada = await site.signIn("user_ada");
    for (const path of ["/%61dmin/plugins", "/%2561dmin/plugins", "/admin%2Fplugins", "/%61dmin/nothing-here", "/%2561dmin/audit", "/%61dmin/plugins/slow-words/1.2.0"]) {
      for (const cookie of [undefined, ada]) {
        const response = await site.fetch(path, cookie ? { headers: { cookie } } : {});
        expect(response.status, path).toBe(404);
        expect(await response.text(), path).toBe(notFound());
        expect(headersOf(response), path).toBe(headersOf(unknown));
      }
    }
    const grace = await site.signIn("user_grace");
    const sent = await site.fetch("/%2561dmin/plugins", { headers: { cookie: grace } });
    expect(sent.status).toBe(301);
    expect(sent.headers.get("location")).toBe("/admin/plugins");
  });

  test("a form's body past what any form sends is never parsed: 413, or the staff's 404 to anyone but an admin", async () => {
    const big = "note=" + "x".repeat(2 * 1024 * 1024);
    const headers = (cookie: string) => ({ cookie, origin: "https://zeron.sh", "content-type": "application/x-www-form-urlencoded", "content-length": String(big.length) });
    // (The site answers before it takes the body, as it should; the test's client then sometimes can't finish
    // sending and reports EPIPE in place of the answer. That alone is tried again: every answer read is checked.)
    const send = async (path: string, cookie: string): Promise<Response> => {
      for (let tries = 1; ; tries++) {
        try {
          return await site.fetch(path, { method: "POST", headers: headers(cookie), body: big });
        } catch (error) {
          const code = (error as { cause?: { code?: string } }).cause?.code;
          if (tries >= 5 || (code !== "EPIPE" && code !== "ECONNRESET")) throw error;
        }
      }
    };
    const kai = await site.signIn("user_kai");
    expect((await send("/plugins/theme-sync/report", kai)).status).toBe(413);
    const grace = await site.signIn("user_grace");
    expect((await send("/admin/reports/1", grace)).status).toBe(413);
    // To anyone else, under /admin: the same answer whether a page is there or not, the body unread.
    const ada = await site.signIn("user_ada");
    for (const path of ["/admin/reports/1", "/admin/nothing-here"]) {
      const started = performance.now();
      const response = await send(path, ada);
      expect(response.status, path).toBe(404);
      expect(performance.now() - started, path).toBeLessThan(2_000);
    }
    // A session that ended: off to sign in, the body unread, nothing of it carried along.
    const ended = await send("/admin/reports/1", "__Host-zeron_session=x");
    expect(ended.status).toBe(303);
    expect(ended.headers.get("location")).toBe(`/auth/web/login?${new URLSearchParams({ return_to: "/admin/reports/1" })}`);
  });

  test("a picture the manifest names outside its package is never put on the review page", async () => {
    const { body } = await html("/admin/plugins/slow-words/1.2.0", await site.signIn("user_grace"));
    // (The raw manifest shows the path, as text; no picture is ever made of it.)
    expect(body).not.toMatch(/<img[^>]*me\.json/);
    expect(body).not.toMatch(/<img[^>]*src="[^"]*\.\./);
  });

  test("the built 404 goes out with the site's headers; its bytes as built", async () => {
    const response = await site.fetch("/nothing-here");
    expect(response.status).toBe(404);
    expect(await response.text()).toBe(notFound());
    expect(response.headers.get("content-security-policy")).toContain("default-src 'none'");
    expect(response.headers.get("x-frame-options")).toBe("DENY");
    expect(response.headers.get("x-content-type-options")).toBe("nosniff");
  });

  test("on the Worker's other names, the store's pages are the site's own origin's; the landing pages stay", async () => {
    // (The origin is the build's: zeron.sh, or the staging site's on a staging build.)
    const origin = BAKED.vars.SITE_ORIGIN as string;
    for (const host of ["https://comet.zeron.sh", `https://${BAKED.name}.someone.workers.dev`]) {
      for (const path of ["/plugins/all?sort=new", "/account", "/admin/plugins", "/auth/web/login"]) {
        const response = await site.fetch(`${host}${path}`);
        expect(response.status, `${host}${path}`).toBe(path.startsWith("/admin") ? 404 : 301);
        if (!path.startsWith("/admin")) expect(response.headers.get("location")).toBe(`${origin}${path}`);
      }
      expect((await site.fetch(`${host}/`)).status).toBe(200);
    }
  });
});
