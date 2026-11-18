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
const stateOf = (body: string, version: string) => new RegExp(`<span class="line version">${version.replace(/\./g, "\\.")}</span></td><td><span class="line"><span class="badge sm[^"]*">([^<]+)</span>`).exec(body)?.[1];
/** A version's second badge in its row: whether a reviewer verified it (amendment 45), when it's listed or yanked. */
const verifiedOf = (body: string, version: string) => new RegExp(`<span class="line version">${version.replace(/\./g, "\\.")}</span></td><td><span class="line"><span class="badge sm[^"]*">[^<]+</span><span class="badge sm[^"]*">([^<]+)</span>`).exec(body)?.[1];

/** The session's CSRF token, as a page carries it in its forms. */
const tokenOf = (body: string) => /name="csrf" value="([^"]+)"/.exec(body)?.[1] ?? "";

describe("the deploy config", () => {
  test("production is the zeron.sh worker reading the production edge", () => {
    const config = JSON.parse(readFileSync(new URL("../dist/server/wrangler.json", import.meta.url), "utf8"));
    expect(config.name).toBe("comet-landing");
    expect(config.routes.map((r: { pattern: string }) => r.pattern)).toEqual(["zeron.sh", "comet.zeron.sh"]);
    expect(config.services).toEqual([{ binding: "EDGE", service: "comet-native-edge" }]);
    expect(config.assets.binding).toBe("ASSETS");
    expect(config.assets.not_found_handling).toBeUndefined();
    expect(config.cache).toEqual({ enabled: true });
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
    expect(body).toContain('id="featured-heading"');
  });

});

describe("All plugins (/plugins/all)", () => {
  /** The plugins a list's page shows, in order. */
  const listed = (body: string) => [...body.matchAll(/class="name name-link" href="\/plugins\/([^"]+)"/g)].map((m) => m[1]);

  test("public, cached for a minute at most, a strict CSP; the columns, a row a plugin", async () => {
    const { response, body } = await html("/plugins/all");
    expect(response.status).toBe(200);
    expect(response.headers.get("cache-control")).toBe("public, max-age=30");
    expect(response.headers.get("cloudflare-cdn-cache-control")).toBe("public, max-age=30, stale-while-revalidate=30");
    expect(response.headers.get("content-security-policy")).toMatch(/script-src 'self';/);
    expect(hasInlineScript(body)).toBe(false);
    inert(body);
    expect(body).toMatch(/<th scope="col">Plugin<\/th><th scope="col">By<\/th><th scope="col">Category<\/th><th scope="col" class="end">Installs<\/th>/);
    expect(listed(body)).toHaveLength(24);
    expect(body).toContain('aria-current="page"><span class="glyph g16"');
  });

  test("Next and Previous are real addresses that carry the cursor, and come back to the same list", async () => {
    const first = await html("/plugins/all");
    const next = /href="([^"]+)" rel="next"/.exec(first.body)?.[1]?.replaceAll("&amp;", "&");
    expect(next).toMatch(/^\/plugins\/all\?cursor=/);
    expect(first.body).toMatch(/<span class="btn secondary sm" aria-disabled="true">.*Previous<\/span>/);
    const second = await html(next!);
    expect(second.response.status).toBe(200);
    expect(listed(second.body)).not.toEqual(listed(first.body));
    const previous = /href="([^"]+)" rel="prev"/.exec(second.body)?.[1]?.replaceAll("&amp;", "&");
    expect(previous).toMatch(/^\/plugins\/all/);
    expect(listed((await html(previous!)).body)).toEqual(listed(first.body));
  });

  test("the last column is what the list is sorted by, its head naming it", async () => {
    const head = (body: string) => /<th scope="col" class="end">([^<]+)<\/th>/.exec(body)?.[1];
    expect(head((await html("/plugins/all")).body)).toBe("Installs");
    expect(head((await html("/plugins/all?q=timer")).body)).toBe("Installs");
    const updated = await html("/plugins/all?sort=updated");
    expect(head(updated.body)).toBe("Updated");
    expect(updated.body).toMatch(/<td class="installs end">[A-Z][a-z]{2} \d{1,2}(, \d{4})?<\/td>/);
    expect(updated.body).toMatch(/<span class="meta">@[a-z-]+ · Updated [A-Z][a-z]{2} \d/);
    // Newest: the day each was first listed (amendment 39), the newest first.
    const newest = await html("/plugins/all?sort=new");
    expect(head(newest.body)).toBe("Published");
    expect(newest.body).toMatch(/<span class="meta">@[a-z-]+ · Published [A-Z][a-z]{2} \d/);
    const days = [...newest.body.matchAll(/<td class="installs end">([^<]+)<\/td>/g)].map((m) => Date.parse(`${m[1]}${/\d{4}/.test(m[1]!) ? "" : `, ${new Date().getUTCFullYear()}`}`));
    expect(days.length).toBeGreaterThan(1);
    expect(days).toEqual([...days].sort((a, b) => b - a));
  });

  test("the pager: Previous, where you are of how many, Next", async () => {
    const first = await html("/plugins/all");
    const total = Number(/<span class="title-count"> · ([\d,]+)<\/span>/.exec(first.body)![1]!.replace(",", ""));
    expect(first.body).toContain(`<span class="where">1–24 of ${total}</span>`);
    const next = /href="([^"]+)" rel="next"/.exec(first.body)![1]!.replaceAll("&amp;", "&");
    expect(next).toMatch(/&page=2$/);
    expect((await html(next)).body).toMatch(new RegExp(`<span class="where">25–\\d+ of ${total}</span>`));
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
    expect(timer.body).toContain("Results for “timer”");
    // The title and the count, always (amendment 38's total).
    expect(timer.body).toMatch(/Results for “timer”<span class="title-count"> · \d+<\/span><\/h1>/);
    // (The field keeps the words.)
    expect(timer.body).toContain('name="q" value="timer"');
    const empty = await html("/plugins/all?q=zzzzqqq");
    expect(empty.body).toContain("Nothing matches “zzzzqqq”");
    expect(empty.body).toContain('href="/plugins/all">Clear search</a>');
    expect(empty.body).toContain('<span class="title-count"> · 0</span>');
    const git = await html("/plugins/all?category=git");
    expect(listed(git.body)).toContain("pr-radar");
    expect(listed(git.body)).not.toContain("token-meter");
    expect(git.body).toContain('<link rel="canonical" href="https://zeron.sh/plugins/all?category=git">');
    expect(git.body).toMatch(/<a class="nav-row" href="\/plugins\/all\?category=git" aria-current="page">/);
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
    expect(body).toContain('href="zeron://open/plugin/pr-radar"');
    expect(body).toContain("Run gh, git on your computer as you");
    expect(body).toContain("Install it only if you trust @kai.");
    expect(body).toContain('<h3 id="user-content-what-it-does">What it does</h3>');
    expect(body).toContain('"@type":"SoftwareApplication"');
    expect(hasInlineScript(body)).toBe(false);
  });

  test("the page's rules: one primary button, pictures by their number, the README under the page, its parts as tabs", async () => {
    const { body } = await html("/plugins/focus-timer");
    // One action slot, one style: Install the page's one primary button; what else is said, in words.
    expect(body.match(/class="btn primary/g)).toHaveLength(1);
    expect(body).toContain("<summary>Nothing happened?</summary>");
    // Two pictures share the column.
    expect(body).toContain('class="strip two"');
    expect((await html("/plugins/pr-radar")).body).toMatch(/class="strip (one|two|many)"/);
    // The README doesn't say the name again, and nothing in it outweighs the title.
    expect(body.match(/<h[1-6][^>]*>Focus timer<\/h[1-6]>/g)).toEqual(['<h1 class="t-title">Focus timer</h1>']);
    expect(body).not.toMatch(/<div class="prose"[^>]*>[^]*?<h1/);
    // Its parts are tabs that are addresses; Report is always in the header.
    expect(body).toMatch(/<a href="#overview" aria-current="page">Overview<\/a><a href="#changelog">Changelog <span class="count">\d+<\/span><\/a><a href="#permissions">Permissions <span class="count">\d+<\/span><\/a>/);
    expect(body).toMatch(/<a slot="tools"|class="tools"><a class="btn ghost sm" href="\/plugins\/focus-timer\/report">/);
    // Older versions are rows that open in place; the latest's notes are there to clamp.
    expect(body).toContain("data-clamp");
    expect(body).toMatch(/<details><summary><span class="num">1\.1\.0<\/span>/);
    // A risky ask, in the rail and the tab, in the danger's ink.
    expect((await html("/plugins/pr-radar")).body).toMatch(/<li class="asks-line risky">/);
  });

  test("a hostile plugin's words are only words", async () => {
    const { body } = await html("/plugins/xss-probe");
    // (Tags only: an attribute's value may hold "<script>" as inert text.)
    const markup = body.replace(/="[^"]*"/g, '=""');
    const scripts = [...markup.matchAll(/<script\b[^>]*>/g)].map((m) => m[0]);
    expect(scripts).toEqual(['<script type="">', '<script type="" src="">', '<script type="" src="">', '<script type="" src="">']);
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
    expect(body).toMatch(/<h1[^>]*>[^<]*\u202eevil<\/h1>/);
  });

  test("an author's links are followed only as parsed https addresses", async () => {
    const { body } = await html("/plugins/focus-timer");
    expect(body).toContain('href="https://example.com/focus-timer" rel="nofollow noopener ugc"');
    expect(body).toContain('href="https://github.com/ada/focus-timer" rel="nofollow noopener ugc"');
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

describe("public pages are the same for everyone (so a shared cache can keep them)", () => {
  test("signed in or not, admin or not, the bytes are the same", async () => {
    const people = [undefined, await site.signIn("user_ada"), await site.signIn("user_grace")];
    for (const path of ["/plugins", "/plugins?category=git", "/plugins/focus-timer", "/plugins/session-badges", "/plugins/sitemap.xml"]) {
      // (Drawn once first: a README still being drawn for the first visitor is shown as its text to one who comes meanwhile.)
      await html(path);
      const bodies = await Promise.all(people.map(async (cookie) => (await html(path, cookie)).body));
      expect(new Set(bodies).size, path).toBe(1);
      const csp = await Promise.all(people.map(async (cookie) => (await site.fetch(path, cookie ? { headers: { cookie } } : {})).headers.get("set-cookie")));
      expect(csp.every((c) => c === null), `${path} sets no cookie`).toBe(true);
    }
  });
});

describe("signed-in pages", () => {
  test("signed out, they send you to sign in and back (a path here, never anywhere else)", async () => {
    for (const path of ["/account", "/account/plugins/focus-timer", "/plugins/focus-timer/report"]) {
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

  test("its place in the store: Listed, plain, the moment a version is published, verified or not; Not listed, once nothing of it can be installed", async () => {
    const { body } = await html("/account", await site.signIn("user_ada"));
    const cell = (id: string) => body.slice(body.indexOf(`<span class="id">${id}</span>`)).match(/<td class="listing">(.*?)<\/td>/)![1]!;
    const newest = (id: string) => body.slice(body.indexOf(`<span class="id">${id}</span>`)).match(/<td class="newest">(.*?)<\/td>/)![1]!;
    expect(cell("focus-timer")).toBe('<span class="steady">Listed</span>');
    // (Amendment 45: a first version is listed at once; its newest says it isn't verified yet, or a reviewer didn't verify it.)
    expect(cell("first-light")).toBe('<span class="steady">Listed</span>');
    expect(newest("first-light")).toContain(">Not verified yet<");
    expect(cell("rough-draft")).toBe('<span class="steady">Listed</span>');
    expect(newest("rough-draft")).toContain(">Not verified<");
    expect(cell("clipboard-sync")).toContain(">Not listed<");
    expect(newest("clipboard-sync")).toContain(">Taken down<");
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
    expect(page).toContain('href="zeron://open/plugin/worst-case-listing-at-every-limit-xyz12" data-install-link');
    expect(page).toContain('<a href="zeron://open/plugin/worst-case-listing-at-every-limit-xyz12?version=1.0.0">Install 1.0.0 instead</a>');
    const older = (await html("/plugins/worst-case-listing-at-every-limit-xyz12/versions/1.0.0")).body;
    expect(older).toContain('href="zeron://open/plugin/worst-case-listing-at-every-limit-xyz12?version=1.0.0" data-install-link');
    expect(older).toContain("Install 1.0.0 in Zeron");
    // (Its newest's page: the plain link.)
    expect((await html("/plugins/worst-case-listing-at-every-limit-xyz12/versions/1.5.0")).body).toContain('href="zeron://open/plugin/worst-case-listing-at-every-limit-xyz12" data-install-link');
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
    // (33 characters and a "+": the app's link carries 32 at most.)
    const long = "1.0.0-rc.1+build.20261118.0123456";
    try {
      await site.fake("rename", { id: "worst-case-listing-at-every-limit-xyz12", from: "1.0.0", to: long });
      const cookie = await site.signIn("user_kai");
      const page = (await html("/plugins/worst-case-listing-at-every-limit-xyz12", cookie)).body;
      expect(page).not.toContain(`Install ${long} instead`);
      expect(page).not.toContain("?version=");
      // (Its own page: no version is that long, so no address names one: the site's 404.)
      expect((await html(`/plugins/worst-case-listing-at-every-limit-xyz12/versions/${encodeURIComponent(long)}`, cookie)).response.status).toBe(404);
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
    expect(body).toContain("You haven't published a plugin yet");
    expect(body).toContain("<code tabindex=\"0\">zeron plugin publish</code>");
    expect(body).toMatch(/Your plugins<span class="title-count"> · 0<\/span>/);
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
    // Its keys are the account's now: the tab is a way to them.
    expect(body).toContain('<a href="/account/keys">Deploy keys</a>');
  });

  test("in public, a version that takes no new installs says why: replaced before it was verified, or stopped by its author", async () => {
    const superseded = await html("/plugins/worst-case-listing-at-every-limit-xyz12/versions/1.4.0");
    expect(superseded.body).toContain("Replaced by 1.5.0 before it was verified. Those who have it keep it working.");
    // (The app's sentence, its newest verified listed version a link to it: 1.0.0, its author having stopped 1.1.0.)
    expect(superseded.body).toContain('<b>1.4.0 isn&#39;t verified yet. <a href="/plugins/worst-case-listing-at-every-limit-xyz12/versions/1.0.0">1.0.0</a> is.</b>');
    // (Never the reviewer's reason: that is its author's to read.)
    expect(superseded.body).not.toContain("Until then this one stays listed");
    expect((await html("/plugins/worst-case-listing-at-every-limit-xyz12/versions/1.1.0")).body).toContain("Its author stopped new installs of it.");
  });

  test("the account's deploy keys: each with what it may publish; keys short, the gone ones apart", async () => {
    const { body } = await html("/account/keys", await site.signIn("user_ada"));
    // Eight working (one Ada made for focus-timer, seven of the worst case's), four gone in their group.
    expect(body.match(/data-dialog="revoke-/g)).toHaveLength(8);
    expect(body).toMatch(/<details class="gone"><summary>Revoked keys · 4/);
    for (const m of body.matchAll(/<span class="line key" title="([^"]+)">([^<]+)<\/span>/g)) expect(m[2]).toBe(`${m[1]!.slice(0, 6)}…${m[1]!.slice(-4)}`);
    // What each may publish: all of yours, or the plugins it names, by their names.
    expect(body).toContain("May publish: All your plugins, those you publish later too");
    expect(body).toContain('<a href="/account/plugins/focus-timer">Focus timer</a>');
    // Ending within 14 days: the warning's ink and its days in words, never colour alone.
    expect([...body.matchAll(/<span class="line soon" title="Ends in (\d+) days?">.*?<span class="sr-only">, Ends in \1 days?<\/span>/g)].length).toBeGreaterThan(0);
    expect(body).toContain('<dialog class="sheet" id="new-key"');
  });

  test("revoking a key asks first, then the key stops", async () => {
    const ada = await site.signIn("user_ada");
    const key = "k5Yh3xQ2b8VtYwGm1rPz0nLc4dEa6sJu9oKi7fBg2Wq";
    const ask = await html(`/account/keys/${key}`, ada);
    expect(ask.body).toContain("Revoke GitHub Actions?");
    const response = await post(`/account/keys/${key}`, ada, { csrf: tokenOf(ask.body) });
    expect(response.headers.get("location")).toBe("/account/keys");
    expect(flashOf(response)).toBe("__Host-zeron_flash=key_revoked");
    expect((await html(`/account/keys/${key}`, ada)).response.status).toBe(404);
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
    expect((await html("/admin/plugins/focus-timer/1.3.0-beta.1", await site.signIn("user_grace"))).body).toContain("Not verified yet");
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
    expect(overview.body).toContain("Notes for 1.1.0");
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
    expect(page.body).toContain("Claim a username first.");
    const response = await post("/auth/web/logout", lin, { csrf: tokenOf(page.body) });
    expect(response.status).toBe(303);
    // (As the edge does: always the front page, absolute.)
    expect(response.headers.get("location")).toBe("https://zeron.sh/");
    expect((await site.fetch("/account", { headers: { cookie: lin } })).status).toBe(303);
  });
});

describe("Discover", () => {
  test("Featured draws covers only when every featured plugin has a picture; one without, and the row is compact, one height", async () => {
    const withCovers = (await html("/plugins")).body;
    expect(withCovers).toContain('<ul class="featured-row" role="list">');
    expect(withCovers.match(/<img class="feature-picture"/g)).toHaveLength(3);
    try {
      await site.fake("bare", { id: "pr-radar" });
      // (A minute's cache: another address of the same page, a fresh one.)
      const { body } = await html("/plugins", await site.signIn("user_kai"));
      expect(body).toContain('<ul class="featured-row compact" role="list">');
      expect(body).not.toContain("feature-picture");
    } finally {
      await site.fake("bare");
    }
  });

  test("its lists say See all the one way", async () => {
    const { body } = await html("/plugins");
    expect(body.match(/>See all<span class="glyph/g)).toHaveLength(2);
  });
});

describe("the staff's side: the queue, a submission, the decision", () => {
  /** The rows of a table, each its cells' text. */
  const rowsOf = (body: string, table: string) => {
    const at = body.indexOf(`class="table ${table}`);
    const tbody = body.slice(body.indexOf("<tbody>", at), body.indexOf("</tbody>", at));
    return tbody.split("<tr").slice(1);
  };

  test("the queue: a table, reported first, then the longest waiting, its count in the title; a badge only where a row needs a look", async () => {
    const grace = await site.signIn("user_grace");
    const { body } = await html("/admin/plugins", grace);
    const rows = rowsOf(body, "comfortable queue");
    const total = Number(body.match(/Queue<span class="title-count"> · (\d+)<\/span>/)![1]);
    expect(rows).toHaveLength(total);
    // (Amendment 45: plugins with open reports first, then the oldest.)
    const reported = rows.map((r) => /title="Open reports on this plugin/.test(r));
    expect(reported.some(Boolean)).toBe(true);
    expect(reported).toEqual([...reported].sort((a, b) => Number(b) - Number(a)));
    for (const part of [rows.filter((_, i) => reported[i]), rows.filter((_, i) => !reported[i])]) {
      const sent = part.map((r) => Date.parse(r.match(/<time datetime="([^"]+)"/)![1]!));
      expect(sent).toEqual([...sent].sort((a, b) => a - b));
    }
    // An update that asks for more (here a far-reaching ask among them): the danger's badge.
    const worst = rows.find((r) => r.includes("worst-case-listing-at-every-limit-xyz12"))!;
    expect(worst).toContain("Update");
    // (Against its newest verified version, 1.1.0: verified, then stopped by its author.)
    expect(worst).toMatch(/<span class="badge sm bad" title="New since 1\.1\.0: [^"]*messages\.send[^"]*">Asks for more<\/span>/);
    // Nothing new, but a host swapped inside an ask it had: "Asks changed".
    const xss = rows.find((r) => r.includes(">xss-probe<"))!;
    expect(xss).toMatch(/<span class="badge sm warn" title="What it asks for changed inside since 1\.0\.0">Asks changed<\/span>/);
    // A new plugin asks for everything it asks for: said by its kind, not a badge.
    const first = rows.find((r) => r.includes(">first-light<"))!;
    expect(first).toContain("None verified yet");
    expect(first).not.toContain("badge");
  });

  test("an empty queue says so; forty fit one page, with no pager", async () => {
    const grace = await site.signIn("user_grace");
    try {
      await site.queueAs("empty");
      const empty = await html("/admin/plugins", grace);
      expect(empty.body).toContain("Nothing to verify");
      expect(empty.body).toMatch(/Queue<span class="title-count"> · 0<\/span>/);
      await site.queueAs(40);
      const forty = await html("/admin/plugins", grace);
      expect(rowsOf(forty.body, "comfortable queue")).toHaveLength(40);
      expect(forty.body).not.toContain('class="pager"');
    } finally {
      await site.queueAs(null);
    }
  });

  test("a version to verify: what it asks for against the newest verified version, what moved first, the risky in the danger's ink", async () => {
    const { body } = await html("/admin/plugins/worst-case-listing-at-every-limit-xyz12/1.5.0", await site.signIn("user_grace"));
    expect(body).toContain("Against 1.1.0, the newest verified version: 2 new asks, 2 asks changed inside, its MCP server, its service and a setting that grants changed.");
    const rows = body.slice(body.indexOf('class="weigh"'), body.indexOf('id="checks"')).split('class="weigh-row').slice(1);
    const titles = rows.map((r) => r.match(/<p class="title">([^<]+)/)![1]!.trim());
    expect(titles.slice(0, 4)).toEqual(["Send you notifications", "Connect to these places", "Run python3, git, jq, bash on your computer as you", "Send messages to your agents as you"]);
    expect(rows[0]).toContain('<span class="badge sm info">New</span>');
    expect(rows[2]).toMatch(/^ risky"/);
    expect(rows[2]).toContain('<span class="badge sm info">Changed</span>');
    expect(rows[2]).toMatch(/<span class="plus" aria-hidden="true">\+<\/span><span>bash/);
    expect(rows[1]).toMatch(/<span class="minus" aria-hidden="true">−<\/span><span class="t-3">\*/);
    // Then what it binds past its asks (amendment 41), risky, its MCP server and service whole, before and after.
    expect(titles.slice(4, 7)).toEqual(["Its MCP server changed", "Its service changed", "A setting that grants access changed"]);
    expect(rows.slice(4, 7).every((r) => r.startsWith(" risky"))).toBe(true);
    expect(rows[4]).toMatch(/<dt>Before<\/dt><dd><span>node server\.js<\/span><span>API_URL=https:\/\/api\.[^<]+<\/span><\/dd>/);
    expect(rows[4]).toContain("<span>node server.js --listen 0.0.0.0:7777</span>");
    expect(rows[5]).toContain("<dd><span>services/sync.luau</span></dd>");
    // Then what is as before, quieter.
    expect(rows.slice(7).every((r) => r.startsWith(" same"))).toBe(true);
  });

  test("the decision: Verify, the one primary, asks once with what there is to weigh; Don't verify asks for a reason and words, showing the author's view; Take down and Remove ask why; a verified version's are only those two", async () => {
    const grace = await site.signIn("user_grace");
    const pending = (await html("/admin/plugins/worst-case-listing-at-every-limit-xyz12/1.5.0", grace)).body;
    const rail = pending.slice(pending.indexOf('<aside class="decide'), pending.indexOf("</aside>"));
    expect(rail.match(/btn primary/g)).toHaveLength(1);
    expect(rail).toContain('<a class="btn primary" href="/admin/plugins/worst-case-listing-at-every-limit-xyz12/1.5.0/verify" data-dialog="approve">Verify…</a>');
    const ask = pending.slice(pending.indexOf('<dialog class="dialog sm" id="approve"'), pending.indexOf("</dialog>", pending.indexOf('id="approve"')));
    expect(ask).toContain("Everyone installing it sees it&#39;s verified");
    expect(ask).toContain('<p class="weigh-line bad">2 new asks · 2 asks changed · Its MCP server, its service and a setting that grants changed · 1 check to look at</p>');
    // (Verify has the focus: a clean update is a click, then Enter.)
    expect(ask).toContain('<button class="btn primary" type="submit" autofocus>Verify</button>');
    expect(rail).toContain("<dt>This author</dt><dd>");
    expect(rail).toContain('data-dialog="reject">Don\'t verify…</a>');
    // (Listed already: a harmful or misplaced one is taken down or removed whether or not it's decided.)
    expect(rail).toContain('data-dialog="take-down">Take down…</a>');
    expect(rail).toContain('data-dialog="remove">Remove…</a>');
    expect(pending).toContain('<select id="reject-reason" name="reason" required>');
    expect(pending).toContain('<option value="" disabled selected>Choose a reason</option>');
    expect(pending).toContain("What @ada reads");
    // (The raw manifest, folded.)
    expect(pending).toMatch(/<details class="fold">\s*<summary>/);
    const verified = (await html("/admin/plugins/focus-timer/1.2.0", grace)).body;
    const verifiedRail = verified.slice(verified.indexOf('<aside class="decide'), verified.indexOf("</aside>"));
    expect(verifiedRail).toContain('data-dialog="take-down">Take down…</a>');
    expect(verifiedRail).toContain('data-dialog="remove">Remove…</a>');
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
    const rail = before.slice(before.indexOf('<aside class="decide'), before.indexOf("</aside>"));
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
    expect(first.body).toContain("<blockquote>");
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
    for (const host of ["https://comet.zeron.sh", "https://comet-landing.someone.workers.dev"]) {
      for (const path of ["/plugins/all?sort=new", "/account", "/admin/plugins", "/auth/web/login"]) {
        const response = await site.fetch(`${host}${path}`);
        expect(response.status, `${host}${path}`).toBe(path.startsWith("/admin") ? 404 : 301);
        if (!path.startsWith("/admin")) expect(response.headers.get("location")).toBe(`https://zeron.sh${path}`);
      }
      expect((await site.fetch(`${host}/`)).status).toBe(200);
    }
  });
});
